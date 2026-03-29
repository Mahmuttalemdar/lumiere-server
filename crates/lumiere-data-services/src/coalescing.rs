use dashmap::DashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::error::DataServiceError;

/// Request coalescing cache. When multiple callers request the same key
/// simultaneously, only one fetch executes and all waiters receive the result.
///
/// This is Discord's "request coalescing" pattern: if 10,000 users open the
/// same channel at once, only ONE database query runs.
pub struct CoalescingCache<K, V> {
    /// In-flight requests: key -> broadcast sender that will deliver the result.
    in_flight: DashMap<K, broadcast::Sender<Arc<V>>>,
}

impl<K, V> CoalescingCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            in_flight: DashMap::new(),
        }
    }

    /// Fetch a value by key. If another task is already fetching the same key,
    /// this call will wait for that result instead of issuing a duplicate query.
    ///
    /// The `fetch` closure is only called if no in-flight request exists for
    /// this key. All concurrent callers for the same key share one result.
    pub async fn get_or_fetch<F, Fut>(
        &self,
        key: K,
        fetch: F,
    ) -> Result<Arc<V>, DataServiceError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, anyhow::Error>>,
    {
        // Fast path: check if someone is already fetching this key.
        if let Some(entry) = self.in_flight.get(&key) {
            let mut rx = entry.value().subscribe();
            // Drop the dashmap ref before awaiting to avoid holding the shard lock.
            drop(entry);
            return rx
                .recv()
                .await
                .map_err(|_| DataServiceError::CoalescingFailed);
        }

        // Slow path: we might be the first. Insert a broadcast channel.
        // Use entry API to handle the race where another task inserted between
        // our get() and this insert.
        let (tx, is_leader) = {
            let entry = self.in_flight.entry(key.clone());
            match entry {
                dashmap::mapref::entry::Entry::Occupied(occ) => {
                    let tx = occ.get().clone();
                    (tx, false)
                }
                dashmap::mapref::entry::Entry::Vacant(vac) => {
                    // Buffer of 64 for safety — handles many concurrent subscribers.
                    let (tx, _) = broadcast::channel(64);
                    vac.insert(tx.clone());
                    (tx, true)
                }
            }
        };

        if !is_leader {
            // Another task won the race. Subscribe and wait.
            let mut rx = tx.subscribe();
            return rx
                .recv()
                .await
                .map_err(|_| DataServiceError::CoalescingFailed);
        }

        // We are the leader: execute the fetch.
        let result = fetch().await;

        match result {
            Ok(value) => {
                let arc_value = Arc::new(value);
                // Broadcast to all waiters. Ignore send errors (no receivers is fine).
                let _ = tx.send(arc_value.clone());
                // Remove the in-flight entry AFTER sending, so subscribers created
                // before the send can still receive the value.
                self.in_flight.remove(&key);
                Ok(arc_value)
            }
            Err(e) => {
                // Drop the sender — all waiting receivers will get RecvError.
                drop(tx);
                // Remove after dropping the sender so no new subscribers see a dead channel.
                self.in_flight.remove(&key);
                Err(DataServiceError::Fetch(e))
            }
        }
    }

    /// Number of currently in-flight requests.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

impl<K, V> Default for CoalescingCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_single_fetch() {
        let cache = CoalescingCache::<String, String>::new();
        let result = cache
            .get_or_fetch("key1".to_string(), || async {
                Ok("value1".to_string())
            })
            .await
            .unwrap();
        assert_eq!(*result, "value1");
        assert_eq!(cache.in_flight_count(), 0);
    }

    #[tokio::test]
    async fn test_coalescing_only_one_fetch() {
        let cache = Arc::new(CoalescingCache::<String, String>::new());
        let fetch_count = Arc::new(AtomicU32::new(0));

        let mut handles = vec![];

        for _ in 0..50 {
            let cache = Arc::clone(&cache);
            let fetch_count = Arc::clone(&fetch_count);
            handles.push(tokio::spawn(async move {
                cache
                    .get_or_fetch("shared_key".to_string(), || {
                        let fc = Arc::clone(&fetch_count);
                        async move {
                            fc.fetch_add(1, Ordering::SeqCst);
                            // Simulate slow DB query.
                            sleep(Duration::from_millis(50)).await;
                            Ok("shared_value".to_string())
                        }
                    })
                    .await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap().unwrap();
            assert_eq!(*result, "shared_value");
        }

        // Only one fetch should have executed.
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);
        assert_eq!(cache.in_flight_count(), 0);
    }

    #[tokio::test]
    async fn test_fetch_error_propagates() {
        let cache = CoalescingCache::<String, String>::new();
        let result = cache
            .get_or_fetch("bad_key".to_string(), || async {
                Err(anyhow::anyhow!("db is down"))
            })
            .await;
        assert!(result.is_err());
        assert_eq!(cache.in_flight_count(), 0);
    }

    #[tokio::test]
    async fn test_different_keys_independent() {
        let cache = Arc::new(CoalescingCache::<String, u32>::new());
        let fetch_count = Arc::new(AtomicU32::new(0));

        let mut handles = vec![];
        for i in 0..5 {
            let cache = Arc::clone(&cache);
            let fc = Arc::clone(&fetch_count);
            handles.push(tokio::spawn(async move {
                cache
                    .get_or_fetch(format!("key_{}", i), || {
                        let fc = fc.clone();
                        async move {
                            fc.fetch_add(1, Ordering::SeqCst);
                            Ok(i)
                        }
                    })
                    .await
                    .unwrap()
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap();
            assert_eq!(*result, i as u32);
        }

        assert_eq!(fetch_count.load(Ordering::SeqCst), 5);
    }
}
