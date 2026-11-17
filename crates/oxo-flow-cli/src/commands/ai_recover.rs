//! AI-powered error recovery for run failures.

use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_ai::config::AutoFixMode;
use oxo_flow_ai::{knowledge::builtin, provider::AiProvider, session::archive_before_modify};
use std::path::{Path, PathBuf};

/// Diagnostic result from AI error analysis.
pub struct DiagnoseResult {
    pub root_cause: String,
    pub fix_action: String,
    pub modified_toml: Option<String>,
    pub safe_to_auto_apply: bool,
}

/// Parse the model's safety verdict from the mandated output line
/// `- Safe to auto-apply: <yes/no>`.
///
/// The previous substring test (`contains("safe to auto-apply")`) matched
/// every conforming reply — including `- Safe to auto-apply: no` — so the
/// auto-apply gate in `run.rs` never blocked anything. Anything other than an
/// explicit `yes`/`true` verdict (missing, unparseable, or an explicit
/// `NOT safe to auto-apply` line) means: do not rewrite the user's workflow.
fn parse_safe_to_auto_apply(response_text: &str) -> bool {
    // An explicit refusal wins wherever it appears in the reply.
    if response_text
        .to_ascii_lowercase()
        .contains("not safe to auto-apply")
    {
        return false;
    }
    for line in response_text.lines() {
        // `to_ascii_lowercase` preserves byte length, so the index stays valid
        // on the original line.
        let lower = line.to_ascii_lowercase();
        let Some(idx) = lower.find("safe to auto-apply") else {
            continue;
        };
        let verdict = line[idx + "safe to auto-apply".len()..]
            .trim_start_matches(|c: char| c == ':' || c == '*' || c == '`' || c.is_whitespace())
            .trim_matches(|c: char| c == '*' || c == '_' || c == '`' || c.is_whitespace())
            .to_ascii_lowercase();
        if verdict.starts_with("yes") || verdict.starts_with("true") {
            return true;
        }
    }
    false
}

/// What the recovery path may do with an AI-proposed workflow patch.
///
/// The verdict is layered ON TOP of workflow validation (`WorkflowConfig::parse`)
/// — it never bypasses it. `ShowOnly` carries the reason the patch is only
/// displayed, so the console message can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoFixDecision {
    /// Write the patch (still subject to TOML validation).
    Apply,
    /// Prompt the user on an interactive terminal; denied elsewhere.
    Prompt,
    /// Display the proposal, never write it. The payload is the reason.
    ShowOnly(&'static str),
}

/// Decide what the recovery path may do with an AI-proposed patch, per the
/// resolved `[ai] auto_fix` mode (issue #1013 O1; audit RB-14/AI-08).
///
/// RB-14 exit evidence: `never`, `ask`, and `always` produce distinct write
/// behavior. The design-spec permission table
/// (`docs/superpowers/specs/2026-08-09-ai-native-cli-design.md`) is the
/// source of the semantics:
///
/// - `never` — only report, never modify (`ShowOnly` always)
/// - `ask` (documented default) — propose and wait for user confirmation
///   (`Prompt`; non-interactive sessions are denied, the same safe default
///   as tool-call approval)
/// - `always` — apply safe changes automatically (the pre-O1 behavior,
///   now opt-in instead of the silent default)
pub(crate) fn auto_fix_decision(mode: AutoFixMode, safe_to_auto_apply: bool) -> AutoFixDecision {
    match mode {
        AutoFixMode::Never => {
            AutoFixDecision::ShowOnly("auto_fix = \"never\" disables automatic writes")
        }
        AutoFixMode::Ask => AutoFixDecision::Prompt,
        // `always` still honors the model's own safety verdict — a patch
        // flagged unsafe is shown, never written, in every mode.
        AutoFixMode::Always if safe_to_auto_apply => AutoFixDecision::Apply,
        AutoFixMode::Always => AutoFixDecision::ShowOnly("flagged unsafe by the safety assessment"),
    }
}

/// Resolve the effective `[ai] auto_fix` mode for a workflow, mirroring the
/// `AiRuntime::new` config chain (global → project `.oxo-flow/ai.toml` →
/// workflow `[ai]`) so both surfaces agree on precedence. Missing layers
/// fall through to the documented `ask` default.
pub(crate) fn resolve_auto_fix_mode(workflow_path: &Path) -> AutoFixMode {
    let global = oxo_flow_ai::config::AiConfig::from_global_file();
    let project = workflow_path
        .parent()
        .and_then(oxo_flow_ai::config::AiConfig::from_project_file);
    let workflow = std::fs::read_to_string(workflow_path).ok().and_then(|c| {
        c.parse::<toml::Table>()
            .ok()
            .and_then(|t| oxo_flow_ai::config::AiConfig::from_workflow_toml(&t))
    });
    oxo_flow_ai::config::AiConfig::resolve_chain(
        global.as_ref(),
        project.as_ref(),
        workflow.as_ref(),
        None,
        None,
    )
    .auto_fix
}

/// Interactive confirmation for writing an AI-proposed workflow patch under
/// `auto_fix = "ask"` — the y/N pattern of `prompt_tool_approval_blocking`
/// (tool-call approval). The caller gates on interactivity first
/// (`stdin`/`stderr` `is_terminal`) and only prompts when both are
/// terminals; non-interactive sessions are denied before this runs — the
/// safe default, matching the trust boundary that AI never rewrites the
/// workflow autonomously. A `false` return here means the user (or EOF)
/// said no.
pub(crate) fn confirm_auto_fix_blocking() -> bool {
    use std::io::Write as _;

    eprintln!("  {} AI proposes a workflow fix:", "⚠".yellow());
    eprint!("  Apply the fix? [y/N] ");
    std::io::stderr().flush().ok();

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_ok() {
        let trimmed = input.trim();
        trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
    } else {
        false
    }
}

/// Analyze a pipeline failure using AI.
pub async fn diagnose_failure(
    workflow_path: &Path,
    failed_rule: &str,
    exit_code: i32,
    stderr: &str,
    provider: &AiProvider,
) -> Result<DiagnoseResult> {
    eprintln!();
    eprintln!("{}", "AI Error Recovery".bold().green());
    eprintln!("  Failed rule: {}", failed_rule.yellow());
    eprintln!("  Exit code: {}", exit_code.to_string().red());
    eprintln!(
        "  Model: {}\n",
        provider.model().unwrap_or_else(|| "default".into())
    );

    let system = build_diagnose_prompt();
    let toml_content =
        std::fs::read_to_string(workflow_path).unwrap_or_else(|_| "(unable to read)".into());

    let user = format!(
        "## Pipeline Failure\n\n\
         Rule: **{failed_rule}**\n\
         Exit code: {exit_code}\n\n\
         ## Error Output\n```\n{stderr}\n```\n\n\
         ## Current Workflow\n```toml\n{toml_content}\n```\n\n\
         ## Task\n\
         1. Match the error against known patterns\n\
         2. Identify the root cause\n\
         3. Propose a specific fix (change exact lines in the TOML)\n\
         4. Output the corrected TOML if changes are needed (inside ```toml fences)\n\
         5. State whether the fix is safe to auto-apply",
    );

    eprintln!("{}", "  Diagnosing...".bold().cyan());

    use oxo_flow_ai::types::Message;
    let messages = vec![Message::system(&system), Message::user(&user)];
    let response = provider.chat_with_tools(&messages, &[]).await?;
    let response_text = response.content.unwrap_or_default();

    // Record token usage
    tracing::info!(
        tokens_in = response.usage.prompt_tokens,
        tokens_out = response.usage.completion_tokens,
        "AI recovery analysis completed"
    );

    // Parse the AI response
    let root_cause = extract_section(&response_text, "Root Cause");
    let fix_action = extract_section(&response_text, "Fix");
    let safe = parse_safe_to_auto_apply(&response_text);

    // Extract modified TOML — only a complete replacement that survives
    // validation against the current workflow is accepted (issue #1110).
    let modified_toml = extract_toml_block(&response_text, &toml_content);

    eprintln!(
        "\n{}\n{}",
        "Root Cause:".bold().red(),
        root_cause
            .as_deref()
            .unwrap_or("Unknown — see full analysis below")
    );
    eprintln!(
        "{}\n{}",
        "Suggested Fix:".bold().yellow(),
        fix_action
            .as_deref()
            .unwrap_or("Manual review needed — see full analysis below")
    );
    eprintln!(
        "Safe to auto-apply: {}",
        if safe { "yes".green() } else { "no".red() }
    );
    // When section parsing failed the model answered off-format; surface the
    // raw analysis instead of a dead-end pointer (the old "see full analysis"
    // wording pointed at output that was never printed).
    if root_cause.is_none() || fix_action.is_none() {
        eprintln!(
            "\n{}\n{}",
            "Full Analysis:".bold().underline(),
            response_text
        );
    }

    Ok(DiagnoseResult {
        root_cause: root_cause.unwrap_or_else(|| "Unknown — see full analysis".into()),
        fix_action: fix_action.unwrap_or_else(|| "Manual review needed".into()),
        modified_toml,
        safe_to_auto_apply: safe,
    })
}

/// Compute the sibling `.bak` path that guards the pre-fix workflow
/// (`workflow.oxoflow` → `workflow.oxoflow.bak`).
fn workflow_backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "workflow.oxoflow".into());
    name.push(".bak");
    path.with_file_name(name)
}

/// Sibling temp path for the atomic write — unique per process so concurrent
/// writers never rename or delete each other's staged file.
fn temp_sibling_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "workflow.oxoflow".into());
    name.push(format!(".{}.oxo-fix.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Write `content` to `path` atomically: stage into a same-directory temp
/// file, fsync it, rename it over the target, then fsync the parent so the
/// rename is durable. Mirrors the checkpoint's staged-write discipline
/// (`write_checkpoint_staged`) — a crash mid-write must never leave a
/// truncated workflow. Returns an error (and removes the temp file) on any
/// failure.
fn write_atomically(path: &Path, content: &str) -> Result<()> {
    let parent = oxo_flow_core::parent_dir(path);
    let tmp = temp_sibling_path(path);
    let result = (|| -> std::io::Result<()> {
        use std::io::Write as _;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        // fsync the parent directory so the rename is durable (POSIX).
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!("failed to write {}: {e}", path.display())
    })
}

/// Validate an AI-proposed fix against the file it will replace (issue #1110,
/// audit ledger RB-11).
///
/// The diagnostic prompt invites the model to emit "only the changed
/// portions" — a `[workflow]`-only or `[[rules]]`-only fragment. Such a
/// fragment is a *valid* workflow on its own (zero rules is allowed), so a
/// standalone parse check cannot tell it apart from a complete document;
/// writing it verbatim silently drops every section the original carried.
/// Two gates, both required:
///
/// 1. the modified TOML must parse and validate as a workflow, via the same
///    entry point the run path uses (`oxo_flow_core::WorkflowConfig::parse`),
///    and
/// 2. it must not drop sections the original file already declared — the
///    engine's own safety rule ("adding/removing rules is not safe to
///    auto-apply") forbids a fix that shrinks the rule/include/reference set.
///
/// A genuine minimal fix keeps every rule; a fragment that would drop them is
/// refused rather than merged.
fn validate_fix(original: &str, modified: &str) -> std::result::Result<(), String> {
    let modified_config = oxo_flow_core::WorkflowConfig::parse(modified)
        .map_err(|e| format!("generated TOML failed validation ({e})"))?;

    // Use the original as a shape baseline only when it parses; a workflow
    // that already failed to load cannot tell us what must be preserved.
    let Ok(original_config) = oxo_flow_core::WorkflowConfig::parse(original) else {
        return Ok(());
    };

    let mut reasons = Vec::new();
    let mut dropped = |what: &str, was: usize, now: usize| {
        if now < was {
            reasons.push(format!("the fix drops {what} ({was} → {now})"));
        }
    };
    dropped(
        "rules",
        original_config.rules.len(),
        modified_config.rules.len(),
    );
    dropped(
        "includes",
        original_config.includes.len(),
        modified_config.includes.len(),
    );
    dropped(
        "references",
        original_config.references.len(),
        modified_config.references.len(),
    );

    if reasons.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} — a partial document must be refused, not written as a full replacement",
            reasons.join("; ")
        ))
    }
}

/// Apply a fix to the workflow file, archiving the original.
///
/// The proposed TOML is validated against the file it replaces BEFORE any
/// mutation (issue #1110, audit ledger RB-11): a partial model reply that
/// would drop existing `[[rules]]` (or fail workflow validation) is refused
/// with a precise diagnostic and the workflow is left byte-identical. On a
/// genuine fix the original is archived through the shared machinery AND
/// written to a `.bak` beside the workflow; the fix itself lands atomically
/// (temp file + rename), so a crash mid-write can never truncate the file.
pub fn apply_fix(workflow_path: &Path, modified_toml: &str, session_id: &str) -> Result<PathBuf> {
    let original = std::fs::read_to_string(workflow_path)
        .with_context(|| format!("failed to read workflow {}", workflow_path.display()))?;

    // Validate BEFORE touching the file. A rejected fix must leave the
    // workflow exactly as it was.
    if let Err(reason) = validate_fix(&original, modified_toml) {
        // Defensive: validation runs first, so the file is normally already
        // intact — but restore the original bytes if anything changed them,
        // so a rejected fix can never leave a partial file behind.
        if let Ok(current) = std::fs::read_to_string(workflow_path)
            && current != original
        {
            write_atomically(workflow_path, &original)?;
        }
        anyhow::bail!(
            "refusing to apply AI fix to {}: {reason} — the workflow is left unchanged",
            workflow_path.display()
        );
    }

    // Archive original via the shared #544/P3-10 machinery: the snapshot
    // carries the full pre-fix workflow content, so it must land 0600 and
    // the per-workflow archive dir must stay pruned — `archive_dir_for`
    // used to hand-roll the layout with neither.
    let backup_path = archive_before_modify(workflow_path, &original, session_id)?;
    eprintln!(
        "{} Original archived to {}",
        "  ✓".green(),
        backup_path.display()
    );

    // A second, close-at-hand guard: the pre-fix bytes beside the workflow.
    write_atomically(&workflow_backup_path(workflow_path), &original)?;

    // Write the fix atomically.
    write_atomically(workflow_path, modified_toml)?;
    eprintln!(
        "{} Fix applied to {}",
        "  ✓".green(),
        workflow_path.display()
    );

    Ok(backup_path)
}

fn build_diagnose_prompt() -> String {
    let error_patterns = builtin::format_error_patterns();

    format!(
        r#"## Role & Identity
You are a senior bioinformatics pipeline incident responder for oxo-flow. When a pipeline rule fails,
you diagnose the root cause with surgical precision and propose the minimal fix needed.
Your analysis must be specific, actionable, and safe.

## Error Pattern Reference (Common Failures)
{error_patterns}

## Diagnostic Protocol (Execute in Order)
### Step 1 — Triage
- Classify the failure: resource (OOM/timeout), data (missing/corrupt input), software (version/API change), configuration (wrong params/paths)
- Map exit code to category: 137=OOM, 139=segfault, 1=user error, 127=command not found

### Step 2 — Root Cause Analysis
- Read the error output CAREFULLY — the answer is usually in the last 5 lines
- Cross-reference against error patterns above
- Check: is the tool version compatible with the provided parameters?
- Check: are input files actually produced by the dependency rule?
- Check: does the working directory have sufficient disk space?

### Step 3 — Fix Proposal
- Identify the EXACT line(s) in the TOML that need changing
- Propose the MINIMAL change — do not restructure the workflow unless essential
- If the fix is a resource adjustment (threads/memory), specify EXACT new values
- If the fix requires a tool parameter change, verify against the tool's documentation
- NEVER propose removing QC steps, validation, or safety checks

### Step 4 — Safety Assessment
- **Safe to auto-apply**: parameter tuning (threads, memory), fixing a typo, adding a missing flag
- **NOT safe to auto-apply**: adding/removing rules, changing DAG edges, modifying shell logic, changing file paths

## Output Format
Respond in this exact structure:

```
## Root Cause
<1-2 sentences explaining WHY the failure occurred, citing specific evidence from the error output>

## Proposed Fix
<Specific action. If changing TOML parameters, list old→new values. Reference exact lines.>

## Corrected TOML
```toml
<complete corrected [workflow] section or [[rules]] block — only the changed portions>
```

## Safety Assessment
- Safe to auto-apply: <yes/no>
- Risk level: <low/medium/high>
- Rollback: <how to undo this change>

## Prevention
<1 sentence on how to prevent this failure in future pipelines>
```

⚠ ONLY propose changes that fix the reported error. Do NOT make unrelated improvements.
"#
    )
}

/// Extract the TOML code block from an AI response — but ONLY when it is a
/// complete, valid replacement for `original` (issue #1110, audit ledger
/// RB-11).
///
/// The diagnostic prompt invites "only the changed portions", so a model may
/// answer with a bare `[workflow]` or `[[rules]]` fragment. Such a fragment
/// is a *valid* workflow on its own, yet writing it verbatim would silently
/// drop every section the original carried — the failure this guards. There
/// is no hand-rolled merge: a candidate is accepted only if it is a full
/// document that survives [`validate_fix`] against `original` (parses, and
/// drops no rules/includes/references). Anything less is skipped, never
/// returned as a replacement; if no fence qualifies the model's proposal is
/// simply not applied.
///
/// Fence pairing is the shared state machine
/// (`pipeline_gen::fenced_segments`); the guard runs per segment so a first
/// fragment without anchors cannot hide a later complete fix block.
fn extract_toml_block(response: &str, original: &str) -> Option<String> {
    oxo_flow_ai::agent::pipeline_gen::fenced_segments(response)
        .into_iter()
        .find(|(tag, body)| {
            tag.eq_ignore_ascii_case("toml")
                && (body.contains("[workflow]") || body.contains("[[rules]"))
                && validate_fix(original, body).is_ok()
        })
        .map(|(_, body)| body)
}

/// Extract a named section from the AI response.
///
/// Tries the inline patterns first (`Marker:`, `**Marker**:`, ...), then the
/// markdown headings the diagnostic prompt itself mandates (`## Root Cause`,
/// `## Proposed Fix`). The heading form is what conforming replies actually
/// use — without it every well-formed answer fell through to the fallback
/// text and the real analysis was dropped.
fn extract_section(text: &str, marker: &str) -> Option<String> {
    let patterns = [
        format!("[{marker}]:"),
        format!("[{marker}]"),
        format!("**{marker}**:"),
        format!("{marker}:"),
    ];
    for pat in &patterns {
        if let Some(pos) = text.find(pat.as_str()) {
            let remainder = &text[pos + pat.len()..];
            let end = remainder
                .find("\n[")
                .or_else(|| remainder.find("\n**"))
                .or_else(|| remainder.find("\n```"))
                .unwrap_or(remainder.len());
            let found = remainder[..end].trim();
            if !found.is_empty() {
                return Some(found.to_string());
            }
        }
    }
    extract_heading_section(text, marker)
}

/// Capture the body under a markdown heading whose title contains `marker`
/// (case-insensitive), ending at the next heading of any level or a code fence.
fn extract_heading_section(text: &str, marker: &str) -> Option<String> {
    let marker_lower = marker.to_ascii_lowercase();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with('#') {
            continue;
        }
        let title = trimmed.trim_start_matches('#').trim().to_ascii_lowercase();
        if !title.contains(&marker_lower) {
            continue;
        }
        let body: String = text
            .lines()
            .skip(idx + 1)
            .take_while(|l| {
                let t = l.trim();
                !(t.starts_with('#') || t.starts_with("```"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let body = body.trim();
        if !body.is_empty() {
            return Some(body.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::apply_fix;
    use super::{AutoFixDecision, resolve_auto_fix_mode};
    use super::{auto_fix_decision, extract_section, extract_toml_block, parse_safe_to_auto_apply};
    use oxo_flow_ai::config::AutoFixMode;
    use std::path::Path;

    #[test]
    fn safe_gate_requires_an_explicit_yes() {
        assert!(parse_safe_to_auto_apply("- Safe to auto-apply: yes"));
        assert!(parse_safe_to_auto_apply("- **Safe to auto-apply**: YES"));
        assert!(parse_safe_to_auto_apply("Safe to auto-apply: true"));
        // The regression: the old substring test matched this exact line.
        assert!(!parse_safe_to_auto_apply("- Safe to auto-apply: no"));
        assert!(!parse_safe_to_auto_apply("- Safe to auto-apply: <yes/no>"));
        assert!(!parse_safe_to_auto_apply(
            "- **NOT safe to auto-apply**: changing DAG edges"
        ));
        // Missing/unparseable verdicts must never authorize a rewrite.
        assert!(!parse_safe_to_auto_apply(
            "## Root Cause\nbwa ran out of memory"
        ));
        assert!(!parse_safe_to_auto_apply(""));
    }

    /// RB-14 exit evidence: `never`, `ask`, and `always` produce distinct
    /// write behavior for the same (safe) proposal.
    #[test]
    fn auto_fix_modes_produce_distinct_write_behavior() {
        // never: the patch is never written, whatever the model says.
        assert_eq!(
            auto_fix_decision(AutoFixMode::Never, true),
            AutoFixDecision::ShowOnly("auto_fix = \"never\" disables automatic writes")
        );
        // ask (the documented default): distinct from both — the patch is
        // held for an interactive confirmation, not written outright and
        // not hard-blocked.
        assert_eq!(
            auto_fix_decision(AutoFixMode::Ask, true),
            AutoFixDecision::Prompt
        );
        // always: the only mode that reaches a write — and only when the
        // model's own safety verdict passed.
        assert_eq!(
            auto_fix_decision(AutoFixMode::Always, true),
            AutoFixDecision::Apply
        );
        // always does NOT bypass the safety verdict: a patch flagged unsafe
        // is shown, never written, in every mode.
        assert_eq!(
            auto_fix_decision(AutoFixMode::Always, false),
            AutoFixDecision::ShowOnly("flagged unsafe by the safety assessment")
        );
        assert_eq!(
            auto_fix_decision(AutoFixMode::Never, false),
            AutoFixDecision::ShowOnly("auto_fix = \"never\" disables automatic writes")
        );
    }

    /// The documented default is `ask` — a workflow with no `[ai]` section
    /// at all must not silently upgrade to auto-apply.
    #[test]
    fn resolve_auto_fix_mode_defaults_to_ask_without_ai_section() {
        with_temp_cwd("autofix-default", |_| {
            let wf = Path::new("workflow.oxoflow");
            std::fs::write(wf, "[workflow]\nname = \"t\"\n").unwrap();
            assert_eq!(resolve_auto_fix_mode(wf), AutoFixMode::Ask);
        });
    }

    /// Precedence chain: workflow `[ai]` beats project `.oxo-flow/ai.toml`
    /// beats global config; a missing layer falls through (mirrors
    /// `AiRuntime::new`).
    #[test]
    fn resolve_auto_fix_mode_follows_config_chain_precedence() {
        with_temp_cwd("autofix-chain", |_| {
            let wf = Path::new("workflow.oxoflow");
            std::fs::write(wf, "[ai]\nauto_fix = \"never\"\n").unwrap();
            assert_eq!(resolve_auto_fix_mode(wf), AutoFixMode::Never);

            // Project layer wins when the workflow is silent.
            std::fs::write(wf, "[workflow]\nname = \"t\"\n").unwrap();
            std::fs::create_dir_all(".oxo-flow").unwrap();
            std::fs::write(".oxo-flow/ai.toml", "[ai]\nauto_fix = \"always\"\n").unwrap();
            assert_eq!(resolve_auto_fix_mode(wf), AutoFixMode::Always);

            // An invalid workflow value parses as the default (ask), so the
            // project layer's non-default setting survives.
            std::fs::write(wf, "[ai]\nauto_fix = \"suggest\"\n").unwrap();
            assert_eq!(resolve_auto_fix_mode(wf), AutoFixMode::Always);
        });
    }

    /// A fully-conforming reply uses the heading format the prompt mandates.
    #[test]
    fn section_extraction_matches_prompt_mandated_headings() {
        let reply = "## Root Cause\nThe rule requested 16 threads but the \
machine exposes 4; the scheduler killed it (exit 137).\n\n## Proposed Fix\n\
Change `threads = 16` to `threads = 4` in rule `align`.\n\n## Corrected TOML\n\
```toml\n[[rules]]\nname = \"align\"\nthreads = 4\n```\n\n## Safety Assessment\n\
- Safe to auto-apply: yes\n- Risk level: low\n";

        let cause = extract_section(reply, "Root Cause").expect("root cause parsed");
        assert!(cause.contains("exit 137"), "got: {cause}");
        let fix = extract_section(reply, "Fix").expect("fix parsed");
        assert!(fix.contains("threads = 4"), "got: {fix}");
    }

    #[test]
    fn section_extraction_stops_at_next_heading_or_fence() {
        let reply = "## Root Cause\nbad reference genome\n\n## Proposed Fix\npoint to hg38";
        assert_eq!(
            extract_section(reply, "Root Cause").as_deref(),
            Some("bad reference genome")
        );
        assert_eq!(
            extract_section(reply, "Fix").as_deref(),
            Some("point to hg38")
        );
    }

    #[test]
    fn section_extraction_still_supports_inline_markers() {
        assert_eq!(
            extract_section("**Root Cause**: typo in path", "Root Cause").as_deref(),
            Some("typo in path")
        );
        assert_eq!(
            extract_section("Root Cause: typo in path", "Root Cause").as_deref(),
            Some("typo in path")
        );
    }

    // A full workflow the model may legitimately return unchanged.
    const ORIGINAL: &str = "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"align\"\nthreads = 4\nshell = \"echo align\"\n";

    /// A complete document is always a candidate; a fragment that drops the
    /// original's rules is rejected (issue #1110, audit ledger RB-11).
    #[test]
    fn toml_block_rejects_partial_fragments_and_keeps_complete_documents() {
        // A complete document (workflow + rules) passes. The fence body is
        // trimmed of its trailing newline by `fenced_segments`.
        let full = format!("```toml\n{ORIGINAL}```");
        assert_eq!(
            extract_toml_block(&full, ORIGINAL).as_deref(),
            Some(ORIGINAL.trim_end())
        );

        // A `[workflow]`-only fragment against a file with rules parses
        // standalone but would drop them — refused, never returned.
        let wf_only = "```toml\n[workflow]\nname = \"x\"\n```";
        assert!(extract_toml_block(wf_only, ORIGINAL).is_none());

        // A `[[rules]]`-only fragment has no `[workflow]` table, so it is not
        // a complete workflow — refused.
        let rules_only =
            "```toml\n[[rules]]\nname = \"align\"\nthreads = 2\nshell = \"echo x\"\n```";
        assert!(extract_toml_block(rules_only, ORIGINAL).is_none());

        // Non-TOML / unterminated blocks are still ignored.
        assert!(extract_toml_block("```toml\nnot toml at all\n```", ORIGINAL).is_none());
        assert!(extract_toml_block("no fence here", ORIGINAL).is_none());
    }

    /// `apply_fix` resolves the archive root from the process cwd, so these
    /// tests run inside a temp cwd (pattern from web `workspace.rs` tests).
    /// The cwd dance takes the crate-shared CWD_LOCK (commands/mod.rs,
    /// issue #982 P1-1) — run_preview's when-gate cwd test dances the same
    /// process state and must not interleave.
    fn with_temp_cwd(tag: &str, f: impl FnOnce(&Path)) {
        let _cwd = crate::commands::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::Builder::new()
            .prefix(&format!("oxo-recover-{tag}-"))
            .tempdir()
            .unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        // A project-local `.oxo-flow` marker makes `resolve_oxo_flow_dir`
        // anchor here instead of falling back to the real `$HOME` — the
        // layout assertions below must never touch the developer's home.
        std::fs::create_dir_all(".oxo-flow").unwrap();
        std::fs::write("workflow.oxoflow", "[workflow]\nname = \"t\"\n").unwrap();

        let _restore = crate::commands::RestoreCwd(prev);

        f(tmp.path());
    }

    #[test]
    fn recovered_archive_is_owner_only() {
        // Issue #964: recover-side backups carry the full pre-fix workflow
        // content — they must land 0600 exactly like the orchestrator-side
        // archives (`archived_snapshot_is_owner_only` in oxo-flow-ai).
        with_temp_cwd("perm", |_| {
            let backup = apply_fix(
                Path::new("workflow.oxoflow"),
                "[workflow]\nname = \"fixed\"\n",
                "sess-964",
            )
            .unwrap();

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "recover archive must be owner-only"
                );
            }
            let _ = backup;
        });
    }

    #[test]
    fn recovered_archive_prunes_oldest_snapshots() {
        // Issue #964: repeated recover runs previously grew
        // `.oxo-flow/ai_archive/<stem>/` without bound; the shared machinery
        // caps it (200 kept here — verified via the prune contract on a
        // smaller keep by `archive_prune_keeps_the_newest_snapshots_per_workflow`,
        // so this test only pins that the cap applies on the recover path).
        with_temp_cwd("prune", |_| {
            // Pre-fill the per-workflow archive dir beyond the cap with
            // older-dated snapshots, then let apply_fix add one more.
            let dir = std::env::current_dir()
                .unwrap()
                .join(".oxo-flow/ai_archive/workflow");
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..200 {
                let p = dir.join(format!("20250101-0000{i:02}-old{i}-before.oxoflow"));
                std::fs::write(&p, "old").unwrap();
                // Distinct mtimes: a same-second batch would leave the sort
                // order to the filesystem.
                let t = filetime::FileTime::from_unix_time(1_700_000_000 + i as i64, 0);
                filetime::set_file_mtime(&p, t).unwrap();
            }

            apply_fix(
                Path::new("workflow.oxoflow"),
                "[workflow]\nname = \"fixed\"\n",
                "sess-prune",
            )
            .unwrap();

            let remaining: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                remaining.len(),
                200,
                "recover archives must be pruned to the cap: got {}",
                remaining.len()
            );
            assert!(
                !remaining.iter().any(|n| n.contains("old0-")),
                "the oldest pre-filled snapshot must be pruned: {remaining:?}"
            );
            assert!(
                remaining.iter().any(|n| n.contains("sess-prune")),
                "the fresh snapshot must survive the prune: {remaining:?}"
            );
        });
    }

    #[test]
    fn recovered_archive_layout_matches_session_machinery() {
        // The recover path must not retype its own archive layout: the
        // backup lives under `.oxo-flow/ai_archive/<stem>/` with the
        // `{timestamp}-{session}-before.oxoflow` name the shared helper
        // owns (and the fix is actually written).
        with_temp_cwd("layout", |_| {
            let backup = apply_fix(
                Path::new("workflow.oxoflow"),
                "[workflow]\nname = \"fixed\"\n",
                "sess-layout",
            )
            .unwrap();

            let expected_parent = std::env::current_dir()
                .unwrap()
                .join(".oxo-flow/ai_archive/workflow");
            assert_eq!(
                backup.parent().unwrap(),
                expected_parent,
                "archive dir must come from the shared resolver"
            );
            assert!(
                backup
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with("-sess-layout-before.oxoflow"),
                "backup filename must follow the shared convention: {}",
                backup.display()
            );
            assert_eq!(
                std::fs::read_to_string("workflow.oxoflow").unwrap(),
                "[workflow]\nname = \"fixed\"\n",
                "the fix itself must be written"
            );
        });
    }

    /// RB-11 regression: a `[workflow]`-only model reply against a file that
    /// carried `[[rules]]` must be refused — the file stays byte-identical,
    /// never truncated to the fragment. (Both `apply_fix` and the extraction
    /// gate refuse; this pins the writer.)
    #[test]
    fn apply_fix_refuses_partial_document_and_leaves_file_untouched() {
        with_temp_cwd("refuse-partial", |_| {
            let original =
                "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"align\"\nshell = \"echo align\"\n";
            std::fs::write("workflow.oxoflow", original).unwrap();

            let err = apply_fix(
                Path::new("workflow.oxoflow"),
                "[workflow]\nname = \"x\"\n",
                "sess-1110",
            );
            assert!(
                err.is_err(),
                "a partial document must be refused, not written"
            );
            assert_eq!(
                std::fs::read_to_string("workflow.oxoflow").unwrap(),
                original,
                "the workflow must be left byte-identical after a refusal"
            );
            // No `.bak` is left behind: nothing was applied.
            assert!(!Path::new("workflow.oxoflow.bak").exists());
        });
    }

    /// Positive: a valid full-document fix still applies, and the `.bak`
    /// guard beside the workflow holds the ORIGINAL pre-fix bytes.
    #[test]
    fn apply_fix_writes_valid_fix_and_bak_holds_original() {
        with_temp_cwd("apply-full", |_| {
            let original = "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"align\"\nthreads = 4\nshell = \"echo align\"\n";
            std::fs::write("workflow.oxoflow", original).unwrap();
            // A genuine minimal fix: same rules, one changed parameter.
            let fixed = "[workflow]\nname = \"t\"\n\n[[rules]]\nname = \"align\"\nthreads = 1\nshell = \"echo align\"\n";

            apply_fix(Path::new("workflow.oxoflow"), fixed, "sess-apply").unwrap();

            assert_eq!(
                std::fs::read_to_string("workflow.oxoflow").unwrap(),
                fixed,
                "the valid fix must be written"
            );
            assert_eq!(
                std::fs::read_to_string("workflow.oxoflow.bak").unwrap(),
                original,
                "the .bak guard must hold the original pre-fix bytes"
            );
        });
    }
}
