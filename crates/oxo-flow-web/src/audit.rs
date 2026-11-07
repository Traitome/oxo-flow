//! Audit logging middleware for oxo-flow-web.
//!
//! Records state-changing HTTP requests into the `audit_logs` database table
//! and structured observability event streams.

/// Middleware: record every state-changing request (non-GET/HEAD/OPTIONS) in
/// the `audit_logs` table — the single audit write point covering all
/// mutation handlers (issue #79 P1-05: the table had schema but zero write
/// call sites).
///
/// Must be layered INSIDE `require_auth` (server.rs) so the authenticated
/// user id inserted into request extensions is visible; personal-mode
/// requests fall back to the 'default' pseudo-user. The insert is
/// fire-and-forget — auditing must never block or fail the request.
pub async fn audit_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::Method;

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    // The auth middleware (S2, issue #82 P0-4) injects a CurrentUser;
    // personal mode falls back to the 'default' pseudo-user.
    let user = audit_actor_id(
        request
            .extensions()
            .get::<crate::domains::auth::current_user::CurrentUser>(),
    );

    let response = next.run(request).await;

    if matches!(method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return response;
    }

    let status = response.status().as_u16();
    let action = format!("{method} {path}");
    let result = if (200..300).contains(&status) {
        "success"
    } else {
        "failure"
    };
    let metadata = serde_json::json!({ "status": status }).to_string();

    // Awaited, not fire-and-forget: an audit trail that can silently lose
    // rows is not an audit trail. One indexed INSERT per mutation is cheap
    // and the response is already computed.
    // PostgreSQL deployments never initialize the legacy audit store. A
    // missing audit backend must degrade explicitly, not panic the request
    // path — the structured log event below remains the machine trail.
    if let Some(backend) = crate::infra::db::global_backend() {
        if let Err(e) = backend
            .record_audit_log(&user, &action, &path, result, Some(&metadata))
            .await
        {
            tracing::error!("audit insert failed for {action}: {e}");
        }
    } else {
        tracing::warn!("audit store unavailable: {action} not audited");
    }
    // Structured event stream for machine consumers (issue #81).
    crate::domains::observability::logging::log_event(
        "audit",
        serde_json::json!({
            "user": user, "action": action, "resource": path, "result": result,
        }),
    );

    response
}

/// Resolve the actor id stored in `audit_logs.user_id`.
///
/// Canonical form is `users.id` — the same identity every ownership column
/// and session carries — never a display name resolved at request time.
/// `users.id` is a UUID for DB-created/bootstrap accounts and the username
/// itself for env-password-provisioned identities (`user`, `viewer`) and the
/// `default` personal-mode pseudo-user, so stored rows legitimately show
/// both shapes; existing rows are never rewritten (report finding T-F4).
fn audit_actor_id(user: Option<&crate::domains::auth::current_user::CurrentUser>) -> String {
    user.map(|u| u.id.clone())
        .unwrap_or_else(|| crate::domains::auth::current_user::CurrentUser::default_user().id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::auth::current_user::CurrentUser;

    /// Report finding T-F4: every new audit row is attributed to the
    /// canonical `users.id` — the same value sessions and ownership columns
    /// use — never a raw login name. The id's shape (UUID vs env-provisioned
    /// username) belongs to the users table, not to this writer.
    #[test]
    fn audit_actor_is_the_canonical_user_id() {
        let uuid_user = CurrentUser {
            id: "5b7d0c81-0000-4000-8000-000000000000".into(),
            role: "admin".into(),
        };
        assert_eq!(
            audit_actor_id(Some(&uuid_user)),
            "5b7d0c81-0000-4000-8000-000000000000"
        );
        let env_user = CurrentUser {
            id: "user".into(),
            role: "user".into(),
        };
        assert_eq!(audit_actor_id(Some(&env_user)), "user");
        // Pre-auth/public requests have no extension: the same 'default'
        // pseudo-user `resolve()` yields everywhere else.
        assert_eq!(audit_actor_id(None), "default");
    }
}
