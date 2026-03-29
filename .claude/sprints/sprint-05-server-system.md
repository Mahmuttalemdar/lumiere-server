# Sprint 05 — Server System

**Status:** Not Started
**Dependencies:** Sprint 03
**Crates:** lumiere-server, lumiere-db

## Goal

Full server (guild) management: creation, settings, invite system, member management, and server discovery.

## Tasks

### 5.1 — Server CRUD

```
POST /api/v1/servers
    Body: { name, icon?, region? }
    Response: Server object
    Actions:
        - Create server
        - Create @everyone role (id = server_id, position 0)
        - Create default channels: "general" (text), "General" (voice)
        - Add creator as owner/member
        - Assign @everyone role to creator

GET /api/v1/servers/:server_id
    Response: Server object with channels, roles, member_count
    Auth: Must be member

PATCH /api/v1/servers/:server_id
    Body: { name?, icon?, banner?, description?, region?, verification_level?,
            default_message_notifications?, explicit_content_filter?,
            system_channel_id?, rules_channel_id? }
    Response: Updated server object
    Auth: MANAGE_SERVER permission

DELETE /api/v1/servers/:server_id
    Response: 204 No Content
    Auth: Must be owner
    Note: Requires confirmation (password in body)
```

### 5.2 — Server Object

```rust
pub struct Server {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub description: Option<String>,
    pub owner_id: Snowflake,
    pub region: Option<String>,
    pub features: Vec<String>,
    pub verification_level: u8,
    pub default_message_notifications: u8,
    pub explicit_content_filter: u8,
    pub system_channel_id: Option<Snowflake>,
    pub rules_channel_id: Option<Snowflake>,
    pub max_members: u32,
    pub member_count: u32,
    pub roles: Vec<Role>,
    pub channels: Vec<Channel>,
    pub emojis: Vec<Emoji>,
    pub created_at: DateTime<Utc>,
}
```

### 5.3 — Invite System

```
POST /api/v1/channels/:channel_id/invites
    Body: { max_age?: 86400, max_uses?: 0, temporary?: false }
    Response: Invite object
    Auth: CREATE_INVITE permission
    Note: max_age=0 means never expire, max_uses=0 means unlimited

GET /api/v1/servers/:server_id/invites
    Response: [Invite objects with use count and inviter]
    Auth: MANAGE_SERVER permission

GET /api/v1/invites/:code
    Response: Invite preview (server name, icon, member count, channel name)
    Auth: None required (public preview)

POST /api/v1/invites/:code
    Response: Server object (joined)
    Auth: Authenticated user
    Actions:
        - Check invite validity (expired? max uses reached?)
        - Check user not already member
        - Check user not banned
        - Add user as member
        - Assign @everyone role
        - Increment invite use count
        - Dispatch GUILD_MEMBER_ADD event

DELETE /api/v1/invites/:code
    Response: 204 No Content
    Auth: MANAGE_SERVER permission or invite creator
```

Invite code generation: 8-character alphanumeric string (nanoid or similar).

### 5.4 — Member Management

```
GET /api/v1/servers/:server_id/members
    Query: ?limit=100&after=snowflake_id
    Response: [Member objects]
    Auth: Must be member
    Note: Cursor-based pagination using user_id

GET /api/v1/servers/:server_id/members/:user_id
    Response: Member object
    Auth: Must be member

PATCH /api/v1/servers/:server_id/members/:user_id
    Body: { nickname?, roles?: [role_ids], communication_disabled_until? }
    Response: Updated member object
    Auth: Varies by field:
        - nickname (self): CHANGE_NICKNAME
        - nickname (other): MANAGE_NICKNAMES
        - roles: MANAGE_ROLES (and can only assign roles below your highest)
        - timeout: MODERATE_MEMBERS

DELETE /api/v1/servers/:server_id/members/:user_id
    Response: 204 No Content
    Auth: KICK_MEMBERS permission
    Action: Kick member, dispatch GUILD_MEMBER_REMOVE

PATCH /api/v1/servers/:server_id/members/@me
    Body: { nickname? }
    Response: Updated member object
    Auth: CHANGE_NICKNAME

DELETE /api/v1/servers/:server_id/members/@me
    Response: 204 No Content
    Action: Leave server (owner cannot leave, must transfer first)
```

### 5.5 — Member Object

```rust
pub struct Member {
    pub user: PublicUser,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub roles: Vec<Snowflake>,
    pub joined_at: DateTime<Utc>,
    pub communication_disabled_until: Option<DateTime<Utc>>,
    pub flags: u64,
}
```

### 5.6 — Ban Management

```
GET /api/v1/servers/:server_id/bans
    Response: [{ user, reason }]
    Auth: BAN_MEMBERS

GET /api/v1/servers/:server_id/bans/:user_id
    Response: { user, reason }
    Auth: BAN_MEMBERS

PUT /api/v1/servers/:server_id/bans/:user_id
    Body: { reason?, delete_message_seconds?: 0 }
    Response: 204 No Content
    Auth: BAN_MEMBERS
    Actions:
        - Remove member if present
        - Create ban record
        - Optionally delete recent messages (up to 7 days)
        - Dispatch GUILD_BAN_ADD

DELETE /api/v1/servers/:server_id/bans/:user_id
    Response: 204 No Content
    Auth: BAN_MEMBERS
    Action: Remove ban, dispatch GUILD_BAN_REMOVE
```

### 5.7 — Ownership Transfer

```
PATCH /api/v1/servers/:server_id
    Body: { owner_id: new_owner_snowflake }
    Auth: Must be current owner
    Validation: New owner must be a server member
```

### 5.8 — Server List for User

```
GET /api/v1/users/@me/servers
    Response: [Partial server objects]
    Note: Returns all servers the user is a member of
```

## WebSocket Events Dispatched

- `GUILD_CREATE` — When user joins a server (sent to the joining user)
- `GUILD_UPDATE` — When server settings change
- `GUILD_DELETE` — When server is deleted or user is removed
- `GUILD_MEMBER_ADD` — New member joined
- `GUILD_MEMBER_UPDATE` — Member nickname/roles/timeout changed
- `GUILD_MEMBER_REMOVE` — Member left/kicked
- `GUILD_BAN_ADD` — User banned
- `GUILD_BAN_REMOVE` — User unbanned
- `INVITE_CREATE` — New invite created
- `INVITE_DELETE` — Invite deleted/expired

## Acceptance Criteria

- [ ] Server creation creates default channels and @everyone role
- [ ] Server deletion removes all associated data (cascade)
- [ ] Invite system: create, use, expire, max uses all work
- [ ] Invite preview works without authentication
- [ ] Banned users cannot rejoin via invite
- [ ] Member pagination works with cursor-based approach
- [ ] Nickname changes respect permissions
- [ ] Role assignment checks hierarchy (can't assign roles above your own)
- [ ] Owner transfer works and old owner loses owner-only permissions
- [ ] Owner cannot leave without transferring ownership
- [ ] All member changes dispatch appropriate WebSocket events
- [ ] Timeout (communication_disabled_until) prevents user from sending messages
