use std::collections::HashMap;
use std::sync::Arc;

use lumiere_models::snowflake::Snowflake;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum PushError {
    #[error("device token not found for user {0}")]
    TokenNotFound(Snowflake),

    #[error("APNs delivery failed: {0}")]
    ApnsError(String),

    #[error("FCM delivery failed: {0}")]
    FcmError(String),

    #[error("invalid device token: {0}")]
    InvalidToken(String),

    #[error("push service not configured for platform {0:?}")]
    PlatformNotConfigured(Platform),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type PushResult<T> = Result<T, PushError>;

// ---------------------------------------------------------------------------
// Platform & notification types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    Android,
}

/// A device token registered by a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceToken {
    /// Opaque token string (hex for APNs, registration token for FCM).
    pub token: String,
    pub platform: Platform,
    pub user_id: Snowflake,
    /// Optional human-readable device name (e.g. "Mahmut's iPhone").
    pub device_name: Option<String>,
}

/// Push notification payload ready for delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushNotification {
    pub title: String,
    pub body: String,
    /// Arbitrary key-value data forwarded to the client.
    #[serde(default)]
    pub data: HashMap<String, String>,
    /// Badge count (iOS). `None` leaves badge unchanged.
    pub badge: Option<u32>,
    /// Sound name. `"default"` plays the system sound.
    pub sound: Option<String>,
    /// Thread/group identifier for notification grouping.
    pub thread_id: Option<String>,
}

impl PushNotification {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            data: HashMap::new(),
            badge: None,
            sound: Some("default".to_string()),
            thread_id: None,
        }
    }

    pub fn with_data(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }

    pub fn with_badge(mut self, badge: u32) -> Self {
        self.badge = Some(badge);
        self
    }

    pub fn with_sound(mut self, sound: impl Into<String>) -> Self {
        self.sound = Some(sound.into());
        self
    }

    pub fn with_thread_id(mut self, thread_id: impl Into<String>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }
}

// ---------------------------------------------------------------------------
// APNs client (stub)
// ---------------------------------------------------------------------------

/// Configuration for the Apple Push Notification service.
#[derive(Debug, Clone)]
pub struct ApnsConfig {
    /// Path to the `.p8` signing key file.
    pub key_path: String,
    /// 10-character Key ID from App Store Connect.
    pub key_id: String,
    /// 10-character Team ID.
    pub team_id: String,
    /// Bundle identifier (e.g. `com.lumiere.app`).
    pub bundle_id: String,
    /// Use the production APNs endpoint (`true`) or sandbox (`false`).
    pub production: bool,
}

/// Placeholder APNs client. In production this will hold an HTTP/2 connection
/// pool and a signed JWT cache.
pub struct ApnsClient {
    config: ApnsConfig,
}

impl ApnsClient {
    pub fn new(config: ApnsConfig) -> Self {
        info!(
            bundle_id = %config.bundle_id,
            production = config.production,
            "APNs client initialised (stub)"
        );
        Self { config }
    }

    /// Send a push notification to a single APNs device token.
    ///
    /// TODO: Real implementation requires:
    ///   1. Load the `.p8` ES256 private key from `config.key_path`.
    ///   2. Mint a short-lived JWT (iss = team_id, kid = key_id).
    ///   3. Open an HTTP/2 connection to
    ///      `api.push.apple.com` (prod) or `api.sandbox.push.apple.com`.
    ///   4. POST to `/3/device/{device_token}` with JSON payload:
    ///      ```json
    ///      {
    ///        "aps": {
    ///          "alert": { "title": "...", "body": "..." },
    ///          "badge": N,
    ///          "sound": "default",
    ///          "thread-id": "..."
    ///        },
    ///        ...custom data keys
    ///      }
    ///      ```
    ///   5. Handle HTTP 200 (success), 410 (unregistered — remove token),
    ///      and other error codes.
    ///   6. Maintain a connection pool and reuse HTTP/2 streams.
    pub async fn send(
        &self,
        device_token: &str,
        notification: &PushNotification,
    ) -> PushResult<()> {
        info!(
            token = &device_token[..8.min(device_token.len())],
            title = %notification.title,
            bundle_id = %self.config.bundle_id,
            "APNs send (stub) — would deliver push notification"
        );

        // TODO: Replace with real HTTP/2 request to APNs.
        // Validate APNs device token format: must be exactly 64 hex characters.
        if device_token.len() != 64 || !device_token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PushError::InvalidToken(format!(
                "APNs token must be exactly 64 hex characters, got {} chars",
                device_token.len()
            )));
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FCM client (stub)
// ---------------------------------------------------------------------------

/// Configuration for Firebase Cloud Messaging.
#[derive(Debug, Clone)]
pub struct FcmConfig {
    /// Path to the Firebase service account JSON key file.
    pub service_account_key_path: String,
    /// Firebase project ID (e.g. `lumiere-12345`).
    pub project_id: String,
}

/// Placeholder FCM client. In production this will hold an OAuth2 token cache
/// and an HTTP client for the FCM v1 API.
pub struct FcmClient {
    config: FcmConfig,
}

impl FcmClient {
    pub fn new(config: FcmConfig) -> Self {
        info!(
            project_id = %config.project_id,
            "FCM client initialised (stub)"
        );
        Self { config }
    }

    /// Send a push notification to a single FCM registration token.
    ///
    /// TODO: Real implementation requires:
    ///   1. Load the service account JSON from `config.service_account_key_path`.
    ///   2. Obtain an OAuth2 access token (JWT grant, scope
    ///      `https://www.googleapis.com/auth/firebase.messaging`).
    ///   3. POST to `https://fcm.googleapis.com/v1/projects/{project_id}/messages:send`
    ///      with JSON body:
    ///      ```json
    ///      {
    ///        "message": {
    ///          "token": "...",
    ///          "notification": { "title": "...", "body": "..." },
    ///          "android": { "notification": { "sound": "default" } },
    ///          "data": { ... }
    ///        }
    ///      }
    ///      ```
    ///   4. Handle success, `UNREGISTERED` (remove token), quota errors, etc.
    ///   5. Cache the OAuth2 token and refresh before expiry.
    pub async fn send(
        &self,
        registration_token: &str,
        notification: &PushNotification,
    ) -> PushResult<()> {
        info!(
            token = &registration_token[..8.min(registration_token.len())],
            title = %notification.title,
            project_id = %self.config.project_id,
            "FCM send (stub) — would deliver push notification"
        );

        // TODO: Replace with real FCM v1 HTTP request.
        if registration_token.is_empty() {
            return Err(PushError::InvalidToken(
                "FCM registration token is empty".to_string(),
            ));
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Device token store (in-memory for now)
// ---------------------------------------------------------------------------

/// In-memory device token store. Will be backed by PostgreSQL in production.
///
/// WARNING: This is an in-memory store. All tokens are lost on server restart.
/// For production use, this MUST be backed by Redis or a database (PostgreSQL).
///
/// TODO: Replace with database-backed storage:
///   - Table `device_tokens (user_id BIGINT, token TEXT, platform TEXT,
///     device_name TEXT, created_at TIMESTAMPTZ, last_used_at TIMESTAMPTZ)`
///   - Unique constraint on `(user_id, token)`.
///   - Index on `user_id` for fast lookup.
#[derive(Debug, Default)]
pub struct DeviceTokenStore {
    /// user_id -> list of registered device tokens
    tokens: RwLock<HashMap<Snowflake, Vec<DeviceToken>>>,
}

impl DeviceTokenStore {
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Register a device token for a user. Replaces any existing entry with
    /// the same token string (re-registration after app reinstall, etc.).
    pub async fn register(&self, device: DeviceToken) {
        let mut map = self.tokens.write().await;
        let entries = map.entry(device.user_id).or_default();

        // Remove any previous registration with the same raw token.
        entries.retain(|d| d.token != device.token);
        entries.push(device);
    }

    /// Unregister a specific device token (e.g. on logout or 410 from APNs).
    pub async fn unregister(&self, user_id: Snowflake, token: &str) -> bool {
        let mut map = self.tokens.write().await;
        if let Some(entries) = map.get_mut(&user_id) {
            let before = entries.len();
            entries.retain(|d| d.token != token);
            return entries.len() < before;
        }
        false
    }

    /// Unregister all tokens for a user (e.g. account deletion).
    pub async fn unregister_all(&self, user_id: Snowflake) {
        let mut map = self.tokens.write().await;
        map.remove(&user_id);
    }

    /// Get all device tokens for a user.
    pub async fn get_tokens(&self, user_id: Snowflake) -> Vec<DeviceToken> {
        let map = self.tokens.read().await;
        map.get(&user_id).cloned().unwrap_or_default()
    }

    /// Remove a token by its raw string across all users. Useful when APNs
    /// reports a token as invalid (410 response).
    pub async fn remove_invalid_token(&self, token: &str) {
        let mut map = self.tokens.write().await;
        for entries in map.values_mut() {
            entries.retain(|d| d.token != token);
        }
    }
}

// ---------------------------------------------------------------------------
// PushService — the main entry point
// ---------------------------------------------------------------------------

/// Central push notification service that dispatches to APNs or FCM based on
/// the target device platform.
pub struct PushService {
    apns: Option<ApnsClient>,
    fcm: Option<FcmClient>,
    tokens: Arc<DeviceTokenStore>,
}

impl PushService {
    /// Create a new push service. Either or both clients may be `None` if that
    /// platform is not configured (e.g. development environments).
    pub fn new(
        apns_config: Option<ApnsConfig>,
        fcm_config: Option<FcmConfig>,
        tokens: Arc<DeviceTokenStore>,
    ) -> Self {
        Self {
            apns: apns_config.map(ApnsClient::new),
            fcm: fcm_config.map(FcmClient::new),
            tokens,
        }
    }

    /// Reference to the shared device token store.
    pub fn token_store(&self) -> &Arc<DeviceTokenStore> {
        &self.tokens
    }

    /// Send a push notification to a single device.
    pub async fn send_to_device(
        &self,
        device: &DeviceToken,
        notification: &PushNotification,
    ) -> PushResult<()> {
        match device.platform {
            Platform::Ios => {
                let apns = self
                    .apns
                    .as_ref()
                    .ok_or(PushError::PlatformNotConfigured(Platform::Ios))?;
                apns.send(&device.token, notification).await
            }
            Platform::Android => {
                let fcm = self
                    .fcm
                    .as_ref()
                    .ok_or(PushError::PlatformNotConfigured(Platform::Android))?;
                fcm.send(&device.token, notification).await
            }
        }
    }

    /// Send a push notification to ALL registered devices for a user.
    /// Returns the count of successfully delivered notifications and logs
    /// any per-device failures.
    pub async fn send_to_user(
        &self,
        user_id: Snowflake,
        notification: &PushNotification,
    ) -> PushResult<usize> {
        let devices = self.tokens.get_tokens(user_id).await;

        if devices.is_empty() {
            return Err(PushError::TokenNotFound(user_id));
        }

        let mut success_count = 0usize;

        for device in &devices {
            match self.send_to_device(device, notification).await {
                Ok(()) => success_count += 1,
                Err(PushError::InvalidToken(reason)) => {
                    warn!(
                        user_id = %user_id.value(),
                        platform = ?device.platform,
                        reason = %reason,
                        "removing invalid device token"
                    );
                    self.tokens
                        .unregister(user_id, &device.token)
                        .await;
                }
                Err(e) => {
                    warn!(
                        user_id = %user_id.value(),
                        platform = ?device.platform,
                        error = %e,
                        "failed to deliver push notification"
                    );
                }
            }
        }

        Ok(success_count)
    }

    /// Send a push notification to multiple users. Best-effort: failures for
    /// individual users are logged but do not abort the batch.
    pub async fn send_to_users(
        &self,
        user_ids: &[Snowflake],
        notification: &PushNotification,
    ) -> usize {
        let mut total = 0usize;

        for &user_id in user_ids {
            match self.send_to_user(user_id, notification).await {
                Ok(n) => total += n,
                Err(PushError::TokenNotFound(_)) => {
                    // User has no registered devices — skip silently.
                }
                Err(e) => {
                    warn!(
                        user_id = %user_id.value(),
                        error = %e,
                        "failed to send push to user"
                    );
                }
            }
        }

        total
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_apns_config() -> ApnsConfig {
        ApnsConfig {
            key_path: "/tmp/fake.p8".to_string(),
            key_id: "ABC123DEF4".to_string(),
            team_id: "TEAM56789A".to_string(),
            bundle_id: "com.lumiere.app".to_string(),
            production: false,
        }
    }

    fn test_fcm_config() -> FcmConfig {
        FcmConfig {
            service_account_key_path: "/tmp/fake-sa.json".to_string(),
            project_id: "lumiere-test".to_string(),
        }
    }

    #[test]
    fn test_push_notification_builder() {
        let notif = PushNotification::new("Hello", "World")
            .with_badge(3)
            .with_data("channel_id", "12345")
            .with_thread_id("server-1-channel-2");

        assert_eq!(notif.title, "Hello");
        assert_eq!(notif.body, "World");
        assert_eq!(notif.badge, Some(3));
        assert_eq!(notif.sound, Some("default".to_string()));
        assert_eq!(notif.data.get("channel_id").unwrap(), "12345");
        assert_eq!(notif.thread_id, Some("server-1-channel-2".to_string()));
    }

    #[test]
    fn test_notification_serialization() {
        let notif = PushNotification::new("Test", "Body")
            .with_data("key", "value");
        let json = serde_json::to_string(&notif).unwrap();
        let deserialized: PushNotification = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.title, "Test");
        assert_eq!(deserialized.data.get("key").unwrap(), "value");
    }

    #[tokio::test]
    async fn test_device_token_store_register_and_get() {
        let store = DeviceTokenStore::new();
        let user_id = Snowflake::new(1001);

        store
            .register(DeviceToken {
                token: "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344".to_string(),
                platform: Platform::Ios,
                user_id,
                device_name: Some("Test iPhone".to_string()),
            })
            .await;

        let tokens = store.get_tokens(user_id).await;
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].platform, Platform::Ios);
    }

    #[tokio::test]
    async fn test_device_token_store_dedup() {
        let store = DeviceTokenStore::new();
        let user_id = Snowflake::new(1002);
        let token_str = "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344".to_string();

        for _ in 0..3 {
            store
                .register(DeviceToken {
                    token: token_str.clone(),
                    platform: Platform::Ios,
                    user_id,
                    device_name: None,
                })
                .await;
        }

        let tokens = store.get_tokens(user_id).await;
        assert_eq!(tokens.len(), 1, "duplicate tokens should be deduplicated");
    }

    #[tokio::test]
    async fn test_device_token_store_unregister() {
        let store = DeviceTokenStore::new();
        let user_id = Snowflake::new(1003);
        let token_str = "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344".to_string();

        store
            .register(DeviceToken {
                token: token_str.clone(),
                platform: Platform::Android,
                user_id,
                device_name: None,
            })
            .await;

        assert!(store.unregister(user_id, &token_str).await);
        assert!(store.get_tokens(user_id).await.is_empty());
    }

    #[tokio::test]
    async fn test_send_to_device_ios_stub() {
        let tokens = Arc::new(DeviceTokenStore::new());
        let service = PushService::new(Some(test_apns_config()), None, tokens);

        let device = DeviceToken {
            token: "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344".to_string(),
            platform: Platform::Ios,
            user_id: Snowflake::new(2001),
            device_name: None,
        };

        let notif = PushNotification::new("Test", "Hello iOS");
        let result = service.send_to_device(&device, &notif).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_to_device_android_stub() {
        let tokens = Arc::new(DeviceTokenStore::new());
        let service = PushService::new(None, Some(test_fcm_config()), tokens);

        let device = DeviceToken {
            token: "fcm-registration-token-value".to_string(),
            platform: Platform::Android,
            user_id: Snowflake::new(2002),
            device_name: None,
        };

        let notif = PushNotification::new("Test", "Hello Android");
        let result = service.send_to_device(&device, &notif).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_fails_without_platform_config() {
        let tokens = Arc::new(DeviceTokenStore::new());
        // No APNs or FCM configured
        let service = PushService::new(None, None, tokens);

        let device = DeviceToken {
            token: "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344".to_string(),
            platform: Platform::Ios,
            user_id: Snowflake::new(2003),
            device_name: None,
        };

        let notif = PushNotification::new("Test", "Should fail");
        let result = service.send_to_device(&device, &notif).await;
        assert!(matches!(result, Err(PushError::PlatformNotConfigured(_))));
    }

    #[tokio::test]
    async fn test_send_to_user() {
        let tokens = Arc::new(DeviceTokenStore::new());
        let user_id = Snowflake::new(3001);

        tokens
            .register(DeviceToken {
                token: "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344".to_string(),
                platform: Platform::Ios,
                user_id,
                device_name: Some("iPhone".to_string()),
            })
            .await;

        tokens
            .register(DeviceToken {
                token: "fcm-token-for-android-device".to_string(),
                platform: Platform::Android,
                user_id,
                device_name: Some("Pixel".to_string()),
            })
            .await;

        let service =
            PushService::new(Some(test_apns_config()), Some(test_fcm_config()), tokens);

        let notif = PushNotification::new("New message", "Hey there!");
        let delivered = service.send_to_user(user_id, &notif).await.unwrap();
        assert_eq!(delivered, 2);
    }

    #[tokio::test]
    async fn test_send_to_user_no_tokens() {
        let tokens = Arc::new(DeviceTokenStore::new());
        let service =
            PushService::new(Some(test_apns_config()), Some(test_fcm_config()), tokens);

        let notif = PushNotification::new("Test", "No devices");
        let result = service.send_to_user(Snowflake::new(9999), &notif).await;
        assert!(matches!(result, Err(PushError::TokenNotFound(_))));
    }

    #[tokio::test]
    async fn test_remove_invalid_token_across_users() {
        let store = DeviceTokenStore::new();
        let shared_token = "shared-invalid-token-xxxxxxxxxxxxxx".to_string();

        for uid in [5001, 5002, 5003] {
            store
                .register(DeviceToken {
                    token: shared_token.clone(),
                    platform: Platform::Android,
                    user_id: Snowflake::new(uid),
                    device_name: None,
                })
                .await;
        }

        store.remove_invalid_token(&shared_token).await;

        for uid in [5001, 5002, 5003] {
            assert!(store.get_tokens(Snowflake::new(uid)).await.is_empty());
        }
    }
}
