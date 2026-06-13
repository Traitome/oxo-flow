//! Chat service — conversational pipeline creation with multi-agent orchestration.
//!
//! Coordinates: Orchestrator → (Data Agent, Tool Expert, Validator) → Response.
//! All agents call deterministic core APIs — zero write access to DB/FS/process.

use super::types::*;
use crate::ai_provider::AiProviderRegistry;
use crate::domains::workflow::service as workflow_svc;

/// Process a chat message and return SSE events via a channel.
/// This is the main entry point for the conversational AI pipeline.
pub async fn process_chat(
    message: &str,
    _session_id: Option<&str>,
    context: Option<&ChatContext>,
    templates: &[String],
) -> Result<(String, serde_json::Value), String> {
    // Phase 1: Orchestrator — understand intent
    let intent = if let Some(ctx) = context {
        if let Some(ref i) = ctx.intent {
            i.clone()
        } else {
            infer_intent(message)
        }
    } else {
        infer_intent(message)
    };

    // Phase 2: Data Agent — analyze data if paths provided
    let data_report = if let Some(ctx) = context {
        if let Some(ref paths) = ctx.data_paths {
            if !paths.is_empty() {
                analyze_data_paths(paths)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Phase 3: AI generation via provider
    let provider = AiProviderRegistry::global().get_provider();
    let system_prompt = build_system_prompt(&intent, data_report.as_ref(), templates);
    let user_prompt = format!("Generate a .oxoflow pipeline for: {message}");

    let ai_response = provider
        .chat(&system_prompt, &user_prompt)
        .await
        .map_err(|e| format!("AI generation failed: {e}"))?;

    // Phase 4: Extract TOML and validate
    let toml_content = extract_toml_from_response(&ai_response)
        .unwrap_or_else(|| ai_response.clone());

    let validation = workflow_svc::validate_pipeline(&toml_content)?;

    // Phase 5: Build response
    let pipeline_id = uuid::Uuid::new_v4().to_string();
    let parsed = workflow_svc::parse_pipeline(&toml_content, None).ok();

    let response = serde_json::json!({
        "pipeline_id": pipeline_id,
        "toml_content": toml_content,
        "intent": intent,
        "data_report": data_report,
        "validation": {
            "valid": validation.valid,
            "errors": validation.errors.iter().map(|e| serde_json::json!({
                "code": e.code, "message": e.message, "suggestion": e.suggestion
            })).collect::<Vec<_>>()
        },
        "rules": parsed.as_ref().map(|p| p.rules.iter().map(|r| serde_json::json!({
            "name": r.name,
            "inputs": r.inputs,
            "outputs": r.outputs,
            "environment": r.environment,
            "threads": r.threads
        })).collect::<Vec<_>>()),
        "dag": parsed.as_ref().map(|p| serde_json::json!({
            "nodes": p.dag.nodes.iter().map(|n| serde_json::json!({
                "id": n.id, "label": n.label, "color": n.color
            })).collect::<Vec<_>>(),
            "edges": p.dag.edges.iter().map(|e| serde_json::json!({
                "from": e.from, "to": e.to
            })).collect::<Vec<_>>(),
            "parallel_groups": p.dag.parallel_groups
        }))
    });

    Ok((ai_response, response))
}

/// Infer the user's intent from their message.
pub fn infer_intent(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("rna-seq") || lower.contains("rnaseq") || lower.contains("transcriptome") {
        "RNA-seq analysis".into()
    } else if lower.contains("variant") || lower.contains("wgs") || lower.contains("germline") {
        "Variant calling".into()
    } else if lower.contains("chip-seq") || lower.contains("chipseq") {
        "ChIP-seq analysis".into()
    } else if lower.contains("single-cell") || lower.contains("scrna") || lower.contains("10x") {
        "Single-cell RNA-seq".into()
    } else if lower.contains("qc") || lower.contains("quality") || lower.contains("fastqc") {
        "Quality control".into()
    } else if lower.contains("alignment") || lower.contains("align") || lower.contains("star") {
        "Read alignment".into()
    } else {
        "Bioinformatics analysis".into()
    }
}

/// Analyze data paths using the deterministic data discovery module.
pub fn analyze_data_paths(paths: &[String]) -> Option<serde_json::Value> {
    let max_depth = Some(2usize);
    match crate::domains::workflow::data::analyze_files(paths, max_depth) {
        Ok(report) => Some(serde_json::json!({
            "files": report.files.iter().map(|f| serde_json::json!({
                "path": f.path, "size": f.size, "format": f.format,
                "format_confidence": f.format_confidence, "sample_name": f.sample_name
            })).collect::<Vec<_>>(),
            "summary": {
                "total_size": report.summary.total_size,
                "formats_detected": report.summary.formats_detected,
                "paired_end_detected": report.summary.paired_end_detected,
            },
            "suggested_workflow": report.suggested_workflow.as_ref().map(|sw| serde_json::json!({
                "template": sw.template, "confidence": sw.confidence, "reason": sw.reason
            }))
        })),
        Err(_) => None,
    }
}

/// Build the system prompt for the AI with all available context.
fn build_system_prompt(
    intent: &str,
    data_report: Option<&serde_json::Value>,
    templates: &[String],
) -> String {
    let mut prompt = format!(
        "You are a bioinformatics pipeline expert. Generate valid .oxoflow TOML configurations.\n\n\
         Intent: {intent}\n\n\
         Rules:\n\
         1. Output TOML in ```toml code fences\n\
         2. Use well-known bioinformatics tools with correct command-line syntax\n\
         3. Include [workflow] section with name, version, description\n\
         4. Define rules with name, shell, inputs, outputs, depends\n\
         5. Use {{sample}} wildcard for sample-varying paths\n\
         6. Specify conda environment for each rule when possible\n\
         7. Include resource hints (threads, memory) in [resources] section\n"
    );

    if let Some(report) = data_report {
        if let Some(summary) = report.get("summary") {
            prompt.push_str(&format!(
                "\nData summary: Formats={}, Paired-end={}\n",
                summary.get("formats_detected")
                    .and_then(|f| f.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default(),
                summary.get("paired_end_detected").and_then(|v| v.as_bool()).unwrap_or(false)
            ));
        }
        if let Some(sw) = report.get("suggested_workflow")
            && let Some(template) = sw.get("template").and_then(|v| v.as_str())
        {
                prompt.push_str(&format!("Suggested template: {template}\n"));
        }
    }

    if !templates.is_empty() {
        prompt.push_str(&format!(
            "\nAvailable templates for reference: {}\n",
            templates.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    prompt
}

/// Extract TOML content from an AI response (code fences or raw).
fn extract_toml_from_response(response: &str) -> Option<String> {
    if let Some(start) = response.find("```toml") {
        let start = start + 7;
        if let Some(end) = response[start..].find("```") {
            return Some(response[start..start + end].trim().to_string());
        }
    }
    if response.contains("[workflow]") {
        return Some(response.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_intent_rnaseq() {
        let intent = infer_intent("RNA-seq differential expression");
        assert_eq!(intent, "RNA-seq analysis");
    }

    #[test]
    fn test_infer_intent_variant() {
        let intent = infer_intent("WGS germline variant calling");
        assert_eq!(intent, "Variant calling");
    }

    #[test]
    fn test_infer_intent_qc() {
        let intent = infer_intent("run fastqc quality check");
        assert_eq!(intent, "Quality control");
    }

    #[test]
    fn test_extract_toml_fenced() {
        let response = "Here:\n```toml\n[workflow]\nname = \"test\"\n```\nDone";
        let toml = extract_toml_from_response(response);
        assert_eq!(toml, Some("[workflow]\nname = \"test\"".into()));
    }

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt("RNA-seq", None, &["rnaseq".into()]);
        assert!(prompt.contains("RNA-seq"));
        assert!(prompt.contains("rnaseq"));
        assert!(prompt.contains("[workflow]"));
    }
}
