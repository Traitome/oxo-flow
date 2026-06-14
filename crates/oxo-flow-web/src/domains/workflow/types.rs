use serde::{Deserialize, Serialize};

/// Request to parse a TOML workflow into a structured pipeline representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseRequest {
    pub toml_content: String,
    pub format_version: Option<String>,
}

/// Result of parsing a workflow: pipeline identity, rules, DAG, and statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseResponse {
    pub pipeline_id: String,
    pub name: String,
    pub version: String,
    pub rules: Vec<RuleSummary>,
    pub dag: DagJsonResponse,
    pub stats: WorkflowStatsResponse,
}

/// Summary of a single rule in a pipeline (name, I/O, environment, threads).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSummary {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub environment: Option<String>,
    pub threads: Option<u32>,
}

/// A node in the DAG visualization (id, label, color-coded status).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagJsonNode {
    pub id: String,
    pub label: String,
    pub color: String,
}

/// A directed edge between two DAG nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagJsonEdge {
    pub from: String,
    pub to: String,
}

/// Full DAG representation: nodes, edges, parallel groups, critical path, metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagJsonResponse {
    pub nodes: Vec<DagJsonNode>,
    pub edges: Vec<DagJsonEdge>,
    pub parallel_groups: Vec<Vec<String>>,
    pub critical_path: Vec<String>,
    pub metrics: DagMetrics,
}

/// Aggregate DAG metrics: node/edge counts, depth, width, and parallelism.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagMetrics {
    pub node_count: usize,
    pub edge_count: usize,
    pub max_depth: usize,
    pub max_width: usize,
    pub critical_path_length: usize,
    pub parallel_group_count: usize,
}

/// Aggregate workflow statistics (rules, dependencies, environments, wildcards).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStatsResponse {
    pub rule_count: usize,
    pub shell_rules: usize,
    pub script_rules: usize,
    pub dependency_count: usize,
    pub parallel_groups: usize,
    pub max_depth: usize,
    pub environments: Vec<String>,
    pub total_threads: u32,
    pub wildcard_count: usize,
    pub wildcard_names: Vec<String>,
}

/// Request to validate a pipeline's DAG for structural issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateRequest {
    pub toml_content: String,
    pub pipeline_id: Option<String>,
}

/// Validation result (empty errors = valid pipeline).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
}

/// A single validation error with error code, message, and optional fix suggestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    pub rule: Option<String>,
    pub suggestion: Option<String>,
}

/// Request to prepare a pipeline: expand wildcards and resolve environments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareRequest {
    pub toml_content: String,
    pub resolve_wildcards: Option<bool>,
    pub apply_defaults: Option<bool>,
    pub pipeline_id: Option<String>,
}

/// Result of pipeline preparation: expanded rules, wildcard combinations, env commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareResponse {
    pub pipeline_id: String,
    pub expanded_rules_count: usize,
    pub wildcard_combinations: usize,
    pub environment_setup_cmds: Vec<String>,
}

/// Request to diff two pipelines by their TOML content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffRequest {
    pub toml_a: String,
    pub toml_b: String,
    pub pipeline_a_id: Option<String>,
    pub pipeline_b_id: Option<String>,
}

/// A single structural difference between two pipeline versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEntry {
    pub path: String,
    pub category: String,
    pub description: String,
    pub severity: String,
}

/// Result of diffing two pipelines: a list of structural differences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResponse {
    pub diffs: Vec<DiffEntry>,
}

/// Request to search pipelines by query and optional scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub scope: Option<String>,
}

/// A single search result with relevance score and match reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub name: String,
    pub source: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub match_reason: String,
    pub score: f64,
}

/// Search results with query echo and total count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub total: usize,
    pub results: Vec<SearchResult>,
}

/// Request to export a pipeline as Dockerfile or Singularity definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRequest {
    pub toml_content: String,
    pub format: Option<String>,
    pub pipeline_id: Option<String>,
}

/// Exported container definition (format + content string).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportResponse {
    pub format: String,
    pub content: String,
}

/// Canonical TOML formatting result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatResponse {
    pub formatted: String,
}

/// A saved pipeline with full metadata, ownership, visibility, and TOML content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub version: String,
    pub toml_content: String,
    pub rules_count: usize,
    pub forked_from: Option<String>,
    pub visibility: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Filter criteria for listing templates (category, tags, search text).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateFilter {
    pub category: Option<String>,
    pub tags: Option<Vec<String>>,
    pub search: Option<String>,
}

/// A pipeline template with metadata, tags, usage count, and optional TOML content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub tags: Vec<String>,
    pub toml_content: Option<String>,
    pub is_system: bool,
    pub created_by: Option<String>,
    pub usage_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

// ---- Plugin validation types ----

/// Request to validate a plugin manifest and optionally verify its signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatePluginRequest {
    /// Plugin manifest as a JSON object.
    pub manifest: oxo_flow_core::plugin::PluginManifest,
    /// Optional trusted keys for signature verification (key_id -> hex-encoded key).
    pub trusted_keys: Option<std::collections::HashMap<String, String>>,
}

/// Plugin validation result: validity, parsed metadata, signature status, and errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatePluginResponse {
    /// Whether the manifest is valid.
    pub valid: bool,
    /// Parsed plugin name.
    pub name: Option<String>,
    /// Parsed plugin version.
    pub version: Option<String>,
    /// Parsed plugin type.
    pub plugin_type: Option<String>,
    /// Signature verification result.
    pub signature_valid: Option<bool>,
    /// List of validation errors.
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_request_roundtrip() {
        let req = ParseRequest {
            toml_content: "[workflow]\nname = \"test\"".into(),
            format_version: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ParseRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.toml_content, req.toml_content);
    }

    #[test]
    fn test_parse_response_roundtrip() {
        let resp = ParseResponse {
            pipeline_id: "p1".into(),
            name: "test".into(),
            version: "1.0".into(),
            rules: vec![RuleSummary {
                name: "rule1".into(),
                inputs: vec!["in.txt".into()],
                outputs: vec!["out.txt".into()],
                environment: Some("conda".into()),
                threads: Some(4),
            }],
            dag: DagJsonResponse {
                nodes: vec![DagJsonNode {
                    id: "n1".into(),
                    label: "Rule 1".into(),
                    color: "#ff0".into(),
                }],
                edges: vec![DagJsonEdge {
                    from: "n1".into(),
                    to: "n2".into(),
                }],
                parallel_groups: vec![vec!["n1".into()]],
                critical_path: vec!["n1".into()],
                metrics: DagMetrics {
                    node_count: 1,
                    edge_count: 1,
                    max_depth: 1,
                    max_width: 1,
                    critical_path_length: 1,
                    parallel_group_count: 1,
                },
            },
            stats: WorkflowStatsResponse {
                rule_count: 1,
                shell_rules: 1,
                script_rules: 0,
                dependency_count: 1,
                parallel_groups: 1,
                max_depth: 1,
                environments: vec!["conda".into()],
                total_threads: 4,
                wildcard_count: 0,
                wildcard_names: vec![],
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ParseResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pipeline_id, resp.pipeline_id);
    }

    #[test]
    fn test_validate_response_roundtrip() {
        let resp = ValidateResponse {
            valid: false,
            errors: vec![ValidationError {
                code: "E001".into(),
                message: "missing input".into(),
                rule: Some("rule1".into()),
                suggestion: Some("add input".into()),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ValidateResponse = serde_json::from_str(&json).unwrap();
        assert!(!back.valid);
        assert_eq!(back.errors[0].code, "E001");
    }

    #[test]
    fn test_prepare_response_roundtrip() {
        let resp = PrepareResponse {
            pipeline_id: "p1".into(),
            expanded_rules_count: 10,
            wildcard_combinations: 5,
            environment_setup_cmds: vec!["conda activate".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: PrepareResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pipeline_id, resp.pipeline_id);
    }

    #[test]
    fn test_diff_response_roundtrip() {
        let resp = DiffResponse {
            diffs: vec![DiffEntry {
                path: "rule1".into(),
                category: "env".into(),
                description: "env changed".into(),
                severity: "low".into(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: DiffResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.diffs.len(), 1);
    }

    #[test]
    fn test_search_response_roundtrip() {
        let resp = SearchResponse {
            query: "test".into(),
            total: 1,
            results: vec![SearchResult {
                id: "s1".into(),
                name: "result1".into(),
                source: "local".into(),
                category: Some("pipeline".into()),
                description: Some("desc".into()),
                tags: Some(vec!["rna".into()]),
                match_reason: "name match".into(),
                score: 0.95,
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: SearchResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total, 1);
    }

    #[test]
    fn test_pipeline_roundtrip() {
        let p = Pipeline {
            id: "p1".into(),
            user_id: "u1".into(),
            name: "test".into(),
            version: "1.0".into(),
            toml_content: "content".into(),
            rules_count: 5,
            forked_from: None,
            visibility: "public".into(),
            created_at: "2024-01-01".into(),
            updated_at: "2024-01-02".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Pipeline = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, p.id);
    }

    #[test]
    fn test_template_roundtrip() {
        let t = Template {
            id: "t1".into(),
            name: "RNA-seq".into(),
            category: "bioinformatics".into(),
            description: "RNA-seq pipeline".into(),
            tags: vec!["rna".into(), "seq".into()],
            toml_content: Some("[workflow]".into()),
            is_system: true,
            created_by: Some("admin".into()),
            usage_count: 42,
            created_at: "2024-01-01".into(),
            updated_at: "2024-01-02".into(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: Template = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, t.id);
    }
}
