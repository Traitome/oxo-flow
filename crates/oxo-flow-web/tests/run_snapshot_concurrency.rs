//! Admission policy for create-runs (issue #1130, audit RB-02).
//!
//! Three contracts share one binary so they use one isolated workspace and
//! one stub CLI:
//!
//! - two simultaneous `POST /api/runs` for one saved pipeline admit exactly
//!   one run (`409 RUN_NOT_TERMINAL` for the loser);
//! - the admission slot is keyed on the saved pipeline globally, so a second
//!   identity creating from a shared (`workspace`-visibility) pipeline gets
//!   the same 409 while the owner's run is live;
//! - a validation-rejected create leaves neither a run row nor an orphan
//!   `runs/<uuid>/` directory.
//!
//! `OXO_FLOW_BIN` and `OXO_FLOW_WORKSPACE` are process-global, so the
//! environment is initialized exactly once and the tests hold a process-wide
//! mutex: they mutate shared counters and compare workspace snapshots.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

mod common;

const WORKFLOW: &str = r#"
[workflow]
name = "rb02-concurrent"
version = "1.0.0"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

/// A workflow whose `[[include]]` cannot resolve on disk: Create Run answers
/// 400 from the resolved preflight, after the row would have been persisted
/// in the pre-#1130 ordering.
const INVALID_TOML: &str = r#"
[workflow]
name = "rb02-rejected-create"

[[include]]
path = "modules/missing.oxoflow"
"#;

/// Serializes the tests: the workspace snapshot diff and the shared rate
/// limiter are process-wide state.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const ALICE_TOKEN: &str = "rb02-alice-token";
const BOB_TOKEN: &str = "rb02-bob-token";

/// Hermetic workspace + stub CLI, initialized exactly once per process.
/// SAFETY: every test initializes before its first workspace read, and the
/// `OnceLock` runs the `set_var` calls on one thread while the others block.
fn init_env() -> &'static Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let base = std::env::var("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let root = base.join(format!("run-admission-{}", std::process::id()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace root");

        let stub = root.join("oxo-flow-sleep-stub");
        write_sleep_stub(&stub);
        unsafe {
            std::env::set_var("OXO_FLOW_WORKSPACE", &workspace);
            std::env::set_var("OXO_FLOW_BIN", &stub);
            std::env::set_var(
                "OXO_FLOW_AI_CONFIG",
                root.join("hermetic/.oxo-flow/ai_config.json"),
            );
        }
        oxo_flow_web::domains::execution::handlers::set_run_rate_limit_for_tests(0);
        workspace
    })
}

/// Stay alive long enough that a competing request cannot observe the
/// admitted run as terminal, but short enough that a leaked child is
/// harmless.
fn write_sleep_stub(stub: &Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(stub, "#!/bin/sh\nsleep 5\nexit 0\n").expect("write stub");
    let mut perms = std::fs::metadata(stub)
        .expect("stub metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(stub, perms).expect("chmod stub");
}

fn app(mode: &str) -> axum::Router {
    server::build_router(mode)
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn pool() -> &'static sqlx::SqlitePool {
    oxo_flow_web::infra::db::sqlite::pool()
}

async fn wait_terminal(run_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool())
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

/// Names of the run directories under one user's `runs/` root.
fn run_dir_names(workspace: &Path, user_id: &str) -> BTreeSet<String> {
    let root = workspace.join("users").join(user_id).join("runs");
    match std::fs::read_dir(&root) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
        Err(e) => panic!("read {}: {e}", root.display()),
    }
}

/// Seed a user and a bearer session (team-mode tests).
async fn seed_identity(token: &str, id: &str, username: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
         VALUES (?, ?, 'user', 'password', ?)",
    )
    .bind(id)
    .bind(username)
    .bind(&now)
    .execute(pool())
    .await
    .expect("seed user");
    sqlx::query(
        "INSERT OR REPLACE INTO sessions (token, user_id, created_at, expires_at) \
         VALUES (?, ?, ?, '2999-01-01T00:00:00Z')",
    )
    .bind(token)
    .bind(id)
    .bind(&now)
    .execute(pool())
    .await
    .expect("seed session");
}

#[tokio::test]
async fn concurrent_creates_from_one_saved_pipeline_admit_exactly_one() {
    let workspace = init_env().to_path_buf();
    let _serial = SERIAL.lock().await;
    common::ensure_db().await;
    let app = app("personal");

    let before = run_dir_names(&workspace, "default");

    let (status, body) = send(
        &app,
        "POST",
        "/api/pipelines",
        None,
        Some(json!({"name": "rb02-concurrent", "toml_content": WORKFLOW})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let pipeline = body["id"].as_str().expect("pipeline id").to_string();

    let request = || {
        let app = app.clone();
        let pipeline = pipeline.clone();
        async move {
            send(
                &app,
                "POST",
                "/api/runs",
                None,
                Some(json!({
                    "toml_content": WORKFLOW,
                    "pipeline_id": pipeline,
                    "max_jobs": 1,
                })),
            )
            .await
        }
    };
    let (a, b) = tokio::join!(request(), request());
    let responses = [a, b];

    let admitted: Vec<&(StatusCode, Value)> = responses
        .iter()
        .filter(|(status, _)| *status == StatusCode::OK)
        .collect();
    let rejected: Vec<&(StatusCode, Value)> = responses
        .iter()
        .filter(|(status, _)| *status == StatusCode::CONFLICT)
        .collect();
    assert_eq!(
        (admitted.len(), rejected.len()),
        (1, 1),
        "exactly one create must be admitted and one rejected: {responses:?}"
    );
    assert_eq!(
        rejected[0].1["code"], "RUN_NOT_TERMINAL",
        "the loser must report the documented conflict: {responses:?}"
    );

    let run_id = admitted[0].1["run_id"]
        .as_str()
        .expect("run id")
        .to_string();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE pipeline_id = ?")
        .bind(&pipeline)
        .fetch_one(pool())
        .await
        .expect("count runs");
    assert_eq!(count, 1, "only the admitted request may persist a run row");

    // Exactly one NEW run directory exists; the rejected request left none.
    let new_dirs: Vec<String> = run_dir_names(&workspace, "default")
        .difference(&before)
        .cloned()
        .collect();
    assert_eq!(
        new_dirs,
        vec![run_id.clone()],
        "only the admitted run may leave a workdir"
    );

    // Release the guard: the run is cancelled, then a new create is admitted.
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/runs/{run_id}/cancel"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    wait_terminal(&run_id).await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/runs",
        None,
        Some(json!({
            "toml_content": WORKFLOW,
            "pipeline_id": pipeline,
            "max_jobs": 1,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a terminal run must release the pipeline admission guard: {body}"
    );
}

/// F5 (review of #1130): the saved-pipeline slot is global to the pipeline,
/// not per user. A shared (`workspace`-visibility) pipeline admits one live
/// run regardless of which reader creates the next one, and the conflict is
/// the documented 409 — not a hidden 404.
#[tokio::test]
async fn shared_pipeline_slot_is_global_across_identities() {
    let workspace = init_env().to_path_buf();
    let _serial = SERIAL.lock().await;
    common::ensure_db().await;
    seed_identity(ALICE_TOKEN, "rb02-alice", "rb02-alice").await;
    seed_identity(BOB_TOKEN, "rb02-bob", "rb02-bob").await;
    let app = app("team");

    let (status, body) = send(
        &app,
        "POST",
        "/api/pipelines",
        Some(ALICE_TOKEN),
        Some(json!({
            "name": "rb02-shared",
            "toml_content": WORKFLOW,
            "visibility": "workspace",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let pipeline = body["id"].as_str().expect("pipeline id").to_string();

    // Bob really can read the shared pipeline — otherwise the 409 below
    // could be a masked permission denial.
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/pipelines/{pipeline}"),
        Some(BOB_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = send(
        &app,
        "POST",
        "/api/runs",
        Some(ALICE_TOKEN),
        Some(json!({
            "toml_content": WORKFLOW,
            "pipeline_id": pipeline,
            "max_jobs": 1,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let run_id = body["run_id"].as_str().expect("run id").to_string();

    let (status, body) = send(
        &app,
        "POST",
        "/api/runs",
        Some(BOB_TOKEN),
        Some(json!({
            "toml_content": WORKFLOW,
            "pipeline_id": pipeline,
            "max_jobs": 1,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a second identity must not start a second live run of the shared pipeline: {body}"
    );
    assert_eq!(body["code"], "RUN_NOT_TERMINAL", "{body}");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE pipeline_id = ?")
        .bind(&pipeline)
        .fetch_one(pool())
        .await
        .expect("count runs");
    assert_eq!(count, 1, "only the owner's run may be persisted");

    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/runs/{run_id}/cancel"),
        Some(ALICE_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    wait_terminal(&run_id).await;
    assert!(
        workspace.join("users").join("rb02-alice").exists(),
        "the owner's run workdir must exist under their own workspace root"
    );
}

/// F2 (review of #1130): a create rejected by validation must leave no run
/// row and no orphan `runs/<uuid>/` directory.
#[tokio::test]
async fn validation_rejected_create_leaves_no_run_directory() {
    let workspace = init_env().to_path_buf();
    let _serial = SERIAL.lock().await;
    common::ensure_db().await;
    let app = app("personal");

    let before = run_dir_names(&workspace, "default");

    let (status, body) = send(
        &app,
        "POST",
        "/api/runs",
        None,
        Some(json!({"toml_content": INVALID_TOML, "dry_run": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "RUN_ERROR", "{body}");

    let persisted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runs WHERE workflow_name = 'rb02-rejected-create'",
    )
    .fetch_one(pool())
    .await
    .expect("count runs");
    assert_eq!(persisted, 0, "rejected workflow must not create a run row");

    assert_eq!(
        run_dir_names(&workspace, "default"),
        before,
        "a rejected create must not leave an orphan run directory"
    );
}
