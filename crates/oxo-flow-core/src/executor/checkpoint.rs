use crate::error::{OxoFlowError, Result};
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
}

/// Sorted, deduplicated snapshot of a rule's resolved input files.
/// Two manifests are equal iff the file set and every file's size + mtime
/// are unchanged.
pub type InputManifest = Vec<InputManifestEntry>;

/// Performance metrics recorded after executing a rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRecord {
    /// Name of the rule that was benchmarked.
    pub rule: String,
    /// Wall-clock time in seconds.
    pub wall_time_secs: f64,
    /// Peak resident memory in megabytes (placeholder — not yet measured).
    pub max_memory_mb: Option<u64>,
    /// Total CPU seconds consumed (placeholder — not yet measured).
    pub cpu_seconds: Option<f64>,
    /// Number of retry attempts before success (0 = first attempt succeeded).
    #[serde(default)]
    pub retries: u32,
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
    /// Working directory the rules executed in (issue #68). `resume` re-runs
    /// from this directory so completed rules' outputs resolve the same way;
    /// absent in legacy checkpoints, which fall back to the workflow's
    /// directory.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Output file checksums for provenance verification.
    /// Maps relative output file path → "sha256:<hex>".
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
    /// Maps rule name → "sha256:<hex>" of the fields that determine rule
    /// output content (shell, script, inputs, outputs, envvars, params,
    /// conditions, environment). A mismatch invalidates the rule and its
    /// downstream (issue #62).
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub rule_fingerprints: HashMap<String, String>,
    /// Per-rule input manifests at completion time (issue #72).
    /// Maps rule name → sorted list of (relative path, size, mtime) for every
    /// file the rule's inputs resolved to. A mismatch with the current file
    /// set invalidates the rule and its downstream.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub input_manifests: HashMap<String, InputManifest>,
}

impl CheckpointState {
    /// Create a new, empty checkpoint state.
    pub fn new() -> Self {
        Self {
            completed_rules: HashSet::new(),
            failed_rules: HashSet::new(),
            benchmarks: HashMap::new(),
            workflow_path: None,
            workdir: None,
            checksums: HashMap::new(),
            config_snapshot: HashMap::new(),
            rule_fingerprints: HashMap::new(),
            input_manifests: HashMap::new(),
        }
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
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let parent = crate::parent_dir(path);
        if parent != std::path::Path::new(".") {
            std::fs::create_dir_all(parent).map_err(|e| OxoFlowError::Config {
                message: format!("failed to create checkpoint directory: {e}"),
            })?;
        }
        let json = self.to_json()?;
        std::fs::write(path, json).map_err(|e| OxoFlowError::Config {
            message: format!("failed to save checkpoint to {}: {e}", path.display()),
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

/// Verify output file checksums match previously recorded values.
///
/// Returns a list of (file_path, expected, actual) tuples for any mismatches.
pub fn verify_output_checksums(
    checksums: &HashMap<String, String>,
    workdir: &Path,
) -> Vec<(String, String, String)> {
    let mut mismatches = Vec::new();
    for (file, expected) in checksums {
        let path = workdir.join(file);
        match compute_file_checksum(&path) {
            Ok(actual) if actual != *expected => {
                mismatches.push((file.clone(), expected.clone(), actual));
            }
            Err(_) => {
                mismatches.push((file.clone(), expected.clone(), "<unreadable>".to_string()));
            }
            _ => {}
        }
    }
    mismatches
}

/// Check if a rule should be skipped based on content-aware caching.
///
/// Unlike [`should_skip_rule`] which only checks file modification times,
/// this function also considers file content checksums. This avoids
/// unnecessary re-execution when a file's mtime changes but its content
/// does not (e.g., after `touch` or a no-op rebuild).
///
/// `known_checksums` maps file paths to their previously recorded checksums.
/// If a file's current checksum matches its known checksum, the file is
/// considered unchanged even if its mtime is newer.
pub fn should_skip_rule_content_aware(
    rule: &Rule,
    workdir: &Path,
    known_checksums: &HashMap<String, String>,
) -> bool {
    if rule.output.is_empty() {
        return false;
    }
    // Skip check for wildcard patterns
    if rule.output.iter().any(|o| o.contains('{')) || rule.input.iter().any(|i| i.contains('{')) {
        return false;
    }
    let all_outputs_exist = rule.output.iter().all(|o| workdir.join(o).exists());
    if !all_outputs_exist {
        return false;
    }
    if rule.input.is_empty() {
        return true;
    }

    // First check mtime (fast path)
    let mtime_fresh = rule.input.iter().all(|input| {
        let input_path = workdir.join(input);
        rule.output.iter().all(|output| {
            let output_path = workdir.join(output);
            file_is_newer(&output_path, &input_path)
        })
    });

    if mtime_fresh {
        return true;
    }

    // Mtime says stale — check content checksums as fallback
    // If all input files have unchanged content (matching known checksums),
    // we can still skip the rule
    rule.input.iter().all(|input| {
        let input_path = workdir.join(input);
        if let Some(known) = known_checksums.get(input) {
            match compute_file_checksum(&input_path) {
                Ok(current) => current == *known,
                Err(_) => false,
            }
        } else {
            false // No known checksum — can't verify content
        }
    })
}

/// Compute checksums for all non-wildcard input files of a rule.
///
/// Returns a map from file path (relative) to hex-encoded checksum.
/// Files that cannot be read are silently skipped.
pub fn compute_input_checksums(rule: &Rule, workdir: &Path) -> HashMap<String, String> {
    let mut checksums = HashMap::new();
    for input in &rule.input {
        if crate::wildcard::has_wildcards(input) {
            continue;
        }
        let path = workdir.join(input);
        if let Ok(checksum) = compute_file_checksum(&path) {
            checksums.insert(input.clone(), checksum);
        }
    }
    checksums
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
        collect_pattern_entries(&expanded, dir_filter.as_deref(), workdir, &mut entries)?;
    }

    if !saw_resolvable {
        return Ok(None);
    }
    Ok(Some(entries.into_values().collect()))
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
    let mtime_nanos = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    entries.insert(
        rel.clone(),
        InputManifestEntry {
            path: rel,
            size: md.len(),
            mtime_nanos,
        },
    );
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
    let mut result = path.to_string();
    for (key, value) in wildcard_values {
        result = result.replace(&format!("{{{key}}}"), value);
    }
    result
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
        snapshot_input_manifest(rule, workdir, &HashMap::new()).unwrap()
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
        assert!(snapshot_input_manifest(&rule, &wd, &HashMap::new()).is_err());
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
        let manifest = snapshot_input_manifest(&rule, &wd, &values)
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
            }]
        );
        // Older checkpoints without input_manifests still load.
        let legacy = r#"{"completed_rules":["r"],"failed_rules":[],"benchmarks":{}}"#;
        let loaded: CheckpointState = serde_json::from_str(legacy).unwrap();
        assert!(loaded.input_manifests.is_empty());
    }
}
