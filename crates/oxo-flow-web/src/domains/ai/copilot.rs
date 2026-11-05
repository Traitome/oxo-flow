//! Context-engineered prompt assembly for AI features.
//!
//! ## Design Principles (from awesome-ai-system-prompts patterns)
//!
//! 1. **Clear Role Definition** — Each prompt starts with who the AI is and why.
//! 2. **Structured Organization** — ## sections for clear parsing.
//! 3. **Safety Protocols** — Explicit trust boundary: what to NEVER do.
//! 4. **Tool Contracts** — Clear tool boundaries and expected outputs.
//! 5. **Step-by-Step Guidance** — The AI is guided through its reasoning process.
//!
//! This module does NOT call any AI provider — `service.rs` handles that.
//! It only assembles prompt strings from deterministic API results.

use crate::domains::ai::types::OptimizeConstraints;
use crate::domains::execution::types::DiagnosticsResponse;

/// Assemble the explain prompt: diagnostics → human-readable explanation.
///
/// ## Role
/// You are a bioinformatics debugging expert. You analyze pipeline failures
/// using deterministic diagnostics data and provide actionable fixes.
///
/// ## Safety
/// - NEVER suggest ignoring errors or silencing warnings.
/// - ALWAYS root-cause analysis first, then fix suggestions.
pub fn assemble_explain_prompt(
    diagnostics: &DiagnosticsResponse,
    log_excerpt: &str,
    language: &str,
) -> (String, String) {
    let system = r#"## Role
You are a bioinformatics debugging expert specializing in pipeline failure analysis for oxo-flow.

## Analysis Protocol
1. FIRST identify the root cause from the diagnostics data
2. SECOND check if the error matches known patterns (OOM, segfault, missing files)
3. THIRD suggest a specific, actionable fix (not generic advice)
4. FINALLY recommend prevention (add checkpointing, increase limits, etc.)

## Known Error Patterns
| Pattern | Symptoms | Action |
|---------|----------|--------|
| OOM (exit 137/9) | Killed by kernel | Increase memory, reduce threads, split input |
| Segfault (exit 139) | Memory corruption | Reduce threads, check data integrity |
| Missing file (exit 1) | "No such file" | Verify paths, check dependencies |
| Disk full | "No space left" | Clean intermediates, increase quota |
| Permission denied | "Access denied" | Check file permissions, user |

## Safety
- NEVER suggest disabling error handling or ignoring failures
- NEVER suggest workarounds that bypass quality control
- Always provide the reasoning behind each fix suggestion
"#;

    let mut user = String::from("## Failed Run Diagnostics\n\n");
    for node in &diagnostics.failed_nodes {
        user.push_str(&format!("### Rule: {}\n", node.rule));
        user.push_str(&format!("- Likely cause: {}\n", node.likely_cause));
        if let Some(ref pattern) = node.error_pattern {
            user.push_str(&format!("- Error pattern: {pattern}\n"));
        }
        if !node.suggestions.is_empty() {
            user.push_str("- Suggested fixes:\n");
            for s in &node.suggestions {
                user.push_str(&format!("  - {s}\n"));
            }
        }
        user.push('\n');
    }
    user.push_str(&format!("## Log Excerpt\n```\n{log_excerpt}\n```\n\n"));
    user.push_str(&format!("## Task\nExplain what went wrong and suggest actionable fixes in {language}. Follow the analysis protocol."));
    (system.to_string(), user)
}

/// Assemble the interpret prompt: results → findings + caveats.
///
/// ## Role
/// You are a bioinformatics results interpreter. You generate scientific
/// narratives from pipeline output, distinguishing signal from noise.
///
/// ## Safety
/// - ALWAYS note limitations and caveats.
/// - NEVER overstate significance — distinguish statistical from biological.
pub fn assemble_interpret_prompt(
    _run_id: &str,
    result_type: &str,
    output_summary: &str,
) -> (String, String) {
    let system = r#"## Role
You are a bioinformatics results interpreter. You read pipeline output and generate structured scientific narratives.

## Interpretation Protocol
1. Review the output data and quality metrics
2. Identify significant findings (statistical AND biological)
3. Note caveats, limitations, and potential confounders
4. Suggest concrete next steps for validation or follow-up

## Output Structure
### Narrative Summary
<concise 2-3 paragraph summary>

### Key Findings
| Finding | Significance | Evidence |
|---------|-------------|----------|
| ... | high/medium/low | ... |

### Caveats
- Limitation 1 (impact: high/medium/low)
- Limitation 2

### Suggested Next Steps
1. ... (priority: high/medium/low)

## Safety
- Distinguish statistical significance from biological significance
- Note small sample sizes, batch effects, and technical artifacts
- Be honest about inconclusive results
"#;

    let user = format!(
        "## Input Data\nInterpret these {result_type} analysis results:\n\n```\n{output_summary}\n```\n\n## Task\nFollow the interpretation protocol and output the structured report."
    );
    (system.to_string(), user)
}

/// Assemble the optimize prompt: pipeline → performance improvements.
///
/// ## Role
/// You are a bioinformatics pipeline optimizer. You suggest concrete
/// parameter changes to improve throughput, reduce memory, or lower costs.
///
/// ## Safety
/// - NEVER suggest changes that reduce reproducibility.
/// - ALWAYS explain the tradeoffs (faster vs. more memory, etc.).
pub fn assemble_optimize_prompt(
    toml_content: &str,
    goal: &str,
    constraints: Option<&OptimizeConstraints>,
) -> (String, String) {
    let system = r#"## Role
You are a bioinformatics pipeline optimizer for oxo-flow. You analyze pipeline TOML configurations and suggest concrete parameter improvements.

## Optimization Protocol
1. Parse the current configuration's resource allocations and tool choices
2. Identify bottlenecks (I/O-bound vs CPU-bound vs memory-bound steps)
3. Suggest specific parameter changes with expected impact
4. Respect ALL provided constraints (max memory, max threads)

## Optimization Heuristics
| Tool | Bottleneck | Strategy |
|------|-----------|----------|
| STAR | CPU/memory | Increase threads up to 16, ensure 32GB+ RAM |
| BWA-MEM | CPU | 4-8 threads optimal, more gives diminishing returns |
| GATK | Memory | 4-8GB per thread, use --max-reads-per-alignment-limit |
| fastp | I/O | 2-4 threads sufficient, I/O limited beyond that |
| featureCounts | I/O | 4-8 threads, memory rarely the bottleneck |
| MACS2 | CPU | 2-4 threads, single-threaded for some steps |

## Safety
- NEVER suggest removing QC or validation steps
- NEVER suggest resource allocations exceeding system limits
- ALWAYS report tradeoffs: time vs memory vs cost
- ALWAYS validate the optimized pipeline would still produce correct results
"#;

    let mut user =
        format!("## Pipeline to Optimize\nGoal: {goal}\n\n```toml\n{toml_content}\n```\n\n");
    if let Some(c) = constraints {
        if let Some(ref mem) = c.max_memory {
            user.push_str(&format!("## Constraint\nMax memory: {mem}\n"));
        }
        if let Some(t) = c.max_threads {
            user.push_str(&format!("Max threads: {t}\n"));
        }
    }

    user.push_str("\n## Task\nSuggest parameter changes following the optimization protocol. Output the modified TOML inside ```toml fences. Explain each change and its expected impact.");
    (system.to_string(), user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explain_prompt_has_protocol() {
        let diag = DiagnosticsResponse {
            failed_nodes: vec![],
            warnings: vec![],
            resource_bottlenecks: vec![],
        };
        let (system, _) = assemble_explain_prompt(&diag, "", "en");
        assert!(
            system.contains("## Analysis Protocol"),
            "should have protocol section"
        );
        assert!(
            system.contains("## Known Error Patterns"),
            "should have error patterns table"
        );
    }

    #[test]
    fn test_interpret_prompt_has_output_structure() {
        let (system, _) = assemble_interpret_prompt("run1", "differential expression", "data");
        assert!(
            system.contains("## Output Structure"),
            "should specify output structure"
        );
        assert!(
            system.contains("### Key Findings"),
            "should have findings template"
        );
        assert!(
            system.contains("### Caveats"),
            "should have caveats template"
        );
    }

    #[test]
    fn test_optimize_prompt_has_heuristics_and_constraints() {
        let (system, _) = assemble_optimize_prompt("[workflow]", "speed", None);
        assert!(
            system.contains("## Optimization Heuristics"),
            "should have heuristics"
        );
        assert!(system.contains("## Safety"), "should have safety section");

        let constraints = OptimizeConstraints {
            max_memory: Some("16GB".into()),
            max_threads: Some(8),
        };
        let (_, user) = assemble_optimize_prompt("[workflow]", "speed", Some(&constraints));
        assert!(user.contains("16GB"), "should include memory constraint");
        assert!(
            user.contains("Max threads: 8"),
            "should include thread constraint"
        );
    }
}
