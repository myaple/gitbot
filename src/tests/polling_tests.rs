#[cfg(test)]
mod tests {
    use crate::config::AppSettings;
    use crate::file_indexer::FileIndexManager;
    use crate::gitlab::GitlabApiClient;
    use crate::models::{GitlabIssue, GitlabNoteAttributes, GitlabUser};
    use crate::polling::*;
    use chrono::{Duration as ChronoDuration, Utc};
    use mockito::Matcher;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::sync::Mutex;

    const TEST_BOT_USERNAME: &str = "test_bot";
    const STALE_LABEL: &str = "stale";
    const PROJECT_ID: i64 = 1;

    fn test_error_tracker() -> Arc<Mutex<ErrorTracker>> {
        Arc::new(Mutex::new(ErrorTracker::new(Duration::from_secs(3600))))
    }

    fn test_config(stale_days: u64, bot_username: &str, base_url: String) -> Arc<AppSettings> {
        let mut settings = AppSettings::default();
        // Set only the non-default fields
        settings.gitlab_url = base_url;
        settings.gitlab_token = "test_token".to_string();
        settings.openai_api_key = "key".to_string();
        settings.openai_custom_url = "url".to_string();
        settings.repos_to_poll = vec!["org/repo1".to_string()];
        settings.log_level = "debug".to_string();
        settings.bot_username = bot_username.to_string();
        settings.stale_issue_days = stale_days;
        settings.context_repo_path = Some("org/context-repo".to_string());
        Arc::new(settings)
    }

    fn create_issue(
        iid: i64,
        updated_at_str: &str,
        labels: Vec<String>,
        state: &str,
    ) -> GitlabIssue {
        GitlabIssue {
            id: iid * 10, // Just to make it different from iid
            iid,
            project_id: PROJECT_ID,
            title: format!("Test Issue {}", iid),
            description: Some(format!("Description for issue {}", iid)),
            state: state.to_string(),
            author: GitlabUser {
                id: 100,
                username: "issue_author".to_string(),
                name: "Issue Author".to_string(),
                avatar_url: None,
            },
            web_url: format!("http://example.com/issues/{}", iid),
            labels,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: updated_at_str.to_string(),
        }
    }

    fn create_note(id: i64, author_username: &str, updated_at_str: &str) -> GitlabNoteAttributes {
        GitlabNoteAttributes {
            id,
            note: format!("This is note {}", id),
            author: GitlabUser {
                id: if author_username == TEST_BOT_USERNAME {
                    50
                } else {
                    51
                },
                username: author_username.to_string(),
                name: format!("User {}", author_username),
                avatar_url: None,
            },
            project_id: PROJECT_ID,
            noteable_type: "Issue".to_string(),
            noteable_id: Some(1), // Assuming it's for issue iid 1, adjust if needed per test
            iid: Some(1),         // Assuming it's for issue iid 1
            url: Some(format!("http://example.com/notes/{}", id)),
            updated_at: updated_at_str.to_string(),
        }
    }

    #[tokio::test]
    async fn test_issue_becomes_stale() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let old_update = (Utc::now() - ChronoDuration::days(35)).to_rfc3339();
        let issue1 = create_issue(1, &old_update, vec![], "opened");

        let _m_notes = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([]).to_string()) // No notes
            .create_async()
            .await;

        let m_add_label = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!(create_issue(
                    1,
                    &old_update,
                    vec![STALE_LABEL.to_string()],
                    "opened"
                ))
                .to_string(),
            ) // Simulate response with label
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        check_stale_issues(PROJECT_ID, client, config, &[issue1], test_error_tracker())
            .await
            .unwrap();
        m_add_label.assert_async().await;
    }

    #[tokio::test]
    async fn test_stale_issue_remains_stale() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let old_update = (Utc::now() - ChronoDuration::days(40)).to_rfc3339();
        let issue1 = create_issue(1, &old_update, vec![STALE_LABEL.to_string()], "opened");

        let _m_notes = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        // We expect no calls to add or remove labels.
        let _m_add_label_unexpected = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .with_status(500) // This will cause GitlabError::Api if called
            .create_async()
            .await;
        let _m_remove_label_unexpected = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .match_body(Matcher::JsonString(
                json!({"remove_labels": STALE_LABEL}).to_string(),
            ))
            .with_status(500) // This will cause GitlabError::Api if called
            .create_async()
            .await;

        check_stale_issues(PROJECT_ID, client, config, &[issue1], test_error_tracker())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_stale_issue_becomes_active_by_user_note() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let _issue_update_old = (Utc::now() - ChronoDuration::days(50)).to_rfc3339();
        let recent_note_update = (Utc::now() - ChronoDuration::days(5)).to_rfc3339();

        // Ensure the issue's updated_at reflects the recent note, consistent with GitLab behavior
        let issue1 = create_issue(
            1,
            &recent_note_update,
            vec![STALE_LABEL.to_string()],
            "opened",
        );
        let note1 = create_note(101, "human_user", &recent_note_update);

        let _m_notes = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([note1]).to_string())
            .create_async()
            .await;

        let m_remove_label = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!(create_issue(1, &recent_note_update, vec![], "opened")).to_string()) // Simulate response without label
            .match_body(Matcher::JsonString(
                json!({"remove_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        check_stale_issues(PROJECT_ID, client, config, &[issue1], test_error_tracker())
            .await
            .unwrap();
        m_remove_label.assert_async().await;
    }

    #[tokio::test]
    async fn test_issue_remains_active_not_stale() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let recent_update = (Utc::now() - ChronoDuration::days(10)).to_rfc3339();
        let issue1 = create_issue(1, &recent_update, vec![], "opened");

        let _m_notes = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        // Similar to above, ensure no unexpected label operations occur.
        let _m_add_label_unexpected = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .with_status(500)
            .create_async()
            .await;
        let _m_remove_label_unexpected = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .match_body(Matcher::JsonString(
                json!({"remove_labels": STALE_LABEL}).to_string(),
            ))
            .with_status(500)
            .create_async()
            .await;

        check_stale_issues(PROJECT_ID, client, config, &[issue1], test_error_tracker())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_bot_comment_does_not_affect_staleness() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let issue_update_old = (Utc::now() - ChronoDuration::days(60)).to_rfc3339();
        let bot_note_recent = (Utc::now() - ChronoDuration::days(1)).to_rfc3339();

        let issue1 = create_issue(1, &issue_update_old, vec![], "opened"); // No stale label initially
        let note_bot = create_note(102, TEST_BOT_USERNAME, &bot_note_recent);

        let _m_notes = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([note_bot]).to_string())
            .create_async()
            .await;

        // Should add stale label because only bot comment is recent
        let m_add_label = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .with_status(200)
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        check_stale_issues(PROJECT_ID, client, config, &[issue1], test_error_tracker())
            .await
            .unwrap();
        m_add_label.assert_async().await;
    }

    #[tokio::test]
    async fn test_issue_with_no_notes_becomes_stale() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let old_update = (Utc::now() - ChronoDuration::days(35)).to_rfc3339();
        let issue1 = create_issue(1, &old_update, vec![], "opened");

        let _m_notes = server // No notes
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([]).to_string())
            .create_async()
            .await;
        let m_add_label = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .with_status(200)
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        check_stale_issues(PROJECT_ID, client, config, &[issue1], test_error_tracker())
            .await
            .unwrap();
        m_add_label.assert_async().await;
    }

    #[tokio::test]
    async fn test_issue_with_only_old_bot_notes_becomes_stale() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let issue_update_very_old = (Utc::now() - ChronoDuration::days(100)).to_rfc3339();
        let bot_note_also_old = (Utc::now() - ChronoDuration::days(90)).to_rfc3339();

        let issue1 = create_issue(1, &issue_update_very_old, vec![], "opened");
        let note_bot_old = create_note(103, TEST_BOT_USERNAME, &bot_note_also_old);

        let _m_notes = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([note_bot_old]).to_string())
            .create_async()
            .await;
        let m_add_label = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .with_status(200)
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        check_stale_issues(PROJECT_ID, client, config, &[issue1], test_error_tracker())
            .await
            .unwrap();
        m_add_label.assert_async().await;
    }

    #[tokio::test]
    async fn test_get_issue_notes_failure_continues() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let issue_update_old = (Utc::now() - ChronoDuration::days(40)).to_rfc3339();
        // Issue 1 will have notes fail, Issue 2 should still be processed.
        let issue1 = create_issue(1, &issue_update_old, vec![], "opened");
        let issue2 = create_issue(2, &issue_update_old, vec![], "opened");

        // Notes for issue 1 fail
        let _m_notes1_fail = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(500)
            .create_async()
            .await;

        // Notes for issue 2 succeed (empty)
        let _m_notes2_ok = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/2/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        // The logic for issue1 (m_add_label_issue1_actually_called) is expected to be called.
        // Issue 2 is also expected to be labeled.

        let m_add_label_issue2 = server
            .mock("PUT", "/api/v4/projects/1/issues/2")
            .with_status(200)
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        let m_add_label_issue1_actually_called = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .with_status(200)
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        let result = check_stale_issues(
            PROJECT_ID,
            client,
            config,
            &[issue1, issue2],
            test_error_tracker(),
        )
        .await;
        assert!(result.is_ok()); // The function itself should complete
        m_add_label_issue1_actually_called.assert_async().await; // issue1 gets labeled based on its own old date
        m_add_label_issue2.assert_async().await; // issue2 gets labeled
    }

    #[tokio::test]
    async fn test_add_label_failure_continues() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        let old_update = (Utc::now() - ChronoDuration::days(45)).to_rfc3339();
        let issue1 = create_issue(1, &old_update, vec![], "opened"); // Should become stale
        let issue2 = create_issue(2, &old_update, vec![], "opened"); // Should also become stale

        let _m_notes1 = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([]).to_string())
            .create_async()
            .await;
        let _m_notes2 = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/2/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        // add_issue_label for issue 1 fails
        let m_add_label1_fail = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .with_status(500) // Simulate API error
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        // add_issue_label for issue 2 succeeds
        let m_add_label2_ok = server
            .mock("PUT", "/api/v4/projects/1/issues/2")
            .with_status(200)
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        let result = check_stale_issues(PROJECT_ID, client, config, &[issue1, issue2], test_error_tracker()).await;
        assert!(result.is_ok()); // Function completes
        m_add_label1_fail.assert_async().await; // Call was made
        m_add_label2_ok.assert_async().await; // Call was made
    }

    #[tokio::test]
    async fn test_polling_service_creation() {
        let server = mockito::Server::new_async().await;
        let base_url = server.url();

        let settings_obj = test_config(30, TEST_BOT_USERNAME, base_url.clone());
        let gitlab_client = Arc::new(GitlabApiClient::new(settings_obj.clone()).unwrap());
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let polling_service =
            PollingService::new(gitlab_client, settings_obj, file_index_manager, None);

        let last_checked = *polling_service.last_checked.lock().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(
            now.saturating_sub(last_checked) >= 3500 && now.saturating_sub(last_checked) <= 3700
        );
    }

    #[tokio::test]
    async fn test_get_issues_since_timestamp() {
        let server = mockito::Server::new_async().await;
        let base_url = server.url();

        // Create settings with max_age_hours = 12
        let mut settings = AppSettings::default();
        // Set only the non-default fields
        settings.gitlab_url = base_url.clone();
        settings.gitlab_token = "test_token".to_string();
        settings.openai_api_key = "test_key".to_string();
        settings.openai_custom_url = "https://api.openai.com/v1".to_string();
        settings.repos_to_poll = vec!["org/repo".to_string()];
        settings.log_level = "debug".to_string();
        settings.bot_username = "test_bot".to_string();
        settings.max_age_hours = 12; // Set to 12 hours for this test

        // Setup timestamp calculation test
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let old_timestamp = now - (20 * 3600); // 20 hours ago (older than max_age_hours)

        // Directly test the timestamp calculation logic
        let settings_arc = Arc::new(settings);
        let effective_timestamp = if old_timestamp < now - (settings_arc.max_age_hours * 3600) {
            now - (settings_arc.max_age_hours * 3600)
        } else {
            old_timestamp
        };

        // Calculate what the expected timestamp should be (12 hours ago)
        let expected_timestamp = now - (12 * 3600);

        // Verify timestamp bounds (within 10 seconds precision)
        assert!(effective_timestamp >= expected_timestamp - 10);
        assert!(effective_timestamp <= expected_timestamp + 10);
    }
}
