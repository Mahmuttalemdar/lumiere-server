use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use lumiere_auth::middleware::AuthUser;
use lumiere_models::{error::AppError, snowflake::Snowflake};
use lumiere_push::{PgDeviceTokenStore, Platform};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::Validate;

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/@me/devices", post(register_device))
        .route("/@me/devices", get(list_devices))
        .route("/@me/devices/{device_id}", delete(unregister_device))
}

// ─── Request / Response types ──────────────────────────────────

#[derive(Debug, Deserialize, Validate)]
pub struct RegisterDeviceRequest {
    /// The push token from APNs or FCM.
    #[validate(length(min = 1, max = 512))]
    pub token: String,
    /// `"ios"` or `"android"`.
    pub platform: Platform,
}

#[derive(Debug, Serialize)]
pub struct DeviceResponse {
    pub id: Snowflake,
    pub token: String,
    pub platform: Platform,
}

// ─── Handlers ──────────────────────────────────────────────────

/// `POST /api/v1/users/@me/devices` — register a push notification token.
async fn register_device(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<RegisterDeviceRequest>,
) -> Result<impl IntoResponse, AppError> {
    if let Err(errors) = body.validate() {
        return Err(AppError::Validation(super::validation_errors(errors)));
    }

    let pg_store = PgDeviceTokenStore::new(state.db.pg.clone());

    // Enforce a maximum of 10 device tokens per user
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM device_tokens WHERE user_id = $1")
        .bind(auth.id)
        .fetch_one(&state.db.pg)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    if count >= 10 {
        return Err(AppError::BadRequest(
            "Too many registered devices (maximum 10)".into(),
        ));
    }

    let device_id = state.snowflake.next_id();

    pg_store
        .register(device_id, auth.id, body.platform, &body.token)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    Ok((
        StatusCode::CREATED,
        Json(DeviceResponse {
            id: device_id,
            token: mask_token(&body.token),
            platform: body.platform,
        }),
    ))
}

/// `GET /api/v1/users/@me/devices` — list registered push tokens.
async fn list_devices(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let pg_store = PgDeviceTokenStore::new(state.db.pg.clone());

    let tokens = pg_store
        .get_tokens(auth.id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    let response: Vec<DeviceResponse> = tokens
        .into_iter()
        .map(|d| DeviceResponse {
            id: d.id.unwrap_or_else(|| Snowflake::new(0)),
            token: mask_token(&d.token),
            platform: d.platform,
        })
        .collect();

    Ok(Json(response))
}

/// `DELETE /api/v1/users/@me/devices/:device_id` — unregister a push token.
async fn unregister_device(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(device_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let pg_store = PgDeviceTokenStore::new(state.db.pg.clone());

    let removed = pg_store
        .unregister_by_id(auth.id, Snowflake::from(device_id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{}", e)))?;

    if !removed {
        return Err(AppError::NotFound("Device not found".into()));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Mask a device token so only the first 4 and last 4 characters are visible.
fn mask_token(token: &str) -> String {
    if token.len() > 8 {
        format!("{}...{}", &token[..4], &token[token.len() - 4..])
    } else {
        "****".to_string()
    }
}
