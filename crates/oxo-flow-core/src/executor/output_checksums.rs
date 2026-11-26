//! Output checksum/manifest collection shared by both execution paths
//! (issues #1093, #1426).
//!
//! `--provenance` and per-rule `checksum = "sha256" | "md5"` record output
//! digests into the checkpoint's `checksums` map. The local executor and
//! the cluster settle hook must record the SAME entries for the same
//! workflow — same expanded keys, same algorithms — or `provenance verify`
//! and the freshness gate would read different shapes depending on where
//! the workflow ran. This module owns that one collection routine; callers
//! only persist what it returns (inside whatever checkpoint lock they
//! already hold).
//!
//! [`collect_output_manifest`] is the always-on companion (issue #1426):
//! every successful rule records its declared outputs with size and a
//! capped `sha256`, so `resume` can verify content without `--provenance`.

use crate::executor::checkpoint::{
    OutputChecksumAlgorithm, OutputManifestEntry, compute_file_checksum, expand_config_in_path,
    hash_file_with_algorithm,
};
use crate::rule::Rule;
use std::collections::HashMap;
use std::path::Path;

/// Hash every declared output of `rule` that resolves to an existing file.
///
/// `wildcard_scope` carries the run's `{config.*}` values plus the
/// instance's fan-out bindings (see [`crate::config::WorkflowConfig::instance_bindings`]),
/// exactly the merge both execution paths already perform for path
/// handling. Returns `(expanded_path, checksum)` pairs, in declaration
/// order; recorded values carry their algorithm prefix (`"sha256:<hex>"` /
/// `"md5:<hex>"`).
///
/// Two shapes are skipped by design, matching the pre-existing local
/// contract:
/// - paths that still contain `{` after expansion are per-instance
///   templates no executor can resolve here — a template never names a
///   file that exists;
/// - paths with no file on disk record nothing (an unreadable output is
///   the diagnostic's job, not provenance's).
pub fn collect_output_checksums(
    rule: &Rule,
    workdir: &Path,
    wildcard_scope: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let algorithm = rule
        .checksum
        .as_deref()
        .and_then(OutputChecksumAlgorithm::from_rule_spec);
    let mut entries = Vec::new();
    for output in &rule.output {
        let expanded = expand_config_in_path(output, wildcard_scope);
        if expanded.contains('{') {
            continue; // per-instance wildcard: resolved only at fan-out
        }
        let output_path = workdir.join(&expanded);
        if !output_path.exists() {
            continue;
        }
        let checksum_result = if let Some(alg) = algorithm {
            hash_file_with_algorithm(&output_path, alg)
        } else {
            compute_file_checksum(&output_path)
        };
        if let Ok(checksum) = checksum_result {
            entries.push((expanded, checksum));
        }
    }
    entries
}

/// Snapshot a rule's declared outputs for the checkpoint's
/// `output_manifests` map (issue #1426) — recorded on every successful
/// rule, independent of `--provenance`.
///
/// Same expansion as [`collect_output_checksums`]: `wildcard_scope` carries
/// the run's `{config.*}` values plus the instance's fan-out bindings, and
/// paths that still contain `{` after expansion are skipped (a per-instance
/// template never names a file that exists here). Missing/unreadable
/// outputs record no entry — strict resume then reports the declared output
/// as unverified and re-runs the rule, which is the safe verdict.
///
/// A regular file at or under `hash_max_bytes` records its
/// `sha256:<hex>`; larger or non-regular outputs record size only, and
/// strict resume treats such an entry as unverified.
pub fn collect_output_manifest(
    rule: &Rule,
    workdir: &Path,
    wildcard_scope: &HashMap<String, String>,
    hash_max_bytes: u64,
) -> Vec<OutputManifestEntry> {
    let mut entries = Vec::new();
    for output in &rule.output {
        let expanded = expand_config_in_path(output, wildcard_scope);
        if expanded.contains('{') {
            continue; // per-instance wildcard: resolved only at fan-out
        }
        let output_path = workdir.join(&expanded);
        let Ok(md) = std::fs::metadata(&output_path) else {
            continue; // missing/unreadable: no verifiable entry to record
        };
        let sha256 = if md.is_file() && md.len() <= hash_max_bytes {
            compute_file_checksum(&output_path).ok()
        } else {
            None
        };
        entries.push(OutputManifestEntry {
            path: expanded,
            size: md.len(),
            sha256,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::{collect_output_checksums, collect_output_manifest};
    use crate::config::WorkflowConfig;
    use std::collections::HashMap;

    fn rule_from(toml: &str, name: &str) -> crate::rule::Rule {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wf.oxoflow");
        std::fs::write(&path, toml).unwrap();
        let config = WorkflowConfig::from_file(&path).unwrap();
        config.get_rule(name).expect("rule must exist").clone()
    }

    #[test]
    fn hashes_existing_literal_outputs_with_sha256_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "hello").unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[[rules]]\nname = \"a\"\n\
             output = [\"out.txt\"]\nshell = \"true\"\n",
            "a",
        );
        let entries = collect_output_checksums(&rule, dir.path(), &HashMap::new());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "out.txt");
        assert!(entries[0].1.starts_with("sha256:"));
    }

    #[test]
    fn uses_the_rules_declared_algorithm() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "hello").unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[[rules]]\nname = \"a\"\n\
             output = [\"out.txt\"]\nchecksum = \"md5\"\nshell = \"true\"\n",
            "a",
        );
        let entries = collect_output_checksums(&rule, dir.path(), &HashMap::new());
        assert_eq!(entries.len(), 1);
        assert!(entries[0].1.starts_with("md5:"));
    }

    #[test]
    fn skips_missing_outputs_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[[rules]]\nname = \"a\"\n\
             output = [\"absent.txt\"]\nshell = \"true\"\n",
            "a",
        );
        assert!(collect_output_checksums(&rule, dir.path(), &HashMap::new()).is_empty());
    }

    #[test]
    fn skips_paths_that_still_carry_a_template() {
        // `{sample}` survives expansion when the scope has no binding for
        // it — exactly the runtime-discovered case; a template must never
        // become a recorded key.
        let dir = tempfile::tempdir().unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[[rules]]\nname = \"a\"\n\
             output = [\"aln/{sample}.bam\"]\nshell = \"true\"\n",
            "a",
        );
        assert!(collect_output_checksums(&rule, dir.path(), &HashMap::new()).is_empty());

        // With the binding in scope the same output resolves and hashes.
        std::fs::create_dir_all(dir.path().join("aln")).unwrap();
        std::fs::write(dir.path().join("aln/S1.bam"), "bam").unwrap();
        let mut scope = HashMap::new();
        scope.insert("sample".to_string(), "S1".to_string());
        let entries = collect_output_checksums(&rule, dir.path(), &scope);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "aln/S1.bam");
    }

    #[test]
    fn expands_config_values_in_output_paths() {
        // `{config.outdir}`-style references expand before hashing, so the
        // recorded key names the file that exists (issue #276's lesson).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("results")).unwrap();
        std::fs::write(dir.path().join("results/out.txt"), "hello").unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[config]\noutdir = \"results\"\n\
             [[rules]]\nname = \"a\"\noutput = [\"{config.outdir}/out.txt\"]\n\
             shell = \"true\"\n",
            "a",
        );
        // `wildcard_values` keys carry the `config.` prefix (the run loop
        // builds the map that way: `config.<key>` → value).
        let mut scope = HashMap::new();
        scope.insert("config.outdir".to_string(), "results".to_string());
        let entries = collect_output_checksums(&rule, dir.path(), &scope);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "results/out.txt");
    }

    // ── Output manifest (issue #1426) ────────────────────────────────────

    #[test]
    fn manifest_hashes_regular_outputs_and_records_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "hello").unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[[rules]]\nname = \"a\"\n\
             output = [\"out.txt\"]\nshell = \"true\"\n",
            "a",
        );
        let entries = collect_output_manifest(&rule, dir.path(), &HashMap::new(), 64);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "out.txt");
        assert_eq!(entries[0].size, 5);
        assert!(
            entries[0]
                .sha256
                .as_deref()
                .is_some_and(|h| h.starts_with("sha256:")),
            "a small regular file must record a sha256: {entries:?}"
        );
    }

    #[test]
    fn manifest_records_size_only_above_the_cap() {
        // Above the cap the entry is deliberately UNVERIFIED (size only) —
        // strict resume then re-runs unless --trust-mtime.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.bin"), [0u8; 16]).unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[[rules]]\nname = \"a\"\n\
             output = [\"big.bin\"]\nshell = \"true\"\n",
            "a",
        );
        let entries = collect_output_manifest(&rule, dir.path(), &HashMap::new(), 8);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size, 16);
        assert_eq!(
            entries[0].sha256, None,
            "over-cap output must record size only"
        );

        // A cap at/above the size records the hash (boundary: <=).
        let entries = collect_output_manifest(&rule, dir.path(), &HashMap::new(), 16);
        assert!(entries[0].sha256.is_some());
    }

    #[test]
    fn manifest_skips_unresolved_templates_and_missing_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[[rules]]\nname = \"a\"\n\
             output = [\"aln/{sample}.bam\", \"absent.txt\"]\nshell = \"true\"\n",
            "a",
        );
        let entries = collect_output_manifest(&rule, dir.path(), &HashMap::new(), 64);
        assert!(
            entries.iter().all(|e| !e.path.contains('{')),
            "an unresolved template must never become a recorded key: {entries:?}"
        );

        // With the binding in scope the same output resolves and hashes.
        std::fs::create_dir_all(dir.path().join("aln")).unwrap();
        std::fs::write(dir.path().join("aln/S1.bam"), "bam").unwrap();
        let mut scope = HashMap::new();
        scope.insert("sample".to_string(), "S1".to_string());
        let entries = collect_output_manifest(&rule, dir.path(), &scope, 64);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "aln/S1.bam");
    }

    #[test]
    fn manifest_uses_the_shared_expansion_before_recording() {
        // Same expansion contract as collect_output_checksums: config values
        // (and fan-out bindings merged by the caller) expand before the key
        // is derived.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("results")).unwrap();
        std::fs::write(dir.path().join("results/out.txt"), "hello").unwrap();
        let rule = rule_from(
            "[workflow]\nname = \"t\"\n[config]\noutdir = \"results\"\n\
             [[rules]]\nname = \"a\"\noutput = [\"{config.outdir}/out.txt\"]\n\
             shell = \"true\"\n",
            "a",
        );
        let mut scope = HashMap::new();
        scope.insert("config.outdir".to_string(), "results".to_string());
        let entries = collect_output_manifest(&rule, dir.path(), &scope, 64);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "results/out.txt");
    }
}
