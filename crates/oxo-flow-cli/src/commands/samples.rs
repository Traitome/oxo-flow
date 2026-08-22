//! `--samples` sample-selection helpers: sheet override (`@path`), sheet
//! append (`+@path`), name declaration on template workflows, pilot
//! subsets (`first:N`), explicit-name filters, and the `ready` spec for
//! incremental data arrival (issue #63).

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::{SampleGroup, WorkflowConfig};
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
    // Split specs, applying sheet operations IN ORDER (a later `@path`
    // resets earlier `+@path` appends):
    //   `@path`  — REPLACE: the sheet's groups override the workflow's set
    //              (samples the workflow never declared become the new set);
    //   `+@path` — APPEND: same-name groups merge (union, dedup), new
    //              groups are added — the sheet can only add samples;
    //   names / `first:N` / `ready` — FILTER the (possibly replaced or
    //              appended) set to a subset; unknown names fail (issue
    //              #79's phantom-sample guard). Filters apply AFTER every
    //              sheet op, regardless of their position in the spec.
    let mut did_sample_op = false;
    let mut filter_specs: Vec<String> = Vec::new();
    for spec in specs {
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (action, path) = if let Some(path) = part.strip_prefix("+@") {
                ("append", path)
            } else if let Some(path) = part.strip_prefix('@') {
                ("override", path)
            } else {
                filter_specs.push(part.to_string());
                continue;
            };
            // Resolve relative sheet paths against base_dir — the SAME
            // base the `ready` spec uses — never the process CWD, so a
            // relative sheet does not silently resolve differently
            // depending on where the CLI was invoked (issue #136 tier-2
            // audit; `--workdir`/the workflow directory is the documented
            // resolution base for --samples paths).
            let sheet_path = std::path::Path::new(path);
            let sheet_path = if sheet_path.is_absolute() {
                sheet_path.to_path_buf()
            } else {
                base_dir.join(sheet_path)
            };
            let groups = SampleGroup::load_from_file(&sheet_path)
                .with_context(|| format!("failed to load samplesheet '{path}'"))?;
            // A samplesheet with no data rows must fail loudly: silently
            // falling back to the discovered samples would run the WRONG
            // set (the whole point of the @ signals is explicitness).
            if groups.is_empty() {
                anyhow::bail!(
                    "--samples {} '{path}' contains no sample rows \
                     (expected a 'name'/'samples' sheet)",
                    if action == "append" { "+@" } else { "@" }
                );
            }
            if action == "append" {
                config.append_sample_groups(groups)?;
            } else {
                let kept = config.override_sample_groups(groups)?;
                // An override that selects nothing is a static authoring
                // error — never a silent zero-instance run.
                if kept.is_empty() {
                    anyhow::bail!(
                        "--samples @path override produced no samples (all rows are empty)"
                    );
                }
            }
            did_sample_op = true;
        }
    }

    // No subset filter: the sheet operation alone defines the run set.
    if did_sample_op && filter_specs.is_empty() {
        let total: usize = config.sample_groups.iter().map(|g| g.samples.len()).sum();
        // A sheet whose rows select zero instances must fail loudly — the
        // same empty-selection guard the filter path enforces below. The
        // `+@` append path can reach this with a row whose samples cell is
        // empty on a workflow that declares none, and a silent
        // "Running 0 sample(s)" would be a zero-instance run (issue #136
        // audit). `ready` can never reach this path, so an empty selection
        // always bails when the flag demands it.
        let selection_empty = total == 0 && config.pairs.is_empty();
        if selection_empty && bail_on_empty {
            anyhow::bail!("--samples matched no samples in this workflow");
        }
        eprintln!(
            "  {} Running {} sample(s) via --samples (sheet selection)",
            "Samples:".cyan(),
            total
        );
        return Ok(None);
    }

    // Bare names on a workflow that declares NO samples are a sample
    // DECLARATION (replace), not a filter — the template-workflow
    // invocation pattern (`--samples SRR1,SRR2` on a workflow shipped
    // without fixtures). On a workflow WITH declared samples the same
    // names keep their filter semantics, so the phantom-sample guard
    // still fails typos there.
    let mut bare_names: Vec<String> = Vec::new();
    let mut pure_filter_specs: Vec<String> = Vec::new();
    for part in &filter_specs {
        if part.starts_with("first:") || part == "ready" {
            pure_filter_specs.push(part.clone());
        } else {
            bare_names.push(part.clone());
        }
    }
    if config.sample_groups.is_empty() && !bare_names.is_empty() {
        config.override_samples(&bare_names)?;
        if pure_filter_specs.is_empty() {
            eprintln!(
                "  {} Running {} sample(s) via --samples (name declaration)",
                "Samples:".cyan(),
                bare_names.len()
            );
            return Ok(None);
        }
        // Template workflow + names + first:N/ready: names declare the
        // set, the remaining specs filter it.
        filter_specs = pure_filter_specs;
    }

    let (resolved, report) = resolve_ready_spec(config, &filter_specs, base_dir)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::config::WorkflowConfig;

    fn config_with_inline_samples() -> WorkflowConfig {
        WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"
            version = "1.0.0"

            [[sample_groups]]
            name = "cohort"
            samples = ["S1", "S2"]

            [[rules]]
            name = "align"
            input = ["raw/{sample}.fq"]
            output = ["aln/{sample}.bam"]
            shell = "touch {output}"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn samplesheet_override_replaces_inline_samples() {
        let path = std::env::temp_dir().join("oxo_flow_override_test_samples.tsv");
        std::fs::write(&path, "name\tsamples\ncohort\tSRR1,SRR2\n").unwrap();

        let mut config = config_with_inline_samples();
        let spec = format!("@{}", path.display());
        let result = apply_samples_filter(&mut config, &[spec], true, std::path::Path::new("."));
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok(), "override should succeed: {result:?}");
        assert_eq!(config.sample_groups.len(), 1);
        assert_eq!(config.sample_groups[0].name, "cohort");
        assert_eq!(
            config.sample_groups[0].samples,
            vec!["SRR1".to_string(), "SRR2".to_string()]
        );
    }

    #[test]
    fn append_sheet_merges_same_name_group() {
        let path = std::env::temp_dir().join("oxo_flow_append_test_samples.tsv");
        std::fs::write(&path, "name\tsamples\ncohort\tS2,S3\n").unwrap();

        let mut config = config_with_inline_samples();
        let spec = format!("+@{}", path.display());
        let result = apply_samples_filter(&mut config, &[spec], true, std::path::Path::new("."));
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok(), "append should succeed: {result:?}");
        // S1 from the workflow, S2 deduped, S3 appended.
        assert_eq!(
            config.sample_groups[0].samples,
            vec!["S1".to_string(), "S2".to_string(), "S3".to_string()]
        );
    }

    #[test]
    fn empty_samplesheet_fails_loudly() {
        // Header-only sheet: must fail — silently falling back to the
        // workflow's own samples would run the WRONG set.
        let path = std::env::temp_dir().join("oxo_flow_empty_samples.tsv");
        std::fs::write(&path, "name\tsamples\n").unwrap();

        let mut config = config_with_inline_samples();
        let spec = format!("@{}", path.display());
        let result = apply_samples_filter(&mut config, &[spec], true, std::path::Path::new("."));
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err(), "empty sheet must fail");
        assert!(format!("{result:?}").contains("no sample rows"));
    }

    #[test]
    fn override_with_only_empty_rows_fails_loudly() {
        // A row with an empty samples cell selects nothing — a static
        // authoring error, never a silent zero-instance run.
        let path = std::env::temp_dir().join("oxo_flow_empty_rows_samples.tsv");
        std::fs::write(&path, "name\tsamples\ncohort\t\n").unwrap();

        let mut config = config_with_inline_samples();
        let spec = format!("@{}", path.display());
        let result = apply_samples_filter(&mut config, &[spec], true, std::path::Path::new("."));
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err(), "empty rows must fail");
        assert!(format!("{result:?}").contains("produced no samples"));
    }

    #[test]
    fn zero_instance_append_sheet_fails_loudly_when_flag_demands_it() {
        // `+@sheet` on a workflow that declares no samples: a row whose
        // samples cell is empty leaves the selection with zero instances.
        // The sheet-only path must honor the same empty-selection guard as
        // name/first:N/ready — never a silent "Running 0 sample(s)".
        let path = std::env::temp_dir().join("oxo_flow_zero_instance_samples.tsv");
        std::fs::write(&path, "name\tsamples\ncohort\t\n").unwrap();

        let mut config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "template"
            version = "1.0.0"

            [[rules]]
            name = "analyze"
            output = ["out/{sample}.txt"]
            shell = "echo {sample} > {output}"
            "#,
        )
        .unwrap();
        let spec = format!("+@{}", path.display());
        let result = apply_samples_filter(&mut config, &[spec], true, std::path::Path::new("."));
        let _ = std::fs::remove_file(&path);
        assert!(
            result.is_err(),
            "zero-instance append must fail: {result:?}"
        );
        assert!(
            format!("{result:?}").contains("matched no samples"),
            "error must name the empty selection: {result:?}"
        );

        // The same append with a sample is a normal selection.
        let path = std::env::temp_dir().join("oxo_flow_zero_instance_samples.tsv");
        std::fs::write(&path, "name\tsamples\ncohort\tSRR1\n").unwrap();
        let mut config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "template"
            version = "1.0.0"

            [[rules]]
            name = "analyze"
            output = ["out/{sample}.txt"]
            shell = "echo {sample} > {output}"
            "#,
        )
        .unwrap();
        let spec = format!("+@{}", path.display());
        let result = apply_samples_filter(&mut config, &[spec], true, std::path::Path::new("."));
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok(), "non-empty append must succeed: {result:?}");
    }

    #[test]
    fn bare_names_declare_samples_on_template_workflows() {
        // A workflow without any declared samples: bare names are a sample
        // DECLARATION (replace) — the template-workflow invocation pattern.
        let mut config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "template"
            version = "1.0.0"

            [[rules]]
            name = "analyze"
            output = ["out/{sample}.txt"]
            shell = "echo {sample} > {output}"
            "#,
        )
        .unwrap();
        let result = apply_samples_filter(
            &mut config,
            &["SRR1,SRR2".to_string()],
            true,
            std::path::Path::new("."),
        );
        assert!(result.is_ok(), "name declaration: {result:?}");
        assert_eq!(config.sample_groups.len(), 1);
        assert_eq!(config.sample_groups[0].name, "samples");
        assert_eq!(
            config.sample_groups[0].samples,
            vec!["SRR1".to_string(), "SRR2".to_string()]
        );
    }

    #[test]
    fn explicit_names_still_filter_and_reject_unknown() {
        // Known name → subset filter holds.
        let mut config = config_with_inline_samples();
        let result = apply_samples_filter(
            &mut config,
            &["S1".to_string()],
            true,
            std::path::Path::new("."),
        );
        assert!(result.is_ok(), "known name filter: {result:?}");
        assert_eq!(config.sample_groups[0].samples, vec!["S1".to_string()]);

        // Unknown name → the phantom-sample guard fails the selection.
        let mut config = config_with_inline_samples();
        let result = apply_samples_filter(
            &mut config,
            &["S99".to_string()],
            true,
            std::path::Path::new("."),
        );
        assert!(result.is_err(), "unknown name must fail: {result:?}");
    }

    #[test]
    fn relative_sheet_resolves_against_base_dir_not_cwd() {
        // `@path` resolves against the workdir base — the SAME base the
        // `ready` spec uses — never the process CWD (issue #136 tier-2
        // audit: the two specs used to resolve differently, so a relative
        // sheet silently depended on where the CLI was invoked).
        let base = std::env::temp_dir().join("oxo_flow_sheet_base_test");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("sheet.tsv"), "name\tsamples\ncohort\tSRR1\n").unwrap();

        let mut config = config_with_inline_samples();
        let result = apply_samples_filter(&mut config, &["@sheet.tsv".to_string()], true, &base);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_ok(),
            "sheet must resolve under base_dir, not the CWD: {result:?}"
        );
        assert_eq!(config.sample_groups[0].samples, vec!["SRR1".to_string()]);
    }
}
