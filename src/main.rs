use std::sync::Arc;
use anyhow::{Result, Context};
use crate::config::load_config;
use crate::gitlab::GitlabApiClient;
use crate::polling::PollingService;
use tracing_subscriber::EnvFilter;
use tracing::{info, error};

mod config;
mod gitlab;
mod handlers;
mod models;
mod openai;
mod polling;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize Logging (initial basic setup)
    let initial_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(initial_filter)
        .init();

    info!("Starting application...");

    // Load Configuration
    let app_settings = load_config()
        .with_context(|| "Failed to load configuration")?;
    
    info!("Configuration loaded successfully.");
    
    // Re-initialize logging with level from config if RUST_LOG is not set
    // This ensures that the config's log level is respected.
    let log_level_from_config = app_settings.log_level.clone();
    let _final_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_level_from_config.clone()));
    // Skip re-initialization as it's not necessary and can cause issues
    info!("Using log level: {}", log_level_from_config);

    info!("Configuration loaded and logger re-initialized with config log level if applicable.");

    // Initialize GitLab API Client
    let gitlab_client = GitlabApiClient::new(&app_settings)
        .with_context(|| "Failed to create GitLab client")?;
    
    info!("GitLab API client initialized successfully.");
    let gitlab_client = Arc::new(gitlab_client);

    // Create polling service
    let config_arc = Arc::new(app_settings);
    let polling_service = PollingService::new(gitlab_client, config_arc.clone());
    
    info!("Starting polling service with interval of {} seconds...", config_arc.poll_interval_seconds);
    
    // Start polling (this will run indefinitely)
    polling_service.start_polling().await?;
    
    Ok(())
}
