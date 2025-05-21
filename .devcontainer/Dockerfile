FROM mcr.microsoft.com/devcontainers/rust:latest

# Install additional packages
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get -y install --no-install-recommends \
    pkg-config \
    libssl-dev \
    && apt-get clean -y \
    && rm -rf /var/lib/apt/lists/*

# Install additional Rust components
RUN rustup component add clippy rustfmt

# Set up environment
ENV RUST_BACKTRACE=1