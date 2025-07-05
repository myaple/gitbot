use reqwest::{header, Client, Identity, StatusCode};
use std::fs;
use thiserror::Error;
use tracing::{debug, error, instrument};
use url::Url;

use crate::config::AppSettings;
use crate::models::{OpenAIChatRequest, OpenAIChatResponse};

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
    #[error("File I/O error: {0}")]
    Io(#[from] std::io::Error),
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

        // Build the client with optional client certificate
        let mut client_builder = Client::builder();

        // Add client certificate if both cert and key paths are provided
        if let (Some(cert_path), Some(key_path)) =
            (&settings.client_cert_path, &settings.client_key_path)
        {
            debug!("Loading client certificate from: {}", cert_path);
            debug!("Loading client private key from: {}", key_path);

            // Read certificate file
            let cert_data = fs::read(cert_path)?;

            // Read private key file
            let key_data = fs::read(key_path)?;

            // Create identity based on available data
            let identity = if cert_path.ends_with(".p12") || cert_path.ends_with(".pfx") {
                // PKCS#12 format - requires password
                let password = settings.client_key_password.as_deref().unwrap_or("");
                Identity::from_pkcs12_der(&cert_data, password)
            } else {
                // Try PEM format with separate cert and key files
                Identity::from_pkcs8_pem(&cert_data, &key_data)
            };

            match identity {
                Ok(id) => {
                    debug!("Successfully loaded client certificate");
                    client_builder = client_builder.identity(id);
                }
                Err(e) => {
                    error!("Failed to load client certificate: {}", e);
                    return Err(OpenAIClient::Request(e));
                }
            }
        }

        let client = client_builder.build().map_err(OpenAIClient::Request)?;

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
                .unwrap_or_else(|e| format!("Failed to read error body: {e}"));
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
