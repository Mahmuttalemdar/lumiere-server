use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_media::{validate_content_magic_bytes, AccountTier, FileValidation};
use lumiere_models::{error::AppError, snowflake::Snowflake};
use lumiere_permissions::Permissions;
use serde::Serialize;
use std::sync::Arc;

use super::servers::require_permissions;
use crate::AppState;

/// Router for attachment upload (nested under /api/v1/channels).
pub fn upload_router() -> Router<Arc<AppState>> {
    Router::new().route("/{channel_id}/attachments", post(upload_attachment))
}

/// Router for attachment download (nested under /api/v1/attachments).
pub fn download_router() -> Router<Arc<AppState>> {
    Router::new().route("/{attachment_id}", get(download_attachment))
}

// ─── Response types ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AttachmentResponse {
    pub id: Snowflake,
    pub filename: String,
    pub content_type: String,
    pub size: i64,
    pub url: String,
}

// ─── Upload ─────────────────────────────────────────────────────

/// POST /api/v1/channels/:channel_id/attachments
///
/// Accepts multipart/form-data with a single file field.
/// Validates content type, magic bytes, and file size.
/// Stores the file in MinIO and records metadata in PostgreSQL.
/// Returns attachment metadata with a presigned download URL.
async fn upload_attachment(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<Snowflake>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    // Look up the channel to find its server_id for permission checks
    let row: Option<(i64,)> = sqlx::query_as("SELECT server_id FROM channels WHERE id = $1")
        .bind(channel_id)
        .fetch_optional(&state.db.pg)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let server_id = row
        .ok_or_else(|| AppError::NotFound("Channel not found".into()))?
        .0;

    // Require ATTACH_FILES permission
    require_permissions(&state, server_id, auth.id, Permissions::ATTACH_FILES).await?;

    // Determine the user's account tier for size limits
    let tier_row: Option<(i16,)> = sqlx::query_as("SELECT premium_type FROM users WHERE id = $1")
        .bind(auth.id)
        .fetch_optional(&state.db.pg)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let tier = match tier_row.map(|r| r.0).unwrap_or(0) {
        0 => AccountTier::Free,
        _ => AccountTier::Premium,
    };

    // Extract the file field from the multipart form
    let field = match multipart.next_field().await {
        Ok(Some(f)) => f,
        Ok(None) => {
            return Err(AppError::BadRequest(
                "No file field in multipart body".into(),
            ))
        }
        Err(e) => return Err(AppError::BadRequest(format!("Invalid multipart: {}", e))),
    };

    let filename = field.file_name().unwrap_or("file").to_string();

    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();

    // Validate content type is allowed
    FileValidation::validate_attachment_content_type(&content_type)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // Read the file data (we need it in memory for magic byte validation).
    // For very large files, the streaming path in MediaService can be used,
    // but magic byte validation requires at least the first chunk.
    let data = field
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to read file: {}", e)))?;

    // Reject empty files
    if data.is_empty() {
        return Err(AppError::BadRequest("File is empty".into()));
    }

    // Validate magic bytes match the claimed content type
    validate_content_magic_bytes(&data, &content_type)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // Generate a Snowflake ID for the attachment
    let attachment_id = state.snowflake.next_id();

    // Upload to MinIO via the media service
    let key = state
        .media
        .upload_attachment(
            channel_id,
            attachment_id,
            &filename,
            &data,
            &content_type,
            tier,
        )
        .await
        .map_err(|e| match e {
            lumiere_media::MediaError::FileTooLarge { size, max } => AppError::BadRequest(format!(
                "File too large: {} bytes exceeds maximum {} bytes",
                size, max
            )),
            lumiere_media::MediaError::InvalidContentType(msg) => AppError::BadRequest(msg),
            other => AppError::Internal(anyhow::anyhow!(other)),
        })?;

    let file_size = data.len() as i64;

    // Record the attachment in PostgreSQL
    sqlx::query(
        "INSERT INTO attachments (id, channel_id, message_id, filename, content_type, size, s3_key, uploaded_by) \
         VALUES ($1, $2, NULL, $3, $4, $5, $6, $7)",
    )
    .bind(attachment_id)
    .bind(channel_id)
    .bind(&filename)
    .bind(&content_type)
    .bind(file_size)
    .bind(&key)
    .bind(auth.id)
    .execute(&state.db.pg)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    // Generate a presigned download URL (1 hour expiry)
    let url = state
        .media
        .get_presigned_url(&key, 3600)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let response = AttachmentResponse {
        id: attachment_id,
        filename,
        content_type,
        size: file_size,
        url,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

// ─── Download ───────────────────────────────────────────────────

/// GET /api/v1/attachments/:attachment_id
///
/// Looks up the attachment in PostgreSQL and redirects the client
/// to a presigned MinIO URL. Sets aggressive cache headers since
/// attachment content is immutable (identified by Snowflake ID).
async fn download_attachment(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(attachment_id): Path<Snowflake>,
) -> Result<Response, AppError> {
    // Look up the attachment record
    let row: Option<(i64, String, String, i64, String)> = sqlx::query_as(
        "SELECT channel_id, filename, content_type, size, s3_key FROM attachments WHERE id = $1",
    )
    .bind(attachment_id)
    .fetch_optional(&state.db.pg)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    let (channel_id, filename, content_type, _size, s3_key) =
        row.ok_or_else(|| AppError::NotFound("Attachment not found".into()))?;

    // Check the user has access to the channel's server
    let server_row: Option<(i64,)> = sqlx::query_as("SELECT server_id FROM channels WHERE id = $1")
        .bind(channel_id)
        .fetch_optional(&state.db.pg)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    if let Some((server_id,)) = server_row {
        require_permissions(&state, server_id, auth.id, Permissions::VIEW_CHANNEL).await?;
    } else {
        // DM channel — verify user is a recipient
        let is_recipient = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM dm_recipients WHERE channel_id = $1 AND user_id = $2)",
        )
        .bind(channel_id)
        .bind(auth.id)
        .fetch_one(&state.db.pg)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

        if !is_recipient {
            return Err(AppError::Forbidden("Access denied".into()));
        }
    }

    // Generate a presigned URL (1 hour expiry)
    let url = state
        .media
        .get_download_url(&s3_key, 3600)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // Sanitize filename for Content-Disposition header to prevent header injection
    let safe_filename = filename.replace('"', "'").replace(['\n', '\r'], "");

    // Determine Content-Disposition: inline for safe types, attachment for others
    let disposition = if is_inline_safe(&content_type) {
        format!("inline; filename=\"{}\"", safe_filename)
    } else {
        format!("attachment; filename=\"{}\"", safe_filename)
    };

    // Redirect to the presigned URL with cache headers.
    // max-age matches the presigned URL expiry (1 hour) since the redirect
    // target (presigned URL) expires — caching the redirect longer is wrong.
    let response = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, &url)
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::empty())
        .unwrap();

    Ok(response)
}

/// Content types that are safe to display inline in the browser.
fn is_inline_safe(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png"
            | "image/jpeg"
            | "image/webp"
            | "image/gif"
            | "video/mp4"
            | "video/webm"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/wav"
            | "text/plain"
    )
}
