# Chat Orchestrator

Thin HTTP front-door for a multi-tenant chatbot prototype. `chat-orch`
receives chat requests from the frontend, ensures a session exists in
`conversation-chat`, forwards the turn, and returns the response.

Session persistence, LLM calls, tool execution, and history all live in
`conversation-chat`. This service is intentionally a forwarder — nothing more.

- **Language:** Rust (edition 2021)
- **Framework:** Axum 0.7 + Tokio
- **License:** MIT

## Architecture

```
frontend ──► chat-orch ──► conversation-chat ──► (LLM, tools, Mongo)
```

Per request to `POST /v1/chat`:

1. If the request body has no `session_id`, call
   `POST {CONVERSATION_CHAT_URL}/api/v1/sessions` with `{tenant_id}` and take
   the `sid` from the response.
2. Call `POST {CONVERSATION_CHAT_URL}/api/v1/sessions/{sid}/turns` with
   `{message}`.
3. Return the downstream body verbatim, merged with the `session_id` used.

## Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Liveness probe. Returns `{"status":"ok"}`. |
| `/v1/chat` | POST | Forwards a chat turn to `conversation-chat`. |

Request body for `/v1/chat`:

```json
{ "tenant_id": "string", "session_id": "optional string", "message": "string" }
```

Error responses use `{"error": "..."}`. Malformed body → 400, downstream
unreachable → 502, anything else → 500.

## Quick Start

```bash
cp .env.example .env
# edit .env — at minimum set CONVERSATION_CHAT_URL, TENANT_SERVICE_URL, OPENAI_API_KEY
cargo run
```

The server binds to `0.0.0.0:3000` by default.

## Project Structure

```
src/
  main.rs     Server bootstrap, tracing, graceful shutdown on SIGTERM/SIGINT
  config.rs   AppConfig::from_env() (9 env vars) + unit tests
  error.rs    AppError + IntoResponse
  lib.rs      Module declarations + AppState
  routes.rs   /health and /v1/chat handlers
  gateway.rs  ConversationChatClient (reqwest wrapper)
```

## Configuration

See [`.env.example`](.env.example) for the full (9-variable) list. Required:
`CONVERSATION_CHAT_URL`, `TENANT_SERVICE_URL`, `OPENAI_API_KEY`.

## Testing

```bash
cargo test        # unit tests (config parsing)
cargo clippy      # lint
```

## Docker

```bash
docker build -t chat-orch .
docker run --rm -p 3000:3000 \
  -e CONVERSATION_CHAT_URL=http://host.docker.internal:8082 \
  -e TENANT_SERVICE_URL=http://host.docker.internal:8080 \
  -e OPENAI_API_KEY=sk-... \
  chat-orch
```

The Dockerfile uses `rust:1.88-slim` for the builder and
`debian:bookworm-slim` for the runtime. The binary runs as non-root UID 10001.

## Scope boundary

`chat-orch` does **not** do: session persistence, LLM calls, tool execution,
channel webhooks (Telegram/WhatsApp), webhook signature verification, auth
middleware, rate limiting, metrics, OpenTelemetry, retries, circuit breakers.
These either belong to `conversation-chat` or are out of scope for the
prototype. See [`REFACTOR_PROMPT.md`](REFACTOR_PROMPT.md) for the full spec.
