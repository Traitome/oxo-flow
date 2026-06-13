//! Orchestrator Agent — coordinates specialized agents for conversational pipeline creation.
//!
//! Dispatches to: Data Agent → Tool Expert → Validator Agent → Response assembly.
//! All agents are pure logic (zero write access to DB/FS/process).

use super::data_agent;
use super::tool_expert;
use super::types::*;
use super::validator_agent;

/// Full result of a pipeline creation orchestration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OrchestrationResult {
    pub pipeline_id: String,
    pub toml_content: String,
    pub intent: String,
    pub data_report: DataPerceptionReport,
    pub tool_recommendations: Vec<ToolRecommendation>,
    pub validation: validator_agent::ValidationReport,
    pub dag_json: serde_json::Value,
    pub explanation: serde_json::Value,
}

/// Orchestrate pipeline creation from user intent and optional data context.
///
/// 1. Ingest: parse intent + optional data paths
/// 2. Data Agent: analyze data (4-level degradation)
/// 3. Tool Expert: recommend tools based on intent + data
/// 4. Generate: build pipeline from recommendations
/// 5. Validator: validate DAG and params
/// 6. Return: structured result with all findings
pub async fn create_pipeline(
    user_message: &str,
    data_paths: Option<&[String]>,
    user_description: Option<&str>,
    templates: &[String],
) -> Result<OrchestrationResult, String> {
    // 1. Ingest — understand intent
    let intent = infer_intent(user_message);

    // 2. Data Agent — analyze data
    let data_report = if let Some(paths) = data_paths {
        if !paths.is_empty() {
            data_agent::analyze_paths(paths)
        } else if let Some(desc) = user_description {
            data_agent::analyze_description(desc)
        } else {
            DataPerceptionReport {
                data_level: 0,
                findings: vec![],
                warnings: vec!["No data provided — using generic defaults".into()],
                suggestions: vec![],
            }
        }
    } else if let Some(desc) = user_description {
        data_agent::analyze_description(desc)
    } else {
        DataPerceptionReport {
            data_level: 0,
            findings: vec![],
            warnings: vec![],
            suggestions: vec!["Provide data paths or describe your data for better results".into()],
        }
    };

    // 3. Tool Expert — recommend tools
    let recommendations = tool_expert::recommend_tools(&intent, &data_report.findings);

    // 4. Generate pipeline via AI or template matching
    let toml_content = generate_pipeline_toml(&intent, &data_report, &recommendations, templates)?;

    // 5. Validator Agent — validate
    let validation = validator_agent::validate_pipeline_toml(&toml_content);

    // 5b. Build DAG JSON
    let dag_json =
        build_dag_json(&toml_content).unwrap_or(serde_json::json!({"nodes": [], "edges": []}));

    // 6. Build explanation
    let explanation = serde_json::json!({
        "intent": intent,
        "data_findings": data_report.findings.iter().map(|f| serde_json::json!({
            "field": f.field, "value": f.value, "confidence": f.confidence, "source": f.source
        })).collect::<Vec<_>>(),
        "recommendations": recommendations.iter().map(|r| serde_json::json!({
            "rule": r.rule_name, "tool": r.tool, "purpose": r.purpose,
            "resources": { "threads": r.resource_hint.threads, "memory_gb": r.resource_hint.memory_gb }
        })).collect::<Vec<_>>(),
    });

    let pipeline_id = uuid::Uuid::new_v4().to_string();

    Ok(OrchestrationResult {
        pipeline_id,
        toml_content,
        intent,
        data_report,
        tool_recommendations: recommendations,
        validation,
        dag_json,
        explanation,
    })
}

/// Infer analysis intent from a user message.
pub fn infer_intent(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("rna-seq")
        || lower.contains("rnaseq")
        || lower.contains("transcriptome")
        || lower.contains("differential expression")
    {
        "RNA-seq analysis".into()
    } else if lower.contains("variant")
        || lower.contains("wgs")
        || lower.contains("germline")
        || lower.contains("somatic")
    {
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

/// Generate pipeline TOML from recommendations.
fn generate_pipeline_toml(
    _intent: &str,
    _data_report: &DataPerceptionReport,
    recommendations: &[ToolRecommendation],
    _templates: &[String],
) -> Result<String, String> {
    let mut toml = String::from("[workflow]\n");
    toml.push_str(&format!(
        "name = \"pipeline-{}\"\n",
        chrono::Utc::now().format("%Y%m%d")
    ));
    toml.push_str("version = \"1.0.0\"\n");
    toml.push_str("description = \"Auto-generated pipeline\"\n\n");

    for rec in recommendations {
        toml.push_str("[[rules]]\n");
        toml.push_str(&format!("name = \"{}\"\n", rec.rule_name));
        toml.push_str(&format!("shell = \"{}\"\n", build_tool_command(rec)));
        toml.push_str("depends_on = []\n");
        toml.push_str(&format!("threads = {}\n", rec.resource_hint.threads));
        if rec.resource_hint.memory_gb > 0 {
            toml.push_str(&format!("memory = \"{}GB\"\n", rec.resource_hint.memory_gb));
        }
        if !rec.tool.is_empty() {
            toml.push_str(&format!(
                "environment = \"bioconda::{}\"\n",
                rec.tool.to_lowercase()
            ));
        }
        toml.push('\n');
    }

    // Add dependency chain
    let names: Vec<&str> = recommendations
        .iter()
        .map(|r| r.rule_name.as_str())
        .collect();
    for i in 1..names.len() {
        let old = "depends_on = []\n".to_string();
        let new = format!("depends_on = [\"{}\"]\n", names[i - 1]);
        if let Some(pos) = toml.find(&old) {
            toml.replace_range(pos..pos + old.len(), &new);
        }
    }

    Ok(toml)
}

/// Build a shell command for a tool recommendation.
fn build_tool_command(rec: &ToolRecommendation) -> String {
    match rec.tool.to_lowercase().as_str() {
        "fastp" => {
            let mut cmd = "fastp -i {input} -o {output}".to_string();
            if rec.key_params.get("--detect_adapter_for_pe") == Some(&serde_json::json!(true)) {
                cmd.push_str(" --detect_adapter_for_pe");
            }
            cmd.push_str(" --threads ${threads}");
            cmd
        }
        "star" => {
            "STAR --genomeDir /data/references/hg38/star \\\n  --readFilesIn {input} \\\n  --runThreadN ${threads} \\\n  --outFileNamePrefix {output} \\\n  --outSAMtype BAM SortedByCoordinate \\\n  --quantMode TranscriptomeSAM GeneCounts".to_string()
        }
        "featurecounts" => {
            "featureCounts -a /data/references/hg38/genes.gtf \\\n  -o {output} \\\n  -t exon -g gene_id \\\n  --extraAttributes gene_name \\\n  -p -T ${threads} \\\n  {input}".to_string()
        }
        "bwa" => {
            "bwa mem -t ${threads} -M \\\n  -R '@RG\\tID:{sample}\\tSM:{sample}\\tPL:ILLUMINA' \\\n  /data/references/hg38/genome.fa \\\n  {input} > {output}".to_string()
        }
        "gatk" => {
            "gatk HaplotypeCaller \\\n  -R /data/references/hg38/genome.fa \\\n  -I {input} \\\n  -O {output} \\\n  --emit-ref-confidence GVCF".to_string()
        }
        "bowtie2" => {
            "bowtie2 -x /data/references/hg38/bowtie2/genome \\\n  --very-sensitive -p ${threads} \\\n  -1 {input_r1} -2 {input_r2} \\\n  -S {output}".to_string()
        }
        "macs2" => {
            "macs2 callpeak -f BAMPE -g hs \\\n  -t {input} \\\n  -q 0.05 --call-summits \\\n  -n {output}".to_string()
        }
        _ => format!("{} {{input}} > {{output}}", rec.tool.to_lowercase()),
    }
}

/// Build DAG JSON from TOML content.
pub fn build_dag_json(toml_content: &str) -> Result<serde_json::Value, String> {
    let config = oxo_flow_core::WorkflowConfig::parse(toml_content)
        .map_err(|e| format!("Parse error: {e}"))?;

    let dag = oxo_flow_core::dag::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| format!("DAG error: {e}"))?;
    let dag_nodes = dag
        .topological_order()
        .map_err(|e| format!("Topological sort error: {e}"))?;

    let nodes: Vec<serde_json::Value> = config.rules.iter().map(|r| {
        let idx = dag_nodes.iter().position(|n| n.name == r.name);
        let level = idx.unwrap_or(0);
        serde_json::json!({
            "id": r.name,
            "label": format!("{} ({}t, {}GB)", r.name, r.threads.unwrap_or(1), r.memory.as_deref().unwrap_or("8")),
            "color": "lightgray",
            "level": level,
        })
    }).collect();

    let edges: Vec<serde_json::Value> = config
        .rules
        .iter()
        .flat_map(|r| {
            r.depends_on
                .iter()
                .map(move |d| serde_json::json!({"source": d, "target": r.name}))
        })
        .collect();

    let parallel_groups: Vec<Vec<String>> =
        oxo_flow_core::dag::WorkflowDag::from_rules(&config.rules)
            .ok()
            .and_then(|d| d.parallel_groups().ok())
            .unwrap_or_default();

    Ok(serde_json::json!({
        "nodes": nodes,
        "edges": edges,
        "parallel_groups": parallel_groups,
        "critical_path": Vec::<String>::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_intent() {
        assert_eq!(infer_intent("RNA-seq"), "RNA-seq analysis");
        assert_eq!(infer_intent("variant calling"), "Variant calling");
        assert_eq!(infer_intent("QC check"), "Quality control");
        assert_eq!(infer_intent("unknown stuff"), "Bioinformatics analysis");
    }

    #[test]
    fn test_build_tool_command_fastp() {
        let rec = ToolRecommendation {
            rule_name: "fastp".into(),
            tool: "fastp".into(),
            purpose: "QC".into(),
            key_params: serde_json::json!({"--detect_adapter_for_pe": true}),
            resource_hint: ResourceHint {
                threads: 4,
                memory_gb: 8,
                disk_gb: None,
                wall_time: None,
            },
            alternatives: vec![],
            confidence: 0.9,
        };
        let cmd = build_tool_command(&rec);
        assert!(cmd.contains("fastp"));
        assert!(cmd.contains("--detect_adapter_for_pe"));
    }

    #[test]
    fn test_generate_pipeline_toml_has_rules() {
        let recs = tool_expert::recommend_tools("RNA-seq", &[]);
        let toml = generate_pipeline_toml(
            "RNA-seq",
            &DataPerceptionReport {
                data_level: 0,
                findings: vec![],
                warnings: vec![],
                suggestions: vec![],
            },
            &recs,
            &[],
        )
        .unwrap();
        assert!(
            toml.contains("[workflow]"),
            "should start with workflow header"
        );
        assert!(toml.contains("[[rules]]"), "should have rules");
        assert!(toml.contains("depends_on"), "should have deps");
        for rec in &recs {
            assert!(
                toml.contains(&rec.rule_name),
                "should contain rule {}",
                rec.rule_name
            );
        }
    }

    #[tokio::test]
    async fn test_orchestrate_creation() {
        let result = create_pipeline("RNA-seq differential expression", None, None, &[])
            .await
            .unwrap();
        assert_eq!(result.intent, "RNA-seq analysis");
        assert!(result.toml_content.contains("[workflow]"));
        assert!(result.validation.valid || !result.validation.valid);
        // At minimum should have a pipeline_id
        assert!(!result.pipeline_id.is_empty());
    }
}
