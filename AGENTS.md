# AGENTS.md — Chat Orchestrator

## Overview

Thin HTTP front-door that forwards chat requests from the frontend to
`conversation-chat` (Go, :8082). `conversation-chat` owns sessions, LLM turns,
tool calls, and history. `chat-orch` does nothing else.

- **Language**: Rust (edition 2021)
- **Framework**: Axum 0.7 + Tokio
- **License**: MIT

The authoritative scope doc is [`TECHNICAL.md`](TECHNICAL.md). Anything
outside that spec is out of scope.

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
  config.rs   AppConfig + unit tests
  error.rs    AppError enum + IntoResponse + From<reqwest::Error>
  lib.rs      Module declarations + AppState
  routes.rs   /health and /v1/chat handlers
  gateway.rs  ConversationChatClient, TelegramClient, MetricasClient (reqwest wrappers)
  telegram.rs TelegramLoop — long-poll + async job dispatch
  runtime.rs  Synchronous run_turn() fallback (used when AGENT_RUNTIME_URL is unset)
  session.rs  In-memory SessionStore (fallback only)
  hospital.rs HospitalClient + tool definitions
  llm.rs      LlmClient (OpenAI-compatible)
  sse.rs      SseHub for streaming web clients
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

---

## Async Telegram flow

When `AGENT_RUNTIME_URL` is set, incoming Telegram messages are dispatched
asynchronously via RabbitMQ instead of running `run_turn()` locally.

### Flow

```
Telegram message
  → get/create conversation-chat session (POST /api/v1/sessions via agent-runtime)
  → POST /api/v1/jobs  →  { job_id }
  → wait up to 5 s for GET /api/v1/jobs/:job_id/wait?timeout=5000
      ├─ fast path (result arrives in time): send LLM reply directly
      └─ slow path (timeout):
            1. send "Estamos trabajando para responder tu solicitud,
               por favor permanece en línea."
            2. tokio::spawn background task:
               GET /api/v1/jobs/:job_id/wait?timeout=120000
               → when resolved: send final LLM reply
```

### Key constants (`src/telegram.rs`)

| Constant | Value | Purpose |
|---|---|---|
| `FAST_REPLY_TIMEOUT_MS` | 5 000 ms | Window for immediate reply |
| `BACKGROUND_WAIT_TIMEOUT_MS` | 120 000 ms | Max time for background delivery |
| `WORKING_MESSAGE` | Spanish hardcoded | Sent on slow path |

### Session management

`TelegramLoop` maintains a `HashMap<chat_id, session_id>` where `session_id`
is a conversation-chat session (obtained via `POST /api/v1/sessions` on first
message, then reused). When `AGENT_RUNTIME_URL` is unset, the fallback path
uses the local in-memory `SessionStore` instead.

### No-duplicate guarantee

Each job is assigned a unique `job_id`. The agent-runtime in-memory job store
resolves each job exactly once. The background task and the immediate-reply
path are mutually exclusive — the background task only spawns on the slow path,
after the immediate reply window has already expired.

---

## Out of Scope

Session persistence, tool execution from inside chat-orch,
channel webhooks, webhook signature verification, auth middleware, rate
limiting, metrics, OpenTelemetry, retries, circuit breakers.
