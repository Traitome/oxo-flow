//! Environment management for oxo-flow.
//!
//! Provides a trait-based abstraction for different software environment
//! managers (conda, pixi, docker, singularity, venv) and a resolver that
//! selects the appropriate backend for each rule.

use crate::error::{OxoFlowError, Result};
use crate::rule::EnvironmentSpec;

/// Trait for environment backends.
///
/// Each backend (conda, docker, etc.) implements this trait to provide
/// environment detection, creation, and command wrapping.
pub trait EnvironmentBackend: Send + Sync {
    /// Returns the name of this environment type.
    fn name(&self) -> &str;

    /// Check if this environment backend is available on the system.
    fn is_available(&self) -> bool;

    /// Wrap a shell command to run inside this environment.
    fn wrap_command(&self, command: &str, spec: &str) -> Result<String>;
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

    fn wrap_command(&self, command: &str, spec: &str) -> Result<String> {
        // spec is the conda environment YAML file or environment name
        Ok(format!(
            "conda run --no-banner -n $(conda env list --json | python3 -c \"import sys,json; print(next((e.split('/')[-1] for e in json.load(sys.stdin)['envs'] if '{spec}' in e), '{spec}'))\") {command}"
        ))
    }
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

    fn wrap_command(&self, command: &str, spec: &str) -> Result<String> {
        let workdir = std::env::current_dir()
            .unwrap_or_default()
            .display()
            .to_string();
        Ok(format!(
            "docker run --rm -v {workdir}:{workdir} -w {workdir} {spec} sh -c '{command}'"
        ))
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

    fn wrap_command(&self, command: &str, spec: &str) -> Result<String> {
        let workdir = std::env::current_dir()
            .unwrap_or_default()
            .display()
            .to_string();
        Ok(format!(
            "singularity exec --bind {workdir}:{workdir} {spec} sh -c '{command}'"
        ))
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

    fn wrap_command(&self, command: &str, spec: &str) -> Result<String> {
        Ok(format!("source {spec}/bin/activate && {command}"))
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

    fn wrap_command(&self, command: &str, spec: &str) -> Result<String> {
        Ok(format!("pixi run -e {spec} {command}"))
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

    fn wrap_command(&self, command: &str, _spec: &str) -> Result<String> {
        Ok(command.to_string())
    }
}

/// Resolves the appropriate environment backend for a rule's environment spec.
pub struct EnvironmentResolver {
    conda: CondaBackend,
    docker: DockerBackend,
    singularity: SingularityBackend,
    venv: VenvBackend,
    pixi: PixiBackend,
    system: SystemBackend,
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
            system: SystemBackend,
        }
    }

    /// Wrap a command using the appropriate environment backend.
    pub fn wrap_command(&self, command: &str, env_spec: &EnvironmentSpec) -> Result<String> {
        if let Some(ref conda) = env_spec.conda {
            return self.conda.wrap_command(command, conda);
        }
        if let Some(ref pixi) = env_spec.pixi {
            return self.pixi.wrap_command(command, pixi);
        }
        if let Some(ref docker) = env_spec.docker {
            return self.docker.wrap_command(command, docker);
        }
        if let Some(ref singularity) = env_spec.singularity {
            return self.singularity.wrap_command(command, singularity);
        }
        if let Some(ref venv) = env_spec.venv {
            return self.venv.wrap_command(command, venv);
        }
        self.system.wrap_command(command, "")
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
        available
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

    #[test]
    fn system_backend_always_available() {
        let backend = SystemBackend;
        assert!(backend.is_available());
        assert_eq!(backend.name(), "system");
    }

    #[test]
    fn system_backend_passthrough() {
        let backend = SystemBackend;
        let result = backend.wrap_command("echo hello", "").unwrap();
        assert_eq!(result, "echo hello");
    }

    #[test]
    fn docker_wrap_command() {
        let backend = DockerBackend;
        let result = backend
            .wrap_command("bwa mem ref.fa reads.fq", "biocontainers/bwa:0.7.17")
            .unwrap();
        assert!(result.contains("docker run"));
        assert!(result.contains("biocontainers/bwa:0.7.17"));
    }

    #[test]
    fn singularity_wrap_command() {
        let backend = SingularityBackend;
        let result = backend
            .wrap_command("samtools sort input.bam", "image.sif")
            .unwrap();
        assert!(result.contains("singularity exec"));
        assert!(result.contains("image.sif"));
    }

    #[test]
    fn resolver_empty_spec_uses_system() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec::default();
        let result = resolver.wrap_command("echo test", &spec).unwrap();
        assert_eq!(result, "echo test");
    }

    #[test]
    fn resolver_docker_spec() {
        let resolver = EnvironmentResolver::new();
        let spec = EnvironmentSpec {
            docker: Some("ubuntu:22.04".to_string()),
            ..Default::default()
        };
        let result = resolver.wrap_command("echo test", &spec).unwrap();
        assert!(result.contains("docker run"));
        assert!(result.contains("ubuntu:22.04"));
    }

    #[test]
    fn resolver_available_backends() {
        let resolver = EnvironmentResolver::new();
        let available = resolver.available_backends();
        assert!(available.contains(&"system"));
    }
}
