# Build stage
FROM rust:1.88-slim AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY chat-orch/Cargo.toml chat-orch/Cargo.lock ./
COPY chat-orch/src ./src

RUN cargo build --release --bin chat-orch

# Run stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -u 10001 -m app

WORKDIR /app
COPY --from=builder /app/target/release/chat-orch /usr/local/bin/chat-orch

USER app
EXPOSE 3000

CMD ["chat-orch"]
