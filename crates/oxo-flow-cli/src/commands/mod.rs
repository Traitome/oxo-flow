//! Shared helpers and utilities for CLI commands.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

/// Auto-discover workflow file in current directory.
/// Priority: main.oxoflow > *.oxoflow (alphabetically first)
pub fn discover_workflow_file() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("cannot determine current directory")?;
    discover_workflow_file_in(&cwd)
}

/// Auto-discover a workflow file in a specific directory (used by `pull`
/// after cloning a repository).
/// Priority: main.oxoflow > *.oxoflow (alphabetically first)
pub fn discover_workflow_file_in(dir: &Path) -> Result<PathBuf> {
    // Priority 1: main.oxoflow
    let main_workflow = dir.join("main.oxoflow");
    if main_workflow.exists() {
        return Ok(main_workflow);
    }

    // Priority 2: any *.oxoflow file (alphabetically first)
    let mut oxoflow_files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("cannot read directory {}", dir.display()))?
    {
        let entry = entry.context("cannot read directory entry")?;
        let path = entry.path();
        if let Some(ext) = path.extension()
            && ext == "oxoflow"
        {
            oxoflow_files.push(path);
        }
    }

    if oxoflow_files.is_empty() {
        return Err(anyhow::anyhow!(
            "no .oxoflow file found in {}.\n\
            The repository must contain a workflow file (main.oxoflow or any *.oxoflow).\n\
            Create one with `oxo-flow init <name>` or `oxo-flow template <description> -o <file>`.",
            dir.display()
        ));
    }

    // Sort alphabetically and pick first
    oxoflow_files.sort();
    Ok(oxoflow_files.into_iter().next().unwrap())
}

/// Resolve workflow path: use provided path or auto-discover.
pub fn resolve_workflow(provided: Option<PathBuf>) -> Result<PathBuf> {
    match provided {
        Some(path) => Ok(path),
        None => discover_workflow_file(),
    }
}

/// Global quiet mode flag — set by main.rs when --quiet is passed.
static QUIET_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_quiet_mode(quiet: bool) {
    QUIET_MODE.store(quiet, std::sync::atomic::Ordering::Relaxed);
}

/// Whether `--quiet` is active (suppresses non-essential console output).
pub fn is_quiet() -> bool {
    QUIET_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Serializes tests that `set_current_dir` into a temp cwd (issue #982
/// P1-1): several command test modules dance the process cwd, and a
/// parallel test mid-dance would break siblings resolving relative paths
/// against the cwd. One lock shared by every cwd-dancing test module
/// (ai_recover, run_preview); a poisoned lock (one test panicking while
/// the cwd is elsewhere) must not cascade into every sibling failing with
/// PoisonError — callers recover via `unwrap_or_else(|e| e.into_inner())`.
#[cfg(test)]
pub(crate) static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restores the process cwd on drop, panics included (issue #982 P1-1) —
/// a cwd left inside a deleted temp dir would make every other test fail
/// resolving relative paths. Test-only companion to [`CWD_LOCK`].
#[cfg(test)]
pub(crate) struct RestoreCwd(std::path::PathBuf);

#[cfg(test)]
impl Drop for RestoreCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

pub fn print_banner() {
    if QUIET_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // The banner is decoration for humans at an interactive terminal. When
    // stderr is redirected (nohup, `> log 2>&1`, CI, schedulers) it becomes
    // noise on every invocation and pollutes captured logs (issue #433);
    // version provenance stays available via the run-log header and --version.
    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        return;
    }
    eprintln!(
        "{} v{} — {}",
        "oxo-flow".bold().cyan(),
        env!("CARGO_PKG_VERSION"),
        "Rust-native bioinformatics pipeline engine".dimmed()
    );
    eprintln!("{}", env!("CARGO_PKG_REPOSITORY").cyan());
}

/// Expand placeholders in a command template.
pub fn expand_batch_template(template: &str, item: &str, nr: usize) -> String {
    let path = std::path::Path::new(item);
    let basename = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy())
        .unwrap_or_default();
    let dir = oxo_flow_core::parent_dir(path).to_string_lossy();

    template
        .replace("{}", item)
        .replace("{item}", item)
        .replace("{nr}", &nr.to_string())
        .replace("{basename}", &basename)
        .replace("{stem}", &stem)
        .replace("{ext}", &ext)
        .replace("{dir}", &dir)
}

/// Parse item lines, skipping blank lines and # comments.
pub fn parse_item_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .map(|line| line.trim().to_string())
        .collect()
}

/// Collect items from arguments, file, or stdin.
pub fn collect_batch_items(items: &[String], file: Option<&PathBuf>) -> Result<Vec<String>> {
    // Priority: file > stdin > arguments. `-f` wins silently before —
    // positional items given alongside it vanished without a trace.
    if let Some(path) = file {
        if !items.is_empty() {
            tracing::warn!(
                "{} positional item(s) ignored — -f {} takes priority",
                items.len(),
                path.display()
            );
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read items from {}", path.display()))?;
        return Ok(parse_item_lines(&content));
    }

    // Check stdin if no items provided
    if items.is_empty() {
        use std::io::{self, BufRead};
        let stdin = io::stdin();
        // A non-UTF-8 byte in piped input must ERROR, not silently
        // truncate the item list to the prefix before it (audit #671) —
        // batch then ran on fewer files than piped, all succeeding, exit 0.
        let lines: Vec<String> = stdin
            .lock()
            .lines()
            .collect::<io::Result<Vec<_>>>()
            .map_err(|e| anyhow::anyhow!("reading batch items from stdin failed: {e}"))?;
        if !lines.is_empty() {
            return Ok(parse_item_lines(&lines.join("\n")));
        }
        return Err(anyhow::anyhow!(
            "no items provided (use -f FILE, stdin, or arguments)"
        ));
    }

    // Expand globs in arguments. A malformed pattern and a pattern that
    // matches nothing are both hard errors — the old `.ok()` + empty-set
    // behavior let `batch "echo {item}" "*.zzz"` exit 0 after doing
    // nothing, breaking the #540 contract that any failure is visible
    // through the exit code.
    let mut expanded: Vec<String> = Vec::new();
    for item in items {
        if item.contains('*') || item.contains('?') || item.contains('[') {
            let paths = glob::glob(item)
                .map_err(|e| anyhow::anyhow!("invalid glob pattern '{item}': {e}"))?
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            if paths.is_empty() {
                anyhow::bail!("glob pattern '{item}' matched no files");
            }
            expanded.extend(paths);
        } else {
            expanded.push(item.clone());
        }
    }

    Ok(expanded)
}

/// Quote a string as a single POSIX shell word, escaping embedded single
/// quotes (`'` → `'\''`).
fn shell_quote_word(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Compute the hex digest of a file with a stat-first regular-file gate
/// and a streaming 64KB read loop. Shared by `compute_sha256` and
/// `verify-data`'s md5 hashing — the #714 FIFO-safety gate lives here so
/// it can never drift from one of its call sites.
fn hash_file_with<D: sha2::Digest>(path: &Path, mut hasher: D) -> Result<String> {
    use std::io::Read;

    // Stat-first regular-file gate (issue #714, pattern from #695/#706):
    // File::open on a writer-less FIFO blocks forever.
    let meta = std::fs::metadata(path)
        .with_context(|| format!("failed to stat for checksum: {}", path.display()))?;
    if !meta.is_file() {
        anyhow::bail!("cannot checksum {}: not a regular file", path.display());
    }

    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open for checksum: {}", path.display()))?;
    let mut reader = std::io::BufReader::with_capacity(65536, file);
    let mut buf = [0u8; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Compute SHA-256 checksum of a file (streaming, 64KB buffer).
pub(crate) fn compute_sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    Ok(format!("sha256:{}", hash_file_with(path, Sha256::new())?))
}

/// Wrap command with environment activation.
///
/// The conda arms reuse the engine's shared conda-family builder
/// (`CondaBackend::wrap_command_with_opts`, issue #1155 / ledger WF-05):
/// `batch -e "conda: <spec>"` must target the SAME environment `oxo-flow run`
/// creates — `<yaml name|stem>-<hash8>` for file-backed specs — and run the
/// whole item inside that env's shell. The previous hand-derived
/// `conda run --no-banner -n <file stem>` copy targeted an env the engine
/// never creates and let shell operators escape the environment.
pub fn wrap_batch_command(cmd: &str, env_spec: &str, workdir: &std::path::Path) -> Result<String> {
    // Parse environment spec: "conda: env.yaml" or "docker: image"
    if let Some((type_, spec)) = env_spec.split_once(':') {
        match type_.trim() {
            "conda" | "mamba" => {
                // Resolve a file-backed spec against the batch workdir — the
                // same anchor the executor uses (`resolve_relative_paths`).
                // The env name is derived from the file (name + content
                // hash), so resolving against the process cwd would target a
                // different environment than `oxo-flow run` (review of
                // #1155, D1).
                let spec = spec.trim();
                let resolved = workdir.join(spec);
                let resolved = if resolved.is_file() {
                    resolved
                } else {
                    std::path::PathBuf::from(spec)
                };
                let spec = resolved.to_string_lossy().to_string();
                if type_.trim() == "mamba" {
                    oxo_flow_core::environment::MambaBackend::new()
                        .wrap_command_with_opts(cmd, &spec, None)
                        .context("failed to build the mamba environment wrapper for batch")
                } else {
                    oxo_flow_core::environment::CondaBackend
                        .wrap_command_with_opts(cmd, &spec, None)
                        .context("failed to build the conda environment wrapper for batch")
                }
            }
            t if batch_env_type_is_container(t) => {
                // Each whitespace-separated token of the spec is passed as one
                // quoted argument so characters like `;` or `$(` in the TOML
                // cannot break out of the wrapper into the surrounding shell.
                let spec_args = spec
                    .split_whitespace()
                    .map(shell_quote_word)
                    .collect::<Vec<_>>()
                    .join(" ");
                // The command itself is embedded inside a single-quoted `sh -c`
                // script; escape quotes so commands containing `'` (e.g. awk
                // programs) survive and cannot terminate the quoting early.
                let quoted_cmd = shell_quote_word(cmd);
                Ok(match type_.trim() {
                    "docker" => format!("docker run --rm {spec_args} sh -c {quoted_cmd}"),
                    _ => format!("singularity exec {spec_args} sh -c {quoted_cmd}"),
                })
            }
            other => Err(anyhow::anyhow!(
                "batch environment kind '{other}' is not supported — use `conda:`/`mamba:` \
                 (shared conda-family lifecycle), `docker:`, or `singularity:`; refusing to \
                 run the item on the host PATH"
            )),
        }
    } else {
        // Legacy form: the bare argument is an environment NAME, kept
        // verbatim. Routing it through the spec-name derivation would
        // truncate a legal dotted conda name (`r-4.4` -> `r-4`) and target a
        // different env (review of #1155, D2).
        Ok(oxo_flow_core::environment::conda_wrap_named_env(
            "conda",
            cmd,
            env_spec.trim(),
        ))
    }
}

/// Whether a batch environment TYPE (the trimmed `kind:` prefix of the
/// `-e` spec) selects a local container runtime the engine invokes as a
/// host binary. The ONE decision shared by `wrap_batch_command`'s render
/// arm (its guard) and [`batch_env_is_container`] — a container kind added
/// here is admitted by both, never mirrored (invariant "Don't mirror
/// semantics — share them"; issue #1173/#1233).
fn batch_env_type_is_container(type_: &str) -> bool {
    matches!(type_, "docker" | "singularity")
}

/// Whether a `batch` environment spec selects a local container runtime
/// (`docker:`/`singularity:`) that the engine invokes as a host binary —
/// the same admission test the rule path uses
/// (`EnvironmentSpec::is_container_runtime`, issue #1173/#1233). The spec is
/// the raw `-e` string, so this reads its `kind:` prefix through the shared
/// [`batch_env_type_is_container`] decision that `wrap_batch_command`'s
/// render arm guards on.
pub fn batch_env_is_container(env_spec: &str) -> bool {
    env_spec
        .split_once(':')
        .is_some_and(|(type_, _)| batch_env_type_is_container(type_.trim()))
}

/// Run a single batch command synchronously.
///
/// RB-15 (#1233): batch shells are a shell spawn site just like a rule
/// shell, so they run through the SAME `apply_rule_env_allowlist` policy —
/// without it, a batch command inherits the operator's full ambient
/// environment (including AI tokens the engine hides from rules). The
/// `[engine] tmp_dir`-derived scratch vars are re-admitted when a batch
/// invocation resolved a TMPDIR (see `OXO_FLOW_TMPDIR`).
///
/// `container_daemon` must be `true` for a `docker:`/`singularity:` item
/// (`run_batch_command` cannot see the `-e` spec, so the caller passes
/// `batch_env_is_container(...)`): the rendered wrapper runs `docker run` /
/// `singularity exec` from THIS shell, so the daemon variables
/// (`DOCKER_HOST`, `DOCKER_TLS_VERIFY`, …) must be re-admitted exactly as
/// the rule path does (issue #1173). A non-container item keeps the bare
/// allowlist.
pub fn run_batch_command(cmd: &str, workdir: &Path, container_daemon: bool) -> Result<i32> {
    use std::process::Command;

    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd).current_dir(workdir);

    let mut envs = std::collections::HashMap::new();
    oxo_flow_core::executor::insert_tmpdir_vars(&mut envs, batch_tmp_dir().as_deref());
    oxo_flow_core::executor::apply_rule_env_allowlist(&mut command, &envs, container_daemon);

    let output = command
        .output()
        .with_context(|| format!("failed to execute: {}", cmd))?;

    Ok(output.status.code().unwrap_or(-1))
}

/// Scratch dir for `batch` shells: `OXO_FLOW_TMPDIR` when set (the same
/// per-invocation override `run` honors), else `None`. `batch` has no
/// `[engine]` section to consult, so the env knob is the only source.
fn batch_tmp_dir() -> Option<PathBuf> {
    let raw = std::env::var("OXO_FLOW_TMPDIR").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

pub mod agent_docs;
pub mod ai_check;
pub mod ai_explain;
pub mod ai_recover;
pub mod ai_runtime;
pub mod ai_session;
pub mod ai_status;
pub mod ai_template;
pub mod attestation;
pub mod batch;
pub mod bundle;
pub mod bundle_signing;
pub mod clean;
pub mod cluster;
pub mod completions;
pub mod config_comments;
pub mod diagnostics;
pub mod export_dsl;
pub mod import;
pub mod info;
pub mod infra;
pub mod json_error;
pub mod knowledge;
pub mod output;
pub mod project;
pub mod provenance;
pub mod publish;
pub mod pull;
pub mod quality;
pub mod run;
pub mod run_cluster;
pub mod run_preview;
pub mod runs_gc;
pub mod samples;
pub mod verify_data;
pub mod wait;
pub mod web;
pub mod workdir_sources;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_wrapper_escapes_single_quotes() {
        let wrapped = wrap_batch_command(
            "awk '{print $1}'",
            "docker: biocontainers/samtools",
            std::path::Path::new("."),
        )
        .unwrap();
        assert!(wrapped.starts_with("docker run --rm 'biocontainers/samtools' sh -c "));
        // The command must survive as one quoted argument.
        assert!(wrapped.ends_with(r#"'awk '\''{print $1}'\'''"#));
        assert_eq!(wrapped.matches("sh -c").count(), 1);
    }

    #[test]
    fn singularity_wrapper_escapes_injection() {
        let wrapped = wrap_batch_command(
            "echo ok'; rm -rf /tmp/x",
            "singularity: image.sif",
            std::path::Path::new("."),
        )
        .unwrap();
        // Exact wrapping: the injected quote is escaped (`'\''`) and the
        // trailing `; rm ...` stays inside the still-open quote, so the inner
        // `sh -c` receives it as part of the container's command string rather
        // than as outer-shell syntax.
        assert_eq!(
            wrapped,
            r#"singularity exec 'image.sif' sh -c 'echo ok'\''; rm -rf /tmp/x'"#
        );
    }

    #[test]
    fn docker_spec_tokens_are_individually_quoted() {
        let wrapped = wrap_batch_command(
            "true",
            "docker: -v /a:/b --gpus all",
            std::path::Path::new("."),
        )
        .unwrap();
        // Every token quoted separately keeps multi-arg semantics while
        // neutralizing metacharacters inside tokens.
        assert!(wrapped.contains("'-v' '/a:/b' '--gpus' 'all'"));
    }

    /// Issue #1155 (WF-05): `batch -e "conda: ..."` must be the engine's
    /// shared conda-family wrap, not a hand-derived copy. Before this fix
    /// the batch arm built `conda run --no-banner -n <file stem>`, targeting
    /// an env `oxo-flow run` never creates for a file-backed spec and letting
    /// shell operators escape the environment.
    #[test]
    fn conda_wrapper_reuses_the_engine_conda_family_command() {
        let engine = |cmd: &str, spec: &str| {
            oxo_flow_core::environment::CondaBackend
                .wrap_command_with_opts(cmd, spec, None)
                .unwrap()
        };

        // File-backed spec: ONE derived name (`<yaml name>-<hash8>`) — the
        // exact env the engine creates — and the item runs inside its shell.
        let dir = tempfile::tempdir().unwrap();
        let yaml = dir.path().join("tools.yaml");
        std::fs::write(&yaml, "name: tools\ndependencies:\n  - python=3.11\n").unwrap();
        let spec = yaml.to_str().unwrap();
        let wrapped = wrap_batch_command(
            "python x.py",
            &format!("conda: {spec}"),
            std::path::Path::new("."),
        )
        .unwrap();
        assert_eq!(wrapped, engine("python x.py", spec));
        assert!(wrapped.contains("-n tools-"), "{wrapped}");
        assert!(
            wrapped.contains("bash -c 'export PATH=\"$CONDA_PREFIX/bin:$PATH\"; python x.py'"),
            "the whole item must run inside the env shell: {wrapped}"
        );

        // File-less spec: the plain validated name (no hash suffix), again
        // through the shared builder.
        let named =
            wrap_batch_command("python x.py", "conda: bwa_env", std::path::Path::new(".")).unwrap();
        assert_eq!(named, engine("python x.py", "bwa_env"));
        assert!(named.contains("-n bwa_env "), "{named}");

        // The no-colon legacy spelling is an environment NAME, kept
        // verbatim: a dotted name must not be truncated into a file stem
        // (review of #1155, D2).
        let legacy = wrap_batch_command("true", "r-4.4", std::path::Path::new(".")).unwrap();
        assert!(legacy.contains("-n 'r-4.4'"), "{legacy}");
        let legacy_named =
            wrap_batch_command("true", "bwa_env", std::path::Path::new(".")).unwrap();
        // Same run-shell shape; the legacy path quotes the name verbatim.
        assert_eq!(
            legacy_named.replace("-n 'bwa_env'", "-n bwa_env"),
            engine("true", "bwa_env")
        );
    }

    /// Issue #1155 review D1: a relative spec must resolve against the
    /// batch WORKDIR, not the process cwd — the env name is derived from the
    /// file, so the wrong anchor targets a different environment.
    #[test]
    fn batch_conda_spec_resolves_against_the_batch_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let envs = dir.path().join("envs");
        std::fs::create_dir_all(&envs).unwrap();
        let yaml = envs.join("tools.yaml");
        std::fs::write(&yaml, "name: tools\ndependencies:\n  - python=3.11\n").unwrap();

        // The process cwd is NOT the workdir here.
        let wrapped =
            wrap_batch_command("python x.py", "conda: envs/tools.yaml", dir.path()).unwrap();
        let engine = oxo_flow_core::environment::CondaBackend
            .wrap_command_with_opts("python x.py", yaml.to_str().unwrap(), None)
            .unwrap();
        assert_eq!(wrapped, engine);
        assert!(wrapped.contains("-n tools-"), "{wrapped}");
    }

    /// Issue #1155 review D3: an unsupported environment kind must fail
    /// closed instead of silently running the item on the host PATH.
    #[test]
    fn batch_rejects_unsupported_environment_kinds() {
        for spec in ["pixi: pixi.toml", "venv: requirements.txt", "modules: gcc"] {
            let err = wrap_batch_command("true", spec, std::path::Path::new("."))
                .expect_err("unsupported kind must not silently unwrap");
            let msg = format!("{err}");
            assert!(msg.contains("not supported"), "{msg}");
        }
    }
}
