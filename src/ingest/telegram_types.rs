// Telegram Bot API types — subset needed for webhook handling.
//
// These mirror the Telegram Bot API response shapes. Only fields we actually
// use are included to keep deserialization lightweight.

use serde::Deserialize;

/// Top-level Telegram update object.
/// Received at `POST /webhook/telegram/:tenant_slug`.
#[derive(Debug, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub edited_message: Option<TelegramMessage>,
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub from: Option<TelegramUser>,
    pub chat: TelegramChat,
    pub date: i64,

    // Content fields — at most one is populated per message
    pub text: Option<String>,
    pub photo: Option<Vec<PhotoSize>>,
    pub video: Option<TelegramVideo>,
    pub audio: Option<TelegramAudio>,
    pub voice: Option<TelegramVoice>,
    pub document: Option<TelegramDocument>,
    pub sticker: Option<TelegramSticker>,
    pub location: Option<TelegramLocation>,
    pub contact: Option<TelegramContact>,

    // Reply context
    pub reply_to_message: Option<Box<TelegramMessage>>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
}

#[derive(Debug, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: u32,
    pub height: u32,
    pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramVideo {
    pub file_id: String,
    pub duration: u32,
}

#[derive(Debug, Deserialize)]
pub struct TelegramAudio {
    pub file_id: String,
    pub duration: u32,
}

#[derive(Debug, Deserialize)]
pub struct TelegramVoice {
    pub file_id: String,
    pub duration: u32,
}

#[derive(Debug, Deserialize)]
pub struct TelegramDocument {
    pub file_id: String,
    pub file_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramSticker {
    pub file_id: String,
    pub emoji: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramLocation {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Deserialize)]
pub struct TelegramContact {
    pub phone_number: String,
    pub first_name: String,
    pub last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: TelegramUser,
    pub message: Option<TelegramMessage>,
    pub data: Option<String>,
}
