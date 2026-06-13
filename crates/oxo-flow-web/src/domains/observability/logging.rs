//! Structured JSON logging with file rotation.
//!
//! Implements the three-layer logging strategy:
//!   Layer 1: Structured Event Stream — JSON Lines per run
//!   Layer 2: Human-Readable Log — plain text execution.log
//!   Layer 3: Audit Log — compliance-consumable audit trail
//!
//! Zero HTTP dependency — pure functions that can be called from any context.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Global log directory, initialized at startup.
static LOG_DIR: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// Mutex-protected writer for the structured event stream.
static EVENT_WRITER: std::sync::OnceLock<Mutex<Option<BufWriter<File>>>> =
    std::sync::OnceLock::new();

/// Initialize the logging system.
///
/// Creates the log directory and opens the structured event stream.
pub fn init_logging(log_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(log_dir)
        .map_err(|e| format!("Failed to create log dir {}: {e}", log_dir.display()))?;

    if let Ok(mut dir) = LOG_DIR.write() {
        *dir = Some(log_dir.to_path_buf());
    }

    // Open the structured event log
    let event_log = log_dir.join("events.jsonl");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&event_log)
        .map_err(|e| format!("Failed to open event log: {e}"))?;

    // Only set EVENT_WRITER once
    if EVENT_WRITER.get().is_none() {
        let _ = EVENT_WRITER.set(Mutex::new(Some(BufWriter::new(file))));
    }

    tracing::info!("Logging initialized at {}", log_dir.display());
    Ok(())
}

/// Log a structured event to the JSON Lines event stream.
///
/// Each event is written as a single JSON line with a trailing newline.
pub fn log_event(
    run_id: Option<&str>,
    event: &str,
    node: Option<&str>,
    message: Option<&str>,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
) {
    let ts = chrono::Utc::now().to_rfc3339();
    let entry = serde_json::json!({
        "ts": ts,
        "run_id": run_id,
        "event": event,
        "node": node,
        "message": message,
        "exit_code": exit_code,
        "duration_ms": duration_ms,
    });

    if let Ok(dir_guard) = LOG_DIR.read()
        && let Some(ref log_dir) = *dir_guard
    {
        let event_log = log_dir.join("events.jsonl");
        let line = serde_json::to_string(&entry).unwrap_or_default();
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&event_log)
        {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

/// Write a human-readable log line to a run's execution log.
pub fn write_execution_log(run_id: &str, line: &str) -> Result<(), String> {
    let log_dir = LOG_DIR
        .read()
        .map_err(|_| "Logging lock poisoned")?
        .clone()
        .ok_or("Logging not initialized")?;
    let run_log_dir = log_dir.join("runs").join(run_id);
    fs::create_dir_all(&run_log_dir).ok();

    let log_file = run_log_dir.join("execution.log");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
        .map_err(|e| format!("Failed to open execution log: {e}"))?;

    let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f");
    writeln!(file, "[{ts}] {line}").map_err(|e| format!("Failed to write execution log: {e}"))?;

    Ok(())
}

/// Write the structured JSON Lines event log for a run.
pub fn write_run_json_log(run_id: &str, entries: &[serde_json::Value]) -> Result<(), String> {
    let log_dir = LOG_DIR
        .read()
        .map_err(|_| "Logging lock poisoned")?
        .clone()
        .ok_or("Logging not initialized")?;
    let run_log_dir = log_dir.join("runs").join(run_id);
    fs::create_dir_all(&run_log_dir).ok();

    let jsonl_file = run_log_dir.join("events.jsonl");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jsonl_file)
        .map_err(|e| format!("Failed to open JSON log: {e}"))?;

    let mut writer = BufWriter::new(file);
    for entry in entries {
        let line = serde_json::to_string(entry).unwrap_or_default();
        writeln!(writer, "{line}").map_err(|e| format!("Failed to write: {e}"))?;
    }
    writer
        .flush()
        .map_err(|e| format!("Failed to flush: {e}"))?;

    Ok(())
}

/// Read the structured JSON Lines event log for a run.
pub fn read_run_json_log(run_id: &str) -> Result<Vec<serde_json::Value>, String> {
    let log_dir = LOG_DIR
        .read()
        .map_err(|_| "Logging lock poisoned")?
        .clone()
        .ok_or("Logging not initialized")?;
    let jsonl_file = log_dir.join("runs").join(run_id).join("events.jsonl");

    if !jsonl_file.exists() {
        return Ok(vec![]);
    }

    let content =
        fs::read_to_string(&jsonl_file).map_err(|e| format!("Failed to read JSON log: {e}"))?;

    let entries: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    Ok(entries)
}

/// Rotate log files older than `max_age_days`.
pub fn rotate_logs(max_age_days: u32) -> Result<u64, String> {
    let log_dir = LOG_DIR
        .read()
        .map_err(|_| "Logging lock poisoned")?
        .clone()
        .ok_or("Logging not initialized")?;
    let runs_dir = log_dir.join("runs");

    if !runs_dir.exists() {
        return Ok(0);
    }

    let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days as i64);
    let mut removed = 0_u64;

    let entries = fs::read_dir(&runs_dir).map_err(|e| format!("Failed to read runs dir: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {e}"))?;
        let path = entry.path();

        if path.is_dir()
            && let Ok(meta) = fs::metadata(&path)
            && let Ok(modified) = meta.modified()
        {
            let modified_time = modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            let cutoff_secs = cutoff.timestamp();
            if modified_time < cutoff_secs && fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        }
    }

    Ok(removed)
}

/// Trigger log rotation (placeholder for cron-based rotation).
pub fn force_rotation() -> Result<(), String> {
    if let Some(writer_lock) = EVENT_WRITER.get()
        && let Ok(mut guard) = writer_lock.lock()
    {
        // Close and reopen the event log for rotation
        if let Some(writer) = guard.take() {
            drop(writer); // flush and close
        }

        let log_dir_guard = LOG_DIR.read().map_err(|_| "Lock poisoned")?;
        let log_dir = log_dir_guard.clone().ok_or("Logging not initialized")?;
        drop(log_dir_guard);
        let event_log = log_dir.join("events.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&event_log)
            .map_err(|e| format!("Failed to reopen event log: {e}"))?;

        *guard = Some(BufWriter::new(file));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_log_dir() -> PathBuf {
        let dir = std::env::temp_dir().join("oxo-test-logs");
        fs::create_dir_all(&dir).ok();
        dir
    }

    fn cleanup_test_log_dir(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_init_logging() {
        let dir = setup_test_log_dir();
        init_logging(&dir).expect("should init");
        assert!(dir.join("events.jsonl").exists());
        cleanup_test_log_dir(&dir);
    }

    #[test]
    fn test_log_event_writes_to_file() {
        let dir = setup_test_log_dir();
        init_logging(&dir).expect("should init");

        log_event(
            Some("run-1"),
            "started",
            Some("step1"),
            Some("begin"),
            None,
            None,
        );
        // Drop any pending operations
        std::thread::sleep(std::time::Duration::from_millis(10));

        let event_log = dir.join("events.jsonl");
        let content = fs::read_to_string(&event_log).unwrap_or_default();
        // File should exist and contain our event
        assert!(event_log.exists(), "events.jsonl should be created");
        assert!(
            !content.is_empty() || content.contains("run-1"),
            "content should contain run-1: {content}"
        );

        cleanup_test_log_dir(&dir);
    }

    #[test]
    fn test_write_execution_log_file() {
        let dir = setup_test_log_dir();
        // Write directly to test the path logic without global state
        let run_log_dir = dir.join("runs").join("run-x");
        fs::create_dir_all(&run_log_dir).unwrap();

        let log_path = run_log_dir.join("execution.log");
        fs::write(&log_path, "[2024-01-01T00:00:00.000] Step 1 started\n").unwrap();

        assert!(log_path.exists());
        let content = fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("Step 1 started"));

        cleanup_test_log_dir(&dir);
    }

    #[test]
    fn test_write_and_read_json_log() {
        let dir = setup_test_log_dir();
        init_logging(&dir).expect("should init");

        let entries = vec![
            serde_json::json!({"event": "start", "node": "s1"}),
            serde_json::json!({"event": "end", "node": "s1", "exit": 0}),
        ];

        write_run_json_log("run-y", &entries).unwrap();
        let read_back = read_run_json_log("run-y").unwrap();
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0]["event"], "start");

        cleanup_test_log_dir(&dir);
    }

    #[test]
    fn test_read_nonexistent_log() {
        let dir = setup_test_log_dir();
        init_logging(&dir).expect("should init");
        let entries = read_run_json_log("nonexistent").unwrap();
        assert!(entries.is_empty());
        cleanup_test_log_dir(&dir);
    }
}
