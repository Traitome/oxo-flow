//! Normalized resource and cost accounting ledger (issue #1371).
//!
//! One machine-readable document spans the local, HPC, and cloud execution
//! paths: per-rule measurements plus run totals, each carrying the source
//! that produced it. The document is a *view* over a checkpoint — it never
//! invents a measurement. A metric the executing backend did not report is
//! `null` (omitted in JSON, an empty CSV cell), never `0`, and the rule's
//! [`ResourceSource`] says who reported what, including when the answer is
//! "nobody did".
//!
//! # Schema (`oxo-flow.resource-ledger/v1`)
//!
//! ```json
//! {
//!   "schema": "oxo-flow.resource-ledger/v1",
//!   "run": {
//!     "workflow": "pipeline.oxoflow",
//!     "started_at": "2026-11-25T10:00:00Z",
//!     "finished_at": "2026-11-25T10:12:00Z",
//!     "totals": { "rules_total": 2, "rules_executed": 2, "...": "..." },
//!     "sources": [ { "backend": "local", "tool": "sysinfo" } ],
//!     "conflicts": []
//!   },
//!   "rules": [
//!     {
//!       "rule": "align",
//!       "status": "completed",
//!       "wall_time_secs": 720.5,
//!       "cpu_seconds": 5700.0,
//!       "max_rss_bytes": 1073741824,
//!       "source": { "backend": "slurm", "tool": "sacct", "raw_fields": { "MaxRSS": "1024M" } }
//!     }
//!   ]
//! }
//! ```
//!
//! # Semantics
//!
//! - **Additive vs peak.** `wall_time_secs`, `cpu_seconds`, `io_read_bytes`,
//!   `io_write_bytes`, and `scheduler_wait_secs` sum across rules. Memory is a
//!   **peak**: `max_rss_bytes` is the maximum observed resident set size, not a
//!   sum, and a run-level `max_rss_bytes` is the max over rules.
//! - **Unknown is never zero.** Every metric is optional. An absent field
//!   means the source did not report it; a recorded `0` would claim a
//!   measurement nobody made.
//! - **Completeness.** Totals are sums over the rules that reported a metric.
//!   `*_complete` says whether *every* executed rule reported it, so a partial
//!   sum is never presented as a full one.
//! - **Sources.** `source.backend` names the execution backend (`local`,
//!   `slurm`, `pbs`, `sge`, `lsf`, `kubernetes`, `aws-batch`, `gcp-batch`,
//!   `tencent-batch`); `source.tool` names the program that answered
//!   (`sysinfo`, `sacct`, `qstat -x`, `qacct`, `bacct`, `scontrol`, or
//!   `none` when no accounting record existed). `source.raw_fields` preserves
//!   the scheduler's own raw values key-ordered, so a consumer can audit the
//!   conversion. Backend-specific numbers never appear as ad-hoc top-level
//!   fields.
//! - **Cost.** `cost` and `currency` are emitted only when a source reports a
//!   price. No supported backend does today, so they are absent — the schema
//!   reserves the shape, not a fabricated amount.
//! - **Reconciliation.** When a caller supplies raw scheduler totals from an
//!   independent export (the CLI's `--acct` CSV), they are compared against
//!   the per-rule sums. A disagreement beyond the stated tolerance is listed
//!   under `run.conflicts` with both numbers and both sources; the ledger
//!   reports the disagreement rather than picking a winner.
//!
//! # Determinism
//!
//! Rules are sorted by name, sources by `(backend, tool, version, note,
//! raw_fields)`, and raw-field maps are [`BTreeMap`]s, so identical checkpoint
//! state renders byte-identical JSON/CSV. No timestamp is minted at render
//! time: `started_at`/`finished_at` come from the recorded rule runs.

use crate::config::CostBudget;
use crate::cost_budget::CostAccrual;
use crate::error::{OxoFlowError, Result};
use crate::executor::checkpoint::CheckpointState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Versioned schema identifier of the ledger document.
pub const RESOURCE_LEDGER_SCHEMA: &str = "oxo-flow.resource-ledger/v1";

/// Relative tolerance for reconciliation: a metric may drift by 5% of the
/// larger of the two totals before it is reported as a conflict.
pub const RECONCILE_RELATIVE_TOLERANCE: f64 = 0.05;

/// Absolute tolerance (seconds) for time-valued reconciliation — small runs
/// must not flag sub-second rounding as a conflict.
pub const RECONCILE_ABS_TOLERANCE_SECS: f64 = 1.0;

/// Absolute tolerance (bytes) for memory reconciliation: scheduler exports
/// round RSS to MB, so a sub-MiB difference is expected, not a conflict.
pub const RECONCILE_ABS_TOLERANCE_BYTES: f64 = 1024.0 * 1024.0;

mod render;
pub use render::CSV_HEADER;

/// Where a set of resource numbers came from.
///
/// Serialized inside every rule record and every run-level source entry.
/// `raw_fields` keeps the store's own spelling/value pairs (key-ordered);
/// `note` explains an absence the backend cannot fill (for example LSF's
/// state-only `bacct` output) so a `null` metric is never mistaken for a
/// measured zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResourceSource {
    /// Execution backend that produced the numbers (or `unknown` for legacy
    /// checkpoints written before source tracking).
    pub backend: String,
    /// Program that reported them: `sysinfo`, `sacct`, `qstat -x`, `qacct`,
    /// `bacct`, `scontrol`, `sacct-csv`, or `none` when no record existed.
    pub tool: String,
    /// Tool version, when the collector can prove one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Human-readable qualification of an absent or partial record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Raw field name → raw value exactly as the source reported it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raw_fields: BTreeMap<String, String>,
}

impl ResourceSource {
    /// A source for data whose provenance was not recorded (legacy
    /// checkpoints) or does not exist (a rule that never ran).
    pub fn unknown(note: impl Into<String>) -> Self {
        Self {
            backend: "unknown".to_string(),
            tool: "unknown".to_string(),
            version: None,
            note: Some(note.into()),
            raw_fields: BTreeMap::new(),
        }
    }
}

/// Per-rule measurements that are not already carried by
/// [`crate::executor::checkpoint::BenchmarkRecord`] (which holds wall time,
/// peak memory in MB, and CPU seconds).
///
/// Persisted in the checkpoint's `resource_usage` map so the ledger can
/// attribute every number to a source even when the rule failed (a failed
/// rule has no benchmark record, but its scheduler/process measurements are
/// still real).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// Peak resident set size in **bytes** — a peak, never summed.
    /// Recorded exactly by the local sampler; converted from the scheduler's
    /// MB figure on the cluster path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rss_bytes: Option<u64>,
    /// Total CPU seconds across the rule's threads/cores when the source
    /// measured it. Duplicated from the success-path benchmark so a rule
    /// whose attempt FAILED or was cancelled still contributes the CPU it
    /// actually burned (it has no benchmark record to carry it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<f64>,
    /// Bytes read from disk across the rule's processes, when the source
    /// reports them (Linux `/proc/<pid>/io`, Slurm `AveDiskRead`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_read_bytes: Option<u64>,
    /// Bytes written to disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_write_bytes: Option<u64>,
    /// Time between scheduler submission and execution start (Slurm
    /// `Submit`→`Start`). `None` where the store does not report both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler_wait_secs: Option<u64>,
    /// Allocated nodes (Slurm `NNodes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<u32>,
    /// Allocated CPUs/cores (Slurm `AllocCPUS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cores: Option<u32>,
    /// Provenance for the values above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResourceSource>,
}

impl ResourceUsage {
    /// A usage bundle that records only the source — used when the source
    /// proves the backend identity but reported no measurements (cloud
    /// backends without an accounting API, an interrupted job).
    pub fn source_only(source: ResourceSource) -> Self {
        Self {
            source: Some(source),
            ..Self::default()
        }
    }
}

/// A known cost figure. Only emitted when a source reports one; no supported
/// backend does, so this is part of the schema contract rather than a
/// populated field today.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// Amount in `currency`.
    pub amount: f64,
    /// ISO 4217 code or the vendor's currency label.
    pub currency: String,
    /// How the amount was derived (e.g. `"vendor-billing-api"`).
    pub method: String,
    /// Where the amount and method came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResourceSource>,
}

/// One rule's normalized resource record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerRule {
    /// Rule (instance) name, matching the checkpoint key.
    pub rule: String,
    /// `completed`, `failed`, `cancelled`, `timed_out`, `skipped`, or
    /// `unknown` (an interrupted attempt without a terminal verdict).
    pub status: String,
    /// Wall-clock seconds spent executing. Synthetically re-completed rules
    /// (`recorded_as` set) carry no measurement here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_secs: Option<f64>,
    /// Total CPU seconds across all threads/cores the source accounts for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<f64>,
    /// User-mode CPU seconds. No supported source separates user/system CPU
    /// today, so this is absent — unknown, not zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_seconds: Option<f64>,
    /// System-mode CPU seconds. Absent for the same reason as `user_seconds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_seconds: Option<f64>,
    /// Peak resident set size in bytes (a peak, not a sum).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rss_bytes: Option<u64>,
    /// Peak memory reported by a control plane's own metric (not RSS). No
    /// supported backend reports one today, so this is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_memory_bytes: Option<u64>,
    /// Bytes read from disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_read_bytes: Option<u64>,
    /// Bytes written to disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_write_bytes: Option<u64>,
    /// GPU-seconds. No supported source reports GPU utilization today, so
    /// this is absent — unknown, not zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_seconds: Option<f64>,
    /// Scheduler queue wait (submit→start) in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler_wait_secs: Option<u64>,
    /// Allocated nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<u32>,
    /// Allocated CPUs/cores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cores: Option<u32>,
    /// Cost, only when a source reports one (see [`Cost`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// Provenance for this rule's numbers. Always present: when nothing was
    /// recorded, an `unknown` source with a note explains why.
    pub source: ResourceSource,
    /// Why this entry carries no real measurements (e.g. `"outputs
    /// up-to-date"`), copied from the checkpoint benchmark. Absent for
    /// measured rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_as: Option<String>,
}

/// Run-level totals. Additive metrics are sums over the executed rules that
/// reported them; `max_rss_bytes` is the maximum across those rules.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LedgerTotals {
    /// Rules in the ledger (executed or not).
    pub rules_total: usize,
    /// Rules that actually executed in this run (completed, failed,
    /// cancelled, or timed out). Skipped, unknown, and synthetic
    /// "outputs up-to-date" completions contribute nothing.
    pub rules_executed: usize,
    /// Sum of per-rule wall time, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_secs: Option<f64>,
    /// Every executed rule reported wall time.
    #[serde(default)]
    pub wall_time_secs_complete: bool,
    /// Sum of per-rule CPU seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<f64>,
    /// Every executed rule reported CPU seconds.
    #[serde(default)]
    pub cpu_seconds_complete: bool,
    /// Peak RSS across rules, in bytes (max, not sum).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rss_bytes: Option<u64>,
    /// Every executed rule reported a peak RSS.
    #[serde(default)]
    pub max_rss_bytes_complete: bool,
    /// Sum of per-rule bytes read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_read_bytes: Option<u64>,
    /// Every executed rule reported bytes read.
    #[serde(default)]
    pub io_read_bytes_complete: bool,
    /// Sum of per-rule bytes written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_write_bytes: Option<u64>,
    /// Every executed rule reported bytes written.
    #[serde(default)]
    pub io_write_bytes_complete: bool,
    /// Sum of per-rule scheduler wait, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler_wait_secs: Option<u64>,
    /// Every executed rule reported scheduler wait.
    #[serde(default)]
    pub scheduler_wait_secs_complete: bool,
    /// Amount, only when a source reports a price for the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
}

/// Raw scheduler totals from an independent export, plus the rules it covers.
///
/// `rules` is the coverage set used to decide whether a comparison is
/// meaningful: a raw export that does not cover every executed rule is not a
/// run total and produces no conflicts (it would otherwise flag a coverage
/// difference as a numeric disagreement).
#[derive(Debug, Clone, PartialEq)]
pub struct RawSchedulerTotals {
    /// Totals computed from the raw export, in the same units as the ledger.
    pub totals: LedgerTotals,
    /// Where the export came from (e.g. the `--acct` CSV).
    pub source: ResourceSource,
    /// Rule names the export covers.
    pub rules: Vec<String>,
}

/// One reconciliation disagreement between the per-rule ledger sums and a
/// raw scheduler export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerConflict {
    /// Metric name, identical to the rule-level field name.
    pub metric: String,
    /// `seconds` or `bytes`.
    pub unit: String,
    /// Sum/peak from the checkpoint's per-rule records.
    pub per_rule_total: f64,
    /// Sum/peak from the raw scheduler export.
    pub scheduler_total: f64,
    /// The tolerance that was exceeded (`max(absolute, 5% of the larger)`).
    pub tolerance: f64,
    /// Sources that produced the per-rule numbers.
    pub per_rule_sources: Vec<ResourceSource>,
    /// Source of the raw export.
    pub scheduler_source: ResourceSource,
    /// Why this is reported instead of resolved.
    pub note: String,
}

/// Outcome of comparing the per-rule totals against a raw scheduler export.
///
/// Present whenever a raw export was supplied, so "compared and agreed" is
/// distinguishable from "never compared" without cross-checking counts
/// (review M3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerReconciliation {
    /// Executed rules present in both the ledger and the raw export, sorted.
    pub covered_rules: Vec<String>,
    /// Metrics whose values were complete on both sides and were compared.
    pub compared_metrics: Vec<String>,
    /// Metrics that could not be compared, each as `"<metric>: <reason>"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_metrics: Vec<String>,
    /// Human-readable outcome.
    pub note: String,
}

/// The run section of the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerRun {
    /// Run identity when the caller has one. CLI runs do not mint one, so it
    /// is normally absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Workflow file the checkpoint recorded (or the caller supplied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// Earliest recorded rule start, RFC 3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// Latest recorded rule finish, RFC 3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// Totals computed from the checkpoint's per-rule records.
    pub totals: LedgerTotals,
    /// Totals computed from an independent raw export, when one was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler_totals: Option<LedgerTotals>,
    /// Source of `scheduler_totals` (backend/tool/version/note/raw fields),
    /// present exactly when `scheduler_totals` is: every number carries its
    /// provenance, including the raw side (review I1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler_source: Option<ResourceSource>,
    /// How the raw-export comparison proceeded: covered rules, compared
    /// metrics, and metrics skipped with their reasons. Present whenever a
    /// raw export was supplied, even when everything agreed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<LedgerReconciliation>,
    /// Run currency, only when a cost source reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Distinct per-rule sources, sorted canonically.
    pub sources: Vec<ResourceSource>,
    /// Reconciliation disagreements (empty when no raw export was given or
    /// the two sides agree within tolerance).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<LedgerConflict>,
}

/// The normalized `oxo-flow.resource-ledger/v1` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceLedger {
    /// Always [`RESOURCE_LEDGER_SCHEMA`].
    pub schema: String,
    /// Run identity, totals, sources, and conflicts.
    pub run: LedgerRun,
    /// Per-rule records, sorted by rule name.
    pub rules: Vec<LedgerRule>,
    /// The workflow's declared `[resource_budget] max_cost`, when present
    /// (issue #1400). Absent for every workflow that declares none, so the
    /// JSON document is byte-identical to a pre-#1400 ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<CostBudget>,
    /// Cost accrual against `budget`, computed from the same per-rule records
    /// (issue #1400). `accrued` is `None` (never `0`) when a configured
    /// rate's input metric is missing; `unknown_rules` names the executed
    /// rules it could not cost. Present only alongside `budget`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_accrual: Option<CostAccrual>,
}

/// Build the ledger from a checkpoint.
///
/// `workflow` and `run_id` are fallbacks used only when the checkpoint did
/// not record them. `raw` supplies independently collected scheduler totals
/// for reconciliation; pass `None` to skip reconciliation entirely (the
/// ledger then has no `scheduler_totals` and no conflicts — it never invents
/// a raw side).
pub fn build_resource_ledger(
    checkpoint: &CheckpointState,
    workflow: Option<String>,
    run_id: Option<String>,
    raw: Option<&RawSchedulerTotals>,
) -> ResourceLedger {
    let rules: Vec<LedgerRule> = rule_names(checkpoint)
        .into_iter()
        .map(|name| build_rule(checkpoint, &name))
        .collect();

    let totals = compute_totals(&rules);
    // Run-level sources describe work performed in this run; a synthetic
    // completion's explanatory `unknown` source is not a data source.
    let sources: Vec<ResourceSource> = rules
        .iter()
        .filter(|r| actually_executed(r))
        .map(|r| r.source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let (started_at, finished_at) = run_bounds(checkpoint, &rules);
    let (conflicts, reconciliation) = raw
        .map(|raw| compute_conflicts(&rules, &totals, raw))
        .unwrap_or_default();

    ResourceLedger {
        schema: RESOURCE_LEDGER_SCHEMA.to_string(),
        run: LedgerRun {
            run_id,
            workflow: checkpoint.workflow_path.clone().or(workflow),
            started_at,
            finished_at,
            totals,
            scheduler_totals: raw.map(|r| r.totals.clone()),
            scheduler_source: raw.map(|r| r.source.clone()),
            reconciliation,
            currency: None,
            sources,
            conflicts,
        },
        rules,
        budget: None,
        cost_accrual: None,
    }
}

impl ResourceLedger {
    /// Attach the workflow's declared cost budget and the cost accrued
    /// against it (issue #1400).
    ///
    /// `None` leaves both fields unset, so a workflow without
    /// `[resource_budget] max_cost` renders the exact pre-#1400 document.
    /// Per-rule accrual reuses the canonical [`crate::cost_budget::accrue_cost`]
    /// over this ledger's own rule records (reuse, never retype), and only
    /// rules that actually executed contribute — unknown is never zero, so an
    /// executed rule missing a rate's input metric leaves `accrued` `None`
    /// and names itself in `unknown_rules`.
    #[must_use]
    pub fn with_cost_budget(mut self, budget: Option<&CostBudget>) -> Self {
        self.budget = budget.cloned();
        self.cost_accrual = budget.map(|b| crate::cost_budget::accrue_cost(&self.rules, b));
        self
    }
}

/// Rule names the ledger must cover: every name any checkpoint record
/// mentions, sorted. `BTreeSet` gives the canonical order in one step.
fn rule_names(checkpoint: &CheckpointState) -> Vec<String> {
    checkpoint
        .benchmarks
        .keys()
        .chain(checkpoint.completed_rules.iter())
        .chain(checkpoint.failed_rules.iter())
        .chain(checkpoint.rule_runs.keys())
        .chain(checkpoint.resource_usage.keys())
        .chain(checkpoint.skipped_attempts.keys())
        .chain(checkpoint.when_verdicts.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Names of rules that actually executed in this checkpoint's latest
/// records — the filter a raw scheduler export must be reduced to before it
/// can be compared with the per-rule ledger (review I2).
///
/// Delegates to `build_rule` so the `--acct` coverage set and the rendered
/// per-rule records can never disagree about what executed (task-7c I-1:
/// they used to diverge when an earlier synthetic completion left a
/// `recorded_as` benchmark behind a failed real attempt).
pub fn executed_rule_names(checkpoint: &CheckpointState) -> BTreeSet<String> {
    rule_names(checkpoint)
        .into_iter()
        .filter(|name| actually_executed(&build_rule(checkpoint, name)))
        .collect()
}

/// Whether a status describes a rule that actually executed.
fn is_executed(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "timed_out")
}

/// Whether this record represents work performed in THIS run. A synthetic
/// "outputs up-to-date" completion is `completed` bookkeeping, not execution,
/// so it does not participate in executed-rule counts or completeness.
fn actually_executed(rule: &LedgerRule) -> bool {
    is_executed(&rule.status) && rule.recorded_as.is_none()
}

/// Public predicate exposing `actually_executed` so other modules (the cost
/// budget in [`crate::cost_budget`], issue #1400) that must count the *same*
/// set of executed rules share the one definition instead of retyping the
/// status/`recorded_as` check (engineering invariant: don't mirror semantics —
/// share them).
#[must_use]
pub fn rule_actually_executed(rule: &LedgerRule) -> bool {
    actually_executed(rule)
}

/// Terminal status for one rule, preferring the recorded run status (it can
/// distinguish a cancellation or a timeout) and falling back to the
/// checkpoint's completion/failure sets for legacy records.
fn rule_status(checkpoint: &CheckpointState, rule: &str) -> &'static str {
    let run_status = checkpoint
        .rule_runs
        .get(rule)
        .and_then(|r| r.status.as_deref());
    // The LATEST attempt is authoritative when it was skipped: a re-run that
    // found outputs up to date records a skip marker while `completed_rules`
    // keeps the rule for reuse. Counting that as "completed in this run"
    // would report the previous run's metrics as this one's (issue #1371
    // review I4). The `rule_runs` check keeps pre-marker checkpoints honest.
    if checkpoint.skipped_attempts.contains_key(rule) || run_status == Some("skipped") {
        return "skipped";
    }
    if checkpoint.completed_rules.contains(rule) {
        return "completed";
    }
    if checkpoint.failed_rules.contains(rule) {
        return match run_status {
            Some("cancelled") => "cancelled",
            Some("timed_out") => "timed_out",
            _ => "failed",
        };
    }
    match run_status {
        Some("success") => "completed",
        Some("failed") => "failed",
        Some("cancelled") => "cancelled",
        Some("skipped") => "skipped",
        Some("timed_out") => "timed_out",
        Some("running") | Some("pending") => "unknown",
        _ => {
            if checkpoint.when_verdicts.get(rule) == Some(&false) {
                "skipped"
            } else if checkpoint.benchmarks.contains_key(rule) {
                "completed"
            } else {
                "unknown"
            }
        }
    }
}

/// Whether a surviving benchmark record describes the latest recorded
/// attempt for this rule (task-7c I-1).
///
/// `record_run` writes a `rule_runs` status for every terminal attempt, so a
/// latest status other than `success` means the benchmark predates this
/// invocation: `mark_failed`/`mark_completed_quiet` keep it for history while
/// a re-execution fails or is skipped. A `rule_runs` entry without a status
/// (or no entry at all) predates status tracking — those legacy checkpoints
/// keep the documented benchmark-based rendering.
fn benchmark_describes_latest_attempt(checkpoint: &CheckpointState, rule: &str) -> bool {
    match checkpoint
        .rule_runs
        .get(rule)
        .and_then(|r| r.status.as_deref())
    {
        Some(status) => status == "success",
        None => true,
    }
}

/// Build one rule's record. Every metric is copied only from a real record;
/// an absent record leaves the field `None`.
fn build_rule(checkpoint: &CheckpointState, rule: &str) -> LedgerRule {
    let status = rule_status(checkpoint, rule);
    let benchmark_is_current = benchmark_describes_latest_attempt(checkpoint, rule);
    // A synthetic marker only describes THIS entry when the benchmark still
    // describes the latest attempt; a later real (failed) attempt supersedes
    // it, and presenting the marker would falsely say the rule did not run.
    let recorded_as = checkpoint
        .benchmarks
        .get(rule)
        .filter(|_| benchmark_is_current)
        .and_then(|b| b.recorded_as.clone());
    let latest_was_skipped = checkpoint.skipped_attempts.contains_key(rule)
        || checkpoint
            .rule_runs
            .get(rule)
            .and_then(|r| r.status.as_deref())
            == Some("skipped");
    // A skipped or synthetic "outputs up-to-date" completion is completion
    // bookkeeping, not a measurement: neither the 0.0 wall time nor any
    // benchmark/resource_usage left by an earlier attempt describes THIS
    // run's cost, so every metric is suppressed (the source says why).
    let stale = recorded_as.is_some() || latest_was_skipped;
    let usage = if stale {
        None
    } else {
        checkpoint.resource_usage.get(rule)
    };
    let benchmark = checkpoint
        .benchmarks
        .get(rule)
        .filter(|b| b.recorded_as.is_none() && !latest_was_skipped && benchmark_is_current);

    let wall_time_secs = match benchmark {
        Some(b) => Some(b.wall_time_secs),
        // A skipped/synthetic attempt has no measurement, and an earlier
        // attempt's start/finish pair describes that attempt, not this run.
        None if stale => None,
        None => run_duration_secs(checkpoint, rule),
    };
    // A failed/cancelled rule has no benchmark, but the source may still
    // have measured its CPU; `resource_usage` carries it (review I5/I6).
    let cpu_seconds = benchmark
        .and_then(|b| b.cpu_seconds)
        .or_else(|| usage.and_then(|u| u.cpu_seconds));
    let max_rss_bytes = usage.and_then(|u| u.max_rss_bytes).or_else(|| {
        // Checkpoints written before exact bytes were recorded carry only
        // the MB-rounded peak; convert it and let the source note explain.
        benchmark
            .and_then(|b| b.max_memory_mb)
            .map(|mb| mb.saturating_mul(1024 * 1024))
    });

    let source = usage.and_then(|u| u.source.clone()).unwrap_or_else(|| {
        match (recorded_as.is_some(), latest_was_skipped, benchmark) {
            (true, _, _) => ResourceSource::unknown(
                "synthetic 'outputs up-to-date' completion: the rule did not execute in this run",
            ),
            (false, true, _) => {
                ResourceSource::unknown(match checkpoint.skipped_attempts.get(rule) {
                    Some(reason) => format!(
                        "not executed in this run ({reason}); measurements from an earlier run \
                     are not this run's"
                    ),
                    None => "latest recorded attempt was skipped (outputs up-to-date or a false \
                         condition); measurements from an earlier run are not this run's"
                        .to_string(),
                })
            }
            (false, false, Some(_)) => ResourceSource::unknown(
                "legacy checkpoint: benchmark recorded without resource source metadata",
            ),
            (false, false, None) => {
                ResourceSource::unknown("no resource record was persisted for this rule")
            }
        }
    });

    LedgerRule {
        rule: rule.to_string(),
        status: status.to_string(),
        wall_time_secs,
        cpu_seconds,
        user_seconds: None,
        system_seconds: None,
        max_rss_bytes,
        peak_memory_bytes: None,
        io_read_bytes: usage.and_then(|u| u.io_read_bytes),
        io_write_bytes: usage.and_then(|u| u.io_write_bytes),
        gpu_seconds: None,
        scheduler_wait_secs: usage.and_then(|u| u.scheduler_wait_secs),
        nodes: usage.and_then(|u| u.nodes),
        cores: usage.and_then(|u| u.cores),
        cost: None,
        source,
        recorded_as,
    }
}

/// Wall time for a rule without a benchmark: the recorded start/finish pair
/// (this is how failed rules get an honest duration). `None` when either end
/// is missing or the pair is inconsistent.
fn run_duration_secs(checkpoint: &CheckpointState, rule: &str) -> Option<f64> {
    let run = checkpoint.rule_runs.get(rule)?;
    let start = run.started_at?;
    let finish = run.finished_at?;
    let secs = (finish - start).num_milliseconds() as f64 / 1000.0;
    (secs >= 0.0).then_some(secs)
}

/// Earliest recorded start and latest recorded finish across the rules that
/// executed in this run, as RFC 3339 UTC strings.
fn run_bounds(
    checkpoint: &CheckpointState,
    rules: &[LedgerRule],
) -> (Option<String>, Option<String>) {
    let executed: BTreeSet<&str> = rules
        .iter()
        .filter(|r| actually_executed(r))
        .map(|r| r.rule.as_str())
        .collect();
    let mut start = None;
    let mut finish = None;
    for (name, run) in &checkpoint.rule_runs {
        if !executed.contains(name.as_str()) {
            continue;
        }
        if let Some(s) = run.started_at
            && start.is_none_or(|current| s < current)
        {
            start = Some(s);
        }
        if let Some(f) = run.finished_at
            && finish.is_none_or(|current| f > current)
        {
            finish = Some(f);
        }
    }
    let render =
        |ts: chrono::DateTime<chrono::Utc>| ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    (start.map(render), finish.map(render))
}

/// Compute run totals from already-built per-rule records.
fn compute_totals(rules: &[LedgerRule]) -> LedgerTotals {
    let executed: Vec<&LedgerRule> = rules.iter().filter(|r| actually_executed(r)).collect();
    // Sorted rule order is already fixed, so the float sums are
    // order-independent run to run.
    let samples: Vec<TotalsSample> = executed
        .iter()
        .map(|rule| TotalsSample {
            wall_time_secs: rule.wall_time_secs,
            cpu_seconds: rule.cpu_seconds,
            max_rss_bytes: rule.max_rss_bytes,
            io_read_bytes: rule.io_read_bytes,
            io_write_bytes: rule.io_write_bytes,
            scheduler_wait_secs: rule.scheduler_wait_secs,
        })
        .collect();
    totals_from_samples(rules.len(), executed.len(), &samples)
}

/// One rule's contribution to a [`LedgerTotals`] accumulation.
///
/// Both the checkpoint-derived totals and the `--acct` raw-export totals go
/// through [`totals_from_samples`], so the additive/peak/completeness rules
/// cannot drift between the two sides of a reconciliation (review M4).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TotalsSample {
    /// Wall-clock seconds.
    pub wall_time_secs: Option<f64>,
    /// CPU seconds.
    pub cpu_seconds: Option<f64>,
    /// Peak RSS in bytes (a peak, not a sum).
    pub max_rss_bytes: Option<u64>,
    /// Bytes read.
    pub io_read_bytes: Option<u64>,
    /// Bytes written.
    pub io_write_bytes: Option<u64>,
    /// Scheduler queue wait in seconds.
    pub scheduler_wait_secs: Option<u64>,
}

/// Accumulate run totals from per-rule samples.
///
/// Additive metrics (`wall`, `cpu`, I/O, wait) sum over the samples that
/// reported them; `max_rss_bytes` is the maximum. A `*_complete` flag is
/// true only when every counted sample reported the metric — and only when
/// at least one sample was counted, so an empty run never claims a
/// measured zero.
pub fn totals_from_samples(
    rules_total: usize,
    rules_executed: usize,
    samples: &[TotalsSample],
) -> LedgerTotals {
    let mut wall = 0.0;
    let mut wall_reported = 0usize;
    let mut cpu = 0.0;
    let mut cpu_reported = 0usize;
    let mut rss: Option<u64> = None;
    let mut rss_reported = 0usize;
    let mut read = 0u64;
    let mut read_reported = 0usize;
    let mut write = 0u64;
    let mut write_reported = 0usize;
    let mut wait = 0u64;
    let mut wait_reported = 0usize;

    for sample in samples {
        if let Some(v) = sample.wall_time_secs {
            wall += v;
            wall_reported += 1;
        }
        if let Some(v) = sample.cpu_seconds {
            cpu += v;
            cpu_reported += 1;
        }
        if let Some(v) = sample.max_rss_bytes {
            rss = Some(rss.map_or(v, |current| current.max(v)));
            rss_reported += 1;
        }
        if let Some(v) = sample.io_read_bytes {
            read = read.saturating_add(v);
            read_reported += 1;
        }
        if let Some(v) = sample.io_write_bytes {
            write = write.saturating_add(v);
            write_reported += 1;
        }
        if let Some(v) = sample.scheduler_wait_secs {
            wait = wait.saturating_add(v);
            wait_reported += 1;
        }
    }

    let counted = samples.len();
    let complete = |reported: usize| counted > 0 && reported == counted;
    LedgerTotals {
        rules_total,
        rules_executed,
        wall_time_secs: (wall_reported > 0).then_some(wall),
        wall_time_secs_complete: complete(wall_reported),
        cpu_seconds: (cpu_reported > 0).then_some(cpu),
        cpu_seconds_complete: complete(cpu_reported),
        max_rss_bytes: rss,
        max_rss_bytes_complete: complete(rss_reported),
        io_read_bytes: (read_reported > 0).then_some(read),
        io_read_bytes_complete: complete(read_reported),
        io_write_bytes: (write_reported > 0).then_some(write),
        io_write_bytes_complete: complete(write_reported),
        scheduler_wait_secs: (wait_reported > 0).then_some(wait),
        scheduler_wait_secs_complete: complete(wait_reported),
        cost: None,
    }
}

/// Reconciliation tolerance for a metric pair: 5% of the larger value, never
/// below the metric's absolute floor.
fn tolerance_for(per_rule: f64, raw: f64, absolute: f64) -> f64 {
    (per_rule.abs().max(raw.abs()) * RECONCILE_RELATIVE_TOLERANCE).max(absolute)
}

/// Compare per-rule totals against a raw export.
///
/// Emits a conflict for every metric that both sides report completely and
/// that differs beyond the stated tolerance, and ALWAYS returns a
/// [`LedgerReconciliation`] describing what was (and was not) compared, so
/// "agreed" is distinguishable from "never compared" (review M3).
fn compute_conflicts(
    rules: &[LedgerRule],
    totals: &LedgerTotals,
    raw: &RawSchedulerTotals,
) -> (Vec<LedgerConflict>, Option<LedgerReconciliation>) {
    let executed: BTreeSet<&str> = rules
        .iter()
        .filter(|r| actually_executed(r))
        .map(|r| r.rule.as_str())
        .collect();
    let raw_rules: BTreeSet<&str> = raw.rules.iter().map(String::as_str).collect();
    let covered_rules: Vec<String> = executed
        .intersection(&raw_rules)
        .map(|name| (*name).to_string())
        .collect();
    // A raw export that does not cover every executed rule is not a run
    // total — comparing it would report a coverage gap as a numeric lie.
    let coverage_ok = executed.is_subset(&raw_rules);

    let per_rule_sources: Vec<ResourceSource> = rules
        .iter()
        .filter(|r| actually_executed(r))
        .map(|r| r.source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut conflicts = Vec::new();
    let mut compared_metrics: Vec<String> = Vec::new();
    let mut skipped_metrics: Vec<String> = Vec::new();
    let mut compare = |metric: &str,
                       unit: &str,
                       per: Option<f64>,
                       per_complete: bool,
                       raw_value: Option<f64>,
                       raw_complete: bool,
                       absolute: f64| {
        if executed.is_empty() {
            skipped_metrics.push(format!("{metric}: no executed rules to compare"));
            return;
        }
        if !coverage_ok {
            skipped_metrics.push(format!(
                "{metric}: the raw export does not cover every executed rule"
            ));
            return;
        }
        let (Some(per), Some(raw_value)) = (per, raw_value) else {
            skipped_metrics.push(format!("{metric}: one side did not report the metric"));
            return;
        };
        if !per_complete || !raw_complete {
            skipped_metrics.push(format!("{metric}: at least one side is incomplete"));
            return;
        }
        compared_metrics.push(metric.to_string());
        let tolerance = tolerance_for(per, raw_value, absolute);
        if (per - raw_value).abs() <= tolerance {
            return;
        }
        conflicts.push(LedgerConflict {
            metric: metric.to_string(),
            unit: unit.to_string(),
            per_rule_total: per,
            scheduler_total: raw_value,
            tolerance,
            per_rule_sources: per_rule_sources.clone(),
            scheduler_source: raw.source.clone(),
            note: "the per-rule ledger sum and the raw scheduler export disagree beyond the \
                   stated tolerance; both values are reported as-is"
                .to_string(),
        });
    };

    compare(
        "wall_time_secs",
        "seconds",
        totals.wall_time_secs,
        totals.wall_time_secs_complete,
        raw.totals.wall_time_secs,
        raw.totals.wall_time_secs_complete,
        RECONCILE_ABS_TOLERANCE_SECS,
    );
    compare(
        "cpu_seconds",
        "seconds",
        totals.cpu_seconds,
        totals.cpu_seconds_complete,
        raw.totals.cpu_seconds,
        raw.totals.cpu_seconds_complete,
        RECONCILE_ABS_TOLERANCE_SECS,
    );
    compare(
        "max_rss_bytes",
        "bytes",
        totals.max_rss_bytes.map(|v| v as f64),
        totals.max_rss_bytes_complete,
        raw.totals.max_rss_bytes.map(|v| v as f64),
        raw.totals.max_rss_bytes_complete,
        RECONCILE_ABS_TOLERANCE_BYTES,
    );

    let note = if executed.is_empty() {
        "not compared: the checkpoint records no executed rules".to_string()
    } else if !coverage_ok {
        format!(
            "not compared: the raw export covers {} of {} executed rule(s)",
            covered_rules.len(),
            executed.len()
        )
    } else if compared_metrics.is_empty() {
        "not compared: no metric was complete on both sides".to_string()
    } else if conflicts.is_empty() {
        format!(
            "compared {} metric(s) across {} rule(s); no disagreement beyond tolerance",
            compared_metrics.len(),
            covered_rules.len()
        )
    } else {
        format!(
            "{} of {} compared metric(s) disagree beyond tolerance",
            conflicts.len(),
            compared_metrics.len()
        )
    };

    (
        conflicts,
        Some(LedgerReconciliation {
            covered_rules,
            compared_metrics,
            skipped_metrics,
            note,
        }),
    )
}

#[cfg(test)]
mod tests;
