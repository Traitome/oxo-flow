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
//! Timeouts are bounded like the GCS backend: connect 30 s, per-attempt
//! 30 min — generous for multi-GB object bodies streamed without a buffer,
//! but a stalled transfer can no longer hang a rule forever. 404s are
//! detected through the SDK's typed errors, never by string-matching
//! `Display` (which truncates to "service error" and made the old check
//! dead code, issue #575). Uploads above the 5 GiB single-PUT limit
//! switch to a multipart transfer automatically.
//!
//! # Testing
//!
//! The constructor accepts an optional pre-configured client, which makes
//! it easy to swap in a fake or test-double client without hitting real S3.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{OxoFlowError, Result};
use crate::storage::{RemoteStat, StorageBackend, StoragePath};

use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::error::DisplayErrorContext;
use aws_sdk_s3::primitives::{ByteStream, Length};
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};

// ---------------------------------------------------------------------------
// Transfer sizing
// ---------------------------------------------------------------------------

/// Single-PUT limit on real S3; objects at or above this size must use
/// multipart upload.
const MULTIPART_THRESHOLD_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// Part size for multipart uploads: 64 MiB — 160 parts per GiB, well
/// under the 10,000-part ceiling for any object a pipeline produces.
const MULTIPART_PART_SIZE: u64 = 64 * 1024 * 1024;
/// Connect timeout: dead peers must surface quickly (mirrors GCS).
const CONNECT_TIMEOUT_SECS: u64 = 30;
/// Per-attempt cap for whole operations (connect + send + receive).
/// Object bodies stream, so the ceiling must cover a multi-GB transfer
/// over a slow link, not a metadata call (mirrors GCS's total timeout).
const OPERATION_ATTEMPT_TIMEOUT_SECS: u64 = 30 * 60;

/// Plan the multipart layout for `len` bytes.
///
/// Returns `(part_size, part_count)` or `None` when the object fits a
/// single PUT (below [`MULTIPART_THRESHOLD_BYTES`]). Part size grows on the
/// 64 MiB base up to the 5 GiB per-part maximum so any object up to the
/// ~48.8 TiB multipart ceiling (5 GiB × 10,000 parts) stays under the
/// 10,000-part limit; larger objects have no valid layout and are rejected
/// by the caller.
fn multipart_plan(len: u64) -> Option<(u64, usize)> {
    if len < MULTIPART_THRESHOLD_BYTES {
        return None;
    }
    // 10,000 parts max: scale the part size up, capped at the 5 GiB
    // per-part limit S3 itself enforces.
    let mut part_size = MULTIPART_PART_SIZE.max(1);
    while len.div_ceil(part_size) > 10_000 {
        if part_size >= MULTIPART_THRESHOLD_BYTES {
            // 5 GiB × 10,000 parts ≈ 48.8 TiB — no valid layout exists.
            return None;
        }
        part_size = (part_size.saturating_mul(2)).min(MULTIPART_THRESHOLD_BYTES);
    }
    let count = len.div_ceil(part_size) as usize;
    Some((part_size, count))
}

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
        // The SDK default ships no operation timeout — a stalled transfer
        // (dead NAT, throttled peer) would block the executor's await
        // forever (issue #575). Bound it like the GCS backend: quick
        // connect failure, generous per-attempt ceiling for streamed
        // multi-GB bodies.
        let timeouts = TimeoutConfig::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .operation_attempt_timeout(Duration::from_secs(OPERATION_ATTEMPT_TIMEOUT_SECS))
            .build();
        builder = builder.timeout_config(timeouts);
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
            // Typed predicates — never string-match Display (issue #575).
            Err(e)
                if e.as_service_error().is_some_and(|se| {
                    se.is_bucket_already_exists() || se.is_bucket_already_owned_by_you()
                }) =>
            {
                Ok(())
            }
            Err(e) => Err(s3_error(format!(
                "S3 create_bucket error: {}",
                s3_display(&e)
            ))),
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
            .map_err(|e| s3_error(format!("S3 delete_object error: {}", s3_display(&e))))
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

/// Map an SdkError to a context-rich string via [`DisplayErrorContext`]
/// (Display alone truncates to "service error", hiding the service
/// message and request id — issue #575).
fn s3_display<E: std::error::Error + 'static>(e: &aws_sdk_s3::error::SdkError<E>) -> String {
    DisplayErrorContext(e).to_string()
}

/// Typed 404 detection for HEAD: HEAD responses may carry no body, so the
/// check must run on the typed error, never on a Display string-match
/// (the old `contains("NotFound")` was dead code — Display truncates to
/// "service error", issue #575).
fn is_not_found(
    e: &aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::head_object::HeadObjectError>,
) -> bool {
    e.as_service_error().is_some_and(|se| se.is_not_found())
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
            // Typed 404 detection: HEAD responses may carry no body, so
            // the check must run on the typed error, not on a Display
            // string-match (the old contains("NotFound") was dead code —
            // Display truncates to "service error", issue #575).
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(s3_error(format!(
                "S3 head_object error: {}",
                s3_display(&e)
            ))),
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
            .map_err(|e| s3_error(format!("S3 get_object error: {}", s3_display(&e))))?;

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
            .map_err(|e| s3_error(format!("S3 put_object error: {}", s3_display(&e))))?;

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
                    .map_err(|e| {
                        s3_error(format!("S3 stage get_object error: {}", s3_display(&e)))
                    })?;
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
    ///
    /// Files below the 5 GiB single-PUT limit go through `put_object`;
    /// anything larger switches to a multipart upload — real S3 rejects a
    /// single PUT above the limit outright, and a 20 GB CRAM output must
    /// not fail the rule (issue #575). Any multipart failure aborts the
    /// server-side transfer so orphaned parts are not billed.
    async fn upload(&self, local: &Path, remote: &StoragePath) -> Result<()> {
        let bucket = require_bucket(remote)?;

        let len = tokio::fs::metadata(local)
            .await
            .map_err(|e| {
                s3_error(format!(
                    "failed to stat local file '{}': {e}",
                    local.display()
                ))
            })?
            .len();

        if let Some((part_size, part_count)) = multipart_plan(len) {
            return self
                .upload_multipart(local, bucket, &remote.key, len, part_size, part_count)
                .await;
        }

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
            .map_err(|e| s3_error(format!("S3 put_object upload error: {}", s3_display(&e))))?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "s3"
    }
}

impl S3Storage {
    /// Multipart upload of `local` in `part_count` chunks of `part_size`
    /// bytes (layout from [`multipart_plan`]). Ranged parts stream from
    /// disk (`ByteStream::read_from` with offset + length) — the file is
    /// never buffered whole. On any failure the server-side transfer is
    /// aborted so orphaned parts are not billed; abort errors are logged,
    /// never masked over the original failure.
    async fn upload_multipart(
        &self,
        local: &Path,
        bucket: &str,
        key: &str,
        len: u64,
        part_size: u64,
        part_count: usize,
    ) -> Result<()> {
        let upload_id = self
            .client
            .create_multipart_upload()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                s3_error(format!(
                    "S3 create_multipart_upload error for '{}': {}",
                    local.display(),
                    s3_display(&e)
                ))
            })?
            .upload_id()
            .ok_or_else(|| s3_error("S3 create_multipart_upload returned no upload id"))?
            .to_string();

        let mut parts: Vec<CompletedPart> = Vec::with_capacity(part_count);
        let result = async {
            for part_number in 1..=part_count {
                let offset = (part_number as u64 - 1) * part_size;
                // Length::Exact fails on a truncated file — exactly what we
                // want if the file shrank since the stat.
                let body = ByteStream::read_from()
                    .path(local)
                    .offset(offset)
                    .length(Length::Exact(part_size.min(len - offset)))
                    .build()
                    .await
                    .map_err(|e| {
                        s3_error(format!(
                            "failed to read part {part_number} of '{}': {e}",
                            local.display()
                        ))
                    })?;
                let out = self
                    .client
                    .upload_part()
                    .bucket(bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .part_number(part_number as i32)
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| {
                        s3_error(format!(
                            "S3 upload_part {part_number}/{part_count} error: {}",
                            s3_display(&e)
                        ))
                    })?;
                parts.push(
                    CompletedPart::builder()
                        .set_e_tag(out.e_tag().map(str::to_string))
                        .part_number(part_number as i32)
                        .build(),
                );
            }
            self.client
                .complete_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(parts))
                        .build(),
                )
                .send()
                .await
                .map_err(|e| {
                    s3_error(format!(
                        "S3 complete_multipart_upload error: {}",
                        s3_display(&e)
                    ))
                })?;
            Ok(())
        }
        .await;

        if let Err(e) = result {
            if let Err(abort_err) = self
                .client
                .abort_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&upload_id)
                .send()
                .await
            {
                tracing::warn!(
                    error = %abort_err,
                    key,
                    "failed to abort interrupted multipart upload"
                );
            }
            return Err(e);
        }
        Ok(())
    }
}

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

    #[tokio::test]
    async fn with_client_uses_the_supplied_client() {
        // The test asserted nothing before. A client pointed at a closed
        // local port makes the difference observable: any request must fail
        // with a transport error, proving `with_client` really uses the
        // supplied client (the default one would try real AWS). No external
        // network is touched.
        let conf = aws_sdk_s3::config::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "test-key",
                "test-secret",
                None,
                None,
                "test",
            ))
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .endpoint_url("http://127.0.0.1:1")
            .build();
        let backend = S3Storage::with_client(aws_sdk_s3::Client::from_conf(conf));
        assert_eq!(backend.name(), "s3");
        let sp = StoragePath::parse("s3://bucket/key");
        assert!(
            backend.exists(&sp).await.is_err(),
            "an unreachable endpoint must surface a transport error"
        );
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

    // ── multipart planning (issue #575) ───────────────────────────────────

    #[test]
    fn multipart_plan_below_threshold_is_single_put() {
        assert_eq!(multipart_plan(0), None);
        assert_eq!(multipart_plan(MULTIPART_THRESHOLD_BYTES - 1), None);
    }

    #[test]
    fn multipart_plan_at_threshold_uses_base_part_size() {
        let (part_size, count) = multipart_plan(MULTIPART_THRESHOLD_BYTES).expect(">= 5 GiB");
        assert_eq!(part_size, MULTIPART_PART_SIZE);
        assert_eq!(
            count as u64,
            MULTIPART_THRESHOLD_BYTES.div_ceil(MULTIPART_PART_SIZE)
        );
    }

    #[test]
    fn multipart_plan_huge_object_grows_part_size_within_limits() {
        // Far beyond 64 MiB × 10,000 = 625 GiB: the plan must grow the part
        // size so the count stays within the 10,000-part ceiling.
        let len = 20 * 1024 * 1024 * 1024 * 1024; // 20 TiB
        let (part_size, count) = multipart_plan(len).expect("<= ~48.8 TiB ceiling");
        assert!(part_size <= MULTIPART_THRESHOLD_BYTES, "≤ 5 GiB per part");
        assert!(count <= 10_000, "≤ 10,000 parts, got {count}");
        assert!(count as u64 * part_size >= len, "layout covers len");
    }

    #[test]
    fn multipart_plan_beyond_ceiling_is_unplannable() {
        // 5 GiB × 10,000 parts ≈ 48.8 TiB — anything above has no valid
        // multipart layout and must be rejected rather than sent.
        let len = MULTIPART_THRESHOLD_BYTES * 10_000 + 1;
        assert_eq!(multipart_plan(len), None);
    }

    #[test]
    fn multipart_plan_last_part_covers_remainder() {
        // Not a multiple of the part size: count must round up and the
        // final ranged read must stay within bounds.
        let len = MULTIPART_THRESHOLD_BYTES + 1;
        let (part_size, count) = multipart_plan(len).expect(">= 5 GiB");
        assert_eq!(count as u64, len.div_ceil(part_size));
        assert!((count as u64 - 1) * part_size < len);
    }
}
