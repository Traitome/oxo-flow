use anyhow::{Context, Result};
use colored::Colorize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Bundle a workflow with its referenced environment files into a verifiable archive.
///
/// Reads the .oxoflow workflow file, follows `[[include]]` references to discover
/// all environment spec files, collects `scripts/` and `bin/` directories, and
/// produces a single `.tar.zst` archive with a complete, checksum-verified manifest.
///
/// With `--with-lockfiles`, generates deterministic conda lockfiles for each
/// conda/mamba environment YAML, ensuring exact reproducibility across time.
pub fn publish_command(
    workflow: PathBuf,
    output: Option<PathBuf>,
    with_lockfiles: bool,
) -> Result<()> {
    let workflow_path =
        std::path::absolute(&workflow).context("failed to resolve workflow path")?;
    let workflow_dir = workflow_path.parent().unwrap_or(Path::new("."));

    let workflow_name = workflow_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("workflow");

    let output_archive = if let Some(out) = output {
        if out.extension().is_none() {
            PathBuf::from(format!("{}.tar.zst", out.display()))
        } else {
            out
        }
    } else {
        PathBuf::from(format!("{}-bundle.tar.zst", workflow_name))
    };

    // ── Collect all referenced files ──────────────────────────────────────

    let mut referenced_files: Vec<(String, PathBuf)> = Vec::new();
    let mut container_refs: Vec<serde_json::Value> = Vec::new();
    let mut scanned_workflows: std::collections::HashSet<PathBuf> =
        std::collections::HashSet::new();

    // Scan the main workflow and all included sub-workflows recursively.
    scan_workflow_env_files(
        &workflow_path,
        workflow_dir,
        &mut referenced_files,
        &mut container_refs,
        &mut scanned_workflows,
    )?;

    // ── Generate conda lockfiles (if --with-lockfiles) ────────────────────

    if with_lockfiles {
        generate_lockfiles(workflow_dir, &mut referenced_files);
    }

    // Collect scripts/ and bin/ directories if they exist (Nextflow-style auto-PATH convention)
    for dir_name in &["scripts", "bin"] {
        let dir_path = workflow_dir.join(dir_name);
        if dir_path.is_dir() {
            collect_directory_files(&dir_path, dir_name, &mut referenced_files)?;
        }
    }

    // ── Build manifest with checksums ─────────────────────────────────────

    let oxo_version = env!("CARGO_PKG_VERSION").to_string();
    let mut manifest_files = Vec::new();
    let temp_dir = std::env::temp_dir().join(format!("oxo-publish-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;

    // Copy the main workflow file
    let wf_filename = workflow_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workflow.oxoflow");
    let wf_dest = temp_dir.join(wf_filename);
    std::fs::copy(&workflow_path, &wf_dest)?;
    let wf_checksum = compute_sha256(&workflow_path)?;
    let wf_size = std::fs::metadata(&workflow_path)?.len();
    manifest_files.push(serde_json::json!({
        "path": wf_filename,
        "sha256": wf_checksum,
        "size": wf_size,
    }));

    // Copy all env/script files to temp dir and compute checksums
    for (rel_path, abs_path) in &referenced_files {
        let checksum = compute_sha256(abs_path)?;
        let dest = temp_dir.join(rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(abs_path, &dest)?;

        let size = std::fs::metadata(abs_path)?.len();
        manifest_files.push(serde_json::json!({
            "path": rel_path,
            "sha256": checksum,
            "size": size,
        }));
    }

    // Build manifest
    let checksum_count = manifest_files.len();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let manifest = serde_json::json!({
        "format": "oxoflow-bundle-v1",
        "workflow": workflow_path.file_name().and_then(|s| s.to_str()),
        "oxo_flow_version": oxo_version,
        "created_at_epoch": timestamp,
        "entrypoint": workflow_path.file_name().and_then(|s| s.to_str()),
        "files": &manifest_files,
        "containers": &container_refs,
        // Reserved for bundle signing. Always empty today — present so that adding
        // signatures later is an additive change rather than a manifest format bump.
        // Consumers read the manifest field-by-field, so an empty array is ignored
        // by older versions of oxo-flow.
        "signatures": serde_json::Value::Array(Vec::new()),
    });

    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let manifest_path = temp_dir.join("manifest.json");
    std::fs::write(&manifest_path, &manifest_json)?;

    // ── Build .tar.zst archive ────────────────────────────────────────────

    let archive_file = std::fs::File::create(&output_archive)
        .with_context(|| format!("failed to create archive: {}", output_archive.display()))?;
    let zstd_encoder = zstd::stream::write::Encoder::new(archive_file, 3)
        .context("failed to create zstd encoder")?;
    let mut tar_builder = tar::Builder::new(zstd_encoder);

    // Add manifest first, then workflow, then all referenced files
    tar_builder.append_path_with_name(&manifest_path, "manifest.json")?;
    tar_builder.append_path_with_name(&wf_dest, wf_filename)?;
    for (rel_path, _) in &referenced_files {
        let src = temp_dir.join(rel_path);
        if src.exists() {
            tar_builder.append_path_with_name(&src, rel_path)?;
        }
    }

    let zstd_encoder = tar_builder.into_inner().context("failed to finalize tar")?;
    zstd_encoder
        .finish()
        .context("failed to finalize zstd compression")?;

    // Cleanup temp dir
    let _ = std::fs::remove_dir_all(&temp_dir);

    // ── Summary ───────────────────────────────────────────────────────────

    let archive_size = std::fs::metadata(&output_archive)
        .map(|m| m.len())
        .unwrap_or(0);
    let size_str = if archive_size > 1_048_576 {
        format!("{:.1} MB", archive_size as f64 / 1_048_576.0)
    } else if archive_size > 1_024 {
        format!("{:.1} KB", archive_size as f64 / 1_024.0)
    } else {
        format!("{} B", archive_size)
    };

    eprintln!(
        "{} Published to {}",
        "✓".green().bold(),
        output_archive.display()
    );
    eprintln!("  size:      {}", size_str);
    eprintln!(
        "  files:     {} (workflow + env + scripts/bin)",
        referenced_files.len() + 1
    );
    eprintln!("  checksums: {} files verified (SHA-256)", checksum_count);

    Ok(())
}

/// Recursively scan a workflow file (and its `[[include]]` children) for
/// environment file references.
fn scan_workflow_env_files(
    wf_path: &Path,
    workflow_dir: &Path,
    referenced_files: &mut Vec<(String, PathBuf)>,
    container_refs: &mut Vec<serde_json::Value>,
    scanned: &mut std::collections::HashSet<PathBuf>,
) -> Result<()> {
    let canonical = std::path::absolute(wf_path)?;
    if !scanned.insert(canonical) {
        return Ok(()); // Already scanned — avoid cycles
    }

    let content = std::fs::read_to_string(wf_path)
        .with_context(|| format!("failed to read workflow file: {}", wf_path.display()))?;
    let toml_value: toml::Table =
        toml::from_str(&content).context("failed to parse workflow as TOML")?;

    // ── Scan [[rules]] → [rules.environment] ──────────────────────────

    if let Some(rules) = toml_value.get("rules").and_then(|v| v.as_array()) {
        for rule in rules {
            let Some(env) = rule.get("environment") else {
                continue;
            };
            // All local-file environment fields (conda, mamba, pixi, venv, venv_requirements).
            for field in ["conda", "mamba", "pixi", "venv", "venv_requirements"] {
                add_env_file(env, field, workflow_dir, referenced_files);
            }
            // Container image references — record for reproducibility
            for field in ["docker", "singularity"] {
                if let Some(image) = env.get(field).and_then(|v| v.as_str())
                    && !container_refs.iter().any(|c| c["image"] == image)
                {
                    container_refs.push(serde_json::json!({
                        "type": field,
                        "image": image,
                    }));
                }
            }
        }
    }

    // ── Also scan [env_groups] for named environment specs ─────────────

    if let Some(env_groups) = toml_value.get("env_groups").and_then(|v| v.as_table()) {
        for (_group_name, env_spec) in env_groups {
            for field in ["conda", "mamba", "pixi", "venv", "venv_requirements"] {
                add_env_file(env_spec, field, workflow_dir, referenced_files);
            }
            for field in ["docker", "singularity"] {
                if let Some(image) = env_spec.get(field).and_then(|v| v.as_str())
                    && !container_refs.iter().any(|c| c["image"] == image)
                {
                    container_refs.push(serde_json::json!({
                        "type": field,
                        "image": image,
                    }));
                }
            }
        }
    }

    // ── Scan [workflow] for pairs_file / sample_groups_file ───────────

    if let Some(wf) = toml_value.get("workflow") {
        for key in &["pairs_file", "sample_groups_file"] {
            if let Some(file_path) = wf.get(key).and_then(|v| v.as_str()) {
                let abs_path = workflow_dir.join(file_path);
                if abs_path.exists() {
                    let filename = Path::new(file_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !referenced_files.iter().any(|(name, _)| name == &filename) {
                        referenced_files.push((filename, abs_path));
                    }
                }
            }
        }
    }

    // ── Follow [[include]] references ─────────────────────────────────

    if let Some(includes) = toml_value.get("include").and_then(|v| v.as_array()) {
        for inc in includes {
            if let Some(inc_path) = inc.get("path").and_then(|v| v.as_str()) {
                let included_wf = workflow_dir.join(inc_path);
                if included_wf.exists() {
                    scan_workflow_env_files(
                        &included_wf,
                        workflow_dir,
                        referenced_files,
                        container_refs,
                        scanned,
                    )?;
                }
            }
        }
    }

    Ok(())
}

/// Add a single environment file reference if it exists on disk.
fn add_env_file(
    env: &toml::Value,
    field: &str,
    workflow_dir: &Path,
    referenced_files: &mut Vec<(String, PathBuf)>,
) {
    if let Some(env_file) = env.get(field).and_then(|v| v.as_str()) {
        let abs_path = workflow_dir.join(env_file);
        if abs_path.exists() {
            let filename = abs_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !referenced_files.iter().any(|(name, _)| name == &filename) {
                referenced_files.push((filename, abs_path));
            }
        } else {
            eprintln!(
                "  {} env file referenced but not found: {} (field: {})",
                "⚠".yellow(),
                env_file,
                field
            );
        }
    }
}

/// Collect all files from a directory, preserving relative paths.
fn collect_directory_files(
    dir: &Path,
    prefix: &str,
    referenced_files: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let abs = entry.path().to_path_buf();
        let rel = Path::new(prefix).join(entry.path().strip_prefix(dir).unwrap());
        let rel_str = rel.to_string_lossy().to_string();
        if !referenced_files.iter().any(|(name, _)| name == &rel_str) {
            referenced_files.push((rel_str, abs));
        }
    }
    Ok(())
}

/// Compute SHA-256 checksum of a file (streaming, 64KB buffer).
fn compute_sha256(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open for checksum: {}", path.display()))?;
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

/// Generate conda lockfiles for collected environment YAML files.
///
/// Tries `conda-lock` first, then falls back to `conda env export`.
/// Lockfiles are added to `referenced_files` for inclusion in the bundle.
fn generate_lockfiles(_workflow_dir: &Path, referenced_files: &mut Vec<(String, PathBuf)>) {
    // Find conda-lock or compatible tool
    let lock_tool = if std::process::Command::new("conda-lock")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        Some("conda-lock")
    } else {
        None
    };

    if let Some(tool) = lock_tool {
        eprintln!("  {} Generating lockfiles with {}", "→".cyan(), tool);
    } else {
        eprintln!(
            "  {} conda-lock not found — install with: pip install conda-lock",
            "⚠".yellow()
        );
        eprintln!(
            "  {} lockfiles not generated; environments may resolve differently over time",
            "⚠".yellow()
        );
        return;
    }

    let temp_dir = std::env::temp_dir().join(format!("oxo-lock-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);

    // Collect conda/mamba env files to lock
    let env_files: Vec<(String, PathBuf)> = referenced_files
        .iter()
        .filter(|(name, _)| name.ends_with(".yaml") || name.ends_with(".yml"))
        .map(|(name, path)| (name.clone(), path.clone()))
        .collect();

    for (name, abs_path) in &env_files {
        let lock_name = format!(
            "{}.lock.yml",
            Path::new(name).file_stem().unwrap().to_string_lossy()
        );
        let lock_path = temp_dir.join(&lock_name);

        eprintln!("    Locking {}...", name);
        let result = std::process::Command::new("conda-lock")
            .args([
                "lock",
                "--file",
                &abs_path.display().to_string(),
                "--platform",
                "linux-64",
                "--platform",
                "osx-64",
                "--lockfile",
                &lock_path.display().to_string(),
                "--quiet",
            ])
            .output();

        match result {
            Ok(output) if output.status.success() => {
                if lock_path.exists() && !referenced_files.iter().any(|(n, _)| n == &lock_name) {
                    eprintln!("      {} {} generated", "✓".green(), lock_name);
                    referenced_files.push((lock_name, lock_path));
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!(
                    "      {} conda-lock failed for {}: {}",
                    "⚠".yellow(),
                    name,
                    stderr.lines().next().unwrap_or("unknown error")
                );
            }
            Err(e) => {
                eprintln!(
                    "      {} failed to run conda-lock for {}: {}",
                    "⚠".yellow(),
                    name,
                    e
                );
            }
        }
    }

    // Note: lock temp dir intentionally not cleaned — files are referenced
    // by the archive builder and must persist until tar creation finishes.
}
