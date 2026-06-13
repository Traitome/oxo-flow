use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateRequest {
    pub intent: String,
    pub context: Option<TranslateContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateContext {
    pub data_analysis_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateResponse {
    pub pipeline_id: String,
    pub toml_content: String,
    pub explanation: TranslateExplanation,
    pub alternatives: Vec<Alternative>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateExplanation {
    pub steps: Vec<TranslationStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationStep {
    pub rule: String,
    pub purpose: String,
    pub tool: String,
    pub key_params: String,
    pub why_chosen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alternative {
    pub description: String,
    pub diff_summary: String,
    pub tradeoffs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainRequest {
    pub run_id: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainResponse {
    pub summary: String,
    pub root_cause: Option<RootCause>,
    pub fix_suggestion: Option<FixSuggestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootCause {
    pub rule: String,
    pub error_type: String,
    pub evidence: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixSuggestion {
    pub action: String,
    pub automated: bool,
    pub estimated_impact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterpretRequest {
    pub run_id: String,
    pub result_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterpretResponse {
    pub narrative: String,
    pub highlights: Vec<Finding>,
    pub caveats: Vec<String>,
    pub suggested_next: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub finding: String,
    pub significance: String,
    pub supporting_evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeRequest {
    pub pipeline_id: String,
    pub goal: String,
    pub constraints: Option<OptimizeConstraints>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeConstraints {
    pub max_memory: Option<String>,
    pub max_threads: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeResponse {
    pub optimized_toml: String,
    pub changes: Vec<OptimizationChange>,
    pub estimated: OptimizationEstimate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationChange {
    pub rule: String,
    pub before: String,
    pub after: String,
    pub rationale: String,
    pub expected_impact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationEstimate {
    pub time_saved: String,
    pub memory_reduction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfigRequest {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfigResponse {
    pub provider: String,
    pub model: Option<String>,
    pub api_url: Option<String>,
    pub is_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTestResponse {
    pub success: bool,
    pub message: String,
    pub provider: String,
    pub model: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_request_roundtrip() {
        let req = TranslateRequest {
            intent: "run RNA-seq".into(),
            context: Some(TranslateContext {
                data_analysis_id: Some("da1".into()),
            }),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: TranslateRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.intent, req.intent);
    }

    #[test]
    fn test_translate_response_roundtrip() {
        let resp = TranslateResponse {
            pipeline_id: "p1".into(),
            toml_content: "[workflow]".into(),
            explanation: TranslateExplanation {
                steps: vec![TranslationStep {
                    rule: "step1".into(),
                    purpose: "align".into(),
                    tool: "star".into(),
                    key_params: "--genomeDir".into(),
                    why_chosen: "fast".into(),
                }],
            },
            alternatives: vec![Alternative {
                description: "use hisat2".into(),
                diff_summary: "different aligner".into(),
                tradeoffs: "slower but more accurate".into(),
            }],
            confidence: 0.95,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: TranslateResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pipeline_id, resp.pipeline_id);
    }

    #[test]
    fn test_explain_response_roundtrip() {
        let resp = ExplainResponse {
            summary: "run failed".into(),
            root_cause: Some(RootCause {
                rule: "rule1".into(),
                error_type: "OOM".into(),
                evidence: "memory exceeded".into(),
                confidence: 0.9,
            }),
            fix_suggestion: Some(FixSuggestion {
                action: "increase memory".into(),
                automated: false,
                estimated_impact: "high".into(),
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ExplainResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.summary, resp.summary);
    }

    #[test]
    fn test_interpret_response_roundtrip() {
        let resp = InterpretResponse {
            narrative: "results show".into(),
            highlights: vec![Finding {
                finding: "significant".into(),
                significance: "high".into(),
                supporting_evidence: "p-value 0.01".into(),
            }],
            caveats: vec!["small sample".into()],
            suggested_next: vec!["validate".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: InterpretResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.narrative, resp.narrative);
    }

    #[test]
    fn test_optimize_response_roundtrip() {
        let resp = OptimizeResponse {
            optimized_toml: "[workflow]".into(),
            changes: vec![OptimizationChange {
                rule: "rule1".into(),
                before: "threads=8".into(),
                after: "threads=4".into(),
                rationale: "reduce usage".into(),
                expected_impact: "slower but stable".into(),
            }],
            estimated: OptimizationEstimate {
                time_saved: "10%".into(),
                memory_reduction: "50%".into(),
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: OptimizeResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.optimized_toml, resp.optimized_toml);
    }

    #[test]
    fn test_ai_config_roundtrip() {
        let resp = AiConfigResponse {
            provider: "openai".into(),
            model: Some("gpt-4".into()),
            api_url: Some("https://api.openai.com".into()),
            is_configured: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: AiConfigResponse = serde_json::from_str(&json).unwrap();
        assert!(back.is_configured);
    }

    #[test]
    fn test_ai_test_response_roundtrip() {
        let resp = AiTestResponse {
            success: true,
            message: "connected".into(),
            provider: "openai".into(),
            model: Some("gpt-4".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: AiTestResponse = serde_json::from_str(&json).unwrap();
        assert!(back.success);
    }
}
