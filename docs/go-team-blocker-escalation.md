# Cross-Team Blocker: Missing `GET /internal/resolve-channel` Endpoint

*Created: March 2026 | Status: BLOCKING for WhatsApp production deployment*

---

## Summary

The Chat Orchestrator requires a **reverse-lookup endpoint** to resolve WhatsApp
`phone_number_id` values to tenant identifiers. This endpoint
(`GET /internal/resolve-channel`) was requested when the orchestrator was designed,
but it is **not present** in the Go team's v2.1 requirements document.

Verification: `docs/requiremnts_v2.1.md` is byte-identical to the previous
`docs/tenant_acr_requirements_v2.docx.md`. The Go team published the same
document with a different filename. No new endpoints were added.

## What We Need

```
GET /internal/resolve-channel?channel_type=whatsapp&channel_key=<phone_number_id>
```

**Response:**
```json
{
  "tenant_id": "550e8400-e29b-41d4-a716-446655440000",
  "tenant_slug": "hospital-san-ignacio",
  "agent_profile_id": "770e8400-e29b-41d4-a716-446655440002",
  "webhook_secret_ref": "vault-key-whatsapp-secret",
  "is_active": true
}
```

## Why We Need It

WhatsApp uses a **single webhook URL** (`POST /webhook/whatsapp`) for ALL tenants.
The `phone_number_id` in the payload metadata identifies which tenant the message
belongs to. Without a reverse-lookup endpoint, we cannot map incoming WhatsApp
messages to tenants.

Telegram does not have this problem because it uses **per-tenant webhook URLs**
(`POST /webhook/telegram/:tenant_slug`) where the tenant is embedded in the path.

## Architectural Challenge

The Go team's data model uses **per-tenant schemas** with a `channels` table in
each schema. A reverse lookup (channel_key -> tenant_id) requires searching
across all tenant schemas, which is non-trivial in their architecture.

## Proposed Solutions for the Go Team

1. **Global `channel_registry` table** (recommended): Create a table in the
   shared/public schema: `(channel_type, channel_key) -> (tenant_id, tenant_slug,
   profile_id)`. Maintained via triggers or application-level writes when channels
   are created/updated in tenant schemas.

2. **Materialized view**: `CREATE MATERIALIZED VIEW channel_lookup AS SELECT ...`
   joining across all tenant schemas. Refreshed on a schedule (e.g., every 60s).
   Simple but introduces staleness.

3. **Redis-based index**: When a channel is created in a tenant schema, the Go
   service writes a Redis key `channel:{type}:{key} -> {tenant_id,slug,profile_id}`.
   Fast lookups, but introduces a cache consistency problem.

## Current Workaround

We have implemented a static tenant map via the `WHATSAPP_STATIC_TENANT_MAP`
environment variable. This is a JSON array mapping `phone_number_id` to tenant
resolution data, configured at deployment time.

**Limitations of the workaround:**
- Requires service restart to update mappings
- Does not support dynamic channel provisioning
- Only suitable for MVP / single-tenant deployments
- Cannot scale to multi-tenant production

## Impact on Timeline

- **MVP deployment**: Not blocked (static map workaround works for testing)
- **Multi-tenant production**: BLOCKED until the Go team ships this endpoint
- **WhatsApp onboarding**: BLOCKED (new tenants cannot be dynamically resolved)

## Secondary Ask: Tool Registry Alignment

The v2.1 spec (section 3.3) defines a `tool_registry` table with
`openai_function_def` JSONB. Our orchestrator now integrates with this via
`GET /api/v1/tool-registry`. We need confirmation from the Go team on:

1. Is this endpoint available in their current deployment?
2. What is the expected response format? (We've assumed the schema from their spec)
3. Will tool definitions include full OpenAI function-calling JSON schemas?

## Action Items

- [ ] Go team to add `GET /internal/resolve-channel` to their API spec
- [ ] Go team to choose an implementation approach (global table, materialized view, or Redis)
- [ ] Go team to provide an estimated delivery date
- [ ] Go team to confirm `GET /api/v1/tool-registry` endpoint availability
- [ ] Orchestrator team to remove static tenant map once the endpoint is live
