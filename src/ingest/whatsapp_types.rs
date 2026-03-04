// WhatsApp Cloud API webhook types.
//
// The webhook payload is deeply nested:
// entry[].changes[].value.messages[]
//
// CRITICAL: Filter out `statuses[]` (delivery receipts) — never route to LLM.

use serde::Deserialize;

/// Top-level WhatsApp webhook payload.
#[derive(Debug, Deserialize)]
pub struct WhatsAppWebhook {
    pub object: String,
    pub entry: Vec<WhatsAppEntry>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppEntry {
    pub id: String,
    pub changes: Vec<WhatsAppChange>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppChange {
    pub value: WhatsAppValue,
    pub field: String,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppValue {
    pub messaging_product: Option<String>,
    pub metadata: WhatsAppMetadata,
    /// Actual messages — may be absent (e.g., status-only webhooks)
    #[serde(default)]
    pub messages: Vec<WhatsAppMessage>,
    /// Contact info for the senders (parallel array to messages)
    #[serde(default)]
    pub contacts: Vec<WhatsAppContact>,
    /// Delivery status updates — FILTER THESE OUT, never route to LLM
    #[serde(default)]
    pub statuses: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppMetadata {
    /// This is the canonical channel_key for tenant resolution.
    pub phone_number_id: String,
    pub display_phone_number: String,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppMessage {
    pub from: String,
    pub id: String,
    pub timestamp: String,
    #[serde(rename = "type")]
    pub msg_type: String,

    // Content fields — exactly one is populated per message
    pub text: Option<WhatsAppText>,
    pub image: Option<WhatsAppMedia>,
    pub video: Option<WhatsAppMedia>,
    pub audio: Option<WhatsAppMedia>,
    pub document: Option<WhatsAppDocument>,
    pub location: Option<WhatsAppLocation>,
    pub contacts: Option<Vec<WhatsAppMessageContact>>,
    pub sticker: Option<WhatsAppMedia>,
    pub interactive: Option<WhatsAppInteractive>,
    pub button: Option<WhatsAppButton>,

    // Context (reply)
    pub context: Option<WhatsAppContext>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppText {
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppMedia {
    pub id: String,
    pub mime_type: Option<String>,
    pub caption: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppDocument {
    pub id: String,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub caption: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub name: Option<String>,
    pub address: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppMessageContact {
    pub name: WhatsAppContactName,
    pub phones: Option<Vec<WhatsAppPhone>>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppContactName {
    pub formatted_name: String,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppPhone {
    pub phone: String,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppInteractive {
    #[serde(rename = "type")]
    pub interactive_type: String,
    pub list_reply: Option<WhatsAppListReply>,
    pub button_reply: Option<WhatsAppButtonReply>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppListReply {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppButtonReply {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppButton {
    pub text: String,
    pub payload: String,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppContext {
    pub message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppContact {
    pub profile: WhatsAppProfile,
    pub wa_id: String,
}

#[derive(Debug, Deserialize)]
pub struct WhatsAppProfile {
    pub name: String,
}
