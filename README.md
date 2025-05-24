# gitbot

This is a bot that interacts specifically with GitLab repositories
to provide Copilot-like LLM assistance in the absence of GitLab
Ultimate licensing.

## Prerequisites

- Rust (latest stable version recommended)

## Getting Started

### Building

1.  Clone the repository:
    ```bash
    git clone <repository-url>
    cd gitbot
    ```
2.  Build the project:
    ```bash
    cargo build
    ```
    For a release build:
    ```bash
    cargo build --release
    ```
    The executable will be located at `target/debug/gitbot` or `target/release/gitbot`.

### Running

To run the bot:
```bash
./target/debug/gitbot --gitlab-token YOUR_GITLAB_TOKEN --openai-api-key YOUR_OPENAI_KEY --bot-username YOUR_BOT_USERNAME --repos-to-poll group/project1,group/project2
# or for release
./target/release/gitbot --gitlab-token YOUR_GITLAB_TOKEN --openai-api-key YOUR_OPENAI_KEY --bot-username YOUR_BOT_USERNAME --repos-to-poll group/project1,group/project2
```

For a full list of available options:
```bash
./target/debug/gitbot --help
```

You can also set configuration via environment variables:
```bash
export APP_GITLAB_TOKEN=YOUR_GITLAB_TOKEN
export APP_OPENAI_API_KEY=YOUR_OPENAI_KEY
export APP_BOT_USERNAME=YOUR_BOT_USERNAME
export APP_REPOS_TO_POLL=group/project1,group/project2
./target/debug/gitbot
```

## Features

Gitbot comments on issues and merge requests when tagged, providing
an LLM-powered summary of the issue or MR, including repo context
and code. 

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on how to contribute to this project.

## License

MIT License.