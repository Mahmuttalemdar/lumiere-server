use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::{PushError, PushNotification, PushResult};

const APNS_PRODUCTION_URL: &str = "https://api.push.apple.com";
const APNS_SANDBOX_URL: &str = "https://api.sandbox.push.apple.com";

/// JWT token valid for 1 hour; refresh at 50 minutes.
const TOKEN_TTL: Duration = Duration::from_secs(3600);
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(600);

struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// Real Apple Push Notification Service client using HTTP/2 + JWT (ES256).
pub struct ApnsClient {
    http: reqwest::Client,
    key_id: String,
    team_id: String,
    bundle_id: String,
    signing_key: EncodingKey,
    base_url: String,
    cached_token: RwLock<Option<CachedToken>>,
}

#[derive(Serialize)]
struct ApnsPayload {
    aps: ApnsAps,
    #[serde(flatten)]
    data: std::collections::HashMap<String, String>,
}

#[derive(Serialize)]
struct ApnsAps {
    alert: ApnsAlert,
    #[serde(skip_serializing_if = "Option::is_none")]
    badge: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sound: Option<String>,
    #[serde(rename = "thread-id", skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
}

#[derive(Serialize)]
struct ApnsAlert {
    title: String,
    body: String,
}

#[derive(Serialize)]
struct ApnsJwtClaims {
    iss: String,
    iat: i64,
}

#[derive(Deserialize)]
struct ApnsErrorBody {
    reason: String,
}

impl ApnsClient {
    /// Create a new APNs client by loading the `.p8` private key.
    ///
    /// Returns `Err` if the key file cannot be read or parsed.
    pub fn new(
        key_path: &str,
        key_id: &str,
        team_id: &str,
        bundle_id: &str,
        sandbox: bool,
    ) -> Result<Self, PushError> {
        let key_data = std::fs::read(key_path).map_err(|e| {
            PushError::ApnsError(format!("Failed to read APNs key at {}: {}", key_path, e))
        })?;

        let signing_key = EncodingKey::from_ec_pem(&key_data).map_err(|e| {
            PushError::ApnsError(format!("Failed to parse APNs ES256 key: {}", e))
        })?;

        let base_url = if sandbox {
            APNS_SANDBOX_URL
        } else {
            APNS_PRODUCTION_URL
        };

        let http = reqwest::Client::builder()
            .http2_prior_knowledge()
            .pool_max_idle_per_host(20)
            .build()
            .map_err(|e| PushError::ApnsError(format!("Failed to build HTTP client: {}", e)))?;

        info!(
            key_id = key_id,
            team_id = team_id,
            bundle_id = bundle_id,
            sandbox = sandbox,
            "APNs client initialized with real ES256 key"
        );

        Ok(Self {
            http,
            key_id: key_id.to_string(),
            team_id: team_id.to_string(),
            bundle_id: bundle_id.to_string(),
            signing_key,
            base_url: base_url.to_string(),
            cached_token: RwLock::new(None),
        })
    }

    /// Get or refresh the JWT bearer token.
    async fn get_or_refresh_token(&self) -> PushResult<String> {
        // Fast path: check if cached token is still valid.
        {
            let guard = self.cached_token.read().await;
            if let Some(ref cached) = *guard {
                if cached.expires_at > Instant::now() {
                    return Ok(cached.token.clone());
                }
            }
        }

        // Slow path: mint a new JWT.
        let mut guard = self.cached_token.write().await;

        // Double-check after acquiring write lock.
        if let Some(ref cached) = *guard {
            if cached.expires_at > Instant::now() {
                return Ok(cached.token.clone());
            }
        }

        let now = chrono::Utc::now().timestamp();
        let claims = ApnsJwtClaims {
            iss: self.team_id.clone(),
            iat: now,
        };

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());

        let token = jsonwebtoken::encode(&header, &claims, &self.signing_key)
            .map_err(|e| PushError::ApnsError(format!("JWT encoding failed: {}", e)))?;

        let expires_at = Instant::now() + TOKEN_TTL - TOKEN_REFRESH_MARGIN;

        *guard = Some(CachedToken {
            token: token.clone(),
            expires_at,
        });

        debug!("APNs JWT token refreshed");
        Ok(token)
    }

    /// Send a push notification to a single APNs device token.
    pub async fn send(
        &self,
        device_token: &str,
        notification: &PushNotification,
    ) -> PushResult<()> {
        // Validate APNs device token format: exactly 64 hex characters.
        if device_token.len() != 64 || !device_token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PushError::InvalidToken(format!(
                "APNs token must be exactly 64 hex characters, got {} chars",
                device_token.len()
            )));
        }

        let jwt = self.get_or_refresh_token().await?;

        let payload = ApnsPayload {
            aps: ApnsAps {
                alert: ApnsAlert {
                    title: notification.title.clone(),
                    body: notification.body.clone(),
                },
                badge: notification.badge,
                sound: notification.sound.clone(),
                thread_id: notification.thread_id.clone(),
            },
            data: notification.data.clone(),
        };

        let url = format!("{}/3/device/{}", self.base_url, device_token);

        let response = self
            .http
            .post(&url)
            .header("authorization", format!("bearer {}", jwt))
            .header("apns-topic", &self.bundle_id)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .json(&payload)
            .send()
            .await
            .map_err(|e| PushError::ApnsError(format!("HTTP request failed: {}", e)))?;

        let status = response.status();

        match status.as_u16() {
            200 => {
                debug!(
                    token = &device_token[..8],
                    "APNs notification delivered"
                );
                Ok(())
            }
            410 => {
                // Device token is no longer valid (unregistered).
                warn!(
                    token = &device_token[..8],
                    "APNs token unregistered (410 Gone)"
                );
                Err(PushError::InvalidToken(format!(
                    "APNs token {} is no longer valid (410 Gone)",
                    &device_token[..8]
                )))
            }
            400 => {
                let body = response.text().await.unwrap_or_default();
                let reason = serde_json::from_str::<ApnsErrorBody>(&body)
                    .map(|b| b.reason)
                    .unwrap_or_else(|_| body);
                warn!(reason = %reason, "APNs bad request (400)");
                Err(PushError::ApnsError(format!("Bad request: {}", reason)))
            }
            429 => {
                warn!("APNs rate limited (429)");
                Err(PushError::ApnsError("Rate limited by APNs".to_string()))
            }
            other => {
                let body = response.text().await.unwrap_or_default();
                Err(PushError::ApnsError(format!(
                    "Unexpected APNs status {}: {}",
                    other, body
                )))
            }
        }
    }
}
