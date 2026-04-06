//! Wildcard pattern expansion for oxo-flow.
//!
//! Supports `{wildcard}` patterns in file paths, expanding them
//! against provided values or input file discovery.

use crate::error::{OxoFlowError, Result};
use regex::Regex;
use std::collections::HashMap;

/// A single wildcard binding, e.g., `sample = "TUMOR_01"`.
pub type WildcardValues = HashMap<String, String>;

/// A set of wildcard value combinations for expanding rules.
pub type WildcardCombinations = Vec<WildcardValues>;

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
    let re = Regex::new(r"\{(\w+)\}").expect("valid regex");
    let mut names = Vec::new();
    for cap in re.captures_iter(pattern) {
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
    let re = Regex::new(r"\{(\w+)\}").expect("valid regex");
    let mut result = pattern.to_string();
    let mut missing = Vec::new();

    for cap in re.captures_iter(pattern) {
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
    let re = Regex::new(r"\{(\w+)\}").expect("valid regex");
    re.is_match(pattern)
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
}
