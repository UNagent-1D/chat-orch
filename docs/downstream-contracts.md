# Downstream Service Contracts — Services We Call

*Version 1.0 | March 2026*

This document defines the exact HTTP contracts the Chat Orchestrator uses to
communicate with downstream services. These contracts are the source of truth
for both the gateway client implementations and mock services.

---

## 1. Tenant Service (Go + Gin)

Base URL: `TENANT_SERVICE_URL` env var (e.g., `http://localhost:3001`)

### 1.1 Resolve Channel (Internal) — CROSS-TEAM DEPENDENCY

> **STATUS: 🔴 Not yet implemented by Go team.**
> This endpoint is required for WhatsApp tenant resolution.
> See the message sent to the Go team for details.

```
GET /internal/resolve-channel?channel_type=whatsapp&channel_key=<phone_number_id>
```

**Response (200 OK):**
```json
{
  "tenant_id": "550e8400-e29b-41d4-a716-446655440000",
  "tenant_slug": "hospital-san-ignacio",
  "agent_profile_id": "770e8400-e29b-41d4-a716-446655440002",
  "webhook_secret_ref": "vault-key-whatsapp-secret",
  "is_active": true
}
```

**Response (404 Not Found):**
```json
{ "error": "channel not found" }
```

**Cache strategy:** moka, TTL 5 min, max 100K entries, `try_get_with` for thundering herd.

### 1.2 Get Tenant Detail

```
GET /api/v1/tenants/:id
Authorization: Bearer <JWT>
```

**Response (200 OK):**
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "slug": "hospital-san-ignacio",
  "name": "Hospital San Ignacio",
  "plan": "pro",
  "status": "active",
  "branding_logo_url": "https://cdn.example.com/logo.png",
  "branding_primary_color": "#2E75B6"
}
```

### 1.3 Get Tenant Channels

```
GET /api/v1/tenants/:id/channels
Authorization: Bearer <JWT>
```

**Response (200 OK):**
```json
[
  {
    "id": "channel-uuid-1",
    "tenant_id": "550e8400-e29b-41d4-a716-446655440000",
    "channel_type": "whatsapp",
    "channel_key": "123456789012345",
    "webhook_secret_ref": "vault-key-whatsapp-secret",
    "is_active": true
  }
]
```

### 1.4 Get Agent Profiles

```
GET /api/v1/tenants/:id/profiles
Authorization: Bearer <JWT>
```

**Response (200 OK):**
```json
[
  {
    "id": "770e8400-e29b-41d4-a716-446655440002",
    "name": "Scheduling Bot",
    "description": "Handles appointment scheduling for patients",
    "scheduling_flow_rules": { "steps": ["greet", "collect_specialty", "find_doctor", "book"] },
    "escalation_rules": { "triggers": ["frustrated_3x", "explicit_request"] },
    "allowed_specialties": ["cardiology", "pediatrics", "general"],
    "allowed_locations": ["bogota-norte", "medellin-centro"],
    "agent_config_id": "880e8400-e29b-41d4-a716-446655440003"
  }
]
```

### 1.5 Get Data Sources

```
GET /api/v1/tenants/:id/data-sources
Authorization: Bearer <JWT>
```

**Response (200 OK):**
```json
[
  {
    "id": "ds-uuid-1",
    "name": "Hospital Mock API",
    "source_type": "scheduling",
    "base_url": "https://mock-hospital-api.internal",
    "credential_ref": "vault-key-hospital-api",
    "route_configs": {
      "list_doctors": { "method": "GET", "path": "/doctors" },
      "get_doctor_schedule": { "method": "GET", "path": "/doctors/{id}/schedule" },
      "book_appointment": { "method": "POST", "path": "/appointments" },
      "reschedule_appointment": { "method": "PATCH", "path": "/appointments/{id}" },
      "cancel_appointment": { "method": "DELETE", "path": "/appointments/{id}" }
    },
    "is_active": true
  }
]
```

### 1.6 Bulk Channel Export (Nice-to-have)

> **STATUS: 🟡 Requested but not blocking.**
> Used for cache warm-up on cold start.

```
GET /internal/channels/all?active=true
```

**Response (200 OK):**
```json
[
  {
    "channel_type": "whatsapp",
    "channel_key": "123456789012345",
    "tenant_id": "550e8400-e29b-41d4-a716-446655440000",
    "agent_profile_id": "770e8400-e29b-41d4-a716-446655440002"
  }
]
```

---

## 2. Agent Config Registry (Go + Gin)

Base URL: `ACR_SERVICE_URL` env var (e.g., `http://localhost:3002`)

### 2.1 Get Active Config

```
GET /api/v1/tenants/:tenant_id/profiles/:profile_id/configs/active
Authorization: Bearer <JWT>
```

**Response (200 OK):**
```json
{
  "id": "880e8400-e29b-41d4-a716-446655440003",
  "agent_profile_id": "770e8400-e29b-41d4-a716-446655440002",
  "version": 3,
  "status": "active",
  "conversation_policy": {
    "steps": [
      "greet_patient",
      "identify_need",
      "collect_specialty",
      "find_available_doctors",
      "present_options",
      "confirm_booking",
      "send_confirmation"
    ]
  },
  "escalation_rules": {
    "triggers": [
      { "condition": "user_frustrated_count >= 3", "action": "handoff_to_human" },
      { "condition": "explicit_request_for_human", "action": "handoff_to_human" },
      { "condition": "medical_emergency_detected", "action": "show_emergency_numbers" }
    ]
  },
  "tool_permissions": [
    { "tool_name": "list_doctors", "constraints": {} },
    { "tool_name": "get_doctor_schedule", "constraints": {} },
    { "tool_name": "book_appointment", "constraints": { "max_horizon_days": 90 } },
    { "tool_name": "reschedule_appointment", "constraints": {} },
    { "tool_name": "cancel_appointment", "constraints": {} }
  ],
  "llm_params": {
    "model": "gpt-4o",
    "temperature": 0.3,
    "max_tokens": 1024,
    "system_prompt": "You are a scheduling assistant for Hospital San Ignacio..."
  },
  "channel_format_rules": {
    "whatsapp": { "max_chars": 1600 },
    "telegram": { "max_chars": 4096 },
    "web_widget": { "max_chars": 8000 }
  },
  "created_at": "2026-03-01T10:00:00Z",
  "activated_at": "2026-03-02T14:30:00Z"
}
```

**Cache strategy:** moka, TTL 2 min, max 50K entries, key = `(tenant_id, profile_id)`.

---

## 3. OpenAI API

Base URL: `OPENAI_BASE_URL` env var (default: `https://api.openai.com/v1`)

### 3.1 Chat Completions

```
POST /chat/completions
Authorization: Bearer <OPENAI_API_KEY>
Content-Type: application/json
```

**Request:**
```json
{
  "model": "gpt-4o",
  "temperature": 0.3,
  "max_tokens": 1024,
  "messages": [
    { "role": "system", "content": "You are a scheduling assistant..." },
    { "role": "user", "content": "I need a cardiologist appointment" }
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "list_doctors",
        "description": "List available doctors",
        "parameters": { "type": "object", "properties": {} }
      }
    }
  ]
}
```

**Response:** Standard OpenAI chat completion response with `choices[0].message`
containing either `content` (text response) or `tool_calls` (function calls).

---

## 4. Channel APIs (Outbound Replies)

### 4.1 Telegram Bot API

```
POST https://api.telegram.org/bot<TELEGRAM_BOT_TOKEN>/sendMessage
Content-Type: application/json
```

**Request:**
```json
{
  "chat_id": 12345678,
  "text": "I can help you schedule an appointment...",
  "parse_mode": "Markdown"
}
```

### 4.2 WhatsApp Cloud API (Graph API)

```
POST https://graph.facebook.com/<WHATSAPP_API_VERSION>/<PHONE_NUMBER_ID>/messages
Authorization: Bearer <WHATSAPP_ACCESS_TOKEN>
Content-Type: application/json
```

**Request (text):**
```json
{
  "messaging_product": "whatsapp",
  "to": "573009876543",
  "type": "text",
  "text": { "body": "I can help you schedule an appointment..." }
}
```

**Request (interactive — list picker):**
```json
{
  "messaging_product": "whatsapp",
  "to": "573009876543",
  "type": "interactive",
  "interactive": {
    "type": "list",
    "header": { "type": "text", "text": "Available Doctors" },
    "body": { "text": "Please select a doctor:" },
    "action": {
      "button": "View Doctors",
      "sections": [
        {
          "title": "Cardiology",
          "rows": [
            { "id": "doc-1", "title": "Dr. Garcia", "description": "Mon-Fri 9am-5pm" }
          ]
        }
      ]
    }
  }
}
```
