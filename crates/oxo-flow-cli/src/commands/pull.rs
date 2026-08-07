//! Pull — download a published oxo-flow bundle from a remote source.
//!
//! Supports three URL schemes:
//! - `https://example.com/path/to/bundle.tar.zst` — direct download
//! - `gh:owner/repo@v1.0.0` — GitHub release asset (tag → release → asset)
//! - `file:///path/to/bundle.tar.zst` — local file (copy)

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

/// Download and verify a bundle from a remote URL.
///
/// Returns the path to the downloaded (and verified) `.tar.zst` bundle.
pub async fn pull_command(url: &str, output: Option<PathBuf>) -> Result<()> {
    let bundle_path = if let Some(out) = output {
        out
    } else {
        // Derive output name from URL
        let name = url
            .rsplit('/')
            .next()
            .unwrap_or("bundle")
            .trim_end_matches(".tar.zst");
        PathBuf::from(format!("{}.tar.zst", name))
    };

    eprintln!("{} Pulling bundle from {}", "→".cyan().bold(), url);

    let data = if url.starts_with("gh:") {
        pull_github_release(url).await?
    } else if url.starts_with("https://") || url.starts_with("http://") {
        pull_http(url).await?
    } else if url.starts_with("file://") {
        let local_path = Path::new(url.trim_start_matches("file://"));
        std::fs::read(local_path)
            .with_context(|| format!("failed to read local bundle: {}", local_path.display()))?
    } else {
        anyhow::bail!("unsupported URL scheme. Use https://, gh:owner/repo@ref, or file://");
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

    // Find the first .tar.zst asset
    let asset = assets
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(".tar.zst")))
        .context("no .tar.zst asset found in GitHub release")?;

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
