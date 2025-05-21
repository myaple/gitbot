use serde::Deserialize;
use std::fmt::Debug;

#[derive(Debug, Deserialize)]
pub struct AppSettings {
    pub gitlab_url: String,
    pub gitlab_token: String,
    pub gitlab_webhook_secret: String,
    pub openai_api_key: String,
    pub openai_custom_url: String,
    pub server_address: String,
    #[serde(deserialize_with = "deserialize_whitelisted_repos")]
    pub whitelisted_repos: Vec<String>,
    pub log_level: String,
    pub bot_username: String,
}

fn deserialize_whitelisted_repos<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(s.split(',').map(|item| item.trim().to_string()).collect())
}

pub fn load_config() -> Result<AppSettings, config::ConfigError> {
    dotenvy::dotenv().ok();

    let config = config::Config::builder()
        .add_source(config::Environment::default().prefix("APP").separator("_"))
        .build()?;

    config.try_deserialize::<AppSettings>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_load_config() {
        // Set mock environment variables
        env::set_var("APP_GITLAB_URL", "https://gitlab.example.com");
        env::set_var("APP_GITLAB_TOKEN", "test_gitlab_token");
        env::set_var("APP_GITLAB_WEBHOOK_SECRET", "test_webhook_secret");
        env::set_var("APP_OPENAI_API_KEY", "test_openai_key");
        env::set_var("APP_OPENAI_CUSTOM_URL", "https://api.openai.com/v1");
        env::set_var("APP_SERVER_ADDRESS", "127.0.0.1:8888");
        env::set_var("APP_WHITELISTED_REPOS", "org/repo1,group/project2");
        env::set_var("APP_LOG_LEVEL", "debug");
        env::set_var("APP_BOT_USERNAME", "test_bot");

        let app_settings = load_config();

        assert!(app_settings.is_ok());
        let settings = app_settings.unwrap();

        assert_eq!(settings.gitlab_url, "https://gitlab.example.com");
        assert_eq!(settings.gitlab_token, "test_gitlab_token");
        assert_eq!(settings.gitlab_webhook_secret, "test_webhook_secret");
        assert_eq!(settings.openai_api_key, "test_openai_key");
        assert_eq!(settings.openai_custom_url, "https://api.openai.com/v1");
        assert_eq!(settings.server_address, "127.0.0.1:8888");
        assert_eq!(
            settings.whitelisted_repos,
            vec!["org/repo1".to_string(), "group/project2".to_string()]
        );
        assert_eq!(settings.log_level, "debug");
        assert_eq!(settings.bot_username, "test_bot");

        // Unset environment variables
        env::remove_var("APP_GITLAB_URL");
        env::remove_var("APP_GITLAB_TOKEN");
        env::remove_var("APP_GITLAB_WEBHOOK_SECRET");
        env::remove_var("APP_OPENAI_API_KEY");
        env::remove_var("APP_OPENAI_CUSTOM_URL");
        env::remove_var("APP_SERVER_ADDRESS");
        env::remove_var("APP_WHITELISTED_REPOS");
        env::remove_var("APP_LOG_LEVEL");
        env::remove_var("APP_BOT_USERNAME");
    }
}
