use std::convert::TryFrom;

use axum::body::Body as AxumBody;
use eyre::{Result, WrapErr};
use http_body_util::BodyExt;
use hyper::{Request, Response};
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::ports::file_system::{FileSystem, FileSystemError};

/// File system adapter using tower-http ServeDir for static file serving
#[derive(Debug, Default, Clone)]
pub struct FileSystemAdapter;

impl FileSystemAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl FileSystem for FileSystemAdapter {
    async fn serve_file(
        &self,
        root: &str,
        path: &str,
        req: Request<AxumBody>,
    ) -> Result<Response<AxumBody>, FileSystemError> {
        let root = root.to_string();
        let path = path.to_string();

        // Create a new request with the path adjusted for ServeDir
        let uri_string = format!("/{path}", path = path.trim_start_matches('/'));
        let uri = hyper::Uri::try_from(uri_string)
            .wrap_err("Failed to parse URI for file serving")
            .map_err(|e| FileSystemError::InvalidPath(e.to_string()))?;

        let (parts, body) = req.into_parts();
        let mut new_req = Request::from_parts(parts, body);
        *new_req.uri_mut() = uri;

        // Use ServeDir from tower-http with index file support
        let serve_dir = ServeDir::new(&root).append_index_html_on_directories(true); // 自动添加 index.html 到目录路径

        let response = serve_dir
            .oneshot(new_req)
            .await
            .wrap_err("ServeDir failed to serve file")
            .map_err(|e| {
                FileSystemError::IoError(std::io::Error::other(format!("ServeDir error: {e}")))
            })?;

        let (parts, tower_body) = response.into_parts();
        let axum_body = AxumBody::new(tower_body.map_err(|e| {
            tracing::error!("Error reading static file body: {}", e);
            // Convert Infallible to a type compatible with AxumBody's error
            axum::Error::new(e)
        }));

        Ok(Response::from_parts(parts, axum_body))
    }
}

impl FileSystemAdapter {
    // Helper methods for additional file operations outside the trait
    // These are implementation-specific methods that go beyond the port interface
    pub async fn file_exists(&self, root: &str, path: &str) -> Result<bool, FileSystemError> {
        let full_path = std::path::Path::new(root).join(path.trim_start_matches('/'));

        // Security check: ensure the resolved path is still within root
        let canonical_root = std::fs::canonicalize(root).map_err(FileSystemError::IoError)?;
        let canonical_path = match std::fs::canonicalize(&full_path) {
            Ok(p) => p,
            Err(e) => {
                return if e.kind() == std::io::ErrorKind::NotFound {
                    // If the path doesn't exist, treat as not found only if it would be within root
                    // Check the parent directory canonicalization
                    match full_path
                        .parent()
                        .and_then(|p| std::fs::canonicalize(p).ok())
                    {
                        Some(parent) if parent.starts_with(&canonical_root) => Ok(false),
                        _ => Err(FileSystemError::InvalidPath(
                            "Path traversal attempt detected".to_string(),
                        )),
                    }
                } else {
                    Err(FileSystemError::IoError(e))
                };
            }
        };

        if !canonical_path.starts_with(&canonical_root) {
            return Err(FileSystemError::InvalidPath(
                "Path traversal attempt detected".to_string(),
            ));
        }

        match tokio::fs::metadata(&full_path).await {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(FileSystemError::IoError(e)),
        }
    }

    pub async fn read_file(&self, root: &str, path: &str) -> Result<Vec<u8>, FileSystemError> {
        let full_path = std::path::Path::new(root).join(path.trim_start_matches('/'));

        // Security check: ensure the resolved path is still within root
        let canonical_root = std::fs::canonicalize(root).map_err(FileSystemError::IoError)?;
        let canonical_path = std::fs::canonicalize(&full_path).map_err(FileSystemError::IoError)?;

        if !canonical_path.starts_with(&canonical_root) {
            return Err(FileSystemError::InvalidPath(
                "Path traversal attempt detected".to_string(),
            ));
        }

        tokio::fs::read(&full_path)
            .await
            .map_err(FileSystemError::IoError)
    }

    pub async fn write_file(
        &self,
        root: &str,
        path: &str,
        content: &[u8],
    ) -> Result<(), FileSystemError> {
        let full_path = std::path::Path::new(root).join(path.trim_start_matches('/'));

        // Security check: ensure the resolved path is still within root
        let canonical_root = std::fs::canonicalize(root).map_err(FileSystemError::IoError)?;

        // Create parent directories if they don't exist
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(FileSystemError::IoError)?;
        }

        // Verify the path after parent creation
        let canonical_path = std::fs::canonicalize(full_path.parent().unwrap_or(&full_path))
            .map_err(FileSystemError::IoError)?;

        if !canonical_path.starts_with(&canonical_root) {
            return Err(FileSystemError::InvalidPath(
                "Path traversal attempt detected".to_string(),
            ));
        }

        tokio::fs::write(&full_path, content)
            .await
            .map_err(FileSystemError::IoError)
    }

    pub async fn delete_file(&self, root: &str, path: &str) -> Result<(), FileSystemError> {
        let full_path = std::path::Path::new(root).join(path.trim_start_matches('/'));

        // Security check: ensure the resolved path is still within root
        let canonical_root = std::fs::canonicalize(root).map_err(FileSystemError::IoError)?;
        let canonical_path = std::fs::canonicalize(&full_path).map_err(FileSystemError::IoError)?;

        if !canonical_path.starts_with(&canonical_root) {
            return Err(FileSystemError::InvalidPath(
                "Path traversal attempt detected".to_string(),
            ));
        }

        tokio::fs::remove_file(&full_path)
            .await
            .map_err(FileSystemError::IoError)
    }

    pub async fn list_directory(
        &self,
        root: &str,
        path: &str,
    ) -> Result<Vec<String>, FileSystemError> {
        let full_path = std::path::Path::new(root).join(path.trim_start_matches('/'));

        // Security check: ensure the resolved path is still within root
        let canonical_root = std::fs::canonicalize(root).map_err(FileSystemError::IoError)?;
        let canonical_path = std::fs::canonicalize(&full_path).map_err(FileSystemError::IoError)?;

        if !canonical_path.starts_with(&canonical_root) {
            return Err(FileSystemError::InvalidPath(
                "Path traversal attempt detected".to_string(),
            ));
        }

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&full_path)
            .await
            .map_err(FileSystemError::IoError)?;

        while let Some(entry) = dir.next_entry().await.map_err(FileSystemError::IoError)? {
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    async fn create_test_file(dir: &TempDir, path: &str, content: &str) -> std::io::Result<()> {
        let full_path = dir.path().join(path);
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(full_path, content).await
    }

    #[tokio::test]
    async fn test_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        let fs = FileSystemAdapter::new();

        // Create a test file
        create_test_file(&temp_dir, "test.txt", "content")
            .await
            .unwrap();

        let root = temp_dir.path().to_str().unwrap();

        assert!(fs.file_exists(root, "test.txt").await.unwrap());
        assert!(!fs.file_exists(root, "nonexistent.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_read_file() {
        let temp_dir = TempDir::new().unwrap();
        let fs = FileSystemAdapter::new();
        let content = "Hello, world!";

        // Create a test file
        create_test_file(&temp_dir, "test.txt", content)
            .await
            .unwrap();

        let root = temp_dir.path().to_str().unwrap();
        let result = fs.read_file(root, "test.txt").await.unwrap();

        assert_eq!(result, content.as_bytes());
    }

    #[tokio::test]
    async fn test_write_file() {
        let temp_dir = TempDir::new().unwrap();
        let fs = FileSystemAdapter::new();
        let content = b"Test content";

        let root = temp_dir.path().to_str().unwrap();
        fs.write_file(root, "new_file.txt", content).await.unwrap();

        let result = fs.read_file(root, "new_file.txt").await.unwrap();
        assert_eq!(result, content);
    }

    #[tokio::test]
    async fn test_path_traversal_protection() {
        let temp_dir = TempDir::new().unwrap();
        let fs = FileSystemAdapter::new();

        let root = temp_dir.path().to_str().unwrap();

        // Try to access parent directory
        let result = fs.file_exists(root, "../../../etc/passwd").await;
        assert!(result.is_err());

        let result = fs.read_file(root, "../sensitive_file.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_directory() {
        let temp_dir = TempDir::new().unwrap();
        let fs = FileSystemAdapter::new();

        // Create test files
        create_test_file(&temp_dir, "file1.txt", "content1")
            .await
            .unwrap();
        create_test_file(&temp_dir, "file2.txt", "content2")
            .await
            .unwrap();
        create_test_file(&temp_dir, "subdir/file3.txt", "content3")
            .await
            .unwrap();

        let root = temp_dir.path().to_str().unwrap();
        let mut entries = fs.list_directory(root, "/").await.unwrap();
        entries.sort();

        assert_eq!(entries.len(), 3);
        assert!(entries.contains(&"file1.txt".to_string()));
        assert!(entries.contains(&"file2.txt".to_string()));
        assert!(entries.contains(&"subdir".to_string()));
    }
}
