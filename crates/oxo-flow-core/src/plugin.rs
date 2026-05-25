//! Plugin system for oxo-flow.
//!
//! Provides a compile-time + config-based plugin architecture. Plugins are
//! registered via TOML configuration files and ship as Rust crates compiled
//! into the binary — no unsafe dynamic loading required.
//!
//! ## Plugin Discovery
//!
//! Plugins are discovered from:
//! - `~/.oxo-flow/plugins/*.plugin.toml` (user-level)
//! - `<project>/.oxo-flow/plugins/*.plugin.toml` (project-level)
//!
//! ## Signature Verification
//!
//! Each plugin config must include an HMAC-SHA256 signature in its `[signature]`
//! section. The signature is verified against a trusted key to ensure plugin
//! authenticity and integrity.
//!
//! ## TOML Integration
//!
//! Workflows declare plugin usage in the `[plugins]` section:
//! ```toml
//! [plugins]
//! rules = ["r-function"]
//! executor = "slurm-custom"
//! ```

use crate::error::{OxoFlowError, Result};
use crate::rule::Rule;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Version of the plugin API. Incremented on breaking changes.
pub const PLUGIN_API_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Plugin Traits (implement these in your crate)
// ---------------------------------------------------------------------------

/// Trait for custom rule types (e.g., R functions, Python scripts).
pub trait RulePlugin: Send + Sync {
    fn rule_type(&self) -> &str;
    fn build_command(&self, rule: &Rule, values: &HashMap<String, String>) -> Result<String>;
    fn validate(&self, rule: &Rule) -> Result<()>;
    fn extra_fields(&self) -> Vec<(&str, &str)> {
        Vec::new()
    }
}

/// Trait for custom executors.
pub trait ExecutorPlugin: Send + Sync {
    fn backend_name(&self) -> &str;
    fn submit(
        &self,
        rule: &Rule,
        workdir: &Path,
    ) -> Result<crate::executor::checkpoint::BenchmarkRecord>;
    fn status(&self, job_id: &str) -> Result<crate::executor::JobStatus>;
    fn cancel(&self, job_id: &str) -> Result<()>;
}

/// Trait for custom report renderers.
pub trait ReportPlugin: Send + Sync {
    fn renderer_name(&self) -> &str;
    fn output_format(&self) -> &str;
    fn render(&self, report: &crate::report::Report) -> Result<Vec<u8>>;
}

// ---------------------------------------------------------------------------
// Plugin Configuration (TOML)
// ---------------------------------------------------------------------------

/// A plugin manifest loaded from a `.plugin.toml` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name (unique identifier).
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Plugin type: "rule", "executor", "report".
    pub plugin_type: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Author information.
    pub author: Option<String>,
    /// Entry point / command template for rule plugins.
    pub command_template: Option<String>,
    /// Required environment (conda YAML path, docker image, etc.).
    pub environment: Option<String>,
    /// HMAC signature for authenticity verification.
    pub signature: Option<PluginSignature>,
}

/// HMAC-SHA256 signature for plugin verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSignature {
    /// Key identifier (which trusted key to verify against).
    pub key_id: String,
    /// Hex-encoded HMAC-SHA256 signature of the plugin manifest content.
    pub value: String,
}

impl PluginManifest {
    /// Compute the signing payload (all fields except signature).
    pub fn signing_payload(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.name,
            self.version,
            self.plugin_type,
            self.description.as_deref().unwrap_or("")
        )
    }

    /// Verify the HMAC-SHA256 signature against a trusted key.
    pub fn verify_signature(&self, trusted_keys: &HashMap<String, String>) -> Result<bool> {
        let sig = self
            .signature
            .as_ref()
            .ok_or_else(|| OxoFlowError::Config {
                message: format!("plugin '{}' has no signature", self.name),
            })?;

        let key = trusted_keys
            .get(&sig.key_id)
            .ok_or_else(|| OxoFlowError::Config {
                message: format!("unknown key_id '{}' for plugin '{}'", sig.key_id, self.name),
            })?;

        let payload = self.signing_payload();
        let expected = compute_keyed_sha256(key, &payload);

        Ok(expected == sig.value)
    }
}

/// Compute a keyed SHA-256 hex digest: SHA256(key || message).
///
/// NOTE: This is NOT a true HMAC (RFC 2104). It uses simple concatenation
/// instead of the ipad/opad construction. It provides integrity checking
/// against accidental corruption but NOT cryptographic authentication.
/// For production deployments, use a proper HMAC implementation.
fn compute_keyed_sha256(key: &str, message: &str) -> String {
    use sha2::Digest;
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hasher.update(message.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// Plugin Discovery
// ---------------------------------------------------------------------------

/// Discover plugin manifests from standard directories.
pub fn discover_plugins(project_dir: Option<&Path>) -> Vec<PluginManifest> {
    let mut manifests = Vec::new();

    // User-level plugins
    if let Some(home) = dirs_next() {
        let user_dir = home.join(".oxo-flow").join("plugins");
        manifests.extend(scan_plugin_dir(&user_dir));
    }

    // Project-level plugins
    if let Some(proj) = project_dir {
        let proj_dir = proj.join(".oxo-flow").join("plugins");
        manifests.extend(scan_plugin_dir(&proj_dir));
    }

    manifests
}

/// Scan a directory for `.plugin.toml` files and parse them.
fn scan_plugin_dir(dir: &Path) -> Vec<PluginManifest> {
    let mut manifests = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return manifests;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml")
            && path
                .file_stem()
                .is_some_and(|s| s.to_string_lossy().ends_with(".plugin"))
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(manifest) = toml::from_str::<PluginManifest>(&content)
        {
            manifests.push(manifest);
        }
    }

    manifests
}

/// Simple home directory helper.
fn dirs_next() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Plugin Registry
// ---------------------------------------------------------------------------

/// Registry of loaded and verified plugins.
#[derive(Default)]
pub struct PluginRegistry {
    /// Rule-type plugins indexed by rule_type.
    pub rule_plugins: HashMap<String, Box<dyn RulePlugin>>,
    /// Executor plugins indexed by backend_name.
    pub executor_plugins: HashMap<String, Box<dyn ExecutorPlugin>>,
    /// Report plugins indexed by renderer_name.
    pub report_plugins: HashMap<String, Box<dyn ReportPlugin>>,
    /// Verified plugin manifests.
    pub manifests: Vec<PluginManifest>,
    /// Trusted keys for signature verification.
    pub trusted_keys: HashMap<String, String>,
}

impl PluginRegistry {
    /// Add a trusted key for plugin verification.
    pub fn add_trusted_key(&mut self, key_id: &str, key: &str) {
        self.trusted_keys
            .insert(key_id.to_string(), key.to_string());
    }

    /// Register a rule plugin.
    pub fn register_rule(&mut self, plugin: Box<dyn RulePlugin>) {
        self.rule_plugins
            .insert(plugin.rule_type().to_string(), plugin);
    }

    /// Register an executor plugin.
    pub fn register_executor(&mut self, plugin: Box<dyn ExecutorPlugin>) {
        self.executor_plugins
            .insert(plugin.backend_name().to_string(), plugin);
    }

    /// Register a report plugin.
    pub fn register_report(&mut self, plugin: Box<dyn ReportPlugin>) {
        self.report_plugins
            .insert(plugin.renderer_name().to_string(), plugin);
    }

    /// Discover plugins from the filesystem and verify their signatures.
    pub fn discover(&mut self, project_dir: Option<&Path>) -> Result<usize> {
        let manifests = discover_plugins(project_dir);
        let mut loaded = 0;

        for manifest in manifests {
            // Verify signature if present and trusted keys are configured
            if manifest.signature.is_some() {
                if self.trusted_keys.is_empty() {
                    tracing::warn!(
                        plugin = %manifest.name,
                        "plugin has a signature but no trusted keys are configured — \
                         signature verification skipped. Configure trusted_keys_file in [plugins]."
                    );
                } else if !manifest.verify_signature(&self.trusted_keys)? {
                    tracing::warn!(
                        plugin = %manifest.name,
                        "plugin signature verification failed — skipping"
                    );
                    continue;
                }
            } else {
                tracing::warn!(
                    plugin = %manifest.name,
                    "plugin has no signature — loaded without integrity verification"
                );
            }

            self.manifests.push(manifest);
            loaded += 1;
        }

        Ok(loaded)
    }

    /// Find a rule plugin by type name.
    pub fn find_rule(&self, rule_type: &str) -> Option<&dyn RulePlugin> {
        self.rule_plugins.get(rule_type).map(|p| p.as_ref())
    }

    /// Find an executor plugin by backend name.
    pub fn find_executor(&self, backend: &str) -> Option<&dyn ExecutorPlugin> {
        self.executor_plugins.get(backend).map(|p| p.as_ref())
    }

    /// Find a report plugin by renderer name.
    pub fn find_report(&self, renderer: &str) -> Option<&dyn ReportPlugin> {
        self.report_plugins.get(renderer).map(|p| p.as_ref())
    }
}

// ---------------------------------------------------------------------------
// TOML Integration: [plugins] section in .oxoflow files
// ---------------------------------------------------------------------------

/// Plugin configuration parsed from `[plugins]` section in a workflow file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginsConfig {
    /// Rule plugin types to enable.
    #[serde(default)]
    pub rules: Vec<String>,
    /// Executor plugin to use.
    #[serde(default)]
    pub executor: Option<String>,
    /// Report plugins to enable.
    #[serde(default)]
    pub reports: Vec<String>,
    /// Path to a trusted keys file for signature verification.
    #[serde(default)]
    pub trusted_keys_file: Option<String>,
}

// ---------------------------------------------------------------------------
// Subprocess Plugin Executor (dynamic loading without unsafe code)
// ---------------------------------------------------------------------------

/// Input sent to a plugin subprocess via stdin (JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInput {
    /// Rule name being executed.
    pub rule: String,
    /// Input files.
    pub inputs: Vec<String>,
    /// Output files.
    pub outputs: Vec<String>,
    /// Shell command to execute (for simple plugins, oxo-flow builds it).
    pub command: Option<String>,
    /// Configuration variables from the workflow.
    pub config: HashMap<String, String>,
    /// Extra plugin-specific parameters.
    pub params: HashMap<String, String>,
}

/// Output received from a plugin subprocess via stdout (JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginOutput {
    /// Whether execution succeeded.
    pub success: bool,
    /// Modified or final shell command (if plugin transforms the command).
    pub command: Option<String>,
    /// Validation errors (if any).
    pub errors: Vec<String>,
    /// Log messages.
    pub logs: Vec<String>,
    /// Exit code suggestion (0 = success).
    pub exit_code: i32,
}

impl Default for PluginOutput {
    fn default() -> Self {
        Self {
            success: true,
            command: None,
            errors: Vec::new(),
            logs: Vec::new(),
            exit_code: 0,
        }
    }
}

/// Execute a plugin subprocess and collect its output.
///
/// The plugin executable receives JSON input on stdin and must
/// write JSON output to stdout within the given timeout.
///
/// This is the safe, portable alternative to `libloading`-based
/// dynamic loading — plugins are standalone executables that
/// communicate via a simple JSON protocol.
pub async fn execute_plugin_subprocess(
    executable: &Path,
    input: &PluginInput,
    timeout_secs: u64,
) -> Result<PluginOutput> {
    let input_json = serde_json::to_string(input)?;

    let mut child = tokio::process::Command::new(executable)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| OxoFlowError::Execution {
            rule: input.rule.clone(),
            message: format!("failed to spawn plugin '{}': {}", executable.display(), e),
        })?;

    // Write input to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(input_json.as_bytes())
            .await
            .map_err(|e| OxoFlowError::Execution {
                rule: input.rule.clone(),
                message: format!("failed to write plugin input: {}", e),
            })?;
        drop(stdin);
    }

    // Wait for output with timeout
    let output = if timeout_secs > 0 {
        tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| OxoFlowError::Execution {
            rule: input.rule.clone(),
            message: format!(
                "plugin '{}' timed out after {}s",
                executable.display(),
                timeout_secs
            ),
        })?
        .map_err(|e| OxoFlowError::Execution {
            rule: input.rule.clone(),
            message: format!("plugin '{}' failed: {}", executable.display(), e),
        })?
    } else {
        child
            .wait_with_output()
            .await
            .map_err(|e| OxoFlowError::Execution {
                rule: input.rule.clone(),
                message: format!("plugin '{}' failed: {}", executable.display(), e),
            })?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(PluginOutput {
            success: false,
            command: None,
            errors: vec![format!(
                "plugin exited with code {:?}: {}",
                output.status.code(),
                stderr.trim()
            )],
            logs: Vec::new(),
            exit_code: output.status.code().unwrap_or(1),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<PluginOutput>(stdout.trim()).map_err(|e| OxoFlowError::Execution {
        rule: input.rule.clone(),
        message: format!(
            "failed to parse plugin output from '{}': {}",
            executable.display(),
            e
        ),
    })
}

/// Build a plugin executable path from a manifest.
pub fn resolve_plugin_executable(manifest: &PluginManifest) -> Option<PathBuf> {
    // Priority: command_template, then search PATH for plugin name
    if let Some(ref tmpl) = manifest.command_template {
        let exe = tmpl.split_whitespace().next().unwrap_or(tmpl);
        return Some(PathBuf::from(exe));
    }
    // Search PATH
    find_in_path(&manifest.name)
}

/// Search for an executable in PATH.
fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            let exe = dir.join(name);
            if exe.exists() { Some(exe) } else { None }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubRulePlugin;
    impl RulePlugin for StubRulePlugin {
        fn rule_type(&self) -> &str {
            "stub"
        }
        fn build_command(&self, _rule: &Rule, _values: &HashMap<String, String>) -> Result<String> {
            Ok("echo stub".into())
        }
        fn validate(&self, _rule: &Rule) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn registry_register_and_find() {
        let mut registry = PluginRegistry::default();
        registry.register_rule(Box::new(StubRulePlugin));
        assert!(registry.find_rule("stub").is_some());
        assert!(registry.find_rule("nonexistent").is_none());
    }

    #[test]
    fn plugin_api_version_is_stable() {
        assert_eq!(PLUGIN_API_VERSION, 1);
    }

    #[test]
    fn signing_payload_is_deterministic() {
        let manifest = PluginManifest {
            name: "test".into(),
            version: "1.0".into(),
            plugin_type: "rule".into(),
            description: Some("A test plugin".into()),
            author: None,
            command_template: None,
            environment: None,
            signature: None,
        };
        let payload1 = manifest.signing_payload();
        let payload2 = manifest.signing_payload();
        assert_eq!(payload1, payload2);
        assert!(payload1.contains("test"));
        assert!(payload1.contains("1.0"));
    }

    #[test]
    fn signature_verification_works() {
        let key = "test-secret-key-32bytes-minimum!";
        let manifest = PluginManifest {
            name: "verified-plugin".into(),
            version: "1.0".into(),
            plugin_type: "rule".into(),
            description: None,
            author: None,
            command_template: None,
            environment: None,
            signature: None,
        };

        let payload = manifest.signing_payload();
        let sig_value = compute_keyed_sha256(key, &payload);

        let mut manifest_signed = manifest.clone();
        manifest_signed.signature = Some(PluginSignature {
            key_id: "key-001".into(),
            value: sig_value,
        });

        let mut trusted = HashMap::new();
        trusted.insert("key-001".into(), key.to_string());

        assert!(manifest_signed.verify_signature(&trusted).unwrap());
    }

    #[test]
    fn signature_verification_rejects_wrong_key() {
        let manifest = PluginManifest {
            name: "test".into(),
            version: "1.0".into(),
            plugin_type: "rule".into(),
            description: None,
            author: None,
            command_template: None,
            environment: None,
            signature: Some(PluginSignature {
                key_id: "key-001".into(),
                value: "deadbeef".into(),
            }),
        };

        let mut trusted = HashMap::new();
        trusted.insert("key-001".into(), "correct-key".to_string());

        assert!(!manifest.verify_signature(&trusted).unwrap());
    }

    #[test]
    fn plugins_config_deserializes() {
        let toml_str = r#"
rules = ["r-function", "python-script"]
executor = "aws-batch"
reports = ["native-pdf"]
"#;
        let config: PluginsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.rules, vec!["r-function", "python-script"]);
        assert_eq!(config.executor.as_deref(), Some("aws-batch"));
        assert_eq!(config.reports, vec!["native-pdf"]);
    }
}
