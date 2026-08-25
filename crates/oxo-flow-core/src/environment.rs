//! Environment management for oxo-flow.
//!
//! Provides a trait-based abstraction for different software environment
//! managers (conda, pixi, docker, singularity, venv) and a resolver that
//! selects the appropriate backend for each rule.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::{OxoFlowError, Result};
use crate::rule::EnvironmentSpec;

/// Trait for environment backends.
///
/// Each backend (conda, docker, etc.) implements this trait to provide
/// environment detection, creation, command wrapping, and lifecycle management.
pub trait EnvironmentBackend: Send + Sync {
    /// Returns the name of this environment type.
    fn name(&self) -> &str;

    /// Check if this environment backend is available on the system.
    fn is_available(&self) -> bool;

    /// Wrap a shell command to run inside this environment.
    ///
    /// `workdir` is the executor's working directory — used by container
    /// backends for bind-mount paths. Other backends may ignore it.
    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        _resources: Option<&crate::rule::Resources>,
        workdir: &std::path::Path,
    ) -> Result<String>;

    /// Return the shell command to set up / create this environment.
    fn setup_command(&self, spec: &str) -> Result<String>;

    /// Return the shell command to tear down / remove this environment,
    /// or `None` if no cleanup is needed.
    fn teardown_command(&self, spec: &str) -> Result<Option<String>>;

    /// Return a shell command that verifies the environment is actually
    /// usable after setup, or `None` when setup success is sufficient proof.
    ///
    /// Needed because conda/mamba's create/update can exit 0 while leaving
    /// a broken env behind (interrupted transactions on loaded machines;
    /// prefixes that exist but are not complete environments — live evidence:
    /// tx-ubuntu, where `conda env update --prune` left an env with no
    /// `bin/` and the engine marked it ready on exit code alone).
    fn verify_command(&self, spec: &str) -> Result<Option<String>>;

    /// Return a cache key that uniquely identifies this environment
    /// configuration so it can be reused across rules.
    fn cache_key(&self, spec: &str) -> String;
}

/// Read a conda YAML spec and extract the `name:` field, falling back to
/// file stem. The name is interpolated into a shell command string
/// (`conda run -n <name>`), so it must be validated: an untrusted YAML
/// `name:` could otherwise break out of the quoting and run arbitrary
/// shell (issue #136). Fail fast with a clear error instead of emitting
/// a command that does not do what it says. `kind` names the backend
/// (conda/mamba) in the error.
///
/// When the spec is a readable file, the name gets a short content-hash
/// suffix (`name-<hash8>`): two workflows that ship DIFFERENT yamls with
/// the same name then build into distinct envs instead of silently
/// sharing one prefix (live evidence: rnaseq vs rnaseq-star-deseq2
/// deseq2 collision, issue #159). Same content → same name, so
/// identical specs keep deduplicating. Non-file specs (inline strings)
/// keep the plain name.
fn conda_env_name_from_spec(kind: &str, spec: &str) -> Result<String> {
    // Try reading the YAML file to extract `name:` field
    let file_content = std::fs::read(spec).ok();
    let from_yaml = file_content
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|content| {
            content.lines().find_map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("name:") || trimmed.starts_with("name :") {
                    let name = trimmed
                        .split_once(':')
                        .map(|(_, v)| v)
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    (!name.is_empty()).then(|| name.to_string())
                } else {
                    None
                }
            })
        });
    // Fall back to file stem
    let mut name = from_yaml.unwrap_or_else(|| {
        std::path::Path::new(spec)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(spec)
            .to_string()
    });
    // Content-hash suffix for file-backed specs (issue #159).
    if let Some(bytes) = file_content {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(bytes);
        let hash8: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
        name.push('-');
        name.push_str(&hash8);
    }
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(name)
    } else {
        Err(OxoFlowError::Environment {
            kind: kind.to_string(),
            message: format!(
                "invalid environment name '{name}' derived from spec '{spec}': names may contain only alphanumerics, '_' and '-'"
            ),
        })
    }
}

/// Conda environment backend.
#[derive(Debug, Default)]
pub struct CondaBackend;

impl EnvironmentBackend for CondaBackend {
    fn name(&self) -> &str {
        "conda"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("conda")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        _resources: Option<&crate::rule::Resources>,
        _workdir: &std::path::Path,
    ) -> Result<String> {
        let env_name = conda_env_name_from_spec("conda", spec)?;
        let escaped = escape_for_sh_single_quote(command);
        // `--no-capture-output` (conda >= 4.13): without it, `conda run`
        // buffers the wrapped process's stdout/stderr through its own
        // internal pipes and polls them for EOF — when the wrapped
        // command's grandchildren inherit those pipe write-ends (forked
        // helpers, gzip/pigz workers, sra-tools daemons), the poll never
        // sees EOF and the whole rule parks for 30+ minutes (live:
        // auto-sra fasterq-dump rules on tx-ubuntu; the identical command
        // ran instantly by hand). With the flag, conda forwards the child's
        // fds straight through — the engine's own pipe draining
        // (wait_with_output) is the capture mechanism.
        //
        // The wrapped bash also re-prepends $CONDA_PREFIX/bin to PATH:
        // on conda+pixi (or other toolchain-manager) hybrid boxes the
        // host PATH can precede the env's bin inside `conda run`, so
        // tools resolve to unrelated pre-existing envs (live: pinned
        // gatk 4.4.0.0 was bypassed by a user pixi env with 4.6.2.0 +
        // JDK 25 → Spark Subject.getSubject crash). The env's own bin
        // must always win, regardless of host PATH ordering.
        Ok(format!(
            "conda run --no-capture-output -n {env_name} bash -c 'export PATH=\"$CONDA_PREFIX/bin:$PATH\"; {escaped}'"
        ))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        // `-n <name>` keeps setup consistent with `wrap_command` (which runs
        // `conda run -n <name>`): the name comes from the YAML's `name:` field
        // or the file stem. Without `-n`, conda 25+ fails with "Unable to
        // determine environment" for YAMLs that declare no name (the common
        // nf-core style — found live: mixscape's seurat_lda.yaml).
        let env_name = conda_env_name_from_spec("conda", spec)?;
        Ok(format!(
            "conda env create -n {env_name} -f {spec} 2>/dev/null || conda env update -n {env_name} -f {spec} --prune"
        ))
    }

    fn teardown_command(&self, spec: &str) -> Result<Option<String>> {
        let env_name = conda_env_name_from_spec("conda", spec)?;
        Ok(Some(format!("conda env remove -n {env_name} -y")))
    }

    fn verify_command(&self, spec: &str) -> Result<Option<String>> {
        // `conda run` succeeds even when the env's own bin/ is missing
        // (tools then resolve from the system PATH), so the check must
        // target the env prefix itself — CONDA_PREFIX is set by `conda run`.
        // A present-but-empty bin/ (interrupted transaction) is just as
        // broken: require at least one entry, not just the dir (issue #136).
        let env_name = conda_env_name_from_spec("conda", spec)?;
        Ok(Some(format!(
            "conda run -n {env_name} bash -c 'test -d \"$CONDA_PREFIX/bin\" && ls \"$CONDA_PREFIX/bin\" | head -1 | grep -q .'"
        )))
    }

    fn cache_key(&self, spec: &str) -> String {
        format!("conda:{spec}")
    }
}

impl CondaBackend {
    /// Setup command with optional project-local prefix.
    ///
    /// When `prefix` is `Some`, uses `-p <prefix>` (install to the given
    /// directory). When `None`, uses the default name-based `-n <name>`
    /// (install to the system conda directory).
    pub fn setup_command_with_opts(&self, spec: &str, prefix: Option<&str>) -> Result<String> {
        if let Some(prefix) = prefix {
            Ok(format!(
                "conda env create -p {prefix} -f {spec} 2>/dev/null || conda env update -p {prefix} -f {spec} --prune"
            ))
        } else {
            self.setup_command(spec)
        }
    }

    /// Wrap command with optional project-local prefix.
    pub fn wrap_command_with_opts(
        &self,
        command: &str,
        spec: &str,
        prefix: Option<&str>,
    ) -> Result<String> {
        let escaped = escape_for_sh_single_quote(command);
        // Same --no-capture-output + CONDA_PREFIX/bin-first rationale as
        // wrap_command above.
        if let Some(prefix) = prefix {
            Ok(format!(
                "conda run --no-capture-output -p {prefix} bash -c 'export PATH=\"$CONDA_PREFIX/bin:$PATH\"; {escaped}'"
            ))
        } else {
            let env_name = conda_env_name_from_spec("conda", spec)?;
            Ok(format!(
                "conda run --no-capture-output -n {env_name} bash -c 'export PATH=\"$CONDA_PREFIX/bin:$PATH\"; {escaped}'"
            ))
        }
    }

    /// Teardown command with optional project-local prefix.
    pub fn teardown_command_with_opts(
        &self,
        spec: &str,
        prefix: Option<&str>,
    ) -> Result<Option<String>> {
        if let Some(prefix) = prefix {
            Ok(Some(format!("conda env remove -p {prefix} -y")))
        } else {
            self.teardown_command(spec)
        }
    }

    /// Verify command with optional project-local prefix.
    ///
    /// A prefix install (`conda env create -p`) creates no named env, so the
    /// plain `-n {name}` verify checks a different (nonexistent) env and
    /// always fails — a healthy pre-existing prefix env then failed verify,
    /// which sent the executor down the teardown-and-recreate path and
    /// deleted the user's own env (issue #136).
    pub fn verify_command_with_opts(
        &self,
        spec: &str,
        prefix: Option<&str>,
    ) -> Result<Option<String>> {
        if let Some(prefix) = prefix {
            Ok(Some(format!(
                "conda run -p {prefix} bash -c 'test -d \"$CONDA_PREFIX/bin\" && ls \"$CONDA_PREFIX/bin\" | head -1 | grep -q .'"
            )))
        } else {
            self.verify_command(spec)
        }
    }

    /// Cache key with optional project-local prefix.
    pub fn cache_key_with_opts(&self, spec: &str, prefix: Option<&str>) -> String {
        if let Some(prefix) = prefix {
            format!("conda:{spec}:{prefix}")
        } else {
            self.cache_key(spec)
        }
    }
}

/// Container-internal shell shim: re-exec the user script under `bash -c`
/// when the image provides bash, otherwise run it with the container `sh`
/// (the previous behaviour).
///
/// The container default `sh` is often dash/busybox, while nf-core-derived
/// images ship bash and their scripts rely on bash features (`set -o
/// pipefail`, `[[ ]]`); eager 2.5.3 (`set: Illegal option -o pipefail`) and
/// the nanoseq qcat `[[ ]]` failures were both this mismatch. The script is
/// passed as `$1` so it is never re-escaped into the shim text.
const CONTAINER_BASH_SHIM: &str = "if command -v bash >/dev/null 2>&1; \
then exec bash -c \"$1\"; else exec sh -c \"$1\"; fi";

/// Escape a string for safe embedding inside a `sh -c '...'` invocation.
///
/// Replaces every `'` with `'\''` (close quote, escaped literal quote, reopen quote)
/// so the value is safe regardless of what shell interprets the outer wrapper.
fn escape_for_sh_single_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Absolute host path for container bind mounts: docker's `-v`/`-w` and
/// singularity's `--bind` reject relative sources ("the working directory
/// '.' is invalid"). Resolves relative paths against the process CWD;
/// falls back to the path as given when the CWD is unavailable.
fn absolute_host_path(path: &std::path::Path) -> String {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    // Lexically drop CurDir components so `cwd/.` renders as `cwd` (docker
    // accepts either, but a clean mount string reads better in the plan).
    let mut clean = std::path::PathBuf::new();
    for component in joined.components() {
        if !matches!(component, std::path::Component::CurDir) {
            clean.push(component.as_os_str());
        }
    }
    clean.display().to_string()
}

/// Mamba / micromamba environment backend.
///
/// Auto-detects the installed binary, preferring `mamba` (fast solver)
/// over `micromamba` (standalone) over `conda` as a last-resort fallback.
/// CLI interface is compatible with conda — command templates are shared.
#[derive(Debug)]
pub struct MambaBackend {
    binary: String,
}

impl Default for MambaBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MambaBackend {
    /// Create a new backend, auto-detecting the available binary.
    ///
    /// Detection priority: `mamba` → `micromamba` → `conda` (fallback).
    pub fn new() -> Self {
        let binary = if std::process::Command::new("mamba")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            "mamba"
        } else if std::process::Command::new("micromamba")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            "micromamba"
        } else {
            "conda"
        };
        Self {
            binary: binary.to_string(),
        }
    }

    /// Setup command with optional project-local prefix.
    pub fn setup_command_with_opts(&self, spec: &str, prefix: Option<&str>) -> Result<String> {
        if let Some(prefix) = prefix {
            Ok(format!(
                "{} env create -p {prefix} -f {spec} 2>/dev/null || {} env update -p {prefix} -f {spec} --prune",
                self.binary, self.binary
            ))
        } else {
            self.setup_command(spec)
        }
    }

    /// Wrap command with optional project-local prefix.
    pub fn wrap_command_with_opts(
        &self,
        command: &str,
        spec: &str,
        prefix: Option<&str>,
    ) -> Result<String> {
        let escaped = escape_for_sh_single_quote(command);
        if let Some(prefix) = prefix {
            Ok(format!(
                "{} run -p {prefix} bash -c '{escaped}'",
                self.binary
            ))
        } else {
            let env_name = conda_env_name_from_spec("mamba", spec)?;
            Ok(format!(
                "{} run -n {env_name} bash -c '{escaped}'",
                self.binary
            ))
        }
    }

    /// Teardown command with optional project-local prefix.
    pub fn teardown_command_with_opts(
        &self,
        spec: &str,
        prefix: Option<&str>,
    ) -> Result<Option<String>> {
        if let Some(prefix) = prefix {
            Ok(Some(format!("{} env remove -p {prefix} -y", self.binary)))
        } else {
            self.teardown_command(spec)
        }
    }

    /// Verify command with optional project-local prefix — same
    /// prefix-vs-named-env rationale as [`CondaBackend::verify_command_with_opts`].
    pub fn verify_command_with_opts(
        &self,
        spec: &str,
        prefix: Option<&str>,
    ) -> Result<Option<String>> {
        if let Some(prefix) = prefix {
            Ok(Some(format!(
                "{} run -p {prefix} bash -c 'test -d \"$CONDA_PREFIX/bin\" && ls \"$CONDA_PREFIX/bin\" | head -1 | grep -q .'",
                self.binary
            )))
        } else {
            self.verify_command(spec)
        }
    }

    /// Cache key with optional project-local prefix.
    pub fn cache_key_with_opts(&self, spec: &str, prefix: Option<&str>) -> String {
        if let Some(prefix) = prefix {
            format!("mamba:{}:{spec}:{prefix}", self.binary)
        } else {
            self.cache_key(spec)
        }
    }
}

impl EnvironmentBackend for MambaBackend {
    fn name(&self) -> &str {
        "mamba"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new(&self.binary)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        _resources: Option<&crate::rule::Resources>,
        _workdir: &std::path::Path,
    ) -> Result<String> {
        let env_name = conda_env_name_from_spec("mamba", spec)?;
        let escaped = escape_for_sh_single_quote(command);
        Ok(format!(
            "{} run -n {env_name} bash -c '{escaped}'",
            self.binary
        ))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        // Same name-consistency fix as CondaBackend (see there): mamba 2.x
        // refuses nameless YAMLs without `-n`.
        let env_name = conda_env_name_from_spec("mamba", spec)?;
        Ok(format!(
            "{} env create -n {env_name} -f {spec} 2>/dev/null || {} env update -n {env_name} -f {spec} --prune",
            self.binary, self.binary
        ))
    }

    fn teardown_command(&self, spec: &str) -> Result<Option<String>> {
        let env_name = conda_env_name_from_spec("mamba", spec)?;
        Ok(Some(format!("{} env remove -n {env_name} -y", self.binary)))
    }

    fn verify_command(&self, spec: &str) -> Result<Option<String>> {
        let env_name = conda_env_name_from_spec("mamba", spec)?;
        Ok(Some(format!(
            "{} run -n {env_name} bash -c 'test -d \"$CONDA_PREFIX/bin\" && ls \"$CONDA_PREFIX/bin\" | head -1 | grep -q .'",
            self.binary
        )))
    }

    fn cache_key(&self, spec: &str) -> String {
        format!("mamba:{}:{spec}", self.binary)
    }
}

/// Docker environment backend.
#[derive(Debug, Default)]
pub struct DockerBackend;

/// The quay.io retry target for a bare Docker image name, if any.
///
/// Biocontainers publishes on quay.io, not Docker Hub, so the common
/// patterns `biocontainers/<tool>:<tag>` and bare `<tool>:<tag>` 404 on
/// Docker Hub. The retry only fires after a failed `docker pull` — an
/// explicitly registry-qualified spec (quay.io/…, docker.io/…,
/// localhost:5000/…) or a multi-segment path is pulled verbatim.
fn quay_biocontainers_fallback(spec: &str) -> Option<String> {
    let image = spec.split('@').next().unwrap_or(spec); // strip digest: name@sha256:…
    // Strip the tag, but only when the colon starts a tag — a colon whose
    // suffix contains '/' belongs to a host:port registry prefix
    // (localhost:5000/team/tool:1.0), which must be left intact.
    let name = match image.rsplit_once(':') {
        Some((head, tail)) if !tail.is_empty() && !tail.contains('/') => head,
        _ => image,
    };
    let segments: Vec<&str> = name.split('/').collect();
    if segments
        .first()
        .is_some_and(|s| s.contains('.') || s.contains(':'))
    {
        return None; // registry-qualified: quay.io/…, docker.io/…, host:port/…
    }
    match segments.as_slice() {
        [_single] => Some(format!("quay.io/biocontainers/{spec}")),
        ["biocontainers", _rest] => Some(format!("quay.io/{spec}")),
        _ => None,
    }
}

impl EnvironmentBackend for DockerBackend {
    fn name(&self) -> &str {
        "docker"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("docker")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        resources: Option<&crate::rule::Resources>,
        workdir: &std::path::Path,
    ) -> Result<String> {
        // Docker requires absolute paths for -v/-w: a relative workdir
        // ("." when running from the workflow dir) is rejected by the
        // daemon ("the working directory '.' is invalid").
        let workdir = absolute_host_path(workdir);
        let escaped_cmd = escape_for_sh_single_quote(command);

        let mut mem_arg = String::new();
        if let Some(res) = resources
            && let Some(mem) = &res.memory
        {
            mem_arg = format!(" --memory {mem}");
        }

        Ok(format!(
            "docker run --rm --user $(id -u):$(id -g){mem_arg} -v {workdir}:{workdir} -w {workdir} {spec} sh -c '{CONTAINER_BASH_SHIM}' sh '{escaped_cmd}'"
        ))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        // Pull only when the image is absent: two rules sharing one
        // image run their setup concurrently, and simultaneous
        // `docker pull`s of the same image race in the daemon (live:
        // fastqc x2 -> "failed to lease content: NotFound"). The
        // per-key env lock serializes the truly-missing case.
        //
        // Bare image names that 404 on Docker Hub get one quay.io
        // retry (biocontainers publishes there, not on Docker Hub —
        // see quay_biocontainers_fallback). Explicit registries are
        // never shadowed.
        //
        // A successful fallback pull is re-tagged with the original
        // spec: `docker run` re-resolves bare names against docker.io
        // even when the quay image exists locally, so without the tag
        // the run step would fail with the same 404 (live-verified on
        // tx-ubuntu).
        let mut pull = format!("docker pull {spec}");
        if let Some(fallback) = quay_biocontainers_fallback(spec) {
            pull.push_str(&format!(
                " || (docker pull {fallback} && docker tag {fallback} {spec})"
            ));
        }
        Ok(format!(
            "docker image inspect {spec} >/dev/null 2>&1 || {pull}"
        ))
    }

    fn teardown_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn verify_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn cache_key(&self, spec: &str) -> String {
        format!("docker:{spec}")
    }
}

/// Singularity/Apptainer environment backend.
///
/// Auto-detects the installed binary (preferring `apptainer` over
/// `singularity`) and uses it for all operations.
#[derive(Debug)]
pub struct SingularityBackend {
    binary: String,
}

impl Default for SingularityBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SingularityBackend {
    /// Create a new backend, auto-detecting the available binary.
    pub fn new() -> Self {
        let binary = if std::process::Command::new("apptainer")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            "apptainer"
        } else {
            "singularity"
        };
        Self {
            binary: binary.to_string(),
        }
    }
}

impl EnvironmentBackend for SingularityBackend {
    fn name(&self) -> &str {
        "singularity"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new(&self.binary)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        _resources: Option<&crate::rule::Resources>,
        workdir: &std::path::Path,
    ) -> Result<String> {
        // Same as docker: --bind sources must be absolute host paths.
        let workdir = absolute_host_path(workdir);
        let escaped_cmd = escape_for_sh_single_quote(command);

        Ok(format!(
            "{} exec --bind {workdir}:{workdir} {spec} sh -c '{CONTAINER_BASH_SHIM}' sh '{escaped_cmd}'",
            self.binary
        ))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        // Two spec shapes:
        // - a pull URI (docker://, library://, oras://, https://): pull
        //   when the derived SIF is absent — `pull` refuses to overwrite
        //   an existing SIF, and pulling per rule would rebuild the image
        //   every time. The SIF name follows the pull naming (last path
        //   segment, ':' -> '_'); an https URI whose final segment already
        //   ends in `.sif` must not get a second `.sif` appended — the
        //   existence guard would then never match the pulled artifact
        //   and every rule would re-pull (issue #136). The per-key env
        //   lock serializes the truly-missing case.
        // - a local SIF path: already deployed (e.g. a shared site image
        //   store) — nothing to pull. `{binary} pull` accepts only URIs,
        //   so a path must bypass it entirely; a missing local file fails
        //   with a clear diagnostic instead of pull's URI parse error.
        // - URI-encoded colons (%3A/%3a) are decoded BEFORE the colon
        //   substitution so the derived IMG name matches the name a
        //   previous `pull` produced — otherwise the existence guard
        //   never fires and every rule re-pulls (issue #162). Only %3A
        //   is decoded on purpose: decoding %2F would introduce path
        //   separators into the segment-derived filename.
        Ok(format!(
            "case '{spec}' in *://*) IMG=$(printf '%s' '{spec}' | sed 's#^docker://##; s#.*/##; s#%3A#:#g; s#%3a#:#g; s#:#_#g'); case \"$IMG\" in *.sif) ;; *) IMG=\"$IMG.sif\" ;; esac; [ -f \"$IMG\" ] || {b} pull \"$IMG\" {spec} ;; *) [ -f '{spec}' ] || {{ echo \"singularity spec '{spec}' is neither a pull URI nor an existing file\" >&2; exit 1; }} ;; esac",
            b = self.binary,
        ))
    }

    fn teardown_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn verify_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn cache_key(&self, spec: &str) -> String {
        format!("singularity:{spec}")
    }
}

/// Python venv environment backend.
#[derive(Debug, Default)]
pub struct VenvBackend;

impl EnvironmentBackend for VenvBackend {
    fn name(&self) -> &str {
        "venv"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        _resources: Option<&crate::rule::Resources>,
        _workdir: &std::path::Path,
    ) -> Result<String> {
        Ok(format!("source {spec}/bin/activate && {command}"))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        Ok(format!(
            "python3 -m venv {spec} && source {spec}/bin/activate && pip install -r requirements.txt"
        ))
    }

    fn teardown_command(&self, spec: &str) -> Result<Option<String>> {
        // Guard against dangerous paths — only allow relative, simple venv dirs.
        if spec.is_empty() || spec.contains("..") || spec.starts_with('/') {
            return Err(OxoFlowError::Environment {
                kind: "venv".to_string(),
                message: format!("refusing to remove unsafe path: {spec}"),
            });
        }
        Ok(Some(format!("rm -rf {spec}")))
    }

    fn verify_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn cache_key(&self, spec: &str) -> String {
        format!("venv:{spec}")
    }
}

impl VenvBackend {
    /// Setup command with configurable requirements file.
    ///
    /// `requirements` is a path to the pip requirements file. Defaults to
    /// `requirements.txt` when `None`.
    pub fn setup_command_with_reqs(
        &self,
        spec: &str,
        requirements: Option<&str>,
    ) -> Result<String> {
        let reqs = requirements.unwrap_or("requirements.txt");
        // POSIX `.` (not `source` — a bashism): setup commands run under sh.
        Ok(format!(
            "python3 -m venv {spec} && . {spec}/bin/activate && pip install -r {reqs}"
        ))
    }
}

/// Pixi environment backend.
#[derive(Debug, Default)]
pub struct PixiBackend;

impl EnvironmentBackend for PixiBackend {
    fn name(&self) -> &str {
        "pixi"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("pixi")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        _resources: Option<&crate::rule::Resources>,
        _workdir: &std::path::Path,
    ) -> Result<String> {
        // The workflow contract names the manifest FILE (`pixi =
        // "envs/pixi.toml"`) — `-e` selects an environment NAME inside a
        // pixi.toml already discovered from the CWD, which fails when the
        // manifest lives anywhere else (live-caught on tx-ubuntu).
        Ok(format!("pixi run --manifest-path {spec} {command}"))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        Ok(format!("pixi install --manifest-path {spec}"))
    }

    fn teardown_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn verify_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn cache_key(&self, spec: &str) -> String {
        format!("pixi:{spec}")
    }
}

/// System (no-op) environment backend for rules without environment specs.
#[derive(Debug, Default)]
pub struct SystemBackend;

impl EnvironmentBackend for SystemBackend {
    fn name(&self) -> &str {
        "system"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn wrap_command(
        &self,
        command: &str,
        _spec: &str,
        _resources: Option<&crate::rule::Resources>,
        _workdir: &std::path::Path,
    ) -> Result<String> {
        Ok(command.to_string())
    }

    fn setup_command(&self, _spec: &str) -> Result<String> {
        // No setup needed for the system backend.
        Ok("true".to_string())
    }

    fn teardown_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn verify_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn cache_key(&self, _spec: &str) -> String {
        "system".to_string()
    }
}

/// HPC Modules environment backend.
#[derive(Debug, Default)]
pub struct ModulesBackend;

impl EnvironmentBackend for ModulesBackend {
    fn name(&self) -> &str {
        "modules"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("modulecmd")
            .arg("--version")
            .output()
            .is_ok()
            || std::process::Command::new("module")
                .arg("--version")
                .output()
                .is_ok()
    }

    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        _resources: Option<&crate::rule::Resources>,
        _workdir: &std::path::Path,
    ) -> Result<String> {
        let modules = spec.replace(',', " ");
        // Initialize module system before loading modules
        // Different HPC sites use different module system installations
        let module_init = r#"# Initialize module system
if [ -f /etc/profile.d/modules.sh ]; then
    source /etc/profile.d/modules.sh
elif [ -f /usr/share/modules/init/bash ]; then
    source /usr/share/modules/init/bash
elif [ -f /usr/share/Modules/init/bash ]; then
    source /usr/share/Modules/init/bash
elif [ -f /opt/Modules/default/init/bash ]; then
    source /opt/Modules/default/init/bash
fi
"#;
        Ok(format!(
            "{module_init}if ! command -v module >/dev/null 2>&1; then echo \"oxo-flow: module command not found — is environment-modules or Lmod installed?\" >&2; exit 1; fi\nmodule load {modules} && {command}"
        ))
    }

    fn setup_command(&self, _spec: &str) -> Result<String> {
        Ok("true".to_string())
    }

    fn teardown_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn verify_command(&self, _spec: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn cache_key(&self, spec: &str) -> String {
        format!("modules:{spec}")
    }
}

/// Tracks which environments have already been set up so duplicate
/// setup work can be avoided across rules sharing the same environment.
#[derive(Debug, Default)]
pub struct EnvironmentCache {
    ready: HashSet<String>,
    /// Path to the cache file for persistence (optional).
    cache_file: Option<std::path::PathBuf>,
}

impl EnvironmentCache {
    /// Create a new, empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cache with file persistence.
    pub fn with_cache_dir(cache_dir: &std::path::Path) -> Self {
        let cache_file = cache_dir.join("environment_cache.json");
        let mut cache = Self {
            ready: HashSet::new(),
            cache_file: Some(cache_file.clone()),
        };

        // Try to load existing cache
        if let Err(e) = cache.load() {
            tracing::debug!("could not load environment cache: {}", e);
        }

        cache
    }

    /// Returns `true` if the environment identified by `key` has been set up.
    pub fn is_ready(&self, key: &str) -> bool {
        self.ready.contains(key)
    }

    /// Mark the environment identified by `key` as ready.
    pub fn mark_ready(&mut self, key: &str) {
        self.ready.insert(key.to_string());
        // Persist to file if configured
        if let Err(e) = self.save() {
            tracing::warn!("could not save environment cache: {}", e);
        }
    }

    /// Remove the entry for `key` — the cached environment no longer exists
    /// on disk (migrated, deleted, or failed mid-setup). The next readiness
    /// check misses and the setup path rebuilds it.
    pub fn invalidate(&mut self, key: &str) {
        if self.ready.remove(key) {
            // Persist to file if configured
            if let Err(e) = self.save() {
                tracing::warn!("could not save environment cache: {}", e);
            }
        }
    }

    /// Load cache from file.
    fn load(&mut self) -> Result<()> {
        if let Some(ref path) = self.cache_file
            && path.exists()
        {
            let content = std::fs::read_to_string(path).map_err(|e| OxoFlowError::Config {
                message: format!("failed to read cache file: {}", e),
            })?;
            let entries: Vec<String> =
                serde_json::from_str(&content).map_err(|e| OxoFlowError::Config {
                    message: format!("failed to parse cache file: {}", e),
                })?;
            self.ready = entries.into_iter().collect();
            tracing::debug!(
                "loaded {} cached environments from {}",
                self.ready.len(),
                path.display()
            );
        }
        Ok(())
    }

    /// Save cache to file.
    fn save(&self) -> Result<()> {
        if let Some(ref path) = self.cache_file {
            // Ensure parent directory exists
            let parent = crate::parent_dir(path);
            if parent != std::path::Path::new(".") {
                std::fs::create_dir_all(parent).map_err(|e| OxoFlowError::Config {
                    message: format!("failed to create cache directory: {}", e),
                })?;
            }

            let entries: Vec<String> = self.ready.iter().cloned().collect();
            let content = serde_json::to_string(&entries).map_err(|e| OxoFlowError::Config {
                message: format!("failed to serialize cache: {}", e),
            })?;

            std::fs::write(path, content).map_err(|e| OxoFlowError::Config {
                message: format!("failed to write cache file: {}", e),
            })?;

            tracing::trace!(
                "saved {} cached environments to {}",
                self.ready.len(),
                path.display()
            );
        }
        Ok(())
    }
}

/// Resolves the appropriate environment backend for a rule's environment spec.
pub struct EnvironmentResolver {
    mamba: MambaBackend,
    conda: CondaBackend,
    docker: DockerBackend,
    singularity: SingularityBackend,
    venv: VenvBackend,
    pixi: PixiBackend,
    modules: ModulesBackend,
    system: SystemBackend,
    cache: Arc<Mutex<EnvironmentCache>>,
    /// Per-cache-key setup mutexes (see [`Self::setup_lock`]).
    setup_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Per-cache-key record of environments this resolver issued a setup
    /// for, with whether the env pre-existed before the setup (true = the
    /// env was NOT created by this run). Teardown refuses to remove such
    /// envs — a failed verify must never destroy a pre-existing prefix env
    /// the user owns (issue #136). The resolver lives for one run, so this
    /// is exactly "created this run".
    setup_origins: std::sync::Mutex<HashMap<String, bool>>,
}

impl Default for EnvironmentResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvironmentResolver {
    /// Create a new environment resolver.
    pub fn new() -> Self {
        Self {
            mamba: MambaBackend::new(),
            conda: CondaBackend,
            docker: DockerBackend,
            singularity: SingularityBackend::new(),
            venv: VenvBackend,
            pixi: PixiBackend,
            modules: ModulesBackend,
            system: SystemBackend,
            cache: Arc::new(Mutex::new(EnvironmentCache::new())),
            setup_locks: std::sync::Mutex::new(HashMap::new()),
            setup_origins: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Create a new environment resolver with persistent cache directory.
    pub fn with_cache_dir(cache_dir: &std::path::Path) -> Self {
        Self {
            mamba: MambaBackend::new(),
            conda: CondaBackend,
            docker: DockerBackend,
            singularity: SingularityBackend::new(),
            venv: VenvBackend,
            pixi: PixiBackend,
            modules: ModulesBackend,
            system: SystemBackend,
            cache: Arc::new(Mutex::new(EnvironmentCache::with_cache_dir(cache_dir))),
            setup_locks: std::sync::Mutex::new(HashMap::new()),
            setup_origins: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Return a reference to the environment cache (async).
    pub async fn cache_is_ready(&self, key: &str) -> bool {
        let cache = self.cache.lock().await;
        cache.is_ready(key)
    }

    /// Mark an environment as ready in the cache (async).
    pub async fn cache_mark_ready(&self, key: &str) {
        let mut cache = self.cache.lock().await;
        cache.mark_ready(key);
    }

    /// Invalidate a cache entry (the cached environment vanished on disk).
    pub async fn cache_invalidate(&self, key: &str) {
        let mut cache = self.cache.lock().await;
        cache.invalidate(key);
    }

    /// Wrap a command using the appropriate environment backend.
    pub fn wrap_command(
        &self,
        command: &str,
        env_spec: &EnvironmentSpec,
        resources: Option<&crate::rule::Resources>,
        workdir: &std::path::Path,
    ) -> Result<String> {
        if let Some(ref mamba) = env_spec.mamba {
            return self.mamba.wrap_command_with_opts(
                command,
                mamba,
                env_spec.mamba_prefix.as_deref(),
            );
        }
        if let Some(ref conda) = env_spec.conda {
            return self.conda.wrap_command_with_opts(
                command,
                conda,
                env_spec.conda_prefix.as_deref(),
            );
        }
        if let Some(ref pixi) = env_spec.pixi {
            return self.pixi.wrap_command(command, pixi, resources, workdir);
        }
        if let Some(ref docker) = env_spec.docker {
            return self
                .docker
                .wrap_command(command, docker, resources, workdir);
        }
        if let Some(ref singularity) = env_spec.singularity {
            return self
                .singularity
                .wrap_command(command, singularity, resources, workdir);
        }
        if let Some(ref venv) = env_spec.venv {
            return self.venv.wrap_command(command, venv, resources, workdir);
        }
        if !env_spec.modules.is_empty() {
            let spec = env_spec.modules.join(",");
            return self
                .modules
                .wrap_command(command, &spec, resources, workdir);
        }
        self.system.wrap_command(command, "", resources, workdir)
    }

    /// Get the cache key for an environment specification.
    /// Used to track whether an environment has already been set up.
    pub fn cache_key(&self, env_spec: &EnvironmentSpec) -> String {
        if let Some(ref mamba) = env_spec.mamba {
            return self
                .mamba
                .cache_key_with_opts(mamba, env_spec.mamba_prefix.as_deref());
        }
        if let Some(ref conda) = env_spec.conda {
            return self
                .conda
                .cache_key_with_opts(conda, env_spec.conda_prefix.as_deref());
        }
        if let Some(ref pixi) = env_spec.pixi {
            return self.pixi.cache_key(pixi);
        }
        if let Some(ref docker) = env_spec.docker {
            return self.docker.cache_key(docker);
        }
        if let Some(ref singularity) = env_spec.singularity {
            return self.singularity.cache_key(singularity);
        }
        if let Some(ref venv) = env_spec.venv {
            return self.venv.cache_key(venv);
        }
        if !env_spec.modules.is_empty() {
            return self.modules.cache_key(&env_spec.modules.join(","));
        }
        self.system.cache_key("")
    }

    /// Get the setup command for an environment specification.
    /// This command creates/pulls the environment before first use.
    ///
    /// Records the setup origin per cache key (see `setup_origins`): for
    /// prefix installs the snapshot is whether the prefix directory already
    /// existed (relative prefixes resolve against the process CWD — the
    /// executor runs conda in the workflow dir, and the standard invocation
    /// runs from there too).
    pub fn setup_command(&self, env_spec: &EnvironmentSpec) -> Result<String> {
        if let Some(ref mamba) = env_spec.mamba {
            let pre_existed = env_spec
                .mamba_prefix
                .as_deref()
                .map(|p| std::path::Path::new(p).exists())
                .unwrap_or(false);
            self.record_setup(&self.cache_key(env_spec), pre_existed);
            return self
                .mamba
                .setup_command_with_opts(mamba, env_spec.mamba_prefix.as_deref());
        }
        if let Some(ref conda) = env_spec.conda {
            let pre_existed = env_spec
                .conda_prefix
                .as_deref()
                .map(|p| std::path::Path::new(p).exists())
                .unwrap_or(false);
            self.record_setup(&self.cache_key(env_spec), pre_existed);
            return self
                .conda
                .setup_command_with_opts(conda, env_spec.conda_prefix.as_deref());
        }
        if let Some(ref pixi) = env_spec.pixi {
            return self.pixi.setup_command(pixi);
        }
        if let Some(ref docker) = env_spec.docker {
            return self.docker.setup_command(docker);
        }
        if let Some(ref singularity) = env_spec.singularity {
            return self.singularity.setup_command(singularity);
        }
        if let Some(ref venv) = env_spec.venv {
            return self
                .venv
                .setup_command_with_reqs(venv, env_spec.venv_requirements.as_deref());
        }
        if !env_spec.modules.is_empty() {
            return self.modules.setup_command(&env_spec.modules.join(","));
        }
        self.system.setup_command("")
    }

    /// Per-environment setup mutex: concurrent rule instances that share an
    /// env must not run `conda env create` simultaneously — the loser's
    /// create transaction removes the winner's just-installed packages
    /// (live evidence: rnaseq S1/S2 race left `-fq` right after `+fq` in
    /// the env history and an empty env marked ready).
    pub fn setup_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .setup_locks
            .lock()
            .expect("environment cache mutex poisoned");
        locks.entry(key.to_string()).or_default().clone()
    }

    /// Post-setup usability check for the environment (conda/mamba verify
    /// the env's `bin/` exists; other backends return `None`). Prefix
    /// installs are verified against the prefix (`-p`), never the named-env
    /// form — `conda env create -p` creates no named env, so the `-n` check
    /// would always fail and drive the executor into the teardown path that
    /// deletes the user's own pre-existing prefix env (issue #136).
    pub fn verify_command(&self, env_spec: &EnvironmentSpec) -> Result<Option<String>> {
        if let Some(ref mamba) = env_spec.mamba {
            return self
                .mamba
                .verify_command_with_opts(mamba, env_spec.mamba_prefix.as_deref());
        }
        if let Some(ref conda) = env_spec.conda {
            return self
                .conda
                .verify_command_with_opts(conda, env_spec.conda_prefix.as_deref());
        }
        Ok(None)
    }

    /// Get the teardown command for an environment specification
    /// (removes the env so a broken one can be recreated cleanly).
    ///
    /// Refuses to tear down anything this run did not create: an env whose
    /// setup was never issued (verify failed before any setup) or whose
    /// prefix directory pre-existed when setup was issued is the user's
    /// own, and a failed verify must not destroy it. Returns `Ok(None)` in
    /// those cases; the executor treats that as "no teardown" and skips the
    /// recreate-retry.
    pub fn teardown_command(&self, env_spec: &EnvironmentSpec) -> Result<Option<String>> {
        let key = self.cache_key(env_spec);
        let origins = self
            .setup_origins
            .lock()
            .expect("environment cache mutex poisoned");
        match origins.get(&key) {
            None => {
                tracing::warn!(
                    env = %key,
                    "teardown skipped: no setup was issued for this environment in this run"
                );
                return Ok(None);
            }
            Some(true) => {
                tracing::warn!(
                    env = %key,
                    "teardown skipped: the environment pre-existed before this run's setup — refusing to remove it"
                );
                return Ok(None);
            }
            Some(false) => {}
        }
        if let Some(ref mamba) = env_spec.mamba {
            return self
                .mamba
                .teardown_command_with_opts(mamba, env_spec.mamba_prefix.as_deref());
        }
        if let Some(ref conda) = env_spec.conda {
            return self
                .conda
                .teardown_command_with_opts(conda, env_spec.conda_prefix.as_deref());
        }
        Ok(None)
    }

    /// Record that this run issued a setup for `key`, and whether the
    /// target env pre-existed (see `setup_origins`).
    fn record_setup(&self, key: &str, pre_existed: bool) {
        self.setup_origins
            .lock()
            .expect("environment cache mutex poisoned")
            .insert(key.to_string(), pre_existed);
    }

    /// Check which environment backends are available on the system.
    pub fn available_backends(&self) -> Vec<&str> {
        let mut available = vec!["system"];
        if self.mamba.is_available() {
            available.push("mamba");
        }
        if self.conda.is_available() {
            available.push("conda");
        }
        if self.pixi.is_available() {
            available.push("pixi");
        }
        if self.docker.is_available() {
            available.push("docker");
        }
        if self.singularity.is_available() {
            available.push("singularity");
        }
        if self.venv.is_available() {
            available.push("venv");
        }
        if self.modules.is_available() {
            available.push("modules");
        }
        available
    }

    /// Returns the names of all supported (non-system) environment backends,
    /// regardless of whether they are installed on the current system.
    ///
    /// Use this as the authoritative list when iterating over backends, so that
    /// user-facing code stays in sync with the resolver implementation.
    pub fn all_known_backends() -> &'static [&'static str] {
        &[
            "mamba",
            "conda",
            "pixi",
            "docker",
            "singularity",
            "venv",
            "modules",
        ]
    }

    /// Validate that the required environment backend is available for a spec.
    pub fn validate_spec(&self, env_spec: &EnvironmentSpec) -> Result<()> {
        if env_spec.mamba.is_some() && !self.mamba.is_available() {
            return Err(OxoFlowError::Environment {
                kind: "mamba".to_string(),
                message: "neither mamba nor micromamba is installed or in PATH".to_string(),
            });
        }
        if env_spec.conda.is_some() && !self.conda.is_available() {
            return Err(OxoFlowError::Environment {
                kind: "conda".to_string(),
                message: "conda is not installed or not in PATH".to_string(),
            });
        }
        if env_spec.pixi.is_some() {
            if !self.pixi.is_available() {
                return Err(OxoFlowError::Environment {
                    kind: "pixi".to_string(),
                    message: "pixi is not installed or not in PATH".to_string(),
                });
            }
            if !std::path::Path::new("pixi.toml").exists() {
                return Err(OxoFlowError::Environment {
                    kind: "pixi".to_string(),
                    message:
                        "pixi.toml not found in current directory — required for pixi environments"
                            .to_string(),
                });
            }
        }
        if env_spec.docker.is_some() && !self.docker.is_available() {
            return Err(OxoFlowError::Environment {
                kind: "docker".to_string(),
                message: "docker is not installed or not in PATH".to_string(),
            });
        }
        if !env_spec.modules.is_empty() && !self.modules.is_available() {
            return Err(OxoFlowError::Environment {
                kind: "modules".to_string(),
                message: "environment modules (modulecmd) is not installed or not in PATH"
                    .to_string(),
            });
        }
        if env_spec.singularity.is_some() && !self.singularity.is_available() {
            return Err(OxoFlowError::Environment {
                kind: "singularity".to_string(),
                message: "singularity/apptainer is not installed or not in PATH".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Container workdir absolutization ───────────────────────────

    #[test]
    fn container_wrappers_absolutize_relative_workdir() {
        // docker -v/-w and singularity --bind reject relative paths;
        // wrap_command must resolve them against the process CWD.
        let cwd = std::env::current_dir().unwrap();
        let expected = cwd.display().to_string();
        let docker = DockerBackend
            .wrap_command("echo hi", "ubuntu:24.04", None, std::path::Path::new("."))
            .unwrap();
        assert!(
            docker.contains(&format!("-v {expected}:{expected}")),
            "{docker}"
        );
        let sing = SingularityBackend::new()
            .wrap_command("echo hi", "ubuntu:24.04", None, std::path::Path::new("."))
            .unwrap();
        assert!(
            sing.contains(&format!("--bind {expected}:{expected}")),
            "{sing}"
        );
    }

    #[test]
    fn container_wrappers_keep_absolute_workdir_unchanged() {
        let abs = std::env::temp_dir().join("oxo-abs-test");
        let docker = DockerBackend
            .wrap_command("echo hi", "ubuntu:24.04", None, &abs)
            .unwrap();
        assert!(docker.contains(&format!("-v {}:{}", abs.display(), abs.display())));
    }

    // ── Container bash re-exec shim ─────────────────────────────────

    #[test]
    fn container_wrappers_reexec_under_bash_when_available() {
        // nf-core-derived containers ship bash; their scripts need it
        // (`set -o pipefail` is rejected by dash/busybox `sh`).
        let workdir = std::path::Path::new("/tmp/oxo-shim-test");
        let docker = DockerBackend
            .wrap_command("echo hi", "ubuntu:24.04", None, workdir)
            .unwrap();
        assert!(
            docker.contains("sh -c 'if command -v bash >/dev/null 2>&1; then exec bash -c \"$1\"; else exec sh -c \"$1\"; fi' sh 'echo hi'"),
            "{docker}"
        );
        let sing = SingularityBackend::new()
            .wrap_command("echo hi", "ubuntu:24.04", None, workdir)
            .unwrap();
        assert!(
            sing.contains("sh -c 'if command -v bash >/dev/null 2>&1; then exec bash -c \"$1\"; else exec sh -c \"$1\"; fi' sh 'echo hi'"),
            "{sing}"
        );
    }

    #[test]
    fn container_shim_keeps_user_script_out_of_shim_text() {
        // The user script travels as the `$1` argument (single-quote-escaped
        // for the host shell) — it is never interpolated into the shim, so
        // scripts containing quotes or shim-like tokens survive intact.
        let script = "echo 'a b'; grep \" sh -c '\" x";
        let docker = DockerBackend
            .wrap_command(script, "ubuntu:24.04", None, std::path::Path::new("/tmp/x"))
            .unwrap();
        let shim_marker = format!("sh -c '{CONTAINER_BASH_SHIM}' sh '");
        assert!(docker.contains(&shim_marker), "{docker}");
        // everything after the shim marker is the escaped script…
        let rest = docker.split(&shim_marker).nth(1).unwrap();
        assert!(rest.contains("grep"), "{docker}");
        assert!(rest.contains("sh -c"), "{docker}"); // script content preserved verbatim
    }

    // ── SystemBackend ──────────────────────────────────────────────

    #[test]
    fn system_backend_always_available() {
        let backend = SystemBackend;
        assert!(backend.is_available());
        assert_eq!(backend.name(), "system");
    }

    #[test]
    fn system_backend_passthrough() {
        let backend = SystemBackend;
        let result = backend
            .wrap_command("echo hello", "", None, std::path::Path::new("."))
            .unwrap();
        assert_eq!(result, "echo hello");
    }

    #[test]
    fn system_setup_command() {
        let backend = SystemBackend;
        assert_eq!(backend.setup_command("").unwrap(), "true");
    }

    #[test]
    fn system_teardown_command() {
        let backend = SystemBackend;
        assert!(backend.teardown_command("").unwrap().is_none());
    }

    #[test]
    fn system_cache_key() {
        let backend = SystemBackend;
        assert_eq!(backend.cache_key("anything"), "system");
    }

    // ── CondaBackend ───────────────────────────────────────────────

    #[test]
    fn conda_setup_command() {
        let backend = CondaBackend;
        let cmd = backend.setup_command("envs/qc.yaml").unwrap();
        // `-n <stem>` keeps setup consistent with `conda run -n <stem>`
        // wrapping — nameless YAMLs (nf-core style) no longer fail setup
        // with "Unable to determine environment" (conda 25+).
        assert!(cmd.contains("conda env create -n qc -f envs/qc.yaml"));
        assert!(cmd.contains("conda env update -n qc -f envs/qc.yaml --prune"));
    }

    #[test]
    fn conda_teardown_command() {
        let backend = CondaBackend;
        let cmd = backend.teardown_command("envs/qc.yaml").unwrap().unwrap();
        assert_eq!(cmd, "conda env remove -n qc -y");
    }

    #[test]
    fn conda_teardown_bare_name() {
        let backend = CondaBackend;
        let cmd = backend.teardown_command("myenv").unwrap().unwrap();
        assert_eq!(cmd, "conda env remove -n myenv -y");
    }

    #[test]
    fn conda_cache_key() {
        let backend = CondaBackend;
        assert_eq!(backend.cache_key("envs/qc.yaml"), "conda:envs/qc.yaml");
    }

    #[test]
    fn conda_env_name_carries_content_hash_for_file_specs() {
        // issue #159: same stem, different content → different env names,
        // so two workflows' `deseq2.yaml` variants never share a prefix.
        let dir = tempfile::tempdir().unwrap();
        // Distinct directories so both specs share the STEM `deseq2` while
        // carrying different content — the exact cross-workflow collision
        // shape from the live incident.
        let a = dir.path().join("wf-a").join("deseq2.yaml");
        let b = dir.path().join("wf-b").join("deseq2.yaml");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        std::fs::write(&a, "channels: [bioconda]\ndependencies: [r-deseq2]\n").unwrap();
        let name_a = CondaBackend.setup_command(a.to_str().unwrap()).unwrap();
        std::fs::write(
            &b,
            "channels: [bioconda]\ndependencies: [r-deseq2, r-optparse=1.7.5]\n",
        )
        .unwrap();
        let name_b = CondaBackend.setup_command(b.to_str().unwrap()).unwrap();
        let extract = |cmd: String| {
            let start = cmd.find(" -n ").unwrap() + 4;
            let rest = &cmd[start..];
            rest.split_whitespace().next().unwrap().to_string()
        };
        let na = extract(name_a);
        let nb = extract(name_b);
        assert_ne!(na, nb, "different content must derive different env names");
        assert!(
            na.starts_with("deseq2-"),
            "file specs carry a hash suffix: {na}"
        );
        assert!(nb.starts_with("deseq2-"));
        // Same content → same name (dedup preserved).
        let c = dir.path().join("other-dir").join("deseq2.yaml");
        std::fs::create_dir_all(c.parent().unwrap()).unwrap();
        std::fs::copy(&a, &c).unwrap();
        let name_c = CondaBackend.setup_command(c.to_str().unwrap()).unwrap();
        assert_eq!(
            extract(name_c),
            na,
            "identical content must keep the same env name"
        );
    }

    #[test]
    fn conda_env_name_bare_name_has_no_hash() {
        // Non-file specs (plain names) keep the plain name — no suffix.
        let backend = CondaBackend;
        let cmd = backend.setup_command("myenv").unwrap();
        assert!(cmd.contains("conda env create -n myenv -f myenv"));
    }

    #[test]
    fn conda_env_name_validation_rejects_untrusted_yaml_names() {
        // The env name is interpolated into `conda run -n <name>` inside a
        // shell string, so an untrusted YAML `name:` could break out of the
        // quoting and run arbitrary shell (issue #136). The derived name is
        // validated and the failure is a clear environment error, not a
        // silently emitted command that does not do what it says.
        let dir = tempfile::tempdir().unwrap();
        let evil = dir.path().join("evil.yaml");
        std::fs::write(&evil, "name: \"x'; rm -rf ~ ;'\"\n").unwrap();
        match conda_env_name_from_spec("conda", evil.to_str().unwrap()) {
            Err(OxoFlowError::Environment { kind, message }) => {
                assert_eq!(kind, "conda");
                assert!(
                    message.contains("invalid environment name"),
                    "message must explain the rejection: {message}"
                );
                assert!(
                    message.contains("alphanumerics"),
                    "message must state the allowed alphabet: {message}"
                );
            }
            other => panic!("expected an Environment error, got: {other:?}"),
        }
        // The mamba kind is surfaced through the same check.
        assert!(matches!(
            conda_env_name_from_spec("mamba", evil.to_str().unwrap()),
            Err(OxoFlowError::Environment { kind, .. }) if kind == "mamba"
        ));
    }

    #[test]
    fn conda_env_name_validation_accepts_plain_names() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = dir.path().join("env.yaml");
        std::fs::write(&yaml, "name: rnaseq_2\nchannels: [bioconda]\n").unwrap();
        let derived = conda_env_name_from_spec("conda", yaml.to_str().unwrap()).unwrap();
        assert!(
            derived.starts_with("rnaseq_2-") && derived.len() == "rnaseq_2-".len() + 8,
            "file specs keep the YAML name and gain the 8-hex content-hash suffix: {derived}"
        );
        assert_eq!(
            conda_env_name_from_spec("conda", "envs/qc.yaml").unwrap(),
            "qc",
            "the file-stem fallback passes for plain stems (no file on disk → no suffix)"
        );
        assert_eq!(
            conda_env_name_from_spec("conda", "my-env_1").unwrap(),
            "my-env_1",
            "a bare name spec passes"
        );
    }

    // ── DockerBackend ──────────────────────────────────────────────

    #[test]
    fn docker_wrap_command() {
        let backend = DockerBackend;
        let result = backend
            .wrap_command(
                "bwa mem ref.fa reads.fq",
                "biocontainers/bwa:0.7.17",
                None,
                std::path::Path::new("."),
            )
            .unwrap();
        assert!(result.contains("docker run"));
        assert!(result.contains("--user $(id -u):$(id -g)"));
        assert!(result.contains("biocontainers/bwa:0.7.17"));
    }

    #[test]
    fn docker_setup_command() {
        let backend = DockerBackend;
        let cmd = backend.setup_command("biocontainers/bwa:0.7.17").unwrap();
        assert_eq!(
            cmd,
            "docker image inspect biocontainers/bwa:0.7.17 >/dev/null 2>&1 || \
             docker pull biocontainers/bwa:0.7.17 || \
             (docker pull quay.io/biocontainers/bwa:0.7.17 && \
             docker tag quay.io/biocontainers/bwa:0.7.17 biocontainers/bwa:0.7.17)"
        );
    }

    #[test]
    fn docker_fallback_pull_is_retagged_with_the_original_spec() {
        // `docker run` re-resolves bare names against docker.io even when
        // the quay image exists locally — the fallback pull must re-tag so
        // the run step finds the image under the original spec.
        let backend = DockerBackend;
        let cmd = backend.setup_command("bwa:0.7.19").unwrap();
        assert!(
            cmd.contains(
                "docker pull quay.io/biocontainers/bwa:0.7.19 \
                 && docker tag quay.io/biocontainers/bwa:0.7.19 bwa:0.7.19"
            ),
            "fallback pull must be re-tagged with the original spec: {cmd}"
        );
    }

    #[test]
    fn docker_setup_command_falls_back_to_quay_for_bare_names() {
        let backend = DockerBackend;
        // Bare single-name specs (a common bioinformatics pattern) retry
        // against quay.io/biocontainers after a docker.io failure.
        let cmd = backend.setup_command("bwa:0.7.19").unwrap();
        assert!(
            cmd.contains("docker pull bwa:0.7.19 || (docker pull quay.io/biocontainers/bwa:0.7.19"),
            "bare name must get the quay.io/biocontainers fallback: {cmd}"
        );
    }

    #[test]
    fn docker_setup_command_has_no_fallback_for_registry_qualified_specs() {
        let backend = DockerBackend;
        // Explicit registries must never be shadowed by a fallback.
        let cmd = backend
            .setup_command("quay.io/nf-core/cellranger:7.1.0")
            .unwrap();
        assert_eq!(
            cmd.matches("docker pull").count(),
            1,
            "explicit quay spec must not get a second (fallback) pull: {cmd}"
        );
        assert!(
            cmd.ends_with("docker pull quay.io/nf-core/cellranger:7.1.0"),
            "explicit quay spec must be pulled verbatim: {cmd}"
        );
        // Local registries (host:port) and multi-segment paths also stay verbatim.
        let cmd = backend
            .setup_command("localhost:5000/team/tool:1.0")
            .unwrap();
        assert!(
            !cmd.contains("quay.io/biocontainers"),
            "local registry: {cmd}"
        );
        let cmd = backend
            .setup_command("docker.io/library/ubuntu:22.04")
            .unwrap();
        assert!(
            !cmd.contains("quay.io/biocontainers"),
            "docker.io explicit: {cmd}"
        );
    }

    #[test]
    fn docker_teardown_is_noop() {
        let backend = DockerBackend;
        assert!(
            backend
                .teardown_command("biocontainers/bwa:0.7.17")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn docker_cache_key() {
        let backend = DockerBackend;
        assert_eq!(
            backend.cache_key("biocontainers/bwa:0.7.17"),
            "docker:biocontainers/bwa:0.7.17"
        );
    }

    // ── SingularityBackend ─────────────────────────────────────────

    #[test]
    fn singularity_wrap_command() {
        let backend = SingularityBackend::new();
        let result = backend
            .wrap_command(
                "samtools sort input.bam",
                "image.sif",
                None,
                std::path::Path::new("."),
            )
            .unwrap();
        assert!(result.contains(" exec "));
        assert!(!result.contains("--memory"));
        assert!(result.contains("image.sif"));
    }

    #[test]
    fn singularity_setup_command() {
        let backend = SingularityBackend::new();
        let cmd = backend.setup_command("docker://ubuntu:22.04").unwrap();
        assert!(cmd.contains(r#" pull "$IMG" docker://ubuntu:22.04"#));
        // inspect-first guard for the SIF naming
        assert!(cmd.starts_with("case 'docker://ubuntu:22.04' in *://*)"));
        assert!(cmd.contains("IMG=$(printf '%s' 'docker://ubuntu:22.04'"));
        assert!(cmd.contains("[ -f \"$IMG\" ] || "));
    }

    #[test]
    fn singularity_setup_https_uri_derives_final_segment_without_double_sif() {
        let backend = SingularityBackend::new();
        let cmd = backend
            .setup_command("https://example.com/images/foo.sif")
            .unwrap();
        // The derived name is the final URI segment — `foo.sif`, not
        // `foo.sif.sif`: the old unconditional `.sif` append made the
        // existence guard look for the wrong artifact, so every rule
        // re-pulled (issue #136).
        assert!(
            cmd.contains("IMG=$(printf '%s' 'https://example.com/images/foo.sif'"),
            "the URI arm must derive the name from the spec: {cmd}"
        );
        assert!(
            cmd.contains("case \"$IMG\" in *.sif) ;; *) IMG=\"$IMG.sif\" ;; esac;"),
            "a final segment already ending in .sif must not get a second .sif: {cmd}"
        );
        assert!(cmd.contains("[ -f \"$IMG\" ] || "));
    }

    #[test]
    fn singularity_setup_pulls_into_the_derived_artifact_name() {
        // %-encoded HTTP URIs (issue #185): apptainer writes the URI's
        // raw final segment (delly%3A1.7.2--h4d20210_0, no .sif) when
        // pulling without an output argument, so the derived-name
        // existence guard never matches and a second rule re-pulls —
        // and apptainer refuses to overwrite the stale artifact, killing
        // the run (live: clindet SV_delly_mini on tx-ubuntu). The pull
        // must write into $IMG explicitly so check and artifact agree.
        let backend = SingularityBackend::new();
        let cmd = backend
            .setup_command("https://depot.galaxyproject.org/singularity/delly%3A1.7.2--h4d20210_0")
            .unwrap();
        assert!(
            cmd.contains(r#"[ -f "$IMG" ] || "#)
                && cmd.contains(r#" pull "$IMG" https://depot.galaxyproject.org/singularity/delly%3A1.7.2--h4d20210_0"#),
            "the pull must write into the derived $IMG name: {cmd}"
        );
    }

    #[test]
    fn singularity_setup_decodes_uri_encoded_colons_before_img_naming() {
        // %3A-encoded colons must be decoded before the `s#:#_#g`
        // substitution, or the derived IMG name never matches the file a
        // previous pull produced and every rule re-pulls (issue #162 —
        // live evidence: clindet's lofreq URI on tx-ubuntu).
        let backend = SingularityBackend::new();
        let cmd = backend
            .setup_command("docker://quay.io/biocontainers/tool%3A1.2.3--h1a2b3c4")
            .unwrap();
        assert!(
            cmd.contains("sed 's#^docker://##; s#.*/##; s#%3A#:#g; s#%3a#:#g; s#:#_#g'"),
            "the URI arm must decode %3A before the colon substitution: {cmd}"
        );
    }

    #[test]
    fn singularity_setup_local_sif_needs_no_pull() {
        let backend = SingularityBackend::new();
        let cmd = backend
            .setup_command("/data/images/bwa_0.7.17.sif")
            .unwrap();
        // A local SIF is already deployed: the local branch is an
        // existence check with a clear failure for the missing case —
        // pull only ever appears in the URI arm of the case.
        assert!(cmd.contains("case '/data/images/bwa_0.7.17.sif' in"));
        assert!(cmd.contains("*) [ -f '/data/images/bwa_0.7.17.sif' ] ||"));
        assert!(cmd.contains("neither a pull URI nor an existing file"));
    }

    #[test]
    fn singularity_teardown_is_noop() {
        let backend = SingularityBackend::new();
        assert!(backend.teardown_command("image.sif").unwrap().is_none());
    }

    #[test]
    fn singularity_cache_key() {
        let backend = SingularityBackend::new();
        assert_eq!(backend.cache_key("image.sif"), "singularity:image.sif");
    }

    // ── VenvBackend ────────────────────────────────────────────────

    #[test]
    fn venv_setup_command() {
        let backend = VenvBackend;
        let cmd = backend.setup_command(".venv").unwrap();
        assert!(cmd.contains("python3 -m venv .venv"));
        assert!(cmd.contains("pip install -r requirements.txt"));
    }

    #[test]
    fn venv_teardown_command() {
        let backend = VenvBackend;
        let cmd = backend.teardown_command(".venv").unwrap().unwrap();
        assert_eq!(cmd, "rm -rf .venv");
    }

    #[test]
    fn venv_teardown_rejects_absolute_path() {
        let backend = VenvBackend;
        assert!(backend.teardown_command("/usr").is_err());
    }

    #[test]
    fn venv_teardown_rejects_traversal() {
        let backend = VenvBackend;
        assert!(backend.teardown_command("../escape").is_err());
    }

    #[test]
    fn venv_cache_key() {
        let backend = VenvBackend;
        assert_eq!(backend.cache_key(".venv"), "venv:.venv");
    }

    // ── PixiBackend ────────────────────────────────────────────────

    #[test]
    fn pixi_setup_command() {
        let backend = PixiBackend;
        // The workflow spec names the manifest FILE; -e (environment
        // name) fails whenever the manifest is not in the CWD.
        let cmd = backend.setup_command("envs/pixi.toml").unwrap();
        assert_eq!(cmd, "pixi install --manifest-path envs/pixi.toml");
    }

    #[test]
    fn pixi_teardown_is_noop() {
        let backend = PixiBackend;
        assert!(backend.teardown_command("default").unwrap().is_none());
    }

    #[test]
    fn pixi_cache_key() {
        let backend = PixiBackend;
        assert_eq!(backend.cache_key("default"), "pixi:default");
    }

    // ── EnvironmentCache ───────────────────────────────────────────

    #[test]
    fn cache_initially_empty() {
        let cache = EnvironmentCache::new();
        assert!(!cache.is_ready("conda:envs/qc.yaml"));
    }

    #[test]
    fn cache_mark_and_query() {
        let mut cache = EnvironmentCache::new();
        cache.mark_ready("docker:ubuntu:22.04");
        assert!(cache.is_ready("docker:ubuntu:22.04"));
        assert!(!cache.is_ready("docker:alpine:3.18"));
    }

    #[test]
    fn cache_multiple_entries() {
        let mut cache = EnvironmentCache::new();
        cache.mark_ready("conda:envs/qc.yaml");
        cache.mark_ready("docker:ubuntu:22.04");
        cache.mark_ready("venv:.venv");
        assert!(cache.is_ready("conda:envs/qc.yaml"));
        assert!(cache.is_ready("docker:ubuntu:22.04"));
        assert!(cache.is_ready("venv:.venv"));
        assert!(!cache.is_ready("pixi:default"));
    }

    #[test]
    fn cache_idempotent_mark() {
        let mut cache = EnvironmentCache::new();
        cache.mark_ready("system");
        cache.mark_ready("system");
        assert!(cache.is_ready("system"));
    }

    #[test]
    fn cache_invalidate_removes_entry() {
        let mut cache = EnvironmentCache::new();
        cache.mark_ready("conda:envs/qc.yaml");
        cache.mark_ready("docker:ubuntu:22.04");
        cache.invalidate("conda:envs/qc.yaml");
        assert!(!cache.is_ready("conda:envs/qc.yaml"));
        assert!(cache.is_ready("docker:ubuntu:22.04"));
        // Invalidating an unknown key is a no-op, not an error.
        cache.invalidate("pixi:default");
    }

    // ── EnvironmentResolver ────────────────────────────────────────

    #[test]
    fn resolver_empty_spec_uses_system() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec::default();
        let result = resolver
            .wrap_command("echo test", &spec, None, std::path::Path::new("."))
            .unwrap();
        assert_eq!(result, "echo test");
    }

    #[test]
    fn resolver_docker_spec() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            docker: Some("ubuntu:22.04".to_string()),
            ..Default::default()
        };
        let result = resolver
            .wrap_command("echo test", &spec, None, std::path::Path::new("."))
            .unwrap();
        assert!(result.contains("docker run"));
        assert!(result.contains("ubuntu:22.04"));
    }

    #[test]
    fn resolver_available_backends() {
        let resolver = EnvironmentResolver::new();
        let available = resolver.available_backends();
        assert!(available.contains(&"system"));
    }

    #[tokio::test]
    async fn resolver_cache_integration() {
        let resolver = EnvironmentResolver::new();
        let key = CondaBackend.cache_key("envs/qc.yaml");
        assert!(!resolver.cache_is_ready(&key).await);
        resolver.cache_mark_ready(&key).await;
        assert!(resolver.cache_is_ready(&key).await);
    }

    // ── Additional wrap_command tests ──────────────────────────────

    #[test]
    fn conda_wrap_command() {
        let backend = CondaBackend;
        let result = backend
            .wrap_command(
                "fastqc reads.fq",
                "envs/qc.yaml",
                None,
                std::path::Path::new("."),
            )
            .unwrap();
        assert!(
            result.contains("conda run"),
            "expected 'conda run' in: {result}"
        );
        assert!(result.contains("fastqc reads.fq"));
    }

    #[test]
    fn mamba_backend_name() {
        let backend = MambaBackend::new();
        assert_eq!(backend.name(), "mamba");
    }

    #[test]
    fn mamba_wrap_command() {
        let backend = MambaBackend::new();
        let result = backend
            .wrap_command(
                "fastqc reads.fq",
                "envs/qc.yaml",
                None,
                std::path::Path::new("."),
            )
            .unwrap();
        assert!(result.contains("run -n"), "expected 'run -n' in: {result}");
        assert!(result.contains("fastqc reads.fq"));
    }

    #[test]
    fn mamba_setup_command() {
        let backend = MambaBackend::new();
        let result = backend.setup_command("envs/qc.yaml").unwrap();
        assert!(result.contains("env create"));
        assert!(result.contains("env update"));
    }

    #[test]
    fn mamba_teardown_command() {
        let backend = MambaBackend::new();
        let result = backend.teardown_command("envs/qc.yaml").unwrap();
        assert!(result.unwrap().contains("env remove"));
    }

    #[test]
    fn conda_verify_command_checks_env_bin_directory() {
        let backend = CondaBackend;
        let cmd = backend.verify_command("envs/qc.yaml").unwrap().unwrap();
        assert!(cmd.contains("conda run -n qc"));
        // The check must target the env's own prefix (CONDA_PREFIX), not a
        // tool lookup — a broken env with no bin/ still lets `true` resolve
        // from the system PATH.
        assert!(cmd.contains("test -d"));
        assert!(cmd.contains("CONDA_PREFIX/bin"));
    }

    #[test]
    fn mamba_verify_command_uses_binary() {
        let backend = MambaBackend::new();
        let cmd = backend.verify_command("envs/qc.yaml").unwrap().unwrap();
        // The detection chain picks mamba → micromamba → conda; assert on
        // the backend's actual binary, not a hardcoded name.
        assert!(cmd.contains(&format!("{} run -n qc", backend.binary)));
        assert!(cmd.contains("CONDA_PREFIX/bin"));
    }

    #[test]
    fn docker_verify_command_is_none() {
        let backend = DockerBackend;
        assert!(backend.verify_command("image:tag").unwrap().is_none());
    }

    #[test]
    fn mamba_cache_key() {
        let backend = MambaBackend::new();
        let key = backend.cache_key("envs/qc.yaml");
        assert!(key.starts_with("mamba:"), "expected 'mamba:' prefix: {key}");
        assert!(key.contains("envs/qc.yaml"));
    }

    #[test]
    fn mamba_setup_command_with_prefix() {
        let backend = MambaBackend::new();
        let result = backend
            .setup_command_with_opts("envs/qc.yaml", Some(".oxo-conda"))
            .unwrap();
        assert!(result.contains("-p .oxo-conda"));
    }

    #[test]
    fn mamba_wrap_command_with_prefix() {
        let backend = MambaBackend::new();
        let result = backend
            .wrap_command_with_opts("fastqc reads.fq", "envs/qc.yaml", Some(".oxo-conda"))
            .unwrap();
        assert!(result.contains("-p .oxo-conda"));
        assert!(result.contains("fastqc reads.fq"));
    }

    #[test]
    fn mamba_cache_key_with_prefix() {
        let backend = MambaBackend::new();
        let key = backend.cache_key_with_opts("envs/qc.yaml", Some(".oxo-conda"));
        assert!(key.contains(".oxo-conda"));
    }

    #[test]
    fn resolver_wraps_mamba_spec() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            mamba: Some("envs/qc.yaml".to_string()),
            ..Default::default()
        };
        let result = resolver
            .wrap_command("fastqc reads.fq", &spec, None, std::path::Path::new("."))
            .unwrap();
        assert!(
            result.contains("run -n"),
            "expected wrapped mamba command: {result}"
        );
    }

    #[test]
    fn resolver_mamba_preferred_over_conda() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            mamba: Some("envs/qc.yaml".to_string()),
            conda: Some("envs/other.yaml".to_string()),
            ..Default::default()
        };
        let wrapped = resolver
            .wrap_command("echo test", &spec, None, std::path::Path::new("."))
            .unwrap();
        // Should use mamba binary, not conda
        assert!(wrapped.contains("run -n"), "expected 'run -n': {wrapped}");
        let cache_key = resolver.cache_key(&spec);
        assert!(
            cache_key.starts_with("mamba:"),
            "mamba should take priority over conda: {cache_key}"
        );
    }

    #[test]
    fn venv_wrap_command() {
        let backend = VenvBackend;
        let result = backend
            .wrap_command("pip list", ".venv", None, std::path::Path::new("."))
            .unwrap();
        assert!(result.contains("source .venv/bin/activate"));
        assert!(result.contains("pip list"));
    }

    #[test]
    fn pixi_wrap_command() {
        let backend = PixiBackend;
        let result = backend
            .wrap_command(
                "python main.py",
                "envs/pixi.toml",
                None,
                std::path::Path::new("."),
            )
            .unwrap();
        assert_eq!(
            result,
            "pixi run --manifest-path envs/pixi.toml python main.py"
        );
    }

    #[test]
    fn resolver_wraps_conda_spec() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            conda: Some("envs/qc.yaml".to_string()),
            ..Default::default()
        };
        let result = resolver
            .wrap_command("fastqc reads.fq", &spec, None, std::path::Path::new("."))
            .unwrap();
        assert!(
            result.contains("conda run"),
            "expected conda wrapping, got: {result}"
        );
    }

    #[test]
    fn resolver_wraps_docker_spec() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            docker: Some("biocontainers/bwa:0.7.17".to_string()),
            ..Default::default()
        };
        let result = resolver
            .wrap_command(
                "bwa mem ref.fa reads.fq",
                &spec,
                None,
                std::path::Path::new("."),
            )
            .unwrap();
        assert!(result.contains("docker run"));
        assert!(result.contains("biocontainers/bwa:0.7.17"));
        assert!(result.contains("bwa mem ref.fa reads.fq"));
    }

    #[test]
    fn venv_teardown_guards_unsafe_paths() {
        let backend = VenvBackend;
        // Absolute paths must be rejected
        assert!(backend.teardown_command("/usr/local").is_err());
        assert!(backend.teardown_command("/home/user/.venv").is_err());
        // Traversal paths must be rejected
        assert!(backend.teardown_command("../escape").is_err());
        assert!(backend.teardown_command("foo/../bar").is_err());
        // Empty spec must be rejected
        assert!(backend.teardown_command("").is_err());
        // Relative, safe paths must succeed
        assert!(backend.teardown_command(".venv").is_ok());
        assert!(backend.teardown_command("my_env").is_ok());
    }

    #[test]
    fn environment_cache_operations() {
        let mut cache = EnvironmentCache::new();

        // Initially nothing is ready
        assert!(!cache.is_ready("conda:envs/qc.yaml"));
        assert!(!cache.is_ready("docker:ubuntu:22.04"));

        // Mark one ready and verify
        cache.mark_ready("conda:envs/qc.yaml");
        assert!(cache.is_ready("conda:envs/qc.yaml"));
        assert!(!cache.is_ready("docker:ubuntu:22.04"));

        // Mark another and verify both
        cache.mark_ready("docker:ubuntu:22.04");
        assert!(cache.is_ready("conda:envs/qc.yaml"));
        assert!(cache.is_ready("docker:ubuntu:22.04"));

        // Idempotent — marking twice doesn't break anything
        cache.mark_ready("conda:envs/qc.yaml");
        assert!(cache.is_ready("conda:envs/qc.yaml"));
    }

    // --- ModulesBackend tests -------------------------------------------------

    #[test]
    fn modules_backend_name() {
        assert_eq!(ModulesBackend.name(), "modules");
    }

    #[test]
    fn modules_setup_command() {
        let backend = ModulesBackend;
        let result = backend.setup_command("java/11,gatk/4.2");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "true");
    }

    #[test]
    fn modules_teardown_is_noop() {
        let backend = ModulesBackend;
        assert!(backend.teardown_command("java/11").unwrap().is_none());
    }

    #[test]
    fn modules_wrap_command() {
        let backend = ModulesBackend;
        let cmd = backend
            .wrap_command(
                "java -jar gatk.jar",
                "java/11,gatk/4.2",
                None,
                std::path::Path::new("."),
            )
            .unwrap();
        assert!(cmd.contains("module load java/11 gatk/4.2"));
        assert!(cmd.contains("java -jar gatk.jar"));
    }

    #[test]
    fn modules_cache_key() {
        let backend = ModulesBackend;
        assert_eq!(
            backend.cache_key("java/11,gatk/4.2"),
            "modules:java/11,gatk/4.2"
        );
    }

    // --- cache file persistence test -----------------------------------------

    #[test]
    fn setup_lock_is_shared_per_key() {
        let resolver = EnvironmentResolver::new();
        let a = resolver.setup_lock("conda:envs/fq.yaml");
        let b = resolver.setup_lock("conda:envs/fq.yaml");
        let other = resolver.setup_lock("conda:envs/star.yaml");
        // Same key → same mutex (concurrent rule instances serialize their
        // env setup); different keys → independent mutexes.
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &other));
    }

    #[test]
    fn environment_cache_dir_initialization() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");

        // with_cache_dir creates a fresh cache backed by the file
        let mut cache = EnvironmentCache::with_cache_dir(&cache_path);
        assert!(!cache.is_ready("conda:envs/qc.yaml"));
        cache.mark_ready("conda:envs/qc.yaml");
        assert!(cache.is_ready("conda:envs/qc.yaml"));
    }

    // --- conda prefix tests ---------------------------------------------------

    #[test]
    fn conda_setup_command_with_prefix() {
        let backend = CondaBackend;
        let cmd = backend
            .setup_command_with_opts("envs/qc.yaml", Some(".oxo-conda"))
            .unwrap();
        assert!(
            cmd.contains("conda env create -p .oxo-conda -f envs/qc.yaml"),
            "expected -p prefix form, got: {cmd}"
        );
        assert!(cmd.contains("conda env update -p .oxo-conda -f envs/qc.yaml --prune"));
    }

    #[test]
    fn conda_setup_command_without_prefix() {
        let backend = CondaBackend;
        let cmd = backend
            .setup_command_with_opts("envs/qc.yaml", None)
            .unwrap();
        assert!(cmd.contains("conda env create -n qc -f envs/qc.yaml"));
        // Should NOT contain -p
        assert!(!cmd.contains(" -p "));
    }

    #[test]
    fn conda_wrap_command_with_prefix() {
        let backend = CondaBackend;
        let result = backend
            .wrap_command_with_opts("echo hi", "envs/qc.yaml", Some(".oxo-conda"))
            .unwrap();
        assert!(
            result.contains("conda run --no-capture-output -p .oxo-conda bash -c 'export PATH=\"$CONDA_PREFIX/bin:$PATH\"; echo hi'"),
            "expected -p prefix form, got: {result}"
        );
        assert!(result.contains("echo hi"));
    }

    #[test]
    fn conda_wrap_command_without_prefix() {
        let backend = CondaBackend;
        let result = backend
            .wrap_command_with_opts("echo hi", "envs/qc.yaml", None)
            .unwrap();
        assert!(result.contains("conda run --no-capture-output -n qc bash -c 'export PATH=\"$CONDA_PREFIX/bin:$PATH\"; echo hi'"));
        assert!(!result.contains(" -p "));
    }

    #[test]
    fn conda_teardown_command_with_prefix() {
        let backend = CondaBackend;
        let cmd = backend
            .teardown_command_with_opts("envs/qc.yaml", Some(".oxo-conda"))
            .unwrap()
            .unwrap();
        assert_eq!(cmd, "conda env remove -p .oxo-conda -y");
    }

    #[test]
    fn conda_teardown_command_without_prefix() {
        let backend = CondaBackend;
        let cmd = backend
            .teardown_command_with_opts("envs/qc.yaml", None)
            .unwrap()
            .unwrap();
        assert_eq!(cmd, "conda env remove -n qc -y");
    }

    #[test]
    fn conda_cache_key_with_prefix() {
        let backend = CondaBackend;
        let key = backend.cache_key_with_opts("envs/qc.yaml", Some(".oxo-conda"));
        assert_eq!(key, "conda:envs/qc.yaml:.oxo-conda");
    }

    #[test]
    fn conda_cache_key_without_prefix() {
        let backend = CondaBackend;
        let key = backend.cache_key_with_opts("envs/qc.yaml", None);
        assert_eq!(key, "conda:envs/qc.yaml");
    }

    // --- venv custom requirements tests ---------------------------------------

    #[test]
    fn venv_setup_command_with_custom_requirements() {
        let backend = VenvBackend;
        let cmd = backend
            .setup_command_with_reqs(".venv", Some("requirements-dev.txt"))
            .unwrap();
        assert!(cmd.contains("python3 -m venv .venv"));
        assert!(cmd.contains("pip install -r requirements-dev.txt"));
    }

    #[test]
    fn venv_setup_command_defaults_to_requirements_txt() {
        let backend = VenvBackend;
        let cmd = backend.setup_command_with_reqs(".venv", None).unwrap();
        assert!(cmd.contains("pip install -r requirements.txt"));
    }

    // --- modules init guard test ----------------------------------------------

    #[test]
    fn modules_wrap_command_has_init_error_guard() {
        let backend = ModulesBackend;
        let cmd = backend
            .wrap_command("echo test", "gcc/11.2", None, std::path::Path::new("."))
            .unwrap();
        assert!(
            cmd.contains("command -v module"),
            "expected init guard, got: {cmd}"
        );
        assert!(cmd.contains("module load gcc/11.2"));
    }

    // --- resolver integration tests -------------------------------------------

    #[test]
    fn resolver_conda_prefix_integration() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            conda: Some("envs/qc.yaml".to_string()),
            conda_prefix: Some(".oxo-conda".to_string()),
            ..Default::default()
        };
        let result = resolver
            .wrap_command("fastqc reads.fq", &spec, None, std::path::Path::new("."))
            .unwrap();
        assert!(result.contains("conda run --no-capture-output -p .oxo-conda bash -c 'export PATH=\"$CONDA_PREFIX/bin:$PATH\"; fastqc reads.fq'"));
        assert!(!result.contains(" -n "));
    }

    #[test]
    fn conda_verify_command_with_prefix() {
        let backend = CondaBackend;
        let cmd = backend
            .verify_command_with_opts("envs/qc.yaml", Some(".oxo-conda"))
            .unwrap()
            .unwrap();
        assert!(
            cmd.contains("conda run -p .oxo-conda bash -c 'test -d \"$CONDA_PREFIX/bin\" && ls \"$CONDA_PREFIX/bin\" | head -1 | grep -q .'"),
            "a prefix env must be verified with -p (the -n form checks a \
             named env that `conda env create -p` never creates) and the \
             check must require at least one entry in bin/, got: {cmd}"
        );
        assert!(!cmd.contains(" -n "));
    }

    #[test]
    fn conda_verify_command_without_prefix() {
        let backend = CondaBackend;
        let cmd = backend.verify_command("envs/qc.yaml").unwrap().unwrap();
        assert!(
            cmd.contains("conda run -n qc bash -c 'test -d \"$CONDA_PREFIX/bin\" && ls \"$CONDA_PREFIX/bin\" | head -1 | grep -q .'"),
            "the named-env verify must also require at least one entry in bin/ \
             (an empty bin/ is a broken env), got: {cmd}"
        );
        assert!(!cmd.contains(" -p "));
    }

    #[test]
    fn mamba_verify_command_with_prefix() {
        let backend = MambaBackend::new();
        let cmd = backend
            .verify_command_with_opts("envs/qc.yaml", Some(".oxo-conda"))
            .unwrap()
            .unwrap();
        assert!(
            cmd.contains(&format!(
                "{} run -p .oxo-conda bash -c 'test -d \"$CONDA_PREFIX/bin\" && ls \"$CONDA_PREFIX/bin\" | head -1 | grep -q .'",
                backend.binary
            )),
            "expected -p prefix form with a non-empty bin/ check, got: {cmd}"
        );
        assert!(!cmd.contains(" -n "));
    }

    #[test]
    fn resolver_verify_command_uses_prefix_for_conda_prefix_env() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            conda: Some("envs/qc.yaml".to_string()),
            conda_prefix: Some(".oxo-conda".to_string()),
            ..Default::default()
        };
        let cmd = resolver.verify_command(&spec).unwrap().unwrap();
        assert!(
            cmd.contains("conda run -p .oxo-conda bash -c 'test -d \"$CONDA_PREFIX/bin\" && ls \"$CONDA_PREFIX/bin\" | head -1 | grep -q .'"),
            "the named-env verify checks the WRONG env for a prefix install, got: {cmd}"
        );
        assert!(!cmd.contains(" -n "));
    }

    #[test]
    fn teardown_skips_preexisting_prefix_env() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("env");
        std::fs::create_dir_all(&prefix).unwrap();
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            conda: Some("envs/qc.yaml".to_string()),
            conda_prefix: Some(prefix.to_string_lossy().into_owned()),
            ..Default::default()
        };
        // The cold-cache flow: verify failed, setup was issued (its update
        // fallback exits 0), verify failed again — teardown must refuse to
        // remove an env that pre-existed before this run's setup.
        let _setup = resolver.setup_command(&spec).unwrap();
        assert!(
            resolver.teardown_command(&spec).unwrap().is_none(),
            "a pre-existing prefix env (the user's own) must never be torn down"
        );
        assert!(
            prefix.exists(),
            "the user's pre-existing prefix env must survive"
        );
    }

    #[test]
    fn teardown_skips_when_verify_failed_before_any_setup() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("env");
        std::fs::create_dir_all(&prefix).unwrap();
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            conda: Some("envs/qc.yaml".to_string()),
            conda_prefix: Some(prefix.to_string_lossy().into_owned()),
            ..Default::default()
        };
        // Verify failed before any setup was issued this run — teardown must
        // not remove the env.
        assert!(resolver.teardown_command(&spec).unwrap().is_none());
        assert!(prefix.exists());
    }

    #[test]
    fn teardown_removes_env_created_this_run() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("env"); // does not exist before setup
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            conda: Some("envs/qc.yaml".to_string()),
            conda_prefix: Some(prefix.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let _setup = resolver.setup_command(&spec).unwrap();
        let cmd = resolver
            .teardown_command(&spec)
            .unwrap()
            .expect("an env this run created may be torn down");
        assert!(cmd.contains("conda env remove -p"));
        assert!(cmd.contains(&prefix.to_string_lossy().into_owned()));
    }

    #[test]
    fn resolver_venv_custom_requirements_integration() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            venv: Some(".venv".to_string()),
            venv_requirements: Some("requirements-test.txt".to_string()),
            ..Default::default()
        };
        let cmd = resolver.setup_command(&spec).unwrap();
        assert!(cmd.contains("pip install -r requirements-test.txt"));
    }

    #[test]
    fn singularity_no_memory_flag() {
        let backend = SingularityBackend::new();
        let resources = crate::rule::Resources {
            memory: Some("32g".to_string()),
            ..Default::default()
        };
        let result = backend
            .wrap_command(
                "bwa mem ref.fa reads.fq",
                "image.sif",
                Some(&resources),
                std::path::Path::new("."),
            )
            .unwrap();
        assert!(!result.contains("--memory"));
    }
}
