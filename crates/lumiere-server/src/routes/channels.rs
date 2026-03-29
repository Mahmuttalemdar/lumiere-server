use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_models::{
    error::AppError,
    snowflake::Snowflake,
};
use lumiere_permissions::Permissions;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::Validate;

use crate::AppState;
use super::servers::{require_member, require_permissions};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{channel_id}", get(get_channel))
        .route("/{channel_id}", patch(update_channel))
        .route("/{channel_id}", delete(delete_channel))
        .route("/{channel_id}/followers", post(follow_channel))
}


// ─── Response types ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ChannelResponse {
    pub id: Snowflake,
    pub server_id: Option<Snowflake>,
    pub parent_id: Option<Snowflake>,
    #[serde(rename = "type")]
    pub channel_type: i16,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub position: i32,
    pub bitrate: Option<i32>,
    pub user_limit: Option<i32>,
    pub rate_limit: i32,
    pub nsfw: bool,
    pub last_message_id: Option<Snowflake>,
    pub icon: Option<String>,
    pub e2ee_enabled: bool,
    pub permission_overrides: Vec<PermissionOverrideResponse>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct PermissionOverrideResponse {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub target_type: i16,
    pub allow: i64,
    pub deny: i64,
}

// ─── Helpers ────────────────────────────────────────────────────

async fn fetch_channel(state: &AppState, channel_id: i64) -> Result<ChannelResponse, AppError> {
    let row = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, i16, Option<String>, Option<String>, i32, Option<i32>, Option<i32>, i32, bool, Option<i64>, Option<String>, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, server_id, parent_id, type, name, topic, position, bitrate, user_limit, \
                rate_limit, nsfw, last_message_id, icon, e2ee_enabled, created_at \
         FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Channel not found".into()))?;

    let overrides = sqlx::query_as::<_, (i64, i16, i64, i64)>(
        "SELECT target_id, target_type, allow_bits, deny_bits \
         FROM permission_overrides WHERE channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(&state.db.pg)
    .await?;

    Ok(ChannelResponse {
        id: Snowflake::from(row.0),
        server_id: row.1.map(Snowflake::from),
        parent_id: row.2.map(Snowflake::from),
        channel_type: row.3,
        name: row.4,
        topic: row.5,
        position: row.6,
        bitrate: row.7,
        user_limit: row.8,
        rate_limit: row.9,
        nsfw: row.10,
        last_message_id: row.11.map(Snowflake::from),
        icon: row.12,
        e2ee_enabled: row.13,
        permission_overrides: overrides
            .into_iter()
            .map(|o| PermissionOverrideResponse {
                id: Snowflake::from(o.0),
                target_type: o.1,
                allow: o.2,
                deny: o.3,
            })
            .collect(),
        created_at: row.14,
    })
}

fn normalize_channel_name(name: &str, channel_type: i16) -> String {
    let trimmed = name.trim();
    // Text channels: lowercase, replace spaces with hyphens
    if channel_type == 0 || channel_type == 5 {
        let normalized: String = trimmed
            .to_lowercase()
            .replace(' ', "-")
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if normalized.is_empty() {
            "general".to_string()
        } else {
            normalized
        }
    } else {
        trimmed.to_string()
    }
}

// ─── Handlers ───────────────────────────────────────────────────

pub async fn get_server_channels(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_member(&state, server_id, auth.id).await?;

    let rows = sqlx::query_as::<_, (i64,)>(
        "SELECT id FROM channels WHERE server_id = $1 ORDER BY position ASC, id ASC",
    )
    .bind(server_id)
    .fetch_all(&state.db.pg)
    .await?;

    let mut channels = Vec::with_capacity(rows.len());
    for row in rows {
        channels.push(fetch_channel(&state, row.0).await?);
    }

    Ok(Json(channels))
}

async fn get_channel(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let channel = fetch_channel(&state, channel_id).await?;

    // For server channels, check membership; for DMs, check recipient
    if let Some(server_id) = channel.server_id {
        require_member(&state, server_id.value() as i64, auth.id).await?;
    } else {
        // DM channel — check recipient
        let is_recipient = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM dm_recipients WHERE channel_id = $1 AND user_id = $2)",
        )
        .bind(channel_id)
        .bind(auth.id)
        .fetch_one(&state.db.pg)
        .await?;
        if !is_recipient {
            return Err(AppError::Forbidden("Not a recipient of this channel".into()));
        }
    }

    Ok(Json(channel))
}

#[derive(Debug, Deserialize, Validate)]
pub struct CreateChannelRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,
    #[serde(rename = "type")]
    pub channel_type: Option<i16>,
    pub parent_id: Option<i64>,
    #[validate(length(max = 1024))]
    pub topic: Option<String>,
    pub position: Option<i32>,
    pub bitrate: Option<i32>,
    pub user_limit: Option<i32>,
    pub rate_limit: Option<i32>,
    pub nsfw: Option<bool>,
    pub permission_overrides: Option<Vec<PermissionOverrideInput>>,
}

#[derive(Debug, Deserialize)]
pub struct PermissionOverrideInput {
    pub id: i64,
    #[serde(rename = "type")]
    pub target_type: i16,
    pub allow: i64,
    pub deny: i64,
}

pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Json(body): Json<CreateChannelRequest>,
) -> Result<impl IntoResponse, AppError> {
    if let Err(errors) = body.validate() {
        return Err(AppError::Validation(crate::routes::validation_errors(errors)));
    }

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_CHANNELS).await?;

    let channel_type = body.channel_type.unwrap_or(0);

    // Validate channel type
    if ![0, 2, 4, 5].contains(&channel_type) {
        return Err(AppError::BadRequest("Invalid channel type for server".into()));
    }

    // Max 500 channels per server
    let channel_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM channels WHERE server_id = $1",
    )
    .bind(server_id)
    .fetch_one(&state.db.pg)
    .await?;

    if channel_count >= 500 {
        return Err(AppError::BadRequest("Server has reached max channel limit (500)".into()));
    }

    // Validate parent_id
    if let Some(parent_id) = body.parent_id {
        let parent_type = sqlx::query_scalar::<_, i16>(
            "SELECT type FROM channels WHERE id = $1 AND server_id = $2",
        )
        .bind(parent_id)
        .bind(server_id)
        .fetch_optional(&state.db.pg)
        .await?
        .ok_or_else(|| AppError::BadRequest("Parent channel not found".into()))?;

        if parent_type != 4 {
            return Err(AppError::BadRequest("Parent must be a category channel".into()));
        }

        // Max 50 channels per category
        let category_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM channels WHERE parent_id = $1",
        )
        .bind(parent_id)
        .fetch_one(&state.db.pg)
        .await?;

        if category_count >= 50 {
            return Err(AppError::BadRequest("Category has reached max channel limit (50)".into()));
        }
    }

    // Validate voice-specific fields
    if channel_type == 2 {
        if let Some(bitrate) = body.bitrate {
            if !(8000..=384000).contains(&bitrate) {
                return Err(AppError::BadRequest("Bitrate must be 8000-384000".into()));
            }
        }
    }

    if let Some(rate_limit) = body.rate_limit {
        if !(0..=21600).contains(&rate_limit) {
            return Err(AppError::BadRequest("Rate limit must be 0-21600 seconds".into()));
        }
    }

    let channel_name = normalize_channel_name(&body.name, channel_type);
    let channel_id = state.snowflake.next_id();
    let position = body.position.unwrap_or(0);

    sqlx::query(
        "INSERT INTO channels (id, server_id, parent_id, type, name, topic, position, bitrate, user_limit, rate_limit, nsfw) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(channel_id)
    .bind(server_id)
    .bind(body.parent_id)
    .bind(channel_type)
    .bind(&channel_name)
    .bind(&body.topic)
    .bind(position)
    .bind(body.bitrate)
    .bind(body.user_limit)
    .bind(body.rate_limit.unwrap_or(0))
    .bind(body.nsfw.unwrap_or(false))
    .execute(&state.db.pg)
    .await?;

    // Create permission overrides
    if let Some(ref overrides) = body.permission_overrides {
        for ov in overrides {
            // Validate target_type: 0 = role, 1 = member
            if ov.target_type != 0 && ov.target_type != 1 {
                return Err(AppError::BadRequest("Override target_type must be 0 (role) or 1 (member)".into()));
            }
            // Strip ADMINISTRATOR bit from allow/deny
            let allow = ov.allow & !1;
            let deny = ov.deny & !1;
            sqlx::query(
                "INSERT INTO permission_overrides (channel_id, target_id, target_type, allow_bits, deny_bits) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(channel_id)
            .bind(ov.id)
            .bind(ov.target_type)
            .bind(allow)
            .bind(deny)
            .execute(&state.db.pg)
            .await?;
        }
    }

    // Dispatch CHANNEL_CREATE
    let event = serde_json::json!({
        "type": "CHANNEL_CREATE",
        "server_id": server_id,
        "channel_id": channel_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    let channel = fetch_channel(&state, channel_id.value() as i64).await?;
    Ok((StatusCode::CREATED, Json(channel)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateChannelRequest {
    pub name: Option<String>,
    pub topic: Option<Option<String>>,
    pub position: Option<i32>,
    pub parent_id: Option<Option<i64>>,
    pub bitrate: Option<i32>,
    pub user_limit: Option<i32>,
    pub rate_limit: Option<i32>,
    pub nsfw: Option<bool>,
    pub permission_overrides: Option<Vec<PermissionOverrideInput>>,
}

async fn update_channel(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<UpdateChannelRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Get channel to find server_id
    let (server_id, channel_type) = sqlx::query_as::<_, (Option<i64>, i16)>(
        "SELECT server_id, type FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Channel not found".into()))?;

    let server_id = server_id.ok_or_else(|| AppError::BadRequest("Cannot update DM channels this way".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_CHANNELS).await?;

    // Validate update fields
    if let Some(ref name) = body.name {
        if name.trim().is_empty() || name.len() > 100 {
            return Err(AppError::BadRequest("Channel name must be 1-100 characters".into()));
        }
    }
    if let Some(ref topic) = body.topic {
        if let Some(ref t) = topic {
            if t.len() > 1024 {
                return Err(AppError::BadRequest("Topic must be 1024 characters or less".into()));
            }
        }
    }
    if let Some(bitrate) = body.bitrate {
        if !(8000..=384000).contains(&bitrate) {
            return Err(AppError::BadRequest("Bitrate must be 8000-384000".into()));
        }
    }
    if let Some(rate_limit) = body.rate_limit {
        if !(0..=21600).contains(&rate_limit) {
            return Err(AppError::BadRequest("Rate limit must be 0-21600".into()));
        }
    }

    // Build dynamic update
    let mut sets = Vec::new();
    let mut param_idx = 2u32;

    macro_rules! add_set {
        ($field:ident, $col:expr) => {
            if body.$field.is_some() {
                sets.push(format!("{} = ${}", $col, param_idx));
                #[allow(unused_assignments)]
                { param_idx += 1; }
            }
        };
    }

    // Handle name normalization
    let normalized_name = body.name.as_ref().map(|n| normalize_channel_name(n, channel_type));
    if normalized_name.is_some() {
        sets.push(format!("name = ${param_idx}"));
        param_idx += 1;
    }
    add_set!(topic, "topic");
    add_set!(position, "position");
    add_set!(parent_id, "parent_id");
    add_set!(bitrate, "bitrate");
    add_set!(user_limit, "user_limit");
    add_set!(rate_limit, "rate_limit");
    add_set!(nsfw, "nsfw");

    if !sets.is_empty() {
        let sql = format!("UPDATE channels SET {} WHERE id = $1", sets.join(", "));
        let mut query = sqlx::query(&sql).bind(channel_id);

        if let Some(ref name) = normalized_name { query = query.bind(name.as_str()); }
        if let Some(ref v) = body.topic { query = query.bind(v.as_deref()); }
        if let Some(v) = body.position { query = query.bind(v); }
        if let Some(ref v) = body.parent_id { query = query.bind(*v); }
        if let Some(v) = body.bitrate { query = query.bind(v); }
        if let Some(v) = body.user_limit { query = query.bind(v); }
        if let Some(v) = body.rate_limit { query = query.bind(v); }
        if let Some(v) = body.nsfw { query = query.bind(v); }

        query.execute(&state.db.pg).await?;
    }

    // Replace permission overrides if provided (in a transaction)
    if let Some(ref overrides) = body.permission_overrides {
        // Validate all overrides first
        for ov in overrides {
            if ov.target_type != 0 && ov.target_type != 1 {
                return Err(AppError::BadRequest("Override target_type must be 0 (role) or 1 (member)".into()));
            }
        }

        let mut tx = state.db.pg.begin().await?;

        sqlx::query("DELETE FROM permission_overrides WHERE channel_id = $1")
            .bind(channel_id)
            .execute(&mut *tx)
            .await?;

        for ov in overrides {
            // Strip ADMINISTRATOR bit from allow/deny
            let allow = ov.allow & !1;
            let deny = ov.deny & !1;
            sqlx::query(
                "INSERT INTO permission_overrides (channel_id, target_id, target_type, allow_bits, deny_bits) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(channel_id)
            .bind(ov.id)
            .bind(ov.target_type)
            .bind(allow)
            .bind(deny)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
    }

    // Dispatch CHANNEL_UPDATE
    let event = serde_json::json!({
        "type": "CHANNEL_UPDATE",
        "server_id": server_id,
        "channel_id": channel_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    let channel = fetch_channel(&state, channel_id).await?;
    Ok(Json(channel))
}

async fn delete_channel(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let (server_id, channel_type) = sqlx::query_as::<_, (Option<i64>, i16)>(
        "SELECT server_id, type FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Channel not found".into()))?;

    let server_id = server_id.ok_or_else(|| AppError::BadRequest("Cannot delete DM channels this way".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_CHANNELS).await?;

    // Cannot delete last text channel
    if channel_type == 0 {
        let text_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM channels WHERE server_id = $1 AND type = 0",
        )
        .bind(server_id)
        .fetch_one(&state.db.pg)
        .await?;

        if text_count <= 1 {
            return Err(AppError::BadRequest(
                "Cannot delete the last text channel in a server".into(),
            ));
        }
    }

    // If deleting a category, move children to no-category
    if channel_type == 4 {
        sqlx::query("UPDATE channels SET parent_id = NULL WHERE parent_id = $1")
            .bind(channel_id)
            .execute(&state.db.pg)
            .await?;
    }

    sqlx::query("DELETE FROM channels WHERE id = $1")
        .bind(channel_id)
        .execute(&state.db.pg)
        .await?;

    // Dispatch CHANNEL_DELETE
    let event = serde_json::json!({
        "type": "CHANNEL_DELETE",
        "server_id": server_id,
        "channel_id": channel_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Channel Ordering ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChannelPositionUpdate {
    pub id: i64,
    pub position: i32,
    pub parent_id: Option<Option<i64>>,
}

pub async fn reorder_channels(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Json(body): Json<Vec<ChannelPositionUpdate>>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_CHANNELS).await?;

    for update in &body {
        let sql = if update.parent_id.is_some() {
            "UPDATE channels SET position = $1, parent_id = $2 WHERE id = $3 AND server_id = $4"
        } else {
            "UPDATE channels SET position = $1 WHERE id = $2 AND server_id = $3"
        };

        if let Some(ref parent_id) = update.parent_id {
            sqlx::query(sql)
                .bind(update.position)
                .bind(*parent_id)
                .bind(update.id)
                .bind(server_id)
                .execute(&state.db.pg)
                .await?;
        } else {
            sqlx::query(sql)
                .bind(update.position)
                .bind(update.id)
                .bind(server_id)
                .execute(&state.db.pg)
                .await?;
        }
    }

    let event = serde_json::json!({
        "type": "CHANNEL_UPDATE",
        "server_id": server_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Channel Followers (Announcement) ───────────────────────────

#[derive(Debug, Deserialize)]
pub struct FollowChannelRequest {
    pub webhook_channel_id: i64,
}

#[derive(Debug, Serialize)]
pub struct FollowChannelResponse {
    pub channel_id: Snowflake,
    pub webhook_channel_id: Snowflake,
}

async fn follow_channel(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<FollowChannelRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Verify source is an announcement channel
    let (_source_server_id, channel_type) = sqlx::query_as::<_, (Option<i64>, i16)>(
        "SELECT server_id, type FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Channel not found".into()))?;

    if channel_type != 5 {
        return Err(AppError::BadRequest("Can only follow announcement channels".into()));
    }

    // Check MANAGE_WEBHOOKS in target channel's server
    let target_server_id = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT server_id FROM channels WHERE id = $1",
    )
    .bind(body.webhook_channel_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Target channel not found".into()))?;

    if let Some(target_sid) = target_server_id {
        require_permissions(&state, target_sid, auth.id, Permissions::MANAGE_WEBHOOKS).await?;
    }

    // Create a webhook in the target channel for crossposting
    let webhook_id = state.snowflake.next_id();
    let token = nanoid::nanoid!(68);
    let source_server_id = target_server_id.unwrap_or(0);

    sqlx::query(
        "INSERT INTO webhooks (id, server_id, channel_id, creator_id, name, token, type) \
         VALUES ($1, $2, $3, $4, $5, $6, 2)",
    )
    .bind(webhook_id)
    .bind(source_server_id)
    .bind(body.webhook_channel_id)
    .bind(auth.id)
    .bind("Announcement Follow")
    .bind(&token)
    .execute(&state.db.pg)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(FollowChannelResponse {
            channel_id: Snowflake::from(channel_id),
            webhook_channel_id: Snowflake::from(body.webhook_channel_id),
        }),
    ))
}
