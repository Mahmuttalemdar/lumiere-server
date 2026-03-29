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
use lumiere_permissions::Permissions;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::Validate;

use crate::AppState;
use super::servers::{get_user_permissions, require_member, require_permissions};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{server_id}/roles", get(get_roles))
        .route("/{server_id}/roles", post(create_role))
        .route("/{server_id}/roles", patch(reorder_roles))
        .route("/{server_id}/roles/{role_id}", patch(update_role))
        .route("/{server_id}/roles/{role_id}", delete(delete_role))
}

pub fn channel_permissions_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{channel_id}/permissions/{override_id}", put(set_channel_override))
        .route("/{channel_id}/permissions/{override_id}", delete(delete_channel_override))
}

// ─── Response types ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RoleResponse {
    pub id: Snowflake,
    pub server_id: Snowflake,
    pub name: String,
    pub color: i32,
    pub hoist: bool,
    pub icon: Option<String>,
    pub position: i32,
    pub permissions: String,
    pub mentionable: bool,
    pub is_default: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ─── Helpers ────────────────────────────────────────────────────

async fn fetch_role(state: &AppState, role_id: i64) -> Result<RoleResponse, AppError> {
    let row = sqlx::query_as::<_, (i64, i64, String, i32, bool, Option<String>, i32, i64, bool, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, server_id, name, color, hoist, icon, position, permissions, mentionable, is_default, created_at \
         FROM roles WHERE id = $1",
    )
    .bind(role_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Role not found".into()))?;

    Ok(RoleResponse {
        id: Snowflake::from(row.0),
        server_id: Snowflake::from(row.1),
        name: row.2,
        color: row.3,
        hoist: row.4,
        icon: row.5,
        position: row.6,
        permissions: row.7.to_string(),
        mentionable: row.8,
        is_default: row.9,
        created_at: row.10,
    })
}

async fn get_actor_highest_position(state: &AppState, server_id: i64, user_id: Snowflake) -> Result<i32, AppError> {
    // Check if owner first
    let is_owner = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE id = $1 AND owner_id = $2)",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_one(&state.db.pg)
    .await?;

    if is_owner {
        return Ok(i32::MAX);
    }

    let highest = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT MAX(r.position) FROM roles r \
         JOIN member_roles mr ON mr.role_id = r.id \
         WHERE mr.server_id = $1 AND mr.user_id = $2",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_one(&state.db.pg)
    .await?;

    Ok(highest.unwrap_or(0))
}

// ─── Handlers ───────────────────────────────────────────────────

async fn get_roles(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_member(&state, server_id, auth.id).await?;

    let rows = sqlx::query_as::<_, (i64, i64, String, i32, bool, Option<String>, i32, i64, bool, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, server_id, name, color, hoist, icon, position, permissions, mentionable, is_default, created_at \
         FROM roles WHERE server_id = $1 ORDER BY position ASC",
    )
    .bind(server_id)
    .fetch_all(&state.db.pg)
    .await?;

    let roles: Vec<RoleResponse> = rows
        .into_iter()
        .map(|r| RoleResponse {
            id: Snowflake::from(r.0),
            server_id: Snowflake::from(r.1),
            name: r.2,
            color: r.3,
            hoist: r.4,
            icon: r.5,
            position: r.6,
            permissions: r.7.to_string(),
            mentionable: r.8,
            is_default: r.9,
            created_at: r.10,
        })
        .collect();

    Ok(Json(roles))
}

#[derive(Debug, Deserialize, Validate)]
pub struct CreateRoleRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,
    pub color: Option<i32>,
    pub hoist: Option<bool>,
    pub mentionable: Option<bool>,
    pub permissions: Option<String>,
}

async fn create_role(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Json(body): Json<CreateRoleRequest>,
) -> Result<impl IntoResponse, AppError> {
    if let Err(errors) = body.validate() {
        return Err(AppError::Validation(crate::routes::validation_errors(errors)));
    }

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_ROLES).await?;

    let role_id = state.snowflake.next_id();

    // Get next position
    let max_pos = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT MAX(position) FROM roles WHERE server_id = $1",
    )
    .bind(server_id)
    .fetch_one(&state.db.pg)
    .await?;
    let position = max_pos.unwrap_or(0) + 1;

    let permissions: i64 = body
        .permissions
        .as_ref()
        .map(|s| s.parse::<i64>())
        .transpose()
        .map_err(|_| AppError::BadRequest("Invalid permissions value".into()))?
        .unwrap_or(0);

    // Escalation prevention: cannot grant permissions you don't have
    if permissions != 0 {
        let actor_perms = get_user_permissions(&state, server_id, auth.id).await?;
        let new_bits = permissions as u64;
        let forbidden_bits = new_bits & !actor_perms.bits();
        if forbidden_bits != 0 {
            return Err(AppError::Forbidden("Cannot grant permissions you don't have".into()));
        }
    }

    sqlx::query(
        "INSERT INTO roles (id, server_id, name, color, hoist, position, permissions, mentionable) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(role_id)
    .bind(server_id)
    .bind(body.name.trim())
    .bind(body.color.unwrap_or(0))
    .bind(body.hoist.unwrap_or(false))
    .bind(position)
    .bind(permissions)
    .bind(body.mentionable.unwrap_or(false))
    .execute(&state.db.pg)
    .await?;

    let event = serde_json::json!({
        "type": "GUILD_ROLE_CREATE",
        "server_id": server_id,
        "role_id": role_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    let role = fetch_role(&state, role_id.value() as i64).await?;
    Ok((StatusCode::CREATED, Json(role)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleRequest {
    pub name: Option<String>,
    pub color: Option<i32>,
    pub hoist: Option<bool>,
    pub icon: Option<Option<String>>,
    pub mentionable: Option<bool>,
    pub permissions: Option<String>,
    pub position: Option<i32>,
}

async fn update_role(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, role_id)): Path<(i64, i64)>,
    Json(body): Json<UpdateRoleRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_ROLES).await?;

    // Check role hierarchy — cannot modify roles at or above your position
    let actor_highest = get_actor_highest_position(&state, server_id, auth.id).await?;
    let role_position = sqlx::query_scalar::<_, i32>(
        "SELECT position FROM roles WHERE id = $1 AND server_id = $2",
    )
    .bind(role_id)
    .bind(server_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Role not found".into()))?;

    if role_position >= actor_highest && actor_highest != i32::MAX {
        return Err(AppError::Forbidden(
            "Cannot modify roles at or above your highest role position".into(),
        ));
    }

    // Build dynamic update
    let mut sets = Vec::new();
    let mut param_idx = 3u32; // $1 = role_id, $2 = server_id

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
    add_set!(color, "color");
    add_set!(hoist, "hoist");
    add_set!(icon, "icon");
    add_set!(mentionable, "mentionable");
    add_set!(permissions, "permissions");
    add_set!(position, "position");

    if sets.is_empty() {
        return Err(AppError::BadRequest("No fields to update".into()));
    }

    let sql = format!(
        "UPDATE roles SET {} WHERE id = $1 AND server_id = $2",
        sets.join(", ")
    );
    let mut query = sqlx::query(&sql).bind(role_id).bind(server_id);

    // Validate and check permission escalation before binding
    let parsed_permissions: Option<i64> = if let Some(ref v) = body.permissions {
        let bits: i64 = v.parse::<i64>()
            .map_err(|_| AppError::BadRequest("Invalid permissions value".into()))?;
        let actor_perms = get_user_permissions(&state, server_id, auth.id).await?;
        let forbidden_bits = (bits as u64) & !actor_perms.bits();
        if forbidden_bits != 0 {
            return Err(AppError::Forbidden("Cannot grant permissions you don't have".into()));
        }
        Some(bits)
    } else {
        None
    };

    if let Some(ref v) = body.name { query = query.bind(v.trim()); }
    if let Some(v) = body.color { query = query.bind(v); }
    if let Some(v) = body.hoist { query = query.bind(v); }
    if let Some(ref v) = body.icon { query = query.bind(v.as_deref()); }
    if let Some(v) = body.mentionable { query = query.bind(v); }
    if let Some(bits) = parsed_permissions {
        query = query.bind(bits);
    }
    if let Some(v) = body.position { query = query.bind(v); }

    query.execute(&state.db.pg).await?;

    let event = serde_json::json!({
        "type": "GUILD_ROLE_UPDATE",
        "server_id": server_id,
        "role_id": role_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    let role = fetch_role(&state, role_id).await?;
    Ok(Json(role))
}

async fn delete_role(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, role_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_ROLES).await?;

    // Cannot delete @everyone role
    let is_default = sqlx::query_scalar::<_, bool>(
        "SELECT is_default FROM roles WHERE id = $1 AND server_id = $2",
    )
    .bind(role_id)
    .bind(server_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Role not found".into()))?;

    if is_default {
        return Err(AppError::BadRequest("Cannot delete the @everyone role".into()));
    }

    // Check hierarchy
    let actor_highest = get_actor_highest_position(&state, server_id, auth.id).await?;
    let role_position = sqlx::query_scalar::<_, i32>(
        "SELECT position FROM roles WHERE id = $1",
    )
    .bind(role_id)
    .fetch_one(&state.db.pg)
    .await?;

    if role_position >= actor_highest && actor_highest != i32::MAX {
        return Err(AppError::Forbidden(
            "Cannot delete roles at or above your highest role position".into(),
        ));
    }

    sqlx::query("DELETE FROM roles WHERE id = $1 AND server_id = $2")
        .bind(role_id)
        .bind(server_id)
        .execute(&state.db.pg)
        .await?;

    let event = serde_json::json!({
        "type": "GUILD_ROLE_DELETE",
        "server_id": server_id,
        "role_id": role_id,
    });
    let _ = state.nats.publish(&format!("server.{}.events", server_id), &event).await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct RolePositionUpdate {
    pub id: i64,
    pub position: i32,
}

async fn reorder_roles(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Json(body): Json<Vec<RolePositionUpdate>>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_ROLES).await?;

    // Verify all roles being repositioned are below the actor's highest role position
    let actor_highest = get_actor_highest_position(&state, server_id, auth.id).await?;
    if actor_highest != i32::MAX {
        for update in &body {
            let role_position = sqlx::query_scalar::<_, i32>(
                "SELECT position FROM roles WHERE id = $1 AND server_id = $2",
            )
            .bind(update.id)
            .bind(server_id)
            .fetch_optional(&state.db.pg)
            .await?
            .ok_or_else(|| AppError::NotFound("Role not found".into()))?;

            if role_position >= actor_highest {
                return Err(AppError::Forbidden(
                    "Cannot reorder roles at or above your highest role position".into(),
                ));
            }
        }
    }

    for update in &body {
        sqlx::query("UPDATE roles SET position = $1 WHERE id = $2 AND server_id = $3")
            .bind(update.position)
            .bind(update.id)
            .bind(server_id)
            .execute(&state.db.pg)
            .await?;
    }

    // Return all roles
    get_roles(State(state), auth, Path(server_id)).await
}

// ─── Channel Permission Overrides ───────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetOverrideRequest {
    #[serde(rename = "type")]
    pub target_type: i16,
    pub allow: String,
    pub deny: String,
}

async fn set_channel_override(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, override_id)): Path<(i64, i64)>,
    Json(body): Json<SetOverrideRequest>,
) -> Result<impl IntoResponse, AppError> {
    let server_id = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT server_id FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Channel not found".into()))?
    .ok_or_else(|| AppError::BadRequest("Cannot set overrides on DM channels".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_ROLES).await?;

    let allow_bits: i64 = body.allow.parse::<i64>()
        .map_err(|_| AppError::BadRequest("Invalid allow permissions value".into()))?;
    let deny_bits: i64 = body.deny.parse::<i64>()
        .map_err(|_| AppError::BadRequest("Invalid deny permissions value".into()))?;

    // Strip ADMINISTRATOR bit from overrides
    let allow_bits = allow_bits & !1;
    let deny_bits = deny_bits & !1;

    // Validate bits against actor's permissions
    let actor_perms = get_user_permissions(&state, server_id, auth.id).await?;
    let all_bits = (allow_bits as u64) | (deny_bits as u64);
    let forbidden_bits = all_bits & !actor_perms.bits();
    if forbidden_bits != 0 {
        return Err(AppError::Forbidden("Cannot set permissions you don't have".into()));
    }

    sqlx::query(
        "INSERT INTO permission_overrides (channel_id, target_id, target_type, allow_bits, deny_bits) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (channel_id, target_id) DO UPDATE SET target_type = $3, allow_bits = $4, deny_bits = $5",
    )
    .bind(channel_id)
    .bind(override_id)
    .bind(body.target_type)
    .bind(allow_bits)
    .bind(deny_bits)
    .execute(&state.db.pg)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

async fn delete_channel_override(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, override_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    let server_id = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT server_id FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Channel not found".into()))?
    .ok_or_else(|| AppError::BadRequest("Cannot modify DM channels".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_ROLES).await?;

    sqlx::query("DELETE FROM permission_overrides WHERE channel_id = $1 AND target_id = $2")
        .bind(channel_id)
        .bind(override_id)
        .execute(&state.db.pg)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
