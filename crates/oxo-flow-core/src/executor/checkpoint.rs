use crate::error::{OxoFlowError, Result};
use crate::executor::JobRecord;
use crate::rule::{FilePatterns, Rule};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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
    /// Tail of the rule's stderr (see the `STDERR_TAIL_CHARS` constant) for
    /// failure diagnosis. Absent when the rule produced no stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
}

/// Persistent checkpoint state for resumable workflow execution.
///
/// Tracks which rules have completed or failed so that a restarted workflow
/// can skip already-finished work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointState {
    /// Rules that completed successfully.
    pub completed_rules: HashSet<String>,
    /// Rules that failed during execution.
    pub failed_rules: HashSet<String>,
    /// Benchmark records keyed by rule name.
    pub benchmarks: HashMap<String, BenchmarkRecord>,
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
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub checksums: HashMap<String, String>,
    /// Config value snapshot at the time rules completed.
    /// Maps config key → canonical value string (sensitive keys store a
    /// SHA-256 digest instead of the plaintext value). Compared against the
    /// current config on every run to drive precise invalidation (issue #62).
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub config_snapshot: HashMap<String, String>,
    /// Per-rule structural fingerprints at completion time.
    /// Maps rule name → `sha256:<hex>` of the fields that determine rule
    /// output content (shell, script, inputs, outputs, envvars, params,
    /// conditions, environment). A mismatch invalidates the rule and its
    /// downstream (issue #62).
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub rule_fingerprints: HashMap<String, String>,
    /// Per-rule fingerprints with the input field EXCLUDED (issue #142 M1).
    /// Distinguishes a genuine rule edit from a pure `--samples` selection
    /// change: expand_inputs-over-injected-key rules bake the selection into
    /// their input list, so the full fingerprint differs on every subset
    /// run while this one stays identical. Absent for checkpoints written by
    /// older binaries — those keep invalidating (safe default).
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub rule_fingerprints_no_input: HashMap<String, String>,
    /// Per-rule input manifests at completion time (issue #72).
    /// Maps rule name → sorted list of (relative path, size, mtime) for every
    /// file the rule's inputs resolved to. A mismatch with the current file
    /// set invalidates the rule and its downstream.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub input_manifests: HashMap<String, InputManifest>,

    /// Tombstones for `temporary = true` rules whose outputs were deleted
    /// after a fully successful run. Maps rule name → deleted output paths;
    /// a future run regenerates them via cascade-up (the completed producer
    /// is re-executed when a dependent needs the missing inputs).
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub tombstones: HashMap<String, Vec<String>>,

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
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub rule_runs: HashMap<String, RuleRunRecord>,
}

/// Bound on the stderr excerpt persisted per rule (issue #83 WS2). Full
/// output stays in the terminal; the checkpoint keeps enough for failure
/// diagnosis without growing unbounded on noisy tools.
const STDERR_TAIL_CHARS: usize = 2048;

/// Last [`STDERR_TAIL_CHARS`] characters of a rule's stderr, prefixed with
/// an ellipsis marker when truncated.
fn stderr_tail(stderr: Option<&str>) -> Option<String> {
    let stderr = stderr?;
    // nth_back is 0-indexed from the end, so N-1 lands exactly N chars back.
    match stderr.char_indices().nth_back(STDERR_TAIL_CHARS - 1) {
        Some((start, _)) => Some(format!("…\n{}", &stderr[start..])),
        None => Some(stderr.to_string()),
    }
}

impl CheckpointState {
    /// Create a new, empty checkpoint state.
    pub fn new() -> Self {
        Self {
            completed_rules: HashSet::new(),
            failed_rules: HashSet::new(),
            benchmarks: HashMap::new(),
            workflow_path: None,
            workflow_git_sha: None,
            workdir: None,
            checksums: HashMap::new(),
            config_snapshot: HashMap::new(),
            rule_fingerprints: HashMap::new(),
            rule_fingerprints_no_input: HashMap::new(),
            input_manifests: HashMap::new(),
            tombstones: HashMap::new(),
            reentries: Vec::new(),
            rule_runs: HashMap::new(),
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

    /// Record the input manifest for a rule (issue #72).
    pub fn record_input_manifest(&mut self, rule: &str, manifest: InputManifest) {
        self.input_manifests.insert(rule.to_string(), manifest);
    }

    /// Set the workflow path that generated this checkpoint.
    pub fn set_workflow_path(&mut self, path: &Path) {
        self.workflow_path = Some(path.to_string_lossy().to_string());
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

    /// Mark a rule as successfully completed and store its benchmark.
    pub fn mark_completed(&mut self, rule: &str, benchmark: BenchmarkRecord) {
        self.completed_rules.insert(rule.to_string());
        self.failed_rules.remove(rule);
        self.benchmarks.insert(rule.to_string(), benchmark);
    }

    /// Mark a rule as failed.
    pub fn mark_failed(&mut self, rule: &str) {
        self.failed_rules.insert(rule.to_string());
        self.completed_rules.remove(rule);
    }

    /// Persist execution detail for reporting (issue #83 WS2): the exit
    /// code and expanded command that actually ran, plus a bounded stderr
    /// excerpt. Call at completion/failure time, before the corresponding
    /// `mark_completed`/`mark_failed`.
    pub fn record_run(&mut self, record: &JobRecord) {
        self.rule_runs.insert(
            record.rule.clone(),
            RuleRunRecord {
                exit_code: record.exit_code,
                command: record.command.clone(),
                stderr_tail: stderr_tail(record.stderr.as_deref()),
            },
        );
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
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let parent = crate::parent_dir(path);
        if parent != std::path::Path::new(".") {
            std::fs::create_dir_all(parent).map_err(|e| OxoFlowError::Config {
                message: format!("failed to create checkpoint directory: {e}"),
            })?;
        }
        let json = self.to_json()?;
        let tmp_path = path.with_extension("json.tmp");
        let write_result = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp_path)?;
            use std::io::Write;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp_path, path)?;
            // fsync the parent directory so the rename is durable (POSIX).
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        })();
        write_result.map_err(|e| {
            // Never leave a partial tmp file behind for the next attempt to
            // trip over; the real checkpoint (old or new) is what matters.
            let _ = std::fs::remove_file(&tmp_path);
            OxoFlowError::Config {
                message: format!("failed to save checkpoint to {}: {e}", path.display()),
            }
        })
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
    Ok(format!("sha256:{:x}", hash))
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
/// `Some("sha256:…")` when the file is at most [`MANIFEST_HASH_MAX_BYTES`]
/// and readable; `None` for larger files (guarded by size+mtime) and for
/// unreadable files (best-effort degrade, never an error).
pub fn content_hash_if_small(path: &Path, md: &std::fs::Metadata) -> Option<String> {
    (md.len() <= MANIFEST_HASH_MAX_BYTES)
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
        if expanded.contains('{') {
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
        collect_pattern_entries(&expanded, dir_filter.as_deref(), workdir, &mut entries)?;
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
    for pattern in rule.input.to_vec() {
        let expanded = expand_config_in_path(&pattern, wildcard_values);
        if expanded.contains('{') || expanded.starts_with(".oxo-flow/chunks") {
            continue;
        }
        let mut entries = std::collections::BTreeMap::new();
        if collect_pattern_entries(&expanded, dir_filter.as_deref(), workdir, &mut entries).is_err()
        {
            missing.push(expanded);
        }
    }
    missing
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

/// Check if a rule should be skipped based on output freshness.
///
/// Returns true if all outputs exist and are newer than all inputs.
/// Config variable placeholders (e.g. `{config.sample}`) are expanded using
/// `wildcard_values` before the path existence check.
pub fn should_skip_rule(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> bool {
    should_skip_rule_with_checksums(rule, workdir, wildcard_values, None)
}

/// [`should_skip_rule`] with provenance checksums (issue #194 B2).
///
/// When `checksums` holds a recorded content hash for EVERY expanded output
/// of the rule, freshness requires the CURRENT content to match — mtime
/// alone no longer decides (a `touch` or clock skew cannot fake reuse).
/// Two honest fallbacks keep the old behavior: any output beyond the hash
/// cap (no re-verifiable digest), and rules with no recorded checksums at
/// all, both degrade to the mtime comparison.
pub fn should_skip_rule_with_checksums(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    checksums: Option<&HashMap<String, String>>,
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

    if expanded_inputs.is_empty() {
        return true; // No inputs to check freshness against
    }
    // Check if all outputs are newer than all inputs
    expanded_inputs.iter().all(|input| {
        let input_path = workdir.join(input);
        expanded_outputs.iter().all(|output| {
            let output_path = workdir.join(output);
            file_is_newer(&output_path, &input_path)
        })
    })
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
pub async fn cleanup_transform_chunks(rule: &Rule, workdir: &Path) {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    for chunk in rule.input.iter() {
        let path = workdir.join(chunk);
        if tokio::fs::try_exists(&path).await.ok() == Some(true) {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    tracing::debug!(file = %path.display(), "removed transform chunk");
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::FilePatterns;
    use crate::storage::{RemoteStat, StorageBackend, StoragePath, StorageResolver, StorageScheme};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

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
        assert!(!dir.path().join("checkpoint.json.tmp").exists());
        let loaded = CheckpointState::load_from_file(&path).unwrap();
        assert!(loaded.completed_rules.contains("rule_a"));
        // A second save overwrites cleanly.
        ck.completed_rules.insert("rule_b".to_string());
        ck.save_to_file(&path).unwrap();
        let loaded = CheckpointState::load_from_file(&path).unwrap();
        assert!(loaded.completed_rules.contains("rule_b"));
        assert!(!dir.path().join("checkpoint.json.tmp").exists());
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
        assert!(!dir.path().join("checkpoint.json.tmp").exists());
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
        let recorded: HashMap<String, String> = [(
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

        cleanup_transform_chunks(&rule, &workdir).await;

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
}
