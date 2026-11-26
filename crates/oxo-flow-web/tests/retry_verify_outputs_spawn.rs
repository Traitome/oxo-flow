//! `POST /api/runs/{id}/retry` spawns the CLI with `--verify-outputs`
//! (issue #1426 residual): a retry continues the predecessor run in its
//! workdir, so the rules it reuses must have their recorded output
//! manifests verified instead of passing the historical size+mtime gate.
//!
//! The argv builder is unit-tested in `executor.rs`, but only an end-to-end
//! retry proves the handler sets the flag and the executor forwards it. This
//! binary points `OXO_FLOW_BIN` at a stub that records its argv and its
//! `OXO_FLOW_MANIFEST_HASH_MAX_BYTES` value, then asserts:
//!
//! 1. the retry argv carries `--verify-outputs` together with
//!    `--resume-failed`;
//! 2. a plain create argv does NOT carry it (a fresh `run` keeps the
//!    historical gate by design);
//! 3. both spawns receive the operator's manifest hash cap — the web spawn
//!    environment allowlist forwards it.
//!
//! One test per binary on purpose: `OXO_FLOW_BIN`, `OXO_FLOW_WORKSPACE`, and
//! `OXO_FLOW_MANIFEST_HASH_MAX_BYTES` are process-global, and tests within a
//! binary run on parallel threads. The stub is swapped between the two
//! spawns sequentially, after the first run has reached a terminal state, so
//! no spawn can resolve the wrong pin.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

const WORKFLOW: &str = r#"
[workflow]
name = "retry-verify-outputs"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

/// The operator-tuned content-hash cap the stub must observe.
const MANIFEST_HASH_CAP: &str = "1048576";

/// This binary contains exactly one test, so mutating process-global
/// environment cannot race another thread. SAFETY: `set_var` runs before any
/// other thread can read the value.
fn set_env(key: &str, value: &str) {
    unsafe { std::env::set_var(key, value) };
}

/// Write an executable stub `oxo-flow` that records its argv (one entry per
/// line) to `record` and its manifest-hash-cap environment value to
/// `env_record`, then exits 0 — the executor's happy path.
fn write_stub(stub: &std::path::Path, record: &std::path::Path, env_record: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' \"${{OXO_FLOW_MANIFEST_HASH_MAX_BYTES:-}}\" > '{}'\nexit 0\n",
        record.display(),
        env_record.display()
    );
    std::fs::write(stub, script).expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Wait for a stub's argv record to exist, then return it line by line.
async fn recorded_argv(record: &std::path::Path) -> Vec<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(record) {
            return text.lines().map(str::to_string).collect();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the spawned CLI never ran (no argv record at {})",
            record.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Wait for a stub's environment record, then return it verbatim.
async fn recorded_manifest_cap(record: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(record) {
            return text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the spawned CLI never wrote its environment record at {}",
            record.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Wait until the run is neither queued nor running — the executor has
/// finished spawning and reaping, so the stub pin is safe to swap.
async fn wait_terminal(run_id: &str) {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .expect("run row exists");
        if status != "queued" && status != "running" {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {run_id} never reached a terminal state (stuck at {status})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Seed the predecessor run a retry continues: a FAILED row in a workdir
/// that holds the workflow the run executed.
async fn seed_failed_run(id: &str, workdir: &std::path::Path) {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query(
        "INSERT OR REPLACE INTO runs (id, user_id, pipeline_id, pipeline_snapshot, \
         workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at) \
         VALUES (?, 'default', NULL, '', NULL, 'failed', 'executing', NULL, ?, NULL, NULL, ?)",
    )
    .bind(id)
    .bind(workdir.to_string_lossy().to_string())
    .bind("2026-01-01T00:00:00Z")
    .execute(pool)
    .await
    .expect("seed failed run");
}

#[tokio::test]
async fn retry_spawn_verifies_outputs_and_forwards_manifest_hash_cap() {
    // Arrange — isolated workspace + DB; one stub per spawn.
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root =
        std::path::PathBuf::from(tmp).join(format!("retry-verify-outputs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create scratch root");
    set_env(
        "OXO_FLOW_WORKSPACE",
        root.join("workspace").to_str().unwrap(),
    );
    // Hermetic AI config (repo test rule): a non-existent path containing
    // `.oxo-flow`. This binary never reads or writes provider config, and
    // the single test owns the process (no env races).
    set_env(
        "OXO_FLOW_AI_CONFIG",
        root.join("hermetic/.oxo-flow/ai_config.json")
            .to_str()
            .unwrap(),
    );
    // The operator knob whose forwarding is under test.
    set_env("OXO_FLOW_MANIFEST_HASH_MAX_BYTES", MANIFEST_HASH_CAP);

    let retry_stub = root.join("oxo-flow-retry");
    let retry_record = root.join("argv-retry.txt");
    let retry_env_record = root.join("env-retry.txt");
    let create_stub = root.join("oxo-flow-create");
    let create_record = root.join("argv-create.txt");
    let create_env_record = root.join("env-create.txt");

    common::ensure_db().await;
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    let app = server::build_router("personal");

    // The retry continues this workdir; the workflow document the retry's
    // plan reconstruction reads is the staged one.
    let predecessor_dir = root.join("workdirs/run-retry");
    std::fs::create_dir_all(&predecessor_dir).expect("create predecessor workdir");
    std::fs::write(predecessor_dir.join("workflow.oxoflow"), WORKFLOW)
        .expect("write staged workflow");
    seed_failed_run("run-retry", &predecessor_dir).await;

    // Act 1 — a retry must opt into strict manifest verification.
    write_stub(&retry_stub, &retry_record, &retry_env_record);
    set_env("OXO_FLOW_BIN", retry_stub.to_str().unwrap());
    let (status, body) = post(&app, "/api/runs/run-retry/retry", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let retry_run = body["new_run_id"].as_str().expect("new_run_id").to_string();
    let argv = recorded_argv(&retry_record).await;

    // Assert 1 — the strict flag rides the real retry spawn, next to the
    // checkpoint-reuse flag the retry plan promises.
    assert_eq!(argv.first().map(String::as_str), Some("run"), "{argv:?}");
    assert!(
        argv.iter().any(|a| a == "--resume-failed"),
        "a skip_succeeded retry must forward --resume-failed: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a == "--verify-outputs"),
        "a retry must verify recorded output manifests: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a == "--workdir"),
        "the retry argv must still carry the workdir: {argv:?}"
    );
    // Assert 2 — the operator's hash cap reaches the spawned CLI.
    assert_eq!(
        recorded_manifest_cap(&retry_env_record).await,
        MANIFEST_HASH_CAP,
        "the web spawn must forward OXO_FLOW_MANIFEST_HASH_MAX_BYTES"
    );
    wait_terminal(&retry_run).await;

    // Act 1b — the flag is unconditional across the retry flag mappings, so
    // the other two spawn shapes must carry it too. `skip_succeeded: false`
    // spawns `--rerun` (every rule executes; the flag is harmless).
    std::fs::remove_file(&retry_record).expect("clear first argv record");
    let (status, body) = post(
        &app,
        "/api/runs/run-retry/retry",
        json!({"skip_succeeded": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rerun_run = body["new_run_id"].as_str().expect("new_run_id").to_string();
    let argv = recorded_argv(&retry_record).await;
    assert!(
        argv.iter().any(|a| a == "--rerun"),
        "skip_succeeded: false must forward --rerun: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a == "--verify-outputs"),
        "a --rerun retry must still carry --verify-outputs: {argv:?}"
    );
    wait_terminal(&rerun_run).await;

    // Act 1c — `from_rule` reuses the completed rules outside the downstream
    // closure, so it must verify their manifests as well.
    std::fs::remove_file(&retry_record).expect("clear second argv record");
    let (status, body) = post(
        &app,
        "/api/runs/run-retry/retry",
        json!({"from_rule": "hello"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let from_run = body["new_run_id"].as_str().expect("new_run_id").to_string();
    let argv = recorded_argv(&retry_record).await;
    assert!(
        argv.iter().any(|a| a == "--from-rule"),
        "a from_rule retry must forward --from-rule: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a == "hello"),
        "a from_rule retry must name the rule: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a == "--verify-outputs"),
        "a from_rule retry must verify recorded output manifests: {argv:?}"
    );
    wait_terminal(&from_run).await;

    // Act 2 — a plain create is a fresh `run`, not a continuation.
    write_stub(&create_stub, &create_record, &create_env_record);
    set_env("OXO_FLOW_BIN", create_stub.to_str().unwrap());
    let (status, body) = post(&app, "/api/runs", json!({"toml_content": WORKFLOW})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let create_run = body["run_id"].as_str().expect("run_id").to_string();
    let argv = recorded_argv(&create_record).await;

    // Assert 3 — create keeps the historical mtime gate by design.
    assert_eq!(argv.first().map(String::as_str), Some("run"), "{argv:?}");
    assert!(
        !argv.iter().any(|a| a == "--verify-outputs"),
        "a plain create must not carry --verify-outputs: {argv:?}"
    );
    // Assert 4 — the cap is forwarded on the create path too (the allowlist
    // is spawn-wide, not retry-specific).
    assert_eq!(
        recorded_manifest_cap(&create_env_record).await,
        MANIFEST_HASH_CAP,
        "the web spawn must forward OXO_FLOW_MANIFEST_HASH_MAX_BYTES"
    );
    wait_terminal(&create_run).await;
}
