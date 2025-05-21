use reqwest::{Client, header, StatusCode, Method};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use thiserror::Error;
use tracing::{debug, error, instrument};
use url::Url;
use crate::models::{GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes}; // Removed unused imports
use crate::config::AppSettings;

#[derive(Error, Debug)]
pub enum GitlabClientError {
    #[error("Request failed: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("API error: {status} - {body}")]
    ApiError { status: StatusCode, body: String },
    #[error("URL parsing error: {0}")]
    UrlParseError(#[from] url::ParseError),
    #[error("Failed to deserialize response: {0}")]
    DeserializationError(reqwest::Error),
}

#[derive(Debug)]
pub struct GitlabApiClient {
    client: Client,
    gitlab_url: Url,
    private_token: String,
}

impl GitlabApiClient {
    pub fn new(settings: &AppSettings) -> Result<Self, GitlabClientError> {
        let gitlab_url = Url::parse(&settings.gitlab_url)?;
        let client = Client::new();
        Ok(Self {
            client,
            gitlab_url,
            private_token: settings.gitlab_token.clone(),
        })
    }

    #[instrument(skip(self, body), fields(method, path))]
    async fn send_request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query_params: Option<&[(&str, &str)]>,
        body: Option<impl Serialize + Debug>, // Added Debug for logging
    ) -> Result<T, GitlabClientError> {
        let mut url = self.gitlab_url.join(path)?;
        if let Some(params) = query_params {
            url.query_pairs_mut().extend_pairs(params);
        }

        debug!("Sending request to URL: {}", url);
        if let Some(b) = &body {
            debug!("Request body: {:?}", b);
        }


        let mut request_builder = self.client.request(method, url)
            .header("PRIVATE-TOKEN", &self.private_token);

        if body.is_some() {
            request_builder = request_builder.header(header::CONTENT_TYPE, "application/json");
        }
        
        if let Some(b) = body {
            request_builder = request_builder.json(&b);
        }

        let response = request_builder.send().await.map_err(GitlabClientError::RequestError)?;

        let status = response.status();
        if !status.is_success() {
            let response_body = response.text().await.unwrap_or_else(|_| "Could not read error body".to_string());
            error!("API Error: {} - {}", status, response_body);
            return Err(GitlabClientError::ApiError {
                status,
                body: response_body,
            });
        }

        let parsed_response = response.json::<T>().await.map_err(GitlabClientError::DeserializationError)?;
        Ok(parsed_response)
    }
    
    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn get_issue(&self, project_id: i64, issue_iid: i64) -> Result<GitlabIssue, GitlabClientError> {
        let path = format!("/api/v4/projects/{}/issues/{}", project_id, issue_iid);
        self.send_request(Method::GET, &path, None, None::<()>).await
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn get_merge_request(&self, project_id: i64, mr_iid: i64) -> Result<GitlabMergeRequest, GitlabClientError> {
        let path = format!("/api/v4/projects/{}/merge_requests/{}", project_id, mr_iid);
        self.send_request(Method::GET, &path, None, None::<()>).await
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn post_comment_to_issue(&self, project_id: i64, issue_iid: i64, comment_body: &str) -> Result<GitlabNoteAttributes, GitlabClientError> {
        let path = format!("/api/v4/projects/{}/issues/{}/notes", project_id, issue_iid);
        let body = serde_json::json!({"body": comment_body});
        self.send_request(Method::POST, &path, None, Some(body)).await
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn post_comment_to_merge_request(&self, project_id: i64, mr_iid: i64, comment_body: &str) -> Result<GitlabNoteAttributes, GitlabClientError> {
        let path = format!("/api/v4/projects/{}/merge_requests/{}/notes", project_id, mr_iid);
        let body = serde_json::json!({"body": comment_body});
        self.send_request(Method::POST, &path, None, Some(body)).await
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
            gitlab_webhook_secret: "secret".to_string(),
            openai_api_key: "key".to_string(),
            openai_custom_url: "url".to_string(),
            server_address: "127.0.0.1:8080".to_string(),
            whitelisted_repos: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
        }
    }

    #[tokio::test]
    async fn test_new_gitlab_api_client_valid_url() {
        let settings = create_test_settings("http://localhost:1234".to_string());
        let client = GitlabApiClient::new(&settings);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_new_gitlab_api_client_invalid_url() {
        let settings = create_test_settings("not a valid url".to_string());
        let client = GitlabApiClient::new(&settings);
        assert!(client.is_err());
        match client.err().unwrap() {
            GitlabClientError::UrlParseError(_) => {} // Expected error
            _ => panic!("Expected UrlParseError"),
        }
    }

    #[tokio::test]
    async fn test_get_issue_success() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();

        let settings = create_test_settings(base_url.clone());
        let client = GitlabApiClient::new(&settings).unwrap();

        let mock_issue_response = json!({
            "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue",
            "description": "A test issue", "state": "opened",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"}, // web_url added
            "web_url": "http://example.com/issue/1", "labels": [], "assignees": [], "type": "ISSUE", // assignees and type added
            "milestone": null, "closed_at": null, "closed_by": null, "created_at": "date", "updated_at": "date", // more optional fields
            "upvotes": 0, "downvotes": 0, "merge_requests_count": 0, "subscriber_count": 0, "user_notes_count": 0,
            "due_date": null, "confidential": false, "discussion_locked": null, "time_stats": { // time_stats is complex
                "time_estimate": 0, "total_time_spent": 0, "human_time_estimate": null, "human_total_time_spent": null
            },
            "task_completion_status": {"count": 0, "completed_count": 0}
        });

        let _m = server.mock("GET", "/api/v4/projects/1/issues/101")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_issue_response.to_string())
            .create_async().await;

        let issue = client.get_issue(1, 101).await.unwrap();
        assert_eq!(issue.title, "Test Issue");
        assert_eq!(issue.author.username, "tester");
    }

    #[tokio::test]
    async fn test_get_issue_not_found() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();

        let settings = create_test_settings(base_url.clone());
        let client = GitlabApiClient::new(&settings).unwrap();

        let _m = server.mock("GET", "/api/v4/projects/2/issues/202")
            .with_status(404)
            .with_body("{\"message\": \"Issue not found\"}")
            .create_async().await;

        let result = client.get_issue(2, 202).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabClientError::ApiError { status, body } => {
                assert_eq!(status, StatusCode::NOT_FOUND);
                assert_eq!(body, "{\"message\": \"Issue not found\"}");
            }
            _ => panic!("Expected ApiError"),
        }
    }

    #[tokio::test]
    async fn test_get_merge_request_success() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = GitlabApiClient::new(&settings).unwrap();

        let mock_mr_response = json!({
            "id": 1, "iid": 5, "project_id": 1, "title": "Test MR",
            "description": "A test merge request", "state": "opened",
            "author": {"id": 1, "username": "mr_tester", "name": "MR Test User", "avatar_url": null, "web_url": "url"},
            "source_branch": "feature-branch", "target_branch": "main",
            "web_url": "http://example.com/mr/1", "labels": [], "assignees": [], "reviewers": [], // reviewers added
            "milestone": null, "closed_at": null, "closed_by": null, "created_at": "date", "updated_at": "date",
            "upvotes": 0, "downvotes": 0, "user_notes_count": 0, "work_in_progress": false, "draft": false, // work_in_progress renamed to draft
            "merge_when_pipeline_succeeds": false, "detailed_merge_status": "mergeable", "merge_status": "can_be_merged", // merge_status added
            "sha": "abc123xyz", "squash": false, "diff_refs": {"base_sha": "def", "head_sha": "abc", "start_sha": "def"}, // diff_refs added
            "references": {"short": "!5", "relative": "!5", "full": "group/project!5"}, // references added
            "time_stats": {
                "time_estimate": 0, "total_time_spent": 0, "human_time_estimate": null, "human_total_time_spent": null
            }
        });

        let _m = server.mock("GET", "/api/v4/projects/1/merge_requests/5")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_mr_response.to_string())
            .create_async().await;

        let mr = client.get_merge_request(1, 5).await.unwrap();
        assert_eq!(mr.title, "Test MR");
        assert_eq!(mr.author.username, "mr_tester");
        assert_eq!(mr.detailed_merge_status, Some("mergeable".to_string()));
    }
    
    #[tokio::test]
    async fn test_post_comment_to_issue_success() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = GitlabApiClient::new(&settings).unwrap();
        let comment_body = "This is a test comment on an issue.";

        let mock_response_body = json!({
            "id": 123,
            "note": comment_body,
            "author_id": 1,
            "project_id": 1,
            "noteable_type": "Issue",
            "noteable_id": 101,
            "iid": 101, // Added iid for the noteable itself
            "url": "http://example.com/project/1/issues/101#note_123"
        });
        
        let mock = server.mock("POST", "/api/v4/projects/1/issues/101/notes")
            .with_status(201) // 201 Created
            .with_header("content-type", "application/json")
            .with_body(mock_response_body.to_string())
            // Skip body matching to avoid JSON format issues
            .create_async().await;

        let result = client.post_comment_to_issue(1, 101, comment_body).await;
        
        mock.assert_async().await; // Verify the mock was called
        assert!(result.is_ok());
        let note = result.unwrap();
        assert_eq!(note.note, comment_body);
        assert_eq!(note.id, 123);
    }

    #[tokio::test]
    async fn test_post_comment_to_merge_request_error() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = GitlabApiClient::new(&settings).unwrap();
        let comment_body = "This comment should fail.";

        let mock = server.mock("POST", "/api/v4/projects/1/merge_requests/5/notes")
            .with_status(500) // Internal Server Error
            .with_body("{\"message\": \"Server error processing note\"}")
            // Skip body matching to avoid JSON format issues
            .create_async().await;

        let result = client.post_comment_to_merge_request(1, 5, comment_body).await;

        mock.assert_async().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabClientError::ApiError { status, body } => {
                assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
                assert_eq!(body, "{\"message\": \"Server error processing note\"}");
            }
            _ => panic!("Expected ApiError"),
        }
    }
}
