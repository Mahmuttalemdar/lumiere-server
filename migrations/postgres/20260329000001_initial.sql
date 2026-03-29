-- Lumiere PostgreSQL Schema
-- All IDs are BIGINT (Snowflake IDs)

-- ============================================
-- USERS
-- ============================================
CREATE TABLE users (
    id              BIGINT PRIMARY KEY,
    username        VARCHAR(32) NOT NULL UNIQUE,
    discriminator   SMALLINT NOT NULL DEFAULT 0,
    email           VARCHAR(255) NOT NULL UNIQUE,
    password_hash   VARCHAR(255) NOT NULL,
    avatar          VARCHAR(255),
    banner          VARCHAR(255),
    bio             VARCHAR(190),
    locale          VARCHAR(10) NOT NULL DEFAULT 'en-US',
    flags           BIGINT NOT NULL DEFAULT 0,
    premium_type    SMALLINT NOT NULL DEFAULT 0,
    is_bot          BOOLEAN NOT NULL DEFAULT false,
    is_system       BOOLEAN NOT NULL DEFAULT false,
    deleted_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_users_username ON users(username);

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

CREATE INDEX idx_servers_owner ON servers(owner_id);

-- ============================================
-- CHANNELS
-- ============================================
CREATE TABLE channels (
    id              BIGINT PRIMARY KEY,
    server_id       BIGINT REFERENCES servers(id) ON DELETE CASCADE,
    parent_id       BIGINT REFERENCES channels(id) ON DELETE SET NULL,
    type            SMALLINT NOT NULL,
    name            VARCHAR(100),
    topic           VARCHAR(1024),
    position        INTEGER NOT NULL DEFAULT 0,
    bitrate         INTEGER,
    user_limit      INTEGER,
    rate_limit      INTEGER NOT NULL DEFAULT 0,
    nsfw            BOOLEAN NOT NULL DEFAULT false,
    last_message_id BIGINT,
    icon            VARCHAR(255),
    e2ee_enabled    BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_channels_server ON channels(server_id);
CREATE INDEX idx_channels_parent ON channels(parent_id);

-- Add foreign keys for server system/rules channels after channels table exists
ALTER TABLE servers ADD CONSTRAINT fk_servers_system_channel
    FOREIGN KEY (system_channel_id) REFERENCES channels(id) ON DELETE SET NULL;
ALTER TABLE servers ADD CONSTRAINT fk_servers_rules_channel
    FOREIGN KEY (rules_channel_id) REFERENCES channels(id) ON DELETE SET NULL;

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
    permissions     BIGINT NOT NULL DEFAULT 0,
    mentionable     BOOLEAN NOT NULL DEFAULT false,
    is_default      BOOLEAN NOT NULL DEFAULT false,
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
    avatar          VARCHAR(255),
    joined_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    communication_disabled_until TIMESTAMPTZ,
    flags           BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (server_id, user_id)
);

CREATE INDEX idx_members_user ON server_members(user_id);

-- ============================================
-- MEMBER ROLES (junction)
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
    target_id       BIGINT NOT NULL,
    target_type     SMALLINT NOT NULL,
    allow_bits      BIGINT NOT NULL DEFAULT 0,
    deny_bits       BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, target_id)
);

-- ============================================
-- RELATIONSHIPS (friends, blocks)
-- ============================================
CREATE TABLE relationships (
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    type            SMALLINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, target_id)
);

CREATE INDEX idx_relationships_target ON relationships(target_id);

-- ============================================
-- DM CHANNEL RECIPIENTS
-- ============================================
CREATE TABLE dm_recipients (
    channel_id      BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (channel_id, user_id)
);

CREATE INDEX idx_dm_recipients_user ON dm_recipients(user_id);

-- ============================================
-- SERVER INVITES
-- ============================================
CREATE TABLE invites (
    code            VARCHAR(16) PRIMARY KEY,
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    channel_id      BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    inviter_id      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    max_uses        INTEGER NOT NULL DEFAULT 0,
    uses            INTEGER NOT NULL DEFAULT 0,
    max_age         INTEGER NOT NULL DEFAULT 86400,
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
    type            SMALLINT NOT NULL DEFAULT 1,
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

CREATE INDEX idx_audit_server_time ON audit_log(server_id, created_at DESC);
CREATE INDEX idx_audit_user ON audit_log(server_id, user_id);

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
    dm_notifications BOOLEAN NOT NULL DEFAULT true,
    friend_request_notifications BOOLEAN NOT NULL DEFAULT true,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================
-- DEVICE TOKENS (Push Notifications)
-- ============================================
CREATE TABLE device_tokens (
    id              BIGINT PRIMARY KEY,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    platform        SMALLINT NOT NULL,
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
    server_id       BIGINT,
    channel_id      BIGINT,
    muted           BOOLEAN NOT NULL DEFAULT false,
    mute_until      TIMESTAMPTZ,
    suppress_everyone BOOLEAN NOT NULL DEFAULT false,
    suppress_roles  BOOLEAN NOT NULL DEFAULT false,
    level           SMALLINT NOT NULL DEFAULT 0,
    UNIQUE (user_id, server_id, channel_id)
);

-- ============================================
-- WARNINGS (moderation)
-- ============================================
CREATE TABLE warnings (
    id              BIGINT PRIMARY KEY,
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    moderator_id    BIGINT REFERENCES users(id) ON DELETE SET NULL,
    reason          VARCHAR(512) NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_warnings_server_user ON warnings(server_id, user_id);

-- ============================================
-- AUTO-MODERATION RULES
-- ============================================
CREATE TABLE auto_mod_rules (
    id              BIGINT PRIMARY KEY,
    server_id       BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    name            VARCHAR(100) NOT NULL,
    event_type      SMALLINT NOT NULL,
    trigger_type    SMALLINT NOT NULL,
    trigger_metadata JSONB NOT NULL DEFAULT '{}',
    actions         JSONB NOT NULL DEFAULT '[]',
    enabled         BOOLEAN NOT NULL DEFAULT true,
    exempt_roles    BIGINT[] NOT NULL DEFAULT '{}',
    exempt_channels BIGINT[] NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_auto_mod_server ON auto_mod_rules(server_id);

-- ============================================
-- APPLICATIONS (bot framework)
-- ============================================
CREATE TABLE applications (
    id              BIGINT PRIMARY KEY,
    owner_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            VARCHAR(100) NOT NULL,
    description     VARCHAR(400),
    icon            VARCHAR(255),
    bot_id          BIGINT UNIQUE REFERENCES users(id) ON DELETE SET NULL,
    bot_token_hash  VARCHAR(255),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================
-- APPLICATION COMMANDS (slash commands)
-- ============================================
CREATE TABLE application_commands (
    id              BIGINT PRIMARY KEY,
    application_id  BIGINT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    server_id       BIGINT REFERENCES servers(id) ON DELETE CASCADE,
    name            VARCHAR(32) NOT NULL,
    description     VARCHAR(100) NOT NULL,
    options         JSONB NOT NULL DEFAULT '[]',
    default_permission BOOLEAN NOT NULL DEFAULT true,
    type            SMALLINT NOT NULL DEFAULT 1,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_commands_app ON application_commands(application_id);
CREATE INDEX idx_commands_server ON application_commands(server_id);
CREATE UNIQUE INDEX idx_commands_unique ON application_commands(application_id, server_id, name) WHERE server_id IS NOT NULL;
CREATE UNIQUE INDEX idx_commands_unique_global ON application_commands(application_id, name) WHERE server_id IS NULL;

-- ============================================
-- USER NOTES
-- ============================================
CREATE TABLE user_notes (
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    note            VARCHAR(256) NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, target_id)
);

-- ============================================
-- REPORTS
-- ============================================
CREATE TABLE reports (
    id              BIGINT PRIMARY KEY,
    reporter_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_type     VARCHAR(20) NOT NULL,
    target_id       BIGINT NOT NULL,
    reason          SMALLINT NOT NULL,
    description     VARCHAR(1000),
    status          SMALLINT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================
-- ATTACHMENTS METADATA
-- ============================================
CREATE TABLE attachments (
    id              BIGINT PRIMARY KEY,
    channel_id      BIGINT NOT NULL,
    message_id      BIGINT,
    uploader_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename        VARCHAR(255) NOT NULL,
    content_type    VARCHAR(100) NOT NULL,
    size            BIGINT NOT NULL,
    width           INTEGER,
    height          INTEGER,
    object_key      VARCHAR(512) NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_attachments_message ON attachments(message_id);
CREATE INDEX idx_attachments_orphan ON attachments(message_id) WHERE message_id IS NULL;

-- ============================================
-- Updated_at trigger function
-- ============================================
CREATE OR REPLACE FUNCTION update_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply to all tables with updated_at
CREATE TRIGGER trg_users_updated_at BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_servers_updated_at BEFORE UPDATE ON servers FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_channels_updated_at BEFORE UPDATE ON channels FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_roles_updated_at BEFORE UPDATE ON roles FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_user_settings_updated_at BEFORE UPDATE ON user_settings FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_webhooks_updated_at BEFORE UPDATE ON webhooks FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_device_tokens_updated_at BEFORE UPDATE ON device_tokens FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_applications_updated_at BEFORE UPDATE ON applications FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_auto_mod_rules_updated_at BEFORE UPDATE ON auto_mod_rules FOR EACH ROW EXECUTE FUNCTION update_updated_at();
