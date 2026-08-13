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
    from_urls: &[String],
    from_files: &[PathBuf],
    output: Option<PathBuf>,
    ai_max_retries: Option<u32>,
) -> Result<()> {
    // L1-L3: Initialize AI runtime with scope config + tools
    let project_dir = std::env::current_dir().ok();
    let runtime =
        crate::commands::ai_runtime::AiRuntime::new(None, project_dir.as_deref(), ai_max_retries)
            .await?;
    let provider = &runtime.provider;

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
    let mut system = format!(
        r#"## Role & Identity
You are an expert bioinformatics pipeline architect specializing in the oxo-flow workflow engine.
You translate high-level scientific goals into precise, production-grade .oxoflow TOML configurations.
Your pipelines must be correct, safe, reproducible, and optimized for the selected tools.

## oxo-flow TOML Syntax Reference (CRITICAL — VIOLATIONS CAUSE RUNTIME FAILURES)

### Template variable syntax: SINGLE braces ONLY
oxo-flow uses SINGLE curly braces for all template variables. NEVER use double braces.

```
CORRECT:   {{config.sample}}    {{input[0]}}    {{threads}}    {{output[0]}}    {{memory}}
WRONG:     {{{{config.sample}}}}  {{{{input[0]}}}}  {{{{threads}}}}  {{{{output[0]}}}}  {{{{memory}}}}
```

### TOML syntax: standard TOML only
- TOML does NOT support `+=` assignment. Use `=` only.
- TOML does NOT support `{{%%}}` or `{{{{}}}}` syntax — only `{{var}}`.
- Multi-line strings use triple quotes: `"""..."""`.
- Shell command line continuation uses `\` at end of line.

### Full example (study this carefully):

```toml
[workflow]
name = "pipeline-name"
version = "0.1.0"
description = "What this does"

[config]
sample = "SAMPLE_ID"
ref_fasta = "reference/hg38.fa"

[defaults]
threads = 4
memory = "8G"

[[rules]]
name = "fastp"
description = "Trim and QC reads"
input = ["raw/{{config.sample}}_R1.fastq.gz", "raw/{{config.sample}}_R2.fastq.gz"]
output = ["trimmed/{{config.sample}}_R1.fq.gz", "trimmed/{{config.sample}}_R2.fq.gz", "qc/fastp.json"]
threads = 4
memory = "8G"
depends_on = []
shell = """
fastp \
    --in1 {{input[0]}} \
    --in2 {{input[1]}} \
    --out1 {{output[0]}} \
    --out2 {{output[1]}} \
    --json {{output[2]}} \
    --thread {{threads}}
"""

[rules.environment]
conda = "bioconda::fastp=0.23.4"

[[rules]]
name = "bwa_mem"
description = "Align reads with BWA-MEM"
input = ["trimmed/{{config.sample}}_R1.fq.gz", "trimmed/{{config.sample}}_R2.fq.gz"]
output = ["aligned/{{config.sample}}.bam"]
threads = 8
memory = "24G"
depends_on = ["fastp"]
shell = """
bwa mem \
    -t {{threads}} \
    -R "@RG\\tID:{{config.sample}}\\tSM:{{config.sample}}\\tPL:ILLUMINA" \
    {{config.ref_fasta}} \
    {{input[0]}} {{input[1]}} \
    | samtools sort -@ 4 -o {{output[0]}}
"""

[rules.environment]
conda = "bioconda::bwa=0.7.17"
```

### KEY SYNTAX RULES (MEMORIZE THESE):
1. Template variables use SINGLE braces: `{{config.key}}`, `{{input[0]}}`, `{{output[0]}}`, `{{threads}}`, `{{memory}}`
2. NEVER use `{{{{var}}}}` (double-brace) — this is Python/Go syntax, NOT oxo-flow
3. NEVER use `shell +=` — this is Python/Snakemake, NOT valid TOML
4. NEVER concatenate strings with `+` in TOML
5. Environment tables [rules.environment] MUST appear directly after their [[rules]] block
6. depends_on arrays list rule names, not file names
7. ALL conda packages MUST include version: `bioconda::tool=X.Y.Z`

## Bioinformatics Tool Reference
{}

## Embedded Bioconda Tool Database
You have a `lookup_tool` function that searches the FULL embedded Bioconda CLI
database (6103 tools with current versions and descriptions). Use it to:
- Confirm a tool exists and its exact Bioconda package name
- Get the CURRENT version for pinning (e.g. `lookup_tool("samtools")` → 1.23.x)
- Discover alternative tools by purpose keyword (e.g. `lookup_tool("peak calling")`)
- Check platform support before recommending a tool

## Pipeline Design Methodology
1. **Understand the assay type** — RNA-seq, DNA-seq, ChIP-seq, ATAC-seq, metagenomics, etc.
2. **Select tools** — Match tools to steps. Prefer the curated reference table above; use `lookup_tool` for anything not listed there or to verify current versions.
3. **Design DAG topology** — Map data flow: raw data → QC → processing → analysis → summarization.
4. **Assign resources per tool** — Use the table's recommended threads/memory exactly. Do NOT guess.
5. **Add QC at every stage** — Pre-processing QC (fastp), alignment QC (flagstat), post-analysis QC (multiQC).
6. **Pin software versions** — Every conda/container declaration must include a version. Use `lookup_tool` to get the current Bioconda version; fall back to your knowledge if the lookup misses.

## Safety Rules (NON-NEGOTIABLE)
1. **Resource constraints required**: Every [[rules]] block MUST have threads and memory fields.
2. **Environment required**: Every rule MUST declare [rules.environment] with conda or container.
3. **Version pinning required**: conda packages MUST include version (e.g., `bioconda::star=2.7.11b`).
4. **QC mandatory**: Include QC steps at critical junctures.
5. **No destructive commands**: NEVER use `rm -rf`, `>|` (force redirect), or unlink.
6. **No absolute paths except references**: Use `{{config.ref_dir}}/filename` pattern.
7. **DAG edges explicit**: Every rule consuming another's output MUST declare depends_on.
8. **Input/output validation**: Inputs must be produced by a dependency OR declared external.

## Output Requirements
Generate ONLY the .oxoflow TOML inside ```toml code fences. After the TOML, provide a brief explanation of the DAG logic and key design decisions.

Your TOML MUST include:
1. Complete [workflow] header with name derived from user intent
2. [config] section with configurable paths/parameters as variables
3. Well-named [[rules]] forming a coherent DAG via depends_on
4. Every rule has: threads, memory, shell, and [rules.environment]
5. Functional shell commands using SINGLE-brace template syntax: `{{input[0]}}`, `{{output[0]}}`, `{{threads}}`, `{{config.key}}`

## Quality Checklist (self-verify before responding)
- [ ] Every rule has threads AND memory set
- [ ] Every rule has [rules.environment] with version-pinned package
- [ ] All depends_on references exist as rule names
- [ ] Template variables use SINGLE braces: `{{var}}` NOT `{{{{var}}}}`
- [ ] NO `shell +=` or string concatenation — valid TOML only
- [ ] QC step present before any alignment/processing
- [ ] Resource values match the tool reference table
"#,
        format_tool_table()
    );

    // User-defined skills explicitly activated via [ai] skills (pure
    // prompt injection — never code execution).
    if !runtime.skill_context.is_empty() {
        system.push_str("\n\n## Activated Custom Skills\n");
        system.push_str(&runtime.skill_context);
    }

    // Inject domain-matched bioinformatics skills (bioSkills) so the
    // generated workflow follows curated domain expertise: correct tool
    // choice, parameters, and known caveats.
    let intent_domains = oxo_flow_ai::knowledge::skills::domains_for_intent(intent);
    let mut skill_context = String::new();
    let mut seen_skills = std::collections::HashSet::new();
    for domain in &intent_domains {
        for skill in oxo_flow_ai::knowledge::skills::skills_in_domain(domain)
            .into_iter()
            .take(3)
        {
            if seen_skills.insert(skill.name.clone()) {
                skill_context.push_str(&format!(
                    "- [{}] {}: {} ({})\n",
                    skill.domain,
                    skill.name,
                    skill.description,
                    if skill.primary_tool.is_empty() {
                        "general"
                    } else {
                        &skill.primary_tool
                    }
                ));
            }
        }
    }
    if !intent_domains.is_empty() {
        println!(
            "{} Matched domains: {}",
            "  •".dimmed(),
            intent_domains.join(", ").cyan()
        );
    }

    let mut user = format!("## User Request\nGenerate a .oxoflow pipeline for: {intent}\n\n");

    if !skill_context.is_empty() {
        user.push_str("## Domain Expertise (bioSkills)\n");
        user.push_str(&skill_context);
        user.push_str(
            "\nFollow these domain procedures for tool choice, parameters, and caveats where applicable.\n\n",
        );
    }

    if !external_context.is_empty() {
        user.push_str("## Reference Materials\n\n");
        user.push_str(&external_context);
    }

    user.push_str("\n## Task\nGenerate the optimized .oxoflow TOML configuration now. Output inside ```toml fences.");

    // Call AI through AiRuntime (L1-L3 connected)
    println!("{}", "  Generating workflow...".bold().cyan());
    let mut cmd_session =
        crate::commands::ai_session::AiCommandSession::begin("template", intent, provider);

    use oxo_flow_ai::agent::orchestrator::tool_call_approved;
    use oxo_flow_ai::types::Message;

    // Tool-calling loop: the model may query the embedded knowledge tools
    // (lookup_tool / lookup_skill / lookup_pipeline) or MCP tools from
    // activated tool skills. Non-read-only tools require interactive
    // approval; without a terminal they are refused.
    let tool_defs = runtime.tool_registry.to_defs();
    let mut messages = vec![Message::system(&system), Message::user(&user)];
    let max_rounds = runtime.config.max_retries.max(1);
    let mut response_text: Option<String> = None;
    for _round in 0..max_rounds {
        let response = provider
            .chat_with_tools(&messages, &tool_defs)
            .await
            .context("AI provider call failed")?;
        cmd_session.record_usage(&response.usage);

        match response.tool_calls {
            Some(tool_calls) if !tool_calls.is_empty() => {
                messages.push(Message::assistant_with_tools(tool_calls.clone()));
                for tc in tool_calls {
                    // Shared approval policy (orchestrator + template loop):
                    // read-only tools auto-run; anything else needs an
                    // interactive approval before execution.
                    let approved =
                        tool_call_approved(&runtime.tool_registry, None, &tc.name, &tc.arguments)
                            || crate::commands::ai_runtime::prompt_tool_approval(
                                &tc.name,
                                &tc.arguments,
                            )
                            .await;
                    let start = std::time::Instant::now();
                    let result = if approved {
                        runtime.tool_registry.execute(&tc.name, &tc.arguments).await
                    } else {
                        Err(oxo_flow_ai::error::AiError::ToolError {
                            tool: tc.name.clone(),
                            message: "execution requires human approval".to_string(),
                        })
                    };
                    let duration_ms = start.elapsed().as_millis() as u64;
                    let content = match result {
                        Ok(content) => content,
                        Err(e) => format!("tool error: {e}"),
                    };
                    cmd_session.record_tool_call(
                        &tc.name,
                        &tc.arguments,
                        &content,
                        !content.starts_with("tool error"),
                        duration_ms,
                    );
                    messages.push(Message::tool(&tc.id, &tc.name, &content));
                }
            }
            _ => {
                response_text = response.content;
                break;
            }
        }
    }
    let response_text = match response_text {
        Some(text) => text,
        _ => {
            // The model kept calling tools without finalizing — force one
            // plain answer without any tools.
            let final_response = provider
                .chat_with_tools(&messages, &[])
                .await
                .context("AI provider call failed during final (tool-free) round")?;
            cmd_session.record_usage(&final_response.usage);
            final_response
                .content
                .ok_or_else(|| anyhow::anyhow!("AI response contained no text content"))?
        }
    };

    // Extract TOML
    let toml_content =
        extract_toml(&response_text).context("AI response did not contain valid .oxoflow TOML")?;

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
