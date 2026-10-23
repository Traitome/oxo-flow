//! Sample-group management. (issue #206 extraction).
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
use std::collections::HashMap;

/// One concrete output-path collision between two differently-owned
/// instances of the same sample id (issue #871).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OwnerCollision {
    /// The duplicated sample id.
    pub sample: String,
    /// First declaring owner (e.g. `group 'case'`).
    pub owner_a: String,
    /// Second declaring owner (e.g. `pair 'P1'`).
    pub owner_b: String,
    /// The rule writing `path` on the first owner's side.
    pub rule_a: String,
    /// The rule writing `path` on the second owner's side (same as
    /// `rule_a` when one rule collides with itself).
    pub rule_b: String,
    /// The exact path both instances would write.
    pub path: String,
}

/// Matches the namespaced `{values.name}` wildcard form.
///
/// The engine's placeholder regex (`\w+` only) cannot match dotted names, so
/// this namespace is detected and substituted textually — see
impl WorkflowConfig {
    /// Reject user-declared sample groups named `auto-discovered` — the
    /// engine's own group for `sample_pattern` discovery results
    /// ([`AUTO_DISCOVERED_GROUP_NAME`](crate::config::AUTO_DISCOVERED_GROUP_NAME),
    /// issue #895). A user group with that name is indistinguishable from
    /// the engine group: the `duplicate_sample_owners` pair-exemption keys
    /// on the literal owner string, so a real collision with the user group
    /// would be silently swallowed, and rule instances fan out under the
    /// same `{rule}_{group}_{sample}` names.
    pub(crate) fn reject_reserved_sample_group_names(groups: &[SampleGroup]) -> Result<()> {
        for group in groups {
            if group.name == crate::config::AUTO_DISCOVERED_GROUP_NAME {
                return Err(OxoFlowError::Validation {
                    message: format!(
                        "[[sample_groups]] name '{}' is reserved — the engine uses it for \
                         sample_pattern discovery results",
                        group.name
                    ),
                    rule: None,
                    suggestion: Some(
                        "rename the group (e.g. use a cohort- or condition-specific name)"
                            .to_string(),
                    ),
                });
            }
        }
        Ok(())
    }

    /// Filter the workflow's samples to a pilot subset.
    ///
    /// Specs are `first:N` (the first N samples in workflow order) and/or
    /// explicit comma-separated sample names — both forms may be combined
    /// and repeated. Filtering is applied to every sample source
    /// (`[[sample_groups]]`, `sample_pattern` auto-discovery, sample-group
    /// files), the merged `config.samples_list` / `config.pairs_list`, and
    /// experiment/control `[[pairs]]` whose samples were filtered out.
    ///
    /// Returns `(kept, unknown)` — the kept samples in workflow order and
    /// any explicitly named samples that were not found.
    pub fn filter_samples(&mut self, specs: &[String]) -> Result<(Vec<String>, Vec<String>)> {
        let mut take_first: Option<usize> = None;
        let mut explicit: Vec<String> = Vec::new();
        for spec in specs {
            for part in spec.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                if let Some(n) = part.strip_prefix("first:") {
                    let n: usize = n.trim().parse().map_err(|_| OxoFlowError::Config {
                        message: format!(
                            "invalid --samples spec '{part}': expected first:<N> or a sample name"
                        ),
                    })?;
                    take_first = Some(take_first.map_or(n, |cur| cur.max(n)));
                } else {
                    explicit.push(part.to_string());
                }
            }
        }

        // Workflow order: group order, then within-group order, then each
        // pair's experiment/control in pair order — deduplicated. Pair
        // names are part of the selectable domain: a pairs-only workflow
        // declares no group samples, and `first:N`/explicit names must
        // still be able to select its pairs (a `--samples first:2` there
        // used to drop every pair and report "matched no samples").
        let ordered: Vec<String> = {
            let mut out = Vec::new();
            for group in &self.sample_groups {
                for s in &group.samples {
                    if !out.contains(s) {
                        out.push(s.clone());
                    }
                }
            }
            for pair in &self.pairs {
                for name in std::iter::once(&pair.experiment).chain(pair.control.iter()) {
                    if !out.contains(name) {
                        out.push(name.clone());
                    }
                }
            }
            out
        };

        let mut allowed: std::collections::HashSet<String> = if let Some(n) = take_first {
            ordered.iter().take(n).cloned().collect()
        } else {
            std::collections::HashSet::new()
        };
        for name in &explicit {
            allowed.insert(name.clone());
            // A pair ID is a selectable identifier too: naming the pair
            // selects both of its samples, so `--samples P1` keeps P1.
            for pair in self.pairs.iter().filter(|p| p.pair_id == *name) {
                allowed.insert(pair.experiment.clone());
                if let Some(ref control) = pair.control {
                    allowed.insert(control.clone());
                }
            }
        }
        let kept: Vec<String> = ordered
            .iter()
            .filter(|s| allowed.contains(*s))
            .cloned()
            .collect();
        // Pair experiment/control names are valid sample identifiers too —
        // they must not be reported as unknown (issue #63 feeds resolved
        // `ready` names through this path). Pair IDs select their pair and
        // are known for the same reason.
        let unknown: Vec<String> = explicit
            .iter()
            .filter(|name| {
                !ordered.iter().any(|s| s == name.as_str())
                    && !self.pairs.iter().any(|p| {
                        p.pair_id == name.as_str()
                            || p.experiment == name.as_str()
                            || p.control.as_deref().is_some_and(|c| c == name.as_str())
                    })
            })
            .cloned()
            .collect();

        // Filter every sample source and the merged samples_list.
        for group in &mut self.sample_groups {
            group.samples.retain(|s| allowed.contains(s));
        }
        self.pairs.retain(|p| {
            allowed.contains(&p.experiment)
                && p.control.as_ref().is_none_or(|c| allowed.contains(c))
        });
        // Keep the injected config.pairs_list / paired_pairs_list /
        // samples_list in sync with the surviving sets — including the
        // empty case, so a filter that
        // drops EVERY pair/sample cannot leave a stale list behind for
        // expand_inputs to resolve against rules that no longer exist.
        let mut pair_ids: Vec<String> = self.pairs.iter().map(|p| p.pair_id.clone()).collect();
        pair_ids.sort();
        pair_ids.dedup();
        self.config.insert(
            "pairs_list".to_string(),
            toml::Value::String(pair_ids.join(",")),
        );
        // Keep the paired subset (#881) in sync with the surviving pairs.
        let mut paired_ids: Vec<String> = self
            .pairs
            .iter()
            .filter(|p| p.control.as_deref().is_some_and(|c| !c.is_empty()))
            .map(|p| p.pair_id.clone())
            .collect();
        paired_ids.sort();
        paired_ids.dedup();
        self.config.insert(
            "paired_pairs_list".to_string(),
            toml::Value::String(paired_ids.join(",")),
        );
        self.config.insert(
            "paired_pairs_count".to_string(),
            toml::Value::Integer(paired_ids.len() as i64),
        );
        self.config.insert(
            "samples_list".to_string(),
            toml::Value::String(kept.join(",")),
        );
        for group in &self.sample_groups {
            self.config.insert(
                format!("samples_{}", group.name),
                toml::Value::String(group.samples.join(",")),
            );
        }

        Ok((kept, unknown))
    }

    /// Sample ids declared by more than one owner — a second
    /// `[[sample_groups]]` entry, or a `[[pairs]]` experiment/control that
    /// already belongs to a group. Returns `(sample, first_owner,
    /// second_owner)` in workflow order; owners are `group '<name>'` /
    /// `pair '<id>'`.
    ///
    /// A repeated id is only safe when every rule path distinguishes the
    /// owners (e.g. a `{group}` component in the output). Otherwise the two
    /// instances share an output path: the second is skipped as
    /// "outputs up-to-date" while the checkpoint records both as
    /// completed, so one sample's data can be consumed as the other's.
    /// Repeats WITHIN one owner (a sample listed twice in the same group)
    /// are not duplicates — they deduplicate to a single instance.
    ///
    /// The engine's [`AUTO_DISCOVERED_GROUP_NAME`](crate::config::AUTO_DISCOVERED_GROUP_NAME)
    /// group is exempt against `[[pairs]]`: feeding discovered samples into
    /// pairs is the documented pairing workflow, not a competing
    /// declaration. It still counts against a user-declared group (two
    /// groups fanning out the same `{sample}` is the real double-owner
    /// case).
    #[must_use]
    pub fn duplicate_sample_owners(&self) -> Vec<(String, String, String)> {
        fn record(
            owner_of: &mut HashMap<String, String>,
            duplicates: &mut Vec<(String, String, String)>,
            sample: &str,
            owner: String,
        ) {
            match owner_of.get(sample) {
                Some(first) if *first != owner => {
                    duplicates.push((sample.to_string(), first.clone(), owner));
                }
                Some(_) => {}
                None => {
                    owner_of.insert(sample.to_string(), owner);
                }
            }
        }

        let mut owner_of: HashMap<String, String> = HashMap::new();
        let mut duplicates = Vec::new();
        for group in &self.sample_groups {
            for sample in &group.samples {
                record(
                    &mut owner_of,
                    &mut duplicates,
                    sample,
                    format!("group '{}'", group.name),
                );
            }
        }
        let auto_owner = format!("group '{}'", crate::config::AUTO_DISCOVERED_GROUP_NAME);
        for pair in &self.pairs {
            let owner = format!("pair '{}'", pair.pair_id);
            for member in std::iter::once(&pair.experiment).chain(pair.control.iter()) {
                // Auto-discovery feeding pairs is the documented pairing
                // workflow (`sample_pattern` + `[[pairs]]`), not a competing
                // owner — only user-declared groups are compared here.
                if owner_of
                    .get(member)
                    .is_some_and(|first| *first == auto_owner)
                {
                    continue;
                }
                record(&mut owner_of, &mut duplicates, member, owner.clone());
            }
        }
        duplicates
    }

    /// Concrete output-path collisions between differently-owned instances
    /// of the same sample id (issue #871).
    ///
    /// A duplicate sample `(S, owner_a, owner_b)` is only dangerous when the
    /// owner-instantiated rule instances actually expand some output to the
    /// SAME path — the benign cohort shape (`qc/{sample}.html` everywhere)
    /// must stay silent. The analysis mirrors `expand_wildcards`
    /// branch-for-branch: same trigger vocabulary, same `[[values]]`
    /// Cartesian fan-out, same constraint and per-instance `when` gating,
    /// so a path reported here is a path the engine would genuinely
    /// schedule twice.
    ///
    /// Returns one collision record per duplicated sample (struct is
    /// module-private; fields carry the sample id, both owners, the
    /// colliding rule(s), and the exact path), deduplicated and sorted.
    pub fn duplicate_owner_output_collisions(&self) -> Result<Vec<OwnerCollision>> {
        use crate::wildcard::{
            validate_wildcard_constraints_compiled, wildcard_combinations_from_groups,
            wildcard_combinations_from_pairs,
        };

        const GROUP_WILDCARDS: &[&str] = &["group", "sample"];

        let duplicates = self.duplicate_sample_owners();
        if duplicates.is_empty() {
            return Ok(Vec::new());
        }

        let config_values: HashMap<String, toml::Value> = self
            .config
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let active_pairs: Vec<crate::config::ExperimentControlPair> = self
            .pairs
            .iter()
            .filter(|pair| {
                pair.when.as_deref().is_none_or(|when| {
                    crate::executor::process::evaluate_condition_with_wildcards_and_base_dir(
                        when,
                        &config_values,
                        &HashMap::new(),
                        self.base_dir.as_deref(),
                    )
                })
            })
            .cloned()
            .collect();
        let pair_combos = wildcard_combinations_from_pairs(&active_pairs);
        let group_combos = wildcard_combinations_from_groups(&self.sample_groups);
        let compiled_constraints = self.compiled_wildcard_constraints()?;

        // At parse time `rule_templates` is still empty (expand_wildcards
        // seeds it from `rules` on first entry) — mirror that here.
        let templates = if self.rule_templates.is_empty() {
            &self.rules
        } else {
            &self.rule_templates
        };

        // Per-rule domain classification happens inline in the main loop
        // below (pair precedence, mirroring expand_wildcards' trigger
        // classification). Rules mentioning neither vocabulary cannot
        // produce owner-instantiated output paths; input_groups rules fan
        // out in the post-loop pass and are out of scope here.
        let pair_wildcards = self.pair_fanout_wildcards();

        // Instantiations of one duplicate sample under one of its owners:
        // a group owner fans out (group, sample) combos containing S; a pair
        // owner fans out (active) pair combos naming P whose members
        // include S. Empty set → that owner contributes no instances.
        let owner_instantiations =
            |sample: &str, owner: &str| -> Result<Vec<crate::wildcard::WildcardValues>> {
                let mut instantiations: Vec<crate::wildcard::WildcardValues> = Vec::new();
                if owner.starts_with("group '") {
                    let group_name = owner.trim_start_matches("group '").trim_end_matches('\'');
                    for group in &self.sample_groups {
                        if group.name != group_name || !group.samples.iter().any(|s| s == sample) {
                            continue;
                        }
                        for combo in &group_combos {
                            if combo.get("group").map(String::as_str) == Some(group_name)
                                && combo.get("sample").map(String::as_str) == Some(sample)
                            {
                                instantiations.push(combo.clone());
                            }
                        }
                    }
                } else if owner.starts_with("pair '") {
                    let pair_id = owner.trim_start_matches("pair '").trim_end_matches('\'');
                    let pair_covers_sample = |p: &crate::config::ExperimentControlPair| {
                        p.pair_id == pair_id
                            && (p.experiment == sample || p.control.as_deref() == Some(sample))
                    };
                    if active_pairs.iter().any(&pair_covers_sample) {
                        for combo in &pair_combos {
                            if combo.get("pair_id").map(String::as_str) == Some(pair_id) {
                                instantiations.push(combo.clone());
                            }
                        }
                    }
                }
                Ok(instantiations)
            };
        // Output paths for one rule instance: expand the template output
        // under the merged bindings, resolve `[config.*]` references, and
        // keep only FULLY-resolved paths (a residual `{` means an unbound
        // fresh/values wildcard — skipping it can only under-report, never
        // false-positive). Config resolution runs BEFORE the brace filter:
        // `{config.out_dir}/x.vcf` only becomes filterable (or collidable)
        // after its config reference resolves, mirroring the engine's
        // config-expansion of producer outputs at DAG level.
        let instance_outputs =
            |rule: &Rule, merged: &crate::wildcard::WildcardValues| -> Result<Vec<String>> {
                let mut outputs = expand_rule_patterns(&rule.output, merged).to_vec();
                outputs.sort();
                Ok(outputs
                    .into_iter()
                    .map(|o| expand_config_vars_in_path(&o, &self.config))
                    .filter(|o| !o.contains('{'))
                    .collect())
            };

        // Per-instance gating, identical order to expand_wildcards:
        // constraints first, then per-instance `when` (both namespaces
        // baked). Returns false when the combo produces no instance.
        let instance_passes =
            |rule: &Rule, merged: &crate::wildcard::WildcardValues| -> Result<bool> {
                if validate_wildcard_constraints_compiled(merged, &compiled_constraints).is_err() {
                    return Ok(false);
                }
                if let Some(ref when) = rule.when
                    && (when.contains("wildcard.") || when.contains("{meta."))
                {
                    let combo_values = Self::expansion_when_context(merged);
                    let baked = Self::bake_wildcard_when(when, merged);
                    let baked = Self::bake_meta_when(&baked, &self.metadata, merged);
                    if !crate::executor::process::evaluate_condition_with_wildcards_and_base_dir(
                        &baked,
                        &config_values,
                        &combo_values,
                        self.base_dir.as_deref(),
                    ) {
                        return Ok(false);
                    }
                }
                Ok(true)
            };

        // Per-owner output map: which rule writes which concrete path for
        // this duplicate sample's instances under ONE owner. A rule is
        // instantiated by exactly ONE domain (pair precedence, mirroring
        // expand_wildcards' trigger classification), so a group owner
        // contributes group-domain rules expanded with its (group, sample)
        // combos and a pair owner contributes pair-domain rules expanded
        // with its pair combos — a group rule never runs on the pair side
        // and vice versa.
        let expanded_set =
            |rule: &Rule, combos: &[crate::wildcard::WildcardValues]| -> Result<Vec<String>> {
                let mut paths: Vec<String> = Vec::new();
                let tables = self.active_value_tables_for_rule(rule);
                if !tables.is_empty() {
                    let tables: Vec<&ValueGroup> = tables.iter().collect();
                    let value_combos = self.cartesian_value_combos(&tables)?;
                    for combo in combos {
                        for value_combo in &value_combos {
                            let mut instance = combo.clone();
                            instance.extend(value_combo.clone());
                            if !instance_passes(rule, &instance)? {
                                continue;
                            }
                            paths.extend(instance_outputs(rule, &instance)?);
                        }
                    }
                } else {
                    for combo in combos {
                        if !instance_passes(rule, combo)? {
                            continue;
                        }
                        paths.extend(instance_outputs(rule, combo)?);
                    }
                }
                Ok(paths)
            };

        let owner_paths = |sample: &str, owner: &str| -> Result<HashMap<String, String>> {
            let instantiations = owner_instantiations(sample, owner)?;
            if instantiations.is_empty() {
                return Ok(HashMap::new());
            }
            let wants_group_rules = owner.starts_with("group '");
            let mut map: HashMap<String, String> = HashMap::new();
            for rule in templates {
                if !rule.input_groups.is_empty() {
                    continue;
                }
                let trigger_text: Vec<String> = {
                    let mut t: Vec<String> = Vec::new();
                    for text in rule.input.iter().chain(rule.output.iter()) {
                        t.push(text.clone());
                    }
                    if let Some(ref shell) = rule.shell {
                        t.push(shell.clone());
                    }
                    if let Some(ref when) = rule.when {
                        t.push(when.clone());
                    }
                    if let Some(ref op) = rule.output_pattern {
                        t.push(op.clone());
                    }
                    t
                };
                let mentions = |wildcards: &[&str]| -> bool {
                    trigger_text.iter().any(|t| {
                        wildcards.iter().any(|w| {
                            t.contains(&format!("{{{w}}}")) || t.contains(&format!("wildcard.{w}"))
                        })
                    })
                };
                // Gate classification on non-empty combo sets, exactly like
                // expand_wildcards: when every pair (or group) entry is
                // when-gated out, rules mentioning that vocabulary fall
                // through to the other domain — keeping them classified as
                // pair-rules would silently miss a group↔group duplicate.
                let is_pair_rule = !pair_combos.is_empty()
                    && mentions(
                        &pair_wildcards
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                    );
                let is_group_rule =
                    !is_pair_rule && !group_combos.is_empty() && mentions(GROUP_WILDCARDS);
                let domain_matches = if wants_group_rules {
                    is_group_rule
                } else {
                    is_pair_rule
                };
                if !domain_matches {
                    continue;
                }
                for path in expanded_set(rule, &instantiations)? {
                    map.entry(path).or_insert_with(|| rule.name.clone());
                }
            }
            Ok(map)
        };

        let mut collisions: std::collections::BTreeSet<OwnerCollision> =
            std::collections::BTreeSet::new();
        for (sample, owner_a, owner_b) in &duplicates {
            let paths_a = owner_paths(sample, owner_a)?;
            let paths_b = owner_paths(sample, owner_b)?;
            for (path, rule_a) in &paths_a {
                if let Some(rule_b) = paths_b.get(path) {
                    collisions.insert(OwnerCollision {
                        sample: sample.clone(),
                        owner_a: owner_a.clone(),
                        owner_b: owner_b.clone(),
                        rule_a: rule_a.clone(),
                        rule_b: rule_b.clone(),
                        path: path.clone(),
                    });
                }
            }
        }

        Ok(collisions.into_iter().collect())
    }

    /// The engine's dup-ownership warning, rendered once per parse
    /// (issue #871).
    ///
    /// `None` when every sample has a single owner — or when duplicates
    /// exist but no two owner-instantiated rule instances expand to the
    /// same output path (the standard tumor/normal cohort shape: groups
    /// feed `[[pairs]]`, every rule path already carries the full
    /// `{sample}` name). A REAL collision renders one line per colliding
    /// (sample, owners, rule, path), naming exactly what would be
    /// double-scheduled.
    #[must_use]
    pub fn duplicate_owner_warning(&self) -> Option<String> {
        match self.duplicate_owner_output_collisions() {
            Ok(collisions) if collisions.is_empty() => None,
            Ok(collisions) => {
                let lines: Vec<String> = collisions
                    .iter()
                    .map(|c| {
                        let rules = if c.rule_a == c.rule_b {
                            format!("rule '{}'", c.rule_a)
                        } else {
                            format!("rules '{}' and '{}'", c.rule_a, c.rule_b)
                        };
                        format!(
                            "  sample '{}' (declared by {} and {}): {} write '{}' \
                             for both instances — one is skipped as up-to-date while \
                             consuming the other's data",
                            c.sample, c.owner_a, c.owner_b, rules, c.path
                        )
                    })
                    .collect();
                Some(format!(
                    "{} from samples declared by more than one \
                     owner (cross-owner instance overlap):\n{}",
                    crate::plural(
                        lines.len(),
                        "colliding output path",
                        "colliding output paths"
                    ),
                    lines.join("\n")
                ))
            }
            // Expansion prerequisites (a bad constraint regex, an
            // unresolvable [[values]] values_from) must not silence the
            // legacy message — fall back to the cheap ownership census so
            // parse-time diagnostics stay actionable; validate/plan will
            // surface the underlying error right after.
            Err(_) => {
                let duplicates = self.duplicate_sample_owners();
                let list = duplicates
                    .iter()
                    .map(|(sample, first, second)| format!("{sample} ({first} / {second})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let note = "collision analysis could not run; unless every rule path \
                            distinguishes the instances (e.g. a {group} component), they \
                            share an output path and one is skipped as up-to-date while \
                            consuming the other's data";
                if duplicates.is_empty() {
                    None
                } else {
                    Some(format!(
                        "{} samples are declared by more than one owner: {list} — {note}",
                        duplicates.len()
                    ))
                }
            }
        }
    }

    /// Replace the workflow's sample groups outright and keep the injected
    /// config lists (`samples_list` / `samples_<group>` / `pairs_list`) and
    /// `[[pairs]]` in sync with the new set.
    ///
    /// This is the "override" path: the given groups REPLACE the inline /
    /// auto-discovered / file-loaded groups instead of filtering them. It is
    /// how the CLI lets a workflow ship with fixture samples (e.g. `S1`/`S2`)
    /// and a caller swap in real identifiers without editing the file.
    ///
    /// Returns the final deduplicated, order-preserving sample list.
    pub fn override_sample_groups(&mut self, groups: Vec<SampleGroup>) -> Result<Vec<String>> {
        Self::reject_reserved_sample_group_names(&groups)?;
        let mut final_samples: Vec<String> = Vec::new();
        for group in &groups {
            for sample in &group.samples {
                if !final_samples.iter().any(|s| s == sample) {
                    final_samples.push(sample.clone());
                }
            }
        }

        // Group names that existed before the override: their injected
        // `samples_<group>` keys must not survive when the group is gone
        // (expand_inputs would keep resolving the stale list).
        let old_group_names: std::collections::HashSet<String> =
            self.sample_groups.iter().map(|g| g.name.clone()).collect();

        self.sample_groups = groups;

        // Prune stale injected samples_<group> keys for dropped groups.
        let new_group_names: std::collections::HashSet<String> =
            self.sample_groups.iter().map(|g| g.name.clone()).collect();
        for stale in old_group_names.difference(&new_group_names) {
            self.config.remove(&format!("samples_{stale}"));
        }

        // Drop pairs whose experiment/control are no longer selected.
        self.pairs.retain(|p| {
            final_samples.iter().any(|s| s == &p.experiment)
                && p.control
                    .as_ref()
                    .is_none_or(|c| final_samples.iter().any(|s| s == c))
        });

        // Keep the injected config lists in sync with the surviving set.
        self.config.insert(
            "samples_list".to_string(),
            toml::Value::String(final_samples.join(",")),
        );
        for group in &self.sample_groups {
            self.config.insert(
                format!("samples_{}", group.name),
                toml::Value::String(group.samples.join(",")),
            );
        }
        let mut pair_ids: Vec<String> = self.pairs.iter().map(|p| p.pair_id.clone()).collect();
        pair_ids.sort();
        pair_ids.dedup();
        self.config.insert(
            "pairs_list".to_string(),
            toml::Value::String(pair_ids.join(",")),
        );
        // Keep the paired subset (#881) in sync with the surviving pairs.
        let mut paired_ids: Vec<String> = self
            .pairs
            .iter()
            .filter(|p| p.control.as_deref().is_some_and(|c| !c.is_empty()))
            .map(|p| p.pair_id.clone())
            .collect();
        paired_ids.sort();
        paired_ids.dedup();
        self.config.insert(
            "paired_pairs_list".to_string(),
            toml::Value::String(paired_ids.join(",")),
        );
        self.config.insert(
            "paired_pairs_count".to_string(),
            toml::Value::Integer(paired_ids.len() as i64),
        );

        Ok(final_samples)
    }

    /// Append sample groups on top of the workflow's current set — the
    /// "add" counterpart of [`Self::override_sample_groups`] (`+@path` on
    /// the CLI). A sheet group whose name matches an existing group extends
    /// it (union, dedup, order-preserving); new group names are added
    /// as-is. `[[pairs]]` are left untouched: appending can only ADD
    /// samples, never remove a pair's side.
    ///
    /// Returns the final deduplicated, order-preserving sample list.
    pub fn append_sample_groups(&mut self, groups: Vec<SampleGroup>) -> Result<Vec<String>> {
        Self::reject_reserved_sample_group_names(&groups)?;
        for incoming in groups {
            if let Some(existing) = self
                .sample_groups
                .iter_mut()
                .find(|g| g.name == incoming.name)
            {
                for sample in incoming.samples {
                    if !existing.samples.contains(&sample) {
                        existing.samples.push(sample);
                    }
                }
            } else {
                self.sample_groups.push(incoming);
            }
        }

        let mut final_samples: Vec<String> = Vec::new();
        for group in &self.sample_groups {
            for sample in &group.samples {
                if !final_samples.iter().any(|s| s == sample) {
                    final_samples.push(sample.clone());
                }
            }
        }
        self.config.insert(
            "samples_list".to_string(),
            toml::Value::String(final_samples.join(",")),
        );
        for group in &self.sample_groups {
            self.config.insert(
                format!("samples_{}", group.name),
                toml::Value::String(group.samples.join(",")),
            );
        }
        Ok(final_samples)
    }

    /// Override the workflow's samples with a flat list — collapses every
    /// group into a single group (reusing the first group's name, or
    /// `"samples"` when the workflow declares no groups) so `{group}` /
    /// `{sample}` expansion keeps working. See [`Self::override_sample_groups`].
    ///
    /// Returns the final deduplicated, order-preserving sample list.
    pub fn override_samples(&mut self, names: &[String]) -> Result<Vec<String>> {
        let mut final_samples: Vec<String> = Vec::new();
        for name in names {
            let name = name.trim();
            if !name.is_empty() && !final_samples.iter().any(|s| s == name) {
                final_samples.push(name.to_string());
            }
        }

        // Reuse the first group's name so `{group}` references keep resolving;
        // fall back to `"samples"` when the workflow declares no groups.
        let group_name = self
            .sample_groups
            .first()
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "samples".to_string());

        self.override_sample_groups(vec![SampleGroup {
            name: group_name,
            samples: final_samples,
            metadata: HashMap::new(),
        }])
    }

    /// Validate a sample sheet CSV/TSV: check that it has a header row,
    /// no duplicate sample IDs, and at least one data row.
    #[must_use]
    pub fn validate_sample_sheet(content: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            warnings.push("Sample sheet is empty".to_string());
            return warnings;
        }
        // Detect delimiter
        let delimiter = if lines[0].contains('\t') { '\t' } else { ',' };
        let header: Vec<&str> = lines[0].split(delimiter).collect();
        if header.is_empty() {
            warnings.push("Sample sheet header is empty".to_string());
            return warnings;
        }
        if lines.len() < 2 {
            warnings.push("Sample sheet has no data rows".to_string());
            return warnings;
        }
        // Check for duplicate IDs in the first column
        let mut seen = std::collections::HashSet::new();
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(delimiter).collect();
            if let Some(id) = fields.first()
                && !seen.insert(*id)
            {
                warnings.push(format!("Duplicate sample ID '{}' at line {}", id, i + 1));
            }
        }
        warnings
    }
}
