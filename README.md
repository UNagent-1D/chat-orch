# Chat Orchestrator

General Orchestrator for a multi-tenant conversational AI platform.
Acts as the universal ingestion point for chat messages from any channel
(Telegram, WhatsApp, future sources), normalizes them, resolves tenants,
manages sessions, executes LLM-powered conversation turns with tool calling,
and delivers responses back to the originating channel.

- **Language:** Rust (edition 2021, MSRV 1.75)
- **Framework:** Axum 0.7 + Tokio
- **Scale target:** 100k msg/sec (architecture-ready)
- **License:** MIT

## Architecture

```
Telegram ──► /webhook/telegram/:slug ─┐
                                      ├─► Normalize ─► Resolve Tenant ─► Session ─► LLM Turn Loop ─► Reply
WhatsApp ──► /webhook/whatsapp ───────┘                                              ▲
                                                                                     │
REST ──────► /conversation/chat/turn ────────────────────────────────────────────────►┘
```

Messages flow through this pipeline:

1. **Ingest** — Webhook arrives, signature verified, payload parsed, normalized to `IngestMessage`
2. **Pipeline** — Semaphore permit acquired (10K max), `tokio::spawn` background task
3. **Resolve** — Channel key mapped to tenant via moka cache (backed by Tenant Service)
4. **Session** — Redis session created or retrieved (composite key: tenant + channel + user)
5. **Conversation** — Agent config fetched, tool registry merged, LLM turn loop executed (with tool calls)
6. **Reply** — Response formatted for originating channel and sent via channel API

Webhook handlers return fast (200/503) — processing happens in the background.
REST handlers (`/conversation/chat/turn`) await the full response synchronously.

## Quick Start

### Prerequisites

- Rust 1.75+ (`rustup update`)
- Redis 7+ (or use Docker)
- A `.env` file (copy from `.env.example`)

### Local Development

```bash
# Start Redis (if not running)
docker compose up redis -d

# Copy env file and fill in values
cp .env.example .env

# Build and run
cargo run
```

The server starts on `http://0.0.0.0:3000` by default.

### Docker (full stack)

```bash
docker compose up --build
```

This starts the orchestrator, Redis, Caddy (TLS proxy), and Python mock services
for the Tenant Service, Agent Config Registry, and channel APIs.

## API Endpoints

| Endpoint | Method | Auth | Description |
|----------|--------|------|-------------|
| `/health` | GET | None | Liveness probe |
| `/ready` | GET | None | Readiness probe (checks Redis) |
| `/metrics/pipeline` | GET | API Key (`X-Api-Key`) | Pipeline metrics (semaphore, sessions) |
| `/webhook/telegram/:tenant_slug` | POST | `X-Telegram-Bot-Api-Secret-Token` | Telegram webhook ingestion |
| `/webhook/whatsapp` | GET | Verify token (query param) | WhatsApp webhook verification |
| `/webhook/whatsapp` | POST | HMAC-SHA256 (`X-Hub-Signature-256`) | WhatsApp webhook ingestion |
| `/conversation/entrypoint/open` | POST | JWT Bearer | Open a new conversation session |
| `/conversation/chat/turn` | POST | JWT Bearer | Execute a conversation turn (synchronous) |

### Interactive API Docs (Swagger)

Full OpenAPI 3.0.3 spec is at [`docs/openapi.yaml`](docs/openapi.yaml). To browse it interactively:

```bash
# Option A: Swagger UI via Docker
docker run -p 8080:8080 -e SWAGGER_JSON=/spec/openapi.yaml \
  -v ./docs:/spec swaggerapi/swagger-ui

# Option B: Paste into https://editor.swagger.io
```

## Project Structure

```
src/
  main.rs              Server bootstrap, tracing, graceful shutdown
  config.rs            AppConfig loaded from environment variables
  state.rs             AppState: caches, clients, semaphore, Redis pools
  router.rs            Axum router: webhook routes, REST routes, middleware
  error.rs             AppError enum with IntoResponse impl
  conversation.rs      Turn processing logic (session + config + LLM)

  auth/                JWT middleware, API key middleware
  types/               Domain types (IngestMessage, ResolvedMessage, Session, etc.)
  ingest/              Channel webhook handlers (Telegram, WhatsApp)
  gateway/             HTTP clients for downstream services + caches
  pipeline/            Concurrency: semaphore-bounded task spawning, sessions, dedup
  llm/                 LLM client trait, OpenAI impl, tool executor, turn loop

(planned) docker/           Containerization assets (Dockerfile, Caddyfile, k6 load test scripts) — not yet in this repo
(planned) mock-services/    Python mocks for downstream services — not yet in this repo
docs/                       API contracts, downstream contracts, escalation docs
(planned) tests/            Integration tests (wiremock-based) — not yet in this repo
```

## Testing

```bash
# Unit + integration tests
cargo test

# Benchmarks (criterion)
cargo bench

# Load tests (requires running instance + k6)
./scripts/load_test.sh
```

## Configuration

All configuration is via environment variables. See [`.env.example`](.env.example) for the
full list with descriptions.

Key variables:

| Variable | Required | Description |
|----------|----------|-------------|
| `REDIS_URL` | Yes | Redis connection URL |
| `TENANT_SERVICE_URL` | Yes | Go Tenant Service base URL |
| `ACR_SERVICE_URL` | Yes | Go Agent Config Registry base URL |
| `OPENAI_API_KEY` | Yes | OpenAI API key |
| `TELEGRAM_BOT_TOKEN` | For Telegram | Bot token from @BotFather |
| `WHATSAPP_ACCESS_TOKEN` | For WhatsApp | Meta App access token |
| `WHATSAPP_APP_SECRET` | For WhatsApp | App secret for signature verification |
| `JWT_SECRET` | For REST API | JWT signing secret |
| `METRICS_API_KEY` | No | API key for `/metrics/pipeline` (fail-closed if unset) |
| `WHATSAPP_STATIC_TENANT_MAP` | No | JSON array for static tenant resolution (MVP workaround) |

## Downstream Services

| Service | Language | Endpoints Called |
|---------|----------|-----------------|
| Tenant Service | Go + Gin | `GET /api/v1/tenants/:id`, `GET /internal/resolve-channel` |
| Agent Config Registry | Go + Gin | `GET /api/v1/tenants/:id/profiles/:pid/configs/active`, `GET /api/v1/tool-registry` |
| OpenAI API | -- | `POST /v1/chat/completions` |
| Telegram Bot API | -- | `POST /bot<TOKEN>/sendMessage` |
| WhatsApp Graph API | -- | `POST /v18.0/<PNID>/messages` |

## Key Design Decisions

- **TypeState pattern**: `IngestMessage` (tenant unknown) vs `ResolvedMessage` (tenant guaranteed at compile time)
- **Handler-per-channel**: Each channel has its own Axum handler — no shared trait until a 3rd channel arrives
- **4 independent moka caches**: Channel resolution (5min), agent config (2min), tool registry (5min), each with different TTLs and max entries
- **Redis for write-heavy state**: Sessions and dedup use Redis for cross-replica consistency
- **No media downloads**: Only `file_id` / URL references are stored
- **Constant-time auth**: API key comparison uses `subtle::ConstantTimeEq` to prevent timing attacks
- **Static tenant map**: Override-first resolution for WhatsApp tenants when the Go team's resolve-channel endpoint is unavailable
