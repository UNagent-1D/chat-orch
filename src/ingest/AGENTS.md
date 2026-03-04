# AGENTS.md — ingest/

## Purpose

Channel-specific webhook handlers for ingesting chat messages. Each channel
(Telegram, WhatsApp) has its own handler that owns the full inbound flow:
signature verification, payload parsing, normalization to `IngestMessage`,
and submission to the pipeline.

## Pattern: Handler-per-Channel

No shared trait for MVP. Each channel is a standalone Axum handler module.
Extract a common trait when the 3rd channel arrives.

## Telegram

- **Webhook**: `POST /webhook/telegram/:tenant_slug`
- **Signature**: Validates `X-Telegram-Bot-Api-Secret-Token` header
- **Polling fallback**: `TELEGRAM_USE_POLLING=true` for local dev without HTTPS
- **Update types handled**: message (text, photo, video, voice, document, sticker,
  location, contact), callback_query, edited_message
- **Sender**: `POST https://api.telegram.org/bot<TOKEN>/sendMessage` (and variants)

## WhatsApp (Meta Cloud API)

- **Webhook**: `POST /webhook/whatsapp` (single URL for all tenants)
- **Verification**: `GET /webhook/whatsapp` echoes `hub.challenge` during registration
- **Signature**: HMAC-SHA256 of raw body using `WHATSAPP_APP_SECRET`
- **Payload structure**: `entry[].changes[].value.messages[]` — iterate ALL messages
- **CRITICAL**: Filter out `statuses[]` (delivery receipts) — never route to LLM
- **Channel key**: `phone_number_id` from `metadata` (NOT the phone number string)
- **Sender**: `POST https://graph.facebook.com/v18.0/<PNID>/messages`

## Conventions

- Webhook handlers acquire semaphore BEFORE returning 200 OK
- Never download media binary — store `file_id` / URL only
- Unsupported message types → polite fallback reply, never silently dropped
- Raw body (`Bytes`) extracted before JSON parse for HMAC verification
