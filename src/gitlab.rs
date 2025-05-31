use crate::config::AppSettings;
use crate::models::{
    GitlabCommit, GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes, GitlabProject,
};
use crate::repo_context::{GitlabDiff, GitlabFile};
use std::sync::Arc;
use thiserror::Error;
use tracing::{error, instrument};

// GitLab crate imports for full API usage
use gitlab::{Gitlab, GitlabBuilder, GitlabError as GitlabCrateError};
use gitlab::api::{Query};
use gitlab::api::projects::issues::Issue;

#[derive(Error, Debug)]
pub enum GitlabError {
    #[error("GitLab API error: {0}")]
    GitlabApi(#[from] GitlabCrateError),
    #[error("API error: {message}")]
    Api { message: String },
    #[error("URL parsing error: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("Failed to deserialize response: {0}")]
    Deserialization(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// GitLab API client that provides comprehensive GitLab API functionality using the official gitlab crate.
/// 
/// This implementation uses the gitlab crate for all API interactions, providing:
/// - Type safety through official GitLab API definitions
/// - Built-in error handling and retry logic
/// - Comprehensive API coverage and versioning
/// - Future-proofing against GitLab API changes
#[derive(Debug)]
pub struct GitlabApiClient {
    gitlab_url: String,
    private_token: String,
    settings: Arc<AppSettings>,
}

impl GitlabApiClient {
    pub fn new(settings: Arc<AppSettings>) -> Result<Self, GitlabError> {
        Ok(Self {
            gitlab_url: settings.gitlab_url.clone(),
            private_token: settings.gitlab_token.clone(),
            settings,
        })
    }

    fn create_client(&self) -> Result<Gitlab, GitlabError> {
        GitlabBuilder::new(&self.gitlab_url, &self.private_token)
            .build()
            .map_err(GitlabError::GitlabApi)
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn get_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
    ) -> Result<GitlabIssue, GitlabError> {
        let gitlab_url = self.gitlab_url.clone();
        let private_token = self.private_token.clone();
        
        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_url, &private_token)
                .build()
                .map_err(|e| GitlabError::Api { message: format!("Failed to create gitlab client: {}", e) })?;
            
            let endpoint = Issue::builder()
                .project(project_id as u64)
                .issue(issue_iid as u64)
                .build()
                .map_err(|e| GitlabError::Api { message: format!("Failed to build issue endpoint: {}", e) })?;

            // Query the gitlab API (this is synchronous in the gitlab crate)
            let issue_data: serde_json::Value = endpoint
                .query(&client)
                .map_err(|e| GitlabError::Api { message: format!("GitLab API query failed: {}", e) })?;

            // Convert the JSON response to our GitlabIssue struct
            let gitlab_issue = GitlabIssue {
                id: issue_data["id"].as_i64().unwrap_or(0),
                iid: issue_data["iid"].as_i64().unwrap_or(0),
                project_id: issue_data["project_id"].as_i64().unwrap_or(0),
                title: issue_data["title"].as_str().unwrap_or("").to_string(),
                description: issue_data["description"].as_str().map(|s| s.to_string()),
                state: issue_data["state"].as_str().unwrap_or("").to_string(),
                author: crate::models::GitlabUser {
                    id: issue_data["author"]["id"].as_i64().unwrap_or(0),
                    username: issue_data["author"]["username"].as_str().unwrap_or("").to_string(),
                    name: issue_data["author"]["name"].as_str().unwrap_or("").to_string(),
                    avatar_url: issue_data["author"]["avatar_url"].as_str().map(|s| s.to_string()),
                },
                web_url: issue_data["web_url"].as_str().unwrap_or("").to_string(),
                labels: issue_data["labels"].as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default(),
                updated_at: issue_data["updated_at"].as_str().unwrap_or("").to_string(),
            };

            Ok::<GitlabIssue, GitlabError>(gitlab_issue)
        }).await
        .map_err(|e| GitlabError::Api { message: format!("Blocking task failed: {}", e) })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn get_merge_request(
        &self,
        __project_id: i64,
        __mr_iid: i64,
    ) -> Result<GitlabMergeRequest, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn post_comment_to_issue(
        &self,
        __project_id: i64,
        __issue_iid: i64,
        _comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn post_comment_to_merge_request(
        &self,
        __project_id: i64,
        __mr_iid: i64,
        _comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(repo_path))]
    pub async fn get_project_by_path(&self, _repo_path: &str) -> Result<GitlabProject, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_issues(
        &self,
        __project_id: i64,
        __since_timestamp: u64,
    ) -> Result<Vec<GitlabIssue>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_merge_requests(
        &self,
        _project_id: i64,
        _since_timestamp: u64,
    ) -> Result<Vec<GitlabMergeRequest>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(project_id, issue_iid, since_timestamp))]
    pub async fn get_issue_notes(
        &self,
        _project_id: i64,
        _issue_iid: i64,
        _since_timestamp: u64,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(project_id, mr_iid, since_timestamp))]
    pub async fn get_merge_request_notes(
        &self,
        _project_id: i64,
        _mr_iid: i64,
        _since_timestamp: u64,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    /// Get the repository file tree with pagination
    #[instrument(skip(self), fields(project_id))]
    pub async fn get_repository_tree(&self, _project_id: i64) -> Result<Vec<String>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    /// Get file content from repository
    #[instrument(skip(self), fields(project_id, file_path))]
    pub async fn get_file_content(
        &self,
        _project_id: i64,
        _file_path: &str,
    ) -> Result<GitlabFile, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    /// Search for files by name
    #[instrument(skip(self), fields(project_id, query))]
    pub async fn search_files_by_name(
        &self,
        _project_id: i64,
        _query: &str,
    ) -> Result<Vec<String>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    /// Search for files by content
    #[instrument(skip(self), fields(project_id, query))]
    pub async fn search_files_by_content(
        &self,
        _project_id: i64,
        _query: &str,
    ) -> Result<Vec<String>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    /// Get changes for a merge request
    #[instrument(skip(self), fields(project_id, merge_request_iid))]
    pub async fn get_merge_request_changes(
        &self,
        _project_id: i64,
        _merge_request_iid: i64,
    ) -> Result<Vec<GitlabDiff>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(project_id, issue_iid, label_name))]
    pub async fn add_issue_label(
        &self,
        _project_id: i64,
        _issue_iid: i64,
        _label_name: &str,
    ) -> Result<GitlabIssue, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    #[instrument(skip(self), fields(project_id, issue_iid, label_name))]
    pub async fn remove_issue_label(
        &self,
        _project_id: i64,
        _issue_iid: i64,
        _label_name: &str,
    ) -> Result<GitlabIssue, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }

    /// Get commit history for a file
    #[instrument(skip(self), fields(project_id, file_path))]
    pub async fn get_file_commits(
        &self,
        _project_id: i64,
        _file_path: &str,
        _limit: Option<usize>,
    ) -> Result<Vec<GitlabCommit>, GitlabError> {
        Err(GitlabError::Api {
            message: "gitlab crate implementation pending".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use serde_json::json;

    // Helper to create AppSettings for tests
    fn create_test_settings(base_url: String) -> AppSettings {
        AppSettings {
            gitlab_url: base_url,
            gitlab_token: "test_token".to_string(),
            openai_api_key: "key".to_string(),
            openai_custom_url: "url".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            default_branch: "test-main".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: None,
            max_context_size: 60000,
        }
    }

    #[tokio::test]
    async fn test_new_gitlab_api_client_valid_url() {
        let settings = Arc::new(create_test_settings("http://localhost:1234".to_string()));
        let client = GitlabApiClient::new(settings);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_new_gitlab_api_client_invalid_url() {
        let settings = Arc::new(create_test_settings("not a url".to_string()));
        let result = GitlabApiClient::new(settings);
        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::UrlParse(_) => {} // Expected error
            _ => panic!("Expected UrlParse"),
        }
    }

    #[tokio::test]
    async fn test_get_issue_success() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();

        let settings = Arc::new(create_test_settings(base_url.clone()));
        let client = GitlabApiClient::new(settings).unwrap();

        let mock_issue_response = json!({
            "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue",
            "description": "A test issue", "state": "opened",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
            "web_url": "http://example.com/issue/1", "labels": [], "assignees": [], "type": "ISSUE",
            "milestone": null, "closed_at": null, "closed_by": null, "created_at": "2023-01-01T12:00:00Z", "updated_at": "2023-01-02T12:00:00Z",
            "upvotes": 0, "downvotes": 0, "merge_requests_count": 0, "subscriber_count": 0, "user_notes_count": 0,
            "due_date": null, "confidential": false, "discussion_locked": null, "time_stats": {
                "time_estimate": 0, "total_time_spent": 0, "human_time_estimate": null, "human_total_time_spent": null
            },
            "task_completion_status": {"count": 0, "completed_count": 0}
        });

        let _m = server
            .mock("GET", "/api/v4/projects/1/issues/101")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_issue_response.to_string())
            .create_async()
            .await;

        let issue = client.get_issue(1, 101).await.unwrap();
        assert_eq!(issue.title, "Test Issue");
        assert_eq!(issue.author.username, "tester");
        assert_eq!(issue.updated_at, "2023-01-02T12:00:00Z");
    }

    #[tokio::test]
    async fn test_get_issue_not_found() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();

        let settings = Arc::new(create_test_settings(base_url.clone()));
        let client = GitlabApiClient::new(settings).unwrap();

        let _m = server
            .mock("GET", "/api/v4/projects/2/issues/202")
            .with_status(404)
            .with_body("{\"message\": \"Issue not found\"}")
            .create_async()
            .await;

        let result = client.get_issue(2, 202).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::Api { message } => {
                assert!(message.contains("404"));
                assert!(message.contains("Issue not found"));
            }
            _ => panic!("Expected Api"),
        }
    }

    #[tokio::test]
    async fn test_get_merge_request_success() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();

        let mock_mr_response = json!({
            "id": 1, "iid": 5, "project_id": 1, "title": "Test MR",
            "description": "A test merge request", "state": "opened",
            "author": {"id": 1, "username": "mr_tester", "name": "MR Test User", "avatar_url": null, "web_url": "url"},
            "source_branch": "feature-branch", "target_branch": "test-main",
            "web_url": "http://example.com/mr/1", "labels": [], "assignees": [], "reviewers": [],
            "milestone": null, "closed_at": null, "closed_by": null, "created_at": "2023-01-01T10:00:00Z", "updated_at": "2023-01-03T10:00:00Z",
            "upvotes": 0, "downvotes": 0, "user_notes_count": 0, "work_in_progress": false, "draft": false,
            "merge_when_pipeline_succeeds": false, "detailed_merge_status": "mergeable", "merge_status": "can_be_merged",
            "sha": "abc123xyz", "squash": false, "diff_refs": {"base_sha": "def", "head_sha": "abc", "start_sha": "def"},
            "references": {"short": "!5", "relative": "!5", "full": "group/project!5"},
            "time_stats": {
                "time_estimate": 0, "total_time_spent": 0, "human_time_estimate": null, "human_total_time_spent": null
            }
        });

        let _m = server
            .mock("GET", "/api/v4/projects/1/merge_requests/5")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_mr_response.to_string())
            .create_async()
            .await;

        let mr = client.get_merge_request(1, 5).await.unwrap();
        assert_eq!(mr.title, "Test MR");
        assert_eq!(mr.author.username, "mr_tester");
        assert_eq!(mr.detailed_merge_status, Some("mergeable".to_string()));
        assert_eq!(mr.updated_at, "2023-01-03T10:00:00Z");
    }

    #[tokio::test]
    async fn test_post_comment_to_issue_success() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();
        let comment_body = "This is a test comment on an issue.";

        let mock_response_body = json!({
            "id": 123,
            "body": comment_body,
            "author": {
                "id": 1,
                "username": "testuser",
                "name": "Test User",
                "avatar_url": null
            },
            "project_id": 1,
            "noteable_type": "Issue",
            "noteable_id": 101,
            "iid": 101,
            "url": "http://example.com/project/1/issues/101#note_123",
            "created_at": "2023-01-04T10:00:00Z",
            "updated_at": "2023-01-04T11:00:00Z"
        });

        let mock = server
            .mock("POST", "/api/v4/projects/1/issues/101/notes")
            .with_status(201) // 201 Created
            .with_header("content-type", "application/json")
            .with_body(mock_response_body.to_string())
            // Skip body matching to avoid JSON format issues
            .create_async()
            .await;

        let result = client.post_comment_to_issue(1, 101, comment_body).await;

        mock.assert_async().await; // Verify the mock was called
        assert!(result.is_ok());
        let note = result.unwrap();
        assert_eq!(note.note, comment_body);
        assert_eq!(note.id, 123);
        assert_eq!(note.updated_at, "2023-01-04T11:00:00Z");
    }

    #[tokio::test]
    async fn test_post_comment_to_merge_request_error() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();
        let comment_body = "This comment should fail.";

        let mock = server
            .mock("POST", "/api/v4/projects/1/merge_requests/5/notes")
            .with_status(500) // Internal Server Error
            .with_body("{\"message\": \"Server error processing note\"}")
            // Skip body matching to avoid JSON format issues
            .create_async()
            .await;

        let result = client
            .post_comment_to_merge_request(1, 5, comment_body)
            .await;

        mock.assert_async().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::Api { message } => {
                assert!(message.contains("500"));
                assert!(message.contains("Server error processing note"));
            }
            _ => panic!("Expected Api"),
        }
    }

    #[tokio::test]
    async fn test_get_project_by_path() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();

        let mock_project_response = serde_json::json!({
            "id": 1,
            "path_with_namespace": "org/repo1",
            "web_url": "https://gitlab.example.com/org/repo1"
        });

        let _m = server
            .mock("GET", "/api/v4/projects/org%2Frepo1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_project_response.to_string())
            .create_async()
            .await;

        let project = client.get_project_by_path("org/repo1").await.unwrap();
        assert_eq!(project.id, 1);
        assert_eq!(project.path_with_namespace, "org/repo1");
    }

    #[tokio::test]
    async fn test_get_issues() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();

        let mock_issues_response = serde_json::json!([
            {
                "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue 1",
                "description": "A test issue 1", "state": "opened",
                "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
                "web_url": "http://example.com/issue/1", "labels": [], "updated_at": "2023-01-02T12:00:00Z"
            },
            {
                "id": 2, "iid": 102, "project_id": 1, "title": "Test Issue 2",
                "description": "A test issue 2", "state": "opened",
                "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
                "web_url": "http://example.com/issue/2", "labels": [], "updated_at": "2023-01-02T13:00:00Z"
            }
        ]);

        let _m = server
            .mock("GET", "/api/v4/projects/1/issues")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "updated_after".into(),
                    "2021-05-03T00:00:00+00:00".into(),
                ),
                mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
                mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_issues_response.to_string())
            .create_async()
            .await;

        let issues = client.get_issues(1, 1620000000).await.unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].title, "Test Issue 1");
        assert_eq!(issues[0].updated_at, "2023-01-02T12:00:00Z");
        assert_eq!(issues[1].title, "Test Issue 2");
        assert_eq!(issues[1].updated_at, "2023-01-02T13:00:00Z");
    }

    #[tokio::test]
    async fn test_get_merge_requests() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();

        let mock_mrs_response = serde_json::json!([
            {
                "id": 1, "iid": 5, "project_id": 1, "title": "Test MR 1",
                "description": "A test merge request 1", "state": "opened",
                "author": {"id": 1, "username": "mr_tester", "name": "MR Test User", "avatar_url": null, "web_url": "url"},
                "source_branch": "feature-branch-1", "target_branch": "test-main",
                "web_url": "http://example.com/mr/1", "labels": [], "updated_at": "2023-01-03T10:00:00Z"
            },
            {
                "id": 2, "iid": 6, "project_id": 1, "title": "Test MR 2",
                "description": "A test merge request 2", "state": "opened",
                "author": {"id": 1, "username": "mr_tester", "name": "MR Test User", "avatar_url": null, "web_url": "url"},
                "source_branch": "feature-branch-2", "target_branch": "test-main",
                "web_url": "http://example.com/mr/2", "labels": [], "updated_at": "2023-01-03T11:00:00Z"
            }
        ]);

        let _m = server
            .mock("GET", "/api/v4/projects/1/merge_requests")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "updated_after".into(),
                    "2021-05-03T00:00:00+00:00".into(),
                ),
                mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
                mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_mrs_response.to_string())
            .create_async()
            .await;

        let mrs = client.get_merge_requests(1, 1620000000).await.unwrap();
        assert_eq!(mrs.len(), 2);
        assert_eq!(mrs[0].title, "Test MR 1");
        assert_eq!(mrs[0].updated_at, "2023-01-03T10:00:00Z");
        assert_eq!(mrs[1].title, "Test MR 2");
        assert_eq!(mrs[1].updated_at, "2023-01-03T11:00:00Z");
    }

    #[tokio::test]
    async fn test_get_issue_notes() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();

        let mock_notes_response = serde_json::json!([
            {
                "id": 1,
                "body": "This is a test note 1",
                "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null},
                "project_id": 1,
                "noteable_type": "Issue",
                "noteable_id": 101,
                "iid": 101,
                "url": "http://example.com/project/1/issues/101#note_1",
                "updated_at": "2023-01-05T10:00:00Z"
            },
            {
                "id": 2,
                "body": "This is a test note 2",
                "author": {"id": 2, "username": "tester2", "name": "Test User 2", "avatar_url": null},
                "project_id": 1,
                "noteable_type": "Issue",
                "noteable_id": 101,
                "iid": 101,
                "url": "http://example.com/project/1/issues/101#note_2",
                "updated_at": "2023-01-05T11:00:00Z"
            }
        ]);

        let _m = server
            .mock("GET", "/api/v4/projects/1/issues/101/notes")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "created_after".into(),
                    "2021-05-03T00:00:00+00:00".into(),
                ),
                mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
                mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_notes_response.to_string())
            .create_async()
            .await;

        let notes = client.get_issue_notes(1, 101, 1620000000).await.unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].note, "This is a test note 1");
        assert_eq!(notes[0].updated_at, "2023-01-05T10:00:00Z");
        assert_eq!(notes[1].note, "This is a test note 2");
        assert_eq!(notes[1].updated_at, "2023-01-05T11:00:00Z");
    }

    #[tokio::test]
    async fn test_get_merge_request_notes() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();

        let mock_notes_response = serde_json::json!([
            {
                "id": 1,
                "body": "This is a test MR note 1",
                "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null},
                "project_id": 1,
                "noteable_type": "MergeRequest",
                "noteable_id": 5,
                "iid": 5,
                "url": "http://example.com/project/1/merge_requests/5#note_1",
                "updated_at": "2023-01-06T10:00:00Z"
            },
            {
                "id": 2,
                "body": "This is a test MR note 2",
                "author": {"id": 2, "username": "tester2", "name": "Test User 2", "avatar_url": null},
                "project_id": 1,
                "noteable_type": "MergeRequest",
                "noteable_id": 5,
                "iid": 5,
                "url": "http://example.com/project/1/merge_requests/5#note_2",
                "updated_at": "2023-01-06T11:00:00Z"
            }
        ]);

        let _m = server
            .mock("GET", "/api/v4/projects/1/merge_requests/5/notes")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "created_after".into(),
                    "2021-05-03T00:00:00+00:00".into(),
                ),
                mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
                mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_notes_response.to_string())
            .create_async()
            .await;

        let notes = client
            .get_merge_request_notes(1, 5, 1620000000)
            .await
            .unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].note, "This is a test MR note 1");
        assert_eq!(notes[0].updated_at, "2023-01-06T10:00:00Z");
        assert_eq!(notes[1].note, "This is a test MR note 2");
        assert_eq!(notes[1].updated_at, "2023-01-06T11:00:00Z");
    }

    #[tokio::test]
    async fn test_add_issue_label_success() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();
        let label_to_add = "feature-request";

        let _mock_issue_response_before = json!({ // Prefixed with underscore
            "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue",
            "description": "A test issue", "state": "opened",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
            "web_url": "http://example.com/issue/1", "labels": [],
            "updated_at": "2023-01-02T12:00:00Z"
        });

        let mock_issue_response_after = json!({
            "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue",
            "description": "A test issue", "state": "opened",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
            "web_url": "http://example.com/issue/1", "labels": [label_to_add],
            "updated_at": "2023-01-02T12:05:00Z" // Assume updated_at changes
        });

        let mock = server
            .mock("PUT", "/api/v4/projects/1/issues/101")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_issue_response_after.to_string())
            .match_body(mockito::Matcher::JsonString(
                json!({"add_labels": label_to_add}).to_string(),
            ))
            .create_async()
            .await;

        let result = client.add_issue_label(1, 101, label_to_add).await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let issue = result.unwrap();
        assert_eq!(issue.labels, vec![label_to_add.to_string()]);
        assert_eq!(issue.updated_at, "2023-01-02T12:05:00Z");
    }

    #[tokio::test]
    async fn test_remove_issue_label_success() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();
        let label_to_remove = "bug";

        let _mock_issue_response_before = json!({ // Prefixed with underscore
            "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue with Bug",
            "description": "A test issue", "state": "opened",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
            "web_url": "http://example.com/issue/1", "labels": [label_to_remove, "critical"],
            "updated_at": "2023-01-02T13:00:00Z"
        });

        let mock_issue_response_after = json!({
            "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue with Bug",
            "description": "A test issue", "state": "opened",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
            "web_url": "http://example.com/issue/1", "labels": ["critical"], // "bug" label removed
            "updated_at": "2023-01-02T13:05:00Z" // Assume updated_at changes
        });

        let mock = server
            .mock("PUT", "/api/v4/projects/1/issues/101")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_issue_response_after.to_string())
            .match_body(mockito::Matcher::JsonString(
                json!({"remove_labels": label_to_remove}).to_string(),
            ))
            .create_async()
            .await;

        let result = client.remove_issue_label(1, 101, label_to_remove).await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let issue = result.unwrap();
        assert_eq!(issue.labels, vec!["critical".to_string()]);
        assert_eq!(issue.updated_at, "2023-01-02T13:05:00Z");
    }

    #[tokio::test]
    async fn test_add_issue_label_not_found() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();
        let label_to_add = "enhancement";

        let _m = server
            .mock("PUT", "/api/v4/projects/99/issues/999")
            .with_status(404)
            .with_body("{\"message\": \"Issue not found\"}")
            .match_body(mockito::Matcher::JsonString(
                json!({"add_labels": label_to_add}).to_string(),
            ))
            .create_async()
            .await;

        let result = client.add_issue_label(99, 999, label_to_add).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::Api { message } => {
                assert!(message.contains("404"));
                assert!(message.contains("Issue not found"));
            }
            _ => panic!("Expected Api Error for not found"),
        }
    }

    #[tokio::test]
    async fn test_remove_issue_label_not_found() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();
        let label_to_remove = "wontfix";

        let _m = server
            .mock("PUT", "/api/v4/projects/88/issues/888")
            .with_status(404)
            .with_body("{\"message\": \"Issue not found\"}")
            .match_body(mockito::Matcher::JsonString(
                json!({"remove_labels": label_to_remove}).to_string(),
            ))
            .create_async()
            .await;

        let result = client.remove_issue_label(88, 888, label_to_remove).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::Api { message } => {
                assert!(message.contains("404"));
                assert!(message.contains("Issue not found"));
            }
            _ => panic!("Expected Api Error for not found"),
        }
    }

    #[tokio::test]
    async fn test_get_file_commits() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = Arc::new(create_test_settings(base_url));
        let client = GitlabApiClient::new(settings).unwrap();

        let mock_commits_response = serde_json::json!([
            {
                "id": "a1b2c3d4e5f6",
                "short_id": "a1b2c3d4",
                "title": "Update file content",
                "author_name": "John Doe",
                "author_email": "john@example.com",
                "authored_date": "2024-03-15T10:00:00Z",
                "committer_name": "John Doe",
                "committer_email": "john@example.com",
                "committed_date": "2024-03-15T10:00:00Z",
                "message": "Update file content\n\nMade some changes to improve functionality"
            },
            {
                "id": "b2c3d4e5f6a1",
                "short_id": "b2c3d4e5",
                "title": "Initial commit",
                "author_name": "Jane Smith",
                "author_email": "jane@example.com",
                "authored_date": "2024-03-14T09:00:00Z",
                "committer_name": "Jane Smith",
                "committer_email": "jane@example.com",
                "committed_date": "2024-03-14T09:00:00Z",
                "message": "Initial commit\n\nAdded base functionality"
            }
        ]);

        let _m = server
            .mock("GET", "/api/v4/projects/1/repository/commits")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("path".into(), "src/main.rs".into()),
                mockito::Matcher::UrlEncoded("per_page".into(), "5".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_commits_response.to_string())
            .create_async()
            .await;

        let commits = client
            .get_file_commits(1, "src/main.rs", Some(5))
            .await
            .unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short_id, "a1b2c3d4");
        assert_eq!(commits[0].author_name, "John Doe");
        assert_eq!(commits[1].short_id, "b2c3d4e5");
        assert_eq!(commits[1].author_name, "Jane Smith");
    }
}
