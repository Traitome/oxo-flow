//! Sampled peak-RSS metering for local rule execution (issue #67 §4).
//!
//! Rules deliberately run in the *run's* process group (one run = one
//! group, so supervisors can signal the run as a whole — issue #79), so
//! per-rule attribution cannot use process groups: one shared background
//! sampler refreshes the process table at a fixed interval and updates
//! each tracked child's peak as the summed RSS of its process subtree
//! (parent-link walk, the same shape `timeout::kill_process_tree` uses).
//!
//! The metric is a **sampled peak**, not an exact `getrusage` maximum:
//! sub-interval spikes can be missed. That is sufficient for bottleneck
//! detection (sustained pressure), and is documented in the diagnostics
//! API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sysinfo::ProcessesToUpdate;
use tokio::sync::Notify;

/// Sample interval for peak detection.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

/// Shared background sampler: one per `LocalExecutor` (i.e. one per run
/// process), so concurrent rules share a single process-table refresh
/// instead of N independent sysinfo scans.
pub struct RssSampler {
    tracked: Arc<Mutex<HashMap<u32, Arc<AtomicU64>>>>,
    wake: Arc<Notify>,
    stop: Arc<AtomicBool>,
}

/// A tracked child process. Dropping the handle untracks the pid; the
/// peak stays readable until the handle itself is dropped.
pub struct RssHandle {
    pid: u32,
    peak: Arc<AtomicU64>,
    tracked: Arc<Mutex<HashMap<u32, Arc<AtomicU64>>>>,
}

impl RssSampler {
    /// Start the sampler background worker.
    ///
    /// The worker runs on its **own thread with its own tokio runtime** —
    /// `LocalExecutor` may be constructed from synchronous contexts (tests,
    /// previews) where no ambient runtime exists, and `tokio::spawn` would
    /// panic. The same dedicated-thread bridge `head_blocking` uses for
    /// storage backends.
    pub fn new() -> Self {
        let tracked: Arc<Mutex<HashMap<u32, Arc<AtomicU64>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let wake = Arc::new(Notify::new());
        let stop = Arc::new(AtomicBool::new(false));

        {
            let tracked = tracked.clone();
            let wake = wake.clone();
            let stop = stop.clone();
            let thread = std::thread::Builder::new()
                .name("oxo-rss-sampler".into())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("rss sampler runtime");
                    runtime.block_on(async move {
                        loop {
                            tokio::select! {
                                () = wake.notified() => {}
                                _ = tokio::time::sleep(SAMPLE_INTERVAL) => {}
                            }
                            if stop.load(Ordering::Relaxed) {
                                return;
                            }
                            let snapshot: Vec<(u32, Arc<AtomicU64>)> = {
                                let guard = tracked.lock().expect("rss sampler lock");
                                if guard.is_empty() {
                                    continue;
                                }
                                guard
                                    .iter()
                                    .map(|(pid, peak)| (*pid, peak.clone()))
                                    .collect()
                            };
                            let mut system = sysinfo::System::new();
                            system.refresh_processes(ProcessesToUpdate::All, false);
                            for (pid, peak) in &snapshot {
                                peak.fetch_max(subtree_rss_bytes(&system, *pid), Ordering::Relaxed);
                            }
                        }
                    });
                });
            if let Err(e) = thread {
                tracing::warn!(error = %e, "rss sampler thread failed to start — peak-RSS metering disabled");
            }
        }

        Self {
            tracked,
            wake,
            stop,
        }
    }

    /// Track a child process; returns the handle to finish with.
    pub fn track(&self, pid: u32) -> RssHandle {
        let peak = Arc::new(AtomicU64::new(0));
        self.tracked
            .lock()
            .expect("rss sampler lock")
            .insert(pid, peak.clone());
        self.wake.notify_one();
        RssHandle {
            pid,
            peak,
            tracked: self.tracked.clone(),
        }
    }
}

impl Default for RssSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RssSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.notify_one();
    }
}

impl RssHandle {
    /// Finish tracking and return the sampled peak in bytes.
    pub fn finish(self) -> u64 {
        let peak = self.peak.load(Ordering::Relaxed);
        if let Ok(mut guard) = self.tracked.lock() {
            guard.remove(&self.pid);
        }
        peak
    }

    /// The sampled peak so far (bytes).
    pub fn peak_bytes(&self) -> u64 {
        self.peak.load(Ordering::Relaxed)
    }
}

impl Drop for RssHandle {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.tracked.lock() {
            guard.remove(&self.pid);
        }
    }
}

/// Summed RSS (bytes) of `root` and all its live descendants.
fn subtree_rss_bytes(system: &sysinfo::System, root: u32) -> u64 {
    let mut targets = vec![root];
    // Expand outward: anything whose parent is already in the set.
    loop {
        let mut found = false;
        for (child_pid, proc) in system.processes() {
            let child = child_pid.as_u32();
            if targets.contains(&child) {
                continue;
            }
            if let Some(parent) = proc.parent()
                && targets.contains(&parent.as_u32())
            {
                targets.push(child);
                found = true;
            }
        }
        if !found {
            break;
        }
    }
    targets
        .iter()
        .filter_map(|pid| system.process(sysinfo::Pid::from_u32(*pid)))
        .map(|p| p.memory())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spawn a memory-eater child — ~64 MiB allocated and held for ~1.2 s,
    /// RSS the sampler must see. Prefers `perl` (present on macOS and Linux
    /// CI images) with `python3` as fallback; skips gracefully when neither
    /// exists.
    #[test]
    fn sampler_tracks_child_subtree_peak() {
        let child = std::process::Command::new("perl")
            .args(["-e", "$x = \"a\" x (64 * 1024 * 1024); sleep 2;"])
            .spawn()
            .or_else(|_| {
                std::process::Command::new("python3")
                    .args([
                        "-c",
                        "import time; x = bytearray(64 * 1024 * 1024); time.sleep(1.2)",
                    ])
                    .spawn()
            });
        let mut child = match child {
            Ok(c) => c,
            Err(_) => {
                eprintln!("skipped: neither perl nor python3 available");
                return;
            }
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let sampler = RssSampler::new();
            let handle = sampler.track(child.id());
            let status = child.wait().unwrap();
            assert!(status.success());
            // Allow the last sample tick to land before finishing.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let peak = handle.finish();
            // The 64 MiB allocation must show up in the sampled subtree
            // peak — with generous slack for platform RSS accounting.
            let peak_mb = peak / (1024 * 1024);
            assert!(
                (40..=4096).contains(&peak_mb),
                "expected a peak in the tens-of-MB range, got {peak_mb} MiB"
            );
        });
    }

    #[test]
    fn sampler_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<RssSampler>();
        assert_sync::<RssSampler>();
    }
}
