//! Regression test for issue #634: the documented `[ai]` activation path
//! must round-trip. The E017 known-keys whitelist used to reject the
//! section outright, so `oxo-flow validate` failed on every workflow that
//! followed the docs' one-line enable (`[ai] enabled = true`) and the
//! settings never reached `AiConfig::from_workflow_toml`.
//!
//! Both halves of the documented contract are asserted:
//!
//! - `WorkflowConfig::parse` accepts the section (no E017), and
//! - the same TOML table yields an `AiConfig` carrying the settings
//!   through (`enabled`, provider, retries, skills, team profile).
//!
//! This file lives in oxo-flow-cli because it is the only crate that
//! depends on both oxo-flow-core (whitelist) and oxo-flow-ai (reader).

use oxo_flow_ai::config::AiConfig;
use oxo_flow_ai::provider::ProviderKind;

/// A minimal workflow carrying the full documented `[ai]` section.
const WORKFLOW: &str = r#"
[workflow]
name = "ai-activation"
version = "1.0.0"

[ai]
enabled = true
provider = "deepseek"
model = "deepseek-chat"
max_retries = 6
auto_fix = "suggest"
temperature = 0.2
skills = ["my-skill"]
team_profile = "compact"

[[rules]]
name = "step1"
output = ["r/out1.txt"]
shell = "mkdir -p r && echo done > {output[0]}"
"#;

#[test]
fn parse_accepts_the_documented_ai_section() {
    oxo_flow_core::WorkflowConfig::parse(WORKFLOW).expect("[ai] section must not fail parsing");
}

#[test]
fn ai_section_reaches_aiconfig() {
    let table: toml::Table = toml::from_str(WORKFLOW).unwrap();
    let config = AiConfig::from_workflow_toml(&table).expect("[ai] section yields an AiConfig");
    assert!(config.enabled);
    assert_eq!(config.provider, ProviderKind::DeepSeek);
    assert_eq!(config.model.as_deref(), Some("deepseek-chat"));
    assert_eq!(config.max_retries, 6);
    assert_eq!(config.skills, vec!["my-skill".to_string()]);
    // Credentials never come from workflow files, whatever the section says.
    assert!(config.api_key.is_none());
    assert!(config.api_url.is_none());
}

#[test]
fn validate_cli_passes_on_an_ai_enabled_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("w.oxoflow");
    std::fs::write(&workflow, WORKFLOW).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
        .args(["validate", workflow.to_str().unwrap(), "--json"])
        .output()
        .expect("oxo-flow validate spawns");
    assert!(
        output.status.success(),
        "validate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["valid"], serde_json::json!(true));
}
