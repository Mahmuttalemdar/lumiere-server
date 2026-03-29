-- Messages table
-- Partition: (channel_id, bucket) where bucket = epoch_days / 10
-- Clustering: message_id DESC for reverse chronological order
CREATE TABLE IF NOT EXISTS messages (
    channel_id      BIGINT,
    bucket          INT,
    message_id      BIGINT,
    author_id       BIGINT,
    content         TEXT,
    type            SMALLINT,
    flags           BIGINT,
    edited_at       TIMESTAMP,
    embeds          TEXT,
    attachments     TEXT,
    mentions        TEXT,
    mention_roles   TEXT,
    mention_everyone BOOLEAN,
    reactions       TEXT,
    pinned          BOOLEAN,
    reference_id    BIGINT,
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

-- Read states: last read message per user per channel
CREATE TABLE IF NOT EXISTS read_states (
    user_id         BIGINT,
    channel_id      BIGINT,
    last_message_id BIGINT,
    mention_count   INT,
    updated_at      TIMESTAMP,
    PRIMARY KEY (user_id, channel_id)
);

-- Pins lookup
CREATE TABLE IF NOT EXISTS pins (
    channel_id      BIGINT,
    message_id      BIGINT,
    pinned_by       BIGINT,
    pinned_at       TIMESTAMP,
    PRIMARY KEY (channel_id, message_id)
) WITH CLUSTERING ORDER BY (message_id DESC);

-- Reactions per message per emoji per user
CREATE TABLE IF NOT EXISTS reactions (
    channel_id      BIGINT,
    message_id      BIGINT,
    emoji           TEXT,
    user_id         BIGINT,
    created_at      TIMESTAMP,
    PRIMARY KEY ((channel_id, message_id), emoji, user_id)
);

-- Encrypted messages (E2E encrypted channels)
CREATE TABLE IF NOT EXISTS encrypted_messages (
    channel_id      BIGINT,
    bucket          INT,
    message_id      BIGINT,
    author_id       BIGINT,
    sender_device_id TEXT,
    ciphertext      BLOB,
    epoch           BIGINT,
    content_type    TEXT,
    PRIMARY KEY ((channel_id, bucket), message_id)
) WITH CLUSTERING ORDER BY (message_id DESC);
