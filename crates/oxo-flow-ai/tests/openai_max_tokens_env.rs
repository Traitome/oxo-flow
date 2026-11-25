//! `OXO_FLOW_AI_MAX_TOKENS` must reach the OpenAI-compatible wire ONLY when
//! explicitly set (campaign Task 8 review Minor 6).
//!
//! The unit tests exercise `build_body_with_budget` with explicit arguments;
//! this binary owns the process environment and proves the env → body gate
//! end to end against a local mock endpoint (no paid calls).

use std::io::{Read, Write};
use std::net::TcpListener;

use oxo_flow_ai::provider::{AiProvider, OpenAiBackend};

fn set_env(key: &str, value: &str) {
    // SAFETY: this binary has one test; variables are set before any request
    // and no concurrent reader exists.
    unsafe { std::env::set_var(key, value) };
}

fn unset_env(key: &str) {
    // SAFETY: same single-threaded setup window as `set_env`.
    unsafe { std::env::remove_var(key) };
}

/// Read one HTTP request body from a mock OpenAI endpoint.
fn read_request_body(stream: &mut std::net::TcpStream) -> String {
    let mut received = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return String::new(),
            Ok(read) => received.extend_from_slice(&chunk[..read]),
        }
        let Some(head_end) = received.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&received[..head_end]);
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let body_start = head_end + 4;
        while received.len() < body_start + content_length {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => received.extend_from_slice(&chunk[..read]),
            }
        }
        return String::from_utf8_lossy(&received[body_start..]).into_owned();
    }
}

/// Serve exactly one chat-completion request and return its JSON body.
fn serve_once() -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock provider");
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().expect("mock addr")
    );
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock accept");
        let body = read_request_body(&mut stream);
        let response = r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response.len(),
            response
        );
        stream
            .write_all(reply.as_bytes())
            .expect("mock response write");
        body
    });
    (url, handle)
}

#[tokio::test]
async fn openai_max_tokens_follows_the_env_knob() {
    unset_env("OXO_FLOW_AI_CONTEXT_TOKENS");

    // Explicitly set: the field is sent.
    set_env("OXO_FLOW_AI_MAX_TOKENS", "32000");
    let (url, handle) = serve_once();
    let provider = AiProvider::OpenAi(OpenAiBackend::new(
        "test-key".into(),
        Some("qwen3.8-flash-next-nothink".into()),
        Some(url),
    ));
    provider
        .chat("You are a helpful assistant.", "Reply with exactly: OK")
        .await
        .expect("mock provider must answer");
    let body = handle.join().expect("mock server thread");
    assert!(
        body.contains("\"max_tokens\":32000"),
        "explicit OXO_FLOW_AI_MAX_TOKENS must reach the wire: {body}"
    );

    // Unset: the field is omitted (historical contract).
    unset_env("OXO_FLOW_AI_MAX_TOKENS");
    let (url, handle) = serve_once();
    let provider = AiProvider::OpenAi(OpenAiBackend::new(
        "test-key".into(),
        Some("qwen3.8-flash-next-nothink".into()),
        Some(url),
    ));
    provider
        .chat("You are a helpful assistant.", "Reply with exactly: OK")
        .await
        .expect("mock provider must answer");
    let body = handle.join().expect("mock server thread");
    assert!(
        !body.contains("\"max_tokens\""),
        "unset max-token knob must omit the field: {body}"
    );

    // Invalid explicit values stay omitted rather than imposing a budget.
    set_env("OXO_FLOW_AI_MAX_TOKENS", "not-a-number");
    let (url, handle) = serve_once();
    let provider = AiProvider::OpenAi(OpenAiBackend::new(
        "test-key".into(),
        Some("qwen3.8-flash-next-nothink".into()),
        Some(url),
    ));
    provider
        .chat("You are a helpful assistant.", "Reply with exactly: OK")
        .await
        .expect("mock provider must answer");
    let body = handle.join().expect("mock server thread");
    assert!(
        !body.contains("\"max_tokens\""),
        "invalid max-token knob must not impose a budget: {body}"
    );
}
