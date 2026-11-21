//! WEB-15 D1 / issue #1311: write-ahead audit intent.
//!
//! The audit row for a state-changing request must be durable BEFORE the
//! handler mutates state, so a crash between the mutation and its outcome
//! write cannot leave a committed state change with no audit row. These tests
//! fail under the old "write the row AFTER the handler" order.
//!
//! Two independent probes:
//!
//! 1. `audit_row_is_durable_before_the_handler_runs` — a handler that reads the
//!    `audit_logs` table while it executes sees its own `pending` intent row
//!    already committed. Under the old order the table is empty for that action
//!    at handler time, so the assertion fails.
//! 2. `crashed_intent_leaves_a_durable_pending_row` — an intent that is never
//!    finalized (the crash simulation) stays on disk as `pending`, i.e. the
//!    trail is never shorter than the set of mutations.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

mod common;

async fn json_body(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// The handler observes the audit store mid-request and reports the `result`
/// column it finds for its own action. A `pending` value proves the write-ahead
/// intent was committed before the mutation (the handler) ran.
async fn probe_handler() -> axum::Json<Value> {
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let row: Option<(String,)> =
        sqlx::query_as("SELECT result FROM audit_logs WHERE action = ? LIMIT 1")
            .bind("POST /api/pipelines/tx-probe")
            .fetch_optional(pool)
            .await
            .expect("read the audit store mid-request");
    axum::Json(serde_json::json!({
        "observed_during_handler": row.map(|r| r.0),
    }))
}

#[tokio::test]
async fn audit_row_is_durable_before_the_handler_runs() {
    common::ensure_db().await;

    let app = axum::Router::new()
        .route(
            "/api/pipelines/tx-probe",
            axum::routing::post(probe_handler),
        )
        .layer(axum::middleware::from_fn(
            oxo_flow_web::audit::audit_middleware,
        ));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/pipelines/tx-probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp.into_body()).await;
    assert_eq!(
        body["observed_during_handler"], "pending",
        "the audit intent row must be committed BEFORE the handler runs: {body}"
    );

    // After the handler returned, the middleware finalized the same row.
    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let finalized: (String,) =
        sqlx::query_as("SELECT result FROM audit_logs WHERE action = ? LIMIT 1")
            .bind("POST /api/pipelines/tx-probe")
            .fetch_one(pool)
            .await
            .expect("the finalized audit row must be recorded");
    assert_eq!(
        finalized.0, "success",
        "the intent is finalized with the observed outcome"
    );
}

#[tokio::test]
async fn crashed_intent_leaves_a_durable_pending_row() {
    common::ensure_db().await;

    use oxo_flow_web::infra::db::StorageBackend;
    use oxo_flow_web::infra::db::sqlite::SqliteBackend;

    let backend = SqliteBackend::new(common::db_url())
        .await
        .expect("backend connect");

    // Write the intent, then simulate a crash before finalize: nothing further
    // runs. The row must already be durable, and its synthetic-user seed must
    // have committed in the same transaction.
    backend
        .begin_audit_intent(
            "tx-crash-intent-id",
            "crash-actor",
            "DELETE /api/pipelines/crash-target",
            "/api/pipelines/crash-target",
            None,
        )
        .await
        .expect("intent write must succeed while storage is healthy");

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let row: (String, String) =
        sqlx::query_as("SELECT result, user_id FROM audit_logs WHERE id = 'tx-crash-intent-id'")
            .fetch_one(pool)
            .await
            .expect("the un-finalized intent row must still be durable (no audit-less mutation)");
    assert_eq!(row.0, "pending", "an un-finalized intent reads as pending");
    assert_eq!(row.1, "crash-actor");

    // The synthetic-user seed is part of the same transaction.
    let seeded: (String,) = sqlx::query_as("SELECT id FROM users WHERE id = 'crash-actor'")
        .fetch_one(pool)
        .await
        .expect("the synthetic user seed committed with the intent");
    assert_eq!(seeded.0, "crash-actor");

    // Finalizing a row that does not exist is an error (a broken trail).
    let missing = backend
        .finalize_audit_intent("no-such-intent-id", "success", None)
        .await;
    assert!(
        missing.is_err(),
        "finalize must error when the intent row is gone"
    );
}

#[tokio::test]
async fn healthy_router_records_a_finalized_row_per_mutation() {
    common::ensure_db().await;

    // The production router path ends with exactly one finalized row (no
    // leftover `pending`), so the write-ahead step did not change the recorded
    // trail for the healthy case.
    let app = axum::Router::new()
        .route(
            "/api/pipelines/tx-probe-ok",
            axum::routing::post(probe_handler),
        )
        .layer(axum::middleware::from_fn(
            oxo_flow_web::audit::audit_middleware,
        ));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/pipelines/tx-probe-ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = json_body(resp.into_body()).await;

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_logs WHERE target = ?")
        .bind("/api/pipelines/tx-probe-ok")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(rows.0, 1, "one finalized row per mutation");
    let pending: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_logs WHERE target = ? AND result = 'pending'")
            .bind("/api/pipelines/tx-probe-ok")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(pending.0, 0, "no leftover pending row on the healthy path");
}
