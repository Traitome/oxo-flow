//! Content-addressed output reuse for rules that declare `cache_key`
//! (issue #194 §2.3, previously W026 "not yet consulted").
//!
//! A rule instance's cache identity is the hash of everything that
//! determines its output: the declared `cache_key`, the expanded output
//! patterns, the fully rendered command (environment wrapper included),
//! and the content identity of every resolved input (sha256 for local
//! files, etag for remote objects). An entry with a matching identity can
//! only have been produced from the same content, so restoring its outputs
//! is safe across workdirs and across runs — invalidation is structural,
//! not time-based.
//!
//! Safety rails:
//! - Local inputs over [`super::checkpoint::MANIFEST_HASH_MAX_BYTES`] have
//!   no content hash, and remote inputs without an etag have no remote
//!   identity — such rule instances simply do not participate (no reuse is
//!   better than unprovable reuse).
//! - Rules with remote outputs never participate: the cached local copy
//!   cannot stand in for the cloud object.
//! - Entries are written atomically (populate into a temp sibling, rename,
//!   write `entry.json` last as the completeness marker), so a restore
//!   sees a complete entry or none — parallel instances of the same rule
//!   with identical content cannot corrupt each other.

use crate::error::Result;
use crate::executor::checkpoint::InputManifestEntry;
use crate::rule::Rule;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Version tag mixed into every cache identity — bump to invalidate all
/// entries when the key material or restore semantics change.
const CACHE_FORMAT_VERSION: &str = "oxo-flow content cache v1";

/// Root of all content-cache entries under a workdir.
fn cache_root(workdir: &Path) -> PathBuf {
    workdir.join(".oxo-flow/content-cache")
}

/// The entry directory for a rule instance's cache identity.
pub fn cache_entry_dir(workdir: &Path, rule_name: &str, key: &str) -> PathBuf {
    cache_root(workdir)
        .join(super::process::sanitize_dir_component(rule_name))
        .join(key)
}

/// The content-addressed identity of a rule instance — everything that
/// determines its outputs: the declared `cache_key`, the expanded output
/// patterns, the fully rendered command, and the content identity of every
/// resolved input. Returns `None` when the rule declares no `cache_key`
/// or when any input cannot be content-addressed (a local file over the
/// hash cap, or a remote object without an etag): reuse would not be
/// provably safe, so the instance does not participate.
pub fn cache_entry_key(
    rule: &Rule,
    wildcard_values: &HashMap<String, String>,
    rendered_command: &str,
    manifest: &[InputManifestEntry],
) -> Option<String> {
    let cache_key = rule.cache_key.as_deref()?;
    let mut hasher = Sha256::new();
    hasher.update(CACHE_FORMAT_VERSION.as_bytes());
    hasher.update(b"\n");
    hasher.update(cache_key.as_bytes());
    hasher.update(b"\n");
    for output in &rule.output {
        let expanded = super::checkpoint::expand_config_in_path(output, wildcard_values);
        hasher.update(expanded.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(rendered_command.as_bytes());
    hasher.update(b"\n");
    for entry in manifest {
        hasher.update(entry.path.as_bytes());
        hasher.update(b"|");
        match (&entry.remote, &entry.hash) {
            // Local content identity: the manifest's sha256 for files
            // under the hash cap.
            (None, Some(hash)) => hasher.update(hash.as_bytes()),
            // Remote identity: the object's etag stands for its content.
            (Some(remote), _) => {
                let etag = remote.etag.as_ref()?;
                hasher.update(etag.as_bytes());
            }
            // Local file over the hash cap, or a legacy hash-less entry —
            // content is unknown, refuse participation.
            (None, None) => return None,
        }
        hasher.update(b"\n");
    }
    Some(hex::encode(hasher.finalize()))
}

/// Restore a cache entry's outputs into the workdir. Returns `Ok(true)`
/// when the entry existed and its outputs were restored, `Ok(false)` when
/// no complete entry is present (no `entry.json` marker).
///
/// Output files are copied with the same atomic pattern as cross-filesystem
/// moves (`copy_tree_atomic`), so a crash mid-restore leaves no truncated
/// output. Unresolved wildcard outputs are skipped, mirroring scratch
/// collection.
pub async fn restore_outputs(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    entry_dir: &Path,
) -> Result<bool> {
    if !entry_dir.join("entry.json").exists() {
        return Ok(false);
    }
    for output in &rule.output {
        let expanded = super::checkpoint::expand_config_in_path(output, wildcard_values);
        if crate::wildcard::has_wildcards(&expanded) {
            continue;
        }
        let src = entry_dir.join(&expanded);
        if !src.exists() {
            continue;
        }
        let dest = workdir.join(&expanded);
        if let Some(parent) = dest.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)?;
        }
        super::process::copy_tree_atomic(&src, &dest)?;
    }
    Ok(true)
}

/// Copy a rule's resolved outputs into its cache entry — atomically: a
/// temp sibling directory is filled, renamed over the entry, and
/// `entry.json` written last as the completeness marker. A failed
/// populate leaves no partial entry behind.
///
/// Outputs that do not exist (declared patterns validation skipped) are
/// not cached; if nothing at all is copied, no entry is created.
pub async fn populate_outputs(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    entry_dir: &Path,
    manifest: &[InputManifestEntry],
    rendered_command: &str,
    key: &str,
) -> Result<()> {
    let tmp = sibling_tmp_dir(entry_dir);
    let _ = std::fs::remove_dir_all(&tmp);

    let mut copied_any = false;
    for output in &rule.output {
        let expanded = super::checkpoint::expand_config_in_path(output, wildcard_values);
        if crate::wildcard::has_wildcards(&expanded) {
            continue;
        }
        let src = workdir.join(&expanded);
        if !src.exists() {
            continue;
        }
        copied_any = true;
        let dest = tmp.join(&expanded);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        super::process::copy_tree(&src, &dest)?;
    }
    if !copied_any {
        let _ = std::fs::remove_dir_all(&tmp);
        return Ok(());
    }

    // Replace any previous entry with identical content (parallel
    // instances of the same rule race here; both write identical bytes).
    if entry_dir.exists() {
        std::fs::remove_dir_all(entry_dir)?;
    }
    std::fs::rename(&tmp, entry_dir)?;

    // Completeness marker, last: restores require it.
    let meta = serde_json::json!({
        "rule": rule.name,
        "cache_key": rule.cache_key.as_deref().unwrap_or(""),
        "key": key,
        "command": rendered_command,
        "inputs": manifest.iter().map(|e| &e.path).collect::<Vec<_>>(),
    });
    std::fs::write(
        entry_dir.join("entry.json"),
        serde_json::to_vec_pretty(&meta)?,
    )?;
    Ok(())
}

/// `{entry}.oxo-tmp` — a sibling of the entry dir, so the final rename
/// never crosses filesystems.
fn sibling_tmp_dir(entry_dir: &Path) -> PathBuf {
    let mut name = entry_dir
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".oxo-tmp");
    entry_dir.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::FilePatterns;

    fn rule_with_cache_key(key: &str) -> Rule {
        Rule {
            name: "cacheable".to_string(),
            cache_key: Some(key.to_string()),
            input: FilePatterns::List(vec!["input.txt".to_string()]),
            output: vec!["out/result.txt".to_string()].into(),
            ..Default::default()
        }
    }

    fn manifest_entry(path: &str, hash: Option<&str>) -> InputManifestEntry {
        InputManifestEntry {
            path: path.to_string(),
            size: 0,
            mtime_nanos: 0,
            hash: hash.map(str::to_string),
            remote: None,
        }
    }

    #[test]
    fn cache_entry_key_is_stable_and_content_sensitive() {
        let rule = rule_with_cache_key("align-v1");
        let manifest = vec![manifest_entry("input.txt", Some("sha256:aaa"))];
        let a = cache_entry_key(&rule, &HashMap::new(), "bwa mem", &manifest).unwrap();
        let b = cache_entry_key(&rule, &HashMap::new(), "bwa mem", &manifest).unwrap();
        assert_eq!(a, b, "identical content has an identical identity");

        // Any of the four dimensions changing changes the identity.
        let other_key = Rule {
            cache_key: Some("align-v2".to_string()),
            ..rule.clone()
        };
        assert_ne!(
            a,
            cache_entry_key(&other_key, &HashMap::new(), "bwa mem", &manifest).unwrap()
        );
        assert_ne!(
            a,
            cache_entry_key(&rule, &HashMap::new(), "bwa mem -t 8", &manifest).unwrap()
        );
        assert_ne!(
            a,
            cache_entry_key(
                &rule,
                &HashMap::new(),
                "bwa mem",
                &[manifest_entry("input.txt", Some("sha256:bbb"))],
            )
            .unwrap()
        );
    }

    #[test]
    fn cache_entry_key_refuses_unhashable_inputs() {
        let rule = rule_with_cache_key("k");
        // Local file over the hash cap: no content identity.
        assert!(
            cache_entry_key(
                &rule,
                &HashMap::new(),
                "cmd",
                &[manifest_entry("big.bam", None)]
            )
            .is_none()
        );
        // Remote object without an etag: no remote identity.
        let remote = InputManifestEntry {
            path: "s3://b/o".to_string(),
            size: 1,
            mtime_nanos: 0,
            hash: None,
            remote: Some(crate::executor::checkpoint::RemoteManifestEntry {
                scheme: "s3".to_string(),
                key: "s3://b/o".to_string(),
                size: 1,
                etag: None,
            }),
        };
        assert!(cache_entry_key(&rule, &HashMap::new(), "cmd", &[remote]).is_none());
        // Rules without cache_key never participate.
        let plain = Rule {
            cache_key: None,
            ..rule
        };
        let manifest = vec![manifest_entry("input.txt", Some("sha256:aaa"))];
        assert!(cache_entry_key(&plain, &HashMap::new(), "cmd", &manifest).is_none());
    }

    #[tokio::test]
    async fn populate_and_restore_roundtrip_outputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workdir = dir.path();
        let rule = rule_with_cache_key("k");
        let manifest = vec![manifest_entry("input.txt", Some("sha256:aaa"))];
        let key = cache_entry_key(&rule, &HashMap::new(), "cmd", &manifest).unwrap();
        let entry = cache_entry_dir(workdir, &rule.name, &key);

        // Populate requires the outputs to exist in the workdir.
        std::fs::create_dir_all(workdir.join("out")).unwrap();
        std::fs::write(workdir.join("out/result.txt"), b"result-bytes").unwrap();
        populate_outputs(
            &rule,
            workdir,
            &HashMap::new(),
            &entry,
            &manifest,
            "cmd",
            &key,
        )
        .await
        .expect("populate");
        assert!(entry.join("entry.json").exists(), "entry marker written");
        assert!(
            !entry
                .with_file_name(format!(
                    "{}.oxo-tmp",
                    entry.file_name().unwrap().to_string_lossy()
                ))
                .exists(),
            "no temp sibling litter"
        );

        // Remove the workdir outputs, then restore from the entry.
        std::fs::remove_file(workdir.join("out/result.txt")).unwrap();
        assert!(
            restore_outputs(&rule, workdir, &HashMap::new(), &entry)
                .await
                .expect("restore")
        );
        assert_eq!(
            std::fs::read(workdir.join("out/result.txt")).unwrap(),
            b"result-bytes"
        );
    }

    #[tokio::test]
    async fn restore_without_complete_entry_reports_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rule = rule_with_cache_key("k");
        let entry = cache_entry_dir(dir.path(), &rule.name, "some-key");
        std::fs::create_dir_all(&entry).unwrap(); // no entry.json marker
        assert!(
            !restore_outputs(&rule, dir.path(), &HashMap::new(), &entry)
                .await
                .expect("restore")
        );
    }
}
