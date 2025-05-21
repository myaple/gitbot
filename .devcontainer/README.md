# Development Container for GitBot

This directory contains configuration files for setting up a development environment using GitHub Codespaces or VS Code Remote Containers.

## Features

- Rust development environment with the latest stable toolchain
- Pre-installed development tools:
  - rust-analyzer for code intelligence
  - clippy for linting
  - rustfmt for code formatting
- Git and GitHub CLI for source control
- SSL development libraries for network-related dependencies
- Automatic dependency checking on container creation

## Usage

### GitHub Codespaces

1. Go to the GitHub repository
2. Click on the "Code" button
3. Select the "Codespaces" tab
4. Click "Create codespace on [branch]"

### VS Code Remote Containers

1. Install the [Remote Development extension pack](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.vscode-remote-extensionpack) in VS Code
2. Clone the repository locally
3. Open the repository in VS Code
4. Click on the green button in the bottom-left corner of VS Code
5. Select "Reopen in Container"

## Customization

You can customize the development environment by modifying:

- `devcontainer.json`: Container configuration and VS Code settings
- `Dockerfile`: Container image definition and package installation
- `docker-compose.yml`: Multi-container setup if needed