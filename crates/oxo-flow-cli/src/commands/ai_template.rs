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
///
/// Reuses the engine-wide staged-write discipline
/// ([`oxo_flow_core::atomic_file::write_file_atomic`]): the bytes land in a
/// same-directory temp file, are fsynced, renamed over the destination, and
/// the parent directory is fsynced. A crash or write failure leaves the
/// previous workflow intact — never a truncated file (RB-14 / AI-10).
fn write_output(target: &OutputTarget, toml_content: &str) -> Result<PathBuf> {
    let path = target.file_path();
    oxo_flow_core::atomic_file::write_file_atomic(&path, toml_content)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

/// What to do about a destination that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverwriteDecision {
    /// The destination is free (or the caller passed `--force`).
    Proceed,
    /// An interactive terminal may confirm the overwrite.
    Prompt,
    /// Refuse without touching the destination.
    Refuse(&'static str),
}

/// The overwrite policy for `template --ai` (RB-14 / AI-10).
///
/// A generated workflow that already exists may be a reviewed, hand-edited
/// artifact: it is never clobbered silently. `--force` is an explicit
/// consent for scripts; an interactive terminal gets a y/N confirmation;
/// a non-interactive session without `--force` refuses (the same safe
/// default as `auto_fix = "ask"` and tool-call approval).
fn overwrite_decision(exists: bool, force: bool, interactive: bool) -> OverwriteDecision {
    if !exists || force {
        return OverwriteDecision::Proceed;
    }
    if interactive {
        OverwriteDecision::Prompt
    } else {
        OverwriteDecision::Refuse(
            "the destination already exists and this session is not interactive — \
             pass --force to overwrite it, or choose another path",
        )
    }
}

/// Enforce [`overwrite_decision`] for the resolved destination BEFORE any
/// paid generation happens.
///
/// Returns whether the destination existed at guard time; the pre-write
/// re-check ([`ensure_destination_did_not_appear`]) uses it to detect a file
/// that appeared while the model was running (TOCTOU).
fn ensure_destination_writable(target: &OutputTarget, force: bool) -> Result<bool> {
    use std::io::IsTerminal as _;

    let path = target.file_path();
    let existed = path.exists();
    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    match overwrite_decision(existed, force, interactive) {
        OverwriteDecision::Proceed => Ok(existed),
        OverwriteDecision::Refuse(reason) => {
            anyhow::bail!("{} already exists — {reason}", path.display())
        }
        OverwriteDecision::Prompt => {
            eprintln!(
                "  {} {} already exists. Overwrite it? [y/N] ",
                "⚠".yellow(),
                path.display()
            );
            let mut input = String::new();
            let confirmed = std::io::stdin().read_line(&mut input).is_ok_and(|_| {
                let trimmed = input.trim();
                trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
            });
            if confirmed {
                Ok(existed)
            } else {
                anyhow::bail!("{} already exists and was left untouched", path.display())
            }
        }
    }
}

/// Close the TOCTOU window between the pre-generation overwrite guard and
/// the atomic replace: if the destination was ABSENT when generation started
/// and exists now, something else created it while the model was running. Do
/// not clobber it — refuse (the atomic rename would otherwise silently
/// replace a file the user never consented to lose). `--force` was explicit
/// consent for the path, so it keeps its meaning.
fn ensure_destination_did_not_appear(
    target: &OutputTarget,
    force: bool,
    existed_before_generation: bool,
) -> Result<()> {
    if !force && !existed_before_generation && target.file_path().exists() {
        anyhow::bail!(
            "{} appeared while the workflow was being generated — refusing to \
             overwrite it; pass --force to replace it",
            target.file_path().display()
        );
    }
    Ok(())
}

/// Print (and log) the structured degraded-mode reason on a surface that
/// continues without AI. Never silent: an `[ai] enabled = true` workflow
/// whose provider cannot resolve says exactly why (RB-14 / AI-09).
fn emit_degraded_reason(reason: &oxo_flow_ai::provider::AiDegradedReason) {
    eprintln!("  {} {}", "⚠".yellow(), reason);
    tracing::warn!(
        code = reason.code,
        message = %reason.message,
        suggestion = %reason.suggestion,
        "AI degraded to no provider"
    );
}

/// Resolve the AI provider from the documented config chain, returning a
/// structured degraded-mode reason if enabled AI cannot resolve one.
pub fn resolve_ai_provider() -> Result<AiProvider> {
    let project_dir = std::env::current_dir().ok();
    let scope = crate::commands::ai_runtime::resolve_ai_scope(None, project_dir.as_deref(), None);
    match scope.degraded_reason() {
        Some(reason) => Err(anyhow::Error::new(reason)),
        None => Ok(scope.provider),
    }
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

/// Try to resolve an AI provider for an optional workflow-scoped analysis.
///
/// Returns `None` when AI was not requested. When AI WAS requested
/// (workflow `[ai] enabled = true` or an explicit flag) but the resolved
/// chain yields no usable provider, the structured degraded-mode reason is
/// printed and logged before returning `None` — the command continues
/// without AI, but never silently (RB-14 / AI-09).
pub fn try_resolve_ai(workflow_path: Option<&Path>, cli_flag: bool) -> Option<AiProvider> {
    if !should_use_ai(workflow_path, cli_flag) {
        return None;
    }
    // `None` project dir: resolve_ai_scope falls back to the workflow's
    // parent directory for `.oxo-flow/ai.toml`, matching the recovery
    // path's `resolve_auto_fix_mode`.
    let scope = crate::commands::ai_runtime::resolve_ai_scope(workflow_path, None, None);
    match scope.degraded_reason() {
        Some(reason) => {
            emit_degraded_reason(&reason);
            None
        }
        None => Some(scope.provider),
    }
}

/// Generate a workflow from natural language using AI.
#[allow(clippy::too_many_arguments)]
pub async fn generate_workflow(
    intent: &str,
    from_urls: &[String],
    from_files: &[PathBuf],
    output: Option<PathBuf>,
    force: bool,
    ai_max_retries: Option<u32>,
    team_profile: Option<oxo_flow_ai::agent::team::TeamProfile>,
    ai_attempts: u32,
    contract_feedback: bool,
) -> Result<()> {
    // Resolve and prepare the destination BEFORE anything is sent to the
    // provider: a bad `-o` must fail cheaply, not after a paid generation.
    let output_target = resolve_output_target(output.as_deref(), intent);
    prepare_output_target(&output_target)?;
    // Overwrite policy also runs before the paid call: an existing
    // destination is confirmed (`--force` / interactive y/N) or refused
    // while nothing has been spent and the old file is untouched.
    let destination_existed = ensure_destination_writable(&output_target, force)?;

    // L1-L3: Initialize AI runtime with scope config + tools
    let project_dir = std::env::current_dir().ok();
    let runtime =
        crate::commands::ai_runtime::AiRuntime::new(None, project_dir.as_deref(), ai_max_retries)
            .await?;
    let provider = &runtime.provider;
    // CLI flag wins over the resolved config chain — `[ai] team_profile` in
    // `.oxo-flow/ai.toml` reaches `runtime.config` via `AiConfig::resolve_chain`.
    // (The previous fallback read the web crate's global registry, which the
    // CLI never initializes, so the documented project setting silently
    // no-op'd.)
    let team_profile = team_profile.or(runtime.config.team_profile);

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
                // #544: align the URL bound with the file bound — 300
                // bytes contributed ~2 sentences of grounding for a fetch
                // the user explicitly paid for.
                let preview = if text.len() > 2000 {
                    truncate_utf8(&text, 2000).to_string()
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

    let validator: OutputValidator = Arc::new(move |toml: &str| {
        if let Err(e) = validate_basic_structure(toml) {
            return ValidationResult::failed(vec![e.to_string()]);
        }
        match toml::from_str::<oxo_flow_core::config::WorkflowConfig>(toml) {
            Ok(_) => {
                if contract_feedback {
                    let contract_errors = contract_feedback_errors(toml);
                    if !contract_errors.is_empty() {
                        return ValidationResult::failed(contract_errors);
                    }
                }
                ValidationResult::passed()
            }
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
            // Rounds concatenate: keep each round's text on its own line so
            // the line-anchored fence scan cannot glue one round's trailing
            // prose to the next round's opening ```toml fence.
            if !buf.is_empty() && !buf.ends_with('\n') {
                buf.push('\n');
            }
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
            archive_sessions(&sessions);
            (o.content.unwrap(), false)
        }
        Ok(o) => {
            archive_sessions(&sessions);
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
            archive_sessions(&sessions);
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
    //
    // Re-check the overwrite decision immediately before the atomic
    // replace: a file that appeared during generation was never consented
    // to and must not be clobbered (TOCTOU).
    let output_path =
        match ensure_destination_did_not_appear(&output_target, force, destination_existed)
            .and_then(|()| write_output(&output_target, &toml_content))
        {
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

/// Archive every accumulated team session. `sessions.lock().unwrap()` is
/// poison-tolerant by construction here: a panicking agent thread must not
/// take the archive path down with it (the locked vec is still readable),
/// so a broken lock degrades to skipping those sessions instead of panicking.
fn archive_sessions(sessions: &std::sync::Mutex<Vec<AiSession>>) {
    if let Ok(queued) = sessions.lock() {
        for session in queued.iter() {
            archive_session(session);
        }
    }
}

/// Basic structural validation before passing to the core engine — the
/// same floor the pipeline_gen agent enforces, so the two can never drift
/// (the local copy used to reject valid script/transform-only pipelines).
fn validate_basic_structure(toml: &str) -> Result<()> {
    let errors = oxo_flow_ai::agent::pipeline_gen::basic_structure_errors(toml);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(errors.join("; ")))
    }
}

/// Contract-level feedback for the Arm C generation loop (issue #1315).
///
/// The matched three-arm ablation (plan:109-111) needs a real engine toggle
/// between Arm B (syntax/structural feedback only) and Arm C (BioTypes
/// `TYPE-*` + `scientific_preflight` `SCI-*` + `deep_check` `D00*`). This is
/// that Arm C path: the same three checks the run-time admission gate uses,
/// reusing their existing entry points (reuse, never retype), rendered into
/// the orchestrator's correction channel.
///
/// The findings are returned as `ValidationResult::failed` ERRORS, not
/// warnings: the orchestrator only feeds `validation.errors` back to the
/// model (`agent/orchestrator.rs`), so a warning-only finding would never
/// change a generation round and the arm would not differ from Arm B. The
/// caller gates this behind an explicit opt-in flag, so normal
/// `template --ai` behavior is unchanged.
fn contract_feedback_errors(toml: &str) -> Vec<String> {
    let config: oxo_flow_core::config::WorkflowConfig = match toml::from_str(toml) {
        Ok(config) => config,
        // The structural + engine-schema floors already reject a config that
        // cannot parse; a parse failure here is not a contract finding.
        Err(_) => return Vec::new(),
    };

    let mut findings: Vec<String> = Vec::new();

    // BioTypes (TYPE-*): the DAG edge type verification the run gate applies.
    // Build the DAG with the same placeholder expansion, then surface every
    // fatal type diagnostic. A DAG-build failure is already an engine-schema
    // class, not a contract finding, so it is not reported here.
    let placeholder_values = crate::commands::run::config_placeholder_values(&config.config);
    if let Ok(dag) =
        oxo_flow_core::dag::WorkflowDag::from_rules_with_config(&config.rules, &placeholder_values)
    {
        for d in oxo_flow_core::types::check_workflow_types(&config, &dag) {
            findings.push(d.to_string());
        }
    }

    // Scientific preflight (SCI-*): cohort/parameter sanity warnings.
    for w in oxo_flow_core::scientific_preflight::analyze_scientific_constraints(&config) {
        findings.push(format!(
            "[{}] {}: {}{}",
            w.code,
            w.rule,
            w.message,
            if w.suggestion.is_empty() {
                String::new()
            } else {
                format!("; hint: {}", w.suggestion)
            }
        ));
    }

    // Deep check (D00*): script/env/command/reference probes against the
    // current directory (generation has no workflow file yet).
    let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for f in oxo_flow_core::deep_check::compute_deep_check(&config, &base_dir).findings {
        let rule = f.rule.as_deref().unwrap_or("-");
        let hint = f
            .suggestion
            .as_deref()
            .map(|s| format!("; hint: {s}"))
            .unwrap_or_default();
        findings.push(format!(
            "[{}] {} ({}, {}): {}{}",
            f.code, f.message, f.severity, rule, f.path, hint
        ));
    }

    findings
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
    fn validate_basic_structure_accepts_script_only_rules() {
        // `shell`-only substring matching used to reject these even though
        // the engine runs them.
        let toml = "[workflow]\nname = \"test\"\n\n[[rules]]\nname = \"s1\"\noutput = [\"o.txt\"]\nscript = \"analysis.py\"";
        assert!(validate_basic_structure(toml).is_ok());
    }

    #[test]
    fn engine_validator_accepts_system_only_builtin_rule() {
        // Issue #1415: `template --ai` pinned conda for an `echo`-only rule.
        // The CLI generation validator is the structural floor plus the core
        // engine schema — a rule with no [rules.environment] must pass both,
        // and the engine must classify it as the always-available system
        // backend instead of requiring conda on the host.
        let toml = concat!(
            "[workflow]\n",
            "name = \"hello\"\n",
            "\n",
            "[config]\n",
            "message = \"hello\"\n",
            "\n",
            "[[rules]]\n",
            "name = \"hello\"\n",
            "threads = 1\n",
            "memory = \"1G\"\n",
            "output = [\"out.txt\"]\n",
            "shell = \"\"\"\n",
            "echo \"{config.message}\" > {output[0]}\n",
            "\"\"\"\n",
        );
        assert!(validate_basic_structure(toml).is_ok());
        let config: oxo_flow_core::config::WorkflowConfig =
            toml::from_str(toml).expect("engine schema accepts a system-only rule");
        let rule = &config.rules[0];
        assert!(rule.environment.is_empty());
        assert_eq!(rule.environment.kind(), "system");
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
    fn overwrite_decision_requires_consent_for_existing_destinations() {
        // A new destination is always free to write.
        assert_eq!(
            overwrite_decision(false, false, false),
            OverwriteDecision::Proceed
        );
        // `--force` is explicit consent, interactive or not.
        assert_eq!(
            overwrite_decision(true, true, false),
            OverwriteDecision::Proceed
        );
        // An interactive terminal may confirm.
        assert_eq!(
            overwrite_decision(true, false, true),
            OverwriteDecision::Prompt
        );
        // A non-interactive session without --force refuses (never clobber
        // a reviewed workflow from CI/scripts).
        match overwrite_decision(true, false, false) {
            OverwriteDecision::Refuse(reason) => {
                assert!(reason.contains("--force"), "reason: {reason}");
                assert!(reason.contains("not interactive"), "reason: {reason}");
            }
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    #[test]
    fn destination_that_appeared_during_generation_is_refused() {
        // TOCTOU (RB-14 review): the pre-generation guard saw the path
        // absent; another process created it while the model ran. The
        // pre-write re-check must refuse and leave that file untouched.
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("raced.oxoflow");
        let target = OutputTarget::File(destination.clone());
        assert!(!destination.exists());
        std::fs::write(&destination, "someone else's workflow").unwrap();

        let err = ensure_destination_did_not_appear(&target, false, false)
            .expect_err("the raced destination must be refused");
        assert!(
            err.to_string()
                .contains("appeared while the workflow was being generated"),
            "unexpected error: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "someone else's workflow"
        );
    }

    #[test]
    fn force_consents_to_a_destination_that_appeared_during_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("raced.oxoflow");
        std::fs::write(&destination, "old").unwrap();
        let target = OutputTarget::File(destination);
        assert!(ensure_destination_did_not_appear(&target, true, false).is_ok());
    }

    #[test]
    fn destination_present_before_generation_is_not_treated_as_a_race() {
        // The user already confirmed/forced an existing destination; the
        // re-check only guards files that appeared mid-generation.
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("confirmed.oxoflow");
        std::fs::write(&destination, "old").unwrap();
        let target = OutputTarget::File(destination);
        assert!(ensure_destination_did_not_appear(&target, false, true).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn write_output_replaces_the_destination_atomically_without_following_symlinks() {
        // Atomic replacement means a rename over the destination, so a
        // symlinked destination is replaced as an entry — the link target
        // is never written through. `std::fs::write` (the pre-fix code)
        // follows the symlink and clobbers the unrelated target, which is
        // exactly the lossy behavior this pins against.
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("reviewed-workflow.oxoflow");
        std::fs::write(&real, "reviewed content").unwrap();
        let link = tmp.path().join("gen.oxoflow");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let written = write_output(&OutputTarget::File(link.clone()), "generated content").unwrap();

        assert_eq!(written, link);
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "generated content");
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "reviewed content",
            "the write must replace the destination, never write through the symlink"
        );
        assert!(
            !std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the destination must now be the generated regular file"
        );
    }

    #[test]
    fn write_output_leaves_no_staged_temp_files() {
        // The shared staged-write helper removes its temp sibling after the
        // rename; a leftover `*.tmp` next to the workflow is the failure
        // signature of a hand-rolled write.
        let tmp = tempfile::tempdir().unwrap();
        let target = OutputTarget::File(tmp.path().join("w.oxoflow"));
        write_output(&target, "content").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staged temp files left: {leftovers:?}"
        );
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

    // --- Arm B / Arm C generation-feedback toggle (issue #1315) --------------
    //
    // The matched three-arm ablation (plan:109-111) needs Arm B (syntax and
    // structural feedback ONLY) to differ from Arm C (the same loop plus
    // BioTypes/TYPE-*, scientific_preflight/SCI-*, and deep_check/D00*
    // feedback). `contract_feedback_errors` is the Arm C path; when the flag
    // is off the generation loop never calls it, so Arm B provably sees none
    // of these codes. These tests pin the code presence/absence directly, with
    // no provider round.

    /// A typed edge whose formats disagree (producer SAM, consumer BAM) — the
    /// canonical TYPE-001 violation shape from `types/negative_tests.rs`.
    const TYPED_VIOLATION: &str = "\
[workflow]
name = \"typed\"

[[rules]]
name = \"producer\"
output = [\"stage.bam\"]
shell = \"echo\"
[rules.types]
outputs = { default = \"SAM\" }

[[rules]]
name = \"consumer\"
input = [\"stage.bam\"]
output = [\"final.vcf\"]
shell = \"echo\"
[rules.types]
inputs = { default = \"BAM\" }
";

    /// A rule invoking `bwa mem` with no read group — triggers SCI-BWA-MEM-NO-RG.
    const BWA_NO_RG: &str = "\
[workflow]
name = \"bwa\"

[[rules]]
name = \"align\"
input = [\"reads.fq\"]
output = [\"aligned.bam\"]
shell = \"bwa mem ref.fa reads.fq > aligned.bam\"
";

    /// A clean, contract-free workflow: no typed edges, no scientific slips.
    const CLEAN_WORKFLOW: &str = "\
[workflow]
name = \"clean\"

[[rules]]
name = \"r\"
output = [\"a.txt\"]
shell = \"echo hi\"
";

    fn has_code(findings: &[String], prefix: &str) -> bool {
        findings.iter().any(|f| f.contains(prefix))
    }

    #[test]
    fn arm_c_contract_feedback_reports_type_violation() {
        // Arm C must surface the BioTypes (TYPE-*) finding.
        let findings = contract_feedback_errors(TYPED_VIOLATION);
        assert!(
            has_code(&findings, "TYPE-001"),
            "Arm C must report the TYPE-001 format violation, got: {findings:?}"
        );
    }

    #[test]
    fn arm_c_contract_feedback_reports_scientific_warning() {
        // Arm C must surface the scientific_preflight (SCI-*) finding.
        let findings = contract_feedback_errors(BWA_NO_RG);
        assert!(
            has_code(&findings, "SCI-BWA-MEM-NO-RG"),
            "Arm C must report SCI-BWA-MEM-NO-RG, got: {findings:?}"
        );
    }

    #[test]
    fn arm_b_baseline_emits_no_contract_codes() {
        // Arm B never calls `contract_feedback_errors`; this asserts the
        // function is the ONLY source of these codes and that a clean workflow
        // contributes none, so an Arm B run's feedback cannot carry them.
        let findings = contract_feedback_errors(CLEAN_WORKFLOW);
        assert!(
            !has_code(&findings, "TYPE-")
                && !has_code(&findings, "SCI-")
                && !has_code(&findings, "[D"),
            "a clean workflow must yield no TYPE-/SCI-/D00* contract codes, got: {findings:?}"
        );
    }
}
