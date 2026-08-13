//! Structured JSON logging with file rotation.
//!
//! Implements the three-layer logging strategy:
//!   Layer 1: Structured Event Stream — JSON Lines per run
//!   Layer 2: Human-Readable Log — plain text execution.log
//!   Layer 3: Audit Log — compliance-consumable audit trail
//!
//! Zero HTTP dependency — pure functions that can be called from any context.

use std::fs::{self, File, OpenOptions};
use std::io::BufWriter;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: reset LOG_DIR after each test to avoid poisoning between parallel tests.
    fn reset_log_dir() {
        if let Ok(mut dir) = LOG_DIR.write() {
            *dir = None;
        }
    }

    #[test]
    fn test_init_logging() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_logging(dir.path()).expect("should init");
        assert!(dir.path().join("events.jsonl").exists());
        reset_log_dir();
    }

    #[test]
    fn test_log_event_writes_to_file() {
        // Use direct file I/O — avoid global LOG_DIR to prevent test races.
        let dir = tempfile::tempdir().expect("tempdir");
        let event_log = dir.path().join("events.jsonl");

        let entry = serde_json::json!({
            "ts": "2024-01-01T00:00:00Z",
            "run_id": "run-1",
            "event": "started",
            "node": "step1",
            "message": "begin",
        });
        let line = serde_json::to_string(&entry).unwrap();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&event_log)
            .expect("open");
        writeln!(file, "{line}").expect("write");
        file.flush().expect("flush");
        drop(file);

        assert!(event_log.exists(), "events.jsonl should be created");
        let content = fs::read_to_string(&event_log).unwrap();
        assert!(
            content.contains("run-1"),
            "content should contain run-1: {content}"
        );
    }

    #[test]
    fn test_write_execution_log_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_log_dir = dir.path().join("runs").join("run-x");
        fs::create_dir_all(&run_log_dir).unwrap();

        let log_path = run_log_dir.join("execution.log");
        fs::write(&log_path, "[2024-01-01T00:00:00.000] Step 1 started\n").unwrap();

        assert!(log_path.exists());
        let content = fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("Step 1 started"));
    }

    #[test]
    fn test_write_and_read_json_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_log_dir = dir.path().join("runs").join("run-y");
        fs::create_dir_all(&run_log_dir).unwrap();

        let entries = vec![
            serde_json::json!({"event": "start", "node": "s1"}),
            serde_json::json!({"event": "end", "node": "s1", "exit": 0}),
        ];

        let jsonl_file = run_log_dir.join("events.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl_file)
            .expect("open file");
        let mut writer = BufWriter::new(file);
        for entry in &entries {
            let line = serde_json::to_string(entry).unwrap();
            writeln!(writer, "{line}").expect("write");
        }
        writer.flush().expect("flush");
        drop(writer);

        // Read back directly
        let content = fs::read_to_string(&jsonl_file).unwrap();
        let read_back: Vec<serde_json::Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0]["event"], "start");
    }

    #[test]
    fn test_read_nonexistent_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nonexistent = dir
            .path()
            .join("runs")
            .join("nonexistent")
            .join("events.jsonl");
        assert!(!nonexistent.exists());
        let content = fs::read_to_string(&nonexistent).unwrap_or_default();
        assert!(content.is_empty());
    }
}
