use crate::gitlab::GitlabApiClient;
use crate::models::{GitlabIssue, GitlabMergeRequest, GitlabProject};

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, info};

const MAX_CONTEXT_SIZE: usize = 10000; // Maximum characters of context to include

#[derive(Debug, Deserialize)]
pub struct GitlabFile {
    pub file_name: String,
    pub file_path: String,
    pub size: usize,
    pub content: Option<String>,
    pub encoding: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitlabDiff {
    pub old_path: String,
    pub new_path: String,
    pub diff: String,
}

pub struct RepoContextExtractor {
    gitlab_client: Arc<GitlabApiClient>,
}

impl RepoContextExtractor {
    pub fn new(gitlab_client: Arc<GitlabApiClient>) -> Self {
        Self { gitlab_client }
    }

    /// Extract relevant context from a repository for an issue
    pub async fn extract_context_for_issue(
        &self,
        issue: &GitlabIssue,
        project: &GitlabProject,
        context_repo_path: Option<&str>,
    ) -> Result<String> {
        // Determine which repository to use for context
        let repo_path = if let Some(context_path) = context_repo_path {
            context_path
        } else {
            &project.path_with_namespace
        };

        info!("Extracting context for issue #{} from repo {}", issue.iid, repo_path);
        
        // Get repository files that might be relevant to the issue
        let relevant_files = self.find_relevant_files_for_issue(issue, repo_path).await?;
        
        // Format the context
        let mut context = String::new();
        let mut total_size = 0;
        
        for file in relevant_files {
            if let Some(content) = file.content {
                let file_context = format!(
                    "\n--- File: {} ---\n{}\n",
                    file.file_path,
                    content
                );
                
                // Check if adding this file would exceed our context limit
                if total_size + file_context.len() > MAX_CONTEXT_SIZE {
                    // If we're about to exceed the limit, add a truncation notice
                    context.push_str("\n[Additional files omitted due to context size limits]\n");
                    break;
                }
                
                context.push_str(&file_context);
                total_size += file_context.len();
            }
        }
        
        if context.is_empty() {
            context = "No relevant files found in the repository.".to_string();
        }
        
        Ok(context)
    }
    
    /// Extract diff context for a merge request
    pub async fn extract_context_for_mr(
        &self,
        mr: &GitlabMergeRequest,
        project: &GitlabProject,
    ) -> Result<String> {
        info!("Extracting diff context for MR !{} in {}", mr.iid, project.path_with_namespace);
        
        let diffs = self.gitlab_client
            .get_merge_request_changes(project.id, mr.iid)
            .await
            .context("Failed to get merge request changes")?;
        
        let mut context = String::new();
        let mut total_size = 0;
        
        for diff in diffs {
            let diff_context = format!(
                "\n--- Changes in {} ---\n{}\n",
                diff.new_path,
                diff.diff
            );
            
            // Check if adding this diff would exceed our context limit
            if total_size + diff_context.len() > MAX_CONTEXT_SIZE {
                context.push_str("\n[Additional diffs omitted due to context size limits]\n");
                break;
            }
            
            context.push_str(&diff_context);
            total_size += diff_context.len();
        }
        
        if context.is_empty() {
            context = "No changes found in this merge request.".to_string();
        }
        
        Ok(context)
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
        debug!("Extracted keywords for issue #{}: {:?}", issue.iid, keywords);
        
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
        let top_files: Vec<String> = scored_files.into_iter()
            .take(5) // Limit to 5 most relevant files
            .map(|(path, _)| path)
            .collect();
        
        // Fetch content for top files
        let mut files_with_content = Vec::new();
        for file_path in top_files {
            match self.gitlab_client.get_file_content(project.id, &file_path).await {
                Ok(file) => files_with_content.push(file),
                Err(e) => debug!("Failed to get content for file {}: {}", file_path, e),
            }
        }
        
        Ok(files_with_content)
    }
    
    /// Extract keywords from issue title and description
    fn extract_keywords(&self, issue: &GitlabIssue) -> Vec<String> {
        let mut text = issue.title.clone();
        if let Some(desc) = &issue.description {
            text.push_str(" ");
            text.push_str(desc);
        }
        
        // Convert to lowercase and split by non-alphanumeric characters
        let words: Vec<String> = text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty() && s.len() > 2) // Filter out empty strings and very short words
            .map(|s| s.to_string())
            .collect();
        
        // Remove common words
        let common_words = [
            "the", "and", "for", "this", "that", "with", "from", "have", "not", 
            "but", "what", "all", "are", "when", "your", "can", "has", "been",
        ];
        
        words.into_iter()
            .filter(|word| !common_words.contains(&word.as_str()))
            .collect()
    }
    
    /// Calculate relevance score of a file to the keywords
    fn calculate_relevance_score(&self, file_path: &str, keywords: &[String]) -> usize {
        let path_lower = file_path.to_lowercase();
        
        // Skip binary files and non-code files
        let binary_extensions = [
            ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".ico", ".svg", 
            ".pdf", ".zip", ".tar", ".gz", ".rar", ".exe", ".dll", ".so",
            ".bin", ".dat", ".db", ".sqlite", ".mp3", ".mp4", ".avi", ".mov",
        ];
        
        if binary_extensions.iter().any(|ext| path_lower.ends_with(ext)) {
            return 0;
        }
        
        // Calculate score based on keyword matches
        let mut score = 0;
        
        // Prefer documentation files
        if path_lower.contains("readme") || 
           path_lower.contains("docs/") || 
           path_lower.ends_with(".md") {
            score += 5;
        }
        
        // Prefer source code files
        let code_extensions = [
            ".rs", ".py", ".js", ".ts", ".java", ".c", ".cpp", ".h", ".hpp",
            ".go", ".rb", ".php", ".cs", ".scala", ".kt", ".swift", ".sh",
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
        };
        
        let settings = AppSettings {
            gitlab_url: "https://gitlab.com".to_string(),
            gitlab_token: "test_token".to_string(),
            openai_api_key: "key".to_string(),
            openai_custom_url: "url".to_string(),
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            context_repo_path: None,
        };
        
        let extractor = RepoContextExtractor::new(Arc::new(GitlabApiClient::new(&settings).unwrap()));
        
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
            gitlab_url: "https://gitlab.com".to_string(),
            gitlab_token: "test_token".to_string(),
            openai_api_key: "key".to_string(),
            openai_custom_url: "url".to_string(),
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            context_repo_path: None,
        };
        
        let extractor = RepoContextExtractor::new(Arc::new(GitlabApiClient::new(&settings).unwrap()));
        
        let keywords = vec![
            "authentication".to_string(),
            "login".to_string(),
            "jwt".to_string(),
        ];
        
        // Test scoring for different file paths
        let scores = [
            ("src/auth/login.rs", extractor.calculate_relevance_score("src/auth/login.rs", &keywords)),
            ("README.md", extractor.calculate_relevance_score("README.md", &keywords)),
            ("docs/authentication.md", extractor.calculate_relevance_score("docs/authentication.md", &keywords)),
            ("src/utils.rs", extractor.calculate_relevance_score("src/utils.rs", &keywords)),
            ("image.png", extractor.calculate_relevance_score("image.png", &keywords)),
        ];
        
        // Check that relevant files have higher scores
        assert!(scores[0].1 > 0); // auth/login.rs should have high score
        assert!(scores[2].1 > 0); // authentication.md should have high score
        assert_eq!(scores[4].1, 0); // image.png should have zero score (binary file)
        
        // Check relative scoring
        assert!(scores[0].1 > scores[3].1); // login.rs should score higher than utils.rs
        assert!(scores[2].1 > scores[1].1); // authentication.md should score higher than README.md
    }
}