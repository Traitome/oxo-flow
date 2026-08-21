//! Run-log persistence: every `run` (and `resume`, which re-enters the same
//! path) writes the engine's tracing stream into a per-workdir log file,
//! rotating previous logs with numbered backups (`oxo-flow.log` →
//! `oxo-flow.log.1` → …). Each run therefore leaves its own archived record,
//! headed by the exact workflow version (name, version, git HEAD SHA) that
//! produced it (issue #115 pillar 1 extension).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tracing_subscriber::fmt::MakeWriter;

/// Number of numbered backups kept per run log (`.1` … `.9`); the oldest
/// backup is deleted on every rotation.
pub const RUN_LOG_BACKUPS: u32 = 9;

static ACTIVE_LOG: OnceLock<Arc<Mutex<Option<File>>>> = OnceLock::new();

fn slot() -> &'static Arc<Mutex<Option<File>>> {
    ACTIVE_LOG.get_or_init(|| Arc::new(Mutex::new(None)))
}

/// True while a run log file is armed (tracing events tee into it).
#[cfg(test)]
pub fn is_run_log_active() -> bool {
    slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some()
}

/// Disarm the tee and close the active run log file (the guard's `Drop`
/// does this too; exposed for explicit completion).
pub fn deactivate_run_log() {
    if let Some(mut file) = slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        let _ = file.flush();
    }
}

fn numbered_path(base: &Path, n: u32) -> PathBuf {
    PathBuf::from(format!("{}.{}", base.display(), n))
}

/// Shift existing logs one slot up: `<base>` → `<base>.1`, `<base>.i` →
/// `<base>.(i+1)`; backups beyond [`RUN_LOG_BACKUPS`] are deleted. No-op
/// when neither the base nor any backup exists.
pub fn rotate_run_log(base: &Path) -> io::Result<()> {
    // Drop every overflow slot first so the shifts below never collide.
    let mut j = RUN_LOG_BACKUPS + 1;
    loop {
        let overflow = numbered_path(base, j);
        if overflow.exists() {
            fs::remove_file(&overflow)?;
            j += 1;
        } else {
            break;
        }
    }
    // Shift from the highest kept backup down so each rename target is free.
    for i in (1..=RUN_LOG_BACKUPS).rev() {
        let from = if i == 1 {
            base.to_path_buf()
        } else {
            numbered_path(base, i - 1)
        };
        if from.exists() {
            fs::rename(&from, numbered_path(base, i))?;
        }
    }
    Ok(())
}

/// Rotate previous logs, open `path` as the new current run log, and write
/// `header` as its first content. Returns a guard that keeps the tee armed
/// (tracing events are duplicated into the file) until it is dropped.
pub fn activate_run_log(path: &Path, header: &str) -> io::Result<RunLogGuard> {
    rotate_run_log(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(header.as_bytes())?;
    let clone = file.try_clone()?;
    *slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(file);
    Ok(RunLogGuard { file: Some(clone) })
}

/// Keeps a run log armed. Dropping it disarms the tee (flush + close), so
/// every early return from `run_command` still finalizes the log.
pub struct RunLogGuard {
    file: Option<File>,
}

impl Write for RunLogGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.file {
            Some(f) => f.write(buf),
            None => Ok(0),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

impl Drop for RunLogGuard {
    fn drop(&mut self) {
        if let Some(f) = &mut self.file {
            let _ = f.flush();
        }
        deactivate_run_log();
    }
}

/// The tracing writer installed in `main`: stderr only while no run log is
/// armed; stderr + the active run log file once [`activate_run_log`] runs.
/// File write failures are reported once to stderr and the tee degrades to
/// stderr-only — a logging hiccup never fails the run.
pub struct TeeWriter;

impl<'a> MakeWriter<'a> for TeeWriter {
    type Writer = Box<dyn Write + 'a>;

    fn make_writer(&'a self) -> Self::Writer {
        let file = slot()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|f| f.try_clone().ok());
        match file {
            Some(f) => Box::new(Tee {
                stderr: io::stderr(),
                file: Some(f),
            }),
            None => Box::new(io::stderr()),
        }
    }
}

static FILE_WRITE_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);

struct Tee {
    stderr: io::Stderr,
    file: Option<File>,
}

/// Write `buf` to `out` with ANSI CSI escape sequences stripped — run-log
/// files are plain text, colors stay on stderr only. `tracing` formats each
/// event as one write, so sequences are never split across calls.
fn write_plain(out: &mut File, buf: &[u8]) -> io::Result<()> {
    let mut plain = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == 0x1b && i + 1 < buf.len() && buf[i + 1] == b'[' {
            // CSI sequence: ESC '[' parameters final-byte.
            i += 2;
            while i < buf.len() && !(0x40..=0x7e).contains(&buf[i]) {
                i += 1;
            }
            if i < buf.len() {
                i += 1; // consume the final byte
            }
        } else if buf[i] == 0x1b {
            i += 1; // stray escape — drop it
        } else {
            plain.push(buf[i]);
            i += 1;
        }
    }
    out.write_all(&plain)
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = self.stderr.write(buf);
        if let Some(f) = &mut self.file
            && let Err(e) = write_plain(f, buf)
        {
            if !FILE_WRITE_ERROR_REPORTED.swap(true, Ordering::Relaxed) {
                let _ = writeln!(
                    self.stderr,
                    "warning: run log write failed ({e}); continuing without file logging"
                );
            }
            self.file = None;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.stderr.flush();
        if let Some(f) = &mut self.file {
            f.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxo-runlog-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rotate_shifts_numbered_backups_and_caps() {
        let dir = scratch("rotate");
        let base = dir.join("oxo-flow.log");
        fs::write(&base, "latest").unwrap();
        for i in 1..=(RUN_LOG_BACKUPS + 3) {
            fs::write(dir.join(format!("oxo-flow.log.{i}")), format!("backup{i}")).unwrap();
        }
        rotate_run_log(&base).unwrap();
        // The old current log becomes .1; every .i shifts one slot up.
        assert_eq!(
            fs::read_to_string(dir.join("oxo-flow.log.1")).unwrap(),
            "latest"
        );
        for i in 2..=RUN_LOG_BACKUPS {
            assert_eq!(
                fs::read_to_string(dir.join(format!("oxo-flow.log.{i}"))).unwrap(),
                format!("backup{}", i - 1)
            );
        }
        // Oldest backups beyond the cap are deleted, never shifted further.
        for i in (RUN_LOG_BACKUPS + 1)..=(RUN_LOG_BACKUPS + 3) {
            assert!(!dir.join(format!("oxo-flow.log.{i}")).exists());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_without_existing_logs_is_a_noop() {
        let dir = scratch("rotate-noop");
        let base = dir.join("oxo-flow.log");
        rotate_run_log(&base).unwrap();
        assert!(!base.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn activate_creates_parent_dirs_and_writes_header() {
        let dir = scratch("activate");
        let log = dir.join("nested/.oxo-flow/logs/oxo-flow.log");
        let mut guard = activate_run_log(&log, "run header\nsecond line\n").unwrap();
        let content = fs::read_to_string(&log).unwrap();
        assert!(content.starts_with("run header\n"));
        assert!(content.contains("second line"));
        // The guard tees writes into the active file.
        guard.write_all(b"event: rule x started\n").unwrap();
        guard.flush().unwrap();
        let content = fs::read_to_string(&log).unwrap();
        assert!(content.contains("event: rule x started"));
        assert!(is_run_log_active());
        drop(guard);
        // Deactivation on drop: no run log stays armed.
        assert!(!is_run_log_active());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tee_strips_ansi_codes_from_file_writes() {
        let dir = scratch("ansi");
        let log = dir.join("run.log");
        let _guard = activate_run_log(&log, "").unwrap();
        let file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        let mut tee = Tee {
            stderr: io::stderr(),
            file: Some(file),
        };
        tee.write_all(b"\x1b[2m2026-08-21T14:54:11Z\x1b[0m \x1b[32m INFO\x1b[0m rule started\n")
            .unwrap();
        tee.flush().unwrap();
        let content = fs::read_to_string(&log).unwrap();
        assert!(content.contains(" INFO rule started"));
        assert!(
            !content.contains("\x1b["),
            "run-log files must be plain text"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn activate_replaces_previous_run_log_after_rotation() {
        let dir = scratch("replace");
        let base = dir.join("oxo-flow.log");
        fs::write(&base, "old run").unwrap();
        let _guard = activate_run_log(&base, "new header\n").unwrap();
        let content = fs::read_to_string(&base).unwrap();
        assert!(content.starts_with("new header\n"));
        assert!(!content.contains("old run"));
        // The previous log was rotated into .1 before truncation.
        assert_eq!(
            fs::read_to_string(dir.join("oxo-flow.log.1")).unwrap(),
            "old run"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
