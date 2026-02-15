use anyhow::Result;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::config::AppSettings;
use crate::file_indexer::FileIndexManager;
use crate::gitlab::GitlabApiClient;
use crate::gitlab::GitlabError;
use crate::models::{GitlabIssue, GitlabMergeRequest, GitlabProject};

pub(crate) const MAX_SOURCE_FILES: usize = 250; // Maximum number of source files to include in context
pub(crate) const AGENTS_MD_FILE: &str = "AGENTS.md";

/// Estimates the number of tokens in a text string.
/// Uses a heuristic of approximately 4 characters per token for English text.
/// This is a rough approximation but sufficient for context size limiting.
pub(crate) fn estimate_tokens(text: &str) -> usize {
    // Simple heuristic: roughly 4 characters per token for English text
    // This accounts for spaces, punctuation, and typical word lengths
    let char_count = text.chars().count();
    char_count.div_ceil(4)
}

#[derive(Debug, Deserialize)]
pub struct GitlabFile {
    pub file_path: String,
    pub size: usize,
    pub content: Option<String>,
    pub encoding: Option<String>,
    pub relevance_score: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct FileContentMatch {
    /// Starting line number (1-based)
    pub start_line: usize,
    /// Ending line number (1-based, inclusive)
    pub end_line: usize,
    /// The actual content lines
    pub lines: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitlabDiff {
    pub new_path: String,
    pub diff: String,
}

pub struct RepoContextExtractor {
    pub(crate) gitlab_client: Arc<GitlabApiClient>,
    pub(crate) settings: Arc<AppSettings>,
    pub(crate) file_index_manager: Arc<FileIndexManager>,
}

impl RepoContextExtractor {
    pub fn new_with_file_indexer(
        gitlab_client: Arc<GitlabApiClient>,
        settings: Arc<AppSettings>,
        file_index_manager: Arc<FileIndexManager>,
    ) -> Self {
        Self {
            gitlab_client,
            settings,
            file_index_manager,
        }
    }

    /// Initialize file indexes for a list of projects
    pub async fn initialize_file_indexes(&self, projects: Vec<GitlabProject>) -> Result<()> {
        info!("Initializing file indexes for {} projects", projects.len());

        // Start a background task to periodically refresh the indexes
        self.file_index_manager
            .clone()
            .start_refresh_task(projects.clone());

        // Build initial indexes for all projects
        for project in &projects {
            if let Err(e) = self.file_index_manager.build_index(project).await {
                warn!(
                    "Failed to build initial index for {}: {}",
                    project.path_with_namespace, e
                );
            }
        }

        Ok(())
    }

    async fn get_file_content_from_project(
        &self,
        project_id: i64,
        file_path: &str,
    ) -> Result<Option<String>> {
        match self
            .gitlab_client
            .get_file_content(project_id, file_path, None)
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
        let mut total_tokens = 0;
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
            total_tokens += estimate_tokens(&files_list);
        }

        // Add AGENTS.md content if available
        match self.get_agents_md_content(project, context_repo_path).await {
            Ok(Some(agents_md)) => {
                has_any_content = true;
                let agents_md_context = format!("\n--- {AGENTS_MD_FILE} ---\n{agents_md}\n");
                if total_tokens + estimate_tokens(&agents_md_context)
                    <= self.settings.max_context_size
                {
                    context.push_str(&agents_md_context);
                    total_tokens += estimate_tokens(&agents_md_context);
                } else {
                    warn!(
                        "AGENTS.md content too large to fit in context for issue #{}",
                        issue.iid
                    );
                    context.push_str(&format!(
                        "\n--- {AGENTS_MD_FILE} ---\n[Content omitted due to context size limits]\n"
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

        // Get repository files that might be relevant to the issue from both main project and context repo
        let relevant_files = self
            .find_relevant_files_from_all_sources(issue, project, context_repo_path)
            .await;

        if !relevant_files.is_empty() {
            has_any_content = true;
            // Extract keywords from issue for content filtering
            let keywords = self.extract_keywords(issue);

            // Then add relevant file contents with matched sections only
            for file in relevant_files {
                if let Some(content) = file.content {
                    // Extract only relevant sections that contain keywords
                    let matches = self.extract_relevant_file_sections(&content, &keywords);

                    if !matches.is_empty() {
                        // Build the file content with line numbers and sections
                        let mut content_with_lines = String::new();

                        for (i, section) in matches.iter().enumerate() {
                            if i > 0 {
                                content_with_lines.push_str("\n...\n\n"); // Separator between sections
                            }

                            content_with_lines.push_str(&format!(
                                "Lines {}-{}:\n",
                                section.start_line, section.end_line
                            ));

                            for (j, line) in section.lines.iter().enumerate() {
                                let line_number = section.start_line + j;
                                content_with_lines.push_str(&format!("{line_number:4}: {line}\n"));
                            }
                        }

                        // Use the format_weighted_file_context function
                        let relevance_score = file.relevance_score.unwrap_or(0);
                        let file_context = format!(
                            "\n{}",
                            self.format_weighted_file_context(
                                &file.file_path,
                                &content_with_lines,
                                relevance_score
                            )
                        );

                        // Check if adding this file would exceed our context limit
                        if total_tokens + estimate_tokens(&file_context)
                            > self.settings.max_context_size
                        {
                            // If we're about to exceed the limit, add a truncation notice
                            context.push_str(
                                "\n[Additional files omitted due to context size limits]\n",
                            );
                            break;
                        }

                        context.push_str(&file_context);
                        total_tokens += estimate_tokens(&file_context);
                    }
                }
            }
        } else {
            debug!("No relevant files found for issue #{}", issue.iid);
        }

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
        let mut total_tokens = 0;
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
            total_tokens += estimate_tokens(&files_list);
        }

        // Add AGENTS.md content if available
        match self.get_agents_md_content(project, context_repo_path).await {
            Ok(Some(agents_md)) => {
                has_any_content = true;
                let agents_md_context = format!("\n--- {AGENTS_MD_FILE} ---\n{agents_md}\n");
                if total_tokens + estimate_tokens(&agents_md_context)
                    <= self.settings.max_context_size
                {
                    context_for_llm.push_str(&agents_md_context);
                    total_tokens += estimate_tokens(&agents_md_context);
                } else {
                    warn!(
                        "AGENTS.md content too large to fit in context for MR !{}",
                        mr.iid
                    );
                    context_for_llm.push_str(&format!(
                        "\n--- {AGENTS_MD_FILE} ---\n[Content omitted due to context size limits]\n"
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

        if total_tokens + estimate_tokens(&pipeline_status_context)
            <= self.settings.max_context_size
        {
            context_for_llm.push_str(&pipeline_status_context);
            total_tokens += estimate_tokens(&pipeline_status_context);
        } else {
            warn!(
                "Pipeline status too large to fit in context for MR !{}",
                mr.iid
            );
            context_for_llm.push_str("\n--- Latest Pipeline Status ---\n[Pipeline status omitted due to context size limits]\n---");
            // We don't add the tokens of the omission message to total_tokens,
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
            if total_tokens + estimate_tokens(&file_context) > self.settings.max_context_size {
                context_for_llm
                    .push_str("\n[Additional files omitted due to context size limits]\n");
                break;
            }

            context_for_llm.push_str(&file_context);
            total_tokens += estimate_tokens(&file_context);
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

    /// Find files that might be relevant to the issue based on keywords
    async fn find_relevant_files_for_issue(
        &self,
        issue: &GitlabIssue,
        repo_path: &str,
    ) -> Result<Vec<GitlabFile>> {
        // Get project ID from path
        let project = match self.gitlab_client.get_project_by_path(repo_path).await {
            Ok(project) => project,
            Err(e) => {
                warn!("Failed to get project by path {}: {}", repo_path, e);
                return Err(e.into());
            }
        };

        // Extract keywords from issue title and description
        let keywords = self.extract_keywords(issue);
        debug!(
            "Extracted keywords for issue #{}: {:?}",
            issue.iid, keywords
        );

        if keywords.is_empty() {
            debug!("No meaningful keywords extracted from issue #{}", issue.iid);
            return Ok(Vec::new());
        }

        // Use the file index to find relevant files
        match self
            .file_index_manager
            .search_files(project.id, &keywords)
            .await
        {
            Ok(files) => {
                debug!(
                    "Found {} relevant files using content index for issue #{}",
                    files.len(),
                    issue.iid
                );
                return Ok(files);
            }
            Err(e) => {
                warn!(
                    "Error searching file index for issue #{}: {}. Falling back to path-based search.",
                    issue.iid, e
                );
                // Fall back to the original path-based search method
            }
        }

        // Fallback: Get repository file tree
        let files = match self.gitlab_client.get_repository_tree(project.id).await {
            Ok(files) => files,
            Err(e) => {
                warn!(
                    "Failed to get repository tree for project {}: {}",
                    project.id, e
                );
                return Err(e.into());
            }
        };

        // Score files based on relevance to keywords
        let mut scored_files = Vec::new();
        for file_path in &files {
            let score = self.calculate_relevance_score(file_path, &keywords);
            if score > 0 {
                scored_files.push((file_path.clone(), score));
            }
        }

        // Sort by relevance score (highest first)
        scored_files.sort_by(|a, b| b.1.cmp(&a.1));

        // Take top N most relevant files and preserve their path scores
        let top_files: Vec<(String, usize)> = scored_files
            .into_iter()
            .take(5) // Limit to 5 most relevant files
            .collect();

        // Fetch content for top files and calculate combined scores
        let mut files_with_content = Vec::new();
        for (file_path, _path_score) in top_files {
            match self
                .gitlab_client
                .get_file_content(project.id, &file_path, None)
                .await
            {
                Ok(mut file) => {
                    // Calculate combined relevance score including content
                    let combined_score = self.calculate_combined_relevance_score(
                        &file_path,
                        file.content.as_deref(),
                        &keywords,
                    );

                    file.relevance_score = Some(combined_score);
                    files_with_content.push(file);
                }
                Err(e) => warn!("Failed to get content for file {}: {}", file_path, e),
            }
        }

        // Re-sort by combined score (highest first)
        files_with_content.sort_by(|a, b| {
            b.relevance_score
                .unwrap_or(0)
                .cmp(&a.relevance_score.unwrap_or(0))
        });

        Ok(files_with_content)
    }

    // Find relevant files from both main project and context repo if provided
    async fn find_relevant_files_from_all_sources(
        &self,
        issue: &GitlabIssue,
        project: &GitlabProject,
        context_repo_path: Option<&str>,
    ) -> Vec<GitlabFile> {
        let mut all_relevant_files = Vec::new();

        // First try to get files from the main project
        match self
            .find_relevant_files_for_issue(issue, &project.path_with_namespace)
            .await
        {
            Ok(files) => {
                debug!(
                    "Found {} relevant files in main project {}",
                    files.len(),
                    project.path_with_namespace
                );
                all_relevant_files.extend(files);
            }
            Err(e) => {
                warn!(
                    "Failed to find relevant files in main project {}: {}. Will try context repo if available.",
                    project.path_with_namespace, e
                );
            }
        }

        // If context repo is provided, also get files from there
        if let Some(context_path) = context_repo_path {
            match self
                .find_relevant_files_for_issue(issue, context_path)
                .await
            {
                Ok(files) => {
                    debug!(
                        "Found {} relevant files in context repo {}",
                        files.len(),
                        context_path
                    );
                    all_relevant_files.extend(files);
                }
                Err(e) => {
                    warn!(
                        "Failed to find relevant files in context repo {}: {}",
                        context_path, e
                    );
                }
            }
        }

        all_relevant_files
    }

    /// Extract keywords from issue title and description
    pub(crate) fn extract_keywords(&self, issue: &GitlabIssue) -> Vec<String> {
        let mut text = issue.title.clone();
        if let Some(desc) = &issue.description {
            text.push(' ');
            text.push_str(desc);
        }

        // Convert to lowercase and split by non-alphanumeric characters
        let words: Vec<String> = text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty() && s.len() > 2) // Filter out empty strings and very short words
            .map(|s| s.to_string())
            .collect();

        // Remove common words
        let common_words = [
            "the", "and", "for", "this", "that", "with", "from", "have", "not", "but", "what",
            "all", "are", "when", "your", "can", "has", "been",
        ];

        words
            .into_iter()
            .filter(|word| !common_words.contains(&word.as_str()))
            .collect()
    }

    /// Calculate relevance score of a file to the keywords
    pub(crate) fn calculate_relevance_score(&self, file_path: &str, keywords: &[String]) -> usize {
        let path_lower = file_path.to_lowercase();

        // Skip binary files and non-code files
        let binary_extensions = [
            ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".ico", ".svg", ".pdf", ".zip", ".tar", ".gz",
            ".rar", ".exe", ".dll", ".so", ".bin", ".dat", ".db", ".sqlite", ".mp3", ".mp4",
            ".avi", ".mov",
        ];

        if binary_extensions
            .iter()
            .any(|ext| path_lower.ends_with(ext))
        {
            return 0;
        }

        // Calculate score based on keyword matches
        let mut score = 0;

        // Prefer documentation files, but only if they match keywords
        let has_keyword_match = keywords
            .iter()
            .any(|keyword| path_lower.contains(&keyword.to_lowercase()));

        if has_keyword_match {
            if path_lower.contains("readme")
                || path_lower.contains("docs/")
                || path_lower.ends_with(".md")
            {
                score += 5;
            }

            // Prefer source code files
            let code_extensions = [
                ".rs", ".py", ".js", ".ts", ".java", ".c", ".cpp", ".h", ".hpp", ".go", ".rb",
                ".php", ".cs", ".scala", ".kt", ".swift", ".sh", ".jsx", ".tsx", ".vue", ".svelte",
            ];

            if code_extensions.iter().any(|ext| path_lower.ends_with(ext)) {
                score += 3;
            }

            // Add points for each keyword match in the file path
            for keyword in keywords {
                if path_lower.contains(&keyword.to_lowercase()) {
                    score += 10; // Higher score for direct matches in path
                }
            }
        }

        score
    }

    /// Calculate relevance score based on content keyword frequency
    pub(crate) fn calculate_content_relevance_score(
        &self,
        content: &str,
        keywords: &[String],
    ) -> usize {
        if keywords.is_empty() || content.is_empty() {
            return 0;
        }

        let content_lower = content.to_lowercase();
        let mut total_hits = 0;

        // Count occurrences of each keyword
        for keyword in keywords {
            let keyword_lower = keyword.to_lowercase();
            let hits = content_lower.matches(&keyword_lower).count();
            total_hits += hits;
        }

        total_hits
    }

    /// Calculate combined relevance score considering both path and content
    pub(crate) fn calculate_combined_relevance_score(
        &self,
        file_path: &str,
        content: Option<&str>,
        keywords: &[String],
    ) -> usize {
        let path_score = self.calculate_relevance_score(file_path, keywords);

        let content_score = match content {
            Some(content_str) => self.calculate_content_relevance_score(content_str, keywords),
            None => 0,
        };

        // Combine scores with weighting - content hits are more valuable than path matches
        path_score + (content_score * 5)
    }

    /// Format file context with weight information for LLM
    pub(crate) fn format_weighted_file_context(
        &self,
        file_path: &str,
        content: &str,
        weight: usize,
    ) -> String {
        let relevance_percentage = if weight > 0 {
            std::cmp::min(weight * 2, 100) // Convert weight to a percentage (capped at 100%)
        } else {
            0
        };

        format!("--- File: {file_path} (Relevance: {relevance_percentage}%) ---\n{content}\n")
    }

    /// Extract relevant sections from file content based on keyword matches
    pub(crate) fn extract_relevant_file_sections(
        &self,
        file_content: &str,
        keywords: &[String],
    ) -> Vec<FileContentMatch> {
        let lines: Vec<String> = file_content.lines().map(|s| s.to_string()).collect();
        if lines.is_empty() {
            return Vec::new();
        }

        // Find line numbers that contain any of the keywords (case-insensitive)
        let mut matching_lines = HashSet::new();
        for (line_idx, line) in lines.iter().enumerate() {
            let line_lower = line.to_lowercase();
            for keyword in keywords {
                if line_lower.contains(&keyword.to_lowercase()) {
                    matching_lines.insert(line_idx);
                    break; // Found a match, no need to check other keywords for this line
                }
            }
        }

        if matching_lines.is_empty() {
            return Vec::new();
        }

        // Convert to sorted vector and merge overlapping or adjacent ranges
        let mut sorted_matches: Vec<usize> = matching_lines.into_iter().collect();
        sorted_matches.sort();

        let mut merged_ranges: Vec<(usize, usize)> = Vec::new();
        for &line_idx in &sorted_matches {
            // Calculate range with context
            let start = line_idx.saturating_sub(self.settings.context_lines);
            let end = (line_idx + self.settings.context_lines).min(lines.len() - 1);

            // Check if this range overlaps with the last range
            if let Some(last_range) = merged_ranges.last_mut() {
                if start <= last_range.1 + 1 {
                    // Overlapping or adjacent, merge by extending the end
                    last_range.1 = end;
                } else {
                    // No overlap, add new range
                    merged_ranges.push((start, end));
                }
            } else {
                // First range
                merged_ranges.push((start, end));
            }
        }

        // Convert ranges to FileContentMatch structs
        let mut matches = Vec::new();
        for (start, end) in merged_ranges {
            let range_lines = lines[start..=end].to_vec();
            matches.push(FileContentMatch {
                start_line: start + 1, // Convert to 1-based line numbering
                end_line: end + 1,     // Convert to 1-based line numbering
                lines: range_lines,
            });
        }

        matches
    }
}
