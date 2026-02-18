#[cfg(test)]
mod tests {
    use crate::config::AppSettings;
    use crate::file_indexer::FileIndexManager;
    use crate::gitlab::GitlabApiClient;
    use crate::repo_context::RepoContextExtractor;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn test_extract_relevant_file_sections_perf() {
        // Setup minimal extractor
        let settings = AppSettings {
            context_lines: 5,
            ..Default::default()
        };
        let settings_arc = Arc::new(settings);
        // We use a dummy client since we won't be making network calls,
        // but we need to construct it properly to satisfy types.
        let mut minimal_settings = AppSettings::default();
        minimal_settings.gitlab_url = "https://example.com".to_string();
        minimal_settings.gitlab_token = "dummy".to_string();
        minimal_settings.openai_api_key = "dummy".to_string();

        let valid_settings = Arc::new(minimal_settings);
        let gitlab_client = Arc::new(GitlabApiClient::new(valid_settings.clone()).unwrap());
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let extractor = RepoContextExtractor::new_with_file_indexer(
            gitlab_client,
            settings_arc, // This one is used for context_lines
            file_index_manager,
        );

        // Generate large content
        // 100,000 lines, ~50 chars per line -> ~5MB
        let line_count = 100_000;
        let mut content = String::with_capacity(line_count * 60);
        for i in 0..line_count {
            if i % 1000 == 0 {
                content.push_str("This line contains the magic keyword TARGET.\n");
            } else {
                content.push_str(
                    "This is a regular line of code with some content that is not relevant.\n",
                );
            }
        }

        let keywords = vec!["TARGET".to_string()];

        println!("Starting benchmark with {} lines...", line_count);
        let start = Instant::now();

        // Run multiple times to average? Or just once for large enough dataset.
        // 100k lines should be enough to see difference.
        let _matches = extractor.extract_relevant_file_sections(&content, &keywords);

        let duration = start.elapsed();
        println!("Extraction took: {:?}", duration);

        // Sanity check
        assert!(!_matches.is_empty());
    }
}
