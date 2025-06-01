use crate::config::AppSettings;
use crate::file_indexer::FileIndexManager;
use crate::gitlab::GitlabApiClient;
use crate::handlers::process_mention;
use crate::mention_cache::MentionCache;
use crate::models::{
    GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes, GitlabNoteEvent, GitlabNoteObject,
    GitlabProject, GitlabUser,
};
use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::time;
use tracing::{debug, error, info, warn};

#[derive(Clone)]
pub struct PollingService {
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
    pub(crate) last_checked: Arc<Mutex<u64>>,
    processed_mentions_cache: MentionCache,
    file_index_manager: Arc<FileIndexManager>,
}

impl PollingService {
    pub fn new(
        gitlab_client: Arc<GitlabApiClient>,
        config: Arc<AppSettings>,
        file_index_manager: Arc<FileIndexManager>,
    ) -> Self {
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
            processed_mentions_cache: MentionCache::new(),
            file_index_manager,
        }
    }

    pub async fn start_polling(&self) -> Result<()> {
        info!(
            "Starting polling service for repositories: {:?}",
            self.config.repos_to_poll
        );

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

        // Create a vector of futures for parallel execution
        let mut polling_tasks = Vec::new();

        // Create a future for each repository
        for repo_path in &self.config.repos_to_poll {
            let repo_path_clone = repo_path.clone();
            let timestamp = *last_checked;
            let self_clone = self.clone();

            // Create a future that polls a single repository
            let task = tokio::spawn(async move {
                if let Err(e) = self_clone
                    .poll_repository(&repo_path_clone, timestamp)
                    .await
                {
                    error!("Error polling repository {}: {}", repo_path_clone, e);
                }
            });

            polling_tasks.push(task);
        }

        // Wait for all polling tasks to complete
        for task in polling_tasks {
            if let Err(e) = task.await {
                error!("Task join error: {}", e);
            }
        }

        // Update last checked time
        *last_checked = current_time;
        Ok(())
    }

    pub async fn poll_repository(&self, repo_path: &str, since_timestamp: u64) -> Result<()> {
        info!("Polling repository: {}", repo_path);

        // Get project ID from path
        let project = self.gitlab_client.get_project_by_path(repo_path).await?;
        let project_id = project.id;

        // Calculate the timestamp for max_age_hours
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();

        // Convert hours to seconds and subtract from current time
        let max_age_seconds = self.config.max_age_hours * 3600;
        let max_age_timestamp = now.saturating_sub(max_age_seconds);

        // Use the less recent of since_timestamp and max_age_timestamp
        let effective_timestamp = std::cmp::min(since_timestamp, max_age_timestamp);

        info!(
            "Using effective timestamp: {} (max age: {} hours)",
            effective_timestamp, self.config.max_age_hours
        );

        // Create tasks for polling issues and merge requests in parallel
        let issues_task = {
            let self_clone = self.clone();
            let project_clone = project.clone();
            tokio::spawn(async move {
                if let Err(e) = self_clone
                    .poll_issues(project_id, effective_timestamp, &project_clone)
                    .await
                {
                    error!("Error polling issues for project {}: {}", project_id, e);
                }
            })
        };

        let mrs_task = {
            let self_clone = self.clone();
            let project_clone = project.clone();
            tokio::spawn(async move {
                if let Err(e) = self_clone
                    .poll_merge_requests(project_id, effective_timestamp, &project_clone)
                    .await
                {
                    error!(
                        "Error polling merge requests for project {}: {}",
                        project_id, e
                    );
                }
            })
        };

        // Wait for both tasks to complete
        if let Err(e) = issues_task.await {
            error!("Task join error for issues polling: {}", e);
        }

        if let Err(e) = mrs_task.await {
            error!("Task join error for merge requests polling: {}", e);
        }

        // Task for checking stale issues
        let stale_check_task = {
            let self_clone = self.clone();
            let project_id_clone = project_id; // project_id is already i64
            let gitlab_client_clone = self_clone.gitlab_client.clone();
            let config_clone = self_clone.config.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    check_stale_issues(project_id_clone, gitlab_client_clone, config_clone).await
                {
                    error!(
                        "Error checking stale issues for project {}: {}",
                        project_id_clone, e
                    );
                }
            })
        };

        if let Err(e) = stale_check_task.await {
            error!("Task join error for stale issue checking: {}", e);
        }

        Ok(())
    }

    async fn poll_issues(
        &self,
        project_id: i64,
        since_timestamp: u64,
        project: &GitlabProject,
    ) -> Result<()> {
        debug!("Polling issues for project ID: {}", project_id);

        // Get issues updated since last check
        let issues = self
            .gitlab_client
            .get_issues(project_id, since_timestamp)
            .await?;

        for issue in issues {
            // Get notes for this issue
            let notes = self
                .gitlab_client
                .get_issue_notes(project_id, issue.iid, since_timestamp)
                .await?;

            for note in notes {
                // Skip notes by the bot itself
                if note.author.username == self.config.bot_username {
                    continue;
                }

                // Check if note mentions the bot
                if note
                    .note
                    .contains(&format!("@{}", self.config.bot_username))
                {
                    info!("Found mention in issue #{} note #{}", issue.iid, note.id);

                    // Create a GitlabNoteEvent from the note
                    let event = self.create_issue_note_event(project.clone(), note, issue.clone());

                    // Process the mention
                    if let Err(e) = process_mention(
                        event,
                        self.gitlab_client.clone(),
                        self.config.clone(),
                        &self.processed_mentions_cache,
                        self.file_index_manager.clone(),
                    )
                    .await
                    {
                        error!("Error processing mention: {}", e);
                    }
                }
            }
        }

        Ok(())
    }

    async fn poll_merge_requests(
        &self,
        project_id: i64,
        since_timestamp: u64,
        project: &GitlabProject,
    ) -> Result<()> {
        debug!("Polling merge requests for project ID: {}", project_id);

        // Get merge requests updated since last check
        let merge_requests = self
            .gitlab_client
            .get_merge_requests(project_id, since_timestamp)
            .await?;

        for mr in merge_requests {
            // Get notes for this merge request
            let notes = self
                .gitlab_client
                .get_merge_request_notes(project_id, mr.iid, since_timestamp)
                .await?;

            for note in notes {
                // Skip notes by the bot itself
                if note.author.username == self.config.bot_username {
                    continue;
                }

                // Check if note mentions the bot
                if note
                    .note
                    .contains(&format!("@{}", self.config.bot_username))
                {
                    info!("Found mention in MR !{} note #{}", mr.iid, note.id);

                    // Create a GitlabNoteEvent from the note
                    let event = self.create_mr_note_event(project.clone(), note, mr.clone());

                    // Process the mention
                    if let Err(e) = process_mention(
                        event,
                        self.gitlab_client.clone(),
                        self.config.clone(),
                        &self.processed_mentions_cache,
                        self.file_index_manager.clone(),
                    )
                    .await
                    {
                        error!("Error processing mention: {}", e);
                    }
                }
            }
        }

        Ok(())
    }

    fn create_issue_note_event(
        &self,
        project: GitlabProject,
        note: GitlabNoteAttributes,
        issue: GitlabIssue,
    ) -> GitlabNoteEvent {
        // Clone the author data to avoid ownership issues
        let author = GitlabUser {
            id: note.author.id,
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

    fn create_mr_note_event(
        &self,
        project: GitlabProject,
        note: GitlabNoteAttributes,
        mr: GitlabMergeRequest,
    ) -> GitlabNoteEvent {
        // Clone the author data to avoid ownership issues
        let author = GitlabUser {
            id: note.author.id,
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

pub(crate) async fn check_stale_issues(
    project_id: i64,
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
) -> Result<()> {
    info!("Checking for stale issues in project ID: {}", project_id);
    let stale_label_name = "stale"; // Define the label name

    // Fetch all issues (or a broad set by passing 0 as since_timestamp)
    // We will filter for "opened" state client-side.
    let all_issues = gitlab_client.get_issues(project_id, 0).await?;
    let open_issues = all_issues
        .into_iter()
        .filter(|issue| issue.state == "opened");

    for issue in open_issues {
        debug!("Processing issue #{} for staleness", issue.iid);

        let mut last_activity_ts: Option<DateTime<Utc>> = None;

        // Start with the issue's own updated_at timestamp
        match DateTime::parse_from_rfc3339(&issue.updated_at) {
            Ok(ts) => last_activity_ts = Some(ts.with_timezone(&Utc)),
            Err(e) => {
                warn!(
                    "Failed to parse issue updated_at timestamp for issue #{}: {}. Error: {}",
                    issue.iid, issue.updated_at, e
                );
                // Continue, but this issue might not be accurately processed for staleness
                // if its own timestamp is the only one or the latest.
            }
        }

        // Fetch all notes for the issue (since_timestamp = 0 to get all)
        let notes = match gitlab_client
            .get_issue_notes(project_id, issue.iid, 0)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                error!(
                    "Failed to fetch notes for issue #{}: {}. Skipping note processing for this issue.",
                    issue.iid, e
                );
                Vec::new() // Process with no notes if fetching failed
            }
        };

        for note in notes {
            if note.author.username == config.bot_username {
                continue; // Skip notes from the bot itself
            }
            match DateTime::parse_from_rfc3339(&note.updated_at) {
                Ok(note_ts_rfc3339) => {
                    let note_ts = note_ts_rfc3339.with_timezone(&Utc);
                    if let Some(current_max_ts) = last_activity_ts {
                        if note_ts > current_max_ts {
                            last_activity_ts = Some(note_ts);
                        }
                    } else {
                        last_activity_ts = Some(note_ts);
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to parse note updated_at for note #{} on issue #{}: {}. Error: {}",
                        note.id, issue.iid, note.updated_at, e
                    );
                }
            }
        }

        if let Some(last_active_date) = last_activity_ts {
            let now = Utc::now();
            let days_stale = config.stale_issue_days;
            let staleness_threshold = ChronoDuration::days(days_stale as i64);

            if now - last_active_date > staleness_threshold {
                // Issue is stale
                if !issue.labels.iter().any(|l| l == stale_label_name) {
                    info!(
                        "Issue #{} is stale and not labeled. Adding '{}' label.",
                        issue.iid, stale_label_name
                    );
                    if let Err(e) = gitlab_client
                        .add_issue_label(project_id, issue.iid, stale_label_name)
                        .await
                    {
                        error!(
                            "Failed to add '{}' label to issue #{}: {}",
                            stale_label_name, issue.iid, e
                        );
                    }
                }
            } else {
                // Issue is not stale
                if issue.labels.iter().any(|l| l == stale_label_name) {
                    info!(
                        "Issue #{} is not stale but has '{}' label. Removing label.",
                        issue.iid, stale_label_name
                    );
                    if let Err(e) = gitlab_client
                        .remove_issue_label(project_id, issue.iid, stale_label_name)
                        .await
                    {
                        error!(
                            "Failed to remove '{}' label from issue #{}: {}",
                            stale_label_name, issue.iid, e
                        );
                    }
                }
            }
        } else {
            warn!(
                "Could not determine last activity timestamp for issue #{}. Skipping staleness check.",
                issue.iid
            );
        }
    }
    Ok(())
}
