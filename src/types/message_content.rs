use serde::{Deserialize, Serialize};

/// All supported message content types across all channels.
///
/// Media content stores `file_id` or URL references only — binary data
/// is NEVER downloaded or held in memory by the orchestrator.
///
/// See `docs/message-type-matrix.md` for routing decisions per type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    // ── P0: Always route to LLM ──────────────────────────────────────
    /// Plain text message — the primary content type.
    Text { text: String },

    /// Structured response from interactive menus (WhatsApp list/button reply).
    Interactive {
        /// e.g., "list_reply", "button_reply"
        action_type: String,
        /// Channel-specific payload (selected option, etc.)
        payload: serde_json::Value,
    },

    /// Inline keyboard button press (Telegram callback_query).
    CallbackQuery {
        /// The `data` field from the callback button.
        data: String,
        /// The message_id of the message with the keyboard.
        message_id: String,
    },

    // ── P1: Route to LLM with context ────────────────────────────────
    /// Image with optional caption. Caption is sent to LLM as text.
    Image {
        /// Telegram `file_id` or WhatsApp media URL/ID.
        file_id: String,
        caption: Option<String>,
    },

    /// Geographic location — useful for finding nearby clinics.
    Location { lat: f64, lng: f64 },

    // ── P2: Acknowledge with fallback, don't route to LLM ────────────
    /// Video message — not processed in v1.
    Video {
        file_id: String,
        caption: Option<String>,
    },

    /// Audio/voice message — not processed in v1 (v2: Whisper transcription).
    Audio {
        file_id: String,
        duration_secs: Option<u32>,
    },

    /// Document/file attachment — not processed in v1.
    Document { file_id: String, filename: String },

    /// Shared contact information.
    Contact { name: String, phone: String },

    // ── P3: Silent acknowledge or skip ───────────────────────────────
    /// Sticker — silently acknowledged, no reply.
    Sticker {
        file_id: String,
        emoji: Option<String>,
    },

    /// Reaction to a message — silently acknowledged, no reply.
    Reaction {
        emoji: String,
        target_message_id: String,
    },

    // ── Catch-all ────────────────────────────────────────────────────
    /// Unknown or unsupported message type.
    /// Logged + metricked + polite fallback reply sent to user.
    Unsupported {
        /// The raw type name (for logging/metrics).
        type_name: String,
        /// Truncated raw payload for debugging (max 1KB).
        raw_sample: Option<String>,
    },
}

impl MessageContent {
    /// Whether this content type should be routed to the LLM for processing.
    pub fn should_route_to_llm(&self) -> bool {
        matches!(
            self,
            MessageContent::Text { .. }
                | MessageContent::Interactive { .. }
                | MessageContent::CallbackQuery { .. }
                | MessageContent::Image {
                    caption: Some(_),
                    ..
                }
                | MessageContent::Location { .. }
        )
    }

    /// Whether this content type should trigger a fallback reply to the user.
    pub fn needs_fallback_reply(&self) -> bool {
        matches!(
            self,
            MessageContent::Image { caption: None, .. }
                | MessageContent::Video { .. }
                | MessageContent::Audio { .. }
                | MessageContent::Document { .. }
                | MessageContent::Contact { .. }
                | MessageContent::Unsupported { .. }
        )
    }

    /// Whether this content type should be silently acknowledged (no reply).
    pub fn is_silent(&self) -> bool {
        matches!(
            self,
            MessageContent::Sticker { .. } | MessageContent::Reaction { .. }
        )
    }

    /// Stable, low-cardinality type name safe for use as a metric label.
    ///
    /// For `Unsupported` variants this returns the fixed string `"unsupported"`
    /// rather than the raw provider-supplied name — use [`raw_type_name()`] if
    /// you need the original value for debug logging.
    pub fn type_name(&self) -> &'static str {
        match self {
            MessageContent::Text { .. } => "text",
            MessageContent::Interactive { .. } => "interactive",
            MessageContent::CallbackQuery { .. } => "callback_query",
            MessageContent::Image { .. } => "image",
            MessageContent::Location { .. } => "location",
            MessageContent::Video { .. } => "video",
            MessageContent::Audio { .. } => "audio",
            MessageContent::Document { .. } => "document",
            MessageContent::Contact { .. } => "contact",
            MessageContent::Sticker { .. } => "sticker",
            MessageContent::Reaction { .. } => "reaction",
            MessageContent::Unsupported { .. } => "unsupported",
        }
    }

    /// The raw type name from the channel provider (only meaningful for
    /// `Unsupported` variants). Returns `None` for known types.
    pub fn raw_type_name(&self) -> Option<&str> {
        match self {
            MessageContent::Unsupported { type_name, .. } => Some(type_name),
            _ => None,
        }
    }
}
