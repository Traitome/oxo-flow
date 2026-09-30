//! Audit logging module for oxo-flow-web.
//!
//! Provides file-based audit logging with automatic rotation.
//! Logs are stored in `logs/audit/YYYY-MM-DD.log` format.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// Audit log entry format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO 8601 timestamp of the event.
    pub timestamp: String,
    /// User identifier who performed the action.
    pub user: String,
    /// Action that was performed (e.g., "login", "workflow.run", "workflow.delete").
    pub action: String,
    /// Resource affected by the action (e.g., workflow name, file path).
    pub resource: String,
    /// Outcome of the action ("success" or "failure").
    pub result: String,
}

/// Get the audit log directory path.
fn audit_log_dir() -> PathBuf {
    PathBuf::from("logs/audit")
}

/// Get the audit log file path for a specific date.
fn audit_log_path(date: DateTime<Utc>) -> PathBuf {
    audit_log_dir().join(format!("{}.log", date.format("%Y-%m-%d")))
}

/// Ensure the audit log directory exists.
fn ensure_audit_dir() -> std::io::Result<()> {
    let dir = audit_log_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    Ok(())
}

/// Write an audit log entry.
///
/// Creates the log directory if it doesn't exist. Entries are written
/// as JSON lines to `logs/audit/YYYY-MM-DD.log`.
///
/// # Example
///
/// ```ignore
/// write_audit_log("user123", "workflow.run", "my-workflow", "success");
/// write_audit_log("admin", "user.delete", "user456", "success");
/// ```
pub fn write_audit_log(
    user_id: &str,
    action: &str,
    resource: &str,
    result: &str,
) -> std::io::Result<()> {
    ensure_audit_dir()?;

    let entry = AuditEntry {
        timestamp: Utc::now().to_rfc3339(),
        user: user_id.to_string(),
        action: action.to_string(),
        resource: resource.to_string(),
        result: result.to_string(),
    };

    let path = audit_log_path(Utc::now());
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;

    let json = serde_json::to_string(&entry)?;
    writeln!(file, "{}", json)?;

    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditEntry {
            timestamp: "2024-01-15T10:30:00Z".to_string(),
            user: "testuser".to_string(),
            action: "test.action".to_string(),
            resource: "test-resource".to_string(),
            result: "success".to_string(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("testuser"));
        assert!(json.contains("test.action"));
        assert!(json.contains("test-resource"));

        let parsed: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.user, entry.user);
        assert_eq!(parsed.action, entry.action);
        assert_eq!(parsed.resource, entry.resource);
    }
}
