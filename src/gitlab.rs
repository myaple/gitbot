use crate::config::AppSettings;
use crate::models::{
    GitlabCommit, GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes, GitlabProject,
};
use crate::repo_context::{GitlabDiff, GitlabFile};
use chrono::{DateTime, TimeZone, Utc};
// GitLab API client implementation using the gitlab crate
use gitlab::{AsyncGitlab, GitlabBuilder};
// Try importing API components we attempted before
use gitlab::api::{Query, AsyncQuery};
use gitlab::api::projects::issues::{Issue, Issues};
use gitlab::api::projects::merge_requests::MergeRequest;
use gitlab::api::projects::Project;
use gitlab::api::projects::issues::notes::CreateIssueNote;
use gitlab::api::projects::merge_requests::notes::CreateMergeRequestNote;
use gitlab::api::{Pagination};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, instrument};
use urlencoding::encode;

#[derive(Error, Debug)]
pub enum GitlabError {
    #[error("GitLab API error: {0}")]
    GitlabApi(#[from] gitlab::GitlabError),
    #[error("URL parsing error: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("API error: {message}")]
    Api { message: String },
}

#[derive(Debug)]
pub struct GitlabApiClient {
    client: AsyncGitlab,
    settings: Arc<AppSettings>,
}

impl GitlabApiClient {
    pub async fn new(settings: Arc<AppSettings>) -> Result<Self, GitlabError> {
        let client = GitlabBuilder::new(&settings.gitlab_url, &settings.gitlab_token)
            .build_async()
            .await
            .map_err(GitlabError::GitlabApi)?;
        
        Ok(Self {
            client,
            settings,
        })
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn get_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
    ) -> Result<GitlabIssue, GitlabError> {
        // Try the most common pattern: direct HTTP request methods
        // Many GitLab clients provide get/post/put/delete methods
        
        let url = format!("/api/v4/projects/{}/issues/{}", project_id, issue_iid);
        
        // Try to see what methods are available
        // This will generate helpful compiler errors showing available methods
        // Use the gitlab crate API pattern with endpoint builders
        let endpoint = Issue::builder()
            .project(project_id as u64)
            .issue(issue_iid as u64)
            .build()
            .map_err(|e| GitlabError::Api { message: format!("Failed to build issue endpoint: {}", e) })?;
            
        let issue: GitlabIssue = endpoint
            .query_async(&self.client)
            .await
            .map_err(|e| GitlabError::Api { message: format!("API error: {}", e) })?;
            
        Ok(issue)
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn get_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
    ) -> Result<GitlabMergeRequest, GitlabError> {
        let endpoint = MergeRequest::builder()
            .project(project_id as u64)
            .merge_request(mr_iid as u64)
            .build()
            .map_err(|e| GitlabError::Api { message: format!("Failed to build merge request endpoint: {}", e) })?;
            
        let mr: GitlabMergeRequest = endpoint
            .query_async(&self.client)
            .await
            .map_err(|e| GitlabError::Api { message: format!("API error: {}", e) })?;
            
        Ok(mr)
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn post_comment_to_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
        comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        let endpoint = CreateIssueNote::builder()
            .project(project_id as u64)
            .issue(issue_iid as u64)
            .body(comment_body)
            .build()
            .map_err(|e| GitlabError::Api { message: format!("Failed to build create issue note endpoint: {}", e) })?;
            
        let note: GitlabNoteAttributes = endpoint
            .query_async(&self.client)
            .await
            .map_err(|e| GitlabError::Api { message: format!("API error: {}", e) })?;
            
        Ok(note)
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn post_comment_to_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
        comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        let endpoint = CreateMergeRequestNote::builder()
            .project(project_id as u64)
            .merge_request(mr_iid as u64)
            .body(comment_body)
            .build()
            .map_err(|e| GitlabError::Api { message: format!("Failed to build create merge request note endpoint: {}", e) })?;
            
        let note: GitlabNoteAttributes = endpoint
            .query_async(&self.client)
            .await
            .map_err(|e| GitlabError::Api { message: format!("API error: {}", e) })?;
            
        Ok(note)
    }

    #[instrument(skip(self), fields(repo_path))]
    pub async fn get_project_by_path(&self, repo_path: &str) -> Result<GitlabProject, GitlabError> {
        let endpoint = Project::builder()
            .project(repo_path)
            .build()
            .map_err(|e| GitlabError::Api { message: format!("Failed to build project endpoint: {}", e) })?;
            
        let project: GitlabProject = endpoint
            .query_async(&self.client)
            .await
            .map_err(|e| GitlabError::Api { message: format!("API error: {}", e) })?;
            
        Ok(project)
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_issues(
        &self,
        project_id: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabIssue>, GitlabError> {
        let dt = DateTime::from_timestamp(since_timestamp as i64, 0).unwrap_or_else(|| {
            Utc.timestamp_opt(0, 0)
                .single()
                .expect("Fallback timestamp failed for 0")
        });
        
        let endpoint = Issues::builder()
            .project(project_id as u64)
            .updated_after(dt)
            .sort(gitlab::api::common::SortOrder::Ascending)
            .build()
            .map_err(|e| GitlabError::Api { message: format!("Failed to build issues endpoint: {}", e) })?;
            
        let issues: Vec<GitlabIssue> = gitlab::api::paged(endpoint, Pagination::All)
            .query_async(&self.client)
            .await
            .map_err(|e| GitlabError::Api { message: format!("API error: {}", e) })?;
            
        Ok(issues)
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_merge_requests(
        &self,
        project_id: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabMergeRequest>, GitlabError> {
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for get_merge_requests({}, {})", project_id, since_timestamp) 
        })
    }

    #[instrument(skip(self), fields(project_id, issue_iid, since_timestamp))]
    pub async fn get_issue_notes(
        &self,
        project_id: i64,
        issue_iid: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for get_issue_notes({}, {}, {})", project_id, issue_iid, since_timestamp) 
        })
    }

    #[instrument(skip(self), fields(project_id, mr_iid, since_timestamp))]
    pub async fn get_merge_request_notes(
        &self,
        project_id: i64,
        mr_iid: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for get_merge_request_notes({}, {}, {})", project_id, mr_iid, since_timestamp) 
        })
    }

    /// Get the repository file tree with pagination
    #[instrument(skip(self), fields(project_id))]
    pub async fn get_repository_tree(&self, project_id: i64) -> Result<Vec<String>, GitlabError> {
        Err(GitlabError::Api { message: "Method needs to be implemented with gitlab crate".to_string() })
    }

    /// Get file content from repository
    #[instrument(skip(self), fields(project_id, file_path))]
    pub async fn get_file_content(
        &self,
        project_id: i64,
        file_path: &str,
    ) -> Result<GitlabFile, GitlabError> {
        Err(GitlabError::Api { message: "Method needs to be implemented with gitlab crate".to_string() })
    }

    /// Search for files by name
    #[instrument(skip(self), fields(project_id, query))]
    pub async fn search_files_by_name(
        &self,
        project_id: i64,
        query: &str,
    ) -> Result<Vec<String>, GitlabError> {
        Err(GitlabError::Api { message: "Method needs to be implemented with gitlab crate".to_string() })
    }

    /// Search for files by content
    #[instrument(skip(self), fields(project_id, query))]
    pub async fn search_files_by_content(
        &self,
        project_id: i64,
        query: &str,
    ) -> Result<Vec<String>, GitlabError> {
        Err(GitlabError::Api { message: "Method needs to be implemented with gitlab crate".to_string() })
    }

    /// Get changes for a merge request
    #[instrument(skip(self), fields(project_id, merge_request_iid))]
    pub async fn get_merge_request_changes(
        &self,
        project_id: i64,
        merge_request_iid: i64,
    ) -> Result<Vec<GitlabDiff>, GitlabError> {
        Err(GitlabError::Api { message: "Method needs to be implemented with gitlab crate".to_string() })
    }

    #[instrument(skip(self), fields(project_id, issue_iid, label_name))]
    pub async fn add_issue_label(
        &self,
        project_id: i64,
        issue_iid: i64,
        label_name: &str,
    ) -> Result<GitlabIssue, GitlabError> {
        Err(GitlabError::Api { message: "Method needs to be implemented with gitlab crate".to_string() })
    }

    #[instrument(skip(self), fields(project_id, issue_iid, label_name))]
    pub async fn remove_issue_label(
        &self,
        project_id: i64,
        issue_iid: i64,
        label_name: &str,
    ) -> Result<GitlabIssue, GitlabError> {
        Err(GitlabError::Api { message: "Method needs to be implemented with gitlab crate".to_string() })
    }

    /// Get commit history for a file
    #[instrument(skip(self), fields(project_id, file_path))]
    pub async fn get_file_commits(
        &self,
        project_id: i64,
        file_path: &str,
        limit: Option<usize>,
    ) -> Result<Vec<GitlabCommit>, GitlabError> {
        Err(GitlabError::Api { message: "Method needs to be implemented with gitlab crate".to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use std::sync::Arc;

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
    async fn test_gitlab_crate_api_exploration() {
        // Create a gitlab client to explore available methods
        let result = GitlabBuilder::new("http://localhost:1234", "test_token")
            .build();
            
        if let Ok(client) = result {
            // Try to understand what methods are available
            // Let's see what happens when we try common method names
            
            // Attempt 1: Check if there are direct method calls
            // client.issues() // This will show us if issues() exists
            // client.projects() // This will show us if projects() exists
            // client.get() // This will show us if get() exists
            
            // Attempt 2: Check if it implements common traits
            // let _: &dyn Query = &client; // This will tell us about Query trait
            
            println!("GitLab client type: {:?}", std::any::type_name_of_val(&client));
            
            // For now, just ensure we can create the client
            assert!(true, "GitLab client created successfully");
        } else {
            panic!("Failed to create GitLab client");
        }
    }

    #[tokio::test]
    async fn test_new_gitlab_api_client_valid_url() {
        let settings = Arc::new(create_test_settings("http://localhost:1234".to_string()));
        let client = GitlabApiClient::new(settings).await;
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_new_gitlab_api_client_invalid_url() {
        let settings = Arc::new(create_test_settings("not a url".to_string()));
        let result = GitlabApiClient::new(settings).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabApi(_) => {} // Expected error from gitlab crate
            _ => panic!("Expected GitlabApi error"),
        }
    }

    #[tokio::test]
    async fn test_post_comment_to_merge_request_compiles() {
        // Just test that our method compiles and can be called
        // We don't need a real GitLab server for this
        let settings = Arc::new(create_test_settings("http://localhost:1234".to_string()));
        let client = GitlabApiClient::new(settings).await;
        assert!(client.is_ok());
        
        // We can't actually test the API call without a server, but we can verify the method exists
        // This test confirms the method signature and basic structure are correct
        let client = client.unwrap();
        
        // This would fail if the API method doesn't exist or has wrong signature
        let _result = client.post_comment_to_merge_request(1, 1, "test comment").await;
        // We expect this to fail with an API error since we don't have a real server
        // But the important thing is that the method exists and compiles
    }
}
