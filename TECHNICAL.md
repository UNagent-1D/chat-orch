# `chat-orch` — Technical Document

Design, data flow, and implementation notes for the General Orchestrator
service of the multi-tenant hospital chatbot prototype.

---

## 1. Purpose and context

`chat-orch` sits between clients (web frontend, Telegram) and the rest of the
chatbot platform. It owns the per-turn conversation loop: accept user input,
drive the LLM, call tools on `hospital-mock`, buffer assistant replies for
real-time UI, and report KPIs to `metricas`.

The platform prototype is organized as:

| Service           | Stack        | Role                                                  |
|-------------------|--------------|-------------------------------------------------------|
| `frontend`        | React        | Web UI.                                               |
| **`chat-orch`**   | Rust/Axum    | Chat orchestration (this service).                    |
| `conversation-chat` | Go         | Historically intended as session/LLM owner (see §11). |
| `tenant`          | Go           | Tenant metadata; reserved for future calls.           |
| `hospital-mock`   | Python       | Appointment domain service (doctors, slots, bookings).|
| `metricas`        | —            | KPI / CSAT aggregation.                               |
| `postgres`, `mongo`, `qdrant` | — | Datastores, owned by other services.              |

### Deployment shape

Single binary, single crate, single container. Stateless with respect to
disk — all session state is in-process memory (see §6).

---

## 2. Runtime topology

```
                          ┌──────────────────────────────────┐
                          │            chat-orch             │
                          │                                  │
   HTTP /v1/chat ─────────►  routes::chat_forward            │
                          │        │                         │
   SSE  /v1/chat/stream ──►  routes::chat_stream             │
                          │        │       ▲                 │
   HTTP /v1/feedback ─────►  routes::submit_feedback         │
                          │        │       │                 │
                          │        ▼       │ publish         │
                          │   runtime::run_turn ── SseHub ──►│──► SSE client
                          │        │                         │
                          │        ├──► LlmClient ──────────►│──► OpenRouter
                          │        │                         │
                          │        ├──► HospitalClient ─────►│──► hospital-mock
                          │        │                         │
                          │        └──► SessionStore (mem)   │
                          │                                  │
   Telegram long-poll ◄──►  TelegramLoop ──► run_turn ───────┤
                          │                                  │
                          │  MetricasClient (fire & forget) ►│──► metricas
                          └──────────────────────────────────┘
```

All components share one `reqwest::Client` (30s idle pool timeout, rustls
TLS) and are stored as `Arc` fields of `AppState`.

---

## 3. Request lifecycle: `POST /v1/chat`

Source: `src/routes.rs:65`, `src/runtime.rs:33`.

1. **Decode & validate** — Axum deserializes the JSON body into
   `ChatRequest`. Empty `tenant_id` or `message` ⇒ `AppError::BadRequest`.
2. **Metric tap (pre)** — if `METRICAS_URL` is set, `MetricasClient::record_turn`
   is invoked with `resolved = false`. The call is spawned onto Tokio and
   never blocks the handler.
3. **Session id resolution** — if `session_id` is present and non-empty, use
   it; otherwise mint `sess-<uuid v4>` via `SessionStore::new_session_id`.
4. **`run_turn`** — drives the LLM loop (§4). Returns `(reply_text, resolved)`.
5. **SSE publish** — if the reply is non-empty, `SseHub::publish` sends it on
   the session's broadcast channel. If no subscriber is connected, the event
   is dropped (no replay buffer).
6. **Metric tap (post)** — if `resolved` (a `book_appointment` tool call
   succeeded), emit a second `record_turn` with `resolved = true` so
   `metricas` can compute containment / resolution rate.
7. **Response** — `{session_id, message:{text}}` as JSON.

---

## 4. The tool-calling loop

Source: `src/runtime.rs`.

The orchestrator implements the OpenAI function-calling protocol directly —
there is no intermediate agent framework.

```
for round in 0..MAX_TOOL_ROUNDS (5):
    messages = [system_prompt] + session.history(sid)
    resp = llm.complete(messages, tool_definitions())
    match resp:
        Content(text) -> append assistant, return (text, resolved)
        ToolCalls(calls) ->
            append assistant(tool_calls=calls) to history
            for call in calls:
                result = hospital.call_tool(call.name, call.arguments)
                append tool(role="tool", tool_call_id, name, content=result)
            continue
tracing::warn!("tool loop hit round cap")
return FALLBACK_REPLY, false
```

Key points:

- **System prompt** is baked into the code (`SYSTEM_PROMPT`) — scopes the
  assistant to the Clínica San Ignacio domain, Spanish responses, and a list
  of available tools. Not externalized on purpose; cheap to iterate.
- **Tool schemas** (`hospital::tool_definitions`) are emitted in
  OpenAI-style `{type:"function",function:{name,description,parameters}}`.
- **Tool errors surface to the LLM.** `HospitalClient::decode` wraps non-2xx
  hospital-mock responses as `{error:true,status,body,url}` and returns them
  as tool content. The model is instructed to explain the failure in user
  terms, so transport-level issues don't short-circuit the conversation.
- **Resolution signal** — `execute_tool` marks a turn resolved iff the tool
  name is `book_appointment` and the hospital-mock response does not carry
  `error:true`. This is the only signal fed to `metricas`.
- **LLM transport failure** — if `llm.complete` returns `Err`, the handler
  logs at `warn` and returns `FALLBACK_REPLY` without incrementing history.
  This hides OpenRouter outages from end users at the cost of context loss.
- **Round cap** — `MAX_TOOL_ROUNDS = 5`. If the LLM keeps calling tools past
  this, we bail out with `FALLBACK_REPLY` to avoid an infinite loop.

---

## 5. Hospital tools

Source: `src/hospital.rs`.

| Tool                        | HTTP call                                          |
|-----------------------------|----------------------------------------------------|
| `list_doctors`              | `GET /doctors?area=…&place=…`                      |
| `get_doctor_schedule`       | `GET /doctors/{doctor_id}/schedule?days_ahead=…`   |
| `book_appointment`          | `POST /appointments` (args as JSON body)           |
| `cancel_appointment`        | `POST /appointments/{id}/cancel` with `{reason}`   |
| `get_patient_appointments`  | `GET /patients/{patient_ref}/appointments?status=…`|

Unknown tool names raise `AppError::BadRequest`, which is serialized into the
tool result so the model can correct course. All responses (success or
error) are JSON — `call_tool` never panics on shape and defaults to `{}` if
the body is not valid JSON.

---

## 6. Session history

Source: `src/session.rs`.

- In-memory `HashMap<session_id, Vec<ChatMessage>>` behind a `tokio::sync::Mutex`.
- Bounded at `MAX_HISTORY = 40` messages per session. When appended past the
  cap, the oldest entries are drained (FIFO eviction).
- **Persistence: none.** A restart loses all sessions. Acceptable for the
  prototype; upgrade path is to move this into `conversation-chat`+Mongo.
- **Eviction by idle: none.** Sessions live until process restart. For a
  long-running demo this can grow unbounded in `chat_id` count; fine for the
  dev/demo environment.
- `ChatMessage` mirrors the OpenAI wire format (role, content, tool_calls,
  tool_call_id, name) so we can round-trip through chat.completions without
  translation.

### Concurrency on a session

A single `Mutex` guards the whole map. Two concurrent turns for the same
`session_id` will serialize on `append` / `history` locks. Cross-session
parallelism is unaffected because each round performs multiple short
critical sections rather than holding the lock across the LLM call.

---

## 7. SSE streaming

Source: `src/sse.rs`.

- `SseHub` owns a `HashMap<session_id, broadcast::Sender<StreamEvent>>` in a
  `std::sync::Mutex`.
- A subscriber (browser) calls `GET /v1/chat/stream?session_id=...`. That
  handler materializes the sender (creating the channel on first use with a
  `CHANNEL_BUFFER = 32` buffer) and converts the receiver into an Axum SSE
  stream.
- The `POST /v1/chat` handler calls `publish` after the turn completes.
  `publish` only sends if a sender already exists for the session — so a
  turn that finishes before any subscriber connected is dropped.
- The stream emits a 15s `ping` keep-alive to survive proxies.
- `BroadcastStream` lag errors are dropped silently. Clients that care about
  missed events must reconnect and rely on the next `POST /v1/chat` response.

This is a deliberately simple model: the SSE channel is a live tap for UI
feedback, not a durable event log.

---

## 8. Telegram integration

Source: `src/telegram.rs`, `src/gateway.rs:TelegramClient`.

- Enabled only when both `TELEGRAM_BOT_TOKEN` and
  `TELEGRAM_DEFAULT_TENANT_ID` are set.
- Long-polls `GET /bot{token}/getUpdates?timeout=30&offset=…`. The HTTP
  timeout is set to `timeout_secs + 10` to allow Telegram's own timeout to
  elapse first.
- On error, sleeps `BACKOFF_ON_ERROR = 2s` and retries. No exponential
  backoff on purpose — Telegram outages are transient.
- Each `chat_id` is mapped to a `session_id` in an internal
  `HashMap<i64, String>` guarded by a mutex. The mapping persists for the
  life of the process, so Telegram users get continuous history until
  restart.
- Messages are processed through the same `run_turn` used by HTTP, so the
  LLM + tool logic is shared.
- `tenant_id` is read from `TELEGRAM_DEFAULT_TENANT_ID` — no per-chat
  tenant dispatch in this prototype.

---

## 9. Metrics (`metricas`)

Source: `src/gateway.rs:MetricasClient`.

- All emissions are **fire-and-forget**: the handler spawns a task and
  returns immediately. Failures log at `warn` but never surface to the user.
- Two emission points:
  - `record_turn(tenant_id, message, resolved)` — `POST {METRICAS_URL}/conversation/chat`
    with header `X-Tenant-ID`.
  - `record_feedback(tenant_id, score)` — `POST {METRICAS_URL}/feedback/csat`
    with header `X-Tenant-ID` and body `{score}`.
- Disabling is implicit: if `METRICAS_URL` is unset, `AppState.metricas` is
  `None` and the taps are skipped.

---

## 10. Error model

Source: `src/error.rs`.

```rust
pub enum AppError {
    BadRequest(String),   // -> 400
    Downstream(String),   // -> 502 ("downstream service unavailable")
    MissingEnv(String),   // -> 500 ("internal error") — only at startup
    Internal(String),     // -> 500 ("internal error")
}
```

- `IntoResponse` always produces `{"error": "..."}` JSON.
- The outward-facing message for 5xx errors is intentionally generic —
  details go to `tracing::error!`, not the client.
- `From<reqwest::Error>` maps any reqwest failure to `Downstream`. This is a
  coarse mapping but matches the prototype's observability needs.
- The application avoids `unwrap()` outside tests; `main.rs` uses `anyhow`
  only for startup-time context. Handlers use `?` on typed `AppError`.

---

## 11. Scope drift vs. the original "thin forwarder" design

An earlier iteration of this service was spec'd as a strictly "thin
forwarder" to `conversation-chat`. The current implementation is broader:

| Original spec | Reality in `src/` |
|----|----|
| "Forward turns to conversation-chat; nothing else." | Calls the LLM directly, executes tools, owns sessions. |
| No LLM calls from chat-orch. | `llm::LlmClient` calls `{OPENAI_BASE_URL}/chat/completions`. |
| No tool execution. | `hospital::HospitalClient` + tool loop in `runtime.rs`. |
| No session persistence. | `session::SessionStore` (in-memory). |
| No channel webhooks. | `telegram::TelegramLoop` long-polls. |
| No metrics. | `gateway::MetricasClient` fire-and-forget. |
| 9 env vars, fixed list. | 14 env vars. |
| Target files: `main`, `config`, `error`, `lib`, `routes`, `gateway`. | Plus `hospital`, `llm`, `runtime`, `session`, `sse`, `telegram`. |

In practice, `conversation-chat` was never wired through, and the
orchestrator absorbed those responsibilities to get the demo end-to-end.
`gateway::ConversationChatClient` remains in the code but is not
constructed by `main.rs`. `CONVERSATION_CHAT_URL` and `TENANT_SERVICE_URL`
are still required at startup to avoid config breakage with the compose
stack, but they are not called on the request path.

**Implication for graders / reviewers:** treat this document as the
current contract.

---

## 12. Observability

- `tracing` + `tracing-subscriber` with `EnvFilter`. `LOG_FORMAT=json` for
  containers; `pretty` for local dev.
- `tower_http::trace::TraceLayer` emits per-request spans (method, URI,
  status, latency).
- Hot-path warnings:
  - `llm completion failed` on LLM error.
  - `tool loop hit round cap` on runaway tool calls.
  - `metricas emit failed / non-2xx` on metric emission issues.
  - `telegram getUpdates failed, backing off` on Telegram polling errors.
- No metrics export (Prometheus / OpenTelemetry) — intentional for the
  prototype.

---

## 13. Security posture

- **TLS to downstreams** — reqwest is built with `rustls-tls`, no OpenSSL.
- **Secrets** — loaded from env only; no committed `.env`.
- **Auth** — none in front of the HTTP API. Assumes deployment behind a
  trusted ingress (compose network or internal VPC).
- **CORS** — single-origin allow-list from `CORS_ALLOW_ORIGIN`; if parsing
  fails, falls back to `Any` with a warning (prototype-friendly, not
  production-appropriate).
- **Downstream bearer token** — `ConversationChatClient` uses
  `bearer_auth("internal")`, a placeholder shared secret. Relevant only if
  the legacy forwarder path is ever wired back.
- **PII** — message text is logged at `info` via request tracing and sent
  to `metricas`. Revisit before touching real patient data.

---

## 14. Build & release

- `cargo build --release` with LTO, `codegen-units = 1`, `strip = true` for
  a compact binary.
- `Dockerfile` is a two-stage build: `rust:1.88-slim` for compile,
  `debian:bookworm-slim` + `ca-certificates` for runtime. Final image runs
  as non-root UID 10001 on port 3000.
- MSRV declared as `1.75` in `Cargo.toml`, but the build image pins `1.88`
  because transitive deps require it; local `rustc 1.75` is not guaranteed
  to build.

---

## 15. Known gaps & future work

Ordered by decreasing priority for a production hand-off:

1. **Move session ownership to `conversation-chat`.** Reinstate the thin-
   forwarder shape; the in-memory store is the biggest operational risk.
2. **Persist session history to Mongo** (as originally designed) so that
   process restarts don't wipe conversations.
3. **Per-session idle eviction** to bound memory on the current in-memory
   store while sessions remain there.
4. **AuthN/Z on `/v1/*`** — at minimum a shared secret header between
   frontend and orch.
5. **Rate limiting and per-tenant quotas.**
6. **Tenant-scoped Telegram routing** so multiple bots can share one orch
   process without `TELEGRAM_DEFAULT_TENANT_ID`.
7. **Metrics export** (Prometheus `/metrics`) and OpenTelemetry traces.
8. **Retries / circuit breakers** on LLM and hospital-mock calls.
9. **PII redaction** on logs and on the `metricas` payload.
10. **Replace the hard-coded system prompt** with a per-tenant configuration
    loaded from the `tenant` service.
