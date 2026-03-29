#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("S3 connection error: {0}")]
    Connection(String),

    #[error("Upload failed: {0}")]
    Upload(String),

    #[error("Download failed: {0}")]
    Download(String),

    #[error("Delete failed: {0}")]
    Delete(String),

    #[error("Object not found: {0}")]
    NotFound(String),

    #[error("Presign URL failed: {0}")]
    PresignUrl(String),

    #[error("File too large: {size} bytes exceeds maximum {max} bytes")]
    FileTooLarge { size: usize, max: usize },

    #[error("Invalid content type: {0}")]
    InvalidContentType(String),

    #[error("Invalid file extension: {0}")]
    InvalidExtension(String),
}
