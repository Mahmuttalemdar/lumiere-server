use anyhow::Result;
use chrono::Utc;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub user_id: String,
    pub jti: String,
    pub device_info: Option<String>,
    pub ip: Option<String>,
    pub created_at: i64,
    pub last_active: i64,
}

pub struct SessionManager {
    redis: redis::aio::ConnectionManager,
}

impl SessionManager {
    pub fn new(redis: redis::aio::ConnectionManager) -> Self {
        Self { redis }
    }

    /// Store a refresh token session
    pub async fn create_session(
        &self,
        user_id: &str,
        jti: &str,
        ttl_seconds: u64,
        device_info: Option<&str>,
        ip: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.redis.clone();
        let now = Utc::now().timestamp();

        let session = Session {
            user_id: user_id.to_string(),
            jti: jti.to_string(),
            device_info: device_info.map(|s| s.to_string()),
            ip: ip.map(|s| s.to_string()),
            created_at: now,
            last_active: now,
        };

        let session_json = serde_json::to_string(&session)?;

        // Store refresh token mapping
        let refresh_key = format!("refresh:{}", jti);
        conn.set_ex::<_, _, ()>(&refresh_key, &session_json, ttl_seconds)
            .await?;

        // Add to user's session set
        let user_sessions_key = format!("user_sessions:{}", user_id);
        conn.sadd::<_, _, ()>(&user_sessions_key, jti).await?;

        Ok(())
    }

    /// Validate a refresh token exists and return its session
    pub async fn get_session(&self, jti: &str) -> Result<Option<Session>> {
        let mut conn = self.redis.clone();
        let key = format!("refresh:{}", jti);
        let session_json: Option<String> = conn.get(&key).await?;

        match session_json {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Revoke a single refresh token
    pub async fn revoke_session(&self, jti: &str, user_id: &str) -> Result<()> {
        let mut conn = self.redis.clone();

        let refresh_key = format!("refresh:{}", jti);
        conn.del::<_, ()>(&refresh_key).await?;

        let user_sessions_key = format!("user_sessions:{}", user_id);
        conn.srem::<_, _, ()>(&user_sessions_key, jti).await?;

        Ok(())
    }

    /// Revoke all sessions for a user
    pub async fn revoke_all_sessions(&self, user_id: &str) -> Result<()> {
        let mut conn = self.redis.clone();

        let user_sessions_key = format!("user_sessions:{}", user_id);
        let jtis: Vec<String> = conn.smembers(&user_sessions_key).await?;

        for jti in &jtis {
            let refresh_key = format!("refresh:{}", jti);
            conn.del::<_, ()>(&refresh_key).await?;
        }

        conn.del::<_, ()>(&user_sessions_key).await?;

        Ok(())
    }

    /// Update last_active timestamp
    pub async fn touch_session(&self, jti: &str) -> Result<()> {
        let mut conn = self.redis.clone();
        let key = format!("refresh:{}", jti);

        let session_json: Option<String> = conn.get(&key).await?;
        if let Some(json) = session_json {
            let mut session: Session = serde_json::from_str(&json)?;
            session.last_active = Utc::now().timestamp();
            let ttl: i64 = conn.ttl(&key).await?;
            if ttl > 0 {
                let updated = serde_json::to_string(&session)?;
                conn.set_ex::<_, _, ()>(&key, &updated, ttl as u64).await?;
            }
        }

        Ok(())
    }
}
