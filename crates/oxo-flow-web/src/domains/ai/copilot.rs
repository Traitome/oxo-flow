//! Prompt assembly for AI features.
//!
//! This module assembles prompts by combining user intent with deterministic
//! API results. It does NOT call any AI provider — service.rs handles that.

use crate::domains::ai::types::OptimizeConstraints;
use crate::domains::execution::types::DiagnosticsResponse;

/// Assemble the translate prompt: intent -> .oxoflow generation.
pub fn assemble_translate_prompt(
    intent: &str,
    data_summary: Option<&str>,
    templates: &[String],
) -> (String, String) {
    let system = r#"You are a bioinformatics pipeline expert. Generate valid .oxoflow TOML configurations.
Rules: 1) Output ONLY valid TOML between ```toml fences. 2) Use common bioinformatics tools: fastp, STAR, HISAT2, samtools, featureCounts, GATK, etc. 3) Include [workflow] header with name and version. 4) Each rule needs name, shell/script, input, output. 5) Use {sample} wildcards for multiple samples. 6) Use conda environment specifications where appropriate."#;

    let mut user = format!("Generate a bioinformatics workflow for: {intent}\n\n");
    if let Some(summary) = data_summary {
        user.push_str(&format!("Data analysis results:\n{summary}\n\n"));
    }
    if !templates.is_empty() {
        user.push_str("Available templates for reference:\n");
        for t in templates.iter().take(3) {
            user.push_str(&format!("- {t}\n"));
        }
    }
    user.push_str("\nGenerate the .oxoflow TOML configuration:");
    (system.to_string(), user)
}

/// Assemble the explain prompt: diagnostics -> human-readable explanation.
pub fn assemble_explain_prompt(
    diagnostics: &DiagnosticsResponse,
    log_excerpt: &str,
    language: &str,
) -> (String, String) {
    let system = "You are a bioinformatics debugging expert. Explain pipeline failures clearly and suggest actionable fixes. Be concise and technical.";

    let mut user = String::from("Analyze this failed bioinformatics pipeline run:\n\n");
    for node in &diagnostics.failed_nodes {
        user.push_str(&format!("Rule: {}\n", node.rule));
        user.push_str(&format!("Likely cause: {}\n", node.likely_cause));
        if let Some(ref pattern) = node.error_pattern {
            user.push_str(&format!("Error pattern: {pattern}\n"));
        }
        user.push('\n');
    }
    user.push_str(&format!("Log excerpt:\n{log_excerpt}\n\n"));
    user.push_str(&format!(
        "Explain what went wrong and suggest fixes in {language}."
    ));
    (system.to_string(), user)
}

/// Assemble the interpret prompt: results -> findings + caveats.
pub fn assemble_interpret_prompt(
    _run_id: &str,
    result_type: &str,
    output_summary: &str,
) -> (String, String) {
    let system = "You are a bioinformatics results interpreter. Provide structured interpretation with findings, significance, caveats, and suggested next steps. Be honest about limitations.";

    let user = format!(
        "Interpret these {result_type} analysis results:\n\n{output_summary}\n\n\
         Provide: 1) Narrative summary 2) Key findings with significance \
         3) Important caveats 4) Suggested next analysis steps."
    );
    (system.to_string(), user)
}

/// Assemble the optimize prompt: pipeline -> performance improvements.
pub fn assemble_optimize_prompt(
    toml_content: &str,
    goal: &str,
    constraints: Option<&OptimizeConstraints>,
) -> (String, String) {
    let system = "You are a bioinformatics pipeline optimizer. Suggest parameter changes to improve performance. Output ONLY the modified TOML between ```toml fences.";

    let mut user =
        format!("Optimize this .oxoflow pipeline for: {goal}\n\n```toml\n{toml_content}\n```\n\n");
    if let Some(c) = constraints {
        if let Some(ref mem) = c.max_memory {
            user.push_str(&format!("Constraint: max memory = {mem}\n"));
        }
        if let Some(t) = c.max_threads {
            user.push_str(&format!("Constraint: max threads = {t}\n"));
        }
    }
    user.push_str(
        "\nSuggest parameter changes (threads, memory, tool alternatives) to achieve the goal.",
    );
    (system.to_string(), user)
}
