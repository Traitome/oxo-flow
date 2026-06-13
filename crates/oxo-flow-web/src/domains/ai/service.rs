//! AI service orchestration layer.
//!
//! Coordinates the AI translation pipeline:
//! deterministic API calls -> prompt assembly -> AI provider call -> response parsing.
//!
//! **Zero write access guarantee**: this module has NO import of DB write
//! functions, filesystem write, or process spawn. All side effects are
//! constrained to read-only AI chat calls.

use super::copilot;
use super::types::*;
use crate::ai_provider::AiProvider;
use crate::domains::workflow::service as workflow_svc;

/// Translate natural language intent into a validated .oxoflow pipeline.
///
/// Step 1: Match templates (deterministic, zero AI cost)
/// Step 2: Assemble prompt with copilot
/// Step 3: AI generates TOML
/// Step 4: Validate generated TOML (deterministic gate)
pub async fn translate_intent(
    provider: &AiProvider,
    intent: &str,
    data_summary: Option<&str>,
    templates: &[String],
) -> Result<TranslateResponse, String> {
    // Try template-based fallback first (deterministic, no AI cost)
    let template_match = templates.iter().find(|t| {
        intent
            .to_lowercase()
            .contains(&t.to_lowercase().replace('-', " "))
    });

    let (system, user) = copilot::assemble_translate_prompt(intent, data_summary, templates);

    let toml_content = match provider.chat(&system, &user).await {
        Ok(response) => {
            // Extract TOML from markdown code fences
            if let Some(start) = response.find("```toml") {
                let start = start + 7;
                if let Some(end) = response[start..].find("```") {
                    response[start..start + end].trim().to_string()
                } else {
                    response.clone()
                }
            } else if response.contains("[workflow]") {
                response.clone()
            } else {
                return Err("AI response did not contain valid .oxoflow TOML".into());
            }
        }
        Err(e) => {
            // Fallback: use keyword matching to suggest a template
            if let Some(name) = template_match {
                return Err(format!("AI unavailable. Try the '{name}' template: {e}"));
            }
            return Err(format!("AI generation failed: {e}"));
        }
    };

    // Validate the generated TOML — deterministic gate
    let validation = workflow_svc::validate_pipeline(&toml_content)?;
    if !validation.valid {
        let error_msgs: Vec<String> = validation
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.code, e.message))
            .collect();
        return Err(format!(
            "Generated pipeline failed validation: {}",
            error_msgs.join("; ")
        ));
    }

    // Parse to get structure for explanation
    let parsed = workflow_svc::parse_pipeline(&toml_content, None)?;

    // Build explanation steps from the generated rules
    let steps: Vec<TranslationStep> = parsed
        .rules
        .iter()
        .map(|r| TranslationStep {
            rule: r.name.clone(),
            purpose: format!("{} step", r.name),
            tool: r.environment.clone().unwrap_or_default(),
            key_params: format!("threads: {}", r.threads.unwrap_or(1)),
            why_chosen: "AI-generated based on intent".into(),
        })
        .collect();

    Ok(TranslateResponse {
        pipeline_id: parsed.pipeline_id,
        toml_content,
        explanation: TranslateExplanation { steps },
        alternatives: vec![],
        confidence: 0.85,
    })
}

/// Explain a failed run using diagnostics + AI.
pub async fn explain_failure(
    provider: &AiProvider,
    diagnostics: &crate::domains::execution::types::DiagnosticsResponse,
    log_output: &str,
    language: &str,
) -> Result<ExplainResponse, String> {
    let (system, user) = copilot::assemble_explain_prompt(diagnostics, log_output, language);

    let response = provider
        .chat(&system, &user)
        .await
        .map_err(|e| format!("AI explain failed: {e}"))?;

    let root_cause = diagnostics.failed_nodes.first().map(|n| RootCause {
        rule: n.rule.clone(),
        error_type: n.error_pattern.clone().unwrap_or_default(),
        evidence: n.relevant_log_lines.first().cloned().unwrap_or_default(),
        confidence: 0.9,
    });

    let fix_suggestion = diagnostics.failed_nodes.first().and_then(|n| {
        n.suggestions.first().map(|s| FixSuggestion {
            action: s.clone(),
            automated: false,
            estimated_impact: "Unknown — apply and re-run to measure".into(),
        })
    });

    Ok(ExplainResponse {
        summary: response,
        root_cause,
        fix_suggestion,
    })
}

/// Interpret results with AI-generated narrative.
pub async fn interpret_results(
    provider: &AiProvider,
    run_id: &str,
    result_type: &str,
    output_summary: &str,
) -> Result<InterpretResponse, String> {
    let (system, user) = copilot::assemble_interpret_prompt(run_id, result_type, output_summary);

    let response = provider
        .chat(&system, &user)
        .await
        .map_err(|e| format!("AI interpret failed: {e}"))?;

    // Parse response — structured extraction deferred to production refinement
    Ok(InterpretResponse {
        narrative: response,
        highlights: vec![],
        caveats: vec!["AI-generated interpretation — review before use".into()],
        suggested_next: vec![],
    })
}

/// Optimize pipeline parameters via AI suggestions.
pub async fn optimize_pipeline(
    provider: &AiProvider,
    toml_content: &str,
    goal: &str,
    constraints: Option<OptimizeConstraints>,
) -> Result<OptimizeResponse, String> {
    let (system, user) =
        copilot::assemble_optimize_prompt(toml_content, goal, constraints.as_ref());

    let optimized_toml = provider
        .chat(&system, &user)
        .await
        .map_err(|e| format!("AI optimize failed: {e}"))?;

    let toml = if let Some(start) = optimized_toml.find("```toml") {
        let start = start + 7;
        if let Some(end) = optimized_toml[start..].find("```") {
            optimized_toml[start..start + end].trim().to_string()
        } else {
            optimized_toml.clone()
        }
    } else {
        optimized_toml
    };

    // Validate the optimized TOML
    let _validation = workflow_svc::validate_pipeline(&toml)?;

    Ok(OptimizeResponse {
        optimized_toml: toml,
        changes: vec![],
        estimated: OptimizationEstimate {
            time_saved: "Unknown — benchmark to measure".into(),
            memory_reduction: "Unknown — benchmark to measure".into(),
        },
    })
}

// ---------------------------------------------------------------------------
// Security boundary: ZERO WRITE ACCESS
// ---------------------------------------------------------------------------
// This module does NOT import any DB-write, FS-write, or process-spawn APIs.
// Verification: tests/ai_security.rs checks the source of this file.
