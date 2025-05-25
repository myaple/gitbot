use crate::config::AppSettings;
use crate::models::{GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes, GitlabProject};
use crate::repo_context::{GitlabDiff, GitlabFile};
use chrono::{DateTime, TimeZone, Utc};
use gitlab::Gitlab;
use reqwest::{header, Method, StatusCode}; // Remove Client as it's no longer directly used
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use thiserror::Error;
use tracing::{debug, error, instrument}; // Removed 'instrument' as it might not be needed if methods are removed/changed
use url::Url;
// Removed urlencoding::encode as it's part of the old client's implementation details

#[derive(Error, Debug)]
pub enum GitlabError {
    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error), // This might be covered by GitlabClient error or still be needed for other direct reqwest calls if any
    #[error("API error: {status} - {body}")]
    Api { status: StatusCode, body: String }, // This might be superseded by GitlabClient error
    #[error("URL parsing error: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("Failed to deserialize response: {0}")]
    Deserialization(reqwest::Error), // This might be covered by GitlabClient error
    #[error("GitLab client error: {0}")]
    GitlabClient(#[from] gitlab::api::ApiError<gitlab::api::clients::RestError>),
    #[error("Content decoding error: {0}")]
    ContentDecoding(String),
}

// GitlabApiClient struct is removed.

// The new function now returns a gitlab::Gitlab client.
// All methods of the old GitlabApiClient are removed as they would be using the old client.
// These will be re-implemented or replaced by direct calls to the new gitlab::Gitlab client in subsequent tasks.
pub fn new(settings: &AppSettings) -> Result<Gitlab, GitlabError> {
    Gitlab::new(&settings.gitlab_url, &settings.gitlab_token)
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::Client(e)))
}

pub async fn get_issue(
    client: &Gitlab,
    project_id: u64,
    issue_iid: u64,
) -> Result<GitlabIssue, GitlabError> {
    let endpoint = gitlab::api::projects::issues::Issue::builder()
        .project(project_id)
        .issue(issue_iid)
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?; // Or a more specific error mapping

    let upstream_issue: gitlab::Issue = gitlab::api::AsyncQuery::query_async(endpoint, client)
        .await
        .map_err(GitlabError::GitlabClient)?;

    // Convert gitlab::Issue to our GitlabIssue
    // This requires careful mapping. Assuming gitlab::Issue has similar fields.
    // And that gitlab::UserBasic can be mapped to GitlabUser.
    let author = GitlabUser {
        id: upstream_issue.author.id.value(),
        username: upstream_issue.author.username.clone(),
        name: upstream_issue.author.name.clone(),
        avatar_url: upstream_issue.author.avatar_url.map(|u| u.to_string()),
    };

    Ok(GitlabIssue {
        id: upstream_issue.id.value(),
        iid: upstream_issue.iid.value(),
        project_id: upstream_issue.project_id.value(),
        title: upstream_issue.title.clone(),
        description: upstream_issue.description.clone(),
        state: upstream_issue.state.to_string(), // Assuming state is an enum in gitlab::Issue
        author,
        web_url: upstream_issue.web_url.clone(),
        labels: upstream_issue.labels.clone(),
        updated_at: upstream_issue
            .updated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default(), // Handle Option<DateTime<Utc>>
    })
}

pub async fn get_project_by_path(
    client: &Gitlab,
    repo_path: &str, // e.g., "group/project"
) -> Result<GitlabProject, GitlabError> {
    let endpoint = gitlab::api::projects::Project::builder()
        .project(repo_path) // The builder accepts path as &str
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_project: gitlab::Project =
        gitlab::api::AsyncQuery::query_async(endpoint, client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    Ok(GitlabProject {
        id: upstream_project.id.value() as i64, // Our model uses i64, gitlab crate uses u64
        path_with_namespace: upstream_project.path_with_namespace.clone(),
        web_url: upstream_project.web_url.clone(),
    })
}

pub async fn post_comment_to_merge_request(
    client: &Gitlab,
    project_id: u64,
    mr_iid: u64, // This is the IID of the merge request
    comment_body: &str,
) -> Result<GitlabNoteAttributes, GitlabError> {
    let endpoint = gitlab::api::projects::merge_requests::notes::CreateMergeRequestNote::builder()
        .project(project_id)
        .merge_request(mr_iid) // The builder uses 'merge_request' for the MR IID
        .body(comment_body)
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_note: gitlab::Note = gitlab::api::AsyncQuery::query_async(endpoint, client)
        .await
        .map_err(GitlabError::GitlabClient)?;

    let author = GitlabUser {
        id: upstream_note.author.id.value(),
        username: upstream_note.author.username.clone(),
        name: upstream_note.author.name.clone(),
        avatar_url: upstream_note.author.avatar_url.map(|u| u.to_string()),
    };

    Ok(GitlabNoteAttributes {
        id: upstream_note.id.value(),
        note: upstream_note.body.clone(),
        author,
        project_id: upstream_note.project_id.map(|p| p.value()).unwrap_or(project_id),
        noteable_type: "MergeRequest".to_string(),
        noteable_id: Some(upstream_note.noteable_id.map(|n| n.value() as i64).unwrap_or(0)),
        iid: Some(mr_iid as i64), // The IID of the MR the note is on
        url: None, // Typically not returned directly by create note API
        updated_at: upstream_note
            .updated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| {
                upstream_note
                    .created_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
    })
}

pub async fn post_comment_to_issue(
    client: &Gitlab,
    project_id: u64,
    issue_iid: u64, // This is the IID of the issue itself
    comment_body: &str,
) -> Result<GitlabNoteAttributes, GitlabError> {
    let endpoint = gitlab::api::projects::issues::notes::CreateIssueNote::builder()
        .project(project_id)
        .issue(issue_iid) // The builder uses 'issue' for the issue IID
        .body(comment_body)
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_note: gitlab::Note = gitlab::api::AsyncQuery::query_async(endpoint, client)
        .await
        .map_err(GitlabError::GitlabClient)?;

    let author = GitlabUser {
        id: upstream_note.author.id.value(),
        username: upstream_note.author.username.clone(),
        name: upstream_note.author.name.clone(),
        avatar_url: upstream_note.author.avatar_url.map(|u| u.to_string()),
    };

    Ok(GitlabNoteAttributes {
        id: upstream_note.id.value(),
        note: upstream_note.body.clone(),
        author,
        project_id: upstream_note.project_id.map(|p| p.value()).unwrap_or(project_id), // project_id might not be on note directly, fall back
        noteable_type: "Issue".to_string(),
        noteable_id: Some(upstream_note.noteable_id.map(|n| n.value() as i64).unwrap_or(0)), // Map noteable_id if available
        iid: Some(issue_iid as i64), // The IID of the issue the note is on
        url: None, // GitLab API for creating notes might not return a direct URL to the note itself in this response
        updated_at: upstream_note
            .updated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| {
                upstream_note
                    .created_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
    })
}

pub async fn get_merge_request(
    client: &Gitlab,
    project_id: u64,
    mr_iid: u64,
) -> Result<GitlabMergeRequest, GitlabError> {
    let endpoint = gitlab::api::projects::merge_requests::MergeRequest::builder()
        .project(project_id)
        .merge_request(mr_iid)
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_mr: gitlab::MergeRequest =
        gitlab::api::AsyncQuery::query_async(endpoint, client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let author = GitlabUser {
        id: upstream_mr.author.id.value(),
        username: upstream_mr.author.username.clone(),
        name: upstream_mr.author.name.clone(),
        avatar_url: upstream_mr.author.avatar_url.map(|u| u.to_string()),
    };

    Ok(GitlabMergeRequest {
        id: upstream_mr.id.value(),
        iid: upstream_mr.iid.value(),
        project_id: upstream_mr.project_id.value(),
        title: upstream_mr.title.clone(),
        description: upstream_mr.description.clone(),
        state: upstream_mr.state.to_string(), // Assuming state is an enum
        author,
        source_branch: upstream_mr.source_branch.clone(),
        target_branch: upstream_mr.target_branch.clone(),
        web_url: upstream_mr.web_url.clone(),
        labels: upstream_mr.labels.clone(),
        detailed_merge_status: upstream_mr.detailed_merge_status.map(|s| s.to_string()), // Or merge_status
        updated_at: upstream_mr
            .updated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default(),
    })
}

pub async fn get_issues(
    client: &Gitlab,
    project_id: u64,
    since_timestamp: u64,
) -> Result<Vec<GitlabIssue>, GitlabError> {
    let updated_after_datetime = Utc
        .timestamp_opt(since_timestamp as i64, 0)
        .single()
        .ok_or_else(|| {
            // Create a generic error for invalid timestamp.
            // This could be a new GitlabError variant if more detailed error handling is needed.
            GitlabError::Api {
                status: StatusCode::BAD_REQUEST, // Or some internal error status
                body: "Invalid 'since_timestamp' provided".to_string(),
            }
        })?;

    let endpoint = gitlab::api::projects::issues::Issues::builder()
        .project(project_id)
        .updated_after(updated_after_datetime)
        .sort(gitlab::api::issues::IssueSort::Asc) // Ensure this matches desired sort order
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_issues: Vec<gitlab::Issue> =
        gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let issues = upstream_issues
        .into_iter()
        .map(|upstream_issue| {
            let author = GitlabUser {
                id: upstream_issue.author.id.value(),
                username: upstream_issue.author.username.clone(),
                name: upstream_issue.author.name.clone(),
                avatar_url: upstream_issue.author.avatar_url.map(|u| u.to_string()),
            };
            GitlabIssue {
                id: upstream_issue.id.value(),
                iid: upstream_issue.iid.value(),
                project_id: upstream_issue.project_id.value(),
                title: upstream_issue.title.clone(),
                description: upstream_issue.description.clone(),
                state: upstream_issue.state.to_string(),
                author,
                web_url: upstream_issue.web_url.clone(),
                labels: upstream_issue.labels.clone(),
                updated_at: upstream_issue
                    .updated_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            }
        })
        .collect();

    Ok(issues)
}

pub async fn get_merge_requests(
    client: &Gitlab,
    project_id: u64,
    since_timestamp: u64,
) -> Result<Vec<GitlabMergeRequest>, GitlabError> {
    let updated_after_datetime = Utc
        .timestamp_opt(since_timestamp as i64, 0)
        .single()
        .ok_or_else(|| GitlabError::Api {
            status: StatusCode::BAD_REQUEST,
            body: "Invalid 'since_timestamp' provided".to_string(),
        })?;

    let endpoint = gitlab::api::projects::merge_requests::MergeRequests::builder()
        .project(project_id)
        .updated_after(updated_after_datetime)
        .sort(gitlab::api::merge_requests::MergeRequestSort::Asc) // Sort by updated_at ascending
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_mrs: Vec<gitlab::MergeRequest> =
        gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let mrs = upstream_mrs
        .into_iter()
        .map(|upstream_mr| {
            let author = GitlabUser {
                id: upstream_mr.author.id.value(),
                username: upstream_mr.author.username.clone(),
                name: upstream_mr.author.name.clone(),
                avatar_url: upstream_mr.author.avatar_url.map(|u| u.to_string()),
            };
            GitlabMergeRequest {
                id: upstream_mr.id.value(),
                iid: upstream_mr.iid.value(),
                project_id: upstream_mr.project_id.value(),
                title: upstream_mr.title.clone(),
                description: upstream_mr.description.clone(),
                state: upstream_mr.state.to_string(),
                author,
                source_branch: upstream_mr.source_branch.clone(),
                target_branch: upstream_mr.target_branch.clone(),
                web_url: upstream_mr.web_url.clone(),
                labels: upstream_mr.labels.clone(),
                detailed_merge_status: upstream_mr
                    .detailed_merge_status
                    .map(|s| s.to_string()),
                updated_at: upstream_mr
                    .updated_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            }
        })
        .collect();

    Ok(mrs)
}

pub async fn get_issue_notes(
    client: &Gitlab,
    project_id: u64,
    issue_iid: u64,
    since_timestamp: u64,
) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
    let created_after_datetime = Utc
        .timestamp_opt(since_timestamp as i64, 0)
        .single()
        .ok_or_else(|| GitlabError::Api {
            status: StatusCode::BAD_REQUEST,
            body: "Invalid 'since_timestamp' provided".to_string(),
        })?;

    let endpoint = gitlab::api::projects::issues::notes::IssueNotes::builder()
        .project(project_id)
        .issue(issue_iid)
        .created_after(created_after_datetime)
        .sort(gitlab::api::notes::NoteSort::Asc) // Sort by created_at ascending
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_notes: Vec<gitlab::Note> =
        gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let notes = upstream_notes
        .into_iter()
        .map(|upstream_note| {
            let author = GitlabUser {
                id: upstream_note.author.id.value(),
                username: upstream_note.author.username.clone(),
                name: upstream_note.author.name.clone(),
                avatar_url: upstream_note.author.avatar_url.map(|u| u.to_string()),
            };
            GitlabNoteAttributes {
                id: upstream_note.id.value(),
                note: upstream_note.body.clone(),
                author,
                project_id: upstream_note.project_id.map(|p| p.value()).unwrap_or(project_id),
                noteable_type: "Issue".to_string(),
                noteable_id: Some(upstream_note.noteable_id.map(|n| n.value() as i64).unwrap_or(0)),
                iid: Some(issue_iid as i64), // The IID of the issue the note is on
                url: None,                   // Typically not returned directly
                updated_at: upstream_note
                    .updated_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| {
                        upstream_note
                            .created_at
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
            }
        })
        .collect();

    Ok(notes)
}

pub async fn get_merge_request_notes(
    client: &Gitlab,
    project_id: u64,
    mr_iid: u64,
    since_timestamp: u64,
) -> Result<Vec<GitlabNoteAttributes>, GitlabError> {
    let created_after_datetime = Utc
        .timestamp_opt(since_timestamp as i64, 0)
        .single()
        .ok_or_else(|| GitlabError::Api {
            status: StatusCode::BAD_REQUEST,
            body: "Invalid 'since_timestamp' provided".to_string(),
        })?;

    let endpoint = gitlab::api::projects::merge_requests::notes::MergeRequestNotes::builder()
        .project(project_id)
        .merge_request(mr_iid)
        .created_after(created_after_datetime)
        .sort(gitlab::api::notes::NoteSort::Asc) // Sort by created_at ascending
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_notes: Vec<gitlab::Note> =
        gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let notes = upstream_notes
        .into_iter()
        .map(|upstream_note| {
            let author = GitlabUser {
                id: upstream_note.author.id.value(),
                username: upstream_note.author.username.clone(),
                name: upstream_note.author.name.clone(),
                avatar_url: upstream_note.author.avatar_url.map(|u| u.to_string()),
            };
            GitlabNoteAttributes {
                id: upstream_note.id.value(),
                note: upstream_note.body.clone(),
                author,
                project_id: upstream_note.project_id.map(|p| p.value()).unwrap_or(project_id),
                noteable_type: "MergeRequest".to_string(),
                noteable_id: Some(upstream_note.noteable_id.map(|n| n.value() as i64).unwrap_or(0)),
                iid: Some(mr_iid as i64), // The IID of the MR the note is on
                url: None,                // Typically not returned directly
                updated_at: upstream_note
                    .updated_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| {
                        upstream_note
                            .created_at
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
            }
        })
        .collect();

    Ok(notes)
}

pub async fn get_repository_tree(
    client: &Gitlab,
    project_id: u64,
) -> Result<Vec<String>, GitlabError> {
    let endpoint = gitlab::api::projects::repository::Tree::builder()
        .project(project_id)
        .recursive(true) // Ensure recursive listing
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let tree_nodes: Vec<gitlab::TreeNode> =
        gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let file_paths = tree_nodes
        .into_iter()
        .filter(|node| node.node_type == gitlab::TreeNodeType::Blob)
        .map(|node| node.path)
        .collect();

    Ok(file_paths)
}

pub async fn search_files_by_content(
    client: &Gitlab,
    project_id: u64,
    query: &str,
    git_ref: &str,
) -> Result<Vec<String>, GitlabError> {
    let endpoint = gitlab::api::projects::search::ProjectSearch::builder()
        .project(project_id)
        .scope(gitlab::api::search::SearchScope::Blobs)
        .search(query)
        .ref_(git_ref)
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    // The ProjectSearch endpoint returns Vec<SearchResult>.
    // For content searches, SearchResult contains `data` (the content snippet) and `filename`.
    let search_results: Vec<gitlab::SearchResult> =
        gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let file_paths = search_results
        .into_iter()
        .map(|result| result.filename) // Extract filename for blobs
        .collect();

    Ok(file_paths)
}

pub async fn get_file_content(
    client: &Gitlab,
    project_id: u64,
    file_path: &str,
    git_ref: &str,
) -> Result<GitlabFile, GitlabError> {
    let endpoint = gitlab::api::projects::repository::files::File::builder()
        .project(project_id)
        .file_path(file_path)
        .ref_(git_ref)
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    let upstream_file_info: gitlab::File =
        gitlab::api::AsyncQuery::query_async(endpoint, client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let mut decoded_content: Option<String> = None;
    if let Some(content_str) = upstream_file_info.content {
        if upstream_file_info.encoding.as_deref() == Some("base64") {
            let bytes = base64::decode(&content_str)
                .map_err(|e| GitlabError::ContentDecoding(format!("Base64 decode error: {}", e)))?;
            decoded_content = Some(String::from_utf8(bytes).map_err(|e| {
                GitlabError::ContentDecoding(format!("UTF-8 conversion error: {}", e))
            })?);
        } else {
            // If not base64, assume it's plain text or content is not meant to be decoded here.
            // The original code didn't handle other encodings, so we'll keep it simple.
            decoded_content = Some(content_str);
        }
    }

    Ok(GitlabFile {
        file_path: upstream_file_info.file_path,
        size: upstream_file_info.size as usize, // Assuming size is u64 in gitlab::File
        encoding: upstream_file_info.encoding,
        content: decoded_content,
    })
}

pub async fn search_files_by_name(
    client: &Gitlab,
    project_id: u64,
    query: &str,
    git_ref: &str,
) -> Result<Vec<String>, GitlabError> {
    let endpoint = gitlab::api::projects::search::ProjectSearch::builder()
        .project(project_id)
        .scope(gitlab::api::search::SearchScope::Blobs)
        .search(query)
        .ref_(git_ref) // Use ref_ for the branch/tag/commit
        .build()
        .map_err(|e| GitlabError::GitlabClient(gitlab::api::ApiError::builder(e)))?;

    // The ProjectSearch endpoint returns Vec<SearchResult>, where SearchResult has a `filename` field for blobs.
    let search_results: Vec<gitlab::SearchResult> =
        gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(client)
            .await
            .map_err(GitlabError::GitlabClient)?;

    let file_paths = search_results
        .into_iter()
        .map(|result| result.filename) // SearchResult struct has `filename` for blobs
        .collect();

    Ok(file_paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::models::GitlabUser; // Import GitlabUser for mapping
    use serde_json::json; // Keep for potential mock responses if http_mock is used later

    // Helper to create AppSettings for tests
    fn create_test_settings(base_url: String) -> AppSettings {
        AppSettings {
            gitlab_url: base_url,
            gitlab_token: "test_token".to_string(),
            openai_api_key: "key".to_string(),
            openai_custom_url: "url".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: None,
        }
    }

    #[tokio::test]
    async fn test_new_gitlab_client_valid_url() {
        let settings = create_test_settings("http://localhost:1234".to_string()); // mockito server url
        let client_result = new(&settings);
        assert!(client_result.is_ok());
    }

    #[tokio::test]
    async fn test_new_gitlab_client_invalid_host() {
        // Test with a host that is unlikely to be resolvable or connectable quickly.
        // The `gitlab::Gitlab::new` itself doesn't perform network requests,
        // so it won't error here. Errors would occur on actual API calls.
        // This test as written for `new` might not be very useful for invalid URLs
        // unless the URL is so malformed that parsing host/token fails.
        let settings = create_test_settings("http://nonexistent-domain-for-testing:12345".to_string());
        let client_result = new(&settings);
        // `Gitlab::new` primarily checks if the URL can act as a base and if token is provided.
        // It doesn't validate connectivity.
        assert!(client_result.is_ok()); // Expect Ok as it's just client setup
    }

    #[tokio::test]
    async fn test_get_issue_error_mock_server() {
        // This test simulates calling get_issue against a mock server that isn't actually GitLab.
        // The purpose is to ensure our get_issue function correctly calls the gitlab client
        // and propagates errors from it. We expect a GitlabClient error because the
        // underlying client will fail to connect or get a valid response.
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url(); // Use mockito server's URL

        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();

        // No mock is set up on the server for the specific API path.
        // The gitlab crate's client will attempt a connection, which might fail or return an unexpected response.
        // This should result in an error from the gitlab crate, wrapped in our GitlabError::GitlabClient.

        let result = get_issue(&client, 1, 101).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                // We can't easily assert specific details of api_error without knowing
                // exactly how the gitlab crate's reqwest client would fail against a
                // generic mockito server endpoint that isn't configured for this call.
                // It could be a connection error, an unexpected response (e.g., 404 or 501 if mockito defaults to that),
                // or a deserialization error if it gets HTML/plain text.
                // The key is that it's a GitlabClient error.
                println!("Received expected GitlabClient error: {:?}", api_error);
            }
            other_error => panic!("Expected GitlabError::GitlabClient, got {:?}", other_error),
        }
    }

    // Note: Testing a successful `get_issue` call that correctly maps fields
    // would require a more sophisticated mocking setup, likely using `httpmock`
    // as the `gitlab` crate does for its own tests, or by defining a trait
    // for the GitLab client operations and mocking that trait.
    // The current test `test_get_issue_error_mock_server` primarily checks
    // the error path and integration with the `gitlab::Gitlab` client.

    #[tokio::test]
    async fn test_get_merge_request_error_mock_server() {
        // Similar to test_get_issue_error_mock_server, this test checks the error path.
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();

        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();

        // No mock is set up for the specific MR API path.
        // Expect a GitlabClient error.
        let result = get_merge_request(&client, 1, 5).await; // project_id=1, mr_iid=5

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for MR: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for MR, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_post_comment_to_issue_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();
        let comment_body = "This is a test comment on an issue.";

        // No mock is set up for the POST request to create a note.
        // Expect a GitlabClient error.
        let result = post_comment_to_issue(&client, 1, 101, comment_body).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for post_comment_to_issue: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for post_comment_to_issue, got {:?}",
                other_error
            ),
        }
    }

    // Testing the success case for post_comment_to_issue correctly is complex
    // without httpmock, as it requires matching the POST body and returning a
    // valid GitLab Note JSON response. The error case test ensures the function
    // correctly uses the client and propagates errors.

    #[tokio::test]
    async fn test_post_comment_to_merge_request_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();
        let comment_body = "This is a test comment on an MR.";

        // No mock is set up for the POST request. Expect a GitlabClient error.
        let result = post_comment_to_merge_request(&client, 1, 5, comment_body).await; // project_id=1, mr_iid=5

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for post_comment_to_merge_request: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for post_comment_to_merge_request, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_project_by_path_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();
        let repo_path = "group/project-does-not-exist";

        // No mock is set up for this specific project path. Expect a GitlabClient error.
        let result = get_project_by_path(&client, repo_path).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for get_project_by_path: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for get_project_by_path, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_issues_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();
        let since_timestamp = 1620000000; // Example timestamp

        // No mock is set up for the issues list. Expect a GitlabClient error.
        let result = get_issues(&client, 1, since_timestamp).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for get_issues: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for get_issues, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_issues_invalid_timestamp() {
        let settings = create_test_settings("http://localhost:1234".to_string()); // URL doesn't matter here
        let client = new(&settings).unwrap();
        // An extremely large timestamp that is likely out of chrono's valid range
        let since_timestamp = u64::MAX / 1000; // Avoid overflow if it's multiplied by 1000 for ms

        let result = get_issues(&client, 1, since_timestamp).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::Api { status, body } => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert_eq!(body, "Invalid 'since_timestamp' provided");
            }
            other_error => panic!("Expected GitlabError::Api for invalid timestamp, got {:?}", other_error),
        }
    }

    #[tokio::test]
    async fn test_get_merge_requests_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();
        let since_timestamp = 1620000000; // Example timestamp

        // No mock is set up. Expect a GitlabClient error.
        let result = get_merge_requests(&client, 1, since_timestamp).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for get_merge_requests: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for get_merge_requests, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_merge_requests_invalid_timestamp() {
        let settings = create_test_settings("http://localhost:1234".to_string());
        let client = new(&settings).unwrap();
        let since_timestamp = u64::MAX / 1000;

        let result = get_merge_requests(&client, 1, since_timestamp).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::Api { status, body } => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert_eq!(body, "Invalid 'since_timestamp' provided");
            }
            other_error => panic!(
                "Expected GitlabError::Api for invalid timestamp in get_merge_requests, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_issue_notes_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();
        let since_timestamp = 1620000000; // Example timestamp

        // No mock is set up. Expect a GitlabClient error.
        let result = get_issue_notes(&client, 1, 101, since_timestamp).await; // project_id=1, issue_iid=101

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for get_issue_notes: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for get_issue_notes, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_issue_notes_invalid_timestamp() {
        let settings = create_test_settings("http://localhost:1234".to_string());
        let client = new(&settings).unwrap();
        let since_timestamp = u64::MAX / 1000;

        let result = get_issue_notes(&client, 1, 101, since_timestamp).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::Api { status, body } => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert_eq!(body, "Invalid 'since_timestamp' provided");
            }
            other_error => panic!(
                "Expected GitlabError::Api for invalid timestamp in get_issue_notes, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_merge_request_notes_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();
        let since_timestamp = 1620000000; // Example timestamp

        // No mock is set up. Expect a GitlabClient error.
        let result = get_merge_request_notes(&client, 1, 5, since_timestamp).await; // project_id=1, mr_iid=5

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for get_merge_request_notes: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for get_merge_request_notes, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_merge_request_notes_invalid_timestamp() {
        let settings = create_test_settings("http://localhost:1234".to_string());
        let client = new(&settings).unwrap();
        let since_timestamp = u64::MAX / 1000;

        let result = get_merge_request_notes(&client, 1, 5, since_timestamp).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::Api { status, body } => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert_eq!(body, "Invalid 'since_timestamp' provided");
            }
            other_error => panic!(
                "Expected GitlabError::Api for invalid timestamp in get_merge_request_notes, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_repository_tree_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();

        // No mock is set up. Expect a GitlabClient error.
        let result = get_repository_tree(&client, 1).await; // project_id=1

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for get_repository_tree: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for get_repository_tree, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_file_content_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();

        // No mock is set up. Expect a GitlabClient error.
        let result = get_file_content(&client, 1, "src/main.rs", "main").await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for get_file_content: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for get_file_content, got {:?}",
                other_error
            ),
        }
    }

    #[tokio::test]
    async fn test_get_file_content_decoding_error() {
        // This test doesn't use a mock server. It tests the decoding logic directly
        // by constructing a gitlab::File-like object (if we could) or by
        // recognizing that testing this part properly requires a successful API call first.
        // Since we can't easily mock a successful API call that returns specific base64 content
        // with mockito for the gitlab crate, we'll note this limitation.
        // The function is structured to use base64::decode and String::from_utf8,
        // and errors from these are mapped to GitlabError::ContentDecoding.
        // A more direct unit test for this would involve creating a `gitlab::File` instance
        // with invalid base64 or non-UTF8 decoded content, which is hard without the actual struct.

        // For now, this test serves as a placeholder to acknowledge the decoding error path.
        // If `get_file_content` were to take a `gitlab::File` as input for the decoding part,
        // that part could be unit tested independently.
        // As it is, an integration test or using `httpmock` would be needed.
        let error = GitlabError::ContentDecoding("Test decoding error".to_string());
        assert_eq!(
            format!("{}", error),
            "Content decoding error: Test decoding error"
        );
    }

    #[tokio::test]
    async fn test_search_files_by_name_error_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = new(&settings).unwrap();

        // No mock is set up. Expect a GitlabClient error.
        let result = search_files_by_name(&client, 1, "Cargo.toml", "main").await;

        assert!(result.is_err());
        match result.err().unwrap() {
            GitlabError::GitlabClient(api_error) => {
                println!(
                    "Received expected GitlabClient error for search_files_by_name: {:?}",
                    api_error
                );
            }
            other_error => panic!(
                "Expected GitlabError::GitlabClient for search_files_by_name, got {:?}",
                other_error
            ),
        }
    }
}
