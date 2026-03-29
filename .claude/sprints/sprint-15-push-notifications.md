# Sprint 15 — Push Notifications

**Status:** Not Started
**Dependencies:** Sprint 09
**Crates:** lumiere-push, lumiere-server

## Goal

Push notification delivery to mobile devices via APNs (iOS) and FCM (Android). Device registration, notification preferences, and a JetStream consumer that processes message events and sends pushes.

## Tasks

### 15.1 — APNs Client

```rust
// crates/lumiere-push/src/apns.rs
use a2::{Client, Notification, NotificationBuilder, PlainNotificationBuilder};

pub struct ApnsService {
    client: Client,
    topic: String,  // Bundle ID: com.lumiere.app
}

impl ApnsService {
    /// Initialize with p8 key file
    pub fn new(
        key_path: &str,
        key_id: &str,
        team_id: &str,
        topic: &str,
        production: bool,
    ) -> Result<Self> { ... }

    pub async fn send(&self, device_token: &str, notification: PushPayload) -> Result<()> {
        let builder = PlainNotificationBuilder::new(&notification.body)
            .set_title(&notification.title)
            .set_sound("default")
            .set_badge(notification.badge_count)
            .set_mutable_content()  // For notification extension
            .set_category(&notification.category);

        let mut payload = builder.build(device_token, Default::default());
        payload.add_custom_data("channel_id", &notification.channel_id)?;
        payload.add_custom_data("server_id", &notification.server_id)?;
        payload.add_custom_data("message_id", &notification.message_id)?;

        self.client.send(payload).await?;
        Ok(())
    }
}
```

### 15.2 — FCM Client

```rust
// crates/lumiere-push/src/fcm.rs

pub struct FcmService {
    client: reqwest::Client,
    server_key: String,
    project_id: String,
}

impl FcmService {
    pub async fn send(&self, device_token: &str, notification: PushPayload) -> Result<()> {
        // Use FCM v1 API (HTTP/2)
        let body = json!({
            "message": {
                "token": device_token,
                "notification": {
                    "title": notification.title,
                    "body": notification.body,
                },
                "data": {
                    "channel_id": notification.channel_id,
                    "server_id": notification.server_id,
                    "message_id": notification.message_id,
                    "type": notification.notification_type,
                },
                "android": {
                    "priority": "high",
                    "notification": {
                        "channel_id": "messages",
                        "click_action": "FLUTTER_NOTIFICATION_CLICK",
                        "sound": "default",
                    }
                }
            }
        });

        self.client.post(&format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            self.project_id
        ))
        .bearer_auth(&self.get_access_token().await?)
        .json(&body)
        .send()
        .await?;

        Ok(())
    }
}
```

### 15.3 — Push Payload

```rust
pub struct PushPayload {
    pub title: String,
    pub body: String,
    pub badge_count: u32,
    pub channel_id: String,
    pub server_id: Option<String>,
    pub message_id: String,
    pub notification_type: NotificationType,
    pub category: String,
}

pub enum NotificationType {
    Message,
    DirectMessage,
    Mention,
    FriendRequest,
    Call,
}
```

Notification content formatting:
- **Server message**: `[Server Name] #channel — Author: message preview`
- **DM**: `Author: message preview`
- **Mention**: `[Server Name] #channel — Author mentioned you: preview`
- **Call**: `Author is calling you`
- **Friend request**: `Author sent you a friend request`

Message preview: first 100 chars of content, strip markdown, replace mentions with @username.

### 15.4 — Device Token Registration

```
POST /api/v1/users/@me/devices
    Body: { platform: "ios"|"android", token: "device_token_string" }
    Response: { id, platform, token, created_at }
    Auth: Authenticated user

DELETE /api/v1/users/@me/devices/:device_id
    Response: 204 No Content

GET /api/v1/users/@me/devices
    Response: [{ id, platform, created_at }]
    Note: Token not returned for security
```

Handle token updates:
- If device sends a new token, update existing record (upsert on user_id + old_token)
- Delete stale tokens when APNs/FCM returns "invalid token" error

### 15.5 — Notification Preferences

```
PATCH /api/v1/users/@me/settings
    Body: {
        notification_settings: {
            dm_notifications: true,
            friend_request_notifications: true,
        }
    }

PATCH /api/v1/users/@me/servers/:server_id/notification-settings
    Body: {
        muted: false,
        suppress_everyone: false,
        suppress_roles: false,
        level: 1    // 0=default, 1=all_messages, 2=only_mentions, 3=nothing
    }

PATCH /api/v1/users/@me/channels/:channel_id/notification-settings
    Body: {
        muted: true,
        mute_until: "2026-04-01T00:00:00Z"  // null = indefinite
    }
```

### 15.6 — Push Notification Worker (JetStream Consumer)

```rust
pub async fn run_push_worker(
    jetstream: &async_nats::jetstream::Context,
    push: &PushService,
    db: &Database,
) -> Result<()> {
    let consumer = jetstream.get_or_create_consumer(
        "MESSAGES",
        async_nats::jetstream::consumer::pull::Config {
            durable_name: Some("push-worker".to_string()),
            filter_subject: "persist.messages.>".to_string(),
            ..Default::default()
        },
    ).await?;

    let mut messages = consumer.messages().await?;

    while let Some(msg) = messages.next().await {
        let msg = msg?;
        let event: MessageEvent = serde_json::from_slice(&msg.payload)?;

        if event.action == Action::Create {
            process_push_notification(&event.message, push, db).await?;
        }

        msg.ack().await?;
    }

    Ok(())
}

async fn process_push_notification(
    message: &Message,
    push: &PushService,
    db: &Database,
) -> Result<()> {
    // 1. Get channel members who should receive push
    // 2. Filter out:
    //    - Message author
    //    - Users who are online (have active WebSocket = already got the message)
    //    - Users who have muted the channel/server
    //    - Users whose notification level filters this message
    //    - Users who are in DND status
    // 3. For remaining users, get their device tokens
    // 4. Format notification for each user
    // 5. Send via APNs (iOS) or FCM (Android)
    // 6. Handle failures: retry transient errors, delete invalid tokens
}
```

### 15.7 — Online Status Check

Before sending push, check if user is online (has active gateway session):

```rust
// Redis check: does the user have an active gateway session?
async fn is_user_online(redis: &RedisClient, user_id: Snowflake) -> bool {
    // Check presence:{user_id} key exists in Redis
    // If online, they're getting messages via WebSocket — skip push
    redis.exists(format!("presence:{}", user_id)).await.unwrap_or(false)
}
```

### 15.8 — Badge Count

iOS badge shows total unread mention count. Calculate per user:

```rust
async fn get_badge_count(db: &Database, user_id: Snowflake) -> u32 {
    // Sum mention_count from all read_states where mention_count > 0
    // Plus unread DM count
}
```

## Acceptance Criteria

- [ ] iOS push via APNs works (test with real device)
- [ ] Android push via FCM works (test with real device)
- [ ] Device token registration and deregistration
- [ ] Stale/invalid tokens automatically cleaned up
- [ ] Push NOT sent to online users (they get WebSocket events)
- [ ] Push NOT sent for muted channels/servers
- [ ] Push NOT sent to DND users (except DMs with override)
- [ ] Notification level filtering works (all/mentions/nothing)
- [ ] @everyone/@here suppression per server
- [ ] Badge count accurate on iOS
- [ ] Call notifications delivered with high priority
- [ ] Notification content formatted correctly (server, channel, author)
- [ ] JetStream consumer processes messages reliably (at-least-once)
- [ ] Integration test: send message while user offline → verify push delivery
