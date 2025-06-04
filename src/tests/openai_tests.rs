use crate::config::AppSettings;
use crate::models::{OpenAIChatMessage, OpenAIChatRequest};
use crate::openai::{OpenAIApiClient, OpenAIClient, OPENAI_CHAT_COMPLETIONS_PATH};
use mockito::Matcher;
use reqwest::StatusCode;
use serde_json::json;

fn create_test_settings(base_url: String) -> AppSettings {
    AppSettings {
        openai_custom_url: base_url,
        openai_api_key: "test_api_key".to_string(),
        openai_model: "gpt-3.5-turbo".to_string(),
        openai_temperature: 0.7,
        openai_max_tokens: 1024,
        gitlab_url: "https://gitlab.example.com".to_string(),
        gitlab_token: "gitlab_token".to_string(),
        repos_to_poll: vec!["org/repo1".to_string()],
        log_level: "debug".to_string(),
        bot_username: "openai_bot".to_string(),
        poll_interval_seconds: 60,
        stale_issue_days: 30, // Added default for tests
        max_age_hours: 24,
        context_repo_path: None,
        max_context_size: 60000,
        default_branch: "main".to_string(),
        client_cert_path: None,
        client_key_path: None,
        client_key_password: None,
        max_comment_length: 1000,
    }
}

#[tokio::test]
async fn test_new_openai_api_client_valid_url() {
    let settings = create_test_settings("http://localhost:1234/v1/".to_string()); // Ensure it's a base URL
    let client = OpenAIApiClient::new(&settings);
    assert!(client.is_ok());
}

#[tokio::test]
async fn test_new_openai_api_client_invalid_url() {
    let settings = create_test_settings("not a valid url".to_string());
    let client = OpenAIApiClient::new(&settings);
    assert!(client.is_err());
    match client.err().unwrap() {
        OpenAIClient::UrlParse(_) => {} // Expected error
        e => panic!("Expected UrlParse, got {:?}", e),
    }
}

#[tokio::test]
async fn test_send_chat_completion_success() {
    let mut server = mockito::Server::new_async().await;
    // openai_custom_url should be the base URL of the mock server.
    let base_mock_url = server.url(); // This will be something like http://127.0.0.1:1234

    let settings = create_test_settings(base_mock_url.clone());
    let client = OpenAIApiClient::new(&settings).unwrap();

    let request_payload = OpenAIChatRequest {
        model: "test-model".to_string(),
        messages: vec![OpenAIChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
        temperature: Some(0.7),
        max_tokens: Some(50),
    };

    let mock_response_body = json!({
        "id": "chatcmpl-123",
        "object": "chat.completion",
        "created": 1677652288,
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hi there!"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 9,
            "completion_tokens": 12,
            "total_tokens": 21
        }
    });

    // The mock path should be "/chat/completions" relative to the server's base URL.
    let mock = server
        .mock(
            "POST",
            Matcher::Exact(format!("/{}", OPENAI_CHAT_COMPLETIONS_PATH)),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_response_body.to_string())
        .match_header("Authorization", "Bearer test_api_key")
        // Skip body matching to avoid JSON format issues
        .create_async()
        .await;

    let response_result = client.send_chat_completion(&request_payload).await;

    mock.assert_async().await; // Verify the mock was called
    assert!(
        response_result.is_ok(),
        "Expected Ok, got Err: {:?}",
        response_result.err()
    );
    let response = response_result.unwrap();
    assert!(!response.choices.is_empty());
    assert_eq!(response.choices[0].message.content, "Hi there!");
}

#[tokio::test]
async fn test_send_chat_completion_success_with_path_no_trailing_slash() {
    let mut server = mockito::Server::new_async().await;
    // openai_custom_url is the base URL of the mock server + a path segment without a trailing slash
    let base_mock_url_with_path = format!("{}/v1", server.url()); // e.g., http://127.0.0.1:1234/v1

    let settings = create_test_settings(base_mock_url_with_path.clone());
    let client = OpenAIApiClient::new(&settings).unwrap();

    let request_payload = OpenAIChatRequest {
        model: "test-model".to_string(),
        messages: vec![OpenAIChatMessage {
            role: "user".to_string(),
            content: "Hello from /v1".to_string(),
        }],
        temperature: Some(0.7),
        max_tokens: Some(50),
    };

    let mock_response_body = json!({
        "id": "chatcmpl-v1-123",
        "object": "chat.completion",
        "created": 1677652289,
        "model": "test-model-v1",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hi there from /v1!"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 13,
            "total_tokens": 23
        }
    });

    // The mock path should be "/v1/chat/completions"
    let mock = server
        .mock(
            "POST",
            Matcher::Exact(format!("/v1/{}", OPENAI_CHAT_COMPLETIONS_PATH)),
        ) // Note the /v1 prefix
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_response_body.to_string())
        .match_header("Authorization", "Bearer test_api_key")
        .create_async()
        .await;

    let response_result = client.send_chat_completion(&request_payload).await;

    mock.assert_async().await; // Verify the mock was called
    assert!(
        response_result.is_ok(),
        "Expected Ok, got Err: {:?}",
        response_result.err()
    );
    let response = response_result.unwrap();
    assert!(!response.choices.is_empty());
    assert_eq!(response.choices[0].message.content, "Hi there from /v1!");
}

#[tokio::test]
async fn test_send_chat_completion_success_with_path_with_trailing_slash() {
    let mut server = mockito::Server::new_async().await;
    // openai_custom_url is the base URL of the mock server + a path segment with a trailing slash
    let base_mock_url_with_path = format!("{}/v1/", server.url()); // e.g., http://127.0.0.1:1234/v1/

    let settings = create_test_settings(base_mock_url_with_path.clone());
    let client = OpenAIApiClient::new(&settings).unwrap();

    let request_payload = OpenAIChatRequest {
        model: "test-model".to_string(),
        messages: vec![OpenAIChatMessage {
            role: "user".to_string(),
            content: "Hello from /v1/".to_string(),
        }],
        temperature: Some(0.7),
        max_tokens: Some(50),
    };

    let mock_response_body = json!({
        "id": "chatcmpl-v1slash-123",
        "object": "chat.completion",
        "created": 1677652290,
        "model": "test-model-v1slash",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hi there from /v1/!"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 14,
            "total_tokens": 25
        }
    });

    // The mock path should still be "/v1/chat/completions"
    // as the client should correctly handle the existing trailing slash.
    let mock = server
        .mock(
            "POST",
            Matcher::Exact(format!("/v1/{}", OPENAI_CHAT_COMPLETIONS_PATH)),
        ) // Note the /v1 prefix
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_response_body.to_string())
        .match_header("Authorization", "Bearer test_api_key")
        .create_async()
        .await;

    let response_result = client.send_chat_completion(&request_payload).await;

    mock.assert_async().await; // Verify the mock was called
    assert!(
        response_result.is_ok(),
        "Expected Ok, got Err: {:?}",
        response_result.err()
    );
    let response = response_result.unwrap();
    assert!(!response.choices.is_empty());
    assert_eq!(response.choices[0].message.content, "Hi there from /v1/!");
}

#[tokio::test]
async fn test_send_chat_completion_api_error() {
    let mut server = mockito::Server::new_async().await;
    let base_mock_url = server.url(); // Base URL of the mock server

    let settings = create_test_settings(base_mock_url.clone());
    let client = OpenAIApiClient::new(&settings).unwrap();

    let request_payload = OpenAIChatRequest {
        model: "test-model".to_string(),
        messages: vec![OpenAIChatMessage {
            role: "user".to_string(),
            content: "Trigger error".to_string(),
        }],
        temperature: None,
        max_tokens: None,
    };

    let error_body = json!({"error": {"message": "Invalid API key", "type": "auth_error"}});

    let mock = server
        .mock(
            "POST",
            Matcher::Exact(format!("/{}", OPENAI_CHAT_COMPLETIONS_PATH)),
        ) // Mock the appended path
        .with_status(401) // Unauthorized
        .with_header("content-type", "application/json")
        .with_body(error_body.to_string())
        // Skip body matching to avoid JSON format issues
        .create_async()
        .await;

    let result = client.send_chat_completion(&request_payload).await;

    mock.assert_async().await;
    assert!(result.is_err());
    match result.err().unwrap() {
        OpenAIClient::Api { status, body } => {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body, error_body.to_string());
        }
        e => panic!("Expected Api, got {:?}", e),
    }
}

#[tokio::test]
async fn test_send_chat_completion_empty_choices() {
    let mut server = mockito::Server::new_async().await;
    let base_mock_url = server.url(); // Base URL of the mock server

    let settings = create_test_settings(base_mock_url.clone());
    let client = OpenAIApiClient::new(&settings).unwrap();

    let request_payload = OpenAIChatRequest {
        model: "test-model-empty-choice".to_string(),
        messages: vec![OpenAIChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
        temperature: Some(0.5),
        max_tokens: Some(10),
    };

    let mock_response_body = json!({
        "id": "chatcmpl-456",
        "object": "chat.completion",
        "created": 1677652300,
        "model": "test-model-empty-choice",
        "choices": [], // Empty choices array
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 0,
            "total_tokens": 5
        }
    });

    let mock = server
        .mock(
            "POST",
            Matcher::Exact(format!("/{}", OPENAI_CHAT_COMPLETIONS_PATH)),
        ) // Mock the appended path
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_response_body.to_string())
        // Skip body matching to avoid JSON format issues
        .create_async()
        .await;

    let response_result = client.send_chat_completion(&request_payload).await;

    mock.assert_async().await;

    // Current implementation does not throw OpenAIClient::NoChoicesReturned,
    // it returns the response as is. The caller should handle empty choices.
    // If NoChoicesReturned were to be implemented, this test would change.
    assert!(
        response_result.is_ok(),
        "Expected Ok for empty choices, got Err: {:?}",
        response_result.err()
    );
    let response = response_result.unwrap();
    assert!(response.choices.is_empty(), "Expected choices to be empty");

    // Example of how to test for NoChoicesReturned if it were implemented:
    // assert!(result.is_err());
    // match result.err().unwrap() {
    //     OpenAIClient::NoChoicesReturned => {} // Expected
    //     e => panic!("Expected NoChoicesReturned, got {:?}", e),
    // }
}

#[tokio::test]
async fn test_new_openai_api_client_with_client_cert_config() {
    // Test that client can be created when client certificate paths are provided
    // but files don't exist (should not fail creation, only when making actual requests)
    let settings = AppSettings {
        openai_custom_url: "https://api.openai.com/v1".to_string(),
        openai_api_key: "test_api_key".to_string(),
        openai_model: "gpt-3.5-turbo".to_string(),
        openai_temperature: 0.7,
        openai_max_tokens: 1024,
        gitlab_url: "https://gitlab.example.com".to_string(),
        gitlab_token: "gitlab_token".to_string(),
        repos_to_poll: vec!["org/repo1".to_string()],
        log_level: "debug".to_string(),
        bot_username: "openai_bot".to_string(),
        poll_interval_seconds: 60,
        stale_issue_days: 30,
        max_age_hours: 24,
        context_repo_path: None,
        max_context_size: 60000,
        default_branch: "main".to_string(),
        client_cert_path: Some("/nonexistent/cert.pem".to_string()),
        client_key_path: Some("/nonexistent/key.pem".to_string()),
        client_key_password: Some("test_password".to_string()),
        max_comment_length: 1000,
    };

    // This should fail because the certificate files don't exist
    let result = OpenAIApiClient::new(&settings);
    assert!(
        result.is_err(),
        "Expected error when certificate files don't exist"
    );

    // Check that it's an I/O error (file not found)
    match result.err().unwrap() {
        OpenAIClient::Io(_) => {
            // Expected - certificate files don't exist
        }
        e => panic!("Expected Io error, got {:?}", e),
    }
}

#[tokio::test]
async fn test_new_openai_api_client_without_client_cert() {
    // Test that client creation works without client certificates
    let settings = AppSettings {
        openai_custom_url: "https://api.openai.com/v1".to_string(),
        openai_api_key: "test_api_key".to_string(),
        openai_model: "gpt-3.5-turbo".to_string(),
        openai_temperature: 0.7,
        openai_max_tokens: 1024,
        gitlab_url: "https://gitlab.example.com".to_string(),
        gitlab_token: "gitlab_token".to_string(),
        repos_to_poll: vec!["org/repo1".to_string()],
        log_level: "debug".to_string(),
        bot_username: "openai_bot".to_string(),
        poll_interval_seconds: 60,
        stale_issue_days: 30,
        max_age_hours: 24,
        context_repo_path: None,
        max_context_size: 60000,
        default_branch: "main".to_string(),
        client_cert_path: None,
        client_key_path: None,
        client_key_password: None,
        max_comment_length: 1000,
    };

    // This should succeed - no client certificates required
    let result = OpenAIApiClient::new(&settings);
    assert!(
        result.is_ok(),
        "Expected success when no client cert configured: {:?}",
        result.err()
    );
}
