use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, put},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_models::{error::AppError, snowflake::Snowflake};
use lumiere_permissions::Permissions;
use scylla::frame::value::CqlTimestamp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use super::messages::{check_channel_permission, scylla_err};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{channel_id}/messages/{message_id}/reactions/{emoji}/@me",
            put(add_reaction),
        )
        .route(
            "/{channel_id}/messages/{message_id}/reactions/{emoji}/@me",
            delete(remove_own_reaction),
        )
        .route(
            "/{channel_id}/messages/{message_id}/reactions/{emoji}/{user_id}",
            delete(remove_user_reaction),
        )
        .route(
            "/{channel_id}/messages/{message_id}/reactions/{emoji}",
            get(get_reactors),
        )
        .route(
            "/{channel_id}/messages/{message_id}/reactions/{emoji}",
            delete(remove_all_emoji_reactions),
        )
        .route(
            "/{channel_id}/messages/{message_id}/reactions",
            delete(remove_all_reactions),
        )
}

#[derive(Debug, Serialize)]
pub struct ReactionUser {
    pub id: Snowflake,
    pub username: String,
    pub avatar: Option<String>,
}

async fn add_reaction(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id, emoji)): Path<(i64, i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    if emoji.is_empty() || emoji.len() > 100 {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }
    check_channel_permission(&state, channel_id, auth.id, Permissions::ADD_REACTIONS).await?;

    let now = CqlTimestamp(chrono::Utc::now().timestamp_millis());
    state.db.scylla.execute_unpaged(
        &state.db.prepared().insert_reaction,
        (channel_id, message_id, emoji.as_str(), auth.id.value() as i64, now),
    )
    .await
    .map_err(scylla_err)?;

    let event = serde_json::json!({
        "type": "MESSAGE_REACTION_ADD",
        "channel_id": channel_id,
        "message_id": message_id,
        "user_id": auth.id,
        "emoji": emoji,
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn remove_own_reaction(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id, emoji)): Path<(i64, i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    if emoji.is_empty() || emoji.len() > 100 {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }
    check_channel_permission(&state, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;

    state.db.scylla.execute_unpaged(
        &state.db.prepared().delete_reaction,
        (channel_id, message_id, emoji.as_str(), auth.id.value() as i64),
    )
    .await
    .map_err(scylla_err)?;

    let event = serde_json::json!({
        "type": "MESSAGE_REACTION_REMOVE",
        "channel_id": channel_id,
        "message_id": message_id,
        "user_id": auth.id,
        "emoji": emoji,
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn remove_user_reaction(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id, emoji, user_id)): Path<(i64, i64, String, i64)>,
) -> Result<impl IntoResponse, AppError> {
    if emoji.is_empty() || emoji.len() > 100 {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }
    check_channel_permission(&state, channel_id, auth.id, Permissions::MANAGE_MESSAGES).await?;

    state.db.scylla.execute_unpaged(
        &state.db.prepared().delete_reaction,
        (channel_id, message_id, emoji.as_str(), user_id),
    )
    .await
    .map_err(scylla_err)?;

    let event = serde_json::json!({
        "type": "MESSAGE_REACTION_REMOVE",
        "channel_id": channel_id,
        "message_id": message_id,
        "user_id": user_id,
        "emoji": emoji,
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GetReactorsQuery {
    pub limit: Option<i32>,
    pub after: Option<i64>,
}

async fn get_reactors(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id, emoji)): Path<(i64, i64, String)>,
    Query(query): Query<GetReactorsQuery>,
) -> Result<impl IntoResponse, AppError> {
    if emoji.is_empty() || emoji.len() > 100 {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }
    check_channel_permission(&state, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;

    let limit = query.limit.unwrap_or(25).clamp(1, 100);

    let qr = if let Some(after_id) = query.after {
        state.db.scylla.execute_unpaged(
            &state.db.prepared().get_reactors_after,
            (channel_id, message_id, emoji.as_str(), after_id, limit),
        )
        .await
        .map_err(scylla_err)?
    } else {
        state.db.scylla.execute_unpaged(
            &state.db.prepared().get_reactors,
            (channel_id, message_id, emoji.as_str(), limit),
        )
        .await
        .map_err(scylla_err)?
    };

    let mut user_ids = Vec::new();
    if let Ok(rows_result) = qr.into_rows_result() {
        if let Ok(iter) = rows_result.rows::<(i64,)>() {
            for row in iter.flatten() {
                user_ids.push(row.0);
            }
        }
    }

    if user_ids.is_empty() {
        return Ok(Json(Vec::<ReactionUser>::new()));
    }

    // Batch query users instead of N+1
    let rows = sqlx::query_as::<_, (i64, String, Option<String>)>(
        "SELECT id, username, avatar FROM users WHERE id = ANY($1) AND deleted_at IS NULL",
    )
    .bind(&user_ids)
    .fetch_all(&state.db.pg)
    .await?;

    // Preserve ordering from ScyllaDB result
    let user_map: HashMap<i64, (String, Option<String>)> = rows
        .into_iter()
        .map(|(id, username, avatar)| (id, (username, avatar)))
        .collect();

    let users: Vec<ReactionUser> = user_ids
        .iter()
        .filter_map(|uid| {
            user_map.get(uid).map(|(username, avatar)| ReactionUser {
                id: Snowflake::from(*uid),
                username: username.clone(),
                avatar: avatar.clone(),
            })
        })
        .collect();

    Ok(Json(users))
}

async fn remove_all_reactions(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::MANAGE_MESSAGES).await?;

    state.db.scylla.execute_unpaged(
        &state.db.prepared().delete_all_reactions,
        (channel_id, message_id),
    )
    .await
    .map_err(scylla_err)?;

    let event = serde_json::json!({
        "type": "MESSAGE_REACTION_REMOVE_ALL",
        "channel_id": channel_id,
        "message_id": message_id,
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn remove_all_emoji_reactions(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id, emoji)): Path<(i64, i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    if emoji.is_empty() || emoji.len() > 100 {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }
    check_channel_permission(&state, channel_id, auth.id, Permissions::MANAGE_MESSAGES).await?;

    state.db.scylla.execute_unpaged(
        &state.db.prepared().delete_emoji_reactions,
        (channel_id, message_id, emoji.as_str()),
    )
    .await
    .map_err(scylla_err)?;

    let event = serde_json::json!({
        "type": "MESSAGE_REACTION_REMOVE_EMOJI",
        "channel_id": channel_id,
        "message_id": message_id,
        "emoji": emoji,
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }

    Ok(StatusCode::NO_CONTENT)
}
