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
        // Derive the environment name from the YAML file stem
        // (e.g., "envs/bwa_mem2.yaml" → "bwa_mem2", "my_env" → "my_env").
        let env_name = std::path::Path::new(spec)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(spec);
        Ok(format!("conda run --no-banner -n {env_name} {command}"))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        Ok(format!("conda env create -f {spec}"))
    }

    fn teardown_command(&self, spec: &str) -> Result<Option<String>> {
        // Derive env name from the YAML filename (strip path & extension).
        let env_name = std::path::Path::new(spec)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(spec);
        Ok(Some(format!("conda env remove -n {env_name} -y")))
    }

    fn cache_key(&self, spec: &str) -> String {
        format!("conda:{spec}")
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
            "docker run --rm{mem_arg} -v {workdir}:{workdir} -w {workdir} {spec} sh -c '{escaped_cmd}'"
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
#[derive(Debug, Default)]
pub struct SingularityBackend;

impl EnvironmentBackend for SingularityBackend {
    fn name(&self) -> &str {
        "singularity"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("singularity")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
            || std::process::Command::new("apptainer")
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
            "singularity exec{mem_arg} --bind {workdir}:{workdir} {spec} sh -c '{escaped_cmd}'"
        ))
    }

    fn setup_command(&self, spec: &str) -> Result<String> {
        Ok(format!("singularity pull {spec}"))
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
        Ok(format!("module load {modules} && {command}"))
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
            if let Some(parent) = path.parent() {
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
            singularity: SingularityBackend,
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
            singularity: SingularityBackend,
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
            return self.conda.wrap_command(command, conda, resources);
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
            return self.conda.cache_key(conda);
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
            return self.conda.setup_command(conda);
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
            return self.venv.setup_command(venv);
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
        if env_spec.pixi.is_some() && !self.pixi.is_available() {
            return Err(OxoFlowError::Environment {
                kind: "pixi".to_string(),
                message: "pixi is not installed or not in PATH".to_string(),
            });
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
        assert_eq!(cmd, "conda env create -f envs/qc.yaml");
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
        let backend = SingularityBackend;
        let result = backend
            .wrap_command("samtools sort input.bam", "image.sif", None)
            .unwrap();
        assert!(result.contains("singularity exec"));
        assert!(result.contains("image.sif"));
    }

    #[test]
    fn singularity_setup_command() {
        let backend = SingularityBackend;
        let cmd = backend.setup_command("docker://ubuntu:22.04").unwrap();
        assert_eq!(cmd, "singularity pull docker://ubuntu:22.04");
    }

    #[test]
    fn singularity_teardown_is_noop() {
        let backend = SingularityBackend;
        assert!(backend.teardown_command("image.sif").unwrap().is_none());
    }

    #[test]
    fn singularity_cache_key() {
        let backend = SingularityBackend;
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
}
