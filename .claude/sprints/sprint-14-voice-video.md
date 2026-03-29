# Sprint 14 — Voice & Video

**Status:** Not Started
**Dependencies:** Sprint 08
**Crates:** lumiere-voice, lumiere-server, lumiere-gateway

## Goal

Voice and video communication using LiveKit as the WebRTC SFU. Voice channel join/leave, voice states, screen sharing, and video calls in DMs/group DMs.

## Tasks

### 14.1 — LiveKit Integration

```rust
// crates/lumiere-voice/src/lib.rs
use livekit_api::services::room::RoomClient;
use livekit_api::access_token::{AccessToken, VideoGrant};

pub struct VoiceService {
    room_client: RoomClient,
    api_key: String,
    api_secret: String,
    livekit_url: String,
}

impl VoiceService {
    /// Generate a LiveKit access token for a user to join a room
    pub fn create_token(
        &self,
        user_id: Snowflake,
        username: String,
        room_name: String,
        can_publish: bool,
        can_subscribe: bool,
    ) -> Result<String> {
        let grant = VideoGrant {
            room_join: true,
            room: room_name.clone(),
            can_publish,
            can_subscribe,
            can_publish_data: true,
            ..Default::default()
        };

        let token = AccessToken::with_api_key(&self.api_key, &self.api_secret)
            .with_identity(&user_id.to_string())
            .with_name(&username)
            .with_grants(grant)
            .to_jwt()?;

        Ok(token)
    }

    /// Create a LiveKit room
    pub async fn create_room(&self, name: &str, max_participants: u32) -> Result<()> { ... }

    /// List participants in a room
    pub async fn list_participants(&self, room: &str) -> Result<Vec<Participant>> { ... }

    /// Mute/unmute a participant (server-side)
    pub async fn mute_participant(&self, room: &str, identity: &str, muted: bool) -> Result<()> { ... }

    /// Remove participant from room
    pub async fn remove_participant(&self, room: &str, identity: &str) -> Result<()> { ... }
}
```

### 14.2 — Voice State

Track which users are in which voice channels:

```rust
pub struct VoiceState {
    pub server_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub user_id: Snowflake,
    pub session_id: String,
    pub deaf: bool,              // Server-deafened
    pub mute: bool,              // Server-muted
    pub self_deaf: bool,         // Self-deafened
    pub self_mute: bool,         // Self-muted
    pub self_stream: bool,       // Streaming (Go Live)
    pub self_video: bool,        // Camera on
    pub suppress: bool,          // Suppressed in stage channel
    pub request_to_speak_timestamp: Option<DateTime<Utc>>,
}
```

Voice states stored in Redis (ephemeral):
```
voice_state:{server_id}:{user_id} → VoiceState JSON
voice_channel:{channel_id} → Set of user_ids
```

### 14.3 — Join Voice Channel

Client sends `VoiceStateUpdate` via WebSocket:

```json
{
    "op": 4,
    "d": {
        "server_id": "123",
        "channel_id": "456",    // null to disconnect
        "self_mute": false,
        "self_deaf": false
    }
}
```

Server processing:
1. Check CONNECT permission in channel
2. Check SPEAK permission (for can_publish)
3. Check channel user_limit (0 = unlimited)
4. If user is in another voice channel: disconnect from current
5. Create/update voice state in Redis
6. Map channel to LiveKit room: `{server_id}_{channel_id}`
7. Generate LiveKit access token
8. Send `VOICE_STATE_UPDATE` to server members via NATS
9. Send `VOICE_SERVER_UPDATE` to the user with LiveKit connection info:

```json
{
    "op": 0,
    "t": "VOICE_SERVER_UPDATE",
    "d": {
        "token": "livekit_jwt_token",
        "endpoint": "wss://livekit.lumiere.app"
    }
}
```

### 14.4 — Leave Voice Channel

Send VoiceStateUpdate with `channel_id: null`:
1. Remove voice state from Redis
2. Remove user from LiveKit room
3. Dispatch VOICE_STATE_UPDATE with channel_id=null

### 14.5 — Voice Channel Move

Users with MOVE_MEMBERS permission can move others:

```
PATCH /api/v1/servers/:server_id/members/:user_id
    Body: { channel_id: "new_voice_channel_id" }
    Auth: MOVE_MEMBERS permission
    Actions:
        - Remove from current LiveKit room
        - Add to new LiveKit room
        - Update voice state
        - Dispatch VOICE_STATE_UPDATE
```

### 14.6 — Server Mute/Deafen

```
PATCH /api/v1/servers/:server_id/members/:user_id
    Body: { mute: true }  or  { deaf: true }
    Auth: MUTE_MEMBERS or DEAFEN_MEMBERS permission
    Actions:
        - Update voice state in Redis
        - Update LiveKit participant permissions (mute audio track)
        - Dispatch VOICE_STATE_UPDATE
```

### 14.7 — Screen Sharing (Go Live)

When a user starts screen sharing:
1. Client updates: `self_stream: true`
2. LiveKit handles the screen share track automatically
3. Server updates voice state and dispatches event
4. Other clients in the channel can subscribe to the screen share track

Permission: STREAM permission required.

### 14.8 — DM Voice/Video Calls

Voice/video calls in DMs and Group DMs:

```
POST /api/v1/channels/:channel_id/call
    Response: { voice_token, endpoint }
    Auth: DM participant
    Actions:
        - Create LiveKit room: dm_{channel_id}
        - Generate token for caller
        - Send CALL_CREATE event to other DM participants (ring notification)

POST /api/v1/channels/:channel_id/call/join
    Response: { voice_token, endpoint }
    Action: Join existing call

DELETE /api/v1/channels/:channel_id/call
    Action: End call (if last participant leaves)
```

Ring notification via NATS → push notification to offline users.

### 14.9 — LiveKit Webhook Handler

LiveKit sends webhooks for room events:

```
POST /api/v1/webhooks/livekit
    Events:
        - participant_joined → update voice state
        - participant_left → remove voice state, dispatch event
        - track_published → update self_stream/self_video state
        - track_unpublished → update state
        - room_finished → cleanup
```

Validate webhook signature using LiveKit API key/secret.

### 14.10 — Voice Channel Status

Include voice state information in channel objects:

```rust
pub struct VoiceChannelStatus {
    pub channel_id: Snowflake,
    pub connected_users: Vec<VoiceState>,
}
```

Sent in READY event for all voice channels in the user's servers.

## Acceptance Criteria

- [ ] User can join a voice channel via WebSocket VoiceStateUpdate
- [ ] LiveKit token generated with correct permissions (publish/subscribe)
- [ ] Voice state tracked in Redis and broadcast to server members
- [ ] User limit on voice channels enforced
- [ ] Disconnecting from voice cleans up state
- [ ] Server mute/deafen works via LiveKit server-side control
- [ ] Screen sharing (Go Live) works with STREAM permission
- [ ] DM voice/video calls with ring notification
- [ ] Moving users between voice channels works
- [ ] LiveKit webhooks update voice state on participant join/leave
- [ ] READY event includes current voice states
- [ ] Integration test: join voice → verify state → leave → verify cleanup
