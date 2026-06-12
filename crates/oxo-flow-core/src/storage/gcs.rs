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
use crate::storage::{StorageBackend, StoragePath};

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
    base64::engine::general_purpose::STANDARD.encode(&code_bytes)
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
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("failed to create GCS HTTP client")
    })
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

/// Make a signed GET request for a GCS object and return the response bytes.
async fn gcs_get(bucket: &str, key: &str) -> Result<Vec<u8>> {
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

    let status = resp.status();
    if !status.is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(gcs_io_error(
            "GET",
            bucket,
            key,
            &format!("HTTP {status}: {body}"),
        ));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| gcs_io_error("GET", bucket, key, &e.to_string()))
}

/// Make a signed HEAD request and return whether the object exists.
async fn gcs_head(bucket: &str, key: &str) -> Result<bool> {
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
        200 => Ok(true),
        404 => Ok(false),
        status => {
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

/// Make a signed PUT request with a body.
async fn gcs_put(bucket: &str, key: &str, data: &[u8], content_type: &str) -> Result<()> {
    let creds = load_credentials()?;
    let url = gcs_url(bucket, key);
    let date = rfc1123_date();
    let md5 = compute_md5(data);
    let resource = canonical_resource(bucket, key);
    let auth = gcs_authorization(creds, "PUT", &md5, content_type, &date, &resource);

    let resp = http_client()
        .put(&url)
        .header("Date", &date)
        .header("Authorization", &auth)
        .header("Content-Type", content_type)
        .header("Content-MD5", &md5)
        .body(data.to_vec())
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
    let digest = md5::Md5::digest(data);
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(&digest[..])
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
        let local_path = workdir.join(&path.key);
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| gcs_io_error("stage mkdir", bucket, &path.key, &e.to_string()))?;
        }
        let bytes = gcs_get(bucket, &path.key).await?;
        tokio::fs::write(&local_path, bytes)
            .await
            .map_err(|e| gcs_io_error("stage write", bucket, &path.key, &e.to_string()))?;
        Ok(local_path)
    }

    async fn upload(&self, local: &Path, remote: &StoragePath) -> Result<()> {
        let bucket = require_gcs_bucket(remote)?;
        let data = tokio::fs::read(local)
            .await
            .map_err(|e| gcs_io_error("upload read", bucket, &remote.key, &e.to_string()))?;
        gcs_put(bucket, &remote.key, &data, "application/octet-stream").await
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
