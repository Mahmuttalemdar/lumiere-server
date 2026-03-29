# Sprint 13 — Search System

**Status:** Not Started
**Dependencies:** Sprint 09
**Crates:** lumiere-search, lumiere-server

## Goal

Full-text message search using Meilisearch. Asynchronous indexing via NATS JetStream consumer. Search API with filters (author, channel, date range, content).

## Tasks

### 13.1 — Meilisearch Client Setup

```rust
// crates/lumiere-search/src/lib.rs
use meilisearch_sdk::Client;

pub struct SearchService {
    client: Client,
}

impl SearchService {
    pub async fn new(config: &MeilisearchConfig) -> Result<Self> { ... }
    pub async fn setup_indexes(&self) -> Result<()> { ... }
}
```

### 13.2 — Index Configuration

Create `messages` index with proper settings:

```rust
pub async fn setup_indexes(&self) -> Result<()> {
    let index = self.client.index("messages");

    // Searchable fields (in priority order)
    index.set_searchable_attributes(&[
        "content",
        "author_username",
        "attachment_filenames",
    ]).await?;

    // Filterable fields (for search filters)
    index.set_filterable_attributes(&[
        "channel_id",
        "server_id",
        "author_id",
        "has_attachment",
        "has_embed",
        "has_link",
        "is_pinned",
        "timestamp",
    ]).await?;

    // Sortable fields
    index.set_sortable_attributes(&[
        "timestamp",
    ]).await?;

    // Displayed fields (what's returned in results)
    index.set_displayed_attributes(&[
        "id",
        "channel_id",
        "server_id",
        "author_id",
        "author_username",
        "content",
        "timestamp",
        "has_attachment",
    ]).await?;

    Ok(())
}
```

### 13.3 — Document Schema for Meilisearch

```rust
#[derive(Serialize)]
pub struct SearchableMessage {
    pub id: String,                  // Snowflake as string (Meilisearch primary key)
    pub channel_id: String,
    pub server_id: String,
    pub author_id: String,
    pub author_username: String,
    pub content: String,
    pub timestamp: i64,              // Unix timestamp for filtering/sorting
    pub has_attachment: bool,
    pub has_embed: bool,
    pub has_link: bool,
    pub is_pinned: bool,
    pub attachment_filenames: Vec<String>,
}
```

### 13.4 — Indexing Pipeline (JetStream Consumer)

```rust
// Background worker: consumes from JetStream MESSAGES stream
pub async fn run_search_indexer(
    jetstream: &async_nats::jetstream::Context,
    search: &SearchService,
) -> Result<()> {
    let consumer = jetstream.get_or_create_consumer(
        "MESSAGES",
        async_nats::jetstream::consumer::pull::Config {
            durable_name: Some("search-indexer".to_string()),
            filter_subject: "persist.messages.>".to_string(),
            ..Default::default()
        },
    ).await?;

    let mut messages = consumer.messages().await?;

    while let Some(msg) = messages.next().await {
        let msg = msg?;
        let event: MessageEvent = serde_json::from_slice(&msg.payload)?;

        match event.action {
            Action::Create => {
                let doc = SearchableMessage::from(event.message);
                search.index_message(doc).await?;
            }
            Action::Update => {
                let doc = SearchableMessage::from(event.message);
                search.update_message(doc).await?;
            }
            Action::Delete => {
                search.delete_message(&event.message_id).await?;
            }
        }

        msg.ack().await?;
    }

    Ok(())
}
```

Batch indexing for efficiency:
```rust
// Buffer messages and index in batches of 100 or every 500ms, whichever comes first
pub async fn batch_index(&self, messages: Vec<SearchableMessage>) -> Result<()> {
    self.client.index("messages")
        .add_documents(&messages, Some("id"))
        .await?;
    Ok(())
}
```

### 13.5 — Search API

```
GET /api/v1/servers/:server_id/messages/search
    Query params:
        ?content=hello world        # Search text
        ?author_id=123              # Filter by author
        ?channel_id=456             # Filter by channel
        ?has=file                   # has: file, link, embed, image, video, sound
        ?before=2026-03-01          # Messages before date
        ?after=2026-01-01           # Messages after date
        ?sort=timestamp:desc        # Sort order
        ?offset=0                   # Pagination offset
        ?limit=25                   # Results per page (max 25)

    Response: {
        total_hits: 150,
        messages: [
            {
                id: "...",
                channel_id: "...",
                content: "Hello world, this is...",  // With highlights
                author: { ... },
                timestamp: "...",
                hit: {
                    _formatted: {
                        content: "**Hello** **world**, this is..."
                    }
                }
            }
        ]
    }

    Auth: Must be server member; results filtered to channels user can VIEW_CHANNEL
```

Permission filtering:
1. Get all channels the user has VIEW_CHANNEL + READ_MESSAGE_HISTORY permissions in
2. Add `channel_id IN [list]` filter to Meilisearch query
3. This ensures users only see search results from channels they can access

### 13.6 — DM Search

```
GET /api/v1/channels/:channel_id/messages/search
    Query params: same as above (minus server-specific filters)
    Auth: Must be participant of the DM
```

### 13.7 — Index Maintenance

- **Reindex command**: Admin endpoint to trigger full reindex from ScyllaDB
- **Index stats**: Health endpoint showing index size, pending tasks
- **Auto-cleanup**: Delete indexed messages that have been deleted from ScyllaDB (daily job)

```
POST /api/v1/admin/search/reindex
    Auth: ADMINISTRATOR or system user
    Action: Queue full reindex from ScyllaDB messages table
```

## Performance Considerations

- Meilisearch handles up to 10M documents well on a single node
- Beyond that, consider sharding by server_id (separate indexes per large server)
- Index updates are batched (100 documents or 500ms)
- Search latency target: < 50ms p99
- Permission filtering done at query time (Meilisearch filter), not post-query

## Acceptance Criteria

- [ ] Messages automatically indexed to Meilisearch via JetStream consumer
- [ ] Search by content returns relevant results with highlights
- [ ] Filter by author, channel, date range, attachment type
- [ ] Results respect channel permissions (users only see searchable channels)
- [ ] Edited messages update in search index
- [ ] Deleted messages removed from search index
- [ ] Pagination works with offset/limit
- [ ] DM search works
- [ ] Batch indexing for efficiency
- [ ] Search latency < 50ms for typical queries
- [ ] Integration test: send message → wait for indexing → search → verify result
