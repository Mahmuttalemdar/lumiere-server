use lumiere_models::snowflake::Snowflake;
use sqlx::PgPool;
use tracing::{debug, warn};

use crate::{DeviceToken, Platform, PushError, PushResult};

/// PostgreSQL-backed device token store. Persists tokens across restarts.
pub struct PgDeviceTokenStore {
    pool: PgPool,
}

impl PgDeviceTokenStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Register a device token for a user. If the same token string already
    /// exists for this user, the row is updated (upsert).
    pub async fn register(
        &self,
        id: Snowflake,
        user_id: Snowflake,
        platform: Platform,
        token: &str,
    ) -> PushResult<()> {
        let platform_i16 = platform_to_i16(platform);

        sqlx::query(
            r#"
            INSERT INTO device_tokens (id, user_id, platform, token, created_at, updated_at)
            VALUES ($1, $2, $3, $4, now(), now())
            ON CONFLICT (user_id, token)
            DO UPDATE SET platform = EXCLUDED.platform, updated_at = now()
            "#,
        )
        .bind(id.value() as i64)
        .bind(user_id.value() as i64)
        .bind(platform_i16)
        .bind(token)
        .execute(&self.pool)
        .await
        .map_err(|e| PushError::Internal(anyhow::anyhow!("Failed to register device token: {}", e)))?;

        debug!(user_id = %user_id.value(), platform = ?platform, "Device token registered");
        Ok(())
    }

    /// Unregister a specific device token by its database id.
    pub async fn unregister_by_id(&self, user_id: Snowflake, device_id: Snowflake) -> PushResult<bool> {
        let result = sqlx::query(
            "DELETE FROM device_tokens WHERE id = $1 AND user_id = $2",
        )
        .bind(device_id.value() as i64)
        .bind(user_id.value() as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| PushError::Internal(anyhow::anyhow!("Failed to unregister device: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }

    /// Unregister a specific device token by token string.
    pub async fn unregister(&self, user_id: Snowflake, token: &str) -> PushResult<bool> {
        let result = sqlx::query(
            "DELETE FROM device_tokens WHERE user_id = $1 AND token = $2",
        )
        .bind(user_id.value() as i64)
        .bind(token)
        .execute(&self.pool)
        .await
        .map_err(|e| PushError::Internal(anyhow::anyhow!("Failed to unregister device: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }

    /// Get all device tokens for a user.
    pub async fn get_tokens(&self, user_id: Snowflake) -> PushResult<Vec<DeviceToken>> {
        let rows = sqlx::query_as::<_, DeviceTokenRow>(
            "SELECT id, user_id, platform, token FROM device_tokens WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id.value() as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PushError::Internal(anyhow::anyhow!("Failed to get device tokens: {}", e)))?;

        rows.into_iter()
            .map(DeviceToken::try_from)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Remove a token by its raw string across all users. Used when APNs/FCM
    /// reports a token as invalid.
    pub async fn remove_invalid_token(&self, token: &str) -> PushResult<()> {
        let result = sqlx::query("DELETE FROM device_tokens WHERE token = $1")
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                PushError::Internal(anyhow::anyhow!("Failed to remove invalid token: {}", e))
            })?;

        if result.rows_affected() > 0 {
            warn!(
                rows = result.rows_affected(),
                "Removed invalid device token from store"
            );
        }

        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct DeviceTokenRow {
    id: i64,
    user_id: i64,
    platform: i16,
    token: String,
}

impl TryFrom<DeviceTokenRow> for DeviceToken {
    type Error = PushError;

    fn try_from(row: DeviceTokenRow) -> Result<Self, PushError> {
        Ok(DeviceToken {
            id: Some(Snowflake::new(row.id as u64)),
            token: row.token,
            platform: platform_from_i16(row.platform)?,
            user_id: Snowflake::new(row.user_id as u64),
            device_name: None,
        })
    }
}

fn platform_to_i16(platform: Platform) -> i16 {
    match platform {
        Platform::Ios => 0,
        Platform::Android => 1,
    }
}

fn platform_from_i16(value: i16) -> Result<Platform, PushError> {
    match value {
        0 => Ok(Platform::Ios),
        1 => Ok(Platform::Android),
        other => Err(PushError::Internal(anyhow::anyhow!(
            "Unknown platform value: {}",
            other
        ))),
    }
}
