//! GET /api/ai/config/server honors web-api.md's contract: admin-only
//! outside personal mode (H-5). The endpoint URL identifies the internal
//! AI proxy/gateway — exactly the reconnaissance input a hardened team
//! deployment must not expose to non-admin callers.
//!
//! Runs in its own test binary so `set_running_mode("team")` (a
//! first-call-wins process global) is deterministic here.

use axum::extract::Extension;
use oxo_flow_web::domains::ai::handlers::get_server_ai_config;
use oxo_flow_web::domains::auth::current_user::{CurrentUser, resolve};
use oxo_flow_web::server;

fn user_with_role(role: &str) -> CurrentUser {
    CurrentUser {
        id: format!("gate-probe-{role}"),
        role: role.to_string(),
    }
}

#[tokio::test]
async fn server_ai_config_read_is_admin_only_outside_personal_mode() {
    server::set_running_mode("team");

    // No identity at all: the default user is not an admin.
    let (status, body) = get_server_ai_config(None).await.unwrap_err();
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(body.0.code, "ACCESS_DENIED");

    // Every authenticated non-admin role is refused the same way.
    for role in ["viewer", "user"] {
        let user = resolve(Some(&Extension(user_with_role(role))));
        let (status, body) = get_server_ai_config(Some(Extension(user)))
            .await
            .unwrap_err();
        assert_eq!(status, axum::http::StatusCode::FORBIDDEN, "role {role}");
        assert_eq!(body.0.code, "ACCESS_DENIED", "role {role}");
    }

    // An admin passes the gate — the handler proceeds to the storage
    // backend, which is uninitialized in this test process (503 DB_ERROR,
    // anything but the gate's 403).
    let admin = resolve(Some(&Extension(user_with_role("admin"))));
    let (status, _body) = get_server_ai_config(Some(Extension(admin)))
        .await
        .unwrap_err();
    assert_ne!(status, axum::http::StatusCode::FORBIDDEN);
}
