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

/// Matches the namespaced `{values.name}` wildcard form.
///
/// The engine's placeholder regex (`\w+` only) cannot match dotted names, so
/// this namespace is detected and substituted textually — see
impl WorkflowConfig {
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

        // Workflow order: group order, then within-group order, deduplicated.
        let ordered: Vec<String> = {
            let mut out = Vec::new();
            for group in &self.sample_groups {
                for s in &group.samples {
                    if !out.contains(s) {
                        out.push(s.clone());
                    }
                }
            }
            out
        };

        let allowed: std::collections::HashSet<String> = if let Some(n) = take_first {
            ordered
                .iter()
                .take(n)
                .cloned()
                .chain(explicit.iter().cloned())
                .collect()
        } else {
            explicit.iter().cloned().collect()
        };
        let kept: Vec<String> = ordered
            .iter()
            .filter(|s| allowed.contains(*s))
            .cloned()
            .collect();
        // Pair experiment/control names are valid sample identifiers too —
        // they must not be reported as unknown (issue #63 feeds resolved
        // `ready` names through this path).
        let unknown: Vec<String> = explicit
            .iter()
            .filter(|name| {
                !ordered.iter().any(|s| s == name.as_str())
                    && !self.pairs.iter().any(|p| {
                        p.experiment == name.as_str()
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
        // Keep the injected config.pairs_list / samples_list in sync with
        // the surviving sets — including the empty case, so a filter that
        // drops EVERY pair/sample cannot leave a stale list behind for
        // expand_inputs to resolve against rules that no longer exist.
        let mut pair_ids: Vec<String> = self.pairs.iter().map(|p| p.pair_id.clone()).collect();
        pair_ids.sort();
        pair_ids.dedup();
        self.config.insert(
            "pairs_list".to_string(),
            toml::Value::String(pair_ids.join(",")),
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
