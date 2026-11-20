//! Shared config-override resolution (issue #1277, audit ledger RB-04).
//!
//! The CLI (`run` / `dry-run` / `env touch`) and the web run-creation
//! admission path must resolve `config_overrides` through the SAME code, so
//! the DAG they feed to the canonical BioType checker is identical for the
//! same workflow + overrides. This module is that single definition; the CLI
//! wraps it to keep its `anyhow` error type and its extra non-fatal warnings
//! (retired-key, unknown-key), the web calls it directly.
//!
//! Behavior contract (matches the pre-#1277 CLI implementation byte-for-byte
//! so no user-visible message changes):
//! - a declared `[config]` key with an override value is validated against
//!   its [`ConfigDef`] and written into `config.config`;
//! - a declared key with a default but no override falls back to the default
//!   (existing entries win);
//! - a declared key that is `required` with no value errors;
//! - undeclared `KEY=VALUE` pairs are injected verbatim, so a CLI override
//!   wins over the workflow's `[config]` table (issue #62).

use super::model::{ConfigDef, WorkflowConfig};
use std::collections::{HashMap, HashSet};

/// Resolve an override key that does not exactly match a declared `[config]`
/// key by normalizing hyphens to underscores (issue #794). Returns the
/// declared key only when the normalization is unambiguous — a normalized
/// form that would map to two declared keys is left alone (the caller then
/// applies issue #62/#71 semantics to the literal spelling). Callers invoke
/// this only after an exact-match check, and a key reaching here always
/// contains a hyphen, so the result never equals the input.
///
/// Shared by the CLI's `parse_cli_overrides` and the web admission path
/// (issue #1277, RB-04) so both resolve a hyphen-spelled override onto the
/// declared spelling identically.
pub fn canonicalize_override_key(
    key: &str,
    declared_config_keys: &HashSet<String>,
) -> Option<String> {
    if !key.contains('-') {
        return None;
    }
    let normalized = key.replace('-', "_");
    if declared_config_keys.contains(&normalized) {
        return Some(normalized);
    }
    let matches: Vec<&String> = declared_config_keys
        .iter()
        .filter(|d| d.replace('-', "_") == normalized)
        .collect();
    match matches.as_slice() {
        [only] => Some((*only).clone()),
        _ => None,
    }
}

/// Parse equals-form override strings (`KEY=VALUE`, optionally `--KEY=VALUE`)
/// into a value map, canonicalizing a hyphen-spelled key onto its declared
/// `[config]` spelling (issue #794). Entries without `=` are skipped — the
/// callers' shape gates reject them first.
///
/// This is the equals-form core of the CLI's `parse_cli_overrides`, extracted
/// so the web run-creation admission path resolves the SAME `config_overrides`
/// into the SAME map the CLI does before building and checking the DAG
/// (issue #1277, audit ledger RB-04 — "reuse, never retype").
pub fn parse_config_override_pairs(
    entries: &[String],
    declared_config_keys: &HashSet<String>,
) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for arg_str in entries {
        let Some(eq) = arg_str.find('=') else {
            continue;
        };
        let mut k = arg_str[..eq].trim_start_matches('-').to_string();
        if k.is_empty() {
            continue;
        }
        let v = arg_str[eq + 1..].to_string();
        if !declared_config_keys.contains(&k)
            && let Some(canonical) = canonicalize_override_key(&k, declared_config_keys)
        {
            k = canonical;
        }
        values.insert(k, v);
    }
    values
}

/// Validate a config value against its [`ConfigDef`] declaration.
///
/// `from_cli` distinguishes an EXPLICIT user-provided value from a
/// workflow-declared default: an empty string from the CLI is a legitimate
/// derive-if-empty override (issue #430) and skips the path checks, while
/// defaults stay strict.
///
/// Returns the user-facing message on failure (core has no `anyhow`); callers
/// wrap it in their own error type.
pub fn validate_config_value(
    name: &str,
    value: &str,
    def: &ConfigDef,
    from_cli: bool,
) -> Result<(), String> {
    // ── choices ──
    if let Some(ref choices) = def.choices
        && !choices.iter().any(|c| c == value)
    {
        return Err(format!(
            "invalid value '{}' for config '{}'. Allowed: [{}]\n  {}",
            value,
            name,
            choices.join(", "),
            def.help.as_deref().unwrap_or("")
        ));
    }

    // ── type validation ──
    if let Some(ref type_) = def.type_ {
        match type_.as_str() {
            "int" => {
                value.parse::<i64>().map_err(|_| {
                    format!(
                        "config '{}' expects an integer, got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    )
                })?;
            }
            "float" => {
                value.parse::<f64>().map_err(|_| {
                    format!(
                        "config '{}' expects a float, got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    )
                })?;
            }
            "bool" => {
                if value != "true" && value != "false" {
                    return Err(format!(
                        "config '{}' expects a boolean (true/false), got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    ));
                }
            }
            "path" => {
                // issue #430: an explicitly-passed EMPTY value is a
                // legitimate override — pipelines branch on
                // `[ -n "{config.x}" ]` for derive-if-empty behavior, so
                // `--gene_bed ""` selects "derive from fasta+gtf" and must
                // bypass both the empty-path check and `must_exist` (there
                // is no file to exist yet). Only DEFAULTS keep the strict
                // check: a workflow author's default must point at a real
                // file.
                if value.is_empty() && from_cli {
                    return Ok(());
                }
                if value.is_empty() {
                    return Err(format!(
                        "config '{}' expects a path, got empty string\n  {}",
                        name,
                        def.help.as_deref().unwrap_or("")
                    ));
                }
                // must_exist check
                if def.must_exist {
                    let p = std::path::Path::new(value);
                    if !p.exists() {
                        return Err(format!(
                            "config '{}' path does not exist: '{}'\n  {}",
                            name,
                            value,
                            def.help.as_deref().unwrap_or("")
                        ));
                    }
                }
            }
            _ => { /* "string" or unknown — no validation */ }
        }
    }

    // ── range validation (requires type = int or float) ──
    if let Some(ref range) = def.range
        && let Some((min_str, max_str)) = range.split_once("..")
    {
        let min: f64 = min_str
            .trim()
            .parse()
            .map_err(|_| format!("invalid range min '{}' for config '{}'", min_str, name))?;
        let max: f64 = max_str
            .trim()
            .parse()
            .map_err(|_| format!("invalid range max '{}' for config '{}'", max_str, name))?;
        let val: f64 = value.parse().map_err(|_| {
            format!(
                "config '{}' has range '{range}' but value '{}' is not numeric\n  {}",
                name,
                value,
                def.help.as_deref().unwrap_or("")
            )
        })?;
        if val < min || val > max {
            return Err(format!(
                "config '{}' value {} is outside range {}..{}\n  {}",
                name,
                val,
                min,
                max,
                def.help.as_deref().unwrap_or("")
            ));
        }
    }

    Ok(())
}

/// Apply parsed overrides and the defaults of declarative config entries
/// (`key = { default, required, … }` in `[config]`).
///
/// Shared by the CLI's `run` / `dry-run` / `env touch` and the web
/// run-creation admission path (issue #1277) so preview, execution, and
/// admission validate identically and build the same resolved DAG.
pub fn apply_config_overrides(
    config: &mut WorkflowConfig,
    overrides: &HashMap<String, String>,
) -> Result<(), String> {
    for (name, cfg_def) in &config.config_meta {
        let effective_value = if let Some(val) = overrides.get(name) {
            validate_config_value(name, val, cfg_def, true)?;
            config
                .config
                .insert(name.clone(), toml::Value::String(val.clone()));
            val.clone()
        } else if let Some(ref default) = cfg_def.default {
            validate_config_value(name, default, cfg_def, false)?;
            config
                .config
                .entry(name.clone())
                .or_insert_with(|| toml::Value::String(default.clone()));
            default.clone()
        } else if cfg_def.required {
            let help_suffix = cfg_def
                .help
                .as_deref()
                .map(|h| format!("\n  {h}"))
                .unwrap_or_default();
            let display_name = if cfg_def.sensitive {
                format!("{name} (sensitive)")
            } else {
                name.clone()
            };
            return Err(format!(
                "required config '{display_name}' not set. Use --{name} <value>{help_suffix}"
            ));
        } else {
            continue;
        };
        // Mask sensitive values in all non-execution output
        if cfg_def.sensitive {
            tracing::info!("config '{}' = **** (sensitive)", name);
        } else {
            tracing::debug!("config '{}' = '{}'", name, effective_value);
        }
    }
    // Inject undeclared KEY=VALUE values as {config.xxx} — an override must
    // win over the workflow's [config] table (issue #62).
    for (k, v) in overrides {
        config
            .config
            .insert(k.clone(), toml::Value::String(v.clone()));
    }
    Ok(())
}
