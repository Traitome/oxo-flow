//! Webhook delivery end-to-end against a local mock receiver.
//!
//! `domains::observability::webhook::notify_terminal` is the exact code path
//! the executor calls on a terminal run state. These tests configure the
//! endpoint through the real `PUT /api/webhook` handler, insert a run row,
//! invoke the notifier, and assert on the bytes the receiver saw — method,
//! `Content-Type`, the documented `X-OxoFlow-Signature` HMAC, the payload
//! shape from `docs/guide/src/reference/webhook.md`, exact event
//! subscription, retries and failure surfacing.
//!
//! The mock receiver is a plain `TcpListener` HTTP/1.1 server on loopback:
//! no external network, no credentials.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use oxo_flow_web::infra::db::global_backend;
use oxo_flow_web::infra::db::models::RunRow;
use oxo_flow_web::server;
use serde_json::{Value, json};
use sha2::Sha256;
use tower::ServiceExt;

mod common;

const WORKFLOW_NAME: &str = "webhook-fixture";
const SECRET: &str = "local-mock-signing-secret";

// ---------------------------------------------------------------------------
// Mock receiver
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Received {
    method: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// HTTP/1.1 mock receiver. Each request pops the next scripted status (200
/// once the script is exhausted), optionally after `delay`.
struct MockReceiver {
    base_url: String,
    requests: Arc<Mutex<Vec<Received>>>,
}

impl MockReceiver {
    fn spawn(statuses: Vec<u16>, delay: Option<Duration>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock webhook receiver");
        let addr = listener.local_addr().expect("mock receiver addr");
        let requests: Arc<Mutex<Vec<Received>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let statuses = Arc::new(Mutex::new(VecDeque::from(statuses)));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                // One thread per connection: reqwest keeps idle connections
                // pooled, and a serial accept loop would block new clients
                // behind a lingering keep-alive read.
                let recorded = Arc::clone(&recorded);
                let statuses = Arc::clone(&statuses);
                std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                    while let Some(request) = read_request(&mut stream) {
                        recorded.lock().unwrap().push(request);
                        if let Some(delay) = delay {
                            std::thread::sleep(delay);
                        }
                        let status = statuses
                            .lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or(StatusCode::OK.as_u16());
                        let reason = match status {
                            200 => "OK",
                            500 => "Internal Server Error",
                            _ => "Status",
                        };
                        let response = format!(
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
                        );
                        if stream.write_all(response.as_bytes()).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Self {
            base_url: format!("http://{addr}/hook"),
            requests,
        }
    }

    fn requests(&self) -> Vec<Received> {
        self.requests.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// Read one HTTP request: request line + headers + `Content-Length` bytes.
fn read_request(stream: &mut TcpStream) -> Option<Received> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = find_subslice(&buffer, b"\r\n\r\n") {
            break position;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.lines();
    let method = lines.next()?.split_whitespace().next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            line.split_once(':')
                .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    let content_length = headers
        .iter()
        .find(|(key, _)| key == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length.min(body.len()));
    Some(Received {
        method,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

/// The webhook config row (id = 1) and the runs table are process-global in
/// this test binary; serialize the tests that mutate them.
fn test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn put_webhook(body: Value) -> (StatusCode, Value) {
    router_call(
        Request::builder()
            .method("PUT")
            .uri("/api/webhook")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn get_webhook() -> (StatusCode, Value) {
    router_call(
        Request::builder()
            .method("GET")
            .uri("/api/webhook")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn router_call(request: Request<Body>) -> (StatusCode, Value) {
    let response = server::build_router("personal")
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body");
    let value = serde_json::from_slice(&bytes).unwrap_or(json!(null));
    (status, value)
}

async fn configure(url: &str, events: &[&str]) {
    let (status, body) = put_webhook(json!({
        "enabled": true,
        "url": url,
        "secret": SECRET,
        "events": events,
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "PUT /api/webhook: {body:?}");
}

/// Insert a finished run row and return its id.
async fn insert_run(status: &str) -> String {
    let backend = global_backend().expect("storage backend");
    let run_id = format!("webhook-test-{}", uuid::Uuid::new_v4());
    let user = backend
        .create_user(&format!("webhook-user-{run_id}"), "user", None)
        .await
        .expect("insert owner user row");
    let started = "2026-11-08T10:00:00+00:00".to_string();
    let finished = "2026-11-08T10:00:02+00:00".to_string();
    backend
        .create_run(&RunRow {
            id: run_id.clone(),
            user_id: user.id,
            pipeline_id: None,
            pipeline_snapshot: "{}".to_string(),
            workflow_name: Some(WORKFLOW_NAME.to_string()),
            status: status.to_string(),
            phase: "executing".to_string(),
            dry_run: false,
            pid: None,
            workdir: Some(
                std::env::temp_dir()
                    .join(&run_id)
                    .to_string_lossy()
                    .to_string(),
            ),
            config_overrides: None,
            started_at: Some(started.clone()),
            finished_at: Some(finished),
            created_at: started,
        })
        .await
        .expect("insert run row");
    run_id
}

fn expected_signature(body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).expect("hmac key");
    mac.update(body.as_bytes());
    format!("hmac-sha256={}", hex::encode(mac.finalize().into_bytes()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terminal_webhook_matches_documented_contract() {
    let _guard = test_lock().lock().await;
    common::ensure_db().await;
    let receiver = MockReceiver::spawn(vec![200], None);
    configure(&receiver.base_url, &["workflow_completed"]).await;
    let run_id = insert_run("completed").await;

    oxo_flow_web::domains::observability::webhook::notify_terminal(&run_id, "completed").await;

    let requests = receiver.requests();
    assert_eq!(requests.len(), 1, "exactly one delivery: {requests:?}");
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.header("content-type"),
        Some("application/json"),
        "documented Content-Type"
    );

    // The signature is the RFC-2104 HMAC over the exact bytes received.
    let signature = request
        .header("x-oxoflow-signature")
        .expect("X-OxoFlow-Signature header");
    assert_eq!(signature, expected_signature(&request.body));
    assert!(signature.starts_with("hmac-sha256="), "{signature}");

    let payload: Value = serde_json::from_str(&request.body).expect("JSON payload");
    assert_eq!(payload["event"], "workflow_completed");
    assert_eq!(payload["workflow_name"], WORKFLOW_NAME);
    assert!(
        payload["timestamp"]
            .as_str()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .is_some(),
        "timestamp must be ISO 8601: {payload:?}"
    );
    assert_eq!(payload["data"]["total_rules"], 0);
    assert_eq!(payload["data"]["succeeded"], 0);
    assert_eq!(payload["data"]["failed"], 0);
    assert_eq!(payload["data"]["skipped"], 0);
    assert_eq!(payload["data"]["duration_ms"], 2000);
    assert!(
        payload["data"].get("rule").is_none(),
        "absent fields are omitted: {payload:?}"
    );
    assert_eq!(payload["version"], env!("CARGO_PKG_VERSION"));

    // GET reports the stored configuration without echoing the secret.
    let (status, settings) = get_webhook().await;
    assert_eq!(status, StatusCode::OK, "{settings:?}");
    assert_eq!(settings["enabled"], true);
    assert_eq!(settings["url"], receiver.base_url);
    assert_eq!(settings["secret_set"], true);
    assert!(settings.get("secret").is_none(), "{settings:?}");
    assert_eq!(settings["signature_scheme"], "hmac-sha256");
}

#[tokio::test]
async fn event_subscription_is_exact_per_terminal_event() {
    let _guard = test_lock().lock().await;
    common::ensure_db().await;

    // A completed-only subscription must stay silent on failure. Before the
    // fix the pre-filter only checked whether *either* terminal spelling was
    // configured, so this failed run was delivered.
    let receiver = MockReceiver::spawn(vec![200, 200], None);
    configure(&receiver.base_url, &["workflow_completed"]).await;
    let failed_run = insert_run("failed").await;
    oxo_flow_web::domains::observability::webhook::notify_terminal(&failed_run, "failed").await;
    assert_eq!(
        receiver.count(),
        0,
        "workflow_completed-only subscription received a failure event"
    );

    // A failed-only subscription stays silent on success, then delivers the
    // failure it asked for.
    configure(&receiver.base_url, &["workflow_failed"]).await;
    let completed_run = insert_run("completed").await;
    oxo_flow_web::domains::observability::webhook::notify_terminal(&completed_run, "completed")
        .await;
    assert_eq!(
        receiver.count(),
        0,
        "workflow_failed-only subscription received a success event"
    );
    oxo_flow_web::domains::observability::webhook::notify_terminal(&failed_run, "failed").await;
    let requests = receiver.requests();
    assert_eq!(requests.len(), 1, "the subscribed failure must deliver");
    let payload: Value = serde_json::from_str(&requests[0].body).expect("JSON payload");
    assert_eq!(payload["event"], "workflow_failed");
    assert_eq!(payload["data"]["error"], format!("Run {failed_run} failed"));

    // The documented default (both terminal spellings) keeps working.
    configure(
        &receiver.base_url,
        &["workflow_completed", "workflow_failed"],
    )
    .await;
    oxo_flow_web::domains::observability::webhook::notify_terminal(&completed_run, "completed")
        .await;
    assert_eq!(receiver.count(), 2);
}

#[tokio::test]
async fn retries_once_after_a_5xx_response() {
    let _guard = test_lock().lock().await;
    common::ensure_db().await;
    // The web layer configures max_retries = 1 with the documented 1s
    // backoff for the first retry.
    let receiver = MockReceiver::spawn(vec![500, 200], None);
    configure(
        &receiver.base_url,
        &["workflow_completed", "workflow_failed"],
    )
    .await;
    let run_id = insert_run("completed").await;

    let started = Instant::now();
    oxo_flow_web::domains::observability::webhook::notify_terminal(&run_id, "completed").await;
    let elapsed = started.elapsed();

    assert_eq!(
        receiver.count(),
        2,
        "500 must be retried exactly once (max_retries = 1)"
    );
    assert!(
        elapsed >= Duration::from_millis(900),
        "the retry must wait the documented 1s backoff, took {elapsed:?}"
    );
    for request in receiver.requests() {
        assert_eq!(
            request.header("x-oxoflow-signature").map(str::to_string),
            Some(expected_signature(&request.body)),
            "every attempt carries a valid signature"
        );
    }
}

/// A closed port produces a connection error; `notify_terminal` must surface
/// it as a warning (the documented best-effort contract) and return.
#[test]
fn connection_failure_is_surfaced_as_a_warning() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    // Hold the shared-state lock across setup and delivery. Acquiring it
    // inside a block_on and returning the guard keeps it alive for the rest
    // of the test (a nested block_on inside one runtime would panic).
    let _guard = runtime.block_on(test_lock().lock());
    let run_id = runtime.block_on(async {
        common::ensure_db().await;
        // Reserve and release a port so the connect is refused.
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let addr = listener.local_addr().expect("reserved addr");
        drop(listener);
        configure(&format!("http://{addr}/hook"), &["workflow_completed"]).await;
        insert_run("completed").await
    });

    let captured = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(captured.clone())
        .finish();
    let started = Instant::now();
    tracing::subscriber::with_default(subscriber, || {
        runtime.block_on(
            oxo_flow_web::domains::observability::webhook::notify_terminal(&run_id, "completed"),
        );
    });
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "the failed attempt must be retried once before giving up"
    );
    let log = captured.text();
    assert!(
        log.contains("webhook delivery failed"),
        "the delivery failure must be logged, not swallowed: {log:?}"
    );
}

/// The core client's timeout bounds a hung receiver and the error text never
/// echoes the (credentialed) webhook URL.
#[tokio::test]
async fn timeout_is_bounded_and_the_url_is_redacted() {
    use oxo_flow_core::webhook::{
        HttpMethod, SignatureScheme, WebhookClient, WebhookConfig, WebhookData, WebhookEvent,
        WebhookPayload,
    };

    let receiver = MockReceiver::spawn(vec![200], Some(Duration::from_secs(2)));
    let client = WebhookClient::new(WebhookConfig {
        url: receiver.base_url.clone(),
        method: HttpMethod::Post,
        headers: Default::default(),
        events: vec![WebhookEvent::WorkflowCompleted],
        secret: Some(SECRET.to_string()),
        signature_scheme: SignatureScheme::HmacSha256,
        timeout_secs: 1,
        max_retries: 0,
    });
    let payload = WebhookPayload {
        event: WebhookEvent::WorkflowCompleted,
        workflow_name: WORKFLOW_NAME.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        data: WebhookData::default(),
        version: "test".to_string(),
    };

    let started = Instant::now();
    let error = client
        .send(&payload)
        .await
        .expect_err("a 1s timeout must fail a 2s response");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(3),
        "timeout must fire at ~1s, took {elapsed:?}"
    );
    assert!(
        !error.to_string().contains(&receiver.base_url),
        "the credentialed webhook URL must not leak into the error: {error}"
    );
}

// ---------------------------------------------------------------------------
// Tracing capture
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct CapturedLog(Arc<Mutex<Vec<u8>>>);

impl CapturedLog {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).to_string()
    }
}

impl Write for CapturedLog {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = CapturedLog;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
