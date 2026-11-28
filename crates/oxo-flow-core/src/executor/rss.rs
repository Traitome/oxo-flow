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
//! - **CPU time**: `Process::accumulated_cpu_time()` deltas summed over
//!   the subtree (issue #1445 L1). The old root-only
//!   `cpu_usage()/100 × SAMPLE_INTERVAL_SECS` accumulation reported ≈0 for
//!   the common shell-rule shape, where the bioinformatics tool is a child
//!   of the tracked shell, while the field claims the rule's CPU. Deltas
//!   are kept per pid (integer microseconds, float-free atomics); a process
//!   that exits between ticks loses only its last sub-interval, exactly like
//!   RSS/I-O sampling.
//! - **I/O bytes** (Linux only, issue #1371): `/proc/<pid>/io`'s
//!   `read_bytes`/`write_bytes` deltas summed over the subtree. A process
//!   that exits stops being visible, so its last cumulative counters are
//!   kept per pid and only the deltas between ticks are added — the total
//!   never shrinks when a child disappears. `None` off Linux.
//!
//! All metrics are **sampled**, not exact `getrusage` values: sub-interval
//! spikes can be missed, and the CPU total only includes deltas observed
//! while the sampled processes were alive. That is sufficient for
//! bottleneck detection (sustained pressure), and is documented in the
//! diagnostics API.
//!
//! All three metrics share one subtree walk, so a process that LEAVES the
//! tree stops being attributed: a child orphaned when the root exits, or one
//! that daemonizes (double fork + parent exit), is re-parented away from the
//! tracked root and its later CPU/RSS/I-O are not counted. Only processes
//! reachable from the tracked root at sample time are measured.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sysinfo::ProcessesToUpdate;
use tokio::sync::Notify;

/// Sampling cadence in milliseconds. Public so the resource ledger's source
/// metadata can name the method without duplicating the constant.
pub const SAMPLE_INTERVAL_MILLIS: u64 = 200;

/// Sample interval for peak detection.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(SAMPLE_INTERVAL_MILLIS);

/// Per-pid accumulators for one tracked child. Integer counts keep the
/// shared atomics float-free; `cpu_seconds()` converts on read.
struct TrackedProcess {
    /// Sampled peak subtree RSS in bytes.
    peak: AtomicU64,
    /// Sampled CPU time in microseconds.
    cpu_micros: AtomicU64,
    /// Bytes read from disk across the subtree (Linux only).
    io_read: AtomicU64,
    /// Bytes written to disk across the subtree (Linux only).
    io_write: AtomicU64,
    /// Set once the sampler has seen a process of the subtree alive during a
    /// tick.
    seen: AtomicBool,
    /// Set once at least one `/proc/<pid>/io` read succeeded.
    io_seen: AtomicBool,
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
                        // One `System` is reused across ticks so the process
                        // table (and each entry's cumulative CPU counters)
                        // is refreshed in place; dead processes are dropped
                        // each refresh so the table stays bounded over long
                        // runs. sysinfo 0.40 made construction fallible; a
                        // failure here disables peak-RSS metering for the
                        // whole run (degrade, not panic — same stance as the
                        // thread spawn failure right below).
                        let Ok(mut system) = sysinfo::System::new() else {
                            tracing::warn!("sysinfo init failed — peak-RSS metering disabled");
                            return;
                        };
                        // Per tracked root → per descendant pid → the last
                        // cumulative CPU time and the process start time that
                        // identifies which process the accumulator belongs to
                        // (issue #1445 L1). Deltas are folded into the root's
                        // total; keeping the last value lets a disappearing
                        // child's final observed contribution stand.
                        let mut cpu_last: HashMap<u32, HashMap<u32, (u64, u64)>> = HashMap::new();
                        // Per tracked root → per descendant pid → the last
                        // cumulative `(read_bytes, write_bytes)` read from
                        // /proc. Deltas are folded into the root's totals;
                        // keeping the last value lets a disappearing child
                        // keep its final contribution (its counters vanish
                        // with it).
                        #[cfg(target_os = "linux")]
                        let mut io_last: HashMap<
                            u32,
                            HashMap<u32, (u64, u64)>,
                        > = HashMap::new();
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
                                    #[cfg(target_os = "linux")]
                                    io_last.clear();
                                    cpu_last.clear();
                                    continue;
                                }
                                guard.iter().map(|(pid, tp)| (*pid, tp.clone())).collect()
                            };
                            system.refresh_processes(ProcessesToUpdate::All, true);
                            // One shared parent→children map per tick (built
                            // once, O(P)), plus one shared subtree walk per
                            // root. The old shape re-scanned the whole
                            // process table per tracked child in a fixpoint
                            // loop — O(ticks × rules × passes × P)
                            // (#268 item 4).
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
                                // CPU, RSS, and (on Linux) I/O all describe
                                // the same subtree: walk it once per root and
                                // share it (issue #1445 L1). Thread entries
                                // are removed from the shared list — sysinfo's
                                // Linux backend lists each thread as its own
                                // process entry parented by the process, and a
                                // thread shares the process's address space
                                // and I/O accounting, so counting leader +
                                // threads would inflate RSS (~N×) and I/O
                                // (~2×). The leader's entries already carry
                                // the group totals (issue #1445 L1 review).
                                let live = live_subtree_pids(&children_of, *pid, |child| {
                                    system
                                        .process(sysinfo::Pid::from_u32(child))
                                        .is_some_and(|proc| proc.thread_kind().is_some())
                                });
                                // CPU: sum each process's cumulative CPU-time
                                // delta over the subtree. A process that exits
                                // stops being visible, so only deltas observed
                                // while it was alive are counted — the same
                                // sampling contract as RSS/I-O. The per-pid
                                // previous value is kept with the pid's start
                                // time, so a reused pid is treated as a new
                                // process (its whole lifetime is added) rather
                                // than producing a bogus negative delta.
                                let mut cpu_micros = 0u64;
                                let mut observed = false;
                                {
                                    let per_root = cpu_last.entry(*pid).or_default();
                                    for child in &live {
                                        let Some(proc) =
                                            system.process(sysinfo::Pid::from_u32(*child))
                                        else {
                                            continue;
                                        };
                                        // A dead-but-retained entry must not
                                        // inflate the total.
                                        if !proc.exists() {
                                            continue;
                                        }
                                        observed = true;
                                        let start = proc.start_time();
                                        let cpu_ms = proc.accumulated_cpu_time();
                                        let delta = match per_root.get(child) {
                                            Some(&(last_start, last_cpu))
                                                if last_start == start =>
                                            {
                                                cpu_ms.saturating_sub(last_cpu)
                                            }
                                            // First sighting (or a reused
                                            // pid): its whole lifetime.
                                            _ => cpu_ms,
                                        };
                                        cpu_micros =
                                            cpu_micros.saturating_add(delta.saturating_mul(1_000));
                                        per_root.insert(*child, (start, cpu_ms));
                                    }
                                    let alive: std::collections::HashSet<u32> =
                                        live.iter().copied().collect();
                                    per_root.retain(|child, _| alive.contains(child));
                                }
                                if observed {
                                    // The delta lands *before* the SeqCst
                                    // seen-store publishes it, so a reader
                                    // that observes seen=true in
                                    // `cpu_seconds()` must also observe this
                                    // tick's cpu_micros update (no one-tick
                                    // under-count).
                                    if cpu_micros > 0 {
                                        tp.cpu_micros.fetch_add(cpu_micros, Ordering::SeqCst);
                                    }
                                    tp.seen.store(true, Ordering::SeqCst);
                                }
                                // Peak RSS: the summed `statm` of the
                                // (thread-filtered) subtree in bytes.
                                let subtree_rss: u64 = live
                                    .iter()
                                    .filter_map(|child| {
                                        system.process(sysinfo::Pid::from_u32(*child))
                                    })
                                    .map(sysinfo::Process::memory)
                                    .sum();
                                tp.peak.fetch_max(subtree_rss, Ordering::Relaxed);
                                #[cfg(target_os = "linux")]
                                {
                                    let per_root = io_last.entry(*pid).or_default();
                                    let mut sampled = false;
                                    for child in &live {
                                        if let Some((read, write)) = read_proc_io(*child) {
                                            let (last_read, last_write) =
                                                per_root.get(child).copied().unwrap_or((0, 0));
                                            // A counter that moved backwards is
                                            // a reused pid, not negative I/O.
                                            tp.io_read.fetch_add(
                                                read.saturating_sub(last_read),
                                                Ordering::Relaxed,
                                            );
                                            tp.io_write.fetch_add(
                                                write.saturating_sub(last_write),
                                                Ordering::Relaxed,
                                            );
                                            per_root.insert(*child, (read, write));
                                            sampled = true;
                                        }
                                    }
                                    let alive: std::collections::HashSet<u32> =
                                        live.into_iter().collect();
                                    per_root.retain(|child, _| alive.contains(child));
                                    if sampled {
                                        tp.io_seen.store(true, Ordering::Relaxed);
                                    }
                                }
                            }
                            let live_roots: std::collections::HashSet<u32> =
                                snapshot.iter().map(|(pid, _)| *pid).collect();
                            cpu_last.retain(|root, _| live_roots.contains(root));
                            #[cfg(target_os = "linux")]
                            io_last.retain(|root, _| live_roots.contains(root));
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
            io_read: AtomicU64::new(0),
            io_write: AtomicU64::new(0),
            seen: AtomicBool::new(false),
            io_seen: AtomicBool::new(false),
        });
        // Same poison recovery the tick path documents (issue #194 §3.5):
        // `track` runs on the rule launch path — converting poisoning into
        // a panic there turned a degraded-but-running run into a crash (M-3).
        self.tracked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

    /// Sampled CPU time of the rule's process subtree so far, in seconds.
    ///
    /// **Sampled**: each tick folds the cumulative CPU-time deltas of every
    /// live process in the subtree rooted at the tracked child (issue
    /// #1445 L1), so a rule whose real work happens in a shell's child — the
    /// common bioinformatics shape — is attributed its CPU, not just the
    /// shell's. Sub-interval CPU bursts and the final interval before a
    /// process exits are missed, like the RSS/I-O samples; a process that
    /// leaves the subtree (orphaned, or daemonized via a double fork) stops
    /// being counted. `None` when the sampler never observed any process of
    /// the subtree alive during a tick (e.g. it exited before the first
    /// refresh).
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

    /// Sampled disk I/O of the process subtree as `(read_bytes,
    /// write_bytes)`, summed from `/proc/<pid>/io` deltas.
    ///
    /// **Linux only.** `None` when no `/proc/<pid>/io` read ever succeeded
    /// (including every non-Linux platform) — unknown, never zero. The
    /// counters are deltas between ticks, so a child that exits keeps the
    /// bytes it already reported.
    pub fn io_bytes(&self) -> Option<(u64, u64)> {
        self.tracked_process
            .io_seen
            .load(Ordering::Relaxed)
            .then(|| {
                (
                    self.tracked_process.io_read.load(Ordering::Relaxed),
                    self.tracked_process.io_write.load(Ordering::Relaxed),
                )
            })
    }
}

impl Drop for RssHandle {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.tracked.lock() {
            guard.remove(&self.pid);
        }
    }
}

/// All pids of `root`'s process subtree, thread entries included.
///
/// `children_of` is the parent→children map over the CURRENT process table,
/// built once per tick by the caller and shared by every tracked root: the
/// walk is then O(subtree) instead of a full-table fixpoint per rule. The
/// sampler filters thread entries with [`live_subtree_pids`] before measuring.
fn subtree_pids(children_of: &HashMap<u32, Vec<u32>>, root: u32) -> Vec<u32> {
    let mut pids = Vec::new();
    let mut stack = vec![root];
    let mut visited = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        pids.push(pid);
        if let Some(children) = children_of.get(&pid) {
            stack.extend_from_slice(children);
        }
    }
    pids
}

/// Cumulative `(read_bytes, write_bytes)` from `/proc/<pid>/io`.
///
/// `read_bytes`/`write_bytes` count bytes that actually reached storage
/// (page-cache hits are excluded), which is the closest Linux analogue of
/// the scheduler's disk-accounting fields. Unreadable processes (permission,
/// race with exit) yield `None` — one missing tick, never a fabricated zero.
#[cfg(target_os = "linux")]
fn read_proc_io(pid: u32) -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let mut read = None;
    let mut write = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("read_bytes:") {
            read = value.trim().parse::<u64>().ok();
        } else if let Some(value) = line.strip_prefix("write_bytes:") {
            write = value.trim().parse::<u64>().ok();
        }
    }
    Some((read?, write?))
}

/// Subtree pids of `root` with sysinfo's user-space THREAD entries removed.
///
/// `is_thread` reports sysinfo's thread classification (meaningful only on
/// Linux, where `with_tasks()` lists every thread as its own process entry
/// parented by the process). A thread shares the process's address space and
/// I/O accounting, so counting the leader AND its threads inflates peak RSS
/// (~N× for N threads) and disk I/O (~2×); the leader's entries already
/// carry the group totals. Traversal still walks THROUGH a thread entry:
/// a process forked by a non-leader thread is parented to that thread and is
/// a real process that must be measured (issue #1445 L1 review).
fn live_subtree_pids(
    children_of: &HashMap<u32, Vec<u32>>,
    root: u32,
    is_thread: impl Fn(u32) -> bool,
) -> Vec<u32> {
    subtree_pids(children_of, root)
        .into_iter()
        .filter(|pid| !is_thread(*pid))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared subtree walk must exclude thread entries while still
    /// reaching processes forked BY a non-leader thread (their parent is the
    /// thread, not the process leader).
    #[test]
    fn live_subtree_excludes_threads_but_keeps_their_children() {
        let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
        children_of.insert(10, vec![11, 12]);
        children_of.insert(12, vec![13]); // 13 forked by thread 12
        let mut live = live_subtree_pids(&children_of, 10, |pid| pid == 12);
        live.sort_unstable();
        assert_eq!(
            live,
            vec![10, 11, 13],
            "the thread entry is not measured, but its child process is"
        );
    }

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

    /// CPU must be attributed to the whole rule subtree, not just the
    /// tracked root (issue #1445 L1). The tracked process here is a shell
    /// that WAITS (≈0 CPU) while its child burns CPU — the root-only
    /// implementation reported ~0 s. Bounded to roughly one second of
    /// busy-loop wall time.
    #[cfg(unix)]
    #[test]
    fn sampler_accumulates_subtree_cpu_beyond_the_root() {
        /// A shell one-liner whose child burns CPU. The trailing `:`
        /// keeps the shell from `exec`-replacing itself with its only
        /// command, so the burner really is a CHILD of the tracked root.
        /// `None` when neither interpreter is available (the test skips).
        fn burner() -> Option<&'static str> {
            if std::process::Command::new("python3")
                .arg("--version")
                .output()
                .is_ok()
            {
                return Some(concat!(
                    "python3 -c 'import time\n",
                    "end = time.monotonic() + 0.8\n",
                    "while time.monotonic() < end: pass'; :"
                ));
            }
            if std::process::Command::new("perl")
                .arg("-e")
                .arg("1")
                .output()
                .is_ok()
            {
                return Some(concat!(
                    "perl -MTime::HiRes=time -e ",
                    "'$end = time + 0.8; 1 while time < $end;'; :"
                ));
            }
            None
        }
        let Some(script) = burner() else {
            eprintln!("skipped: neither python3 nor perl available");
            return;
        };
        // The tracked root is the shell; the CPU burner is its CHILD.
        let mut child = match std::process::Command::new("sh")
            .args(["-c", script])
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                eprintln!("skipped: no POSIX shell");
                return;
            }
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let sampler = RssSampler::new();
            let handle = sampler.track(child.id());
            assert!(child.wait().unwrap().success());
            // Allow the last sample tick to land before finishing.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let cpu = handle
                .cpu_seconds()
                .expect("the sampler observed the subtree alive");
            // The child burned ~0.8 CPU-seconds; the waiting shell itself
            // contributes ≈0. A root-only measurement stays far below this
            // bound even with generous sampling slack.
            assert!(
                cpu >= 0.3,
                "expected the child's CPU to be attributed to the rule subtree, got {cpu:.3} s"
            );
        });
    }

    /// Issue #1445 L1 review (MEDIUM): a multi-threaded child must be
    /// counted ONCE in the peak RSS. sysinfo's Linux backend lists every
    /// thread as its own process entry parented by the process, and a
    /// thread's `statm` reports the shared address space — summing leader +
    /// threads inflated the peak ~N× for N threads. The child here holds
    /// ~96 MiB across 8 threads: the process is ~100-150 MiB, while the
    /// per-thread sum is ~9× that.
    #[cfg(target_os = "linux")]
    #[test]
    fn sampler_counts_a_multithreaded_child_rss_once() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipped: python3 not available");
            return;
        }
        // The trailing `:` keeps the shell from exec-replacing itself, so
        // the tracked root is the shell and the threaded process is its
        // child (the shape real rules have).
        let script = concat!(
            "python3 -c 'import threading, time\n",
            "payload = bytearray(96 * 1024 * 1024)\n",
            "def hold():\n",
            "    time.sleep(1.5)\n",
            "threads = [threading.Thread(target=hold) for _ in range(8)]\n",
            "for t in threads: t.start()\n",
            "for t in threads: t.join()'; :"
        );
        let mut child = match std::process::Command::new("sh")
            .args(["-c", script])
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                eprintln!("skipped: no POSIX shell");
                return;
            }
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let sampler = RssSampler::new();
            let handle = sampler.track(child.id());
            assert!(child.wait().unwrap().success());
            tokio::time::sleep(Duration::from_millis(300)).await;
            let peak_mb = handle.finish() / (1024 * 1024);
            // One process (~100 MiB + interpreter baseline). A per-thread
            // sum lands around 9× that; 400 MiB separates the two cleanly.
            assert!(
                (40..=400).contains(&peak_mb),
                "expected the threaded child's RSS to be counted once, got {peak_mb} MiB"
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

    /// Linux has `/proc/<pid>/io`; the sampler must attribute the child's
    /// disk writes to the subtree (issue #1371).
    #[cfg(target_os = "linux")]
    #[test]
    fn sampler_reports_subtree_io_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("payload.bin");
        let mut child = match std::process::Command::new("python3")
            .args([
                "-c",
                // fsync forces the bytes to the storage layer, which is what
                // `/proc/<pid>/io`'s write_bytes counts.
                "import os,sys,time; f=open(sys.argv[1],'wb'); f.write(b'x'*(8*1024*1024)); \
                 f.flush(); os.fsync(f.fileno()); time.sleep(0.6)",
            ])
            .arg(&out)
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                eprintln!("skipped: python3 not available");
                return;
            }
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let sampler = RssSampler::new();
            let handle = sampler.track(child.id());
            assert!(child.wait().unwrap().success());
            tokio::time::sleep(Duration::from_millis(300)).await;
            let (_read, write) = handle
                .io_bytes()
                .expect("linux /proc/<pid>/io must be sampled");
            // Page-cache writes may undercount `write_bytes`, so assert the
            // direction rather than an exact size.
            assert!(write > 0, "expected disk writes to be attributed: {write}");
        });
    }

    /// Off Linux no `/proc/<pid>/io` exists: I/O is unknown (None), never a
    /// fabricated zero.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn sampler_reports_unknown_io_off_linux() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let sampler = RssSampler::new();
            let handle = sampler.track(child.id());
            assert!(child.wait().unwrap().success());
            tokio::time::sleep(Duration::from_millis(SAMPLE_INTERVAL_MILLIS * 2)).await;
            assert_eq!(handle.io_bytes(), None);
        });
    }
}
