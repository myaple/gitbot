# GitBot 🤖

[![Rust CI/CD](https://github.com/myaple/gitbot/actions/workflows/rust.yml/badge.svg)](https://github.com/myaple/gitbot/actions/workflows/rust.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70+-blue.svg)](https://www.rust-lang.org)
[![GitLab](https://img.shields.io/badge/GitLab-compatible-orange.svg)](https://gitlab.com)

> A powerful GitLab bot that provides Copilot-like LLM assistance for GitLab repositories

GitBot monitors GitLab repositories for mentions and responds with AI-powered summaries and assistance for issues and merge requests. It provides intelligent code analysis, repository context, and automated responses without requiring GitLab Ultimate licensing.

## ✨ Features

- 🔍 **Smart Repository Analysis**: Automatically indexes and searches repository content for relevant context
- 🤖 **AI-Powered Responses**: Uses OpenAI's GPT models to provide intelligent summaries and assistance  
- 📝 **Issue & MR Support**: Responds to mentions in both issues and merge requests with contextual information
- 🏷️ **Stale Issue Management**: Automatically tracks and labels stale issues based on configurable time periods
- 🔄 **Real-time Polling**: Continuously monitors repositories for new activity and mentions
- 🐳 **Docker Ready**: Fully containerized for easy deployment and scaling
- ⚡ **High Performance**: Built with Rust for optimal speed and memory efficiency

## 🚀 Quick Start

### Prerequisites

- Rust (latest stable version recommended)
- GitLab account with API access
- OpenAI API key

### Installation

1. **Clone the repository:**
   ```bash
   git clone https://github.com/myaple/gitbot.git
   cd gitbot
   ```

2. **Build the project:**
   ```bash
   cargo build --release
   ```

3. **Run with configuration:**
   ```bash
   ./target/release/gitbot \
     --gitlab-token YOUR_GITLAB_TOKEN \
     --openai-api-key YOUR_OPENAI_KEY \
     --bot-username YOUR_BOT_USERNAME \
     --repos-to-poll group/project1,group/project2
   ```

### Using Environment Variables

```bash
export GITBOT_GITLAB_TOKEN=YOUR_GITLAB_TOKEN
export GITBOT_OPENAI_API_KEY=YOUR_OPENAI_KEY  
export GITBOT_BOT_USERNAME=YOUR_BOT_USERNAME
export GITBOT_REPOS_TO_POLL=group/project1,group/project2
./target/release/gitbot
```

### Docker Deployment

1. **Build the Docker image:**
   ```bash
   docker build -t gitbot .
   ```

2. **Run the container:**
   ```bash
   docker run -d --name gitbot \
     -e GITBOT_GITLAB_TOKEN="YOUR_GITLAB_TOKEN" \
     -e GITBOT_OPENAI_API_KEY="YOUR_OPENAI_KEY" \
     -e GITBOT_BOT_USERNAME="YOUR_BOT_USERNAME" \
     -e GITBOT_REPOS_TO_POLL="group/project1,group/project2" \
     gitbot
   ```

## ⚙️ Configuration

GitBot supports extensive configuration through command line arguments or environment variables:

| Environment Variable | CLI Argument | Default | Description |
|---------------------|--------------|---------|-------------|
| `GITBOT_GITLAB_URL` | `--gitlab-url` | `https://gitlab.com` | GitLab instance URL |
| `GITBOT_GITLAB_TOKEN` | `--gitlab-token` | - | GitLab API token (required) |
| `GITBOT_OPENAI_API_KEY` | `--openai-api-key` | - | OpenAI API key (required) |
| `GITBOT_BOT_USERNAME` | `--bot-username` | - | Bot username on GitLab (required) |
| `GITBOT_REPOS_TO_POLL` | `--repos-to-poll` | - | Comma-separated list of repositories (required) |
| `GITBOT_OPENAI_MODEL` | `--openai-model` | `gpt-3.5-turbo` | OpenAI model to use |
| `GITBOT_POLL_INTERVAL_SECONDS` | `--poll-interval-seconds` | `60` | Polling interval in seconds |
| `GITBOT_STALE_ISSUE_DAYS` | `--stale-issue-days` | `30` | Days after which issues are marked stale |
| `GITBOT_LOG_LEVEL` | `--log-level` | `info` | Log level (trace, debug, info, warn, error) |

<details>
<summary>View all configuration options</summary>

| Environment Variable | CLI Argument | Default | Description |
|---------------------|--------------|---------|-------------|
| `GITBOT_OPENAI_CUSTOM_URL` | `--openai-custom-url` | `https://api.openai.com/v1` | Custom OpenAI API URL |
| `GITBOT_OPENAI_TEMPERATURE` | `--openai-temperature` | `0.7` | Temperature for AI responses (0.0-1.0) |
| `GITBOT_OPENAI_MAX_TOKENS` | `--openai-max-tokens` | `1024` | Maximum tokens in AI responses |
| `GITBOT_MAX_AGE_HOURS` | `--max-age-hours` | `24` | Maximum age for issues/MRs to process |
| `GITBOT_CONTEXT_REPO_PATH` | `--context-repo-path` | - | Additional repository for context |
| `GITBOT_MAX_CONTEXT_SIZE` | `--max-context-size` | `60000` | Maximum context characters |
| `GITBOT_DEFAULT_BRANCH` | `--default-branch` | `main` | Default branch name |

</details>

## 📖 Usage

1. **Set up the bot**: Configure GitBot with your GitLab and OpenAI credentials
2. **Add to repositories**: Ensure the bot user has access to the repositories you want to monitor
3. **Mention the bot**: Use `@your-bot-username` in issue or MR comments to trigger responses
4. **Get AI assistance**: The bot will analyze the context and provide intelligent responses

### Example Interactions

```
@gitbot Can you help me understand this bug report?
@gitbot What files are relevant to this feature request?
@gitbot Summarize the recent changes in this merge request
```

## 🛠️ Development

### Building from Source

```bash
# Clone and build
git clone https://github.com/myaple/gitbot.git
cd gitbot
cargo build

# Run tests
cargo test

# Run with development settings  
cargo run -- --help
```

### Code Quality

This project maintains high code quality standards:

```bash
# Format code
cargo fmt

# Run linter  
cargo clippy

# Run all tests
cargo test
```

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on:

- Code style and standards
- Testing requirements  
- Pull request process
- Development workflow

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🆘 Support

- 📝 [Create an issue](https://github.com/myaple/gitbot/issues) for bug reports or feature requests
- 📖 Check the [documentation](README.md) for configuration help
- 💬 Join discussions in the [Issues](https://github.com/myaple/gitbot/issues) section

## 🏗️ Architecture

GitBot is built with:

- **Language**: Rust 🦀
- **Async Runtime**: Tokio  
- **HTTP Client**: Reqwest
- **AI Integration**: OpenAI API
- **Configuration**: Clap + Environment Variables
- **Logging**: Tracing
- **Testing**: Built-in Rust testing + Mockito

---

<div align="center">
  <strong>Built with ❤️ by the GitBot team</strong>
</div>