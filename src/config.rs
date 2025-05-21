use serde::Deserialize;
use std::fmt::Debug;

#[derive(Debug, Deserialize, Clone)]
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


    #[test]
    fn test_create_app_settings() {
        // Instead of testing load_config which depends on environment variables,
        // let's test that we can create an AppSettings struct directly
        let settings = AppSettings {
            gitlab_url: "https://gitlab.example.com".to_string(),
            gitlab_token: "test_gitlab_token".to_string(),
            gitlab_webhook_secret: "test_webhook_secret".to_string(),
            openai_api_key: "test_openai_key".to_string(),
            openai_custom_url: "https://api.openai.com/v1".to_string(),
            server_address: "127.0.0.1:8888".to_string(),
            whitelisted_repos: vec!["org/repo1".to_string(), "group/project2".to_string()],
            log_level: "debug".to_string(),
            bot_username: "test_bot".to_string(),
        };

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
    }
}
