use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_models::{error::AppError, snowflake::Snowflake};
use lumiere_permissions::Permissions;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use super::servers::require_permissions;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Channel webhooks
        .route("/{channel_id}/webhooks", post(create_webhook))
        .route("/{channel_id}/webhooks", get(get_channel_webhooks))
}

pub fn webhook_exec_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{webhook_id}/{token}", post(execute_webhook))
        .route("/{webhook_id}", get(get_webhook))
        .route("/{webhook_id}", patch(update_webhook))
        .route("/{webhook_id}", delete(delete_webhook))
}

pub fn applications_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(create_application))
        .route("/@me", get(get_my_applications))
        .route("/{app_id}", patch(update_application))
        .route("/{app_id}/bot/reset-token", post(reset_bot_token))
        .route("/{app_id}/commands", post(create_command))
        .route("/{app_id}/commands", get(get_commands))
        .route("/{app_id}/commands/{command_id}", patch(update_command))
        .route("/{app_id}/commands/{command_id}", delete(delete_command))
}

// ─── Types ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub name: String,
    pub avatar: Option<String>,
    pub token: Option<String>,
    #[serde(rename = "type")]
    pub webhook_type: i16,
}

#[derive(Debug, Serialize)]
pub struct ApplicationResponse {
    pub id: Snowflake,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub bot_id: Option<Snowflake>,
}

#[derive(Debug, Serialize)]
pub struct CommandResponse {
    pub id: Snowflake,
    pub application_id: Snowflake,
    pub name: String,
    pub description: String,
    pub options: serde_json::Value,
}

// ─── Webhooks ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub name: String,
    pub avatar: Option<String>,
}

async fn create_webhook(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<CreateWebhookRequest>,
) -> Result<impl IntoResponse, AppError> {
    let server_id =
        sqlx::query_scalar::<_, Option<i64>>("SELECT server_id FROM channels WHERE id = $1")
            .bind(channel_id)
            .fetch_optional(&state.db.pg)
            .await?
            .ok_or_else(|| AppError::NotFound("Channel not found".into()))?
            .ok_or_else(|| AppError::BadRequest("Cannot create webhooks for DMs".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_WEBHOOKS).await?;

    let webhook_id = state.snowflake.next_id();
    let token = nanoid::nanoid!(68);

    // Fix #21: Hash webhook token before storing
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));

    sqlx::query(
        "INSERT INTO webhooks (id, server_id, channel_id, creator_id, name, token, type) \
         VALUES ($1, $2, $3, $4, $5, $6, 1)",
    )
    .bind(webhook_id)
    .bind(server_id)
    .bind(channel_id)
    .bind(auth.id)
    .bind(&body.name)
    .bind(&token_hash)
    .execute(&state.db.pg)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(WebhookResponse {
            id: webhook_id,
            channel_id: Snowflake::from(channel_id),
            name: body.name,
            avatar: body.avatar,
            token: Some(token),
            webhook_type: 1,
        }),
    ))
}

async fn get_channel_webhooks(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let server_id =
        sqlx::query_scalar::<_, Option<i64>>("SELECT server_id FROM channels WHERE id = $1")
            .bind(channel_id)
            .fetch_optional(&state.db.pg)
            .await?
            .flatten()
            .ok_or_else(|| AppError::NotFound("Channel not found".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_WEBHOOKS).await?;

    let rows = sqlx::query_as::<_, (i64, i64, String, Option<String>, i16)>(
        "SELECT id, channel_id, name, avatar, type FROM webhooks WHERE channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(&state.db.pg)
    .await?;

    let webhooks: Vec<WebhookResponse> = rows
        .into_iter()
        .map(|r| WebhookResponse {
            id: Snowflake::from(r.0),
            channel_id: Snowflake::from(r.1),
            name: r.2,
            avatar: r.3,
            token: None,
            webhook_type: r.4,
        })
        .collect();

    Ok(Json(webhooks))
}

// Fix #19: Add permission checks to get_webhook
async fn get_webhook(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(webhook_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    // Look up the webhook's server_id and require MANAGE_WEBHOOKS
    let server_id = sqlx::query_scalar::<_, i64>("SELECT server_id FROM webhooks WHERE id = $1")
        .bind(webhook_id)
        .fetch_optional(&state.db.pg)
        .await?
        .ok_or_else(|| AppError::NotFound("Webhook not found".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_WEBHOOKS).await?;

    let row = sqlx::query_as::<_, (i64, i64, String, Option<String>, i16)>(
        "SELECT id, channel_id, name, avatar, type FROM webhooks WHERE id = $1",
    )
    .bind(webhook_id)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::NotFound("Webhook not found".into()))?;

    Ok(Json(WebhookResponse {
        id: Snowflake::from(row.0),
        channel_id: Snowflake::from(row.1),
        name: row.2,
        avatar: row.3,
        token: None,
        webhook_type: row.4,
    }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateWebhookRequest {
    pub name: Option<String>,
    pub avatar: Option<Option<String>>,
    pub channel_id: Option<i64>,
}

// Fix #19: Add permission checks to update_webhook
async fn update_webhook(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(webhook_id): Path<i64>,
    Json(body): Json<UpdateWebhookRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Look up the webhook's server_id and require MANAGE_WEBHOOKS
    let server_id = sqlx::query_scalar::<_, i64>("SELECT server_id FROM webhooks WHERE id = $1")
        .bind(webhook_id)
        .fetch_optional(&state.db.pg)
        .await?
        .ok_or_else(|| AppError::NotFound("Webhook not found".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_WEBHOOKS).await?;

    if let Some(ref name) = body.name {
        sqlx::query("UPDATE webhooks SET name = $1 WHERE id = $2")
            .bind(name)
            .bind(webhook_id)
            .execute(&state.db.pg)
            .await?;
    }
    if let Some(ref avatar) = body.avatar {
        sqlx::query("UPDATE webhooks SET avatar = $1 WHERE id = $2")
            .bind(avatar.as_deref())
            .bind(webhook_id)
            .execute(&state.db.pg)
            .await?;
    }
    if let Some(channel_id) = body.channel_id {
        sqlx::query("UPDATE webhooks SET channel_id = $1 WHERE id = $2")
            .bind(channel_id)
            .bind(webhook_id)
            .execute(&state.db.pg)
            .await?;
    }

    get_webhook(State(state), auth, Path(webhook_id)).await
}

// Fix #19: Add permission checks to delete_webhook
async fn delete_webhook(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(webhook_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    // Look up the webhook's server_id and require MANAGE_WEBHOOKS
    let server_id = sqlx::query_scalar::<_, i64>("SELECT server_id FROM webhooks WHERE id = $1")
        .bind(webhook_id)
        .fetch_optional(&state.db.pg)
        .await?
        .ok_or_else(|| AppError::NotFound("Webhook not found".into()))?;

    require_permissions(&state, server_id, auth.id, Permissions::MANAGE_WEBHOOKS).await?;

    sqlx::query("DELETE FROM webhooks WHERE id = $1")
        .bind(webhook_id)
        .execute(&state.db.pg)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Execute webhook — no auth required, token in URL
async fn execute_webhook(
    State(state): State<Arc<AppState>>,
    Path((webhook_id, token)): Path<(i64, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, AppError> {
    // Fix #23: Validate content
    let content = body["content"].as_str().unwrap_or("");
    if content.is_empty() {
        return Err(AppError::BadRequest("Content must not be empty".into()));
    }
    if content.chars().count() > 4000 {
        return Err(AppError::BadRequest(
            "Content must be 4000 characters or less".into(),
        ));
    }

    // Fix #21: Hash the incoming token and compare against stored hash
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));

    let row = sqlx::query_as::<_, (i64, String)>(
        "SELECT channel_id, name FROM webhooks WHERE id = $1 AND token = $2",
    )
    .bind(webhook_id)
    .bind(&token_hash)
    .fetch_optional(&state.db.pg)
    .await?
    .ok_or_else(|| AppError::Unauthorized("Invalid webhook token".into()))?;

    let channel_id = row.0;
    let webhook_name = row.1;

    // Create message via webhook
    let message_id = state.snowflake.next_id();
    let b = lumiere_models::bucket::bucket_from_snowflake(message_id);

    state
        .db
        .scylla
        .execute_unpaged(
            &state.db.prepared().insert_webhook_message,
            (
                channel_id,
                b,
                message_id.value() as i64,
                webhook_id,
                Some(content),
            ),
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("ScyllaDB error: {}", e)))?;

    // Fix #23: Include full message payload in NATS event
    let event = serde_json::json!({
        "type": "MESSAGE_CREATE",
        "channel_id": channel_id,
        "webhook_id": webhook_id,
        "message": {
            "id": message_id,
            "channel_id": channel_id,
            "content": content,
            "author": {
                "id": webhook_id,
                "username": webhook_name,
                "bot": true,
            },
            "webhook_id": webhook_id,
        },
    });
    let _ = state
        .nats
        .publish(&format!("channel.{}.messages", channel_id), &event)
        .await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Applications / Bot Framework ───────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateApplicationRequest {
    pub name: String,
    pub description: Option<String>,
}

async fn create_application(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateApplicationRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app_id = state.snowflake.next_id();

    // Create bot user
    let bot_user_id = state.snowflake.next_id();
    let bot_token = format!(
        "Bot {}.{}",
        base64_encode(bot_user_id.value()),
        nanoid::nanoid!(32)
    );

    // Fix #24: Use SHA-256 instead of Argon2 for bot token hashing
    let token_hash = hex::encode(Sha256::digest(bot_token.as_bytes()));

    // Fix #22: Wrap bot creation in a transaction
    let mut tx = state.db.pg.begin().await?;

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, is_bot) VALUES ($1, $2, $3, $4, true)",
    )
    .bind(bot_user_id)
    .bind(format!("{} Bot", &body.name))
    .bind(format!("bot+{}@lumiere.internal", app_id))
    .bind(&token_hash)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO applications (id, owner_id, name, description, bot_id, bot_token_hash) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(app_id)
    .bind(auth.id)
    .bind(&body.name)
    .bind(&body.description)
    .bind(bot_user_id)
    .bind(&token_hash)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": app_id,
            "name": body.name,
            "bot_id": bot_user_id,
            "bot_token": bot_token,
        })),
    ))
}

fn base64_encode(value: u64) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(value.to_string())
}

async fn get_my_applications(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, Option<String>, Option<i64>)>(
        "SELECT id, name, description, icon, bot_id FROM applications WHERE owner_id = $1",
    )
    .bind(auth.id)
    .fetch_all(&state.db.pg)
    .await?;

    let apps: Vec<ApplicationResponse> = rows
        .into_iter()
        .map(|r| ApplicationResponse {
            id: Snowflake::from(r.0),
            name: r.1,
            description: r.2,
            icon: r.3,
            bot_id: r.4.map(Snowflake::from),
        })
        .collect();

    Ok(Json(apps))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct UpdateApplicationRequest {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub icon: Option<Option<String>>,
}

async fn update_application(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(app_id): Path<i64>,
    Json(body): Json<UpdateApplicationRequest>,
) -> Result<impl IntoResponse, AppError> {
    if let Some(ref name) = body.name {
        sqlx::query("UPDATE applications SET name = $1 WHERE id = $2 AND owner_id = $3")
            .bind(name)
            .bind(app_id)
            .bind(auth.id)
            .execute(&state.db.pg)
            .await?;
    }
    if let Some(ref desc) = body.description {
        sqlx::query("UPDATE applications SET description = $1 WHERE id = $2 AND owner_id = $3")
            .bind(desc.as_deref())
            .bind(app_id)
            .bind(auth.id)
            .execute(&state.db.pg)
            .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn reset_bot_token(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(app_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let bot_id = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT bot_id FROM applications WHERE id = $1 AND owner_id = $2",
    )
    .bind(app_id)
    .bind(auth.id)
    .fetch_optional(&state.db.pg)
    .await?
    .flatten()
    .ok_or_else(|| AppError::NotFound("Application not found".into()))?;

    let new_token = format!(
        "Bot {}.{}",
        base64_encode(bot_id as u64),
        nanoid::nanoid!(32)
    );

    // Fix #24: Use SHA-256 instead of Argon2 for bot token hashing
    let token_hash = hex::encode(Sha256::digest(new_token.as_bytes()));

    sqlx::query("UPDATE applications SET bot_token_hash = $1 WHERE id = $2")
        .bind(&token_hash)
        .bind(app_id)
        .execute(&state.db.pg)
        .await?;

    Ok(Json(serde_json::json!({ "token": new_token })))
}

// ─── Slash Commands ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateCommandRequest {
    pub name: String,
    pub description: String,
    pub options: Option<serde_json::Value>,
    pub server_id: Option<i64>,
}

async fn create_command(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(app_id): Path<i64>,
    Json(body): Json<CreateCommandRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate command name length (Discord-style: 1–32 characters)
    if body.name.is_empty() || body.name.len() > 32 {
        return Err(AppError::BadRequest(
            "Command name must be between 1 and 32 characters".into(),
        ));
    }
    if body.description.len() > 100 {
        return Err(AppError::BadRequest(
            "Command description must be 100 characters or less".into(),
        ));
    }

    // Verify ownership
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM applications WHERE id = $1 AND owner_id = $2)",
    )
    .bind(app_id)
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    if !exists {
        return Err(AppError::Forbidden("Not the application owner".into()));
    }

    let command_id = state.snowflake.next_id();

    sqlx::query(
        "INSERT INTO application_commands (id, application_id, server_id, name, description, options) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(command_id)
    .bind(app_id)
    .bind(body.server_id)
    .bind(&body.name)
    .bind(&body.description)
    .bind(body.options.as_ref().unwrap_or(&serde_json::json!([])))
    .execute(&state.db.pg)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(CommandResponse {
            id: command_id,
            application_id: Snowflake::from(app_id),
            name: body.name,
            description: body.description,
            options: body.options.unwrap_or(serde_json::json!([])),
        }),
    ))
}

async fn get_commands(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(app_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let rows = sqlx::query_as::<_, (i64, i64, String, String, serde_json::Value)>(
        "SELECT id, application_id, name, description, options FROM application_commands WHERE application_id = $1",
    )
    .bind(app_id)
    .fetch_all(&state.db.pg)
    .await?;

    let commands: Vec<CommandResponse> = rows
        .into_iter()
        .map(|r| CommandResponse {
            id: Snowflake::from(r.0),
            application_id: Snowflake::from(r.1),
            name: r.2,
            description: r.3,
            options: r.4,
        })
        .collect();

    Ok(Json(commands))
}

// Fix #20: Add ownership check to update_command
async fn update_command(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((app_id, command_id)): Path<(i64, i64)>,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, AppError> {
    // Verify ownership
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM applications WHERE id = $1 AND owner_id = $2)",
    )
    .bind(app_id)
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    if !exists {
        return Err(AppError::Forbidden("Not the application owner".into()));
    }

    if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
        sqlx::query(
            "UPDATE application_commands SET name = $1 WHERE id = $2 AND application_id = $3",
        )
        .bind(name)
        .bind(command_id)
        .bind(app_id)
        .execute(&state.db.pg)
        .await?;
    }
    if let Some(desc) = body.get("description").and_then(|v| v.as_str()) {
        sqlx::query("UPDATE application_commands SET description = $1 WHERE id = $2 AND application_id = $3")
            .bind(desc).bind(command_id).bind(app_id).execute(&state.db.pg).await?;
    }
    if let Some(options) = body.get("options") {
        sqlx::query(
            "UPDATE application_commands SET options = $1 WHERE id = $2 AND application_id = $3",
        )
        .bind(options)
        .bind(command_id)
        .bind(app_id)
        .execute(&state.db.pg)
        .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

// Fix #20: Add ownership check to delete_command
async fn delete_command(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((app_id, command_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    // Verify ownership
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM applications WHERE id = $1 AND owner_id = $2)",
    )
    .bind(app_id)
    .bind(auth.id)
    .fetch_one(&state.db.pg)
    .await?;

    if !exists {
        return Err(AppError::Forbidden("Not the application owner".into()));
    }

    sqlx::query("DELETE FROM application_commands WHERE id = $1 AND application_id = $2")
        .bind(command_id)
        .bind(app_id)
        .execute(&state.db.pg)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
