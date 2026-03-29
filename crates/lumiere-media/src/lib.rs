mod error;
mod service;
mod validation;

pub use error::MediaError;
pub use service::MediaService;
pub use validation::{AccountTier, FileValidation, validate_content_magic_bytes};

use lumiere_models::config::MinioConfig;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::Region;
use std::sync::Arc;

/// Low-level S3/MinIO client wrapper.
#[derive(Clone)]
pub struct S3Client {
    bucket: Arc<Bucket>,
}

impl S3Client {
    /// Connect to MinIO/S3 using the provided configuration.
    pub fn connect(config: &MinioConfig) -> Result<Self, MediaError> {
        let region = Region::Custom {
            region: config.region.clone(),
            endpoint: config.endpoint.clone(),
        };

        let credentials = Credentials::new(
            Some(&config.access_key),
            Some(&config.secret_key),
            None,
            None,
            None,
        )
        .map_err(|e| MediaError::Connection(e.to_string()))?;

        let mut bucket = Bucket::new(&config.bucket, region, credentials)
            .map_err(|e| MediaError::Connection(e.to_string()))?;

        if config.use_path_style {
            bucket.set_path_style();
        }

        tracing::info!(
            endpoint = %config.endpoint,
            bucket = %config.bucket,
            "Connected to S3/MinIO"
        );

        Ok(Self {
            bucket: Arc::new(*bucket),
        })
    }

    /// Upload bytes with a given object key and content type.
    pub async fn upload(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), MediaError> {
        let response = self
            .bucket
            .put_object_with_content_type(key, data, content_type)
            .await
            .map_err(|e| MediaError::Upload(e.to_string()))?;

        if response.status_code() >= 300 {
            return Err(MediaError::Upload(format!(
                "S3 upload failed with status {}",
                response.status_code()
            )));
        }

        tracing::debug!(key = %key, size = data.len(), "Uploaded object to S3");
        Ok(())
    }

    /// Download an object by key, returning the bytes.
    pub async fn download(&self, key: &str) -> Result<Vec<u8>, MediaError> {
        let response = self
            .bucket
            .get_object(key)
            .await
            .map_err(|e| MediaError::Download(e.to_string()))?;

        if response.status_code() == 404 {
            return Err(MediaError::NotFound(key.to_string()));
        }

        if response.status_code() >= 300 {
            return Err(MediaError::Download(format!(
                "S3 download failed with status {}",
                response.status_code()
            )));
        }

        Ok(response.to_vec())
    }

    /// Delete an object by key.
    pub async fn delete(&self, key: &str) -> Result<(), MediaError> {
        let response = self
            .bucket
            .delete_object(key)
            .await
            .map_err(|e| MediaError::Delete(e.to_string()))?;

        if response.status_code() >= 300 && response.status_code() != 404 {
            return Err(MediaError::Delete(format!(
                "S3 delete failed with status {}",
                response.status_code()
            )));
        }

        tracing::debug!(key = %key, "Deleted object from S3");
        Ok(())
    }

    /// Generate a presigned URL for downloading an object.
    pub async fn get_presigned_url(
        &self,
        key: &str,
        expiry_secs: u32,
    ) -> Result<String, MediaError> {
        let url = self
            .bucket
            .presign_get(key, expiry_secs, None)
            .await
            .map_err(|e| MediaError::PresignUrl(e.to_string()))?;

        Ok(url)
    }
}
