//! S3 storage backend (stub).
//!
//! This module is only compiled when the `s3-storage` feature is enabled.
//! The current implementation is a stub that returns a clear error message
//! indicating that full S3 support requires the feature flag.
//!
//! Once the `aws-sdk-s3` integration is wired up, replace the stub methods
//! below with real SDK calls.

use std::path::{Path, PathBuf};

use crate::error::{OxoFlowError, Result};
use crate::storage::{StorageBackend, StoragePath};

/// S3 storage backend (stub).
///
/// All methods return an error instructing the caller to enable the
/// `s3-storage` feature flag.
pub struct S3Storage;

#[async_trait::async_trait]
impl StorageBackend for S3Storage {
    async fn exists(&self, _path: &StoragePath) -> Result<bool> {
        Err(s3_not_available())
    }

    async fn read_to_string(&self, _path: &StoragePath) -> Result<String> {
        Err(s3_not_available())
    }

    async fn write(&self, _path: &StoragePath, _data: &[u8]) -> Result<()> {
        Err(s3_not_available())
    }

    async fn stage(&self, _path: &StoragePath, _workdir: &Path) -> Result<PathBuf> {
        Err(s3_not_available())
    }

    async fn upload(&self, _local: &Path, _remote: &StoragePath) -> Result<()> {
        Err(s3_not_available())
    }

    fn name(&self) -> &'static str {
        "s3 (stub)"
    }
}

fn s3_not_available() -> OxoFlowError {
    OxoFlowError::Config {
        message: "S3 storage support requires the 's3-storage' feature flag: \
                  add `s3-storage` to `[features]` in your Cargo.toml or enable it \
                  when building oxo-flow"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoragePath;

    #[tokio::test]
    async fn test_s3_stub_returns_error() {
        let sp = StoragePath::parse("s3://bucket/key.txt");
        let backend = S3Storage;
        let err = backend.exists(&sp).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("s3-storage"),
            "error should mention the feature flag: {msg}"
        );
    }

    #[tokio::test]
    async fn test_s3_stub_name() {
        assert_eq!(S3Storage.name(), "s3 (stub)");
    }
}
