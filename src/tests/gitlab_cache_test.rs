use crate::config::AppSettings;
use crate::gitlab::GitlabApiClient;
use mockito;
use serde_json::json;
use std::sync::Arc;

// Helper to create AppSettings for tests
fn create_test_settings(base_url: String) -> AppSettings {
    let mut settings = AppSettings::default();
    settings.gitlab_url = base_url;
    settings.gitlab_token = "test_token".to_string();
    settings.default_branch = "test-main".to_string();
    settings
}

#[tokio::test]
async fn test_get_repository_tree_cache_behavior() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();

    let settings = Arc::new(create_test_settings(base_url.clone()));
    let client = GitlabApiClient::new(settings).expect("Failed to create client");

    // Mock response for tree
    let mock_tree_response = json!([
        {
            "id": "a1b2c3d4e5f6",
            "name": "README.md",
            "type": "blob",
            "path": "README.md",
            "mode": "100644"
        }
    ]);

    // Mock the endpoint - expect 2 calls initially (baseline)
    let mock = server
        .mock("GET", "/api/v4/projects/1/repository/tree")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("recursive".into(), "true".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            mockito::Matcher::UrlEncoded("page".into(), "1".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("X-Total-Pages", "1")
        .with_body(mock_tree_response.to_string())
        .expect(1) // Optimization: Expect 1 call because of caching
        .create_async()
        .await;

    // First call
    let files1 = client
        .get_repository_tree(1)
        .await
        .expect("First call failed");
    assert_eq!(files1.len(), 1);
    assert_eq!(files1[0], "README.md");

    // Second call
    let files2 = client
        .get_repository_tree(1)
        .await
        .expect("Second call failed");
    assert_eq!(files2.len(), 1);
    assert_eq!(files2[0], "README.md");

    mock.assert_async().await;
}
