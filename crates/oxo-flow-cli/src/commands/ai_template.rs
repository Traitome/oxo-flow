//! AI-powered template generation.
//!
//! Uses oxo-flow-ai's provider + knowledge system to generate .oxoflow
//! workflow files from natural language descriptions.

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_ai::{knowledge::builtin::format_tool_table, provider::AiProvider};
use std::path::PathBuf;

/// Resolve AI provider from environment or config, returning an error if not configured.
pub fn resolve_ai_provider() -> Result<AiProvider> {
    let provider = oxo_flow_ai::provider::create_provider_from_env();
    if matches!(provider, AiProvider::Noop) {
        anyhow::bail!(
            "AI provider not configured.\n\
             Set OXO_FLOW_AI_PROVIDER=deepseek and DEEPSEEK_API_KEY=sk-...\n\
             Or configure via ~/.oxo-flow/ai_config.json"
        );
    }
    Ok(provider)
}

/// Generate a workflow from natural language using AI.
pub async fn generate_workflow(
    intent: &str,
    provider: &AiProvider,
    from_urls: &[String],
    from_files: &[PathBuf],
    output: Option<PathBuf>,
) -> Result<()> {
    println!("{}", "AI Template Generator".bold().green());
    println!(
        "  Model: {}",
        provider.model().unwrap_or_else(|| "default".into())
    );
    println!("  Intent: {intent}\n");

    // Build external sources
    let mut external_context = String::new();

    // Fetch URLs
    for url in from_urls {
        println!("{} Fetching {url}...", "  •".dimmed());
        match reqwest::get(url).await {
            Ok(resp) => {
                if let Ok(text) = resp.text().await {
                    let preview = if text.len() > 300 {
                        format!("{}...", &text[..300])
                    } else {
                        text.clone()
                    };
                    external_context.push_str(&format!(
                        "## External Reference: {url}\n\n```\n{preview}\n```\n\n"
                    ));
                    println!("{}   Fetched {} chars", "  ✓".green(), text.len());
                }
            }
            Err(e) => {
                eprintln!("{}   Failed to fetch: {e}", "  ⚠".yellow());
            }
        }
    }

    // Read local files
    for path in from_files {
        println!("{} Reading {}...", "  •".dimmed(), path.display());
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let preview = if content.len() > 2000 {
                    format!("{}...", &content[..2000])
                } else {
                    content.clone()
                };
                external_context.push_str(&format!(
                    "## Reference File: {}\n\n```\n{preview}\n```\n\n",
                    path.display()
                ));
                println!("{}   Read {} chars", "  ✓".green(), content.len());
            }
            Err(e) => {
                eprintln!("{}   Failed to read: {e}", "  ⚠".yellow());
            }
        }
    }

    // Assemble system prompt with knowledge
    let system = format!(
        r#"## Role
You are oxo-flow's AI bioinformatics pipeline architect. You translate scientist intent into valid, production-grade .oxoflow pipeline definitions.

## Core Knowledge
- oxo-flow uses .oxoflow TOML format with [workflow] header and [[rules]] sections
- Rules form a DAG via `depends_on` fields
- Wildcards like {{sample}} are expanded at runtime
- Each rule must specify resources (threads, memory) and environment
- Config variables use {{{{config.key}}}} syntax in shell commands
- Input/output files use {{{{input[0]}}}}, {{{{output[0]}}}} syntax

{}

## Safety Rules (NON-NEGOTIABLE)
- NEVER omit resource constraints (threads, memory) — every rule needs them
- NEVER disable QC steps — quality control is mandatory
- NEVER use `rm -rf` or destructive commands
- NEVER write files outside the pipeline's working directory
- ALWAYS specify environment (conda environment or container)
- ALWAYS use explicit depends_on to establish DAG edges

## Output Format
Generate the complete .oxoflow TOML inside ```toml fences. Include:
1. [workflow] section with name, version, description
2. [config] section with any needed variables
3. [[rules]] sections with name, description, depends_on, input, output, shell, threads, memory
4. [rules.environment] subsections with conda or container

## Example .oxoflow Format
```toml
[workflow]
name = "my-pipeline"
version = "0.1.0"
description = "Analysis pipeline"

[config]
sample = "{{default}}"

[[rules]]
name = "step1"
description = "First step"
output = ["results/{{{{config.sample}}}}_output.txt"]
threads = 4
memory = "8G"
shell = "echo 'Processing' > {{{{output[0]}}}}"

[[rules]]
name = "step2"
description = "Second step"
input = ["results/{{{{config.sample}}}}_output.txt"]
output = ["results/final.txt"]
depends_on = ["step1"]
threads = 2
memory = "4G"
shell = "cat {{{{input[0]}}}} | wc -l > {{{{output[0]}}}}"
```
"#,
        format_tool_table()
    );

    let mut user = format!("## User Request\nGenerate a .oxoflow pipeline for: {intent}\n\n");

    if !external_context.is_empty() {
        user.push_str("## Reference Materials\n\n");
        user.push_str(&external_context);
    }

    user.push_str("\n## Task\nGenerate the optimized .oxoflow TOML configuration now. Output inside ```toml fences.");

    // Call AI
    println!("{}", "  Generating workflow...".bold().cyan());
    let response = provider
        .chat(&system, &user)
        .await
        .context("AI provider call failed")?;

    // Extract TOML
    let toml_content =
        extract_toml(&response).context("AI response did not contain valid .oxoflow TOML")?;

    // Validate basic structure
    validate_basic_structure(&toml_content)?;

    // Try parsing with core engine for extra validation
    match toml::from_str::<oxo_flow_core::config::WorkflowConfig>(&toml_content) {
        Ok(_) => {
            println!("{} Schema validation passed", "  ✓".green());
        }
        Err(e) => {
            println!("{} Schema validation warning: {e}", "  ⚠".yellow());
            println!("  The generated workflow may need manual adjustment.");
        }
    }

    // Write output
    let output_path = output.unwrap_or_else(|| {
        let name = intent
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join("_")
            .to_lowercase()
            .replace(|c: char| !c.is_alphanumeric() && c != '_', "");
        PathBuf::from(format!("{name}.oxoflow"))
    });

    std::fs::write(&output_path, &toml_content)?;
    println!(
        "{} Workflow written to {} ({} bytes)",
        "  ✓".green(),
        output_path.display(),
        toml_content.len()
    );

    // Count rules for summary
    let rule_count = toml_content
        .lines()
        .filter(|l| l.trim().starts_with("[[rules]]"))
        .count();
    println!("  Rules: {rule_count}");
    println!(
        "{}",
        "Done! Review the generated workflow before running.".bold()
    );

    Ok(())
}

/// Extract TOML content from an AI response.
fn extract_toml(response: &str) -> Option<String> {
    // Try ```toml code fence
    if let Some(start) = response.find("```toml") {
        let start = start + 7;
        if let Some(end) = response[start..].find("```") {
            let content = response[start..start + end].trim().to_string();
            if !content.is_empty() {
                return Some(content);
            }
        }
    }
    // Try generic ``` code fence
    if let Some(start) = response.find("```") {
        let start = start + 3;
        // Skip language identifier line if present
        let after_open = &response[start..];
        let content_start = if let Some(newline) = after_open.find('\n') {
            start + newline + 1
        } else {
            start
        };
        if let Some(end) = response[content_start..].find("```") {
            let content = response[content_start..content_start + end]
                .trim()
                .to_string();
            if content.contains("[workflow]") {
                return Some(content);
            }
        }
    }
    // Try raw [workflow] content
    if let Some(pos) = response.find("[workflow]") {
        return Some(response[pos..].trim().to_string());
    }
    None
}

/// Basic structural validation before passing to core engine.
fn validate_basic_structure(toml: &str) -> Result<()> {
    if !toml.contains("[workflow]") {
        anyhow::bail!("Generated TOML missing [workflow] section");
    }
    if !toml.contains("[[rules]]") {
        anyhow::bail!("Generated TOML has no [[rules]] sections");
    }
    if !toml.contains("shell") {
        anyhow::bail!("Generated TOML rules missing 'shell' field");
    }
    if !toml.contains("name") {
        anyhow::bail!("Generated TOML missing 'name' field");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_toml_from_code_fence() {
        let response = "Here is the pipeline:\n```toml\n[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"step1\"\nshell = \"echo hi\"\n```\nDone.";
        let result = extract_toml(response).unwrap();
        assert!(result.contains("[workflow]"));
        assert!(result.contains("[[rules]]"));
        assert!(!result.contains("```"));
    }

    #[test]
    fn extract_toml_raw_workflow() {
        let response =
            "Some text\n[workflow]\nname = \"test\"\n[[rules]]\nname = \"s1\"\nshell = \"echo\"";
        let result = extract_toml(response).unwrap();
        assert!(result.contains("[workflow]"));
    }

    #[test]
    fn extract_toml_no_toml() {
        let result = extract_toml("No TOML here");
        assert!(result.is_none());
    }

    #[test]
    fn validate_basic_structure_good() {
        let toml = "[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"s1\"\nshell = \"echo hi\"";
        assert!(validate_basic_structure(toml).is_ok());
    }

    #[test]
    fn validate_basic_structure_missing_workflow() {
        let toml = "[[rules]]\nname = \"s1\"\nshell = \"echo hi\"";
        assert!(validate_basic_structure(toml).is_err());
    }

    #[test]
    fn validate_basic_structure_missing_rules() {
        let toml = "[workflow]\nname = \"test\"";
        assert!(validate_basic_structure(toml).is_err());
    }

    #[test]
    fn validate_basic_structure_missing_shell() {
        let toml = "[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"s1\"";
        assert!(validate_basic_structure(toml).is_err());
    }
}
