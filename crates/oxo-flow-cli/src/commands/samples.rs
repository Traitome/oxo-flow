//! `--samples` filtering helpers: pilot subsets, explicit names, and the
//! `ready` spec for incremental data arrival (issue #63).

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::readiness::ReadinessReport;

/// Replace a `ready` spec (issue #63) with the names of samples whose entry
/// inputs are complete.
///
/// Readiness is computed on a scratch clone (expanded) so the real config
/// stays pre-expansion for `filter_samples`. Returns the resolved specs and,
/// when `ready` was requested, the full cohort readiness report.
pub(crate) fn resolve_ready_spec(
    config: &WorkflowConfig,
    specs: &[String],
    base_dir: &std::path::Path,
) -> Result<(Vec<String>, Option<ReadinessReport>)> {
    let mut resolved: Vec<String> = Vec::new();
    let mut report: Option<ReadinessReport> = None;
    for spec in specs {
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if part == "ready" {
                if report.is_none() {
                    let mut scratch = config.clone();
                    scratch.apply_defaults();
                    scratch
                        .expand_wildcards()
                        .context("failed to expand wildcard rules")?;
                    report = Some(oxo_flow_core::readiness::compute_readiness(
                        &scratch, base_dir,
                    ));
                }
                let ready_names: Vec<String> = report
                    .as_ref()
                    .expect("report set above")
                    .ready
                    .iter()
                    .map(|status| status.name.clone())
                    .collect();
                resolved.extend(ready_names);
            } else {
                resolved.push(part.to_string());
            }
        }
    }
    Ok((resolved, report))
}

/// Apply the `--samples` filter to a pre-expansion config, resolving the
/// `ready` spec (issue #63). Returns the readiness report when `ready` was
/// requested.
///
/// `bail_on_empty` distinguishes `run` (an empty selection must abort) from
/// `dry-run` with `ready` (an empty selection is a reportable fact — the
/// readiness section explains why nothing is runnable). Static specs always
/// bail on empty selection, preserving #60 behavior.
pub(crate) fn apply_samples_filter(
    config: &mut WorkflowConfig,
    specs: &[String],
    bail_on_empty: bool,
    base_dir: &std::path::Path,
) -> Result<Option<ReadinessReport>> {
    let (resolved, report) = resolve_ready_spec(config, specs, base_dir)?;
    let pairs_before = config.pairs.len();
    let (kept, unknown) = config.filter_samples(&resolved)?;
    let pairs_dropped = pairs_before - config.pairs.len();

    for name in &unknown {
        eprintln!(
            "  {} sample '{}' not found in workflow samples",
            "⚠".yellow(),
            name
        );
    }
    let ready_requested = report.is_some();
    let selection_empty = kept.is_empty() && config.pairs.is_empty();
    if selection_empty && (bail_on_empty || !ready_requested) {
        if let Some(readiness) = &report {
            if !readiness.waiting.is_empty() && readiness.ready.is_empty() {
                let waiting: Vec<&str> = readiness
                    .waiting
                    .iter()
                    .map(|status| status.name.as_str())
                    .collect();
                anyhow::bail!(
                    "--samples ready: 0 of {} samples have complete inputs; waiting: {}",
                    readiness.total,
                    waiting.join(", ")
                );
            }
            if pairs_dropped > 0 {
                anyhow::bail!(
                    "--samples ready: no complete pairs — both experiment and control \
                     inputs must exist"
                );
            }
        }
        anyhow::bail!("--samples matched no samples in this workflow");
    }
    if pairs_dropped > 0 && ready_requested {
        eprintln!(
            "  {} {pairs_dropped} pair(s) skipped: both experiment and control inputs must be ready",
            "Note:".yellow()
        );
    }
    Ok(report)
}

/// Print the sample-readiness section (issue #63): how many samples have
/// complete entry inputs, which ones are still waiting, and any missing
/// workflow-level inputs.
pub(crate) fn print_readiness_section(report: &ReadinessReport) {
    if report.total == 0 {
        return;
    }
    eprintln!(
        "{} {}/{} complete, {} waiting",
        "Sample readiness:".bold(),
        report.ready.len(),
        report.total,
        report.waiting.len(),
    );
    const MAX_WAITING_SHOWN: usize = 8;
    for status in report.waiting.iter().take(MAX_WAITING_SHOWN) {
        match status.missing.first() {
            Some(first) if status.missing.len() == 1 => {
                eprintln!("    ⏳ {} (missing: {first})", status.name);
            }
            Some(first) => {
                eprintln!(
                    "    ⏳ {} (missing: {first} +{} more)",
                    status.name,
                    status.missing.len() - 1
                );
            }
            None => {
                eprintln!("    ⏳ {}", status.name);
            }
        }
    }
    if report.waiting.len() > MAX_WAITING_SHOWN {
        eprintln!(
            "    … and {} more waiting",
            report.waiting.len() - MAX_WAITING_SHOWN
        );
    }
    if !report.missing_global.is_empty() {
        eprintln!(
            "    {} workflow-level inputs missing (block every sample): {}",
            "⚠".yellow(),
            report.missing_global.join(", ")
        );
    }
}
