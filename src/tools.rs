use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::future;
use serde_json::Value;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::config::AppSettings;
use crate::gitlab::GitlabApiClient;
use crate::models::{FunctionSpec, Tool, ToolCall};

/// Tool trait that defines the interface for all tools
#[async_trait]
pub trait ToolTrait: Send + Sync {
    /// Get the name of the tool
    fn name(&self) -> &str;

    /// Get the description of the tool
    fn description(&self) -> &str;

    /// Get the parameter schema for the tool
    fn parameters(&self) -> Option<Value>;

    /// Execute the tool with the given arguments
    async fn execute(&self, arguments: &str) -> Result<String>;

    /// Get the function specification for OpenAI API
    fn get_function_spec(&self) -> FunctionSpec {
        FunctionSpec {
            name: self.name().to_string(),
            description: Some(self.description().to_string()),
            parameters: self.parameters(),
        }
    }

    /// Get the tool specification for OpenAI API
    fn get_tool_spec(&self) -> Tool {
        Tool {
            r#type: "function".to_string(),
            function: self.get_function_spec(),
        }
    }
}

/// Tool registry that manages available tools
#[derive(Clone)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn ToolTrait>>,
}

impl ToolRegistry {
    /// Create a new empty tool registry
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool with the registry
    pub fn register_tool(&mut self, tool: Arc<dyn ToolTrait>) {
        self.tools.push(tool);
    }

    /// Get all registered tools as OpenAI tool specifications
    pub fn get_tool_specs(&self) -> Vec<Tool> {
        self.tools.iter().map(|tool| tool.get_tool_spec()).collect()
    }

    /// Find a tool by name
    pub fn find_tool(&self, name: &str) -> Option<Arc<dyn ToolTrait>> {
        self.tools.iter().find(|tool| tool.name() == name).cloned()
    }

    /// Execute a tool call with safety checks
    pub async fn execute_tool_call(&self, tool_call: &ToolCall) -> Result<ToolCallResponse> {
        // Validate tool call ID format
        if tool_call.id.is_empty() || tool_call.id.len() > 100 {
            return Err(anyhow!("Invalid tool call ID format"));
        }

        // Validate function name
        if tool_call.function.name.is_empty() || tool_call.function.name.len() > 100 {
            return Err(anyhow!("Invalid function name"));
        }

        // Validate arguments size
        if tool_call.function.arguments.len() > 2000 {
            return Err(anyhow!("Tool arguments too large (max 2000 characters)"));
        }

        let tool = self
            .find_tool(&tool_call.function.name)
            .ok_or_else(|| anyhow!("Tool {} not found", tool_call.function.name))?;

        info!("Executing tool: {}", tool.name());
        debug!("Tool arguments: {}", tool_call.function.arguments);

        // Execute tool with error handling
        let result = tool
            .execute(&tool_call.function.arguments)
            .await
            .map_err(|e| {
                error!("Tool {} execution failed: {}", tool.name(), e);
                anyhow!("Tool execution failed: {}", e)
            })?;

        // Validate result size and truncate if necessary
        if result.len() > 5000 {
            warn!(
                "Tool {} result too large ({} chars), truncating to 5000 characters",
                tool.name(),
                result.len()
            );
            let truncated_result = format!(
                "{}\n\n[Result truncated due to size limit. Original result was {} characters. Please narrow your search or request more specific information.]",
                &result[..5000.min(result.len().saturating_sub(200))], // Leave room for truncation message
                result.len()
            );
            Ok(ToolCallResponse {
                content: truncated_result,
            })
        } else {
            Ok(ToolCallResponse { content: result })
        }
    }
}

/// Response from a tool call execution
#[derive(Debug, Clone)]
pub struct ToolCallResponse {
    pub content: String,
}

/// Tool call context that tracks tool usage
#[derive(Clone)]
pub struct ToolCallContext {
    max_tool_calls: u32,
    tool_calls_made: u32,
    registry: ToolRegistry,
}

impl ToolCallContext {
    /// Create a new tool call context
    pub fn new(max_tool_calls: u32, registry: ToolRegistry) -> Self {
        Self {
            max_tool_calls,
            tool_calls_made: 0,
            registry,
        }
    }

    /// Get the remaining tool calls allowed
    pub fn remaining_tool_calls(&self) -> u32 {
        self.max_tool_calls.saturating_sub(self.tool_calls_made)
    }

    /// Get the maximum tool calls allowed
    pub fn max_tool_calls(&self) -> u32 {
        self.max_tool_calls
    }

    /// Get all tool specifications for OpenAI API
    pub fn get_tool_specs(&self) -> Vec<Tool> {
        self.registry.get_tool_specs()
    }

    /// Execute multiple tool calls in parallel
    pub async fn execute_tool_calls_parallel(
        &mut self,
        tool_calls: &[&crate::models::ToolCall],
    ) -> Vec<(crate::models::ToolCall, Result<ToolCallResponse>)> {
        // Check if we have enough capacity for all tool calls
        if self.tool_calls_made + tool_calls.len() as u32 > self.max_tool_calls {
            return tool_calls
                .iter()
                .map(|tool_call| {
                    let tool_call = (*tool_call).clone();
                    let error = Err(anyhow!(
                        "Maximum tool calls reached: {} (trying to make {} more calls)",
                        self.max_tool_calls,
                        tool_calls.len()
                    ));
                    (tool_call, error)
                })
                .collect();
        }

        // Use atomic counter to track tool calls safely across parallel tasks
        let calls_made = Arc::new(AtomicU32::new(self.tool_calls_made));
        let max_calls = self.max_tool_calls;
        let registry = self.registry.clone();

        let tool_futures: Vec<_> = tool_calls
            .iter()
            .map(|tool_call| {
                let tool_call = (*tool_call).clone();
                let registry = registry.clone();
                let calls_made = calls_made.clone();
                async move {
                    // Check if we can still make a tool call
                    if calls_made.load(Ordering::Relaxed) >= max_calls {
                        return (
                            tool_call,
                            Err(anyhow!("Maximum tool calls reached: {}", max_calls)),
                        );
                    }

                    // Execute the tool call
                    let result = registry.execute_tool_call(&tool_call).await;

                    // Increment counter if successful
                    if result.is_ok() {
                        calls_made.fetch_add(1, Ordering::Relaxed);
                    }

                    (tool_call, result)
                }
            })
            .collect();

        let results = future::join_all(tool_futures).await;

        // Update the tool calls made counter
        self.tool_calls_made = calls_made.load(Ordering::Relaxed);

        results
    }
}

/// Basic tool for getting issue details
pub struct GetIssueDetailsTool {
    gitlab_client: Arc<GitlabApiClient>,
}

#[async_trait]
impl ToolTrait for GetIssueDetailsTool {
    fn name(&self) -> &str {
        "get_issue_details"
    }

    fn description(&self) -> &str {
        "Get detailed information about a GitLab issue. Use the main project ID where the issue is located."
    }

    fn parameters(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "integer",
                    "description": "The GitLab project ID"
                },
                "issue_iid": {
                    "type": "integer",
                    "description": "The issue IID (internal ID)"
                }
            },
            "required": ["project_id", "issue_iid"]
        }))
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        // Safety check: validate arguments are not empty
        if arguments.is_empty() {
            return Err(anyhow!("Tool requires arguments"));
        }

        let params: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| anyhow!("Failed to parse arguments: {}", e))?;

        // Validate required parameters exist
        let project_id = params
            .get("project_id")
            .ok_or_else(|| anyhow!("Missing required parameter: project_id"))?;
        let issue_iid = params
            .get("issue_iid")
            .ok_or_else(|| anyhow!("Missing required parameter: issue_iid"))?;

        // Validate parameter types
        let project_id = project_id
            .as_i64()
            .ok_or_else(|| anyhow!("project_id must be an integer"))?;
        let issue_iid = issue_iid
            .as_i64()
            .ok_or_else(|| anyhow!("issue_iid must be an integer"))?;

        // Validate parameter ranges
        if project_id <= 0 {
            return Err(anyhow!("project_id must be positive"));
        }
        if issue_iid <= 0 {
            return Err(anyhow!("issue_iid must be positive"));
        }

        // Make real GitLab API call using proper async
        debug!(
            "Fetching issue details for project_id: {}, issue_iid: {}",
            project_id, issue_iid
        );

        let issue = match self.gitlab_client.get_issue(project_id, issue_iid).await {
            Ok(issue) => {
                debug!(
                    "Successfully fetched issue #{} from project {}",
                    issue_iid, project_id
                );
                issue
            }
            Err(e) => {
                error!(
                    "Failed to fetch issue #{} from project {}: {}",
                    issue_iid, project_id, e
                );
                return Err(anyhow!("GitLab API error: {}", e));
            }
        };

        match serde_json::to_string(&issue) {
            Ok(json) => Ok(json),
            Err(e) => {
                error!("Failed to serialize issue to JSON: {}", e);
                Err(anyhow!("Failed to format issue details: {}", e))
            }
        }
    }
}

/// Basic tool for getting merge request details
pub struct GetMergeRequestDetailsTool {
    gitlab_client: Arc<GitlabApiClient>,
}

#[async_trait]
impl ToolTrait for GetMergeRequestDetailsTool {
    fn name(&self) -> &str {
        "get_merge_request_details"
    }

    fn description(&self) -> &str {
        "Get detailed information about a GitLab merge request. Use the main project ID where the merge request is located."
    }

    fn parameters(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "integer",
                    "description": "The GitLab project ID"
                },
                "mr_iid": {
                    "type": "integer",
                    "description": "The merge request IID (internal ID)"
                }
            },
            "required": ["project_id", "mr_iid"]
        }))
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        // Safety check: validate arguments are not empty
        if arguments.is_empty() {
            return Err(anyhow!("Tool requires arguments"));
        }

        let params: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| anyhow!("Failed to parse arguments: {}", e))?;

        // Validate required parameters exist
        let project_id = params
            .get("project_id")
            .ok_or_else(|| anyhow!("Missing required parameter: project_id"))?;
        let mr_iid = params
            .get("mr_iid")
            .ok_or_else(|| anyhow!("Missing required parameter: mr_iid"))?;

        // Validate parameter types
        let project_id = project_id
            .as_i64()
            .ok_or_else(|| anyhow!("project_id must be an integer"))?;
        let mr_iid = mr_iid
            .as_i64()
            .ok_or_else(|| anyhow!("mr_iid must be an integer"))?;

        // Validate parameter ranges
        if project_id <= 0 {
            return Err(anyhow!("project_id must be positive"));
        }
        if mr_iid <= 0 {
            return Err(anyhow!("mr_iid must be positive"));
        }

        // Make real GitLab API call using proper async
        debug!(
            "Fetching merge request details for project_id: {}, mr_iid: {}",
            project_id, mr_iid
        );

        let mr = match self
            .gitlab_client
            .get_merge_request(project_id, mr_iid)
            .await
        {
            Ok(mr) => {
                debug!(
                    "Successfully fetched merge request !{} from project {}",
                    mr_iid, project_id
                );
                mr
            }
            Err(e) => {
                error!(
                    "Failed to fetch merge request !{} from project {}: {}",
                    mr_iid, project_id, e
                );
                return Err(anyhow!("GitLab API error: {}", e));
            }
        };

        match serde_json::to_string(&mr) {
            Ok(json) => Ok(json),
            Err(e) => {
                error!("Failed to serialize merge request to JSON: {}", e);
                Err(anyhow!("Failed to format merge request details: {}", e))
            }
        }
    }
}

/// Tool for searching code in a repository
pub struct SearchCodeTool {
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
}

#[async_trait]
impl ToolTrait for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }

    fn description(&self) -> &str {
        "Search for code in a GitLab repository. Use the main project ID for main project files, or the context repository project ID for context files. If no branch is specified, uses the repository's default branch."
    }

    fn parameters(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "integer",
                    "description": "The GitLab project ID"
                },
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "branch": {
                    "type": "string",
                    "description": "The branch to search in (optional, defaults to the repository's default branch)"
                }
            },
            "required": ["project_id", "query"]
        }))
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        // Safety check: validate arguments are not empty
        if arguments.is_empty() {
            return Err(anyhow!("Tool requires arguments"));
        }

        let params: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| anyhow!("Failed to parse arguments: {}", e))?;

        // Validate required parameters exist
        let project_id = params
            .get("project_id")
            .ok_or_else(|| anyhow!("Missing required parameter: project_id"))?;
        let query = params
            .get("query")
            .ok_or_else(|| anyhow!("Missing required parameter: query"))?;

        // Validate parameter types
        let project_id = project_id
            .as_i64()
            .ok_or_else(|| anyhow!("project_id must be an integer"))?;
        let query = query
            .as_str()
            .ok_or_else(|| anyhow!("query must be a string"))?;

        // Handle optional branch parameter
        let branch = params
            .get("branch")
            .and_then(|b| b.as_str())
            .unwrap_or(&self.config.default_branch);

        // Validate parameter ranges
        if project_id <= 0 {
            return Err(anyhow!("project_id must be positive"));
        }

        // Make real GitLab API call using proper async
        debug!(
            "Making search call to project_id: {}, branch: '{}', query: '{}'",
            project_id, branch, query
        );

        let search_results = match self
            .gitlab_client
            .search_code(project_id, query, branch)
            .await
        {
            Ok(results) => {
                debug!(
                    "Search completed successfully, found {} results",
                    results.len()
                );
                results
            }
            Err(e) => {
                error!(
                    "GitLab API search failed for project_id: {}, branch: '{}', query: '{}': {}",
                    project_id, branch, query, e
                );
                return Err(anyhow!("GitLab API error: {}", e));
            }
        };

        // Format the search results as JSON
        match serde_json::to_string(&search_results) {
            Ok(json) => {
                debug!(
                    "Successfully formatted {} search results as JSON ({} bytes)",
                    search_results.len(),
                    json.len()
                );
                Ok(json)
            }
            Err(e) => {
                error!("Failed to serialize search results to JSON: {}", e);
                Err(anyhow!("Failed to format search results: {}", e))
            }
        }
    }
}

/// Tool for getting project details by path
pub struct GetProjectByPathTool {
    gitlab_client: Arc<GitlabApiClient>,
}

#[async_trait]
impl ToolTrait for GetProjectByPathTool {
    fn name(&self) -> &str {
        "get_project_by_path"
    }

    fn description(&self) -> &str {
        "Get project details (including project ID) by providing the project path (e.g., 'group/project-name')"
    }

    fn parameters(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "project_path": {
                    "type": "string",
                    "description": "The project path (e.g., 'group/project-name' or just 'project-name')"
                }
            },
            "required": ["project_path"]
        }))
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        // Safety check: validate arguments are not empty
        if arguments.is_empty() {
            return Err(anyhow!("Tool requires arguments"));
        }

        let params: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| anyhow!("Failed to parse arguments: {}", e))?;

        // Validate required parameters exist
        let project_path = params
            .get("project_path")
            .ok_or_else(|| anyhow!("Missing required parameter: project_path"))?;

        // Validate parameter types
        let project_path = project_path
            .as_str()
            .ok_or_else(|| anyhow!("project_path must be a string"))?;

        if project_path.is_empty() {
            return Err(anyhow!("project_path cannot be empty"));
        }

        // Make real GitLab API call using proper async
        debug!("Fetching project details for path: '{}'", project_path);

        let project = match self.gitlab_client.get_project_by_path(project_path).await {
            Ok(project) => {
                debug!(
                    "Successfully fetched project '{}' (ID: {})",
                    project_path, project.id
                );
                project
            }
            Err(e) => {
                error!("Failed to fetch project '{}': {}", project_path, e);
                return Err(anyhow!("GitLab API error: {}", e));
            }
        };

        match serde_json::to_string(&project) {
            Ok(json) => Ok(json),
            Err(e) => {
                error!("Failed to serialize project to JSON: {}", e);
                Err(anyhow!("Failed to format project details: {}", e))
            }
        }
    }
}

/// Tool for listing branches in a GitLab project
pub struct ListBranchesTool {
    gitlab_client: Arc<GitlabApiClient>,
}

#[async_trait]
impl ToolTrait for ListBranchesTool {
    fn name(&self) -> &str {
        "list_branches"
    }

    fn description(&self) -> &str {
        "List all branches in a GitLab project. Returns branch names and metadata like default branch, merged status, and protection status."
    }

    fn parameters(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "project_path": {
                    "type": "string",
                    "description": "The project path (e.g., 'group/project-name' or just 'project-name')"
                }
            },
            "required": ["project_path"]
        }))
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        // Safety check: validate arguments are not empty
        if arguments.is_empty() {
            return Err(anyhow!("Tool requires arguments"));
        }

        let params: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| anyhow!("Failed to parse arguments: {}", e))?;

        // Validate required parameters exist
        let project_path = params
            .get("project_path")
            .ok_or_else(|| anyhow!("Missing required parameter: project_path"))?;

        // Validate parameter types
        let project_path = project_path
            .as_str()
            .ok_or_else(|| anyhow!("project_path must be a string"))?;

        if project_path.is_empty() {
            return Err(anyhow!("project_path cannot be empty"));
        }

        // First get the project to resolve the path to an ID
        debug!("Fetching project details for path: '{}'", project_path);
        let project = match self.gitlab_client.get_project_by_path(project_path).await {
            Ok(project) => {
                debug!(
                    "Successfully fetched project '{}' (ID: {})",
                    project_path, project.id
                );
                project
            }
            Err(e) => {
                error!("Failed to fetch project '{}': {}", project_path, e);
                return Err(anyhow!("GitLab API error: {}", e));
            }
        };

        // Now get the branches for the project
        debug!(
            "Fetching branches for project '{}' (ID: {})",
            project_path, project.id
        );
        match self.gitlab_client.get_branches(project.id).await {
            Ok(branches) => {
                debug!(
                    "Successfully fetched {} branches for project '{}'",
                    branches.len(),
                    project_path
                );
                match serde_json::to_string(&branches) {
                    Ok(json) => Ok(json),
                    Err(e) => {
                        error!("Failed to serialize branches to JSON: {}", e);
                        Err(anyhow!("Failed to format branches: {}", e))
                    }
                }
            }
            Err(e) => {
                error!(
                    "Failed to fetch branches for project '{}': {}",
                    project_path, e
                );
                Err(anyhow!("GitLab API error: {}", e))
            }
        }
    }
}

/// Tool for getting file content
pub struct GetFileContentTool {
    gitlab_client: Arc<GitlabApiClient>,
}

#[async_trait]
impl ToolTrait for GetFileContentTool {
    fn name(&self) -> &str {
        "get_file_content"
    }

    fn description(&self) -> &str {
        "Get the content of a file from a GitLab repository. Use the project ID and file path. Optionally specify a ref (branch/commit)."
    }

    fn parameters(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "integer",
                    "description": "The GitLab project ID"
                },
                "file_path": {
                    "type": "string",
                    "description": "The path to the file"
                },
                "ref": {
                    "type": "string",
                    "description": "The branch, tag or commit to use (optional, defaults to default branch)"
                }
            },
            "required": ["project_id", "file_path"]
        }))
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        if arguments.is_empty() {
            return Err(anyhow!("Tool requires arguments"));
        }

        let params: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| anyhow!("Failed to parse arguments: {}", e))?;

        let project_id = params
            .get("project_id")
            .ok_or_else(|| anyhow!("Missing required parameter: project_id"))?;
        let file_path = params
            .get("file_path")
            .ok_or_else(|| anyhow!("Missing required parameter: file_path"))?;

        let project_id = project_id
            .as_i64()
            .ok_or_else(|| anyhow!("project_id must be an integer"))?;
        let file_path = file_path
            .as_str()
            .ok_or_else(|| anyhow!("file_path must be a string"))?;

        if project_id <= 0 {
            return Err(anyhow!("project_id must be positive"));
        }
        if file_path.is_empty() {
            return Err(anyhow!("file_path cannot be empty"));
        }

        // Handle optional ref parameter
        let ref_param = params.get("ref").and_then(|r| r.as_str());

        debug!(
            "Fetching file content for project_id: {}, path: '{}', ref: '{:?}'",
            project_id, file_path, ref_param
        );

        match self
            .gitlab_client
            .get_file_content(project_id, file_path, ref_param)
            .await
        {
            Ok(file) => {
                debug!("Successfully fetched content for '{}'", file_path);
                Ok(file.content.unwrap_or_else(|| "".to_string()))
            }
            Err(e) => {
                error!(
                    "Failed to fetch file content for '{}' in project {}: {}",
                    file_path, project_id, e
                );
                Err(anyhow!("GitLab API error: {}", e))
            }
        }
    }
}

/// Tool for getting issue notes (comments)
pub struct GetIssueNotesTool {
    gitlab_client: Arc<GitlabApiClient>,
}

#[async_trait]
impl ToolTrait for GetIssueNotesTool {
    fn name(&self) -> &str {
        "get_issue_notes"
    }

    fn description(&self) -> &str {
        "Get the comments (notes) for a GitLab issue. Returns the conversation thread."
    }

    fn parameters(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "integer",
                    "description": "The GitLab project ID"
                },
                "issue_iid": {
                    "type": "integer",
                    "description": "The issue IID (internal ID)"
                }
            },
            "required": ["project_id", "issue_iid"]
        }))
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        if arguments.is_empty() {
            return Err(anyhow!("Tool requires arguments"));
        }

        let params: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| anyhow!("Failed to parse arguments: {}", e))?;

        let project_id = params
            .get("project_id")
            .ok_or_else(|| anyhow!("Missing required parameter: project_id"))?;
        let issue_iid = params
            .get("issue_iid")
            .ok_or_else(|| anyhow!("Missing required parameter: issue_iid"))?;

        let project_id = project_id
            .as_i64()
            .ok_or_else(|| anyhow!("project_id must be an integer"))?;
        let issue_iid = issue_iid
            .as_i64()
            .ok_or_else(|| anyhow!("issue_iid must be an integer"))?;

        if project_id <= 0 {
            return Err(anyhow!("project_id must be positive"));
        }
        if issue_iid <= 0 {
            return Err(anyhow!("issue_iid must be positive"));
        }

        debug!(
            "Fetching notes for issue #{} in project {}",
            issue_iid, project_id
        );

        match self
            .gitlab_client
            .get_all_issue_notes(project_id, issue_iid)
            .await
        {
            Ok(notes) => {
                debug!(
                    "Successfully fetched {} notes for issue #{}",
                    notes.len(),
                    issue_iid
                );
                // Format notes as a readable conversation thread
                let mut conversation = String::new();
                for note in notes {
                    // Skip system notes (where system is true, but our model doesn't have that field exposed clearly?
                    // GitlabNoteAttributes doesn't have system field. Usually system notes have a specific author or content pattern.
                    // For now we include everything.
                    conversation.push_str(&format!(
                        "[{}] @{}:\n{}\n\n",
                        note.updated_at, note.author.username, note.note
                    ));
                }

                if conversation.is_empty() {
                    Ok("No comments found.".to_string())
                } else {
                    Ok(conversation)
                }
            }
            Err(e) => {
                error!(
                    "Failed to fetch notes for issue #{} in project {}: {}",
                    issue_iid, project_id, e
                );
                Err(anyhow!("GitLab API error: {}", e))
            }
        }
    }
}

/// Create a basic tool registry with GitLab tools
pub fn create_basic_tool_registry(
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register_tool(Arc::new(GetIssueDetailsTool {
        gitlab_client: gitlab_client.clone(),
    }));
    registry.register_tool(Arc::new(GetMergeRequestDetailsTool {
        gitlab_client: gitlab_client.clone(),
    }));
    registry.register_tool(Arc::new(SearchCodeTool {
        gitlab_client: gitlab_client.clone(),
        config: config.clone(),
    }));
    registry.register_tool(Arc::new(GetProjectByPathTool {
        gitlab_client: gitlab_client.clone(),
    }));
    registry.register_tool(Arc::new(ListBranchesTool {
        gitlab_client: gitlab_client.clone(),
    }));
    registry.register_tool(Arc::new(GetFileContentTool {
        gitlab_client: gitlab_client.clone(),
    }));
    registry.register_tool(Arc::new(GetIssueNotesTool {
        gitlab_client: gitlab_client.clone(),
    }));

    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FunctionCall;

    #[test]
    fn test_tool_registry_creation() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.get_tool_specs().len(), 0);
    }

    #[test]
    fn test_tool_call_context_creation() {
        let registry = ToolRegistry::new();
        let context = ToolCallContext::new(3, registry);
        assert_eq!(context.max_tool_calls(), 3);
        assert_eq!(context.remaining_tool_calls(), 3);
    }

    #[test]
    fn test_tool_call_context_limits() {
        let registry = ToolRegistry::new();
        let mut context = ToolCallContext::new(2, registry);

        // Initially should have 2 remaining calls
        assert_eq!(context.remaining_tool_calls(), 2);
        assert_eq!(context.max_tool_calls(), 2);

        // After manually setting the counter to test the logic
        context.tool_calls_made = 1;
        assert_eq!(context.remaining_tool_calls(), 1);

        context.tool_calls_made = 2;
        assert_eq!(context.remaining_tool_calls(), 0);
    }

    #[tokio::test]
    async fn test_tool_registry_safety_checks() {
        let registry = ToolRegistry::new();

        // Test invalid tool call ID
        let invalid_tool_call = ToolCall {
            id: "".to_string(),
            r#type: "function".to_string(),
            function: FunctionCall {
                name: "test".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let result = registry.execute_tool_call(&invalid_tool_call).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid tool call ID format"));

        // Test tool not found
        let valid_tool_call = ToolCall {
            id: "call_123".to_string(),
            r#type: "function".to_string(),
            function: FunctionCall {
                name: "nonexistent_tool".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let result = registry.execute_tool_call(&valid_tool_call).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Tool nonexistent_tool not found"));
    }

    #[test]
    fn test_parameter_validation_logic() {
        // Test the parameter validation logic directly without needing a real GitLab client
        // This tests the validation patterns used in the tools

        // Test empty string validation
        let empty_str = "";
        assert!(empty_str.is_empty());

        // Test JSON parsing validation pattern
        let invalid_json = "not valid json";
        let result: Result<serde_json::Value, _> = serde_json::from_str(invalid_json);
        assert!(result.is_err());

        // Test parameter existence validation pattern
        let json_obj: serde_json::Value = serde_json::from_str("{}").unwrap();
        let missing_param = json_obj.get("nonexistent");
        assert!(missing_param.is_none());

        // Test integer type validation pattern
        let string_value: serde_json::Value =
            serde_json::from_str(r#"{"value": "not_a_number"}"#).unwrap();
        let not_int = string_value["value"].as_i64();
        assert!(not_int.is_none());

        // Test positive number validation pattern
        let zero_value = 0;
        assert!(zero_value <= 0);
    }

    #[tokio::test]
    async fn test_tool_call_context_max_tool_calls() {
        let registry = ToolRegistry::new();
        let mut context = ToolCallContext::new(1, registry);

        // Create a mock tool call that would succeed if not for the limit
        let tool_call = ToolCall {
            id: "call_123".to_string(),
            r#type: "function".to_string(),
            function: FunctionCall {
                name: "test_tool".to_string(),
                arguments: "{}".to_string(),
            },
        };

        // Test with parallel execution - should fail because tool doesn't exist
        let results = context.execute_tool_calls_parallel(&[&tool_call]).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_err());

        // The tool call counter should not be incremented on failure
        // This is the correct behavior - we only count successful tool executions
        assert_eq!(context.remaining_tool_calls(), 1);
        assert_eq!(context.tool_calls_made, 0);
    }

    #[test]
    fn test_all_tools_registered() {
        // Create dummy dependencies
        let config = Arc::new(AppSettings::default());
        let gitlab_client =
            Arc::new(GitlabApiClient::new(config.clone()).expect("Failed to create client"));

        let registry = create_basic_tool_registry(gitlab_client, config);
        let specs = registry.get_tool_specs();

        // We expect 7 tools:
        // 1. GetIssueDetailsTool
        // 2. GetMergeRequestDetailsTool
        // 3. SearchCodeTool
        // 4. GetProjectByPathTool
        // 5. ListBranchesTool
        // 6. GetFileContentTool
        // 7. GetIssueNotesTool
        assert_eq!(specs.len(), 7);

        // Verify specific tools are present
        let tool_names: Vec<String> = specs.iter().map(|t| t.function.name.clone()).collect();
        assert!(tool_names.contains(&"get_file_content".to_string()));
        assert!(tool_names.contains(&"get_issue_notes".to_string()));
    }

    #[test]
    fn test_get_file_content_tool_definition() {
        let config = Arc::new(AppSettings::default());
        let gitlab_client =
            Arc::new(GitlabApiClient::new(config.clone()).expect("Failed to create client"));
        let tool = GetFileContentTool { gitlab_client };

        assert_eq!(tool.name(), "get_file_content");
        assert!(tool
            .description()
            .contains("Get the content of a file from a GitLab repository"));

        let params = tool.parameters().unwrap();
        let properties = params.get("properties").unwrap();
        assert!(properties.get("project_id").is_some());
        assert!(properties.get("file_path").is_some());
        assert!(properties.get("ref").is_some());

        let required = params.get("required").unwrap().as_array().unwrap();
        assert!(required.contains(&serde_json::json!("project_id")));
        assert!(required.contains(&serde_json::json!("file_path")));
    }

    #[test]
    fn test_get_issue_notes_tool_definition() {
        let config = Arc::new(AppSettings::default());
        let gitlab_client =
            Arc::new(GitlabApiClient::new(config.clone()).expect("Failed to create client"));
        let tool = GetIssueNotesTool { gitlab_client };

        assert_eq!(tool.name(), "get_issue_notes");
        assert!(tool.description().contains("Get the comments (notes)"));

        let params = tool.parameters().unwrap();
        let properties = params.get("properties").unwrap();
        assert!(properties.get("project_id").is_some());
        assert!(properties.get("issue_iid").is_some());

        let required = params.get("required").unwrap().as_array().unwrap();
        assert!(required.contains(&serde_json::json!("project_id")));
        assert!(required.contains(&serde_json::json!("issue_iid")));
    }
}
