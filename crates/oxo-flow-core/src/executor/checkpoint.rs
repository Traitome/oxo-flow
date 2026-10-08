use crate::error::{OxoFlowError, Result};
use crate::executor::JobRecord;
use crate::rule::{FilePatterns, Rule};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter making every staged checkpoint filename unique within
/// this process (combined with the PID, unique on the machine).
static STAGED_SAVE_SEQ: AtomicU64 = AtomicU64::new(0);

/// `checkpoint.json` → `checkpoint.json.<seq>.<pid>.tmp`: a sibling staged
/// name no other concurrent save can collide with.
fn unique_staged_path(path: &Path) -> PathBuf {
    let seq = STAGED_SAVE_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "checkpoint.json".into());
    name.push(format!(".{seq}.{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Shared save body for [`CheckpointState::save_to_file`] and
/// [`save_to_file_async`](CheckpointState::save_to_file_async): create the
/// parent dir, write + fsync the staged file, rename it over the target,
/// fsync the parent dir. The staged path is caller-chosen and unique per
/// call, so concurrent saves never rename or delete each other's file.
fn write_checkpoint_staged(json: &str, path: &Path, tmp_path: &Path) -> Result<()> {
    let parent = crate::parent_dir(path);
    if parent != std::path::Path::new(".") {
        std::fs::create_dir_all(parent).map_err(|e| OxoFlowError::Config {
            message: format!("failed to create checkpoint directory: {e}"),
        })?;
    }
    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(tmp_path)?;
        use std::io::Write;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(tmp_path, path)?;
        // fsync the parent directory so the rename is durable (POSIX).
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    write_result.map_err(|e| {
        // Never leave a partial tmp file behind for the next attempt to
        // trip over; the real checkpoint (old or new) is what matters.
        let _ = std::fs::remove_file(tmp_path);
        OxoFlowError::Config {
            message: format!("failed to save checkpoint to {}: {e}", path.display()),
        }
    })
}

/// One file in an input manifest: part of the file set a rule's inputs
/// resolved to when the rule completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputManifestEntry {
    /// Path relative to the working directory.
    pub path: String,
    /// File size in bytes at snapshot time.
    pub size: u64,
    /// Last-modified time (nanoseconds since the Unix epoch) at snapshot time.
    pub mtime_nanos: i128,
    /// `sha256:<hex>` content hash for files up to
    /// [`MANIFEST_HASH_MAX_BYTES`]. `None` for larger files (size+mtime
    /// policy) and for legacy checkpoints written before hashing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Remote-object identity for `s3://` / `gs://` inputs (issue #78 P2).
    /// `None` for local files and legacy checkpoints — those keep the
    /// size+mtime+sha256 policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteManifestEntry>,
}

/// Content identity of a remote input object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteManifestEntry {
    /// URI scheme: `"s3"` or `"gs"`.
    pub scheme: String,
    /// The full URI as declared in the workflow.
    pub key: String,
    /// Object size in bytes at snapshot time.
    pub size: u64,
    /// Content identity as reported by the store: S3 ETag (raw, possibly a
    /// composite multipart hash) or GCS `md5Hash` (base64). `None` when the
    /// store cannot provide one — matching then degrades to size-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// Files up to this size are content-hashed in input manifests. Hashing
/// multi-gigabyte intermediates (BAM, CRAM) on every run would cost more
/// than the invalidation precision buys — those keep the size+mtime policy.
pub const MANIFEST_HASH_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Sorted, deduplicated snapshot of a rule's resolved input files.
pub type InputManifest = Vec<InputManifestEntry>;

/// Whether a recorded manifest still matches the current file set —
/// the hash-aware version of plain equality (see [`InputManifestEntry`]).
///
/// Entries WITH a recorded hash compare content (mtime is irrelevant —
/// touching a file no longer invalidates); legacy entries without one keep
/// the size+mtime policy instead of invalidating everything once.
pub fn manifests_match(recorded: &[InputManifestEntry], current: &[InputManifestEntry]) -> bool {
    if recorded.len() != current.len() {
        return false;
    }
    recorded
        .iter()
        .zip(current)
        .all(|(r, c)| entry_matches(r, c))
}

/// Whether one manifest entry still describes the current file state.
fn entry_matches(recorded: &InputManifestEntry, current: &InputManifestEntry) -> bool {
    match (&recorded.remote, &current.remote) {
        // Local entries: the existing size+mtime(+sha256) policy.
        (None, None) => {
            recorded.path == current.path
                && recorded.size == current.size
                && match &recorded.hash {
                    Some(rec_hash) => current.hash.as_deref() == Some(rec_hash.as_str()),
                    None => recorded.mtime_nanos == current.mtime_nanos,
                }
        }
        // Remote entries (issue #78 P2): scheme+key+size+etag. When neither
        // side has an etag, size is the only identity left (documented
        // conservative-for-availability fallback).
        (Some(rr), Some(rc)) => {
            rr.scheme == rc.scheme
                && rr.key == rc.key
                && rr.size == rc.size
                && match (&rr.etag, &rc.etag) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                }
        }
        _ => false,
    }
}

/// Human-readable description of what changed between a recorded manifest
/// and the current file set — `"<path> (changed)"`, `"<path> (added)"`,
/// `"<path> (removed)"` — the explanatory side of [`manifests_match`]
/// (issue #194 §2.10: post-run re-verification reports WHICH inputs moved).
pub fn manifest_changes(
    recorded: &[InputManifestEntry],
    current: &[InputManifestEntry],
) -> Vec<String> {
    let mut changes = Vec::new();
    let recorded_by_path: HashMap<&str, &InputManifestEntry> =
        recorded.iter().map(|e| (e.path.as_str(), e)).collect();
    let current_by_path: HashMap<&str, &InputManifestEntry> =
        current.iter().map(|e| (e.path.as_str(), e)).collect();

    for (path, recorded_entry) in &recorded_by_path {
        match current_by_path.get(path) {
            Some(current_entry) if !entry_matches(recorded_entry, current_entry) => {
                changes.push(format!("{path} (changed)"));
            }
            Some(_) => {}
            None => changes.push(format!("{path} (removed)")),
        }
    }
    for path in current_by_path.keys() {
        if !recorded_by_path.contains_key(path) {
            changes.push(format!("{path} (added)"));
        }
    }
    changes.sort();
    changes
}

/// Performance metrics recorded after executing a rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRecord {
    /// Name of the rule that was benchmarked.
    pub rule: String,
    /// Wall-clock time in seconds.
    pub wall_time_secs: f64,
    /// Peak RSS in megabytes (issue #67 §4) — sampled by the local
    /// executor, read from the scheduler's accounting store on the cluster
    /// path. `None` for legacy checkpoints and whenever the source did not
    /// report it.
    pub max_memory_mb: Option<u64>,
    /// The rule's declared memory limit in megabytes (`effective_memory()`
    /// resolved at execution time) — the "limit" side of bottleneck
    /// detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit_mb: Option<u64>,
    /// CPU time in seconds — sampled from the rule's own process by the
    /// local executor (all its threads; child processes are not
    /// accumulated; issue #83 P1-13), reported by the accounting store on
    /// the cluster path, where it DOES span every step of the job. `None`
    /// for legacy checkpoints and whenever the source did not report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<f64>,
    /// Number of retry attempts before success (0 = first attempt succeeded).
    #[serde(default)]
    pub retries: u32,
    /// Why this benchmark carries no real measurements. `Some("outputs
    /// up-to-date")` marks the synthetic record written on resume when an
    /// interrupted rule's outputs are verdicted up-to-date (issue #324 F-1):
    /// the entry exists so later runs short-circuit it as completed, but its
    /// `wall_time_secs` is a 0.0 placeholder, not a measurement — displays
    /// use this marker to show `-`/exclude it instead of a fake `0.0s`.
    /// `None` for every normally-executed rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_as: Option<String>,
}

/// Per-rule execution record persisted for reporting (issue #83 WS2):
/// the exit code and expanded command that actually ran, plus a bounded
/// stderr excerpt for failure diagnosis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleRunRecord {
    /// Process exit code; `None` when the record predates execution or the
    /// rule was skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The expanded command that was executed (wildcards and `{config.x}`
    /// resolved). Absent in legacy checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Tail of the rule's stderr (see [`DEFAULT_OUTPUT_TAIL_BYTES`]) for
    /// failure diagnosis. Absent when the rule produced no stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    /// Tail of the rule's stdout (see [`DEFAULT_OUTPUT_TAIL_BYTES`], issue
    /// #691): some tools print their root cause or key diagnostics on
    /// stdout, which used to be visible only live in the terminal. Absent
    /// when the rule produced no stdout or the record predates the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_tail: Option<String>,
    /// The rule's resolved report caption (inline text or the content of its
    /// `report.file`), captured at execution time (issue #281). Absent in
    /// legacy checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// Terminal status of the run ("failed", "cancelled", "timed_out", …)
    /// — lets `rule_runs` express a cancellation as distinct from a
    /// failure (issue #498). Absent in legacy checkpoints, which keep the
    /// old inference: exit_code present → failed/completed as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Why the rule did not run to completion (`"run aborted before this
    /// rule finished — required rule 'x' failed"`, …). Absent in legacy
    /// checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// Terminating signal when the process died to one — the evidence
    /// that a recorded failure was the abort's kill, not a self-caused
    /// failure (issue #498). Absent in legacy checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
}

/// Persistent checkpoint state for resumable workflow execution.
///
/// Tracks which rules have completed or failed so that a restarted workflow
/// can skip already-finished work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointState {
    /// Rules that completed successfully.
    pub completed_rules: BTreeSet<String>,
    /// Rules that failed during execution.
    pub failed_rules: BTreeSet<String>,
    /// Rules submitted to the scheduler whose process had not finished when
    /// the checkpoint was last written (issue #685). Persisted at spawn time
    /// so an interrupted run's resume sees an honest in-flight set instead of
    /// treating those rules as "not yet executed". Entries are transient:
    /// every terminal transition removes the rule, and a resume clears stale
    /// entries once (the processes they describe died with the run).
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub running: BTreeSet<String>,
    /// Benchmark records keyed by rule name.
    pub benchmarks: BTreeMap<String, BenchmarkRecord>,
    /// Path to the workflow file that generated this checkpoint.
    /// Enables the `resume` command to locate the original workflow.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_path: Option<String>,
    /// HEAD commit SHA of the git repository the workflow lives in, recorded
    /// at run start (issue #115 pillar 1): which workflow VERSION produced
    /// these results, auditably. `None` when the workflow is not inside a
    /// git repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_git_sha: Option<String>,
    /// Working directory the rules executed in (issue #68). `resume` re-runs
    /// from this directory so completed rules' outputs resolve the same way;
    /// absent in legacy checkpoints, which fall back to the workflow's
    /// directory.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Output file checksums for provenance verification.
    /// Maps relative output file path → `sha256:<hex>`.
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub checksums: BTreeMap<String, String>,
    /// Checksums of outputs the engine deletes by design at the end of a
    /// successful run (transform chunk intermediates with `cleanup = true`,
    /// issue #315 F2). Preserved for the audit trail; `provenance verify`
    /// reports them as "cleaned", never "missing". Absent in legacy
    /// checkpoints, which keep the old behavior (empty = nothing cleaned).
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cleaned_checksums: BTreeMap<String, String>,
    /// Config value snapshot at the time rules completed.
    /// Maps config key → canonical value string (sensitive keys store a
    /// SHA-256 digest instead of the plaintext value). Compared against the
    /// current config on every run to drive precise invalidation (issue #62).
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config_snapshot: BTreeMap<String, String>,
    /// Per-rule structural fingerprints at completion time.
    /// Maps rule name → `sha256:<hex>` of the fields that determine rule
    /// output content (shell, script, inputs, outputs, envvars, params,
    /// conditions, environment). A mismatch invalidates the rule and its
    /// downstream (issue #62).
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub rule_fingerprints: BTreeMap<String, String>,
    /// Per-rule fingerprints with the input field EXCLUDED (issue #142 M1).
    /// Distinguishes a genuine rule edit from a pure `--samples` selection
    /// change: expand_inputs-over-injected-key rules bake the selection into
    /// their input list, so the full fingerprint differs on every subset
    /// run while this one stays identical. Absent for checkpoints written by
    /// older binaries — those keep invalidating (safe default).
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub rule_fingerprints_no_input: BTreeMap<String, String>,
    /// Per-rule input manifests at completion time (issue #72).
    /// Maps rule name → sorted list of (relative path, size, mtime) for every
    /// file the rule's inputs resolved to. A mismatch with the current file
    /// set invalidates the rule and its downstream.
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub input_manifests: BTreeMap<String, InputManifest>,
    /// Per-rule `when`-condition verdicts under the run's config (issue
    /// #198). Maps rule name → the boolean the gate evaluated to at config-
    /// change detection time. A completed rule whose gate references changed
    /// keys but whose verdict is unchanged keeps its checkpoint entry; a
    /// flipped verdict invalidates it. Absent in checkpoints written by
    /// older binaries — referencing rules keep invalidating until adopted.
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub when_verdicts: BTreeMap<String, bool>,

    /// Tombstones for `temporary = true` rules whose outputs were deleted
    /// after a fully successful run. Maps rule name → deleted output paths;
    /// a future run regenerates them via cascade-up (the completed producer
    /// is re-executed when a dependent needs the missing inputs).
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tombstones: BTreeMap<String, Vec<String>>,

    /// Checkpoint re-entries (issue #78 P3): the values each checkpoint rule
    /// contributed to the plan, so resumes replay them deterministically and
    /// revoke them when the rule is invalidated. Legacy checkpoints load
    /// with an empty list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reentries: Vec<crate::reentry::ReentryRecord>,

    /// Per-rule execution records for reporting (issue #83 WS2): exit code,
    /// expanded command, and stderr excerpt. Legacy checkpoints load with an
    /// empty map; the report falls back to declared workflow templates for
    /// rules without a record.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rule_runs: BTreeMap<String, RuleRunRecord>,

    /// Runtime-discovered output_pattern domains (issue #227 item 5):
    /// producer template → the wildcard combos discovered after its
    /// instances completed. Persisted so `resume` re-instantiates the
    /// deferred consumers WITHOUT re-running the producer. Legacy
    /// checkpoints load with an empty map.
    ///
    /// Serializes canonically (#528): combos as key-sorted pairs (and the
    /// combo list itself sorted), so identical state produces identical
    /// bytes — the in-memory `WildcardValues` is a `HashMap`, whose
    /// iteration order is per-process random and made provenance diffs
    /// noisy. Dedup semantics only ever compare `wildcard_combo_key`, so
    /// canonical ordering cannot change behavior.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        serialize_with = "serialize_output_pattern_domains",
        deserialize_with = "deserialize_output_pattern_domains"
    )]
    pub output_pattern_domains: BTreeMap<String, Vec<crate::wildcard::WildcardValues>>,
}

/// Canonical on-the-wire form: template → combos → key-sorted
/// `(key, value)` pairs, combos sorted among themselves.
type CanonicalDomains = BTreeMap<String, Vec<Vec<(String, String)>>>;

fn canonical_domains(
    domains: &BTreeMap<String, Vec<crate::wildcard::WildcardValues>>,
) -> CanonicalDomains {
    domains
        .iter()
        .map(|(template, combos)| {
            let mut canonical_combos: Vec<Vec<(String, String)>> = combos
                .iter()
                .map(|combo| {
                    let mut pairs: Vec<(String, String)> =
                        combo.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    pairs.sort();
                    pairs
                })
                .collect();
            canonical_combos.sort();
            (template.clone(), canonical_combos)
        })
        .collect()
}

fn serialize_output_pattern_domains<S: serde::Serializer>(
    domains: &BTreeMap<String, Vec<crate::wildcard::WildcardValues>>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    canonical_domains(domains).serialize(serializer)
}

fn deserialize_output_pattern_domains<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, Vec<crate::wildcard::WildcardValues>>, D::Error> {
    let raw: CanonicalDomains = serde::Deserialize::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|(template, combos)| {
            (
                template,
                combos
                    .into_iter()
                    .map(|pairs| {
                        pairs
                            .into_iter()
                            .collect::<crate::wildcard::WildcardValues>()
                    })
                    .collect(),
            )
        })
        .collect())
}

/// Default bound on the stdout/stderr excerpt persisted per rule. The
/// excerpt is the tail of the captured output: enough for failure
/// diagnosis without growing unbounded on noisy tools. Override with
/// `OXO_FLOW_OUTPUT_TAIL_BYTES` (see
/// [`crate::executor::ExecutorConfig::output_tail_bytes`]).
pub const DEFAULT_OUTPUT_TAIL_BYTES: usize = 64 * 1024;

/// Resolve the configured output-tail size from `OXO_FLOW_OUTPUT_TAIL_BYTES`.
///
/// Callers without access to an
/// [`crate::executor::ExecutorConfig`]-derived value (the signal
/// handler, cluster submission paths) use this to honor the same override.
/// A missing variable yields the default; an unparsable or zero value falls
/// back to the default with a warning — an explicit-but-broken setting must
/// not silently disable the tail.
pub fn output_tail_bytes_from_env() -> usize {
    const ENV_VAR: &str = "OXO_FLOW_OUTPUT_TAIL_BYTES";
    match std::env::var(ENV_VAR) {
        Ok(raw) => match raw.trim().parse::<usize>() {
            Ok(0) | Err(_) => {
                tracing::warn!(value = %raw, "invalid {ENV_VAR}, using the default output tail size");
                DEFAULT_OUTPUT_TAIL_BYTES
            }
            Ok(bytes) => bytes,
        },
        Err(_) => DEFAULT_OUTPUT_TAIL_BYTES,
    }
}

/// Last `bytes` bytes of a stream capture, prefixed with an ellipsis marker
/// when truncated. The cut point is walked back to a UTF-8 character
/// boundary so the persisted tail is always valid `String` content. The
/// `"…\n"` marker (4 UTF-8 bytes) is additive metadata — the content
/// portion never exceeds `bytes`, so the persisted total is at most
/// `bytes + 4`.
fn stream_tail(stream: Option<&str>, bytes: usize) -> Option<String> {
    let stream = stream?;
    if bytes == 0 {
        return Some(String::from("…\n"));
    }
    if stream.len() <= bytes {
        return Some(stream.to_string());
    }
    let mut start = stream.len() - bytes;
    while !stream.is_char_boundary(start) {
        start += 1;
    }
    Some(format!("…\n{}", &stream[start..]))
}

impl CheckpointState {
    /// Create a new, empty checkpoint state.
    pub fn new() -> Self {
        Self {
            completed_rules: BTreeSet::new(),
            failed_rules: BTreeSet::new(),
            running: BTreeSet::new(),
            benchmarks: BTreeMap::new(),
            workflow_path: None,
            workflow_git_sha: None,
            workdir: None,
            checksums: BTreeMap::new(),
            cleaned_checksums: BTreeMap::new(),
            config_snapshot: BTreeMap::new(),
            rule_fingerprints: BTreeMap::new(),
            rule_fingerprints_no_input: BTreeMap::new(),
            input_manifests: BTreeMap::new(),
            when_verdicts: BTreeMap::new(),
            tombstones: BTreeMap::new(),
            reentries: Vec::new(),
            rule_runs: BTreeMap::new(),
            output_pattern_domains: BTreeMap::new(),
        }
    }

    /// Record a runtime-discovered output_pattern domain for a producer
    /// template (issue #227 item 5): the union across the producer's
    /// instances, deduped by sorted key=value — the same canonical form
    /// `contribute_output_pattern_domain` uses, so repeated contributions
    /// are idempotent.
    pub fn record_output_pattern_domain(
        &mut self,
        producer_template: &str,
        combos: Vec<crate::wildcard::WildcardValues>,
    ) {
        let entry = self
            .output_pattern_domains
            .entry(producer_template.to_string())
            .or_default();
        for combo in combos {
            let key = crate::wildcard::wildcard_combo_key(&combo);
            if !entry
                .iter()
                .any(|e| crate::wildcard::wildcard_combo_key(e) == key)
            {
                entry.push(combo);
            }
        }
    }

    /// Record a checkpoint re-entry, superseding any previous record for the
    /// same checkpoint rule (issue #78 P3).
    pub fn record_reentry(&mut self, record: crate::reentry::ReentryRecord) {
        self.reentries.retain(|r| r.rule != record.rule);
        self.reentries.push(record);
    }

    /// Record a checksum for an output file (provenance tracking).
    pub fn record_checksum(&mut self, path: &str, checksum: String) {
        self.checksums.insert(path.to_string(), checksum);
    }

    /// Record a checksum for an output the engine deletes by design
    /// (transform chunk intermediates with `cleanup = true`, issue #315 F2).
    /// Kept out of `checksums` so `provenance verify` reports these as
    /// "cleaned" instead of "missing".
    pub fn record_cleaned_checksum(&mut self, path: &str, checksum: String) {
        self.cleaned_checksums.insert(path.to_string(), checksum);
    }

    /// Record the input manifest for a rule (issue #72).
    pub fn record_input_manifest(&mut self, rule: &str, manifest: InputManifest) {
        self.input_manifests.insert(rule.to_string(), manifest);
    }

    /// Set the workflow path that generated this checkpoint.
    pub fn set_workflow_path(&mut self, path: &Path) {
        self.workflow_path = Some(path.to_string_lossy().to_string());
    }

    /// Whether this checkpoint was written by a DIFFERENT workflow than
    /// `current` (audit B8). A checkpoint lives under a shared workdir, so
    /// two workflows pointed at the same directory read each other's
    /// completed-rule records and reuse one another's artifacts; freshness
    /// is per-workflow identity, not per-path.
    ///
    /// `None` means the checkpoint predates workflow tracking — callers
    /// keep the historical behavior and surface a note.
    pub fn foreign_workflow(&self, current: &Path) -> Option<bool> {
        let recorded = self.workflow_path.as_deref()?;
        Some(!same_workflow_path(Path::new(recorded), current))
    }

    /// Drop every reuse record a foreign checkpoint contributed (audit B8):
    /// completed and failed rules, benchmark and provenance records, and the
    /// per-rule fingerprints/manifests that gate re-execution. Structural
    /// fields that describe THIS run (workflow identity, workdir) are left
    /// for the caller to set.
    pub fn invalidate_reuse_records(&mut self) {
        self.completed_rules.clear();
        self.failed_rules.clear();
        // A foreign checkpoint's running entries describe processes from a
        // different workflow's run — meaningless here (issue #685).
        self.running.clear();
        self.benchmarks.clear();
        self.checksums.clear();
        self.config_snapshot.clear();
        self.rule_fingerprints.clear();
        self.rule_fingerprints_no_input.clear();
        self.input_manifests.clear();
        self.when_verdicts.clear();
        self.tombstones.clear();
        self.rule_runs.clear();
        self.reentries.clear();
        self.output_pattern_domains.clear();
    }

    /// Record the workflow repository's HEAD SHA (issue #115 pillar 1).
    pub fn set_workflow_git_sha(&mut self, sha: String) {
        self.workflow_git_sha = Some(sha);
    }

    /// Resolve the HEAD commit SHA of the git repository containing
    /// `workflow_path`, if any. Walks up from the workflow's directory to
    /// find `.git`, then runs `git rev-parse HEAD`. Returns `None` when the
    /// workflow is not in a git repository or git is unavailable.
    pub fn workflow_git_sha(workflow_path: &Path) -> Option<String> {
        let root = crate::git::find_repo_root(workflow_path)?;
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .ok()?;
        if out.status.success() {
            let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !sha.is_empty() {
                return Some(sha);
            }
        }
        None
    }

    /// Set the working directory rules executed in (issue #68).
    pub fn set_workdir(&mut self, path: &Path) {
        self.workdir = Some(path.to_string_lossy().to_string());
    }

    /// Record that a rule's process has been submitted for execution
    /// (issue #685). Written at spawn time, NOT plan time, so the persisted
    /// running set is authoritative: a checkpoint written after spawn always
    /// lists the rule until a terminal transition removes it.
    pub fn mark_running(&mut self, rule: &str) {
        self.running.insert(rule.to_string());
    }

    /// Whether the rule is currently recorded as in-flight (issue #685).
    pub fn is_running(&self, rule: &str) -> bool {
        self.running.contains(rule)
    }

    /// The set of rules recorded as in-flight (issue #685).
    pub fn running_rules(&self) -> &BTreeSet<String> {
        &self.running
    }

    /// Remove a rule from the running set without a terminal verdict
    /// (issue #685). Used on resume to clear stale entries left by an
    /// interrupted run — those processes died with the run, so carrying
    /// them forward would be a lie.
    pub fn clear_running(&mut self, rule: &str) {
        self.running.remove(rule);
    }

    /// Mark a rule as successfully completed and store its benchmark.
    pub fn mark_completed(&mut self, rule: &str, benchmark: BenchmarkRecord) {
        self.completed_rules.insert(rule.to_string());
        self.failed_rules.remove(rule);
        self.running.remove(rule);
        self.benchmarks.insert(rule.to_string(), benchmark);
    }

    /// Mark a rule as completed without touching its benchmark.
    ///
    /// Used when a skip verdict ("outputs up-to-date") re-completes a rule
    /// that already has a measured benchmark — the placeholder must not
    /// overwrite real wall-time history.
    pub fn mark_completed_quiet(&mut self, rule: &str) {
        self.completed_rules.insert(rule.to_string());
        self.failed_rules.remove(rule);
        self.running.remove(rule);
    }

    /// Mark a rule as failed.
    pub fn mark_failed(&mut self, rule: &str) {
        self.failed_rules.insert(rule.to_string());
        self.completed_rules.remove(rule);
        self.running.remove(rule);
    }

    /// Persist execution detail for reporting (issue #83 WS2): the exit
    /// code and expanded command that actually ran, plus a bounded excerpt
    /// of each output stream. Call at completion/failure time, before the
    /// corresponding `mark_completed`/`mark_failed`. `tail_bytes` bounds
    /// each excerpt (see [`DEFAULT_OUTPUT_TAIL_BYTES`]).
    pub fn record_run(&mut self, record: &JobRecord, tail_bytes: usize) {
        self.rule_runs.insert(
            record.rule.clone(),
            RuleRunRecord {
                exit_code: record.exit_code,
                command: record.command.clone(),
                stderr_tail: stream_tail(record.stderr.as_deref(), tail_bytes),
                stdout_tail: stream_tail(record.stdout.as_deref(), tail_bytes),
                caption: record.caption.clone(),
                status: Some(record.status.to_string()),
                skip_reason: record.skip_reason.clone(),
                signal: record.signal,
            },
        );
    }

    /// Demote a recorded failure to a cancellation when the only evidence
    /// is a signal death with no exit code (issue #498): during a fail-fast
    /// abort, siblings killed by the process-tree signal used to be counted
    /// as self-caused failures because their closures completed their
    /// bookkeeping before the abort's reconciliation ran. Removes the rule
    /// from `failed_rules` and re-records the run as cancelled, preserving
    /// the command and stderr tail for the audit trail. Returns `false`
    /// (no-op) for rules without pure signal-death evidence — a genuine
    /// failure keeps its record.
    pub fn demote_failed_to_cancelled(&mut self, rule: &str, skip_reason: String) -> bool {
        let is_signal_death = self.failed_rules.contains(rule)
            && self
                .rule_runs
                .get(rule)
                .is_some_and(|r| r.exit_code.is_none() && r.signal.is_some());
        if !is_signal_death {
            return false;
        }
        self.failed_rules.remove(rule);
        if let Some(existing) = self.rule_runs.get(rule) {
            let command = existing.command.clone();
            let stderr_tail = existing.stderr_tail.clone();
            let stdout_tail = existing.stdout_tail.clone();
            let caption = existing.caption.clone();
            let signal = existing.signal;
            self.rule_runs.insert(
                rule.to_string(),
                RuleRunRecord {
                    exit_code: None,
                    command,
                    stderr_tail,
                    stdout_tail,
                    caption,
                    status: Some(crate::executor::JobStatus::Cancelled.to_string()),
                    skip_reason: Some(skip_reason),
                    signal,
                },
            );
        }
        true
    }

    /// Returns `true` if the rule finished successfully.
    pub fn is_completed(&self, rule: &str) -> bool {
        self.completed_rules.contains(rule)
    }

    /// Returns `true` if the rule should be skipped (i.e., it already completed).
    pub fn should_skip(&self, rule: &str) -> bool {
        self.is_completed(rule)
    }

    /// Serialize the checkpoint state to a JSON string.
    #[must_use = "serialization returns a Result that must be used"]
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| OxoFlowError::Config {
            message: format!("failed to serialize checkpoint: {e}"),
        })
    }

    /// Deserialize a checkpoint state from a JSON string.
    #[must_use = "deserialization returns a Result that must be used"]
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| OxoFlowError::Config {
            message: format!("failed to deserialize checkpoint: {e}"),
        })
    }

    /// Save checkpoint state to a file.
    ///
    /// Atomic (issue #194 A1): serialize to a sibling `*.tmp`, fsync it,
    /// rename over the target (atomic on POSIX), then fsync the parent
    /// directory so the rename itself survives a crash. A power failure can
    /// no longer leave a truncated `checkpoint.json` — readers see either
    /// the previous state or the complete new one.
    /// Blocking-pool variant of [`Self::save_to_file`] (#527): every rule
    /// completion serializes and fsyncs the whole checkpoint — the write +
    /// two fsyncs belong on the blocking pool, not a runtime worker. Call
    /// it while holding the checkpoint mutex; `&self` stays borrowed, the
    /// serialized bytes move to the pool.
    ///
    /// The staged file carries a per-call unique suffix instead of a fixed
    /// `.tmp` name: a concurrent writer (e.g. a rule task orphaned by
    /// `abort_all()` whose `save_to_file_async` future was dropped mid-`await`
    /// — the spawned blocking task still runs to completion) would otherwise
    /// rename/delete a sibling save's staged file out from under it, surfacing
    /// as an intermittent `No such file or directory` on the abort path.
    pub async fn save_to_file_async(&self, path: &Path) -> Result<()> {
        let json = self.to_json()?;
        let path = path.to_path_buf();
        let tmp_path = unique_staged_path(&path);
        tokio::task::spawn_blocking(move || write_checkpoint_staged(&json, &path, &tmp_path))
            .await
            .map_err(|e| OxoFlowError::Config {
                message: format!("checkpoint save task failed: {e}"),
            })?
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let json = self.to_json()?;
        let tmp_path = unique_staged_path(path);
        write_checkpoint_staged(&json, path, &tmp_path)
    }

    /// Load checkpoint state from a file.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let json = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Config {
            message: format!("failed to read checkpoint from {}: {e}", path.display()),
        })?;
        if json.trim().is_empty() {
            return Ok(Self::default());
        }
        Self::from_json(&json).map_err(|e| OxoFlowError::Config {
            message: format!(
                "failed to deserialize checkpoint from {}: {}",
                path.display(),
                e
            ),
        })
    }

    /// Returns the default checkpoint file path for a workflow.
    pub fn default_path(workdir: &Path) -> std::path::PathBuf {
        workdir.join(".oxo-flow").join("checkpoint.json")
    }

    /// Generate Prometheus-style text metrics from checkpoint state.
    ///
    /// Returns metrics in the Prometheus text exposition format suitable
    /// for scraping by Prometheus or compatible monitoring tools.
    pub fn to_prometheus_metrics(&self) -> String {
        let mut output = String::new();

        output.push_str(
            "# HELP oxo_flow_rules_completed_total Number of rules completed successfully.\n",
        );
        output.push_str("# TYPE oxo_flow_rules_completed_total counter\n");
        output.push_str(&format!(
            "oxo_flow_rules_completed_total {}\n",
            self.completed_rules.len()
        ));

        output.push_str("# HELP oxo_flow_rules_failed_total Number of rules that failed.\n");
        output.push_str("# TYPE oxo_flow_rules_failed_total counter\n");
        output.push_str(&format!(
            "oxo_flow_rules_failed_total {}\n",
            self.failed_rules.len()
        ));

        output.push_str("# HELP oxo_flow_rule_duration_seconds Wall-clock time per rule.\n");
        output.push_str("# TYPE oxo_flow_rule_duration_seconds gauge\n");
        for (rule, benchmark) in &self.benchmarks {
            output.push_str(&format!(
                "oxo_flow_rule_duration_seconds{{rule=\"{}\"}} {:.3}\n",
                rule, benchmark.wall_time_secs
            ));
        }

        if !self.benchmarks.is_empty() {
            let total_time: f64 = self.benchmarks.values().map(|b| b.wall_time_secs).sum();
            output.push_str("# HELP oxo_flow_total_duration_seconds Total execution time.\n");
            output.push_str("# TYPE oxo_flow_total_duration_seconds gauge\n");
            output.push_str(&format!(
                "oxo_flow_total_duration_seconds {:.3}\n",
                total_time
            ));
        }

        output
    }
}

impl Default for CheckpointState {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if `source` is newer than `target` (Make-style freshness check).
///
/// If either file does not exist or its metadata cannot be read, returns `false`.
pub fn file_is_newer(source: &Path, target: &Path) -> bool {
    let source_modified = match std::fs::metadata(source).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let target_modified = match std::fs::metadata(target).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    source_modified > target_modified
}

/// Compute a checksum of a file for integrity and non-determinism detection.
///
/// Uses SHA-256 for clinical-grade integrity verification.
///
/// Returns the hex-encoded SHA-256 hash string prefixed with "sha256:",
/// or an error if the file cannot be read.
pub fn compute_file_checksum(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    // Refuse special files before opening: `File::open` on a writer-less
    // FIFO blocks forever (seen with STAR's leftover `tmp.fifo.read*`
    // under .oxo-flow/tmp/ after a killed run, issue #695), and sockets /
    // char devices have no meaningful content checksum.
    let md = std::fs::metadata(path).map_err(|e| OxoFlowError::Execution {
        rule: String::new(),
        message: format!("failed to stat {} for checksum: {e}", path.display()),
    })?;
    if !md.is_file() {
        return Err(OxoFlowError::Execution {
            rule: String::new(),
            message: format!("cannot checksum {}: not a regular file", path.display()),
        });
    }

    let file = std::fs::File::open(path).map_err(|e| OxoFlowError::Execution {
        rule: String::new(),
        message: format!("failed to open {} for checksum: {e}", path.display()),
    })?;

    // Streaming SHA-256 with 64KB buffer — avoids loading entire file into memory.
    // Critical for large bioinformatics files (BAM, FASTQ can be >100GB).
    let mut reader = std::io::BufReader::with_capacity(65536, file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| OxoFlowError::Execution {
                rule: String::new(),
                message: format!("failed to read {} for checksum: {e}", path.display()),
            })?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    let hash = hasher.finalize();
    Ok(format!("sha256:{}", hex::encode(hash)))
}

/// mtime in nanoseconds since the UNIX epoch, for change detection.
///
/// Shared by input manifests (issue #72) and reference fingerprints
/// (issue #97): the two invalidation layers must agree on what counts as
/// "changed". An unreadable mtime degrades to 0 — never an error — and is
/// traced for diagnosis.
pub fn mtime_nanos(md: &std::fs::Metadata) -> i128 {
    match md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    {
        Some(d) => d.as_nanos() as i128,
        None => {
            tracing::debug!("mtime unavailable for change detection, degrading to 0");
            0
        }
    }
}

/// Content hash for small files, under the shared input-manifest policy:
/// `Some("sha256:…")` when the path is a regular file at most
/// [`MANIFEST_HASH_MAX_BYTES`]; `None` for larger files (guarded by
/// size+mtime), for unreadable files (best-effort degrade, never an
/// error), and for special files — hashing a FIFO would block forever
/// on open (issue #695), so those entries keep the size+mtime policy.
pub fn content_hash_if_small(path: &Path, md: &std::fs::Metadata) -> Option<String> {
    (md.is_file() && md.len() <= MANIFEST_HASH_MAX_BYTES)
        .then(|| compute_file_checksum(path).ok())
        .flatten()
}

/// Snapshot the file set a rule's inputs resolve to (issue #72).
///
/// Returns `Ok(None)` when the rule has no resolvable inputs: the input list
/// is empty, every pattern still contains an engine wildcard (`{sample}`,
/// `{threads}`, …) after config expansion, or the rule declares
/// `cleanup_chunks` / consumes engine-managed `.oxo-flow/chunks/` files
/// (ephemeral intermediates whose lifecycle the engine already governs).
///
/// Otherwise returns the sorted, deduplicated list of (relative path, size,
/// mtime) entries covering:
///
/// - plain file inputs (one entry each),
/// - literal glob inputs (`*`, `?`, `[`) expanded with `glob`-crate
///   semantics (the same expander used by sample discovery),
/// - `FilePatterns::Dir` inputs — a recursive listing, optionally filtered
///   by the Dir `pattern` glob,
/// - plain paths that resolve to directories (recursive listing).
///
/// Symlinked directories are recorded as single entries, never traversed —
/// the `walkdir` default — which keeps walks cycle-safe.
///
/// Returns `Err` when an input cannot be resolved (missing file/dir,
/// unreadable metadata, invalid glob pattern). Callers treat that as "cannot
/// verify" and invalidate the rule rather than reuse it.
/// Blocking-pool wrapper for [`snapshot_input_manifest`] (#527): the
/// manifest hashes every (small) input file — keep it off the workers.
pub async fn snapshot_input_manifest_async(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    resolver: &crate::storage::StorageResolver,
) -> Result<Option<InputManifest>> {
    let owned_rule = rule.clone();
    let rule_name = rule.name.clone();
    let workdir = workdir.to_path_buf();
    let wildcard_values = wildcard_values.clone();
    let resolver = resolver.clone();
    tokio::task::spawn_blocking(move || {
        snapshot_input_manifest(&owned_rule, &workdir, &wildcard_values, &resolver)
    })
    .await
    .map_err(|e| crate::error::OxoFlowError::Execution {
        rule: rule_name,
        message: format!("input manifest task failed: {e}"),
    })?
}

pub fn snapshot_input_manifest(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    resolver: &crate::storage::StorageResolver,
) -> Result<Option<InputManifest>> {
    if rule.input.is_empty() {
        return Ok(None);
    }
    // Chunk consumers clean their inputs at the end of a successful run —
    // snapshotting them would flag every completed transform as "inputs
    // deleted" on the next run. The engine's own invalidation (upstream
    // re-runs cascade downstream) already governs those intermediates.
    if rule.cleanup_chunks {
        return Ok(None);
    }

    // The Dir variant carries an optional filter glob that to_vec() omits.
    let dir_filter = match &rule.input {
        FilePatterns::Dir { pattern, .. } => pattern
            .as_ref()
            .map(|p| expand_config_in_path(p, wildcard_values)),
        _ => None,
    };

    let mut entries: std::collections::BTreeMap<String, InputManifestEntry> =
        std::collections::BTreeMap::new();
    let mut saw_resolvable = false;
    for pattern in rule.input.to_vec() {
        let expanded = expand_config_in_path(&pattern, wildcard_values);
        if expanded.is_empty() {
            // Config-optional input resolved to the empty string (e.g.
            // `input = ["{config.annotation_gtf}"]` with an empty default
            // for the download branch). `workdir.join("")` is the workdir
            // root — walking it would record the ENTIRE workdir (logs,
            // .git, other rules' outputs) as the rule's input manifest,
            // making every volatile file change spuriously invalidate the
            // rule and its whole downstream DAG. The empty pattern
            // contributes nothing to snapshot; skip it (live: rnaseq-sd
            // get_annotation/get_genome cascaded 121 rules into a ~96-min
            // star_index rebuild).
            continue;
        }
        if expanded.contains('{') || input_is_ancient(rule, &expanded, wildcard_values) {
            // Engine wildcard ({sample}, {threads}, …) — expanded per
            // instance before checkpointing, not resolvable here.
            continue;
        }
        if expanded.starts_with(".oxo-flow/chunks") {
            // Engine-managed ephemeral intermediates — see cleanup_chunks.
            continue;
        }
        saw_resolvable = true;

        // Remote objects (issue #78 P2): record (scheme, key, size, etag)
        // so the same manifests_match path serves local and cloud inputs.
        let storage_path = crate::storage::StoragePath::parse(&expanded);
        if storage_path.is_remote() {
            match resolve_remote_stat(resolver, &storage_path) {
                Ok(Some(stat)) => {
                    entries.insert(
                        expanded.clone(),
                        InputManifestEntry {
                            path: expanded.clone(),
                            size: stat.size,
                            mtime_nanos: 0,
                            hash: None,
                            remote: Some(RemoteManifestEntry {
                                scheme: match storage_path.scheme {
                                    crate::storage::StorageScheme::S3 => "s3",
                                    crate::storage::StorageScheme::Gcs => "gs",
                                    crate::storage::StorageScheme::Local => "local",
                                }
                                .to_string(),
                                key: expanded.clone(),
                                size: stat.size,
                                etag: stat.etag,
                            }),
                        },
                    );
                }
                Ok(None) => {
                    tracing::warn!(input = %expanded, "remote input does not exist at snapshot time; entry skipped");
                }
                Err(e) => {
                    tracing::warn!(input = %expanded, error = %e, "remote input metadata unavailable; entry skipped");
                }
            }
            continue;
        }
        if let Err(e) =
            collect_pattern_entries(&expanded, dir_filter.as_deref(), workdir, &mut entries)
        {
            if rule.optional.is_optional() {
                // Optional rule (`optional = true`/`"any"`, issue #633): a
                // permanently-absent declared input — its producer was never
                // instantiated (input_groups matched no files, or the
                // endedness filter dropped it) — must not poison the whole
                // snapshot. Skip the entry like the remote-path degradation
                // above: the runtime skip semantics (`optional_inputs_missing`)
                // already govern whether the rule may run, and a recorded
                // partial manifest is what makes the completed rule stable
                // across runs. Non-optional rules still propagate — a missing
                // required input is genuine invalidation.
                tracing::warn!(
                    input = %expanded,
                    error = %e,
                    "optional input absent at snapshot time; entry skipped"
                );
                continue;
            }
            if rule.expand_inputs_baked.contains(&expanded) {
                // expand_inputs-baked literal absent by design (issue
                // #757): the producer is gated off under the active
                // config, so the baked path can never exist right now —
                // but a later config flip that produces it must still
                // invalidate. Record the existing subset: the manifest
                // then mismatches when the file set changes, which is
                // exactly the desired invalidation. Hard-erroring here
                // instead forfeited the manifest AND invalidated the
                // completed rule on every subsequent no-op rerun (a
                // 50-producer cascade in the rnaseq fixture).
                tracing::warn!(
                    input = %expanded,
                    "expand_inputs-baked input absent at snapshot time (producer \
                     gated off under the active config?); entry skipped"
                );
                continue;
            }
            return Err(e);
        }
    }

    if !saw_resolvable {
        return Ok(None);
    }
    Ok(Some(entries.into_values().collect()))
}

/// Resolve a remote object's metadata through the registered backend.
///
/// The snapshot function is synchronous (the preview path calls it from
/// sync code); remote HEAD requests bridge onto the ambient tokio runtime.
/// Local-only workflows never reach this function.
fn resolve_remote_stat(
    resolver: &crate::storage::StorageResolver,
    path: &crate::storage::StoragePath,
) -> Result<Option<crate::storage::RemoteStat>> {
    let backend = match resolver.get_backend(&path.scheme) {
        Some(b) => b.clone(),
        None => {
            return Err(OxoFlowError::Config {
                message: format!("no storage backend registered for scheme '{}'", path.raw),
            });
        }
    };
    backend.head_blocking(path)
}

/// Input patterns (config-expanded) of `rule` that currently fail to
/// resolve, in declaration order. Mirrors [`snapshot_input_manifest`]'s
/// per-pattern walk — same expansion, same skip rules — so callers can tell
/// WHICH inputs are missing: tombstone-aware callers need the exact
/// producers, not just "cannot verify".
#[must_use]
pub fn missing_input_patterns(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> Vec<String> {
    if rule.input.is_empty() || rule.cleanup_chunks {
        return Vec::new();
    }
    let dir_filter = match &rule.input {
        FilePatterns::Dir { pattern, .. } => pattern
            .as_ref()
            .map(|p| expand_config_in_path(p, wildcard_values)),
        _ => None,
    };
    let mut missing = Vec::new();
    for expanded in rendered_input_patterns(rule, wildcard_values) {
        let mut entries = std::collections::BTreeMap::new();
        if collect_pattern_entries(&expanded, dir_filter.as_deref(), workdir, &mut entries).is_err()
        {
            missing.push(expanded);
        }
    }
    missing
}

/// Config-expanded input patterns of `rule` that this run must resolve
/// locally, in declaration order — the SAME walk (and skip rules) as
/// [`missing_input_patterns`] but WITHOUT any existence requirement:
/// empty/`{`-residual/`.oxo-flow/chunks` state, `ancient` inputs, remote
/// objects, and `expand_inputs_baked` literals are all skipped. Issue #837
/// workdir source linking consumes these to decide which workflow-repo
/// files must be reachable from the run workdir.
#[must_use]
pub fn rendered_input_patterns(
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
) -> Vec<String> {
    if rule.input.is_empty() || rule.cleanup_chunks {
        return Vec::new();
    }
    let mut rendered = Vec::new();
    for pattern in rule.input.to_vec() {
        let expanded = expand_config_in_path(&pattern, wildcard_values);
        if expanded.is_empty() || expanded.contains('{') || expanded.starts_with(".oxo-flow/chunks")
        {
            // Same skip rules as snapshot_input_manifest — an empty
            // config-optional input is not a missing file.
            continue;
        }
        // Mirror snapshot_input_manifest's remaining two skips (audit
        // #649): remote objects resolve via HEAD requests and are never
        // reported as locally missing (a workdir.join("s3://…") stat
        // always fails and would poison the tombstone explanation with a
        // phantom), and `ancient` inputs are exempt from invalidation
        // entirely (issue #469).
        if input_is_ancient(rule, &expanded, wildcard_values) {
            continue;
        }
        if crate::storage::StoragePath::parse(&expanded).is_remote() {
            continue;
        }
        // Same skip as the snapshot walk (issue #757): a baked literal
        // absent by design is tolerated, not reported missing.
        if rule.expand_inputs_baked.contains(&expanded) {
            continue;
        }
        rendered.push(expanded);
    }
    rendered
}

/// Concrete regular files matched by an input `pattern` under `base_dir`:
/// globs resolve (no match → empty), plain paths are taken literally and
/// must exist as files. Used by workdir source linking (#837) to expand
/// repo-side globs into linkable files; a missing path yields `Ok(vec![])`
/// — absence is the CALLER's signal, not an error.
pub fn resolve_pattern_files(pattern: &str, base_dir: &Path) -> Vec<PathBuf> {
    let full = base_dir.join(pattern);
    let mut files = Vec::new();
    if is_glob_pattern(pattern) {
        if let Ok(paths) = glob::glob(&full.to_string_lossy()) {
            for matched in paths.flatten() {
                if matched.is_file() {
                    files.push(matched);
                }
            }
        }
    } else if full.is_file() {
        files.push(full);
    }
    files
}

/// Literal glob characters — distinct from `{engine}` wildcards
/// (`crate::wildcard::has_wildcards` only matches braces).
fn is_glob_pattern(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

fn collect_pattern_entries(
    pattern: &str,
    dir_filter: Option<&str>,
    workdir: &Path,
    entries: &mut std::collections::BTreeMap<String, InputManifestEntry>,
) -> Result<()> {
    let full = workdir.join(pattern);

    // A Dir input with a filter globs inside the directory; the directory
    // itself must exist or the rule cannot be verified.
    if let Some(filter) = dir_filter {
        let glob_pattern = full.join(filter);
        if !full.exists() {
            return Err(OxoFlowError::Execution {
                rule: String::new(),
                message: format!(
                    "cannot verify input directory {}: it does not exist",
                    full.display()
                ),
            });
        }
        for matched in
            glob::glob(&glob_pattern.to_string_lossy()).map_err(|e| OxoFlowError::Execution {
                rule: String::new(),
                message: format!("invalid glob pattern '{}': {}", glob_pattern.display(), e),
            })?
        {
            let matched = matched.map_err(|e| OxoFlowError::Execution {
                rule: String::new(),
                message: format!("glob error: {}", e),
            })?;
            insert_manifest_entry(&matched, workdir, entries)?;
        }
        return Ok(());
    }

    if is_glob_pattern(pattern) {
        for matched in glob::glob(&full.to_string_lossy()).map_err(|e| OxoFlowError::Execution {
            rule: String::new(),
            message: format!("invalid glob pattern '{}': {}", full.display(), e),
        })? {
            let matched = matched.map_err(|e| OxoFlowError::Execution {
                rule: String::new(),
                message: format!("glob error: {}", e),
            })?;
            insert_manifest_entry(&matched, workdir, entries)?;
        }
        // A glob matching nothing is a legitimate (if degenerate) input set.
        return Ok(());
    }

    insert_manifest_entry(&full, workdir, entries)
}

/// Record one path (file or directory) in the manifest.
///
/// Symlinked directories are recorded as single entries, never traversed
/// (cycle-safe, `walkdir` semantics); real directories are walked
/// recursively.
fn insert_manifest_entry(
    path: &Path,
    workdir: &Path,
    entries: &mut std::collections::BTreeMap<String, InputManifestEntry>,
) -> Result<()> {
    let smd = std::fs::symlink_metadata(path).map_err(|e| OxoFlowError::Execution {
        rule: String::new(),
        message: format!("cannot stat input {}: {}", path.display(), e),
    })?;
    if smd.file_type().is_dir() {
        return walk_dir(path, workdir, entries);
    }
    if smd.file_type().is_symlink() {
        // Stat the target (size/mtime) without traversing into it.
        let md = std::fs::metadata(path).map_err(|e| OxoFlowError::Execution {
            rule: String::new(),
            message: format!("cannot stat symlink target {}: {}", path.display(), e),
        })?;
        record_manifest_file(path, workdir, &md, entries);
        return Ok(());
    }
    record_manifest_file(path, workdir, &smd, entries);
    Ok(())
}

/// Recursively list regular files under `dir` (no symlink traversal).
fn walk_dir(
    dir: &Path,
    workdir: &Path,
    entries: &mut std::collections::BTreeMap<String, InputManifestEntry>,
) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| OxoFlowError::Execution {
        rule: String::new(),
        message: format!("cannot list input directory {}: {}", dir.display(), e),
    })?;
    for item in rd {
        let item = item.map_err(|e| OxoFlowError::Execution {
            rule: String::new(),
            message: format!("cannot read directory {}: {}", dir.display(), e),
        })?;
        let path = item.path();
        let ft = item.file_type().map_err(|e| OxoFlowError::Execution {
            rule: String::new(),
            message: format!("cannot stat {}: {}", path.display(), e),
        })?;
        if ft.is_dir() {
            walk_dir(&path, workdir, entries)?;
        } else if ft.is_symlink() {
            let md = std::fs::metadata(&path).map_err(|e| OxoFlowError::Execution {
                rule: String::new(),
                message: format!("cannot stat symlink target {}: {}", path.display(), e),
            })?;
            record_manifest_file(&path, workdir, &md, entries);
        } else {
            let md = item.metadata().map_err(|e| OxoFlowError::Execution {
                rule: String::new(),
                message: format!("cannot stat {}: {}", path.display(), e),
            })?;
            record_manifest_file(&path, workdir, &md, entries);
        }
    }
    Ok(())
}

fn record_manifest_file(
    path: &Path,
    workdir: &Path,
    md: &std::fs::Metadata,
    entries: &mut std::collections::BTreeMap<String, InputManifestEntry>,
) {
    let rel = path
        .strip_prefix(workdir)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    let mtime = mtime_nanos(md);
    let hash = content_hash_if_small(path, md);
    entries.insert(
        rel.clone(),
        InputManifestEntry {
            path: rel,
            size: md.len(),
            mtime_nanos: mtime,
            hash,
            remote: None,
        },
    );
}

/// Returns `true` when a rule declares `optional = true` and at least one of
/// its inputs does not exist (issue #75).
///
/// Existence rules: `{config.x}` placeholders are expanded first; engine
/// wildcards (`{sample}` etc.) are assumed present (they resolve
/// per-instance); literal globs count as present when they match at least
/// one file; plain paths must exist (file or directory).
pub fn optional_inputs_missing(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> bool {
    if !rule.optional.is_optional() || rule.input.is_empty() {
        return false;
    }
    let any_mode = rule.optional.is_any();
    for input in rule.input.to_vec() {
        let expanded = expand_config_in_path(&input, wildcard_values);
        if expanded.contains('{') {
            // Engine wildcard — assume present. In "any" mode one assumed
            // present is enough to run the rule.
            if any_mode {
                return false;
            }
            continue;
        }
        let exists = if is_glob_pattern(&expanded) {
            let pattern = workdir.join(&expanded);
            glob::glob(&pattern.to_string_lossy())
                .map(|paths| paths.filter_map(|p| p.ok()).next().is_some())
                .unwrap_or(false)
        } else {
            workdir.join(&expanded).exists()
        };
        if any_mode {
            if exists {
                return false; // at least one input exists — run
            }
        } else if !exists {
            return true; // "all" mode — any missing input skips the rule
        }
    }
    // "any" mode: none of the inputs existed — skip.
    any_mode
}

/// Check if a rule should be skipped based on output freshness, optionally
/// consulting recorded provenance checksums (issue #194 B2).
///
/// When `checksums` holds a recorded content hash for EVERY expanded output
/// of the rule, freshness requires the CURRENT content to match — mtime
/// alone no longer decides (a `touch` or clock skew cannot fake reuse).
/// Two honest fallbacks keep the old behavior: any output beyond the hash
/// cap (no re-verifiable digest), and rules with no recorded checksums at
/// all, both degrade to the mtime comparison.
/// Whether `expanded_input` is covered by the rule's `ancient` declarations
/// (issue #469): ancient inputs never trigger re-execution, so the
/// freshness gate and input manifests must ignore them. A literal
/// declaration matches after `{config.*}` expansion; a declaration that
/// still carries engine wildcards (`ref/{build}/hg38.fa`) matches
/// structurally via the template matcher.
fn input_is_ancient(
    rule: &Rule,
    expanded_input: &str,
    wildcard_values: &HashMap<String, String>,
) -> bool {
    rule.ancient.iter().any(|ancient| {
        let expanded = expand_config_in_path(ancient, wildcard_values);
        if expanded == expanded_input {
            return true;
        }
        if expanded.contains('{')
            && let Ok(re) = crate::wildcard::pattern_to_regex(&expanded)
        {
            return re.is_match(expanded_input);
        }
        false
    })
}

/// Blocking-pool wrapper for [`should_skip_rule_with_checksums`] (#527):
/// the checksum path hashes up to 64 MiB per output — synchronous bulk
/// work that must not run on a tokio worker.
pub async fn should_skip_rule_with_checksums_async(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    checksums: Option<&BTreeMap<String, String>>,
) -> bool {
    let rule = rule.clone();
    let workdir = workdir.to_path_buf();
    let wildcard_values = wildcard_values.clone();
    let checksums = checksums.cloned();
    tokio::task::spawn_blocking(move || {
        should_skip_rule_with_checksums(&rule, &workdir, &wildcard_values, checksums.as_ref())
    })
    .await
    .unwrap_or(false)
}

pub fn should_skip_rule_with_checksums(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    checksums: Option<&BTreeMap<String, String>>,
) -> bool {
    if rule.output.is_empty() {
        return false;
    }

    // Expand config vars in output paths (e.g. {config.sample} → SAMPLE001)
    let expanded_outputs: Vec<String> = rule
        .output
        .iter()
        .map(|o| expand_config_in_path(o, wildcard_values))
        .collect();

    // Skip if any expanded output still contains a wildcard pattern ({sample} etc.)
    if expanded_outputs.iter().any(|o| o.contains('{')) {
        return false;
    }
    // Expand config vars in inputs too (for freshness comparison)
    let expanded_inputs: Vec<String> = rule
        .input
        .iter()
        .map(|i| expand_config_in_path(i, wildcard_values))
        .collect();
    if expanded_inputs.iter().any(|i| i.contains('{')) {
        return false;
    }

    let all_outputs_exist = expanded_outputs.iter().all(|o| workdir.join(o).exists());
    if !all_outputs_exist {
        return false;
    }

    // A character device, FIFO, or socket can never prove a rule ran: its
    // mtime is effectively frozen (audit B4 — a `/dev/null` output made the
    // mtime gate report "up to date" forever), so any non-regular, non-
    // directory output forces re-execution.
    if expanded_outputs
        .iter()
        .any(|o| is_special_file(&workdir.join(o)))
    {
        return false;
    }

    // Checksum path (issue #194 B2): taken only when EVERY output has both a
    // recorded hash AND a re-computable digest (small-file cap). Content
    // identity then decides; a divergence re-executes even when mtime looks
    // fresh. Missing records or over-cap outputs degrade to the mtime path.
    if let Some(map) = checksums
        && expanded_outputs.iter().all(|o| map.contains_key(o))
    {
        let mut digests = Vec::with_capacity(expanded_outputs.len());
        let mut all_verifiable = true;
        for o in &expanded_outputs {
            let path = workdir.join(o);
            let Ok(md) = std::fs::metadata(&path) else {
                all_verifiable = false;
                break;
            };
            match content_hash_if_small(&path, &md) {
                Some(d) => digests.push(d),
                None => {
                    all_verifiable = false;
                    break;
                }
            }
        }
        if all_verifiable {
            let all_match = expanded_outputs
                .iter()
                .zip(&digests)
                .all(|(o, current)| map.get(o).is_some_and(|recorded| recorded == current));
            if all_match {
                return true;
            }
            // Content diverged — even if mtime looks fresh, re-execute.
            return false;
        }
    }

    // `ancient` inputs never trigger re-execution (issue #469) — excluded
    // from the mtime comparison entirely.
    let freshness_inputs: Vec<&String> = expanded_inputs
        .iter()
        .filter(|i| !input_is_ancient(rule, i, wildcard_values))
        .collect();
    if freshness_inputs.is_empty() {
        return true; // No inputs to check freshness against
    }
    // Check if all outputs are newer than all inputs
    freshness_inputs.iter().all(|input| {
        let input_path = workdir.join(input);
        expanded_outputs.iter().all(|output| {
            let output_path = workdir.join(output);
            file_is_newer(&output_path, &input_path)
        })
    })
}

/// Compare two recorded workflow paths for identity: canonicalized when both
/// resolve (so a moved workdir or a different mount spelling does not
/// invalidate an honest checkpoint), falling back to the raw strings.
fn same_workflow_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Returns `true` when `path` is a character device, block device, FIFO, or
/// socket — file types whose mtime says nothing about a rule having run, so
/// the freshness gate must never treat them as up to date (audit B4).
/// Directories stay eligible: a rule may legitimately declare one.
fn is_special_file(path: &Path) -> bool {
    let Ok(md) = std::fs::metadata(path) else {
        return false;
    };
    let ft = md.file_type();
    if ft.is_file() || ft.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        ft.is_char_device() || ft.is_block_device() || ft.is_fifo() || ft.is_socket()
    }
    #[cfg(not(unix))]
    {
        // No device/FIFO/socket types on this platform: nothing to exclude.
        false
    }
}

/// Expand `{key}` placeholders in a path string using the provided values map.
///
/// Only performs simple key-value substitution (no `{input[N]}` / `{output[N]}` logic).
/// Used for checking output file existence after expansion of config variables.
pub fn expand_config_in_path(path: &str, wildcard_values: &HashMap<String, String>) -> String {
    super::expand_to_fixed_point(path, wildcard_values, |value| value.to_owned())
}

/// Validate that declared output files exist after execution.
/// Returns a list of missing output file paths (after expanding config variables).
pub fn validate_outputs(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> Vec<String> {
    rule.output
        .iter()
        .filter_map(|output| {
            // Expand config variables (e.g. {config.sample}) before checking
            let expanded = expand_config_in_path(output, wildcard_values);
            // Skip paths that still contain wildcard patterns (e.g. {sample} from wildcard rules)
            if crate::wildcard::has_wildcards(&expanded) {
                return None;
            }
            let path = workdir.join(&expanded);
            if path.exists() { None } else { Some(expanded) }
        })
        .collect()
}

/// Clean up temporary output files produced by a rule.
pub async fn cleanup_temp_outputs(rule: &Rule, workdir: &Path) {
    for temp in &rule.temp_output {
        let path = workdir.join(temp);
        if tokio::fs::try_exists(&path).await.ok() == Some(true) {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!(file = %path.display(), error = %e, "failed to remove temp output");
            } else {
                tracing::debug!(file = %path.display(), "removed temp output");
            }
        }
    }
}

/// Clean up transform chunk files after a successful combine.
/// Deletes each chunk (the rule's inputs) and removes chunk directories
/// that became empty as a result. Directories holding chunks from other
/// rules are left untouched.
///
/// Returns the successfully deleted relative paths so the caller can move
/// their checksums into the `cleaned_checksums` bucket (issue #315 F2) —
/// deletion and record-migration happen in the same step.
pub async fn cleanup_transform_chunks(rule: &Rule, workdir: &Path) -> Vec<String> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    for chunk in rule.input.iter() {
        let path = workdir.join(chunk);
        if tokio::fs::try_exists(&path).await.ok() == Some(true) {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    tracing::debug!(file = %path.display(), "removed transform chunk");
                    deleted.push(chunk.to_string());
                    if let Some(parent) = path.parent() {
                        dirs.push(parent.to_path_buf());
                    }
                }
                Err(e) => {
                    tracing::warn!(file = %path.display(), error = %e, "failed to remove transform chunk")
                }
            }
        }
    }

    // Best-effort removal of emptied directories, deepest first.
    // remove_dir only succeeds when the directory is empty, so chunks
    // belonging to other rules keep their directories alive.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for dir in dirs {
        let _ = tokio::fs::remove_dir(&dir).await;
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::FilePatterns;
    use crate::storage::{RemoteStat, StorageBackend, StoragePath, StorageResolver, StorageScheme};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[test]
    fn mark_completed_quiet_preserves_measured_benchmark() {
        // The "outputs up-to-date" skip path re-completes a rule with a
        // synthetic 0.0s placeholder; that must not overwrite the real
        // benchmark (Dashboard Total Runtime and Prometheus duration gauges
        // sum these records).
        let mut ck = CheckpointState::default();
        ck.mark_completed(
            "fastqc",
            BenchmarkRecord {
                rule: "fastqc".into(),
                wall_time_secs: 2.75,
                max_memory_mb: Some(512),
                memory_limit_mb: None,
                cpu_seconds: Some(2.1),
                retries: 0,
                recorded_as: None,
            },
        );
        ck.mark_completed_quiet("fastqc");
        assert!(ck.completed_rules.contains("fastqc"));
        assert_eq!(ck.benchmarks["fastqc"].wall_time_secs, 2.75);
        assert_eq!(ck.benchmarks["fastqc"].recorded_as, None);
        // And mark_completed still overwrites when a caller genuinely wants
        // to insert a new record (unchanged contract).
        ck.mark_completed(
            "fastqc",
            BenchmarkRecord {
                rule: "fastqc".into(),
                wall_time_secs: 0.0,
                max_memory_mb: None,
                memory_limit_mb: None,
                cpu_seconds: None,
                retries: 0,
                recorded_as: Some("outputs up-to-date".into()),
            },
        );
        assert_eq!(
            ck.benchmarks["fastqc"].recorded_as.as_deref(),
            Some("outputs up-to-date")
        );
    }

    #[test]
    fn record_run_captures_status_skip_reason_and_signal() {
        // Issue #498: rule_runs can now express a cancellation (or any
        // terminal status) distinct from a failure, and carry the
        // signal-death evidence.
        let mut ck = CheckpointState::default();
        let record = JobRecord {
            rule: "slow_scan".into(),
            status: crate::executor::JobStatus::Cancelled,
            started_at: None,
            finished_at: Some(chrono::Utc::now()),
            exit_code: None,
            stdout: None,
            stderr: Some("[oxo-flow] command terminated by SIGTERM (15)".into()),
            command: Some("sleep 30".into()),
            retries: 0,
            skip_reason: Some("run aborted".into()),
            max_rss_mb: None,
            cpu_seconds: None,
            caption: None,
            signal: Some(15),
        };
        ck.record_run(&record, DEFAULT_OUTPUT_TAIL_BYTES);
        let r = &ck.rule_runs["slow_scan"];
        assert_eq!(r.status.as_deref(), Some("cancelled"));
        assert_eq!(r.skip_reason.as_deref(), Some("run aborted"));
        assert_eq!(r.signal, Some(15));
    }

    #[test]
    fn demote_failed_to_cancelled_only_for_signal_deaths() {
        // Issue #498: the abort reconciliation may demote a failure to a
        // cancellation ONLY when the failure's evidence is a signal death
        // with no exit code — a self-caused failure (exit code) and a
        // legacy record (no status/signal fields) both keep their record.
        let mk = |record: JobRecord| {
            let mut ck = CheckpointState::default();
            ck.mark_failed(&record.rule);
            ck.record_run(&record, DEFAULT_OUTPUT_TAIL_BYTES);
            ck
        };
        let base = |status: crate::executor::JobStatus,
                    exit_code: Option<i32>,
                    signal: Option<i32>|
         -> JobRecord {
            JobRecord {
                rule: "slow_scan".into(),
                status,
                started_at: None,
                finished_at: Some(chrono::Utc::now()),
                exit_code,
                stdout: None,
                stderr: None,
                command: Some("sleep 30".into()),
                retries: 0,
                skip_reason: None,
                max_rss_mb: None,
                cpu_seconds: None,
                caption: None,
                signal,
            }
        };

        // Signal death → demoted: out of failed_rules, record cancelled.
        let mut ck = mk(base(crate::executor::JobStatus::Failed, None, Some(15)));
        assert!(ck.demote_failed_to_cancelled(
            "slow_scan",
            "run aborted before this rule finished".into()
        ));
        assert!(!ck.failed_rules.contains("slow_scan"));
        let r = &ck.rule_runs["slow_scan"];
        assert_eq!(r.status.as_deref(), Some("cancelled"));
        assert!(r.stderr_tail.is_none() || r.signal == Some(15));

        // Exit-code failure → kept.
        let mut ck = mk(base(crate::executor::JobStatus::Failed, Some(1), None));
        assert!(!ck.demote_failed_to_cancelled("slow_scan", "run aborted".into()));
        assert!(ck.failed_rules.contains("slow_scan"));

        // Legacy record (no status/signal fields) → kept, never demoted.
        let mut ck = CheckpointState::default();
        ck.mark_failed("legacy");
        ck.rule_runs.insert(
            "legacy".into(),
            RuleRunRecord {
                exit_code: None,
                command: None,
                stderr_tail: None,
                stdout_tail: None,
                caption: None,
                status: None,
                skip_reason: None,
                signal: None,
            },
        );
        assert!(!ck.demote_failed_to_cancelled("legacy", "run aborted".into()));
        assert!(ck.failed_rules.contains("legacy"));
    }

    #[test]
    fn ancient_inputs_never_trigger_reexecution() {
        // Issue #469: an `ancient` input is excluded from the mtime
        // comparison — even strictly NEWER than the output, it must not
        // force re-execution; without the declaration it must.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("reads.fq"), b"READS").unwrap();
        std::fs::write(dir.path().join("ref.fa"), b"REF").unwrap();
        std::fs::write(dir.path().join("out.txt"), b"OUT").unwrap();
        // Pin explicit filetimes (issue #249 family): write order is not
        // enough on filesystems with coarse mtime granularity — equal
        // mtimes make reads.fq look "not older" than the output. T0 <
        // out < ref (ref is the ancient input).
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        for (name, offset) in [("reads.fq", -120i64), ("out.txt", -60), ("ref.fa", 60)] {
            filetime::set_file_mtime(
                dir.path().join(name),
                filetime::FileTime::from_unix_time(t0 + offset, 0),
            )
            .unwrap();
        }
        let mk = |ancient: Vec<String>| crate::rule::Rule {
            name: "align".into(),
            input: vec!["ref.fa".to_string(), "reads.fq".to_string()].into(),
            output: vec!["out.txt".to_string()].into(),
            ancient,
            ..Default::default()
        };
        let values = HashMap::new();

        assert!(
            should_skip_rule_with_checksums(&mk(vec!["ref.fa".into()]), dir.path(), &values, None),
            "an ancient input must never trigger re-execution"
        );
        assert!(
            !should_skip_rule_with_checksums(&mk(vec![]), dir.path(), &values, None),
            "without the ancient declaration the fresh mtime forces re-execution"
        );

        // Manifest: the ancient input is not tracked; the regular one is.
        let rule = mk(vec!["ref.fa".into()]);
        let manifest = snapshot_input_manifest(
            &rule,
            dir.path(),
            &values,
            &crate::storage::StorageResolver::with_local(),
        )
        .unwrap()
        .expect("manifest exists for the regular input");
        assert!(
            manifest.iter().any(|e| e.path == "reads.fq"),
            "regular inputs stay tracked: {manifest:?}"
        );
        assert!(
            !manifest.iter().any(|e| e.path == "ref.fa"),
            "ancient inputs must be excluded from the input manifest: {manifest:?}"
        );
    }

    #[test]
    fn ancient_template_matches_expanded_input() {
        // An ancient declaration carrying an engine wildcard matches the
        // expanded input structurally.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("refs")).unwrap();
        std::fs::write(dir.path().join("refs/hg38.fa"), b"R").unwrap();
        std::fs::write(dir.path().join("out.txt"), b"O").unwrap();
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        filetime::set_file_mtime(
            dir.path().join("refs/hg38.fa"),
            filetime::FileTime::from_unix_time(t0 + 60, 0),
        )
        .unwrap();
        filetime::set_file_mtime(
            dir.path().join("out.txt"),
            filetime::FileTime::from_unix_time(t0 - 60, 0),
        )
        .unwrap();
        let rule = crate::rule::Rule {
            name: "align".into(),
            input: vec!["refs/hg38.fa".to_string()].into(),
            output: vec!["out.txt".to_string()].into(),
            ancient: vec!["refs/{build}.fa".to_string()],
            ..Default::default()
        };
        assert!(
            should_skip_rule_with_checksums(&rule, dir.path(), &HashMap::new(), None),
            "an ancient template declaration must cover its expanded input"
        );
    }

    #[test]
    fn checkpoint_json_is_insertion_order_independent() {
        // checkpoint.json is the provenance/audit artifact (resume,
        // `provenance verify`): identical state must serialize to identical
        // bytes regardless of the order keys were inserted in — which with
        // HashMap/HashSet fields used to vary per process and defeat
        // byte-level diffs between runs. BTreeMap/BTreeSet fields pin the
        // order to sorted.
        let build = |order: &[&str]| -> CheckpointState {
            let mut ck = CheckpointState::default();
            for rule in order {
                ck.completed_rules.insert(rule.to_string());
                ck.checksums
                    .insert(format!("out/{rule}.txt"), format!("sha256:{rule}"));
                ck.config_snapshot
                    .insert(format!("k_{rule}"), rule.to_string());
            }
            ck
        };
        let forward = build(&["r1", "r2", "r3"]);
        let backward = build(&["r3", "r2", "r1"]);
        assert_eq!(
            forward.to_json().unwrap(),
            backward.to_json().unwrap(),
            "identical checkpoint state must serialize identically regardless of insertion order"
        );
    }

    #[test]
    fn output_pattern_domains_serialize_canonically() {
        // #528: each domain combo is a HashMap — its key order is
        // per-process random, and the combo list followed discovery order.
        // Identical state must still produce identical checkpoint bytes,
        // whatever order the keys were inserted in and whichever combo was
        // discovered first.
        use crate::wildcard::WildcardValues;
        let build = |first: bool| -> CheckpointState {
            let mut ck = CheckpointState::default();
            let mut combo_a = WildcardValues::new();
            combo_a.insert("build".to_string(), "37".to_string());
            combo_a.insert("part".to_string(), "1".to_string());
            let mut combo_b = WildcardValues::new();
            combo_b.insert("part".to_string(), "2".to_string());
            combo_b.insert("build".to_string(), "38".to_string());
            let combos = if first {
                vec![combo_a, combo_b]
            } else {
                vec![combo_b, combo_a]
            };
            ck.record_output_pattern_domain("idx/{build}/{part}.bt2", combos);
            ck
        };
        let forward = build(true);
        let backward = build(false);
        assert_eq!(
            forward.to_json().unwrap(),
            backward.to_json().unwrap(),
            "identical domains must serialize identically regardless of HashMap order"
        );
        // Round-trip: resume reads the canonical form back into HashMaps.
        let loaded = CheckpointState::from_json(&forward.to_json().unwrap()).unwrap();
        let combos = loaded
            .output_pattern_domains
            .get("idx/{build}/{part}.bt2")
            .unwrap();
        assert_eq!(combos.len(), 2);
        assert!(
            combos
                .iter()
                .any(|c| c.get("build").map(String::as_str) == Some("37"))
        );
        assert!(
            combos
                .iter()
                .any(|c| c.get("part").map(String::as_str) == Some("2"))
        );
    }

    #[test]
    fn checkpoint_save_is_atomic_and_leaves_no_tmp() {
        // issue #194 A1: the save goes through a sibling tmp + rename, so a
        // failed save never leaves a partial tmp behind and a successful one
        // yields a fully-parseable document.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.json");
        let mut ck = CheckpointState::default();
        ck.completed_rules.insert("rule_a".to_string());
        ck.save_to_file(&path).unwrap();
        assert!(path.exists());
        assert!(!dir_has_staged_tmp(dir.path()));
        let loaded = CheckpointState::load_from_file(&path).unwrap();
        assert!(loaded.completed_rules.contains("rule_a"));
        // A second save overwrites cleanly.
        ck.completed_rules.insert("rule_b".to_string());
        ck.save_to_file(&path).unwrap();
        let loaded = CheckpointState::load_from_file(&path).unwrap();
        assert!(loaded.completed_rules.contains("rule_b"));
        assert!(!dir_has_staged_tmp(dir.path()));
    }

    /// Any leftover `*.tmp` sibling (staged names are unique per call now,
    /// so the no-litter assertion must scan the directory, not one name).
    fn dir_has_staged_tmp(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            })
            .unwrap_or(false)
    }

    #[test]
    fn checkpoint_save_failure_cleans_tmp() {
        // Rename fails when the target path is a directory: the error must
        // surface AND the tmp sibling must be removed (no litter).
        let dir = tempfile::tempdir().unwrap();
        let target_dir = dir.path().join("checkpoint.json");
        std::fs::create_dir(&target_dir).unwrap();
        let ck = CheckpointState::default();
        assert!(ck.save_to_file(&target_dir).is_err());
        assert!(!dir_has_staged_tmp(dir.path()));
    }

    #[test]
    fn checkpoint_staged_names_are_unique_per_call() {
        // Concurrent saves (rule completion racing an abort-path save) must
        // never share a staged file: each call gets a fresh name, so one
        // save's rename cannot yank another's staged file (live ENOENT seen
        // on the abort path with the old fixed `.tmp` name).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.json");
        let a = unique_staged_path(&path);
        let b = unique_staged_path(&path);
        assert_ne!(a, b, "staged names must be unique within a process");
        assert!(a.starts_with(dir.path()));
        assert!(a.to_string_lossy().ends_with(".tmp"));
    }

    #[test]
    fn checksum_aware_skip_uses_content_identity_over_mtime() {
        // issue #194 B2: with a recorded checksum for every output, a fresh
        // mtime alone must NOT decide reuse — matching content skips even
        // when the input is newer (touch), diverging content re-executes
        // even when mtimes look fresh.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("in.txt"), "input").unwrap();
        std::fs::write(dir.path().join("out.txt"), "content-v1").unwrap();
        let rule = crate::rule::Rule {
            name: "r".to_string(),
            input: vec!["in.txt".to_string()].into(),
            output: vec!["out.txt".to_string()].into(),
            ..Default::default()
        };
        let recorded: BTreeMap<String, String> = [(
            "out.txt".to_string(),
            format!("sha256:{}", sha256_hex(b"content-v1")),
        )]
        .into_iter()
        .collect();

        // Input made NEWER than the output (mtime path would re-execute):
        // the recorded checksum matches, so the skip still holds.
        filetime_touch_newer(dir.path().join("in.txt"));
        assert!(
            should_skip_rule_with_checksums(&rule, dir.path(), &HashMap::new(), Some(&recorded)),
            "matching content must skip even when the input mtime is newer"
        );

        // Content diverged (simulated rewrite): even fresh mtimes must
        // re-execute.
        std::fs::write(dir.path().join("out.txt"), "content-v2").unwrap();
        assert!(
            !should_skip_rule_with_checksums(&rule, dir.path(), &HashMap::new(), Some(&recorded)),
            "diverging content must re-execute despite fresh-looking mtime"
        );
    }

    /// SHA-256 hex for the test above (the production hash is
    /// `compute_file_checksum`; hashing bytes directly keeps the test free
    /// of file-format coupling).
    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(bytes);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Bump `path`'s mtime forward one second (test helper; production
    /// never mutates mtimes).
    fn filetime_touch_newer(path: std::path::PathBuf) {
        let md = std::fs::metadata(&path).unwrap();
        let modified = md.modified().unwrap();
        let future = modified + std::time::Duration::from_secs(2);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(future)
            .unwrap();
    }

    /// A character device output must never read as up to date (audit B4):
    /// its mtime is effectively constant, so the mtime comparison would skip
    /// the rule forever regardless of what the rule was supposed to produce.
    /// `/dev/null` is the only portable char device a test can declare.
    #[cfg(unix)]
    #[test]
    fn character_device_output_is_never_fresh() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("in.txt"), "input").unwrap();
        let rule = crate::rule::Rule {
            name: "devnull".to_string(),
            input: vec!["in.txt".to_string()].into(),
            output: vec!["/dev/null".to_string()].into(),
            ..Default::default()
        };
        assert!(
            !should_skip_rule_with_checksums(&rule, dir.path(), &HashMap::new(), None),
            "a char-device output must be re-executed, never skipped as up to date"
        );
        // A regular output under the same inputs stays skippable: the gate is
        // about the file TYPE, not the absolute path.
        let regular = crate::rule::Rule {
            name: "regular".to_string(),
            input: vec!["in.txt".to_string()].into(),
            output: vec!["out.txt".to_string()].into(),
            ..Default::default()
        };
        std::fs::write(dir.path().join("out.txt"), "out").unwrap();
        // Pin explicit filetimes: a fresh tempdir can host both writes inside
        // one filesystem timestamp tick, and equal mtimes read as "not
        // fresher than input" — an mtime-comparison flake, not a gate bug
        // (issue #249 family). in.txt strictly older than out.txt.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        filetime::set_file_mtime(
            dir.path().join("in.txt"),
            filetime::FileTime::from_unix_time(now.as_secs() as i64 - 60, 0),
        )
        .unwrap();
        filetime::set_file_mtime(
            dir.path().join("out.txt"),
            filetime::FileTime::from_unix_time(now.as_secs() as i64, 0),
        )
        .unwrap();
        assert!(
            should_skip_rule_with_checksums(&regular, dir.path(), &HashMap::new(), None),
            "a regular file output with fresh mtime must still skip"
        );
    }

    /// A checkpoint written by another workflow must be reported as foreign
    /// (audit B8) — even when the two workflows share rule names and output
    /// paths, their artifacts must not be reused — while a checkpoint from
    /// the same workflow (same path, different spelling) is not.
    #[test]
    fn foreign_workflow_detection_and_reuse_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        let wf_a = dir.path().join("a.oxoflow");
        let wf_b = dir.path().join("b.oxoflow");
        std::fs::write(&wf_a, "").unwrap();
        std::fs::write(&wf_b, "").unwrap();

        let mut ck = CheckpointState::new();
        // No recorded path: legacy checkpoint, caller keeps the old behavior.
        assert_eq!(ck.foreign_workflow(&wf_a), None);

        ck.set_workflow_path(&wf_a);
        ck.completed_rules.insert("fastqc".to_string());
        ck.checksums
            .insert("out.txt".to_string(), "sha256:deadbeef".to_string());
        ck.rule_fingerprints
            .insert("fastqc".to_string(), "sha256:01".to_string());
        assert_eq!(ck.foreign_workflow(&wf_a), Some(false));
        assert_eq!(ck.foreign_workflow(&wf_b), Some(true));

        // Dropping the reuse records leaves nothing to reuse.
        ck.invalidate_reuse_records();
        assert!(ck.completed_rules.is_empty());
        assert!(ck.checksums.is_empty());
        assert!(ck.rule_fingerprints.is_empty());
    }

    /// In-memory cloud backend with a mutable etag per key — the semantic
    /// proof for issue #78 P2 (same-size remote rewrites invalidate).
    struct FakeCloudStorage {
        etags: Arc<Mutex<HashMap<String, String>>>,
    }

    #[async_trait::async_trait]
    impl StorageBackend for FakeCloudStorage {
        async fn exists(&self, path: &StoragePath) -> Result<bool> {
            Ok(self.etags.lock().unwrap().contains_key(&path.raw))
        }

        async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>> {
            let etag = self.etags.lock().unwrap().get(&path.raw).cloned();
            Ok(etag.map(|e| RemoteStat {
                size: 100,
                etag: Some(e),
            }))
        }

        async fn read_to_string(&self, _path: &StoragePath) -> Result<String> {
            Ok(String::new())
        }

        async fn write(&self, _path: &StoragePath, _data: &[u8]) -> Result<()> {
            Ok(())
        }

        async fn stage(&self, _path: &StoragePath, _workdir: &Path) -> Result<PathBuf> {
            Ok(PathBuf::new())
        }

        async fn upload(&self, _local: &Path, _remote: &StoragePath) -> Result<()> {
            Ok(())
        }

        fn name(&self) -> &'static str {
            "fake-cloud"
        }
    }

    fn remote_rule() -> Rule {
        Rule {
            name: "remote-consumer".to_string(),
            input: FilePatterns::List(vec!["s3://bucket/key".to_string()]),
            output: FilePatterns::List(vec!["out.txt".to_string()]),
            shell: Some("true".to_string()),
            ..Default::default()
        }
    }

    fn resolver_with_fake(fake: Arc<FakeCloudStorage>) -> StorageResolver {
        let mut resolver = StorageResolver::with_local();
        resolver.add_backend(StorageScheme::S3, fake);
        resolver
    }

    fn snapshot_remote(rule: &Rule, resolver: &StorageResolver) -> Option<InputManifest> {
        snapshot_input_manifest(rule, Path::new("."), &HashMap::new(), resolver).unwrap()
    }

    #[tokio::test]
    async fn same_size_etag_change_invalidates_remote_input() {
        let fake = Arc::new(FakeCloudStorage {
            etags: Arc::new(Mutex::new(HashMap::from([(
                "s3://bucket/key".to_string(),
                "v1".to_string(),
            )]))),
        });
        let resolver = resolver_with_fake(fake.clone());
        let rule = remote_rule();

        let recorded = snapshot_remote(&rule, &resolver).expect("remote input snapshots");
        assert_eq!(recorded.len(), 1);
        let remote = recorded[0].remote.as_ref().expect("remote entry recorded");
        assert_eq!(remote.scheme, "s3");
        assert_eq!(remote.etag.as_deref(), Some("v1"));

        // Same size, new etag → invalidated (the exact issue #78 P2 scenario).
        *fake
            .etags
            .lock()
            .unwrap()
            .get_mut("s3://bucket/key")
            .unwrap() = "v2".to_string();
        let current = snapshot_remote(&rule, &resolver).unwrap();
        assert!(!manifests_match(&recorded, &current));

        // Unchanged etag → still matches.
        let again = snapshot_remote(&rule, &resolver).unwrap();
        assert!(manifests_match(&current, &again));
    }

    fn entry(remote: Option<RemoteManifestEntry>) -> InputManifestEntry {
        InputManifestEntry {
            path: "k".to_string(),
            size: 100,
            mtime_nanos: 0,
            hash: None,
            remote,
        }
    }

    fn remote_entry(scheme: &str, size: u64, etag: Option<&str>) -> Option<RemoteManifestEntry> {
        Some(RemoteManifestEntry {
            scheme: scheme.to_string(),
            key: "s3://b/k".to_string(),
            size,
            etag: etag.map(str::to_string),
        })
    }

    #[test]
    fn manifests_match_remote_matrix() {
        let r = remote_entry("s3", 100, Some("a"));
        // etag equal → match
        assert!(manifests_match(
            &[entry(remote_entry("s3", 100, Some("a")))],
            &[entry(remote_entry("s3", 100, Some("a")))]
        ));
        // etag differs → mismatch
        assert!(!manifests_match(
            &[entry(remote_entry("s3", 100, Some("a")))],
            &[entry(remote_entry("s3", 100, Some("b")))]
        ));
        // size differs → mismatch
        assert!(!manifests_match(
            &[entry(remote_entry("s3", 100, Some("a")))],
            &[entry(remote_entry("s3", 200, Some("a")))]
        ));
        // scheme differs → mismatch
        assert!(!manifests_match(
            &[entry(remote_entry("s3", 100, Some("a")))],
            &[entry(remote_entry("gs", 100, Some("a")))]
        ));
        // etag unavailable on both sides → size decides
        assert!(manifests_match(
            &[entry(remote_entry("s3", 100, None))],
            &[entry(remote_entry("s3", 100, None))]
        ));
        assert!(!manifests_match(
            &[entry(remote_entry("s3", 100, None))],
            &[entry(remote_entry("s3", 200, None))]
        ));
        // local vs remote → mismatch
        assert!(!manifests_match(
            &[entry(None)],
            &[entry(remote_entry("s3", 100, Some("a")))]
        ));
        assert!(!manifests_match(
            &[entry(remote_entry("s3", 100, Some("a")))],
            &[entry(None)]
        ));
        // local vs local unchanged behaviour
        let local = entry(None);
        assert!(manifests_match(
            std::slice::from_ref(&local),
            std::slice::from_ref(&local)
        ));
        let _ = r;
    }

    fn small_file_manifest(dir: &Path, name: &str, content: &[u8]) -> InputManifest {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        let rule = Rule {
            name: "r".to_string(),
            input: FilePatterns::List(vec![name.to_string()]),
            ..Default::default()
        };
        snapshot_input_manifest(
            &rule,
            dir,
            &Default::default(),
            &StorageResolver::with_local(),
        )
        .unwrap()
        .expect("small file input snapshots")
    }

    #[test]
    fn manifest_snapshot_hashes_small_files() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = small_file_manifest(dir.path(), "in.txt", b"data");
        let entry = &manifest[0];
        assert_eq!(entry.size, 4);
        let hash = entry
            .hash
            .as_deref()
            .expect("small files get content hashes");
        assert!(hash.starts_with("sha256:"), "{hash}");
    }

    #[test]
    fn manifest_snapshot_skips_hash_for_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.bam");
        // Sparse file: declares the size without writing 64 MiB.
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MANIFEST_HASH_MAX_BYTES + 1)
            .unwrap();
        let rule = Rule {
            name: "r".to_string(),
            input: FilePatterns::List(vec!["big.bam".to_string()]),
            ..Default::default()
        };
        let manifest = snapshot_input_manifest(
            &rule,
            dir.path(),
            &Default::default(),
            &StorageResolver::with_local(),
        )
        .unwrap()
        .unwrap();
        assert!(
            manifest[0].hash.is_none(),
            "files above the threshold keep the size+mtime policy"
        );
    }

    #[test]
    fn manifests_match_detects_same_size_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let before = small_file_manifest(dir.path(), "in.txt", b"aaaa");
        std::fs::write(dir.path().join("in.txt"), b"bbbb").unwrap();
        let after = small_file_manifest(dir.path(), "in.txt", b"bbbb");

        assert!(manifests_match(&before, &before));
        assert!(
            !manifests_match(&before, &after),
            "same size + different content must invalidate (hash policy)"
        );
    }

    #[test]
    fn manifests_match_legacy_entries_keep_size_mtime_policy() {
        // Pre-hash checkpoints have entries without hashes: they compare
        // size+mtime against a fresh snapshot (which now carries hashes)
        // instead of invalidating everything once.
        let recorded = vec![InputManifestEntry {
            path: "in.txt".to_string(),
            size: 4,
            mtime_nanos: 42,
            hash: None,
            remote: None,
        }];
        let current = vec![InputManifestEntry {
            path: "in.txt".to_string(),
            size: 4,
            mtime_nanos: 42,
            hash: Some("sha256:abc".to_string()),
            remote: None,
        }];
        assert!(manifests_match(&recorded, &current));

        let current_changed = vec![InputManifestEntry {
            path: "in.txt".to_string(),
            size: 5,
            mtime_nanos: 42,
            hash: Some("sha256:abc".to_string()),
            remote: None,
        }];
        assert!(!manifests_match(&recorded, &current_changed));
    }

    #[test]
    fn manifests_match_hash_wins_over_mtime() {
        let recorded = vec![InputManifestEntry {
            path: "in.txt".to_string(),
            size: 4,
            mtime_nanos: 1,
            hash: Some("sha256:abc".to_string()),
            remote: None,
        }];
        let current = vec![InputManifestEntry {
            path: "in.txt".to_string(),
            size: 4,
            mtime_nanos: 999,
            hash: Some("sha256:abc".to_string()),
            remote: None,
        }];
        // Content identical: an mtime-only touch no longer invalidates.
        assert!(manifests_match(&recorded, &current));
    }

    #[test]
    fn manifest_changes_lists_added_removed_and_changed_paths() {
        fn local(path: &str, size: u64, mtime: i128, hash: Option<&str>) -> InputManifestEntry {
            InputManifestEntry {
                path: path.to_string(),
                size,
                mtime_nanos: mtime,
                hash: hash.map(str::to_string),
                remote: None,
            }
        }
        let recorded = vec![
            local("a.txt", 10, 100, Some("sha256:aaa")),
            local("b.txt", 20, 200, Some("sha256:bbb")),
            local("gone.txt", 5, 50, None),
        ];
        let current = vec![
            // mtime moved but the content hash matches — not a change.
            local("a.txt", 10, 999, Some("sha256:aaa")),
            // Same path, different content — a real change.
            local("b.txt", 20, 200, Some("sha256:CHANGED")),
            local("new.txt", 1, 1, None),
        ];
        let changes = manifest_changes(&recorded, &current);
        assert_eq!(
            changes,
            vec!["b.txt (changed)", "gone.txt (removed)", "new.txt (added)"]
        );
        assert!(!manifests_match(&recorded, &current));
    }

    #[tokio::test]
    async fn cleanup_transform_chunks_removes_chunk_files_and_empty_dirs() {
        let workdir = std::env::temp_dir().join(format!("oxo-cleanup-test-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&workdir).await;
        tokio::fs::create_dir_all(workdir.join(".oxo-flow/chunks/chr"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(workdir.join(".oxo-flow/chunks/sample"))
            .await
            .unwrap();

        let rule = Rule {
            name: "variant_calling_combine".to_string(),
            input: FilePatterns::List(vec![
                ".oxo-flow/chunks/chr/chr1.g.vcf.gz".to_string(),
                ".oxo-flow/chunks/chr/chr2.g.vcf.gz".to_string(),
            ]),
            cleanup_chunks: true,
            ..Default::default()
        };

        // Chunk files owned by the combine rule
        tokio::fs::write(workdir.join(".oxo-flow/chunks/chr/chr1.g.vcf.gz"), b"x")
            .await
            .unwrap();
        tokio::fs::write(workdir.join(".oxo-flow/chunks/chr/chr2.g.vcf.gz"), b"x")
            .await
            .unwrap();
        // Unrelated chunk from another rule keeps the chunks dir alive
        tokio::fs::write(workdir.join(".oxo-flow/chunks/sample/keep.out"), b"x")
            .await
            .unwrap();

        let deleted = cleanup_transform_chunks(&rule, &workdir).await;
        // The returned paths feed the caller's checksum migration
        // (issue #315 F2): deletion and record-move happen together.
        assert_eq!(deleted.len(), 2);

        assert!(!workdir.join(".oxo-flow/chunks/chr/chr1.g.vcf.gz").exists());
        assert!(!workdir.join(".oxo-flow/chunks/chr/chr2.g.vcf.gz").exists());
        // The {by} directory became empty and was removed
        assert!(!workdir.join(".oxo-flow/chunks/chr").exists());
        // Unrelated files and their directories are untouched
        assert!(workdir.join(".oxo-flow/chunks/sample/keep.out").exists());
        assert!(workdir.join(".oxo-flow/chunks").exists());

        let _ = tokio::fs::remove_dir_all(&workdir).await;
    }

    #[test]
    fn cleaned_checksums_roundtrip_and_legacy_default() {
        // issue #315 F2: cleaned chunk checksums serialize separately from
        // normal checksums, and legacy checkpoints without the field read
        // back empty (old behavior: chunks treated as ordinary outputs).
        let mut state = CheckpointState::new();
        state.record_cleaned_checksum(".oxo-flow/chunks/chr/chr1.out", "sha256:aaaa".into());

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("cleaned_checksums"));
        assert!(!json.contains("\"checksums\""));

        let back: CheckpointState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cleaned_checksums.len(), 1);
        assert!(back.checksums.is_empty());

        let legacy: CheckpointState = serde_json::from_str(
            r#"{"completed_rules":[],"failed_rules":[],"benchmarks":{},"checksums":{"a":"sha256:bb"}}"#,
        )
        .unwrap();
        assert!(legacy.cleaned_checksums.is_empty());
        assert_eq!(legacy.checksums.len(), 1);
    }

    #[test]
    fn checkpoint_roundtrip_preserves_workdir() {
        // issue #68: resume must re-run from the same working directory the
        // original run used, or completed rules are misjudged as stale.
        let mut state = CheckpointState::new();
        state.set_workflow_path(std::path::Path::new("/wf/p.oxoflow"));
        state.set_workdir(std::path::Path::new("/custom/wd"));
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("workdir"));
        let loaded: CheckpointState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.workdir.as_deref(), Some("/custom/wd"));
    }

    #[test]
    fn legacy_checkpoint_without_workdir_still_loads() {
        // Older checkpoints have no workdir field — deserialization must
        // not break, and resume falls back to the workflow's directory.
        let json = r#"{"completed_rules":[],"failed_rules":[],"benchmarks":{},"workflow_path":"/wf/p.oxoflow"}"#;
        let loaded: CheckpointState = serde_json::from_str(json).unwrap();
        assert_eq!(loaded.workdir, None);
        assert_eq!(loaded.workflow_path.as_deref(), Some("/wf/p.oxoflow"));
    }

    #[test]
    fn running_set_roundtrip_and_mutator_cleanup() {
        // issue #685: the running set is written at spawn time so a crashed
        // run leaves an honest record. Every terminal mutator must clear
        // the entry — a stale running mark must never survive a completed
        // or failed transition.
        let mut ck = CheckpointState::new();
        ck.mark_running("trim_S1");
        ck.mark_running("align_S1");
        assert!(ck.is_running("trim_S1"));
        assert_eq!(ck.running_rules().len(), 2);

        let json = serde_json::to_string(&ck).unwrap();
        assert!(json.contains("\"running\""));
        let loaded: CheckpointState = serde_json::from_str(&json).unwrap();
        assert!(loaded.is_running("align_S1"));

        // Terminal transitions remove the running mark.
        ck.mark_completed_quiet("trim_S1");
        assert!(!ck.is_running("trim_S1"));
        ck.mark_failed("align_S1");
        assert!(!ck.is_running("align_S1"));
        assert!(ck.running_rules().is_empty());

        // mark_completed (full variant) clears it too, and clear_running is
        // available for the resume-time stale cleanup.
        ck.mark_running("qc_S1");
        ck.mark_completed(
            "qc_S1",
            BenchmarkRecord {
                rule: "qc_S1".into(),
                wall_time_secs: 1.0,
                max_memory_mb: None,
                memory_limit_mb: None,
                cpu_seconds: None,
                retries: 0,
                recorded_as: None,
            },
        );
        assert!(!ck.is_running("qc_S1"));
        ck.mark_running("qc_S1");
        ck.clear_running("qc_S1");
        assert!(!ck.is_running("qc_S1"));
    }

    #[test]
    fn running_set_skipped_when_empty_and_legacy_loads_empty() {
        // Empty running set stays out of the persisted JSON (smaller
        // artifacts, order-independent persistence invariant), and legacy
        // checkpoints without the field deserialize to an empty set.
        let mut state = CheckpointState::new();
        state.mark_running("a");
        state.clear_running("a");
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("\"running\""));

        let legacy: CheckpointState =
            serde_json::from_str(r#"{"completed_rules":["x"],"failed_rules":[],"benchmarks":{}}"#)
                .unwrap();
        assert!(legacy.running_rules().is_empty());
    }

    #[test]
    fn tombstones_roundtrip_and_legacy_checkpoints_load_empty() {
        let mut state = CheckpointState::new();
        state
            .tombstones
            .insert("trim_S1".to_string(), vec!["trimmed/S1.fq".to_string()]);
        let json = serde_json::to_string(&state).unwrap();
        let loaded: CheckpointState = serde_json::from_str(&json).unwrap();
        assert_eq!(
            loaded.tombstones.get("trim_S1").map(Vec::as_slice),
            Some(&["trimmed/S1.fq".to_string()][..])
        );

        // Pre-tombstone checkpoints load with an empty map.
        let legacy: CheckpointState = serde_json::from_str(
            r#"{"completed_rules":[],"failed_rules":[],"benchmarks":{},"workflow_path":"/wf/p.oxoflow"}"#,
        )
        .unwrap();
        assert!(legacy.tombstones.is_empty());
        assert!(legacy.reentries.is_empty());
    }

    #[test]
    fn checkpoint_reentry_roundtrip_and_supersede() {
        let mut ck = CheckpointState::new();
        ck.record_reentry(crate::reentry::ReentryRecord {
            round: 1,
            rule: "discover".into(),
            group: None,
            samples: vec!["S2".into()],
            pairs: vec![],
        });
        ck.record_reentry(crate::reentry::ReentryRecord {
            round: 2,
            rule: "discover".into(),
            group: None,
            samples: vec!["S2".into(), "S3".into()],
            pairs: vec![],
        });
        // Same rule → superseded, not appended.
        assert_eq!(ck.reentries.len(), 1);
        assert_eq!(ck.reentries[0].samples, vec!["S2", "S3"]);

        let json = ck.to_json().unwrap();
        let back = CheckpointState::from_json(&json).unwrap();
        assert_eq!(back.reentries, ck.reentries);
    }

    // ─── Input manifest snapshots (issue #72) ─────────────────────────────

    fn temp_workdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oxo-manifest-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(workdir: &Path, rel: &str, content: &str) {
        let path = workdir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn list_rule(name: &str, inputs: &[&str]) -> Rule {
        Rule {
            name: name.to_string(),
            input: FilePatterns::List(inputs.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    fn snapshot(rule: &Rule, workdir: &Path) -> Option<InputManifest> {
        snapshot_input_manifest(
            rule,
            workdir,
            &HashMap::new(),
            &StorageResolver::with_local(),
        )
        .unwrap()
    }

    #[test]
    fn manifest_plain_file_records_path_size_and_mtime() {
        let wd = temp_workdir("plain");
        write_file(&wd, "data/a.txt", "hello");
        let rule = list_rule("r", &["data/a.txt"]);
        let manifest = snapshot(&rule, &wd).expect("plain input is trackable");
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].path, "data/a.txt");
        assert_eq!(manifest[0].size, 5);
        assert!(manifest[0].mtime_nanos > 0);
        let _ = std::fs::remove_dir_all(&wd);
    }

    /// A writer-less FIFO (STAR's leftover `tmp.fifo.read*` after a killed
    /// run, issue #695) must not hang the manifest snapshot: opening it to
    /// hash would block forever, so the entry is recorded with size+mtime
    /// only (no hash) — same policy as a large file.
    #[cfg(unix)]
    #[test]
    fn manifest_snapshot_completes_with_fifo_input() {
        let wd = temp_workdir("fifo");
        std::fs::create_dir_all(wd.join("STARtmp")).unwrap();
        let fifo = wd.join("STARtmp/tmp.fifo.read1");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must exist on unix");
        assert!(status.success(), "mkfifo failed");
        let rule = list_rule("star_align", &["STARtmp"]);

        // Would hang forever before the fix (compute_file_checksum opened
        // the FIFO and blocked in wait_for_partner). Must now complete.
        let manifest = snapshot(&rule, &wd).expect("dir input with FIFO is trackable");
        let entry = manifest
            .iter()
            .find(|e| e.path == "STARtmp/tmp.fifo.read1")
            .expect("FIFO is recorded in the manifest");
        assert!(entry.hash.is_none(), "FIFO must not be content-hashed");

        // Direct hashing of the FIFO refuses instead of blocking.
        assert!(
            compute_file_checksum(&fifo).is_err(),
            "checksum on a writer-less FIFO must error, never open it"
        );
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn missing_input_patterns_lists_only_unresolvable_patterns() {
        let wd = temp_workdir("missing");
        write_file(&wd, "data/a.txt", "a");
        let rule = list_rule("r", &["data/a.txt", "data/missing.txt"]);
        assert_eq!(
            missing_input_patterns(&rule, &wd, &HashMap::new()),
            vec!["data/missing.txt".to_string()]
        );
        // Engine wildcards and chunk paths are skipped, like the snapshot walk.
        let wildcard_rule = list_rule("w", &["{sample}.fq", ".oxo-flow/chunks/x.bam"]);
        assert!(missing_input_patterns(&wildcard_rule, &wd, &HashMap::new()).is_empty());
        // Remote and ancient inputs are never locally missing (audit #649):
        // the snapshot resolves remote objects via HEAD (never local stats —
        // a workdir.join("s3://…") always fails) and exempts ancient inputs
        // from invalidation entirely (issue #469).
        let remote_rule = list_rule("r2", &["s3://bucket/ref.fq", "data/missing.txt"]);
        assert_eq!(
            missing_input_patterns(&remote_rule, &wd, &HashMap::new()),
            vec!["data/missing.txt".to_string()],
            "a remote URI must not appear as a phantom local missing pattern"
        );
        let mut ancient_rule = list_rule("r3", &["data/missing.txt", "data/also-missing.txt"]);
        ancient_rule.ancient = vec!["data/missing.txt".to_string()];
        assert_eq!(
            missing_input_patterns(&ancient_rule, &wd, &HashMap::new()),
            vec!["data/also-missing.txt".to_string()],
            "an ancient input is exempt from missing-input reporting"
        );
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn baked_absent_expand_inputs_entry_is_tolerated() {
        // Issue #757: a missing expand_inputs-baked literal (its producer
        // is gated off under the active config) must not poison the whole
        // snapshot — record the existing subset. WITHOUT the baked flag
        // the same missing literal still hard-errors (genuine invalidation
        // for non-baken inputs).
        let wd = temp_workdir("baked-manifest");
        write_file(&wd, "data/a.txt", "a");
        let resolver = crate::storage::StorageResolver::new();

        let mut rule = list_rule(
            "r",
            &["data/a.txt", "results/star_salmon/log/S1.bowtie2.log"],
        );
        let snapshot = snapshot_input_manifest(&rule, &wd, &HashMap::new(), &resolver);
        assert!(snapshot.is_err(), "non-baken missing input must error");

        rule.expand_inputs_baked
            .insert("results/star_salmon/log/S1.bowtie2.log".to_string());
        let snapshot =
            snapshot_input_manifest(&rule, &wd, &HashMap::new(), &resolver).expect("tolerated");
        let manifest = snapshot.expect("existing subset recorded");
        assert_eq!(manifest.len(), 1, "only the existing input recorded");
        assert_eq!(manifest[0].path, "data/a.txt");

        // The mirror walk reports the tolerated entry as not-missing.
        assert!(missing_input_patterns(&rule, &wd, &HashMap::new()).is_empty());
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_empty_config_input_does_not_walk_the_workdir() {
        // Live failure (rnaseq-sd get_annotation/get_genome): a rule declares
        // `input = ["{config.annotation_gtf}"]` whose default is "" for the
        // download branch. The expanded empty string reached
        // collect_pattern_entries, where `workdir.join("")` is the workdir
        // ROOT — the manifest then recorded every file in the workdir
        // (.git, logs, other rules' outputs), and any volatile change
        // cascaded into a full-DAG invalidation (121 rules → ~96-min
        // star_index rebuild).
        let wd = temp_workdir("empty-config-input");
        write_file(&wd, "some/volatile.log", "grows across runs");
        let mut wildcard_values = HashMap::new();
        wildcard_values.insert("config.annotation_gtf".to_string(), String::new());
        let rule = list_rule("get_annotation", &["{config.annotation_gtf}"]);

        let manifest =
            snapshot_input_manifest(&rule, &wd, &wildcard_values, &StorageResolver::with_local())
                .unwrap();
        // The empty pattern is not a resolvable input: no manifest at all —
        // NOT a workdir-wide entry list.
        assert!(
            manifest.is_none(),
            "empty config-optional input must not produce a workdir walk; got {} entries",
            manifest.map(|m| m.len()).unwrap_or(0)
        );
        // And it is not reported missing either (same skip rules as the
        // snapshot walk — missing_input_patterns mirrors it).
        assert!(missing_input_patterns(&rule, &wd, &wildcard_values).is_empty());

        // Contrast: a NON-empty config value still snapshots the real file.
        write_file(&wd, "refs/genes.gtf", "chr1\tsrc\texon\t1\t9\t.\t+\t.\t");
        let mut filled = HashMap::new();
        filled.insert(
            "config.annotation_gtf".to_string(),
            "refs/genes.gtf".to_string(),
        );
        let manifest =
            snapshot_input_manifest(&rule, &wd, &filled, &StorageResolver::with_local()).unwrap();
        assert_eq!(manifest.as_ref().map(|m| m.len()), Some(1));
        assert_eq!(manifest.unwrap()[0].path, "refs/genes.gtf");
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_glob_matches_only_matching_files_sorted_and_deduped() {
        let wd = temp_workdir("glob");
        write_file(&wd, "data/a.txt", "a");
        write_file(&wd, "data/b.txt", "b");
        write_file(&wd, "data/c.log", "c");
        let rule = list_rule("r", &["data/*.txt"]);
        let manifest = snapshot(&rule, &wd).unwrap();
        let paths: Vec<&str> = manifest.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["data/a.txt", "data/b.txt"]);
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_dir_input_lists_files_recursively() {
        let wd = temp_workdir("dir");
        write_file(&wd, "results/summary.txt", "s");
        write_file(&wd, "results/sub/x.log", "x");
        let rule = Rule {
            name: "r".to_string(),
            input: FilePatterns::Dir {
                path: "results".to_string(),
                pattern: None,
            },
            ..Default::default()
        };
        let manifest = snapshot(&rule, &wd).unwrap();
        let paths: Vec<&str> = manifest.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["results/sub/x.log", "results/summary.txt"]);
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_dir_pattern_filters_files() {
        let wd = temp_workdir("dirpat");
        write_file(&wd, "results/a.fastq", "a");
        write_file(&wd, "results/b.txt", "b");
        let rule = Rule {
            name: "r".to_string(),
            input: FilePatterns::Dir {
                path: "results".to_string(),
                pattern: Some("*.fastq".to_string()),
            },
            ..Default::default()
        };
        let manifest = snapshot(&rule, &wd).unwrap();
        let paths: Vec<&str> = manifest.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["results/a.fastq"]);
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_empty_inputs_return_none() {
        let wd = temp_workdir("empty");
        let rule = list_rule("r", &[]);
        assert!(snapshot(&rule, &wd).is_none());
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_engine_wildcards_return_none() {
        let wd = temp_workdir("wild");
        write_file(&wd, "data/a.txt", "a");
        // {sample} is expanded per-instance before checkpointing — the raw
        // pattern is not resolvable here and must not be globbed.
        let rule = list_rule("r", &["data/{sample}.txt"]);
        assert!(snapshot(&rule, &wd).is_none());
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_cleanup_chunks_rule_returns_none() {
        let wd = temp_workdir("cleanup");
        write_file(&wd, "x.txt", "x");
        let rule = Rule {
            name: "r".to_string(),
            input: FilePatterns::List(vec!["x.txt".to_string()]),
            cleanup_chunks: true,
            ..Default::default()
        };
        assert!(snapshot(&rule, &wd).is_none());
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_chunk_inputs_are_excluded() {
        let wd = temp_workdir("chunks");
        write_file(&wd, "src.txt", "s");
        write_file(&wd, ".oxo-flow/chunks/0/chunk1.txt", "c");
        let rule = list_rule("r", &["src.txt", ".oxo-flow/chunks/0/chunk1.txt"]);
        let manifest = snapshot(&rule, &wd).unwrap();
        let paths: Vec<&str> = manifest.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["src.txt"]);
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_missing_input_is_err() {
        let wd = temp_workdir("missing");
        let rule = list_rule("r", &["data/nope.txt"]);
        assert!(
            snapshot_input_manifest(&rule, &wd, &HashMap::new(), &StorageResolver::with_local())
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_optional_rule_records_partial_snapshot_for_absent_input() {
        // Issue #633: an optional rule (`optional = true`/`"any"`) whose
        // producer was never instantiated (input_groups matched no files /
        // endedness filter) has a permanently-absent declared input. The
        // snapshot must skip that entry and record the rest — otherwise no
        // manifest is ever written and every run fully re-runs the rule.
        let wd = temp_workdir("optional-absent");
        write_file(&wd, "data/a.txt", "a");
        let mut rule = list_rule("r", &["data/a.txt", "data/never_produced.txt"]);
        rule.optional = crate::rule::OptionalMode::Any;
        let manifest =
            snapshot_input_manifest(&rule, &wd, &HashMap::new(), &StorageResolver::with_local())
                .expect("optional rule with an absent input still snapshots");
        let paths: Vec<&str> = manifest
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(paths, ["data/a.txt"]);

        // Same shape with `optional = true`.
        let mut all_mode = list_rule("all_mode", &["data/never_produced.txt"]);
        all_mode.optional = crate::rule::OptionalMode::All(true);
        let all_manifest = snapshot_input_manifest(
            &all_mode,
            &wd,
            &HashMap::new(),
            &StorageResolver::with_local(),
        )
        .unwrap();
        assert!(all_manifest.is_some());
        assert!(all_manifest.unwrap().is_empty());

        // And the detection side still names the absent pattern so
        // tombstone-aware callers can find its (non-existent) producer.
        assert_eq!(
            missing_input_patterns(&rule, &wd, &HashMap::new()),
            vec!["data/never_produced.txt".to_string()]
        );

        // A required rule with the same absent input still errs — a missing
        // required input is genuine invalidation, not absence by design.
        let required = list_rule("req", &["data/never_produced.txt"]);
        assert!(
            snapshot_input_manifest(
                &required,
                &wd,
                &HashMap::new(),
                &StorageResolver::with_local(),
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_comparison_detects_add_remove_and_mtime_change() {
        // The issue #72 core comparison: a changed file set (or a changed
        // file) invalidates; an untouched set compares equal.
        let wd = temp_workdir("compare");
        write_file(&wd, "data/a.txt", "a");
        let rule = list_rule("r", &["data/*.txt"]);
        let baseline = snapshot(&rule, &wd).unwrap();
        assert_eq!(baseline.len(), 1);

        // Unchanged → equal.
        assert_eq!(snapshot(&rule, &wd).unwrap(), baseline);

        // Added file → different.
        write_file(&wd, "data/b.txt", "b");
        assert_ne!(snapshot(&rule, &wd).unwrap(), baseline);

        // Restore original set → equal again.
        std::fs::remove_file(wd.join("data/b.txt")).unwrap();
        assert_eq!(snapshot(&rule, &wd).unwrap(), baseline);

        // Content change bumps mtime → different.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_file(&wd, "data/a.txt", "longer content");
        assert_ne!(snapshot(&rule, &wd).unwrap(), baseline);

        // Removed file → different.
        let with_b = {
            write_file(&wd, "data/b.txt", "b");
            snapshot(&rule, &wd).unwrap()
        };
        std::fs::remove_file(wd.join("data/a.txt")).unwrap();
        assert_ne!(snapshot(&rule, &wd).unwrap(), with_b);
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_expands_config_placeholders() {
        let wd = temp_workdir("cfg");
        write_file(&wd, "out/x.txt", "x");
        let rule = list_rule("r", &["{config.results_dir}/x.txt"]);
        let mut values = HashMap::new();
        values.insert("config.results_dir".to_string(), "out".to_string());
        let manifest = snapshot_input_manifest(&rule, &wd, &values, &StorageResolver::with_local())
            .unwrap()
            .unwrap();
        assert_eq!(manifest[0].path, "out/x.txt");
        let _ = std::fs::remove_dir_all(&wd);
    }

    #[test]
    fn manifest_roundtrip_and_legacy_compat() {
        let mut state = CheckpointState::new();
        state.record_input_manifest(
            "r",
            vec![InputManifestEntry {
                path: "data/a.txt".to_string(),
                size: 7,
                mtime_nanos: 42,
                hash: None,
                remote: None,
            }],
        );
        let json = state.to_json().unwrap();
        let loaded: CheckpointState = serde_json::from_str(&json).unwrap();
        assert_eq!(
            loaded.input_manifests["r"],
            vec![InputManifestEntry {
                path: "data/a.txt".to_string(),
                size: 7,
                mtime_nanos: 42,
                hash: None,
                remote: None,
            }]
        );
        // Older checkpoints without input_manifests still load.
        let legacy = r#"{"completed_rules":["r"],"failed_rules":[],"benchmarks":{}}"#;
        let loaded: CheckpointState = serde_json::from_str(legacy).unwrap();
        assert!(loaded.input_manifests.is_empty());
    }

    #[test]
    fn workflow_git_sha_roundtrip_preserves_value() {
        let mut state = CheckpointState::new();
        state.set_workflow_git_sha("0123456789abcdef".to_string());
        let json = state.to_json().unwrap();
        let loaded: CheckpointState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.workflow_git_sha.as_deref(), Some("0123456789abcdef"));
        // A checkpoint without the field (legacy) loads as None.
        let legacy = r#"{"completed_rules":[],"failed_rules":[],"benchmarks":{}}"#;
        let loaded: CheckpointState = serde_json::from_str(legacy).unwrap();
        assert!(loaded.workflow_git_sha.is_none());
    }

    #[test]
    fn workflow_git_sha_absent_from_json_by_default() {
        // Fresh checkpoints only carry the field once a git repo is detected
        // (skip_serializing_if) — legacy consumers never see a null key.
        let json = CheckpointState::new().to_json().unwrap();
        assert!(!json.contains("workflow_git_sha"));
    }

    #[test]
    fn workflow_git_sha_resolver_returns_none_outside_git_repo() {
        let dir = std::env::temp_dir().join(format!("oxo-sha-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wf = dir.join("wf.oxoflow");
        std::fs::write(&wf, "[workflow]").unwrap();
        assert_eq!(CheckpointState::workflow_git_sha(&wf), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workflow_git_sha_resolver_finds_repo_head() {
        // oxo-flow-core's manifest sits at the workspace root, which is a git
        // repository: any path inside it must walk up to the current HEAD.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let wf = root.join("Cargo.toml");
        let sha = CheckpointState::workflow_git_sha(&wf);
        let expected = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(expected.status.success());
        let expected_sha = String::from_utf8_lossy(&expected.stdout).trim().to_string();
        assert!(!expected_sha.is_empty());
        assert_eq!(sha.as_deref(), Some(expected_sha.as_str()));
    }
}

// ─── Optional-input skipping (issue #75) ──────────────────────────────────

#[cfg(test)]
mod optional_tests {
    use super::*;

    fn rule_optional(name: &str, inputs: &[&str]) -> Rule {
        Rule {
            name: name.to_string(),
            input: FilePatterns::List(inputs.iter().map(|s| s.to_string()).collect()),
            optional: crate::rule::OptionalMode::All(true),
            ..Default::default()
        }
    }

    fn rule_optional_any(name: &str, inputs: &[&str]) -> Rule {
        Rule {
            name: name.to_string(),
            input: FilePatterns::List(inputs.iter().map(|s| s.to_string()).collect()),
            optional: crate::rule::OptionalMode::Any,
            ..Default::default()
        }
    }

    fn wd(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oxo-optional-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_plain_input_is_missing() {
        let dir = wd("plain");
        let rule = rule_optional("r", &["data/nope.txt"]);
        assert!(optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn present_plain_input_is_not_missing() {
        let dir = wd("present");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/a.txt"), "x").unwrap();
        let rule = rule_optional("r", &["data/a.txt"]);
        assert!(!optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_optional_rule_is_never_missing() {
        let dir = wd("nonopt");
        let rule = Rule {
            name: "r".to_string(),
            input: FilePatterns::List(vec!["missing.txt".to_string()]),
            optional: crate::rule::OptionalMode::All(false),
            ..Default::default()
        };
        assert!(!optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_input_missing_only_when_nothing_matches() {
        let dir = wd("glob");
        let rule = rule_optional("r", &["data/*.txt"]);
        assert!(optional_inputs_missing(&rule, &dir, &HashMap::new()));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/a.txt"), "x").unwrap();
        assert!(!optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_wildcard_input_is_assumed_present() {
        let dir = wd("engine");
        let rule = rule_optional("r", &["out/{sample}.txt"]);
        assert!(!optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_placeholder_is_expanded_before_checking() {
        let dir = wd("config");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/x.txt"), "x").unwrap();
        let rule = rule_optional("r", &["{config.datadir}/x.txt"]);
        let mut values = HashMap::new();
        values.insert("config.datadir".to_string(), "real".to_string());
        assert!(!optional_inputs_missing(&rule, &dir, &values));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── "any" mode (alternative-input pattern, issue #200) ────────────────

    #[test]
    fn all_mode_skips_when_one_of_several_inputs_is_missing() {
        // Regression guard: optional = true keeps "skip when ANY input is
        // missing" (e.g. chipseq macs3 whose control BAM may not exist).
        let dir = wd("allmulti");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/a.txt"), "x").unwrap();
        let rule = rule_optional("r", &["data/a.txt", "data/b.txt"]);
        assert!(optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn any_mode_runs_when_one_of_several_inputs_exists() {
        // live: eager's samtools_filter across mapper naming schemes —
        // only the configured mapper's BAM exists, yet the rule must run.
        let dir = wd("anyone");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/b.txt"), "x").unwrap();
        let rule = rule_optional_any("r", &["data/a.txt", "data/b.txt", "data/c.txt"]);
        assert!(!optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn any_mode_skips_when_no_input_exists() {
        let dir = wd("anynone");
        let rule = rule_optional_any("r", &["data/a.txt", "data/b.txt"]);
        assert!(optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn any_mode_with_engine_wildcard_runs() {
        let dir = wd("anyengine");
        let rule = rule_optional_any("r", &["out/{sample}.txt"]);
        assert!(!optional_inputs_missing(&rule, &dir, &HashMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_pattern_domains_roundtrip_and_replay() {
        // issue #227 item 5: discovered domains persist through the
        // checkpoint so `resume` re-instantiates consumers without
        // re-running the producer. Round-trips serde, dedups unions, and
        // legacy checkpoints load with an empty map.
        use crate::wildcard::WildcardValues;

        let mut state = CheckpointState::new();
        let mut combo = WildcardValues::new();
        combo.insert("sample".to_string(), "S1".to_string());
        combo.insert("part".to_string(), "p1".to_string());
        state.record_output_pattern_domain("split", vec![combo.clone()]);
        // Union across instances: same combo again is a no-op, a new combo
        // appends.
        state.record_output_pattern_domain("split", vec![combo.clone()]);
        let mut combo2 = WildcardValues::new();
        combo2.insert("sample".to_string(), "S2".to_string());
        combo2.insert("part".to_string(), "p1".to_string());
        state.record_output_pattern_domain("split", vec![combo2.clone()]);

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("output_pattern_domains"));
        let loaded: CheckpointState = serde_json::from_str(&json).unwrap();
        let domain = loaded.output_pattern_domains.get("split").unwrap();
        assert_eq!(domain.len(), 2, "deduped union of two distinct combos");
        assert!(domain.contains(&combo));
        assert!(domain.contains(&combo2));

        // Legacy checkpoints load empty (safe default).
        let legacy: CheckpointState =
            serde_json::from_str(r#"{"completed_rules":[],"failed_rules":[],"benchmarks":{}}"#)
                .unwrap();
        assert!(legacy.output_pattern_domains.is_empty());
    }

    #[test]
    fn output_pattern_domain_replay_reinstantiates_consumers() {
        // The resume path: load the persisted domain into a fresh config
        // (producer never re-runs — it is already completed), replay the
        // domain, and the deferred consumer re-instantiates with the same
        // deterministic names.
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("replay.oxoflow");
        for part in ["1", "2"] {
            let path = dir.path().join(format!("results/chunks/{part}.txt"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, part).unwrap();
        }
        std::fs::write(
            &workflow_path,
            r#"
            [workflow]
            name = "replay"

            [[rules]]
            name = "split"
            output_pattern = "results/chunks/{part}.txt"
            shell = "mkdir -p results/chunks && echo x > results/chunks/1.txt"

            [[rules]]
            name = "collect"
            input = ["results/chunks/{part}.txt"]
            output = ["results/merged/{part}.txt"]
            shell = "cat {input} > {output}"
            "#,
        )
        .unwrap();

        // Run 1: producer completes, domain discovered and persisted.
        let mut state = CheckpointState::new();
        let mut config = crate::config::WorkflowConfig::from_file(&workflow_path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let split = config.get_rule("split").cloned().unwrap();
        let combos = config
            .discover_output_pattern_files(&split, dir.path())
            .unwrap();
        state.record_output_pattern_domain("split", combos);
        assert!(
            !state
                .output_pattern_domains
                .get("split")
                .unwrap()
                .is_empty()
        );

        // Resume: a FRESH config from the same workflow (no producer re-run
        // simulated), the persisted domain replayed into it.
        let mut resumed = crate::config::WorkflowConfig::from_file(&workflow_path).unwrap();
        resumed.apply_defaults();
        resumed.expand_wildcards().unwrap();
        resumed.discovered_output_patterns = state.output_pattern_domains.clone();
        let new_names = resumed.expand_output_pattern_consumers().unwrap();
        assert_eq!(new_names, vec!["collect_1", "collect_2"]);
        let c = resumed.get_rule("collect_1").unwrap();
        assert_eq!(c.input.to_vec(), vec!["results/chunks/1.txt"]);
    }
}
