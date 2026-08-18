use async_trait::async_trait;
use bytes::Bytes;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use super::{ByteStream, ObjectStore};
use crate::error::{AppError, AppResult};

pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    pub fn new(root: PathBuf) -> AppResult<Self> {
        std::fs::create_dir_all(&root).map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(Self { root })
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.path_for(key).with_extension("meta")
    }

    async fn content_type_for(&self, key: &str) -> String {
        fs::read_to_string(self.meta_path(key))
            .await
            .unwrap_or_else(|_| "application/octet-stream".into())
    }
}

#[async_trait]
impl ObjectStore for LocalStorage {
    async fn put(&self, key: &str, content_type: &str, data: Bytes) -> AppResult<()> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        let mut file = fs::File::create(&path)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        file.write_all(&data)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        fs::write(self.meta_path(key), content_type)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn put_if_absent(&self, key: &str, content_type: &str, data: Bytes) -> AppResult<()> {
        let path = self.path_for(key);
        if path.exists() {
            return Err(AppError::Conflict(format!(
                "object key already exists: {key}"
            )));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        let mut file = fs::File::create(&path)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        file.write_all(&data)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        fs::write(self.meta_path(key), content_type)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> AppResult<(Bytes, String)> {
        let path = self.path_for(key);
        if !path.exists() {
            return Err(AppError::NotFound);
        }
        let data = fs::read(&path)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        let content_type = self.content_type_for(key).await;
        Ok((Bytes::from(data), content_type))
    }

    async fn head(&self, key: &str) -> AppResult<String> {
        let path = self.path_for(key);
        if !path.exists() {
            return Err(AppError::NotFound);
        }
        Ok(self.content_type_for(key).await)
    }

    async fn object_size(&self, key: &str) -> AppResult<u64> {
        let path = self.path_for(key);
        if !path.exists() {
            return Err(AppError::NotFound);
        }
        let meta = fs::metadata(&path)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(meta.len())
    }

    async fn stream(&self, key: &str) -> AppResult<(ByteStream, String)> {
        let path = self.path_for(key);
        if !path.exists() {
            return Err(AppError::NotFound);
        }
        let content_type = self.content_type_for(key).await;
        let file = fs::File::open(&path)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        let stream = ReaderStream::new(file);
        Ok((Box::pin(stream), content_type))
    }

    async fn presign_get(&self, _key: &str, _ttl_secs: u32) -> AppResult<String> {
        Err(AppError::Storage(
            "presigned GET requires STORAGE_BACKEND=r2".into(),
        ))
    }

    async fn presign_put(
        &self,
        _key: &str,
        _content_type: &str,
        _ttl_secs: u32,
    ) -> AppResult<super::PresignedPut> {
        Err(AppError::Storage(
            "presigned PUT requires STORAGE_BACKEND=r2".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::error::AppError;
    use crate::storage::{LocalStorage, ObjectStore};

    #[tokio::test]
    async fn put_if_absent_rejects_existing_key() {
        let root = std::env::temp_dir().join(format!("forge-storage-test-{}", uuid::Uuid::new_v4()));
        let store = LocalStorage::new(root.clone()).expect("storage");
        let key = "deadbeef";
        let data = Bytes::from_static(b"hello");
        store
            .put_if_absent(key, "text/plain", data.clone())
            .await
            .expect("first put");
        let err = store
            .put_if_absent(key, "text/plain", data)
            .await
            .expect_err("second put must fail");
        assert!(matches!(err, AppError::Conflict(_)));
        let _ = std::fs::remove_dir_all(root);
    }
}
