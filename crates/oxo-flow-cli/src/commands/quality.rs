use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::commands::print_banner;

/// `"1 rule"` / `"2 rules"` — the summary counts read "1 rules" before the
/// CLI grew pluralisation (audit P4-1d).
fn plural(count: usize, singular: &str, plural_form: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural_form}")
    }
}

pub async fn validate_command(
    workflow: PathBuf,
    as_include: bool,
    json: bool,
    ai: bool,
) -> Result<()> {
    // AI: auto-detect from workflow [ai] or explicit --ai flag
    if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai) {
        crate::commands::ai_check::analyze_workflow(&workflow, &provider, "validate", "").await?;
        println!();
    }

    let config_res = WorkflowConfig::from_file(&workflow);
    match config_res {
        Ok(cfg) => {
            if cfg.rules.is_empty() {
                if json {
                    let output = serde_json::json!({
                        "command": "validate",
                        "workflow": workflow.display().to_string(),
                        "valid": true,
                        "rules": 0,
                        "dependencies": 0,
                        "errors": [],
                        "missing_inputs": [],
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    eprintln!("{} {} — 0 rules", "✓".green().bold(), workflow.display());
                    eprintln!(
                        "  {} Workflow has no rules. Add [[rules]] sections to define pipeline steps.",
                        "⚠ Warning:".yellow().bold()
                    );
                }
                return Ok(());
            }

            // Run semantic validation (E001-E008)
            let validation = oxo_flow_core::format::validate_format(&cfg);
            let mut error_count = 0usize;
            let mut errors_json: Vec<serde_json::Value> = Vec::new();

            for d in &validation.diagnostics {
                if d.severity == oxo_flow_core::format::Severity::Error {
                    // --as-include skips input-existence checks (E010, W020) since
                    // sub-workflow fragments may reference files not yet present
                    if as_include && (d.code == "E010" || d.code == "W020") {
                        continue;
                    }
                    error_count += 1;
                    if json {
                        errors_json.push(serde_json::json!({
                            "code": d.code,
                            "message": d.message,
                            "rule": d.rule,
                            "suggestion": d.suggestion,
                        }));
                    } else {
                        eprintln!("  {} [{}]: {}", "error".red().bold(), d.code, d.message);
                        if let Some(ref rule) = d.rule {
                            eprintln!("    rule: {}", rule);
                        }
                        if let Some(ref suggestion) = d.suggestion {
                            eprintln!("    hint: {}", suggestion);
                        }
                    }
                }
            }

            // Check for missing input files (skip for --as-include).
            // Relative paths resolve against the workflow file's directory —
            // the same base the executor uses at run time (issue #68).
            let workflow_dir = oxo_flow_core::parent_dir(&workflow);
            let mut missing_inputs = Vec::new();
            if !as_include {
                missing_inputs = collect_missing_inputs(&cfg, workflow_dir);
            }

            // Validate DAG construction (skip for --as-include)
            let (rules, dependencies) = if as_include {
                (cfg.rules.len(), 0)
            } else {
                match WorkflowDag::from_rules_with_config(
                    &cfg.rules,
                    &cfg.config_placeholder_values(),
                ) {
                    Ok(dag) => (dag.node_count(), dag.edge_count()),
                    Err(e) => {
                        if json {
                            let output = serde_json::json!({
                                "command": "validate",
                                "workflow": workflow.display().to_string(),
                                "valid": false,
                                "rules": cfg.rules.len(),
                                "dependencies": 0,
                                "errors": [{"code": "DAG", "message": e.to_string()}],
                                "missing_inputs": [],
                            });
                            println!("{}", serde_json::to_string_pretty(&output)?);
                        } else {
                            eprintln!(
                                "{} {} — DAG error: {}",
                                "✗".red().bold(),
                                workflow.display(),
                                e
                            );
                        }
                        std::process::exit(1);
                    }
                }
            };

            if json {
                let output = serde_json::json!({
                    "command": "validate",
                    "workflow": workflow.display().to_string(),
                    "valid": error_count == 0,
                    "rules": rules,
                    "dependencies": dependencies,
                    "errors": errors_json,
                    "missing_inputs": missing_inputs,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                if error_count == 0 {
                    eprintln!(
                        "{} {} — {}, {}",
                        "✓".green().bold(),
                        workflow.display(),
                        plural(rules, "rule", "rules"),
                        plural(dependencies, "dependency", "dependencies")
                    );
                } else {
                    eprintln!(
                        "{} {} — {} validation error(s)",
                        "✗".red().bold(),
                        workflow.display(),
                        error_count
                    );
                }

                if !missing_inputs.is_empty() {
                    eprintln!(
                        "\n  {} The following input files do not exist:",
                        "⚠ Warning:".yellow().bold()
                    );
                    for input in missing_inputs {
                        eprintln!("    - {}", input);
                    }
                }
            }

            // Exit with error if validation failed
            if error_count > 0 {
                std::process::exit(1);
            }
        }
        Err(e) => {
            if json {
                let output = serde_json::json!({
                    "command": "validate",
                    "workflow": workflow.display().to_string(),
                    "valid": false,
                    "rules": 0,
                    "dependencies": 0,
                    "errors": [{"code": "PARSE", "message": e.to_string()}],
                    "missing_inputs": [],
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                eprintln!("{} {} — {}", "✗".red().bold(), workflow.display(), e);
            }
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn lint_command(workflow: PathBuf, strict: bool, json: bool, ai: bool) -> Result<()> {
    print_banner();

    // AI: auto-detect from workflow [ai] or explicit --ai flag
    if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai) {
        crate::commands::ai_check::analyze_workflow(&workflow, &provider, "lint", "").await?;
        println!();
    }
    let config = match WorkflowConfig::from_file(&workflow) {
        Ok(config) => config,
        Err(e) => {
            // The JSON surface stays machine-readable on a parse failure too
            // (audit P4-6): it used to emit nothing on stdout, so a caller
            // could not tell a broken workflow from a crashed run — the same
            // case `validate --json` already reports as code=PARSE.
            if json {
                let output = serde_json::json!({
                    "command": "lint",
                    "workflow": workflow.display().to_string(),
                    "strict": strict,
                    "diagnostics": [{
                        "severity": "error",
                        "code": "PARSE",
                        "message": e.to_string(),
                        "rule": serde_json::Value::Null,
                        "suggestion": serde_json::Value::Null,
                    }],
                    "error_count": 1,
                    "warning_count": 0,
                    "info_count": 0,
                    "passed": false,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            }
            return Err(e).with_context(|| format!("failed to parse {}", workflow.display()));
        }
    };

    let validation = oxo_flow_core::format::validate_format(&config);
    let lint_diags = oxo_flow_core::format::lint_format(&config, workflow.parent());

    // Read the raw file content: secret scanning and the schema pass both
    // need it. verify_schema (S001–S006) is wired here because the parsed
    // config cannot retain unknown top-level keys — this is the only place
    // S006 "unknown section" warnings can ever reach a user (issue #470).
    // S001–S005 are unreachable post-parse; their checks stay in
    // verify_schema as the schema-check contract.
    let raw_content = std::fs::read_to_string(&workflow).ok();
    let schema_diags = raw_content
        .as_deref()
        .map(oxo_flow_core::format::verify_schema)
        .map(|v| v.diagnostics)
        .unwrap_or_default();
    let secret_diags = raw_content
        .as_deref()
        .map(oxo_flow_core::format::scan_for_secrets)
        .unwrap_or_default();

    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    let mut info_count = 0usize;

    for d in validation
        .diagnostics
        .iter()
        .chain(lint_diags.iter())
        .chain(schema_diags.iter())
        .chain(secret_diags.iter())
    {
        let prefix = match d.severity {
            oxo_flow_core::format::Severity::Error => {
                error_count += 1;
                "error".red().bold().to_string()
            }
            oxo_flow_core::format::Severity::Warning => {
                warning_count += 1;
                "warning".yellow().bold().to_string()
            }
            oxo_flow_core::format::Severity::Info => {
                info_count += 1;
                "info".blue().to_string()
            }
        };
        eprint!("  {} [{}]: {}", prefix, d.code, d.message);
        if let Some(ref rule) = d.rule {
            eprint!(" (rule: {})", rule);
        }
        eprintln!();
        // The fix hint is part of the human output too, not just --json —
        // the JSON-only placement silently dropped every suggestion from
        // the text report (issue #142 M11). Same style as validate.
        if let Some(ref suggestion) = d.suggestion {
            eprintln!("    hint: {}", suggestion);
        }
    }

    eprintln!(
        "\n{} {} error(s), {} warning(s), {} info",
        "Summary:".bold(),
        error_count,
        warning_count,
        info_count
    );

    // JSON output mode
    if json {
        let diagnostics: Vec<serde_json::Value> = validation
            .diagnostics
            .iter()
            .chain(lint_diags.iter())
            .chain(schema_diags.iter())
            .chain(secret_diags.iter())
            .map(|d| {
                serde_json::json!({
                    "severity": format!("{:?}", d.severity).to_lowercase(),
                    "code": d.code,
                    "message": d.message,
                    "rule": d.rule,
                    "suggestion": d.suggestion,
                })
            })
            .collect();

        let output = serde_json::json!({
            "command": "lint",
            "workflow": workflow.display().to_string(),
            "strict": strict,
            "diagnostics": diagnostics,
            "error_count": error_count,
            "warning_count": warning_count,
            "info_count": info_count,
            "passed": error_count == 0 && (!strict || warning_count == 0),
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        if error_count > 0 || (strict && warning_count > 0) {
            std::process::exit(1);
        }
        return Ok(());
    }

    if error_count > 0 || (strict && warning_count > 0) {
        std::process::exit(1);
    }
    Ok(())
}

/// Deep pipeline health checks for `test --deep` (issue #64).
///
/// Checks script files (D001, error), environment definition files (D002,
/// warning), system-backend binaries in PATH (D003, warning), and reference
/// data paths (D004, warning). Exits 1 when any error-severity finding is
/// reported; warnings are informational — PATH and reference data are
/// machine-specific and can arrive later (issue #63).
pub fn deep_check_command(workflow: &Path, workdir: Option<&Path>, json: bool) -> Result<()> {
    let (report, document) = run_deep_check(workflow, workdir)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&document)?);
    }

    if report.error_count > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Run the deep checks, print the human report to stderr, and build the JSON
/// document.
///
/// Shared by `deep-check --json` and the `test --json` aggregate (which
/// embeds the document instead of printing a second one — audit finding).
pub fn run_deep_check(
    workflow: &Path,
    workdir: Option<&Path>,
) -> Result<(
    oxo_flow_core::deep_check::DeepCheckReport,
    serde_json::Value,
)> {
    let config = WorkflowConfig::from_file(workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;
    // Judge existence from the same base the executor runs rules from:
    // `--workdir`, or the workflow file's directory (issue #68 semantics).
    let base_dir = workdir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| oxo_flow_core::parent_dir(workflow).to_path_buf());
    let report = oxo_flow_core::deep_check::compute_deep_check(&config, &base_dir);

    print_deep_console(&report);

    let diagnostics: Vec<serde_json::Value> = report
        .findings
        .iter()
        .map(|f| {
            serde_json::json!({
                "severity": format!("{:?}", f.severity).to_lowercase(),
                "code": f.code,
                "message": f.message,
                "rule": f.rule,
                "suggestion": f.suggestion,
                "path": f.path,
            })
        })
        .collect();
    let document = serde_json::json!({
        "command": "deep-check",
        "workflow": workflow.display().to_string(),
        "diagnostics": diagnostics,
        "error_count": report.error_count,
        "warning_count": report.warning_count,
        "passed": report.passed,
    });
    Ok((report, document))
}

/// Print the human-readable deep-check report to stderr, grouped by category
/// with a green summary line per clean category (lint precedent: human
/// output goes to stderr even in `--json` mode).
fn print_deep_console(report: &oxo_flow_core::deep_check::DeepCheckReport) {
    let finding_lines = |code: &str| {
        for f in report.findings.iter().filter(|f| f.code == code) {
            let icon = if f.severity == oxo_flow_core::format::Severity::Error {
                "✗".red().bold()
            } else {
                "⚠".yellow().bold()
            };
            let rule_suffix = f
                .rule
                .as_deref()
                .map(|rule| format!(" (rule: {rule})"))
                .unwrap_or_default();
            eprintln!("    {} {}{} [{}]", icon, f.message, rule_suffix, f.code);
            if let Some(hint) = &f.suggestion {
                eprintln!("      hint: {}", hint.dimmed());
            }
        }
    };

    // Categories with nothing checked and no findings are omitted entirely.
    let has = |code: &str| report.findings.iter().any(|f| f.code == code);

    if report.scripts_checked > 0 || has("D001") {
        eprintln!("{}", "  Scripts:".bold());
        if report.scripts_checked > 0 && !has("D001") {
            eprintln!(
                "    {} {} script reference(s) found",
                "✓".green().bold(),
                report.scripts_checked
            );
        }
        finding_lines("D001");
    }

    if report.envs_checked > 0 || has("D002") {
        eprintln!("{}", "  Environments:".bold());
        if report.envs_checked > 0 && !has("D002") {
            eprintln!(
                "    {} {} environment definition(s) found",
                "✓".green().bold(),
                report.envs_checked
            );
        }
        finding_lines("D002");
    }

    if report.commands_probed > 0 || has("D003") {
        eprintln!("{}", "  Binaries:".bold());
        if report.commands_probed > 0 && !has("D003") {
            eprintln!(
                "    {} {} command(s) found in PATH",
                "✓".green().bold(),
                report.commands_probed
            );
        }
        finding_lines("D003");
    }

    if report.references_checked > 0 || has("D004") {
        eprintln!("{}", "  References:".bold());
        if report.references_checked > 0 && !has("D004") {
            eprintln!(
                "    {} {} reference path(s) found",
                "✓".green().bold(),
                report.references_checked
            );
        }
        finding_lines("D004");
    }

    eprintln!(
        "\n{} {} error(s), {} warning(s)",
        "Deep check summary:".bold(),
        report.error_count,
        report.warning_count
    );
}

pub fn format_command(workflow: PathBuf, output: Option<PathBuf>, check: bool) -> Result<()> {
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    let formatted = oxo_flow_core::format::format_workflow(&config);

    if check {
        let original = std::fs::read_to_string(&workflow)?;
        if original.trim() == formatted.trim() {
            eprintln!(
                "{} {} is already formatted",
                "✓".green().bold(),
                workflow.display()
            );
        } else {
            eprintln!(
                "{} {} needs formatting",
                "✗".red().bold(),
                workflow.display()
            );
            std::process::exit(1);
        }
    } else {
        match output {
            Some(path) => {
                std::fs::write(&path, &formatted)?;
                eprintln!("Formatted workflow written to {}", path.display());
            }
            None => {
                print!("{formatted}");
            }
        }
    }
    Ok(())
}

pub fn touch_command(
    workflow: PathBuf,
    rules: Vec<String>,
    workdir: Option<PathBuf>,
    cli_args: Vec<String>,
) -> Result<()> {
    print_banner();
    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // Config overrides first: rules may branch on {config.*} before the
    // engine ever expands wildcards (issue #432). Parsing/validation reuses
    // the exact `run`/`dry-run` machinery so `touch` and `run` agree on
    // what an override means.
    let declared_config_keys: std::collections::HashSet<String> =
        config.config_meta.keys().cloned().collect();
    let cli_arg_values = super::run::parse_cli_overrides(cli_args, &declared_config_keys, "touch")?;
    super::run::apply_cli_overrides(&mut config, &cli_arg_values)?;
    super::run::warn_unknown_override_keys(&cli_arg_values, &declared_config_keys, &workflow);

    config.apply_defaults();
    // Expand wildcards so output patterns are concrete paths
    if let Err(e) = config.expand_wildcards() {
        eprintln!("  {} Could not expand wildcards: {}", "Note:".yellow(), e);
        eprintln!(
            "  {} Wildcard patterns in outputs will be skipped.",
            "Info:".dimmed()
        );
    }

    // Outputs may embed {config.*} references (e.g. "{config.out_dir}/x.bam")
    // — resolve them against the overridden config so `touch` and `run`
    // agree on where the files live.
    let config_values: HashMap<String, String> =
        super::run::config_placeholder_values(&config.config);

    // A typo'd rule name must not exit 0 with "0 file(s) touched" — scripts
    // would read that as success (audit #276 P4-5). The command fails at the
    // end when this list is non-empty, after touching whatever it did match.
    let unknown_rules: Vec<&String> = rules
        .iter()
        .filter(|name| !config.rules.iter().any(|rule| &rule.name == *name))
        .collect();
    let rules_to_touch: Vec<&oxo_flow_core::rule::Rule> = if rules.is_empty() {
        config.rules.iter().collect()
    } else {
        let matched: Vec<&oxo_flow_core::rule::Rule> = config
            .rules
            .iter()
            .filter(|r| rules.contains(&r.name))
            .collect();
        for name in &unknown_rules {
            eprintln!(
                "  {} rule '{name}' not found in workflow — no files touched for it",
                "⚠".yellow()
            );
        }
        if !unknown_rules.is_empty() {
            let expanded_hint = if config.rules.len() != config.rule_templates.len() {
                " (run `oxo-flow dry-run <workflow>` to list expanded rule names)"
            } else {
                ""
            };
            eprintln!(
                "  {} known rules: {}{expanded_hint}",
                "Info:".dimmed(),
                config
                    .rules
                    .iter()
                    .map(|r| r.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        matched
    };

    let mut touched = 0usize;
    let mut skipped = 0usize;
    let mut skipped_patterns: Vec<(String, String)> = Vec::new(); // (rule_name, pattern)

    // Outputs live next to the workflow file (--workdir overrides) — the
    // same path-resolution convention every other workflow command uses.
    // The old current_dir() default touched files in the caller's cwd
    // when the workflow lived elsewhere (CLI alignment audit 2026-08-14).
    let base_dir = workdir
        .clone()
        .unwrap_or_else(|| workflow.parent().map(Path::to_path_buf).unwrap_or_default());

    for rule in &rules_to_touch {
        for output in &rule.output {
            let output =
                oxo_flow_core::executor::checkpoint::expand_config_in_path(output, &config_values);
            let has_wildcard = output.contains('{') && output.contains('}');
            if has_wildcard {
                skipped += 1;
                skipped_patterns.push((rule.name.clone(), output.clone()));
                continue;
            }

            // Path safety: reject path traversal and absolute paths
            if output.contains("..") || output.starts_with('/') || output.starts_with('~') {
                eprintln!("  {} {} (rejected: unsafe path)", "✗".red().bold(), output);
                continue;
            }

            let path = base_dir.join(&output);
            if path.exists() {
                // Update modification time
                match filetime::set_file_mtime(&path, filetime::FileTime::now()) {
                    Ok(()) => {
                        touched += 1;
                        eprintln!("  {} {}", "✓".green(), output);
                    }
                    Err(e) => {
                        eprintln!("  {} {} ({})", "✗".red(), output, e);
                    }
                }
            } else {
                // Create empty file to mark as "done"
                if let Some(parent) = path.parent()
                    && let Err(e) = std::fs::create_dir_all(parent)
                {
                    eprintln!(
                        "  {} {} (cannot create directory: {})",
                        "✗".red(),
                        output,
                        e
                    );
                    continue;
                }
                match std::fs::write(&path, "") {
                    Ok(()) => {
                        touched += 1;
                        eprintln!("  {} {} (created)", "✓".green(), output);
                    }
                    Err(e) => {
                        eprintln!("  {} {} (failed: {})", "✗".red(), output, e);
                    }
                }
            }
        }
    }

    eprintln!(
        "\n{} {} file(s) touched, {} wildcard pattern(s) skipped",
        "Done:".bold(),
        touched,
        skipped
    );

    if !skipped_patterns.is_empty() {
        eprintln!();
        for (rule_name, pattern) in &skipped_patterns {
            eprintln!(
                "  {} {} → {} (wildcard pattern — not expanded)",
                "Skipped:".yellow(),
                rule_name,
                pattern.dimmed()
            );
        }
        eprintln!();
        eprintln!(
            "  {} To touch expanded rules, use specific rule names after wildcard expansion.",
            "Tip:".bold().cyan()
        );
        eprintln!(
            "  {} Run 'oxo-flow dry-run {}' to see expanded rule names, then use:",
            "   ".dimmed(),
            workflow.display()
        );
        eprintln!(
            "  {}   oxo-flow touch {} --rule <expanded_name>",
            "   ".dimmed(),
            workflow.display()
        );
    }

    // Requesting a rule that does not exist is a usage error, not a no-op:
    // exit non-zero so scripts (and the audit that found this) can tell the
    // difference between "touched" and "nothing matched".
    if !unknown_rules.is_empty() {
        anyhow::bail!(
            "{} requested rule name(s) not found in '{}' — nothing was touched for them.\n  \
             Run `oxo-flow dry-run {}` to list the expanded rule names.",
            unknown_rules.len(),
            workflow.display(),
            workflow.display()
        );
    }

    Ok(())
}

/// Inputs that resolve to no file and no producer — the CLI validate /
/// doctor missing-input pass (issue #467).
///
/// `{config.*}` placeholders expand against the parsed config before the
/// concrete checks so a config-routed input is existence-checked against
/// its real path, and a concrete input whose producer declares the same
/// path through a config-routed output template is recognized as
/// generated instead of "missing". Engine wildcards (`{sample}`, …) keep
/// their raw-token handling: they need a declared sample domain to ever
/// bind (issue #324 F-3), and without one the run fails mid-flight with
/// a literal brace token — validate must flag that here.
fn collect_missing_inputs(cfg: &WorkflowConfig, workflow_dir: &std::path::Path) -> Vec<String> {
    // Sample placeholders need a declared sample domain to resolve at
    // run time: without any of sample_groups / pairs / sample_pattern the
    // wildcard can never bind.
    let sample_domain_declared = !cfg.sample_groups.is_empty()
        || !cfg.pairs.is_empty()
        || cfg.workflow.sample_pattern.is_some();
    const SAMPLE_PLACEHOLDERS: &[&str] = &[
        "{sample}",
        "{group}",
        "{pair_id}",
        "{experiment}",
        "{control}",
        "{tumor}",
        "{normal}",
        "{log}",
    ];
    let expand = |path: &str| oxo_flow_core::config::expand_config_vars_in_path(path, &cfg.config);
    let mut missing = Vec::new();
    for rule in &cfg.rules {
        // A `when`-gated-off rule can never run in this configuration —
        // its inputs are not required to exist (issue #493, same stance as
        // the validate-side E010/W020 gate).
        if rule.when.as_deref().is_some_and(|when| {
            !oxo_flow_core::executor::process::evaluate_condition_with_wildcards_and_base_dir(
                when,
                &cfg.config,
                &std::collections::HashMap::new(),
                Some(workflow_dir),
            )
        }) {
            continue;
        }
        for input in &rule.input {
            let expanded = expand(input);
            if expanded.contains('{') {
                if !sample_domain_declared
                    && SAMPLE_PLACEHOLDERS.iter().any(|ph| input.contains(ph))
                {
                    // Wildcard input with no sample domain to bind it.
                    missing.push(format!(
                        "{input} (no sample groups/pairs/sample_pattern declared)"
                    ));
                }
                continue;
            }
            if !workflow_dir.join(&expanded).exists() {
                // Also check if it's an output of another rule — outputs
                // expand with the same map so a concrete input matches its
                // config-routed producer template.
                let is_generated = cfg
                    .rules
                    .iter()
                    .any(|r| r.output.iter().any(|o| expand(o) == expanded));
                if !is_generated {
                    missing.push(expanded);
                }
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use assert_cmd::Command;
    use predicates::prelude::PredicateBooleanExt;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn missing_inputs_resolve_config_placeholders_and_when_gates() {
        use super::collect_missing_inputs;
        // Issue #467: config-routed producers must satisfy concrete inputs
        // and config-routed inputs must be existence-checked. Issue #493:
        // a when-gated-off rule's missing inputs stay silent.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("ref.fa");
        std::fs::write(&real, b"ACGT").unwrap();
        let toml = format!(
            r#"
[workflow]
name = "t"

[config]
out = "{}"
reference = "{}"
prepare_reference = false

[[rules]]
name = "produce"
output = ["{{config.out}}/x.txt"]
shell = "echo x > {{config.out}}/x.txt"

[[rules]]
name = "consume"
input = ["{}/x.txt", "{{config.reference}}", "definitely-missing.txt"]
output = ["final.txt"]
shell = "cat {{config.reference}} > final.txt"

[[rules]]
name = "gated_index"
input = ["{{config.reference}}.missing-sidecar"]
output = ["sidecar.done"]
when = "config.prepare_reference"
shell = "true"
"#,
            dir.path().display(),
            real.display(),
            dir.path().display()
        );
        let cfg = oxo_flow_core::config::WorkflowConfig::parse(&toml).unwrap();
        let missing = collect_missing_inputs(&cfg, dir.path());
        assert_eq!(
            missing,
            vec!["definitely-missing.txt".to_string()],
            "config-routed generated input and existing config-routed input must pass; \
             only the genuinely missing file may be flagged; the when-gated-off rule's \
             missing sidecar must stay silent (issue #493): {missing:?}"
        );
    }

    #[test]
    fn test_as_include_skips_dag_validation() {
        // Create a fragment with rules that reference undefined inputs
        let fragment = r#"
[workflow]
name = "qc-fragment"

[[rules]]
name = "fastqc"
input = ["{sample}.fastq"]
output = ["{sample}_fastqc.html"]
shell = "fastqc {input}"
"#;
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(fragment.as_bytes()).unwrap();

        // Should pass with --as-include (skips DAG validation)
        Command::cargo_bin("oxo-flow")
            .unwrap()
            .arg("validate")
            .arg("--as-include")
            .arg(file.path())
            .assert()
            .success();
    }

    #[test]
    fn test_as_include_validates_syntax() {
        // Create an invalid fragment (missing required 'name' field)
        let fragment = r#"
[workflow]
name = "bad-fragment"

[[rules]]
# Missing required 'name' field
input = ["test.txt"]
"#;
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(fragment.as_bytes()).unwrap();

        // Should fail even with --as-include (syntax errors)
        Command::cargo_bin("oxo-flow")
            .unwrap()
            .arg("validate")
            .arg("--as-include")
            .arg(file.path())
            .assert()
            .failure();
    }

    #[test]
    fn validate_summary_pluralises_counts() {
        // Audit P4-1d: a single-rule workflow reported "1 rules".
        let single = r#"
[workflow]
name = "one-rule"

[[rules]]
name = "only"
output = ["o.txt"]
shell = "true"
"#;
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(single.as_bytes()).unwrap();

        Command::cargo_bin("oxo-flow")
            .unwrap()
            .arg("validate")
            .arg(file.path())
            .assert()
            .success()
            .stderr(
                predicates::str::contains("1 rule, 0 dependencies")
                    .and(predicates::str::contains("1 rules").not()),
            );
    }

    #[test]
    fn lint_json_reports_parse_failures() {
        // Audit P4-6: `lint --json` on an unparseable workflow used to print
        // nothing on stdout, leaving the caller without a machine-readable
        // verdict; `validate --json` already reports code=PARSE here.
        let broken = "[workflow\nname = \"broken\"\n";
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(broken.as_bytes()).unwrap();

        let assert = Command::cargo_bin("oxo-flow")
            .unwrap()
            .arg("lint")
            .arg("--json")
            .arg(file.path())
            .assert()
            .failure();

        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("lint --json must emit JSON on stdout");
        assert_eq!(parsed["command"], "lint");
        assert_eq!(parsed["passed"], false);
        assert_eq!(parsed["error_count"], 1);
        assert_eq!(parsed["diagnostics"][0]["code"], "PARSE");
        assert_eq!(parsed["diagnostics"][0]["severity"], "error");
        assert!(
            !stdout.is_empty(),
            "the JSON surface must never be empty, even on a parse failure"
        );
    }

    #[test]
    fn test_touch_accepts_config_overrides() {
        // Issue #432: `touch` must accept the same KEY=VALUE override form
        // as `run`, and outputs that reference {config.*} must resolve
        // against the overridden value.
        let fragment = r#"
[workflow]
name = "touch-override"

[config]
out_dir = { default = "results", required = false }

[[rules]]
name = "step"
input = []
output = ["{config.out_dir}/done.flag"]
shell = "touch {output[0]}"
"#;
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(&wf, fragment).unwrap();

        Command::cargo_bin("oxo-flow")
            .unwrap()
            .args(["touch", wf.to_str().unwrap(), "out_dir=other"])
            .assert()
            .success();

        assert!(
            dir.path().join("other/done.flag").exists(),
            "output must resolve under the overridden out_dir"
        );
        assert!(
            !dir.path().join("results/done.flag").exists(),
            "default out_dir must NOT be touched when overridden"
        );
    }

    #[test]
    fn test_touch_unknown_flag_is_command_aware() {
        // A run-only flag must be rejected with a touch-aware message
        // (issue #430 contract shared via parse_cli_overrides).
        let fragment = r#"
[workflow]
name = "touch-flag"

[[rules]]
name = "step"
input = []
output = ["a.txt"]
shell = "touch a.txt"
"#;
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(fragment.as_bytes()).unwrap();

        let assert = Command::cargo_bin("oxo-flow")
            .unwrap()
            .args(["touch", file.path().to_str().unwrap(), "--force"])
            .assert()
            .failure();

        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
        assert!(
            stderr.contains("touch"),
            "flag error must name the touch command, got: {stderr}"
        );
    }
}
