//! Checkpoint-aware run preview for `dry-run` (issue #66).
//!
//! Predicts what an actual `run` would do with the same checkpoint:
//! which rules re-execute (selected/invalidated/missing outputs), which
//! cascade downstream, and which stay protected by the checkpoint.
//!
//! The classification reuses the exact same detection machinery `run`
//! uses (config-impact fingerprints, input manifests, DAG downstream
//! closure) but operates on a CLONED checkpoint — the preview is strictly
//! read-only and never mutates the on-disk state.

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::checkpoint::CheckpointState;
use oxo_flow_core::rule::Rule;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Why a rule will execute (or not) in the predicted run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleStatus {
    /// No completed checkpoint entry — it has never run.
    NeverCompleted,
    /// Config change or rule-definition edit invalidated it (issue #62).
    ConfigInvalidated,
    /// Its input files changed since completion (issue #72).
    InputInvalidated,
    /// Declared outputs no longer exist.
    OutputsMissing,
    /// Was completed, but sits downstream of a rule that will execute.
    Cascaded { from: String },
    /// Checkpoint hit — will be skipped.
    Skipped,
    /// Its `when` condition evaluates to false — `run` skips it regardless
    /// of invalidation state, so the preview does too.
    SkippedByWhen,
    /// Was completed, but a dependent's inputs are missing — it re-executes
    /// first to regenerate them (cascade-up after tombstoned temporaries).
    CascadedUpstream { from: String },
    /// `--rerun` forced re-execution: every up-to-date check is bypassed
    /// (the preview mirrors `run --rerun`'s execution set).
    Forced,
    /// No checkpoint entry, but the executor's freshness gate would skip
    /// it anyway (outputs exist and are newer than inputs — e.g. leftovers
    /// from a run whose checkpoint was never written after a crash or a
    /// failed exit).
    SkippedFresh,
}

impl RuleStatus {
    /// Whether this rule will be skipped rather than executed. The single
    /// definition of the will-run boundary: the preview's `will_skip` count
    /// and the cluster path's submission set both read it, so a new skip
    /// variant cannot mean "skip" in one place and "run" in the other.
    pub fn is_skip(&self) -> bool {
        matches!(
            self,
            RuleStatus::Skipped | RuleStatus::SkippedByWhen | RuleStatus::SkippedFresh
        )
    }
}

/// One rule in the predicted plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRule {
    pub name: String,
    pub status: RuleStatus,
}

/// The predicted execution plan for one dry-run invocation.
#[derive(Debug, Clone)]
pub struct RunPreview {
    /// Checkpoint the prediction was computed from (may not exist on disk).
    pub checkpoint_path: std::path::PathBuf,
    /// File modification time — the "last run" proxy the checkpoint format
    /// itself does not record.
    pub checkpoint_modified: Option<std::time::SystemTime>,
    /// Completed entries in the checkpoint before this run.
    pub completed_total: usize,
    /// Rules in the execution set that will execute (any non-Skipped status).
    pub plan: Vec<PreviewRule>,
    /// Rules in the execution set that will be skipped.
    pub will_skip: usize,
    /// Completed rules OUTSIDE this execution set — work this run preserves.
    pub protected_outside: usize,
    /// Cascade chains (seed → … → queue-level rule) for the display.
    pub cascade_chains: Vec<Vec<String>>,
}

/// The executor's freshness gate for a rule with no checkpoint entry:
/// all outputs exist and (when inputs exist) every output is newer than
/// every input. `run` skips such rules as "outputs up-to-date" even
/// without a completion record; the preview mirrors it.
///
/// Delegates to the executor's own gate (`checkpoint::should_skip_rule`,
/// the same function process.rs evaluates) so the two cannot drift: a path
/// that still contains `{` — an undeclared wildcard like `{sample}` or an
/// unexpanded config ref — is NOT fresh on the run side (the executor
/// executes the rule), so the preview must not classify it as "will skip:
/// outputs up-to-date" (issue #142 H3: dry-run said skip while `run`
/// actually wrote a literal `out_{sample}.txt`).
fn rule_outputs_exist_fresh(
    rule: &oxo_flow_core::rule::Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    checksums: Option<&HashMap<String, String>>,
) -> bool {
    oxo_flow_core::executor::checkpoint::should_skip_rule_with_checksums(
        rule,
        workdir,
        wildcard_values,
        checksums,
    )
}

/// Storage resolver for manifest snapshots and remote staging: local by
/// default; cloud backends register when the CLI crate opts into the
/// `s3-storage` / `gcs-storage` features (issue #78 P2 / #80 — remote
/// inputs then get etag-aware invalidation and local staging end to end).
/// Shared by run and dry-run so their snapshot semantics cannot drift.
pub fn storage_resolver() -> oxo_flow_core::storage::StorageResolver {
    // `mut` is only used when a cloud-storage feature is enabled.
    #[allow(unused_mut)]
    let mut resolver = oxo_flow_core::storage::StorageResolver::with_local();
    #[cfg(feature = "s3-storage")]
    resolver.add_backend(
        oxo_flow_core::storage::StorageScheme::S3,
        std::sync::Arc::new(oxo_flow_core::storage::s3::S3Storage::new()),
    );
    #[cfg(feature = "gcs-storage")]
    resolver.add_backend(
        oxo_flow_core::storage::StorageScheme::Gcs,
        std::sync::Arc::new(oxo_flow_core::storage::gcs::GcsStorage),
    );
    resolver
}

/// Compute the read-only preview for an execution set.
///
/// Mirrors `run`'s preprocessing exactly: config-impact detection first,
/// then input-manifest comparison, then DAG downstream closure. All
/// mutations happen on a clone of `ck`.
#[allow(clippy::too_many_arguments)]
pub fn preview_run_plan(
    ck: &CheckpointState,
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    order: &[String],
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    sensitive_keys: &HashSet<String>,
    interpreter_map: &HashMap<String, String>,
    checkpoint_path: &Path,
    rerun: bool,
    resume_failed: bool,
) -> RunPreview {
    let completed_original: HashSet<String> = ck.completed_rules.clone();
    let mut clone = ck.clone();
    // `--resume-failed` re-executes failed rules: `run` clears the failed
    // set from its in-memory checkpoint before scheduling; the preview
    // mirrors that on its clone (failed rules classify as NeverCompleted).
    if resume_failed {
        clone.failed_rules.clear();
    }

    // `--rerun` bypasses every up-to-date check on the run side (the whole
    // invalidation block is skipped); the preview mirrors that: no
    // invalidation machinery, every rule in the execution set is forced —
    // except `when`-false rules, which dominate even forced re-runs.
    if rerun {
        let mut plan = Vec::with_capacity(order.len());
        let mut will_skip = 0usize;
        for name in order {
            let status = if config
                .get_rule(name)
                .is_some_and(|rule| when_condition_false(rule, config, wildcard_values))
            {
                will_skip += 1;
                RuleStatus::SkippedByWhen
            } else {
                RuleStatus::Forced
            };
            plan.push(PreviewRule {
                name: name.clone(),
                status,
            });
        }
        return RunPreview {
            checkpoint_path: checkpoint_path.to_path_buf(),
            checkpoint_modified: std::fs::metadata(checkpoint_path)
                .and_then(|m| m.modified())
                .ok(),
            completed_total: completed_original.len(),
            plan,
            will_skip,
            protected_outside: completed_original
                .iter()
                .filter(|name| !order.contains(name))
                .count(),
            cascade_chains: Vec::new(),
        };
    }

    // 1. Config-impact invalidation (issue #62) — on the clone only.
    // `file_exists(...)` in `when` resolves against the workflow root
    // (issue #241) — same root as run and plan-time expansion.
    let config_report = oxo_flow_core::config_impact::detect_config_changes_with_replay(
        &mut clone,
        &config.rules,
        dag,
        &config.config,
        sensitive_keys,
        interpreter_map,
        config.defaults.shell_prelude.as_deref(),
        config.base_dir(),
        Some(workdir),
        Some(config),
    );
    let config_invalidated: HashSet<String> = config_report.invalidated.iter().cloned().collect();
    // Issue #142 M1: rules whose fingerprint differed only in the
    // sample-derived input list stay completed — warn (mirrors run), and
    // the input-manifest check below re-verifies set + content, so a
    // genuine input edit still invalidates.
    for name in &config_report.sample_selection_exempt {
        eprintln!(
            "  {} rule '{}' skipped: its definition only changed with the --samples selection, not a real edit — its outputs still cover the previous run's full sample set",
            "⚠".yellow(),
            name
        );
    }

    // 2. Input-manifest invalidation (issue #72) — on the clone only.
    //    Missing inputs (typically tombstoned temporaries) cascade UP: the
    //    completed producers re-execute first, exactly like run.
    let (manifest_invalidated, missing_inputs, _baselined, sample_selection_driven) =
        detect_input_manifest_invalidations(
            &mut clone,
            config,
            dag,
            order,
            workdir,
            wildcard_values,
        );
    // Issue #142 M1: rules whose ONLY input change is the engine-injected
    // sample-list stay skipped — warn (mirrors run), never invalidate.
    for name in sample_selection_driven.iter() {
        eprintln!(
            "  {} rule '{}' skipped: its inputs only reflect the --samples selection, not a real change — its outputs still cover the previous run's full sample set",
            "⚠".yellow(),
            name
        );
    }
    // Genuinely missing inputs cascade UP: every completed producer of the
    // missing files re-executes first (exactly like run).
    let mut upstream_set: HashSet<String> = cascade_up(&mut clone, dag, &missing_inputs)
        .into_iter()
        .collect();

    // 3. DAG downstream closure of the manifest mismatches — the same
    //    cascade `run` applies.
    let seeds: HashSet<String> = manifest_invalidated.iter().cloned().collect();
    invalidate_with_downstream(&mut clone, dag, &seeds);

    // 3b. Tombstone-aware skip: a tombstoned rule stays skipped while every
    //     dependent remains completed (mirrors run's skip loop).
    let tombstone_keep: HashSet<String> = clone
        .tombstones
        .keys()
        .filter(|rule| {
            dag.dependents(rule)
                .map(|deps| deps.iter().all(|d| clone.completed_rules.contains(d)))
                .unwrap_or(true)
        })
        .cloned()
        .collect();

    // 3c. Rules that WILL execute (never completed, invalidated, cascaded,
    //     or completed with outputs missing) may depend on tombstoned
    //     producers whose outputs were deleted by design — those producers
    //     must regenerate first, like run's lazy cascade-up.
    let will_run_seeds: HashSet<String> = order
        .iter()
        .filter(|name| {
            !clone.completed_rules.contains(*name)
                || (!tombstone_keep.contains(*name)
                    && !config
                        .get_rule(name)
                        .is_some_and(|rule| when_condition_false(rule, config, wildcard_values))
                    && config
                        .get_rule(name)
                        .is_some_and(|rule| !rule_outputs_exist(rule, workdir, wildcard_values)))
        })
        .cloned()
        .collect();
    upstream_set.extend(cascade_up_tombstoned(&mut clone, dag, &will_run_seeds));

    // 4. Classify every rule in the execution set.
    let seeds_for_cascade: HashSet<String> = config_invalidated
        .union(&manifest_invalidated)
        .cloned()
        .collect();
    let order_set: HashSet<&str> = order.iter().map(String::as_str).collect();
    let mut plan = Vec::with_capacity(order.len());
    let mut will_skip = 0usize;
    for name in order {
        let status = if config
            .get_rule(name)
            .is_some_and(|rule| when_condition_false(rule, config, wildcard_values))
        {
            // Evaluated exactly like run does (typed config values win over
            // string wildcard values); a false condition dominates every
            // other consideration — even forced re-runs.
            RuleStatus::SkippedByWhen
        } else if !completed_original.contains(name) {
            // A rule absent from the checkpoint can still be config-
            // invalidated — `run` puts it in force_rules, which bypasses
            // the executor freshness gate, so config invalidation wins here.
            if config_invalidated.contains(name) {
                RuleStatus::ConfigInvalidated
            } else if config.get_rule(name).is_some_and(|rule| {
                rule_outputs_exist_fresh(rule, workdir, wildcard_values, Some(&ck.checksums))
            }) {
                // `run` applies the executor freshness gate to rules with
                // no checkpoint entry: up-to-date outputs skip even without
                // a completion record (issue #77 parity — crash leftovers).
                RuleStatus::SkippedFresh
            } else {
                RuleStatus::NeverCompleted
            }
        } else if config_invalidated.contains(name) {
            RuleStatus::ConfigInvalidated
        } else if manifest_invalidated.contains(name) {
            RuleStatus::InputInvalidated
        } else if tombstone_keep.contains(name) && !upstream_set.contains(name) {
            // Outputs were deleted by design; nothing downstream needs them.
            RuleStatus::Skipped
        } else if upstream_set.contains(name) {
            // A completed producer whose outputs a dependent needs again —
            // it re-executes before that dependent. Attribute the nearest
            // dependent that triggered the regeneration.
            let upstream_seeds: HashSet<String> =
                missing_inputs.union(&will_run_seeds).cloned().collect();
            RuleStatus::CascadedUpstream {
                from: dependent_seed(name, dag, &upstream_seeds),
            }
        } else if let Some(rule) = config.get_rule(name)
            && !rule_outputs_exist(rule, workdir, wildcard_values)
        {
            RuleStatus::OutputsMissing
        } else if !clone.completed_rules.contains(name) {
            // Was completed at the start but the closure removed it — a
            // cascaded invalidation. Attribute the nearest seed.
            RuleStatus::Cascaded {
                from: nearest_seed(name, dag, &seeds_for_cascade),
            }
        } else {
            RuleStatus::Skipped
        };
        if status.is_skip() {
            will_skip += 1;
        }
        plan.push(PreviewRule {
            name: name.clone(),
            status,
        });
    }

    // 5. Cascade chains for display: from each seed that was previously
    //    completed, walk dependents within the execution set.
    let mut cascade_chains: Vec<Vec<String>> = Vec::new();
    for seed in seeds_for_cascade.iter() {
        if !completed_original.contains(seed) {
            continue; // never completed — nothing was "infected"
        }
        let chain = cascade_chain(seed, dag, order_set.clone());
        if chain.len() > 1 {
            cascade_chains.push(chain);
        }
    }
    cascade_chains.sort_by(|a, b| a.first().cmp(&b.first()));

    let protected_outside = completed_original
        .iter()
        .filter(|name| !order_set.contains(name.as_str()))
        .count();

    RunPreview {
        checkpoint_path: checkpoint_path.to_path_buf(),
        checkpoint_modified: std::fs::metadata(checkpoint_path)
            .and_then(|m| m.modified())
            .ok(),
        completed_total: completed_original.len(),
        plan,
        will_skip,
        protected_outside,
        cascade_chains,
    }
}

/// Walk dependents of `seed` (in execution-set order) that were completed —
/// the displayed infection chain.
fn cascade_chain(seed: &str, dag: &WorkflowDag, order_set: HashSet<&str>) -> Vec<String> {
    let mut chain = vec![seed.to_string()];
    let mut frontier = vec![seed.to_string()];
    let mut visited: HashSet<String> = frontier.iter().cloned().collect();
    while let Some(name) = frontier.pop() {
        let Ok(dependents) = dag.dependents(&name) else {
            continue;
        };
        let mut next: Vec<String> = dependents
            .into_iter()
            .filter(|d| order_set.contains(d.as_str()))
            .filter(|d| visited.insert(d.clone()))
            .collect();
        next.sort();
        chain.extend(next.iter().cloned());
        frontier.extend(next);
    }
    chain
}

/// The (sorted-first) missing-input rule that depends on `name` — the
/// "from" attribution for cascade-up.
fn dependent_seed(name: &str, dag: &WorkflowDag, seeds: &HashSet<String>) -> String {
    let mut sorted: Vec<&String> = seeds.iter().collect();
    sorted.sort();
    for seed in sorted {
        if let Ok(dependencies) = dag.dependencies(seed)
            && dependencies.iter().any(|d| d == name)
        {
            return seed.clone();
        }
    }
    "<unknown>".to_string()
}

/// Nearest invalidation seed that reaches `rule` through dependents.
fn nearest_seed(rule: &str, dag: &WorkflowDag, seeds: &HashSet<String>) -> String {
    if seeds.contains(rule) {
        return rule.to_string();
    }
    // BFS upstream along dependencies: prefer the first seed in
    // lexicographic order for deterministic output.
    let mut sorted_seeds: Vec<&String> = seeds.iter().collect();
    sorted_seeds.sort();
    for seed in sorted_seeds {
        if let Some(path) = dag_path_exists(seed, rule, dag) {
            let _ = path;
            return seed.clone();
        }
    }
    "<unknown>".to_string()
}

/// Whether `from` can reach `to` via dependency edges (BFS).
fn dag_path_exists(from: &str, to: &str, dag: &WorkflowDag) -> Option<usize> {
    let mut frontier = vec![(from.to_string(), 0)];
    let mut visited: HashSet<String> = frontier.iter().map(|(n, _)| n.clone()).collect();
    while let Some((name, depth)) = frontier.pop() {
        if name == to {
            return Some(depth);
        }
        let Ok(dependents) = dag.dependents(&name) else {
            continue;
        };
        for dependent in dependents {
            if visited.insert(dependent.clone()) {
                frontier.push((dependent, depth + 1));
            }
        }
    }
    None
}

/// Whether every declared output of the rule exists (the same check `run`
/// performs before re-submitting a completed rule).
pub fn rule_outputs_exist(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> bool {
    rule.output.iter().all(|output| {
        let expanded =
            oxo_flow_core::executor::checkpoint::expand_config_in_path(output, wildcard_values);
        expanded.contains('{') || workdir.join(&expanded).exists()
    })
}

/// Compare completed rules' input manifests against the current file set.
///
/// Returns (content-mismatched rule names, rules whose inputs are MISSING,
/// number of legacy-baseline adoptions, rules whose only mismatch is the
/// engine-injected sample-selection change). The missing set drives
/// cascade-up: the completed producers of those inputs must re-execute
/// first. The sample-selection set stays completed — its mismatch does not
/// invalidate (issue #142 M1, documented run.md contract) and is surfaced
/// as a warning by the caller.
///
/// Mutates `ck` by recording baselines for legacy checkpoints — exactly
/// like `run` does; pass a clone for a read-only preview.
pub fn detect_input_manifest_invalidations(
    ck: &mut CheckpointState,
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    order: &[String],
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> (HashSet<String>, HashSet<String>, usize, HashSet<String>) {
    let mut mismatched: HashSet<String> = HashSet::new();
    let mut missing_inputs: HashSet<String> = HashSet::new();
    let mut baselined = 0usize;
    let mut sample_selection_driven: HashSet<String> = HashSet::new();
    for name in order {
        if !ck.completed_rules.contains(name) {
            continue;
        }
        let Some(rule) = config.get_rule(name) else {
            continue;
        };
        match oxo_flow_core::executor::checkpoint::snapshot_input_manifest(
            rule,
            workdir,
            wildcard_values,
            &storage_resolver(),
        ) {
            Ok(Some(current)) => match ck.input_manifests.get(name) {
                Some(recorded)
                    if oxo_flow_core::executor::checkpoint::manifests_match(recorded, &current) => {
                }
                Some(recorded) => {
                    // The file set changed — but if the change is exactly
                    // the engine's injected sample list (expand_inputs over
                    // config.samples_list / samples_<group> / pairs_list),
                    // toggling --samples must stay cheap (run.md contract):
                    // the rule stays completed, its outputs keep reflecting
                    // the previous run's sample set. Anything else — a
                    // genuine file edit, a user-touched set — invalidates
                    // as before (issue #142 M1).
                    if manifest_mismatch_is_sample_selection(rule, recorded, &current, config) {
                        sample_selection_driven.insert(name.clone());
                    } else {
                        mismatched.insert(name.clone());
                    }
                }
                None => {
                    // Legacy baseline: adopt the current set.
                    ck.record_input_manifest(name, current);
                    baselined += 1;
                }
            },
            Ok(None) => {}
            Err(_) => {
                // Inputs cannot be resolved — files are missing. If every
                // unresolvable input is the tombstone of a completed
                // producer (a temporary rule deleted its outputs by design
                // after all dependents finished), nothing needs to happen:
                // the rule stays completed. Genuinely missing inputs
                // invalidate the rule and cascade up to the producers.
                let missing = oxo_flow_core::executor::checkpoint::missing_input_patterns(
                    rule,
                    workdir,
                    wildcard_values,
                );
                let explained_by_tombstones = !missing.is_empty()
                    && missing.iter().all(|pattern| {
                        dag.producer_of(pattern).is_some_and(|producer| {
                            ck.tombstones.contains_key(producer)
                                && ck.completed_rules.contains(producer)
                        })
                    });
                if !explained_by_tombstones {
                    mismatched.insert(name.clone());
                    missing_inputs.insert(name.clone());
                }
            }
        }
    }
    (
        mismatched,
        missing_inputs,
        baselined,
        sample_selection_driven,
    )
}

/// Whether a recorded-vs-current input-manifest mismatch is caused solely by
/// the engine's injected sample-list values changing between runs (issue
/// #142 M1).
///
/// The documented contract (run.md): `samples_list` / `samples_<group>` /
/// `pairs_list` are engine-injected and never trigger invalidation, so
/// toggling `--samples` stays cheap. Config-impact detection already
/// excludes those keys; the input-manifest path must too — a gather rule
/// whose input list is baked from `expand_inputs` over `config.samples_list`
/// would otherwise re-run on every subset run and overwrite cohort outputs
/// with a subset table.
///
/// Three conditions, ALL verified — the exclusion is never broad:
/// 1. Some `expand_inputs` pattern of the rule resolves against an
///    engine-injected config key; any other input source keeps normal
///    (invalidate) behavior.
/// 2. Re-expanding under the CURRENT config reproduces the CURRENT manifest
///    path set exactly — the file set is fully determined by the injected
///    value; a user-added/removed/touched file set would not reproduce.
/// 3. Every path present in BOTH manifests is content-identical — a real
///    edit to a file the rule consumes still invalidates.
fn manifest_mismatch_is_sample_selection(
    rule: &Rule,
    recorded: &[oxo_flow_core::executor::checkpoint::InputManifestEntry],
    current: &[oxo_flow_core::executor::checkpoint::InputManifestEntry],
    config: &WorkflowConfig,
) -> bool {
    let Some(expected) = engine_expanded_inputs_from_injected(rule, config) else {
        return false;
    };
    // (2) The current manifest must be exactly the engine-derived set.
    let expected_paths: HashSet<&str> = expected.iter().map(String::as_str).collect();
    let current_paths: HashSet<&str> = current.iter().map(|e| e.path.as_str()).collect();
    if current_paths != expected_paths {
        return false;
    }
    // (3) Overlapping entries must be identical (size/hash-aware — the
    // `PartialEq` on entries covers remote etags too).
    let current_by_path: HashMap<&str, &oxo_flow_core::executor::checkpoint::InputManifestEntry> =
        current.iter().map(|e| (e.path.as_str(), e)).collect();
    recorded
        .iter()
        .filter(|r| current_by_path.contains_key(r.path.as_str()))
        .all(|r| current_by_path[r.path.as_str()] == r)
}

/// The input paths a rule's `expand_inputs` patterns bake to under the
/// CURRENT config — the same resolution `expand_wildcards` applies at run
/// start (config.rs: `resolve_config_list` + `cartesian_expand`, literal
/// lists, and per-instance `[[values]]` bindings).
///
/// Returns None when no `expand_inputs` pattern resolves against an
/// engine-injected config key (`samples_list` / `samples_<group>` /
/// `pairs_list`) — the only rules whose manifest mismatch can be a pure
/// sample-selection change. `[[values]]` per-instance bindings are not
/// re-applied here: a pattern depending on them fails the path-set
/// reproduction check (condition 2) and falls back to normal invalidation —
/// conservative by construction.
fn engine_expanded_inputs_from_injected(
    rule: &Rule,
    config: &WorkflowConfig,
) -> Option<Vec<String>> {
    let mut touched_injected = false;
    let mut expanded: Vec<String> = Vec::new();
    for exp in &rule.expand_inputs {
        let mut variables: HashMap<String, Vec<String>> = HashMap::new();
        for (var_name, var_ref) in &exp.variables {
            let referenced = var_ref.strip_prefix("config.").unwrap_or(var_ref);
            if oxo_flow_core::config_impact::is_engine_injected_key(referenced) {
                touched_injected = true;
            }
            if let Some(vals) = config.resolve_config_list(var_ref) {
                variables.insert(var_name.clone(), vals);
            } else if var_ref.starts_with('[') && var_ref.ends_with(']') {
                let inner = &var_ref[1..var_ref.len() - 1];
                let vals = inner
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                variables.insert(var_name.clone(), vals);
            } else {
                variables.insert(var_name.clone(), vec![var_ref.clone()]);
            }
        }
        expanded.extend(oxo_flow_core::wildcard::cartesian_expand(
            &exp.pattern,
            &variables,
        ));
    }
    touched_injected.then_some(expanded)
}

/// Remove the completed UPSTREAM dependencies of `seeds` from the completed
/// set (cascade-up): a rule whose inputs are missing needs its producers to
/// re-execute first. Returns every affected producer, sorted.
pub(crate) fn cascade_up(
    ck: &mut CheckpointState,
    dag: &WorkflowDag,
    seeds: &HashSet<String>,
) -> Vec<String> {
    let mut upstream: HashSet<String> = HashSet::new();
    for seed in seeds {
        if let Ok(dependencies) = dag.dependencies(seed) {
            for dep in dependencies {
                if ck.completed_rules.contains(&dep) {
                    upstream.insert(dep);
                }
            }
        }
    }
    for name in &upstream {
        ck.completed_rules.remove(name);
    }
    let mut names: Vec<String> = upstream.into_iter().collect();
    names.sort();
    names
}

/// Remove completed TOMBSTONED producers needed by `seeds` (rules that will
/// execute) from the completed set — lazy regeneration: a temporary rule's
/// outputs were deleted by design, so any rule about to consume them needs
/// its producer to re-execute first. Walks transitively so a chain of
/// tombstoned producers regenerates from the deepest one. Returns every
/// affected producer, sorted.
pub(crate) fn cascade_up_tombstoned(
    ck: &mut CheckpointState,
    dag: &WorkflowDag,
    seeds: &HashSet<String>,
) -> Vec<String> {
    let mut upstream: HashSet<String> = HashSet::new();
    let mut frontier: Vec<String> = seeds.iter().cloned().collect();
    while let Some(name) = frontier.pop() {
        let Ok(dependencies) = dag.dependencies(&name) else {
            continue;
        };
        for dep in dependencies {
            if ck.tombstones.contains_key(&dep)
                && ck.completed_rules.contains(&dep)
                && upstream.insert(dep.clone())
            {
                frontier.push(dep);
            }
        }
    }
    for name in &upstream {
        ck.completed_rules.remove(name);
    }
    let mut names: Vec<String> = upstream.into_iter().collect();
    names.sort();
    names
}

/// Remove `seeds` and their DAG downstream from the completed set, returning
/// every affected name (the same cascade `run` applies, issue #72).
pub(crate) fn invalidate_with_downstream(
    ck: &mut CheckpointState,
    dag: &WorkflowDag,
    seeds: &HashSet<String>,
) -> Vec<String> {
    let mut invalidated: HashSet<String> = seeds.clone();
    let mut frontier: Vec<String> = seeds.iter().cloned().collect();
    while let Some(name) = frontier.pop() {
        if let Ok(dependents) = dag.dependents(&name) {
            for dependent in dependents {
                if invalidated.insert(dependent.clone()) {
                    frontier.push(dependent);
                }
            }
        }
    }
    for name in &invalidated {
        ck.completed_rules.remove(name);
    }
    let mut names: Vec<String> = invalidated.into_iter().collect();
    names.sort();
    names
}

/// Whether the rule's `when` condition evaluates to false against the
/// merged config — the same inputs `run` evaluates it with (typed config
/// values win over string wildcard values; process.rs mirrors this).
/// `file_exists(...)` resolves against the workflow root (issue #241).
pub(crate) fn when_condition_false(
    rule: &Rule,
    config: &WorkflowConfig,
    wildcard_values: &HashMap<String, String>,
) -> bool {
    let Some(condition) = rule.when.as_deref() else {
        return false;
    };
    let mut config_values: HashMap<String, toml::Value> = config.config.clone();
    for (k, v) in wildcard_values {
        if let Some(key) = k.strip_prefix("config.") {
            config_values
                .entry(key.to_string())
                .or_insert_with(|| toml::Value::String(v.clone()));
        }
    }
    // Every rule output of this run is pending at plan time: an existing
    // file at one of those paths is a previous run's leftover that the
    // producer will overwrite, so its stale value must not decide the
    // gate here (issue #282 review). The verdict is deferred (true) and
    // re-checked at execution time.
    let pending_outputs: Vec<String> = config
        .rules
        .iter()
        .flat_map(|r| {
            let mut outs = r.output.to_vec();
            if let Some(p) = &r.output_pattern {
                outs.push(p.clone());
            }
            outs
        })
        .collect();
    !oxo_flow_core::executor::process::evaluate_condition_with_wildcards_base_dir_and_pending_outputs(
        condition,
        &config_values,
        &HashMap::new(),
        config.base_dir(),
        &pending_outputs,
    )
}

/// The profile names available in `<workflow-dir>/profiles/` — `<NAME>`
/// from every `<NAME>.toml` / `<NAME>.oxoflow` file, sorted and deduped.
fn available_profiles(workflow_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(workflow_dir.join("profiles")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            for ext in [".toml", ".oxoflow"] {
                if let Some(stripped) = name.strip_suffix(ext) {
                    return Some(stripped.to_string());
                }
            }
            None
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Merge a profile's `[config]` table into the workflow config — the same
/// semantics `run` applies. Profile values only FILL IN keys the workflow
/// does not set (`or_insert`); profile lookup is workflow-dir-only:
/// `<workflow-dir>/profiles/<NAME>.toml`, then `.oxoflow`; a missing
/// profile is a HARD error naming every available profile (issue #142 H4 —
/// a typo'd `--profile` previously warned and ran with the wrong config).
pub(crate) fn merge_profile(
    config: &mut WorkflowConfig,
    profile_name: &str,
    workflow_dir: &Path,
) -> Result<()> {
    use colored::Colorize;

    let profile_paths = [
        workflow_dir
            .join("profiles")
            .join(format!("{profile_name}.toml")),
        workflow_dir
            .join("profiles")
            .join(format!("{profile_name}.oxoflow")),
    ];
    let profile_path = profile_paths.iter().find(|p| p.exists());
    let Some(path) = profile_path else {
        let available = available_profiles(workflow_dir);
        let hint = if available.is_empty() {
            "the profiles/ directory does not exist or contains no .toml/.oxoflow profile files"
                .to_string()
        } else {
            format!("available profiles: {}", available.join(", "))
        };
        return Err(anyhow::anyhow!(
            "profile '{}' not found in profiles/ directory ({hint})",
            profile_name
        ));
    };
    let profile_content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read profile {}", path.display()))?;
    // NOTE: `toml::Value::from_str` in toml 1.x parses a SINGLE inline
    // value, not a document — `[config]` would fail with "unexpected
    // content". Parse the document explicitly (this also fixes the
    // pre-existing run --profile bug the shared extraction surfaced).
    let profile_toml: toml::Value = toml::from_str(&profile_content)
        .with_context(|| format!("failed to parse profile {}", path.display()))?;
    // The core merge honors `[workflow] profile_mode = "fill" | "override"`
    // (default fill = the legacy or_insert behavior; override replaces
    // workflow values — the "cluster profile switches threads/memory" case).
    config
        .merge_profile(&profile_toml)
        .map_err(|e| anyhow::anyhow!("profile '{}' merge failed: {e}", profile_name))?;
    eprintln!(
        "{} Applied config values from profile '{}'",
        "Profile:".bold().cyan(),
        profile_name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::config::WorkflowConfig;
    use oxo_flow_core::executor::checkpoint::snapshot_input_manifest;

    /// Issue #241: a `when` with a relative `file_exists(...)` gate must
    /// resolve against the workflow root — the process cwd is irrelevant.
    /// Regression scenario from the snparcher port: launching from a
    /// different directory silently flipped every gated rule.
    #[test]
    fn when_file_exists_resolves_against_workflow_root_not_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("panel.bed"), b"chr1\t1\t100").unwrap();
        let wf_path = dir.path().join("wf.oxoflow");
        std::fs::write(
            &wf_path,
            r#"
[workflow]
name = "t"
version = "1.0"

[[rules]]
name = "gen"
input = []
output = ["out.txt"]
when = 'file_exists("panel.bed")'
shell = "echo hi > {output[0]}"
"#,
        )
        .unwrap();

        // Load via from_file (like run does): base_dir = workflow parent.
        let config = WorkflowConfig::from_file(&wf_path).unwrap();
        let rule = &config.rules[0];
        let wildcard_values = HashMap::new();

        // Gate is true regardless of process cwd (this test process's cwd is
        // the repo root, which has no panel.bed).
        assert!(
            !when_condition_false(rule, &config, &wildcard_values),
            "gate must resolve panel.bed against the workflow root, not cwd"
        );

        // And from a genuinely different cwd, same verdict.
        let elsewhere = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(elsewhere.path()).unwrap();
        let verdict = when_condition_false(rule, &config, &wildcard_values);
        std::env::set_current_dir(prev).unwrap();
        assert!(!verdict, "changing the process cwd must not flip the gate");
    }

    /// Fixture: two samples, trim → align per sample, one queue-level combine.
    /// All input/output files exist so snapshots resolve.
    fn fixture(
        dir: &std::path::Path,
    ) -> (
        WorkflowConfig,
        WorkflowDag,
        Vec<String>,
        HashMap<String, String>,
    ) {
        let toml = r#"
[workflow]
name = "t"
version = "1.0"

[config]
ref = "ref.fa"

[[sample_groups]]
name = "cohort"
samples = ["S1", "S2"]

[[rules]]
name = "trim"
input = ["raw/{sample}.fq"]
output = ["trimmed/{sample}.fq"]
shell = "cp {input[0]} {output[0]}"

[[rules]]
name = "align"
input = ["trimmed/{sample}.fq"]
output = ["aligned/{sample}.bam"]
depends_on = ["trim"]
shell = "cp {input[0]} {output[0]}"

[[rules]]
name = "combine"
input = ["aligned/*.bam"]
output = ["combined.bam"]
depends_on = ["align"]
shell = "cat {input[0]} > {output[0]} && echo {config.ref}"
"#;
        // Files the rules read and write.
        for f in [
            "raw/S1.fq",
            "raw/S2.fq",
            "trimmed/S1.fq",
            "trimmed/S2.fq",
            "aligned/S1.bam",
            "aligned/S2.bam",
            "combined.bam",
        ] {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "data1").unwrap();
        }

        let mut config = WorkflowConfig::parse(toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let mut wildcard_values = HashMap::new();
        for (key, value) in &config.config {
            wildcard_values.insert(
                format!("config.{key}"),
                value.as_str().unwrap_or_default().to_string(),
            );
        }
        (config, dag, order, wildcard_values)
    }

    fn completed_checkpoint(
        config: &WorkflowConfig,
        order: &[String],
        dir: &std::path::Path,
        wildcard_values: &HashMap<String, String>,
    ) -> CheckpointState {
        let mut ck = CheckpointState::new();
        for name in order {
            ck.completed_rules.insert(name.clone());
            if let Some(rule) = config.get_rule(name)
                && let Ok(Some(manifest)) =
                    snapshot_input_manifest(rule, dir, wildcard_values, &storage_resolver())
            {
                ck.record_input_manifest(name, manifest);
            }
        }
        ck
    }

    fn sensitive() -> HashSet<String> {
        HashSet::new()
    }

    fn run_preview(
        ck: &CheckpointState,
        config: &WorkflowConfig,
        dag: &WorkflowDag,
        order: &[String],
        dir: &std::path::Path,
        wildcard_values: &HashMap<String, String>,
    ) -> RunPreview {
        preview_run_plan(
            ck,
            config,
            dag,
            order,
            dir,
            wildcard_values,
            &sensitive(),
            &config.workflow.interpreter_map,
            &dir.join(".oxo-flow/checkpoint.json"),
            false,
            false,
        )
    }

    fn status_of<'a>(preview: &'a RunPreview, name: &str) -> &'a RuleStatus {
        &preview
            .plan
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} missing from plan"))
            .status
    }

    fn fixture_with_when(
        threshold: &str,
    ) -> (
        WorkflowConfig,
        WorkflowDag,
        Vec<String>,
        HashMap<String, String>,
    ) {
        let toml = format!(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
threshold = {threshold}

[[rules]]
name = "fast"
input = ["in.txt"]
output = ["out.txt"]
when = "config.threshold >= 10"
shell = "cp in.txt out.txt"
"#
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.apply_defaults();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let wildcard_values = config
            .config
            .iter()
            .map(|(k, v)| {
                (
                    format!("config.{k}"),
                    v.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        (config, dag, order, wildcard_values)
    }

    #[test]
    fn missing_input_cascades_up_to_completed_producer() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // Delete the trim output: align's manifest snapshot now errors.
        std::fs::remove_file(dir.path().join("trimmed/S1.fq")).unwrap();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(
            status_of(&preview, "trim_cohort_S1"),
            &RuleStatus::CascadedUpstream {
                from: "align_cohort_S1".to_string()
            },
            "the completed producer re-executes first"
        );
        assert_eq!(
            status_of(&preview, "align_cohort_S1"),
            &RuleStatus::InputInvalidated
        );
    }

    #[test]
    fn tombstoned_rule_stays_skipped_while_dependents_complete() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let mut ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);
        // Simulate a past tombstone: trim_S1's outputs deleted, recorded.
        ck.tombstones.insert(
            "trim_cohort_S1".to_string(),
            vec!["trimmed/S1.fq".to_string()],
        );
        std::fs::remove_file(dir.path().join("trimmed/S1.fq")).unwrap();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(
            status_of(&preview, "trim_cohort_S1"),
            &RuleStatus::Skipped,
            "nothing downstream needs the output — stay skipped"
        );
        assert_eq!(status_of(&preview, "align_cohort_S1"), &RuleStatus::Skipped);
    }

    #[test]
    fn tombstoned_producer_regenerates_when_dependent_will_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let mut ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);
        ck.tombstones.insert(
            "trim_cohort_S1".to_string(),
            vec!["trimmed/S1.fq".to_string()],
        );
        std::fs::remove_file(dir.path().join("trimmed/S1.fq")).unwrap();
        // align_S1's OWN outputs vanished → it will re-run and needs the
        // tombstoned producer's outputs again.
        std::fs::remove_file(dir.path().join("aligned/S1.bam")).unwrap();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(
            status_of(&preview, "trim_cohort_S1"),
            &RuleStatus::CascadedUpstream {
                from: "align_cohort_S1".to_string()
            },
            "the tombstoned producer regenerates before its dependent"
        );
        assert_eq!(
            status_of(&preview, "align_cohort_S1"),
            &RuleStatus::OutputsMissing
        );
    }

    #[test]
    fn when_false_dominates_every_other_consideration() {
        let (config, dag, order, wildcard_values) = fixture_with_when("5");
        let ck = CheckpointState::new();
        let preview = run_preview(
            &ck,
            &config,
            &dag,
            &order,
            std::path::Path::new("."),
            &wildcard_values,
        );
        assert_eq!(
            status_of(&preview, "fast"),
            &RuleStatus::SkippedByWhen,
            "a false when condition skips the rule even though it never completed"
        );
        assert_eq!(preview.will_skip, 1);
    }

    #[test]
    fn when_true_classifies_normally() {
        let (config, dag, order, wildcard_values) = fixture_with_when("10");
        let ck = CheckpointState::new();
        let preview = run_preview(
            &ck,
            &config,
            &dag,
            &order,
            std::path::Path::new("."),
            &wildcard_values,
        );
        assert_eq!(status_of(&preview, "fast"), &RuleStatus::NeverCompleted);
        assert_eq!(preview.will_skip, 0);
    }

    #[test]
    fn merge_profile_fills_missing_keys_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
        std::fs::write(
            dir.path().join("profiles/batch.toml"),
            r#"[config]
threshold = 20
mode = "fast"
"#,
        )
        .unwrap();
        let mut config = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
threshold = 5
"#,
        )
        .unwrap();
        merge_profile(&mut config, "batch", dir.path()).unwrap();
        assert_eq!(
            config.config["threshold"].as_integer(),
            Some(5),
            "existing keys are never overwritten"
        );
        assert_eq!(
            config.config["mode"].as_str(),
            Some("fast"),
            "missing keys are filled in"
        );
    }

    #[test]
    fn merge_profile_missing_profile_is_hard_error_with_available_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
        std::fs::write(
            dir.path().join("profiles/batch.toml"),
            "[config]\nthreads = 4\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("profiles/dev.oxoflow"), "[config]\n").unwrap();
        let mut config = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"
version = "1.0"
"#,
        )
        .unwrap();
        let err = merge_profile(&mut config, "nope", dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("nope"), "error should name the profile: {msg}");
        assert!(
            msg.contains("batch") && msg.contains("dev"),
            "error should list available profiles: {msg}"
        );
        // No profile dir at all still errors (with a different hint).
        let bare = tempfile::tempdir().unwrap();
        let mut config2 = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"
version = "1.0"
"#,
        )
        .unwrap();
        let err2 = merge_profile(&mut config2, "nope", bare.path()).unwrap_err();
        assert!(format!("{err2:#}").contains("no .toml/.oxoflow profile files"));
    }

    #[test]
    fn empty_checkpoint_with_fresh_outputs_skips_everything() {
        // The fixture pre-creates every declared output. With no checkpoint,
        // `run` skips through the executor freshness gate — the preview
        // mirrors that (SkippedFresh). The one exception mirrors the
        // executor exactly: `combine` reads a GLOB input, and
        // `file_is_newer` cannot stat a glob pattern, so the executor does
        // NOT skip it either — it stays NeverCompleted.
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        // Deterministic freshness: the fixture writes inputs and outputs
        // back to back, and on filesystems with coarse timestamp
        // granularity the strict `>` comparison in `file_is_newer` flips
        // (CI flake: trim_cohort_S1 stayed NeverCompleted). Age the inputs
        // so the pre-created outputs are unambiguously newer.
        let older = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        for f in ["raw/S1.fq", "raw/S2.fq"] {
            let p = dir.path().join(f);
            let file = std::fs::OpenOptions::new().write(true).open(p).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(older))
                .unwrap();
        }
        let ck = CheckpointState::new();
        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(preview.plan.len(), 5);
        for rule in &preview.plan {
            let expected = if rule.name == "combine" {
                RuleStatus::NeverCompleted
            } else {
                RuleStatus::SkippedFresh
            };
            assert_eq!(rule.status, expected, "wrong status for {}", rule.name);
        }
        assert_eq!(preview.will_skip, 4);
        assert_eq!(preview.protected_outside, 0);
        assert!(preview.cascade_chains.is_empty());
    }

    #[test]
    fn empty_checkpoint_without_outputs_runs_everything() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        // Remove every declared output: nothing is fresh any more.
        for rule in &config.rules {
            for output in rule.output.iter() {
                let p = dir.path().join(output);
                let _ = std::fs::remove_file(&p);
            }
        }
        let ck = CheckpointState::new();
        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(preview.plan.len(), 5);
        assert!(
            preview
                .plan
                .iter()
                .all(|r| r.status == RuleStatus::NeverCompleted)
        );
        assert_eq!(preview.will_skip, 0);
    }

    #[test]
    fn up_to_date_workflow_skips_everything() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);
        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert!(preview.plan.iter().all(|r| r.status == RuleStatus::Skipped));
        assert_eq!(preview.will_skip, 5);
        assert_eq!(preview.completed_total, 5);
        assert!(preview.cascade_chains.is_empty());
    }

    #[test]
    fn changed_input_invalidates_rule_and_cascades_downstream() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // S1's raw data is re-downloaded: different size + mtime.
        std::fs::write(dir.path().join("raw/S1.fq"), "completely new content").unwrap();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(
            status_of(&preview, "trim_cohort_S1"),
            &RuleStatus::InputInvalidated
        );
        // The engine rewrites `depends_on = ["trim"]` to EVERY expanded trim
        // (both samples) — align rules for S2 also sit downstream of
        // trim_cohort_S1. The preview surfaces this conservative coupling:
        // one sample's data change infects the whole queue.
        assert_eq!(
            status_of(&preview, "align_cohort_S1"),
            &RuleStatus::Cascaded {
                from: "trim_cohort_S1".to_string()
            }
        );
        assert_eq!(
            status_of(&preview, "align_cohort_S2"),
            &RuleStatus::Cascaded {
                from: "trim_cohort_S1".to_string()
            }
        );
        assert_eq!(
            status_of(&preview, "combine"),
            &RuleStatus::Cascaded {
                from: "trim_cohort_S1".to_string()
            }
        );
        // Only S2's trim itself stays protected.
        assert_eq!(status_of(&preview, "trim_cohort_S2"), &RuleStatus::Skipped);
        assert_eq!(preview.will_skip, 1);
        // The infection chain is surfaced for display.
        assert!(preview.cascade_chains.iter().any(|c| c
            == &vec![
                "trim_cohort_S1".to_string(),
                "align_cohort_S1".to_string(),
                "align_cohort_S2".to_string(),
                "combine".to_string()
            ]));
    }

    #[test]
    fn missing_output_marks_rerun_but_leaves_manifest_intact() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // The queue output was deleted; its inputs are untouched.
        std::fs::remove_file(dir.path().join("combined.bam")).unwrap();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(status_of(&preview, "combine"), &RuleStatus::OutputsMissing);
        assert_eq!(status_of(&preview, "trim_cohort_S1"), &RuleStatus::Skipped);
    }

    #[test]
    fn target_subset_counts_protected_outside() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // Target only S1's trim: 1 in the set, 4 completed outside it.
        let targets = ["trim_cohort_S1".to_string()];
        let subset: Vec<String> = dag
            .execution_order_for_targets(&targets.iter().map(String::as_str).collect::<Vec<_>>())
            .unwrap();
        let preview = run_preview(&ck, &config, &dag, &subset, dir.path(), &wildcard_values);
        assert_eq!(preview.plan.len(), 1);
        assert_eq!(preview.protected_outside, 4);
        assert_eq!(preview.will_skip, 1);
    }

    #[test]
    fn config_change_invalidates_referencing_rules() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let mut ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);

        // Bootstrap a config snapshot (legacy path records the baseline),
        // then change a key the queue rule references in its shell.
        oxo_flow_core::config_impact::detect_config_changes(
            &mut ck,
            &config.rules,
            &dag,
            &config.config,
            &sensitive(),
            &config.workflow.interpreter_map,
            config.defaults.shell_prelude.as_deref(),
            config.base_dir(),
        );

        let mut changed_config = config.clone();
        changed_config
            .config
            .insert("ref".to_string(), toml::Value::String("other.fa".into()));

        let mut changed_wildcards = wildcard_values.clone();
        changed_wildcards.insert("config.ref".to_string(), "other.fa".into());

        let preview = run_preview(
            &ck,
            &changed_config,
            &dag,
            &order,
            dir.path(),
            &changed_wildcards,
        );
        assert_eq!(
            status_of(&preview, "combine"),
            &RuleStatus::ConfigInvalidated
        );
        assert_eq!(status_of(&preview, "trim_cohort_S1"), &RuleStatus::Skipped);
    }

    #[test]
    fn legacy_checkpoint_adopts_baseline_without_invalidating() {
        let dir = tempfile::tempdir().unwrap();
        let (config, dag, order, wildcard_values) = fixture(dir.path());
        let mut ck = completed_checkpoint(&config, &order, dir.path(), &wildcard_values);
        // Legacy: completed but no manifests, no snapshots.
        ck.input_manifests.clear();
        ck.config_snapshot.clear();
        ck.rule_fingerprints.clear();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert!(preview.plan.iter().all(|r| r.status == RuleStatus::Skipped));
    }

    // ─── Issue #142 H3a: undeclared wildcard parity ────────────────────

    /// A rule whose output contains an undeclared `{sample}` is NEVER fresh:
    /// the brace path cannot exist as itself (the executor runs the rule and
    /// writes the literal name), so the preview must say `NeverCompleted` —
    /// never `SkippedFresh` — matching `should_skip_rule` exactly.
    #[test]
    fn undeclared_wildcard_output_is_never_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"
version = "1.0"

[[rules]]
name = "gen"
output = ["out_{sample}.txt"]
shell = "echo hi > {output}"
"#,
        )
        .unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let wildcard_values = HashMap::new();
        let ck = CheckpointState::new();

        // Pre-create the LITERAL brace-named file — even then the rule must
        // not classify as fresh, exactly like the executor treats it.
        std::fs::write(dir.path().join("out_{sample}.txt"), "hi").unwrap();

        let preview = run_preview(&ck, &config, &dag, &order, dir.path(), &wildcard_values);
        assert_eq!(
            status_of(&preview, "gen"),
            &RuleStatus::NeverCompleted,
            "an undeclared-wildcard output must never count as up-to-date"
        );
        assert!(
            !preview
                .plan
                .iter()
                .any(|r| r.status == RuleStatus::SkippedFresh)
        );
    }

    // ─── Issue #142 M1: sample-selection manifest exclusion ────────────

    fn manifest_entry(
        path: &str,
        size: u64,
    ) -> oxo_flow_core::executor::checkpoint::InputManifestEntry {
        oxo_flow_core::executor::checkpoint::InputManifestEntry {
            path: path.to_string(),
            size,
            mtime_nanos: 1000,
            hash: Some("sha256:abc".to_string()),
            remote: None,
        }
    }

    fn gather_config(samples_list: &str) -> WorkflowConfig {
        let toml = format!(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
samples_list = "{samples_list}"

[[rules]]
name = "gather"
output = ["counts.txt"]
expand_inputs = [{{ pattern = "per/{{sample}}.txt", variables = {{ sample = "config.samples_list" }} }}]
shell = "cat {{input}} > {{output}}"
"#
        );
        let mut config = WorkflowConfig::parse(&toml).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        config
    }

    #[test]
    fn engine_expanded_inputs_from_injected_lists_injected_rules() {
        let config = gather_config("S1,S2,S3");
        let gather = config.get_rule("gather").unwrap();
        let expanded =
            engine_expanded_inputs_from_injected(gather, &config).expect("injected expansion");
        assert_eq!(expanded, vec!["per/S1.txt", "per/S2.txt", "per/S3.txt"]);
    }

    #[test]
    fn engine_expanded_inputs_from_injected_is_none_without_injected_keys() {
        let mut config = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
ref = "ref.fa"

[[rules]]
name = "gather"
output = ["counts.txt"]
expand_inputs = [{ pattern = "raw/{sample}.txt", variables = { sample = "config.ref" } }]
shell = "cat {input} > {output}"
"#,
        )
        .unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let gather = config.get_rule("gather").unwrap();
        assert!(
            engine_expanded_inputs_from_injected(gather, &config).is_none(),
            "a config.key that is NOT engine-injected must not qualify"
        );
        // No expand_inputs at all — also none.
        let mut plain = WorkflowConfig::parse(
            r#"
[workflow]
name = "t"
version = "1.0"

[[rules]]
name = "gen"
output = ["out.txt"]
shell = "echo hi > {output}"
"#,
        )
        .unwrap();
        plain.apply_defaults();
        plain.expand_wildcards().unwrap();
        assert!(
            engine_expanded_inputs_from_injected(plain.get_rule("gen").unwrap(), &plain).is_none()
        );
    }

    /// The true case: the recorded full-cohort manifest vs the subset
    /// manifest, under the subset config — identical overlap, set
    /// reproduced by re-expansion → sample-selection-driven.
    #[test]
    fn manifest_mismatch_is_sample_selection_on_pure_subset_change() {
        let config = gather_config("S2");
        let gather = config.get_rule("gather").unwrap();
        let recorded = vec![
            manifest_entry("per/S1.txt", 10),
            manifest_entry("per/S2.txt", 20),
            manifest_entry("per/S3.txt", 30),
        ];
        let current = vec![manifest_entry("per/S2.txt", 20)];
        assert!(
            manifest_mismatch_is_sample_selection(gather, &recorded, &current, &config),
            "a pure samples_list subset change must classify as selection-driven"
        );
    }

    /// A content edit to a file the rule consumes still invalidates: the
    /// overlapping entry differs (condition 3).
    #[test]
    fn manifest_mismatch_with_edited_file_is_not_sample_selection() {
        let config = gather_config("S2");
        let gather = config.get_rule("gather").unwrap();
        let recorded = vec![
            manifest_entry("per/S1.txt", 10),
            manifest_entry("per/S2.txt", 20),
            manifest_entry("per/S3.txt", 30),
        ];
        let mut current = vec![manifest_entry("per/S2.txt", 20)];
        current[0].size = 99; // the file the rule consumes was edited
        assert!(
            !manifest_mismatch_is_sample_selection(gather, &recorded, &current, &config),
            "an edited consumed file must invalidate, never exempt"
        );
    }

    /// A file set the injected value does not reproduce is not exempt: a
    /// user-added input file means a real (non-selection) change.
    #[test]
    fn manifest_mismatch_with_foreign_file_is_not_sample_selection() {
        let config = gather_config("S2");
        let gather = config.get_rule("gather").unwrap();
        let recorded = vec![
            manifest_entry("per/S1.txt", 10),
            manifest_entry("per/S2.txt", 20),
            manifest_entry("per/S3.txt", 30),
        ];
        let current = vec![
            manifest_entry("per/S2.txt", 20),
            manifest_entry("per/S4.txt", 40), // S4 not in samples_list
        ];
        assert!(
            !manifest_mismatch_is_sample_selection(gather, &recorded, &current, &config),
            "a set the injected value cannot produce must invalidate"
        );
    }
}
