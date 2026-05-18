//! Local filesystem storage backend.
//!
//! Wraps `tokio::fs` for async file operations.  `stage` and `upload` are
//! no-ops since local paths need no staging.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::storage::{StorageBackend, StoragePath};

/// Local filesystem implementation of [`StorageBackend`].
///
/// All operations delegate to `tokio::fs` for non-blocking I/O. The `stage`
/// and `upload` methods are no-ops because local files are already on the
/// local filesystem.
pub struct LocalStorage;

#[async_trait::async_trait]
impl StorageBackend for LocalStorage {
    async fn exists(&self, path: &StoragePath) -> Result<bool> {
        Ok(tokio::fs::try_exists(&path.key).await?)
    }

    async fn read_to_string(&self, path: &StoragePath) -> Result<String> {
        Ok(tokio::fs::read_to_string(&path.key).await?)
    }

    async fn write(&self, path: &StoragePath, data: &[u8]) -> Result<()> {
        let p = Path::new(&path.key);
        if let Some(parent) = p.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        Ok(tokio::fs::write(p, data).await?)
    }

    async fn stage(&self, path: &StoragePath, _workdir: &Path) -> Result<PathBuf> {
        // Local files are already local -- nothing to stage.
        Ok(PathBuf::from(&path.key))
    }

    async fn upload(&self, _local: &Path, _remote: &StoragePath) -> Result<()> {
        // Local-to-local copy is a no-op (caller already has the file).
        Ok(())
    }

    fn name(&self) -> &'static str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoragePath;

    #[tokio::test]
    async fn test_exists_true() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, b"hello").await.unwrap();
        let sp = StoragePath::parse(file_path.to_str().unwrap());
        let backend = LocalStorage;
        assert!(backend.exists(&sp).await.unwrap());
    }

    #[tokio::test]
    async fn test_exists_false() {
        let sp = StoragePath::parse("/nonexistent/path/12345");
        let backend = LocalStorage;
        assert!(!backend.exists(&sp).await.unwrap());
    }

    #[tokio::test]
    async fn test_read_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("out.txt");
        let sp = StoragePath::parse(file_path.to_str().unwrap());
        let backend = LocalStorage;

        backend.write(&sp, b"hello world").await.unwrap();
        let content = backend.read_to_string(&sp).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_stage_local_noop() {
        let dir = tempfile::tempdir().unwrap();
        let sp = StoragePath::parse("/some/local/path.bam");
        let backend = LocalStorage;
        let result = backend.stage(&sp, dir.path()).await.unwrap();
        assert_eq!(result, PathBuf::from("/some/local/path.bam"));
    }

    #[tokio::test]
    async fn test_upload_local_noop() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("test.txt");
        let remote = StoragePath::parse("/some/remote/path.txt");
        let backend = LocalStorage;
        // Should succeed without doing anything.
        backend.upload(&local, &remote).await.unwrap();
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c").join("nested.txt");
        let sp = StoragePath::parse(nested.to_str().unwrap());
        let backend = LocalStorage;
        backend.write(&sp, b"nested").await.unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn test_name() {
        assert_eq!(LocalStorage.name(), "local");
    }
}
