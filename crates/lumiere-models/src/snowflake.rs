use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

/// Custom epoch: 2024-01-01T00:00:00Z in milliseconds
const LUMIERE_EPOCH: u64 = 1_704_067_200_000;

const MACHINE_ID_BITS: u64 = 10;
const SEQUENCE_BITS: u64 = 12;

const MAX_MACHINE_ID: u16 = (1 << MACHINE_ID_BITS) - 1;
const MAX_SEQUENCE: u16 = (1 << SEQUENCE_BITS) - 1;

const MACHINE_ID_SHIFT: u64 = SEQUENCE_BITS;
const TIMESTAMP_SHIFT: u64 = MACHINE_ID_BITS + SEQUENCE_BITS;

/// 64-bit Snowflake ID: [41-bit timestamp][10-bit machine_id][12-bit sequence]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Snowflake(pub u64);

impl Snowflake {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn value(&self) -> u64 {
        self.0
    }

    /// Extract timestamp in milliseconds since Lumiere epoch
    pub fn timestamp_ms(&self) -> u64 {
        self.0 >> TIMESTAMP_SHIFT
    }

    /// Extract creation time as UTC datetime
    pub fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        let ms = self.timestamp_ms() + LUMIERE_EPOCH;
        chrono::DateTime::from_timestamp_millis(ms as i64).unwrap_or_default()
    }

    /// Extract machine ID
    pub fn machine_id(&self) -> u16 {
        ((self.0 >> MACHINE_ID_SHIFT) & MAX_MACHINE_ID as u64) as u16
    }

    /// Extract sequence number
    pub fn sequence(&self) -> u16 {
        (self.0 & MAX_SEQUENCE as u64) as u16
    }
}

impl fmt::Display for Snowflake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Snowflake {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Snowflake(s.parse()?))
    }
}

impl From<u64> for Snowflake {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<Snowflake> for u64 {
    fn from(s: Snowflake) -> Self {
        s.0
    }
}

impl From<i64> for Snowflake {
    fn from(v: i64) -> Self {
        Self(v as u64)
    }
}

impl From<Snowflake> for i64 {
    fn from(s: Snowflake) -> Self {
        s.0 as i64
    }
}

/// Serialize as string (JavaScript can't handle 64-bit integers)
impl Serialize for Snowflake {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// Deserialize from string or number
impl<'de> Deserialize<'de> for Snowflake {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SnowflakeVisitor;

        impl serde::de::Visitor<'_> for SnowflakeVisitor {
            type Value = Snowflake;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string or integer snowflake ID")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Snowflake, E> {
                Ok(Snowflake(v))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Snowflake, E> {
                Ok(Snowflake(v as u64))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Snowflake, E> {
                v.parse::<u64>()
                    .map(Snowflake)
                    .map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(SnowflakeVisitor)
    }
}

/// sqlx: encode Snowflake as i64 for PostgreSQL BIGINT
impl<'q> sqlx::Encode<'q, sqlx::Postgres> for Snowflake {
    fn encode_by_ref(
        &self,
        buf: &mut <sqlx::Postgres as sqlx::Database>::ArgumentBuffer<'q>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <i64 as sqlx::Encode<'q, sqlx::Postgres>>::encode_by_ref(&(self.0 as i64), buf)
    }
}

impl sqlx::Decode<'_, sqlx::Postgres> for Snowflake {
    fn decode(
        value: <sqlx::Postgres as sqlx::Database>::ValueRef<'_>,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let v = <i64 as sqlx::Decode<'_, sqlx::Postgres>>::decode(value)?;
        Ok(Snowflake(v as u64))
    }
}

impl sqlx::Type<sqlx::Postgres> for Snowflake {
    fn type_info() -> <sqlx::Postgres as sqlx::Database>::TypeInfo {
        <i64 as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

/// Thread-safe Snowflake ID generator.
/// Uses a single AtomicU64 packing (timestamp << 12 | sequence) to avoid race conditions.
pub struct SnowflakeGenerator {
    machine_id: u16,
    /// Packed state: upper 52 bits = timestamp, lower 12 bits = sequence
    state: AtomicU64,
}

impl SnowflakeGenerator {
    pub fn new(machine_id: u16) -> Self {
        assert!(
            machine_id <= MAX_MACHINE_ID,
            "machine_id must be <= {}",
            MAX_MACHINE_ID
        );
        Self {
            machine_id,
            state: AtomicU64::new(0),
        }
    }

    pub fn next_id(&self) -> Snowflake {
        loop {
            let now = current_timestamp_ms();
            let old_state = self.state.load(Ordering::Acquire);
            let old_ts = old_state >> SEQUENCE_BITS;
            let old_seq = (old_state & MAX_SEQUENCE as u64) as u16;

            let (new_ts, new_seq) = if now > old_ts {
                // New millisecond — reset sequence to 0
                (now, 0u16)
            } else {
                // Same millisecond (or clock went backwards) — increment sequence
                let next_seq = old_seq + 1;
                if next_seq > MAX_SEQUENCE {
                    // Sequence exhausted — spin wait for next millisecond
                    std::hint::spin_loop();
                    continue;
                }
                (old_ts, next_seq)
            };

            let new_state = (new_ts << SEQUENCE_BITS) | (new_seq as u64);

            if self
                .state
                .compare_exchange_weak(old_state, new_state, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                let id = (new_ts << TIMESTAMP_SHIFT)
                    | ((self.machine_id as u64) << MACHINE_ID_SHIFT)
                    | (new_seq as u64);
                return Snowflake(id);
            }
            // CAS failed — another thread won, retry
        }
    }
}

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_millis() as u64
        - LUMIERE_EPOCH
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_snowflake_uniqueness() {
        let gen = SnowflakeGenerator::new(1);
        let mut ids = HashSet::new();
        for _ in 0..10_000 {
            let id = gen.next_id();
            assert!(ids.insert(id.0), "duplicate ID generated: {}", id);
        }
    }

    #[test]
    fn test_snowflake_monotonic() {
        let gen = SnowflakeGenerator::new(1);
        let mut prev = gen.next_id();
        for _ in 0..1_000 {
            let next = gen.next_id();
            assert!(next > prev, "{} should be > {}", next, prev);
            prev = next;
        }
    }

    #[test]
    fn test_snowflake_field_extraction() {
        let gen = SnowflakeGenerator::new(42);
        let id = gen.next_id();
        assert_eq!(id.machine_id(), 42);
        assert!(id.timestamp_ms() > 0);
        assert!(id.created_at().timestamp() > 0);
    }

    #[test]
    fn test_snowflake_serde_string() {
        let id = Snowflake(123456789);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"123456789\"");

        let parsed: Snowflake = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_snowflake_serde_number() {
        let parsed: Snowflake = serde_json::from_str("123456789").unwrap();
        assert_eq!(parsed.0, 123456789);
    }

    #[test]
    fn test_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let gen = Arc::new(SnowflakeGenerator::new(1));
        let mut handles = vec![];

        for _ in 0..8 {
            let gen = Arc::clone(&gen);
            handles.push(thread::spawn(move || {
                let mut ids = Vec::with_capacity(10_000);
                for _ in 0..10_000 {
                    ids.push(gen.next_id().0);
                }
                ids
            }));
        }

        let mut all_ids = HashSet::new();
        for handle in handles {
            for id in handle.join().unwrap() {
                assert!(all_ids.insert(id), "duplicate ID across threads: {}", id);
            }
        }
        assert_eq!(all_ids.len(), 80_000);
    }
}
