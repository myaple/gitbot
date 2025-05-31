# GitBot Repository Instructions for GitHub Copilot

## Project Overview

GitBot is a Rust-based GitLab bot that provides Copilot-like LLM assistance for GitLab repositories. It monitors GitLab repositories for mentions and responds with AI-powered summaries and assistance for issues and merge requests.

## Repository Structure

### Core Application Files

- **`src/main.rs`** - Entry point that sets up logging, loads configuration, and starts the polling service
- **`src/config.rs`** - Configuration management using clap for CLI arguments and environment variables
- **`src/gitlab.rs`** - GitLab API client implementation with methods for issues, merge requests, notes, and repository operations (~1200 lines)
- **`src/handlers.rs`** - Event handlers for processing mentions in issues and merge requests, integrating with OpenAI API (~1400 lines)
- **`src/polling.rs`** - Polling service that monitors GitLab repositories for new activity and stale issues (~1200 lines)
- **`src/repo_context.rs`** - Repository context extraction that gathers relevant code and documentation for AI prompts (~1300 lines)
- **`src/openai.rs`** - OpenAI API client for LLM interactions and chat completions (~450 lines)
- **`src/models.rs`** - Data structures and serialization models for GitLab API responses (~150 lines)
- **`src/mention_cache.rs`** - Simple cache for preventing duplicate responses to the same mentions (~26 lines)
- **`src/gitlab_ext.rs`** - Legacy file containing mostly tests, methods moved to `gitlab.rs` (~200 lines)
- **`src/polling_test.rs`** - Additional polling service tests (~48 lines)

### Configuration and Documentation

- **`Cargo.toml`** - Rust package configuration with dependencies
- **`CONTRIBUTING.md`** - Development guidelines and contribution instructions
- **`README.md`** - Project documentation with setup and usage instructions
- **`.github/workflows/rust.yml`** - CI/CD pipeline for building, testing, and linting

## Build, Test, and Development Commands

### Building
```bash
cargo build              # Debug build
cargo build --release    # Release build
```

### Testing
```bash
cargo test               # Run all tests (currently 54 tests)
```

### Code Quality
```bash
cargo clippy             # Linting - must report no warnings or errors
cargo fmt                # Code formatting
cargo fmt --check        # Check if code is properly formatted
```

## Code Quality Guidelines

### Function Design
- **Single Concern**: Functions should have a single, well-defined responsibility
- **Minimal Scope**: Individual files should be scoped to deal with a specific piece of functionality
- **Avoid Broad Modules**: Minimize broad scope modules in favor of focused, specific modules

### Testing Standards
- **Test Location**: Tests should reside in the file where the function they are testing is located
- **Test Module**: Tests should be in a module called `tests` using `#[cfg(test)]`
- **All Tests Must Pass**: `cargo test` must pass without failures

### Code Standards
- **Formatting**: All code must be formatted with `cargo fmt`
- **Linting**: `cargo clippy` will report no warnings or errors
- **Error Handling**: Use `anyhow::Result` for error handling patterns

### Git Workflow
- **Single Feature PRs**: All merge requests should be scoped to one feature or change
- **Squash and Rebase**: All pull requests are squashed and rebased, no merge commits
- **Clean History**: Maintain a linear git history

## Architecture Patterns

### Async/Await
The codebase uses tokio for async operations, particularly for:
- HTTP requests to GitLab and OpenAI APIs
- Concurrent polling of multiple repositories
- Non-blocking cache operations

### Error Handling
- Use `anyhow::Result<T>` for most error handling
- Custom error types with `thiserror` for specific error categories (e.g., `GitlabError`)
- Graceful degradation when non-critical operations fail

### Dependency Injection
- Use `Arc<T>` for shared configuration and clients
- Pass dependencies explicitly rather than using global state
- Clone `Arc` references for concurrent operations

### Testing Strategy
- Unit tests in the same file as the code being tested
- Mock external API calls using `mockito`
- Test both success and error scenarios
- Integration tests for complex workflows

## Key Dependencies

- **tokio** - Async runtime
- **reqwest** - HTTP client for API calls
- **serde** - JSON serialization/deserialization
- **clap** - Command line argument parsing
- **anyhow** - Error handling
- **tracing** - Structured logging
- **mockito** - HTTP mocking for tests

## Development Workflow

1. Ensure Rust stable toolchain is installed
2. Run `cargo build` to verify the project compiles
3. Run `cargo test` to ensure all tests pass
4. Run `cargo clippy` to check for linting issues
5. Run `cargo fmt` to format code
6. Make focused, single-concern changes
7. Add tests for new functionality
8. Ensure all CI checks pass before submitting PRs