// This file is no longer needed as the methods have been moved to gitlab.rs
// Keeping the tests for reference

#[cfg(test)]
mod tests {
    use crate::gitlab::GitlabApiClient;
    use crate::config::AppSettings;
    
    fn create_test_settings(base_url: String) -> AppSettings {
        AppSettings {
            gitlab_url: base_url,
            gitlab_token: "test_token".to_string(),
            openai_api_key: "key".to_string(),
            openai_custom_url: "url".to_string(),
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            context_repo_path: None,
        }
    }
    
    #[tokio::test]
    async fn test_get_repository_tree() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = GitlabApiClient::new(&settings).unwrap();
        
        let mock_tree_response = serde_json::json!([
            {
                "id": "a1b2c3d4e5f6",
                "name": "README.md",
                "type": "blob",
                "path": "README.md",
                "mode": "100644"
            },
            {
                "id": "b2c3d4e5f6a1",
                "name": "src",
                "type": "tree",
                "path": "src",
                "mode": "040000"
            },
            {
                "id": "c3d4e5f6a1b2",
                "name": "main.rs",
                "type": "blob",
                "path": "src/main.rs",
                "mode": "100644"
            }
        ]);
        
        let _m = server.mock("GET", "/api/v4/projects/1/repository/tree")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("recursive".into(), "true".into()),
                mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_tree_response.to_string())
            .create_async().await;
            
        let files = client.get_repository_tree(1).await.unwrap();
        assert_eq!(files.len(), 2); // Only blobs, not trees
        assert!(files.contains(&"README.md".to_string()));
        assert!(files.contains(&"src/main.rs".to_string()));
    }
    
    #[tokio::test]
    async fn test_get_file_content() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = GitlabApiClient::new(&settings).unwrap();
        
        let mock_file_response = serde_json::json!({
            "file_name": "main.rs",
            "file_path": "src/main.rs",
            "size": 123,
            "encoding": "base64",
            "content": "Zm4gbWFpbigpIHsKICAgIHByaW50bG4hKCJIZWxsbyBXb3JsZCIpOwp9" // base64 for: fn main() { println!("Hello World"); }
        });
        
        let _m = server.mock("GET", "/api/v4/projects/1/repository/files/src%2Fmain.rs")
            .match_query(mockito::Matcher::UrlEncoded("ref".into(), "main".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_file_response.to_string())
            .create_async().await;
            
        let file = client.get_file_content(1, "src/main.rs").await.unwrap();
        assert_eq!(file.file_name, "main.rs");
        assert_eq!(file.file_path, "src/main.rs");
        assert_eq!(file.size, 123);
        assert_eq!(file.encoding, Some("base64".to_string()));
        assert!(file.content.is_some());
    }
    
    #[tokio::test]
    async fn test_get_merge_request_changes() {
        let mut server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = create_test_settings(base_url);
        let client = GitlabApiClient::new(&settings).unwrap();
        
        let mock_changes_response = serde_json::json!({
            "changes": [
                {
                    "old_path": "src/main.rs",
                    "new_path": "src/main.rs",
                    "diff": "@@ -1,3 +1,5 @@\n fn main() {\n-    println!(\"Hello World\");\n+    println!(\"Hello, World!\");\n+    println!(\"Welcome to GitBot\");\n }"
                },
                {
                    "old_path": "README.md",
                    "new_path": "README.md",
                    "diff": "@@ -1 +1,2 @@\n # My Project\n+A simple Rust project."
                }
            ]
        });
        
        let _m = server.mock("GET", "/api/v4/projects/1/merge_requests/5/changes")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_changes_response.to_string())
            .create_async().await;
            
        let changes = client.get_merge_request_changes(1, 5).await.unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].old_path, "src/main.rs");
        assert_eq!(changes[0].new_path, "src/main.rs");
        assert!(changes[0].diff.contains("Hello, World!"));
        assert_eq!(changes[1].old_path, "README.md");
        assert_eq!(changes[1].new_path, "README.md");
        assert!(changes[1].diff.contains("A simple Rust project."));
    }
}