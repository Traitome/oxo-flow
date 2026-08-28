//! Wildcard pattern expansion for oxo-flow.
//!
//! Supports `{wildcard}` patterns in file paths, expanding them
//! against provided values or input file discovery.

use crate::error::{OxoFlowError, Result};
use regex::Regex;
use std::collections::{HashMap, HashSet};
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

/// Validate wildcard values against pre-compiled regex constraints.
pub fn validate_wildcard_constraints_compiled(
    values: &WildcardValues,
    constraints: &HashMap<String, Regex>,
) -> Result<()> {
    let mut violations = Vec::new();

    for (name, re) in constraints {
        if let Some(value) = values.get(name).filter(|v| !re.is_match(v)) {
            violations.push(format!(
                "wildcard '{}' value '{}' does not match constraint '{}'",
                name,
                value,
                re.as_str()
            ));
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

/// Validate wildcard values against regex constraints.
///
/// NOTE: This function compiles regexes on every call. Use `validate_wildcard_constraints_compiled`
/// for better performance when validating multiple combinations.
pub fn validate_wildcard_constraints(
    values: &WildcardValues,
    constraints: &WildcardConstraints,
) -> Result<()> {
    let mut compiled = HashMap::new();
    for (name, pattern) in constraints {
        match Regex::new(pattern) {
            Ok(re) => {
                compiled.insert(name.clone(), re);
            }
            Err(e) => {
                return Err(OxoFlowError::Wildcard {
                    rule: String::new(),
                    message: format!(
                        "invalid regex constraint '{}' for wildcard '{}': {}",
                        pattern, name, e
                    ),
                });
            }
        }
    }
    validate_wildcard_constraints_compiled(values, &compiled)
}

/// Convert a wildcard pattern (e.g., `{sample}_R{read}.fastq.gz`) to a regex
/// for file discovery against directory listings.
///
/// The `{name}` placeholders are replaced with named capture groups.
pub fn pattern_to_regex(pattern: &str) -> Result<Regex> {
    let mut regex_str = String::from("^");
    let mut last_end = 0;
    // A pattern may repeat the same wildcard (e.g. "consensus/{antibody}/
    // {antibody}.peaks.bed") — the regex crate forbids duplicate capture
    // names, so the first occurrence captures and later ones match
    // anonymously. Anonymous groups alone cannot enforce that both
    // positions hold the SAME value; the discovery walkers add a
    // round-trip check (expand back and compare) for that guarantee.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

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
        if seen.insert(name.to_string()) {
            regex_str.push_str(&format!("(?P<{}>\\S+)", name));
        } else {
            regex_str.push_str("(?:\\S+)");
        }

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
    // Identify which wildcards in the pattern have provided values
    let wildcards = extract_wildcards(pattern);
    let mut active_vars = Vec::new();
    for name in &wildcards {
        if let Some(vals) = variables.get(name).filter(|v| !v.is_empty()) {
            active_vars.push((name.clone(), vals));
        }
    }

    if active_vars.is_empty() {
        // Two very different cases share this branch: a literal pattern
        // (no wildcards at all) round-trips as-is, but a pattern WITH
        // wildcards whose values are all empty/missing has an EMPTY
        // Cartesian product — zero expansions. Returning the raw pattern
        // for the latter injects literal `{gene_set}` tokens into
        // {input} (enrichment port finding, the #199/#239 literal-token
        // family).
        return if wildcards.is_empty() {
            vec![pattern.to_string()]
        } else {
            Vec::new()
        };
    }

    // Pre-calculate pattern parts to avoid string searching/replacement in loops
    #[derive(Debug)]
    enum Part {
        Literal(String),
        Placeholder(usize), // Index into active_vars
    }

    let mut parts = Vec::new();
    let mut last_end = 0;
    for mat in WILDCARD_RE.find_iter(pattern) {
        if mat.start() > last_end {
            parts.push(Part::Literal(pattern[last_end..mat.start()].to_string()));
        }
        let cap = WILDCARD_RE.captures(mat.as_str()).unwrap();
        let name = &cap[1];

        // Find index of this wildcard in active_vars
        if let Some(idx) = active_vars.iter().position(|(n, _)| *n == name) {
            parts.push(Part::Placeholder(idx));
        } else {
            // Not an active variable, keep as literal placeholder
            parts.push(Part::Literal(format!("{{{name}}}")));
        }
        last_end = mat.end();
    }
    if last_end < pattern.len() {
        parts.push(Part::Literal(pattern[last_end..].to_string()));
    }

    // Generate combinations using recursive part assembly
    let mut results = Vec::new();
    let mut current_values = vec![""; active_vars.len()];

    fn generate<'a>(
        var_idx: usize,
        active_vars_list: &[(&String, &'a Vec<String>)],
        current_values: &mut [&'a str],
        parts: &[Part],
        results: &mut Vec<String>,
    ) {
        if var_idx == active_vars_list.len() {
            // All variables assigned, assemble the final string
            let mut assembled = String::with_capacity(256);
            for part in parts {
                match part {
                    Part::Literal(s) => assembled.push_str(s),
                    Part::Placeholder(idx) => {
                        assembled.push_str(current_values[*idx]);
                    }
                }
            }
            results.push(assembled);
            return;
        }

        let (_, vals) = active_vars_list[var_idx];
        for val in vals {
            current_values[var_idx] = val;
            generate(
                var_idx + 1,
                active_vars_list,
                current_values,
                parts,
                results,
            );
        }
    }

    let active_vars_refs: Vec<(&String, &Vec<String>)> =
        active_vars.iter().map(|(n, v)| (n, *v)).collect();

    generate(
        0,
        &active_vars_refs,
        &mut current_values,
        &parts,
        &mut results,
    );

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
    let mut seen = HashSet::new();

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
                // Round-trip guard against phantom instances: with a
                // repeated wildcard the regex's anonymous groups may
                // capture a DIFFERENT value than the named one (e.g.
                // "{s}.{s}.bam" regex-matches "B.A.bam" with s="B"). A
                // combo whose expansion differs from the matched file
                // name can only come from such a mismatch — skip it.
                let is_real =
                    expand_pattern(pattern, &values).is_ok_and(|expanded| expanded == filename);
                if !values.is_empty() && is_real && seen.insert(wildcard_combo_key(&values)) {
                    results.push(values);
                }
            }
        }
    }

    Ok(results)
}

/// Discover wildcard combinations by walking a directory TREE, matching
/// each file's path (relative to `dir`, `/`-separated) against `pattern`.
///
/// The single-directory [`discover_wildcards_from_pattern`] only ever
/// sees bare file names, so patterns with literal directory components
/// (`results/adapterremoval/{sample}_{lane}_R1.fastq.gz`) can never
/// match. This walker resolves those — the filesystem source of the
/// per-sample grouping primitive (`input_groups`, issue #227 item 3).
///
/// Patterns WITHOUT a directory component deliberately keep the
/// single-directory semantics (no surprise matches in nested folders),
/// so existing `sample_pattern`-style discovery is unchanged.
pub fn discover_wildcards_from_pattern_tree(
    dir: &std::path::Path,
    pattern: &str,
) -> Result<WildcardCombinations> {
    if !pattern.contains('/') {
        return discover_wildcards_from_pattern(dir, pattern);
    }
    let re = pattern_to_regex(pattern)?;
    let wildcard_names = extract_wildcards(pattern);
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    // Walk the tree; only files whose relative path matches the full
    // pattern contribute. Symlinks are followed, unreadable subtrees are
    // skipped (best-effort, matching the flat walker's stance).
    fn walk(
        dir: &std::path::Path,
        base: &std::path::Path,
        pattern: &str,
        re: &Regex,
        wildcard_names: &[String],
        seen: &mut HashSet<String>,
        results: &mut Vec<WildcardValues>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().ok();
            if file_type.is_some_and(|t| t.is_dir()) {
                walk(&path, base, pattern, re, wildcard_names, seen, results);
            } else if file_type.is_none_or(|t| t.is_file() || t.is_symlink()) {
                let rel = match path.strip_prefix(base) {
                    Ok(rel) => rel,
                    Err(_) => continue,
                };
                let rel_str = rel
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                if let Some(captures) = re.captures(&rel_str) {
                    let mut values = WildcardValues::new();
                    for name in wildcard_names {
                        if let Some(m) = captures.name(name) {
                            values.insert(name.clone(), m.as_str().to_string());
                        }
                    }
                    // Round-trip guard against phantom instances — same
                    // as the flat walker: a repeated wildcard's anonymous
                    // regex groups may capture a value different from the
                    // named one, so only accept combos that re-expand to
                    // the matched relative path.
                    let is_real =
                        expand_pattern(pattern, &values).is_ok_and(|expanded| expanded == rel_str);
                    if !values.is_empty() && is_real && seen.insert(wildcard_combo_key(&values)) {
                        results.push(values);
                    }
                }
            }
        }
    }
    walk(
        dir,
        dir,
        pattern,
        &re,
        &wildcard_names,
        &mut seen,
        &mut results,
    );

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
    let mut seen = HashSet::new();
    for cap in WILDCARD_RE.captures_iter(pattern) {
        let name = cap[1].to_string();
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names
}

/// Compact dedup key for a wildcard combo: sorted `key=value` parts joined
/// by commas — the same canonical form the discovery walkers build, so a
/// rediscovered combo can never double-contribute (issue #227 item 5).
#[must_use]
pub(crate) fn wildcard_combo_key(combo: &WildcardValues) -> String {
    let mut parts: Vec<(&str, &str)> = combo
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    parts.sort_by_key(|(k, _)| *k);
    parts.iter().flat_map(|(k, v)| [*k, "=", *v, ","]).collect()
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
    if !pattern.contains('{') {
        return Ok(pattern.to_string());
    }

    let mut result = String::with_capacity(pattern.len() + 32);
    let mut last_end = 0;

    for mat in WILDCARD_RE.find_iter(pattern) {
        result.push_str(&pattern[last_end..mat.start()]);

        let cap = WILDCARD_RE.captures(mat.as_str()).unwrap();
        let name = &cap[1];

        match values.get(name) {
            Some(value) => {
                result.push_str(value);
            }
            None => {
                // If wildcard is not found, keep it as-is for later expansion
                // (e.g. {threads}, {input}, {memory}, etc.)
                result.push('{');
                result.push_str(name);
                result.push('}');
            }
        }
        last_end = mat.end();
    }

    result.push_str(&pattern[last_end..]);
    Ok(result)
}

/// Returns `true` if the pattern contains any wildcard placeholders.
pub fn has_wildcards(pattern: &str) -> bool {
    WILDCARD_RE.is_match(pattern)
}

/// Sanitize a wildcard value for use in an expanded rule instance name.
///
/// Non-alphanumeric characters become `_`, so `1.5` → `1_5` and
/// `K=21` → `K_21`. Instance names must be shell-safe and unambiguous
/// (`[[values]]` fan-out, e.g. `assemble_assembler_spades`).
#[must_use = "sanitizing returns a new String"]
pub fn sanitize_instance_value(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Returns `true` if the pattern references the `{values.name}` namespace.
///
/// The engine's placeholder regex only matches `\w+` names, so the
/// namespaced form must be detected and substituted textually — see
/// [`expand_values_namespace`].
#[must_use]
pub fn contains_values_namespace(pattern: &str) -> bool {
    pattern.contains("{values.")
}

/// Expand `{values.name}` namespace placeholders with the given bindings.
///
/// `{values.assembler}` is the namespaced sibling of the bare `{assembler}`
/// form; both expand to the same value. Unbound names are left untouched —
/// the caller validates them separately.
#[must_use = "expanding returns a new String"]
pub fn expand_values_namespace(pattern: &str, bindings: &WildcardValues) -> String {
    if !pattern.contains("{values.") {
        return pattern.to_string();
    }
    let mut out = pattern.to_string();
    for (name, value) in bindings {
        out = out.replace(&format!("{{values.{name}}}"), value);
    }
    out
}

/// The per-sample metadata table: sample id → column → value
/// (issue #227 item 2).
pub type MetadataTable =
    std::collections::HashMap<String, std::collections::HashMap<String, String>>;

/// The sample-like instance bindings tried, in order, when resolving
/// `{meta.<column>}` for an instance: the `{sample}` binding first (the
/// common group/sample fan-out), then the pair vocabulary for pair
/// workflows (rows keyed by pair_id or by experiment/control names).
pub const METADATA_LOOKUP_KEYS: &[&str] = &["sample", "pair_id", "experiment", "control"];

/// Resolve an instance's metadata row: the first non-empty sample-like
/// binding that has a row in the table. `None` when no binding matches —
/// the instance renders every `{meta.<column>}` reference empty.
#[must_use]
pub fn metadata_row_for<'a>(
    bindings: &WildcardValues,
    table: &'a MetadataTable,
) -> Option<&'a std::collections::HashMap<String, String>> {
    METADATA_LOOKUP_KEYS.iter().find_map(|key| {
        let id = bindings.get(*key)?;
        if id.is_empty() {
            return None;
        }
        table.get(id)
    })
}

/// The union of column names defined by any metadata row — the known
/// `{meta.<column>}` vocabulary for plan-time typo warnings.
#[must_use]
pub fn metadata_columns(table: &MetadataTable) -> std::collections::HashSet<String> {
    table.values().flat_map(|row| row.keys().cloned()).collect()
}

/// Expand `{meta.<column>}` namespace placeholders for one instance.
///
/// The metadata row is resolved from the instance's sample-like binding
/// ([`metadata_row_for`]); a column present on the row substitutes its
/// value, and a missing row OR column substitutes an empty string (the
/// `when = "config.single_end_mode || {meta.endedness} == 'SE'"` gate
/// therefore evaluates false for samples without the data — the gate is
/// closed, never a literal token). Unresolvable references in rules that
/// never fan out (no sample-like binding at all) stay untouched; the
/// execution-time residual-placeholder guard warns about those.
#[must_use = "expanding returns a new String"]
pub fn expand_meta_namespace(
    text: &str,
    table: &MetadataTable,
    bindings: &WildcardValues,
) -> String {
    if !text.contains("{meta.") {
        return text.to_string();
    }
    let row = metadata_row_for(bindings, table);
    crate::config::META_NS_RE
        .replace_all(text, |caps: &regex::Captures| {
            let column = &caps[1];
            row.and_then(|r| r.get(column)).cloned().unwrap_or_default()
        })
        .into_owned()
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

    // Pre-calculate total combinations for single allocation
    let total = keys
        .iter()
        .map(|k| wildcard_lists[*k].len().max(1))
        .product::<usize>()
        .max(1);

    // Guard against combinatorial explosion (e.g., 5 wildcards × 1000 values = 10^15)
    const MAX_COMBINATIONS: usize = 100_000;
    if total > MAX_COMBINATIONS {
        tracing::warn!(
            "wildcard expansion would produce {total} combinations (exceeds limit of {MAX_COMBINATIONS}). \
             This may cause high memory usage. Consider reducing wildcard values or splitting the workflow."
        );
    }
    let alloc_size = total.min(MAX_COMBINATIONS);

    let mut combinations: WildcardCombinations = Vec::with_capacity(alloc_size);
    combinations.push(HashMap::with_capacity(keys.len()));

    for key in &keys {
        let values = &wildcard_lists[*key];
        if values.is_empty() {
            continue;
        }
        let prev_len = combinations.len();
        let value_count = values.len();

        // Pre-allocate space for expanded combinations
        combinations.reserve(prev_len * (value_count - 1));

        // Clone existing combinations for additional values (index 1..)
        for (_, value) in values.iter().enumerate().skip(1) {
            for j in 0..prev_len {
                let mut new_combo = combinations[j].clone();
                new_combo.insert((*key).clone(), value.clone());
                combinations.push(new_combo);
            }
        }

        // Update original combinations in-place with first value
        for combo in &mut combinations[..prev_len] {
            combo.insert((*key).clone(), values[0].clone());
        }
    }

    combinations
}

/// Extract wildcard names from multiple patterns.
pub fn extract_wildcards_from_patterns(patterns: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for pattern in patterns {
        for name in extract_wildcards(pattern) {
            if seen.insert(name.clone()) {
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
///         control: Some("CTRL_01".to_string()),
///         experiment_type: Some("lung".to_string()),
///         metadata: Default::default(),
///         when: None,
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
            values.insert(
                "control".to_string(),
                pair.control.clone().unwrap_or_default(),
            );
            // Backward-compatible aliases
            values.insert("tumor".to_string(), pair.experiment.clone());
            values.insert(
                "normal".to_string(),
                pair.control.clone().unwrap_or_default(),
            );
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
    fn cartesian_expand_empty_values_yield_empty_product() {
        // A pattern WITH wildcards whose variable lists are all empty has
        // an EMPTY Cartesian product — zero expansions. Returning the raw
        // pattern instead injects a literal `{gene_set}` token into
        // {input} (enrichment port finding, the #199/#239 literal-token
        // family).
        let mut variables = HashMap::new();
        variables.insert("gene_set".to_string(), Vec::new());
        assert!(
            cartesian_expand("{gene_set}", &variables).is_empty(),
            "wildcards with empty values must expand to nothing"
        );

        // A literal pattern (no wildcards at all) still round-trips as-is.
        let mut empty = HashMap::new();
        empty.insert("gene_set".to_string(), Vec::new());
        assert_eq!(
            cartesian_expand("output.bam", &empty),
            vec!["output.bam".to_string()]
        );
    }

    #[test]
    fn sanitize_instance_value_alphanumeric_unchanged() {
        assert_eq!(sanitize_instance_value("spades"), "spades");
        assert_eq!(sanitize_instance_value("21"), "21");
        assert_eq!(sanitize_instance_value("NA12878"), "NA12878");
    }

    #[test]
    fn sanitize_instance_value_replaces_special_chars() {
        assert_eq!(sanitize_instance_value("1.5"), "1_5");
        assert_eq!(sanitize_instance_value("K=21"), "K_21");
        assert_eq!(sanitize_instance_value("beta-1"), "beta_1");
    }

    #[test]
    fn contains_values_namespace_detects_only_namespaced_form() {
        assert!(contains_values_namespace(
            "results/{values.assembler}/x.txt"
        ));
        assert!(!contains_values_namespace("results/{assembler}/x.txt"));
        assert!(!contains_values_namespace("plain.txt"));
    }

    #[test]
    fn expand_values_namespace_substitutes_bound_names() {
        let mut bindings = WildcardValues::new();
        bindings.insert("assembler".to_string(), "spades".to_string());
        bindings.insert("k".to_string(), "21".to_string());
        assert_eq!(
            expand_values_namespace("results/{values.assembler}/k{values.k}.txt", &bindings),
            "results/spades/k21.txt"
        );
        // Without any namespaced reference the input is returned as-is.
        assert_eq!(expand_values_namespace("plain.txt", &bindings), "plain.txt");
        // Bare `{assembler}` is not touched — expand_pattern handles it.
        assert_eq!(
            expand_values_namespace("results/{assembler}/x.txt", &bindings),
            "results/{assembler}/x.txt"
        );
    }

    #[test]
    fn expand_values_namespace_leaves_unbound_names() {
        let mut bindings = WildcardValues::new();
        bindings.insert("assembler".to_string(), "spades".to_string());
        assert_eq!(
            expand_values_namespace("results/{values.k}/x.txt", &bindings),
            "results/{values.k}/x.txt"
        );
    }

    #[test]
    fn expand_pattern_missing_wildcard() {
        // Permissive expansion: unknown wildcards are kept as-is for later expansion
        // (e.g., {threads}, {input}, {memory}, etc.)
        let values = WildcardValues::new();
        let result = expand_pattern("{sample}.bam", &values).unwrap();
        assert_eq!(result, "{sample}.bam");
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
    fn pattern_to_regex_repeated_wildcard() {
        // "consensus/{antibody}/{antibody}.peaks.bed" repeats the wildcard
        // — the first occurrence captures, later ones match anonymously
        // (regex crate: no duplicate capture names).
        let re = pattern_to_regex("consensus/{antibody}/{antibody}.peaks.bed").unwrap();
        assert!(re.is_match("consensus/H3K4me3/H3K4me3.peaks.bed"));
        let caps = re.captures("consensus/H3K4me3/H3K4me3.peaks.bed").unwrap();
        assert_eq!(&caps["antibody"], "H3K4me3");
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

    #[test]
    fn discover_wildcards_from_pattern_tree_matches_subdirectories() {
        // The recursive walker resolves patterns with literal directory
        // components (`results/adapterremoval/{sample}_{lane}_R1.fastq.gz`),
        // which the single-directory walker cannot see — the engine-level
        // input_groups primitive (issue #227 item 3) scans with this.
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("results/adapterremoval");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("S1_L1_R1.fastq.gz"), "").unwrap();
        std::fs::write(raw.join("S1_L2_R1.fastq.gz"), "").unwrap();
        std::fs::write(raw.join("S2_L1_R1.fastq.gz"), "").unwrap();
        // A decoy outside the literal directory must not match.
        std::fs::write(dir.path().join("S1_L9_R1.fastq.gz"), "").unwrap();

        let results = discover_wildcards_from_pattern_tree(
            dir.path(),
            "results/adapterremoval/{sample}_{lane}_R1.fastq.gz",
        )
        .unwrap();
        let mut combos: Vec<_> = results
            .into_iter()
            .map(|c| (c["sample"].clone(), c["lane"].clone()))
            .collect();
        combos.sort();
        assert_eq!(
            combos,
            vec![
                ("S1".to_string(), "L1".to_string()),
                ("S1".to_string(), "L2".to_string()),
                ("S2".to_string(), "L1".to_string()),
            ]
        );

        // Patterns without a directory component keep the single-directory
        // semantics — no surprise matches in nested folders.
        let flat = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(flat.path().join("sub")).unwrap();
        std::fs::write(flat.path().join("sub/S1_L1_R1.fastq.gz"), "").unwrap();
        let results =
            discover_wildcards_from_pattern_tree(flat.path(), "{sample}_{lane}_R1.fastq.gz")
                .unwrap();
        assert!(results.is_empty(), "flat pattern must not scan recursively");
    }

    #[test]
    fn discover_wildcards_from_pattern_tree_rejects_mismatched_repeated_wildcard() {
        // A repeated wildcard must hold the SAME value at every position:
        // "consensus/{antibody}/{antibody}.peaks.bed" may regex-match
        // "consensus/H3K4me3/H3K27ac.peaks.bed" (the second position is an
        // anonymous group), but that combo re-expands to a path that does
        // not exist — it must not surface as a phantom instance. The
        // H3K27ac phantom has NO consistent counterpart file, so dedup
        // alone cannot hide it.
        let dir = tempfile::tempdir().unwrap();
        let consensus = dir.path().join("consensus");
        std::fs::create_dir_all(consensus.join("H3K4me3")).unwrap();
        std::fs::create_dir_all(consensus.join("H3K27ac")).unwrap();
        std::fs::write(consensus.join("H3K4me3/H3K4me3.peaks.bed"), "").unwrap();
        // Mixed names: the two positions disagree — must be rejected.
        std::fs::write(consensus.join("H3K27ac/H3K4me3.peaks.bed"), "").unwrap();

        let results = discover_wildcards_from_pattern_tree(
            dir.path(),
            "consensus/{antibody}/{antibody}.peaks.bed",
        )
        .unwrap();
        let combos: Vec<_> = results.into_iter().map(|c| c["antibody"].clone()).collect();
        assert_eq!(combos, vec!["H3K4me3".to_string()]);
    }

    #[test]
    fn discover_wildcards_from_pattern_rejects_mismatched_repeated_wildcard() {
        // Same guarantee for the flat walker: "{s}.{s}.bam" must not yield
        // a combo from "B.A.bam" (captured value "B" has no consistent
        // counterpart file, so dedup alone cannot hide the phantom).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("A.A.bam"), "").unwrap();
        std::fs::write(dir.path().join("B.A.bam"), "").unwrap();

        let results = discover_wildcards_from_pattern(dir.path(), "{s}.{s}.bam").unwrap();
        let combos: Vec<_> = results.into_iter().map(|c| c["s"].clone()).collect();
        assert_eq!(combos, vec!["A".to_string()]);
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
                control: Some("CTRL_01".to_string()),
                experiment_type: Some("lung".to_string()),
                metadata: Default::default(),
                when: None,
            },
            ExperimentControlPair {
                pair_id: "CASE_002".to_string(),
                experiment: "EXP_02".to_string(),
                control: Some("CTRL_02".to_string()),
                experiment_type: None,
                metadata: Default::default(),
                when: None,
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
            control: Some("C1".to_string()),
            experiment_type: None,
            metadata: meta,
            when: None,
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
