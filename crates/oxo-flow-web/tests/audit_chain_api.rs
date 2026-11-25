//! Web surface of the hash-chained audit trail (issue #1372): the admin-only
//! export/verify endpoints, their deterministic bytes, and the 200-with-finding
//! contract for tampering.
//!
//! One test function on purpose: every request in this binary shares the
//! process-global storage backend (`common::ensure_db`), and the tamper steps
//! mutate that database. Keeping the steps sequential makes the fixture
//! deterministic without a serial-test harness.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_core::audit_chain::{
    AUDIT_CHAIN_FORMAT, FINDING_HASH_MISMATCH, FINDING_LOG_MISMATCH, FINDING_MISSING_LOG_ROW,
};
use serde_json::Value;
use tower::ServiceExt;

mod common;

async fn get_json(uri: &str) -> (StatusCode, Vec<u8>) {
    let app = oxo_flow_web::server::build_router("personal");
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

fn finding_codes(verification: &Value) -> Vec<&str> {
    verification["findings"]
        .as_array()
        .map(|findings| {
            findings
                .iter()
                .filter_map(|finding| finding["code"].as_str())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn audit_chain_endpoints_export_verify_and_report_tampering() {
    common::ensure_db().await;
    let backend = oxo_flow_web::infra::db::global_backend().expect("storage backend");

    // Two chained intents plus one legacy terminal row.
    backend
        .begin_audit_intent(
            "api-chain-1",
            "api-actor",
            "POST /api/runs",
            "/api/runs",
            None,
        )
        .await
        .unwrap();
    backend
        .finalize_audit_intent("api-chain-1", "success", Some(r#"{"status":200}"#))
        .await
        .unwrap();
    backend
        .begin_audit_intent("api-chain-2", "api-actor", "DELETE /api/x", "/api/x", None)
        .await
        .unwrap();
    backend
        .finalize_audit_intent("api-chain-2", "failure", None)
        .await
        .unwrap();
    backend
        .record_audit_log(
            "api-actor",
            "POST /api/legacy",
            "/api/legacy",
            "success",
            Some(r#"{"b":2,"a":1}"#),
        )
        .await
        .unwrap();

    // Export: 200, canonical document, deterministic bytes.
    let (status, first_bytes) = get_json("/api/audit/chain/export").await;
    assert_eq!(status, StatusCode::OK);
    let export: Value = serde_json::from_slice(&first_bytes).expect("export is JSON");
    assert_eq!(export["version"], AUDIT_CHAIN_FORMAT);
    let events = export["events"].as_array().expect("events array");
    assert!(events.len() >= 5, "2 pairs + 1 legacy terminal: {events:?}");
    assert_eq!(
        export["head"]["seq"].as_u64().unwrap(),
        events.len() as u64,
        "head tracks the last event"
    );
    assert_eq!(
        export["head"]["hash"].as_str().unwrap(),
        events.last().unwrap()["hash"].as_str().unwrap()
    );
    let (_, second_bytes) = get_json("/api/audit/chain/export").await;
    assert_eq!(
        first_bytes, second_bytes,
        "an unchanged database must export byte-identical documents"
    );

    // Verify: 200, ok true, no findings.
    let (status, body) = get_json("/api/audit/chain/verify").await;
    assert_eq!(status, StatusCode::OK);
    let verification: Value = serde_json::from_slice(&body).expect("verification is JSON");
    assert_eq!(verification["ok"], true, "{verification}");
    assert_eq!(
        verification["events"].as_u64().unwrap(),
        events.len() as u64
    );
    assert!(finding_codes(&verification).is_empty());

    // Tamper 1: delete a mutable audit row — 200, ok false, a finding.
    sqlx::query("DELETE FROM audit_logs WHERE id = 'api-chain-1'")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();
    let (status, body) = get_json("/api/audit/chain/verify").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "tampering is evidence, not an HTTP error"
    );
    let verification: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(verification["ok"], false);
    assert!(
        finding_codes(&verification).contains(&FINDING_MISSING_LOG_ROW),
        "{verification}"
    );

    // Tamper 2: mutate another audit row.
    sqlx::query("UPDATE audit_logs SET target = '/api/edited' WHERE id = 'api-chain-2'")
        .execute(oxo_flow_web::infra::db::sqlite::pool())
        .await
        .unwrap();
    let (_, body) = get_json("/api/audit/chain/verify").await;
    let verification: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(verification["ok"], false);
    assert!(
        finding_codes(&verification).contains(&FINDING_LOG_MISMATCH),
        "{verification}"
    );

    // Tamper 3: mutate a chain event itself.
    sqlx::query(
        "UPDATE audit_chain SET target = '/api/edited' \
         WHERE audit_id = 'api-chain-2' AND phase = 'failure'",
    )
    .execute(oxo_flow_web::infra::db::sqlite::pool())
    .await
    .unwrap();
    let (_, body) = get_json("/api/audit/chain/verify").await;
    let verification: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(verification["ok"], false);
    assert!(
        finding_codes(&verification).contains(&FINDING_HASH_MISMATCH),
        "{verification}"
    );
}
