//! The web chat runs the REAL agent loop: a ScriptedBackend replays a
//! tool-call round (grounded lookup) and a final validated TOML, and the SSE
//! stream must carry typed tool_call / text / action / done events — not the
//! old fake progress narrative.
//!
//! Both scripted-provider scenarios live in ONE test: the AI registry is
//! process-wide, so parallel tests installing different providers would race.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_ai::scripted::{ScriptedTurn, scripted_provider};
use oxo_flow_ai::types::ToolCall;
use oxo_flow_web::server;
use serde_json::json;
use tower::ServiceExt;

const VALID_TOML: &str = "[workflow]\nname = \"scripted-pipeline\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"fastqc\"\ninput = [\"{sample}.fastq.gz\"]\noutput = [\"qc/{sample}_fastqc.html\"]\nshell = \"fastqc {input} -o qc/\"\n";

fn install_scripted_provider_with(turns: Vec<ScriptedTurn>) {
    oxo_flow_ai::AI.set_provider(scripted_provider(turns));
}

async fn post_chat(body: serde_json::Value) -> String {
    let resp = server::build_router("personal")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat/send")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn chat_agent_loop_emits_typed_events_and_registers_run_tools() {
    // ── Scenario 1: grounded tool call + validated TOML ──
    install_scripted_provider_with(vec![
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
            content: Some(format!("```toml\n{VALID_TOML}```")),
            tool_calls: None,
            error: None,
            delay_ms: 0,
        },
    ]);
    let text = post_chat(json!({"message": "run fastqc on my samples"})).await;
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

    // ── Scenario 2: run_id registers the read-only diagnosis tools ──
    install_scripted_provider_with(vec![
        ScriptedTurn {
            content: None,
            tool_calls: Some(vec![ToolCall {
                id: "tc-2".into(),
                name: "get_run_status".into(),
                arguments: "{}".into(),
            }]),
            error: None,
            delay_ms: 0,
        },
        ScriptedTurn {
            // Must validate as a workflow — the chat agent rejects non-TOML.
            content: Some(VALID_TOML.to_string()),
            tool_calls: None,
            error: None,
            delay_ms: 0,
        },
    ]);
    let text =
        post_chat(json!({"message": "why did my run fail", "run_id": "does-not-exist"})).await;
    assert!(
        text.contains("event: tool_call") && text.contains("get_run_status"),
        "run-scoped tools must be registered when run_id is present: {text}"
    );
    assert!(
        text.contains("run does-not-exist not found"),
        "tool errors must surface as honest tool_result summaries: {text}"
    );
}

/// Report Q&A and visualization must answer from the run's REAL data —
/// not the empty-report stub (B7).
#[tokio::test]
async fn report_ask_and_visualize_use_real_run_data() {
    // In-process DB with a run row whose workdir contains a real output file.
    let dir = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let path = format!("{dir}/report-qa-test.db");
    let _ = std::fs::remove_file(&path);
    let url = format!("sqlite:{path}?mode=rwc");
    oxo_flow_web::db::init_db(&url).await.ok();
    oxo_flow_web::infra::db::sqlite::init_pool(&url).await;

    let workdir = format!("{dir}/report-qa-workdir");
    let _ = std::fs::remove_dir_all(&workdir);
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(format!("{workdir}/results.txt"), "vcf output").unwrap();

    let pool = oxo_flow_web::infra::db::sqlite::pool();
    sqlx::query(
        "INSERT INTO runs (id, user_id, pipeline_snapshot, workflow_name, status, phase, pid, workdir, started_at, finished_at, created_at)
         VALUES ('qa-run-1', 'default', '[workflow]\nname = \"qa\"\n', 'qa', 'completed', 'executing', NULL, ?, NULL, '2026-01-01T00:01:00Z', '2026-01-01T00:00:00Z')",
    )
    .bind(&workdir)
    .execute(pool)
    .await
    .unwrap();

    let ask = server::build_router("personal")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs/qa-run-1/report/ask")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"question": "what files were produced"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ask.status(), StatusCode::OK);
    let answer = axum::body::to_bytes(ask.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let answer = String::from_utf8_lossy(&answer);
    assert!(
        answer.contains("1 output files"),
        "the answer must reflect the real run's file tree (was an empty stub): {answer}"
    );

    let viz = server::build_router("personal")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs/qa-run-1/report/visualize")
                .header("content-type", "application/json")
                .body(Body::from(json!({"type": "files"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viz.status(), StatusCode::OK);
    let body = axum::body::to_bytes(viz.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let viz_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = viz_json["data"]
        .as_array()
        .expect("visualization carries data");
    assert!(
        data.iter()
            .any(|d| d["name"].as_str() == Some("results.txt")),
        "visualization data must come from the run's files: {viz_json}"
    );
}
