use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::{PushError, PushNotification, PushResult};

const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

/// OAuth2 token valid for 1 hour; refresh at 50 minutes.
const TOKEN_TTL: Duration = Duration::from_secs(3600);
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(600);

struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// Firebase service account JSON structure (subset of fields we need).
#[derive(Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    #[allow(dead_code)]
    project_id: Option<String>,
}

/// Real Firebase Cloud Messaging v1 client using OAuth2 service account auth.
pub struct FcmClient {
    http: reqwest::Client,
    project_id: String,
    service_account_email: String,
    signing_key: EncodingKey,
    cached_token: RwLock<Option<CachedToken>>,
}

#[derive(Serialize)]
struct FcmRequest {
    message: FcmMessage,
}

#[derive(Serialize)]
struct FcmMessage {
    token: String,
    notification: FcmNotification,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    data: std::collections::HashMap<String, String>,
    android: FcmAndroidConfig,
}

#[derive(Serialize)]
struct FcmNotification {
    title: String,
    body: String,
}

#[derive(Serialize)]
struct FcmAndroidConfig {
    priority: String,
}

#[derive(Serialize)]
struct GoogleJwtClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: i64,
    exp: i64,
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    #[allow(dead_code)]
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct FcmErrorResponse {
    error: Option<FcmErrorDetail>,
}

#[derive(Deserialize)]
struct FcmErrorDetail {
    message: Option<String>,
    #[allow(dead_code)]
    status: Option<String>,
    #[allow(dead_code)]
    code: Option<u32>,
}

impl FcmClient {
    /// Create a new FCM client by loading the service account JSON key file.
    ///
    /// Returns `Err` if the file cannot be read or parsed.
    pub fn new(service_account_key_path: &str, project_id: &str) -> Result<Self, PushError> {
        let key_data = std::fs::read_to_string(service_account_key_path).map_err(|e| {
            PushError::FcmError(format!(
                "Failed to read service account key at {}: {}",
                service_account_key_path, e
            ))
        })?;

        let sa: ServiceAccountKey = serde_json::from_str(&key_data).map_err(|e| {
            PushError::FcmError(format!("Failed to parse service account JSON: {}", e))
        })?;

        let signing_key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
            .map_err(|e| PushError::FcmError(format!("Failed to parse RSA private key: {}", e)))?;

        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(20)
            .build()
            .map_err(|e| PushError::FcmError(format!("Failed to build HTTP client: {}", e)))?;

        info!(
            project_id = project_id,
            client_email = %sa.client_email,
            "FCM client initialized with service account"
        );

        Ok(Self {
            http,
            project_id: project_id.to_string(),
            service_account_email: sa.client_email,
            signing_key,
            cached_token: RwLock::new(None),
        })
    }

    /// Get or refresh the OAuth2 access token.
    async fn get_or_refresh_token(&self) -> PushResult<String> {
        // Fast path: check cached token.
        {
            let guard = self.cached_token.read().await;
            if let Some(ref cached) = *guard {
                if cached.expires_at > Instant::now() {
                    return Ok(cached.token.clone());
                }
            }
        }

        // Slow path: exchange a self-signed JWT for an OAuth2 access token.
        let mut guard = self.cached_token.write().await;

        // Double-check after write lock.
        if let Some(ref cached) = *guard {
            if cached.expires_at > Instant::now() {
                return Ok(cached.token.clone());
            }
        }

        let now = chrono::Utc::now().timestamp();
        let claims = GoogleJwtClaims {
            iss: self.service_account_email.clone(),
            scope: FCM_SCOPE.to_string(),
            aud: GOOGLE_TOKEN_URL.to_string(),
            iat: now,
            exp: now + TOKEN_TTL.as_secs() as i64,
        };

        let header = Header::new(Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &self.signing_key)
            .map_err(|e| PushError::FcmError(format!("JWT encoding failed: {}", e)))?;

        // Exchange JWT for access token.
        let response = self
            .http
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| PushError::FcmError(format!("Token exchange request failed: {}", e)))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PushError::FcmError(format!(
                "Token exchange failed: {}",
                body
            )));
        }

        let token_response: GoogleTokenResponse = response
            .json()
            .await
            .map_err(|e| PushError::FcmError(format!("Failed to parse token response: {}", e)))?;

        let expires_at = Instant::now() + TOKEN_TTL - TOKEN_REFRESH_MARGIN;

        let access_token = token_response.access_token.clone();
        *guard = Some(CachedToken {
            token: token_response.access_token,
            expires_at,
        });

        debug!("FCM OAuth2 token refreshed");
        Ok(access_token)
    }

    /// Send a push notification to a single FCM registration token.
    pub async fn send(
        &self,
        registration_token: &str,
        notification: &PushNotification,
    ) -> PushResult<()> {
        if registration_token.is_empty() {
            return Err(PushError::InvalidToken(
                "FCM registration token is empty".to_string(),
            ));
        }

        let access_token = self.get_or_refresh_token().await?;

        let request_body = FcmRequest {
            message: FcmMessage {
                token: registration_token.to_string(),
                notification: FcmNotification {
                    title: notification.title.clone(),
                    body: notification.body.clone(),
                },
                data: notification.data.clone(),
                android: FcmAndroidConfig {
                    priority: "high".to_string(),
                },
            },
        };

        let url = format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            self.project_id
        );

        let response = self
            .http
            .post(&url)
            .bearer_auth(&access_token)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| PushError::FcmError(format!("HTTP request failed: {}", e)))?;

        let status = response.status();

        match status.as_u16() {
            200 => {
                debug!(
                    token = &registration_token[..8.min(registration_token.len())],
                    "FCM notification delivered"
                );
                Ok(())
            }
            401 => {
                // Token expired. Invalidate cache and retry once.
                warn!("FCM OAuth2 token expired (401), invalidating cache");
                {
                    let mut guard = self.cached_token.write().await;
                    *guard = None;
                }

                let fresh_token = self.get_or_refresh_token().await?;
                let retry_response = self
                    .http
                    .post(&url)
                    .bearer_auth(&fresh_token)
                    .json(&request_body)
                    .send()
                    .await
                    .map_err(|e| {
                        PushError::FcmError(format!("Retry HTTP request failed: {}", e))
                    })?;

                if retry_response.status().is_success() {
                    debug!("FCM notification delivered after token refresh");
                    Ok(())
                } else {
                    let body = retry_response.text().await.unwrap_or_default();
                    Err(PushError::FcmError(format!(
                        "FCM retry failed after token refresh: {}",
                        body
                    )))
                }
            }
            404 => {
                // Token unregistered.
                let body = response.text().await.unwrap_or_default();
                let is_unregistered = body.contains("UNREGISTERED") || body.contains("not found");
                if is_unregistered {
                    warn!(
                        token = &registration_token[..8.min(registration_token.len())],
                        "FCM token unregistered (404)"
                    );
                    Err(PushError::InvalidToken(format!(
                        "FCM token {} is unregistered",
                        &registration_token[..8.min(registration_token.len())]
                    )))
                } else {
                    Err(PushError::FcmError(format!("FCM 404 error: {}", body)))
                }
            }
            429 => {
                warn!("FCM rate limited (429)");
                Err(PushError::FcmError("Rate limited by FCM".to_string()))
            }
            _ => {
                let body = response.text().await.unwrap_or_default();
                // Check for UNREGISTERED in any error response.
                if body.contains("UNREGISTERED") {
                    warn!(
                        token = &registration_token[..8.min(registration_token.len())],
                        "FCM token unregistered"
                    );
                    return Err(PushError::InvalidToken(format!(
                        "FCM token {} is unregistered",
                        &registration_token[..8.min(registration_token.len())]
                    )));
                }

                let error_detail = serde_json::from_str::<FcmErrorResponse>(&body)
                    .ok()
                    .and_then(|e| e.error)
                    .and_then(|e| e.message);

                Err(PushError::FcmError(format!(
                    "FCM status {}: {}",
                    status.as_u16(),
                    error_detail.unwrap_or(body)
                )))
            }
        }
    }
}
