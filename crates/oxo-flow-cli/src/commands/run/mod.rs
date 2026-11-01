//! Facade for the run-command domain (issue #948 P2-5). Physical layout
//! split only: public paths are unchanged — `crate::commands::run::...` and
//! `super::run::...` resolve exactly as before.

use crate::commands::{print_banner, resolve_workflow, samples};
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::config_impact::{ConfigChangeReport, config_value_string};
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::{
    CheckpointState, ExecutorConfig, LocalExecutor, WorkdirLock, output_tail_bytes_from_env,
};
use oxo_flow_core::rule::{Rule, parse_duration_secs};
use oxo_flow_core::wildcard;
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) mod cluster_path;
pub mod debug;
pub mod dry_run;
mod env_knobs;
mod failure;
mod findings;
mod hooks;
mod narrate;
mod overrides;
mod preflight;
mod reentry;
mod refs;
pub mod resume;
mod run_log;
mod signals;
pub mod status;
mod task;

pub use debug::debug_command;
pub use dry_run::dry_run_command;
pub(crate) use env_knobs::*;
pub(crate) use failure::*;
pub(crate) use findings::*;
pub(crate) use hooks::*;
pub(crate) use narrate::*;
pub(crate) use overrides::*;
pub(crate) use preflight::*;
pub(crate) use reentry::*;
pub(crate) use refs::*;
pub use resume::resume_command;
pub(crate) use run_log::*;
pub(crate) use signals::*;
pub use status::handle_status;
pub(crate) use status::*;
pub(crate) use task::*;

/// Flatten workflow config values into the `{config.key}` placeholder map
/// used for path expansion (DAG edge matching, run-time rendering, …).
/// Default targets (issue #469): rules marked `target = true` are what
/// `run`/`dry-run` build when neither `-t` nor `--module` selected
/// anything — Snakemake's rule-all semantics. No marked rule → the full
/// DAG (backward compatible); an explicit `-t`/`--module` always wins.
pub(crate) fn resolve_default_targets(config: &WorkflowConfig, target: &[String]) -> Vec<String> {
    if !target.is_empty() {
        return target.to_vec();
    }
    let marked: Vec<String> = config
        .rules
        .iter()
        .filter(|r| r.target)
        .map(|r| r.name.clone())
        .collect();
    if marked.is_empty() {
        target.to_vec()
    } else {
        marked
    }
}

#[cfg(test)]
mod default_target_tests {
    use super::*;

    #[test]
    fn resolve_default_targets_prefers_explicit_selection() {
        let config = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"

[[rules]]
name = "a"
target = true
shell = "true"

[[rules]]
name = "b"
shell = "true"
"#,
        )
        .unwrap();

        // Explicit -t always wins.
        assert_eq!(
            resolve_default_targets(&config, &["b".to_string()]),
            vec!["b".to_string()]
        );
        // No explicit selection → the marked rule.
        assert_eq!(resolve_default_targets(&config, &[]), vec!["a".to_string()]);
        // --module already selected something → unchanged.
        assert_eq!(
            resolve_default_targets(&config, &["m_rule".to_string()]),
            vec!["m_rule".to_string()]
        );
    }

    #[test]
    fn resolve_default_targets_full_dag_without_marks() {
        let config = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"

[[rules]]
name = "a"
shell = "true"
"#,
        )
        .unwrap();
        assert!(resolve_default_targets(&config, &[]).is_empty());
    }
}

pub(crate) fn config_placeholder_values(
    config: &HashMap<String, toml::Value>,
) -> HashMap<String, String> {
    oxo_flow_core::config_impact::config_placeholder_map(config)
}
#[allow(clippy::too_many_arguments)]
/// Everything one `oxo-flow run` invocation needs, as named fields — the
/// former 27-positional-parameter signature made every call site a
/// comment-annotated guessing game. Field order here is the contract;
/// callers construct with `..`-free literals so the compiler flags any new
/// field on every call site.
pub struct RunRequest {
    pub workflow: Option<PathBuf>,
    pub jobs: usize,
    pub keep_going: bool,
    pub workdir: Option<PathBuf>,
    pub log_file: Option<PathBuf>,
    pub target: Vec<String>,
    pub module: Vec<String>,
    pub retry: u32,
    pub timeout: String,
    pub resume_failed: bool,
    pub profile: Option<String>,
    pub max_threads: u32,
    pub max_memory: u64,
    pub skip_env_setup: bool,
    pub skip_ref_build: bool,
    pub cache_dir: Option<PathBuf>,
    pub provenance: bool,
    pub json: bool,
    pub cli_args: Vec<String>,
    pub ai_recover: bool,
    pub ai_max_retries: Option<u32>,
    pub samples_filter: Vec<String>,
    pub rerun: bool,
    pub no_report_snapshot: bool,
    pub max_submitted: Option<usize>,
    /// #538: `resume` forwards the user-supplied checkpoint path — without
    /// this, run_command re-derived `<workdir>`/.oxo-flow/checkpoint.json and
    /// an explicit `resume <path>` silently resumed from a DIFFERENT (or
    /// empty) checkpoint while the banner claimed otherwise.
    pub checkpoint_override: Option<PathBuf>,
    /// Issue #935 P1: proceed despite protected-tree drift (the fail-gate
    /// above exists so damaged vendor data never silently poisons a run).
    pub allow_protected_drift: bool,
}

pub async fn run_command(
    RunRequest {
        workflow,
        jobs,
        keep_going,
        workdir,
        log_file,
        target,
        module,
        retry,
        timeout,
        resume_failed,
        profile,
        max_threads,
        max_memory,
        skip_env_setup,
        skip_ref_build,
        cache_dir,
        provenance,
        json,
        cli_args,
        ai_recover,
        ai_max_retries,
        samples_filter,
        rerun,
        no_report_snapshot,
        max_submitted,
        checkpoint_override,
        allow_protected_drift,
    }: RunRequest,
) -> Result<()> {
    print_banner();

    // `--json` consumers get the summary document on every exit path — the
    // guard emits it from `Drop` when a path returns before the explicit
    // emission below (audit finding: preflight/parse aborts exited 1 with
    // zero bytes on stdout).
    let mut json_summary = RunJsonSummary::new(json, workflow.as_deref());
    // A detached background child removes the pid file its launcher wrote
    // when this run finishes; a foreground run gets an inert guard.
    let _background_pid_file = crate::background::BackgroundPidFileGuard::from_env();

    // `-j 0` means "no explicit concurrency limit": clamp once at the
    // boundary so every downstream consumer — the scheduler submit cap,
    // the run-loop semaphore, and the executor's own semaphore (which
    // would otherwise be a zero-permit gate that hangs the first
    // submission) — sees a consistent value (issue #136 fix 1).
    let jobs = jobs.max(1);

    // ── Repository-URL workflows (nextflow-style `run <repo>`) ──────────
    // `oxo-flow run gh:owner/repo[@ref]` / `owner/repo[@ref]` (or a *.git
    // URL / local repo dir) checks out into <cwd>/.oxo-flow/repos/<owner>-<name>
    // (reused on later runs).
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
    json_summary.set_workflow(&workflow);
    // A#2 (persona campaign): name the workflow file once, right after
    // resolution — auto-discovery would otherwise pick a file silently when
    // no path was passed, and repo checkouts extract to a temp path the user
    // never typed. The run log is not armed yet at this point (it is created
    // further down), so this stays stderr-only for now.
    diagnostic_narrate(
        format_args!(
            "{} Using workflow file: {}",
            "Info:".bold().cyan(),
            workflow.display()
        ),
        None,
    );
    // Issue #427: everything downstream — base_dir, workflow_root,
    // runtime_workdir comparison in expand.rs, reference anchoring, env
    // resolution — compares against this directory. A bare `wf.oxoflow`
    // resolves its parent to ".", so workflow_root = Some(".") anchors
    // inputs to "./data/…" which still fails from the run workdir.
    // Absolutize once; relative workflow_dir only resolves from the
    // invocation cwd anyway.
    let workflow_dir = normalize_dots(absolutize(oxo_flow_core::parent_dir(&workflow))?);
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
    // Issue #427: absolutize the effective workdir once — the lock file
    // path, workflow_root comparison, and expand.rs's runtime_workdir
    // comparison all want canonical forms.
    let workdir_effective =
        normalize_dots(absolutize(workdir.as_ref().unwrap_or(&workdir_default))?);
    // Surface the lock error's suggestion (issue #158): without it a user
    // hitting a busy workdir learns WHAT is wrong but not that the lock
    // auto-releases when the other process exits.
    let _workdir_lock =
        WorkdirLock::acquire(&workdir_effective).map_err(|e| match e.suggestion() {
            Some(s) => anyhow::anyhow!("{e}\n  hint: {s}"),
            None => anyhow::anyhow!("{e}"),
        })?;

    // Issue #751: bundle runs extract the workflow into a temp dir, so
    // relative sample_pattern/pairs_pattern scans anchor inside the
    // extraction and never see data kept under --workdir. When the workflow
    // file lives in the extraction AND the effective workdir differs from
    // it, hand the workdir to the parser as a discovery fallback. (Without
    // an explicit --workdir, main.rs passes the extraction dir itself, so
    // the two are equal and the fallback is a no-op.)
    let discovery_fallback =
        if workflow.starts_with(std::env::temp_dir()) && workdir_effective != workflow_dir {
            Some(workdir_effective.as_path())
        } else {
            None
        };
    let mut config =
        WorkflowConfig::from_file_with_discovery_fallback(&workflow, discovery_fallback)
            .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // ── Parse and merge CLI config overrides (the shared phase, overrides.rs) ─────
    apply_cli_override_phase(&mut config, &workflow, cli_args, "run")?;

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
    // Issue #427: when the run workdir differs from the workflow directory,
    // tell expansion so relative input_groups disk-scan patterns emit
    // workflow-absolute paths — rule shells run with cwd = workdir, and
    // workflow-relative `{input}` would not resolve from there. Every
    // other expand caller (preview, validate, cluster, web) leaves this
    // unset and keeps raw emission. Absolutized so the expand.rs comparison
    // `runtime_workdir != base_dir` sees canonical forms on both sides.
    if let Some(effective_workdir) = workdir.as_ref() {
        config.runtime_workdir = Some(normalize_dots(absolutize(effective_workdir)?));
    }
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    // ── Undefined `{config.*}` gate (issue #142 H1) ───────────────────────
    // Runs AFTER expansion so engine-generated rules are covered too, and
    // before any file is touched — a typo'd key must fail the run, never
    // produce literal-placeholder outputs with exit 0.
    let (mut dag, target) = match build_dag_and_targets(&config, target, &module) {
        Ok(pair) => pair,
        // Pre-execution abort: nothing ran, but the summary contract still
        // holds for --json (issue #142 H6).
        Err(e) => {
            json_summary.emit("failed", &RunCounts::default(), vec![]);
            return Err(e);
        }
    };
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
    // The startup phase lives in run_log.rs (path resolution + masked
    // header + tee arming are unit-tested there).
    let RunLogStart {
        run_log,
        workflow_abs,
        workflow_git_sha,
    } = start_run_log(
        &workflow,
        &config,
        workdir.as_ref().unwrap_or(&workdir_default),
        log_file.as_deref(),
        &sensitive_values,
    )?;

    // When-aware planning (#882/#883): rules whose template `when` evaluates
    // false under the effective config seed the pruned set. Targeted runs
    // close over the INSTANTIATED DAG (issue #247) — instances whose `when`
    // evaluates false never enter the execution set, and the closure does
    // not expand through them. Full runs apply the same seeds so consumers
    // of when-skipped producers get pruned instead of failing at runtime.
    let wildcard_values: HashMap<String, String> = config_placeholder_values(&config.config);
    let when_false_rules =
        crate::commands::run_preview::when_false_rules(&config, &wildcard_values);

    // Default targets (issue #469): `target = true` rules seed the build
    // when nothing explicit was selected. Explicit -t/--module wins.
    let target = resolve_default_targets(&config, &target);
    let mut order = execution_order_skipping_with_notes(
        &dag,
        &target,
        &when_false_rules,
        workdir.as_ref().unwrap_or(&workdir_default),
        Some(&run_log),
        true,
    )?;
    if !target.is_empty() && order.is_empty() {
        json_summary.emit("failed", &RunCounts::default(), vec![]);
        return Err(anyhow::anyhow!(
            "all requested targets are when-gated false — nothing to run"
        ));
    }
    // ── Workdir source linking (issue #837) ────────────────────────────
    // In the two-directory mode (shared workflow repo + separate workdir)
    // rules run with CWD = workdir, so relative paths resolve against the
    // WORKDIR copy of a repo file — which operators used to hand-sync with
    // nothing verifying the copies. The engine now materializes referenced
    // repo files in the workdir as SYMLINKS: zero duplicated content, the
    // repo stays the single source of truth. This runs BEFORE the
    // missing-source gate so a fresh workdir is satisfied by the freshly
    // created links instead of hard-failing on absent source inputs.
    let link_report = crate::commands::workdir_sources::link_workflow_sources(
        &config,
        &dag,
        &order,
        &workflow_dir,
        workdir.as_ref().unwrap_or(&workdir_default),
        &config_placeholder_values(&config.config),
    );
    narrate_source_link_report(&link_report, Some(&run_log));
    // ── Missing external source gate ───────────────────────────────────
    // A required input with no producer in the DAG is a source file — if it
    // is absent NOW the run can never succeed, so fail before scheduling
    // instead of exiting 0 with stale outputs. Generated intermediates are
    // excluded (they have producers and are owned by the manifest cascade).
    let missing_sources = missing_source_findings(
        &config,
        &dag,
        &order,
        workdir.as_ref().unwrap_or(&workdir_default),
        &workflow_dir,
        &config_placeholder_values(&config.config),
    );
    if !missing_sources.is_empty() {
        json_summary.emit("failed", &RunCounts::default(), vec![]);
        return Err(anyhow::anyhow!(
            "workflow is missing {} required source input{} — no rule produces {}:\n  {}",
            missing_sources.len(),
            if missing_sources.len() == 1 { "" } else { "s" },
            if missing_sources.len() == 1 {
                "it"
            } else {
                "them"
            },
            missing_sources.join("\n  ")
        ));
    }
    emit_execution_event(oxo_flow_core::executor::ExecutionEvent::WorkflowStarted {
        workflow_name: config.workflow.name.clone(),
        total_rules: order.len(),
    });
    notify_webhook(
        &config,
        oxo_flow_core::webhook::WebhookEvent::WorkflowStarted,
        None,
    )
    .await;
    if !crate::commands::is_quiet()
        && let Err(e) = write_execution_plan(&mut std::io::stderr().lock(), &order, &config)
    {
        tracing::trace!(error = %e, "execution plan console output dropped (stderr unavailable)");
    }

    // ── Load checkpoint + config-change impact analysis (issue #62) ─────
    // Detection must happen AFTER overrides/profile merges and wildcard
    // expansion (config.config is final) and BEFORE ExecutorConfig
    // construction (the invalidation set must reach the executor's
    // freshness gate, which would otherwise silently skip re-submitted
    // rules with stale outputs).
    // An explicit checkpoint path (resume) wins over the workdir-derived
    // default (#538); load AND save both use it.
    let checkpoint_path = checkpoint_override.unwrap_or_else(|| {
        CheckpointState::default_path(workdir.as_ref().unwrap_or(&workdir_default))
    });
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

    // Issue #432(a): a corrupt checkpoint must never degrade silently into
    // an empty state — that silently re-executes every completed rule, which
    // costs hours on real data. Fail loudly BEFORE any work is scheduled so
    // the user can back up / repair the file rather than lose the credit.
    // The shared load phase (findings.rs): corruption refuses to start
    // (issue #432(a)), adoption notes reach the run log, stale `running`
    // entries from a dead previous run are cleared and persisted (issue
    // #685) so the reuse block and resume accounting stay honest, loudly.
    let (loaded_checkpoint, stale_running) = load_checkpoint_for_command(
        &checkpoint_path,
        &workflow_abs,
        CheckpointLoadMode::Execute,
        Some(&run_log),
    )?;
    if !stale_running.is_empty() {
        diagnostic_narrate(
            format_args!(
                "  {} checkpoint lists {} as running from a previous run \
                 ({}): they died with it and will re-run.",
                "!".yellow(),
                oxo_flow_core::plural(stale_running.len(), "rule", "rules"),
                stale_running.join(", ")
            ),
            Some(&run_log),
        );
    }
    let checkpoint: Arc<Mutex<CheckpointState>> = Arc::new(Mutex::new(loaded_checkpoint));

    // Sensitive keys are needed by the snapshot-drift warning below and by
    // config-change detection.
    let sensitive_keys = sensitive_keys_of(&config);

    // ── Snapshot drift warning (issue #243) ──────────────────────────────
    // The rationale and the masking rule live on the shared finding
    // (findings::config_snapshot_drift).
    {
        let ck = checkpoint.lock().await;
        config_snapshot_drift(&ck, &config, &sensitive_keys);
    }

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
        // Record resolved include provenance (issue #899): per repo-include,
        // the repo@ref and the checkout's HEAD SHA at parse time. Empty for
        // workflows without repo includes — a no-op in that case.
        ck.set_include_resolutions(&config.include_provenance);
        ck.set_workdir(&absolutize(workdir.as_deref().unwrap_or(&workdir_default))?);
    }
    // Same absolutized run workdir, reused by the config-change replay below
    // (issue #282): runtime file functions in `when` resolve their relative
    // paths against it so threshold changes get real verdicts.
    let run_workdir = absolutize(workdir.as_deref().unwrap_or(&workdir_default))?;

    // Changed config keys → only rules referencing them (plus DAG downstream)
    // are invalidated; edited rule definitions are caught by fingerprints.
    // Engine-injected keys (samples_list, samples_<group>) are excluded:
    // their --samples churn must not invalidate everything.
    let old_snapshot = {
        let ck = checkpoint.lock().await;
        ck.config_snapshot.clone()
    };
    let (change_report, mut force_rules, completed_in_run) = {
        let mut ck = checkpoint.lock().await;
        let report = oxo_flow_core::config_impact::detect_config_changes_with_replay(
            &mut ck,
            &config.rules,
            &dag,
            &config.config,
            &sensitive_keys,
            &config.workflow.interpreter_map,
            config.defaults.shell_prelude.as_deref(),
            Some(&workflow_dir),
            Some(&run_workdir),
            Some(&config),
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
        Some(&run_log),
    );

    // Issue #142 M1: rules whose fingerprint differed only in the
    // sample-derived input list are NOT invalidated — toggling --samples
    // must not re-run (and overwrite) cohort-level gather outputs. The
    // input-manifest check re-verifies set + content below, so a genuine
    // input edit still invalidates there.
    if !change_report.sample_selection_exempt.is_empty() {
        diagnostic_narrate(
            format_args!(
                "  {} skipped {} whose definition only changed with the --samples selection: {} — outputs still cover the previous run's full sample set; use --rerun to regenerate with the new selection",
                "⚠".yellow(),
                oxo_flow_core::plural(change_report.sample_selection_exempt.len(), "rule", "rules"),
                change_report.sample_selection_exempt.join(", ")
            ),
            Some(&run_log),
        );
    }

    // All config values (including CLI --arg overrides) become {config.key} in templates.
    let wildcard_values: Arc<HashMap<String, String>> =
        Arc::new(config_placeholder_values(&config.config));
    let workdir_actual = Arc::new(workdir.as_ref().unwrap_or(&workdir_default).clone());

    // `[engine] tmp_dir` resolved once here so both the executor config and
    // the workflow-level terminal hooks route scratch identically (issue #907).
    let tmp_dir_resolved = tmp_dir_resolved(
        config.engine.as_ref().and_then(|e| e.tmp_dir.clone()),
        &workflow_dir,
    )?;
    // ── Pre-flight disk check (issue #75) ─────────────────────────────────
    warn_low_free_space(workdir_actual.as_path());

    // ── Input manifest comparison (issue #72) ──────────────────────────────
    // The rationale and the narration live on the shared phase
    // (findings::apply_input_manifest_phase); the fail-gate below reads the
    // returned protected-drift evidence.
    let input_manifest = {
        let mut ck = checkpoint.lock().await;
        apply_input_manifest_phase(
            &mut ck,
            &config,
            &dag,
            &order,
            workdir_actual.as_ref(),
            &wildcard_values,
            &checkpoint_path,
            Some(&run_log),
        )
    };
    force_rules.extend(input_manifest.force_rules.iter().cloned());
    let protected_drift_lines = input_manifest.protected_drift_lines;
    let protected_drift = input_manifest.protected_drift;

    // ── Protected-tree drift gate (issue #935 P1): fail the run ──────────
    // Detection may not proceed silently: damaged protected data invalidates
    // the premise of every downstream rule. The flag is an explicit operator
    // decision, not a default.
    if protected_drift && !allow_protected_drift {
        let detail = protected_drift_lines.join("\n  ");
        return Err(anyhow::anyhow!(
            "protected data tree drift detected ({} file(s) changed since baseline):\n  {}\n\nRefusing to run against changed protected data. Restore the files or pass --allow-protected-drift to proceed anyway.",
            protected_drift_lines.len(),
            detail
        ));
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
            diagnostic_narrate(
                format_args!(
                    "  {} temporary outputs needed again — re-running {}: {}",
                    "↻".yellow(),
                    oxo_flow_core::plural(upstream.len(), "producer rule", "producer rules"),
                    upstream.join(", ")
                ),
                Some(&run_log),
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
        // A typo'd --timeout must fail fast: silently disabling the timeout
        // turns "kill the job after 30m" into "let it run forever", which is
        // the opposite of what the flag promises (audit finding).
        parse_duration_secs(&timeout).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid --timeout value '{timeout}'.\n  \
                 Expected seconds (e.g. 3600) or a duration like 1h / 30m / 2d."
            )
        })?
    };

    if let Some(cluster_profile) =
        cluster_path::routed_cluster_profile(profile.as_deref(), config.cluster.as_ref())
    {
        cluster_path::dispatch_cluster_run(
            cluster_profile,
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
            &mut json_summary,
            &workflow,
            no_report_snapshot,
        )
        .await?;
        return Ok(());
    }

    let exec_config = ExecutorConfig {
        max_jobs: jobs,
        dry_run: false,
        // Issue #427: absolutized — the executor's rule shells run with
        // cwd = workdir, and anchored {input}/{log} rendering assumes a
        // usable base. A raw relative value would only resolve from the
        // invocation directory.
        workdir: match workdir.as_ref() {
            Some(p) => normalize_dots(absolutize(p)?),
            None => workdir_default.clone(),
        },
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
        // Unconstrained wildcards fall back to the safe default charset
        // inside validate_wildcard_injection (issue #203).
        wildcard_constraints: config.wildcard_constraints.clone(),
        // `file_exists(...)` in `when` conditions resolves relative paths
        // against the workflow root (issue #241) — same as plan time.
        base_dir: Some(workflow_dir.clone()),
        // Issue #427: file-backed env specs and workflow-shipped rule
        // inputs/scripts anchor to the workflow directory whenever the
        // run workdir differs from it — rule shells run with cwd = workdir.
        // workdir_effective is a CLI value here or workflow_dir itself;
        // both sides of the comparison are absolute (workflow_dir was
        // absolutized at the top), so a relative --workdir can't fool it.
        workflow_root: if *workdir_effective != workflow_dir {
            Some(workflow_dir.clone())
        } else {
            None
        },
        // Shared with the manifest snapshot resolver so staging and
        // invalidation always see the same backends (issue #80 item 2).
        storage_resolver: crate::commands::run_preview::storage_resolver(),
        // Checkpoint excerpt size, honoring OXO_FLOW_OUTPUT_TAIL_BYTES (also
        // read by the signal handler and cluster paths, which have no
        // ExecutorConfig here).
        output_tail_bytes: output_tail_bytes_from_env(),
        // The freshness gate reads recorded provenance checksums from the
        // live checkpoint (issue #194 B2).
        checkpoint: Some(checkpoint.clone()),
        // `[engine] tmp_dir` → global scratch routing (issue #907): relative
        // paths anchor to the workflow directory (same root as file-backed
        // env specs), absolute paths pass through. OXO_FLOW_TMPDIR wins when
        // set, matching the env-override pattern of OXO_FLOW_OUTPUT_TAIL_BYTES.
        // Hoisted so the workflow-level terminal hooks route their scratch
        // through the same directory.
        tmp_dir: tmp_dir_resolved.clone(),
        hardware_topology: if config
            .engine
            .as_ref()
            .and_then(|e| e.numa_aware)
            .unwrap_or(false)
            || config
                .rules
                .iter()
                .any(|r| r.resources.numa_pin || r.resources.numa_node.is_some())
        {
            Some(oxo_flow_core::hardware::HardwareTopology::discover())
        } else {
            None
        },
        storage_tiers: Some(
            oxo_flow_core::hardware::StorageTierLadder::from_engine_config(config.engine.as_ref()),
        ),
    };

    // Fail fast if any rule's declared request can never fit an explicit
    // --max-memory / --max-threads cap. Otherwise the run would execute earlier
    // rules and only discover the impossible one mid-pipeline.
    let scheduled: Vec<&oxo_flow_core::rule::Rule> = order
        .iter()
        .filter_map(|name| config.get_rule(name))
        .collect();
    if let Err(e) = validate_resource_budget(
        &scheduled,
        exec_config.max_threads,
        exec_config.max_memory_mb,
    ) {
        progress.finish_and_clear();
        json_summary.emit("failed", &RunCounts::default(), vec![]);
        return Err(e);
    }

    // Disk pre-flight (issue #75) and runtime disk-pressure monitor pre-flight (#843).
    warn_disk_preflight(&scheduled, workdir_actual.as_ref(), config.engine.as_ref());

    // Backend availability pre-flight: a pending rule declaring an
    // environment whose backend binary is missing used to surface only
    // mid-run as the backend's own "command not found" buried in that
    // rule's stderr log (live: a missing pixi discovered hours into a
    // cohort run). Reuse the exact validator `env check` uses and fail
    // before executing anything when a pending rule's backend is unusable
    // — mirroring the budget pre-flight above. Skipped/cached rules do not
    // block: this run never invokes their backend.
    let pending_envs = {
        let ck = checkpoint.lock().await;
        let preview = crate::commands::run_preview::preview_run_plan(
            &ck,
            &config,
            &dag,
            &order,
            workdir_actual.as_ref(),
            &wildcard_values,
            &sensitive_keys,
            &config.workflow.interpreter_map,
            &checkpoint_path,
            rerun,
            resume_failed,
        );
        pending_rule_environments(&preview.plan, &config, &workflow_dir)
    };
    if let Err(e) = validate_pending_backends(
        &pending_envs,
        &oxo_flow_core::environment::EnvironmentResolver::new(),
    ) {
        progress.finish_and_clear();
        json_summary.emit("failed", &RunCounts::default(), vec![]);
        return Err(e);
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
    // Checksums recorded under --provenance across the whole run: when the
    // flag was set but nothing landed, the summary must say so loudly
    // instead of leaving `provenance verify` to puzzle the user (issue #276).
    let provenance_recorded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // Failures of `required = false` rules (issue #99 B2): counted
    // separately, surfaced in the summary, but exempt from failing the run.
    let non_required_fail_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let skipped_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed_rules_set: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));
    // Rules skipped at execution time without producing outputs (e.g.
    // "optional inputs missing"). Non-optional dependents are blocked from
    // running on them — the declared input files can never exist, so
    // executing them would just produce a missing-file shell error
    // (issue #200, live: eager's samtools_flagstat after a skipped filter).
    let skipped_no_output: Arc<Mutex<std::collections::HashSet<String>>> =
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
    // The build loop, legacy-state migration, and consumer invalidation
    // live in refs::auto_build_references.
    auto_build_references(
        &config,
        &dag,
        &checkpoint,
        &checkpoint_path,
        workdir.as_ref().unwrap_or(&workdir_default),
        &workflow_dir,
        &wildcard_values,
        &sensitive_values,
        &env_cache_dir,
        &mut json_summary,
        skip_ref_build,
    )
    .await?;

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
            replay_checkpoint_reentries(
                &mut ReplayPlan {
                    config: &mut config,
                    dag: &mut dag,
                    order: &mut order,
                    target: &target,
                    when_false_rules: &when_false_rules,
                    workdir: workdir.as_ref().unwrap_or(&workdir_default),
                    run_log: Some(&run_log),
                },
                &ck.reentries,
                &valid,
                ReplayPlanOrdering::ExecutionOrder,
                "run",
                false,
            )?;
        }
    }

    // ── Runtime-discovered output_pattern replay (issue #227 item 5) ──────
    // Rationale and producer completion gating live in
    // `reentry::replay_output_pattern_domains`.
    {
        let ck = checkpoint.lock().await;
        replay_output_pattern_domains(
            &mut ReplayPlan {
                config: &mut config,
                dag: &mut dag,
                order: &mut order,
                target: &target,
                when_false_rules: &when_false_rules,
                workdir: workdir.as_ref().unwrap_or(&workdir_default),
                run_log: Some(&run_log),
            },
            &ck,
            rerun,
            ReplayPlanOrdering::ExecutionOrder,
        )?;
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
    // Task id → abort handle (issue #872): the watchdog requeue aborts a
    // specific stuck spawn task; `JoinSet` itself cannot abort by id.
    let mut task_abort: std::collections::HashMap<tokio::task::Id, tokio::task::AbortHandle> =
        std::collections::HashMap::new();

    // Pre-process checkpoint-completed rules so they are never re-submitted.
    {
        let ck = checkpoint.lock().await;
        for rule_name in &order {
            // A rule still marked running (issue #685) is never "already
            // completed": the previous run died with it mid-flight, its
            // outputs may be half-written, and treating it as done would
            // silently skip real work.
            if ck.is_running(rule_name) {
                continue;
            }
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
                        signal: None,
                        rule: rule_name.clone(),
                        status: oxo_flow_core::executor::JobStatus::Success,
                        enqueued_at: None,
                        started_at: None,
                        finished_at: None,
                        exit_code: Some(0),
                        stdout: None,
                        stderr: None,
                        command: None,
                        retries: 0,
                        skip_reason: None,
                        max_rss_mb: None,
                        cpu_seconds: None,
                        caption: config.get_rule(rule_name).and_then(|r| {
                            oxo_flow_core::executor::process::rule_report_caption(
                                r,
                                workdir_actual.as_ref(),
                            )
                        }),
                    });
                    skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !is_tty {
                        diagnostic_narrate(
                            format_args!("  {} {} (already completed)", "⊝".dimmed(), rule_name),
                            Some(&run_log),
                        );
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
    let task_ctx = Arc::new(TaskContext {
        executor: executor.clone(),
        checkpoint: checkpoint.clone(),
        checkpoint_path: checkpoint_path.clone(),
        workdir_actual: workdir_actual.clone(),
        task_run_log: Arc::clone(&run_log),
        sensitive_values: sensitive_values.clone(),
        failures: failures.clone(),
        failed_rules_set: failed_rules_set.clone(),
        skipped_no_output: skipped_no_output.clone(),
        success_count: success_count.clone(),
        fail_count: fail_count.clone(),
        non_required_fail_count: non_required_fail_count.clone(),
        skipped_count: skipped_count.clone(),
        provenance_recorded: provenance_recorded.clone(),
        provenance,
        keep_going,
        is_tty,
        semaphore: semaphore.clone(),
    });

    let mut waited_rounds: std::collections::HashMap<String, i32> = Default::default();

    // Spawn watchdog (issue #685): wall-clock time each rule was marked
    // running, used to detect rules stuck in Running without ever spawning
    // a child process (lost wakeup wedge). Entries are removed in the
    // common completion funnel below; cleanup on every terminal path.
    let mut running_since: std::collections::HashMap<String, std::time::Instant> =
        Default::default();
    // Watchdog fire counters (issue #872): the watchdog re-arms every
    // threshold interval instead of reporting once per rule per run, and
    // counts consecutive fires per rule. A rule reaching
    // `watchdog_requeue_fires` on the ERROR branch (dispatched but never
    // forked) is requeued to Pending; queued (WARN) rules never escalate.
    let mut watchdog_fires: std::collections::HashMap<String, u32> = Default::default();
    let watchdog_requeue_fires = watchdog_requeue_fires();
    let watchdog_threshold = spawn_watchdog_threshold();
    let mut watchdog_tick =
        tokio::time::interval(std::time::Duration::from_secs(SPAWN_WATCHDOG_TICK_SECS));

    // #843 disk-pressure monitor: level thresholds resolved once from the
    // validated config (None = feature off — zero monitoring overhead).
    let disk_thresholds = config.engine.as_ref().and_then(|e| e.disk_thresholds());
    let mut disk_monitor = oxo_flow_core::scheduler::DiskPressureMonitor::new(
        disk_thresholds.map_or(0, |t| t.0),
        disk_thresholds.map_or(0, |t| t.1),
    );
    let mut disk_tick = tokio::time::interval(std::time::Duration::from_secs(disk_tick_secs()));
    // Grace rounds accumulated while parked under Hold with nothing in
    // flight; disk recovery resets it.
    let mut disk_hold_rounds: u32 = 0;
    let disk_hold_abort_rounds = disk_hold_abort_rounds();

    // ---- main event loop -------------------------------------------------

    // Termination signals (audit C14): SIGINT/SIGTERM/SIGHUP stop the run
    // through `terminate_on_signal` instead of killing the process outright.
    // A watch channel keeps the wait re-armable across loop iterations, the
    // way a oneshot receiver could only be awaited once.
    let (signal_tx, mut signal_rx) = tokio::sync::watch::channel(None::<InterruptSignal>);
    tokio::spawn(async move {
        if let Some(signal) = wait_for_termination_signal().await
            && signal_tx.send(Some(signal)).is_err()
        {
            // The run loop is gone but the handlers are still installed, so
            // a late signal must still end the process.
            std::process::exit(signal.exit_code());
        }
    });

    loop {
        // #843 disk-pressure gate — measure BEFORE any dispatch decision so
        // a floor crossing narrates once (crossing dedup inside the monitor)
        // and enforces its policy this round.
        if disk_thresholds.is_some() {
            let mut n = |args: std::fmt::Arguments<'_>| {
                diagnostic_narrate(args, Some(&run_log));
            };
            let level = disk_monitor.observe(workdir_actual.as_ref(), &mut n);
            let hold = level >= oxo_flow_core::scheduler::DiskPressureLevel::Hold;

            // Reclaim BEFORE the Hold-park decision (review F1): a run that
            // starts below the floor — or falls through to Hold within a
            // single tick — must still get a reclaim pass before parking,
            // or the "nothing left to reclaim" abort below is untrue and
            // reclaimable intermediates are never released.
            if level >= oxo_flow_core::scheduler::DiskPressureLevel::Reclaim {
                // Completed temporary rules whose dependents are all done
                // no longer need their outputs.
                let wildcards = wildcard_values.clone();
                let candidates = {
                    let ck = checkpoint.lock().await;
                    oxo_flow_core::scheduler::reclaim_candidates(
                        &config.rules,
                        &dag,
                        &ck,
                        workdir_actual.as_ref(),
                        &wildcards,
                        &config.expansion_templates,
                    )
                };
                for candidate in candidates {
                    if let Err(reclaim_err) = reclaim_rule_outputs(
                        &candidate,
                        workdir_actual.as_ref(),
                        &checkpoint,
                        &checkpoint_path,
                        &run_log,
                    )
                    .await
                    {
                        diagnostic_narrate(
                            format_args!(
                                "  {} reclaim of '{}' failed: {reclaim_err}",
                                "!".yellow(),
                                candidate.rule
                            ),
                            Some(&run_log),
                        );
                    }
                }
            }
            if hold && !sched.is_complete() && sched.running_count() == 0 && join_set.is_empty() {
                // Nothing in flight and nothing dispatchable: park for a
                // tick (external space may free up) and re-measure. After
                // disk_hold_abort_rounds consecutive parked rounds below the
                // floor, abort budget-abort style — reclaim has already fired
                // above, before the park decision; if the disk is still full,
                // no progress is possible.
                if disk_hold_rounds >= disk_hold_abort_rounds {
                    {
                        let ck = checkpoint.lock().await;
                        ck.save_to_file_async(&checkpoint_path).await.ok();
                    }
                    progress.finish_and_clear();
                    json_summary.emit("failed", &RunCounts::default(), vec![]);
                    anyhow::bail!(
                        "disk pressure: {}MB free is below min_free_disk ({}MB) with no rules \
                         in flight and nothing left to reclaim — the run cannot make progress. \
                         Free space manually (delete intermediates or run `oxo-flow clean`), \
                         then resume from the checkpoint, or lower [engine] min_free_disk.",
                        oxo_flow_core::scheduler::check_available_disk_mb(workdir_actual.as_ref())
                            .unwrap_or(0),
                        disk_thresholds.map_or(0, |t| t.0),
                    );
                }
                disk_hold_rounds += 1;
                tokio::time::sleep(std::time::Duration::from_secs(disk_tick_secs())).await;
                continue;
            }
            // Any pass that is not a parked hold clears the grace counter.
            disk_hold_rounds = 0;
        }

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
        // Under #843 Hold, new dispatches are withheld entirely — in-flight
        // rules continue to completion.
        let available = if disk_thresholds.is_some()
            && disk_monitor.last_level() >= oxo_flow_core::scheduler::DiskPressureLevel::Hold
        {
            0
        } else {
            jobs.saturating_sub(sched.running_count())
        };
        let to_submit: Vec<String> = ready.iter().take(available).cloned().collect();
        for name in ready.iter().skip(available) {
            *waited_rounds.entry(name.clone()).or_insert(0) += AGING_STEP;
        }
        for rule_name in &to_submit {
            // Skip rules blocked by a failed — or no-output-skipped — upstream
            // dependency. Optional rules are exempt: they evaluate their own
            // inputs and skip gracefully when they're absent (issue #200).
            let rule_is_optional = config
                .get_rule(rule_name)
                .is_some_and(|r| r.optional.is_optional());
            let blocked_by = if rule_is_optional {
                None
            } else {
                let frs = failed_rules_set.lock().await;
                let sno = skipped_no_output.lock().await;
                dag.dependencies(rule_name).ok().and_then(|deps| {
                    deps.into_iter()
                        .find(|d| frs.contains(d.as_str()) || sno.contains(d.as_str()))
                })
            };
            if let Some(dep) = blocked_by {
                skipped_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // Propagate transitively: this rule produced nothing, so its
                // own dependents are blocked the same way.
                let dep_skipped = {
                    let sno = skipped_no_output.lock().await;
                    sno.contains(dep.as_str())
                };
                if dep_skipped {
                    let mut sno = skipped_no_output.lock().await;
                    sno.insert(rule_name.clone());
                } else {
                    let mut frs = failed_rules_set.lock().await;
                    frs.insert(rule_name.clone());
                }
                blocked.lock().await.push((rule_name.clone(), dep));
                submitted.insert(rule_name.clone());
                sched.mark_completed(oxo_flow_core::executor::JobRecord {
                    signal: None,
                    rule: rule_name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Skipped,
                    enqueued_at: None,
                    started_at: None,
                    finished_at: None,
                    exit_code: None,
                    stdout: None,
                    stderr: None,
                    command: None,
                    retries: 0,
                    skip_reason: Some(
                        if dep_skipped {
                            "blocked by skipped upstream dependency"
                        } else {
                            "blocked by failed upstream dependency"
                        }
                        .into(),
                    ),
                    max_rss_mb: None,
                    cpu_seconds: None,
                    caption: config.get_rule(rule_name).and_then(|r| {
                        oxo_flow_core::executor::process::rule_report_caption(
                            r,
                            workdir_actual.as_ref(),
                        )
                    }),
                });
                if !is_tty {
                    narrate_console_line(format_args!(
                        "  {} {} (blocked by {} dependency)",
                        "⊘".yellow(),
                        rule_name,
                        if dep_skipped { "skipped" } else { "failed" }
                    ));
                }
                progress.inc(1);
                continue;
            }

            submitted.insert(rule_name.clone());
            sched.mark_running(rule_name);
            // Spawn-time persistence (issue #685, ask 3): the checkpoint's
            // running set is written when the rule actually enters flight,
            // so a crashed run leaves an honest record for the next start.
            {
                let mut ck = checkpoint.lock().await;
                ck.mark_running(rule_name);
                if let Err(e) = ck.save_to_file_async(&checkpoint_path).await {
                    tracing::warn!("Failed to save checkpoint (running mark): {e}");
                }
            }
            running_since.insert(rule_name.clone(), std::time::Instant::now());

            // ---- spawn task via execute_rule_task (task.rs) -----
            let Some(rule) = config.get_rule(rule_name).cloned() else {
                anyhow::bail!("rule '{rule_name}' not found in workflow");
            };
            let instance_bindings = config.instance_bindings(rule_name);
            progress.set_message(format!("executing {}", rule_name));
            if !is_tty {
                progress_narrate(
                    format_args!("  {} {}", "Running:".bold().cyan(), rule_name),
                    &run_log,
                );
            }
            emit_execution_event(oxo_flow_core::executor::ExecutionEvent::RuleStarted {
                rule: rule_name.clone(),
                command: None,
            });

            let typed_config = config.config.clone();
            let rule_caption = config.get_rule(rule_name).and_then(|r| {
                oxo_flow_core::executor::process::rule_report_caption(r, workdir_actual.as_ref())
            });
            let when_gated_off = rule
                .when
                .as_deref()
                .is_some_and(|when| {
                    !when.contains('{')
                        && !oxo_flow_core::executor::process::evaluate_condition_with_wildcards_and_base_dir(
                            when,
                            &config.config,
                            &wildcard_values,
                            Some(workdir_actual.as_ref().as_path()),
                        )
                });
            let task_rule_name = rule_name.clone();
            let handle = join_set.spawn(execute_rule_task(
                TaskInvocation {
                    rule,
                    rule_caption,
                    when_gated_off,
                    instance_bindings,
                    wildcard_values: wildcard_values.clone(),
                    typed_config,
                },
                task_ctx.clone(),
            ));
            task_rule.insert(handle.id(), task_rule_name);
            task_abort.insert(handle.id(), handle);
        }

        // ---- wait for completions -----------------------------------------

        // If nothing is in-flight, we're done (all rules completed, failed, or blocked).
        if join_set.is_empty() {
            break;
        }

        // Wait for the next rule to finish — or for a termination signal
        // (audit C14): the in-flight rules must be torn down and the
        // checkpoint persisted before the process exits, instead of the
        // default disposition killing oxo-flow outright and orphaning every
        // child (PPID 1) with a half-written checkpoint.
        let join_result = tokio::select! {
            // `biased` with join-next first: a result that is already
            // reapable is ALWAYS drained before the signal branch can be
            // taken. Without this, a signal landing in the same poll
            // cycle as a completion races it at random — the teardown
            // would then see the finished rule's id still in `task_rule`
            // and record it Failed in the checkpoint.
            biased;
            result = join_set.join_next_with_id() => Some(result),
            received = async {
                match signal_rx.changed().await {
                    Ok(()) => *signal_rx.borrow_and_update(),
                    // The watcher died before the run did; the run then
                    // keeps the signals' default dispositions.
                    Err(_) => None,
                }
            } => match received {
                Some(signal) => {
                    terminate_on_signal(
                        signal,
                        &executor,
                        &mut join_set,
                        task_rule.values(),
                        &checkpoint,
                        &checkpoint_path,
                        &mut json_summary,
                        &config,
                    )
                    .await
                }
                // The watcher died before the run did; the run then keeps
                // the signals' default dispositions.
                None => None,
            },
            // Spawn watchdog (issue #685): a rule stuck in Running without
            // ever spawning a child process wedged the engine in a futex
            // wait (empty checkpoint running-set, zero completions). Every
            // SPAWN_WATCHDOG_TICK_SECS, compare the scheduler's running set
            // with the executor's live child pids: any rule running longer
            // than the threshold is re-reported each interval, with the
            // consecutive-fire count. Issue #847 splits the report: a rule
            // still queued in the resource pool is a benign wait — the
            // scheduler's own 60s blockage WARNs already narrate holders
            // and FIFO position — so it only earns a WARN here and never
            // escalates. The #685 ERROR is reserved for rules NOT queued in
            // the pool (env-setup stragglers have their own throttled WARN;
            // anything else is the genuine lost-wakeup signature). After
            // `watchdog_requeue_fires` consecutive ERROR fires the rule is
            // requeued to Pending (issue #872): both live wedge recoveries
            // needed ~9× the threshold to self-heal, so a bounded requeue
            // beats indefinite lane-freezing keep-waiting.
            _ = watchdog_tick.tick() => {
                let live_rules: std::collections::HashSet<String> = executor
                    .active_pids()
                    .into_iter()
                    .map(|(rule, _)| rule)
                    .collect();
                let queued: std::collections::HashSet<String> =
                    executor.resource_waiters().await.into_iter().collect();
                let n = |args: std::fmt::Arguments<'_>| {
                    diagnostic_narrate(args, Some(&run_log));
                };
                for rule in sched.running_rules() {
                    if live_rules.contains(&rule) {
                        watchdog_fires.remove(&rule);
                        continue;
                    }
                    let Some(started) = running_since.get(&rule) else {
                        continue;
                    };
                    if started.elapsed() < watchdog_threshold {
                        continue;
                    }
                    let running_secs = started.elapsed().as_secs();
                    if queued.contains(&rule) {
                        let fires = watchdog_fires.entry(rule.clone()).or_insert(0);
                        *fires += 1;
                        n(format_args!(
                            "{} rule '{rule}' has been Running for {running_secs}s while \
                             queued in the resource pool — benign wait (watchdog fire {}), \
                             the scheduler reports blockage details every 60s (issue #847). \
                             The engine will keep waiting.",
                            "Watchdog:".yellow().bold(),
                            *fires
                        ));
                        continue;
                    }
                    // Not queued and no live child: dispatched but never
                    // forked (issue #872 dispatch-state wording).
                    let fires = watchdog_fires.entry(rule.clone()).or_insert(0);
                    *fires += 1;
                    if *fires < watchdog_requeue_fires {
                        n(format_args!(
                            "{} rule '{rule}' has been Running for {running_secs}s but never \
                             spawned a child process (dispatched but not forked, fire {} of {}) \
                             — likely causes: environment setup still in progress, or a lost \
                             scheduler wakeup (issue #685). The engine will keep waiting; at \
                             fire {} the rule is requeued to Pending.",
                            "Watchdog ERROR:".yellow().bold(),
                            *fires,
                            watchdog_requeue_fires,
                            watchdog_requeue_fires
                        ));
                        tracing::error!(
                            rule = %rule,
                            running_secs,
                            fires = *fires,
                            state = "dispatched-not-forked",
                            "rule has been Running for over {}s but never spawned a child \
                             process (issue #685)",
                            watchdog_threshold.as_secs()
                        );
                        continue;
                    }
                    // Escalation: requeue the stuck rule (issue #872). Abort
                    // its spawn task so the submit-cap slot is freed; the
                    // reap path routes the resulting non-panic cancellation
                    // to a no-op — all cleanup happens here.
                    n(format_args!(
                        "{} rule '{rule}' has been Running for {running_secs}s but never \
                         spawned a child process after {} watchdog fires (~{}s) — requeueing \
                         it to Pending. Its submit slot is released and the rule is \
                         re-dispatched when it next becomes ready; if it wedges again the \
                         watchdog will re-report and requeue it (issue #872).",
                        "Watchdog ERROR:".yellow().bold(),
                        *fires,
                        running_secs
                    ));
                    tracing::error!(
                        rule = %rule,
                        running_secs,
                        fires = *fires,
                        state = "requeued",
                        "rule requeued to Pending after {} watchdog fires without a child \
                         process (issue #872)",
                        *fires
                    );
                    let stuck_id = task_rule
                        .iter()
                        .find_map(|(id, name)| (name == &rule).then_some(*id));
                    match stuck_id.and_then(|id| task_abort.get(&id).map(|h| (id, h))) {
                        Some((task_id, abort)) => {
                            // Tree-kill before abort: the wedged task may have
                            // spawned grand-children that kill_on_drop would
                            // orphan; they could race the re-dispatched rule's
                            // outputs. Snapshot the pid while the task is
                            // still live, then sweep after the abort — same
                            // shape as terminate_on_signal (#524) and the
                            // first-failure abort. The fresh-pid check here
                            // also closes the requeue-vs-reap race (review
                            // NIT 6): a rule reaped between snapshot and
                            // abort has no pid left to kill.
                            let stuck_pids: Vec<u32> = executor
                                .active_pids()
                                .into_iter()
                                .filter(|(name, _)| name == &rule)
                                .map(|(_, pid)| pid)
                                .collect();
                            abort.abort();
                            task_abort.remove(&task_id);
                            task_rule.remove(&task_id);
                            if !stuck_pids.is_empty() {
                                // kill_process_tree's grace poll sleeps up to
                                // 10 s — run the sweep on the blocking pool,
                                // not a runtime worker (#524).
                                let rule_name = rule.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    for pid in stuck_pids {
                                        if let Err(e) = oxo_flow_core::executor::timeout::
                                            kill_process_tree(pid)
                                        {
                                            tracing::warn!(
                                                rule = %rule_name,
                                                pid,
                                                "watchdog requeue: failed to kill stuck \
                                                 process tree: {e}"
                                            );
                                        }
                                    }
                                })
                                .await;
                            }
                        }
                        None => {
                            // No task link: the id→rule registration is the only
                            // path back to the spawn task. Without it we cannot
                            // free the submit slot, so fail the run loudly
                            // instead of silently shrinking the cap (mirrors the
                            // reap-path internal-error invariant).
                            json_summary.emit(
                                "failed",
                                &RunCounts {
                                    succeeded: success_count
                                        .load(std::sync::atomic::Ordering::Relaxed),
                                    skipped: skipped_count
                                        .load(std::sync::atomic::Ordering::Relaxed),
                                    failed: fail_count
                                        .load(std::sync::atomic::Ordering::Relaxed),
                                    non_required_failed: non_required_fail_count
                                        .load(std::sync::atomic::Ordering::Relaxed),
                                    blocked: blocked.lock().await.len(),
                                },
                                vec![],
                            );
                            return Err(anyhow::anyhow!(
                                "internal error: watchdog requeue found no task for rule '{rule}'"
                            ));
                        }
                    }
                    executor.clear_resource_waiters().await;
                    if let Some(rule_cfg) = config.get_rule(&rule) {
                        executor.release_resources(rule_cfg).await;
                    }
                    // The spawn task may still be reaped later this tick; the
                    // scheduler running-set entry it would otherwise hold is
                    // already cleared above, so dependents re-admit normally.
                    sched.reset_to_pending(&rule);
                    {
                        let mut ck = checkpoint.lock().await;
                        ck.clear_running(&rule);
                        if let Err(save_err) = ck.save_to_file_async(&checkpoint_path).await {
                            tracing::warn!("Failed to save checkpoint: {save_err}");
                        }
                    }
                    running_since.remove(&rule);
                    submitted.remove(&rule);
                    watchdog_fires.remove(&rule);
                }
                None
            }
            // #843 disk-pressure tick: while rules are in flight, re-measure
            // free space periodically so Reclaim fires even without a
            // completion boundary (e.g. a single long-running rule writing
            // hundreds of GB). Expression is None — the measured state is
            // enforced at the top of the next loop iteration.
            _ = disk_tick.tick() => {
                // F4: skip the statfs entirely while the feature is off —
                // otherwise a disabled config still pays one measurement
                // per tick for nothing.
                if disk_thresholds.is_some() {
                    let mut n = |args: std::fmt::Arguments<'_>| {
                        diagnostic_narrate(args, Some(&run_log));
                    };
                    disk_monitor.observe(workdir_actual.as_ref(), &mut n);
                }
                None
            }
        };
        let Some(join_result) = join_result else {
            continue;
        };
        let (completed_rule, status, record) = match join_result {
            // Reaped: drop the id→rule link so a later signal teardown
            // cannot record this rule Failed. (Only the panic branch
            // used to remove the entry — every success or rule-level
            // failure lingered here, so Ctrl-C marked ALL finished
            // rules Failed in the checkpoint.)
            Some(Ok((id, v))) => {
                task_rule.remove(&id);
                task_abort.remove(&id);
                v
            }
            Some(Err(e)) => {
                // Watchdog-initiated requeue (issue #872): a non-panic
                // cancellation is the abort issued by the watchdog's
                // escalation path, which already released the pool
                // reservation, requeued the rule to Pending, and cleared the
                // checkpoint running-set. All the engine-fault bookkeeping
                // below (fail_count, ck.mark_failed, failures.push) must be
                // skipped — otherwise the requeue would double-count as a
                // failure. The id→rule link is dropped here so the
                // "no recorded rule" invariant stays intact for real panics.
                // Invariant (review NIT 8): ONLY a non-panic cancellation
                // may take this early-continue. The loop is inline in
                // `run_command` and not unit-testable without a refactor,
                // so this branch is guarded by review, not by a test — a
                // regression here (dropping the `!e.is_panic()` guard, say)
                // would double-count every watchdog requeue as a rule
                // failure.
                if e.is_cancelled() && !e.is_panic() {
                    task_rule.remove(&e.id());
                    task_abort.remove(&e.id());
                    continue;
                }
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
                    task_abort.remove(&e.id());
                    // Internal invariant: every spawned task registers its
                    // name at spawn. Fail the run rather than leak the cap.
                    // The engine fault counts as a failure in the summary
                    // (issue #142 H6).
                    json_summary.emit(
                        "failed",
                        &RunCounts {
                            succeeded: success_count.load(std::sync::atomic::Ordering::Relaxed),
                            skipped: skipped_count.load(std::sync::atomic::Ordering::Relaxed),
                            failed: fail_count.load(std::sync::atomic::Ordering::Relaxed),
                            non_required_failed: non_required_fail_count
                                .load(std::sync::atomic::Ordering::Relaxed),
                            blocked: blocked.lock().await.len(),
                        },
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
                    signal: None,
                    rule: rule_name.clone(),
                    status: oxo_flow_core::executor::JobStatus::Failed,
                    enqueued_at: None,
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
                    skip_reason: None,
                    max_rss_mb: None,
                    cpu_seconds: None,
                    caption: config.get_rule(&rule_name).and_then(|r| {
                        oxo_flow_core::executor::process::rule_report_caption(
                            r,
                            workdir_actual.as_ref(),
                        )
                    }),
                };
                let mut ck = checkpoint.lock().await;
                ck.record_run(&record, output_tail_bytes_from_env());
                ck.mark_failed(&rule_name);
                if let Err(save_err) = ck.save_to_file_async(&checkpoint_path).await {
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
        running_since.remove(&completed_rule);
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
                signal: None,
                rule: completed_rule.clone(),
                status: oxo_flow_core::executor::JobStatus::Failed,
                enqueued_at: None,
                started_at: None,
                finished_at: None,
                exit_code: Some(1),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                skip_reason: Some(format!("re-entry manifest: {e}")),
                max_rss_mb: None,
                cpu_seconds: None,
                caption: config.get_rule(&completed_rule).and_then(|r| {
                    oxo_flow_core::executor::process::rule_report_caption(
                        r,
                        workdir_actual.as_ref(),
                    )
                }),
            };
            {
                let mut frs = failed_rules_set.lock().await;
                frs.insert(completed_rule.clone());
            }
            {
                let mut ck = checkpoint.lock().await;
                ck.record_run(&record, output_tail_bytes_from_env());
                ck.mark_failed(&completed_rule);
                if let Err(save_err) = ck.save_to_file_async(&checkpoint_path).await {
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

        // ── Runtime-discovered output_pattern processing (issue #227 item 5) ─
        // A producer instance succeeded: scan its output_pattern domain,
        // persist it, and — once ALL of the producer's instances are
        // completed (the domain union is final) — instantiate the deferred
        // consumers and insert them into the remaining plan. A discovery
        // failure fails the producer like a re-entry failure: the run aborts
        // with attribution and the next run re-executes the producer.
        if status == oxo_flow_core::executor::JobStatus::Success
            && config
                .get_rule(&completed_rule)
                .is_some_and(|r| r.output_pattern.is_some())
            && let Err(e) = process_output_pattern_discovery(
                &mut config,
                &completed_rule,
                workdir_actual.as_ref(),
                &mut sched,
                &mut order,
                &mut order_set,
                &mut dag,
                &checkpoint,
                &checkpoint_path,
            )
            .await
        {
            tracing::error!(rule = %completed_rule, error = %e, "output_pattern discovery failed");
            let record = oxo_flow_core::executor::JobRecord {
                signal: None,
                rule: completed_rule.clone(),
                status: oxo_flow_core::executor::JobStatus::Failed,
                enqueued_at: None,
                started_at: None,
                finished_at: None,
                exit_code: Some(1),
                stdout: None,
                stderr: None,
                command: None,
                retries: 0,
                skip_reason: Some(format!("output_pattern discovery: {e}")),
                max_rss_mb: None,
                cpu_seconds: None,
                caption: config.get_rule(&completed_rule).and_then(|r| {
                    oxo_flow_core::executor::process::rule_report_caption(
                        r,
                        workdir_actual.as_ref(),
                    )
                }),
            };
            {
                let mut frs = failed_rules_set.lock().await;
                frs.insert(completed_rule.clone());
            }
            {
                let mut ck = checkpoint.lock().await;
                ck.record_run(&record, output_tail_bytes_from_env());
                ck.mark_failed(&completed_rule);
                if let Err(save_err) = ck.save_to_file_async(&checkpoint_path).await {
                    tracing::warn!("Failed to save checkpoint: {save_err}");
                }
            }
            {
                let mut f = failures.lock().await;
                f.push((
                    completed_rule.clone(),
                    format!("output_pattern discovery: {e}"),
                ));
            }
            fail_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            sched.mark_completed(record);
        }

        // Failure attribution for output_pattern producers (issue #227
        // item 5): a producer that failed leaves its deferred consumers
        // uninstantiated — name them so the failure is attributable, and
        // record them for the failure summary.
        if (status == oxo_flow_core::executor::JobStatus::Failed
            || status == oxo_flow_core::executor::JobStatus::TimedOut
            || status == oxo_flow_core::executor::JobStatus::Cancelled)
            && let Some(producer_template) = config.output_pattern_template_of(&completed_rule)
        {
            let consumers = config.pending_output_pattern_consumers_of(&producer_template);
            if !consumers.is_empty() {
                narrate_console_line(format_args!(
                    "{} {} not instantiated — output_pattern producer '{}' failed",
                    "Warning:".bold().yellow(),
                    consumers.join(", "),
                    producer_template
                ));
                let mut f = failures.lock().await;
                for c in consumers {
                    f.push((
                        c,
                        format!(
                            "not instantiated — output_pattern producer '{}' failed",
                            producer_template
                        ),
                    ));
                }
            }
        }

        // Skipped producers (issue #297 item 4): an outputs-up-to-date or
        // condition-false skip never reaches discovery, so the producer's
        // deferred consumers silently vanish from THIS run's plan. A
        // checkpointed domain from an earlier success is replayed on resume,
        // but a skip with no recorded domain leaves them uninstantiated for
        // good — name both sides so the disappearance is attributable. A
        // skip is not a failure, so unlike the block above this stays a
        // warning and does not enter the failure summary.
        if status == oxo_flow_core::executor::JobStatus::Skipped
            && let Some(producer_template) = config.output_pattern_template_of(&completed_rule)
        {
            let consumers = config.pending_output_pattern_consumers_of(&producer_template);
            if !consumers.is_empty() {
                narrate_console_line(format_args!(
                    "{} {} not instantiated — output_pattern producer '{}' was skipped",
                    "Warning:".bold().yellow(),
                    consumers.join(", "),
                    producer_template
                ));
                tracing::warn!(
                    producer = %producer_template,
                    consumers = ?consumers,
                    "skipped output_pattern producer leaves deferred consumers uninstantiated"
                );
            }
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
            return handle_run_failure_abort(
                &completed_rule,
                status,
                AbortContext {
                    executor: &executor,
                    join_set: &mut join_set,
                    task_rule: &task_rule,
                    failed_rules_set: &failed_rules_set,
                    checkpoint: &checkpoint,
                    checkpoint_path: &checkpoint_path,
                    failures: &failures,
                    fail_count: &fail_count,
                    non_required_fail_count: &non_required_fail_count,
                    success_count: &success_count,
                    skipped_count: &skipped_count,
                    blocked: &blocked,
                    config: &config,
                    workflow: &workflow,
                    workdir_actual: workdir_actual.as_ref(),
                    tmp_dir_resolved: tmp_dir_resolved.as_deref(),
                    run_log: &run_log,
                    json_summary: &mut json_summary,
                    is_tty,
                    ai_recover,
                    ai_max_retries,
                },
            )
            .await;
        }

        // Propagate failure transitively: every rule that (transitively)
        // depends on a failed rule is marked as skipped/blocked.
        let nrf = non_required_fail_count.load(std::sync::atomic::Ordering::Relaxed);
        if (fc > 0 && keep_going) || nrf > 0 {
            propagate_transitive_failures(TransitiveFailureContext {
                dag: &dag,
                rule_names: &rule_names,
                submitted: &mut submitted,
                failed_rules_set: &failed_rules_set,
                skipped_count: &skipped_count,
                blocked: &blocked,
                sched: &mut sched,
                config: &config,
                workdir_actual: workdir_actual.as_ref(),
                is_tty,
                progress: &progress,
            })
            .await;
        }
    }

    // #540: the run loop is done — drop the receiver so a Ctrl-C landing
    // in the post-run work (report snapshot, webhook POST, AI report)
    // makes the watcher's `send` fail and its process-exit fallback
    // engage. With the receiver alive but never polled again, the late
    // signal was swallowed: nothing happened for potentially minutes.
    drop(signal_rx);

    let success_count = success_count.load(std::sync::atomic::Ordering::Relaxed);
    let fail_count = fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let non_required_fail_count =
        non_required_fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let skipped_count = skipped_count.load(std::sync::atomic::Ordering::Relaxed);
    let mut checkpoint = checkpoint.lock().await;
    let failures = failures.lock().await;
    let blocked = blocked.lock().await;

    progress.finish_and_clear();

    emit_execution_event(oxo_flow_core::executor::ExecutionEvent::WorkflowCompleted {
        total_duration_ms: run_started.elapsed().as_millis() as u64,
        succeeded: success_count,
        failed: fail_count,
        skipped: skipped_count,
    });
    if non_required_fail_count > 0 {
        progress_narrate(
            format_args!(
                "\n{} {} succeeded, {} skipped, {} failed, {} non-required failed",
                "Done:".bold(),
                success_count,
                skipped_count,
                fail_count,
                non_required_fail_count
            ),
            &run_log,
        );
    } else {
        progress_narrate(
            format_args!(
                "\n{} {} succeeded, {} skipped, {} failed",
                "Done:".bold(),
                success_count,
                skipped_count,
                fail_count
            ),
            &run_log,
        );
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
        progress_narrate(
            format_args!("  {} Report snapshot failed: {e}", "⚠".yellow()),
            &run_log,
        );
    }

    // --provenance recorded nothing: say so loudly. The historical failure
    // mode was silent — the user asked for tracking, got an empty checksum
    // map, and `provenance verify` later reported "Run with --provenance"
    // after they had done exactly that (issue #276).
    if provenance {
        let recorded = provenance_recorded.load(std::sync::atomic::Ordering::Relaxed);
        if recorded == 0 {
            progress_narrate(
                format_args!(
                    "  {} --provenance recorded 0 checksums — no literal outputs completed \
                     (templated or wildcard outputs are tracked per instance only after fan-out); \
                     'provenance verify' will find nothing to check",
                    "⚠".yellow()
                ),
                &run_log,
            );
        }
    }

    // Workflow-level terminal hooks (issue #227 item 1): on_complete /
    // on_error fire on every terminal path of the run loop, and the
    // configured `[webhook]` endpoint receives the matching event.
    run_terminal_hook(
        &config,
        &workdir_actual,
        tmp_dir_resolved.as_deref(),
        success_count,
        fail_count,
        skipped_count,
    )
    .await;
    notify_webhook(
        &config,
        if fail_count > 0 {
            oxo_flow_core::webhook::WebhookEvent::WorkflowFailed
        } else {
            oxo_flow_core::webhook::WebhookEvent::WorkflowCompleted
        },
        Some((success_count, fail_count, skipped_count)),
    )
    .await;

    // With --keep-going, execution continues past failures, so list every failed
    // rule (and why) in one place rather than making the user hunt for them.
    // Issue #887: with modules, failures and blocked rules group under the
    // same module headers as the plan listing; the flat per-line shapes are
    // unchanged (web SSE / report tooling parse these lines).
    if !failures.is_empty() {
        progress_narrate(format_args!("\n{}", "Failed rules:".bold().red()), &run_log);
        let names: Vec<&str> = failures.iter().map(|(name, _)| name.as_str()).collect();
        match module_groups_for_rules(&names, &config.module_rules) {
            Some(groups) => {
                for (group, members) in &groups {
                    progress_narrate(
                        format_args!("  {} ({}):", run_group_title(*group).bold(), members.len()),
                        &run_log,
                    );
                    for name in members {
                        if let Some((_, reason)) = failures.iter().find(|(n, _)| n == name) {
                            progress_narrate(
                                format_args!("    {} {} — {}", "✗".red(), name.bold(), reason),
                                &run_log,
                            );
                        }
                    }
                }
            }
            None => {
                for (name, reason) in failures.iter() {
                    progress_narrate(
                        format_args!("  {} {} — {}", "✗".red(), name.bold(), reason),
                        &run_log,
                    );
                }
            }
        }
    }

    // Blocked rules did not run and produced no outputs. Listing them keeps the
    // fallout of a failure visible without disguising it as a genuine failure.
    if !blocked.is_empty() {
        progress_narrate(
            format_args!("\n{}", "Blocked rules (did not run):".bold().yellow()),
            &run_log,
        );
        let names: Vec<&str> = blocked.iter().map(|(name, _)| name.as_str()).collect();
        match module_groups_for_rules(&names, &config.module_rules) {
            Some(groups) => {
                for (group, members) in &groups {
                    progress_narrate(
                        format_args!("  {} ({}):", run_group_title(*group).bold(), members.len()),
                        &run_log,
                    );
                    for name in members {
                        if let Some((_, dep)) = blocked.iter().find(|(n, _)| n == name) {
                            progress_narrate(
                                format_args!(
                                    "    {} {} — depends on '{}'",
                                    "⊘".yellow(),
                                    name.bold(),
                                    dep
                                ),
                                &run_log,
                            );
                        }
                    }
                }
            }
            None => {
                for (name, dep) in blocked.iter() {
                    progress_narrate(
                        format_args!("  {} {} — depends on '{}'", "⊘".yellow(), name.bold(), dep),
                        &run_log,
                    );
                }
            }
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
                "{} {} were not found:",
                "⚠".yellow(),
                oxo_flow_core::plural(missing_outputs.len(), "output file", "output files")
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
        // Not gated by the #893 disk floor below: chunk inputs are small
        // scratch files, and deleting them regardless of free space prevents
        // unbounded accumulation between runs.
        for rule in &config.rules {
            if rule.cleanup_chunks && checkpoint.is_completed(&rule.name) {
                // Migrate the chunk checksums BEFORE deleting the files
                // (issue #315 F2): a checkpoint that lists already-deleted
                // files as ordinary outputs would be an audit-trail lie —
                // verify would report them missing. If the migration
                // cannot be persisted, keep the files and skip the
                // deletion entirely.
                //
                // Checksums only exist under `--provenance`. With no
                // recorded checksum there is nothing that can lie, so the
                // deletion proceeds without a persistence round-trip
                // (issue #324 F-2) — previously the empty migration made
                // cleanup a silent no-op on every default run.
                let mut migrated: Vec<String> = Vec::new();
                for chunk in rule.input.iter() {
                    if let Some(sha) = checkpoint.checksums.remove(&chunk.to_string()) {
                        checkpoint.record_cleaned_checksum(&chunk.to_string(), sha);
                        migrated.push(chunk.to_string());
                    }
                }
                if !migrated.is_empty()
                    && let Err(e) = checkpoint.save_to_file(&checkpoint_path)
                {
                    tracing::warn!(
                        error = %e,
                        "failed to persist chunk migration — keeping chunk files"
                    );
                    // Roll the in-memory state back so a later save (e.g.
                    // tombstoning) does not persist the migration without
                    // the deletion.
                    for path in &migrated {
                        if let Some(sha) = checkpoint.cleaned_checksums.remove(path) {
                            checkpoint.checksums.insert(path.clone(), sha);
                        }
                    }
                    continue;
                }
                oxo_flow_core::executor::checkpoint::cleanup_transform_chunks(rule, workdir_actual)
                    .await;
            }
        }

        // Temporary rules: after a FULLY successful run, delete their
        // outputs once every dependent is complete and record a tombstone.
        // A future run regenerates them via cascade-up (missing inputs
        // re-run the completed producer). Leaf rules keep their outputs.
        // #893: when `[engine]` disk thresholds are configured, the sweep
        // only fires under the SAME free-space floor as the mid-run
        // reclaim — with disk comfortable, temporary outputs are kept so
        // dependent runs skip their producers instead of re-materializing
        // them every time. Without `[engine]`, the historical unconditional
        // sweep applies.
        let sweep_outputs = match endofrun_prune_decision(
            oxo_flow_core::scheduler::check_available_disk_mb(workdir_actual),
            disk_thresholds,
        ) {
            EndOfRunPruneDecision::Skip {
                free_mb,
                reclaim_mb,
            } => {
                diagnostic_narrate(
                    format_args!(
                        "ℹ free space {free_mb}MB ≥ reclaim_free_disk {reclaim_mb}MB — temporary outputs kept (disk pressure alone triggers re-materialization; remove the [engine] thresholds to always prune)"
                    ),
                    Some(&run_log),
                );
                false
            }
            EndOfRunPruneDecision::KeepOnUnknown => {
                diagnostic_narrate(
                    format_args!(
                        "ℹ free space unavailable on this platform — temporary outputs kept as a precaution ([engine] thresholds are set)"
                    ),
                    Some(&run_log),
                );
                false
            }
            EndOfRunPruneDecision::Sweep => true,
        };
        let mut new_tombstones: Vec<(String, Vec<String>)> = Vec::new();
        for rule in &config.rules {
            if !sweep_outputs || !rule.temporary || !checkpoint.is_completed(&rule.name) {
                continue;
            }
            // #843: mid-run reclamation may already have tombstoned this
            // rule — re-deleting would double-unlink. Skip only while the
            // reclaimed outputs are genuinely gone; a tombstoned rule whose
            // outputs are back on disk (restored by hand) must be
            // re-deleted and its tombstone refreshed. Regeneration clears
            // the tombstone at execution time (mark_completed site).
            if checkpoint
                .tombstones
                .get(&rule.name)
                .is_some_and(|paths| paths.iter().all(|p| !workdir_actual.join(p).exists()))
            {
                continue;
            }
            let dependents = dag.dependents(&rule.name).unwrap_or_default();
            // Issue #844: output_pattern producers register no DAG edges by
            // design (dag.rs excludes pattern-claimed outputs from template
            // matching), so `dependents` alone misclassifies them as leaves.
            // Detect consumers structurally instead: compile the producer's
            // config-expanded output_pattern and match every other rule's
            // config-expanded input paths against it. Consumer instantiation
            // bakes instance inputs ("chunks/1.txt"), which match
            // `^chunks/(?P<i>\S+)\.txt$`; consumer TEMPLATES still carry a
            // `{` placeholder and are skipped so they cannot falsely match.
            let mut consumers: HashSet<String> = dependents.iter().cloned().collect();
            if let Some(pattern) = &rule.output_pattern {
                let expanded_pattern = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                    pattern,
                    &wildcard_values,
                );
                if let Ok(re) = wildcard::pattern_to_regex(&expanded_pattern) {
                    for other in &config.rules {
                        if other.name == rule.name {
                            continue;
                        }
                        for input in other.input.iter() {
                            let expanded =
                                oxo_flow_core::executor::checkpoint::expand_config_in_path(
                                    input,
                                    &wildcard_values,
                                );
                            if !expanded.contains('{') && re.is_match(&expanded) {
                                consumers.insert(other.name.clone());
                                break;
                            }
                        }
                    }
                }
            }
            if consumers.is_empty() {
                eprintln!(
                    "  {} temporary rule '{}' is a leaf — outputs kept",
                    "ℹ".dimmed(),
                    rule.name
                );
                continue;
            }
            if !consumers.iter().all(|d| checkpoint.is_completed(d)) {
                let pending: Vec<&str> = consumers
                    .iter()
                    .filter(|d| !checkpoint.is_completed(d))
                    .map(String::as_str)
                    .collect();
                diagnostic_narrate(
                    format_args!(
                        "  {} temporary rule '{}' kept: dependents not complete yet ({})",
                        "!".yellow(),
                        rule.name,
                        pending.join(", ")
                    ),
                    Some(&run_log),
                );
                continue;
            }
            let mut deleted = Vec::new();
            // Output patterns that still contain `{` after config
            // expansion and resolved to no concrete instance path
            // (issue #844 diagnostic) — collected during the loop below
            // so a pattern that DID expand is never falsely reported.
            let mut skipped: Vec<String> = Vec::new();
            // Instance bindings recorded for this producer template by the
            // output_pattern discovery pass (issue #844): template → one
            // combo per completed instance. Empty for plain [[values]]/
            // [[pairs]] fan-outs (their instances run under their own names
            // and expand their outputs at plan time) and for legacy
            // checkpoints.
            let template = config.output_pattern_template_of(&rule.name);
            let domains = template
                .as_deref()
                .and_then(|t| checkpoint.output_pattern_domains.get(t))
                .cloned()
                .unwrap_or_default();
            // #844: an output_pattern producer has an EMPTY `output` list —
            // the pattern itself is the deletion target, expanded per
            // recorded instance combo in the loop below. Chain it in so the
            // same expansion logic covers both declaration styles.
            for output in rule.output.iter().chain(rule.output_pattern.iter()) {
                let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                    output,
                    &wildcard_values,
                );
                if !expanded.contains('{') {
                    if oxo_flow_core::executor::output_invalidation::is_protected_path(
                        rule,
                        &wildcard_values,
                        &expanded,
                    ) {
                        // #843: protected outputs are user-declared
                        // keep-forever files — the same gate reclamation
                        // applies, so the sweep must not delete what the
                        // monitor wouldn't.
                        continue;
                    }
                    deleted.push(expanded);
                    continue;
                }
                // #844: a pattern-level path still containing `{` after
                // config expansion is a wildcard output (e.g. per-sample
                // BAMs). Delete only the concrete instance paths that
                // actually ran — expand every recorded combo, never the
                // pattern itself.
                let mut resolved_any = false;
                for combo in &domains {
                    let instance = wildcard::expand_pattern(&expanded, combo)
                        .unwrap_or_else(|_| expanded.clone());
                    if instance.contains('{') {
                        continue;
                    }
                    resolved_any = true;
                    if oxo_flow_core::executor::output_invalidation::is_protected_path(
                        rule,
                        &wildcard_values,
                        &instance,
                    ) {
                        continue;
                    }
                    if !deleted.contains(&instance) {
                        deleted.push(instance);
                    }
                }
                if !resolved_any {
                    // Expansion failure only — instances that resolved but
                    // matched `protected_output` are intentionally kept,
                    // not skipped.
                    if domains.is_empty() {
                        skipped.push(format!(
                            "{output} (no recorded instance bindings — legacy checkpoint or non-output_pattern fan-out; use `oxo-flow clean`)"
                        ));
                    } else {
                        skipped.push(format!(
                            "{expanded} (wildcard did not resolve against recorded combos)"
                        ));
                    }
                }
            }
            if !skipped.is_empty() {
                // Diagnostic (issue #844): a silent no-op is the worst
                // outcome — name the rule and the patterns that could not
                // be expanded to instance paths.
                diagnostic_narrate(
                    format_args!(
                        "  {} temporary wildcard outputs of '{}' left in place: instance paths could not be expanded: {}",
                        "!".yellow(),
                        rule.name,
                        skipped.join("; ")
                    ),
                    Some(&run_log),
                );
            }
            if !deleted.is_empty() {
                new_tombstones.push((rule.name.clone(), deleted));
            }
        }
        if !new_tombstones.is_empty() {
            // Same audit rule as chunk cleanup (issue #315 F2): persist
            // tombstones + the cleaned migration BEFORE deleting, and skip
            // the deletion when the persistence fails.
            let mut rollback: Vec<String> = Vec::new();
            for (rule, paths) in &new_tombstones {
                checkpoint.tombstones.insert(rule.clone(), paths.clone());
                for path in paths {
                    if let Some(sha) = checkpoint.checksums.remove(path) {
                        checkpoint.record_cleaned_checksum(path, sha);
                        rollback.push(path.clone());
                    }
                }
            }
            if let Err(e) = checkpoint.save_to_file(&checkpoint_path) {
                tracing::warn!(
                    error = %e,
                    "failed to persist tombstone migration — keeping temporary outputs"
                );
                for (rule, _) in &new_tombstones {
                    checkpoint.tombstones.remove(rule);
                }
                for path in &rollback {
                    if let Some(sha) = checkpoint.cleaned_checksums.remove(path) {
                        checkpoint.checksums.insert(path.clone(), sha);
                    }
                }
            } else {
                for (rule, paths) in &new_tombstones {
                    for expanded in paths {
                        let resolved = workdir_actual.join(expanded);
                        // Symlink-aware classification (review): a symlink is
                        // always unlinked as the link itself — never the
                        // bytes it points at.
                        let _ = match std::fs::symlink_metadata(&resolved) {
                            Ok(md) if md.file_type().is_symlink() => {
                                std::fs::remove_file(&resolved)
                            }
                            Ok(md) if md.is_dir() => std::fs::remove_dir_all(&resolved),
                            Ok(_) => std::fs::remove_file(&resolved),
                            Err(_) => Ok(()),
                        };
                    }
                    eprintln!(
                        "  {} temporary outputs deleted for '{}' ({}, regenerated on demand)",
                        "⊘".dimmed(),
                        rule,
                        oxo_flow_core::plural(paths.len(), "file", "files")
                    );
                }
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
            // Issue #441: in the run's post-expansion config each template
            // appears once per sample instance, so a 1-template finding
            // printed as "3 finding(s)" for a 3-sample pilot. Group by the
            // finding's content — identical (code, message, suggestion)
            // across instances is ONE distinct finding with M instances.
            type WarningGroup<'a> = (
                (&'a str, &'a str, &'a str),
                Vec<&'a oxo_flow_core::scientific_preflight::ScientificWarning>,
            );
            let mut distinct: Vec<WarningGroup> = Vec::new();
            for w in &scientific {
                let key = (w.code.as_str(), w.message.as_str(), w.suggestion.as_str());
                match distinct.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, group)) => group.push(w),
                    None => distinct.push((key, vec![w])),
                }
            }
            eprintln!(
                "  {} ({} across sample rules) — see 'oxo-flow dry-run --samples ...'",
                oxo_flow_core::plural(
                    distinct.len(),
                    "distinct scientific preflight finding",
                    "distinct scientific preflight findings"
                ),
                oxo_flow_core::plural(scientific.len(), "instance", "instances")
            );
            for (key, group) in &distinct {
                eprintln!("    ⚠ [{}] {} (×{})", key.0, group[0].rule, group.len());
            }
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
                "A pilot run of {} out of {} completed in {:.1}s ({} succeeded, {} failed).\n\
                 Per-sample wall time: {:.1}s; projected full run: ~{:.0}s.\n\
                 Scientific preflight findings:\n{}\n\n\
                 Write a short pilot report (3-5 sentences): whether the pilot results are\n\
                 healthy, whether scaling up is advisable, and what to fix first.",
                oxo_flow_core::plural(pilot_count, "sample", "samples"),
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
    json_summary.emit(
        if fail_count > 0 {
            "failed"
        } else {
            "completed"
        },
        &RunCounts {
            succeeded: success_count,
            skipped: skipped_count,
            failed: fail_count,
            non_required_failed: non_required_fail_count,
            blocked: blocked.len(),
        },
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

#[cfg(test)]
mod tests {
    use super::{EndOfRunPruneDecision as Prune, endofrun_prune_decision};
    use super::{
        age_ready_list, ai_attempts, apply_cli_override_phase, apply_cli_overrides,
        closest_declared_key, config_placeholder_values, failure_exit_code, known_modules_hint,
        missing_source_findings, missing_source_lines, parse_cli_overrides,
        parse_disk_hold_abort_rounds, parse_disk_tick_secs, parse_spawn_watchdog_threshold,
        parse_watchdog_requeue_fires, reconcile_failed_rules, substitute_source_placeholder,
        suggest_jobs,
    };
    use oxo_flow_core::dag::WorkflowDag;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn reconcile_failed_rules_separates_real_failures_from_when_false_skips() {
        // Issue #690: a checkpoint written by an older version keeps a
        // when-gated rule in failed_rules; only the recorded verdict makes
        // it a skip.
        let failed: std::collections::BTreeSet<String> = ["align", "qc_S2", "trim"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let verdicts: std::collections::BTreeMap<String, bool> =
            [("qc_S2".to_string(), false)].into_iter().collect();
        let (real_failed, skipped) = reconcile_failed_rules(&failed, &verdicts);
        assert_eq!(real_failed, vec!["align", "trim"]);
        assert_eq!(skipped, vec!["qc_S2"]);
    }

    #[test]
    fn reconcile_failed_rules_keeps_failures_without_a_recorded_verdict() {
        // No verdict recorded (or verdict true — a real failure followed by
        // a gate flip back to true must stay a failure): everything is real.
        let failed: std::collections::BTreeSet<String> =
            ["a".to_string(), "b".to_string()].into_iter().collect();
        let verdicts: std::collections::BTreeMap<String, bool> =
            [("a".to_string(), true)].into_iter().collect();
        let (real_failed, skipped) = reconcile_failed_rules(&failed, &verdicts);
        assert_eq!(real_failed, vec!["a", "b"]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn parse_disk_env_knobs_fall_back_to_defaults() {
        // Unset, empty, garbage, zero — a typo can never disable the
        // abort (it would hang instead), so every invalid form falls
        // back to the shipped default.
        assert_eq!(parse_disk_tick_secs(None), 60);
        assert_eq!(parse_disk_tick_secs(Some("")), 60);
        assert_eq!(parse_disk_tick_secs(Some("abc")), 60);
        assert_eq!(parse_disk_tick_secs(Some("0")), 60);
        assert_eq!(parse_disk_tick_secs(Some("-5")), 60);
        assert_eq!(parse_disk_hold_abort_rounds(None), 2);
        assert_eq!(parse_disk_hold_abort_rounds(Some("xyz")), 2);
        assert_eq!(parse_disk_hold_abort_rounds(Some("0")), 2);
    }

    #[test]
    fn parse_disk_env_knobs_honor_valid_values() {
        assert_eq!(parse_disk_tick_secs(Some("1")), 1);
        assert_eq!(parse_disk_tick_secs(Some("300")), 300);
        assert_eq!(parse_disk_hold_abort_rounds(Some("1")), 1);
        assert_eq!(parse_disk_hold_abort_rounds(Some("10")), 10);
    }

    #[test]
    fn endofrun_prune_sweeps_without_engine_thresholds() {
        // No `[engine]` section → the historical unconditional sweep, on
        // every free-space outcome (the measurement is simply irrelevant).
        assert!(matches!(
            endofrun_prune_decision(Some(0), None),
            Prune::Sweep
        ));
        assert!(matches!(
            endofrun_prune_decision(Some(39_000_000), None),
            Prune::Sweep
        ));
        assert!(matches!(endofrun_prune_decision(None, None), Prune::Sweep));
    }

    #[test]
    fn endofrun_prune_skips_above_reclaim_floor() {
        // #893: with thresholds configured, comfortable free space keeps
        // temporary outputs — the same floor the mid-run reclaim uses.
        let thresholds = Some((5_000_000, 5_000_000)); // 5T reclaim
        match endofrun_prune_decision(Some(39_000_000), thresholds) {
            Prune::Skip {
                free_mb,
                reclaim_mb,
            } => {
                assert_eq!(free_mb, 39_000_000);
                assert_eq!(reclaim_mb, 5_000_000);
            }
            other => panic!("expected Skip, got {other:?}"),
        }
        // Exactly at the floor counts as above it (mid-run reclaim only
        // fires BELOW reclaim_free_disk — mirror that boundary).
        assert!(matches!(
            endofrun_prune_decision(Some(5_000_000), thresholds),
            Prune::Skip { .. }
        ));
    }

    #[test]
    fn endofrun_prune_sweeps_below_reclaim_floor() {
        // Under pressure → sweep (delete temporaries to free the space).
        assert!(matches!(
            endofrun_prune_decision(Some(4_999_999), Some((5_000_000, 5_000_000))),
            Prune::Sweep
        ));
    }

    #[test]
    fn endofrun_prune_keeps_outputs_when_measurement_unknown() {
        // Fail-open matches DiskPressureMonitor: a non-Unix platform under
        // configured thresholds must NOT sweep (an unknown disk state is
        // not a license to delete).
        assert!(matches!(
            endofrun_prune_decision(None, Some((5_000_000, 5_000_000))),
            Prune::KeepOnUnknown
        ));
    }

    #[test]
    fn endofrun_prune_debug_representations_are_distinct() {
        // Guards the enum from collapsing arms (e.g. Skip accidentally
        // constructed for the KeepOnUnknown case) in a debug-print context.
        let skip = format!(
            "{:?}",
            Prune::Skip {
                free_mb: 1,
                reclaim_mb: 1
            }
        );
        let unknown = format!("{:?}", Prune::KeepOnUnknown);
        let sweep = format!("{:?}", Prune::Sweep);
        assert_ne!(skip, unknown);
        assert_ne!(skip, sweep);
        assert_ne!(unknown, sweep);
    }

    #[test]
    fn snapshot_age_is_derived_from_checkpoint_mtime() {
        // Issue #849: snapshot freshness comes from the file's mtime, not
        // from any field inside the checkpoint (CheckpointState carries no
        // self-timestamp). A freshly written file yields a small age; a
        // backdated mtime yields the round-tripped age, clamped at 0.
        let dir = std::env::temp_dir().join(format!(
            "oxoflow-status-age-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.json");
        std::fs::write(&path, "{}").unwrap();

        let mtime_now: chrono::DateTime<chrono::Utc> =
            std::fs::metadata(&path).unwrap().modified().unwrap().into();
        let age_now = chrono::Utc::now()
            .signed_duration_since(mtime_now)
            .num_seconds()
            .max(0);
        assert!(age_now < 60, "fresh file reported {age_now}s old");

        // Backdate the mtime by 10 minutes via filetime semantics through
        // std: rewrite the file, then shift the clock comparison instead —
        // std has no mtime setter, so assert the arithmetic directly.
        let backdated = chrono::Utc::now() - chrono::Duration::seconds(600);
        let age_old = chrono::Utc::now()
            .signed_duration_since(backdated)
            .num_seconds()
            .max(0);
        assert_eq!(age_old, 600);
        assert!(age_old > super::SNAPSHOT_STALE_SECS);

        std::fs::remove_dir_all(&dir).ok();
    }

    fn config_with_rules(rules_toml: &str) -> oxo_flow_core::config::WorkflowConfig {
        oxo_flow_core::config::WorkflowConfig::parse(rules_toml).unwrap()
    }

    fn source_gate_fixture(
        dir: &std::path::Path,
    ) -> (
        oxo_flow_core::config::WorkflowConfig,
        WorkflowDag,
        Vec<String>,
        HashMap<String, String>,
    ) {
        let toml = r#"
[workflow]
name = "t"
version = "1.0"

[config]
threshold = 5
empty = []

[[rules]]
name = "gen"
input = []
output = ["out.txt"]
shell = "echo x > {output[0]}"

[[rules]]
name = "use"
input = ["out.txt", "ext.txt"]
output = ["final.txt"]
shell = "cat {input} > {output[0]}"

[[rules]]
name = "gated"
input = ["gated_missing.txt"]
output = ["gated.txt"]
when = "config.threshold >= 10"
shell = "true"

[[rules]]
name = "scatter_zero"
input = ["scatter_missing.txt"]
output = ["scatter.txt"]
scatter = { variable = "chunk", values_from = "config.empty" }
shell = "true"

[[rules]]
name = "opt"
input = ["optional_missing.txt"]
output = ["opt.txt"]
optional = true
shell = "true"
"#;
        let mut config = config_with_rules(toml);
        config.apply_defaults();
        let dag = WorkflowDag::from_rules_with_config(
            &config.rules,
            &config_placeholder_values(&config.config),
        )
        .unwrap();
        let order = dag.execution_order().unwrap();
        let wildcard_values = config_placeholder_values(&config.config);
        std::fs::create_dir_all(dir).unwrap();
        (config, dag, order, wildcard_values)
    }

    fn source_gate_call(
        config: &oxo_flow_core::config::WorkflowConfig,
        dag: &WorkflowDag,
        order: &[String],
        workdir: &std::path::Path,
        workflow_dir: &std::path::Path,
        wildcard_values: &HashMap<String, String>,
    ) -> Vec<String> {
        missing_source_findings(config, dag, order, workdir, workflow_dir, wildcard_values)
    }

    #[test]
    fn missing_source_gate_flags_only_unproduced_required_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = source_gate_fixture(dir.path());
        let findings = source_gate_call(
            &config,
            &dag,
            &order,
            dir.path(),
            dir.path(),
            &wildcard_values,
        );
        assert_eq!(
            findings,
            vec!["rule 'use': input 'ext.txt' does not exist and no rule produces it".to_string()],
            "'out.txt' has a producer (gen) and belongs to the cascade path; when-false, \
             zero-fan-out scatter, and optional rules are all skipped"
        );
    }

    #[test]
    fn missing_source_gate_passes_when_sources_exist() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = source_gate_fixture(dir.path());
        std::fs::write(dir.path().join("ext.txt"), b"data").unwrap();
        let findings = source_gate_call(
            &config,
            &dag,
            &order,
            dir.path(),
            dir.path(),
            &wildcard_values,
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn missing_source_gate_silent_with_full_checkpoint_is_the_regressed_scenario() {
        // The reported bug: every rule completed, raw/ deleted, re-run exited
        // 0. At gate time (pre-checkpoint) the completed state is irrelevant
        // — the missing external source must be flagged regardless.
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = source_gate_fixture(dir.path());
        std::fs::write(dir.path().join("out.txt"), b"stale").unwrap();
        std::fs::write(dir.path().join("final.txt"), b"stale").unwrap();
        let findings = source_gate_call(
            &config,
            &dag,
            &order,
            dir.path(),
            dir.path(),
            &wildcard_values,
        );
        assert!(!findings.is_empty(), "missing ext.txt must gate the run");
    }

    #[test]
    fn missing_source_gate_skips_reference_outputs() {
        // [[references]] outputs are built by the reference machinery after
        // this gate and are not DAG nodes — a missing one must not gate.
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[workflow]
name = "refgate"

[[references]]
name = "idx"
source = "genome.fa"
output = "genome.idx"
build = "cat genome.fa > genome.idx"

[[rules]]
name = "use_ref"
input = ["genome.idx"]
output = ["result.txt"]
shell = "cat {input} > {output}"
"#;
        let mut config = config_with_rules(toml);
        config.apply_defaults();
        let dag = WorkflowDag::from_rules_with_config(
            &config.rules,
            &config_placeholder_values(&config.config),
        )
        .unwrap();
        let order = dag.execution_order().unwrap();
        let wildcard_values = config_placeholder_values(&config.config);
        std::fs::write(dir.path().join("genome.fa"), b"ACGT").unwrap();
        let findings = source_gate_call(
            &config,
            &dag,
            &order,
            dir.path(),
            dir.path(),
            &wildcard_values,
        );
        assert!(
            findings.is_empty(),
            "reference-built output must not gate the run: {findings:?}"
        );
    }

    #[test]
    fn missing_source_gate_accepts_sample_pattern_dir_inputs() {
        // Bare relative input whose data lives in the sample_pattern
        // discovery directory (absolute pattern, sibling of the workdir) —
        // the web instance-table shape.
        let dir = tempfile::tempdir().unwrap(); // workflow dir
        let data = tempfile::tempdir().unwrap(); // sample_pattern root
        let toml = format!(
            r#"
[workflow]
name = "samplegate"
sample_pattern = "{root}/data/{{{{sample}}}}.fq"

[[rules]]
name = "qc"
input = ["{{{{sample}}}}.fq"]
output = ["qc_{{{{sample}}}}.txt"]
shell = "cat {{{{input}}}} > {{{{output}}}}"
"#,
            root = data.path().display(),
        );
        let mut config = config_with_rules(&toml);
        config.apply_defaults();
        std::fs::create_dir_all(data.path().join("data")).unwrap();
        std::fs::write(data.path().join("data").join("S1.fq"), b"read").unwrap();
        let dag = WorkflowDag::from_rules_with_config(
            &config.rules,
            &config_placeholder_values(&config.config),
        )
        .unwrap();
        let order = dag.execution_order().unwrap();
        let wildcard_values = config_placeholder_values(&config.config);
        let findings = source_gate_call(
            &config,
            &dag,
            &order,
            dir.path(),
            dir.path(),
            &wildcard_values,
        );
        assert!(
            findings.is_empty(),
            "input present in the sample_pattern dir must not gate: {findings:?}"
        );
    }

    #[test]
    fn missing_source_gate_skips_input_of_when_false_shared_output_producer() {
        // Issue #915 Bug 3 (complementary when gates on one shared output):
        // in a tumor-only cohort the fanned paired instance is vetoed by its
        // `when` at expansion, so `shared.txt` has NO producer in the DAG and
        // no declaring rule survives in config.rules. The preflight gate must
        // still not flag inputs that belong to the dead side of the
        // complementary pair: expansion recorded the vetoed outputs in
        // `when_skipped_outputs` (#882), and that exemption alone resolves
        // this — mutation-detectable (deleting the clause fails the test).
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[workflow]
name = "sharedout"
version = "1.0"

[config]
mode = "tumoronly"

[[rules]]
name = "consumer"
input = ["shared.txt"]
output = ["final.txt"]
shell = "cat {input} > {output[0]}"
"#;
        let mut config = config_with_rules(toml);
        config.apply_defaults();
        // Production-faithful post-expansion state: the paired instance was
        // fully vetoed at fan-out — expansion removed it from config.rules
        // AND recorded its outputs (so no DAG producer for shared.txt).
        config.when_skipped_outputs.insert("shared.txt".to_string());
        let wildcard_values = config_placeholder_values(&config.config);
        let dag = WorkflowDag::from_rules_with_config(&config.rules, &wildcard_values).unwrap();
        assert!(
            dag.producer_of("shared.txt").is_none(),
            "fixture precondition: the vetoed side must leave shared.txt producerless"
        );
        let order = dag.execution_order().unwrap();
        let findings = source_gate_call(
            &config,
            &dag,
            &order,
            dir.path(),
            dir.path(),
            &wildcard_values,
        );
        assert!(
            findings.is_empty(),
            "the gated-off producer's input must not gate the satisfiable consumer: {findings:?}"
        );
    }

    #[test]
    fn missing_source_gate_skips_pattern_recorded_in_when_skipped_outputs() {
        // Issue #915 (expansion half): an instance vetoed at fan-out
        // records its would-be outputs in when_skipped_outputs (#882).
        // A consumer missing such a pattern must not gate — the producer
        // never existed for this cohort by design.
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[workflow]
name = "skipped"
version = "1.0"

[config]
mode = "off"

[[rules]]
name = "consumer"
input = ["morphed.txt"]
output = ["final.txt"]
shell = "cat {input} > {output[0]}"
"#;
        let mut config = config_with_rules(toml);
        config.apply_defaults();
        config
            .when_skipped_outputs
            .insert("morphed.txt".to_string());
        let wildcard_values = config_placeholder_values(&config.config);
        // The vetoed morph rule is gone from config.rules entirely (as a
        // fully-vetoed fan-out leaves nothing behind), so the DAG has no
        // producer for morphed.txt and the when_skipped_outputs exemption
        // is the only path to an empty gate.
        let dag = WorkflowDag::from_rules_with_config(&config.rules, &wildcard_values).unwrap();
        assert!(
            dag.producer_of("morphed.txt").is_none(),
            "fixture precondition: the vetoed producer must leave morphed.txt producerless"
        );
        let order = dag.execution_order().unwrap();
        let findings = source_gate_call(
            &config,
            &dag,
            &order,
            dir.path(),
            dir.path(),
            &wildcard_values,
        );
        assert!(
            findings.is_empty(),
            "a when_skipped_outputs pattern must not gate its consumers: {findings:?}"
        );
    }

    #[test]
    fn missing_source_gate_expands_config_tokens_in_when_skipped_outputs() {
        // Issue #915 review NIT-1: the record site (#882) expands only `\w+`
        // wildcards and `{values.*}`, so a `{config.*}`-bearing vetoed output
        // stays RAW in when_skipped_outputs while the gate's patterns are
        // config-expanded. The compare-time expansion must bridge that gap.
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[workflow]
name = "cfgtoken"
version = "1.0"

[config]
genome = "hg38"

[[rules]]
name = "consumer"
input = ["ref_hg38.fai"]
output = ["final.txt"]
shell = "cat {input} > {output[0]}"
"#;
        let mut config = config_with_rules(toml);
        config.apply_defaults();
        // Recorded exactly as expand_rule_patterns leaves it: raw token.
        config
            .when_skipped_outputs
            .insert("ref_{config.genome}.fai".to_string());
        let wildcard_values = config_placeholder_values(&config.config);
        let dag = WorkflowDag::from_rules_with_config(&config.rules, &wildcard_values).unwrap();
        let order = dag.execution_order().unwrap();
        let findings = source_gate_call(
            &config,
            &dag,
            &order,
            dir.path(),
            dir.path(),
            &wildcard_values,
        );
        assert!(
            findings.is_empty(),
            "a raw {{config.*}} recorded entry must match after compare-time expansion: {findings:?}"
        );
    }

    #[test]
    fn missing_source_lines_lists_rule_names_and_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let (config, _dag, _order, wildcard_values) = source_gate_fixture(dir.path());
        let mut missing = HashSet::new();
        missing.insert("use".to_string());
        let lines = missing_source_lines(&missing, &config, dir.path(), &wildcard_values);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("use"), "{:?}", lines[0]);
        assert!(lines[0].contains("ext.txt"), "{:?}", lines[0]);
    }

    #[test]
    fn suggest_jobs_evaluates_each_wave_against_its_own_heaviest_rule() {
        // Waves: 20×t1, 2×t16, 20×t1 on a 64-core box. The old global
        // formula suggested 4; the per-group form keeps the wide light
        // waves at 20 (issue #361 measured 11.0 s vs 3.0 s makespan on
        // this exact shape).
        let mut toml = String::from("[workflow]\nname = \"t\"\nversion = \"0.1\"\n");
        for i in 0..20 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"a{i}\"\noutput = [\"a{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 1\n"
            ));
        }
        for i in 0..2 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"b{i}\"\noutput = [\"b{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 16\n"
            ));
        }
        for i in 0..20 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"c{i}\"\noutput = [\"c{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 1\n"
            ));
        }
        let config = config_with_rules(&toml);
        let groups = vec![
            (0..20).map(|i| format!("a{i}")).collect::<Vec<_>>(),
            (0..2).map(|i| format!("b{i}")).collect::<Vec<_>>(),
            (0..20).map(|i| format!("c{i}")).collect::<Vec<_>>(),
        ];
        assert_eq!(suggest_jobs(&config, &groups, 64), 20);
        // The old global formula would have said 4 here.
        assert_ne!(suggest_jobs(&config, &groups, 64), 4);
    }

    #[test]
    fn suggest_jobs_clamps_to_the_wave_width_and_system_threads() {
        // A single wave of 5 heavy rules on 4 cores: 4/16 → 0 → clamped
        // to at least 1 job, and never above the wave width (5).
        let mut toml = String::from("[workflow]\nname = \"t\"\nversion = \"0.1\"\n");
        for i in 0..5 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"h{i}\"\noutput = [\"h{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 16\n"
            ));
        }
        let config = config_with_rules(&toml);
        let groups = vec![(0..5).map(|i| format!("h{i}")).collect::<Vec<_>>()];
        assert_eq!(suggest_jobs(&config, &groups, 4), 1);
        // Wide light wave on a big machine: the 16-job legacy ceiling is gone.
        let mut toml = String::from("[workflow]\nname = \"t\"\nversion = \"0.1\"\n");
        for i in 0..20 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"l{i}\"\noutput = [\"l{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 1\n"
            ));
        }
        let config = config_with_rules(&toml);
        let groups = vec![(0..20).map(|i| format!("l{i}")).collect::<Vec<_>>()];
        assert_eq!(suggest_jobs(&config, &groups, 64), 20);
    }

    #[test]
    fn suggest_jobs_defaults_to_one_without_rules() {
        let config = config_with_rules("[workflow]\nname = \"t\"\nversion = \"0.1\"\n");
        assert_eq!(suggest_jobs(&config, &[], 64), 1);
        assert_eq!(suggest_jobs(&config, &[vec![]], 64), 1);
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
            "run",
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
            "run",
        )
        .unwrap();
        assert_eq!(map["min_quality"], "45");
    }

    #[test]
    fn canonicalizes_hyphen_spelled_override_to_declared_key() {
        // issue #794: `--api-token=x` names the declared key `api_token`.
        // Without canonicalization the override is stored under the dashed
        // alias: the declared key keeps its default and the value bypasses
        // sensitive masking in the checkpoint snapshot.
        let map = parse_cli_overrides(
            vec![
                "--api-token=sk-OVERRIDE-123".to_string(),
                "min-quality=45".to_string(),
            ],
            &declared(&["api_token", "min_quality"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["api_token"], "sk-OVERRIDE-123");
        assert_eq!(map["min_quality"], "45");
        assert!(!map.contains_key("api-token"), "{map:?}");
        assert!(!map.contains_key("min-quality"), "{map:?}");
    }

    #[test]
    fn canonicalizes_space_form_for_hyphen_spelled_declared_key() {
        // issue #794: `--api-token VALUE` must be accepted when only the
        // underscore-spelled `api_token` is declared.
        let map = parse_cli_overrides(
            vec!["--api-token".to_string(), "sk-OVERRIDE-123".to_string()],
            &declared(&["api_token"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["api_token"], "sk-OVERRIDE-123");
    }

    #[test]
    fn canonicalization_wins_on_exact_match_and_stays_silent_on_ambiguity() {
        // Exact declared match beats normalization: `mode` declared plus
        // `sub-mode` hyphen-spelled keeps its literal key.
        let map = parse_cli_overrides(
            vec!["--sub-mode=2".to_string()],
            &declared(&["sub_mode"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["sub_mode"], "2");

        // Two declared keys normalizing to the same form is pathological but
        // must not crash or silently pick one: the literal spelling is kept
        // and the usual undeclared-injection / unknown-token rules apply.
        let map = parse_cli_overrides(
            vec!["--a-b=1".to_string()],
            &declared(&["a_b", "a-b"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["a-b"], "1");

        // Nothing declared matches → literal key, issue #62 injection.
        let map = parse_cli_overrides(
            vec!["--totally-unknown=7".to_string()],
            &declared(&["min_quality"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["totally-unknown"], "7");

        // Space form for an unresolvable dashed token stays rejected (#71).
        let err = parse_cli_overrides(
            vec!["--totally-unknown".to_string(), "7".to_string()],
            &declared(&["min_quality"]),
            "run",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("unknown argument"), "{err:?}");
    }

    #[test]
    fn apply_cli_overrides_puts_override_on_declared_sensitive_key() {
        // issue #794 end-to-end at the apply layer: the canonicalized
        // override must win over the declarative default (precedence
        // CLI > default) instead of leaving the default in place.
        let mut config = config_with_rules(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
api_token = { default = "sk-DEFAULT-abc", sensitive = true, help = "API token" }

[[rules]]
name = "r"
shell = "true"
"#,
        );
        let overrides = parse_cli_overrides(
            vec!["--api-token=sk-OVERRIDE-123".to_string()],
            &declared(&["api_token"]),
            "run",
        )
        .unwrap();
        apply_cli_overrides(&mut config, &overrides).unwrap();
        assert_eq!(config.config["api_token"].as_str(), Some("sk-OVERRIDE-123"));
    }

    #[test]
    fn override_phase_wires_parse_apply_and_command_label() {
        // Composition pin for apply_cli_override_phase: one call performs
        // the parse → apply → warn sequence both run and dry-run rely on,
        // and the command label reaches the parse errors (issue #77
        // parity — the preview must reject what execution rejects).
        let mut config = config_with_rules(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
threads = { default = "4", help = "thread count" }

[[rules]]
name = "r"
shell = "true"
"#,
        );
        apply_cli_override_phase(
            &mut config,
            std::path::Path::new("t.oxoflow"),
            vec!["--threads=8".to_string()],
            "run",
        )
        .unwrap();
        assert_eq!(config.config["threads"].as_str(), Some("8"));

        let err = apply_cli_override_phase(
            &mut config,
            std::path::Path::new("t.oxoflow"),
            vec!["--config".to_string(), "config/x.toml".to_string()],
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("dry-run"), "{msg}");
    }

    #[test]
    fn rejects_unknown_dash_token() {
        let err = parse_cli_overrides(
            vec!["--config".to_string(), "config/x.toml".to_string()],
            &declared(&["min_quality"]),
            "run",
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
            "run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("command flag"), "{msg}");
        assert!(msg.contains("--json"), "{msg}");
    }

    #[test]
    fn dry_run_treats_shared_run_flags_as_misordered_not_unsupported() {
        // issue #441: dry-run accepts `--cache-dir` as a real flag now, so a
        // swallowed one must read as misordered, not "not supported by
        // dry-run".
        let err = parse_cli_overrides(
            vec!["threads=8".to_string(), "--cache-dir=x".to_string()],
            &declared(&["threads"]),
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("command flag"), "{msg}");
        assert!(msg.contains("must come before"), "{msg}");

        // Other run flags stay "not supported" under dry-run.
        let err = parse_cli_overrides(
            vec!["threads=8".to_string(), "--provenance".to_string()],
            &declared(&["threads"]),
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not supported by dry-run"), "{msg}");
    }

    #[test]
    fn rejects_space_form_for_undeclared_key_even_via_arg() {
        // `--arg --config x` lands in the same override list.
        let err = parse_cli_overrides(
            vec!["--config".to_string(), "x".to_string()],
            &declared(&[]),
            "run",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("unknown argument"));
    }

    #[test]
    fn rejects_declared_key_without_value() {
        let err = parse_cli_overrides(
            vec!["--min_quality".to_string()],
            &declared(&["min_quality"]),
            "run",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("invalid config flag"), "{err:?}");
    }

    #[test]
    fn rejects_space_form_that_would_swallow_a_command_flag() {
        // Audit finding: `dry-run wf --mode --json` consumed `--json` as the
        // value of the declared key `mode`, so the run silently dropped its
        // JSON output. The `=` form already errors; the space form must too.
        let err = parse_cli_overrides(
            vec!["--mode".to_string(), "--json".to_string()],
            &declared(&["mode"]),
            "run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("--mode"), "{msg}");
        assert!(msg.contains("--json"), "{msg}");
        assert!(msg.contains("--mode=<value>"), "{msg}");
    }

    #[test]
    fn ai_attempts_defaults_to_one_and_never_zero() {
        // Audit finding: the flag was accepted and discarded. It now bounds
        // the recovery/analysis attempts; absent → the pre-flag single try,
        // and an explicit 0 still means one attempt (never none).
        assert_eq!(ai_attempts(None), 1);
        assert_eq!(ai_attempts(Some(0)), 1);
        assert_eq!(ai_attempts(Some(3)), 3);
    }

    #[test]
    fn failure_exit_code_parses_recorded_reasons() {
        // The failure summary format from the executor path.
        assert_eq!(
            failure_exit_code("exit code 3 — ERROR: unknown reference genome hg999"),
            3
        );
        assert_eq!(failure_exit_code("exit code 137"), 137);
        assert_eq!(failure_exit_code("exit code 137\nstderr: ..."), 137);
        // Panics/aborts carry no code in the reason → keep -1.
        assert_eq!(failure_exit_code("task panicked: boom"), -1);
        assert_eq!(failure_exit_code(""), -1);
        assert_eq!(failure_exit_code("exit code notanumber"), -1);
    }

    #[test]
    fn closest_declared_key_names_a_typo_but_not_an_unrelated_key() {
        // Audit finding: `samplename=world` was silently accepted.
        let keys = declared(&["sample_name", "threads", "min_quality"]);
        assert_eq!(
            closest_declared_key("samplename", &keys),
            Some("sample_name")
        );
        assert_eq!(closest_declared_key("threds", &keys), Some("threads"));
        assert_eq!(closest_declared_key("completely_different", &keys), None);
    }

    #[test]
    fn rejects_bare_non_key_value_positional() {
        let err =
            parse_cli_overrides(vec!["naked".to_string()], &declared(&[]), "run").unwrap_err();
        assert!(format!("{err}").contains("KEY=VALUE"), "{err:?}");
    }

    #[test]
    fn accepts_empty_value_overrides() {
        // issue #430: `KEY=` and `--KEY ""` are legitimate derive-if-empty
        // overrides (rnaseq gene_bed/chrom_sizes/transcript_fasta branch on
        // `[ -n "{config.x}" ]`).
        let map = parse_cli_overrides(
            vec![
                "gene_bed=".to_string(),
                "--chrom_sizes".to_string(),
                String::new(),
                "--transcript_fasta=".to_string(),
            ],
            &declared(&["gene_bed", "chrom_sizes", "transcript_fasta"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["gene_bed"], "");
        assert_eq!(map["chrom_sizes"], "");
        assert_eq!(map["transcript_fasta"], "");
    }

    #[test]
    fn rejects_empty_key_but_accepts_empty_value() {
        // "=value" trims to an empty KEY — still a syntax error.
        let err = parse_cli_overrides(vec!["=v".to_string()], &declared(&[]), "run").unwrap_err();
        assert!(
            format!("{err}").contains("KEY must be non-empty"),
            "{err:?}"
        );
    }

    #[test]
    fn dry_run_flag_error_is_command_aware() {
        // issue #430: `-j` under dry-run is not a misordered run flag — the
        // command does not support it at all. The message must say so
        // instead of implying a flag-ordering problem.
        let err = parse_cli_overrides(
            vec!["-j".to_string(), "12".to_string()],
            &declared(&[]),
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not supported by dry-run"), "{msg}");
        assert!(
            !msg.contains("command flag, not a config override"),
            "{msg}"
        );
    }

    #[test]
    fn run_flag_error_under_run_keeps_ordering_hint() {
        // `run` itself keeps the original ordering guidance (issue #71).
        let err = parse_cli_overrides(
            vec!["threads=8".to_string(), "-j".to_string()],
            &declared(&["threads"]),
            "run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("command flag"), "{msg}");
        assert!(msg.contains("before KEY=VALUE"), "{msg}");
    }

    #[test]
    fn unknown_token_under_dry_run_stays_command_aware() {
        // An unknown --token under a non-run command points at the command,
        // not at run's ordering rule.
        let err = parse_cli_overrides(
            vec!["--config".to_string(), "x".to_string()],
            &declared(&[]),
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unknown argument"), "{msg}");
        assert!(msg.contains("dry-run"), "{msg}");
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

    // ── Issue #432: dry-run headline + status --workdir / staleness ──────

    #[test]
    fn dry_run_headline_reports_run_skip_completed_not_dag_size() {
        // The headline must answer "what happens next" (issue #432b): the
        // old `DAG: (dry-run) N rules would execute` read as "everything
        // re-runs" while the checkpoint said most rules were up to date.
        use assert_cmd::Command;
        let wf_toml = r#"
[workflow]
name = "headline"

[[rules]]
name = "step"
input = []
output = ["out.txt"]
shell = "touch out.txt"
"#;
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();

        let assert = Command::cargo_bin("oxo-flow")
            .unwrap()
            .args(["dry-run", wf.to_str().unwrap()])
            .env("NO_COLOR", "1")
            .assert()
            .success();

        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
        assert!(
            stderr.contains("Plan: would run: 1 | skip: 0 | completed: 0 (DAG size: 1)"),
            "headline must lead with would-run/skip/completed, got: {stderr}"
        );
        assert!(
            !stderr.contains("rules would execute"),
            "the old misleading headline must be gone, got: {stderr}"
        );
    }

    #[test]
    fn status_accepts_workdir_like_run() {
        // Issue #432(d): `status --workdir <dir>` must resolve the default
        // checkpoint under that run directory, same as `run --workdir`.
        use assert_cmd::Command;
        let wf_toml = r#"
[workflow]
name = "status-workdir"

[[rules]]
name = "step"
input = []
output = ["out.txt"]
shell = "touch out.txt"
"#;
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let run_dir = dir.path().join("run1");

        // A real run first: it records the checkpoint in the workdir.
        Command::cargo_bin("oxo-flow")
            .unwrap()
            .args([
                "run",
                wf.to_str().unwrap(),
                "--workdir",
                run_dir.to_str().unwrap(),
                "-j",
                "1",
            ])
            .env("NO_COLOR", "1")
            .assert()
            .success();

        // status --workdir finds the checkpoint without the internal path.
        let assert = Command::cargo_bin("oxo-flow")
            .unwrap()
            .args(["status", "--workdir", run_dir.to_str().unwrap(), "--json"])
            .env("NO_COLOR", "1")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("status --json must emit JSON");
        assert_eq!(parsed["completed"], serde_json::json!(["step"]));
    }

    #[test]
    fn status_reports_staleness_reasons_for_a_stale_completed_rule() {
        // Issue #432(c): after a completed rule's output is deleted, status
        // must say WHY it would re-run (outputs missing), not just list it.
        use assert_cmd::Command;
        let wf_toml = r#"
[workflow]
name = "stale-reasons"

[[rules]]
name = "producer"
input = []
output = ["stage1.txt"]
shell = "printf 'v1' > stage1.txt"

[[rules]]
name = "consumer"
input = ["stage1.txt"]
output = ["stage2.txt"]
shell = "cp stage1.txt stage2.txt"
"#;
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join("wf.oxoflow");
        std::fs::write(&wf, wf_toml).unwrap();
        let run_dir = dir.path().join("run1");

        Command::cargo_bin("oxo-flow")
            .unwrap()
            .args([
                "run",
                wf.to_str().unwrap(),
                "--workdir",
                run_dir.to_str().unwrap(),
                "-j",
                "1",
            ])
            .env("NO_COLOR", "1")
            .assert()
            .success();

        // Delete the producer's output → it is stale (outputs missing) and
        // the consumer cascades behind it.
        std::fs::remove_file(run_dir.join("stage1.txt")).unwrap();

        let assert = Command::cargo_bin("oxo-flow")
            .unwrap()
            .args(["status", "--workdir", run_dir.to_str().unwrap(), "--json"])
            .env("NO_COLOR", "1")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let staleness = parsed["staleness"].as_object().expect("staleness map");
        let producer = &staleness["producer"];
        // stage1.txt was deleted: the producer's completion record survives
        // but its output is gone, so it classifies as the lazy cascade-up
        // producer (a downstream rule needs the intermediate).
        assert_eq!(producer["status"], "missing input");
        let consumer = &staleness["consumer"];
        // stage1.txt no longer matches the recorded manifest (gone), so the
        // consumer's own credit is input-invalidated before any cascade.
        assert_eq!(consumer["status"], "input changed");
        assert!(
            consumer["detail"].as_str().unwrap().contains("manifest"),
            "input-changed detail must explain the manifest mismatch, got: {consumer}"
        );
    }

    #[test]
    fn spawn_watchdog_threshold_parsing() {
        // issue #685: unset, garbage, and `0` all fall back to the default
        // — `0` matches the OXO_ENV_LOCK_TIMEOUT_SECS convention rather
        // than meaning "disable", so a typo can never silently turn the
        // watchdog off. The derivation is pure: no env mutation needed.
        assert_eq!(
            parse_spawn_watchdog_threshold(None),
            std::time::Duration::from_secs(600)
        );
        assert_eq!(
            parse_spawn_watchdog_threshold(Some("abc")),
            std::time::Duration::from_secs(600)
        );
        assert_eq!(
            parse_spawn_watchdog_threshold(Some("0")),
            std::time::Duration::from_secs(600)
        );
        assert_eq!(
            parse_spawn_watchdog_threshold(Some("120")),
            std::time::Duration::from_secs(120)
        );
    }

    #[test]
    fn watchdog_requeue_fires_parsing() {
        // issue #872: same convention as the threshold — unset, garbage, and
        // `0` all fall back to the default so a typo can never disable the
        // requeue (it would wedge the lane instead). Pure derivation.
        assert_eq!(parse_watchdog_requeue_fires(None), 6);
        assert_eq!(parse_watchdog_requeue_fires(Some("abc")), 6);
        assert_eq!(parse_watchdog_requeue_fires(Some("0")), 6);
        assert_eq!(parse_watchdog_requeue_fires(Some("3")), 3);
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
        assert!(super::backend_preflight_failures(&[], &resolver).is_empty());
        // A rule with no environment declaration never invokes a backend.
        let no_env = vec![("qc".to_string(), Default::default())];
        assert!(super::backend_preflight_failures(&no_env, &resolver).is_empty());
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
        let failures = super::backend_preflight_failures(&pending, &resolver);
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
        let make = |message: &str| super::BackendPreflightFailure {
            kind: "pixi".to_string(),
            message: message.to_string(),
            example_rule: "trim_S1".to_string(),
            spec: "envs/absent/pixi.toml".to_string(),
            count: 2,
        };
        let manifest = super::backend_install_hint(&make(
            "pixi manifest 'envs/absent/pixi.toml' not found — the rule declares it via `pixi = \"envs/absent/pixi.toml\"` (resolved relative to the workflow directory)",
        ));
        assert_eq!(
            manifest,
            "create the manifest or correct the rule's `pixi` path (resolved relative to the workflow directory)"
        );
        let binary = super::backend_install_hint(&make("pixi is not installed or not in PATH"));
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
        let failures = super::backend_preflight_failures(&pending, &resolver);
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
