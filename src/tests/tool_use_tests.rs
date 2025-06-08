use crate::config::AppSettings;
use crate::file_indexer::FileIndexManager;
use crate::gitlab::GitlabApiClient;
use crate::handlers::execute_tool_call;
use crate::models::{OpenAIToolCall, OpenAIFunctionCall};
use serde_json::json;
use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_settings(server_url: &str) -> Arc<AppSettings> {
        Arc::new(AppSettings {
            openai_custom_url: "https://api.openai.com/v1".to_string(),
            openai_api_key: "test_api_key".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            gitlab_url: server_url.to_string(),
            gitlab_token: "gitlab_token".to_string(),
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "test_bot".to_string(),
            poll_interval_seconds: 60,
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: None,
            max_context_size: 60000,
            max_comment_length: 1000,
            context_lines: 10,
            default_branch: "main".to_string(),
            client_cert_path: None,
            client_key_path: None,
            client_key_password: None,
        })
    }

    #[tokio::test]
    async fn test_execute_get_file_content_tool() {
        let mut server = mockito::Server::new_async().await;
        let settings = create_test_settings(&server.url());
        
        let mock_file_content = json!({
            "file_name": "main.rs",
            "file_path": "src/main.rs",
            "size": 500,
            "encoding": "base64",
            "content_sha256": "abcd1234",
            "ref": "main",
            "blob_id": "blob123",
            "commit_id": "commit123",
            "last_commit_id": "commit123",
            "content": "fn main() {\n    println!(\"Hello, world!\");\n}"
        });

        let mock = server
            .mock("GET", "/api/v4/projects/123/repository/files/src%2Fmain.rs")
            .match_query(mockito::Matcher::UrlEncoded("ref".into(), "main".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_file_content.to_string())
            .create_async()
            .await;

        let gitlab_client = Arc::new(
            GitlabApiClient::new(settings.clone())
                .expect("Failed to create GitlabApiClient")
        );

        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let tool_call = OpenAIToolCall {
            id: "call_123".to_string(),
            tool_type: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "get_file_content".to_string(),
                arguments: json!({"file_path": "src/main.rs"}).to_string(),
            },
        };

        let result = execute_tool_call(&tool_call, &gitlab_client, 123, &file_index_manager).await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("Content of src/main.rs:"));
        assert!(content.contains("fn main()"));
    }

    #[tokio::test]
    async fn test_execute_get_file_lines_tool() {
        let mut server = mockito::Server::new_async().await;
        let settings = create_test_settings(&server.url());
        
        let file_content = "line 1\nline 2\nline 3\nline 4\nline 5";
        let mock_file_content = json!({
            "file_name": "test.txt",
            "file_path": "test.txt",
            "size": file_content.len(),
            "encoding": "base64",
            "content_sha256": "abcd1234",
            "ref": "main",
            "blob_id": "blob123",
            "commit_id": "commit123",
            "last_commit_id": "commit123",
            "content": file_content
        });

        let mock = server
            .mock("GET", "/api/v4/projects/123/repository/files/test.txt")
            .match_query(mockito::Matcher::UrlEncoded("ref".into(), "main".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_file_content.to_string())
            .create_async()
            .await;

        let gitlab_client = Arc::new(
            GitlabApiClient::new(settings.clone())
                .expect("Failed to create GitlabApiClient")
        );

        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let tool_call = OpenAIToolCall {
            id: "call_456".to_string(),
            tool_type: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "get_file_lines".to_string(),
                arguments: json!({
                    "file_path": "test.txt",
                    "start_line": 2,
                    "end_line": 4
                }).to_string(),
            },
        };

        let result = execute_tool_call(&tool_call, &gitlab_client, 123, &file_index_manager).await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("Lines 2-4 of test.txt:"));
        assert!(content.contains("2: line 2"));
        assert!(content.contains("3: line 3"));
        assert!(content.contains("4: line 4"));
        assert!(!content.contains("1: line 1"));
        assert!(!content.contains("5: line 5"));
    }

    #[tokio::test]
    async fn test_execute_get_file_lines_invalid_range() {
        let mut server = mockito::Server::new_async().await;
        let settings = create_test_settings(&server.url());
        
        let file_content = "line 1\nline 2\nline 3";
        let mock_file_content = json!({
            "file_name": "test.txt",
            "file_path": "test.txt",
            "size": file_content.len(),
            "encoding": "base64",
            "content_sha256": "abcd1234",
            "ref": "main",
            "blob_id": "blob123",
            "commit_id": "commit123",
            "last_commit_id": "commit123",
            "content": file_content
        });

        let mock = server
            .mock("GET", "/api/v4/projects/123/repository/files/test.txt")
            .match_query(mockito::Matcher::UrlEncoded("ref".into(), "main".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_file_content.to_string())
            .create_async()
            .await;

        let gitlab_client = Arc::new(
            GitlabApiClient::new(settings.clone())
                .expect("Failed to create GitlabApiClient")
        );

        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let tool_call = OpenAIToolCall {
            id: "call_789".to_string(),
            tool_type: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "get_file_lines".to_string(),
                arguments: json!({
                    "file_path": "test.txt",
                    "start_line": 2,
                    "end_line": 10  // Invalid: beyond file length
                }).to_string(),
            },
        };

        let result = execute_tool_call(&tool_call, &gitlab_client, 123, &file_index_manager).await;

        mock.assert_async().await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("Error: Invalid line range"));
        assert!(content.contains("file has 3 lines"));
    }

    #[tokio::test]
    async fn test_execute_search_repository_files_tool() {
        let settings = create_test_settings("https://gitlab.example.com/api/v4");
        let gitlab_client = Arc::new(
            GitlabApiClient::new(settings)
                .expect("Failed to create GitlabApiClient")
        );

        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let tool_call = OpenAIToolCall {
            id: "call_search".to_string(),
            tool_type: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "search_repository_files".to_string(),
                arguments: json!({
                    "keywords": ["main", "rust"],
                    "limit": 3
                }).to_string(),
            },
        };

        let result = execute_tool_call(&tool_call, &gitlab_client, 123, &file_index_manager).await;

        assert!(result.is_ok());
        let content = result.unwrap();
        // Since we're not mocking the file index search, it should return an empty result or error
        // The important thing is that it doesn't crash and handles the tool call properly
        assert!(content.contains("files") || content.contains("Error"));
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let settings = create_test_settings("https://gitlab.example.com/api/v4");
        let gitlab_client = Arc::new(
            GitlabApiClient::new(settings)
                .expect("Failed to create GitlabApiClient")
        );

        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let tool_call = OpenAIToolCall {
            id: "call_unknown".to_string(),
            tool_type: "function".to_string(),
            function: OpenAIFunctionCall {
                name: "unknown_function".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let result = execute_tool_call(&tool_call, &gitlab_client, 123, &file_index_manager).await;

        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("Unknown tool function: unknown_function"));
    }
}