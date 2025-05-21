use actix_web::{web, HttpRequest, HttpResponse, Responder};
use serde_json; 
use tracing::{debug, error, info, warn};
use crate::config::AppSettings;
use crate::models::{GitlabNoteEvent, OpenAIChatMessage, OpenAIChatRequest};
use crate::gitlab::GitlabApiClient;
use crate::openai::OpenAIApiClient;

// hmac, Sha256 and hex are not used yet, but added for future reference as per instructions
#[allow(unused_imports)]
use hmac::{Hmac, Mac};
#[allow(unused_imports)]
use sha2::Sha256;
#[allow(unused_imports)]
use hex;

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

pub async fn gitlab_webhook_handler(
    req: HttpRequest,
    payload: web::Bytes,
    config: web::Data<AppSettings>,
    gitlab_client: web::Data<GitlabApiClient>, 
    openai_client: web::Data<OpenAIApiClient>,
) -> impl Responder {
    info!(
        "Received webhook request: {} {} from {}",
        req.method(),
        req.uri(),
        req.peer_addr().map_or_else(|| "unknown".to_string(), |addr| addr.to_string())
    );

    debug!("Request headers:");
    for (name, value) in req.headers().iter() {
        debug!("  {}: {:?}", name, value.to_str().unwrap_or("<non-utf8>"));
    }

    // Secret Token Verification
    let provided_token = match req.headers().get("X-Gitlab-Token") {
        Some(token) => match token.to_str() {
            Ok(t) => t,
            Err(_) => {
                error!("X-Gitlab-Token header contains non-UTF8 characters");
                return HttpResponse::Unauthorized().body("Invalid X-Gitlab-Token header");
            }
        },
        None => {
            error!("Missing X-Gitlab-Token header");
            return HttpResponse::Unauthorized().body("Missing X-Gitlab-Token header");
        }
    };

    if provided_token != config.gitlab_webhook_secret {
        error!(
            "Invalid X-Gitlab-Token. Expected: '{}', Received: '{}'",
            config.gitlab_webhook_secret, provided_token
        );
        return HttpResponse::Unauthorized().body("Invalid X-Gitlab-Token");
    }
    info!("X-Gitlab-Token verified successfully.");

    // Deserialize Payload
    let event: GitlabNoteEvent = match serde_json::from_slice(&payload) {
        Ok(event_data) => {
            info!("Successfully deserialized GitlabNoteEvent.");
            if let Ok(pretty_json) = serde_json::to_string_pretty(&event_data) {
                debug!("Deserialized event payload (JSON):\n{}", pretty_json);
            } else {
                debug!("Deserialized event payload (JSON, unformatted): {:?}", event_data);
            }
            event_data
        }
        Err(e) => {
            error!("Failed to deserialize GitlabNoteEvent: {}. Payload: {:?}", e, String::from_utf8_lossy(&payload));
            return HttpResponse::BadRequest().body("Invalid payload format for GitlabNoteEvent");
        }
    };

    // Log Event Details
    info!(
        "Received GitLab note event from user: {} in project: {}",
        event.user.username, event.project.path_with_namespace
    );
    
    // Self-Mention Check (using bot_username from config)
    if event.user.username == config.bot_username {
        info!("Comment is from the bot itself (@{}), ignoring.", config.bot_username);
        return HttpResponse::Ok().finish(); // Changed to finish() for empty body
    }

    // Whitelist Check
    if !config.whitelisted_repos.contains(&event.project.path_with_namespace) {
        info!(
            "Project {} is not in the whitelisted_repos. Ignoring event.",
            event.project.path_with_namespace
        );
        return HttpResponse::Ok().body("Project not whitelisted");
    }
    info!("Project {} is whitelisted.", event.project.path_with_namespace);
    
    // Verify Object Kind and Event Type
    if event.object_kind != "note" || event.event_type != "note" {
        warn!(
            "Received event with object_kind: '{}' and event_type: '{}'. Expected 'note' for both. Ignoring.",
            event.object_kind, event.event_type
        );
        return HttpResponse::BadRequest().body("Event is not a standard note event.");
    }
    info!("Event object_kind and event_type verified as 'note'.");

    // Extract Note Details
    let note_attributes = &event.object_attributes;
    let note_content = &note_attributes.note;
    
    // Check if bot is mentioned
    let user_provided_context = extract_context_after_mention(note_content, &config.bot_username);

    if user_provided_context.is_none() && !note_content.contains(&format!("@{}", config.bot_username)) {
        info!("Bot @{} was not directly mentioned with a command or the command was empty. Ignoring.", config.bot_username);
        return HttpResponse::Ok().body("Bot not mentioned or command empty");
    }
    info!("Bot @{} was mentioned.", config.bot_username);


    // Prompt Assembly Logic
    let mut prompt_parts: Vec<String> = Vec::new();
    let llm_task_description: String;
    
    let project_id = event.project.id;
    let is_issue: bool;


    match note_attributes.noteable_type.as_str() {
        "Issue" => {
            is_issue = true;
            let issue_iid = match event.issue.as_ref().map(|i| i.iid) {
                Some(iid) => iid,
                None => {
                    error!("Missing issue details (iid) in note event for an Issue. Payload: {:?}", event);
                    return HttpResponse::BadRequest().body("Missing issue details in note event");
                }
            };
            info!("Note event pertains to Issue #{} in project ID {}.", issue_iid, project_id);

            if let Some(context) = &user_provided_context {
                llm_task_description = format!("The user @{} provided the following request regarding this issue: '{}'.", event.user.username, context);
                let issue_details = match gitlab_client.get_issue(project_id, issue_iid).await {
                    Ok(details) => details,
                    Err(e) => {
                        error!("Failed to get issue details for context: {}", e);
                        return HttpResponse::InternalServerError().body("Failed to fetch issue details from GitLab.");
                    }
                };
                prompt_parts.push(format!("Title: {}", issue_details.title));
                prompt_parts.push(format!("Description: {}", issue_details.description.as_deref().unwrap_or("N/A")));
                prompt_parts.push(format!("User's specific request: {}", context));
            } else { // No specific context, summarize
                llm_task_description = format!("Please summarize this issue for user @{}.", event.user.username);
                let issue = match gitlab_client.get_issue(project_id, issue_iid).await {
                    Ok(details) => details,
                    Err(e) => {
                        error!("Failed to get issue details for summary: {}", e);
                        return HttpResponse::InternalServerError().body("Failed to fetch issue details from GitLab.");
                    }
                };
                prompt_parts.push(format!("Issue Title: {}", issue.title));
                prompt_parts.push(format!("Issue Description: {}", issue.description.as_deref().unwrap_or("No description.")));
                prompt_parts.push(format!("Author: {}", issue.author.name));
                prompt_parts.push(format!("State: {}", issue.state));
                if !issue.labels.is_empty() { prompt_parts.push(format!("Labels: {}", issue.labels.join(", "))); }
            }
        }
        "MergeRequest" => {
            is_issue = false;
            let mr_iid = match event.merge_request.as_ref().map(|mr| mr.iid) {
                Some(iid) => iid,
                None => {
                    error!("Missing merge request details (iid) in note event for a MergeRequest. Payload: {:?}", event);
                    return HttpResponse::BadRequest().body("Missing merge request details in note event");
                }
            };
            info!("Note event pertains to Merge Request !{} in project ID {}.", mr_iid, project_id);

            if let Some(context) = &user_provided_context {
                llm_task_description = format!("The user @{} provided the following request regarding this merge request: '{}'.", event.user.username, context);
                let mr_details = match gitlab_client.get_merge_request(project_id, mr_iid).await {
                    Ok(details) => details,
                    Err(e) => {
                        error!("Failed to get MR details for context: {}", e);
                        return HttpResponse::InternalServerError().body("Failed to fetch MR details from GitLab.");
                    }
                };
                prompt_parts.push(format!("Title: {}", mr_details.title));
                prompt_parts.push(format!("Description: {}", mr_details.description.as_deref().unwrap_or("N/A")));
                prompt_parts.push(format!("User's specific request: {}", context));
            } else { // No specific context, summarize
                llm_task_description = format!("Please summarize this merge request for user @{}.", event.user.username);
                let mr = match gitlab_client.get_merge_request(project_id, mr_iid).await {
                     Ok(details) => details,
                    Err(e) => {
                        error!("Failed to get MR details for summary: {}", e);
                        return HttpResponse::InternalServerError().body("Failed to fetch MR details from GitLab.");
                    }
                };
                prompt_parts.push(format!("Merge Request Title: {}", mr.title));
                prompt_parts.push(format!("Merge Request Description: {}", mr.description.as_deref().unwrap_or("No description.")));
                prompt_parts.push(format!("Author: {}", mr.author.name));
                prompt_parts.push(format!("State: {}", mr.state));
                if !mr.labels.is_empty() { prompt_parts.push(format!("Labels: {}", mr.labels.join(", "))); }
                prompt_parts.push(format!("Source Branch: {}", mr.source_branch));
                prompt_parts.push(format!("Target Branch: {}", mr.target_branch));
            }
        }
        other_type => {
            info!("Note on unsupported noteable_type: {}, ignoring.", other_type);
            return HttpResponse::Ok().body("Unsupported noteable type");
        }
    };

    let item_type = if is_issue { "issue" } else { "merge request" };
    if user_provided_context.is_none() {
         prompt_parts.insert(0, format!("The user @{} wants a summary of this {}.", event.user.username, item_type));
    }


    let final_prompt_text = format!("{}\n\nContext:\n{}", llm_task_description, prompt_parts.join("\n---\n"));
    info!("Formatted prompt for LLM:\n{}", final_prompt_text);
    debug!("Full prompt for LLM (debug):\n{}", final_prompt_text);


    // Call OpenAI Client
    let messages = vec![OpenAIChatMessage { role: "user".to_string(), content: final_prompt_text }];
    let openai_request = OpenAIChatRequest { 
        model: "gpt-3.5-turbo".to_string(), // TODO: Make configurable
        messages,
        temperature: Some(0.7), // TODO: Make configurable
        max_tokens: Some(1024)  // TODO: Make configurable
    };

    let openai_response = match openai_client.send_chat_completion(&openai_request).await {
        Ok(response) => response,
        Err(e) => {
            error!("Failed to communicate with OpenAI: {}", e);
            return HttpResponse::InternalServerError().body("Failed to communicate with OpenAI");
        }
    };
    debug!("OpenAI response: {:?}", openai_response);

    // Extract LLM's Reply
    let llm_reply = openai_response.choices.get(0)
        .map(|choice| choice.message.content.clone())
        .unwrap_or_else(|| "Sorry, I couldn't get a valid response from the LLM.".to_string());
    
    info!("LLM Reply: {}", llm_reply);

    let user_who_triggered = &event.user.username;
    let final_comment_body = format!("Hey @{}, here's the information you requested:\n\n---\n\n{}", user_who_triggered, llm_reply);

    // Post the comment back to GitLab
    if is_issue {
        let issue_iid = match event.issue.as_ref().map(|i| i.iid) {
            Some(iid) => iid,
            None => {
                // This should ideally not happen if we reached here, but as a safeguard:
                error!("Critical: Missing issue_iid when trying to post comment. Event: {:?}", event);
                return HttpResponse::InternalServerError().body("Internal error: Missing issue context for comment posting.");
            }
        };
        match gitlab_client.post_comment_to_issue(project_id, issue_iid, &final_comment_body).await {
            Ok(_note) => {
                info!("Successfully posted comment to issue {}/{}", project_id, issue_iid);
                HttpResponse::Ok().body("Successfully processed mention and posted reply to issue.")
            }
            Err(e) => {
                error!("Failed to post comment to issue {}/{}: {}", project_id, issue_iid, e);
                HttpResponse::InternalServerError().body("Failed to post comment to GitLab issue.")
            }
        }
    } else { // Is Merge Request
        let mr_iid = match event.merge_request.as_ref().map(|mr| mr.iid) {
            Some(iid) => iid,
            None => {
                // This should ideally not happen if we reached here, but as a safeguard:
                error!("Critical: Missing mr_iid when trying to post comment. Event: {:?}", event);
                return HttpResponse::InternalServerError().body("Internal error: Missing MR context for comment posting.");
            }
        };
        match gitlab_client.post_comment_to_merge_request(project_id, mr_iid, &final_comment_body).await {
            Ok(_note) => {
                info!("Successfully posted comment to MR {}!{}", project_id, mr_iid);
                HttpResponse::Ok().body("Successfully processed mention and posted reply to merge request.")
            }
            Err(e) => {
                error!("Failed to post comment to MR {}!{}: {}", project_id, mr_iid, e);
                HttpResponse::InternalServerError().body("Failed to post comment to GitLab merge request.")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Note: Full handler tests are complex due to dependencies (clients, config).
    // These tests focus on the helper function. Integration tests would cover the handler.

    #[test]
    fn test_extract_context_after_mention() {
        let bot_name = "mybot";
        
        // Basic case
        let note1 = "Hello @mybot please summarize this";
        assert_eq!(extract_context_after_mention(note1, bot_name), Some("please summarize this".to_string()));

        // With leading/trailing whitespace for context
        let note2 = "@mybot  summarize this for me  ";
        assert_eq!(extract_context_after_mention(note2, bot_name), Some("summarize this for me".to_string()));

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
        assert_eq!(extract_context_after_mention(note6, bot_name), Some(", what do you think?".to_string()));

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
        assert_eq!(extract_context_after_mention(note10, bot_name), Some("summarize this, and also @mybot do that".to_string()));
    }
}
