//! AI-powered workflow analysis for dry-run and validate commands.

use anyhow::Result;
use colored::Colorize;
use oxo_flow_ai::{knowledge::builtin, provider::AiProvider};
use std::path::Path;

/// Run AI analysis on a workflow file.
/// Shared between `dry-run --ai` and `validate --ai`.
pub async fn analyze_workflow(
    workflow_path: &Path,
    provider: &AiProvider,
    command: &str, // "dry-run" or "validate"
) -> Result<()> {
    let toml_content = std::fs::read_to_string(workflow_path)
        .map_err(|e| anyhow::anyhow!("Cannot read {}: {e}", workflow_path.display()))?;

    println!(
        "{} {}",
        format!("AI Workflow Analysis ({command})").bold().green(),
        format!("— {}", workflow_path.display()).dimmed()
    );
    println!(
        "  Model: {}\n",
        provider.model().unwrap_or_else(|| "default".into())
    );

    let system = build_analysis_prompt();

    let user = format!(
        "## Workflow to Analyze\n\nFile: {}\n\n```toml\n{toml_content}\n```\n\n\
         ## Task\n\
         Analyze this workflow and report issues. For each issue, specify:\n\
         - Severity: ERROR (must fix), WARNING (should fix), or INFO (suggestion)\n\
         - Rule name (or \"global\" for workflow-level issues)\n\
         - Finding description\n\
         - Suggested fix\n\n\
         Output your analysis in this format:\n\
         ```\n\
         [SEVERITY] [rule_name] Finding → Suggested fix\n\
         ...\n\
         ```\n\
         Then provide a 1-2 sentence summary.",
        workflow_path.display()
    );

    println!("{}", "  Analyzing...".bold().cyan());
    let response = provider.chat(&system, &user).await?;

    // Display analysis
    println!("\n{}\n", "Analysis Results".bold().underline());
    println!("{response}");

    // Count issues by severity
    let errors = response.lines().filter(|l| l.contains("[ERROR]")).count();
    let warnings = response.lines().filter(|l| l.contains("[WARNING]")).count();
    let infos = response.lines().filter(|l| l.contains("[INFO]")).count();

    println!("\n{}", "Summary".bold().underline());
    println!(
        "  {} errors, {} warnings, {} suggestions",
        errors.to_string().red(),
        warnings.to_string().yellow(),
        infos.to_string().dimmed()
    );

    if errors > 0 {
        println!(
            "\n{} Fix errors before running this workflow.",
            "⚠".yellow()
        );
    } else if warnings > 0 {
        println!("\n{} Review warnings before running.", "ℹ".dimmed());
    } else {
        println!("\n{} No issues found.", "✓".green());
    }

    Ok(())
}

/// Build the system prompt for workflow analysis.
fn build_analysis_prompt() -> String {
    let tool_table = builtin::format_tool_table();
    let best_practices = builtin::format_best_practices();

    format!(
        r#"## Role
You are oxo-flow's pipeline quality auditor — a bioinformatics workflow expert who analyzes .oxoflow pipelines for correctness, efficiency, and safety.

## Audit Protocol
1. Parse the workflow structure — rules, dependencies, resource allocations
2. For each rule, cross-reference the shell command against the tool reference table
3. Check resource allocations match tool recommendations
4. Flag safety violations (destructive commands, missing QC)
5. Check DAG correctness — are all edges valid? any cycles?
6. Verify environment declarations exist for all rules

## Tool Reference
{tool_table}

## Best Practices Checklist
{best_practices}

## Safety Rules (Non-Negotiable)
- Rules MUST have threads and memory declared
- Rules MUST have environment (conda or container)
- No destructive commands (rm -rf, force overwrite)
- Every data-processing rule should connect to a QC step
- Input files must exist or be produced by a dependency
- depends_on must reference valid rule names

## Output Format
Report each issue as:
[SEVERITY] [rule_name] Finding → Suggested fix

Where severity is ERROR (must fix), WARNING (should fix), or INFO (suggestion).

End with a 1-2 sentence summary.
"#
    )
}
