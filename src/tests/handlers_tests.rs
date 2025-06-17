#[cfg(test)]
mod tests {
    use crate::config::AppSettings;
    use crate::file_indexer::FileIndexManager;
    use crate::gitlab::GitlabApiClient;
    use crate::handlers::*;
    // Removed: use crate::mention_cache::MentionCache;
    use crate::models::{
        GitlabIssue, GitlabNoteAttributes, GitlabNoteEvent, GitlabNoteObject, GitlabProject,
        GitlabUser,
    };
    use chrono::Utc;
    use mockito::Matcher;
    use serde_json::json;
    use std::sync::Arc;

    pub(crate) const TEST_MENTION_ID: i64 = 12345;
    pub(crate) const TEST_PROJECT_ID: i64 = 1;
    pub(crate) const TEST_ISSUE_IID: i64 = 101;
    pub(crate) const TEST_BOT_USERNAME: &str = "test_bot";
    pub(crate) const TEST_USER_USERNAME: &str = "test_user";
    pub(crate) const TEST_GENERIC_USER_ID: i64 = 2;
    pub(crate) const TEST_BOT_USER_ID: i64 = 99;

    pub(crate) fn test_app_settings(base_url: String) -> Arc<AppSettings> {
        Arc::new(AppSettings {
            gitlab_url: base_url.clone(),
            gitlab_token: "test_token".to_string(),
            openai_api_key: "test_openai_key".to_string(),
            openai_custom_url: base_url,
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
            max_comment_length: 1000,
            context_lines: 10,
            stale_issue_days: 30,
            max_age_hours: 24,
            context_repo_path: None,
            max_context_size: 60000,
        })
    }

    // Removed unused function: pub(crate) fn create_test_note_event(...)

    pub(crate) fn create_test_note_event_with_id(
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
            noteable_id: Some(1),
            iid: Some(if noteable_type == "Issue" {
                TEST_ISSUE_IID
            } else {
                202
            }),
            url: Some(format!(
                "https://gitlab.example.com/org/repo1/-/issues/{}#note_{}",
                TEST_ISSUE_IID, mention_id
            )),
            updated_at: updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()),
        };

        let issue = if noteable_type == "Issue" {
            Some(GitlabNoteObject {
                id: 1,
                iid: TEST_ISSUE_IID,
                title: "Test Issue".to_string(),
                description: Some("This is a test issue".to_string()),
            })
        } else {
            None
        };

        let merge_request = if noteable_type == "MergeRequest" {
            Some(GitlabNoteObject {
                id: 1,
                iid: 202,
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
        let note1 = "Hello @mybot please summarize this";
        assert_eq!(
            extract_context_after_mention(note1, bot_name),
            Some("please summarize this".to_string())
        );
        let note2 = "@mybot  summarize this for me  ";
        assert_eq!(
            extract_context_after_mention(note2, bot_name),
            Some("summarize this for me".to_string())
        );
        let note3 = "Thanks @mybot";
        assert_eq!(extract_context_after_mention(note3, bot_name), None);
    }

    // ... (The rest of the tests from the `mod tests` block, like test_process_mention_no_bot_mention, etc.) ...
    // Omitting for brevity, but they would be here in the actual overwrite.

    #[cfg(test)]
    mod issue_deduplication_tests {
        use super::*;
        use crate::gitlab::GitlabError;
        use tracing_test::traced_test;

        fn create_dedup_test_gitlab_issue(
            id: i64,
            iid: i64,
            title: &str,
            description: &str,
        ) -> GitlabIssue {
            GitlabIssue {
                id,
                iid,
                project_id: TEST_PROJECT_ID,
                title: title.to_string(),
                description: Some(description.to_string()),
                state: "opened".to_string(),
                author: GitlabUser {
                    id: TEST_GENERIC_USER_ID + iid,
                    username: format!("author_{}", iid),
                    name: format!("Author {}", iid),
                    avatar_url: None,
                },
                labels: vec![],
                web_url: format!(
                    "http://gitlab.example.com/test_org/test_repo/issues/{}",
                    iid
                ),
                updated_at: Utc::now().to_rfc3339(),
            }
        }

        async fn run_handle_issue_mention_for_dedup_test(
            server: &mut mockito::ServerGuard,
            config: Arc<AppSettings>,
            gitlab_client: Arc<GitlabApiClient>,
            event: GitlabNoteEvent,
            mock_open_issues: Result<Vec<GitlabIssue>, GitlabError>,
            explicit_current_issue_for_mocking: Option<GitlabIssue>,
        ) -> Vec<String> {
            let project_id = event.project.id;
            let current_issue_event_iid = event.issue.as_ref().unwrap().iid;
            let current_issue_event_id = event.issue.as_ref().unwrap().id;

            let (mock_issue_id, mock_issue_iid, mock_issue_title, mock_issue_description) =
                if let Some(ref explicit_issue) = explicit_current_issue_for_mocking {
                    (
                        explicit_issue.id,
                        explicit_issue.iid,
                        explicit_issue.title.clone(),
                        explicit_issue.description.clone().unwrap_or_default(),
                    )
                } else {
                    (
                        current_issue_event_id,
                        current_issue_event_iid,
                        event.issue.as_ref().unwrap().title.clone(),
                        event
                            .issue
                            .as_ref()
                            .unwrap()
                            .description
                            .clone()
                            .unwrap_or_default(),
                    )
                };

            let _m_get_current_issue = server
                .mock(
                    "GET",
                    format!(
                        "/api/v4/projects/{}/issues/{}",
                        project_id, current_issue_event_iid
                    )
                    .as_str(),
                )
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(
                    json!(create_dedup_test_gitlab_issue(
                        mock_issue_id,
                        mock_issue_iid,
                        &mock_issue_title,
                        &mock_issue_description
                    ))
                    .to_string(),
                )
                .create_async()
                .await;

            let _m_remove_label = server.mock("PUT", Matcher::Any)
                .with_status(200)
                .with_body(json!({"id": current_issue_event_id, "iid": current_issue_event_iid, "title": "Test Issue", "labels": []}).to_string())
                .create_async().await;

            match mock_open_issues {
                Ok(issues) => {
                    server
                        .mock(
                            "GET",
                            format!("/api/v4/projects/{}/issues", project_id).as_str(),
                        )
                        .match_query(Matcher::AllOf(vec![
                            Matcher::UrlEncoded("state".into(), "opened".into()),
                            Matcher::UrlEncoded("per_page".into(), "100".into()),
                        ]))
                        .with_status(200)
                        .with_header("content-type", "application/json")
                        .with_body(json!(issues).to_string())
                        .create_async()
                        .await;
                }
                Err(GitlabError::Api { status, body }) => {
                    server
                        .mock(
                            "GET",
                            format!("/api/v4/projects/{}/issues", project_id).as_str(),
                        )
                        .match_query(Matcher::AllOf(vec![
                            Matcher::UrlEncoded("state".into(), "opened".into()),
                            Matcher::UrlEncoded("per_page".into(), "100".into()),
                        ]))
                        .with_status(status.as_u16() as usize)
                        .with_header("content-type", "application/json")
                        .with_body(body)
                        .create_async()
                        .await;
                }
                _ => panic!("Unsupported error type for mocking get_all_open_issues"),
            }

            server
                .mock(
                    "GET",
                    format!(
                        "/api/v4/projects/{}/issues/{}/notes",
                        project_id, current_issue_event_iid
                    )
                    .as_str(),
                )
                .match_query(Matcher::AllOf(vec![
                    Matcher::UrlEncoded("sort".into(), "asc".into()),
                    Matcher::UrlEncoded("per_page".into(), "100".into()),
                ]))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(json!([]).to_string())
                .create_async()
                .await;

            server
                .mock(
                    "GET",
                    Matcher::Regex(r"/api/v4/projects/.*/issues/.*/notes.*".to_string()),
                )
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(json!([]).to_string())
                .create_async()
                .await;

            server
                .mock(
                    "GET",
                    Matcher::Regex(r"/api/v4/projects/.*/repository/tree.*".to_string()),
                )
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(json!([]).to_string())
                .create_async()
                .await;

            server
                .mock(
                    "GET",
                    Matcher::Regex(r"/api/v4/projects/.*/search.*".to_string()),
                )
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(json!([]).to_string())
                .create_async()
                .await;

            server
                .mock(
                    "GET",
                    Matcher::Regex(r"/api/v4/projects/.*/repository/files/.*".to_string()),
                )
                .with_status(404)
                .create_async()
                .await;

            let mut prompt_parts = Vec::new();
            let file_index_manager = Arc::new(FileIndexManager::new(gitlab_client.clone(), 3600));

            let result = handle_issue_mention(
                &event,
                &gitlab_client,
                &config,
                project_id,
                &mut prompt_parts,
                &extract_context_after_mention(&event.object_attributes.note, &config.bot_username),
                &file_index_manager,
            )
            .await;

            if result.is_err() {
                eprintln!("handle_issue_mention failed: {:?}", result.as_ref().err());
            }

            prompt_parts
        }

        #[tokio::test]
        #[traced_test]
        async fn test_deduplication_no_similar_issues() {
            let mut server = mockito::Server::new_async().await;
            let config = test_app_settings(server.url());
            let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

            let current_event_note = format!("@{} summarize this issue", TEST_BOT_USERNAME);
            let mut event = create_test_note_event_with_id(
                TEST_USER_USERNAME,
                "Issue",
                TEST_MENTION_ID,
                Some(current_event_note),
                None,
            );
            if let Some(issue_obj) = event.issue.as_mut() {
                issue_obj.title = "Unique issue title for testing".to_string();
                issue_obj.description = Some("A very specific description that is unlikely to match others due to unique phrasing and keywords like ZYXWVU.".to_string());
            }

            let open_issues = Ok(vec![
                create_dedup_test_gitlab_issue(
                    2,
                    202,
                    "Completely Different Title",
                    "Very different description text here with no common trigrams like QWERTY.",
                ),
                create_dedup_test_gitlab_issue(
                    3,
                    303,
                    "Another One Entirely",
                    "Also not related at all to the current issue content, for example ABCDEFG.",
                ),
            ]);

            let prompt_parts = run_handle_issue_mention_for_dedup_test(
                &mut server,
                config,
                gitlab_client,
                event,
                open_issues,
                None,
            )
            .await;

            assert!(!logs_contain(
                "Failed to fetch open issues for n-gram deduplication"
            ));
            let similar_issues_section = prompt_parts
                .iter()
                .find(|s| s.contains("--- Potentially Similar Issues (N-gram based) ---"));
            assert!(
                similar_issues_section.is_none(),
                "Prompt should not contain similar issues section. Got: {:?}",
                prompt_parts
            );
        }

        #[tokio::test]
        #[traced_test]
        async fn test_deduplication_similar_issues_found() {
            let mut server = mockito::Server::new_async().await;
            let config = test_app_settings(server.url());
            let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

            let mut event = create_test_note_event_with_id(
                TEST_USER_USERNAME,
                "Issue",
                TEST_MENTION_ID,
                Some(format!(
                    "@{} please investigate this login problem",
                    TEST_BOT_USERNAME
                )),
                None,
            );
            if let Some(issue_obj) = event.issue.as_mut() {
                issue_obj.title = "Login button not working on main page".to_string();
                issue_obj.description = Some("The login button on the main page is unresponsive after the recent update. Users cannot access their accounts using this button.".to_string());
            }

            let open_issues = Ok(vec![
            create_dedup_test_gitlab_issue(2, 202, "Cannot login to system - main page button", "Users reporting main login button is broken and does not work. This is a critical issue preventing account access."),
            create_dedup_test_gitlab_issue(3, 303, "Profile page error after login", "After successful login, the profile page shows an error for some users trying to update their information. Button clicks seem fine."),
            create_dedup_test_gitlab_issue(4, 404, "Dashboard slow loading after deployment", "The main dashboard is very slow after the new deployment. It takes minutes to load charts and data."),
        ]);

            let prompt_parts = run_handle_issue_mention_for_dedup_test(
                &mut server,
                config,
                gitlab_client,
                event,
                open_issues,
                None,
            )
            .await;

            assert!(!logs_contain(
                "Failed to fetch open issues for n-gram deduplication"
            ));
            let similar_issues_section = prompt_parts
                .iter()
                .find(|s| s.contains("--- Potentially Similar Issues (N-gram based) ---"));
            assert!(
                similar_issues_section.is_some(),
                "Prompt should contain n-gram similar issues section. Got: {:?}",
                prompt_parts
            );

            let section_content = similar_issues_section.unwrap();
            assert!(section_content.contains("Cannot login to system - main page button"));
            assert!(section_content.contains("IID: 202"));
            assert!(section_content.contains("Similarity Score:"));

            if section_content.contains("Profile page error after login") {
                assert!(section_content.contains("IID: 303"));
                assert!(section_content.contains("Similarity Score:"));
            }
            assert!(!section_content.contains("Dashboard slow loading after deployment"));
        }

        #[tokio::test]
        #[traced_test]
        async fn test_deduplication_thresholding() {
            let mut server = mockito::Server::new_async().await;
            let config = test_app_settings(server.url());
            let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

            let mut event = create_test_note_event_with_id(
                TEST_USER_USERNAME,
                "Issue",
                TEST_MENTION_ID,
                Some(format!(
                    "@{} look into this UI glitch on the settings page",
                    TEST_BOT_USERNAME
                )),
                None,
            );
            if let Some(issue_obj) = event.issue.as_mut() {
                issue_obj.title = "UI glitch on settings page".to_string();
                issue_obj.description = Some("The save button is misaligned on the user settings screen after changing display resolution. It's hard to click this button.".to_string());
            }

            let open_issues = Ok(vec![
            create_dedup_test_gitlab_issue(2, 202, "Button issue on settings page", "Users report save button alignment problem on settings page. This makes saving changes difficult for them."),
            create_dedup_test_gitlab_issue(3, 303, "Visual bug on main dashboard", "The main company logo seems a bit off-center after the latest css stylesheet updates were pushed from marketing."),
            create_dedup_test_gitlab_issue(4, 404, "Settings page save button broken", "The button to save settings is not working at all. Clicking it does nothing, please fix this save button."),
        ]);

            let prompt_parts = run_handle_issue_mention_for_dedup_test(
                &mut server,
                config,
                gitlab_client,
                event,
                open_issues,
                None,
            )
            .await;

            assert!(!logs_contain(
                "Failed to fetch open issues for n-gram deduplication"
            ));
            let similar_issues_section_opt = prompt_parts
                .iter()
                .find(|s| s.contains("--- Potentially Similar Issues (N-gram based) ---"));

            assert!(similar_issues_section_opt.is_some(), "Deduplication section should be present if issues are above threshold. Actual prompt parts: {:?}", prompt_parts);
            if let Some(section_content) = similar_issues_section_opt {
                assert!(
                    section_content.contains("Button issue on settings page")
                        && section_content.contains("IID: 202")
                        && section_content.contains("Similarity Score:")
                );
                assert!(
                    section_content.contains("Settings page save button broken")
                        && section_content.contains("IID: 404")
                        && section_content.contains("Similarity Score:")
                );
                assert!(!section_content.contains("Visual bug on main dashboard"), "Issue with expected score below threshold should not be included. Actual content: {}", section_content);
            }
        }

        #[tokio::test]
        #[traced_test]
        async fn test_deduplication_filters_current_issue() {
            let mut server = mockito::Server::new_async().await;
            let config = test_app_settings(server.url());
            let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

            let current_event_note = format!("@{} summarize current Test Issue", TEST_BOT_USERNAME);
            let event = create_test_note_event_with_id(
                TEST_USER_USERNAME,
                "Issue",
                TEST_MENTION_ID,
                Some(current_event_note),
                None,
            );

            let open_issues = Ok(vec![
                create_dedup_test_gitlab_issue(
                    event.issue.as_ref().unwrap().id,
                    event.issue.as_ref().unwrap().iid,
                    &event.issue.as_ref().unwrap().title,
                    "This is the current issue's description.",
                ),
                create_dedup_test_gitlab_issue(
                    2,
                    202,
                    "Another Test Issue",
                    "This is a different issue but title is similar to 'current Test Issue'.",
                ),
            ]);

            let prompt_parts = run_handle_issue_mention_for_dedup_test(
                &mut server,
                config,
                gitlab_client,
                event,
                open_issues,
                None,
            )
            .await;

            assert!(!logs_contain(
                "Failed to fetch open issues for n-gram deduplication"
            ));
            let similar_issues_section_opt = prompt_parts
                .iter()
                .find(|s| s.contains("--- Potentially Similar Issues (N-gram based) ---"));

            if let Some(similar_issues_section) = similar_issues_section_opt {
                assert!(
                    !similar_issues_section.contains(&format!("IID: {}", TEST_ISSUE_IID)),
                    "Current issue (IID {}) should not be in duplicates. Section: {}",
                    TEST_ISSUE_IID,
                    similar_issues_section
                );
                assert!(similar_issues_section.contains("Another Test Issue"));
                assert!(similar_issues_section.contains("IID: 202"));
            }
        }

        #[tokio::test]
        #[traced_test]
        async fn test_deduplication_limit() {
            let mut server = mockito::Server::new_async().await;
            let config = test_app_settings(server.url());
            let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

            let current_event_note = format!("@{} summarize this issue", TEST_BOT_USERNAME);
            let mut event = create_test_note_event_with_id(
                TEST_USER_USERNAME,
                "Issue",
                TEST_MENTION_ID,
                Some(current_event_note),
                None,
            );

            let current_issue_for_mocking = create_dedup_test_gitlab_issue(
            event.issue.as_ref().unwrap().id,
            event.issue.as_ref().unwrap().iid,
            "Login button broken on main page",
            "Users are reporting that the main login button is not working after the recent deployment. This is a critical issue."
        );

            if let Some(issue_obj) = event.issue.as_mut() {
                issue_obj.title = current_issue_for_mocking.title.clone();
                issue_obj.description = current_issue_for_mocking.description.clone();
            }

            let mut issues_to_mock = Vec::new();
            for i in 1..=7 {
                issues_to_mock.push(create_dedup_test_gitlab_issue(i + 10, i + 200, &format!("Login button broken on main page version {}", i), "Users are reporting that the main login button is not working after the recent deployment. This is a critical issue. Please investigate this login button problem."));
            }
            let open_issues = Ok(issues_to_mock);

            let prompt_parts = run_handle_issue_mention_for_dedup_test(
                &mut server,
                config,
                gitlab_client,
                event,
                open_issues,
                Some(current_issue_for_mocking),
            )
            .await;

            assert!(!logs_contain(
                "Failed to fetch open issues for n-gram deduplication"
            ));
            let similar_issues_section = prompt_parts
                .iter()
                .find(|s| s.contains("--- Potentially Similar Issues (N-gram based) ---"));
            assert!(
                similar_issues_section.is_some(),
                "Prompt should contain similar issues section. Actual prompt parts: {:?}",
                prompt_parts
            );

            let count = similar_issues_section.unwrap().matches("IID:").count();
            assert_eq!(count, 5, "Should only list up to 5 similar issues");
        }

        #[tokio::test]
        #[traced_test]
        async fn test_deduplication_api_error_fetching_open_issues() {
            let mut server = mockito::Server::new_async().await;
            let config = test_app_settings(server.url());
            let gitlab_client = Arc::new(GitlabApiClient::new(config.clone()).unwrap());

            let current_event_note = format!("@{} summarize this issue", TEST_BOT_USERNAME);
            let event = create_test_note_event_with_id(
                TEST_USER_USERNAME,
                "Issue",
                TEST_MENTION_ID,
                Some(current_event_note),
                None,
            );

            let open_issues_error = Err(GitlabError::Api {
                status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                body: "GitLab server error".to_string(),
            });

            let prompt_parts = run_handle_issue_mention_for_dedup_test(
                &mut server,
                config,
                gitlab_client,
                event,
                open_issues_error,
                None,
            )
            .await;

            assert!(logs_contain("Failed to fetch open issues for n-gram deduplication: API error:"), "Warning log for API error should be present. Actual logs may contain more specific context from the tracing library.");
            let similar_issues_section = prompt_parts
                .iter()
                .find(|s| s.contains("--- Potentially Similar Issues (N-gram based) ---"));
            assert!(
                similar_issues_section.is_none(),
                "Prompt should not contain similar issues section on API error"
            );
        }
    }
} // This closes the main `mod tests`
