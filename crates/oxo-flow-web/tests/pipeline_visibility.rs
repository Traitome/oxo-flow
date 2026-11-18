//! Pipeline visibility across tenants (WEB-13 / issue #1138).
//!
//! These tests drive the REAL team-mode router with real Bearer identities
//! (seeded users + sessions, the same fixture shape as
//! `run_limiter_composition.rs` / `ai_config_admin_team.rs`) and pin the
//! policy documented in `domains/workflow/visibility.rs`:
//!
//! - own pipeline visible (list + read);
//! - another tenant's private pipeline absent from list/search and 404 on
//!   every read path (no existence leak — 404, never 403);
//! - a `workspace`-shared pipeline visible to every signed-in tenant but
//!   writable only by its owner;
//! - a deleted pipeline invisible in both list and read (deletion is a hard
//!   delete, so the row is gone);
//! - UPDATE and rollback of a foreign pipeline refused, leaving it intact;
//! - the admin override and the share-token exemption are the documented
//!   exceptions.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;

/// A definition the engine accepts, with exactly one rule.
const VALID_TOML: &str = r#"
[workflow]
name = "visibility-test"

[[rules]]
name = "hello"
input = ["in.txt"]
output = ["out.txt"]
shell = "cat {input} > {output}"
"#;

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

async fn call(
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
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Save a pipeline through the production handler and return its id.
async fn save_pipeline(app: &axum::Router, token: &str, name: &str, visibility: &str) -> String {
    let (status, body) = call(
        app,
        "POST",
        "/api/pipelines",
        Some(token),
        Some(json!({"name": name, "toml_content": VALID_TOML, "visibility": visibility})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "save must succeed: {body}");
    body["id"].as_str().expect("pipeline id").to_string()
}

/// Ids in a `GET /api/pipelines` array response.
fn list_ids(body: &Value) -> Vec<String> {
    body.as_array()
        .expect("pipeline list is an array")
        .iter()
        .filter_map(|p| p["id"].as_str().map(String::from))
        .collect()
}

// ---------------------------------------------------------------------------
// Required case 1: own pipeline visible (list + read)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn own_pipeline_is_visible_in_list_and_read() {
    common::ensure_db().await;
    seed_identity(
        "vis-own-alice-token",
        "vis-own-alice-id",
        "vis-own-alice",
        "user",
    )
    .await;
    let app = server::build_router("team");

    let id = save_pipeline(&app, "vis-own-alice-token", "vis-own-private", "private").await;

    let (status, body) = call(
        &app,
        "GET",
        "/api/pipelines",
        Some("vis-own-alice-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        list_ids(&body).contains(&id),
        "the owner's own private pipeline must be listed: {body}"
    );

    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/pipelines/{id}"),
        Some("vis-own-alice-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner read must succeed: {body}");
    assert_eq!(body["id"], id.as_str());
}

// ---------------------------------------------------------------------------
// Required case 2: another tenant's private pipeline invisible
// ---------------------------------------------------------------------------

#[tokio::test]
async fn other_tenants_private_pipeline_is_absent_from_list_and_404_on_read() {
    common::ensure_db().await;
    seed_identity("vis-bob-bob-token", "vis-bob-bob-id", "vis-bob-bob", "user").await;
    seed_identity(
        "vis-bob-alice-token",
        "vis-bob-alice-id",
        "vis-bob-alice",
        "user",
    )
    .await;
    let app = server::build_router("team");

    let private_id = save_pipeline(&app, "vis-bob-bob-token", "vis-bob-private", "private").await;

    // List: bob's private pipeline must not appear for alice.
    let (status, body) = call(
        &app,
        "GET",
        "/api/pipelines",
        Some("vis-bob-alice-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !list_ids(&body).contains(&private_id),
        "another tenant's private pipeline must not be listed: {body}"
    );

    // Read: 404 (NOT 403 — existence must not leak).
    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/pipelines/{private_id}"),
        Some("vis-bob-alice-token"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign private read must 404: {body}"
    );
    assert_eq!(body["code"], "NOT_FOUND", "{body}");

    // Search and the TOML-loading analysis routes use the same predicate.
    let (status, body) = call(
        &app,
        "POST",
        "/api/pipelines/search",
        Some("vis-bob-alice-token"),
        Some(json!({"query": "vis-bob-private"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["results"]
            .as_array()
            .is_some_and(|results| results.iter().all(|r| r["id"] != private_id.as_str())),
        "foreign private pipeline must not surface in search: {body}"
    );

    let (status, body) = call(
        &app,
        "POST",
        "/api/pipelines/export",
        Some("vis-bob-alice-token"),
        Some(json!({"pipeline_id": private_id, "format": "docker"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign private export must 404: {body}"
    );

    // Version history is a read path too.
    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/pipelines/{private_id}/revisions"),
        Some("vis-bob-alice-token"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign private revisions must 404: {body}"
    );
}

// ---------------------------------------------------------------------------
// Required case 3: shared (workspace) pipeline visible
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workspace_shared_pipeline_is_visible_to_other_tenants_read_only() {
    common::ensure_db().await;
    seed_identity(
        "vis-share-bob-token",
        "vis-share-bob-id",
        "vis-share-bob",
        "user",
    )
    .await;
    seed_identity(
        "vis-share-alice-token",
        "vis-share-alice-id",
        "vis-share-alice",
        "user",
    )
    .await;
    let app = server::build_router("team");

    let shared_id = save_pipeline(
        &app,
        "vis-share-bob-token",
        "vis-share-workspace",
        "workspace",
    )
    .await;

    // Visible in list.
    let (status, body) = call(
        &app,
        "GET",
        "/api/pipelines",
        Some("vis-share-alice-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        list_ids(&body).contains(&shared_id),
        "a workspace-shared pipeline must be listed for every signed-in user: {body}"
    );

    // Readable with full TOML.
    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/pipelines/{shared_id}"),
        Some("vis-share-alice-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "shared read must succeed: {body}");
    assert_eq!(body["visibility"], "workspace", "{body}");

    // Visible in search.
    let (status, body) = call(
        &app,
        "POST",
        "/api/pipelines/search",
        Some("vis-share-alice-token"),
        Some(json!({"query": "vis-share-workspace"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["results"]
            .as_array()
            .is_some_and(|results| results.iter().any(|r| r["id"] == shared_id.as_str())),
        "a workspace-shared pipeline must surface in search: {body}"
    );

    // Sharing a read does NOT share a write: only the owner (or an admin)
    // may modify it, and the refusal is a 404 like every other foreign row.
    let (status, body) = call(
        &app,
        "PUT",
        &format!("/api/pipelines/{shared_id}"),
        Some("vis-share-alice-token"),
        Some(json!({"name": "hijacked"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a workspace reader must not modify the pipeline: {body}"
    );
}

// ---------------------------------------------------------------------------
// Required case 4: deleted pipeline invisible (list + read)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleted_pipeline_is_invisible_in_list_and_read() {
    common::ensure_db().await;
    seed_identity(
        "vis-del-alice-token",
        "vis-del-alice-id",
        "vis-del-alice",
        "user",
    )
    .await;
    let app = server::build_router("team");

    let id = save_pipeline(&app, "vis-del-alice-token", "vis-del-doomed", "private").await;

    let (status, body) = call(
        &app,
        "DELETE",
        &format!("/api/pipelines/{id}"),
        Some("vis-del-alice-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner delete must succeed: {body}");

    // Read: the hard-deleted row is gone — 404, not a resurrectable tombstone.
    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/pipelines/{id}"),
        Some("vis-del-alice-token"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a deleted pipeline must not be readable: {body}"
    );

    // List: absent.
    let (status, body) = call(
        &app,
        "GET",
        "/api/pipelines",
        Some("vis-del-alice-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !list_ids(&body).contains(&id),
        "a deleted pipeline must not be listed: {body}"
    );

    // Write paths agree: update and rollback are 404 on a deleted row.
    let (status, body) = call(
        &app,
        "PUT",
        &format!("/api/pipelines/{id}"),
        Some("vis-del-alice-token"),
        Some(json!({"name": "resurrected"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/pipelines/{id}/rollback"),
        Some("vis-del-alice-token"),
        Some(json!({"revision_id": "00000000-0000-0000-0000-000000000000"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

// ---------------------------------------------------------------------------
// Required case 5: UPDATE/rollback enforcement (previously only list/read
// were exercised by tests)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn foreign_pipeline_update_rollback_and_delete_are_refused_404() {
    common::ensure_db().await;
    seed_identity("vis-mut-bob-token", "vis-mut-bob-id", "vis-mut-bob", "user").await;
    seed_identity(
        "vis-mut-alice-token",
        "vis-mut-alice-id",
        "vis-mut-alice",
        "user",
    )
    .await;
    let app = server::build_router("team");

    let id = save_pipeline(&app, "vis-mut-bob-token", "vis-mut-bob-private", "private").await;

    // UPDATE: 404; nothing is written.
    let (status, body) = call(
        &app,
        "PUT",
        &format!("/api/pipelines/{id}"),
        Some("vis-mut-alice-token"),
        Some(json!({"name": "hijacked", "toml_content": VALID_TOML})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign update must 404, not 403: {body}"
    );
    assert_eq!(body["code"], "NOT_FOUND", "{body}");

    // ROLLBACK: 404 before the revision lookup (the body's revision id does
    // not matter — the pipeline itself is invisible).
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/pipelines/{id}/rollback"),
        Some("vis-mut-alice-token"),
        Some(json!({"revision_id": "00000000-0000-0000-0000-000000000000"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign rollback must 404: {body}"
    );

    // DELETE: 404 and the row survives.
    let (status, body) = call(
        &app,
        "DELETE",
        &format!("/api/pipelines/{id}"),
        Some("vis-mut-alice-token"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign delete must 404: {body}"
    );

    // FORK (read gate) and SHARE (write gate) refuse as well.
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/pipelines/{id}/fork"),
        Some("vis-mut-alice-token"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign fork must 404: {body}"
    );

    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/pipelines/{id}/share"),
        Some("vis-mut-alice-token"),
        Some(json!({"visibility": "link"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign share must 404: {body}"
    );

    // Run creation by pipeline id uses the same read predicate.
    let (status, body) = call(
        &app,
        "POST",
        "/api/runs",
        Some("vis-mut-alice-token"),
        Some(json!({"toml_content": VALID_TOML, "pipeline_id": id})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign pipeline run creation must 404: {body}"
    );
    assert_eq!(body["code"], "PIPELINE_NOT_FOUND", "{body}");

    // The owner still sees the untouched pipeline.
    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/pipelines/{id}"),
        Some("vis-mut-bob-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["name"], "vis-mut-bob-private",
        "row must be intact: {body}"
    );
    assert_eq!(
        body["toml_content"], VALID_TOML,
        "content must be intact: {body}"
    );
}

// ---------------------------------------------------------------------------
// Documented exemptions: admin override and share-token capability
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_override_sees_foreign_private_pipeline() {
    common::ensure_db().await;
    seed_identity("vis-adm-bob-token", "vis-adm-bob-id", "vis-adm-bob", "user").await;
    seed_identity(
        "vis-adm-root-token",
        "vis-adm-root-id",
        "vis-adm-root",
        "admin",
    )
    .await;
    let app = server::build_router("team");

    let id = save_pipeline(
        &app,
        "vis-adm-bob-token",
        "vis-adm-tenant-private",
        "private",
    )
    .await;

    let (status, body) = call(
        &app,
        "GET",
        "/api/pipelines",
        Some("vis-adm-root-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        list_ids(&body).contains(&id),
        "the documented admin override lists every tenant: {body}"
    );

    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/pipelines/{id}"),
        Some("vis-adm-root-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin read: {body}");
}

#[tokio::test]
async fn share_token_is_the_authorization_exemption() {
    common::ensure_db().await;
    seed_identity("vis-tok-bob-token", "vis-tok-bob-id", "vis-tok-bob", "user").await;
    seed_identity(
        "vis-tok-alice-token",
        "vis-tok-alice-id",
        "vis-tok-alice",
        "user",
    )
    .await;
    let app = server::build_router("team");

    let id = save_pipeline(&app, "vis-tok-bob-token", "vis-tok-private", "private").await;

    // A foreign tenant cannot read the pipeline directly...
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/pipelines/{id}"),
        Some("vis-tok-alice-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // ...but the owner may mint a share token, and the token (not the
    // predicate) authorizes the anonymous landing payload — the documented
    // exemption.
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/pipelines/{id}/share"),
        Some("vis-tok-bob-token"),
        Some(json!({"visibility": "link"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner share must succeed: {body}");
    let token = body["access_token"]
        .as_str()
        .expect("share token")
        .to_string();

    let (status, body) = call(&app, "GET", &format!("/api/share/{token}"), None, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the share token is the authorization for the landing payload: {body}"
    );
    assert_eq!(body["pipeline"]["name"], "vis-tok-private", "{body}");
}
