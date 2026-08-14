//! Logic for output-related subcommands: graph, report, diff, export.

use crate::commands::{print_banner, resolve_workflow};
use anyhow::{Context, Result};
use colored::Colorize;
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use std::path::PathBuf;

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

pub async fn handle_report(
    workflow: PathBuf,
    format: Option<String>,
    output: Option<PathBuf>,
    checkpoint_path: Option<PathBuf>,
    ai: bool,
    workdir: Option<PathBuf>,
    ci: bool,
    no_timestamps: bool,
    strict: bool,
    list_sections: bool,
) -> Result<()> {
    use oxo_flow_core::{
        executor::CheckpointState,
        report::{ReportBuilder, ReportContent, ReportSection},
    };

    print_banner();

    // --list-sections: enumerate the registry and exit (issue #83 P2-7).
    if list_sections {
        let registry = oxo_flow_core::report::SectionRegistry::with_defaults();
        for (name, description) in registry.sections() {
            println!("{name:<24} {description}");
        }
        return Ok(());
    }

    let workflow = resolve_workflow(Some(workflow))?;
    let config = WorkflowConfig::from_file(&workflow)
        .with_context(|| format!("failed to parse {}", workflow.display()))?;

    // [report].template/format are parsed but not consumed — say so instead
    // of silently ignoring user configuration (issue #83 P0-9).
    if let Some(report_cfg) = &config.report {
        let unsupported: Vec<&str> = [
            report_cfg.template.is_some().then_some("template"),
            (!report_cfg.format.is_empty()).then_some("format"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !unsupported.is_empty() {
            let msg = format!(
                "[report].{} is declared in the workflow but not supported yet — only \
                 [report].sections is honored",
                unsupported.join("/")
            );
            if strict {
                anyhow::bail!(msg);
            }
            eprintln!("  {} {}", "⚠".yellow(), msg);
        }
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

    // Determine checkpoint path: explicit > workdir-relative (--workdir,
    // else the workflow's directory) > warn (issue #68).
    let checkpoint_path = checkpoint_path.unwrap_or_else(|| {
        let base = workdir
            .clone()
            .unwrap_or_else(|| oxo_flow_core::parent_dir(&workflow).to_path_buf());
        base.join(".oxo-flow").join("checkpoint.json")
    });

    let checkpoint = match CheckpointState::load_from_file(&checkpoint_path) {
        Ok(cp) => Some(cp),
        Err(_) => {
            if strict {
                // Exit code 2 = data source unavailable — CI can tell a
                // template-only report from a complete one (issue #83 P1-17).
                eprintln!(
                    "  {} No checkpoint found at {}",
                    "✗".red(),
                    checkpoint_path.display()
                );
                eprintln!(
                    "  Run the workflow first, or drop --strict to allow a template-level report."
                );
                std::process::exit(2);
            }
            eprintln!(
                "  {} No checkpoint found at {}",
                "Note:".yellow(),
                checkpoint_path.display()
            );
            eprintln!(
                "  {} Report will show template-level data only. Run the workflow first for execution metrics.",
                "Info:".dimmed()
            );
            None
        }
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

    // ── Build report using the pluggable section system ──
    // Domain auto-detection tailors sections to the workflow type.
    // Users can override via [report].sections in their .oxoflow file.

    let domain = oxo_flow_core::report::WorkflowDomain::detect(&config.rules);
    let ctx = oxo_flow_core::report::ReportContext {
        config: &config,
        checkpoint: checkpoint.as_ref(),
        domain,
        workflow_path: Some(workflow.as_path()),
        checkpoint_path: Some(checkpoint_path.as_path()),
    };

    // Resolve section filter from [report].sections config (if present).
    let section_filter: Option<std::collections::HashSet<String>> =
        config.report.as_ref().and_then(|r| {
            let sections = &r.sections;
            if sections.is_empty() {
                None
            } else {
                Some(sections.iter().cloned().collect())
            }
        });

    let registry = oxo_flow_core::report::SectionRegistry::with_defaults();
    let mut sections = registry.generate(&ctx, section_filter.as_ref());
    if let Some(ai_sec) = ai_section {
        sections.push(ai_sec);
    }

    // Generation timestamp: --no-timestamps omits it; --ci pins it to
    // SOURCE_DATE_EPOCH (or the Unix epoch) so identical state yields
    // byte-identical reports (issue #83 P1-4).
    let generated_at = if no_timestamps {
        None
    } else if ci {
        Some(pinned_timestamp())
    } else {
        Some(chrono::Utc::now())
    };

    let mut report = ReportBuilder::new(
        &format!("{} Report", config.workflow.name),
        &config.workflow.name,
        &config.workflow.version,
    )
    .workflow_path(Some(workflow.display().to_string()))
    .checkpoint_path(checkpoint.map(|_| checkpoint_path.display().to_string()))
    .generated_at(generated_at);
    for section in sections {
        report = report.section(section);
    }

    let report = report.build();

    let content = match format.as_str() {
        "html" | "htm" => report.to_html(),
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
        None => {
            println!("{content}");
        }
    }

    Ok(())
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
