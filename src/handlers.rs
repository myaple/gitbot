use crate::config::AppSettings;
use crate::gitlab::GitlabApiClient;
use crate::models::{GitlabNoteEvent, OpenAIChatMessage, OpenAIChatRequest};
use crate::openai::OpenAIApiClient;
use crate::repo_context::RepoContextExtractor;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

// Helper function to extract context after bot mention
fn extract_context_after_mention(note: &str, bot_name: &str) -> Option<String> {
    let mention = format!("@{}", bot_name);
    note.find(&mention).and_then(|start_index| {
        let after_mention = &note[start_index + mention.len()..];
        let trimmed_context = after_mention.trim();
        if trimmed_context.is_empty() {
            None
        } else {
            Some(trimmed_context.to_string())
        }
    })
}

pub async fn process_mention(
    event: GitlabNoteEvent,
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
) -> Result<()> {
    // Log Event Details
    info!(
        "Processing mention from user: {} in project: {}",
        event.user.username, event.project.path_with_namespace
    );

    // Self-Mention Check (using bot_username from config)
    if event.user.username == config.bot_username {
        info!(
            "Comment is from the bot itself (@{}), ignoring.",
            config.bot_username
        );
        return Ok(());
    }

    // Initialize variables at the top level
    let project_id = event.project.id;
    let mut prompt_parts = Vec::new();
    let mut commit_history = String::new();
    let mut is_issue = false;

    // Verify Object Kind and Event Type
    if event.object_kind != "note" || event.event_type != "note" {
        warn!(
            "Received event with object_kind: '{}' and event_type: '{}'. Expected 'note' for both. Ignoring.",
            event.object_kind, event.event_type
        );
        return Err(anyhow!("Event is not a standard note event"));
    }
    info!("Event object_kind and event_type verified as 'note'.");

    // Extract Note Details
    let note_attributes = &event.object_attributes;
    let note_content = &note_attributes.note;

    // Check if bot is mentioned
    let user_provided_context = extract_context_after_mention(note_content, &config.bot_username);

    if user_provided_context.is_none()
        && !note_content.contains(&format!("@{}", config.bot_username))
    {
        info!(
            "Bot @{} was not directly mentioned with a command or the command was empty. Ignoring.",
            config.bot_username
        );
        return Ok(());
    }
    info!("Bot @{} was mentioned.", config.bot_username);

    // --- START: Check if already replied ---
    let mention_timestamp_str = &event.object_attributes.updated_at;
    let mention_timestamp_dt = match DateTime::parse_from_rfc3339(mention_timestamp_str) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(e) => {
            error!(
                mention_timestamp = %mention_timestamp_str,
                error = %e,
                "Failed to parse mention timestamp. Cannot reliably check for previous replies. Aborting."
            );
            return Err(anyhow!(
                "Failed to parse mention timestamp '{}': {}",
                mention_timestamp_str,
                e
            ));
        }
    };

    let notes_since_timestamp_u64 = mention_timestamp_dt.timestamp() as u64;
    let project_id_for_notes = event.project.id;
    let current_mention_note_id = event.object_attributes.id;

    debug!(
        mention_id = current_mention_note_id,
        mention_timestamp = %mention_timestamp_dt,
        notes_since_unix_ts = notes_since_timestamp_u64,
        "Preparing to check for subsequent bot replies."
    );

    let subsequent_notes_result = match event.object_attributes.noteable_type.as_str() {
        "Issue" => {
            is_issue = true;
            let issue_iid = match event.issue.as_ref().map(|i| i.iid) {
                Some(iid) => iid,
                None => {
                    error!(
                        "Missing issue details (iid) in note event for an Issue. Event: {:?}",
                        event
                    );
                    return Err(anyhow!("Missing issue details in note event"));
                }
            };
            info!(
                project_id = project_id_for_notes,
                issue_iid = issue_iid,
                notes_since_timestamp_u64 = notes_since_timestamp_u64,
                "Fetching subsequent notes for issue to check for prior bot replies."
            );
            gitlab_client
                .get_issue_notes(project_id_for_notes, issue_iid, notes_since_timestamp_u64)
                .await
        }
        "MergeRequest" => {
            is_issue = false;
            let mr_iid = match event.merge_request.as_ref().map(|mr| mr.iid) {
                Some(iid) => iid,
                None => {
                    error!("Missing merge request details (iid) in note event for a MergeRequest. Event: {:?}", event);
                    return Err(anyhow!("Missing merge request details in note event"));
                }
            };
            info!(
                project_id = project_id_for_notes,
                mr_iid = mr_iid,
                notes_since_timestamp_u64 = notes_since_timestamp_u64,
                "Fetching subsequent notes for merge request to check for prior bot replies."
            );
            gitlab_client
                .get_merge_request_notes(project_id_for_notes, mr_iid, notes_since_timestamp_u64)
                .await
        }
        other_type => {
            warn!(
                noteable_type = %other_type,
                "Unsupported noteable_type for checking subsequent notes. Skipping reply check."
            );
            Ok(Vec::new()) // Return an empty vec, so the loop below evaluating notes doesn't run
        }
    };

    match subsequent_notes_result {
        Ok(notes) => {
            if !notes.is_empty() {
                debug!("Fetched {} subsequent notes for reply check.", notes.len());
            }
            for note in notes {
                // Skip the current mention note itself. This is vital.
                if note.id == current_mention_note_id {
                    trace!(
                        note_id = note.id,
                        "Skipping current mention note (self) during reply check."
                    );
                    continue;
                }

                // Parse the fetched note's timestamp
                match DateTime::parse_from_rfc3339(&note.updated_at) {
                    Ok(fetched_note_dt_utc) => {
                        let fetched_note_dt = fetched_note_dt_utc.with_timezone(&Utc);

                        // Check if the note is from the bot and was created strictly after the mention
                        if note.author.username == config.bot_username
                            && fetched_note_dt > mention_timestamp_dt
                        {
                            info!(
                               note_id = note.id,
                               note_author = %note.author.username,
                               note_timestamp = %fetched_note_dt,
                               mention_id = current_mention_note_id,
                               mention_timestamp = %mention_timestamp_dt,
                               "Bot @{} has already replied (note ID: {}) after this mention (note ID: {}). Ignoring current mention.",
                               config.bot_username,
                               note.id,
                               current_mention_note_id
                            );
                            return Ok(()); // Already replied
                        }
                    }
                    Err(e) => {
                        warn!(
                            note_id = note.id,
                            note_timestamp_str = %note.updated_at,
                            error = %e,
                            "Failed to parse timestamp for a fetched note. Skipping this note in reply check."
                        );
                    }
                }
            }
            // If we've reached this point without returning Ok(()), no relevant bot reply was found.
            info!("No subsequent bot reply found that meets the criteria for duplicate prevention. Proceeding to process mention.");
        }
        Err(e) => {
            warn!(
                mention_id = current_mention_note_id,
                error = %e,
                "Failed to fetch subsequent notes to check for prior replies. Proceeding with mention processing as a precaution."
            );
        }
    }
    // --- END: Check if already replied ---

    // Read CONTRIBUTING.md
    let contributing_md_content: Option<String> = match std::fs::read_to_string("CONTRIBUTING.md") {
        Ok(content) if !content.is_empty() => Some(content),
        Ok(_) => {
            info!("CONTRIBUTING.md is empty, will not use for prompt.");
            None
        }
        Err(e) => {
            warn!(
                "Failed to read CONTRIBUTING.md: {}. It will not be used in the prompt.",
                e
            );
            None
        }
    };

    // Prompt Assembly Logic
    match note_attributes.noteable_type.as_str() {
        "Issue" => {
            let issue_iid = match event.issue.as_ref().map(|i| i.iid) {
                Some(iid) => iid,
                None => {
                    error!(
                        "Missing issue details (iid) in note event for an Issue. Event: {:?}",
                        event
                    );
                    return Err(anyhow!("Missing issue details in note event"));
                }
            };
            info!(
                "Note event pertains to Issue #{} in project ID {}.",
                issue_iid, project_id
            );

            // Check and remove "stale" label if a user (not the bot) comments on a stale issue
            if event.user.username != config.bot_username {
                match gitlab_client.get_issue(project_id, issue_iid).await {
                    Ok(issue_details_for_stale_check) => {
                        if issue_details_for_stale_check
                            .labels
                            .iter()
                            .any(|label| label == "stale")
                        {
                            info!("Issue #{} has 'stale' label and received a comment from user {}. Attempting to remove 'stale' label.", issue_iid, event.user.username);
                            match gitlab_client.remove_issue_label(project_id, issue_iid, "stale").await {
                                Ok(_) => info!("Successfully removed 'stale' label from issue #{}", issue_iid),
                                Err(e) => warn!("Failed to remove 'stale' label from issue #{}: {}. Processing will continue.", issue_iid, e),
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to fetch issue details for stale check on issue #{}: {}. Stale label check will be skipped.", issue_iid, e);
                    }
                }
            }

            let issue = gitlab_client
                .get_issue(project_id, issue_iid)
                .await
                .map_err(|e| {
                    error!("Failed to get issue details for summary: {}", e);
                    anyhow!("Failed to fetch issue details from GitLab: {}", e)
                })?;

            if let Some(context) = &user_provided_context {
                prompt_parts.push(format!(
                    "The user @{} provided the following request regarding this issue: '{}'.",
                    event.user.username, context
                ));
                let issue_details = gitlab_client
                    .get_issue(project_id, issue_iid)
                    .await
                    .map_err(|e| {
                        error!("Failed to get issue details for context: {}", e);
                        anyhow!("Failed to fetch issue details from GitLab: {}", e)
                    })?;

                prompt_parts.push(format!("Title: {}", issue_details.title));
                prompt_parts.push(format!(
                    "Description: {}",
                    issue_details.description.as_deref().unwrap_or("N/A")
                ));

                prompt_parts.push(format!("State: {}", issue.state));
                if !issue.labels.is_empty() {
                    prompt_parts.push(format!("Labels: {}", issue.labels.join(", ")));
                }

                // Add repository context
                let repo_context_extractor =
                    RepoContextExtractor::new(gitlab_client.clone(), config.clone());
                match repo_context_extractor
                    .extract_context_for_issue(
                        &issue,
                        &event.project,
                        config.context_repo_path.as_deref(),
                    )
                    .await
                {
                    Ok(context) => {
                        prompt_parts.push(format!("Repository Context: {}", context));
                    }
                    Err(e) => {
                        warn!("Failed to extract repository context: {}", e);
                    }
                }

                prompt_parts.push(format!("User's specific request: {}", context));
            } else {
                // No specific context, summarize and suggest steps
                prompt_parts.push(format!(
                    "Please summarize this issue for user @{} and suggest steps to address it. Be specific about which files, functions, or modules need to be modified.",
                    event.user.username
                ));
                prompt_parts.push(format!("Issue Title: {}", issue.title));
                prompt_parts.push(format!(
                    "Issue Description: {}",
                    issue.description.as_deref().unwrap_or("No description.")
                ));
                prompt_parts.push(format!("Author: {}", issue.author.name));
                prompt_parts.push(format!("State: {}", issue.state));
                if !issue.labels.is_empty() {
                    prompt_parts.push(format!("Labels: {}", issue.labels.join(", ")));
                }

                // Add repository context
                let repo_context_extractor =
                    RepoContextExtractor::new(gitlab_client.clone(), config.clone());
                match repo_context_extractor
                    .extract_context_for_issue(
                        &issue,
                        &event.project,
                        config.context_repo_path.as_deref(),
                    )
                    .await
                {
                    Ok(context) => {
                        prompt_parts.push(format!("Repository Context: {}", context));
                    }
                    Err(e) => {
                        warn!("Failed to extract repository context: {}", e);
                    }
                }

                // Add instructions for steps
                prompt_parts.push(
                    String::from(
                        "Please provide a summary of the issue and suggest specific steps to",
                    ) + "address it based on the repository context. Again, be specific about"
                        + "which files, functions, or modules need to be modified.",
                );
            }
        }
        "MergeRequest" => {
            let mr_iid = match event.merge_request.as_ref().map(|mr| mr.iid) {
                Some(iid) => iid,
                None => {
                    error!("Missing merge request details (iid) in note event for a MergeRequest. Event: {:?}", event);
                    return Err(anyhow!("Missing merge request details in note event"));
                }
            };
            info!(
                "Note event pertains to Merge Request !{} in project ID {}.",
                mr_iid, project_id
            );

            let mr = gitlab_client
                .get_merge_request(project_id, mr_iid)
                .await
                .map_err(|e| {
                    error!("Failed to get MR details for summary: {}", e);
                    anyhow!("Failed to fetch MR details from GitLab: {}", e)
                })?;

            if let Some(context) = &user_provided_context {
                prompt_parts.push(format!("The user @{} provided the following request regarding this merge request: '{}'.", event.user.username, context));

                prompt_parts.push(format!("Title: {}", mr.title));
                prompt_parts.push(format!(
                    "Description: {}",
                    mr.description.as_deref().unwrap_or("N/A")
                ));
                prompt_parts.push(format!("State: {}", mr.state));
                if !mr.labels.is_empty() {
                    prompt_parts.push(format!("Labels: {}", mr.labels.join(", ")));
                }
                prompt_parts.push(format!("Source Branch: {}", mr.source_branch));
                prompt_parts.push(format!("Target Branch: {}", mr.target_branch));

                // Add code diff context
                let repo_context_extractor =
                    RepoContextExtractor::new(gitlab_client.clone(), config.clone());
                match repo_context_extractor
                    .extract_context_for_mr(&mr, &event.project)
                    .await
                {
                    Ok((context_for_llm, context_for_comment)) => {
                        prompt_parts.push(format!("Code Changes: {}", context_for_llm));
                        commit_history = context_for_comment;
                    }
                    Err(e) => {
                        warn!("Failed to extract merge request diff context: {}", e);
                    }
                }

                prompt_parts.push(format!("User's specific request: {}", context));
            } else {
                // No specific context, summarize with code diffs
                prompt_parts.push(format!("Please review this merge request for user @{} and provide a summary of the changes.", event.user.username));
                prompt_parts.push(format!("Merge Request Title: {}", mr.title));
                prompt_parts.push(format!(
                    "Merge Request Description: {}",
                    mr.description.as_deref().unwrap_or("No description.")
                ));
                prompt_parts.push(format!("Author: {}", mr.author.name));
                prompt_parts.push(format!("State: {}", mr.state));
                if !mr.labels.is_empty() {
                    prompt_parts.push(format!("Labels: {}", mr.labels.join(", ")));
                }
                prompt_parts.push(format!("Source Branch: {}", mr.source_branch));
                prompt_parts.push(format!("Target Branch: {}", mr.target_branch));

                // Add code diff context
                let repo_context_extractor =
                    RepoContextExtractor::new(gitlab_client.clone(), config.clone());
                match repo_context_extractor
                    .extract_context_for_mr(&mr, &event.project)
                    .await
                {
                    Ok((context_for_llm, context_for_comment)) => {
                        prompt_parts.push(format!("Code Changes: {}", context_for_llm));
                        commit_history = context_for_comment;
                    }
                    Err(e) => {
                        warn!("Failed to extract merge request diff context: {}", e);
                    }
                }

                // Add instructions for review
                if let Some(contributing_content) = &contributing_md_content {
                    prompt_parts.push(format!(
                        "The following are the guidelines from CONTRIBUTING.md:\n{}\n\nPlease review how well this MR adheres to these guidelines.",
                        contributing_content
                    ));
                    prompt_parts.push(
                        "Provide specific examples of good adherence and areas for improvement. \
                        Offer constructive criticism and praise regarding its adherence. \
                        Finally, provide an overall summary of the merge request and feedback on the implementation.".to_string()
                    );
                } else {
                    // Fallback if CONTRIBUTING.md is not available
                    prompt_parts.push(
                        "Please provide a summary of the merge request, review the code changes, and provide feedback on the implementation.".to_string()
                    );
                }
            }
        }
        other_type => {
            info!(
                "Note on unsupported noteable_type: {}, ignoring.",
                other_type
            );
            return Ok(());
        }
    };

    let final_prompt_text = format!(
        "{}\n\nContext:\n{}",
        prompt_parts.join("\n---\n"),
        commit_history
    );
    trace!("Formatted prompt for LLM:\n{}", final_prompt_text);
    trace!("Full prompt for LLM (debug):\n{}", final_prompt_text);

    // Create OpenAI client
    let openai_client = OpenAIApiClient::new(&config)
        .map_err(|e| anyhow!("Failed to create OpenAI client: {}", e))?;

    // Call OpenAI Client
    let messages = vec![OpenAIChatMessage {
        role: "user".to_string(),
        content: final_prompt_text,
    }];
    let openai_request = OpenAIChatRequest {
        model: config.openai_model.clone(),
        messages,
        temperature: Some(config.openai_temperature),
        max_tokens: Some(config.openai_max_tokens),
    };

    let openai_response = openai_client
        .send_chat_completion(&openai_request)
        .await
        .map_err(|e| {
            error!("Failed to communicate with OpenAI: {}", e);
            anyhow!("Failed to communicate with OpenAI: {}", e)
        })?;

    debug!("OpenAI response: {:?}", openai_response);

    // Extract LLM's Reply
    let llm_reply = openai_response
        .choices
        .first()
        .ok_or_else(|| anyhow!("No response choices from OpenAI"))?
        .message
        .content
        .clone();

    // Format final comment
    let final_comment_body = if is_issue {
        format!(
            "Hey @{}, here's the information you requested:\n\n---\n\n{}",
            event.user.username, llm_reply
        )
    } else {
        // For merge requests, include commit history only if no user context was provided
        if user_provided_context.is_none() {
            format!(
                "Hey @{}, here's the information you requested:\n\n---\n\n{}\n\n### Commit History\n\n{}",
                event.user.username, llm_reply, commit_history
            )
        } else {
            format!(
                "Hey @{}, here's the information you requested:\n\n---\n\n{}",
                event.user.username, llm_reply
            )
        }
    };

    // Post the comment
    if is_issue {
        let issue_iid = event.issue.as_ref().map(|i| i.iid).ok_or_else(|| {
            error!(
                "Critical: Missing issue_iid when trying to post comment. Event: {:?}",
                event
            );
            anyhow!("Internal error: Missing issue context for comment posting")
        })?;

        gitlab_client
            .post_comment_to_issue(project_id, issue_iid, &final_comment_body)
            .await
            .map_err(|e| {
                error!(
                    "Failed to post comment to issue {}#{}: {}",
                    project_id, issue_iid, e
                );
                anyhow!("Failed to post comment to GitLab issue: {}", e)
            })?;

        info!(
            "Successfully posted comment to issue {}#{}",
            project_id, issue_iid
        );
    } else {
        // Is Merge Request
        let mr_iid = event
            .merge_request
            .as_ref()
            .map(|mr| mr.iid)
            .ok_or_else(|| {
                error!(
                    "Critical: Missing mr_iid when trying to post comment. Event: {:?}",
                    event
                );
                anyhow!("Internal error: Missing MR context for comment posting")
            })?;

        gitlab_client
            .post_comment_to_merge_request(project_id, mr_iid, &final_comment_body)
            .await
            .map_err(|e| {
                error!(
                    "Failed to post comment to MR {}!{}: {}",
                    project_id, mr_iid, e
                );
                anyhow!("Failed to post comment to GitLab merge request: {}", e)
            })?;

        info!(
            "Successfully posted comment to MR {}!{}",
            project_id, mr_iid
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        GitlabNoteAttributes, GitlabNoteEvent, GitlabNoteObject, GitlabProject, GitlabUser,
    };
    use std::sync::Arc;

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
    async fn test_process_mention_self_mention() {
        // Create a test event where the bot mentions itself
        let event = create_test_note_event("gitbot", "Issue");

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
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            max_age_hours: 24,
            stale_issue_days: 30, // Added default for tests
            context_repo_path: None,
            max_context_size: 60000,
        };
        let gitlab_client = Arc::new(GitlabApiClient::new(&settings).unwrap());

        // Process the mention
        let result = process_mention(event, gitlab_client, config).await;

        // Should return Ok since we're ignoring self-mentions
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_mention_no_bot_mention() {
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
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            max_age_hours: 24,
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
            stale_issue_days: 30, // Added default for tests
            context_repo_path: None,
            max_context_size: 60000,
        };
        let gitlab_client = Arc::new(GitlabApiClient::new(&settings).unwrap());

        // Process the mention
        let result = process_mention(event, gitlab_client, config).await;

        // Should return Ok since we're ignoring comments without mentions
        assert!(result.is_ok());
    }

    // Helper function to create a test note event
    fn create_test_note_event(username: &str, noteable_type: &str) -> GitlabNoteEvent {
        let user = GitlabUser {
            id: 1,
            username: username.to_string(),
            name: format!("{} User", username),
            avatar_url: None,
        };

        let project = GitlabProject {
            id: 1,
            path_with_namespace: "org/repo1".to_string(),
            web_url: "https://gitlab.example.com/org/repo1".to_string(),
        };

        let note_attributes = GitlabNoteAttributes {
            id: 1,
            note: format!(
                "Hello @gitbot please help with this {}",
                noteable_type.to_lowercase()
            ),
            author: user.clone(),
            project_id: 1,
            noteable_type: noteable_type.to_string(),
            noteable_id: Some(1),
            iid: Some(1),
            url: Some("https://gitlab.example.com/org/repo1/-/issues/1#note_1".to_string()),
            updated_at: "2023-01-01T00:00:00Z".to_string(), // Added default for tests
        };

        let issue = if noteable_type == "Issue" {
            Some(GitlabNoteObject {
                id: 1,
                iid: 1,
                title: "Test Issue".to_string(),
                description: Some("This is a test issue".to_string()),
            })
        } else {
            None
        };

        let merge_request = if noteable_type == "MergeRequest" {
            Some(GitlabNoteObject {
                id: 1,
                iid: 1,
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
}
