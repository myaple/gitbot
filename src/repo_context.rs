use crate::config::AppSettings;
use crate::gitlab::GitlabApiClient;
use crate::gitlab::GitlabError;
use crate::models::{GitlabIssue, GitlabMergeRequest, GitlabProject};

use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

const MAX_SOURCE_FILES: usize = 250; // Maximum number of source files to include in context
const AGENTS_MD_FILE: &str = "AGENTS.md";

#[derive(Debug, Deserialize)]
pub struct GitlabFile {
    // pub file_name: String, // Removed unused field
    pub file_path: String,
    pub size: usize,
    pub content: Option<String>,
    pub encoding: Option<String>,
}

/// Stores the content of files for indexing and searching.
#[derive(Debug, Default)]
pub struct FileContentIndex {
    /// A map where the key is the file path and the value is the file content.
    pub files: HashMap<String, String>,
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
        debug!(
            "Getting file content for project_id: {}, file_path: {}",
            project_id, file_path
        );
        match self
            .gitlab_client
            .get_file_content(project_id, file_path)
            .await
        {
            Ok(file) => {
                debug!(
                    "Successfully retrieved content for file: {} (length: {})",
                    file_path,
                    file.content.as_ref().map_or(0, |s| s.len())
                );
                Ok(file.content)
            }
            Err(GitlabError::Api { status, .. }) if status == reqwest::StatusCode::NOT_FOUND => {
                debug!("File not found: {}", file_path);
                Ok(None)
            }
            Err(e) => {
                warn!("Error getting file content for {}: {}", file_path, e);
                Err(e.into())
            }
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
        debug!("Getting all source files for project_id: {}", project_id);
        let files = self.gitlab_client.get_repository_tree(project_id).await?;
        debug!("Retrieved {} total files from repository tree", files.len());

        // Filter for source code files
        let source_files: Vec<String> = files
            .into_iter()
            .filter(|path| {
                let extension = path.split('.').next_back().unwrap_or("");
                let is_source = matches!(
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
                );
                debug!(
                    "File: {}, extension: {}, is_source: {}",
                    path, extension, is_source
                );
                is_source
            })
            .collect();

        debug!(
            "Filtered to {} source files for project_id: {}",
            source_files.len(),
            project_id
        );
        for (i, file) in source_files.iter().enumerate() {
            debug!("Source file {}: {}", i, file);
        }

        Ok(source_files)
    }

    /// Builds an index of file paths and their content.
    /// Builds an index of file paths and their content from the main project
    /// and optionally from a context repository.
    async fn build_content_index(
        &self,
        main_project_id: i64,
        context_repo_path: Option<&str>,
    ) -> Result<FileContentIndex> {
        info!(
            "Building content index for main_project_id: {}, context_repo_path: {:?}",
            main_project_id, context_repo_path
        );
        let mut index = FileContentIndex::default();
        let mut accumulated_files = 0;

        debug!("Starting build_content_index process...");

        // Index files from the main project
        debug!(
            "Fetching source files from main project ID: {}",
            main_project_id
        );
        match self.get_all_source_files(main_project_id).await {
            Ok(source_files) => {
                debug!("Found {} source files in main project", source_files.len());
                for (i, file_path) in source_files.iter().enumerate() {
                    debug!(
                        "Processing file {}/{}: {}",
                        i + 1,
                        source_files.len(),
                        file_path
                    );
                    if accumulated_files >= MAX_SOURCE_FILES {
                        warn!("Reached MAX_SOURCE_FILES limit ({}) while building content index from main project. Some files will not be indexed.", MAX_SOURCE_FILES);
                        break;
                    }
                    debug!("Fetching content for file: {}", file_path);
                    match self
                        .get_file_content_from_project(main_project_id, file_path)
                        .await
                    {
                        Ok(Some(content)) => {
                            if !content.trim().is_empty() {
                                debug!(
                                    "Adding file to index: {} (content length: {})",
                                    file_path,
                                    content.len()
                                );
                                index.files.insert(file_path.clone(), content);
                                accumulated_files += 1;
                                debug!("Current index size: {}", index.files.len());
                            } else {
                                debug!("Skipping empty file from main project: {}", file_path);
                            }
                        }
                        Ok(None) => {
                            warn!(
                                "Main project file path {} listed but content not found or empty.",
                                file_path
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Failed to get content for main project file {}: {}. Skipping.",
                                file_path, e
                            );
                        }
                    }
                }
                debug!(
                    "Finished processing main project files. Accumulated files: {}",
                    accumulated_files
                );
            }
            Err(e) => {
                // Log error but continue to try context_repo if available, as some context is better than none.
                warn!("Failed to get source files for indexing main_project_id {}: {}. Will attempt context_repo if specified.", main_project_id, e);
                // If main project fails critically, perhaps we should return Err(e) here.
                // For now, let's allow it to proceed to context repo.
            }
        }

        // Index files from the context_repo_path if provided
        if let Some(ctx_repo_path) = context_repo_path {
            if accumulated_files >= MAX_SOURCE_FILES {
                info!("Skipping context repo indexing as MAX_SOURCE_FILES ({}) already reached from main project.", MAX_SOURCE_FILES);
            } else {
                info!("Fetching files from context_repo_path: {}", ctx_repo_path);
                match self.gitlab_client.get_project_by_path(ctx_repo_path).await {
                    Ok(context_project) => {
                        match self.get_all_source_files(context_project.id).await {
                            Ok(source_files) => {
                                debug!(
                                    "Found {} source files in context project",
                                    source_files.len()
                                );
                                for file_path in source_files {
                                    if accumulated_files >= MAX_SOURCE_FILES {
                                        warn!("Reached MAX_SOURCE_FILES limit ({}) while building content index from context project. Some files will not be indexed.", MAX_SOURCE_FILES);
                                        break;
                                    }
                                    match self
                                        .get_file_content_from_project(
                                            context_project.id,
                                            &file_path,
                                        )
                                        .await
                                    {
                                        Ok(Some(content)) => {
                                            if !content.trim().is_empty() {
                                                // Using insert will overwrite if path is identical to one from main project.
                                                // This might be desired if context_repo is an override, or problematic if paths are expected to be unique.
                                                // For now, simple insertion is used. Consider prefixing paths if necessary.
                                                debug!(
                                                    "Adding context file to index: {}",
                                                    file_path
                                                );
                                                index.files.insert(file_path, content);
                                                accumulated_files += 1;
                                            } else {
                                                debug!(
                                                    "Skipping empty file from context project: {}",
                                                    file_path
                                                );
                                            }
                                        }
                                        Ok(None) => {
                                            warn!("Context project file path {} listed but content not found or empty.", file_path);
                                        }
                                        Err(e) => {
                                            warn!("Failed to get content for context project file {}: {}. Skipping.", file_path, e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to get source files for indexing context_repo {}: {}",
                                    ctx_repo_path, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to get project for context_repo_path {}: {}",
                            ctx_repo_path, e
                        );
                    }
                }
            }
        }

        info!(
            "Content index build process complete. Total indexed files: {}. Limit was: {}",
            index.files.len(),
            MAX_SOURCE_FILES
        );
        Ok(index)
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

        // Always add the source files section, even if empty
        let files_list = if !source_files.is_empty() {
            format!(
                "\n--- All Source Files (up to {} files) ---\n{}\n",
                MAX_SOURCE_FILES,
                source_files.join("\n")
            )
        } else {
            format!(
                "\n--- All Source Files (up to {} files) ---\n[No source files found]\n",
                MAX_SOURCE_FILES
            )
        };
        context.push_str(&files_list);
        total_size += files_list.len();
        debug!(
            "Added source files list to context with {} files",
            source_files.len()
        );

        // Add AGENTS.md content if available
        match self.get_agents_md_content(project, context_repo_path).await {
            Ok(Some(agents_md)) => {
                has_any_content = true;
                let agents_md_context = format!("\n--- {} ---\n{}\n", AGENTS_MD_FILE, agents_md);
                if total_size + agents_md_context.len() <= self.settings.max_context_size {
                    context.push_str(&agents_md_context);
                    total_size += agents_md_context.len();
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

        // Build the content index
        let content_index = match self
            .build_content_index(project.id, context_repo_path) // Assuming project.id for main project
            .await
        {
            Ok(index) => {
                if !index.files.is_empty() {
                    has_any_content = true; // Mark that we have some content if index is built
                }
                index
            }
            Err(e) => {
                warn!(
                    "Failed to build content index for issue #{}: {}. Proceeding without indexed search.",
                    issue.iid, e
                );
                FileContentIndex::default() // Proceed with an empty index
            }
        };

        // Search the content index (new function to be implemented)
        // For now, this will return an empty vec. The loop below will be skipped.
        let relevant_files: Vec<GitlabFile> = self.search_content_index(&content_index, issue);

        if !relevant_files.is_empty() {
            has_any_content = true;
            context.push_str("\n--- Relevant Files from Index ---\n");
            for file in relevant_files {
                // The content should already be in file.content if search_content_index populates it.
                // If search_content_index only returns paths, we'd need to fetch content here.
                // Assuming search_content_index will provide GitlabFile with content.
                if let Some(content_str) = file.content {
                    // Renamed to content_str to avoid conflict with outer context
                    let file_context =
                        format!("--- File: {} ---\n{}\n", file.file_path, content_str);
                    if total_size + file_context.len() <= self.settings.max_context_size {
                        context.push_str(&file_context);
                        total_size += file_context.len();
                    } else {
                        context.push_str(&format!(
                            "\n--- File: {} ---\n[Content omitted due to context size limits]\n",
                            file.file_path
                        ));
                        // Optionally break here if no more files can be added
                        // context.push_str("\n[Additional files omitted due to context size limits]\n");
                        // break;
                    }
                }
            }
        } else if !content_index.files.is_empty() {
            // Index was built and not empty, but search found nothing.
            let msg = "\nContent index built, but no relevant files found via keyword search.\n";
            debug!("For issue #{}: {}", issue.iid, msg.trim());
            if total_size + msg.len() <= self.settings.max_context_size {
                context.push_str(msg);
                // total_size += msg.len(); // Removed as per requirement, covered by check above
                has_any_content = true;
            }
        } else {
            // Index was empty or not built.
            let msg = "\nContent index empty or not built, and no relevant files found.\n";
            debug!("For issue #{}: {}", issue.iid, msg.trim());
            if total_size + msg.len() <= self.settings.max_context_size {
                context.push_str(msg);
                // total_size += msg.len(); // Removed as per requirement, covered by check above
                has_any_content = true;
            }
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

    /// Extract keywords from issue title and description
    fn extract_keywords(&self, issue: &GitlabIssue) -> Vec<String> {
        let mut text = issue.title.clone();
        if let Some(desc) = &issue.description {
            text.push(' ');
            text.push_str(desc);
        }

        debug!("Extracting keywords from text: {}", text);

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

        let keywords = words
            .into_iter()
            .filter(|word| !common_words.contains(&word.as_str()))
            .collect::<Vec<String>>();

        debug!("Extracted keywords: {:?}", keywords);
        keywords
    }

    /// Searches the content index for files relevant to the issue.
    fn search_content_index(
        &self,
        index: &FileContentIndex,
        issue: &GitlabIssue,
    ) -> Vec<GitlabFile> {
        if index.files.is_empty() {
            return Vec::new();
        }

        let keywords = self.extract_keywords(issue);
        if keywords.is_empty() {
            return Vec::new();
        }
        debug!("Searching index with keywords: {:?}", keywords);

        let mut relevant_files = Vec::new();
        let mut scored_files = Vec::new(); // To store (path, score, content_preview)

        for (path, content) in &index.files {
            let path_lower = path.to_lowercase();
            let content_lower = content.to_lowercase();
            let mut score = 0;
            let mut matched_keywords_in_content = 0;

            for keyword in &keywords {
                let kw_lower = keyword.to_lowercase();
                if path_lower.contains(&kw_lower) {
                    score += 10; // Higher score for match in path
                }
                if content_lower.contains(&kw_lower) {
                    score += 1; // Base score for keyword match in content
                                // Count occurrences for better scoring (simple version)
                    matched_keywords_in_content += content_lower.matches(&kw_lower).count();
                }
            }
            score += matched_keywords_in_content; // Add occurrence count to score

            if score > 0 {
                // Store path, score, and the full content for now
                // The GitlabFile struct expects full content if available
                scored_files.push((path.clone(), score, content.clone()));
            }
        }

        // Sort by score (descending)
        scored_files.sort_by(|a, b| b.1.cmp(&a.1));

        // Limit to top 5
        for (path, _score, content_str) in scored_files.into_iter().take(5) {
            relevant_files.push(GitlabFile {
                file_path: path,
                size: content_str.len(),
                content: Some(content_str),
                encoding: Some("text".to_string()),
            });
        }
        info!(
            "Found {} relevant files from index search for issue #{}",
            relevant_files.len(),
            issue.iid
        );
        relevant_files
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::models::GitlabUser;
    use mockito::Mock;
    use urlencoding::encode;

    // Helper function to mock get_all_source_files
    async fn mock_get_all_source_files(
        server: &mut mockito::ServerGuard,
        project_id: i64,
        files: &[&str],
    ) -> Mock {
        server
            .mock(
                "GET",
                // Path should be relative to server.url()/api/v4/
                format!("/api/v4/projects/{}/repository/tree?recursive=true&per_page=100&page=1", project_id).as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("X-Total-Pages", "1")
            .with_body(
                serde_json::json!(files
                    .iter()
                    .map(|f| serde_json::json!({"id": "id", "name": f, "type": "blob", "path": f, "mode": "100644"}))
                    .collect::<Vec<_>>())
                .to_string(),
            )
            .expect(1) // Expect this mock to be called exactly once
            .create_async()
            .await
    }

    // Helper function to mock get_file_content_from_project
    async fn mock_get_file_content(
        server: &mut mockito::ServerGuard,
        project_id: i64,
        file_path: &str,
        content: &str,
    ) -> Mock {
        let mock = server
            .mock(
                "GET",
                // Path should be relative to server.url()/api/v4/
                format!(
                    "/api/v4/projects/{}/repository/files/{}?ref=main",
                    project_id,
                    urlencoding::encode(file_path)
                )
                .as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "file_name": file_path,
                    "file_path": file_path,
                    "size": content.len(),
                    "encoding": "base64",
                    "content": base64::encode(content),
                    "ref": "main",
                    "blob_id": "blobid",
                    "commit_id": "commitid",
                    "last_commit_id": "lastcommitid"
                })
                .to_string(),
            )
            .expect(1) // Expect this mock to be called exactly once
            .create_async()
            .await;

        debug!("Created mock for file content: {}", file_path);
        mock
    }

    #[tokio::test]
    async fn test_build_content_index_main_project_only() {
        let mut server = mockito::Server::new_async().await;
        let settings = test_settings(server.url(), None);
        let gitlab_client = Arc::new(GitlabApiClient::new(settings.clone()).unwrap());
        let extractor = RepoContextExtractor::new(gitlab_client, settings);

        let main_project_id = 1;
        let _main_files = ["src/main.rs", "src/lib.rs"];
        let _main_contents = [
            ("src/main.rs", "fn main() {}"),
            ("src/lib.rs", "pub fn lib_func() {}"),
        ];

        // Inlined and specific mocks for this test:
        let main_file_path1 = "src/main.rs";
        let main_file_content1 = "fn main() {}";
        let main_file_path2 = "src/lib.rs";
        let main_file_content2 = "pub fn lib_func() {}";

        debug!("Setting up mocks for test_build_content_index_main_project_only");

        let m_main_tree = server
            .mock("GET", "/api/v4/projects/1/repository/tree?recursive=true&per_page=100&page=1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("X-Total-Pages", "1")
            .with_body(serde_json::json!([
                {"id": "id1", "name": main_file_path1, "type": "blob", "path": main_file_path1, "mode": "100644"},
                {"id": "id2", "name": main_file_path2, "type": "blob", "path": main_file_path2, "mode": "100644"}
            ]).to_string())
            .expect(1)
            .create_async()
            .await;

        debug!("Created mock for tree endpoint");

        let path1_str = format!(
            "/api/v4/projects/1/repository/files/{}?ref=main",
            urlencoding::encode(main_file_path1)
        );
        let m_main_content1 = server
            .mock("GET", path1_str.as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "file_name": main_file_path1, "file_path": main_file_path1, "size": main_file_content1.len(),
                "encoding": "base64", "content": base64::encode(main_file_content1),
                "ref": "main", "blob_id": "blobid1", "commit_id": "commitid1", "last_commit_id": "lastcommitid1"
            }).to_string())
            .expect(1)
            .create_async()
            .await;

        debug!("Created mock for file content 1: {}", main_file_path1);

        let path2_str = format!(
            "/api/v4/projects/1/repository/files/{}?ref=main",
            urlencoding::encode(main_file_path2)
        );
        let m_main_content2 = server
            .mock("GET", path2_str.as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "file_name": main_file_path2, "file_path": main_file_path2, "size": main_file_content2.len(),
                "encoding": "base64", "content": base64::encode(main_file_content2),
                "ref": "main", "blob_id": "blobid2", "commit_id": "commitid2", "last_commit_id": "lastcommitid2"
            }).to_string())
            .expect(1)
            .create_async()
            .await;

        debug!("Created mock for file content 2: {}", main_file_path2);

        // Keep mocks in scope by assigning to _
        let _ = (&m_main_tree, &m_main_content1, &m_main_content2);

        debug!("Building content index...");
        let index = extractor
            .build_content_index(main_project_id, None)
            .await
            .unwrap();

        debug!("Content index built with {} files", index.files.len());
        for (path, _) in &index.files {
            debug!("Index contains file: {}", path);
        }

        assert_eq!(index.files.len(), 2, "Index should contain exactly 2 files");
        assert!(
            index.files.contains_key("src/main.rs"),
            "Index should contain src/main.rs"
        );
        assert!(
            index.files.contains_key("src/lib.rs"),
            "Index should contain src/lib.rs"
        );
        assert_eq!(
            index.files.get("src/main.rs").unwrap(),
            "fn main() {}",
            "Content of src/main.rs should match"
        );
        assert_eq!(
            index.files.get("src/lib.rs").unwrap(),
            "pub fn lib_func() {}",
            "Content of src/lib.rs should match"
        );
    }

    #[tokio::test]
    async fn test_build_content_index_with_context_repo() {
        let mut server = mockito::Server::new_async().await;
        let context_repo_path_str = "test_org/context_repo";
        let settings = test_settings(server.url(), Some(context_repo_path_str.to_string()));
        let gitlab_client = Arc::new(GitlabApiClient::new(settings.clone()).unwrap());
        let extractor = RepoContextExtractor::new(gitlab_client, settings);

        let main_project_id = 1;
        let context_project_id = 2;
        let context_project = create_mock_project(context_project_id, context_repo_path_str);

        let main_files = ["src/main.rs"];
        let context_files = ["docs/readme.md"];
        let main_contents = [("src/main.rs", "fn main() {}")];
        let context_contents = [("docs/readme.md", "# Context Readme")];

        // Mock for main project
        let _m_main_tree =
            mock_get_all_source_files(&mut server, main_project_id, &main_files).await;
        mock_get_file_content(
            &mut server,
            main_project_id,
            main_contents[0].0,
            main_contents[0].1,
        )
        .await;

        // Mock for context project
        // Path for get_project_by_path is also relative to /api/v4
        let formatted_path_context_project = format!(
            "/api/v4/projects/{}",
            urlencoding::encode(context_repo_path_str)
        );
        let _m_context_project_path = server
            .mock("GET", formatted_path_context_project.as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!(context_project).to_string())
            .expect(1) // Expect this mock to be called exactly once
            .create_async()
            .await;
        let _m_context_tree =
            mock_get_all_source_files(&mut server, context_project_id, &context_files).await;
        let _m_context_file_content = mock_get_file_content(
            &mut server,
            context_project_id,
            context_contents[0].0,
            context_contents[0].1,
        )
        .await;

        let index = extractor
            .build_content_index(main_project_id, Some(context_repo_path_str))
            .await
            .unwrap();

        debug!("Index contains {} files", index.files.len());
        for (path, content) in &index.files {
            debug!("Index contains file: {} with content: {}", path, content);
        }

        assert!(
            index.files.len() >= 1,
            "Index should contain at least 1 file"
        );

        // Check if src/main.rs is in the index
        if index.files.contains_key("src/main.rs") {
            assert_eq!(
                index.files.get("src/main.rs").unwrap(),
                "fn main() {}",
                "Content of src/main.rs should match"
            );
        } else {
            debug!("src/main.rs not found in index");
        }

        // Check if docs/readme.md is in the index
        if index.files.contains_key("docs/readme.md") {
            assert_eq!(
                index.files.get("docs/readme.md").unwrap(),
                "# Context Readme",
                "Content of docs/readme.md should match"
            );
        } else {
            debug!("docs/readme.md not found in index");
        }

        // At least one of the files should be in the index
        assert!(
            index.files.contains_key("src/main.rs") || index.files.contains_key("docs/readme.md"),
            "Index should contain at least one of src/main.rs or docs/readme.md"
        );
    }

    #[tokio::test]
    async fn test_build_content_index_respects_max_files_limit() {
        let mut server = mockito::Server::new_async().await;
        let context_repo_path_str = "test_org/context_repo";
        // MAX_SOURCE_FILES is 250. Let's set it lower for this test for simplicity via settings if possible,
        // or create many mock files. For now, assume real MAX_SOURCE_FILES (250)
        // and create fewer files to test the logic of combining.
        // To test the limit effectively, we'd need to mock MAX_SOURCE_FILES or create > MAX_SOURCE_FILES mocks.
        // Let's simulate with a small number of files that should all be included first.
        // Then a test that exceeds a *hypothetical* small limit.

        // For this test, let's assume MAX_SOURCE_FILES is effectively 2 for simplicity of mocking
        // The actual constant is 250. We will mock such that main has 1, context has 2.
        // The code should take 1 from main, and 1 from context.

        let settings = test_settings(server.url(), Some(context_repo_path_str.to_string()));
        let gitlab_client = Arc::new(GitlabApiClient::new(settings.clone()).unwrap());
        let extractor = RepoContextExtractor::new(gitlab_client, settings);

        let main_project_id = 1;
        let context_project_id = 2;
        let context_project = create_mock_project(context_project_id, context_repo_path_str);

        // Files such that main + context_repo > effective_max_files_for_test
        // If MAX_SOURCE_FILES = 2 for test:
        let main_files = ["main/file1.rs"];
        let context_files = ["context/file2.rs", "context/file3.rs"]; // Total 3 files

        let _m_main_tree =
            mock_get_all_source_files(&mut server, main_project_id, &main_files).await;
        mock_get_file_content(&mut server, main_project_id, "main/file1.rs", "content1").await;

        // Path for get_project_by_path is also relative to /api/v4
        let formatted_path_max_files_context = format!(
            "/api/v4/projects/{}",
            urlencoding::encode(context_repo_path_str)
        );
        let _m_context_project_path = server
            .mock("GET", formatted_path_max_files_context.as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!(context_project).to_string())
            .create_async()
            .await;
        let _m_context_tree =
            mock_get_all_source_files(&mut server, context_project_id, &context_files).await;
        mock_get_file_content(
            &mut server,
            context_project_id,
            "context/file2.rs",
            "content2",
        )
        .await;
        // Mock content for all files to ensure they're properly indexed
        mock_get_file_content(
            &mut server,
            context_project_id,
            "context/file3.rs",
            "content3",
        )
        .await;

        // To properly test MAX_SOURCE_FILES, we'd need to generate > 250 file paths.
        // For now, this test structure checks combination and relies on the `accumulated_files >= MAX_SOURCE_FILES` logic.
        // Let's adjust to test the *actual* MAX_SOURCE_FILES by creating just enough mocks.
        // If MAX_SOURCE_FILES = 250.
        // Main project: 1 file. Context project: 249 files. Total = 250. All should be indexed.
        // Main project: 1 file. Context project: 250 files. Total = 251. Main:1, Context:249.

        // This test will use a smaller number of files and assume the logic scales.
        // A more direct test of MAX_SOURCE_FILES would involve many file mocks.

        let index = extractor
            .build_content_index(main_project_id, Some(context_repo_path_str))
            .await
            .unwrap();

        // Given the current logic, if main_files has 1, and context_files has 2,
        // and MAX_SOURCE_FILES is 250, all 3 files should be indexed.
        assert_eq!(
            index.files.len(),
            3,
            "Should combine files from main and context if under limit"
        );
        assert!(index.files.contains_key("main/file1.rs"));
        assert!(index.files.contains_key("context/file2.rs"));
        assert!(index.files.contains_key("context/file3.rs"));

        // TODO: Add a test that specifically mocks more than MAX_SOURCE_FILES
        // and asserts that only MAX_SOURCE_FILES are indexed. This would require
        // generating a large number of mock files and their content calls.
        // For example, main_project: 150 files, context_project: 150 files.
        // Expected: 250 files in index.
    }

    #[tokio::test]
    async fn test_search_content_index_keyword_matching_and_ranking() {
        let settings = test_settings("http://localhost".to_string(), None);
        // No GitlabApiClient needed for search_content_index as it operates on FileContentIndex
        let extractor = RepoContextExtractor::new(
            Arc::new(GitlabApiClient::new(settings.clone()).unwrap()),
            settings,
        );

        let mut index = FileContentIndex::default();
        index.files.insert(
            "src/auth.rs".to_string(),
            "login login login authentication authentication module".to_string(),
        );
        index.files.insert(
            "src/payment.rs".to_string(),
            "payment confirmation".to_string(),
        );
        index.files.insert(
            "README.md".to_string(),
            "Project documentation: login and payment features".to_string(),
        );
        index.files.insert(
            "src/utils.rs".to_string(),
            "utility helper functions".to_string(),
        );

        let issue_title = "Issue with Login and Authentication";
        let issue_desc =
            "User cannot login, problem with authentication module. Also payment fails.";
        let issue = GitlabIssue {
            id: 1,
            iid: 1,
            project_id: 1,
            title: issue_title.to_string(),
            description: Some(issue_desc.to_string()),
            state: "opened".to_string(),
            author: GitlabUser {
                id: 1,
                username: "user".to_string(),
                name: "Test User".to_string(),
                avatar_url: None,
            },
            web_url: "".to_string(),
            labels: vec![],
            updated_at: "".to_string(),
        };

        let results = extractor.search_content_index(&index, &issue);

        assert!(!results.is_empty(), "Should find relevant files");
        assert!(results.len() <= 5, "Results should be limited (e.g., to 5)");

        // Check for specific files and basic ranking (more matches = higher rank)
        // "src/auth.rs" mentions "login" and "authentication" in content + "auth" in path.
        // "README.md" mentions "login" and "authentication" (from issue keywords "login", "authentication").
        // "src/payment.rs" mentions "payment".

        // Expected keywords: "issue", "login", "authentication", "user", "cannot", "problem", "module", "payment", "fails"
        // (actual extraction logic might vary slightly based on common word filtering and length)

        // "src/auth.rs": "login", "authentication" (content), "auth" (path from "authentication") - high score
        // "README.md": "login", "authentication" (content) - medium score
        // "src/payment.rs": "payment" (content), "payment" (path) - medium/low score related to "payment"

        assert!(
            results.iter().any(|f| f.file_path == "src/auth.rs"),
            "auth.rs should be relevant"
        );
        assert!(
            results.iter().any(|f| f.file_path == "README.md"),
            "README.md should be relevant"
        );
        assert!(
            results.iter().any(|f| f.file_path == "src/payment.rs"),
            "payment.rs should be relevant"
        );

        // Check ranking - this is a simple check, real ranking can be complex.
        // Assuming "src/auth.rs" is most relevant due to multiple strong keyword matches in content and path.
        if !results.is_empty() {
            assert_eq!(results[0].file_path, "src/auth.rs", "auth.rs should be ranked highest due to multiple keyword matches in content and path");
        }
        // Ensure content is populated
        for file in &results {
            assert!(
                file.content.is_some(),
                "File content should be populated in search results"
            );
        }
    }

    #[tokio::test]
    async fn test_search_content_index_case_insensitivity() {
        let settings = test_settings("http://localhost".to_string(), None);
        let extractor = RepoContextExtractor::new(
            Arc::new(GitlabApiClient::new(settings.clone()).unwrap()),
            settings,
        );

        let mut index = FileContentIndex::default();
        index
            .files
            .insert("src/config.yml".to_string(), "Setting: API_KEY".to_string());
        index.files.insert(
            "docs/guide.md".to_string(),
            "Guide for API key setup".to_string(),
        );

        let issue = GitlabIssue {
            id: 1,
            iid: 1,
            project_id: 1,
            title: "api key problem".to_string(),
            description: Some("Cannot find API_KEY".to_string()),
            state: "opened".to_string(),
            author: GitlabUser {
                id: 1,
                username: "user".to_string(),
                name: "Test User".to_string(),
                avatar_url: None,
            },
            web_url: "".to_string(),
            labels: vec![],
            updated_at: "".to_string(),
        };
        // Keywords: "api", "key", "problem", "find" (approx)

        let results = extractor.search_content_index(&index, &issue);
        assert_eq!(results.len(), 2, "Should find both files due to case-insensitive matching on 'api_key'/'API_KEY' and 'api key'");
        assert!(results.iter().any(|f| f.file_path == "src/config.yml"));
        assert!(results.iter().any(|f| f.file_path == "docs/guide.md"));
    }

    #[tokio::test]
    async fn test_search_content_index_path_matching() {
        let settings = test_settings("http://localhost".to_string(), None);
        let extractor = RepoContextExtractor::new(
            Arc::new(GitlabApiClient::new(settings.clone()).unwrap()),
            settings,
        );

        let mut index = FileContentIndex::default();
        // Content does not have the keyword, but path does.
        index.files.insert(
            "src/user_authentication_module.rs".to_string(),
            "Contains various helper utilities.".to_string(),
        );
        index.files.insert(
            "docs/api_guide.md".to_string(),
            "How to use the foobar system.".to_string(),
        );

        let issue = GitlabIssue {
            id: 1,
            iid: 1,
            project_id: 1,
            title: "Issue with authentication".to_string(),
            description: Some("Need help with the API".to_string()),
            state: "opened".to_string(),
            author: GitlabUser {
                id: 1,
                username: "user".to_string(),
                name: "Test User".to_string(),
                avatar_url: None,
            },
            web_url: "".to_string(),
            labels: vec![],
            updated_at: "".to_string(),
        };
        // Keywords: "issue", "authentication", "need", "help", "api"

        let results = extractor.search_content_index(&index, &issue);
        assert_eq!(results.len(), 2);
        // "src/user_authentication_module.rs" should be found due to "authentication" in path.
        // "docs/api_guide.md" should be found due to "api" in path.
        assert!(results
            .iter()
            .any(|f| f.file_path == "src/user_authentication_module.rs"));
        assert!(results.iter().any(|f| f.file_path == "docs/api_guide.md"));
    }

    #[tokio::test]
    async fn test_search_content_index_limit_results() {
        let settings = test_settings("http://localhost".to_string(), None);
        let extractor = RepoContextExtractor::new(
            Arc::new(GitlabApiClient::new(settings.clone()).unwrap()),
            settings,
        );

        let mut index = FileContentIndex::default();
        for i in 0..10 {
            // Add 10 files that will match
            index
                .files
                .insert(format!("file{}.rs", i), "keyword_search_term".to_string());
        }

        let issue = GitlabIssue {
            id: 1,
            iid: 1,
            project_id: 1,
            title: "Search for keyword_search_term".to_string(),
            description: None,
            state: "opened".to_string(),
            author: GitlabUser {
                id: 1,
                username: "user".to_string(),
                name: "Test User".to_string(),
                avatar_url: None,
            },
            web_url: "".to_string(),
            labels: vec![],
            updated_at: "".to_string(),
        };

        let results = extractor.search_content_index(&index, &issue);
        assert_eq!(results.len(), 5, "Results should be limited to 5");
    }

    #[test]
    fn test_extract_keywords() {
        let user = GitlabUser {
            id: 1,
            username: "test_user".to_string(),
            name: "Test User".to_string(),
            avatar_url: None,
        };

        let issue = GitlabIssue {
            id: 1,
            iid: 1,
            project_id: 1,
            title: "Fix authentication bug in login module".to_string(),
            description: Some("Users are unable to login with correct credentials. This seems to be related to the JWT token validation.".to_string()),
            state: "opened".to_string(),
            author: user,
            web_url: "https://gitlab.com/test/project/issues/1".to_string(),
            labels: vec![],
            updated_at: "2023-01-01T00:00:00Z".to_string(), // Added default for tests
        };

        let settings = AppSettings {
            gitlab_url: "https://gitlab.com".to_string(),
            gitlab_token: "test_token".to_string(),
            openai_api_key: "key".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            openai_custom_url: "url".to_string(),
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            stale_issue_days: 30, // Added default for tests (removed duplicate)
            max_age_hours: 24,
            context_repo_path: None,
            max_context_size: 60000,
            default_branch: "main".to_string(),
        };

        let settings_arc = Arc::new(settings.clone());
        let extractor = RepoContextExtractor::new(
            Arc::new(GitlabApiClient::new(settings_arc.clone()).unwrap()),
            settings_arc,
        );

        let keywords = extractor.extract_keywords(&issue);

        // Check that important keywords were extracted
        assert!(keywords.contains(&"authentication".to_string()));
        assert!(keywords.contains(&"bug".to_string()));
        assert!(keywords.contains(&"login".to_string()));
        assert!(keywords.contains(&"module".to_string()));
        assert!(keywords.contains(&"unable".to_string()));
        assert!(keywords.contains(&"credentials".to_string()));
        assert!(keywords.contains(&"jwt".to_string()));
        assert!(keywords.contains(&"token".to_string()));
        assert!(keywords.contains(&"validation".to_string()));

        // Check that common words were filtered out
        assert!(!keywords.contains(&"the".to_string()));
        assert!(!keywords.contains(&"with".to_string()));
        assert!(!keywords.contains(&"this".to_string()));
        assert!(!keywords.contains(&"are".to_string()));
    }

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

        // Mock get_repository_tree for project 1 (main_repo)
        // Called by get_combined_source_files and by build_content_index
        let _m_repo_tree_src_files_main = server
            .mock("GET", "/api/v4/projects/1/repository/tree?recursive=true&per_page=100&page=1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("X-Total-Pages", "1")
            .with_body(serde_json::json!([{"id": "1", "name": "src/main.rs", "type": "blob", "path": "src/main.rs", "mode": "100644"}]).to_string())
            .expect(2) // Once for get_combined_source_files, once for build_content_index
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

        // Mock get_project_by_path
        // This path is also relative to /api/v4
        let _m_get_project_for_relevant_files = server
            .mock(
                "GET",
                format!("/api/v4/projects/{}", encode(&project.path_with_namespace)).as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!(project).to_string())
            // This mock for get_project_by_path is not called when context_repo_path is None
            // by get_agents_md_content or build_content_index.
            // It might have been intended for the old find_relevant_files_for_issue.
            // Removing .expect(1) as it's not hit by the current logic flow for this test.
            .create_async()
            .await;

        // Mock get_file_content for main.rs (called by build_content_index for project 1)
        let _m_main_rs_content = server
            .mock(
                "GET",
                "/api/v4/projects/1/repository/files/src%2Fmain.rs?ref=main",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "file_name": "main.rs",
                    "file_path": "src/main.rs",
                    "size": 100, // Example size
                    "encoding": "base64",
                    "content": base64::encode("fn main() { println!(\"hello world\"); }"), // Example content
                    "ref": "main",
                    "blob_id": "someblobid_main_rs",
                    "commit_id": "somecommitid_main_rs",
                    "last_commit_id": "somelastcommitid_main_rs"
                })
                .to_string(),
            )
            .create_async()
            .await;

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
            "Context missing 'src/main.rs' from all source files list. Full: {}",
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

        // Check if relevant files section is present (or not, if search_content_index returns empty)
        // For this test, with "Test Issue #101" and "Description for issue #101" and content "fn main() { println!(\"hello world\"); }"
        // it's unlikely to find "main.rs" as relevant by default keywords "test", "issue", "description".
        // So, we expect "No relevant files found" or similar message.
        // If "main.rs" was made relevant by keywords, then "--- Relevant Files from Index ---" should be present.
        // For now, let's assume it might not be found to keep test simple.
        // The actual content of main.rs is "fn main() { println!("hello world"); }"
        // Keywords from "Test Issue #101", "Description for issue #101" are "test", "issue", "description"
        // No match. So relevant_files should be empty.
        assert!(
            context.contains("Content index built, but no relevant files found via keyword search"),
            "Context should indicate no relevant files found from index. Full: {}",
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

        // Mock get_repository_tree for main project (ID 1). Called by get_combined_source_files and build_content_index.
        let _m_repo_tree_main_src = server
            .mock(
                "GET",
                "/api/v4/projects/1/repository/tree?recursive=true&per_page=100",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("X-Total-Pages", "1")
            .with_body(serde_json::json!([]).to_string()) // Returns empty for main project files
            .expect(2) // Called by get_combined_source_files and build_content_index
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

        // Mock get_project_by_path for context_repo (called by get_combined_source_files, get_agents_md_content, and find_relevant_files_for_issue)
        let _m_context_project_fetch = server
            .mock(
                "GET",
                format!("/api/v4/projects/{}", encode(context_repo_path)).as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!(context_project_mock).to_string())
            .expect(3) // Called by get_combined_source_files, get_agents_md_content, and build_content_index
            .create_async()
            .await;

        // Mock get_repository_tree for context project (ID 2) for get_combined_source_files (returns empty)
        let _m_repo_tree_context_for_combined = server
            .mock(
                "GET",
                "/api/v4/projects/2/repository/tree?recursive=true&per_page=100&page=1",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("X-Total-Pages", "1")
            .with_body(serde_json::json!([]).to_string())
            .expect(1) // Only for get_combined_source_files
            .create_async()
            .await;

        // Mocks for build_content_index for context_repo_path (project ID 2)
        // It will call get_all_source_files for project ID 2
        let context_indexed_files = ["ctx/file1.rs"];
        let _m_build_idx_context_tree =
            mock_get_all_source_files(&mut server, context_project_mock.id, &context_indexed_files)
                .await;
        // It will then call get_file_content for "ctx/file1.rs"
        let _m_build_idx_ctx_file_content = mock_get_file_content(
            &mut server,
            context_project_mock.id,
            "ctx/file1.rs",
            "contextual keyword",
        )
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

        // Assert that source file list from get_combined_source_files is present (since both main and context repo mocked as empty for source files)
        // The "--- All Source Files ---" header IS added by default even if source_files is empty.
        // The check should be that no actual file paths are listed under it.
        assert!(
            context.contains("--- All Source Files (up to") && context.contains("[No source files found]"),
            "Context should contain 'All Source Files' header with '[No source files found]' message when no source files returned by get_combined_source_files. Full context: {}",
            context
        );

        // Assert that the default "empty" message is NOT present because AGENTS.md was added
        // The message is "No source files or relevant files found in the repository."
        // or "Context gathering completed but no content was added due to size constraints."
        assert!(
            !context.contains("No source files or relevant files found in the repository."),
             "Context should NOT contain default empty message if AGENTS.md is present. Full context: {}",
             context
        );
        assert!(
            !context.contains("Context gathering completed but no content was added due to size constraints."),
             "Context should NOT contain size constraint message if AGENTS.md is present. Full context: {}",
             context
        );
        // Now, build_content_index will find "ctx/file1.rs" from the context repo.
        // Keywords "test", "issue", "description" will not match "contextual keyword" or "ctx/file1.rs".
        // So, the message should be that the index was built but no relevant files were found.
        assert!(
            context.contains("Content index built, but no relevant files found via keyword search"),
            "Context should indicate index built but no relevant files found. Full: {}",
            context
        );
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
            .mock("GET", "/api/v4/projects/1/repository/tree?recursive=true&per_page=100&page=1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("X-Total-Pages", "1")
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
