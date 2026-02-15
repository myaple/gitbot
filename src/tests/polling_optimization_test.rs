#[cfg(test)]
mod tests {
    use crate::config::AppSettings;
    use crate::file_indexer::FileIndexManager;
    use crate::gitlab::GitlabApiClient;
    use crate::models::GitlabProject;
    use crate::polling::PollingService;
    use mockito::Matcher;
    use serde_json::json;
    use std::sync::Arc;

    fn test_config(bot_username: &str, base_url: String) -> Arc<AppSettings> {
        let mut settings = AppSettings::default();
        settings.gitlab_url = base_url;
        settings.gitlab_token = "test_token".to_string();
        settings.openai_api_key = "key".to_string();
        settings.repos_to_poll = vec!["org/repo1".to_string()];
        settings.bot_username = bot_username.to_string();
        settings.poll_interval_seconds = 1;
        Arc::new(settings)
    }

    #[tokio::test]
    async fn test_poll_issues_n_plus_one_repro() {
        let mut server = mockito::Server::new_async().await;
        let config = test_config("test_bot", server.url());
        let client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let file_indexer = Arc::new(FileIndexManager::new(client.clone(), 3600));

        let service = PollingService::new(
            client.clone(),
            config.clone(),
            file_indexer,
            None,
        );

        let project_id = 1;
        let project = GitlabProject {
            id: project_id,
            path_with_namespace: "org/repo1".to_string(),
            web_url: "url".to_string(),
        };

        // Mock GraphQL response
        let graphql_response = json!({
            "data": {
                "project": {
                    "issues": {
                        "nodes": [
                            {
                                "id": "gid://gitlab/Issue/10",
                                "iid": "1",
                                "title": "Test Issue 1",
                                "description": "desc",
                                "state": "opened",
                                "webUrl": "url",
                                "updatedAt": "2024-01-02T00:00:00Z",
                                "author": {
                                    "id": "gid://gitlab/User/100",
                                    "username": "author",
                                    "name": "Author",
                                    "avatarUrl": null
                                },
                                "notes": {
                                    "nodes": []
                                }
                            },
                            {
                                "id": "gid://gitlab/Issue/20",
                                "iid": "2",
                                "title": "Test Issue 2",
                                "description": "desc",
                                "state": "opened",
                                "webUrl": "url",
                                "updatedAt": "2024-01-02T00:00:00Z",
                                "author": {
                                    "id": "gid://gitlab/User/100",
                                    "username": "author",
                                    "name": "Author",
                                    "avatarUrl": null
                                },
                                "notes": {
                                    "nodes": []
                                }
                            }
                        ]
                    }
                }
            }
        });

        // 1. Mock GraphQL endpoint -> returns issues with notes
        let m_graphql = server
            .mock("POST", "/api/graphql")
            .with_status(200)
            .with_body(graphql_response.to_string())
            .expect(1)
            .create_async()
            .await;

        // 2. Ensure REST endpoints are NOT called
        let m_issues = server
            .mock("GET", Matcher::Regex(r"/api/v4/projects/1/issues\?.+".to_string()))
            .expect(0)
            .create_async()
            .await;

        let m_notes1 = server
            .mock("GET", Matcher::Regex(r"/api/v4/projects/1/issues/1/notes\?.+".to_string()))
            .expect(0)
            .create_async()
            .await;

        let m_notes2 = server
            .mock("GET", Matcher::Regex(r"/api/v4/projects/1/issues/2/notes\?.+".to_string()))
            .expect(0)
            .create_async()
            .await;

        // Run poll_issues
        service.poll_issues(project_id, 0, &project).await.unwrap();

        // Verify expectations
        m_graphql.assert_async().await;
        m_issues.assert_async().await;
        m_notes1.assert_async().await;
        m_notes2.assert_async().await;
    }
}
