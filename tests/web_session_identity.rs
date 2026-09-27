//! Session identity integrity (issue #516).
//!
//! The shared env passwords (OXO_FLOW_USER_PASSWORD / _VIEWER_PASSWORD)
//! must never mint or adopt a privileged identity, and role resolution must
//! key on the canonical users.id resolved at login — never on a
//! client-chosen username.

mod common;
use common::free_port;
use common::spawn_web_server;

use reqwest::Client;
use serde_json::{Value, json};

async fn post(
    base: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (reqwest::StatusCode, Value) {
    let client = Client::new();
    let mut req = client.post(format!("{base}{path}")).json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let value = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, value)
}

async fn get(base: &str, path: &str, token: Option<&str>) -> (reqwest::StatusCode, Value) {
    let client = Client::new();
    let mut req = client.get(format!("{base}{path}"));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let value = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn env_passwords_cannot_mint_or_adopt_privileged_identities() {
    let dir = tempfile::tempdir().unwrap();
    let server = spawn_web_server(
        dir.path(),
        free_port(),
        &[
            ("OXO_FLOW_MODE", "team"),
            ("OXO_FLOW_ADMIN_PASSWORD", "secret-admin-pw"),
            ("OXO_FLOW_USER_PASSWORD", "shared-user-pw"),
        ],
    );
    let base = server.base.clone();

    // The shared USER password must not sign in as "admin" (Path A of #516).
    let (status, body) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "admin", "password": "shared-user-pw"}),
    )
    .await;
    assert_eq!(status, 401, "user password must not mint admin: {body}");

    // The admin password still signs in as admin, and the session's role
    // resolves to admin via its canonical row.
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "admin", "password": "secret-admin-pw"}),
    )
    .await;
    assert_eq!(status, 200, "admin login: {login}");
    assert_eq!(login["role"], "admin");
    let admin = login["token"].as_str().unwrap().to_string();
    let (status, me) = get(&base, "/api/auth/me", Some(&admin)).await;
    assert_eq!(status, 200);
    assert_eq!(me["role"], "admin", "admin /me: {me}");
    assert_eq!(me["username"], "admin", "admin /me username: {me}");
    let (status, _) = get(&base, "/api/users", Some(&admin)).await;
    assert_eq!(
        status, 200,
        "admin surface reachable with new-style session"
    );

    // An env-password identity logs in under its own name with role user.
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "worker1", "password": "shared-user-pw"}),
    )
    .await;
    assert_eq!(status, 200, "env user login: {login}");
    assert_eq!(login["role"], "user");
    let worker = login["token"].as_str().unwrap().to_string();
    let (status, me) = get(&base, "/api/auth/me", Some(&worker)).await;
    assert_eq!(status, 200);
    assert_eq!(me["username"], "worker1", "env identity /me: {me}");
    assert_eq!(me["role"], "user", "env identity /me role: {me}");
    let (status, _) = get(&base, "/api/users", Some(&worker)).await;
    assert_eq!(status, 403, "env identity must stay non-admin");

    // A managed (UUID-keyed) admin account must not be adoptable through
    // the shared user password either — the name is not enough.
    let (status, body) = post(
        &base,
        "/api/users",
        Some(&admin),
        json!({"username": "boss", "password": "boss-own-pw", "role": "admin"}),
    )
    .await;
    assert_eq!(status, 200, "create managed admin: {body}");
    let (status, body) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "boss", "password": "shared-user-pw"}),
    )
    .await;
    assert_eq!(
        status, 401,
        "shared user password must not adopt managed account: {body}"
    );
    // The account's own password still works and yields the admin role.
    let (status, login) = post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "boss", "password": "boss-own-pw"}),
    )
    .await;
    assert_eq!(status, 200, "managed admin own login: {login}");
    assert_eq!(login["role"], "admin");
    let boss = login["token"].as_str().unwrap().to_string();
    let (status, me) = get(&base, "/api/auth/me", Some(&boss)).await;
    assert_eq!(status, 200);
    assert_eq!(me["username"], "boss");
    assert_eq!(me["role"], "admin");
}
