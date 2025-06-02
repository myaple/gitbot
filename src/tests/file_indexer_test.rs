#[cfg(test)]
mod tests {
    use crate::file_indexer::{FileContentIndex, FileIndexManager};
    use crate::gitlab::GitlabApiClient;
    use crate::models::GitlabProject;
    use std::sync::Arc;

    #[test]
    fn test_file_content_index() {
        let index = FileContentIndex::new(1);

        // Add files to the index
        index.add_file("src/main.rs", "fn main() { println!(\"Hello, world!\"); }");
        index.add_file("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }");
        index.add_file("README.md", "# Project\nThis is a test project.");

        // Test searching with single keyword
        let results = index.search(&["main".to_string()]);
        assert_eq!(results.len(), 1);
        assert!(results.contains(&"src/main.rs".to_string()));

        // Test searching with multiple keywords
        let results = index.search(&["fn".to_string(), "println".to_string()]);
        assert_eq!(results.len(), 1);
        assert!(results.contains(&"src/main.rs".to_string()));

        // Test searching with keywords that match multiple files
        let results = index.search(&["fn".to_string()]);
        assert!(!results.is_empty());
        assert!(results.contains(&"src/main.rs".to_string()));
        assert!(results.contains(&"src/lib.rs".to_string()));

        // Test searching with non-existent keyword
        let results = index.search(&["nonexistent".to_string()]);
        assert_eq!(results.len(), 0);

        // Test removing a file
        index.remove_file("src/main.rs");
        let results = index.search(&["main".to_string()]);
        assert_eq!(results.len(), 0);

        // Verify the other file is still indexed
        let results = index.search(&["add".to_string()]);
        assert_eq!(results.len(), 1);
        assert!(results.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_should_index_file() {
        assert!(FileContentIndex::should_index_file("src/main.rs"));
        assert!(FileContentIndex::should_index_file("lib/utils.js"));
        assert!(FileContentIndex::should_index_file("docs/README.md"));
        assert!(!FileContentIndex::should_index_file("images/logo.png"));
        assert!(!FileContentIndex::should_index_file("build/app.exe"));
        assert!(!FileContentIndex::should_index_file("data.json"));
    }

    #[test]
    fn test_generate_ngrams() {
        let ngrams = FileContentIndex::generate_ngrams("hello");
        assert_eq!(ngrams.len(), 3);
        assert!(ngrams.contains("hel"));
        assert!(ngrams.contains("ell"));
        assert!(ngrams.contains("llo"));

        // Test with short text
        let short_ngrams = FileContentIndex::generate_ngrams("hi");
        assert_eq!(short_ngrams.len(), 1);
        assert!(short_ngrams.contains("hi"));

        // Test with mixed case
        let mixed_ngrams = FileContentIndex::generate_ngrams("Hello");
        assert_eq!(mixed_ngrams.len(), 3);
        assert!(mixed_ngrams.contains("hel"));
        assert!(!mixed_ngrams.contains("Hel"));
    }

    // We'll skip the mock test for now since it requires more setup
    #[test]
    fn test_file_index_manager_basic() {
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
        };

        let settings_arc = Arc::new(settings);
        let gitlab_client = Arc::new(GitlabApiClient::new(settings_arc).unwrap());

        // Create a test project
        let _project = GitlabProject {
            id: 1,
            path_with_namespace: "org/repo1".to_string(),
            web_url: "https://gitlab.com/org/repo1".to_string(),
        };

        // Create file index manager with a short refresh interval
        let index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 60));

        // Test getting or creating an index
        let _index = index_manager.get_or_create_index(1);
    }

    #[test]
    fn test_content_hash() {
        let content1 = "fn main() { println!(\"Hello, world!\"); }";
        let content2 = "fn main() { println!(\"Hello, world!\"); }";
        let content3 = "fn main() { println!(\"Hello, Rust!\"); }";

        let hash1 = FileContentIndex::calculate_content_hash(content1);
        let hash2 = FileContentIndex::calculate_content_hash(content2);
        let hash3 = FileContentIndex::calculate_content_hash(content3);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}
