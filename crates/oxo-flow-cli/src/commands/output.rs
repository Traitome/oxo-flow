//! Logic for output-related subcommands: graph, report, diff, export.

use crate::commands::{print_banner, resolve_workflow};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::CheckpointState;
use oxo_flow_core::report::{Report, ReportContent, ReportSection, TemplateEngine};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub fn handle_graph(
    workflow: PathBuf,
    format: String,
    output: Option<PathBuf>,
    expanded: bool,
) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(Some(workflow))?;
    let mut config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // --expanded: show the runtime DAG after wildcard/sample/scatter
    // expansion — the actual DAG that `run` executes.
    if expanded {
        config.apply_defaults();
        config
            .expand_wildcards()
            .context("failed to expand wildcard rules")?;
    }

    let dag = WorkflowDag::from_rules(&config.rules).context("failed to build workflow DAG")?;

    let result = match format.as_str() {
        "ascii" => dag.to_ascii().map_err(|e| anyhow::anyhow!(e)),
        "dot" => Ok(dag.to_dot()),
        "dot-clustered" => dag.to_dot_clustered().map_err(|e| anyhow::anyhow!(e)),
        "tree" => dag.to_ascii_tree().map_err(|e| anyhow::anyhow!(e)),
        "mermaid" => Ok(dag.to_mermaid()),
        "metro" => dag.to_metro(&config.rules).map_err(|e| anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("unsupported graph format: {}", format)),
    }?;

    if let Some(path) = output {
        std::fs::write(&path, result)?;
        eprintln!("{} Graph saved to {}", "✓".green(), path.display());
    } else {
        println!("{}", result);
    }

    Ok(())
}

/// Grouped arguments for the report command — keeps the handler
/// signature small under `-D warnings` (clippy::too_many_arguments).
pub struct ReportArgs {
    pub workflow: Option<PathBuf>,
    pub format: Option<String>,
    pub output: Option<PathBuf>,
    pub checkpoint_path: Option<PathBuf>,
    pub ai: bool,
    pub workdir: Option<PathBuf>,
    pub ci: bool,
    pub no_timestamps: bool,
    pub strict: bool,
    pub list_sections: bool,
    pub run_dir: Option<PathBuf>,
    pub failed: bool,
    pub plan: bool,
    pub init_template: bool,
    pub list_templates: bool,
    pub r_data: Option<PathBuf>,
    pub diff: Option<PathBuf>,
    pub acct: Option<PathBuf>,
}

/// Auto-discover a workflow file for `report` (issue #83 WS5).
///
/// Unlike `commands::discover_workflow_file_in` (alphabetically-first), an
/// ambiguous directory is an error: a report is a one-shot artifact and the
/// user must pick the workflow explicitly rather than get a silent arbitrary
/// choice.
fn discover_report_workflow_in(dir: &Path) -> Result<PathBuf> {
    // Priority: main.oxoflow (the conventional single-entry workflow).
    let main_workflow = dir.join("main.oxoflow");
    if main_workflow.exists() {
        return Ok(main_workflow);
    }

    let mut oxoflow_files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read directory {}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "oxoflow"))
        .collect();
    oxoflow_files.sort();

    match oxoflow_files.len() {
        0 => anyhow::bail!(
            "no workflow found in {} — pass WORKFLOW explicitly, or use --plan for a template-only report",
            dir.display()
        ),
        1 => Ok(oxoflow_files.pop().expect("len == 1")),
        _ => anyhow::bail!(
            "multiple .oxoflow files found in {} — pass WORKFLOW explicitly to choose one",
            dir.display()
        ),
    }
}

/// Load the checkpoint at `path`, honoring `--strict` (exit 2 when the data
/// source is unavailable, issue #83 P1-17). Returns `None` when absent and
/// not strict — the report degrades to template-level data.
fn load_checkpoint(
    path: &Path,
    strict: bool,
) -> Result<Option<oxo_flow_core::executor::CheckpointState>> {
    match oxo_flow_core::executor::CheckpointState::load_from_file(path) {
        Ok(cp) => Ok(Some(cp)),
        Err(_) => {
            if strict {
                // Exit code 2 = data source unavailable — CI can tell a
                // template-only report from a complete one (issue #83 P1-17).
                eprintln!("  {} No checkpoint found at {}", "✗".red(), path.display());
                eprintln!(
                    "  Run the workflow first, or drop --strict to allow a template-level report."
                );
                std::process::exit(2);
            }
            eprintln!(
                "  {} No checkpoint found at {}",
                "Note:".yellow(),
                path.display()
            );
            eprintln!(
                "  {} Report will show template-level data only. Run the workflow first for execution metrics.",
                "Info:".dimmed()
            );
            Ok(None)
        }
    }
}

/// Result of workflow resolution for `report`.
struct ResolvedReportWorkflow {
    workflow: PathBuf,
    /// Some when the workflow was auto-discovered (zero-arg or --run): the
    /// directory whose `.oxo-flow/` anchors the checkpoint and the default
    /// report output.
    discovery_dir: Option<PathBuf>,
    /// Checkpoint already loaded during discovery — either the workflow
    /// came from its workflow_path, or the checkpoint exists but cannot
    /// pin the workflow. Carried forward so the reporting phase never
    /// loads (or warns about) it twice.
    checkpoint: Option<oxo_flow_core::executor::CheckpointState>,
    /// Path of that pre-loaded checkpoint.
    checkpoint_path: Option<PathBuf>,
}

/// Resolve the workflow for `report` (issue #83 WS5): explicit path wins;
/// otherwise auto-discovery — `--run DIR` > `--workdir` > cwd — via the
/// discovery-relative checkpoint's workflow_path (falling back to a unique
/// `*.oxoflow` in the directory). `--plan` skips the checkpoint load.
fn resolve_report_workflow(
    workflow: Option<PathBuf>,
    run_dir: Option<&Path>,
    workdir: Option<&Path>,
    checkpoint_path: Option<&Path>,
    plan: bool,
) -> Result<ResolvedReportWorkflow> {
    let cwd = std::env::current_dir().context("cannot determine current directory")?;
    let discovery_dir: Option<PathBuf> = match (&workflow, run_dir) {
        (Some(_), _) => None,
        (None, Some(dir)) => Some(dir.to_path_buf()),
        (None, None) => Some(match workdir {
            Some(dir) => dir.to_path_buf(),
            None => cwd,
        }),
    };

    let mut checkpoint: Option<oxo_flow_core::executor::CheckpointState> = None;
    let mut discovered_checkpoint_path: Option<PathBuf> = None;

    let workflow = match workflow {
        Some(path) => path,
        None => {
            let discovery_dir = discovery_dir
                .as_ref()
                .expect("None only for explicit workflows");
            let disc_cp = checkpoint_path
                .map(Path::to_path_buf)
                .unwrap_or_else(|| discovery_dir.join(".oxo-flow").join("checkpoint.json"));
            // --plan ignores execution data: no checkpoint load, straight
            // to workflow discovery.
            let loaded = if plan {
                None
            } else {
                oxo_flow_core::executor::CheckpointState::load_from_file(&disc_cp).ok()
            };
            match loaded {
                Some(cp) => {
                    // Carry the loaded checkpoint forward even when it
                    // cannot pin the workflow (missing workflow_path) —
                    // the reporting phase must not reload or double-warn.
                    discovered_checkpoint_path = Some(disc_cp);
                    if let Some(wp) = cp.workflow_path.clone()
                        && Path::new(&wp).is_file()
                    {
                        checkpoint = Some(cp);
                        PathBuf::from(wp)
                    } else {
                        checkpoint = Some(cp);
                        discover_report_workflow_in(discovery_dir)?
                    }
                }
                None => discover_report_workflow_in(discovery_dir)?,
            }
        }
    };

    Ok(ResolvedReportWorkflow {
        workflow,
        discovery_dir,
        checkpoint,
        checkpoint_path: discovered_checkpoint_path,
    })
}

/// Resolve a `[report].template` entry and render the report through it.
///
/// `"report.html"` selects the built-in Tera template; anything else is a
/// template file path, resolved relative to the workflow's directory first
/// (a template next to the workflow works from any cwd), then the process
/// cwd. Files must be UTF-8 text.
///
/// The template is registered under a name with an autoescape-suffixed
/// extension so Tera escapes `{{ variables }}` exactly like the built-in
/// template — a hostile workflow name must not render raw HTML into a
/// shareable report. Any failure propagates for the caller to fall back on.
fn render_with_custom_template(
    template: &str,
    report: &oxo_flow_core::report::Report,
    workflow_dir: &Path,
) -> Result<String> {
    let mut engine = TemplateEngine::new()?;
    if template == "report.html" {
        Ok(engine.render_report(report)?)
    } else {
        let path = Path::new(template);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            let next_to_workflow = workflow_dir.join(path);
            if next_to_workflow.is_file() {
                next_to_workflow
            } else {
                path.to_path_buf()
            }
        };
        let content = std::fs::read_to_string(&resolved)
            .with_context(|| format!("failed to read template {}", resolved.display()))?;
        // Tera autoescapes only names ending in .html/.htm/.xml; the
        // --init-template scaffold is report-template.tera, so fall back
        // to "custom.html" unless the file name already carries an
        // autoescape suffix.
        let name = resolved
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| n.ends_with(".html") || n.ends_with(".htm") || n.ends_with(".xml"))
            .map(str::to_string)
            .unwrap_or_else(|| "custom.html".to_string());
        engine.add_template(&name, &content)?;
        Ok(engine.render_with_template(&name, report)?)
    }
}

/// Render `[report].template` for the effective output format (issue #83
/// P0-9).
///
/// Templates apply to HTML output only: with `-f json`/`-f md` (or pdf)
/// the template is skipped with a stderr note — never rendered, so a
/// broken template cannot warn or exit 2 (under --strict) for a format
/// that would not use it. Render failures fall back to the default
/// renderer, or exit 2 under --strict.
fn render_template_for_format(
    config: &WorkflowConfig,
    format: &str,
    strict: bool,
    report: &oxo_flow_core::report::Report,
    workflow: &Path,
) -> Option<String> {
    let template = config.report.as_ref().and_then(|r| r.template.clone())?;
    if !matches!(format, "html" | "htm") {
        eprintln!(
            "  {} template applies to HTML output only",
            "Note:".yellow()
        );
        return None;
    }
    match render_with_custom_template(&template, report, oxo_flow_core::parent_dir(workflow)) {
        Ok(html) => Some(html),
        Err(e) => {
            eprintln!(
                "  {} template render failed — falling back to the default renderer: {e}",
                "⚠".yellow()
            );
            if strict {
                std::process::exit(2);
            }
            None
        }
    }
}

pub async fn handle_report(args: ReportArgs) -> Result<()> {
    let ReportArgs {
        workflow,
        format,
        output,
        checkpoint_path,
        ai,
        workdir,
        ci,
        no_timestamps,
        strict,
        list_sections,
        run_dir,
        failed,
        plan,
        init_template,
        list_templates,
        r_data,
        diff,
        acct,
    } = args;

    print_banner();

    // --list-sections: enumerate the registry and exit (issue #83 P2-7).
    // Before workflow resolution/discovery — listing sections needs no
    // workflow file, so it must succeed in an empty directory too.
    if list_sections {
        let registry = oxo_flow_core::report::SectionRegistry::with_defaults();
        for (name, description) in registry.sections() {
            println!("{name:<24} {description}");
        }
        return Ok(());
    }

    // --list-templates: enumerate available templates and exit (issue #83
    // P2-7). Early return — no workflow needed.
    if list_templates {
        println!("report.html  built-in default");
        eprintln!(
            "  Custom templates load from [report].template — \"report.html\" (built-in) or a \
             template file path resolved relative to the workflow file."
        );
        return Ok(());
    }

    // --init-template: scaffold the built-in Tera template (issue #83
    // P2-7). Refuses to overwrite an existing file. Early return.
    if init_template {
        let path = Path::new("report-template.tera");
        if path.exists() {
            anyhow::bail!(
                "{} already exists — refusing to overwrite. Move it aside or delete it first.",
                path.display()
            );
        }
        std::fs::write(path, oxo_flow_core::report::builtin_template())?;
        println!("{}", path.display());
        return Ok(());
    }

    // ── Workflow resolution ─────────────────────────────────────────────
    // Explicit WORKFLOW wins; otherwise auto-discovery (issue #83 WS5) —
    // see resolve_report_workflow.
    let resolved = resolve_report_workflow(
        workflow,
        run_dir.as_deref(),
        workdir.as_deref(),
        checkpoint_path.as_deref(),
        plan,
    )?;
    let discovery_dir = resolved.discovery_dir;
    let workflow = resolved.workflow;
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // [report].format is parsed but not consumed — say so instead of
    // silently ignoring user configuration (issue #83 P0-9). [report].
    // template IS consumed now (see below).
    if let Some(report_cfg) = &config.report
        && !report_cfg.format.is_empty()
    {
        let msg = "[report].format is declared in the workflow but not supported yet — \
                   use -f to select the output format";
        if strict {
            anyhow::bail!(msg);
        }
        eprintln!("  {} {}", "⚠".yellow(), msg);
    }

    // Format: explicit -f wins; otherwise infer from the -o extension;
    // otherwise html (issue #83 P2-2).
    let format = match format {
        Some(f) => f,
        None => match output
            .as_deref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
        {
            Some("json") => "json".to_string(),
            Some("md") | Some("markdown") => "md".to_string(),
            Some("pdf") => "pdf".to_string(),
            _ => "html".to_string(),
        },
    };

    // Determine checkpoint path: discovery-loaded > explicit --checkpoint >
    // base-relative. The base is --workdir when given (even with an
    // explicit WORKFLOW — issue #68 semantics), else the auto-discovered
    // directory, else the workflow's directory (issue #83 WS5).
    let checkpoint_path = resolved.checkpoint_path.unwrap_or_else(|| {
        checkpoint_path.unwrap_or_else(|| {
            let base = workdir
                .clone()
                .or(discovery_dir.clone())
                .unwrap_or_else(|| oxo_flow_core::parent_dir(&workflow).to_path_buf());
            base.join(".oxo-flow").join("checkpoint.json")
        })
    });

    // --plan skips execution data entirely (checkpoint = None): the UNRUN
    // dashboard is the honest representation, and no "no checkpoint"
    // warning is printed. Discovery may have already loaded it.
    let checkpoint = if plan {
        None
    } else if resolved.checkpoint.is_some() {
        resolved.checkpoint
    } else {
        load_checkpoint(&checkpoint_path, strict)?
    };

    // AI result interpretation: plain-language summary of outcomes,
    // caveats, and next steps. It goes to stderr (never pollutes the
    // stdout pipeline) AND into the report as a marked section. A missing
    // provider degrades to the standard report instead of aborting
    // (issue #83 P1-2).
    let mut ai_section: Option<ReportSection> = None;
    if let Some(provider) = crate::commands::ai_template::try_resolve_ai(Some(&workflow), ai) {
        // A failed AI call must not cost the user their report: warn and
        // fall back to the standard report.
        match interpret_report_with_ai(&workflow, &config, checkpoint.as_ref(), &provider).await {
            Ok((model, text)) => {
                eprintln!("{}", "AI Result Interpretation".bold().green().underline());
                eprintln!("  Model: {model}\n");
                eprintln!("{text}");
                ai_section = Some(ReportSection {
                    title: "AI Interpretation".into(),
                    id: "ai-interpretation".into(),
                    content: ReportContent::Markdown { markdown: text },
                    subsections: vec![ReportSection {
                        title: "About This Section".into(),
                        id: "ai-about".into(),
                        content: ReportContent::Text {
                            text: format!(
                                "Generated by an AI model ({model}). Machine-generated \
                                 content — review it before relying on it."
                            ),
                        },
                        subsections: vec![],
                    }],
                });
            }
            Err(e) => eprintln!(
                "  {} AI interpretation failed — continuing with the standard report: {e}",
                "⚠".yellow()
            ),
        }
    } else if ai {
        eprintln!(
            "  {} --ai requested but no AI provider is configured (OXO_FLOW_AI_PROVIDER) — \
             generating the standard report.",
            "⚠".yellow()
        );
    }

    // ── R-friendly TSV exports (issue #83 P1-15) ─────────────────────────
    // Additional output, orthogonal to -f: a sample table and a per-rule
    // metrics table for downstream R analysis.
    if let Some(ref dir) = r_data {
        write_r_data(dir, &config, checkpoint.as_ref())?;
    }

    // ── Checkpoint diff (issue #83 P1-6) ─────────────────────────────────
    // Model-level diff against another checkpoint, printed to stderr so the
    // stdout report pipe stays clean. Both checkpoints must exist — a diff
    // without both sides is a usage error (exit 1), not a report.
    if let Some(ref other_path) = diff {
        let other = CheckpointState::load_from_file(other_path).with_context(|| {
            format!(
                "cannot load checkpoint for --diff: {} \
                 (a diff needs both this report's checkpoint and the one passed to --diff)",
                other_path.display()
            )
        })?;
        let current = match &checkpoint {
            Some(cp) => cp,
            None => {
                anyhow::bail!(
                    "no checkpoint found at {} — a diff needs this report's checkpoint AND the \
                     one passed to --diff. Run the workflow first.",
                    checkpoint_path.display()
                );
            }
        };
        print_checkpoint_diff(current, &checkpoint_path, &other, other_path);
    }

    // ── sacct resource accounting import (issue #83 P1-13) ───────────────
    // Parse the CSV up front (fail fast on a malformed file); the section
    // itself is appended to the report after the standard build.
    let acct_section = if let Some(ref csv_path) = acct {
        let rows = parse_acct_csv(csv_path)?;
        let rule_names: Vec<String> = config.rules.iter().map(|r| r.name.clone()).collect();
        let merged = merge_acct_rows(rows, &rule_names);
        Some(resource_accounting_section(&merged, &rule_names, csv_path))
    } else {
        None
    };

    // ── Build report using the pluggable section system ──
    // Domain auto-detection tailors sections to the workflow type.
    // Users can override via [report].sections in their .oxoflow file.
    let generated_at = if no_timestamps {
        None
    } else if ci {
        Some(pinned_timestamp())
    } else {
        Some(Utc::now())
    };

    let mut report = build_report(
        &config,
        checkpoint.as_ref(),
        Some(checkpoint_path.as_path()),
        workflow.as_path(),
        generated_at,
    );

    // Additional user-requested sections (post-build, like --ai): the AI
    // interpretation (--ai) and the sacct resource-accounting import
    // (--acct). Never filtered by [report].sections — explicitly asked for.
    if let Some(ai_sec) = ai_section {
        report.add_section(ai_sec);
    }
    if let Some(acct_sec) = acct_section {
        report.add_section(acct_sec);
    }

    // --failed: failure diagnosis is the first screen (issue #83 P2-5).
    // Stable order otherwise; with no checkpoint or no failures the section
    // does not exist and the order is unchanged.
    if failed
        && let Some(pos) = report
            .sections
            .iter()
            .position(|s| s.id == "failure-diagnosis")
    {
        let diagnosis = report.sections.remove(pos);
        report.sections.insert(0, diagnosis);
    }

    // ── Custom template (issue #83 P0-9) ────────────────────────────────
    // [report].template applies to HTML output only: for json/md/pdf it is
    // skipped with a stderr note (never rendered, so a broken template
    // cannot fail a format that would not use it); for html the rendered
    // string replaces to_html(), falling back on error (exit 2 under
    // --strict). See render_template_for_format.
    let template_html = render_template_for_format(&config, &format, strict, &report, &workflow);

    let content = match format.as_str() {
        "html" | "htm" => template_html.unwrap_or_else(|| report.to_html()),
        "json" => report.to_json().map_err(|e| anyhow::anyhow!(e))?,
        "md" | "markdown" => report.to_markdown(),
        "pdf" => {
            let pdf_output = output
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("{}_report.pdf", config.workflow.name)));
            if wkhtmltopdf_available() {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(async { report.to_pdf(&pdf_output).await })?;
                eprintln!(
                    "{} PDF report written to {}",
                    "✓".green(),
                    pdf_output.display()
                );
            } else {
                // wkhtmltopdf's upstream is archived; when it is absent,
                // degrade to a printable HTML file instead of failing
                // (issue #83 P1-7).
                let fallback = pdf_output.with_extension("html");
                std::fs::write(&fallback, report.to_printable_html())?;
                eprintln!(
                    "{} wkhtmltopdf not found — wrote printable HTML to {} instead. \
                     Install wkhtmltopdf (note: upstream archived) for PDF output.",
                    "⚠".yellow(),
                    fallback.display()
                );
            }
            return Ok(());
        }
        "pdf-command" => {
            let pdf_output = output
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("{}_report.pdf", config.workflow.name)));
            println!(
                "{}",
                report.to_pdf_command(&pdf_output.to_string_lossy(), vec![])
            );
            return Ok(());
        }
        other => anyhow::bail!(
            "unsupported report format: '{}'. Supported formats: html, json, md, pdf, pdf-command",
            other
        ),
    };

    match output {
        // "-o -" targets stdout explicitly (issue #83 P2-2).
        Some(path) if path.as_os_str() == "-" => {
            println!("{content}");
        }
        Some(path) => {
            std::fs::write(&path, &content)?;
            eprintln!("Report written to {}", path.display());
        }
        None => match &discovery_dir {
            // Auto-discovered workflow (zero-arg or --run): write the report
            // next to the checkpoint instead of dumping HTML to stdout
            // (issue #83 WS5). The timestamp is deterministic enough — the
            // CLI has no run-id concept.
            Some(dir) => {
                let reports_dir = dir.join(".oxo-flow").join("reports");
                std::fs::create_dir_all(&reports_dir)?;
                let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                let path = reports_dir.join(format!("report-{stamp}.html"));
                std::fs::write(&path, &content)?;
                eprintln!("Report written to {}", path.display());
            }
            None => {
                println!("{content}");
            }
        },
    };

    Ok(())
}

// ── Report construction (shared by `report` and the run snapshot) ───────────

/// Build a report from workflow + checkpoint state.
///
/// Shared by `handle_report` and [`snapshot_report`] (issue #83 P1-14):
/// domain detection, registry generation honoring `[report].sections`, the
/// fluent builder, and provenance wiring. The `generated_at` timestamp is a
/// parameter so callers control reproducibility (`--ci` / `--no-timestamps`
/// / snapshot-now).
pub fn build_report(
    config: &WorkflowConfig,
    checkpoint: Option<&CheckpointState>,
    checkpoint_path: Option<&Path>,
    workflow_path: &Path,
    generated_at: Option<DateTime<Utc>>,
) -> Report {
    use oxo_flow_core::report::{ReportBuilder, ReportContext, SectionRegistry, WorkflowDomain};

    let domain = WorkflowDomain::detect(&config.rules);
    let ctx = ReportContext {
        config,
        checkpoint,
        domain,
        workflow_path: Some(workflow_path),
        checkpoint_path,
    };

    // Resolve section filter from [report].sections config (if present).
    let section_filter: Option<HashSet<String>> = config.report.as_ref().and_then(|r| {
        let sections = &r.sections;
        if sections.is_empty() {
            None
        } else {
            Some(sections.iter().cloned().collect())
        }
    });

    let registry = SectionRegistry::with_defaults();
    let sections = registry.generate(&ctx, section_filter.as_ref());

    let mut report = ReportBuilder::new(
        &format!("{} Report", config.workflow.name),
        &config.workflow.name,
        &config.workflow.version,
    )
    .workflow_path(Some(workflow_path.display().to_string()))
    .checkpoint_path(
        checkpoint
            .is_some()
            .then(|| checkpoint_path.map(|p| p.display().to_string()))
            .flatten(),
    )
    .workflow_git_sha(checkpoint.and_then(|c| c.workflow_git_sha.clone()))
    .generated_at(generated_at);
    for section in sections {
        report = report.section(section);
    }
    report.build()
}

/// Snapshot the final checkpoint as a JSON report (issue #83 P1-14).
///
/// Writes `<workdir>/.oxo-flow/reports/report-<UTC yyyyMMdd-HHmmss>.json`
/// (a `-N` suffix when the same second is taken — two runs cannot collide
/// under the workdir lock, but sequential runs can) and appends
/// `{generated_at, workflow, checkpoint, report}` to `index.json` — a JSON
/// array, created when absent, read-modify-written, sorted by generated_at.
///
/// Callers treat errors as warnings: a reporting hiccup must never fail a
/// run. Prints `✓ Report snapshot: <path>` to stderr.
pub fn snapshot_report(
    workflow_path: &Path,
    workdir: &Path,
    checkpoint: &CheckpointState,
) -> Result<PathBuf> {
    let config = WorkflowConfig::from_file(workflow_path)
        .with_context(|| format!("failed to parse {}", workflow_path.display()))?;
    let checkpoint_path = workdir.join(".oxo-flow").join("checkpoint.json");
    let generated_at = Utc::now();
    let report = build_report(
        &config,
        Some(checkpoint),
        Some(&checkpoint_path),
        workflow_path,
        Some(generated_at),
    );
    let json = report.to_json().map_err(|e| anyhow::anyhow!(e))?;

    let reports_dir = workdir.join(".oxo-flow").join("reports");
    std::fs::create_dir_all(&reports_dir)?;

    let stamp = generated_at.format("%Y%m%d-%H%M%S");
    let mut path = reports_dir.join(format!("report-{stamp}.json"));
    let mut suffix = 1;
    while path.exists() {
        path = reports_dir.join(format!("report-{stamp}-{suffix}.json"));
        suffix += 1;
    }
    std::fs::write(&path, &json)?;

    // Index: JSON array of {generated_at, workflow, checkpoint, report},
    // kept sorted by generated_at so consumers can take the last entry as
    // the newest snapshot.
    let index_path = reports_dir.join("index.json");
    let mut entries: Vec<serde_json::Value> = if index_path.exists() {
        let raw = std::fs::read_to_string(&index_path)?;
        serde_json::from_str(&raw).with_context(|| {
            format!(
                "report index {} is not a JSON array — refusing to overwrite it",
                index_path.display()
            )
        })?
    } else {
        Vec::new()
    };
    let entry = serde_json::json!({
        "generated_at": generated_at.to_rfc3339(),
        "workflow": workflow_path.display().to_string(),
        "checkpoint": checkpoint_path.display().to_string(),
        "report": path.file_name().map(|f| f.to_string_lossy().to_string()),
    });
    entries.push(entry);
    entries.sort_by(|a, b| {
        // RFC3339 UTC strings sort lexicographically in chronological order
        // (fixed-width date/time fields; fractional seconds compare digit by
        // digit with equal-length integer parts). Unparseable entries sort
        // first — they predate the format, roughly.
        a["generated_at"]
            .as_str()
            .unwrap_or("")
            .cmp(b["generated_at"].as_str().unwrap_or(""))
    });
    std::fs::write(&index_path, serde_json::to_string_pretty(&entries)?)?;

    eprintln!("{} Report snapshot: {}", "✓".green(), path.display());
    Ok(path)
}

// ── R-friendly TSV exports (issue #83 P1-15) ─────────────────────────────────

/// Replace TSV-hostile characters (tabs, newlines, carriage returns) with
/// spaces so a rule/sample name can never break the column layout.
fn sanitize_tsv(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

/// Print the per-file confirmation line for `--r-data` exports.
fn announce_r_data_written(path: &Path) {
    eprintln!("  {} R data written to {}", "✓".green(), path.display());
}

/// Write `sample_table.tsv` (sample → group) and `metrics.tsv` (rule →
/// wall/memory/status) into `dir` for downstream R analysis. Headers first,
/// deterministic order, one path per line on stderr. With no checkpoint the
/// files carry headers only and a stderr note explains why.
fn write_r_data(
    dir: &Path,
    config: &WorkflowConfig,
    checkpoint: Option<&CheckpointState>,
) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;

    // sample_table.tsv — sample_groups plus pairs (a pair's group label is
    // its experiment_type when present, else "-"; pairs have no group).
    let mut rows: Vec<(String, String)> = Vec::new();
    for group in &config.sample_groups {
        for sample in &group.samples {
            rows.push((sample.clone(), group.name.clone()));
        }
    }
    for pair in &config.pairs {
        let group = pair
            .experiment_type
            .clone()
            .unwrap_or_else(|| "-".to_string());
        rows.push((pair.experiment.clone(), group.clone()));
        if let Some(control) = &pair.control {
            rows.push((control.clone(), group.clone()));
        }
    }
    // Deterministic: group first, then sample name; exact duplicates from
    // overlapping group/pair definitions collapse.
    rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    rows.dedup();

    let mut sample_table = String::from("sample\tgroup\n");
    for (sample, group) in &rows {
        sample_table.push_str(&format!(
            "{}\t{}\n",
            sanitize_tsv(sample),
            sanitize_tsv(group)
        ));
    }
    let sample_path = dir.join("sample_table.tsv");
    std::fs::write(&sample_path, &sample_table)?;
    announce_r_data_written(&sample_path);

    // metrics.tsv — one row per benchmark record, plus failed rules that
    // have no benchmark (wall/memory "-"), sorted by rule.
    let mut metric_rows: Vec<(String, String, String, String)> = Vec::new();
    if let Some(cp) = checkpoint {
        let mut names: Vec<&String> = cp.benchmarks.keys().collect();
        names.extend(
            cp.failed_rules
                .iter()
                .filter(|r| !cp.benchmarks.contains_key(*r)),
        );
        names.sort_unstable();
        names.dedup();
        for name in names {
            let (wall, mem) = match cp.benchmarks.get(name) {
                Some(b) => (
                    b.wall_time_secs.to_string(),
                    b.max_memory_mb
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
                None => ("-".to_string(), "-".to_string()),
            };
            let status = if cp.completed_rules.contains(name) {
                "success"
            } else if cp.failed_rules.contains(name) {
                "failed"
            } else {
                "-"
            };
            metric_rows.push((name.clone(), wall, mem, status.to_string()));
        }
    }

    let mut metrics = String::from("rule\twall_time_secs\tmax_memory_mb\tstatus\n");
    for (rule, wall, mem, status) in &metric_rows {
        metrics.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            sanitize_tsv(rule),
            wall,
            mem,
            status
        ));
    }
    let metrics_path = dir.join("metrics.tsv");
    std::fs::write(&metrics_path, &metrics)?;
    announce_r_data_written(&metrics_path);

    if checkpoint.is_none() {
        eprintln!(
            "  {} No checkpoint found — TSV files contain headers only",
            "Note:".yellow()
        );
    } else if metric_rows.is_empty() {
        eprintln!(
            "  {} No execution metrics in the checkpoint — metrics.tsv contains headers only",
            "Note:".yellow()
        );
    }

    Ok(())
}

// ── Checkpoint diff (issue #83 P1-6) ─────────────────────────────────────────

/// Sorted, deduplicated union of checkpoint rule-name sets — the iteration
/// order for every diff axis (deterministic output).
fn sorted_union(sets: &[&HashSet<String>]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for set in sets {
        names.extend(set.iter().cloned());
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// Sorted, deduplicated union of map keys — the iteration order for the
/// checksum diff axis (deterministic output).
fn sorted_map_keys(maps: &[&HashMap<String, String>]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for map in maps {
        keys.extend(map.keys().cloned());
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Abbreviate a `sha256:<hex>` checksum for diff lines.
fn short_checksum(value: &str) -> String {
    let bare = value.strip_prefix("sha256:").unwrap_or(value);
    let end = bare.len().min(12);
    bare[..end].to_string()
}

/// Model-level diff of a checkpoint against another, printed to stderr with
/// terminal highlighting (green + / red - / yellow ~, like `handle_diff`).
/// stdout stays the report's pipe; the exit code stays 0 — a diff is
/// information, not failure.
fn print_checkpoint_diff(
    current: &CheckpointState,
    current_path: &Path,
    other: &CheckpointState,
    other_path: &Path,
) {
    let mut lines: Vec<(char, String)> = Vec::new();

    // Completed / failed rule membership changes (other → current).
    for rule in sorted_union(&[&current.completed_rules, &other.completed_rules]) {
        match (
            current.completed_rules.contains(&rule),
            other.completed_rules.contains(&rule),
        ) {
            (true, false) => lines.push(('+', format!("completed: {rule}"))),
            (false, true) => lines.push(('-', format!("completed: {rule}"))),
            _ => {}
        }
    }
    for rule in sorted_union(&[&current.failed_rules, &other.failed_rules]) {
        match (
            current.failed_rules.contains(&rule),
            other.failed_rules.contains(&rule),
        ) {
            (true, false) => lines.push(('+', format!("failed: {rule}"))),
            (false, true) => lines.push(('-', format!("failed: {rule}"))),
            _ => {}
        }
    }

    // Status flips: completed ↔ failed between the two checkpoints.
    for rule in sorted_union(&[
        &current.completed_rules,
        &current.failed_rules,
        &other.completed_rules,
        &other.failed_rules,
    ]) {
        let now_done = current.completed_rules.contains(&rule);
        let now_failed = current.failed_rules.contains(&rule);
        let before_done = other.completed_rules.contains(&rule);
        let before_failed = other.failed_rules.contains(&rule);
        match (before_done, before_failed, now_done, now_failed) {
            (true, _, false, true) => lines.push(('~', format!("{rule}: completed → failed"))),
            (_, true, true, false) => lines.push(('~', format!("{rule}: failed → completed"))),
            _ => {}
        }
    }

    // Benchmark deltas for rules present in BOTH checkpoints (old → new).
    let mut bench_names: Vec<String> = current
        .benchmarks
        .keys()
        .filter(|n| other.benchmarks.contains_key(*n))
        .cloned()
        .collect();
    bench_names.sort_unstable();
    for name in bench_names {
        let now = &current.benchmarks[&name];
        let before = &other.benchmarks[&name];
        if now.wall_time_secs != before.wall_time_secs {
            lines.push((
                '~',
                format!(
                    "benchmark {name}: wall time {:.2}s → {:.2}s",
                    before.wall_time_secs, now.wall_time_secs
                ),
            ));
        }
        if now.max_memory_mb != before.max_memory_mb {
            let fmt = |m: Option<u64>| {
                m.map(|v| format!("{v}MB"))
                    .unwrap_or_else(|| "-".to_string())
            };
            lines.push((
                '~',
                format!(
                    "benchmark {name}: memory {} → {}",
                    fmt(before.max_memory_mb),
                    fmt(now.max_memory_mb)
                ),
            ));
        }
    }

    // Output checksum changes (added / removed / changed).
    for path in sorted_map_keys(&[&current.checksums, &other.checksums]) {
        match (current.checksums.get(&path), other.checksums.get(&path)) {
            (Some(_), None) => lines.push(('+', format!("checksum: {path}"))),
            (None, Some(_)) => lines.push(('-', format!("checksum: {path}"))),
            (Some(a), Some(b)) if a != b => lines.push((
                '~',
                format!(
                    "checksum: {path} {} → {}",
                    short_checksum(b),
                    short_checksum(a)
                ),
            )),
            _ => {}
        }
    }

    if lines.is_empty() {
        eprintln!("{} Checkpoints are identical", "✓".green().bold());
        return;
    }
    eprintln!(
        "{} {} difference(s) between {} and {}:",
        "Diff:".bold().yellow(),
        lines.len(),
        current_path.display(),
        other_path.display()
    );
    for (marker, text) in lines {
        let colored = match marker {
            '+' => format!("{} {}", "+".green(), text),
            '-' => format!("{} {}", "-".red(), text),
            _ => format!("{} {}", "~".yellow(), text),
        };
        eprintln!("  {colored}");
    }
}

// ── sacct CSV accounting import (issue #83 P1-13) ────────────────────────────

/// One parsed sacct row, matched to a workflow rule by JobName.
struct AcctRow {
    rule: String,
    jobid: String,
    state: String,
    elapsed_secs: Option<f64>,
    cpu_secs: Option<f64>,
    max_rss_mb: Option<u64>,
}

/// Parse an sacct Elapsed/CPUTime string to seconds.
///
/// Accepted forms (from `sacct -o Elapsed,CPUTime`): `"MM:SS"`,
/// `"HH:MM:SS"`, `"[D-]HH:MM:SS"`, each optionally with a fractional tail
/// (`"00:00:01.5"`). A bare number is treated as seconds.
fn parse_hms_to_secs(value: &str) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (days, rest) = match value.split_once('-') {
        Some((d, r)) => (d.trim().parse::<u64>().ok()?, r),
        None => (0u64, value),
    };
    let (clock, frac) = match rest.split_once('.') {
        Some((c, f)) => (c, f.parse::<f64>().ok()? / 10f64.powi(f.len() as i32)),
        None => (rest, 0.0),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    let secs: f64 = match parts.len() {
        1 => parts[0].parse::<u64>().ok()? as f64,
        2 => (parts[0].parse::<u64>().ok()? * 60 + parts[1].parse::<u64>().ok()?) as f64,
        3 => {
            (parts[0].parse::<u64>().ok()? * 3600
                + parts[1].parse::<u64>().ok()? * 60
                + parts[2].parse::<u64>().ok()?) as f64
        }
        _ => return None,
    };
    Some(secs + frac + days as f64 * 86_400.0)
}

/// Parse an sacct MaxRSS value to MB.
///
/// Accepted forms: `"2048K"`, `"512M"`, `"1.5G"`, `"12345c"` (bytes), or a
/// bare number (bytes); `T` (tebibytes) is also accepted as a harmless
/// extension beyond sacct's documented K/M/G output. Suffixes are
/// case-insensitive and base-1024; the result rounds UP so the table never
/// under-reports a peak.
fn parse_maxrss_to_mb(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (num, mult) = match value.as_bytes().last().copied() {
        Some(b) if b.is_ascii_alphabetic() => {
            let (num, suffix) = value.split_at(value.len() - 1);
            let mult = match suffix.to_ascii_lowercase().as_str() {
                "k" => 1024u64,
                "m" => 1024u64.pow(2),
                "g" => 1024u64.pow(3),
                "t" => 1024u64.pow(4),
                "c" => 1u64,
                _ => return None,
            };
            (num, mult)
        }
        _ => (value, 1u64),
    };
    let bytes = num.trim().parse::<f64>().ok()? * mult as f64;
    Some((bytes / (1024.0 * 1024.0)).ceil() as u64)
}

/// Format seconds compactly: whole numbers drop the decimal tail.
fn format_secs(secs: f64) -> String {
    if secs.fract() == 0.0 {
        format!("{secs:.0}")
    } else {
        format!("{secs:.2}")
    }
}

/// Parse an sacct-style CSV into accounting rows.
///
/// Column detection is header-based and case-insensitive (jobname, state,
/// elapsed, cputime, maxrss; jobid optional — used only to prefer the batch
/// row over step rows when a JobName repeats). A missing required column is
/// a hard error: silently dropping a column would fake a complete table.
fn parse_acct_csv(path: &Path) -> Result<Vec<AcctRow>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .with_context(|| format!("failed to read sacct CSV {}", path.display()))?;

    let headers: Vec<String> = reader
        .headers()
        .with_context(|| format!("sacct CSV {} has no header row", path.display()))?
        .iter()
        .map(|h| h.trim().to_lowercase())
        .collect();
    let col = |name: &str| headers.iter().position(|h| h == name);

    let required = ["jobname", "state", "elapsed", "cputime", "maxrss"];
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|c| col(c).is_none())
        .collect();
    if !missing.is_empty() {
        anyhow::bail!(
            "sacct CSV {} is missing required column(s): {} — found headers: {}",
            path.display(),
            missing.join(", "),
            headers.join(", ")
        );
    }

    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record?;
        // jobid is optional — col() is None for it, so the closure must
        // degrade gracefully; required columns were checked above.
        let get = |name: &str| col(name).and_then(|i| record.get(i));
        let rule = get("jobname").unwrap_or("").trim().to_string();
        if rule.is_empty() {
            continue;
        }
        rows.push(AcctRow {
            rule,
            jobid: get("jobid").unwrap_or("").trim().to_string(),
            state: get("state").unwrap_or("").trim().to_string(),
            elapsed_secs: get("elapsed").and_then(parse_hms_to_secs),
            cpu_secs: get("cputime").and_then(parse_hms_to_secs),
            max_rss_mb: get("maxrss").and_then(parse_maxrss_to_mb),
        });
    }
    Ok(rows)
}

/// Merge sacct rows with the workflow's rules: one row per workflow rule
/// that has a record, sorted by rule name, preferring the batch row (JobID
/// without a step separator — `.` or `_`) over step rows.
fn merge_acct_rows(rows: Vec<AcctRow>, rule_names: &[String]) -> Vec<AcctRow> {
    let mut by_name: HashMap<String, Vec<AcctRow>> = HashMap::new();
    for row in rows {
        by_name.entry(row.rule.clone()).or_default().push(row);
    }
    let mut names: Vec<String> = rule_names.to_vec();
    names.sort_unstable();
    let mut merged: Vec<AcctRow> = Vec::new();
    for name in names {
        let Some(mut candidates) = by_name.remove(&name) else {
            continue; // no accounting record for this rule — omitted
        };
        // Stable sort: batch rows (JobID without a step separator — dot or
        // underscore — in it) first, file order kept; array-task IDs like
        // `12345_0` sort as step rows, matching sacct's own view.
        candidates.sort_by_key(|r| r.jobid.contains(['.', '_']));
        merged.push(candidates.into_iter().next().expect("non-empty group"));
    }
    merged
}

/// Build the "Resource Accounting" report section from merged sacct rows:
/// a [Rule, State, Elapsed, CPU Time, Max RSS (MB)] table sorted by rule,
/// plus import provenance and an honest coverage note. User-provided data —
/// every value travels through the normal table path and is escaped at
/// render time.
fn resource_accounting_section(
    rows: &[AcctRow],
    rule_names: &[String],
    source: &Path,
) -> ReportSection {
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.rule.clone(),
                r.state.clone(),
                r.elapsed_secs
                    .map(format_secs)
                    .unwrap_or_else(|| "-".to_string()),
                r.cpu_secs
                    .map(format_secs)
                    .unwrap_or_else(|| "-".to_string()),
                r.max_rss_mb
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();

    let no_record: Vec<&String> = rule_names
        .iter()
        .filter(|r| !rows.iter().any(|row| &row.rule == *r))
        .collect();

    let mut subsections = vec![ReportSection {
        title: "Import".into(),
        id: "resource-accounting-import".into(),
        content: ReportContent::Text {
            text: format!("Imported from sacct CSV: {}", source.display()),
        },
        subsections: vec![],
    }];
    if !no_record.is_empty() {
        subsections.push(ReportSection {
            title: "Coverage".into(),
            id: "resource-accounting-coverage".into(),
            content: ReportContent::Text {
                text: format!(
                    "{} rules had no accounting record: {}",
                    no_record.len(),
                    no_record
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
            subsections: vec![],
        });
    }

    ReportSection {
        title: "Resource Accounting".into(),
        id: "resource-accounting".into(),
        content: ReportContent::Table {
            headers: vec![
                "Rule".into(),
                "State".into(),
                "Elapsed".into(),
                "CPU Time".into(),
                "Max RSS (MB)".into(),
            ],
            rows: table_rows,
        },
        subsections,
    }
}

/// Reproducible-build timestamp: `SOURCE_DATE_EPOCH` when set, otherwise
/// the Unix epoch (issue #83 P1-4).
fn pinned_timestamp() -> chrono::DateTime<chrono::Utc> {
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH")
        && let Ok(secs) = epoch.parse::<i64>()
        && let Some(ts) = chrono::DateTime::from_timestamp(secs, 0)
    {
        return ts;
    }
    chrono::DateTime::from_timestamp(0, 0).expect("unix epoch is representable")
}

/// Whether the wkhtmltopdf binary is present and runnable.
fn wkhtmltopdf_available() -> bool {
    std::process::Command::new("wkhtmltopdf")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn handle_diff(workflow_a: PathBuf, workflow_b: PathBuf) -> Result<()> {
    print_banner();
    let config_a = WorkflowConfig::from_file(&workflow_a)
        .with_context(|| format!("failed to parse {}", workflow_a.display()))?;
    let config_b = WorkflowConfig::from_file(&workflow_b)
        .with_context(|| format!("failed to parse {}", workflow_b.display()))?;

    let diffs = oxo_flow_core::format::diff_workflows(&config_a, &config_b);

    if diffs.is_empty() {
        eprintln!("{} Workflows are identical", "✓".green().bold());
    } else {
        eprintln!(
            "{} {} difference(s) between {} and {}:",
            "Diff:".bold().yellow(),
            diffs.len(),
            workflow_a.display(),
            workflow_b.display()
        );
        for diff in &diffs {
            let cat_color = match diff.category.as_str() {
                "added" | "rule added" => "✓".green(),
                "removed" | "rule removed" => "✗".red(),
                "changed" => "~".yellow(),
                _ => "•".cyan(),
            };
            eprintln!(
                "  {} [{}] {}",
                cat_color,
                diff.category.cyan(),
                diff.description
            );
        }
    }
    Ok(())
}

pub fn handle_export(workflow: PathBuf, format: String, output: Option<PathBuf>) -> Result<()> {
    print_banner();
    let workflow = resolve_workflow(Some(workflow))?;
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    let content = match format.as_str() {
        "docker" => {
            let pkg = oxo_flow_core::container::PackageConfig::default();
            oxo_flow_core::container::generate_dockerfile(&config, &pkg)
                .map_err(|e| anyhow::anyhow!(e))?
        }
        "singularity" => {
            let pkg = oxo_flow_core::container::PackageConfig {
                format: oxo_flow_core::container::ContainerFormat::Singularity,
                ..Default::default()
            };
            oxo_flow_core::container::generate_singularity_def(&config, &pkg)
                .map_err(|e| anyhow::anyhow!(e))?
        }
        "compose" => {
            let pkg = oxo_flow_core::container::PackageConfig {
                format: oxo_flow_core::container::ContainerFormat::Compose,
                ..Default::default()
            };
            oxo_flow_core::container::generate_compose_file(&config, &pkg)
                .map_err(|e| anyhow::anyhow!(e))?
        }
        "toml" => oxo_flow_core::format::format_workflow(&config),
        other => anyhow::bail!(
            "unsupported export format '{}'. Supported formats: docker, singularity, compose, toml",
            other
        ),
    };

    match output {
        Some(path) => {
            std::fs::write(&path, &content)?;
            eprintln!(
                "{} Exported {} to {}",
                "✓".green().bold(),
                format,
                path.display()
            );
        }
        None => {
            println!("{content}");
        }
    }

    Ok(())
}

// ── AI result interpretation ───────────────────────────────────────────────

/// Plain-language interpretation of execution outcomes: what succeeded,
/// what the key metrics mean, caveats, and suggested next steps.
///
/// Returns `(model_name, interpretation_text)` — the caller decides where
/// it goes (stderr + report section). Nothing is printed here: stdout is
/// reserved for the report body (issue #83 P1-2).
async fn interpret_report_with_ai(
    _workflow: &std::path::Path,
    config: &WorkflowConfig,
    checkpoint: Option<&oxo_flow_core::executor::CheckpointState>,
    provider: &oxo_flow_ai::provider::AiProvider,
) -> Result<(String, String)> {
    let model = provider.model().unwrap_or_else(|| "default".into());

    // Compact execution summary for the prompt
    let (completed, failed, total) = match checkpoint {
        Some(cp) => (
            cp.completed_rules.len(),
            cp.failed_rules.len(),
            config.rules.len(),
        ),
        None => (0usize, 0usize, config.rules.len()),
    };
    let benchmarks: Vec<String> = checkpoint
        .map(|cp| {
            let mut names: Vec<&String> = cp.benchmarks.keys().collect();
            names.sort_unstable();
            names
                .into_iter()
                .map(|n| format!("- {n}: {:.1}s", cp.benchmarks[n].wall_time_secs))
                .take(20)
                .collect()
        })
        .unwrap_or_default();

    let system = r#"## Role
You are a senior bioinformatics analyst. Interpret workflow execution results
in plain language for a user who may not be a bioinformatics expert.

## Output Requirements
Provide:
1. **Summary** — 1-2 sentences: what ran and whether it succeeded
2. **Key metrics** — the 2-3 most important numbers and what they mean in plain language
3. **Caveats** — 1-2 limitations or things to check before trusting the results
4. **Next steps** — 1-2 concrete suggestions

Keep the total under 200 words. Use simple language; explain jargon.
"#;

    let user = format!(
        "## Workflow: {} (v{})\nDescription: {}\nRules: {total}, succeeded: {completed}, failed: {failed}\n\n## Per-rule timings\n{}\n\nInterpret these results.",
        config.workflow.name,
        config.workflow.version,
        config.workflow.description.as_deref().unwrap_or("(none)"),
        if benchmarks.is_empty() {
            "(no checkpoint benchmarks available — run the workflow first)".to_string()
        } else {
            benchmarks.join("\n")
        }
    );

    use oxo_flow_ai::types::Message;
    let messages = vec![Message::system(system), Message::user(&user)];
    let response = provider.chat_with_tools(&messages, &[]).await?;
    let text = response.content.unwrap_or_default();

    Ok((model, text))
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{parse_hms_to_secs, parse_maxrss_to_mb};

    #[test]
    fn hms_parser_handles_sacct_formats() {
        assert_eq!(parse_hms_to_secs("00:02"), Some(2.0));
        assert_eq!(parse_hms_to_secs("00:02:30"), Some(150.0));
        assert_eq!(parse_hms_to_secs("01:02:03"), Some(3723.0));
        assert_eq!(parse_hms_to_secs("1-00:00:00"), Some(86_400.0));
        assert_eq!(
            parse_hms_to_secs("2-12:30:15"),
            Some(2.0 * 86_400.0 + 12.0 * 3600.0 + 30.0 * 60.0 + 15.0)
        );
        assert_eq!(parse_hms_to_secs("00:00:00.5"), Some(0.5));
        assert_eq!(parse_hms_to_secs("00:00:01.25"), Some(1.25));
        assert_eq!(parse_hms_to_secs("  00:05  "), Some(5.0));
        assert_eq!(parse_hms_to_secs("0"), Some(0.0));
        assert_eq!(parse_hms_to_secs(""), None);
        assert_eq!(parse_hms_to_secs("abc"), None);
        assert_eq!(parse_hms_to_secs("1:2:3:4"), None);
    }

    #[test]
    fn maxrss_parser_handles_suffixes() {
        assert_eq!(parse_maxrss_to_mb("2048K"), Some(2));
        assert_eq!(parse_maxrss_to_mb("512M"), Some(512));
        assert_eq!(parse_maxrss_to_mb("1G"), Some(1024));
        assert_eq!(parse_maxrss_to_mb("1.5G"), Some(1536));
        assert_eq!(parse_maxrss_to_mb("2k"), Some(1)); // 2 KiB rounds up to 1 MB
        assert_eq!(parse_maxrss_to_mb("123456789c"), Some(118)); // 117.7… rounds up
        assert_eq!(parse_maxrss_to_mb("0"), Some(0));
        assert_eq!(parse_maxrss_to_mb("  8G  "), Some(8192));
        assert_eq!(parse_maxrss_to_mb(""), None);
        assert_eq!(parse_maxrss_to_mb("12X"), None);
        assert_eq!(parse_maxrss_to_mb("abc"), None);
    }
}
