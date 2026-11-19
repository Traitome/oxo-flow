//! Tenant-boundary hardening for web-spawned runs (RB-01a / issue #1129).
//!
//! Before this change the web spawn path handed a tenant-authored rule shell
//! the server process's ENTIRE environment: `DATABASE_URL`,
//! `OXO_FLOW_MASTER_KEY`, provider credentials and MCP tokens all reached the
//! `oxo-flow` CLI the server spawned, and from there every rule shell the
//! workflow ran. `spawn_background_run_with_args` now reuses the engine's own
//! rule-shell environment policy (`apply_rule_env_allowlist`) so the child
//! sees only `PATH`/`HOME`/`LANG`/`LC_*` plus the engine's non-secret
//! configuration knobs, and installs operator-wide `RLIMIT_CPU`/`RLIMIT_NPROC`
//! ceilings alongside the existing `RLIMIT_DATA`.
//!
//! This test drives the REAL team-mode router with two seeded identities (A
//! submits, B holds private data) and a stub `oxo-flow` binary that stands in
//! for the CLI. The stub dumps its environment and its own rlimits to files in
//! the run directory, so the test observes exactly what a rule shell would
//! inherit.
//!
//! NON-VACUITY: removing the `apply_rule_env_allowlist` call from the executor
//! makes the child inherit the server environment verbatim, so the stub's dump
//! carries `OXO_FLOW_MASTER_KEY`/`DATABASE_URL` and the assertions here fail.
//! The proof is recorded in the PR body.
//!
//! HONESTY (RB-01 residual): there is NO filesystem namespace, uid boundary,
//! or per-run container on the web spawn path. A malicious workflow that
//! guesses another tenant's absolute path CAN still read (and write) it —
//! this test therefore asserts only that a run does not mutate tenant B's
//! seeded file, never that B's files are unreachable. The residual is recorded
//! in the audit ledger and the web API reference.

// One test per binary: the harness mutates process-global environment
// (`OXO_FLOW_WORKSPACE`, `OXO_FLOW_BIN`, the secret sentinels, the rlimit
// knobs), so a second parallel test would race these `set_var` calls.
// The rlimit assertions are Linux-gated (the executor's rlimit install is a
// no-op off Linux), but the environment-isolation assertions are portable.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

/// Identities and sentinels. Distinct strings so a leaked value is
/// unmistakable in the child's environment dump.
const A_TOKEN: &str = "rb01-a-token";
const A_ID: &str = "rb01-a-id";
const A_USER: &str = "rb01-a-user";
const B_TOKEN: &str = "rb01-b-token";
const B_ID: &str = "rb01-b-id";
const B_USER: &str = "rb01-b-user";

const MASTER_KEY_SENTINEL: &str = "rb01-master-key-sentinel-do-not-leak";
const DATABASE_URL_SENTINEL: &str = "postgres://rb01:rb01-db-secret@127.0.0.1/rb01";
const B_SECRET_FILE: &str = "rb01-b-private.txt";
const B_SECRET_BODY: &str = "tenant-b-original-contents";

/// A valid workflow whose rule does nothing of consequence — the stub binary
/// replaces the CLI, so the shell text only has to parse and pass admission.
const ENV_PROBE_WORKFLOW: &str = r#"
[workflow]
name = "rb01-env-probe"

[[rules]]
name = "probe"
output = ["out.txt"]
shell = "echo rb01-env-probe"
"#;

/// The same shape, but the `SPIN_CPU` marker tells the stub to burn CPU until
/// the `RLIMIT_CPU` ceiling kills it (Linux only).
#[cfg(target_os = "linux")]
const CPU_SPIN_WORKFLOW: &str = r#"
[workflow]
name = "rb01-cpu-spin"

[[rules]]
name = "spin"
output = ["out.txt"]
shell = "true # SPIN_CPU"
"#;

#[tokio::test]
async fn web_spawned_rule_shell_cannot_see_server_secrets() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root = std::path::PathBuf::from(tmp).join(format!("rb01-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace root");
    let stub = root.join("oxo-flow-stub");
    write_stub(&stub);

    // SAFETY: this binary contains exactly one test, and every `set_var` here
    // runs before the router (and therefore any spawn task) exists.
    unsafe {
        std::env::set_var("OXO_FLOW_WORKSPACE", &workspace);
        std::env::set_var("OXO_FLOW_BIN", &stub);
        std::env::set_var("OXO_FLOW_AI_CONFIG", root.join("hermetic/ai_config.json"));
        // The secrets a rule shell must never inherit. `DATABASE_URL` points
        // at a live-looking credential so a leak is grep-able, not inferred.
        std::env::set_var("OXO_FLOW_MASTER_KEY", MASTER_KEY_SENTINEL);
        std::env::set_var("DATABASE_URL", DATABASE_URL_SENTINEL);
        // Operator-wide ceilings (RB-01a). `RLIMIT_CPU=2` kills the spin run
        // within a couple of CPU-seconds; `RLIMIT_NPROC=1024` is distinctive
        // enough to assert exactly and far above this test's own process
        // count, so it never starves the harness.
        std::env::set_var("OXO_FLOW_WEB_RLIMIT_CPU_SECS", "2");
        std::env::set_var("OXO_FLOW_WEB_RLIMIT_NPROC", "1024");
    }
    common::ensure_db().await;
    oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
    let app = server::build_router("team");

    seed_identity(A_TOKEN, A_ID, A_USER, "user").await;
    seed_identity(B_TOKEN, B_ID, B_USER, "user").await;

    // Tenant B keeps a private file. Nothing A does may mutate it.
    let b_inputs = workspace.join("users").join(B_ID).join("inputs");
    std::fs::create_dir_all(&b_inputs).expect("create B inputs dir");
    let b_file = b_inputs.join(B_SECRET_FILE);
    std::fs::write(&b_file, B_SECRET_BODY).expect("seed B file");

    // ── A submits a run; the stub dumps what a rule shell would inherit ────
    let (status, body) = post_run(&app, A_TOKEN, ENV_PROBE_WORKFLOW).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create accepts the workflow: {body}"
    );
    let run_id = body["run_id"].as_str().expect("run_id").to_string();
    let run_status = wait_terminal(&run_id).await;
    assert_eq!(
        run_status, "completed",
        "the stub exits 0, so the run completes"
    );

    let run_dir =
        oxo_flow_web::workspace::get_run_directory(A_ID, &run_id).expect("run directory resolves");
    let child_env =
        std::fs::read_to_string(run_dir.join("child-env.txt")).expect("stub wrote its environment");

    // The server's secrets must be absent from the child environment.
    assert!(
        !child_env.contains(MASTER_KEY_SENTINEL),
        "OXO_FLOW_MASTER_KEY leaked into the rule shell's environment:\n{child_env}"
    );
    assert!(
        !child_env.contains("DATABASE_URL="),
        "DATABASE_URL leaked into the rule shell's environment:\n{child_env}"
    );
    // ...while the shell's own necessities survive the allowlist.
    for required in ["PATH=", "HOME="] {
        assert!(
            child_env.lines().any(|l| l.starts_with(required)),
            "{required} must still reach the rule shell:\n{child_env}"
        );
    }

    // ── Tenant B's data is untouched by A's run ───────────────────────────
    // (Not a reachability guarantee — see the module doc / residual note.)
    assert_eq!(
        std::fs::read_to_string(&b_file).expect("B file still exists"),
        B_SECRET_BODY,
        "tenant A's run must not mutate tenant B's files"
    );

    // ── Linux: the operator ceilings are installed and enforced ───────────
    #[cfg(target_os = "linux")]
    {
        let ulimits = std::fs::read_to_string(run_dir.join("child-ulimits.txt"))
            .expect("stub wrote its rlimits");
        assert!(
            ulimits.contains("CPU=2"),
            "RLIMIT_CPU must be installed at the configured ceiling:\n{ulimits}"
        );
        assert!(
            ulimits.contains("NPROC=1024"),
            "RLIMIT_NPROC must be installed at the configured ceiling:\n{ulimits}"
        );

        // A rule that spins CPU must be killed — observable failure, not a
        // pinned server core.
        let (status, body) = post_run(&app, A_TOKEN, CPU_SPIN_WORKFLOW).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "create accepts the workflow: {body}"
        );
        let spin_run = body["run_id"].as_str().expect("run_id").to_string();
        let spin_status = wait_terminal(&spin_run).await;
        assert_eq!(
            spin_status, "failed",
            "a CPU-spinning rule must be killed by RLIMIT_CPU (run reached \
             {spin_status})"
        );
    }
}

/// A stub `oxo-flow` standing in for the CLI.
///
/// It parses `--workdir` out of its argv (the web builds
/// `run <workflow> --workdir <dir> …`), writes its environment and its own
/// `ulimit -t`/`ulimit -u` readings into the run directory, and — when the
/// workflow text carries the `SPIN_CPU` marker — spins until the `RLIMIT_CPU`
/// ceiling kills it. Otherwise it exits 0.
fn write_stub(stub: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = r#"#!/bin/sh
wf=""
dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    --workdir) dir="$2"; shift 2 ;;
    *.oxoflow) wf="$1"; shift ;;
    *) shift ;;
  esac
done
if [ -n "$dir" ]; then
  env > "$dir/child-env.txt" 2>&1
  { echo "CPU=$(ulimit -t)"; echo "NPROC=$(ulimit -u)"; } > "$dir/child-ulimits.txt" 2>&1
fi
if [ -n "$wf" ] && grep -q SPIN_CPU "$wf" 2>/dev/null; then
  while :; do :; done
fi
exit 0
"#;
    std::fs::write(stub, script).expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

async fn seed_identity(token: &str, id: &str, username: &str, role: &str) {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
         VALUES (?, ?, ?, 'password', ?)",
    )
    .bind(id)
    .bind(username)
    .bind(role)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) \
         VALUES (?, ?, ?, '2999-01-01T00:00:00Z')",
    )
    .bind(token)
    .bind(id)
    .bind(&now)
    .execute(pool)
    .await
    .unwrap();
}

async fn post_run(app: &axum::Router, token: &str, toml: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"toml_content": toml}).to_string()))
                .unwrap(),
        )
        .await
        .expect("router response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn wait_terminal(run_id: &str) -> String {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .expect("run row exists");
        if status != "queued" && status != "running" {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {run_id} never reached a terminal state (stuck at {status})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
