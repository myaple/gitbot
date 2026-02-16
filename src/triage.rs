use anyhow::{anyhow, Result};
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::config::AppSettings;
use crate::gitlab::{GitlabApiClient, IssueQueryOptions};
use crate::models::GitlabIssue;
use crate::openai::{ChatRequestBuilder, OpenAIApiClient};

/// Learned information about a label
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LabelKnowledge {
    pub name: String,
    pub description: Option<String>,
    pub color: String,
    pub learned_summary: String,
    pub sample_issues: Vec<IssueSample>,
}

/// A sample issue for learning
#[derive(Debug, Clone)]
pub struct IssueSample {
    pub title: String,
    pub description: Option<String>,
}

/// The triage service that learns label meanings and applies them
#[derive(Clone)]
pub struct TriageService {
    gitlab_client: Arc<GitlabApiClient>,
    openai_client: Arc<OpenAIApiClient>,
    config: Arc<AppSettings>,
    /// Map of project_id -> label_name -> LabelKnowledge
    label_knowledge: Arc<tokio::sync::RwLock<HashMap<i64, HashMap<String, LabelKnowledge>>>>,
}

impl TriageService {
    pub fn new(
        gitlab_client: Arc<GitlabApiClient>,
        openai_client: Arc<OpenAIApiClient>,
        config: Arc<AppSettings>,
    ) -> Self {
        Self {
            gitlab_client,
            openai_client,
            config,
            label_knowledge: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Learn label meanings for all projects on startup
    pub async fn learn_labels_for_projects(&self, project_ids: &[i64]) -> Result<()> {
        info!("Starting label learning for {} projects", project_ids.len());

        // Process all projects in parallel
        let _results: Vec<_> = stream::iter(project_ids.iter())
            .map(|&project_id| {
                let self_clone = self.clone();
                async move {
                    if let Err(e) = self_clone.learn_labels_for_project(project_id).await {
                        error!("Failed to learn labels for project {}: {}", project_id, e);
                    }
                }
            })
            .buffer_unordered(3)
            .collect()
            .await;

        info!("Label learning completed for all projects");
        Ok(())
    }

    /// Learn label meanings for a specific project
    async fn learn_labels_for_project(&self, project_id: i64) -> Result<()> {
        info!("Learning labels for project {}", project_id);

        // Get all labels for the project
        let labels = self
            .gitlab_client
            .get_labels(project_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch labels: {}", e))?;

        info!("Found {} labels for project {}", labels.len(), project_id);

        // Filter out system labels we don't want to learn or apply
        let excluded_labels = ["stale", "doing", "todo", "in progress"];
        let learnable_labels: Vec<_> = labels
            .into_iter()
            .filter(|l| {
                !excluded_labels.contains(&l.name.to_lowercase().as_str())
                    && !l.name.starts_with("To:") // Exclude "To:" assignment labels
            })
            .collect();

        let mut label_map = HashMap::new();

        // Learn each label in parallel with controlled concurrency
        let label_results: Vec<_> = stream::iter(learnable_labels.iter())
            .map(|label| {
                let self_clone = self.clone();
                let label = label.clone();
                async move {
                    match self_clone.learn_single_label(project_id, &label).await {
                        Ok(knowledge) => Some((label.name.clone(), knowledge)),
                        Err(e) => {
                            warn!(
                                "Failed to learn label '{}' for project {}: {}",
                                label.name, project_id, e
                            );
                            None
                        }
                    }
                }
            })
            .buffer_unordered(5)
            .collect()
            .await;

        // Collect successful results
        for (name, knowledge) in label_results.into_iter().flatten() {
            label_map.insert(name, knowledge);
        }

        // Store the learned knowledge
        let label_count = label_map.len();
        self.label_knowledge
            .write()
            .await
            .insert(project_id, label_map);

        info!(
            "Successfully learned {} labels for project {}",
            label_count, project_id
        );

        Ok(())
    }

    /// Learn about a single label by analyzing sample issues
    async fn learn_single_label(
        &self,
        project_id: i64,
        label: &crate::models::GitlabLabel,
    ) -> Result<LabelKnowledge> {
        debug!(
            "Learning label '{}' (id: {}) for project {}",
            label.name, label.id, project_id
        );

        // Get sample issues with this label
        let sample_issues = self
            .gitlab_client
            .get_issues(
                project_id,
                IssueQueryOptions {
                    labels: Some(label.name.clone()),
                    state: Some("opened".to_string()),
                    per_page: Some(self.config.label_learning_samples),
                    order_by: Some("created_at".to_string()),
                    sort: Some("desc".to_string()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch sample issues: {}", e))?;

        if sample_issues.is_empty() {
            debug!(
                "No sample issues found for label '{}', using label description only",
                label.name
            );
            return Ok(LabelKnowledge {
                name: label.name.clone(),
                description: label.description.clone(),
                color: label.color.clone(),
                learned_summary: label
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("Label: {}", label.name)),
                sample_issues: Vec::new(),
            });
        }

        // Extract issue samples
        let issue_samples: Vec<IssueSample> = sample_issues
            .iter()
            .map(|issue| IssueSample {
                title: issue.title.clone(),
                description: issue.description.clone(),
            })
            .collect();

        // Use LLM to summarize what this label means
        let summary = self
            .summarize_label_meaning(&label.name, label.description.as_deref(), &issue_samples)
            .await
            .unwrap_or_else(|e| {
                warn!(
                    "Failed to use LLM to summarize label '{}': {}. Using basic summary.",
                    label.name, e
                );
                format!(
                    "Label: {} - Used in {} issues",
                    label.name,
                    issue_samples.len()
                )
            });

        debug!("Learned summary for label '{}': {}", label.name, summary);

        Ok(LabelKnowledge {
            name: label.name.clone(),
            description: label.description.clone(),
            color: label.color.clone(),
            learned_summary: summary,
            sample_issues: issue_samples,
        })
    }

    /// Use LLM to summarize what a label means based on sample issues
    async fn summarize_label_meaning(
        &self,
        label_name: &str,
        label_description: Option<&str>,
        sample_issues: &[IssueSample],
    ) -> Result<String> {
        // Build a prompt to understand the label
        let mut prompt =
            "You are analyzing GitLab issue labels to understand their usage patterns.\n\n"
                .to_string();

        if let Some(desc) = label_description {
            prompt.push_str(&format!("Label name: {}\n", label_name));
            prompt.push_str(&format!("Label description: {}\n\n", desc));
        } else {
            prompt.push_str(&format!("Label name: {}\n\n", label_name));
        }

        prompt.push_str("Here are some example issues that use this label:\n\n");

        for (i, issue) in sample_issues.iter().enumerate() {
            prompt.push_str(&format!("--- Example {} ---\n", i + 1));
            prompt.push_str(&format!("Title: {}\n", issue.title));
            if let Some(desc) = &issue.description {
                let truncated_desc = if desc.len() > 500 {
                    format!("{}...", &desc[..500])
                } else {
                    desc.clone()
                };
                prompt.push_str(&format!("Description: {}\n", truncated_desc));
            }
            prompt.push('\n');
        }

        prompt.push_str(&format!(
            "\nBased on these examples, provide a concise 1-2 sentence summary of \
            when the '{}' label should be used. Focus on the common patterns in the examples. \
            Do not include the word 'Summary' or any preamble - just provide the description directly.",
            label_name
        ));

        // Use ChatRequestBuilder to create the request
        let mut builder = ChatRequestBuilder::new(&self.config);
        builder.with_user_message(&prompt);
        let request = builder
            .build()
            .map_err(|e| anyhow!("Failed to build request: {}", e))?;

        let response = self
            .openai_client
            .send_chat_completion(&request)
            .await
            .map_err(|e| anyhow::anyhow!("OpenAI API error: {}", e))?;

        let summary = response
            .choices
            .first()
            .ok_or_else(|| anyhow!("No response from OpenAI"))?
            .message
            .content
            .trim()
            .to_string();

        Ok(summary)
    }

    /// Get suggested labels for an issue using the LLM
    pub async fn suggest_labels_for_issue(
        &self,
        project_id: i64,
        issue: &GitlabIssue,
    ) -> Result<Vec<String>> {
        // Get the label knowledge for this project
        let label_knowledge = self.label_knowledge.read().await;
        let project_labels = label_knowledge
            .get(&project_id)
            .ok_or_else(|| anyhow!("No label knowledge for project {}", project_id))?;

        if project_labels.is_empty() {
            return Ok(Vec::new());
        }

        // Build a prompt for the LLM
        let mut prompt = String::from(
            "You are a GitLab issue triage assistant. Your task is to suggest appropriate labels \
            for an issue based on its title and description.\n\n",
        );

        prompt.push_str("Available labels and their meanings:\n\n");

        for (name, knowledge) in project_labels.iter() {
            prompt.push_str(&format!("- **{}**: {}\n", name, knowledge.learned_summary));
        }

        prompt.push_str("\n--- Issue to Label ---\n\n");
        prompt.push_str(&format!("Title: {}\n", issue.title));
        if let Some(desc) = &issue.description {
            let truncated_desc = if desc.len() > 2000 {
                format!("{}...", &desc[..2000])
            } else {
                desc.clone()
            };
            prompt.push_str(&format!("Description: {}\n", truncated_desc));
        }

        prompt.push_str(
            "\nSelect the most appropriate labels from the list above. Return ONLY a JSON array \
            of label names that apply to this issue. If no labels are appropriate, return an empty array.\n\n\
            Example response: [\"bug\", \"high-priority\"]\n\n\
            Labels:",
        );

        // Use ChatRequestBuilder to create the request
        let mut builder = ChatRequestBuilder::new(&self.config);
        builder.with_user_message(&prompt);
        let request = builder
            .build()
            .map_err(|e| anyhow!("Failed to build request: {}", e))?;

        // Note: Using the non-structured API first. We could upgrade to use response_format for models that support it
        let response = self
            .openai_client
            .send_chat_completion(&request)
            .await
            .map_err(|e| anyhow::anyhow!("OpenAI API error: {}", e))?;

        let content = response
            .choices
            .first()
            .ok_or_else(|| anyhow!("No response from OpenAI"))?
            .message
            .content
            .trim();

        // Parse the response - handle both JSON arrays and markdown code blocks
        let labels: Vec<String> = if content.starts_with("```") {
            // Extract JSON from markdown code block
            let start = content.find('[').unwrap_or(0);
            let json_str = if let Some(end) = content[start..].find(']') {
                &content[start..start + end + 1]
            } else {
                content
            };
            serde_json::from_str(json_str).unwrap_or_else(|_| Vec::new())
        } else if content.starts_with('[') {
            serde_json::from_str(content).unwrap_or_else(|_| Vec::new())
        } else {
            // Try to extract array from text
            let start = content.find('[').unwrap_or(0);
            let json_str = if let Some(end) = content[start..].find(']') {
                &content[start..start + end + 1]
            } else {
                "[]"
            };
            serde_json::from_str(json_str).unwrap_or_else(|_| Vec::new())
        };

        // Validate labels against available labels
        let valid_labels: Vec<String> = labels
            .into_iter()
            .filter(|l| project_labels.contains_key(l))
            .collect();

        debug!(
            "Suggested labels for issue #{}: {:?}",
            issue.iid, valid_labels
        );

        Ok(valid_labels)
    }
}

/// Process unlabeled issues and apply labels
pub async fn triage_unlabeled_issues(
    triage_service: &TriageService,
    project_id: i64,
    issues: &[GitlabIssue],
    lookback_hours: u64,
) -> Result<usize> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0))
        .as_secs();

    let cutoff_timestamp = now.saturating_sub(lookback_hours * 3600);

    let unlabeled_issues: Vec<_> = issues
        .iter()
        .filter(|issue| issue.labels.is_empty())
        .filter(|issue| {
            // Parse created_at timestamp and check if it's within the lookback window
            match chrono::DateTime::parse_from_rfc3339(&issue.created_at) {
                Ok(dt) => {
                    let created_ts = dt.timestamp() as u64;
                    created_ts >= cutoff_timestamp
                }
                Err(e) => {
                    warn!(
                        "Failed to parse created_at timestamp for issue #{}: {}. Skipping issue.",
                        issue.iid, e
                    );
                    false
                }
            }
        })
        .collect();

    if unlabeled_issues.is_empty() {
        debug!("No unlabeled issues to triage for project {}", project_id);
        return Ok(0);
    }

    info!(
        "Found {} unlabeled issues to triage for project {}",
        unlabeled_issues.len(),
        project_id
    );

    let mut labeled_count = 0;

    // Process issues in parallel with controlled concurrency
    let results: Vec<_> = stream::iter(unlabeled_issues.into_iter().cloned())
        .map(|issue| {
            let triage = triage_service.clone();
            async move {
                match triage.suggest_labels_for_issue(project_id, &issue).await {
                    Ok(labels) => Some((issue, labels)),
                    Err(e) => {
                        error!("Failed to suggest labels for issue #{}: {}", issue.iid, e);
                        None
                    }
                }
            }
        })
        .buffer_unordered(3)
        .collect()
        .await;

    for (issue, labels) in results.into_iter().flatten() {
        if labels.is_empty() {
            debug!("No labels suggested for issue #{}", issue.iid);
            continue;
        }

        info!("Applying labels {:?} to issue #{}", labels, issue.iid);

        if let Err(e) = triage_service
            .gitlab_client
            .add_issue_labels(
                project_id,
                issue.iid,
                &labels.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            )
            .await
        {
            error!("Failed to apply labels to issue #{}: {}", issue.iid, e);
        } else {
            labeled_count += 1;
        }
    }

    info!(
        "Successfully labeled {} issues for project {}",
        labeled_count, project_id
    );

    Ok(labeled_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_sample_creation() {
        let sample = IssueSample {
            title: "Test Issue".to_string(),
            description: Some("Test description".to_string()),
        };

        assert_eq!(sample.title, "Test Issue");
    }

    #[test]
    fn test_label_knowledge_creation() {
        let knowledge = LabelKnowledge {
            name: "bug".to_string(),
            description: Some("A bug report".to_string()),
            color: "#ff0000".to_string(),
            learned_summary: "Issues reporting bugs or defects".to_string(),
            sample_issues: vec![],
        };

        assert_eq!(knowledge.name, "bug");
        assert_eq!(knowledge.sample_issues.len(), 0);
    }
}
