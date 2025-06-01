use crate::config::AppSettings;

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
        max_age_hours: 24,
        context_repo_path: Some("org/context-repo".to_string()),
        max_context_size: 60000,
        default_branch: "main".to_string(),
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
    assert_eq!(settings.max_age_hours, 24);
    assert_eq!(
        settings.context_repo_path,
        Some("org/context-repo".to_string())
    );
}
