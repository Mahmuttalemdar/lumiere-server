use axum::{
    extract::{Path, Query, State},
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
use lumiere_permissions::Permissions;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::Validate;

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(create_server))
        .route("/{server_id}", get(get_server))
        .route("/{server_id}", patch(update_server))
        .route("/{server_id}", delete(delete_server))
        // Members
        .route("/{server_id}/members", get(get_members))
        .route("/{server_id}/members/{user_id}", get(get_member))
        .route("/{server_id}/members/{user_id}", patch(update_member))
        .route("/{server_id}/members/{user_id}", delete(kick_member))
        .route("/{server_id}/members/@me", patch(update_self_member))
        .route("/{server_id}/members/@me/nick", patch(update_self_member))
        .route("/{server_id}/leave", delete(leave_server))
        // Bans
        .route("/{server_id}/bans", get(get_bans))
        .route("/{server_id}/bans/{user_id}", get(get_ban))
        .route("/{server_id}/bans/{user_id}", put(create_ban))
        .route("/{server_id}/bans/{user_id}", delete(remove_ban))
        // Invites
        .route("/{server_id}/invites", get(get_server_invites))
        // Channel management (delegated to channels module)
        .route("/{server_id}/channels", get(super::channels::get_server_channels))
        .route("/{server_id}/channels", post(super::channels::create_channel))
        .route("/{server_id}/channels/reorder", patch(super::channels::reorder_channels))
}

/// Invite routes mounted under /api/v1/invites and /api/v1/channels
pub fn invite_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{code}", get(get_invite))
        .route("/{code}", post(use_invite))
        .route("/{code}", delete(delete_invite))
}

pub fn channel_invite_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{channel_id}/invites", post(create_invite))
}

// ─── Response types ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ServerResponse {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub description: Option<String>,
    pub owner_id: Snowflake,
    pub region: Option<String>,
    pub features: Vec<String>,
    pub verification_level: i16,
    pub default_message_notifications: i16,
    pub explicit_content_filter: i16,
    pub system_channel_id: Option<Snowflake>,
    pub rules_channel_id: Option<Snowflake>,
    pub max_members: i32,
    pub member_count: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct MemberResponse {
    pub user: super::users::PublicUser,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub roles: Vec<Snowflake>,
    pub joined_at: chrono::DateTime<chrono::Utc>,
    pub communication_disabled_until: Option<chrono::DateTime<chrono::Utc>>,
    pub flags: i64,
}

#[derive(Debug, Serialize)]
pub struct BanResponse {
    pub user: super::users::PublicUser,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InviteResponse {
    pub code: String,
    pub server_id: Snowflake,
    pub channel_id: Snowflake,
    pub inviter_id: Option<Snowflake>,
    pub max_uses: i32,
    pub uses: i32,
    pub max_age: i32,
    pub temporary: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ─── Helpers ────────────────────────────────────────────────────

/// Check if user is a member of the server
pub async fn require_member(state: &AppState, server_id: i64, user_id: Snowflake) -> Result<(), AppError> {
    let is_member = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM server_members WHERE server_id = $1 AND user_id = $2)",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_one(&state.db.pg)
    .await?;

    if !is_member {
        return Err(AppError::Forbidden("Not a member of this server".into()));
    }
    Ok(())
}

/// Get the computed permissions for a user in a server
pub async fn get_user_permissions(state: &AppState, server_id: i64, user_id: Snowflake) -> Result<Permissions, AppError> {
    // Check if owner
    let is_owner = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE id = $1 AND owner_id = $2)",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_one(&state.db.pg)
    .await?;

    if is_owner {
        return Ok(Permissions::all());
    }

    // Get all roles for this member (including @everyone)
    let role_perms = sqlx::query_scalar::<_, i64>(
        "SELECT r.permissions FROM roles r \
         LEFT JOIN member_roles mr ON mr.role_id = r.id AND mr.user_id = $2 \
         WHERE r.server_id = $1 AND (r.is_default = true OR mr.user_id IS NOT NULL)",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_all(&state.db.pg)
    .await?;

    let mut perms = Permissions::empty();
    for p in role_perms {
        perms |= Permissions::from_bits_truncate(p as u64);
    }

    // ADMINISTRATOR grants everything
    if perms.contains(Permissions::ADMINISTRATOR) {
        return Ok(Permissions::all());
    }

    Ok(perms)
}

/// Require specific permissions
pub async fn require_permissions(
    state: &AppState,
    server_id: i64,
    user_id: Snowflake,
    required: Permissions,
) -> Result<(), AppError> {
    require_member(state, server_id, user_id).await?;
    let perms = get_user_permissions(state, server_id, user_id).await?;

    if !perms.contains(required) {
        return Err(AppError::Forbidden("Missing required permissions".into()));
    }
    Ok(())
}

/// Check role hierarchy: actor must have a strictly higher role than target.
/// Server owner bypasses hierarchy checks.
pub async fn check_role_hierarchy(state: &AppState, server_id: i64, actor_id: Snowflake, target_id: i64) -> Result<(), AppError> {
    // Owner bypasses hierarchy
    let is_owner = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE id = $1 AND owner_id = $2)",
    )
    .bind(server_id)
    .bind(actor_id)
    .fetch_one(&state.db.pg)
    .await?;

    if is_owner {
        return Ok(());
    }

    let actor_highest = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT MAX(r.position) FROM roles r JOIN member_roles mr ON mr.role_id = r.id WHERE mr.server_id = $1 AND mr.user_id = $2",
    )
    .bind(server_id)
    .bind(actor_id)
    .fetch_one(&state.db.pg)
    .await?
    .unwrap_or(0);

    let target_highest = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT MAX(r.position) FROM roles r JOIN member_roles mr ON mr.role_id = r.id WHERE mr.server_id = $1 AND mr.user_id = $2",
    )
    .bind(server_id)
    .bind(target_id)
    .fetch_one(&state.db.pg)
    .await?
    .unwrap_or(0);

    if actor_highest <= target_highest {
        return Err(AppError::Forbidden("Cannot modify a member with equal or higher role".into()));
    }

    Ok(())
}

// ─── 5.1 — Server CRUD ─────────────────────────────────────────

#[derive(Debug, Deserialize, Validate)]
pub struct CreateServerRequest {
    #[validate(length(min = 2, max = 100))]
    pub name: String,
    pub icon: Option<String>,
    pub region: Option<String>,
}

async fn create_server(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateServerRequest>,
) -> Result<impl IntoResponse, AppError> {
    if let Err(errors) = body.validate() {
        return Err(AppError::Validation(crate::routes::validation_errors(errors)));
    }

    // Fix #10: Server creation limit — max 100 servers per user
    let server_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM server_members WHERE user_id = $1",
    )
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    if server_count >= 100 {
        return Err(AppError::BadRequest("You have reached the maximum number of servers (100)".into()));
    }

    let server_id = state.snowflake.next_id();

    // Fix #1: Wrap entire create_server in a transaction
    let mut tx = state.db.pg.begin().await?;

    // Create server
    sqlx::query(
        "INSERT INTO servers (id, name, icon, region, owner_id, member_count) \
         VALUES ($1, $2, $3, $4, $5, 1)",
    )
    .bind(server_id)
    .bind(body.name.trim())
    .bind(&body.icon)
    .bind(&body.region)
    .bind(auth.id)
    .execute(&mut *tx)
    .await?;

    // Create @everyone role (id = server_id, position 0)
    let everyone_perms = Permissions::default_everyone().bits() as i64;
    sqlx::query(
        "INSERT INTO roles (id, server_id, name, permissions, position, is_default) \
         VALUES ($1, $1, '@everyone', $2, 0, true)",
    )
    .bind(server_id)
    .bind(everyone_perms)
    .execute(&mut *tx)
    .await?;

    // Create default text channel "general"
    let text_channel_id = state.snowflake.next_id();
    sqlx::query(
        "INSERT INTO channels (id, server_id, type, name, position) \
         VALUES ($1, $2, 0, 'general', 0)",
    )
    .bind(text_channel_id)
    .bind(server_id)
    .execute(&mut *tx)
    .await?;

    // Create default voice channel "General"
    let voice_channel_id = state.snowflake.next_id();
    sqlx::query(
        "INSERT INTO channels (id, server_id, type, name, position) \
         VALUES ($1, $2, 2, 'General', 1)",
    )
    .bind(voice_channel_id)
    .bind(server_id)
    .execute(&mut *tx)
    .await?;

    // Set system_channel_id to the text channel
    sqlx::query("UPDATE servers SET system_channel_id = $1 WHERE id = $2")
        .bind(text_channel_id)
        .bind(server_id)
        .execute(&mut *tx)
        .await?;

    // Add creator as member
    sqlx::query(
        "INSERT INTO server_members (server_id, user_id) VALUES ($1, $2)",
    )
    .bind(server_id)
    .bind(auth.id)
    .execute(&mut *tx)
    .await?;

    // Assign @everyone role to creator
    sqlx::query(
        "INSERT INTO member_roles (server_id, user_id, role_id) VALUES ($1, $2, $1)",
    )
    .bind(server_id)
    .bind(auth.id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // Dispatch GUILD_CREATE
    let event = serde_json::json!({
        "type": "GUILD_CREATE",
        "server_id": server_id,
        "user_id": auth.id,
    });
    let _ = state.nats.publish(&format!("user.{}.guilds", auth.id), &event).await;

    let server = fetch_server(&state, server_id).await?;
    Ok((StatusCode::CREATED, Json(server)))
}

async fn fetch_server(state: &AppState, server_id: Snowflake) -> Result<ServerResponse, AppError> {
    let row = sqlx::query_as::<_, (i64, String, Option<String>, Option<String>, Option<String>, i64, Option<String>, Vec<String>, i16, i16, i16, Option<i64>, Option<i64>, i32, i32, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, name, icon, banner, description, owner_id, region, features, \
                verification_level, default_message_notifications, explicit_content_filter, \
                system_channel_id, rules_channel_id, max_members, member_count, created_at \
         FROM servers WHERE id = $1",
    )
    .bind(server_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Server not found".into()))?;

    Ok(ServerResponse {
        id: Snowflake::from(row.0),
        name: row.1,
        icon: row.2,
        banner: row.3,
        description: row.4,
        owner_id: Snowflake::from(row.5),
        region: row.6,
        features: row.7,
        verification_level: row.8,
        default_message_notifications: row.9,
        explicit_content_filter: row.10,
        system_channel_id: row.11.map(Snowflake::from),
        rules_channel_id: row.12.map(Snowflake::from),
        max_members: row.13,
        member_count: row.14,
        created_at: row.15,
    })
}

async fn get_server(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_member(&state, server_id, auth.id).await?;
    let server = fetch_server(&state, Snowflake::from(server_id)).await?;
    Ok(Json(server))
}

#[derive(Debug, Deserialize)]
pub struct UpdateServerRequest {
    pub name: Option<String>,
    pub icon: Option<Option<String>>,
    pub banner: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub region: Option<Option<String>>,
    pub verification_level: Option<i16>,
    pub default_message_notifications: Option<i16>,
    pub explicit_content_filter: Option<i16>,
    pub system_channel_id: Option<Option<i64>>,
    pub rules_channel_id: Option<Option<i64>>,
    pub owner_id: Option<i64>,
}

async fn update_server(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Json(body): Json<UpdateServerRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Fix #5: Ownership transfer requires being the owner
    if body.owner_id.is_some() {
        let is_owner = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM servers WHERE id = $1 AND owner_id = $2)",
        )
        .bind(server_id)
        .bind(auth.id)
        .fetch_one(&state.db.pg)
        .await?;

        if !is_owner {
            return Err(AppError::Forbidden("Only the owner can transfer ownership".into()));
        }

        let new_owner = body.owner_id.unwrap();
        // Verify new owner is a member
        require_member(&state, server_id, Snowflake::from(new_owner)).await?;

        sqlx::query("UPDATE servers SET owner_id = $1 WHERE id = $2")
            .bind(new_owner)
            .bind(server_id)
            .execute(&state.db.pg)
            .await?;
    }

    // Fix #5: Other field updates require MANAGE_SERVER permission
    let has_other_fields = body.name.is_some()
        || body.icon.is_some()
        || body.banner.is_some()
        || body.description.is_some()
        || body.region.is_some()
        || body.verification_level.is_some()
        || body.default_message_notifications.is_some()
        || body.explicit_content_filter.is_some()
        || body.system_channel_id.is_some()
        || body.rules_channel_id.is_some();

    if has_other_fields {
        require_permissions(&state, server_id, auth.id, Permissions::MANAGE_SERVER).await?;
    }

    // Build dynamic UPDATE
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

    add_set!(name, "name");
    add_set!(icon, "icon");
    add_set!(banner, "banner");
    add_set!(description, "description");
    add_set!(region, "region");
    add_set!(verification_level, "verification_level");
    add_set!(default_message_notifications, "default_message_notifications");
    add_set!(explicit_content_filter, "explicit_content_filter");
    add_set!(system_channel_id, "system_channel_id");
    add_set!(rules_channel_id, "rules_channel_id");

    if !sets.is_empty() {
        let sql = format!("UPDATE servers SET {} WHERE id = $1", sets.join(", "));
        let mut query = sqlx::query(&sql).bind(server_id);

        if let Some(ref v) = body.name { query = query.bind(v.trim()); }
        if let Some(ref v) = body.icon { query = query.bind(v.as_deref()); }
        if let Some(ref v) = body.banner { query = query.bind(v.as_deref()); }
        if let Some(ref v) = body.description { query = query.bind(v.as_deref()); }
        if let Some(ref v) = body.region { query = query.bind(v.as_deref()); }
        if let Some(v) = body.verification_level { query = query.bind(v); }
        if let Some(v) = body.default_message_notifications { query = query.bind(v); }
        if let Some(v) = body.explicit_content_filter { query = query.bind(v); }
        if let Some(ref v) = body.system_channel_id { query = query.bind(*v); }
        if let Some(ref v) = body.rules_channel_id { query = query.bind(*v); }

        query.execute(&state.db.pg).await?;
    }

    // Dispatch GUILD_UPDATE
    let event = serde_json::json!({ "type": "GUILD_UPDATE", "server_id": server_id });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    let server = fetch_server(&state, Snowflake::from(server_id)).await?;
    Ok(Json(server))
}

#[derive(Debug, Deserialize)]
pub struct DeleteServerRequest {
    pub password: String,
}

async fn delete_server(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Json(body): Json<DeleteServerRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Must be owner
    let is_owner = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE id = $1 AND owner_id = $2)",
    )
    .bind(server_id)
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    if !is_owner {
        return Err(AppError::Forbidden("Only the owner can delete a server".into()));
    }

    // Verify password
    let password_hash = sqlx::query_scalar::<_, String>(
        "SELECT password_hash FROM users WHERE id = $1",
    )
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    let valid = lumiere_auth::password::verify_password(&body.password, &password_hash)
        .map_err(AppError::Internal)?;

    if !valid {
        return Err(AppError::Unauthorized("Invalid password".into()));
    }

    // Dispatch GUILD_DELETE before cascade
    let event = serde_json::json!({ "type": "GUILD_DELETE", "server_id": server_id });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    // Delete server (CASCADE handles channels, roles, members, bans, invites)
    sqlx::query("DELETE FROM servers WHERE id = $1")
        .bind(server_id)
        .execute(&state.db.pg)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── 5.4 — Member Management ───────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MemberQuery {
    pub limit: Option<i64>,
    pub after: Option<i64>,
}

async fn get_members(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Query(query): Query<MemberQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_member(&state, server_id, auth.id).await?;

    let limit = query.limit.unwrap_or(100).min(1000);
    let after = query.after.unwrap_or(0);

    let rows = sqlx::query_as::<_, (i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, i64)>(
        "SELECT u.id, u.username, u.discriminator, u.avatar, u.banner, u.bio, u.flags, u.is_bot, \
                sm.nickname, sm.avatar, sm.joined_at, sm.communication_disabled_until, sm.flags \
         FROM server_members sm \
         JOIN users u ON u.id = sm.user_id \
         WHERE sm.server_id = $1 AND sm.user_id > $2 AND u.deleted_at IS NULL \
         ORDER BY sm.user_id ASC \
         LIMIT $3",
    )
    .bind(server_id)
    .bind(after)
    .bind(limit)
    .fetch_all(&state.db.pg)
    .await?;

    // Fix #7: N+1 fix — batch fetch all roles for all members
    let user_ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let all_roles = sqlx::query_as::<_, (i64, i64)>(
        "SELECT user_id, role_id FROM member_roles WHERE server_id = $1 AND user_id = ANY($2)",
    )
    .bind(server_id)
    .bind(&user_ids)
    .fetch_all(&state.db.pg)
    .await?;

    let mut role_map: std::collections::HashMap<i64, Vec<Snowflake>> = std::collections::HashMap::new();
    for (uid, rid) in all_roles {
        role_map.entry(uid).or_default().push(Snowflake::from(rid));
    }

    let mut members = Vec::with_capacity(rows.len());
    for r in rows {
        let user_id = r.0;
        members.push(MemberResponse {
            user: super::users::PublicUser {
                id: Snowflake::from(user_id),
                username: r.1,
                discriminator: r.2,
                avatar: r.3,
                banner: r.4,
                bio: r.5,
                flags: r.6,
                is_bot: r.7,
            },
            nickname: r.8,
            avatar: r.9,
            roles: role_map.remove(&user_id).unwrap_or_default(),
            joined_at: r.10,
            communication_disabled_until: r.11,
            flags: r.12,
        });
    }

    Ok(Json(members))
}

async fn get_member(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, user_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_member(&state, server_id, auth.id).await?;

    let row = sqlx::query_as::<_, (i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, i64)>(
        "SELECT u.id, u.username, u.discriminator, u.avatar, u.banner, u.bio, u.flags, u.is_bot, \
                sm.nickname, sm.avatar, sm.joined_at, sm.communication_disabled_until, sm.flags \
         FROM server_members sm \
         JOIN users u ON u.id = sm.user_id \
         WHERE sm.server_id = $1 AND sm.user_id = $2 AND u.deleted_at IS NULL",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Member not found".into()))?;

    let role_ids = sqlx::query_scalar::<_, i64>(
        "SELECT role_id FROM member_roles WHERE server_id = $1 AND user_id = $2",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_all(&state.db.pg)
    .await?;

    Ok(Json(MemberResponse {
        user: super::users::PublicUser {
            id: Snowflake::from(row.0),
            username: row.1,
            discriminator: row.2,
            avatar: row.3,
            banner: row.4,
            bio: row.5,
            flags: row.6,
            is_bot: row.7,
        },
        nickname: row.8,
        avatar: row.9,
        roles: role_ids.into_iter().map(Snowflake::from).collect(),
        joined_at: row.10,
        communication_disabled_until: row.11,
        flags: row.12,
    }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemberRequest {
    pub nickname: Option<Option<String>>,
    pub roles: Option<Vec<i64>>,
    pub communication_disabled_until: Option<Option<chrono::DateTime<chrono::Utc>>>,
}

async fn update_member(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, user_id)): Path<(i64, i64)>,
    Json(body): Json<UpdateMemberRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_member(&state, server_id, auth.id).await?;

    // Fix #4: Role hierarchy check before modifying another member
    check_role_hierarchy(&state, server_id, auth.id, user_id).await?;

    let perms = get_user_permissions(&state, server_id, auth.id).await?;

    // Nickname changes
    if body.nickname.is_some() {
        let is_self = user_id == auth.id.value() as i64;
        if is_self && !perms.contains(Permissions::CHANGE_NICKNAME) {
            return Err(AppError::Forbidden("Missing CHANGE_NICKNAME permission".into()));
        }
        if !is_self && !perms.contains(Permissions::MANAGE_NICKNAMES) {
            return Err(AppError::Forbidden("Missing MANAGE_NICKNAMES permission".into()));
        }
    }

    // Role changes
    if body.roles.is_some() {
        if !perms.contains(Permissions::MANAGE_ROLES) {
            return Err(AppError::Forbidden("Missing MANAGE_ROLES permission".into()));
        }
    }

    // Timeout changes
    if body.communication_disabled_until.is_some() {
        if !perms.contains(Permissions::MODERATE_MEMBERS) {
            return Err(AppError::Forbidden("Missing MODERATE_MEMBERS permission".into()));
        }
    }

    // Apply nickname
    if let Some(ref nickname) = body.nickname {
        sqlx::query("UPDATE server_members SET nickname = $1 WHERE server_id = $2 AND user_id = $3")
            .bind(nickname.as_deref())
            .bind(server_id)
            .bind(user_id)
            .execute(&state.db.pg)
            .await?;
    }

    // Apply timeout
    if let Some(ref timeout) = body.communication_disabled_until {
        sqlx::query("UPDATE server_members SET communication_disabled_until = $1 WHERE server_id = $2 AND user_id = $3")
            .bind(*timeout)
            .bind(server_id)
            .bind(user_id)
            .execute(&state.db.pg)
            .await?;
    }

    // Apply role changes
    if let Some(ref role_ids) = body.roles {
        // Remove existing roles (except @everyone)
        sqlx::query("DELETE FROM member_roles WHERE server_id = $1 AND user_id = $2 AND role_id != $1")
            .bind(server_id)
            .bind(user_id)
            .execute(&state.db.pg)
            .await?;

        // Add new roles
        for role_id in role_ids {
            sqlx::query(
                "INSERT INTO member_roles (server_id, user_id, role_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(server_id)
            .bind(user_id)
            .bind(role_id)
            .execute(&state.db.pg)
            .await?;
        }
    }

    // Dispatch GUILD_MEMBER_UPDATE
    let event = serde_json::json!({
        "type": "GUILD_MEMBER_UPDATE",
        "server_id": server_id,
        "user_id": user_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    // Return updated member
    get_member(State(state), auth, Path((server_id, user_id))).await
}

#[derive(Debug, Deserialize)]
pub struct UpdateSelfMemberRequest {
    pub nickname: Option<Option<String>>,
}

async fn update_self_member(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Json(body): Json<UpdateSelfMemberRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::CHANGE_NICKNAME).await?;

    if let Some(ref nickname) = body.nickname {
        sqlx::query("UPDATE server_members SET nickname = $1 WHERE server_id = $2 AND user_id = $3")
            .bind(nickname.as_deref())
            .bind(server_id)
            .bind(auth.id)
            .execute(&state.db.pg)
            .await?;
    }

    let event = serde_json::json!({
        "type": "GUILD_MEMBER_UPDATE",
        "server_id": server_id,
        "user_id": auth.id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    let user_id = auth.id.value() as i64;
    get_member(State(state), auth, Path((server_id, user_id))).await
}

async fn kick_member(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, user_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::KICK_MEMBERS).await?;

    // Can't kick the owner
    let is_owner = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE id = $1 AND owner_id = $2)",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_one(&state.db.pg)
    .await?;

    if is_owner {
        return Err(AppError::Forbidden("Cannot kick the server owner".into()));
    }

    // Fix #4: Role hierarchy check before kicking
    check_role_hierarchy(&state, server_id, auth.id, user_id).await?;

    let result = sqlx::query("DELETE FROM server_members WHERE server_id = $1 AND user_id = $2")
        .bind(server_id)
        .bind(user_id)
        .execute(&state.db.pg)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Member not found".into()));
    }

    // Fix #3: Use COUNT(*) to avoid member_count race condition
    sqlx::query(
        "UPDATE servers SET member_count = (SELECT COUNT(*) FROM server_members WHERE server_id = $1) WHERE id = $1",
    )
    .bind(server_id)
    .execute(&state.db.pg)
    .await?;

    // Dispatch events
    let event = serde_json::json!({
        "type": "GUILD_MEMBER_REMOVE",
        "server_id": server_id,
        "user_id": user_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    let guild_delete = serde_json::json!({
        "type": "GUILD_DELETE",
        "server_id": server_id,
    });
    let _ = state.nats.publish(&format!("user.{}.guilds", user_id), &guild_delete).await;

    Ok(StatusCode::NO_CONTENT)
}

async fn leave_server(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    // Owner cannot leave
    let is_owner = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE id = $1 AND owner_id = $2)",
    )
    .bind(server_id)
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    if is_owner {
        return Err(AppError::BadRequest(
            "Owner cannot leave. Transfer ownership first.".into(),
        ));
    }

    let result = sqlx::query("DELETE FROM server_members WHERE server_id = $1 AND user_id = $2")
        .bind(server_id)
        .bind(auth.id)
        .execute(&state.db.pg)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Not a member of this server".into()));
    }

    // Fix #3: Use COUNT(*) to avoid member_count race condition
    sqlx::query(
        "UPDATE servers SET member_count = (SELECT COUNT(*) FROM server_members WHERE server_id = $1) WHERE id = $1",
    )
    .bind(server_id)
    .execute(&state.db.pg)
    .await?;

    let event = serde_json::json!({
        "type": "GUILD_MEMBER_REMOVE",
        "server_id": server_id,
        "user_id": auth.id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── 5.6 — Ban Management ──────────────────────────────────────

async fn get_bans(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::BAN_MEMBERS).await?;

    let rows = sqlx::query_as::<_, (Option<String>, i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool)>(
        "SELECT b.reason, u.id, u.username, u.discriminator, u.avatar, u.banner, u.bio, u.flags, u.is_bot \
         FROM bans b \
         JOIN users u ON u.id = b.user_id \
         WHERE b.server_id = $1",
    )
    .bind(server_id)
    .fetch_all(&state.db.pg)
    .await?;

    let bans: Vec<BanResponse> = rows
        .into_iter()
        .map(|r| BanResponse {
            reason: r.0,
            user: super::users::PublicUser {
                id: Snowflake::from(r.1),
                username: r.2,
                discriminator: r.3,
                avatar: r.4,
                banner: r.5,
                bio: r.6,
                flags: r.7,
                is_bot: r.8,
            },
        })
        .collect();

    Ok(Json(bans))
}

async fn get_ban(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, user_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::BAN_MEMBERS).await?;

    let row = sqlx::query_as::<_, (Option<String>, i64, String, i16, Option<String>, Option<String>, Option<String>, i64, bool)>(
        "SELECT b.reason, u.id, u.username, u.discriminator, u.avatar, u.banner, u.bio, u.flags, u.is_bot \
         FROM bans b \
         JOIN users u ON u.id = b.user_id \
         WHERE b.server_id = $1 AND b.user_id = $2",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Ban not found".into()))?;

    Ok(Json(BanResponse {
        reason: row.0,
        user: super::users::PublicUser {
            id: Snowflake::from(row.1),
            username: row.2,
            discriminator: row.3,
            avatar: row.4,
            banner: row.5,
            bio: row.6,
            flags: row.7,
            is_bot: row.8,
        },
    }))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct CreateBanRequest {
    pub reason: Option<String>,
    /// Will be used when messaging system is implemented (Sprint 09)
    pub delete_message_seconds: Option<i64>,
}

async fn create_ban(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, user_id)): Path<(i64, i64)>,
    Json(body): Json<CreateBanRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::BAN_MEMBERS).await?;

    // Can't ban the owner
    let is_owner = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE id = $1 AND owner_id = $2)",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_one(&state.db.pg)
    .await?;

    if is_owner {
        return Err(AppError::Forbidden("Cannot ban the server owner".into()));
    }

    // Fix #4: Role hierarchy check before banning
    check_role_hierarchy(&state, server_id, auth.id, user_id).await?;

    // Remove member if present
    sqlx::query("DELETE FROM server_members WHERE server_id = $1 AND user_id = $2")
        .bind(server_id)
        .bind(user_id)
        .execute(&state.db.pg)
        .await?;

    // Create ban record
    sqlx::query(
        "INSERT INTO bans (server_id, user_id, reason, banned_by) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (server_id, user_id) DO UPDATE SET reason = $3, banned_by = $4",
    )
    .bind(server_id)
    .bind(user_id)
    .bind(&body.reason)
    .bind(auth.id)
    .execute(&state.db.pg)
    .await?;

    // Update member count
    sqlx::query(
        "UPDATE servers SET member_count = (SELECT COUNT(*) FROM server_members WHERE server_id = $1) WHERE id = $1",
    )
    .bind(server_id)
    .execute(&state.db.pg)
    .await?;

    // Dispatch events
    let event = serde_json::json!({
        "type": "GUILD_BAN_ADD",
        "server_id": server_id,
        "user_id": user_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    let guild_delete = serde_json::json!({ "type": "GUILD_DELETE", "server_id": server_id });
    let _ = state.nats.publish(&format!("user.{}.guilds", user_id), &guild_delete).await;

    Ok(StatusCode::NO_CONTENT)
}

async fn remove_ban(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, user_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::BAN_MEMBERS).await?;

    let result = sqlx::query("DELETE FROM bans WHERE server_id = $1 AND user_id = $2")
        .bind(server_id)
        .bind(user_id)
        .execute(&state.db.pg)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Ban not found".into()));
    }

    let event = serde_json::json!({
        "type": "GUILD_BAN_REMOVE",
        "server_id": server_id,
        "user_id": user_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── 5.3 — Invite System ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateInviteRequest {
    pub max_age: Option<i32>,
    pub max_uses: Option<i32>,
    pub temporary: Option<bool>,
}

async fn create_invite(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<CreateInviteRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Get server_id from channel
    let server_id = sqlx::query_scalar::<_, i64>(
        "SELECT server_id FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Channel not found".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::CREATE_INVITE).await?;

    let code = nanoid::nanoid!(8, &nanoid::alphabet::SAFE);
    let max_age = body.max_age.unwrap_or(86400);
    let max_uses = body.max_uses.unwrap_or(0);
    let temporary = body.temporary.unwrap_or(false);

    sqlx::query(
        "INSERT INTO invites (code, server_id, channel_id, inviter_id, max_age, max_uses, temporary) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&code)
    .bind(server_id)
    .bind(channel_id)
    .bind(auth.id)
    .bind(max_age)
    .bind(max_uses)
    .bind(temporary)
    .execute(&state.db.pg)
    .await?;

    // Dispatch INVITE_CREATE
    let event = serde_json::json!({
        "type": "INVITE_CREATE",
        "code": code,
        "server_id": server_id,
        "channel_id": channel_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    Ok((
        StatusCode::CREATED,
        Json(InviteResponse {
            code,
            server_id: Snowflake::from(server_id),
            channel_id: Snowflake::from(channel_id),
            inviter_id: Some(auth.id),
            max_uses,
            uses: 0,
            max_age,
            temporary,
            created_at: chrono::Utc::now(),
        }),
    ))
}

async fn get_server_invites(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_SERVER).await?;

    // Fix #8: Filter expired invites
    let rows = sqlx::query_as::<_, (String, i64, i64, Option<i64>, i32, i32, i32, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT code, server_id, channel_id, inviter_id, max_uses, uses, max_age, temporary, created_at \
         FROM invites WHERE server_id = $1 \
         AND (max_age = 0 OR created_at + interval '1 second' * max_age > now())",
    )
    .bind(server_id)
    .fetch_all(&state.db.pg)
    .await?;

    let invites: Vec<InviteResponse> = rows
        .into_iter()
        .map(|r| InviteResponse {
            code: r.0,
            server_id: Snowflake::from(r.1),
            channel_id: Snowflake::from(r.2),
            inviter_id: r.3.map(Snowflake::from),
            max_uses: r.4,
            uses: r.5,
            max_age: r.6,
            temporary: r.7,
            created_at: r.8,
        })
        .collect();

    Ok(Json(invites))
}

#[derive(Debug, Serialize)]
pub struct InvitePreview {
    pub code: String,
    pub server: InviteServerPreview,
    pub channel_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InviteServerPreview {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    pub member_count: i32,
}

// Public endpoint per spec — no auth required (invite preview)
async fn get_invite(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Fix #9: Include uses in the initial SELECT so we don't need a second query
    let row = sqlx::query_as::<_, (String, i64, String, Option<String>, i32, Option<String>, i32, i32, i32, chrono::DateTime<chrono::Utc>)>(
        "SELECT i.code, s.id, s.name, s.icon, s.member_count, c.name, i.max_age, i.max_uses, i.uses, i.created_at \
         FROM invites i \
         JOIN servers s ON s.id = i.server_id \
         JOIN channels c ON c.id = i.channel_id \
         WHERE i.code = $1",
    )
    .bind(&code)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Invite not found".into()))?;

    // Check if expired
    let max_age = row.6;
    if max_age > 0 {
        let created: chrono::DateTime<chrono::Utc> = row.9;
        let expires = created + chrono::Duration::seconds(max_age as i64);
        if chrono::Utc::now() > expires {
            return Err(AppError::NotFound("Invite expired".into()));
        }
    }

    // Check max uses (uses already fetched in row.8)
    let max_uses = row.7;
    let uses = row.8;
    if max_uses > 0 && uses >= max_uses {
        return Err(AppError::NotFound("Invite has reached max uses".into()));
    }

    Ok(Json(InvitePreview {
        code: row.0,
        server: InviteServerPreview {
            id: Snowflake::from(row.1),
            name: row.2,
            icon: row.3,
            member_count: row.4,
        },
        channel_name: row.5,
    }))
}

async fn use_invite(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(code): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Fix #2: Wrap use_invite in a transaction with SELECT ... FOR UPDATE
    let mut tx = state.db.pg.begin().await?;

    let invite = sqlx::query_as::<_, (i64, i64, i32, i32, i32, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT server_id, channel_id, max_age, max_uses, uses, temporary, created_at \
         FROM invites WHERE code = $1 FOR UPDATE",
    )
    .bind(&code)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("Invite not found".into()))?;

    let server_id = invite.0;

    // Check expiry
    if invite.2 > 0 {
        let expires = invite.6 + chrono::Duration::seconds(invite.2 as i64);
        if chrono::Utc::now() > expires {
            return Err(AppError::BadRequest("Invite has expired".into()));
        }
    }

    // Check max uses
    if invite.3 > 0 && invite.4 >= invite.3 {
        return Err(AppError::BadRequest("Invite has reached max uses".into()));
    }

    // Check not already member
    let is_member = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM server_members WHERE server_id = $1 AND user_id = $2)",
    )
    .bind(server_id)
    .bind(auth.id)
    .fetch_one(&mut *tx)
    .await?;

    if is_member {
        // Just return the server (no need to commit changes)
        tx.commit().await?;
        let server = fetch_server(&state, Snowflake::from(server_id)).await?;
        return Ok(Json(server));
    }

    // Check not banned
    let is_banned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM bans WHERE server_id = $1 AND user_id = $2)",
    )
    .bind(server_id)
    .bind(auth.id)
    .fetch_one(&mut *tx)
    .await?;

    if is_banned {
        return Err(AppError::Forbidden("You are banned from this server".into()));
    }

    // Add as member
    sqlx::query("INSERT INTO server_members (server_id, user_id) VALUES ($1, $2)")
        .bind(server_id)
        .bind(auth.id)
        .execute(&mut *tx)
        .await?;

    // Assign @everyone role
    sqlx::query("INSERT INTO member_roles (server_id, user_id, role_id) VALUES ($1, $2, $1)")
        .bind(server_id)
        .bind(auth.id)
        .execute(&mut *tx)
        .await?;

    // Update member count and invite uses
    sqlx::query("UPDATE servers SET member_count = member_count + 1 WHERE id = $1")
        .bind(server_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("UPDATE invites SET uses = uses + 1 WHERE code = $1")
        .bind(&code)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Dispatch events
    let member_event = serde_json::json!({
        "type": "GUILD_MEMBER_ADD",
        "server_id": server_id,
        "user_id": auth.id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &member_event).await;

    let guild_event = serde_json::json!({
        "type": "GUILD_CREATE",
        "server_id": server_id,
    });
    let _ = state.nats.publish(&format!("user.{}.guilds", auth.id), &guild_event).await;

    let server = fetch_server(&state, Snowflake::from(server_id)).await?;
    Ok(Json(server))
}

async fn delete_invite(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(code): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Get invite info
    let invite = sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT server_id, inviter_id FROM invites WHERE code = $1",
    )
    .bind(&code)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Invite not found".into()))?;

    let server_id = invite.0;
    let inviter_id = invite.1;

    // Must be invite creator or have MANAGE_SERVER
    let is_creator = inviter_id == Some(auth.id.value() as i64);
    if !is_creator {
        require_permissions(&state, server_id, auth.id, Permissions::MANAGE_SERVER).await?;
    }

    sqlx::query("DELETE FROM invites WHERE code = $1")
        .bind(&code)
        .execute(&state.db.pg)
        .await?;

    let event = serde_json::json!({
        "type": "INVITE_DELETE",
        "code": code,
        "server_id": server_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    Ok(StatusCode::NO_CONTENT)
}
