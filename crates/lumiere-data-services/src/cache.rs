use serde::{de::DeserializeOwned, Serialize};
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use crate::coalescing::CoalescingCache;
use crate::error::DataServiceError;
use crate::redis_cache::RedisCache;

/// Configuration for a data service cache layer.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// How long cached entries live in Redis.
    pub ttl: Duration,
    /// Maximum number of in-flight coalescing entries before rejecting new ones.
    /// Set to 0 to disable the limit.
    pub max_in_flight: usize,
}

impl CacheConfig {
    pub fn new(ttl: Duration, max_in_flight: usize) -> Self {
        Self { ttl, max_in_flight }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(300), // 5 minutes
            max_in_flight: 10_000,
        }
    }
}

/// The main data service layer combining Redis cache + request coalescing.
///
/// Pattern: check Redis -> if miss, coalesce concurrent requests -> fetch from
/// DB -> cache result in Redis -> return to all waiters.
///
/// `K` is the cache/coalescing key type (must be convertible to a string for Redis).
/// `V` is the cached value type.
pub struct DataService<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    redis: RedisCache,
    coalescing: CoalescingCache<K, V>,
    config: CacheConfig,
}

impl<K, V> DataService<K, V>
where
    K: Eq + Hash + Clone + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    pub fn new(redis: RedisCache, config: CacheConfig) -> Self {
        Self {
            redis,
            coalescing: CoalescingCache::new(),
            config,
        }
    }

    /// The core get-or-fetch pattern:
    ///
    /// 1. Check Redis cache
    /// 2. If miss, coalesce with other concurrent requests for the same key
    /// 3. Execute one DB fetch
    /// 4. Store result in Redis
    /// 5. Return to all waiters
    pub async fn get_or_fetch<F, Fut>(&self, key: K, fetch: F) -> Result<Arc<V>, DataServiceError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<V, anyhow::Error>> + Send,
    {
        let redis_key = key.to_string();

        // Step 1: Check Redis.
        match self.redis.get::<V>(&redis_key).await {
            Ok(Some(cached)) => {
                tracing::trace!(key = %redis_key, "cache hit");
                return Ok(Arc::new(cached));
            }
            Ok(None) => {
                tracing::trace!(key = %redis_key, "cache miss");
            }
            Err(e) => {
                // Redis failure is not fatal — log and proceed to DB.
                tracing::warn!(key = %redis_key, error = %e, "redis get failed, falling through to DB");
            }
        }

        // Step 2: Guard against unbounded in-flight growth.
        if self.config.max_in_flight > 0
            && self.coalescing.in_flight_count() >= self.config.max_in_flight
        {
            tracing::warn!(
                count = self.coalescing.in_flight_count(),
                limit = self.config.max_in_flight,
                "in-flight coalescing limit reached"
            );
            return Err(DataServiceError::Fetch(anyhow::anyhow!(
                "too many in-flight requests"
            )));
        }

        // Step 3: Coalesce — only one fetch per key.
        let redis = self.redis.clone();
        let ttl = self.config.ttl;
        let rk = redis_key.clone();

        let result = self
            .coalescing
            .get_or_fetch(key, || async move {
                let value = fetch().await?;

                // Step 4: Cache in Redis (best-effort).
                if let Err(e) = redis.set(&rk, &value, ttl).await {
                    tracing::warn!(key = %rk, error = %e, "failed to cache in redis");
                }

                Ok(value)
            })
            .await?;

        Ok(result)
    }

    /// Invalidate a cached entry in Redis.
    pub async fn invalidate(&self, key: &str) -> Result<(), DataServiceError> {
        self.redis.delete(key).await
    }

    /// Invalidate all entries matching a pattern.
    pub async fn invalidate_pattern(&self, pattern: &str) -> Result<u64, DataServiceError> {
        self.redis.delete_pattern(pattern).await
    }

    /// Number of currently in-flight coalesced requests.
    pub fn in_flight_count(&self) -> usize {
        self.coalescing.in_flight_count()
    }
}
