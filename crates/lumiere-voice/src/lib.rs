use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use livekit_api::access_token::{AccessToken, VideoGrants};
use livekit_api::services::room::{CreateRoomOptions, RoomClient};
use lumiere_models::config::LivekitConfig;
use lumiere_models::snowflake::Snowflake;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum VoiceError {
    #[error("LiveKit token generation failed: {0}")]
    TokenGeneration(String),

    #[error("LiveKit room operation failed: {0}")]
    RoomOperation(String),

    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Voice state not found for user {user_id} in server {server_id}")]
    StateNotFound {
        user_id: Snowflake,
        server_id: Snowflake,
    },
}

// ---------------------------------------------------------------------------
// Voice State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceState {
    pub user_id: Snowflake,
    pub channel_id: Snowflake,
    pub server_id: Snowflake,
    pub session_id: String,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub server_mute: bool,
    pub server_deaf: bool,
    pub suppress: bool,
    pub self_video: bool,
    pub self_stream: bool,
    pub joined_at: i64,
}

impl VoiceState {
    pub fn new(
        user_id: Snowflake,
        channel_id: Snowflake,
        server_id: Snowflake,
        session_id: String,
    ) -> Self {
        Self {
            user_id,
            channel_id,
            server_id,
            session_id,
            self_mute: false,
            self_deaf: false,
            server_mute: false,
            server_deaf: false,
            suppress: false,
            self_video: false,
            self_stream: false,
            joined_at: Utc::now().timestamp(),
        }
    }

    fn redis_key(server_id: Snowflake, user_id: Snowflake) -> String {
        format!("voice_state:{}:{}", server_id, user_id)
    }

    fn channel_members_key(channel_id: Snowflake) -> String {
        format!("voice_channel_members:{}", channel_id)
    }
}

// ---------------------------------------------------------------------------
// Voice Service
// ---------------------------------------------------------------------------

pub struct VoiceService {
    room_client: RoomClient,
    redis: redis::aio::ConnectionManager,
    config: LivekitConfig,
}

impl VoiceService {
    /// Connect to LiveKit and Redis, returning a ready VoiceService.
    pub async fn connect(
        config: LivekitConfig,
        redis: redis::aio::ConnectionManager,
    ) -> Result<Self> {
        let room_client =
            RoomClient::with_api_key(&config.url, &config.api_key, &config.api_secret);

        info!(url = %config.url, "Connected to LiveKit");

        Ok(Self {
            room_client,
            redis,
            config,
        })
    }

    // -----------------------------------------------------------------------
    // Room management
    // -----------------------------------------------------------------------

    /// Create a LiveKit room for the given channel.
    /// Room name follows the pattern `{server_id}_{channel_id}`.
    pub async fn create_room(
        &self,
        server_id: Snowflake,
        channel_id: Snowflake,
        max_participants: u32,
    ) -> Result<(), VoiceError> {
        let room_name = Self::room_name(server_id, channel_id);

        // Clamp max_participants to a sane range.
        let max_participants = max_participants.clamp(1, 1000);

        let options = CreateRoomOptions {
            empty_timeout: 300, // 5 min — auto-cleanup if everyone leaves
            max_participants,
            ..Default::default()
        };

        self.room_client
            .create_room(&room_name, options)
            .await
            .map_err(|e| VoiceError::RoomOperation(e.to_string()))?;

        info!(%room_name, "Created LiveKit room");
        Ok(())
    }

    /// Delete a LiveKit room for the given channel.
    pub async fn delete_room(
        &self,
        server_id: Snowflake,
        channel_id: Snowflake,
    ) -> Result<(), VoiceError> {
        let room_name = Self::room_name(server_id, channel_id);

        self.room_client
            .delete_room(&room_name)
            .await
            .map_err(|e| VoiceError::RoomOperation(e.to_string()))?;

        // Clean up the channel members set in Redis
        let mut conn = self.redis.clone();
        let members_key = VoiceState::channel_members_key(channel_id);
        conn.del::<_, ()>(&members_key).await?;

        info!(%room_name, "Deleted LiveKit room");
        Ok(())
    }

    /// List rooms that currently exist on the LiveKit server.
    pub async fn list_rooms(&self) -> Result<Vec<String>, VoiceError> {
        let rooms = self
            .room_client
            .list_rooms(Vec::new())
            .await
            .map_err(|e| VoiceError::RoomOperation(e.to_string()))?;

        Ok(rooms.into_iter().map(|r| r.name).collect())
    }

    // -----------------------------------------------------------------------
    // Token generation
    // -----------------------------------------------------------------------

    /// Generate a LiveKit access token for a user to join a voice channel.
    pub fn generate_token(
        &self,
        user_id: Snowflake,
        username: &str,
        server_id: Snowflake,
        channel_id: Snowflake,
        ttl: Duration,
    ) -> Result<String, VoiceError> {
        let room_name = Self::room_name(server_id, channel_id);
        let identity = user_id.to_string();

        let grants = VideoGrants {
            room_join: true,
            room: room_name.clone(),
            can_publish: true,
            can_subscribe: true,
            can_publish_data: true,
            ..Default::default()
        };

        let token = AccessToken::with_api_key(&self.config.api_key, &self.config.api_secret)
            .with_identity(&identity)
            .with_name(username)
            .with_grants(grants)
            .with_ttl(ttl)
            .to_jwt()
            .map_err(|e| VoiceError::TokenGeneration(e.to_string()))?;

        Ok(token)
    }

    /// Generate a server-admin token (for bots, recording, etc.).
    pub fn generate_admin_token(&self, ttl: Duration) -> Result<String, VoiceError> {
        let grants = VideoGrants {
            room_create: true,
            room_list: true,
            room_admin: true,
            ..Default::default()
        };

        let token = AccessToken::with_api_key(&self.config.api_key, &self.config.api_secret)
            .with_identity("lumiere-server")
            .with_grants(grants)
            .with_ttl(ttl)
            .to_jwt()
            .map_err(|e| VoiceError::TokenGeneration(e.to_string()))?;

        Ok(token)
    }

    // -----------------------------------------------------------------------
    // Voice state tracking (Redis)
    // -----------------------------------------------------------------------

    /// Set voice state when a user joins a voice channel.
    /// The TTL is always refreshed on each call to keep the state alive.
    pub async fn set_voice_state(&self, state: &VoiceState) -> Result<(), VoiceError> {
        let mut conn = self.redis.clone();

        let key = VoiceState::redis_key(state.server_id, state.user_id);
        let json = serde_json::to_string(state)?;

        // Store the voice state with a 24h TTL as a safety net.
        // TTL is always refreshed to keep the presence alive while the user is connected.
        conn.set_ex::<_, _, ()>(&key, &json, 86400).await?;

        // Track membership in the channel
        let members_key = VoiceState::channel_members_key(state.channel_id);
        conn.sadd::<_, _, ()>(&members_key, state.user_id.to_string())
            .await?;

        info!(
            user_id = %state.user_id,
            channel_id = %state.channel_id,
            server_id = %state.server_id,
            "Voice state set"
        );

        Ok(())
    }

    /// Get voice state for a user in a server.
    pub async fn get_voice_state(
        &self,
        server_id: Snowflake,
        user_id: Snowflake,
    ) -> Result<Option<VoiceState>, VoiceError> {
        let mut conn = self.redis.clone();
        let key = VoiceState::redis_key(server_id, user_id);
        let json: Option<String> = conn.get(&key).await?;

        match json {
            Some(data) => Ok(Some(serde_json::from_str(&data)?)),
            None => Ok(None),
        }
    }

    /// Update mute/deaf/video state for a user.
    ///
    /// TODO: This has a known race condition — concurrent updates can overwrite
    /// each other (read-modify-write without atomicity). For a proper fix, use
    /// Redis WATCH/MULTI/EXEC optimistic locking or a Lua script to apply
    /// modifications atomically. Acceptable for now since voice state updates
    /// for the same user are unlikely to be truly concurrent.
    pub async fn update_voice_state(
        &self,
        server_id: Snowflake,
        user_id: Snowflake,
        update: VoiceStateUpdate,
    ) -> Result<VoiceState, VoiceError> {
        let mut state = self
            .get_voice_state(server_id, user_id)
            .await?
            .ok_or(VoiceError::StateNotFound { user_id, server_id })?;

        if let Some(v) = update.self_mute {
            state.self_mute = v;
        }
        if let Some(v) = update.self_deaf {
            state.self_deaf = v;
            // Being deaf implies muted
            if v {
                state.self_mute = true;
            }
        }
        if let Some(v) = update.self_video {
            state.self_video = v;
        }
        if let Some(v) = update.self_stream {
            state.self_stream = v;
        }
        if let Some(v) = update.server_mute {
            state.server_mute = v;
        }
        if let Some(v) = update.server_deaf {
            state.server_deaf = v;
        }
        if let Some(v) = update.suppress {
            state.suppress = v;
        }

        self.set_voice_state(&state).await?;
        Ok(state)
    }

    /// Remove voice state when a user leaves or disconnects.
    pub async fn remove_voice_state(
        &self,
        server_id: Snowflake,
        user_id: Snowflake,
    ) -> Result<Option<VoiceState>, VoiceError> {
        let mut conn = self.redis.clone();

        // Fetch current state so we know which channel to update
        let state = self.get_voice_state(server_id, user_id).await?;

        let key = VoiceState::redis_key(server_id, user_id);
        conn.del::<_, ()>(&key).await?;

        if let Some(ref s) = state {
            let members_key = VoiceState::channel_members_key(s.channel_id);
            conn.srem::<_, _, ()>(&members_key, user_id.to_string())
                .await?;
        }

        if state.is_some() {
            info!(%server_id, %user_id, "Voice state removed");
        }

        Ok(state)
    }

    /// Get all user IDs currently in a voice channel.
    pub async fn get_channel_members(
        &self,
        channel_id: Snowflake,
    ) -> Result<Vec<Snowflake>, VoiceError> {
        let mut conn = self.redis.clone();
        let members_key = VoiceState::channel_members_key(channel_id);
        let member_ids: Vec<String> = conn.smembers(&members_key).await?;

        let snowflakes = member_ids
            .into_iter()
            .filter_map(|id| id.parse::<u64>().ok().map(Snowflake::new))
            .collect();

        Ok(snowflakes)
    }

    /// Get voice states for all users in a server.
    /// Scans Redis for `voice_state:{server_id}:*` keys using SCAN (non-blocking).
    pub async fn get_server_voice_states(
        &self,
        server_id: Snowflake,
    ) -> Result<Vec<VoiceState>, VoiceError> {
        let mut conn = self.redis.clone();
        let pattern = format!("voice_state:{}:*", server_id);

        // Use SCAN instead of KEYS to avoid blocking Redis on large keyspaces.
        let mut keys: Vec<String> = Vec::new();
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await?;

            keys.extend(batch);
            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }

        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let values: Vec<Option<String>> = conn.mget(&keys).await?;

        let states = values
            .into_iter()
            .flatten()
            .filter_map(|json| match serde_json::from_str::<VoiceState>(&json) {
                Ok(state) => Some(state),
                Err(e) => {
                    warn!(error = %e, "Failed to deserialize voice state");
                    None
                }
            })
            .collect();

        Ok(states)
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Canonical room name for a voice channel.
    fn room_name(server_id: Snowflake, channel_id: Snowflake) -> String {
        format!("{}_{}", server_id, channel_id)
    }
}

// ---------------------------------------------------------------------------
// Partial update DTO
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceStateUpdate {
    pub self_mute: Option<bool>,
    pub self_deaf: Option<bool>,
    pub self_video: Option<bool>,
    pub self_stream: Option<bool>,
    pub server_mute: Option<bool>,
    pub server_deaf: Option<bool>,
    pub suppress: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_name_format() {
        let server = Snowflake::new(123);
        let channel = Snowflake::new(456);
        assert_eq!(VoiceService::room_name(server, channel), "123_456");
    }

    #[test]
    fn test_voice_state_redis_key() {
        let server = Snowflake::new(100);
        let user = Snowflake::new(200);
        assert_eq!(VoiceState::redis_key(server, user), "voice_state:100:200");
    }

    #[test]
    fn test_voice_state_new_defaults() {
        let state = VoiceState::new(
            Snowflake::new(1),
            Snowflake::new(2),
            Snowflake::new(3),
            "session-abc".to_string(),
        );

        assert!(!state.self_mute);
        assert!(!state.self_deaf);
        assert!(!state.server_mute);
        assert!(!state.server_deaf);
        assert!(!state.suppress);
        assert!(!state.self_video);
        assert!(!state.self_stream);
        assert!(state.joined_at > 0);
    }

    #[test]
    fn test_voice_state_serialization() {
        let state = VoiceState::new(
            Snowflake::new(1),
            Snowflake::new(2),
            Snowflake::new(3),
            "session-xyz".to_string(),
        );

        let json = serde_json::to_string(&state).unwrap();
        let parsed: VoiceState = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.user_id, state.user_id);
        assert_eq!(parsed.channel_id, state.channel_id);
        assert_eq!(parsed.server_id, state.server_id);
        assert_eq!(parsed.session_id, "session-xyz");
    }

    #[test]
    fn test_voice_state_update_default() {
        let update = VoiceStateUpdate::default();
        assert!(update.self_mute.is_none());
        assert!(update.self_deaf.is_none());
        assert!(update.self_video.is_none());
        assert!(update.self_stream.is_none());
        assert!(update.server_mute.is_none());
        assert!(update.server_deaf.is_none());
        assert!(update.suppress.is_none());
    }
}
