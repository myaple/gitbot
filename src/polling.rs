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
    GitlabIssue, GitlabNoteAttributes, GitlabNoteEvent, GitlabNoteObject, GitlabProject,
    GitlabProjectEvent,
};
use crate::openai::OpenAIApiClient;
use crate::triage::{triage_unlabeled_issues, TriageService};

#[derive(Clone)]
pub struct PollingService {
    gitlab_client: Arc<GitlabApiClient>,
    openai_client: Arc<OpenAIApiClient>,
    config: Arc<AppSettings>,
    pub(crate) last_checked: Arc<Mutex<u64>>,
    processed_mentions_cache: MentionCache,
    file_index_manager: Arc<FileIndexManager>,
    triage_service: Option<TriageService>,
    log_dedup: LogDeduplicator,
    /// Tracks when the stale-issue check last ran per project_id (keyed by project_id).
    last_stale_check: Arc<dashmap::DashMap<i64, std::time::Instant>>,
}

impl PollingService {
    pub fn new(
        gitlab_client: Arc<GitlabApiClient>,
        openai_client: Arc<OpenAIApiClient>,
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
            openai_client,
            config,
            last_checked: Arc::new(Mutex::new(initial_time)),
            processed_mentions_cache: MentionCache::new(),
            file_index_manager,
            triage_service,
            log_dedup,
            last_stale_check: Arc::new(dashmap::DashMap::new()),
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

        // Fetch all note events for the project in a single API call.
        // This replaces the previous pattern of fetching notes per-issue and per-MR.
        let note_events = match self
            .gitlab_client
            .get_project_note_events(project_id, effective_timestamp)
            .await
        {
            Ok(events) => events,
            Err(e) => {
                let log_key = format!("fetch_events_error_{}", project_id);
                if self.log_dedup.should_log(&log_key).await {
                    error!(
                        "Failed to fetch note events for project {}: {}",
                        project_id, e
                    );
                }
                Vec::new()
            }
        };

        // Task for processing all mentions (issues + MRs) from the single events batch
        let mentions_task = {
            let self_clone = self.clone();
            let project_clone = project.clone();
            tokio::spawn(async move {
                if let Err(e) = self_clone
                    .process_note_events(project_id, note_events, &project_clone)
                    .await
                {
                    error!(
                        "Error processing note events for project {}: {}",
                        project_id, e
                    );
                }
            })
        };

        // Task for checking stale issues (only once per day per project)
        let stale_check_task = if self.should_run_stale_check(project_id) {
            self.update_last_stale_check(project_id);
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

            let gitlab_client_clone = self.gitlab_client.clone();
            let config_clone = self.config.clone();
            Some(tokio::spawn(async move {
                if let Err(e) = check_stale_issues(
                    project_id,
                    gitlab_client_clone,
                    config_clone,
                    &open_stale_issues,
                )
                .await
                {
                    error!(
                        "Error checking stale issues for project {}: {}",
                        project_id, e
                    );
                }
            }))
        } else {
            debug!(
                "Skipping stale check for project {} (already ran within the last 24 hours)",
                project_id
            );
            None
        };

        // Task for triaging unlabeled issues
        let triage_task = if let Some(triage) = &self.triage_service {
            let triage_lookback_seconds = self.config.triage_lookback_hours * 3600;
            let triage_cutoff = now.saturating_sub(triage_lookback_seconds);

            let open_recent_issues = match self
                .gitlab_client
                .get_issues(
                    project_id,
                    IssueQueryOptions {
                        updated_after: Some(triage_cutoff),
                        state: Some("opened".to_string()),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(issues) => issues,
                Err(e) => {
                    let log_key = format!("triage_fetch_error_{}", project_id);
                    if self.log_dedup.should_log(&log_key).await {
                        error!(
                            "Failed to fetch issues for triage for project {}: {}",
                            project_id, e
                        );
                    }
                    Vec::new()
                }
            };

            let triage_clone = triage.clone();
            let config_clone = self.config.clone();
            Some(tokio::spawn(async move {
                if let Err(e) = triage_unlabeled_issues(
                    &triage_clone,
                    project_id,
                    &open_recent_issues,
                    config_clone.triage_lookback_hours,
                )
                .await
                {
                    error!(
                        "Error triaging unlabeled issues for project {}: {}",
                        project_id, e
                    );
                }
            }))
        } else {
            None
        };

        // Wait for all tasks
        if let Err(e) = mentions_task.await {
            error!("Task join error for note events processing: {}", e);
        }
        if let Some(task) = stale_check_task {
            if let Err(e) = task.await {
                error!("Task join error for stale issue checking: {}", e);
            }
        }
        if let Some(task) = triage_task {
            if let Err(e) = task.await {
                error!("Task join error for issue triage: {}", e);
            }
        }

        Ok(())
    }

    /// Process all note events from the project events API, detecting bot mentions across
    /// both issues and merge requests in a single pass.
    async fn process_note_events(
        &self,
        project_id: i64,
        events: Vec<GitlabProjectEvent>,
        project: &GitlabProject,
    ) -> Result<()> {
        // Pre-filter synchronously: only non-system notes that mention the bot on issues/MRs
        let bot_mention = format!("@{}", self.config.bot_username);
        let mention_events: Vec<GitlabProjectEvent> = events
            .into_iter()
            .filter(|event| {
                let Some(note) = &event.note else {
                    return false;
                };
                !note.system
                    && note.author.username != self.config.bot_username
                    && note.body.contains(&bot_mention)
                    && (note.noteable_type == "Issue" || note.noteable_type == "MergeRequest")
            })
            .collect();

        if mention_events.is_empty() {
            debug!("No bot mentions in note events for project {}", project_id);
            return Ok(());
        }

        debug!(
            "Processing {} bot mention(s) from note events for project {}",
            mention_events.len(),
            project_id
        );

        let mention_count = Arc::new(Mutex::new(0_u32));

        // Process mentions in parallel with controlled concurrency
        let _results: Vec<_> = stream::iter(mention_events.into_iter())
            .map(|event| {
                let gitlab_client = self.gitlab_client.clone();
                let openai_client = self.openai_client.clone();
                let config = self.config.clone();
                let processed_mentions_cache = self.processed_mentions_cache.clone();
                let file_index_manager = self.file_index_manager.clone();
                let project = project.clone();
                let mention_count = mention_count.clone();
                async move {
                    let note_data = event.note.unwrap(); // safe: pre-filtered above

                    let noteable_iid = note_data.noteable_iid.unwrap_or(0);
                    let noteable_id = note_data.noteable_id.unwrap_or(0);

                    let note_attrs = GitlabNoteAttributes {
                        id: note_data.id,
                        note: note_data.body.clone(),
                        author: note_data.author.clone(),
                        project_id,
                        noteable_type: note_data.noteable_type.clone(),
                        noteable_id: note_data.noteable_id,
                        iid: Some(noteable_iid),
                        url: note_data.url.clone(),
                        updated_at: note_data.updated_at.clone(),
                    };

                    let noteable_obj = GitlabNoteObject {
                        id: noteable_id,
                        iid: noteable_iid,
                        // title/description are not available from the events API;
                        // downstream processing re-fetches the full issue/MR anyway.
                        title: String::new(),
                        description: None,
                    };

                    let gitlab_event = match note_data.noteable_type.as_str() {
                        "Issue" => {
                            info!(
                                "Found mention in issue #{} note #{}",
                                noteable_iid, note_data.id
                            );
                            GitlabNoteEvent {
                                object_kind: "note".to_string(),
                                event_type: "note".to_string(),
                                user: note_data.author.clone(),
                                project,
                                object_attributes: note_attrs,
                                issue: Some(noteable_obj),
                                merge_request: None,
                            }
                        }
                        "MergeRequest" => {
                            info!(
                                "Found mention in MR !{} note #{}",
                                noteable_iid, note_data.id
                            );
                            GitlabNoteEvent {
                                object_kind: "note".to_string(),
                                event_type: "note".to_string(),
                                user: note_data.author.clone(),
                                project,
                                object_attributes: note_attrs,
                                issue: None,
                                merge_request: Some(noteable_obj),
                            }
                        }
                        _ => return,
                    };

                    *mention_count.lock().await += 1;

                    if let Err(e) = process_mention(
                        gitlab_event,
                        gitlab_client,
                        openai_client,
                        config,
                        &processed_mentions_cache,
                        file_index_manager,
                    )
                    .await
                    {
                        error!("Error processing mention: {}", e);
                    }
                }
            })
            .buffer_unordered(4)
            .collect()
            .await;

        let total_mentions = *mention_count.lock().await;
        if total_mentions > 0 {
            info!(
                "Processed {} mention(s) for project {}",
                total_mentions, project_id
            );
        }

        Ok(())
    }

    /// Returns true if the stale-issue check has not run in the last 24 hours for the given project.
    fn should_run_stale_check(&self, project_id: i64) -> bool {
        match self.last_stale_check.get(&project_id) {
            Some(last) => last.elapsed() >= Duration::from_secs(86_400),
            None => true,
        }
    }

    fn update_last_stale_check(&self, project_id: i64) {
        self.last_stale_check
            .insert(project_id, std::time::Instant::now());
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
