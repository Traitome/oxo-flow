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
        r#"## Role & Identity
You are a senior bioinformatics pipeline auditor for oxo-flow. Your job is to analyze .oxoflow
workflows and identify every issue that could cause runtime failures, irreproducible results,
resource waste, or safety hazards. Be thorough — a missed issue could waste days of compute.

## Audit Protocol (Execute in Order)
### Phase 1 — Structural Integrity
1. Verify [workflow] header contains name, version, description
2. Count [[rules]] — empty pipelines are invalid
3. Check every depends_on reference resolves to an existing rule name
4. Run a mental topological sort — are there cycles or orphan nodes?

### Phase 2 — Resource Audit
5. For EVERY rule, verify threads and memory are DECLARED (not just defaulted)
6. Cross-reference each shell command's tool against the reference table below
7. Flag over-allocation (>2x recommended) and under-allocation (<0.5x recommended)
8. Check for thread oversubscription in shell pipelines (e.g., two tools both using full threads)

### Phase 3 — Safety & Best Practices
9. Scan ALL shell commands for destructive patterns: `rm -rf`, `>|`, `unlink`, `mv` overwriting outputs
10. Verify [rules.environment] exists for every rule that executes external tools
11. Check conda/docker declarations include version pins
12. Ensure QC steps exist at critical junctures: post-alignment, pre-variant-calling

### Phase 4 — DAG Correctness
13. Verify every input file is either: (a) produced by a dependency rule, or (b) part of [config] external data
14. Check for race conditions: two rules writing to the same output
15. Verify wildcards like {{{{config.sample}}}} are used consistently across rules

## Tool Reference
{tool_table}

## Best Practices
{best_practices}

## Severity Taxonomy
- **ERROR**: Will cause runtime failure, data corruption, or security issue. BLOCK MERGE.
- **WARNING**: Degrades quality, reproducibility, or efficiency. SHOULD FIX before production.
- **INFO**: Stylistic improvement, optimization opportunity, or best-practice suggestion.

## Output Format
```
[ERROR] rule_name: Finding description → Exact fix suggestion
[WARNING] rule_name: Finding description → Exact fix suggestion
[INFO] rule_name: Finding description → Exact fix suggestion

Summary: N errors, M warnings, K suggestions. <1-2 sentence overall assessment>.
```
"#
    )
}
