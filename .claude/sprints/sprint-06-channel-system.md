# Sprint 06 — Channel System

**Status:** Not Started
**Dependencies:** Sprint 05
**Crates:** lumiere-server, lumiere-db

## Goal

Full channel management: text, voice, announcement, and category channels. Channel ordering, topic management, and slowmode.

## Tasks

### 6.1 — Channel Types

```rust
pub enum ChannelType {
    Text = 0,           // Standard text channel in a server
    DM = 1,             // Direct message between two users
    Voice = 2,          // Voice channel in a server
    GroupDM = 3,        // Group DM (2-10 users)
    Category = 4,       // Channel category (organizer)
    Announcement = 5,   // Announcement channel (can be followed)
}
```

### 6.2 — Channel CRUD

```
POST /api/v1/servers/:server_id/channels
    Body: {
        name: "general-chat",
        type: 0,
        parent_id?: snowflake,    // Category ID
        topic?: "Welcome to general",
        position?: 0,
        bitrate?: 64000,          // Voice only (8000-384000)
        user_limit?: 0,           // Voice only (0=unlimited)
        rate_limit?: 0,           // Slowmode seconds (0-21600)
        nsfw?: false,
        permission_overrides?: [{ id, type, allow, deny }]
    }
    Response: Channel object
    Auth: MANAGE_CHANNELS permission
    Validation:
        - name: 1-100 chars, lowercase, no spaces (auto-converted to hyphens for text)
        - parent_id must reference a category channel in the same server
        - Max 500 channels per server
        - Max 50 channels per category

GET /api/v1/channels/:channel_id
    Response: Channel object
    Auth: VIEW_CHANNEL permission

PATCH /api/v1/channels/:channel_id
    Body: { name?, topic?, position?, parent_id?, bitrate?, user_limit?,
            rate_limit?, nsfw?, permission_overrides? }
    Response: Updated channel object
    Auth: MANAGE_CHANNELS permission

DELETE /api/v1/channels/:channel_id
    Response: 204 No Content
    Auth: MANAGE_CHANNELS permission
    Note: Cannot delete the last text channel in a server
```

### 6.3 — Channel Object

```rust
pub struct Channel {
    pub id: Snowflake,
    pub server_id: Option<Snowflake>,
    pub parent_id: Option<Snowflake>,
    pub r#type: ChannelType,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub position: i32,
    pub bitrate: Option<u32>,
    pub user_limit: Option<u32>,
    pub rate_limit: u32,
    pub nsfw: bool,
    pub last_message_id: Option<Snowflake>,
    pub icon: Option<String>,
    pub permission_overrides: Vec<PermissionOverride>,
    pub created_at: DateTime<Utc>,
}
```

### 6.4 — Channel Ordering

Channels have a `position` field. When reordering:

```
PATCH /api/v1/servers/:server_id/channels
    Body: [
        { id: "channel_1", position: 0, parent_id: "category_1" },
        { id: "channel_2", position: 1, parent_id: "category_1" },
        { id: "channel_3", position: 0, parent_id: null }
    ]
    Response: 204 No Content
    Auth: MANAGE_CHANNELS
```

Ordering rules:
- Categories are ordered among themselves
- Channels within a category are ordered among themselves
- Channels without a category are ordered separately
- Voice channels appear after text channels in the same category

### 6.5 — Slowmode

`rate_limit` field on channels (0-21600 seconds = 0-6 hours).

When `rate_limit > 0`:
- After sending a message, user must wait `rate_limit` seconds before sending another
- Tracked per user per channel in Redis: `slowmode:{channel_id}:{user_id}` with TTL
- Users with MANAGE_MESSAGES or MANAGE_CHANNELS bypass slowmode
- Return `429 Too Many Requests` with `Retry-After` header if violated

### 6.6 — Channel Followers (Announcement Channels)

Announcement channels can be "followed" — messages published in them are crossposted to a target channel in another server.

```
POST /api/v1/channels/:channel_id/followers
    Body: { webhook_channel_id }
    Response: { channel_id, webhook_id }
    Auth: MANAGE_WEBHOOKS in target channel
```

When a message is published in an announcement channel:
1. Create a webhook message in each following channel
2. Set the `CROSSPOSTED` flag on the original message
3. Dispatch `MESSAGE_CREATE` in following channels

### 6.7 — Thread Channels (Stretch Goal)

Basic thread support:

```
POST /api/v1/channels/:channel_id/threads
    Body: { name, auto_archive_duration?: 1440, type?: 11 }
    Response: Thread channel object
    Note: Creates a thread from the channel (or from a specific message)

Thread types:
    11 = public thread
    12 = private thread
```

Threads are channels with:
- `parent_id` pointing to the text channel they belong to
- Auto-archive after inactivity (60, 1440, 4320, 10080 minutes)
- Member list (users who have joined the thread)

## WebSocket Events

- `CHANNEL_CREATE` — New channel created
- `CHANNEL_UPDATE` — Channel settings changed
- `CHANNEL_DELETE` — Channel deleted
- `CHANNEL_PINS_UPDATE` — Pin added/removed in a channel

## Acceptance Criteria

- [ ] All channel types can be created and managed
- [ ] Channel name validation (auto-hyphenation for text channels)
- [ ] Category nesting works (channels inside categories)
- [ ] Channel ordering persists and reorder endpoint works
- [ ] Slowmode enforced per user per channel with Redis tracking
- [ ] Users with MANAGE_MESSAGES bypass slowmode
- [ ] Max 500 channels per server enforced
- [ ] Deleting a category moves its channels to no-category
- [ ] Cannot delete last text channel in server
- [ ] Voice channel bitrate and user_limit validated
- [ ] Permission overrides stored and returned with channel
- [ ] All channel mutations dispatch WebSocket events
