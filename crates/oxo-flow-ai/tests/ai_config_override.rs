//! `OXO_FLOW_AI_CONFIG` redirects the AI config file to an explicit path.
//!
//! Regression for the test-isolation incident: an in-process web integration
//! test triggered `AiRegistry::reconfigure`, which persisted through
//! `ai_config_path()` and overwrote the operator's real
//! `$HOME/.oxo-flow/ai_config.json` with placeholder values. The override is
//! the hermetic knob tests and CI use so no save can touch the real home.
//!
//! One test function drives every scenario sequentially: the environment is
//! process-global, so splitting these into parallel tests in this binary
//! would race on the override itself.

use oxo_flow_ai::provider::{
    AI_CONFIG_OVERRIDE_ENV, ai_config_path, load_ai_config, resolved_ai_config_path, save_ai_config,
};
use serde_json::Value;

fn read_json(path: &std::path::Path) -> Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

#[test]
fn override_redirects_config_io_and_default_stays_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let default_path = home.join(".oxo-flow").join("ai_config.json");
    let legacy_path = home.join(".config").join("oxo-flow").join("ai_config.json");
    std::fs::create_dir_all(default_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();

    // SAFETY: this integration-test binary contains exactly one test, so no
    // other thread reads or writes the environment concurrently.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::remove_var(AI_CONFIG_OVERRIDE_ENV);
    }

    // ── Default resolution with no override (unchanged behavior) ──
    assert_eq!(
        ai_config_path(),
        default_path,
        "without the override the canonical $HOME path must not change"
    );

    // ── Override set: saves land at the override, home stays untouched ──
    let override_path = dir.path().join("scratch").join("ai_config.json");
    // SAFETY: see above.
    unsafe { std::env::set_var(AI_CONFIG_OVERRIDE_ENV, &override_path) };
    assert_eq!(ai_config_path(), override_path);

    save_ai_config(
        "openai",
        Some("sk-override-key"),
        Some("https://api.example.invalid/v1"),
        Some("gpt-test"),
    );
    let saved = read_json(&override_path);
    assert_eq!(saved["provider"], "openai");
    assert_eq!(saved["api_key"], "sk-override-key");
    assert_eq!(saved["api_url"], "https://api.example.invalid/v1");
    assert_eq!(saved["model"], "gpt-test");
    assert!(
        !default_path.exists(),
        "the $HOME config must stay untouched while the override is set: {}",
        default_path.display()
    );
    assert_eq!(
        load_ai_config().map(|(kind, ..)| kind),
        Some("openai".to_string()),
        "reads must resolve the override file too"
    );

    // ── Override is authoritative: no legacy $HOME fallback ──
    std::fs::write(
        &legacy_path,
        r#"{"provider":"claude","api_key":"sk-legacy","api_url":"","model":""}"#,
    )
    .unwrap();
    let missing_override = dir.path().join("does-not-exist.json");
    // SAFETY: see above.
    unsafe { std::env::set_var(AI_CONFIG_OVERRIDE_ENV, &missing_override) };
    assert_eq!(
        resolved_ai_config_path(),
        None,
        "a redirected caller must never pick up a $HOME-scoped legacy file"
    );
    assert_eq!(load_ai_config(), None);

    // ── Override unset: default path resolves and saves as before ──
    // SAFETY: see above.
    unsafe { std::env::remove_var(AI_CONFIG_OVERRIDE_ENV) };
    assert_eq!(ai_config_path(), default_path);
    save_ai_config(
        "deepseek",
        Some("sk-default-key"),
        None,
        Some("deepseek-chat"),
    );
    let saved = read_json(&default_path);
    assert_eq!(saved["provider"], "deepseek");
    assert_eq!(saved["api_key"], "sk-default-key");
    assert_eq!(
        load_ai_config().map(|(kind, ..)| kind),
        Some("deepseek".to_string())
    );

    // ── An empty override value is treated as unset ──
    // SAFETY: see above.
    unsafe { std::env::set_var(AI_CONFIG_OVERRIDE_ENV, "") };
    assert_eq!(ai_config_path(), default_path);
}
