# Build stage
FROM rust:1.88-slim AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --bin chat-orch

# Run stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -u 10001 -m app

WORKDIR /app
COPY --from=builder /app/target/release/chat-orch /usr/local/bin/chat-orch

USER app
EXPOSE 3000

CMD ["chat-orch"]
