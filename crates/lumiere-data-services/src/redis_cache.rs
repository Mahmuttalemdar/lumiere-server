use redis::AsyncCommands;
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;

use crate::error::DataServiceError;

/// Thin wrapper around a Redis connection manager for typed get/set with TTL.
#[derive(Clone)]
pub struct RedisCache {
    conn: redis::aio::ConnectionManager,
    /// Key prefix to namespace all cache entries (e.g. "lumiere:cache:").
    prefix: String,
}

impl RedisCache {
    pub fn new(conn: redis::aio::ConnectionManager, prefix: impl Into<String>) -> Self {
        Self {
            conn,
            prefix: prefix.into(),
        }
    }

    fn prefixed_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// Get a cached value, returning None on cache miss.
    pub async fn get<V: DeserializeOwned>(&self, key: &str) -> Result<Option<V>, DataServiceError> {
        let full_key = self.prefixed_key(key);
        let mut conn = self.conn.clone();
        let raw: Option<String> = conn.get(&full_key).await?;
        match raw {
            Some(json) => {
                let value = serde_json::from_str(&json)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Set a value in cache with a TTL.
    pub async fn set<V: Serialize>(
        &self,
        key: &str,
        value: &V,
        ttl: Duration,
    ) -> Result<(), DataServiceError> {
        let full_key = self.prefixed_key(key);
        let json = serde_json::to_string(value)?;
        let mut conn = self.conn.clone();
        conn.set_ex::<_, _, ()>(&full_key, &json, ttl.as_secs().max(1))
            .await?;
        Ok(())
    }

    /// Delete a cached entry.
    pub async fn delete(&self, key: &str) -> Result<(), DataServiceError> {
        let full_key = self.prefixed_key(key);
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(&full_key).await?;
        Ok(())
    }

    /// Delete all entries matching a pattern (e.g. "user:123:*").
    /// Uses SCAN to avoid blocking Redis on large keyspaces.
    pub async fn delete_pattern(&self, pattern: &str) -> Result<u64, DataServiceError> {
        let full_pattern = self.prefixed_key(pattern);
        let mut conn = self.conn.clone();

        // Use SCAN instead of KEYS to avoid blocking Redis.
        let mut keys: Vec<String> = Vec::new();
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&full_pattern)
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
            return Ok(0);
        }

        let count = keys.len() as u64;

        // Delete keys in batches using a single DEL command with multiple keys.
        for chunk in keys.chunks(100) {
            let mut cmd = redis::cmd("DEL");
            for key in chunk {
                cmd.arg(key);
            }
            cmd.query_async::<()>(&mut conn).await?;
        }

        Ok(count)
    }
}
