# P2-05 — Push Notifications — Real APNs & FCM

**Status:** Not Started
**Dependencies:** P2-04 (consumer delivers push)
**Crates:** lumiere-push

## Goal

Replace stub APNs/FCM clients with real implementations. Move DeviceTokenStore from in-memory to PostgreSQL.

## Tasks

### 5.1 — APNs HTTP/2 Client

- Load `.p8` private key (ES256)
- Mint JWT for APNs authentication (team_id + key_id)
- Cache JWT (valid for 1 hour, refresh at 50 min)
- HTTP/2 POST to `https://api.push.apple.com/3/device/{token}`
- Handle responses: 200 (success), 410 (unregistered → remove token), 429 (rate limit)
- Connection pooling via `hyper` or `reqwest` HTTP/2

### 5.2 — FCM v1 Client

- Load service account JSON
- Generate OAuth2 access token via Google's token endpoint
- Cache token (valid ~1 hour, auto-refresh)
- POST to `https://fcm.googleapis.com/v1/projects/{project_id}/messages:send`
- Handle responses: 200 (success), NOT_FOUND/UNREGISTERED (remove token)
- Batch sending support

### 5.3 — PostgreSQL-Backed Device Token Store

Replace in-memory HashMap with PostgreSQL:
- Table `device_tokens` already exists in schema
- CRUD operations via sqlx
- Unique constraint on (user_id, token)
- Platform enum stored as SMALLINT

### 5.4 — Notification Preferences

Check before sending:
1. Global DM notification setting (user_settings.dm_notifications)
2. Per-server mute (notification_settings.muted)
3. Per-channel mute (notification_settings.muted)
4. @everyone/@roles suppression
5. Do not send to online users (check Redis presence)

### 5.5 — iOS Badge Count

Calculate unread count across all channels for badge number:
- Query read_states for channels with unread messages
- Sum mention_count for all unread channels
- Include in APNs payload as `badge` field

## Acceptance Criteria

- [ ] APNs delivers real push to iOS device
- [ ] FCM delivers real push to Android device
- [ ] Device tokens persist in PostgreSQL across restarts
- [ ] Invalid tokens auto-removed on delivery failure
- [ ] Online users don't receive push
- [ ] Muted channels/servers don't generate push
- [ ] iOS badge shows correct unread count
