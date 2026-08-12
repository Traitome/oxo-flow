//! Bundle consumption — extract, verify, and execute published bundles.
//!
//! A bundle is a compressed tar archive produced by `oxo-flow publish` containing
//! a workflow file, environment specs, scripts, and a checksum-verified manifest.
//!
//! Supported formats: `.tar.zst` (default), `.tar.gz`.

use anyhow::{Context, Result};
use colored::Colorize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Supported bundle archive formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleFormat {
    TarZst,
    TarGz,
}

impl BundleFormat {
    pub fn extension(&self) -> &str {
        match self {
            Self::TarZst => "tar.zst",
            Self::TarGz => "tar.gz",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let s = path.to_string_lossy();
        if s.ends_with(".tar.zst") {
            Some(Self::TarZst)
        } else if s.ends_with(".tar.gz") || s.ends_with(".tgz") {
            Some(Self::TarGz)
        } else {
            None
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "tar.zst" | "zst" => Ok(Self::TarZst),
            "tar.gz" | "gz" | "tgz" => Ok(Self::TarGz),
            _ => anyhow::bail!("unsupported format '{}'. Use 'tar.zst' or 'tar.gz'", s),
        }
    }
}

/// Extract a bundle, verify all file checksums against the manifest,
/// and return `(workflow_path, extraction_dir)`. Auto-detects format
/// from file extension.
pub fn extract_and_verify_bundle(bundle_path: &Path) -> Result<(PathBuf, PathBuf)> {
    let bundle_abs = std::path::absolute(bundle_path)
        .with_context(|| format!("failed to resolve bundle path: {}", bundle_path.display()))?;

    // Extract to a freshly created temp directory.
    //
    // The previous `temp_dir()/oxo-bundle-<pid>` path was predictable and created
    // with `create_dir_all`, which succeeds against a directory that already
    // exists — on a shared machine another user can pre-create it, and PIDs are
    // reused. `tempdir()` creates a uniquely named directory with 0700
    // permissions, failing if it cannot create it exclusively.
    //
    // The directory is deliberately kept rather than dropped: it becomes the
    // working directory for the run and holds the workflow's output files, so it
    // has to outlive this function.
    let extract_dir = tempfile::Builder::new()
        .prefix("oxo-bundle-")
        .tempdir()
        .context("failed to create temporary directory for bundle extraction")?
        .keep();

    // Auto-detect format from extension and open archive
    let format = BundleFormat::from_path(&bundle_abs).unwrap_or(BundleFormat::TarZst); // default fallback
    let file = std::fs::File::open(&bundle_abs)
        .with_context(|| format!("failed to open bundle: {}", bundle_abs.display()))?;
    let reader: Box<dyn Read> = match format {
        BundleFormat::TarZst => Box::new(
            zstd::stream::read::Decoder::new(file).context("failed to decompress bundle (zstd)")?,
        ),
        BundleFormat::TarGz => Box::new(flate2::read::GzDecoder::new(file)),
    };
    let mut archive = tar::Archive::new(reader);

    // Extract all files
    archive
        .unpack(&extract_dir)
        .context("failed to extract bundle")?;

    // Read and verify manifest
    let manifest_path = extract_dir.join("manifest.json");
    let manifest_json = std::fs::read_to_string(&manifest_path)
        .context("bundle is missing manifest.json — not a valid oxo-flow bundle")?;
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_json).context("failed to parse manifest.json")?;

    let format = manifest["format"].as_str().unwrap_or("unknown");
    if format != "oxoflow-bundle-v1" {
        anyhow::bail!(
            "unsupported bundle format '{}' — expected 'oxoflow-bundle-v1'",
            format
        );
    }

    let entrypoint = manifest["entrypoint"]
        .as_str()
        .context("manifest missing 'entrypoint' field")?;
    let workflow_path = extract_dir.join(entrypoint);
    if !workflow_path.exists() {
        anyhow::bail!(
            "bundle entrypoint '{}' not found in archive",
            workflow_path.display()
        );
    }

    // Display resource requirements from manifest (if present)
    if let Some(resources) = manifest.get("resources") {
        if let Some(recommendations) = resources.get("recommendations") {
            eprintln!("{}", "Bundle resource requirements:".bold().underline());
            if let Some(t) = recommendations["min_threads"].as_u64() {
                eprintln!("  Min threads: {}", t.to_string().cyan());
            }
            if let Some(m) = recommendations["min_memory_mb"].as_u64() {
                let gb = m as f64 / 1024.0;
                eprintln!("  Min memory:  {} ({:.1} GB)", m.to_string().cyan(), gb);
            }
            if let Some(g) = recommendations["min_gpu"].as_u64()
                && g > 0
            {
                eprintln!("  Min GPU:     {}", g.to_string().cyan());
            }
        }
        if let Some(rules) = resources.get("rules").and_then(|r| r.as_array()) {
            eprintln!(
                "  {} rules with resource declarations",
                rules.len().to_string().cyan()
            );
        }
        eprintln!();
    }

    // Verify checksums
    let files = manifest["files"]
        .as_array()
        .context("manifest missing 'files' array")?;
    let mut verified = 0usize;
    for file_entry in files {
        let path = file_entry["path"]
            .as_str()
            .context("file entry missing 'path'")?;
        let expected_sha = file_entry["sha256"]
            .as_str()
            .context("file entry missing 'sha256'")?;

        let file_path = extract_dir.join(path);
        if !file_path.exists() {
            anyhow::bail!(
                "file '{}' declared in manifest but missing from archive",
                path
            );
        }

        let actual_sha = compute_sha256(&file_path)?;
        if actual_sha != expected_sha {
            // Clean up on mismatch
            let _ = std::fs::remove_dir_all(&extract_dir);
            anyhow::bail!(
                "checksum mismatch for '{}':\n  expected: {}\n  actual:   {}\nBundle verification failed.",
                path,
                expected_sha,
                actual_sha
            );
        }
        verified += 1;
    }

    eprintln!(
        "{} Bundle verified: {}/{} files OK",
        "✓".green(),
        verified,
        files.len()
    );

    Ok((workflow_path, extract_dir))
}

/// Compute SHA-256 checksum of a file (streaming, 64KB buffer).
fn compute_sha256(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(65536, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Find manifest.json in an extracted bundle directory.
pub fn find_manifest_in_dir(dir: &Path) -> Result<PathBuf> {
    let manifest_path = dir.join("manifest.json");
    if manifest_path.exists() {
        Ok(manifest_path)
    } else {
        anyhow::bail!(
            "manifest.json not found in extracted bundle: {}",
            dir.display()
        )
    }
}
