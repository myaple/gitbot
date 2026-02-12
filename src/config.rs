use clap::Parser;
use std::env;
use std::fmt::Debug;

#[derive(Debug, Clone, Parser)]
#[command(
    author,
    version,
    about = "A GitLab bot that responds to mentions using AI"
)]
pub struct AppSettings {
    /// GitLab instance URL
    #[arg(long, env = "GITBOT_GITLAB_URL", default_value = "https://gitlab.com")]
    pub gitlab_url: String,

    /// Optional prefix to prepend to every prompt sent to the LLM
    #[arg(long, env = "GITBOT_PROMPT_PREFIX")]
    pub prompt_prefix: Option<String>,

    /// GitLab API token
    #[arg(long, env = "GITBOT_GITLAB_TOKEN")]
    pub gitlab_token: String,

    /// OpenAI API key
    #[arg(long, env = "GITBOT_OPENAI_API_KEY")]
    pub openai_api_key: String,

    /// Custom OpenAI API URL (if using a proxy or alternative endpoint)
    #[arg(
        long,
        env = "GITBOT_OPENAI_CUSTOM_URL",
        default_value = "https://api.openai.com/v1"
    )]
    pub openai_custom_url: String,

    /// OpenAI model to use
    #[arg(long, env = "GITBOT_OPENAI_MODEL", default_value = "gpt-3.5-turbo")]
    pub openai_model: String,

    /// Temperature parameter for OpenAI API (0.0 to 1.0)
    #[arg(long, env = "GITBOT_OPENAI_TEMPERATURE", default_value_t = 0.7)]
    pub openai_temperature: f32,

    /// Maximum number of tokens to generate in the response
    #[arg(long, env = "GITBOT_OPENAI_MAX_TOKENS", default_value_t = 1024)]
    pub openai_max_tokens: u32,

    /// Token parameter mode for OpenAI API: "max_tokens" (legacy) or "max_completion_tokens" (new)
    #[arg(long, env = "GITBOT_OPENAI_TOKEN_MODE", default_value = "max_tokens", value_parser = validate_token_mode)]
    pub openai_token_mode: String,

    /// OpenAI request timeout in seconds (default: 120)
    #[arg(long, env = "GITBOT_OPENAI_TIMEOUT_SECS", default_value_t = 120)]
    pub openai_timeout_secs: u64,

    /// OpenAI connection timeout in seconds (default: 10)
    #[arg(long, env = "GITBOT_OPENAI_CONNECT_TIMEOUT_SECS", default_value_t = 10)]
    pub openai_connect_timeout_secs: u64,

    /// Number of retries for OpenAI API calls (default: 3, max: 10)
    #[arg(long, env = "GITBOT_OPENAI_MAX_RETRIES", default_value_t = 3, value_parser = validate_max_retries)]
    pub openai_max_retries: usize,

    /// Initial retry delay in milliseconds (default: 1000)
    #[arg(
        long,
        env = "GITBOT_OPENAI_RETRY_INITIAL_DELAY_MS",
        default_value_t = 1000
    )]
    pub openai_retry_initial_delay_ms: u64,

    /// Maximum retry delay in milliseconds (default: 30000)
    #[arg(
        long,
        env = "GITBOT_OPENAI_RETRY_MAX_DELAY_MS",
        default_value_t = 30000
    )]
    pub openai_retry_max_delay_ms: u64,

    /// Exponential backoff multiplier (default: 2.0)
    #[arg(long, env = "GITBOT_OPENAI_RETRY_BACKOFF_MULTIPLIER", default_value_t = 2.0, value_parser = validate_retry_backoff_multiplier)]
    pub openai_retry_backoff_multiplier: f64,

    /// Comma-separated list of repositories to poll (format: group/project)
    #[arg(long, env = "GITBOT_REPOS_TO_POLL", value_delimiter = ',')]
    pub repos_to_poll: Vec<String>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, env = "GITBOT_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// Bot username on GitLab (without @)
    #[arg(long, env = "GITBOT_BOT_USERNAME")]
    pub bot_username: String,

    /// How often to poll for new mentions (in seconds)
    #[arg(long, env = "GITBOT_POLL_INTERVAL_SECONDS", default_value_t = 60)]
    pub poll_interval_seconds: u64,

    /// Number of days after which an issue is considered stale
    #[arg(long, env = "GITBOT_STALE_ISSUE_DAYS", default_value_t = 30)]
    pub stale_issue_days: u64,

    /// Maximum age in hours for issues and merge requests to pull (default: 24 hours)
    #[arg(long, env = "GITBOT_MAX_AGE_HOURS", default_value_t = 24)]
    pub max_age_hours: u64,

    /// Optional repository to use for additional context (format: group/project)
    #[arg(long, env = "GITBOT_CONTEXT_REPO_PATH")]
    pub context_repo_path: Option<String>,

    /// Maximum number of tokens of context to include (default: 60000)
    #[arg(long, env = "GITBOT_MAX_CONTEXT_SIZE", default_value_t = 60000)]
    pub max_context_size: usize,

    /// Maximum number of characters per comment to include in context (default: 1000)
    #[arg(long, env = "GITBOT_MAX_COMMENT_LENGTH", default_value_t = 1000)]
    pub max_comment_length: usize,

    /// Number of lines to include before and after keyword matches in file context (default: 10)
    #[arg(long, env = "GITBOT_CONTEXT_LINES", default_value_t = 10)]
    pub context_lines: usize,

    /// Default branch name to use for repository operations (default: main)
    #[arg(long, env = "GITBOT_DEFAULT_BRANCH", default_value = "main")]
    pub default_branch: String,

    /// Maximum number of tool calls allowed per bot invocation (default: 3, max: 10)
    #[arg(long, env = "GITBOT_MAX_TOOL_CALLS", default_value_t = 3)]
    pub max_tool_calls: u32,

    /// Path to client certificate file for OpenAI API authentication
    #[arg(long, env = "GITBOT_CLIENT_CERT_PATH")]
    pub client_cert_path: Option<String>,

    /// Path to client private key file for OpenAI API authentication
    #[arg(long, env = "GITBOT_CLIENT_KEY_PATH")]
    pub client_key_path: Option<String>,

    /// Password for client private key (environment variable only)
    /// This field is populated from GITBOT_CLIENT_KEY_PASSWORD environment variable
    /// No CLI argument is provided for security reasons
    pub client_key_password: Option<String>,

    /// Enable automatic issue triage (labeling unlabeled issues)
    #[arg(long, env = "GITBOT_AUTO_TRIAGE_ENABLED", default_value_t = true)]
    pub auto_triage_enabled: bool,

    /// Number of sample issues to analyze per label for learning label meanings
    #[arg(long, env = "GITBOT_LABEL_LEARNING_SAMPLES", default_value_t = 3)]
    pub label_learning_samples: usize,

    /// Number of hours to look back for unlabeled issues to triage (default: 24 hours)
    #[arg(long, env = "GITBOT_TRIAGE_LOOKBACK_HOURS", default_value_t = 24)]
    pub triage_lookback_hours: u64,
}

/// Validate that openai_token_mode is a valid option
fn validate_token_mode(value: &str) -> Result<String, String> {
    match value {
        "max_tokens" | "max_completion_tokens" => Ok(value.to_string()),
        _ => Err(format!(
            "openai_token_mode must be either 'max_tokens' or 'max_completion_tokens', got '{}'",
            value
        )),
    }
}

/// Validate that max_tool_calls is within reasonable bounds
fn validate_max_tool_calls(value: u32) -> Result<u32, String> {
    const MIN_TOOL_CALLS: u32 = 1;
    const MAX_TOOL_CALLS: u32 = 10;

    if value < MIN_TOOL_CALLS {
        Err(format!(
            "max_tool_calls must be at least {MIN_TOOL_CALLS}, got {value}"
        ))
    } else if value > MAX_TOOL_CALLS {
        Err(format!(
            "max_tool_calls must be at most {MAX_TOOL_CALLS}, got {value}"
        ))
    } else {
        Ok(value)
    }
}

/// Validate that openai_max_retries is within reasonable bounds
fn validate_max_retries(value: &str) -> Result<usize, String> {
    const MAX_RETRIES: usize = 10;

    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("openai_max_retries must be a number, got '{value}'"))?;

    if parsed > MAX_RETRIES {
        Err(format!(
            "openai_max_retries must be at most {MAX_RETRIES}, got {parsed}"
        ))
    } else {
        Ok(parsed)
    }
}

/// Validate that retry backoff multiplier is reasonable
fn validate_retry_backoff_multiplier(value: &str) -> Result<f64, String> {
    const MIN_MULTIPLIER: f64 = 1.0;
    const MAX_MULTIPLIER: f64 = 10.0;

    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("openai_retry_backoff_multiplier must be a number, got '{value}'"))?;

    if !(MIN_MULTIPLIER..=MAX_MULTIPLIER).contains(&parsed) {
        Err(format!(
            "openai_retry_backoff_multiplier must be between {MIN_MULTIPLIER} and {MAX_MULTIPLIER}, got {parsed}"
        ))
    } else {
        Ok(parsed)
    }
}

pub fn load_config() -> anyhow::Result<AppSettings> {
    // Parse command line arguments and environment variables
    let mut app_settings = AppSettings::parse();

    // Load client key password from environment variable only (no CLI argument for security)
    app_settings.client_key_password = env::var("GITBOT_CLIENT_KEY_PASSWORD").ok();

    // Validate token_mode for environment variables (CLI args are already validated by clap)
    if let Ok(token_mode) = env::var("GITBOT_OPENAI_TOKEN_MODE") {
        app_settings.openai_token_mode =
            validate_token_mode(&token_mode).map_err(|e| anyhow::anyhow!(e))?;
    }

    // Validate max_tool_calls
    app_settings.max_tool_calls =
        validate_max_tool_calls(app_settings.max_tool_calls).map_err(|e| anyhow::anyhow!(e))?;

    // Validate retry configuration
    app_settings.validate_retry_config()?;

    Ok(app_settings)
}

impl AppSettings {
    /// Validate retry configuration to prevent invalid values
    pub fn validate_retry_config(&self) -> anyhow::Result<()> {
        // Validate retry initial delay doesn't exceed reasonable bounds
        if self.openai_retry_initial_delay_ms > 60000 {
            anyhow::bail!(
                "openai_retry_initial_delay_ms cannot exceed 60 seconds, got {}",
                self.openai_retry_initial_delay_ms
            );
        }

        // Validate retry max delay doesn't exceed reasonable bounds
        if self.openai_retry_max_delay_ms > 300000 {
            anyhow::bail!(
                "openai_retry_max_delay_ms cannot exceed 5 minutes, got {}",
                self.openai_retry_max_delay_ms
            );
        }

        // Ensure initial delay is less than max delay
        if self.openai_retry_initial_delay_ms >= self.openai_retry_max_delay_ms {
            anyhow::bail!(
                "openai_retry_initial_delay_ms ({}) must be less than openai_retry_max_delay_ms ({})",
                self.openai_retry_initial_delay_ms,
                self.openai_retry_max_delay_ms
            );
        }

        Ok(())
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            gitlab_url: "https://gitlab.com".to_string(),
            prompt_prefix: None,
            gitlab_token: "default_token".to_string(),
            openai_api_key: "default_key".to_string(),
            openai_custom_url: "https://api.openai.com/v1".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            openai_token_mode: "max_tokens".to_string(),
            openai_timeout_secs: 120,
            openai_connect_timeout_secs: 10,
            openai_max_retries: 3,
            openai_retry_initial_delay_ms: 1000,
            openai_retry_max_delay_ms: 30000,
            openai_retry_backoff_multiplier: 2.0,
            repos_to_poll: Vec::new(),
            log_level: "info".to_string(),
            bot_username: "gitbot".to_string(),
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
            auto_triage_enabled: true,
            label_learning_samples: 3,
            triage_lookback_hours: 24,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_token_mode_valid_values() {
        // Test valid values
        assert_eq!(
            validate_token_mode("max_tokens"),
            Ok("max_tokens".to_string())
        );
        assert_eq!(
            validate_token_mode("max_completion_tokens"),
            Ok("max_completion_tokens".to_string())
        );
    }

    #[test]
    fn test_validate_token_mode_invalid_values() {
        // Test invalid values
        assert!(validate_token_mode("invalid").is_err());
        let err = validate_token_mode("invalid").unwrap_err();
        assert!(err.contains("must be either 'max_tokens' or 'max_completion_tokens'"));

        assert!(validate_token_mode("max_completion_token").is_err());
        assert!(validate_token_mode("MAX_TOKENS").is_err());
    }

    #[test]
    fn test_validate_max_tool_calls_valid_values() {
        // Test valid values within range
        assert_eq!(validate_max_tool_calls(1), Ok(1));
        assert_eq!(validate_max_tool_calls(5), Ok(5));
        assert_eq!(validate_max_tool_calls(10), Ok(10));
    }

    #[test]
    fn test_validate_max_tool_calls_invalid_values() {
        // Test values below minimum
        assert!(validate_max_tool_calls(0).is_err());
        let err = validate_max_tool_calls(0).unwrap_err();
        assert!(err.contains("must be at least 1"));

        // Test values above maximum
        assert!(validate_max_tool_calls(11).is_err());
        let err = validate_max_tool_calls(11).unwrap_err();
        assert!(err.contains("must be at most 10"));
    }

    #[test]
    fn test_validate_max_retries_valid_values() {
        // Test valid values
        assert_eq!(validate_max_retries("0"), Ok(0));
        assert_eq!(validate_max_retries("3"), Ok(3));
        assert_eq!(validate_max_retries("10"), Ok(10));
    }

    #[test]
    fn test_validate_max_retries_invalid_values() {
        // Test value above maximum
        assert!(validate_max_retries("11").is_err());
        let err = validate_max_retries("11").unwrap_err();
        assert!(err.contains("must be at most 10"));
    }

    #[test]
    fn test_validate_retry_backoff_multiplier_valid_values() {
        // Test valid values within range
        assert_eq!(validate_retry_backoff_multiplier("1.0"), Ok(1.0));
        assert_eq!(validate_retry_backoff_multiplier("2.0"), Ok(2.0));
        assert_eq!(validate_retry_backoff_multiplier("10.0"), Ok(10.0));
    }

    #[test]
    fn test_validate_retry_backoff_multiplier_invalid_values() {
        // Test values below minimum
        assert!(validate_retry_backoff_multiplier("0.9").is_err());
        let err = validate_retry_backoff_multiplier("0.9").unwrap_err();
        assert!(err.contains("must be between"));

        // Test values above maximum
        assert!(validate_retry_backoff_multiplier("10.1").is_err());
        let err = validate_retry_backoff_multiplier("10.1").unwrap_err();
        assert!(err.contains("must be between"));
    }

    #[test]
    fn test_validate_retry_config() {
        // Valid configuration
        let mut config = AppSettings {
            openai_retry_initial_delay_ms: 1000,
            openai_retry_max_delay_ms: 30000,
            ..Default::default()
        };
        assert!(config.validate_retry_config().is_ok());

        // Initial delay exceeds maximum
        config.openai_retry_initial_delay_ms = 70000;
        assert!(config.validate_retry_config().is_err());

        // Max delay exceeds maximum
        config.openai_retry_initial_delay_ms = 1000;
        config.openai_retry_max_delay_ms = 400000;
        assert!(config.validate_retry_config().is_err());

        // Initial delay equals max delay
        config.openai_retry_initial_delay_ms = 30000;
        config.openai_retry_max_delay_ms = 30000;
        assert!(config.validate_retry_config().is_err());

        // Initial delay greater than max delay
        config.openai_retry_initial_delay_ms = 40000;
        config.openai_retry_max_delay_ms = 30000;
        assert!(config.validate_retry_config().is_err());
    }
}
