//! The web chat runs the REAL agent loop: a ScriptedBackend replays a
//! tool-call round (grounded lookup) and a final validated TOML, and the SSE
//! stream must carry typed tool_call / text / action / done events — not the
//! old fake progress narrative.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_ai::provider::AiProvider;
use oxo_flow_ai::scripted::{ScriptedTurn, scripted_provider};
use oxo_flow_ai::types::ToolCall;
use oxo_flow_web::server;
use serde_json::json;
use tower::ServiceExt;

const VALID_TOML: &str = "[workflow]\nname = \"scripted-pipeline\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"fastqc\"\ninput = [\"{sample}.fastq.gz\"]\noutput = [\"qc/{sample}_fastqc.html\"]\nshell = \"fastqc {input} -o qc/\"\n";

fn install_scripted_provider() {
    let toml = VALID_TOML;
    let provider: AiProvider = scripted_provider(vec![
        ScriptedTurn {
            content: None,
            tool_calls: Some(vec![ToolCall {
                id: "tc-1".into(),
                name: "lookup_tool".into(),
                arguments: json!({"query": "fastqc"}).to_string(),
            }]),
            error: None,
            delay_ms: 0,
        },
        ScriptedTurn {
            content: Some(format!("```toml\n{toml}```")),
            tool_calls: None,
            error: None,
            delay_ms: 0,
        },
    ]);
    oxo_flow_ai::AI.set_provider(provider);
}

#[tokio::test]
async fn chat_send_emits_typed_agent_events() {
    install_scripted_provider();

    let resp = server::build_router("personal")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat/send")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"message": "run fastqc on my samples"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);

    // Typed events from the real loop.
    assert!(
        text.contains("event: tool_call"),
        "missing tool_call: {text}"
    );
    assert!(
        text.contains("lookup_tool"),
        "the tool call must name the grounded lookup: {text}"
    );
    assert!(text.contains("event: tool_result"), "missing tool_result");
    assert!(text.contains("event: text"), "missing text event");
    assert!(text.contains("event: action"), "missing action event");
    assert!(
        text.contains("pipeline_ready"),
        "the action must be pipeline_ready: {text}"
    );
    assert!(
        text.contains("fastqc {input} -o qc/"),
        "the payload must carry the validated TOML: {text}"
    );
    assert!(text.contains("event: done"), "missing done event");
}
