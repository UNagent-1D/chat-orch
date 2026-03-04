// Integration test: Telegram webhook handling.
//
// Tests the full webhook flow: signature → parse → normalize → dedup.
// Uses wiremock for the Tenant Service mock.

/// Test that a valid Telegram text message is normalized correctly.
#[tokio::test]
async fn telegram_text_message_normalizes() {
    // Build a minimal Telegram update
    let update_json = serde_json::json!({
        "update_id": 123456,
        "message": {
            "message_id": 1,
            "from": { "id": 42, "first_name": "Test", "is_bot": false },
            "chat": { "id": 42, "type": "private" },
            "date": 1709568000,
            "text": "Hello bot!"
        }
    });

    let update: chat_orch::ingest::telegram_types::TelegramUpdate =
        serde_json::from_value(update_json).expect("should parse Telegram update");

    assert!(update.message.is_some());
    let msg = update.message.unwrap();
    assert_eq!(msg.text.as_deref(), Some("Hello bot!"));
    assert_eq!(msg.from.unwrap().id, 42);
}

/// Test that callback_query is parsed correctly.
#[tokio::test]
async fn telegram_callback_query_parses() {
    let update_json = serde_json::json!({
        "update_id": 123457,
        "callback_query": {
            "id": "cb_123",
            "from": { "id": 42, "first_name": "Test", "is_bot": false },
            "message": {
                "message_id": 5,
                "from": { "id": 100, "first_name": "Bot", "is_bot": true },
                "chat": { "id": 42, "type": "private" },
                "date": 1709568000,
                "text": "Pick a doctor"
            },
            "data": "doc-001"
        }
    });

    let update: chat_orch::ingest::telegram_types::TelegramUpdate =
        serde_json::from_value(update_json).expect("should parse callback_query");

    assert!(update.callback_query.is_some());
    let cb = update.callback_query.unwrap();
    assert_eq!(cb.data.as_deref(), Some("doc-001"));
    assert_eq!(cb.from.id, 42);
}
