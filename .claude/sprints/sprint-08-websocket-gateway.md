# Sprint 08 — WebSocket Gateway

**Status:** Not Started
**Dependencies:** Sprint 03, Sprint 07
**Crates:** lumiere-gateway, lumiere-nats, lumiere-server

## Goal

Build the real-time WebSocket gateway: connection lifecycle, heartbeat, session resume, NATS integration for event fanout, and presence tracking.

## Tasks

### 8.1 — Gateway Protocol

WebSocket endpoint: `ws://host/gateway`

All messages are JSON. Each message has an opcode:

```rust
pub enum OpCode {
    Dispatch = 0,        // Server → Client: Event dispatch
    Heartbeat = 1,       // Client → Server: Heartbeat ping
    Identify = 2,        // Client → Server: Auth + session start
    PresenceUpdate = 3,  // Client → Server: Update presence
    VoiceStateUpdate = 4,// Client → Server: Join/leave voice
    Resume = 6,          // Client → Server: Resume disconnected session
    Reconnect = 7,       // Server → Client: Please reconnect
    InvalidSession = 9,  // Server → Client: Session invalid, re-identify
    Hello = 10,          // Server → Client: Sent on connect, contains heartbeat_interval
    HeartbeatAck = 11,   // Server → Client: Heartbeat acknowledged
}
```

### 8.2 — Connection Lifecycle

```
Client connects via WebSocket
    ↓
Server sends Hello { heartbeat_interval: 41250 }
    ↓
Client sends Identify { token, properties, presence }
    ↓
Server validates token
    ↓
Server sends Ready {
    user, servers, private_channels, session_id, resume_gateway_url
}
    ↓
Client starts heartbeat loop
    ↓
Server streams Dispatch events
    ↓
On disconnect: client attempts Resume with session_id + last sequence
```

### 8.3 — Identify Payload

```rust
pub struct IdentifyPayload {
    pub token: String,
    pub properties: ConnectionProperties,
    pub presence: Option<PresenceUpdate>,
    pub compress: Option<bool>,     // zlib-stream compression
    pub large_threshold: Option<u8>, // 50-250, member count threshold for lazy loading
}

pub struct ConnectionProperties {
    pub os: String,
    pub browser: String,
    pub device: String,
}
```

On Identify:
1. Validate access token
2. Create session in Redis: `gateway_session:{session_id}` → { user_id, sequence, events_buffer }
3. Subscribe to NATS subjects for all user's servers and DMs
4. Load user data, server list, DM channels
5. Send Ready event
6. Broadcast PRESENCE_UPDATE to friends and servers

### 8.4 — Session Resume

When a client disconnects and reconnects quickly:

```rust
pub struct ResumePayload {
    pub token: String,
    pub session_id: String,
    pub sequence: u64,  // Last received sequence number
}
```

On Resume:
1. Validate token and session_id
2. Look up session in Redis
3. Replay all events after the given sequence number
4. Continue normal operation

Sessions expire from Redis after 5 minutes — after that, client must re-identify.

### 8.5 — Heartbeat System

- Server sends `heartbeat_interval` in Hello (41250ms ≈ 41.25 seconds)
- Client must send Heartbeat every `heartbeat_interval` ms
- Server responds with HeartbeatAck
- If server doesn't receive heartbeat within `heartbeat_interval * 1.5`, close connection
- If client doesn't receive HeartbeatAck, attempt reconnect

### 8.6 — NATS Integration

Each gateway connection subscribes to NATS subjects:

```
// Per-user subjects (DMs, friend events, presence)
user.{user_id}

// Per-server subjects (server events, channel messages)
server.{server_id}

// Per-channel subjects (typing indicators, specific channel events)
channel.{channel_id}
```

When an API endpoint needs to dispatch an event:
1. Publish to appropriate NATS subject
2. All gateway instances subscribed to that subject receive it
3. Each gateway instance fans out to connected WebSocket clients

```rust
// crates/lumiere-nats/src/lib.rs

pub struct NatsDispatcher {
    client: async_nats::Client,
}

impl NatsDispatcher {
    /// Dispatch event to all members of a server
    pub async fn dispatch_server_event(
        &self,
        server_id: Snowflake,
        event: GatewayEvent,
    ) -> Result<()> {
        let subject = format!("server.{}", server_id);
        let payload = serde_json::to_vec(&event)?;
        self.client.publish(subject, payload.into()).await?;
        Ok(())
    }

    /// Dispatch event to a specific user (across all their connections)
    pub async fn dispatch_user_event(
        &self,
        user_id: Snowflake,
        event: GatewayEvent,
    ) -> Result<()> {
        let subject = format!("user.{}", user_id);
        let payload = serde_json::to_vec(&event)?;
        self.client.publish(subject, payload.into()).await?;
        Ok(())
    }

    /// Dispatch event to all users who can see a channel
    pub async fn dispatch_channel_event(
        &self,
        channel_id: Snowflake,
        event: GatewayEvent,
    ) -> Result<()> {
        let subject = format!("channel.{}", channel_id);
        let payload = serde_json::to_vec(&event)?;
        self.client.publish(subject, payload.into()).await?;
        Ok(())
    }
}
```

### 8.7 — Gateway Event Structure

```rust
pub struct GatewayMessage {
    pub op: OpCode,
    pub d: Option<serde_json::Value>,  // Event data
    pub s: Option<u64>,                // Sequence number (only for Dispatch)
    pub t: Option<String>,             // Event name (only for Dispatch)
}
```

Event names for Dispatch:
```
READY, RESUMED,
GUILD_CREATE, GUILD_UPDATE, GUILD_DELETE,
GUILD_MEMBER_ADD, GUILD_MEMBER_UPDATE, GUILD_MEMBER_REMOVE,
GUILD_ROLE_CREATE, GUILD_ROLE_UPDATE, GUILD_ROLE_DELETE,
GUILD_BAN_ADD, GUILD_BAN_REMOVE,
CHANNEL_CREATE, CHANNEL_UPDATE, CHANNEL_DELETE, CHANNEL_PINS_UPDATE,
MESSAGE_CREATE, MESSAGE_UPDATE, MESSAGE_DELETE, MESSAGE_DELETE_BULK,
MESSAGE_REACTION_ADD, MESSAGE_REACTION_REMOVE, MESSAGE_REACTION_REMOVE_ALL,
TYPING_START,
PRESENCE_UPDATE,
VOICE_STATE_UPDATE, VOICE_SERVER_UPDATE,
USER_UPDATE,
INVITE_CREATE, INVITE_DELETE,
```

### 8.8 — Connection State Machine

```rust
pub enum ConnectionState {
    Connected,        // WebSocket open, waiting for Identify
    Identifying,      // Identify received, processing
    Ready,            // Session active, dispatching events
    Resuming,         // Resume received, replaying events
    Disconnecting,    // Closing
}
```

### 8.9 — Gateway Session Manager

Manages all active WebSocket connections on this gateway instance:

```rust
pub struct GatewaySessionManager {
    /// Map of session_id → active connection handle
    sessions: DashMap<String, GatewaySession>,
    /// Map of user_id → list of session_ids (multi-device)
    user_sessions: DashMap<Snowflake, Vec<String>>,
}

pub struct GatewaySession {
    pub session_id: String,
    pub user_id: Snowflake,
    pub sequence: AtomicU64,
    pub sender: mpsc::Sender<GatewayMessage>,
    pub subscriptions: Vec<async_nats::Subscriber>,
    pub last_heartbeat: AtomicU64,
    pub connected_at: Instant,
}
```

### 8.10 — Compression (Optional)

Support `zlib-stream` compression for bandwidth reduction:
- Client requests compression in Identify
- All subsequent messages are zlib-compressed
- Use `flate2` crate

### 8.11 — Gateway Rate Limiting

Per-connection rate limits:
- 120 commands per 60 seconds
- Identify: 1 per 5 seconds
- Presence update: 5 per 60 seconds
- Exceeding limits → close connection with code 4008 (Rate Limited)

## Acceptance Criteria

- [ ] WebSocket connection establishes and Hello is sent
- [ ] Identify validates token and sends Ready with user data
- [ ] Heartbeat keeps connection alive, missed heartbeat closes connection
- [ ] Events dispatch to correct connections via NATS subjects
- [ ] Multiple connections per user (multi-device) work correctly
- [ ] Resume replays missed events from Redis session buffer
- [ ] Session expires from Redis after 5 min disconnect
- [ ] Gateway rate limiting enforced per connection
- [ ] Connection state machine prevents invalid transitions
- [ ] NATS subjects correctly scoped (server, channel, user)
- [ ] Integration test: connect → identify → receive events → disconnect → resume
- [ ] Load test: 1000 concurrent WebSocket connections
