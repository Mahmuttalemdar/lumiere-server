# P2-04 — JetStream Consumers (Search Indexer, Push Worker)

**Status:** Not Started
**Dependencies:** P2-01
**Crates:** lumiere-nats, lumiere-search, lumiere-push, lumiere-server

## Goal

Messages are published to JetStream (`persist.messages.>`) but nothing consumes them. Build the async workers that index messages to Meilisearch and deliver push notifications.

## Tasks

### 4.1 — Consumer Framework in lumiere-nats

Add consumer management to `NatsService`:

```rust
pub async fn create_consumer(
    &self,
    stream: &str,
    consumer_name: &str,
    filter_subject: &str,
) -> Result<Consumer<Pull>>

pub async fn consume_messages<F, Fut>(
    consumer: Consumer<Pull>,
    handler: F,
    batch_size: usize,
    batch_timeout: Duration,
) where F: Fn(Vec<Message>) -> Fut, Fut: Future<Output = Result<()>>
```

### 4.2 — Search Indexer Consumer

Consumes `persist.messages.>` stream:
1. Pull messages in batches (100 docs or 500ms window)
2. Parse MESSAGE_CREATE events → build `MessageDocument`
3. Parse MESSAGE_UPDATE events → re-index document
4. Parse MESSAGE_DELETE events → delete from index
5. Call `search_service.index_messages(batch)` for creates
6. Acknowledge processed messages

Consumer name: `search-indexer`
Durable: yes
Deliver policy: all (catch up on startup)

### 4.3 — Push Notification Consumer

Consumes `persist.messages.>` stream:
1. Pull MESSAGE_CREATE events
2. Determine recipients (channel members minus online users)
3. Check notification preferences (muted? suppressed?)
4. Build notification payload (author name, content preview)
5. Call `push_service.send_to_users(recipient_ids, notification)`
6. Acknowledge processed messages

Consumer name: `push-worker`
Durable: yes
Deliver policy: new (only future messages)

### 4.4 — Read State Updater Consumer

Consumes `persist.messages.>` stream:
1. On MESSAGE_CREATE with mentions
2. Increment `mention_count` in ScyllaDB `read_states` for mentioned users
3. Handle @everyone/@here — increment for all channel members

Consumer name: `read-state-updater`

### 4.5 — Consumer Startup & Lifecycle

- Start all consumers as background Tokio tasks in `main.rs`
- Graceful shutdown: drain consumers on SIGTERM
- Health check: consumers report lag via `/health/ready`
- Metrics: messages_processed, processing_latency, consumer_lag

## Acceptance Criteria

- [ ] Search indexer automatically indexes new messages within 1 second
- [ ] Push worker delivers notifications for new messages
- [ ] Read state updater tracks mention counts
- [ ] Consumers recover after restart (durable, ack-based)
- [ ] Consumer lag visible in health endpoint
- [ ] Integration test: send message → verify it appears in Meilisearch
