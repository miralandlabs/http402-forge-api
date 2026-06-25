mod local;
mod r2;

use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use crate::config::{AppConfig, StorageBackend};
use crate::error::AppResult;

pub use local::LocalStorage;
pub use r2::R2Storage;

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put(&self, key: &str, content_type: &str, data: Bytes) -> AppResult<()>;
    async fn get(&self, key: &str) -> AppResult<(Bytes, String)>;
    async fn stream(&self, key: &str) -> AppResult<(ByteStream, String)>;
}

pub enum Storage {
    Local(LocalStorage),
    R2(R2Storage),
}

#[async_trait]
impl ObjectStore for Storage {
    async fn put(&self, key: &str, content_type: &str, data: Bytes) -> AppResult<()> {
        match self {
            Self::Local(s) => s.put(key, content_type, data).await,
            Self::R2(s) => s.put(key, content_type, data).await,
        }
    }

    async fn get(&self, key: &str) -> AppResult<(Bytes, String)> {
        match self {
            Self::Local(s) => s.get(key).await,
            Self::R2(s) => s.get(key).await,
        }
    }

    async fn stream(&self, key: &str) -> AppResult<(ByteStream, String)> {
        match self {
            Self::Local(s) => s.stream(key).await,
            Self::R2(s) => s.stream(key).await,
        }
    }
}

impl Storage {
    pub async fn from_config(config: &AppConfig) -> AppResult<Self> {
        match config.storage_backend {
            StorageBackend::Local => {
                let store = LocalStorage::new(config.local_storage_path.clone())?;
                Ok(Self::Local(store))
            }
            StorageBackend::R2 => {
                let store = R2Storage::from_config(config).await?;
                Ok(Self::R2(store))
            }
        }
    }
}

pub fn object_key(prefix: &str, id: uuid::Uuid, filename: &str) -> String {
    let safe: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{prefix}/{id}/{safe}")
}
