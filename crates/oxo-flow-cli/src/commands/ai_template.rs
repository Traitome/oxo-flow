//! AI-powered template generation.
//!
//! Uses oxo-flow-ai's provider + knowledge system to generate .oxoflow
//! workflow files from natural language descriptions.

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_ai::agent::events::AgentEvent;
use oxo_flow_ai::agent::orchestrator::Orchestrator;
use oxo_flow_ai::agent::pipeline_gen::{self, OutputValidator, PipelineGenAgent};
use oxo_flow_ai::agent::team::{self, TeamProfile};
use oxo_flow_ai::agent::{AgentContext, ValidationResult};
use oxo_flow_ai::provider::AiProvider;
use oxo_flow_ai::session::AiSession;
use oxo_flow_ai::tools::{Tool, builtin::FetchUrlTool};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Fetch a `--from-url` reference through the agent's SSRF-screened fetcher.
///
/// The builtin `fetch_url` tool validates every hop (scheme/host screen,
/// DNS-resolution check, pinned reconnect) before anything is fetched;
/// routing the CLI's own `--from-url` through it keeps that guard from
/// being bypassable by simply not using `--ai`'s tool loop.
async fn fetch_reference(fetcher: &FetchUrlTool, url: &str) -> Result<String, String> {
    let arguments = serde_json::json!({ "url": url }).to_string();
    fetcher.execute(&arguments).await.map_err(|e| e.to_string())
}

/// Largest prefix of `s` with at most `max_bytes` bytes, cut at a char
/// boundary. Byte-slicing (`&s[..n]`) panics when `n` lands inside a
/// multi-byte UTF-8 sequence — CJK reference text routinely does (issue
/// #297 item 6).
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Where the generated workflow lands.
///
/// `-o` documents two shapes: a file path, and a directory signalled by a
/// trailing `/`. Only the first was implemented — the directory form fell
/// through to a plain write and failed with ENOENT after the paid LLM call
/// had already produced the artifact. The shape is therefore resolved (and
/// its directories created) up front.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OutputTarget {
    /// `-o dir/` — write `<dir>/<derived name>`.
    Directory { dir: PathBuf, file_name: String },
    /// `-o path`, or the default derived name when `-o` is absent.
    File(PathBuf),
}

impl OutputTarget {
    /// The concrete file this target resolves to.
    fn file_path(&self) -> PathBuf {
        match self {
            Self::Directory { dir, file_name } => dir.join(file_name),
            Self::File(path) => path.clone(),
        }
    }

    /// Human-readable destination for progress and error messages.
    fn display(&self) -> String {
        self.file_path().display().to_string()
    }
}

/// Derive the default workflow file name from the user's intent: the first
/// three words, lower-cased, with punctuation stripped.
fn derived_file_name(intent: &str) -> String {
    let name = intent
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join("_")
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "");
    // Punctuation-only intents degrade to a bare underscore — fall back to a
    // fixed stem rather than writing a hidden ".oxoflow" file.
    let stem = name.trim_matches('_');
    if stem.is_empty() {
        "workflow.oxoflow".to_string()
    } else {
        format!("{stem}.oxoflow")
    }
}

/// Resolve the `-o` value into a concrete output target.
fn resolve_output_target(output: Option<&Path>, intent: &str) -> OutputTarget {
    match output {
        Some(path) if path.to_string_lossy().ends_with('/') => OutputTarget::Directory {
            dir: path.to_path_buf(),
            file_name: derived_file_name(intent),
        },
        Some(path) => OutputTarget::File(path.to_path_buf()),
        None => OutputTarget::File(PathBuf::from(derived_file_name(intent))),
    }
}

/// Create the directories the target needs. Runs before the LLM call so a
/// bad destination fails cheaply instead of after a paid generation.
fn prepare_output_target(target: &OutputTarget) -> Result<()> {
    let dir = match target {
        OutputTarget::Directory { dir, .. } => Some(dir.clone()),
        OutputTarget::File(path) => path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf),
    };
    let Some(dir) = dir else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create output directory {}", dir.display()))
}

/// Write the generated workflow, returning the path it landed on.
fn write_output(target: &OutputTarget, toml_content: &str) -> Result<PathBuf> {
    let path = target.file_path();
    std::fs::write(&path, toml_content)?;
    Ok(path)
}

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
    team_profile: Option<oxo_flow_ai::agent::team::TeamProfile>,
    ai_attempts: u32,
) -> Result<()> {
    // Resolve and prepare the destination BEFORE anything is sent to the
    // provider: a bad `-o` must fail cheaply, not after a paid generation.
    let output_target = resolve_output_target(output.as_deref(), intent);
    prepare_output_target(&output_target)?;

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

    // Build external sources. The agent renders them into its user prompt
    // (same bounded previews as before — full content is never shipped).
    let mut external_sources = Vec::new();

    // Fetch URLs
    let fetcher = FetchUrlTool::new();
    for url in from_urls {
        println!("{} Fetching {url}...", "  •".dimmed());
        match fetch_reference(&fetcher, url).await {
            Ok(text) => {
                let preview = if text.len() > 300 {
                    truncate_utf8(&text, 300).to_string()
                } else {
                    text.clone()
                };
                external_sources.push(oxo_flow_ai::agent::ExternalSource::Url {
                    url: url.clone(),
                    content: preview,
                });
                println!("{}   Fetched {} chars", "  ✓".green(), text.len());
            }
            Err(e) => {
                eprintln!("{}   Blocked or failed to fetch: {e}", "  ⚠".yellow());
            }
        }
    }

    // Read local files
    for path in from_files {
        println!("{} Reading {}...", "  •".dimmed(), path.display());
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let preview = if content.len() > 2000 {
                    truncate_utf8(&content, 2000).to_string()
                } else {
                    content.clone()
                };
                external_sources.push(oxo_flow_ai::agent::ExternalSource::File {
                    path: path.clone(),
                    content: preview,
                });
                println!("{}   Read {} chars", "  ✓".green(), content.len());
            }
            Err(e) => {
                eprintln!("{}   Failed to read: {e}", "  ⚠".yellow());
            }
        }
    }

    // bioSkills: inject domain-matched expertise so the generated workflow
    // follows curated domain procedures (tool choice, parameters, caveats).
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

    println!("{}", "  Generating workflow...".bold().cyan());

    // Scientist Team profile. Compact (default) runs the generation agent
    // alone; Full adds the deterministic Curator brief, a bounded task
    // contract, and an independent review pass with one bounded fix.
    let full = matches!(team_profile, Some(TeamProfile::Full));

    let curator_brief = if full {
        match oxo_flow_ai::agent::team::curate(intent) {
            Some(brief) => {
                println!(
                    "{} Curator: embedded-knowledge brief assembled (deterministic)",
                    "  •".dimmed()
                );
                Some(brief)
            }
            None => None,
        }
    } else {
        None
    };

    let contract = if full {
        println!("{} PI: drafting the task contract...", "  •".dimmed());
        draft_task_contract(provider, intent).await
    } else {
        None
    };

    let validator: OutputValidator = Arc::new(|toml: &str| {
        if let Err(e) = validate_basic_structure(toml) {
            return ValidationResult::failed(vec![e.to_string()]);
        }
        match toml::from_str::<oxo_flow_core::config::WorkflowConfig>(toml) {
            Ok(_) => ValidationResult::passed(),
            Err(e) => ValidationResult::failed(vec![format!("engine schema: {e}")]),
        }
    });

    // Build the generation agent. Rebuilt for the bounded review-fix pass
    // with the reviewer's findings attached.
    let build_agent = {
        let validator = validator.clone();
        let curator_brief = curator_brief.clone();
        let contract = contract.clone();
        move |findings: Option<&str>| -> PipelineGenAgent {
            let mut agent = PipelineGenAgent::new(intent)
                .with_validator(validator.clone())
                .with_text_fixer(Arc::new(oxo_flow_core::format::fix_undefined_config_keys));
            if !runtime.skill_context.is_empty() {
                agent = agent.with_system_addition(format!(
                    "## Activated Custom Skills\n{}",
                    runtime.skill_context
                ));
            }
            if !skill_context.is_empty() {
                agent = agent.with_user_addition(format!(
                "## Domain Expertise (bioSkills)\n{skill_context}\n\n\
                 Follow these domain procedures for tool choice, parameters, and caveats where applicable."
            ));
            }
            if let Some(brief) = &curator_brief {
                agent = agent.with_user_addition(brief.clone());
            }
            if let Some(contract) = &contract {
                agent = agent.with_user_addition(format!(
                    "## Task Contract\nThe requester's intent, standardized by the PI. \
                 Follow these decisions unless the data contradicts them.\n\n{contract}"
                ));
            }
            if let Some(findings) = findings {
                agent = agent.with_user_addition(format!(
                    "## Independent Review Findings\nThe previous draft was returned by an \
                 independent reviewer. Address every blocking finding; keep everything \
                 else as-is.\n\n{findings}"
                ));
            }
            agent
        }
    };

    // Terminal progress from the shared agent-event stream. Text is also
    // buffered so a round-cap failure can still deliver the generated
    // artifact — the same degradation contract as the web chat agent.
    let text_buf = Arc::new(std::sync::Mutex::new(String::new()));
    let sink_buf = text_buf.clone();
    let mut sink = move |e: AgentEvent| {
        if let AgentEvent::Text(ref chunk) = e
            && let Ok(mut buf) = sink_buf.lock()
        {
            buf.push_str(chunk);
        }
        match e {
            AgentEvent::Status(s) => println!("{} {}", "  •".dimmed(), s),
            AgentEvent::ToolCall { name, .. } => println!("{} querying {name}", "  •".dimmed()),
            AgentEvent::ToolResult { name, summary } => {
                println!("{} {name}: {summary}", "  ✓".green());
            }
            _ => {}
        }
    };

    let sessions: Arc<std::sync::Mutex<Vec<AiSession>>> = Arc::default();
    let approver: Arc<oxo_flow_ai::agent::ToolApprover> = Arc::new(|def, args| {
        crate::commands::ai_runtime::prompt_tool_approval_blocking(&def.name, args)
    });

    let mut ctx = AgentContext {
        intent: intent.to_string(),
        command: "template".into(),
        workflow_path: None,
        workflow_content: None,
        external_sources,
        max_rounds: runtime.config.max_retries.max(1),
        tool_registry: runtime.tool_registry,
        tool_approver: Some(approver),
        session: AiSession::new(
            "template",
            intent,
            provider.name(),
            &provider.model().unwrap_or_else(|| "default".into()),
        ),
    };

    // pass@k with early exit: every attempt is a fresh draw with its own
    // session; the gates are deterministic, so the first validating draw
    // wins and later attempts are never paid. Only failing runs pay for
    // attempt 2..N.
    let attempts = ai_attempts.max(1);
    let mut outcome = None;
    for attempt in 1..=attempts {
        if attempt > 1 {
            println!(
                "{} Generation attempt {attempt}/{attempts} (fresh draw)...",
                "  •".dimmed()
            );
        }
        let agent = build_agent(None);
        ctx.session = AiSession::new(
            "template",
            intent,
            provider.name(),
            &provider.model().unwrap_or_else(|| "default".into()),
        );

        let attempt_outcome = runtime
            .orchestrator
            .execute_with_sink(&agent, &ctx, Some(&mut sink), None)
            .await;

        // Independent review (Full profile): a fresh instance sees the contract
        // and the artifact — never the generation transcript. A request_changes
        // verdict triggers ONE regeneration, which replaces the artifact only if
        // IT ALSO VALIDATES: review can only improve the outcome, never trade a
        // validated draft for an unvalidated one (the first ablation showed a
        // replace-unconditionally review regressing pass@1 83% → 56%).
        let attempt_outcome = if full {
            match attempt_outcome {
                Ok(first) if first.success && first.content.is_some() => {
                    let artifact = first.content.clone().expect("checked above");
                    match run_review(provider, contract.as_deref(), &artifact).await {
                        Some(team::Verdict::RequestChanges(findings)) => {
                            println!(
                                "{} Review: request_changes — one bounded regeneration",
                                "  ⚠".yellow()
                            );
                            if let Ok(mut s) = sessions.lock() {
                                s.push(first.session.clone());
                            }
                            let fix_agent = build_agent(Some(&findings));
                            ctx.session = AiSession::new(
                                "template",
                                intent,
                                provider.name(),
                                &provider.model().unwrap_or_else(|| "default".into()),
                            );
                            match runtime
                                .orchestrator
                                .execute_with_sink(&fix_agent, &ctx, Some(&mut sink), None)
                                .await
                            {
                                Ok(o) if o.success && o.content.is_some() => {
                                    println!(
                                        "{} Review fix validated — adopting it",
                                        "  ✓".green()
                                    );
                                    Ok(o)
                                }
                                other => {
                                    println!(
                                        "{} Review fix did not validate — keeping the original artifact",
                                        "  ⚠".yellow()
                                    );
                                    if let Ok(mut s) = sessions.lock()
                                        && let Ok(o) = &other
                                    {
                                        s.push(o.session.clone());
                                    }
                                    Ok(first)
                                }
                            }
                        }
                        Some(team::Verdict::Approve) => {
                            println!("{} Review: approved", "  ✓".green());
                            Ok(first)
                        }
                        None => Ok(first),
                    }
                }
                other => other,
            }
        } else {
            attempt_outcome
        };

        let produced = matches!(&attempt_outcome, Ok(o) if o.success && o.content.is_some());
        if produced {
            outcome = Some(attempt_outcome);
            break;
        }
        // A failed attempt's spend must stay visible: archive its session
        // now (the tail archives the final outcome's session itself).
        if attempt < attempts
            && let Ok(mut s) = sessions.lock()
            && let Ok(o) = &attempt_outcome
        {
            s.push(o.session.clone());
        }
        outcome = Some(attempt_outcome);
    }

    let outcome = outcome.expect("attempts >= 1");

    // Archive every session the team produced (contract/review calls are
    // archived inside their helpers; generation + review-fix here).
    let (toml_content, degraded) = match outcome {
        Ok(o) if o.success && o.content.is_some() => {
            if let Ok(mut s) = sessions.lock() {
                s.push(o.session.clone());
            }
            for session in sessions.lock().unwrap().iter() {
                archive_session(session);
            }
            (o.content.unwrap(), false)
        }
        Ok(o) => {
            for session in sessions.lock().unwrap().iter() {
                archive_session(session);
            }
            archive_session(&o.session);
            anyhow::bail!(
                "pipeline generation did not produce a valid workflow: {}",
                o.summary
            );
        }
        Err(e) => {
            // Round-cap or provider failure: the orchestrator archived the
            // failed session (with usage). Deliver the generated TOML from
            // the transcript rather than losing the paid-for artifact.
            for session in sessions.lock().unwrap().iter() {
                archive_session(session);
            }
            let text = text_buf.lock().map(|b| b.clone()).unwrap_or_default();
            let toml = pipeline_gen::extract_toml(&text)
                .ok_or_else(|| anyhow::anyhow!("AI generation failed: {e}"))?;
            (toml, true)
        }
    };

    if degraded {
        println!(
            "{} Generated pipeline reached its correction-round budget — review carefully before running.",
            "  ⚠".yellow().bold()
        );
    } else {
        // The orchestrator validator guarantees the core schema here.
        println!("{} Schema validation passed", "  ✓".green());
    }

    // Write output. A failure here must not swallow the artifact — the
    // generation already happened and was paid for, so the full TOML is
    // echoed to stdout before the command reports the write failure.
    let output_path = match write_output(&output_target, &toml_content) {
        Ok(path) => path,
        Err(e) => {
            eprintln!(
                "{} Could not write {}: {e}",
                "  ⚠".yellow().bold(),
                output_target.display()
            );
            println!();
            println!(
                "{}",
                "── Generated workflow (write failed; full TOML below) ──"
                    .yellow()
                    .bold()
            );
            println!("{toml_content}");
            println!("{}", "── end ──".yellow().bold());
            anyhow::bail!(
                "Workflow could not be written to {} — the full TOML was printed above",
                output_target.display()
            );
        }
    };
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

/// Stage 0 (Full profile): draft the task contract with a bounded,
/// low-temperature call. Returns `None` when the contract call fails or
/// produces an unparsable contract — generation then proceeds from the raw
/// intent, exactly as the compact profile would.
async fn draft_task_contract(
    provider: &oxo_flow_ai::provider::AiProvider,
    intent: &str,
) -> Option<String> {
    let agent = team::ContractAgent::new(intent, None);
    let ctx = AgentContext {
        intent: intent.to_string(),
        command: "template-contract".into(),
        workflow_path: None,
        workflow_content: None,
        external_sources: Vec::new(),
        max_rounds: 2,
        tool_registry: oxo_flow_ai::tools::ToolRegistry::new(),
        tool_approver: None,
        session: AiSession::new(
            "template-contract",
            intent,
            provider.name(),
            &provider.model().unwrap_or_else(|| "default".into()),
        ),
    };
    // Low temperature: the contract states decisions, not creative prose.
    let orchestrator = Orchestrator::new(provider.clone().with_temperature(Some(0.2)), 2);
    match orchestrator.execute(&agent, &ctx).await {
        Ok(o) if o.success && o.content.is_some() => {
            archive_session(&o.session);
            o.content
        }
        Ok(o) => {
            archive_session(&o.session);
            tracing::warn!("task contract unavailable: {}", o.summary);
            None
        }
        Err(e) => {
            tracing::warn!("task contract failed: {e}");
            None
        }
    }
}

/// Stage 5 (Full profile): independent adversarial review of the final
/// artifact against the contract. Fresh instance, no generation transcript.
/// Returns `None` when the review itself fails — a broken reviewer never
/// blocks delivery.
async fn run_review(
    provider: &oxo_flow_ai::provider::AiProvider,
    contract: Option<&str>,
    artifact: &str,
) -> Option<team::Verdict> {
    let agent = team::ReviewAgent::new(contract.map(String::from), artifact);
    let ctx = AgentContext {
        intent: "review".into(),
        command: "template-review".into(),
        workflow_path: None,
        workflow_content: None,
        external_sources: Vec::new(),
        max_rounds: 2,
        tool_registry: oxo_flow_ai::tools::ToolRegistry::new(),
        tool_approver: None,
        session: AiSession::new(
            "template-review",
            "review",
            provider.name(),
            &provider.model().unwrap_or_else(|| "default".into()),
        ),
    };
    let orchestrator = Orchestrator::new(provider.clone().with_temperature(Some(0.2)), 2);
    match orchestrator.execute(&agent, &ctx).await {
        Ok(o) if o.success && o.content.is_some() => {
            archive_session(&o.session);
            let review = o.content.unwrap();
            let verdict = team::parse_verdict(&review);
            if verdict.is_none() {
                tracing::warn!("review verdict unparsable — discarding the review");
            }
            verdict
        }
        Ok(o) => {
            archive_session(&o.session);
            tracing::warn!("review unavailable: {}", o.summary);
            None
        }
        Err(e) => {
            tracing::warn!("review failed: {e}");
            None
        }
    }
}

/// Persist the orchestrator-owned session and print the token summary.
/// The orchestrator mutates the session it was given (usage, tool calls,
/// modifications) and returns it in the outcome — this is the single
/// archive point for the template command.
fn archive_session(session: &AiSession) {
    if let Err(e) = oxo_flow_ai::session::save_session(session) {
        tracing::warn!("Failed to save AI session: {e}");
    }
    let input = session.total_usage.prompt_tokens;
    let output = session.total_usage.completion_tokens;
    if input > 0 || output > 0 {
        println!(
            "{} AI session: {} | tokens: {input} in + {output} out",
            "  ✓".green(),
            session.id
        );
    }
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
    fn fixer_declares_missing_config_keys() {
        // The measured failure class: the draft references {config.prefix}
        // without declaring it — a paid model round used to be the fix.
        let draft = "[workflow]\nname = \"x\"\n\n[config]\nsample = \"S1\"\n\n[[rules]]\nname = \"r\"\noutput = [\"{{config.prefix}}_qc.txt\"]\nshell = \"echo hi\"";
        let (fixed, notes) = oxo_flow_core::format::fix_undefined_config_keys(draft.to_string());
        assert_eq!(notes.len(), 1, "notes: {notes:?}");
        assert!(notes[0].contains("prefix"));
        assert!(fixed.contains("prefix = \"prefix\""), "fixed: {fixed}");
        // The fix must satisfy the engine check it targets.
        let config: oxo_flow_core::config::WorkflowConfig = toml::from_str(&fixed).unwrap();
        for rule in &config.rules {
            assert!(oxo_flow_core::format::undefined_config_refs(rule, &config).is_empty());
        }
    }

    #[test]
    fn fixer_creates_config_section_when_absent_and_leaves_clean_toml_alone() {
        let no_config = "[workflow]\nname = \"x\"\n\n[[rules]]\nname = \"r\"\noutput = [\"{{config.root}}/a.txt\"]\nshell = \"echo hi\"";
        let (fixed, notes) =
            oxo_flow_core::format::fix_undefined_config_keys(no_config.to_string());
        assert_eq!(notes.len(), 1);
        assert!(fixed.contains("[config]"));

        // Clean TOML passes through byte-identical.
        let clean = "[workflow]\nname = \"x\"\n\n[config]\nsample = \"S1\"\n\n[[rules]]\nname = \"r\"\noutput = [\"a.txt\"]\nshell = \"echo hi\"";
        let (fixed, notes) = oxo_flow_core::format::fix_undefined_config_keys(clean.to_string());
        assert_eq!(fixed, clean);
        assert!(notes.is_empty());
    }

    #[test]
    fn truncate_utf8_cuts_at_char_boundary() {
        // Issue #297 item 6: `&text[..300]` panicked when byte 300 split a
        // multi-byte UTF-8 char — CJK reference text hit this on every fetch.
        let cjk = "样本".repeat(150); // 3-byte chars; byte 298 lands inside char 100
        assert_eq!(truncate_utf8(&cjk, 298).len(), 297);
        assert!(truncate_utf8(&cjk, 298).ends_with("样"));
        // A 4-byte emoji spanning byte 300 is dropped whole, no panic.
        let emoji = format!("{}🧬y", "x".repeat(299));
        assert_eq!(truncate_utf8(&emoji, 300), "x".repeat(299));
        // Short input is returned as-is; ASCII cut at a boundary is exact.
        assert_eq!(truncate_utf8("short", 300), "short");
        assert_eq!(truncate_utf8(&"a".repeat(400), 300).len(), 300);
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

    #[test]
    fn derived_file_name_uses_first_three_words() {
        assert_eq!(
            derived_file_name("RNA seq analysis of tumor"),
            "rna_seq_analysis.oxoflow"
        );
    }

    #[test]
    fn derived_file_name_falls_back_when_intent_has_no_words() {
        assert_eq!(derived_file_name("/// ???"), "workflow.oxoflow");
        assert_eq!(derived_file_name(""), "workflow.oxoflow");
    }

    #[test]
    fn trailing_slash_selects_directory_target_with_derived_name() {
        // `-o out/` is documented as "output directory"; previously the raw
        // value (slash included) was handed to fs::write and failed with
        // ENOENT only after the paid LLM call.
        let target = resolve_output_target(Some(Path::new("out/")), "RNA seq analysis");
        assert_eq!(
            target,
            OutputTarget::Directory {
                dir: PathBuf::from("out/"),
                file_name: "rna_seq_analysis.oxoflow".to_string(),
            }
        );
        assert!(target.display().ends_with("out/rna_seq_analysis.oxoflow"));
    }

    #[test]
    fn explicit_path_and_absent_output_resolve_to_file_targets() {
        assert_eq!(
            resolve_output_target(Some(Path::new("custom.oxoflow")), "RNA seq"),
            OutputTarget::File(PathBuf::from("custom.oxoflow"))
        );
        assert_eq!(
            resolve_output_target(None, "RNA seq analysis"),
            OutputTarget::File(PathBuf::from("rna_seq_analysis.oxoflow"))
        );
    }

    #[test]
    fn prepare_output_target_creates_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let target = OutputTarget::Directory {
            dir: tmp.path().join("nested/deep"),
            file_name: "w.oxoflow".to_string(),
        };
        prepare_output_target(&target).unwrap();
        assert!(tmp.path().join("nested/deep").is_dir());
    }

    #[test]
    fn prepare_output_target_creates_missing_parent_for_file() {
        let tmp = tempfile::tempdir().unwrap();
        let target = OutputTarget::File(tmp.path().join("new/dir/w.oxoflow"));
        prepare_output_target(&target).unwrap();
        assert!(tmp.path().join("new/dir").is_dir());
    }

    #[test]
    fn write_output_lands_inside_directory_target() {
        let tmp = tempfile::tempdir().unwrap();
        let target = OutputTarget::Directory {
            dir: tmp.path().to_path_buf(),
            file_name: "gen.oxoflow".to_string(),
        };
        let written = write_output(&target, "[workflow]").unwrap();
        assert_eq!(written, tmp.path().join("gen.oxoflow"));
        assert_eq!(std::fs::read_to_string(written).unwrap(), "[workflow]");
    }

    #[test]
    fn write_output_reports_failure_instead_of_losing_the_artifact() {
        // The write can still fail (permissions, target removed mid-run);
        // the caller's fallback is echoing the TOML, so this path must be
        // an Err rather than a silent loss.
        let tmp = tempfile::tempdir().unwrap();
        let target = OutputTarget::Directory {
            dir: tmp.path().to_path_buf(),
            file_name: "gen.oxoflow".to_string(),
        };
        prepare_output_target(&target).unwrap();
        std::fs::remove_dir(tmp.path()).unwrap();
        assert!(write_output(&target, "[workflow]").is_err());
    }

    #[tokio::test]
    async fn from_url_reference_is_ssrf_screened() {
        // `--from-url` must go through the same guard as the fetch_url tool:
        // loopback targets are rejected before any connection is attempted.
        let fetcher = FetchUrlTool::new();
        for url in [
            "http://127.0.0.1:9/workflow.oxoflow",
            "http://localhost:9/x",
        ] {
            let err = fetch_reference(&fetcher, url)
                .await
                .expect_err("loopback URL must be rejected");
            assert!(
                err.contains("blocked"),
                "expected an SSRF rejection for {url}, got: {err}"
            );
        }
    }
}
