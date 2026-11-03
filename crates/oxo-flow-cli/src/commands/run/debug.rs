//! `debug` command execution: expands rule shell commands and explains them,
//! optionally with AI analysis.

use super::*;
use crate::commands::print_banner;
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use std::path::PathBuf;

pub async fn debug_command(workflow: PathBuf, rule_name: Option<String>, ai: bool) -> Result<()> {
    print_banner();
    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    config.apply_defaults();
    config
        .expand_wildcards()
        .context("failed to expand wildcard rules")?;

    // One placeholder map shared by every consumer below (AI analysis, DAG
    // build, rule dump) — all expand from the same config, so they must see
    // the same values.
    let wildcard_values = config_placeholder_values(&config.config);

    // AI: auto-detect from workflow [ai] or explicit --ai flag
    if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai) {
        let expanded: Vec<String> = config
            .rules
            .iter()
            .map(|r| {
                let shell_cmd = r.shell.as_deref().unwrap_or("(no shell command)");
                let shell = oxo_flow_core::executor::process::render_shell_command(
                    shell_cmd,
                    r,
                    &wildcard_values,
                    oxo_flow_core::scheduler::detect_system_limits(),
                );
                format!(
                    "## {}\nthreads={}, memory={}, env={:?}\n```bash\n{}\n```",
                    r.name,
                    r.resources.threads,
                    r.resources.memory.as_deref().unwrap_or("1G"),
                    r.environment,
                    shell
                )
            })
            .collect();
        let joined = expanded.join("\n\n");

        let system = r#"## Role
You are a bioinformatics pipeline debugger. Explain each expanded shell command in plain language,
flagging potential issues: parameter misuse, resource mismatch, missing required flags, or
logical errors. Output format per rule:

**rule_name**: <what this command does — 1 sentence>
⚠ <any issues found — 1 line per issue, or "No issues" if clean>
"#
        .to_string();
        let user = format!(
            "## Workflow: {}\n\n## Expanded Commands\n\n{joined}\n\n## Task\nExplain each command and flag issues.",
            workflow.display()
        );

        println!("{}", "  Analyzing commands...".bold().cyan());
        match provider.chat(&system, &user).await {
            Ok(response) => {
                println!("\n{}\n{response}", "AI Analysis".bold().green().underline());
                return Ok(());
            }
            Err(e) => eprintln!("  AI analysis failed: {e}"),
        }
    }

    let dag = WorkflowDag::from_rules_with_config(&config.rules, &wildcard_values)
        .context("failed to build workflow DAG")?;

    let rules_to_show: Vec<&oxo_flow_core::rule::Rule> = if let Some(ref name) = rule_name {
        match config.rules.iter().find(|r| r.name == *name) {
            Some(r) => vec![r],
            None => {
                // Instance names are what the expanded DAG holds, but every
                // other surface — the workflow file, `lint`, `validate`,
                // docs — uses the template name (issue #862). Accept it and
                // show every instance expanded from that template.
                let instances: Vec<&oxo_flow_core::rule::Rule> = config
                    .rules
                    .iter()
                    .filter(|r| config.template_of(&r.name) == Some(name.as_str()))
                    .collect();
                if !instances.is_empty() {
                    eprintln!(
                        "{} template '{}' expanded to {}",
                        "Debug:".bold().cyan(),
                        name,
                        oxo_flow_core::plural(instances.len(), "instance", "instances")
                    );
                    instances
                } else {
                    let mut candidates: Vec<&str> = config
                        .rule_templates
                        .iter()
                        .map(|r| r.name.as_str())
                        .chain(config.rules.iter().map(|r| r.name.as_str()))
                        .collect();
                    candidates.sort_unstable();
                    candidates.dedup();
                    let suggestions = nearest_rule_names(name, &candidates, 5);
                    if suggestions.is_empty() {
                        eprintln!("{} rule '{}' not found", "error:".bold().red(), name);
                    } else {
                        eprintln!(
                            "{} rule '{}' not found — did you mean: {}?",
                            "error:".bold().red(),
                            name,
                            suggestions.join(", ")
                        );
                    }
                    return Err(anyhow::anyhow!("rule not found"));
                }
            }
        }
    } else {
        config.rules.iter().collect()
    };

    eprintln!(
        "{} Debugging {} rules",
        "Debug:".bold().cyan(),
        rules_to_show.len()
    );

    for rule in &rules_to_show {
        eprintln!("{}", format!("── Rule: {} ──", rule.name).bold().cyan());

        if let Some(ref desc) = rule.description {
            eprintln!("  {} {}", "Description:".dimmed(), desc);
        }

        if !rule.output.is_empty() {
            let expanded_outputs: Vec<String> = rule
                .output
                .iter()
                .map(|o| {
                    oxo_flow_core::executor::checkpoint::expand_config_in_path(o, &wildcard_values)
                })
                .collect();
            eprintln!("  {} {:?}", "Outputs:".dimmed(), expanded_outputs);
        }

        if let Some(ref cmd) = rule.shell {
            let expanded = oxo_flow_core::executor::process::render_shell_command(
                cmd,
                rule,
                &wildcard_values,
                oxo_flow_core::scheduler::detect_system_limits(),
            );
            eprintln!("  {} {}", "Shell (expanded):".dimmed(), expanded);
        }

        if let Ok(deps) = dag.dependencies(&rule.name)
            && !deps.is_empty()
        {
            let mut deps = deps;
            deps.sort();
            deps.dedup();
            eprintln!("  {} {:?}", "Dependencies:".dimmed(), deps);
        }

        eprintln!();
    }

    Ok(())
}
