# Message Type Decision Matrix

*Version 1.0 | March 2026*

This document defines how each incoming message type is handled by the
Chat Orchestrator. It is the source of truth for normalization and routing
decisions in the ingest layer.

---

## Decision Rules

| # | Content Type | Route to LLM? | Response Strategy | Priority |
|---|-------------|----------------|-------------------|----------|
| 1 | **Text** | Yes | Full conversation turn | P0 |
| 2 | **Image** (with caption) | Yes | Process caption as text, acknowledge image receipt | P0 |
| 3 | **Image** (no caption) | No | "I received your image. Could you describe what you need in text?" | P2 |
| 4 | **Video** | No | "I can help you via text. Please type your request." | P2 |
| 5 | **Audio / Voice** | No (v1) | "I can't process audio yet. Please type your request." (v2: Whisper transcription) | P2 |
| 6 | **Document** | No (v1) | "I received your document. Please describe what you need." | P2 |
| 7 | **Location** | Yes | Forward to LLM — useful for finding nearby clinics | P1 |
| 8 | **Contact** | No | "I can help schedule appointments — please type your request." | P2 |
| 9 | **Sticker** | No | Silent acknowledge (200 OK), no reply sent | P3 |
| 10 | **Reaction** | No | Silent acknowledge (200 OK), no reply sent | P3 |
| 11 | **Interactive / Button Reply** (WhatsApp) | Yes | Process as structured user response (extract selected option) | P0 |
| 12 | **Callback Query** (Telegram) | Yes | Process as structured user response (extract callback data) | P0 |
| 13 | **Edited Message** (Telegram) | No | Silent acknowledge — do not re-process | P3 |
| 14 | **Delivery Status** (WhatsApp) | NEVER | Filter out before normalization — these are NOT messages | P0 |
| 15 | **Inline Query** (Telegram) | No | Out of scope for scheduling bot | P3 |
| 16 | **Unsupported / Unknown** | No | Log + metric + "I can help with text messages. Please type your request." | P1 |

---

## Channel-Specific Notes

### Telegram

- `update.message` — primary: text, photo, video, voice, document, sticker, location, contact
- `update.callback_query` — inline keyboard button presses (MUST process)
- `update.edited_message` — user edited a previous message (skip)
- `update.channel_post` — channel messages, not direct chat (skip)
- `update.inline_query` — inline bot queries (skip)
- `update.my_chat_member` — bot added/removed from chat (log, skip)
- User ID type: `i64` (signed 64-bit integer)
- `update_id` is monotonically increasing — useful for dedup

### WhatsApp Cloud API

- `entry[].changes[].value.messages[]` — actual messages to process
- `entry[].changes[].value.statuses[]` — delivery receipts (NEVER process)
- Message types: text, image, video, audio, document, sticker, location, contacts, interactive, reaction, order, system, unknown
- `interactive` subtypes: `list_reply`, `button_reply` — these are user responses to structured menus
- `phone_number_id` from `metadata` — use as `channel_key` for tenant resolution
- User phone is in `messages[].from` — use as `channel_user_id`
- Message ID format: `wamid.xxx` — use for dedup

---

## MessageContent Enum Mapping

```rust
pub enum MessageContent {
    // P0 — Always route to LLM
    Text(String),
    Interactive { action_type: String, payload: serde_json::Value },
    CallbackQuery { data: String, message_id: String },

    // P1 — Route to LLM with context
    Image { file_id: String, caption: Option<String> },
    Location { lat: f64, lng: f64 },

    // P2 — Acknowledge with fallback message, don't route to LLM
    Video { file_id: String, caption: Option<String> },
    Audio { file_id: String, duration_secs: Option<u32> },
    Document { file_id: String, filename: String },
    Contact { name: String, phone: String },

    // P3 — Silent acknowledge or skip
    Sticker { file_id: String, emoji: Option<String> },
    Reaction { emoji: String, target_message_id: String },

    // Catch-all — log and send polite fallback
    Unsupported { type_name: String, raw_sample: Option<String> },
}
```

---

## Fallback Reply Templates

| Content Type | Fallback Reply |
|-------------|---------------|
| Audio/Voice | "I can't process audio messages yet. Please type your request and I'll be happy to help you schedule an appointment." |
| Video | "I can't process video messages. Please type your request and I'll help you with scheduling." |
| Document | "I received your document. Could you please describe what you need? I can help with appointment scheduling." |
| Contact | "Thanks for sharing. I can help you schedule a medical appointment — please type what you need." |
| Unsupported | "I work best with text messages. Please type your request and I'll help you schedule an appointment." |
