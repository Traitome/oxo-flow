//! AI service orchestration layer.
//!
//! Coordinates the AI translation pipeline:
//! deterministic API calls -> prompt assembly -> AI provider call -> response parsing.
//!
//! **Zero write access guarantee**: this module has NO import of DB write
//! functions, filesystem write, or process spawn. All side effects are
//! constrained to read-only AI chat calls.
//!
//! ## Fallback chain
//! Claude → OpenAI → Ollama → template keyword match
//!
//! ## Correction loop
//! After AI generates TOML, validate it. If invalid, feed errors back to
//! the AI for correction (max 3 rounds).

use std::collections::HashMap;
use std::sync::Mutex;

use super::copilot;
use super::types::*;
use crate::ai_provider::{AiProvider, AiProviderRegistry};
use crate::domains::workflow::service as workflow_svc;
use oxo_flow_ai::agent::orchestrator::Orchestrator;

/// In-memory request cache for deduplication.
/// Key: hash of (intent, data_summary). Value: (pipeline_id, toml_content).
static REQUEST_CACHE: std::sync::LazyLock<Mutex<HashMap<String, (String, String)>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Maximum entries in the request cache before eviction.
const MAX_CACHE_ENTRIES: usize = 128;

/// Extract TOML content from an AI response string.
/// Looks for ```toml code fences first, then raw `[workflow]` content.
fn extract_toml(response: &str) -> Option<String> {
    if let Some(start) = response.find("```toml") {
        let start = start + 7;
        if let Some(end) = response[start..].find("```") {
            return Some(response[start..start + end].trim().to_string());
        }
    }
    if response.contains("[workflow]") {
        return Some(response.to_string());
    }
    None
}

/// Compute a cache key from the acting user, intent, and optional data
/// summary. The user id is part of the key: a cached translation embeds the
/// requester's own data (paths, sample names), so sharing it across users
/// would leak one tenant's content to another.
fn cache_key(user_id: &str, intent: &str, data_summary: Option<&str>) -> String {
    format!("{user_id}|{intent}|{}", data_summary.unwrap_or("no-data"))
}

/// Provider candidates for translate, in fallback order: the configured
/// runtime provider first, then env-discovered Claude/OpenAI/Ollama.
/// (Issue #342: the fallback chain is a provider-selection concern and
/// stays here; generation itself is the shared agent + orchestrator.)
fn provider_candidates() -> Vec<AiProvider> {
    let registry = AiProviderRegistry::global();
    let config = registry.get_config();
    let mut candidates = Vec::new();
    if config.is_configured {
        candidates.push(registry.get_provider());
    }
    if config.provider != "claude"
        && let Ok(claude) = AiProviderRegistry::create_claude_from_env()
    {
        candidates.push(claude);
    }
    if config.provider != "openai"
        && let Ok(openai) = AiProviderRegistry::create_openai_from_env()
    {
        candidates.push(openai);
    }
    if config.provider != "ollama"
        && let Ok(ollama) = AiProviderRegistry::create_ollama_from_env()
    {
        candidates.push(ollama);
    }
    candidates
}

/// Translate natural language intent into a validated .oxoflow pipeline.
///
/// Pipeline (issue #342 — one harness across all generation surfaces):
/// 1. Check request cache (dedup)
/// 2. Match templates (deterministic, zero AI cost)
/// 3. Run the SHARED `PipelineGenAgent` through the orchestrator —
///    knowledge tools + engine-accurate prompt + validation-feedback
///    correction loop — over the provider fallback chain
/// 4. Prepare pipeline (expand wildcards)
/// 5. Parse for explanation and return
pub async fn translate_intent(
    _provider: &AiProvider,
    user_id: &str,
    intent: &str,
    data_summary: Option<&str>,
    templates: &[String],
) -> Result<TranslateResponse, String> {
    // Step 0: Dedup — check cache
    let key = cache_key(user_id, intent, data_summary);
    {
        let cache = REQUEST_CACHE.lock().map_err(|_| "Cache lock poisoned")?;
        if let Some((pipeline_id, toml_content)) = cache.get(&key) {
            // Re-parse to build explanation
            if let Ok(parsed) = workflow_svc::parse_pipeline(toml_content, None) {
                let steps: Vec<TranslationStep> = parsed
                    .rules
                    .iter()
                    .map(|r| TranslationStep {
                        rule: r.name.clone(),
                        purpose: format!("{} step", r.name),
                        tool: r.environment.clone().unwrap_or_default(),
                        key_params: format!("threads: {}", r.threads.unwrap_or(1)),
                        why_chosen: "AI-generated based on intent (cached)".into(),
                    })
                    .collect();
                return Ok(TranslateResponse {
                    pipeline_id: pipeline_id.clone(),
                    toml_content: toml_content.clone(),
                    explanation: TranslateExplanation { steps },
                    alternatives: vec![],
                    confidence: 0.95,
                });
            }
        }
    }

    // Step 1: Template keyword matching (deterministic, zero AI cost)
    let template_match = templates.iter().find(|t| {
        intent
            .to_lowercase()
            .contains(&t.to_lowercase().replace('-', " "))
    });

    // Step 2: Shared agent + orchestrator over the provider fallback chain.
    // Read-only knowledge tools only, no approver: non-read-only calls are
    // refused by construction (the module's zero-write guarantee).
    let validator: oxo_flow_ai::agent::pipeline_gen::OutputValidator = std::sync::Arc::new(
        |toml: &str| match workflow_svc::validate_pipeline(toml, None) {
            Ok(v) if v.valid => oxo_flow_ai::agent::ValidationResult::passed(),
            Ok(v) => oxo_flow_ai::agent::ValidationResult {
                passed: false,
                errors: v.errors.iter().map(|e| e.message.clone()).collect(),
                warnings: vec![],
                summary: format!("{} validation error(s)", v.errors.len()),
            },
            Err(e) => oxo_flow_ai::agent::ValidationResult {
                passed: false,
                errors: vec![e],
                warnings: vec![],
                summary: "validation failed".into(),
            },
        },
    );

    let mut agent =
        oxo_flow_ai::agent::pipeline_gen::PipelineGenAgent::new(intent).with_validator(validator);
    if let Some(summary) = data_summary {
        agent = agent.with_user_addition(format!("## Data Context\n{summary}"));
    }
    if !templates.is_empty() {
        agent = agent.with_user_addition(format!(
            "## Available Templates\n{}",
            templates
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n- ")
        ));
    }

    let candidates = provider_candidates();
    let mut generated: Option<(String, String)> = None;
    for (idx, candidate) in candidates.iter().enumerate() {
        let label = if idx == 0 {
            candidate.name().to_string()
        } else {
            format!("{} (fallback)", candidate.name())
        };
        let ctx = oxo_flow_ai::agent::AgentContext {
            intent: intent.to_string(),
            command: "translate".into(),
            workflow_path: None,
            workflow_content: None,
            external_sources: vec![],
            max_rounds: 4,
            tool_registry: oxo_flow_ai::tools::builtin::knowledge_tool_registry(),
            tool_approver: None,
            session: oxo_flow_ai::session::AiSession::new(
                "web-translate",
                "translate",
                candidate.name(),
                &candidate.model().unwrap_or_else(|| "default".into()),
            ),
        };
        let orchestrator = Orchestrator::new(candidate.clone(), 4);
        match orchestrator.execute(&agent, &ctx).await {
            Ok(outcome) if outcome.success && outcome.content.is_some() => {
                generated = Some((outcome.content.unwrap(), label));
                break;
            }
            Ok(outcome) => {
                tracing::warn!(
                    "translate provider {} produced no valid pipeline: {}",
                    candidate.name(),
                    outcome.summary
                );
            }
            Err(e) => {
                tracing::warn!("translate provider {} failed: {e}", candidate.name());
            }
        }
    }

    let Some((toml_content, provider_used)) = generated else {
        return match template_match {
            Some(name) => Err(format!(
                "AI unavailable (tried all providers). Try the '{name}' template."
            )),
            None => Err("AI generation failed after trying all providers".to_string()),
        };
    };

    // Step 4: Prepare (expand wildcards, resolve environments)
    let _prepared = workflow_svc::prepare_pipeline(&toml_content, true, true)?;

    // Step 5: Parse for explanation
    let parsed = workflow_svc::parse_pipeline(&toml_content, None)?;

    // Step 6: Build explanation
    let steps: Vec<TranslationStep> = parsed
        .rules
        .iter()
        .map(|r| TranslationStep {
            rule: r.name.clone(),
            purpose: format!("{} step (via {provider_used})", r.name),
            tool: r.environment.clone().unwrap_or_default(),
            key_params: format!("threads: {}", r.threads.unwrap_or(1)),
            why_chosen: "AI-generated based on intent".into(),
        })
        .collect();

    // Step 7: Cache the result
    {
        if let Ok(mut cache) = REQUEST_CACHE.lock() {
            if cache.len() >= MAX_CACHE_ENTRIES {
                // Simple eviction: clear half the cache
                let keys: Vec<String> = cache.keys().take(MAX_CACHE_ENTRIES / 2).cloned().collect();
                for k in keys {
                    cache.remove(&k);
                }
            }
            cache.insert(key, (parsed.pipeline_id.clone(), toml_content.clone()));
        }
    }

    Ok(TranslateResponse {
        pipeline_id: parsed.pipeline_id,
        toml_content,
        explanation: TranslateExplanation { steps },
        alternatives: vec![],
        confidence: if provider_used.contains("fallback") {
            0.75
        } else {
            0.88
        },
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
///
/// Parses the AI response into narrative, highlights, caveats, and suggested
/// next steps by extracting structured sections from the response text.
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

    // Parse structured sections from the AI response
    let narrative = extract_section(&response, "Narrative").unwrap_or_else(|| response.clone());
    let highlights = parse_bullet_list(&response, "Highlights");
    let caveats = parse_bullet_list(&response, "Caveats");
    let suggested_next = parse_bullet_list(&response, "Next Steps");

    Ok(InterpretResponse {
        narrative,
        highlights: highlights
            .into_iter()
            .map(|h| Finding {
                finding: h,
                significance: String::new(),
                supporting_evidence: String::new(),
            })
            .collect(),
        caveats: if caveats.is_empty() {
            vec!["AI-generated interpretation — review before use".into()]
        } else {
            caveats
        },
        suggested_next,
    })
}

/// Extract a named section from an AI response.
/// Looks for headers like "## Highlights" or "**Caveats**".
fn extract_section(text: &str, section: &str) -> Option<String> {
    let markers = [
        format!("## {section}"),
        format!("### {section}"),
        format!("**{section}**"),
        format!("{section}:"),
    ];
    for marker in &markers {
        if let Some(pos) = text.find(marker.as_str()) {
            let start = pos + marker.len();
            let remainder = &text[start..];
            // Find the next section header or end of text
            if let Some(next_section) = remainder
                .find("\n## ")
                .or_else(|| remainder.find("\n### "))
                .or_else(|| remainder.find("\n**"))
            {
                return Some(remainder[..next_section].trim().to_string());
            }
            return Some(remainder.trim().to_string());
        }
    }
    None
}

/// Parse a bulleted list from a named section in the AI response.
fn parse_bullet_list(text: &str, section: &str) -> Vec<String> {
    if let Some(section_text) = extract_section(text, section) {
        section_text
            .lines()
            .filter(|l| l.trim().starts_with('-') || l.trim().starts_with('*'))
            .map(|l| l.trim().trim_start_matches(['-', '*']).trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![]
    }
}

/// Optimize pipeline parameters via AI suggestions.
///
/// Sends the current TOML to the AI with optimization goal and constraints,
/// validates the returned TOML, and parses the explanation of changes.
pub async fn optimize_pipeline(
    provider: &AiProvider,
    toml_content: &str,
    goal: &str,
    constraints: Option<OptimizeConstraints>,
) -> Result<OptimizeResponse, String> {
    let (system, user) =
        copilot::assemble_optimize_prompt(toml_content, goal, constraints.as_ref());

    let raw_response = provider
        .chat(&system, &user)
        .await
        .map_err(|e| format!("AI optimize failed: {e}"))?;

    let toml = extract_toml(&raw_response).unwrap_or(raw_response.clone());

    // Validate the optimized TOML
    let _validation = workflow_svc::validate_pipeline(&toml, None)?;

    // Parse changes from the AI response
    let changes = parse_optimization_changes(&raw_response);

    // Parse estimates or use defaults
    let time_saved = extract_section(&raw_response, "Time Saved")
        .unwrap_or_else(|| "Unknown — benchmark to measure".to_string());
    let memory_reduction = extract_section(&raw_response, "Memory Reduction")
        .unwrap_or_else(|| "Unknown — benchmark to measure".to_string());

    Ok(OptimizeResponse {
        optimized_toml: toml,
        changes,
        estimated: OptimizationEstimate {
            time_saved,
            memory_reduction,
        },
    })
}

/// Parse optimization changes from AI response text.
/// Expects each change as a line like "- threads: 4 → 8 (better parallelism)".
fn parse_optimization_changes(text: &str) -> Vec<OptimizationChange> {
    if let Some(section) = extract_section(text, "Changes") {
        parse_bullet_list(&format!("Changes:\n{section}",), "Changes")
            .iter()
            .filter_map(|line| {
                // Parse "param: old → new (reason)" format
                if let Some(arrow_pos) = line.find("→").or_else(|| line.find("->")) {
                    let before_part = &line[..arrow_pos].trim();
                    let after_part =
                        &line[arrow_pos + if line.contains("→") { 3 } else { 2 }..].trim();
                    // Split after_part into value and rationale
                    let (after, rationale) = if let Some(paren) = after_part.find('(') {
                        let val = after_part[..paren].trim().to_string();
                        let reason = after_part[paren..]
                            .trim_matches(|c| c == '(' || c == ')')
                            .to_string();
                        (val, reason)
                    } else {
                        (after_part.to_string(), String::new())
                    };
                    Some(OptimizationChange {
                        rule: String::new(),
                        before: before_part.to_string(),
                        after,
                        rationale,
                        expected_impact: String::new(),
                    })
                } else {
                    None
                }
            })
            .collect()
    } else {
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Security boundary: ZERO WRITE ACCESS
// ---------------------------------------------------------------------------
// This module does NOT import any DB-write, FS-write, or process-spawn APIs.
// Verification: tests/ai_security.rs checks the source of this file.

#[cfg(test)]
mod tests {
    use super::*;

    // ── TOML extraction tests ──

    #[test]
    fn test_extract_toml_from_code_fence() {
        let response = "Here is the pipeline:\n```toml\n[workflow]\nname = \"test\"\n```\nDone.";
        let result = extract_toml(response);
        assert_eq!(result, Some("[workflow]\nname = \"test\"".to_string()));
    }

    #[test]
    fn test_extract_toml_raw_workflow() {
        let response = "[workflow]\nname = \"test\"\n[[rules]]\nname = \"step1\"";
        let result = extract_toml(response);
        assert!(result.is_some());
        assert!(result.unwrap().contains("[workflow]"));
    }

    #[test]
    fn test_extract_toml_no_toml() {
        let response = "I cannot generate a pipeline right now.";
        let result = extract_toml(response);
        assert_eq!(result, None);
    }

    // ── Section extraction tests ──

    #[test]
    fn test_extract_section_with_header() {
        let text = "## Narrative\nThis is the narrative.\n\n## Caveats\n- Item 1\n- Item 2";
        let narrative = extract_section(text, "Narrative");
        assert_eq!(narrative, Some("This is the narrative.".to_string()));
    }

    #[test]
    fn test_extract_section_not_found() {
        let text = "Just some random text.";
        let result = extract_section(text, "Narrative");
        assert_eq!(result, None);
    }

    // ── Bullet list parsing tests ──

    #[test]
    fn test_parse_bullet_list() {
        let text = "## Caveats\n- First caveat\n- Second caveat\n- Third caveat";
        let items = parse_bullet_list(text, "Caveats");
        assert_eq!(items, vec!["First caveat", "Second caveat", "Third caveat"]);
    }

    #[test]
    fn test_parse_bullet_list_empty() {
        let text = "## Caveats\n\nNo items here.";
        let items = parse_bullet_list(text, "Caveats");
        assert!(items.is_empty());
    }

    // ── Optimization change parsing tests ──

    #[test]
    fn test_parse_optimization_changes() {
        let text =
            "## Changes\n- threads: 4 → 8 (Increase for better parallelism)\n- memory: 16GB → 32GB";
        let changes = parse_optimization_changes(text);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].before, "threads: 4");
        assert_eq!(changes[0].after, "8");
        assert_eq!(changes[0].rationale, "Increase for better parallelism");
    }

    #[test]
    fn test_parse_optimization_changes_empty() {
        let text = "No changes needed.";
        let changes = parse_optimization_changes(text);
        assert!(changes.is_empty());
    }

    // ── Cache key tests ──

    #[test]
    fn test_cache_key_identity() {
        let k1 = cache_key("alice", "RNA-seq analysis", Some("fastq files"));
        let k2 = cache_key("alice", "RNA-seq analysis", Some("fastq files"));
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_cache_key_different() {
        let k1 = cache_key("alice", "RNA-seq", None);
        let k2 = cache_key("alice", "Variant calling", None);
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_key_isolates_users_with_identical_intents() {
        // Arrange — two tenants asking the same thing must never share a
        // cache entry: the cached TOML embeds the first user's data.
        // Act
        let k1 = cache_key("alice", "RNA-seq analysis", Some("fastq files"));
        let k2 = cache_key("bob", "RNA-seq analysis", Some("fastq files"));

        // Assert
        assert_ne!(k1, k2);
    }
}
