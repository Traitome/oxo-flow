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

/// Storage backend trait - abstracts file operations across providers.
///
/// Every method is async to support network-based backends. Local filesystem
/// implementations delegate to `tokio::fs` (or `spawn_blocking`).
#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync {
    /// Check whether a path exists.
    async fn exists(&self, path: &StoragePath) -> Result<bool>;

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
pub struct StorageResolver {
    backends: Vec<(StorageScheme, Arc<dyn StorageBackend>)>,
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

    /// Stage a remote file locally if needed, returning the local path to use.
    ///
    /// For local paths this is a no-op; for remote paths the corresponding
    /// backend's `stage` method is called.
    pub async fn stage_if_remote(&self, path_str: &str, workdir: &Path) -> Result<PathBuf> {
        let sp = StoragePath::parse(path_str);
        if !sp.is_remote() {
            return Ok(PathBuf::from(path_str));
        }
        if let Some(backend) = self.get_backend(&sp.scheme) {
            backend.stage(&sp, workdir).await
        } else {
            Err(OxoFlowError::Config {
                message: format!(
                    "No storage backend available for scheme: {:?}. \
                     Enable the corresponding feature flag (e.g. 's3-storage')",
                    sp.scheme
                ),
            })
        }
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
}
