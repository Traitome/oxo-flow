//! Issue #1040 P2-4: the chat surface must route conversational turns to a
//! plain-text completion and pipeline turns to generation.
//!
//! One test owns the process-wide AI registry (the scripted provider), so
//! the scenarios run sequentially inside a single `#[tokio::test]` — two
//! tests installing different providers would race.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_ai::scripted::{ScriptedTurn, scripted_provider};
use oxo_flow_web::server;
use serde_json::json;
use tower::ServiceExt;

const VALID_TOML: &str = "[workflow]\nname = \"scripted-pipeline\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"fastqc\"\ninput = [\"{sample}.fastq.gz\"]\noutput = [\"qc/{sample}_fastqc.html\"]\nshell = \"fastqc {input} -o qc/\"\n";

fn install(turns: Vec<ScriptedTurn>) {
    oxo_flow_ai::AI.set_provider(scripted_provider(turns));
}

async fn post(path: &str, body: serde_json::Value) -> (StatusCode, String) {
    let resp = server::build_router("personal")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn chat_intent_answers_in_text_and_pipeline_intent_still_generates() {
    // ── Scenario 1: the issue's exact message answers in text ──
    //
    // Exactly ONE scripted turn is available: if the classifier sent this
    // to generation, "OK" would fail validation and the correction loop
    // would exhaust the script — the request would fail instead of
    // returning a chat reply.
    install(vec![ScriptedTurn::content("OK")]);
    let (status, body) = post(
        "/api/chat/send/json",
        json!({"message": "Reply with the single word OK."}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let reply: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(reply["mode"], "chat", "body: {body}");
    assert_eq!(reply["message"], "OK", "body: {body}");
    assert!(
        reply.get("toml_content").is_none() && reply.get("pipeline_id").is_none(),
        "chat intent must not return a pipeline: {body}"
    );

    // ── Scenario 2: a pipeline phrase still generates and validates ──
    install(vec![ScriptedTurn::content(VALID_TOML)]);
    let (status, body) = post(
        "/api/chat/send/json",
        json!({"message": "Build an RNA-seq pipeline with FastQC"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let pipeline: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(pipeline["mode"], "pipeline", "body: {body}");
    assert!(
        pipeline["toml_content"]
            .as_str()
            .is_some_and(|toml| toml.contains("[workflow]")),
        "pipeline intent must keep returning toml_content: {body}"
    );
    assert_eq!(pipeline["intent"], "RNA-seq analysis", "body: {body}");
    assert!(
        pipeline.get("message").is_none(),
        "the pipeline path has no conversational text field: {body}"
    );
    assert!(pipeline["rules"].is_array(), "body: {body}");

    // ── Scenario 3: the SSE route makes the same decision ──
    install(vec![ScriptedTurn::content("OK")]);
    let (status, stream) = post(
        "/api/chat/send",
        json!({"message": "Reply with the single word OK."}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "stream: {stream}");
    assert!(
        stream.contains("\"mode\":\"chat\""),
        "ready/done must carry the classification: {stream}"
    );
    assert!(
        stream.contains("event: text") && stream.contains("\"chunk\":\"OK\""),
        "the chat reply must travel as a text event: {stream}"
    );
    assert!(
        !stream.contains("event: action") && !stream.contains("pipeline_ready"),
        "chat intent must not emit pipeline actions: {stream}"
    );
    assert!(
        stream.contains("\"message\":\"OK\""),
        "the terminal done payload must carry the text reply: {stream}"
    );
}
