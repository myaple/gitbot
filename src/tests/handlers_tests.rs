#[cfg(test)]
mod tests {
    use crate::config::AppSettings;
    use crate::file_indexer::FileIndexManager;
    use crate::gitlab::GitlabApiClient;
    use crate::handlers::*;
    use crate::mention_cache::MentionCache;
    use crate::models::{
        GitlabIssue, GitlabNoteAttributes, GitlabNoteEvent, GitlabNoteObject, GitlabProject,
        GitlabUser,
    };
    use chrono::{Duration as ChronoDuration, Utc};
    use mockito::Matcher;
    use serde_json::json;
    use std::sync::Arc;

    const TEST_MENTION_ID: i64 = 12345;
    const TEST_PROJECT_ID: i64 = 1;
    const TEST_ISSUE_IID: i64 = 101;
    const TEST_BOT_USERNAME: &str = "test_bot";
    const TEST_USER_USERNAME: &str = "test_user";
    const TEST_GENERIC_USER_ID: i64 = 2; // For generic users like issue authors
    const TEST_BOT_USER_ID: i64 = 99; // For the bot user

    // Helper to create a basic AppSettings for tests
    fn test_app_settings(base_url: String) -> Arc<AppSettings> {
        Arc::new(AppSettings {
            gitlab_url: base_url.clone(), // Cloning base_url if used for both
            gitlab_token: "test_token".to_string(),
            openai_api_key: "test_openai_key".to_string(),
            openai_custom_url: base_url, // Corrected to use the mock server's URL
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 150,
            repos_to_poll: vec!["test_org/test_repo".to_string()],
            log_level: "debug".to_string(),
            bot_username: TEST_BOT_USERNAME.to_string(),
            poll_interval_seconds: 60,
            default_branch: "main".to_string(),
            client_cert_path: None,
            client_key_path: None,
            client_key_password: None,
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: None,
            max_context_size: 60000,
        })
    }

    // Simple wrapper around create_test_note_event_with_id with defaults
    fn create_test_note_event(username: &str, noteable_type: &str) -> GitlabNoteEvent {
        create_test_note_event_with_id(username, noteable_type, 123, None, None)
    }

    // Updated helper to create a test note event, allowing mention ID override
    fn create_test_note_event_with_id(
        username: &str,
        noteable_type: &str,
        mention_id: i64,
        note_content: Option<String>,
        updated_at: Option<String>,
    ) -> GitlabNoteEvent {
        let user = GitlabUser {
            id: if username == TEST_BOT_USERNAME {
                TEST_BOT_USER_ID
            } else {
                TEST_GENERIC_USER_ID
            },
            username: username.to_string(),
            name: format!("{} User", username),
            avatar_url: None,
        };

        let project = GitlabProject {
            id: TEST_PROJECT_ID,
            path_with_namespace: "org/repo1".to_string(),
            web_url: "https://gitlab.example.com/org/repo1".to_string(),
        };

        let default_note = format!(
            "Hello @{} please help with this {}",
            TEST_BOT_USERNAME,
            noteable_type.to_lowercase()
        );

        let note_attributes = GitlabNoteAttributes {
            id: mention_id,
            note: note_content.unwrap_or(default_note),
            author: user.clone(),
            project_id: TEST_PROJECT_ID,
            noteable_type: noteable_type.to_string(),
            noteable_id: Some(1), // Corresponds to Issue/MR ID
            iid: Some(if noteable_type == "Issue" {
                TEST_ISSUE_IID
            } else {
                202 // MR IID
            }),
            url: Some(format!(
                "https://gitlab.example.com/org/repo1/-/issues/{}#note_{}",
                TEST_ISSUE_IID, mention_id
            )),
            updated_at: updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()),
        };

        let issue = if noteable_type == "Issue" {
            Some(GitlabNoteObject {
                id: 1, // Matches noteable_id
                iid: TEST_ISSUE_IID,
                title: "Test Issue".to_string(),
                description: Some("This is a test issue".to_string()),
            })
        } else {
            None
        };

        let merge_request = if noteable_type == "MergeRequest" {
            Some(GitlabNoteObject {
                id: 1,    // Matches noteable_id
                iid: 202, // MR IID
                title: "Test Merge Request".to_string(),
                description: Some("This is a test merge request".to_string()),
            })
        } else {
            None
        };

        GitlabNoteEvent {
            object_kind: "note".to_string(),
            event_type: "note".to_string(),
            user,
            project,
            object_attributes: note_attributes,
            issue,
            merge_request,
        }
    }

    #[test]
    fn test_extract_context_after_mention() {
        let bot_name = "mybot";

        // Basic case
        let note1 = "Hello @mybot please summarize this";
        assert_eq!(
            extract_context_after_mention(note1, bot_name),
            Some("please summarize this".to_string())
        );

        // With leading/trailing whitespace for context
        let note2 = "@mybot  summarize this for me  ";
        assert_eq!(
            extract_context_after_mention(note2, bot_name),
            Some("summarize this for me".to_string())
        );

        // No context after mention
        let note3 = "Thanks @mybot";
        assert_eq!(extract_context_after_mention(note3, bot_name), None);

        // No context after mention but with spaces
        let note4 = "Thanks @mybot   ";
        assert_eq!(extract_context_after_mention(note4, bot_name), None);

        // Mention at the end of the string
        let note5 = "Can you help @mybot";
        assert_eq!(extract_context_after_mention(note5, bot_name), None);

        // Mention in the middle, but no actual command after it before other text
        let note6 = "@mybot, what do you think?"; // Assumes comma is part of context
        assert_eq!(
            extract_context_after_mention(note6, bot_name),
            Some(", what do you think?".to_string())
        );

        // No mention
        let note7 = "This is a regular comment.";
        assert_eq!(extract_context_after_mention(note7, bot_name), None);

        // Different bot mentioned
        let note8 = "Hey @otherbot what's up?";
        assert_eq!(extract_context_after_mention(note8, bot_name), None);

        // Mention with mixed case (current implementation is case-sensitive)
        let note9 = "Hey @MyBot summarize";
        assert_eq!(extract_context_after_mention(note9, bot_name), None); // Fails as bot_name is "mybot"

        // Multiple mentions, should pick first
        let note10 = "@mybot summarize this, and also @mybot do that";
        assert_eq!(
            extract_context_after_mention(note10, bot_name),
            Some("summarize this, and also @mybot do that".to_string())
        );
    }

    #[tokio::test]
    async fn test_process_mention_no_bot_mention() {
        // Create a test event where the bot is not mentioned
        let mut event = create_test_note_event("user", "Issue");
        // Override the note content to remove bot mention
        event.object_attributes.note = "This is a comment without any bot mention".to_string();

        // Create test config
        let config = Arc::new(AppSettings {
            gitlab_url: "https://gitlab.example.com".to_string(),
            gitlab_token: "test_token".to_string(),
            openai_api_key: "test_key".to_string(),
            openai_custom_url: "https://api.openai.com/v1".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            repos_to_poll: vec!["test/repo".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: None,
            max_context_size: 60000,
            default_branch: "main".to_string(),
            client_cert_path: None,
            client_key_path: None,
            client_key_password: None,
        });

        // Create a mock GitLab client
        let server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = AppSettings {
            gitlab_url: base_url,
            gitlab_token: "test_token".to_string(),
            openai_api_key: "test_key".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            openai_custom_url: "https://api.openai.com/v1".to_string(),
            repos_to_poll: vec!["test/repo".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            max_age_hours: 24,
            stale_issue_days: 30, // Added default for tests
            context_repo_path: None,
            max_context_size: 60000,
            default_branch: "main".to_string(),
            client_cert_path: None,
            client_key_path: None,
            client_key_password: None,
        };
        let gitlab_client = Arc::new(GitlabApiClient::new(Arc::new(settings.clone())).unwrap());

        // Create a cache for the test
        let cache = MentionCache::new();

        // Create a file index manager for the test
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        // Process the mention
        let result =
            process_mention(event, gitlab_client, config, &cache, file_index_manager).await; // Pass as reference

        // Should return Ok since we're ignoring comments without mentions
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_mention_with_no_bot_mention() {
        // Create a test event with no bot mention
        let mut event = create_test_note_event("user1", "Issue");
        event.object_attributes.note = "This is a comment with no bot mention".to_string();

        // Create test config
        let config = Arc::new(AppSettings {
            gitlab_url: "https://gitlab.example.com".to_string(),
            gitlab_token: "test_token".to_string(),
            openai_api_key: "test_key".to_string(),
            openai_custom_url: "https://api.openai.com/v1".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            max_age_hours: 24,
            poll_interval_seconds: 60,
            stale_issue_days: 30, // Added default for tests
            context_repo_path: None,
            max_context_size: 60000,
            default_branch: "main".to_string(),
            client_cert_path: None,
            client_key_path: None,
            client_key_password: None,
        });

        // Create a cache for the test
        let cache = MentionCache::new();

        // Create a mock GitLab client
        let server = mockito::Server::new_async().await;
        let base_url = server.url();
        let settings = AppSettings {
            gitlab_url: base_url,
            gitlab_token: "test_token".to_string(),
            openai_api_key: "test_key".to_string(),
            openai_model: "gpt-3.5-turbo".to_string(),
            openai_temperature: 0.7,
            openai_max_tokens: 1024,
            openai_custom_url: "https://api.openai.com/v1".to_string(),
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            max_age_hours: 24,
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            stale_issue_days: 30, // Added default for tests
            context_repo_path: None,
            max_context_size: 60000,
            default_branch: "main".to_string(),
            client_cert_path: None,
            client_key_path: None,
            client_key_password: None,
        };
        let gitlab_client = Arc::new(GitlabApiClient::new(Arc::new(settings.clone())).unwrap());
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        // Process the mention
        let result =
            process_mention(event, gitlab_client, config, &cache, file_index_manager).await; // Pass as reference

        // Should return Ok since we're ignoring comments without mentions
        assert!(result.is_ok());
    }

    // Test Cache Miss and Successful Processing
    #[tokio::test]
    async fn test_cache_miss_and_successful_processing() {
        let mut server = mockito::Server::new_async().await;
        let config = test_app_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let cache = MentionCache::new(); // Use new MentionCache

        let event_time = Utc::now();
        let event = create_test_note_event_with_id(
            TEST_USER_USERNAME,
            "Issue",
            TEST_MENTION_ID,
            Some(format!("Hello @{} please summarize", TEST_BOT_USERNAME)),
            Some(event_time.to_rfc3339()),
        );

        // 1. Mock Gitlab: get_issue_notes (for de-duplication check) - return empty
        let _m_get_notes = server
            .mock(
                "GET",
                Matcher::Regex(format!(
                    r"/api/v4/projects/{}/issues/{}/notes\?.+",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        // 2. Mock Gitlab: get_issue
        let mock_issue = GitlabIssue {
            id: 1,
            iid: TEST_ISSUE_IID,
            project_id: TEST_PROJECT_ID,
            title: "Test Issue".to_string(),
            description: Some("Issue description here.".to_string()),
            state: "opened".to_string(),
            author: GitlabUser {
                // Author of the issue itself
                id: TEST_GENERIC_USER_ID + 1, // Different from the commenting user or bot
                username: "issue_author".to_string(),
                name: "Issue Author".to_string(),
                avatar_url: None,
            },
            labels: vec![],
            web_url: "url".to_string(),
            updated_at: event_time.to_rfc3339(),
        };
        let _m_get_issue = server
            .mock(
                "GET",
                format!(
                    "/api/v4/projects/{}/issues/{}",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )
                .as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!(mock_issue).to_string())
            .create_async()
            .await;

        // Mock get_file_content for repo_context (CONTRIBUTING.md - will 404)
        let _m_get_contrib_md = server
            .mock("GET", Matcher::Regex(r".*CONTRIBUTING.md.*".to_string()))
            .with_status(404)
            .create_async()
            .await;

        // 3. Mock OpenAI: send_chat_completion
        let _m_openai = server
            .mock(
                "POST",
                Matcher::Exact(format!("/{}", crate::openai::OPENAI_CHAT_COMPLETIONS_PATH)),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .match_header(
                "Authorization",
                format!("Bearer {}", config.openai_api_key).as_str(),
            )
            .with_body(
                json!({
                    "id": "chatcmpl-test-handler",
                    "object": "chat.completion",
                    "created": 1677652288,
                    "model": config.openai_model.clone(),
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Mocked OpenAI response."
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 10,
                        "total_tokens": 20
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        // 4. Mock Gitlab: post_comment_to_issue
        let _m_post_comment = server
            .mock(
                "POST",
                format!(
                    "/api/v4/projects/{}/issues/{}/notes",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )
                .as_str(),
            )
            .with_status(201) // Successfully created
            .with_header("content-type", "application/json")
            .with_body(json!({
                "id": 999, // ID of the new note
                "note": "Posted comment", // Matches 'note' field in GitlabNoteAttributes
                "author": {
                    "id": TEST_BOT_USER_ID, // The bot is the author of the reply
                    "username": config.bot_username.clone(),
                    "name": format!("{} Bot", config.bot_username),
                    "avatar_url": null,
                    "state": "active",
                    "web_url": format!("https://gitlab.example.com/{}", config.bot_username)
                },
                "project_id": TEST_PROJECT_ID,
                "noteable_type": "Issue",
                // For noteable_id, use the actual ID of the issue if available, not IID.
                // Assuming event.issue.as_ref().unwrap().id is the correct one if it exists.
                // For this mock, event.issue.as_ref().unwrap().id is 1.
                "noteable_id": event.issue.as_ref().unwrap().id,
                "iid": event.issue.as_ref().unwrap().iid, // This is the issue's IID
                "created_at": Utc::now().to_rfc3339(),
                "updated_at": Utc::now().to_rfc3339(),
                "system": false,
                "url": format!("https://gitlab.example.com/org/repo1/-/issues/{}/notes/999", event.issue.as_ref().unwrap().iid)
            }).to_string())
            .create_async()
            .await;

        // RepoContextExtractor related mocks (get_file_content for files, list_repository_tree)
        // Assuming no specific files are successfully fetched for simplicity, all 404
        let _m_get_any_file = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/.*/repository/files/.*".to_string()),
            )
            .with_status(404)
            .create_async()
            .await;
        let _m_list_tree = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/.*/repository/tree.*".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([]).to_string()) // Empty tree
            .create_async()
            .await;
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let result = process_mention(
            event,
            gitlab_client.clone(),
            config.clone(),
            &cache,
            file_index_manager,
        )
        .await; // Pass as reference

        assert!(result.is_ok(), "Processing failed: {:?}", result.err());
        assert!(cache.check(TEST_MENTION_ID).await); // Use new check method
    }

    // Test Cache Hit
    #[tokio::test]
    async fn test_cache_hit() {
        let mut server = mockito::Server::new_async().await;
        let config = test_app_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let cache = MentionCache::new(); // Use new MentionCache
        cache.add(TEST_MENTION_ID).await; // Pre-populate cache

        let event = create_test_note_event_with_id(
            TEST_USER_USERNAME,
            "Issue",
            TEST_MENTION_ID, // Same ID as in cache
            None,
            None,
        );

        // Mock for get_issue_notes - this SHOULD NOT be called.
        // If mockito supported .times(0) easily with _async, we'd use it.
        // Instead, we define it but don't assert it, or make it fail if called.
        // For this test, not defining further mocks is key.
        let m_get_notes_uncalled = server
            .mock(
                "GET",
                Matcher::Regex(format!(
                    r"/api/v4/projects/{}/issues/{}/notes\?.+",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )),
            )
            .with_status(500) // Should fail test if called
            .create_async()
            .await;
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let result =
            process_mention(event, gitlab_client, config, &cache, file_index_manager).await; // Pass as reference

        assert!(result.is_ok());
        m_get_notes_uncalled.expect(0).assert_async().await; // Explicitly assert not called
                                                             // No other mocks for OpenAI or posting comments should be called.
    }

    // Test Cache Update on Existing De-duplication Logic Trigger
    #[tokio::test]
    async fn test_cache_update_on_deduplication_trigger() {
        let mut server = mockito::Server::new_async().await;
        let config = test_app_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let cache = MentionCache::new(); // Empty cache initially

        let mention_time = Utc::now();
        let bot_reply_time = mention_time + ChronoDuration::seconds(10);

        let event = create_test_note_event_with_id(
            TEST_USER_USERNAME,
            "Issue",
            TEST_MENTION_ID,
            Some(format!("Hello @{}", TEST_BOT_USERNAME)),
            Some(mention_time.to_rfc3339()),
        );

        // Mock Gitlab: get_issue_notes returns a note from the bot, after the mention
        let bot_note = GitlabNoteAttributes {
            id: TEST_MENTION_ID + 1,
            note: "I already replied to this.".to_string(),
            author: GitlabUser {
                id: 99, // Bot's user ID
                username: TEST_BOT_USERNAME.to_string(),
                name: "Test Bot".to_string(),
                avatar_url: None,
            },
            project_id: TEST_PROJECT_ID,
            noteable_type: "Issue".to_string(),
            noteable_id: Some(1),
            iid: Some(TEST_ISSUE_IID),
            url: None,
            updated_at: bot_reply_time.to_rfc3339(),
        };
        let _m_get_notes = server
            .mock(
                "GET",
                Matcher::Regex(format!(
                    r"/api/v4/projects/{}/issues/{}/notes\?.+",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([bot_note]).to_string())
            .create_async()
            .await;

        // Mocks for OpenAI and post_comment should not be called
        let m_openai_uncalled = server
            .mock("POST", Matcher::Any) // Broad matcher for OpenAI
            .with_status(500) // Fail if called
            .create_async()
            .await;
        let m_post_comment_uncalled = server
            .mock("POST", Matcher::Regex(r".*/notes".to_string())) // Broad for post comment
            .with_status(500) // Fail if called
            .create_async()
            .await;
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let result =
            process_mention(event, gitlab_client, config, &cache, file_index_manager).await; // Pass as reference

        assert!(result.is_ok());
        assert!(cache.check(TEST_MENTION_ID).await); // Original mention ID added to cache
        m_openai_uncalled.expect(0).assert_async().await;
        m_post_comment_uncalled.expect(0).assert_async().await;
    }

    // Test No Cache Update on Processing Failure
    #[tokio::test]
    async fn test_no_cache_update_on_processing_failure() {
        let mut server = mockito::Server::new_async().await;
        let config = test_app_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let cache = MentionCache::new(); // Empty cache

        let event = create_test_note_event_with_id(
            TEST_USER_USERNAME,
            "Issue",
            TEST_MENTION_ID,
            Some(format!("Hello @{}", TEST_BOT_USERNAME)),
            None,
        );

        // Mock Gitlab: get_issue_notes (for de-duplication) returns empty
        let _m_get_notes = server
            .mock(
                "GET",
                Matcher::Regex(format!(
                    r"/api/v4/projects/{}/issues/{}/notes\?.+",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )),
            )
            .with_status(200)
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        // Mock Gitlab: get_issue returns an error
        let _m_get_issue_fail = server
            .mock(
                "GET",
                format!(
                    "/api/v4/projects/{}/issues/{}",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )
                .as_str(),
            )
            .with_status(500) // Simulate server error
            .with_body("Internal Server Error")
            .create_async()
            .await;
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let result =
            process_mention(event, gitlab_client, config, &cache, file_index_manager).await; // Pass as reference

        assert!(result.is_err());
        assert!(!cache.check(TEST_MENTION_ID).await); // Cache should NOT contain the ID
    }
}
