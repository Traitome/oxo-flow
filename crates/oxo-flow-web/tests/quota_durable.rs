//! Durable, atomic quota admission (RB-09 / issue #1133).
//!
//! Pins the three exit-evidence properties that do not need an HTTP client:
//!
//! 1. admission is atomic — N concurrent admissions against a limit of K
//!    yield exactly K approvals, even across two backend instances;
//! 2. the counters are durable and shared — a second instance (and a
//!    simulated restart) sees the state the first wrote, and a release from
//!    one is visible to the other;
//! 3. at the request layer, two concurrent `POST /api/runs` against
//!    `max_concurrent_runs = 1` produce exactly one `200` and one
//!    `429 QUOTA_EXCEEDED` (the pre-RB-09 check-then-record pair let both
//!    pass).
//!
//! The OS memory-ceiling half of the evidence lives in
//! `quota_memory_enforcement.rs` (Linux-gated) so this binary's env writes
//! cannot race that one.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::infra::db::StorageBackend;
use oxo_flow_web::infra::db::sqlite::SqliteBackend;
use oxo_flow_web::infra::quota::{self, QuotaConfig};
use oxo_flow_web::server;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Storage-level: atomicity, durability, cross-instance visibility, restart
/// persistence, idempotent release, and stale-reservation reconciliation.
#[tokio::test]
async fn quota_state_is_atomic_durable_and_shared_across_instances() {
    let dir = tempfile::tempdir().expect("temp dir");
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("quota.db").display());

    // Two independent pools on the same database file — the same shape as
    // two server instances (or a restart) sharing one durable store.
    let instance_a = SqliteBackend::new(&url).await.expect("backend A");
    instance_a.init().await.expect("schema A");
    let instance_b = SqliteBackend::new(&url).await.expect("backend B");
    instance_b.init().await.expect("schema B");

    let limits = QuotaConfig {
        max_concurrent_runs: 2,
        max_total_threads: 16,
        max_total_memory_mb: 8192,
        max_runs_per_day: 50,
    };
    // Persisting the limits is idempotent (RB-09 schema bootstrap re-runs it
    // on every boot) and makes instance B enforce the same numbers.
    instance_a
        .save_quota_config(&limits)
        .await
        .expect("save limits");
    instance_b
        .save_quota_config(&limits)
        .await
        .expect("re-save limits");

    // ── Atomicity under concurrency ─────────────────────────────────────
    // Eight simultaneous admissions (four per instance) against a limit of
    // two: exactly two may win. The old separate check()/record_start()
    // pair let every request pass the check.
    let a: Arc<dyn StorageBackend> = Arc::new(instance_a);
    let b: Arc<dyn StorageBackend> = Arc::new(instance_b);
    let now = 1_800_000_000_000i64;
    let mut handles = Vec::new();
    for i in 0..8 {
        let backend = if i % 2 == 0 { a.clone() } else { b.clone() };
        let limits = limits.clone();
        let run_id = format!("run-{i}");
        handles.push(tokio::spawn(async move {
            backend
                .quota_admit(&run_id, "user-1", 1, 64, &limits, now)
                .await
                .expect("admission call")
                .0
        }));
    }
    let mut allowed = 0;
    for handle in handles {
        if handle.await.expect("join") {
            allowed += 1;
        }
    }
    assert_eq!(
        allowed, 2,
        "exactly max_concurrent_runs admissions may pass under concurrency"
    );

    // ── Shared + durable ────────────────────────────────────────────────
    let usage = b.quota_usage("user-1", now).await.expect("usage B");
    assert_eq!(usage.active_runs, 2, "instance B sees instance A's writes");
    assert_eq!(usage.used_threads, 2);
    assert_eq!(usage.used_memory_mb, 128);

    // Simulated restart: a brand-new backend (new pool, new process would be
    // equivalent) reads the same durable counters.
    let restarted = SqliteBackend::new(&url).await.expect("restarted backend");
    restarted.init().await.expect("schema after restart");
    let usage = restarted
        .quota_usage("user-1", now)
        .await
        .expect("usage after restart");
    assert_eq!(usage.active_runs, 2, "restart preserves quota usage");
    assert_eq!(usage.runs_today, 2, "the daily counter survives restart");

    // Concurrent double-release through two instances: exactly one call may
    // decrement (the DELETE ... RETURNING gate), and the result is visible
    // through both instances.
    let winners = restarted
        .quota_stale_reservations()
        .await
        .expect("stale reservations");
    assert_eq!(winners.len(), 2, "both admitted runs own a reservation");
    let target = winners[0].clone();
    let (ra, rb) = tokio::join!(a.quota_release(&target), b.quota_release(&target));
    let released =
        usize::from(ra.expect("release via A")) + usize::from(rb.expect("release via B"));
    assert_eq!(released, 1, "exactly one concurrent release decrements");
    assert!(!b.quota_release(&target).await.expect("later release"));
    let usage = a.quota_usage("user-1", now).await.expect("usage A");
    assert_eq!(usage.active_runs, 1, "release is shared across instances");
    assert_eq!(
        usage.runs_today, 2,
        "completion never erases the daily count"
    );

    // ── Startup reconciliation ─────────────────────────────────────────
    // Neither admitted run id has a runs row, so both reservations are
    // stale: reconciliation releases the remaining one exactly once.
    let released = quota::reconcile_stale(a.as_ref()).await.expect("reconcile");
    assert_eq!(released, 1, "only the still-held reservation is released");
    let usage = a.quota_usage("user-1", now).await.expect("final usage");
    assert_eq!(usage.active_runs, 0, "no leaked active run after reconcile");
}

/// A user's FIRST admission is gated by the same limits as every later one.
/// The durable upsert's `DO UPDATE … WHERE` only filters the update path —
/// the fresh-row INSERT used to run unconditionally, admitting an
/// over-limit first run that `quota::evaluate` and the no-backend fallback
/// both reject (limit semantics mirrored here from `evaluate`).
#[tokio::test]
async fn first_admission_over_limit_is_rejected_like_every_other() {
    let dir = tempfile::tempdir().expect("temp dir");
    let url = format!("sqlite:{}?mode=rwc", dir.path().join("quota.db").display());
    let backend = SqliteBackend::new(&url).await.expect("backend");
    backend.init().await.expect("schema");

    let limits = QuotaConfig {
        max_concurrent_runs: 4,
        max_total_threads: 2,
        max_total_memory_mb: 512,
        max_runs_per_day: 5,
    };
    backend
        .save_quota_config(&limits)
        .await
        .expect("save limits");
    let now = 1_800_000_000_000i64;

    // Fresh user, threads over the total limit: must be rejected, not
    // admitted into a first row that never existed to guard the update.
    let (allowed, _) = backend
        .quota_admit("run-over-threads", "fresh-user", 99, 64, &limits, now)
        .await
        .expect("admission call");
    assert!(
        !allowed,
        "an over-limit FIRST admission must be rejected like any other"
    );

    // Same for memory and the zero-limit edge (no concurrent capacity).
    let (allowed, _) = backend
        .quota_admit("run-over-memory", "fresh-user", 1, 4096, &limits, now)
        .await
        .expect("admission call");
    assert!(!allowed, "an over-memory first admission must be rejected");

    let zero_concurrency = QuotaConfig {
        max_concurrent_runs: 0,
        max_total_threads: 16,
        max_total_memory_mb: 8192,
        max_runs_per_day: 50,
    };
    let (allowed, _) = backend
        .quota_admit(
            "run-zero-capacity",
            "fresh-user-2",
            1,
            64,
            &zero_concurrency,
            now,
        )
        .await
        .expect("admission call");
    assert!(
        !allowed,
        "max_concurrent_runs = 0 admits nothing, not even a first run"
    );

    // Nothing was recorded for the rejected attempts.
    let usage = backend
        .quota_usage("fresh-user", now)
        .await
        .expect("usage after rejections");
    assert_eq!(usage.active_runs, 0, "rejections record no usage");

    // A within-limit first admission for the same user still succeeds (the
    // gated INSERT path admits a row whose usage is within every limit).
    let (allowed, usage) = backend
        .quota_admit("run-ok", "fresh-user", 2, 64, &limits, now)
        .await
        .expect("admission call");
    assert!(allowed, "a within-limit first admission still succeeds");
    assert_eq!(usage.active_runs, 1);
    assert_eq!(usage.used_threads, 2, "usage counts the admitted run");
}

/// Request-level: two concurrent creates under `max_concurrent_runs = 1`
/// cannot both be admitted. The spawned stub blocks until the test releases
/// it, so the winner's reservation is still held while the loser is refused.
#[tokio::test]
async fn concurrent_create_requests_admit_exactly_one_under_the_limit() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let root = std::path::PathBuf::from(tmp).join(format!("quota-durable-{}", std::process::id()));
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
    backend
        .save_quota_config(&QuotaConfig {
            max_concurrent_runs: 1,
            ..QuotaConfig::default()
        })
        .await
        .expect("persist limits");

    let app = server::build_router("personal");
    let body = json!({"toml_content": WORKFLOW});

    let (first, second) = tokio::join!(post_run(&app, body.clone()), post_run(&app, body));
    let (first_status, first_body) = first;
    let (second_status, second_body) = second;

    let statuses = [first_status, second_status];
    let admitted = statuses.iter().filter(|s| **s == StatusCode::OK).count();
    let rejected = statuses
        .iter()
        .filter(|s| **s == StatusCode::TOO_MANY_REQUESTS)
        .count();
    assert_eq!(
        admitted, 1,
        "exactly one concurrent create may be admitted: {first_body} / {second_body}"
    );
    assert_eq!(
        rejected, 1,
        "the other create must be quota-rejected: {first_body} / {second_body}"
    );
    let (loser_body, winner_body) = if first_status == StatusCode::OK {
        (&second_body, &first_body)
    } else {
        (&first_body, &second_body)
    };
    assert_eq!(loser_body["code"], "QUOTA_EXCEEDED", "{loser_body}");
    assert!(
        loser_body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Concurrent run limit reached"),
        "the rejection names the breached limit: {loser_body}"
    );

    let winner_run = winner_body["run_id"].as_str().expect("run_id").to_string();
    // Only the admitted request left a run row behind.
    let (run_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM runs")
        .fetch_one(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .expect("count runs");
    assert_eq!(run_count, 1, "a rejected create never inserts a run row");

    // Let the stub finish; the terminal path releases the reservation.
    std::fs::write(&release_file, b"go").expect("release stub");
    wait_terminal(&winner_run).await;
    let usage = backend
        .quota_usage("default", now_ms())
        .await
        .expect("usage read");
    assert_eq!(
        usage.active_runs, 0,
        "the terminal path released the admission"
    );
    assert_eq!(usage.runs_today, 1, "the admitted run is counted once");
}

const WORKFLOW: &str = r#"
[workflow]
name = "quota-admission"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A stub `oxo-flow` that blocks until `release` exists, then exits 0 —
/// keeps the winner's run active while the concurrent loser is evaluated.
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
