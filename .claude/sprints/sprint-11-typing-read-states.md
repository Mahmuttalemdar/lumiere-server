# Sprint 11 — Typing Indicators & Read States

**Status:** Not Started
**Dependencies:** Sprint 09
**Crates:** lumiere-gateway, lumiere-db, lumiere-server

## Goal

Real-time typing indicators and per-user per-channel read state tracking with unread counts and mention counts.

## Tasks

### 11.1 — Typing Indicators

```
POST /api/v1/channels/:channel_id/typing
    Response: 204 No Content
    Auth: SEND_MESSAGES permission
    Action: Dispatch TYPING_START event to channel
```

TYPING_START event:
```json
{
    "op": 0,
    "t": "TYPING_START",
    "d": {
        "channel_id": "123",
        "user_id": "456",
        "timestamp": 1234567890,
        "member": { ... }  // Server member object (if in server)
    }
}
```

Typing state:
- Published via NATS Core (fire-and-forget, no persistence needed)
- Client shows "User is typing..." for 10 seconds after TYPING_START
- Client sends POST /typing every 10 seconds while actively typing
- No explicit "stop typing" event — it just expires client-side
- Rate limit: 1 typing event per 10 seconds per channel

### 11.2 — Read States

Track what each user has read in each channel.

Storage in ScyllaDB (read_states table from Sprint 02):
```cql
-- Already defined:
-- PRIMARY KEY (user_id, channel_id)
-- Fields: last_message_id, mention_count, updated_at
```

### 11.3 — Acknowledge Messages (Mark as Read)

```
POST /api/v1/channels/:channel_id/messages/:message_id/ack
    Body: { mention_count?: 0 }
    Response: 204 No Content
    Auth: VIEW_CHANNEL permission
    Actions:
        - Update read_states in ScyllaDB: set last_message_id, reset mention_count
        - Dispatch MESSAGE_ACK event to user's other sessions
```

MESSAGE_ACK event (sent to the user only, across devices):
```json
{
    "op": 0,
    "t": "MESSAGE_ACK",
    "d": {
        "channel_id": "123",
        "message_id": "789",
        "mention_count": 0
    }
}
```

### 11.4 — Unread Tracking

For each channel a user can see, the client needs to know:
- Is there an unread message? (`channel.last_message_id > read_state.last_message_id`)
- How many mentions? (`read_state.mention_count`)

This data is sent in the READY event when the client connects:

```json
{
    "read_state": [
        {
            "channel_id": "123",
            "last_message_id": "456",
            "mention_count": 3
        },
        ...
    ]
}
```

### 11.5 — Mention Count Incrementing

When a message is sent that mentions a user:
1. JetStream consumer processes the message
2. For each mentioned user, increment their `mention_count` in read_states
3. This happens asynchronously — not in the message send path

```rust
// JetStream consumer: mention counter
async fn process_message_mentions(msg: &Message) -> Result<()> {
    let mentioned_users = &msg.mentions;

    // Also @everyone/@here mentions
    if msg.mention_everyone {
        // Get all channel members, add to mentioned_users
    }

    for user_id in mentioned_users {
        // Increment mention_count in ScyllaDB
        // ScyllaDB counter: UPDATE read_states SET mention_count = mention_count + 1
        // WHERE user_id = ? AND channel_id = ?
    }

    Ok(())
}
```

### 11.6 — Mark All as Read

```
POST /api/v1/servers/:server_id/ack
    Response: 204 No Content
    Action: Mark all channels in the server as read for the user
```

### 11.7 — Unread Badge Count

For mobile push notifications, the client needs a total unread count:

```
GET /api/v1/users/@me/unread
    Response: {
        total_mentions: 15,
        channels: [
            { channel_id: "123", unread: true, mention_count: 3 },
            ...
        ]
    }
```

## Performance Considerations

- Typing indicators use NATS Core only (no persistence, no JetStream)
- Read state writes are batched — debounce rapid acks (e.g., scrolling through messages)
- Mention count uses ScyllaDB counters for atomic increment without read-before-write
- Read state is loaded once on READY, then kept in sync via MESSAGE_ACK events

## Acceptance Criteria

- [ ] Typing indicator appears for channel members when a user types
- [ ] Typing indicator disappears after 10 seconds
- [ ] Typing is rate limited to 1 event per 10 seconds per channel
- [ ] Message ack updates read state in ScyllaDB
- [ ] Unread state calculated correctly (last_message_id comparison)
- [ ] Mention count increments when user is mentioned
- [ ] @everyone/@here increment mention count for all online members
- [ ] MESSAGE_ACK syncs across user's devices
- [ ] READY event includes current read states
- [ ] Mark all as read works for an entire server
- [ ] Total unread count endpoint returns correct numbers
