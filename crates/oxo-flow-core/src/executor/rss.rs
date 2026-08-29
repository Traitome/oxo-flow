//! Sampled peak-RSS and CPU-seconds metering for local rule execution
//! (issue #67 §4, issue #83 P1-13).
//!
//! Rules deliberately run in the *run's* process group (one run = one
//! group, so supervisors can signal the run as a whole — issue #79), so
//! per-rule attribution cannot use process groups: one shared background
//! sampler refreshes the process table at a fixed interval and updates
//! each tracked child's metrics from its process subtree (parent-link
//! walk, the same shape `timeout::kill_process_tree` uses):
//!
//! - **Peak RSS**: the summed RSS of the subtree in bytes.
//! - **CPU time**: `Process::cpu_usage()/100 × SAMPLE_INTERVAL_SECS`
//!   accumulated per tick (integer microseconds, float-free atomics).
//!
//! Both metrics are **sampled**, not exact `getrusage` values: sub-interval
//! spikes can be missed, and CPU seconds assume every tick spanned exactly
//! `SAMPLE_INTERVAL_SECS`. That is sufficient for bottleneck detection
//! (sustained pressure), and is documented in the diagnostics API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sysinfo::ProcessesToUpdate;
use tokio::sync::Notify;

/// Sample interval for peak detection.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

/// Seconds of wall time between sample ticks — the per-tick basis for
/// CPU-time accumulation (`cpu_usage()/100 × SAMPLE_INTERVAL_SECS`).
/// Derived from `SAMPLE_INTERVAL` so the two can never drift apart.
const SAMPLE_INTERVAL_SECS: f64 = SAMPLE_INTERVAL.as_secs_f64();

/// Per-pid accumulators for one tracked child. Integer counts keep the
/// shared atomics float-free; `cpu_seconds()` converts on read.
struct TrackedProcess {
    /// Sampled peak subtree RSS in bytes.
    peak: AtomicU64,
    /// Sampled CPU time in microseconds.
    cpu_micros: AtomicU64,
    /// Set once the sampler has seen the process alive during a tick.
    seen: AtomicBool,
}

/// Shared background sampler: one per `LocalExecutor` (i.e. one per run
/// process), so concurrent rules share a single process-table refresh
/// instead of N independent sysinfo scans.
pub struct RssSampler {
    tracked: Arc<Mutex<HashMap<u32, Arc<TrackedProcess>>>>,
    wake: Arc<Notify>,
    stop: Arc<AtomicBool>,
}

/// A tracked child process. Dropping the handle untracks the pid; the
/// peak and CPU totals stay readable until the handle itself is dropped.
pub struct RssHandle {
    pid: u32,
    tracked_process: Arc<TrackedProcess>,
    tracked: Arc<Mutex<HashMap<u32, Arc<TrackedProcess>>>>,
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
        let tracked: Arc<Mutex<HashMap<u32, Arc<TrackedProcess>>>> =
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
                        // One `System` is reused across ticks: sysinfo
                        // computes `Process::cpu_usage()` only from the
                        // delta between two refreshes of the *same*
                        // instance (a fresh instance per tick would always
                        // report 0). Dead processes are dropped each
                        // refresh so the table stays bounded over long runs.
                        let mut system = sysinfo::System::new();
                        loop {
                            tokio::select! {
                                () = wake.notified() => {}
                                _ = tokio::time::sleep(SAMPLE_INTERVAL) => {}
                            }
                            if stop.load(Ordering::Relaxed) {
                                return;
                            }
                            let snapshot: Vec<(u32, Arc<TrackedProcess>)> = {
                                // A poisoned lock must not panic the sampler
                                // thread (issue #194 §3.5) — the worst case
                                // is one skipped tick, not a lost sampler.
                                let guard = tracked
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                if guard.is_empty() {
                                    continue;
                                }
                                guard.iter().map(|(pid, tp)| (*pid, tp.clone())).collect()
                            };
                            system.refresh_processes(ProcessesToUpdate::All, true);
                            // One shared parent→children map per tick (built
                            // once, O(P)), plus one shared subtree walk per
                            // root. The old shape called `subtree_rss_bytes`
                            // per tracked child, each re-scanning the whole
                            // process table in a fixpoint loop —
                            // O(ticks × rules × passes × P) (#268 item 4).
                            let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
                            for (pid, proc) in system.processes() {
                                if let Some(parent) = proc.parent() {
                                    children_of
                                        .entry(parent.as_u32())
                                        .or_default()
                                        .push(pid.as_u32());
                                }
                            }
                            for (pid, tp) in &snapshot {
                                // CPU accumulates only while the process is
                                // alive: a dead-but-retained entry or a
                                // failed lookup must not inflate the total
                                // (that pid's tick is skipped instead).
                                // CPU accumulates only while the process is
                                // alive. With `remove_dead_processes=true`
                                // dead pids are already dropped from the
                                // table, so this exists() gate is a cheap
                                // defensive check for lookup races rather
                                // than a real failure path — it must never
                                // inflate the total.
                                if let Some(proc) = system.process(sysinfo::Pid::from_u32(*pid))
                                    && proc.exists()
                                {
                                    let micros = (proc.cpu_usage() as f64 / 100.0
                                        * SAMPLE_INTERVAL_SECS
                                        * 1_000_000.0)
                                        as u64;
                                    // The delta lands *before* the SeqCst
                                    // seen-store publishes it, so a reader
                                    // that observes seen=true in
                                    // `cpu_seconds()` must also observe this
                                    // tick's cpu_micros update (no one-tick
                                    // under-count).
                                    tp.cpu_micros.fetch_add(micros, Ordering::SeqCst);
                                    tp.seen.store(true, Ordering::SeqCst);
                                }
                                tp.peak.fetch_max(
                                    subtree_rss_bytes(&children_of, &system, *pid),
                                    Ordering::Relaxed,
                                );
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
        let tracked_process = Arc::new(TrackedProcess {
            peak: AtomicU64::new(0),
            cpu_micros: AtomicU64::new(0),
            seen: AtomicBool::new(false),
        });
        self.tracked
            .lock()
            .expect("rss sampler lock")
            .insert(pid, tracked_process.clone());
        self.wake.notify_one();
        RssHandle {
            pid,
            tracked_process,
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
        let peak = self.peak_bytes();
        if let Ok(mut guard) = self.tracked.lock() {
            guard.remove(&self.pid);
        }
        peak
    }

    /// The sampled peak so far (bytes).
    pub fn peak_bytes(&self) -> u64 {
        self.tracked_process.peak.load(Ordering::Relaxed)
    }

    /// Sampled CPU time so far, in seconds.
    ///
    /// **Sampled**: accumulated per tick as `cpu_usage()/100 ×
    /// SAMPLE_INTERVAL_SECS`, so sub-interval CPU bursts are missed and
    /// every tick is assumed to span exactly `SAMPLE_INTERVAL_SECS`.
    /// `None` when the sampler never observed the process alive during a
    /// tick (e.g. it exited before the first refresh).
    pub fn cpu_seconds(&self) -> Option<f64> {
        // SeqCst pairs with the seen-store in the sampler: once `seen` is
        // observable, this tick's cpu_micros delta is observable too (a
        // Relaxed read could see the flag from tick N with micros from
        // tick N-1 — a one-tick under-count).
        self.tracked_process
            .seen
            .load(Ordering::SeqCst)
            .then(|| self.tracked_process.cpu_micros.load(Ordering::SeqCst) as f64 / 1_000_000.0)
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
///
/// `children_of` is the parent→children map over the CURRENT process table,
/// built once per tick by the caller and shared by every tracked root: the
/// walk is then O(subtree) instead of a full-table fixpoint per rule.
fn subtree_rss_bytes(
    children_of: &HashMap<u32, Vec<u32>>,
    system: &sysinfo::System,
    root: u32,
) -> u64 {
    // Iterate the descendant closure over an explicit stack (a recursive
    // visit would need the same map anyway).
    let mut total = 0u64;
    let mut stack = vec![root];
    let mut visited = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Some(proc) = system.process(sysinfo::Pid::from_u32(pid)) {
            total += proc.memory();
        }
        if let Some(children) = children_of.get(&pid) {
            stack.extend_from_slice(children);
        }
    }
    total
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
