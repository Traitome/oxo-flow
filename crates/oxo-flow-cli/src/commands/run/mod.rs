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
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) mod cluster_path;
mod completion;
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
mod scheduler_loop;
pub(crate) mod signals;
pub mod status;
mod task;

pub(crate) use completion::*;
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
pub(crate) use scheduler_loop::*;
pub(crate) use signals::*;
pub use status::handle_status;
pub(crate) use status::*;
pub(crate) use task::*;

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

/// Flatten workflow config values into the `{config.key}` placeholder map
/// used for path expansion (DAG edge matching, run-time rendering, …).
pub(crate) fn config_placeholder_values(
    config: &HashMap<String, toml::Value>,
) -> HashMap<String, String> {
    oxo_flow_core::config_impact::config_placeholder_map(config)
}

/// One distinct scientific-preflight finding with its instance count —
/// the (code, message, suggestion) grouping from the issue #441 pilot
/// summary, extracted so run-mode surfacing reports the same shape
/// without a second hand-copied grouping loop (reuse, never retype).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupedScientificWarning {
    pub code: String,
    /// Rule name of the FIRST instance (per-instance rule names differ in
    /// the post-expansion config; the group is the shared finding text).
    pub rule: String,
    pub message: String,
    pub instances: usize,
}

/// Group post-expansion preflight findings by content: identical
/// (code, message, suggestion) across sample instances is ONE distinct
/// finding with M instances. Preserves first-seen order.
pub(crate) fn group_scientific_warnings(
    warnings: &[oxo_flow_core::scientific_preflight::ScientificWarning],
) -> Vec<GroupedScientificWarning> {
    let mut distinct: Vec<GroupedScientificWarning> = Vec::new();
    for w in warnings {
        match distinct
            .iter_mut()
            .find(|g| g.code == w.code && g.message == w.message)
        {
            Some(g) => g.instances += 1,
            None => distinct.push(GroupedScientificWarning {
                code: w.code.clone(),
                rule: w.rule.clone(),
                message: w.message.clone(),
                instances: 1,
            }),
        }
    }
    distinct
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
    /// Issue #1017: when set, the engine computes the transitive-dependent
    /// closure of this rule in the DAG and invalidates their checkpoint
    /// entries before planning, so downstream rules re-execute with the
    /// new upstream outputs instead of silently passing the freshness
    /// gate with stale artifacts.
    pub from_rule: Option<String>,
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
        from_rule,
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
    let mut jobs = jobs.max(1);

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

    // ── [resource_budget] max_jobs clamp (issue #1013 E1) ───────────────
    // max_jobs caps the TOTAL concurrently running jobs for the run, so the
    // faithful semantics are a clamp of the requested `-j` concurrency, not
    // a per-rule breach error (a single rule cannot "exceed" a job count —
    // per-rule language applies only to max_memory/max_threads, which stay
    // hard errors in validate_resource_budget / check_budget). Both local
    // consumers below (ExecutorConfig.max_jobs and SchedulerLoopContext
    // .jobs) read this same variable, so one reassignment covers both.
    // Cluster-path parity lives in run_cluster.rs (max_submitted clamp).
    if let Some(max_jobs) = config.resource_budget.as_ref().and_then(|b| b.max_jobs)
        && jobs > max_jobs
    {
        eprintln!(
            "  {} [resource_budget] max_jobs = {} caps the requested -j {} — running at most {} jobs concurrently",
            "Warning:".bold().yellow(),
            max_jobs,
            jobs,
            max_jobs
        );
        jobs = max_jobs;
    }

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

    // Scientific preflight (dry-run parity): the constraint analysis runs at
    // dry-run and pilot-summary time; `run` itself stayed silent, so the
    // first real signal a user got was the tool misbehaving inside the
    // shell. Surface the same findings here — advisory only (warn, never
    // abort), reported once against the POST-expansion config, so identical
    // per-sample template findings are grouped into distinct instances
    // (issue #441 grouping, shared helper with the pilot summary).
    let scientific = oxo_flow_core::scientific_preflight::analyze_scientific_constraints(&config);
    if !scientific.is_empty() {
        let distinct = group_scientific_warnings(&scientific);
        diagnostic_narrate(
            format_args!("\n{}", "Scientific preflight:".bold().yellow()),
            Some(&run_log),
        );
        for g in &distinct {
            diagnostic_narrate(
                format_args!(
                    "    ⚠ [{}] {} (×{}): {}",
                    g.code, g.rule, g.instances, g.message
                ),
                Some(&run_log),
            );
        }
        diagnostic_narrate(
            format_args!(
                "  {} ({} across sample rules) — see 'oxo-flow dry-run' for full suggestions",
                oxo_flow_core::plural(
                    distinct.len(),
                    "distinct scientific preflight finding",
                    "distinct scientific preflight findings"
                ),
                oxo_flow_core::plural(scientific.len(), "instance", "instances")
            ),
            Some(&run_log),
        );
    }

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
    // The walk stats/unlinks with std::fs; off-thread like the #527
    // sibling write paths so a large or cold workdir cannot stall the
    // tokio worker at every run start (M-1).
    let stale_asides = {
        let workdir = workdir.as_ref().unwrap_or(&workdir_default).to_path_buf();
        tokio::task::spawn_blocking(move || {
            oxo_flow_core::executor::output_invalidation::cleanup_stale_failed_asides(
                &workdir,
                oxo_flow_core::executor::output_invalidation::OXOX_FAILED_RETENTION_DAYS,
            )
        })
        .await
        .unwrap_or(0)
    };
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

    // ── Issue #1017: downstream-closure invalidation for --from-rule ─────
    // When the user asks to re-run from a specific rule (`--from-rule X`),
    // every transitive dependent of X must re-execute with X's new outputs —
    // not silently pass the freshness gate with artifacts built from the
    // old X. Compute the closure in the DAG, invalidate those checkpoint
    // entries, and persist the invalidated state before any scheduling
    // decision is made. Runs BEFORE the freshness gate so checksum-verified
    // outputs (E6 machinery) also re-derive.
    if let Some(ref from_rule_name) = from_rule {
        let closure: Vec<String> = dag.transitive_dependents(from_rule_name)?;
        if !closure.is_empty() {
            let mut ck = checkpoint.lock().await;
            ck.invalidate_closure(&closure);
            drop(ck);
            // Persist the invalidation so a concurrent reader or a crash
            // before the first save still sees the closure as un-done.
            checkpoint.lock().await.save_to_file(&checkpoint_path)?;
            diagnostic_narrate(
                format_args!(
                    "  {} --from-rule: invalidated checkpoint for {} downstream \
                     of '{}' — they will re-execute.",
                    "Info:".bold().cyan(),
                    oxo_flow_core::plural(closure.len(), "rule", "rules"),
                    from_rule_name,
                ),
                Some(&run_log),
            );
        }
    }

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
                    crate::commands::run_preview::will_run_seed(
                        name,
                        &ck.completed_rules,
                        &tombstone_keep,
                        &config,
                        &wildcard_values,
                        workdir_actual.as_ref(),
                    )
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
        // The remote wrapper accepts a fixed flag surface; warn ONLY for the
        // flags the user actually passed (a blanket banner on every cluster
        // run was noise). ClusterRunArgs deliberately does not carry these —
        // they are parsed by clap but dropped by this dispatch.
        for (passed, flag, note) in [
            (
                retry > 0,
                "--retry",
                "cluster drivers poll their own retry/backoff semantics",
            ),
            (
                keep_going,
                "--keep-going",
                "the driver keeps dispatching independent jobs until the queue drains",
            ),
            (
                timeout_secs > 0,
                "--timeout",
                "per-job walltime is set through -w/--walltime instead",
            ),
            (
                provenance,
                "--provenance",
                "record output checksums on the local path, then submit",
            ),
            (
                max_threads != 0 || max_memory != 0,
                "--max-threads/--max-memory",
                "per-rule [resources] and the [cluster] profile drive scheduler limits",
            ),
        ] {
            if passed {
                eprintln!(
                    "  {} {flag} is not honored on the cluster path — {note}",
                    "Warning:".bold().yellow()
                );
            }
        }
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
                // [engine] tmp_dir / OXO_FLOW_TMPDIR follow the job onto
                // the cluster (issue #1013 C2) — the same resolved value
                // the local path injects into its process envs.
                tmp_dir: tmp_dir_resolved.as_deref(),
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

    run_scheduler_loop(SchedulerLoopContext {
        jobs,
        order: &mut order,
        config: &mut config,
        dag: &mut dag,
        tombstone_keep: &tombstone_keep,
        workdir_actual: &workdir_actual,
        tmp_dir_resolved: tmp_dir_resolved.as_deref(),
        checkpoint: &checkpoint,
        checkpoint_path: &checkpoint_path,
        executor: &executor,
        run_log: &run_log,
        progress: &progress,
        is_tty,
        keep_going,
        rerun,
        ai_recover,
        ai_max_retries,
        no_report_snapshot,
        provenance,
        sensitive_values: &sensitive_values,
        wildcard_values: &wildcard_values,
        samples_filter: &samples_filter,
        workflow: &workflow,
        json_summary: &mut json_summary,
    })
    .await
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

#[cfg(test)]
mod scientific_preflight_tests {
    use super::*;

    fn warning(
        code: &str,
        rule: &str,
        message: &str,
    ) -> oxo_flow_core::scientific_preflight::ScientificWarning {
        oxo_flow_core::scientific_preflight::ScientificWarning {
            code: code.to_string(),
            rule: rule.to_string(),
            message: message.to_string(),
            suggestion: "fix it".to_string(),
        }
    }

    #[test]
    fn identical_findings_across_sample_rules_collapse_to_one_group() {
        // Post-expansion: the same template finding appears once per sample
        // instance with a different rule name (issue #441).
        let warnings = vec![
            warning("SCI-TEST", "rule_S1", "same problem"),
            warning("SCI-TEST", "rule_S2", "same problem"),
            warning("SCI-TEST", "rule_S3", "same problem"),
        ];
        let groups = group_scientific_warnings(&warnings);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].code, "SCI-TEST");
        assert_eq!(groups[0].rule, "rule_S1");
        assert_eq!(groups[0].instances, 3);
    }

    #[test]
    fn different_messages_or_codes_stay_distinct() {
        let warnings = vec![
            warning("SCI-A", "rule_S1", "problem one"),
            warning("SCI-A", "rule_S2", "problem two"),
            warning("SCI-B", "rule_S1", "problem one"),
        ];
        let groups = group_scientific_warnings(&warnings);
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|g| g.instances == 1));
    }

    #[test]
    fn first_seen_order_is_preserved() {
        let warnings = vec![
            warning("SCI-B", "r1", "m1"),
            warning("SCI-A", "r2", "m2"),
            warning("SCI-B", "r3", "m1"),
        ];
        let groups = group_scientific_warnings(&warnings);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].code, "SCI-B");
        assert_eq!(groups[0].instances, 2);
        assert_eq!(groups[1].code, "SCI-A");
    }
}
