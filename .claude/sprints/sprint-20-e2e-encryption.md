# Sprint 20 — E2E Encryption

**Status:** Not Started
**Dependencies:** Sprint 09
**Crates:** lumiere-server, lumiere-db, lumiere-gateway

## Goal

End-to-end encryption for DMs and optional server channels using the MLS (Messaging Layer Security) protocol. Key management, multi-device support, device verification, and encrypted message storage.

## Tasks

### 20.1 — Protocol Choice: MLS (RFC 9420)

MLS is chosen over Signal Protocol (Double Ratchet) because:
- Designed for group messaging (scales O(log n) for group operations vs O(n))
- IETF standard (RFC 9420)
- Better multi-device support
- Forward secrecy and post-compromise security
- Rust implementation available (`openmls` crate)

### 20.2 — Device Management

Each user can have multiple devices. Each device has its own key pair.

```
POST /api/v1/users/@me/devices/keys
    Body: {
        device_id: "unique_device_id",
        identity_key: "base64_public_key",
        key_packages: [                    // Pre-uploaded key packages for async adds
            { key_package: "base64_mls_key_package" },
            ...
        ]
    }
    Auth: Authenticated user

GET /api/v1/users/:user_id/devices
    Response: [{
        device_id: "...",
        identity_key: "...",
        last_active: "...",
        device_name: "iPhone 15",
    }]

DELETE /api/v1/users/@me/devices/:device_id
    Action: Remove device, revoke its keys, update all groups
```

### 20.3 — Key Package Distribution

MLS requires pre-uploaded key packages so users can be added to groups asynchronously:

```
GET /api/v1/users/:user_id/devices/:device_id/key-package
    Response: { key_package: "base64" }
    Action: Returns and consumes ONE key package (one-time use)
    Note: Client should pre-upload 100 key packages and replenish when low

POST /api/v1/users/@me/devices/:device_id/key-packages
    Body: { key_packages: ["base64", "base64", ...] }
    Action: Upload batch of key packages (max 100)
```

### 20.4 — MLS Group Management

For each E2E encrypted channel/DM, there is an MLS group:

```
POST /api/v1/channels/:channel_id/e2ee/group
    Body: { group_info: "base64_mls_group_info" }
    Auth: Channel admin
    Action: Initialize MLS group for this channel

POST /api/v1/channels/:channel_id/e2ee/commit
    Body: {
        commit: "base64_mls_commit",
        welcome: "base64_mls_welcome",  // For new members
    }
    Auth: Group member
    Action: Apply MLS commit (add member, remove member, update keys)

GET /api/v1/channels/:channel_id/e2ee/group-info
    Response: { group_info: "base64" }
    Auth: Group member
```

### 20.5 — Encrypted Message Format

E2E encrypted messages are stored as opaque blobs:

```rust
pub struct EncryptedMessage {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub author_id: Snowflake,
    pub ciphertext: Vec<u8>,           // MLS encrypted payload
    pub epoch: u64,                     // MLS epoch (key generation)
    pub content_type: String,           // "mls_application_message"
    pub sender_device_id: String,
}
```

The server:
- **Cannot** read message content (it's encrypted)
- **Can** see metadata: who sent it, when, to which channel
- Stores the ciphertext in ScyllaDB as-is
- Delivers via the same NATS → WebSocket pipeline

### 20.6 — ScyllaDB Schema for Encrypted Messages

```cql
CREATE TABLE encrypted_messages (
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
```

### 20.7 — Device Verification

Users can verify each other's devices:

```
POST /api/v1/users/@me/devices/:device_id/verify
    Body: { target_user_id, target_device_id, verification_method: "qr"|"emoji" }

// QR code verification:
GET /api/v1/users/@me/devices/:device_id/verification-qr
    Response: { qr_data: "..." }

// Emoji verification:
POST /api/v1/users/@me/devices/:device_id/verification-emoji
    Response: { emojis: ["🐶", "🎸", "🌍", "🔑", "🏠", "🚀", "🎯"] }
    Note: Both devices should show same emoji sequence
```

### 20.8 — Key Backup & Recovery

Allow users to backup encryption keys:

```
POST /api/v1/users/@me/e2ee/backup
    Body: {
        encrypted_backup: "base64",        // Encrypted with user's recovery key
        backup_version: 1,
    }

GET /api/v1/users/@me/e2ee/backup
    Response: { encrypted_backup: "base64", version: 1 }

// Recovery key is derived from a passphrase (PBKDF2/Argon2)
// The server NEVER sees the recovery key or plaintext backup
```

### 20.9 — Opt-in Encryption for Server Channels

By default, server channels are NOT encrypted (allows search, moderation, etc.).
Admins can enable E2E for specific channels:

```
PATCH /api/v1/channels/:channel_id
    Body: { e2ee_enabled: true }
    Auth: MANAGE_CHANNELS
    Note: Enabling E2E disables server-side search and auto-moderation for this channel
```

DMs are always E2E encrypted by default (can be disabled in user settings).

### 20.10 — Limitations of E2E Channels

When E2E is enabled:
- Server-side search does NOT work (content is encrypted)
- Auto-moderation does NOT work on message content
- Link previews are NOT generated server-side
- Push notification content shows "Encrypted message" instead of preview
- Message history for new members starts from when they joined the group

These trade-offs should be clearly communicated to users.

## Acceptance Criteria

- [ ] Device key pair generation and upload
- [ ] Key package upload and consumption (one-time use)
- [ ] MLS group creation for DM channels
- [ ] MLS commit processing (add member, remove member, update)
- [ ] Encrypted messages stored and delivered without server decryption
- [ ] Multi-device: message delivered to all user's devices
- [ ] New device can join existing groups via welcome message
- [ ] Device removal triggers key update (forward secrecy)
- [ ] Device verification via QR code and emoji sequence
- [ ] Key backup and recovery with passphrase
- [ ] Server channels can opt-in to E2E
- [ ] E2E channels correctly disable search/auto-mod
- [ ] Push notifications show "Encrypted message" for E2E content
- [ ] Integration test: device A sends encrypted message → device B decrypts → verify content matches
