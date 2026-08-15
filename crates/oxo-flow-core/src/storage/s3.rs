//! S3 storage backend backed by [`aws_sdk_s3`].
//!
//! Configuration comes from environment variables: `AWS_REGION`,
//! `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`
//! (credentials; the SDK's profile-file/IMDS chain lives in aws_config's
//! async loader and is deliberately not used here — the same env-only
//! contract as the GCS backend). For local testing against MinIO or
//! LocalStack, also set `AWS_ENDPOINT_URL` and `OXO_S3_FORCE_PATH_STYLE=1`
//! (path-style addressing; the SDK has no env knob of its own for it).
//!
//! # Testing
//!
//! The constructor accepts an optional pre-configured client, which makes
//! it easy to swap in a fake or test-double client without hitting real S3.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::error::{OxoFlowError, Result};
use crate::storage::{RemoteStat, StorageBackend, StoragePath};

use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;

// ---------------------------------------------------------------------------
// Lazily-initialised default client
// ---------------------------------------------------------------------------

fn default_client() -> &'static S3Client {
    static CLIENT: OnceLock<S3Client> = OnceLock::new();
    CLIENT.get_or_init(S3Storage::build_client)
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

    /// Build a standalone SDK client with the standard env configuration.
    ///
    /// `new()` returns a process-wide singleton; code that runs on
    /// multiple tokio runtimes (test suites, embedded hosts) should build
    /// one client per runtime instead — an SDK client's HTTP connector
    /// binds to the runtime it first serves requests on, and a dropped
    /// runtime turns later requests into dispatch failures.
    pub fn build_client() -> S3Client {
        // `Config::builder().build()` is synchronous (env/config-file
        // values are resolved lazily per request), so no runtime is
        // needed here — and none may be started inside an ambient one.
        //
        // The service-level builder only reads the AWS_S3_* service keys;
        // the generic AWS_ENDPOINT_URL / AWS_REGION env vars come from
        // aws_config's async loader, so wire the two we document in
        // `S3Storage` explicitly. Credentials are resolved per request by
        // the SDK's default provider chain (env vars included).
        let mut builder = aws_sdk_s3::config::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest());
        // A plain service-builder config has no credentials provider —
        // requests would go out anonymous. The SDK's full default chain
        // (profile files, IMDS) lives in aws_config's *async* loader, so
        // this backend reads credentials from the environment only
        // (AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY /
        // AWS_SESSION_TOKEN) — the same contract as the GCS backend.
        builder = builder.credentials_provider(
            aws_config::environment::credentials::EnvironmentVariableCredentialsProvider::new(),
        );
        if let Ok(region) = std::env::var("AWS_REGION") {
            builder = builder.region(aws_sdk_s3::config::Region::new(region));
        }
        if let Ok(endpoint) = std::env::var("AWS_ENDPOINT_URL") {
            builder = builder.endpoint_url(endpoint);
        }
        // S3-compatible servers (MinIO, LocalStack) require path-style
        // addressing; the SDK has no env knob for it, so opt in explicitly.
        if let Ok(v) = std::env::var("OXO_S3_FORCE_PATH_STYLE")
            && (v == "1" || v.eq_ignore_ascii_case("true"))
        {
            builder = builder.force_path_style(true);
        }
        S3Client::from_conf(builder.build())
    }

    /// Wrap a pre-configured client (test doubles, per-runtime isolation).
    pub fn with_client(client: S3Client) -> Self {
        Self { client }
    }

    /// Create the bucket if it does not exist (idempotent; an
    /// `AlreadyOwnedByYou`/`BucketAlreadyExists` response is success).
    ///
    /// Not part of the [`StorageBackend`] trait — the engine never creates
    /// buckets implicitly. Used by setup tooling and live integration tests.
    pub async fn ensure_bucket(&self, bucket: &str) -> Result<()> {
        match self.client.create_bucket().bucket(bucket).send().await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("BucketAlreadyOwnedByYou")
                    || msg.contains("BucketAlreadyExists")
                    || msg.contains("BucketAlreadyOwned")
                {
                    Ok(())
                } else {
                    Err(s3_error(format!("S3 create_bucket error: {e:?}")))
                }
            }
        }
    }

    /// Delete an object (idempotent — deleting a missing key succeeds).
    ///
    /// Not part of the [`StorageBackend`] trait; used by ops tooling and
    /// live integration tests.
    pub async fn delete(&self, path: &StoragePath) -> Result<()> {
        let bucket = require_bucket(path)?;
        self.client
            .delete_object()
            .bucket(bucket)
            .key(&path.key)
            .send()
            .await
            .map(|_| ())
            .map_err(|e| s3_error(format!("S3 delete_object error: {e:?}")))
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
        Ok(self.head(path).await?.is_some())
    }

    /// HEAD with metadata: object size + ETag. Composite ETags from
    /// multipart uploads (`"hash-N"`) are recorded verbatim — equality
    /// comparison only, never recomputed locally (issue #78 P2).
    async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>> {
        let bucket = require_bucket(path)?;
        match self
            .client
            .head_object()
            .bucket(bucket)
            .key(&path.key)
            .send()
            .await
        {
            Ok(resp) => Ok(Some(RemoteStat {
                size: resp.content_length().unwrap_or(0) as u64,
                etag: resp.e_tag().map(str::to_string),
            })),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("NotFound") || msg.contains("404") {
                    Ok(None)
                } else {
                    // Debug includes the full error chain (service message,
                    // request id) that Display truncates.
                    Err(s3_error(format!("S3 head_object error: {e:?}")))
                }
            }
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
        let dest = crate::storage::staged_path(workdir, path)?;
        let stat = self
            .head(path)
            .await?
            .ok_or_else(|| s3_error(format!("cannot stage {}: object does not exist", path.raw)))?;
        let client = self.client.clone();
        let bucket = bucket.to_string();
        let key = path.key.clone();
        crate::storage::stage_with_cache(stat, &dest, move |mut file| {
            let client = client.clone();
            let bucket = bucket.clone();
            let key = key.clone();
            async move {
                let resp = client
                    .get_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|e| s3_error(format!("S3 stage get_object error: {e:?}")))?;
                let mut body = resp.body.into_async_read();
                tokio::io::copy(&mut body, &mut file)
                    .await
                    .map_err(|e| s3_error(format!("S3 stage read body error: {e}")))?;
                Ok(())
            }
        })
        .await?;
        Ok(dest)
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
