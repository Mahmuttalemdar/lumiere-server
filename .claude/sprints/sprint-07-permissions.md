# Sprint 07 — Permission System

**Status:** Not Started
**Dependencies:** Sprint 05, Sprint 06
**Crates:** lumiere-permissions, lumiere-server

## Goal

Discord-style permission system with role hierarchy, permission bitfields, channel-level overrides, and a permission checking middleware.

## Tasks

### 7.1 — Permission Bitfield

64-bit integer where each bit represents a permission:

```rust
// crates/lumiere-permissions/src/lib.rs

bitflags::bitflags! {
    pub struct Permissions: u64 {
        // General
        const ADMINISTRATOR           = 1 << 0;
        const VIEW_AUDIT_LOG          = 1 << 1;
        const MANAGE_SERVER           = 1 << 2;
        const MANAGE_ROLES            = 1 << 3;
        const MANAGE_CHANNELS         = 1 << 4;
        const KICK_MEMBERS            = 1 << 5;
        const BAN_MEMBERS             = 1 << 6;
        const CREATE_INVITE           = 1 << 7;
        const CHANGE_NICKNAME         = 1 << 8;
        const MANAGE_NICKNAMES        = 1 << 9;
        const MANAGE_EMOJIS           = 1 << 10;
        const MANAGE_WEBHOOKS         = 1 << 11;
        const VIEW_CHANNEL            = 1 << 12;
        const MODERATE_MEMBERS        = 1 << 13;

        // Text
        const SEND_MESSAGES           = 1 << 14;
        const SEND_TTS_MESSAGES       = 1 << 15;
        const MANAGE_MESSAGES         = 1 << 16;
        const EMBED_LINKS             = 1 << 17;
        const ATTACH_FILES            = 1 << 18;
        const READ_MESSAGE_HISTORY    = 1 << 19;
        const MENTION_EVERYONE        = 1 << 20;
        const USE_EXTERNAL_EMOJIS     = 1 << 21;
        const ADD_REACTIONS           = 1 << 22;
        const USE_SLASH_COMMANDS      = 1 << 23;
        const SEND_MESSAGES_IN_THREADS = 1 << 24;
        const CREATE_PUBLIC_THREADS   = 1 << 25;
        const CREATE_PRIVATE_THREADS  = 1 << 26;
        const MANAGE_THREADS          = 1 << 27;

        // Voice
        const CONNECT                 = 1 << 28;
        const SPEAK                   = 1 << 29;
        const MUTE_MEMBERS            = 1 << 30;
        const DEAFEN_MEMBERS          = 1 << 31;
        const MOVE_MEMBERS            = 1 << 32;
        const USE_VAD                 = 1 << 33;  // Voice Activity Detection
        const PRIORITY_SPEAKER        = 1 << 34;
        const STREAM                  = 1 << 35;  // Go Live
        const USE_SOUNDBOARD          = 1 << 36;

        // Stage
        const REQUEST_TO_SPEAK        = 1 << 37;
    }
}
```

### 7.2 — Default Permissions

`@everyone` role (created with server) gets these by default:

```rust
const DEFAULT_EVERYONE: Permissions =
    VIEW_CHANNEL | SEND_MESSAGES | READ_MESSAGE_HISTORY |
    EMBED_LINKS | ATTACH_FILES | ADD_REACTIONS |
    USE_EXTERNAL_EMOJIS | CONNECT | SPEAK | USE_VAD |
    CHANGE_NICKNAME | CREATE_INVITE;
```

### 7.3 — Role CRUD

```
POST /api/v1/servers/:server_id/roles
    Body: { name, color?, hoist?, mentionable?, permissions? }
    Response: Role object
    Auth: MANAGE_ROLES

GET /api/v1/servers/:server_id/roles
    Response: [Role objects, sorted by position]
    Auth: Member of server

PATCH /api/v1/servers/:server_id/roles/:role_id
    Body: { name?, color?, hoist?, mentionable?, permissions?, position? }
    Response: Updated role object
    Auth: MANAGE_ROLES
    Validation: Cannot modify roles at or above your highest role position

DELETE /api/v1/servers/:server_id/roles/:role_id
    Response: 204 No Content
    Auth: MANAGE_ROLES
    Validation: Cannot delete @everyone role, cannot delete roles at or above yours

PATCH /api/v1/servers/:server_id/roles
    Body: [{ id, position }, ...]
    Response: [Updated role objects]
    Auth: MANAGE_ROLES
    Note: Bulk reorder
```

### 7.4 — Role Object

```rust
pub struct Role {
    pub id: Snowflake,
    pub server_id: Snowflake,
    pub name: String,
    pub color: u32,          // RGB integer (0 = no color)
    pub hoist: bool,         // Show separately in member list
    pub icon: Option<String>,
    pub position: i32,
    pub permissions: Permissions,
    pub mentionable: bool,
    pub is_default: bool,    // @everyone
    pub created_at: DateTime<Utc>,
}
```

### 7.5 — Channel Permission Overrides

Each channel can override permissions for specific roles or users:

```rust
pub struct PermissionOverride {
    pub id: Snowflake,       // Role ID or User ID
    pub r#type: OverrideType, // 0=role, 1=member
    pub allow: Permissions,   // Bits explicitly allowed
    pub deny: Permissions,    // Bits explicitly denied
}
```

```
PUT /api/v1/channels/:channel_id/permissions/:override_id
    Body: { type: 0, allow: "1024", deny: "2048" }
    Response: 204 No Content
    Auth: MANAGE_ROLES

DELETE /api/v1/channels/:channel_id/permissions/:override_id
    Response: 204 No Content
    Auth: MANAGE_ROLES
```

### 7.6 — Permission Calculation Algorithm

This is the core algorithm. It must match Discord's behavior exactly:

```rust
pub fn compute_permissions(
    member: &Member,
    server: &Server,
    roles: &[Role],
    channel: Option<&Channel>,
) -> Permissions {
    // Step 1: Server owner has ALL permissions
    if member.user.id == server.owner_id {
        return Permissions::all();
    }

    // Step 2: Calculate base permissions from roles
    // Start with @everyone role permissions
    let mut permissions = everyone_role.permissions;

    // OR together all role permissions
    for role_id in &member.roles {
        if let Some(role) = roles.iter().find(|r| r.id == *role_id) {
            permissions |= role.permissions;
        }
    }

    // Step 3: ADMINISTRATOR grants everything
    if permissions.contains(Permissions::ADMINISTRATOR) {
        return Permissions::all();
    }

    // Step 4: Apply channel overrides (if channel context)
    if let Some(channel) = channel {
        // 4a: Apply @everyone role override for this channel
        if let Some(override_) = channel.overrides.iter()
            .find(|o| o.id == server.id && o.r#type == OverrideType::Role)
        {
            permissions &= !override_.deny;
            permissions |= override_.allow;
        }

        // 4b: Apply role overrides (OR together all allow, OR together all deny)
        let mut role_allow = Permissions::empty();
        let mut role_deny = Permissions::empty();
        for role_id in &member.roles {
            if let Some(override_) = channel.overrides.iter()
                .find(|o| o.id == *role_id && o.r#type == OverrideType::Role)
            {
                role_allow |= override_.allow;
                role_deny |= override_.deny;
            }
        }
        permissions &= !role_deny;
        permissions |= role_allow;

        // 4c: Apply member-specific override (highest priority)
        if let Some(override_) = channel.overrides.iter()
            .find(|o| o.id == member.user.id && o.r#type == OverrideType::Member)
        {
            permissions &= !override_.deny;
            permissions |= override_.allow;
        }
    }

    permissions
}
```

### 7.7 — Permission Middleware

Axum middleware/extractor that checks permissions before handler execution:

```rust
// Usage in handlers:
async fn send_message(
    auth: AuthUser,
    Path(channel_id): Path<Snowflake>,
    RequirePermission(Permissions::SEND_MESSAGES): RequirePermission,
    Json(body): Json<CreateMessage>,
) -> Result<impl IntoResponse> {
    // If we reach here, user has SEND_MESSAGES in this channel
}
```

The `RequirePermission` extractor:
1. Loads the member's roles
2. Loads the channel's permission overrides
3. Runs `compute_permissions()`
4. Returns `403 Forbidden` if permission is missing

### 7.8 — Role Hierarchy Enforcement

When modifying roles or members:
- A user can only modify roles **below** their highest role position
- A user can only kick/ban members whose highest role is **below** theirs
- A user can only assign/remove roles **below** their highest role
- Server owner bypasses all hierarchy checks

```rust
pub fn can_modify_member(actor: &Member, target: &Member, roles: &[Role]) -> bool {
    let actor_highest = highest_role_position(actor, roles);
    let target_highest = highest_role_position(target, roles);
    actor_highest > target_highest
}
```

## WebSocket Events

- `GUILD_ROLE_CREATE`
- `GUILD_ROLE_UPDATE`
- `GUILD_ROLE_DELETE`

## Acceptance Criteria

- [ ] Permission bitfield correctly represents all permissions
- [ ] @everyone role created with default permissions on server creation
- [ ] Role CRUD with position management
- [ ] Channel permission overrides: allow and deny work correctly
- [ ] Permission calculation follows exact Discord algorithm (4-step)
- [ ] ADMINISTRATOR overrides everything
- [ ] Server owner has all permissions always
- [ ] Role hierarchy enforced on role assignment
- [ ] Role hierarchy enforced on kick/ban
- [ ] Users cannot modify roles at or above their position
- [ ] Permission middleware returns 403 with missing permission name
- [ ] Unit tests for every step of permission calculation
- [ ] Unit tests for hierarchy enforcement edge cases
- [ ] Integration test: create role → assign to member → verify channel access
