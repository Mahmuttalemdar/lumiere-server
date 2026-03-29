use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_models::{error::AppError, snowflake::Snowflake};
use lumiere_permissions::Permissions;
use scylla::frame::value::CqlTimestamp;
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use super::messages::check_channel_permission;
use super::servers::require_member;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{channel_id}/typing", post(send_typing))
        .route("/{channel_id}/messages/{message_id}/ack", post(ack_message))
}

pub fn user_unread_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/@me/unread", get(get_unread))
}

pub fn server_ack_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{server_id}/ack", post(ack_server))
}

// ─── Typing Indicator ───────────────────────────────────────────

async fn send_typing(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let server_id = check_channel_permission(&state, channel_id, auth.id, Permissions::SEND_MESSAGES).await?;

    // Rate limit: 1 per 10 seconds
    let mut conn = state.redis.clone();
    let key = format!("typing:{}:{}", channel_id, auth.id);
    let set: bool = redis::cmd("SET")
        .arg(&key)
        .arg("1")
        .arg("EX")
        .arg(10i64)
        .arg("NX")
        .query_async(&mut conn)
        .await
        .unwrap_or(false);

    if !set {
        // Already typing, skip broadcast but return success
        return Ok(StatusCode::NO_CONTENT);
    }

    // Broadcast TYPING_START
    let event = serde_json::json!({
        "type": "TYPING_START",
        "channel_id": channel_id,
        "user_id": auth.id,
        "timestamp": chrono::Utc::now().timestamp(),
    });
    // Publish to channel subject (for DM subscriptions)
    if let Err(e) = state.nats.publish(&format!("channel.{}.typing", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }
    // Also publish to server subject for gateway subscribers
    if let Some(sid) = server_id {
        if let Err(e) = state.nats.publish(&format!("server.{}.events", sid), &event).await {
            tracing::warn!(error = %e, "Failed to publish server NATS event");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── Read States ────────────────────────────────────────────────

async fn ack_message(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;

    let now = CqlTimestamp(chrono::Utc::now().timestamp_millis());

    state.db.scylla.execute_unpaged(
        &state.db.prepared().upsert_read_state,
        (auth.id.value() as i64, channel_id, message_id, now),
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("ScyllaDB error: {}", e)))?;

    let event = serde_json::json!({
        "type": "MESSAGE_ACK",
        "channel_id": channel_id,
        "message_id": message_id,
    });
    if let Err(e) = state.nats.publish(&format!("user.{}.ack", auth.id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn ack_server(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(server_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_member(&state, server_id, auth.id).await?;

    // Get all channels in the server
    let channels = sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT id, last_message_id FROM channels WHERE server_id = $1",
    )
    .bind(server_id)
    .fetch_all(&state.db.pg)
    .await?;

    let now = CqlTimestamp(chrono::Utc::now().timestamp_millis());
    let user_id = auth.id.value() as i64;

    for (channel_id, last_msg) in channels {
        if let Some(last_message_id) = last_msg {
            let _ = state.db.scylla.execute_unpaged(
                &state.db.prepared().upsert_read_state,
                (user_id, channel_id, last_message_id, now),
            ).await;
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct UnreadInfo {
    pub channel_id: Snowflake,
    pub last_message_id: Option<Snowflake>,
    pub mention_count: i32,
}

async fn get_unread(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let user_id = auth.id.value() as i64;

    let qr = state.db.scylla.execute_unpaged(
        &state.db.prepared().get_unread,
        (user_id,),
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("ScyllaDB error: {}", e)))?;

    let mut unread = Vec::new();
    if let Ok(rows_result) = qr.into_rows_result() {
        if let Ok(iter) = rows_result.rows::<(i64, i64, i32)>() {
            for row in iter.flatten() {
                unread.push(UnreadInfo {
                    channel_id: Snowflake::from(row.0),
                    last_message_id: Some(Snowflake::from(row.1)),
                    mention_count: row.2,
                });
            }
        }
    }

    Ok(Json(unread))
}
