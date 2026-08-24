//! Git helpers shared by workflow pulling and module includes (issue #112).
//!
//! Pinned module includes (config.rs) use [`ensure_pinned`], which manages
//! the cache lifecycle: clone on miss, re-fetch branch/tag refs on every
//! activation, full-SHA pins fetched by SHA, cross-process locking, and
//! cleanup of partial clones (issue #136).

use fs2::FileExt;

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
    // Walk starts at the directory itself when `path` is a directory, and
    // at its parent when it is a file — the doc promised this, the code
    // skipped the first step (issue #136).
    let mut dir = if start.is_dir() {
        Some(start.as_path())
    } else {
        Some(start.parent().unwrap_or(&start))
    };
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Classify a pinned ref: a full 40-hex commit SHA must be fetched by SHA —
/// `git clone --branch <sha>` fails with "Remote branch `<sha>` not found
/// upstream" because SHAs are not advertised refs. Anything else (branch,
/// tag, short SHA) goes through the regular `--branch` clone.
pub fn is_full_sha(git_ref: &str) -> bool {
    git_ref.len() == 40 && git_ref.chars().all(|c| c.is_ascii_hexdigit())
}

/// Command plan for cloning an unadvertised commit SHA: `git init`, add the
/// origin, `git fetch origin <sha>`, detached checkout of FETCH_HEAD.
/// Extracted so tests exercise the production command strings without
/// touching the network.
fn clone_sha_commands(dest: &std::path::Path, url: &str, sha: &str) -> [Vec<String>; 4] {
    let dir = dest.to_string_lossy().into_owned();
    [
        vec!["init".to_string(), dir.clone()],
        vec![
            "-C".to_string(),
            dir.clone(),
            "remote".to_string(),
            "add".to_string(),
            "origin".to_string(),
            url.to_string(),
        ],
        vec![
            "-C".to_string(),
            dir.clone(),
            "fetch".to_string(),
            "--depth".to_string(),
            "1".to_string(),
            "origin".to_string(),
            sha.to_string(),
        ],
        vec![
            "-C".to_string(),
            dir,
            "checkout".to_string(),
            "--detach".to_string(),
            "FETCH_HEAD".to_string(),
        ],
    ]
}

/// Command plan for cloning a branch/tag ref with a shallow `--branch`.
fn clone_ref_commands(url: &str, git_ref: &str, dest: &std::path::Path) -> Vec<String> {
    vec![
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--branch".to_string(),
        git_ref.to_string(),
        url.to_string(),
        dest.to_string_lossy().into_owned(),
    ]
}

/// Command plan for bringing a cached clone up to date with `git_ref`:
/// re-fetch on every activation (branch/tag pins must track the remote
/// instead of freezing at the first snapshot) and activate the new state.
/// Full-SHA pins skip the refresh entirely — they are immutable, and
/// `ensure_pinned` guards them with a readability check instead.
fn refresh_commands(dest: &std::path::Path, git_ref: &str) -> [Vec<String>; 2] {
    let dir = dest.to_string_lossy().into_owned();
    [
        vec![
            "-C".to_string(),
            dir.clone(),
            "fetch".to_string(),
            "--depth".to_string(),
            "1".to_string(),
            "origin".to_string(),
            git_ref.to_string(),
        ],
        vec![
            "-C".to_string(),
            dir,
            "reset".to_string(),
            "--hard".to_string(),
            "FETCH_HEAD".to_string(),
        ],
    ]
}

/// Sibling lock file for a cache dir. flock semantics: the OS releases the
/// lock when the holder process dies, so a SIGKILLed clone cannot wedge the
/// cache — the lock file itself may stay behind, harmlessly.
fn lock_path_for(dest: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.lock", dest.display()))
}

/// Remove a partial clone directory. Tolerates an already-missing dir so
/// callers can clean up unconditionally after a failed attempt.
fn remove_partial_clone(dest: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            tracing::warn!(dir = %dest.display(), error = %e, "failed to remove partial clone");
            Err(e)
        }
    }
}

/// Bounded-wait exclusive lock around one module-cache entry's clone or
/// refresh. Two concurrent runs cloning into the same dir race: the loser's
/// `git clone` fails on the non-empty dir and its cleanup can then delete
/// the winner's completed checkout (issue #136).
struct CloneLock {
    file: std::fs::File,
}

/// How long a clone/refresh holder may hold the lock before contenders give
/// up: depth-1 clones of typical module repos take seconds; a holder stuck
/// on a hung network clone must not block every other run forever.
const CLONE_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

impl CloneLock {
    fn acquire(dest: &std::path::Path) -> std::io::Result<CloneLock> {
        let path = lock_path_for(dest);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        let deadline = std::time::Instant::now() + CLONE_LOCK_TIMEOUT;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(CloneLock { file }),
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(std::io::Error::other(format!(
                        "timed out waiting for module cache lock {path:?}: {e}"
                    )));
                }
            }
        }
    }
}

impl Drop for CloneLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Run one git command from a plan, returning its stderr on failure.
fn run_git_plan(plan: &[String]) -> std::io::Result<()> {
    let out = std::process::Command::new("git").args(plan).output()?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let display = plan.join(" ");
    if stderr.is_empty() {
        Err(std::io::Error::other(format!(
            "`git {display}` failed (exit {})",
            out.status.code().unwrap_or(-1)
        )))
    } else {
        Err(std::io::Error::other(format!(
            "`git {display}` failed: {stderr}"
        )))
    }
}

/// Clone `repo` at `git_ref` (tag/branch/commit) into `dest`, with mirror
/// fallback. Full commit SHAs are fetched by SHA (see [`is_full_sha`]); any
/// other ref uses `git clone --depth 1 --branch`. A partial directory left
/// by a failed attempt is removed so callers can retry cleanly.
pub fn clone_pinned(repo_url: &str, git_ref: &str, dest: &std::path::Path) -> std::io::Result<()> {
    let mut failures: Vec<String> = Vec::new();
    for (index, candidate) in mirror_candidates(repo_url).iter().enumerate() {
        if index > 0 {
            tracing::info!("official clone failed — trying mirror {candidate}");
        }
        let plan: Vec<Vec<String>> = if is_full_sha(git_ref) {
            clone_sha_commands(dest, candidate, git_ref).to_vec()
        } else {
            vec![clone_ref_commands(candidate, git_ref, dest)]
        };
        let mut failed = false;
        for command in &plan {
            if let Err(e) = run_git_plan(command) {
                failures.push(e.to_string());
                failed = true;
                break;
            }
        }
        if !failed {
            return Ok(());
        }
        let _ = remove_partial_clone(dest);
    }
    Err(std::io::Error::other(format!(
        "failed to clone '{repo_url}' at '{git_ref}': {}",
        failures.join("; ")
    )))
}

/// Bring an existing cached clone up to date with `git_ref` (see
/// [`refresh_commands`]).
fn refresh_pinned(dest: &std::path::Path, git_ref: &str) -> std::io::Result<()> {
    for command in refresh_commands(dest, git_ref) {
        run_git_plan(&command)?;
    }
    Ok(())
}

/// Ensure a usable pinned clone of `repo@git_ref` exists at `dest`: clone
/// on a cache miss (healing any partial dir a killed clone left behind),
/// re-fetch branch/tag refs on every activation (branch pins must not
/// freeze at their first snapshot), and heal a cache entry whose checkout
/// is unreadable (e.g. a SIGKILLed clone that left `.git` behind with a
/// truncated checkout — that must not poison the cache permanently).
/// Full-SHA pins are immutable: no re-fetch on activation — a commit
/// force-pushed away upstream must not break a locally complete checkout —
/// only the readability guard. Serialized across processes by `CloneLock`.
pub fn ensure_pinned(repo_url: &str, git_ref: &str, dest: &std::path::Path) -> std::io::Result<()> {
    let _lock = CloneLock::acquire(dest)?;
    if !dest.join(".git").exists() {
        let _ = remove_partial_clone(dest);
        return clone_pinned(repo_url, git_ref, dest);
    }
    // Full-SHA pins are immutable — no re-fetch (a commit force-pushed away
    // upstream must not break a locally complete checkout); branch/tag refs
    // are re-fetched on every activation so they don't freeze at their first
    // snapshot.
    let refreshed = if is_full_sha(git_ref) {
        Ok(())
    } else {
        refresh_pinned(dest, git_ref)
    };
    match refreshed {
        Ok(()) => Ok(()),
        Err(refresh_err) => {
            // A truncated checkout (`rev-parse` fails) is cache poison from
            // a killed clone — remove and re-clone. A plain network failure
            // on a healthy checkout must surface, not silently re-clone.
            if has_valid_head(dest) {
                return Err(refresh_err);
            }
            let _ = remove_partial_clone(dest);
            clone_pinned(repo_url, git_ref, dest)
        }
    }
}

/// Is the cache entry's checkout readable (`git rev-parse HEAD` succeeds)?
/// Cheap guard against a SIGKILLed clone that left `.git` behind with a
/// truncated checkout.
fn has_valid_head(dest: &std::path::Path) -> bool {
    let dir = dest.to_string_lossy();
    std::process::Command::new("git")
        .args(["-C", dir.as_ref(), "rev-parse", "--verify", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
///
/// The readable prefix preserves the old `_`-flattened form; the hash
/// suffix is the identity. Flattening alone is lossy: `https://a/b` and
/// `https://a_b` both become `https_a_b`, so unrelated modules would
/// share a cache dir and clobber each other (issue #136). The FNV-1a
/// 64-bit hash over `repo\0ref` separates confusable pairs while the
/// prefix keeps the directory names human-readable.
pub fn cache_dir_name(repo_url: &str, git_ref: &str) -> String {
    let repo = repo_url
        .trim_end_matches('/')
        .replace("://", "_")
        .replace('/', "_");
    let reference = git_ref.replace('/', "_");
    format!(
        "{repo}@{reference}@{:016x}",
        cache_dir_hash(repo_url, git_ref)
    )
}

/// FNV-1a 64-bit over `repo_url\0git_ref` (NUL-separated so adjacent
/// segments cannot run together — repo URLs and git refs cannot contain
/// NUL, so the separator is unambiguous).
fn cache_dir_hash(repo_url: &str, git_ref: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in repo_url
        .trim_end_matches('/')
        .bytes()
        .chain(std::iter::once(0))
        .chain(git_ref.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_full_sha_classifies_ref_kinds() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(sha.len(), 40);
        assert!(is_full_sha(sha), "40-hex commit SHAs are fetchable by SHA");
        assert!(!is_full_sha("main"));
        assert!(!is_full_sha("v0.13.0"));
        assert!(
            !is_full_sha("a1b2c3"),
            "short SHAs are not advertised refs either"
        );
        assert!(!is_full_sha(""));
        assert!(
            !is_full_sha(&format!("{sha}Z")),
            "non-hex chars are not a SHA"
        );
        assert!(!is_full_sha(&format!("{sha}0")), "41 chars is not a SHA");
    }

    #[test]
    fn sha_pin_clone_plan_fetches_by_sha_never_branch() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let dest = std::path::Path::new(
            "/cache/https_github.com_org_repo@0123456789abcdef0123456789abcdef01234567",
        );
        let commands = clone_sha_commands(dest, "https://github.com/org/repo", sha);
        assert_eq!(
            commands.len(),
            4,
            "init, remote add, fetch by sha, detached checkout"
        );
        let joined = commands
            .iter()
            .flatten()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains(&format!("fetch --depth 1 origin {sha}")),
            "an unadvertised SHA must be fetched by SHA: {joined}"
        );
        assert!(joined.contains("checkout --detach FETCH_HEAD"));
        assert!(
            !joined.contains("--branch"),
            "git clone --branch <sha> fails with 'Remote branch not found': {joined}"
        );
    }

    #[test]
    fn branch_pin_clone_plan_uses_branch_flag() {
        let dest = std::path::Path::new("/cache/x@main");
        let cmd = clone_ref_commands("https://github.com/org/repo", "main", dest);
        assert_eq!(
            cmd,
            vec![
                "clone".to_string(),
                "--depth".to_string(),
                "1".to_string(),
                "--branch".to_string(),
                "main".to_string(),
                "https://github.com/org/repo".to_string(),
                "/cache/x@main".to_string(),
            ]
        );
    }

    #[test]
    fn refresh_plan_refetches_the_ref_and_activates_it() {
        let dest = std::path::Path::new("/cache/x@main");
        let commands = refresh_commands(dest, "main");
        let joined = commands
            .iter()
            .flatten()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("fetch --depth 1 origin main"),
            "branch refs must be re-fetched on every activation, not frozen at the first snapshot: {joined}"
        );
        assert!(joined.contains("reset --hard FETCH_HEAD"));
    }

    #[test]
    fn lock_path_is_sibling_lock_file() {
        let dest = std::path::Path::new("/cache/https_github.com_org_repo@main");
        assert_eq!(
            lock_path_for(dest),
            std::path::PathBuf::from("/cache/https_github.com_org_repo@main.lock")
        );
    }

    #[test]
    fn remove_partial_clone_cleans_incomplete_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let clone = dir.path().join("partial");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        std::fs::write(clone.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        std::fs::write(clone.join("wf.oxoflow"), "stale").unwrap();
        remove_partial_clone(&clone).unwrap();
        assert!(
            !clone.exists(),
            "a SIGKILLed clone's partial dir must be removed so the next run starts clean"
        );
        // Removing a missing dir is a no-op, not an error.
        assert!(remove_partial_clone(&clone).is_ok());
    }

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
        // A directory outside any repository is also None.
        assert_eq!(find_repo_root(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_repo_root_handles_directory_inputs() {
        let dir = std::env::temp_dir().join(format!("oxo-gitdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join("sub/dir")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // The repo directory itself must be found — the doc promises
        // "walk starts at the directory itself", but the code skipped the
        // first step and walked from the parent (issue #136).
        assert_eq!(
            find_repo_root(&repo),
            Some(repo.clone()),
            "a repo passed as a directory must resolve to itself"
        );
        // A directory deep inside the repo resolves to the root.
        assert_eq!(
            find_repo_root(&repo.join("sub/dir")),
            Some(repo),
            "a nested directory must resolve to the containing repo"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_dir_names_are_injective_for_confusable_inputs() {
        // The old scheme flattened both repo URL and ref with `_` —
        // `https://github.com/a/b` and `https://github.com/a_b` collapsed
        // to the same cache dir, so unrelated modules clobbered each other
        // (issue #136). The pair now carries a hash suffix: identical
        // inputs still hash identically, confusable inputs differ.
        let a_b = cache_dir_name("https://github.com/a/b", "main");
        let a_b_confusable = cache_dir_name("https://github.com/a_b", "main");
        assert_ne!(
            a_b, a_b_confusable,
            "`a/b` and `a_b` must not share a cache dir"
        );
        assert_eq!(
            cache_dir_name("https://github.com/a/b", "main"),
            a_b,
            "the name must be deterministic for the same pair"
        );
        // Trailing slashes on the repo URL are normalized before hashing.
        assert_eq!(
            cache_dir_name("https://github.com/a/b/", "main"),
            a_b,
            "a trailing slash must not change the cache dir"
        );
        // The prefix stays readable for humans.
        assert!(
            a_b.starts_with("https_github.com_a_b@main@"),
            "the readable prefix must be preserved: {a_b}"
        );
        // Ref slashes are flattened like before, still hashed apart.
        let with_slash = cache_dir_name("https://github.com/a/b", "feature/x");
        assert!(with_slash.starts_with("https_github.com_a_b@feature_x@"));
        assert_ne!(with_slash, a_b);
    }
}
