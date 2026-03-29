use crate::error::MediaError;
use crate::validation::{AccountTier, FileValidation};
use crate::S3Client;
use bytes::Bytes;
use futures::stream::Stream;
use lumiere_models::snowflake::Snowflake;
use sha2::{Digest, Sha256};
use std::pin::Pin;
use tokio::io::AsyncRead;

/// High-level media operations built on top of S3Client.
#[derive(Clone)]
pub struct MediaService {
    s3: S3Client,
}

impl MediaService {
    pub fn new(s3: S3Client) -> Self {
        Self { s3 }
    }

    /// Return a reference to the underlying S3 client.
    pub fn s3(&self) -> &S3Client {
        &self.s3
    }

    /// Upload a user or server avatar.
    ///
    /// Object key: `avatars/{owner_id}/{hash}.{ext}`
    ///
    /// Returns the object key on success.
    pub async fn upload_avatar(
        &self,
        owner_id: Snowflake,
        data: &[u8],
        content_type: &str,
    ) -> Result<String, MediaError> {
        FileValidation::validate_avatar(data, content_type)?;

        let ext = FileValidation::extension_for_content_type(content_type)?;
        let hash = content_hash(data);
        let key = format!("avatars/{}/{}.{}", owner_id, hash, ext);

        self.s3.upload(&key, data, content_type).await?;

        tracing::info!(
            owner_id = %owner_id,
            key = %key,
            size = data.len(),
            "Avatar uploaded"
        );

        Ok(key)
    }

    /// Upload a message attachment.
    ///
    /// Object key: `attachments/{channel_id}/{attachment_id}/{filename}`
    ///
    /// The filename is sanitized to prevent path traversal.
    /// Returns the object key on success.
    pub async fn upload_attachment(
        &self,
        channel_id: Snowflake,
        attachment_id: Snowflake,
        filename: &str,
        data: &[u8],
        content_type: &str,
        tier: AccountTier,
    ) -> Result<String, MediaError> {
        FileValidation::validate_attachment(data, content_type, tier)?;

        let safe_filename = sanitize_filename(filename);
        let key = format!(
            "attachments/{}/{}/{}",
            channel_id, attachment_id, safe_filename
        );

        self.s3.upload(&key, data, content_type).await?;

        tracing::info!(
            channel_id = %channel_id,
            attachment_id = %attachment_id,
            key = %key,
            size = data.len(),
            "Attachment uploaded"
        );

        Ok(key)
    }

    /// Upload a message attachment from an async byte stream.
    ///
    /// This avoids buffering the entire file in memory — data flows from the
    /// client's multipart upload directly into MinIO/S3.
    ///
    /// **Validation**: Content type is checked against the allowlist. Magic byte
    /// validation cannot be performed on streams (the first chunk would need to
    /// be inspected), so callers should validate magic bytes on the first chunk
    /// before calling this method when security is critical.
    ///
    /// Returns `(object_key, uploaded_bytes)`.
    pub async fn upload_attachment_stream<R>(
        &self,
        channel_id: Snowflake,
        attachment_id: Snowflake,
        filename: &str,
        content_type: &str,
        content_length: u64,
        tier: AccountTier,
        reader: &mut R,
    ) -> Result<(String, u64), MediaError>
    where
        R: AsyncRead + Unpin,
    {
        // Validate content type is allowed
        FileValidation::validate_attachment_content_type(content_type)?;

        // Validate content length against tier limit
        let max = tier.max_attachment_size();
        if content_length > max as u64 {
            return Err(MediaError::FileTooLarge {
                size: content_length as usize,
                max,
            });
        }

        let safe_filename = sanitize_filename(filename);
        let key = format!(
            "attachments/{}/{}/{}",
            channel_id, attachment_id, safe_filename
        );

        let uploaded = self.s3.upload_stream(&key, content_type, reader).await?;

        tracing::info!(
            channel_id = %channel_id,
            attachment_id = %attachment_id,
            key = %key,
            size = uploaded,
            "Attachment streamed to S3"
        );

        Ok((key, uploaded))
    }

    /// Delete a file by its object key.
    pub async fn delete_file(&self, key: &str) -> Result<(), MediaError> {
        self.s3.delete(key).await?;
        tracing::info!(key = %key, "File deleted");
        Ok(())
    }

    /// Download a file by its object key.
    /// The key must start with an allowed prefix (avatars/, attachments/, icons/, emojis/).
    pub async fn download_file(&self, key: &str) -> Result<Vec<u8>, MediaError> {
        validate_key_prefix(key)?;
        self.s3.download(key).await
    }

    /// Download a file as a byte stream (for proxied downloads).
    ///
    /// The key must start with an allowed prefix. Returns a stream of `Bytes`
    /// chunks that can be forwarded directly to an HTTP response body.
    pub async fn download_file_stream(
        &self,
        key: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>, MediaError>
    {
        validate_key_prefix(key)?;
        let (_status, stream) = self.s3.download_stream(key).await?;
        Ok(stream)
    }

    /// Generate a presigned download URL.
    /// The key must start with an allowed prefix. Expiry is clamped to 60..604800 seconds.
    pub async fn get_presigned_url(
        &self,
        key: &str,
        expiry_secs: u32,
    ) -> Result<String, MediaError> {
        validate_key_prefix(key)?;
        let clamped_expiry = expiry_secs.clamp(60, 604800);
        self.s3.get_presigned_url(key, clamped_expiry).await
    }

    /// Get a presigned URL for direct client download (preferred over proxying).
    ///
    /// This avoids proxying through the server entirely. Default expiry: 1 hour.
    pub async fn get_download_url(
        &self,
        key: &str,
        expiry_secs: u32,
    ) -> Result<String, MediaError> {
        validate_key_prefix(key)?;
        let clamped = expiry_secs.clamp(60, 604800);
        self.s3.get_download_url(key, clamped).await
    }
}

/// Produce a truncated SHA-256 hex hash of the file content (first 16 hex chars).
fn content_hash(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    hex::encode(&digest[..8])
}

/// Allowed key prefixes for presigned URL and download operations.
const ALLOWED_KEY_PREFIXES: &[&str] = &["avatars/", "attachments/", "icons/", "emojis/"];

/// Validate that an object key starts with an expected prefix to prevent path traversal.
fn validate_key_prefix(key: &str) -> Result<(), MediaError> {
    if ALLOWED_KEY_PREFIXES.iter().any(|prefix| key.starts_with(prefix)) {
        Ok(())
    } else {
        Err(MediaError::InvalidContentType(format!(
            "object key '{}' does not start with an allowed prefix ({:?})",
            key, ALLOWED_KEY_PREFIXES
        )))
    }
}

/// Strip path components and dangerous characters from a filename.
fn sanitize_filename(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file");

    let sanitized: String = base
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();

    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "file".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_hash_deterministic() {
        let data = b"hello world";
        let h1 = content_hash(data);
        let h2 = content_hash(data);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16); // 8 bytes = 16 hex chars
    }

    #[test]
    fn test_sanitize_filename_strips_path() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\Users\\file.txt"), "file.txt");
        assert_eq!(sanitize_filename("normal.png"), "normal.png");
    }

    #[test]
    fn test_sanitize_filename_replaces_dangerous_chars() {
        assert_eq!(sanitize_filename("file<name>.txt"), "file_name_.txt");
        assert_eq!(sanitize_filename("a:b*c?d"), "a_b_c_d");
    }

    #[test]
    fn test_sanitize_filename_empty() {
        assert_eq!(sanitize_filename(""), "file");
    }
}
