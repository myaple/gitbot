use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// A log deduplicator that prevents repetitive log messages
/// Tracks messages and only allows them to be logged once per time window
#[derive(Clone)]
pub struct LogDeduplicator {
    /// Map of log key -> last logged time
    last_logged: Arc<Mutex<HashMap<String, Instant>>>,
    /// How long to suppress duplicate messages
    suppression_window: Duration,
}

impl LogDeduplicator {
    /// Create a new log deduplicator with the given suppression window
    pub fn new(suppression_window: Duration) -> Self {
        Self {
            last_logged: Arc::new(Mutex::new(HashMap::new())),
            suppression_window,
        }
    }

    /// Check if a log message should be allowed (not a duplicate within the window)
    /// Returns true if the message should be logged, false if it should be suppressed
    pub async fn should_log(&self, key: &str) -> bool {
        let mut map = self.last_logged.lock().await;
        let now = Instant::now();

        if let Some(last_time) = map.get(key) {
            if now.duration_since(*last_time) < self.suppression_window {
                // Still within suppression window, don't log
                return false;
            }
        }

        // Update the last logged time
        map.insert(key.to_string(), now);
        true
    }

    /// Clear old entries to prevent memory growth
    /// Call this periodically to clean up the map
    #[allow(dead_code)]
    pub async fn cleanup(&self) {
        let mut map = self.last_logged.lock().await;
        let now = Instant::now();

        // Remove entries older than 2x the suppression window
        map.retain(|_, last_time| now.duration_since(*last_time) < self.suppression_window * 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_deduplication() {
        let dedup = LogDeduplicator::new(Duration::from_millis(100));

        // First call should allow logging
        assert!(dedup.should_log("test_message").await);

        // Immediate second call should suppress
        assert!(!dedup.should_log("test_message").await);

        // After window expires, should allow again
        sleep(Duration::from_millis(150)).await;
        assert!(dedup.should_log("test_message").await);
    }

    #[tokio::test]
    async fn test_different_keys() {
        let dedup = LogDeduplicator::new(Duration::from_millis(100));

        // Different keys should not interfere
        assert!(dedup.should_log("message1").await);
        assert!(dedup.should_log("message2").await);
        assert!(!dedup.should_log("message1").await);
        assert!(!dedup.should_log("message2").await);
    }

    #[tokio::test]
    async fn test_cleanup() {
        let dedup = LogDeduplicator::new(Duration::from_millis(50));

        dedup.should_log("test1").await;
        dedup.should_log("test2").await;
        dedup.should_log("test3").await;

        // Map should have 3 entries
        assert_eq!(dedup.last_logged.lock().await.len(), 3);

        // Wait for entries to expire
        sleep(Duration::from_millis(150)).await;

        // Cleanup should remove old entries
        dedup.cleanup().await;
        assert_eq!(dedup.last_logged.lock().await.len(), 0);
    }
}
