# Contributing to gitbot

We welcome contributions to `gitbot`! Please follow these guidelines to ensure a smooth process.

## Getting Started

-   Ensure you have the latest stable version of Rust installed.
-   Fork the repository and create a new branch for your feature or bug fix.

## Development Process

1.  **Code Style**:
    *   Format your code using `rustfmt`. Before committing, run:
        ```bash
        cargo fmt
        ```
    *   Ensure your code is free of linter warnings by running `clippy`:
        ```bash
        cargo clippy -- -D warnings
        ```
        Address any warnings reported by `clippy`.

2.  **Commits**:
    *   Write clear and concise commit messages.
    *   Follow conventional commit message formats if possible (e.g., `feat: add new feature`, `fix: resolve bug`).

3.  **Testing**:
    *   (Placeholder: Add details about writing and running tests if applicable. For now, we'll assume `cargo test` is the standard.)
    *   Ensure all existing tests pass by running:
        ```bash
        cargo test
        ```

4.  **Pull Requests (PRs)**:
    *   Once your changes are ready, push your branch to your fork and open a Pull Request against the `main` branch of the original repository.
    *   Provide a clear description of the changes in your PR.
    *   Ensure your PR passes all CI checks (which will be set up to enforce formatting and clippy warnings).
    *   Be prepared to address any feedback or requested changes.

## Reporting Bugs

If you find a bug, please open an issue on the GitHub repository with the following information:
-   A clear and descriptive title.
-   Steps to reproduce the bug.
-   What you expected to happen.
-   What actually happened.
-   Your environment (OS, Rust version, etc.).

## Suggesting Enhancements

If you have an idea for a new feature or an improvement, please open an issue to discuss it before starting work. This helps ensure that your contribution aligns with the project's goals.

Thank you for contributing!
