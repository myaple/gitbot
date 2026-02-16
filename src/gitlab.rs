use chrono::{DateTime, TimeZone, Utc};
use dashmap::DashMap;
use reqwest::{header, Client, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, error, instrument, warn};
use url::Url;
use urlencoding::encode;

use crate::config::AppSettings;
use crate::models::{
    GitlabBranch, GitlabCommit, GitlabIssue, GitlabLabel, GitlabMergeRequest, GitlabNoteAttributes,
    GitlabProject, GitlabSearchResult,
};
use crate::repo_context::{GitlabDiff, GitlabFile};

#[derive(Error, Debug)]
pub enum GitlabError {
    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("API error: {status} - {body}")]
    Api { status: StatusCode, body: String },
    #[error("URL parsing error: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("Failed to deserialize response: {0}")]
    Deserialization(reqwest::Error),
}

const REPO_TREE_CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

/// Options for querying issues from GitLab
#[derive(Debug, Default, Clone)]
pub struct IssueQueryOptions {
    /// Filter issues updated after this timestamp
    pub updated_after: Option<u64>,
    /// Filter issues by state (e.g., "opened", "closed")
    pub state: Option<String>,
    /// Filter issues by labels (comma-separated)
    pub labels: Option<String>,
    /// Number of results per page (default: 100)
    pub per_page: Option<usize>,
    /// Field to order by (e.g., "created_at", "updated_at")
    pub order_by: Option<String>,
    /// Sort order ("asc" or "desc")
    pub sort: Option<String>,
}

/// Label operation type for updating issue labels
#[derive(Debug, Clone)]
pub enum LabelOperation {
    /// Add labels to an issue (preserves existing labels)
    Add(Vec<String>),
    /// Remove labels from an issue (preserves other labels)
    Remove(Vec<String>),
    /// Set labels on an issue (replaces all existing labels)
    Set(Vec<String>),
}

#[derive(Debug)]
pub struct GitlabApiClient {
    client: Client,
    gitlab_url: Url,
    private_token: String,
    settings: Arc<AppSettings>,
    repo_tree_cache: DashMap<i64, (Vec<String>, Instant)>,
}

/// Helper function to format a Unix timestamp into RFC3339 format
fn format_timestamp(timestamp: u64) -> String {
    DateTime::from_timestamp(timestamp as i64, 0)
        .unwrap_or_else(|| {
            Utc.timestamp_opt(0, 0)
                .single()
                .expect("Fallback timestamp failed for 0")
        })
        .to_rfc3339()
}

impl GitlabApiClient {
    pub fn new(settings: Arc<AppSettings>) -> Result<Self, GitlabError> {
        let gitlab_url = Url::parse(&settings.gitlab_url)?;

        // Configure client with proper settings for GitLab.com
        let client_builder = Client::builder()
            .timeout(std::time::Duration::from_secs(60)) // 60 second timeout for requests
            .connect_timeout(std::time::Duration::from_secs(10)) // 10 second connection timeout
            .user_agent("gitbot/0.1.0") // Set proper User-Agent
            .redirect(reqwest::redirect::Policy::limited(10)); // Allow redirects

        // Try to configure advanced settings, but don't fail if they're not supported
        let client = match client_builder
            .pool_max_idle_per_host(0) // Disable connection pooling to avoid issues
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .http2_keep_alive_interval(None) // Disable HTTP2 keep-alive
            .http2_keep_alive_timeout(std::time::Duration::from_secs(30))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .build()
        {
            Ok(client) => {
                debug!("HTTP client configured with advanced settings");
                client
            }
            Err(e) => {
                warn!("Failed to configure HTTP client with advanced settings ({}), trying basic configuration", e);
                Client::builder()
                    .timeout(std::time::Duration::from_secs(60))
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .user_agent("gitbot/0.1.0")
                    .redirect(reqwest::redirect::Policy::limited(10))
                    .build()
                    .map_err(GitlabError::Request)?
            }
        };

        Ok(Self {
            client,
            gitlab_url,
            private_token: settings.gitlab_token.clone(),
            settings,
            repo_tree_cache: DashMap::new(),
        })
    }

    #[instrument(skip(self, body), fields(method, path))]
    pub async fn send_request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query_params: Option<&[(&str, &str)]>,
        body: Option<impl Serialize + Debug>, // Added Debug for logging
    ) -> Result<T, GitlabError> {
        let mut url = self.gitlab_url.join(path)?;
        if let Some(params) = query_params {
            url.query_pairs_mut().extend_pairs(params);
        }

        debug!("Sending request to URL: {}", url);
        debug!("Request method: {:?}", method);
        if let Some(b) = &body {
            debug!("Request body: {:?}", b);
        }

        let mut request_builder = self
            .client
            .request(method.clone(), url.clone())
            .header("PRIVATE-TOKEN", &self.private_token);

        if body.is_some() {
            request_builder = request_builder.header(header::CONTENT_TYPE, "application/json");
        }

        if let Some(b) = body {
            request_builder = request_builder.json(&b);
        }

        debug!("Executing {} request to {}", method, url);
        let start_time = std::time::Instant::now();

        let response = request_builder.send().await.map_err(|e| {
            let elapsed = start_time.elapsed();
            error!(
                "Request failed after {}ms for {} {}: {}",
                elapsed.as_millis(),
                method,
                url,
                e
            );
            GitlabError::Request(e)
        })?;

        let elapsed = start_time.elapsed();
        debug!(
            "Request completed in {}ms for {} {}",
            elapsed.as_millis(),
            method,
            url
        );

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
            .map_err(GitlabError::Deserialization)?;
        Ok(parsed_response)
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn get_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
    ) -> Result<GitlabIssue, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/issues/{issue_iid}");
        self.send_request(Method::GET, &path, None, None::<()>)
            .await
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn get_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
    ) -> Result<GitlabMergeRequest, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/merge_requests/{mr_iid}");
        self.send_request(Method::GET, &path, None, None::<()>)
            .await
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn post_comment_to_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
        comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/issues/{issue_iid}/notes");
        let body = serde_json::json!({"body": comment_body});
        self.send_request(Method::POST, &path, None, Some(body))
            .await
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn post_comment_to_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
        comment_body: &str,
    ) -> Result<GitlabNoteAttributes, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/merge_requests/{mr_iid}/notes");
        let body = serde_json::json!({"body": comment_body});
        self.send_request(Method::POST, &path, None, Some(body))
            .await
    }

    #[instrument(skip(self), fields(repo_path))]
    pub async fn get_project_by_path(&self, repo_path: &str) -> Result<GitlabProject, GitlabError> {
        let encoded_path = urlencoding::encode(repo_path);
        let path = format!("/api/v4/projects/{encoded_path}");
        self.send_request(Method::GET, &path, None, None::<()>)
            .await
    }

    /// Get issues from a project with flexible query options
    ///
    /// This is the consolidated method for all issue fetching operations.
    /// Use IssueQueryOptions to specify filters like state, labels, timestamps, etc.
    ///
    /// # Examples
    /// ```ignore
    /// // Get all issues updated after timestamp
    /// let options = IssueQueryOptions {
    ///     updated_after: Some(timestamp),
    ///     ..Default::default()
    /// };
    /// let issues = client.get_issues(project_id, options).await?;
    ///
    /// // Get opened issues only
    /// let options = IssueQueryOptions {
    ///     state: Some("opened".to_string()),
    ///     ..Default::default()
    /// };
    /// let issues = client.get_issues(project_id, options).await?;
    /// ```
    #[instrument(skip(self, options), fields(project_id))]
    pub async fn get_issues(
        &self,
        project_id: i64,
        options: IssueQueryOptions,
    ) -> Result<Vec<GitlabIssue>, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/issues");

        let mut query_params_values: Vec<(String, String)> = Vec::new();

        // Add updated_after filter if specified
        if let Some(timestamp) = options.updated_after {
            query_params_values.push(("updated_after".to_string(), format_timestamp(timestamp)));
        }

        // Add state filter if specified
        if let Some(state) = options.state {
            query_params_values.push(("state".to_string(), state));
        }

        // Add labels filter if specified
        if let Some(labels) = options.labels {
            query_params_values.push(("labels".to_string(), labels));
        }

        // Add order_by if specified
        if let Some(order_by) = options.order_by {
            query_params_values.push(("order_by".to_string(), order_by));
        }

        // Add sort order (default: asc)
        let sort = options.sort.unwrap_or_else(|| "asc".to_string());
        query_params_values.push(("sort".to_string(), sort));

        // Add per_page (default: 100)
        let per_page = options.per_page.unwrap_or(100);
        query_params_values.push(("per_page".to_string(), per_page.to_string()));

        let params: Vec<(&str, &str)> = query_params_values
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        self.send_request(Method::GET, &path, Some(&params), None::<()>)
            .await
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_merge_requests(
        &self,
        project_id: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabMergeRequest>, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/merge_requests");
        let formatted_timestamp_string = format_timestamp(since_timestamp);

        let query_params_values = [
            ("updated_after", formatted_timestamp_string),
            ("sort", "asc".to_string()),
            ("per_page", "100".to_string()),
        ];
        let params: Vec<(&str, &str)> = query_params_values
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        self.send_request(Method::GET, &path, Some(&params), None::<()>)
            .await
    }

    /// Get notes from an issue
    ///
    /// Optionally filter by creation timestamp. Pass None to get all notes.
    ///
    /// # Arguments
    /// * `project_id` - The project ID
    /// * `issue_iid` - The issue IID
    /// * `since_timestamp` - Optional timestamp filter (notes created after this time)
    #[instrument(skip(self), fields(project_id, issue_iid, since_timestamp))]
    pub async fn get_issue_notes(
        &self,
        project_id: i64,
        issue_iid: i64,
        since_timestamp: Option<u64>,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/issues/{issue_iid}/notes");

        let mut query_params_values =
            vec![("sort", "asc".to_string()), ("per_page", "100".to_string())];

        if let Some(timestamp) = since_timestamp {
            query_params_values.push(("created_after", format_timestamp(timestamp)));
        }

        let params: Vec<(&str, &str)> = query_params_values
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        self.send_request(Method::GET, &path, Some(&params), None::<()>)
            .await
    }

    /// Get notes from a merge request
    ///
    /// Optionally filter by creation timestamp. Pass None to get all notes.
    ///
    /// # Arguments
    /// * `project_id` - The project ID
    /// * `mr_iid` - The merge request IID
    /// * `since_timestamp` - Optional timestamp filter (notes created after this time)
    #[instrument(skip(self), fields(project_id, mr_iid, since_timestamp))]
    pub async fn get_merge_request_notes(
        &self,
        project_id: i64,
        mr_iid: i64,
        since_timestamp: Option<u64>,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/merge_requests/{mr_iid}/notes");

        let mut query_params_values =
            vec![("sort", "asc".to_string()), ("per_page", "100".to_string())];

        if let Some(timestamp) = since_timestamp {
            query_params_values.push(("created_after", format_timestamp(timestamp)));
        }

        let params: Vec<(&str, &str)> = query_params_values
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        self.send_request(Method::GET, &path, Some(&params), None::<()>)
            .await
    }

    /// Get the repository file tree with pagination
    #[instrument(skip(self), fields(project_id))]
    pub async fn get_repository_tree(&self, project_id: i64) -> Result<Vec<String>, GitlabError> {
        // Check cache first
        if let Some(entry) = self.repo_tree_cache.get(&project_id) {
            let (files, timestamp) = entry.value();
            if timestamp.elapsed() < REPO_TREE_CACHE_TTL {
                debug!(
                    "Returning cached repository tree for project {}",
                    project_id
                );
                return Ok(files.clone());
            }
        }

        let path = format!("/api/v4/projects/{project_id}/repository/tree");
        let per_page = 100;

        let mut all_items = Vec::new();
        let mut current_page = 1;

        loop {
            let query = &[
                ("recursive", "true"),
                ("per_page", &per_page.to_string()),
                ("page", &current_page.to_string()),
            ];

            debug!("Fetching repository tree page {}", current_page);

            // Create the request manually to access headers
            let mut url = self.gitlab_url.join(&path)?;
            url.query_pairs_mut().extend_pairs(query);

            let request_builder = self
                .client
                .request(Method::GET, url)
                .header("PRIVATE-TOKEN", &self.private_token);

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

            // Check pagination headers
            let total_pages = response
                .headers()
                .get("X-Total-Pages")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);

            // Parse the response body
            let items: Vec<serde_json::Value> = response
                .json()
                .await
                .map_err(GitlabError::Deserialization)?;

            // Check if we've reached the last page
            let is_empty = items.is_empty();

            // Add items to our collection
            all_items.extend(items);

            // Break if we've reached the last page or no items were returned
            if current_page >= total_pages || is_empty {
                break;
            }

            // Move to the next page
            current_page += 1;
        }

        debug!(
            "Fetched a total of {} items from repository tree",
            all_items.len()
        );

        // Extract file paths
        let file_paths: Vec<String> = all_items
            .into_iter()
            .filter(|item| item["type"].as_str().unwrap_or("") == "blob") // Only include files, not directories
            .filter_map(|item| item["path"].as_str().map(|s| s.to_string()))
            .collect();

        // Update cache
        self.repo_tree_cache
            .insert(project_id, (file_paths.clone(), Instant::now()));

        Ok(file_paths)
    }

    /// Get file content from repository
    #[instrument(skip(self), fields(project_id, file_path, ref_name))]
    pub async fn get_file_content(
        &self,
        project_id: i64,
        file_path: &str,
        ref_name: Option<&str>,
    ) -> Result<GitlabFile, GitlabError> {
        let path = format!(
            "/api/v4/projects/{}/repository/files/{}",
            project_id,
            encode(file_path)
        );
        let default_ref = self.settings.default_branch.as_str();
        let ref_str = ref_name.unwrap_or(default_ref);
        let query = &[("ref", ref_str)];

        let mut file: GitlabFile = self
            .send_request(Method::GET, &path, Some(query), None::<()>)
            .await?;

        // If the file is too large, skip content
        if file.size > 100_000 {
            debug!(
                "File {} is too large ({} bytes), skipping content",
                file_path, file.size
            );
            return Ok(file);
        }

        // Decode content if needed
        if let Some(content) = &file.content {
            if let Some(encoding) = &file.encoding {
                if encoding == "base64" {
                    if let Ok(decoded) = base64::decode(content) {
                        if let Ok(text) = String::from_utf8(decoded) {
                            file.content = Some(text);
                        }
                    }
                }
            }
        }

        Ok(file)
    }


    /// Get changes for a merge request
    #[instrument(skip(self), fields(project_id, merge_request_iid))]
    pub async fn get_merge_request_changes(
        &self,
        project_id: i64,
        merge_request_iid: i64,
    ) -> Result<Vec<GitlabDiff>, GitlabError> {
        let path =
            format!("/api/v4/projects/{project_id}/merge_requests/{merge_request_iid}/changes");

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

    /// Update labels on an issue
    ///
    /// This is the consolidated method for all label operations.
    /// Use LabelOperation to specify whether to add, remove, or set labels.
    ///
    /// # Examples
    /// ```ignore
    /// // Add a single label
    /// client.update_issue_labels(
    ///     project_id,
    ///     issue_iid,
    ///     LabelOperation::Add(vec!["bug".to_string()])
    /// ).await?;
    ///
    /// // Remove multiple labels
    /// client.update_issue_labels(
    ///     project_id,
    ///     issue_iid,
    ///     LabelOperation::Remove(vec!["wontfix".to_string(), "duplicate".to_string()])
    /// ).await?;
    ///
    /// // Replace all labels
    /// client.update_issue_labels(
    ///     project_id,
    ///     issue_iid,
    ///     LabelOperation::Set(vec!["feature".to_string(), "priority".to_string()])
    /// ).await?;
    /// ```
    #[instrument(skip(self, operation), fields(project_id, issue_iid))]
    pub async fn update_issue_labels(
        &self,
        project_id: i64,
        issue_iid: i64,
        operation: LabelOperation,
    ) -> Result<GitlabIssue, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/issues/{issue_iid}");

        let body = match operation {
            LabelOperation::Add(labels) => {
                let labels_str = labels.join(",");
                serde_json::json!({ "add_labels": labels_str })
            }
            LabelOperation::Remove(labels) => {
                let labels_str = labels.join(",");
                serde_json::json!({ "remove_labels": labels_str })
            }
            LabelOperation::Set(labels) => {
                let labels_str = labels.join(",");
                serde_json::json!({ "labels": labels_str })
            }
        };

        self.send_request(Method::PUT, &path, None, Some(body))
            .await
    }

    /// Get commit history for a file
    #[instrument(skip(self), fields(project_id, file_path))]
    pub async fn get_file_commits(
        &self,
        project_id: i64,
        file_path: &str,
        limit: Option<usize>,
    ) -> Result<Vec<GitlabCommit>, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/repository/commits");

        let per_page = limit.unwrap_or(5).to_string();
        let query_params = vec![("path", file_path), ("per_page", &per_page)];

        self.send_request(Method::GET, &path, Some(&query_params), None::<()>)
            .await
    }

    /// Search for code in a GitLab repository
    ///
    /// Searches both file names and file content using GitLab's search API with scope=blobs.
    /// This is the consolidated method for all code/file searching operations.
    ///
    /// # Arguments
    /// * `project_id` - The project ID to search in
    /// * `search_query` - The search query string
    /// * `branch` - The branch/ref to search in (e.g., "main", "develop")
    #[instrument(skip(self), fields(project_id, search_query, branch))]
    pub async fn search_code(
        &self,
        project_id: i64,
        search_query: &str,
        branch: &str,
    ) -> Result<Vec<GitlabSearchResult>, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/search");
        let query_params = &[
            ("scope", "blobs"),
            ("search", search_query),
            ("ref", branch),
            ("per_page", "100"),
        ];

        self.send_request(Method::GET, &path, Some(query_params), None::<()>)
            .await
    }

    /// List all branches in a GitLab project
    #[instrument(skip(self), fields(project_id))]
    pub async fn get_branches(&self, project_id: i64) -> Result<Vec<GitlabBranch>, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/repository/branches");
        self.send_request(Method::GET, &path, None, None::<()>)
            .await
    }

    /// Get all labels for a project
    #[instrument(skip(self), fields(project_id))]
    pub async fn get_labels(&self, project_id: i64) -> Result<Vec<GitlabLabel>, GitlabError> {
        let path = format!("/api/v4/projects/{project_id}/labels");
        let query_params = &[("per_page", "100")];
        self.send_request(Method::GET, &path, Some(query_params), None::<()>)
            .await
    }


}
