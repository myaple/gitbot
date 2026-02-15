#[cfg(test)]
mod tests {
    use crate::config::AppSettings;
    use crate::file_indexer::FileIndexManager;
    use crate::gitlab::GitlabApiClient;
    use crate::models::{GitlabIssue, GitlabProject, GitlabUser};
    use crate::polling::*;
    use chrono::{Duration as ChronoDuration, Utc};
    use mockito::Matcher;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::SystemTime;

    const TEST_BOT_USERNAME: &str = "test_bot";
    const STALE_LABEL: &str = "stale";
    const PROJECT_ID: i64 = 1;

    fn test_config(stale_days: u64, bot_username: &str, base_url: String) -> Arc<AppSettings> {
        let mut settings = AppSettings::default();
        settings.gitlab_url = base_url;
        settings.gitlab_token = "test_token".to_string();
        settings.openai_api_key = "key".to_string();
        settings.repos_to_poll = vec!["org/repo1".to_string()];
        settings.bot_username = bot_username.to_string();
        settings.stale_issue_days = stale_days;
        Arc::new(settings)
    }

    fn create_issue(
        iid: i64,
        updated_at_str: &str,
        labels: Vec<String>,
        state: &str,
    ) -> GitlabIssue {
        GitlabIssue {
            id: iid * 10,
            iid,
            project_id: PROJECT_ID,
            title: format!("Test Issue {}", iid),
            description: Some("desc".to_string()),
            state: state.to_string(),
            author: GitlabUser {
                id: 100,
                username: "author".to_string(),
                name: "Author".to_string(),
                avatar_url: None,
            },
            web_url: "url".to_string(),
            labels,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: updated_at_str.to_string(),
        }
    }

    fn create_project() -> GitlabProject {
        GitlabProject {
            id: PROJECT_ID,
            path_with_namespace: "org/repo1".to_string(),
            web_url: "http://gitlab.com/org/repo1".to_string(),
        }
    }

    #[tokio::test]
    async fn test_stale_issue_fetches_notes_baseline() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

        // Issue is stale based on updated_at (35 days old > 30 days threshold)
        let old_update = (Utc::now() - ChronoDuration::days(35)).to_rfc3339();
        let issue1 = create_issue(1, &old_update, vec![], "opened");

        // In the unoptimized code, this SHOULD be called.
        let m_notes = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()),
            )
            .with_status(200)
            .with_body(json!([]).to_string())
            .expect(0) // Expect 0 calls after optimization (issue date is stale enough)
            .create_async()
            .await;

        // We expect the label to be added since it's stale
        let _m_add_label = server
            .mock("PUT", "/api/v4/projects/1/issues/1")
            .with_status(200)
            .match_body(Matcher::JsonString(
                json!({"add_labels": STALE_LABEL}).to_string(),
            ))
            .create_async()
            .await;

        check_stale_issues(PROJECT_ID, client, config, &[issue1])
            .await
            .unwrap();

        m_notes.assert_async().await;
    }

    #[tokio::test]
    async fn test_poll_repository_optimizes_stale_fetch() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config(30, TEST_BOT_USERNAME, server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let file_indexer = Arc::new(FileIndexManager::new(client.clone(), 3600));
        let polling_service = PollingService::new(client.clone(), config.clone(), file_indexer, None);

        // 1. Mock get_project_by_path
        let _m_project = server
            .mock("GET", "/api/v4/projects/org%2Frepo1")
            .with_status(200)
            .with_body(json!(create_project()).to_string())
            .create_async()
            .await;

        // 2. Mock get_issues (recent issues for mentions/triage)
        // This should match updated_after=20... (current year)
        // The service logic uses min(since, max_age) so if we pass 'now', it uses 'now - max_age' (default 24h?)
        // So updated_after will be 24h ago, which is recent (202X).
        let _m_recent_issues = server
            .mock("GET", Matcher::Regex(r"^/api/v4/projects/1/issues\?.*updated_after=2.*".to_string()))
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;

        // 3. Mock get_opened_issues (stale check) - THIS IS THE OPTIMIZATION
        // Must match updated_after=1970... AND state=opened
        let m_stale_fetch = server
            .mock("GET", Matcher::Regex(r"^/api/v4/projects/1/issues.*updated_after=1970.*state=opened.*".to_string()))
            .with_status(200)
            .with_body("[]")
            .expect(1)
            .create_async()
            .await;

        // 4. Mock get_merge_requests
        let _m_mrs = server
            .mock("GET", Matcher::Regex(r"^/api/v4/projects/1/merge_requests\?.*".to_string()))
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;

        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        polling_service.poll_repository("org/repo1", now).await.unwrap();

        m_stale_fetch.assert_async().await;
    }
}
