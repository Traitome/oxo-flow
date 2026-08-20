//! Git helpers shared by workflow pulling and module includes (issue #112).

/// China-friendly clone mirrors tried after the official GitHub URL fails
/// (github.com URLs only — other hosts are left untouched).
const GITHUB_CLONE_MIRRORS: &[&str] = &["https://ghfast.top/", "https://gh-proxy.com/"];

/// Candidate clone URLs: the official URL first, then each mirror.
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
