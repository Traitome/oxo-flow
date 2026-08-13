//! Knowledge search endpoints — the editor palette's grounded tool source.
//!
//! Backed by the embedded in-memory Bioconda / bioSkills databases; no DB
//! pool or network required.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

async fn get_json(uri: &str) -> (StatusCode, Value) {
    let resp = server::build_router("personal")
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn tools_search_returns_grounded_entries() {
    let (status, body) = get_json("/api/knowledge/tools?q=fastp&limit=5").await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert!(
        body["total"].as_u64().unwrap() > 6000,
        "Bioconda DB has 6103 tools: {body}"
    );
    let tools = body["tools"].as_array().unwrap();
    assert!(!tools.is_empty(), "{body}");
    assert!(
        tools
            .iter()
            .any(|t| t["name"].as_str().unwrap().contains("fastp")),
        "{body:?}"
    );
    // Entries carry the fields the palette renders.
    let first = &tools[0];
    assert!(first["name"].is_string());
    assert!(first["version"].is_string());
    assert!(first["summary"].is_string());
}

#[tokio::test]
async fn skills_search_returns_domain_entries() {
    let (status, body) = get_json("/api/knowledge/skills?q=variant").await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert!(body["total"].as_u64().unwrap() > 500, "{body}");
    let skills = body["skills"].as_array().unwrap();
    assert!(!skills.is_empty(), "{body}");
    let first = &skills[0];
    assert!(first["name"].is_string());
    assert!(first["domain"].is_string());
}

#[tokio::test]
async fn tools_search_empty_query_returns_database_size() {
    let (status, body) = get_json("/api/knowledge/tools").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["tools"].as_array().is_some_and(|a| !a.is_empty()));
}
