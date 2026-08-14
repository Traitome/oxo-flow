//! Remote input staging and output upload prep for the local executor
//! (issue #80 item 2).
//!
//! The engine stages remote inputs (exact object URIs — globs and directory
//! references are rejected) into `.oxo-flow/staged/in/…` before execution
//! and redirects remote outputs to `.oxo-flow/staged/out/…` local paths the
//! rule writes; after a successful run the engine uploads them. The
//! substitution happens on a **copy** of the rule — `config.rules` is never
//! mutated, and checkpoint manifests keep recording the original remote URIs.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{OxoFlowError, Result};
use crate::rule::{FilePatterns, Rule};
use crate::storage::{StoragePath, StorageResolver};

/// A prepared rule: remote inputs staged, remote outputs redirected, and
/// the pending uploads (remote URI → local stage path) in declared order.
#[derive(Debug)]
pub struct RemoteIoPrep {
    /// The substituted rule copy.
    pub rule: Rule,
    /// Remote outputs to upload after the rule succeeds.
    pub uploads: Vec<(StoragePath, std::path::PathBuf)>,
    /// A declared optional input does not exist remotely — the rule is a
    /// skip candidate (same contract as local optional inputs).
    pub missing_optional_input: bool,
}

/// Expand a pattern with the instance's wildcard values — the same pass
/// `render_shell_command` performs at the end of rendering.
fn expand_patterns(pattern: &str, wildcard_values: &HashMap<String, String>) -> String {
    let mut expanded = pattern.to_string();
    for (key, value) in wildcard_values {
        expanded = expanded.replace(&format!("{{{key}}}"), value);
    }
    expanded
}

/// Substitute patterns through an original → local-path map.
fn substitute_patterns(patterns: &FilePatterns, map: &HashMap<String, String>) -> FilePatterns {
    match patterns {
        FilePatterns::List(v) => FilePatterns::List(
            v.iter()
                .map(|p| map.get(p).cloned().unwrap_or_else(|| p.clone()))
                .collect(),
        ),
        FilePatterns::Map(m) => FilePatterns::Map(
            m.iter()
                .map(|(k, v)| (k.clone(), map.get(v).cloned().unwrap_or_else(|| v.clone())))
                .collect(),
        ),
        // Remote Dir patterns are rejected before substitution is reached.
        FilePatterns::Dir { .. } => patterns.clone(),
    }
}

fn remote_wildcard_error(kind: &str, pattern: &str) -> OxoFlowError {
    OxoFlowError::Config {
        message: format!(
            "remote {kind} '{pattern}' still contains a wildcard — remote globs and \
             directory references are not supported (exact object URIs only)"
        ),
    }
}

/// Stage every remote input and prepare remote outputs.
///
/// Returns `None` when the rule has no remote paths at all (fast path —
/// purely local workflows keep today's behaviour exactly).
pub async fn stage_remote_io(
    rule: &Rule,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
    resolver: &StorageResolver,
) -> Result<Option<RemoteIoPrep>> {
    let mut input_map: HashMap<String, String> = HashMap::new();
    let mut output_map: HashMap<String, String> = HashMap::new();
    let mut uploads: Vec<(StoragePath, std::path::PathBuf)> = Vec::new();
    let mut missing_optional_input = false;
    let mut saw_remote = false;

    if let FilePatterns::Dir { path, .. } = &rule.input
        && StoragePath::parse(path).is_remote()
    {
        return Err(remote_wildcard_error("input", path));
    }
    if let FilePatterns::Dir { path, .. } = &rule.output
        && StoragePath::parse(path).is_remote()
    {
        return Err(remote_wildcard_error("output", path));
    }

    // ── inputs ─────────────────────────────────────────────────────────────
    for pattern in rule.input.to_vec() {
        let expanded = expand_patterns(&pattern, wildcard_values);
        let storage_path = StoragePath::parse(&expanded);
        if !storage_path.is_remote() {
            continue;
        }
        saw_remote = true;
        if expanded.contains('{') {
            return Err(remote_wildcard_error("input", &pattern));
        }
        let Some(backend) = resolver.get_backend(&storage_path.scheme) else {
            // Degrade gracefully (the #78 P2 contract): the shell keeps
            // seeing the raw URI and must handle it itself.
            tracing::warn!(
                input = %expanded,
                "no storage backend registered — the shell must handle this remote path itself"
            );
            continue;
        };
        match backend.stage(&storage_path, workdir).await {
            Ok(local) => {
                let rel = local
                    .strip_prefix(workdir)
                    .unwrap_or(&local)
                    .to_string_lossy()
                    .to_string();
                tracing::debug!(input = %expanded, staged = %rel, "staged remote input");
                input_map.insert(pattern, rel);
            }
            Err(e) if rule.optional => {
                tracing::warn!(
                    input = %expanded,
                    error = %e,
                    "optional remote input unavailable; treating as missing"
                );
                missing_optional_input = true;
            }
            Err(e) => {
                return Err(OxoFlowError::Execution {
                    rule: rule.name.clone(),
                    message: format!("failed to stage remote input '{expanded}': {e}"),
                });
            }
        }
    }

    // ── outputs ────────────────────────────────────────────────────────────
    for pattern in rule.output.to_vec() {
        let expanded = expand_patterns(&pattern, wildcard_values);
        let storage_path = StoragePath::parse(&expanded);
        if !storage_path.is_remote() {
            continue;
        }
        saw_remote = true;
        if expanded.contains('{') {
            return Err(remote_wildcard_error("output", &pattern));
        }
        let local = crate::storage::upload_stage_path(workdir, &storage_path);
        if let Some(parent) = local.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| OxoFlowError::Config {
                message: format!(
                    "failed to create upload staging dir {}: {e}",
                    parent.display()
                ),
            })?;
        }
        let rel = local
            .strip_prefix(workdir)
            .unwrap_or(&local)
            .to_string_lossy()
            .to_string();
        output_map.insert(pattern, rel);
        uploads.push((storage_path, local));
    }

    if !saw_remote {
        return Ok(None);
    }

    // Warn when the shell references a remote URI literally: the staged
    // path only reaches the shell through the {input[n]}/{output[n]}
    // placeholders.
    for text in rule.shell.iter().chain(rule.script.iter()) {
        for uri in input_map.keys().chain(output_map.keys()) {
            let expanded = expand_patterns(uri, wildcard_values);
            if text.contains(&expanded) {
                tracing::warn!(
                    rule = %rule.name,
                    uri = %expanded,
                    "shell references a remote URI directly — use {{input[n]}}/{{output[n]}} \
                     to receive the staged local path"
                );
            }
        }
    }

    let mut substituted = rule.clone();
    if !input_map.is_empty() {
        substituted.input = substitute_patterns(&rule.input, &input_map);
    }
    if !output_map.is_empty() {
        substituted.output = substitute_patterns(&rule.output, &output_map);
    }

    Ok(Some(RemoteIoPrep {
        rule: substituted,
        uploads,
        missing_optional_input,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{RemoteStat, StorageBackend, StorageScheme};
    use std::sync::Arc;

    /// In-memory fake: objects map URI → (content, etag), plus counters.
    struct FakeCloud {
        objects: tokio::sync::Mutex<HashMap<String, (Vec<u8>, String)>>,
        stage_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl StorageBackend for FakeCloud {
        async fn exists(&self, path: &StoragePath) -> Result<bool> {
            Ok(self.objects.lock().await.contains_key(&path.raw))
        }

        async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>> {
            Ok(self.objects.lock().await.get(&path.raw).map(|(c, etag)| RemoteStat {
                size: c.len() as u64,
                etag: Some(etag.clone()),
            }))
        }

        async fn read_to_string(&self, path: &StoragePath) -> Result<String> {
            let bytes = self.objects.lock().await.get(&path.raw).unwrap().0.clone();
            Ok(String::from_utf8(bytes).unwrap())
        }

        async fn write(&self, path: &StoragePath, content: &[u8]) -> Result<()> {
            self.objects
                .lock()
                .await
                .insert(path.raw.clone(), (content.to_vec(), "etag".into()));
            Ok(())
        }

        async fn stage(&self, path: &StoragePath, workdir: &Path) -> Result<std::path::PathBuf> {
            self.stage_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dest = crate::storage::staged_path(workdir, path);
            let (content, etag) = self
                .objects
                .lock()
                .await
                .get(&path.raw)
                .cloned()
                .ok_or_else(|| OxoFlowError::Config {
                    message: "no such object".into(),
                })?;
            crate::storage::stage_with_cache(
                RemoteStat {
                    size: content.len() as u64,
                    etag: Some(etag),
                },
                &dest,
                move |mut file| {
                    let content = content.clone();
                    async move {
                        tokio::io::AsyncWriteExt::write_all(&mut file, &content)
                            .await
                            .map_err(|e| OxoFlowError::Config {
                                message: e.to_string(),
                            })
                    }
                },
            )
            .await?;
            Ok(dest)
        }

        async fn upload(&self, local: &Path, remote: &StoragePath) -> Result<()> {
            let content = tokio::fs::read(local).await.map_err(|e| OxoFlowError::Config {
                message: e.to_string(),
            })?;
            self.write(remote, &content).await
        }

        fn name(&self) -> &'static str {
            "fake"
        }
    }

    fn resolver_with(uri: &str, content: &[u8], etag: &str) -> (StorageResolver, Arc<FakeCloud>) {
        let fake = Arc::new(FakeCloud {
            objects: tokio::sync::Mutex::new(HashMap::from([(
                uri.to_string(),
                (content.to_vec(), etag.to_string()),
            )])),
            stage_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let mut resolver = StorageResolver::with_local();
        resolver.add_backend(StorageScheme::S3, fake.clone());
        (resolver, fake)
    }

    fn rule_with(io: &str) -> Rule {
        toml::from_str(&format!(
            r#"
name = "t"
shell = "cat {{input[0]}} > {{output[0]}}"
input = {io}
output = ["out.txt"]
"#,
            io = io
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn local_only_returns_none() {
        let rule = rule_with(r#"["data/local.fq"]"#);
        let resolver = StorageResolver::with_local();
        let dir = tempfile::tempdir().unwrap();
        let prep = stage_remote_io(&rule, dir.path(), &HashMap::new(), &resolver)
            .await
            .unwrap();
        assert!(prep.is_none());
    }

    #[tokio::test]
    async fn remote_input_is_staged_and_substituted() {
        let (resolver, fake) = resolver_with("s3://b/k.fq", b">S1\nACGT\n", "e1");
        let rule = rule_with(r#"["s3://b/k.fq", "data/local.fq"]"#);
        let dir = tempfile::tempdir().unwrap();
        let prep = stage_remote_io(&rule, dir.path(), &HashMap::new(), &resolver)
            .await
            .unwrap()
            .expect("remote prep");
        assert_eq!(
            prep.rule.input.to_vec(),
            vec![".oxo-flow/staged/in/s3/b/k.fq".to_string(), "data/local.fq".into()]
        );
        assert!(dir
            .path()
            .join(".oxo-flow/staged/in/s3/b/k.fq")
            .exists());
        assert_eq!(
            fake.stage_calls.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[tokio::test]
    async fn remote_map_output_is_redirected_and_listed_for_upload() {
        let (resolver, _) = resolver_with("s3://b/k.fq", b"x", "e1");
        let rule: Rule = toml::from_str(
            r#"
name = "t"
shell = "true"
input = []
output = { result = "s3://b/out.txt" }
"#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let prep = stage_remote_io(&rule, dir.path(), &HashMap::new(), &resolver)
            .await
            .unwrap()
            .expect("remote prep");
        let FilePatterns::Map(m) = &prep.rule.output else {
            panic!("expected Map");
        };
        assert_eq!(m["result"], ".oxo-flow/staged/out/s3/b/out.txt");
        assert_eq!(prep.uploads.len(), 1);
        assert_eq!(prep.uploads[0].0.raw, "s3://b/out.txt");
    }

    #[tokio::test]
    async fn remote_glob_is_rejected() {
        let (resolver, _) = resolver_with("s3://b/k.fq", b"x", "e1");
        let rule = rule_with(r#"["s3://b/{sample}.fq"]"#);
        let dir = tempfile::tempdir().unwrap();
        let err = stage_remote_io(&rule, dir.path(), &HashMap::new(), &resolver)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("wildcard"));
    }

    #[tokio::test]
    async fn missing_optional_input_marks_skip_candidate() {
        let (resolver, _) = resolver_with("s3://b/other.fq", b"x", "e1");
        let mut rule = rule_with(r#"["s3://b/gone.fq"]"#);
        rule.optional = true;
        let dir = tempfile::tempdir().unwrap();
        let prep = stage_remote_io(&rule, dir.path(), &HashMap::new(), &resolver)
            .await
            .unwrap()
            .expect("remote prep");
        assert!(prep.missing_optional_input);
    }

    #[tokio::test]
    async fn missing_nonoptional_input_is_an_error() {
        let (resolver, _) = resolver_with("s3://b/other.fq", b"x", "e1");
        let rule = rule_with(r#"["s3://b/gone.fq"]"#);
        let dir = tempfile::tempdir().unwrap();
        let err = stage_remote_io(&rule, dir.path(), &HashMap::new(), &resolver)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("stage remote input"));
    }
}
