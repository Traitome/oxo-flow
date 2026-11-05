//! Backend pre-flight: BackendPreflightFailure, install hints, primary env spec, backend_preflight_failures (#948 P2-5).

use super::config_placeholder_values;
use super::findings::{known_modules_hint, undefined_config_findings};
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::rule::Rule;
use std::path::Path;

/// The shared pre-DAG gate `run` and `dry-run` both apply (issue #142 H1,
/// #112): a typo'd `{config.*}` key or an unknown `--module` must abort
/// before any file is touched, and the DAG builds with the same
/// placeholder expansion on both paths. Caller-specific `--json` failure
/// emission stays with the caller.
pub(crate) fn build_dag_and_targets(
    config: &WorkflowConfig,
    target: Vec<String>,
    module: &[String],
) -> Result<(WorkflowDag, Vec<String>)> {
    let e005 = undefined_config_findings(config);
    if !e005.is_empty() {
        anyhow::bail!(
            "workflow references undefined config variables — fix before running:\n  {}",
            e005.join("\n  ")
        );
    }

    let dag = WorkflowDag::from_rules_with_config(
        &config.rules,
        &config_placeholder_values(&config.config),
    )
    .context("failed to build workflow DAG")?;

    if let Err(type_errors) = oxo_flow_core::types::validate_workflow_types(config, &dag) {
        let findings = type_errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n  ");
        anyhow::bail!(
            "declared biological type contracts are violated — fix before running:\n  {findings}"
        );
    }

    // --module partial runs (issue #112 elasticity): each module name
    // resolves to its rules plus the host producers of its declared
    // concrete inputs; upstream DAG dependents come via the target
    // machinery below.
    let mut target = target;
    for m in module {
        match config.module_closure(m) {
            Some(names) => target.extend(names),
            None => anyhow::bail!(
                "unknown module '{m}' — known modules: {}",
                known_modules_hint(&config.module_rules)
            ),
        }
    }
    Ok((dag, target))
}

/// One consolidated backend-availability failure from the run pre-flight:
/// identical (backend, message) pairs across a cohort's instances collapse
/// into a single entry whose `count` says how many pending rules share it.
#[derive(Debug, PartialEq)]
pub(crate) struct BackendPreflightFailure {
    pub(crate) kind: String,
    pub(crate) message: String,
    pub(crate) example_rule: String,
    pub(crate) spec: String,
    pub(crate) count: usize,
}

impl std::fmt::Display for BackendPreflightFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{kind}: {message} — required by {count}, e.g. `{example_rule}` (environment: {spec}); {hint}",
            kind = self.kind,
            message = self.message,
            count = oxo_flow_core::plural(self.count, "pending rule", "pending rules"),
            example_rule = self.example_rule,
            spec = self.spec,
            hint = backend_install_hint(self),
        )
    }
}

/// Remediation hint for a preflight failure. The failure MODE decides the
/// advice (issue #848): a missing pixi MANIFEST is a workflow defect —
/// advise creating/correcting the declared file; a missing backend BINARY
/// is an environment defect — advise installing the tool.
pub(crate) fn backend_install_hint(failure: &BackendPreflightFailure) -> &'static str {
    if failure.kind == "pixi" && failure.message.contains("not found") {
        return "create the manifest or correct the rule's `pixi` path (resolved relative to the workflow directory)";
    }
    match failure.kind.as_str() {
        "pixi" => "install pixi (https://pixi.sh) and ensure it is on PATH",
        "mamba" => "install mamba or micromamba and ensure it is on PATH",
        "conda" => "install conda and ensure it is on PATH",
        "docker" => "install/start Docker and ensure `docker` is on PATH",
        "singularity" => "install Singularity/Apptainer and ensure it is on PATH",
        "modules" => "ensure environment modules (modulecmd) is on PATH",
        _ => "make the environment backend available on PATH",
    }
}

/// Primary spec string for an environment, for the failure message.
pub(crate) fn primary_env_spec(env: &oxo_flow_core::rule::EnvironmentSpec) -> String {
    env.mamba
        .as_deref()
        .or(env.conda.as_deref())
        .or(env.pixi.as_deref())
        .or(env.docker.as_deref())
        .or(env.singularity.as_deref())
        .or(env.venv.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| env.modules.join(","))
}

/// Validate each unique environment spec among the pending rules once
/// (instances fan out one rule template × N samples, so the spec set is
/// tiny even for large cohorts) and collapse the failures: rules sharing
/// one broken spec merge into a single entry, and distinct specs failing
/// with the identical message (the backend-missing case) merge too, so
/// the operator fixes one root cause per line. First-failure order is
/// preserved for stable output.
pub(crate) fn backend_preflight_failures(
    pending: &[(String, oxo_flow_core::rule::EnvironmentSpec)],
    resolver: &oxo_flow_core::environment::EnvironmentResolver,
) -> Vec<BackendPreflightFailure> {
    // spec cache_key → failure slot (None = spec validated OK)
    let mut spec_slot: std::collections::HashMap<String, Option<usize>> =
        std::collections::HashMap::new();
    // (backend kind, message) → failure slot, for merging identical root causes
    let mut msg_slot: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    let mut out: Vec<BackendPreflightFailure> = Vec::new();
    for (rule_name, env) in pending {
        let key = resolver.cache_key(env);
        match spec_slot.get(&key) {
            Some(Some(slot)) => {
                out[*slot].count += 1;
                continue;
            }
            Some(None) => continue, // identical spec, already validated OK
            None => {}
        }
        if let Err(err) = resolver.validate_spec(env) {
            let (kind, message) = match &err {
                oxo_flow_core::OxoFlowError::Environment { kind, message } => {
                    (kind.clone(), message.clone())
                }
                other => ("environment".to_string(), other.to_string()),
            };
            let slot = match msg_slot.get(&(kind.clone(), message.clone())) {
                Some(&slot) => {
                    out[slot].count += 1;
                    slot
                }
                None => {
                    out.push(BackendPreflightFailure {
                        kind: kind.clone(),
                        message: message.clone(),
                        example_rule: rule_name.clone(),
                        spec: primary_env_spec(env),
                        count: 1,
                    });
                    msg_slot.insert((kind, message), out.len() - 1);
                    out.len() - 1
                }
            };
            spec_slot.insert(key, Some(slot));
        } else {
            spec_slot.insert(key, None);
        }
    }
    out
}

/// Extract pending rules' environment specs (with workflow-relative paths resolved)
/// from a preview plan. Skipped rules are excluded because this run never invokes
/// their backend.
pub(crate) fn pending_rule_environments(
    plan: &[crate::commands::run_preview::PreviewRule],
    config: &WorkflowConfig,
    workflow_dir: &Path,
) -> Vec<(String, oxo_flow_core::rule::EnvironmentSpec)> {
    plan.iter()
        .filter(|entry| !entry.status.is_skip())
        .filter_map(|entry| {
            config.get_rule(&entry.name).map(|rule| {
                (
                    entry.name.clone(),
                    rule.environment.resolve_relative_paths(Some(workflow_dir)),
                )
            })
        })
        .filter(|(_, env)| !env.is_empty())
        .collect()
}

/// Validate backend availability before scheduling: pending rules whose backend
/// binary is missing fail fast with actionable install hints before executing any rules.
pub(crate) fn validate_pending_backends(
    pending_envs: &[(String, oxo_flow_core::rule::EnvironmentSpec)],
    resolver: &oxo_flow_core::environment::EnvironmentResolver,
) -> Result<()> {
    let env_failures = backend_preflight_failures(pending_envs, resolver);
    if env_failures.is_empty() {
        return Ok(());
    }
    let detail = env_failures
        .iter()
        .map(|f| format!("  - {f}"))
        .collect::<Vec<_>>()
        .join("\n");
    let n_backends = env_failures.len();
    let backend_desc =
        oxo_flow_core::plural(n_backends, "environment backend", "environment backends");
    let verb = if n_backends == 1 { "is" } else { "are" };
    anyhow::bail!(
        "{backend_desc} required by pending rules {verb} unavailable; no rules were run:\n{detail}"
    )
}

/// Fail fast before any rule runs if any rule's declared resource request
/// can never fit an explicit `--max-memory` or `--max-threads` cap.
pub(crate) fn validate_resource_budget(
    scheduled: &[&Rule],
    max_threads: Option<u32>,
    max_memory_mb: Option<u64>,
) -> Result<()> {
    let breaches =
        oxo_flow_core::scheduler::check_budget_feasibility(scheduled, max_threads, max_memory_mb);
    if breaches.is_empty() {
        return Ok(());
    }
    let detail = breaches
        .iter()
        .map(|b| format!("  - {b}"))
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::bail!(
        "resource budget too small for {}; no rules were run:\n{}",
        oxo_flow_core::plural(breaches.len(), "rule", "rules"),
        detail
    )
}

/// Disk pre-flight (issue #75): warn when a rule's declared `resources.disk`
/// exceeds available free space in the workdir, or when the workdir starts below
/// the configured `[engine] min_free_disk` threshold (issue #843).
pub(crate) fn warn_disk_preflight(
    scheduled: &[&Rule],
    workdir: &Path,
    engine: Option<&oxo_flow_core::config::EngineConfig>,
) {
    let disk_warnings = oxo_flow_core::scheduler::validate_disk_requirements(scheduled, workdir);
    for warning in &disk_warnings {
        eprintln!("  {} {}", "Warning:".bold().yellow(), warning);
    }

    if engine.is_some_and(|e| e.min_free_disk.is_some()) {
        #[cfg(not(unix))]
        {
            eprintln!(
                "  {} [engine] min_free_disk is configured but free-space monitoring is not supported on this platform — the monitor will stay inactive.",
                "Warning:".bold().yellow()
            );
        }
        #[cfg(unix)]
        {
            if let Some((min_mb, _reclaim_mb)) = engine.and_then(|e| e.disk_thresholds())
                && let Some(start_mb) = oxo_flow_core::scheduler::check_available_disk_mb(workdir)
                && start_mb < min_mb
            {
                eprintln!(
                    "  {} workdir starts below [engine] min_free_disk: {}MB free, threshold {}MB — the run will hold dispatches until space is freed.",
                    "Warning:".bold().yellow(),
                    start_mb,
                    min_mb
                );
            }
        }
    }
}

/// Warn when either the workdir or the system temp directory has less than 1 GB free
/// before any rule dispatches (issue #75).
pub(crate) fn warn_low_free_space(workdir: &Path) {
    for (label, path) in [
        ("workdir", workdir),
        ("temp dir", std::env::temp_dir().as_path()),
    ] {
        if let Some(free_kb) = super::refs::free_kilobytes(path) {
            let free_gb = free_kb as f64 / 1024.0 / 1024.0;
            if free_gb < 1.0 {
                tracing::warn!(
                    label,
                    path = %path.display(),
                    free_gb = format!("{free_gb:.1}"),
                    "less than 1 GB free disk space — the run may fail with ENOSPC mid-way"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dag_preflight_rejects_declared_type_contract_violations() {
        let config = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "type_gate"

            [types.raw_sam]
            format = "sam"

            [types.indexed_bam]
            format = "bam"
            index = "bai"

            [[rules]]
            name = "align"
            output = ["out.sam"]
            shell = "touch out.sam"
            [rules.types]
            outputs = { default = "raw_sam" }

            [[rules]]
            name = "call"
            input = ["out.sam"]
            output = ["calls.vcf"]
            shell = "touch calls.vcf"
            [rules.types]
            inputs = { default = "indexed_bam" }
            "#,
        )
        .unwrap();

        let err = build_dag_and_targets(&config, Vec::new(), &[]).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("biological type contracts"), "{message}");
        assert!(message.contains("TYPE-001"), "{message}");
        assert!(message.contains("TYPE-003"), "{message}");
    }

    #[test]
    fn validate_resource_budget_accepts_feasible_rules() {
        let rule = Rule {
            name: "align".to_string(),
            resources: oxo_flow_core::rule::Resources {
                threads: 4,
                memory: Some("4G".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(validate_resource_budget(&[&rule], Some(8), Some(8192)).is_ok());
    }

    #[test]
    fn validate_resource_budget_rejects_excessive_requirements() {
        let rule = Rule {
            name: "heavy_align".to_string(),
            resources: oxo_flow_core::rule::Resources {
                threads: 16,
                ..Default::default()
            },
            ..Default::default()
        };

        let err = validate_resource_budget(&[&rule], Some(8), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("resource budget too small for 1 rule"));
        assert!(msg.contains("heavy_align"));
    }

    #[test]
    fn backend_install_hint_routes_by_kind_and_message() {
        let missing_manifest = BackendPreflightFailure {
            kind: "pixi".to_string(),
            message: "manifest not found".to_string(),
            example_rule: "rule_a".to_string(),
            spec: "pixi.toml".to_string(),
            count: 1,
        };
        assert!(backend_install_hint(&missing_manifest).contains("create the manifest"));

        let missing_binary = BackendPreflightFailure {
            kind: "conda".to_string(),
            message: "conda binary unavailable".to_string(),
            example_rule: "rule_b".to_string(),
            spec: "environment.yaml".to_string(),
            count: 1,
        };
        assert!(backend_install_hint(&missing_binary).contains("install conda"));
    }
    #[test]
    fn pending_rule_environments_filters_skipped_and_empty() {
        let mut config = WorkflowConfig::parse(
            r#"
[workflow]
name = "test"
"#,
        )
        .unwrap();
        let r1 = Rule {
            name: "r1".to_string(),
            environment: oxo_flow_core::rule::EnvironmentSpec {
                pixi: Some("pixi.toml".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let r2 = Rule {
            name: "r2".to_string(),
            environment: oxo_flow_core::rule::EnvironmentSpec {
                conda: Some("env.yaml".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let r3 = Rule {
            name: "r3".to_string(),
            ..Default::default()
        };

        config.rules = vec![r1, r2, r3];

        let plan = vec![
            crate::commands::run_preview::PreviewRule {
                name: "r1".to_string(),
                status: crate::commands::run_preview::RuleStatus::NeverCompleted,
            },
            crate::commands::run_preview::PreviewRule {
                name: "r2".to_string(),
                status: crate::commands::run_preview::RuleStatus::Skipped,
            },
            crate::commands::run_preview::PreviewRule {
                name: "r3".to_string(),
                status: crate::commands::run_preview::RuleStatus::NeverCompleted,
            },
        ];

        let wf_dir = Path::new("/workflows");
        let pending = pending_rule_environments(&plan, &config, wf_dir);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "r1");
        assert_eq!(
            pending[0].1.pixi.as_deref(),
            Some(Path::new("/workflows/pixi.toml").to_str().unwrap())
        );
    }

    fn pixi_env(manifest: &str) -> oxo_flow_core::rule::EnvironmentSpec {
        oxo_flow_core::rule::EnvironmentSpec {
            pixi: Some(manifest.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn backend_preflight_passes_empty_and_empty_specs() {
        let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
        // Nothing pending → nothing to validate.
        assert!(backend_preflight_failures(&[], &resolver).is_empty());
        // A rule with no environment declaration never invokes a backend.
        let no_env = vec![("qc".to_string(), Default::default())];
        assert!(backend_preflight_failures(&no_env, &resolver).is_empty());
    }

    #[test]
    fn backend_preflight_reports_missing_pixi_manifest_once_per_unique_spec() {
        // Two pending instances of one rule template share the same spec:
        // validate it once and collapse the failures, keeping the FIRST
        // pending rule in execution order as the example (deterministic).
        let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
        let pending = vec![
            ("trim_S1".to_string(), pixi_env("envs/absent/pixi.toml")),
            ("trim_S2".to_string(), pixi_env("envs/absent/pixi.toml")),
        ];
        let failures = backend_preflight_failures(&pending, &resolver);
        assert_eq!(
            failures.len(),
            1,
            "identical specs must group: {failures:?}"
        );
        let f = &failures[0];
        assert_eq!(f.kind, "pixi");
        assert_eq!(f.count, 2);
        assert_eq!(f.example_rule, "trim_S1");
        assert_eq!(f.spec, "envs/absent/pixi.toml");
        let rendered = f.to_string();
        assert!(
            rendered.contains("required by 2 pending rules"),
            "the message must carry the cohort size: {rendered}"
        );
        // Whether THIS test observes the manifest mode depends on whether
        // the pixi binary happens to be on PATH (validate_spec checks the
        // binary first); the hint split itself is covered without that
        // dependency by backend_install_hint_splits_manifest_not_found…
    }

    #[test]
    fn backend_install_hint_splits_manifest_not_found_from_missing_binary() {
        // Issue #848: the failure MODE decides the advice. A missing pixi
        // MANIFEST is a workflow defect — the hint must point at the file,
        // not tell the operator to install pixi; a missing backend BINARY
        // keeps the install advice. Constructed failures keep this test
        // independent of the test sandbox's PATH.
        let make = |message: &str| BackendPreflightFailure {
            kind: "pixi".to_string(),
            message: message.to_string(),
            example_rule: "trim_S1".to_string(),
            spec: "envs/absent/pixi.toml".to_string(),
            count: 2,
        };
        let manifest = backend_install_hint(&make(
            "pixi manifest 'envs/absent/pixi.toml' not found — the rule declares it via `pixi = \"envs/absent/pixi.toml\"` (resolved relative to the workflow directory)",
        ));
        assert_eq!(
            manifest,
            "create the manifest or correct the rule's `pixi` path (resolved relative to the workflow directory)"
        );
        let binary = backend_install_hint(&make("pixi is not installed or not in PATH"));
        assert_eq!(
            binary,
            "install pixi (https://pixi.sh) and ensure it is on PATH"
        );
        // The mode discriminator keys on "not found", which only the
        // manifest message carries — the binary message must not trip it.
        assert!(!"pixi is not installed or not in PATH".contains("not found"));
    }

    #[test]
    fn backend_preflight_missing_backend_binary_still_advises_install() {
        // Issue #848: the OTHER failure mode — pixi the BINARY is absent —
        // keeps the install advice. validate_spec checks the binary first,
        // so an unresolvable spec with pixi absent from PATH yields the
        // "not installed" message, distinct from the manifest check.
        let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
        let pending = vec![("trim_S1".to_string(), pixi_env("envs/absent/pixi.toml"))];
        let failures = backend_preflight_failures(&pending, &resolver);
        // The exact mode depends on whether pixi is on PATH in the test
        // sandbox; assert the hint MATCHES whichever mode fired.
        let rendered = failures[0].to_string();
        if failures[0].message.contains("not found") {
            assert!(
                rendered.contains("create the manifest"),
                "manifest mode must carry the file advice: {rendered}"
            );
        } else {
            assert!(
                rendered.contains("install pixi"),
                "binary mode must keep the install advice: {rendered}"
            );
        }
    }
}
