use crate::error::MediaError;

/// 25 MB for free-tier users.
const MAX_FILE_SIZE_FREE: usize = 25 * 1024 * 1024;

/// 50 MB for premium users.
const MAX_FILE_SIZE_PREMIUM: usize = 50 * 1024 * 1024;

/// Allowed image content types for avatars.
const AVATAR_CONTENT_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
];

/// Allowed content types for general attachments.
/// NOTE: SVG is intentionally excluded due to XSS vulnerability risk (embedded scripts).
const ATTACHMENT_CONTENT_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "video/mp4",
    "video/webm",
    "audio/mpeg",
    "audio/ogg",
    "audio/wav",
    "application/pdf",
    "text/plain",
    "application/zip",
    "application/x-tar",
    "application/gzip",
];

/// Maximum avatar file size: 8 MB.
const MAX_AVATAR_SIZE: usize = 8 * 1024 * 1024;

/// Account tier determines upload limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountTier {
    Free,
    Premium,
}

impl AccountTier {
    pub fn max_attachment_size(self) -> usize {
        match self {
            AccountTier::Free => MAX_FILE_SIZE_FREE,
            AccountTier::Premium => MAX_FILE_SIZE_PREMIUM,
        }
    }
}

/// File validation utilities.
pub struct FileValidation;

impl FileValidation {
    /// Validate an avatar upload (size + content type).
    pub fn validate_avatar(data: &[u8], content_type: &str) -> Result<(), MediaError> {
        if data.len() > MAX_AVATAR_SIZE {
            return Err(MediaError::FileTooLarge {
                size: data.len(),
                max: MAX_AVATAR_SIZE,
            });
        }

        if !AVATAR_CONTENT_TYPES.contains(&content_type) {
            return Err(MediaError::InvalidContentType(format!(
                "'{}' is not allowed for avatars. Allowed: {:?}",
                content_type, AVATAR_CONTENT_TYPES
            )));
        }

        Ok(())
    }

    /// Validate a general attachment upload (size + content type).
    pub fn validate_attachment(
        data: &[u8],
        content_type: &str,
        tier: AccountTier,
    ) -> Result<(), MediaError> {
        let max = tier.max_attachment_size();
        if data.len() > max {
            return Err(MediaError::FileTooLarge {
                size: data.len(),
                max,
            });
        }

        if !ATTACHMENT_CONTENT_TYPES.contains(&content_type) {
            return Err(MediaError::InvalidContentType(format!(
                "'{}' is not an allowed attachment type. Allowed: {:?}",
                content_type, ATTACHMENT_CONTENT_TYPES
            )));
        }

        Ok(())
    }

    /// Validate only the content type for an attachment (used for streaming uploads
    /// where the full data is not available upfront).
    pub fn validate_attachment_content_type(content_type: &str) -> Result<(), MediaError> {
        if !ATTACHMENT_CONTENT_TYPES.contains(&content_type) {
            return Err(MediaError::InvalidContentType(format!(
                "'{}' is not an allowed attachment type. Allowed: {:?}",
                content_type, ATTACHMENT_CONTENT_TYPES
            )));
        }
        Ok(())
    }

    /// Infer a file extension from a content type.
    pub fn extension_for_content_type(content_type: &str) -> Result<&'static str, MediaError> {
        match content_type {
            "image/png" => Ok("png"),
            "image/jpeg" => Ok("jpg"),
            "image/webp" => Ok("webp"),
            "image/gif" => Ok("gif"),
            "video/mp4" => Ok("mp4"),
            "video/webm" => Ok("webm"),
            "audio/mpeg" => Ok("mp3"),
            "audio/ogg" => Ok("ogg"),
            "audio/wav" => Ok("wav"),
            "application/pdf" => Ok("pdf"),
            "text/plain" => Ok("txt"),
            "application/zip" => Ok("zip"),
            "application/x-tar" => Ok("tar"),
            "application/gzip" => Ok("gz"),
            other => Err(MediaError::InvalidExtension(other.to_string())),
        }
    }
}

/// Validate that the first bytes of `data` match the claimed content type.
/// This prevents clients from uploading a file with a spoofed Content-Type header.
pub fn validate_content_magic_bytes(data: &[u8], claimed_content_type: &str) -> Result<(), MediaError> {
    let valid = match claimed_content_type {
        // PNG: 89 50 4E 47
        "image/png" => data.len() >= 4 && data[..4] == [0x89, 0x50, 0x4E, 0x47],
        // JPEG: FF D8 FF
        "image/jpeg" => data.len() >= 3 && data[..3] == [0xFF, 0xD8, 0xFF],
        // GIF: 47 49 46 ("GIF")
        "image/gif" => data.len() >= 3 && data[..3] == [0x47, 0x49, 0x46],
        // WebP: RIFF....WEBP
        "image/webp" => {
            data.len() >= 12
                && data[..4] == [0x52, 0x49, 0x46, 0x46]
                && data[8..12] == [0x57, 0x45, 0x42, 0x50]
        }
        // PDF: 25 50 44 46 ("%PDF")
        "application/pdf" => data.len() >= 4 && data[..4] == [0x25, 0x50, 0x44, 0x46],
        // For types without known magic bytes, skip validation.
        _ => true,
    };

    if !valid {
        return Err(MediaError::InvalidContentType(format!(
            "file content does not match claimed content type '{}'",
            claimed_content_type
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_avatar_valid() {
        let data = vec![0u8; 1024];
        assert!(FileValidation::validate_avatar(&data, "image/png").is_ok());
        assert!(FileValidation::validate_avatar(&data, "image/webp").is_ok());
    }

    #[test]
    fn test_validate_avatar_too_large() {
        let data = vec![0u8; MAX_AVATAR_SIZE + 1];
        let result = FileValidation::validate_avatar(&data, "image/png");
        assert!(matches!(result, Err(MediaError::FileTooLarge { .. })));
    }

    #[test]
    fn test_validate_avatar_bad_content_type() {
        let data = vec![0u8; 1024];
        let result = FileValidation::validate_avatar(&data, "application/pdf");
        assert!(matches!(result, Err(MediaError::InvalidContentType(_))));
    }

    #[test]
    fn test_validate_attachment_free_tier() {
        let data = vec![0u8; MAX_FILE_SIZE_FREE + 1];
        let result = FileValidation::validate_attachment(&data, "application/pdf", AccountTier::Free);
        assert!(matches!(result, Err(MediaError::FileTooLarge { .. })));
    }

    #[test]
    fn test_validate_attachment_premium_tier() {
        // Over free limit but under premium
        let data = vec![0u8; MAX_FILE_SIZE_FREE + 1];
        let result =
            FileValidation::validate_attachment(&data, "application/pdf", AccountTier::Premium);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extension_mapping() {
        assert_eq!(FileValidation::extension_for_content_type("image/png").unwrap(), "png");
        assert_eq!(FileValidation::extension_for_content_type("video/mp4").unwrap(), "mp4");
        assert!(FileValidation::extension_for_content_type("application/octet-stream").is_err());
    }
}
