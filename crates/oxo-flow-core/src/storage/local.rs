//! Local filesystem storage backend.
//!
//! Wraps `tokio::fs` for async file operations.  `stage` and `upload` are
//! no-ops since local paths need no staging.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::storage::{RemoteStat, StorageBackend, StoragePath};

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

    async fn head(&self, _path: &StoragePath) -> Result<Option<RemoteStat>> {
        Ok(None)
    }

    async fn read_to_string(&self, path: &StoragePath) -> Result<String> {
        Ok(tokio::fs::read_to_string(&path.key).await?)
    }

    async fn read_range(&self, path: &StoragePath, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let p = Path::new(&path.key);
        let mut file = tokio::fs::File::open(p).await?;
        let metadata = file.metadata().await?;
        let file_len = metadata.len();
        if offset >= file_len {
            return Ok(Vec::new());
        }
        use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
        file.seek(SeekFrom::Start(offset)).await?;
        let to_read = (len as u64).min(file_len - offset) as usize;
        let mut buf = vec![0u8; to_read];
        file.read_exact(&mut buf).await?;
        Ok(buf)
    }

    async fn write(&self, path: &StoragePath, data: &[u8]) -> Result<()> {
        let p = Path::new(&path.key);
        let parent = crate::parent_dir(p);
        if parent != Path::new(".") {
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

    #[tokio::test]
    async fn test_read_range_slices() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("range.txt");
        tokio::fs::write(&file_path, b"hello world from oxoflow")
            .await
            .unwrap();
        let sp = StoragePath::parse(file_path.to_str().unwrap());
        let backend = LocalStorage;

        // Zero len
        assert_eq!(backend.read_range(&sp, 0, 0).await.unwrap(), b"");
        // Start slice
        assert_eq!(backend.read_range(&sp, 0, 5).await.unwrap(), b"hello");
        // Middle slice
        assert_eq!(backend.read_range(&sp, 6, 5).await.unwrap(), b"world");
        // Past EOF
        assert_eq!(backend.read_range(&sp, 100, 10).await.unwrap(), b"");
        // Partial (len larger than remaining)
        assert_eq!(backend.read_range(&sp, 17, 100).await.unwrap(), b"oxoflow");
    }
}
