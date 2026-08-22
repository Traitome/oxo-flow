//! AI workflow explanation — `oxo-flow ai explain`.
//!
//! Three layers: overview (purpose + topology), per-step detail (tool,
//! resources, I/O), and scientific review (deterministic preflight
//! findings explained in plain language).
//!
//! All facts are computed deterministically from the workflow definition
//! and the embedded knowledge bases (bioSkills, pipeline graph, builtin
//! tool table, scientific preflight); the model only synthesizes
//! plain-language prose over those verified facts.

use anyhow::{Context, Result};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Explanation depth. Beginner prose defines jargon and uses analogies;
/// expert prose is parameter-level and efficiency-focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum ExplainLevel {
    Beginner,
    Expert,
}

/// One bioSkills record matched for a rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRef {
    pub name: String,
    pub domain: String,
    pub description: String,
    #[serde(default)]
    pub primary_tool: String,
}

/// One deterministic scientific-preflight finding (copied out of core so
/// the JSON schema stays CLI-owned).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarningPlan {
    pub code: String,
    pub rule: String,
    pub message: String,
    pub suggestion: String,
}

/// One pipeline-knowledge-graph node matched for a rule, with its
/// data-flow transitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRef {
    pub node_id: String,
    pub node_name: String,
    pub overview: String,
    /// Formatted upstream/downstream transitions ("X via [BAM] — N papers").
    pub transitions: Vec<String>,
}

/// One rule of the workflow, annotated for explanation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepPlan {
    pub order: usize,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub depends_on: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    pub tools: Vec<String>,
    pub skills: Vec<SkillRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphRef>,
    /// Plain-language explanation written by the model (empty until merged).
    #[serde(default)]
    pub explanation: String,
}

/// The deterministic skeleton an explanation is built on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainPlan {
    pub workflow_name: String,
    pub workflow_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_description: Option<String>,
    pub rule_count: usize,
    pub domains: Vec<String>,
    pub entry_rules: Vec<String>,
    pub final_rules: Vec<String>,
    pub steps: Vec<StepPlan>,
    /// Deterministic scientific-preflight findings.
    #[serde(rename = "review")]
    pub warnings: Vec<WarningPlan>,
    /// Model-written overview prose (empty until merged).
    #[serde(default)]
    pub overview_summary: String,
    /// Model-written plain-language summary of the scientific findings.
    #[serde(default)]
    pub review_summary: String,
}

/// Build the deterministic explanation skeleton from a workflow.
///
/// `step` limits the plan to one rule (scientific findings are filtered
/// to it as well).
pub fn build_explain_plan(config: &WorkflowConfig, step: Option<&str>) -> Result<ExplainPlan> {
    let dag = WorkflowDag::from_rules(&config.rules)
        .context("failed to build workflow DAG — is the workflow valid?")?;
    let order = dag.execution_order()?;

    if let Some(name) = step
        && !config.rules.iter().any(|r| r.name == name)
    {
        let available = config
            .rules
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("rule '{name}' not found in workflow (rules: {available})");
    }

    let rule_count = config.rules.len();

    // Assay identification: matched bioSkills domains from the workflow's
    // own text (name + description + rule names + shells).
    let intent = {
        let mut text = format!(
            "{} {}",
            config.workflow.name,
            config.workflow.description.as_deref().unwrap_or("")
        );
        for rule in &config.rules {
            text.push(' ');
            text.push_str(&rule.name);
            if let Some(shell) = rule.shell.as_deref() {
                text.push(' ');
                text.push_str(shell);
            }
        }
        text
    };
    let domains = oxo_flow_ai::knowledge::skills::domains_for_intent(&intent);

    let steps = order
        .into_iter()
        .filter(|name| step.is_none_or(|s| s == name))
        .enumerate()
        .map(|(i, name)| {
            let rule = config
                .rules
                .iter()
                .find(|r| r.name == name)
                .expect("execution order must reference existing rules");
            let tools = match_tools(rule.shell.as_deref().unwrap_or(""));
            let skills = match_skills(&name, &tools);
            let graph = match_graph(&name, &tools, &domains, &skills);
            StepPlan {
                order: i + 1,
                name: name.clone(),
                description: rule.description.clone(),
                depends_on: rule.depends_on.clone(),
                inputs: rule.input.iter().cloned().collect(),
                outputs: rule.output.iter().cloned().collect(),
                threads: (rule.effective_threads() > 1).then_some(rule.effective_threads()),
                memory: rule.effective_memory().map(str::to_string),
                environment: environment_summary(&rule.environment),
                shell: rule.shell.clone(),
                tools,
                skills,
                graph,
                explanation: String::new(),
            }
        })
        .collect();

    let warnings = oxo_flow_core::scientific_preflight::analyze_scientific_constraints(config)
        .into_iter()
        .filter(|w| step.is_none_or(|s| s == w.rule))
        .map(|w| WarningPlan {
            code: w.code,
            rule: w.rule,
            message: w.message,
            suggestion: w.suggestion,
        })
        .collect();

    Ok(ExplainPlan {
        workflow_name: config.workflow.name.clone(),
        workflow_version: config.workflow.version.clone(),
        workflow_description: config.workflow.description.clone(),
        rule_count,
        domains,
        entry_rules: dag.root_rules(),
        final_rules: dag.leaf_rules(),
        steps,
        warnings,
        overview_summary: String::new(),
        review_summary: String::new(),
    })
}

/// Lowercase alphanumerics only — used to match tool names against shell
/// text without whitespace/punctuation noise ("bwa mem" vs "BWA-MEM").
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Match builtin tool-table entries against a shell command.
fn match_tools(shell: &str) -> Vec<String> {
    let shell = normalized(shell);
    oxo_flow_ai::knowledge::builtin::TOOL_TABLE
        .iter()
        .filter(|tool| !shell.is_empty() && shell.contains(&normalized(tool.name)))
        .map(|tool| tool.name.to_string())
        .collect()
}

/// Match embedded bioSkills records for one rule.
///
/// `search_skills` matches the whole query as a substring, so each token
/// (rule name, tool names) is queried separately; results are deduplicated
/// and capped to keep the prompt bounded.
const MAX_SKILLS_PER_RULE: usize = 3;

fn match_skills(rule_name: &str, tools: &[String]) -> Vec<SkillRef> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for query in std::iter::once(rule_name).chain(tools.iter().map(String::as_str)) {
        for skill in oxo_flow_ai::knowledge::skills::search_skills(query, 2) {
            if seen.insert(skill.name.clone()) {
                out.push(SkillRef {
                    name: skill.name.clone(),
                    domain: skill.domain.clone(),
                    description: skill.description.clone(),
                    primary_tool: skill.primary_tool.clone(),
                });
            }
        }
        if out.len() >= MAX_SKILLS_PER_RULE {
            break;
        }
    }
    out.truncate(MAX_SKILLS_PER_RULE);
    out
}

/// Ground a rule in the pipeline knowledge graph, if any node matches.
fn match_graph(
    rule_name: &str,
    tools: &[String],
    domains: &[String],
    skills: &[SkillRef],
) -> Option<GraphRef> {
    let candidates = std::iter::once(rule_name.to_string())
        .chain(tools.iter().cloned())
        .chain(domains.iter().cloned())
        .chain(skills.iter().map(|s| s.primary_tool.clone()));
    let node_id = candidates
        .filter_map(|c| oxo_flow_ai::knowledge::pipeline_graph::find_node(&c))
        .map(|n| n.id.clone())
        .next()?;
    let node = oxo_flow_ai::knowledge::pipeline_graph::find_node(&node_id)?;
    let transitions = oxo_flow_ai::knowledge::pipeline_graph::format_transitions(&node.id, "")
        .lines()
        .skip(2) // drop the "## <node> — tools\n<overview>" header lines
        .filter(|l| !l.is_empty() && !l.contains("No transitions found"))
        .map(str::to_string)
        .collect();
    Some(GraphRef {
        node_id: node.id.clone(),
        node_name: node.name.clone(),
        overview: node.overview.clone(),
        transitions,
    })
}

/// One-line summary of a rule's environment declaration.
fn environment_summary(env: &oxo_flow_core::rule::EnvironmentSpec) -> Option<String> {
    let parts: Vec<String> = [
        env.conda.as_ref().map(|v| format!("conda: {v}")),
        env.mamba.as_ref().map(|v| format!("mamba: {v}")),
        env.pixi.as_ref().map(|v| format!("pixi: {v}")),
        env.docker.as_ref().map(|v| format!("docker: {v}")),
        env.singularity
            .as_ref()
            .map(|v| format!("singularity: {v}")),
        env.venv.as_ref().map(|v| format!("venv: {v}")),
        (!env.modules.is_empty()).then(|| format!("modules: {}", env.modules.join(", "))),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

// ── Command ────────────────────────────────────────────────────────────────

/// `oxo-flow ai explain <workflow.oxoflow>` — three-layer explanation.
///
/// Layers: overview (purpose + topology), per-step detail (tool, resources,
/// I/O), scientific review (deterministic preflight findings in plain
/// language). One provider call (plus at most one corrective retry when the
/// `--json` response is malformed).
pub async fn ai_explain_command(
    workflow: &Path,
    step: Option<&str>,
    level: ExplainLevel,
    json: bool,
) -> Result<()> {
    use colored::Colorize;

    // Deterministic validation happens before any provider resolution.
    let config = WorkflowConfig::from_file(workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;
    let mut plan = build_explain_plan(&config, step)?;

    let provider = oxo_flow_ai::provider::create_provider_from_env();
    if matches!(provider, oxo_flow_ai::provider::AiProvider::Noop) {
        // Degraded mode (issue #142 M10): the model is optional — the
        // deterministic grounding layers are still useful on their own.
        // `OXO_FLOW_AI_PROVIDER=disabled` explicitly opts into exactly
        // this offline path; an unconfigured provider gets the same
        // skeleton plus configuration guidance.
        let note = if std::env::var("OXO_FLOW_AI_PROVIDER")
            .is_ok_and(|v| v.eq_ignore_ascii_case("disabled"))
        {
            "AI provider disabled via OXO_FLOW_AI_PROVIDER=disabled — emitting the \
             deterministic explanation skeleton without model prose."
        } else {
            "AI provider not configured — emitting the deterministic explanation \
             skeleton without model prose. Configure one with 'oxo-flow ai setup' \
             (or OXO_FLOW_AI_PROVIDER + *_API_KEY) to add prose."
        };
        return emit_degraded_explanation(&plan, workflow, level, json, note).await;
    }

    // In --json mode stdout carries ONLY the JSON document (machine
    // output convention); the header and progress go to stderr.
    let header = format!(
        "{} {}",
        "AI Workflow Explanation".bold().green(),
        format!("— {}", workflow.display()).dimmed()
    );
    let meta = format!(
        "  Model: {}   Level: {}",
        provider.model().unwrap_or_else(|| "default".into()),
        match level {
            ExplainLevel::Beginner => "beginner",
            ExplainLevel::Expert => "expert",
        }
        .cyan()
    );
    if json {
        eprintln!("{header}\n{meta}\n");
    } else {
        println!("{header}\n{meta}\n");
    }

    let (system, user) = build_explain_prompt(&plan, level, json);
    use oxo_flow_ai::types::Message;
    let mut messages = vec![Message::system(&system), Message::user(&user)];

    let mut session = crate::commands::ai_session::AiCommandSession::begin(
        "explain",
        &plan.workflow_name,
        &provider,
    );

    if json {
        eprintln!("  {}", "Explaining...".bold().cyan());
    } else {
        println!("  {}", "Explaining...".bold().cyan());
    }
    let response = match provider.chat_with_tools_overflow_safe(&messages, &[]).await {
        Ok(response) => response,
        // Provider errors (auth, network) degrade to the deterministic
        // skeleton instead of hard-failing the command (issue #142 M10).
        Err(e) => {
            let note = format!(
                "AI provider call failed ({e}) — emitting the deterministic explanation \
                 skeleton without model prose."
            );
            session.complete_quiet(0.0);
            return emit_degraded_explanation(&plan, workflow, level, json, &note).await;
        }
    };
    session.record_usage(&response.usage);
    let mut text = response.content.unwrap_or_default();

    if json {
        // The model fills a strict JSON template; a malformed reply gets ONE
        // corrective retry, then falls back to the deterministic skeleton.
        let mut explanation = parse_ai_explanation(&text);
        if explanation.is_none() {
            eprintln!(
                "  {} AI response was not valid JSON — retrying once.",
                "⚠".yellow()
            );
            messages.push(Message::user(
                "Your response was not valid JSON matching the template. \
                 Reply with ONLY the JSON object, no other text.",
            ));
            // A failed retry falls into the same skeleton fallback below —
            // the command degrades, it does not hard-fail (issue #142 M10).
            match provider.chat_with_tools(&messages, &[]).await {
                Ok(retry) => {
                    session.record_usage(&retry.usage);
                    text = retry.content.unwrap_or_default();
                    explanation = parse_ai_explanation(&text);
                }
                Err(e) => eprintln!("  {} AI retry failed: {e}", "⚠".yellow()),
            }
        }
        match explanation {
            Some(ai) => merge_explanation(&mut plan, &ai),
            None => eprintln!(
                "  {} AI JSON still malformed — emitting the deterministic skeleton \
                 without prose fields.",
                "⚠".yellow()
            ),
        }
        let output = explain_json(&plan, workflow, level, provider.model());
        println!("\n{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("\n{text}");
    }

    // The deterministic findings are always surfaced — the model cannot
    // silently drop them.
    if !plan.warnings.is_empty() {
        eprintln!("\n{}", "Scientific findings (verified):".bold().yellow());
        for warning in &plan.warnings {
            eprintln!(
                "  ⚠ [{}] {}: {}",
                warning.code, warning.rule, warning.message
            );
            eprintln!("    {} {}", "→".bold(), warning.suggestion);
        }
    }

    if json {
        session.complete_quiet(0.9);
    } else {
        session.complete(0.9);
    }
    Ok(())
}

/// Parse the model's JSON reply into prose fields.
fn parse_ai_explanation(text: &str) -> Option<AiExplanation> {
    // Be forgiving: the model may wrap the JSON in prose or fences.
    let candidate = text
        .split("```json")
        .nth(1)
        .and_then(|s| s.split("```").next())
        .unwrap_or(text);
    let candidate = candidate
        .trim_start_matches(|c: char| {
            !c.is_ascii() || c.is_whitespace() || (!c.is_alphanumeric() && c != '{')
        })
        .to_string();
    let start = candidate.find('{')?;
    let end = candidate.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<AiExplanation>(&candidate[start..=end]).ok()
}

/// Build the `--json` output: the deterministic plan, merged prose, and
/// knowledge-base provenance.
fn explain_json(
    plan: &ExplainPlan,
    workflow: &Path,
    level: ExplainLevel,
    model: Option<String>,
) -> serde_json::Value {
    let mut output = serde_json::to_value(plan).expect("plan serializes");
    let obj = output.as_object_mut().expect("plan is an object");
    obj.insert(
        "workflow_path".into(),
        serde_json::Value::String(workflow.display().to_string()),
    );
    obj.insert(
        "level".into(),
        serde_json::Value::String(
            match level {
                ExplainLevel::Beginner => "beginner",
                ExplainLevel::Expert => "expert",
            }
            .into(),
        ),
    );
    let (graph_nodes, graph_edges) = oxo_flow_ai::knowledge::pipeline_graph::graph_stats();
    obj.insert(
        "provenance".into(),
        serde_json::json!({
            "model": model,
            "bio_skills": oxo_flow_ai::knowledge::skills::skill_count(),
            "pipeline_graph_nodes": graph_nodes,
            "pipeline_graph_edges": graph_edges,
        }),
    );
    output
}

// ── Degraded mode (no model) ───────────────────────────────────────────────

/// Emit the explanation WITHOUT model prose: the deterministic grounding
/// layers (plan facts, knowledge-base refs, scientific findings) that the
/// three-layer explain computes before any provider call.
///
/// The degraded path is a first-class output, not an error (issue #142
/// M10): it is what `OXO_FLOW_AI_PROVIDER=disabled` explicitly requests,
/// and what a failed provider call (auth/network) falls back to — the
/// command exits 0 so scripts can rely on the skeleton being present.
async fn emit_degraded_explanation(
    plan: &ExplainPlan,
    workflow: &Path,
    level: ExplainLevel,
    json: bool,
    note: &str,
) -> Result<()> {
    use colored::Colorize;

    if json {
        // stdout carries ONLY the JSON document (machine-output
        // convention); the note goes to stderr.
        eprintln!("  {} {note}", "⚠".yellow());
        let output = explain_json(plan, workflow, level, None);
        println!("\n{}", serde_json::to_string_pretty(&output)?);
    } else {
        eprintln!("\n  {} {note}", "⚠".yellow());
        print_plan_skeleton(plan);
    }

    // The deterministic findings are always surfaced — with or without a
    // model, they cannot be dropped (mirrors the happy path).
    if !plan.warnings.is_empty() {
        eprintln!("\n{}", "Scientific findings (verified):".bold().yellow());
        for warning in &plan.warnings {
            eprintln!(
                "  ⚠ [{}] {}: {}",
                warning.code, warning.rule, warning.message
            );
            eprintln!("    {} {}", "→".bold(), warning.suggestion);
        }
    }

    if json {
        eprintln!(
            "  {} run with a configured provider to add plain-language prose",
            "Hint:".bold().cyan()
        );
    } else {
        println!(
            "\n{} Run with a configured provider to add plain-language prose.",
            "Hint:".bold().cyan()
        );
    }
    Ok(())
}

/// Human-readable rendering of the deterministic skeleton (no prose) —
/// the facts the model would have wrapped in prose.
fn print_plan_skeleton(plan: &ExplainPlan) {
    use colored::Colorize;

    println!(
        "\n{} {}",
        plan.workflow_name.bold(),
        "(deterministic skeleton)".dimmed()
    );
    println!("  version: {}", plan.workflow_version);
    if let Some(ref desc) = plan.workflow_description {
        println!("  {desc}");
    }
    let domains = if plan.domains.is_empty() {
        String::new()
    } else {
        format!(", domains: {}", plan.domains.join(", "))
    };
    println!("  {} rule(s){domains}", plan.rule_count);
    println!("  entry rules: {}", plan.entry_rules.join(", "));
    println!("  final rules: {}", plan.final_rules.join(", "));
    for step in &plan.steps {
        println!();
        println!("  {}. {}", step.order, step.name.bold());
        if let Some(ref desc) = step.description {
            println!("     {desc}");
        }
        if !step.tools.is_empty() {
            println!("     tools: {}", step.tools.join(", "));
        }
        if !step.inputs.is_empty() {
            println!("     inputs: {}", step.inputs.join(", "));
        }
        if !step.outputs.is_empty() {
            println!("     outputs: {}", step.outputs.join(", "));
        }
        if let Some(threads) = step.threads {
            println!("     threads: {threads}");
        }
        if let Some(memory) = &step.memory {
            println!("     memory: {memory}");
        }
    }
}

// ── AI prose layer ─────────────────────────────────────────────────────────
//
// The model fills only these prose fields; every other field in the
// explanation is computed deterministically by the CLI.

/// Prose the model returns in `--json` mode.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AiExplanation {
    #[serde(default)]
    pub overview_summary: String,
    #[serde(default)]
    pub steps: Vec<AiStepText>,
    #[serde(default)]
    pub review_summary: String,
}

/// Per-rule prose keyed by rule name (the CLI owns the names).
#[derive(Debug, Serialize, Deserialize)]
pub struct AiStepText {
    pub name: String,
    #[serde(default)]
    pub explanation: String,
}

/// Build the (system, user) prompt pair for an explanation.
pub fn build_explain_prompt(
    plan: &ExplainPlan,
    level: ExplainLevel,
    json_mode: bool,
) -> (String, String) {
    let tone = match level {
        ExplainLevel::Beginner => {
            "Your reader is NEW to bioinformatics. Define every technical term in plain \
             language the first time it appears (e.g. 'BAM — a compressed file of read \
             alignments'). Use everyday analogies. Explain what each step does and WHY it \
             exists before touching parameters."
        }
        ExplainLevel::Expert => {
            "Your reader is an experienced bioinformatician evaluating a workflow they \
             inherited. Be concise and parameter-level: focus on scientific rationale, \
             resource efficiency, and deviations from established best practice. Do not \
             define common terms."
        }
    };

    let output_rules = if json_mode {
        "Reply with ONLY a JSON object of exactly this shape (fill the empty strings; keep \
         step names verbatim, in the same order; no markdown fences, no extra text):\n\
         {\"overview_summary\": \"\", \"steps\": [{...}], \"review_summary\": \"\"}\n"
            .to_string()
            + &json_template(plan)
    } else {
        "Write the explanation as markdown with exactly these sections:\n\
         # Overview\n\
         # Step-by-step (one ### subsection per step, keeping the step names)\n\
         # Scientific review (explain the verified findings in plain language)\n\
         End the Scientific review with the finding codes in parentheses."
            .to_string()
    };

    let system = format!(
        r#"## Role & Identity
You are a bioinformatics educator explaining an oxo-flow workflow (.oxoflow) step by step.

## Audience
{tone}

## Hard Constraints (NON-NEGOTIABLE)
- Only use facts present in the grounding data below. NEVER invent tool parameters,
  versions, resource numbers, or caveats that are not provided.
- When the grounding data has no entry for a tool or step, say so briefly —
  do not guess from general knowledge.
- Do not contradict the "Scientific findings (verified)" section; explain it instead.
- Do not describe files as existing on disk — this is a static explanation, not an inspection.

## Output Format
{output_rules}"#
    );

    let user = build_grounding_prompt(plan);
    (system, user)
}

/// JSON template with the deterministic step names pre-filled.
fn json_template(plan: &ExplainPlan) -> String {
    let steps = plan
        .steps
        .iter()
        .map(|s| format!("{{\"name\": \"{}\", \"explanation\": \"\"}}", s.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{\"overview_summary\": \"\", \"steps\": [{steps}], \"review_summary\": \"\"}}")
}

/// The deterministic grounding the model explains.
fn build_grounding_prompt(plan: &ExplainPlan) -> String {
    let mut text = String::new();
    text.push_str("## Workflow facts (verified deterministically — do not contradict these)\n");
    text.push_str(&format!("Name: {}\n", plan.workflow_name));
    text.push_str(&format!("Version: {}\n", plan.workflow_version));
    if let Some(desc) = &plan.workflow_description {
        text.push_str(&format!("Description: {desc}\n"));
    }
    text.push_str(&format!("Rules: {}\n", plan.rule_count));
    text.push_str(&format!("Entry rules: {}\n", plan.entry_rules.join(", ")));
    text.push_str(&format!("Final rules: {}\n", plan.final_rules.join(", ")));
    if !plan.domains.is_empty() {
        text.push_str(&format!(
            "Assay domains (bioSkills match): {}\n",
            plan.domains.join(", ")
        ));
    }

    text.push_str("\n## Steps in execution order\n");
    for step in &plan.steps {
        text.push_str(&format!(
            "### {}. {} (depends on: {})\n",
            step.order,
            step.name,
            if step.depends_on.is_empty() {
                "—".to_string()
            } else {
                step.depends_on.join(", ")
            }
        ));
        if let Some(desc) = &step.description {
            text.push_str(&format!("Declared purpose: {desc}\n"));
        }
        if !step.inputs.is_empty() {
            text.push_str(&format!("Inputs: {}\n", step.inputs.join(", ")));
        }
        if !step.outputs.is_empty() {
            text.push_str(&format!("Outputs: {}\n", step.outputs.join(", ")));
        }
        if let Some(threads) = step.threads {
            text.push_str(&format!("Threads: {threads}\n"));
        }
        if let Some(memory) = &step.memory {
            text.push_str(&format!("Memory: {memory}\n"));
        }
        if let Some(env) = &step.environment {
            text.push_str(&format!("Environment: {env}\n"));
        }
        if let Some(shell) = &step.shell {
            text.push_str(&format!("Shell: {shell}\n"));
        }
        if !step.tools.is_empty() {
            text.push_str(&format!("Tools: {}\n", step.tools.join(", ")));
            for tool in &step.tools {
                if let Some(tool_ref) = oxo_flow_ai::knowledge::builtin::ToolRef::find(tool) {
                    text.push_str(&format!("  - {}\n", tool_ref.to_table_row()));
                }
            }
        }
        if !step.skills.is_empty() {
            text.push_str("Domain skills (bioSkills):\n");
            for skill in &step.skills {
                text.push_str(&format!(
                    "- [{}] {} — {} (primary tool: {})\n",
                    skill.domain, skill.name, skill.description, skill.primary_tool
                ));
            }
        }
        if let Some(graph) = &step.graph {
            text.push_str(&format!(
                "Pipeline graph: {} (`{}`) — {}\n",
                graph.node_name, graph.node_id, graph.overview
            ));
            for transition in &graph.transitions {
                text.push_str(&format!("  {transition}\n"));
            }
        }
        text.push('\n');
    }

    if !plan.warnings.is_empty() {
        text.push_str("## Scientific findings (verified deterministically)\n");
        for warning in &plan.warnings {
            text.push_str(&format!(
                "- [{}] {}: {}\n  Fix: {}\n",
                warning.code, warning.rule, warning.message, warning.suggestion
            ));
        }
    }

    text
}

/// Merge AI prose into the deterministic plan, keyed by rule name.
///
/// Unknown or duplicated step names from the model are ignored; steps the
/// model skipped keep empty explanations.
pub fn merge_explanation(plan: &mut ExplainPlan, ai: &AiExplanation) {
    plan.overview_summary = ai.overview_summary.trim().to_string();
    plan.review_summary = ai.review_summary.trim().to_string();
    let mut seen = std::collections::HashSet::new();
    for ai_step in &ai.steps {
        if !seen.insert(ai_step.name.clone()) {
            continue; // model duplicated a rule — first write wins
        }
        if let Some(step) = plan.steps.iter_mut().find(|s| s.name == ai_step.name) {
            step.explanation = ai_step.explanation.trim().to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::config::WorkflowConfig;

    /// WGS germline fixture: fastp → BWA-MEM → HaplotypeCaller → VQSR,
    /// with a 3-sample cohort (below the GATK VQSR minimum of ~30).
    fn fixture_config() -> WorkflowConfig {
        let toml = r#"
[workflow]
name = "wgs-germline"
version = "0.1.0"
description = "WGS germline variant calling with GATK best practices"

[defaults]
threads = 4
memory = "8G"

[[sample_groups]]
name = "cohort"
samples = ["S1", "S2", "S3"]

[[rules]]
name = "fastp_qc"
description = "Trim and QC reads"
input = ["raw/{sample}_R1.fastq.gz", "raw/{sample}_R2.fastq.gz"]
output = ["trimmed/{sample}_R1.fq.gz", "trimmed/{sample}_R2.fq.gz"]
threads = 4
memory = "8G"
shell = "fastp --in1 {input[0]} --in2 {input[1]} --out1 {output[0]} --out2 {output[1]}"

[[rules]]
name = "bwa_align"
description = "Align reads with BWA-MEM"
input = ["trimmed/{sample}_R1.fq.gz", "trimmed/{sample}_R2.fq.gz"]
output = ["aligned/{sample}.bam"]
threads = 8
memory = "24G"
depends_on = ["fastp_qc"]
shell = "bwa mem -t {threads} ref.fa {input[0]} {input[1]} | samtools sort -o {output[0]}"

[[rules]]
name = "haplotype_call"
description = "Call germline variants"
input = ["aligned/{sample}.bam"]
output = ["variants/{sample}.g.vcf.gz"]
depends_on = ["bwa_align"]
shell = "gatk HaplotypeCaller -R ref.fa -I {input[0]} -O {output[0]}"

[[rules]]
name = "vqsr_snps"
input = ["variants/{sample}.g.vcf.gz"]
output = ["variants/recalibrated.vcf.gz"]
depends_on = ["haplotype_call"]
shell = "gatk VariantRecalibrator -V {input[0]} -O {output[0]}"
"#;
        WorkflowConfig::parse(toml).unwrap()
    }

    #[test]
    fn plan_captures_entry_and_final_rules() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();
        assert_eq!(plan.entry_rules, vec!["fastp_qc"]);
        assert_eq!(plan.final_rules, vec!["vqsr_snps"]);
    }

    #[test]
    fn plan_identifies_assay_domains_from_workflow_text() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();
        // fastp → read-qc, bwa/WGS → read-alignment, gatk → variant-calling.
        for expected in ["read-qc", "read-alignment", "variant-calling"] {
            assert!(
                plan.domains.iter().any(|d| d == expected),
                "domains {:?} should include {expected}",
                plan.domains
            );
        }
    }

    #[test]
    fn plan_matches_tools_from_shell_text() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();

        let fastp = plan.steps.iter().find(|s| s.name == "fastp_qc").unwrap();
        assert!(fastp.tools.iter().any(|t| t == "fastp"));

        // A shell with a pipeline matches both tools.
        let align = plan.steps.iter().find(|s| s.name == "bwa_align").unwrap();
        assert!(align.tools.iter().any(|t| t == "BWA-MEM"));
        assert!(align.tools.iter().any(|t| t == "samtools"));

        let call = plan
            .steps
            .iter()
            .find(|s| s.name == "haplotype_call")
            .unwrap();
        assert!(call.tools.iter().any(|t| t == "GATK HaplotypeCaller"));

        // VariantRecalibrator is not in the builtin tool table — no match.
        let vqsr = plan.steps.iter().find(|s| s.name == "vqsr_snps").unwrap();
        assert!(vqsr.tools.is_empty());
    }

    #[test]
    fn plan_matches_embedded_skills_per_rule() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();
        let fastp = plan.steps.iter().find(|s| s.name == "fastp_qc").unwrap();
        assert!(
            !fastp.skills.is_empty(),
            "fastp rule should match bioSkills"
        );
        assert!(
            fastp
                .skills
                .iter()
                .all(|s| !s.name.is_empty() && !s.description.is_empty()),
            "matched skills must be complete records"
        );
    }

    #[test]
    fn plan_grounds_rules_in_pipeline_graph() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();
        // HaplotypeCaller → variant-calling node, fed by wgs-alignment.
        let call = plan
            .steps
            .iter()
            .find(|s| s.name == "haplotype_call")
            .unwrap();
        let graph = call
            .graph
            .as_ref()
            .expect("haplotype_call should hit the graph");
        assert_eq!(graph.node_id, "variant-calling");
        assert!(
            graph
                .transitions
                .iter()
                .any(|t| t.contains("wgs-alignment")),
            "transitions should mention wgs-alignment: {:?}",
            graph.transitions
        );
    }

    #[test]
    fn plan_collects_deterministic_scientific_warnings() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();
        // 3-sample cohort < GATK VQSR minimum of ~30.
        assert_eq!(plan.warnings.len(), 1);
        let warning = &plan.warnings[0];
        assert_eq!(warning.code, "SCI-VQSR-COHORT");
        assert_eq!(warning.rule, "vqsr_snps");
        assert!(!warning.suggestion.is_empty());
    }

    #[test]
    fn prompt_contains_grounding_facts() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();
        let (system, user) = build_explain_prompt(&plan, ExplainLevel::Beginner, false);
        // The model must see the verified facts it explains: workflow
        // metadata, shells, and the deterministic scientific finding.
        assert!(user.contains("wgs-germline"));
        assert!(user.contains("fastp_qc"));
        assert!(user.contains("bwa mem"));
        assert!(user.contains("SCI-VQSR-COHORT"));
        assert!(user.contains("GATK"));
        assert!(!system.is_empty());
    }

    #[test]
    fn prompt_tone_differs_by_level() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();
        let (beginner, _) = build_explain_prompt(&plan, ExplainLevel::Beginner, false);
        let (expert, _) = build_explain_prompt(&plan, ExplainLevel::Expert, false);
        assert_ne!(beginner, expert);
        assert!(
            beginner.contains("analog"),
            "beginner tone should use analogies: {beginner}"
        );
        assert!(
            expert.contains("efficiency"),
            "expert tone should discuss efficiency: {expert}"
        );
    }

    #[test]
    fn prompt_json_mode_requests_strict_template() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();
        let (system, _) = build_explain_prompt(&plan, ExplainLevel::Beginner, true);
        assert!(system.contains("\"overview_summary\""));
        assert!(system.contains("\"fastp_qc\""));
        assert!(system.contains("ONLY"));
    }

    #[test]
    fn prompt_step_mode_contains_only_the_focused_rule() {
        let plan = build_explain_plan(&fixture_config(), Some("bwa_align")).unwrap();
        let (_, user) = build_explain_prompt(&plan, ExplainLevel::Beginner, false);
        assert!(user.contains("bwa_align"));
        assert_eq!(
            user.matches("### ").count(),
            1,
            "focused prompt should describe exactly one step"
        );
        // depends_on and matched KB records are legitimate grounding; other
        // rules' shells are not.
        assert!(
            !user.contains("fastp --in1"),
            "focused prompt leaks other rule shells"
        );
        assert!(
            !user.contains("gatk HaplotypeCaller"),
            "focused prompt leaks other rule shells"
        );
        assert!(
            !user.contains("VariantRecalibrator"),
            "focused prompt leaks other rule shells"
        );
    }

    #[test]
    fn merge_fills_prose_fields_by_rule_name() {
        let mut plan = build_explain_plan(&fixture_config(), None).unwrap();
        let ai = AiExplanation {
            overview_summary: "This workflow calls germline variants.".into(),
            steps: vec![AiStepText {
                name: "fastp_qc".into(),
                explanation: "Trims adapters and low-quality bases.".into(),
            }],
            review_summary: "The VQSR cohort is too small.".into(),
        };
        merge_explanation(&mut plan, &ai);

        assert!(plan.overview_summary.contains("germline"));
        assert!(plan.review_summary.contains("VQSR"));
        let fastp = plan.steps.iter().find(|s| s.name == "fastp_qc").unwrap();
        assert!(fastp.explanation.contains("Trims"));
        // Steps the model did not cover stay empty, not fabricated.
        let vqsr = plan.steps.iter().find(|s| s.name == "vqsr_snps").unwrap();
        assert!(vqsr.explanation.is_empty());
    }

    #[test]
    fn parse_ai_explanation_accepts_clean_and_fenced_json() {
        let clean = r#"{"overview_summary":"a","steps":[{"name":"r1","explanation":"b"}],"review_summary":"c"}"#;
        let parsed = parse_ai_explanation(clean).unwrap();
        assert_eq!(parsed.overview_summary, "a");
        assert_eq!(parsed.steps[0].name, "r1");
        assert_eq!(parsed.steps[0].explanation, "b");

        // Models often wrap JSON in prose + fences; the parser recovers it.
        let fenced = "Here is the result:\n```json\n{\"overview_summary\":\"x\",\"steps\":[],\"review_summary\":\"\"}\n```\nDone.";
        assert_eq!(parse_ai_explanation(fenced).unwrap().overview_summary, "x");
    }

    #[test]
    fn parse_ai_explanation_rejects_garbage() {
        assert!(parse_ai_explanation("I'm sorry, I cannot do that.").is_none());
        assert!(parse_ai_explanation("{\"overview_summary\": ").is_none());
    }

    #[test]
    fn explain_json_keeps_deterministic_skeleton_and_provenance() {
        let mut plan = build_explain_plan(&fixture_config(), None).unwrap();
        merge_explanation(
            &mut plan,
            &AiExplanation {
                overview_summary: "summary".into(),
                steps: vec![AiStepText {
                    name: "fastp_qc".into(),
                    explanation: "trims".into(),
                }],
                review_summary: "review".into(),
            },
        );
        let output = explain_json(
            &plan,
            std::path::Path::new("wgs.oxoflow"),
            ExplainLevel::Beginner,
            Some("deepseek-v4-pro".into()),
        );

        assert_eq!(output["workflow_path"], "wgs.oxoflow");
        assert_eq!(output["level"], "beginner");
        assert_eq!(output["overview_summary"], "summary");
        assert_eq!(output["review_summary"], "review");
        // Deterministic fields are present and correct regardless of the model.
        assert_eq!(output["steps"][0]["name"], "fastp_qc");
        assert_eq!(output["steps"][0]["explanation"], "trims");
        assert_eq!(output["steps"][3]["name"], "vqsr_snps");
        assert_eq!(output["steps"][3]["explanation"], "");
        assert_eq!(output["review"][0]["code"], "SCI-VQSR-COHORT");
        assert!(output["provenance"]["bio_skills"].as_u64().unwrap() >= 500);
        assert_eq!(output["provenance"]["model"], "deepseek-v4-pro");
    }

    #[test]
    fn merge_ignores_unknown_step_names() {
        let mut plan = build_explain_plan(&fixture_config(), None).unwrap();
        let ai = AiExplanation {
            overview_summary: String::new(),
            steps: vec![
                AiStepText {
                    name: "haplotype_call".into(),
                    explanation: "Calls variants.".into(),
                },
                AiStepText {
                    name: "hallucinated_rule".into(),
                    explanation: "Should be ignored.".into(),
                },
            ],
            review_summary: String::new(),
        };
        merge_explanation(&mut plan, &ai);
        let call = plan
            .steps
            .iter()
            .find(|s| s.name == "haplotype_call")
            .unwrap();
        assert_eq!(call.explanation, "Calls variants.");
        assert!(
            plan.steps.iter().all(|s| s.name != "hallucinated_rule"),
            "unknown rules must not be added to the plan"
        );
    }

    #[test]
    fn step_filter_limits_plan_to_one_rule() {
        let plan = build_explain_plan(&fixture_config(), Some("bwa_align")).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].name, "bwa_align");
        assert_eq!(plan.steps[0].order, 1);
        // Scientific findings are filtered to the focused rule too.
        assert!(plan.warnings.iter().all(|w| w.rule == "bwa_align"));
    }

    #[test]
    fn unknown_step_is_a_clear_error() {
        let err = build_explain_plan(&fixture_config(), Some("no_such_rule"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no_such_rule"),
            "error should name the rule: {err}"
        );
        assert!(
            err.contains("fastp_qc") && err.contains("vqsr_snps"),
            "error should list available rules: {err}"
        );
    }

    #[test]
    fn plan_orders_rules_in_execution_order_and_captures_metadata() {
        let plan = build_explain_plan(&fixture_config(), None).unwrap();

        // Rules follow DAG execution order, not just declaration order.
        let names: Vec<&str> = plan.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["fastp_qc", "bwa_align", "haplotype_call", "vqsr_snps"]
        );
        assert_eq!(plan.steps[0].order, 1);
        assert_eq!(plan.steps[3].order, 4);

        // Workflow metadata flows through.
        assert_eq!(plan.workflow_name, "wgs-germline");
        assert_eq!(plan.workflow_version, "0.1.0");
        assert!(
            plan.workflow_description
                .as_deref()
                .unwrap()
                .contains("WGS germline")
        );
        assert_eq!(plan.rule_count, 4);

        // Per-step metadata: dependencies, I/O patterns, resources.
        let align = &plan.steps[1];
        assert_eq!(align.name, "bwa_align");
        assert_eq!(align.depends_on, vec!["fastp_qc"]);
        assert_eq!(
            align.inputs,
            vec!["trimmed/{sample}_R1.fq.gz", "trimmed/{sample}_R2.fq.gz"]
        );
        assert_eq!(align.outputs, vec!["aligned/{sample}.bam"]);
        assert_eq!(align.threads, Some(8));
        assert_eq!(align.memory.as_deref(), Some("24G"));
    }
}
