use crate::config::AppSettings;
use crate::gitlab::GitlabApiClient;
use crate::handlers::process_mention;
use crate::models::{GitlabIssue, GitlabMergeRequest, GitlabNoteEvent, GitlabNoteObject, GitlabProject, GitlabUser, GitlabNoteAttributes};
use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::time;
use tracing::{debug, error, info};

pub struct PollingService {
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
    last_checked: Arc<Mutex<u64>>,
}

impl PollingService {
    pub fn new(gitlab_client: Arc<GitlabApiClient>, config: Arc<AppSettings>) -> Self {
        // Initialize with current time minus 1 hour to check recent activity on startup
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();
        
        let initial_time = now.saturating_sub(3600); // 1 hour ago
        
        Self {
            gitlab_client,
            config,
            last_checked: Arc::new(Mutex::new(initial_time)),
        }
    }

    pub async fn start_polling(&self) -> Result<()> {
        info!("Starting polling service for repositories: {:?}", self.config.repos_to_poll);
        
        let interval_duration = Duration::from_secs(self.config.poll_interval_seconds);
        let mut interval = time::interval(interval_duration);

        loop {
            interval.tick().await;
            if let Err(e) = self.poll_repositories().await {
                error!("Error polling repositories: {}", e);
            }
        }
    }

    async fn poll_repositories(&self) -> Result<()> {
        let mut last_checked = self.last_checked.lock().await;
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();
        
        info!("Polling repositories since timestamp: {}", *last_checked);

        for repo_path in &self.config.repos_to_poll {
            if let Err(e) = self.poll_repository(repo_path, *last_checked).await {
                error!("Error polling repository {}: {}", repo_path, e);
            }
        }

        // Update last checked time
        *last_checked = current_time;
        Ok(())
    }

    async fn poll_repository(&self, repo_path: &str, since_timestamp: u64) -> Result<()> {
        info!("Polling repository: {}", repo_path);
        
        // Get project ID from path
        let project = self.gitlab_client.get_project_by_path(repo_path).await?;
        let project_id = project.id;
        
        // Poll issues
        self.poll_issues(project_id, since_timestamp, &project).await?;
        
        // Poll merge requests
        self.poll_merge_requests(project_id, since_timestamp, &project).await?;
        
        Ok(())
    }

    async fn poll_issues(&self, project_id: i64, since_timestamp: u64, project: &GitlabProject) -> Result<()> {
        debug!("Polling issues for project ID: {}", project_id);
        
        // Get issues updated since last check
        let issues = self.gitlab_client.get_issues(project_id, since_timestamp).await?;
        
        for issue in issues {
            // Get notes for this issue
            let notes = self.gitlab_client.get_issue_notes(project_id, issue.iid, since_timestamp).await?;
            
            for note in notes {
                // Skip notes by the bot itself
                if note.author.username == self.config.bot_username {
                    continue;
                }
                
                // Check if note mentions the bot
                if note.note.contains(&format!("@{}", self.config.bot_username)) {
                    info!("Found mention in issue #{} note #{}", issue.iid, note.id);
                    
                    // Create a GitlabNoteEvent from the note
                    let event = self.create_issue_note_event(project.clone(), note, issue.clone());
                    
                    // Process the mention
                    if let Err(e) = process_mention(
                        event,
                        self.gitlab_client.clone(),
                        self.config.clone(),
                    ).await {
                        error!("Error processing mention: {}", e);
                    }
                }
            }
        }
        
        Ok(())
    }

    async fn poll_merge_requests(&self, project_id: i64, since_timestamp: u64, project: &GitlabProject) -> Result<()> {
        debug!("Polling merge requests for project ID: {}", project_id);
        
        // Get merge requests updated since last check
        let merge_requests = self.gitlab_client.get_merge_requests(project_id, since_timestamp).await?;
        
        for mr in merge_requests {
            // Get notes for this merge request
            let notes = self.gitlab_client.get_merge_request_notes(project_id, mr.iid, since_timestamp).await?;
            
            for note in notes {
                // Skip notes by the bot itself
                if note.author.username == self.config.bot_username {
                    continue;
                }
                
                // Check if note mentions the bot
                if note.note.contains(&format!("@{}", self.config.bot_username)) {
                    info!("Found mention in MR !{} note #{}", mr.iid, note.id);
                    
                    // Create a GitlabNoteEvent from the note
                    let event = self.create_mr_note_event(project.clone(), note, mr.clone());
                    
                    // Process the mention
                    if let Err(e) = process_mention(
                        event,
                        self.gitlab_client.clone(),
                        self.config.clone(),
                    ).await {
                        error!("Error processing mention: {}", e);
                    }
                }
            }
        }
        
        Ok(())
    }

    fn create_issue_note_event(&self, project: GitlabProject, note: GitlabNoteAttributes, issue: GitlabIssue) -> GitlabNoteEvent {
        // Clone the author data to avoid ownership issues
        let author = GitlabUser {
            id: note.author_id,
            username: note.author.username.clone(),
            name: note.author.name.clone(),
            avatar_url: note.author.avatar_url.clone(),
        };
        
        let issue_object = GitlabNoteObject {
            id: issue.id,
            iid: issue.iid,
            title: issue.title.clone(),
            description: issue.description.clone(),
        };
        
        GitlabNoteEvent {
            object_kind: "note".to_string(),
            event_type: "note".to_string(),
            user: author,
            project,
            object_attributes: note,
            issue: Some(issue_object),
            merge_request: None,
        }
    }

    fn create_mr_note_event(&self, project: GitlabProject, note: GitlabNoteAttributes, mr: GitlabMergeRequest) -> GitlabNoteEvent {
        // Clone the author data to avoid ownership issues
        let author = GitlabUser {
            id: note.author_id,
            username: note.author.username.clone(),
            name: note.author.name.clone(),
            avatar_url: note.author.avatar_url.clone(),
        };
        
        let mr_object = GitlabNoteObject {
            id: mr.id,
            iid: mr.iid,
            title: mr.title.clone(),
            description: mr.description.clone(),
        };
        
        GitlabNoteEvent {
            object_kind: "note".to_string(),
            event_type: "note".to_string(),
            user: author,
            project,
            object_attributes: note,
            issue: None,
            merge_request: Some(mr_object),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito;

    fn create_test_settings(base_url: String) -> AppSettings {
        AppSettings {
            gitlab_url: base_url,
            gitlab_token: "test_token".to_string(),
            openai_api_key: "key".to_string(),
            openai_custom_url: "url".to_string(),
            repos_to_poll: vec!["org/repo1".to_string()],
            log_level: "debug".to_string(),
            bot_username: "gitbot".to_string(),
            poll_interval_seconds: 60,
        }
    }

    #[tokio::test]
    async fn test_polling_service_creation() {
        let server = mockito::Server::new_async().await;
        let base_url = server.url();
        
        let settings = create_test_settings(base_url);
        let gitlab_client = GitlabApiClient::new(&settings).unwrap();
        
        let polling_service = PollingService::new(
            Arc::new(gitlab_client),
            Arc::new(settings),
        );
        
        // Verify initial last_checked time is set
        let last_checked = *polling_service.last_checked.lock().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
            
        // Should be initialized to approximately 1 hour ago
        assert!(now - last_checked >= 3500 && now - last_checked <= 3700);
    }
}