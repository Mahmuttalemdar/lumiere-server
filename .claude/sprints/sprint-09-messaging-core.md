# Sprint 09 — Messaging Core

**Status:** Not Started
**Dependencies:** Sprint 08
**Crates:** lumiere-server, lumiere-db, lumiere-gateway, lumiere-nats

## Goal

Core messaging: send, receive, edit, delete messages via REST and WebSocket. Message persistence in ScyllaDB. Cursor-based pagination for message history. NATS-based real-time delivery.

## Tasks

### 9.1 — Message Object

```rust
pub struct Message {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub author: PublicUser,
    pub content: String,
    pub timestamp: DateTime<Utc>,     // Extracted from Snowflake ID
    pub edited_timestamp: Option<DateTime<Utc>>,
    pub tts: bool,
    pub mention_everyone: bool,
    pub mentions: Vec<PublicUser>,
    pub mention_roles: Vec<Snowflake>,
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub reactions: Vec<Reaction>,
    pub pinned: bool,
    pub r#type: MessageType,
    pub flags: u64,
    pub referenced_message: Option<Box<Message>>,  // For replies
    pub thread: Option<Channel>,
}

pub enum MessageType {
    Default = 0,
    RecipientAdd = 1,
    RecipientRemove = 2,
    Call = 3,
    ChannelNameChange = 4,
    ChannelIconChange = 5,
    PinnedMessage = 6,
    MemberJoin = 7,
    Reply = 19,
    ThreadCreated = 22,
}
```

### 9.2 — Send Message

```
POST /api/v1/channels/:channel_id/messages
    Body: {
        content?: "Hello world",           // Max 4000 chars
        tts?: false,
        embeds?: [...],
        message_reference?: { message_id }, // Reply
        attachments?: [...]                 // File references (uploaded separately)
    }
    Response: Message object
    Auth: SEND_MESSAGES permission
    Validation:
        - At least one of: content, embeds, attachments (can't be empty)
        - content max 4000 chars
        - max 10 embeds
        - max 10 attachments
        - Slowmode check (if channel.rate_limit > 0)
        - Timeout check (if member.communication_disabled_until > now)
```

Flow:
1. Validate permissions (SEND_MESSAGES in channel)
2. Check slowmode (Redis: `slowmode:{channel_id}:{user_id}`)
3. Check member timeout
4. Generate Snowflake ID
5. Parse mentions from content (`<@user_id>`, `<@&role_id>`, `@everyone`, `@here`)
6. Calculate bucket from Snowflake ID
7. Write to ScyllaDB (messages table)
8. Update channel's `last_message_id` in PostgreSQL
9. Set slowmode cooldown in Redis (if applicable)
10. Publish to NATS Core: `channel.{channel_id}` → MESSAGE_CREATE event
11. Publish to NATS JetStream: `messages.{channel_id}` → for persistence workers (search indexer, push)
12. Return message object

### 9.3 — Message History (Get Messages)

```
GET /api/v1/channels/:channel_id/messages
    Query params:
        ?limit=50          (1-100, default 50)
        ?before=snowflake  (get messages before this ID)
        ?after=snowflake   (get messages after this ID)
        ?around=snowflake  (get messages around this ID)
    Response: [Message objects]
    Auth: READ_MESSAGE_HISTORY permission
```

ScyllaDB query strategy:

**Before (most common — loading older messages):**
```cql
SELECT * FROM messages
WHERE channel_id = ? AND bucket = ? AND message_id < ?
ORDER BY message_id DESC
LIMIT ?
```
Start with current bucket, if not enough messages, query previous buckets until limit is reached.

**After (loading newer messages):**
```cql
SELECT * FROM messages
WHERE channel_id = ? AND bucket = ? AND message_id > ?
ORDER BY message_id ASC
LIMIT ?
```

**Around (context around a message):**
- Fetch `limit/2` before and `limit/2` after the target message
- Merge results

**Bucket traversal logic:**
```rust
pub async fn get_messages_before(
    channel_id: Snowflake,
    before: Snowflake,
    limit: usize,
) -> Result<Vec<Message>> {
    let mut results = Vec::new();
    let mut current_bucket = bucket_from_snowflake(before);
    let min_bucket = bucket_from_snowflake(channel_created_at);

    while results.len() < limit && current_bucket >= min_bucket {
        let batch = query_bucket(channel_id, current_bucket, before, limit - results.len()).await?;
        results.extend(batch);
        current_bucket -= 1;
    }

    Ok(results)
}
```

### 9.4 — Edit Message

```
PATCH /api/v1/channels/:channel_id/messages/:message_id
    Body: { content?, embeds?, flags? }
    Response: Updated message object
    Auth: Must be message author (or MANAGE_MESSAGES for flags only)
    Actions:
        - Update content/embeds in ScyllaDB
        - Set edited_at timestamp
        - Dispatch MESSAGE_UPDATE event via NATS
        - Update Meilisearch index (via JetStream)
```

### 9.5 — Delete Message

```
DELETE /api/v1/channels/:channel_id/messages/:message_id
    Response: 204 No Content
    Auth: Must be message author OR MANAGE_MESSAGES permission
    Actions:
        - Soft delete in ScyllaDB (set deleted = true)
        - Dispatch MESSAGE_DELETE event via NATS
        - Remove from Meilisearch index (via JetStream)

POST /api/v1/channels/:channel_id/messages/bulk-delete
    Body: { messages: [message_id_1, message_id_2, ...] }
    Response: 204 No Content
    Auth: MANAGE_MESSAGES
    Validation:
        - 2-100 message IDs
        - Messages must not be older than 14 days
    Actions:
        - Soft delete all in ScyllaDB
        - Dispatch MESSAGE_DELETE_BULK event via NATS
```

### 9.6 — Get Single Message

```
GET /api/v1/channels/:channel_id/messages/:message_id
    Response: Message object
    Auth: READ_MESSAGE_HISTORY permission
```

### 9.7 — Pin Messages

```
PUT /api/v1/channels/:channel_id/pins/:message_id
    Response: 204 No Content
    Auth: MANAGE_MESSAGES
    Validation: Max 50 pins per channel
    Actions:
        - Set pinned=true in ScyllaDB messages table
        - Insert into pins table
        - Create system message (type=PinnedMessage) in channel
        - Dispatch CHANNEL_PINS_UPDATE event

DELETE /api/v1/channels/:channel_id/pins/:message_id
    Response: 204 No Content
    Auth: MANAGE_MESSAGES

GET /api/v1/channels/:channel_id/pins
    Response: [Message objects]
    Auth: VIEW_CHANNEL
```

### 9.8 — Mention Parsing

Parse message content for mentions:

```rust
// Patterns:
// <@USER_ID>       → user mention
// <@&ROLE_ID>      → role mention
// @everyone        → mention everyone
// @here            → mention online members
// <#CHANNEL_ID>    → channel mention

pub fn parse_mentions(content: &str) -> ParsedMentions {
    ParsedMentions {
        users: extract_user_mentions(content),
        roles: extract_role_mentions(content),
        everyone: content.contains("@everyone"),
        here: content.contains("@here"),
        channels: extract_channel_mentions(content),
    }
}
```

Validate that mentioned users are in the server and mentioned roles exist. `@everyone` and `@here` require MENTION_EVERYONE permission.

### 9.9 — NATS Message Flow

```
API Handler (send message)
    |
    |→ NATS Core: "channel.{channel_id}" (instant fanout to WebSocket clients)
    |→ NATS JetStream: "persist.messages.{channel_id}" (durable stream)
        |
        |→ Consumer: Search Indexer (indexes to Meilisearch)
        |→ Consumer: Push Notification Worker (sends to APNs/FCM)
        |→ Consumer: Read State Updater (updates mention counts)
```

JetStream stream configuration:
```rust
async fn setup_message_stream(jetstream: &async_nats::jetstream::Context) -> Result<()> {
    jetstream.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: "MESSAGES".to_string(),
        subjects: vec!["persist.messages.>".to_string()],
        retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
        max_age: std::time::Duration::from_secs(86400 * 7), // 7 days retention
        storage: async_nats::jetstream::stream::StorageType::File,
        ..Default::default()
    }).await?;
    Ok(())
}
```

## Acceptance Criteria

- [ ] Message sends and appears in real-time for all channel members via WebSocket
- [ ] Message persists in ScyllaDB with correct partition key
- [ ] Message history returns correct results with cursor-based pagination
- [ ] Bucket traversal works (queries span multiple 10-day buckets)
- [ ] Message edit updates content and sets edited_at
- [ ] Message delete soft-deletes and dispatches event
- [ ] Bulk delete works for up to 100 messages
- [ ] Mention parsing correctly identifies users, roles, @everyone, @here
- [ ] @everyone/@here require MENTION_EVERYONE permission
- [ ] Pin/unpin with max 50 pins per channel
- [ ] Slowmode enforced on message send
- [ ] Timed-out members cannot send messages
- [ ] NATS dual-path: Core for instant delivery, JetStream for persistence workers
- [ ] Integration test: send message → verify in ScyllaDB → verify WebSocket delivery
- [ ] Performance test: 1000 messages/second to a single channel
