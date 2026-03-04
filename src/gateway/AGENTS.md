# AGENTS.md — gateway/

## Purpose

HTTP clients for downstream services and read-only caches for high-frequency
lookups. Also contains the reply sender that dispatches responses back to
the originating channel.

## Downstream Services

| Client | Service | Key Endpoint |
|--------|---------|-------------|
| `tenant_client.rs` | Tenant Service (Go) | `GET /internal/resolve-channel`, `GET /api/v1/tenants/:id` |
| `acr_client.rs` | Agent Config Registry (Go) | `GET /api/v1/tenants/:id/profiles/:pid/configs/active`, `GET /api/v1/tool-registry` |

## Caches (moka — in-memory, read-only)

| Cache | Key | TTL | Max Entries | Purpose |
|-------|-----|-----|-------------|---------|
| `channel_cache.rs` | `{type}:{channel_key}` | 5 min | 100K | Channel -> Tenant resolution (with static overrides) |
| `config_cache.rs` | `{tenant_id}:{profile_id}` | 2 min | 50K | Agent config (LLM params, tools) |
| `tool_registry_cache.rs` | `()` (global singleton) | 5 min | 1 | Global tool registry (OpenAI function defs) |

All caches use `moka::future::Cache::try_get_with()` for thundering-herd
protection — only one inflight request per cache miss key.

### Channel Cache — Static Overrides

The channel cache supports an optional static tenant map injected at
construction time via the `WHATSAPP_STATIC_TENANT_MAP` env var. This is
an **MVP workaround** for WhatsApp tenant resolution while the Go team
has not yet implemented `GET /internal/resolve-channel`.

Static overrides are checked BEFORE the HTTP call (they are explicit
overrides, not network-error fallbacks). This avoids split-brain issues.

### Tool Registry Cache — Design Note

`tool_registry_cache.rs` uses `key=()` (global singleton) because the tool
registry is a single global catalog, not per-tenant. Uses moka for pattern
consistency with the other caches. On fetch failure, returns empty vec (graceful
degradation to constraints-only tool definitions).

## Reply Sender

`reply_sender.rs` dispatches `AgentResponse` back to the originating channel:
- Telegram: `POST /bot<TOKEN>/sendMessage` (and sendPhoto, sendDocument, etc.)
- WhatsApp: `POST /v18.0/<PNID>/messages` (Graph API)

The sender maps `ResponsePart` variants to channel-native message formats
(e.g., WhatsApp interactive messages, Telegram inline keyboards).

## Conventions

- All HTTP clients share a single `reqwest::Client` with connection pooling
  (`pool_max_idle_per_host = 2000`)
- Timeouts: 10s for Tenant/ACR, 30s for LLM, 15s for channel APIs
- All errors are mapped to `AppError` variants
- Tool registry response is limited to 1MB to prevent OOM
