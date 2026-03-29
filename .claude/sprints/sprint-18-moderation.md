# Sprint 18 — Moderation System

**Status:** Not Started
**Dependencies:** Sprint 07, Sprint 09
**Crates:** lumiere-server, lumiere-db

## Goal

Server moderation tools: kick, ban, timeout, audit log, auto-moderation (word filter, spam detection, invite link filter), and report system.

## Tasks

### 18.1 — Manual Moderation Actions

Already partially covered in Sprint 05 (kick/ban). This sprint adds:

**Timeout (Communication Disabled):**
```
PATCH /api/v1/servers/:server_id/members/:user_id
    Body: { communication_disabled_until: "2026-04-01T00:00:00Z" }
    Auth: MODERATE_MEMBERS
    Max duration: 28 days
    Effect: User cannot send messages, add reactions, join voice, create threads
```

**Warn (custom):**
```
POST /api/v1/servers/:server_id/members/:user_id/warnings
    Body: { reason: "Spamming in general" }
    Auth: MODERATE_MEMBERS
    Actions:
        - Store warning in database
        - DM the user with warning details
        - Log to audit log

GET /api/v1/servers/:server_id/members/:user_id/warnings
    Auth: MODERATE_MEMBERS
    Response: [{ id, reason, moderator, created_at }]

DELETE /api/v1/servers/:server_id/warnings/:warning_id
    Auth: MODERATE_MEMBERS
```

### 18.2 — Audit Log

```
GET /api/v1/servers/:server_id/audit-log
    Query: ?user_id=X&action_type=Y&before=Z&limit=50
    Auth: VIEW_AUDIT_LOG
    Response: {
        audit_log_entries: [...],
        users: [...],   // Referenced users
    }
```

Audit log action types:
```rust
pub enum AuditLogActionType {
    // Server
    ServerUpdate = 1,

    // Channel
    ChannelCreate = 10,
    ChannelUpdate = 11,
    ChannelDelete = 12,
    ChannelOverrideCreate = 13,
    ChannelOverrideUpdate = 14,
    ChannelOverrideDelete = 15,

    // Member
    MemberKick = 20,
    MemberBan = 22,
    MemberUnban = 23,
    MemberUpdate = 24,     // Nickname, roles, timeout
    MemberRoleUpdate = 25,
    MemberMove = 26,       // Voice channel move
    MemberDisconnect = 27, // Voice disconnect

    // Role
    RoleCreate = 30,
    RoleUpdate = 31,
    RoleDelete = 32,

    // Invite
    InviteCreate = 40,
    InviteDelete = 42,

    // Webhook
    WebhookCreate = 50,
    WebhookUpdate = 51,
    WebhookDelete = 52,

    // Emoji
    EmojiCreate = 60,
    EmojiUpdate = 61,
    EmojiDelete = 62,

    // Message
    MessageDelete = 72,
    MessageBulkDelete = 73,
    MessagePin = 74,
    MessageUnpin = 75,

    // Auto-moderation
    AutoModRuleCreate = 140,
    AutoModRuleUpdate = 141,
    AutoModRuleDelete = 142,
    AutoModBlockMessage = 143,
    AutoModTimeout = 145,
}
```

Audit log entry format:
```rust
pub struct AuditLogEntry {
    pub id: Snowflake,
    pub server_id: Snowflake,
    pub user_id: Option<Snowflake>,  // Who performed the action
    pub target_id: Option<Snowflake>, // Who/what was affected
    pub action_type: AuditLogActionType,
    pub changes: Vec<AuditLogChange>,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct AuditLogChange {
    pub key: String,
    pub old_value: Option<serde_json::Value>,
    pub new_value: Option<serde_json::Value>,
}
```

All moderation endpoints accept `X-Audit-Log-Reason` header for the reason field.

### 18.3 — Auto-Moderation Rules

```
POST /api/v1/servers/:server_id/auto-moderation/rules
    Body: {
        name: "No bad words",
        event_type: 1,           // 1 = message_send
        trigger_type: 1,         // 1=keyword, 2=spam, 3=keyword_preset, 4=mention_spam
        trigger_metadata: {
            keyword_filter: ["badword1", "badword2"],
            regex_patterns: ["b[a@]d\\s*w[o0]rd"],
            allow_list: ["badword1context"],  // Exceptions
        },
        actions: [
            { type: 1 },                     // Block message
            { type: 2, metadata: { channel_id: "log_channel" } }, // Send alert to channel
            { type: 3, metadata: { duration_seconds: 300 } },     // Timeout user
        ],
        enabled: true,
        exempt_roles: ["mod_role_id"],
        exempt_channels: ["bot_channel_id"],
    }
    Auth: MANAGE_SERVER
```

Trigger types:
```rust
pub enum AutoModTriggerType {
    Keyword = 1,         // Custom word filter
    Spam = 2,            // ML-based spam detection (heuristic)
    KeywordPreset = 3,   // Built-in: profanity, slurs, sexual content
    MentionSpam = 4,     // Too many mentions in one message
    InviteLinks = 5,     // Block external invite links
}
```

Action types:
```rust
pub enum AutoModActionType {
    BlockMessage = 1,     // Prevent the message from being sent
    SendAlertMessage = 2, // Send alert to a designated channel
    Timeout = 3,          // Timeout the user for X seconds
}
```

### 18.4 — Auto-Moderation Execution

When a message is sent, before persisting:

```rust
pub async fn check_auto_moderation(
    message: &CreateMessageRequest,
    member: &Member,
    channel: &Channel,
    rules: &[AutoModRule],
) -> AutoModResult {
    for rule in rules {
        if !rule.enabled { continue; }
        if rule.exempt_roles.iter().any(|r| member.roles.contains(r)) { continue; }
        if rule.exempt_channels.contains(&channel.id) { continue; }

        let triggered = match rule.trigger_type {
            Keyword => check_keyword_filter(&message.content, &rule.trigger_metadata),
            Spam => check_spam_heuristic(&message.content),
            KeywordPreset => check_preset_filter(&message.content, &rule.trigger_metadata),
            MentionSpam => check_mention_count(&message.content, &rule.trigger_metadata),
            InviteLinks => check_invite_links(&message.content),
        };

        if triggered {
            return AutoModResult::Triggered {
                rule_id: rule.id,
                actions: rule.actions.clone(),
                matched_content: extract_match(&message.content, &rule),
            };
        }
    }

    AutoModResult::Allowed
}
```

Spam heuristic checks:
- Repeated characters (aaaaaaa)
- Repeated messages (same content within 10 seconds)
- Excessive caps (>70% uppercase in messages > 10 chars)
- Excessive emoji (>10 emoji per message)
- Message rate (>5 messages in 5 seconds)

### 18.5 — Keyword Filter

Pattern matching:
- Exact match: `"badword"` matches "badword" but not "badwording"
- Wildcard: `"*badword*"` matches "thisbadwordhere"
- Regex support for advanced patterns
- Case-insensitive by default
- Unicode normalization (catch leetspeak variants)

### 18.6 — Report System

```
POST /api/v1/report
    Body: {
        type: "message"|"user"|"server",
        target_id: "...",
        reason: 1,  // 1=spam, 2=harassment, 3=nsfw, 4=self_harm, 5=other
        description: "Details..."
    }
    Auth: Any authenticated user
    Actions:
        - Store report in database
        - Notify server admins (if server-level report)
        - Queue for platform-level review (if applicable)
```

### 18.7 — Moderation Logging Channel

Server setting for a dedicated moderation log channel:

```
PATCH /api/v1/servers/:server_id
    Body: { system_channel_id: "mod_log_channel_id" }
```

Auto-moderation alerts and manual moderation actions post to this channel as system messages.

## Acceptance Criteria

- [ ] Timeout prevents user from sending messages, reactions, voice
- [ ] Timeout auto-expires at the specified time
- [ ] Warning system: create, list, delete warnings
- [ ] Audit log records all moderation actions with before/after changes
- [ ] X-Audit-Log-Reason header captured in audit entries
- [ ] Auto-moderation keyword filter catches configured words
- [ ] Auto-moderation regex patterns work
- [ ] Auto-moderation actions execute: block, alert, timeout
- [ ] Exempt roles and channels respected in auto-moderation
- [ ] Spam heuristic detects obvious spam patterns
- [ ] Mention spam detection (configurable threshold)
- [ ] Invite link filtering works
- [ ] Report system stores reports
- [ ] Moderation log channel receives formatted alerts
- [ ] Integration test: send message with bad word → verify blocked → verify audit log
