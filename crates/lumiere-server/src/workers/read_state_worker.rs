use crate::AppState;
use futures::StreamExt;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const CONSUMER_NAME: &str = "read-state-updater";
const STREAM_NAME: &str = "MESSAGES";
const FILTER_SUBJECT: &str = "persist.messages.>";

/// Start the read state worker. Pulls MESSAGE_CREATE events from JetStream
/// and increments unread / mention counts in the read_states table.
/// Runs until `cancel` is triggered. Automatically restarts on stream
/// disconnection.
pub async fn start(state: Arc<AppState>, cancel: CancellationToken) {
    loop {
        if cancel.is_cancelled() {
            return;
        }
        tracing::info!("Starting read state worker...");
        match run_consumer(&state, &cancel).await {
            Ok(()) => return, // graceful shutdown
            Err(e) => {
                tracing::error!(error = %e, "Read state worker crashed, restarting in 5s");
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
                tracing::info!("Read state worker shutting down");
                return Ok(());
            }
            msg = messages.next() => {
                let Some(Ok(msg)) = msg else {
                    anyhow::bail!("Read state message stream ended unexpectedly");
                };
                match process_message(state, &msg).await {
                    Ok(()) => {
                        if let Err(e) = msg.ack().await {
                            tracing::warn!(error = %e, "Read state worker ack failed");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Read state worker process error, will be redelivered");
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

    // Only process new messages for read state updates
    if event_type != "MESSAGE_CREATE" {
        return Ok(());
    }

    let Some(message) = event.get("message") else {
        return Ok(());
    };

    let channel_id = event["channel_id"].as_i64().unwrap_or(0);
    let server_id = event["server_id"].as_i64();
    let author_id = message["author_id"]
        .as_i64()
        .or_else(|| message["author_id"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    let mention_everyone = message["mention_everyone"].as_bool().unwrap_or(false);

    if channel_id == 0 {
        return Ok(());
    }

    // Collect explicitly mentioned user IDs
    let mentioned_users: Vec<i64> = message["mentions"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();

    // Get all channel members (from the server membership)
    let members: Vec<i64> = if let Some(sid) = server_id {
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

    if members.is_empty() {
        return Ok(());
    }

    let message_id = message["id"]
        .as_i64()
        .or_else(|| message["id"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);

    // For each member, update their read state:
    // - Set last_message_id to the latest message in the channel
    // - Increment mention_count if they were mentioned (or @everyone was used)
    for &member_id in &members {
        let is_mentioned = mention_everyone || mentioned_users.contains(&member_id);

        // Upsert read state: update last_message_id and optionally increment
        // mention_count. We use ScyllaDB counters via a separate table, or
        // PostgreSQL for read state if that's where it lives.
        //
        // Using PostgreSQL for read_states (metadata, not message-scale).
        if is_mentioned {
            sqlx::query(
                "INSERT INTO read_states (user_id, channel_id, last_message_id, mention_count)
                 VALUES ($1, $2, $3, 1)
                 ON CONFLICT (user_id, channel_id)
                 DO UPDATE SET
                     last_message_id = GREATEST(read_states.last_message_id, EXCLUDED.last_message_id),
                     mention_count = read_states.mention_count + 1",
            )
            .bind(member_id)
            .bind(channel_id)
            .bind(message_id)
            .execute(&state.db.pg)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO read_states (user_id, channel_id, last_message_id, mention_count)
                 VALUES ($1, $2, $3, 0)
                 ON CONFLICT (user_id, channel_id)
                 DO UPDATE SET
                     last_message_id = GREATEST(read_states.last_message_id, EXCLUDED.last_message_id)",
            )
            .bind(member_id)
            .bind(channel_id)
            .bind(message_id)
            .execute(&state.db.pg)
            .await?;
        }
    }

    tracing::debug!(
        channel_id,
        message_id,
        members = members.len(),
        mentions = mentioned_users.len(),
        mention_everyone,
        "Updated read states"
    );

    Ok(())
}
