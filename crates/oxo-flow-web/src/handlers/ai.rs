//! AI-enhanced endpoints for intelligent pipeline management.
//!
//! Provides search, suggestion, and debugging endpoints that use pattern
//! matching and heuristic analysis to assist pipeline authors.

use axum::{extract::Json, http::StatusCode};
use serde::{Deserialize, Serialize};

use crate::{ApiError, ErrorResponse, db, extract_session};

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// Request body for pipeline search.
#[derive(Deserialize)]
pub struct SearchRequest {
    /// Search query (required).
    pub query: String,
    /// Scope of search: "templates", "saved", or "all" (default "all").
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_scope() -> String {
    "all".to_string()
}

/// A single search result entry.
#[derive(Serialize)]
pub struct SearchResult {
    /// Unique identifier.
    pub id: String,
    /// Name of the workflow or template.
    pub name: String,
    /// Source: "template" or "saved".
    pub source: String,
    /// Category (for templates).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Tags (comma-separated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Why this result matched the query.
    pub match_reason: String,
    /// Match score (0.0 to 1.0).
    pub score: f64,
}

/// Response from the search endpoint.
#[derive(Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub total: usize,
    pub results: Vec<SearchResult>,
}

/// Local struct for querying saved workflows in search.
#[derive(Debug, sqlx::FromRow)]
struct SearchWorkflowRow {
    id: String,
    name: String,
    toml_content: String,
}

/// Simple keyword-based scoring for search results.
fn score_match(text: &str, query: &str) -> f64 {
    let lower_text = text.to_lowercase();
    let lower_query = query.to_lowercase();

    // Exact match
    if lower_text == lower_query {
        return 1.0;
    }
    // Contains the full query
    if lower_text.contains(&lower_query) {
        return 0.8;
    }
    // Count matching words
    let query_words: Vec<&str> = lower_query.split_whitespace().collect();
    if query_words.is_empty() {
        return 0.0;
    }
    let matched = query_words
        .iter()
        .filter(|w| lower_text.contains(**w))
        .count();
    matched as f64 / query_words.len() as f64 * 0.6
}

/// `POST /api/workflows/search` — Search pipelines by keyword.
///
/// Searches saved workflows and pipeline templates by matching against
/// name, description, tags, and rule names. Results are scored by relevance.
pub async fn search_workflows(
    headers: axum::http::HeaderMap,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    let query = req.query.trim();
    if query.is_empty() {
        return Err(ApiError::bad_request("Search query cannot be empty", None));
    }

    let session = extract_session(&headers).await.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            code: "AUTH_REQUIRED".to_string(),
            message: "Authentication required".to_string(),
            detail: None,
            suggestion: None,
        },
    })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    let scope = req.scope.as_str();
    let mut results: Vec<SearchResult> = Vec::new();

    // Search templates
    if scope == "all" || scope == "templates" {
        let templates = db::list_templates()
            .await
            .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

        for t in templates {
            let best_score = score_match(&t.name, query)
                .max(score_match(&t.description, query))
                .max(score_match(&t.tags, query));

            if best_score > 0.0 {
                let reasons = vec![
                    (score_match(&t.name, query) > 0.0).then_some("name"),
                    (score_match(&t.description, query) > 0.0).then_some("description"),
                    (score_match(&t.tags, query) > 0.0).then_some("tags"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");

                results.push(SearchResult {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    source: "template".to_string(),
                    category: Some(t.category.clone()),
                    description: Some(t.description.clone()),
                    tags: Some(t.tags.clone()),
                    match_reason: format!("matched by {}", reasons),
                    score: best_score,
                });
            }
        }
    }

    // Search saved workflows
    if scope == "all" || scope == "saved" {
        let saved = sqlx::query_as::<_, SearchWorkflowRow>(
            "SELECT id, name, version, toml_content, created_at, updated_at FROM workflows WHERE user_id = ?",
        )
        .bind(&user.id)
        .fetch_all(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?;

        for w in saved {
            let best_score = score_match(&w.name, query);

            // Also search the raw TOML content for rule names
            let toml_score = if w
                .toml_content
                .to_lowercase()
                .contains(&query.to_lowercase())
            {
                0.5
            } else {
                0.0
            };
            let best_score = best_score.max(toml_score);

            if best_score > 0.0 {
                results.push(SearchResult {
                    id: w.id.clone(),
                    name: w.name.clone(),
                    source: "saved".to_string(),
                    category: None,
                    description: None,
                    tags: None,
                    match_reason: if toml_score > 0.0 {
                        "matched by workflow content"
                    } else {
                        "matched by name"
                    }
                    .to_string(),
                    score: best_score,
                });
            }
        }
    }

    // Sort by score descending
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(Json(SearchResponse {
        query: query.to_string(),
        total: results.len(),
        results,
    }))
}

// ---------------------------------------------------------------------------
// Suggest
// ---------------------------------------------------------------------------

/// Request body for pipeline suggestion.
#[derive(Deserialize)]
pub struct SuggestRequest {
    /// Description of the desired analysis (e.g., "RNA-seq differential expression").
    pub description: String,
    /// Type of data (e.g., "rnaseq", "wgs", "chipseq", "single-cell").
    #[serde(default)]
    pub data_type: Option<String>,
    /// Comma-separated list of input files or file patterns.
    #[serde(default)]
    pub input_files: Option<String>,
}

/// A suggested pipeline with confidence score.
#[derive(Serialize)]
pub struct Suggestion {
    /// Pipeline template name.
    pub name: String,
    /// Human-readable description of what this pipeline does.
    pub description: String,
    /// Confidence score (0.0 to 1.0).
    pub confidence: f64,
    /// Estimated number of rules.
    pub rules_count: usize,
    /// Typical tools used in this pipeline.
    pub tools: Vec<String>,
    /// Data types this pipeline is suitable for.
    pub data_types: Vec<String>,
}

/// Response from the suggestion endpoint.
#[derive(Serialize)]
pub struct SuggestResponse {
    pub description: String,
    pub total: usize,
    pub suggestions: Vec<Suggestion>,
}

/// Template definitions for suggestion matching.
struct PipelineTemplate {
    name: &'static str,
    description: &'static str,
    keywords: &'static [&'static str],
    tools: &'static [&'static str],
    data_types: &'static [&'static str],
    rules_count: usize,
}

static PIPELINE_TEMPLATES: &[PipelineTemplate] = &[
    PipelineTemplate {
        name: "RNA-seq Quantification",
        description: "STAR alignment + featureCounts quantification + DESeq2 differential expression. Suitable for bulk RNA-seq gene expression analysis.",
        keywords: &[
            "rna-seq",
            "rnaseq",
            "rna seq",
            "transcriptome",
            "gene expression",
            "differential expression",
            "quantification",
            "star",
            "featurecounts",
            "deseq2",
        ],
        tools: &["STAR", "featureCounts", "DESeq2", "FastQC", "MultiQC"],
        data_types: &["rnaseq", "rna-seq", "bulk rna-seq"],
        rules_count: 4,
    },
    PipelineTemplate {
        name: "Variant Calling (GATK)",
        description: "BWA alignment + GATK best practices for germline variant discovery. Includes duplicate marking, BQSR, and HaplotypeCaller.",
        keywords: &[
            "variant",
            "wgs",
            "germline",
            "somatic",
            "gatk",
            "bwa",
            "haplotypecaller",
            "snv",
            "indel",
            "mutation",
        ],
        tools: &["BWA", "samtools", "GATK", "Picard"],
        data_types: &["wgs", "wes", "whole genome", "whole exome"],
        rules_count: 5,
    },
    PipelineTemplate {
        name: "Quality Control",
        description: "FastQC quality assessment + MultiQC aggregation. Provides comprehensive sequencing quality metrics for all samples.",
        keywords: &[
            "qc",
            "quality",
            "fastqc",
            "multiqc",
            "sequencing quality",
            "quality control",
            "read quality",
        ],
        tools: &["FastQC", "MultiQC"],
        data_types: &["any", "all"],
        rules_count: 2,
    },
    PipelineTemplate {
        name: "Single-Cell RNA-seq",
        description: "10x Genomics Cell Ranger + Seurat analysis pipeline. Processes raw FASTQ to annotated cell clusters.",
        keywords: &[
            "single-cell",
            "10x",
            "scrna",
            "sc-rna",
            "single cell",
            "cellranger",
            "seurat",
            "umap",
        ],
        tools: &["Cell Ranger", "Seurat", "FastQC"],
        data_types: &["single-cell", "10x", "scrna"],
        rules_count: 3,
    },
    PipelineTemplate {
        name: "ChIP-seq Peak Calling",
        description: "Bowtie2 alignment + MACS2 peak calling + annotation. Suitable for transcription factor and histone mark ChIP-seq.",
        keywords: &[
            "chipseq",
            "chip-seq",
            "chip",
            "peak calling",
            "macs2",
            "bowtie2",
            "histone",
            "transcription factor",
        ],
        tools: &["Bowtie2", "MACS2", "samtools", "HOMER"],
        data_types: &["chipseq", "chip-seq"],
        rules_count: 3,
    },
    PipelineTemplate {
        name: "Multi-Omics Integration",
        description: "Integrates RNA-seq, ATAC-seq, and methylation data for comprehensive multi-modal analysis.",
        keywords: &[
            "multiomics",
            "multi-omics",
            "multi omics",
            "integrate",
            "multi-modal",
            "integration",
            "multi-omic",
        ],
        tools: &["Salmon", "MACS2", "Bismark", "custom R script"],
        data_types: &["multiomics", "multi-omics"],
        rules_count: 4,
    },
    PipelineTemplate {
        name: "Scatter-Gather Processing",
        description: "Parallel processing template with split → process → combine steps. Ideal for large-scale batch operations.",
        keywords: &[
            "scatter",
            "gather",
            "parallel",
            "batch",
            "split",
            "combine",
            "large-scale",
            "high-throughput",
        ],
        tools: &["split", "parallel tool", "cat"],
        data_types: &["large", "batch"],
        rules_count: 3,
    },
];

/// `POST /api/workflows/suggest` — Suggest pipeline templates based on input description.
///
/// Uses keyword matching to find the most relevant pipeline template for the
/// given analysis description and optional data type. Results are ranked by
/// confidence score.
pub async fn suggest_pipeline(Json(req): Json<SuggestRequest>) -> Json<SuggestResponse> {
    let description = req.description.to_lowercase();
    let data_type = req.data_type.as_ref().map(|s| s.to_lowercase());

    let mut suggestions: Vec<Suggestion> = PIPELINE_TEMPLATES
        .iter()
        .filter_map(|t| {
            // Score by keyword overlap
            let keyword_score = t
                .keywords
                .iter()
                .filter(|k| description.contains(&k.to_lowercase()))
                .count() as f64
                / t.keywords.len() as f64;

            // Score by data type match
            let type_score = data_type.as_ref().map_or(0.3, |dt| {
                if t.data_types.iter().any(|d| d == &"any" || d == &"all") {
                    0.5
                } else if t.data_types.iter().any(|d| dt.contains(d)) {
                    0.4
                } else {
                    0.0
                }
            });

            let confidence = (keyword_score * 0.7 + type_score * 0.3).clamp(0.0, 1.0);

            if confidence < 0.1 {
                return None;
            }

            Some(Suggestion {
                name: t.name.to_string(),
                description: t.description.to_string(),
                confidence,
                rules_count: t.rules_count,
                tools: t.tools.iter().map(|s| s.to_string()).collect(),
                data_types: t.data_types.iter().map(|s| s.to_string()).collect(),
            })
        })
        .collect();

    suggestions.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Json(SuggestResponse {
        description: req.description,
        total: suggestions.len(),
        suggestions,
    })
}

// ---------------------------------------------------------------------------
// AI Debug
// ---------------------------------------------------------------------------

/// Response from the debug endpoint.
#[derive(Serialize)]
pub struct DebugAnalysis {
    /// The analyzed run ID.
    pub run_id: String,
    /// Status of the run.
    pub status: String,
    /// Detected error pattern (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_pattern: Option<String>,
    /// Likely cause of the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub likely_cause: Option<String>,
    /// Suggested fixes, ranked by likelihood.
    pub suggestions: Vec<String>,
    /// Log snippets that triggered detection.
    pub matched_log_lines: Vec<String>,
}

/// `POST /api/runs/{id}/debug` — Analyze a failed run and suggest fixes.
///
/// Parses the execution log for known error patterns (command not found,
/// out of memory, permission denied, etc.) and returns actionable
/// suggestions for resolving the issue.
pub async fn debug_run(
    headers: axum::http::HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
) -> Result<Json<DebugAnalysis>, ApiError> {
    let session = crate::extract_session(&headers)
        .await
        .ok_or_else(|| ApiError {
            status: StatusCode::UNAUTHORIZED,
            body: ErrorResponse {
                code: "AUTH_REQUIRED".to_string(),
                message: "Authentication required".to_string(),
                detail: None,
                suggestion: None,
            },
        })?;

    let user = db::get_user_by_id(&session.user_id)
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError::bad_request("User not found", None))?;

    let run = sqlx::query_as::<_, db::Run>("SELECT * FROM runs WHERE id = ? AND user_id = ?")
        .bind(&run_id)
        .bind(&user.id)
        .fetch_optional(db::pool())
        .await
        .map_err(|e| ApiError::bad_request("Database error", Some(e.to_string())))?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "NOT_FOUND".to_string(),
                message: "Run not found or not owned by user".to_string(),
                detail: None,
                suggestion: None,
            },
        })?;

    let run_dir = crate::workspace::get_run_directory(&user.username, &run_id);
    let log_path = run_dir.join("execution.log");

    let log_content = if log_path.exists() {
        std::fs::read_to_string(&log_path).unwrap_or_default()
    } else {
        String::new()
    };

    let (error_pattern, likely_cause, suggestions, matched_lines) =
        analyze_log(&log_content, run.status.as_str());

    Ok(Json(DebugAnalysis {
        run_id,
        status: run.status,
        error_pattern,
        likely_cause,
        suggestions,
        matched_log_lines: matched_lines,
    }))
}

/// Known error patterns and their suggested fixes.
struct ErrorPattern {
    pattern: &'static [&'static str],
    error_name: &'static str,
    cause: &'static str,
    fixes: &'static [&'static str],
}

static ERROR_PATTERNS: &[ErrorPattern] = &[
    ErrorPattern {
        pattern: &[
            "command not found",
            "not found",
            "No such file or directory",
            "cannot find",
        ],
        error_name: "missing_command_or_file",
        cause: "A required command or file is missing from the execution environment.",
        fixes: &[
            "Verify that the tool is installed and available in PATH",
            "Check that the environment specification (conda/docker/venv) is correct",
            "Ensure all input files exist at the expected paths",
        ],
    },
    ErrorPattern {
        pattern: &[
            "out of memory",
            "memory exhausted",
            "cannot allocate memory",
            "killed",
            "OOM",
            "OutOfMemoryError",
            "oom-killer",
        ],
        error_name: "out_of_memory",
        cause: "The process exceeded the available memory allocation. This is common for alignment and variant calling steps with large genomes.",
        fixes: &[
            "Increase the memory allocation for the failing rule in resources.memory",
            "Reduce the number of parallel jobs with -j to decrease memory pressure",
            "Use a larger instance type or add swap space",
        ],
    },
    ErrorPattern {
        pattern: &[
            "permission denied",
            "Permission denied",
            "access denied",
            "EACCES",
        ],
        error_name: "permission_denied",
        cause: "The process does not have permission to read a file, write to a directory, or execute a command.",
        fixes: &[
            "Check file and directory permissions for the working directory",
            "Ensure the output directory is writable",
            "Verify the user has execute permission for the required tool",
        ],
    },
    ErrorPattern {
        pattern: &[
            "timeout",
            "timed out",
            "Timeout",
            "wall time",
            "DUE TO TIME LIMIT",
        ],
        error_name: "timeout",
        cause: "The job exceeded its allocated wall time limit.",
        fixes: &[
            "Increase the time_limit for the failing rule",
            "Reduce input data size or use a more efficient algorithm",
            "Check for infinite loops or stuck processes",
        ],
    },
    ErrorPattern {
        pattern: &[
            "segmentation fault",
            "segfault",
            "SIGSEGV",
            "signal 11",
            "core dumped",
            "bus error",
        ],
        error_name: "segmentation_fault",
        cause: "The tool crashed with a segmentation fault, typically due to a bug or incompatible input.",
        fixes: &[
            "Update the tool to the latest version",
            "Check for incompatible input file formats",
            "Reduce the number of threads to work around memory issues",
            "Try a different version of the tool",
        ],
    },
    ErrorPattern {
        pattern: &[
            "exit code: 1",
            "exit code: 127",
            "exit code: 2",
            "Non-zero exit",
        ],
        error_name: "generic_tool_failure",
        cause: "The tool exited with a non-zero exit code, indicating an error during processing.",
        fixes: &[
            "Check the stderr output above for specific error messages from the tool",
            "Verify the input files are valid and not corrupted",
            "Check that all required parameters are provided correctly",
        ],
    },
    ErrorPattern {
        pattern: &[
            "disk quota",
            "no space left",
            "disk full",
            "ENOSPC",
            "write error",
        ],
        error_name: "disk_full",
        cause: "The disk is full or the job exceeded its allocated storage quota.",
        fixes: &[
            "Free up disk space by removing intermediate or temporary files",
            "Run `oxo-flow clean` to remove obsolete output files",
            "Increase the disk allocation or use a different storage location",
        ],
    },
    ErrorPattern {
        pattern: &[
            "error:",
            "Error:",
            "ERROR:",
            "failed:",
            "FAILED:",
            "exception",
            "Exception",
            "traceback",
            "Traceback",
        ],
        error_name: "unhandled_error",
        cause: "The tool encountered an unexpected error. Review the error details above for specific information.",
        fixes: &[
            "Review the full error message in the execution log",
            "Check tool documentation for the specific error",
            "Verify input data integrity and format",
        ],
    },
];

/// Analyze a log file for known error patterns and return structured diagnostics.
fn analyze_log(
    log_content: &str,
    status: &str,
) -> (Option<String>, Option<String>, Vec<String>, Vec<String>) {
    if status == "success" || status == "completed" {
        return (
            None,
            None,
            vec!["No errors detected — the run completed successfully.".to_string()],
            vec![],
        );
    }

    let mut matched_lines: Vec<String> = Vec::new();
    let mut matched_pattern: Option<&ErrorPattern> = None;

    // Score each error pattern by number of matched lines
    for pattern in ERROR_PATTERNS {
        for line in log_content.lines() {
            if pattern.pattern.iter().any(|p| line.contains(p)) {
                matched_lines.push(line.to_string());
                matched_pattern = Some(pattern);
                break; // One match per pattern is enough
            }
        }
        if matched_lines.len() >= 3 {
            break;
        }
    }

    if let Some(pattern) = matched_pattern {
        let fixes: Vec<String> = pattern.fixes.iter().map(|s| s.to_string()).collect();
        (
            Some(pattern.error_name.to_string()),
            Some(pattern.cause.to_string()),
            fixes,
            matched_lines,
        )
    } else {
        // Generic fallback
        (
            Some("unknown_error".to_string()),
            Some("The run failed but no known error pattern was detected in the log.".to_string()),
            vec![
                "Review the full execution log for error messages".to_string(),
                "Check that the workflow TOML is syntactically valid".to_string(),
                "Verify that all referenced tools and files exist".to_string(),
                "Run `oxo-flow validate` to check the workflow configuration".to_string(),
            ],
            matched_lines,
        )
    }
}

/// Try to generate a pipeline using the configured AI provider.
/// Returns None if AI is disabled or fails, for graceful fallback to template matching.
pub async fn try_ai_generate(
    intent: &str,
    organism: Option<&str>,
    tools: Option<&str>,
) -> Option<crate::GenerateResponse> {
    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();
    if provider.name() == "disabled" {
        return None;
    }

    let system_prompt = r#"You are a bioinformatics pipeline generator. Generate a valid oxo-flow workflow
in TOML format. Follow this structure exactly:

[workflow]
name = "pipeline-name"
version = "1.0.0"
description = "..."

[[rules]]
name = "step_name"
input = ["{sample}.fastq.gz"]
output = ["{sample}_output.txt"]
shell = "tool command {input} > {output}"
threads = 4

Rules:
- Each rule must have a unique name, input/output files, and a shell command
- Use {sample} wildcard for sample-specific paths
- Set reasonable thread counts for each tool
- Return ONLY the TOML content, no explanations.
Generate a complete workflow TOML for: "#;

    let mut user_prompt = intent.to_string();
    if let Some(org) = organism {
        user_prompt.push_str(&format!(
            "
Organism: {org}"
        ));
    }
    if let Some(t) = tools {
        user_prompt.push_str(&format!(
            "
Desired tools: {t}"
        ));
    }

    match provider.chat(system_prompt, &user_prompt).await {
        Ok(response) => {
            let toml_content = if let Some(start) = response.find("[workflow]") {
                response[start..].to_string()
            } else {
                response.clone()
            };

            match oxo_flow_core::WorkflowConfig::parse(&toml_content) {
                Ok(config) => {
                    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules).ok()?;
                    let execution_order = dag.execution_order().ok()?;
                    Some(crate::GenerateResponse {
                        toml_content,
                        workflow_name: config.workflow.name.clone(),
                        rules_count: config.rules.len(),
                        execution_order,
                        description: format!("AI-generated: {}", intent),
                        valid: true,
                    })
                }
                Err(e) => {
                    tracing::warn!("AI-generated TOML validation failed: {e}");
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!("AI provider failed: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// AI Provider Config API
// ---------------------------------------------------------------------------

/// Request to update AI provider configuration at runtime.
#[derive(Deserialize)]
pub struct AiConfigRequest {
    pub provider: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// Response from AI provider test.
#[derive(Serialize)]
pub struct AiTestResponse {
    pub success: bool,
    pub message: String,
    pub provider: String,
    pub model: String,
}

/// `GET /api/ai/config` — Get current AI provider configuration (no secrets).
pub async fn get_ai_config() -> Json<serde_json::Value> {
    let c = crate::ai_provider::AiProviderRegistry::global().get_config();
    Json(serde_json::json!({
        "provider": c.provider,
        "api_url": c.api_url,
        "model": c.model,
        "is_configured": c.is_configured,
    }))
}

/// `POST /api/ai/config` — Update AI provider configuration at runtime.
pub async fn update_ai_config(
    headers: axum::http::HeaderMap,
    Json(req): Json<AiConfigRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = crate::extract_session(&headers)
        .await
        .ok_or_else(|| ApiError::unauthorized("Authentication required", None))?;

    crate::ai_provider::AiProviderRegistry::global()
        .reconfigure(&req.provider, req.api_key, req.api_url, req.model)
        .map_err(|e| ApiError::bad_request("Failed to update AI config", Some(e)))?;

    let c = crate::ai_provider::AiProviderRegistry::global().get_config();
    Ok(Json(serde_json::json!({
        "provider": c.provider,
        "api_url": c.api_url,
        "model": c.model,
        "is_configured": c.is_configured,
    })))
}

/// `POST /api/ai/test` — Test the AI provider with a simple prompt.
pub async fn test_ai_config(
    headers: axum::http::HeaderMap,
) -> Result<Json<AiTestResponse>, ApiError> {
    let _ = crate::extract_session(&headers)
        .await
        .ok_or_else(|| ApiError::unauthorized("Authentication required", None))?;

    let provider = crate::ai_provider::AiProviderRegistry::global().get_provider();
    let provider_name = provider.name().to_string();
    let config = crate::ai_provider::AiProviderRegistry::global().get_config();

    if provider_name == "disabled" {
        return Ok(Json(AiTestResponse {
            success: false,
            message: "AI provider is not configured. Set provider settings first.".to_string(),
            provider: provider_name,
            model: config.model.unwrap_or_default(),
        }));
    }

    match provider
        .chat(
            "You are a helpful assistant. Reply with a short confirmation.",
            "Say hello and confirm you are working.",
        )
        .await
    {
        Ok(response) => {
            let snippet: &str = response.trim();
            let snippet = if snippet.len() > 200 {
                &snippet[..200]
            } else {
                snippet
            };
            Ok(Json(AiTestResponse {
                success: true,
                message: format!("AI provider '{}' responded: {}", provider_name, snippet),
                provider: provider_name,
                model: config.model.unwrap_or_default(),
            }))
        }
        Err(e) => Ok(Json(AiTestResponse {
            success: false,
            message: format!("AI provider '{}' error: {e}", provider_name),
            provider: provider_name,
            model: config.model.unwrap_or_default(),
        })),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_match_exact() {
        let score = score_match("RNA-seq Quantification", "RNA-seq Quantification");
        assert!((score - 1.0).abs() < 0.01);
    }

    #[test]
    fn score_match_partial() {
        let score = score_match("Variant Calling Pipeline", "variant");
        assert!(score > 0.0);
        assert!(score < 1.0);
    }

    #[test]
    fn score_match_no_match() {
        let score = score_match("Quality Control", "rnaseq");
        assert!(score < 0.01);
    }

    #[test]
    fn score_match_case_insensitive() {
        let score = score_match("RNA-seq Analysis", "rna-seq");
        assert!(score > 0.0);
    }

    #[tokio::test]
    async fn suggest_pipeline_rnaseq() {
        let req = SuggestRequest {
            description: "I want to run RNA-seq differential expression analysis".to_string(),
            data_type: Some("rnaseq".to_string()),
            input_files: None,
        };
        let resp = suggest_pipeline(Json(req)).await;
        assert!(!resp.0.suggestions.is_empty());
        assert!(resp.0.suggestions[0].name.contains("RNA-seq"));
    }

    #[tokio::test]
    async fn suggest_pipeline_empty_description() {
        let req = SuggestRequest {
            description: "".to_string(),
            data_type: None,
            input_files: None,
        };
        let resp = suggest_pipeline(Json(req)).await;
        assert!(resp.0.suggestions.is_empty());
    }

    #[tokio::test]
    async fn suggest_pipeline_variant_calling() {
        let req = SuggestRequest {
            description: "germline variant calling with GATK".to_string(),
            data_type: None,
            input_files: None,
        };
        let resp = suggest_pipeline(Json(req)).await;
        assert!(!resp.0.suggestions.is_empty());
        assert!(resp.0.suggestions[0].name.contains("Variant"));
    }

    #[test]
    fn analyze_log_success_run() {
        let (pattern, cause, fixes, lines) = analyze_log("everything worked", "success");
        assert!(pattern.is_none());
        assert!(cause.is_none());
        assert!(!fixes.is_empty());
        assert!(lines.is_empty());
    }

    #[test]
    fn analyze_log_out_of_memory() {
        let log = "bwa mem: cannot allocate memory for index
        Command exited with non-zero status 1";
        let (pattern, cause, fixes, _) = analyze_log(log, "failed");
        assert_eq!(pattern.unwrap(), "out_of_memory");
        assert!(cause.unwrap().contains("memory"));
        assert!(!fixes.is_empty());
    }

    #[test]
    fn analyze_log_command_not_found() {
        let log = "STAR: command not found";
        let (pattern, _cause, fixes, _) = analyze_log(log, "failed");
        assert_eq!(pattern.unwrap(), "missing_command_or_file");
        assert!(!fixes.is_empty());
    }

    #[test]
    fn analyze_log_permission_denied() {
        let log = "samtools: Permission denied when opening output file";
        let (pattern, _cause, fixes, _) = analyze_log(log, "failed");
        assert_eq!(pattern.unwrap(), "permission_denied");
        assert!(!fixes.is_empty());
    }

    #[test]
    fn analyze_log_unknown_error() {
        let log = "some weird cryptic error message that nothing matches";
        let (pattern, _cause, fixes, _) = analyze_log(log, "failed");
        assert_eq!(pattern.unwrap(), "unknown_error");
        assert!(!fixes.is_empty());
    }
}
