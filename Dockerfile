# Horcrux API Server Dockerfile
# Multi-stage build for optimized image size

# Stage 1: Build stage
FROM rust:1.85-slim AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    libvirt-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy dependency manifests
COPY Cargo.toml Cargo.lock ./
COPY horcrux-api/Cargo.toml ./horcrux-api/
COPY horcrux-api/horcrux-ui/Cargo.toml ./horcrux-api/horcrux-ui/
COPY horcrux-cli/Cargo.toml ./horcrux-cli/
COPY horcrux-common/Cargo.toml ./horcrux-common/
COPY horcrux-mobile/Cargo.toml ./horcrux-mobile/
COPY terraform-provider-horcrux/Cargo.toml ./terraform-provider-horcrux/

# Create dummy source files to cache dependencies
RUN mkdir -p horcrux-api/src horcrux-api/horcrux-ui/src horcrux-cli/src horcrux-common/src horcrux-mobile/src terraform-provider-horcrux/src && \
    echo "fn main() {}" > horcrux-api/src/main.rs && \
    echo "pub fn dummy() {}" > horcrux-api/src/lib.rs && \
    echo "pub fn dummy() {}" > horcrux-api/horcrux-ui/src/lib.rs && \
    echo "fn main() {}" > horcrux-cli/src/main.rs && \
    echo "pub fn dummy() {}" > horcrux-common/src/lib.rs && \
    echo "pub fn dummy() {}" > horcrux-mobile/src/lib.rs && \
    echo "pub fn dummy() {}" > terraform-provider-horcrux/src/lib.rs

# Build dependencies (this layer will be cached)
RUN cargo build --release -p horcrux-api || true
RUN rm -rf target/release/.fingerprint/horcrux-* && \
    rm -rf horcrux-*/src terraform-provider-horcrux/src

# Copy actual source code
COPY horcrux-api ./horcrux-api
COPY horcrux-cli ./horcrux-cli
COPY horcrux-common ./horcrux-common
COPY horcrux-mobile ./horcrux-mobile
COPY terraform-provider-horcrux ./terraform-provider-horcrux
COPY docs/openapi.yaml ./docs/openapi.yaml

# Build the actual application
RUN cargo build --release -p horcrux-api

# Stage 2: Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libsqlite3-0 \
    libvirt0 \
    qemu-system-x86 \
    qemu-utils \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create horcrux user
RUN useradd -m -u 1000 -s /bin/bash horcrux

# Create necessary directories
RUN mkdir -p /var/lib/horcrux /var/log/horcrux /etc/horcrux && \
    chown -R horcrux:horcrux /var/lib/horcrux /var/log/horcrux /etc/horcrux

# Copy binary from builder
COPY --from=builder /app/target/release/horcrux-api /usr/local/bin/horcrux-api

# Copy default configuration
COPY deploy/config.toml.example /etc/horcrux/config.toml

# Adjust config for container environment
RUN sed -i 's|/var/lib/horcrux/horcrux.db|/var/lib/horcrux/horcrux.db|g' /etc/horcrux/config.toml && \
    sed -i 's|bind_address = "127.0.0.1:8006"|bind_address = "0.0.0.0:8006"|g' /etc/horcrux/config.toml

# Set ownership
RUN chown horcrux:horcrux /usr/local/bin/horcrux-api

# Switch to horcrux user
USER horcrux

# Expose API port
EXPOSE 8006

# Expose VNC ports for VM consoles (5900-5999)
EXPOSE 5900-5999

# Volume for persistent data
VOLUME ["/var/lib/horcrux", "/var/log/horcrux"]

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD curl -f http://localhost:8006/api/health || exit 1

# Set working directory
WORKDIR /var/lib/horcrux

# Run the application
CMD ["/usr/local/bin/horcrux-api"]
