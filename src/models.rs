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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabNoteAttributes {
    pub id: i64,
    pub note: String, // the content of the comment
    pub author_id: i64,
    pub author: GitlabUser, // Added for polling model
    pub project_id: i64,
    pub noteable_type: String, // e.g., "Issue", "MergeRequest", "Snippet"
    pub noteable_id: Option<i64>, // The ID of the Issue or MR if noteable_type is Issue or MergeRequest
    pub iid: Option<i64>, // The IID of the noteable, e.g. issue iid or mr iid.
    pub url: String, // URL to the comment
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
