use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_models::{
    error::AppError,
    snowflake::Snowflake,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::Validate;

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Profile
        .route("/@me", get(get_me))
        .route("/@me", patch(update_me))
        .route("/@me", delete(delete_me))
        .route("/{user_id}", get(get_user))
        // Relationships
        .route("/@me/relationships", get(get_relationships))
        .route("/@me/relationships", post(create_relationship))
        .route("/@me/relationships/{user_id}", delete(remove_relationship))
        .route("/@me/relationships/{user_id}", put(block_user))
        // DM Channels
        .route("/@me/channels", get(get_dm_channels))
        .route("/@me/channels", post(create_dm_channel))
        // Settings
        .route("/@me/settings", get(get_settings))
        .route("/@me/settings", patch(update_settings))
        // Presence
        .route("/@me/presence", patch(update_presence))
        // My Servers
        .route("/@me/servers", get(get_my_servers))
        // Notes
        .route("/{user_id}/note", get(get_note))
        .route("/{user_id}/note", put(set_note))
}

// ─── Response types ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct FullUser {
    pub id: Snowflake,
    pub username: String,
    pub discriminator: i16,
    pub email: String,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub bio: Option<String>,
    pub locale: String,
    pub flags: i64,
    pub premium_type: i16,
    pub is_bot: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub id: Snowflake,
    pub username: String,
    pub discriminator: i16,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub bio: Option<String>,
    pub flags: i64,
    pub is_bot: bool,
}

// ─── 4.1 — User Profile CRUD ───────────────────────────────────

async fn get_me(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let row = sqlx::query_as::<_, (i64, String, i16, String, Option<String>, Option<String>, Option<String>, String, i64, i16, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, username, discriminator, email, avatar, banner, bio, locale, flags, premium_type, is_bot, created_at \
         FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(auth.id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("User not found".into()))?;

    Ok(Json(FullUser {
        id: Snowflake::from(row.0),
        username: row.1,
        discriminator: row.2,
        email: row.3,
        avatar: row.4,
        banner: row.5,
        bio: row.6,
        locale: row.7,
        flags: row.8,
        premium_type: row.9,
        is_bot: row.10,
        created_at: row.11,
    }))
}

async fn get_user(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let row = sqlx::query_as::<_, (i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool)>(
        "SELECT id, username, discriminator, avatar, banner, bio, flags, is_bot \
         FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("User not found".into()))?;

    Ok(Json(PublicUser {
        id: Snowflake::from(row.0),
        username: row.1,
        discriminator: row.2,
        avatar: row.3,
        banner: row.4,
        bio: row.5,
        flags: row.6,
        is_bot: row.7,
    }))
}

#[derive(Debug, Deserialize, Validate)]
pub struct UpdateProfileRequest {
    #[validate(length(min = 2, max = 32))]
    pub username: Option<String>,
    pub avatar: Option<Option<String>>,
    pub banner: Option<Option<String>>,
    #[validate(length(max = 190))]
    pub bio: Option<String>,
    #[validate(length(max = 10))]
    pub locale: Option<String>,
}

async fn update_me(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateProfileRequest>,
) -> Result<impl IntoResponse, AppError> {
    if let Err(errors) = body.validate() {
        let field_errors = crate::routes::validation_errors(errors);
        return Err(AppError::Validation(field_errors));
    }

    // Fix #11: Username rate limit using atomic Lua script
    if body.username.is_some() {
        let mut conn = state.redis.clone();
        let key = format!("username_change:{}", auth.id);

        let script = redis::Script::new(
            "local count = redis.call('INCR', KEYS[1]) if count == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end return count"
        );
        let count: i64 = script
            .key(&key)
            .arg(3600i64)
            .invoke_async(&mut conn)
            .await
            .unwrap_or(0);

        if count > 2 {
            return Err(AppError::RateLimited { retry_after: 3600 });
        }

        // Check uniqueness
        let username = body.username.as_ref().unwrap().trim();
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM users WHERE username = $1 AND id != $2)",
        )
        .bind(username)
        .bind(auth.id)
        .fetch_one(&state.db.pg)
        .await?;

        if exists {
            return Err(AppError::AlreadyExists("Username already taken".into()));
        }
    }

    // Build dynamic UPDATE query
    let mut sets = Vec::new();
    let mut param_idx = 2u32; // $1 is user_id

    if body.username.is_some() {
        sets.push(format!("username = ${param_idx}"));
        param_idx += 1;
    }
    if body.avatar.is_some() {
        sets.push(format!("avatar = ${param_idx}"));
        param_idx += 1;
    }
    if body.banner.is_some() {
        sets.push(format!("banner = ${param_idx}"));
        param_idx += 1;
    }
    if body.bio.is_some() {
        sets.push(format!("bio = ${param_idx}"));
        param_idx += 1;
    }
    if body.locale.is_some() {
        sets.push(format!("locale = ${param_idx}"));
        // param_idx not needed after last
    }

    if sets.is_empty() {
        return Err(AppError::BadRequest("No fields to update".into()));
    }

    let sql = format!(
        "UPDATE users SET {} WHERE id = $1 AND deleted_at IS NULL \
         RETURNING id, username, discriminator, email, avatar, banner, bio, locale, flags, premium_type, is_bot, created_at",
        sets.join(", ")
    );

    let mut query = sqlx::query_as::<_, (i64, String, i16, String, Option<String>, Option<String>, Option<String>, String, i64, i16, bool, chrono::DateTime<chrono::Utc>)>(&sql)
        .bind(auth.id);

    if let Some(ref username) = body.username {
        query = query.bind(username.trim());
    }
    if let Some(ref avatar) = body.avatar {
        query = query.bind(avatar.as_deref());
    }
    if let Some(ref banner) = body.banner {
        query = query.bind(banner.as_deref());
    }
    if let Some(ref bio) = body.bio {
        query = query.bind(bio.as_str());
    }
    if let Some(ref locale) = body.locale {
        query = query.bind(locale.as_str());
    }

    let row = query
        .fetch_optional(&state.db.pg)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".into()))?;

    Ok(Json(FullUser {
        id: Snowflake::from(row.0),
        username: row.1,
        discriminator: row.2,
        email: row.3,
        avatar: row.4,
        banner: row.5,
        bio: row.6,
        locale: row.7,
        flags: row.8,
        premium_type: row.9,
        is_bot: row.10,
        created_at: row.11,
    }))
}

#[derive(Debug, Deserialize)]
pub struct DeleteAccountRequest {
    pub password: String,
}

async fn delete_me(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<DeleteAccountRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Verify password
    let password_hash = sqlx::query_scalar::<_, String>(
        "SELECT password_hash FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(auth.id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("User not found".into()))?;

    let valid = lumiere_auth::password::verify_password(&body.password, &password_hash)
        .map_err(AppError::Internal)?;

    if !valid {
        return Err(AppError::Unauthorized("Invalid password".into()));
    }

    // Fix #12: Check server ownership before deleting account
    let owns_servers = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE owner_id = $1)",
    )
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    if owns_servers {
        return Err(AppError::BadRequest("Transfer ownership of all servers before deleting your account".into()));
    }

    // Soft delete — set deleted_at, will be cleaned up after 30 days
    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(auth.id)
        .execute(&state.db.pg)
        .await?;

    // Revoke all sessions
    let session_mgr = lumiere_auth::session::SessionManager::new(state.redis.clone());
    session_mgr
        .revoke_all_sessions(&auth.id.to_string())
        .await
        .map_err(AppError::Internal)?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── 5.8 — Server List for User ─────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PartialServer {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    pub owner_id: Snowflake,
    pub member_count: i32,
    pub features: Vec<String>,
}

async fn get_my_servers(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, i64, i32, Vec<String>)>(
        "SELECT s.id, s.name, s.icon, s.owner_id, s.member_count, s.features \
         FROM servers s \
         JOIN server_members sm ON sm.server_id = s.id \
         WHERE sm.user_id = $1 \
         ORDER BY sm.joined_at DESC",
    )
    .bind(auth.id)
    .fetch_all(&state.db.pg)
    .await?;

    let servers: Vec<PartialServer> = rows
        .into_iter()
        .map(|r| PartialServer {
            id: Snowflake::from(r.0),
            name: r.1,
            icon: r.2,
            owner_id: Snowflake::from(r.3),
            member_count: r.4,
            features: r.5,
        })
        .collect();

    Ok(Json(servers))
}

// ─── 4.3 — Friend System ───────────────────────────────────────

/// Relationship types: 1=friend, 2=blocked, 3=incoming_request, 4=outgoing_request
#[derive(Debug, Serialize)]
pub struct Relationship {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub rel_type: i16,
    pub user: PublicUser,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

async fn get_relationships(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let rows = sqlx::query_as::<_, (i64, i16, chrono::DateTime<chrono::Utc>, i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool)>(
        "SELECT r.target_id, r.type, r.created_at, \
                u.id, u.username, u.discriminator, u.avatar, u.banner, u.bio, u.flags, u.is_bot \
         FROM relationships r \
         JOIN users u ON u.id = r.target_id \
         WHERE r.user_id = $1 AND u.deleted_at IS NULL \
         ORDER BY r.created_at DESC",
    )
    .bind(auth.id)
    .fetch_all(&state.db.pg)
    .await?;

    let relationships: Vec<Relationship> = rows
        .into_iter()
        .map(|r| Relationship {
            id: Snowflake::from(r.0),
            rel_type: r.1,
            user: PublicUser {
                id: Snowflake::from(r.3),
                username: r.4,
                discriminator: r.5,
                avatar: r.6,
                banner: r.7,
                bio: r.8,
                flags: r.9,
                is_bot: r.10,
            },
            created_at: r.2,
        })
        .collect();

    Ok(Json(relationships))
}

#[derive(Debug, Deserialize)]
pub struct CreateRelationshipRequest {
    pub username: Option<String>,
    pub user_id: Option<i64>,
}

async fn create_relationship(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateRelationshipRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Resolve target user
    let target_id: i64 = if let Some(user_id) = body.user_id {
        user_id
    } else if let Some(ref username) = body.username {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM users WHERE username = $1 AND deleted_at IS NULL",
        )
        .bind(username)
        .fetch_optional(&state.db.pg)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".into()))?
    } else {
        return Err(AppError::BadRequest(
            "Must provide username or user_id".into(),
        ));
    };

    if target_id == auth.id.value() as i64 {
        return Err(AppError::BadRequest(
            "Cannot send friend request to yourself".into(),
        ));
    }

    // Fix #17: Verify target user exists when user_id was provided directly
    if body.user_id.is_some() {
        let target_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NULL)",
        )
        .bind(target_id)
        .fetch_one(&state.db.pg)
        .await?;

        if !target_exists {
            return Err(AppError::NotFound("User not found".into()));
        }
    }

    // Check if target has blocked us
    let blocked = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM relationships WHERE user_id = $1 AND target_id = $2 AND type = 2)",
    )
    .bind(target_id)
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    if blocked {
        return Err(AppError::BadRequest(
            "Cannot send friend request to this user".into(),
        ));
    }

    // Check if we blocked them
    let we_blocked = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM relationships WHERE user_id = $1 AND target_id = $2 AND type = 2)",
    )
    .bind(auth.id)
    .bind(target_id)
    .fetch_one(&state.db.pg)
    .await?;

    if we_blocked {
        return Err(AppError::BadRequest(
            "You have blocked this user. Unblock them first.".into(),
        ));
    }

    // Check if incoming request exists (accept it)
    let incoming = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM relationships WHERE user_id = $1 AND target_id = $2 AND type = 3)",
    )
    .bind(auth.id)
    .bind(target_id)
    .fetch_one(&state.db.pg)
    .await?;

    if incoming {
        // Accept: update both sides to type=1 (friend)
        sqlx::query(
            "UPDATE relationships SET type = 1 WHERE \
             (user_id = $1 AND target_id = $2) OR (user_id = $2 AND target_id = $1)",
        )
        .bind(auth.id)
        .bind(target_id)
        .execute(&state.db.pg)
        .await?;

        // Broadcast RELATIONSHIP_ADD via NATS
        let event = serde_json::json!({
            "type": "RELATIONSHIP_ADD",
            "user_id": auth.id,
            "target_id": target_id,
            "relationship_type": 1
        });
        let _ = state.nats.publish(&format!("user.{}.relationships", auth.id), &event).await;
        let _ = state.nats.publish(&format!("user.{}.relationships", target_id), &event).await;

        return Ok(StatusCode::NO_CONTENT);
    }

    // Check if already friends or request pending
    let existing = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM relationships WHERE user_id = $1 AND target_id = $2)",
    )
    .bind(auth.id)
    .bind(target_id)
    .fetch_one(&state.db.pg)
    .await?;

    if existing {
        return Err(AppError::AlreadyExists(
            "Relationship already exists".into(),
        ));
    }

    // Create outgoing request (type=4) for sender, incoming (type=3) for receiver
    sqlx::query(
        "INSERT INTO relationships (user_id, target_id, type) VALUES ($1, $2, 4), ($2, $1, 3)",
    )
    .bind(auth.id)
    .bind(target_id)
    .execute(&state.db.pg)
    .await?;

    // Broadcast events
    let event_out = serde_json::json!({
        "type": "RELATIONSHIP_ADD",
        "user_id": auth.id,
        "target_id": target_id,
        "relationship_type": 4
    });
    let event_in = serde_json::json!({
        "type": "RELATIONSHIP_ADD",
        "user_id": target_id,
        "target_id": auth.id.value(),
        "relationship_type": 3
    });
    let _ = state.nats.publish(&format!("user.{}.relationships", auth.id), &event_out).await;
    let _ = state.nats.publish(&format!("user.{}.relationships", target_id), &event_in).await;

    Ok(StatusCode::NO_CONTENT)
}

async fn remove_relationship(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(target_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    // Remove both sides
    let result = sqlx::query(
        "DELETE FROM relationships WHERE \
         (user_id = $1 AND target_id = $2) OR (user_id = $2 AND target_id = $1)",
    )
    .bind(auth.id)
    .bind(target_id)
    .execute(&state.db.pg)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Relationship not found".into()));
    }

    // Broadcast RELATIONSHIP_REMOVE
    let event = serde_json::json!({
        "type": "RELATIONSHIP_REMOVE",
        "user_id": auth.id,
        "target_id": target_id
    });
    let _ = state.nats.publish(&format!("user.{}.relationships", auth.id), &event).await;
    let _ = state.nats.publish(&format!("user.{}.relationships", target_id), &event).await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct BlockRequest {
    #[serde(rename = "type")]
    pub rel_type: i16,
}

async fn block_user(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(target_id): Path<i64>,
    Json(body): Json<BlockRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.rel_type != 2 {
        return Err(AppError::BadRequest(
            "Only type 2 (block) is supported via PUT".into(),
        ));
    }

    if target_id == auth.id.value() as i64 {
        return Err(AppError::BadRequest("Cannot block yourself".into()));
    }

    // Fix #16: Verify target user exists before blocking
    let target_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(target_id)
    .fetch_one(&state.db.pg)
    .await?;

    if !target_exists {
        return Err(AppError::NotFound("User not found".into()));
    }

    // Remove any existing relationships in both directions
    sqlx::query(
        "DELETE FROM relationships WHERE \
         (user_id = $1 AND target_id = $2) OR (user_id = $2 AND target_id = $1)",
    )
    .bind(auth.id)
    .bind(target_id)
    .execute(&state.db.pg)
    .await?;

    // Insert block relationship (only blocker's side)
    sqlx::query("INSERT INTO relationships (user_id, target_id, type) VALUES ($1, $2, 2)")
        .bind(auth.id)
        .bind(target_id)
        .execute(&state.db.pg)
        .await?;

    // Broadcast events
    let event = serde_json::json!({
        "type": "RELATIONSHIP_REMOVE",
        "user_id": auth.id,
        "target_id": target_id
    });
    let _ = state.nats.publish(&format!("user.{}.relationships", target_id), &event).await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── 4.4 — Direct Messages ─────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DmChannel {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub channel_type: i16,
    pub recipients: Vec<PublicUser>,
    pub last_message_id: Option<Snowflake>,
    pub name: Option<String>,
    pub icon: Option<String>,
}

async fn get_dm_channels(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    // Get all DM channels the user is a recipient of
    let rows = sqlx::query_as::<_, (i64, i16, Option<i64>, Option<String>, Option<String>)>(
        "SELECT c.id, c.type, c.last_message_id, c.name, c.icon \
         FROM channels c \
         JOIN dm_recipients dr ON dr.channel_id = c.id \
         WHERE dr.user_id = $1 AND c.type IN (1, 3) \
         ORDER BY c.last_message_id DESC NULLS LAST",
    )
    .bind(auth.id)
    .fetch_all(&state.db.pg)
    .await?;

    // Fix #13: N+1 fix — batch-load all recipients for all DM channels
    let channel_ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let all_recipients = sqlx::query_as::<_, (i64, i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool)>(
        "SELECT dr.channel_id, u.id, u.username, u.discriminator, u.avatar, u.banner, u.bio, u.flags, u.is_bot \
         FROM dm_recipients dr JOIN users u ON u.id = dr.user_id \
         WHERE dr.channel_id = ANY($1) AND dr.user_id != $2 AND u.deleted_at IS NULL",
    )
    .bind(&channel_ids)
    .bind(auth.id)
    .fetch_all(&state.db.pg)
    .await?;

    let mut recipient_map: std::collections::HashMap<i64, Vec<PublicUser>> = std::collections::HashMap::new();
    for r in all_recipients {
        recipient_map.entry(r.0).or_default().push(PublicUser {
            id: Snowflake::from(r.1),
            username: r.2,
            discriminator: r.3,
            avatar: r.4,
            banner: r.5,
            bio: r.6,
            flags: r.7,
            is_bot: r.8,
        });
    }

    let mut channels = Vec::with_capacity(rows.len());
    for row in rows {
        let channel_id = row.0;
        channels.push(DmChannel {
            id: Snowflake::from(channel_id),
            channel_type: row.1,
            last_message_id: row.2.map(Snowflake::from),
            name: row.3,
            icon: row.4,
            recipients: recipient_map.remove(&channel_id).unwrap_or_default(),
        });
    }

    Ok(Json(channels))
}

#[derive(Debug, Deserialize)]
pub struct CreateDmRequest {
    pub recipient_id: Option<i64>,
    pub recipients: Option<Vec<i64>>,
}

async fn create_dm_channel(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateDmRequest>,
) -> Result<impl IntoResponse, AppError> {
    let recipient_ids: Vec<i64> = if let Some(id) = body.recipient_id {
        vec![id]
    } else if let Some(ids) = body.recipients {
        ids
    } else {
        return Err(AppError::BadRequest(
            "Must provide recipient_id or recipients".into(),
        ));
    };

    if recipient_ids.is_empty() {
        return Err(AppError::BadRequest("Must provide at least one recipient".into()));
    }

    // Group DM max 10 users (including self)
    if recipient_ids.len() > 9 {
        return Err(AppError::BadRequest(
            "Group DM limited to 10 users".into(),
        ));
    }

    let auth_id_i64 = auth.id.value() as i64;

    // Cannot DM yourself
    if recipient_ids.len() == 1 && recipient_ids[0] == auth_id_i64 {
        return Err(AppError::BadRequest("Cannot create DM with yourself".into()));
    }

    // Check blocked status for 1:1 DMs
    if recipient_ids.len() == 1 {
        let blocked = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM relationships WHERE user_id = $1 AND target_id = $2 AND type = 2)",
        )
        .bind(recipient_ids[0])
        .bind(auth.id)
        .fetch_one(&state.db.pg)
        .await?;

        if blocked {
            return Err(AppError::Forbidden(
                "Cannot send DM to this user".into(),
            ));
        }
    }

    // For 1:1 DM, check if channel already exists (idempotent)
    if recipient_ids.len() == 1 {
        let existing = sqlx::query_scalar::<_, i64>(
            "SELECT c.id FROM channels c \
             JOIN dm_recipients dr1 ON dr1.channel_id = c.id AND dr1.user_id = $1 \
             JOIN dm_recipients dr2 ON dr2.channel_id = c.id AND dr2.user_id = $2 \
             WHERE c.type = 1",
        )
        .bind(auth.id)
        .bind(recipient_ids[0])
        .fetch_optional(&state.db.pg)
        .await?;

        if let Some(channel_id) = existing {
            // Return existing channel
            let recipient = sqlx::query_as::<_, (i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool)>(
                "SELECT id, username, discriminator, avatar, banner, bio, flags, is_bot \
                 FROM users WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(recipient_ids[0])
            .fetch_optional(&state.db.pg)
            .await?
            .ok_or_else(|| AppError::NotFound("Recipient not found".into()))?;

            return Ok((
                StatusCode::OK,
                Json(DmChannel {
                    id: Snowflake::from(channel_id),
                    channel_type: 1,
                    last_message_id: None,
                    name: None,
                    icon: None,
                    recipients: vec![PublicUser {
                        id: Snowflake::from(recipient.0),
                        username: recipient.1,
                        discriminator: recipient.2,
                        avatar: recipient.3,
                        banner: recipient.4,
                        bio: recipient.5,
                        flags: recipient.6,
                        is_bot: recipient.7,
                    }],
                }),
            ));
        }
    }

    // Create new channel
    let channel_id = state.snowflake.next_id();
    let channel_type: i16 = if recipient_ids.len() == 1 { 1 } else { 3 };

    sqlx::query("INSERT INTO channels (id, type) VALUES ($1, $2)")
        .bind(channel_id)
        .bind(channel_type)
        .execute(&state.db.pg)
        .await?;

    // Add all recipients including self
    let mut all_users = vec![auth_id_i64];
    all_users.extend(&recipient_ids);

    for user_id in &all_users {
        sqlx::query("INSERT INTO dm_recipients (channel_id, user_id) VALUES ($1, $2)")
            .bind(channel_id)
            .bind(user_id)
            .execute(&state.db.pg)
            .await?;
    }

    // Fetch recipient public profiles
    let recipients = sqlx::query_as::<_, (i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool)>(
        "SELECT id, username, discriminator, avatar, banner, bio, flags, is_bot \
         FROM users WHERE id = ANY($1) AND id != $2 AND deleted_at IS NULL",
    )
    .bind(&recipient_ids)
    .bind(auth.id)
    .fetch_all(&state.db.pg)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(DmChannel {
            id: channel_id,
            channel_type,
            last_message_id: None,
            name: None,
            icon: None,
            recipients: recipients
                .into_iter()
                .map(|r| PublicUser {
                    id: Snowflake::from(r.0),
                    username: r.1,
                    discriminator: r.2,
                    avatar: r.3,
                    banner: r.4,
                    bio: r.5,
                    flags: r.6,
                    is_bot: r.7,
                })
                .collect(),
        }),
    ))
}

// ─── 4.5 — User Settings ───────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct UserSettings {
    pub theme: String,
    pub message_display: String,
    pub locale: String,
    pub show_current_game: bool,
    pub inline_attachment_media: bool,
    pub inline_embed_media: bool,
    pub render_embeds: bool,
    pub render_reactions: bool,
    pub animate_emoji: bool,
    pub enable_tts: bool,
    pub status: String,
    pub custom_status: Option<serde_json::Value>,
    pub dm_notifications: bool,
    pub friend_request_notifications: bool,
}

async fn get_settings(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let row = sqlx::query_as::<_, (String, String, String, bool, bool, bool, bool, bool, bool, bool, String, Option<serde_json::Value>, bool, bool)>(
        "SELECT theme, message_display, locale, show_current_game, inline_attachment_media, \
                inline_embed_media, render_embeds, render_reactions, animate_emoji, enable_tts, \
                status, custom_status, dm_notifications, friend_request_notifications \
         FROM user_settings WHERE user_id = $1",
    )
    .bind(auth.id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Settings not found".into()))?;

    Ok(Json(UserSettings {
        theme: row.0,
        message_display: row.1,
        locale: row.2,
        show_current_game: row.3,
        inline_attachment_media: row.4,
        inline_embed_media: row.5,
        render_embeds: row.6,
        render_reactions: row.7,
        animate_emoji: row.8,
        enable_tts: row.9,
        status: row.10,
        custom_status: row.11,
        dm_notifications: row.12,
        friend_request_notifications: row.13,
    }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub theme: Option<String>,
    pub message_display: Option<String>,
    pub locale: Option<String>,
    pub show_current_game: Option<bool>,
    pub inline_attachment_media: Option<bool>,
    pub inline_embed_media: Option<bool>,
    pub render_embeds: Option<bool>,
    pub render_reactions: Option<bool>,
    pub animate_emoji: Option<bool>,
    pub enable_tts: Option<bool>,
    pub status: Option<String>,
    pub custom_status: Option<Option<serde_json::Value>>,
    pub dm_notifications: Option<bool>,
    pub friend_request_notifications: Option<bool>,
}

async fn update_settings(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateSettingsRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Fix #15: Validate status — "offline" is server-determined, not user-settable
    if let Some(ref status) = body.status {
        let valid = ["online", "idle", "dnd", "invisible"];
        if !valid.contains(&status.as_str()) {
            return Err(AppError::BadRequest(format!(
                "Invalid status. Must be one of: {}",
                valid.join(", ")
            )));
        }
    }

    // Build dynamic update
    let mut sets = Vec::new();
    let mut param_idx = 2u32;

    macro_rules! add_field {
        ($field:ident, $col:expr) => {
            if body.$field.is_some() {
                sets.push(format!("{} = ${}", $col, param_idx));
                #[allow(unused_assignments)]
                { param_idx += 1; }
            }
        };
    }

    add_field!(theme, "theme");
    add_field!(message_display, "message_display");
    add_field!(locale, "locale");
    add_field!(show_current_game, "show_current_game");
    add_field!(inline_attachment_media, "inline_attachment_media");
    add_field!(inline_embed_media, "inline_embed_media");
    add_field!(render_embeds, "render_embeds");
    add_field!(render_reactions, "render_reactions");
    add_field!(animate_emoji, "animate_emoji");
    add_field!(enable_tts, "enable_tts");
    add_field!(status, "status");
    add_field!(custom_status, "custom_status");
    add_field!(dm_notifications, "dm_notifications");
    add_field!(friend_request_notifications, "friend_request_notifications");

    if sets.is_empty() {
        return Err(AppError::BadRequest("No fields to update".into()));
    }

    let sql = format!(
        "UPDATE user_settings SET {} WHERE user_id = $1 \
         RETURNING theme, message_display, locale, show_current_game, inline_attachment_media, \
                   inline_embed_media, render_embeds, render_reactions, animate_emoji, enable_tts, \
                   status, custom_status, dm_notifications, friend_request_notifications",
        sets.join(", ")
    );

    let mut query = sqlx::query_as::<_, (String, String, String, bool, bool, bool, bool, bool, bool, bool, String, Option<serde_json::Value>, bool, bool)>(&sql)
        .bind(auth.id);

    // Bind in same order as sets
    if let Some(ref v) = body.theme { query = query.bind(v.as_str()); }
    if let Some(ref v) = body.message_display { query = query.bind(v.as_str()); }
    if let Some(ref v) = body.locale { query = query.bind(v.as_str()); }
    if let Some(v) = body.show_current_game { query = query.bind(v); }
    if let Some(v) = body.inline_attachment_media { query = query.bind(v); }
    if let Some(v) = body.inline_embed_media { query = query.bind(v); }
    if let Some(v) = body.render_embeds { query = query.bind(v); }
    if let Some(v) = body.render_reactions { query = query.bind(v); }
    if let Some(v) = body.animate_emoji { query = query.bind(v); }
    if let Some(v) = body.enable_tts { query = query.bind(v); }
    if let Some(ref v) = body.status { query = query.bind(v.as_str()); }
    if let Some(ref v) = body.custom_status {
        query = query.bind(v.as_ref());
    }
    if let Some(v) = body.dm_notifications { query = query.bind(v); }
    if let Some(v) = body.friend_request_notifications { query = query.bind(v); }

    let row = query
        .fetch_optional(&state.db.pg)
        .await?
        .ok_or_else(|| AppError::NotFound("Settings not found".into()))?;

    // If status changed, update presence in Redis and broadcast
    if body.status.is_some() || body.custom_status.is_some() {
        update_presence_in_redis(&state, auth.id, row.10.as_str(), row.11.as_ref()).await;
    }

    Ok(Json(UserSettings {
        theme: row.0,
        message_display: row.1,
        locale: row.2,
        show_current_game: row.3,
        inline_attachment_media: row.4,
        inline_embed_media: row.5,
        render_embeds: row.6,
        render_reactions: row.7,
        animate_emoji: row.8,
        enable_tts: row.9,
        status: row.10,
        custom_status: row.11,
        dm_notifications: row.12,
        friend_request_notifications: row.13,
    }))
}

// ─── 4.6 — Presence / Status ───────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UpdatePresenceRequest {
    pub status: String,
    pub custom_status: Option<serde_json::Value>,
}

async fn update_presence(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdatePresenceRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Fix #15: Consistent status validation — "offline" is server-determined
    let valid = ["online", "idle", "dnd", "invisible"];
    if !valid.contains(&body.status.as_str()) {
        return Err(AppError::BadRequest(format!(
            "Invalid status. Must be one of: {}",
            valid.join(", ")
        )));
    }

    // Update settings table too
    sqlx::query("UPDATE user_settings SET status = $1, custom_status = $2 WHERE user_id = $3")
        .bind(&body.status)
        .bind(&body.custom_status)
        .bind(auth.id)
        .execute(&state.db.pg)
        .await?;

    update_presence_in_redis(&state, auth.id, &body.status, body.custom_status.as_ref()).await;

    Ok(StatusCode::NO_CONTENT)
}

async fn update_presence_in_redis(
    state: &AppState,
    user_id: Snowflake,
    status: &str,
    custom_status: Option<&serde_json::Value>,
) {
    let mut conn = state.redis.clone();
    let key = format!("presence:{}", user_id);
    let presence = serde_json::json!({
        "status": status,
        "custom_status": custom_status,
        "last_active": chrono::Utc::now().to_rfc3339(),
    });

    // Store in Redis with 5-minute TTL
    let _: Result<(), _> = redis::cmd("SET")
        .arg(&key)
        .arg(presence.to_string())
        .arg("EX")
        .arg(300i64)
        .query_async(&mut conn)
        .await;

    // Broadcast PRESENCE_UPDATE via NATS (only if not invisible)
    if status != "invisible" {
        let event = serde_json::json!({
            "type": "PRESENCE_UPDATE",
            "user_id": user_id,
            "status": status,
            "custom_status": custom_status,
        });
        let _ = state
            .nats
            .publish(&format!("user.{}.presence", user_id), &event)
            .await;
    }
}

// ─── 4.7 — User Notes ──────────────────────────────────────────

async fn get_note(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(target_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let note = sqlx::query_scalar::<_, String>(
        "SELECT note FROM user_notes WHERE user_id = $1 AND target_id = $2",
    )
    .bind(auth.id)
    .bind(target_id)
    .fetch_optional(&state.db.pg)
    .await?;

    match note {
        Some(n) => Ok(Json(serde_json::json!({ "note": n }))),
        None => Ok(Json(serde_json::json!({ "note": null }))),
    }
}

#[derive(Debug, Deserialize)]
pub struct SetNoteRequest {
    pub note: String,
}

async fn set_note(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(target_id): Path<i64>,
    Json(body): Json<SetNoteRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Fix #14: Use chars().count() for proper Unicode character counting
    if body.note.chars().count() > 256 {
        return Err(AppError::BadRequest(
            "Note must be 256 characters or less".into(),
        ));
    }

    if body.note.is_empty() {
        // Delete note if empty
        sqlx::query("DELETE FROM user_notes WHERE user_id = $1 AND target_id = $2")
            .bind(auth.id)
            .bind(target_id)
            .execute(&state.db.pg)
            .await?;
    } else {
        // Upsert note
        sqlx::query(
            "INSERT INTO user_notes (user_id, target_id, note) VALUES ($1, $2, $3) \
             ON CONFLICT (user_id, target_id) DO UPDATE SET note = $3, updated_at = now()",
        )
        .bind(auth.id)
        .bind(target_id)
        .bind(&body.note)
        .execute(&state.db.pg)
        .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}
