mod error;
mod service;
mod validation;

pub use error::MediaError;
pub use service::MediaService;
pub use validation::{validate_content_magic_bytes, AccountTier, FileValidation};

use bytes::Bytes;
use futures::stream::Stream;
use lumiere_models::config::MinioConfig;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::Region;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

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

    /// Upload from an async byte stream (does not buffer the entire file in memory).
    ///
    /// Uses rust-s3's `put_object_stream_with_content_type` which reads from
    /// the `AsyncRead` in chunks and streams them to S3/MinIO.
    pub async fn upload_stream<R>(
        &self,
        key: &str,
        content_type: &str,
        reader: &mut R,
    ) -> Result<u64, MediaError>
    where
        R: AsyncRead + Unpin,
    {
        let response = self
            .bucket
            .put_object_stream_with_content_type(reader, key, content_type)
            .await
            .map_err(|e| MediaError::Upload(e.to_string()))?;

        if response.status_code() >= 300 {
            return Err(MediaError::Upload(format!(
                "S3 streaming upload failed with status {}",
                response.status_code()
            )));
        }

        let uploaded = response.uploaded_bytes() as u64;
        tracing::debug!(key = %key, size = uploaded, "Streamed object to S3");
        Ok(uploaded)
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

    /// Download an object as a byte stream (does not buffer in memory).
    ///
    /// Returns the HTTP status code and a `Stream<Item = Result<Bytes, ...>>`
    /// suitable for proxied streaming responses to clients.
    pub async fn download_stream(
        &self,
        key: &str,
    ) -> Result<
        (
            u16,
            Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>,
        ),
        MediaError,
    > {
        let response = self
            .bucket
            .get_object_stream(key)
            .await
            .map_err(|e| MediaError::Download(e.to_string()))?;

        if response.status_code == 404 {
            return Err(MediaError::NotFound(key.to_string()));
        }

        if response.status_code >= 300 {
            return Err(MediaError::Download(format!(
                "S3 streaming download failed with status {}",
                response.status_code
            )));
        }

        // Map the S3 stream items to std::io::Error for compatibility with axum's Body
        let mapped = futures::StreamExt::map(response.bytes, |item| {
            item.map_err(|e| std::io::Error::other(e.to_string()))
        });

        Ok((response.status_code, Box::pin(mapped)))
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

    /// Get a presigned URL for direct client download (preferred over proxying).
    ///
    /// This avoids proxying through the server entirely — the client fetches
    /// directly from MinIO/S3.
    pub async fn get_download_url(
        &self,
        key: &str,
        expiry_secs: u32,
    ) -> Result<String, MediaError> {
        self.get_presigned_url(key, expiry_secs).await
    }
}
