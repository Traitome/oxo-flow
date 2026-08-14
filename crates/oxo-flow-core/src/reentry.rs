//! Checkpoint re-entry (issue #78 P3): static + dynamic hybrid DAG.
//!
//! A `checkpoint = true` rule writes a TOML manifest at runtime declaring new
//! wildcard values (new samples); the engine merges them and re-expands the
//! rule templates — every round is still a static plan, so previews stay
//! deterministic and resumes reconstruct the same plan.

use crate::config::{SampleGroup, WorkflowConfig};
use crate::error::{OxoFlowError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Hard cap on re-entry rounds: a checkpoint rule that keeps discovering new
/// values past this point is a workflow bug, not an engine feature.
pub const MAX_REENTRY_ROUNDS: u32 = 32;

/// One re-entry contribution recorded in the checkpoint: the values a
/// checkpoint rule added to the plan, so resumes can replay them
/// deterministically (and revoke them when the rule is invalidated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReentryRecord {
    /// Global re-entry round (1-based) this record was produced in.
    pub round: u32,
    /// The checkpoint rule (instance) that produced it.
    pub rule: String,
    /// Target sample group; `None` = "auto-discovered".
    pub group: Option<String>,
    /// New wildcard values appended (dedup) to the group.
    pub samples: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    reentry: ReentryTable,
}

#[derive(Debug, Deserialize)]
struct ReentryTable {
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    sample: Vec<String>,
}

/// Parse a checkpoint re-entry manifest: `(group, samples)`.
pub fn parse_manifest(content: &str) -> Result<(Option<String>, Vec<String>)> {
    let m: Manifest = toml::from_str(content).map_err(|e| OxoFlowError::Config {
        message: format!("invalid re-entry manifest: {e}"),
    })?;
    Ok((m.reentry.group, m.reentry.sample))
}

/// Merge new samples into the target group (dedup) and re-expand from the
/// rule templates. Returns the names of newly created instances (already in
/// `config.rules`).
pub fn apply_reentry(
    config: &mut WorkflowConfig,
    group: Option<&str>,
    samples: &[String],
) -> Result<Vec<String>> {
    let prev: HashSet<String> = config.rules.iter().map(|r| r.name.clone()).collect();
    let group_name = resolve_group_name(config, group);
    let added = merge_samples(config, &group_name, samples);
    if added.is_empty() {
        return Ok(Vec::new());
    }
    reexpand_from_templates(config)?;
    Ok(config
        .rules
        .iter()
        .map(|r| r.name.clone())
        .filter(|n| !prev.contains(n))
        .collect())
}

/// Replay recorded re-entries whose checkpoint rule still stands, then
/// re-expand from templates. Records for invalidated rules are revoked:
/// their samples are not merged, so their instances disappear from the plan
/// until the rule re-runs and re-records.
pub fn replay_valid_reentries(
    config: &mut WorkflowConfig,
    records: &[ReentryRecord],
    valid_rules: &HashSet<String>,
) -> Result<Vec<ReentryRecord>> {
    let mut replayed = Vec::new();
    for rec in records {
        if valid_rules.contains(&rec.rule) {
            let group_name = resolve_group_name(config, rec.group.as_deref());
            merge_samples(config, &group_name, &rec.samples);
            replayed.push(rec.clone());
        }
    }
    reexpand_from_templates(config)?;
    Ok(replayed)
}

/// The target group for a re-entry: the manifest's explicit group, else the
/// workflow's first (primary) sample group, else "auto-discovered".
fn resolve_group_name(config: &WorkflowConfig, group: Option<&str>) -> String {
    group
        .map(str::to_string)
        .or_else(|| config.sample_groups.first().map(|g| g.name.clone()))
        .unwrap_or_else(|| "auto-discovered".to_string())
}

/// Re-expand from the preserved rule templates. The expansion is
/// deterministic: existing instances regenerate with identical names, so
/// only genuinely new combos produce new instances.
fn reexpand_from_templates(config: &mut WorkflowConfig) -> Result<()> {
    if config.rule_templates.is_empty() {
        return Err(OxoFlowError::Config {
            message: "re-entry requires rule templates (expand_wildcards must run first)"
                .to_string(),
        });
    }
    config.rules = config.rule_templates.clone();
    config.expand_wildcards()
}

fn merge_samples(config: &mut WorkflowConfig, group_name: &str, samples: &[String]) -> Vec<String> {
    let group = match config
        .sample_groups
        .iter_mut()
        .find(|g| g.name == group_name)
    {
        Some(g) => g,
        None => {
            config.sample_groups.push(SampleGroup {
                name: group_name.to_string(),
                samples: Vec::new(),
                metadata: Default::default(),
            });
            config.sample_groups.last_mut().expect("just pushed")
        }
    };
    let mut added = Vec::new();
    for s in samples {
        if !group.samples.contains(s) {
            group.samples.push(s.clone());
            added.push(s.clone());
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;

    fn write_wf(dir: &std::path::Path, samples: &[&str]) -> std::path::PathBuf {
        let toml = format!(
            r#"
            [workflow]
            name = "reentry-test"
            [[sample_groups]]
            name = "batch"
            samples = {samples:?}
            [[rules]]
            name = "discover"
            shell = "echo discover > discover.done"
            output = ["discover.done"]
            checkpoint = true
            checkpoint_manifest = "discover.toml"
            [[rules]]
            name = "analyze"
            input = ["discover.done"]
            output = ["out/{{sample}}.txt"]
            shell = "touch out/{{sample}}.txt"
        "#,
        );
        let path = dir.join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        path
    }

    fn wf_config(dir: &std::path::Path, samples: &[&str]) -> WorkflowConfig {
        let mut config = WorkflowConfig::from_file(&write_wf(dir, samples)).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        config
    }

    #[test]
    fn apply_reentry_adds_only_new_instances() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = wf_config(dir.path(), &["S1"]);
        let before: Vec<String> = config.rules.iter().map(|r| r.name.clone()).collect();
        let new = apply_reentry(&mut config, None, &["S2".into(), "S1".into()]).unwrap();
        assert_eq!(new, vec!["analyze_batch_S2".to_string()]);
        assert!(
            before
                .iter()
                .all(|n| config.rules.iter().any(|r| &r.name == n))
        );
    }

    #[test]
    fn apply_reentry_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = wf_config(dir.path(), &["S1"]);
        let _ = apply_reentry(&mut config, None, &["S2".to_string()]).unwrap();
        let second = apply_reentry(&mut config, None, &["S2".to_string()]).unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn apply_reentry_unknown_group_creates_group() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = wf_config(dir.path(), &["S1"]);
        let new = apply_reentry(&mut config, Some("late"), &["S9".to_string()]).unwrap();
        assert_eq!(new, vec!["analyze_late_S9".to_string()]);
    }

    #[test]
    fn replay_reentries_only_keeps_valid_records() {
        let dir = tempfile::tempdir().unwrap();
        let records = vec![ReentryRecord {
            round: 1,
            rule: "discover".to_string(),
            group: Some("batch".to_string()),
            samples: vec!["S2".to_string()],
        }];

        // Revoked: checkpoint rule invalid → no S2 instance.
        let mut config = wf_config(dir.path(), &["S1"]);
        let replayed = replay_valid_reentries(&mut config, &records, &HashSet::new()).unwrap();
        assert!(replayed.is_empty());
        assert!(!config.rules.iter().any(|r| r.name == "analyze_batch_S2"));

        // Valid: checkpoint rule stands → S2 instance present.
        let mut config = wf_config(dir.path(), &["S1"]);
        let valid: HashSet<String> = ["discover".to_string()].into_iter().collect();
        let replayed = replay_valid_reentries(&mut config, &records, &valid).unwrap();
        assert_eq!(replayed, records);
        assert!(config.rules.iter().any(|r| r.name == "analyze_batch_S2"));
    }

    #[test]
    fn parse_manifest_shapes() {
        assert_eq!(
            parse_manifest("[reentry]\nsample = [\"S4\"]\n").unwrap(),
            (None, vec!["S4".to_string()])
        );
        assert_eq!(
            parse_manifest("[reentry]\ngroup = \"g\"\nsample = [\"S4\",\"S5\"]\n").unwrap(),
            (Some("g".into()), vec!["S4".into(), "S5".into()])
        );
        assert_eq!(
            parse_manifest("[reentry]\nsample = []\n").unwrap(),
            (None, Vec::<String>::new())
        );
        assert!(parse_manifest("no reentry table").is_err());
        assert!(parse_manifest("[other]\nx = 1\n").is_err());
    }
}
