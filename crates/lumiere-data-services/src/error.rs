#[derive(Debug, thiserror::Error)]
pub enum DataServiceError {
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Fetch failed: {0}")]
    Fetch(#[from] anyhow::Error),

    #[error("Coalesced request failed: sender dropped")]
    CoalescingFailed,
}
