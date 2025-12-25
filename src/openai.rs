use reqwest::{header, Client, Identity, StatusCode};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, error, info, instrument, warn};
use url::Url;

use backoff::future::retry_notify;
use backoff::ExponentialBackoff;

use crate::config::AppSettings;
use crate::models::{OpenAIChatMessage, OpenAIChatRequest, OpenAIChatResponse, Tool, ToolChoice};

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
    #[error("Request failed after {attempts} attempts over {duration:?}: {source}")]
    RetryFailed {
        attempts: usize,
        duration: Duration,
        #[source]
        source: Box<OpenAIClient>,
    },
}

#[derive(Debug)]
pub struct OpenAIApiClient {
    client: Client,
    openai_custom_url: Url,
    api_key: String,
    retry_initial_delay_ms: u64,
    retry_max_delay_ms: u64,
    retry_backoff_multiplier: f64,
    max_retries: usize,
}

pub const OPENAI_CHAT_COMPLETIONS_PATH: &str = "chat/completions";

impl OpenAIApiClient {
    pub fn new(settings: &AppSettings) -> Result<Self, OpenAIClient> {
        let openai_custom_url = Url::parse(&settings.openai_custom_url)?;

        // Build the client with timeouts and connection pooling
        let mut client_builder = Client::builder()
            .timeout(Duration::from_secs(settings.openai_timeout_secs))
            .connect_timeout(Duration::from_secs(settings.openai_connect_timeout_secs))
            .pool_max_idle_per_host(0)
            .pool_idle_timeout(Duration::from_secs(30))
            .tcp_keepalive(Duration::from_secs(60));

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
            retry_initial_delay_ms: settings.openai_retry_initial_delay_ms,
            retry_max_delay_ms: settings.openai_retry_max_delay_ms,
            retry_backoff_multiplier: settings.openai_retry_backoff_multiplier,
            max_retries: settings.openai_max_retries,
        })
    }

    /// Check if reqwest error indicates a broken pipe (retryable connection issue)
    fn is_broken_pipe(err: &reqwest::Error) -> bool {
        let err_str = err.to_string().to_lowercase();
        err_str.contains("broken pipe") || err_str.contains("connection reset")
    }

    /// Determine if an error is retryable
    fn is_retryable_error(&self, error: &OpenAIClient) -> bool {
        match error {
            // Timeout and connection errors are retryable
            OpenAIClient::Request(reqwest_err) => {
                reqwest_err.is_timeout()
                    || reqwest_err.is_connect()
                    || Self::is_broken_pipe(reqwest_err)
            }
            // 5xx server errors, 408 timeout, 429 rate limit are retryable
            OpenAIClient::Api { status, .. } => {
                let code = status.as_u16();
                code == 408 || code == 429 || status.is_server_error()
            }
            // Everything else is permanent
            _ => false,
        }
    }

    /// Internal method that performs a single API call (no retry logic)
    #[instrument(skip(self, request_payload), fields(model = %request_payload.model))]
    async fn send_chat_completion_internal(
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

    /// Public method with retry logic
    #[instrument(skip(self, request_payload), fields(model = %request_payload.model))]
    pub async fn send_chat_completion(
        &self,
        request_payload: &OpenAIChatRequest,
    ) -> Result<OpenAIChatResponse, OpenAIClient> {
        // Create backoff strategy from config
        let backoff_strategy = ExponentialBackoff {
            initial_interval: Duration::from_millis(self.retry_initial_delay_ms),
            max_interval: Duration::from_millis(self.retry_max_delay_ms),
            multiplier: self.retry_backoff_multiplier,
            randomization_factor: 0.1, // Add jitter to prevent thundering herd
            ..Default::default()
        };

        let attempt = Arc::new(AtomicUsize::new(0));
        let start = Instant::now();

        // Clone Arc for use in the operation closure
        let attempt_clone = attempt.clone();

        // Use retry_notify to track attempts and log retries
        let operation = || async {
            let current_attempt = attempt_clone.fetch_add(1, Ordering::SeqCst) + 1;

            self.send_chat_completion_internal(request_payload)
                .await
                .map_err(|e| {
                    if self.is_retryable_error(&e) {
                        warn!(
                            attempt = current_attempt,
                            max_retries = self.max_retries,
                            error = %e,
                            error_type = std::any::type_name_of_val(&e),
                            model = %request_payload.model,
                            "OpenAI request failed, retrying with exponential backoff"
                        );
                        backoff::Error::transient(e)
                    } else {
                        backoff::Error::permanent(e)
                    }
                })
        };

        let notify = |err: OpenAIClient, duration: Duration| {
            warn!(
                error = %err,
                next_retry_in_ms = duration.as_millis(),
                "Scheduling next retry attempt"
            );
        };

        match retry_notify(backoff_strategy, operation, notify).await {
            Ok(response) => {
                let final_attempt = attempt.load(Ordering::SeqCst);
                if final_attempt > 1 {
                    info!(
                        retries = final_attempt - 1,
                        total_time_ms = start.elapsed().as_millis(),
                        "Request succeeded after retries"
                    );
                }
                Ok(response)
            }
            Err(e) => {
                // Only wrap in RetryFailed if we attempted multiple retries
                let final_attempt = attempt.load(Ordering::SeqCst);
                if final_attempt > 1 {
                    error!(
                        attempts = final_attempt,
                        max_retries = self.max_retries,
                        total_time_ms = start.elapsed().as_millis(),
                        "Request failed after retries"
                    );
                    Err(OpenAIClient::RetryFailed {
                        attempts: final_attempt,
                        duration: start.elapsed(),
                        source: Box::new(e),
                    })
                } else {
                    // Single attempt failed - return the original error
                    Err(e)
                }
            }
        }
    }
}

/// Error type for builder validation failures
#[derive(Error, Debug)]
pub enum BuilderError {
    #[error("Cannot build request with no messages")]
    NoMessages,
}

/// Builder for creating OpenAI chat completion requests with proper configuration
pub struct ChatRequestBuilder {
    model: String,
    temperature: f32,
    max_tokens: u32,
    token_mode: String,
    messages: Vec<OpenAIChatMessage>,
    tools: Option<Vec<Tool>>,
    tool_choice: Option<ToolChoice>,
    prompt_prefix: Option<String>,
    user_message_count: usize,
}

impl ChatRequestBuilder {
    /// Create a new builder from configuration
    pub fn new(config: &AppSettings) -> Self {
        Self {
            model: config.openai_model.clone(),
            temperature: config.openai_temperature,
            max_tokens: config.openai_max_tokens,
            token_mode: config.openai_token_mode.clone(),
            messages: Vec::new(),
            tools: None,
            tool_choice: None,
            prompt_prefix: config.prompt_prefix.clone(),
            user_message_count: 0,
        }
    }

    /// Add a system message
    #[allow(dead_code)]
    pub fn with_system_message(&mut self, content: &str) -> &mut Self {
        self.messages.push(OpenAIChatMessage {
            role: "system".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
        self
    }

    /// Add a user message (applies prompt_prefix to first user message only)
    pub fn with_user_message(&mut self, content: &str) -> &mut Self {
        let final_content = if self.user_message_count == 0 {
            self.prompt_prefix
                .as_ref()
                .map(|p| format!("{}\n\n{}", p, content))
                .unwrap_or_else(|| content.to_string())
        } else {
            content.to_string()
        };

        self.user_message_count += 1;
        self.messages.push(OpenAIChatMessage {
            role: "user".to_string(),
            content: final_content,
            tool_calls: None,
            tool_call_id: None,
        });
        self
    }

    /// Replace all messages (for complex multi-turn scenarios)
    pub fn with_messages(&mut self, messages: Vec<OpenAIChatMessage>) -> &mut Self {
        self.messages = messages;
        self
    }

    /// Add a single message (for tool calling loops)
    #[allow(dead_code)]
    pub fn add_message(&mut self, message: OpenAIChatMessage) -> &mut Self {
        self.messages.push(message);
        self
    }

    /// Get mutable reference to messages (for advanced manipulation)
    #[allow(dead_code)]
    pub fn messages_mut(&mut self) -> &mut Vec<OpenAIChatMessage> {
        &mut self.messages
    }

    /// Set tools for function calling
    pub fn with_tools(&mut self, tools: Vec<Tool>) -> &mut Self {
        self.tools = Some(tools);
        self
    }

    /// Set tool choice strategy
    pub fn with_tool_choice(&mut self, choice: ToolChoice) -> &mut Self {
        self.tool_choice = Some(choice);
        self
    }

    /// Override temperature (optional, defaults to config)
    #[allow(dead_code)]
    pub fn with_temperature(&mut self, temperature: f32) -> &mut Self {
        self.temperature = temperature;
        self
    }

    /// Build the request, validating state
    pub fn build(self) -> Result<OpenAIChatRequest, BuilderError> {
        if self.messages.is_empty() {
            return Err(BuilderError::NoMessages);
        }

        let (max_tokens, max_completion_tokens) = match self.token_mode.as_str() {
            "max_completion_tokens" => (None, Some(self.max_tokens)),
            _ => (Some(self.max_tokens), None),
        };

        Ok(OpenAIChatRequest {
            model: self.model,
            messages: self.messages,
            temperature: Some(self.temperature),
            max_tokens,
            max_completion_tokens,
            tools: self.tools,
            tool_choice: self.tool_choice,
        })
    }
}
