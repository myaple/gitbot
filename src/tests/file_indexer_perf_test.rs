use crate::config::AppSettings;
use crate::file_indexer::FileIndexManager;
use crate::gitlab::GitlabApiClient;
use std::sync::Arc;
use std::time::{Duration, Instant};
use urlencoding::encode;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_search_files_performance() {
    let mock_server = MockServer::start().await;

    let mut settings = AppSettings::default();
    settings.gitlab_url = mock_server.uri();
    settings.gitlab_token = "token".to_string();
    settings.default_branch = "main".to_string();

    let client = Arc::new(GitlabApiClient::new(Arc::new(settings)).unwrap());
    let manager = Arc::new(FileIndexManager::new(client.clone(), 60));

    let project_id = 1;
    let index = manager.get_or_create_index(project_id);

    // Populate index with 5 files that match the keyword "test"
    for i in 0..5 {
        let file_path = format!("src/file_{}.rs", i);
        index.add_file(&file_path, "fn test() {}");

        let encoded_path = encode(&file_path);
        let endpoint_path = format!(
            "/api/v4/projects/{}/repository/files/{}",
            project_id, encoded_path
        );

        let content = base64::encode("fn test() {}");
        let response_body = serde_json::json!({
            "file_name": format!("file_{}.rs", i),
            "file_path": file_path,
            "size": 100,
            "encoding": "base64",
            "content": content,
            "ref": "main",
            "blob_id": "123",
            "commit_id": "456",
            "last_commit_id": "789"
        });

        Mock::given(method("GET"))
            .and(path(endpoint_path))
            .and(query_param("ref", "main"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(response_body)
                    .set_delay(Duration::from_millis(100)),
            )
            .mount(&mock_server)
            .await;
    }

    let start = Instant::now();
    let results = manager
        .search_files(project_id, &["test".to_string()])
        .await
        .unwrap();
    let duration = start.elapsed();

    println!("Search took {:?}", duration);
    assert_eq!(results.len(), 5);

    // In concurrent mode, it should be close to the max delay of a single request (100ms)
    // We assert < 250ms to allow for some overhead (was ~500ms sequentially)
    assert!(
        duration.as_millis() < 250,
        "Expected duration < 250ms (concurrent), got {:?}",
        duration
    );
}
