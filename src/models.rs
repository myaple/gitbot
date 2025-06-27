use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabUser {
    pub id: i64,
    pub username: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabProject {
    pub id: i64,
    pub path_with_namespace: String,
    pub web_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabIssue {
    pub id: i64,
    pub iid: i64, // internal ID, unique within a project
    pub project_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub state: String, // e.g., "opened", "closed"
    pub author: GitlabUser,
    pub web_url: String,
    pub labels: Vec<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabMergeRequest {
    pub id: i64,
    pub iid: i64,
    pub project_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub state: String, // e.g., "opened", "merged", "closed"
    pub author: GitlabUser,
    pub source_branch: String,
    pub target_branch: String,
    pub web_url: String,
    pub labels: Vec<String>,
    pub detailed_merge_status: Option<String>, // e.g. "mergeable", "broken_status" - sometimes called merge_status
    pub updated_at: String,
    pub head_pipeline: Option<GitlabPipeline>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabPipeline {
    pub id: i64,
    pub iid: i64,
    pub project_id: i64,
    pub status: String,
    pub source: Option<String>,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub sha: String,
    pub web_url: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabNoteAttributes {
    pub id: i64,
    #[serde(alias = "body")]
    pub note: String, // the content of the comment, GitLab API uses "body"
    pub author: GitlabUser, // Added for polling model
    pub project_id: i64,
    pub noteable_type: String, // e.g., "Issue", "MergeRequest", "Snippet"
    pub noteable_id: Option<i64>, // The ID of the Issue or MR if noteable_type is Issue or MergeRequest
    pub iid: Option<i64>,         // The IID of the noteable, e.g. issue iid or mr iid.
    pub url: Option<String>, // URL to the comment - GitLab API for notes might not always provide this directly
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabNoteObject {
    pub id: i64,
    pub iid: i64,
    pub title: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabNoteEvent {
    pub object_kind: String, // should be "note"
    pub event_type: String,  // should be "note"
    pub user: GitlabUser,    // user who triggered the event, i.e., wrote the comment
    pub project: GitlabProject,
    pub object_attributes: GitlabNoteAttributes,
    pub issue: Option<GitlabNoteObject>, // present if note is on an issue
    pub merge_request: Option<GitlabNoteObject>, // present if note is on a merge request
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAIChatMessage {
    pub role: String, // e.g., "system", "user", "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAIChatRequest {
    pub model: String, // e.g., "gpt-3.5-turbo"
    pub messages: Vec<OpenAIChatMessage>,
    pub temperature: Option<f32>, // e.g., 0.7
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAIChatChoice {
    pub index: u32,
    pub message: OpenAIChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: Option<u32>,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAIChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OpenAIChatChoice>,
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabCommit {
    pub id: String,
    pub short_id: String,
    pub title: String,
    pub author_name: String,
    pub author_email: String,
    pub authored_date: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committed_date: String,
    pub message: String,
}

// Structured output for /summarize command
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SummarizeOutput {
    pub adherence_to_guidelines: String,
    pub performance_impact: String,
    pub strengths: String,
    pub areas_for_improvement: String,
    pub conclusion_and_recommendations: String,
}

// Structured output for /postmortem command
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostmortemOutput {
    pub timeline: String,
    pub root_cause_analysis: String,
}

// Structured output for /suggestions command
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SuggestionsOutput {
    pub suggested_solution: String,
}

// Description of a single command for /help
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommandDescription {
    pub name: String,
    pub description: String,
}

// Structured output for /help command
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HelpOutput {
    pub available_commands: Vec<CommandDescription>,
    pub usage_instructions: String,
}

// Enum to wrap all possible structured output types
// This will be useful for the return type of functions that parse the LLM response.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)] // Allows deserializing into the first matching variant
pub enum StructuredLlMResponse {
    Summary(SummarizeOutput),
    Postmortem(PostmortemOutput),
    Suggestions(SuggestionsOutput),
    Help(HelpOutput),
    // Potentially add a variant for plain text fallback if needed
    // PlainText(String),
}

// Enum to represent either a structured response or a plain text string from the LLM
#[derive(Debug, Clone)]
pub enum LLMResponse {
    Structured(StructuredLlMResponse),
    Plain(String),
}
