//! Cluster SSH-key exposure (issue #517).
//!
//! `GET /api/clusters` must never return the stored `ssh_key` — any
//! authenticated user could previously read the server's key paths (or any
//! pasted key material). Responses carry `ssh_key_set` instead, and the
//! probe endpoint (which actively uses the credential) is admin-only
//! outside personal mode.

mod common;
use common::free_port;
use common::spawn_web_server;

use common::{api_get, api_post};
use serde_json::json;

#[tokio::test]
async fn cluster_responses_never_carry_ssh_key() {
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

    let (status, login) = api_post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "admin", "password": "secret-admin"}),
    )
    .await;
    assert_eq!(status, 200, "admin login: {login}");
    let admin = login["token"].as_str().unwrap().to_string();
    let (status, body) = api_post(
        &base,
        "/api/users",
        Some(&admin),
        json!({"username": "view", "password": "view-pw", "role": "viewer"}),
    )
    .await;
    assert_eq!(status, 200, "create viewer: {body}");
    let (status, login) = api_post(
        &base,
        "/api/auth/login",
        None,
        json!({"username": "view", "password": "view-pw"}),
    )
    .await;
    assert_eq!(status, 200, "viewer login: {login}");
    let viewer = login["token"].as_str().unwrap().to_string();

    // Admin stores a cluster with sensitive key material.
    let (status, upserted) = api_post(
        &base,
        "/api/clusters",
        Some(&admin),
        json!({
            "id": "lab",
            "name": "Lab",
            "ssh_host": "login.lab.example.edu",
            "ssh_user": "bioinf",
            "ssh_key": "SUPER-SECRET-KEY-MATERIAL",
            "scheduler": "slurm",
            "enabled": true
        }),
    )
    .await;
    assert_eq!(status, 200, "upsert: {upserted}");

    // The upsert response must not echo the key either.
    assert!(
        !upserted.to_string().contains("SUPER-SECRET-KEY-MATERIAL"),
        "upsert response leaks key: {upserted}"
    );

    // The list must not expose the key to anyone — admin or viewer.
    for token in [&admin, &viewer] {
        let (status, list) = api_get(&base, "/api/clusters", Some(token)).await;
        assert_eq!(status, 200, "list as {token}: {list}");
        let text = list.to_string();
        assert!(
            !text.contains("SUPER-SECRET-KEY-MATERIAL"),
            "list leaks key: {text}"
        );
        assert!(
            list[0]["ssh_key"].is_null(),
            "ssh_key field must be absent: {list}"
        );
        assert_eq!(list[0]["ssh_key_set"], true, "ssh_key_set flag: {list}");
    }

    // The probe endpoint actively uses the credential — admin-only.
    let (status, _) = api_post(&base, "/api/clusters/lab/probe", Some(&viewer), json!({})).await;
    assert_eq!(status, 403, "viewer must not probe: cluster credential use");
}
