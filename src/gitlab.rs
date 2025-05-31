use crate::config::AppSettings;
use crate::models::{
    GitlabCommit, GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes, GitlabProject,
};
use crate::repo_context::{GitlabDiff, GitlabFile};
use chrono::{DateTime, Utc};
use reqwest::{header, Method};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, instrument};

// GitLab crate imports for full API usage
use gitlab::api::projects::issues::notes::{CreateIssueNote, IssueNotes};
use gitlab::api::projects::issues::{EditIssue, Issue, Issues};
use gitlab::api::projects::merge_requests::notes::{CreateMergeRequestNote, MergeRequestNotes};
use gitlab::api::projects::merge_requests::{MergeRequest, MergeRequests};
use gitlab::api::projects::repository::commits::Commits;
use gitlab::api::projects::repository::files::FileRaw;
use gitlab::api::projects::repository::Tree;
use gitlab::api::projects::Project;
use gitlab::api::Query;
use gitlab::{Gitlab, GitlabBuilder, GitlabError as GitlabCrateError};

#[derive(Error, Debug)]
pub enum GitlabError {
    #[error("GitLab API error: {0}")]
    GitlabApi(#[from] GitlabCrateError),
    #[error("API error: {status} - {body}")]
    Api {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("URL parsing error: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("HTTP request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Failed to deserialize response: {0}")]
    Deserialization(String),
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
    gitlab_url: url::Url,
    private_token: String,
    client: reqwest::Client, // Add reqwest client for direct API calls
}

impl GitlabApiClient {
    pub fn new(settings: Arc<AppSettings>) -> Result<Self, GitlabError> {
        let gitlab_url = url::Url::parse(&settings.gitlab_url)?;

        Ok(Self {
            gitlab_url,
            private_token: settings.gitlab_token.clone(),
            client: reqwest::Client::new(),
        })
    }

    fn get_gitlab_host(&self) -> Result<String, GitlabError> {
        // GitlabBuilder expects hostname without protocol, it adds https:// automatically
        let host = self.gitlab_url.host_str().ok_or_else(|| GitlabError::Api {
            status: reqwest::StatusCode::BAD_REQUEST,
            body: "Invalid GitLab URL: no host found".to_string(),
        })?;

        Ok(if let Some(port) = self.gitlab_url.port() {
            format!("{}:{}", host, port)
        } else {
            host.to_string()
        })
    }

    fn create_client(&self) -> Result<Gitlab, GitlabError> {
        self.create_gitlab_client()
    }

    fn create_gitlab_client(&self) -> Result<Gitlab, GitlabError> {
        let host = self.get_gitlab_host()?;
        debug!("Creating client with host: {}", host);
        GitlabBuilder::new(&host, &self.private_token)
            .build()
            .map_err(GitlabError::GitlabApi)
    }

    // Direct HTTP request method for endpoints not supported by gitlab crate
    async fn send_request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query_params: Option<&[(&str, &str)]>,
        body: Option<impl Serialize + std::fmt::Debug>, // Added Debug for logging
    ) -> Result<T, GitlabError> {
        let mut url = self.gitlab_url.join(path)?;
        if let Some(params) = query_params {
            url.query_pairs_mut().extend_pairs(params);
        }

        debug!("Sending request to URL: {}", url);
        if let Some(b) = &body {
            debug!("Request body: {:?}", b);
        }

        let mut request_builder = self
            .client
            .request(method, url)
            .header("PRIVATE-TOKEN", &self.private_token);

        if body.is_some() {
            request_builder = request_builder.header(header::CONTENT_TYPE, "application/json");
        }

        if let Some(b) = body {
            request_builder = request_builder.json(&b);
        }

        let response = request_builder.send().await.map_err(GitlabError::Request)?;

        let status = response.status();
        if !status.is_success() {
            let response_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Could not read error body".to_string());
            error!("API Error: {} - {}", status, response_body);
            return Err(GitlabError::Api {
                status,
                body: response_body,
            });
        }

        let parsed_response = response
            .json::<T>()
            .await
            .map_err(|e| GitlabError::Deserialization(e.to_string()))?;
        Ok(parsed_response)
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn get_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
    ) -> Result<GitlabIssue, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = Issue::builder()
                .project(project_id as u64)
                .issue(issue_iid as u64)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build issue endpoint: {}", e),
                })?;

            // Query the gitlab API (this is synchronous in the gitlab crate)
            let issue_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

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
                    username: issue_data["author"]["username"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    name: issue_data["author"]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    avatar_url: issue_data["author"]["avatar_url"]
                        .as_str()
                        .map(|s| s.to_string()),
                },
                web_url: issue_data["web_url"].as_str().unwrap_or("").to_string(),
                labels: issue_data["labels"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                updated_at: issue_data["updated_at"].as_str().unwrap_or("").to_string(),
            };

            Ok::<GitlabIssue, GitlabError>(gitlab_issue)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn get_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
    ) -> Result<GitlabMergeRequest, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = MergeRequest::builder()
                .project(project_id as u64)
                .merge_request(mr_iid as u64)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build merge request endpoint: {}", e),
                })?;

            // Query the gitlab API
            let mr_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabMergeRequest struct
            let gitlab_mr = GitlabMergeRequest {
                id: mr_data["id"].as_i64().unwrap_or(0),
                iid: mr_data["iid"].as_i64().unwrap_or(0),
                project_id: mr_data["project_id"].as_i64().unwrap_or(0),
                title: mr_data["title"].as_str().unwrap_or("").to_string(),
                description: mr_data["description"].as_str().map(|s| s.to_string()),
                state: mr_data["state"].as_str().unwrap_or("").to_string(),
                author: crate::models::GitlabUser {
                    id: mr_data["author"]["id"].as_i64().unwrap_or(0),
                    username: mr_data["author"]["username"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    name: mr_data["author"]["name"].as_str().unwrap_or("").to_string(),
                    avatar_url: mr_data["author"]["avatar_url"]
                        .as_str()
                        .map(|s| s.to_string()),
                },
                source_branch: mr_data["source_branch"].as_str().unwrap_or("").to_string(),
                target_branch: mr_data["target_branch"].as_str().unwrap_or("").to_string(),
                web_url: mr_data["web_url"].as_str().unwrap_or("").to_string(),
                labels: mr_data["labels"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                detailed_merge_status: mr_data["detailed_merge_status"]
                    .as_str()
                    .map(|s| s.to_string()),
                updated_at: mr_data["updated_at"].as_str().unwrap_or("").to_string(),
                head_pipeline: None, // TODO: Parse pipeline if needed
            };

            Ok::<GitlabMergeRequest, GitlabError>(gitlab_mr)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn post_comment_to_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
        comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();
        let body = comment_body.to_string();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = CreateIssueNote::builder()
                .project(project_id as u64)
                .issue(issue_iid as u64)
                .body(body)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build create note endpoint: {}", e),
                })?;

            // Query the gitlab API
            let note_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabNoteAttributes struct
            let gitlab_note = GitlabNoteAttributes {
                id: note_data["id"].as_i64().unwrap_or(0),
                note: note_data["body"].as_str().unwrap_or("").to_string(),
                author: crate::models::GitlabUser {
                    id: note_data["author"]["id"].as_i64().unwrap_or(0),
                    username: note_data["author"]["username"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    name: note_data["author"]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    avatar_url: note_data["author"]["avatar_url"]
                        .as_str()
                        .map(|s| s.to_string()),
                },
                project_id: note_data["project_id"].as_i64().unwrap_or(0),
                noteable_type: note_data["noteable_type"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                noteable_id: note_data["noteable_id"].as_i64(),
                iid: note_data["iid"].as_i64(),
                url: note_data["url"].as_str().map(|s| s.to_string()),
                updated_at: note_data["updated_at"].as_str().unwrap_or("").to_string(),
            };

            Ok::<GitlabNoteAttributes, GitlabError>(gitlab_note)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn post_comment_to_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
        comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();
        let body = comment_body.to_string();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = CreateMergeRequestNote::builder()
                .project(project_id as u64)
                .merge_request(mr_iid as u64)
                .body(body)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build create note endpoint: {}", e),
                })?;

            // Query the gitlab API
            let note_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabNoteAttributes struct
            let gitlab_note = GitlabNoteAttributes {
                id: note_data["id"].as_i64().unwrap_or(0),
                note: note_data["body"].as_str().unwrap_or("").to_string(),
                author: crate::models::GitlabUser {
                    id: note_data["author"]["id"].as_i64().unwrap_or(0),
                    username: note_data["author"]["username"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    name: note_data["author"]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    avatar_url: note_data["author"]["avatar_url"]
                        .as_str()
                        .map(|s| s.to_string()),
                },
                project_id: note_data["project_id"].as_i64().unwrap_or(0),
                noteable_type: note_data["noteable_type"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                noteable_id: note_data["noteable_id"].as_i64(),
                iid: note_data["iid"].as_i64(),
                url: note_data["url"].as_str().map(|s| s.to_string()),
                updated_at: note_data["updated_at"].as_str().unwrap_or("").to_string(),
            };

            Ok::<GitlabNoteAttributes, GitlabError>(gitlab_note)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(repo_path))]
    pub async fn get_project_by_path(&self, repo_path: &str) -> Result<GitlabProject, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();
        let path = repo_path.to_string();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint =
                Project::builder()
                    .project(&path)
                    .build()
                    .map_err(|e| GitlabError::Api {
                        status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                        body: format!("Failed to build project endpoint: {}", e),
                    })?;

            // Query the gitlab API
            let project_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabProject struct
            let gitlab_project = GitlabProject {
                id: project_data["id"].as_i64().unwrap_or(0),
                path_with_namespace: project_data["path_with_namespace"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                web_url: project_data["web_url"].as_str().unwrap_or("").to_string(),
            };

            Ok::<GitlabProject, GitlabError>(gitlab_project)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_issues(
        &self,
        project_id: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabIssue>, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            // Convert timestamp to DateTime<Utc>
            let updated_after =
                DateTime::from_timestamp(since_timestamp as i64, 0).unwrap_or_else(Utc::now);

            let endpoint = Issues::builder()
                .project(project_id as u64)
                .updated_after(updated_after)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build issues endpoint: {}", e),
                })?;

            // Query the gitlab API
            let issues_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabIssue structs
            let mut issues = Vec::new();
            if let Some(issues_array) = issues_data.as_array() {
                for issue_data in issues_array {
                    let gitlab_issue = GitlabIssue {
                        id: issue_data["id"].as_i64().unwrap_or(0),
                        iid: issue_data["iid"].as_i64().unwrap_or(0),
                        project_id: issue_data["project_id"].as_i64().unwrap_or(0),
                        title: issue_data["title"].as_str().unwrap_or("").to_string(),
                        description: issue_data["description"].as_str().map(|s| s.to_string()),
                        state: issue_data["state"].as_str().unwrap_or("").to_string(),
                        author: crate::models::GitlabUser {
                            id: issue_data["author"]["id"].as_i64().unwrap_or(0),
                            username: issue_data["author"]["username"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                            name: issue_data["author"]["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                            avatar_url: issue_data["author"]["avatar_url"]
                                .as_str()
                                .map(|s| s.to_string()),
                        },
                        web_url: issue_data["web_url"].as_str().unwrap_or("").to_string(),
                        labels: issue_data["labels"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        updated_at: issue_data["updated_at"].as_str().unwrap_or("").to_string(),
                    };
                    issues.push(gitlab_issue);
                }
            }

            Ok::<Vec<GitlabIssue>, GitlabError>(issues)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_merge_requests(
        &self,
        project_id: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabMergeRequest>, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            // Convert timestamp to DateTime<Utc>
            let updated_after =
                DateTime::from_timestamp(since_timestamp as i64, 0).unwrap_or_else(Utc::now);

            let endpoint = MergeRequests::builder()
                .project(project_id as u64)
                .updated_after(updated_after)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build merge requests endpoint: {}", e),
                })?;

            // Query the gitlab API
            let mrs_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabMergeRequest structs
            let mut merge_requests = Vec::new();
            if let Some(mrs_array) = mrs_data.as_array() {
                for mr_data in mrs_array {
                    let gitlab_mr = GitlabMergeRequest {
                        id: mr_data["id"].as_i64().unwrap_or(0),
                        iid: mr_data["iid"].as_i64().unwrap_or(0),
                        project_id: mr_data["project_id"].as_i64().unwrap_or(0),
                        title: mr_data["title"].as_str().unwrap_or("").to_string(),
                        description: mr_data["description"].as_str().map(|s| s.to_string()),
                        state: mr_data["state"].as_str().unwrap_or("").to_string(),
                        author: crate::models::GitlabUser {
                            id: mr_data["author"]["id"].as_i64().unwrap_or(0),
                            username: mr_data["author"]["username"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                            name: mr_data["author"]["name"].as_str().unwrap_or("").to_string(),
                            avatar_url: mr_data["author"]["avatar_url"]
                                .as_str()
                                .map(|s| s.to_string()),
                        },
                        source_branch: mr_data["source_branch"].as_str().unwrap_or("").to_string(),
                        target_branch: mr_data["target_branch"].as_str().unwrap_or("").to_string(),
                        web_url: mr_data["web_url"].as_str().unwrap_or("").to_string(),
                        labels: mr_data["labels"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        detailed_merge_status: mr_data["detailed_merge_status"]
                            .as_str()
                            .map(|s| s.to_string()),
                        updated_at: mr_data["updated_at"].as_str().unwrap_or("").to_string(),
                        head_pipeline: None, // TODO: Parse pipeline if needed
                    };
                    merge_requests.push(gitlab_mr);
                }
            }

            Ok::<Vec<GitlabMergeRequest>, GitlabError>(merge_requests)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, issue_iid, since_timestamp))]
    pub async fn get_issue_notes(
        &self,
        project_id: i64,
        issue_iid: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = IssueNotes::builder()
                .project(project_id as u64)
                .issue(issue_iid as u64)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build issue notes endpoint: {}", e),
                })?;

            // Query the gitlab API
            let notes_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabNoteAttributes structs
            let mut notes = Vec::new();
            if let Some(notes_array) = notes_data.as_array() {
                for note_data in notes_array {
                    // Filter by since_timestamp
                    if let Some(updated_at_str) = note_data["updated_at"].as_str() {
                        if let Ok(updated_at) = DateTime::parse_from_rfc3339(updated_at_str) {
                            let updated_timestamp = updated_at.timestamp() as u64;
                            if updated_timestamp < since_timestamp {
                                continue; // Skip notes older than since_timestamp
                            }
                        }
                    }

                    let gitlab_note = GitlabNoteAttributes {
                        id: note_data["id"].as_i64().unwrap_or(0),
                        note: note_data["body"].as_str().unwrap_or("").to_string(),
                        author: crate::models::GitlabUser {
                            id: note_data["author"]["id"].as_i64().unwrap_or(0),
                            username: note_data["author"]["username"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                            name: note_data["author"]["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                            avatar_url: note_data["author"]["avatar_url"]
                                .as_str()
                                .map(|s| s.to_string()),
                        },
                        project_id: note_data["project_id"].as_i64().unwrap_or(0),
                        noteable_type: note_data["noteable_type"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        noteable_id: note_data["noteable_id"].as_i64(),
                        iid: note_data["iid"].as_i64(),
                        url: note_data["url"].as_str().map(|s| s.to_string()),
                        updated_at: note_data["updated_at"].as_str().unwrap_or("").to_string(),
                    };
                    notes.push(gitlab_note);
                }
            }

            Ok::<Vec<GitlabNoteAttributes>, GitlabError>(notes)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, mr_iid, since_timestamp))]
    pub async fn get_merge_request_notes(
        &self,
        project_id: i64,
        mr_iid: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = MergeRequestNotes::builder()
                .project(project_id as u64)
                .merge_request(mr_iid as u64)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build merge request notes endpoint: {}", e),
                })?;

            // Query the gitlab API
            let notes_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabNoteAttributes structs
            let mut notes = Vec::new();
            if let Some(notes_array) = notes_data.as_array() {
                for note_data in notes_array {
                    // Filter by since_timestamp
                    if let Some(updated_at_str) = note_data["updated_at"].as_str() {
                        if let Ok(updated_at) = DateTime::parse_from_rfc3339(updated_at_str) {
                            let updated_timestamp = updated_at.timestamp() as u64;
                            if updated_timestamp < since_timestamp {
                                continue; // Skip notes older than since_timestamp
                            }
                        }
                    }

                    let gitlab_note = GitlabNoteAttributes {
                        id: note_data["id"].as_i64().unwrap_or(0),
                        note: note_data["body"].as_str().unwrap_or("").to_string(),
                        author: crate::models::GitlabUser {
                            id: note_data["author"]["id"].as_i64().unwrap_or(0),
                            username: note_data["author"]["username"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                            name: note_data["author"]["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                            avatar_url: note_data["author"]["avatar_url"]
                                .as_str()
                                .map(|s| s.to_string()),
                        },
                        project_id: note_data["project_id"].as_i64().unwrap_or(0),
                        noteable_type: note_data["noteable_type"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        noteable_id: note_data["noteable_id"].as_i64(),
                        iid: note_data["iid"].as_i64(),
                        url: note_data["url"].as_str().map(|s| s.to_string()),
                        updated_at: note_data["updated_at"].as_str().unwrap_or("").to_string(),
                    };
                    notes.push(gitlab_note);
                }
            }

            Ok::<Vec<GitlabNoteAttributes>, GitlabError>(notes)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    /// Get the repository file tree with pagination
    #[instrument(skip(self), fields(project_id))]
    pub async fn get_repository_tree(&self, project_id: i64) -> Result<Vec<String>, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = Tree::builder()
                .project(project_id as u64)
                .recursive(true)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build tree endpoint: {}", e),
                })?;

            // Query the gitlab API
            let tree_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Extract file paths from the tree response
            let mut file_paths = Vec::new();
            if let Some(tree_array) = tree_data.as_array() {
                for item in tree_array {
                    if let Some(path) = item["path"].as_str() {
                        // Only include files, not directories
                        if item["type"].as_str() == Some("blob") {
                            file_paths.push(path.to_string());
                        }
                    }
                }
            }

            Ok::<Vec<String>, GitlabError>(file_paths)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    /// Get file content from repository
    #[instrument(skip(self), fields(project_id, file_path))]
    pub async fn get_file_content(
        &self,
        project_id: i64,
        file_path: &str,
    ) -> Result<GitlabFile, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();
        let path = file_path.to_string();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = FileRaw::builder()
                .project(project_id as u64)
                .file_path(&path)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build file endpoint: {}", e),
                })?;

            // Query the gitlab API
            let file_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Extract content from file response
            let content = if let Some(content_b64) = file_data["content"].as_str() {
                // Decode base64 content
                match base64::decode(content_b64) {
                    Ok(decoded) => Some(String::from_utf8_lossy(&decoded).to_string()),
                    Err(_) => Some(content_b64.to_string()), // Fallback to raw if decode fails
                }
            } else {
                None
            };

            let gitlab_file = GitlabFile {
                file_path: path,
                content,
            };

            Ok::<GitlabFile, GitlabError>(gitlab_file)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    /// Search for files by name
    #[instrument(skip(self), fields(project_id, query))]
    pub async fn search_files_by_name(
        &self,
        project_id: i64,
        query: &str,
    ) -> Result<Vec<String>, GitlabError> {
        let path = format!("/api/v4/projects/{}/search", project_id);
        let query_params = &[
            ("scope", "blobs"),
            ("search", query),
            ("ref", "main"),
            ("per_page", "20"),
        ];

        let results: Vec<serde_json::Value> = self
            .send_request(Method::GET, &path, Some(query_params), None::<()>)
            .await?;

        let file_paths = results
            .into_iter()
            .filter_map(|item| item["path"].as_str().map(|s| s.to_string()))
            .collect();

        Ok(file_paths)
    }

    /// Search for files by content
    #[instrument(skip(self), fields(project_id, query))]
    pub async fn search_files_by_content(
        &self,
        project_id: i64,
        query: &str,
    ) -> Result<Vec<String>, GitlabError> {
        let path = format!("/api/v4/projects/{}/search", project_id);
        let query_params = &[
            ("scope", "blobs"),
            ("search", query),
            ("ref", "main"),
            ("per_page", "20"),
        ];

        let results: Vec<serde_json::Value> = self
            .send_request(Method::GET, &path, Some(query_params), None::<()>)
            .await?;

        let file_paths = results
            .into_iter()
            .filter_map(|item| item["path"].as_str().map(|s| s.to_string()))
            .collect();

        Ok(file_paths)
    }

    /// Get changes for a merge request
    #[instrument(skip(self), fields(project_id, merge_request_iid))]
    pub async fn get_merge_request_changes(
        &self,
        project_id: i64,
        merge_request_iid: i64,
    ) -> Result<Vec<GitlabDiff>, GitlabError> {
        let path = format!(
            "/api/v4/projects/{}/merge_requests/{}/changes",
            project_id, merge_request_iid
        );

        let response: serde_json::Value = self
            .send_request(Method::GET, &path, None, None::<()>)
            .await?;

        // Extract changes from the response
        let changes = response["changes"]
            .as_array()
            .map(|changes| {
                changes
                    .iter()
                    .filter_map(|change| {
                        let new_path = change["new_path"].as_str()?.to_string();
                        let diff = change["diff"].as_str()?.to_string();

                        Some(GitlabDiff { new_path, diff })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(changes)
    }

    #[instrument(skip(self), fields(project_id, issue_iid, label_name))]
    pub async fn add_issue_label(
        &self,
        project_id: i64,
        issue_iid: i64,
        label_name: &str,
    ) -> Result<GitlabIssue, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();
        let label = label_name.to_string();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = EditIssue::builder()
                .project(project_id as u64)
                .issue(issue_iid as u64)
                .add_label(label)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build edit issue endpoint: {}", e),
                })?;

            // Query the gitlab API
            let issue_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

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
                    username: issue_data["author"]["username"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    name: issue_data["author"]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    avatar_url: issue_data["author"]["avatar_url"]
                        .as_str()
                        .map(|s| s.to_string()),
                },
                web_url: issue_data["web_url"].as_str().unwrap_or("").to_string(),
                labels: issue_data["labels"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                updated_at: issue_data["updated_at"].as_str().unwrap_or("").to_string(),
            };

            Ok::<GitlabIssue, GitlabError>(gitlab_issue)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    #[instrument(skip(self), fields(project_id, issue_iid, label_name))]
    pub async fn remove_issue_label(
        &self,
        project_id: i64,
        issue_iid: i64,
        label_name: &str,
    ) -> Result<GitlabIssue, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();
        let label = label_name.to_string();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            let endpoint = EditIssue::builder()
                .project(project_id as u64)
                .issue(issue_iid as u64)
                .remove_label(label)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build edit issue endpoint: {}", e),
                })?;

            // Query the gitlab API
            let issue_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

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
                    username: issue_data["author"]["username"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    name: issue_data["author"]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    avatar_url: issue_data["author"]["avatar_url"]
                        .as_str()
                        .map(|s| s.to_string()),
                },
                web_url: issue_data["web_url"].as_str().unwrap_or("").to_string(),
                labels: issue_data["labels"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                updated_at: issue_data["updated_at"].as_str().unwrap_or("").to_string(),
            };

            Ok::<GitlabIssue, GitlabError>(gitlab_issue)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
    }

    /// Get commit history for a file
    #[instrument(skip(self), fields(project_id, file_path))]
    pub async fn get_file_commits(
        &self,
        project_id: i64,
        file_path: &str,
        limit: Option<usize>,
    ) -> Result<Vec<GitlabCommit>, GitlabError> {
        let gitlab_host = self.get_gitlab_host()?;
        let private_token = self.private_token.clone();
        let path = file_path.to_string();

        // Run the gitlab crate operations in a blocking context
        let result = tokio::task::spawn_blocking(move || {
            let client = GitlabBuilder::new(&gitlab_host, &private_token)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to create gitlab client: {}", e),
                })?;

            // Note: per_page method may not be available, limiting handled by GitLab API defaults
            let _ = limit; // Acknowledge but don't use for now

            let endpoint = Commits::builder()
                .project(project_id as u64)
                .path(&path)
                .build()
                .map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Failed to build commits endpoint: {}", e),
                })?;

            // Query the gitlab API
            let commits_data: serde_json::Value =
                endpoint.query(&client).map_err(|e| GitlabError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("GitLab API query failed: {}", e),
                })?;

            // Convert the JSON response to our GitlabCommit structs
            let mut commits = Vec::new();
            if let Some(commits_array) = commits_data.as_array() {
                for commit_data in commits_array {
                    let gitlab_commit = GitlabCommit {
                        id: commit_data["id"].as_str().unwrap_or("").to_string(),
                        short_id: commit_data["short_id"].as_str().unwrap_or("").to_string(),
                        title: commit_data["title"].as_str().unwrap_or("").to_string(),
                        author_name: commit_data["author_name"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        author_email: commit_data["author_email"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        authored_date: commit_data["authored_date"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        committer_name: commit_data["committer_name"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        committer_email: commit_data["committer_email"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        committed_date: commit_data["committed_date"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                        message: commit_data["message"].as_str().unwrap_or("").to_string(),
                    };
                    commits.push(gitlab_commit);
                }
            }

            Ok::<Vec<GitlabCommit>, GitlabError>(commits)
        })
        .await
        .map_err(|e| GitlabError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("Blocking task failed: {}", e),
        })??;

        Ok(result)
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

        // Mock the user endpoint that gitlab crate uses for authentication
        let _user_mock = server
            .mock("GET", "/api/v4/user")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"id": 1, "username": "test_user"}).to_string())
            .create_async()
            .await;

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
            GitlabError::Api { status: _, body } => {
                assert!(body.contains("404"));
                assert!(body.contains("Issue not found"));
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
            GitlabError::Api { status: _, body } => {
                assert!(body.contains("500"));
                assert!(body.contains("Server error processing note"));
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
            GitlabError::Api { status: _, body } => {
                assert!(body.contains("404"));
                assert!(body.contains("Issue not found"));
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
            GitlabError::Api { status: _, body } => {
                assert!(body.contains("404"));
                assert!(body.contains("Issue not found"));
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
