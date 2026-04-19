# chat-orch refactor prompt (authoritative spec)

You are rewriting the `chat-orch` service (Rust + Axum 0.7 + Tokio) to be a thin,
performant HTTP forwarder. The existing code is a skeleton (main.rs prints
"starting", config.rs is a config struct for features that don't exist). Throw
away the old design docs — the scope below is authoritative.

## Project context
- Part of a university architecture prototype: multi-tenant chatbot for
  customer service. chat-orch is the "general orchestrator" front-door.
- Grader values: distributed design, clean separation of concerns,
  maintainability. Performance matters but 100 req/s is fine for the demo.
- Other services already exist: `tenant` (Go, :8080), `conversation-chat`
  (Go, :8082, owns sessions + LLM turns + tool calls + history), `hospital-mock`
  (Python, data source), `frontend` (React), `postgres`, `mongo`, `qdrant`.
- LLM access is via OpenRouter using the OpenAI-compatible API
  (`https://openrouter.ai/api/v1`). Default model:
  `nvidia/nemotron-3-super-120b-a12b:free`.

## Scope of chat-orch
chat-orch is a thin HTTP front-door. Its job: accept a chat request from the
frontend, ensure a session exists in conversation-chat, forward the turn, and
return the response. Nothing else.

### Endpoints
- `GET /health` → 200 `{status:"ok"}`
- `POST /v1/chat`
  - Request body: `{tenant_id: string, session_id?: string, message: string}`
  - Behavior:
    1. If `session_id` absent: `POST {CONVERSATION_CHAT_URL}/api/v1/sessions`
       with `{tenant_id}` → receive `{sid}`.
    2. `POST {CONVERSATION_CHAT_URL}/api/v1/sessions/{sid}/turns` with
       `{message}`.
    3. Return the conversation-chat response verbatim, plus the `session_id`
       if we opened one.
  - Errors map to 502 if conversation-chat is unreachable, 400 if the body
    is malformed, 500 for anything else. Respond with `{error: string}`.

### Configuration (env vars — THIS IS THE COMPLETE LIST)
- `SERVER_HOST` (default `0.0.0.0`)
- `SERVER_PORT` (default `3000`)
- `CONVERSATION_CHAT_URL` (required, e.g. `http://conversation-chat:8082`)
- `TENANT_SERVICE_URL` (required — reserved for future tenant lookups; don't
  call it in this iteration)
- `OPENAI_API_KEY` (required — fed by `OPENROUTER_API_KEY` in compose)
- `OPENAI_BASE_URL` (default `https://openrouter.ai/api/v1`)
- `OPENAI_DEFAULT_MODEL` (default `nvidia/nemotron-3-super-120b-a12b:free`)
- `RUST_LOG` (default `chat_orch=info,tower_http=info`)
- `LOG_FORMAT` (`json` | `pretty`, default `pretty`)

Anything not in this list — DO NOT ADD. No Redis, no JWT, no caches, no
telegram/whatsapp, no metrics API key, no semaphore tuning.

### File layout (target)
src/
  main.rs          # tokio runtime, tracing init, build router, bind & serve
  config.rs        # AppConfig::from_env(), one small struct
  error.rs         # AppError enum + IntoResponse + Fromreqwest::Error
  lib.rs           # pub mod config; pub mod error; pub mod routes; pub mod gateway;
  routes.rs        # health + chat_forward Axum handlers
  gateway.rs       # ConversationChatClient (reqwest::Client wrapper)

### Quality bar
- One binary, one crate. No sub-crates.
- Use `axum::extract::State` for shared `reqwest::Client` + config.
- Tracing via `tracing_subscriber`, respects `LOG_FORMAT`.
- Graceful shutdown on SIGTERM/SIGINT (tokio::signal).
- `serde` derive for request/response types — no manual JSON parsing.
- `thiserror::Error` for `AppError`. No `anyhow` outside `main.rs`.
- Tests: one unit test for config parsing (good + missing-var case). That's it.
- No `unwrap()` outside tests; use `?` + typed errors.

### Dockerfile
Base image `rust:1.88-slim` (NOT 1.82 — transitive deps need 1.88+).
Multi-stage: builder compiles `--release --bin chat-orch`, runtime is
`debian:bookworm-slim` with `ca-certificates`. Run as non-root UID 10001.
Expose 3000.

### Cargo.toml
Keep minimal: axum 0.7, tokio (full), tower-http (trace), reqwest (json,
rustls-tls), serde + serde_json, thiserror, anyhow, tracing,
tracing-subscriber (env-filter, json), dotenvy. Drop: redis, moka, hmac,
sha2, subtle, jsonwebtoken, uuid, chrono, async-trait, futures, hex. Drop
the [[bench]] section and the `benches/` dependency on criterion.

### Out of scope (do not implement)
Session persistence, tool execution, LLM calls from inside chat-orch,
channel webhooks, webhook signature verification, auth middleware, rate
limiting, metrics, OpenTelemetry, retries/circuit breakers. These either
belong to conversation-chat already, or are future work.

### Definition of done
- `cargo check` passes clean (no warnings).
- `cargo run` starts and serves `/health`.
- `curl -X POST localhost:3000/v1/chat -d '{"tenant_id":"t1","message":"hi"}'`
  reaches a running conversation-chat and returns its response body.
- `docker build` succeeds; image <100MB runtime layer.
- README and .env.example match the new surface area, and nothing more.
