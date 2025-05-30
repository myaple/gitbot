use crate::config::load_config;
use crate::gitlab::GitlabApiClient;
// use crate::models::GitlabProject;
use crate::polling::PollingService;
use crate::repo_context::RepoContextExtractor;
use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod config;
mod file_indexer;
mod gitlab;
mod gitlab_ext;
mod handlers;
mod mention_cache;
mod models;
mod openai;
mod polling;
mod repo_context;

#[cfg(test)]
mod file_indexer_test;
#[cfg(test)]
mod polling_test;

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments and load configuration
    let app_settings = load_config().with_context(|| "Failed to load configuration")?;

    // Initialize Logging with level from config
    let log_level = app_settings.log_level.clone();
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level.clone()));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    info!("Starting application...");
    info!("Using log level: {}", log_level);
    info!("Configuration loaded successfully.");

    // Initialize GitLab API Client
    let config_arc = Arc::new(app_settings);
    let gitlab_client = GitlabApiClient::new(config_arc.clone())
        .with_context(|| "Failed to create GitLab client")?;

    info!("GitLab API client initialized successfully.");
    let gitlab_client = Arc::new(gitlab_client);

    // Create repo context extractor
    let repo_context_extractor =
        RepoContextExtractor::new(gitlab_client.clone(), config_arc.clone());

    // Initialize file indexes for all repos to poll
    let mut projects = Vec::new();
    for repo_path in &config_arc.repos_to_poll {
        match gitlab_client.get_project_by_path(repo_path).await {
            Ok(project) => {
                info!("Found project: {}", project.path_with_namespace);
                projects.push(project);
            }
            Err(e) => {
                warn!("Failed to get project for {}: {}", repo_path, e);
            }
        }
    }

    // Also add context repo if configured
    if let Some(context_repo_path) = &config_arc.context_repo_path {
        match gitlab_client.get_project_by_path(context_repo_path).await {
            Ok(project) => {
                info!(
                    "Found context repo project: {}",
                    project.path_with_namespace
                );
                projects.push(project);
            }
            Err(e) => {
                warn!(
                    "Failed to get context repo project for {}: {}",
                    context_repo_path, e
                );
            }
        }
    }

    // Initialize file indexes in the background
    let projects_clone = projects.clone();
    tokio::spawn(async move {
        if let Err(e) = repo_context_extractor
            .initialize_file_indexes(projects_clone)
            .await
        {
            warn!("Failed to initialize file indexes: {}", e);
        }
    });

    // Create polling service
    let polling_service = PollingService::new(gitlab_client, config_arc.clone());

    info!(
        "Starting polling service with interval of {} seconds...",
        config_arc.poll_interval_seconds
    );

    // Start polling (this will run indefinitely)
    polling_service.start_polling().await?;

    Ok(())
}
