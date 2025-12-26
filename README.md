# GitBot 🤖

[![Rust CI/CD](https://github.com/myaple/gitbot/actions/workflows/rust.yml/badge.svg)](https://github.com/myaple/gitbot/actions/workflows/rust.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70+-blue.svg)](https://www.rust-lang.org)
[![GitLab](https://img.shields.io/badge/GitLab-compatible-orange.svg)](https://gitlab.com)

> A powerful GitLab bot that provides Copilot-like LLM assistance for GitLab repositories

GitBot monitors GitLab repositories for mentions and responds with AI-powered summaries and assistance for issues and merge requests. It provides intelligent code analysis, repository context, and automated responses without requiring GitLab Ultimate licensing. Compatible with OpenAI and OpenAI-compatible API endpoints.

## ✨ Features

- 🔍 **Smart Repository Analysis**: Automatically indexes and searches repository content for relevant context
- 🤖 **AI-Powered Responses**: Uses OpenAI-compatible LLMs to provide intelligent summaries and assistance
- 📝 **Issue & MR Support**: Responds to mentions in both issues and merge requests with contextual information
- 🎯 **Slash Commands**: Specialized commands for planning, fixing, security reviews, documentation, tests, and postmortems
- 🏷️ **Automatic Labeling**: AI-powered automatic issue labeling to triage unlabeled issues
- 🔄 **Smart Retry Logic**: Automatic retry with exponential backoff for transient API failures
- 🕰️ **Stale Issue Management**: Automatically tracks and labels stale issues based on configurable time periods
- 🔄 **Real-time Polling**: Continuously monitors repositories for new activity and mentions
- 🐳 **Docker Ready**: Fully containerized for easy deployment and scaling
- ⚡ **High Performance**: Built with Rust for optimal speed and memory efficiency

## 🚀 Quick Start

### Prerequisites

- Rust (latest stable version recommended)
- GitLab account with API access
- OpenAI API key or compatible LLM endpoint (e.g., Azure OpenAI, local models with OpenAI-compatible APIs)

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
     --openai-api-key YOUR_API_KEY \
     --bot-username YOUR_BOT_USERNAME \
     --repos-to-poll group/project1,group/project2 \
     --context-lines 15
   ```

### Using Environment Variables

```bash
export GITBOT_GITLAB_TOKEN=YOUR_GITLAB_TOKEN
export GITBOT_OPENAI_API_KEY=YOUR_API_KEY
export GITBOT_BOT_USERNAME=YOUR_BOT_USERNAME
export GITBOT_REPOS_TO_POLL=group/project1,group/project2
export GITBOT_CONTEXT_LINES=15
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
     -e GITBOT_OPENAI_API_KEY="YOUR_API_KEY" \
     -e GITBOT_BOT_USERNAME="YOUR_BOT_USERNAME" \
     -e GITBOT_REPOS_TO_POLL="group/project1,group/project2" \
     -e GITBOT_CONTEXT_LINES="15" \
     gitbot
   ```

## 🎯 Slash Commands

GitBot supports specialized slash commands that provide targeted AI assistance for different tasks:

### Issue Commands

| Command | Description |
|---------|-------------|
| `/plan` | Create a detailed implementation plan with executive summary, technical approach, phased implementation breakdown, risk assessment, testing strategy, and success metrics |
| `/fix` | Analyze bugs and provide specific fixes with root cause analysis (5 Whys methodology), implementation changes, target file priorities, testing strategy, and risk assessment |
| `/postmortem` | Generate comprehensive incident postmortems with executive summary, timeline (automatically extracted from issue history), root cause analysis, impact assessment, SMART action items, lessons learned, and follow-up plan |

### Merge Request Commands

| Command | Description |
|---------|-------------|
| `/security` | Perform comprehensive security review based on OWASP Top 10 2021 with severity ratings (Critical/High/Medium/Low), CVSS-like scoring, and specific remediation recommendations |
| `/docs` | Generate language-agnostic documentation including module/class-level docs, function/method documentation, usage examples, and parameter/return value descriptions |
| `/tests` | Suggest comprehensive test coverage including unit tests, integration tests, edge cases, property-based testing, and coverage thresholds |
| `/summarize` | Provide detailed MR analysis including guideline adherence, performance impact, code quality review, risk assessment, strengths, areas for improvement, and recommendations |

### General Commands

| Command | Description |
|---------|-------------|
| `/help` | Display all available slash commands with descriptions |

**Usage Example:**
```
@gitbot /plan
@gitbot /plan focus on the authentication module
@gitbot /security check for SQL injection vulnerabilities
```

You can add additional context after any command to guide the AI's analysis.

## ⚙️ Configuration

GitBot supports extensive configuration through command line arguments or environment variables:

| Environment Variable | CLI Argument | Default | Description |
|---------------------|--------------|---------|-------------|
| `GITBOT_GITLAB_URL` | `--gitlab-url` | `https://gitlab.com` | GitLab instance URL |
| `GITBOT_GITLAB_TOKEN` | `--gitlab-token` | - | GitLab API token (required) |
| `GITBOT_OPENAI_API_KEY` | `--openai-api-key` | - | API key for OpenAI or compatible LLM endpoint (required) |
| `GITBOT_BOT_USERNAME` | `--bot-username` | - | Bot username on GitLab (required) |
| `GITBOT_REPOS_TO_POLL` | `--repos-to-poll` | - | Comma-separated list of repositories (required) |
| `GITBOT_OPENAI_MODEL` | `--openai-model` | `gpt-3.5-turbo` | Model name to use (e.g., gpt-3.5-turbo, gpt-4, or compatible model) |
| `GITBOT_POLL_INTERVAL_SECONDS` | `--poll-interval-seconds` | `60` | Polling interval in seconds |
| `GITBOT_STALE_ISSUE_DAYS` | `--stale-issue-days` | `30` | Days after which issues are marked stale |
| `GITBOT_CONTEXT_LINES` | `--context-lines` | `10` | Number of lines before and after keyword matches |
| `GITBOT_LOG_LEVEL` | `--log-level` | `info` | Log level (trace, debug, info, warn, error) |
| `GITBOT_OPENAI_TIMEOUT_SECS` | `--openai-timeout-secs` | `120` | Request timeout in seconds |
| `GITBOT_OPENAI_CONNECT_TIMEOUT_SECS` | `--openai-connect-timeout-secs` | `10` | Connection timeout in seconds |
| `GITBOT_OPENAI_MAX_RETRIES` | `--openai-max-retries` | `3` | Maximum number of retry attempts (max: 10) |
| `GITBOT_AUTO_TRIAGE_ENABLED` | `--auto-triage-enabled` | `true` | Enable automatic issue labeling/triage |
| `GITBOT_LABEL_LEARNING_SAMPLES` | `--label-learning-samples` | `3` | Number of sample issues to analyze per label |
| `GITBOT_TRIAGE_LOOKBACK_HOURS` | `--triage-lookback-hours` | `24` | Hours to look back for unlabeled issues to triage |

<details>
<summary>View all configuration options</summary>

| Environment Variable | CLI Argument | Default | Description |
|---------------------|--------------|---------|-------------|
| `GITBOT_OPENAI_CUSTOM_URL` | `--openai-custom-url` | `https://api.openai.com/v1` | Custom API URL for OpenAI-compatible endpoints |
| `GITBOT_OPENAI_TEMPERATURE` | `--openai-temperature` | `0.7` | Temperature for AI responses (0.0-1.0) |
| `GITBOT_OPENAI_MAX_TOKENS` | `--openai-max-tokens` | `1024` | Maximum tokens in AI responses |
| `GITBOT_OPENAI_TOKEN_MODE` | `--openai-token-mode` | `max_tokens` | Token parameter mode: "max_tokens" or "max_completion_tokens" |
| `GITBOT_OPENAI_RETRY_INITIAL_DELAY_MS` | `--openai-retry-initial-delay-ms` | `1000` | Initial retry delay in milliseconds |
| `GITBOT_OPENAI_RETRY_MAX_DELAY_MS` | `--openai-retry-max-delay-ms` | `30000` | Maximum retry delay in milliseconds |
| `GITBOT_OPENAI_RETRY_BACKOFF_MULTIPLIER` | `--openai-retry-backoff-multiplier` | `2.0` | Exponential backoff multiplier (1.0-10.0) |
| `GITBOT_MAX_AGE_HOURS` | `--max-age-hours` | `24` | Maximum age for issues/MRs to process |
| `GITBOT_CONTEXT_REPO_PATH` | `--context-repo-path` | - | Additional repository for context |
| `GITBOT_MAX_CONTEXT_SIZE` | `--max-context-size` | `60000` | Maximum context characters |
| `GITBOT_MAX_COMMENT_LENGTH` | `--max-comment-length` | `1000` | Maximum characters per comment in context |
| `GITBOT_MAX_TOOL_CALLS` | `--max-tool-calls` | `3` | Maximum tool calls per bot invocation (max: 10) |
| `GITBOT_DEFAULT_BRANCH` | `--default-branch` | `main` | Default branch name |
| `GITBOT_PROMPT_PREFIX` | `--prompt-prefix` | - | Optional prefix to prepend to every prompt sent to the LLM |
| `GITBOT_CLIENT_CERT_PATH` | `--client-cert-path` | - | Path to client certificate file for mTLS authentication |
| `GITBOT_CLIENT_KEY_PATH` | `--client-key-path` | - | Path to client private key file for mTLS authentication |
| `GITBOT_CLIENT_KEY_PASSWORD` | *env only* | - | Password for client private key (environment variable only) |

</details>

## 📖 Usage

1. **Set up the bot**: Configure GitBot with your GitLab and LLM API credentials
2. **Add to repositories**: Ensure the bot user has access to the repositories you want to monitor
3. **Mention the bot**: Use `@your-bot-username` in issue or MR comments to trigger responses
4. **Get AI assistance**: The bot will analyze the context and provide intelligent responses

### Example Interactions

**General Questions:**
```
@gitbot Can you help me understand this bug report?
@gitbot What files are relevant to this feature request?
```

**Slash Commands (Issues):**
```
@gitbot /plan
@gitbot /plan focus on the authentication module
@gitbot /fix this memory leak in the database connection
@gitbot /postmortem for the production outage
```

**Slash Commands (Merge Requests):**
```
@gitbot /summarize
@gitbot /security check for OWASP Top 10 vulnerabilities
@gitbot /docs for the new API endpoints
@gitbot /tests for the user registration flow
```

### Client Certificate Authentication

GitBot supports client certificate authentication for OpenAI-compatible endpoints that require mTLS (mutual TLS) authentication:

```bash
export GITBOT_CLIENT_CERT_PATH=/path/to/client.crt
export GITBOT_CLIENT_KEY_PATH=/path/to/client.key
export GITBOT_CLIENT_KEY_PASSWORD=your_key_password  # Optional, for encrypted keys
```

**Supported certificate formats:**
- **PKCS#12** (`.p12`, `.pfx`): Combined certificate and key file with password protection
- **PEM**: Separate certificate (`.crt`, `.pem`) and private key (`.key`) files

**Note:** The `GITBOT_CLIENT_KEY_PASSWORD` environment variable is only available as an environment variable for security reasons (no CLI argument).

## 🔄 LLM API Retry Logic

GitBot includes automatic retry logic with exponential backoff for transient API failures. The client automatically retries on:

- Network timeouts (both request and connection)
- Connection errors (broken pipe, connection reset)
- HTTP 5xx server errors
- HTTP 429 rate limit errors
- HTTP 408 request timeout errors

The client does NOT retry on:
- HTTP 4xx client errors (except 408 and 429)
- Authentication failures (401)
- Validation errors (400)

### Retry Configuration

Configure retry behavior using these environment variables:

```bash
export GITBOT_OPENAI_TIMEOUT_SECS=120              # Request timeout (default: 120)
export GITBOT_OPENAI_CONNECT_TIMEOUT_SECS=10       # Connection timeout (default: 10)
export GITBOT_OPENAI_MAX_RETRIES=3                 # Max retries (default: 3, max: 10)
export GITBOT_OPENAI_RETRY_INITIAL_DELAY_MS=1000   # Initial delay (default: 1000ms)
export GITBOT_OPENAI_RETRY_MAX_DELAY_MS=30000      # Max delay (default: 30000ms)
export GITBOT_OPENAI_RETRY_BACKOFF_MULTIPLIER=2.0  # Backoff multiplier (default: 2.0)
```

**Example:** With default settings, retries will be attempted at: 1s, 2s, 4s, 8s, 16s, 30s (capped).

## 🔍 Prompt Prefix Customization

GitBot allows you to add a consistent prefix to every prompt sent to the LLM. This is useful for:
- Setting a specific persona or role for the bot
- Adding formatting requirements
- Including security guidelines
- Specifying response length preferences

### Usage

Set the prefix using either environment variable or command-line argument:

```bash
# Environment variable
GITBOT_PROMPT_PREFIX="You are an expert software developer. Always provide detailed explanations." ./gitbot

# Command-line argument
./gitbot --prompt-prefix "You are an expert software developer. Always provide detailed explanations."
```

### Example

With this configuration:
```bash
GITBOT_PROMPT_PREFIX="You are a senior Python developer. Always include code examples in your responses."
```

When a user asks:
```
@gitbot How can I optimize this function?
```

The actual prompt sent to the LLM will be:
```
You are a senior Python developer. Always include code examples in your responses.

How can I optimize this function?
```

This ensures consistent behavior across all interactions.

## 🏷️ Automatic Issue Labeling

GitBot includes automatic issue labeling (triage) to help categorize unlabeled issues using AI. The bot learns from existing labeled issues in your repository and applies appropriate labels to new issues.

### How It Works

1. **Learning Phase**: GitBot analyzes a sample of existing labeled issues to understand what each label means
2. **Detection**: Identifies unlabeled issues within the lookback period
3. **Labeling**: Uses AI to determine appropriate labels based on the learned patterns
4. **Application**: Automatically applies the suggested labels to the issue

### Configuration

```bash
export GITBOT_AUTO_TRIAGE_ENABLED=true                # Enable/disable auto-triage (default: true)
export GITBOT_LABEL_LEARNING_SAMPLES=3                # Sample issues per label to learn from (default: 3)
export GITBOT_TRIAGE_LOOKBACK_HOURS=24                # Hours to look back for unlabeled issues (default: 24)
```

**Note**: Auto-triage requires at least 3 labeled issues per label type to learn effectively. Ensure your repository has sufficient labeled data before enabling this feature.

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