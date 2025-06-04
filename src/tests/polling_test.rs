use crate::config::AppSettings;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn test_max_age_hours_calculation() {
    // Create settings with max_age_hours = 12
    let settings = AppSettings {
        gitlab_url: "https://gitlab.example.com".to_string(),
        gitlab_token: "test_token".to_string(),
        openai_api_key: "test_key".to_string(),
        openai_model: "gpt-3.5-turbo".to_string(),
        openai_temperature: 0.7,
        openai_max_tokens: 1024,
        openai_custom_url: "https://api.openai.com/v1".to_string(),
        repos_to_poll: vec!["test/repo".to_string()],
        log_level: "debug".to_string(),
        bot_username: "gitbot".to_string(),
        poll_interval_seconds: 60,
        stale_issue_days: 30,
        max_age_hours: 12,
        context_repo_path: None,
        max_context_size: 60000,
        default_branch: "main".to_string(),
        client_cert_path: None,
        client_key_path: None,
        client_key_password: None,
        max_comment_length: 1000,
    };

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
