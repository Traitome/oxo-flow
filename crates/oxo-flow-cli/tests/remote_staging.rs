//! Live S3 E2E (issue #80 item 1): real etag-driven invalidation against a
//! MinIO instance.
//!
//! Gated twice so the default suite stays hermetic:
//! 1. `#![cfg(feature = "s3-storage")]` — only built when the CLI enables
//!    the backend (run with `cargo test -p oxo-flow-cli --features
//!    s3-storage --test remote_staging`).
//! 2. `OXO_S3_E2E=1` — only executes against a live endpoint; otherwise it
//!    returns early with a notice (CI stays green without MinIO).
//!
//! Expected environment (defaults target a local MinIO):
//!   OXO_S3_E2E=1  AWS_ENDPOINT_URL=http://127.0.0.1:9000
//!   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_REGION
//!   OXO_S3_FORCE_PATH_STYLE=1
#![cfg(feature = "s3-storage")]

use std::path::Path;
use std::process::Command;

use oxo_flow_core::storage::s3::S3Storage;
use oxo_flow_core::storage::{StorageBackend, StoragePath};

/// Env for child `oxo-flow` processes, or `None` when the live gate is off.
fn live_env() -> Option<Vec<(&'static str, String)>> {
    if std::env::var("OXO_S3_E2E").is_ok_and(|v| v == "1") {
        Some(vec![
            (
                "AWS_ENDPOINT_URL",
                std::env::var("AWS_ENDPOINT_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:9000".into()),
            ),
            (
                "AWS_ACCESS_KEY_ID",
                std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_else(|_| "minioadmin".into()),
            ),
            (
                "AWS_SECRET_ACCESS_KEY",
                std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".into()),
            ),
            (
                "AWS_REGION",
                std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into()),
            ),
            ("OXO_S3_FORCE_PATH_STYLE", "1".into()),
        ])
    } else {
        None
    }
}

/// Write a workflow whose rule consumes one remote + one local input.
fn write_workflow(dir: &Path, remote: &str) {
    let workflow = format!(
        r#"
[workflow]
name = "s3-live-e2e"

[[rules]]
name = "mix"
input = ["{remote}", "data/local.fq"]
output = ["out.txt"]
shell = "wc -l data/local.fq > out.txt"
"#
    );
    std::fs::write(dir.join("wf.oxoflow"), workflow).unwrap();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/local.fq"), ">S1\nACGT\n").unwrap();
}

fn run(dir: &Path) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oxo-flow"));
    cmd.args(["run", "wf.oxoflow"]).current_dir(dir);
    if let Some(envs) = live_env() {
        cmd.envs(envs);
    }
    let out = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.success(), stderr)
}

fn checkpoint_etag(dir: &Path) -> Option<String> {
    let ck: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json")).unwrap(),
    )
    .unwrap();
    ck["input_manifests"]["mix"]
        .as_array()?
        .iter()
        .find_map(|e| e["remote"]["etag"].as_str().map(str::to_string))
}

#[tokio::test]
async fn etag_rewrite_invalidates_exactly_the_remote_input_rule() {
    let Some(envs) = live_env() else {
        eprintln!("skipped: OXO_S3_E2E is not set (no live S3 endpoint)");
        return;
    };
    // The test process itself also talks S3 (bucket setup, rewrites), and
    // `S3Storage` reads its configuration from the *process* environment —
    // the live gate therefore requires the caller to export the AWS vars
    // (run: `AWS_ENDPOINT_URL=… AWS_ACCESS_KEY_ID=… AWS_SECRET_ACCESS_KEY=…
    // AWS_REGION=… OXO_S3_FORCE_PATH_STYLE=1 OXO_S3_E2E=1 cargo test …`).
    for (k, _) in &envs {
        if std::env::var(k).is_err() {
            eprintln!("skipped: {k} is not exported in the test process");
            return;
        }
    }
    let backend = S3Storage::with_client(S3Storage::build_client());
    let bucket = format!("oxo-e2e-{}", std::process::id());
    backend.ensure_bucket(&bucket).await.unwrap_or_else(|e| {
        panic!("MinIO bucket setup failed ({e:?}); is a server listening at AWS_ENDPOINT_URL?")
    });
    let remote = format!("s3://{bucket}/k1");

    // Seed the object (16 bytes).
    let key = StoragePath::parse(&remote);
    backend.write(&key, b"v1-content-AAAA\n").await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), &remote);

    // Run 1: executes; the manifest records the remote object's etag.
    let (ok, stderr) = run(dir.path());
    assert!(ok, "run 1 failed: {stderr}");
    assert!(dir.path().join("out.txt").exists());
    let etag1 = checkpoint_etag(dir.path()).expect("remote etag in manifest");

    // Run 2: nothing changed — the rule is up-to-date and skipped.
    let (ok, stderr) = run(dir.path());
    assert!(ok, "run 2 failed: {stderr}");
    assert!(
        stderr.contains("1 skipped") || stderr.contains("already completed"),
        "expected a skip on unchanged cloud input: {stderr}"
    );

    // Rewrite the object with the same size but new content → new etag.
    backend.write(&key, b"v2-content-BBBB\n").await.unwrap();
    let new_head = backend.head(&key).await.unwrap().unwrap();
    assert_ne!(new_head.etag.as_deref(), Some(etag1.as_str()));

    // Run 3: exactly the one rule invalidates and re-executes.
    let (ok, stderr) = run(dir.path());
    assert!(ok, "run 3 failed: {stderr}");
    assert!(
        stderr.contains("invalidated 1 rule"),
        "expected the etag change to invalidate the rule: {stderr}"
    );

    // The manifest now carries the new etag — future runs stay skipped.
    let etag2 = checkpoint_etag(dir.path()).expect("remote etag after rewrite");
    assert_ne!(etag1, etag2);
    let (ok, stderr) = run(dir.path());
    assert!(ok, "run 4 failed: {stderr}");
    assert!(
        stderr.contains("1 skipped") || stderr.contains("already completed"),
        "expected a skip after the re-execution: {stderr}"
    );
}

/// #80 item 2 acceptance: s3:// input → local execution → output upload →
/// cloud-input rewrite → precise invalidation → deleted cloud output → the
/// freshness gate re-runs and re-uploads.
#[tokio::test]
async fn staged_input_and_uploaded_output_end_to_end() {
    let Some(envs) = live_env() else {
        eprintln!("skipped: OXO_S3_E2E is not set (no live S3 endpoint)");
        return;
    };
    for (k, _) in &envs {
        if std::env::var(k).is_err() {
            eprintln!("skipped: {k} is not exported in the test process");
            return;
        }
    }
    let backend = S3Storage::with_client(S3Storage::build_client());
    let bucket = format!("oxo-e2e-staging-{}", std::process::id());
    backend.ensure_bucket(&bucket).await.unwrap();
    let input_uri = format!("s3://{bucket}/k1");
    let output_uri = format!("s3://{bucket}/out.txt");
    let input_path = StoragePath::parse(&input_uri);
    let output_path = StoragePath::parse(&output_uri);

    backend
        .write(&input_path, b"v1-content-AAAA\n")
        .await
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let workflow = format!(
        r#"
[workflow]
name = "staging-e2e"

[[rules]]
name = "consume"
input = ["{input_uri}"]
output = ["out.txt", "{output_uri}"]
shell = "cat {{input[0]}} > out.txt && cp out.txt {{output[1]}}"
"#
    );
    std::fs::write(dir.path().join("wf.oxoflow"), workflow).unwrap();

    // Run 1: executes; the shell reads the staged input and the engine
    // uploads the remote output.
    let (ok, stderr) = run(dir.path());
    assert!(ok, "run 1 failed: {stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "v1-content-AAAA\n"
    );
    assert_eq!(
        backend.read_to_string(&output_path).await.unwrap(),
        "v1-content-AAAA\n"
    );

    // Run 2: nothing changed — skipped (staged cache + cloud output exist).
    let (ok, stderr) = run(dir.path());
    assert!(ok, "run 2 failed: {stderr}");
    assert!(stderr.contains("1 skipped") || stderr.contains("already completed"));

    // Rewrite the cloud input (same size, new content) → precise
    // invalidation → re-execution → re-upload.
    backend
        .write(&input_path, b"v2-content-BBBB\n")
        .await
        .unwrap();
    let (ok, stderr) = run(dir.path());
    assert!(ok, "run 3 failed: {stderr}");
    assert!(stderr.contains("invalidated 1 rule"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "v2-content-BBBB\n"
    );
    assert_eq!(
        backend.read_to_string(&output_path).await.unwrap(),
        "v2-content-BBBB\n"
    );

    // Delete the cloud output → the freshness gate notices the object is
    // gone → the rule re-runs and restores it.
    backend.delete(&output_path).await.unwrap();
    let (ok, stderr) = run(dir.path());
    assert!(ok, "run 4 failed: {stderr}");
    assert!(!stderr.contains("1 skipped"), "expected a re-run: {stderr}");
    assert_eq!(
        backend.read_to_string(&output_path).await.unwrap(),
        "v2-content-BBBB\n",
        "remote output must be re-uploaded after deletion"
    );
}
