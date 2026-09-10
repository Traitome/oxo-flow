//! The unified pipeline-generation persona (issue #342).
//!
//! Every surface that turns a natural-language intent into an `.oxoflow`
//! pipeline — CLI `template --ai`, `POST /api/ai/translate`, and the web
//! chat agent — runs THIS agent through the shared [`Orchestrator`]. The
//! persona owns three things that previously existed as divergent copies:
//!
//! 1. the engine-accurate system prompt (the web paths' short prompts
//!    taught a Snakemake-style dialect that the engine rejects with E017);
//! 2. `extract_toml` — one extraction routine with the anti-prose guard;
//! 3. output validation — the caller injects its engine binding as a
//!     closure (CLI: core `WorkflowConfig` parse; web: workflow service),
//!     so this crate stays engine-agnostic and validation failures feed
//!     the orchestrator's correction loop.

use std::sync::Arc;

use async_trait::async_trait;

use super::{Agent, AgentContext, ValidationResult};
use crate::knowledge::builtin::format_tool_table;
use crate::types::Message;

/// Caller-injected output validator. Receives the extracted TOML and
/// returns the validation verdict; errors are fed back to the model for
/// correction by the orchestrator.
pub type OutputValidator = Arc<dyn Fn(&str) -> ValidationResult + Send + Sync>;

// ── System prompt ──────────────────────────────────────────────────────────

/// The generation persona's system prompt: oxo-flow TOML syntax reference,
/// tool reference table, design methodology, safety rules, and output
/// contract. Single source of truth for all generation surfaces.
pub fn generation_system_prompt() -> String {
    format!(
        r#"## Role & Identity
You are an expert bioinformatics pipeline architect specializing in the oxo-flow workflow engine.
You translate high-level scientific goals into precise, production-grade .oxoflow TOML configurations.
Your pipelines must be correct, safe, reproducible, and optimized for the selected tools.

## oxo-flow TOML Syntax Reference (CRITICAL — VIOLATIONS CAUSE RUNTIME FAILURES)

### Template variable syntax: SINGLE braces ONLY
oxo-flow uses SINGLE curly braces for all template variables. NEVER use double braces.

```
CORRECT:   {{config.sample}}    {{input[0]}}    {{threads}}    {{output[0]}}    {{memory}}
WRONG:     {{{{config.sample}}}}  {{{{input[0]}}}}  {{{{threads}}}}  {{{{output[0]}}}}  {{{{memory}}}}
```

### TOML syntax: standard TOML only
- TOML does NOT support `+=` assignment. Use `=` only.
- Multi-line strings use triple quotes: `"""..."""`.
- Shell command line continuation uses `\` at end of line.
- Rules are `[[rules]]` ARRAYS with `input`/`output` as TOML string ARRAYS.
  NEVER use `[rules.name]` tables, `inputs = {{...}}` maps, `foreach`, or `depends` —
  the engine rejects unknown keys (E017).

### Full example (study this carefully):

```toml
[workflow]
name = "pipeline-name"
version = "0.1.0"
description = "What this does"

[config]
sample = "SAMPLE_ID"
ref_fasta = "reference/hg38.fa"

[defaults]
threads = 4
memory = "8G"

[[rules]]
name = "fastp"
description = "Trim and QC reads"
input = ["raw/{{config.sample}}_R1.fastq.gz", "raw/{{config.sample}}_R2.fastq.gz"]
output = ["trimmed/{{config.sample}}_R1.fq.gz", "trimmed/{{config.sample}}_R2.fq.gz", "qc/fastp.json"]
threads = 4
memory = "8G"
depends_on = []
shell = """
fastp \
    --in1 {{input[0]}} \
    --in2 {{input[1]}} \
    --out1 {{output[0]}} \
    --out2 {{output[1]}} \
    --json {{output[2]}} \
    --thread {{threads}}
"""

[rules.environment]
conda = "bioconda::fastp=0.23.4"

[[rules]]
name = "bwa_mem"
description = "Align reads with BWA-MEM"
input = ["trimmed/{{config.sample}}_R1.fq.gz", "trimmed/{{config.sample}}_R2.fq.gz"]
output = ["aligned/{{config.sample}}.bam"]
threads = 8
memory = "24G"
depends_on = ["fastp"]
shell = """
bwa mem \
    -t {{threads}} \
    -R "@RG\\tID:{{config.sample}}\\tSM:{{config.sample}}\\tPL:ILLUMINA" \
    {{config.ref_fasta}} \
    {{input[0]}} {{input[1]}} \
    | samtools sort -@ 4 -o {{output[0]}}
"""

[rules.environment]
conda = "bioconda::bwa=0.7.17"
```

### KEY SYNTAX RULES (MEMORIZE THESE):
1. Template variables use SINGLE braces: `{{config.key}}`, `{{input[0]}}`, `{{output[0]}}`, `{{threads}}`, `{{memory}}`
2. NEVER use `{{{{var}}}}` (double-brace) — this is Python/Go syntax, NOT oxo-flow
3. NEVER use `shell +=` — this is Python/Snakemake, NOT valid TOML
4. NEVER concatenate strings with `+` in TOML
5. Environment tables [rules.environment] MUST appear directly after their [[rules]] block
6. depends_on arrays list rule names, not file names
7. ALL conda packages MUST include version: `bioconda::tool=X.Y.Z`
8. NO `foreach`, `inputs`/`outputs` maps, or `[resources]` section — per-rule `threads`/`memory` fields

## Bioinformatics Tool Reference
{}

## Embedded Bioconda Tool Database
You have a `lookup_tool` function that searches the FULL embedded Bioconda CLI
database (6000+ tools with current versions and descriptions). Use it to:
- Confirm a tool exists and its exact Bioconda package name
- Get the CURRENT version for pinning (e.g. `lookup_tool("samtools")` → 1.23.x)
- Discover alternative tools by purpose keyword (e.g. `lookup_tool("peak calling")`)
- Check platform support before recommending a tool

## Pipeline Design Methodology
1. **Understand the assay type** — RNA-seq, DNA-seq, ChIP-seq, ATAC-seq, metagenomics, etc.
2. **Select tools** — Match tools to steps. Prefer the curated reference table above; use `lookup_tool` for anything not listed there or to verify current versions.
3. **Design DAG topology** — Map data flow: raw data → QC → processing → analysis → summarization.
4. **Assign resources per tool** — Use the table's recommended threads/memory exactly. Do NOT guess.
5. **Add QC at every stage** — Pre-processing QC (fastp), alignment QC (flagstat), post-analysis QC (multiQC).
6. **Pin software versions** — Every conda/container declaration must include a version. Use `lookup_tool` to get the current Bioconda version; fall back to your knowledge if the lookup misses.

## Safety Rules (NON-NEGOTIABLE)
1. **Resource constraints required**: Every [[rules]] block MUST have threads and memory fields.
2. **Environment required**: Every rule MUST declare [rules.environment] with conda or container.
3. **Version pinning required**: conda packages MUST include version (e.g., `bioconda::star=2.7.11b`).
4. **QC mandatory**: Include QC steps at critical junctures.
5. **No destructive commands**: NEVER use `rm -rf`, `>|` (force redirect), or unlink.
6. **No absolute paths except references**: Use `{{config.ref_dir}}/filename` pattern.
7. **DAG edges explicit**: Every rule consuming another's output MUST declare depends_on.
8. **Input/output validation**: Inputs must be produced by a dependency OR declared external.

## Output Requirements
Generate ONLY the .oxoflow TOML inside ```toml code fences. After the TOML, provide a brief explanation of the DAG logic and key design decisions.

Your TOML MUST include:
1. Complete [workflow] header with name derived from user intent
2. [config] section with configurable paths/parameters as variables
3. Well-named [[rules]] forming a coherent DAG via depends_on
4. Every rule has: threads, memory, shell, and [rules.environment]
5. Functional shell commands using SINGLE-brace template syntax: `{{input[0]}}`, `{{output[0]}}`, `{{threads}}`, `{{config.key}}`

## Quality Checklist (self-verify before responding)
- [ ] Every rule has threads AND memory set
- [ ] Every rule has [rules.environment] with version-pinned package
- [ ] All depends_on references exist as rule names
- [ ] Template variables use SINGLE braces: `{{var}}` NOT `{{{{var}}}}`
- [ ] NO `shell +=`, `foreach`, or string concatenation — valid TOML only
- [ ] QC step present before any alignment/processing
- [ ] Resource values match the tool reference table
"#,
        format_tool_table()
    )
}

// ── Extraction ─────────────────────────────────────────────────────────────

/// Extract `.oxoflow` TOML from a model response — the single canonical
/// implementation (previously four divergent copies across CLI and web).
///
/// Order: ```toml fence → case-variant ```TOML fence → generic ``` fence
/// containing `[workflow]` → raw text from the first `[workflow]`. The two
/// fallbacks require a `[[rules]]`/`[rules]` table so prose is never
/// mistaken for a pipeline.
pub fn extract_toml(response: &str) -> Option<String> {
    for fence in ["```toml", "```TOML"] {
        if let Some(content) = fenced_block(response, fence) {
            if !content.is_empty() {
                return Some(content);
            }
        }
    }
    if let Some(content) = fenced_block(response, "```")
        && content.contains("[workflow]")
    {
        return Some(content);
    }
    raw_workflow(response)
}

fn fenced_block(text: &str, fence: &str) -> Option<String> {
    let start = text.find(fence)? + fence.len();
    let after = &text[start..];
    let end = after.find("```")?;
    Some(after[..end].trim().to_string())
}

/// Raw fallback: everything from the first `[workflow]` line, provided at
/// least one rules table follows — guards against prose-only output.
fn raw_workflow(response: &str) -> Option<String> {
    let idx = response.find("[workflow]")?;
    let candidate = response[idx..].trim();
    let looks_like_pipeline = candidate.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("[[rules]]") || t.starts_with("[rules]")
    });
    if looks_like_pipeline {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// Structural floor used when the caller injects no engine validator:
/// the TOML must declare a workflow, at least one rule, and shells.
pub fn basic_structure_errors(toml: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if !toml.contains("[workflow]") {
        errors.push("Generated TOML missing [workflow] section".into());
    }
    if !toml.contains("[[rules]]") {
        errors.push("Generated TOML has no [[rules]] sections".into());
    }
    if !toml.contains("shell") {
        errors.push("Generated TOML rules missing 'shell' field".into());
    }
    errors
}

// ── Agent ──────────────────────────────────────────────────────────────────

/// The shared generation agent. Surfaces customize it with prompt
/// additions (skills, data reports) and an engine-bound validator.
pub struct PipelineGenAgent {
    intent: String,
    system_additions: Vec<String>,
    user_additions: Vec<String>,
    validator: Option<OutputValidator>,
}

impl PipelineGenAgent {
    pub fn new(intent: impl Into<String>) -> Self {
        Self {
            intent: intent.into(),
            system_additions: Vec::new(),
            user_additions: Vec::new(),
            validator: None,
        }
    }

    /// Inject the engine-bound validator (see [`OutputValidator`]). Without
    /// one, only the structural floor (`basic_structure_errors`) applies.
    pub fn with_validator(mut self, validator: OutputValidator) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Extra system-prompt sections (e.g. activated user-defined skills).
    pub fn with_system_addition(mut self, section: impl Into<String>) -> Self {
        self.system_additions.push(section.into());
        self
    }

    /// Extra user-prompt sections (e.g. bioSkills, data reports, template
    /// hints). External URL/file sources are rendered automatically from
    /// the agent context and need no explicit addition.
    pub fn with_user_addition(mut self, section: impl Into<String>) -> Self {
        self.user_additions.push(section.into());
        self
    }
}

#[async_trait]
impl Agent for PipelineGenAgent {
    fn name(&self) -> &str {
        "pipeline-gen"
    }

    fn plan(&self, _ctx: &AgentContext) -> Message {
        let mut prompt = generation_system_prompt();
        for addition in &self.system_additions {
            prompt.push_str("\n\n");
            prompt.push_str(addition);
        }
        Message::system(&prompt)
    }

    fn user_message(&self, ctx: &AgentContext) -> Message {
        let mut user = format!(
            "## User Request\nGenerate a .oxoflow pipeline for: {}\n\n",
            self.intent
        );
        for addition in &self.user_additions {
            user.push_str(addition);
            user.push_str("\n\n");
        }
        for src in &ctx.external_sources {
            user.push_str(&src.to_prompt_section());
            user.push('\n');
        }
        user.push_str(
            "## Task\nGenerate the optimized .oxoflow TOML configuration now. Output inside ```toml fences.",
        );
        Message::user(&user)
    }

    fn validate(&self, content: &str, _ctx: &AgentContext) -> ValidationResult {
        if let Some(validator) = &self.validator {
            return validator(content);
        }
        let errors = basic_structure_errors(content);
        if errors.is_empty() {
            ValidationResult::passed()
        } else {
            ValidationResult::failed(errors)
        }
    }

    fn extract_content(&self, response_content: &str) -> Option<String> {
        extract_toml(response_content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_toml_from_toml_fence() {
        let r = extract_toml("Here:\n```toml\n[workflow]\nname = \"x\"\n```\nDone");
        assert_eq!(r.as_deref(), Some("[workflow]\nname = \"x\""));
    }

    #[test]
    fn extract_toml_case_variant_and_generic_fence() {
        let upper = extract_toml("```TOML\n[workflow]\nname = \"x\"\n```");
        assert_eq!(upper.as_deref(), Some("[workflow]\nname = \"x\""));
        let generic = extract_toml("```\n[workflow]\nname = \"x\"\n[[rules]]\nname = \"r\"\n```");
        assert!(generic.is_some_and(|c| c.contains("[[rules]]")));
    }

    #[test]
    fn extract_toml_raw_requires_rules_table() {
        // Anti-prose guard: [workflow] without any rules table is prose.
        assert_eq!(extract_toml("blah [workflow]\nname = \"x\" and talk"), None);
        let ok = extract_toml("blah\n[workflow]\nname = \"x\"\n\n[[rules]]\nname = \"r\"\nshell = \"echo\"");
        assert!(ok.is_some_and(|c| c.starts_with("[workflow]")));
    }

    #[test]
    fn extract_toml_rejects_plain_prose() {
        assert_eq!(extract_toml("I cannot generate a pipeline right now."), None);
    }

    #[test]
    fn basic_structure_catches_missing_sections() {
        assert!(!basic_structure_errors("[workflow]").is_empty());
        assert!(basic_structure_errors("[workflow]\n[[rules]]\nshell = \"x\"").is_empty());
    }

    #[test]
    fn system_prompt_teaches_engine_schema() {
        let p = generation_system_prompt();
        assert!(p.contains("[[rules]]"));
        assert!(p.contains("depends_on"));
        assert!(p.contains("lookup_tool"));
        assert!(p.contains("NO `foreach`"));
    }

    #[test]
    fn agent_uses_injected_validator() {
        let agent = PipelineGenAgent::new("qc").with_validator(Arc::new(|content| {
            ValidationResult::failed(vec![format!("rejected: {content}")])
        }));
        let ctx = test_ctx();
        let v = agent.validate("anything", &ctx);
        assert!(!v.passed);
        assert!(v.errors[0].starts_with("rejected:"));
        // Extraction delegates to the canonical extract_toml.
        assert_eq!(
            agent.extract_content("```toml\n[workflow]\n```"),
            Some("[workflow]".into())
        );
    }

    fn test_ctx() -> AgentContext {
        AgentContext {
            intent: "x".into(),
            command: "x".into(),
            workflow_path: None,
            workflow_content: None,
            external_sources: vec![],
            max_rounds: 1,
            tool_registry: crate::tools::ToolRegistry::new(),
            tool_approver: None,
            session: crate::session::AiSession::new("t", "t", "noop", "none"),
        }
    }
}
