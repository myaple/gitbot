use crate::config::AppSettings;
use crate::gitlab::GitlabApiClient;
use crate::gitlab::GitlabError;
use crate::models::{GitlabIssue, GitlabMergeRequest, GitlabProject};

use anyhow::Result;
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, info, warn};

const MAX_SOURCE_FILES: usize = 250; // Maximum number of source files to include in context
const AGENTS_MD_FILE: &str = "AGENTS.md";

#[derive(Debug, Deserialize)]
pub struct GitlabFile {
    // pub file_name: String, // Removed unused field
    #[allow(dead_code)] // Used in tests
    pub file_path: String,
    pub size: usize,
    pub content: Option<String>,
    pub encoding: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitlabDiff {
    // pub old_path: String, // Removed unused field
    pub new_path: String,
    pub diff: String,
}

pub struct RepoContextExtractor {
    gitlab_client: Arc<GitlabApiClient>,
    settings: Arc<AppSettings>,
}

impl RepoContextExtractor {
    pub fn new(gitlab_client: Arc<GitlabApiClient>, settings: Arc<AppSettings>) -> Self {
        Self {
            gitlab_client,
            settings,
        }
    }

    async fn get_file_content_from_project(
        &self,
        project_id: i64,
        file_path: &str,
    ) -> Result<Option<String>> {
        match self
            .gitlab_client
            .get_file_content(project_id, file_path)
            .await
        {
            Ok(file) => Ok(file.content),
            Err(GitlabError::Api { status, .. }) if status == reqwest::StatusCode::NOT_FOUND => {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn get_agents_md_content(
        &self,
        project: &GitlabProject,
        context_repo_path: Option<&str>,
    ) -> Result<Option<String>> {
        // Try fetching from the main project first
        match self
            .get_file_content_from_project(project.id, AGENTS_MD_FILE)
            .await
        {
            Ok(Some(content)) => {
                info!(
                    "Found AGENTS.md in main project {}",
                    project.path_with_namespace
                );
                return Ok(Some(content));
            }
            Ok(None) => {
                // File not found in main project, proceed to context repo
                debug!(
                    "AGENTS.md not found in main project {}, trying context repo if available.",
                    project.path_with_namespace
                );
            }
            Err(e) => {
                warn!(
                    "Failed to fetch AGENTS.MD from main project {}: {}. Will try context repo if available.",
                    project.path_with_namespace, e
                );
                // Do not return here; proceed to try context_repo_path
            }
        }

        // If not found in main project (or an error occurred there) and context repo is specified, try fetching from context repo
        if let Some(context_path) = context_repo_path {
            match self.gitlab_client.get_project_by_path(context_path).await {
                Ok(context_project) => {
                    // The `?` here is acceptable as per requirements: if fetching context project's AGENTS.MD fails critically,
                    // the whole operation should return Err. If it's just not found (Ok(None)), that's fine.
                    match self
                        .get_file_content_from_project(context_project.id, AGENTS_MD_FILE)
                        .await
                    {
                        Ok(Some(content)) => {
                            info!("Found AGENTS.md in context project {}", context_path);
                            return Ok(Some(content));
                        }
                        Ok(None) => {
                            // File not found in context project either
                            debug!("AGENTS.md not found in context project {}.", context_path);
                        }
                        Err(e) => {
                            // Critical error fetching from context project
                            warn!(
                                "Failed to fetch AGENTS.MD from context project {}: {}",
                                context_path, e
                            );
                            return Err(e); // Return the error
                        }
                    }
                }
                Err(e) => {
                    // This error means the context_repo_path itself is problematic.
                    // Log a warning and proceed to return Ok(None) as the file wasn't found.
                    warn!(
                        "Failed to get context repo project {}: {}. AGENTS.md cannot be fetched from it.",
                        context_path, e
                    );
                }
            }
        }

        // If we reach here, AGENTS.md was not found in either the main project or the context repo (or context repo was not specified/accessible)
        info!(
            "AGENTS.md not found in main project {} nor in context repo {:?}",
            project.path_with_namespace, context_repo_path
        );
        Ok(None)
    }

    /// Get all source code files in the repository, up to MAX_SOURCE_FILES limit
    async fn get_all_source_files(&self, project_id: i64) -> Result<Vec<String>> {
        let files = self.gitlab_client.get_repository_tree(project_id).await?;

        // Filter for source code files
        let source_files: Vec<String> = files
            .into_iter()
            .filter(|path| {
                let extension = path.split('.').next_back().unwrap_or("");
                matches!(
                    extension,
                    "rs" | "py"
                        | "js"
                        | "ts"
                        | "java"
                        | "c"
                        | "cpp"
                        | "h"
                        | "hpp"
                        | "go"
                        | "rb"
                        | "php"
                        | "cs"
                        | "scala"
                        | "kt"
                        | "swift"
                        | "sh"
                        | "jsx"
                        | "tsx"
                        | "vue"
                        | "svelte"
                )
            })
            .collect();

        Ok(source_files)
    }

    async fn get_combined_source_files(
        &self,
        project: &GitlabProject,
        context_repo_path: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut main_project_fetch_error: Option<anyhow::Error> = None;

        // Get source files from the main project
        let mut all_files = match self.get_all_source_files(project.id).await {
            Ok(files) => files,
            Err(e) => {
                warn!(
                    "Failed to get source files from main project {}: {}. Will attempt to use context repo files if available.",
                    project.path_with_namespace, e
                );
                main_project_fetch_error = Some(e);
                Vec::new() // Initialize with an empty vec and continue
            }
        };

        // If context repo is specified, get its files too
        if let Some(context_path) = context_repo_path {
            match self.gitlab_client.get_project_by_path(context_path).await {
                Ok(context_project) => match self.get_all_source_files(context_project.id).await {
                    Ok(context_files) => {
                        all_files.extend(context_files);
                    }
                    Err(e) => {
                        warn!(
                            "Failed to get source files from context repo {}: {}",
                            context_path, e
                        );
                    }
                },
                Err(e) => {
                    warn!("Failed to get context repo {}: {}", context_path, e);
                }
            }
        }

        // Limit the total combined files
        all_files.truncate(MAX_SOURCE_FILES);

        // Final return logic
        if !all_files.is_empty() {
            Ok(all_files)
        } else {
            // all_files is empty here
            if let Some(main_err) = main_project_fetch_error {
                // Main project fetch failed. If context repo also failed or wasn't specified,
                // or if context repo succeeded but had no files, this original error should be returned.
                Err(main_err)
            } else {
                // Main project fetch succeeded but returned no files,
                // and context repo (if specified) also returned no files or failed non-critically.
                Ok(Vec::new())
            }
        }
    }

    /// Extract relevant context from a repository for an issue
    pub async fn extract_context_for_issue(
        &self,
        issue: &GitlabIssue,
        project: &GitlabProject,
        context_repo_path: Option<&str>,
    ) -> Result<String> {
        info!(
            "Extracting context for issue #{} from main project {} and context repo {:?}",
            issue.iid, project.path_with_namespace, context_repo_path
        );

        // Format the context
        let mut context = String::new();
        let mut total_size = 0;
        let mut has_any_content = false;

        // First add the list of all source files from both projects
        let source_files = match self
            .get_combined_source_files(project, context_repo_path)
            .await
        {
            Ok(files) => {
                has_any_content = true;
                files
            }
            Err(e) => {
                warn!(
                    "Failed to get combined source files for issue #{}: {}. Continuing with other context.",
                    issue.iid, e
                );
                Vec::new()
            }
        };

        if !source_files.is_empty() {
            let files_list = format!(
                "\n--- All Source Files (up to {} files) ---\n{}\n",
                MAX_SOURCE_FILES,
                source_files.join("\n")
            );
            context.push_str(&files_list);
            total_size += files_list.len();
        }

        // Add AGENTS.md content if available
        match self.get_agents_md_content(project, context_repo_path).await {
            Ok(Some(agents_md)) => {
                has_any_content = true;
                let agents_md_context = format!("\n--- {} ---\n{}\n", AGENTS_MD_FILE, agents_md);
                if total_size + agents_md_context.len() <= self.settings.max_context_size {
                    context.push_str(&agents_md_context);
                } else {
                    warn!(
                        "AGENTS.md content too large to fit in context for issue #{}",
                        issue.iid
                    );
                    context.push_str(&format!(
                        "\n--- {} ---\n[Content omitted due to context size limits]\n",
                        AGENTS_MD_FILE
                    ));
                }
            }
            Ok(None) => {
                // AGENTS.md not found, do nothing
                debug!("AGENTS.md not found for issue #{}", issue.iid);
            }
            Err(e) => {
                warn!(
                    "Failed to fetch AGENTS.md for issue #{}: {}. Continuing with other context.",
                    issue.iid, e
                );
            }
        }

        // No longer searching for relevant files
        debug!("Skipping relevant files search for issue #{}", issue.iid);

        if !has_any_content {
            context = "No source files or relevant files found in the repository.".to_string();
        } else if context.is_empty() {
            context =
                "Context gathering completed but no content was added due to size constraints."
                    .to_string();
        }

        Ok(context)
    }

    /// Extract diff context for a merge request
    pub async fn extract_context_for_mr(
        &self,
        mr: &GitlabMergeRequest,
        project: &GitlabProject,
        context_repo_path: Option<&str>,
    ) -> Result<(String, String)> {
        info!(
            "Extracting diff context for MR !{} in {} and context repo {:?}",
            mr.iid, project.path_with_namespace, context_repo_path
        );

        let mut context_for_llm = String::new();
        let mut context_for_comment = String::new();
        let mut total_size = 0;
        let mut has_any_content = false;

        // First add the list of all source files
        let source_files = match self
            .get_combined_source_files(project, context_repo_path)
            .await
        {
            Ok(files) => {
                has_any_content = true;
                files
            }
            Err(e) => {
                warn!(
                    "Failed to get combined source files for MR !{}: {}. Continuing with other context.",
                    mr.iid, e
                );
                Vec::new()
            }
        };

        if !source_files.is_empty() {
            let files_list = format!(
                "\n--- All Source Files (up to {} files) ---\n{}\n",
                MAX_SOURCE_FILES,
                source_files.join("\n")
            );
            context_for_llm.push_str(&files_list);
            total_size += files_list.len();
        }

        // Add AGENTS.md content if available
        match self.get_agents_md_content(project, context_repo_path).await {
            Ok(Some(agents_md)) => {
                has_any_content = true;
                let agents_md_context = format!("\n--- {} ---\n{}\n", AGENTS_MD_FILE, agents_md);
                if total_size + agents_md_context.len() <= self.settings.max_context_size {
                    context_for_llm.push_str(&agents_md_context);
                    total_size += agents_md_context.len();
                } else {
                    warn!(
                        "AGENTS.md content too large to fit in context for MR !{}",
                        mr.iid
                    );
                    context_for_llm.push_str(&format!(
                        "\n--- {} ---\n[Content omitted due to context size limits]\n",
                        AGENTS_MD_FILE
                    ));
                }
            }
            Ok(None) => {
                // AGENTS.md not found, do nothing
                debug!("AGENTS.md not found for MR !{}", mr.iid);
            }
            Err(e) => {
                warn!(
                    "Failed to fetch AGENTS.md for MR !{}: {}. Continuing with other context.",
                    mr.iid, e
                );
            }
        }

        // Add pipeline status information
        let pipeline_status_context = if let Some(pipeline) = &mr.head_pipeline {
            format!(
                "\n--- Latest Pipeline Status ---\n        Status: {}\n        URL: {}\n        Source: {}\n        Ref: {}\n        SHA: {}\n        Created At: {}\n        Updated At: {}\n---",
                pipeline.status,
                pipeline.web_url,
                pipeline.source.as_deref().unwrap_or("N/A"),
                pipeline.ref_,
                pipeline.sha,
                pipeline.created_at,
                pipeline.updated_at
            )
        } else {
            "\n--- Latest Pipeline Status ---\nNo pipeline information available for this merge request.\n---".to_string()
        };

        if total_size + pipeline_status_context.len() <= self.settings.max_context_size {
            context_for_llm.push_str(&pipeline_status_context);
            total_size += pipeline_status_context.len();
        } else {
            warn!(
                "Pipeline status too large to fit in context for MR !{}",
                mr.iid
            );
            context_for_llm.push_str("\n--- Latest Pipeline Status ---\n[Pipeline status omitted due to context size limits]\n---");
            // We don't add the size of the omission message to total_size,
            // as it's a fixed small string replacing potentially larger content.
            // Or, if precise accounting is needed:
            // total_size += "\n--- Latest Pipeline Status ---\n[Pipeline status omitted due to context size limits]\n---".len();
        }

        // Then add the diff context and file history
        let diffs = match self
            .gitlab_client
            .get_merge_request_changes(project.id, mr.iid)
            .await
        {
            Ok(diffs) => {
                has_any_content = true;
                diffs
            }
            Err(e) => {
                warn!(
                    "Failed to get merge request changes for MR !{}: {}. Continuing with other context.",
                    mr.iid, e
                );
                Vec::new()
            }
        };

        for diff in diffs {
            let mut file_context =
                format!("\n--- Changes in {} ---\n{}\n", diff.new_path, diff.diff);

            // Get commit history for this file
            match self
                .gitlab_client
                .get_file_commits(project.id, &diff.new_path, Some(5))
                .await
            {
                Ok(commits) => {
                    // Add commit history to LLM context
                    file_context.push_str("\n--- Recent Commit History ---\n");
                    for commit in commits.iter() {
                        file_context.push_str(&format!(
                            "* {} ({}) - {}\n  {}\n",
                            commit.short_id, commit.authored_date, commit.author_name, commit.title
                        ));
                    }
                    file_context.push('\n');

                    // Add commit history to comment in a more user-friendly format with hyperlinks
                    context_for_comment
                        .push_str(&format!("\n### Recent commits for `{}`:\n", diff.new_path));
                    context_for_comment.push_str("| Commit | Author | Date | Title |\n");
                    context_for_comment.push_str("|--------|---------|------|-------|\n");
                    for commit in commits.iter() {
                        let commit_url = format!("{}/commit/{}", project.web_url, commit.id);
                        context_for_comment.push_str(&format!(
                            "| [{}]({}) | {} | {} | {} |\n",
                            &commit.short_id,
                            commit_url,
                            commit.author_name,
                            commit
                                .authored_date
                                .split('T')
                                .next()
                                .unwrap_or(&commit.authored_date),
                            commit.title
                        ));
                    }
                    context_for_comment.push('\n');
                }
                Err(e) => {
                    warn!(
                        "Failed to get commit history for {}: {}. Continuing with other context.",
                        diff.new_path, e
                    );
                }
            }

            // Check if adding this context would exceed our context limit
            if total_size + file_context.len() > self.settings.max_context_size {
                context_for_llm
                    .push_str("\n[Additional files omitted due to context size limits]\n");
                break;
            }

            context_for_llm.push_str(&file_context);
            total_size += file_context.len();
        }

        if !has_any_content {
            context_for_llm = "No source files or changes found in this merge request.".to_string();
        } else if context_for_llm.is_empty() {
            context_for_llm =
                "Context gathering completed but no content was added due to size constraints."
                    .to_string();
        }

        if context_for_comment.is_empty() {
            context_for_comment = "No commit history available for the changed files.".to_string();
        }

        Ok((context_for_llm, context_for_comment))
    }

    // Functions related to finding relevant files have been removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::models::GitlabUser;
    use urlencoding::encode;

    // Tests for extract_keywords and calculate_relevance_score have been removed

    // Helper to create AppSettings for tests
    fn test_settings(gitlab_url: String, context_repo: Option<String>) -> Arc<AppSettings> {
        Arc::new(AppSettings {
            gitlab_url: gitlab_url.clone(),
            gitlab_token: "test_token".to_string(),
            openai_api_key: "test_openai_key".to_string(),
            openai_custom_url: gitlab_url, // Mock server URL
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 150,
            repos_to_poll: vec!["test_org/test_repo".to_string()],
            log_level: "debug".to_string(),
            bot_username: "test_bot".to_string(),
            poll_interval_seconds: 60,
            default_branch: "main".to_string(),
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: context_repo,
            max_context_size: 60000,
        })
    }

    fn create_mock_project(id: i64, path_with_namespace: &str) -> GitlabProject {
        GitlabProject {
            id,
            path_with_namespace: path_with_namespace.to_string(),
            web_url: format!("https://gitlab.com/{}", path_with_namespace),
        }
    }

    fn create_mock_issue(iid: i64, project_id: i64) -> GitlabIssue {
        GitlabIssue {
            id: iid, // Typically id and iid might be different, but for mock it's fine
            iid,
            project_id,
            title: format!("Test Issue #{}", iid),
            description: Some(format!("Description for issue #{}", iid)),
            state: "opened".to_string(),
            author: GitlabUser {
                id: 1,
                username: "test_user".to_string(),
                name: "Test User".to_string(),
                avatar_url: None,
            },
            web_url: "url".to_string(),
            labels: vec![],
            updated_at: "2023-01-01T00:00:00Z".to_string(),
        }
    }

    fn create_mock_mr(iid: i64, project_id: i64) -> GitlabMergeRequest {
        GitlabMergeRequest {
            id: iid,
            iid,
            project_id,
            title: format!("Test MR !{}", iid),
            description: Some(format!("Description for MR !{}", iid)),
            state: "opened".to_string(),
            author: GitlabUser {
                id: 1,
                username: "test_user".to_string(),
                name: "Test User".to_string(),
                avatar_url: None,
            },
            source_branch: "feature-branch".to_string(),
            target_branch: "main".to_string(),
            web_url: "url".to_string(),
            labels: vec![],
            detailed_merge_status: Some("mergeable".to_string()),
            updated_at: "2023-01-01T00:00:00Z".to_string(),
            head_pipeline: None,
        }
    }

    #[tokio::test]
    async fn test_extract_context_for_issue_with_agents_md() {
        let mut server = mockito::Server::new_async().await;
        let settings = test_settings(server.url(), None);
        let gitlab_client = Arc::new(GitlabApiClient::new(settings.clone()).unwrap());
        let extractor = RepoContextExtractor::new(gitlab_client.clone(), settings.clone());

        let project = create_mock_project(1, "test_org/main_repo");
        let issue = create_mock_issue(101, project.id);
        let agents_md_content = "This is the AGENTS.md content from main_repo.";

        // Mock get_repository_tree for the first call (by get_combined_source_files)
        let _m_repo_tree_src_files = server
            .mock("GET", "/api/v4/projects/1/repository/tree?recursive=true&per_page=100")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!([{"id": "1", "name": "main.rs", "type": "blob", "path": "src/main.rs", "mode": "100644"}]).to_string())
            .expect(1) // Called once for get_combined_source_files
            .create_async()
            .await;

        let _m_agents_md_main = server
            .mock(
                "GET",
                format!(
                    "/api/v4/projects/1/repository/files/{}?ref=main",
                    AGENTS_MD_FILE
                )
                .as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "file_name": AGENTS_MD_FILE,
                    "file_path": AGENTS_MD_FILE,
                    "size": agents_md_content.len(),
                    "encoding": "base64",
                    "content": base64::encode(agents_md_content),
                    "ref": "main",
                    "blob_id": "someblobid",
                    "commit_id": "somecommitid",
                    "last_commit_id": "somelastcommitid"
                })
                .to_string(),
            )
            .create_async()
            .await;

        // Mock for get_project_by_path no longer needed since find_relevant_files_for_issue was removed

        let context = extractor
            .extract_context_for_issue(&issue, &project, None)
            .await
            .unwrap();

        assert!(
            context.contains(&format!(
                "--- All Source Files (up to {} files) ---",
                MAX_SOURCE_FILES
            )),
            "Context missing correctly formatted 'All Source Files' header. Full: {}",
            context
        );
        assert!(
            context.contains("src/main.rs"),
            "Context missing 'src/main.rs'. Full: {}",
            context
        );
        assert!(
            context.contains("--- AGENTS.md ---"),
            "Context missing AGENTS.md header. Full: {}",
            context
        );
        assert!(
            context.contains(agents_md_content),
            "Context missing AGENTS.md content. Full: {}",
            context
        );
    }

    #[tokio::test]
    async fn test_extract_context_for_issue_with_agents_md_in_context_repo() {
        let mut server = mockito::Server::new_async().await;
        let context_repo_path = "test_org/context_repo";
        let settings = test_settings(server.url(), Some(context_repo_path.to_string()));
        let gitlab_client = Arc::new(GitlabApiClient::new(settings.clone()).unwrap());
        let extractor = RepoContextExtractor::new(gitlab_client.clone(), settings.clone());

        let main_project = create_mock_project(1, "test_org/main_repo");
        let context_project_mock = create_mock_project(2, context_repo_path);
        let issue = create_mock_issue(102, main_project.id);
        let agents_md_content = "This is the AGENTS.md content from context_repo.";

        // Mock get_repository_tree for main project (empty source files for simplicity in get_combined_source_files)
        let _m_repo_tree_main_src = server
            .mock(
                "GET",
                "/api/v4/projects/1/repository/tree?recursive=true&per_page=100",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!([]).to_string())
            .create_async()
            .await;

        // Mock get_file_content for AGENTS.md in main project (not found)
        let _m_agents_md_main_not_found = server
            .mock(
                "GET",
                format!(
                    "/api/v4/projects/1/repository/files/{}?ref=main",
                    AGENTS_MD_FILE
                )
                .as_str(),
            )
            .with_status(404) // Not Found
            .create_async()
            .await;

        // Mock get_project_by_path for context_repo (called by get_combined_source_files and get_agents_md_content)
        let _m_context_project_fetch = server
            .mock(
                "GET",
                format!("/api/v4/projects/{}", encode(context_repo_path)).as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!(context_project_mock).to_string())
            .expect(2) // Called by get_combined_source_files and get_agents_md_content
            .create_async()
            .await;

        // Mock get_repository_tree for context project (for get_combined_source_files)
        let _m_repo_tree_context_src = server
            .mock(
                "GET",
                "/api/v4/projects/2/repository/tree?recursive=true&per_page=100",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!([]).to_string()) // No source files in context repo for this part
            .create_async()
            .await;

        // Mock get_file_content for AGENTS.md in context project
        let _m_agents_md_context = server
            .mock(
                "GET",
                format!(
                    "/api/v4/projects/2/repository/files/{}?ref=main",
                    AGENTS_MD_FILE
                )
                .as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "file_name": AGENTS_MD_FILE,
                    "file_path": AGENTS_MD_FILE,
                    "size": agents_md_content.len(),
                    "encoding": "base64",
                    "content": base64::encode(agents_md_content),
                    "ref": "main",
                    "blob_id": "someblobid",
                    "commit_id": "somecommitid",
                    "last_commit_id": "somelastcommitid"
                })
                .to_string(),
            )
            .create_async()
            .await;

        // Mocks for find_relevant_files_for_issue have been removed

        let context = extractor
            .extract_context_for_issue(&issue, &main_project, Some(context_repo_path))
            .await
            .unwrap();

        // Assert AGENTS.md content is present
        assert!(
            context.contains("--- AGENTS.md ---"),
            "Context should contain AGENTS.md header. Full context: {}",
            context
        );
        assert!(
            context.contains(agents_md_content),
            "Context should contain AGENTS.md content from context_repo. Full context: {}",
            context
        );

        // Assert that source file list is NOT present (since mocked as empty)
        assert!(!context.contains("--- All Source Files ---"), "Context should NOT contain 'All Source Files' header if no source files. Full context: {}", context);

        // Assert that the default "empty" message is NOT present because AGENTS.md was added
        assert!(!context.contains("No source files or relevant files found"), "Context should NOT contain default empty message if AGENTS.md is present. Full context: {}", context);
    }

    #[tokio::test]
    async fn test_extract_context_for_mr_with_agents_md() {
        let mut server = mockito::Server::new_async().await;
        let settings = test_settings(server.url(), None);
        let gitlab_client = Arc::new(GitlabApiClient::new(settings.clone()).unwrap());
        let extractor = RepoContextExtractor::new(gitlab_client.clone(), settings.clone());

        let project = create_mock_project(1, "test_org/main_repo");
        let mr = create_mock_mr(201, project.id);
        let agents_md_content = "MR AGENTS.md content.";

        // Mock get_repository_tree (for source files)
        let _m_repo_tree = server
            .mock("GET", "/api/v4/projects/1/repository/tree?recursive=true&per_page=100")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!([{"id": "1", "name": "code.rs", "type": "blob", "path": "src/code.rs", "mode": "100644"}]).to_string())
            .create_async()
            .await;

        // Mock get_file_content for AGENTS.md in main project
        let _m_agents_md_main = server
            .mock(
                "GET",
                format!(
                    "/api/v4/projects/1/repository/files/{}?ref=main",
                    AGENTS_MD_FILE
                )
                .as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "file_name": AGENTS_MD_FILE,
                    "file_path": AGENTS_MD_FILE,
                    "size": agents_md_content.len(),
                    "encoding": "base64",
                    "content": base64::encode(agents_md_content),
                    "ref": "main",
                    "blob_id": "someblobid",
                    "commit_id": "somecommitid",
                    "last_commit_id": "somelastcommitid"
                })
                .to_string(),
            )
            .create_async()
            .await;

        // Mock get_merge_request_changes (empty diff for simplicity)
        let _m_mr_changes = server
            .mock("GET", "/api/v4/projects/1/merge_requests/201/changes")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({ "changes": [] }).to_string())
            .create_async()
            .await;

        let (context_llm, context_comment) = extractor
            .extract_context_for_mr(&mr, &project, None)
            .await
            .unwrap();

        assert!(
            context_llm.contains(&format!(
                "--- All Source Files (up to {} files) ---",
                MAX_SOURCE_FILES
            )),
            "LLM context missing correctly formatted 'All Source Files' header. Full: {}",
            context_llm
        );
        assert!(
            context_llm.contains("src/code.rs"),
            "LLM context missing 'src/code.rs'. Full: {}",
            context_llm
        );
        assert!(
            context_llm.contains("--- AGENTS.md ---"),
            "LLM context missing AGENTS.md header. Full: {}",
            context_llm
        );
        assert!(
            context_llm.contains(agents_md_content),
            "LLM context missing AGENTS.md content. Full: {}",
            context_llm
        );
        // Since diffs are empty, no "Changes in file" section. Commit history for comment should be default.
        assert!(
            !context_llm.contains("Changes in"),
            "LLM context should not contain diff changes. Full: {}",
            context_llm
        );
        assert_eq!(
            context_comment,
            "No commit history available for the changed files."
        );
        // Ensure the default "No source files or changes found..." message is NOT there because we have source files and AGENTS.md
        assert!(
            !context_llm.contains("No source files or changes found"),
            "LLM context should not contain default empty message. Full: {}",
            context_llm
        );
    }
}
