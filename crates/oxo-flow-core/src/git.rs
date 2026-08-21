//! Git helpers shared by workflow pulling and module includes (issue #112).

/// China-friendly clone mirrors tried after the official GitHub URL fails
/// (github.com URLs only — other hosts are left untouched).
const GITHUB_CLONE_MIRRORS: &[&str] = &["https://ghfast.top/", "https://gh-proxy.com/"];

/// Candidate clone URLs: the official URL first, then each mirror.
/// Locate the root of the git repository containing `path`, if any: the
/// nearest ancestor directory holding a `.git` entry. Walks up from
/// `path`'s parent (when `path` is a file) or `path` itself (when it is a
/// directory). Shared by checkpoint provenance (issue #115 pillar 1) and
/// catalog metadata derivation (`info --json`, issue #124 pillar 3).
pub fn find_repo_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
    // Absolute first: lexical parent-walking on a shallow relative path
    // (e.g. `examples/gallery/x.oxoflow` from the repo root) terminates at
    // the empty path and would never reach the CWD's repository.
    let start = std::path::absolute(path).ok()?;
    let mut dir = Some(start.parent().unwrap_or(&start));
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

pub fn mirror_candidates(repo_url: &str) -> Vec<String> {
    let official = repo_url.trim_end_matches('/').to_string();
    if !official.starts_with("https://github.com/") {
        return vec![official];
    }
    std::iter::once(official.clone())
        .chain(
            GITHUB_CLONE_MIRRORS
                .iter()
                .map(|prefix| format!("{prefix}{official}")),
        )
        .collect()
}

/// Clone `repo` at `git_ref` (tag/branch/commit) into `dest`, with mirror
/// fallback. A partial directory left by a failed attempt is removed so
/// callers can retry cleanly.
pub fn clone_pinned(repo_url: &str, git_ref: &str, dest: &std::path::Path) -> std::io::Result<()> {
    let mut failures: Vec<String> = Vec::new();
    for (index, candidate) in mirror_candidates(repo_url).iter().enumerate() {
        if index > 0 {
            tracing::info!("official clone failed — trying mirror {candidate}");
        }
        let out = std::process::Command::new("git")
            .args(["clone", "--depth", "1", "--branch", git_ref, candidate])
            .arg(dest)
            .output()?;
        if out.status.success() {
            return Ok(());
        }
        failures.push(String::from_utf8_lossy(&out.stderr).trim().to_string());
        let _ = std::fs::remove_dir_all(dest);
    }
    Err(std::io::Error::other(format!(
        "failed to clone '{repo_url}' at '{git_ref}': {}",
        failures.join("; ")
    )))
}

/// Module cache root: `$OXO_FLOW_MODULE_CACHE` or `~/.cache/oxo-flow/modules`.
pub fn module_cache_root() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("OXO_FLOW_MODULE_CACHE") {
        return std::path::PathBuf::from(dir);
    }
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(".cache/oxo-flow/modules")
}

/// Filesystem-safe cache key for a repo@ref pair.
pub fn cache_dir_name(repo_url: &str, git_ref: &str) -> String {
    let mut name = repo_url
        .trim_end_matches('/')
        .replace("://", "_")
        .replace('/', "_");
    name.push_str(&format!("@{}", git_ref.replace('/', "_")));
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_repo_root_resolves_cwd_relative_workflow() {
        // Regression: a shallow CWD-relative path (e.g. `examples/gallery/
        // x.oxoflow` from the repo root) must still reach the repository —
        // lexical parent-walking alone stops at the empty path and misses
        // the CWD's repo. The core crate's test CWD is the workspace root.
        let wf = std::path::Path::new("Cargo.toml");
        let root = find_repo_root(wf).expect("workspace root is a git repo");
        assert!(root.join(".git").exists());
    }

    #[test]
    fn find_repo_root_returns_none_outside_repo() {
        let dir = std::env::temp_dir().join(format!("oxo-gitroot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(find_repo_root(&dir.join("wf.oxoflow")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
