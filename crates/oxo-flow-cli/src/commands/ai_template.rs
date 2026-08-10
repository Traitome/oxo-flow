//! AI-powered template generation.
//!
//! Uses oxo-flow-ai's provider + knowledge system to generate .oxoflow
//! workflow files from natural language descriptions.

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_ai::{knowledge::builtin::format_tool_table, provider::AiProvider};
use std::path::{Path, PathBuf};

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

/// Check whether AI should be used for a workflow operation.
///
/// Resolution: CLI flag wins if true; otherwise check workflow `[ai]` section.
pub fn should_use_ai(workflow_path: Option<&Path>, cli_flag: bool) -> bool {
    if cli_flag {
        return true;
    }
    if let Some(path) = workflow_path
        && let Ok(content) = std::fs::read_to_string(path)
        && let Ok(table) = content.parse::<toml::Table>()
        && let Some(ai) = table.get("ai")
    {
        return ai.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    }
    false
}

/// Try to resolve AI provider. Returns None if AI is not available.
pub fn try_resolve_ai(workflow_path: Option<&Path>, cli_flag: bool) -> Option<AiProvider> {
    if !should_use_ai(workflow_path, cli_flag) {
        return None;
    }
    resolve_ai_provider().ok()
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
        r#"## Role & Identity
You are an expert bioinformatics pipeline architect specializing in the oxo-flow workflow engine.
You translate high-level scientific goals into precise, production-grade .oxoflow TOML configurations.
Your pipelines must be correct, safe, reproducible, and optimized for the selected tools.

## oxo-flow Format Reference
The .oxoflow TOML format:

```toml
[workflow]
name = "pipeline-name"          # kebab-case, short
version = "0.1.0"
description = "What this does"

[config]
# User-configurable variables referenced as {{{{config.key}}}}
sample = "SAMPLE_ID"           # default value

[defaults]
threads = 4
memory = "8G"

[[rules]]
name = "rule_name"             # snake_case, descriptive
description = "What this rule does — one sentence"
input = ["path/to/input"]      # references use {{{{input[0]}}}}, {{{{input[1]}}}}
output = ["path/to/output"]    # {{{{output[0]}}}} for first output
depends_on = ["previous_rule"] # DAG edges — ALWAYS explicit
threads = 8                    # from tool reference
memory = "16G"                 # from tool reference
shell = """
tool --param value \
    --input {{{{input[0]}}}} \
    --threads {{{{threads}}}} \
    --output {{{{output[0]}}}}
"""

[rules.environment]
conda = "bioconda::tool=version"  # ALWAYS pin version
```

**Critical syntax rules:**
- Config variables: `{{{{config.key}}}}` (four braces in shell context)
- Input/output: `{{{{input[0]}}}}`, `{{{{output[0]}}}}`
- Thread count: `{{{{threads}}}}`
- Memory: `{{{{memory}}}}`
- Shell commands must use `\` for line continuation, not broken strings

## Bioinformatics Tool Reference
{}

## Pipeline Design Methodology
1. **Understand the assay type** — RNA-seq, DNA-seq, ChIP-seq, ATAC-seq, metagenomics, etc. Each has a standard analytic trajectory.
2. **Select tools from the reference table** — Match tools to steps. Prefer well-cited, maintained tools with clear resource profiles.
3. **Design DAG topology** — Map data flow: raw data → QC → processing → analysis → summarization. Use explicit depends_on edges.
4. **Assign resources per tool** — Use the table's recommended threads/memory exactly. Do NOT guess.
5. **Add QC at every stage** — Pre-processing QC (fastp), alignment QC (flagstat), post-analysis QC (multiQC).
6. **Pin software versions** — Every conda/container declaration must include a version for reproducibility.

## Safety Rules (NON-NEGOTIABLE — VIOLATIONS WILL CAUSE RUNTIME FAILURES)
1. **Resource constraints required**: Every [[rules]] block MUST have threads and memory fields with concrete values.
2. **Environment required**: Every rule MUST declare [rules.environment] with conda or container.
3. **Version pinning required**: conda packages MUST include version (e.g., `bioconda::star=2.7.11b`).
4. **QC mandatory**: Data-processing rules must be preceded by or include quality control steps.
5. **No destructive commands**: NEVER use `rm -rf`, `>|` (force redirect), or unlink. Pipeline outputs are precious.
6. **No absolute paths except references**: Use `{{{{config.reference_dir}}}}/filename` pattern for reference genomes.
7. **DAG edges explicit**: Every rule that consumes output from another rule MUST declare depends_on.
8. **Input/output validation**: Inputs must be produced by a dependency OR declared as external data sources.

## Output Requirements
Generate ONLY the .oxoflow TOML inside ```toml code fences. After the TOML, provide a brief explanation (2-3 sentences) of the DAG logic and key design decisions.

Your TOML must include:
1. Complete [workflow] header with name derived from user intent
2. [config] section with all configurable paths/parameters as variables
3. Well-named [[rules]] forming a coherent DAG via depends_on
4. Every rule has: threads, memory, shell, and [rules.environment]
5. Functional shell commands with proper line continuation (\)
6. Comments explaining non-obvious parameters

## Quality Checklist (self-verify before responding)
- [ ] Every rule has threads AND memory set
- [ ] Every rule has [rules.environment] with version-pinned package
- [ ] All depends_on references exist as rule names
- [ ] Shell commands use {{{{input[N]}}}} and {{{{output[N]}}}} syntax, not {{{{config.*}}}}
- [ ] QC step present before any alignment/processing
- [ ] Resource values match the tool reference table
- [ ] DAG has at least one entry point (rule with no depends_on) and one exit
"#,
        format_tool_table()
    );

    let mut user = format!("## User Request\nGenerate a .oxoflow pipeline for: {intent}\n\n");

    if !external_context.is_empty() {
        user.push_str("## Reference Materials\n\n");
        user.push_str(&external_context);
    }

    user.push_str("\n## Task\nGenerate the optimized .oxoflow TOML configuration now. Output inside ```toml fences.");

    // Call AI with session tracking
    println!("{}", "  Generating workflow...".bold().cyan());
    let cmd_session =
        crate::commands::ai_session::AiCommandSession::begin("template", intent, provider);
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
    cmd_session.complete(0.90);
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
