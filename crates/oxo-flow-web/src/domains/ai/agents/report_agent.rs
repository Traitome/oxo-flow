//! Report Agent — generates scientific narratives and Q&A from pipeline results.
//!
//! Features: narrative generation, key finding extraction, interactive Q&A,
//! chart suggestion, file tree analysis.

use super::types::*;

/// Generate a structured report from pipeline results.
///
/// `run_status` and `failed_rules` make the report failure-aware (issue
/// #759): a failed run's headline says so and Q&A answers from the
/// checkpoint's structured failure data instead of the completed-fallback.
pub fn generate_report(
    pipeline_name: &str,
    files: &[ReportFile],
    log_summary: &str,
    diagnostics: &[String],
    run_status: &str,
    failed_rules: Vec<FailedRuleInfo>,
) -> ReportData {
    let mut findings = Vec::new();
    let mut caveats = Vec::new();
    let mut suggested_next = Vec::new();
    let mut total_size: i64 = 0;

    for f in files {
        total_size += f.size_bytes;
    }

    // QC summary
    let qc_summary = serde_json::json!({
        "total_files": files.len(),
        "total_size_mb": format!("{:.1}", total_size as f64 / 1048576.0),
        "directories": files.iter().filter(|f| f.is_dir).count(),
        "file_types": extract_file_types(files),
    });

    // Generate key findings from log/diagnostics. The OOM match is
    // case-insensitive so the engine's lowercase pattern ids
    // (`oom_killed`) trigger it too, not just hand-written "OOM" strings.
    if !diagnostics.is_empty() {
        for diag in diagnostics {
            let lower = diag.to_lowercase();
            if lower.contains("oom") || lower.contains("out of memory") || diag.contains("137") {
                findings.push(ReportFinding {
                    finding: format!("Memory issue detected: {diag}"),
                    significance: "high".into(),
                    evidence: "System reported OOM".into(),
                });
            }
        }
    }

    // Standard findings from the log
    if log_summary.contains("error") || log_summary.contains("Error") {
        caveats.push("Some steps reported errors — review diagnostics".into());
    }
    if run_status == "failed" && !failed_rules.is_empty() {
        let names: Vec<&str> = failed_rules.iter().map(|f| f.rule.as_str()).collect();
        caveats.push(format!("Failed rule(s): {}", names.join(", ")));
    }

    // Suggest next steps based on typical RNA-seq/variant analysis
    let lower_name = pipeline_name.to_lowercase();
    if lower_name.contains("rna") || lower_name.contains("rnaseq") {
        suggested_next.push("Run DESeq2 or edgeR for differential expression analysis".into());
        suggested_next.push("Perform GO/KEGG enrichment on differentially expressed genes".into());
        suggested_next.push("Validate key findings with qPCR or alternative method".into());
    } else if lower_name.contains("variant") || lower_name.contains("wgs") {
        suggested_next.push("Annotate variants with VEP or ANNOVAR".into());
        suggested_next.push("Filter variants by quality and population frequency".into());
        suggested_next.push("Validate candidate variants with Sanger sequencing".into());
    } else {
        suggested_next.push("Review output files for quality metrics".into());
        suggested_next.push("Compare results with known benchmarks".into());
    }

    // Chart suggestions
    let charts = suggest_charts(files, pipeline_name);

    // Build narrative
    let narrative_md = build_narrative(
        pipeline_name,
        &qc_summary,
        &findings,
        &caveats,
        &suggested_next,
        run_status,
        &failed_rules,
    );

    ReportData {
        qc_summary,
        key_findings: findings,
        narrative_md,
        caveats,
        suggested_next,
        file_tree: files.to_vec(),
        charts,
        run_status: run_status.to_string(),
        failed_rules,
    }
}

/// Build a markdown-formatted narrative.
fn build_narrative(
    pipeline_name: &str,
    qc_summary: &serde_json::Value,
    findings: &[ReportFinding],
    caveats: &[String],
    suggested_next: &[String],
    run_status: &str,
    failed_rules: &[FailedRuleInfo],
) -> String {
    let mut md = format!("# Pipeline Report: {pipeline_name}\n\n");

    md.push_str("## Summary\n\n");
    if run_status == "failed" {
        // A failed run must never read "completed" (issue #759).
        if failed_rules.is_empty() {
            md.push_str("Pipeline **failed** before completing all steps.\n\n");
        } else if failed_rules.len() == 1 {
            let f = &failed_rules[0];
            let code = f
                .exit_code
                .map(|c| format!(" (exit {c})"))
                .unwrap_or_default();
            md.push_str(&format!(
                "Pipeline **failed** at rule `{}`{code}.\n\n",
                f.rule
            ));
        } else {
            let names: Vec<&str> = failed_rules.iter().map(|f| f.rule.as_str()).collect();
            md.push_str(&format!(
                "Pipeline **failed** at {} rules: {}.\n\n",
                failed_rules.len(),
                names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        md.push_str(&format!(
            "The run stopped at the failure; **{}** output files were produced before it.\n\n",
            qc_summary["total_files"]
        ));
    } else {
        md.push_str(&format!(
            "Pipeline completed with **{}** output files.\n\n",
            qc_summary["total_files"]
        ));
    }
    md.push_str(&format!(
        "Total output size: **{}**.\n\n",
        qc_summary["total_size_mb"].as_str().unwrap_or("unknown")
    ));

    if !findings.is_empty() {
        md.push_str("## Key Findings\n\n");
        for f in findings {
            md.push_str(&format!(
                "- **{}** (significance: {}) — {}\n",
                f.finding, f.significance, f.evidence
            ));
        }
        md.push('\n');
    }

    if !caveats.is_empty() {
        md.push_str("## Caveats\n\n");
        for c in caveats {
            md.push_str(&format!("- ⚠️ {c}\n"));
        }
        md.push('\n');
    }

    if !suggested_next.is_empty() {
        md.push_str("## Suggested Next Steps\n\n");
        for s in suggested_next {
            md.push_str(&format!("- 💡 {s}\n"));
        }
        md.push('\n');
    }

    md.push_str("---\n");
    md.push_str("*Report generated by oxo-flow AI Companion v0.8*\n");

    md
}

/// Suggest charts based on output files.
fn suggest_charts(files: &[ReportFile], pipeline_name: &str) -> Vec<ChartConfig> {
    let mut charts = Vec::new();
    let lower = pipeline_name.to_lowercase();

    let has_counts = files
        .iter()
        .any(|f| f.name.contains(".counts") || f.name.contains("featurecounts"));
    let has_vcf = files
        .iter()
        .any(|f| f.name.contains(".vcf") || f.name.contains(".gvcf"));
    let has_bam = files.iter().any(|f| f.name.contains(".bam"));
    let has_qc = files
        .iter()
        .any(|f| f.name.contains("_fastqc") || f.name.contains("qc_"));

    if has_counts && lower.contains("rna") {
        charts.push(ChartConfig {
            chart_type: "volcano".into(),
            title: "Differential Expression Volcano Plot".into(),
            spec: serde_json::json!({
                "mark": "point",
                "encoding": {
                    "x": {"field": "log2FC", "type": "quantitative", "title": "Log2 Fold Change"},
                    "y": {"field": "negLog10P", "type": "quantitative", "title": "-Log10(p-value)"},
                    "color": {"field": "significant", "type": "nominal"}
                }
            }),
        });
    }

    if has_vcf {
        charts.push(ChartConfig {
            chart_type: "bar".into(),
            title: "Variant Type Distribution".into(),
            spec: serde_json::json!({
                "mark": "bar",
                "encoding": {
                    "x": {"field": "variant_type", "type": "nominal", "title": "Variant Type"},
                    "y": {"field": "count", "type": "quantitative", "title": "Count"}
                }
            }),
        });
    }

    if has_qc || has_bam {
        charts.push(ChartConfig {
            chart_type: "line".into(),
            title: "Coverage Distribution".into(),
            spec: serde_json::json!({
                "mark": "line",
                "encoding": {
                    "x": {"field": "depth", "type": "quantitative", "title": "Sequencing Depth"},
                    "y": {"field": "fraction", "type": "quantitative", "title": "Fraction of Genome"}
                }
            }),
        });
    }

    if lower.contains("chip") {
        charts.push(ChartConfig {
            chart_type: "heatmap".into(),
            title: "Peak Signal Heatmap".into(),
            spec: serde_json::json!({
                "mark": "rect",
                "encoding": {
                    "x": {"field": "peak_region", "type": "nominal", "title": "Peak Region"},
                    "y": {"field": "sample", "type": "nominal", "title": "Sample"},
                    "color": {"field": "signal", "type": "quantitative"}
                }
            }),
        });
    }

    charts
}

/// Extract unique file extensions from the file list.
fn extract_file_types(files: &[ReportFile]) -> Vec<String> {
    let mut types: Vec<String> = files
        .iter()
        .filter_map(|f| {
            std::path::Path::new(&f.name)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_string())
        })
        .collect();
    types.sort();
    types.dedup();
    types
}

/// Summarize a failed run from its structured failure data (issue #759) —
/// shared by the keyword branch and the fallback so NO question on a
/// failed run ever reaches the "pipeline completed" text.
fn failure_answer(report: &ReportData) -> String {
    if report.failed_rules.is_empty() {
        return "The run failed before any rule completed its record — check the \
                execution log for the reported error (likely a configuration or \
                expansion failure at spawn)."
            .to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for f in &report.failed_rules {
        let code = f
            .exit_code
            .map(|c| format!(" (exit {c})"))
            .unwrap_or_default();
        let mut entry = format!("rule `{}`{code}", f.rule);
        // stderr first; stdout is the fallback (issue #765) — some tools
        // print their root cause on stdout and leave stderr empty.
        let tail = f
            .stderr_tail
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .or(f.stdout_tail.as_deref());
        if let Some(tail) = tail {
            let trimmed = tail.trim();
            if !trimmed.is_empty() {
                // Bounded excerpt: the stored tail is up to 64 KiB —
                // an answer is not the place for the whole thing.
                let excerpt: String = trimmed
                    .chars()
                    .rev()
                    .take(600)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                entry.push_str(&format!(" — error tail: …{excerpt}"));
            }
        }
        parts.push(entry);
    }
    format!(
        "The run failed: {}. Fix the reported error before retrying — the retry \
         re-executes the failed rule and its downstream dependents.",
        parts.join("; ")
    )
}

/// Answer a question about the report using context.
pub fn answer_question(report: &ReportData, question: &str) -> String {
    let lower = question.to_lowercase();

    // Failure-aware first (issue #759): on a failed run, questions about
    // failure/error/why answer from the checkpoint's structured failure
    // data — never the "pipeline completed" fallback, which actively
    // contradicted the run state.
    let asks_failure = ["fail", "error", "why", "wrong", "broke", "crash"]
        .iter()
        .any(|k| lower.contains(k));
    if asks_failure && report.run_status == "failed" {
        return failure_answer(report);
    }

    if lower.contains("file") || lower.contains("output") || lower.contains("result") {
        let total = report.file_tree.len();
        let total_size: i64 = report.file_tree.iter().map(|f| f.size_bytes).sum();
        format!(
            "The pipeline produced {total} output files totaling {:.1} MB. \
             The most common file types are: {}.",
            total_size as f64 / 1048576.0,
            extract_file_types(&report.file_tree).join(", ")
        )
    } else if lower.contains("quality") || lower.contains("qc") || lower.contains("good") {
        if report.caveats.is_empty() {
            "All quality checks passed. No significant issues detected.".to_string()
        } else {
            format!(
                "Some quality caveats to note: {}",
                report.caveats.join("; ")
            )
        }
    } else if lower.contains("next") || lower.contains("step") || lower.contains("suggest") {
        format!("Suggested next steps: {}", report.suggested_next.join("; "))
    } else if lower.contains("caveat") || lower.contains("limit") || lower.contains("caution") {
        if report.caveats.is_empty() {
            "No specific caveats identified for this run.".to_string()
        } else {
            format!("Caveats: {}", report.caveats.join("; "))
        }
    } else if lower.contains("chart")
        || lower.contains("plot")
        || lower.contains("graph")
        || lower.contains("visualize")
    {
        let charts: Vec<String> = report.charts.iter().map(|c| c.title.clone()).collect();
        format!("Available visualizations: {}", charts.join(", "))
    } else if report.run_status == "failed" {
        // The generic fallback must never claim completion on a failed
        // run, whatever the question said (issue #759).
        failure_answer(report)
    } else {
        format!(
            "Based on the analysis report for this run: the pipeline completed with {} output files. \
             {}",
            report.file_tree.len(),
            if report.key_findings.is_empty() {
                "No unexpected findings reported.".to_string()
            } else {
                format!(
                    "Key findings include: {}",
                    report
                        .key_findings
                        .iter()
                        .map(|f| f.finding.clone())
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_files() -> Vec<ReportFile> {
        vec![
            ReportFile {
                path: "/data/results/sample1.bam".into(),
                name: "sample1.bam".into(),
                size_bytes: 2_000_000_000,
                is_dir: false,
            },
            ReportFile {
                path: "/data/results/sample1.counts.tsv".into(),
                name: "sample1.counts.tsv".into(),
                size_bytes: 500_000,
                is_dir: false,
            },
            ReportFile {
                path: "/data/results/qc".into(),
                name: "qc".into(),
                size_bytes: 100_000,
                is_dir: true,
            },
            ReportFile {
                path: "/data/results/sample1_fastqc.html".into(),
                name: "sample1_fastqc.html".into(),
                size_bytes: 1_000_000,
                is_dir: false,
            },
        ]
    }

    #[test]
    fn test_generate_report_basic() {
        let report = generate_report(
            "rnaseq-test",
            &sample_files(),
            "All steps completed successfully",
            &[],
            "completed",
            Vec::new(),
        );
        assert!(report.narrative_md.contains("Pipeline Report"));
        assert!(!report.file_tree.is_empty());
        assert!(report.qc_summary["total_files"].as_u64().unwrap() >= 4);
    }

    #[test]
    fn test_generate_report_with_diagnostics() {
        let report = generate_report(
            "rnaseq-test",
            &sample_files(),
            "OOM error",
            &["OOM detected at step 2".into()],
            "completed",
            Vec::new(),
        );
        assert!(!report.key_findings.is_empty());
        assert!(report.key_findings[0].finding.contains("OOM"));
    }

    #[test]
    fn test_answer_about_files() {
        let report = generate_report("test", &sample_files(), "", &[], "completed", Vec::new());
        let answer = answer_question(&report, "What output files were generated?");
        assert!(answer.contains("output files"));
    }

    #[test]
    fn test_answer_about_quality() {
        let report = generate_report("test", &sample_files(), "", &[], "completed", Vec::new());
        let answer = answer_question(&report, "How is the quality?");
        assert!(!answer.is_empty());
    }

    #[test]
    fn test_answer_about_charts() {
        let report = generate_report(
            "rnaseq-test",
            &sample_files(),
            "",
            &[],
            "completed",
            Vec::new(),
        );
        let answer = answer_question(&report, "What charts are available?");
        assert!(answer.contains("visualization") || answer.contains("chart"));
    }

    #[test]
    fn test_extract_file_types() {
        let types = extract_file_types(&sample_files());
        assert!(types.contains(&"bam".to_string()));
        assert!(types.contains(&"tsv".to_string()));
    }

    #[test]
    fn failed_run_headline_says_failed_not_completed() {
        // #759: a failed run's narrative used to read "Pipeline completed
        // with N output files" — actively contradicting the run state.
        let report = generate_report(
            "when-gate-690",
            &sample_files(),
            "Error: output pattern contains unbound wildcard",
            &[],
            "failed",
            vec![FailedRuleInfo {
                rule: "trim".into(),
                exit_code: Some(-1),
                stderr_tail: Some("output pattern contains unbound wildcard {sample}".into()),
                stdout_tail: None,
            }],
        );
        assert!(
            report.narrative_md.contains("failed"),
            "headline must name the failure: {}",
            report.narrative_md
        );
        assert!(
            report.narrative_md.contains("`trim`"),
            "headline must name the failed rule: {}",
            report.narrative_md
        );
        assert_eq!(report.run_status, "failed");
        assert_eq!(report.failed_rules.len(), 1);
    }

    #[test]
    fn completed_run_narrative_unchanged() {
        let report = generate_report(
            "rnaseq-test",
            &sample_files(),
            "All steps completed successfully",
            &[],
            "completed",
            Vec::new(),
        );
        assert!(report.narrative_md.contains("Pipeline completed"));
        assert!(!report.narrative_md.contains("**failed**"));
    }

    #[test]
    fn why_did_it_fail_answers_from_failure_data() {
        // #759: this used to fall through to "the pipeline completed with
        // N output files. No unexpected findings reported."
        let report = generate_report(
            "when-gate-690",
            &sample_files(),
            "",
            &[],
            "failed",
            vec![FailedRuleInfo {
                rule: "trim".into(),
                exit_code: Some(-1),
                stderr_tail: Some("output pattern contains unbound wildcard {sample}".into()),
                stdout_tail: None,
            }],
        );
        let answer = answer_question(&report, "why did it fail");
        assert!(
            answer.contains("failed"),
            "answer must state the failure: {answer}"
        );
        assert!(
            answer.contains("trim"),
            "answer must name the failed rule: {answer}"
        );
        assert!(
            answer.contains("unbound wildcard"),
            "answer must surface the stderr tail: {answer}"
        );
        assert!(
            !answer.contains("completed with"),
            "answer must not use the completed fallback: {answer}"
        );
    }

    #[test]
    fn failure_question_on_pre_execution_failure_stays_honest() {
        let report = generate_report(
            "broken",
            &[],
            "Error: failed to expand wildcard rules",
            &[],
            "failed",
            Vec::new(),
        );
        let answer = answer_question(&report, "why did it fail?");
        assert!(answer.contains("failed"), "{answer}");
        assert!(
            answer.contains("execution log"),
            "must point at the log when no per-rule record exists: {answer}"
        );
    }

    #[test]
    fn keywordless_question_on_failed_run_never_claims_completion() {
        // #759 residual: "summarize" matches no keyword branch — the
        // generic fallback used to answer "the pipeline completed with N
        // output files" on a failed run.
        let report = generate_report(
            "when-gate-690",
            &sample_files(),
            "",
            &[],
            "failed",
            vec![FailedRuleInfo {
                rule: "trim".into(),
                exit_code: Some(-1),
                stderr_tail: Some("unbound wildcard {sample}".into()),
                stdout_tail: None,
            }],
        );
        let answer = answer_question(&report, "summarize this run");
        assert!(
            !answer.contains("completed with"),
            "no completion claim on a failed run: {answer}"
        );
        assert!(answer.contains("failed"), "{answer}");
        assert!(answer.contains("trim"), "{answer}");
    }

    #[test]
    fn stdout_tail_is_the_fallback_when_stderr_is_empty() {
        // #765: some tools print their root cause on stdout — the answer
        // must surface it instead of showing an empty error tail.
        let report = generate_report(
            "stdout-tool",
            &sample_files(),
            "",
            &[],
            "failed",
            vec![FailedRuleInfo {
                rule: "kickoff".into(),
                exit_code: Some(1),
                stderr_tail: None,
                stdout_tail: Some("Traceback (most recent call last): ValueError".into()),
            }],
        );
        let answer = answer_question(&report, "why did it fail");
        assert!(answer.contains("ValueError"), "{answer}");

        // stderr wins when both are present.
        let report = generate_report(
            "both-tools",
            &sample_files(),
            "",
            &[],
            "failed",
            vec![FailedRuleInfo {
                rule: "kickoff".into(),
                exit_code: Some(1),
                stderr_tail: Some("stderr-side error".into()),
                stdout_tail: Some("stdout-side noise".into()),
            }],
        );
        let answer = answer_question(&report, "why did it fail");
        assert!(answer.contains("stderr-side error"), "{answer}");
        assert!(!answer.contains("stdout-side noise"), "{answer}");
    }

    #[test]
    fn test_chart_suggestion_rnaseq() {
        let charts = suggest_charts(&sample_files(), "rnaseq-analysis");
        assert!(
            charts.iter().any(|c| c.chart_type == "volcano"),
            "RNA-seq should suggest volcano"
        );
    }
}
