use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

use crate::config::AppSettings;
use crate::file_indexer::FileIndexManager;
use crate::gitlab::{GitlabApiClient, GitlabError};
use crate::mention_cache::MentionCache;
use crate::models::{GitlabNoteAttributes, GitlabNoteEvent, OpenAIChatMessage, OpenAIChatRequest, OpenAITool, OpenAIFunction, OpenAIToolCall};
use crate::openai::OpenAIApiClient;
use crate::repo_context::RepoContextExtractor;

// Helper function to extract context after bot mention
pub(crate) fn extract_context_after_mention(note: &str, bot_name: &str) -> Option<String> {
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

// Slash command definitions
#[derive(Debug, Clone, PartialEq)]
pub enum SlashCommand {
    Summarize,
    Postmortem,
    Suggestions,
    Help,
}

impl SlashCommand {
    pub fn get_precanned_prompt(&self) -> &'static str {
        match self {
            SlashCommand::Summarize => {
                "Summarize changes with a section for adherence to guidelines, possible changes to cpu/memory/big-O performance, strengths, areas for improvement in the changes, and a conclusion with recommendations."
            }
            SlashCommand::Postmortem => {
                "Summarize in a traditional, SaaS postmortem, including a timeline and root cause analysis sections."
            }
            SlashCommand::Suggestions => {
                "Describe a possible solution or implementation to resolve this issue."
            }
            SlashCommand::Help => {
                "You should respond by listing all available slash commands for GitBot and explaining their purposes. Also offer to help the user understand what GitBot can do for them. Be helpful and welcoming in your response, as the user is trying to understand GitBot's capabilities."
            }
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "summarize" => Some(SlashCommand::Summarize),
            "postmortem" => Some(SlashCommand::Postmortem),
            "suggestions" => Some(SlashCommand::Suggestions),
            "help" => Some(SlashCommand::Help),
            _ => None,
        }
    }
}

// Helper function to parse slash commands from user context
pub(crate) fn parse_slash_command(context: &str) -> Option<(SlashCommand, Option<String>)> {
    let trimmed = context.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let parts: Vec<&str> = trimmed[1..].splitn(2, ' ').collect();
    let command_name = parts[0];
    let additional_context = parts
        .get(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    SlashCommand::from_str(command_name).map(|cmd| (cmd, additional_context))
}

// Helper function to generate help message
pub(crate) fn generate_help_message() -> String {
    format!(
        "Available slash commands:\n\n\
        • `/summarize` - {}\n\
        • `/postmortem` - {}\n\
        • `/suggestions` - {}\n\
        • `/help` - {}\n\n\
        You can add additional context after any command, e.g., `/summarize please focus on security implications`",
        SlashCommand::Summarize.get_precanned_prompt(),
        SlashCommand::Postmortem.get_precanned_prompt(),
        SlashCommand::Suggestions.get_precanned_prompt(),
        SlashCommand::Help.get_precanned_prompt()
    )
}

// Helper function to parse mention timestamp
fn parse_mention_timestamp(timestamp_str: &str) -> Result<DateTime<Utc>> {
    match DateTime::parse_from_rfc3339(timestamp_str) {
        Ok(dt) => Ok(dt.with_timezone(&Utc)),
        Err(e) => {
            error!(
                mention_timestamp = %timestamp_str,
                error = %e,
                "Failed to parse mention timestamp. Cannot reliably check for previous replies. Aborting."
            );
            Err(anyhow!(
                "Failed to parse mention timestamp '{}': {}",
                timestamp_str,
                e
            ))
        }
    }
}

// Helper function to format comments for LLM context
pub(crate) fn format_comments_for_context(
    notes: &[GitlabNoteAttributes],
    max_comment_length: usize,
    current_note_id: i64,
) -> String {
    if notes.is_empty() {
        return "No previous comments found.".to_string();
    }

    let mut formatted_comments = Vec::new();

    for note in notes {
        // Skip the current note that triggered the bot mention
        if note.id == current_note_id {
            continue;
        }

        // Parse the timestamp and format it nicely
        let timestamp = match chrono::DateTime::parse_from_rfc3339(&note.updated_at) {
            Ok(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
            Err(_) => note.updated_at.clone(),
        };

        // Truncate comment content if it's too long
        let content = if note.note.len() > max_comment_length {
            format!("{}... [truncated]", &note.note[..max_comment_length])
        } else {
            note.note.clone()
        };

        formatted_comments.push(format!(
            "**Comment by @{} ({})**:\n{}",
            note.author.username, timestamp, content
        ));
    }

    if formatted_comments.is_empty() {
        "No previous comments found.".to_string()
    } else {
        format!(
            "--- Previous Comments ---\n{}\n--- End of Comments ---",
            formatted_comments.join("\n\n")
        )
    }
}

// Helper function to fetch all comments for an issue
async fn fetch_all_issue_comments(
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
    issue_iid: i64,
) -> Result<Vec<GitlabNoteAttributes>> {
    gitlab_client
        .get_all_issue_notes(project_id, issue_iid)
        .await
        .map_err(|e| anyhow!("Failed to get all issue comments: {}", e))
}

// Helper function to fetch all comments for a merge request
async fn fetch_all_merge_request_comments(
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
    mr_iid: i64,
) -> Result<Vec<GitlabNoteAttributes>> {
    gitlab_client
        .get_all_merge_request_notes(project_id, mr_iid)
        .await
        .map_err(|e| anyhow!("Failed to get all merge request comments: {}", e))
}

// Helper function to fetch subsequent notes based on noteable type
async fn fetch_subsequent_notes_by_type(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    timestamp_u64: u64,
    is_issue: &mut bool,
) -> Result<Vec<crate::models::GitlabNoteAttributes>> {
    let project_id = event.project.id;

    match event.object_attributes.noteable_type.as_str() {
        "Issue" => {
            *is_issue = true;
            let issue_iid = match event.issue.as_ref().map(|i| i.iid) {
                Some(iid) => iid,
                None => {
                    error!(
                        "Missing issue details (iid) in note event for an Issue. Event: {:?}",
                        event
                    );
                    return Err(anyhow!(
                        "Missing issue details in note event for reply check"
                    ));
                }
            };
            info!(
                project_id = project_id,
                issue_iid = issue_iid,
                notes_since_timestamp_u64 = timestamp_u64,
                "Fetching subsequent notes for issue to check for prior bot replies."
            );
            gitlab_client
                .get_issue_notes(project_id, issue_iid, timestamp_u64)
                .await
                .map_err(|e| anyhow!("Failed to get issue notes: {}", e))
        }
        "MergeRequest" => {
            *is_issue = false;
            let mr_iid = match event.merge_request.as_ref().map(|mr| mr.iid) {
                Some(iid) => iid,
                None => {
                    error!("Missing merge request details (iid) in note event for a MergeRequest. Event: {:?}", event);
                    return Err(anyhow!(
                        "Missing merge request details in note event for reply check"
                    ));
                }
            };
            info!(
                project_id = project_id,
                mr_iid = mr_iid,
                notes_since_timestamp_u64 = timestamp_u64,
                "Fetching subsequent notes for merge request to check for prior bot replies."
            );
            gitlab_client
                .get_merge_request_notes(project_id, mr_iid, timestamp_u64)
                .await
                .map_err(|e| anyhow!("Failed to get merge request notes: {}", e))
        }
        other_type => {
            warn!(
                noteable_type = %other_type,
                "Unsupported noteable_type for checking subsequent notes. Skipping reply check."
            );
            Ok(Vec::new()) // Return an empty vec, so the loop below evaluating notes doesn't run
        }
    }
}

// Helper function to check for existing bot replies in notes
async fn check_for_bot_reply_in_notes(
    notes: Vec<crate::models::GitlabNoteAttributes>,
    config: &Arc<AppSettings>,
    mention_timestamp_dt: DateTime<Utc>,
    current_mention_note_id: i64,
    processed_mentions_cache: &MentionCache,
) -> Result<bool> {
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
                    processed_mentions_cache.add(current_mention_note_id).await;
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

// Helper function to check if the bot has already replied to a mention
async fn has_bot_already_replied(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    processed_mentions_cache: &MentionCache,
    is_issue: &mut bool, // This function will set this based on noteable_type
) -> Result<bool> {
    let mention_timestamp_str = &event.object_attributes.updated_at;
    let mention_timestamp_dt = parse_mention_timestamp(mention_timestamp_str)?;

    let notes_since_timestamp_u64 = mention_timestamp_dt.timestamp() as u64;
    let current_mention_note_id = event.object_attributes.id;

    debug!(
        mention_id = current_mention_note_id,
        mention_timestamp = %mention_timestamp_dt,
        notes_since_unix_ts = notes_since_timestamp_u64,
        "Preparing to check for subsequent bot replies."
    );

    match fetch_subsequent_notes_by_type(event, gitlab_client, notes_since_timestamp_u64, is_issue)
        .await
    {
        Ok(notes) => {
            check_for_bot_reply_in_notes(
                notes,
                config,
                mention_timestamp_dt,
                current_mention_note_id,
                processed_mentions_cache,
            )
            .await
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

// Helper function to validate mention and check initial conditions
fn validate_and_check_mention(
    event: &GitlabNoteEvent,
    config: &Arc<AppSettings>,
) -> Result<(Option<String>, bool)> {
    // Verify Object Kind and Event Type
    if event.object_kind != "note" || event.event_type != "note" {
        warn!(
            "Received event with object_kind: '{}' and event_type: '{}'. Expected 'note' for both. Ignoring.",
            event.object_kind, event.event_type
        );
        return Err(anyhow!("Event is not a standard note event"));
    }
    info!("Event object_kind and event_type verified as 'note'.");

    // Extract Note Details and check if bot is mentioned
    let note_content = &event.object_attributes.note;
    let user_provided_context = extract_context_after_mention(note_content, &config.bot_username);

    if user_provided_context.is_none()
        && !note_content.contains(&format!("@{}", config.bot_username))
    {
        info!(
            "Bot @{} was not directly mentioned with a command or the command was empty. Ignoring.",
            config.bot_username
        );
        return Ok((None, false)); // Signal to exit early
    }
    info!("Bot @{} was mentioned.", config.bot_username);

    Ok((user_provided_context, true))
}

// Helper function to build the prompt based on noteable type
async fn build_prompt_for_mention(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    project_id: i64,
    user_provided_context: &Option<String>,
    file_index_manager: &Arc<FileIndexManager>,
) -> Result<(Vec<String>, String)> {
    let mut prompt_parts = Vec::new();
    let mut commit_history = String::new();
    let note_attributes = &event.object_attributes;

    // Prompt Assembly Logic
    match note_attributes.noteable_type.as_str() {
        "Issue" => {
            handle_issue_mention(
                event,
                gitlab_client,
                config,
                project_id,
                &mut prompt_parts,
                user_provided_context,
                file_index_manager,
            )
            .await?
        }
        "MergeRequest" => {
            handle_merge_request_mention(
                event,
                gitlab_client,
                config,
                project_id,
                &mut prompt_parts,
                &mut commit_history,
                user_provided_context,
                file_index_manager,
            )
            .await?
        }
        other_type => {
            info!(
                "Note on unsupported noteable_type: {}, ignoring.",
                other_type
            );
            return Err(anyhow!("Unsupported noteable_type: {}", other_type));
        }
    };

    Ok((prompt_parts, commit_history))
}

// Helper struct for reply generation parameters
struct ReplyContext<'a> {
    prompt_parts: Vec<String>,
    commit_history: String,
    user_provided_context: &'a Option<String>,
    is_issue: bool,
}

// Helper function to generate and post reply
async fn generate_and_post_reply(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    project_id: i64,
    reply_context: ReplyContext<'_>,
    file_index_manager: &Arc<FileIndexManager>,
) -> Result<()> {
    let final_prompt_text = format!(
        "{}\n\nContext:\n{}",
        reply_context.prompt_parts.join("\n---\n"),
        reply_context.commit_history
    );
    trace!("Formatted prompt for LLM:\n{}", final_prompt_text);
    trace!("Full prompt for LLM (debug):\n{}", final_prompt_text);

    // Create OpenAI client
    let openai_client = OpenAIApiClient::new(config)
        .map_err(|e| anyhow!("Failed to create OpenAI client: {}", e))?;

    let llm_reply = get_llm_reply_with_tools(
        &openai_client,
        config,
        &final_prompt_text,
        gitlab_client,
        project_id,
        file_index_manager,
    ).await?;

    // Format final comment
    let final_comment_body = format_final_reply_body(
        &event.user.username,
        &llm_reply,
        reply_context.is_issue,
        reply_context.user_provided_context,
        &reply_context.commit_history,
    );

    // Post the comment
    post_reply_to_gitlab(
        event,
        gitlab_client,
        project_id,
        reply_context.is_issue,
        &final_comment_body,
    )
    .await
}

pub async fn process_mention(
    event: GitlabNoteEvent,
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
    processed_mentions_cache: &MentionCache, // Changed type
    file_index_manager: Arc<FileIndexManager>,
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
        info!("Mention ID {} found in cache, skipping.", mention_id);
        return Ok(());
    }

    // Initialize variables at the top level
    let project_id = event.project.id;
    let mut is_issue = false;

    // Validate and check mention
    let (user_provided_context, should_continue) = validate_and_check_mention(&event, &config)?;
    if !should_continue {
        return Ok(()); // Early return if bot not mentioned properly
    }

    // Check if already replied
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

    // Build prompt
    let (prompt_parts, commit_history) = build_prompt_for_mention(
        &event,
        &gitlab_client,
        &config,
        project_id,
        &user_provided_context,
        &file_index_manager,
    )
    .await?;

    // Generate and post reply
    let reply_context = ReplyContext {
        prompt_parts,
        commit_history,
        user_provided_context: &user_provided_context,
        is_issue,
    };

    generate_and_post_reply(&event, &gitlab_client, &config, project_id, reply_context, &file_index_manager).await?;

    // Add to cache after successful processing
    processed_mentions_cache.add(mention_id).await;
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
        content: Some(prompt_text.to_string()),
        tool_calls: None,
        tool_call_id: None,
    }];
    let openai_request = OpenAIChatRequest {
        model: config.openai_model.clone(),
        messages,
        temperature: Some(config.openai_temperature),
        max_tokens: Some(config.openai_max_tokens),
        tools: None,
        tool_choice: None,
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
            if let Some(content) = &choice.message.content {
                if content.is_empty() {
                    Err(anyhow!("LLM response content is empty"))
                } else {
                    Ok(content.clone())
                }
            } else {
                Err(anyhow!("LLM response has no content"))
            }
        })
}

// Tool function definitions for LLM tool use
fn create_repository_tools() -> Vec<OpenAITool> {
    vec![
        OpenAITool {
            tool_type: "function".to_string(),
            function: OpenAIFunction {
                name: "get_file_content".to_string(),
                description: "Get the full content of a specific file from the repository. Use this when you need to examine the complete implementation of a file.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "The path to the file in the repository (e.g., 'src/main.rs', 'README.md')"
                        }
                    },
                    "required": ["file_path"]
                }),
            },
        },
        OpenAITool {
            tool_type: "function".to_string(),
            function: OpenAIFunction {
                name: "get_file_lines".to_string(),
                description: "Get specific line ranges from a file. Use this when you need to examine specific sections of a file rather than the entire content.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "The path to the file in the repository"
                        },
                        "start_line": {
                            "type": "integer",
                            "description": "The starting line number (1-based)"
                        },
                        "end_line": {
                            "type": "integer",
                            "description": "The ending line number (1-based, inclusive)"
                        }
                    },
                    "required": ["file_path", "start_line", "end_line"]
                }),
            },
        },
        OpenAITool {
            tool_type: "function".to_string(),
            function: OpenAIFunction {
                name: "search_repository_files".to_string(),
                description: "Search for files in the repository by keywords. Use this to find relevant files when you don't know their exact paths.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "keywords": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "Keywords to search for in file names and content"
                        },
                        "limit": {
                            "type": "integer",
                            "default": 5,
                            "description": "Maximum number of files to return (default: 5, max: 10)"
                        }
                    },
                    "required": ["keywords"]
                }),
            },
        },
    ]
}

// Tool execution functions
pub async fn execute_tool_call(
    tool_call: &OpenAIToolCall,
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
    file_index_manager: &Arc<FileIndexManager>,
) -> Result<String> {
    let function_args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
        .map_err(|e| anyhow!("Failed to parse tool arguments: {}", e))?;

    match tool_call.function.name.as_str() {
        "get_file_content" => {
            let file_path = function_args["file_path"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing file_path parameter"))?;
            
            match gitlab_client.get_file_content(project_id, file_path).await {
                Ok(file) => {
                    if let Some(content) = file.content {
                        Ok(format!("Content of {}:\n\n{}", file_path, content))
                    } else {
                        Ok(format!("File {} exists but has no content", file_path))
                    }
                }
                Err(e) => Ok(format!("Error accessing file {}: {}", file_path, e)),
            }
        }
        "get_file_lines" => {
            let file_path = function_args["file_path"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing file_path parameter"))?;
            let start_line = function_args["start_line"]
                .as_i64()
                .ok_or_else(|| anyhow!("Missing start_line parameter"))? as usize;
            let end_line = function_args["end_line"]
                .as_i64()
                .ok_or_else(|| anyhow!("Missing end_line parameter"))? as usize;

            if start_line == 0 {
                return Ok("Error: Line numbers must be 1-based (start from 1)".to_string());
            }

            match gitlab_client.get_file_content(project_id, file_path).await {
                Ok(file) => {
                    if let Some(content) = file.content {
                        let lines: Vec<&str> = content.lines().collect();
                        if start_line > lines.len() || end_line > lines.len() || start_line > end_line {
                            Ok(format!("Error: Invalid line range {}-{} for file {} (file has {} lines)", 
                                start_line, end_line, file_path, lines.len()))
                        } else {
                            let selected_lines = &lines[(start_line - 1)..end_line];
                            let result = selected_lines
                                .iter()
                                .enumerate()
                                .map(|(i, line)| format!("{}: {}", start_line + i, line))
                                .collect::<Vec<_>>()
                                .join("\n");
                            Ok(format!("Lines {}-{} of {}:\n\n{}", start_line, end_line, file_path, result))
                        }
                    } else {
                        Ok(format!("File {} exists but has no content", file_path))
                    }
                }
                Err(e) => Ok(format!("Error accessing file {}: {}", file_path, e)),
            }
        }
        "search_repository_files" => {
            let keywords_array = function_args["keywords"]
                .as_array()
                .ok_or_else(|| anyhow!("Missing keywords parameter"))?;
            let keywords: Vec<String> = keywords_array
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            
            if keywords.is_empty() {
                return Ok("Error: At least one keyword is required for search".to_string());
            }

            let limit = function_args["limit"]
                .as_i64()
                .unwrap_or(5)
                .min(10) as usize;

            match file_index_manager.search_files(project_id, &keywords).await {
                Ok(files) => {
                    if files.is_empty() {
                        Ok(format!("No files found matching keywords: {:?}", keywords))
                    } else {
                        let limited_files = files.into_iter().take(limit).collect::<Vec<_>>();
                        let result = limited_files
                            .iter()
                            .map(|f| format!("- {} (size: {} bytes)", f.file_path, f.size))
                            .collect::<Vec<_>>()
                            .join("\n");
                        Ok(format!("Found {} files matching keywords {:?}:\n\n{}", 
                            limited_files.len(), keywords, result))
                    }
                }
                Err(e) => Ok(format!("Error searching files: {}", e)),
            }
        }
        _ => Ok(format!("Unknown tool function: {}", tool_call.function.name)),
    }
}

// New function for LLM reply with tool use capability
async fn get_llm_reply_with_tools(
    openai_client: &OpenAIApiClient,
    config: &Arc<AppSettings>,
    prompt_text: &str,
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
    file_index_manager: &Arc<FileIndexManager>,
) -> Result<String> {
    let mut messages = vec![OpenAIChatMessage {
        role: "user".to_string(),
        content: Some(prompt_text.to_string()),
        tool_calls: None,
        tool_call_id: None,
    }];

    let tools = create_repository_tools();
    let max_iterations = 2; // As requested in the issue: 1-2 turns

    for iteration in 0..max_iterations {
        debug!("Tool use iteration {} of {}", iteration + 1, max_iterations);

        let openai_request = OpenAIChatRequest {
            model: config.openai_model.clone(),
            messages: messages.clone(),
            temperature: Some(config.openai_temperature),
            max_tokens: Some(config.openai_max_tokens),
            tools: Some(tools.clone()),
            tool_choice: Some("auto".to_string()),
        };

        let openai_response = openai_client
            .send_chat_completion(&openai_request)
            .await
            .map_err(|e| {
                error!("Failed to communicate with OpenAI: {}", e);
                anyhow!("Failed to communicate with OpenAI: {}", e)
            })?;

        debug!("OpenAI response iteration {}: {:?}", iteration + 1, openai_response);

        let choice = openai_response.choices.first()
            .ok_or_else(|| anyhow!("No response choices from OpenAI"))?;

        // Add the assistant's response to the conversation
        messages.push(choice.message.clone());

        // Check if the assistant wants to call tools
        if let Some(tool_calls) = &choice.message.tool_calls {
            debug!("Assistant requested {} tool calls", tool_calls.len());
            
            // Execute each tool call
            for tool_call in tool_calls {
                let tool_result = execute_tool_call(
                    tool_call,
                    gitlab_client,
                    project_id,
                    file_index_manager,
                ).await?;

                // Add tool response to conversation
                messages.push(OpenAIChatMessage {
                    role: "tool".to_string(),
                    content: Some(tool_result),
                    tool_calls: None,
                    tool_call_id: Some(tool_call.id.clone()),
                });
            }
            // Continue to next iteration to get final response
        } else {
            // No tool calls, return the content
            if let Some(content) = &choice.message.content {
                if !content.is_empty() {
                    return Ok(content.clone());
                }
            }
            return Err(anyhow!("LLM response has no content and no tool calls"));
        }
    }

    // After max iterations, make one final call without tools to get the answer
    debug!("Max iterations reached, making final call without tools");
    
    let final_request = OpenAIChatRequest {
        model: config.openai_model.clone(),
        messages,
        temperature: Some(config.openai_temperature),
        max_tokens: Some(config.openai_max_tokens),
        tools: None,
        tool_choice: None,
    };

    let final_response = openai_client
        .send_chat_completion(&final_request)
        .await
        .map_err(|e| {
            error!("Failed to communicate with OpenAI on final call: {}", e);
            anyhow!("Failed to communicate with OpenAI: {}", e)
        })?;

    let final_choice = final_response.choices.first()
        .ok_or_else(|| anyhow!("No response choices from OpenAI on final call"))?;

    if let Some(content) = &final_choice.message.content {
        if !content.is_empty() {
            Ok(content.clone())
        } else {
            Err(anyhow!("Final LLM response content is empty"))
        }
    } else {
        Err(anyhow!("Final LLM response has no content"))
    }
}

// Helper function to extract issue details and handle stale label removal
async fn extract_issue_details_and_handle_stale(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    project_id: i64,
) -> Result<(i64, crate::models::GitlabIssue)> {
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

    Ok((issue_iid, issue))
}

// Helper function to add repository context to prompt
async fn add_repository_context_to_prompt(
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    file_index_manager: &Arc<FileIndexManager>,
    issue: &crate::models::GitlabIssue,
    project: &crate::models::GitlabProject,
    prompt_parts: &mut Vec<String>,
) {
    let repo_context_extractor = RepoContextExtractor::new_with_file_indexer(
        gitlab_client.clone(),
        config.clone(),
        file_index_manager.clone(),
    );

    match repo_context_extractor
        .extract_context_for_issue(issue, project, config.context_repo_path.as_deref())
        .await
    {
        Ok(context_str) => {
            let enhanced_context = format!(
                "Repository Context (files are ranked by relevance based on keyword frequency - higher percentages indicate more relevant content): {}",
                context_str
            );
            prompt_parts.push(enhanced_context);
        }
        Err(e) => {
            // This should now only happen in catastrophic failures
            warn!(
                "Failed to extract repository context: {}. This is a critical error.",
                e
            );
        }
    }
}

// Helper struct for issue prompt building context
struct IssuePromptContext<'a> {
    event: &'a GitlabNoteEvent,
    gitlab_client: &'a Arc<GitlabApiClient>,
    config: &'a Arc<AppSettings>,
    project_id: i64,
    issue_iid: i64,
    issue: &'a crate::models::GitlabIssue,
    file_index_manager: &'a Arc<FileIndexManager>,
}

// Helper function to build issue prompt with user-provided context
async fn build_issue_prompt_with_context(
    context: IssuePromptContext<'_>,
    user_context: &str,
    prompt_parts: &mut Vec<String>,
) -> Result<()> {
    // Check for slash commands
    if let Some((slash_command, additional_context)) = parse_slash_command(user_context) {
        // Use precanned prompt for all slash commands, including Help
        let precanned_prompt = slash_command.get_precanned_prompt();
        if let Some(extra_context) = additional_context {
            prompt_parts.push(format!(
                "The user @{} requested: '{}' with additional context: '{}'.",
                context.event.user.username, precanned_prompt, extra_context
            ));
        } else {
            prompt_parts.push(format!(
                "The user @{} requested: '{}'.",
                context.event.user.username, precanned_prompt
            ));
        }

        // For help command, provide information about available commands
        if matches!(slash_command, SlashCommand::Help) {
            prompt_parts.push(format!(
                "Available slash commands and their purposes:\n{}",
                generate_help_message()
            ));
        }
    } else {
        // Original behavior for non-slash commands
        prompt_parts.push(format!(
            "The user @{} provided the following request regarding this issue: '{}'.",
            context.event.user.username, user_context
        ));
    }

    let issue_details = context
        .gitlab_client
        .get_issue(context.project_id, context.issue_iid)
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

    prompt_parts.push(format!("State: {}", context.issue.state));
    if !context.issue.labels.is_empty() {
        prompt_parts.push(format!("Labels: {}", context.issue.labels.join(", ")));
    }

    // Add repository context
    add_repository_context_to_prompt(
        context.gitlab_client,
        context.config,
        context.file_index_manager,
        context.issue,
        &context.event.project,
        prompt_parts,
    )
    .await;

    // Add comments context
    match fetch_all_issue_comments(context.gitlab_client, context.project_id, context.issue_iid)
        .await
    {
        Ok(comments) => {
            let formatted_comments = format_comments_for_context(
                &comments,
                context.config.max_comment_length,
                context.event.object_attributes.id,
            );
            prompt_parts.push(formatted_comments);
        }
        Err(e) => {
            warn!("Failed to fetch issue comments for context: {}", e);
            prompt_parts.push("Previous comments could not be retrieved.".to_string());
        }
    }

    prompt_parts.push(format!("User's specific request: {}", user_context));

    Ok(())
}

// Helper function to build issue prompt without user context (default summarization)
async fn build_issue_prompt_without_context(
    context: IssuePromptContext<'_>,
    prompt_parts: &mut Vec<String>,
) {
    // No specific context, summarize and suggest steps
    prompt_parts.push(format!(
        "Please summarize this issue for user @{} and suggest steps to address it. Be specific about which files, functions, or modules need to be modified.",
        context.event.user.username
    ));
    prompt_parts.push(format!("Issue Title: {}", context.issue.title));
    prompt_parts.push(format!(
        "Issue Description: {}",
        context
            .issue
            .description
            .as_deref()
            .unwrap_or("No description.")
    ));
    prompt_parts.push(format!("Author: {}", context.issue.author.name));
    prompt_parts.push(format!("State: {}", context.issue.state));
    if !context.issue.labels.is_empty() {
        prompt_parts.push(format!("Labels: {}", context.issue.labels.join(", ")));
    }

    // Add repository context
    add_repository_context_to_prompt(
        context.gitlab_client,
        context.config,
        context.file_index_manager,
        context.issue,
        &context.event.project,
        prompt_parts,
    )
    .await;

    // Add comments context
    match fetch_all_issue_comments(context.gitlab_client, context.project_id, context.issue_iid)
        .await
    {
        Ok(comments) => {
            let formatted_comments = format_comments_for_context(
                &comments,
                context.config.max_comment_length,
                context.event.object_attributes.id,
            );
            prompt_parts.push(formatted_comments);
        }
        Err(e) => {
            warn!("Failed to fetch issue comments for context: {}", e);
            prompt_parts.push("Previous comments could not be retrieved.".to_string());
        }
    }

    // Add instructions for steps
    prompt_parts.push(
        String::from("Please provide a summary of the issue and suggest specific steps to")
            + "address it based on the repository context. Again, be specific about"
            + "which files, functions, or modules need to be modified.",
    );
}

async fn handle_issue_mention(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    project_id: i64,
    prompt_parts: &mut Vec<String>,
    user_provided_context: &Option<String>,
    file_index_manager: &Arc<FileIndexManager>,
) -> Result<()> {
    let (issue_iid, issue) =
        extract_issue_details_and_handle_stale(event, gitlab_client, config, project_id).await?;

    let context = IssuePromptContext {
        event,
        gitlab_client,
        config,
        project_id,
        issue_iid,
        issue: &issue,
        file_index_manager,
    };

    if let Some(user_context) = user_provided_context {
        build_issue_prompt_with_context(context, user_context, prompt_parts).await?;
    } else {
        build_issue_prompt_without_context(context, prompt_parts).await;
    }

    Ok(())
}

// Helper function to extract merge request details
async fn extract_merge_request_details(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
) -> Result<(i64, crate::models::GitlabMergeRequest)> {
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

    Ok((mr_iid, mr))
}

// Helper function to fetch CONTRIBUTING.md content
async fn fetch_contributing_guidelines(
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
) -> Option<String> {
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
                    Some(content)
                } else {
                    info!(
                        "CONTRIBUTING.md is empty for project ID {}. Proceeding without it.",
                        project_id
                    );
                    None
                }
            } else {
                // This case might occur if the API returns a success status but no content field,
                // or if get_file_content is changed to return Ok(GitlabFile { content: None }) on 404.
                info!(
                    "CONTRIBUTING.md has no content or content was null for project ID {}. Proceeding without it.",
                    project_id
                );
                None
            }
        }
        Err(e) => match e {
            GitlabError::Api { status, .. } if status == reqwest::StatusCode::NOT_FOUND => {
                info!(
                    "CONTRIBUTING.md not found (404) for project ID {}. Proceeding without it.",
                    project_id
                );
                None
            }
            _ => {
                warn!(
                    "Failed to fetch CONTRIBUTING.md for project ID {}: {:?}. Proceeding without it.",
                    project_id, e
                );
                None
            }
        },
    }
}

// Helper function to add MR context to prompt
async fn add_mr_context_to_prompt(
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    file_index_manager: &Arc<FileIndexManager>,
    mr: &crate::models::GitlabMergeRequest,
    project: &crate::models::GitlabProject,
    prompt_parts: &mut Vec<String>,
    commit_history: &mut String,
) {
    let repo_context_extractor = RepoContextExtractor::new_with_file_indexer(
        gitlab_client.clone(),
        config.clone(),
        file_index_manager.clone(),
    );

    match repo_context_extractor
        .extract_context_for_mr(mr, project, config.context_repo_path.as_deref())
        .await
    {
        Ok((context_for_llm, context_for_comment)) => {
            let enhanced_context = format!(
                "Code Changes (files are ranked by relevance based on keyword frequency - higher percentages indicate more relevant content): {}",
                context_for_llm
            );
            prompt_parts.push(enhanced_context);
            *commit_history = context_for_comment; // Update commit_history
        }
        Err(e) => {
            warn!("Failed to extract merge request diff context: {}", e);
        }
    }
}

// Helper struct for MR prompt building context
struct MrPromptContext<'a> {
    event: &'a GitlabNoteEvent,
    gitlab_client: &'a Arc<GitlabApiClient>,
    config: &'a Arc<AppSettings>,
    mr: &'a crate::models::GitlabMergeRequest,
    file_index_manager: &'a Arc<FileIndexManager>,
}

// Helper function to build MR prompt with user-provided context
async fn build_mr_prompt_with_context(
    context: MrPromptContext<'_>,
    user_context: &str,
    prompt_parts: &mut Vec<String>,
    commit_history: &mut String,
) {
    // Check for slash commands
    if let Some((slash_command, additional_context)) = parse_slash_command(user_context) {
        // Use precanned prompt for all slash commands, including Help
        let precanned_prompt = slash_command.get_precanned_prompt();
        if let Some(extra_context) = additional_context {
            prompt_parts.push(format!(
                "The user @{} requested: '{}' with additional context: '{}'.",
                context.event.user.username, precanned_prompt, extra_context
            ));
        } else {
            prompt_parts.push(format!(
                "The user @{} requested: '{}'.",
                context.event.user.username, precanned_prompt
            ));
        }

        // For help command, provide information about available commands
        if matches!(slash_command, SlashCommand::Help) {
            prompt_parts.push(format!(
                "Available slash commands and their purposes:\n{}",
                generate_help_message()
            ));
        }
    } else {
        // Original behavior for non-slash commands
        prompt_parts.push(format!(
            "The user @{} provided the following request regarding this merge request: '{}'.",
            context.event.user.username, user_context
        ));
    }

    prompt_parts.push(format!("Title: {}", context.mr.title));
    prompt_parts.push(format!(
        "Description: {}",
        context.mr.description.as_deref().unwrap_or("N/A")
    ));
    prompt_parts.push(format!("State: {}", context.mr.state));
    if !context.mr.labels.is_empty() {
        prompt_parts.push(format!("Labels: {}", context.mr.labels.join(", ")));
    }
    prompt_parts.push(format!("Source Branch: {}", context.mr.source_branch));
    prompt_parts.push(format!("Target Branch: {}", context.mr.target_branch));

    // Add code diff context
    add_mr_context_to_prompt(
        context.gitlab_client,
        context.config,
        context.file_index_manager,
        context.mr,
        &context.event.project,
        prompt_parts,
        commit_history,
    )
    .await;

    // Add comments context
    match fetch_all_merge_request_comments(
        context.gitlab_client,
        context.event.project.id,
        context.mr.iid,
    )
    .await
    {
        Ok(comments) => {
            let formatted_comments = format_comments_for_context(
                &comments,
                context.config.max_comment_length,
                context.event.object_attributes.id,
            );
            prompt_parts.push(formatted_comments);
        }
        Err(e) => {
            warn!("Failed to fetch merge request comments for context: {}", e);
            prompt_parts.push("Previous comments could not be retrieved.".to_string());
        }
    }

    prompt_parts.push(format!("User's specific request: {}", user_context));
}

// Helper function to build MR prompt without user context (default review)
async fn build_mr_prompt_without_context(
    context: MrPromptContext<'_>,
    contributing_md_content: Option<String>,
    prompt_parts: &mut Vec<String>,
    commit_history: &mut String,
) {
    // No specific context, summarize with code diffs
    prompt_parts.push(format!(
        "Please review this merge request for user @{} and provide a summary of the changes.",
        context.event.user.username
    ));
    prompt_parts.push(format!("Merge Request Title: {}", context.mr.title));
    prompt_parts.push(format!(
        "Merge Request Description: {}",
        context
            .mr
            .description
            .as_deref()
            .unwrap_or("No description.")
    ));
    prompt_parts.push(format!("Author: {}", context.mr.author.name));
    prompt_parts.push(format!("State: {}", context.mr.state));
    if !context.mr.labels.is_empty() {
        prompt_parts.push(format!("Labels: {}", context.mr.labels.join(", ")));
    }
    prompt_parts.push(format!("Source Branch: {}", context.mr.source_branch));
    prompt_parts.push(format!("Target Branch: {}", context.mr.target_branch));

    // Add code diff context
    add_mr_context_to_prompt(
        context.gitlab_client,
        context.config,
        context.file_index_manager,
        context.mr,
        &context.event.project,
        prompt_parts,
        commit_history,
    )
    .await;

    // Add comments context
    match fetch_all_merge_request_comments(
        context.gitlab_client,
        context.event.project.id,
        context.mr.iid,
    )
    .await
    {
        Ok(comments) => {
            let formatted_comments = format_comments_for_context(
                &comments,
                context.config.max_comment_length,
                context.event.object_attributes.id,
            );
            prompt_parts.push(formatted_comments);
        }
        Err(e) => {
            warn!("Failed to fetch merge request comments for context: {}", e);
            prompt_parts.push("Previous comments could not be retrieved.".to_string());
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

#[allow(clippy::too_many_arguments)]
async fn handle_merge_request_mention(
    event: &GitlabNoteEvent,
    gitlab_client: &Arc<GitlabApiClient>,
    config: &Arc<AppSettings>,
    project_id: i64,
    prompt_parts: &mut Vec<String>,
    commit_history: &mut String, // Changed to mutable reference
    user_provided_context: &Option<String>,
    file_index_manager: &Arc<FileIndexManager>,
) -> Result<()> {
    let (_mr_iid, mr) = extract_merge_request_details(event, gitlab_client, project_id).await?;

    let contributing_md_content = fetch_contributing_guidelines(gitlab_client, project_id).await;

    let context = MrPromptContext {
        event,
        gitlab_client,
        config,
        mr: &mr,
        file_index_manager,
    };

    if let Some(user_context) = user_provided_context {
        build_mr_prompt_with_context(context, user_context, prompt_parts, commit_history).await;
    } else {
        build_mr_prompt_without_context(
            context,
            contributing_md_content,
            prompt_parts,
            commit_history,
        )
        .await;
    }

    Ok(())
}
