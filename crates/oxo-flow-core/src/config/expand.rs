//! Wildcard expansion engine. (issue #206 extraction).
//! Workflow configuration and `.oxoflow` file parsing.
// Accesses deprecated `Rule::threads` / `Rule::memory` shorthand fields to
// apply defaults and expand rules.  Will be removed once the shorthand
// fields are retired.
#![allow(deprecated)]
//!
//! The `.oxoflow` format is TOML-based with workflow metadata, configuration
//! variables, default settings, and a list of rules.

use super::*;
use crate::error::{OxoFlowError, Result};
use crate::rule::{EnvironmentSpec, FilePatterns, Rule};
use std::collections::HashMap;

/// Matches the namespaced `{values.name}` wildcard form.
///
/// The engine's placeholder regex (`\w+` only) cannot match dotted names, so
/// this namespace is detected and substituted textually — see
impl WorkflowConfig {
    /// Expand rules that contain pair or group wildcards into concrete instances.
    ///
    /// Scans each rule for wildcard placeholders:
    /// - Rules containing `{experiment}`, `{control}`, or `{pair_id}` are
    ///   expanded once per entry in `self.pairs`.
    /// - Backward-compatible aliases `{tumor}` and `{normal}` are also
    ///   recognized.
    /// - Rules containing `{group}` or `{sample}` are expanded once per
    ///   (group, sample) combination in `self.sample_groups`.
    /// - Rules without any of these wildcards are kept unchanged.
    ///
    /// The expanded rule names follow the pattern `{original_name}_{suffix}`,
    /// where the suffix is the `pair_id` for pair rules or `{group}_{sample}`
    /// for group rules.
    ///
    /// After calling this method, `self.rules` contains only concrete rules
    /// (no pair/group wildcards) and the DAG can be built normally.
    ///
    /// # Errors
    ///
    /// Returns an error if duplicate rule names would be produced (e.g., two
    /// pairs with the same `pair_id`), or if a pair/group is defined but no
    /// rules reference its wildcards (this is not an error—those pairs are
    /// simply ignored).
    /// Build the `wildcard.<key>` evaluation context for one expansion combo.
    ///
    /// Optional pair wildcards (`experiment_type`, `tumor_type`) are filled
    /// with empty strings so `wildcard.experiment_type != ''` predicates see
    /// a definite value (missing keys otherwise evaluate false). Metadata
    /// keys flow in through the combo itself.
    fn expansion_when_context(combo: &crate::wildcard::WildcardValues) -> HashMap<String, String> {
        let mut ctx: HashMap<String, String> =
            combo.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for key in [
            "pair_id",
            "experiment",
            "control",
            "tumor",
            "normal",
            "experiment_type",
            "tumor_type",
            "group",
            "sample",
        ] {
            ctx.entry(key.to_string()).or_default();
        }
        ctx
    }

    /// Resolve a kept instance's `when` wildcard references to that
    /// instance's own bindings, so the execution-time re-check re-evaluates
    /// the same per-instance verdict without a wildcard context.
    ///
    /// - `wildcard.<key>` as a comparison operand → the bound value as a
    ///   quoted literal (the evaluator compares literals properly).
    /// - a bare `wildcard.<key>` (truthiness position) → the literal
    ///   `true`/`false` for the bound value.
    /// - keys absent from the combo are left untouched: they were
    ///   non-decisive for a kept instance, and the strict unbound→false
    ///   semantics at execution mirror the expansion-time verdict.
    ///
    /// Without this, the strict missing-key semantics added to the `when`
    /// evaluator would veto every fan-out instance at execution time (no
    /// pair/group context exists there) — the live snparcher incident was
    /// the reverse failure: an unbound key evaluated TRUE at expansion.
    pub(crate) fn bake_wildcard_when(
        when: &str,
        combo: &crate::wildcard::WildcardValues,
    ) -> String {
        // Longest-first so `>=` wins over `>`, `==`/`!=` are not confused
        // with `=`/`!` (which are not comparison operators here).
        const OPS: &[&str] = &[">=", "<=", "==", "!=", ">", "<"];

        let mut result = String::with_capacity(when.len());
        let mut cursor = 0usize;
        for cap in WHEN_WILDCARD_REF_RE.captures_iter(when) {
            let m = cap.get(0).expect("full match");
            let key = cap.get(1).expect("key group").as_str();
            let (start, end) = (m.start(), m.end());
            result.push_str(&when[cursor..start]);

            // Non-space context around the token decides its position.
            let after = when[end..]
                .find(|c: char| !c.is_whitespace())
                .map(|i| &when[end + i..])
                .unwrap_or_default();
            let before = when[..start].trim_end();

            match combo.get(key) {
                // Comparison operand → quoted literal of the bound value.
                Some(value)
                    if OPS
                        .iter()
                        .any(|op| after.starts_with(op) || before.ends_with(op)) =>
                {
                    let rendered = crate::executor::process::render_wildcard_value(value);
                    if rendered.contains('\'') {
                        result.push_str(&format!("\"{rendered}\""));
                    } else {
                        result.push_str(&format!("'{rendered}'"));
                    }
                }
                // Bare truthiness position → literal true/false.
                Some(value) => {
                    let truthy = !value.is_empty() && value != "false" && value != "0";
                    result.push_str(if truthy { "true" } else { "false" });
                }
                // Unbound: leave the token for the strict execution-time
                // evaluator (false — same verdict as expansion).
                None => result.push_str(&when[start..end]),
            }
            cursor = end;
        }
        result.push_str(&when[cursor..]);
        result
    }

    /// Per-instance `{meta.<column>}` substitution for `when` expressions,
    /// mirroring [`Self::bake_wildcard_when`] so the execution-time re-check
    /// re-evaluates the same per-instance verdict:
    ///
    /// - comparison operand → the metadata value as a quoted literal
    ///   (`'SE' == 'SE'` — the evaluator's literal comparison requires
    ///   quotes on both sides),
    /// - bare truthiness position → `true`/`false` for the value,
    /// - missing row OR column → `''` in a comparison (a closed gate) and
    ///   `false` in a truthiness position — never a bare token, which the
    ///   evaluator's default-true fallback would run.
    fn bake_meta_when(
        when: &str,
        table: &crate::wildcard::MetadataTable,
        combo: &crate::wildcard::WildcardValues,
    ) -> String {
        const OPS: &[&str] = &[">=", "<=", "==", "!=", ">", "<"];

        let row = crate::wildcard::metadata_row_for(combo, table);
        let mut result = String::with_capacity(when.len());
        let mut cursor = 0usize;
        for cap in crate::config::META_NS_RE.captures_iter(when) {
            let m = cap.get(0).expect("full match");
            let column = cap.get(1).expect("column group").as_str();
            let (start, end) = (m.start(), m.end());
            result.push_str(&when[cursor..start]);

            // Non-space context around the token decides its position.
            let after = when[end..]
                .find(|c: char| !c.is_whitespace())
                .map(|i| &when[end + i..])
                .unwrap_or_default();
            let before = when[..start].trim_end();
            let in_comparison = OPS
                .iter()
                .any(|op| after.starts_with(op) || before.ends_with(op));

            match row.and_then(|r| r.get(column)) {
                // Comparison operand → quoted literal of the metadata value.
                Some(value) if in_comparison => {
                    let rendered = crate::executor::process::render_wildcard_value(value);
                    if rendered.contains('\'') {
                        result.push_str(&format!("\"{rendered}\""));
                    } else {
                        result.push_str(&format!("'{rendered}'"));
                    }
                }
                // Bare truthiness position → literal true/false.
                Some(value) => {
                    let truthy = !value.is_empty() && value != "false" && value != "0";
                    result.push_str(if truthy { "true" } else { "false" });
                }
                // Missing row or column: a closed gate, never a bare token.
                None if in_comparison => result.push_str("''"),
                None => result.push_str("false"),
            }
            cursor = end;
        }
        result.push_str(&when[cursor..]);
        result
    }

    pub fn expand_wildcards(&mut self) -> Result<()> {
        // Preserve the unexpanded templates on first expansion — checkpoint
        // re-entry (issue #78 P3) re-expands from them with merged values.
        if self.rule_templates.is_empty() {
            self.rule_templates = self.rules.clone();
        }
        use crate::wildcard::{
            expand_pattern, has_wildcards, validate_wildcard_constraints_compiled,
            wildcard_combinations_from_groups, wildcard_combinations_from_pairs,
        };
        use regex::Regex;

        let pair_combos = wildcard_combinations_from_pairs(&self.pairs);
        let group_combos = wildcard_combinations_from_groups(&self.sample_groups);

        // Rebuild expansion provenance from scratch — this method may run on a
        // config that was expanded before.
        self.expansion_samples.clear();
        self.expansion_values.clear();
        self.expansion_pairs.clear();

        // Validate [[values]] tables: non-empty names/values, unique names,
        // and no collisions with built-in wildcards (a rule referencing
        // `{sample}` must not be ambiguous between group and value fan-out).
        let mut seen_value_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        const RESERVED_VALUE_NAMES: &[&str] = &[
            "sample",
            "group",
            "pair_id",
            "experiment",
            "control",
            "tumor",
            "normal",
            "experiment_type",
            "tumor_type",
            // Executor placeholders — a value table named `input` (etc.)
            // would replace the placeholder in every rule's shell.
            "input",
            "output",
            "log",
            "threads",
            "memory",
        ];
        for table in &self.values {
            if table.name.is_empty() {
                return Err(OxoFlowError::Validation {
                    message: "[[values]] table must have a non-empty name".to_string(),
                    rule: None,
                    suggestion: None,
                });
            }
            if table.values.is_empty() {
                return Err(OxoFlowError::Validation {
                    message: format!("[[values]] table '{}' has no values", table.name),
                    rule: None,
                    suggestion: Some("add at least one value, or remove the table".to_string()),
                });
            }
            if RESERVED_VALUE_NAMES.contains(&table.name.as_str()) {
                return Err(OxoFlowError::Validation {
                    message: format!(
                        "[[values]] name '{}' collides with a built-in wildcard",
                        table.name
                    ),
                    rule: None,
                    suggestion: Some(
                        "rename the table (e.g. use a tool-specific parameter name)".to_string(),
                    ),
                });
            }
            if !seen_value_names.insert(table.name.clone()) {
                return Err(OxoFlowError::Validation {
                    message: format!("duplicate [[values]] table '{}'", table.name),
                    rule: None,
                    suggestion: Some("merge the tables or rename one of them".to_string()),
                });
            }
        }

        // Pre-compile constraints for performance
        let mut compiled_constraints = HashMap::new();
        for (name, pattern) in &self.wildcard_constraints {
            let re = Regex::new(pattern).map_err(|e| OxoFlowError::Wildcard {
                rule: String::new(),
                message: format!(
                    "invalid regex constraint '{}' for wildcard '{}': {}",
                    pattern, name, e
                ),
            })?;
            compiled_constraints.insert(name.clone(), re);
        }

        // Wildcards that trigger pair expansion.
        // Include backward-compatible aliases `{tumor}`/`{normal}`.
        let mut pair_wildcards = vec![
            "experiment",
            "control",
            "tumor",
            "normal",
            "pair_id",
            "experiment_type",
            "tumor_type",
        ];
        // Also include any metadata keys from defined pairs
        for pair in &self.pairs {
            for key in pair.metadata.keys() {
                pair_wildcards.push(key.as_str());
            }
        }

        // Wildcards that trigger group expansion
        const GROUP_WILDCARDS: &[&str] = &["group", "sample"];

        // The known `{meta.<column>}` vocabulary (issue #227 item 2): the
        // union of columns any metadata row defines. Used for the
        // plan-time typo warning below.
        let metadata_columns: std::collections::HashSet<String> =
            crate::wildcard::metadata_columns(&self.metadata);

        let mut expanded_rules: Vec<Rule> = Vec::new();
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Track original → expanded name mapping for depends_on resolution
        let mut name_map: HashMap<String, Vec<String>> = HashMap::new();
        // input_groups rules (issue #227 item 3): fanned out in the
        // post-loop pass below, where producer outputs are known.
        let mut pending_input_groups: Vec<Rule> = Vec::new();

        // output_pattern plan-time registration (issue #227 item 5): the
        // FRESH wildcard vocabulary — wildcards of every declared
        // `output_pattern` that no existing fan-out source binds (pairs,
        // groups/{sample}, [[values]] tables). Rules referencing a fresh
        // wildcard cannot be instantiated until the producer has run and
        // the domain has been discovered: they are deferred to runtime.
        // One producer per fresh wildcard (v1); chained producers (a rule
        // consuming one fresh wildcard and producing another) are legal.
        let mut fresh_wildcards: HashMap<String, String> = HashMap::new();
        for rule in &self.rules {
            if rule.output_pattern.is_none() {
                continue;
            }
            rule.validate_output_pattern()?;
            let wildcards = crate::wildcard::extract_wildcards(
                rule.output_pattern.as_deref().unwrap_or_default(),
            );
            for w in wildcards {
                if pair_wildcards.contains(&w.as_str())
                    || GROUP_WILDCARDS.contains(&w.as_str())
                    || self.values.iter().any(|v| v.name == w)
                    || w.starts_with("meta.")
                    || w.starts_with("config.")
                {
                    continue;
                }
                if let Some(prev) = fresh_wildcards.get(&w) {
                    if prev == &rule.name {
                        continue;
                    }
                    // Cross-reference (chain): a producer whose pattern
                    // reuses an ALREADY-declared fresh wildcard is legal
                    // only when the rule consumes it (the wildcard appears
                    // in its inputs/shell/… and is baked per instance). A
                    // bare redeclaration would claim the same vocabulary
                    // for two producers.
                    let refs = consumer_scan_text(rule);
                    let consumes = refs.iter().any(|t| t.contains(&format!("{{{w}}}")));
                    if !consumes {
                        return Err(OxoFlowError::Validation {
                            message: format!(
                                "output_pattern rules '{prev}' and '{}' both declare the fresh \
                                 wildcard '{{{w}}}'; one producer per fresh wildcard (v1)",
                                rule.name
                            ),
                            rule: Some(rule.name.clone()),
                            suggestion: Some(
                                "rename one wildcard, or declare a single rule whose pattern \
                                 covers both domains"
                                    .to_string(),
                            ),
                        });
                    }
                    continue;
                }
                fresh_wildcards.insert(w.clone(), rule.name.clone());
            }
        }
        self.output_pattern_producers = fresh_wildcards.clone();
        // Rebuilt from scratch on every expansion (re-expansion included);
        // runtime-instantiated consumers re-enter the pending set only when
        // their domain is still empty.
        self.pending_output_pattern.clear();

        for rule in &self.rules {
            if !rule.input_groups.is_empty() {
                // Per-sample multi-file grouping (issue #227 item 3 — the
                // groupTuple pattern). The fan-out runs in a post-loop pass
                // once every producer's expanded literal outputs are known:
                // group enumeration must see the files the workflow ITSELF
                // will produce, not only files that pre-exist on disk. Rules
                // declaring input_groups never fan out on pair/group/value
                // wildcards — the discovered group key IS the instance's
                // binding source; any wildcard the instance map does not
                // bind stays literal and hits the execution-time
                // residual-placeholder guard (loud, attributable).
                pending_input_groups.push(rule.clone());
                continue;
            }

            // Collect all text fields that might contain wildcards. The fan-out
            // TRIGGER set is input/output/shell only — script and the hooks
            // substitute per instance when the rule fans out, but never start
            // a fan-out themselves (cloning on a hook-only wildcard would
            // duplicate the whole rule execution, and `${name}` bash
            // spellings inside script would false-trigger). `when` joins the
            // trigger set: a rule whose pair/group scope is expressed only in
            // `when` (snakemake-style per-sample DAG morphing, e.g.
            // `when = "wildcard.control != ''"`) still fans out per combo.
            let mut all_text: Vec<&str> = rule.input.iter().map(String::as_str).collect();
            all_text.extend(rule.output.iter().map(String::as_str));
            if let Some(ref shell) = rule.shell {
                all_text.push(shell);
            }
            if let Some(ref when) = rule.when {
                all_text.push(when);
            }
            // The fan-out TRIGGER text: `all_text` plus the rule's own
            // `output_pattern` (issue #227 item 5). A producer fans out on
            // its pattern's BOUND wildcards ({sample}, {assembler}, …) so
            // every instance scans only its own slice of the filesystem;
            // its fresh wildcard stays unbound through expansion.
            let mut trigger_text: Vec<&str> = all_text.clone();
            if let Some(ref op) = rule.output_pattern {
                trigger_text.push(op);
            }

            // `{meta.<column>}` plan-time typo check (issue #227 item 2): a
            // column no row defines renders empty on every instance — warn
            // once per rule+column so the author notices at plan time,
            // matching the `{values.name}` stance (warn, never error).
            // Free-text fields (log, script, hooks) are scanned too.
            if !metadata_columns.is_empty() || all_text.iter().any(|t| t.contains("{meta.")) {
                let mut meta_texts: Vec<&str> = all_text.clone();
                if let Some(ref log) = rule.log {
                    meta_texts.push(log);
                }
                for text in [
                    &rule.script,
                    &rule.pre_exec,
                    &rule.on_success,
                    &rule.on_failure,
                ]
                .into_iter()
                .flatten()
                {
                    meta_texts.push(text);
                }
                let mut warned_columns: Vec<String> = Vec::new();
                for text in meta_texts {
                    for cap in META_NS_RE.captures_iter(text) {
                        let column = &cap[1];
                        if !metadata_columns.contains(column)
                            && !warned_columns.iter().any(|w| w == column)
                        {
                            tracing::warn!(
                                rule = %rule.name,
                                column,
                                "rule references '{{meta.{column}}}' but no metadata row defines a column named '{column}' — the placeholder will render empty"
                            );
                            warned_columns.push(column.to_string());
                        }
                    }
                }
            }

            let uses_pair_wildcard = !pair_combos.is_empty()
                && trigger_text.iter().any(|t| {
                    pair_wildcards
                        .iter()
                        .any(|w| t.contains(&format!("{{{w}}}")))
                });

            let uses_group_wildcard = !group_combos.is_empty()
                && trigger_text.iter().any(|t| {
                    GROUP_WILDCARDS
                        .iter()
                        .any(|w| t.contains(&format!("{{{w}}}")))
                });

            // `[[values]]` fan-out: tables whose names appear in the rule —
            // in inputs/outputs/shells and in `expand_inputs` patterns —
            // become an additional Cartesian dimension, orthogonal to pairs
            // and groups. `{values.name}` is the namespaced sibling of the
            // bare `{name}` form.
            let expand_texts: Vec<&str> = all_text
                .iter()
                .copied()
                .chain(rule.expand_inputs.iter().map(|e| e.pattern.as_str()))
                .collect();
            let mut active_value_tables: Vec<&ValueGroup> = Vec::new();
            for table in &self.values {
                let referenced = trigger_text.iter().any(|t| {
                    t.contains(&format!("{{{}}}", table.name))
                        || t.contains(&format!("{{values.{}}}", table.name))
                });
                if referenced {
                    active_value_tables.push(table);
                }
            }
            let uses_value_wildcard = !active_value_tables.is_empty();

            // output_pattern consumer detection (issue #227 item 5): a rule
            // referencing a fresh wildcard — in inputs, outputs, shell,
            // `when`, or `expand_inputs` patterns (its OWN output_pattern
            // excluded: a rule's own fresh wildcard is what it produces) —
            // cannot be instantiated until the producer's domain has been
            // discovered at runtime. Defer the template whole: its own
            // pair/group/value fan-out, if any, is baked from the producer
            // instance bindings when the domain is projected. A consumer of
            // TWO producers' fresh wildcards is a v1 error; a rule that is
            // both consumer and producer (chain) is legal.
            if !fresh_wildcards.is_empty() {
                let mut referenced: Vec<String> = Vec::new();
                for text in &expand_texts {
                    for w in fresh_wildcards.keys() {
                        if text.contains(&format!("{{{w}}}")) && !referenced.contains(w) {
                            referenced.push(w.clone());
                        }
                    }
                }
                if !referenced.is_empty() {
                    let mut producers: Vec<&str> = Vec::new();
                    for w in &referenced {
                        let p = fresh_wildcards[w].as_str();
                        if !producers.contains(&p) {
                            producers.push(p);
                        }
                    }
                    if producers.len() > 1 {
                        return Err(OxoFlowError::Validation {
                            message: format!(
                                "rule '{}' references the fresh wildcards {{{}}} of \
                                 output_pattern producers {}; one producer per consumer (v1)",
                                rule.name,
                                referenced.join("}, {"),
                                producers
                                    .iter()
                                    .map(|p| format!("'{}'", p))
                                    .collect::<Vec<_>>()
                                    .join(" and ")
                            ),
                            rule: Some(rule.name.clone()),
                            suggestion: Some(
                                "split the consumer into one rule per producer".to_string(),
                            ),
                        });
                    }
                    // Declaration-order constraint: the consumer must be
                    // declared AFTER its producer so the runtime fan-out
                    // attribution is stable and diagnosable. Warn — the
                    // pass still resolves (all producers are registered
                    // upfront).
                    let producer_name = producers[0];
                    let producer_idx = self.rules.iter().position(|r| r.name == producer_name);
                    let consumer_idx = self.rules.iter().position(|r| r.name == rule.name);
                    if let (Some(p), Some(c)) = (producer_idx, consumer_idx)
                        && p > c
                    {
                        tracing::warn!(
                            rule = %rule.name,
                            producer = producer_name,
                            "output_pattern consumer declared BEFORE its producer; \
                             the fan-out still resolves, but declare the producer first \
                             for stable run attribution"
                        );
                    }
                    self.pending_output_pattern.push(rule.clone());
                    continue;
                }
            }

            // Unbound `{values.name}` namespace references have no fan-out
            // source: warn (never error — same stance as unbound `{sample}`)
            // so the author notices before runtime. Bare `{name}` is
            // indistinguishable from engine placeholders (`{input}`,
            // `{threads}`) and stays silent.
            let mut warned_value_ns: Vec<String> = Vec::new();
            for text in &expand_texts {
                for cap in VALUES_NS_RE.captures_iter(text) {
                    let name = cap[1].to_string();
                    let has_table = self.values.iter().any(|v| v.name == name);
                    if !has_table && !warned_value_ns.contains(&name) {
                        tracing::warn!(
                            rule = %rule.name,
                            "rule references '{{values.{name}}}' but no [[values]] table named '{name}' exists; the placeholder will be left unexpanded"
                        );
                        warned_value_ns.push(name);
                    }
                }
            }

            // Cartesian product of the active tables, deterministic: the
            // last referenced table varies fastest, mirroring declaration
            // order. Rules using no value table get a single empty combo —
            // the identity element, so the pair/group branches below run
            // exactly as before.
            let mut value_combos: Vec<crate::wildcard::WildcardValues> =
                vec![crate::wildcard::WildcardValues::new()];
            for table in &active_value_tables {
                let mut next = Vec::with_capacity(value_combos.len() * table.values.len());
                for combo in &value_combos {
                    for value in &table.values {
                        let mut c = combo.clone();
                        c.insert(table.name.clone(), value.clone());
                        next.push(c);
                    }
                }
                value_combos = next;
            }

            if uses_pair_wildcard {
                let orig_name = rule.name.clone();
                let mut expanded_names = Vec::new();
                // Expand for each value-combo × pair combination (orthogonal
                // fan-out — [[values]] adds a dimension on top of pairs).
                for value_combo in &value_combos {
                    for pair_combo in &pair_combos {
                        // Merge the binding sources so `{assembler}` and
                        // `{experiment}` both resolve during expansion.
                        let mut combo = pair_combo.clone();
                        combo.extend(value_combo.clone());

                        // Filter by constraints (non-matching combos are
                        // skipped, per docs).
                        if validate_wildcard_constraints_compiled(&combo, &compiled_constraints)
                            .is_err()
                        {
                            continue;
                        }

                        // Per-instance `when` filtering (snakemake-style DAG
                        // morphing): conditions referencing `wildcard.<key>`
                        // resolve against this combo and non-matching
                        // instances never enter the DAG. Conditions without
                        // wildcard references keep the execution-time flow.
                        if let Some(ref when) = rule.when
                            && when.contains("wildcard.")
                        {
                            let config_values: HashMap<String, toml::Value> = self
                                .config
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                            let combo_values = Self::expansion_when_context(&combo);
                            if !crate::executor::process::evaluate_condition_with_wildcards(
                                when,
                                &config_values,
                                &combo_values,
                            ) {
                                continue;
                            }
                        }

                        let suffix = pair_combo.get("pair_id").cloned().unwrap_or_else(|| {
                            pair_combo.values().cloned().collect::<Vec<_>>().join("_")
                        });
                        let new_name = format!(
                            "{}{}_{}",
                            rule.name,
                            value_instance_suffix(value_combo, &active_value_tables),
                            suffix
                        );
                        expanded_names.push(new_name.clone());

                        if !seen_names.insert(new_name.clone()) {
                            return Err(OxoFlowError::DuplicateRule { name: new_name });
                        }

                        let mut expanded = rule.clone();
                        expanded.name = new_name;

                        // Bake the per-instance wildcard bindings into the
                        // `when` (the instance survived filtering above), so
                        // the execution-time re-check re-evaluates this same
                        // verdict with no wildcard context.
                        if let Some(ref when) = rule.when
                            && when.contains("wildcard.")
                        {
                            expanded.when = Some(Self::bake_wildcard_when(when, &combo));
                        }

                        // Expand input/output/shell/log patterns
                        expanded.input = expand_rule_patterns(&rule.input, &combo);
                        expanded.output = expand_rule_patterns(&rule.output, &combo);
                        if let Some(ref shell) = rule.shell {
                            expanded.shell = Some(expand_rule_shell(shell, &combo));
                        }
                        if let Some(ref log) = rule.log {
                            expanded.log = Some(expand_rule_shell(log, &combo));
                        }
                        // output_pattern (issue #227 item 5): bake the
                        // bound wildcards so each instance scans only its
                        // own files; the fresh wildcard stays unbound.
                        if let Some(ref op) = rule.output_pattern {
                            expanded.output_pattern = Some(expand_rule_shell(op, &combo));
                        }
                        // Script and hooks carry the per-instance
                        // substitution too (issue #98) — same class as
                        // shell/log.
                        expand_command_text_fields(&mut expanded, rule, |s| {
                            expand_rule_shell(s, &combo)
                        });
                        // Per-instance `{meta.<column>}` substitution from
                        // the sample-like bindings (issue #227 item 2).
                        self.apply_instance_meta(&mut expanded, &combo);

                        // Record which sample names this expansion belongs to
                        // (issue #63 readiness attribution).
                        let mut involved: Vec<String> = Vec::new();
                        for key in ["experiment", "control"] {
                            if let Some(value) = combo.get(key)
                                && !value.is_empty()
                                && !involved.contains(value)
                            {
                                involved.push(value.clone());
                            }
                        }
                        self.expansion_samples
                            .insert(expanded.name.clone(), involved);
                        // Per-instance pair/group bindings for expand_inputs
                        // pattern resolution (see the field docs).
                        self.expansion_pairs
                            .insert(expanded.name.clone(), combo.clone());

                        if !value_combo.is_empty() {
                            self.expansion_values
                                .insert(expanded.name.clone(), value_combo.clone());
                        }
                        self.expansion_templates
                            .insert(expanded.name.clone(), orig_name.clone());

                        expanded_rules.push(expanded);
                    }
                }
                name_map.insert(orig_name, expanded_names);
            } else if uses_group_wildcard {
                let orig_name = rule.name.clone();
                let mut expanded_names = Vec::new();
                // Expand for each value-combo × (group, sample) combination
                // (orthogonal fan-out — [[values]] adds a dimension on top
                // of sample groups).
                for value_combo in &value_combos {
                    for combo in &group_combos {
                        let mut merged = combo.clone();
                        merged.extend(value_combo.clone());

                        // Filter by constraints (non-matching combos are
                        // skipped, per docs).
                        if validate_wildcard_constraints_compiled(&merged, &compiled_constraints)
                            .is_err()
                        {
                            continue;
                        }

                        // Per-instance `when` filtering (snakemake-style DAG
                        // morphing) — see the pair branch above.
                        if let Some(ref when) = rule.when
                            && when.contains("wildcard.")
                        {
                            let config_values: HashMap<String, toml::Value> = self
                                .config
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                            let combo_values = Self::expansion_when_context(&merged);
                            if !crate::executor::process::evaluate_condition_with_wildcards(
                                when,
                                &config_values,
                                &combo_values,
                            ) {
                                continue;
                            }
                        }

                        let group = combo.get("group").map(String::as_str).unwrap_or("group");
                        let sample = combo.get("sample").map(String::as_str).unwrap_or("sample");
                        let new_name = format!(
                            "{}{}_{}_{}",
                            rule.name,
                            value_instance_suffix(value_combo, &active_value_tables),
                            group,
                            sample
                        );
                        expanded_names.push(new_name.clone());

                        if !seen_names.insert(new_name.clone()) {
                            return Err(OxoFlowError::DuplicateRule { name: new_name });
                        }

                        let mut expanded = rule.clone();
                        expanded.name = new_name;

                        // Bake the per-instance wildcard bindings into the
                        // `when` (the instance survived filtering above) —
                        // see the pair branch for the rationale.
                        if let Some(ref when) = rule.when
                            && when.contains("wildcard.")
                        {
                            expanded.when = Some(Self::bake_wildcard_when(when, &merged));
                        }

                        expanded.input = rule
                            .input
                            .iter()
                            .map(|p| {
                                if has_wildcards(p) || crate::wildcard::contains_values_namespace(p)
                                {
                                    crate::wildcard::expand_values_namespace(
                                        &expand_pattern(p, &merged).unwrap_or_else(|_| p.clone()),
                                        &merged,
                                    )
                                } else {
                                    p.clone()
                                }
                            })
                            .collect();
                        expanded.output = rule
                            .output
                            .iter()
                            .map(|p| {
                                if has_wildcards(p) || crate::wildcard::contains_values_namespace(p)
                                {
                                    crate::wildcard::expand_values_namespace(
                                        &expand_pattern(p, &merged).unwrap_or_else(|_| p.clone()),
                                        &merged,
                                    )
                                } else {
                                    p.clone()
                                }
                            })
                            .collect();
                        if let Some(ref shell) = rule.shell {
                            expanded.shell = if has_wildcards(shell)
                                || crate::wildcard::contains_values_namespace(shell)
                            {
                                Some(crate::wildcard::expand_values_namespace(
                                    &expand_pattern(shell, &merged)
                                        .unwrap_or_else(|_| shell.clone()),
                                    &merged,
                                ))
                            } else {
                                Some(shell.clone())
                            };
                        }
                        if let Some(ref log) = rule.log {
                            expanded.log = if has_wildcards(log)
                                || crate::wildcard::contains_values_namespace(log)
                            {
                                Some(crate::wildcard::expand_values_namespace(
                                    &expand_pattern(log, &merged).unwrap_or_else(|_| log.clone()),
                                    &merged,
                                ))
                            } else {
                                Some(log.clone())
                            };
                        }
                        // output_pattern (issue #227 item 5): bake the bound
                        // wildcards per instance (see the pair branch).
                        if let Some(ref op) = rule.output_pattern {
                            expanded.output_pattern = Some(expand_rule_shell(op, &merged));
                        }
                        // Free-text command fields take the same per-instance
                        // substitution as shell/log (issue #98): script and
                        // the hooks render through the same placeholder pass
                        // at execution time, which never sees pair/group
                        // names, so the values must be baked in here.
                        expand_command_text_fields(&mut expanded, rule, |s| {
                            expand_rule_shell(s, &merged)
                        });
                        // Per-instance `{meta.<column>}` substitution from
                        // the instance's `{sample}` binding (issue #227
                        // item 2).
                        self.apply_instance_meta(&mut expanded, &merged);

                        // Record which sample this expansion belongs to
                        // (issue #63 readiness attribution).
                        let involved: Vec<String> = combo
                            .get("sample")
                            .cloned()
                            .into_iter()
                            .filter(|name| !name.is_empty())
                            .collect();
                        self.expansion_samples
                            .insert(expanded.name.clone(), involved);
                        // Per-instance pair/group bindings for expand_inputs
                        // pattern resolution (see the field docs).
                        self.expansion_pairs
                            .insert(expanded.name.clone(), combo.clone());

                        if !value_combo.is_empty() {
                            self.expansion_values
                                .insert(expanded.name.clone(), value_combo.clone());
                        }
                        self.expansion_templates
                            .insert(expanded.name.clone(), orig_name.clone());

                        expanded_rules.push(expanded);
                    }
                }
                name_map.insert(orig_name, expanded_names);
            } else if uses_value_wildcard {
                // [[values]] fan-out without pair/group wildcards.
                let orig_name = rule.name.clone();
                let mut expanded_names = Vec::new();
                for combo in &value_combos {
                    // Filter by constraints (non-matching combos are skipped,
                    // per docs).
                    if validate_wildcard_constraints_compiled(combo, &compiled_constraints).is_err()
                    {
                        continue;
                    }

                    // Per-instance `when` filtering (snakemake-style DAG
                    // morphing) — see the pair branch above.
                    if let Some(ref when) = rule.when
                        && when.contains("wildcard.")
                    {
                        let config_values: HashMap<String, toml::Value> = self
                            .config
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        let combo_values = Self::expansion_when_context(combo);
                        if !crate::executor::process::evaluate_condition_with_wildcards(
                            when,
                            &config_values,
                            &combo_values,
                        ) {
                            continue;
                        }
                    }

                    let new_name = format!(
                        "{}{}",
                        rule.name,
                        value_instance_suffix(combo, &active_value_tables)
                    );
                    expanded_names.push(new_name.clone());

                    if !seen_names.insert(new_name.clone()) {
                        return Err(OxoFlowError::DuplicateRule { name: new_name });
                    }

                    let mut expanded = rule.clone();
                    expanded.name = new_name.clone();

                    // Bake the per-instance wildcard bindings into the
                    // `when` (the instance survived filtering above) — see
                    // the pair branch for the rationale.
                    if let Some(ref when) = rule.when
                        && when.contains("wildcard.")
                    {
                        expanded.when = Some(Self::bake_wildcard_when(when, combo));
                    }

                    // Structure-preserving expansion (List / Map / Dir).
                    expanded.input = expand_rule_patterns(&rule.input, combo);
                    expanded.output = expand_rule_patterns(&rule.output, combo);
                    if let Some(ref shell) = rule.shell {
                        expanded.shell = Some(expand_rule_shell(shell, combo));
                    }
                    if let Some(ref log) = rule.log {
                        expanded.log = Some(expand_rule_shell(log, combo));
                    }
                    // output_pattern (issue #227 item 5): bake the bound
                    // wildcards per instance (see the pair branch).
                    if let Some(ref op) = rule.output_pattern {
                        expanded.output_pattern = Some(expand_rule_shell(op, combo));
                    }
                    // Script and hooks carry the per-instance substitution
                    // too (issue #98) — same class as shell/log.
                    expand_command_text_fields(&mut expanded, rule, |s| {
                        expand_rule_shell(s, combo)
                    });
                    // Per-instance `{meta.<column>}` substitution (issue
                    // #227 item 2) — see the pair branch.
                    self.apply_instance_meta(&mut expanded, combo);

                    self.expansion_values
                        .insert(new_name.clone(), combo.clone());
                    self.expansion_templates
                        .insert(new_name.clone(), orig_name.clone());
                    expanded_rules.push(expanded);
                }
                name_map.insert(orig_name, expanded_names);
            } else {
                // No expansion needed — keep rule as-is
                if !seen_names.insert(rule.name.clone()) {
                    return Err(OxoFlowError::DuplicateRule {
                        name: rule.name.clone(),
                    });
                }
                expanded_rules.push(rule.clone());
            }
        }

        // Resolve depends_on references: replace template names with expanded names
        if !name_map.is_empty() {
            for rule in &mut expanded_rules {
                if rule.depends_on.is_empty() {
                    continue;
                }
                let mut resolved_deps = Vec::new();
                for dep in &rule.depends_on {
                    if let Some(expanded_names) = name_map.get(dep.as_str()) {
                        resolved_deps.extend(expanded_names.clone());
                    } else {
                        resolved_deps.push(dep.clone());
                    }
                }
                rule.depends_on = resolved_deps;
            }
        }

        let mut final_rules = Vec::new();
        let mut gather_injections: HashMap<String, Vec<String>> = HashMap::new();

        for rule in expanded_rules {
            if let Some(ref scatter) = rule.scatter {
                let mut values = scatter.values.clone();
                if values.is_empty()
                    && let Some(ref v_from) = scatter.values_from
                    && let Some(resolved) = self.resolve_config_list(v_from)
                {
                    values = resolved;
                }

                let mut scatter_outputs = Vec::new();

                for val in &values {
                    let mut combo = HashMap::new();
                    combo.insert(scatter.variable.clone(), val.clone());

                    let mut scattered_rule = rule.clone();
                    scattered_rule.name = format!("{}_{}", rule.name, val);
                    scattered_rule.scatter = None; // remove scatter from generated rule

                    scattered_rule.input = match scattered_rule.input {
                        FilePatterns::List(ref v) => FilePatterns::List(
                            v.iter()
                                .map(|p| {
                                    expand_pattern(p, &combo).unwrap_or_else(|_| p.to_string())
                                })
                                .collect(),
                        ),
                        FilePatterns::Map(ref m) => FilePatterns::Map(
                            m.iter()
                                .map(|(k, v)| {
                                    (
                                        k.clone(),
                                        expand_pattern(v, &combo).unwrap_or_else(|_| v.to_string()),
                                    )
                                })
                                .collect(),
                        ),
                        FilePatterns::Dir {
                            ref path,
                            ref pattern,
                        } => FilePatterns::Dir {
                            path: expand_pattern(path, &combo).unwrap_or_else(|_| path.clone()),
                            pattern: pattern.clone(),
                        },
                    };
                    scattered_rule.output = match scattered_rule.output {
                        FilePatterns::List(ref v) => FilePatterns::List(
                            v.iter()
                                .map(|p| {
                                    expand_pattern(p, &combo).unwrap_or_else(|_| p.to_string())
                                })
                                .collect(),
                        ),
                        FilePatterns::Map(ref m) => FilePatterns::Map(
                            m.iter()
                                .map(|(k, v)| {
                                    (
                                        k.clone(),
                                        expand_pattern(v, &combo).unwrap_or_else(|_| v.to_string()),
                                    )
                                })
                                .collect(),
                        ),
                        FilePatterns::Dir {
                            ref path,
                            ref pattern,
                        } => FilePatterns::Dir {
                            path: expand_pattern(path, &combo).unwrap_or_else(|_| path.clone()),
                            pattern: pattern.clone(),
                        },
                    };
                    if let Some(ref shell) = scattered_rule.shell {
                        scattered_rule.shell =
                            Some(expand_pattern(shell, &combo).unwrap_or_else(|_| shell.clone()));
                    }
                    if let Some(ref log) = scattered_rule.log {
                        scattered_rule.log =
                            Some(expand_pattern(log, &combo).unwrap_or_else(|_| log.clone()));
                    }
                    // output_pattern (issue #227 item 5): the scatter
                    // variable joins the bound vocabulary baked into each
                    // scattered instance's pattern.
                    if let Some(ref op) = scattered_rule.output_pattern {
                        scattered_rule.output_pattern =
                            Some(expand_pattern(op, &combo).unwrap_or_else(|_| op.clone()));
                    }
                    // Script and hooks take the same substitution (issue #98):
                    // a script whose path or content depends on the scatter
                    // variable must resolve per instance (live: the pca rule
                    // had to be split into three explicit rules).
                    expand_command_text_fields(&mut scattered_rule, &rule, |text| {
                        expand_pattern(text, &combo).unwrap_or_else(|_| text.to_string())
                    });

                    // Carry the pre-scatter [[values]] bindings over to the
                    // scattered name and add the scatter variable, so
                    // expand_inputs patterns referencing {assembler} (etc.)
                    // still resolve per instance after the rename.
                    let mut scatter_bindings = self
                        .expansion_values
                        .get(&rule.name)
                        .cloned()
                        .unwrap_or_default();
                    scatter_bindings.insert(scatter.variable.clone(), val.clone());
                    self.expansion_values
                        .insert(scattered_rule.name.clone(), scatter_bindings);
                    self.expansion_templates
                        .insert(scattered_rule.name.clone(), rule.name.clone());

                    scatter_outputs.extend(scattered_rule.output.to_vec());
                    final_rules.push(scattered_rule);
                }

                if let Some(ref gather_rule) = scatter.gather {
                    gather_injections
                        .entry(gather_rule.clone())
                        .or_default()
                        .extend(scatter_outputs);
                }
            } else if let Some(ref transform) = rule.transform {
                // Handle transform operator: split -> map -> combine
                let split_values = self.resolve_split_values(&transform.split)?;

                // Validate that split values are not empty
                if split_values.is_empty() {
                    return Err(OxoFlowError::Validation {
                        message: format!("transform rule '{}' has no split values", rule.name),
                        rule: Some(rule.name.clone()),
                        suggestion: Some(
                            "provide values, values_from, n, or glob in split config".to_string(),
                        ),
                    });
                }

                let split_var = &transform.split.by;
                let mut all_chunk_outputs: Vec<String> = Vec::new();

                // Generate map rules for each split value
                for value in &split_values {
                    // Determine chunk output path
                    let chunk_output = if rule.output.is_empty() {
                        format!(".oxo-flow/chunks/{split_var}/{value}.out")
                    } else if rule
                        .output
                        .get_index(0)
                        .map(|o| o.contains(&format!("{{{split_var}}}")))
                        .unwrap_or(false)
                    {
                        // Replace only {split_var} in output
                        rule.output
                            .get_index(0)
                            .unwrap()
                            .replace(&format!("{{{split_var}}}"), value)
                    } else {
                        let base = rule
                            .output
                            .get_index(0)
                            .expect("output non-empty: guarded by is_empty() check");
                        // Keep the full multi-part extension (e.g. "vcf.gz", not
                        // just "gz") so tools can infer the format from the name.
                        let file_part = base.rsplit('/').next().unwrap_or(base);
                        let ext = file_part.split_once('.').map(|(_, e)| e).unwrap_or("out");
                        format!(".oxo-flow/chunks/{split_var}/{value}.{ext}")
                    };

                    all_chunk_outputs.push(chunk_output.clone());

                    let map_rule_name = format!("{}_{}", rule.name, value);
                    // Replace only {split_var} in map shell, keep other placeholders for execution
                    let map_shell = transform.map.replace(&format!("{{{split_var}}}"), value);

                    let mut map_rule = Rule {
                        name: map_rule_name,
                        input: rule.input.clone(),
                        output: vec![chunk_output].into(),
                        shell: Some(map_shell),
                        // Inherit the parent's required semantics (issue
                        // #142 H5): Rule's plain Default makes bools false
                        // while serde defaults `required` to true — an
                        // engine-generated chunk must not silently become
                        // best-effort.
                        required: rule.required,

                        threads: rule.threads,
                        memory: rule.memory.clone(),
                        resources: rule.resources.clone(),
                        environment: rule.environment.clone(),
                        retries: rule.retries,
                        ..Default::default()
                    };

                    #[allow(deprecated)]
                    {
                        map_rule.threads = rule.threads;
                        map_rule.memory = rule.memory.clone();
                    }

                    final_rules.push(map_rule);
                }

                // Generate combine rule if specified
                if let Some(ref combine) = transform.combine {
                    let combine_rule_name = format!("{}_combine", rule.name);
                    let combine_shell = if let Some(ref shell) = combine.shell {
                        let chunks_str = all_chunk_outputs.join(" ");
                        shell
                            .replace("{chunks}", &chunks_str)
                            .replace("{input}", &chunks_str)
                            .replace("{output}", &rule.output.join(" "))
                    } else if combine.aggregate {
                        let method = combine.method.as_deref().unwrap_or("concat");
                        let chunks_str = all_chunk_outputs.join(" ");
                        let output_str = rule.output.join(" ");

                        match method {
                            "concat" => {
                                let header = combine
                                    .header
                                    .as_deref()
                                    .map(|h| format!("echo '{}' && ", h))
                                    .unwrap_or_default();
                                format!("{}cat {} > {}", header, chunks_str, output_str)
                            }
                            "json_merge" => {
                                format!("jq -s 'add' {} > {}", chunks_str, output_str)
                            }
                            _ => {
                                return Err(OxoFlowError::Validation {
                                    message: format!("unknown aggregation method: {}", method),
                                    rule: Some(rule.name.clone()),
                                    suggestion: Some("use 'concat' or 'json_merge'".to_string()),
                                });
                            }
                        }
                    } else {
                        return Err(OxoFlowError::Validation {
                            message: format!(
                                "transform rule '{}' has combine but no shell or aggregate method",
                                rule.name
                            ),
                            rule: Some(rule.name.clone()),
                            suggestion: Some(
                                "specify combine.shell or combine.aggregate".to_string(),
                            ),
                        });
                    };

                    let mut combine_rule = Rule {
                        name: combine_rule_name,
                        input: FilePatterns::List(all_chunk_outputs.clone()),
                        output: rule.output.clone(),
                        shell: Some(combine_shell),
                        required: rule.required,
                        cleanup_chunks: transform.cleanup,
                        threads: rule.threads,
                        memory: rule.memory.clone(),
                        resources: rule.resources.clone(),
                        environment: rule.environment.clone(),
                        ..Default::default()
                    };

                    #[allow(deprecated)]
                    {
                        combine_rule.threads = rule.threads;
                        combine_rule.memory = rule.memory.clone();
                    }

                    final_rules.push(combine_rule);
                }
            } else {
                final_rules.push(rule);
            }
        }

        // ── input_groups fan-out (issue #227 item 3, the groupTuple
        // pattern) ────────────────────────────────────────────────────────
        // Runs AFTER scatter/transform expansion so group enumeration sees
        // every materialized producer's literal outputs — the
        // plan-time-known paths that make exact-match DAG edges work even
        // for files that only come into existence mid-run. The producer
        // pool grows as instances materialize, so input_groups chains
        // (lanemerge → library_merge → seqtype_merge) resolve in
        // declaration order. Producers declared AFTER their input_groups
        // consumer are not yet in the pool when the consumer processes
        // (v1: declare in topological order; a zero-match consumer then
        // warns and instantiates nothing, per the skip semantics).
        // Generated instances append after the regular rules — DAG edges,
        // not list order, drive scheduling.
        if !pending_input_groups.is_empty() {
            // Literal (wildcard-free, `{config.x}`-expanded) outputs of
            // every rule materialized so far — the enumeration source for
            // files the workflow itself will produce.
            let mut producer_outputs: Vec<String> = Vec::new();
            for rule in &final_rules {
                producer_outputs.extend(
                    rule.output
                        .to_vec()
                        .into_iter()
                        .map(|o| expand_config_vars_in_path(&o, &self.config)),
                );
            }
            for rule in pending_input_groups {
                let instances = self.expand_input_groups_rule(&rule, &producer_outputs)?;
                let mut expanded_names = Vec::new();
                for instance in instances {
                    if !seen_names.insert(instance.name.clone()) {
                        return Err(OxoFlowError::DuplicateRule {
                            name: instance.name.clone(),
                        });
                    }
                    expanded_names.push(instance.name.clone());
                    producer_outputs.extend(
                        instance
                            .output
                            .to_vec()
                            .into_iter()
                            .map(|o| expand_config_vars_in_path(&o, &self.config)),
                    );
                    final_rules.push(instance);
                }
                name_map.insert(rule.name.clone(), expanded_names);
            }

            // The input_groups instances joined the rule set after the
            // depends_on pass above ran on expanded_rules — re-resolve on
            // the final set so `depends_on = ["lanemerge"]` reaches every
            // instance (idempotent: already-expanded names are not keys).
            if !name_map.is_empty() {
                for rule in &mut final_rules {
                    if rule.depends_on.is_empty() {
                        continue;
                    }
                    let mut resolved_deps = Vec::new();
                    for dep in &rule.depends_on {
                        if let Some(expanded_names) = name_map.get(dep.as_str()) {
                            resolved_deps.extend(expanded_names.clone());
                        } else {
                            resolved_deps.push(dep.clone());
                        }
                    }
                    rule.depends_on = resolved_deps;
                }
            }
        }

        // Apply gather injections and expand_inputs
        for rule in &mut final_rules {
            if let Some(injected) = gather_injections.get(&rule.name) {
                let mut current_input = rule.input.to_vec();
                current_input.extend(injected.clone());
                rule.input = FilePatterns::List(current_input);
            }

            // process expand_inputs
            for exp in &rule.expand_inputs {
                let mut variables = HashMap::new();
                for (var_name, var_ref) in &exp.variables {
                    if let Some(vals) = self.resolve_config_list(var_ref) {
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

                // Bind this instance's own [[values]] values so `{assembler}`
                // inside the expand pattern resolves per instance — the
                // spades instance never sees the megahit value.
                let bindings = self.expansion_values.get(&rule.name).cloned();
                if let Some(bindings) = &bindings {
                    for (name, value) in bindings {
                        variables
                            .entry(name.clone())
                            .or_insert_with(|| vec![value.clone()]);
                    }
                }

                // Same per-instance binding for pair wildcards: `{pair_id}`
                // (and the other pair keys) inside an expand pattern resolve
                // to THIS instance's pair, so a per-pair gather rule picks up
                // its own pair's files only (snakemake-style; the
                // cohort-level `pair_id = "config.pairs_list"` variable form
                // still wins when declared explicitly).
                if let Some(pair_bindings) = self.expansion_pairs.get(&rule.name) {
                    for (name, value) in pair_bindings {
                        variables
                            .entry(name.clone())
                            .or_insert_with(|| vec![value.clone()]);
                    }
                }

                let mut expanded = crate::wildcard::cartesian_expand(&exp.pattern, &variables);
                // Resolve the `{values.name}` namespace form per instance.
                if let Some(bindings) = &bindings {
                    for path in &mut expanded {
                        *path = crate::wildcard::expand_values_namespace(path, bindings);
                    }
                }
                let mut current_input = rule.input.to_vec();
                current_input.extend(expanded);
                rule.input = FilePatterns::List(current_input);
            }
        }

        self.rules = final_rules;
        Ok(())
    }

    /// Apply the per-instance `{meta.<column>}` substitution (issue #227
    /// item 2) to every text field of an expanded rule — inputs, outputs,
    /// shell, log, `when`, script, and the hooks — the same field set that
    /// already carries per-instance substitutions.
    ///
    /// The instance's sample-like binding (from `combo`) selects the
    /// metadata row; a missing row OR column renders empty, so
    /// `when = "config.single_end_mode || {meta.endedness} == 'SE'"`-style
    /// predicates evaluate false for samples without the data (the gate is
    /// closed, never a literal token). Rules without any `{meta.`
    /// reference are untouched.
    fn apply_instance_meta(&self, expanded: &mut Rule, combo: &crate::wildcard::WildcardValues) {
        let refers = expanded.input.iter().any(|p| p.contains("{meta."))
            || expanded.output.iter().any(|p| p.contains("{meta."))
            || expanded
                .shell
                .as_deref()
                .is_some_and(|s| s.contains("{meta."))
            || expanded
                .log
                .as_deref()
                .is_some_and(|s| s.contains("{meta."))
            || expanded
                .when
                .as_deref()
                .is_some_and(|s| s.contains("{meta."))
            || expanded
                .script
                .as_deref()
                .is_some_and(|s| s.contains("{meta."))
            || expanded
                .pre_exec
                .as_deref()
                .is_some_and(|s| s.contains("{meta."))
            || expanded
                .on_success
                .as_deref()
                .is_some_and(|s| s.contains("{meta."))
            || expanded
                .on_failure
                .as_deref()
                .is_some_and(|s| s.contains("{meta."));
        if !refers {
            return;
        }
        let expand =
            |text: &str| crate::wildcard::expand_meta_namespace(text, &self.metadata, combo);
        let meta_expand_patterns = |patterns: &FilePatterns| -> FilePatterns {
            match patterns {
                FilePatterns::List(v) => FilePatterns::List(v.iter().map(|p| expand(p)).collect()),
                FilePatterns::Map(m) => {
                    FilePatterns::Map(m.iter().map(|(k, v)| (k.clone(), expand(v))).collect())
                }
                FilePatterns::Dir { path, pattern } => FilePatterns::Dir {
                    path: expand(path),
                    pattern: pattern.clone(),
                },
            }
        };
        expanded.input = meta_expand_patterns(&expanded.input);
        expanded.output = meta_expand_patterns(&expanded.output);
        expanded.shell = expanded.shell.as_deref().map(expand);
        expanded.log = expanded.log.as_deref().map(expand);
        // `when` gets the bake treatment (quoted literals in comparisons,
        // true/false in truthiness positions) so the execution-time re-check
        // re-evaluates this same verdict — plain substitution would leave
        // unquoted values that the evaluator's default-true fallback runs.
        expanded.when = expanded
            .when
            .as_deref()
            .map(|w| Self::bake_meta_when(w, &self.metadata, combo));
        expanded.script = expanded.script.as_deref().map(expand);
        expanded.pre_exec = expanded.pre_exec.as_deref().map(expand);
        expanded.on_success = expanded.on_success.as_deref().map(expand);
        expanded.on_failure = expanded.on_failure.as_deref().map(expand);
    }

    /// Expand one `input_groups` rule into its per-group instances (issue
    /// #227 item 3).
    ///
    /// Group candidates come from TWO plan-time-known sources, deduplicated
    /// by path:
    /// 1. files already on disk under the workflow root (the same
    ///    filesystem walk as `sample_pattern` discovery), and
    /// 2. literal outputs already materialized by producers (the files the
    ///    workflow itself will produce — matching a producer's expanded
    ///    output against the pattern extracts the same wildcard values, so
    ///    exact-match DAG edges to those producers work).
    ///
    /// Files are grouped by the `group_by` wildcard: one instance per key
    /// with `{input}` = the group's files (sorted), the instance wildcard
    /// map = group key + first occurrence of every `keep`-listed wildcard,
    /// and `{input_group.<wildcard>}` = space-joined per-group value lists.
    fn expand_input_groups_rule(
        &mut self,
        rule: &Rule,
        producer_outputs: &[String],
    ) -> Result<Vec<Rule>> {
        if rule.input_groups.len() > 1 {
            return Err(OxoFlowError::Validation {
                message: format!(
                    "rule '{}' declares {} input_groups entries — v1 supports at most one",
                    rule.name,
                    rule.input_groups.len()
                ),
                rule: Some(rule.name.clone()),
                suggestion: Some("split the rule into one rule per pattern".to_string()),
            });
        }
        let decl = &rule.input_groups[0];
        let pattern_wildcards = crate::wildcard::extract_wildcards(&decl.pattern);
        if pattern_wildcards.is_empty() {
            return Err(OxoFlowError::Validation {
                message: format!(
                    "input_groups pattern '{}' of rule '{}' has no {{wildcard}} placeholders",
                    decl.pattern, rule.name
                ),
                rule: Some(rule.name.clone()),
                suggestion: Some(
                    "add a {wildcard} (e.g. {sample}) to the pattern so files can be grouped"
                        .to_string(),
                ),
            });
        }
        // `group_by = "meta.<column>"` (issue #227 item 4): the group key
        // comes from the per-sample metadata table instead of a pattern
        // wildcard. The instance map binds the COLUMN name (e.g.
        // `{antibody}`); pattern wildcards are never bound — the group's
        // value lists are reachable as `{input_group.<wildcard>}` — so
        // `keep` is meaningless for metadata grouping and rejected.
        let meta_group_by: Option<&str> = decl
            .group_by
            .strip_prefix("meta.")
            .filter(|column| !column.is_empty());
        if let Some(column) = meta_group_by {
            let known = crate::wildcard::metadata_columns(&self.metadata);
            if !known.contains(column) {
                return Err(OxoFlowError::Validation {
                    message: format!(
                        "input_groups group_by '{}' of rule '{}' references metadata column '{}' that no metadata row defines",
                        decl.group_by, rule.name, column
                    ),
                    rule: Some(rule.name.clone()),
                    suggestion: Some(
                        "check the metadata_file columns, or use a pattern wildcard as group_by"
                            .to_string(),
                    ),
                });
            }
            if decl.keep.is_some() {
                return Err(OxoFlowError::Validation {
                    message: format!(
                        "input_groups keep of rule '{}' is not supported with group_by = '{}': \
                         metadata-grouped instances bind only the group key ('{{{column}}}')",
                        rule.name, decl.group_by
                    ),
                    rule: Some(rule.name.clone()),
                    suggestion: Some(
                        "use {input_group.<wildcard>} to reference the group's value lists"
                            .to_string(),
                    ),
                });
            }
        } else if !pattern_wildcards.contains(&decl.group_by) {
            return Err(OxoFlowError::Validation {
                message: format!(
                    "input_groups group_by '{}' of rule '{}' is not a wildcard in pattern '{}'",
                    decl.group_by, rule.name, decl.pattern
                ),
                rule: Some(rule.name.clone()),
                suggestion: Some(format!(
                    "use one of the pattern wildcards: {}",
                    pattern_wildcards.join(", ")
                )),
            });
        }
        let others: Vec<String> = pattern_wildcards
            .iter()
            .filter(|w| w.as_str() != decl.group_by)
            .cloned()
            .collect();
        let keep: Vec<String> = if meta_group_by.is_some() {
            // Metadata grouping binds only the column name; no pattern
            // wildcard is bound (validation above rejects `keep`).
            Vec::new()
        } else {
            match &decl.keep {
                Some(names) => {
                    for name in names {
                        if !others.contains(name) {
                            return Err(OxoFlowError::Validation {
                                message: format!(
                                    "input_groups keep of rule '{}' names '{}' which is not a \
                                     pattern wildcard (other than group_by '{}')",
                                    rule.name, name, decl.group_by
                                ),
                                rule: Some(rule.name.clone()),
                                suggestion: Some(format!(
                                    "keep may only list: {}",
                                    others.join(", ")
                                )),
                            });
                        }
                    }
                    names.clone()
                }
                None => others.clone(),
            }
        };

        // `{config.x}` placeholders resolve against the workflow config
        // before matching, exactly like every other path in the engine.
        let base = self
            .base_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let pattern = expand_config_vars_in_path(&decl.pattern, &self.config);
        let pattern_re = crate::wildcard::pattern_to_regex(&pattern)?;

        // Source 1: files already on disk under the workflow root.
        let mut candidates: Vec<(String, crate::wildcard::WildcardValues)> = Vec::new();
        for combo in crate::wildcard::discover_wildcards_from_pattern_tree(&base, &pattern)? {
            let path = crate::wildcard::expand_pattern(&pattern, &combo)
                .unwrap_or_else(|_| pattern.clone());
            candidates.push((path, combo));
        }

        // Source 2: literal outputs of producers already materialized —
        // the files this workflow itself will create. Matching the
        // config-expanded output string against the pattern regex
        // extracts the same wildcard values a disk scan would.
        for output in producer_outputs {
            let Some(captures) = pattern_re.captures(output) else {
                continue;
            };
            let mut combo = crate::wildcard::WildcardValues::new();
            let mut any = false;
            for name in &pattern_wildcards {
                if let Some(m) = captures.name(name) {
                    combo.insert(name.clone(), m.as_str().to_string());
                    any = true;
                }
            }
            if !any {
                continue;
            }
            candidates.push((output.clone(), combo));
        }

        if candidates.is_empty() {
            tracing::warn!(
                rule = %rule.name,
                "input_groups pattern '{}' matched no files (disk or producer outputs) — the rule is not instantiated (nothing to run)",
                decl.pattern
            );
            return Ok(Vec::new());
        }

        // The group key of a candidate: the `group_by` wildcard value, or —
        // for `group_by = "meta.<column>"` — the metadata row's column
        // value resolved from the combo's sample-like binding. A missing
        // metadata row or an empty cell yields no key: the file is skipped
        // (empty-column rows are never grouped).
        let group_key_of = |combo: &crate::wildcard::WildcardValues| -> Option<String> {
            match meta_group_by {
                Some(column) => crate::wildcard::metadata_row_for(combo, &self.metadata)
                    .and_then(|row| row.get(column))
                    .filter(|value| !value.is_empty())
                    .cloned(),
                None => combo.get(&decl.group_by).cloned(),
            }
        };

        // Group the (file, combo) pairs by the group key. Determinism
        // matters: keys sort by BTreeMap, files sort within a group, and
        // the "first occurrence" wildcard bindings come from the FIRST
        // SORTED file — never from readdir order.
        let mut grouped: std::collections::BTreeMap<
            String,
            Vec<(String, crate::wildcard::WildcardValues)>,
        > = std::collections::BTreeMap::new();
        for (path, combo) in candidates {
            let Some(key) = group_key_of(&combo) else {
                continue;
            };
            if key.is_empty() {
                continue;
            }
            grouped.entry(key).or_default().push((path, combo));
        }
        if grouped.is_empty() {
            tracing::warn!(
                rule = %rule.name,
                group_by = %decl.group_by,
                "input_groups pattern matched files but none had a group key — the rule is not instantiated (nothing to run)"
            );
            return Ok(Vec::new());
        }

        let mut out = Vec::with_capacity(grouped.len());
        for (key, mut entries) in grouped {
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries.dedup_by(|a, b| a.0 == b.0);
            let mut files = Vec::with_capacity(entries.len());
            let mut value_lists: HashMap<String, Vec<String>> = HashMap::new();
            for (path, combo) in &entries {
                files.push(path.clone());
                for name in &pattern_wildcards {
                    if let Some(v) = combo.get(name)
                        && !value_lists
                            .get(name)
                            .is_some_and(|list: &Vec<String>| list.contains(v))
                    {
                        value_lists.entry(name.clone()).or_default().push(v.clone());
                    }
                }
            }
            // The instance wildcard map: the group key + the first
            // occurrence of every kept wildcard (for `{output}` etc.).
            // Metadata grouping binds the group key under the COLUMN name
            // (`group_by = "meta.antibody"` → `{antibody}`) and never
            // binds pattern wildcards (`keep` is rejected above).
            let mut instance_map = crate::wildcard::WildcardValues::new();
            let key_name = meta_group_by.unwrap_or(decl.group_by.as_str());
            instance_map.insert(key_name.to_string(), key.clone());
            for name in &keep {
                if let Some(v) = entries[0].1.get(name) {
                    instance_map.insert(name.clone(), v.clone());
                }
            }
            // Space-joined per-group value lists for `{input_group.<name>}`.
            // The distinct sample values are kept for readiness attribution.
            let mut group_lists = HashMap::new();
            let mut group_samples: Vec<String> = Vec::new();
            for (name, values) in value_lists {
                if name == "sample" {
                    group_samples = values.clone();
                }
                group_lists.insert(name, values.join(" "));
            }

            // ── Build the instance ──────────────────────────────────────
            let orig_name = rule.name.clone();
            let new_name = format!(
                "{}_{}",
                rule.name,
                crate::wildcard::sanitize_instance_value(&key)
            );
            let mut expanded = rule.clone();
            expanded.name = new_name;
            // Instances never re-expand: the declaration is consumed by
            // this fan-out.
            expanded.input_groups.clear();

            // Group files come FIRST (sorted, stable order), the declared
            // `input` entries append after — both resolved against the
            // instance map and `{input_group.*}` lists.
            let mut inputs = files;
            inputs.extend(
                rule.input
                    .to_vec()
                    .into_iter()
                    .map(|p| expand_group_text(&p, &instance_map, &group_lists)),
            );
            expanded.input = FilePatterns::List(inputs);
            expanded.output = match &rule.output {
                FilePatterns::List(v) => FilePatterns::List(
                    v.iter()
                        .map(|p| expand_group_text(p, &instance_map, &group_lists))
                        .collect(),
                ),
                FilePatterns::Map(m) => FilePatterns::Map(
                    m.iter()
                        .map(|(k, v)| {
                            (k.clone(), expand_group_text(v, &instance_map, &group_lists))
                        })
                        .collect(),
                ),
                FilePatterns::Dir { path, pattern } => FilePatterns::Dir {
                    path: expand_group_text(path, &instance_map, &group_lists),
                    pattern: pattern.clone(),
                },
            };
            if let Some(ref shell) = rule.shell {
                expanded.shell = Some(expand_group_text(shell, &instance_map, &group_lists));
            }
            if let Some(ref log) = rule.log {
                expanded.log = Some(expand_group_text(log, &instance_map, &group_lists));
            }
            // Script and hooks take the same per-instance substitution
            // (issue #98) — same class as shell/log.
            expand_command_text_fields(&mut expanded, rule, |s| {
                expand_group_text(s, &instance_map, &group_lists)
            });
            // Per-instance `when` filtering (snakemake-style DAG morphing):
            // wildcard references bake like the pair/group branches;
            // `{input_group.*}` and bare `{sample}`-style placeholders
            // resolve from the instance map.
            if let Some(ref when) = rule.when {
                let baked = if when.contains("wildcard.") {
                    Self::bake_wildcard_when(when, &instance_map)
                } else {
                    when.clone()
                };
                expanded.when = Some(expand_group_text(&baked, &instance_map, &group_lists));
            }
            // Per-instance `{meta.<column>}` substitution (issue #227 item
            // 2): a plain group binds `{sample}` so the row lookup works;
            // a metadata-grouped instance binds only the column name, so no
            // row resolves and `{meta.*}` renders empty (the instance has
            // no single sample — an ambiguous lookup must not pick one).
            self.apply_instance_meta(&mut expanded, &instance_map);

            // Metadata-grouped instances have no single pattern-wildcard
            // binding — `{sample}` in an output is an authoring error (the
            // instance's one binding is the column name, e.g. `{antibody}`).
            // Fail the plan instead of running a literal token. The column
            // name itself is exempt (a pattern may legitimately carry the
            // same wildcard as the metadata column).
            if let Some(column) = meta_group_by
                && let Some(wild) = pattern_wildcards.iter().find(|w| {
                    w.as_str() != column
                        && expanded
                            .output
                            .iter()
                            .any(|out| out.contains(&format!("{{{w}}}")))
                })
            {
                return Err(OxoFlowError::Validation {
                    message: format!(
                        "outputs of input_groups rule '{}' use '{{{wild}}}' but metadata-grouped \
                         instances have no single '{{{wild}}}' binding — the group key \
                         ('{{{column}}}') is the instance's only binding",
                        expanded.name
                    ),
                    rule: Some(rule.name.clone()),
                    suggestion: Some(format!(
                        "use '{{{column}}}' for the group key or '{{{{input_group.{wild}}}}}' for the group's values"
                    )),
                });
            }

            // A leftover `{input_group.<name>}` means the author referenced
            // a wildcard this pattern never captures — fail the plan
            // instead of running a literal token.
            let mut known: Vec<&str> = group_lists.keys().map(String::as_str).collect();
            known.sort_unstable();
            let mut residual: Vec<&str> = expanded
                .input
                .iter()
                .map(String::as_str)
                .chain(expanded.output.iter().map(String::as_str))
                .collect();
            if let Some(ref shell) = expanded.shell {
                residual.push(shell);
            }
            if let Some(ref log) = expanded.log {
                residual.push(log);
            }
            if let Some(ref script) = expanded.script {
                residual.push(script);
            }
            if let Some(ref w) = expanded.when {
                residual.push(w);
            }
            if let Some(bad) = residual.iter().find(|t| t.contains("{input_group.")) {
                return Err(OxoFlowError::Validation {
                    message: format!(
                        "rule '{}' references unknown `{{{{input_group.<wildcard>}}}}` \
                         placeholder in '{}' — input_groups exposes: {}",
                        expanded.name,
                        bad,
                        known.join(", ")
                    ),
                    rule: Some(rule.name.clone()),
                    suggestion: Some(
                        "fix the wildcard name, or add it to the input_groups pattern".to_string(),
                    ),
                });
            }

            // Readiness attribution (issue #63): the group key is the
            // sample in the common case; metadata-grouped instances
            // attribute their group's sample values instead.
            if meta_group_by.is_some() {
                self.expansion_samples
                    .insert(expanded.name.clone(), group_samples);
            } else {
                self.expansion_samples
                    .insert(expanded.name.clone(), vec![key]);
            }
            // Per-instance bindings so expand_inputs patterns referencing
            // `{sample}` / kept wildcards resolve per instance (same
            // contract as the pair/group branches).
            self.expansion_values
                .insert(expanded.name.clone(), instance_map);
            self.expansion_templates
                .insert(expanded.name.clone(), orig_name);

            out.push(expanded);
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // Runtime-discovered output fan-out (issue #227 item 5)
    // -----------------------------------------------------------------------

    /// Scan the filesystem for the files this producer instance has just
    /// created. The instance's BAKED `output_pattern` (bound wildcards
    /// already resolved, `{config.x}` expanded from the workflow config)
    /// is matched against the tree rooted at `root`; every discovered
    /// combo is merged with the instance's own bindings (`[[values]]` /
    /// `[[pairs]]`) so the FULL wildcard map is reconstructed — the
    /// per-sample union source for downstream consumers.
    pub fn discover_output_pattern_files(
        &self,
        instance: &Rule,
        root: &std::path::Path,
    ) -> Result<Vec<crate::wildcard::WildcardValues>> {
        let Some(ref pattern) = instance.output_pattern else {
            return Ok(Vec::new());
        };
        let pattern = expand_config_vars_in_path(pattern, &self.config);
        let combos = if pattern.contains('/') {
            crate::wildcard::discover_wildcards_from_pattern_tree(root, &pattern)?
        } else {
            crate::wildcard::discover_wildcards_from_pattern(root, &pattern)?
        };
        let bindings = self.instance_bindings(&instance.name);
        let mut full = Vec::with_capacity(combos.len());
        for mut combo in combos {
            for (k, v) in &bindings {
                combo.entry(k.clone()).or_insert_with(|| v.clone());
            }
            full.push(combo);
        }
        Ok(full)
    }

    /// Contribute a producer instance's discovered combos to the template's
    /// accumulated domain — the UNION across producer instances, deduped
    /// by sorted key=value. Returns the number of NEW combos added.
    pub fn contribute_output_pattern_domain(
        &mut self,
        producer_template: &str,
        combos: Vec<crate::wildcard::WildcardValues>,
    ) -> usize {
        let entry = self
            .discovered_output_patterns
            .entry(producer_template.to_string())
            .or_default();
        let mut added = 0;
        for combo in combos {
            let key = wildcard_combo_key(&combo);
            if !entry
                .iter()
                .any(|existing| wildcard_combo_key(existing) == key)
            {
                entry.push(combo);
                added += 1;
            }
        }
        added
    }

    /// The producer TEMPLATE name a rule belongs to, when the rule is an
    /// output_pattern producer instance (or an unexpanded producer
    /// itself). `None` for every other rule.
    pub fn output_pattern_template_of(&self, name: &str) -> Option<String> {
        let rule = self.get_rule(name)?;
        rule.output_pattern.as_ref()?;
        Some(
            self.expansion_templates
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.to_string()),
        )
    }

    /// Instantiate every pending consumer whose producer's discovered
    /// domain is non-empty: one instance per combo, named from the
    /// consumer template plus the producer-pattern wildcard values in
    /// pattern order (deterministic across runs and resume). Idempotent:
    /// instantiated consumers leave the pending set; a re-expansion
    /// rebuilds the pending set from templates and re-instantiates the
    /// same names.
    pub fn expand_output_pattern_consumers(&mut self) -> Result<Vec<String>> {
        // Resolve each consumer's producer BEFORE draining the pending set
        // — `output_pattern_producer_of` consults it too.
        let producers: Vec<Option<String>> = self
            .pending_output_pattern
            .iter()
            .map(|c| self.output_pattern_producer_of(&c.name))
            .collect();
        let pending = std::mem::take(&mut self.pending_output_pattern);
        let mut new_names = Vec::new();
        let mut still_pending = Vec::new();
        for (consumer, producer) in pending.into_iter().zip(producers) {
            let Some(producer) = producer else {
                // Not attached to any producer (defensive): keep pending.
                still_pending.push(consumer);
                continue;
            };
            // The producer TEMPLATE's pattern wildcards drive instance
            // naming — stable even when the producer fanned out over
            // bound wildcards ({sample}, …).
            let pattern_wildcards: Vec<String> = self
                .rule_templates
                .iter()
                .find(|r| r.name == producer)
                .and_then(|r| r.output_pattern.as_deref())
                .map(crate::wildcard::extract_wildcards)
                .unwrap_or_default();
            if pattern_wildcards.is_empty() {
                still_pending.push(consumer);
                continue;
            }
            let Some(domain) = self.discovered_output_patterns.get(&producer) else {
                still_pending.push(consumer);
                continue;
            };
            if domain.is_empty() {
                still_pending.push(consumer);
                continue;
            }
            // The caller gates on ALL producer instances having completed,
            // so the domain is final when this pass runs. Sort it by the
            // pattern wildcard values so instantiation order (and the
            // reported names) is deterministic across runs and resume —
            // the discovery walkers see filesystem order, which is not.
            let mut sorted: Vec<&crate::wildcard::WildcardValues> = domain.iter().collect();
            sorted.sort_by_key(|combo| {
                pattern_wildcards
                    .iter()
                    .map(|w| combo.get(w).cloned().unwrap_or_default())
                    .collect::<Vec<String>>()
            });
            for combo in sorted {
                let mut name = consumer.name.clone();
                for w in &pattern_wildcards {
                    name.push('_');
                    name.push_str(&crate::wildcard::sanitize_instance_value(
                        combo.get(w).map(String::as_str).unwrap_or_default(),
                    ));
                }
                if self.rules.iter().any(|r| r.name == name) {
                    // A previous instance of THIS consumer (post-reentry
                    // re-expansion) is an idempotent no-op; any other
                    // collision is a genuine naming conflict.
                    if self
                        .expansion_templates
                        .get(&name)
                        .is_some_and(|t| t == &consumer.name)
                    {
                        continue;
                    }
                    return Err(OxoFlowError::DuplicateRule { name });
                }
                let mut instance = consumer.clone();
                instance.name = name.clone();
                instance.input = expand_rule_patterns(&consumer.input, combo);
                instance.output = expand_rule_patterns(&consumer.output, combo);
                if let Some(ref shell) = consumer.shell {
                    instance.shell = Some(expand_rule_shell(shell, combo));
                }
                if let Some(ref log) = consumer.log {
                    instance.log = Some(expand_rule_shell(log, combo));
                }
                if let Some(ref op) = consumer.output_pattern {
                    instance.output_pattern = Some(expand_rule_shell(op, combo));
                }
                if let Some(ref when) = consumer.when
                    && when.contains("wildcard.")
                {
                    instance.when = Some(Self::bake_wildcard_when(when, combo));
                }
                expand_command_text_fields(&mut instance, &consumer, |s| {
                    expand_rule_shell(s, combo)
                });
                self.apply_instance_meta(&mut instance, combo);
                // Readiness attribution (issue #63): the producer-pattern
                // wildcard bindings of this instance.
                let samples: Vec<String> = pattern_wildcards
                    .iter()
                    .filter_map(|w| combo.get(w).cloned())
                    .collect();
                if !samples.is_empty() {
                    self.expansion_samples.insert(name.clone(), samples);
                }
                self.expansion_templates
                    .insert(name.clone(), consumer.name.clone());
                self.rules.push(instance);
                new_names.push(name);
            }
        }
        self.pending_output_pattern = still_pending;
        Ok(new_names)
    }

    /// Resolve split values from SplitConfig.
    pub(crate) fn resolve_split_values(
        &self,
        split: &crate::rule::SplitConfig,
    ) -> Result<Vec<String>> {
        // Priority: values > values_from > n > glob
        if !split.values.is_empty() {
            return Ok(split.values.clone());
        }

        if let Some(ref values_from) = split.values_from {
            if let Some(vals) = self.resolve_config_list(values_from) {
                return Ok(vals);
            }
            return Err(OxoFlowError::Validation {
                message: format!("cannot resolve split.values_from: {}", values_from),
                rule: None,
                suggestion: Some("ensure config variable exists and is an array".to_string()),
            });
        }

        if let Some(ref n_str) = split.n {
            // Resolve n from config (config.<key> or bare <key>) or parse as number
            let n = self
                .resolve_config_list(n_str)
                .and_then(|v| v.first().and_then(|s| s.parse::<usize>().ok()))
                .unwrap_or_else(|| n_str.parse::<usize>().unwrap_or(1));
            // Generate chunk indices: 0, 1, 2, ..., n-1
            return Ok((0..n).map(|i| i.to_string()).collect());
        }

        if let Some(ref glob) = split.glob {
            // Glob expansion - find matching files
            let matches: Vec<String> = glob::glob(glob)
                .map_err(|e| OxoFlowError::Validation {
                    message: format!("invalid glob pattern: {}", e),
                    rule: None,
                    suggestion: None,
                })?
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            if matches.is_empty() {
                return Err(OxoFlowError::Validation {
                    message: format!("glob pattern '{}' matched no files", glob),
                    rule: None,
                    suggestion: Some("check the glob path and ensure files exist".to_string()),
                });
            }
            return Ok(matches);
        }

        Ok(Vec::new())
    }

    /// Resolve a config variable (e.g., "config.samples") into a list of strings.
    ///
    /// Accepts both the `config.<key>` form and a bare `<key>` reference.
    ///
    /// String values are split on commas (trimmed, empties dropped) so
    /// engine-injected comma-joined lists like `config.samples_list`,
    /// `config.pairs_list`, and `config.samples_<group>` expand per value.
    /// A string without commas
    /// still resolves to a single value; use a single-element array
    /// (e.g. `["a,b"]`) to force a comma-containing string to stay whole.
    pub fn resolve_config_list(&self, var: &str) -> Option<Vec<String>> {
        let key = var.strip_prefix("config.").unwrap_or(var);
        let val = self.config.get(key)?;
        if let Some(arr) = val.as_array() {
            return Some(
                arr.iter()
                    .map(|v| match v {
                        toml::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect(),
            );
        } else if let Some(s) = val.as_str() {
            return Some(
                s.split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
                    .collect(),
            );
        }
        None
    }

    /// Resolve environment for a rule, checking env_group first.
    ///
    /// Returns the environment spec from the following sources (in order):
    /// 1. `env_groups[group_name]` if `rule.env_group` is set
    /// 2. `rule.environment` if not empty
    /// 3. `defaults.environment` as fallback
    pub fn resolve_environment(&self, rule: &Rule) -> Option<EnvironmentSpec> {
        // Check env_group first
        if let Some(ref group_name) = rule.env_group
            && let Some(env) = self.env_groups.get(group_name)
        {
            return Some(env.clone());
        }
        // Fall back to rule's environment if not empty
        if !rule.environment.is_empty() {
            return Some(rule.environment.clone());
        }
        // Fall back to defaults
        self.defaults.environment.clone()
    }
}

/// Expand a rule text field with an input_groups instance: bake the bare
/// wildcard bindings (`{sample}`, kept wildcards) and then the
/// space-joined `{input_group.<wildcard>}` lists. `{input}` and the other
/// execution-time placeholders never match the `\w+` placeholder regex
/// and pass through untouched.
fn expand_group_text(
    text: &str,
    combo: &crate::wildcard::WildcardValues,
    lists: &HashMap<String, String>,
) -> String {
    let mut out = expand_rule_shell(text, combo);
    for (name, joined) in lists {
        out = out.replace(&format!("{{input_group.{name}}}"), joined);
    }
    out
}

/// Compact dedup key for a wildcard combo: sorted `key=value` parts joined
/// by commas — the same canonical form the discovery walkers use, so a
/// rediscovered combo can never double-contribute (issue #227 item 5).
fn wildcard_combo_key(combo: &crate::wildcard::WildcardValues) -> String {
    crate::wildcard::wildcard_combo_key(combo)
}
