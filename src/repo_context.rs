use crate::config::AppSettings;
use crate::gitlab::GitlabApiClient;
use crate::models::{GitlabIssue, GitlabMergeRequest, GitlabProject};

use anyhow::{Context, Result};
use serde::Deserialize;
use std::error::Error; // Added for AGENTS.md error handling
use std::sync::Arc;
use tracing::{debug, info, warn};

const MAX_SOURCE_FILES: usize = 250; // Maximum number of source files to include in context

#[derive(Debug, Deserialize)]
pub struct GitlabFile {
    // pub file_name: String, // Removed unused field
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

    /// Get the content of a specific file (e.g. AGENTS.md) from the repository root
    async fn get_agents_md_context(&self, project_id: i64, file_path: &str) -> Result<Option<String>> {
        match self.gitlab_client.get_file_content(project_id, file_path).await {
            Ok(file) => {
                // Content is already decoded by gitlab_client.get_file_content if it was base64
                if file.content.is_some() {
                    return Ok(file.content);
                }
                Ok(None) // No content
            }
            Err(e) => {
                // Check if the error is a 404 Not Found by iterating through sources
                let mut source_err = e.source();
                while let Some(s_err) = source_err {
                    if let Some(gitlab_error) = s_err.downcast_ref::<crate::gitlab::GitlabError>() {
                        if matches!(gitlab_error, crate::gitlab::GitlabError::Api { status, .. } if *status == reqwest::StatusCode::NOT_FOUND) {
                            debug!("File {} not found for project {}: {}", file_path, project_id, gitlab_error);
                            return Ok(None); // File not found is not an error for this context
                        }
                        // Found GitlabError, no need to look further up the chain for this specific check.
                        break; 
                    }
                    source_err = s_err.source();
                }
                // For other errors (or if GitlabError was found but not a 404), log and treat as if file not found for context purposes.
                warn!("Failed to get {} for project {}: {}", file_path, project_id, e);
                Ok(None)
            }
        }
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
        // Get source files from the main project
        let mut all_files = self.get_all_source_files(project.id).await?;

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
        Ok(all_files)
    }

    /// Extract relevant context from a repository for an issue
    pub async fn extract_context_for_issue(
        &self,
        issue: &GitlabIssue,
        main_issue_project: &GitlabProject, // Project object for the issue's own repository
        context_repo_path_option: Option<&str>, // Optional path to a different repository for context
    ) -> Result<String> {
        let mut agents_md_combined_content = String::new();
        let mut agents_md_total_size = 0;

        // 1. Get AGENTS.md from the main project (where the issue resides)
        if let Ok(Some(content)) = self.get_agents_md_context(main_issue_project.id, "AGENTS.md").await {
            let formatted_content = format!(
                "\n--- AGENTS.md (Main Repo: {}) ---\n{}\n",
                main_issue_project.path_with_namespace, content
            );
            agents_md_combined_content.push_str(&formatted_content);
            agents_md_total_size += formatted_content.len();
        }

        // 2. Get AGENTS.md from the context_repo if specified and different from the main_issue_project
        if let Some(context_path_str) = context_repo_path_option {
            if context_path_str != main_issue_project.path_with_namespace {
                match self.gitlab_client.get_project_by_path(context_path_str).await {
                    Ok(context_project_details) => {
                        if let Ok(Some(content)) = self.get_agents_md_context(context_project_details.id, "AGENTS.md").await {
                            let formatted_content = format!(
                                "\n--- AGENTS.md (Context Repo: {}) ---\n{}\n",
                                context_path_str, content
                            );
                            agents_md_combined_content.push_str(&formatted_content);
                            agents_md_total_size += formatted_content.len();
                        }
                    }
                    Err(e) => {
                        warn!("Failed to get project details for context repo {}: {}", context_path_str, e);
                    }
                }
            }
        }
        
        // Determine the active project from which to fetch source files and relevant files.
        let path_for_files_context = context_repo_path_option.unwrap_or(&main_issue_project.path_with_namespace);
        let active_project_for_files = self.gitlab_client.get_project_by_path(path_for_files_context).await?;

        info!(
            "Extracting file context for issue #{} from primary repo for files: {}",
            issue.iid, active_project_for_files.path_with_namespace
        );
        
        let mut context_string = agents_md_combined_content;
        let mut total_size = agents_md_total_size;

        // First add the list of all source files from both projects
        let source_files = self
            .get_combined_source_files(project, context_repo_path)
            .await?;

        if !source_files.is_empty() {
            let files_list_header = format!(
                "\n--- All Source Files from {} (up to {} files) ---\n",
                active_project_for_files.path_with_namespace, MAX_SOURCE_FILES
            );
            let files_list_content = source_files.join("\n");
            let files_list = format!("{}{}\n", files_list_header, files_list_content);

            if total_size + files_list.len() <= self.settings.max_context_size {
                context_string.push_str(&files_list);
                total_size += files_list.len();
            } else {
                 context_string.push_str(&format!("\n[Source files list from {} omitted due to context size limits]\n", active_project_for_files.path_with_namespace));
            }
        }
        
        // Get repository files that might be relevant to the issue (from active_project_for_files)
        let relevant_files = self.find_relevant_files_for_issue(issue, &active_project_for_files.path_with_namespace).await?;

        // Then add relevant file contents
        for file in relevant_files {
            if let Some(content) = file.content {
                let file_context = format!("\n--- File: {} ---\n{}\n", file.file_path, content);

                // Check if adding this file would exceed our context limit
                if total_size + file_context.len() > self.settings.max_context_size {
                    context_string.push_str("\n[Additional files omitted due to context size limits]\n");
                    break;
                }

                context_string.push_str(&file_context);
                total_size += file_context.len();
            }
        }

        if context_string.is_empty() {
            context_string = "No source files or relevant files found in the repository.".to_string();
        }

        Ok(context_string)
    }

    /// Extract diff context for a merge request
        for file in relevant_files {
            if let Some(content) = file.content {
                let file_context = format!("\n--- File: {} ---\n{}\n", file.file_path, content);

                // Check if adding this file would exceed our context limit
                if total_size + file_context.len() > self.settings.max_context_size {
                    // If we're about to exceed the limit, add a truncation notice
                    context.push_str("\n[Additional files omitted due to context size limits]\n");
                    break;
                }

                context.push_str(&file_context);
                total_size += file_context.len();
            }
        }

        if context.is_empty() {
            context = "No source files or relevant files found in the repository.".to_string();
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
            "Extracting diff context for MR !{} in {}",
            mr.iid, project.path_with_namespace
        );

        let mut agents_md_for_llm = String::new();
        let mut agents_md_size = 0;
        if let Ok(Some(content)) = self.get_agents_md_context(project.id, "AGENTS.md").await {
            let formatted_content = format!(
                "\n--- AGENTS.md ---\n{}\n",
                content
            );
            agents_md_for_llm.push_str(&formatted_content);
            agents_md_size += formatted_content.len();
        }

        let mut context_for_llm = agents_md_for_llm;
        let mut context_for_comment = String::new(); // AGENTS.md is not added to comments
        let mut total_size = agents_md_size;

        // First add the list of all source files
        let source_files = self
            .get_combined_source_files(project, context_repo_path)
            .await?;

        if !source_files.is_empty() {
            let files_list_header = format!(
                "\n--- All Source Files from {} (up to {} files) ---\n",
                 project.path_with_namespace, MAX_SOURCE_FILES
            );
            let files_list_content = source_files.join("\n");
            let files_list = format!("{}{}\n", files_list_header, files_list_content);

            if total_size + files_list.len() <= self.settings.max_context_size {
                context_for_llm.push_str(&files_list);
                total_size += files_list.len();
            } else {
                context_for_llm.push_str(&format!("\n[Source files list from {} omitted due to context size limits]\n", project.path_with_namespace));
            }
        }

        // Then add the diff context and file history
        let diffs = self
            .gitlab_client
            .get_merge_request_changes(project.id, mr.iid)
            .await
            .context("Failed to get merge request changes")?;

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
                    warn!("Failed to get commit history for {}: {}", diff.new_path, e);
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

        if context_for_llm.is_empty() {
            context_for_llm = "No source files or changes found in this merge request.".to_string();
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
        let project = self.gitlab_client.get_project_by_path(repo_path).await?;

        // Extract keywords from issue title and description
        let keywords = self.extract_keywords(issue);
        debug!(
            "Extracted keywords for issue #{}: {:?}",
            issue.iid, keywords
        );

        // Get repository file tree
        let files = self.gitlab_client.get_repository_tree(project.id).await?;

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

        // Take top N most relevant files
        let top_files: Vec<String> = scored_files
            .into_iter()
            .take(5) // Limit to 5 most relevant files
            .map(|(path, _)| path)
            .collect();

        // Fetch content for top files
        let mut files_with_content = Vec::new();
        for file_path in top_files {
            match self
                .gitlab_client
                .get_file_content(project.id, &file_path)
                .await
            {
                Ok(file) => files_with_content.push(file),
                Err(e) => warn!("Failed to get content for file {}: {}", file_path, e),
            }
        }

        Ok(files_with_content)
    }

    /// Extract keywords from issue title and description
    fn extract_keywords(&self, issue: &GitlabIssue) -> Vec<String> {
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
    fn calculate_relevance_score(&self, file_path: &str, keywords: &[String]) -> usize {
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
                ".php", ".cs", ".scala", ".kt", ".swift", ".sh",
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::models::GitlabUser;

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
        };

        let extractor = RepoContextExtractor::new(
            Arc::new(GitlabApiClient::new(&settings).unwrap()),
            Arc::new(settings.clone()),
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

    #[test]
    fn test_calculate_relevance_score() {
        let settings = AppSettings {
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            gitlab_url: "https://gitlab.com".to_string(),
            gitlab_token: "test_token".to_string(),
            openai_api_key: "key".to_string(),
            openai_custom_url: "url".to_string(),
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: None,
            max_context_size: 60000,
        };

        let extractor = RepoContextExtractor::new(
            Arc::new(GitlabApiClient::new(&settings).unwrap()),
            Arc::new(settings.clone()),
        );

        let keywords = vec![
            "authentication".to_string(),
            "login".to_string(),
            "jwt".to_string(),
        ];

        // Test scoring for different file paths
        let scores = [
            (
                "src/auth/login.rs",
                extractor.calculate_relevance_score("src/auth/login.rs", &keywords),
            ),
            (
                "README.md",
                extractor.calculate_relevance_score("README.md", &keywords),
            ),
            (
                "docs/authentication.md",
                extractor.calculate_relevance_score("docs/authentication.md", &keywords),
            ),
            (
                "src/utils.rs",
                extractor.calculate_relevance_score("src/utils.rs", &keywords),
            ),
            (
                "image.png",
                extractor.calculate_relevance_score("image.png", &keywords),
            ),
        ];

        // Check that relevant files have higher scores
        assert!(scores[0].1 > 0); // auth/login.rs should have high score
        assert!(scores[2].1 > 0); // authentication.md should have high score
        assert!(scores[1].1 == 0); // README.md should have no score
        assert!(scores[3].1 == 0); // utils.rs should have no score
        assert!(scores[4].1 == 0); // image.png should have no score
    }
}
