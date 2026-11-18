//! Create-run quota reservation books the RESOLVED (include-merged) rule set
//! (issue #1152 review, finding F1).
//!
//! `POST /api/runs` admits a run and records its resource reservation
//! (`admit_spawn_budget` → `quota_reservations`) from the *host* TOML text
//! before the fix: for a staged-include workflow whose host rule `extends` a
//! rule that lives only inside a staged `[[include]]`, the host text does not
//! parse on its own, so `quota_usage_for` falls back to `(1, 0)` and the run
//! is admitted at one thread / zero MB while it executes the merged rule set.
//!
//! The reservation must instead be charged from the resolved config — the
//! same `from_file` load the plan and admission gate already use — exactly as
//! retry and resume do.
//!
//! This binary pins `OXO_FLOW_BIN` to a stub that blocks until released, so
//! the winner's reservation is still held while the test reads it. The env
//! writes are the reason this is a dedicated binary: mutating `OXO_FLOW_BIN`
//! in a shared test binary would race its sibling tests (the real CLI they
//! depend on is replaced).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::infra::quota::QuotaConfig;
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

/// The staged base declares `threads = 5, memory = "6G"`; the host rule
/// `extends` it and inherits both. The resolved rule set therefore charges
/// 10 threads and 12288 MB — distinct from the `(1, 0)` host-string fallback
/// the pre-fix path booked.
const BASE: &str = r#"
[workflow]
name = "base"
version = "1.0.0"

[[rules]]
name = "base_align"
threads = 5
memory = "6G"
output = ["base_align.txt"]
shell = "echo base > {output}"
"#;

const CHILD: &str = r#"
[workflow]
name = "extends-include"
version = "1.0.0"

[[include]]
path = "base.oxoflow"

[[rules]]
name = "child_align"
extends = "base_align"
output = ["child_align.txt"]
shell = "echo child > {output}"
"#;

const RESOLVED_THREADS: i64 = 10;
const RESOLVED_MEMORY_MB: i64 = 12288;

#[tokio::test]
async fn staged_include_create_run_reserves_the_resolved_quota() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root =
        std::path::PathBuf::from(tmp).join(format!("quota-create-resv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create scratch root");
    let release_file = root.join("release-stub");
    let stub = root.join("oxo-flow-stub");
    write_blocking_stub(&stub, &release_file);

    // This binary's single test owns the process environment.
    // SAFETY: set before the router (and therefore any spawn) exists; no
    // other thread reads these variables.
    unsafe {
        std::env::set_var("OXO_FLOW_WORKSPACE", root.join("workspace"));
        std::env::set_var("OXO_FLOW_BIN", &stub);
        std::env::set_var(
            "OXO_FLOW_AI_CONFIG",
            root.join("hermetic/.oxo-flow/ai_config.json"),
        );
    }
    common::ensure_db().await;
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);

    let backend = oxo_flow_web::infra::db::global_backend().expect("global backend");
    // Generous limits: the point is the recorded reservation, not a breach.
    backend
        .save_quota_config(&QuotaConfig {
            max_concurrent_runs: 100,
            ..QuotaConfig::default()
        })
        .await
        .expect("persist limits");

    let app = server::build_router("personal");
    let (status, body) = post_run(
        &app,
        json!({
            "toml_content": CHILD,
            "include_files": { "base.oxoflow": BASE },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create run failed: {body}");
    let run_id = body["run_id"].as_str().expect("run_id").to_string();

    // The stub has not been released, so the run is still active and its
    // reservation still held. Read the durable reservation row directly.
    let (threads, memory_mb): (i64, i64) =
        sqlx::query_as("SELECT threads, memory_mb FROM quota_reservations WHERE run_id = ?")
            .bind(&run_id)
            .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
            .await
            .expect("the admitted run owns a reservation row");

    assert_eq!(
        threads, RESOLVED_THREADS,
        "the reservation must charge the RESOLVED thread count, not the (1, 0) host fallback"
    );
    assert_eq!(
        memory_mb, RESOLVED_MEMORY_MB,
        "the reservation must charge the RESOLVED memory, not the (1, 0) host fallback"
    );

    // Let the stub finish so the terminal path releases the reservation.
    std::fs::write(&release_file, b"go").expect("release stub");
    wait_terminal(&run_id).await;
    let (remaining,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM quota_reservations WHERE run_id = ?")
            .bind(&run_id)
            .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
            .await
            .expect("count reservations");
    assert_eq!(remaining, 0, "the terminal path released the reservation");
}

/// A stub `oxo-flow` that blocks until `release` exists, then exits 0 —
/// keeps the run active (and its reservation held) while the test reads it.
fn write_blocking_stub(stub: &std::path::Path, release: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        "#!/bin/sh\nwhile [ ! -f '{}' ]; do sleep 0.02; done\nexit 0\n",
        release.display()
    );
    std::fs::write(stub, script).expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

async fn post_run(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("router response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

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
