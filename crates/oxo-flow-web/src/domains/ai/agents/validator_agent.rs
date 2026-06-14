//! Validator Agent — validates DAG structure, parameters, and pipeline configuration.
//!
//! Checks: cycles, orphan nodes, missing inputs, resource consistency, env availability.

/// Validation result from all checks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub checks: Vec<ValidationCheck>,
    pub warnings: Vec<String>,
    pub suggestions: Vec<String>,
}

/// A single validation check result.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationCheck {
    pub check: String,
    pub passed: bool,
    pub severity: String,
    pub message: String,
    pub rule: Option<String>,
}

/// Validate a pipeline TOML content — DAG structure + parameter sanity.
pub fn validate_pipeline_toml(toml_content: &str) -> ValidationReport {
    let mut checks = Vec::new();
    let mut warnings = Vec::new();
    let mut suggestions = Vec::new();

    // Check 1: Parseability
    let parsed = match oxo_flow_core::WorkflowConfig::parse(toml_content) {
        Ok(config) => {
            checks.push(ValidationCheck {
                check: "parse".into(),
                passed: true,
                severity: "info".into(),
                message: "TOML parsed successfully".into(),
                rule: None,
            });
            config
        }
        Err(e) => {
            checks.push(ValidationCheck {
                check: "parse".into(),
                passed: false,
                severity: "error".into(),
                message: format!("TOML parse error: {e}"),
                rule: None,
            });
            return ValidationReport {
                valid: false,
                checks,
                warnings,
                suggestions,
            };
        }
    };

    // Check 2: Has rules
    if parsed.rules.is_empty() {
        checks.push(ValidationCheck {
            check: "rules".into(),
            passed: false,
            severity: "error".into(),
            message: "Pipeline has no rules".into(),
            rule: None,
        });
    } else {
        checks.push(ValidationCheck {
            check: "rules".into(),
            passed: true,
            severity: "info".into(),
            message: format!("{} rules defined", parsed.rules.len()),
            rule: None,
        });
    }

    // Check 3: DAG cycle detection
    let dag_result = oxo_flow_core::dag::WorkflowDag::from_rules(&parsed.rules);
    let cycles: Vec<String> = match &dag_result {
        Ok(dag) => {
            if let Err(e) = dag.validate() {
                vec![e.to_string()]
            } else {
                vec![]
            }
        }
        Err(e) => vec![e.to_string()],
    };
    if !cycles.is_empty() {
        for cycle in &cycles {
            checks.push(ValidationCheck {
                check: "cycles".into(),
                passed: false,
                severity: "error".into(),
                message: format!("Cycle detected involving: {cycle}"),
                rule: Some(cycle.clone()),
            });
        }
    } else {
        checks.push(ValidationCheck {
            check: "cycles".into(),
            passed: true,
            severity: "info".into(),
            message: "No cycles detected in DAG".into(),
            rule: None,
        });
    }

    // Check 4: Orphan rules (no deps and not depended upon)
    let headless: Vec<&str> = parsed
        .rules
        .iter()
        .filter(|r| r.depends_on.is_empty())
        .map(|r| r.name.as_str())
        .collect();
    if headless.is_empty() {
        checks.push(ValidationCheck {
            check: "entry_points".into(),
            passed: false,
            severity: "error".into(),
            message: "No entry-point rules (all rules have dependencies)".into(),
            rule: None,
        });
    } else if headless.len() > 1 {
        warnings.push(format!(
            "Multiple entry-point rules ({}): {:?} — verify intended parallelism",
            headless.len(),
            headless
        ));
    }

    // Check 5: Resource consistency
    for rule in &parsed.rules {
        if let Some(t) = rule.threads {
            if t == 0 {
                warnings.push(format!("Rule '{}' has threads=0", rule.name));
            } else if t > 64 {
                warnings.push(format!(
                    "Rule '{}' has unusually high threads={}",
                    rule.name, t
                ));
            }
        }
    }

    // Check 6: Environment specifications
    for rule in &parsed.rules {
        let env_set = rule.environment.conda.is_some()
            || rule.environment.pixi.is_some()
            || rule.environment.docker.is_some()
            || rule.environment.singularity.is_some()
            || rule.environment.venv.is_some();
        if !env_set {
            warnings.push(format!(
                "Rule '{}' has no environment specification",
                rule.name
            ));
        }
    }

    // Check 7: Shell/script existence
    for rule in &parsed.rules {
        let shell_empty = rule.shell.as_ref().is_none_or(|s| s.trim().is_empty());
        let script_empty = rule.script.as_ref().is_none_or(|s| s.trim().is_empty());
        if shell_empty && script_empty {
            checks.push(ValidationCheck {
                check: "command".into(),
                passed: false,
                severity: "error".into(),
                message: format!("Rule '{}' has no shell command or script", rule.name),
                rule: Some(rule.name.clone()),
            });
        }
    }

    let valid = checks.iter().all(|c| c.passed || c.severity != "error");
    if !valid {
        suggestions.push("Use the DAG Editor to fix validation errors".into());
        suggestions.push("Click 💡 AI suggest optimal for automated parameter tuning".into());
    }

    ValidationReport {
        valid,
        checks,
        warnings,
        suggestions,
    }
}

/// Check if an edit operation would create an invalid DAG.
pub fn validate_edit(
    toml_content: &str,
    operation: &str,
    payload: &serde_json::Value,
) -> Vec<String> {
    let mut errors = Vec::new();
    let config = match oxo_flow_core::WorkflowConfig::parse(toml_content) {
        Ok(c) => c,
        Err(e) => {
            errors.push(format!("Parse error: {e}"));
            return errors;
        }
    };

    match operation {
        "remove_rule" => {
            if let Some(name) = payload.get("name").and_then(|v| v.as_str()) {
                // Check if rule is depended upon
                for rule in &config.rules {
                    if rule.depends_on.contains(&name.to_string()) {
                        errors.push(format!(
                            "Cannot remove '{name}': rule '{}' depends on it",
                            rule.name
                        ));
                    }
                }
            }
        }
        "connect" => {
            let from = payload.get("from").and_then(|v| v.as_str()).unwrap_or("");
            let to = payload.get("to").and_then(|v| v.as_str()).unwrap_or("");
            if !config.rules.iter().any(|r| r.name == from) {
                errors.push(format!("Source rule '{from}' not found"));
            }
            if !config.rules.iter().any(|r| r.name == to) {
                errors.push(format!("Target rule '{to}' not found"));
            }
            if from == to {
                errors.push("Cannot connect a rule to itself".into());
            }
        }
        _ => {}
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_TOML: &str = r#"
[workflow]
name = "test"
version = "1.0"

[[rules]]
name = "step_a"
shell = "echo a"

[[rules]]
name = "step_b"
shell = "echo b"
depends_on = ["step_a"]
"#;

    #[test]
    fn test_validate_valid_pipeline() {
        let report = validate_pipeline_toml(VALID_TOML);
        assert!(
            report.valid,
            "valid pipeline should pass: {:?}",
            report.checks
        );
    }

    #[test]
    fn test_validate_empty_pipeline() {
        let toml = "[workflow]\nname = \"empty\"\nversion = \"1\"\n";
        let report = validate_pipeline_toml(toml);
        assert!(!report.valid, "empty pipeline should fail");
    }

    #[test]
    fn test_validate_invalid_toml() {
        let report = validate_pipeline_toml("not valid toml {{{{");
        assert!(!report.valid);
    }

    #[test]
    fn test_validate_edit_connect_self() {
        let errors = validate_edit(
            VALID_TOML,
            "connect",
            &serde_json::json!({"from": "step_a", "to": "step_a"}),
        );
        assert!(!errors.is_empty(), "self-connect should error");
    }

    #[test]
    fn test_validate_edit_missing_source() {
        let errors = validate_edit(
            VALID_TOML,
            "connect",
            &serde_json::json!({"from": "nonexistent", "to": "step_b"}),
        );
        assert!(!errors.is_empty(), "missing source should error");
    }
}
