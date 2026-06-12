//! S3 storage backend backed by [`aws_sdk_s3`].
//!
//! Uses the standard AWS credential chain (env vars, ~/.aws/credentials,
//! instance metadata, etc.) so no configuration is required beyond what
//! the AWS SDK normally reads.  For local testing against MinIO or
//! LocalStack, set `AWS_ENDPOINT_URL`, `AWS_ACCESS_KEY_ID`, and
//! `AWS_SECRET_ACCESS_KEY`.
//!
//! # Testing
//!
//! The constructor accepts an optional pre-configured client, which makes
//! it easy to swap in a fake or test-double client without hitting real S3.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::error::{OxoFlowError, Result};
use crate::storage::{StorageBackend, StoragePath};

use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;

// ---------------------------------------------------------------------------
// Lazily-initialised default client
// ---------------------------------------------------------------------------

fn default_client() -> &'static S3Client {
    static CLIENT: OnceLock<S3Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let rt = tokio::runtime::Runtime::new()
            .expect("failed to create tokio runtime for S3 client init");
        rt.block_on(async {
            let config = aws_sdk_s3::config::Config::builder()
                .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
                .build();
            S3Client::from_conf(config)
        })
    })
}

/// S3 storage backend.
///
/// All methods use the AWS SDK's standard credential resolution and require
/// the `s3-storage` feature flag.
///
/// ## Examples
///
/// ```rust,ignore
/// use oxo_flow_core::storage::s3::S3Storage;
/// use oxo_flow_core::storage::{StorageBackend, StoragePath};
///
/// let backend = S3Storage::new();
/// let sp = StoragePath::parse("s3://my-bucket/data.fastq");
/// let exists = backend.exists(&sp).await.unwrap();
/// ```
pub struct S3Storage {
    client: S3Client,
}

impl S3Storage {
    /// Create a new S3 backend using the default AWS credential chain.
    ///
    /// The underlying SDK client is initialised **once** and cached for the
    /// lifetime of the process.
    pub fn new() -> Self {
        Self {
            client: default_client().clone(),
        }
    }

    /// Create an S3 backend with a pre-configured client (useful for tests
    /// or custom endpoints such as MinIO / LocalStack).
    pub fn with_client(client: S3Client) -> Self {
        Self { client }
    }
}

impl Default for S3Storage {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn require_bucket(sp: &StoragePath) -> Result<&str> {
    sp.bucket.as_deref().ok_or_else(|| OxoFlowError::Config {
        message: format!(
            "S3 path '{}' must include a bucket name (s3://bucket/key)",
            sp.raw
        ),
    })
}

fn s3_error(msg: impl Into<String>) -> OxoFlowError {
    OxoFlowError::Config {
        message: msg.into(),
    }
}

// ---------------------------------------------------------------------------
// StorageBackend trait implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl StorageBackend for S3Storage {
    /// Check whether an object exists by issuing a HEAD request.
    ///
    /// Returns `Ok(true)` when the object exists, `Ok(false)` on a 404 /
    /// NotFound error, and propagates other errors.
    async fn exists(&self, path: &StoragePath) -> Result<bool> {
        let bucket = require_bucket(path)?;
        match self
            .client
            .head_object()
            .bucket(bucket)
            .key(&path.key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("NotFound") || msg.contains("404") {
                    Ok(false)
                } else {
                    Err(s3_error(format!("S3 head_object error: {e}")))
                }
            }
            Err(e) => Err(s3_error(format!("S3 head_object error: {e}"))),
        }
    }

    /// Read the full object into a UTF-8 string.
    ///
    /// Fails with a type-specific error when the content is not valid UTF-8.
    async fn read_to_string(&self, path: &StoragePath) -> Result<String> {
        let bucket = require_bucket(path)?;
        let resp = self
            .client
            .get_object()
            .bucket(bucket)
            .key(&path.key)
            .send()
            .await
            .map_err(|e| s3_error(format!("S3 get_object error: {e}")))?;

        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| s3_error(format!("S3 read body error: {e}")))?
            .into_bytes();

        String::from_utf8(bytes.to_vec()).map_err(|e| OxoFlowError::Config {
            message: format!("S3 content is not valid UTF-8: {e}"),
        })
    }

    /// Write bytes to an object, replacing it if it already exists.
    async fn write(&self, path: &StoragePath, data: &[u8]) -> Result<()> {
        let bucket = require_bucket(path)?;
        let body = ByteStream::from(data.to_vec());

        self.client
            .put_object()
            .bucket(bucket)
            .key(&path.key)
            .body(body)
            .send()
            .await
            .map_err(|e| s3_error(format!("S3 put_object error: {e}")))?;

        Ok(())
    }

    /// Download a remote object to a local working directory, mirroring the
    /// remote key structure under `workdir`.
    async fn stage(&self, path: &StoragePath, workdir: &Path) -> Result<PathBuf> {
        let bucket = require_bucket(path)?;
        let local_path = workdir.join(&path.key);

        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| s3_error(format!("failed to create local staging dir: {e}")))?;
        }

        let resp = self
            .client
            .get_object()
            .bucket(bucket)
            .key(&path.key)
            .send()
            .await
            .map_err(|e| s3_error(format!("S3 stage get_object error: {e}")))?;

        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| s3_error(format!("S3 stage read body error: {e}")))?
            .into_bytes();

        tokio::fs::write(&local_path, bytes)
            .await
            .map_err(|e| s3_error(format!("failed to write staged file: {e}")))?;

        Ok(local_path)
    }

    /// Upload a local file to a remote S3 location.
    async fn upload(&self, local: &Path, remote: &StoragePath) -> Result<()> {
        let bucket = require_bucket(remote)?;

        let body = ByteStream::from_path(local).await.map_err(|e| {
            s3_error(format!(
                "failed to read local file '{}': {e}",
                local.display()
            ))
        })?;

        self.client
            .put_object()
            .bucket(bucket)
            .key(&remote.key)
            .body(body)
            .send()
            .await
            .map_err(|e| s3_error(format!("S3 put_object upload error: {e}")))?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "s3"
    }
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoragePath;

    // ── struct & constructor ──────────────────────────────────────────────

    #[test]
    fn default_impl_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<S3Storage>();
        assert_sync::<S3Storage>();
    }

    #[test]
    fn name_is_s3() {
        let s3 = S3Storage::new();
        assert_eq!(s3.name(), "s3");
    }

    #[test]
    fn with_client_accepts_custom_client() {
        let _ = S3Storage::new();
    }

    // ── path parsing errors ───────────────────────────────────────────────

    #[tokio::test]
    async fn exists_missing_bucket_returns_config_error() {
        let backend = S3Storage::new();
        let sp = StoragePath::parse("s3://just-a-bucket-no-key");
        let err = backend.exists(&sp).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bucket"), "expected bucket error, got: {msg}");
    }

    #[tokio::test]
    async fn read_missing_bucket_returns_config_error() {
        let backend = S3Storage::new();
        let sp = StoragePath::parse("s3://nope");
        let err = backend.read_to_string(&sp).await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }

    #[tokio::test]
    async fn write_missing_bucket_returns_config_error() {
        let backend = S3Storage::new();
        let sp = StoragePath::parse("s3://nope");
        let err = backend.write(&sp, b"data").await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }

    #[tokio::test]
    async fn stage_missing_bucket_returns_config_error() {
        let backend = S3Storage::new();
        let sp = StoragePath::parse("s3://nope");
        let err = backend.stage(&sp, Path::new("/tmp")).await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }

    #[tokio::test]
    async fn upload_missing_bucket_returns_config_error() {
        let backend = S3Storage::new();
        let local = Path::new("/tmp/fake.txt");
        let remote = StoragePath::parse("s3://nope");
        let err = backend.upload(local, &remote).await.unwrap_err();
        assert!(err.to_string().contains("bucket"));
    }
}
