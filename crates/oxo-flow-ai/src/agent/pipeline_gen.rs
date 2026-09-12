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
//!    closure (CLI: core `WorkflowConfig` parse; web: workflow service),
//!    so this crate stays engine-agnostic and validation failures feed
//!    the orchestrator's correction loop.

use std::sync::Arc;

use async_trait::async_trait;

use super::{Agent, AgentContext, ValidationResult};
use crate::knowledge::builtin::format_tool_table;
use crate::types::Message;

/// Caller-injected output validator. Receives the extracted TOML and
/// returns the validation verdict; errors are fed back to the model for
/// correction by the orchestrator.
pub type OutputValidator = Arc<dyn Fn(&str) -> ValidationResult + Send + Sync>;

/// Caller-injected deterministic text repair, applied to the extracted TOML
/// BEFORE validation. Mechanical error classes (missing config declarations,
/// joined-dialect keys) are cheaper and more reliable to fix in code than to
/// spend a paid model round on — the orchestrator validates and adopts the
/// FIXED text, so a successful fix never reaches the model at all.
pub type TextFixer = Arc<dyn Fn(String) -> (String, Vec<String>) + Send + Sync>;

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
/// Order: fenced segments, scanned latest first (orchestrator transcripts
/// accumulate correction rounds, and the last complete draft is the
/// model's final word) → raw text from the first `[workflow]`. A fenced
/// candidate is accepted whole only when it parses as TOML and passes the
/// structural floor, so unparseable round-1 garbage can never leak
/// through a cross-round fence pairing; a parseable-but-incomplete
/// segment is kept only as a best-effort fallback so an injected
/// validator can still steer the correction loop. The raw fallback
/// applies the same floor, so degraded delivery can never write
/// structurally broken TOML.
pub fn extract_toml(response: &str) -> Option<String> {
    let mut fallback = None;
    for segment in fenced_segments(response).into_iter().rev() {
        let Ok(value) = toml::from_str::<toml::Value>(&segment) else {
            continue;
        };
        if floor_errors(&value).is_empty() {
            return Some(segment);
        }
        // Parseable but incomplete: keep the latest such fragment as a
        // best-effort fallback in case no complete draft exists.
        if fallback.is_none() && value.get("workflow").is_some() {
            fallback = Some(segment);
        }
    }
    fallback.or_else(|| raw_workflow(response))
}

/// All ```-delimited segments in document order, with the info tag on the
/// opening fence (```toml, ```TOML, …) dropped. A tagged fence line while
/// a segment is open closes it and starts a new one: models correct an
/// unclosed draft by opening a fresh fence, and that stray opener must
/// not be mistaken for the missing closer (which would let the stale
/// draft shadow the corrected one). An unclosed fence yields no segment —
/// its content has no verified boundary, so it is left to the raw
/// fallback.
fn fenced_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut open: Option<String> = None;
    for line in text.lines() {
        match line.trim_start().strip_prefix("```") {
            None => {
                if let Some(buf) = open.as_mut() {
                    buf.push_str(line);
                    buf.push('\n');
                }
            }
            // Bare fence: closes the open segment, or opens one from scratch.
            Some(tag) if tag.trim().is_empty() => match open.take() {
                Some(buf) => segments.push(buf.trim().to_string()),
                None => open = Some(String::new()),
            },
            // Tagged fence: reopens (implicitly closing any open segment).
            Some(_) => {
                if let Some(buf) = open.take() {
                    segments.push(buf.trim().to_string());
                }
                open = Some(String::new());
            }
        }
    }
    segments
}

/// Raw fallback: everything from a `[workflow]` line, provided the
/// candidate parses as TOML and still passes the structural floor — guards
/// against prose and against truncated output (e.g. an unclosed fence cut
/// off mid-shell-string) being delivered as a broken artifact. Trailing
/// prose after the workflow body is tolerated: the candidate is cut at the
/// first line that breaks the TOML parse. Anchors are scanned latest-first
/// — the same stale-draft-cannot-shadow invariant the fenced path got —
/// so a broken early unfenced draft cannot hide a later complete one.
fn raw_workflow(response: &str) -> Option<String> {
    let anchors: Vec<usize> = response
        .match_indices("[workflow]")
        .map(|(i, _)| i)
        .collect();
    anchors
        .iter()
        .rev()
        .find_map(|&idx| raw_workflow_from(response, idx))
}

/// Longest parseable, floor-passing prefix of the tail starting at `idx`.
fn raw_workflow_from(response: &str, idx: usize) -> Option<String> {
    let candidate = response[idx..].trim();
    // Parse the candidate; on failure retry with successively shorter
    // prefixes. The first prefix that parses and passes the floor is the
    // workflow body — prose trailing a complete document can no longer
    // break it, and a truncation mid-string never yields a parseable
    // prefix that still satisfies the floor (a truncated rule is dropped
    // with its table header, so `[[rules]]` alone — parseable TOML but
    // floor-invalid — cannot masquerade as a pipeline).
    let lines: Vec<&str> = candidate.lines().collect();
    for end in (1..=lines.len()).rev() {
        let prefix = lines[..end].join("\n");
        let Ok(value) = toml::from_str::<toml::Value>(&prefix) else {
            continue;
        };
        if floor_errors(&value).is_empty() {
            return Some(prefix);
        }
    }
    None
}

/// Structural floor over a parsed workflow: a `[workflow]` table, at
/// least one rule, and every rule carrying an execution body — `shell`,
/// `script`, or `transform` (the engine accepts all three; demanding a
/// `shell` would disqualify engine-valid script/transform rules and
/// truncate degraded salvage at the first such rule). Checked per rule
/// (an empty string counts as absent), so a dangling body-less rule
/// cannot survive salvage on the strength of an earlier rule's body,
/// which substring-level probes cannot distinguish.
fn floor_errors(value: &toml::Value) -> Vec<String> {
    let mut errors = Vec::new();
    if value.get("workflow").is_none() {
        errors.push("Generated TOML missing [workflow] section".into());
    }
    let Some(rules) = value.get("rules").and_then(|rules| rules.as_array()) else {
        errors.push("Generated TOML has no [[rules]] sections".into());
        return errors;
    };
    if rules.is_empty() {
        errors.push("Generated TOML has no [[rules]] sections".into());
    }
    for (idx, rule) in rules.iter().enumerate() {
        let has_body = ["shell", "script"].iter().any(|key| {
            rule.get(*key)
                .is_some_and(|body| body.as_str().is_some_and(|cmd| !cmd.trim().is_empty()))
        }) || rule.get("transform").is_some();
        if !has_body {
            errors.push(format!(
                "Rule {} has no 'shell', 'script', or 'transform' body",
                idx + 1
            ));
        }
    }
    errors
}

/// Structural floor for workflow text: with no engine validator injected
/// this is the sole acceptance check for generated TOML, and it gates
/// `raw_workflow`'s salvage even when a validator is in play. Parseable
/// input is checked per rule via [`floor_errors`]; unparseable input
/// falls back to substring probes so the caller still learns which
/// skeleton pieces are absent.
pub fn basic_structure_errors(toml: &str) -> Vec<String> {
    match toml::from_str::<toml::Value>(toml) {
        Ok(value) => floor_errors(&value),
        Err(_) => {
            let mut errors = Vec::new();
            if !toml.contains("[workflow]") {
                errors.push("Generated TOML missing [workflow] section".into());
            }
            if !toml.contains("[[rules]]") {
                errors.push("Generated TOML has no [[rules]] sections".into());
            }
            if !toml.contains("shell") && !toml.contains("script") && !toml.contains("transform") {
                errors
                    .push("Generated TOML rules missing 'shell'/'script'/'transform' body".into());
            }
            errors
        }
    }
}

// ── Agent ──────────────────────────────────────────────────────────────────

/// The shared generation agent. Surfaces customize it with prompt
/// additions (skills, data reports), an engine-bound validator, and an
/// optional deterministic text fixer.
pub struct PipelineGenAgent {
    intent: String,
    system_additions: Vec<String>,
    user_additions: Vec<String>,
    validator: Option<OutputValidator>,
    fixer: Option<TextFixer>,
}

impl PipelineGenAgent {
    pub fn new(intent: impl Into<String>) -> Self {
        Self {
            intent: intent.into(),
            system_additions: Vec::new(),
            user_additions: Vec::new(),
            validator: None,
            fixer: None,
        }
    }

    /// Inject the engine-bound validator (see [`OutputValidator`]). Without
    /// one, only the structural floor (`basic_structure_errors`) applies.
    pub fn with_validator(mut self, validator: OutputValidator) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Inject a deterministic repair pass (see [`TextFixer`]).
    pub fn with_text_fixer(mut self, fixer: TextFixer) -> Self {
        self.fixer = Some(fixer);
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
        let toml = extract_toml(response_content)?;
        match &self.fixer {
            None => Some(toml),
            // A fixer failure must never lose the artifact — fall back to
            // the unfixed text and let validation report the problem.
            Some(fix) => {
                let (fixed, notes) = fix(toml.clone());
                if fixed.trim().is_empty() {
                    Some(toml)
                } else {
                    if !notes.is_empty() {
                        tracing::info!(notes = ?notes, "deterministic fixes applied to the draft");
                    }
                    Some(fixed)
                }
            }
        }
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
        let ok = extract_toml(
            "blah\n[workflow]\nname = \"x\"\n\n[[rules]]\nname = \"r\"\nshell = \"echo\"",
        );
        assert!(ok.is_some_and(|c| c.starts_with("[workflow]")));
    }

    #[test]
    fn extract_toml_rejects_plain_prose() {
        assert_eq!(
            extract_toml("I cannot generate a pipeline right now."),
            None
        );
    }

    #[test]
    fn extract_toml_rejects_unparseable_raw_fallback() {
        // Live Qwen failure: the round cap hit while the ```toml fence was
        // still open, so no closing ``` exists and both fenced paths return
        // None. The raw fallback used to deliver this structurally broken
        // TOML (unterminated shell string) to the degraded arm, which wrote
        // it to disk. A raw candidate must parse as TOML or be rejected —
        // the correction loop then gets the fence-oriented retry directive
        // instead of a broken artifact.
        let truncated = concat!(
            "```toml\n",
            "[workflow]\n",
            "name = \"hisat2-align\"\n",
            "\n",
            "[[rules]]\n",
            "name = \"align\"\n",
            "shell = \"\"\"hisat2 -x {config.index} -1 {input[0]} -2 {input[1]} \\\n",
        );
        assert_eq!(extract_toml(truncated), None);
    }

    #[test]
    fn extract_toml_raw_rejects_truncated_before_rule_fields() {
        // Truncation before the rule has any fields: the bare `[[rules]]`
        // prefix parses as TOML but fails the structural floor (no shell),
        // so nothing is delivered.
        let truncated = "```toml\n[workflow]\nname = \"x\"\n\n[[rules]]\n";
        assert_eq!(extract_toml(truncated), None);
    }

    #[test]
    fn floor_accepts_script_and_transform_rule_bodies() {
        // The engine accepts `shell`, `script`, or `transform` as a rule's
        // execution body — the floor must not disqualify engine-valid
        // script/transform rules and truncate degraded salvage at the
        // first such rule.
        let draft = concat!(
            "[workflow]\n",
            "name = \"mixed\"\n",
            "\n",
            "[[rules]]\n",
            "name = \"shelled\"\n",
            "shell = \"true\"\n",
            "\n",
            "[[rules]]\n",
            "name = \"scripted\"\n",
            "script = \"run.sh\"\n",
        );
        let parsed: toml::Value = toml::from_str(draft).unwrap();
        assert!(
            floor_errors(&parsed).is_empty(),
            "engine-valid script rules pass the floor: {:?}",
            floor_errors(&parsed)
        );
        assert!(extract_toml(&format!("```toml\n{draft}\n```")).is_some());

        let bodyless = "[workflow]\nname = \"x\"\n\n[[rules]]\nname = \"empty\"\n";
        let parsed: toml::Value = toml::from_str(bodyless).unwrap();
        let errors = floor_errors(&parsed);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("no 'shell', 'script', or 'transform' body")),
            "a body-less rule still fails: {errors:?}"
        );
    }

    #[test]
    fn raw_salvage_scans_workflow_anchors_latest_first() {
        // A broken early unfenced draft must not shadow a later complete
        // unfenced draft — the same latest-first invariant the fenced path
        // got. All prefixes anchored at the first [workflow] fail (the
        // draft is truncated mid-value), so the second anchor wins.
        let response = concat!(
            "[workflow]\n",
            "name = \n",
            "prose between drafts\n",
            "[workflow]\n",
            "name = \"good\"\n",
            "\n",
            "[[rules]]\n",
            "name = \"r\"\n",
            "shell = \"true\"\n",
        );
        let salvaged = extract_toml(response)
            .unwrap_or_else(|| panic!("the later complete draft must be found"));
        assert!(
            salvaged.contains("name = \"good\""),
            "the latest anchor must win, got: {salvaged}"
        );
    }

    #[test]
    fn extract_toml_prefers_latest_draft_when_fence_reopens() {
        // Round-1 draft never closes its fence; the corrected round-2 draft
        // opens its own. The stray tagged opener must close the dangling
        // fence (not pair as its closer), so the model's final word wins.
        let response = concat!(
            "Here is a draft:\n```toml\n[workflow]\nname = \"draft\"\n[[rules]]\nname = \"r\"\nshell = \"echo old\"\n",
            "Actually, corrected:\n```toml\n[workflow]\nname = \"final\"\n[[rules]]\nname = \"r\"\nshell = \"echo new\"\n```",
        );
        let extracted = extract_toml(response).expect("corrected draft extracted");
        assert!(extracted.contains("final"));
        assert!(!extracted.contains("draft"));
    }

    #[test]
    fn extract_toml_raw_drops_dangling_shell_less_rule() {
        // A trailing rule missing its shell must not ride on an earlier
        // rule's shell: the floor is per-rule, so salvage keeps only the
        // complete rules.
        let response = "[workflow]\nname = \"x\"\n[[rules]]\nname = \"a\"\nshell = \"echo a\"\n[[rules]]\nname = \"b\"\n";
        let extracted = extract_toml(response).expect("complete rule salvaged");
        assert!(extracted.contains("name = \"a\""));
        assert!(!extracted.contains("name = \"b\""));
    }

    #[test]
    fn extract_toml_raw_salvages_prefix_before_trailing_prose() {
        // Trailing prose must not sink the artifact: the candidate is cut at
        // the first line that breaks the TOML parse.
        let ok = extract_toml(
            "[workflow]\nname = \"x\"\n\n[[rules]]\nname = \"r\"\nshell = \"echo\"\n\nNote: adjust the index path before running.",
        );
        assert!(ok.is_some_and(|c| c.starts_with("[workflow]") && c.contains("shell")));
    }

    #[test]
    fn floor_requires_shell_in_every_rule() {
        let value: toml::Value =
            toml::from_str("[workflow]\n[[rules]]\nname = \"a\"\nshell = \"echo\"").unwrap();
        assert!(floor_errors(&value).is_empty());

        let dangling: toml::Value = toml::from_str(
            "[workflow]\n[[rules]]\nname = \"a\"\nshell = \"echo\"\n[[rules]]\nname = \"b\"",
        )
        .unwrap();
        assert!(floor_errors(&dangling).iter().any(|e| e.contains("2")));
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
