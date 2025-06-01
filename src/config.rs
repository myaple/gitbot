use clap::Parser;
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

    /// Maximum number of characters of context to include (default: 60000)
    #[arg(long, env = "GITBOT_MAX_CONTEXT_SIZE", default_value_t = 60000)]
    pub max_context_size: usize,

    /// Default branch name to use for repository operations (default: main)
    #[arg(long, env = "GITBOT_DEFAULT_BRANCH", default_value = "main")]
    pub default_branch: String,
}

// fn parse_repos_list(s: &str) -> Result<Vec<String>, String> {
// Ok(s.split(',')
// .map(|item| item.trim().to_string())
// .filter(|s| !s.is_empty())
// .collect())
// }

pub fn load_config() -> anyhow::Result<AppSettings> {
    // Parse command line arguments and environment variables
    let app_settings = AppSettings::parse();
    Ok(app_settings)
}
