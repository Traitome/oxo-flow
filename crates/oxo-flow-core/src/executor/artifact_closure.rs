//! Artifact closure verification for pulled remote runs (issue #1369).
//!
//! A remote run's exit code 0 is not a contract on its artifacts. The pulled
//! workdir is checked against everything the engine recorded:
//!
//! - **missing**: a declared output of a completed rule is absent
//!   (existence for wildcard / `output_pattern` rules is delegated to the
//!   single oracle [`rule_outputs_exist_with_patterns`], exactly like the CLI
//!   run loop and reclaim gate);
//! - **modified**: a recorded `--provenance`/`checksum =` digest no longer
//!   matches the pulled bytes;
//! - **truncated**: the file exists but is empty where the recorded digest
//!   proves non-empty content (a transfer that collapsed the file);
//! - **extra**: an undeclared file appeared inside a declared output
//!   directory (declared-roots scoped — never a whole-tree walk);
//! - **cleaned**: a `temporary = true` output was removed by design, so an
//!   absent cleaned artifact is reported as `cleaned`, never `missing`.
//!
//! Every finding names the rule, path, status and the recorded/actual values
//! so the web run row (`artifacts_incomplete`) carries a structured, JSON
//! report instead of one opaque sentence. Maps are serialized key-ordered
//! (`BTreeMap`) per the repository's persisted-artifact invariant.

use crate::config::WorkflowConfig;
use crate::executor::checkpoint::{
    CheckpointState, OutputChecksumAlgorithm, expand_config_in_path, hash_file_with_algorithm,
    rule_outputs_exist_with_patterns, validate_outputs,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Status of one artifact relative to what the engine recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    /// Present and (when a checksum was recorded) byte-identical.
    Present,
    /// Declared by a completed rule but absent from the pulled tree.
    Missing,
    /// Present but empty where the recorded digest proves non-empty content.
    Truncated,
    /// Present but the recorded checksum does not match.
    Modified,
    /// Undeclared file inside a declared output directory.
    Extra,
    /// `temporary = true` output intentionally removed by the engine.
    Cleaned,
}

impl ArtifactStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Truncated => "truncated",
            Self::Modified => "modified",
            Self::Extra => "extra",
            Self::Cleaned => "cleaned",
        }
    }

    /// Whether this status makes the closure FAIL. `Present`, `Cleaned` and
    /// `Extra` are non-failing outcomes: `Cleaned` is documented behavior,
    /// and an undeclared sidecar inside a declared directory (a `.bai` index,
    /// a tool's log) is reported for the audit trail but is not evidence that
    /// a declared output is incomplete or changed.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Missing | Self::Truncated | Self::Modified)
    }
}

/// One artifact finding, key-ordered on serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactFinding {
    pub rule: String,
    pub path: String,
    pub status: ArtifactStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
}

/// Structured artifact-closure report. `findings` is sorted (path, status)
/// so two verifications of the same tree serialize byte-identically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactClosureReport {
    pub ok: bool,
    /// Whether recorded checksums were available to verify (provenance runs).
    pub checksums_verified: bool,
    pub counts: BTreeMap<String, usize>,
    pub findings: Vec<ArtifactFinding>,
}

impl ArtifactClosureReport {
    /// Human/`artifacts_incomplete` one-line reason naming the first failures.
    pub fn reason(&self) -> String {
        let failures: Vec<&ArtifactFinding> = self
            .findings
            .iter()
            .filter(|f| f.status.is_failure())
            .collect();
        if failures.is_empty() {
            return "artifact closure verified".to_string();
        }
        let names: Vec<String> = failures
            .iter()
            .take(5)
            .map(|f| format!("{} {}: {}", f.rule, f.status.as_str(), f.path))
            .collect();
        let more = failures.len().saturating_sub(names.len());
        if more == 0 {
            format!("artifact closure failed: {}", names.join("; "))
        } else {
            format!(
                "artifact closure failed: {}; (+{more} more)",
                names.join("; ")
            )
        }
    }
}

/// Verify the pulled workdir's artifact closure.
///
/// `config` must be the RESOLVED, wildcard-expanded workflow the remote run
/// executed (the web layer expands before calling), so instance names line
/// up with the checkpoint's completed-rule names.
pub fn verify_artifact_closure(
    config: &WorkflowConfig,
    checkpoint: &CheckpointState,
    workdir: &Path,
) -> ArtifactClosureReport {
    let config_values = config.config_placeholder_values();
    let mut findings: Vec<ArtifactFinding> = Vec::new();

    // Completed rules → declared outputs; existence via the canonical oracle
    // (fan-out / output_pattern aware), then concrete paths for findings.
    let mut declared_concrete: BTreeSet<PathBuf> = BTreeSet::new();
    let mut output_roots: BTreeSet<PathBuf> = BTreeSet::new();
    for rule in &config.rules {
        if !checkpoint.is_completed(&rule.name) {
            continue;
        }
        if !rule_outputs_exist_with_patterns(
            rule,
            workdir,
            &config_values,
            &checkpoint.output_pattern_domains,
            &config.expansion_templates,
        ) {
            let missing = validate_outputs(rule, workdir, &config_values);
            if missing.is_empty() {
                for declared in &rule.output {
                    let expanded = expand_config_in_path(declared, &config_values);
                    if !expanded.contains('{') {
                        findings.push(ArtifactFinding {
                            rule: rule.name.clone(),
                            path: expanded,
                            status: ArtifactStatus::Missing,
                            expected: None,
                            actual: None,
                        });
                    }
                }
            } else {
                for path in missing {
                    findings.push(ArtifactFinding {
                        rule: rule.name.clone(),
                        path,
                        status: ArtifactStatus::Missing,
                        expected: None,
                        actual: None,
                    });
                }
            }
        }
        for declared in &rule.output {
            let expanded = expand_config_in_path(declared, &config_values);
            if expanded.contains('{') {
                continue; // per-instance wildcard: resolved by the oracle only
            }
            let path = Path::new(&expanded);
            declared_concrete.insert(path.to_path_buf());
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                output_roots.insert(parent.to_path_buf());
            }
        }
    }

    // Artifact → owning rule, derived from the same declared set (an
    // artifact may be declared by several rules; the first in declaration
    // order wins, deterministically).
    let mut owner_by_path: BTreeMap<String, String> = BTreeMap::new();
    for rule in &config.rules {
        if !checkpoint.is_completed(&rule.name) {
            continue;
        }
        for declared in &rule.output {
            let expanded = expand_config_in_path(declared, &config_values);
            if expanded.contains('{') {
                continue;
            }
            owner_by_path
                .entry(expanded.replace('\\', "/"))
                .or_insert_with(|| rule.name.clone());
        }
    }

    // Recorded checksums (`--provenance` or rule `checksum =`): re-hash the
    // pulled bytes and classify missing / truncated / modified.
    let checksums_verified = !checkpoint.checksums.is_empty();
    for (artifact, recorded) in &checkpoint.checksums {
        let path = workdir.join(artifact);
        if !path.is_file() {
            findings.push(ArtifactFinding {
                rule: owner_by_path.get(artifact).cloned().unwrap_or_default(),
                path: artifact.clone(),
                status: ArtifactStatus::Missing,
                expected: Some(recorded.clone()),
                actual: None,
            });
            continue;
        }
        let Some(algorithm) = OutputChecksumAlgorithm::from_recorded_prefix(recorded) else {
            continue; // uninterpretable record: never fail on it
        };
        let actual = match hash_file_with_algorithm(&path, algorithm) {
            Ok(value) => value,
            Err(_) => {
                findings.push(ArtifactFinding {
                    rule: String::new(),
                    path: artifact.clone(),
                    status: ArtifactStatus::Missing,
                    expected: Some(recorded.clone()),
                    actual: Some("unreadable".to_string()),
                });
                continue;
            }
        };
        if &actual == recorded {
            continue;
        }
        let empty = std::fs::metadata(&path)
            .map(|m| m.len() == 0)
            .unwrap_or(false);
        findings.push(ArtifactFinding {
            rule: owner_by_path.get(artifact).cloned().unwrap_or_default(),
            path: artifact.clone(),
            status: if empty {
                ArtifactStatus::Truncated
            } else {
                ArtifactStatus::Modified
            },
            expected: Some(recorded.clone()),
            actual: Some(actual),
        });
    }

    // Cleaned temporaries are documented removals, never missing artifacts.
    for (artifact, recorded) in &checkpoint.cleaned_checksums {
        if workdir.join(artifact).exists() {
            continue; // still on disk: nothing to explain
        }
        findings.push(ArtifactFinding {
            rule: String::new(),
            path: artifact.clone(),
            status: ArtifactStatus::Cleaned,
            expected: Some(recorded.clone()),
            actual: None,
        });
    }

    // Extra artifacts: undeclared files inside declared output directories.
    // Declared-roots scoped (never a whole-tree walk); skip the workdir root
    // itself and engine bookkeeping directories.
    const MAX_EXTRA_PER_ROOT: usize = 1000;
    for root in &output_roots {
        let dir = workdir.join(root);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for (index, entry) in entries.flatten().enumerate() {
            if index >= MAX_EXTRA_PER_ROOT {
                break;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let relative = PathBuf::from(root).join(entry.file_name());
            if declared_concrete.contains(&relative)
                || checkpoint
                    .cleaned_checksums
                    .contains_key(&path_key(&relative))
                || checkpoint.checksums.contains_key(&path_key(&relative))
            {
                continue;
            }
            // Nested outputs declared deeper under this root are still
            // declared (their parent appears in output_roots separately).
            if declared_concrete.iter().any(|d| d.starts_with(&relative)) {
                continue;
            }
            findings.push(ArtifactFinding {
                rule: String::new(),
                path: path_key(&relative),
                status: ArtifactStatus::Extra,
                expected: None,
                actual: Some(format!("{} bytes", meta.len())),
            });
        }
    }

    findings.sort_by(|a, b| {
        a.status
            .cmp(&b.status)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.rule.cmp(&b.rule))
    });
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for finding in &findings {
        *counts
            .entry(finding.status.as_str().to_string())
            .or_default() += 1;
    }
    let ok = !findings.iter().any(|f| f.status.is_failure());
    ArtifactClosureReport {
        ok,
        checksums_verified,
        counts,
        findings,
    }
}

/// Normalized key for a workdir-relative path (forward slashes, no `./`).
fn path_key(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::Rule;

    fn rule(name: &str, outputs: &[&str]) -> Rule {
        Rule {
            name: name.to_string(),
            input: crate::rule::FilePatterns::List(vec!["in.txt".to_string()]),
            output: crate::rule::FilePatterns::List(
                outputs.iter().map(|o| o.to_string()).collect(),
            ),
            shell: Some("true".to_string()),
            ..Default::default()
        }
    }

    fn config(rules: Vec<Rule>) -> WorkflowConfig {
        let mut config =
            WorkflowConfig::parse("[workflow]\nname = \"artifact-closure-tests\"\n").unwrap();
        config.rules = rules;
        config
    }

    fn checkpoint(completed: &[&str], checksums: &[(&str, &str)]) -> CheckpointState {
        let mut state = CheckpointState::default();
        for name in completed {
            state.completed_rules.insert(name.to_string());
        }
        for (path, hash) in checksums {
            state.checksums.insert(path.to_string(), hash.to_string());
        }
        state
    }

    fn write(dir: &Path, name: &str, body: &[u8]) -> String {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
        hash_file_with_algorithm(&path, OutputChecksumAlgorithm::Sha256).unwrap()
    }

    #[test]
    fn intact_tree_passes_with_and_without_checksums() {
        let dir = tempfile::tempdir().unwrap();
        let hash = write(dir.path(), "out/a.txt", b"alpha");
        let cfg = config(vec![rule("make", &["out/a.txt"])]);
        let cp = checkpoint(&["make"], &[("out/a.txt", &hash)]);
        let report = verify_artifact_closure(&cfg, &cp, dir.path());
        assert!(report.ok, "{report:?}");
        assert!(report.checksums_verified);
        // No checksums recorded: existence-only still passes.
        let report = verify_artifact_closure(&cfg, &checkpoint(&["make"], &[]), dir.path());
        assert!(report.ok && !report.checksums_verified, "{report:?}");
    }

    #[test]
    fn missing_modified_and_truncated_are_distinct_failures() {
        let dir = tempfile::tempdir().unwrap();
        let hash = write(dir.path(), "out/a.txt", b"alpha");
        let cfg = config(vec![rule("make", &["out/a.txt", "out/b.txt"])]);
        let mut cp = checkpoint(&["make"], &[("out/a.txt", &hash), ("out/b.txt", &hash)]);
        cp.checksums.insert("out/c.txt".to_string(), hash.clone());

        // b.txt missing entirely; c.txt recorded but absent.
        let report = verify_artifact_closure(&cfg, &cp, dir.path());
        assert!(!report.ok);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "out/b.txt" && f.status == ArtifactStatus::Missing)
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "out/c.txt" && f.status == ArtifactStatus::Missing)
        );

        // Modified: same size-class content, different bytes.
        write(dir.path(), "out/b.txt", b"bravo");
        std::fs::write(dir.path().join("out/a.txt"), b"ALPHA").unwrap();
        let report = verify_artifact_closure(&cfg, &cp, dir.path());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "out/a.txt" && f.status == ArtifactStatus::Modified)
        );

        // Truncated: transfer collapsed the file to zero bytes.
        std::fs::write(dir.path().join("out/a.txt"), b"").unwrap();
        let report = verify_artifact_closure(&cfg, &cp, dir.path());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "out/a.txt" && f.status == ArtifactStatus::Truncated)
        );
    }

    #[test]
    fn extra_files_in_declared_output_dirs_fail_but_cleaned_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let hash = write(dir.path(), "out/a.txt", b"alpha");
        let mut cp = checkpoint(&["make"], &[("out/a.txt", &hash)]);
        cp.cleaned_checksums
            .insert("out/tmp.txt".to_string(), hash.clone());
        let cfg = config(vec![rule("make", &["out/a.txt"])]);

        // Cleaned temporary absent → documented, not a failure.
        let report = verify_artifact_closure(&cfg, &cp, dir.path());
        assert!(report.ok, "{report:?}");
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "out/tmp.txt" && f.status == ArtifactStatus::Cleaned)
        );

        // An undeclared file inside the declared output root is reported as
        // extra but does not fail the closure (sidecars such as `.bai` are
        // legitimate).
        std::fs::write(dir.path().join("out/stray.txt"), b"x").unwrap();
        let report = verify_artifact_closure(&cfg, &cp, dir.path());
        assert!(report.ok, "{report:?}");
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "out/stray.txt" && f.status == ArtifactStatus::Extra)
        );
    }

    #[test]
    fn report_serialization_is_key_ordered_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(vec![rule("make", &["out/a.txt"])]);
        let cp = checkpoint(&["make"], &[]);
        let report = verify_artifact_closure(&cfg, &cp, dir.path());
        let first = serde_json::to_string(&report).unwrap();
        let second =
            serde_json::to_string(&verify_artifact_closure(&cfg, &cp, dir.path())).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("\"ok\":false")); // a.txt missing
    }
}
