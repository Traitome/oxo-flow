use crate::commands::{print_banner, resolve_workflow, samples};
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::config_impact::{ConfigChangeReport, config_value_string};
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::{CheckpointState, ExecutorConfig, LocalExecutor, WorkdirLock};
use oxo_flow_core::rule::{Rule, parse_duration_secs};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Flatten workflow config values into the `{config.key}` placeholder map
/// used for path expansion (DAG edge matching, run-time rendering, …).
fn config_placeholder_values(config: &HashMap<String, toml::Value>) -> HashMap<String, String> {
    config
        .iter()
        .map(|(key, value)| {
            let string_val = match value {
                toml::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (format!("config.{key}"), string_val)
        })
        .collect()
}

/// Emit one progress narrative line to BOTH the console (stderr, exactly
/// as before) and the tracing stream — the tracing copy is what the
/// archived run log preserves (issue #194 B1). Colors live in the console
/// form; the log side gets the same text and strips ANSI at write time.
///
/// Background runs skip the tracing copy: their stderr is already
/// redirected onto the run log, so a second copy would duplicate the line
/// (issue #194 A3).
fn progress_narrate(msg: std::fmt::Arguments<'_>) {
    eprintln!("{msg}");
    if std::env::var_os("OXO_FLOW_STDERR_ALREADY_REDIRECTED").is_none() {
        tracing::info!(target: "progress", message = %msg.to_string());
    }
}

/// Emit a structured [`ExecutionEvent`] as one JSON line in the tracing
/// stream (issue #194 B3): the event schema stops being dead code and the
/// run log gains a machine-readable execution timeline alongside the prose.
/// Background runs skip it for the same reason as [`progress_narrate`] —
/// the redirected stderr already carries the line exactly once.
fn emit_execution_event(event: oxo_flow_core::executor::ExecutionEvent) {
    if std::env::var_os("OXO_FLOW_STDERR_ALREADY_REDIRECTED").is_none() {
        tracing::info!(target: "execution_event", "{}", event.to_json_log());
    }
}

/// Print the config-change impact summary (issue #62).
///
/// Distinguishes the full invalidation set (checkpoint mutation; includes
/// rules outside this run's targets/samples) from the rules that will
/// actually re-execute in THIS run (intersection with `order`). Under
/// `--rerun` everything is forced anyway, so the summary is suppressed —
/// the snapshot/fingerprint refresh still happened silently.
#[allow(clippy::too_many_arguments)]
/// Values of `[config_meta.*] sensitive = true` keys, for runner-side
/// output masking (issue #99 B1). Shared by the local run, the verbose
/// plan print, and the cluster path.
fn sensitive_values_of(config: &WorkflowConfig) -> Vec<String> {
    config
        .config_meta
        .iter()
        .filter(|(_, def)| def.sensitive)
        .filter_map(|(key, _)| config.config.get(key))
        .map(oxo_flow_core::config_impact::config_value_string)
        .collect()
}

fn print_config_change_summary(
    report: &ConfigChangeReport,
    old_snapshot: &HashMap<String, String>,
    config: &WorkflowConfig,
    sensitive_keys: &HashSet<String>,
    order: &[String],
    completed_in_run: usize,
    rerun: bool,
) {
    if rerun {
        return;
    }
    if report.is_legacy {
        eprintln!(
            "  {} checkpoint predates config tracking: recorded a baseline snapshot; \
             future config changes will invalidate affected rules automatically",
            "Note:".yellow()
        );
        return;
    }
    if report.changed_keys.is_empty()
        && report.added_keys.is_empty()
        && report.removed_keys.is_empty()
        && report.fingerprint_mismatches.is_empty()
    {
        return;
    }

    eprintln!("{}", "Config change:".bold().cyan());
    for key in &report.changed_keys {
        if sensitive_keys.contains(key) {
            eprintln!("  {key}: **** → ****");
        } else {
            let old = old_snapshot.get(key).map(String::as_str).unwrap_or("?");
            let new = config
                .config
                .get(key)
                .map(config_value_string)
                .unwrap_or_else(|| "?".to_string());
            eprintln!("  {key}: {old} → {new}");
        }
    }
    for key in &report.added_keys {
        eprintln!("  {key}: (new key)");
    }
    for key in &report.removed_keys {
        eprintln!("  {key}: (removed)");
    }
    if !report.fingerprint_mismatches.is_empty() {
        // Truncate the list — a cohort-wide shell edit can mismatch hundreds
        // of expanded rule instances (e.g. step5 × 100 samples).
        let shown: Vec<&str> = report
            .fingerprint_mismatches
            .iter()
            .take(3)
            .map(String::as_str)
            .collect();
        let extra = report.fingerprint_mismatches.len().saturating_sub(3);
        let suffix = if extra > 0 {
            format!(", … (+{extra} more)")
        } else {
            String::new()
        };
        eprintln!("  rule definition changed: {}{}", shown.join(", "), suffix);
    }

    let order_set: HashSet<&str> = order.iter().map(String::as_str).collect();
    let rerun_this_run = report
        .invalidated
        .iter()
        .filter(|name| order_set.contains(name.as_str()))
        .count();
    eprintln!(
        "  → invalidated {} ({} directly affected), re-running {}/{} this run, skipping {}",
        report.invalidated.len(),
        report.directly_affected.len(),
        rerun_this_run,
        order.len(),
        completed_in_run,
    );
}

/// Resolve a possibly-relative path against the current directory without
/// resolving symlinks. Checkpoint records use this so `resume` works from
/// any invocation directory (issue #68).
fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Long-form names of `oxo-flow run` flags (plus clap's global --help/
/// --version). Keep in sync with the `Run` variant in main.rs.
///
/// clap's trailing `config_overrides` positional (`allow_hyphen_values`)
/// cannot distinguish `--json` the run flag from `--json` a hyphen-value
/// once positional overrides start, so flags typed after `KEY=VALUE` land
/// in the override list (issue #71). Matching them here turns a confusing
/// "invalid config flag" error — or a silently wrong override — into
/// actionable guidance.
const RUN_FLAG_NAMES: &[&str] = &[
    "jobs",
    "keep-going",
    "workdir",
    "target",
    "retry",
    "timeout",
    "resume-failed",
    "profile",
    "max-threads",
    "max-memory",
    "skip-env-setup",
    "skip-ref-build",
    "cache-dir",
    "provenance",
    "bundle",
    "yes",
    "arg",
    "sample",
    "ai-recover",
    "ai-max-retries",
    "samples",
    "rerun",
    "json",
    "help",
    "version",
];

/// Short-form run flag names (same contract as [`RUN_FLAG_NAMES`]).
const RUN_SHORT_FLAGS: &[&str] = &["j", "k", "d", "t", "r", "h", "V"];

/// Validate a config value against its ConfigDef declaration.
fn validate_config_value(
    name: &str,
    value: &str,
    def: &oxo_flow_core::config::ConfigDef,
) -> anyhow::Result<()> {
    // ── choices ──
    if let Some(ref choices) = def.choices
        && !choices.iter().any(|c| c == value)
    {
        anyhow::bail!(
            "invalid value '{}' for config '{}'. Allowed: [{}]\n  {}",
            value,
            name,
            choices.join(", "),
            def.help.as_deref().unwrap_or("")
        );
    }

    // ── type validation ──
    if let Some(ref type_) = def.type_ {
        match type_.as_str() {
            "int" => {
                value.parse::<i64>().map_err(|_| {
                    anyhow::anyhow!(
                        "config '{}' expects an integer, got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    )
                })?;
            }
            "float" => {
                value.parse::<f64>().map_err(|_| {
                    anyhow::anyhow!(
                        "config '{}' expects a float, got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    )
                })?;
            }
            "bool" => {
                if value != "true" && value != "false" {
                    anyhow::bail!(
                        "config '{}' expects a boolean (true/false), got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    );
                }
            }
            "path" => {
                if value.is_empty() {
                    anyhow::bail!(
                        "config '{}' expects a path, got empty string\n  {}",
                        name,
                        def.help.as_deref().unwrap_or("")
                    );
                }
                // must_exist check
                if def.must_exist {
                    let p = std::path::Path::new(value);
                    if !p.exists() {
                        anyhow::bail!(
                            "config '{}' path does not exist: '{}'\n  {}",
                            name,
                            value,
                            def.help.as_deref().unwrap_or("")
                        );
                    }
                }
            }
            _ => { /* "string" or unknown — no validation */ }
        }
    }

    // ── range validation (requires type = int or float) ──
    if let Some(ref range) = def.range
        && let Some((min_str, max_str)) = range.split_once("..")
    {
        let min: f64 = min_str.trim().parse().map_err(|_| {
            anyhow::anyhow!("invalid range min '{}' for config '{}'", min_str, name)
        })?;
        let max: f64 = max_str.trim().parse().map_err(|_| {
            anyhow::anyhow!("invalid range max '{}' for config '{}'", max_str, name)
        })?;
        let val: f64 = value.parse().map_err(|_| {
            anyhow::anyhow!(
                "config '{}' has range '{range}' but value '{}' is not numeric\n  {}",
                name,
                value,
                def.help.as_deref().unwrap_or("")
            )
        })?;
        if val < min || val > max {
            anyhow::bail!(
                "config '{}' value {} is outside range {}..{}\n  {}",
                name,
                val,
                min,
                max,
                def.help.as_deref().unwrap_or("")
            );
        }
    }

    Ok(())
}

/// Remove entries in the environment cache that have not been touched for
/// `max_age_days` (issue #75; recursive since #194 C1 — subdirectories
/// were previously never aged out). Returns the number of entries removed.
fn cleanup_cache_dir(cache_dir: &std::path::Path, max_age_days: u64) -> usize {
    let max_age = std::time::Duration::from_secs(max_age_days * 24 * 3600);
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    let mut stack = vec![cache_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_file = path.is_file();
            if !is_file && !path.is_dir() {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let Ok(age) = now.duration_since(modified) else {
                continue;
            };
            if age <= max_age {
                // Young entries may still contain aged descendants; only
                // descend into directories that are themselves young.
                if !is_file {
                    stack.push(path);
                }
                continue;
            }
            let ok = if is_file {
                std::fs::remove_file(&path).is_ok()
            } else {
                std::fs::remove_dir_all(&path).is_ok()
            };
            if ok {
                removed += 1;
            }
        }
    }
    removed
}

#[allow(clippy::too_many_arguments)]
/// Parse CLI config overrides — the SAME accepted forms for `run` and
/// `dry-run` (issue #77 parity):
///   KEY=VALUE            direct positional form
///   --KEY=VALUE          long-flag form
///   --KEY VALUE          long-flag form with separate value (only for
///                        config keys the workflow DECLARES — an unknown
///                        `--token` is a typo'd command flag and must not
///                        be silently swallowed as an override, issue #71)
///   --arg KEY=VALUE      legacy `--arg` form (backward compatible)
fn parse_cli_overrides(
    cli_args: Vec<String>,
    declared_config_keys: &std::collections::HashSet<String>,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut cli_arg_values: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut cli_args_iter = cli_args.into_iter().peekable();
    while let Some(arg_str) = cli_args_iter.next() {
        // issue #71: run flags swallowed by the trailing overrides positional
        // get a targeted error instead of a confusing parse failure.
        let swallowed_flag = arg_str
            .strip_prefix("--")
            .map(|flag| flag.split('=').next().unwrap_or(flag))
            .filter(|name| RUN_FLAG_NAMES.contains(name))
            .map(|name| format!("--{name}"))
            .or_else(|| {
                arg_str
                    .strip_prefix('-')
                    .filter(|rest| !rest.starts_with('-'))
                    .map(|flag| flag.split('=').next().unwrap_or(flag))
                    .filter(|name| RUN_SHORT_FLAGS.contains(name))
                    .map(|name| format!("-{name}"))
            });
        if let Some(flag) = swallowed_flag {
            anyhow::bail!(
                "'{flag}' is a command flag, not a config override.\n  \
                 Command flags must come before KEY=VALUE overrides, e.g.:\n  \
                 oxo-flow run <workflow.oxoflow> --json min_quality=30\n  \
                 For a config key that itself starts with dashes, use --arg KEY=VALUE\n  \
                 (also placed before positional overrides)."
            );
        }
        let (k, v) = if let Some(eq) = arg_str.find('=') {
            let k = arg_str[..eq].trim_start_matches('-').to_string();
            (k, arg_str[eq + 1..].to_string())
        } else if let Some(k) = arg_str.strip_prefix("--") {
            if declared_config_keys.contains(k) {
                // `--KEY VALUE` — consume the next argument as the value
                let v = cli_args_iter.next().with_context(|| {
                    format!(
                        "invalid config flag: '{arg_str}' — expected --KEY=VALUE or --KEY VALUE"
                    )
                })?;
                (k.to_string(), v)
            } else {
                // issue #71 follow-up: a `--` token that is neither a
                // registered flag (caught above) nor a declared config key
                // is almost certainly a typo'd command flag (e.g.
                // `--config x`). Never silently swallow it as an override.
                anyhow::bail!(
                    "unknown argument '{arg_str}' — did you mean KEY=VALUE overrides?\n  \
                     Config overrides take KEY=VALUE (e.g. threads=8) or --KEY=VALUE; \
                     for a config key that itself starts with dashes, use --arg KEY=VALUE"
                );
            }
        } else {
            anyhow::bail!(
                "invalid config value format: '{arg_str}' — expected KEY=VALUE, --KEY=VALUE, or --KEY VALUE"
            );
        };
        if k.is_empty() || v.is_empty() {
            anyhow::bail!(
                "invalid config value format: '{arg_str}' — KEY and VALUE must be non-empty"
            );
        }
        cli_arg_values.insert(k, v);
    }
    Ok(cli_arg_values)
}

/// Apply parsed overrides and the defaults of declarative config entries
/// (`key = { default, required, … }` in `[config]`). Shared by `run` and
/// `dry-run` so preview and execution validate identically.
fn apply_cli_overrides(
    config: &mut WorkflowConfig,
    cli_arg_values: &std::collections::HashMap<String, String>,
) -> anyhow::Result<()> {
    for (name, cfg_def) in &config.config_meta {
        let effective_value = if let Some(val) = cli_arg_values.get(name) {
            validate_config_value(name, val, cfg_def)?;
            config
                .config
                .insert(name.clone(), toml::Value::String(val.clone()));
            val.clone()
        } else if let Some(ref default) = cfg_def.default {
            validate_config_value(name, default, cfg_def)?;
            config
                .config
                .entry(name.clone())
                .or_insert_with(|| toml::Value::String(default.clone()));
            default.clone()
        } else if cfg_def.required {
            let help_suffix = cfg_def
                .help
                .as_deref()
                .map(|h| format!("\n  {h}"))
                .unwrap_or_default();
            let display_name = if cfg_def.sensitive {
                format!("{name} (sensitive)")
            } else {
                name.clone()
            };
            anyhow::bail!(
                "required config '{display_name}' not set. Use --{name} <value>{help_suffix}"
            );
        } else {
            continue;
        };
        // Mask sensitive values in all non-execution output
        if cfg_def.sensitive {
            tracing::info!("config '{}' = **** (sensitive)", name);
        } else {
            tracing::debug!("config '{}' = '{}'", name, effective_value);
        }
    }
    // Inject undeclared KEY=VALUE values as {config.xxx} — a CLI override
    // must win over the workflow's [config] table (issue #62).
    for (k, v) in cli_arg_values {
        config
            .config
            .insert(k.clone(), toml::Value::String(v.clone()));
    }
    Ok(())
}

/// Quote a path for safe embedding in a reference build command (executed
/// via `bash -c` / `sh -c`): wrap in single quotes and escape embedded
/// quotes with the POSIX `'\''` sequence — the same idiom the environment
/// wrapper uses when embedding commands into `sh -c '...'` and the cluster
/// renderer uses for its `cd '...'`. Without the quoting, a source path
/// containing spaces or shell metacharacters would be spliced bare into the
/// command (issue #136 tier-2 audit).
fn quote_shell_path(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// Substitute the reference builder's `{source}` placeholder with the
/// expanded source path, shell-quoted so the path survives as one argument
/// (issue #136 tier-2 audit — the raw splice broke on spaces/metacharacters).
fn substitute_source_placeholder(build_cmd: &str, expanded_source: &str) -> String {
    build_cmd.replace("{source}", &quote_shell_path(expanded_source))
}

/// Render the "known modules" list for an unknown-module error. `run` and
/// `dry-run` share the phrasing — in particular the explicit empty-list
/// hint, so a workflow without includes cannot print a trailing "known
/// modules: " (issue #136 tier-2 audit).
fn known_modules_hint(module_rules: &std::collections::HashMap<String, Vec<String>>) -> String {
    if module_rules.is_empty() {
        "(none — no [[include]] modules)".to_string()
    } else {
        module_rules
            .keys()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Issue #142 H1 gate: an unknown `{config.*}` placeholder used to expand to
/// literal text while the run exited 0 — silent wrong outputs. This is the
/// same E005 detector `validate` uses, applied as a hard gate by both `run`
/// and `dry-run`, so the three surfaces can never disagree about a typo'd
/// key. Returns the human-readable findings (rule, key, fix) or empty.
fn undefined_config_findings(config: &WorkflowConfig) -> Vec<String> {
    config
        .rules
        .iter()
        .flat_map(|rule| oxo_flow_core::format::undefined_config_refs(rule, config))
        .map(|d| {
            format!(
                "rule '{}': {} ({})",
                d.rule.as_deref().unwrap_or("<unknown>"),
                d.message,
                d.suggestion.unwrap_or_default()
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn run_command(
    workflow: Option<PathBuf>,
    jobs: usize,
    keep_going: bool,
    workdir: Option<PathBuf>,
    log_file: Option<PathBuf>,
    target: Vec<String>,
    module: Vec<String>,
    retry: u32,
    timeout: String,
    resume_failed: bool,
    profile: Option<String>,
    max_threads: u32,
    max_memory: u64,
    skip_env_setup: bool,
    skip_ref_build: bool,
    cache_dir: Option<PathBuf>,
    provenance: bool,
    json: bool,
    cli_args: Vec<String>,
    ai_recover: bool,
    _ai_max_retries: Option<u32>,
    samples_filter: Vec<String>,
    rerun: bool,
    no_report_snapshot: bool,
    max_submitted: Option<usize>,
) -> Result<()> {
    print_banner();

    // `-j 0` means "no explicit concurrency limit": clamp once at the
    // boundary so every downstream consumer — the scheduler submit cap,
    // the run-loop semaphore, and the executor's own semaphore (which
    // would otherwise be a zero-permit gate that hangs the first
    // submission) — sees a consistent value (issue #136 fix 1).
    let jobs = jobs.max(1);

    // ── Repository-URL workflows (nextflow-style `run <repo>`) ──────────
    // `oxo-flow run gh:owner/repo[@ref]` (or a *.git URL / local repo dir)
    // checks out into <cwd>/.oxo-flow/repos/<name> (reused on later runs).
    // For run, @ref is a git branch/tag — never a Release asset (that is
    // pull's bundle namespace). Outputs/checkpoint default to the CURRENT
    // directory: data belongs outside the clone.
    let (workflow, from_repo) = match workflow {
        Some(w) => {
            let text = w.to_string_lossy();
            match crate::commands::pull::classify_run_source(&text) {
                Some(crate::commands::pull::RunSource::Repo { url, git_ref }) => {
                    let cache = crate::commands::pull::repo_cache_dir(&url, git_ref.as_deref())?;
                    let wf = crate::commands::pull::checkout_repo_workflow(
                        &url,
                        git_ref.as_deref(),
                        &cache,
                    )
                    .await?;
                    (wf, true)
                }
                None => (w, false),
            }
        }
        None => (resolve_workflow(None)?, false),
    };
    let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();
    // Workdir default: the workflow's own directory, EXCEPT for repository
    // runs, where the current directory holds the user's data (the clone is
    // a read-only cache).
    let workdir_default: PathBuf = if from_repo {
        std::env::current_dir().context("cannot determine current directory")?
    } else {
        workflow_dir.clone()
    };

    // ── Workdir lock (issue #70) ────────────────────────────────────────
    // Concurrent runs on the same workdir would race on
    // .oxo-flow/checkpoint.json (last-writer-wins). The exclusive lock is
    // acquired as early as possible and held for the whole run; the OS
    // releases it automatically if this process exits or crashes, so there
    // are no stale locks.
    let workdir_effective = workdir.as_ref().unwrap_or(&workdir_default);
    // Surface the lock error's suggestion (issue #158): without it a user
    // hitting a busy workdir learns WHAT is wrong but not that the lock
    // auto-releases when the other process exits.
    let _workdir_lock =
        WorkdirLock::acquire(workdir_effective).map_err(|e| match e.suggestion() {
            Some(s) => anyhow::anyhow!("{e}\n  hint: {s}"),
            None => anyhow::anyhow!("{e}"),
        })?;

    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // ── Parse and merge CLI config overrides (shared with dry-run) ─────
    // The workflow's own [config] keys gate the `--KEY VALUE` space form:
    // unknown `--` tokens are rejected as typos instead of silently
    // swallowed. config.config covers BOTH plain values (`key = "v"`) and
    // declarative entries (`key = { default = ... }`) — config_meta alone
    // only sees the latter.
    let declared_config_keys: std::collections::HashSet<String> =
        config.config.keys().cloned().collect();
    let cli_arg_values = parse_cli_overrides(cli_args, &declared_config_keys)?;

    apply_cli_overrides(&mut config, &cli_arg_values)?;

    // ── Filter to a sample subset (--samples @path / first:N / names / ready) ────
    // Runs after CLI overrides so `ready` resolution sees the final config
    // values in `{config.x}` paths (issue #63).
    if !samples_filter.is_empty()
        && let Some(readiness) = samples::apply_samples_filter(
            &mut config,
            &samples_filter,
            true,
            workdir.as_deref().unwrap_or(&workdir_default),
        )?
    {
        samples::print_readiness_section(&readiness);
    }

    // Load profile if specified and merge config values — the SAME shared
    // helper dry-run uses, so preview and execution can never drift apart.
    // Merged BEFORE apply_defaults so a profile's `[defaults]` table
    // actually reaches the rules (apply_defaults consumes self.defaults).
    if let Some(ref profile_name) = profile {
        crate::commands::run_preview::merge_profile(&mut config, profile_name, &workflow_dir)?;
    }

    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    // ── Undefined `{config.*}` gate (issue #142 H1) ───────────────────────
    // Runs AFTER expansion so engine-generated rules are covered too, and
    // before any file is touched — a typo'd key must fail the run, never
    // produce literal-placeholder outputs with exit 0.
    let e005 = undefined_config_findings(&config);
    if !e005.is_empty() {
        if json {
            emit_run_json_summary(json, "failed", &workflow, 0, 0, 0, 0, 0, vec![]);
        }
        return Err(anyhow::anyhow!(
            "workflow references undefined config variable(s) — fix before running:\n  {}",
            e005.join("\n  ")
        ));
    }

    let mut dag = WorkflowDag::from_rules_with_config(
        &config.rules,
        &config_placeholder_values(&config.config),
    )
    .context("failed to build workflow DAG")?;

    // --module partial runs (issue #112 elasticity): each module name
    // resolves to its rules plus the host producers of its declared
    // concrete inputs; upstream DAG dependents come via the target
    // machinery below.
    let mut target = target;
    for m in &module {
        match config.module_closure(m) {
            Some(names) => target.extend(names),
            None => {
                // Pre-execution abort: nothing ran, but the summary
                // contract still holds for --json (issue #142 H6).
                emit_run_json_summary(json, "failed", &workflow, 0, 0, 0, 0, 0, vec![]);
                return Err(anyhow::anyhow!(
                    "unknown module '{m}' — known modules: {}",
                    known_modules_hint(&config.module_rules)
                ));
            }
        }
    }
    // ── Run log + workflow provenance (issue #115 pillar 1) ────────────
    // Every run (and `resume`, which re-enters this function) archives its
    // own log under the workdir with numbered rotation; the header names
    // the exact workflow version (name, version, git HEAD) that produced
    // this record. Best-effort: a logging failure never fails the run.
    // Sensitive values must be computed BEFORE the run-log header is built:
    // the header embeds the raw command line, and `--arg KEY=secret` would
    // otherwise land in plaintext in the run log and its rotated backups
    // (issue #99 B1 / #136 fix 3). Computed once here; the executor's
    // capture masking below reuses the same list.
    let sensitive_values = sensitive_values_of(&config);
    let workflow_abs = absolutize(&workflow)?;
    let workflow_git_sha =
        oxo_flow_core::executor::checkpoint::CheckpointState::workflow_git_sha(&workflow_abs);
    let effective_workdir_log = workdir.as_ref().unwrap_or(&workdir_default);
    let run_log_path = match &log_file {
        // Relative --log-file paths resolve against the workdir, matching
        // the default log location (issue #136 fix 4).
        Some(p) if p.is_absolute() => p.clone(),
        Some(p) => effective_workdir_log.join(p),
        None => effective_workdir_log.join(".oxo-flow/logs/oxo-flow.log"),
    };
    let run_log_header = format!(
        "oxo-flow run log\nstarted_at: {}\noxo-flow: v{}\ncommand: {}\nworkflow: {}\nworkflow_name: {}\nworkflow_version: {}\ngit_sha: {}\nworkdir: {}\n\n",
        chrono::Local::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION"),
        oxo_flow_core::executor::process::mask_sensitive(
            &std::env::args().collect::<Vec<_>>().join(" "),
            &sensitive_values,
        ),
        workflow_abs.display(),
        config.workflow.name,
        config.workflow.version,
        workflow_git_sha
            .as_deref()
            .unwrap_or("(not inside a git repository)"),
        effective_workdir_log.display(),
    );
    // Background runs (issue #194 A3): `spawn_detached` already redirected
    // this process's stdout/stderr onto the run log, so arming the tracing
    // tee here would write every event TWICE into the same file. In that
    // mode the header is printed to stderr (→ the redirect) and the tee is
    // skipped entirely; foreground runs arm the tee as before.
    let _run_log_guard = if std::env::var_os("OXO_FLOW_STDERR_ALREADY_REDIRECTED").is_some() {
        eprintln!("{run_log_header}");
        None
    } else {
        match crate::logging::activate_run_log(&run_log_path, &run_log_header) {
            Ok(guard) => Some(guard),
            Err(e) => {
                tracing::warn!(error = %e, path = %run_log_path.display(), "failed to open run log; continuing without file logging");
                None
            }
        }
    };
    tracing::info!(
        workflow = %config.workflow.name,
        version = %config.workflow.version,
        git_sha = workflow_git_sha.as_deref().unwrap_or("(none)"),
        workdir = %effective_workdir_log.display(),
        "workflow run started (log: {})",
        run_log_path.display()
    );

    let mut order = if target.is_empty() {
        dag.execution_order()?
    } else {
        let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
        dag.execution_order_for_targets(&target_refs)
            .with_context(|| "failed to resolve target rules")?
    };
    emit_execution_event(oxo_flow_core::executor::ExecutionEvent::WorkflowStarted {
        workflow_name: config.workflow.name.clone(),
        total_rules: order.len(),
    });
    eprintln!(
        "{} {} rules in execution order",
        "DAG:".bold().green(),
        order.len()
    );

    for (i, rule_name) in order.iter().enumerate() {
        eprintln!("  {}. {}", i + 1, rule_name);
    }

    // ── Load checkpoint + config-change impact analysis (issue #62) ─────
    // Detection must happen AFTER overrides/profile merges and wildcard
    // expansion (config.config is final) and BEFORE ExecutorConfig
    // construction (the invalidation set must reach the executor's
    // freshness gate, which would otherwise silently skip re-submitted
    // rules with stale outputs).
    let checkpoint_path = workdir
        .as_ref()
        .unwrap_or(&workdir_default)
        .join(".oxo-flow/checkpoint.json");
    // Retention for failed-output aside files (issue #194 C2): stale
    // `.oxo-failed` evidence ages out at run start instead of accumulating
    // forever. Best-effort — cleanup must never block the run.
    let stale_asides = oxo_flow_core::executor::output_invalidation::cleanup_stale_failed_asides(
        workdir.as_ref().unwrap_or(&workdir_default),
        oxo_flow_core::executor::output_invalidation::OXOX_FAILED_RETENTION_DAYS,
    );
    if stale_asides > 0 {
        tracing::info!(
            count = stale_asides,
            "removed stale .oxo-failed aside files (retention)"
        );
    }

    let checkpoint: Arc<Mutex<CheckpointState>> = if checkpoint_path.exists() {
        Arc::new(Mutex::new(
            CheckpointState::load_from_file(&checkpoint_path).unwrap_or_default(),
        ))
    } else {
        Arc::new(Mutex::new(CheckpointState::default()))
    };

    // Store workflow path and working directory in the checkpoint so
    // `resume` re-runs from the same place (issue #68). Both are made
    // absolute — a raw relative path would only resolve from the original
    // invocation directory.
    {
        let mut ck = checkpoint.lock().await;
        ck.set_workflow_path(&workflow_abs);
        // Record the workflow repository's HEAD SHA for provenance
        // (issue #115 pillar 1) — best-effort, never fails the run.
        if let Some(sha) = &workflow_git_sha {
            ck.set_workflow_git_sha(sha.clone());
        }
        ck.set_workdir(&absolutize(workdir.as_deref().unwrap_or(&workdir_default))?);
    }

    // Changed config keys → only rules referencing them (plus DAG downstream)
    // are invalidated; edited rule definitions are caught by fingerprints.
    // Engine-injected keys (samples_list, samples_<group>) are excluded:
    // their --samples churn must not invalidate everything.
    let sensitive_keys: std::collections::HashSet<String> = config
        .config_meta
        .iter()
        .filter(|(_, def)| def.sensitive)
        .map(|(key, _)| key.clone())
        .collect();
    let old_snapshot = {
        let ck = checkpoint.lock().await;
        ck.config_snapshot.clone()
    };
    let (change_report, mut force_rules, completed_in_run) = {
        let mut ck = checkpoint.lock().await;
        let report = oxo_flow_core::config_impact::detect_config_changes(
            &mut ck,
            &config.rules,
            &dag,
            &config.config,
            &sensitive_keys,
            &config.workflow.interpreter_map,
            config.defaults.shell_prelude.as_deref(),
        );
        let force_rules: std::collections::HashSet<String> =
            report.invalidated.iter().cloned().collect();
        let completed_in_run = order
            .iter()
            .filter(|name| ck.completed_rules.contains(*name))
            .count();
        // Eager save: the invalidation is correct regardless of whether this
        // run proceeds (budget bail, crash, a `when` condition flipping to
        // false). Persisting now makes change detection idempotent across
        // aborted runs and stabilizes the `when`-false state on disk.
        if let Err(e) = ck.save_to_file(&checkpoint_path) {
            tracing::warn!(error = %e, "failed to save checkpoint after config-change detection");
        }
        (report, force_rules, completed_in_run)
    };
    print_config_change_summary(
        &change_report,
        &old_snapshot,
        &config,
        &sensitive_keys,
        &order,
        completed_in_run,
        rerun,
    );

    // Issue #142 M1: rules whose fingerprint differed only in the
    // sample-derived input list are NOT invalidated — toggling --samples
    // must not re-run (and overwrite) cohort-level gather outputs. The
    // input-manifest check re-verifies set + content below, so a genuine
    // input edit still invalidates there.
    if !change_report.sample_selection_exempt.is_empty() {
        eprintln!(
            "  {} skipped {} rule(s) whose definition only changed with the --samples selection: {} — outputs still cover the previous run's full sample set; use --rerun to regenerate with the new selection",
            "⚠".yellow(),
            change_report.sample_selection_exempt.len(),
            change_report.sample_selection_exempt.join(", ")
        );
    }

    // All config values (including CLI --arg overrides) become {config.key} in templates.
    let wildcard_values: Arc<HashMap<String, String>> =
        Arc::new(config_placeholder_values(&config.config));
    let workdir_actual = Arc::new(workdir.as_ref().unwrap_or(&workdir_default).clone());

    // ── Pre-flight disk check (issue #75, wired 2026-08-23) ────────────────
    // ENOSPC mid-run is the classic silent-loss incident (live: TMPDIR full
    // killed a scatter campaign half-way). Warn when either the workdir or
    // the temp dir has less than 1 GB free BEFORE any rule runs.
    for (label, path) in [
        ("workdir", workdir_actual.as_path()),
        ("temp dir", std::env::temp_dir().as_path()),
    ] {
        if let Some(free_kb) = free_kilobytes(path) {
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

    // ── Input manifest comparison (issue #72) ──────────────────────────────
    // A completed rule is reused only when the file set its inputs resolved
    // to at completion time (paths + size + mtime) still matches. Globs and
    // Dir inputs detect added/removed files; plain files detect edits. A
    // mismatch invalidates the rule and its DAG downstream (the same cascade
    // as config changes); legacy checkpoints (no manifest recorded) adopt
    // the current set as a one-time baseline — the same policy as the config
    // snapshot (issue #62).
    {
        let mut ck = checkpoint.lock().await;
        // Shared with dry-run's read-only preview (issue #66) — keep the
        // detection semantics in ONE place.
        let (mismatched, missing_inputs, baselined, sample_selection_driven) =
            crate::commands::run_preview::detect_input_manifest_invalidations(
                &mut ck,
                &config,
                &dag,
                &order,
                workdir_actual.as_ref(),
                &wildcard_values,
            );
        // Issue #142 M1: a rule whose only input-manifest change is the
        // engine-injected sample list is NOT invalidated — toggling
        // --samples must not overwrite cohort-level gather outputs with a
        // subset table. Emit the documented warning; the detection function
        // already proved (re-expansion + content-identity) the change is
        // sample-selection-only.
        if !sample_selection_driven.is_empty() {
            let names: Vec<&str> = sample_selection_driven.iter().map(String::as_str).collect();
            eprintln!(
                "  {} skipped {} rule(s) whose inputs only changed with the --samples selection: {} — outputs still cover the previous run's full sample set; use --rerun to regenerate with the new selection",
                "⚠".yellow(),
                names.len(),
                names.join(", ")
            );
        }
        // Cascade-up: missing inputs (tombstoned temporaries) re-run their
        // completed producers first — the same semantics the preview shows.
        if !missing_inputs.is_empty() {
            let upstream = crate::commands::run_preview::cascade_up(&mut ck, &dag, &missing_inputs);
            force_rules.extend(upstream.iter().cloned());
            eprintln!(
                "  {} missing intermediate inputs — re-running {} producer rule(s): {}",
                "↻".yellow(),
                upstream.len(),
                upstream.join(", ")
            );
        }
        if !mismatched.is_empty() {
            let invalidated = crate::commands::run_preview::invalidate_with_downstream(
                &mut ck,
                &dag,
                &mismatched,
            );
            force_rules.extend(invalidated.iter().cloned());
            eprintln!(
                "  {} input changes invalidated {} rule(s): {}",
                "↻".yellow(),
                invalidated.len(),
                invalidated.join(", ")
            );
        }
        if baselined > 0 {
            eprintln!(
                "  Note: checkpoint predates input tracking: recorded baseline input manifests for {} completed rule(s); future input changes will invalidate them automatically",
                baselined
            );
        }
        if (!mismatched.is_empty() || baselined > 0)
            && let Err(e) = ck.save_to_file(&checkpoint_path)
        {
            tracing::warn!(error = %e, "failed to save checkpoint after input-manifest detection");
        }
    }

    // Tombstone-aware skip (temporary rules): a tombstoned rule whose
    // outputs are deleted stays SKIPPED while no dependent needs them;
    // when a dependent will run, cascade-up has already removed it from
    // completed so it re-executes first. Mirrored by the dry-run preview.
    let tombstone_keep: std::collections::HashSet<String> = {
        let ck = checkpoint.lock().await;
        ck.tombstones
            .keys()
            .filter(|rule| {
                dag.dependents(rule)
                    .map(|deps| deps.iter().all(|d| ck.completed_rules.contains(d)))
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    };

    // Lazy regeneration (temporary rules): any rule that will execute may
    // consume the tombstoned outputs of completed producers — cascade up so
    // those producers re-execute first. Mirrored by the dry-run preview.
    if !rerun {
        let will_run_seeds: std::collections::HashSet<String> = {
            let ck = checkpoint.lock().await;
            order
                .iter()
                .filter(|name| {
                    !ck.completed_rules.contains(*name)
                        || (!tombstone_keep.contains(*name)
                            && !config.get_rule(name).is_some_and(|rule| {
                                crate::commands::run_preview::when_condition_false(
                                    rule,
                                    &config,
                                    &wildcard_values,
                                )
                            })
                            && config.get_rule(name).is_some_and(|rule| {
                                !crate::commands::run_preview::rule_outputs_exist(
                                    rule,
                                    workdir_actual.as_ref(),
                                    &wildcard_values,
                                )
                            }))
                })
                .cloned()
                .collect()
        };
        let upstream = {
            let mut ck = checkpoint.lock().await;
            crate::commands::run_preview::cascade_up_tombstoned(&mut ck, &dag, &will_run_seeds)
        };
        if !upstream.is_empty() {
            force_rules.extend(upstream.iter().cloned());
            eprintln!(
                "  {} temporary outputs needed again — re-running {} producer rule(s): {}",
                "↻".yellow(),
                upstream.len(),
                upstream.join(", ")
            );
        }
    }

    // indicatif's stderr draw target auto-hides when stderr is not a terminal,
    // which makes every per-rule progress message silently disappear under pipes,
    // redirects, nohup, CI, or schedulers. When that happens, fall back to plain
    // eprintln lines so the run is never silent.
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    // Background runs redirect stderr onto the run log (issue #194 A3):
    // the bar's redraw sequences would land in the file as ANSI garbage.
    let stderr_redirected = std::env::var_os("OXO_FLOW_STDERR_ALREADY_REDIRECTED").is_some();

    let progress = indicatif::ProgressBar::new(order.len() as u64);
    progress.set_style(
        indicatif::ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ETA:{eta} ({msg})",
            )?
            .progress_chars("#>-"),
    );
    if !is_tty || stderr_redirected {
        // Draw into the void instead of stderr: no bar output, but every
        // set_message/set_position call stays valid (message lines are
        // printed separately via progress_narrate when !is_tty).
        progress.set_draw_target(indicatif::ProgressDrawTarget::hidden());
    }

    let timeout_secs: u64 = if timeout == "0" {
        0
    } else if let Ok(n) = timeout.parse::<u64>() {
        n
    } else {
        parse_duration_secs(&timeout).unwrap_or_else(|| {
            eprintln!(
                "{} Invalid timeout format '{}', defaulting to no timeout",
                "Warning:".bold().yellow(),
                timeout
            );
            0
        })
    };

    // ── Cluster path (issue #74 phase 2) ───────────────────────────────────
    // A `[cluster]` block in effect plus a named profile routes execution to
    // a scheduler instead of the local executor. Both conditions are
    // required: a workflow carrying `[cluster]` keeps running locally until
    // the user opts in with `--profile`, so no existing workflow starts
    // submitting jobs because of this change.
    if profile.is_some()
        && let Some(cluster_profile) = config.cluster.clone()
    {
        let summary = crate::commands::run_cluster::run_on_cluster(
            &cluster_profile,
            crate::commands::run_cluster::ClusterRunArgs {
                config: &config,
                dag: &dag,
                order: &order,
                checkpoint: &checkpoint,
                checkpoint_path: &checkpoint_path,
                workdir: workdir_actual.as_ref(),
                wildcard_values: wildcard_values.as_ref(),
                sensitive_keys: &sensitive_keys,
                sensitive_values: &sensitive_values,
                force_rules: &force_rules,
                max_submitted,
                rerun,
                resume_failed,
                // The cluster path honors --cache-dir (shared env cache);
                // --skip-env-setup/--ai-recover are unsupported there and
                // warned about at submit time (issue #136 tier-2 audit).
                cache_dir: cache_dir.clone(),
                skip_env_setup,
                ai_recover,
            },
        )
        .await?;
        // The cluster path returns before the common summary emission, so
        // it routes through the same `--json` contract itself (issue #142
        // H6): the document must appear on both outcomes.
        emit_run_json_summary(
            json,
            if summary.is_success() {
                "completed"
            } else {
                "failed"
            },
            &workflow,
            summary.succeeded,
            summary.skipped,
            summary.failed,
            summary.non_required_failed,
            0,
            // Cluster per-rule resources come from sacct accounting at
            // report time, not the live checkpoint — empty here.
            vec![],
        );
        if !summary.is_success() {
            return Err(anyhow::anyhow!("workflow execution failed"));
        }
        // Report snapshot parity with the local path (issue #83 P1-14):
        // a cluster run leaves the same `.oxo-flow/reports/` artifacts as a
        // local one. Errors are warnings, exactly like the local call site.
        if !no_report_snapshot {
            let ck = checkpoint.lock().await;
            if let Err(e) =
                crate::commands::output::snapshot_report(&workflow, &workdir_actual, &ck)
            {
                eprintln!("  {} Report snapshot failed: {e}", "⚠".yellow());
            }
        }
        return Ok(());
    }

    let exec_config = ExecutorConfig {
        max_jobs: jobs,
        dry_run: false,
        workdir: workdir.clone().unwrap_or_else(|| workdir_default.clone()),
        sensitive_values: sensitive_values.clone(),
        shell_prelude: config.defaults.shell_prelude.clone(),
        keep_going,
        retry_count: retry,
        timeout: if timeout_secs > 0 {
            Some(std::time::Duration::from_secs(timeout_secs))
        } else {
            None
        },
        max_threads: if max_threads > 0 {
            Some(max_threads)
        } else {
            None
        },
        max_memory_mb: if max_memory > 0 {
            Some(max_memory)
        } else {
            None
        },
        force_rerun: rerun,
        // Rules invalidated by config-change analysis bypass the executor's
        // mtime freshness gate so their stale outputs are actually rebuilt.
        force_rules: force_rules.clone(),
        resource_groups: config
            .resource_groups
            .iter()
            .map(|(k, v)| (k.clone(), v.max))
            .collect(),
        skip_env_setup,
        cache_dir: cache_dir.clone(),
        interpreter_map: config.workflow.interpreter_map.clone(),
        // Shared with the manifest snapshot resolver so staging and
        // invalidation always see the same backends (issue #80 item 2).
        storage_resolver: crate::commands::run_preview::storage_resolver(),
        // The freshness gate reads recorded provenance checksums from the
        // live checkpoint (issue #194 B2).
        checkpoint: Some(checkpoint.clone()),
    };

    // Fail fast if any rule's declared request can never fit an explicit
    // --max-memory / --max-threads cap. Otherwise the run would execute earlier
    // rules and only discover the impossible one mid-pipeline.
    let scheduled: Vec<&oxo_flow_core::rule::Rule> = order
        .iter()
        .filter_map(|name| config.get_rule(name))
        .collect();
    let breaches = oxo_flow_core::scheduler::check_budget_feasibility(
        &scheduled,
        exec_config.max_threads,
        exec_config.max_memory_mb,
    );
    if !breaches.is_empty() {
        progress.finish_and_clear();
        let detail = breaches
            .iter()
            .map(|b| format!("  - {b}"))
            .collect::<Vec<_>>()
            .join("\n");
        // Pre-execution abort: nothing ran — the summary still reports
        // the failed run for --json consumers (issue #142 H6).
        emit_run_json_summary(json, "failed", &workflow, 0, 0, 0, 0, 0, vec![]);
        return Err(anyhow::anyhow!(
            "resource budget too small for {} rule(s); no rules were run:\n{}",
            breaches.len(),
            detail
        ));
    }

    // Disk pre-flight (issue #75): warn when a rule's declared
    // `resources.disk` exceeds the free space in the workdir — a long run
    // should not discover this mid-pipeline.
    let disk_warnings = oxo_flow_core::scheduler::validate_disk_requirements(
        &scheduled.iter().map(|r| (*r).clone()).collect::<Vec<_>>(),
        workdir_actual.as_ref(),
    );
    for warning in &disk_warnings {
        eprintln!("  {} {}", "Warning:".bold().yellow(), warning);
    }

    // Same cache dir the executor's EnvironmentResolver uses — reference
    // builds that declare an environment share the env cache with rules.
    let env_cache_dir = exec_config
        .cache_dir
        .clone()
        .unwrap_or_else(|| workdir_actual.as_ref().join(".oxo-flow").join("env-cache"));
    let executor = Arc::new(LocalExecutor::new(exec_config));
    let success_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // Failures of `required = false` rules (issue #99 B2): counted
    // separately, surfaced in the summary, but exempt from failing the run.
    let non_required_fail_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let skipped_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed_rules_set: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));
    let failures: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    // Rules that never ran because an upstream dependency failed, paired with the
    // dependency that blocked them. Reported separately from genuine failures so
    // the root cause stays distinguishable from the fallout.
    let blocked: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

    // When --resume-failed is set, clear failed rules from checkpoint so they re-execute.
    if resume_failed && checkpoint_path.exists() {
        let mut ck = checkpoint.lock().await;
        let failed_count = ck.failed_rules.len();
        let completed_count = ck.completed_rules.len();
        ck.failed_rules.clear();
        eprintln!(
            "{} Resuming {} completed, re-running {} failed rules",
            "Resume:".bold().cyan(),
            completed_count,
            failed_count
        );
    }

    // ── Auto-build references (indexes, data files) ──────────────────────

    if !skip_ref_build && !config.references.is_empty() {
        let ref_workdir = workdir.as_ref().unwrap_or(&workdir_default);
        let ref_checkpoint = checkpoint_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("reference-checkpoint.json");
        // Migration: versions before this fix wrote the state one level
        // deeper (the checkpoint parent already ends in `.oxo-flow`, so
        // the old join produced `.oxo-flow/.oxo-flow/`). Move the old file
        // over once so stored fingerprints survive the upgrade.
        let legacy_ref_checkpoint = ref_checkpoint
            .parent()
            .unwrap_or(Path::new("."))
            .join(".oxo-flow/reference-checkpoint.json");
        if !ref_checkpoint.exists() && legacy_ref_checkpoint.exists() {
            let _ = std::fs::rename(&legacy_ref_checkpoint, &ref_checkpoint);
            // The rename runs under the workdir lock (acquired above); the
            // now-empty nested directory is removed so no shadow path stays
            // behind for a future reader to mistake for the real one.
            if let Some(parent) = legacy_ref_checkpoint.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
        // name → fingerprint. Legacy checkpoints store a plain JSON array of
        // names; those entries are adopted with the current fingerprint
        // (no rebuild) — the same one-time window as rule config snapshots.
        let mut ref_state: HashMap<String, RefRecord> = if ref_checkpoint.exists() {
            let raw = std::fs::read_to_string(&ref_checkpoint).unwrap_or_default();
            serde_json::from_str::<HashMap<String, RefRecord>>(&raw)
                .ok()
                .or_else(|| {
                    // Legacy formats: a plain name→fingerprint map, or a
                    // bare name list — adopted with no content signature.
                    serde_json::from_str::<HashMap<String, String>>(&raw)
                        .ok()
                        .map(|m| {
                            m.into_iter()
                                .map(|(k, v)| {
                                    (
                                        k,
                                        RefRecord {
                                            fingerprint: v,
                                            content_sig: None,
                                            source_path: None,
                                        },
                                    )
                                })
                                .collect()
                        })
                })
                .or_else(|| {
                    serde_json::from_str::<Vec<String>>(&raw)
                        .map(|names| {
                            names
                                .into_iter()
                                .map(|n| {
                                    (
                                        n,
                                        RefRecord {
                                            fingerprint: String::new(),
                                            content_sig: None,
                                            source_path: None,
                                        },
                                    )
                                })
                                .collect::<HashMap<String, RefRecord>>()
                        })
                        .ok()
                })
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        // Artifacts actually rebuilt in THIS run — consumers (rules whose
        // declared inputs match) are invalidated afterwards.
        let mut rebuilt_outputs: Vec<String> = Vec::new();

        for ref_def in &config.references {
            let output_path = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                &ref_def.output,
                &wildcard_values,
            );
            let output_full = ref_workdir.join(&output_path);
            // Resolved once: the fingerprint guards the source CONTENT
            // (issue #97) and the freshness check below guards mtime vs
            // the output — both need the same workdir-joined path.
            let resolved_source = ref_def.source.as_ref().map(|source| {
                ref_workdir.join(oxo_flow_core::executor::checkpoint::expand_config_in_path(
                    source,
                    &wildcard_values,
                ))
            });
            let current_fp = oxo_flow_core::config_impact::reference_fingerprint(
                ref_def,
                &config.config,
                resolved_source.as_deref(),
            );
            let stored: Option<RefRecord> = ref_state.get(&ref_def.name).cloned();
            // Content signature of the CURRENT source (size + full content
            // hash for small files). Equality with the stored signature
            // means the source is a byte-identical copy at a new path — the
            // fingerprint differs only in the path string, so a rebuild
            // would reproduce identical output (issue #142 follow-up).
            let current_sig = resolved_source.as_deref().and_then(source_content_sig);

            // Decide whether the artifact must be (re)built. The freshness
            // check comes FIRST so an mtime-only touch reports the accurate
            // reason; a fingerprint mismatch then covers definition edits,
            // referenced-config changes, and content changes the freshness
            // check cannot see (same-path, timestamp-preserving rewrites).
            let source_newer = resolved_source.as_deref().is_some_and(|p| {
                p.exists() && oxo_flow_core::executor::checkpoint::file_is_newer(p, &output_full)
            });
            let rebuild_reason = if !output_full.exists() {
                Some("output missing")
            } else if stored.as_ref().is_some_and(|r| r.fingerprint.is_empty()) {
                // Legacy entry: adopt the current fingerprint without
                // rebuilding — and PERSIST the adoption. An in-memory-only
                // adopt would leave the entry "" forever: every future run
                // would silently re-adopt the CURRENT source state and the
                // content guard could never engage.
                ref_state.insert(
                    ref_def.name.clone(),
                    RefRecord {
                        fingerprint: current_fp.clone(),
                        content_sig: current_sig.clone(),
                        source_path: ref_def.source.clone(),
                    },
                );
                if let Some(parent) = ref_checkpoint.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(
                    &ref_checkpoint,
                    serde_json::to_string(&ref_state).unwrap_or_default(),
                );
                None
            } else if source_newer {
                Some("source is newer than output")
            } else if stored.as_ref().is_some_and(|r| r.fingerprint != current_fp) {
                // The fingerprint changed. One benign cause: a pure PATH
                // migration — the same byte content now lives at a new
                // location (live: --arg index=/new/path where /new/path is
                // a content copy). PROOF of purity: re-fingerprinting the
                // definition with the STORED source path must reproduce the
                // stored fingerprint (so the build/output/config did NOT
                // change), AND the content signatures must match with a
                // full hash component (size-only is too weak).
                let pure_path_migration = (|| {
                    let rec = stored.as_ref()?;
                    let old_path_str = rec.source_path.as_deref()?;
                    let old_def = oxo_flow_core::config::ReferenceDef {
                        source: Some(old_path_str.to_string()),
                        ..ref_def.clone()
                    };
                    let fp_at_old_path = oxo_flow_core::config_impact::reference_fingerprint(
                        &old_def,
                        &config.config,
                        Some(ref_workdir.join(old_path_str)).as_deref(),
                    );
                    if fp_at_old_path != rec.fingerprint {
                        return None; // definition or config really changed
                    }
                    let cur = current_sig.as_deref()?;
                    if !cur.contains("|hash:") {
                        return None; // no strong hash — never exempt
                    }
                    (rec.content_sig.as_deref() == Some(cur)).then_some(())
                })()
                .is_some();
                if pure_path_migration {
                    tracing::info!(
                        reference = %ref_def.name,
                        "reference source moved to a new path with identical content — keeping the existing artifact"
                    );
                    ref_state.insert(
                        ref_def.name.clone(),
                        RefRecord {
                            fingerprint: current_fp.clone(),
                            content_sig: current_sig.clone(),
                            source_path: ref_def.source.clone(),
                        },
                    );
                    if let Some(parent) = ref_checkpoint.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(
                        &ref_checkpoint,
                        serde_json::to_string(&ref_state).unwrap_or_default(),
                    );
                    None
                } else {
                    Some("definition, source content, or referenced config changed")
                }
            } else {
                None
            };

            if let Some(reason) = rebuild_reason {
                let synthetic_rule = oxo_flow_core::rule::Rule {
                    name: format!("ref:{}", ref_def.name),
                    // `{input}` is the renderer's alias for the reference
                    // source (documented in config_impact::reference_fingerprint).
                    input: ref_def
                        .source
                        .as_deref()
                        .map(|s| vec![s.to_string()].into())
                        .unwrap_or_default(),
                    output: vec![output_path.clone()].into(),
                    ..Default::default()
                };
                let mut build_cmd = oxo_flow_core::executor::process::render_shell_command(
                    &ref_def.build,
                    &synthetic_rule,
                    &wildcard_values,
                    oxo_flow_core::scheduler::detect_system_limits(),
                );
                // `{source}` is the builder-template spelling of the same
                // thing; render it too (live evidence: tcasia's STAR
                // genomeGenerate died with 'could not open genomeFastaFile:
                // {source}' — the placeholder was never substituted). The
                // path is shell-quoted — a bare splice breaks reference
                // builds whose source path contains spaces (issue #136
                // tier-2 audit).
                if let Some(source) = ref_def.source.as_deref() {
                    let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                        source,
                        &wildcard_values,
                    );
                    build_cmd = substitute_source_placeholder(&build_cmd, &expanded);
                }
                // References take the same shell prelude as rules (issue #92),
                // before the environment wrapper resolves.
                build_cmd = config.defaults.apply_shell_prelude(&build_cmd);
                // References that need workflow tools (bowtie2-build, STAR
                // genomeGenerate, …) declare an `environment` — same spec as
                // `[rules.environment]`. The env is created on first use and
                // the build command runs inside it; without one, the build
                // runs in the bare system shell as before.
                if let Some(ref env) = ref_def.environment
                    && !env.is_empty()
                {
                    let resolver = oxo_flow_core::environment::EnvironmentResolver::with_cache_dir(
                        &env_cache_dir,
                    );
                    let key = resolver.cache_key(env);
                    if !resolver.cache_is_ready(&key).await {
                        let setup = resolver.setup_command(env)?;
                        let out = tokio::process::Command::new("sh")
                            .arg("-c")
                            .arg(&setup)
                            .current_dir(ref_workdir)
                            .output()
                            .await;
                        match out {
                            Ok(o) if o.status.success() => {
                                resolver.cache_mark_ready(&key).await;
                            }
                            Ok(o) => {
                                let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
                                // Pre-execution abort — the summary still
                                // reports the failed run (issue #142 H6).
                                emit_run_json_summary(
                                    json,
                                    "failed",
                                    &workflow,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    vec![],
                                );
                                return Err(anyhow::anyhow!(
                                    "failed to set up the environment for reference '{}': {}",
                                    ref_def.name,
                                    stderr.trim()
                                ));
                            }
                            Err(e) => {
                                emit_run_json_summary(
                                    json,
                                    "failed",
                                    &workflow,
                                    0,
                                    0,
                                    0,
                                    0,
                                    0,
                                    vec![],
                                );
                                return Err(anyhow::anyhow!(
                                    "failed to run the environment setup for reference '{}': {e}",
                                    ref_def.name
                                ));
                            }
                        }
                    }
                    build_cmd = resolver.wrap_command(&build_cmd, env, None, ref_workdir)?;
                }
                eprintln!(
                    "  {} Building {}: {} ({})",
                    "⚙".cyan().bold(),
                    ref_def.name,
                    ref_def.description.as_deref().unwrap_or(&ref_def.output),
                    reason
                );
                // bash first, sh fallback — mirrors the rule executor
                // (spawn_rule_shell): `set -o pipefail` in a shell prelude
                // is invalid under dash, which /bin/sh is on Debian-family
                // systems (review finding on issue #92).
                let spawn_ref_build = |shell: &str| {
                    std::process::Command::new(shell)
                        .arg("-c")
                        .arg(&build_cmd)
                        .current_dir(ref_workdir)
                        .status()
                };
                let status = match spawn_ref_build("bash") {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => spawn_ref_build("sh"),
                    other => other,
                };
                match status {
                    Ok(s) if s.success() => {
                        // Store the POST-build fingerprint: the decision
                        // fingerprint above was computed before the build
                        // ran, and a build that creates or rewrites its own
                        // source (download-then-index) changes the source
                        // state DURING the build — storing the pre-build
                        // state would mismatch on every subsequent run and
                        // rebuild forever.
                        let post_fp = oxo_flow_core::config_impact::reference_fingerprint(
                            ref_def,
                            &config.config,
                            resolved_source.as_deref(),
                        );
                        ref_state.insert(
                            ref_def.name.clone(),
                            RefRecord {
                                fingerprint: post_fp,
                                content_sig: resolved_source
                                    .as_deref()
                                    .and_then(source_content_sig),
                                source_path: ref_def.source.clone(),
                            },
                        );
                        rebuilt_outputs.push(output_path.clone());
                        if let Some(parent) = ref_checkpoint.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(
                            &ref_checkpoint,
                            serde_json::to_string(&ref_state).unwrap_or_default(),
                        );
                        eprintln!("    {} {} built", "✓".green(), ref_def.name);
                    }
                    Ok(s) => {
                        anyhow::bail!(
                            "failed to build reference '{}' (exit {}). Command: {}",
                            ref_def.name,
                            s.code().unwrap_or(-1),
                            oxo_flow_core::executor::process::mask_sensitive(
                                &build_cmd,
                                &sensitive_values
                            )
                        );
                    }
                    Err(e) => {
                        anyhow::bail!("failed to run build command for '{}': {}", ref_def.name, e);
                    }
                }
            } else if stored.is_none() {
                // Output exists but not tracked — adopt as built.
                ref_state.insert(
                    ref_def.name.clone(),
                    RefRecord {
                        fingerprint: current_fp,
                        content_sig: current_sig,
                        source_path: ref_def.source.clone(),
                    },
                );
                if let Some(parent) = ref_checkpoint.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(
                    &ref_checkpoint,
                    serde_json::to_string(&ref_state).unwrap_or_default(),
                );
            }
        }

        // A rebuilt reference invalidates the rules that consume it through
        // declared inputs (plus their DAG downstream). The checkpoint
        // pre-process would otherwise skip them as "already completed"
        // without consulting input freshness; after invalidation the
        // executor's mtime gate sees the rebuilt artifact and re-runs them.
        // Shell-only reads of a reference cannot be tracked — declare
        // reference artifacts in a rule's `input` list.
        if !rebuilt_outputs.is_empty() {
            let mut consumers: HashSet<String> = HashSet::new();
            for rule in &config.rules {
                let inputs: Vec<String> = rule
                    .input
                    .to_vec()
                    .into_iter()
                    .map(|i| {
                        oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            &i,
                            &wildcard_values,
                        )
                    })
                    .collect();
                if inputs.iter().any(|input| {
                    !input.contains('{') && rebuilt_outputs.iter().any(|rebuilt| rebuilt == input)
                }) {
                    consumers.insert(rule.name.clone());
                }
            }
            if !consumers.is_empty() {
                let mut ck = checkpoint.lock().await;
                let invalidated = crate::commands::run_preview::invalidate_with_downstream(
                    &mut ck, &dag, &consumers,
                );
                if let Err(e) = ck.save_to_file(&checkpoint_path) {
                    tracing::warn!(error = %e, "failed to save checkpoint after reference rebuild");
                }
                eprintln!(
                    "  {} reference rebuild invalidated {} rule(s): {}",
                    "↻".yellow(),
                    invalidated.len(),
                    invalidated.join(", ")
                );
            }
        }
    }

    // Event-driven fine-grained scheduler.
    //
    // Instead of group barriers (wait for ALL rules at depth N before starting
    // ANY at depth N+1), each rule is submitted as soon as all its individual
    // dependencies complete.  SchedulerState tracks per-rule status and
    // re-evaluates readiness after every completion event.  This eliminates the
    // "tail latency" problem where a single slow rule in a parallel group delays
    // downstream rules that only depend on already-finished fast rules.
    //
    // Concurrency is still bounded by -j (tokio Semaphore).  ResourcePool
    // (threads/memory/groups) is checked per-rule inside execute_rule_with_config.

    // ── Checkpoint re-entry replay (issue #78 P3) ────────────────────────
    // Re-apply recorded re-entries whose checkpoint rule still stands, then
    // rebuild the DAG so a resume reconstructs the same static plan a fresh
    // run would. Invalidated checkpoint rules are revoked: their samples
    // leave the plan until the rule re-runs and re-records.
    {
        let ck = checkpoint.lock().await;
        if !ck.reentries.is_empty() {
            let valid: std::collections::HashSet<String> = config
                .rules
                .iter()
                .filter(|rule| {
                    ck.is_completed(&rule.name)
                        && !rerun
                        && (tombstone_keep.contains(&rule.name)
                            || crate::commands::run_preview::rule_outputs_exist(
                                rule,
                                workdir_actual.as_ref(),
                                &wildcard_values,
                            ))
                })
                .map(|r| r.name.clone())
                .collect();
            let replayed =
                oxo_flow_core::reentry::replay_valid_reentries(&mut config, &ck.reentries, &valid)?;
            tracing::info!(count = replayed.len(), "replayed checkpoint re-entries");
            dag = WorkflowDag::from_rules_with_config(
                &config.rules,
                &config_placeholder_values(&config.config),
            )
            .context("failed to rebuild workflow DAG after re-entry replay")?;
            order = if target.is_empty() {
                dag.execution_order()?
            } else {
                let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
                dag.execution_order_for_targets(&target_refs)
                    .with_context(|| "failed to resolve target rules")?
            };
        }
    }

    let rule_names: Vec<String> = order.to_vec();
    let rule_name_refs: Vec<&str> = rule_names.iter().map(String::as_str).collect();
    let mut sched = oxo_flow_core::scheduler::SchedulerState::new(&rule_name_refs);
    let mut order_set: std::collections::HashSet<String> = order.iter().cloned().collect();
    let run_started = std::time::Instant::now();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(jobs.max(1)));
    let mut join_set = tokio::task::JoinSet::new();
    let mut submitted: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Task id → rule name for panic attribution (issue #136 fix 2): a
    // panicked task leaves the JoinSet without its payload, so the id is
    // the only way to learn which rule died and release its scheduler slot.
    let mut task_rule: std::collections::HashMap<tokio::task::Id, String> =
        std::collections::HashMap::new();

    // Pre-process checkpoint-completed rules so they are never re-submitted.
    {
        let ck = checkpoint.lock().await;
        for rule_name in &order {
            if ck.is_completed(rule_name)
                && !rerun
                && order_set.contains(rule_name.as_str())
                && !submitted.contains(rule_name.as_str())
            {
                // Shared with dry-run's read-only preview (issue #66).
                // Tombstoned rules count as up to date while no dependent
                // needs their outputs.
                let outputs_ok = tombstone_keep.contains(rule_name)
                    || config
                        .get_rule(rule_name)
                        .map(|rule| {
                            crate::commands::run_preview::rule_outputs_exist(
                                rule,
                                workdir_actual.as_ref(),
                                &wildcard_values,
                            )
                        })
                        .unwrap_or(true);
                if outputs_ok {
                    submitted.insert(rule_name.clone());
                    sched.mark_completed(oxo_flow_core::executor::JobRecord {
                        rule: rule_name.clone(),
                        status: oxo_flow_core::executor::JobStatus::Success,
                        started_at: None,
                        finished_at: None,
                        exit_code: Some(0),
                        stdout: None,
                        stderr: None,
                        command: None,
                        retries: 0,
                        timeout: None,
                        skip_reason: None,
                        max_rss_mb: None,
                        cpu_seconds: None,
                    });
                    skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !is_tty {
                        eprintln!("  {} {} (already completed)", "⊝".dimmed(), rule_name);
                    }
                    progress.inc(1);
                }
            }
        }
    }

    // Ready-list ordering (age_ready_list below, called every dispatch
    // round): the scheduler's ready rules are filtered to this run's order
    // set minus already-submitted rules, then sorted by EFFECTIVE priority
    // — declared priority plus the rounds a rule has waited ready-but-not-
    // submitted (the aging counter of issue #123, cluster-side issue #134)
    // — descending, ties broken by name for determinism. Priorities are
    // captured ONCE up front: the config mutates mid-run across checkpoint
    // re-entry (issue #78 P3), and re-reading priorities per round would
    // drift the ordering.
    let priority_map: std::collections::HashMap<String, i32> = config
        .rules
        .iter()
        .map(|r| (r.name.clone(), r.priority))
        .collect();
    // Fair-dispatch aging (issue #123): every dispatch round a rule spends
    // ready-but-not-submitted adds AGING_STEP to its effective priority, so
    // high-priority rules that keep failing/re-occupying slots cannot starve
    // their lower-priority producers forever (live: auto-sra dumps at p10
    // starved behind merges at p20 sharing the limit_merge group). Aging
    // only matters under contention — with free slots everything submits
    // immediately and the relative order is unchanged.
    const AGING_STEP: i32 = 1;
    let mut waited_rounds: std::collections::HashMap<String, i32> = Default::default();

    // ---- main event loop -------------------------------------------------

    loop {
        // Check deadlock before each scheduling round.
        if !sched.is_complete() && sched.running_count() == 0 && join_set.is_empty() {
            sched.check_deadlock(&dag)?;
        }

        // Submit every rule whose dependencies are now satisfied — capped to
        // the free -j slots so the semaphore queue never builds, and the
        // ready list (aged by age_ready_list) decides dispatch order fairly.
        let ready = age_ready_list(
            sched.ready_rules(&dag)?,
            &order_set,
            &submitted,
            &priority_map,
            &waited_rounds,
        );
        // jobs was clamped to >= 1 at run_command entry, so a raw 0 can
        // never silently suppress all submissions (issue #136 fix 1).
        let available = jobs.saturating_sub(sched.running_count());
        let to_submit: Vec<String> = ready.iter().take(available).cloned().collect();
        for name in ready.iter().skip(available) {
            *waited_rounds.entry(name.clone()).or_insert(0) += AGING_STEP;
        }
        for rule_name in &to_submit {
            // Skip rules blocked by a failed upstream dependency.
            let blocked_by = {
                let frs = failed_rules_set.lock().await;
                dag.dependencies(rule_name)
                    .ok()
                    .and_then(|deps| deps.into_iter().find(|d| frs.contains(d.as_str())))
            };
            if let Some(dep) = blocked_by {
                skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                {
                    let mut frs = failed_rules_set.lock().await;
                    frs.insert(rule_name.clone());
                }
                blocked.lock().await.push((rule_name.clone(), dep));
                submitted.insert(rule_name.clone());
                sched.mark_completed(oxo_flow_core::executor::JobRecord {
                    rule: rule_name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Skipped,
                    started_at: None,
                    finished_at: None,
                    exit_code: None,
                    stdout: None,
                    stderr: None,
                    command: None,
                    retries: 0,
                    timeout: None,
                    skip_reason: Some("blocked by failed upstream dependency".into()),
                    max_rss_mb: None,
                    cpu_seconds: None,
                });
                if !is_tty {
                    eprintln!(
                        "  {} {} (blocked by failed dependency)",
                        "⊘".yellow(),
                        rule_name
                    );
                }
                progress.inc(1);
                continue;
            }

            submitted.insert(rule_name.clone());
            sched.mark_running(rule_name);

            // ---- spawn task (identical logic to pre-scheduler version) -----
            let rule = config
                .get_rule(rule_name)
                .ok_or_else(|| anyhow::anyhow!("rule '{}' not found in workflow", rule_name))
                .unwrap()
                .clone();
            let rule_name = rule_name.clone();
            let executor = executor.clone();
            let checkpoint = checkpoint.clone();
            let checkpoint_path = checkpoint_path.clone();
            let failed_rules_set = failed_rules_set.clone();
            let failures = failures.clone();
            let success_count = success_count.clone();
            let fail_count = fail_count.clone();
            let non_required_fail_count = non_required_fail_count.clone();
            let sensitive_values = sensitive_values.clone();
            let skipped_count = skipped_count.clone();
            let wildcard_values = wildcard_values.clone();
            let workdir_actual = workdir_actual.clone();
            let semaphore = semaphore.clone();
            let progress = progress.clone();

            progress.set_message(format!("executing {}", rule_name));
            if !is_tty {
                progress_narrate(format_args!("  {} {}", "Running:".bold().cyan(), rule_name));
            }
            emit_execution_event(oxo_flow_core::executor::ExecutionEvent::RuleStarted {
                rule: rule_name.clone(),
                command: None,
            });

            let typed_config = config.config.clone();
            // Register the task name BEFORE the spawn: the closure moves
            // `rule_name` in, so the id→name map needs its own clone.
            let task_rule_name = rule_name.clone();
            let handle = join_set.spawn(async move {
                let _permit = semaphore.acquire().await;

                // Snapshot the input file set BEFORE execution (issue #72):
                // the manifest records what the rule is about to consume, so
                // files added mid-run are not silently baked into the
                // recorded baseline.
                // Remote inputs record (scheme, key, size, etag) when a cloud
                // backend is registered (issue #78 P2); without one they
                // degrade gracefully (warning + entry skipped).
                let input_manifest = oxo_flow_core::executor::checkpoint::snapshot_input_manifest(
                    &rule,
                    workdir_actual.as_ref(),
                    &wildcard_values,
                    &crate::commands::run_preview::storage_resolver(),
                )
                .ok()
                .flatten();

                let result = executor
                    .execute_rule_with_config(&rule, &wildcard_values, &typed_config)
                    .await;

                match result {
                    Ok(record) => {
                        let duration = record
                            .finished_at
                            .and_then(|f| record.started_at.map(|s| f.signed_duration_since(s)))
                            .map(|d| d.num_milliseconds() as f64 / 1000.0)
                            .unwrap_or(0.0);

                        if record.status == oxo_flow_core::executor::JobStatus::Success {
                            success_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            emit_execution_event(oxo_flow_core::executor::ExecutionEvent::RuleCompleted {
                                rule: rule_name.clone(),
                                status: oxo_flow_core::executor::JobStatus::Success,
                                duration_ms: (duration * 1000.0) as u64,
                            });
                            if !is_tty {
                                progress_narrate(format_args!(
                                    "  {} {} ({:.1}s)",
                                    "✓".green(),
                                    rule_name,
                                    duration
                                ));
                            }
                            let benchmark =
                                oxo_flow_core::executor::checkpoint::BenchmarkRecord {
                                    rule: rule_name.clone(),
                                    wall_time_secs: duration,
                                    max_memory_mb: record.max_rss_mb,
                                    memory_limit_mb: rule
                                        .effective_memory()
                                        .and_then(oxo_flow_core::scheduler::parse_memory_mb),
                                    cpu_seconds: record.cpu_seconds,
                                    retries: record.retries,
                                };
                            let mut ck = checkpoint.lock().await;
                            ck.record_run(&record);
                            ck.mark_completed(&rule_name, benchmark);
                            if let Some(ref manifest) = input_manifest {
                                ck.record_input_manifest(&rule_name, manifest.clone());
                            }
                            if provenance {
                                for output in &rule.output {
                                    let output_path = workdir_actual.join(output);
                                    if output_path.exists()
                                        && let Ok(checksum) =
                                            oxo_flow_core::executor::checkpoint::compute_file_checksum(&output_path)
                                    {
                                        ck.record_checksum(output, checksum);
                                    }
                                }
                            }
                            if let Err(e) = ck.save_to_file(&checkpoint_path) {
                                tracing::warn!("Failed to save checkpoint: {e}");
                            }
                            (rule_name, oxo_flow_core::executor::JobStatus::Success, record)
                        } else if record.status == oxo_flow_core::executor::JobStatus::Skipped {
                            skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            // Surface the executor's skip reason (condition
                            // false, optional inputs missing, outputs
                            // up-to-date) — otherwise a submitted rule that
                            // never executes looks indistinguishable from one
                            // that ran.
                            if !is_tty {
                                let reason = record
                                    .skip_reason
                                    .as_deref()
                                    .unwrap_or("skipped");
                                eprintln!("  {} {} ({reason})", "⊝".dimmed(), rule_name);
                            }
                            (rule_name, oxo_flow_core::executor::JobStatus::Skipped, record)
                        } else {
                            // A failed `required = false` rule is recorded and
                            // blocks its dependents, but does not fail the
                            // run (issue #99 B2).
                            if rule.required {
                                fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            } else {
                                non_required_fail_count
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            {
                                let mut frs = failed_rules_set.lock().await;
                                frs.insert(rule_name.clone());
                            }
                            let mut ck = checkpoint.lock().await;
                            ck.record_run(&record);
                            ck.mark_failed(&rule_name);
                            if let Err(e) = ck.save_to_file(&checkpoint_path) {
                                tracing::warn!("Failed to save checkpoint: {e}");
                            }
                            let mut err_msg = format!("rule '{}' failed", rule_name);
                            if let Some(ref stderr) = record.stderr {
                                let trimmed = stderr.trim();
                                if !trimmed.is_empty() {
                                    err_msg.push_str(&format!("\nstderr: {}", trimmed));
                                }
                            }
                            if let Some(code) = record.exit_code {
                                err_msg.push_str(&format!("\nexit code: {}", code));
                            }
                            progress_narrate(format_args!("  {} {}", "✗".red(), err_msg));
                            // Always record: keep_going needs the final
                            // listing, non-required failures are listed in
                            // either mode, and the AI recovery path must be
                            // able to find the ABORTING rule by name.
                            {
                                let mut reason = String::new();
                                if let Some(code) = record.exit_code {
                                    reason.push_str(&format!("exit code {}", code));
                                }
                                if let Some(ref stderr) = record.stderr
                                    && let Some(last) = stderr
                                        .trim()
                                        .lines()
                                        .next_back()
                                        .filter(|l| !l.is_empty())
                                {
                                    if !reason.is_empty() {
                                        reason.push_str(" — ");
                                    }
                                    reason.push_str(last);
                                }
                                if reason.is_empty() {
                                    reason.push_str("failed");
                                }
                                if !rule.required {
                                    reason.push_str(" (non-required)");
                                }
                                let mut f = failures.lock().await;
                                f.push((rule_name.clone(), reason));
                            }
                            (
                                rule_name,
                                record.status, // Failed or TimedOut
                                record,
                            )
                        }
                    }
                    Err(e) => {
                        if rule.required {
                            fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        } else {
                            non_required_fail_count
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        {
                            let mut frs = failed_rules_set.lock().await;
                            frs.insert(rule_name.clone());
                        }
                        // Build the failure record first so the checkpoint
                        // captures the same diagnostics the report will show
                        // (issue #83 WS2).
                        let record = oxo_flow_core::executor::JobRecord {
                            rule: rule_name.clone(),
                            status: oxo_flow_core::executor::JobStatus::Failed,
                            started_at: None,
                            finished_at: None,
                            exit_code: Some(-1),
                            stdout: None,
                            // Staging/security errors embed config-expanded
                            // URIs and full commands — mask them like every
                            // other captured surface (issue #99 B1).
                            stderr: Some(oxo_flow_core::executor::process::mask_sensitive(
                                &e.to_string(),
                                &sensitive_values,
                            )),
                            command: None,
                            retries: 0,
                            timeout: None,
                            skip_reason: None,
                            max_rss_mb: None,
                            cpu_seconds: None,
                        };
                        emit_execution_event(oxo_flow_core::executor::ExecutionEvent::RuleCompleted {
                            rule: rule_name.clone(),
                            status: oxo_flow_core::executor::JobStatus::Failed,
                            duration_ms: 0,
                        });
                        let mut ck = checkpoint.lock().await;
                        ck.record_run(&record);
                        ck.mark_failed(&rule_name);
                        if let Err(e) = ck.save_to_file(&checkpoint_path) {
                            tracing::warn!("Failed to save checkpoint: {e}");
                        }
                        if !keep_going {
                            progress_narrate(format_args!(
                                "  {} rule '{}' failed: {}",
                                "✗".red(),
                                rule_name,
                                e
                            ));
                        }
                        // Always record: keep_going needs the final
                        // listing, non-required failures are listed in
                        // either mode, and the AI recovery path must be able
                        // to find the ABORTING rule by name (a non-required
                        // failure recorded earlier must not shadow it).
                        let mut f = failures.lock().await;
                        let reason = if rule.required {
                            e.to_string()
                        } else {
                            format!("{} (non-required)", e)
                        };
                        f.push((rule_name.clone(), reason));
                        (rule_name, oxo_flow_core::executor::JobStatus::Failed, record)
                    }
                }
            });
            task_rule.insert(handle.id(), task_rule_name);
        }

        // ---- wait for completions -----------------------------------------

        // If nothing is in-flight, we're done (all rules completed, failed, or blocked).
        if join_set.is_empty() {
            break;
        }

        // Wait for the next rule to finish.
        let (completed_rule, status, record) = match join_set.join_next_with_id().await {
            Some(Ok((_id, v))) => v,
            Some(Err(e)) => {
                if e.is_panic() {
                    tracing::error!("Task panicked: {e}");
                }
                // A panicked task is an ENGINE fault, not a rule-level
                // best-effort failure — always counts as required (the run
                // fails), regardless of the rule's `required` flag.
                // Its pool registration (if it was waiting) must not hold
                // the FIFO line hostage — live waiters re-register.
                executor.clear_resource_waiters().await;
                fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // The task left the JoinSet without its payload, so the id
                // is the only link back to the rule (issue #136 fix 2).
                // Without marking it completed the rule would stay in the
                // scheduler's running set forever, permanently shrinking
                // the submit cap — with -j 1 every remaining rule would
                // silently never run. Fall through to the common failure
                // bookkeeping instead of `continue`, so dependents are
                // blocked, the checkpoint records the engine fault, and
                // the standard abort path fails the run loudly.
                let Some(rule_name) = task_rule.remove(&e.id()) else {
                    // Internal invariant: every spawned task registers its
                    // name at spawn. Fail the run rather than leak the cap.
                    // The engine fault counts as a failure in the summary
                    // (issue #142 H6).
                    emit_run_json_summary(
                        json,
                        "failed",
                        &workflow,
                        success_count.load(std::sync::atomic::Ordering::Relaxed),
                        skipped_count.load(std::sync::atomic::Ordering::Relaxed),
                        fail_count.load(std::sync::atomic::Ordering::Relaxed),
                        non_required_fail_count.load(std::sync::atomic::Ordering::Relaxed),
                        blocked.lock().await.len(),
                        vec![],
                    );
                    return Err(anyhow::anyhow!(
                        "internal error: task {} has no recorded rule",
                        e.id()
                    ));
                };
                {
                    let mut frs = failed_rules_set.lock().await;
                    frs.insert(rule_name.clone());
                }
                let record = oxo_flow_core::executor::JobRecord {
                    rule: rule_name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Failed,
                    started_at: None,
                    finished_at: None,
                    exit_code: Some(-1),
                    stdout: None,
                    stderr: Some(oxo_flow_core::executor::process::mask_sensitive(
                        &e.to_string(),
                        &sensitive_values,
                    )),
                    command: None,
                    retries: 0,
                    timeout: None,
                    skip_reason: None,
                    max_rss_mb: None,
                    cpu_seconds: None,
                };
                let mut ck = checkpoint.lock().await;
                ck.record_run(&record);
                ck.mark_failed(&rule_name);
                if let Err(save_err) = ck.save_to_file(&checkpoint_path) {
                    tracing::warn!("Failed to save checkpoint: {save_err}");
                }
                let mut f = failures.lock().await;
                f.push((rule_name.clone(), format!("task panicked: {e}")));
                (
                    rule_name,
                    oxo_flow_core::executor::JobStatus::Failed,
                    record,
                )
            }
            None => break,
        };

        sched.mark_completed(record);
        progress.inc(1);

        // ── Checkpoint re-entry processing (issue #78 P3) ───────────────────
        // A successful checkpoint rule may declare new samples in its manifest;
        // they merge into the plan and execute in this same run.
        if status == oxo_flow_core::executor::JobStatus::Success
            && let Some(cp_rule) = config.get_rule(&completed_rule).cloned()
            && cp_rule.checkpoint
            && let Err(e) = process_reentry(
                &mut config,
                &cp_rule,
                workdir_actual.as_ref(),
                &wildcard_values,
                &mut sched,
                &mut order,
                &mut order_set,
                &mut dag,
                &checkpoint,
            )
            .await
        {
            tracing::error!(rule = %completed_rule, error = %e, "checkpoint re-entry failed");
            // Fail the checkpoint rule and propagate like any failure.
            // Build the record first so the checkpoint captures the same
            // diagnostics the report will show (issue #83 WS2).
            let record = oxo_flow_core::executor::JobRecord {
                rule: completed_rule.clone(),
                status: oxo_flow_core::executor::JobStatus::Failed,
                started_at: None,
                finished_at: None,
                exit_code: Some(1),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                timeout: None,
                skip_reason: Some(format!("re-entry manifest: {e}")),
                max_rss_mb: None,
                cpu_seconds: None,
            };
            {
                let mut frs = failed_rules_set.lock().await;
                frs.insert(completed_rule.clone());
            }
            {
                let mut ck = checkpoint.lock().await;
                ck.record_run(&record);
                ck.mark_failed(&completed_rule);
                if let Err(save_err) = ck.save_to_file(&checkpoint_path) {
                    tracing::warn!("Failed to save checkpoint: {save_err}");
                }
            }
            {
                let mut f = failures.lock().await;
                let reason = if cp_rule.required {
                    format!("re-entry manifest: {e}")
                } else {
                    format!("re-entry manifest: {e} (non-required)")
                };
                f.push((completed_rule.clone(), reason));
            }
            if cp_rule.required {
                fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else {
                non_required_fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            sched.mark_completed(record);
        }

        // Abort on first failure when not in keep_going mode.
        let fc = fail_count.load(std::sync::atomic::Ordering::Relaxed);
        if fc > 0
            && !keep_going
            && (status == oxo_flow_core::executor::JobStatus::Failed
                || status == oxo_flow_core::executor::JobStatus::TimedOut
                || status == oxo_flow_core::executor::JobStatus::Cancelled)
        {
            progress.finish_and_clear();
            // Kill in-flight rule processes before cancelling their tasks —
            // `abort_all()` alone orphans the OS children, which keep
            // consuming the machine after the run exits (issue #131).
            // Clear the pool's FIFO waiter queue first so cancelled waiters
            // cannot hold the line hostage (issue #123 100% guarantee).
            executor.clear_resource_waiters().await;
            for (rule_name, pid) in executor.active_pids() {
                if let Err(e) = oxo_flow_core::executor::timeout::kill_process_tree(pid) {
                    tracing::warn!(
                        rule = %rule_name,
                        pid,
                        error = %e,
                        "failed to signal in-flight rule process during abort"
                    );
                }
            }
            join_set.abort_all();

            // AI error recovery
            let should_recover =
                ai_recover || crate::commands::ai_template::should_use_ai(Some(&workflow), false);
            if should_recover {
                let failures_guard = failures.lock().await;
                // Diagnose the rule that actually aborted the run — a
                // non-required failure recorded earlier must not shadow it.
                let failure = failures_guard
                    .iter()
                    .find(|(name, _)| name == &completed_rule)
                    .or_else(|| failures_guard.first());
                if let Some((rule, error)) = failure
                    && let Some(provider) =
                        crate::commands::ai_template::try_resolve_ai(Some(&workflow), true)
                {
                    let result = crate::commands::ai_recover::diagnose_failure(
                        &workflow, rule, -1, error, &provider,
                    )
                    .await;
                    match result {
                        Ok(diag) => {
                            if let Some(ref toml) = diag.modified_toml {
                                let _ = crate::commands::ai_recover::apply_fix(
                                    &workflow, toml, "recovery",
                                );
                            }
                        }
                        Err(e) => eprintln!("  AI diagnosis failed: {e}"),
                    }
                }
            }

            // The plain-failure abort: emit the summary BEFORE returning,
            // mirroring the keep-going path's document (issue #142 H6).
            let ck = checkpoint.lock().await;
            emit_run_json_summary(
                json,
                "failed",
                &workflow,
                success_count.load(std::sync::atomic::Ordering::Relaxed),
                skipped_count.load(std::sync::atomic::Ordering::Relaxed),
                fail_count.load(std::sync::atomic::Ordering::Relaxed),
                non_required_fail_count.load(std::sync::atomic::Ordering::Relaxed),
                blocked.lock().await.len(),
                rule_resource_rows(&ck),
            );
            return Err(anyhow::anyhow!("workflow execution failed"));
        }

        // Propagate failure transitively: every rule that (transitively)
        // depends on a failed rule is marked as skipped/blocked. Uses a
        // worklist to ensure transitive propagation (grandchild of a
        // failed rule is also blocked). Runs when the loop continues past
        // failures: keep_going with any failure, or a non-required failure
        // in either mode (issue #99 B2).
        let nrf = non_required_fail_count.load(std::sync::atomic::Ordering::Relaxed);
        if (fc > 0 && keep_going) || nrf > 0 {
            // Seed the worklist with every rule that directly depends on a
            // failed rule and has not been submitted yet.
            let frs = failed_rules_set.lock().await;
            let mut worklist: Vec<String> = Vec::new();
            for rule_name in &rule_names {
                if submitted.contains(rule_name) {
                    continue;
                }
                let deps = dag.dependencies(rule_name).unwrap_or_default();
                if deps.iter().any(|d| frs.contains(d.as_str())) {
                    worklist.push(rule_name.to_string());
                }
            }
            drop(frs);

            // Process transitively: when a rule is blocked, its own dependents
            // may also become blockable.
            let mut idx = 0;
            while idx < worklist.len() {
                let name = &worklist[idx];
                if submitted.contains(name.as_str()) {
                    idx += 1;
                    continue;
                }
                submitted.insert(name.clone());
                skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                {
                    let mut frs = failed_rules_set.lock().await;
                    frs.insert(name.clone());
                }
                sched.mark_completed(oxo_flow_core::executor::JobRecord {
                    rule: name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Skipped,
                    started_at: None,
                    finished_at: None,
                    exit_code: None,
                    stdout: None,
                    stderr: None,
                    command: None,
                    retries: 0,
                    timeout: None,
                    skip_reason: Some("blocked by failed upstream dependency".into()),
                    max_rss_mb: None,
                    cpu_seconds: None,
                });
                progress.inc(1);
                if !is_tty {
                    eprintln!("  {} {} (blocked by failed dependency)", "⊘".yellow(), name);
                }

                // Find transitive dependents of this newly-blocked rule.
                {
                    let frs = failed_rules_set.lock().await;
                    for other in &rule_names {
                        if submitted.contains(other) || worklist.contains(other) {
                            continue;
                        }
                        let deps = dag.dependencies(other).unwrap_or_default();
                        if deps.iter().any(|d| frs.contains(d.as_str())) {
                            worklist.push(other.to_string());
                        }
                    }
                }

                idx += 1;
            }

            // Record blocked rules for final summary
            {
                let frs = failed_rules_set.lock().await;
                for name in &worklist {
                    let deps = dag.dependencies(name).unwrap_or_default();
                    if let Some(dep) = deps.into_iter().find(|d| frs.contains(d.as_str())) {
                        blocked.lock().await.push((name.clone(), dep));
                    }
                }
            }
        }
    }

    let success_count = success_count.load(std::sync::atomic::Ordering::Relaxed);
    let fail_count = fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let non_required_fail_count =
        non_required_fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let skipped_count = skipped_count.load(std::sync::atomic::Ordering::Relaxed);
    let mut checkpoint = checkpoint.lock().await;
    let failures = failures.lock().await;
    let blocked = blocked.lock().await;

    progress.finish_and_clear();

    // Environment-cache aging (issue #75): --cache-dir would grow without
    // bound; files untouched beyond the age limit are removed after the
    // run so the next one starts clean. Default 90 days, overridable with
    // the `cache_max_age_days` workflow config key (0 disables aging).
    const DEFAULT_CACHE_MAX_AGE_DAYS: u64 = 90;
    let cache_max_age_days: u64 = config
        .config
        .get("cache_max_age_days")
        .and_then(toml::Value::as_integer)
        .filter(|d| *d >= 0)
        .map(|d| d as u64)
        .unwrap_or(DEFAULT_CACHE_MAX_AGE_DAYS);
    if cache_max_age_days > 0
        && let Some(ref cache_dir) = cache_dir
    {
        let removed = cleanup_cache_dir(cache_dir, cache_max_age_days);
        if removed > 0 {
            tracing::info!(
                removed,
                cache = %cache_dir.display(),
                "aged environment-cache files removed"
            );
        }
    }

    emit_execution_event(oxo_flow_core::executor::ExecutionEvent::WorkflowCompleted {
        total_duration_ms: run_started.elapsed().as_millis() as u64,
        succeeded: success_count,
        failed: fail_count,
        skipped: skipped_count,
    });
    if non_required_fail_count > 0 {
        progress_narrate(format_args!(
            "\n{} {} succeeded, {} skipped, {} failed, {} non-required failed",
            "Done:".bold(),
            success_count,
            skipped_count,
            fail_count,
            non_required_fail_count
        ));
    } else {
        progress_narrate(format_args!(
            "\n{} {} succeeded, {} skipped, {} failed",
            "Done:".bold(),
            success_count,
            skipped_count,
            fail_count
        ));
    }

    // Automatic report snapshot (issue #83 P1-14): capture the final
    // checkpoint as a JSON report plus an index.json entry. One call site
    // covers every terminal path of the run loop (and resume, which shares
    // this summary via run_command); dry-run never reaches here. Snapshot
    // errors are warnings — a reporting hiccup must never fail the run.
    if !no_report_snapshot
        && let Err(e) =
            crate::commands::output::snapshot_report(&workflow, &workdir_actual, &checkpoint)
    {
        eprintln!("  {} Report snapshot failed: {e}", "⚠".yellow());
    }

    // With --keep-going, execution continues past failures, so list every failed
    // rule (and why) in one place rather than making the user hunt for them.
    if !failures.is_empty() {
        eprintln!("\n{}", "Failed rules:".bold().red());
        for (name, reason) in failures.iter() {
            eprintln!("  {} {} — {}", "✗".red(), name.bold(), reason);
        }
    }

    // Blocked rules did not run and produced no outputs. Listing them keeps the
    // fallout of a failure visible without disguising it as a genuine failure.
    if !blocked.is_empty() {
        eprintln!("\n{}", "Blocked rules (did not run):".bold().yellow());
        for (name, dep) in blocked.iter() {
            eprintln!("  {} {} — depends on '{}'", "⊘".yellow(), name.bold(), dep);
        }
    }

    // Verify output files exist for completed rules
    if success_count > 0 {
        let workdir_actual = workdir.as_ref().unwrap_or(&workdir_default);
        let mut missing_outputs = Vec::new();
        let mut verified = 0usize;
        let mut total_size: u64 = 0;
        for rule_name in &order {
            if checkpoint.is_completed(rule_name)
                && let Some(rule) = config.get_rule(rule_name)
            {
                for output in &rule.output {
                    if !output.contains('{') {
                        let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            output,
                            &wildcard_values,
                        );
                        let resolved = workdir_actual.join(&expanded);
                        if resolved.exists() {
                            verified += 1;
                            if let Ok(meta) = std::fs::metadata(&resolved) {
                                total_size += meta.len();
                            }
                        } else if !workdir_actual.join(output).exists() {
                            missing_outputs.push(format!("  {}: {}", rule_name, output));
                        } else {
                            verified += 1;
                        }
                    }
                }
            }
        }
        if verified > 0 {
            let size_str = if total_size > 1_073_741_824 {
                format!("{:.1}GB", total_size as f64 / 1_073_741_824.0)
            } else if total_size > 1_048_576 {
                format!("{:.1}MB", total_size as f64 / 1_048_576.0)
            } else {
                format!("{}B", total_size)
            };
            eprintln!(
                "{} {} output files verified ({} total)",
                "✓".green(),
                verified,
                size_str
            );
        }
        if !missing_outputs.is_empty() {
            eprintln!(
                "{} {} output file(s) were not found:",
                "⚠".yellow(),
                missing_outputs.len()
            );
            for m in &missing_outputs {
                eprintln!("{}", m.dimmed());
            }
        }
    }

    // Transform chunk cleanup (transform.cleanup = true): delete the chunk
    // inputs of completed combine rules once the whole run has finished
    // successfully. Deferred to here (instead of per-rule success) so that
    // a failed run keeps its chunks for debugging, and a resumed run never
    // regenerates chunks while the combine stays pre-skipped (which would
    // orphan the freshly created chunk files).
    if fail_count == 0 {
        let workdir_actual = workdir.as_ref().unwrap_or(&workdir_default);
        for rule in &config.rules {
            if rule.cleanup_chunks && checkpoint.is_completed(&rule.name) {
                oxo_flow_core::executor::checkpoint::cleanup_transform_chunks(rule, workdir_actual)
                    .await;
            }
        }

        // Temporary rules: after a FULLY successful run, delete their
        // outputs once every dependent is complete and record a tombstone.
        // A future run regenerates them via cascade-up (missing inputs
        // re-run the completed producer). Leaf rules keep their outputs.
        let mut new_tombstones: Vec<(String, Vec<String>)> = Vec::new();
        for rule in &config.rules {
            if !rule.temporary || !checkpoint.is_completed(&rule.name) {
                continue;
            }
            let dependents = dag.dependents(&rule.name).unwrap_or_default();
            if dependents.is_empty() {
                eprintln!(
                    "  {} temporary rule '{}' is a leaf — outputs kept",
                    "ℹ".dimmed(),
                    rule.name
                );
                continue;
            }
            if !dependents.iter().all(|d| checkpoint.is_completed(d)) {
                continue;
            }
            let mut deleted = Vec::new();
            for output in &rule.output {
                let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                    output,
                    &wildcard_values,
                );
                if expanded.contains('{') {
                    continue;
                }
                let resolved = workdir_actual.join(&expanded);
                let removed = if resolved.is_dir() {
                    std::fs::remove_dir_all(&resolved)
                } else {
                    std::fs::remove_file(&resolved)
                };
                if removed.is_ok() {
                    deleted.push(expanded);
                }
            }
            if !deleted.is_empty() {
                eprintln!(
                    "  {} temporary outputs deleted for '{}' ({} file(s), regenerated on demand)",
                    "⊘".dimmed(),
                    rule.name,
                    deleted.len()
                );
                new_tombstones.push((rule.name.clone(), deleted));
            }
        }
        if !new_tombstones.is_empty() {
            for (rule, paths) in new_tombstones {
                checkpoint.tombstones.insert(rule, paths);
            }
            if let Err(e) = checkpoint.save_to_file(&checkpoint_path) {
                tracing::warn!(error = %e, "failed to save checkpoint after tombstoning");
            }
        }
    }

    // Pilot summary: after a --samples subset run, report what the pilot
    // implies for the full cohort (simple linear projection). With [ai]
    // enabled, append a plain-language interpretation.
    if !samples_filter.is_empty() {
        let pilot_count = oxo_flow_core::scientific_preflight::count_samples(&config);
        let total_count = WorkflowConfig::from_file(&workflow)
            .map(|c| oxo_flow_core::scientific_preflight::count_samples(&c))
            .unwrap_or(pilot_count);
        let elapsed = run_started.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();
        let per_sample = if pilot_count > 0 {
            elapsed_secs / pilot_count as f64
        } else {
            0.0
        };
        let projected_secs = per_sample * total_count as f64;

        eprintln!("\n{}", "Pilot summary:".bold().cyan());
        eprintln!(
            "  Samples: {}/{} (pilot) | Wall time: {:.1}s | Per-sample: {:.1}s | Projected full run: ~{:.0}s",
            pilot_count, total_count, elapsed_secs, per_sample, projected_secs
        );
        let scientific =
            oxo_flow_core::scientific_preflight::analyze_scientific_constraints(&config);
        if !scientific.is_empty() {
            eprintln!(
                "  {} scientific preflight finding(s) — see 'oxo-flow dry-run --samples ...'",
                scientific.len()
            );
        }

        // AI interpretation when the workflow opts in via [ai].enabled.
        if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), false)
        {
            let warnings_text = scientific
                .iter()
                .map(|w| format!("- [{}] {}: {}", w.code, w.rule, w.message))
                .collect::<Vec<_>>()
                .join("\n");
            let prompt = format!(
                "A pilot run of {} sample(s) out of {} completed in {:.1}s ({} succeeded, {} failed).\n\
                 Per-sample wall time: {:.1}s; projected full run: ~{:.0}s.\n\
                 Scientific preflight findings:\n{}\n\n\
                 Write a short pilot report (3-5 sentences): whether the pilot results are\n\
                 healthy, whether scaling up is advisable, and what to fix first.",
                pilot_count,
                total_count,
                elapsed_secs,
                success_count,
                fail_count,
                per_sample,
                projected_secs,
                if warnings_text.is_empty() {
                    "none"
                } else {
                    &warnings_text
                }
            );
            match provider
                .chat(
                    "You are a bioinformatics workflow consultant summarizing a pilot run.",
                    &prompt,
                )
                .await
            {
                Ok(response) => {
                    eprintln!();
                    eprintln!("{}", "Pilot report (AI):".bold().green());
                    eprintln!("{response}");
                }
                Err(e) => eprintln!("  AI pilot report failed: {e}"),
            }
        }
    }

    // JSON output mode (issue #142 H6): the summary is emitted on EVERY
    // path — completed, failed, and aborted — so `--json` consumers can
    // rely on the document regardless of how the run ended.
    emit_run_json_summary(
        json,
        if fail_count > 0 {
            "failed"
        } else {
            "completed"
        },
        &workflow,
        success_count,
        skipped_count,
        fail_count,
        non_required_fail_count,
        blocked.len(),
        // `checkpoint` here is the MutexGuard locked above (line ~2415);
        // Deref coercion hands rule_resource_rows the underlying state.
        rule_resource_rows(&checkpoint),
    );

    // The verdict is independent of --keep-going (issue #133): keep-going
    // changes SCHEDULING (failures don't stop the run), never the verdict —
    // a run with required failures exits non-zero so scripts and the web
    // delegator (which classifies by exit code) see the truth.
    // `required = false` failures keep exit 0 (issue #99 B2).
    if fail_count > 0 {
        return Err(anyhow::anyhow!("workflow execution failed"));
    }

    Ok(())
}

/// Emit the machine-readable run summary (`--json`) in the canonical
/// `{"command":"run",...}` shape.
///
/// Shared by the happy path AND every abort path (preflight failures,
/// budget breaches, cluster runs, the plain-failure abort) so a failed run
/// never leaves stdout at zero bytes while the keep-going path emits the
/// document (issue #142 H6). Only emits when `--json` was requested;
/// stdout carries nothing else, so the document is always the sole output.
#[allow(clippy::too_many_arguments)] // matching the crate's established convention
fn emit_run_json_summary(
    json: bool,
    status: &str,
    workflow: &Path,
    succeeded: usize,
    skipped: usize,
    failed: usize,
    non_required_failed: usize,
    blocked: usize,
    resources: Vec<serde_json::Value>,
) {
    if !json {
        return;
    }
    let output = serde_json::json!({
        "command": "run",
        "status": status,
        "workflow": workflow.to_string_lossy(),
        "results": serde_json::json!({
            "succeeded": succeeded,
            "skipped": skipped,
            "failed": failed,
            "non_required_failed": non_required_failed,
            "blocked": blocked,
        }),
        "resources": resources,
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

/// Per-rule resource rows for the `--json` summary (issue #163): the same
/// data the report's Benchmarks table shows — wall time, sampled peak RSS,
/// sampled CPU seconds, retries — keyed by rule, deterministic order.
/// Empty for aborted paths where no checkpoint summary is meaningful.
fn rule_resource_rows(
    checkpoint: &oxo_flow_core::executor::checkpoint::CheckpointState,
) -> Vec<serde_json::Value> {
    let mut names: Vec<&String> = checkpoint.benchmarks.keys().collect();
    names.sort_unstable();
    names
        .into_iter()
        .map(|name| {
            let b = &checkpoint.benchmarks[name];
            let status = if checkpoint.completed_rules.contains(name) {
                "completed"
            } else if checkpoint.failed_rules.contains(name) {
                "failed"
            } else {
                "running"
            };
            serde_json::json!({
                "rule": name,
                "status": status,
                "wall_time_secs": b.wall_time_secs,
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
struct RefRecord {
    fingerprint: String,
    content_sig: Option<String>,
    /// The source path (config-expanded, workdir-relative) this record was
    /// computed against — the exemption re-fingerprints with it to prove a
    /// fingerprint change is a PURE path migration.
    source_path: Option<String>,
}

/// Content signature of a reference SOURCE: `size:<bytes>` plus
/// `|hash:<sha256>` for files under the manifest hash cap. Directories
/// and large files return `None` — the path-migration exemption requires
/// the strong hash component, so anything weaker never skips a rebuild.
fn source_content_sig(path: &std::path::Path) -> Option<String> {
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
fn free_kilobytes(path: &std::path::Path) -> Option<u64> {
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
fn age_ready_list(
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

#[allow(clippy::too_many_arguments)]
pub async fn dry_run_command(
    workflow: Option<PathBuf>,
    target: Vec<String>,
    module: Vec<String>,
    verbose: bool,
    json: bool,
    ai: bool,
    _ai_max_retries: Option<u32>,
    samples_filter: Vec<String>,
    workdir: Option<PathBuf>,
    profile: Option<String>,
    skip_ref_build: bool,
    cli_args: Vec<String>,
    rerun: bool,
    resume_failed: bool,
) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(workflow)?;
    let workflow_dir = oxo_flow_core::parent_dir(&workflow).to_path_buf();
    // Path resolution base: --workdir wins, default is the workflow file's
    // directory — the same base the executor uses for run (issue #68).
    let base_dir = workdir.as_deref().unwrap_or(&workflow_dir);

    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // ── CLI config overrides (shared with run, issue #77) ─────
    // Applied in run's exact order: overrides first, then
    // the --samples subset filter — so the preview config is structurally
    // identical to what `run` would execute. The workflow's own [config]
    // keys gate the `--KEY VALUE` space form (same rule as run, issue #71).
    let declared_config_keys: std::collections::HashSet<String> =
        config.config.keys().cloned().collect();
    let cli_arg_values = parse_cli_overrides(cli_args, &declared_config_keys)?;
    apply_cli_overrides(&mut config, &cli_arg_values)?;

    // ── Filter to a sample subset (--samples first:N / names / ready) ──
    // `ready` resolves against a scratch expanded clone; the report covers
    // the full cohort so the readiness section stays informative even when
    // the listing is filtered (issue #63).
    let ready_report = if !samples_filter.is_empty() {
        samples::apply_samples_filter(&mut config, &samples_filter, false, base_dir)?
    } else {
        None
    };

    // ── Execution profile (shared with run) ──────────────────────────────
    // The SAME merge helper `run` uses: preview and execution can never
    // drift apart on profile semantics.
    if let Some(ref profile_name) = profile {
        crate::commands::run_preview::merge_profile(&mut config, profile_name, &workflow_dir)?;
    }

    // ── Scientific preflight (deterministic, evidence-backed) ────────────
    // Printed for every dry-run; with --ai the findings are also passed to
    // the model for a plain-language explanation.
    let scientific = oxo_flow_core::scientific_preflight::analyze_scientific_constraints(&config);
    let scientific_context = if scientific.is_empty() {
        String::new()
    } else {
        let mut text = String::new();
        for w in &scientific {
            text.push_str(&format!(
                "- [{}] {}: {}\n  Fix: {}\n",
                w.code, w.rule, w.message, w.suggestion
            ));
        }
        text
    };

    // AI: auto-detect from workflow [ai] or explicit --ai flag
    if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai) {
        crate::commands::ai_check::analyze_workflow(
            &workflow,
            &provider,
            "dry-run",
            &scientific_context,
        )
        .await?;
        println!();
    }

    if !scientific.is_empty() {
        eprintln!("\n{}", "Scientific preflight:".bold().yellow());
        for w in &scientific {
            eprintln!("  ⚠ [{}] {}: {}", w.code, w.rule, w.message);
            eprintln!("    {} {}", "→".bold(), w.suggestion);
        }
    }

    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    // ── Undefined `{config.*}` gate (issue #142 H1) — the same gate run
    // applies: preview must refuse a typo'd key exactly like execution.
    let e005 = undefined_config_findings(&config);
    if !e005.is_empty() {
        return Err(anyhow::anyhow!(
            "workflow references undefined config variable(s) — fix before running:\n  {}",
            e005.join("\n  ")
        ));
    }

    let mut dag = WorkflowDag::from_rules_with_config(
        &config.rules,
        &config_placeholder_values(&config.config),
    )
    .context("failed to build workflow DAG")?;

    // --module partial runs (issue #112 elasticity) — same resolution as
    // `run`.
    let mut target = target;
    for m in &module {
        match config.module_closure(m) {
            Some(names) => target.extend(names),
            None => {
                return Err(anyhow::anyhow!(
                    "unknown module '{m}' — known modules: {}",
                    known_modules_hint(&config.module_rules)
                ));
            }
        }
    }
    // Compute the execution set (respects --target), then display it in
    // parallel-group order — the same grouping `run` uses for scheduling.
    // Independent rules at the same level appear adjacent, so the listing
    // reflects actual concurrency instead of an arbitrary topological order.
    let mut order = if target.is_empty() {
        let order_set: std::collections::HashSet<String> =
            dag.execution_order()?.into_iter().collect();
        let mut ordered: Vec<String> = Vec::new();
        for group in dag.parallel_groups()? {
            let mut level: Vec<String> = group
                .into_iter()
                .filter(|name| order_set.contains(name))
                .collect();
            level.sort();
            ordered.extend(level);
        }
        ordered
    } else {
        let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
        dag.execution_order_for_targets(&target_refs)
            .with_context(|| "failed to resolve target rules")?
    };

    eprintln!(
        "{} (dry-run) {} rules would execute",
        "DAG:".bold().yellow(),
        order.len()
    );

    // All config values (including CLI --arg overrides) become {config.key} in templates.
    let mut wildcard_values: HashMap<String, String> = HashMap::new();
    for (key, value) in &config.config {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        wildcard_values.insert(format!("config.{key}"), string_val);
    }

    // ── Checkpoint-aware rerun preview (issue #66) ────────────────────────
    // Read-only: the preview classifies every rule in the execution set
    // exactly as `run` would — invalidation sources, downstream cascade,
    // and the protected remainder — without touching the checkpoint on disk.
    let checkpoint_path = base_dir.join(".oxo-flow/checkpoint.json");
    // The preview is ALWAYS computed — with an empty state when no
    // checkpoint exists. `when` conditions can skip rules even on a fresh
    // run, so "every rule will execute" would be wrong without it.
    let checkpoint_state =
        match oxo_flow_core::executor::checkpoint::CheckpointState::load_from_file(&checkpoint_path)
        {
            Ok(ck) => ck,
            Err(_) => {
                eprintln!(
                    "{} no checkpoint at {} — treating every rule as never completed",
                    "⚠".yellow(),
                    checkpoint_path.display()
                );
                oxo_flow_core::executor::checkpoint::CheckpointState::default()
            }
        };
    let sensitive_keys: std::collections::HashSet<String> = config
        .config_meta
        .iter()
        .filter(|(_, def)| def.sensitive)
        .map(|(key, _)| key.clone())
        .collect();
    // The verbose plan print must not leak sensitive values (issue #99 B1).
    let sensitive_values = sensitive_values_of(&config);
    let interpreter_map = config.workflow.interpreter_map.clone();
    let mut preview = crate::commands::run_preview::preview_run_plan(
        &checkpoint_state,
        &config,
        &dag,
        &order,
        base_dir,
        &wildcard_values,
        &sensitive_keys,
        &interpreter_map,
        &checkpoint_path,
        rerun,
        resume_failed,
    );

    // ── Checkpoint re-entry replay (issue #78 P3) ────────────────────────
    // The round-0 preview determines which checkpoint rules are up-to-date
    // (Skipped); their recorded re-entries replay and the preview recomputes
    // on the re-expanded plan — the same static plan a run would execute.
    if !checkpoint_state.reentries.is_empty() {
        use crate::commands::run_preview::RuleStatus;
        let valid: std::collections::HashSet<String> = preview
            .plan
            .iter()
            .filter(|p| p.status == RuleStatus::Skipped)
            .map(|p| p.name.clone())
            .collect();
        let replayed = oxo_flow_core::reentry::replay_valid_reentries(
            &mut config,
            &checkpoint_state.reentries,
            &valid,
        )?;
        if !replayed.is_empty() {
            tracing::info!(
                count = replayed.len(),
                "dry-run replayed checkpoint re-entries"
            );
            dag = WorkflowDag::from_rules_with_config(
                &config.rules,
                &config_placeholder_values(&config.config),
            )
            .context("failed to rebuild workflow DAG after re-entry replay")?;
            let new_order: Vec<String> = if target.is_empty() {
                let order_set: std::collections::HashSet<String> =
                    dag.execution_order()?.into_iter().collect();
                let mut ordered: Vec<String> = Vec::new();
                for group in dag.parallel_groups()? {
                    let mut level: Vec<String> = group
                        .into_iter()
                        .filter(|n| order_set.contains(n))
                        .collect();
                    level.sort();
                    ordered.extend(level);
                }
                ordered
            } else {
                let target_refs: Vec<&str> = target.iter().map(String::as_str).collect();
                dag.execution_order_for_targets(&target_refs)
                    .with_context(|| "failed to resolve target rules")?
            };
            let old_order_len = order.len();
            order = new_order;
            if order.len() != old_order_len {
                eprintln!(
                    "  {} re-entry replay: plan grew from {} to {} rules",
                    "Replay:".bold().cyan(),
                    old_order_len,
                    order.len()
                );
            }
            preview = crate::commands::run_preview::preview_run_plan(
                &checkpoint_state,
                &config,
                &dag,
                &order,
                base_dir,
                &wildcard_values,
                &sensitive_keys,
                &interpreter_map,
                &checkpoint_path,
                rerun,
                resume_failed,
            );
        }
    }
    {
        let p = &preview;
        let modified = p
            .checkpoint_modified
            .map(|t| {
                chrono::DateTime::<chrono::Local>::from(t)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| "unknown time".to_string());
        eprintln!(
            "{} {} (modified {})",
            "Checkpoint:".bold().cyan(),
            p.checkpoint_path.display(),
            modified.dimmed()
        );
        let will_run = p.plan.len() - p.will_skip;
        eprintln!(
            "  completed: {} | will run: {} | will skip: {} | protected (outside this run): {}",
            p.completed_total,
            will_run.to_string().green(),
            p.will_skip.to_string().dimmed(),
            p.protected_outside
        );
        for chain in &p.cascade_chains {
            eprintln!("  {} {}", "rerun cascade:".yellow(), chain.join(" → "));
        }
    }

    // ── Reference auto-builds (mirrors run's --skip-ref-build semantics) ──
    // run builds any reference whose output is missing before scheduling;
    // the preview surfaces that cost instead of hiding it.
    let reference_builds: Vec<String> = if skip_ref_build {
        Vec::new()
    } else {
        config
            .references
            .iter()
            .filter(|r| !base_dir.join(&r.output).exists())
            .map(|r| r.name.clone())
            .collect()
    };
    if !reference_builds.is_empty() {
        eprintln!(
            "{} {} reference build(s) would run: {}",
            "References:".bold().cyan(),
            reference_builds.len(),
            reference_builds.join(", ")
        );
        eprintln!(
            "  {} pass --skip-ref-build to assume they are pre-built.",
            "ℹ".dimmed()
        );
    }

    // Sample readiness (issue #63). With `--samples ready` the report covers
    // the full cohort (computed pre-filter); otherwise it is computed on the
    // (possibly filtered) expanded config.
    let readiness_report = match ready_report {
        Some(report) => Some(report),
        None => Some(oxo_flow_core::readiness::compute_readiness(
            &config, base_dir,
        )),
    };
    if let Some(report) = &readiness_report {
        samples::print_readiness_section(report);
    }

    for (i, rule_name) in order.iter().enumerate() {
        let rule = config
            .get_rule(rule_name)
            .ok_or_else(|| anyhow::anyhow!("rule '{}' not found", rule_name))?;
        let status_text = preview
            .plan
            .iter()
            .find(|r| r.name == *rule_name)
            .map(|r| match &r.status {
                crate::commands::run_preview::RuleStatus::NeverCompleted => {
                    format!("{}", "[run: never completed]".bold())
                }
                crate::commands::run_preview::RuleStatus::ConfigInvalidated => {
                    format!("{}", "[run: config changed]".bold())
                }
                crate::commands::run_preview::RuleStatus::InputInvalidated => {
                    format!("{}", "[run: input changed]".bold().yellow())
                }
                crate::commands::run_preview::RuleStatus::OutputsMissing => {
                    format!("{}", "[run: outputs missing]".bold().yellow())
                }
                crate::commands::run_preview::RuleStatus::Cascaded { from } => {
                    format!(
                        "{}",
                        format!("[rerun: downstream of {from}]").bold().yellow()
                    )
                }
                crate::commands::run_preview::RuleStatus::Skipped => {
                    format!("{}", "[skip: up to date]".dimmed())
                }
                crate::commands::run_preview::RuleStatus::SkippedByWhen => {
                    format!("{}", "[skip: when condition false]".dimmed())
                }
                crate::commands::run_preview::RuleStatus::CascadedUpstream { from } => {
                    format!("{}", format!("[rerun: upstream of {from}]").bold().yellow())
                }
                crate::commands::run_preview::RuleStatus::Forced => {
                    format!("{}", "[run: forced (--rerun)]".bold())
                }
                crate::commands::run_preview::RuleStatus::SkippedFresh => {
                    format!("{}", "[skip: outputs up-to-date]".dimmed())
                }
            })
            .unwrap_or_default();
        eprintln!("  {}. {}  {}", i + 1, rule_name.bold().cyan(), status_text);

        let threads = rule.effective_threads();
        eprintln!("     threads={}", threads);

        if !rule.environment.is_empty() {
            eprintln!("     env={}", rule.environment.kind());
        }

        if let Some(ref mem) = rule.effective_memory() {
            eprintln!("     memory={}", mem);
        }

        if rule.checkpoint {
            eprintln!("     checkpoint=true");
        }

        if !rule.output.is_empty() {
            let expanded_outputs: Vec<String> = rule
                .output
                .iter()
                .map(|o| {
                    oxo_flow_core::executor::checkpoint::expand_config_in_path(o, &wildcard_values)
                })
                .collect();
            eprintln!("     outputs: {:?}", expanded_outputs);
        }

        if let Some(ref cmd) = rule.shell {
            let expanded = oxo_flow_core::executor::process::render_shell_command(
                cmd,
                rule,
                &wildcard_values,
                oxo_flow_core::scheduler::detect_system_limits(),
            );
            // Mask sensitive values (issue #99 B1): the plan print is a log
            // surface like any other.
            let expanded =
                oxo_flow_core::executor::process::mask_sensitive(&expanded, &sensitive_values);
            if let Some((_category, description)) =
                oxo_flow_core::format::shell_blocking_pattern(&expanded)
            {
                eprintln!(
                    "     command: {}  {}",
                    expanded,
                    format!("(blocked: E011 — {description})").red().bold()
                );
            } else {
                eprintln!("     command: {}", expanded);
            }
        }

        // Show input file status for concrete (non-wildcard) paths.
        // Relative paths resolve against the workflow's directory — the same
        // place the executor runs shells from and readiness checks (issue #63
        // follow-up: CWD-based checks contradicted the readiness report when
        // invoked from another directory).
        for inp in &rule.input {
            let s = inp.to_string();
            if !s.contains('{') && !s.contains('*') && !s.starts_with('/') {
                let exists = base_dir.join(&s).exists();
                let icon = if exists { "✓" } else { "✗" };
                eprintln!("     input {}: {}", icon, s);
            }
        }

        if verbose {
            // Additional verbose info
        }
    }

    // Resource summary
    let total_threads: u32 = config.rules.iter().map(|r| r.effective_threads()).sum();
    let max_threads: u32 = config
        .rules
        .iter()
        .map(|r| r.effective_threads())
        .max()
        .unwrap_or(1);
    let memory_values: Vec<&str> = config
        .rules
        .iter()
        .filter_map(|r| r.effective_memory())
        .collect();
    eprintln!();
    eprintln!(
        "{} {} rules, total {} threads declared, max {} threads/rule",
        "Summary:".bold(),
        order.len(),
        total_threads,
        max_threads,
    );
    if !memory_values.is_empty() {
        eprintln!(
            "         {} rule(s) with memory requirements",
            memory_values.len()
        );
    }
    if !config.sample_groups.is_empty() {
        eprintln!(
            "         {} sample group(s), {} pair(s)",
            config.sample_groups.len(),
            config.pairs.len()
        );
    }

    // Suggest -j based on system threads DIVIDED by the workflow's max
    // per-rule thread declaration, so concurrent jobs don't oversubscribe
    // the CPU. E.g. 10 threads / rules-with-4-threads = 2 concurrent jobs.
    let max_threads_per_rule = config
        .rules
        .iter()
        .map(|r| r.effective_threads())
        .max()
        .unwrap_or(1)
        .max(1);
    let system_threads = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    // Professional suggestion = the smaller of:
    //   - DAG width: the maximum number of rules that can ever run
    //     simultaneously (suggesting -j above this is meaningless)
    //   - Resource math: system_threads / max_threads_per_rule
    //     (more concurrent jobs would oversubscribe the CPU)
    let dag_width = dag
        .parallel_groups()
        .map(|groups| groups.iter().map(|g| g.len()).max().unwrap_or(1))
        .unwrap_or(1) as u32;
    let suggested_jobs = (system_threads / max_threads_per_rule)
        .min(dag_width)
        .clamp(1, 16)
        .to_string();
    eprintln!(
        "\n{}  oxo-flow run {} -j {}",
        "To execute:".bold().cyan(),
        workflow.display(),
        suggested_jobs
    );

    // JSON output mode
    if json {
        let order_list = order.clone();
        let rule_list: Vec<serde_json::Value> = order
            .iter()
            .filter_map(|name| config.get_rule(name))
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "threads": r.effective_threads(),
                    "environment": r.environment.kind(),
                    "memory": r.effective_memory(),
                    "checkpoint": r.checkpoint,
                })
            })
            .collect();

        // Machine-readable sample readiness (issue #63).
        let samples_block = readiness_report.as_ref().map(|report| {
            let ready_names: Vec<&str> = report.ready.iter().map(|s| s.name.as_str()).collect();
            let waiting: Vec<serde_json::Value> = report
                .waiting
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "missing": s.missing,
                    })
                })
                .collect();
            serde_json::json!({
                "total": report.total,
                "ready": report.ready.len(),
                "waiting_count": report.waiting.len(),
                "ready_names": ready_names,
                "waiting": waiting,
                "missing_global": report.missing_global,
            })
        });

        // Machine-readable checkpoint preview (issue #66): the predicted
        // execution plan with per-rule status + reason.
        let checkpoint_block = Some(&preview).map(|p| {
            let plan: Vec<serde_json::Value> = p
                .plan
                .iter()
                .map(|r| {
                    let (status, cascaded_from) = match &r.status {
                        crate::commands::run_preview::RuleStatus::NeverCompleted => {
                            ("run-never-completed", None)
                        }
                        crate::commands::run_preview::RuleStatus::ConfigInvalidated => {
                            ("run-config-changed", None)
                        }
                        crate::commands::run_preview::RuleStatus::InputInvalidated => {
                            ("run-input-changed", None)
                        }
                        crate::commands::run_preview::RuleStatus::OutputsMissing => {
                            ("run-outputs-missing", None)
                        }
                        crate::commands::run_preview::RuleStatus::Cascaded { from } => {
                            ("run-cascaded", Some(from.clone()))
                        }
                        crate::commands::run_preview::RuleStatus::Skipped => ("skip", None),
                        crate::commands::run_preview::RuleStatus::SkippedByWhen => {
                            ("skip-when-condition", None)
                        }
                        crate::commands::run_preview::RuleStatus::CascadedUpstream { from } => {
                            ("run-cascaded-upstream", Some(from.clone()))
                        }
                        crate::commands::run_preview::RuleStatus::Forced => ("run-forced", None),
                        crate::commands::run_preview::RuleStatus::SkippedFresh => {
                            ("skip-fresh", None)
                        }
                    };
                    serde_json::json!({
                        "name": r.name,
                        "status": status,
                        "cascaded_from": cascaded_from,
                    })
                })
                .collect();
            serde_json::json!({
                "path": p.checkpoint_path.display().to_string(),
                "modified": p.checkpoint_modified.map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                }),
                "completed_total": p.completed_total,
                "summary": {
                    "will_run": p.plan.len() - p.will_skip,
                    "will_skip": p.will_skip,
                    "protected_outside": p.protected_outside,
                },
                "plan": plan,
                "cascade_chains": p.cascade_chains,
            })
        });

        // Stable per-rule plan (schema_version 1 — the first release to
        // ship this surface): status is the same bracket word the human
        // listing prints ([run:/[skip:/[rerun:), reason is the human
        // status text after the colon — so the JSON is a 1:1 structured
        // mirror of the stderr plan the ecosystem greps.
        let plan_entries: Vec<serde_json::Value> = order
            .iter()
            .filter_map(|name| {
                let rule = config.get_rule(name)?;
                let (status, reason, cascaded_from) = match preview
                    .plan
                    .iter()
                    .find(|r| r.name == *name)
                    .map(|r| &r.status)
                {
                    Some(crate::commands::run_preview::RuleStatus::NeverCompleted) => {
                        ("run", "never completed".to_string(), None)
                    }
                    Some(crate::commands::run_preview::RuleStatus::ConfigInvalidated) => {
                        ("run", "config changed".to_string(), None)
                    }
                    Some(crate::commands::run_preview::RuleStatus::InputInvalidated) => {
                        ("run", "input changed".to_string(), None)
                    }
                    Some(crate::commands::run_preview::RuleStatus::OutputsMissing) => {
                        ("run", "outputs missing".to_string(), None)
                    }
                    Some(crate::commands::run_preview::RuleStatus::Cascaded { from }) => (
                        "rerun",
                        format!("downstream of {from}"),
                        Some(from.clone()),
                    ),
                    Some(crate::commands::run_preview::RuleStatus::Skipped) => {
                        ("skip", "up to date".to_string(), None)
                    }
                    Some(crate::commands::run_preview::RuleStatus::SkippedByWhen) => {
                        ("skip", "when condition false".to_string(), None)
                    }
                    Some(crate::commands::run_preview::RuleStatus::CascadedUpstream { from }) => (
                        "rerun",
                        format!("upstream of {from}"),
                        Some(from.clone()),
                    ),
                    Some(crate::commands::run_preview::RuleStatus::Forced) => {
                        ("run", "forced (--rerun)".to_string(), None)
                    }
                    Some(crate::commands::run_preview::RuleStatus::SkippedFresh) => {
                        ("skip", "outputs up-to-date".to_string(), None)
                    }
                    // The preview classifies every rule in the execution
                    // set, so this branch is unreachable; keep it loud.
                    None => ("unknown", "unknown".to_string(), None),
                };
                let expanded_shell = rule.shell.as_ref().map(|cmd| {
                    oxo_flow_core::executor::process::render_shell_command(
                        cmd,
                        rule,
                        &wildcard_values,
                        oxo_flow_core::scheduler::detect_system_limits(),
                    )
                });
                // Declared patterns stay raw in inputs/outputs (the stable
                // contract); the expanded variants give consumers the exact
                // per-instance paths the engine will touch — the same
                // {config.x} expansion the human stderr listing shows.
                let inputs_expanded: Vec<String> = rule
                    .input
                    .to_vec()
                    .iter()
                    .map(|p| {
                        oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            p,
                            &wildcard_values,
                        )
                    })
                    .collect();
                let outputs_expanded: Vec<String> = rule
                    .output
                    .to_vec()
                    .iter()
                    .map(|p| {
                        oxo_flow_core::executor::checkpoint::expand_config_in_path(
                            p,
                            &wildcard_values,
                        )
                    })
                    .collect();
                Some(serde_json::json!({
                    "name": rule.name,
                    "status": status,
                    "reason": reason,
                    "cascaded_from": cascaded_from,
                    "threads": rule.effective_threads(),
                    "memory": rule.effective_memory(),
                    "environment": rule.environment.kind(),
                    "command": expanded_shell,
                    "inputs": serde_json::to_value(&rule.input).unwrap_or(serde_json::Value::Null),
                    "outputs": serde_json::to_value(&rule.output).unwrap_or(serde_json::Value::Null),
                    "inputs_expanded": inputs_expanded,
                    "outputs_expanded": outputs_expanded,
                }))
            })
            .collect();

        let output = serde_json::json!({
            "command": "dry-run",
            "schema_version": 1,
            "workflow": workflow.display().to_string(),
            "total_rules": order.len(),
            "execution_order": order_list,
            "rules": rule_list,
            "plan": plan_entries,
            "summary": {
                "total_threads": total_threads,
                "max_threads_per_rule": max_threads,
                "memory_rules": memory_values.len(),
                "would_execute": preview.plan.len() - preview.will_skip,
                "will_skip": preview.will_skip,
                "total_rules": order.len(),
            },
            "sample_groups": config
                .sample_groups
                .iter()
                .map(|g| {
                    serde_json::json!({
                        "name": g.name,
                        "samples": g.samples,
                    })
                })
                .collect::<Vec<_>>(),
            "pairs": config
                .pairs
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "pair_id": p.pair_id,
                        "experiment": p.experiment,
                        "control": p.control,
                    })
                })
                .collect::<Vec<_>>(),
            "suggested_jobs": suggested_jobs,
            "samples": samples_block,
            "checkpoint_preview": checkpoint_block,
            "reentry": {
                // Recorded re-entries (checkpoint.json) — replayed when their
                // checkpoint rule is up-to-date; revoked otherwise (issue #78 P3).
                "recorded": checkpoint_state.reentries,
                // Checkpoint rules that may add instances at runtime.
                "possible": config
                    .rules
                    .iter()
                    .filter(|r| r.checkpoint)
                    .map(|r| serde_json::json!({
                        "rule": r.name,
                        "manifest": r.checkpoint_manifest,
                    }))
                    .collect::<Vec<_>>(),
            },
            "profile": profile,
            "reference_builds": reference_builds,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }

    Ok(())
}

pub async fn debug_command(workflow: PathBuf, rule_name: Option<String>, ai: bool) -> Result<()> {
    print_banner();
    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    // AI: auto-detect from workflow [ai] or explicit --ai flag
    if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai) {
        // Build wildcard values from config so template variables expand
        let mut debug_wildcards = std::collections::HashMap::new();
        for (key, value) in &config.config {
            let string_val = match value {
                toml::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            debug_wildcards.insert(format!("config.{key}"), string_val);
        }
        let expanded: Vec<String> = config
            .rules
            .iter()
            .map(|r| {
                let shell_cmd = r.shell.as_deref().unwrap_or("(no shell command)");
                let shell = oxo_flow_core::executor::process::render_shell_command(
                    shell_cmd,
                    r,
                    &debug_wildcards,
                    oxo_flow_core::scheduler::detect_system_limits(),
                );
                format!(
                    "## {}\nthreads={}, memory={}, env={:?}\n```bash\n{}\n```",
                    r.name,
                    r.resources.threads,
                    r.resources.memory.as_deref().unwrap_or("1G"),
                    r.environment,
                    shell
                )
            })
            .collect();
        let joined = expanded.join("\n\n");

        let system = r#"## Role
You are a bioinformatics pipeline debugger. Explain each expanded shell command in plain language,
flagging potential issues: parameter misuse, resource mismatch, missing required flags, or
logical errors. Output format per rule:

**rule_name**: <what this command does — 1 sentence>
⚠ <any issues found — 1 line per issue, or "No issues" if clean>
"#
        .to_string();
        let user = format!(
            "## Workflow: {}\n\n## Expanded Commands\n\n{joined}\n\n## Task\nExplain each command and flag issues.",
            workflow.display()
        );

        println!("{}", "  Analyzing commands...".bold().cyan());
        match provider.chat(&system, &user).await {
            Ok(response) => {
                println!("\n{}\n{response}", "AI Analysis".bold().green().underline());
                return Ok(());
            }
            Err(e) => eprintln!("  AI analysis failed: {e}"),
        }
    }

    let dag = WorkflowDag::from_rules_with_config(
        &config.rules,
        &config_placeholder_values(&config.config),
    )
    .context("failed to build workflow DAG")?;

    let rules_to_show: Vec<&oxo_flow_core::rule::Rule> = if let Some(ref name) = rule_name {
        match config.rules.iter().find(|r| r.name == *name) {
            Some(r) => vec![r],
            None => {
                eprintln!("{} rule '{}' not found", "error:".bold().red(), name);
                return Err(anyhow::anyhow!("rule not found"));
            }
        }
    } else {
        config.rules.iter().collect()
    };

    eprintln!(
        "{} Debugging {} rules",
        "Debug:".bold().cyan(),
        rules_to_show.len()
    );

    let mut wildcard_values: HashMap<String, String> = HashMap::new();
    for (key, value) in &config.config {
        let string_val = match value {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        wildcard_values.insert(format!("config.{key}"), string_val);
    }

    for rule in &rules_to_show {
        eprintln!("{}", format!("── Rule: {} ──", rule.name).bold().cyan());

        if let Some(ref desc) = rule.description {
            eprintln!("  {} {}", "Description:".dimmed(), desc);
        }

        if !rule.output.is_empty() {
            let expanded_outputs: Vec<String> = rule
                .output
                .iter()
                .map(|o| {
                    oxo_flow_core::executor::checkpoint::expand_config_in_path(o, &wildcard_values)
                })
                .collect();
            eprintln!("  {} {:?}", "Outputs:".dimmed(), expanded_outputs);
        }

        if let Some(ref cmd) = rule.shell {
            let expanded = oxo_flow_core::executor::process::render_shell_command(
                cmd,
                rule,
                &wildcard_values,
                oxo_flow_core::scheduler::detect_system_limits(),
            );
            eprintln!("  {} {}", "Shell (expanded):".dimmed(), expanded);
        }

        if let Ok(deps) = dag.dependencies(&rule.name)
            && !deps.is_empty()
        {
            let mut deps = deps;
            deps.sort();
            deps.dedup();
            eprintln!("  {} {:?}", "Dependencies:".dimmed(), deps);
        }

        eprintln!();
    }

    Ok(())
}

/// Default checkpoint location used when `status` is invoked without an argument.
const DEFAULT_CHECKPOINT: &str = ".oxo-flow/checkpoint.json";

/// Checkpoint rules ordered by wall-clock time (slowest first), plus total time.
/// Process a checkpoint rule's re-entry manifest after it completes
/// (issue #78 P3): merge new samples, re-expand from templates, extend the
/// plan, and record the re-entry in the checkpoint. Errors fail the rule.
#[allow(clippy::too_many_arguments)]
async fn process_reentry(
    config: &mut WorkflowConfig,
    rule: &Rule,
    workdir: &std::path::Path,
    wildcard_values: &std::collections::HashMap<String, String>,
    sched: &mut oxo_flow_core::scheduler::SchedulerState,
    order: &mut Vec<String>,
    order_set: &mut std::collections::HashSet<String>,
    dag: &mut WorkflowDag,
    checkpoint: &Arc<tokio::sync::Mutex<CheckpointState>>,
) -> anyhow::Result<()> {
    let manifest_path = rule.checkpoint_manifest.as_ref().ok_or_else(|| {
        anyhow::anyhow!("checkpoint rule '{}' has no checkpoint_manifest", rule.name)
    })?;
    let expanded =
        oxo_flow_core::executor::checkpoint::expand_config_in_path(manifest_path, wildcard_values);
    let full = workdir.join(&expanded);
    let content = std::fs::read_to_string(&full).map_err(|e| {
        anyhow::anyhow!(
            "checkpoint rule '{}' produced no readable manifest at '{}': {e}",
            rule.name,
            full.display()
        )
    })?;
    let (group, samples, pairs) = oxo_flow_core::reentry::parse_manifest(&content)
        .map_err(|e| anyhow::anyhow!("checkpoint rule '{}': {e}", rule.name))?;
    if samples.is_empty() && pairs.is_empty() {
        return Ok(()); // empty manifest = valid no-op
    }
    let new_names =
        oxo_flow_core::reentry::apply_reentry(config, group.as_deref(), &samples, &pairs)?;
    if new_names.is_empty() {
        return Ok(()); // everything already present
    }
    let round = {
        let mut ck = checkpoint.lock().await;
        let round = ck.reentries.iter().map(|r| r.round).max().unwrap_or(0) + 1;
        if round > oxo_flow_core::reentry::MAX_REENTRY_ROUNDS {
            anyhow::bail!(
                "re-entry round cap ({}) exceeded — checkpoint rule '{}' keeps discovering new values; this is a workflow bug",
                oxo_flow_core::reentry::MAX_REENTRY_ROUNDS,
                rule.name
            );
        }
        ck.record_reentry(oxo_flow_core::reentry::ReentryRecord {
            round,
            rule: rule.name.clone(),
            group,
            samples,
            pairs,
        });
        round
    };
    for name in &new_names {
        sched.add_rule(name);
        order_set.insert(name.clone());
        order.push(name.clone());
    }
    *dag = WorkflowDag::from_rules_with_config(
        &config.rules,
        &config_placeholder_values(&config.config),
    )
    .context("failed to rebuild workflow DAG after re-entry")?;
    tracing::info!(
        rule = %rule.name,
        round = round,
        new_instances = ?new_names,
        "checkpoint re-entry expanded the plan"
    );
    Ok(())
}

fn rule_timings(state: &CheckpointState) -> (Vec<(&str, f64)>, f64) {
    let mut timings: Vec<(&str, f64)> = state
        .benchmarks
        .iter()
        .map(|(rule, bench)| (rule.as_str(), bench.wall_time_secs))
        .collect();
    // Deterministic order: slowest first (HashSet iteration order is arbitrary)
    timings.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total = timings.iter().map(|(_, secs)| secs).sum();
    (timings, total)
}

pub async fn handle_status(
    checkpoint: Option<PathBuf>,
    json: bool,
    timing: bool,
    limit: usize,
) -> Result<()> {
    print_banner();

    // Detect common mistake: user passes a .oxoflow file instead of checkpoint
    if let Some(path) = &checkpoint
        && path.extension().is_some_and(|ext| ext == "oxoflow")
    {
        eprintln!(
            "{} '{}' appears to be a workflow file, not a checkpoint.",
            "Warning:".bold().yellow(),
            path.display()
        );
        eprintln!(
            "  The 'status' command expects a checkpoint file (e.g., .oxo-flow/checkpoint.json)."
        );
        eprintln!(
            "  Run 'oxo-flow run {}' first to generate a checkpoint.",
            path.display()
        );
        anyhow::bail!("Cannot read workflow file as checkpoint");
    }

    let checkpoint_path = checkpoint.unwrap_or_else(|| PathBuf::from(DEFAULT_CHECKPOINT));
    let state = CheckpointState::load_from_file(&checkpoint_path).with_context(|| {
        format!(
            "failed to load checkpoint from '{}'.\n  \
             Check that the file exists and is a valid checkpoint (JSON format).\n  \
             Checkpoint files are generated automatically by 'oxo-flow run' in .oxo-flow/checkpoint.json.",
            checkpoint_path.display()
        )
    })?;

    // Deterministic order (HashSet iteration order is arbitrary)
    let mut completed: Vec<&str> = state.completed_rules.iter().map(String::as_str).collect();
    completed.sort_unstable();
    let mut failed: Vec<&str> = state.failed_rules.iter().map(String::as_str).collect();
    failed.sort_unstable();

    if json {
        let mut output = serde_json::json!({
            "command": "status",
            "checkpoint": checkpoint_path.display().to_string(),
            "workflow": state.workflow_path,
            "completed": completed,
            "failed": failed,
        });
        if timing {
            let (timings, total) = rule_timings(&state);
            // serde_json::Map is a BTreeMap: keys stay deterministically sorted
            let timings_map: serde_json::Map<String, serde_json::Value> = timings
                .into_iter()
                .map(|(rule, secs)| (rule.to_string(), serde_json::json!(secs)))
                .collect();
            output["timings"] = serde_json::Value::Object(timings_map);
            output["total_time_secs"] = serde_json::json!(total);
            // Sampled peak-RSS measurements where recorded (issue #67 §4).
            let memory_map: serde_json::Map<String, serde_json::Value> = state
                .benchmarks
                .iter()
                .filter(|(_, b)| b.max_memory_mb.is_some())
                .map(|(rule, b)| {
                    (
                        rule.to_string(),
                        serde_json::json!({
                            "max_memory_mb": b.max_memory_mb,
                            "memory_limit_mb": b.memory_limit_mb,
                        }),
                    )
                })
                .collect();
            if !memory_map.is_empty() {
                output["memory"] = serde_json::Value::Object(memory_map);
            }
        }
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    eprintln!(
        "{} Status for checkpoint: {}",
        "Status:".bold().cyan(),
        checkpoint_path.display()
    );
    eprintln!("  Completed: {}", completed.len());
    eprintln!("  Failed:    {}", failed.len());

    if timing {
        let (timings, total) = rule_timings(&state);
        if !timings.is_empty() {
            eprintln!(
                "\n{} (top {}, total {:.1}s)",
                "Rule timings:".bold().green(),
                limit.min(timings.len()),
                total
            );
            for (rule, secs) in timings.iter().take(limit) {
                let mem = state.benchmarks.get(*rule).and_then(|b| {
                    b.max_memory_mb.map(|m| match b.memory_limit_mb {
                        Some(l) if l > 0 => {
                            if m * 100 >= l * 80 {
                                format!("  peak {m}/{l} MiB ⚠")
                            } else {
                                format!("  peak {m}/{l} MiB")
                            }
                        }
                        _ => format!("  peak {m} MiB"),
                    })
                });
                match mem {
                    Some(mem) => eprintln!("  {} {} ({:.1}s){}", "✓".green(), rule, secs, mem),
                    None => eprintln!("  {} {} ({:.1}s)", "✓".green(), rule, secs),
                }
            }
        }
    } else {
        if !completed.is_empty() {
            eprintln!("\n{}", "Completed rules:".bold().green());
            for rule in &completed {
                eprintln!("  {} {}", "✓".green(), rule);
            }
        }

        if !failed.is_empty() {
            eprintln!("\n{}", "Failed rules:".bold().red());
            for rule in &failed {
                eprintln!("  {} {}", "✗".red(), rule);
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn resume_command(
    checkpoint: PathBuf,
    jobs: usize,
    ai_recover: bool,
    ai_max_retries: Option<u32>,
    keep_going: bool,
    timeout: String,
    workdir: Option<PathBuf>,
    no_report_snapshot: bool,
) -> Result<()> {
    // resume does not produce structured JSON output
    print_banner();

    // Load checkpoint state
    let state = CheckpointState::load_from_file(&checkpoint).with_context(|| {
        format!(
            "failed to load checkpoint from '{}'.\n  \
             Check that the file exists and is a valid checkpoint (JSON format).",
            checkpoint.display()
        )
    })?;

    // Get workflow path from checkpoint
    let workflow_path = match &state.workflow_path {
        Some(p) => PathBuf::from(p),
        None => {
            anyhow::bail!(
                "Checkpoint does not contain a workflow reference.\n  \
                 The checkpoint at '{}' was generated by an older version of oxo-flow.\n  \
                 To resume manually, run: oxo-flow run <workflow.oxoflow>\n  \
                 (oxo-flow run automatically resumes from the checkpoint)",
                checkpoint.display()
            );
        }
    };

    if !workflow_path.exists() {
        anyhow::bail!(
            "Workflow file '{}' referenced by checkpoint no longer exists.\n  \
             The workflow may have been moved or deleted.",
            workflow_path.display()
        );
    }

    let completed = state.completed_rules.len();
    let failed = state.failed_rules.len();

    eprintln!(
        "{} Resuming workflow '{}'",
        "Resume:".bold().cyan(),
        workflow_path.display()
    );
    eprintln!("  Checkpoint: {}", checkpoint.display());
    eprintln!(
        "  State: {} completed, {} failed, {} remaining",
        completed,
        failed,
        state
            .completed_rules
            .len()
            .saturating_sub(completed.saturating_sub(failed))
    );

    if completed == 0 && failed == 0 {
        eprintln!(
            "  {} No rules have been executed yet. Use 'oxo-flow run' instead.",
            "Note:".yellow()
        );
        return Ok(());
    }

    // Re-run the workflow — the checkpoint with completed rules will cause
    // already-finished rules to be skipped automatically
    eprintln!();
    eprintln!(
        "{} Launching executor with {} parallel job(s)...",
        "Info:".bold().cyan(),
        jobs
    );

    // Workdir precedence (issue #68): explicit --workdir > the directory the
    // original run recorded in the checkpoint > the workflow's directory.
    let effective_workdir = workdir.or_else(|| state.workdir.clone().map(PathBuf::from));
    eprintln!(
        "  Workdir: {}",
        effective_workdir
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(workflow directory)".to_string())
    );

    run_command(
        Some(workflow_path),
        jobs,
        keep_going,        // keep_going (same semantics as `run`)
        effective_workdir, // workdir (recorded by the original run)
        None,              // log_file (default path)
        Vec::new(),        // target
        Vec::new(),        // module
        0,                 // retry
        timeout,           // timeout
        false,             // resume_failed (user can re-run with --resume-failed in 'run')
        None,              // profile
        0,                 // max_threads
        0,                 // max_memory
        false,             // skip_env_setup
        true,              // skip_ref_build (resume: refs already built)
        None,              // cache_dir
        false,             // provenance (checkpoint already has checksums)
        false,             // json (resume defaults to human-readable)
        Vec::new(),        // cli_args (resume reuses checkpoint state)
        ai_recover,
        ai_max_retries,
        Vec::new(), // samples_filter (resume restores checkpoint state as-is)
        false,      // rerun (resume skips completed rules by design)
        no_report_snapshot,
        None, // max_submitted (cluster queue cap — resume keeps the profile's)
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        age_ready_list, cleanup_cache_dir, known_modules_hint, parse_cli_overrides,
        substitute_source_placeholder,
    };
    use std::collections::HashSet;

    #[test]
    fn cleanup_cache_dir_is_recursive() {
        // issue #194 C1: nested directories age out too (max_age_days = 0
        // makes everything eligible immediately).
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("sub").join("deeper");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("old.txt"), "x").unwrap();
        std::fs::write(nested.join("old-too.txt"), "y").unwrap();
        let removed = cleanup_cache_dir(dir.path(), 0);
        assert_eq!(removed, 2, "file + one nested directory removed");
        assert!(!dir.path().join("old.txt").exists());
        assert!(!dir.path().join("sub").exists());
    }

    fn declared(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    fn map_of(pairs: &[(&str, i32)]) -> std::collections::HashMap<String, i32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn set_of(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn age_ready_list_orders_by_declared_priority_then_name() {
        // Arrange
        let priority = map_of(&[("high", 20), ("low", 10)]);
        // Act
        let ready = age_ready_list(
            vec!["low".into(), "high".into()],
            &set_of(&["high", "low"]),
            &HashSet::new(),
            &priority,
            &std::collections::HashMap::new(),
        );
        // Assert
        assert_eq!(ready, vec!["high", "low"]);
    }

    #[test]
    fn age_ready_list_aged_rule_overtakes_higher_declared_priority() {
        // The starvation cure (issue #123): a producer passed over for many
        // rounds must eventually outrank the higher-priority rules that keep
        // re-occupying the slots.
        // Arrange
        let priority = map_of(&[("merge", 20), ("dump", 10)]);
        let waited = map_of(&[("dump", 15)]);
        // Act
        let ready = age_ready_list(
            vec!["merge".into(), "dump".into()],
            &set_of(&["merge", "dump"]),
            &HashSet::new(),
            &priority,
            &waited,
        );
        // Assert
        assert_eq!(ready, vec!["dump", "merge"]);
    }

    #[test]
    fn age_ready_list_drops_submitted_and_out_of_scope_rules() {
        // Arrange
        let priority = map_of(&[("a", 5), ("b", 5), ("c", 5)]);
        let submitted = set_of(&["b"]);
        // Act
        let ready = age_ready_list(
            vec!["c".into(), "b".into(), "a".into()],
            &set_of(&["a", "b", "c"]),
            &submitted,
            &priority,
            &std::collections::HashMap::new(),
        );
        // Assert — b is already submitted, so it cannot be dispatched again.
        assert_eq!(ready, vec!["a", "c"]);
    }

    #[test]
    fn age_ready_list_treats_missing_priority_and_waits_as_zero() {
        // Arrange — neither rule appears in the maps.
        // Act
        let ready = age_ready_list(
            vec!["z".into(), "a".into()],
            &set_of(&["a", "z"]),
            &HashSet::new(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        // Assert — equal effective priority (0), name tie-break.
        assert_eq!(ready, vec!["a", "z"]);
    }

    #[test]
    fn accepts_equals_forms_for_any_key() {
        let map = parse_cli_overrides(
            vec![
                "threads=8".to_string(),
                "--mode=protein".to_string(),
                "--new_key=1".to_string(), // undeclared injection via '=' stays legal
            ],
            &declared(&["threads", "mode"]),
        )
        .unwrap();
        assert_eq!(map["threads"], "8");
        assert_eq!(map["mode"], "protein");
        assert_eq!(map["new_key"], "1");
    }

    #[test]
    fn accepts_space_form_for_declared_keys() {
        let map = parse_cli_overrides(
            vec!["--min_quality".to_string(), "45".to_string()],
            &declared(&["min_quality"]),
        )
        .unwrap();
        assert_eq!(map["min_quality"], "45");
    }

    #[test]
    fn rejects_unknown_dash_token() {
        let err = parse_cli_overrides(
            vec!["--config".to_string(), "config/x.toml".to_string()],
            &declared(&["min_quality"]),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unknown argument"), "{msg}");
        assert!(msg.contains("--config"), "{msg}");
        assert!(msg.contains("KEY=VALUE"), "{msg}");
    }

    #[test]
    fn rejects_known_flag_after_positional_with_flag_error() {
        // issue #71 contract: real flags swallowed by the trailing
        // positional get the "command flag, not a config override" error.
        let err = parse_cli_overrides(
            vec!["threads=8".to_string(), "--json".to_string()],
            &declared(&["threads"]),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("command flag"), "{msg}");
        assert!(msg.contains("--json"), "{msg}");
    }

    #[test]
    fn rejects_space_form_for_undeclared_key_even_via_arg() {
        // `--arg --config x` lands in the same override list.
        let err = parse_cli_overrides(
            vec!["--config".to_string(), "x".to_string()],
            &declared(&[]),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("unknown argument"));
    }

    #[test]
    fn rejects_declared_key_without_value() {
        let err = parse_cli_overrides(
            vec!["--min_quality".to_string()],
            &declared(&["min_quality"]),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("invalid config flag"), "{err:?}");
    }

    #[test]
    fn rejects_bare_non_key_value_positional() {
        let err = parse_cli_overrides(vec!["naked".to_string()], &declared(&[])).unwrap_err();
        assert!(format!("{err}").contains("KEY=VALUE"), "{err:?}");
    }

    #[test]
    fn source_placeholder_quotes_paths_with_spaces() {
        // A `{source}` path containing spaces must survive the reference
        // build as ONE shell argument (issue #136 tier-2 audit — the raw
        // splice broke on spaces/metacharacters).
        let rendered =
            substitute_source_placeholder("STAR --genomeDir {source}", "refs/genome data/hg38");
        assert_eq!(rendered, "STAR --genomeDir 'refs/genome data/hg38'");
    }

    #[test]
    fn source_placeholder_escapes_embedded_quotes() {
        // POSIX-safe: an embedded quote closes and reopens the quoting
        // (the `'\''` idiom the environment wrapper also uses).
        let rendered = substitute_source_placeholder("--in {source}", "dir/we'ird");
        assert_eq!(rendered, "--in 'dir/we'\\''ird'");
    }

    #[test]
    fn source_placeholder_without_placeholder_is_unchanged() {
        let rendered = substitute_source_placeholder("echo built", "ignored");
        assert_eq!(rendered, "echo built");
    }

    #[test]
    fn known_modules_hint_names_the_empty_case() {
        // Dry-run must print the same explicit "(none — no [[include]]
        // modules)" hint `run` does instead of a trailing empty list
        // (issue #136 tier-2 audit).
        let empty: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        assert_eq!(
            known_modules_hint(&empty),
            "(none — no [[include]] modules)"
        );
        let mut map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        map.insert("qc".to_string(), vec!["qc".to_string()]);
        assert_eq!(known_modules_hint(&map), "qc");
    }
}
