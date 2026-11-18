//! Audit logging middleware for oxo-flow-web.
//!
//! Records state-changing HTTP requests into the `audit_logs` database table
//! and structured observability event streams.
//!
//! Every audit write is explicitly classified (WEB-15, issue #1140):
//!
//! - [`AuditDurability::Required`] — the row records a security-relevant
//!   action (authentication, user/role management, run lifecycle control,
//!   pipeline deletion/share grants, credential or key changes). When the
//!   write fails the request must fail closed with a structured
//!   `500 AUDIT_WRITE_FAILED`; a security action without its audit row is
//!   treated as an action the server may not acknowledge.
//! - [`AuditDurability::BestEffort`] — informational mutations (AI
//!   generation, parsing/validation, DAG edits, chat). A failed write is
//!   logged as a warning and the already-computed response is returned; the
//!   structured `audit` event stream remains the machine trail.
//!
//! Both classes share ONE writer — `write_audit`, which calls
//! [`StorageBackend::record_audit_log`](crate::infra::db::StorageBackend::record_audit_log).
//! Handlers must use [`audit_required`] or [`audit_best_effort`] rather than
//! calling the storage method directly, so the class is visible at the call
//! site. Two direct call sites remain outside this module, both in
//! `domains/auth/handlers.rs`: the successful-login row and the
//! license-upload metadata row. The middleware still writes a REQUIRED row
//! for `POST /api/license/upload`; the handler's own metadata row stays
//! best-effort. Migrating both rows to the shared helpers is unblocked
//! follow-up work tracked in the WEB-15 ledger entry.

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::domains::workflow::handlers::ApiError;

/// Durability class of an audit write (WEB-15, issue #1140).
///
/// The class is part of the call site, not a runtime policy: a security
/// event must never be silently downgraded because storage happened to be
/// unhealthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditDurability {
    /// The request fails closed (`500 AUDIT_WRITE_FAILED`) if the row cannot
    /// be persisted. Reserved for security-relevant actions.
    Required,
    /// The request proceeds and the failure is logged as a warning.
    BestEffort,
}

/// Classify a state-changing request for the audit middleware.
///
/// REQUIRED (security-relevant) — authentication, user/role management,
/// run lifecycle control, pipeline deletion and share grants, credential
/// and key changes, cluster connection secrets, webhook configuration, and
/// license upload. Everything else is BEST-EFFORT.
///
/// Dynamic segments are matched by shape, not by a second route table:
/// `/api/runs/{id}/cancel` is required because *any* run-control suffix is;
/// `/api/runs/{id}/report/ask` stays best-effort.
pub fn audit_durability_for(method: &str, path: &str) -> AuditDurability {
    // Tolerate a trailing slash (`/api/runs/` ≡ `/api/runs`).
    let path = path.trim_end_matches('/');

    let required = match method {
        "POST" => {
            matches!(
                path,
                "/api/auth/login"
                    | "/api/auth/logout"
                    | "/api/auth/oauth/authorize"
                    | "/api/auth/oauth/callback"
                    | "/api/users"
                    | "/api/auth/keys"
                    | "/api/runs"
                    | "/api/ai/config"
                    | "/api/clusters"
                    | "/api/license/upload"
            ) || run_control_path(path)
                || share_grant_path(path)
        }
        "PUT" => matches!(
            path,
            "/api/ai/config/server" | "/api/ai/config/user" | "/api/webhook"
        ),
        "DELETE" => {
            user_delete_path(path)
                || api_key_revoke_path(path)
                || cluster_delete_path(path)
                || pipeline_delete_path(path)
        }
        _ => false,
    };

    if required {
        AuditDurability::Required
    } else {
        AuditDurability::BestEffort
    }
}

/// `/api/runs/{id}/{cancel|pause|resume|retry|resume-checkpoint|clean}`.
fn run_control_path(path: &str) -> bool {
    const CONTROL_SUFFIXES: &[&str] = &[
        "cancel",
        "pause",
        "resume",
        "retry",
        "resume-checkpoint",
        "clean",
    ];
    let Some(rest) = path.strip_prefix("/api/runs/") else {
        return false;
    };
    let Some((_id, action)) = rest.split_once('/') else {
        return false;
    };
    CONTROL_SUFFIXES.contains(&action)
}

/// `/api/pipelines/{id}/share` — creating an access grant.
fn share_grant_path(path: &str) -> bool {
    path.strip_prefix("/api/pipelines/")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(id, action)| !id.is_empty() && action == "share")
}

/// `/api/users/{id}` — account deletion (role-bearing).
fn user_delete_path(path: &str) -> bool {
    one_segment_under(path, "/api/users/")
}

/// `/api/auth/keys/{id}` — machine-credential revocation.
fn api_key_revoke_path(path: &str) -> bool {
    one_segment_under(path, "/api/auth/keys/")
}

/// `/api/clusters/{id}` — stored SSH endpoint/credential removal.
fn cluster_delete_path(path: &str) -> bool {
    one_segment_under(path, "/api/clusters/")
}

/// `/api/pipelines/{id}` (no further segment) — pipeline deletion.
fn pipeline_delete_path(path: &str) -> bool {
    one_segment_under(path, "/api/pipelines/")
}

/// True when `path` is exactly `prefix` + one non-empty segment.
fn one_segment_under(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
}

/// Middleware: record every state-changing request (non-GET/HEAD/OPTIONS) in
/// the `audit_logs` table — the single audit write point covering all
/// mutation handlers (issue #79 P1-05: the table had schema but zero write
/// call sites).
///
/// Must be layered INSIDE `require_auth` (server.rs) so the authenticated
/// user id inserted into request extensions is visible; personal-mode
/// requests fall back to the 'default' pseudo-user.
///
/// Durability is per request ([`audit_durability_for`]): a failed REQUIRED
/// insert replaces a non-5xx already-computed response with a structured
/// `500 AUDIT_WRITE_FAILED` (fail closed); a response that is already a
/// server error is preserved (it is not an acknowledgment of success, and
/// the documented runs-domain 503 boundary must survive a no-backend
/// deployment). A failed BEST-EFFORT insert logs a warning and returns the
/// response unchanged.
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

    // Successful sign-ins are recorded by the login handler itself, which
    // knows the authenticated identity — the middleware only sees the
    // pre-auth request, so every login row it writes would be misattributed
    // to the `default` pseudo-user (web-api.md §Audit Logs contract).
    // Failed attempts still fall through here (actor unknown → `default`,
    // result: "failure" — the actionable brute-force signal).
    if login_success_is_handler_recorded(&path, status) {
        return response;
    }

    let result = if (200..300).contains(&status) {
        "success"
    } else {
        "failure"
    };
    let metadata = serde_json::json!({ "status": status }).to_string();

    // One write path, two explicit durability classes (WEB-15 / #1140).
    match audit_durability_for(method.as_str(), &path) {
        AuditDurability::Required => {
            if let Err((error_status, Json(error))) =
                audit_required(&user, &action, &path, result, Some(&metadata)).await
            {
                // The structured event stream still records the attempt for
                // machine consumers even when the database row is lost.
                emit_audit_event(&user, &action, &path, result);
                // A response that is ALREADY a server error is not an
                // acknowledgment of success — preserve it. This keeps the
                // documented runs-domain 503 boundary
                // (`RUNS_REQUIRE_STORAGE_BACKEND`, WEB-17) when a deployment
                // has no initialized backend at all: overwriting it with
                // AUDIT_WRITE_FAILED would hide the actionable cause while
                // still refusing the request. 2xx and 4xx answers are
                // replaced by the structured fail-closed 500.
                if status >= 500 {
                    return response;
                }
                return (error_status, Json(error)).into_response();
            }
        }
        AuditDurability::BestEffort => {
            audit_best_effort(&user, &action, &path, result, Some(&metadata)).await;
        }
    }
    // Structured event stream for machine consumers (issue #81).
    emit_audit_event(&user, &action, &path, result);

    response
}

/// THE audit writer. No other path may insert into `audit_logs`; the
/// durability helpers below both delegate here, and this delegates to the
/// storage backend's single `record_audit_log` implementation.
async fn write_audit(
    actor: &str,
    action: &str,
    target: &str,
    result: &str,
    metadata: Option<&str>,
) -> Result<(), String> {
    let backend = crate::infra::db::global_backend()
        .ok_or_else(|| "audit storage backend is not initialized".to_string())?;
    backend
        .record_audit_log(actor, action, target, result, metadata)
        .await
}

/// Write a REQUIRED audit row; a failure is returned as the structured
/// `500 AUDIT_WRITE_FAILED` every caller must propagate, so the request
/// fails closed.
///
/// Security-relevant call sites use this helper — never a bare storage
/// write — so the durability class is visible where the action happens.
pub async fn audit_required(
    actor: &str,
    action: &str,
    target: &str,
    result: &str,
    metadata: Option<&str>,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    match write_audit(actor, action, target, result, metadata).await {
        Ok(()) => Ok(()),
        Err(error) => {
            tracing::error!(
                actor,
                action,
                error = %error,
                "required audit write failed — refusing the request (fail closed)"
            );
            Err(audit_failure_error(action, &error))
        }
    }
}

/// Write a BEST-EFFORT audit row. A failure is logged as a warning and
/// never changes the response; informational events must not take the
/// feature down with the audit store.
pub async fn audit_best_effort(
    actor: &str,
    action: &str,
    target: &str,
    result: &str,
    metadata: Option<&str>,
) {
    if let Err(error) = write_audit(actor, action, target, result, metadata).await {
        tracing::warn!(
            actor,
            action,
            error = %error,
            "best-effort audit write failed — the request is unaffected"
        );
    }
}

/// The structured `500 AUDIT_WRITE_FAILED` body returned when a REQUIRED
/// audit row cannot be persisted.
///
/// The middleware writes the row AFTER the handler has run, and it is not in
/// the same transaction as the mutation — so the action may already have
/// committed. The message must not claim the request was "refused", and the
/// suggestion must warn that a blind retry can duplicate the effect.
pub fn audit_failure_error(action: &str, error: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            code: "AUDIT_WRITE_FAILED".into(),
            message: format!(
                "The audit record for '{action}' could not be written; the action may already \
                 have been applied"
            ),
            detail: Some(error.to_string()),
            suggestion: Some(
                "Verify the current state (list or inspect the affected resource) before \
                 retrying — retrying may duplicate the effect. Then check that the database is \
                 writable and the audit_logs table exists."
                    .into(),
            ),
        }),
    )
}

/// Structured event stream for machine consumers (issue #81). Emitted for
/// every mutation, including a REQUIRED write that failed — the event is the
/// remaining trail in that case.
fn emit_audit_event(user: &str, action: &str, path: &str, result: &str) {
    crate::domains::observability::logging::log_event(
        "audit",
        serde_json::json!({
            "user": user, "action": action, "resource": path, "result": result,
        }),
    );
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

/// Successful sign-ins are recorded by the login handler (which owns the
/// authenticated identity); the middleware records failed attempts only.
fn login_success_is_handler_recorded(path: &str, status: u16) -> bool {
    path == "/api/auth/login" && (200..300).contains(&status)
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

    /// Login-success audit rows belong to the login handler (canonical
    /// actor id); the middleware must not double-record them, but failed
    /// attempts stay in the middleware trail for brute-force analysis.
    #[test]
    fn login_success_rows_are_handler_owned() {
        assert!(login_success_is_handler_recorded("/api/auth/login", 200));
        assert!(login_success_is_handler_recorded("/api/auth/login", 204));
        assert!(!login_success_is_handler_recorded("/api/auth/login", 401));
        assert!(!login_success_is_handler_recorded("/api/auth/login", 429));
        assert!(!login_success_is_handler_recorded("/api/runs", 200));
    }

    /// WEB-15 / issue #1140: the durability class is a property of the
    /// route, and every security-relevant category named by the audit is
    /// REQUIRED. The table below is deliberately HARDCODED — a contract
    /// pin, not a derivation from the route registrations in `server.rs`
    /// (a route added there must be added here).
    #[test]
    fn audit_durability_matrix_covers_security_relevant_routes() {
        use AuditDurability::{BestEffort, Required};

        let cases: &[(&str, &str, AuditDurability)] = &[
            // Authentication and identity.
            ("POST", "/api/auth/login", Required),
            ("POST", "/api/auth/logout", Required),
            ("POST", "/api/auth/oauth/authorize", Required),
            ("POST", "/api/auth/oauth/callback", Required),
            ("POST", "/api/users", Required),
            ("DELETE", "/api/users/abc", Required),
            ("POST", "/api/auth/keys", Required),
            ("DELETE", "/api/auth/keys/abc", Required),
            ("POST", "/api/license/upload", Required),
            // Run lifecycle control.
            ("POST", "/api/runs", Required),
            ("POST", "/api/runs/abc/cancel", Required),
            ("POST", "/api/runs/abc/pause", Required),
            ("POST", "/api/runs/abc/resume", Required),
            ("POST", "/api/runs/abc/retry", Required),
            ("POST", "/api/runs/abc/resume-checkpoint", Required),
            ("POST", "/api/runs/abc/clean", Required),
            // Destructive pipeline actions and access grants.
            ("DELETE", "/api/pipelines/abc", Required),
            ("POST", "/api/pipelines/abc/share", Required),
            // Credential / key changes.
            ("POST", "/api/ai/config", Required),
            ("PUT", "/api/ai/config/server", Required),
            ("PUT", "/api/ai/config/user", Required),
            ("POST", "/api/clusters", Required),
            ("DELETE", "/api/clusters/abc", Required),
            ("PUT", "/api/webhook", Required),
            // Trailing slash is the same route.
            ("POST", "/api/runs/", Required),
            // Informational mutations stay best-effort.
            ("POST", "/api/pipelines/parse", BestEffort),
            ("POST", "/api/pipelines", BestEffort),
            ("PUT", "/api/pipelines/abc", BestEffort),
            ("POST", "/api/pipelines/abc/fork", BestEffort),
            ("POST", "/api/pipelines/abc/rollback", BestEffort),
            ("POST", "/api/runs/abc/report/ask", BestEffort),
            ("POST", "/api/chat/send", BestEffort),
            ("POST", "/api/ai/translate", BestEffort),
            ("POST", "/api/ai/test", BestEffort),
            ("POST", "/api/events/ticket", BestEffort),
            ("PUT", "/api/quota", BestEffort),
            ("DELETE", "/api/templates/abc", BestEffort),
            ("GET", "/api/runs", BestEffort),
        ];

        for (method, path, expected) in cases {
            assert_eq!(
                audit_durability_for(method, path),
                *expected,
                "{method} {path} must be {expected:?}"
            );
        }
    }

    /// The fail-closed body is the crate's structured error contract, not a
    /// bare 500 — and it must be honest about ordering: the audit row is
    /// written after the handler, not in its transaction, so the action may
    /// already have committed. It must say so and must not claim refusal or
    /// invite a blind retry.
    #[test]
    fn required_audit_failure_is_a_structured_500() {
        let (status, Json(error)) = audit_failure_error("POST /api/runs", "disk I/O error");
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.code, "AUDIT_WRITE_FAILED");
        assert!(error.message.contains("POST /api/runs"));
        assert!(
            error.message.contains("may already have been applied"),
            "the message must not imply the mutation was rolled back: {}",
            error.message
        );
        assert!(
            !error.message.contains("refused"),
            "the request was not refused — the handler already ran: {}",
            error.message
        );
        assert_eq!(error.detail.as_deref(), Some("disk I/O error"));
        let suggestion = error
            .suggestion
            .as_deref()
            .expect("remediation is documented");
        assert!(
            suggestion.contains("Verify the current state"),
            "the suggestion must direct the caller to inspect state first: {suggestion}"
        );
        assert!(
            suggestion.contains("duplicate"),
            "the suggestion must warn that retrying can duplicate effects: {suggestion}"
        );
    }
}
