# Multi-stage Dockerfile using cargo-chef for fast rebuilds.
#
# Stage 1: Planner — generate a recipe file from Cargo.toml + Cargo.lock
# Stage 2: Builder — compile dependencies (cached) then compile app
# Stage 3: Runtime — minimal image with only the binary

# ── Stage 1: Planner ─────────────────────────────────────────────────
FROM rust:1.82-slim AS planner
WORKDIR /app

RUN cargo install cargo-chef --locked
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 2: Builder ─────────────────────────────────────────────────
FROM rust:1.82-slim AS builder
WORKDIR /app

RUN cargo install cargo-chef --locked

# Install build deps for crypto crates
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Cook dependencies (cached layer)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Copy source and build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
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
