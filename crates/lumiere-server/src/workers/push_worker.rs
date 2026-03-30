use crate::AppState;
use futures::StreamExt;
use lumiere_models::snowflake::Snowflake;
use lumiere_push::PushNotification;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const CONSUMER_NAME: &str = "push-worker";
const STREAM_NAME: &str = "MESSAGES";
const FILTER_SUBJECT: &str = "persist.messages.>";

/// Start the push notification worker. Pulls MESSAGE_CREATE events from
/// JetStream and sends push notifications to offline channel members.
/// Runs until `cancel` is triggered. Automatically restarts on stream
/// disconnection.
pub async fn start(state: Arc<AppState>, cancel: CancellationToken) {
    loop {
        if cancel.is_cancelled() {
            return;
        }
        tracing::info!("Starting push notification worker...");
        match run_consumer(&state, &cancel).await {
            Ok(()) => return, // graceful shutdown
            Err(e) => {
                tracing::error!(error = %e, "Push worker crashed, restarting in 5s");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn run_consumer(state: &AppState, cancel: &CancellationToken) -> anyhow::Result<()> {
    let consumer = state
        .nats
        .create_pull_consumer(STREAM_NAME, CONSUMER_NAME, Some(FILTER_SUBJECT))
        .await?;

    let mut messages = consumer.messages().await?;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("Push notification worker shutting down");
                return Ok(());
            }
            msg = messages.next() => {
                let Some(Ok(msg)) = msg else {
                    anyhow::bail!("Push worker message stream ended unexpectedly");
                };
                match process_message(state, &msg).await {
                    Ok(()) => {
                        if let Err(e) = msg.ack().await {
                            tracing::warn!(error = %e, "Push worker ack failed");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Push worker process error, will be redelivered");
                        // Don't ack — NATS will redeliver after ack_wait (30s)
                    }
                }
            }
        }
    }
}

async fn process_message(
    state: &AppState,
    msg: &async_nats::jetstream::Message,
) -> anyhow::Result<()> {
    let event: serde_json::Value = serde_json::from_slice(&msg.payload)?;
    let event_type = event["type"].as_str().unwrap_or("");

    // Only send push notifications for new messages
    if event_type != "MESSAGE_CREATE" {
        return Ok(());
    }

    let Some(message) = event.get("message") else {
        return Ok(());
    };

    let channel_id = event["channel_id"].as_i64().unwrap_or(0);
    let author_id = message["author_id"]
        .as_i64()
        .or_else(|| message["author_id"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    let content = message["content"].as_str().unwrap_or("");

    if channel_id == 0 || author_id == 0 {
        return Ok(());
    }

    // Look up the author's display name for the notification title
    let author_name: Option<String> =
        sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
            .bind(author_id)
            .fetch_optional(&state.db.pg)
            .await?;
    let author_name = author_name.unwrap_or_else(|| "Someone".to_string());

    // Look up channel name
    let channel_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM channels WHERE id = $1")
            .bind(channel_id)
            .fetch_optional(&state.db.pg)
            .await?;
    let channel_name = channel_name.unwrap_or_else(|| "a channel".to_string());

    // Get all members of the channel's server, excluding the author
    let server_id = event["server_id"].as_i64();
    let recipients: Vec<i64> = if let Some(sid) = server_id {
        sqlx::query_scalar(
            "SELECT user_id FROM server_members WHERE server_id = $1 AND user_id != $2",
        )
        .bind(sid)
        .bind(author_id)
        .fetch_all(&state.db.pg)
        .await?
    } else {
        Vec::new()
    };

    if recipients.is_empty() {
        return Ok(());
    }

    // Filter out online users (they already see messages via WebSocket).
    // Users with an active presence key in Redis are considered online.
    // Use a pipeline to avoid N+1 Redis round-trips.
    let mut offline_recipients = Vec::new();
    {
        let mut conn = state.redis.clone();
        let mut pipe = redis::pipe();
        for &uid in &recipients {
            pipe.cmd("EXISTS").arg(format!("presence:{}", uid));
        }
        let results: Vec<bool> = pipe.query_async(&mut conn).await.unwrap_or_default();
        for (i, &uid) in recipients.iter().enumerate() {
            let online = results.get(i).copied().unwrap_or(false);
            if !online {
                offline_recipients.push(uid);
            }
        }
    }

    if offline_recipients.is_empty() {
        return Ok(());
    }

    // Truncate long messages for the notification body.
    // Use .chars().take() to avoid panic on multi-byte UTF-8 boundaries.
    let body_text = if content.chars().count() > 200 {
        let truncated: String = content.chars().take(197).collect();
        format!("{}...", truncated)
    } else if content.is_empty() {
        "Sent an attachment".to_string()
    } else {
        content.to_string()
    };

    let notification =
        PushNotification::new(format!("{} in #{}", author_name, channel_name), body_text)
            .with_data("channel_id", channel_id.to_string())
            .with_data("type", "MESSAGE_CREATE".to_string())
            .with_thread_id(format!("channel-{}", channel_id));

    if let Some(ref push) = state.push {
        let user_ids: Vec<Snowflake> = offline_recipients
            .iter()
            .map(|&id| Snowflake::from(id))
            .collect();
        let delivered = push.send_to_users(&user_ids, &notification).await;
        tracing::debug!(
            channel_id,
            recipients = offline_recipients.len(),
            delivered,
            "Push notifications sent for MESSAGE_CREATE"
        );
    }

    Ok(())
}
