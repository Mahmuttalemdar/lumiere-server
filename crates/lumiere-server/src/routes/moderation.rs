use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_models::{error::AppError, snowflake::Snowflake};
use lumiere_permissions::Permissions;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::Validate;

use crate::AppState;
use super::servers::require_permissions;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Warnings
        .route("/{server_id}/members/{user_id}/warnings", post(create_warning))
        .route("/{server_id}/members/{user_id}/warnings", get(get_warnings))
        .route("/{server_id}/members/{user_id}/warnings/{warning_id}", delete(delete_warning))
        // Audit log
        .route("/{server_id}/audit-log", get(get_audit_log))
        // Auto-moderation
        .route("/{server_id}/auto-moderation/rules", post(create_auto_mod_rule))
        .route("/{server_id}/auto-moderation/rules", get(get_auto_mod_rules))
        .route("/{server_id}/auto-moderation/rules/{rule_id}", patch(update_auto_mod_rule))
        .route("/{server_id}/auto-moderation/rules/{rule_id}", delete(delete_auto_mod_rule))
}

pub fn report_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/report", post(create_report))
}

use axum::routing::patch;

// ─── Types ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct WarningResponse {
    pub id: Snowflake,
    pub server_id: Snowflake,
    pub user_id: Snowflake,
    pub moderator_id: Option<Snowflake>,
    pub reason: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct AuditLogEntry {
    pub id: Snowflake,
    pub user_id: Option<Snowflake>,
    pub target_id: Option<i64>,
    pub action_type: i16,
    pub changes: Option<serde_json::Value>,
    pub reason: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct AutoModRule {
    pub id: Snowflake,
    pub server_id: Snowflake,
    pub name: String,
    pub event_type: i16,
    pub trigger_type: i16,
    pub trigger_metadata: serde_json::Value,
    pub actions: serde_json::Value,
    pub enabled: bool,
}

// ─── Warnings ───────────────────────────────────────────────────

#[derive(Debug, Deserialize, Validate)]
pub struct CreateWarningRequest {
    #[validate(length(min = 1, max = 512))]
    pub reason: String,
}

async fn create_warning(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, user_id)): Path<(i64, i64)>,
    Json(body): Json<CreateWarningRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MODERATE_MEMBERS).await?;

    if let Err(errors) = body.validate() {
        return Err(AppError::Validation(crate::routes::validation_errors(errors)));
    }

    // Prevent self-warning
    if auth.id.value() as i64 == user_id {
        return Err(AppError::BadRequest("Cannot warn yourself".into()));
    }

    // Check target is a member of this server
    let is_member = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM server_members WHERE server_id = $1 AND user_id = $2)",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_one(&state.db.pg)
    .await?;
    if !is_member {
        return Err(AppError::NotFound("Member not found in this server".into()));
    }

    let warning_id = state.snowflake.next_id();

    sqlx::query(
        "INSERT INTO warnings (id, server_id, user_id, moderator_id, reason) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(warning_id)
    .bind(server_id)
    .bind(user_id)
    .bind(auth.id)
    .bind(&body.reason)
    .execute(&state.db.pg)
    .await?;

    // Audit log
    let audit_id = state.snowflake.next_id();
    sqlx::query(
        "INSERT INTO audit_log (id, server_id, user_id, target_id, action_type, reason) VALUES ($1, $2, $3, $4, 50, $5)",
    )
    .bind(audit_id)
    .bind(server_id)
    .bind(auth.id)
    .bind(user_id)
    .bind(&body.reason)
    .execute(&state.db.pg)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(WarningResponse {
            id: warning_id,
            server_id: Snowflake::from(server_id),
            user_id: Snowflake::from(user_id),
            moderator_id: Some(auth.id),
            reason: body.reason,
            created_at: chrono::Utc::now(),
        }),
    ))
}

async fn get_warnings(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, user_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MODERATE_MEMBERS).await?;

    let rows = sqlx::query_as::<_, (i64, i64, i64, Option<i64>, String, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, server_id, user_id, moderator_id, reason, created_at \
         FROM warnings WHERE server_id = $1 AND user_id = $2 ORDER BY created_at DESC",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_all(&state.db.pg)
    .await?;

    let warnings: Vec<WarningResponse> = rows.into_iter().map(|r| WarningResponse {
        id: Snowflake::from(r.0),
        server_id: Snowflake::from(r.1),
        user_id: Snowflake::from(r.2),
        moderator_id: r.3.map(Snowflake::from),
        reason: r.4,
        created_at: r.5,
    }).collect();

    Ok(Json(warnings))
}

async fn delete_warning(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, _user_id, warning_id)): Path<(i64, i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MODERATE_MEMBERS).await?;

    let result = sqlx::query("DELETE FROM warnings WHERE id = $1 AND server_id = $2")
        .bind(warning_id)
        .bind(server_id)
        .execute(&state.db.pg)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Warning not found".into()));
    }

    // Audit log
    let audit_id = state.snowflake.next_id();
    sqlx::query(
        "INSERT INTO audit_log (id, server_id, user_id, target_id, action_type, reason) VALUES ($1, $2, $3, $4, 51, $5)",
    )
    .bind(audit_id)
    .bind(server_id)
    .bind(auth.id)
    .bind(warning_id)
    .bind("Warning deleted")
    .execute(&state.db.pg)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Audit Log ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    pub user_id: Option<i64>,
    pub action_type: Option<i16>,
    pub before: Option<i64>,
    pub limit: Option<i64>,
}

async fn get_audit_log(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Query(query): Query<AuditLogQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::VIEW_AUDIT_LOG).await?;

    let limit = query.limit.unwrap_or(50).max(1).min(100);

    // Build dynamic query
    let mut sql = "SELECT id, user_id, target_id, action_type, changes, reason, created_at \
                   FROM audit_log WHERE server_id = $1".to_string();
    let mut param_idx = 2;

    if query.user_id.is_some() {
        sql.push_str(&format!(" AND user_id = ${param_idx}"));
        param_idx += 1;
    }
    if query.action_type.is_some() {
        sql.push_str(&format!(" AND action_type = ${param_idx}"));
        param_idx += 1;
    }
    if query.before.is_some() {
        sql.push_str(&format!(" AND id < ${param_idx}"));
        param_idx += 1;
    }
    let _ = param_idx;

    sql.push_str(" ORDER BY id DESC LIMIT ");
    sql.push_str(&limit.to_string());

    let mut q = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, i16, Option<serde_json::Value>, Option<String>, chrono::DateTime<chrono::Utc>)>(&sql)
        .bind(server_id);

    if let Some(uid) = query.user_id { q = q.bind(uid); }
    if let Some(at) = query.action_type { q = q.bind(at); }
    if let Some(before) = query.before { q = q.bind(before); }

    let rows = q.fetch_all(&state.db.pg).await?;

    let entries: Vec<AuditLogEntry> = rows.into_iter().map(|r| AuditLogEntry {
        id: Snowflake::from(r.0),
        user_id: r.1.map(Snowflake::from),
        target_id: r.2,
        action_type: r.3,
        changes: r.4,
        reason: r.5,
        created_at: r.6,
    }).collect();

    Ok(Json(entries))
}

// ─── Auto-Moderation ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateAutoModRequest {
    pub name: String,
    pub event_type: i16,
    pub trigger_type: i16,
    pub trigger_metadata: serde_json::Value,
    pub actions: serde_json::Value,
    pub enabled: Option<bool>,
}

async fn create_auto_mod_rule(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
    Json(body): Json<CreateAutoModRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_SERVER).await?;

    let rule_id = state.snowflake.next_id();

    sqlx::query(
        "INSERT INTO auto_mod_rules (id, server_id, name, event_type, trigger_type, trigger_metadata, actions, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(rule_id)
    .bind(server_id)
    .bind(&body.name)
    .bind(body.event_type)
    .bind(body.trigger_type)
    .bind(&body.trigger_metadata)
    .bind(&body.actions)
    .bind(body.enabled.unwrap_or(true))
    .execute(&state.db.pg)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(AutoModRule {
            id: rule_id,
            server_id: Snowflake::from(server_id),
            name: body.name,
            event_type: body.event_type,
            trigger_type: body.trigger_type,
            trigger_metadata: body.trigger_metadata,
            actions: body.actions,
            enabled: body.enabled.unwrap_or(true),
        }),
    ))
}

async fn get_auto_mod_rules(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_SERVER).await?;

    let rows = sqlx::query_as::<_, (i64, i64, String, i16, i16, serde_json::Value, serde_json::Value, bool)>(
        "SELECT id, server_id, name, event_type, trigger_type, trigger_metadata, actions, enabled \
         FROM auto_mod_rules WHERE server_id = $1",
    )
    .bind(server_id)
    .fetch_all(&state.db.pg)
    .await?;

    let rules: Vec<AutoModRule> = rows.into_iter().map(|r| AutoModRule {
        id: Snowflake::from(r.0), server_id: Snowflake::from(r.1), name: r.2,
        event_type: r.3, trigger_type: r.4, trigger_metadata: r.5, actions: r.6, enabled: r.7,
    }).collect();

    Ok(Json(rules))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAutoModRequest {
    pub name: Option<String>,
    pub trigger_metadata: Option<serde_json::Value>,
    pub actions: Option<serde_json::Value>,
    pub enabled: Option<bool>,
}

async fn update_auto_mod_rule(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, rule_id)): Path<(i64, i64)>,
    Json(body): Json<UpdateAutoModRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_SERVER).await?;

    let mut sets = Vec::new();
    let mut param_idx = 3u32;

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
    add_set!(trigger_metadata, "trigger_metadata");
    add_set!(actions, "actions");
    add_set!(enabled, "enabled");

    if sets.is_empty() {
        return Err(AppError::BadRequest("No fields to update".into()));
    }

    let sql = format!("UPDATE auto_mod_rules SET {} WHERE id = $1 AND server_id = $2", sets.join(", "));
    let mut q = sqlx::query(&sql).bind(rule_id).bind(server_id);
    if let Some(ref v) = body.name { q = q.bind(v.as_str()); }
    if let Some(ref v) = body.trigger_metadata { q = q.bind(v); }
    if let Some(ref v) = body.actions { q = q.bind(v); }
    if let Some(v) = body.enabled { q = q.bind(v); }
    q.execute(&state.db.pg).await?;

    Ok(StatusCode::NO_CONTENT)
}

async fn delete_auto_mod_rule(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((server_id, rule_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_SERVER).await?;

    sqlx::query("DELETE FROM auto_mod_rules WHERE id = $1 AND server_id = $2")
        .bind(rule_id)
        .bind(server_id)
        .execute(&state.db.pg)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Reports ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateReportRequest {
    pub target_type: String,
    pub target_id: i64,
    pub reason: i16,
    pub description: Option<String>,
}

async fn create_report(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateReportRequest>,
) -> Result<impl IntoResponse, AppError> {
    let valid_types = ["message", "user", "server"];
    if !valid_types.contains(&body.target_type.as_str()) {
        return Err(AppError::BadRequest("Invalid target_type. Must be: message, user, or server".into()));
    }

    let report_id = state.snowflake.next_id();

    sqlx::query(
        "INSERT INTO reports (id, reporter_id, target_type, target_id, reason, description) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(report_id)
    .bind(auth.id)
    .bind(&body.target_type)
    .bind(body.target_id)
    .bind(body.reason)
    .bind(&body.description)
    .execute(&state.db.pg)
    .await?;

    Ok(StatusCode::CREATED)
}
