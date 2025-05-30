use crate::config::AppSettings;
use crate::gitlab::{GitlabApiClient, GitlabError};
use crate::mention_cache::MentionCache; // Added
use crate::models::{GitlabNoteEvent, OpenAIChatMessage, OpenAIChatRequest};
use crate::openai::OpenAIApiClient;
use crate::repo_context::RepoContextExtractor;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::sync::Arc;
// Removed std::collections::HashSet and tokio::sync::Mutex
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

// Helper function to check if the bot has already replied to a mention
async fn has_bot_already_replied(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    processed_mentions_cache: &MentionCache,
    is_issue: &mut bool, // This function will set this based on noteable_type
) -> Result<bool> {
    let mention_timestamp_str = &event.object_attributes.updated_at;
    let mention_timestamp_dt = match DateTime::parse_from_rfc3339(mention_timestamp_str) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(e) => {
            error!(
                mention_timestamp = %mention_timestamp_str,
                error = %e,
                "Failed to parse mention timestamp. Cannot reliably check for previous replies. Aborting."
            );
            // Return an error that will be propagated up by process_mention
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
            *is_issue = true;
            let issue_iid = match event.issue.as_ref().map(|i| i.iid) {
                Some(iid) => iid,
                None => {
                    error!(
                        "Missing issue details (iid) in note event for an Issue. Event: {:?}",
                        event
                    );
                    // Return an error that will be propagated up by process_mention
                    return Err(anyhow!(
                        "Missing issue details in note event for reply check"
                    ));
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
            *is_issue = false;
            let mr_iid = match event.merge_request.as_ref().map(|mr| mr.iid) {
                Some(iid) => iid,
                None => {
                    error!("Missing merge request details (iid) in note event for a MergeRequest. Event: {:?}", event);
                    // Return an error that will be propagated up by process_mention
                    return Err(anyhow!(
                        "Missing merge request details in note event for reply check"
                    ));
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
                            // Add to cache because we've confirmed a prior reply exists
                            processed_mentions_cache.add(current_mention_note_id).await; // Updated logic
                            info!(
                                "Mention ID {} (already replied by note ID {}) added to cache.",
                                current_mention_note_id, note.id
                            );
                            return Ok(true); // Already replied
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
            // If we've reached this point without returning Ok(true), no relevant bot reply was found.
            info!("No subsequent bot reply found that meets the criteria for duplicate prevention. Proceeding to process mention.");
            Ok(false) // Not replied
        }
        Err(e) => {
            warn!(
                mention_id = current_mention_note_id,
                error = %e,
                "Failed to fetch subsequent notes to check for prior replies. Proceeding with mention processing as a precaution."
            );
            Ok(false) // Proceed as if not replied, but with a warning
        }
    }
}

pub async fn process_mention(
    event: GitlabNoteEvent,
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
    processed_mentions_cache: &MentionCache, // Changed type
) -> Result<()> {
    // Log Event Details
    info!(
        "Processing mention from user: {} in project: {}, mention_id: {}",
        event.user.username, event.project.path_with_namespace, event.object_attributes.id
    );

    // Self-Mention Check (using bot_username from config)
    if event.user.username == config.bot_username {
        info!(
            "Comment is from the bot itself (@{}), ignoring mention_id: {}.",
            config.bot_username, event.object_attributes.id
        );
        return Ok(());
    }

    // Cache Check
    let mention_id = event.object_attributes.id;
    if processed_mentions_cache.check(mention_id).await {
        // Updated logic
        info!("Mention ID {} found in cache, skipping.", mention_id);
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
    if has_bot_already_replied(
        &event,
        &gitlab_client,
        &config,
        processed_mentions_cache,
        &mut is_issue,
    )
    .await?
    {
        return Ok(());
    }
    // --- END: Check if already replied ---

    // Prompt Assembly Logic
    match note_attributes.noteable_type.as_str() {
        "Issue" => {
            handle_issue_mention(
                &event,
                &gitlab_client,
                &config,
                project_id,
                &mut prompt_parts,
                &user_provided_context,
            )
            .await?
        }
        "MergeRequest" => {
            handle_merge_request_mention(
                &event,
                &gitlab_client,
                &config,
                project_id,
                &mut prompt_parts,
                &mut commit_history,
                &user_provided_context,
            )
            .await?
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

    let llm_reply = get_llm_reply(&openai_client, &config, &final_prompt_text).await?;

    // Format final comment
    let final_comment_body = format_final_reply_body(
        &event.user.username,
        &llm_reply,
        is_issue,
        &user_provided_context,
        &commit_history,
    );

    // Post the comment
    post_reply_to_gitlab(
        &event,
        &gitlab_client,
        project_id,
        is_issue,
        &final_comment_body,
    )
    .await?;

    // Add to cache after successful processing
    processed_mentions_cache.add(mention_id).await; // Updated logic
    info!("Mention ID {} added to cache.", mention_id);

    Ok(())
}

fn format_final_reply_body(
    event_user_username: &str,
    llm_reply: &str,
    is_issue: bool,
    user_provided_context: &Option<String>,
    commit_history: &str,
) -> String {
    if is_issue {
        format!(
            "Hey @{}, here's the information you requested:\n\n---\n\n{}",
            event_user_username, llm_reply
        )
    } else {
        // For merge requests, include commit history only if no user context was provided
        if user_provided_context.is_none() {
            format!(
                "Hey @{}, here's the information you requested:\n\n---\n\n{}\n\n<details><summary>Additional Commit History</summary>\n\n{}</details>",
                event_user_username, llm_reply, commit_history
            )
        } else {
            format!(
                "Hey @{}, here's the information you requested:\n\n---\n\n{}",
                event_user_username, llm_reply
            )
        }
    }
}

async fn post_reply_to_gitlab(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
    is_issue: bool,
    final_comment_body: &str,
) -> Result<()> {
    if is_issue {
        let issue_iid = event.issue.as_ref().map(|i| i.iid).ok_or_else(|| {
            error!(
                "Critical: Missing issue_iid when trying to post comment. Event: {:?}",
                event
            );
            anyhow!("Internal error: Missing issue context for comment posting")
        })?;

        gitlab_client
            .post_comment_to_issue(project_id, issue_iid, final_comment_body)
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
            .post_comment_to_merge_request(project_id, mr_iid, final_comment_body)
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

async fn get_llm_reply(
    openai_client: &OpenAIApiClient,
    config: &Arc<AppSettings>,
    prompt_text: &str,
) -> Result<String> {
    // Call OpenAI Client
    let messages = vec![OpenAIChatMessage {
        role: "user".to_string(),
        content: prompt_text.to_string(), // Convert &str to String
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
    openai_response
        .choices
        .first()
        .ok_or_else(|| anyhow!("No response choices from OpenAI"))
        .and_then(|choice| {
            if choice.message.content.is_empty() {
                Err(anyhow!("LLM response content is empty"))
            } else {
                Ok(choice.message.content.clone())
            }
        })
}

async fn handle_issue_mention(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    project_id: i64,
    prompt_parts: &mut Vec<String>,
    user_provided_context: &Option<String>,
) -> Result<()> {
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

    if let Some(context) = user_provided_context {
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
        // The extract_context_for_issue function now handles errors internally and will always return Ok
        // with as much context as it could gather
        match repo_context_extractor
            .extract_context_for_issue(&issue, &event.project, config.context_repo_path.as_deref())
            .await
        {
            Ok(context_str) => {
                prompt_parts.push(format!("Repository Context: {}", context_str));
            }
            Err(e) => {
                // This should now only happen in catastrophic failures
                warn!(
                    "Failed to extract repository context: {}. This is a critical error.",
                    e
                );
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
        // The extract_context_for_issue function now handles errors internally and will always return Ok
        // with as much context as it could gather
        match repo_context_extractor
            .extract_context_for_issue(&issue, &event.project, config.context_repo_path.as_deref())
            .await
        {
            Ok(context_str) => {
                prompt_parts.push(format!("Repository Context: {}", context_str));
            }
            Err(e) => {
                // This should now only happen in catastrophic failures
                warn!(
                    "Failed to extract repository context: {}. This is a critical error.",
                    e
                );
            }
        }

        // Add instructions for steps
        prompt_parts.push(
            String::from("Please provide a summary of the issue and suggest specific steps to")
                + "address it based on the repository context. Again, be specific about"
                + "which files, functions, or modules need to be modified.",
        );
    }
    Ok(())
}

async fn handle_merge_request_mention(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    project_id: i64,
    prompt_parts: &mut Vec<String>,
    commit_history: &mut String, // Changed to mutable reference
    user_provided_context: &Option<String>,
) -> Result<()> {
    let mr_iid = match event.merge_request.as_ref().map(|mr| mr.iid) {
        Some(iid) => iid,
        None => {
            error!(
                "Missing merge request details (iid) in note event for a MergeRequest. Event: {:?}",
                event
            );
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

    let mut contributing_md_content: Option<String> = None;
    match gitlab_client
        .get_file_content(project_id, "CONTRIBUTING.md")
        .await
    {
        Ok(file_response) => {
            if let Some(content) = file_response.content {
                if !content.is_empty() {
                    info!(
                        "Successfully fetched and decoded CONTRIBUTING.md for project ID {}",
                        project_id
                    );
                    contributing_md_content = Some(content);
                } else {
                    info!(
                        "CONTRIBUTING.md is empty for project ID {}. Proceeding without it.",
                        project_id
                    );
                }
            } else {
                // This case might occur if the API returns a success status but no content field,
                // or if get_file_content is changed to return Ok(GitlabFile { content: None }) on 404.
                info!(
                    "CONTRIBUTING.md has no content or content was null for project ID {}. Proceeding without it.",
                    project_id
                );
            }
        }
        Err(e) => match e {
            GitlabError::Api { status, .. } if status == reqwest::StatusCode::NOT_FOUND => {
                info!(
                    "CONTRIBUTING.md not found (404) for project ID {}. Proceeding without it.",
                    project_id
                );
            }
            _ => {
                warn!(
                    "Failed to fetch CONTRIBUTING.md for project ID {}: {:?}. Proceeding without it.",
                    project_id, e
                );
            }
        },
    }

    if let Some(context) = user_provided_context {
        prompt_parts.push(format!(
            "The user @{} provided the following request regarding this merge request: '{}'.",
            event.user.username, context
        ));

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
            .extract_context_for_mr(&mr, &event.project, config.context_repo_path.as_deref())
            .await
        {
            Ok((context_for_llm, context_for_comment)) => {
                prompt_parts.push(format!("Code Changes: {}", context_for_llm));
                *commit_history = context_for_comment; // Update commit_history
            }
            Err(e) => {
                warn!("Failed to extract merge request diff context: {}", e);
            }
        }

        prompt_parts.push(format!("User's specific request: {}", context));
    } else {
        // No specific context, summarize with code diffs
        prompt_parts.push(format!(
            "Please review this merge request for user @{} and provide a summary of the changes.",
            event.user.username
        ));
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
            .extract_context_for_mr(&mr, &event.project, config.context_repo_path.as_deref())
            .await
        {
            Ok((context_for_llm, context_for_comment)) => {
                prompt_parts.push(format!("Code Changes: {}", context_for_llm));
                *commit_history = context_for_comment; // Update commit_history
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mention_cache::MentionCache; // Added for tests
    use crate::models::{
        GitlabIssue, GitlabNoteAttributes, GitlabNoteEvent, GitlabNoteObject, GitlabProject,
        GitlabUser,
    };
    use chrono::Duration as ChronoDuration;
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
        };
        let gitlab_client = Arc::new(GitlabApiClient::new(Arc::new(settings.clone())).unwrap());

        // Create a cache for the test
        let cache = MentionCache::new();

        // Process the mention
        let result = process_mention(event, gitlab_client, config, &cache).await; // Pass as reference

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
        };
        let gitlab_client = Arc::new(GitlabApiClient::new(Arc::new(settings.clone())).unwrap());

        // Process the mention
        let result = process_mention(event, gitlab_client, config, &cache).await; // Pass as reference

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

        let result = process_mention(event, gitlab_client.clone(), config.clone(), &cache).await; // Pass as reference

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

        let result = process_mention(event, gitlab_client, config, &cache).await; // Pass as reference

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

        let result = process_mention(event, gitlab_client, config, &cache).await; // Pass as reference

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

        let result = process_mention(event, gitlab_client, config, &cache).await; // Pass as reference

        assert!(result.is_err());
        assert!(!cache.check(TEST_MENTION_ID).await); // Cache should NOT contain the ID
    }
}
