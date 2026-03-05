# Multi-stage Dockerfile using cargo-chef for fast rebuilds.
#
# Stage 1: Planner — generate a recipe file from Cargo.toml + Cargo.lock
# Stage 2: Builder — compile dependencies (cached) then compile app
# Stage 3: Runtime — minimal image with only the binary

# ── Stage 1: Builder ─────────────────────────────────────────────────
FROM rust:1.88-slim AS builder
WORKDIR /app

# Install build deps for crypto crates
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Cache dependency build: copy manifests first, build with a dummy main
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src benches && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs && echo 'fn main() {}' > benches/throughput.rs
RUN cargo build --release && rm -rf src benches target/release/deps/chat_orch*

# Copy real source and build
COPY src/ ./src/
COPY benches/ ./benches/
RUN cargo build --release

# ── Stage 3: Runtime ─────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
WORKDIR /app

# Install CA certificates for HTTPS calls to external APIs
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/chat-orch /app/chat-orch

# Non-root user
RUN groupadd -r appuser && useradd -r -g appuser appuser
USER appuser

EXPOSE 3000

HEALTHCHECK --interval=15s --timeout=3s --start-period=5s --retries=3 \
    CMD ["/app/chat-orch", "--health-check"] || exit 1

ENTRYPOINT ["/app/chat-orch"]
