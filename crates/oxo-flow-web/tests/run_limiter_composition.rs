//! Router-level composition of the run-creation budget (WEB-18 / #1142).
//!
//! The #213 limiter used to be exercised only through unit tests plus a
//! process-global debug override that *disabled* it. This binary drives the
//! real production router with an ENABLED limiter instance instead: a
//! test-scoped store with a fixed 2-runs/minute budget and an advanceable
//! clock, installed exactly where the production router installs its own
//! instance (`server::build_router_with_run_limiter` → request extension).
//!
//! Pinned here: the documented 429 contract (`RUN_RATE_LIMITED`, countdown
//! in `message`, no `Retry-After` header — see
//! `docs/guide/src/reference/web-api.md`), per-identity keying through the
//! real auth middleware, and window refill once the clock advances.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use oxo_flow_web::domains::execution::run_limit::{Clock, RunRateLimiter};
use oxo_flow_web::server;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tower::ServiceExt;

mod common;

/// Advance-able monotonic clock: `base` is real, the offset is ours.
struct TestClock {
    base: Instant,
    offset_ms: AtomicU64,
}

impl TestClock {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            offset_ms: AtomicU64::new(0),
        }
    }

    fn advance(&self, delta: Duration) {
        self.offset_ms
            .fetch_add(delta.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
    }
}

/// Seed a canonical user row + a live session for the team-mode auth
/// middleware (the same fixture shape as `ai_config_admin_team.rs`).
async fn seed_user(token: &str, id: &str, username: &str) {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR REPLACE INTO users (id, username, role, auth_type, created_at) \
         VALUES (?, ?, 'user', 'password', ?)",
    )
    .bind(id)
    .bind(username)
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

/// POST /api/runs through the real router with a real Bearer identity.
async fn post_run(app: &axum::Router, token: &str, body: &Value) -> (StatusCode, HeaderMap, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/runs")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("non-JSON response ({e}): {bytes:?}"));
    (status, headers, value)
}

const ALICE_TOKEN: &str = "run-limit-alice-token";
const BOB_TOKEN: &str = "run-limit-bob-token";
const BUDGET: u32 = 2;

#[tokio::test]
async fn enabled_run_limiter_is_composed_through_the_router() {
    common::ensure_db().await;

    // This binary owns its process environment: the kill switch off, and an
    // env budget (100/min) that would NOT limit at 3 — the injected
    // test-scoped instance must win.
    // SAFETY: one test in a dedicated test binary; nothing else reads these
    // variables before this point.
    unsafe {
        std::env::remove_var("OXO_FLOW_DISABLE_RATE_LIMIT");
        std::env::set_var("OXO_FLOW_RUNS_RATE_LIMIT", "100");
    }

    seed_user(ALICE_TOKEN, "run-limit-alice-id", "run-limit-alice").await;
    seed_user(BOB_TOKEN, "run-limit-bob-id", "run-limit-bob").await;

    let clock = Arc::new(TestClock::new());
    let limiter = Arc::new(RunRateLimiter::for_tests(BUDGET, clock.clone()));
    // Same assembly as `build_router("team")`; only the limiter instance is
    // test-scoped. Auth is the real middleware over the real sessions table.
    let app = server::build_router_with_run_limiter("team", limiter);

    // The limiter runs before body validation, so these requests consume
    // budget without spawning a CLI; `RUN_ERROR` proves the attempt passed
    // the limiter and reached the handler's own validation.
    let body = json!({"toml_content": "this is not a workflow"});

    // ── N allowed requests ──
    for attempt in 0..BUDGET {
        let (status, _, resp) = post_run(&app, ALICE_TOKEN, &body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "allowed attempt {attempt} must reach validation: {resp}"
        );
        assert_eq!(resp["code"], "RUN_ERROR", "attempt {attempt}: {resp}");
    }

    // ── N+1 gets the documented 429 ──
    let (status, headers, resp) = post_run(&app, ALICE_TOKEN, &body).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the third attempt must be rate limited: {resp}"
    );
    assert_eq!(resp["code"], "RUN_RATE_LIMITED", "{resp}");
    let message = resp["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("retry in"),
        "the countdown lives in `message`: {resp}"
    );
    assert!(
        resp["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("Allowance: 2 runs per minute"),
        "the detail reports the injected budget: {resp}"
    );
    // Documented contract: this 429 carries no Retry-After header (the
    // ApiResult error tuple cannot carry headers).
    assert!(
        headers.get(header::RETRY_AFTER).is_none(),
        "run-rate 429 must not carry Retry-After: {headers:?}"
    );

    // ── Identity keying: a different identity has its own budget ──
    let (status, _, resp) = post_run(&app, BOB_TOKEN, &body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "bob must not inherit alice's spent budget: {resp}"
    );
    assert_eq!(resp["code"], "RUN_ERROR", "{resp}");

    // ── Window refill: past 60 s the spent budget is evicted ──
    clock.advance(Duration::from_secs(61));
    for attempt in 0..BUDGET {
        let (status, _, resp) = post_run(&app, ALICE_TOKEN, &body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "post-refill attempt {attempt} must pass: {resp}"
        );
        assert_eq!(resp["code"], "RUN_ERROR", "attempt {attempt}: {resp}");
    }
    let (status, _, resp) = post_run(&app, ALICE_TOKEN, &body).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the refilled budget is spent again: {resp}"
    );
    assert_eq!(resp["code"], "RUN_RATE_LIMITED", "{resp}");
}
