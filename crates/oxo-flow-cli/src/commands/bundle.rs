//! Bundle consumption — extract, verify, and execute published bundles.
//!
//! A bundle is a `.tar.zst` archive produced by `oxo-flow publish` containing
//! a workflow file, environment specs, scripts, and a checksum-verified manifest.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Extract a `.tar.zst` bundle, verify all file checksums against the
/// manifest, and return `(workflow_path, extraction_dir)`.
///
/// If verification fails for any file, an error is returned and the
/// extracted directory is cleaned up.
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

    // Open and decompress
    let file = std::fs::File::open(&bundle_abs)
        .with_context(|| format!("failed to open bundle: {}", bundle_abs.display()))?;
    let decoder =
        zstd::stream::read::Decoder::new(file).context("failed to decompress bundle (zstd)")?;
    let mut archive = tar::Archive::new(decoder);

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
            eprintln!(
                "{}",
                "Bundle resource requirements:".bold().underline()
            );
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

use colored::Colorize;
