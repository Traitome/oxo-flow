# Plugin System Design

This document describes the planned plugin architecture for oxo-flow,
enabling community-contributed rule types, environment backends, and
executor extensions.

## Design Principles

- **Trait-based**: All extension points are Rust traits with clear contracts.
- **Dynamic loading**: Plugins are compiled as `.so`/`.dylib` shared libraries and loaded at runtime via `libloading`.
- **Sandboxed**: Plugin code runs with restricted capabilities; shell execution is mediated by the core engine.
- **Versioned API**: Plugin API versions are checked at load time to prevent ABI mismatches.

## Extension Points

### 1. Custom Rule Types (`RulePlugin`)

```rust
/// Trait for custom rule types (e.g., R functions, Python scripts, Jupyter notebooks).
pub trait RulePlugin: Send + Sync {
    /// Unique identifier (e.g., "r-function", "python-script").
    fn name(&self) -> &str;
    /// Build the shell command to execute this rule.
    fn build_command(&self, rule: &Rule, values: &HashMap<String, String>) -> Result<String>;
    /// Validate the rule configuration.
    fn validate(&self, rule: &Rule) -> Result<()>;
    /// Return any extra TOML fields this rule type accepts.
    fn extra_fields(&self) -> Vec<(&str, &str)>;
}
```

**Use cases:**
- `RFunction`: `shell = "run_r_script('analysis.R', data = {input[0]})"` — validates R package dependencies
- `PythonScript`: `shell = "python {script}"` — validates Python version and packages
- `JupyterNotebook`: Converts `.ipynb` to script and executes with papermill
- `DockerCompose`: Spins up multi-container services for a rule

### 2. Environment Backends (`EnvironmentPlugin`)

```rust
/// Trait for custom environment backends (e.g., Podman, Enroot, Lmod).
pub trait EnvironmentPlugin: EnvironmentBackend {}
```

The existing `EnvironmentBackend` trait is already pluggable. A plugin registry
at startup discovers installed backends and makes them available via
`oxo-flow env list`.

**Use cases:**
- **Podman**: Drop-in Docker replacement for rootless containers
- **Enroot**: NVIDIA GPU-aware container runtime for HPC
- **Lmod**: Environment modules system commonly used on clusters
- **Guix**: Reproducible package management via GNU Guix

### 3. Executor Extensions (`ExecutorPlugin`)

```rust
/// Trait for custom executors (e.g., cloud batch, Kubernetes).
pub trait ExecutorPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn submit(&self, rule: &Rule, config: &ExecutorConfig) -> Result<JobHandle>;
    fn status(&self, handle: &JobHandle) -> Result<JobStatus>;
    fn cancel(&self, handle: &JobHandle) -> Result<()>;
    fn logs(&self, handle: &JobHandle) -> Result<String>;
}
```

**Use cases:**
- **AWS Batch**: Submit jobs to AWS Batch with auto-scaling
- **GCP Cloud Life Sciences**: Google's genomics-optimized batch API
- **Kubernetes**: Submit as K8s Jobs with resource requests

### 4. Report Templates (`ReportPlugin`)

```rust
pub trait ReportPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn render(&self, report: &Report) -> Result<String>;
    fn output_format(&self) -> &str; // e.g., "pdf", "docx", "pptx"
}
```

**Use cases:**
- **PDF (native)**: Generate PDF without wkhtmltopdf dependency
- **DOCX**: Clinical reports for regulatory submission
- **Interactive HTML**: JavaScript-based interactive reports with filtering

## Plugin Discovery

Plugins are `.so` files placed in `~/.oxo-flow/plugins/` or `<project>/.oxo-flow/plugins/`.
At startup, oxo-flow scans these directories and loads plugins that export:

```rust
#[no_mangle]
pub extern "C" fn oxo_flow_plugin_version() -> u32 { 1 }

#[no_mangle]
pub extern "C" fn oxo_flow_register_rules(registry: &mut RuleRegistry) { ... }

#[no_mangle]  
pub extern "C" fn oxo_flow_register_environments(registry: &mut EnvironmentRegistry) { ... }
```

## Configuration

```toml
# .oxoflow file
[plugins]
rules = ["r-function", "jupyter"]
environments = ["podman"]
executor = "aws-batch"

[[rules]]
name = "deseq2_analysis"
type = "r-function"
script = "scripts/deseq2.R"
```

## Security Considerations

- Plugin code runs in the same process — trust boundary is at the plugin author level.
- Plugins cannot bypass shell injection validation — all commands are sanitized.
- File system access is limited to the workflow directory.
- Plugin loading can be disabled with `--no-plugins` flag.
- Plugin signatures (ed25519) will be verified in a future release.

## Roadmap

| Phase | Feature | Timeline |
|-------|---------|----------|
| 1 | Plugin trait definitions in oxo-flow-core | v0.7 |
| 2 | Dynamic loading via libloading | v0.8 |
| 3 | Plugin registry and discovery | v0.8 |
| 4 | Signed plugin verification | v0.9 |
| 5 | Plugin marketplace / registry | v1.0 |
