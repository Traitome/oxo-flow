//! Checkpoint re-entry (issue #78 P3): static + dynamic hybrid DAG.
//!
//! A `checkpoint = true` rule writes a TOML manifest at runtime declaring new
//! wildcard values (new samples); the engine merges them and re-expands the
//! rule templates — every round is still a static plan, so previews stay
//! deterministic and resumes reconstruct the same plan.

use crate::config::{ExperimentControlPair, SampleGroup, WorkflowConfig};
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
    /// New experiment-control pairs appended (dedup by `pair_id`) to the
    /// workflow's pair list (issue #80 item 3).
    #[serde(default)]
    pub pairs: Vec<ExperimentControlPair>,
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
    #[serde(default)]
    pairs: Vec<PairEntry>,
}

/// Manifest mirror of [`ExperimentControlPair`] with the same aliases.
#[derive(Debug, Deserialize)]
struct PairEntry {
    pair_id: String,
    #[serde(alias = "tumor")]
    experiment: String,
    #[serde(default, alias = "normal")]
    control: Option<String>,
    #[serde(default, alias = "tumor_type")]
    experiment_type: Option<String>,
    #[serde(default)]
    metadata: std::collections::HashMap<String, String>,
}

/// Parse a checkpoint re-entry manifest: `(group, samples, pairs)`.
pub fn parse_manifest(
    content: &str,
) -> Result<(Option<String>, Vec<String>, Vec<ExperimentControlPair>)> {
    let m: Manifest = toml::from_str(content).map_err(|e| OxoFlowError::Config {
        message: format!("invalid re-entry manifest: {e}"),
    })?;
    let pairs = m
        .reentry
        .pairs
        .into_iter()
        .map(|p| {
            if p.pair_id.trim().is_empty() {
                return Err(OxoFlowError::Config {
                    message: "re-entry pair entry missing 'pair_id'".to_string(),
                });
            }
            if p.experiment.trim().is_empty() {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "re-entry pair '{}' missing 'experiment' (alias: 'tumor')",
                        p.pair_id
                    ),
                });
            }
            Ok(ExperimentControlPair {
                pair_id: p.pair_id,
                experiment: p.experiment,
                control: p.control,
                experiment_type: p.experiment_type,
                metadata: p.metadata,
                // Re-entry pairs come from a saved manifest — the gate was
                // already applied when the manifest was written.
                when: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((m.reentry.group, m.reentry.sample, pairs))
}

/// Merge new samples into the target group and new pairs into the pair
/// list (dedup), then re-expand from the rule templates — one expansion
/// covers both kinds. Returns the names of newly created instances
/// (already in `config.rules`).
pub fn apply_reentry(
    config: &mut WorkflowConfig,
    group: Option<&str>,
    samples: &[String],
    pairs: &[ExperimentControlPair],
) -> Result<Vec<String>> {
    let prev: HashSet<String> = config.rules.iter().map(|r| r.name.clone()).collect();
    let group_name = resolve_group_name(config, group);
    let added_samples = merge_samples(config, &group_name, samples);
    let added_pairs = merge_pairs(config, pairs)?;
    if added_samples.is_empty() && added_pairs.is_empty() {
        return Ok(Vec::new());
    }
    reexpand_from_templates(config).map_err(|e| match e {
        // expand_wildcards already rejects duplicate instance names; add the
        // re-entry context so a colliding pair_id points at its discoverer.
        OxoFlowError::DuplicateRule { name } => OxoFlowError::DuplicateRule {
            name: format!(
                "{name} (E016: a re-entry pair_id collides with existing instance names)"
            ),
        },
        other => other,
    })?;
    Ok(config
        .rules
        .iter()
        .map(|r| r.name.clone())
        .filter(|n| !prev.contains(n))
        .collect())
}

/// Replay recorded re-entries whose checkpoint rule still stands, then
/// re-expand from templates. Records for invalidated rules are revoked:
/// their samples and pairs are not merged, so their instances disappear
/// from the plan until the rule re-runs and re-records.
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
            merge_pairs(config, &rec.pairs)?;
            replayed.push(rec.clone());
        }
    }
    reexpand_from_templates(config)?;
    Ok(replayed)
}

/// Append new pairs (dedup by `pair_id`). A known `pair_id` with different
/// content is a conflict — silently superseding it would corrupt already-run
/// pair outputs (E015).
fn merge_pairs(
    config: &mut WorkflowConfig,
    pairs: &[ExperimentControlPair],
) -> Result<Vec<String>> {
    let mut added = Vec::new();
    for pair in pairs {
        match config.pairs.iter().find(|p| p.pair_id == pair.pair_id) {
            None => {
                config.pairs.push(pair.clone());
                added.push(pair.pair_id.clone());
            }
            Some(existing) if existing == pair => {}
            Some(_) => {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "E015: re-entry pair '{}' conflicts with an existing pair of the same \
                         pair_id (same id, different content)",
                        pair.pair_id
                    ),
                });
            }
        }
    }
    Ok(added)
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

    fn write_pairs_wf(dir: &std::path::Path, pairs: &[(&str, &str)]) -> std::path::PathBuf {
        let pair_lines = pairs
            .iter()
            .map(|(t, n)| {
                format!(
                    "pair_id = \"CASE_{t}\"\nexperiment = \"{t}\"\ncontrol = \"{n}\"",
                    t = t,
                    n = n
                )
            })
            .collect::<Vec<_>>()
            .join("\n[[pairs]]\n");
        let toml = format!(
            r#"
            [workflow]
            name = "reentry-pairs-test"
            [[pairs]]
            {pair_lines}
            [[rules]]
            name = "discover"
            shell = "echo discover > discover.done"
            output = ["discover.done"]
            checkpoint = true
            checkpoint_manifest = "discover.toml"
            [[rules]]
            name = "call"
            input = ["discover.done"]
            output = ["out/{{pair_id}}.txt"]
            shell = "echo {{pair_id}} > out/{{pair_id}}.txt"
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

    fn pairs_config(dir: &std::path::Path) -> WorkflowConfig {
        let mut config = WorkflowConfig::from_file(&write_pairs_wf(dir, &[("T1", "N1")])).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        config
    }

    fn pair(pair_id: &str, experiment: &str, control: &str) -> ExperimentControlPair {
        ExperimentControlPair {
            pair_id: pair_id.into(),
            experiment: experiment.into(),
            control: Some(control.into()),
            experiment_type: None,
            metadata: Default::default(),
            when: None,
        }
    }

    #[test]
    fn apply_reentry_adds_only_new_instances() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = wf_config(dir.path(), &["S1"]);
        let before: Vec<String> = config.rules.iter().map(|r| r.name.clone()).collect();
        let new = apply_reentry(&mut config, None, &["S2".into(), "S1".into()], &[]).unwrap();
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
        let _ = apply_reentry(&mut config, None, &["S2".to_string()], &[]).unwrap();
        let second = apply_reentry(&mut config, None, &["S2".to_string()], &[]).unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn apply_reentry_unknown_group_creates_group() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = wf_config(dir.path(), &["S1"]);
        let new = apply_reentry(&mut config, Some("late"), &["S9".to_string()], &[]).unwrap();
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
            pairs: vec![],
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
            (None, vec!["S4".to_string()], vec![])
        );
        assert_eq!(
            parse_manifest("[reentry]\ngroup = \"g\"\nsample = [\"S4\",\"S5\"]\n").unwrap(),
            (Some("g".into()), vec!["S4".into(), "S5".into()], vec![])
        );
        assert_eq!(
            parse_manifest("[reentry]\nsample = []\n").unwrap(),
            (None, Vec::<String>::new(), vec![])
        );
        assert!(parse_manifest("no reentry table").is_err());
        assert!(parse_manifest("[other]\nx = 1\n").is_err());
    }

    // ── pairs (issue #80 item 3) ────────────────────────────────────────

    #[test]
    fn parse_manifest_reads_pairs_entries() {
        let (group, samples, pairs) = parse_manifest(
            r#"[reentry]
sample = ["S4"]
pairs = [
  { pair_id = "CASE_007", tumor = "T7", normal = "N7", tumor_type = "tumor", metadata = { assay = "wes" } },
]"#,
        )
        .unwrap();
        assert_eq!(group, None);
        assert_eq!(samples, vec!["S4".to_string()]);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].pair_id, "CASE_007");
        assert_eq!(pairs[0].experiment, "T7");
        assert_eq!(pairs[0].control.as_deref(), Some("N7"));
        assert_eq!(pairs[0].experiment_type.as_deref(), Some("tumor"));
        assert_eq!(
            pairs[0].metadata.get("assay").map(String::as_str),
            Some("wes")
        );
    }

    #[test]
    fn parse_manifest_pair_validation_errors() {
        let err = parse_manifest("[reentry]\npairs = [{ experiment = \"T1\" }]\n").unwrap_err();
        assert!(err.to_string().contains("pair_id"), "{err}");
        let err = parse_manifest("[reentry]\npairs = [{ pair_id = \"P\" }]\n").unwrap_err();
        assert!(err.to_string().contains("experiment"), "{err}");
    }

    #[test]
    fn apply_reentry_pairs_adds_only_new_instances() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = pairs_config(dir.path());
        assert!(config.rules.iter().any(|r| r.name == "call_CASE_T1"));

        let new = apply_reentry(&mut config, None, &[], &[pair("CASE_T2", "T2", "N2")]).unwrap();
        assert_eq!(new, vec!["call_CASE_T2".to_string()]);
        assert!(config.rules.iter().any(|r| r.name == "call_CASE_T1"));
    }

    #[test]
    fn apply_reentry_pairs_same_id_same_content_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = pairs_config(dir.path());
        let new = apply_reentry(&mut config, None, &[], &[pair("CASE_T1", "T1", "N1")]).unwrap();
        assert!(new.is_empty());
    }

    #[test]
    fn apply_reentry_pairs_conflicting_pair_id_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = pairs_config(dir.path());
        let err =
            apply_reentry(&mut config, None, &[], &[pair("CASE_T1", "T1", "OTHER")]).unwrap_err();
        assert!(err.to_string().contains("E015"), "{err}");
    }

    #[test]
    fn replay_reentries_pairs_valid_and_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let records = vec![ReentryRecord {
            round: 1,
            rule: "discover".to_string(),
            group: None,
            samples: vec![],
            pairs: vec![pair("CASE_T2", "T2", "N2")],
        }];

        // Revoked → no new pair instance.
        let mut config = pairs_config(dir.path());
        let replayed = replay_valid_reentries(&mut config, &records, &HashSet::new()).unwrap();
        assert!(replayed.is_empty());
        assert!(!config.rules.iter().any(|r| r.name == "call_CASE_T2"));

        // Valid → pair instance present, existing ones untouched.
        let mut config = pairs_config(dir.path());
        let valid: HashSet<String> = ["discover".to_string()].into_iter().collect();
        let replayed = replay_valid_reentries(&mut config, &records, &valid).unwrap();
        assert_eq!(replayed, records);
        assert!(config.rules.iter().any(|r| r.name == "call_CASE_T2"));
        assert!(config.rules.iter().any(|r| r.name == "call_CASE_T1"));
    }

    #[test]
    fn apply_reentry_mixed_samples_and_pairs_in_one_round() {
        let dir = tempfile::tempdir().unwrap();
        // A workflow with BOTH a sample group and pairs: one re-entry adds
        // one of each in the same round.
        let toml = r#"
            [workflow]
            name = "mixed"
            [[sample_groups]]
            name = "batch"
            samples = ["S1"]
            [[pairs]]
            pair_id = "CASE_T1"
            experiment = "T1"
            control = "N1"
            [[rules]]
            name = "discover"
            shell = "echo discover > discover.done"
            output = ["discover.done"]
            checkpoint = true
            checkpoint_manifest = "discover.toml"
            [[rules]]
            name = "a"
            input = ["discover.done"]
            output = ["out/{{sample}}.txt"]
            shell = "touch out/{{sample}}.txt"
            [[rules]]
            name = "b"
            input = ["discover.done"]
            output = ["outb/{{pair_id}}.txt"]
            shell = "touch outb/{{pair_id}}.txt"
        "#;
        let path = dir.path().join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let mut config = WorkflowConfig::from_file(&path).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();

        let new = apply_reentry(
            &mut config,
            None,
            &["S2".to_string()],
            &[pair("CASE_T2", "T2", "N2")],
        )
        .unwrap();
        assert_eq!(new, vec!["a_batch_S2".to_string(), "b_CASE_T2".to_string()]);
    }

    #[test]
    fn reentry_record_pairs_roundtrip_and_legacy_load() {
        let rec = ReentryRecord {
            round: 2,
            rule: "discover".into(),
            group: None,
            samples: vec![],
            pairs: vec![pair("CASE_T2", "T2", "N2")],
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: ReentryRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);

        // Legacy checkpoint JSON without the `pairs` key still loads.
        let legacy = r#"{"round":1,"rule":"discover","group":null,"samples":["S2"]}"#;
        let back: ReentryRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.pairs, vec![]);
        assert_eq!(back.samples, vec!["S2".to_string()]);
    }
}
