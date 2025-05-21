use actix_web::{web, App, HttpServer};
use crate::config::{load_config, AppSettings};
use crate::gitlab::GitlabApiClient;
use crate::openai::OpenAIApiClient;
use crate::handlers::gitlab_webhook_handler;
use tracing_subscriber::{fmt, EnvFilter};
use tracing::info;

mod config;
mod gitlab;
mod handlers;
mod models;
mod openai;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Initialize Logging (initial basic setup)
    let initial_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    fmt::subscriber().with_env_filter(initial_filter).init();

    info!("Starting application...");

    // Load Configuration
    let app_settings = match load_config() {
        Ok(cfg) => {
            info!("Configuration loaded successfully.");
            cfg
        }
        Err(e) => {
            eprintln!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    };
    
    // Re-initialize logging with level from config if RUST_LOG is not set
    // This ensures that the config's log level is respected.
    let log_level_from_config = app_settings.log_level.clone();
    let final_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_level_from_config));
    fmt::subscriber().with_env_filter(final_filter).init(); // Re-init with potentially new level

    info!("Configuration loaded and logger re-initialized with config log level if applicable.");

    let app_settings_data = web::Data::new(app_settings.clone()); // Clone for app_settings ownership by this data wrapper

    // Initialize API Clients
    let gitlab_client = match GitlabApiClient::new(&app_settings) {
        Ok(client) => {
            info!("GitLab API client initialized successfully.");
            client
        }
        Err(e) => {
            eprintln!("Failed to create GitLab client: {}", e);
            std::process::exit(1);
        }
    };
    let gitlab_client_data = web::Data::new(gitlab_client);

    let openai_client = match OpenAIApiClient::new(&app_settings) {
        Ok(client) => {
            info!("OpenAI API client initialized successfully.");
            client
        }
        Err(e) => {
            eprintln!("Failed to create OpenAI client: {}", e);
            std::process::exit(1);
        }
    };
    let openai_client_data = web::Data::new(openai_client);


    info!("Starting server on {}...", app_settings.server_address);

    HttpServer::new(move || {
        App::new()
            .app_data(app_settings_data.clone())
            .app_data(gitlab_client_data.clone())
            .app_data(openai_client_data.clone())
            .route("/webhook", web::post().to(gitlab_webhook_handler))
    })
    .bind(&app_settings.server_address)?
    .run()
    .await
}
