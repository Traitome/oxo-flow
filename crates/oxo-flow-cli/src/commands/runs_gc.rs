//! `oxo-flow runs gc` — reclaim stale per-run workspace trees (issue #1228).
//!
//! PR #1218 gave every web create-run its own immutable tree at
//! `workspace/users/<user_id>/runs/<run_id>/` (workflow snapshot, staged
//! inputs/includes, checkpoint, logs, outputs). Nothing reclaimed those trees
//! once a run finished, so finished/failed/cancelled runs leaked disk forever.
//! This module mirrors the existing `oxo-flow env gc` contract (age gate,
//! `--dry-run`, partial-failure = error) and adds run-tree-specific safety
//! guards.
//!
//! Guards, each independently enforced and tested:
//!
//! 1. A live run's tree is never touched (status not terminal).
//! 2. A tree that is the lineage target of a later retry/resume is never
//!    touched: retry/resume copy the predecessor's `workdir`, so a `workdir`
//!    string shared by two or more run rows marks a still-referenced tree.
//! 3. A tree carrying a declared protected output (a rule `protected_output`
//!    or an `[engine] protected_data_trees` entry) is never touched.
//! 4. Deletion only ever happens under the workspace `runs/<run_id>/` root,
//!    with traversal-validated ids (reusing `oxo_flow_web::workspace`).

use anyhow::{Context, Result};
use chrono::DateTime;
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::RunsAction;
use crate::commands::infra::format_bytes;
use crate::commands::print_banner;

/// A tree selected for reclamation, with its measured size.
#[derive(Debug, Clone, Serialize)]
pub struct ReclaimedRun {
    pub run_id: String,
    pub user_id: String,
    pub path: String,
    pub bytes: u64,
}

/// A tree left alone, with the guard that spared it.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedRun {
    pub run_id: String,
    pub reason: String,
}

/// Machine-readable `runs gc --json` report.
#[derive(Debug, Clone, Serialize)]
pub struct RunsGcReport {
    pub dry_run: bool,
    pub older_than_days: u64,
    pub user: Option<String>,
    pub scanned: usize,
    pub reclaimed_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    pub bytes_freed: u64,
    pub reclaimed: Vec<ReclaimedRun>,
    pub skipped: Vec<SkippedRun>,
}

/// Parse an RFC3339 timestamp into seconds since the Unix epoch (`None` on a
/// malformed value — an unparseable age is treated as "too new" so it can
/// never be deleted).
fn rfc3339_secs(value: &str) -> Option<u64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|dt| u64::try_from(dt.timestamp()).ok())
}

/// The age cutoff (seconds since the epoch) below which a run is stale.
///
/// Pure so the `--older-than` boundary is unit-testable without a clock.
#[must_use]
pub fn age_cutoff_secs(now_secs: u64, min_age_days: u64) -> u64 {
    now_secs.saturating_sub(min_age_days.saturating_mul(86_400))
}

/// A run's age reference in seconds — its `finished_at`, falling back to
/// `created_at` (a terminal run always has one of them).
fn run_age_secs(row: &oxo_flow_web::infra::db::models::RunRow) -> Option<u64> {
    row.finished_at
        .as_deref()
        .and_then(rfc3339_secs)
        .or_else(|| rfc3339_secs(&row.created_at))
}

/// Recursive size + every descendant path of a tree, best-effort (walk errors
/// contribute 0 bytes / no entry). The relative-and-absolute candidate list
/// feeds the protected-output guard.
fn tree_size_and_entries(root: &Path) -> (u64, Vec<PathBuf>) {
    let mut total = 0u64;
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let Ok(entry) = entry else { continue };
        if entry.path() == root {
            continue;
        }
        entries.push(entry.path().to_path_buf());
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            total += meta.len();
        }
    }
    (total, entries)
}

/// Collect the protected-output patterns a run declares: every rule's
/// `protected_output` (config-expanded) plus `[engine] protected_data_trees`.
///
/// Returns `None` when the run's snapshot does not parse — the caller treats
/// an unparseable snapshot as protected (fail-closed).
fn protected_patterns_for_snapshot(snapshot: &str) -> Option<Vec<String>> {
    let config = WorkflowConfig::parse(snapshot).ok()?;
    let values = crate::commands::run::config_placeholder_values(&config.config);
    let mut patterns = Vec::new();
    for rule in &config.rules {
        for output in &rule.protected_output {
            patterns.push(oxo_flow_core::executor::checkpoint::expand_config_in_path(
                output, &values,
            ));
        }
    }
    if let Some(engine) = &config.engine {
        for tree in engine.protected_data_trees() {
            patterns.push(tree.clone());
        }
    }
    Some(patterns)
}

/// Whether a tree carrying `entries` (paths under `run_dir`) is protected by
/// any pattern. Uses the shared core matcher (the same one failure
/// invalidation and the reclaim gate use) against both the tree-relative and
/// the absolute candidate form — fail-closed on an unparseable pattern.
fn tree_is_protected(run_dir: &Path, entries: &[PathBuf], patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    entries.iter().any(|path| {
        let relative = path.strip_prefix(run_dir).unwrap_or(path).to_string_lossy();
        let absolute = path.to_string_lossy();
        patterns.iter().any(|pattern| {
            oxo_flow_core::protected::protected_pattern_matches(pattern, &relative)
                || oxo_flow_core::protected::protected_pattern_matches(pattern, &absolute)
        })
    })
}

/// Why a run is spared, or `None` when it is eligible for the sweep.
///
/// `workdir_refcount` maps each non-empty `workdir` string to the number of
/// run rows that carry it (lineage detection); `now_secs` is the wall clock.
fn skip_reason(
    row: &oxo_flow_web::infra::db::models::RunRow,
    workdir_refcount: &HashMap<String, usize>,
    now_secs: u64,
    min_age_days: u64,
) -> Option<String> {
    if !oxo_flow_web::infra::db::models::is_terminal_run_status(&row.status) {
        return Some(format!("live run (status: {})", row.status));
    }
    if let Some(workdir) = row.workdir.as_deref()
        && workdir_refcount.get(workdir).copied().unwrap_or(0) > 1
    {
        return Some("lineage target (shared workdir with a later run)".to_string());
    }
    let Some(age) = run_age_secs(row) else {
        return Some("age unknown (unparseable timestamp)".to_string());
    };
    if age > age_cutoff_secs(now_secs, min_age_days) {
        return Some(format!("too new (age gate: {min_age_days}d)"));
    }
    None
}

/// Count how many run rows share each non-empty `workdir` string.
fn workdir_refcounts(rows: &[oxo_flow_web::infra::db::models::RunRow]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for row in rows {
        if let Some(workdir) = row.workdir.as_deref()
            && !workdir.is_empty()
        {
            *counts.entry(workdir.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// Fetch every run row (optionally restricted to one user) through the
/// cursor-paged storage contract.
async fn fetch_rows(
    backend: &dyn oxo_flow_web::infra::db::StorageBackend,
    user_id: Option<String>,
) -> Result<Vec<oxo_flow_web::infra::db::models::RunRow>> {
    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let filters = oxo_flow_web::infra::db::models::RunFilters {
            user_id: user_id.clone(),
            cursor: cursor.clone(),
            limit: 500,
            ..Default::default()
        };
        let page = backend
            .list_runs_cursor(&filters)
            .await
            .map_err(|e| anyhow::anyhow!("failed to list runs: {e}"))?;
        let next = page.next_cursor.clone();
        rows.extend(page.items);
        match next {
            Some(c) if !c.is_empty() && Some(&c) != cursor.as_ref() => cursor = Some(c),
            _ => break,
        }
    }
    Ok(rows)
}

/// `runs gc` — reclaim stale per-run workspace trees.
///
/// # Errors
/// Fails when the storage layer cannot be initialized or listed, or when a
/// selected tree cannot be deleted (partial failure is an error, exactly like
/// `env gc`, so exit-code-gating scripts see it).
pub(crate) async fn runs_gc(
    min_age_days: u64,
    user: Option<String>,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    oxo_flow_web::init_database()
        .await
        .context("failed to initialize run storage")?;
    let backend = oxo_flow_web::infra::db::global_backend()
        .ok_or_else(|| anyhow::anyhow!("no storage backend initialized — nothing to sweep"))?;

    let user_id = match &user {
        Some(name) => match backend
            .get_user_by_username(name)
            .await
            .map_err(|e| anyhow::anyhow!("failed to look up user {name:?}: {e}"))?
        {
            Some(u) => Some(u.id),
            None => anyhow::bail!("no such user: {name}"),
        },
        None => None,
    };

    let rows = fetch_rows(backend.as_ref(), user_id).await?;
    let refcounts = workdir_refcounts(&rows);
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if !json {
        print_banner();
        eprintln!(
            "{} {} run(s) scanned{}",
            "Run trees:".bold(),
            rows.len(),
            match &user {
                Some(u) => format!(" (user: {u})"),
                None => String::new(),
            }
        );
    }

    let mut report = RunsGcReport {
        dry_run,
        older_than_days: min_age_days,
        user: user.clone(),
        scanned: rows.len(),
        reclaimed_count: 0,
        skipped_count: 0,
        failed_count: 0,
        bytes_freed: 0,
        reclaimed: Vec::new(),
        skipped: Vec::new(),
    };
    let mut failed = 0usize;

    for row in &rows {
        let Some(reason) = skip_reason(row, &refcounts, now_secs, min_age_days) else {
            // Eligible: resolve the traversal-validated tree dir (guard 4).
            let run_dir = match oxo_flow_web::workspace::get_run_directory(&row.user_id, &row.id) {
                Ok(dir) => dir,
                Err(e) => {
                    report.skipped.push(SkippedRun {
                        run_id: row.id.clone(),
                        reason: format!("invalid run id/user id: {e}"),
                    });
                    continue;
                }
            };
            if run_dir
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n == "runs")
                != Some(true)
            {
                report.skipped.push(SkippedRun {
                    run_id: row.id.clone(),
                    reason: "refusing to delete outside the runs/<run_id>/ root".to_string(),
                });
                continue;
            }
            if !run_dir.is_dir() {
                report.skipped.push(SkippedRun {
                    run_id: row.id.clone(),
                    reason: "tree already absent".to_string(),
                });
                continue;
            }

            let (size, entries) = tree_size_and_entries(&run_dir);
            // Guard 3: protected output / protected data tree.
            match protected_patterns_for_snapshot(&row.pipeline_snapshot) {
                Some(patterns) if tree_is_protected(&run_dir, &entries, &patterns) => {
                    report.skipped.push(SkippedRun {
                        run_id: row.id.clone(),
                        reason: "declared protected output present".to_string(),
                    });
                    continue;
                }
                None => {
                    report.skipped.push(SkippedRun {
                        run_id: row.id.clone(),
                        reason: "workflow snapshot unparseable — treated as protected".to_string(),
                    });
                    continue;
                }
                _ => {}
            }

            if !dry_run && let Err(e) = std::fs::remove_dir_all(&run_dir) {
                if !json {
                    eprintln!("  {} {} failed to delete: {}", "✗".red(), row.id, e);
                }
                failed += 1;
                continue;
            }
            report.bytes_freed += size;
            report.reclaimed_count += 1;
            report.reclaimed.push(ReclaimedRun {
                run_id: row.id.clone(),
                user_id: row.user_id.clone(),
                path: run_dir.to_string_lossy().into_owned(),
                bytes: size,
            });
            continue;
        };
        report.skipped.push(SkippedRun {
            run_id: row.id.clone(),
            reason,
        });
    }
    report.skipped_count = report.skipped.len();
    report.failed_count = failed;

    if !dry_run && failed > 0 {
        return Err(anyhow::anyhow!(
            "runs gc: {} failed to delete",
            oxo_flow_core::plural(failed, "run tree", "run trees")
        ));
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for reclaimed in &report.reclaimed {
            if dry_run {
                eprintln!(
                    "  {} {} would be deleted",
                    "·".dimmed(),
                    reclaimed.path.dimmed()
                );
            } else {
                eprintln!("  {} {} deleted", "✓".green(), reclaimed.path);
            }
        }
        let verb = if dry_run {
            "would be deleted"
        } else {
            "deleted"
        };
        eprintln!(
            "\n{} {} run tree(s) {verb}, {} freed",
            "Summary:".bold(),
            report.reclaimed_count,
            format_bytes(report.bytes_freed)
        );
        if report.skipped_count > 0 {
            eprintln!("  {} run tree(s) kept", report.skipped_count);
        }
    }
    Ok(())
}

/// Entry point for the `runs` command group.
///
/// # Errors
/// Propagates [`runs_gc`]'s failures.
pub async fn runs_command(action: RunsAction, json: bool) -> Result<()> {
    match action {
        RunsAction::Gc {
            older_than,
            user,
            dry_run,
        } => runs_gc(older_than, user, dry_run, json).await,
    }
}

/// Human-readable per-run skip detail, keyed by reason (used only by the
/// text report; the JSON report carries the full list).
#[allow(dead_code)]
fn summarize_skips(skipped: &[SkippedRun]) -> HashMap<String, usize> {
    let mut by_reason: HashMap<String, usize> = HashMap::new();
    for s in skipped {
        *by_reason.entry(s.reason.clone()).or_insert(0) += 1;
    }
    by_reason
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        id: &str,
        status: &str,
        workdir: Option<&str>,
        finished_at: Option<&str>,
        created_at: &str,
        snapshot: &str,
    ) -> oxo_flow_web::infra::db::models::RunRow {
        oxo_flow_web::infra::db::models::RunRow {
            id: id.to_string(),
            user_id: "u1".to_string(),
            pipeline_id: None,
            pipeline_snapshot: snapshot.to_string(),
            workflow_name: None,
            status: status.to_string(),
            phase: "reporting".to_string(),
            dry_run: false,
            pid: None,
            workdir: workdir.map(str::to_string),
            config_overrides: None,
            artifacts_incomplete: None,
            started_at: None,
            finished_at: finished_at.map(str::to_string),
            created_at: created_at.to_string(),
        }
    }

    const NOW: u64 = 1_800_000_000; // 2027-01-15T08:00:00Z
    const OLD: &str = "2026-01-01T00:00:00Z"; // > 90d before NOW
    const FRESH: &str = "2027-01-14T00:00:00Z"; // < 90d before NOW

    #[test]
    fn age_cutoff_boundary_is_exact() {
        // Exactly `min_age_days` old is eligible (cutoff is inclusive).
        assert_eq!(age_cutoff_secs(NOW, 90), NOW - 90 * 86_400);
        // A one-second-younger run is still past the cutoff.
        assert!(age_cutoff_secs(NOW, 90) < NOW - 90 * 86_400 + 1);
    }

    #[test]
    fn live_run_is_never_swept() {
        for status in ["queued", "running", "paused"] {
            let r = row("run-live", status, Some("/wd"), Some(OLD), OLD, "");
            assert!(
                skip_reason(&r, &HashMap::new(), NOW, 90).is_some(),
                "{status} must be skipped"
            );
        }
    }

    #[test]
    fn terminal_old_run_is_eligible() {
        let r = row("run-old", "completed", Some("/wd-a"), Some(OLD), OLD, "");
        assert!(skip_reason(&r, &HashMap::new(), NOW, 90).is_none());
    }

    #[test]
    fn age_gate_honors_older_than_boundary() {
        // 90d gate: a fresh run is kept, the same run with a 1d gate is eligible.
        let r = row(
            "run-fresh",
            "completed",
            Some("/wd-b"),
            Some(FRESH),
            FRESH,
            "",
        );
        assert!(skip_reason(&r, &HashMap::new(), NOW, 90).is_some());
        assert!(skip_reason(&r, &HashMap::new(), NOW, 1).is_none());
    }

    #[test]
    fn lineage_target_is_spared() {
        // Two rows share one workdir: the earlier one is a lineage target.
        let a = row("run-a", "completed", Some("/shared"), Some(OLD), OLD, "");
        let b = row(
            "run-b",
            "completed",
            Some("/shared"),
            Some(FRESH),
            FRESH,
            "",
        );
        let refcounts = workdir_refcounts(&[a.clone(), b.clone()]);
        assert_eq!(refcounts.get("/shared").copied(), Some(2));
        assert!(skip_reason(&a, &refcounts, NOW, 90).is_some());
        assert!(skip_reason(&b, &refcounts, NOW, 1).is_some());
    }

    #[test]
    fn unparseable_timestamp_is_kept() {
        let r = row("run-bad", "completed", Some("/wd"), None, "not-a-date", "");
        let reason = skip_reason(&r, &HashMap::new(), NOW, 90);
        assert!(reason.is_some_and(|s| s.contains("age unknown")));
    }

    #[test]
    fn protected_output_patterns_are_collected() {
        let snapshot = r#"
[workflow]
name = "t"

[[rules]]
name = "r"
shell = "true"
protected_output = ["results/{sample}.bam"]

[engine]
protected_data_trees = ["data/refs"]
"#;
        let patterns = protected_patterns_for_snapshot(snapshot).expect("snapshot parses");
        assert!(patterns.iter().any(|p| p.contains("results/")));
        assert!(patterns.iter().any(|p| p == "data/refs"));
    }

    #[test]
    fn tree_is_protected_matches_tree_relative_paths() {
        let dir = Path::new("/tmp/run-tree");
        let entries = vec![PathBuf::from("/tmp/run-tree/results/S1.bam")];
        assert!(tree_is_protected(
            dir,
            &entries,
            &["results/{sample}.bam".to_string()]
        ));
        assert!(!tree_is_protected(
            dir,
            &entries,
            &["results/{sample}.vcf".to_string()]
        ));
    }

    #[test]
    fn unparseable_snapshot_is_fail_closed() {
        assert!(protected_patterns_for_snapshot("this is not toml = = =").is_none());
    }
}
