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

# Development commands
# Always run the following during development:
# - `cargo check` to ensure it compiles
# - `cargo clippy` to ensure it lints
# - `cargo fmt` to ensure it is formatted properly
