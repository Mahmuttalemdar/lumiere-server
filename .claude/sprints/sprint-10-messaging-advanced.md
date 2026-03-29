# Sprint 10 — Messaging Advanced

**Status:** Not Started
**Dependencies:** Sprint 09, Sprint 12
**Crates:** lumiere-server, lumiere-db, lumiere-media

## Goal

Advanced messaging features: attachments, embeds (link previews), emoji reactions, replies/threads, and message crossposting.

## Tasks

### 10.1 — Attachments

Messages can include files uploaded to MinIO.

Upload flow (two-step):
1. Client uploads file via `POST /api/v1/channels/:channel_id/attachments`
2. Server returns attachment ID and upload URL
3. Client includes attachment IDs in the message body

```
POST /api/v1/channels/:channel_id/attachments
    Body: multipart/form-data { file, filename, description? }
    Response: {
        id: snowflake,
        filename: "image.png",
        size: 1048576,
        content_type: "image/png",
        url: "https://cdn.lumiere.app/attachments/...",
        proxy_url: "...",
        width: 1920,      // Images/videos only
        height: 1080,
    }
    Auth: ATTACH_FILES permission
    Validation:
        - Max file size: 25 MB (free), 50 MB (premium)
        - Max 10 attachments per message
        - Content type validation (block executables)
```

```rust
pub struct Attachment {
    pub id: Snowflake,
    pub filename: String,
    pub description: Option<String>,
    pub content_type: String,
    pub size: u64,
    pub url: String,
    pub proxy_url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub ephemeral: bool,
}
```

Image processing:
- Generate thumbnail for images (max 400x300)
- Extract dimensions for images and videos
- Store metadata in attachment record

### 10.2 — Embeds

Embeds are rich content blocks in messages. Two types:

**A. User-crafted embeds** (sent by bots/webhooks):
```rust
pub struct Embed {
    pub title: Option<String>,        // Max 256 chars
    pub description: Option<String>,  // Max 4096 chars
    pub url: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub color: Option<u32>,
    pub footer: Option<EmbedFooter>,
    pub image: Option<EmbedMedia>,
    pub thumbnail: Option<EmbedMedia>,
    pub author: Option<EmbedAuthor>,
    pub fields: Vec<EmbedField>,      // Max 25 fields
}
```

**B. Link previews** (auto-generated):
When a message contains URLs, fetch OpenGraph/oEmbed metadata in the background:

```rust
// Background task (via JetStream consumer)
async fn generate_link_preview(url: &str) -> Option<Embed> {
    // 1. HEAD request to check content type and size
    // 2. If HTML: fetch and parse og:title, og:description, og:image
    // 3. If image: create image embed
    // 4. If video (YouTube, etc.): use oEmbed API
    // 5. Return embed
}
```

After generating:
- Update message embeds in ScyllaDB
- Dispatch MESSAGE_UPDATE event

### 10.3 — Reactions

```
PUT /api/v1/channels/:channel_id/messages/:message_id/reactions/:emoji/@me
    Response: 204 No Content
    Auth: ADD_REACTIONS permission (first reaction) or READ_MESSAGE_HISTORY (subsequent)
    Note: emoji is URL-encoded Unicode (e.g., %F0%9F%91%8D) or custom emoji ID

DELETE /api/v1/channels/:channel_id/messages/:message_id/reactions/:emoji/@me
    Response: 204 No Content

DELETE /api/v1/channels/:channel_id/messages/:message_id/reactions/:emoji/:user_id
    Response: 204 No Content
    Auth: MANAGE_MESSAGES

DELETE /api/v1/channels/:channel_id/messages/:message_id/reactions/:emoji
    Response: 204 No Content
    Auth: MANAGE_MESSAGES
    Action: Remove all reactions of this emoji

DELETE /api/v1/channels/:channel_id/messages/:message_id/reactions
    Response: 204 No Content
    Auth: MANAGE_MESSAGES
    Action: Remove ALL reactions

GET /api/v1/channels/:channel_id/messages/:message_id/reactions/:emoji
    Query: ?limit=100&after=user_id
    Response: [PublicUser objects who reacted]
```

Reaction storage in ScyllaDB:
```cql
-- reactions table (see Sprint 02)
-- Allows fast: "who reacted with X on message Y?"
-- And fast: "what reactions does message Y have?"
```

Also update the denormalized `reactions` JSON field on the message itself for fast loading.

Reaction object:
```rust
pub struct Reaction {
    pub count: u32,
    pub me: bool,           // Did the requesting user react?
    pub emoji: ReactionEmoji,
}

pub enum ReactionEmoji {
    Unicode { name: String },
    Custom { id: Snowflake, name: String, animated: bool },
}
```

### 10.4 — Replies

When `message_reference` is provided in send message:
1. Validate the referenced message exists and is in the same channel
2. Set message `type = Reply` (19)
3. Include `referenced_message` in the response (the message being replied to)
4. If the replied-to message author is in the server, add them to mentions
5. Dispatch MESSAGE_CREATE with the full reply context

```json
{
    "content": "I agree!",
    "message_reference": {
        "message_id": "123456789",
        "channel_id": "987654321",
        "fail_if_not_exists": true
    }
}
```

### 10.5 — Custom Emoji

```
POST /api/v1/servers/:server_id/emojis
    Body: { name, image (base64), roles?: [role_ids] }
    Response: Emoji object
    Auth: MANAGE_EMOJIS
    Validation:
        - name: 2-32 chars, alphanumeric + underscore
        - image: PNG/GIF, max 256 KB
        - Max 50 regular + 50 animated emojis per server

GET /api/v1/servers/:server_id/emojis
    Response: [Emoji objects]

PATCH /api/v1/servers/:server_id/emojis/:emoji_id
    Body: { name?, roles? }
    Auth: MANAGE_EMOJIS

DELETE /api/v1/servers/:server_id/emojis/:emoji_id
    Auth: MANAGE_EMOJIS
```

Custom emoji format in messages: `<:name:id>` or `<a:name:id>` (animated).

### 10.6 — Message Flags

```rust
bitflags::bitflags! {
    pub struct MessageFlags: u64 {
        const CROSSPOSTED           = 1 << 0;  // Published from announcement channel
        const IS_CROSSPOST          = 1 << 1;  // This message is a crosspost
        const SUPPRESS_EMBEDS       = 1 << 2;  // Don't show embeds
        const SOURCE_DELETED        = 1 << 3;  // Source message was deleted (crosspost)
        const URGENT                = 1 << 4;
        const HAS_THREAD            = 1 << 5;
        const EPHEMERAL             = 1 << 6;  // Only visible to invoker
        const LOADING               = 1 << 7;  // Interaction response loading
        const SUPPRESS_NOTIFICATIONS = 1 << 12; // @silent message
    }
}
```

## WebSocket Events

- `MESSAGE_REACTION_ADD` — { user_id, channel_id, message_id, emoji }
- `MESSAGE_REACTION_REMOVE` — { user_id, channel_id, message_id, emoji }
- `MESSAGE_REACTION_REMOVE_ALL` — { channel_id, message_id }
- `MESSAGE_REACTION_REMOVE_EMOJI` — { channel_id, message_id, emoji }

## Acceptance Criteria

- [ ] File attachments upload to MinIO and are accessible via URL
- [ ] Image thumbnails generated automatically
- [ ] File size limits enforced
- [ ] Link previews generated asynchronously for URLs in messages
- [ ] Reactions: add, remove, list reactors all work
- [ ] Reaction count updates in real-time via WebSocket
- [ ] Custom emoji upload, use in messages, and rendering
- [ ] Reply messages include referenced_message
- [ ] Reply author added to mentions
- [ ] Message flags work (suppress embeds, silent messages)
- [ ] Embed validation (field limits, total size limits)
- [ ] Integration test: send message with attachment → verify MinIO upload → verify thumbnail
- [ ] Integration test: reaction flow → add → list → remove → verify counts
