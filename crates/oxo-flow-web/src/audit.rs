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
    let user = request
        .extensions()
        .get::<crate::domains::auth::current_user::CurrentUser>()
        .map(|u| u.id.clone())
        .unwrap_or_else(|| "default".to_string());

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
    if crate::db::try_pool().is_none() {
        tracing::warn!("audit store unavailable (PostgreSQL backend?): {action} not audited");
    } else if let Err(e) =
        crate::db::insert_audit_row(&user, &action, &path, result, &metadata).await
    {
        tracing::error!("audit insert failed for {action}: {e}");
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
