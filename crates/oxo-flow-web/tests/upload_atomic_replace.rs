//! Regression: a failed re-upload must not destroy the previously-good file.
//!
//! The handler used to `File::create(&dest)` (truncating the destination)
//! before the size cap and the full read were validated, then `remove_file`d
//! that same path on the failure paths — so an oversize or aborted re-upload
//! of an existing name erased the good file (audit ledger RB-08). The upload
//! now streams into a sibling temp file and only `rename`s it over `dest`
//! once validation passes; a failure removes the temp and leaves `dest`
//! byte-identical.
//!
//! One test per binary on purpose (same discipline as `upload_limit.rs`):
//! `OXO_FLOW_WORKSPACE` is process-global, so a second concurrent test could
//! delete the first one's workspace out from under it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oxo_flow_web::server;
use serde_json::Value;
use tower::ServiceExt;

mod common;

const BOUNDARY: &str = "oxoflow-atomic-boundary";

/// SAFETY: set once at test start, before any workspace read; this binary's
/// only test is the sole reader.
fn set_workspace_root(root: &str) {
    unsafe { std::env::set_var("OXO_FLOW_WORKSPACE", root) };
}

/// A multipart body carrying one file field with `content`.
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

fn tmp_siblings(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".tmp"))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn failed_reupload_preserves_the_previous_file_and_clean_reupload_replaces_it() {
    let tmp = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let ws = format!("{tmp}/upload-atomic-workspace");
    let _ = std::fs::remove_dir_all(&ws);
    set_workspace_root(&ws);
    common::ensure_db().await;

    let app = server::build_router("personal");
    let dest = std::path::Path::new(&ws).join("users/default/inputs/keep.bin");
    let original = b"original good bytes that must survive";

    // 1. A clean upload establishes the good file.
    let resp = app
        .clone()
        .oneshot(upload_request(Body::from(multipart_body(
            "keep.bin", original,
        ))))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "seeding upload must succeed");
    assert_eq!(std::fs::read(&dest).unwrap(), original);

    // 2. A re-upload of the same name whose body stream aborts mid-flight:
    //    the stream delivers the part header, then the first data chunk, and
    //    only then errors — so the handler is already streaming into the temp
    //    when the read fails. That is the path that used to truncate `dest` up
    //    front and delete it again (audit ledger RB-08). multer's `poll_stream`
    //    drains the body until it pends, so an `Ok` immediately followed by
    //    `Err` would surface from `next_field` instead; the `yield_now`s force
    //    the abort to be observed from `field.chunk()`, the mid-write path.
    let mut head = multipart_body("keep.bin", &[]);
    head.truncate(head.len() - format!("\r\n--{BOUNDARY}--\r\n").len());
    let chunk = vec![b'y'; 64 * 1024];
    let streamed = async_stream::stream! {
        yield Ok::<Vec<u8>, std::io::Error>(head);
        tokio::task::yield_now().await;
        yield Ok(chunk);
        tokio::task::yield_now().await;
        yield Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "client aborted",
        ));
    };
    let resp = app
        .clone()
        .oneshot(upload_request(Body::from_stream(streamed)))
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "an aborted upload must be reported as 413"
    );
    let value = response_json(resp).await;
    assert_eq!(value["code"], "UPLOAD_READ_ERROR");
    let message = value["message"].as_str().unwrap_or("");
    assert!(
        message.contains("aborted after"),
        "the mid-write abort path must be exercised (got: {message})"
    );

    // 3. The good file survives byte-for-byte and the temp is gone.
    assert_eq!(
        std::fs::read(&dest).expect("original file must still exist"),
        original,
        "a failed re-upload must not touch the previous file"
    );
    assert!(
        tmp_siblings(dest.parent().unwrap()).is_empty(),
        "the staged temp must be removed on failure"
    );

    // 4. A clean re-upload now replaces the file and leaves no temp behind.
    let second = b"second version wins";
    let resp = app
        .clone()
        .oneshot(upload_request(Body::from(multipart_body(
            "keep.bin", second,
        ))))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "clean re-upload must succeed"
    );
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        second,
        "a clean re-upload replaces the file"
    );
    assert!(
        tmp_siblings(dest.parent().unwrap()).is_empty(),
        "no temp may linger after a successful replace"
    );

    let _ = std::fs::remove_dir_all(&ws);
}
