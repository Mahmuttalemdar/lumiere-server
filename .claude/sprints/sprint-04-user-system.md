# Sprint 04 — User System

**Status:** Not Started
**Dependencies:** Sprint 03
**Crates:** lumiere-server, lumiere-db

## Goal

Full user management: profiles, friend system, user settings, status/presence, and user search.

## Tasks

### 4.1 — User Profile CRUD

```
GET /api/v1/users/@me
    Response: Full user object (includes email, settings)

GET /api/v1/users/:user_id
    Response: Public user object (no email, limited fields)

PATCH /api/v1/users/@me
    Body: { username?, avatar?, banner?, bio?, locale? }
    Response: Updated user object
    Notes:
        - username change: rate limited to 2 per hour
        - avatar/banner: accepts base64 image or null to remove

DELETE /api/v1/users/@me
    Body: { password }
    Response: 204 No Content
    Notes: Soft delete — marks account as deleted, removes from servers after 30 days
```

### 4.2 — User Object Definitions

```rust
// Full user (returned to self)
pub struct User {
    pub id: Snowflake,
    pub username: String,
    pub discriminator: u16,
    pub email: String,          // Only visible to self
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub bio: Option<String>,
    pub locale: String,
    pub flags: u64,
    pub premium_type: u8,
    pub is_bot: bool,
    pub created_at: DateTime<Utc>,
}

// Public user (returned to others)
pub struct PublicUser {
    pub id: Snowflake,
    pub username: String,
    pub discriminator: u16,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub bio: Option<String>,
    pub flags: u64,
    pub is_bot: bool,
}
```

### 4.3 — Friend System

```
GET /api/v1/users/@me/relationships
    Response: [{ id, type, user }]
    Types: 1=friend, 2=blocked, 3=incoming_request, 4=outgoing_request

POST /api/v1/users/@me/relationships
    Body: { username }  or  { user_id }
    Response: 204 No Content
    Action: Send friend request (or accept if incoming request exists)

DELETE /api/v1/users/@me/relationships/:user_id
    Response: 204 No Content
    Action: Remove friend, cancel request, or unblock

PUT /api/v1/users/@me/relationships/:user_id
    Body: { type: 2 }
    Response: 204 No Content
    Action: Block user (removes friendship if exists)
```

Friend request flow:
1. User A sends request → A gets type=4 (outgoing), B gets type=3 (incoming)
2. User B accepts (sends request back) → Both get type=1 (friend)
3. Either can remove at any time

Blocking:
- Removes existing friendship
- Blocked user cannot send friend requests
- Blocked user cannot send DMs
- Blocked user's messages are hidden (client-side)

### 4.4 — Direct Messages

```
POST /api/v1/users/@me/channels
    Body: { recipient_id }  or  { recipients: [id1, id2, ...] }
    Response: DM channel object
    Notes:
        - Single recipient: creates DM channel (type=1)
        - Multiple recipients: creates group DM (type=3, max 10 users)
        - Returns existing channel if already open

GET /api/v1/users/@me/channels
    Response: [DM channel objects]
    Notes: Returns all open DM channels for the user
```

### 4.5 — User Settings

```
GET /api/v1/users/@me/settings
    Response: Full settings object

PATCH /api/v1/users/@me/settings
    Body: { theme?, message_display?, locale?, ... }
    Response: Updated settings object
```

### 4.6 — Presence / Status

User status types: `online`, `idle`, `dnd` (do not disturb), `invisible` (appears offline), `offline` (actual).

```
PATCH /api/v1/users/@me/settings
    Body: { status: "dnd", custom_status: { text: "Coding", emoji: "💻", expires_at: "..." } }
```

Presence is stored in Redis:
```
Key: presence:{user_id}
Value: { status, custom_status, last_active, client_info }
TTL: 5 minutes (refreshed by heartbeat)
```

Presence is broadcast to:
- All friends of the user
- All servers the user is a member of (only to online members)

### 4.7 — User Notes

Private notes that a user can attach to another user (only visible to the note author):

```
GET /api/v1/users/:user_id/note
PUT /api/v1/users/:user_id/note
    Body: { note: "Met at conference" }
```

## Acceptance Criteria

- [ ] User can view and edit their own profile
- [ ] Other users see public profile only (no email)
- [ ] Avatar/banner upload changes the MinIO object key
- [ ] Friend request flow works: send → accept → friends
- [ ] Blocking prevents DMs and friend requests
- [ ] DM channel creation is idempotent (returns existing)
- [ ] Group DM limited to 10 recipients
- [ ] User settings persist and return correctly
- [ ] Presence updates broadcast to friends via NATS
- [ ] Presence expires after 5 min without heartbeat
- [ ] Account deletion is soft delete with 30-day grace period
- [ ] WebSocket events dispatched: RELATIONSHIP_ADD, RELATIONSHIP_REMOVE, PRESENCE_UPDATE
