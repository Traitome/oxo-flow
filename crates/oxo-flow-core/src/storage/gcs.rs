//! Google Cloud Storage backend using HMAC authentication and the GCS XML API.
//!
//! GCS HMAC keys can be created in the GCP Console under
//! Cloud Storage → Settings → Interoperability.  The backend reads
//! credentials from the following environment variables, in order:
//!
//! 1. `GCS_ACCESS_KEY` / `GCS_SECRET_KEY`
//! 2. `STORAGE_ACCESS_KEY` / `STORAGE_SECRET_KEY` (S3-interop compat)
//!
//! # Authentication
//!
//! The GCS XML API uses an HMAC-SHA1 signature scheme (sometimes called
//! "SigV2-style") that is similar to AWS Signature Version 2.  The
//! `Authorization` header is formatted as:
//!
//! ```text
//! GOOG1 <access-key>:<base64(hmac-sha1(secret, string_to_sign))>
//! ```
//!
//! # Feature flag
//!
//! This module is only compiled when the `gcs-storage` feature is enabled.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{OxoFlowError, Result};
use crate::storage::{RemoteStat, StorageBackend, StoragePath};

use hmac::{Hmac, Mac};
use md5::Digest;
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

// ---------------------------------------------------------------------------
// Credential resolution
// ---------------------------------------------------------------------------

/// GCS HMAC credential pair.
struct GcsCredentials {
    access_key: String,
    secret_key: String,
}

fn load_credentials() -> Result<&'static GcsCredentials> {
    static CREDS: OnceLock<Result<GcsCredentials>> = OnceLock::new();
    CREDS
        .get_or_init(|| {
            let access_key = std::env::var("GCS_ACCESS_KEY")
                .or_else(|_| std::env::var("STORAGE_ACCESS_KEY"))
                .map_err(|_| OxoFlowError::Config {
                    message: "GCS credentials not found. Set GCS_ACCESS_KEY / GCS_SECRET_KEY "
                        .to_string()
                        + "(or STORAGE_ACCESS_KEY / STORAGE_SECRET_KEY for S3-interop)",
                })?;
            let secret_key = std::env::var("GCS_SECRET_KEY")
                .or_else(|_| std::env::var("STORAGE_SECRET_KEY"))
                .map_err(|_| OxoFlowError::Config {
                    message: "GCS secret not found. Set GCS_SECRET_KEY (or STORAGE_SECRET_KEY)"
                        .to_string(),
                })?;
            Ok(GcsCredentials {
                access_key,
                secret_key,
            })
        })
        .as_ref()
        .map_err(|e| OxoFlowError::Config {
            message: match e {
                OxoFlowError::Config { message } => message.clone(),
                _ => e.to_string(),
            },
        })
}

// ---------------------------------------------------------------------------
// GCS XML API helpers
// ---------------------------------------------------------------------------

/// Base URL for the GCS XML API.
fn gcs_url(bucket: &str, key: &str) -> String {
    // URL-encode the key for safe HTTP transmission
    let encoded_key = urlencode_key(key);
    format!("https://storage.googleapis.com/{bucket}/{encoded_key}")
}

/// Minimal URL encoding for object keys (only encode special characters).
fn urlencode_key(key: &str) -> String {
    let mut result = String::with_capacity(key.len());
    for byte in key.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    result
}

/// Compute the GCS HMAC-SHA1 signature.
fn gcs_signature(secret: &str, string_to_sign: &str) -> String {
    let mut mac =
        HmacSha1::new_from_slice(secret.as_bytes()).expect("HMAC can accept any key length");
    mac.update(string_to_sign.as_bytes());
    let result = mac.finalize();
    let code_bytes = result.into_bytes();
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(code_bytes)
}

/// Build the `Date` header value in RFC 1123 format.
fn rfc1123_date() -> String {
    chrono::Utc::now()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

/// Build the `Authorization` header value for a GCS XML API request.
fn gcs_authorization(
    creds: &GcsCredentials,
    method: &str,
    content_md5: &str,
    content_type: &str,
    date: &str,
    resource: &str,
) -> String {
    let string_to_sign = format!("{method}\n{content_md5}\n{content_type}\n{date}\n{resource}");
    let signature = gcs_signature(&creds.secret_key, &string_to_sign);
    format!("GOOG1 {}:{}", creds.access_key, signature)
}

/// Build the canonical resource path: `/<bucket>/<key>`.
fn canonical_resource(bucket: &str, key: &str) -> String {
    format!("/{bucket}/{key}")
}

/// GCS HTTP client (shared request-level client).
///
/// The overall cap is generous because object bodies are streamed, not
/// buffered: a multi-GB reference genome legitimately takes longer than a
/// metadata call, and the old 60 s total timeout killed any large transfer
/// before it finished. A hung connection is still bounded (connect timeout
/// catches the dead-peer case quickly).
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .expect("failed to create GCS HTTP client")
    })
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

/// Send a signed GET and return the live response after the status check.
///
/// [`gcs_get`] buffers the whole object for callers that need the bytes
/// (a small text object); the stage path streams this response to disk
/// chunk by chunk — a reference genome is routinely multi-GB and buffering
/// it first OOMed the engine (the S3 backend stages via `tokio::io::copy`).
async fn gcs_get_response(bucket: &str, key: &str) -> Result<reqwest::Response> {
    let creds = load_credentials()?;
    let url = gcs_url(bucket, key);
    let date = rfc1123_date();
    let resource = canonical_resource(bucket, key);
    let auth = gcs_authorization(creds, "GET", "", "", &date, &resource);

    let resp = http_client()
        .get(&url)
        .header("Date", &date)
        .header("Authorization", &auth)
        .send()
        .await
        .map_err(|e| gcs_io_error("GET", bucket, key, &e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(gcs_io_error(
            "GET",
            bucket,
            key,
            &format!("HTTP {status}: {body}"),
        ));
    }
    Ok(resp)
}

/// Make a signed GET request for a GCS object and return the response bytes.
async fn gcs_get(bucket: &str, key: &str) -> Result<Vec<u8>> {
    gcs_get_response(bucket, key)
        .await?
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| gcs_io_error("GET", bucket, key, &e.to_string()))
}

/// Make a signed HEAD request and return whether the object exists.
async fn gcs_head(bucket: &str, key: &str) -> Result<bool> {
    Ok(gcs_head_stat(bucket, key).await?.is_some())
}

/// HEAD with metadata: object size + `md5Hash` (base64, GCS has no native
/// ETag — the md5 hash is the stronger pure content hash; issue #78 P2).
async fn gcs_head_stat(bucket: &str, key: &str) -> Result<Option<RemoteStat>> {
    let creds = load_credentials()?;
    let url = gcs_url(bucket, key);
    let date = rfc1123_date();
    let resource = canonical_resource(bucket, key);
    let auth = gcs_authorization(creds, "HEAD", "", "", &date, &resource);

    let resp = http_client()
        .head(&url)
        .header("Date", &date)
        .header("Authorization", &auth)
        .send()
        .await
        .map_err(|e| gcs_io_error("HEAD", bucket, key, &e.to_string()))?;

    match resp.status().as_u16() {
        200 => {
            let size = resp
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let etag = parse_md5_hash_header(
                resp.headers()
                    .get("x-goog-hash")
                    .and_then(|v| v.to_str().ok()),
            );
            Ok(Some(RemoteStat { size, etag }))
        }
        404 => Ok(None),
        _ => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(gcs_io_error(
                "HEAD",
                bucket,
                key,
                &format!("HTTP {status}: {body}"),
            ))
        }
    }
}

/// Parse the `md5=` component of an `x-goog-hash` header (base64, kept
/// verbatim — never recomputed locally, equality comparison only).
fn parse_md5_hash_header(header: Option<&str>) -> Option<String> {
    header?
        .split(',')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("md5="))
        .filter(|v| {
            !v.is_empty()
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
        })
        .map(str::to_string)
}

/// Make a signed PUT request with an in-memory body.
async fn gcs_put(bucket: &str, key: &str, data: &[u8], content_type: &str) -> Result<()> {
    let md5 = compute_md5(data);
    gcs_put_body(bucket, key, data.to_vec().into(), &md5, content_type).await
}

/// [`gcs_put`] with a pre-computed Content-MD5 and an arbitrary body.
///
/// The upload path hands reqwest a file-backed stream
/// (`reqwest::Body::from(tokio::fs::File)`), so a multi-GB local output is
/// never read into RAM just to be sent.
async fn gcs_put_body(
    bucket: &str,
    key: &str,
    body: reqwest::Body,
    md5: &str,
    content_type: &str,
) -> Result<()> {
    let creds = load_credentials()?;
    let url = gcs_url(bucket, key);
    let date = rfc1123_date();
    let resource = canonical_resource(bucket, key);
    let auth = gcs_authorization(creds, "PUT", md5, content_type, &date, &resource);

    let resp = http_client()
        .put(&url)
        .header("Date", &date)
        .header("Authorization", &auth)
        .header("Content-Type", content_type)
        .header("Content-MD5", md5)
        .body(body)
        .send()
        .await
        .map_err(|e| gcs_io_error("PUT", bucket, key, &e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(gcs_io_error(
            "PUT",
            bucket,
            key,
            &format!("HTTP {status}: {body}"),
        ));
    }

    Ok(())
}

/// Compute the Content-MD5 header value (base64-encoded MD5).
fn compute_md5(data: &[u8]) -> String {
    encode_md5(md5::Md5::digest(data))
}

/// Streaming Content-MD5 for a local file: the header is part of the signed
/// string-to-sign, so the upload path hashes the file in one pass (no
/// full-object buffer) before streaming it.
async fn streaming_md5(path: &Path) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = md5::Md5::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(encode_md5(hasher.finalize()))
}

/// base64-encode a raw MD5 digest (the Content-MD5 header value).
fn encode_md5(digest: impl AsRef<[u8]>) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(digest.as_ref())
}

fn gcs_io_error(op: &str, bucket: &str, key: &str, detail: &str) -> OxoFlowError {
    OxoFlowError::Config {
        message: format!("GCS {op} error (bucket={bucket}, key={key}): {detail}"),
    }
}

fn require_gcs_bucket(sp: &StoragePath) -> Result<&str> {
    sp.bucket.as_deref().ok_or_else(|| OxoFlowError::Config {
        message: format!(
            "GCS path '{}' must include a bucket name (gs://bucket/key)",
            sp.raw
        ),
    })
}

// ---------------------------------------------------------------------------
// GcsStorage struct & StorageBackend impl
// ---------------------------------------------------------------------------

/// Google Cloud Storage backend using HMAC authentication.
///
/// Reads credentials from `GCS_ACCESS_KEY` / `GCS_SECRET_KEY`
/// (or the S3-interop `STORAGE_ACCESS_KEY` / `STORAGE_SECRET_KEY`).
///
/// Uses the GCS XML API directly via `reqwest` — no Google SDK dependency.
pub struct GcsStorage;

#[async_trait::async_trait]
impl StorageBackend for GcsStorage {
    async fn exists(&self, path: &StoragePath) -> Result<bool> {
        let bucket = require_gcs_bucket(path)?;
        gcs_head(bucket, &path.key).await
    }

    async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>> {
        let bucket = require_gcs_bucket(path)?;
        gcs_head_stat(bucket, &path.key).await
    }

    async fn read_to_string(&self, path: &StoragePath) -> Result<String> {
        let bucket = require_gcs_bucket(path)?;
        let bytes = gcs_get(bucket, &path.key).await?;
        String::from_utf8(bytes).map_err(|e| OxoFlowError::Config {
            message: format!("GCS content is not valid UTF-8: {e}"),
        })
    }

    async fn write(&self, path: &StoragePath, data: &[u8]) -> Result<()> {
        let bucket = require_gcs_bucket(path)?;
        gcs_put(bucket, &path.key, data, "application/octet-stream").await
    }

    async fn stage(&self, path: &StoragePath, workdir: &Path) -> Result<PathBuf> {
        let bucket = require_gcs_bucket(path)?;
        let dest = crate::storage::staged_path(workdir, path)?;
        let stat = self.head(path).await?.ok_or_else(|| {
            gcs_io_error("stage head", bucket, &path.key, "object does not exist")
        })?;
        let bucket = bucket.to_string();
        let key = path.key.clone();
        crate::storage::stage_with_cache(stat, &dest, move |mut file| {
            let bucket = bucket.clone();
            let key = key.clone();
            async move {
                // Stream to disk chunk by chunk — buffering the whole object
                // first (old `gcs_get`) doubled peak memory for every stage.
                let mut resp = gcs_get_response(&bucket, &key).await?;
                while let Some(chunk) = resp
                    .chunk()
                    .await
                    .map_err(|e| gcs_io_error("stage read", &bucket, &key, &e.to_string()))?
                {
                    tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                        .await
                        .map_err(|e| gcs_io_error("stage write", &bucket, &key, &e.to_string()))?;
                }
                Ok(())
            }
        })
        .await?;
        Ok(dest)
    }

    async fn upload(&self, local: &Path, remote: &StoragePath) -> Result<()> {
        let bucket = require_gcs_bucket(remote)?;
        // Hash the file (streaming) for the signed Content-MD5, then stream
        // it as the request body — `tokio::fs::read` held the whole object
        // in RAM, unlike the S3 backend's `ByteStream::from_path`.
        let md5 = streaming_md5(local)
            .await
            .map_err(|e| gcs_io_error("upload read", bucket, &remote.key, &e.to_string()))?;
        let file = tokio::fs::File::open(local)
            .await
            .map_err(|e| gcs_io_error("upload open", bucket, &remote.key, &e.to_string()))?;
        gcs_put_body(
            bucket,
            &remote.key,
            reqwest::Body::from(file),
            &md5,
            "application/octet-stream",
        )
        .await
    }

    fn name(&self) -> &'static str {
        "gcs"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoragePath;

    #[test]
    fn name_is_gcs() {
        assert_eq!(GcsStorage.name(), "gcs");
    }

    #[test]
    fn parse_gcs_md5_hash_header() {
        assert_eq!(
            parse_md5_hash_header(Some("md5=oVPGkKJcW4+2nW/eW3B+WA==")),
            Some("oVPGkKJcW4+2nW/eW3B+WA==".to_string())
        );
        assert_eq!(
            parse_md5_hash_header(Some("crc32c=xyz,md5=abc==")),
            Some("abc==".to_string())
        );
        assert_eq!(parse_md5_hash_header(None), None);
        assert_eq!(parse_md5_hash_header(Some("md5=bad*chars")), None);
        assert_eq!(parse_md5_hash_header(Some("crc32c=xyz")), None);
    }

    #[test]
    fn gcs_storage_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<GcsStorage>();
        assert_sync::<GcsStorage>();
    }

    #[test]
    fn urlencode_handles_special_chars() {
        assert_eq!(urlencode_key("simple.txt"), "simple.txt");
        assert_eq!(urlencode_key("a b"), "a%20b");
        assert_eq!(urlencode_key("path/to/file.bam"), "path/to/file.bam");
        assert_eq!(urlencode_key("sample+name"), "sample%2Bname");
    }

    #[test]
    fn canonical_resource_format() {
        let res = canonical_resource("my-bucket", "data/sample.fastq");
        assert_eq!(res, "/my-bucket/data/sample.fastq");
    }

    #[test]
    fn compute_md5_is_correct() {
        let md5 = compute_md5(b"hello");
        assert!(!md5.is_empty());
        // "XUFAKrxLKna5cZ2REBfFkg==" is the base64 MD5 of "hello"
        assert_eq!(md5, "XUFAKrxLKna5cZ2REBfFkg==");
    }

    #[tokio::test]
    async fn streaming_md5_matches_compute_md5() {
        // The upload path hashes the file in a streaming pass; it must
        // produce exactly the same Content-MD5 the in-memory path would
        // (a mismatch breaks the signed request).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("object.bin");
        // Larger than the 64 KiB read buffer, so the loop iterates.
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &payload).unwrap();

        assert_eq!(
            streaming_md5(&path).await.unwrap(),
            compute_md5(&payload),
            "streamed and buffered MD5 must agree"
        );
    }

    #[test]
    fn rfc1123_date_format() {
        let date = rfc1123_date();
        // Should end with " GMT" and contain a 3-letter weekday
        assert!(date.ends_with(" GMT"), "date={date}");
        assert!(
            date.contains("2026") || date.starts_with("Sat") || date.starts_with("Sun"),
            "unexpected date format: {date}"
        );
    }

    #[test]
    fn urlencode_preserves_slashes() {
        // Slashes must be preserved for GCS key hierarchy.
        assert_eq!(urlencode_key("dir/subdir/file.txt"), "dir/subdir/file.txt");
    }

    #[tokio::test]
    async fn exists_missing_bucket_returns_config_error() {
        let sp = StoragePath::parse("gs://bucket-only-no-key");
        let err = GcsStorage.exists(&sp).await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }

    #[tokio::test]
    async fn read_missing_bucket_returns_config_error() {
        let sp = StoragePath::parse("gs://nope");
        let err = GcsStorage.read_to_string(&sp).await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }

    #[tokio::test]
    async fn write_missing_bucket_returns_config_error() {
        let sp = StoragePath::parse("gs://nope");
        let err = GcsStorage.write(&sp, b"data").await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }

    #[tokio::test]
    async fn stage_missing_bucket_returns_config_error() {
        let sp = StoragePath::parse("gs://nope");
        let err = GcsStorage.stage(&sp, Path::new("/tmp")).await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }

    #[tokio::test]
    async fn upload_missing_bucket_returns_config_error() {
        let local = Path::new("/tmp/fake.txt");
        let remote = StoragePath::parse("gs://nope");
        let err = GcsStorage.upload(local, &remote).await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }

    #[test]
    fn gcs_url_format() {
        let url = gcs_url("my-bucket", "path/to/file.fastq");
        assert_eq!(
            url,
            "https://storage.googleapis.com/my-bucket/path/to/file.fastq"
        );
    }

    #[test]
    fn gcs_url_encodes_special_chars() {
        let url = gcs_url("b", "sample name+1.fq");
        assert_eq!(url, "https://storage.googleapis.com/b/sample%20name%2B1.fq");
    }
}
