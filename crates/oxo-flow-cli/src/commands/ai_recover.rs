//! AI-powered error recovery for run failures.

use anyhow::Result;
use chrono::Utc;
use colored::Colorize;
use oxo_flow_ai::{knowledge::builtin, provider::AiProvider};
use std::path::{Path, PathBuf};

/// Diagnostic result from AI error analysis.
pub struct DiagnoseResult {
    pub root_cause: String,
    pub fix_action: String,
    pub modified_toml: Option<String>,
    pub safe_to_auto_apply: bool,
}

/// Analyze a pipeline failure using AI.
pub async fn diagnose_failure(
    workflow_path: &Path,
    failed_rule: &str,
    exit_code: i32,
    stderr: &str,
    provider: &AiProvider,
) -> Result<DiagnoseResult> {
    println!();
    println!("{}", "AI Error Recovery".bold().green());
    println!("  Failed rule: {}", failed_rule.yellow());
    println!("  Exit code: {}", exit_code.to_string().red());
    println!(
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

    println!("{}", "  Diagnosing...".bold().cyan());
    let response = provider.chat(&system, &user).await?;

    // Parse the AI response
    let root_cause = extract_section(&response, "Root Cause")
        .unwrap_or_else(|| "Unknown — see full analysis".into());
    let fix_action =
        extract_section(&response, "Fix").unwrap_or_else(|| "Manual review needed".into());
    let safe = response.contains("safe to auto-apply") || response.contains("Safe to auto-apply");

    // Extract modified TOML
    let modified_toml = extract_toml_block(&response);

    println!("\n{}\n{}", "Root Cause:".bold().red(), root_cause);
    println!("{}\n{}", "Suggested Fix:".bold().yellow(), fix_action);
    println!(
        "Safe to auto-apply: {}",
        if safe { "yes".green() } else { "no".red() }
    );

    Ok(DiagnoseResult {
        root_cause,
        fix_action,
        modified_toml,
        safe_to_auto_apply: safe,
    })
}

/// Apply a fix to the workflow file, archiving the original.
pub fn apply_fix(workflow_path: &Path, modified_toml: &str, session_id: &str) -> Result<PathBuf> {
    // Archive original
    let archive_dir = archive_dir_for(workflow_path);
    std::fs::create_dir_all(&archive_dir)?;

    let original = std::fs::read_to_string(workflow_path)?;
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let backup_path = archive_dir.join(format!("{timestamp}-{session_id}-before.oxoflow"));
    std::fs::write(&backup_path, &original)?;
    println!(
        "{} Original archived to {}",
        "  ✓".green(),
        backup_path.display()
    );

    // Write fix
    std::fs::write(workflow_path, modified_toml)?;
    println!(
        "{} Fix applied to {}",
        "  ✓".green(),
        workflow_path.display()
    );

    Ok(backup_path)
}

fn archive_dir_for(workflow_path: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let local = cwd.join(".oxo-flow").join("ai_archive").join(
        workflow_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .as_ref(),
    );
    if local.parent().is_some_and(|p| p.exists()) || cwd.join(".oxo-flow").exists() {
        local
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home)
            .join(".oxo-flow")
            .join("ai_archive")
            .join(
                workflow_path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref(),
            )
    }
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

/// Extract TOML code block from AI response.
fn extract_toml_block(response: &str) -> Option<String> {
    if let Some(start) = response.find("```toml") {
        let start = start + 7;
        if let Some(end) = response[start..].find("```") {
            let content = response[start..start + end].trim().to_string();
            if content.contains("[workflow]") {
                return Some(content);
            }
        }
    }
    None
}

/// Extract a named section from the AI response.
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
            return Some(remainder[..end].trim().to_string());
        }
    }
    None
}
