//! Scientist Team — optional roles around the shared [`PipelineGenAgent`].
//!
//! The compact profile (default) is one generation agent over the shared
//! orchestrator with engine-gate feedback. The full profile adds the
//! Scientist Team roles from the generation-design proposal, each of which
//! must earn its cost in the frontier ablation (`eval/frontier/`):
//!
//! - **Curator** (Stage 1, deterministic, zero LLM): retrieves bioconda
//!   candidates, domain skills, and pipeline-graph topology for the intent
//!   up front, so the generation agent does not spend paid rounds on
//!   knowledge-tool lookups it can read from the brief.
//! - **Task contract** (Stage 0, small bounded call): standardizes an
//!   under-specified intent into an explicit contract before generation.
//! - **Independent review** (Stage 5, separate instance that never sees the
//!   generation transcript): adversarial review of contract vs artifact;
//!   a `request_changes` verdict triggers exactly one regeneration pass.

use async_trait::async_trait;

use super::{Agent, AgentContext, ValidationResult};
use crate::knowledge::{bioconda, pipeline_graph, skills};
use crate::types::Message;

/// Which roles run around the generation agent. Compact is the shipped
/// default; Full is opt-in and its value is decided by the ablation data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamProfile {
    /// One generation agent + engine-gate correction loop.
    #[default]
    Compact,
    /// Contract + Curator + generation + independent review (bounded fix).
    Full,
}

impl std::str::FromStr for TeamProfile {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "compact" => Ok(Self::Compact),
            "full" => Ok(Self::Full),
            other => Err(format!(
                "unknown team profile '{other}' (use 'compact' or 'full')"
            )),
        }
    }
}

/// Stage 1 — deterministic curation brief. No model involved: the embedded
/// knowledge bases are queried for the intent's domains, skills, tool
/// candidates, and topology transitions. Returns `None` when nothing in the
/// embedded corpora matches, so the prompt stays free of filler.
pub fn curate(intent: &str) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();

    // Domain skills (same matching the CLI's bioSkills block uses).
    let domains = skills::domains_for_intent(intent);
    let mut skill_lines: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for domain in &domains {
        for skill in skills::skills_in_domain(domain).into_iter().take(3) {
            if seen.insert(skill.name.clone()) {
                skill_lines.push(format!(
                    "- [{}] {} — {}",
                    skill.domain, skill.name, skill.description
                ));
            }
        }
    }
    if !skill_lines.is_empty() {
        sections.push(format!("### Domain procedures\n{}", skill_lines.join("\n")));
    }

    // Tool candidates: only EXACT name hits. The first ablation showed
    // fuzzy search matches crowding the brief and anchoring the generator
    // on wrong tools (the model's own tool knowledge plus lookup_tool is
    // the better source for anything the intent does not name exactly).
    let keywords = intent_keywords(intent);
    let mut tool_lines: Vec<String> = Vec::new();
    let mut seen_tools = std::collections::HashSet::new();
    for keyword in &keywords {
        if let Some(exact) = bioconda::get_tool(keyword)
            && seen_tools.insert(exact.name.to_string())
        {
            tool_lines.push(format!(
                "- {} {} — {}",
                exact.name, exact.version, exact.summary
            ));
        }
    }
    if !tool_lines.is_empty() {
        sections.push(format!(
            "### Exact Bioconda matches (treat as hints — verify unfamiliar \
             tools with lookup_tool)\n{}",
            tool_lines.join("\n")
        ));
    }

    // Topology hints: pipeline-graph nodes matching the keywords, with the
    // literature-backed next steps downstream of each.
    let mut topo_lines: Vec<String> = Vec::new();
    for keyword in intent_keywords(intent) {
        if let Some(node) = pipeline_graph::find_node(&keyword) {
            for edge in pipeline_graph::downstream(&node.id).iter().take(3) {
                topo_lines.push(format!(
                    "- {} → {} (data: {})",
                    node.name, edge.to, edge.data_types
                ));
            }
        }
    }
    if !topo_lines.is_empty() {
        let shown: Vec<String> = topo_lines.iter().take(8).cloned().collect();
        sections.push(format!("### Typical transitions\n{}", shown.join("\n")));
    }

    if sections.is_empty() {
        return None;
    }
    Some(format!(
        "## Curator Brief\nRetrieved deterministically from the embedded knowledge bases \
         (Bioconda 6k tools, bioSkills, pipeline graph). Prefer these exact names and \
         versions; the `lookup_tool` tool remains available for anything missing here.\n\n{}",
        sections.join("\n\n")
    ))
}

/// Significant words of an intent for knowledge lookups: lower-cased
/// alphanumeric tokens minus domain stopwords. Tool names often appear as
/// one token (`fastp`, `kraken2`, `bwa-mem` splits into `bwa`/`mem` — the
/// search handles partial matching).
fn intent_keywords(intent: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "from", "into", "using", "then", "after", "before", "against",
        "when", "where", "this", "that", "these", "those", "my", "our", "their", "data", "samples",
        "sample", "reads", "read", "file", "files", "analysis", "pipeline", "workflow", "step",
        "steps", "stage", "need", "needs", "want", "call", "calling", "produce", "produce",
        "output", "outputs", "report", "reports", "style", "based", "only", "also", "each", "both",
        "via", "per", "all", "set", "run",
    ];
    let mut out: Vec<String> = Vec::new();
    for token in intent.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if token.len() < 3 || STOPWORDS.contains(&token) {
            continue;
        }
        if !out.contains(&token.to_string()) {
            out.push(token.to_string());
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

// ── Stage 0: task contract ────────────────────────────────────────────────

/// Stage 0 agent: turns a possibly under-specified intent into an explicit
/// contract the generation agent can build against. Small, low-temperature,
/// no tools — the cheapest possible model call.
pub struct ContractAgent {
    intent: String,
    data_summary: Option<String>,
}

impl ContractAgent {
    pub fn new(intent: impl Into<String>, data_summary: Option<String>) -> Self {
        Self {
            intent: intent.into(),
            data_summary,
        }
    }
}

#[async_trait]
impl Agent for ContractAgent {
    fn name(&self) -> &str {
        "team-contract"
    }

    fn plan(&self, _ctx: &AgentContext) -> Message {
        Message::system(
            "You are the PI of a bioinformatics pipeline team. Turn the request into a \
             TASK CONTRACT the implementation agent can build against. Be concrete and \
             brief (max ~12 lines). Use exactly this shape:\n\n\
             GOAL: <one sentence>\n\
             ASSAY/DATA: <assay type, paired/single, platform if implied>\n\
             INPUTS: <assumed input files and formats>\n\
             OUTPUTS: <concrete deliverables>\n\
             DECISIONS: <standard choices you are making on the requester's behalf, \
             e.g. reference genome, aligner, peak caller — one per line>\n\
             SUCCESS: <what a correct pipeline achieves>\n\n\
             Where the request is under-specified, pick the field's standard default \
             (state it under DECISIONS) instead of asking questions.",
        )
    }

    fn user_message(&self, ctx: &AgentContext) -> Message {
        let mut user = format!("Request: {}\n", self.intent);
        if let Some(summary) = &self.data_summary {
            user.push_str(&format!("\nData context: {summary}\n"));
        }
        let _ = ctx;
        Message::user(&user)
    }

    fn validate(&self, content: &str, _ctx: &AgentContext) -> ValidationResult {
        let ok = content.contains("GOAL:") && content.contains("OUTPUTS:");
        if ok {
            ValidationResult::passed()
        } else {
            ValidationResult::failed(vec![
                "contract must contain GOAL: and OUTPUTS: sections".into(),
            ])
        }
    }
}

// ── Stage 5: independent review ───────────────────────────────────────────

/// A review verdict parsed from the reviewer's response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    RequestChanges(String),
}

/// Parse the reviewer's `VERDICT:` marker. Anything unparsable is treated as
/// approve-with-no-findings is NOT acceptable — an unparsable review is
/// discarded (returns `None`) so a broken reviewer never blocks delivery.
pub fn parse_verdict(review: &str) -> Option<Verdict> {
    let marker = review.lines().find(|l| l.contains("VERDICT:"))?;
    let lowered = marker.to_lowercase();
    if lowered.contains("approve") && !lowered.contains("request_changes") {
        Some(Verdict::Approve)
    } else if lowered.contains("request_changes") {
        // Findings = the rest of the response, minus the verdict line.
        let findings = review
            .lines()
            .filter(|l| !l.contains("VERDICT:"))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        Some(Verdict::RequestChanges(findings))
    } else {
        None
    }
}

/// Stage 5 agent: an INDEPENDENT evaluator instance. It sees the task
/// contract, the final artifact, and the review criteria — never the
/// generation transcript, so it cannot anchor on the author's reasoning.
pub struct ReviewAgent {
    contract: Option<String>,
    artifact: String,
}

impl ReviewAgent {
    pub fn new(contract: Option<String>, artifact: impl Into<String>) -> Self {
        Self {
            contract,
            artifact: artifact.into(),
        }
    }
}

#[async_trait]
impl Agent for ReviewAgent {
    fn name(&self) -> &str {
        "team-review"
    }

    fn plan(&self, _ctx: &AgentContext) -> Message {
        Message::system(
            "You are an independent pipeline reviewer. You did NOT write the artifact \
             under review; judge it only against the contract and the .oxoflow schema.\n\n\
             APPROVE BY DEFAULT. The engine's own validator already guarantees schema, \
             DAG, and wildcard syntax — do not repeat those checks. request_changes is \
             for scientific-blocking issues ONLY, and every one of these must hold:\n\
             - the pipeline does NOT do what the contract asks (wrong assay, wrong \
             output), or\n\
             - a tool is the wrong choice for the stated assay, or\n\
             - a step would destroy or corrupt the data (unsafe command, broken \
             dependency order), or\n\
             - a required QC step at a critical juncture is entirely absent.\n\n\
             Style, version alternatives, extra optional steps, resource guesses, and \
             anything the schema accepts are NOT findings. If the pipeline plausibly \
             satisfies the contract, the verdict MUST be approve.\n\n\
             Respond in exactly this shape:\n\n\
             VERDICT: approve\n\n\
             or\n\n\
             VERDICT: request_changes\n\
             FINDINGS:\n\
             - <which rule/step> : <blocking issue> : <what to change>\n\n\
             Findings are fed back to a regeneration pass — noise here costs a paid run \
             AND risks replacing a working pipeline, so when in doubt, approve.",
        )
    }

    fn user_message(&self, _ctx: &AgentContext) -> Message {
        let mut user = String::new();
        if let Some(contract) = &self.contract {
            user.push_str("## Task Contract\n");
            user.push_str(contract);
            user.push_str("\n\n");
        }
        user.push_str("## Artifact under review\n```toml\n");
        user.push_str(&self.artifact);
        user.push_str("\n```\n");
        Message::user(&user)
    }

    fn validate(&self, content: &str, _ctx: &AgentContext) -> ValidationResult {
        if parse_verdict(content).is_some() {
            ValidationResult::passed()
        } else {
            ValidationResult::failed(vec![
                "review must contain a 'VERDICT: approve' or 'VERDICT: request_changes' line"
                    .into(),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_profile_parses_strictly() {
        assert_eq!("compact".parse::<TeamProfile>(), Ok(TeamProfile::Compact));
        assert_eq!("FULL".parse::<TeamProfile>(), Ok(TeamProfile::Full));
        assert!("turbo".parse::<TeamProfile>().is_err());
        assert_eq!(TeamProfile::default(), TeamProfile::Compact);
    }

    #[test]
    fn curate_finds_tools_for_a_concrete_intent() {
        let brief = curate("Metagenomic profiling of shotgun reads: fastp QC, kraken2 taxonomic classification, bracken abundance re-estimation, and a Krona visualization chart")
            .expect("concrete intent should curate");
        assert!(brief.contains("Curator Brief"));
        assert!(
            brief.contains("kraken2"),
            "brief should name kraken2: {brief}"
        );
    }

    #[test]
    fn intent_keywords_drop_stopwords_and_duplicates() {
        let kws =
            intent_keywords("RNA-seq analysis pipeline: fastp QC and then fastp again with STAR");
        assert!(!kws.contains(&"and".to_string()));
        assert!(!kws.contains(&"pipeline".to_string()));
        assert_eq!(kws.iter().filter(|k| *k == "fastp").count(), 1);
        assert!(kws.contains(&"fastp".to_string()));
        assert!(kws.contains(&"star".to_string()));
    }

    #[test]
    fn verdict_parser_discriminates_and_discards_garbage() {
        assert_eq!(
            parse_verdict("VERDICT: approve\nLooks fine."),
            Some(Verdict::Approve)
        );
        let rc = parse_verdict("VERDICT: request_changes\nFINDINGS:\n- missing QC");
        assert!(matches!(rc, Some(Verdict::RequestChanges(f)) if f.contains("missing QC")));
        assert_eq!(parse_verdict("I think it's fine overall."), None);
        // "approve" must not be inferred from a request_changes line.
        assert!(matches!(
            parse_verdict("VERDICT: request_changes\n- x"),
            Some(Verdict::RequestChanges(_))
        ));
    }
}
