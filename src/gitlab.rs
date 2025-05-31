use crate::config::AppSettings;
use crate::models::{
    GitlabCommit, GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes, GitlabProject,
};
use crate::repo_context::{GitlabDiff, GitlabFile};
use chrono::{DateTime, TimeZone, Utc};
// GitLab API client implementation using the gitlab crate
use gitlab::{Gitlab, GitlabBuilder};
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
    client: Gitlab,
    settings: Arc<AppSettings>,
}

impl GitlabApiClient {
    pub fn new(settings: Arc<AppSettings>) -> Result<Self, GitlabError> {
        let client = GitlabBuilder::new(&settings.gitlab_url, &settings.gitlab_token)
            .insecure()  // Add if needed for testing
            .build()
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
        // Try using the gitlab crate in a simpler way first
        // For now, return a temporary implementation error
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for get_issue({}, {})", project_id, issue_iid) 
        })
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn get_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
    ) -> Result<GitlabMergeRequest, GitlabError> {
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for get_merge_request({}, {})", project_id, mr_iid) 
        })
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn post_comment_to_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
        comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for post_comment_to_issue({}, {}, len={})", project_id, issue_iid, comment_body.len()) 
        })
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn post_comment_to_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
        comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for post_comment_to_merge_request({}, {}, len={})", project_id, mr_iid, comment_body.len()) 
        })
    }

    #[instrument(skip(self), fields(repo_path))]
    pub async fn get_project_by_path(&self, repo_path: &str) -> Result<GitlabProject, GitlabError> {
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for get_project_by_path({})", repo_path) 
        })
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_issues(
        &self,
        project_id: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabIssue>, GitlabError> {
        Err(GitlabError::Api { 
            message: format!("GitLab crate integration in progress for get_issues({}, {})", project_id, since_timestamp) 
        })
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
    async fn test_gitlab_crate_exploration() {
        // Simple test to understand what the gitlab crate provides
        let result = GitlabBuilder::new("http://localhost:1234", "test_token")
            .build();
            
        match result {
            Ok(_client) => {
                // GitLab client created successfully
                println!("GitLab crate client created successfully");
            },
            Err(e) => {
                println!("Failed to create gitlab client: {:?}", e);
            }
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
            GitlabError::GitlabApi(_) => {} // Expected error from gitlab crate
            _ => panic!("Expected GitlabApi error"),
        }
    }
}
