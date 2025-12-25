use crate::config::AppSettings;
use crate::models::{OpenAIChatMessage, OpenAIChatRequest, Tool, ToolChoice};
use crate::openai::{
    BuilderError, ChatRequestBuilder, OpenAIApiClient, OpenAIClient, OPENAI_CHAT_COMPLETIONS_PATH,
};
use mockito::Matcher;
use reqwest::StatusCode;
use serde_json::json;

fn create_test_settings(base_url: String) -> AppSettings {
    AppSettings {
        prompt_prefix: None,
        openai_custom_url: base_url,
        openai_api_key: "test_api_key".to_string(),
        openai_model: "gpt-3.5-turbo".to_string(),
        openai_temperature: 0.7,
        openai_max_tokens: 1024,
        openai_token_mode: "max_tokens".to_string(),
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
        max_tool_calls: 3,
        client_cert_path: None,
        client_key_path: None,
        client_key_password: None,
        max_comment_length: 1000,
        context_lines: 10,
        auto_triage_enabled: true,
        triage_lookback_hours: 24,
        label_learning_samples: 3,
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
        tools: None,
        tool_choice: None,
        model: "test-model".to_string(),
        messages: vec![OpenAIChatMessage {
            tool_calls: None,
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_call_id: None,
        }],
        temperature: Some(0.7),
        max_tokens: Some(50),
        max_completion_tokens: None,
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
        tools: None,
        tool_choice: None,
        model: "test-model".to_string(),
        messages: vec![OpenAIChatMessage {
            tool_calls: None,
            role: "user".to_string(),
            content: "Hello from /v1".to_string(),
            tool_call_id: None,
        }],
        temperature: Some(0.7),
        max_tokens: Some(50),
        max_completion_tokens: None,
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
        tools: None,
        tool_choice: None,
        model: "test-model".to_string(),
        messages: vec![OpenAIChatMessage {
            tool_calls: None,
            role: "user".to_string(),
            content: "Hello from /v1/".to_string(),
            tool_call_id: None,
        }],
        temperature: Some(0.7),
        max_tokens: Some(50),
        max_completion_tokens: None,
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
        tools: None,
        tool_choice: None,
        model: "test-model".to_string(),
        messages: vec![OpenAIChatMessage {
            tool_calls: None,
            role: "user".to_string(),
            content: "Trigger error".to_string(),
            tool_call_id: None,
        }],
        temperature: None,
        max_tokens: None,
        max_completion_tokens: None,
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
        tools: None,
        tool_choice: None,
        model: "test-model-empty-choice".to_string(),
        messages: vec![OpenAIChatMessage {
            tool_calls: None,
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_call_id: None,
        }],
        temperature: Some(0.5),
        max_tokens: Some(10),
        max_completion_tokens: None,
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
        auto_triage_enabled: true,
        triage_lookback_hours: 24,
        label_learning_samples: 3,
        prompt_prefix: None,
        openai_custom_url: "https://api.openai.com/v1".to_string(),
        openai_api_key: "test_api_key".to_string(),
        openai_model: "gpt-3.5-turbo".to_string(),
        openai_temperature: 0.7,
        openai_max_tokens: 1024,
        openai_token_mode: "max_tokens".to_string(),
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
        max_comment_length: 1000,
        context_lines: 10,
        default_branch: "main".to_string(),
        max_tool_calls: 3,
        client_cert_path: Some("/nonexistent/cert.pem".to_string()),
        client_key_path: Some("/nonexistent/key.pem".to_string()),
        client_key_password: Some("test_password".to_string()),
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
        auto_triage_enabled: true,
        triage_lookback_hours: 24,
        label_learning_samples: 3,
        prompt_prefix: None,
        openai_custom_url: "https://api.openai.com/v1".to_string(),
        openai_api_key: "test_api_key".to_string(),
        openai_model: "gpt-3.5-turbo".to_string(),
        openai_temperature: 0.7,
        openai_max_tokens: 1024,
        openai_token_mode: "max_tokens".to_string(),
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
        max_comment_length: 1000,
        context_lines: 10,
        default_branch: "main".to_string(),
        max_tool_calls: 3,
        client_cert_path: None,
        client_key_path: None,
        client_key_password: None,
    };

    // This should succeed - no client certificates required
    let result = OpenAIApiClient::new(&settings);
    assert!(
        result.is_ok(),
        "Expected success when no client cert configured: {:?}",
        result.err()
    );
}

// Builder tests
#[test]
fn test_builder_basic_construction() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());
    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_user_message("Hello");
    let request = builder.build().unwrap();

    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.messages[0].role, "user");
    assert_eq!(request.messages[0].content, "Hello");
    assert_eq!(request.model, "gpt-3.5-turbo");
    assert_eq!(request.temperature, Some(0.7));
}

#[test]
fn test_builder_token_mode_max_tokens() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());
    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_user_message("test");
    let request = builder.build().unwrap();

    assert!(request.max_tokens.is_some());
    assert!(request.max_completion_tokens.is_none());
    assert_eq!(request.max_tokens.unwrap(), 1024);
}

#[test]
fn test_builder_token_mode_max_completion_tokens() {
    let mut config = create_test_settings("https://api.openai.com/v1".to_string());
    config.openai_token_mode = "max_completion_tokens".to_string();

    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_user_message("test");
    let request = builder.build().unwrap();

    assert!(request.max_tokens.is_none());
    assert!(request.max_completion_tokens.is_some());
    assert_eq!(request.max_completion_tokens.unwrap(), 1024);
}

#[test]
fn test_builder_prompt_prefix_first_message() {
    let mut config = create_test_settings("https://api.openai.com/v1".to_string());
    config.prompt_prefix = Some("You are helpful".to_string());

    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_user_message("Hello");
    let request = builder.build().unwrap();

    assert!(request.messages[0].content.starts_with("You are helpful"));
    assert!(request.messages[0].content.contains("Hello"));
}

#[test]
fn test_builder_prompt_prefix_subsequent_messages() {
    let mut config = create_test_settings("https://api.openai.com/v1".to_string());
    config.prompt_prefix = Some("You are helpful".to_string());

    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_user_message("First");
    builder.with_user_message("Second");
    let request = builder.build().unwrap();

    // First message has prefix
    assert!(request.messages[0].content.starts_with("You are helpful"));

    // Second message does NOT have prefix
    assert_eq!(request.messages[1].content, "Second");
}

#[test]
fn test_builder_with_system_message() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());
    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_system_message("You are a helpful assistant");
    builder.with_user_message("Hello");
    let request = builder.build().unwrap();

    assert_eq!(request.messages.len(), 2);
    assert_eq!(request.messages[0].role, "system");
    assert_eq!(request.messages[0].content, "You are a helpful assistant");
    assert_eq!(request.messages[1].role, "user");
    assert_eq!(request.messages[1].content, "Hello");
}

#[test]
fn test_builder_with_tools() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());
    let tools = vec![Tool {
        r#type: "function".to_string(),
        function: serde_json::from_value(json!({
            "name": "test_function",
            "description": "A test function",
            "parameters": {"type": "object"}
        }))
        .unwrap(),
    }];

    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_user_message("test");
    builder.with_tools(tools.clone());
    builder.with_tool_choice(ToolChoice::Auto);
    let request = builder.build().unwrap();

    assert!(request.tools.is_some());
    assert_eq!(request.tools.unwrap().len(), 1);
    assert!(request.tool_choice.is_some());
}

#[test]
fn test_builder_multi_turn_conversation() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());

    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_user_message("What's the weather?");

    // Simulate assistant response
    builder.add_message(OpenAIChatMessage {
        role: "assistant".to_string(),
        content: "I'll check.".to_string(),
        tool_calls: Some(vec![]),
        tool_call_id: None,
    });

    // Simulate tool response
    builder.add_message(OpenAIChatMessage {
        role: "tool".to_string(),
        content: "It's sunny.".to_string(),
        tool_calls: None,
        tool_call_id: Some("call-123".to_string()),
    });

    let request = builder.build().unwrap();
    assert_eq!(request.messages.len(), 3);
    assert_eq!(request.messages[0].role, "user");
    assert_eq!(request.messages[1].role, "assistant");
    assert_eq!(request.messages[2].role, "tool");
}

#[test]
fn test_builder_no_messages_error() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());

    let builder = ChatRequestBuilder::new(&config);
    let result = builder.build();
    assert!(matches!(result, Err(BuilderError::NoMessages)));
}

#[test]
fn test_builder_messages_mut_for_loop_scenario() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());

    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_system_message("You are helpful");
    builder.with_user_message("Hello");

    // Simulate loop behavior
    let new_messages = vec![OpenAIChatMessage {
        role: "assistant".to_string(),
        content: "Hi there!".to_string(),
        tool_calls: None,
        tool_call_id: None,
    }];

    *builder.messages_mut() = new_messages;

    let request = builder.build().unwrap();
    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.messages[0].role, "assistant");
    assert_eq!(request.messages[0].content, "Hi there!");
}

#[test]
fn test_builder_temperature_override() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());
    let custom_temp = 0.1;

    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_user_message("test");
    builder.with_temperature(custom_temp);
    let request = builder.build().unwrap();

    assert_eq!(request.temperature.unwrap(), custom_temp);
    assert_ne!(custom_temp, config.openai_temperature);
}

#[test]
fn test_builder_with_messages_replace() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());

    let messages = vec![
        OpenAIChatMessage {
            role: "system".to_string(),
            content: "You are helpful".to_string(),
            tool_calls: None,
            tool_call_id: None,
        },
        OpenAIChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    let mut builder = ChatRequestBuilder::new(&config);
    builder.with_messages(messages);
    let request = builder.build().unwrap();

    assert_eq!(request.messages.len(), 2);
    assert_eq!(request.messages[0].role, "system");
    assert_eq!(request.messages[1].role, "user");
}

#[test]
fn test_builder_chaining() {
    let config = create_test_settings("https://api.openai.com/v1".to_string());

    let mut builder = ChatRequestBuilder::new(&config);
    builder
        .with_system_message("You are helpful")
        .with_user_message("First")
        .with_user_message("Second");

    // Can check message count before building
    // We need to call messages_mut() to access them
    assert_eq!(builder.messages_mut().len(), 3);

    let request = builder.build().unwrap();
    assert_eq!(request.messages.len(), 3);
}
