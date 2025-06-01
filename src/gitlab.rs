use chrono::{DateTime, TimeZone, Utc};
use reqwest::{header, Client, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, instrument};
use url::Url;
use urlencoding::encode;

use crate::config::AppSettings;
use crate::models::{
    GitlabCommit, GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes, GitlabProject,
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

#[derive(Debug)]
pub struct GitlabApiClient {
    client: Client,
    gitlab_url: Url,
    private_token: String,
    settings: Arc<AppSettings>,
}

impl GitlabApiClient {
    pub fn new(settings: Arc<AppSettings>) -> Result<Self, GitlabError> {
        let gitlab_url = Url::parse(&settings.gitlab_url)?;
        let client = Client::new();
        Ok(Self {
            client,
            gitlab_url,
            private_token: settings.gitlab_token.clone(),
            settings,
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
            .map_err(GitlabError::Deserialization)?;
        Ok(parsed_response)
    }

    #[instrument(skip(self), fields(project_id, issue_iid))]
    pub async fn get_issue(
        &self,
        project_id: i64,
        issue_iid: i64,
    ) -> Result<GitlabIssue, GitlabError> {
        let path = format!("/api/v4/projects/{}/issues/{}", project_id, issue_iid);
        self.send_request(Method::GET, &path, None, None::<()>)
            .await
    }

    #[instrument(skip(self), fields(project_id, mr_iid))]
    pub async fn get_merge_request(
        &self,
        project_id: i64,
        mr_iid: i64,
    ) -> Result<GitlabMergeRequest, GitlabError> {
        let path = format!("/api/v4/projects/{}/merge_requests/{}", project_id, mr_iid);
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
        let path = format!("/api/v4/projects/{}/issues/{}/notes", project_id, issue_iid);
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
        let path = format!(
            "/api/v4/projects/{}/merge_requests/{}/notes",
            project_id, mr_iid
        );
        let body = serde_json::json!({"body": comment_body});
        self.send_request(Method::POST, &path, None, Some(body))
            .await
    }

    #[instrument(skip(self), fields(repo_path))]
    pub async fn get_project_by_path(&self, repo_path: &str) -> Result<GitlabProject, GitlabError> {
        let encoded_path = urlencoding::encode(repo_path);
        let path = format!("/api/v4/projects/{}", encoded_path);
        self.send_request(Method::GET, &path, None, None::<()>)
            .await
    }

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_issues(
        &self,
        project_id: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabIssue>, GitlabError> {
        let path = format!("/api/v4/projects/{}/issues", project_id);
        let dt = DateTime::from_timestamp(since_timestamp as i64, 0).unwrap_or_else(|| {
            Utc.timestamp_opt(0, 0)
                .single()
                .expect("Fallback timestamp failed for 0")
        });
        let formatted_timestamp_string = dt.to_rfc3339();

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

    #[instrument(skip(self), fields(project_id, since_timestamp))]
    pub async fn get_merge_requests(
        &self,
        project_id: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabMergeRequest>, GitlabError> {
        let path = format!("/api/v4/projects/{}/merge_requests", project_id);
        let dt = DateTime::from_timestamp(since_timestamp as i64, 0).unwrap_or_else(|| {
            Utc.timestamp_opt(0, 0)
                .single()
                .expect("Fallback timestamp failed for 0")
        });
        let formatted_timestamp_string = dt.to_rfc3339();

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

    #[instrument(skip(self), fields(project_id, issue_iid, since_timestamp))]
    pub async fn get_issue_notes(
        &self,
        project_id: i64,
        issue_iid: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        let path = format!("/api/v4/projects/{}/issues/{}/notes", project_id, issue_iid);
        let dt = DateTime::from_timestamp(since_timestamp as i64, 0).unwrap_or_else(|| {
            Utc.timestamp_opt(0, 0)
                .single()
                .expect("Fallback timestamp failed for 0")
        });
        let formatted_timestamp_string = dt.to_rfc3339();

        let query_params_values = [
            ("created_after", formatted_timestamp_string),
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

    #[instrument(skip(self), fields(project_id, mr_iid, since_timestamp))]
    pub async fn get_merge_request_notes(
        &self,
        project_id: i64,
        mr_iid: i64,
        since_timestamp: u64,
    ) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
        let path = format!(
            "/api/v4/projects/{}/merge_requests/{}/notes",
            project_id, mr_iid
        );
        let dt = DateTime::from_timestamp(since_timestamp as i64, 0).unwrap_or_else(|| {
            Utc.timestamp_opt(0, 0)
                .single()
                .expect("Fallback timestamp failed for 0")
        });
        let formatted_timestamp_string = dt.to_rfc3339();

        let query_params_values = [
            ("created_after", formatted_timestamp_string),
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

    /// Get the repository file tree with pagination
    #[instrument(skip(self), fields(project_id))]
    pub async fn get_repository_tree(&self, project_id: i64) -> Result<Vec<String>, GitlabError> {
        let path = format!("/api/v4/projects/{}/repository/tree", project_id);
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
        let file_paths = all_items
            .into_iter()
            .filter(|item| item["type"].as_str().unwrap_or("") == "blob") // Only include files, not directories
            .filter_map(|item| item["path"].as_str().map(|s| s.to_string()))
            .collect();

        Ok(file_paths)
    }

    /// Get file content from repository
    #[instrument(skip(self), fields(project_id, file_path))]
    pub async fn get_file_content(
        &self,
        project_id: i64,
        file_path: &str,
    ) -> Result<GitlabFile, GitlabError> {
        let path = format!(
            "/api/v4/projects/{}/repository/files/{}",
            project_id,
            encode(file_path)
        );
        let ref_str = self.settings.default_branch.as_str();
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
        let path = format!("/api/v4/projects/{}/issues/{}", project_id, issue_iid);
        let body = serde_json::json!({ "add_labels": label_name });
        self.send_request(Method::PUT, &path, None, Some(body))
            .await
    }

    #[instrument(skip(self), fields(project_id, issue_iid, label_name))]
    pub async fn remove_issue_label(
        &self,
        project_id: i64,
        issue_iid: i64,
        label_name: &str,
    ) -> Result<GitlabIssue, GitlabError> {
        let path = format!("/api/v4/projects/{}/issues/{}", project_id, issue_iid);
        let body = serde_json::json!({ "remove_labels": label_name });
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
        let path = format!("/api/v4/projects/{}/repository/commits", project_id);

        let per_page = limit.unwrap_or(5).to_string();
        let query_params = vec![("path", file_path), ("per_page", &per_page)];

        self.send_request(Method::GET, &path, Some(&query_params), None::<()>)
            .await
    }
}
