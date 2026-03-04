// Integration test: WhatsApp webhook handling.

/// Test that a WhatsApp text message payload parses correctly.
#[tokio::test]
async fn whatsapp_text_message_parses() {
    let webhook_json = serde_json::json!({
        "object": "whatsapp_business_account",
        "entry": [{
            "id": "WHATSAPP_BUSINESS_ACCOUNT_ID",
            "changes": [{
                "value": {
                    "messaging_product": "whatsapp",
                    "metadata": {
                        "display_phone_number": "15551234567",
                        "phone_number_id": "123456789012345"
                    },
                    "contacts": [{
                        "profile": { "name": "Test User" },
                        "wa_id": "573009876543"
                    }],
                    "messages": [{
                        "from": "573009876543",
                        "id": "wamid.test123",
                        "timestamp": "1709568000",
                        "text": { "body": "I need a doctor appointment" },
                        "type": "text"
                    }]
                },
                "field": "messages"
            }]
        }]
    });

    let webhook: chat_orch::ingest::whatsapp_types::WhatsAppWebhook =
        serde_json::from_value(webhook_json).expect("should parse WhatsApp webhook");

    assert_eq!(webhook.object, "whatsapp_business_account");
    assert_eq!(webhook.entry.len(), 1);

    let change = &webhook.entry[0].changes[0];
    assert_eq!(change.value.metadata.phone_number_id, "123456789012345");
    assert_eq!(change.value.messages.len(), 1);
    assert_eq!(change.value.messages[0].msg_type, "text");
    assert_eq!(
        change.value.messages[0].text.as_ref().unwrap().body,
        "I need a doctor appointment"
    );
}

/// Test that status-only webhooks have empty messages.
#[tokio::test]
async fn whatsapp_status_only_has_no_messages() {
    let webhook_json = serde_json::json!({
        "object": "whatsapp_business_account",
        "entry": [{
            "id": "WHATSAPP_BUSINESS_ACCOUNT_ID",
            "changes": [{
                "value": {
                    "messaging_product": "whatsapp",
                    "metadata": {
                        "display_phone_number": "15551234567",
                        "phone_number_id": "123456789012345"
                    },
                    "statuses": [{
                        "id": "wamid.test456",
                        "status": "delivered",
                        "timestamp": "1709568001"
                    }]
                },
                "field": "messages"
            }]
        }]
    });

    let webhook: chat_orch::ingest::whatsapp_types::WhatsAppWebhook =
        serde_json::from_value(webhook_json).expect("should parse status webhook");

    let change = &webhook.entry[0].changes[0];
    assert!(change.value.messages.is_empty(), "status-only webhook should have no messages");
    assert!(!change.value.statuses.is_empty(), "should have statuses");
}

/// Test WhatsApp interactive list_reply parsing.
#[tokio::test]
async fn whatsapp_interactive_list_reply_parses() {
    let webhook_json = serde_json::json!({
        "object": "whatsapp_business_account",
        "entry": [{
            "id": "WHATSAPP_BUSINESS_ACCOUNT_ID",
            "changes": [{
                "value": {
                    "messaging_product": "whatsapp",
                    "metadata": {
                        "display_phone_number": "15551234567",
                        "phone_number_id": "123456789012345"
                    },
                    "contacts": [{
                        "profile": { "name": "Test User" },
                        "wa_id": "573009876543"
                    }],
                    "messages": [{
                        "from": "573009876543",
                        "id": "wamid.test789",
                        "timestamp": "1709568002",
                        "type": "interactive",
                        "interactive": {
                            "type": "list_reply",
                            "list_reply": {
                                "id": "doc-001",
                                "title": "Dr. Garcia",
                                "description": "Cardiologist"
                            }
                        }
                    }]
                },
                "field": "messages"
            }]
        }]
    });

    let webhook: chat_orch::ingest::whatsapp_types::WhatsAppWebhook =
        serde_json::from_value(webhook_json).expect("should parse interactive webhook");

    let msg = &webhook.entry[0].changes[0].value.messages[0];
    assert_eq!(msg.msg_type, "interactive");
    assert!(msg.interactive.is_some());
    let interactive = msg.interactive.as_ref().unwrap();
    assert_eq!(interactive.interactive_type, "list_reply");
    assert_eq!(interactive.list_reply.as_ref().unwrap().id, "doc-001");
}
