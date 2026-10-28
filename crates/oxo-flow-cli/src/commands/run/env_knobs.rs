//! Env-knob parsing and small path helpers: spawn-watchdog + disk-pressure tunables, end-of-run prune decision, path absolutize/tmp_dir/normalize (#948 P2-5).

use super::*;

// ── Spawn watchdog (issue #685) ─────────────────────────────────────────────
// A rule can end up marked Running in the scheduler while its task never
// reaches a child spawn (lost wakeup in the wait/dispatch path). The engine
// then wedges: nothing completes, nothing fails, the process idles in a
// futex wait. The watchdog compares the scheduler's running set against the
// executor's live child pids on a fixed tick and reports stragglers. Since
// #847, rules queued in the resource pool are reported as benign waits
// (WARN) — only unqueued rules keep the #685 ERROR. Since #872 the watchdog
// re-arms on every tick once a rule is past the threshold, and after
// `WATCHDOG_REQUEUE_FIRES` consecutive fires on the same rule it requeues
// the rule instead of waiting forever.

/// How often the watchdog compares running rules against live pids.
pub(crate) const SPAWN_WATCHDOG_TICK_SECS: u64 = 30;
/// Default age at which a Running rule with no live child process is
/// reported. Generous: legitimate env setup can take tens of minutes.
pub(crate) const DEFAULT_SPAWN_WATCHDOG_SECS: u64 = 600;
pub(crate) const SPAWN_WATCHDOG_ENV: &str = "OXO_FLOW_SPAWN_WATCHDOG_SECS";
/// Consecutive watchdog fires on one unqueued rule after which the rule is
/// requeued instead of being left to wedge its lane forever (issue #872).
/// The first fire lands at the threshold (600 s default); subsequent fires
/// land on every 30 s tick, so 6 fires ≈ 600 s + 5×30 s ≈ 13 minutes of
/// zero dispatch progress at the defaults.
pub(crate) const WATCHDOG_REQUEUE_FIRES: u32 = 6;
/// Environment override for `WATCHDOG_REQUEUE_FIRES`. Same convention as
/// the other watchdog knobs: unparseable or `0` falls back to the default —
/// a typo can never disable the requeue (it would wedge the lane instead).
pub(crate) const WATCHDOG_REQUEUE_FIRES_ENV: &str = "OXO_FLOW_SPAWN_WATCHDOG_REQUEUE_FIRES";

pub(crate) fn watchdog_requeue_fires() -> u32 {
    parse_watchdog_requeue_fires(std::env::var(WATCHDOG_REQUEUE_FIRES_ENV).ok().as_deref())
}

pub(crate) fn parse_watchdog_requeue_fires(raw: Option<&str>) -> u32 {
    raw.and_then(|v| v.parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(WATCHDOG_REQUEUE_FIRES)
}

/// How often the #843 disk-pressure monitor re-measures free space while the
/// run is parked under Hold (nothing in flight) or between completion batches.
pub(crate) const DISK_PRESSURE_TICK_SECS: u64 = 60;
/// Consecutive parked Hold rounds (nothing in flight, nothing reclaimable)
/// after which the run aborts: the disk is not recovering on its own.
pub(crate) const DISK_HOLD_ABORT_ROUNDS: u32 = 2;
/// Environment overrides for the two constants above (review: shared-HPC
/// scratch may want a longer grace than 2×60s). Same convention as
/// `OXO_FLOW_SPAWN_WATCHDOG_SECS`: unparseable or `0` falls back to the
/// default — a typo can never disable the abort (it would hang instead).
pub(crate) const DISK_TICK_ENV: &str = "OXO_FLOW_DISK_TICK_SECS";
pub(crate) const DISK_HOLD_ROUNDS_ENV: &str = "OXO_FLOW_DISK_HOLD_ROUNDS";

pub(crate) fn disk_tick_secs() -> u64 {
    parse_disk_tick_secs(std::env::var(DISK_TICK_ENV).ok().as_deref())
}

pub(crate) fn parse_disk_tick_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DISK_PRESSURE_TICK_SECS)
}

pub(crate) fn disk_hold_abort_rounds() -> u32 {
    parse_disk_hold_abort_rounds(std::env::var(DISK_HOLD_ROUNDS_ENV).ok().as_deref())
}

pub(crate) fn parse_disk_hold_abort_rounds(raw: Option<&str>) -> u32 {
    raw.and_then(|v| v.parse::<u32>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DISK_HOLD_ABORT_ROUNDS)
}

/// Parse the watchdog threshold from the environment. Extracted as a pure
/// function of the env so tests exercise it without process-global mutation.
pub(crate) fn spawn_watchdog_threshold() -> std::time::Duration {
    parse_spawn_watchdog_threshold(std::env::var(SPAWN_WATCHDOG_ENV).ok().as_deref())
}

/// Pure derivation: `None`/unparseable/`0` fall back to the default — `0`
/// matches the `OXO_ENV_LOCK_TIMEOUT_SECS` convention rather than meaning
/// "disable", so a typo can never silently turn the watchdog off.
pub(crate) fn parse_spawn_watchdog_threshold(raw: Option<&str>) -> std::time::Duration {
    raw.and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(DEFAULT_SPAWN_WATCHDOG_SECS))
}

/// Pure decision for the end-of-run temporary sweep (#893): should the
/// fully-successful run prune temporary outputs?
///
/// - No `[engine]` thresholds configured → sweep unconditionally (the
///   documented historical behavior; disk pressure mid-run is the only
///   reclaim path).
/// - Thresholds configured and free space measured at/above
///   `reclaim_free_disk` → skip the sweep (disk pressure alone triggers
///   re-materialization, not every run).
/// - Thresholds configured but the measurement is unavailable (non-Unix) →
///   keep outputs, matching `DiskPressureMonitor`'s fail-open stance.
pub(crate) fn endofrun_prune_decision(
    free_mb: Option<u64>,
    thresholds: Option<(u64, u64)>,
) -> EndOfRunPruneDecision {
    let Some((_, reclaim_mb)) = thresholds else {
        return EndOfRunPruneDecision::Sweep;
    };
    match free_mb {
        Some(free) if free >= reclaim_mb => EndOfRunPruneDecision::Skip {
            free_mb: free,
            reclaim_mb,
        },
        Some(_) => EndOfRunPruneDecision::Sweep,
        None => EndOfRunPruneDecision::KeepOnUnknown,
    }
}

/// Outcome of [`endofrun_prune_decision`] — see that function for the
/// semantics of each arm.
#[derive(Debug)]
pub(crate) enum EndOfRunPruneDecision {
    /// Run the post-run temporary sweep (historical behavior).
    Sweep,
    /// Free space is at/above the reclaim floor — keep temporary outputs.
    Skip { free_mb: u64, reclaim_mb: u64 },
    /// Free space could not be measured — keep temporary outputs (fail-open).
    KeepOnUnknown,
}
/// Resolve a possibly-relative path against the current directory without
/// resolving symlinks. Checkpoint records use this so `resume` works from
/// any invocation directory (issue #68).
pub(crate) fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Resolve the run's global scratch directory (`[engine] tmp_dir`, issue
/// #907). `OXO_FLOW_TMPDIR` overrides the workflow value — the same
/// env-beats-config pattern as `OXO_FLOW_OUTPUT_TAIL_BYTES` — so operators
/// can redirect temp traffic per invocation without touching the file.
/// A relative workflow value anchors to the workflow directory (the same
/// root as file-backed env specs, issue #427); the configured directory is
/// created on demand so the first rule doesn't race on mkdir. `None`
/// (unset everywhere) keeps the ambient environment untouched.
pub(crate) fn tmp_dir_resolved(
    configured: Option<String>,
    workflow_dir: &Path,
) -> Result<Option<PathBuf>> {
    if let Ok(raw) = std::env::var("OXO_FLOW_TMPDIR") {
        let raw = raw.trim().to_string();
        if !raw.is_empty() {
            let resolved = absolutize(Path::new(&raw))?;
            // Same fail-fast mkdir as the config path below — the docs
            // promise "created on demand at run start" for both routes.
            std::fs::create_dir_all(&resolved)
                .with_context(|| format!("creating OXO_FLOW_TMPDIR dir {}", resolved.display()))?;
            return Ok(Some(resolved));
        }
        tracing::warn!("OXO_FLOW_TMPDIR is empty, ignoring it in favor of [engine] tmp_dir");
    }
    let Some(raw) = configured else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        tracing::warn!("[engine] tmp_dir is empty, ignoring it");
        return Ok(None);
    }
    let raw_path = Path::new(trimmed);
    // Anchor on the RAW value — absolutizing first would join the process
    // cwd and silently turn every relative config into a cwd-relative path.
    let anchored = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        workflow_dir.join(raw_path)
    };
    std::fs::create_dir_all(&anchored)
        .with_context(|| format!("creating [engine] tmp_dir {}", anchored.display()))?;
    Ok(Some(anchored))
}

/// Lexically collapse `.`/`..` segments from an absolutized path
/// (`/wf/./data` → `/wf/data`) without touching the filesystem. The
/// workflow-dir join in `parent_dir` leaves a `.` segment for bare
/// filenames; anchored {input}/{log} strings inherit it (issue #427).
pub(crate) fn normalize_dots(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::with_capacity(path.as_os_str().len());
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // Only reachable for the leading segments of a relative
                // input — absolutize already stripped interior `..` against
                // a real base. Keep it verbatim rather than guessing.
                out.push(comp);
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Long-form names of `oxo-flow run` flags (plus clap's global --help/
/// --version). Keep in sync with the `Run` variant in main.rs.
#[cfg(test)]
mod tmp_dir_tests {
    //! `[engine] tmp_dir` resolution (issue #907): env override wins,
    //! relative values anchor to the workflow dir, unset stays unset.

    use super::tmp_dir_resolved;

    #[test]
    fn unset_everywhere_is_none() {
        // OXO_FLOW_TMPDIR is unset in CI and normal dev shells; if a developer
        // exports it, skip rather than fail — the test asserts the CONFIGURED
        // path, not the operator's machine.
        if std::env::var("OXO_FLOW_TMPDIR").is_ok() {
            return;
        }
        let resolved = tmp_dir_resolved(None, std::path::Path::new("/wf")).unwrap();
        assert!(resolved.is_none(), "no config + no env = None");
    }

    #[test]
    fn relative_config_anchors_to_workflow_dir() {
        let dir = std::env::temp_dir().join(format!("oxo-tmpdircfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let workflow_dir = dir.join("wf");
        std::fs::create_dir_all(&workflow_dir).unwrap();

        let resolved = tmp_dir_resolved(Some("scratch".to_string()), &workflow_dir).unwrap();
        assert_eq!(
            resolved.as_deref(),
            Some(workflow_dir.join("scratch").as_path()),
            "relative tmp_dir must anchor to the workflow directory"
        );
        // Created on demand so the first rule doesn't race on mkdir.
        assert!(workflow_dir.join("scratch").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_config_value_is_ignored() {
        if std::env::var("OXO_FLOW_TMPDIR").is_ok() {
            return;
        }
        let resolved =
            tmp_dir_resolved(Some("   ".to_string()), std::path::Path::new("/wf")).unwrap();
        assert!(resolved.is_none(), "empty/whitespace tmp_dir = None");
    }
}
