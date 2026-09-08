//! Regression: `POST /api/files` must never answer 200 with a truncated file.
//!
//! Axum's `Multipart` extractor applies `DefaultBodyLimit` (2 MiB) unless the
//! route overrides it. The handler's `while let Ok(Some(chunk))` loop then
//! ended silently on the body-limit error and still reported success with the
//! truncated byte count (audit finding C2). This test uploads a 5 MiB file
//! through the real router and pins both the reported and the stored size.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

const PAYLOAD_BYTES: usize = 5 * 1024 * 1024;

/// This binary contains exactly one test, so mutating the process-global
/// workspace root cannot race another thread. SAFETY: no other thread is
/// running when `set_var` executes.
fn set_workspace_root(root: &str) {
    unsafe { std::env::set_var("OXO_FLOW_WORKSPACE", root) };
}

async fn ensure_db(url: &str) {
    oxo_flow_web::db::init_db(url).await.ok();
    oxo_flow_web::infra::db::sqlite::init_pool(url).await;
}

#[tokio::test]
async fn upload_larger_than_default_body_limit_is_stored_whole() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let ws = format!("{tmp}/upload-limit-workspace");
    let _ = std::fs::remove_dir_all(&ws);
    set_workspace_root(&ws);

    let db = format!("sqlite:{tmp}/upload-limit-test.db?mode=rwc");
    let _ = std::fs::remove_file(format!("{tmp}/upload-limit-test.db"));
    ensure_db(&db).await;

    // Hand-built multipart body: a `path` field plus one file field.
    let boundary = "oxoflow-test-boundary";
    let payload = vec![b'x'; PAYLOAD_BYTES];
    let mut body: Vec<u8> = Vec::with_capacity(PAYLOAD_BYTES + 512);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\naudit\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"big.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&payload);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let app = server::build_router("personal");
    let req = Request::builder()
        .method("POST")
        .uri("/api/files")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "upload response is not JSON ({e}): {}",
            String::from_utf8_lossy(&bytes)
        )
    });

    assert_eq!(status, StatusCode::OK, "upload must succeed: {value}");
    assert_eq!(
        value["files"][0]["size_bytes"].as_u64(),
        Some(PAYLOAD_BYTES as u64),
        "reported size must be the whole upload, not a truncated prefix: {value}"
    );

    let stored = std::path::Path::new(&ws).join("users/default/inputs/audit/big.bin");
    let meta = std::fs::metadata(&stored)
        .unwrap_or_else(|e| panic!("uploaded file missing at {}: {e}", stored.display()));
    assert_eq!(
        meta.len(),
        PAYLOAD_BYTES as u64,
        "stored file must carry every uploaded byte"
    );

    let _ = std::fs::remove_dir_all(&ws);
}
