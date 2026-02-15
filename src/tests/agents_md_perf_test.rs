#[cfg(test)]
mod tests {
    use crate::config::AppSettings;
    use crate::file_indexer::FileIndexManager;
    use crate::gitlab::GitlabApiClient;
    use crate::models::GitlabProject;
    use crate::repo_context::*;
    use std::sync::Arc;
    use std::time::Instant;

    fn test_settings(gitlab_url: String) -> Arc<AppSettings> {
        let mut settings = AppSettings::default();
        settings.gitlab_url = gitlab_url;
        settings.gitlab_token = "test_token".to_string();
        settings.openai_api_key = "test_openai_key".to_string();
        settings.repos_to_poll = vec!["test_org/test_repo".to_string()];
        settings.log_level = "debug".to_string();
        settings.bot_username = "test_bot".to_string();
        Arc::new(settings)
    }

    fn create_mock_project(id: i64, path_with_namespace: &str) -> GitlabProject {
        GitlabProject {
            id,
            path_with_namespace: path_with_namespace.to_string(),
            web_url: format!("https://gitlab.com/{}", path_with_namespace),
        }
    }

    #[tokio::test]
    async fn benchmark_get_agents_md_content() {
        let mut server = mockito::Server::new_async().await;
        let settings = test_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(settings.clone()).unwrap());
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));
        let extractor = RepoContextExtractor::new_with_file_indexer(
            gitlab_client.clone(),
            settings.clone(),
            file_index_manager,
        );

        let project = create_mock_project(1, "test_org/main_repo");
        let agents_md_content = "This is the AGENTS.md content from main_repo.";

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
            .expect(1) // Expect exactly 1 call due to caching
            .create_async()
            .await;

        let start = Instant::now();
        for _ in 0..100 {
            let _ = extractor
                .get_agents_md_content(&project, None)
                .await
                .unwrap();
        }
        let duration = start.elapsed();

        println!("Time taken for 100 calls: {:?}", duration);
    }
}
