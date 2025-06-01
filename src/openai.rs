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

pub const OPENAI_CHAT_COMPLETIONS_PATH: &str = "chat/completions";

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
        let mut base_url_string = self.openai_custom_url.to_string();
        if !base_url_string.ends_with('/') {
            base_url_string.push('/');
        }

        let base_for_final_join = Url::parse(&base_url_string).map_err(OpenAIClient::UrlParse)?;

        let request_url = base_for_final_join
            .join(OPENAI_CHAT_COMPLETIONS_PATH)
            .map_err(OpenAIClient::UrlParse)?;

        debug!("Sending chat completion request to: {}", request_url);
        debug!("Request payload: {:?}", request_payload);

        let response = self
            .client
            .post(request_url) // Use the correctly formed URL
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
