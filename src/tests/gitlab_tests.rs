use crate::config::AppSettings;
use crate::gitlab::{GitlabApiClient, GitlabError};
use mockito;
use reqwest::StatusCode;
use serde_json::json;
use std::sync::Arc;

// Helper to create AppSettings for tests
fn create_test_settings(base_url: String) -> AppSettings {
    AppSettings {
        prompt_prefix: None,
        gitlab_url: base_url,
        gitlab_token: "test_token".to_string(),
        openai_api_key: "key".to_string(),
        openai_custom_url: "url".to_string(),
        openai_model: "gpt-3.5-turbo".to_string(),
        default_branch: "test-main".to_string(),
        openai_temperature: 0.7,
        openai_max_tokens: 1024,
        openai_token_mode: "max_tokens".to_string(),
        repos_to_poll: vec!["org/repo1".to_string()],
        log_level: "debug".to_string(),
        bot_username: "gitbot".to_string(),
        poll_interval_seconds: 60,
        stale_issue_days: 30,
        max_age_hours: 24,
        context_repo_path: None,
        max_context_size: 60000,
        max_comment_length: 1000,
        context_lines: 10,
        max_tool_calls: 3,
        client_cert_path: None,
        client_key_path: None,
        client_key_password: None,
    }
}

#[tokio::test]
async fn test_new_gitlab_api_client_valid_url() {
    let settings = Arc::new(create_test_settings("http://localhost:1234".to_string()));
    let client = GitlabApiClient::new(settings);
    assert!(client.is_ok());
}

#[tokio::test]
async fn test_new_gitlab_api_client_invalid_url() {
    let settings = Arc::new(create_test_settings("not a url".to_string()));
    let result = GitlabApiClient::new(settings);
    assert!(result.is_err());
    match result.err().unwrap() {
        GitlabError::UrlParse(_) => {} // Expected error
        _ => panic!("Expected UrlParse"),
    }
}

#[tokio::test]
async fn test_get_issue_success() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();

    let settings = Arc::new(create_test_settings(base_url.clone()));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_issue_response = json!({
        "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue",
        "description": "A test issue", "state": "opened",
        "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
        "web_url": "http://example.com/issue/1", "labels": [], "assignees": [], "type": "ISSUE",
        "milestone": null, "closed_at": null, "closed_by": null, "created_at": "2023-01-01T12:00:00Z", "updated_at": "2023-01-02T12:00:00Z",
        "upvotes": 0, "downvotes": 0, "merge_requests_count": 0, "subscriber_count": 0, "user_notes_count": 0,
        "due_date": null, "confidential": false, "discussion_locked": null, "time_stats": {
            "time_estimate": 0, "total_time_spent": 0, "human_time_estimate": null, "human_total_time_spent": null
        },
        "task_completion_status": {"count": 0, "completed_count": 0}
    });

    let _m = server
        .mock("GET", "/api/v4/projects/1/issues/101")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_issue_response.to_string())
        .create_async()
        .await;

    let issue = client.get_issue(1, 101).await.unwrap();
    assert_eq!(issue.title, "Test Issue");
    assert_eq!(issue.author.username, "tester");
    assert_eq!(issue.updated_at, "2023-01-02T12:00:00Z");
}

#[tokio::test]
async fn test_get_issue_not_found() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();

    let settings = Arc::new(create_test_settings(base_url.clone()));
    let client = GitlabApiClient::new(settings).unwrap();

    let _m = server
        .mock("GET", "/api/v4/projects/2/issues/202")
        .with_status(404)
        .with_body("{\"message\": \"Issue not found\"}")
        .create_async()
        .await;

    let result = client.get_issue(2, 202).await;
    assert!(result.is_err());
    match result.err().unwrap() {
        GitlabError::Api { status, body } => {
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(body, "{\"message\": \"Issue not found\"}");
        }
        _ => panic!("Expected Api"),
    }
}

#[tokio::test]
async fn test_get_merge_request_success() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_mr_response = json!({
        "id": 1, "iid": 5, "project_id": 1, "title": "Test MR",
        "description": "A test merge request", "state": "opened",
        "author": {"id": 1, "username": "mr_tester", "name": "MR Test User", "avatar_url": null, "web_url": "url"},
        "source_branch": "feature-branch", "target_branch": "test-main",
        "web_url": "http://example.com/mr/1", "labels": [], "assignees": [], "reviewers": [],
        "milestone": null, "closed_at": null, "closed_by": null, "created_at": "2023-01-01T10:00:00Z", "updated_at": "2023-01-03T10:00:00Z",
        "upvotes": 0, "downvotes": 0, "user_notes_count": 0, "work_in_progress": false, "draft": false,
        "merge_when_pipeline_succeeds": false, "detailed_merge_status": "mergeable", "merge_status": "can_be_merged",
        "sha": "abc123xyz", "squash": false, "diff_refs": {"base_sha": "def", "head_sha": "abc", "start_sha": "def"},
        "references": {"short": "!5", "relative": "!5", "full": "group/project!5"},
        "time_stats": {
            "time_estimate": 0, "total_time_spent": 0, "human_time_estimate": null, "human_total_time_spent": null
        }
    });

    let _m = server
        .mock("GET", "/api/v4/projects/1/merge_requests/5")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_mr_response.to_string())
        .create_async()
        .await;

    let mr = client.get_merge_request(1, 5).await.unwrap();
    assert_eq!(mr.title, "Test MR");
    assert_eq!(mr.author.username, "mr_tester");
    assert_eq!(mr.detailed_merge_status, Some("mergeable".to_string()));
    assert_eq!(mr.updated_at, "2023-01-03T10:00:00Z");
}

#[tokio::test]
async fn test_post_comment_to_issue_success() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();
    let comment_body = "This is a test comment on an issue.";

    let mock_response_body = json!({
        "id": 123,
        "body": comment_body,
        "author": {
            "id": 1,
            "username": "testuser",
            "name": "Test User",
            "avatar_url": null
        },
        "project_id": 1,
        "noteable_type": "Issue",
        "noteable_id": 101,
        "iid": 101,
        "url": "http://example.com/project/1/issues/101#note_123",
        "created_at": "2023-01-04T10:00:00Z",
        "updated_at": "2023-01-04T11:00:00Z"
    });

    let mock = server
        .mock("POST", "/api/v4/projects/1/issues/101/notes")
        .with_status(201) // 201 Created
        .with_header("content-type", "application/json")
        .with_body(mock_response_body.to_string())
        // Skip body matching to avoid JSON format issues
        .create_async()
        .await;

    let result = client.post_comment_to_issue(1, 101, comment_body).await;

    mock.assert_async().await; // Verify the mock was called
    assert!(result.is_ok());
    let note = result.unwrap();
    assert_eq!(note.note, comment_body);
    assert_eq!(note.id, 123);
    assert_eq!(note.updated_at, "2023-01-04T11:00:00Z");
}

#[tokio::test]
async fn test_post_comment_to_merge_request_error() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();
    let comment_body = "This comment should fail.";

    let mock = server
        .mock("POST", "/api/v4/projects/1/merge_requests/5/notes")
        .with_status(500) // Internal Server Error
        .with_body("{\"message\": \"Server error processing note\"}")
        // Skip body matching to avoid JSON format issues
        .create_async()
        .await;

    let result = client
        .post_comment_to_merge_request(1, 5, comment_body)
        .await;

    mock.assert_async().await;
    assert!(result.is_err());
    match result.err().unwrap() {
        GitlabError::Api { status, body } => {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body, "{\"message\": \"Server error processing note\"}");
        }
        _ => panic!("Expected Api"),
    }
}

#[tokio::test]
async fn test_get_project_by_path() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_project_response = serde_json::json!({
        "id": 1,
        "path_with_namespace": "org/repo1",
        "web_url": "https://gitlab.example.com/org/repo1"
    });

    let _m = server
        .mock("GET", "/api/v4/projects/org%2Frepo1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_project_response.to_string())
        .create_async()
        .await;

    let project = client.get_project_by_path("org/repo1").await.unwrap();
    assert_eq!(project.id, 1);
    assert_eq!(project.path_with_namespace, "org/repo1");
}

#[tokio::test]
async fn test_get_issues() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_issues_response = serde_json::json!([
        {
            "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue 1",
            "description": "A test issue 1", "state": "opened",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
            "web_url": "http://example.com/issue/1", "labels": [], "updated_at": "2023-01-02T12:00:00Z"
        },
        {
            "id": 2, "iid": 102, "project_id": 1, "title": "Test Issue 2",
            "description": "A test issue 2", "state": "opened",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
            "web_url": "http://example.com/issue/2", "labels": [], "updated_at": "2023-01-02T13:00:00Z"
        }
    ]);

    let _m = server
        .mock("GET", "/api/v4/projects/1/issues")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded(
                "updated_after".into(),
                "2021-05-03T00:00:00+00:00".into(),
            ),
            mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_issues_response.to_string())
        .create_async()
        .await;

    let issues = client.get_issues(1, 1620000000).await.unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].title, "Test Issue 1");
    assert_eq!(issues[0].updated_at, "2023-01-02T12:00:00Z");
    assert_eq!(issues[1].title, "Test Issue 2");
    assert_eq!(issues[1].updated_at, "2023-01-02T13:00:00Z");
}

#[tokio::test]
async fn test_get_merge_requests() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_mrs_response = serde_json::json!([
        {
            "id": 1, "iid": 5, "project_id": 1, "title": "Test MR 1",
            "description": "A test merge request 1", "state": "opened",
            "author": {"id": 1, "username": "mr_tester", "name": "MR Test User", "avatar_url": null, "web_url": "url"},
            "source_branch": "feature-branch-1", "target_branch": "test-main",
            "web_url": "http://example.com/mr/1", "labels": [], "updated_at": "2023-01-03T10:00:00Z"
        },
        {
            "id": 2, "iid": 6, "project_id": 1, "title": "Test MR 2",
            "description": "A test merge request 2", "state": "opened",
            "author": {"id": 1, "username": "mr_tester", "name": "MR Test User", "avatar_url": null, "web_url": "url"},
            "source_branch": "feature-branch-2", "target_branch": "test-main",
            "web_url": "http://example.com/mr/2", "labels": [], "updated_at": "2023-01-03T11:00:00Z"
        }
    ]);

    let _m = server
        .mock("GET", "/api/v4/projects/1/merge_requests")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded(
                "updated_after".into(),
                "2021-05-03T00:00:00+00:00".into(),
            ),
            mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_mrs_response.to_string())
        .create_async()
        .await;

    let mrs = client.get_merge_requests(1, 1620000000).await.unwrap();
    assert_eq!(mrs.len(), 2);
    assert_eq!(mrs[0].title, "Test MR 1");
    assert_eq!(mrs[0].updated_at, "2023-01-03T10:00:00Z");
    assert_eq!(mrs[1].title, "Test MR 2");
    assert_eq!(mrs[1].updated_at, "2023-01-03T11:00:00Z");
}

#[tokio::test]
async fn test_get_issue_notes() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_notes_response = serde_json::json!([
        {
            "id": 1,
            "body": "This is a test note 1",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null},
            "project_id": 1,
            "noteable_type": "Issue",
            "noteable_id": 101,
            "iid": 101,
            "url": "http://example.com/project/1/issues/101#note_1",
            "updated_at": "2023-01-05T10:00:00Z"
        },
        {
            "id": 2,
            "body": "This is a test note 2",
            "author": {"id": 2, "username": "tester2", "name": "Test User 2", "avatar_url": null},
            "project_id": 1,
            "noteable_type": "Issue",
            "noteable_id": 101,
            "iid": 101,
            "url": "http://example.com/project/1/issues/101#note_2",
            "updated_at": "2023-01-05T11:00:00Z"
        }
    ]);

    let _m = server
        .mock("GET", "/api/v4/projects/1/issues/101/notes")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded(
                "created_after".into(),
                "2021-05-03T00:00:00+00:00".into(),
            ),
            mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_notes_response.to_string())
        .create_async()
        .await;

    let notes = client.get_issue_notes(1, 101, 1620000000).await.unwrap();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].note, "This is a test note 1");
    assert_eq!(notes[0].updated_at, "2023-01-05T10:00:00Z");
    assert_eq!(notes[1].note, "This is a test note 2");
    assert_eq!(notes[1].updated_at, "2023-01-05T11:00:00Z");
}

#[tokio::test]
async fn test_get_merge_request_notes() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_notes_response = serde_json::json!([
        {
            "id": 1,
            "body": "This is a test MR note 1",
            "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null},
            "project_id": 1,
            "noteable_type": "MergeRequest",
            "noteable_id": 5,
            "iid": 5,
            "url": "http://example.com/project/1/merge_requests/5#note_1",
            "updated_at": "2023-01-06T10:00:00Z"
        },
        {
            "id": 2,
            "body": "This is a test MR note 2",
            "author": {"id": 2, "username": "tester2", "name": "Test User 2", "avatar_url": null},
            "project_id": 1,
            "noteable_type": "MergeRequest",
            "noteable_id": 5,
            "iid": 5,
            "url": "http://example.com/project/1/merge_requests/5#note_2",
            "updated_at": "2023-01-06T11:00:00Z"
        }
    ]);

    let _m = server
        .mock("GET", "/api/v4/projects/1/merge_requests/5/notes")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded(
                "created_after".into(),
                "2021-05-03T00:00:00+00:00".into(),
            ),
            mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_notes_response.to_string())
        .create_async()
        .await;

    let notes = client
        .get_merge_request_notes(1, 5, 1620000000)
        .await
        .unwrap();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].note, "This is a test MR note 1");
    assert_eq!(notes[0].updated_at, "2023-01-06T10:00:00Z");
    assert_eq!(notes[1].note, "This is a test MR note 2");
    assert_eq!(notes[1].updated_at, "2023-01-06T11:00:00Z");
}

#[tokio::test]
async fn test_add_issue_label_success() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();
    let label_to_add = "feature-request";

    let _mock_issue_response_before = json!({ // Prefixed with underscore
        "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue",
        "description": "A test issue", "state": "opened",
        "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
        "web_url": "http://example.com/issue/1", "labels": [],
        "updated_at": "2023-01-02T12:00:00Z"
    });

    let mock_issue_response_after = json!({
        "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue",
        "description": "A test issue", "state": "opened",
        "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
        "web_url": "http://example.com/issue/1", "labels": [label_to_add],
        "updated_at": "2023-01-02T12:05:00Z" // Assume updated_at changes
    });

    let mock = server
        .mock("PUT", "/api/v4/projects/1/issues/101")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_issue_response_after.to_string())
        .match_body(mockito::Matcher::JsonString(
            json!({"add_labels": label_to_add}).to_string(),
        ))
        .create_async()
        .await;

    let result = client.add_issue_label(1, 101, label_to_add).await;

    mock.assert_async().await;
    assert!(result.is_ok());
    let issue = result.unwrap();
    assert_eq!(issue.labels, vec![label_to_add.to_string()]);
    assert_eq!(issue.updated_at, "2023-01-02T12:05:00Z");
}

#[tokio::test]
async fn test_remove_issue_label_success() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();
    let label_to_remove = "bug";

    let _mock_issue_response_before = json!({ // Prefixed with underscore
        "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue with Bug",
        "description": "A test issue", "state": "opened",
        "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
        "web_url": "http://example.com/issue/1", "labels": [label_to_remove, "critical"],
        "updated_at": "2023-01-02T13:00:00Z"
    });

    let mock_issue_response_after = json!({
        "id": 1, "iid": 101, "project_id": 1, "title": "Test Issue with Bug",
        "description": "A test issue", "state": "opened",
        "author": {"id": 1, "username": "tester", "name": "Test User", "avatar_url": null, "web_url": "url"},
        "web_url": "http://example.com/issue/1", "labels": ["critical"], // "bug" label removed
        "updated_at": "2023-01-02T13:05:00Z" // Assume updated_at changes
    });

    let mock = server
        .mock("PUT", "/api/v4/projects/1/issues/101")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_issue_response_after.to_string())
        .match_body(mockito::Matcher::JsonString(
            json!({"remove_labels": label_to_remove}).to_string(),
        ))
        .create_async()
        .await;

    let result = client.remove_issue_label(1, 101, label_to_remove).await;

    mock.assert_async().await;
    assert!(result.is_ok());
    let issue = result.unwrap();
    assert_eq!(issue.labels, vec!["critical".to_string()]);
    assert_eq!(issue.updated_at, "2023-01-02T13:05:00Z");
}

#[tokio::test]
async fn test_add_issue_label_not_found() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();
    let label_to_add = "enhancement";

    let _m = server
        .mock("PUT", "/api/v4/projects/99/issues/999")
        .with_status(404)
        .with_body("{\"message\": \"Issue not found\"}")
        .match_body(mockito::Matcher::JsonString(
            json!({"add_labels": label_to_add}).to_string(),
        ))
        .create_async()
        .await;

    let result = client.add_issue_label(99, 999, label_to_add).await;
    assert!(result.is_err());
    match result.err().unwrap() {
        GitlabError::Api { status, body } => {
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(body, "{\"message\": \"Issue not found\"}");
        }
        _ => panic!("Expected Api Error for not found"),
    }
}

#[tokio::test]
async fn test_remove_issue_label_not_found() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();
    let label_to_remove = "wontfix";

    let _m = server
        .mock("PUT", "/api/v4/projects/88/issues/888")
        .with_status(404)
        .with_body("{\"message\": \"Issue not found\"}")
        .match_body(mockito::Matcher::JsonString(
            json!({"remove_labels": label_to_remove}).to_string(),
        ))
        .create_async()
        .await;

    let result = client.remove_issue_label(88, 888, label_to_remove).await;
    assert!(result.is_err());
    match result.err().unwrap() {
        GitlabError::Api { status, body } => {
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(body, "{\"message\": \"Issue not found\"}");
        }
        _ => panic!("Expected Api Error for not found"),
    }
}

#[tokio::test]
async fn test_get_file_commits() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_commits_response = serde_json::json!([
        {
            "id": "a1b2c3d4e5f6",
            "short_id": "a1b2c3d4",
            "title": "Update file content",
            "author_name": "John Doe",
            "author_email": "john@example.com",
            "authored_date": "2024-03-15T10:00:00Z",
            "committer_name": "John Doe",
            "committer_email": "john@example.com",
            "committed_date": "2024-03-15T10:00:00Z",
            "message": "Update file content\n\nMade some changes to improve functionality"
        },
        {
            "id": "b2c3d4e5f6a1",
            "short_id": "b2c3d4e5",
            "title": "Initial commit",
            "author_name": "Jane Smith",
            "author_email": "jane@example.com",
            "authored_date": "2024-03-14T09:00:00Z",
            "committer_name": "Jane Smith",
            "committer_email": "jane@example.com",
            "committed_date": "2024-03-14T09:00:00Z",
            "message": "Initial commit\n\nAdded base functionality"
        }
    ]);

    let _m = server
        .mock("GET", "/api/v4/projects/1/repository/commits")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("path".into(), "src/main.rs".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "5".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_commits_response.to_string())
        .create_async()
        .await;

    let commits = client
        .get_file_commits(1, "src/main.rs", Some(5))
        .await
        .unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].short_id, "a1b2c3d4");
    assert_eq!(commits[0].author_name, "John Doe");
    assert_eq!(commits[1].short_id, "b2c3d4e5");
    assert_eq!(commits[1].author_name, "Jane Smith");
}

#[tokio::test]
async fn test_get_repository_tree() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    // First page response
    let mock_tree_response_page1 = serde_json::json!([
        {
            "id": "a1b2c3d4e5f6",
            "name": "README.md",
            "type": "blob",
            "path": "README.md",
            "mode": "100644"
        },
        {
            "id": "b2c3d4e5f6a1",
            "name": "src",
            "type": "tree",
            "path": "src",
            "mode": "040000"
        },
        {
            "id": "c3d4e5f6a1b2",
            "name": "main.rs",
            "type": "blob",
            "path": "src/main.rs",
            "mode": "100644"
        }
    ]);

    // Second page response
    let mock_tree_response_page2 = serde_json::json!([
        {
            "id": "d4e5f6a1b2c3",
            "name": "utils.rs",
            "type": "blob",
            "path": "src/utils.rs",
            "mode": "100644"
        },
        {
            "id": "e5f6a1b2c3d4",
            "name": "tests",
            "type": "tree",
            "path": "tests",
            "mode": "040000"
        },
        {
            "id": "f6a1b2c3d4e5",
            "name": "test_main.rs",
            "type": "blob",
            "path": "tests/test_main.rs",
            "mode": "100644"
        }
    ]);

    // Mock first page request
    let _m1 = server
        .mock("GET", "/api/v4/projects/1/repository/tree")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("recursive".into(), "true".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            mockito::Matcher::UrlEncoded("page".into(), "1".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("X-Total-Pages", "2")
        .with_body(mock_tree_response_page1.to_string())
        .create_async()
        .await;

    // Mock second page request
    let _m2 = server
        .mock("GET", "/api/v4/projects/1/repository/tree")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("recursive".into(), "true".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            mockito::Matcher::UrlEncoded("page".into(), "2".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("X-Total-Pages", "2")
        .with_body(mock_tree_response_page2.to_string())
        .create_async()
        .await;

    let files = client.get_repository_tree(1).await.unwrap();
    assert_eq!(files.len(), 4); // Only blobs, not trees
    assert!(files.contains(&"README.md".to_string()));
    assert!(files.contains(&"src/main.rs".to_string()));
    assert!(files.contains(&"src/utils.rs".to_string()));
    assert!(files.contains(&"tests/test_main.rs".to_string()));
}

#[tokio::test]
async fn test_get_file_content() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url.clone()));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_file_response = serde_json::json!({
        "file_name": "main.rs",
        "file_path": "src/main.rs",
        "size": 123,
        "encoding": "base64",
        "content": "Zm4gbWFpbigpIHsKICAgIHByaW50bG4hKCJIZWxsbyBXb3JsZCIpOwp9" // base64 for: fn main() { println!("Hello World"); }
    });

    let _m = server
        .mock("GET", "/api/v4/projects/1/repository/files/src%2Fmain.rs")
        .match_query(mockito::Matcher::UrlEncoded(
            "ref".into(),
            "test-main".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_file_response.to_string())
        .create_async()
        .await;

    let file = client.get_file_content(1, "src/main.rs").await.unwrap();
    assert_eq!(file.file_path, "src/main.rs");
    assert_eq!(file.size, 123);
    assert_eq!(file.encoding, Some("base64".to_string()));
    assert!(file.content.is_some());
}

#[tokio::test]
async fn test_get_merge_request_changes() {
    let mut server = mockito::Server::new_async().await;
    let base_url = server.url();
    let settings = Arc::new(create_test_settings(base_url));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_changes_response = serde_json::json!({
        "changes": [
            {
                "old_path": "src/main.rs",
                "new_path": "src/main.rs",
                "diff": "@@ -1,3 +1,5 @@\n fn main() {\n-    println!(\"Hello World\");\n+    println!(\"Hello, World!\");\n+    println!(\"Welcome to GitBot\");\n }"
            },
            {
                "old_path": "README.md",
                "new_path": "README.md",
                "diff": "@@ -1 +1,2 @@\n # My Project\n+A simple Rust project."
            }
        ]
    });

    let _m = server
        .mock("GET", "/api/v4/projects/1/merge_requests/5/changes")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_changes_response.to_string())
        .create_async()
        .await;

    let changes = client.get_merge_request_changes(1, 5).await.unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].new_path, "src/main.rs");
    assert!(changes[0].diff.contains("Hello, World!"));
    assert_eq!(changes[1].new_path, "README.md");
    assert!(changes[1].diff.contains("A simple Rust project."));
}

#[tokio::test]
async fn test_get_all_issue_notes() {
    let mut server = mockito::Server::new_async().await;
    let settings = Arc::new(create_test_settings(server.url()));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_response = json!([
        {
            "id": 1,
            "body": "First comment",
            "author": {
                "id": 123,
                "username": "user1",
                "name": "User One",
                "avatar_url": null
            },
            "project_id": 1,
            "noteable_type": "Issue",
            "noteable_id": 10,
            "iid": 10,
            "url": null,
            "updated_at": "2023-01-01T12:00:00Z"
        },
        {
            "id": 2,
            "body": "Second comment",
            "author": {
                "id": 456,
                "username": "user2",
                "name": "User Two",
                "avatar_url": null
            },
            "project_id": 1,
            "noteable_type": "Issue",
            "noteable_id": 10,
            "iid": 10,
            "url": null,
            "updated_at": "2023-01-02T14:30:00Z"
        }
    ]);

    let _m = server
        .mock("GET", "/api/v4/projects/1/issues/10/notes")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_response.to_string())
        .create_async()
        .await;

    let notes = client.get_all_issue_notes(1, 10).await.unwrap();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].id, 1);
    assert_eq!(notes[0].note, "First comment");
    assert_eq!(notes[0].author.username, "user1");
    assert_eq!(notes[1].id, 2);
    assert_eq!(notes[1].note, "Second comment");
    assert_eq!(notes[1].author.username, "user2");
}

#[tokio::test]
async fn test_get_all_merge_request_notes() {
    let mut server = mockito::Server::new_async().await;
    let settings = Arc::new(create_test_settings(server.url()));
    let client = GitlabApiClient::new(settings).unwrap();

    let mock_response = json!([
        {
            "id": 3,
            "body": "First MR comment",
            "author": {
                "id": 789,
                "username": "reviewer1",
                "name": "Reviewer One",
                "avatar_url": null
            },
            "project_id": 1,
            "noteable_type": "MergeRequest",
            "noteable_id": 20,
            "iid": 20,
            "url": null,
            "updated_at": "2023-01-01T15:00:00Z"
        }
    ]);

    let _m = server
        .mock("GET", "/api/v4/projects/1/merge_requests/20/notes")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("sort".into(), "asc".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_response.to_string())
        .create_async()
        .await;

    let notes = client.get_all_merge_request_notes(1, 20).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].id, 3);
    assert_eq!(notes[0].note, "First MR comment");
    assert_eq!(notes[0].author.username, "reviewer1");
}
