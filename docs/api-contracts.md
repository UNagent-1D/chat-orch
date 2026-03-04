# API Contracts — Chat Orchestrator (Exposed Endpoints)

*Version 1.0 | March 2026*

This document defines the REST API endpoints that the Chat Orchestrator exposes.
These are consumed by webhook providers (Telegram, WhatsApp) and internal clients
(web widget, admin tools).

---

## Authentication Model

All endpoints in the Chat Orchestrator use one of four authentication mechanisms:

| Endpoint | Auth Mechanism | Header / Method |
|----------|---------------|-----------------|
| `GET /health` | None | Kubernetes liveness probe |
| `GET /ready` | None | Kubernetes readiness probe |
| `GET /metrics/pipeline` | API Key | `X-Api-Key` header (constant-time comparison) |
| `POST /webhook/telegram/:slug` | Signature | `X-Telegram-Bot-Api-Secret-Token` header |
| `GET /webhook/whatsapp` | Verify Token | `hub.verify_token` query param |
| `POST /webhook/whatsapp` | HMAC-SHA256 | `X-Hub-Signature-256` header |
| `POST /conversation/entrypoint/open` | JWT | `Authorization: Bearer <token>` |
| `POST /conversation/chat/turn` | JWT | `Authorization: Bearer <token>` |

**Fail-closed behavior:** If `METRICS_API_KEY` is not configured, the
`/metrics/pipeline` endpoint returns 403 (not 200). This ensures forgetting
to set the env var never exposes internal metrics.

---

## 1. Webhook Endpoints (Channel Ingestion)

These endpoints receive messages from external chat platforms. They use
**channel-specific signature verification** (not JWT). They return fast (200/503)
and process messages asynchronously in the background.

### 1.1 Telegram Webhook

```
POST /webhook/telegram/:tenant_slug
```

**Headers:**
| Header | Required | Description |
|--------|----------|-------------|
| `X-Telegram-Bot-Api-Secret-Token` | Yes | Must match the secret set during `setWebhook` |
| `Content-Type` | Yes | `application/json` |

**Request Body:** Telegram `Update` object (see [Telegram Bot API](https://core.telegram.org/bots/api#update))

**Responses:**
| Status | Meaning |
|--------|---------|
| 200 OK | Message accepted for processing |
| 403 Forbidden | Signature verification failed |
| 503 Service Unavailable | Orchestrator overloaded — Telegram will retry |

**Notes:**
- `tenant_slug` in the URL path maps directly to the tenant (no Tenant Service lookup needed)
- The body is NOT parsed if the signature is invalid
- Duplicate `update_id` values are silently discarded (dedup)

### 1.2 WhatsApp Webhook — Verification (GET)

```
GET /webhook/whatsapp
```

**Query Parameters:**
| Param | Description |
|-------|-------------|
| `hub.mode` | Must be `subscribe` |
| `hub.verify_token` | Must match `WHATSAPP_VERIFY_TOKEN` env var |
| `hub.challenge` | Challenge string to echo back |

**Responses:**
| Status | Meaning |
|--------|---------|
| 200 OK | Body = `hub.challenge` value (plain text) |
| 403 Forbidden | `verify_token` does not match |

### 1.3 WhatsApp Webhook — Messages (POST)

```
POST /webhook/whatsapp
```

**Headers:**
| Header | Required | Description |
|--------|----------|-------------|
| `X-Hub-Signature-256` | Yes | `sha256=<HMAC-SHA256 of raw body using app secret>` |
| `Content-Type` | Yes | `application/json` |

**Request Body:** WhatsApp Cloud API webhook payload:
```json
{
  "object": "whatsapp_business_account",
  "entry": [{
    "id": "<WABA_ID>",
    "changes": [{
      "field": "messages",
      "value": {
        "metadata": {
          "display_phone_number": "+573001234567",
          "phone_number_id": "123456789"
        },
        "messages": [
          {
            "id": "wamid.xxx",
            "from": "573009876543",
            "timestamp": "1709571234",
            "type": "text",
            "text": { "body": "Hello" }
          }
        ],
        "statuses": []
      }
    }]
  }]
}
```

**Responses:**
| Status | Meaning |
|--------|---------|
| 200 OK | Webhook accepted |
| 403 Forbidden | HMAC signature invalid |
| 503 Service Unavailable | Orchestrator overloaded |

**Critical Implementation Notes:**
- `statuses[]` array contains delivery receipts — **NEVER route to LLM**
- `messages[]` can contain **multiple messages** — iterate all
- `entry[]` can contain **multiple entries** — iterate all
- Tenant is resolved via `phone_number_id` from `metadata` (calls `GET /internal/resolve-channel`)
- Raw body (`Bytes`) must be extracted BEFORE JSON deserialization for HMAC verification

---

## 2. REST Endpoints (Non-Webhook Clients)

These endpoints are consumed by internal services (web widget, admin tools).
They require **JWT authentication** and return **synchronous responses**.
They do NOT go through the semaphore pipeline — they await the result directly.

### 2.1 Open Conversation (Entrypoint)

```
POST /conversation/entrypoint/open
Authorization: Bearer <JWT>
Content-Type: application/json
```

**Request:**
```json
{
  "tenant_id": "550e8400-e29b-41d4-a716-446655440000",
  "channel": "web_widget",
  "channel_user_id": "user-session-abc123"
}
```

**Response (201 Created):**
```json
{
  "conversation_id": "660e8400-e29b-41d4-a716-446655440001",
  "session_token": "ses_abc123def456...",
  "config_refs": {
    "agent_profile_id": "770e8400-e29b-41d4-a716-446655440002",
    "agent_config_id": "880e8400-e29b-41d4-a716-446655440003",
    "config_version": 3
  }
}
```

**Errors:**
| Status | Meaning |
|--------|---------|
| 400 | Missing or invalid fields |
| 401 | Missing or invalid JWT |
| 403 | JWT role insufficient or wrong tenant scope |
| 404 | Tenant not found or inactive |

### 2.2 Chat Turn

```
POST /conversation/chat/turn
Authorization: Bearer <JWT>
Content-Type: application/json
```

**Request:**
```json
{
  "conversation_id": "660e8400-e29b-41d4-a716-446655440001",
  "session_token": "ses_abc123def456...",
  "tenant_id": "550e8400-e29b-41d4-a716-446655440000",
  "config_refs": {
    "agent_profile_id": "770e8400-e29b-41d4-a716-446655440002",
    "agent_config_id": "880e8400-e29b-41d4-a716-446655440003",
    "config_version": 3
  },
  "message": {
    "content_type": "text",
    "text": "I need to book an appointment with a cardiologist"
  }
}
```

**Response (200 OK):**
```json
{
  "reply": {
    "parts": [
      {
        "type": "text",
        "text": "I can help you schedule a cardiology appointment. Let me check available doctors."
      },
      {
        "type": "quick_replies",
        "prompt": "Which location works best for you?",
        "options": [
          { "label": "Bogota Norte", "value": "bogota-norte" },
          { "label": "Medellin Centro", "value": "medellin-centro" }
        ]
      }
    ]
  },
  "updated_session_token": null
}
```

**Errors:**
| Status | Meaning |
|--------|---------|
| 400 | Invalid message format |
| 401 | Missing or invalid JWT |
| 404 | Session not found or expired |
| 502 | Downstream service (LLM, Tenant, ACR) error |

---

## 3. Operational Endpoints

### 3.1 Health (Liveness)

```
GET /health
```

Always returns `200 OK` with body `"ok"` if the server is running.
No auth required.

### 3.2 Ready (Readiness)

```
GET /ready
```

Returns `200 OK` if the server is ready to accept traffic.
Checks: Redis connectivity, downstream service reachability.

**Response (200 OK):**
```json
{
  "status": "ok",
  "redis": "ok",
  "tenant_service": "ok",
  "acr_service": "ok"
}
```

**Response (503):** If any dependency is unreachable.

### 3.3 Pipeline Metrics

```
GET /metrics/pipeline
X-Api-Key: <METRICS_API_KEY>
```

Returns pipeline and cache metrics for monitoring/autoscaling.
**Requires `X-Api-Key` header** matching the `METRICS_API_KEY` env var.

**Response (200 OK):**
```json
{
  "pipeline_available_permits": 9850,
  "channel_cache_entries": 42,
  "config_cache_entries": 12
}
```

**Errors:**
| Status | Meaning |
|--------|---------|
| 401 | `X-Api-Key` header missing |
| 403 | Key mismatch or `METRICS_API_KEY` not configured |
