// Tasks 14-15: Full implementation pending.
//
// Channel webhook handlers. Each channel owns its full round-trip:
// verify signature → parse → normalize → pipeline → (async reply via sender)

pub mod telegram;
pub mod telegram_polling;
pub mod telegram_sender;
pub mod telegram_types;
pub mod whatsapp;
pub mod whatsapp_sender;
pub mod whatsapp_types;
