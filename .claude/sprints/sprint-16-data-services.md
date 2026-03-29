# Sprint 16 — Data Services Layer

**Status:** Not Started
**Dependencies:** Sprint 09
**Crates:** lumiere-data-services

## Goal

Build the intermediary data services layer between API handlers and databases. Implements request coalescing (hot partition protection), consistent hash-based routing, caching strategy, and connection optimization. This is Discord's key architecture pattern.

## Tasks

### 16.1 — Request Coalescing

The core innovation: when multiple users request the same data simultaneously, execute only ONE database query and fan out the result.

```rust
// crates/lumiere-data-services/src/coalescer.rs
use tokio::sync::broadcast;
use dashmap::DashMap;

pub struct RequestCoalescer<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    /// In-flight requests: key → broadcast sender
    in_flight: DashMap<K, broadcast::Sender<V>>,
}

impl<K, V> RequestCoalescer<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Execute a query with coalescing
    /// If another request for the same key is in-flight, subscribe to its result
    /// Otherwise, execute the query and broadcast the result
    pub async fn execute<F, Fut>(
        &self,
        key: K,
        query: F,
    ) -> Result<V>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V>>,
    {
        // Check if there's already an in-flight request for this key
        if let Some(sender) = self.in_flight.get(&key) {
            // Subscribe to the existing request's result
            let mut receiver = sender.subscribe();
            drop(sender); // Release DashMap lock
            return Ok(receiver.recv().await?);
        }

        // No in-flight request — we become the leader
        let (tx, _) = broadcast::channel(1);
        self.in_flight.insert(key.clone(), tx.clone());

        // Execute the actual query
        let result = query().await;

        // Remove from in-flight map
        self.in_flight.remove(&key);

        // Broadcast result to all waiting subscribers
        if let Ok(ref value) = result {
            let _ = tx.send(value.clone());
        }

        result
    }
}
```

### 16.2 — Channel Data Service

Wraps all channel-related database operations with coalescing:

```rust
pub struct ChannelDataService {
    db: Database,
    redis: RedisClient,
    coalescer: RequestCoalescer<Snowflake, Vec<Message>>,
    channel_cache: RequestCoalescer<Snowflake, Channel>,
}

impl ChannelDataService {
    /// Get messages with coalescing
    /// 10,000 users opening the same channel = 1 ScyllaDB query
    pub async fn get_messages(
        &self,
        channel_id: Snowflake,
        before: Option<Snowflake>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        // Cache key: channel_id + before cursor + limit
        let cache_key = (channel_id, before, limit);

        // First: check Redis hot cache
        if let Some(cached) = self.redis_cache_get(&cache_key).await? {
            return Ok(cached);
        }

        // Coalesce identical requests
        self.coalescer.execute(channel_id, || async {
            let messages = self.db.get_messages(channel_id, before, limit).await?;

            // Cache hot channels in Redis (TTL 30 seconds)
            self.redis_cache_set(&cache_key, &messages, 30).await?;

            Ok(messages)
        }).await
    }

    /// Get channel with coalescing
    pub async fn get_channel(&self, channel_id: Snowflake) -> Result<Channel> {
        self.channel_cache.execute(channel_id, || async {
            self.db.get_channel(channel_id).await
        }).await
    }
}
```

### 16.3 — Redis Hot Cache Strategy

Cache recently accessed data in Redis to avoid database hits:

```
Key patterns:
    cache:channel:{id}              → Channel JSON (TTL 60s)
    cache:messages:{channel_id}:latest → Last 50 messages JSON (TTL 30s)
    cache:member:{server_id}:{user_id} → Member JSON (TTL 60s)
    cache:roles:{server_id}         → Server roles JSON (TTL 120s)
    cache:server:{id}               → Server JSON (TTL 60s)
```

Cache invalidation:
- On write (message send, channel update, etc.): delete cache key
- TTL-based expiry as safety net
- Use Redis pub/sub for cross-instance cache invalidation (if multiple API instances)

```rust
pub struct CacheLayer {
    redis: RedisClient,
}

impl CacheLayer {
    pub async fn get_or_fetch<T, F, Fut>(
        &self,
        key: &str,
        ttl_seconds: u64,
        fetch: F,
    ) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        // Try cache
        if let Some(cached) = self.redis.get::<String>(key).await? {
            return Ok(serde_json::from_str(&cached)?);
        }

        // Fetch from database
        let value = fetch().await?;

        // Store in cache
        let json = serde_json::to_string(&value)?;
        self.redis.set_ex(key, &json, ttl_seconds).await?;

        Ok(value)
    }

    pub async fn invalidate(&self, key: &str) -> Result<()> {
        self.redis.del(key).await?;
        Ok(())
    }

    pub async fn invalidate_pattern(&self, pattern: &str) -> Result<()> {
        // SCAN for matching keys and delete
        // Use in batch operations (e.g., server delete)
    }
}
```

### 16.4 — Consistent Hash Routing

When running multiple API instances, route requests for the same channel to the same instance for optimal coalescing:

```rust
use std::hash::{Hash, Hasher};

pub struct ConsistentHashRouter {
    ring: Vec<(u64, String)>,  // (hash_point, instance_id)
}

impl ConsistentHashRouter {
    /// Determine which instance should handle requests for this channel
    pub fn route(&self, channel_id: Snowflake) -> &str {
        let hash = self.hash_key(channel_id);
        let idx = self.ring.partition_point(|(h, _)| *h < hash) % self.ring.len();
        &self.ring[idx].1
    }
}
```

In practice: use NATS request-reply pattern. API handler publishes request to `data.channel.{channel_id}`, and the data service instance responsible for that channel's hash range responds.

### 16.5 — Connection Pool Optimization

ScyllaDB driver configuration:
```rust
pub async fn create_scylla_session(config: &ScyllaConfig) -> Result<Session> {
    SessionBuilder::new()
        .known_nodes(&config.nodes)
        .default_consistency(Consistency::LocalQuorum)
        .connection_pool_size(PoolSize::PerShard(NonZeroUsize::new(4).unwrap()))
        .schema_agreement_interval(Duration::from_secs(1))
        .build()
        .await
}
```

PostgreSQL pool via PgBouncer:
```rust
pub async fn create_pg_pool(config: &DatabaseConfig) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(config.max_connections) // Match PgBouncer's default_pool_size
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(3))
        .idle_timeout(Duration::from_secs(600))
        .max_lifetime(Duration::from_secs(1800))
        .connect(&config.url)
        .await
}
```

### 16.6 — Metrics

Track data service layer performance:
- Coalesce hit rate (how often requests are merged)
- Cache hit rate (Redis cache hits vs misses)
- Database query latency (p50, p95, p99)
- In-flight request count
- Cache key count and memory usage

## Acceptance Criteria

- [ ] Request coalescing: 1000 concurrent requests for same channel → 1 DB query
- [ ] Coalesced requests all receive correct result
- [ ] Redis cache reduces database load for hot channels
- [ ] Cache invalidation on writes works correctly
- [ ] ScyllaDB connection pool properly configured with shard awareness
- [ ] PostgreSQL connection pool goes through PgBouncer
- [ ] Consistent hash routing directs same channel to same data service instance
- [ ] Metrics exposed: coalesce rate, cache hit rate, query latency
- [ ] Load test: 10,000 users opening same channel simultaneously → verify coalescing
- [ ] Benchmark: before/after coalescing comparison for hot channels
