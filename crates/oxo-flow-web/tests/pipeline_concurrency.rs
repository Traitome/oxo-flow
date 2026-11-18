//! Issue #1136 (WEB-11): optimistic concurrency and atomic revision
//! snapshots for pipeline writes.
//!
//! The concurrency token is the HTTP `ETag` (the monotonic stored revision);
//! clients echo it back in `If-Match`. Two layers are exercised:
//!
//! - the real router over a shared SQLite database — two interleaved writers
//!   prove that exactly one wins, the loser gets `409
//!   PIPELINE_VERSION_CONFLICT`, and the winner's content is what persists;
//! - a dedicated in-memory SQLite backend — the storage-level compare-and-
//!   swap, plus failure injection for the revision snapshot. The injector is
//!   the least invasive one available: a fresh in-memory database has its
//!   `pipeline_revisions` table dropped, so the snapshot insert fails with a
//!   real database error while the `pipelines` table stays intact. No
//!   test-only hooks exist in production code.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use oxo_flow_web::infra::db::models;
use oxo_flow_web::infra::db::sqlite::SqliteBackend;
use oxo_flow_web::infra::db::{PipelineWriteOutcome, StorageBackend};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

/// A definition the engine accepts, with exactly one rule.
const BASE_TOML: &str = r#"
[workflow]
name = "concurrency-test"
version = "1.0.0"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

fn app() -> axum::Router {
    server::build_router("personal")
}

/// The optional `If-Match` value a conditional write sends.
type IfMatch<'a> = Option<&'a str>;

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    if_match: IfMatch<'_>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(if_match) = if_match {
        builder = builder.header(header::IF_MATCH, if_match);
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
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, value)
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, HeaderMap, Value) {
    request(app, "GET", uri, None, None).await
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, HeaderMap, Value) {
    request(app, "POST", uri, Some(body), None).await
}

async fn put(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, HeaderMap, Value) {
    request(app, "PUT", uri, Some(body), None).await
}

async fn put_if_match(
    app: &axum::Router,
    uri: &str,
    body: Value,
    if_match: &str,
) -> (StatusCode, HeaderMap, Value) {
    request(app, "PUT", uri, Some(body), Some(if_match)).await
}

/// The strong validator a successful pipeline response must carry.
fn etag(headers: &HeaderMap) -> &str {
    headers
        .get(header::ETAG)
        .expect("pipeline responses carry an ETag")
        .to_str()
        .expect("ETag is ASCII")
}

// ---------------------------------------------------------------------------
// HTTP level: the interleaved two-writer race
// ---------------------------------------------------------------------------

#[tokio::test]
async fn interleaved_writers_one_conflicts_and_the_winner_survives() {
    common::ensure_db().await;
    let app = app();

    let (status, headers, created) = post(
        &app,
        "/api/pipelines",
        json!({"toml_content": BASE_TOML, "name": "race"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(
        etag(&headers),
        "\"1\"",
        "a fresh pipeline starts at revision 1"
    );
    let id = created["id"].as_str().expect("pipeline id").to_string();

    // Both writers read revision 1, then race. One CAS lands; the other must
    // be told its token is stale instead of silently overwriting.
    let toml_a = BASE_TOML.replace("concurrency-test", "writer-a");
    let toml_b = BASE_TOML.replace("concurrency-test", "writer-b");
    let uri = format!("/api/pipelines/{id}");
    let (result_a, result_b) = tokio::join!(
        put_if_match(&app, &uri, json!({"toml_content": toml_a}), "\"1\""),
        put_if_match(&app, &uri, json!({"toml_content": toml_b}), "\"1\"")
    );

    let statuses = [result_a.0, result_b.0];
    assert_eq!(
        statuses.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "exactly one writer must win: {result_a:?} / {result_b:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::CONFLICT)
            .count(),
        1,
        "exactly one writer must lose with 409: {result_a:?} / {result_b:?}"
    );

    let (winner_toml, winner_headers, loser) = if result_a.0 == StatusCode::OK {
        (toml_a.clone(), &result_a.1, &result_b.2)
    } else {
        (toml_b.clone(), &result_b.1, &result_a.2)
    };
    assert_eq!(
        etag(winner_headers),
        "\"2\"",
        "the winner's response carries the new token"
    );
    assert_eq!(loser["code"], "PIPELINE_VERSION_CONFLICT", "{loser}");
    assert_eq!(
        loser["detail"], "expected revision 1 but the stored revision is 2",
        "the conflict must name both revisions: {loser}"
    );
    assert!(
        loser["suggestion"]
            .as_str()
            .unwrap_or_default()
            .contains("If-Match"),
        "the conflict must tell the client how to retry: {loser}"
    );

    // The winner's content is what a re-read returns, at the advanced token.
    let (status, current_headers, current) = get(&app, &uri).await;
    assert_eq!(status, StatusCode::OK, "{current}");
    assert_eq!(etag(&current_headers), "\"2\"", "{current}");
    assert_eq!(
        current["toml_content"], winner_toml,
        "the winner's content must survive the race: {current}"
    );

    // The losing write left no trace: exactly two snapshots exist (the
    // create snapshot plus the winner's pre-update snapshot), and none of
    // them contains either writer's content.
    let (status, _, revisions) = get(&app, &format!("/api/pipelines/{id}/revisions")).await;
    assert_eq!(status, StatusCode::OK, "{revisions}");
    let revisions = revisions.as_array().expect("revision list");
    assert_eq!(
        revisions.len(),
        2,
        "create + one successful update record exactly two snapshots: {revisions:?}"
    );
    for revision in revisions {
        let rev_id = revision["id"].as_str().expect("revision id");
        let (status, _, detail) =
            get(&app, &format!("/api/pipelines/{id}/revisions/{rev_id}")).await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        assert_eq!(
            detail["toml_content"], BASE_TOML,
            "the losing write must not appear in any snapshot: {detail}"
        );
    }
}

#[tokio::test]
async fn update_without_if_match_stays_backward_compatible() {
    common::ensure_db().await;
    let app = app();

    let (status, _, created) = post(
        &app,
        "/api/pipelines",
        json!({"toml_content": BASE_TOML, "name": "legacy-client"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    // Legacy shape: no If-Match. The write proceeds, but the token still
    // advances so a later conditional writer can detect this change.
    let updated_toml = BASE_TOML.replace("concurrency-test", "legacy-updated");
    let (status, headers, body) = put(
        &app,
        &format!("/api/pipelines/{id}"),
        json!({"toml_content": updated_toml}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(etag(&headers), "\"2\"", "{body}");

    // A conditional writer holding the OLD token now conflicts.
    let (status, _, body) = put_if_match(
        &app,
        &format!("/api/pipelines/{id}"),
        json!({"toml_content": BASE_TOML}),
        "\"1\"",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "PIPELINE_VERSION_CONFLICT", "{body}");
}

#[tokio::test]
async fn malformed_if_match_is_rejected_not_ignored() {
    common::ensure_db().await;
    let app = app();

    let (status, _, created) = post(
        &app,
        "/api/pipelines",
        json!({"toml_content": BASE_TOML, "name": "bad-token"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    for bad in ["abc", "0", "-1", "1.5", "W/\"1\"", "*", "\"1\", \"2\""] {
        let (status, _, body) = put_if_match(
            &app,
            &format!("/api/pipelines/{id}"),
            json!({"toml_content": BASE_TOML}),
            bad,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert_eq!(body["code"], "INVALID_IF_MATCH", "{bad}: {body}");
    }
}

#[tokio::test]
async fn quoted_and_unquoted_if_match_both_work() {
    common::ensure_db().await;
    let app = app();

    let (status, _, created) = post(
        &app,
        "/api/pipelines",
        json!({"toml_content": BASE_TOML, "name": "unquoted-token"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let uri = format!("/api/pipelines/{id}");

    let (status, _, body) = put_if_match(&app, &uri, json!({}), "1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = put_if_match(&app, &uri, json!({}), "\"2\"").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ---------------------------------------------------------------------------
// Storage level: CAS semantics + snapshot failure injection
// ---------------------------------------------------------------------------

async fn memory_backend() -> SqliteBackend {
    let backend = SqliteBackend::new("sqlite::memory:").await.unwrap();
    backend.init().await.unwrap();
    backend
}

async fn seed_pipeline(backend: &SqliteBackend, name: &str) -> models::PipelineRow {
    let now = chrono::Utc::now().to_rfc3339();
    let row = models::PipelineRow {
        id: uuid::Uuid::new_v4().to_string(),
        // The init seed creates the 'default' pseudo-user.
        user_id: "default".to_string(),
        name: name.to_string(),
        version: "1.0.0".to_string(),
        toml_content: format!("[workflow]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        rules_count: 0,
        forked_from: None,
        visibility: "private".to_string(),
        created_at: now.clone(),
        updated_at: now,
        revision: 1,
    };
    backend
        .create_pipeline_with_revision(&row)
        .await
        .expect("create pipeline with initial revision");
    row
}

#[tokio::test]
async fn cas_applies_once_and_records_the_pre_write_snapshot() {
    let backend = memory_backend().await;
    let row = seed_pipeline(&backend, "cas-applied").await;

    let revisions = backend.list_pipeline_revisions(&row.id, 10).await.unwrap();
    assert_eq!(revisions.len(), 1, "create records the initial snapshot");

    let mut updated = row.clone();
    updated.name = "renamed".to_string();
    updated.toml_content = "[workflow]\nname = \"renamed\"\nversion = \"1.0.0\"\n".to_string();
    updated.updated_at = chrono::Utc::now().to_rfc3339();
    let snapshot = models::RevisionSnapshot {
        user_id: "default",
        version: &row.version,
        toml_content: &row.toml_content,
    };
    let outcome = backend
        .update_pipeline_cas(&updated, Some(1), Some(&snapshot))
        .await
        .unwrap();
    assert_eq!(outcome, PipelineWriteOutcome::Applied { revision: 2 });

    let stored = backend.get_pipeline(&row.id).await.unwrap().unwrap();
    assert_eq!(stored.name, "renamed");
    assert_eq!(stored.revision, 2);
    assert_eq!(stored.toml_content, updated.toml_content);

    let revisions = backend.list_pipeline_revisions(&row.id, 10).await.unwrap();
    assert_eq!(revisions.len(), 2, "the pre-write state was snapshotted");
    assert!(
        revisions
            .iter()
            .any(|revision| revision.toml_content == row.toml_content),
        "the new snapshot holds the pre-update content: {revisions:?}"
    );
}

#[tokio::test]
async fn stale_token_is_a_conflict_and_writes_nothing() {
    let backend = memory_backend().await;
    let row = seed_pipeline(&backend, "cas-conflict").await;

    let mut updated = row.clone();
    updated.name = "should-not-land".to_string();
    updated.updated_at = chrono::Utc::now().to_rfc3339();
    let snapshot = models::RevisionSnapshot {
        user_id: "default",
        version: &row.version,
        toml_content: &row.toml_content,
    };
    let outcome = backend
        .update_pipeline_cas(&updated, Some(99), Some(&snapshot))
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PipelineWriteOutcome::Conflict {
            current_revision: 1
        }
    );

    let stored = backend.get_pipeline(&row.id).await.unwrap().unwrap();
    assert_eq!(stored.name, row.name, "the conflicting write wrote nothing");
    assert_eq!(stored.revision, 1);
    let revisions = backend.list_pipeline_revisions(&row.id, 10).await.unwrap();
    assert_eq!(
        revisions.len(),
        1,
        "a rejected compare-and-swap must not record a snapshot either"
    );

    // A missing pipeline is reported as NotFound, not as a conflict.
    let mut missing = row.clone();
    missing.id = "does-not-exist".to_string();
    let outcome = backend
        .update_pipeline_cas(&missing, Some(1), None)
        .await
        .unwrap();
    assert_eq!(outcome, PipelineWriteOutcome::NotFound);
}

#[tokio::test]
async fn failed_revision_insert_rolls_back_the_pipeline_update() {
    let backend = memory_backend().await;
    let row = seed_pipeline(&backend, "snapshot-failure").await;

    // Failure injection: the snapshot table disappears while the pipeline
    // row stays. This is a real storage error, not a test-only hook.
    sqlx::query("DROP TABLE pipeline_revisions")
        .execute(backend.inner_pool())
        .await
        .expect("drop revision table");

    let mut updated = row.clone();
    updated.name = "changed".to_string();
    updated.toml_content = "[workflow]\nname = \"changed\"\nversion = \"1.0.0\"\n".to_string();
    updated.updated_at = chrono::Utc::now().to_rfc3339();
    let snapshot = models::RevisionSnapshot {
        user_id: "default",
        version: &row.version,
        toml_content: &row.toml_content,
    };
    let error = backend
        .update_pipeline_cas(&updated, Some(1), Some(&snapshot))
        .await;
    assert!(
        error.is_err(),
        "a write whose revision insert fails must fail, got {error:?}"
    );

    let stored = backend.get_pipeline(&row.id).await.unwrap().unwrap();
    assert_eq!(stored.name, row.name, "the pipeline write must roll back");
    assert_eq!(stored.toml_content, row.toml_content);
    assert_eq!(stored.revision, 1);
}

#[tokio::test]
async fn failed_initial_revision_insert_rolls_back_the_create() {
    let backend = memory_backend().await;
    sqlx::query("DROP TABLE pipeline_revisions")
        .execute(backend.inner_pool())
        .await
        .expect("drop revision table");

    let now = chrono::Utc::now().to_rfc3339();
    let row = models::PipelineRow {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: "default".to_string(),
        name: "create-failure".to_string(),
        version: "1.0.0".to_string(),
        toml_content: "[workflow]\nname = \"create-failure\"\nversion = \"1.0.0\"\n".to_string(),
        rules_count: 0,
        forked_from: None,
        visibility: "private".to_string(),
        created_at: now.clone(),
        updated_at: now,
        revision: 1,
    };
    assert!(
        backend.create_pipeline_with_revision(&row).await.is_err(),
        "a create whose initial snapshot fails must not report success"
    );
    assert!(
        backend.get_pipeline(&row.id).await.unwrap().is_none(),
        "the pipeline row must not survive the failed snapshot insert"
    );
}
