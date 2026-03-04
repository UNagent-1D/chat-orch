# AGENTS.md — types/

## Purpose

Domain types for the Chat Orchestrator. This module defines the core data
structures that flow through the entire pipeline.

## Key Pattern: TypeState

The orchestrator uses a **TypeState pattern** to enforce tenant resolution at
compile time:

- `IngestMessage` — produced by webhook handlers. Tenant is NOT yet known.
- `ResolvedMessage` — produced after tenant resolution. `tenant_id` is guaranteed
  to be present (not `Option`). Created via `IngestMessage::resolve(tenant)`.

This makes illegal states unrepresentable: you cannot call `process_turn()` with
an unresolved message.

## Files

| File | Type | Description |
|------|------|-------------|
| `ingest_message.rs` | `IngestMessage` | Pre-resolution message from webhook |
| `resolved_message.rs` | `ResolvedMessage` | Post-resolution, tenant guaranteed |
| `message_content.rs` | `MessageContent` | Enum of all supported content types |
| `session.rs` | `Session`, `SessionKey` | Session state stored in Redis |
| `agent_response.rs` | `AgentResponse`, `ResponsePart` | LLM output, channel-agnostic |

## Conventions

- All types derive `Debug, Clone, Serialize, Deserialize` where appropriate.
- Media content stores `file_id` or URL only — never binary data.
- `Unsupported` message variants include `type_name` for observability.
