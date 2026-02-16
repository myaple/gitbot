use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

use crate::config::AppSettings;
use crate::file_indexer::FileIndexManager;
use crate::gitlab::{GitlabApiClient, GitlabError, LabelOperation};
use crate::mention_cache::MentionCache;
use crate::models::{GitlabNoteAttributes, GitlabNoteEvent, OpenAIChatMessage, ToolChoice};
use crate::openai::{ChatRequestBuilder, OpenAIApiClient};
use crate::repo_context::RepoContextExtractor;
use crate::tools::{create_basic_tool_registry, ToolCallContext};

// Helper function to extract context after bot mention
pub(crate) fn extract_context_after_mention(note: &str, bot_name: &str) -> Option<String> {
    let mention = format!("@{bot_name}");
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
    Help,
    // Issue commands
    Plan,
    Fix,
    // Merge request commands
    Security,
    Docs,
    Tests,
}

impl SlashCommand {
    pub fn get_precanned_prompt(&self) -> &'static str {
        match self {
            SlashCommand::Summarize => {
                "Summarize changes with detailed analysis including:\n\
                1. **Guideline Adherence**: Review against CONTRIBUTING.md and project standards, providing specific examples\n\
                2. **Performance Impact**: CPU and memory usage analysis, Big-O complexity assessment, database query optimization, scalability concerns\n\
                3. **Code Quality**: Readability, maintainability, error handling, security implications\n\
                4. **Risk Assessment**: Breaking changes for backward compatibility, integration points affected, dependency risks\n\
                5. **Strengths**: Highlight notable improvements and best practices followed\n\
                6. **Areas for Improvement**: Specific constructive feedback with actionable suggestions\n\
                7. **Recommendations**: Clear next steps, additional reviews needed, or testing requirements\n\
                Format your response in markdown with clear headings."
            }
            SlashCommand::Postmortem => {
                "Create a comprehensive incident postmortem following this structure:\n\
                ## Executive Summary\n\
                - Brief overview of what happened\n\
                - Impact on users/systems\n\
                - Duration and resolution time\n\
                ## Timeline\n\
                - Chronological sequence of events with timestamps (automatically extracted from issue discussion)\n\
                - Key decisions and their timing\n\
                ## Root Cause Analysis\n\
                - Primary vs. contributing factors\n\
                - Technical vs. systemic causes\n\
                - Use '5 Whys' technique to dig deep\n\
                ## Impact Assessment\n\
                - Affected users/systems\n\
                - Business impact\n\
                - Performance impact\n\
                ## Action Items\n\
                - Specific, measurable, achievable, relevant, time-bound (SMART) actions\n\
                - Owner for each action\n\
                - Priority levels\n\
                ## Lessons Learned\n\
                - What worked well during the incident\n\
                - What could be improved\n\
                - Preventive measures\n\
                ## Follow-up Plan\n\
                - Review timeline\n\
                - Action item tracking\n\
                - Process improvements\n\
                Be blameless and focus on learning and prevention. Format in markdown."
            }
            SlashCommand::Help => {
                "You should respond by listing all available slash commands for GitBot and explaining their purposes. Also offer to help the user understand what GitBot can do for them. Be helpful and welcoming in your response, as the user is trying to understand GitBot's capabilities."
            }
            SlashCommand::Plan => {
                "Create a comprehensive implementation plan for this issue using the following structure:\n\
                ## Executive Summary\n\
                - Brief overview of what will be implemented\n\
                - Business value justification\n\
                - Success criteria\n\
                ## Technical Approach\n\
                - Architecture overview and design decisions\n\
                - Integration points with existing systems\n\
                - Data flow and state management\n\
                ## Implementation Breakdown (prioritized by order of execution)\n\
                1. **Setup & Preparation** (environment, dependencies, scaffolding)\n\
                2. **Core Implementation** (main feature components)\n\
                3. **Integration & Edge Cases** (error handling, edge cases)\n\
                4. **Testing & Validation** (comprehensive testing strategy)\n\
                5. **Documentation & Deployment** (docs, deployment, monitoring)\n\
                For each step, include:\n\
                - Detailed description of work\n\
                - Specific files/lines to modify\n\
                - Estimated complexity (Low/Medium/High)\n\
                - Dependencies on other steps\n\
                - Success criteria for completion\n\
                ## Risk Assessment\n\
                - Technical risks and mitigation strategies\n\
                - Rollback plan\n\
                - Monitoring requirements\n\
                - Performance considerations\n\
                ## Testing Strategy\n\
                - Unit tests (what to test, why)\n\
                - Integration tests (components to validate)\n\
                - End-to-end scenarios\n\
                - Performance/load testing needs\n\
                - Manual test cases\n\
                ## Success Metrics\n\
                - How to verify the implementation works\n\
                - Performance benchmarks\n\
                - User acceptance criteria\n\
                Format your response in markdown with clear headings, code blocks for file paths, and checklists for tasks."
            }
            SlashCommand::Fix => {
                "Analyze this bug report and provide a comprehensive fix following this structured approach:\n\
                ## Investigation & Root Cause Analysis\n\
                - Distinguish symptoms from root causes using the 5 whys technique\n\
                - Analyze failure modes, edge cases, and boundary conditions\n\
                - Suggest steps to reproduce the issue systematically\n\
                - Examine any available error logs, stack traces, or debugging information\n\
                - Check related commits, existing tests, and similar issues\n\
                ## Specific Implementation Changes\n\
                - Provide complete, working code snippets with proper syntax highlighting\n\
                - Include before/after comparisons where helpful\n\
                - Explain the logic and reasoning behind each change\n\
                - Note performance implications and optimization opportunities\n\
                - Identify any refactoring opportunities or cleanup needed\n\
                ## Target Files & Functions (with precision)\n\
                - List files with exact paths and line numbers for modifications\n\
                - Prioritize changes (1=most critical, based on impact and risk)\n\
                - Identify key functions and their interdependencies\n\
                - Note files that should be read but not modified\n\
                - Highlight potential ripple effects to other modules\n\
                ## Comprehensive Testing Strategy\n\
                - Unit tests for modified functions with example inputs/expected outputs\n\
                - Integration tests for component interactions\n\
                - Edge cases: null inputs, boundary values, error conditions\n\
                - Mocking strategy for external dependencies\n\
                - Regression tests to prevent similar bugs\n\
                - Test coverage recommendation (aim for 80%+ on modified code)\n\
                ## Risk Assessment & Mitigation\n\
                - Potential side effects and how to detect them\n\
                - Backward compatibility considerations\n\
                - Performance impact assessment\n\
                - Monitoring recommendations for early detection of issues\n\
                - Rollback plan if the fix causes unexpected problems\n\
                ## Code Quality & Documentation\n\
                - Ensure changes follow project coding standards\n\
                - Consider readability and maintainability\n\
                - Document any new or modified functionality\n\
                - Update relevant comments or documentation\n\
                Use available tools to search code, read files, and verify your analysis before providing your final recommendation. Format in markdown."
            }
            SlashCommand::Security => {
                "Perform a comprehensive security review using OWASP Top 10 2021 and OWASP ASVS guidelines. Analyze:\n\
                1. **Input Validation**: Check for all user inputs - HTTP parameters, form data, headers, cookies, file uploads\n\
                2. **Authentication**: Verify session management, password policies, MFA implementation\n\
                3. **Authorization**: Check for privilege escalation, broken access control, horizontal/vertical privilege escalation\n\
                4. **Data Security**: Analyze encryption at rest/transit, PII handling, secrets management\n\
                5. **API Security**: Review rate limiting, input validation for REST/GraphQL, CORS policies\n\
                6. **File Operations**: Check for path traversal, arbitrary file write, unsafe deserialization\n\
                7. **Business Logic**: Examine for race conditions, transaction issues, payment security\n\
                8. **Dependencies**: Scan for known vulnerabilities in npm/pip/Cargo/Go packages\n\
                \n\
                For each finding:\n\
                - Provide exact file path and line number\n\
                - Include code snippets demonstrating the issue\n\
                - Assign severity (Critical/High/Medium/Low) with CVSS-like scoring\n\
                - Give specific remediation code examples\n\
                - Mention if this affects confidentiality, integrity, or availability\n\
                \n\
                Also check for:\n\
                - Missing security headers (Content-Security-Policy, X-Frame-Options, etc.)\n\
                - Information disclosure in error messages\n\
                - Hardcoded credentials or API keys\n\
                - Insecure random number generation\n\
                - Cryptographic implementation flaws\n\
                Format in markdown with severity tags."
            }
            SlashCommand::Docs => {
                "Generate comprehensive documentation for the changes in this merge request. Focus on:\n\
                ## Code Analysis\n\
                First analyze the code changes to identify:\n\
                - New public APIs, functions, classes, and modules\n\
                - Modified existing interfaces\n\
                - Complex algorithms or business logic\n\
                - Any code with special safety or security requirements\n\
                ## Documentation Requirements\n\
                For each identified component, provide:\n\
                - Module/class-level documentation for public interfaces\n\
                - Function/method signatures with documented parameters and return values\n\
                - Type/generic parameter documentation where applicable\n\
                - Usage examples with proper code blocks for the project's programming language\n\
                - Edge cases and error conditions\n\
                ## Documentation Types Include\n\
                - Module-level documentation for new/changed modules\n\
                - Function/method documentation with parameters and return values\n\
                - Inline comments for complex logic\n\
                - CHANGELOG entries for breaking changes\n\
                - README updates for new user-facing features\n\
                ## Format and Style\n\
                - Use the documentation conventions appropriate for the project's programming language\n\
                - Include proper code blocks with syntax highlighting\n\
                - Follow the project's existing documentation style from README.md and CONTRIBUTING.md\n\
                - Ensure all public APIs are documented\n\
                Review the existing codebase documentation patterns and maintain consistency while ensuring all public interfaces are properly documented. Format in markdown."
            }
            SlashCommand::Tests => {
                "Suggest comprehensive tests for the changes in this merge request. Analyze the code changes and provide:\n\
                ## Test Analysis\n\
                - Identify specific functions/areas needing tests based on code complexity\n\
                - Prioritize by criticality and risk\n\
                - Consider the testing framework used in this repository\n\
                ## Test Categories\n\
                ### 1. Unit Tests\n\
                - For each new public function, suggest tests covering:\n\
                  * Happy path scenarios\n\
                  * Input validation\n\
                  * Error conditions\n\
                  * Edge cases\n\
                - Use the repository's testing framework conventions\n\
                ### 2. Integration Tests\n\
                - Component interactions\n\
                - API contract validation\n\
                - Data flow verification\n\
                ### 3. Edge Cases and Boundary Conditions\n\
                - Null/empty inputs\n\
                - Boundary values\n\
                - Race conditions (for concurrent code)\n\
                - Resource limits\n\
                ### 4. Error Handling Scenarios\n\
                - Exception paths\n\
                - Timeout scenarios\n\
                - Network failures\n\
                - Invalid states\n\
                ### 5. Mock/Fixture Strategy\n\
                - Mocking strategy for external dependencies\n\
                - Test data factories\n\
                - Fixture setup recommendations\n\
                ### 6. Test Coverage\n\
                - Aim for 80%+ coverage on modified code\n\
                - Suggest coverage thresholds\n\
                ### 7. Additional Testing\n\
                - Property-based testing for pure functions\n\
                - Performance/benchmark tests where applicable\n\
                ### Test Organization\n\
                - Suggest test file naming conventions\n\
                - Test organization by module/function\n\
                Provide specific test code examples using the testing framework detected in the repository. Format in markdown with code blocks."
            }
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "summarize" => Some(SlashCommand::Summarize),
            "postmortem" => Some(SlashCommand::Postmortem),
            "help" => Some(SlashCommand::Help),
            "plan" => Some(SlashCommand::Plan),
            "fix" => Some(SlashCommand::Fix),
            "security" => Some(SlashCommand::Security),
            "docs" => Some(SlashCommand::Docs),
            "tests" | "test" => Some(SlashCommand::Tests),
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
        ### Issue Commands:\n\
        • `/plan` - Create a detailed implementation plan with steps, risks, and testing recommendations\n\
        • `/fix` - Analyze bug and provide specific fix with root cause analysis\n\
        • `/postmortem` - {}\n\
        \n\
        ### Merge Request Commands:\n\
        • `/security` - Perform comprehensive security review\n\
        • `/docs` - Generate documentation for the changes\n\
        • `/tests` - Suggest comprehensive tests (unit, integration, edge cases)\n\
        • `/summarize` - {}\n\
        \n\
        ### General:\n\
        • `/help` - {}\n\n\
        You can add additional context after any command, e.g., `/plan focus on the authentication module`",
        SlashCommand::Postmortem.get_precanned_prompt(),
        SlashCommand::Summarize.get_precanned_prompt(),
        SlashCommand::Help.get_precanned_prompt()
    )
}

// Helper function to format incident timeline for postmortem
fn format_incident_timeline(
    issue: &crate::models::GitlabIssue,
    comments: &[crate::models::GitlabNoteAttributes],
) -> String {
    let mut events = Vec::new();

    // Issue creation
    events.push((
        "Issue Created",
        issue.created_at.clone(),
        format!(
            "Issue '{}' created by @{}",
            issue.title, issue.author.username
        ),
    ));

    // Add comments as timeline events
    for comment in comments {
        let content = comment.note.trim().to_string();
        let preview = if content.len() > 200 {
            format!("{}...", &content[..200])
        } else {
            content
        };

        events.push((
            "Comment Posted",
            comment.updated_at.clone(),
            format!("@{} commented: {}", comment.author.username, preview),
        ));
    }

    // Sort by timestamp
    events.sort_by(|a, b| a.1.cmp(&b.1));

    // Format the timeline
    let mut timeline = String::from("## Incident Timeline\n\n");
    for (event_type, timestamp, description) in events {
        timeline.push_str(&format!(
            "- **[{}] {}** - {}\n",
            timestamp, event_type, description
        ));
    }

    // Note if issue is closed (we can't calculate duration without closed_at)
    if issue.state == "closed" {
        timeline.push_str("\n**Note**: This issue has been closed.\n");
    }

    timeline
}

// Helper function to validate command is appropriate for context
#[allow(dead_code)]
fn validate_command_for_context(
    command: &SlashCommand,
    noteable_type: &str,
) -> Result<(), &'static str> {
    match command {
        SlashCommand::Plan | SlashCommand::Fix | SlashCommand::Postmortem => {
            if noteable_type != "Issue" {
                Err("This command can only be used on issues")
            } else {
                Ok(())
            }
        }
        SlashCommand::Security | SlashCommand::Docs | SlashCommand::Tests => {
            if noteable_type != "MergeRequest" {
                Err("This command can only be used on merge requests")
            } else {
                Ok(())
            }
        }
        _ => Ok(()), // Other commands work everywhere
    }
}

// Helper function to add security-specific context for /security command
async fn add_security_context_to_prompt(
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
    prompt_parts: &mut Vec<String>,
) {
    // Check for common security files
    let security_files = vec![
        "SECURITY.md",
        ".github/security/policy.md",
        "security-policy.md",
    ];

    for file in security_files {
        if let Ok(content) = gitlab_client.get_file_content(project_id, file, None).await {
            if let Some(file_content) = content.content {
                // Truncate to avoid overwhelming the prompt
                let truncated = if file_content.len() > 1000 {
                    format!("{}... [truncated]", &file_content[..1000])
                } else {
                    file_content
                };
                prompt_parts.push(format!(
                    "Repository Security Policy (from {}):\n{}\n",
                    file, truncated
                ));
                break;
            }
        }
    }

    // Add general security guidelines
    prompt_parts.push(String::from(
        "Security Review Checklist:\n\
        - [ ] Input validation (sanitize all user input)\n\
        - [ ] Output encoding (prevent XSS)\n\
        - [ ] SQL parameterization (prevent SQL injection)\n\
        - [ ] Authentication checks\n\
        - [ ] Authorization checks\n\
        - [ ] Sensitive data handling\n\
        - [ ] Error messages don't leak information\n\
        - [ ] Dependencies are up to date\n\
        - [ ] Proper session management\n\
        - [ ] CSRF protection",
    ));
}

// Helper function to add testing-specific context for /tests command
async fn add_testing_context_to_prompt(
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
    prompt_parts: &mut Vec<String>,
) {
    // Look for test configuration files
    let test_configs = vec![
        "Cargo.toml",     // Rust
        "pytest.ini",     // Python
        "jest.config.js", // JavaScript
        "go.mod",         // Go
    ];

    for file in test_configs {
        if let Ok(content) = gitlab_client.get_file_content(project_id, file, None).await {
            if content.content.is_some() {
                // Just mention we found test config, don't include content (too verbose)
                prompt_parts.push(format!("Detected testing configuration file: {}", file));
                break;
            }
        }
    }

    // Add testing guidelines
    prompt_parts.push(String::from(
        "Testing Best Practices:\n\
        - Write tests before or alongside code (TDD)\n\
        - Test public interfaces, not implementation details\n\
        - Use descriptive test names\n\
        - Follow AAA pattern (Arrange, Act, Assert)\n\
        - Mock external dependencies\n\
        - Test edge cases and error conditions\n\
        - Maintain test independence\n\
        - Keep tests fast and focused",
    ));
}

// Helper function to add documentation-specific context for /docs command
async fn add_documentation_context_to_prompt(
    gitlab_client: &Arc<GitlabApiClient>,
    project_id: i64,
    prompt_parts: &mut Vec<String>,
) {
    // Check for common documentation files
    let doc_files = vec!["README.md", "CONTRIBUTING.md", "docs/guidelines.md"];

    for file in doc_files {
        if let Ok(content) = gitlab_client.get_file_content(project_id, file, None).await {
            if let Some(file_content) = content.content {
                // Include a snippet to understand documentation style
                let truncated = if file_content.len() > 500 {
                    format!("{}... [truncated]", &file_content[..500])
                } else {
                    file_content.clone()
                };
                prompt_parts.push(format!(
                    "Documentation style reference from {}:\n{}\n",
                    file, truncated
                ));
                break;
            }
        }
    }

    // Add documentation guidelines
    prompt_parts.push(String::from(
        "Documentation Best Practices:\n\
        - Write clear, concise descriptions\n\
        - Include usage examples\n\
        - Document parameters and return values\n\
        - Note edge cases and limitations\n\
        - Keep documentation up-to-date with code\n\
        - Use consistent formatting\n\
        - Include integration points and dependencies",
    ));
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
        .get_issue_notes(project_id, issue_iid, None)
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
        .get_merge_request_notes(project_id, mr_iid, None)
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
                .get_issue_notes(project_id, issue_iid, Some(timestamp_u64))
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
                .get_merge_request_notes(project_id, mr_iid, Some(timestamp_u64))
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
    tool_context: Option<&mut ToolCallContext>,
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

    // Use tool-enabled version if tool context is provided
    let llm_reply = if let Some(tool_ctx) = tool_context {
        get_llm_reply_with_tools(&openai_client, config, &final_prompt_text, tool_ctx).await
    } else {
        get_llm_reply(&openai_client, config, &final_prompt_text).await
    }?;

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

    // Create tool context with GitLab tools
    let tool_registry = create_basic_tool_registry(gitlab_client.clone(), config.clone());
    let mut tool_context = ToolCallContext::new(config.max_tool_calls, tool_registry);

    generate_and_post_reply(
        &event,
        &gitlab_client,
        &config,
        project_id,
        reply_context,
        Some(&mut tool_context),
    )
    .await?;

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
            "Hey @{event_user_username}, here's the information you requested:\n\n---\n\n{llm_reply}"
        )
    } else {
        // For merge requests, include commit history only if no user context was provided
        if user_provided_context.is_none() {
            format!(
                "Hey @{event_user_username}, here's the information you requested:\n\n---\n\n{llm_reply}\n\n<details><summary>Additional Commit History</summary>\n\n{commit_history}</details>"
            )
        } else {
            format!(
                "Hey @{event_user_username}, here's the information you requested:\n\n---\n\n{llm_reply}"
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
    // Use ChatRequestBuilder which automatically handles prompt_prefix
    let mut builder = ChatRequestBuilder::new(config);
    builder.with_user_message(prompt_text);
    let openai_request = builder
        .build()
        .map_err(|e| anyhow!("Failed to build request: {}", e))?;

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
                // Check if it was truncated due to length
                let finish_reason = choice.finish_reason.as_deref().unwrap_or("unknown");
                let usage_info = if let Some(usage) = &openai_response.usage {
                    format!(
                        " (used {}/{} tokens: {} prompt + {} completion)",
                        usage.total_tokens,
                        config.openai_max_tokens,
                        usage.prompt_tokens,
                        usage.completion_tokens.unwrap_or(0)
                    )
                } else {
                    String::new()
                };

                match finish_reason {
                    "length" => Err(anyhow!(
                        "LLM response was truncated due to token limit. \
                         Consider increasing --openai-max-tokens (currently {}) \
                         or reducing context size{}",
                        config.openai_max_tokens,
                        usage_info
                    )),
                    _ => Err(anyhow!(
                        "LLM response content is empty (finish_reason: {}){}",
                        finish_reason,
                        usage_info
                    )),
                }
            } else {
                Ok(choice.message.content.clone())
            }
        })
}

/// Enhanced version of get_llm_reply that supports tool calling
async fn get_llm_reply_with_tools(
    openai_client: &OpenAIApiClient,
    config: &Arc<AppSettings>,
    prompt_text: &str,
    tool_context: &mut ToolCallContext,
) -> Result<String> {
    // Create system message with project ID and branch context
    let system_message = format!(
        "You are GitBot, a helpful assistant for GitLab repositories. When using tools to search or access files, \
        pay close attention to project IDs and branches:\n\
        - Use the main project ID for files and issues/MRs in the main repository\n\
        - If a context repository is configured, use its project ID for files from that repository\n\
        - For merge requests, use the source branch unless specifically asked for the target branch\n\
        - For issues, use the default branch provided in the context\n\
        - Use the get_project_by_path tool to resolve project paths to project IDs if needed\n\
        - The search_code tool defaults to the repository's default branch if no branch is specified\n\
        - The prompt will provide specific project ID and branch information for each context\n\
        - You **DO NOT** have the ability to modify any code directly - provide suggestions to the user as an adviser only\n\
        - IMPORTANT: You can make a maximum of {} tool calls per message. Plan your tool usage efficiently.",
        tool_context.max_tool_calls()
    );

    // Get tool specifications for the request
    let tool_specs = tool_context.get_tool_specs();
    let has_tools = !tool_specs.is_empty();

    // Prepend prompt prefix if configured
    let user_prompt = if let Some(prefix) = &config.prompt_prefix {
        format!("{prefix}\n\n{prompt_text}")
    } else {
        prompt_text.to_string()
    };

    // Create initial messages with system message first
    let mut messages = Vec::new();
    messages.push(OpenAIChatMessage {
        role: "system".to_string(),
        content: system_message,
        tool_calls: None,
        tool_call_id: None,
    });
    messages.push(OpenAIChatMessage {
        role: "user".to_string(),
        content: user_prompt,
        tool_calls: None,
        tool_call_id: None,
    });

    // Multi-turn conversation loop with safety checks
    // Use max_tool_calls as base for conversation turns (with reasonable upper limit)
    let max_turns = std::cmp::min(tool_context.max_tool_calls() * 10, 25); // Prevent infinite loops
    let mut current_turn = 0;
    let max_tool_calls_per_turn = std::cmp::min(tool_context.max_tool_calls(), 10); // Prevent too many tools in one turn

    loop {
        info!("Entering tool call loop");

        current_turn += 1;
        if current_turn > max_turns {
            error!("Maximum conversation turns reached: {}", max_turns);
            return Err(anyhow!(
                "I've reached the maximum number of conversation turns ({}). Please try a simpler request.",
                max_turns
            ));
        }

        // Build the request using the builder with current messages
        let mut builder = ChatRequestBuilder::new(config);
        builder.with_messages(messages.clone());

        if has_tools {
            builder.with_tools(tool_specs.clone());
            builder.with_tool_choice(ToolChoice::Auto);
        }

        let openai_request = builder
            .build()
            .map_err(|e| anyhow!("Failed to build request: {}", e))?;

        // Send request to OpenAI
        let openai_response = openai_client
            .send_chat_completion(&openai_request)
            .await
            .map_err(|e| {
                error!("Failed to communicate with OpenAI: {}", e);
                anyhow!("Failed to communicate with OpenAI: {}", e)
            })?;

        debug!("OpenAI response: {:?}", openai_response);

        // Get the first choice
        let choice = openai_response
            .choices
            .first()
            .ok_or_else(|| anyhow!("No response choices from OpenAI"))?;

        // Check if the LLM wants to call tools
        if let Some(tool_calls) = &choice.message.tool_calls {
            if !tool_calls.is_empty() {
                info!("LLM requested {} tool calls", tool_calls.len());

                // Execute each tool call with safety checks
                let tool_calls_to_execute = if tool_calls.len() > max_tool_calls_per_turn as usize {
                    warn!(
                        "Too many tool calls in one turn: {}, limiting to {}",
                        tool_calls.len(),
                        max_tool_calls_per_turn
                    );
                    &tool_calls[..max_tool_calls_per_turn as usize]
                } else {
                    tool_calls
                };

                let mut reached_limit = false;

                // Check if we have enough capacity for all tool calls
                let available_calls = tool_context.remaining_tool_calls();
                let requested_calls = tool_calls_to_execute.len() as u32;

                // Check if we've reached the limit before limiting the tool calls
                if requested_calls > available_calls {
                    // We've reached the limit
                    reached_limit = true;
                    warn!(
                        "Maximum tool calls reached: {} ({} requested, {} available)",
                        tool_context.max_tool_calls(),
                        requested_calls,
                        available_calls
                    );

                    // Add a system message asking LLM to finalize with available information
                    messages.push(OpenAIChatMessage {
                        role: "system".to_string(),
                        content: format!(
                            "You have reached the maximum tool call limit ({} calls). Please provide your best final answer based on the information you've gathered so far. Do not make additional tool calls.",
                            tool_context.max_tool_calls()
                        ),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }

                // Now limit the tool calls if we don't have enough capacity
                let tool_calls_to_execute = if requested_calls > available_calls {
                    warn!(
                        "Not enough tool call capacity available ({} requested, {} available). Limiting to {} calls.",
                        requested_calls,
                        available_calls,
                        available_calls
                    );
                    &tool_calls_to_execute[..available_calls as usize]
                } else {
                    tool_calls_to_execute
                };

                if !reached_limit {
                    // Safety check: validate tool call arguments length
                    for tool_call in tool_calls_to_execute {
                        if tool_call.function.arguments.len() > 1000 {
                            error!(
                                "Tool call arguments too large: {} bytes",
                                tool_call.function.arguments.len()
                            );
                            return Err(anyhow!(
                                "The requested operation is too complex. Please try something simpler."
                            ));
                        }
                    }

                    // Execute tool calls in parallel using the context
                    info!(
                        "Executing {} tool calls in parallel",
                        tool_calls_to_execute.len()
                    );
                    let tool_results = tool_context
                        .execute_tool_calls_parallel(
                            &tool_calls_to_execute.iter().collect::<Vec<_>>(),
                        )
                        .await;

                    // Process results and update tool call counter
                    let mut successful_calls = 0;
                    for (tool_call, result) in tool_results {
                        match result {
                            Ok(tool_response) => {
                                successful_calls += 1;

                                // Add tool call message
                                messages.push(OpenAIChatMessage {
                                    role: "assistant".to_string(),
                                    content: String::new(), // Empty content for tool calls
                                    tool_calls: Some(vec![crate::models::ToolCall {
                                        id: tool_call.id.clone(),
                                        r#type: "function".to_string(),
                                        function: crate::models::FunctionCall {
                                            name: tool_call.function.name.clone(),
                                            arguments: tool_call.function.arguments.clone(),
                                        },
                                    }]),
                                    tool_call_id: None,
                                });

                                // Add tool response message
                                messages.push(OpenAIChatMessage {
                                    role: "tool".to_string(),
                                    content: tool_response.content,
                                    tool_calls: None,
                                    tool_call_id: Some(tool_call.id.clone()),
                                });
                            }
                            Err(e) => {
                                error!(
                                    "Tool execution failed for {}: {}",
                                    tool_call.function.name, e
                                );
                                // Add error message as tool response
                                messages.push(OpenAIChatMessage {
                                    role: "assistant".to_string(),
                                    content: String::new(),
                                    tool_calls: Some(vec![crate::models::ToolCall {
                                        id: tool_call.id.clone(),
                                        r#type: "function".to_string(),
                                        function: crate::models::FunctionCall {
                                            name: tool_call.function.name.clone(),
                                            arguments: tool_call.function.arguments.clone(),
                                        },
                                    }]),
                                    tool_call_id: None,
                                });

                                messages.push(OpenAIChatMessage {
                                    role: "tool".to_string(),
                                    content: format!("Error: {}", e),
                                    tool_calls: None,
                                    tool_call_id: Some(tool_call.id.clone()),
                                });
                            }
                        }
                    }

                    // Note: tool call counter is now updated automatically in execute_tool_calls_parallel

                    info!(
                        "Successfully executed {} out of {} tool calls in parallel",
                        successful_calls,
                        tool_calls_to_execute.len()
                    );
                }

                // If we limited tool calls, add a message to inform the user
                if tool_calls.len() > max_tool_calls_per_turn as usize {
                    let skipped_count = tool_calls.len() - max_tool_calls_per_turn as usize;
                    messages.push(OpenAIChatMessage {
                        role: "assistant".to_string(),
                        content: format!(
                            "Note: I limited myself to {} tool calls to avoid overwhelming the system. {} additional operations were skipped. Please ask for the remaining operations separately if you still need them.",
                            max_tool_calls_per_turn, skipped_count
                        ),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }

                // If we reached the limit, log it for debugging
                if reached_limit {
                    info!("Tool limit reached, requesting final answer from LLM");
                }

                continue; // Continue the loop to get final answer
            }
        }

        // If we get here, the LLM provided a final answer
        if choice.message.content.is_empty() {
            // Check if it was truncated due to length
            let finish_reason = choice.finish_reason.as_deref().unwrap_or("unknown");
            let usage_info = if let Some(usage) = &openai_response.usage {
                format!(
                    " (used {}/{} tokens: {} prompt + {} completion)",
                    usage.total_tokens,
                    config.openai_max_tokens,
                    usage.prompt_tokens,
                    usage.completion_tokens.unwrap_or(0)
                )
            } else {
                String::new()
            };

            return match finish_reason {
                "length" => Err(anyhow!(
                    "LLM response was truncated due to token limit. \
                     Consider increasing --openai-max-tokens (currently {}) \
                     or reducing context size{}",
                    config.openai_max_tokens,
                    usage_info
                )),
                _ => Err(anyhow!(
                    "LLM response content is empty (finish_reason: {}){}",
                    finish_reason,
                    usage_info
                )),
            };
        }

        return Ok(choice.message.content.clone());
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
                    match gitlab_client
                        .update_issue_labels(
                            project_id,
                            issue_iid,
                            LabelOperation::Remove(vec!["stale".to_string()]),
                        )
                        .await
                    {
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
    // Add project ID and branch context for the LLM
    prompt_parts.push(format!(
        "Project Information:\n- Main Project ID: {} ({}) - This is where the issue/mr is located\n- Context Repository: {:?} - Additional context files come from here\n- Default Branch: {} (used for issue-related operations)",
        project.id,
        project.path_with_namespace,
        config.context_repo_path.as_deref().unwrap_or("None configured"),
        config.default_branch
    ));

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
                "Repository Context (files are ranked by relevance based on keyword frequency - higher percentages indicate more relevant content):\n{context_str}\n\nNOTE: When using tools to search or access files, use project_id {} for files from the main project and the appropriate context repository project ID for files from the context repository. For code searches in issues, use the default branch '{}' unless otherwise specified.",
                project.id, config.default_branch
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
pub(crate) struct IssuePromptContext<'a> {
    pub(crate) event: &'a GitlabNoteEvent,
    pub(crate) gitlab_client: &'a Arc<GitlabApiClient>,
    pub(crate) config: &'a Arc<AppSettings>,
    pub(crate) project_id: i64,
    pub(crate) issue_iid: i64,
    pub(crate) issue: &'a crate::models::GitlabIssue,
    pub(crate) file_index_manager: &'a Arc<FileIndexManager>,
}

// Helper function to build issue prompt with user-provided context
pub(crate) async fn build_issue_prompt_with_context(
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
    } else if user_context.starts_with('/') {
        // Unknown slash command
        prompt_parts.push(SlashCommand::Help.get_precanned_prompt().to_string());
        prompt_parts.push(generate_help_message());
        warn!(
            "User @{} used an unknown slash command: {}",
            context.event.user.username, user_context
        );
    } else {
        // Original behavior for non-slash commands (plain text, not starting with /)
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
    let comments = match fetch_all_issue_comments(
        context.gitlab_client,
        context.project_id,
        context.issue_iid,
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
            comments
        }
        Err(e) => {
            warn!("Failed to fetch issue comments for context: {}", e);
            prompt_parts.push("Previous comments could not be retrieved.".to_string());
            Vec::new()
        }
    };

    // Add timeline for postmortem command
    if let Some((slash_command, _)) = parse_slash_command(user_context) {
        if matches!(slash_command, SlashCommand::Postmortem) {
            let timeline = format_incident_timeline(&issue_details, &comments);
            prompt_parts.push(timeline);
        }
    }

    prompt_parts.push(format!("User's specific request: {user_context}"));

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
        .get_file_content(project_id, "CONTRIBUTING.md", None)
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
    // Add project ID and branch context for the LLM
    prompt_parts.push(format!(
        "Project Information:\n- Main Project ID: {} ({}) - This is where the issue/mr is located\n- Context Repository: {:?} - Additional context files come from here\n- Merge Request Branch: {} (source branch)\n- Target Branch: {} (base branch for the merge request)",
        project.id,
        project.path_with_namespace,
        config.context_repo_path.as_deref().unwrap_or("None configured"),
        mr.source_branch,
        mr.target_branch
    ));

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
                "Code Changes (files are ranked by relevance based on keyword frequency - higher percentages indicate more relevant content):\n{context_for_llm}\n\nNOTE: When using tools to search or access files, use project_id {} for files from the main project and the appropriate context repository project ID for files from the context repository. For merge requests, use branch '{}' (source branch) unless you specifically need to search the target branch '{}'.",
                project.id, mr.source_branch, mr.target_branch
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
pub(crate) struct MrPromptContext<'a> {
    pub(crate) event: &'a GitlabNoteEvent,
    pub(crate) gitlab_client: &'a Arc<GitlabApiClient>,
    pub(crate) config: &'a Arc<AppSettings>,
    pub(crate) mr: &'a crate::models::GitlabMergeRequest,
    pub(crate) file_index_manager: &'a Arc<FileIndexManager>,
}

// Helper function to build MR prompt with user-provided context
pub(crate) async fn build_mr_prompt_with_context(
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
    } else if user_context.starts_with('/') {
        // Unknown slash command
        prompt_parts.push(SlashCommand::Help.get_precanned_prompt().to_string());
        prompt_parts.push(generate_help_message());
        warn!(
            "User @{} used an unknown slash command in MR context: {}",
            context.event.user.username, user_context
        );
    } else {
        // Original behavior for non-slash commands (plain text, not starting with /)
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

    // Add command-specific context for slash commands
    if let Some((slash_command, _)) = parse_slash_command(user_context) {
        match slash_command {
            SlashCommand::Security => {
                add_security_context_to_prompt(
                    context.gitlab_client,
                    context.event.project.id,
                    prompt_parts,
                )
                .await;
            }
            SlashCommand::Tests => {
                add_testing_context_to_prompt(
                    context.gitlab_client,
                    context.event.project.id,
                    prompt_parts,
                )
                .await;
            }
            SlashCommand::Docs => {
                add_documentation_context_to_prompt(
                    context.gitlab_client,
                    context.event.project.id,
                    prompt_parts,
                )
                .await;
            }
            _ => {}
        }
    }

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

    prompt_parts.push(format!("User's specific request: {user_context}"));
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
            "The following are the guidelines from CONTRIBUTING.md:\n{contributing_content}\n\nPlease review how well this MR adheres to these guidelines."
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slash_command_plan() {
        let result = parse_slash_command("/plan");
        assert!(result.is_some());
        let (cmd, ctx) = result.unwrap();
        assert_eq!(cmd, SlashCommand::Plan);
        assert!(ctx.is_none());
    }

    #[test]
    fn test_parse_slash_command_fix() {
        let result = parse_slash_command("/fix");
        assert!(result.is_some());
        let (cmd, ctx) = result.unwrap();
        assert_eq!(cmd, SlashCommand::Fix);
        assert!(ctx.is_none());
    }

    #[test]
    fn test_parse_slash_command_security() {
        let result = parse_slash_command("/security");
        assert!(result.is_some());
        let (cmd, ctx) = result.unwrap();
        assert_eq!(cmd, SlashCommand::Security);
        assert!(ctx.is_none());
    }

    #[test]
    fn test_parse_slash_command_docs() {
        let result = parse_slash_command("/docs");
        assert!(result.is_some());
        let (cmd, ctx) = result.unwrap();
        assert_eq!(cmd, SlashCommand::Docs);
        assert!(ctx.is_none());
    }

    #[test]
    fn test_parse_slash_command_tests() {
        let result = parse_slash_command("/tests");
        assert!(result.is_some());
        let (cmd, ctx) = result.unwrap();
        assert_eq!(cmd, SlashCommand::Tests);
        assert!(ctx.is_none());
    }

    #[test]
    fn test_parse_slash_command_test_alternative() {
        let result = parse_slash_command("/test");
        assert!(result.is_some());
        let (cmd, ctx) = result.unwrap();
        assert_eq!(cmd, SlashCommand::Tests);
        assert!(ctx.is_none());
    }

    #[test]
    fn test_parse_slash_command_with_context() {
        let result = parse_slash_command("/plan focus on auth");
        assert!(result.is_some());
        let (cmd, ctx) = result.unwrap();
        assert_eq!(cmd, SlashCommand::Plan);
        assert_eq!(ctx, Some("focus on auth".to_string()));
    }

    #[test]
    fn test_parse_slash_command_case_insensitive() {
        let result = parse_slash_command("/PLAN");
        assert!(result.is_some());
        let (cmd, _) = result.unwrap();
        assert_eq!(cmd, SlashCommand::Plan);
    }

    #[test]
    fn test_parse_slash_command_empty() {
        let result = parse_slash_command("");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_slash_command_no_slash() {
        let result = parse_slash_command("plan");
        assert!(result.is_none());
    }

    #[test]
    fn test_slash_command_from_str() {
        assert_eq!(SlashCommand::from_str("plan"), Some(SlashCommand::Plan));
        assert_eq!(SlashCommand::from_str("fix"), Some(SlashCommand::Fix));
        assert_eq!(
            SlashCommand::from_str("security"),
            Some(SlashCommand::Security)
        );
        assert_eq!(SlashCommand::from_str("docs"), Some(SlashCommand::Docs));
        assert_eq!(SlashCommand::from_str("test"), Some(SlashCommand::Tests));
        assert_eq!(SlashCommand::from_str("tests"), Some(SlashCommand::Tests));
        assert_eq!(
            SlashCommand::from_str("summarize"),
            Some(SlashCommand::Summarize)
        );
        assert_eq!(
            SlashCommand::from_str("postmortem"),
            Some(SlashCommand::Postmortem)
        );
        assert_eq!(SlashCommand::from_str("help"), Some(SlashCommand::Help));
        assert_eq!(SlashCommand::from_str("unknown"), None);
    }

    #[test]
    fn test_precanned_prompts_exist() {
        assert!(!SlashCommand::Plan.get_precanned_prompt().is_empty());
        assert!(!SlashCommand::Fix.get_precanned_prompt().is_empty());
        assert!(!SlashCommand::Security.get_precanned_prompt().is_empty());
        assert!(!SlashCommand::Docs.get_precanned_prompt().is_empty());
        assert!(!SlashCommand::Tests.get_precanned_prompt().is_empty());
        assert!(!SlashCommand::Summarize.get_precanned_prompt().is_empty());
        assert!(!SlashCommand::Postmortem.get_precanned_prompt().is_empty());
        assert!(!SlashCommand::Help.get_precanned_prompt().is_empty());
    }

    #[test]
    fn test_validate_command_for_context() {
        // Issue-only commands
        assert!(validate_command_for_context(&SlashCommand::Plan, "Issue").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Plan, "MergeRequest").is_err());
        assert!(validate_command_for_context(&SlashCommand::Fix, "Issue").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Fix, "MergeRequest").is_err());
        assert!(validate_command_for_context(&SlashCommand::Postmortem, "Issue").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Postmortem, "MergeRequest").is_err());

        // MR-only commands
        assert!(validate_command_for_context(&SlashCommand::Security, "MergeRequest").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Security, "Issue").is_err());
        assert!(validate_command_for_context(&SlashCommand::Docs, "MergeRequest").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Docs, "Issue").is_err());
        assert!(validate_command_for_context(&SlashCommand::Tests, "MergeRequest").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Tests, "Issue").is_err());

        // Universal commands
        assert!(validate_command_for_context(&SlashCommand::Summarize, "Issue").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Summarize, "MergeRequest").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Help, "Issue").is_ok());
        assert!(validate_command_for_context(&SlashCommand::Help, "MergeRequest").is_ok());
    }

    #[test]
    fn test_generate_help_message_includes_all_commands() {
        let help = generate_help_message();
        assert!(help.contains("/plan"));
        assert!(help.contains("/fix"));
        assert!(help.contains("/security"));
        assert!(help.contains("/docs"));
        assert!(help.contains("/tests"));
        assert!(help.contains("/summarize"));
        assert!(help.contains("/postmortem"));
        assert!(help.contains("/help"));
    }

    #[test]
    fn test_slash_command_equality() {
        assert_eq!(SlashCommand::Plan, SlashCommand::Plan);
        assert_ne!(SlashCommand::Plan, SlashCommand::Fix);
        assert_ne!(SlashCommand::Security, SlashCommand::Docs);
    }

    #[test]
    fn test_extract_context_after_mention() {
        let bot_name = "gitbot";
        let note = "@gitbot /plan";
        let result = extract_context_after_mention(note, bot_name);
        assert_eq!(result, Some("/plan".to_string()));

        let note = "@gitbot /plan with context";
        let result = extract_context_after_mention(note, bot_name);
        assert_eq!(result, Some("/plan with context".to_string()));

        let note = "@gitbot   ";
        let result = extract_context_after_mention(note, bot_name);
        assert_eq!(result, None);
    }
}
