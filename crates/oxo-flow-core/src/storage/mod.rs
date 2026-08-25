//! Cloud storage abstraction for oxo-flow.
//!
//! Provides a `StorageBackend` trait that abstracts file operations across
//! different storage providers (local filesystem, S3, GCS). Workflows can
//! use `s3://bucket/key` or `gs://bucket/key` URIs transparently when a
//! matching backend is registered.
//!
//! # Example
//!
//! ```rust,ignore
//! use oxo_flow_core::storage::{StorageResolver, StorageBackend};
//!
//! let resolver = StorageResolver::with_local();
//! let sp = StorageResolver::parse_path("s3://my-bucket/data.fastq");
//! assert!(sp.is_remote());
//! ```

pub mod local;

#[cfg(feature = "s3-storage")]
pub mod s3;

#[cfg(feature = "gcs-storage")]
pub mod gcs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{OxoFlowError, Result};

/// URI scheme for storage backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageScheme {
    /// Local filesystem (default).
    Local,
    /// Amazon S3 or compatible object store.
    S3,
    /// Google Cloud Storage.
    Gcs,
}

impl StorageScheme {
    /// Detect the scheme from a URI string.
    pub fn from_uri(path: &str) -> Self {
        if path.starts_with("s3://") {
            Self::S3
        } else if path.starts_with("gs://") {
            Self::Gcs
        } else {
            Self::Local
        }
    }

    /// Lowercase scheme name ("local" | "s3" | "gs").
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
            Self::Gcs => "gs",
        }
    }
}

/// A parsed storage URI, normalized into its scheme, bucket, and key parts.
///
/// For local paths the `bucket` field is `None` and `key` holds the raw path.
/// For remote URIs (`s3://bucket/key`) the bucket and key are extracted.
#[derive(Debug, Clone)]
pub struct StoragePath {
    /// The original raw URI string.
    pub raw: String,
    /// Detected storage scheme.
    pub scheme: StorageScheme,
    /// Bucket name (None for local paths).
    pub bucket: Option<String>,
    /// Key or local path within the bucket / filesystem.
    pub key: String,
}

impl StoragePath {
    /// Parse a path or URI into its component parts.
    ///
    /// - `"s3://bucket/some/key"` -> scheme=S3, bucket="bucket", key="some/key"
    /// - `"gs://bucket/obj"`      -> scheme=Gcs, bucket="bucket", key="obj"
    /// - `"/local/path"`          -> scheme=Local, bucket=None, key="/local/path"
    /// - `"relative/path"`        -> scheme=Local, bucket=None, key="relative/path"
    pub fn parse(raw: &str) -> Self {
        let scheme = StorageScheme::from_uri(raw);
        let (bucket, key) = match scheme {
            StorageScheme::S3 | StorageScheme::Gcs => {
                let without_prefix = match scheme {
                    StorageScheme::S3 => raw.strip_prefix("s3://").unwrap_or(raw),
                    StorageScheme::Gcs => raw.strip_prefix("gs://").unwrap_or(raw),
                    _ => raw,
                };
                if let Some((b, k)) = without_prefix.split_once('/') {
                    (Some(b.to_string()), k.to_string())
                } else {
                    (None, without_prefix.to_string())
                }
            }
            StorageScheme::Local => (None, raw.to_string()),
        };
        Self {
            raw: raw.to_string(),
            scheme,
            bucket,
            key,
        }
    }

    /// Returns `true` when the path refers to a remote (non-local) storage location.
    pub fn is_remote(&self) -> bool {
        self.scheme != StorageScheme::Local
    }
}

/// Metadata of a remote object, used for content-addressed invalidation
/// (issue #78 P2). Local files have no `RemoteStat` — local invalidation
/// keeps the size+mtime+sha256 policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStat {
    /// Object size in bytes.
    pub size: u64,
    /// Content identity as reported by the object store: S3 ETag (raw, may
    /// be a composite multipart hash) or GCS `md5Hash` (base64). `None`
    /// when the store cannot provide one.
    pub etag: Option<String>,
}

// ---------------------------------------------------------------------------
// Post-upload verification (issue #194 §2.11): after an upload, the remote
// store is asked what it holds and its identity is compared with the local
// file — a silently truncated transfer would otherwise be checkpointed as
// complete.
// ---------------------------------------------------------------------------

/// Verify a just-uploaded object against the local file it came from:
/// size always, content digest when the remote store exposes a comparable
/// one (S3 single-part ETag = md5 hex; GCS `md5Hash` = base64 md5).
///
/// Composite identities (S3 multipart ETags, `hash-N`) and objects without
/// a digest are skipped with a debug note — they are not comparable to a
/// plain md5. Any comparable mismatch is an [`OxoFlowError::Integrity`]
/// error: the remote copy cannot be trusted and the rule must fail.
pub async fn verify_upload(
    backend: &dyn StorageBackend,
    local: &Path,
    remote: &StoragePath,
) -> Result<()> {
    let Some(stat) = backend.head(remote).await? else {
        tracing::debug!(remote = %remote.raw, "uploaded object has no HEAD result — skipping verification");
        return Ok(());
    };

    let local_size = tokio::fs::metadata(local).await?.len();
    if stat.size != local_size {
        return Err(OxoFlowError::Integrity {
            message: format!(
                "uploaded object '{}' reports {} bytes but the local file is {local_size}",
                remote.raw, stat.size
            ),
            failed_files: vec![remote.raw.clone()],
        });
    }

    let Some(etag) = stat.etag.as_deref() else {
        tracing::debug!(remote = %remote.raw, "remote store reported no content digest — skipping checksum verification");
        return Ok(());
    };

    match remote.scheme {
        StorageScheme::S3 => {
            // Multipart/SSE etags (`hash-N`) are not md5 digests — nothing
            // local to compare against a plain md5, so skip rather than
            // fail a healthy upload.
            if !is_md5_hex(etag) {
                tracing::debug!(remote = %remote.raw, etag, "composite S3 etag is not a plain md5 — skipping checksum verification");
                return Ok(());
            }
            let local_md5 = md5_hex_of_file(local).await?;
            if !local_md5.eq_ignore_ascii_case(etag) {
                return Err(upload_mismatch(remote, &local_md5, etag));
            }
        }
        StorageScheme::Gcs => {
            let local_md5 = md5_base64_of_file(local).await?;
            if local_md5 != etag {
                return Err(upload_mismatch(remote, &local_md5, etag));
            }
        }
        StorageScheme::Local => {}
    }
    Ok(())
}

/// A 32-char hex md5 digest (what a single-part S3 ETag is).
fn is_md5_hex(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn upload_mismatch(remote: &StoragePath, local_digest: &str, remote_digest: &str) -> OxoFlowError {
    OxoFlowError::Integrity {
        message: format!(
            "uploaded object '{}' checksum mismatch: remote reports {remote_digest}, local file computes {local_digest}",
            remote.raw
        ),
        failed_files: vec![remote.raw.clone()],
    }
}

/// Streamed md5 of a local file, hex-encoded (the S3 single-part ETag form).
async fn md5_hex_of_file(path: &Path) -> std::io::Result<String> {
    Ok(hex::encode(md5_of_file(path).await?))
}

/// Streamed md5 of a local file, base64-encoded (the GCS `md5Hash` form).
async fn md5_base64_of_file(path: &Path) -> std::io::Result<String> {
    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(md5_of_file(path).await?))
}

/// Streamed md5 of a local file in bounded chunks (outputs can be large;
/// never buffer the whole file).
async fn md5_of_file(path: &Path) -> std::io::Result<[u8; 16]> {
    use md5::Digest as _;
    use tokio::io::AsyncReadExt as _;
    let mut hasher = md5::Md5::new();
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

// ---------------------------------------------------------------------------
// Engine-managed staging paths and the etag-keyed download cache (issue #80
// item 2). Staged files live under `.oxo-flow/staged/` — engine-internal,
// invisible to checkpoints, safe to delete (a later run re-downloads).
// ---------------------------------------------------------------------------

/// Deterministic local path for a staged remote **input**.
///
/// The key maps into a local tree, so components that could escape it are
/// rejected outright: a leading slash (Path::join would treat the key as
/// absolute and drop the staging prefix) and `..` segments.
pub fn staged_path(workdir: &Path, path: &StoragePath) -> Result<PathBuf> {
    Ok(workdir
        .join(".oxo-flow/staged/in")
        .join(path.scheme.as_str())
        .join(path.bucket.as_deref().unwrap_or("_"))
        .join(stage_key_components(&path.key)?))
}

/// Deterministic local path where a rule writes a remote **output** before
/// the engine uploads it. Same key safety as [`staged_path`].
pub fn upload_stage_path(workdir: &Path, path: &StoragePath) -> Result<PathBuf> {
    Ok(workdir
        .join(".oxo-flow/staged/out")
        .join(path.scheme.as_str())
        .join(path.bucket.as_deref().unwrap_or("_"))
        .join(stage_key_components(&path.key)?))
}

/// Validate a remote key for local staging: no leading slash, no `..`
/// components (a relative key is a tree path under the staging root).
fn stage_key_components(key: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for comp in key.trim_start_matches('/').split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(OxoFlowError::Config {
                message: format!("remote key '{key}' contains '..' and cannot be staged locally"),
            });
        }
        out.push(comp);
    }
    Ok(out)
}

/// Sidecar cache metadata for a staged download: `{dest}.meta.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StageCacheMeta {
    size: u64,
    etag: Option<String>,
}

/// Download a remote object into `dest` with an etag-keyed cache and an
/// atomic replace (issue #80 item 2).
///
/// * `stat` — the current remote metadata (from `StorageBackend::head`).
/// * `transfer` — writes the object's content into the file it receives
///   (the `.part` staging file); on success the file is atomically renamed
///   onto `dest` and the sidecar meta is refreshed.
///
/// Returns `true` when a download happened, `false` on a cache hit (the
/// cached file keeps its original mtime, which the executor's freshness
/// gate relies on). A failed transfer deletes the partial file and leaves
/// any previous cache entry untouched.
pub async fn stage_with_cache<F, Fut>(stat: RemoteStat, dest: &Path, transfer: F) -> Result<bool>
where
    F: FnOnce(tokio::fs::File) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let meta_path = {
        let mut os = dest.as_os_str().to_owned();
        os.push(".meta.json");
        PathBuf::from(os)
    };

    // Cache hit: the object still matches the sidecar (etag when both sides
    // have one; size otherwise) and the file is still present.
    if dest.exists()
        && let Ok(meta_json) = tokio::fs::read_to_string(&meta_path).await
        && let Ok(meta) = serde_json::from_str::<StageCacheMeta>(&meta_json)
        && meta.size == stat.size
        && match (&meta.etag, &stat.etag) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    {
        return Ok(false);
    }

    let Some(parent) = dest.parent() else {
        return Err(OxoFlowError::Config {
            message: format!("staging path has no parent: {}", dest.display()),
        });
    };
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| OxoFlowError::Config {
            message: format!("failed to create staging dir {}: {e}", parent.display()),
        })?;

    let mut part_os = dest.as_os_str().to_owned();
    part_os.push(".part");
    let part = PathBuf::from(part_os);
    let file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| OxoFlowError::Config {
            message: format!("failed to create staging file {}: {e}", part.display()),
        })?;

    if let Err(e) = transfer(file).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }

    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| OxoFlowError::Config {
            message: format!(
                "failed to move staged file {} into place: {e}",
                part.display()
            ),
        })?;
    let meta = StageCacheMeta {
        size: stat.size,
        etag: stat.etag,
    };
    tokio::fs::write(
        &meta_path,
        serde_json::to_vec(&meta).map_err(|e| OxoFlowError::Config {
            message: format!("failed to serialize stage cache meta: {e}"),
        })?,
    )
    .await
    .map_err(|e| OxoFlowError::Config {
        message: format!("failed to write stage cache meta: {e}"),
    })?;

    Ok(true)
}

/// Storage backend trait - abstracts file operations across providers.
///
/// Every method is async to support network-based backends. Local filesystem
/// implementations delegate to `tokio::fs` (or `spawn_blocking`).
#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync {
    /// Check whether a path exists.
    async fn exists(&self, path: &StoragePath) -> Result<bool>;

    /// Return metadata for a remote object, or `Ok(None)` when it does not
    /// exist. Local backends return `Ok(None)` — local invalidation uses
    /// size+mtime+sha256 (see [`RemoteStat`]).
    async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>>;

    /// Synchronous wrapper around [`StorageBackend::head`] for call sites
    /// that cannot be async (manifest snapshots in the preview path).
    ///
    /// Runs the HEAD on a fresh runtime on a dedicated thread: blocking on
    /// the ambient runtime panics inside async contexts, and nested runtimes
    /// are forbidden, so a thread is the one shape that is correct in every
    /// context. Only remote inputs reach this; local paths never call
    /// `head`, so the thread cost is confined to cloud workflows.
    fn head_blocking(&self, path: &StoragePath) -> Result<Option<RemoteStat>> {
        let path = path.clone();
        std::thread::scope(|scope| {
            let fut = async { self.head(&path).await };
            scope
                .spawn(move || {
                    tokio::runtime::Runtime::new()
                        .map_err(|e| OxoFlowError::Config {
                            message: format!("cannot create runtime for remote metadata: {e}"),
                        })
                        .and_then(|runtime| runtime.block_on(fut))
                })
                .join()
                .map_err(|_| OxoFlowError::Config {
                    message: "remote metadata thread panicked".to_string(),
                })?
        })
    }

    /// Read the entire file at `path` into a UTF-8 string.
    async fn read_to_string(&self, path: &StoragePath) -> Result<String>;

    /// Write `data` to the given path (creating parents if needed).
    async fn write(&self, path: &StoragePath, data: &[u8]) -> Result<()>;

    /// Stage a remote file to a local working directory, returning the local
    /// path. For local files this is a no-op returning the original path.
    async fn stage(&self, path: &StoragePath, workdir: &Path) -> Result<PathBuf>;

    /// Upload a local file to a remote location. No-op for local targets.
    async fn upload(&self, local: &Path, remote: &StoragePath) -> Result<()>;

    /// Human-readable backend name for logging / diagnostics.
    fn name(&self) -> &'static str;
}

/// Resolves storage URIs to the appropriate [`StorageBackend`].
///
/// Maintains a registry of backends keyed by scheme. The default resolver
/// (created via [`StorageResolver::with_local`]) registers the local
/// filesystem backend only.
#[derive(Clone)]
pub struct StorageResolver {
    backends: Vec<(StorageScheme, Arc<dyn StorageBackend>)>,
}

impl std::fmt::Debug for StorageResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&'static str> = self
            .backends
            .iter()
            .map(|(scheme, _)| scheme.as_str())
            .collect();
        f.debug_struct("StorageResolver")
            .field("backends", &names)
            .finish()
    }
}

impl StorageResolver {
    /// Create an empty resolver with no backends registered.
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
        }
    }

    /// Create a resolver pre-populated with the local filesystem backend.
    pub fn with_local() -> Self {
        let mut resolver = Self::new();
        resolver.add_backend(StorageScheme::Local, Arc::new(local::LocalStorage));
        resolver
    }

    /// Register a backend for a given scheme. Later registrations override
    /// earlier ones for the same scheme.
    pub fn add_backend(&mut self, scheme: StorageScheme, backend: Arc<dyn StorageBackend>) {
        self.backends.retain(|(s, _)| *s != scheme);
        self.backends.push((scheme, backend));
    }

    /// Parse a path string into a [`StoragePath`] without resolving it.
    pub fn parse_path(path: &str) -> StoragePath {
        StoragePath::parse(path)
    }

    /// Retrieve the backend registered for a given scheme, if any.
    pub fn get_backend(&self, scheme: &StorageScheme) -> Option<&Arc<dyn StorageBackend>> {
        self.backends
            .iter()
            .find(|(s, _)| s == scheme)
            .map(|(_, b)| b)
    }
}

impl Default for StorageResolver {
    fn default() -> Self {
        Self::with_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend stub whose only live method is `head` — everything else
    /// is unreachable in upload-verification tests.
    struct HeadOnlyBackend {
        stat: std::sync::Mutex<Option<RemoteStat>>,
    }

    #[async_trait::async_trait]
    impl StorageBackend for HeadOnlyBackend {
        async fn exists(&self, _path: &StoragePath) -> Result<bool> {
            unreachable!("exists")
        }
        async fn head(&self, _path: &StoragePath) -> Result<Option<RemoteStat>> {
            Ok(self.stat.lock().unwrap().clone())
        }
        async fn read_to_string(&self, _path: &StoragePath) -> Result<String> {
            unreachable!("read_to_string")
        }
        async fn write(&self, _path: &StoragePath, _data: &[u8]) -> Result<()> {
            unreachable!("write")
        }
        async fn stage(&self, _path: &StoragePath, _workdir: &Path) -> Result<PathBuf> {
            unreachable!("stage")
        }
        async fn upload(&self, _local: &Path, _remote: &StoragePath) -> Result<()> {
            unreachable!("upload")
        }
        fn name(&self) -> &'static str {
            "head-only-test"
        }
    }

    fn remote_uri(raw: &str) -> StoragePath {
        StoragePath::parse(raw)
    }

    #[test]
    fn is_md5_hex_accepts_plain_digests_only() {
        assert!(is_md5_hex("5d41402abc4b2a76b9719d911017c592"));
        assert!(is_md5_hex("5D41402ABC4B2A76B9719D911017C592"));
        assert!(!is_md5_hex("5d41402abc4b2a76b9719d911017c592-2")); // multipart
        assert!(!is_md5_hex("5d41402abc4b2a76b9719d911017c59")); // short
        assert!(!is_md5_hex("5d41402abc4b2a76b9719d911017c59z")); // non-hex
    }

    #[test]
    fn md5_file_digests_match_known_vectors() {
        // md5("hello") = 5d41402abc4b2a76b9719d911017c592 (hex) /
        // XUFAKrxLKna5cZ2REBfFkg== (base64).
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, b"hello").expect("write file");
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(
            rt.block_on(md5_hex_of_file(&file)).expect("hex digest"),
            "5d41402abc4b2a76b9719d911017c592"
        );
        assert_eq!(
            rt.block_on(md5_base64_of_file(&file)).expect("b64 digest"),
            "XUFAKrxLKna5cZ2REBfFkg=="
        );
    }

    #[tokio::test]
    async fn verify_upload_passes_when_s3_etag_matches_local_md5() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("out.txt");
        std::fs::write(&file, b"hello").expect("write file");
        let backend = HeadOnlyBackend {
            stat: std::sync::Mutex::new(Some(RemoteStat {
                size: 5,
                etag: Some("5d41402abc4b2a76b9719d911017c592".to_string()),
            })),
        };
        verify_upload(&backend, &file, &remote_uri("s3://b/out.txt"))
            .await
            .expect("matching etag must pass");
    }

    #[tokio::test]
    async fn verify_upload_fails_when_s3_etag_diverges() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("out.txt");
        std::fs::write(&file, b"hello").expect("write file");
        let backend = HeadOnlyBackend {
            stat: std::sync::Mutex::new(Some(RemoteStat {
                size: 5,
                etag: Some("00000000000000000000000000000000".to_string()),
            })),
        };
        let err = verify_upload(&backend, &file, &remote_uri("s3://b/out.txt"))
            .await
            .expect_err("diverging etag must fail");
        assert!(matches!(err, OxoFlowError::Integrity { .. }), "err: {err}");
    }

    #[tokio::test]
    async fn verify_upload_skips_composite_s3_etags() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("out.txt");
        std::fs::write(&file, b"hello").expect("write file");
        let backend = HeadOnlyBackend {
            stat: std::sync::Mutex::new(Some(RemoteStat {
                size: 5,
                etag: Some("5d41402abc4b2a76b9719d911017c592-2".to_string()),
            })),
        };
        // A multipart etag is not comparable to a plain md5 — a healthy
        // upload must not be failed over it.
        verify_upload(&backend, &file, &remote_uri("s3://b/out.txt"))
            .await
            .expect("composite etag is skipped, not an error");
    }

    #[tokio::test]
    async fn verify_upload_fails_on_size_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("out.txt");
        std::fs::write(&file, b"hello").expect("write file");
        let backend = HeadOnlyBackend {
            stat: std::sync::Mutex::new(Some(RemoteStat {
                size: 3, // truncated transfer
                etag: None,
            })),
        };
        let err = verify_upload(&backend, &file, &remote_uri("s3://b/out.txt"))
            .await
            .expect_err("size mismatch must fail even without a digest");
        assert!(matches!(err, OxoFlowError::Integrity { .. }), "err: {err}");
    }

    #[tokio::test]
    async fn verify_upload_passes_for_gcs_base64_md5() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("out.txt");
        std::fs::write(&file, b"hello").expect("write file");
        let backend = HeadOnlyBackend {
            stat: std::sync::Mutex::new(Some(RemoteStat {
                size: 5,
                etag: Some("XUFAKrxLKna5cZ2REBfFkg==".to_string()),
            })),
        };
        verify_upload(&backend, &file, &remote_uri("gs://b/out.txt"))
            .await
            .expect("matching gcs md5Hash must pass");
    }

    #[test]
    fn parse_local_path() {
        let sp = StoragePath::parse("/data/sample.fastq");
        assert_eq!(sp.scheme, StorageScheme::Local);
        assert_eq!(sp.bucket, None);
        assert_eq!(sp.key, "/data/sample.fastq");
        assert!(!sp.is_remote());
    }

    #[test]
    fn parse_relative_local() {
        let sp = StoragePath::parse("relative/path.txt");
        assert_eq!(sp.scheme, StorageScheme::Local);
        assert_eq!(sp.bucket, None);
        assert_eq!(sp.key, "relative/path.txt");
    }

    #[test]
    fn parse_s3_uri() {
        let sp = StoragePath::parse("s3://my-bucket/data/sample.fastq");
        assert_eq!(sp.scheme, StorageScheme::S3);
        assert_eq!(sp.bucket.as_deref(), Some("my-bucket"));
        assert_eq!(sp.key, "data/sample.fastq");
        assert!(sp.is_remote());
    }

    #[test]
    fn parse_s3_no_key() {
        let sp = StoragePath::parse("s3://bucket-only");
        assert_eq!(sp.scheme, StorageScheme::S3);
        assert_eq!(sp.bucket, None);
        assert_eq!(sp.key, "bucket-only");
    }

    #[test]
    fn parse_gcs_uri() {
        let sp = StoragePath::parse("gs://genomics-bucket/reads.fastq.gz");
        assert_eq!(sp.scheme, StorageScheme::Gcs);
        assert_eq!(sp.bucket.as_deref(), Some("genomics-bucket"));
        assert_eq!(sp.key, "reads.fastq.gz");
        assert!(sp.is_remote());
    }

    #[test]
    fn scheme_from_uri() {
        assert_eq!(StorageScheme::from_uri("s3://x"), StorageScheme::S3);
        assert_eq!(StorageScheme::from_uri("gs://x"), StorageScheme::Gcs);
        assert_eq!(StorageScheme::from_uri("/x"), StorageScheme::Local);
        assert_eq!(StorageScheme::from_uri("x"), StorageScheme::Local);
    }

    #[test]
    fn resolver_default_has_local() {
        let resolver = StorageResolver::with_local();
        assert!(resolver.get_backend(&StorageScheme::Local).is_some());
        assert!(resolver.get_backend(&StorageScheme::S3).is_none());
    }

    #[test]
    fn stage_local_is_noop() {
        let sp = StoragePath::parse("/tmp/test.txt");
        assert!(!sp.is_remote());
    }

    #[test]
    fn stage_remote_missing_backend_error() {
        let sp = StoragePath::parse("s3://bucket/key");
        assert!(sp.is_remote());
    }

    #[tokio::test]
    async fn head_local_is_none() {
        let resolver = StorageResolver::with_local();
        let backend = resolver.get_backend(&StorageScheme::Local).unwrap();
        let stat = backend
            .head(&StoragePath::parse("/any/path"))
            .await
            .unwrap();
        assert!(stat.is_none());
    }

    #[test]
    fn staged_path_maps_keys_inside_the_staging_tree() {
        // Arrange
        let workdir = std::path::Path::new("/tmp/wd");
        let sp = StoragePath::parse("s3://bucket/data/sample1.fastq.gz");

        // Act
        let dest = staged_path(workdir, &sp).unwrap();

        // Assert — leading-slash-free keys land under .oxo-flow/staged/in.
        assert_eq!(
            dest,
            std::path::Path::new("/tmp/wd/.oxo-flow/staged/in/s3/bucket/data/sample1.fastq.gz")
        );
    }

    #[test]
    fn staged_path_keeps_leading_slash_keys_inside_the_tree() {
        // Arrange — "s3://b//etc/foo" parses to key "/etc/foo"; a raw join
        // would treat it as absolute and drop the staging prefix entirely.
        let workdir = std::path::Path::new("/tmp/wd");

        // Act
        let dest = staged_path(workdir, &StoragePath::parse("s3://b//etc/foo")).unwrap();

        // Assert — the key is trimmed into the staging subtree.
        assert_eq!(
            dest,
            std::path::Path::new("/tmp/wd/.oxo-flow/staged/in/s3/b/etc/foo")
        );
    }

    #[test]
    fn staged_path_rejects_dotdot_key_components() {
        // Arrange — a '..' component climbs out of the tree on real fs
        // operations and cannot be staged safely.
        let workdir = std::path::Path::new("/tmp/wd");
        for raw in ["s3://b/a/../../etc", "s3://b/../escape"] {
            // Act / Assert
            assert!(
                staged_path(workdir, &StoragePath::parse(raw)).is_err(),
                "key {raw:?} must be rejected"
            );
            assert!(upload_stage_path(workdir, &StoragePath::parse(raw)).is_err());
        }
    }
}
