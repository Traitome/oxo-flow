//! Environment management for oxo-flow.
//!
//! Provides a trait-based abstraction for different software environment
//! managers (conda, pixi, docker, singularity, venv) and a resolver that
//! selects the appropriate backend for each rule.

use std::collections::HashSet;
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
    fn wrap_command(
        &self,
        command: &str,
        spec: &str,
        _resources: Option<&crate::rule::Resources>,
    ) -> Result<String>;

    /// Return the shell command to set up / create this environment.
    fn setup_command(&self, spec: &str) -> Result<String>;

    /// Return the shell command to tear down / remove this environment,
    /// or `None` if no cleanup is needed.
    fn teardown_command(&self, spec: &str) -> Result<Option<String>>;

    /// Return a cache key that uniquely identifies this environment
    /// configuration so it can be reused across rules.
    fn cache_key(&self, spec: &str) -> String;
}

/// Read a conda YAML spec and extract the `name:` field, falling back to file stem.
fn conda_env_name_from_spec(spec: &str) -> String {
    // Try reading the YAML file to extract `name:` field
    if let Ok(content) = std::fs::read_to_string(spec) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("name:") || trimmed.starts_with("name :") {
                let name = trimmed
                    .split_once(':')
                    .map(|(_, v)| v)
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    // Fall back to file stem
    std::path::Path::new(spec)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(spec)
        .to_string()
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
    ) -> Result<String> {
        let env_name = conda_env_name_from_spec(spec);
        let escaped = escape_for_sh_single_quote(command);
        Ok(format!("conda run -n {env_name} bash -c '{escaped}'"))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        Ok(format!(
            "conda env create -f {spec} 2>/dev/null || conda env update -f {spec} --prune"
        ))
    }

    fn teardown_command(&self, spec: &str) -> Result<Option<String>> {
        let env_name = conda_env_name_from_spec(spec);
        Ok(Some(format!("conda env remove -n {env_name} -y")))
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
        if let Some(prefix) = prefix {
            Ok(format!("conda run -p {prefix} bash -c '{escaped}'"))
        } else {
            let env_name = conda_env_name_from_spec(spec);
            Ok(format!("conda run -n {env_name} bash -c '{escaped}'"))
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

    /// Cache key with optional project-local prefix.
    pub fn cache_key_with_opts(&self, spec: &str, prefix: Option<&str>) -> String {
        if let Some(prefix) = prefix {
            format!("conda:{spec}:{prefix}")
        } else {
            self.cache_key(spec)
        }
    }
}

/// Escape a string for safe embedding inside a `sh -c '...'` invocation.
///
/// Replaces every `'` with `'\''` (close quote, escaped literal quote, reopen quote)
/// so the value is safe regardless of what shell interprets the outer wrapper.
fn escape_for_sh_single_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Docker environment backend.
#[derive(Debug, Default)]
pub struct DockerBackend;

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
    ) -> Result<String> {
        let workdir = std::env::current_dir()
            .unwrap_or_default()
            .display()
            .to_string();
        let escaped_cmd = escape_for_sh_single_quote(command);

        let mut mem_arg = String::new();
        if let Some(res) = resources
            && let Some(mem) = &res.memory
        {
            mem_arg = format!(" --memory {mem}");
        }

        Ok(format!(
            "docker run --rm --user $(id -u):$(id -g){mem_arg} -v {workdir}:{workdir} -w {workdir} {spec} sh -c '{escaped_cmd}'"
        ))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        Ok(format!("docker pull {spec}"))
    }

    fn teardown_command(&self, _spec: &str) -> Result<Option<String>> {
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
    ) -> Result<String> {
        let workdir = std::env::current_dir()
            .unwrap_or_default()
            .display()
            .to_string();
        let escaped_cmd = escape_for_sh_single_quote(command);

        Ok(format!(
            "{} exec --bind {workdir}:{workdir} {spec} sh -c '{escaped_cmd}'",
            self.binary
        ))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        Ok(format!("{} pull {spec}", self.binary))
    }

    fn teardown_command(&self, _spec: &str) -> Result<Option<String>> {
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
        Ok(format!(
            "python3 -m venv {spec} && source {spec}/bin/activate && pip install -r {reqs}"
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
    ) -> Result<String> {
        Ok(format!("pixi run -e {spec} {command}"))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        Ok(format!("pixi install -e {spec}"))
    }

    fn teardown_command(&self, _spec: &str) -> Result<Option<String>> {
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
    conda: CondaBackend,
    docker: DockerBackend,
    singularity: SingularityBackend,
    venv: VenvBackend,
    pixi: PixiBackend,
    modules: ModulesBackend,
    system: SystemBackend,
    cache: Arc<Mutex<EnvironmentCache>>,
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
            conda: CondaBackend,
            docker: DockerBackend,
            singularity: SingularityBackend::new(),
            venv: VenvBackend,
            pixi: PixiBackend,
            modules: ModulesBackend,
            system: SystemBackend,
            cache: Arc::new(Mutex::new(EnvironmentCache::new())),
        }
    }

    /// Create a new environment resolver with persistent cache directory.
    pub fn with_cache_dir(cache_dir: &std::path::Path) -> Self {
        Self {
            conda: CondaBackend,
            docker: DockerBackend,
            singularity: SingularityBackend::new(),
            venv: VenvBackend,
            pixi: PixiBackend,
            modules: ModulesBackend,
            system: SystemBackend,
            cache: Arc::new(Mutex::new(EnvironmentCache::with_cache_dir(cache_dir))),
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

    /// Wrap a command using the appropriate environment backend.
    pub fn wrap_command(
        &self,
        command: &str,
        env_spec: &EnvironmentSpec,
        resources: Option<&crate::rule::Resources>,
    ) -> Result<String> {
        if let Some(ref conda) = env_spec.conda {
            return self.conda.wrap_command_with_opts(
                command,
                conda,
                env_spec.conda_prefix.as_deref(),
            );
        }
        if let Some(ref pixi) = env_spec.pixi {
            return self.pixi.wrap_command(command, pixi, resources);
        }
        if let Some(ref docker) = env_spec.docker {
            return self.docker.wrap_command(command, docker, resources);
        }
        if let Some(ref singularity) = env_spec.singularity {
            return self
                .singularity
                .wrap_command(command, singularity, resources);
        }
        if let Some(ref venv) = env_spec.venv {
            return self.venv.wrap_command(command, venv, resources);
        }
        if !env_spec.modules.is_empty() {
            let spec = env_spec.modules.join(",");
            return self.modules.wrap_command(command, &spec, resources);
        }
        self.system.wrap_command(command, "", resources)
    }

    /// Get the cache key for an environment specification.
    /// Used to track whether an environment has already been set up.
    pub fn cache_key(&self, env_spec: &EnvironmentSpec) -> String {
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
    pub fn setup_command(&self, env_spec: &EnvironmentSpec) -> Result<String> {
        if let Some(ref conda) = env_spec.conda {
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

    /// Check which environment backends are available on the system.
    pub fn available_backends(&self) -> Vec<&str> {
        let mut available = vec!["system"];
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
        &["conda", "pixi", "docker", "singularity", "venv", "modules"]
    }

    /// Validate that the required environment backend is available for a spec.
    pub fn validate_spec(&self, env_spec: &EnvironmentSpec) -> Result<()> {
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
        let result = backend.wrap_command("echo hello", "", None).unwrap();
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
        assert!(cmd.contains("conda env create -f envs/qc.yaml"));
        assert!(cmd.contains("conda env update -f envs/qc.yaml --prune"));
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

    // ── DockerBackend ──────────────────────────────────────────────

    #[test]
    fn docker_wrap_command() {
        let backend = DockerBackend;
        let result = backend
            .wrap_command("bwa mem ref.fa reads.fq", "biocontainers/bwa:0.7.17", None)
            .unwrap();
        assert!(result.contains("docker run"));
        assert!(result.contains("--user $(id -u):$(id -g)"));
        assert!(result.contains("biocontainers/bwa:0.7.17"));
    }

    #[test]
    fn docker_setup_command() {
        let backend = DockerBackend;
        let cmd = backend.setup_command("biocontainers/bwa:0.7.17").unwrap();
        assert_eq!(cmd, "docker pull biocontainers/bwa:0.7.17");
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
            .wrap_command("samtools sort input.bam", "image.sif", None)
            .unwrap();
        assert!(result.contains(" exec "));
        assert!(!result.contains("--memory"));
        assert!(result.contains("image.sif"));
    }

    #[test]
    fn singularity_setup_command() {
        let backend = SingularityBackend::new();
        let cmd = backend.setup_command("docker://ubuntu:22.04").unwrap();
        assert!(cmd.contains(" pull docker://ubuntu:22.04"));
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
        let cmd = backend.setup_command("default").unwrap();
        assert_eq!(cmd, "pixi install -e default");
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

    // ── EnvironmentResolver ────────────────────────────────────────

    #[test]
    fn resolver_empty_spec_uses_system() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec::default();
        let result = resolver.wrap_command("echo test", &spec, None).unwrap();
        assert_eq!(result, "echo test");
    }

    #[test]
    fn resolver_docker_spec() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            docker: Some("ubuntu:22.04".to_string()),
            ..Default::default()
        };
        let result = resolver.wrap_command("echo test", &spec, None).unwrap();
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
            .wrap_command("fastqc reads.fq", "envs/qc.yaml", None)
            .unwrap();
        assert!(
            result.contains("conda run"),
            "expected 'conda run' in: {result}"
        );
        assert!(result.contains("fastqc reads.fq"));
    }

    #[test]
    fn venv_wrap_command() {
        let backend = VenvBackend;
        let result = backend.wrap_command("pip list", ".venv", None).unwrap();
        assert!(result.contains("source .venv/bin/activate"));
        assert!(result.contains("pip list"));
    }

    #[test]
    fn pixi_wrap_command() {
        let backend = PixiBackend;
        let result = backend
            .wrap_command("python main.py", "default", None)
            .unwrap();
        assert_eq!(result, "pixi run -e default python main.py");
    }

    #[test]
    fn resolver_wraps_conda_spec() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            conda: Some("envs/qc.yaml".to_string()),
            ..Default::default()
        };
        let result = resolver
            .wrap_command("fastqc reads.fq", &spec, None)
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
            .wrap_command("bwa mem ref.fa reads.fq", &spec, None)
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
            .wrap_command("java -jar gatk.jar", "java/11,gatk/4.2", None)
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
        assert!(cmd.contains("conda env create -f envs/qc.yaml"));
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
            result.contains("conda run -p .oxo-conda"),
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
        assert!(result.contains("conda run -n qc"));
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
        let cmd = backend.wrap_command("echo test", "gcc/11.2", None).unwrap();
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
            .wrap_command("fastqc reads.fq", &spec, None)
            .unwrap();
        assert!(result.contains("conda run -p .oxo-conda"));
        assert!(!result.contains(" -n "));
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
            .wrap_command("bwa mem ref.fa reads.fq", "image.sif", Some(&resources))
            .unwrap();
        assert!(!result.contains("--memory"));
    }
}
