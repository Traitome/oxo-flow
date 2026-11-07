//! Successful web agent-loop runs must be archived, matching the documented
//! "every agent-loop run is persisted" contract (review wave 2 MEDIUM-2).
//!
//! The orchestrator archives FAILED generations itself; successful ones used
//! to leave no file at all, so the audit trail was missing exactly the runs
//! that produced output. This test drives the two web generation surfaces
//! with a scripted provider and asserts:
//!
//! - a successful pipeline-intent chat run leaves a `Success` archive,
//! - a conversational chat turn leaves NO archive (it never runs the agent
//!   loop — the documented carve-out), and
//! - a successful `/api/ai/translate` run leaves its own `Success` archive.
//!
//! Environment variables are process-global, so this binary is a single test
//! that owns them and points `OXO_FLOW_AI_SESSION_DIR` at scratch storage.
//! Inherited AI credentials are cleared: only the scripted provider may run.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_ai::scripted::{ScriptedTurn, scripted_provider};
use oxo_flow_web::server;
use serde_json::json;
use tower::ServiceExt;

const VALID_TOML: &str = "[workflow]\nname = \"archive-pipeline\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"fastqc\"\ninput = [\"reads.fastq.gz\"]\noutput = [\"qc/fastqc.html\"]\nshell = \"fastqc reads.fastq.gz -o qc/\"\n";

fn set_test_env(key: &str, value: &str) {
    // SAFETY: called once, before any provider/HTTP client is built and
    // before the test spawns request work.
    unsafe { std::env::set_var(key, value) };
}

/// Remove inherited credentials so the scripted provider is the only backend
/// this test can reach.
fn clear_inherited_ai_env() {
    for key in [
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_MODEL",
        "DEEPSEEK_API_KEY",
        "OLLAMA_HOST",
        "OXO_FLOW_AI_PROVIDER",
        "OXO_FLOW_AI_API_KEY",
        "OXO_FLOW_AI_API_URL",
        "OXO_FLOW_AI_MODEL",
    ] {
        // SAFETY: same single-threaded setup window as `set_test_env`.
        unsafe { std::env::remove_var(key) };
    }
}

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

/// All session archives currently on disk, sorted by path.
fn archives(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut paths = std::fs::read_dir(dir)
        .expect("archive dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_archive(path: &std::path::Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).expect("read archive");
    serde_json::from_str(&raw).expect("archive must be valid JSON")
}

#[tokio::test]
async fn successful_agent_runs_are_archived_and_chat_turns_are_not() {
    clear_inherited_ai_env();
    let root = std::env::temp_dir().join(format!("oxo-flow-archive-{}", std::process::id()));
    let sessions = root.join("sessions");
    std::fs::create_dir_all(&sessions).expect("scratch session dir");
    set_test_env("HOME", &root.to_string_lossy());
    set_test_env("OXO_FLOW_AI_SESSION_DIR", &sessions.to_string_lossy());

    // ── Pipeline-intent chat: a real agent-loop run that succeeds ──
    install(vec![ScriptedTurn::content(VALID_TOML)]);
    let (status, body) = post(
        "/api/chat/send/json",
        json!({"message": "Build an RNA-seq pipeline with FastQC"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let after_chat = archives(&sessions);
    assert_eq!(
        after_chat.len(),
        1,
        "a successful chat generation must be archived, got {after_chat:?}"
    );
    let chat_archive = read_archive(&after_chat[0]);
    assert_eq!(chat_archive["command"], json!("web-chat"));
    assert_eq!(chat_archive["outcome"], json!("Success"));
    assert!(
        chat_archive["ended_at"].is_string(),
        "a Success archive must be closed: {chat_archive}"
    );

    // ── Conversational chat: no agent loop, therefore no archive ──
    install(vec![ScriptedTurn::content("OK")]);
    let (status, body) = post(
        "/api/chat/send/json",
        json!({"message": "Reply with the single word OK."}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        archives(&sessions).len(),
        1,
        "a conversational turn must not create an agent-loop archive"
    );

    // ── Translate: the other web generation surface ──
    install(vec![ScriptedTurn::content(VALID_TOML)]);
    let (status, body) = post(
        "/api/ai/translate",
        json!({"intent": "archive-success-translate-sentinel"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let all = archives(&sessions);
    assert_eq!(all.len(), 2, "translate success must be archived: {all:?}");
    let translate_archive = all
        .iter()
        .map(|path| read_archive(path))
        .find(|archive| archive["command"] == json!("web-translate"))
        .expect("web-translate archive");
    assert_eq!(translate_archive["outcome"], json!("Success"));
}
