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

    /// Optional repository to use for additional context (format: group/project)
    #[arg(long, env = "GITBOT_CONTEXT_REPO_PATH")]
    pub context_repo_path: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    // #[test]
    // fn test_parse_repos_list() {
    //     let input = "group1/project1,group2/project2, group3/project3";
    //     let result = parse_repos_list(input).unwrap();
    //     assert_eq!(
    //         result,
    //         vec!["group1/project1", "group2/project2", "group3/project3"]
    //     );
    // }

    #[test]
    fn test_create_app_settings() {
        // Create AppSettings directly for testing
        let settings = AppSettings {
            gitlab_url: "https://gitlab.example.com".to_string(),
            gitlab_token: "test_gitlab_token".to_string(),
            openai_api_key: "test_openai_key".to_string(),
            openai_custom_url: "https://api.openai.com/v1".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            repos_to_poll: vec!["org/repo1".to_string(), "group/project2".to_string()],
            log_level: "debug".to_string(),
            bot_username: "test_bot".to_string(),
            poll_interval_seconds: 300,
            stale_issue_days: 30,
            context_repo_path: Some("org/context-repo".to_string()),
        };

        assert_eq!(settings.gitlab_url, "https://gitlab.example.com");
        assert_eq!(settings.gitlab_token, "test_gitlab_token");
        assert_eq!(settings.openai_api_key, "test_openai_key");
        assert_eq!(settings.openai_custom_url, "https://api.openai.com/v1");
        assert_eq!(settings.openai_model, "gpt-3.5-turbo");
        assert_eq!(settings.openai_temperature, 0.7);
        assert_eq!(settings.openai_max_tokens, 1024);
        assert_eq!(settings.repos_to_poll, vec!["org/repo1", "group/project2"]);
        assert_eq!(settings.log_level, "debug");
        assert_eq!(settings.bot_username, "test_bot");
        assert_eq!(settings.poll_interval_seconds, 300);
        assert_eq!(settings.stale_issue_days, 30);
        assert_eq!(
            settings.context_repo_path,
            Some("org/context-repo".to_string())
        );
    }
}
