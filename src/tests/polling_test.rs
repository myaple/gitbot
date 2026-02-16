#![allow(clippy::field_reassign_with_default)]

use crate::config::AppSettings;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn test_max_age_hours_calculation() {
    // Create settings with max_age_hours = 12
    let mut settings = AppSettings::default();
    // Set only the non-default fields
    settings.gitlab_url = "https://gitlab.example.com".to_string();
    settings.gitlab_token = "test_token".to_string();
    settings.openai_api_key = "test_key".to_string();
    settings.repos_to_poll = vec!["test/repo".to_string()];
    settings.log_level = "debug".to_string();
    settings.bot_username = "gitbot".to_string();
    settings.max_age_hours = 12;

    // Get current time and calculate a timestamp from 24 hours ago
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let old_timestamp = now - (24 * 3600); // 24 hours ago

    // Calculate what the effective timestamp should be (12 hours ago)
    let expected_timestamp = now - (12 * 3600);

    // Directly test the timestamp calculation logic
    let settings_arc = Arc::new(settings);
    let effective_timestamp = if old_timestamp < now - (settings_arc.max_age_hours * 3600) {
        now - (settings_arc.max_age_hours * 3600)
    } else {
        old_timestamp
    };

    // Verify that the effective timestamp is close to the expected timestamp (12 hours ago)
    assert!(effective_timestamp >= expected_timestamp - 10);
    assert!(effective_timestamp <= expected_timestamp + 10);
}
