use crate::AppState;
use futures::StreamExt;
use lumiere_models::snowflake::Snowflake;
use lumiere_search::MessageDocument;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const CONSUMER_NAME: &str = "search-indexer";
const STREAM_NAME: &str = "MESSAGES";
const FILTER_SUBJECT: &str = "persist.messages.>";

/// Start the search indexer worker. Pulls messages from JetStream and
/// indexes/removes them in Meilisearch. Runs until `cancel` is triggered.
/// Automatically restarts on stream disconnection.
pub async fn start(state: Arc<AppState>, cancel: CancellationToken) {
    loop {
        if cancel.is_cancelled() {
            return;
        }
        tracing::info!("Starting search indexer worker...");
        match run_consumer(&state, &cancel).await {
            Ok(()) => return, // graceful shutdown
            Err(e) => {
                tracing::error!(error = %e, "Search indexer crashed, restarting in 5s");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn run_consumer(
    state: &AppState,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let consumer = state
        .nats
        .create_pull_consumer(STREAM_NAME, CONSUMER_NAME, Some(FILTER_SUBJECT))
        .await?;

    let mut messages = consumer.messages().await?;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("Search indexer worker shutting down");
                return Ok(());
            }
            msg = messages.next() => {
                let Some(Ok(msg)) = msg else {
                    anyhow::bail!("Search indexer message stream ended unexpectedly");
                };
                match process_message(state, &msg).await {
                    Ok(()) => {
                        if let Err(e) = msg.ack().await {
                            tracing::warn!(error = %e, "Search indexer ack failed");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Search indexer process error, will be redelivered");
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

    match event_type {
        "MESSAGE_CREATE" => {
            let Some(message) = event.get("message") else {
                return Ok(());
            };
            let Some(content) = message["content"].as_str() else {
                return Ok(());
            };

            // Skip empty content (attachment-only, embed-only messages)
            if content.is_empty() {
                return Ok(());
            }

            let id = message["id"]
                .as_str()
                .or_else(|| message["id"].as_i64().map(|_| ""))
                .unwrap_or("");

            // Parse snowflake — the id may be serialized as string or number
            let id_val: i64 = if let Some(n) = message["id"].as_i64() {
                n
            } else if let Ok(n) = id.parse::<i64>() {
                n
            } else {
                tracing::warn!("Search indexer: could not parse message id");
                return Ok(());
            };

            let channel_id = event["channel_id"].as_i64().unwrap_or(0);
            let server_id = event["server_id"].as_i64().unwrap_or(0);
            let author_id = message["author_id"]
                .as_i64()
                .or_else(|| message["author_id"].as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(0);

            let snowflake = Snowflake::from(id_val);
            let doc = MessageDocument::new(
                snowflake,
                Snowflake::from(channel_id),
                Snowflake::from(server_id),
                Snowflake::from(author_id),
                content.to_string(),
                snowflake.created_at(),
            );

            if let Some(ref search) = state.search {
                if let Err(e) = search.index_message(&doc).await {
                    tracing::warn!(error = %e, message_id = id_val, "Failed to index message");
                }
            }

            tracing::debug!(message_id = id_val, "Indexed message to Meilisearch");
        }
        "MESSAGE_UPDATE" => {
            // For updates, we re-index by fetching the message from ScyllaDB.
            // This is a deliberate trade-off: the update event only contains the
            // message_id, so we would need to read the full message to re-index.
            // For now we log it; a full implementation would query ScyllaDB and
            // re-index the updated content.
            let message_id = event["message_id"].as_i64().unwrap_or(0);
            tracing::debug!(message_id, "MESSAGE_UPDATE received — re-indexing not yet wired");
        }
        "MESSAGE_DELETE" => {
            let message_id = event["message_id"].as_i64().unwrap_or(0);
            if message_id != 0 {
                if let Some(ref search) = state.search {
                    if let Err(e) = search.delete_message(Snowflake::from(message_id)).await {
                        tracing::warn!(error = %e, message_id, "Failed to delete message from index");
                    }
                }
                tracing::debug!(message_id, "Removed message from Meilisearch index");
            }
        }
        "MESSAGE_DELETE_BULK" => {
            if let Some(ids) = event["ids"].as_array() {
                if let Some(ref search) = state.search {
                    for id_val in ids {
                        let mid = id_val.as_i64().unwrap_or(0);
                        if mid != 0 {
                            if let Err(e) =
                                search.delete_message(Snowflake::from(mid)).await
                            {
                                tracing::warn!(error = %e, message_id = mid, "Failed to bulk-delete message from index");
                            }
                        }
                    }
                }
                tracing::debug!(count = ids.len(), "Bulk-removed messages from Meilisearch index");
            }
        }
        _ => {
            tracing::trace!(event_type, "Search indexer ignoring unknown event type");
        }
    }

    Ok(())
}
