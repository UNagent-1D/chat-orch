# AGENTS.md — Chat Orchestrator

## Overview

Thin HTTP front-door that forwards chat requests from the frontend to
`conversation-chat` (Go, :8082). `conversation-chat` owns sessions, LLM turns,
tool calls, and history. `chat-orch` does nothing else.

- **Language**: Rust (edition 2021)
- **Framework**: Axum 0.7 + Tokio
- **License**: MIT

The authoritative scope doc is [`REFACTOR_PROMPT.md`](REFACTOR_PROMPT.md).
Anything outside that spec is out of scope.

## Build & Run

```bash
cargo build
cargo run            # needs .env — copy from .env.example
cargo test
cargo run --release
docker build -t chat-orch .
```

## Project Structure

```
src/
  main.rs     Server bootstrap, tracing, graceful shutdown (SIGTERM/SIGINT)
  config.rs   AppConfig (9 env vars) + unit tests
  error.rs    AppError enum + IntoResponse + From<reqwest::Error>
  lib.rs      Module declarations + AppState
  routes.rs   /health and /v1/chat handlers
  gateway.rs  ConversationChatClient (reqwest wrapper)
```

## Endpoints

- `GET /health` → `{"status":"ok"}`.
- `POST /v1/chat` with `{tenant_id, session_id?, message}` →
  opens a session in conversation-chat if needed, forwards the turn,
  returns the downstream body verbatim plus the `session_id`.

Errors: 400 for malformed body, 502 when conversation-chat is unreachable,
500 for anything else. Body always `{"error": "..."}`.

## Conventions

- One binary, one crate. No sub-crates.
- `axum::extract::State` for the shared `reqwest::Client` + config.
- `thiserror::Error` on `AppError`. `anyhow` only in `main.rs`.
- `serde::Deserialize`/`Serialize` on all request/response types. No manual
  JSON parsing.
- Graceful shutdown on SIGTERM + SIGINT via `tokio::signal`.
- `tracing_subscriber` respects `LOG_FORMAT` (`json`|`pretty`) and `RUST_LOG`.
- No `unwrap()` outside tests — use `?` + typed errors.

## Environment

Needs a `.env` file — see [`.env.example`](.env.example). Nine variables.
Required: `CONVERSATION_CHAT_URL`, `TENANT_SERVICE_URL`, `OPENAI_API_KEY`.

## Testing

- `cargo test` — unit tests for `AppConfig::from_env` (happy path + missing
  required var).
- `cargo clippy -- -D warnings` — must be clean.
- End-to-end smoke: `docker build && docker run` against a live
  `conversation-chat`.

## Out of Scope

Session persistence, tool execution, LLM calls from inside chat-orch,
channel webhooks, webhook signature verification, auth middleware, rate
limiting, metrics, OpenTelemetry, retries, circuit breakers.
