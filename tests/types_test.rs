// Integration test: Core type behavior.

use chat_orch::types::message_content::MessageContent;

#[test]
fn text_should_route_to_llm() {
    let content = MessageContent::Text {
        text: "hello".to_string(),
    };
    assert!(content.should_route_to_llm());
    assert!(!content.needs_fallback_reply());
    assert!(!content.is_silent());
}

#[test]
fn image_with_caption_should_route_to_llm() {
    let content = MessageContent::Image {
        file_id: "abc".to_string(),
        caption: Some("look at this".to_string()),
    };
    assert!(content.should_route_to_llm());
}

#[test]
fn image_without_caption_needs_fallback() {
    let content = MessageContent::Image {
        file_id: "abc".to_string(),
        caption: None,
    };
    assert!(!content.should_route_to_llm());
    assert!(content.needs_fallback_reply());
}

#[test]
fn sticker_is_silent() {
    let content = MessageContent::Sticker {
        file_id: "abc".to_string(),
        emoji: Some("😀".to_string()),
    };
    assert!(content.is_silent());
    assert!(!content.should_route_to_llm());
    assert!(!content.needs_fallback_reply());
}

#[test]
fn reaction_is_silent() {
    let content = MessageContent::Reaction {
        emoji: "👍".to_string(),
        target_message_id: "msg123".to_string(),
    };
    assert!(content.is_silent());
}

#[test]
fn unsupported_needs_fallback() {
    let content = MessageContent::Unsupported {
        type_name: "poll".to_string(),
        raw_sample: None,
    };
    assert!(content.needs_fallback_reply());
    assert!(!content.should_route_to_llm());
}

#[test]
fn location_routes_to_llm() {
    let content = MessageContent::Location {
        lat: 4.6,
        lng: -74.1,
    };
    assert!(content.should_route_to_llm());
}

#[test]
fn callback_query_routes_to_llm() {
    let content = MessageContent::CallbackQuery {
        data: "doc-001".to_string(),
        message_id: "5".to_string(),
    };
    assert!(content.should_route_to_llm());
}

#[test]
fn interactive_routes_to_llm() {
    let content = MessageContent::Interactive {
        action_type: "list_reply".to_string(),
        payload: serde_json::json!({"id": "doc-001"}),
    };
    assert!(content.should_route_to_llm());
}

#[test]
fn video_needs_fallback() {
    let content = MessageContent::Video {
        file_id: "abc".to_string(),
        caption: None,
    };
    assert!(content.needs_fallback_reply());
    assert!(!content.should_route_to_llm());
}

#[test]
fn audio_needs_fallback() {
    let content = MessageContent::Audio {
        file_id: "abc".to_string(),
        duration_secs: Some(30),
    };
    assert!(content.needs_fallback_reply());
}

#[test]
fn document_needs_fallback() {
    let content = MessageContent::Document {
        file_id: "abc".to_string(),
        filename: "test.pdf".to_string(),
    };
    assert!(content.needs_fallback_reply());
}

#[test]
fn type_name_is_correct() {
    assert_eq!(
        MessageContent::Text {
            text: "x".to_string()
        }
        .type_name(),
        "text"
    );
    assert_eq!(
        MessageContent::Location {
            lat: 0.0,
            lng: 0.0
        }
        .type_name(),
        "location"
    );
}
