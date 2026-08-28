//! Facade for the workflow-config domain (issue #206).
//! Physical layout split only: public paths are unchanged.

mod expand;
mod known_keys;
mod model;
mod parse;
mod references;
mod samples;
#[cfg(test)]
mod tests;

pub use model::*;
pub(crate) use model::{
    expand_command_text_fields, expand_config_vars_in_path, expand_rule_patterns,
    expand_rule_shell, value_instance_suffix,
};
pub use parse::resolve_rule_templates;

use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;

impl WorkflowConfig {
    /// Validate the workflow configuration for internal consistency.
    #[must_use = "validation returns a Result that must be checked"]
    pub fn validate(&self) -> Result<()> {
        // Check for duplicate rule names
        let mut seen = std::collections::HashSet::new();
        for rule in &self.rules {
            if !seen.insert(&rule.name) {
                return Err(OxoFlowError::DuplicateRule {
                    name: rule.name.clone(),
                });
            }
        }

        // Ensure each rule has either shell, script, or transform
        for rule in &self.rules {
            if rule.shell.is_none()
                && rule.script.is_none()
                && rule.transform.is_none()
                && !rule.output.is_empty()
            {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "rule '{}' has outputs but no shell command, script, or transform",
                        rule.name
                    ),
                });
            }
        }

        self.validate_execution_groups()?;

        // Include interface contracts (issue #112): contract errors fail
        // fast with the wiring gap named; encapsulation gaps warn.
        let (contract_errors, contract_warnings) = self.check_include_contracts();
        if let Some(first) = contract_errors.first() {
            return Err(OxoFlowError::Config {
                message: first.clone(),
            });
        }
        for warning in &contract_warnings {
            tracing::warn!("{warning}");
        }

        // Validate [[references]] entries: builder template names must be
        // known, template builds must declare an output, names must be unique.
        crate::references::validate_reference_defs(&self.references)?;

        // Warn about rules exceeding system capacity (but don't block)
        let system_threads = num_cpus::get() as u32;
        let system_memory_mb = {
            use sysinfo::System;
            // Only memory is needed here; `System::new_all()` would walk all of
            // /proc (every process, disk, and network interface) just to read
            // total RAM, adding ~50ms to every parse/validate/dry-run/run call.
            let mut sys = System::new();
            sys.refresh_memory();
            sys.total_memory() / 1024 / 1024
        };

        for rule in &self.rules {
            for warning in crate::scheduler::validate_resources_against_system(
                rule,
                system_threads,
                system_memory_mb,
            ) {
                tracing::warn!("{}", warning);
            }
        }

        // Validate wildcard constraints
        for (name, pattern) in &self.wildcard_constraints {
            if let Err(e) = regex::Regex::new(pattern) {
                return Err(OxoFlowError::Config {
                    message: format!("invalid regex for wildcard constraint '{}': {}", name, e),
                });
            }
        }

        Ok(())
    }

    /// True when `key` was injected by the engine at parse time (reference
    /// keyed-config values or reference_dir-derived paths), not written by
    /// the user. Run-time injections (samples_list / pairs_list /
    /// samples_*) are covered by `config_impact::is_engine_injected_key`.
    pub fn is_injected_config_key(&self, key: &str) -> bool {
        self.injected_config_keys.contains(key)
    }

    /// Get a rule by name.
    pub fn get_rule(&self, name: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.name == name)
    }

    /// Per-instance wildcard bindings: the union of the `[[values]]`
    /// table bindings and the `[[pairs]]` bindings for this instance
    /// name (runtime-discovered fan-out reconstruction, issue #227 item 5).
    pub(crate) fn instance_bindings(&self, name: &str) -> crate::wildcard::WildcardValues {
        let mut bindings = crate::wildcard::WildcardValues::new();
        if let Some(values) = self.expansion_values.get(name) {
            bindings.extend(values.clone());
        }
        if let Some(pairs) = self.expansion_pairs.get(name) {
            bindings.extend(pairs.clone());
        }
        bindings
    }

    /// The output_pattern producer (template) that owns the fresh wildcard
    /// a consumer references, or `None` when the rule is not an
    /// output_pattern consumer. Works for both template names and
    /// expanded instance names.
    pub fn output_pattern_producer_of(&self, rule_name: &str) -> Option<String> {
        // Deferred consumers live in the pending set, not in `rules`.
        let rule = self.get_rule(rule_name).or_else(|| {
            self.pending_output_pattern
                .iter()
                .find(|r| r.name == rule_name)
        })?;
        // `consumer_scan_text` excludes the rule's OWN output_pattern, so a
        // pure producer (no fresh refs in its consumer fields) resolves to
        // None; a chained rule (consumer AND producer) resolves to the
        // producer it consumes.
        let refs = consumer_scan_text(rule);
        // Invert the fresh-wildcard → producer map, then take the first
        // producer whose vocabulary appears in the consumer's scan text.
        self.output_pattern_producers
            .iter()
            .find(|(wildcard, _)| {
                refs.iter()
                    .any(|text| text.contains(&format!("{{{}}}", wildcard)))
            })
            .map(|(_, producer)| producer.clone())
    }

    /// Pending (not yet runtime-instantiated) consumers of a producer —
    /// used for declaration-order diagnosis and failure attribution.
    pub fn pending_output_pattern_consumers_of(&self, producer: &str) -> Vec<String> {
        // The producer TEMPLATE may no longer be in `rules` (it was
        // replaced by its instances when it fanned out over bound
        // wildcards like `{sample}`) — its pattern lives in
        // `rule_templates`, which retains every template.
        let producer_wildcards: Vec<String> = self
            .rule_templates
            .iter()
            .find(|r| r.name == producer)
            .and_then(|r| r.output_pattern.as_deref())
            .map(crate::wildcard::extract_wildcards)
            .or_else(|| {
                self.get_rule(producer)
                    .and_then(|r| r.output_pattern.as_deref())
                    .map(crate::wildcard::extract_wildcards)
            })
            .unwrap_or_default();
        if producer_wildcards.is_empty() {
            return Vec::new();
        }
        self.pending_output_pattern
            .iter()
            .filter(|consumer| {
                // A pending consumer references the producer's fresh
                // wildcard vocabulary in one of its scanned fields.
                let refs = consumer_scan_text(consumer);
                refs.iter().any(|text| {
                    producer_wildcards
                        .iter()
                        .any(|w| text.contains(&format!("{{{w}}}")))
                })
            })
            .map(|r| r.name.clone())
            .collect()
    }

    /// Get a config value by key.
    pub fn get_config_value(&self, key: &str) -> Option<&toml::Value> {
        self.config.get(key)
    }

    /// Returns the list of all rule names.
    pub fn rule_names(&self) -> Vec<&str> {
        self.rules.iter().map(|r| r.name.as_str()).collect()
    }

    /// The template rule a fanned-out instance was expanded from, if any
    /// (issue #74 phase 3). Rules that never fan out have no entry.
    pub fn template_of(&self, instance: &str) -> Option<&str> {
        self.expansion_templates.get(instance).map(String::as_str)
    }

    /// Compute a SHA-256 checksum of the workflow configuration for reproducibility.
    ///
    /// The checksum is computed from a deterministic hash of the config,
    /// ensuring consistent results regardless of field ordering.
    pub fn checksum(&self) -> String {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::hash::DefaultHasher::new();
        self.workflow.name.hash(&mut hasher);
        self.workflow.version.hash(&mut hasher);
        self.rules.len().hash(&mut hasher);
        for rule in &self.rules {
            rule.name.hash(&mut hasher);
            rule.input.hash(&mut hasher);
            rule.output.hash(&mut hasher);
            rule.shell.hash(&mut hasher);
            rule.script.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }
}
