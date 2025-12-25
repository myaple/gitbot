# GooseHints for GitBot project

# General repository info
README.md
CONTRIBUTING.md
Dockerfile
.github/workflows/rust.yml

# Rust source files
src/*.rs

# Test files
src/tests/**/*.rs

# Exclude build artifacts and dependencies
!target/

# Docker
Dockerfile
.devcontainer/Dockerfile

# Relevant docs
.devcontainer/README.md

# Highlights from README.md to focus on config and usage
# This project is a Rust-based GitLab bot using OpenAI GPT models
# Key config involves env vars like GITBOT_GITLAB_TOKEN, GITBOT_OPENAI_API_KEY, etc.
# Supports CLI args to override env vars
# Docker usage includes environment variables for deployment

# GitHub Actions workflows
.github/workflows/rust.yml
# It contains build, test, lint, release, and docker image publishing steps, primarily focused on the `gitbot` binary

# Dockerfile indicates a multi-stage build with final image exposing port 8080 and running the `gitbot` binary as nonroot user

# Key files for understanding main program flow:
src/main.rs
src/handlers.rs
src/gitlab.rs
src/openai.rs
src/repo_context.rs

# OpenAI Client Configuration
# The OpenAI client supports automatic retry with exponential backoff for transient failures
# Retry behavior is configurable via environment variables:
# GITBOT_OPENAI_TIMEOUT_SECS=120              # Request timeout in seconds (default: 120)
# GITBOT_OPENAI_CONNECT_TIMEOUT_SECS=10       # Connection timeout in seconds (default: 10)
# GITBOT_OPENAI_MAX_RETRIES=3                 # Maximum number of retry attempts (default: 3, max: 10)
# GITBOT_OPENAI_RETRY_INITIAL_DELAY_MS=1000   # Initial retry delay in milliseconds (default: 1000)
# GITBOT_OPENAI_RETRY_MAX_DELAY_MS=30000      # Maximum retry delay in milliseconds (default: 30000)
# GITBOT_OPENAI_RETRY_BACKOFF_MULTIPLIER=2.0  # Exponential backoff multiplier (default: 2.0)
#
# The client automatically retries on:
# - Network timeouts (both request and connection)
# - Connection errors (broken pipe, connection reset)
# - HTTP 5xx server errors
# - HTTP 429 rate limit errors
# - HTTP 408 request timeout errors
#
# The client does NOT retry on:
# - HTTP 4xx client errors (except 408 and 429)
# - Authentication failures (401)
# - Validation errors (400)
# - URL parsing errors
# - File I/O errors

# Development commands
# Always run the following during development:
# - `cargo check` to ensure it compiles
# - `cargo clippy` to ensure it lints
# - `cargo fmt` to ensure it is formatted properly
