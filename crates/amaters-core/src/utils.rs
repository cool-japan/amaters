//! Utility functions and helpers for AmateRS
//!
//! Common utility functions used across the codebase.

use crate::error::{AmateRSError, ErrorContext, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Get current timestamp in microseconds since UNIX epoch
pub fn current_timestamp_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_micros() as u64
}

/// Get current timestamp in milliseconds since UNIX epoch
pub fn current_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

/// Calculate exponential backoff delay
pub fn exponential_backoff(attempt: usize, base_delay_ms: u64, max_delay_ms: u64) -> Duration {
    let delay_ms = base_delay_ms * 2_u64.pow(attempt as u32);
    let delay_ms = delay_ms.min(max_delay_ms);
    Duration::from_millis(delay_ms)
}

/// Calculate linear backoff delay
pub fn linear_backoff(attempt: usize, delay_ms: u64, max_attempts: usize) -> Option<Duration> {
    if attempt < max_attempts {
        Some(Duration::from_millis(delay_ms))
    } else {
        None
    }
}

/// Format bytes as human-readable string
pub fn format_bytes(bytes: usize) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}

/// Format duration as human-readable string
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let nanos = duration.subsec_nanos();

    if secs == 0 {
        if nanos < 1_000 {
            format!("{}ns", nanos)
        } else if nanos < 1_000_000 {
            format!("{:.2}µs", nanos as f64 / 1_000.0)
        } else {
            format!("{:.2}ms", nanos as f64 / 1_000_000.0)
        }
    } else if secs < 60 {
        format!("{:.2}s", secs as f64 + nanos as f64 / 1_000_000_000.0)
    } else if secs < 3600 {
        let mins = secs / 60;
        let secs = secs % 60;
        format!("{}m {}s", mins, secs)
    } else {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{}h {}m", hours, mins)
    }
}

/// Round up to next power of 2
pub fn next_power_of_two(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut power = 1;
    while power < n {
        power *= 2;
    }
    power
}

/// Check if a number is power of 2
pub fn is_power_of_two(n: usize) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

/// Align value to alignment boundary
pub fn align_to(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

/// Calculate checksum for data
pub fn calculate_checksum(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// Verify checksum
pub fn verify_checksum(data: &[u8], expected: u32) -> Result<()> {
    let actual = calculate_checksum(data);
    if actual == expected {
        Ok(())
    } else {
        Err(AmateRSError::StorageIntegrity(ErrorContext::new(format!(
            "Checksum mismatch: expected {}, got {}",
            expected, actual
        ))))
    }
}

/// Retry a function with exponential backoff
pub async fn retry_with_backoff<F, T, E>(
    mut f: F,
    max_attempts: usize,
    base_delay_ms: u64,
) -> std::result::Result<T, E>
where
    F: FnMut() -> std::result::Result<T, E>,
{
    let mut attempt = 0;
    loop {
        match f() {
            Ok(result) => return Ok(result),
            Err(e) => {
                attempt += 1;
                if attempt >= max_attempts {
                    return Err(e);
                }
                let delay = exponential_backoff(attempt, base_delay_ms, 30_000);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_timestamp() {
        let ts_micros = current_timestamp_micros();
        let ts_millis = current_timestamp_millis();

        assert!(ts_micros > 0);
        assert!(ts_millis > 0);
        assert!(ts_micros > ts_millis * 1000);
    }

    #[test]
    fn test_exponential_backoff() {
        let delay1 = exponential_backoff(0, 100, 10_000);
        let delay2 = exponential_backoff(1, 100, 10_000);
        let delay3 = exponential_backoff(2, 100, 10_000);

        assert!(delay2 > delay1);
        assert!(delay3 > delay2);

        // Test max delay
        let delay_max = exponential_backoff(20, 100, 1_000);
        assert_eq!(delay_max, Duration::from_millis(1_000));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1536 * 1024), "1.50 MB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_nanos(500)), "500ns");
        assert_eq!(format_duration(Duration::from_micros(1500)), "1.50ms");
        assert_eq!(format_duration(Duration::from_millis(2500)), "2.50s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3665)), "1h 1m");
    }

    #[test]
    fn test_power_of_two() {
        assert!(is_power_of_two(1));
        assert!(is_power_of_two(2));
        assert!(is_power_of_two(4));
        assert!(is_power_of_two(1024));
        assert!(!is_power_of_two(3));
        assert!(!is_power_of_two(1000));

        assert_eq!(next_power_of_two(0), 1);
        assert_eq!(next_power_of_two(1), 1);
        assert_eq!(next_power_of_two(3), 4);
        assert_eq!(next_power_of_two(1000), 1024);
    }

    #[test]
    fn test_align_to() {
        assert_eq!(align_to(0, 4), 0);
        assert_eq!(align_to(1, 4), 4);
        assert_eq!(align_to(4, 4), 4);
        assert_eq!(align_to(5, 4), 8);
        assert_eq!(align_to(100, 64), 128);
    }

    #[test]
    fn test_checksum() -> Result<()> {
        let data = b"test data";
        let checksum = calculate_checksum(data);

        // Verify should pass
        verify_checksum(data, checksum)?;

        // Verify should fail with wrong checksum
        assert!(verify_checksum(data, checksum + 1).is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_retry_with_backoff() {
        let mut attempts = 0;
        let result = retry_with_backoff(
            || {
                attempts += 1;
                if attempts < 3 { Err("fail") } else { Ok(42) }
            },
            5,
            1, // 1ms base delay for fast test
        )
        .await;

        assert_eq!(result, Ok(42));
        assert_eq!(attempts, 3);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_max_attempts() {
        let mut attempts = 0;
        let result: std::result::Result<i32, &str> = retry_with_backoff(
            || {
                attempts += 1;
                Err("always fail")
            },
            3,
            1,
        )
        .await;

        assert_eq!(result, Err("always fail"));
        assert_eq!(attempts, 3);
    }
}
