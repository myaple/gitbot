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
./target/debug/gitbot
# or for release
./target/release/gitbot
```

## Features

Gitbot comments on issues and merge requests when tagged, providing
an LLM-powered summary of the issue or MR, including repo context
and code. 

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on how to contribute to this project.

## License

MIT License.