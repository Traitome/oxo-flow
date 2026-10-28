//! Reference/prune bookkeeping: rule_resource_rows, RefRecord, source_content_sig, free_kilobytes, age_ready_list, suggest_jobs (#948 P2-5).

use super::*;

/// Per-rule resource rows for the `--json` summary (issue #163): the same
/// data the report's Benchmarks table shows — wall time, sampled peak RSS,
/// sampled CPU seconds, retries — keyed by rule, deterministic order.
/// Empty for aborted paths where no checkpoint summary is meaningful.
pub(crate) fn rule_resource_rows(
    checkpoint: &oxo_flow_core::executor::checkpoint::CheckpointState,
) -> Vec<serde_json::Value> {
    let mut names: Vec<&String> = checkpoint.benchmarks.keys().collect();
    names.sort_unstable();
    names
        .into_iter()
        .map(|name| {
            let b = &checkpoint.benchmarks[name];
            // Issue #690: a stale failed_rules entry with a recorded
            // when-false verdict was gated off, not failed.
            let status = if checkpoint.completed_rules.contains(name) {
                "completed"
            } else if checkpoint.failed_rules.contains(name) {
                if checkpoint.when_verdicts.get(name) == Some(&false) {
                    "skipped_by_when"
                } else {
                    "failed"
                }
            } else {
                "running"
            };
            serde_json::json!({
                "rule": name,
                "status": status,
                "wall_time_secs": b.wall_time_secs,
                // Synthetic up-to-date records (issue #324 F-1) carry a 0.0
                // placeholder wall time — JSON consumers can distinguish
                // them from real measurements via this marker.
                "recorded_as": b.recorded_as,
                "peak_rss_mb": b.max_memory_mb,
                "cpu_seconds": b.cpu_seconds,
                "retries": b.retries,
            })
        })
        .collect()
}

/// Stored reference state: the decision fingerprint plus the source's
/// content signature (size + full content hash for small files) at the
/// time it was recorded. The signature enables the path-migration
/// exemption — identical content at a new path skips the rebuild.
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub(crate) struct RefRecord {
    pub(crate) fingerprint: String,
    pub(crate) content_sig: Option<String>,
    /// The source path (config-expanded, workdir-relative) this record was
    /// computed against — the exemption re-fingerprints with it to prove a
    /// fingerprint change is a PURE path migration.
    pub(crate) source_path: Option<String>,
}

/// Content signature of a reference SOURCE: `size:<bytes>` plus
/// `|hash:<sha256>` for files under the manifest hash cap. Directories
/// and large files return `None` — the path-migration exemption requires
/// the strong hash component, so anything weaker never skips a rebuild.
pub(crate) fn source_content_sig(path: &std::path::Path) -> Option<String> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let mut sig = format!("size:{}", md.len());
    if let Some(hash) = oxo_flow_core::executor::checkpoint::content_hash_if_small(path, &md) {
        sig.push_str(&format!("|hash:{hash}"));
    }
    Some(sig)
}

/// Free disk space in KiB on the filesystem holding `path`, read from
/// `df -Pk` (the portable POSIX form; parses the Available column of the
/// mount line). `None` when df is unavailable or the output is unparsable —
/// the pre-flight degrades to no check rather than blocking the run.
pub(crate) fn free_kilobytes(path: &std::path::Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .args(["-Pk", path.to_str()?])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The last line is the mount that actually holds the path.
    let line = stdout.lines().next_back()?;
    line.split_whitespace().nth(3)?.parse().ok()
}

/// Sorts the scheduler's ready list by effective priority: declared priority
/// plus rounds already waited (the aging counter of issue #123), ties broken
/// by name. Pure and unit-tested — the dispatch loop feeds it the live ready
/// list every round, so passed-over rules gain priority until submitted.
pub(crate) fn age_ready_list(
    mut ready: Vec<String>,
    order_set: &std::collections::HashSet<String>,
    submitted: &std::collections::HashSet<String>,
    priority_map: &std::collections::HashMap<String, i32>,
    waited: &std::collections::HashMap<String, i32>,
) -> Vec<String> {
    ready.retain(|name| order_set.contains(name) && !submitted.contains(name));
    ready.sort_by(|a, b| {
        let pa = priority_map.get(a).copied().unwrap_or(0) + waited.get(a).copied().unwrap_or(0);
        let pb = priority_map.get(b).copied().unwrap_or(0) + waited.get(b).copied().unwrap_or(0);
        pb.cmp(&pa).then_with(|| a.cmp(b))
    });
    ready
}

/// Suggest a `-j` (max concurrent jobs) value for the workflow on this
/// machine, evaluated **per parallel group** (issue #361).
///
/// For every wave: the wider of (wave width) and (cores ÷ heaviest rule in
/// that wave) is the number of jobs that wave can sustain. The suggestion
/// is the maximum across waves, so a light wide wave is never starved by
/// an unrelated heavy wave elsewhere in the DAG — the old global
/// `cores / max_threads_per_rule` formula measured 1.7–3.7× makespan loss
/// on mixed DAGs. The engine's resource pool already enforces declared
/// threads, so a generous suggestion is safe: this is a performance hint,
/// not an oversubscription gate.
pub(crate) fn suggest_jobs(
    config: &WorkflowConfig,
    groups: &[Vec<String>],
    system_threads: u32,
) -> u32 {
    let system_threads = system_threads.max(1);
    let wave_capacity = |names: &[String]| -> u32 {
        let max_threads = names
            .iter()
            .filter_map(|n| config.get_rule(n))
            .map(|r| r.effective_threads())
            .max()
            .unwrap_or(1)
            .max(1);
        (system_threads / max_threads)
            .min(names.len() as u32)
            .max(1)
    };
    groups
        .iter()
        .map(|g| wave_capacity(g))
        .max()
        .unwrap_or(1)
        .clamp(1, system_threads)
}
