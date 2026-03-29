# P2-07 — Media Streaming (Upload/Download)

**Status:** Not Started
**Dependencies:** None (independent)
**Crates:** lumiere-media, lumiere-server

## Goal

Replace in-memory file buffering with streaming. Currently a 50MB upload loads entirely into RAM — 100 concurrent uploads = 5GB memory.

## Tasks

### 7.1 — Streaming Upload

Replace `upload(&[u8])` with streaming:

```rust
pub async fn upload_stream(
    &self,
    key: &str,
    content_type: &str,
    content_length: u64,
    body: impl Stream<Item = Result<Bytes, std::io::Error>> + Send,
) -> Result<(), MediaError>
```

- Use `put_object_stream` from rust-s3
- Axum `BodyStream` extractor for multipart upload
- Validate content-type from multipart headers
- Magic byte check on first chunk only
- Size limit enforced by counting bytes as they stream

### 7.2 — Streaming Download

Replace `download() -> Vec<u8>` with streaming response:

```rust
pub async fn download_stream(
    &self,
    key: &str,
) -> Result<impl Stream<Item = Result<Bytes>>, MediaError>
```

- Return Axum `StreamBody` response
- Set Content-Type, Content-Length, Content-Disposition headers
- Support Range requests for video seeking (partial content 206)
- Cache headers: `Cache-Control: public, max-age=31536000, immutable`

### 7.3 — Multipart Upload Endpoint

New route for file upload:
```
POST /api/v1/channels/:channel_id/attachments
Content-Type: multipart/form-data

Part: file (binary)
Part: filename (text)
```

- Use `axum::extract::Multipart`
- Stream file part directly to MinIO
- Generate attachment metadata in PostgreSQL
- Return attachment object with presigned download URL

### 7.4 — Presigned URL Download (Preferred Path)

For most downloads, use presigned URLs instead of proxying:
- Client requests attachment → server returns presigned MinIO URL
- Client downloads directly from MinIO
- No server memory used for downloads
- Presigned URLs valid for 1 hour

Only proxy through server when:
- MinIO is not publicly accessible
- Access control needs per-request validation

### 7.5 — Avatar Upload Optimization

- Accept base64 in PATCH /users/@me (existing)
- OR accept multipart upload to new endpoint
- Resize on upload: 128, 256, 512, 1024px variants
- Store all sizes, serve by `?size=` query param
- WebP conversion for bandwidth savings

## Acceptance Criteria

- [ ] 50MB file upload uses <5MB server memory
- [ ] Download streams to client without full buffering
- [ ] Multipart upload endpoint works with standard HTTP clients
- [ ] Range requests work for video files
- [ ] Presigned URLs used for direct MinIO downloads
- [ ] Integration test: upload 10MB file → download → verify integrity
