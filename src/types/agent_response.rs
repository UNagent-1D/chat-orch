use serde::{Deserialize, Serialize};

/// The response produced by the conversation turn logic (LLM + tool calls).
///
/// This is **channel-agnostic** — it does not know about Telegram keyboards
/// or WhatsApp interactive messages. The `reply_sender` in the gateway module
/// is responsible for mapping `ResponsePart` variants to channel-native formats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Ordered list of response parts. Most responses have a single `Text` part,
    /// but the agent can also include media, quick replies, or interactive menus.
    pub parts: Vec<ResponsePart>,
}

impl AgentResponse {
    /// Create a simple text-only response.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![ResponsePart::Text {
                text: text.into(),
            }],
        }
    }

    /// Check if the response has any content.
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

/// A single part of an agent response.
///
/// Responses can be composed of multiple parts (e.g., a text message
/// followed by a quick reply picker). The reply sender maps each part
/// to the appropriate channel-native format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsePart {
    /// Plain text response.
    Text {
        text: String,
    },

    /// Media attachment (image, video, document).
    Media {
        url: String,
        media_type: MediaType,
        caption: Option<String>,
    },

    /// Quick reply buttons (Telegram reply keyboard, WhatsApp button message).
    QuickReplies {
        /// Prompt text shown above the buttons.
        prompt: String,
        /// List of quick reply options.
        options: Vec<QuickReply>,
    },

    /// Interactive menu (WhatsApp list picker, Telegram inline keyboard).
    InteractiveMenu {
        header: Option<String>,
        body: String,
        /// Grouped options (WhatsApp sections / Telegram rows).
        sections: Vec<MenuSection>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Image,
    Video,
    Audio,
    Document,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickReply {
    /// Display text on the button.
    pub label: String,
    /// Value sent back when the user taps the button.
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuSection {
    /// Section header (e.g., "Cardiology").
    pub title: String,
    /// Options within this section.
    pub options: Vec<MenuOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuOption {
    /// Unique identifier for this option.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Optional description shown below the title.
    pub description: Option<String>,
}
