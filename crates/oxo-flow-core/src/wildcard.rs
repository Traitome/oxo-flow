//! Wildcard pattern expansion for oxo-flow.
//!
//! Supports `{wildcard}` patterns in file paths, expanding them
//! against provided values or input file discovery.

use crate::error::{OxoFlowError, Result};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Compiled regex that matches a single `{name}` wildcard placeholder.
///
/// Using a module-level static avoids recompiling the same regex on every
/// call to `extract_wildcards`, `expand_pattern`, `has_wildcards`, etc.
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{(\w+)\}").expect("valid wildcard regex"));

/// A single wildcard binding, e.g., `sample = "TUMOR_01"`.
pub type WildcardValues = HashMap<String, String>;

/// A set of wildcard value combinations for expanding rules.
pub type WildcardCombinations = Vec<WildcardValues>;

/// A map of wildcard names to regex constraints for validation.
///
/// When constraints are provided, wildcard values must match the corresponding
/// regex pattern. This enables stricter validation of file patterns.
///
/// # Example
///
/// ```
/// use oxo_flow_core::wildcard::{WildcardConstraints, validate_wildcard_constraints};
///
/// let mut constraints = WildcardConstraints::new();
/// constraints.insert("sample".to_string(), r"^[A-Za-z0-9_]+$".to_string());
/// constraints.insert("chr".to_string(), r"^chr([0-9]+|[XYM])$".to_string());
///
/// let mut values = std::collections::HashMap::new();
/// values.insert("sample".to_string(), "TUMOR_01".to_string());
/// values.insert("chr".to_string(), "chr1".to_string());
///
/// assert!(validate_wildcard_constraints(&values, &constraints).is_ok());
/// ```
pub type WildcardConstraints = HashMap<String, String>;

/// Validate wildcard values against regex constraints.
///
/// Returns Ok(()) if all constrained wildcards match their patterns,
/// or an error listing all violations.
pub fn validate_wildcard_constraints(
    values: &WildcardValues,
    constraints: &WildcardConstraints,
) -> Result<()> {
    let mut violations = Vec::new();

    for (name, pattern) in constraints {
        if let Some(value) = values.get(name) {
            match Regex::new(pattern) {
                Ok(re) => {
                    if !re.is_match(value) {
                        violations.push(format!(
                            "wildcard '{}' value '{}' does not match constraint '{}'",
                            name, value, pattern
                        ));
                    }
                }
                Err(e) => {
                    violations.push(format!(
                        "invalid regex constraint '{}' for wildcard '{}': {}",
                        pattern, name, e
                    ));
                }
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(OxoFlowError::Wildcard {
            rule: String::new(),
            message: violations.join("; "),
        })
    }
}

/// Convert a wildcard pattern (e.g., `{sample}_R{read}.fastq.gz`) to a regex
/// for file discovery against directory listings.
///
/// The `{name}` placeholders are replaced with named capture groups.
pub fn pattern_to_regex(pattern: &str) -> Result<Regex> {
    let mut regex_str = String::from("^");
    let mut last_end = 0;

    for mat in WILDCARD_RE.find_iter(pattern) {
        let literal = &pattern[last_end..mat.start()];
        regex_str.push_str(&regex::escape(literal));

        let cap = WILDCARD_RE
            .captures(&pattern[mat.start()..mat.end()])
            .ok_or_else(|| OxoFlowError::Wildcard {
                rule: String::new(),
                message: format!(
                    "internal error: wildcard regex match failed to capture on pattern part '{}'",
                    &pattern[mat.start()..mat.end()]
                ),
            })?;
        let name = &cap[1];
        regex_str.push_str(&format!("(?P<{}>\\S+)", name));

        last_end = mat.end();
    }

    let remaining = &pattern[last_end..];
    regex_str.push_str(&regex::escape(remaining));
    regex_str.push('$');

    Regex::new(&regex_str).map_err(|e| OxoFlowError::Wildcard {
        rule: String::new(),
        message: format!("failed to compile pattern regex: {}", e),
    })
}

/// Expands a pattern into a list of strings by taking the Cartesian product
/// of provided variable values.
///
/// This is similar to Snakemake's `expand()` function.
///
/// # Examples
///
/// ```
/// use oxo_flow_core::wildcard::cartesian_expand;
/// use std::collections::HashMap;
///
/// let mut variables = HashMap::new();
/// variables.insert("sample".to_string(), vec!["S1".to_string(), "S2".to_string()]);
/// variables.insert("read".to_string(), vec!["1".to_string(), "2".to_string()]);
///
/// let results = cartesian_expand("{sample}_R{read}.fastq.gz", &variables);
/// assert_eq!(results.len(), 4);
/// assert!(results.contains(&"S1_R1.fastq.gz".to_string()));
/// assert!(results.contains(&"S2_R2.fastq.gz".to_string()));
/// ```
pub fn cartesian_expand(pattern: &str, variables: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut results = vec![pattern.to_string()];

    // Identify which wildcards in the pattern have provided values
    let wildcards = extract_wildcards(pattern);
    let mut active_vars = Vec::new();
    for name in wildcards {
        if let Some(vals) = variables.get(&name) {
            active_vars.push((name, vals));
        }
    }

    if active_vars.is_empty() {
        return results;
    }

    // Iteratively expand each variable
    for (name, vals) in active_vars {
        let mut new_results = Vec::new();
        let placeholder = format!("{{{name}}}");
        for r in results {
            for v in vals {
                new_results.push(r.replace(&placeholder, v));
            }
        }
        results = new_results;
    }

    results
}

/// Discovers files matching a wildcard pattern in a directory.
///
/// Returns a list of wildcard value maps, one per matching file found.
pub fn discover_wildcards_from_pattern(
    dir: &std::path::Path,
    pattern: &str,
) -> Result<WildcardCombinations> {
    let re = pattern_to_regex(pattern)?;
    let wildcard_names = extract_wildcards(pattern);
    let mut results = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let filename = entry.file_name().to_string_lossy().to_string();
            if let Some(captures) = re.captures(&filename) {
                let mut values = WildcardValues::new();
                for name in &wildcard_names {
                    if let Some(m) = captures.name(name) {
                        values.insert(name.clone(), m.as_str().to_string());
                    }
                }
                if !values.is_empty() && !results.contains(&values) {
                    results.push(values);
                }
            }
        }
    }

    Ok(results)
}

/// Extracts wildcard names from a pattern string.
///
/// # Examples
///
/// ```
/// use oxo_flow_core::wildcard::extract_wildcards;
///
/// let names = extract_wildcards("{sample}_R{read}.fastq.gz");
/// assert_eq!(names, vec!["sample", "read"]);
/// ```
pub fn extract_wildcards(pattern: &str) -> Vec<String> {
    let mut names = Vec::new();
    for cap in WILDCARD_RE.captures_iter(pattern) {
        let name = cap[1].to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Expands a pattern by substituting wildcard placeholders with values.
///
/// # Examples
///
/// ```
/// use oxo_flow_core::wildcard::{expand_pattern, WildcardValues};
///
/// let mut values = WildcardValues::new();
/// values.insert("sample".to_string(), "TUMOR_01".to_string());
/// values.insert("read".to_string(), "1".to_string());
///
/// let result = expand_pattern("{sample}_R{read}.fastq.gz", &values).unwrap();
/// assert_eq!(result, "TUMOR_01_R1.fastq.gz");
/// ```
#[must_use = "expanding a pattern returns a Result that must be used"]
pub fn expand_pattern(pattern: &str, values: &WildcardValues) -> Result<String> {
    let mut result = pattern.to_string();
    let mut missing = Vec::new();

    for cap in WILDCARD_RE.captures_iter(pattern) {
        let name = &cap[1];
        match values.get(name) {
            Some(value) => {
                result = result.replace(&format!("{{{name}}}"), value);
            }
            None => {
                missing.push(name.to_string());
            }
        }
    }

    if !missing.is_empty() {
        return Err(OxoFlowError::Wildcard {
            rule: String::new(),
            message: format!("unresolved wildcards: {}", missing.join(", ")),
        });
    }

    Ok(result)
}

/// Expands all patterns in a list using the given wildcard values.
#[must_use = "expanding patterns returns a Result that must be used"]
pub fn expand_patterns(patterns: &[String], values: &WildcardValues) -> Result<Vec<String>> {
    patterns.iter().map(|p| expand_pattern(p, values)).collect()
}

/// Returns `true` if the pattern contains any wildcard placeholders.
pub fn has_wildcards(pattern: &str) -> bool {
    WILDCARD_RE.is_match(pattern)
}

/// Generates the Cartesian product of all wildcard value lists.
///
/// Given `{"sample": ["A", "B"], "read": ["1", "2"]}`, produces:
/// `[{sample: A, read: 1}, {sample: A, read: 2}, {sample: B, read: 1}, {sample: B, read: 2}]`
pub fn cartesian_product(wildcard_lists: &HashMap<String, Vec<String>>) -> WildcardCombinations {
    let keys: Vec<&String> = wildcard_lists.keys().collect();
    if keys.is_empty() {
        return vec![WildcardValues::new()];
    }

    let mut combinations: WildcardCombinations = vec![WildcardValues::new()];

    for key in &keys {
        let values = &wildcard_lists[*key];
        let mut new_combinations = Vec::new();

        for combo in &combinations {
            for value in values {
                let mut new_combo = combo.clone();
                new_combo.insert((*key).clone(), value.clone());
                new_combinations.push(new_combo);
            }
        }

        combinations = new_combinations;
    }

    combinations
}

/// Extract wildcard names from multiple patterns.
pub fn extract_wildcards_from_patterns(patterns: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for pattern in patterns {
        for name in extract_wildcards(pattern) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Generate paired-end FASTQ file patterns from a sample name.
///
/// Returns a tuple of (R1_pattern, R2_pattern) for the given sample
/// with the specified directory and extension.
///
/// # Example
/// ```
/// # use oxo_flow_core::wildcard::paired_end_pattern;
/// let (r1, r2) = paired_end_pattern("data", "{sample}", "fastq.gz");
/// assert_eq!(r1, "data/{sample}_R1.fastq.gz");
/// assert_eq!(r2, "data/{sample}_R2.fastq.gz");
/// ```
#[must_use]
pub fn paired_end_pattern(dir: &str, sample_pattern: &str, extension: &str) -> (String, String) {
    let r1 = format!("{}/{}_R1.{}", dir, sample_pattern, extension);
    let r2 = format!("{}/{}_R2.{}", dir, sample_pattern, extension);
    (r1, r2)
}

/// Discover paired-end FASTQ files in a directory for a given sample name.
///
/// Looks for files matching common paired-end naming conventions:
/// `{sample}_R1.fastq.gz` / `{sample}_R2.fastq.gz`,
/// `{sample}_1.fastq.gz` / `{sample}_2.fastq.gz`, etc.
#[must_use]
pub fn discover_paired_files(dir: &std::path::Path, sample: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let suffixes = [
        ("_R1.fastq.gz", "_R2.fastq.gz"),
        ("_R1.fq.gz", "_R2.fq.gz"),
        ("_1.fastq.gz", "_2.fastq.gz"),
        ("_1.fq.gz", "_2.fq.gz"),
        ("_R1.fastq", "_R2.fastq"),
        ("_R1.fq", "_R2.fq"),
    ];
    for (s1, s2) in &suffixes {
        let r1 = dir.join(format!("{}{}", sample, s1));
        let r2 = dir.join(format!("{}{}", sample, s2));
        if r1.exists() && r2.exists() {
            pairs.push((
                r1.to_string_lossy().to_string(),
                r2.to_string_lossy().to_string(),
            ));
        }
    }
    pairs
}

// ---------------------------------------------------------------------------
// WC-01: Experiment-control pairing wildcard helpers
// ---------------------------------------------------------------------------

/// Build wildcard value combinations from a list of experiment-control pairs.
///
/// Each pair produces a [`WildcardValues`] map containing:
/// - `pair_id`    → the pair's unique identifier
/// - `experiment` → experiment sample identifier
/// - `control`    → control sample identifier
/// - `experiment_type` → experiment/condition type (when present)
/// - backward-compatible aliases: `tumor`, `normal`, `tumor_type`
/// - any additional metadata keys defined on the pair
///
/// These combinations are used by [`crate::config::WorkflowConfig::expand_wildcards`]
/// to expand rules containing `{experiment}`, `{control}`, or `{pair_id}`
/// placeholders.
///
/// # Example
///
/// ```
/// use oxo_flow_core::config::ExperimentControlPair;
/// use oxo_flow_core::wildcard::wildcard_combinations_from_pairs;
///
/// let pairs = vec![
///     ExperimentControlPair {
///         pair_id: "CASE_001".to_string(),
///         experiment: "EXP_01".to_string(),
///         control: "CTRL_01".to_string(),
///         experiment_type: Some("lung".to_string()),
///         metadata: Default::default(),
///     },
/// ];
/// let combos = wildcard_combinations_from_pairs(&pairs);
/// assert_eq!(combos.len(), 1);
/// assert_eq!(combos[0]["experiment"], "EXP_01");
/// assert_eq!(combos[0]["control"], "CTRL_01");
/// assert_eq!(combos[0]["pair_id"], "CASE_001");
/// assert_eq!(combos[0]["experiment_type"], "lung");
/// ```
pub fn wildcard_combinations_from_pairs(
    pairs: &[crate::config::ExperimentControlPair],
) -> WildcardCombinations {
    pairs
        .iter()
        .map(|pair| {
            let mut values = WildcardValues::new();
            values.insert("pair_id".to_string(), pair.pair_id.clone());
            values.insert("experiment".to_string(), pair.experiment.clone());
            values.insert("control".to_string(), pair.control.clone());
            // Backward-compatible aliases
            values.insert("tumor".to_string(), pair.experiment.clone());
            values.insert("normal".to_string(), pair.control.clone());
            if let Some(ref t) = pair.experiment_type {
                values.insert("experiment_type".to_string(), t.clone());
                values.insert("tumor_type".to_string(), t.clone());
            }
            for (k, v) in &pair.metadata {
                values.insert(k.clone(), v.clone());
            }
            values
        })
        .collect()
}

// ---------------------------------------------------------------------------
// WC-02: Multi-sample group wildcard helpers
// ---------------------------------------------------------------------------

/// Build wildcard value combinations from sample groups.
///
/// Creates one [`WildcardValues`] entry per (group, sample) combination,
/// providing:
/// - `group`  → the group name
/// - `sample` → the sample identifier within that group
/// - any additional metadata keys defined on the group
///
/// These combinations are used by [`crate::config::WorkflowConfig::expand_wildcards`]
/// to expand rules containing `{group}` or `{sample}` placeholders.
///
/// # Example
///
/// ```
/// use oxo_flow_core::config::SampleGroup;
/// use oxo_flow_core::wildcard::wildcard_combinations_from_groups;
///
/// let groups = vec![
///     SampleGroup {
///         name: "control".to_string(),
///         samples: vec!["S001".to_string(), "S002".to_string()],
///         metadata: Default::default(),
///     },
/// ];
/// let combos = wildcard_combinations_from_groups(&groups);
/// assert_eq!(combos.len(), 2);
/// assert_eq!(combos[0]["group"], "control");
/// assert_eq!(combos[0]["sample"], "S001");
/// assert_eq!(combos[1]["sample"], "S002");
/// ```
pub fn wildcard_combinations_from_groups(
    groups: &[crate::config::SampleGroup],
) -> WildcardCombinations {
    let mut combinations = Vec::new();
    for group in groups {
        for sample in &group.samples {
            let mut values = WildcardValues::new();
            values.insert("group".to_string(), group.name.clone());
            values.insert("sample".to_string(), sample.clone());
            for (k, v) in &group.metadata {
                values.insert(k.clone(), v.clone());
            }
            combinations.push(values);
        }
    }
    combinations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_wildcards_simple() {
        let names = extract_wildcards("{sample}_R{read}.fastq.gz");
        assert_eq!(names, vec!["sample", "read"]);
    }

    #[test]
    fn extract_wildcards_none() {
        let names = extract_wildcards("output.bam");
        assert!(names.is_empty());
    }

    #[test]
    fn extract_wildcards_duplicate() {
        let names = extract_wildcards("{sample}_{sample}.txt");
        assert_eq!(names, vec!["sample"]);
    }

    #[test]
    fn expand_pattern_success() {
        let mut values = WildcardValues::new();
        values.insert("sample".to_string(), "TUMOR".to_string());
        values.insert("read".to_string(), "1".to_string());

        let result = expand_pattern("{sample}_R{read}.fastq.gz", &values).unwrap();
        assert_eq!(result, "TUMOR_R1.fastq.gz");
    }

    #[test]
    fn expand_pattern_missing_wildcard() {
        let values = WildcardValues::new();
        let result = expand_pattern("{sample}.bam", &values);
        assert!(result.is_err());
    }

    #[test]
    fn expand_pattern_no_wildcards() {
        let values = WildcardValues::new();
        let result = expand_pattern("output.bam", &values).unwrap();
        assert_eq!(result, "output.bam");
    }

    #[test]
    fn has_wildcards_true() {
        assert!(has_wildcards("{sample}.bam"));
    }

    #[test]
    fn has_wildcards_false() {
        assert!(!has_wildcards("output.bam"));
    }

    #[test]
    fn cartesian_product_empty() {
        let lists = HashMap::new();
        let result = cartesian_product(&lists);
        assert_eq!(result.len(), 1);
        assert!(result[0].is_empty());
    }

    #[test]
    fn cartesian_product_single() {
        let mut lists = HashMap::new();
        lists.insert("sample".to_string(), vec!["A".to_string(), "B".to_string()]);
        let result = cartesian_product(&lists);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn cartesian_product_two_dimensions() {
        let mut lists = HashMap::new();
        lists.insert("sample".to_string(), vec!["A".to_string(), "B".to_string()]);
        lists.insert("read".to_string(), vec!["1".to_string(), "2".to_string()]);
        let result = cartesian_product(&lists);
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn paired_end_pattern_basic() {
        let (r1, r2) = paired_end_pattern("data", "{sample}", "fastq.gz");
        assert_eq!(r1, "data/{sample}_R1.fastq.gz");
        assert_eq!(r2, "data/{sample}_R2.fastq.gz");
    }

    #[test]
    fn validate_constraints_pass() {
        let mut constraints = WildcardConstraints::new();
        constraints.insert("sample".to_string(), r"^[A-Za-z0-9_]+$".to_string());

        let mut values = WildcardValues::new();
        values.insert("sample".to_string(), "TUMOR_01".to_string());

        assert!(validate_wildcard_constraints(&values, &constraints).is_ok());
    }

    #[test]
    fn validate_constraints_fail() {
        let mut constraints = WildcardConstraints::new();
        constraints.insert("chr".to_string(), r"^chr[0-9XYM]+$".to_string());

        let mut values = WildcardValues::new();
        values.insert("chr".to_string(), "invalid".to_string());

        assert!(validate_wildcard_constraints(&values, &constraints).is_err());
    }

    #[test]
    fn validate_constraints_bad_regex() {
        let mut constraints = WildcardConstraints::new();
        constraints.insert("x".to_string(), r"[invalid".to_string());

        let mut values = WildcardValues::new();
        values.insert("x".to_string(), "test".to_string());

        assert!(validate_wildcard_constraints(&values, &constraints).is_err());
    }

    #[test]
    fn pattern_to_regex_basic() {
        let re = pattern_to_regex("{sample}_R{read}.fastq.gz").unwrap();
        assert!(re.is_match("TUMOR_01_R1.fastq.gz"));
        assert!(!re.is_match("something_else.bam"));

        let caps = re.captures("TUMOR_01_R1.fastq.gz").unwrap();
        assert_eq!(&caps["sample"], "TUMOR_01");
        assert_eq!(&caps["read"], "1");
    }

    #[test]
    fn pattern_to_regex_no_wildcards() {
        let re = pattern_to_regex("output.bam").unwrap();
        assert!(re.is_match("output.bam"));
        assert!(!re.is_match("other.bam"));
    }

    #[test]
    fn discover_wildcards_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SAMPLE_A_R1.fastq.gz"), "").unwrap();
        std::fs::write(dir.path().join("SAMPLE_A_R2.fastq.gz"), "").unwrap();
        std::fs::write(dir.path().join("SAMPLE_B_R1.fastq.gz"), "").unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), "").unwrap();

        let results =
            discover_wildcards_from_pattern(dir.path(), "{sample}_R{read}.fastq.gz").unwrap();
        assert!(results.len() >= 2);
    }

    // -----------------------------------------------------------------------
    // WC-01: Experiment-control pair wildcard tests
    // -----------------------------------------------------------------------

    #[test]
    fn wildcard_combinations_from_pairs_basic() {
        use crate::config::ExperimentControlPair;
        let pairs = vec![
            ExperimentControlPair {
                pair_id: "CASE_001".to_string(),
                experiment: "EXP_01".to_string(),
                control: "CTRL_01".to_string(),
                experiment_type: Some("lung".to_string()),
                metadata: Default::default(),
            },
            ExperimentControlPair {
                pair_id: "CASE_002".to_string(),
                experiment: "EXP_02".to_string(),
                control: "CTRL_02".to_string(),
                experiment_type: None,
                metadata: Default::default(),
            },
        ];
        let combos = wildcard_combinations_from_pairs(&pairs);
        assert_eq!(combos.len(), 2);

        assert_eq!(combos[0]["pair_id"], "CASE_001");
        assert_eq!(combos[0]["experiment"], "EXP_01");
        assert_eq!(combos[0]["control"], "CTRL_01");
        assert_eq!(combos[0]["experiment_type"], "lung");
        // backward-compatible aliases
        assert_eq!(combos[0]["tumor"], "EXP_01");
        assert_eq!(combos[0]["normal"], "CTRL_01");
        assert_eq!(combos[0]["tumor_type"], "lung");

        assert_eq!(combos[1]["pair_id"], "CASE_002");
        assert_eq!(combos[1]["experiment"], "EXP_02");
        assert_eq!(combos[1]["control"], "CTRL_02");
        assert!(!combos[1].contains_key("experiment_type")); // not set
    }

    #[test]
    fn wildcard_combinations_from_pairs_metadata() {
        use crate::config::ExperimentControlPair;
        let mut meta = HashMap::new();
        meta.insert("patient_id".to_string(), "PT-001".to_string());
        let pairs = vec![ExperimentControlPair {
            pair_id: "P1".to_string(),
            experiment: "E1".to_string(),
            control: "C1".to_string(),
            experiment_type: None,
            metadata: meta,
        }];
        let combos = wildcard_combinations_from_pairs(&pairs);
        assert_eq!(combos[0]["patient_id"], "PT-001");
    }

    #[test]
    fn wildcard_combinations_from_pairs_empty() {
        use crate::config::ExperimentControlPair;
        let combos = wildcard_combinations_from_pairs(&[] as &[ExperimentControlPair]);
        assert!(combos.is_empty());
    }

    // -----------------------------------------------------------------------
    // WC-02: Sample group wildcard tests
    // -----------------------------------------------------------------------

    #[test]
    fn wildcard_combinations_from_groups_basic() {
        use crate::config::SampleGroup;
        let groups = vec![
            SampleGroup {
                name: "control".to_string(),
                samples: vec!["S001".to_string(), "S002".to_string()],
                metadata: Default::default(),
            },
            SampleGroup {
                name: "case".to_string(),
                samples: vec!["S003".to_string()],
                metadata: Default::default(),
            },
        ];
        let combos = wildcard_combinations_from_groups(&groups);
        assert_eq!(combos.len(), 3); // 2 control + 1 case

        assert_eq!(combos[0]["group"], "control");
        assert_eq!(combos[0]["sample"], "S001");
        assert_eq!(combos[1]["group"], "control");
        assert_eq!(combos[1]["sample"], "S002");
        assert_eq!(combos[2]["group"], "case");
        assert_eq!(combos[2]["sample"], "S003");
    }

    #[test]
    fn wildcard_combinations_from_groups_metadata() {
        use crate::config::SampleGroup;
        let mut meta = HashMap::new();
        meta.insert("tissue".to_string(), "blood".to_string());
        let groups = vec![SampleGroup {
            name: "grp".to_string(),
            samples: vec!["S1".to_string()],
            metadata: meta,
        }];
        let combos = wildcard_combinations_from_groups(&groups);
        assert_eq!(combos[0]["tissue"], "blood");
    }

    #[test]
    fn wildcard_combinations_from_groups_empty() {
        use crate::config::SampleGroup;
        let combos = wildcard_combinations_from_groups(&[] as &[SampleGroup]);
        assert!(combos.is_empty());
    }
}
