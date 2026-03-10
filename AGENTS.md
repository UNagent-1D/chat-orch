# AGENTS.md — Chat Orchestrator (General Orchestrator)

## Overview

This is the **General Orchestrator** for a multi-tenant conversational AI platform.
It acts as the universal ingestion point for chat messages from any channel
(Telegram, WhatsApp, future sources), normalizes them, resolves tenants, manages
sessions, executes LLM-powered conversation turns with tool calling, and delivers
responses back to the originating channel.

- **Language**: Rust (edition 2021, MSRV 1.75)
- **Framework**: Axum 0.7 + Tokio (multi-threaded async runtime)
- **Scale target**: 100k msg/sec (architecture-ready)
- **License**: MIT

## Build & Run

```bash
# Install dependencies and compile
cargo build

# Run in development (requires .env file — copy from .env.example)
cargo run

# Run tests
cargo test

# Run benchmarks
cargo bench

# Run with release optimizations
cargo run --release

# Docker
docker compose up --build
```

## Project Structure

```
src/
  main.rs            — Server bootstrap, tracing, graceful shutdown
  config.rs          — AppConfig loaded from environment variables
  state.rs           — AppState: caches, clients, semaphore, Redis pools
  router.rs          — Axum router: webhook routes, REST routes, middleware
  error.rs           — AppError enum with IntoResponse impl
  conversation.rs    — Turn processing logic (session + config + LLM)

  types/             — Domain types (IngestMessage, ResolvedMessage, etc.)
  ingest/            — Channel webhook handlers (Telegram, WhatsApp)
  gateway/           — HTTP clients for downstream services + caches
  pipeline/          — Concurrency: semaphore-bounded task spawning
  llm/               — LLM client trait, OpenAI impl, tool executor
  auth/              — JWT validation for REST endpoints
```

## Architecture

Messages flow through this pipeline:

1. **Ingest** — Webhook arrives, signature verified, payload parsed, normalized to `IngestMessage`
2. **Pipeline** — Semaphore permit acquired, `tokio::spawn` background task
3. **Resolve** — Channel key mapped to tenant via moka cache (backed by Tenant Service)
4. **Session** — Redis session created or retrieved (composite key: tenant+channel+user)
5. **Conversation** — Agent config fetched, LLM turn loop executed (with tool calls)
6. **Reply** — Response formatted for originating channel and sent via channel API

## Key Conventions

- **TypeState pattern**: `IngestMessage` (tenant unknown) vs `ResolvedMessage` (tenant guaranteed).
  A `ResolvedMessage` always has a `tenant_id` — enforced at compile time.
- **Handler-per-channel**: Each channel (Telegram, WhatsApp) has its own Axum handler.
  No shared trait for MVP — extract when 3rd channel arrives.
- **Webhook handlers return fast**: Acquire semaphore, spawn task, return 200/503.
  Processing happens in the background. Replies sent via channel API.
- **REST handlers are synchronous**: `/conversation/chat/turn` awaits the response
  and returns it in the HTTP body. Does NOT go through the semaphore pipeline.
- **4 independent moka caches** with different TTLs for downstream read-only data.
- **Redis** for sessions (write-heavy, cross-replica) and dedup (atomic SETNX).
- **Error handling**: `thiserror` for typed errors, `anyhow` in main.rs only.
  All errors implement `IntoResponse` via `AppError`.
- Never download media content — store `file_id` or URL references only.
- Unsupported message types get a polite fallback reply, never silently dropped.

## Environment

Requires a `.env` file — see `.env.example` for all variables.
Critical dependencies: Redis, Tenant Service (Go), Agent Config Registry (Go).

## Testing

- **Unit/integration tests**: `cargo test` — uses `wiremock` crate for HTTP mocking
- **Benchmarks**: `cargo bench` — uses `criterion` for throughput measurement
- **Load tests**: `scripts/load_test.sh` — uses k6 against running instance
- **Docker tests**: `docker compose up` — full stack with mock services

## Downstream Services

| Service | Language | What We Call |
|---------|----------|--------------|
| Tenant Service | Go + Gin | `GET /api/v1/tenants/:id`, `GET /internal/resolve-channel` |
| Agent Config Registry | Go + Gin | `GET /api/v1/tenants/:id/profiles/:pid/configs/active` |
| OpenAI API | — | `POST /v1/chat/completions` |
| Telegram Bot API | — | `POST /bot<TOKEN>/sendMessage` |
| WhatsApp Graph API | — | `POST /v18.0/<PNID>/messages` |
