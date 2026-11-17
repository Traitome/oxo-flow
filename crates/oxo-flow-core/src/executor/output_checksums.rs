//! Output checksum collection shared by both execution paths (issue #1093).
//!
//! `--provenance` and per-rule `checksum = "sha256" | "md5"` record output
//! digests into the checkpoint's `checksums` map. The local executor and
//! the cluster settle hook must record the SAME entries for the same
//! workflow — same expanded keys, same algorithms — or `provenance verify`
//! and the freshness gate would read different shapes depending on where
//! the workflow ran. This module owns that one collection routine; callers
//! only persist what it returns (inside whatever checkpoint lock they
//! already hold).

use crate::executor::checkpoint::{
    OutputChecksumAlgorithm, compute_file_checksum, expand_config_in_path, hash_file_with_algorithm,
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

#[cfg(test)]
mod tests {
    use super::collect_output_checksums;
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
}
