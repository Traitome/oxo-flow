//! Audit logging module for oxo-flow-web.
//!
//! Provides file-based audit logging with automatic rotation.
//! Logs are stored in `logs/audit/YYYY-MM-DD.log` format.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// Maximum age in days for audit logs before cleanup.
const MAX_LOG_AGE_DAYS: u64 = 30;

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
/// write_audit_log("user123", "workflow.run", "my-workflow");
/// write_audit_log("admin", "user.delete", "user456");
/// ```
pub fn write_audit_log(user_id: &str, action: &str, resource: &str) -> std::io::Result<()> {
    ensure_audit_dir()?;

    let entry = AuditEntry {
        timestamp: Utc::now().to_rfc3339(),
        user: user_id.to_string(),
        action: action.to_string(),
        resource: resource.to_string(),
    };

    let path = audit_log_path(Utc::now());
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;

    let json = serde_json::to_string(&entry)?;
    writeln!(file, "{}", json)?;

    Ok(())
}

/// Rotate audit logs by removing files older than the retention period.
///
/// Files older than `MAX_LOG_AGE_DAYS` days are deleted.
/// This should be called periodically (e.g., daily via a scheduler).
pub fn rotate_audit_logs() -> std::io::Result<u64> {
    let dir = audit_log_dir();
    if !dir.exists() {
        return Ok(0);
    }

    let cutoff = Utc::now() - chrono::Duration::days(MAX_LOG_AGE_DAYS as i64);
    let cutoff_date = cutoff.format("%Y-%m-%d").to_string();

    let mut removed_count = 0;

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();

        // Check if it's a log file and older than cutoff
        if let Some(filename) = path.file_name().and_then(|n| n.to_str())
            && filename.ends_with(".log")
        {
            // Extract date from filename (YYYY-MM-DD.log)
            let date_str = filename.trim_end_matches(".log");
            // Compare &str with &str: cutoff_date is String, we take &cutoff_date which derefs to &str
            let cutoff_str: &str = &cutoff_date;
            if cutoff_str > date_str {
                fs::remove_file(&path)?;
                removed_count += 1;
            }
        }
    }

    Ok(removed_count)
}

/// Get recent audit log entries.
///
/// Reads logs from the last `days` days and returns them as JSON lines.
/// Entries are sorted by timestamp (newest first).
///
/// # Arguments
///
/// * `days` - Number of days to look back (1-30)
///
/// # Returns
///
/// A vector of JSON strings, each representing an AuditEntry.
pub fn get_recent_audit_logs(days: u8) -> std::io::Result<Vec<String>> {
    let days = days.clamp(1, 30) as i64;
    let dir = audit_log_dir();
    let mut entries = Vec::new();

    if !dir.exists() {
        return Ok(entries);
    }

    for day_offset in 0..days {
        let date = Utc::now() - chrono::Duration::days(day_offset);
        let path = audit_log_path(date);

        if path.exists() {
            let file = File::open(&path)?;
            let reader = BufReader::new(file);

            for line in reader.lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    entries.push(line);
                }
            }
        }
    }

    // Sort by timestamp (newest first) - parse each entry and sort
    entries.sort_by(|a, b| {
        let ts_a = serde_json::from_str::<AuditEntry>(a)
            .map(|e| e.timestamp)
            .unwrap_or_default();
        let ts_b = serde_json::from_str::<AuditEntry>(b)
            .map(|e| e.timestamp)
            .unwrap_or_default();
        ts_b.cmp(&ts_a)
    });

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditEntry {
            timestamp: "2024-01-15T10:30:00Z".to_string(),
            user: "testuser".to_string(),
            action: "test.action".to_string(),
            resource: "test-resource".to_string(),
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

    #[test]
    fn test_write_and_read_audit_logs() {
        let temp_dir = tempdir().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        // Write some entries
        write_audit_log("user1", "login", "system").unwrap();
        write_audit_log("user2", "workflow.run", "test-workflow").unwrap();

        // Read them back
        let logs = get_recent_audit_logs(1).unwrap();
        assert_eq!(logs.len(), 2);

        // Verify newest first
        let first: AuditEntry = serde_json::from_str(&logs[0]).unwrap();
        assert_eq!(first.action, "workflow.run");

        std::env::set_current_dir(original_dir).unwrap();
    }
}
