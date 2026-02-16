use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures::stream::{self, StreamExt};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::config::AppSettings;
use crate::file_indexer::FileIndexManager;
use crate::gitlab::{GitlabApiClient, IssueQueryOptions, LabelOperation};
use crate::handlers::process_mention;
use crate::log_dedup::LogDeduplicator;
use crate::mention_cache::MentionCache;
use crate::models::{
    GitlabIssue, GitlabMergeRequest, GitlabNoteAttributes, GitlabNoteEvent, GitlabNoteObject,
    GitlabProject, GitlabUser,
};
use crate::triage::{triage_unlabeled_issues, TriageService};

#[derive(Clone)]
pub struct PollingService {
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
    pub(crate) last_checked: Arc<Mutex<u64>>,
    processed_mentions_cache: MentionCache,
    file_index_manager: Arc<FileIndexManager>,
    triage_service: Option<TriageService>,
    log_dedup: LogDeduplicator,
}

impl PollingService {
    pub fn new(
        gitlab_client: Arc<GitlabApiClient>,
        config: Arc<AppSettings>,
        file_index_manager: Arc<FileIndexManager>,
        triage_service: Option<TriageService>,
    ) -> Self {
        // Initialize with current time minus 1 hour to check recent activity on startup
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();

        let initial_time = now.saturating_sub(3600); // 1 hour ago

        // Create log deduplicator with 5 minute suppression window
        let log_dedup = LogDeduplicator::new(Duration::from_secs(300));

        Self {
            gitlab_client,
            config,
            last_checked: Arc::new(Mutex::new(initial_time)),
            processed_mentions_cache: MentionCache::new(),
            file_index_manager,
            triage_service,
            log_dedup,
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

        debug!("Polling repositories since timestamp: {}", *last_checked);

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
                    // Only log errors that haven't been logged recently
                    let log_key = format!("poll_error_{}", repo_path_clone);
                    if self_clone.log_dedup.should_log(&log_key).await {
                        error!("Error polling repository {}: {}", repo_path_clone, e);
                    }
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
        debug!("Polling repository: {}", repo_path);

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

        debug!(
            "Using effective timestamp: {} (max age: {} hours)",
            effective_timestamp, self.config.max_age_hours
        );

        // Determine fetch timestamp for recent issues (covering mentions and triage)
        let fetch_recent_ts = if self.triage_service.is_some() {
            let triage_lookback_seconds = self.config.triage_lookback_hours * 3600;
            let triage_cutoff = now.saturating_sub(triage_lookback_seconds);
            std::cmp::min(effective_timestamp, triage_cutoff)
        } else {
            effective_timestamp
        };

        // Fetch issues covering both mentions and triage needs
        let recent_issues = match self
            .gitlab_client
            .get_issues(
                project_id,
                IssueQueryOptions {
                    updated_after: Some(fetch_recent_ts),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(issues) => issues,
            Err(e) => {
                // Only log this error if we haven't logged it recently
                let log_key = format!("fetch_issues_error_{}", project_id);
                if self.log_dedup.should_log(&log_key).await {
                    error!(
                        "Failed to fetch recent issues for project {}: {}",
                        project_id, e
                    );
                }
                Vec::new()
            }
        };

        // Filter for mentions: update >= effective_timestamp
        let mention_issues: Vec<GitlabIssue> = recent_issues
            .iter()
            .filter(|i| match DateTime::parse_from_rfc3339(&i.updated_at) {
                Ok(dt) => dt.timestamp() as u64 >= effective_timestamp,
                Err(_) => false,
            })
            .cloned()
            .collect();

        // Filter for triage: open issues
        let open_recent_issues: Vec<GitlabIssue> = recent_issues
            .iter()
            .filter(|i| i.state == "opened")
            .cloned()
            .collect();

        // Start task for processing mentions
        let mentions_task = {
            let self_clone = self.clone();
            let project_clone = project.clone();
            let mention_issues_clone = mention_issues;
            tokio::spawn(async move {
                if let Err(e) = self_clone
                    .process_issues_for_mentions(
                        project_id,
                        &mention_issues_clone,
                        effective_timestamp,
                        &project_clone,
                    )
                    .await
                {
                    error!(
                        "Error processing mentions for project {}: {}",
                        project_id, e
                    );
                }
            })
        };

        // Start task for polling merge requests
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

        // Fetch old issues for stale check (since 0)
        // We fetch separately because "sort=asc" means we get OLDEST updated issues with 0,
        // but recent ones with fetch_recent_ts.
        // Filter by state=opened server-side.
        let open_stale_issues = match self
            .gitlab_client
            .get_issues(
                project_id,
                IssueQueryOptions {
                    updated_after: Some(0),
                    state: Some("opened".to_string()),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(issues) => issues,
            Err(e) => {
                // Only log this error if we haven't logged it recently
                let log_key = format!("stale_fetch_error_{}", project_id);
                if self.log_dedup.should_log(&log_key).await {
                    error!(
                        "Failed to fetch issues for stale check for project {}: {}",
                        project_id, e
                    );
                }
                Vec::new()
            }
        };

        // Task for checking stale issues
        let stale_check_task = {
            let project_id_clone = project_id;
            let gitlab_client_clone = self.gitlab_client.clone();
            let config_clone = self.config.clone();
            let open_stale_issues_clone = open_stale_issues;
            tokio::spawn(async move {
                if let Err(e) = check_stale_issues(
                    project_id_clone,
                    gitlab_client_clone,
                    config_clone,
                    &open_stale_issues_clone,
                )
                .await
                {
                    error!(
                        "Error checking stale issues for project {}: {}",
                        project_id_clone, e
                    );
                }
            })
        };

        // Task for triaging unlabeled issues
        let triage_task = if let Some(triage) = &self.triage_service {
            let triage_clone = triage.clone();
            let project_id_clone = project_id;
            let config_clone = self.config.clone();
            let open_recent_issues_clone = open_recent_issues;

            Some(tokio::spawn(async move {
                if let Err(e) = triage_unlabeled_issues(
                    &triage_clone,
                    project_id_clone,
                    &open_recent_issues_clone,
                    config_clone.triage_lookback_hours,
                )
                .await
                {
                    error!(
                        "Error triaging unlabeled issues for project {}: {}",
                        project_id_clone, e
                    );
                }
            }))
        } else {
            None
        };

        // Wait for all tasks
        if let Err(e) = mentions_task.await {
            error!("Task join error for mentions processing: {}", e);
        }
        if let Err(e) = mrs_task.await {
            error!("Task join error for merge requests polling: {}", e);
        }
        if let Err(e) = stale_check_task.await {
            error!("Task join error for stale issue checking: {}", e);
        }
        if let Some(task) = triage_task {
            if let Err(e) = task.await {
                error!("Task join error for issue triage: {}", e);
            }
        }

        Ok(())
    }

    async fn process_issues_for_mentions(
        &self,
        project_id: i64,
        issues: &[GitlabIssue],
        since_timestamp: u64,
        project: &GitlabProject,
    ) -> Result<()> {
        if issues.is_empty() {
            debug!(
                "No issues to process for mentions in project ID: {}",
                project_id
            );
            return Ok(());
        }

        debug!(
            "Processing {} issues for mentions for project ID: {}",
            issues.len(),
            project_id
        );

        let mention_count = Arc::new(Mutex::new(0_u32));

        // Process issues in parallel with controlled concurrency
        let _issue_results: Vec<_> = stream::iter(issues.iter().cloned())
            .map(|issue| {
                let gitlab_client = self.gitlab_client.clone();
                let config = self.config.clone();
                let processed_mentions_cache = self.processed_mentions_cache.clone();
                let file_index_manager = self.file_index_manager.clone();
                let project = project.clone();
                let mention_count = mention_count.clone();
                let log_dedup = self.log_dedup.clone();
                async move {
                    // Get notes for this issue
                    match gitlab_client
                        .get_issue_notes(project_id, issue.iid, Some(since_timestamp))
                        .await
                    {
                        Ok(notes) => {
                            for note in notes {
                                // Skip notes by the bot itself
                                if note.author.username == config.bot_username {
                                    continue;
                                }

                                // Check if note mentions the bot
                                if note.note.contains(&format!("@{}", config.bot_username)) {
                                    *mention_count.lock().await += 1;
                                    info!(
                                        "Found mention in issue #{} note #{}",
                                        issue.iid, note.id
                                    );

                                    // Create a GitlabNoteEvent from the note
                                    let event = Self::create_issue_note_event_static(
                                        project.clone(),
                                        note,
                                        issue.clone(),
                                    );

                                    // Process the mention
                                    if let Err(e) = process_mention(
                                        event,
                                        gitlab_client.clone(),
                                        config.clone(),
                                        &processed_mentions_cache,
                                        file_index_manager.clone(),
                                    )
                                    .await
                                    {
                                        error!("Error processing mention: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            // Only log errors if we haven't logged them recently
                            let log_key =
                                format!("get_notes_error_issue_{}_{}", project_id, issue.iid);
                            if log_dedup.should_log(&log_key).await {
                                error!("Failed to get notes for issue #{}: {}", issue.iid, e);
                            }
                        }
                    }
                }
            })
            .buffer_unordered(4) // Process 4 issues concurrently
            .collect()
            .await;

        let total_mentions = *mention_count.lock().await;
        if total_mentions > 0 {
            info!(
                "Processed {} mention(s) in {} issue(s) for project {}",
                total_mentions,
                issues.len(),
                project_id
            );
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

        if merge_requests.is_empty() {
            debug!(
                "No merge requests to process for project ID: {}",
                project_id
            );
            return Ok(());
        }

        let mention_count = Arc::new(Mutex::new(0_u32));

        // Process merge requests in parallel with controlled concurrency
        let _mr_results: Vec<_> = stream::iter(merge_requests.iter().cloned())
            .map(|mr| {
                let gitlab_client = self.gitlab_client.clone();
                let config = self.config.clone();
                let processed_mentions_cache = self.processed_mentions_cache.clone();
                let file_index_manager = self.file_index_manager.clone();
                let project = project.clone();
                let mention_count = mention_count.clone();
                let log_dedup = self.log_dedup.clone();
                async move {
                    // Get notes for this merge request
                    match gitlab_client
                        .get_merge_request_notes(project_id, mr.iid, Some(since_timestamp))
                        .await
                    {
                        Ok(notes) => {
                            for note in notes {
                                // Skip notes by the bot itself
                                if note.author.username == config.bot_username {
                                    continue;
                                }

                                // Check if note mentions the bot
                                if note.note.contains(&format!("@{}", config.bot_username)) {
                                    *mention_count.lock().await += 1;
                                    info!("Found mention in MR !{} note #{}", mr.iid, note.id);

                                    // Create a GitlabNoteEvent from the note
                                    let event = Self::create_mr_note_event_static(
                                        project.clone(),
                                        note,
                                        mr.clone(),
                                    );

                                    // Process the mention
                                    if let Err(e) = process_mention(
                                        event,
                                        gitlab_client.clone(),
                                        config.clone(),
                                        &processed_mentions_cache,
                                        file_index_manager.clone(),
                                    )
                                    .await
                                    {
                                        error!("Error processing mention: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            // Only log errors if we haven't logged them recently
                            let log_key = format!("get_notes_error_mr_{}_{}", project_id, mr.iid);
                            if log_dedup.should_log(&log_key).await {
                                error!("Failed to get notes for MR !{}: {}", mr.iid, e);
                            }
                        }
                    }
                }
            })
            .buffer_unordered(4) // Process 4 MRs concurrently
            .collect()
            .await;

        let total_mentions = *mention_count.lock().await;
        if total_mentions > 0 {
            info!(
                "Processed {} mention(s) in {} merge request(s) for project {}",
                total_mentions,
                merge_requests.len(),
                project_id
            );
        }

        Ok(())
    }

    fn create_issue_note_event_static(
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

    fn create_mr_note_event_static(
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

async fn determine_last_activity(
    project_id: i64,
    issue: &GitlabIssue,
    gitlab_client: &GitlabApiClient,
    config: &AppSettings,
) -> Option<DateTime<Utc>> {
    let mut last_activity_ts: Option<DateTime<Utc>> = None;

    // Start with the issue's own updated_at timestamp
    match DateTime::parse_from_rfc3339(&issue.updated_at) {
        Ok(ts) => last_activity_ts = Some(ts.with_timezone(&Utc)),
        Err(e) => {
            warn!(
                "Failed to parse issue updated_at timestamp for issue #{}: {}. Error: {}",
                issue.iid, issue.updated_at, e
            );
        }
    }

    // Optimization: If the issue itself is already stale based on updated_at,
    // and the last update was older than our threshold, we can skip fetching notes.
    let days_stale = config.stale_issue_days;
    let staleness_threshold = ChronoDuration::days(days_stale as i64);
    let now = Utc::now();
    let threshold_ts = now - staleness_threshold;

    let is_stale_by_issue_date = if let Some(last_ts) = last_activity_ts {
        last_ts < threshold_ts
    } else {
        false // conservative: if we can't parse date, fetch notes
    };

    // Fetch notes only if issue is not already clearly stale
    let notes = if is_stale_by_issue_date {
        debug!(
            "Issue #{} is stale based on updated_at. Skipping note fetch.",
            issue.iid
        );
        Vec::new()
    } else {
        // Fetch all notes for the issue
        match gitlab_client
            .get_issue_notes(project_id, issue.iid, Some(0))
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

    last_activity_ts
}

async fn manage_stale_label(
    project_id: i64,
    issue: &GitlabIssue,
    is_stale: bool,
    gitlab_client: &GitlabApiClient,
    stale_label_name: &str,
) -> Result<StaleAction> {
    if is_stale {
        // Issue is stale
        if !issue.labels.iter().any(|l| l == stale_label_name) {
            debug!(
                "Issue #{} is stale and not labeled. Adding '{}' label.",
                issue.iid, stale_label_name
            );
            if let Err(e) = gitlab_client
                .update_issue_labels(
                    project_id,
                    issue.iid,
                    LabelOperation::Add(vec![stale_label_name.to_string()]),
                )
                .await
            {
                error!(
                    "Failed to add '{}' label to issue #{}: {}",
                    stale_label_name, issue.iid, e
                );
                return Ok(StaleAction::NoChange);
            }
            return Ok(StaleAction::Labeled);
        }
    } else {
        // Issue is not stale
        if issue.labels.iter().any(|l| l == stale_label_name) {
            debug!(
                "Issue #{} is not stale but has '{}' label. Removing label.",
                issue.iid, stale_label_name
            );
            if let Err(e) = gitlab_client
                .update_issue_labels(
                    project_id,
                    issue.iid,
                    LabelOperation::Remove(vec![stale_label_name.to_string()]),
                )
                .await
            {
                error!(
                    "Failed to remove '{}' label from issue #{}: {}",
                    stale_label_name, issue.iid, e
                );
                return Ok(StaleAction::NoChange);
            }
            return Ok(StaleAction::Unlabeled);
        }
    }
    Ok(StaleAction::NoChange)
}

pub(crate) async fn check_stale_issues(
    project_id: i64,
    gitlab_client: Arc<GitlabApiClient>,
    config: Arc<AppSettings>,
    issues: &[GitlabIssue],
) -> Result<()> {
    if issues.is_empty() {
        debug!(
            "No issues to check for staleness in project ID: {}",
            project_id
        );
        return Ok(());
    }

    debug!(
        "Checking {} issues for staleness in project ID: {}",
        issues.len(),
        project_id
    );
    let stale_label_name = "stale"; // Define the label name

    let labeled_count = Arc::new(Mutex::new(0_u32));
    let unlabeled_count = Arc::new(Mutex::new(0_u32));

    // Issues are filtered by the caller to be only "opened" state issues

    // Process issues in parallel with controlled concurrency
    let _stale_results: Vec<_> = stream::iter(issues.iter().cloned())
        .map(|issue| {
            let gitlab_client = gitlab_client.clone();
            let config = config.clone();
            let labeled_count = labeled_count.clone();
            let unlabeled_count = unlabeled_count.clone();
            async move {
                debug!("Processing issue #{} for staleness", issue.iid);

                let last_activity_ts =
                    determine_last_activity(project_id, &issue, &gitlab_client, &config).await;

                if let Some(last_active_date) = last_activity_ts {
                    let now = Utc::now();
                    let days_stale = config.stale_issue_days;
                    let staleness_threshold = ChronoDuration::days(days_stale as i64);

                    let is_stale = now - last_active_date > staleness_threshold;

                    match manage_stale_label(
                        project_id,
                        &issue,
                        is_stale,
                        &gitlab_client,
                        stale_label_name,
                    )
                    .await
                    {
                        Ok(action) => {
                            match action {
                                StaleAction::Labeled => *labeled_count.lock().await += 1,
                                StaleAction::Unlabeled => *unlabeled_count.lock().await += 1,
                                StaleAction::NoChange => {},
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to manage stale label for issue #{}: {}",
                                issue.iid, e
                            );
                        }
                    }
                } else {
                    debug!(
                        "Could not determine last activity timestamp for issue #{}. Skipping staleness check.",
                        issue.iid
                    );
                }
            }
        })
        .buffer_unordered(6) // Process 6 issues concurrently for stale checking
        .collect()
        .await;

    let total_labeled = *labeled_count.lock().await;
    let total_unlabeled = *unlabeled_count.lock().await;

    if total_labeled > 0 || total_unlabeled > 0 {
        info!("Stale check for project {}: labeled {} issue(s) as stale, removed stale label from {} issue(s)",
              project_id, total_labeled, total_unlabeled);
    }

    Ok(())
}

#[derive(Debug)]
enum StaleAction {
    Labeled,
    Unlabeled,
    NoChange,
}
