use crate::snowflake::Snowflake;
use chrono::{DateTime, Utc};

/// Number of days per ScyllaDB bucket
const BUCKET_SIZE_DAYS: i32 = 10;

/// Custom epoch: 2024-01-01T00:00:00Z in seconds
const LUMIERE_EPOCH_SECS: i64 = 1_704_067_200;

/// Calculate ScyllaDB bucket from a Snowflake ID
pub fn bucket_from_snowflake(snowflake: Snowflake) -> i32 {
    let dt = snowflake.created_at();
    bucket_from_datetime(dt)
}

/// Calculate bucket from a chrono DateTime
pub fn bucket_from_datetime(dt: DateTime<Utc>) -> i32 {
    let epoch_days = ((dt.timestamp() - LUMIERE_EPOCH_SECS) / 86_400) as i32;
    epoch_days / BUCKET_SIZE_DAYS
}

/// Get the current bucket
pub fn current_bucket() -> i32 {
    bucket_from_datetime(Utc::now())
}

/// Get all buckets between two Snowflake IDs (inclusive, descending)
pub fn bucket_range(from: Snowflake, to: Snowflake) -> Vec<i32> {
    let from_bucket = bucket_from_snowflake(from);
    let to_bucket = bucket_from_snowflake(to);

    let (low, high) = if from_bucket <= to_bucket {
        (from_bucket, to_bucket)
    } else {
        (to_bucket, from_bucket)
    };

    (low..=high).rev().collect()
}

/// Get buckets from a given snowflake back to a minimum bucket (descending)
pub fn buckets_before(before: Snowflake, min_bucket: i32) -> Vec<i32> {
    let start_bucket = bucket_from_snowflake(before);
    (min_bucket..=start_bucket).rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_bucket_from_datetime() {
        // 2024-01-01 = bucket 0
        let dt = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(bucket_from_datetime(dt), 0);

        // 2024-01-10 = day 9 → bucket 0
        let dt = Utc.with_ymd_and_hms(2024, 1, 10, 0, 0, 0).unwrap();
        assert_eq!(bucket_from_datetime(dt), 0);

        // 2024-01-11 = day 10 → bucket 1
        let dt = Utc.with_ymd_and_hms(2024, 1, 11, 0, 0, 0).unwrap();
        assert_eq!(bucket_from_datetime(dt), 1);

        // 2024-02-01 = day 31 → bucket 3
        let dt = Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap();
        assert_eq!(bucket_from_datetime(dt), 3);
    }

    #[test]
    fn test_current_bucket_is_positive() {
        assert!(current_bucket() > 0);
    }

    #[test]
    fn test_bucket_range() {
        // Create two snowflakes with known timestamps
        let gen = crate::snowflake::SnowflakeGenerator::new(1);
        let id1 = gen.next_id();
        let id2 = gen.next_id();

        let range = bucket_range(id1, id2);
        // Both generated at the same time, should be single bucket
        assert_eq!(range.len(), 1);
    }

    #[test]
    fn test_buckets_before() {
        let gen = crate::snowflake::SnowflakeGenerator::new(1);
        let id = gen.next_id();
        let buckets = buckets_before(id, 0);

        // Should include all buckets from 0 to current
        assert!(!buckets.is_empty());
        // First bucket should be the highest (descending)
        assert!(buckets[0] >= buckets[buckets.len() - 1]);
        // Last bucket should be 0
        assert_eq!(*buckets.last().unwrap(), 0);
    }
}
