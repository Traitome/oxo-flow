use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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
}

impl CheckpointState {
    /// Create a new, empty checkpoint state.
    pub fn new() -> Self {
        Self {
            completed_rules: HashSet::new(),
            failed_rules: HashSet::new(),
            benchmarks: HashMap::new(),
        }
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

    let content = std::fs::read(path).map_err(|e| OxoFlowError::Execution {
        rule: String::new(),
        message: format!("failed to read {} for checksum: {e}", path.display()),
    })?;

    // SHA-256 for clinical-grade integrity verification (CLIA/CAP requirement)
    let mut hasher = Sha256::new();
    hasher.update(&content);
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
