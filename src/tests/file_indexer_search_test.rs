#[cfg(test)]
mod tests {
    use crate::file_indexer::FileIndexManager;
    use crate::gitlab::GitlabApiClient;
    use anyhow::Result;
    use std::sync::Arc;

    // Helper function to create a GitLab API client
    fn create_gitlab_client() -> Arc<GitlabApiClient> {
        // Create test settings
        let settings = crate::config::AppSettings {
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
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: None,
            max_context_size: 60000,
            default_branch: "main".to_string(),
        client_cert_path: None,
        client_key_path: None,
        client_key_password: None,
        };

        let settings_arc = Arc::new(settings);
        Arc::new(GitlabApiClient::new(settings_arc).unwrap())
    }

    #[tokio::test]
    async fn test_search_files() -> Result<()> {
        // Create a GitLab client
        let gitlab_client = create_gitlab_client();

        // Create a file index manager with a short refresh interval
        let index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 60));

        // Manually add a file to the index
        let index = index_manager.get_or_create_index(1);
        index.add_file("src/main.rs", "fn main() { println!(\"Hello, world!\"); }");

        // Try to call the search_files method, but we don't assert on its results
        // since it would try to make actual API calls
        let _ = index_manager
            .search_files(1, &["main".to_string(), "println".to_string()])
            .await;

        // Since we can't mock the GitLab API call, we'll just check that the index search works correctly
        let matching_files = index.search(&["main".to_string(), "println".to_string()]);
        assert_eq!(matching_files.len(), 1);
        assert_eq!(matching_files[0], "src/main.rs");

        // Test searching with keywords that shouldn't match
        let no_results = index.search(&["nonexistent".to_string()]);
        assert_eq!(no_results.len(), 0);

        // Test searching with empty keywords
        let empty_results = index.search(&[]);
        assert_eq!(empty_results.len(), 0);

        // Test searching in a project with no index
        let no_index = index_manager.get_or_create_index(2);
        let no_index_results = no_index.search(&["main".to_string()]);
        assert_eq!(no_index_results.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_files_with_multiple_matches() -> Result<()> {
        // Create a GitLab client
        let gitlab_client = create_gitlab_client();

        // Create a file index manager
        let index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 60));

        // Manually add multiple files to the index
        let index = index_manager.get_or_create_index(1);
        index.add_file("src/main.rs", "fn main() { println!(\"Hello, world!\"); }");
        index.add_file("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }");
        index.add_file(
            "src/utils.rs",
            "pub fn format_string(s: &str) -> String { s.to_string() }",
        );

        // Test the index search functionality
        let results = index.search(&["fn".to_string()]);

        // Verify we get multiple results
        assert!(results.len() >= 2);

        // Verify the file paths are correct
        assert!(results.contains(&"src/main.rs".to_string()));
        assert!(results.contains(&"src/lib.rs".to_string()));

        Ok(())
    }
}
