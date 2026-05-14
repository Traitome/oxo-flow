//! Container build and packaging utilities.
//!
//! Provides functionality to package oxo-flow workflows into self-contained
//! container images (Docker/Singularity) for portable execution.

use crate::config::WorkflowConfig;
use crate::error::Result;

/// Container format to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
    /// Dockerfile for Docker/Podman builds.
    Docker,
    /// Singularity definition file.
    Singularity,
}

impl std::fmt::Display for ContainerFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Docker => write!(f, "docker"),
            Self::Singularity => write!(f, "singularity"),
        }
    }
}

/// Configuration for container packaging.
#[derive(Debug, Clone)]
pub struct PackageConfig {
    /// Container format to generate.
    pub format: ContainerFormat,

    /// Base image to use.
    pub base_image: String,

    /// Whether to include reference data.
    pub include_data: bool,

    /// Additional labels/metadata.
    pub labels: Vec<(String, String)>,

    /// Additional system packages to install (e.g., "samtools", "bcftools").
    pub extra_packages: Vec<String>,

    /// Enable multi-stage Docker builds to reduce final image size.
    pub multi_stage: bool,

    /// Run container as a non-root user for improved security.
    pub rootless: bool,

    /// Custom HEALTHCHECK command for the container.
    pub healthcheck: Option<String>,
}

impl Default for PackageConfig {
    fn default() -> Self {
        Self {
            format: ContainerFormat::Docker,
            base_image: "ubuntu:22.04".to_string(),
            include_data: false,
            labels: Vec::new(),
            extra_packages: Vec::new(),
            multi_stage: false,
            rootless: true,
            healthcheck: None,
        }
    }
}

/// Generate a `docker run` command string with resource limits.
#[must_use]
pub fn generate_docker_run_command(
    image_name: &str,
    resources: &crate::rule::Resources,
    workdir: &str,
) -> String {
    let mut cmd = "docker run --rm".to_string();
    if let Some(mem) = &resources.memory {
        cmd.push_str(&format!(" --memory={mem}"));
    }
    cmd.push_str(&format!(" --cpus={}", resources.threads));

    // GPU support - add --gpus flag if GPU resources specified
    if let Some(gpu_count) = resources.gpu {
        cmd.push_str(&format!(" --gpus {}", gpu_count));
    }
    if let Some(ref spec) = resources.gpu_spec {
        // Use gpu_spec if available (more specific)
        match &spec.model {
            Some(model) => {
                cmd.push_str(&format!(
                    " --gpus \"device={},count={}\"",
                    model, spec.count
                ));
            }
            None => {
                cmd.push_str(&format!(" --gpus {}", spec.count));
            }
        }
    }

    cmd.push_str(&format!(" -v {workdir}:/data -w /data"));
    cmd.push_str(&format!(" {image_name}"));
    cmd
}

/// Write the environment-installation instructions shared by both single-stage
/// and multi-stage Dockerfiles.
fn write_env_setup(
    dockerfile: &mut String,
    workflow: &WorkflowConfig,
    needs_conda: bool,
    needs_pixi: bool,
) {
    if needs_conda {
        dockerfile.push_str("# Install Miniforge (conda)\n");
        dockerfile.push_str("RUN curl -L -O https://github.com/conda-forge/miniforge/releases/latest/download/Miniforge3-Linux-x86_64.sh \\\n");
        dockerfile.push_str("    && bash Miniforge3-Linux-x86_64.sh -b -p /opt/conda \\\n");
        dockerfile.push_str("    && rm Miniforge3-Linux-x86_64.sh\n");
        dockerfile.push_str("ENV PATH=/opt/conda/bin:$PATH\n\n");

        for rule in &workflow.rules {
            if let Some(ref conda_env) = rule.environment.conda {
                dockerfile.push_str(&format!("# Conda environment for rule: {}\n", rule.name));
                dockerfile.push_str(&format!("COPY {conda_env} /workflow/envs/\n"));
                let env_filename = std::path::Path::new(conda_env)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| conda_env.clone());
                dockerfile.push_str(&format!(
                    "RUN conda env create -f /workflow/envs/{env_filename} -n {}\n\n",
                    rule.name
                ));
            }
        }
    }

    if needs_pixi {
        dockerfile.push_str("# Install pixi\n");
        dockerfile.push_str("RUN curl -fsSL https://pixi.sh/install.sh | bash \\\n");
        dockerfile.push_str("    && mv /root/.pixi/bin/pixi /usr/local/bin/pixi\n");
        dockerfile.push_str("ENV PATH=/usr/local/bin:$PATH\n\n");

        for rule in &workflow.rules {
            if let Some(ref pixi_env) = rule.environment.pixi {
                dockerfile.push_str(&format!("# Pixi environment for rule: {}\n", rule.name));
                dockerfile.push_str(&format!("COPY {pixi_env} /workflow/envs/\n"));
                let env_filename = std::path::Path::new(pixi_env)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| pixi_env.clone());
                dockerfile.push_str(&format!(
                    "RUN cd /workflow/envs && pixi install --manifest-path {env_filename}\n\n"
                ));
            }
        }
    }
}

/// Collect which environment managers the workflow rules require.
fn collect_env_requirements(workflow: &WorkflowConfig) -> (bool, bool, Vec<String>) {
    let mut needs_conda = false;
    let mut needs_pixi = false;
    let mut docker_images = Vec::new();

    for rule in &workflow.rules {
        if rule.environment.conda.is_some() {
            needs_conda = true;
        }
        if rule.environment.pixi.is_some() {
            needs_pixi = true;
        }
        if let Some(ref img) = rule.environment.docker
            && !docker_images.contains(img)
        {
            docker_images.push(img.clone());
        }
    }

    (needs_conda, needs_pixi, docker_images)
}

/// Check if workflow requires GPU resources and return GPU info.
fn detect_gpu_requirements(workflow: &WorkflowConfig) -> Option<crate::rule::GpuSpec> {
    for rule in &workflow.rules {
        // Check for direct GPU spec
        if let Some(ref spec) = rule.resources.gpu_spec {
            return Some(spec.clone());
        }
        // Check for simple GPU count
        if let Some(count) = rule.resources.gpu {
            return Some(crate::rule::GpuSpec {
                count,
                model: None,
                memory_gb: None,
                compute_capability: None,
            });
        }
    }
    None
}

/// Determine the appropriate base image, using CUDA for GPU workflows.
fn select_base_image(workflow: &WorkflowConfig, config: &PackageConfig) -> String {
    // If user explicitly set a base image, respect it
    if config.base_image != "ubuntu:22.04" {
        return config.base_image.clone();
    }

    // Auto-select CUDA base image for GPU workflows
    if let Some(ref gpu_spec) = detect_gpu_requirements(workflow) {
        // Use CUDA runtime base image
        // nvidia/cuda:12.0.0-runtime-ubuntu22.04 is a common choice
        match &gpu_spec.compute_capability {
            Some(cap) if cap.starts_with('8') => {
                // For compute capability 8.x (A100, RTX 30/40 series), use CUDA 12
                "nvidia/cuda:12.0.0-runtime-ubuntu22.04".to_string()
            }
            Some(cap) if cap.starts_with('7') => {
                // For compute capability 7.x (V100, T4), use CUDA 11.8
                "nvidia/cuda:11.8.0-runtime-ubuntu22.04".to_string()
            }
            _ => {
                // Default to CUDA 12 for modern GPUs
                "nvidia/cuda:12.0.0-runtime-ubuntu22.04".to_string()
            }
        }
    } else {
        config.base_image.clone()
    }
}

/// Generate a Dockerfile from a workflow configuration.
pub fn generate_dockerfile(workflow: &WorkflowConfig, config: &PackageConfig) -> Result<String> {
    if config.multi_stage {
        generate_multistage_dockerfile(workflow, config)
    } else {
        generate_singlestage_dockerfile(workflow, config)
    }
}

/// Generate a single-stage Dockerfile.
fn generate_singlestage_dockerfile(
    workflow: &WorkflowConfig,
    config: &PackageConfig,
) -> Result<String> {
    let mut dockerfile = String::new();

    // Select appropriate base image (CUDA for GPU workflows)
    let base_image = select_base_image(workflow, config);

    // Header
    dockerfile.push_str(&format!(
        "# Auto-generated by oxo-flow for workflow: {}\n",
        workflow.workflow.name
    ));
    dockerfile.push_str(&format!("FROM {}\n\n", base_image));

    // Add NVIDIA environment variables for CUDA containers
    if detect_gpu_requirements(workflow).is_some() {
        dockerfile.push_str("# NVIDIA GPU environment\n");
        dockerfile.push_str("ENV NVIDIA_VISIBLE_DEVICES=all\n");
        dockerfile.push_str("ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility\n\n");
    }

    // Labels
    write_labels(&mut dockerfile, workflow, config);

    // Install system dependencies
    write_system_deps(&mut dockerfile, config);

    // Environment setup (conda / pixi)
    let (needs_conda, needs_pixi, _docker_images) = collect_env_requirements(workflow);
    write_env_setup(&mut dockerfile, workflow, needs_conda, needs_pixi);

    // Copy workflow files
    dockerfile.push_str("# Copy workflow\n");
    dockerfile.push_str("WORKDIR /workflow\n");
    dockerfile.push_str("COPY . /workflow/\n\n");

    // Include reference data if configured
    if config.include_data {
        dockerfile.push_str("# Include reference data\n");
        dockerfile.push_str("COPY data/ /workflow/data/\n\n");
    }

    // Rootless user
    if config.rootless {
        write_rootless(&mut dockerfile);
    }

    // Entry point
    dockerfile.push_str("# Default entrypoint\n");
    dockerfile.push_str("ENTRYPOINT [\"oxo-flow\", \"run\"]\n\n");

    // Health check
    write_healthcheck(&mut dockerfile, config);

    Ok(dockerfile)
}

/// Generate a multi-stage Dockerfile that separates build and runtime stages.
fn generate_multistage_dockerfile(
    workflow: &WorkflowConfig,
    config: &PackageConfig,
) -> Result<String> {
    let mut dockerfile = String::new();

    // Select appropriate base image (CUDA for GPU workflows)
    let base_image = select_base_image(workflow, config);
    let has_gpu = detect_gpu_requirements(workflow).is_some();

    // Header
    dockerfile.push_str(&format!(
        "# Auto-generated by oxo-flow for workflow: {}\n",
        workflow.workflow.name
    ));

    // ── Stage 1: builder ──
    dockerfile.push_str(&format!("FROM {} AS builder\n\n", base_image));

    // Add NVIDIA environment variables for CUDA containers
    if has_gpu {
        dockerfile.push_str("# NVIDIA GPU environment\n");
        dockerfile.push_str("ENV NVIDIA_VISIBLE_DEVICES=all\n");
        dockerfile.push_str("ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility\n\n");
    }

    // Install build dependencies
    write_system_deps(&mut dockerfile, config);

    // Environment setup (conda / pixi)
    let (needs_conda, needs_pixi, _docker_images) = collect_env_requirements(workflow);
    write_env_setup(&mut dockerfile, workflow, needs_conda, needs_pixi);

    // Copy workflow files into builder
    dockerfile.push_str("# Copy workflow\n");
    dockerfile.push_str("WORKDIR /workflow\n");
    dockerfile.push_str("COPY . /workflow/\n\n");

    // ── Stage 2: runtime ──
    dockerfile.push_str(&format!("FROM {}\n\n", base_image));

    // NVIDIA environment for runtime stage
    if has_gpu {
        dockerfile.push_str("# NVIDIA GPU environment\n");
        dockerfile.push_str("ENV NVIDIA_VISIBLE_DEVICES=all\n");
        dockerfile.push_str("ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility\n\n");
    }

    // Labels go on the final image
    write_labels(&mut dockerfile, workflow, config);

    // Copy installed environments from builder
    if needs_conda {
        dockerfile.push_str("# Copy conda from builder\n");
        dockerfile.push_str("COPY --from=builder /opt/conda /opt/conda\n");
        dockerfile.push_str("ENV PATH=/opt/conda/bin:$PATH\n\n");
    }
    if needs_pixi {
        dockerfile.push_str("# Copy pixi from builder\n");
        dockerfile.push_str("COPY --from=builder /usr/local/bin/pixi /usr/local/bin/pixi\n");
        dockerfile.push_str("ENV PATH=/usr/local/bin:$PATH\n\n");
    }

    // Copy workflow files from builder
    dockerfile.push_str("# Copy workflow from builder\n");
    dockerfile.push_str("COPY --from=builder /workflow /workflow\n");
    dockerfile.push_str("WORKDIR /workflow\n\n");

    // Include reference data if configured
    if config.include_data {
        dockerfile.push_str("# Include reference data\n");
        dockerfile.push_str("COPY data/ /workflow/data/\n\n");
    }

    // Rootless user
    if config.rootless {
        write_rootless(&mut dockerfile);
    }

    // Entry point
    dockerfile.push_str("# Default entrypoint\n");
    dockerfile.push_str("ENTRYPOINT [\"oxo-flow\", \"run\"]\n\n");

    // Health check
    write_healthcheck(&mut dockerfile, config);

    Ok(dockerfile)
}

/// Write OCI labels to the Dockerfile.
fn write_labels(dockerfile: &mut String, workflow: &WorkflowConfig, config: &PackageConfig) {
    dockerfile.push_str(&format!(
        "LABEL org.opencontainers.image.title=\"{}\"\n",
        workflow.workflow.name
    ));
    dockerfile.push_str(&format!(
        "LABEL org.opencontainers.image.version=\"{}\"\n",
        workflow.workflow.version
    ));
    for (key, value) in &config.labels {
        dockerfile.push_str(&format!("LABEL {key}=\"{value}\"\n"));
    }
    dockerfile.push('\n');
}

/// Write system dependency installation to the Dockerfile.
fn write_system_deps(dockerfile: &mut String, config: &PackageConfig) {
    dockerfile.push_str("# System dependencies\n");
    dockerfile.push_str("RUN apt-get update && apt-get install -y \\\n");
    dockerfile.push_str("    curl wget git build-essential \\\n");
    if !config.extra_packages.is_empty() {
        dockerfile.push_str(&format!("    {} \\\n", config.extra_packages.join(" ")));
    }
    dockerfile.push_str("    && rm -rf /var/lib/apt/lists/*\n\n");
}

/// Write rootless user directives to the Dockerfile.
fn write_rootless(dockerfile: &mut String) {
    dockerfile.push_str("# Run as non-root user\n");
    dockerfile.push_str(
        "RUN groupadd -r oxoflow && useradd -r -g oxoflow -d /home/oxoflow -s /bin/bash oxoflow\n",
    );
    dockerfile.push_str("USER oxoflow\n");
    dockerfile.push_str("WORKDIR /home/oxoflow\n\n");
}

/// Write the HEALTHCHECK directive to the Dockerfile.
fn write_healthcheck(dockerfile: &mut String, config: &PackageConfig) {
    match &config.healthcheck {
        Some(cmd) => {
            dockerfile.push_str(&format!("HEALTHCHECK CMD {cmd}\n"));
        }
        None => {
            dockerfile.push_str("HEALTHCHECK CMD [\"oxo-flow\", \"--version\"]\n");
        }
    }
}

/// Generate a Singularity definition file from a workflow configuration.
pub fn generate_singularity_def(
    workflow: &WorkflowConfig,
    config: &PackageConfig,
) -> Result<String> {
    let mut def = String::new();

    def.push_str(&format!(
        "# Auto-generated by oxo-flow for workflow: {}\n",
        workflow.workflow.name
    ));
    def.push_str("Bootstrap: docker\n");
    def.push_str(&format!("From: {}\n\n", config.base_image));

    def.push_str("%labels\n");
    def.push_str(&format!(
        "    Author {}\n",
        workflow.workflow.author.as_deref().unwrap_or("oxo-flow")
    ));
    def.push_str(&format!("    Version {}\n", workflow.workflow.version));
    def.push_str(&format!(
        "    Description {}\n\n",
        workflow
            .workflow
            .description
            .as_deref()
            .unwrap_or("oxo-flow workflow")
    ));

    // Collect environment requirements
    let mut needs_conda = false;
    let mut needs_pixi = false;
    for rule in &workflow.rules {
        if rule.environment.conda.is_some() {
            needs_conda = true;
        }
        if rule.environment.pixi.is_some() {
            needs_pixi = true;
        }
    }

    // Files section (data + environment specs)
    if config.include_data {
        def.push_str("%files\n");
        def.push_str("    data/ /workflow/data/\n\n");
    }

    // Post section: install system deps, conda, pixi
    def.push_str("%post\n");
    let mut apt_packages = vec!["curl", "wget", "git", "build-essential"];
    for pkg in &config.extra_packages {
        apt_packages.push(pkg);
    }
    def.push_str(&format!(
        "    apt-get update && apt-get install -y {}\n",
        apt_packages.join(" ")
    ));
    def.push_str("    rm -rf /var/lib/apt/lists/*\n");

    if needs_conda {
        def.push_str("\n    # Install Miniforge (conda)\n");
        def.push_str("    curl -L -O https://github.com/conda-forge/miniforge/releases/latest/download/Miniforge3-Linux-x86_64.sh\n");
        def.push_str("    bash Miniforge3-Linux-x86_64.sh -b -p /opt/conda\n");
        def.push_str("    rm Miniforge3-Linux-x86_64.sh\n");

        for rule in &workflow.rules {
            if let Some(ref conda_env) = rule.environment.conda {
                let env_filename = std::path::Path::new(conda_env)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| conda_env.clone());
                def.push_str(&format!(
                    "    /opt/conda/bin/conda env create -f /workflow/envs/{env_filename} -n {}\n",
                    rule.name
                ));
            }
        }
    }

    if needs_pixi {
        def.push_str("\n    # Install pixi\n");
        def.push_str("    curl -fsSL https://pixi.sh/install.sh | bash\n");
        def.push_str("    mv /root/.pixi/bin/pixi /usr/local/bin/pixi\n");

        for rule in &workflow.rules {
            if let Some(ref pixi_env) = rule.environment.pixi {
                let env_filename = std::path::Path::new(pixi_env)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| pixi_env.clone());
                def.push_str(&format!(
                    "    cd /workflow/envs && pixi install --manifest-path {env_filename}\n"
                ));
            }
        }
    }

    def.push('\n');

    // Environment section
    def.push_str("%environment\n");
    let mut path_entries = vec!["/usr/local/bin"];
    if needs_conda {
        path_entries.push("/opt/conda/bin");
    }
    def.push_str(&format!(
        "    export PATH={}:$PATH\n\n",
        path_entries.join(":")
    ));

    def.push_str("%runscript\n");
    def.push_str("    exec oxo-flow run \"$@\"\n\n");

    // Test section
    def.push_str("%test\n");
    def.push_str("    oxo-flow --version\n");

    Ok(def)
}

/// Generate a docker-compose.yml string for running the workflow.
pub fn generate_compose_file(workflow: &WorkflowConfig, config: &PackageConfig) -> Result<String> {
    let mut compose = String::new();

    compose.push_str(&format!(
        "# Auto-generated by oxo-flow for workflow: {}\n",
        workflow.workflow.name
    ));
    compose.push_str("version: \"3.8\"\n");
    compose.push_str("services:\n");
    compose.push_str("  oxo-flow:\n");
    compose.push_str("    build: .\n");
    compose.push_str("    volumes:\n");

    if config.include_data {
        compose.push_str("      - ./data:/workflow/data\n");
    }
    compose.push_str("      - ./results:/workflow/results\n");

    compose.push_str("    command: [\"run\", \"workflow.oxoflow\"]\n");

    Ok(compose)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a non-rootless config for backward-compatible tests.
    fn default_non_rootless() -> PackageConfig {
        PackageConfig {
            rootless: false,
            ..Default::default()
        }
    }

    #[test]
    fn generate_basic_dockerfile() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = default_non_rootless();
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("FROM ubuntu:22.04"));
        assert!(dockerfile.contains("test"));
    }

    #[test]
    fn generate_dockerfile_with_conda() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "conda-test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"

            [rules.environment]
            conda = "envs/tools.yaml"
        "#,
        )
        .unwrap();

        let config = default_non_rootless();
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("Miniforge"));
        assert!(dockerfile.contains("conda env create"));
    }

    #[test]
    fn generate_dockerfile_with_pixi() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "pixi-test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"

            [rules.environment]
            pixi = "envs/pixi.toml"
        "#,
        )
        .unwrap();

        let config = default_non_rootless();
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("pixi.sh/install.sh"));
        assert!(dockerfile.contains("pixi install --manifest-path pixi.toml"));
        assert!(
            !dockerfile.contains("Miniforge"),
            "pixi-only should not install conda"
        );
    }

    #[test]
    fn generate_dockerfile_with_extra_packages() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "extras-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            extra_packages: vec!["samtools".to_string(), "bcftools".to_string()],
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("samtools bcftools"));
    }

    #[test]
    fn generate_dockerfile_with_include_data() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "data-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            include_data: true,
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("COPY data/ /workflow/data/"));
    }

    #[test]
    fn generate_dockerfile_has_healthcheck() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "hc-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = default_non_rootless();
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("HEALTHCHECK"));
    }

    #[test]
    fn generate_singularity_def_basic() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "sing-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            format: ContainerFormat::Singularity,
            rootless: false,
            ..Default::default()
        };
        let def = generate_singularity_def(&workflow, &config).unwrap();
        assert!(def.contains("Bootstrap: docker"));
        assert!(def.contains("From: ubuntu:22.04"));
    }

    #[test]
    fn generate_singularity_def_with_conda() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "sing-conda"
            version = "1.0.0"

            [[rules]]
            name = "align"
            output = ["out.bam"]
            shell = "bwa mem ref.fa in.fq > out.bam"

            [rules.environment]
            conda = "envs/align.yaml"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            format: ContainerFormat::Singularity,
            rootless: false,
            ..Default::default()
        };
        let def = generate_singularity_def(&workflow, &config).unwrap();
        assert!(def.contains("Miniforge"));
        assert!(def.contains("conda env create"));
        assert!(def.contains("/opt/conda/bin"));
    }

    #[test]
    fn generate_singularity_def_with_pixi() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "sing-pixi"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hi"

            [rules.environment]
            pixi = "envs/pixi.toml"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            format: ContainerFormat::Singularity,
            rootless: false,
            ..Default::default()
        };
        let def = generate_singularity_def(&workflow, &config).unwrap();
        assert!(def.contains("pixi.sh/install.sh"));
        assert!(def.contains("pixi install --manifest-path pixi.toml"));
    }

    #[test]
    fn generate_singularity_def_has_environment_section() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "env-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig::default();
        let def = generate_singularity_def(&workflow, &config).unwrap();
        assert!(def.contains("%environment"));
        assert!(def.contains("export PATH="));
    }

    #[test]
    fn generate_singularity_def_has_test_section() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "test-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig::default();
        let def = generate_singularity_def(&workflow, &config).unwrap();
        assert!(def.contains("%test"));
        assert!(def.contains("oxo-flow --version"));
    }

    #[test]
    fn generate_singularity_def_with_data() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "data-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            format: ContainerFormat::Singularity,
            include_data: true,
            rootless: false,
            ..Default::default()
        };
        let def = generate_singularity_def(&workflow, &config).unwrap();
        assert!(def.contains("%files"));
        assert!(def.contains("data/ /workflow/data/"));
    }

    #[test]
    fn container_format_display() {
        assert_eq!(ContainerFormat::Docker.to_string(), "docker");
        assert_eq!(ContainerFormat::Singularity.to_string(), "singularity");
    }

    #[test]
    fn generate_compose_file_basic() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "compose-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig::default();
        let compose = generate_compose_file(&workflow, &config).unwrap();
        assert!(compose.contains("version: \"3.8\""));
        assert!(compose.contains("oxo-flow:"));
        assert!(compose.contains("build: ."));
        assert!(compose.contains("./results:/workflow/results"));
        assert!(compose.contains("command: [\"run\", \"workflow.oxoflow\"]"));
    }

    #[test]
    fn generate_compose_file_with_data() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "compose-data"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            include_data: true,
            ..Default::default()
        };
        let compose = generate_compose_file(&workflow, &config).unwrap();
        assert!(compose.contains("./data:/workflow/data"));
        assert!(compose.contains("./results:/workflow/results"));
    }

    #[test]
    fn compose_file_without_data_excludes_data_volume() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "compose-no-data"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig::default();
        let compose = generate_compose_file(&workflow, &config).unwrap();
        assert!(!compose.contains("./data:/workflow/data"));
    }

    // ── New feature tests ──────────────────────────────────────────

    #[test]
    fn generate_multistage_dockerfile() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "multi-test"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hello"

            [rules.environment]
            conda = "envs/tools.yaml"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            multi_stage: true,
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();

        assert!(dockerfile.contains("FROM ubuntu:22.04 AS builder"));
        assert!(dockerfile.contains("COPY --from=builder /opt/conda /opt/conda"));
        assert!(dockerfile.contains("COPY --from=builder /workflow /workflow"));
        // Two FROM directives (builder + runtime)
        assert_eq!(dockerfile.matches("FROM ").count(), 2);
    }

    #[test]
    fn generate_multistage_dockerfile_with_pixi() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "multi-pixi"
            version = "1.0.0"

            [[rules]]
            name = "step1"
            output = ["out.txt"]
            shell = "echo hi"

            [rules.environment]
            pixi = "envs/pixi.toml"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            multi_stage: true,
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();

        assert!(dockerfile.contains("COPY --from=builder /usr/local/bin/pixi"));
    }

    #[test]
    fn rootless_dockerfile() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "rootless-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            rootless: true,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("groupadd -r oxoflow"));
        assert!(dockerfile.contains("useradd -r -g oxoflow -d /home/oxoflow -s /bin/bash oxoflow"));
        assert!(dockerfile.contains("USER oxoflow"));
        assert!(dockerfile.contains("WORKDIR /home/oxoflow"));
    }

    #[test]
    fn rootless_is_default() {
        let config = PackageConfig::default();
        assert!(config.rootless);
    }

    #[test]
    fn non_rootless_dockerfile_omits_user() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "root-test"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(!dockerfile.contains("USER oxoflow"));
        assert!(!dockerfile.contains("groupadd"));
    }

    #[test]
    fn custom_healthcheck() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "hc-custom"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            healthcheck: Some(r#"["test", "-f", "/home/oxoflow/.ready"] || exit 1"#.to_string()),
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(
            dockerfile
                .contains(r#"HEALTHCHECK CMD ["test", "-f", "/home/oxoflow/.ready"] || exit 1"#)
        );
    }

    #[test]
    fn default_healthcheck_when_none() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "hc-default"
            version = "1.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            healthcheck: None,
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("HEALTHCHECK CMD [\"oxo-flow\", \"--version\"]"));
    }

    #[test]
    fn generate_docker_run_command_basic() {
        let resources = crate::rule::Resources {
            threads: 4,
            memory: Some("8G".to_string()),
            gpu: None,
            gpu_spec: None,
            disk: None,
            time_limit: None,
            partition: None,
            groups: std::collections::HashMap::new(),
        };
        let cmd = generate_docker_run_command("my-image:latest", &resources, "/data/project");
        assert_eq!(
            cmd,
            "docker run --rm --memory=8G --cpus=4 -v /data/project:/data -w /data my-image:latest"
        );
    }

    #[test]
    fn generate_docker_run_command_no_memory() {
        let resources = crate::rule::Resources {
            threads: 2,
            memory: None,
            gpu: None,
            gpu_spec: None,
            disk: None,
            time_limit: None,
            partition: None,
            groups: std::collections::HashMap::new(),
        };
        let cmd = generate_docker_run_command("img:1.0", &resources, "/work");
        assert_eq!(
            cmd,
            "docker run --rm --cpus=2 -v /work:/data -w /data img:1.0"
        );
        assert!(!cmd.contains("--memory"));
    }

    #[test]
    fn multistage_with_rootless() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "full-test"
            version = "2.0.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            multi_stage: true,
            rootless: true,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();
        assert!(dockerfile.contains("AS builder"));
        assert!(dockerfile.contains("USER oxoflow"));
        assert!(dockerfile.contains("HEALTHCHECK"));
    }

    #[test]
    fn generate_docker_run_basic() {
        let resources = crate::rule::Resources::default();
        let cmd = generate_docker_run_command("myimage:latest", &resources, "/data");
        assert!(cmd.contains("docker run"));
        assert!(cmd.contains("myimage:latest"));
        assert!(cmd.contains("/data"));
    }

    // ── GPU container tests ─────────────────────────────────────────────────

    #[test]
    fn generate_docker_run_command_with_gpu_count() {
        let resources = crate::rule::Resources {
            threads: 4,
            memory: Some("16G".to_string()),
            gpu: Some(2),
            gpu_spec: None,
            disk: None,
            time_limit: None,
            partition: None,
            groups: std::collections::HashMap::new(),
        };
        let cmd = generate_docker_run_command("gpu-image:latest", &resources, "/data/project");
        assert!(cmd.contains("--gpus 2"));
        assert!(cmd.contains("--memory=16G"));
        assert!(cmd.contains("--cpus=4"));
    }

    #[test]
    fn generate_docker_run_command_with_gpu_spec() {
        let resources = crate::rule::Resources {
            threads: 8,
            memory: Some("32G".to_string()),
            gpu: None,
            gpu_spec: Some(crate::rule::GpuSpec {
                count: 4,
                model: Some("A100".to_string()),
                memory_gb: None,
                compute_capability: None,
            }),
            disk: None,
            time_limit: None,
            partition: None,
            groups: std::collections::HashMap::new(),
        };
        let cmd = generate_docker_run_command("cuda-image:latest", &resources, "/data/project");
        assert!(cmd.contains("--gpus \"device=A100,count=4\""));
    }

    #[test]
    fn generate_docker_run_command_gpu_spec_no_model() {
        let resources = crate::rule::Resources {
            threads: 2,
            memory: None,
            gpu: None,
            gpu_spec: Some(crate::rule::GpuSpec {
                count: 1,
                model: None,
                memory_gb: None,
                compute_capability: None,
            }),
            disk: None,
            time_limit: None,
            partition: None,
            groups: std::collections::HashMap::new(),
        };
        let cmd = generate_docker_run_command("gpu:latest", &resources, "/work");
        assert!(cmd.contains("--gpus 1"));
    }

    #[test]
    fn detect_gpu_requirements_from_simple_gpu() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "gpu-workflow"
            version = "1.0.0"

            [[rules]]
            name = "train"
            output = ["model.pt"]
            shell = "python train.py"

            [rules.resources]
            gpu = 2
        "#,
        )
        .unwrap();

        let gpu_spec = detect_gpu_requirements(&workflow);
        assert!(gpu_spec.is_some());
        let spec = gpu_spec.unwrap();
        assert_eq!(spec.count, 2);
        assert!(spec.model.is_none());
    }

    #[test]
    fn detect_gpu_requirements_from_gpu_spec() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "gpu-spec-workflow"
            version = "1.0.0"

            [[rules]]
            name = "train"
            output = ["model.pt"]
            shell = "python train.py"

            [rules.resources.gpu_spec]
            count = 4
            model = "V100"
            compute_capability = "7.0"
        "#,
        )
        .unwrap();

        let gpu_spec = detect_gpu_requirements(&workflow);
        assert!(gpu_spec.is_some());
        let spec = gpu_spec.unwrap();
        assert_eq!(spec.count, 4);
        assert_eq!(spec.model, Some("V100".to_string()));
    }

    #[test]
    fn select_base_image_cuda_for_gpu_workflow() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "cuda-workflow"
            version = "1.0.0"

            [[rules]]
            name = "train"
            output = ["model.pt"]
            shell = "python train.py"

            [rules.resources]
            gpu = 1
        "#,
        )
        .unwrap();

        let config = PackageConfig::default();
        let base_image = select_base_image(&workflow, &config);
        assert!(base_image.contains("nvidia/cuda"));
        assert!(base_image.contains("ubuntu22.04"));
    }

    #[test]
    fn select_base_image_cuda_for_compute_cap_8() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "a100-workflow"
            version = "1.0.0"

            [[rules]]
            name = "train"
            output = ["model.pt"]
            shell = "python train.py"

            [rules.resources.gpu_spec]
            count = 2
            compute_capability = "8.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig::default();
        let base_image = select_base_image(&workflow, &config);
        assert!(base_image == "nvidia/cuda:12.0.0-runtime-ubuntu22.04");
    }

    #[test]
    fn select_base_image_cuda_for_compute_cap_7() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "v100-workflow"
            version = "1.0.0"

            [[rules]]
            name = "train"
            output = ["model.pt"]
            shell = "python train.py"

            [rules.resources.gpu_spec]
            count = 2
            compute_capability = "7.0"
        "#,
        )
        .unwrap();

        let config = PackageConfig::default();
        let base_image = select_base_image(&workflow, &config);
        assert!(base_image == "nvidia/cuda:11.8.0-runtime-ubuntu22.04");
    }

    #[test]
    fn select_base_image_respects_custom_config() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "gpu-custom"
            version = "1.0.0"

            [[rules]]
            name = "train"
            output = ["model.pt"]
            shell = "python train.py"

            [rules.resources]
            gpu = 1
        "#,
        )
        .unwrap();

        // User explicitly sets base image
        let config = PackageConfig {
            base_image: "my-custom-cuda:latest".to_string(),
            ..Default::default()
        };
        let base_image = select_base_image(&workflow, &config);
        assert_eq!(base_image, "my-custom-cuda:latest");
    }

    #[test]
    fn dockerfile_gpu_workflow_uses_cuda_base() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "gpu-container"
            version = "1.0.0"

            [[rules]]
            name = "train"
            output = ["model.pt"]
            shell = "python train.py"

            [rules.resources]
            gpu = 2
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();

        assert!(dockerfile.contains("FROM nvidia/cuda"));
        assert!(dockerfile.contains("NVIDIA_VISIBLE_DEVICES=all"));
        assert!(dockerfile.contains("NVIDIA_DRIVER_CAPABILITIES=compute,utility"));
    }

    #[test]
    fn multistage_dockerfile_with_gpu() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "gpu-multistage"
            version = "1.0.0"

            [[rules]]
            name = "train"
            output = ["model.pt"]
            shell = "python train.py"

            [rules.resources]
            gpu = 4
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            multi_stage: true,
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();

        assert!(dockerfile.contains("FROM nvidia/cuda"));
        assert!(dockerfile.contains("AS builder"));
        assert!(dockerfile.contains("NVIDIA_VISIBLE_DEVICES=all"));
    }

    #[test]
    fn no_gpu_workflow_uses_default_base() {
        let workflow = WorkflowConfig::parse(
            r#"
            [workflow]
            name = "cpu-only"
            version = "1.0.0"

            [[rules]]
            name = "process"
            output = ["output.txt"]
            shell = "echo hello"
        "#,
        )
        .unwrap();

        let config = PackageConfig {
            rootless: false,
            ..Default::default()
        };
        let dockerfile = generate_dockerfile(&workflow, &config).unwrap();

        assert!(dockerfile.contains("FROM ubuntu:22.04"));
        assert!(!dockerfile.contains("nvidia/cuda"));
        assert!(!dockerfile.contains("NVIDIA_VISIBLE_DEVICES"));
    }
}
