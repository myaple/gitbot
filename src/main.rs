use crate::config::load_config;
use crate::gitlab::GitlabApiClient;
use crate::polling::PollingService;
use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;
mod gitlab;
mod gitlab_ext;
mod handlers;
mod models;
mod openai;
mod polling;
mod repo_context;

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
