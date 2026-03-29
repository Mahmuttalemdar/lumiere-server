pub mod apns;
pub mod fcm;
pub mod pg_store;

use std::collections::HashMap;
use std::sync::Arc;

use lumiere_models::snowflake::Snowflake;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{info, warn};

// Re-exports for convenience.
pub use apns::ApnsClient;
pub use fcm::FcmClient;
pub use pg_store::PgDeviceTokenStore;

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
    /// Database ID (Snowflake). Present when loaded from store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Snowflake>,
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
// APNs / FCM configuration types (kept for backward compatibility)
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
    /// Use the sandbox APNs endpoint (`true`) or production (`false`).
    pub sandbox: bool,
}

/// Configuration for Firebase Cloud Messaging.
#[derive(Debug, Clone)]
pub struct FcmConfig {
    /// Path to the Firebase service account JSON key file.
    pub service_account_key_path: String,
    /// Firebase project ID (e.g. `lumiere-12345`).
    pub project_id: String,
}

// ---------------------------------------------------------------------------
// Token store — enum dispatch (dyn-compatible without async trait hacks)
// ---------------------------------------------------------------------------

/// In-memory device token store for tests and development.
#[derive(Debug, Default)]
pub struct DeviceTokenStore {
    tokens: RwLock<HashMap<Snowflake, Vec<DeviceToken>>>,
}

impl DeviceTokenStore {
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Register a device token for a user. Replaces any existing entry with
    /// the same token string.
    pub async fn register(&self, device: DeviceToken) {
        let mut map = self.tokens.write().await;
        let entries = map.entry(device.user_id).or_default();
        entries.retain(|d| d.token != device.token);
        entries.push(device);
    }

    /// Unregister all tokens for a user.
    pub async fn unregister_all(&self, user_id: Snowflake) {
        let mut map = self.tokens.write().await;
        map.remove(&user_id);
    }

    /// Get all device tokens for a user.
    pub async fn get_tokens(&self, user_id: Snowflake) -> Vec<DeviceToken> {
        let map = self.tokens.read().await;
        map.get(&user_id).cloned().unwrap_or_default()
    }

    /// Unregister a specific device token.
    pub async fn unregister(&self, user_id: Snowflake, token: &str) -> bool {
        let mut map = self.tokens.write().await;
        if let Some(entries) = map.get_mut(&user_id) {
            let before = entries.len();
            entries.retain(|d| d.token != token);
            return entries.len() < before;
        }
        false
    }

    /// Remove a token by its raw string across all users.
    pub async fn remove_invalid_token(&self, token: &str) {
        let mut map = self.tokens.write().await;
        for entries in map.values_mut() {
            entries.retain(|d| d.token != token);
        }
    }
}

/// Enum-based token store dispatch. Avoids `dyn` + async trait issues.
pub enum TokenStoreBackend {
    InMemory(DeviceTokenStore),
    Postgres(PgDeviceTokenStore),
}

impl TokenStoreBackend {
    pub async fn get_tokens(&self, user_id: Snowflake) -> Vec<DeviceToken> {
        match self {
            Self::InMemory(s) => s.get_tokens(user_id).await,
            Self::Postgres(s) => match s.get_tokens(user_id).await {
                Ok(tokens) => tokens,
                Err(e) => {
                    tracing::error!(user_id = %user_id.value(), error = %e, "Failed to get device tokens from DB");
                    Vec::new()
                }
            },
        }
    }

    pub async fn remove_invalid_token(&self, token: &str) {
        match self {
            Self::InMemory(s) => s.remove_invalid_token(token).await,
            Self::Postgres(s) => {
                if let Err(e) = s.remove_invalid_token(token).await {
                    warn!(error = %e, "Failed to remove invalid token from database");
                }
            }
        }
    }

    pub async fn unregister(&self, user_id: Snowflake, token: &str) -> bool {
        match self {
            Self::InMemory(s) => s.unregister(user_id, token).await,
            Self::Postgres(s) => s.unregister(user_id, token).await.unwrap_or(false),
        }
    }
}

// ---------------------------------------------------------------------------
// PushService — the main entry point
// ---------------------------------------------------------------------------

/// Central push notification service that dispatches to APNs or FCM based on
/// the target device platform.
///
/// Construction uses `Option<ApnsClient>` and `Option<FcmClient>` so that
/// each platform degrades gracefully to a log-only stub when credentials
/// are not configured (development environments).
pub struct PushService {
    apns: Option<ApnsClient>,
    fcm: Option<FcmClient>,
    tokens: Arc<TokenStoreBackend>,
}

impl PushService {
    /// Create a push service with optional real clients.
    ///
    /// Pass `None` for APNs/FCM configs to use log-only stubs for that platform.
    pub fn new(
        apns: Option<ApnsClient>,
        fcm: Option<FcmClient>,
        tokens: Arc<TokenStoreBackend>,
    ) -> Self {
        if apns.is_none() {
            info!("APNs client not configured — iOS push notifications will be skipped");
        }
        if fcm.is_none() {
            info!("FCM client not configured — Android push notifications will be skipped");
        }
        Self { apns, fcm, tokens }
    }

    /// Reference to the shared device token store.
    pub fn token_store(&self) -> &Arc<TokenStoreBackend> {
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
                    self.tokens.remove_invalid_token(&device.token).await;
                }
                Err(PushError::PlatformNotConfigured(platform)) => {
                    // Platform not configured — skip silently in development.
                    warn!(
                        user_id = %user_id.value(),
                        platform = ?platform,
                        "push platform not configured, skipping"
                    );
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

    fn make_device(user_id: u64, token: &str, platform: Platform) -> DeviceToken {
        DeviceToken {
            id: None,
            token: token.to_string(),
            platform,
            user_id: Snowflake::new(user_id),
            device_name: None,
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
            .register(make_device(
                1001,
                "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344",
                Platform::Ios,
            ))
            .await;

        let tokens = store.get_tokens(user_id).await;
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].platform, Platform::Ios);
    }

    #[tokio::test]
    async fn test_device_token_store_dedup() {
        let store = DeviceTokenStore::new();
        let token_str = "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344";

        for _ in 0..3 {
            store.register(make_device(1002, token_str, Platform::Ios)).await;
        }

        let tokens = store.get_tokens(Snowflake::new(1002)).await;
        assert_eq!(tokens.len(), 1, "duplicate tokens should be deduplicated");
    }

    #[tokio::test]
    async fn test_device_token_store_unregister() {
        let store = DeviceTokenStore::new();
        let token_str = "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344";

        store.register(make_device(1003, token_str, Platform::Android)).await;

        assert!(store.unregister(Snowflake::new(1003), token_str).await);
        assert!(store.get_tokens(Snowflake::new(1003)).await.is_empty());
    }

    fn in_memory_backend() -> Arc<TokenStoreBackend> {
        Arc::new(TokenStoreBackend::InMemory(DeviceTokenStore::new()))
    }

    #[tokio::test]
    async fn test_send_fails_without_platform_config() {
        let service = PushService::new(None, None, in_memory_backend());

        let device = make_device(
            2003,
            "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344",
            Platform::Ios,
        );

        let notif = PushNotification::new("Test", "Should fail");
        let result = service.send_to_device(&device, &notif).await;
        assert!(matches!(result, Err(PushError::PlatformNotConfigured(_))));
    }

    #[tokio::test]
    async fn test_send_to_user_no_tokens() {
        let service = PushService::new(None, None, in_memory_backend());

        let notif = PushNotification::new("Test", "No devices");
        let result = service.send_to_user(Snowflake::new(9999), &notif).await;
        assert!(matches!(result, Err(PushError::TokenNotFound(_))));
    }

    #[tokio::test]
    async fn test_remove_invalid_token_across_users() {
        let store = DeviceTokenStore::new();
        let shared_token = "shared-invalid-token-xxxxxxxxxxxxxx";

        for uid in [5001, 5002, 5003] {
            store.register(make_device(uid, shared_token, Platform::Android)).await;
        }

        store.remove_invalid_token(shared_token).await;

        for uid in [5001, 5002, 5003] {
            assert!(store.get_tokens(Snowflake::new(uid)).await.is_empty());
        }
    }
}
