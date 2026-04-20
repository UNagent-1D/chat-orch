# Chat Orchestrator (`chat-orch`)

General orchestrator for a multi-tenant hospital chatbot prototype. `chat-orch`
is the HTTP front door: it terminates chat traffic from the frontend and from
Telegram, runs the LLM tool-calling loop against `hospital-mock`, keeps
per-session conversation history in memory, streams assistant replies over
Server-Sent Events, and emits KPI events to the `metricas` service.

- **Language:** Rust (edition 2021, MSRV 1.75 — build image pins 1.88)
- **Framework:** Axum 0.7 + Tokio
- **LLM transport:** OpenAI-compatible Chat Completions (default: OpenRouter)
- **License:** MIT

## Architecture

```
  ┌──────────┐         ┌──────────────┐        ┌───────────────┐
  │ frontend │ ──────► │              │ ─────► │  OpenRouter   │
  └──────────┘  HTTP   │              │  LLM   │  (chat.comp.) │
                       │              │        └───────────────┘
  ┌──────────┐  long   │  chat-orch   │        ┌───────────────┐
  │ Telegram │ ──────► │              │ ─────► │ hospital-mock │
  └──────────┘  poll   │              │ tools  └───────────────┘
                       │              │
                       │              │ fire&  ┌───────────────┐
                       │              │ forget │   metricas    │
                       └──────────────┘ ─────► └───────────────┘
                              │
                              │ SSE
                              ▼
                         browser tab
```

Per `POST /v1/chat`:

1. Accept `{tenant_id, session_id?, message}`.
2. Mint a `session_id` if absent.
3. Append the user turn to the in-memory session history.
4. Call the LLM with the full history + OpenAI-style tool schemas.
5. If the LLM returns `tool_calls`, dispatch each to `hospital-mock`,
   append the tool results, and loop (up to `MAX_TOOL_ROUNDS = 5`).
6. When the LLM returns plain content, publish it on the session's SSE
   channel and return it to the caller.
7. Emit turn + resolution events to `metricas` (fire-and-forget).

## HTTP Surface

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Liveness probe. `{"status":"ok"}`. |
| `/v1/chat` | POST | Runs one chat turn. Body: `{tenant_id, session_id?, message}`. |
| `/v1/chat/stream` | GET | SSE channel for a session. Query: `?session_id=...`. |
| `/v1/feedback` | POST | CSAT feedback. Body: `{tenant_id, session_id?, score (1-5)}`. |

Error responses are always `{"error": "..."}`:

- `400` — malformed body, missing `tenant_id`/`message`, score out of range.
- `502` — downstream (LLM, hospital-mock, Telegram, metricas) failure.
- `500` — anything else.

### `POST /v1/chat`

Request:

```json
{ "tenant_id": "demo-tenant", "session_id": "sess-...", "message": "Quiero una cita con cardiología" }
```

Response:

```json
{ "session_id": "sess-...", "message": { "text": "Claro, ¿tienes tu patient_ref?..." } }
```

If `session_id` is omitted, the server mints one (`sess-<uuid v4>`) and
returns it. Clients must echo it on subsequent turns to keep history.

### `GET /v1/chat/stream?session_id=...`

Server-Sent Events stream for the given session. Each event's `data` payload
is `{"kind":"assistant","text":"..."}`. A keep-alive `ping` is sent every 15s.
Events are only delivered to subscribers that connect **before** the turn
completes; there is no replay buffer.

### `POST /v1/feedback`

Fire-and-forget CSAT capture. Forwarded to `metricas` as
`POST {METRICAS_URL}/feedback/csat` with header `X-Tenant-ID`.

## Quick Start

```bash
cp .env.example .env
# Set at minimum: CONVERSATION_CHAT_URL, TENANT_SERVICE_URL, OPENAI_API_KEY,
# HOSPITAL_MOCK_URL
cargo run
```

The server binds `0.0.0.0:3000` by default.

```bash
curl -s http://localhost:3000/health
# {"status":"ok"}

curl -s -X POST http://localhost:3000/v1/chat \
  -H 'content-type: application/json' \
  -d '{"tenant_id":"demo-tenant","message":"Hola, ¿qué cardiólogos tienen?"}'
```

## Project Structure

```
src/
  main.rs      Bootstrap: env, tracing, reqwest client, clients, AppState,
               Telegram loop, bind, graceful shutdown (SIGTERM/SIGINT).
  lib.rs       Module declarations + AppState.
  config.rs    AppConfig::from_env() and unit tests.
  error.rs     AppError enum + IntoResponse + From<reqwest::Error>.
  routes.rs    Router + handlers (/health, /v1/chat, /v1/feedback).
  sse.rs       SseHub (broadcast channels per session) + /v1/chat/stream.
  session.rs   In-memory per-session ChatMessage history (bounded).
  runtime.rs   run_turn: system prompt + LLM loop + tool dispatch.
  llm.rs       LlmClient: OpenAI-compatible chat.completions wrapper.
  hospital.rs  HospitalClient + OpenAI-style tool_definitions().
  telegram.rs  TelegramLoop: long-poll getUpdates, runs turns, sendMessage.
  gateway.rs   TelegramClient, MetricasClient, (legacy) ConversationChatClient.
```

## Configuration

See [`.env.example`](.env.example). All variables:

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `SERVER_HOST` | | `0.0.0.0` | Bind host. |
| `SERVER_PORT` | | `3000` | Bind port. |
| `CONVERSATION_CHAT_URL` | yes | — | Reserved; read for legacy gateway. |
| `TENANT_SERVICE_URL` | yes | — | Reserved for future tenant lookups. |
| `HOSPITAL_MOCK_URL` | | `http://hospital-mock:8080` | Tool execution target. |
| `OPENAI_API_KEY` | yes | — | Bearer token for chat.completions. |
| `OPENAI_BASE_URL` | | `https://openrouter.ai/api/v1` | LLM endpoint. |
| `OPENAI_DEFAULT_MODEL` | | `nvidia/nemotron-3-super-120b-a12b:free` | Model id. |
| `METRICAS_URL` | | unset | If unset, KPI emission is disabled. |
| `TELEGRAM_BOT_TOKEN` | | unset | Enables the Telegram long-poll loop. |
| `TELEGRAM_DEFAULT_TENANT_ID` | | unset | Required together with the token. |
| `CORS_ALLOW_ORIGIN` | | `http://localhost:3000` | Single origin; falls back to `Any` if unparseable. |
| `RUST_LOG` | | `chat_orch=info,tower_http=info` | `tracing_subscriber` filter. |
| `LOG_FORMAT` | | `pretty` | `pretty` or `json`. |

The Telegram loop only starts if both `TELEGRAM_BOT_TOKEN` and
`TELEGRAM_DEFAULT_TENANT_ID` are set.

## Testing

```bash
cargo test             # config parsing happy path + missing-var error
cargo clippy -- -D warnings
cargo build --release
```

End-to-end smoke test: `docker compose up` the full stack, then send a
`POST /v1/chat` and watch the tracing logs for LLM and hospital-mock calls.

## Docker

```bash
docker build -t chat-orch .
docker run --rm -p 3000:3000 \
  -e CONVERSATION_CHAT_URL=http://host.docker.internal:8082 \
  -e TENANT_SERVICE_URL=http://host.docker.internal:8080 \
  -e HOSPITAL_MOCK_URL=http://host.docker.internal:8081 \
  -e OPENAI_API_KEY=sk-or-... \
  chat-orch
```

Multi-stage: `rust:1.88-slim` builder → `debian:bookworm-slim` runtime with
`ca-certificates`. The binary runs as non-root UID 10001 on port 3000.

## Further reading

- [`TECHNICAL.md`](TECHNICAL.md) — design notes, data flow, trade-offs.
- [`AGENTS.md`](AGENTS.md) — short brief for agents working on this repo.
- [`REFACTOR_PROMPT.md`](REFACTOR_PROMPT.md) — historic scope note; partially
  superseded by the current implementation (see TECHNICAL.md §"Scope drift").
