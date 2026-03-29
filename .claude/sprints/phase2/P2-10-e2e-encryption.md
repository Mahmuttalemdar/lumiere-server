# P2-10 — E2E Encryption (MLS Protocol)

**Status:** Not Started
**Dependencies:** None (independent, can start anytime)
**Crates:** lumiere-server, lumiere-db, lumiere-gateway

## Goal

Implement the MLS (Messaging Layer Security, RFC 9420) protocol for end-to-end encrypted channels and DMs. This is a differentiator from Discord.

## Tasks

### 10.1 — OpenMLS Integration

Add `openmls` crate to workspace:
- Key pair generation (Ed25519/X25519)
- MLS group creation and management
- Commit/proposal processing
- Welcome message generation

### 10.2 — Device Key Management

```
POST /api/v1/users/@me/devices/keys
    Body: { public_key, key_package }
    Action: Register device's MLS key package

GET /api/v1/users/:user_id/devices
    Response: [{ device_id, public_key, last_active }]

GET /api/v1/users/:user_id/devices/:device_id/key-package
    Response: Key package (one-time, consumed on use)

POST /api/v1/users/@me/devices/:device_id/key-packages
    Body: { key_packages: [...] }
    Action: Upload batch of one-time key packages
```

### 10.3 — MLS Group Lifecycle

```
POST /api/v1/channels/:channel_id/e2ee/group
    Action: Initialize MLS group for channel
    Body: { member_key_packages: [...] }

POST /api/v1/channels/:channel_id/e2ee/commit
    Action: Apply MLS commit (add/remove member, update keys)
    Body: { commit_message }

GET /api/v1/channels/:channel_id/e2ee/group-info
    Response: Current group state for new members
```

### 10.4 — Encrypted Message Storage

Use existing `encrypted_messages` ScyllaDB table:
- Store ciphertext (not plaintext) for E2E channels
- `sender_device_id` for multi-device support
- `epoch` for MLS epoch tracking
- Server cannot read message content

### 10.5 — Device Verification

- QR code verification (scan between devices)
- Emoji sequence verification (compare 6 emoji)
- Verification status stored in PostgreSQL

### 10.6 — Key Backup

- Passphrase-derived encryption for key backup
- Encrypted backup stored server-side
- Recovery flow on new device

## Acceptance Criteria

- [ ] MLS group can be created for a channel
- [ ] Messages encrypted client-side, stored as ciphertext
- [ ] New members receive welcome message with group state
- [ ] Multi-device: all user devices receive messages
- [ ] Device verification flow works
- [ ] Key backup and recovery works
- [ ] Server admin CANNOT read E2E messages
