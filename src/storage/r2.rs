use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use reqwest::Client;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;

use super::{ByteStream, ObjectStore, PresignedPut};
use crate::config::AppConfig;
use crate::error::{AppError, AppResult};

pub struct R2Storage {
    bucket: Box<Bucket>,
    http: Client,
}

impl R2Storage {
    pub async fn from_config(config: &AppConfig) -> AppResult<Self> {
        let account_id = config
            .r2_account_id
            .as_ref()
            .ok_or_else(|| AppError::Storage("R2_ACCOUNT_ID required".into()))?;
        let bucket_name = config
            .r2_bucket
            .clone()
            .ok_or_else(|| AppError::Storage("R2_BUCKET required".into()))?;
        let access_key = config
            .r2_access_key_id
            .as_ref()
            .ok_or_else(|| AppError::Storage("R2_ACCESS_KEY_ID required".into()))?;
        let secret = config
            .r2_secret_access_key
            .as_ref()
            .ok_or_else(|| AppError::Storage("R2_SECRET_ACCESS_KEY required".into()))?;

        let endpoint = format!("https://{account_id}.r2.cloudflarestorage.com");
        let region = Region::Custom {
            region: "auto".into(),
            endpoint,
        };
        let creds = Credentials::new(Some(access_key), Some(secret), None, None, None)
            .map_err(|e| AppError::Storage(e.to_string()))?;
        let bucket = Bucket::new(&bucket_name, region, creds)
            .map_err(|e| AppError::Storage(e.to_string()))?
            .with_path_style();
        Ok(Self {
            bucket,
            http: Client::builder()
                .build()
                .map_err(|e| AppError::Storage(e.to_string()))?,
        })
    }

    async fn head_meta(&self, key: &str) -> AppResult<(String, u64)> {
        let (head, code) = self
            .bucket
            .head_object(key)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        if code == 404 {
            return Err(AppError::NotFound);
        }
        if code != 200 {
            return Err(AppError::Storage(format!("R2 head {key}: HTTP {code}")));
        }
        let content_type = head
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let size = head.content_length.unwrap_or(0).max(0) as u64;
        Ok((content_type, size))
    }
}

#[async_trait]
impl ObjectStore for R2Storage {
    async fn put(&self, key: &str, content_type: &str, data: Bytes) -> AppResult<()> {
        self.bucket
            .put_object_with_content_type(key, &data, content_type)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> AppResult<(Bytes, String)> {
        let (content_type, _) = self.head_meta(key).await?;
        let response = self
            .bucket
            .get_object(key)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        let bytes = response.bytes();
        if bytes.starts_with(b"<?xml") {
            let body = String::from_utf8_lossy(bytes);
            if body.contains("<Error>") {
                return Err(AppError::Storage(format!("R2 get {key}: {body}")));
            }
        }
        Ok((Bytes::from(bytes.to_vec()), content_type))
    }

    async fn head(&self, key: &str) -> AppResult<String> {
        Ok(self.head_meta(key).await?.0)
    }

    async fn object_size(&self, key: &str) -> AppResult<u64> {
        Ok(self.head_meta(key).await?.1)
    }

    async fn stream(&self, key: &str) -> AppResult<(ByteStream, String)> {
        let (content_type, _) = self.head_meta(key).await?;
        let url = self.presign_get(key, 300).await?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        if !response.status().is_success() {
            return Err(AppError::Storage(format!(
                "R2 stream GET failed: HTTP {}",
                response.status()
            )));
        }
        let stream = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other));
        Ok((Box::pin(stream), content_type))
    }

    async fn presign_get(&self, key: &str, ttl_secs: u32) -> AppResult<String> {
        self.bucket
            .presign_get(key, ttl_secs, None)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    async fn presign_put(
        &self,
        key: &str,
        content_type: &str,
        ttl_secs: u32,
    ) -> AppResult<PresignedPut> {
        let url = self
            .bucket
            .presign_put(key, ttl_secs, None, None)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(PresignedPut {
            object_key: key.to_string(),
            method: "PUT",
            url,
            headers: vec![("Content-Type".into(), content_type.to_string())],
        })
    }
}
