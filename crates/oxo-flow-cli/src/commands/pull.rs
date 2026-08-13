//! Pull — fetch an oxo-flow workflow from a remote source.
//!
//! Two modes:
//!
//! **Bundle mode** (versioned, checksummed artifact):
//! - `https://example.com/path/to/bundle.tar.zst` — direct download (.tar.zst or .tar.gz)
//! - `gh:owner/repo@v1.0.0` — GitHub release asset (finds .tar.zst or .tar.gz)
//! - `file:///path/to/bundle.tar.zst` — local file copy
//!
//! **Repository mode** (non-bundle — plain git checkout, no packaging step):
//! - `gh:owner/repo` — clone the default branch via git
//! - `https://example.com/team/pipeline.git` — any git URL ending in `.git`
//! - `file:///path/to/repo` — clone a local repository (a directory)
//!
//! Repository mode runs `git clone`, auto-discovers the `.oxoflow` file
//! (main.oxoflow first, else the alphabetically first), and sanity-parses
//! it with the engine. Private repositories work through the user's normal
//! git credentials.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

/// What a pull URL points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PullSource {
    /// GitHub Release asset bundle (`gh:owner/repo@tag`).
    BundleGithub { repo: String, tag: String },
    /// Direct bundle file download (http/https, not ending in `.git`).
    BundleHttp(String),
    /// Local path — a file is a bundle copy, a directory is a git repo.
    FileLocal(String),
    /// A git repository to clone.
    RepoGit { url: String, dir_name: String },
}

/// Classify a pull URL deterministically — no silent fallbacks between modes.
pub(crate) fn classify_pull_source(url: &str) -> PullSource {
    if let Some(spec) = url.strip_prefix("gh:") {
        return match spec.split_once('@') {
            Some((repo, tag)) => PullSource::BundleGithub {
                repo: repo.to_string(),
                tag: tag.to_string(),
            },
            // No @ref: the release namespace is unambiguous — clone the repo.
            None => PullSource::RepoGit {
                url: format!("https://github.com/{spec}.git"),
                dir_name: repo_dir_name(spec),
            },
        };
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        if url.ends_with(".git") {
            return PullSource::RepoGit {
                url: url.to_string(),
                dir_name: repo_dir_name(url.trim_end_matches(".git")),
            };
        }
        return PullSource::BundleHttp(url.to_string());
    }
    if url.starts_with("file://") {
        return PullSource::FileLocal(url.trim_start_matches("file://").to_string());
    }
    // Bare local path (bundle file or repo directory).
    PullSource::FileLocal(url.to_string())
}

/// Derive a clone directory name from a repo spec (last path segment).
fn repo_dir_name(spec: &str) -> String {
    spec.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("workflow")
        .trim_end_matches(".git")
        .to_string()
}

/// GitHub clone mirrors tried when the official URL fails — ghproxy-style
/// prefixes, matching docs/guide/src/how-to/china-mirrors.md. Only applied
/// to github.com URLs; other hosts get no fallback.
const GITHUB_CLONE_MIRRORS: &[&str] = &["https://ghfast.top/", "https://gh-proxy.com/"];

/// Candidate clone URLs in fallback order: the official URL first, then the
/// mirror-prefixed forms for github.com URLs.
pub(crate) fn mirror_candidates(repo_url: &str) -> Vec<String> {
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

/// `git clone` with mirror fallback: the official URL is tried first, then
/// each China mirror in turn (github.com URLs only). Every failure is
/// reported; a partial clone directory left behind by a failed attempt is
/// removed so callers can retry cleanly.
pub(crate) async fn clone_repo(repo_url: &str, git_ref: Option<&str>, target: &Path) -> Result<()> {
    let mut failures: Vec<String> = Vec::new();
    for (index, candidate) in mirror_candidates(repo_url).iter().enumerate() {
        if index > 0 {
            eprintln!(
                "{} official clone failed — trying mirror {candidate}...",
                "↻".yellow()
            );
        }
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["clone", "--depth", "1"]);
        if let Some(branch) = git_ref {
            cmd.args(["--branch", branch]);
        }
        cmd.arg(candidate).arg(target);
        match cmd.status().await {
            Ok(status) if status.success() => {
                eprintln!("{} Cloned {candidate}", "✓".green().bold());
                return Ok(());
            }
            Ok(status) => failures.push(format!(
                "{candidate} (exit {})",
                status.code().unwrap_or(-1)
            )),
            Err(e) => failures.push(format!("{candidate} ({e})")),
        }
    }
    // A failed attempt can leave a partial directory — never leave it behind.
    if target.exists() {
        let _ = std::fs::remove_dir_all(target);
    }
    anyhow::bail!(
        "git clone failed — tried:\n  {}\n\
         Check network access or use a VPN/proxy. For China networks see \
         docs/guide/src/how-to/china-mirrors.md (mirrors are tried \
         automatically for github.com URLs).",
        failures.join("\n  ")
    )
}

/// Download and verify a bundle, or clone a repository — see [`PullSource`].
pub async fn pull_command(url: &str, output: Option<PathBuf>) -> Result<()> {
    match classify_pull_source(url) {
        PullSource::RepoGit { url: repo_url, .. } => {
            pull_repo_git(&repo_url, output).await?;
            Ok(())
        }
        PullSource::FileLocal(path) if Path::new(&path).is_dir() => {
            // A local directory is a git repository to clone — useful for
            // testing and air-gapped distribution.
            pull_repo_git(&path, output).await?;
            Ok(())
        }
        source => pull_bundle(&source, url, output).await,
    }
}

/// Clone a git repository, discover its workflow file, and sanity-parse it.
///
/// Returns the discovered workflow path. The clone target defaults to the
/// repo's directory name in the current directory.
pub(crate) async fn pull_repo_git(repo_url: &str, target: Option<PathBuf>) -> Result<PathBuf> {
    let target = target.unwrap_or_else(|| PathBuf::from(repo_dir_name(repo_url)));
    if target.exists() {
        anyhow::bail!(
            "target directory {} already exists — remove it first or pass --output",
            target.display()
        );
    }

    clone_repo(repo_url, None, &target).await?;

    let workflow = crate::commands::discover_workflow_file_in(&target)?;
    let config = oxo_flow_core::config::WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    eprintln!(
        "{} Cloned into {} (workflow: {}, {} rules)",
        "✓".green().bold(),
        target.display(),
        workflow.display(),
        config.rules.len()
    );
    eprintln!("  {}", "Run with:".bold().cyan());
    eprintln!("    oxo-flow run {}", workflow.display());
    eprintln!("  (Data belongs outside the clone — use --workdir / --arg overrides.)");
    Ok(workflow)
}

// ── Run-mode repository sources (nextflow-style `oxo-flow run <repo>`) ──────

/// A run-mode source: a git repository to clone and execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunSource {
    Repo {
        url: String,
        git_ref: Option<String>,
    },
}

/// Classify a `run` workflow argument as a repository URL.
///
/// NOTE: for `run`, `@ref` selects a git branch/tag — unlike `pull`, it
/// never means a GitHub Release asset. Run executes source, not artifacts.
pub(crate) fn classify_run_source(text: &str) -> Option<RunSource> {
    if let Some(spec) = text.strip_prefix("gh:") {
        if spec.is_empty() {
            return None;
        }
        let (repo, git_ref) = match spec.split_once('@') {
            Some((repo, git_ref)) => (repo, Some(git_ref.to_string())),
            None => (spec, None),
        };
        return Some(RunSource::Repo {
            url: format!("https://github.com/{repo}.git"),
            git_ref,
        });
    }
    if (text.starts_with("https://") || text.starts_with("http://")) && text.ends_with(".git") {
        return Some(RunSource::Repo {
            url: text.to_string(),
            git_ref: None,
        });
    }
    if let Some(local) = text.strip_prefix("file://")
        && Path::new(local).is_dir()
    {
        return Some(RunSource::Repo {
            url: local.to_string(),
            git_ref: None,
        });
    }
    None
}

/// Cache directory name for a repo checkout (git refs are sanitized).
pub(crate) fn repo_cache_name(repo_url: &str, git_ref: Option<&str>) -> String {
    let base = repo_dir_name(repo_url);
    match git_ref {
        Some(r) if !r.is_empty() => format!("{base}-{}", r.replace(['/', '\\'], "-")),
        _ => base,
    }
}

/// Cache directory for a repo checkout: `<cwd>/.oxo-flow/repos/<name>`.
pub(crate) fn repo_cache_dir(repo_url: &str, git_ref: Option<&str>) -> Result<PathBuf> {
    Ok(std::env::current_dir()
        .context("cannot determine current directory")?
        .join(".oxo-flow")
        .join("repos")
        .join(repo_cache_name(repo_url, git_ref)))
}

/// Clone (or reuse) a repository checkout and return the discovered workflow.
///
/// Existing cache directories are reused as-is — delete the directory to
/// force a fresh clone.
pub(crate) async fn checkout_repo_workflow(
    repo_url: &str,
    git_ref: Option<&str>,
    cache_dir: &Path,
) -> Result<PathBuf> {
    if cache_dir.exists() {
        eprintln!(
            "{} reusing cached checkout at {}",
            "↻".cyan(),
            cache_dir.display()
        );
    } else {
        clone_repo(repo_url, git_ref, cache_dir).await?;
    }
    let workflow = crate::commands::discover_workflow_file_in(cache_dir)?;
    let config = oxo_flow_core::config::WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;
    eprintln!(
        "{} running workflow from {} ({}, {} rules)",
        "→".cyan().bold(),
        workflow.display(),
        config.workflow.name,
        config.rules.len()
    );
    Ok(workflow)
}

/// Download a bundle (http / GitHub release / local file) and verify it.
async fn pull_bundle(
    source: &PullSource,
    original_url: &str,
    output: Option<PathBuf>,
) -> Result<()> {
    let bundle_path = if let Some(out) = output {
        out
    } else {
        // Derive a safe output name from the URL: the last path segment
        // with any archive extension stripped, `@` replaced (gh: tags),
        // then re-suffixed with `.tar.zst`.
        let mut name = original_url
            .rsplit('/')
            .next()
            .unwrap_or("bundle")
            .trim_end_matches(".tar.zst")
            .trim_end_matches(".tar.gz")
            .replace('@', "-");
        if name.is_empty() {
            name = "bundle".to_string();
        }
        PathBuf::from(format!("{name}.tar.zst"))
    };

    eprintln!("{} Pulling bundle from {}", "→".cyan().bold(), original_url);

    let data = match source {
        PullSource::BundleGithub { .. } => pull_github_release(original_url).await?,
        PullSource::BundleHttp(u) => pull_http(u).await?,
        PullSource::FileLocal(path) => {
            std::fs::read(path).with_context(|| format!("failed to read local bundle: {path}"))?
        }
        PullSource::RepoGit { .. } => unreachable!("repo mode handled by caller"),
    };

    std::fs::write(&bundle_path, &data)
        .with_context(|| format!("failed to write bundle: {}", bundle_path.display()))?;

    // Verify the downloaded bundle
    eprintln!("{} Verifying bundle integrity...", "→".cyan().bold());
    let (_wf, _dir) = super::bundle::extract_and_verify_bundle(&bundle_path)?;

    let size = data.len();
    let size_str = if size > 1_048_576 {
        format!("{:.1} MB", size as f64 / 1_048_576.0)
    } else if size > 1_024 {
        format!("{:.1} KB", size as f64 / 1_024.0)
    } else {
        format!("{} B", size)
    };

    eprintln!(
        "{} Pulled and verified {} ({})",
        "✓".green().bold(),
        bundle_path.display(),
        size_str
    );
    eprintln!(
        "  Run with: oxo-flow run --bundle {}",
        bundle_path.display()
    );

    Ok(())
}

/// Download a GitHub release asset.
///
/// Format: `gh:owner/repo@tag`
///
/// Resolves the GitHub release by tag, then downloads the first `.tar.zst` asset.
async fn pull_github_release(url: &str) -> Result<Vec<u8>> {
    let spec = url.trim_start_matches("gh:");
    let (repo, tag) = spec
        .split_once('@')
        .context("gh: URL must be in format 'gh:owner/repo@tag'")?;

    let api_url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    let client = reqwest::Client::new();
    let release: serde_json::Value = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "oxo-flow")
        .send()
        .await
        .context("failed to fetch GitHub release")?
        .json()
        .await
        .context("failed to parse GitHub release JSON")?;

    let assets = release["assets"]
        .as_array()
        .context("GitHub release has no assets")?;

    // Find the first bundle asset (.tar.zst or .tar.gz)
    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n.ends_with(".tar.zst") || n.ends_with(".tar.gz"))
        })
        .context("no .tar.zst or .tar.gz asset found in GitHub release")?;

    let download_url = asset["browser_download_url"]
        .as_str()
        .context("asset missing download URL")?;
    let asset_name = asset["name"].as_str().unwrap_or("bundle.tar.zst");

    eprintln!("  Downloading {}...", asset_name);
    let data = client
        .get(download_url)
        .header("User-Agent", "oxo-flow")
        .send()
        .await
        .context("failed to download release asset")?
        .bytes()
        .await
        .context("failed to read release asset")?;

    Ok(data.to_vec())
}

/// Download a bundle from an HTTP(S) URL.
async fn pull_http(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .context("failed to download bundle")?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP {} downloading bundle", response.status());
    }

    let data = response
        .bytes()
        .await
        .context("failed to read bundle data")?;
    Ok(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_candidates_prefix_github_urls_only() {
        let github = mirror_candidates("https://github.com/owner/repo.git");
        assert_eq!(github.len(), 1 + GITHUB_CLONE_MIRRORS.len());
        assert_eq!(github[0], "https://github.com/owner/repo.git");
        assert!(github[1].starts_with(GITHUB_CLONE_MIRRORS[0]));
        assert!(github[1].contains("github.com/owner/repo.git"));

        let other = mirror_candidates("https://example.com/team/repo.git");
        assert_eq!(other.len(), 1, "non-GitHub URLs get no mirror fallback");
    }

    #[test]
    fn repo_cache_name_sanitizes_ref() {
        assert_eq!(repo_cache_name("https://github.com/o/r.git", None), "r");
        assert_eq!(
            repo_cache_name("https://github.com/o/r.git", Some("v1.0.0")),
            "r-v1.0.0"
        );
        assert_eq!(
            repo_cache_name("https://github.com/o/r.git", Some("feature/x")),
            "r-feature-x"
        );
    }

    #[test]
    fn classify_run_source_variants() {
        // gh: without ref — default branch clone.
        match classify_run_source("gh:owner/repo").unwrap() {
            RunSource::Repo { url, git_ref } => {
                assert_eq!(url, "https://github.com/owner/repo.git");
                assert!(git_ref.is_none());
            }
        }
        // gh: with ref — for run this is a git branch/tag, NOT a release.
        match classify_run_source("gh:owner/repo@v1.0.0").unwrap() {
            RunSource::Repo { url, git_ref } => {
                assert_eq!(url, "https://github.com/owner/repo.git");
                assert_eq!(git_ref.as_deref(), Some("v1.0.0"));
            }
        }
        // .git https URL.
        assert!(matches!(
            classify_run_source("https://example.com/team/p.git"),
            Some(RunSource::Repo { .. })
        ));
        // A plain workflow path is not a repo.
        assert!(classify_run_source("workflows/wgs.oxoflow").is_none());
        assert!(classify_run_source("gh:").is_none());
    }

    #[tokio::test]
    async fn checkout_repo_workflow_clones_and_reuses_cache() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo_with_workflow(dir.path(), "main.oxoflow");
        let cache = dir.path().join(".oxo-flow/repos/demo");

        let wf = checkout_repo_workflow(&repo.display().to_string(), None, &cache)
            .await
            .unwrap();
        assert_eq!(wf, cache.join("main.oxoflow"));

        // Second checkout reuses the cache without touching git.
        let before = std::fs::read(cache.join("main.oxoflow")).unwrap();
        let wf2 = checkout_repo_workflow(&repo.display().to_string(), None, &cache)
            .await
            .unwrap();
        assert_eq!(wf, wf2);
        assert_eq!(before, std::fs::read(cache.join("main.oxoflow")).unwrap());
    }

    #[test]
    fn classify_gh_repo_without_tag_is_git_clone() {
        match classify_pull_source("gh:owner/pipeline") {
            PullSource::RepoGit { url, dir_name } => {
                assert_eq!(url, "https://github.com/owner/pipeline.git");
                assert_eq!(dir_name, "pipeline");
            }
            other => panic!("expected RepoGit, got {other:?}"),
        }
    }

    #[test]
    fn classify_gh_with_tag_is_release_bundle() {
        match classify_pull_source("gh:owner/pipeline@v1.0.0") {
            PullSource::BundleGithub { repo, tag } => {
                assert_eq!(repo, "owner/pipeline");
                assert_eq!(tag, "v1.0.0");
            }
            other => panic!("expected BundleGithub, got {other:?}"),
        }
    }

    #[test]
    fn classify_dot_git_https_url_is_clone() {
        match classify_pull_source("https://example.com/team/pipeline.git") {
            PullSource::RepoGit { url, dir_name } => {
                assert_eq!(url, "https://example.com/team/pipeline.git");
                assert_eq!(dir_name, "pipeline");
            }
            other => panic!("expected RepoGit, got {other:?}"),
        }
    }

    #[test]
    fn classify_archive_url_is_bundle() {
        match classify_pull_source("https://example.com/pipeline.tar.zst") {
            PullSource::BundleHttp(u) => assert_eq!(u, "https://example.com/pipeline.tar.zst"),
            other => panic!("expected BundleHttp, got {other:?}"),
        }
    }

    #[test]
    fn classify_file_url_is_local() {
        match classify_pull_source("file:///tmp/pipeline") {
            PullSource::FileLocal(p) => assert_eq!(p, "/tmp/pipeline"),
            other => panic!("expected FileLocal, got {other:?}"),
        }
    }

    /// Create a local git repo with one committed workflow file.
    fn git_repo_with_workflow(dir: &std::path::Path, wf_name: &str) -> std::path::PathBuf {
        let repo = dir.join("source-repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join(wf_name),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n",
        )
        .unwrap();
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        run(&["add", wf_name]);
        run(&["commit", "-qm", "initial"]);
        repo
    }

    #[tokio::test]
    async fn repo_pull_clones_and_discovers_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo_with_workflow(dir.path(), "main.oxoflow");
        let target = dir.path().join("clone-target");

        let workflow = pull_repo_git(&repo.display().to_string(), Some(target.clone()))
            .await
            .unwrap();
        assert_eq!(workflow, target.join("main.oxoflow"));
        assert!(target.join("main.oxoflow").exists());
    }

    #[tokio::test]
    async fn repo_pull_errors_when_no_workflow_in_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("empty-repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@e.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(repo.join("README.md"), "# hi\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-qm", "initial"]);

        let target = dir.path().join("clone-target");
        let err = pull_repo_git(&repo.display().to_string(), Some(target))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(".oxoflow"),
            "error should point at the missing workflow: {err}"
        );
    }

    #[tokio::test]
    async fn repo_pull_errors_when_target_exists() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo_with_workflow(dir.path(), "w.oxoflow");
        let target = dir.path().join("clone-target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "mine").unwrap();

        let err = pull_repo_git(&repo.display().to_string(), Some(target.clone()))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("already exists"), "{err}");
        assert!(
            target.join("keep.txt").exists(),
            "target must not be touched"
        );
    }
}
