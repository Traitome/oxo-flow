//! Regression: staged-upload temp files from a crashed process must not be
//! advertised to the user, nor mirrored into a run directory (issue #1161).
//!
//! The upload handler streams into `dest.<seq>.<pid>.tmp` beside the
//! destination and `rename`s it over only on success. A process killed
//! mid-stream (or during the rename window) leaves that temp behind; before
//! the fix `GET /api/files` listed it as a phantom input and
//! `stage_user_inputs` copied it into every run directory. Both surfaces now
//! recognise the staged shape (`is_staged_upload_tmp`) and the listing also
//! sweeps temps whose owning process is gone.
//!
//! One test per binary on purpose (same discipline as `upload_limit.rs` and
//! `upload_atomic_replace.rs`): `OXO_FLOW_WORKSPACE` is process-global.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

mod common;

const BOUNDARY: &str = "oxoflow-tmpleft-boundary";

/// SAFETY: set once at test start, before any workspace read; this binary's
/// only test is the sole reader.
fn set_workspace_root(root: &str) {
    unsafe { std::env::set_var("OXO_FLOW_WORKSPACE", root) };
}

fn multipart_body(filename: &str, content: &[u8]) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::with_capacity(content.len() + 512);
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn upload_request(body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/files")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body)
        .unwrap()
}

fn list_request() -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/api/files")
        .body(Body::empty())
        .unwrap()
}

async fn response_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "response is not JSON ({e}): {}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

#[tokio::test]
async fn crashed_upload_temp_is_hidden_and_not_staged() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let ws = format!("{tmp}/upload-tmpleft-workspace");
    let _ = std::fs::remove_dir_all(&ws);
    set_workspace_root(&ws);
    common::ensure_db().await;

    let app = server::build_router("personal");

    // 1. A clean upload lands normally.
    let resp = app
        .clone()
        .oneshot(upload_request(Body::from(multipart_body(
            "keep.bin",
            b"good bytes",
        ))))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 2. Simulate the crash: the temp a killed process leaves beside the
    //    destination, stamped with a PID that will never come back.
    let inputs = std::path::Path::new(&ws).join("users/default/inputs");
    let orphan = inputs.join("sample.bam.1.999999.tmp");
    std::fs::write(&orphan, b"half a file").unwrap();

    // 3. Listing must not advertise the orphan...
    let resp = app.clone().oneshot(list_request()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = response_json(resp).await;
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(
        names.contains(&"keep.bin"),
        "the real upload is still listed (got: {names:?})"
    );
    assert!(
        !names.iter().any(|n| n.ends_with(".tmp")),
        "no staged temp may appear in the listing (got: {names:?})"
    );
    assert!(!orphan.exists(), "the orphan is swept from disk");

    // ...and neither is it mirrored into a run directory.
    let run_dir = std::path::Path::new(&ws).join("users/default/runs/r1");
    std::fs::create_dir_all(&run_dir).unwrap();
    let copied = oxo_flow_web::workspace::stage_user_inputs("default", &run_dir).unwrap();
    assert_eq!(copied, 1, "only the real upload is staged");
    assert!(run_dir.join("keep.bin").is_file());
    assert!(
        !run_dir.join("sample.bam.1.999999.tmp").exists(),
        "the staged temp must not reach a run directory"
    );

    let _ = std::fs::remove_dir_all(&ws);
}
