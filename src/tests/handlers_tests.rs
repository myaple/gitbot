#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
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
    use crate::openai::OpenAIApiClient;
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
        let mut settings = AppSettings::default();
        settings.gitlab_url = base_url.clone();
        settings.gitlab_token = "test_token".to_string();
        settings.openai_api_key = "test_openai_key".to_string();
        settings.openai_custom_url = base_url;
        settings.openai_max_tokens = 150;
        settings.bot_username = TEST_BOT_USERNAME.to_string();
        Arc::new(settings)
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
        let mut config = AppSettings::default();
        // Set only the non-default fields
        config.gitlab_url = "https://gitlab.example.com".to_string();
        config.gitlab_token = "test_token".to_string();
        config.openai_api_key = "test_key".to_string();
        config.repos_to_poll = vec!["test/repo".to_string()];
        config.log_level = "debug".to_string();
        config.bot_username = "gitbot".to_string();
        let config = Arc::new(config);

        // Create a mock GitLab client
        let server = mockito::Server::new_async().await;
        let base_url = server.url();
        let mut settings = AppSettings::default();
        settings.gitlab_url = base_url.clone();
        settings.gitlab_token = "test_token".to_string();
        settings.openai_api_key = "test_key".to_string();
        let gitlab_client = Arc::new(GitlabApiClient::new(Arc::new(settings.clone())).unwrap());

        // Create a cache for the test
        let cache = MentionCache::new();

        // Create a file index manager for the test
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let openai_client = Arc::new(OpenAIApiClient::new(&config).unwrap());

        // Process the mention
        let result =
            process_mention(event, gitlab_client, openai_client, config, &cache, file_index_manager).await; // Pass as reference

        // Should return Ok since we're ignoring comments without mentions
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_mention_with_no_bot_mention() {
        // Create a test event with no bot mention
        let mut event = create_test_note_event("user1", "Issue");
        event.object_attributes.note = "This is a comment with no bot mention".to_string();

        // Create test config
        let mut config = AppSettings::default();
        // Set only the non-default fields
        config.gitlab_url = "https://gitlab.example.com".to_string();
        config.gitlab_token = "test_token".to_string();
        config.openai_api_key = "test_key".to_string();
        config.repos_to_poll = vec!["org/repo1".to_string()];
        config.log_level = "debug".to_string();
        config.bot_username = "gitbot".to_string();
        let config = Arc::new(config);

        // Create a cache for the test
        let cache = MentionCache::new();

        // Create a mock GitLab client
        let server = mockito::Server::new_async().await;
        let base_url = server.url();
        let mut settings = AppSettings::default();
        // Set only the non-default fields
        settings.gitlab_url = base_url;
        settings.gitlab_token = "test_token".to_string();
        settings.openai_api_key = "test_key".to_string();
        settings.repos_to_poll = vec!["org/repo1".to_string()];
        settings.log_level = "debug".to_string();
        settings.bot_username = "gitbot".to_string();
        let gitlab_client = Arc::new(GitlabApiClient::new(Arc::new(settings.clone())).unwrap());
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let openai_client = Arc::new(OpenAIApiClient::new(&config).unwrap());

        // Process the mention
        let result =
            process_mention(event, gitlab_client, openai_client, config, &cache, file_index_manager).await; // Pass as reference

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
            created_at: "2024-01-01T00:00:00Z".to_string(),
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

        let openai_client = Arc::new(OpenAIApiClient::new(&config).unwrap());

        let result = process_mention(
            event,
            gitlab_client.clone(),
            openai_client,
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

        let openai_client = Arc::new(OpenAIApiClient::new(&config).unwrap());

        let result =
            process_mention(event, gitlab_client, openai_client, config, &cache, file_index_manager).await; // Pass as reference

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

        let openai_client = Arc::new(OpenAIApiClient::new(&config).unwrap());

        let result =
            process_mention(event, gitlab_client, openai_client, config, &cache, file_index_manager).await; // Pass as reference

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

        let openai_client = Arc::new(OpenAIApiClient::new(&config).unwrap());

        let result =
            process_mention(event, gitlab_client, openai_client, config, &cache, file_index_manager).await; // Pass as reference

        assert!(result.is_err());
        assert!(!cache.check(TEST_MENTION_ID).await); // Cache should NOT contain the ID
    }

    #[test]
    fn test_format_comments_for_context() {
        // Create test notes
        let notes = vec![
            GitlabNoteAttributes {
                id: 1,
                note: "This is the first comment".to_string(),
                author: GitlabUser {
                    id: 123,
                    username: "user1".to_string(),
                    name: "User One".to_string(),
                    avatar_url: None,
                },
                project_id: 1,
                noteable_type: "Issue".to_string(),
                noteable_id: Some(10),
                iid: Some(10),
                url: None,
                updated_at: "2023-01-01T12:00:00Z".to_string(),
            },
            GitlabNoteAttributes {
                id: 2,
                note: "This is a very long comment that should be truncated because it exceeds the maximum comment length that we have configured for this test case and should result in a truncated version with ellipsis".to_string(),
                author: GitlabUser {
                    id: 456,
                    username: "user2".to_string(),
                    name: "User Two".to_string(),
                    avatar_url: None,
                },
                project_id: 1,
                noteable_type: "Issue".to_string(),
                noteable_id: Some(10),
                iid: Some(10),
                url: None,
                updated_at: "2023-01-02T14:30:00Z".to_string(),
            },
            GitlabNoteAttributes {
                id: 3,
                note: "This is the current comment that triggered the bot".to_string(),
                author: GitlabUser {
                    id: 789,
                    username: "user3".to_string(),
                    name: "User Three".to_string(),
                    avatar_url: None,
                },
                project_id: 1,
                noteable_type: "Issue".to_string(),
                noteable_id: Some(10),
                iid: Some(10),
                url: None,
                updated_at: "2023-01-03T09:15:00Z".to_string(),
            },
        ];

        let max_comment_length = 50;
        let current_note_id = 3;

        let result = format_comments_for_context(&notes, max_comment_length, current_note_id);

        // Should contain two comments (skipping the current one)
        assert!(result.contains("user1"));
        assert!(result.contains("user2"));
        assert!(!result.contains("user3"));

        // Should contain the first comment in full
        assert!(result.contains("This is the first comment"));

        // Should contain truncated version of the long comment
        assert!(result.contains("... [truncated]"));

        // Should contain formatted timestamps
        assert!(result.contains("2023-01-01 12:00 UTC"));
        assert!(result.contains("2023-01-02 14:30 UTC"));

        // Should have proper structure
        assert!(result.contains("--- Previous Comments ---"));
        assert!(result.contains("--- End of Comments ---"));
    }

    #[test]
    fn test_format_comments_for_context_empty() {
        let notes = vec![];
        let result = format_comments_for_context(&notes, 1000, 1);
        assert_eq!(result, "No previous comments found.");
    }

    #[test]
    fn test_format_comments_for_context_only_current() {
        let notes = vec![GitlabNoteAttributes {
            id: 5,
            note: "This is the only comment and it's the current one".to_string(),
            author: GitlabUser {
                id: 123,
                username: "user1".to_string(),
                name: "User One".to_string(),
                avatar_url: None,
            },
            project_id: 1,
            noteable_type: "Issue".to_string(),
            noteable_id: Some(10),
            iid: Some(10),
            url: None,
            updated_at: "2023-01-01T12:00:00Z".to_string(),
        }];

        let result = format_comments_for_context(&notes, 1000, 5);
        assert_eq!(result, "No previous comments found.");
    }

    #[test]
    fn test_parse_slash_command() {
        // Test valid slash commands
        assert_eq!(
            parse_slash_command("/summarize"),
            Some((SlashCommand::Summarize, None))
        );

        assert_eq!(
            parse_slash_command("/postmortem"),
            Some((SlashCommand::Postmortem, None))
        );

        assert_eq!(
            parse_slash_command("/help"),
            Some((SlashCommand::Help, None))
        );

        // Test slash commands with additional context
        assert_eq!(
            parse_slash_command("/summarize please focus on security"),
            Some((
                SlashCommand::Summarize,
                Some("please focus on security".to_string())
            ))
        );

        assert_eq!(
            parse_slash_command("/postmortem with timeline details"),
            Some((
                SlashCommand::Postmortem,
                Some("with timeline details".to_string())
            ))
        );

        // Test case insensitive
        assert_eq!(
            parse_slash_command("/HELP"),
            Some((SlashCommand::Help, None))
        );

        assert_eq!(
            parse_slash_command("/Summarize Please"),
            Some((SlashCommand::Summarize, Some("Please".to_string())))
        );

        // Test invalid slash commands
        assert_eq!(parse_slash_command("/invalid"), None);
        assert_eq!(parse_slash_command("not a slash command"), None);
        assert_eq!(parse_slash_command(""), None);
        assert_eq!(parse_slash_command("regular text"), None);

        // Test edge cases
        assert_eq!(parse_slash_command("/"), None);
        assert_eq!(parse_slash_command("/ summarize"), None); // space after slash
        assert_eq!(
            parse_slash_command("/summarize   "), // trailing spaces
            Some((SlashCommand::Summarize, None))
        );
    }

    #[test]
    fn test_slash_command_get_precanned_prompt() {
        assert!(SlashCommand::Summarize
            .get_precanned_prompt()
            .contains("Summarize changes"));
        assert!(SlashCommand::Postmortem
            .get_precanned_prompt()
            .contains("postmortem"));
        assert!(SlashCommand::Help
            .get_precanned_prompt()
            .contains("slash commands"));
    }

    #[test]
    fn test_generate_help_message() {
        let help_msg = generate_help_message();
        assert!(help_msg.contains("/summarize"));
        assert!(help_msg.contains("/postmortem"));
        assert!(help_msg.contains("/help"));
        assert!(help_msg.contains("additional context"));
    }

    #[tokio::test]
    async fn test_slash_command_integration_summarize() {
        let mut server = mockito::Server::new_async().await;
        let config = test_app_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let cache = MentionCache::new();

        let event_time = Utc::now();
        let event = create_test_note_event_with_id(
            TEST_USER_USERNAME,
            "Issue",
            TEST_MENTION_ID,
            Some(format!(
                "@{} /summarize please focus on security",
                TEST_BOT_USERNAME
            )),
            Some(event_time.to_rfc3339()),
        );

        // Mock Gitlab: get_issue_notes (for de-duplication check) - return empty
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

        // Mock Gitlab: get_issue
        let mock_issue = GitlabIssue {
            id: 1,
            iid: TEST_ISSUE_IID,
            project_id: TEST_PROJECT_ID,
            title: "Test Security Issue".to_string(),
            description: Some("Security issue description here.".to_string()),
            state: "opened".to_string(),
            author: GitlabUser {
                id: TEST_GENERIC_USER_ID + 1,
                username: "issue_author".to_string(),
                name: "Issue Author".to_string(),
                avatar_url: None,
            },
            labels: vec!["security".to_string()],
            web_url: "url".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
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

        // Mock OpenAI - verify it gets called successfully
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
                    "id": "chatcmpl-slash-test",
                    "object": "chat.completion",
                    "created": 1677652288,
                    "model": config.openai_model.clone(),
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Security analysis summary with guidelines adherence."
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

        // Mock Gitlab: post_comment_to_issue
        let _m_post_comment = server
            .mock(
                "POST",
                format!(
                    "/api/v4/projects/{}/issues/{}/notes",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )
                .as_str(),
            )
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(json!({
                "id": 999,
                "note": "Posted comment",
                "author": {
                    "id": TEST_BOT_USER_ID,
                    "username": config.bot_username.clone(),
                    "name": format!("{} Bot", config.bot_username),
                    "avatar_url": null,
                    "state": "active",
                    "web_url": format!("https://gitlab.example.com/{}", config.bot_username)
                },
                "project_id": TEST_PROJECT_ID,
                "noteable_type": "Issue",
                "noteable_id": event.issue.as_ref().unwrap().id,
                "iid": event.issue.as_ref().unwrap().iid,
                "created_at": Utc::now().to_rfc3339(),
                "updated_at": Utc::now().to_rfc3339(),
                "system": false,
                "url": format!("https://gitlab.example.com/org/repo1/-/issues/{}/notes/999", event.issue.as_ref().unwrap().iid)
            }).to_string())
            .create_async()
            .await;

        // Other required mocks
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
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let openai_client = Arc::new(OpenAIApiClient::new(&config).unwrap());

        let result = process_mention(
            event,
            gitlab_client.clone(),
            openai_client,
            config.clone(),
            &cache,
            file_index_manager,
        )
        .await;

        assert!(result.is_ok(), "Processing failed: {:?}", result.err());
        assert!(cache.check(TEST_MENTION_ID).await);
    }

    #[tokio::test]
    async fn test_slash_command_help_integration() {
        let mut server = mockito::Server::new_async().await;
        let config = test_app_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let cache = MentionCache::new();

        let event_time = Utc::now();
        let event = create_test_note_event_with_id(
            TEST_USER_USERNAME,
            "Issue",
            TEST_MENTION_ID,
            Some(format!("@{} /help", TEST_BOT_USERNAME)),
            Some(event_time.to_rfc3339()),
        );

        // Mock Gitlab: get_issue_notes (for de-duplication check) - return empty
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

        // Mock Gitlab: get_issue - needed even for help command
        let mock_issue = GitlabIssue {
            id: 1,
            iid: TEST_ISSUE_IID,
            project_id: TEST_PROJECT_ID,
            title: "Test Help Issue".to_string(),
            description: Some("Issue for testing help command.".to_string()),
            state: "opened".to_string(),
            author: GitlabUser {
                id: TEST_GENERIC_USER_ID + 1,
                username: "issue_author".to_string(),
                name: "Issue Author".to_string(),
                avatar_url: None,
            },
            labels: vec![],
            web_url: "url".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
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

        // Mock OpenAI - help should go through LLM with help message content
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
                    "id": "chatcmpl-help-test",
                    "object": "chat.completion",
                    "created": 1677652288,
                    "model": config.openai_model.clone(),
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Here are the available commands..."
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

        // Mock Gitlab: post_comment_to_issue
        let _m_post_comment = server
            .mock(
                "POST",
                format!(
                    "/api/v4/projects/{}/issues/{}/notes",
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )
                .as_str(),
            )
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(json!({
                "id": 999,
                "note": "Posted comment",
                "author": {
                    "id": TEST_BOT_USER_ID,
                    "username": config.bot_username.clone(),
                    "name": format!("{} Bot", config.bot_username),
                    "avatar_url": null,
                    "state": "active",
                    "web_url": format!("https://gitlab.example.com/{}", config.bot_username)
                },
                "project_id": TEST_PROJECT_ID,
                "noteable_type": "Issue",
                "noteable_id": event.issue.as_ref().unwrap().id,
                "iid": event.issue.as_ref().unwrap().iid,
                "created_at": Utc::now().to_rfc3339(),
                "updated_at": Utc::now().to_rfc3339(),
                "system": false,
                "url": format!("https://gitlab.example.com/org/repo1/-/issues/{}/notes/999", event.issue.as_ref().unwrap().iid)
            }).to_string())
            .create_async()
            .await;

        // Other required mocks
        let _m_get_any_file = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/.*/repository/files/.*".to_string()),
            )
            .with_status(404)
            .create_async()
            .await;

        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let openai_client = Arc::new(OpenAIApiClient::new(&config).unwrap());

        let result = process_mention(
            event,
            gitlab_client.clone(),
            openai_client,
            config.clone(),
            &cache,
            file_index_manager,
        )
        .await;

        assert!(result.is_ok(), "Processing failed: {:?}", result.err());
        assert!(cache.check(TEST_MENTION_ID).await);
    }

    #[tokio::test]
    async fn test_unknown_slash_command_invokes_help_issue() {
        let mut server = mockito::Server::new_async().await;
        let config = test_app_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let event_time = Utc::now();
        let event = create_test_note_event_with_id(
            TEST_USER_USERNAME,
            "Issue",
            TEST_MENTION_ID,
            Some(format!("@{} /unknowncommand", TEST_BOT_USERNAME)),
            Some(event_time.to_rfc3339()),
        );

        let mock_issue_details = GitlabIssue {
            id: 1,
            iid: TEST_ISSUE_IID,
            project_id: TEST_PROJECT_ID,
            title: "Test Issue for Unknown Command".to_string(),
            description: Some("Description".to_string()),
            state: "opened".to_string(),
            author: GitlabUser {
                id: TEST_GENERIC_USER_ID,
                username: "issue_creator".to_string(),
                name: "Issue Creator".to_string(),
                avatar_url: None,
            },
            labels: vec![],
            web_url: "url".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: event_time.to_rfc3339(),
        };

        // Mock get_issue
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
            .with_body(json!(mock_issue_details).to_string())
            .create_async()
            .await;

        // Mock get_all_issue_notes (for comment context) - return empty
        let _m_get_all_notes = server
            .mock(
                "GET",
                Matcher::Regex(format!(
                    r"/api/v4/projects/{}/issues/{}/notes", // No query params for "all notes"
                    TEST_PROJECT_ID, TEST_ISSUE_IID
                )),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        // Mock for RepoContextExtractor (list_repository_tree, get_file_content)
        // These might be called by add_repository_context_to_prompt
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

        let _m_get_any_file_repo_context = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/.*/repository/files/.*".to_string()),
            )
            .with_status(404) // Assume no files found for simplicity
            .create_async()
            .await;

        let issue_prompt_context = IssuePromptContext {
            event: &event,
            gitlab_client: &gitlab_client,
            config: &config,
            project_id: TEST_PROJECT_ID,
            issue_iid: TEST_ISSUE_IID,
            issue: &mock_issue_details,
            file_index_manager: &file_index_manager,
        };

        let mut prompt_parts = Vec::new();
        let user_context = "/unknowncommand"; // The unknown command itself

        build_issue_prompt_with_context(issue_prompt_context, user_context, &mut prompt_parts)
            .await
            .unwrap();

        assert!(
            prompt_parts.contains(&SlashCommand::Help.get_precanned_prompt().to_string()),
            "Prompt parts should contain the help precanned prompt for an unknown issue command."
        );
        assert!(
            prompt_parts.contains(&generate_help_message()),
            "Prompt parts should contain the full help message for an unknown issue command."
        );
    }

    #[tokio::test]
    async fn test_unknown_slash_command_invokes_help_mr() {
        let mut server = mockito::Server::new_async().await;
        let config = test_app_settings(server.url());
        let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());
        let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

        let event_time = Utc::now();
        let test_mr_iid = 303;
        // Create a new event specifically for MR
        let mut event = create_test_note_event_with_id(
            TEST_USER_USERNAME,
            "MergeRequest", // Important: Set to MergeRequest
            TEST_MENTION_ID,
            Some(format!("@{} /anotherunknown", TEST_BOT_USERNAME)),
            Some(event_time.to_rfc3339()),
        );
        // Ensure merge_request field is populated correctly for MR
        event.merge_request = Some(GitlabNoteObject {
            id: 2, // Different from issue's noteable_id if necessary
            iid: test_mr_iid,
            title: "Test MR for Unknown Command".to_string(),
            description: Some("Description for MR".to_string()),
        });
        event.object_attributes.iid = Some(test_mr_iid);

        let mock_mr_details = crate::models::GitlabMergeRequest {
            id: 2,
            iid: test_mr_iid,
            project_id: TEST_PROJECT_ID,
            title: "Test MR for Unknown Command".to_string(),
            description: Some("Description for MR".to_string()),
            state: "opened".to_string(),
            author: GitlabUser {
                id: TEST_GENERIC_USER_ID,
                username: "mr_creator".to_string(),
                name: "MR Creator".to_string(),
                avatar_url: None,
            },
            labels: vec![],
            source_branch: "feature-branch".to_string(),
            target_branch: "main".to_string(),
            web_url: "url_mr".to_string(),
            updated_at: event_time.to_rfc3339(),
            // DiffRefs, commits_count, and changes_count removed as they are not in the current GitlabMergeRequest model
            detailed_merge_status: Some("mergeable".to_string()), // Added an example of a current field
            head_pipeline: None, // Added an example of a current field
        };

        // Mock get_merge_request
        let _m_get_mr = server
            .mock(
                "GET",
                format!(
                    "/api/v4/projects/{}/merge_requests/{}",
                    TEST_PROJECT_ID, test_mr_iid
                )
                .as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!(mock_mr_details).to_string())
            .create_async()
            .await;

        // Mock get_all_merge_request_notes (for comment context) - return empty
        let _m_get_all_mr_notes = server
            .mock(
                "GET",
                Matcher::Regex(format!(
                    r"/api/v4/projects/{}/merge_requests/{}/notes", // No query params
                    TEST_PROJECT_ID, test_mr_iid
                )),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([]).to_string())
            .create_async()
            .await;

        // Mock for RepoContextExtractor (list_repository_tree, get_file_content for diffs)
        let _m_list_tree_mr = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/.*/repository/tree.*".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([]).to_string()) // Empty tree
            .create_async()
            .await;

        // This mock is for files within extract_context_for_mr, e.g., diff files.
        // It's distinct from CONTRIBUTING.md.
        let _m_get_any_file_mr_context = server
            .mock(
                "GET",
                Matcher::Regex(r"/api/v4/projects/.*/repository/files/.*".to_string()),
            )
            .with_status(404) // Assume no specific files found for simplicity
            .create_async()
            .await;

        // Mock get_merge_request_changes (called by extract_context_for_mr)
        let _m_get_mr_changes = server
            .mock(
                "GET",
                Matcher::Regex(format!(
                    "/api/v4/projects/{}/merge_requests/{}/changes",
                    TEST_PROJECT_ID, test_mr_iid
                )),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "changes": [] // No changes for simplicity
                })
                .to_string(),
            ) // Added .to_string() to satisfy AsRef<[u8]>
            .create_async()
            .await;

        let mr_prompt_context = MrPromptContext {
            event: &event,
            gitlab_client: &gitlab_client,
            config: &config,
            mr: &mock_mr_details,
            file_index_manager: &file_index_manager,
        };

        let mut prompt_parts = Vec::new();
        let mut commit_history = String::new(); // Required for build_mr_prompt_with_context
        let user_context = "/anotherunknown"; // The unknown command

        build_mr_prompt_with_context(
            mr_prompt_context,
            user_context,
            &mut prompt_parts,
            &mut commit_history,
        )
        .await; // This function doesn't return a Result in the current code

        assert!(
            prompt_parts.contains(&SlashCommand::Help.get_precanned_prompt().to_string()),
            "Prompt parts should contain the help precanned prompt for an unknown MR command."
        );
        assert!(
            prompt_parts.contains(&generate_help_message()),
            "Prompt parts should contain the full help message for an unknown MR command."
        );
    }
}
