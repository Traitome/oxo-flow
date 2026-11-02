//! Session lifecycle: logout revocation + user-deletion cascade (#520).
//!
//! Sessions used to be irrevocable until expiry, and deleting a user left
//! their sessions and API keys authenticating for up to 24 hours.

mod common;
use common::free_port;
use common::{api_req, spawn_web_server};

use serde_json::json;

#[tokio::test]
async fn logout_revokes_and_user_deletion_cascades() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_web_server(
        dir.path(),
        free_port(),
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin"),
        ],
    );
    let base = server.base.clone();

    // Login → session valid.
    let (status, login) = api_req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "admin", "password": "secret-admin"})),
    )
    .await;
    assert_eq!(status, 200, "admin login: {login}");
    let admin = login["token"].as_str().unwrap().to_string();
    let (status, _) = api_req(&base, "GET", "/api/auth/me", Some(&admin), None).await;
    assert_eq!(status, 200, "fresh session must authenticate");

    // Logout revokes the presented session server-side.
    let (status, body) = api_req(
        &base,
        "POST",
        "/api/auth/logout",
        Some(&admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "logout: {body}");
    assert_eq!(body["logged_out"], true, "{body}");
    let (status, me) = api_req(&base, "GET", "/api/auth/me", Some(&admin), None).await;
    assert_eq!(
        status, 200,
        "/me answers 200 with authenticated=false: {me}"
    );
    assert_eq!(
        me["authenticated"], false,
        "revoked token must not authenticate: {me}"
    );

    // Re-login for the rest of the flow.
    let (_, login) = api_req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "admin", "password": "secret-admin"})),
    )
    .await;
    let admin = login["token"].as_str().unwrap().to_string();

    // User-deletion cascade: the fired user's session dies with the row.
    let (status, body) = api_req(
        &base,
        "POST",
        "/api/users",
        Some(&admin),
        Some(json!({"username": "shortlived", "password": "short-pw"})),
    )
    .await;
    assert_eq!(status, 200, "create user: {body}");
    let user_id = body["id"].as_str().unwrap().to_string();
    let (_, login) = api_req(
        &base,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": "shortlived", "password": "short-pw"})),
    )
    .await;
    let fired = login["token"].as_str().unwrap().to_string();
    let (status, _) = api_req(&base, "GET", "/api/auth/me", Some(&fired), None).await;
    assert_eq!(status, 200);

    let (status, _) = api_req(
        &base,
        "DELETE",
        &format!("/api/users/{user_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, 200, "delete user");
    let (_, me) = api_req(&base, "GET", "/api/auth/me", Some(&fired), None).await;
    assert_eq!(
        me["authenticated"], false,
        "deleted user's session must die: {me}"
    );
}
