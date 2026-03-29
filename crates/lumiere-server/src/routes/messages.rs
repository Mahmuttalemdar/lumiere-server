use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_models::{
    bucket,
    error::AppError,
    snowflake::Snowflake,
};
use lumiere_permissions::Permissions;
use scylla::frame::value::CqlTimestamp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use crate::AppState;
use super::servers::require_permissions;

static USER_MENTION_RE: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"<@(\d+)>").unwrap());
static ROLE_MENTION_RE: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"<@&(\d+)>").unwrap());

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{channel_id}/messages", get(get_messages))
        .route("/{channel_id}/messages", post(send_message))
        .route("/{channel_id}/messages/{message_id}", get(get_message))
        .route("/{channel_id}/messages/{message_id}", patch(edit_message))
        .route("/{channel_id}/messages/{message_id}", delete(delete_message))
        .route("/{channel_id}/messages/bulk-delete", post(bulk_delete))
        .route("/{channel_id}/pins", get(get_pins))
        .route("/{channel_id}/pins/{message_id}", put(pin_message))
        .route("/{channel_id}/pins/{message_id}", delete(unpin_message))
}

// ─── Types ──────────────────────────────────────────────────────

type MsgRow = (i64, i64, i64, Option<String>, i16, i64, Option<CqlTimestamp>, bool, bool, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, bool);

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub author_id: Snowflake,
    pub content: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub edited_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(rename = "type")]
    pub message_type: i16,
    pub flags: i64,
    pub pinned: bool,
    pub mention_everyone: bool,
    pub mentions: Vec<i64>,
    pub mention_roles: Vec<i64>,
    pub embeds: Option<serde_json::Value>,
    pub attachments: Option<serde_json::Value>,
    pub reference_id: Option<Snowflake>,
}

fn row_to_response(r: MsgRow) -> MessageResponse {
    let snowflake = Snowflake::from(r.0);
    MessageResponse {
        id: snowflake,
        channel_id: Snowflake::from(r.1),
        author_id: Snowflake::from(r.2),
        content: r.3,
        timestamp: snowflake.created_at(),
        edited_timestamp: r.6.map(|t| {
            chrono::DateTime::from_timestamp_millis(t.0).unwrap_or_default()
        }),
        message_type: r.4,
        flags: r.5,
        pinned: r.7,
        mention_everyone: r.8,
        mentions: r.9.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        mention_roles: r.10.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        embeds: r.11.and_then(|s| serde_json::from_str(&s).ok()),
        attachments: r.12.and_then(|s| serde_json::from_str(&s).ok()),
        reference_id: r.13.map(Snowflake::from),
    }
}

/// Extract typed rows from a ScyllaDB query result
fn extract_msg_rows(result: scylla::transport::query_result::QueryResult) -> Vec<MsgRow> {
    let mut out = Vec::new();
    if let Ok(rows_result) = result.into_rows_result() {
        if let Ok(typed_iter) = rows_result.rows::<MsgRow>() {
            for row in typed_iter {
                if let Ok(r) = row {
                    if !r.14 { // skip deleted
                        out.push(r);
                    }
                }
            }
        }
    }
    out
}

// ─── Helpers ────────────────────────────────────────────────────

async fn get_channel_server(state: &AppState, channel_id: i64) -> Result<Option<i64>, AppError> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT server_id FROM channels WHERE id = $1")
        .bind(channel_id)
        .fetch_optional(&state.db.pg)
        .await?
        .ok_or_else(|| AppError::NotFound("Channel not found".into()))
}

pub async fn check_channel_permission(
    state: &AppState,
    channel_id: i64,
    user_id: Snowflake,
    perm: Permissions,
) -> Result<Option<i64>, AppError> {
    let server_id = get_channel_server(state, channel_id).await?;
    match server_id {
        Some(sid) => {
            require_permissions(state, sid, user_id, perm).await?;
            Ok(Some(sid))
        }
        None => {
            let is_recipient = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM dm_recipients WHERE channel_id = $1 AND user_id = $2)",
            )
            .bind(channel_id)
            .bind(user_id)
            .fetch_one(&state.db.pg)
            .await?;
            if !is_recipient {
                return Err(AppError::Forbidden("Not a recipient of this channel".into()));
            }
            Ok(None)
        }
    }
}

pub fn scylla_err(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(anyhow::anyhow!("ScyllaDB error: {}", e))
}

async fn query_messages(
    state: &AppState,
    channel_id: i64,
    before: Option<i64>,
    after: Option<i64>,
    limit: usize,
) -> Result<Vec<MsgRow>, AppError> {
    let mut results = Vec::new();
    let ps = state.db.prepared();

    if let Some(before_id) = before {
        let snowflake = Snowflake::from(before_id);
        let start_bucket = bucket::bucket_from_snowflake(snowflake);
        let min_bucket = (start_bucket - 20).max(0); // scan at most 20 buckets (~200 days)
        let buckets = bucket::buckets_before(snowflake, min_bucket);

        for b in buckets {
            if results.len() >= limit { break; }
            let remaining = (limit - results.len()) as i32;
            let qr = state.db.scylla.execute_unpaged(
                &ps.get_messages_before,
                (channel_id, b, before_id, remaining),
            ).await.map_err(scylla_err)?;
            results.extend(extract_msg_rows(qr));
        }
    } else if let Some(after_id) = after {
        let snowflake = Snowflake::from(after_id);
        let start_bucket = bucket::bucket_from_snowflake(snowflake);
        let end_bucket = bucket::current_bucket();

        for b in start_bucket..=end_bucket {
            if results.len() >= limit { break; }
            let remaining = (limit - results.len()) as i32;
            let qr = state.db.scylla.execute_unpaged(
                &ps.get_messages_after,
                (channel_id, b, after_id, remaining),
            ).await.map_err(scylla_err)?;
            results.extend(extract_msg_rows(qr));
        }
    } else {
        let current = bucket::current_bucket();
        let min_bucket = (current - 20).max(0); // scan at most 20 buckets (~200 days)
        for b in (min_bucket..=current).rev() {
            if results.len() >= limit { break; }
            let remaining = (limit - results.len()) as i32;
            let qr = state.db.scylla.execute_unpaged(
                &ps.get_messages_latest,
                (channel_id, b, remaining),
            ).await.map_err(scylla_err)?;
            results.extend(extract_msg_rows(qr));
        }
    }

    Ok(results)
}

fn parse_mentions(content: &str) -> (Vec<i64>, Vec<i64>, bool) {
    let mut user_ids = Vec::new();
    let mut role_ids = Vec::new();
    let mention_everyone = content.contains("@everyone") || content.contains("@here");

    for cap in USER_MENTION_RE.captures_iter(content) {
        if let Some(id) = cap.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
            user_ids.push(id);
        }
    }
    for cap in ROLE_MENTION_RE.captures_iter(content) {
        if let Some(id) = cap.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
            role_ids.push(id);
        }
    }

    (user_ids, role_ids, mention_everyone)
}

// ─── Handlers ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GetMessagesQuery {
    pub limit: Option<i32>,
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub around: Option<i64>,
}

async fn get_messages(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
    Query(query): Query<GetMessagesQuery>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::READ_MESSAGE_HISTORY).await?;

    let limit = query.limit.unwrap_or(50).clamp(1, 100) as usize;

    let rows = if let Some(around_id) = query.around {
        let half = limit / 2;
        let before = query_messages(&state, channel_id, Some(around_id), None, half).await?;
        let after = query_messages(&state, channel_id, None, Some(around_id), half).await?;

        // Fetch the target message itself
        let target_snowflake = Snowflake::from(around_id);
        let target_bucket = bucket::bucket_from_snowflake(target_snowflake);
        let target_qr = state.db.scylla.execute_unpaged(
            &state.db.prepared().get_message_by_id,
            (channel_id, target_bucket, around_id),
        ).await.map_err(scylla_err)?;
        let target_rows = extract_msg_rows(target_qr);

        let mut all = after;
        all.extend(target_rows);
        all.extend(before);
        all
    } else {
        query_messages(&state, channel_id, query.before, query.after, limit).await?
    };

    let response: Vec<MessageResponse> = rows.into_iter().map(row_to_response).collect();
    Ok(Json(response))
}

async fn get_message(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::READ_MESSAGE_HISTORY).await?;

    let snowflake = Snowflake::from(message_id);
    let b = bucket::bucket_from_snowflake(snowflake);

    let qr = state.db.scylla.execute_unpaged(
        &state.db.prepared().get_message_by_id,
        (channel_id, b, message_id),
    ).await.map_err(scylla_err)?;

    let rows = extract_msg_rows(qr);
    let row = rows.into_iter().next()
        .ok_or_else(|| AppError::NotFound("Message not found".into()))?;

    Ok(Json(row_to_response(row)))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SendMessageRequest {
    pub content: Option<String>,
    pub tts: Option<bool>,
    pub embeds: Option<serde_json::Value>,
    pub message_reference: Option<MessageReference>,
    pub attachments: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct MessageReference {
    pub message_id: i64,
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<SendMessageRequest>,
) -> Result<impl IntoResponse, AppError> {
    let server_id = check_channel_permission(&state, channel_id, auth.id, Permissions::SEND_MESSAGES).await?;

    let has_content = body.content.as_ref().is_some_and(|c| !c.is_empty());
    let has_embeds = body.embeds.as_ref().is_some_and(|e| !e.is_null());
    let has_attachments = body.attachments.as_ref().is_some_and(|a| !a.is_null());

    if !has_content && !has_embeds && !has_attachments {
        return Err(AppError::BadRequest("Message must have content, embeds, or attachments".into()));
    }

    if let Some(ref content) = body.content {
        if content.chars().count() > 4000 {
            return Err(AppError::BadRequest("Content must be 4000 characters or less".into()));
        }
    }

    // Slowmode check
    let rate_limit = sqlx::query_scalar::<_, i32>("SELECT rate_limit FROM channels WHERE id = $1")
        .bind(channel_id)
        .fetch_optional(&state.db.pg)
        .await?
        .unwrap_or(0);

    if rate_limit > 0 {
        let mut conn = state.redis.clone();
        let key = format!("slowmode:{}:{}", channel_id, auth.id);
        let exists: bool = redis::cmd("EXISTS").arg(&key).query_async(&mut conn).await.unwrap_or(false);
        if exists {
            return Err(AppError::RateLimited { retry_after: rate_limit as u64 });
        }
    }

    // Timeout check
    if let Some(sid) = server_id {
        let timed_out = sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
            "SELECT communication_disabled_until FROM server_members WHERE server_id = $1 AND user_id = $2",
        )
        .bind(sid)
        .bind(auth.id)
        .fetch_optional(&state.db.pg)
        .await?
        .flatten();

        if let Some(until) = timed_out {
            if until > chrono::Utc::now() {
                return Err(AppError::Forbidden("You are timed out in this server".into()));
            }
        }
    }

    let message_id = state.snowflake.next_id();
    let b = bucket::bucket_from_snowflake(message_id);

    let (mention_users, mention_roles_parsed, mention_everyone_detected) = body
        .content.as_ref().map(|c| parse_mentions(c)).unwrap_or_default();

    let mention_everyone = if mention_everyone_detected {
        if let Some(sid) = server_id {
            require_permissions(&state, sid, auth.id, Permissions::MENTION_EVERYONE).await.is_ok()
        } else {
            true // DMs allow @everyone
        }
    } else {
        false
    };

    let message_type: i16 = if body.message_reference.is_some() { 19 } else { 0 };
    let reference_id = body.message_reference.as_ref().map(|r| r.message_id);

    let mentions_json = serde_json::to_string(&mention_users).unwrap_or_default();
    let mention_roles_json = serde_json::to_string(&mention_roles_parsed).unwrap_or_default();
    let embeds_json = body.embeds.as_ref().map(|e| e.to_string());
    let attachments_json = body.attachments.as_ref().map(|a| a.to_string());

    state.db.scylla.execute_unpaged(
        &state.db.prepared().insert_message,
        (
            channel_id, b, message_id.value() as i64, auth.id.value() as i64,
            body.content.as_deref(), message_type, mention_everyone,
            Some(mentions_json.as_str()), Some(mention_roles_json.as_str()),
            embeds_json.as_deref(), attachments_json.as_deref(), reference_id,
        ),
    ).await.map_err(scylla_err)?;

    // Update last_message_id
    sqlx::query("UPDATE channels SET last_message_id = $1 WHERE id = $2")
        .bind(message_id).bind(channel_id).execute(&state.db.pg).await?;

    // Set slowmode
    if rate_limit > 0 {
        let mut conn = state.redis.clone();
        let key = format!("slowmode:{}:{}", channel_id, auth.id);
        let _: Result<(), _> = redis::cmd("SET").arg(&key).arg("1").arg("EX").arg(rate_limit as i64)
            .query_async(&mut conn).await;
    }

    let response = MessageResponse {
        id: message_id,
        channel_id: Snowflake::from(channel_id),
        author_id: auth.id,
        content: body.content,
        timestamp: message_id.created_at(),
        edited_timestamp: None,
        message_type,
        flags: 0,
        pinned: false,
        mention_everyone,
        mentions: mention_users,
        mention_roles: mention_roles_parsed,
        embeds: body.embeds,
        attachments: body.attachments,
        reference_id: reference_id.map(Snowflake::from),
    };

    // Publish to NATS Core + JetStream
    let event = serde_json::json!({
        "type": "MESSAGE_CREATE",
        "channel_id": channel_id,
        "server_id": server_id,
        "message": serde_json::to_value(&response).unwrap_or_default(),
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }
    if let Err(e) = state.nats.publish_durable(&format!("persist.messages.{}", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish durable NATS event");
    }

    Ok((StatusCode::CREATED, Json(response)))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct EditMessageRequest {
    pub content: Option<String>,
    pub embeds: Option<serde_json::Value>,
    pub flags: Option<i64>,
}

async fn edit_message(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    Json(body): Json<EditMessageRequest>,
) -> Result<impl IntoResponse, AppError> {
    let server_id = check_channel_permission(&state, channel_id, auth.id, Permissions::SEND_MESSAGES).await?;

    let snowflake = Snowflake::from(message_id);
    let b = bucket::bucket_from_snowflake(snowflake);

    // Get author
    let qr = state.db.scylla.execute_unpaged(
        &state.db.prepared().get_message_author,
        (channel_id, b, message_id),
    ).await.map_err(scylla_err)?;

    let author_id: i64 = {
        let rows_result = qr.into_rows_result().map_err(scylla_err)?;
        let mut iter = rows_result.rows::<(i64,)>().map_err(scylla_err)?;
        iter.next()
            .ok_or_else(|| AppError::NotFound("Message not found".into()))?
            .map_err(scylla_err)?
            .0
    };

    let is_author = author_id == auth.id.value() as i64;
    if !is_author {
        if body.content.is_some() || body.embeds.is_some() {
            return Err(AppError::Forbidden("Can only edit your own messages".into()));
        }
        match server_id {
            Some(sid) => require_permissions(&state, sid, auth.id, Permissions::MANAGE_MESSAGES).await?,
            None => return Err(AppError::Forbidden("Cannot edit others' messages in DMs".into())),
        }
    }

    let edited_ts = CqlTimestamp(chrono::Utc::now().timestamp_millis());

    if let Some(ref content) = body.content {
        state.db.scylla.execute_unpaged(
            &state.db.prepared().update_content,
            (content.as_str(), edited_ts, channel_id, b, message_id),
        ).await.map_err(scylla_err)?;
    }

    if let Some(ref embeds) = body.embeds {
        let embeds_str = embeds.to_string();
        state.db.scylla.execute_unpaged(
            &state.db.prepared().update_embeds,
            (embeds_str.as_str(), edited_ts, channel_id, b, message_id),
        ).await.map_err(scylla_err)?;
    }

    let event = serde_json::json!({
        "type": "MESSAGE_UPDATE", "channel_id": channel_id, "message_id": message_id,
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }
    if let Err(e) = state.nats.publish_durable(&format!("persist.messages.{}", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish durable MESSAGE_UPDATE event");
    }

    get_message(State(state), auth, Path((channel_id, message_id))).await
}

async fn delete_message(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;

    let snowflake = Snowflake::from(message_id);
    let b = bucket::bucket_from_snowflake(snowflake);

    let qr = state.db.scylla.execute_unpaged(
        &state.db.prepared().get_message_author,
        (channel_id, b, message_id),
    ).await.map_err(scylla_err)?;

    let author_id: i64 = {
        let rows_result = qr.into_rows_result().map_err(scylla_err)?;
        let mut iter = rows_result.rows::<(i64,)>().map_err(scylla_err)?;
        iter.next()
            .ok_or_else(|| AppError::NotFound("Message not found".into()))?
            .map_err(scylla_err)?
            .0
    };

    let is_author = author_id == auth.id.value() as i64;
    if !is_author {
        check_channel_permission(&state, channel_id, auth.id, Permissions::MANAGE_MESSAGES).await?;
    }

    state.db.scylla.execute_unpaged(
        &state.db.prepared().soft_delete,
        (channel_id, b, message_id),
    ).await.map_err(scylla_err)?;

    let event = serde_json::json!({
        "type": "MESSAGE_DELETE", "channel_id": channel_id, "message_id": message_id,
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }
    if let Err(e) = state.nats.publish_durable(&format!("persist.messages.{}", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish durable MESSAGE_DELETE event");
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct BulkDeleteRequest {
    pub messages: Vec<i64>,
}

async fn bulk_delete(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<BulkDeleteRequest>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::MANAGE_MESSAGES).await?;

    if body.messages.len() < 2 || body.messages.len() > 100 {
        return Err(AppError::BadRequest("Must provide 2-100 message IDs".into()));
    }

    let mut errors = Vec::new();
    for &msg_id in &body.messages {
        let snowflake = Snowflake::from(msg_id);
        let b = bucket::bucket_from_snowflake(snowflake);
        if let Err(e) = state.db.scylla.execute_unpaged(
            &state.db.prepared().soft_delete,
            (channel_id, b, msg_id),
        ).await {
            tracing::warn!(message_id = msg_id, error = %e, "Failed to soft-delete message");
            errors.push(msg_id);
        }
    }

    // Only dispatch event for successfully deleted messages
    let deleted_ids: Vec<i64> = body.messages.iter().filter(|id| !errors.contains(id)).copied().collect();
    let event = serde_json::json!({
        "type": "MESSAGE_DELETE_BULK", "channel_id": channel_id, "ids": deleted_ids,
    });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }
    if let Err(e) = state.nats.publish_durable(&format!("persist.messages.{}", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish durable MESSAGE_DELETE_BULK event");
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── Pins ───────────────────────────────────────────────────────

async fn get_pins(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::VIEW_CHANNEL).await?;

    let qr = state.db.scylla.execute_unpaged(
        &state.db.prepared().get_pin_ids,
        (channel_id,),
    ).await.map_err(scylla_err)?;

    let mut pin_ids = Vec::new();
    if let Ok(rows_result) = qr.into_rows_result() {
        if let Ok(iter) = rows_result.rows::<(i64,)>() {
            for row in iter {
                if let Ok(r) = row { pin_ids.push(r.0); }
            }
        }
    }

    // Group pin IDs by bucket to avoid N+1 queries
    let mut by_bucket: HashMap<i32, Vec<i64>> = HashMap::new();
    for msg_id in &pin_ids {
        let snowflake = Snowflake::from(*msg_id);
        let b = bucket::bucket_from_snowflake(snowflake);
        by_bucket.entry(b).or_default().push(*msg_id);
    }

    let mut messages = Vec::new();
    for (b, ids) in &by_bucket {
        for msg_id in ids {
            let qr = state.db.scylla.execute_unpaged(
                &state.db.prepared().get_message_by_id,
                (channel_id, *b, *msg_id),
            ).await.map_err(scylla_err)?;
            let rows = extract_msg_rows(qr);
            for r in rows {
                messages.push(row_to_response(r));
            }
        }
    }

    Ok(Json(messages))
}

async fn pin_message(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::MANAGE_MESSAGES).await?;

    // Max 50 pins
    let qr = state.db.scylla.execute_unpaged(
        &state.db.prepared().count_pins,
        (channel_id,),
    ).await.map_err(scylla_err)?;

    let count: i64 = {
        let rows_result = qr.into_rows_result().map_err(scylla_err)?;
        let mut iter = rows_result.rows::<(i64,)>().map_err(scylla_err)?;
        iter.next().and_then(|r| r.ok()).map(|r| r.0).unwrap_or(0)
    };

    if count >= 50 {
        return Err(AppError::BadRequest("Channel has reached max pins (50)".into()));
    }

    let snowflake = Snowflake::from(message_id);
    let b = bucket::bucket_from_snowflake(snowflake);

    // Verify the message exists and is not deleted
    let qr = state.db.scylla.execute_unpaged(
        &state.db.prepared().check_message_exists,
        (channel_id, b, message_id),
    ).await.map_err(scylla_err)?;

    let exists = {
        let rows_result = qr.into_rows_result().map_err(scylla_err)?;
        let mut iter = rows_result.rows::<(i64, bool)>().map_err(scylla_err)?;
        match iter.next() {
            Some(Ok((_, deleted))) => !deleted,
            _ => false,
        }
    };
    if !exists {
        return Err(AppError::NotFound("Message not found".into()));
    }

    state.db.scylla.execute_unpaged(
        &state.db.prepared().set_pinned,
        (channel_id, b, message_id),
    ).await.map_err(scylla_err)?;

    let now = CqlTimestamp(chrono::Utc::now().timestamp_millis());
    state.db.scylla.execute_unpaged(
        &state.db.prepared().insert_pin,
        (channel_id, message_id, auth.id.value() as i64, now),
    ).await.map_err(scylla_err)?;

    let event = serde_json::json!({ "type": "CHANNEL_PINS_UPDATE", "channel_id": channel_id });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn unpin_message(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    check_channel_permission(&state, channel_id, auth.id, Permissions::MANAGE_MESSAGES).await?;

    let snowflake = Snowflake::from(message_id);
    let b = bucket::bucket_from_snowflake(snowflake);

    state.db.scylla.execute_unpaged(
        &state.db.prepared().unset_pinned,
        (channel_id, b, message_id),
    ).await.map_err(scylla_err)?;

    state.db.scylla.execute_unpaged(
        &state.db.prepared().delete_pin,
        (channel_id, message_id),
    ).await.map_err(scylla_err)?;

    let event = serde_json::json!({ "type": "CHANNEL_PINS_UPDATE", "channel_id": channel_id });
    if let Err(e) = state.nats.publish(&format!("channel.{}.messages", channel_id), &event).await {
        tracing::warn!(error = %e, "Failed to publish NATS event");
    }

    Ok(StatusCode::NO_CONTENT)
}
