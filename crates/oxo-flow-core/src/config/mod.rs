//! Facade for the workflow-config domain (issue #206).
//! Physical layout split only: public paths are unchanged.

mod expand;
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
