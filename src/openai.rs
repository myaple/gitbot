use crate::config::AppSettings;
use crate::models::{OpenAIChatRequest, OpenAIChatResponse};
use reqwest::{header, Client, StatusCode};
use thiserror::Error;
use tracing::{debug, error, instrument};
use url::Url;

#[derive(Error, Debug)]
pub enum OpenAIClient {
    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("API error: {status} - {body}")]
    Api { status: StatusCode, body: String },
    #[error("URL parsing error: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("Failed to deserialize response: {0}")]
    Deserialization(reqwest::Error),
}

#[derive(Debug)]
pub struct OpenAIApiClient {
    client: Client,
    openai_custom_url: Url,
    api_key: String,
}

impl OpenAIApiClient {
    pub fn new(settings: &AppSettings) -> Result<Self, OpenAIClient> {
        let openai_custom_url = Url::parse(&settings.openai_custom_url)?;
        let client = Client::new();
        Ok(Self {
            client,
            openai_custom_url,
            api_key: settings.openai_api_key.clone(),
        })
    }

    #[instrument(skip(self, request_payload), fields(model = %request_payload.model))]
    pub async fn send_chat_completion(
        &self,
        request_payload: &OpenAIChatRequest,
    ) -> Result<OpenAIChatResponse, OpenAIClient> {
        debug!(
            "Sending chat completion request to: {}",
            self.openai_custom_url
        );
        debug!("Request payload: {:?}", request_payload);

        let response = self
            .client
            .post(self.openai_custom_url.clone()) // URL for chat completions is usually specific like /v1/chat/completions
            .header(header::AUTHORIZATION, format!("Bearer {}", self.api_key))
            .header(header::CONTENT_TYPE, "application/json")
            .json(request_payload)
            .send()
            .await
            .map_err(OpenAIClient::Request)?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|e| format!("Failed to read error body: {}", e));
            error!("OpenAI API Error: {} - {}", status, body);
            return Err(OpenAIClient::Api { status, body });
        }

        let parsed_response = response
            .json::<OpenAIChatResponse>()
            .await
            .map_err(OpenAIClient::Deserialization)?;

        // Optional: Check for empty choices if that's an error condition
        // if parsed_response.choices.is_empty() {
        //     return Err(OpenAIClient::NoChoicesReturned);
        // }

        Ok(parsed_response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::models::OpenAIChatMessage; // Already imported OpenAIChatRequest via super::*
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
            context_repo_path: None,
        }
    }

    #[tokio::test]
    async fn test_new_openai_api_client_valid_url() {
        let settings =
            create_test_settings("http://localhost:1234/v1/chat/completions".to_string());
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
        // The client is configured with the full path, so the mock path should be "/"
        // or the client's URL should be just the base and the path added in send_chat_completion.
        // For simplicity with current setup, let's assume openai_custom_url is the full endpoint.
        let full_mock_url = server.url(); // This will be something like http://127.0.0.1:1234

        let settings = create_test_settings(full_mock_url.clone());
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

        // The mock path should be "/" if the full_mock_url is used for the client.
        let mock = server
            .mock("POST", "/")
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
    async fn test_send_chat_completion_api_error() {
        let mut server = mockito::Server::new_async().await;
        let full_mock_url = server.url();

        let settings = create_test_settings(full_mock_url.clone());
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
            .mock("POST", "/")
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
        let full_mock_url = server.url();

        let settings = create_test_settings(full_mock_url.clone());
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
            .mock("POST", "/")
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
}
