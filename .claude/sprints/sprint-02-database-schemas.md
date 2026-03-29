# Sprint 02 — Database Schemas & Snowflake ID

**Status:** Not Started
**Dependencies:** Sprint 01
**Crates:** lumiere-models, lumiere-db

## Goal

Implement the Snowflake ID generator, design and create all PostgreSQL schemas (metadata) and ScyllaDB schemas (messages), set up database connection layers and migration systems.

## Tasks

### 2.1 — Snowflake ID Generator

64-bit ID: `[41-bit timestamp][10-bit machine_id][12-bit sequence]`

```rust
// crates/lumiere-models/src/snowflake.rs

// Epoch: 2024-01-01T00:00:00Z (custom epoch, not Unix)
const LUMIERE_EPOCH: u64 = 1704067200000; // ms

pub struct SnowflakeGenerator {
    machine_id: u16,    // 0-1023
    sequence: AtomicU16, // 0-4095
    last_timestamp: AtomicU64,
}

impl SnowflakeGenerator {
    pub fn next_id(&self) -> Snowflake { ... }
}

// Snowflake type that implements Serialize, Display, FromStr
// Serializes as string in JSON (JS can't handle 64-bit ints)
pub struct Snowflake(pub u64);

impl Snowflake {
    pub fn timestamp(&self) -> u64 { ... }  // Extract creation time
    pub fn machine_id(&self) -> u16 { ... }
    pub fn sequence(&self) -> u16 { ... }
    pub fn created_at(&self) -> chrono::DateTime<Utc> { ... }
}
```

Key properties:
- Thread-safe (atomic operations)
- Monotonically increasing within a machine
- Globally unique across machines (machine_id)
- Time-extractable (no need for separate `created_at` column in some cases)
- JSON serialization as string (JavaScript BigInt safety)

### 2.2 — PostgreSQL Schema

```sql
-- migrations/postgres/001_initial.sql

-- ============================================
-- USERS
-- ============================================
CREATE TABLE users (
    id              BIGINT PRIMARY KEY,           -- Snowflake ID
    username        VARCHAR(32) NOT NULL UNIQUE,
    discriminator   SMALLINT NOT NULL DEFAULT 0,   -- #0001 style (0 = unique username)
    email           VARCHAR(255) NOT NULL UNIQUE,
    password_hash   VARCHAR(255) NOT NULL,
    avatar          VARCHAR(255),                  -- MinIO object key
    banner          VARCHAR(255),
    bio             VARCHAR(190),
    locale          VARCHAR(10) DEFAULT 'en-US',
    flags           BIGINT NOT NULL DEFAULT 0,     -- Bitfield: staff, verified, etc.
    premium_type    SMALLINT NOT NULL DEFAULT 0,
    is_bot          BOOLEAN NOT NULL DEFAULT false,
    is_system       BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================
-- SERVERS (guilds)
-- ============================================
CREATE TABLE servers (
    id              BIGINT PRIMARY KEY,
    name            VARCHAR(100) NOT NULL,
    icon            VARCHAR(255),
    banner          VARCHAR(255),
    description     VARCHAR(1000),
    owner_id        BIGINT NOT NULL REFERENCES users(id),
    region          VARCHAR(32),
    features        TEXT[] NOT NULL DEFAULT '{}',
    verification_level SMALLINT NOT NULL DEFAULT 0,
    default_message_notifications SMALLINT NOT NULL DEFAULT 0,
    explicit_content_filter SMALLINT NOT NULL DEFAULT 0,
    system_channel_id BIGINT,
    rules_channel_id BIGINT,
    max_members     INTEGER NOT NULL DEFAULT 500000,
    member_count    INTEGER NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================
-- CHANNELS
-- ============================================
CREATE TABLE channels (
    id              BIGINT PRIMARY KEY,
    server_id       BIGINT REFERENCES servers(id) ON DELETE CASCADE,
    parent_id       BIGINT REFERENCES channels(id) ON DELETE SET NULL,
    type            SMALLINT NOT NULL,
    -- Types: 0=text, 1=dm, 2=voice, 3=group_dm, 4=category, 5=announcement
    name            VARCHAR(100),
    topic           VARCHAR(1024),
    position        INTEGER NOT NULL DEFAULT 0,
    bitrate         INTEGER,                       -- Voice channels
    user_limit      INTEGER,                       -- Voice channels
    rate_limit       INTEGER NOT NULL DEFAULT 0,   -- Slowmode seconds
    nsfw            BOOLEAN NOT NULL DEFAULT false,
    last_message_id BIGINT,
    icon            VARCHAR(255),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_channels_server ON channels(server_id);
CREATE INDEX idx_channels_parent ON channels(parent_id);

-- ============================================
-- ROLES
-- ============================================
CREATE TABLE roles (
    id              BIGINT PRIMARY KEY,
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    name            VARCHAR(100) NOT NULL,
    color           INTEGER NOT NULL DEFAULT 0,
    hoist           BOOLEAN NOT NULL DEFAULT false,
    icon            VARCHAR(255),
    position        INTEGER NOT NULL DEFAULT 0,
    permissions     BIGINT NOT NULL DEFAULT 0,     -- Permission bitfield
    mentionable     BOOLEAN NOT NULL DEFAULT false,
    is_default      BOOLEAN NOT NULL DEFAULT false, -- @everyone role
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_roles_server ON roles(server_id);

-- ============================================
-- SERVER MEMBERS
-- ============================================
CREATE TABLE server_members (
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    nickname        VARCHAR(32),
    avatar          VARCHAR(255),                  -- Server-specific avatar
    joined_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    communication_disabled_until TIMESTAMPTZ,       -- Timeout
    flags           BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (server_id, user_id)
);

CREATE INDEX idx_members_user ON server_members(user_id);

-- ============================================
-- MEMBER ROLES (junction table)
-- ============================================
CREATE TABLE member_roles (
    server_id       BIGINT NOT NULL,
    user_id         BIGINT NOT NULL,
    role_id         BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (server_id, user_id, role_id),
    FOREIGN KEY (server_id, user_id) REFERENCES server_members(server_id, user_id) ON DELETE CASCADE
);

-- ============================================
-- CHANNEL PERMISSION OVERRIDES
-- ============================================
CREATE TABLE permission_overrides (
    channel_id      BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    target_id       BIGINT NOT NULL,               -- Role ID or User ID
    target_type     SMALLINT NOT NULL,              -- 0=role, 1=user
    allow           BIGINT NOT NULL DEFAULT 0,
    deny            BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, target_id)
);

-- ============================================
-- RELATIONSHIPS (friends, blocks)
-- ============================================
CREATE TABLE relationships (
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    type            SMALLINT NOT NULL,
    -- Types: 1=friend, 2=blocked, 3=incoming_request, 4=outgoing_request
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, target_id)
);

CREATE INDEX idx_relationships_target ON relationships(target_id);

-- ============================================
-- SERVER INVITES
-- ============================================
CREATE TABLE invites (
    code            VARCHAR(16) PRIMARY KEY,
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    channel_id      BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    inviter_id      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    max_uses        INTEGER NOT NULL DEFAULT 0,    -- 0 = unlimited
    uses            INTEGER NOT NULL DEFAULT 0,
    max_age         INTEGER NOT NULL DEFAULT 86400, -- seconds, 0 = never
    temporary       BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_invites_server ON invites(server_id);

-- ============================================
-- SERVER BANS
-- ============================================
CREATE TABLE bans (
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    reason          VARCHAR(512),
    banned_by       BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (server_id, user_id)
);

-- ============================================
-- WEBHOOKS
-- ============================================
CREATE TABLE webhooks (
    id              BIGINT PRIMARY KEY,
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    channel_id      BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    creator_id      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    name            VARCHAR(80) NOT NULL,
    avatar          VARCHAR(255),
    token           VARCHAR(68) NOT NULL UNIQUE,
    type            SMALLINT NOT NULL DEFAULT 1,   -- 1=incoming, 2=channel_follower
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================
-- EMOJIS
-- ============================================
CREATE TABLE emojis (
    id              BIGINT PRIMARY KEY,
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    name            VARCHAR(32) NOT NULL,
    creator_id      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    animated        BOOLEAN NOT NULL DEFAULT false,
    available       BOOLEAN NOT NULL DEFAULT true,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_emojis_server ON emojis(server_id);

-- ============================================
-- AUDIT LOG
-- ============================================
CREATE TABLE audit_log (
    id              BIGINT PRIMARY KEY,
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    user_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    target_id       BIGINT,
    action_type     SMALLINT NOT NULL,
    changes         JSONB,
    reason          VARCHAR(512),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_audit_server ON audit_log(server_id, created_at DESC);

-- ============================================
-- USER SETTINGS
-- ============================================
CREATE TABLE user_settings (
    user_id         BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    theme           VARCHAR(10) NOT NULL DEFAULT 'dark',
    message_display VARCHAR(10) NOT NULL DEFAULT 'cozy',
    locale          VARCHAR(10) NOT NULL DEFAULT 'en-US',
    show_current_game BOOLEAN NOT NULL DEFAULT true,
    inline_attachment_media BOOLEAN NOT NULL DEFAULT true,
    inline_embed_media BOOLEAN NOT NULL DEFAULT true,
    render_embeds   BOOLEAN NOT NULL DEFAULT true,
    render_reactions BOOLEAN NOT NULL DEFAULT true,
    animate_emoji   BOOLEAN NOT NULL DEFAULT true,
    enable_tts      BOOLEAN NOT NULL DEFAULT true,
    status          VARCHAR(10) NOT NULL DEFAULT 'online',
    custom_status   JSONB,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================
-- DEVICE TOKENS (Push Notifications)
-- ============================================
CREATE TABLE device_tokens (
    id              BIGINT PRIMARY KEY,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    platform        SMALLINT NOT NULL,             -- 0=ios, 1=android
    token           VARCHAR(512) NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(user_id, token)
);

CREATE INDEX idx_device_tokens_user ON device_tokens(user_id);

-- ============================================
-- NOTIFICATION SETTINGS (per server/channel)
-- ============================================
CREATE TABLE notification_settings (
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    server_id       BIGINT REFERENCES servers(id) ON DELETE CASCADE,
    channel_id      BIGINT REFERENCES channels(id) ON DELETE CASCADE,
    muted           BOOLEAN NOT NULL DEFAULT false,
    mute_until      TIMESTAMPTZ,
    suppress_everyone BOOLEAN NOT NULL DEFAULT false,
    suppress_roles  BOOLEAN NOT NULL DEFAULT false,
    level           SMALLINT NOT NULL DEFAULT 0,    -- 0=default, 1=all, 2=mentions, 3=none
    PRIMARY KEY (user_id, COALESCE(server_id, 0), COALESCE(channel_id, 0))
);
```

### 2.3 — ScyllaDB Schema

```cql
-- migrations/scylla/001_initial.cql

CREATE KEYSPACE IF NOT EXISTS lumiere
WITH replication = {
    'class': 'SimpleStrategy',
    'replication_factor': 1  -- 3 for production
};

USE lumiere;

-- ============================================
-- MESSAGES
-- Partition: (channel_id, bucket)
-- Clustering: message_id DESC
-- ============================================
CREATE TABLE messages (
    channel_id      BIGINT,
    bucket          INT,                           -- epoch_days / 10 (10-day windows)
    message_id      BIGINT,                        -- Snowflake ID
    author_id       BIGINT,
    content         TEXT,
    type            SMALLINT,                      -- 0=default, 1=recipient_add, 6=pin, 7=join, 19=reply, etc.
    flags           BIGINT,                        -- Bitfield: crossposted, suppress_embeds, etc.
    edited_at       TIMESTAMP,
    embeds          TEXT,                           -- JSON array of embeds
    attachments     TEXT,                           -- JSON array of attachment metadata
    mentions        TEXT,                           -- JSON array of mentioned user IDs
    mention_roles   TEXT,                           -- JSON array of mentioned role IDs
    mention_everyone BOOLEAN,
    reactions       TEXT,                           -- JSON: {"emoji": [user_ids]}
    pinned          BOOLEAN,
    reference_id    BIGINT,                        -- Reply/reference message ID
    reference_channel_id BIGINT,
    deleted         BOOLEAN,
    PRIMARY KEY ((channel_id, bucket), message_id)
) WITH CLUSTERING ORDER BY (message_id DESC)
  AND compaction = {
    'class': 'TimeWindowCompactionStrategy',
    'compaction_window_unit': 'DAYS',
    'compaction_window_size': 7
  }
  AND gc_grace_seconds = 864000
  AND default_time_to_live = 0;

-- ============================================
-- READ STATES
-- Tracks last read message per user per channel
-- ============================================
CREATE TABLE read_states (
    user_id         BIGINT,
    channel_id      BIGINT,
    last_message_id BIGINT,
    mention_count   INT,
    updated_at      TIMESTAMP,
    PRIMARY KEY (user_id, channel_id)
);

-- ============================================
-- PINS (lookup table)
-- ============================================
CREATE TABLE pins (
    channel_id      BIGINT,
    message_id      BIGINT,
    pinned_by       BIGINT,
    pinned_at       TIMESTAMP,
    PRIMARY KEY (channel_id, message_id)
) WITH CLUSTERING ORDER BY (message_id DESC);

-- ============================================
-- MESSAGE REACTIONS (denormalized for fast lookup)
-- ============================================
CREATE TABLE reactions (
    channel_id      BIGINT,
    message_id      BIGINT,
    emoji           TEXT,                          -- Unicode or custom emoji ID
    user_id         BIGINT,
    created_at      TIMESTAMP,
    PRIMARY KEY ((channel_id, message_id), emoji, user_id)
);
```

### 2.4 — Bucket Calculation Helper

```rust
// crates/lumiere-models/src/bucket.rs

const BUCKET_SIZE_DAYS: u32 = 10;

/// Calculate ScyllaDB bucket from a Snowflake ID
pub fn bucket_from_snowflake(snowflake: Snowflake) -> i32 {
    let timestamp_ms = snowflake.timestamp();
    let epoch_days = (timestamp_ms / 86_400_000) as u32;
    (epoch_days / BUCKET_SIZE_DAYS) as i32
}

/// Calculate bucket from a chrono DateTime
pub fn bucket_from_datetime(dt: DateTime<Utc>) -> i32 {
    let epoch_days = (dt.timestamp() / 86_400) as u32;
    (epoch_days / BUCKET_SIZE_DAYS) as i32
}

/// Get the range of buckets between two Snowflake IDs
pub fn bucket_range(from: Snowflake, to: Snowflake) -> Vec<i32> { ... }
```

### 2.5 — Database Connection Layer

```rust
// crates/lumiere-db/src/lib.rs
pub struct Database {
    pub pg: sqlx::PgPool,
    pub scylla: Arc<scylla::Session>,
}

impl Database {
    pub async fn connect(config: &AppConfig) -> Result<Self> { ... }
    pub async fn run_pg_migrations(&self) -> Result<()> { ... }
    pub async fn run_scylla_migrations(&self) -> Result<()> { ... }
}
```

### 2.6 — Migration System

- PostgreSQL: Use `sqlx migrate` with files in `migrations/postgres/`
- ScyllaDB: Custom migration runner that reads `.cql` files from `migrations/scylla/` and executes them in order

## Acceptance Criteria

- [ ] `Snowflake::next_id()` generates unique, monotonically increasing IDs
- [ ] Snowflake IDs serialize as strings in JSON
- [ ] `Snowflake::timestamp()` correctly extracts creation time
- [ ] PostgreSQL migrations run cleanly on fresh database
- [ ] ScyllaDB migrations create keyspace and tables
- [ ] All tables have proper indexes
- [ ] `Database::connect()` establishes connections to both PG and ScyllaDB
- [ ] Bucket calculation is correct for arbitrary timestamps
- [ ] Unit tests for Snowflake ID generation (uniqueness, ordering, thread safety)
- [ ] Unit tests for bucket calculation
