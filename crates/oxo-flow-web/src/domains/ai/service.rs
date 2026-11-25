//! AI service orchestration layer.
//!
//! Issue #342: generation runs the SHARED `PipelineGenAgent` through the
//! orchestrator (knowledge tools + engine validation feedback loop); this
//! module owns the surface concerns only — request dedup cache, provider
//! fallback chain, deterministic template-keyword fallback, explanation
//! assembly.
//!
//! **Zero write access guarantee**: this module has NO import of DB write
//! functions, filesystem write, or process spawn. All side effects are
//! constrained to read-only AI calls through read-only tools.

use std::collections::HashMap;
use std::sync::Mutex;

use super::copilot;
use super::types::*;
use crate::ai_provider::{AiProvider, AiProviderRegistry};
use crate::domains::workflow::service as workflow_svc;
use oxo_flow_ai::agent::orchestrator::Orchestrator;

/// One cached translate result. The attribution travels with the artifact:
/// a cache hit served without it would contradict the response contract
/// (provider/model/fallback fields are never blank).
#[derive(Clone)]
struct CachedTranslation {
    pipeline_id: String,
    toml_content: String,
    provider: String,
    model: Option<String>,
    fallback_used: bool,
    attempts: Vec<ProviderAttempt>,
}

/// In-memory request cache for deduplication.
/// Key: hash of (intent, data_summary). Value: the generated result.
static REQUEST_CACHE: std::sync::LazyLock<Mutex<HashMap<String, CachedTranslation>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Maximum entries in the request cache before eviction.
const MAX_CACHE_ENTRIES: usize = 128;

/// Compute a cache key from the acting user, intent, and optional data
/// summary. The user id is part of the key: a cached translation embeds the
/// requester's own data (paths, sample names), so sharing it across users
/// would leak one tenant's content to another.
fn cache_key(user_id: &str, intent: &str, data_summary: Option<&str>) -> String {
    format!("{user_id}|{intent}|{}", data_summary.unwrap_or("no-data"))
}

/// Append `provider` unless the same configuration (name, model, endpoint,
/// credential) is already listed — a user provider that resolves to the
/// runtime's own configuration must not be retried after failing, while two
/// rows differing only by API key are distinct candidates.
fn push_unique(candidates: &mut Vec<AiProvider>, provider: AiProvider) {
    if candidates.iter().any(|c| c.same_configuration(&provider)) {
        return;
    }
    candidates.push(provider);
}

/// Cross-provider fallback policy for translate (`OXO_FLOW_AI_FALLBACK`).
///
/// `Auto` (default) preserves the historical chain; `Disabled` limits it to
/// the first usable candidate (the acting user's provider, else the
/// configured runtime) so an explicit provider pin cannot spend a different
/// credential behind the operator's back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FallbackMode {
    Auto,
    Disabled,
}

impl FallbackMode {
    /// Parse the documented values. Empty means unset (`auto`); anything
    /// else is an error so the caller can warn and keep `auto` — a typo must
    /// never silently change the operator's spend boundary.
    fn parse(raw: &str) -> Result<Self, ()> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "disabled" => Ok(Self::Disabled),
            _ => Err(()),
        }
    }
}

/// Read `OXO_FLOW_AI_FALLBACK` once per translate request. An unrecognized
/// value warns once and falls back to `auto`.
fn fallback_mode_from_env() -> FallbackMode {
    match std::env::var("OXO_FLOW_AI_FALLBACK") {
        Err(_) => FallbackMode::Auto,
        Ok(raw) => match FallbackMode::parse(&raw) {
            Ok(mode) => mode,
            Err(()) => {
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    tracing::warn!(
                        env = "OXO_FLOW_AI_FALLBACK",
                        value = %raw.trim(),
                        "unrecognized OXO_FLOW_AI_FALLBACK (expected auto or disabled) — \
                         keeping the default auto fallback chain"
                    );
                });
                FallbackMode::Auto
            }
        },
    }
}

/// Apply the fallback policy to an ordered candidate chain.
fn limit_candidates(candidates: Vec<AiProvider>, mode: FallbackMode) -> Vec<AiProvider> {
    match mode {
        FallbackMode::Auto => candidates,
        FallbackMode::Disabled => candidates.into_iter().take(1).collect(),
    }
}

/// Provider candidates for translate, in fallback order: the acting user's
/// own provider first (their key, their cost — the same isolation rule the
/// chat route follows), then the configured runtime provider, then
/// env-discovered Claude/OpenAI/Ollama. (The fallback chain is a
/// provider-selection concern and stays here; generation itself is the
/// shared agent + orchestrator.)
fn provider_candidates(user_provider: &AiProvider) -> Vec<AiProvider> {
    provider_candidates_for(
        user_provider,
        oxo_flow_ai::provider::provider_pinned_disabled(),
        fallback_mode_from_env(),
    )
}

/// [`provider_candidates`] against an explicit pin state and fallback mode —
/// the testable core. An explicit `OXO_FLOW_AI_PROVIDER=disabled` pins the
/// process off: the env-discovered tiers below would otherwise resurrect a
/// real (paid) provider from ordinary shell credentials even though every
/// runtime config tier is disabled.
fn provider_candidates_for(
    user_provider: &AiProvider,
    pinned_disabled: bool,
    fallback_mode: FallbackMode,
) -> Vec<AiProvider> {
    if pinned_disabled {
        return Vec::new();
    }
    let registry = AiProviderRegistry::global();
    let config = registry.get_config();
    let mut candidates = Vec::new();
    if user_provider.is_usable() {
        push_unique(&mut candidates, user_provider.clone());
    }
    if config.is_configured {
        push_unique(&mut candidates, registry.get_provider());
    }
    if config.provider != "claude"
        && let Ok(claude) = AiProviderRegistry::create_claude_from_env()
    {
        push_unique(&mut candidates, claude);
    }
    if config.provider != "openai"
        && let Ok(openai) = AiProviderRegistry::create_openai_from_env()
    {
        push_unique(&mut candidates, openai);
    }
    // Ollama is the only credential-free backend, so its constructor always
    // succeeds — admitting it unconditionally would put a phantom
    // localhost:11434 candidate in every chain and turn "nothing
    // configured" into a doomed connection attempt. Enter the chain only
    // when the operator opted in (OLLAMA_HOST), matching the CLI, where a
    // fully unconfigured environment resolves to Noop.
    if config.provider != "ollama"
        && std::env::var("OLLAMA_HOST").is_ok()
        && let Ok(ollama) = AiProviderRegistry::create_ollama_from_env()
    {
        push_unique(&mut candidates, ollama);
    }
    limit_candidates(candidates, fallback_mode)
}

/// Whether a translate request could possibly run: the acting user's
/// provider, the runtime, or an env-discovered backend must be usable.
/// Handlers gate their structured AI_NOT_CONFIGURED response on this
/// instead of an ad-hoc provider check, so an env-key-only deployment
/// (no runtime config, but a key in the environment) still translates.
pub fn generation_available(user_provider: &AiProvider) -> bool {
    !provider_candidates(user_provider).is_empty()
}

/// Why a translate request produced no pipeline. `Unavailable` is the
/// structured AI_NOT_CONFIGURED case (503); `TimedOut` is the documented
/// `504 AI_TIMEOUT` case (the last attempted provider hit its per-request
/// timeout); `Failed` is a generation or validation failure (400).
#[derive(Debug)]
pub enum TranslateError {
    Unavailable,
    /// The last-attempted provider hit its per-request timeout; the fields
    /// name that provider so the 504 answer is attributable.
    TimedOut {
        provider: Option<String>,
        model: Option<String>,
    },
    Failed(String),
}

/// A pipeline produced by one provider, with its attribution record.
struct GeneratedPipeline {
    content: String,
    provider: AiProvider,
    attempts: Vec<ProviderAttempt>,
}

/// Why every candidate failed. Carries the full attempt list (each with its
/// reason) so the final structured error can name the provider/model and the
/// failure cause — including "exceeded max rounds" — instead of emitting a
/// bare provider-agnostic string.
#[derive(Debug)]
struct GenerationFailure {
    attempts: Vec<ProviderAttempt>,
    last_failure: String,
    timed_out: bool,
}

impl GenerationFailure {
    /// Last attempted provider and model, when anything was attempted.
    fn last_provider(&self) -> (Option<String>, Option<String>) {
        match self.attempts.last() {
            Some(attempt) => (Some(attempt.provider.clone()), attempt.model.clone()),
            None => (None, None),
        }
    }

    /// The `400 AI_TRANSLATE_ERROR` message. Names the provider/model that
    /// failed last and keeps the orchestrator's cause (`exceeded max rounds
    /// (N)`, auth/transport error, …) visible.
    fn message(&self, template: Option<&str>) -> String {
        let target = match self.attempts.last() {
            Some(attempt) => match &attempt.model {
                Some(model) => format!("{} ({model})", attempt.provider),
                None => attempt.provider.clone(),
            },
            None => "no provider was attempted".to_string(),
        };
        let scope = if self.attempts.len() > 1 {
            format!(
                " after {} provider attempts, last {target}",
                self.attempts.len()
            )
        } else {
            format!(" on {target}")
        };
        let mut message = format!("AI generation failed{scope}: {}", self.last_failure);
        if let Some(name) = template {
            message.push_str(&format!(". Try the '{name}' template"));
        }
        message
    }
}

fn provider_attempt(
    candidate: &AiProvider,
    success: bool,
    reason: Option<String>,
) -> ProviderAttempt {
    ProviderAttempt {
        provider: candidate.name().to_string(),
        model: candidate.model(),
        success,
        reason,
    }
}

/// Run the generation agent over `candidates` in order, recording an
/// attribution entry for every attempt and logging a WARN when the chain
/// falls back from one provider to the next.
///
/// The pre-existing spend guard is kept: a provider that responded and
/// billed tokens but produced unusable output stops the chain (a second
/// full generation on one request would double the cost); only attempts
/// that provably produced nothing (transport/auth errors, zero completions)
/// fall through.
async fn generate_with_candidates(
    agent: &dyn oxo_flow_ai::agent::Agent,
    intent: &str,
    candidates: &[AiProvider],
) -> Result<GeneratedPipeline, GenerationFailure> {
    let mut attempts: Vec<ProviderAttempt> = Vec::new();
    let mut last_failure = "no provider was attempted".to_string();
    let mut last_failure_timed_out = false;

    for (idx, candidate) in candidates.iter().enumerate() {
        if idx > 0 {
            tracing::warn!(
                fallback_from = attempts.last().map(|a| a.provider.as_str()).unwrap_or("?"),
                fallback_to = candidate.name(),
                model = candidate.model().unwrap_or_else(|| "default".into()),
                reason = %last_failure,
                "translate falling back to another AI provider after a failed attempt"
            );
        }
        let ctx = oxo_flow_ai::agent::AgentContext {
            intent: intent.to_string(),
            command: "translate".into(),
            workflow_path: None,
            workflow_content: None,
            external_sources: vec![],
            // Shared shipped budget: knowledge lookups spend 2-3 rounds
            // before text and the validation-feedback fix needs 1-2 more.
            max_rounds: oxo_flow_ai::config::default_max_retries(),
            tool_registry: oxo_flow_ai::tools::builtin::knowledge_tool_registry(),
            tool_approver: None,
            session: oxo_flow_ai::session::AiSession::new(
                "web-translate",
                "translate",
                candidate.name(),
                &candidate.model().unwrap_or_else(|| "default".into()),
            ),
        };
        let orchestrator = Orchestrator::new(
            candidate.clone(),
            oxo_flow_ai::config::default_max_retries(),
        );
        match orchestrator.execute(agent, &ctx).await {
            Ok(outcome) if outcome.success && outcome.content.is_some() => {
                // Failures archive themselves inside the orchestrator; the
                // successful generation is returned here, so the web layer
                // persists it (review wave 2 MEDIUM-2).
                crate::domains::ai::session_archive::archive_agent_session(&outcome.session);
                attempts.push(provider_attempt(candidate, true, None));
                return Ok(GeneratedPipeline {
                    content: outcome.content.unwrap(),
                    provider: candidate.clone(),
                    attempts,
                });
            }
            Ok(outcome) => {
                crate::domains::ai::session_archive::archive_agent_session(&outcome.session);
                last_failure = outcome.summary.clone();
                last_failure_timed_out = false;
                attempts.push(provider_attempt(
                    candidate,
                    false,
                    Some(outcome.summary.clone()),
                ));
                tracing::warn!(
                    "translate provider {} produced no valid pipeline: {}",
                    candidate.name(),
                    outcome.summary
                );
                if outcome.session.total_usage.completion_tokens > 0 {
                    break;
                }
            }
            Err(e) => {
                last_failure_timed_out = e.is_timeout();
                last_failure = e.to_string();
                attempts.push(provider_attempt(candidate, false, Some(e.to_string())));
                tracing::warn!("translate provider {} failed: {e}", candidate.name());
            }
        }
    }

    Err(GenerationFailure {
        attempts,
        last_failure,
        timed_out: last_failure_timed_out,
    })
}

/// Archive a translate generation aborted by the JSON route's total budget.
///
/// The orchestrator archives its own failures, but a route-budget expiry
/// drops its future mid-flight, so the 504 path used to leave no record
/// (review wave 3 LOW-3). Writes the synthetic `Failed` session with the
/// same labels a real translate run uses.
pub fn archive_route_timeout(provider: &AiProvider, budget: std::time::Duration) {
    let session = oxo_flow_ai::session::AiSession::new(
        "web-translate",
        "translate",
        provider.name(),
        &provider.model().unwrap_or_else(|| "default".into()),
    )
    .fail(&crate::domains::ai::timeout::timeout_reason(budget));
    crate::domains::ai::session_archive::archive_agent_session(&session);
}

/// Translate natural language intent into a validated .oxoflow pipeline.
///
/// Pipeline (one harness across all generation surfaces):
/// 1. Check request cache (dedup) — serves without any AI involvement
/// 2. Match templates (deterministic, zero AI cost)
/// 3. Run the SHARED `PipelineGenAgent` through the orchestrator —
///    knowledge tools + engine-accurate prompt + validation-feedback
///    correction loop — over the provider fallback chain
/// 4. Prepare pipeline (expand wildcards)
/// 5. Parse for explanation and return
pub async fn translate_intent(
    provider: &AiProvider,
    user_id: &str,
    intent: &str,
    data_summary: Option<&str>,
    templates: &[String],
) -> Result<TranslateResponse, TranslateError> {
    // Step 0: Dedup — check cache
    let key = cache_key(user_id, intent, data_summary);
    {
        let cache = REQUEST_CACHE
            .lock()
            .map_err(|_| TranslateError::Failed("translate cache is unavailable".into()))?;
        if let Some(cached) = cache.get(&key) {
            // Re-parse to build explanation
            if let Ok(parsed) = workflow_svc::parse_pipeline(&cached.toml_content, None) {
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
                    pipeline_id: cached.pipeline_id.clone(),
                    toml_content: cached.toml_content.clone(),
                    explanation: TranslateExplanation { steps },
                    alternatives: vec![],
                    confidence: 0.95,
                    provider: cached.provider.clone(),
                    model: cached.model.clone(),
                    fallback_used: cached.fallback_used,
                    attempts: cached.attempts.clone(),
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

    // Only reached on a cache miss: without a usable provider anywhere the
    // request fails as structured Unavailable rather than looping over an
    // empty candidate list. A cached pipeline keeps serving regardless.
    if !generation_available(provider) {
        return Err(TranslateError::Unavailable);
    }

    // Step 2: Shared agent + orchestrator over the provider fallback chain.
    // Read-only knowledge tools only, no approver: non-read-only calls are
    // refused by construction (the module's zero-write guarantee).
    let mut agent = oxo_flow_ai::agent::pipeline_gen::PipelineGenAgent::new(intent)
        .with_validator(workflow_svc::pipeline_output_validator())
        .with_text_fixer(workflow_svc::pipeline_output_fixer());
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

    let candidates = provider_candidates(provider);
    let generated = match generate_with_candidates(&agent, intent, &candidates).await {
        Ok(generated) => generated,
        Err(failure) => {
            if failure.timed_out {
                // The provider client's timeout is a 504; carry the last
                // attempted provider so the answer stays attributable
                // (review wave 2 HIGH-1).
                let (provider, model) = failure.last_provider();
                return Err(TranslateError::TimedOut { provider, model });
            }
            return Err(TranslateError::Failed(
                failure.message(template_match.map(String::as_str)),
            ));
        }
    };

    let GeneratedPipeline {
        content: toml_content,
        provider: provider_used,
        attempts,
    } = generated;
    let fallback_used = attempts.len() > 1;
    let provider_used_name = provider_used.name().to_string();
    let provider_used_model = provider_used.model();

    // Step 4: Prepare (expand wildcards, resolve environments)
    let _prepared = workflow_svc::prepare_pipeline(&toml_content, true, true)
        .map_err(TranslateError::Failed)?;

    // Step 5: Parse for explanation
    let parsed =
        workflow_svc::parse_pipeline(&toml_content, None).map_err(TranslateError::Failed)?;

    // Step 6: Build explanation. The actual backend is named on every step
    // (provider + model when known) — the old label said "via claude
    // (fallback)" while the configured provider was OpenAI, and the top-level
    // attribution fields now make the same fact machine-readable.
    let provider_label = match &provider_used_model {
        Some(model) => format!("{provider_used_name} ({model})"),
        None => provider_used_name.clone(),
    };
    let steps: Vec<TranslationStep> = parsed
        .rules
        .iter()
        .map(|r| TranslationStep {
            rule: r.name.clone(),
            purpose: format!("{} step (via {provider_label})", r.name),
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
            cache.insert(
                key,
                CachedTranslation {
                    pipeline_id: parsed.pipeline_id.clone(),
                    toml_content: toml_content.clone(),
                    provider: provider_used_name.clone(),
                    model: provider_used_model.clone(),
                    fallback_used,
                    attempts: attempts.clone(),
                },
            );
        }
    }

    Ok(TranslateResponse {
        pipeline_id: parsed.pipeline_id,
        toml_content,
        explanation: TranslateExplanation { steps },
        alternatives: vec![],
        confidence: if fallback_used { 0.75 } else { 0.88 },
        provider: provider_used_name,
        model: provider_used_model,
        fallback_used,
        attempts,
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

    let toml = oxo_flow_ai::agent::pipeline_gen::extract_toml(&raw_response)
        .unwrap_or(raw_response.clone());

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

    // ── TOML extraction tests (pin the canonical extractor's contract on
    // the web side: fenced content is taken, prose is never mistaken for a
    // pipeline) ──

    #[test]
    fn test_extract_toml_from_code_fence() {
        let response = "Here is the pipeline:\n```toml\n[workflow]\nname = \"test\"\n```\nDone.";
        let result = oxo_flow_ai::agent::pipeline_gen::extract_toml(response);
        assert_eq!(result, Some("[workflow]\nname = \"test\"".to_string()));
    }

    #[test]
    fn test_extract_toml_raw_workflow() {
        // Raw fallback requires the structural floor (workflow + rules +
        // shell), so the fixture carries a shell field.
        let response = "[workflow]\nname = \"test\"\n[[rules]]\nname = \"step1\"\nshell = \"echo\"";
        let result = oxo_flow_ai::agent::pipeline_gen::extract_toml(response);
        assert!(result.is_some());
        assert!(result.unwrap().contains("[workflow]"));
    }

    #[test]
    fn test_extract_toml_no_toml() {
        let response = "I cannot generate a pipeline right now.";
        let result = oxo_flow_ai::agent::pipeline_gen::extract_toml(response);
        assert_eq!(result, None);
    }

    // ── Provider fallback chain tests ──

    #[test]
    fn test_push_unique_dedups_same_configuration_keeps_different_key() {
        // Arrange — a user row and the runtime row can point at the same
        // provider/model/endpoint; only the credential differs. The identical
        // pair must dedup (no pointless retry), the distinct key must stay.
        let user = AiProvider::OpenAi(oxo_flow_ai::provider::OpenAiBackend::new(
            "user-key".into(),
            Some("m".into()),
            Some("https://gw.example.com/v1".into()),
        ));
        let runtime = AiProvider::OpenAi(oxo_flow_ai::provider::OpenAiBackend::new(
            "server-key".into(),
            Some("m".into()),
            Some("https://gw.example.com/v1".into()),
        ));
        let mut candidates = vec![user.clone()];

        // Act + Assert — same endpoint and model, different key: kept.
        push_unique(&mut candidates, runtime);
        assert_eq!(candidates.len(), 2);

        // The exact same configuration again: dropped.
        push_unique(&mut candidates, user);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn pinned_disabled_yields_no_provider_candidates() {
        // Regression: `OXO_FLOW_AI_PROVIDER=disabled` plus a stray shell
        // credential used to make /api/ai/translate call the real provider —
        // the env-discovered fallback tier ignored the pin. The pin must
        // empty the chain even when the acting user has a usable provider.
        let user = AiProvider::OpenAi(oxo_flow_ai::provider::OpenAiBackend::new(
            "user-key".into(),
            Some("m".into()),
            Some("http://127.0.0.1:9/v1".into()),
        ));
        assert!(
            provider_candidates_for(&user, true, FallbackMode::Auto).is_empty(),
            "the pinned-disabled path must yield no candidates"
        );

        // Control: unpinned resolution still honors the user's provider
        // first (ambient env credentials may append fallbacks, so only the
        // first entry is asserted).
        let unpinned = provider_candidates_for(&user, false, FallbackMode::Auto);
        assert!(
            unpinned
                .first()
                .is_some_and(|candidate| candidate.same_configuration(&user)),
            "the acting user's provider must lead the unpinned chain"
        );
    }

    #[test]
    fn fallback_mode_parses_only_the_documented_values() {
        assert_eq!(FallbackMode::parse("auto"), Ok(FallbackMode::Auto));
        assert_eq!(FallbackMode::parse(" AUTO "), Ok(FallbackMode::Auto));
        assert_eq!(FallbackMode::parse(""), Ok(FallbackMode::Auto));
        assert_eq!(FallbackMode::parse("disabled"), Ok(FallbackMode::Disabled));
        assert_eq!(FallbackMode::parse("DISABLED"), Ok(FallbackMode::Disabled));
        // A typo is an error so the caller warns and keeps `auto` — it can
        // never silently disable an operator's fallback boundary.
        assert_eq!(FallbackMode::parse("off"), Err(()));
        assert_eq!(FallbackMode::parse("none"), Err(()));
    }

    #[test]
    fn disabled_fallback_limits_the_chain_to_the_first_candidate() {
        let providers: Vec<AiProvider> = ["a", "b", "c"]
            .iter()
            .map(|model| {
                AiProvider::OpenAi(oxo_flow_ai::provider::OpenAiBackend::new(
                    "key".into(),
                    Some((*model).into()),
                    Some(format!("http://127.0.0.1:9/{model}")),
                ))
            })
            .collect();

        let limited = limit_candidates(providers.clone(), FallbackMode::Disabled);
        assert_eq!(limited.len(), 1, "disabled keeps only the first provider");
        assert_eq!(limited[0].model().as_deref(), Some("a"));

        assert_eq!(
            limit_candidates(providers, FallbackMode::Auto).len(),
            3,
            "auto preserves the full fallback chain"
        );
        assert!(limit_candidates(vec![], FallbackMode::Disabled).is_empty());
    }

    #[test]
    fn failure_message_names_provider_model_and_the_max_round_cause() {
        // The live incident returned a provider-agnostic "AI generation
        // failed" after a max-round exhaustion; operators could not tell
        // which provider/model burned the tokens.
        let failure = GenerationFailure {
            attempts: vec![
                ProviderAttempt {
                    provider: "openai".into(),
                    model: Some("qwen3.8-flash-next-nothink".into()),
                    success: false,
                    reason: Some("Agent exceeded max rounds (10)".into()),
                },
                ProviderAttempt {
                    provider: "claude".into(),
                    model: Some("deepseek-flash[1m]".into()),
                    success: false,
                    reason: Some("HTTP 401".into()),
                },
            ],
            last_failure: "Agent exceeded max rounds (10)".into(),
            timed_out: false,
        };

        let message = failure.message(Some("rna-seq"));
        assert!(
            message.contains("claude (deepseek-flash[1m])"),
            "the last attempted provider/model must be named: {message}"
        );
        assert!(
            message.contains("exceeded max rounds (10)"),
            "the max-round cause must survive: {message}"
        );
        assert!(
            message.contains("2 provider attempts"),
            "multi-provider failures must say so: {message}"
        );
        assert!(
            message.contains("'rna-seq' template"),
            "the template hint must survive: {message}"
        );
        assert_eq!(
            failure.last_provider(),
            (Some("claude".into()), Some("deepseek-flash[1m]".into()))
        );
    }

    /// Thread-local WARN capture for the fallback test. `#[tokio::test]`
    /// uses a current-thread runtime, so every event from the generation
    /// future lands in this buffer.
    #[derive(Clone, Default)]
    struct CapturedWarnLog(std::sync::Arc<Mutex<Vec<u8>>>);

    impl CapturedWarnLog {
        fn text(&self) -> String {
            let buffer = self.0.lock().unwrap_or_else(|e| e.into_inner());
            String::from_utf8_lossy(&buffer).to_string()
        }
    }

    impl std::io::Write for CapturedWarnLog {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedWarnLog {
        type Writer = CapturedWarnLog;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn generation_attribution_records_each_attempt_and_fallback_reason() {
        // Deterministic in-process providers: the first has exhausted its
        // script (an Err that proves no tokens were billed), the second
        // produces valid TOML. This exercises the fallback loop through the
        // real orchestrator with no network.
        use oxo_flow_ai::scripted::{ScriptedBackend, ScriptedTurn};

        const VALID_TOML: &str = "[workflow]\nname = \"scripted-pipeline\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"fastqc\"\ninput = [\"{sample}.fastq.gz\"]\noutput = [\"qc/{sample}_fastqc.html\"]\nshell = \"fastqc {input} -o qc/\"\n";

        let first = AiProvider::Scripted(ScriptedBackend::new(vec![]));
        let second = AiProvider::Scripted(ScriptedBackend::new(vec![ScriptedTurn::content(
            VALID_TOML,
        )]));
        let agent =
            oxo_flow_ai::agent::pipeline_gen::PipelineGenAgent::new("QC pipeline for RNA-seq");

        // Capture the operator-facing fallback signal: the WARN must name
        // the failed attempt's reason, not just the new provider.
        let captured = CapturedWarnLog::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(captured.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let generated = generate_with_candidates(&agent, "QC pipeline", &[first, second])
            .await
            .expect("the second provider must produce the pipeline");

        let warnings = captured.text();
        assert!(
            warnings.contains("falling back"),
            "a fallback must be logged at WARN: {warnings:?}"
        );
        assert!(
            warnings.contains("script exhausted"),
            "the fallback warning must carry the failure reason: {warnings:?}"
        );
        assert_eq!(generated.provider.name(), "scripted");
        assert_eq!(generated.attempts.len(), 2, "both attempts recorded");
        assert!(!generated.attempts[0].success);
        assert!(
            generated.attempts[0]
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("script exhausted"),
            "the first attempt's failure reason must be recorded: {:?}",
            generated.attempts[0].reason
        );
        assert!(generated.attempts[1].success);
        assert!(generated.attempts[1].reason.is_none());
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
