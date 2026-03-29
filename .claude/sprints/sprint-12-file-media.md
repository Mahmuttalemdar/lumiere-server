# Sprint 12 — File & Media System

**Status:** Not Started
**Dependencies:** Sprint 01
**Crates:** lumiere-media, lumiere-server

## Goal

Complete file and media management system: upload, download, image processing (resize, thumbnails), avatar/banner/icon uploads, content type validation, and CDN-like URL serving.

## Tasks

### 12.1 — MinIO Client Setup

```rust
// crates/lumiere-media/src/lib.rs
use s3::Bucket;

pub struct MediaService {
    bucket: Bucket,
    base_url: String,
}

impl MediaService {
    pub async fn new(config: &MinioConfig) -> Result<Self> { ... }
    pub async fn upload(&self, key: &str, data: &[u8], content_type: &str) -> Result<String> { ... }
    pub async fn download(&self, key: &str) -> Result<Vec<u8>> { ... }
    pub async fn delete(&self, key: &str) -> Result<()> { ... }
    pub async fn get_presigned_url(&self, key: &str, expiry: Duration) -> Result<String> { ... }
}
```

Bucket structure in MinIO:
```
lumiere/
├── avatars/{user_id}/{hash}.webp
├── banners/{user_id}/{hash}.webp
├── icons/{server_id}/{hash}.webp
├── emojis/{emoji_id}.webp
├── emojis/{emoji_id}.gif          (animated)
├── attachments/{channel_id}/{message_id}/{filename}
└── thumbnails/{channel_id}/{message_id}/{filename}
```

### 12.2 — Image Processing

Use `image` crate for processing:

```rust
pub struct ImageProcessor;

impl ImageProcessor {
    /// Resize avatar to standard sizes
    pub fn process_avatar(data: &[u8]) -> Result<ProcessedImage> {
        // Output: 128x128, 256x256, 512x512, 1024x1024
        // Format: WebP (best compression/quality ratio)
        // Max input: 8 MB
    }

    /// Resize banner
    pub fn process_banner(data: &[u8]) -> Result<ProcessedImage> {
        // Output: 960x540, 1920x1080
        // Format: WebP
        // Max input: 10 MB
    }

    /// Generate thumbnail for attachment
    pub fn generate_thumbnail(data: &[u8], max_width: u32, max_height: u32) -> Result<ProcessedImage> {
        // Maintain aspect ratio
        // Default max: 400x300
        // Format: WebP
    }

    /// Extract dimensions from image
    pub fn get_dimensions(data: &[u8]) -> Result<(u32, u32)> { ... }
}

pub struct ProcessedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub size: u64,
}
```

### 12.3 — Avatar Upload

```
PATCH /api/v1/users/@me
    Body: { avatar: "data:image/png;base64,..." }
    or
    Body: { avatar: null }  (remove avatar)

Processing:
    1. Decode base64
    2. Validate image format (PNG, JPEG, GIF, WebP)
    3. Validate size (< 8 MB)
    4. Process to standard sizes (128, 256, 512, 1024)
    5. Convert to WebP
    6. Upload all sizes to MinIO: avatars/{user_id}/{hash}.webp
    7. If animated GIF: also store original as .gif
    8. Hash = first 16 chars of SHA256 of original image
    9. Delete old avatar files
    10. Update user.avatar = hash
```

Avatar URL format:
```
GET /api/v1/avatars/{user_id}/{hash}.webp?size=256
```

If no avatar, generate default based on user ID:
```
GET /api/v1/avatars/{user_id}/default.webp?index={user_id % 5}
```

### 12.4 — Server Icon/Banner Upload

Same flow as avatar but for servers:

```
PATCH /api/v1/servers/:server_id
    Body: { icon: "data:image/png;base64,..." }
    Auth: MANAGE_SERVER
```

### 12.5 — File Upload for Attachments

```
POST /api/v1/channels/:channel_id/attachments
    Content-Type: multipart/form-data
    Fields:
        - file: binary data
        - filename: original filename
        - description: alt text (optional)

    Response: {
        id: snowflake,
        upload_filename: "sanitized_name.ext",
        url: "/attachments/channel_id/attachment_id/filename",
        size: 12345,
        content_type: "image/png",
        width: 1920,
        height: 1080,
    }
```

File validation:
```rust
const ALLOWED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
const ALLOWED_VIDEO_TYPES: &[&str] = &["video/mp4", "video/webm"];
const ALLOWED_AUDIO_TYPES: &[&str] = &["audio/mpeg", "audio/ogg", "audio/wav"];
const BLOCKED_EXTENSIONS: &[&str] = &[".exe", ".bat", ".cmd", ".sh", ".ps1", ".msi", ".dll"];

const MAX_FILE_SIZE: u64 = 25 * 1024 * 1024;       // 25 MB
const MAX_FILE_SIZE_PREMIUM: u64 = 50 * 1024 * 1024; // 50 MB
```

### 12.6 — File Serving / CDN Proxy

```
GET /api/v1/attachments/:channel_id/:attachment_id/:filename
    Response: file binary with correct Content-Type and Content-Disposition
    Headers:
        - Content-Type: (from stored metadata)
        - Content-Disposition: inline (images/videos) or attachment (others)
        - Cache-Control: public, max-age=31536000, immutable
        - ETag: (content hash)
```

For images, support query params:
```
?size=256          → resize to max 256px width
?format=webp       → convert to webp
?quality=80        → compression quality
```

### 12.7 — Emoji Upload

```
POST /api/v1/servers/:server_id/emojis
    Body: { name, image: "data:image/png;base64,..." }
    Processing:
        - Validate: PNG or GIF, max 256 KB, 128x128 recommended
        - If GIF: check if animated, store both static frame and animated version
        - Convert to WebP for static
        - Upload to MinIO: emojis/{emoji_id}.webp (and .gif if animated)
```

### 12.8 — Cleanup Job

Background task to clean orphaned files:
- Attachments uploaded but never referenced in a message (after 24h)
- Old avatar/banner versions after user changes them
- Deleted message attachments (after 30 days)

```rust
// JetStream consumer or cron job
async fn cleanup_orphaned_files() -> Result<()> {
    // Query for unreferenced attachment IDs
    // Delete from MinIO
}
```

## Acceptance Criteria

- [ ] Avatar upload processes to multiple sizes and stores as WebP
- [ ] Avatar URL returns correct image at requested size
- [ ] Default avatar generated for users without custom avatar
- [ ] Server icon/banner upload works
- [ ] File attachment upload to MinIO with size validation
- [ ] Blocked file types rejected
- [ ] Image attachments get thumbnail generation
- [ ] Image dimensions extracted and stored
- [ ] File serving with correct Content-Type and caching headers
- [ ] Emoji upload (static PNG + animated GIF)
- [ ] Orphaned file cleanup removes unreferenced attachments
- [ ] Integration test: upload image → verify MinIO storage → download → verify content
