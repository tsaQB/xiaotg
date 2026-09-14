#![allow(dead_code)]

#[path = "../src/bot/models.rs"]
pub mod models;
pub mod bot {
    pub use crate::models;
}
#[path = "../src/parser.rs"]
mod parser;
#[path = "../src/ai/stream.rs"]
mod stream;

use models::{InputMedia, RichBlock, RichMessageButton, RichTextButton};
use stream::SseDecoder;

fn photo_media(id: &str) -> InputMedia {
    InputMedia::Photo {
        media: id.to_string(),
        caption: None,
        parse_mode: None,
        show_caption_above_media: None,
        has_spoiler: None,
    }
}

#[test]
fn voice_note_input_media_uses_bot_api_10_3_discriminator() {
    let media = InputMedia::VoiceNote {
        media: "file-id".to_string(),
        caption: None,
        parse_mode: None,
        duration: None,
    };
    let value = serde_json::to_value(media).expect("voice note should serialize");
    assert_eq!(value["type"], "voice_note");
}

#[test]
fn parsed_voice_note_uses_bot_api_10_3_nested_media_discriminator() {
    let blocks =
        parser::parse_markdown_to_rich_blocks("[voice: Rekaman](https://example.com/sample.ogg)");
    let Some(RichBlock::VoiceNote { voice_note, .. }) = blocks.first() else {
        panic!("expected parsed voice-note block");
    };
    assert_eq!(voice_note["type"], "voice_note");
}

#[test]
fn rich_text_button_includes_required_type_discriminator() {
    let rich_text = RichTextButton {
        button: RichMessageButton::callback("Retry", "retry"),
    };
    let value = serde_json::to_value(rich_text).expect("rich text button should serialize");
    assert_eq!(value["type"], "button");
    assert_eq!(value["button"]["callback_data"], "retry");
}

#[test]
fn malformed_sse_data_is_rejected_instead_of_silently_dropped() {
    let mut decoder = SseDecoder::default();
    let error = decoder
        .push(b"data: {not-json}\n\n")
        .expect_err("malformed SSE JSON must be surfaced as an error");
    assert!(error.contains("invalid JSON"), "unexpected error: {error}");
}

#[test]
fn media_group_requires_two_to_ten_album_compatible_items() {
    assert!(InputMedia::validate_media_group(&[photo_media("a")]).is_err());
    assert!(InputMedia::validate_media_group(&[photo_media("a"), photo_media("b")]).is_ok());
    assert!(InputMedia::validate_media_group(
        &(0..10)
            .map(|index| photo_media(&format!("p{index}")))
            .collect::<Vec<_>>()
    )
    .is_ok());
    assert!(InputMedia::validate_media_group(
        &(0..11)
            .map(|index| photo_media(&format!("p{index}")))
            .collect::<Vec<_>>()
    )
    .is_err());

    let animation = InputMedia::Animation {
        media: "anim".to_string(),
        caption: None,
        parse_mode: None,
        show_caption_above_media: None,
        width: None,
        height: None,
        duration: None,
        has_spoiler: None,
    };
    assert!(InputMedia::validate_media_group(&[photo_media("a"), animation]).is_err());

    let voice_note = InputMedia::VoiceNote {
        media: "voice".to_string(),
        caption: None,
        parse_mode: None,
        duration: None,
    };
    assert!(InputMedia::validate_media_group(&[photo_media("a"), voice_note]).is_err());
}

#[test]
fn media_group_keeps_audio_and_documents_in_homogeneous_albums() {
    let audio = InputMedia::audio("audio", None, None, None, None);
    let document = InputMedia::document("document", None, None);
    let video = InputMedia::video("video", None, None);

    assert!(InputMedia::validate_media_group(&[photo_media("photo"), video]).is_ok());
    assert!(InputMedia::validate_media_group(&[photo_media("photo"), audio.clone()]).is_err());
    assert!(InputMedia::validate_media_group(&[photo_media("photo"), document.clone()]).is_err());
    assert!(InputMedia::validate_media_group(&[audio.clone(), document.clone()]).is_err());
    assert!(InputMedia::validate_media_group(&[audio.clone(), audio]).is_ok());
    assert!(InputMedia::validate_media_group(&[document.clone(), document]).is_ok());
}

#[test]
fn permanent_send_paths_do_not_emit_draft_id_zero() {
    let source = include_str!("../src/bot/client.rs");
    assert!(
        !source.contains("\"draft_id\": 0"),
        "permanent wrapper sendMessage/sendRichMessage payloads must not include draft_id"
    );
}

#[test]
fn raw_transport_permanent_send_paths_do_not_emit_draft_id_zero() {
    let source = include_str!("../src/bot/client/raw.rs");
    assert!(
        !source.contains("\"draft_id\": 0"),
        "inner transport permanent sends must not retain the invalid draft_id sentinel"
    );
}

#[test]
fn raw_media_downloader_enforces_safe_outbound_url_policy() {
    let source = include_str!("../src/bot/client/raw.rs");
    let start = source
        .find("pub async fn download_media_bytes")
        .expect("raw downloader must exist");
    let tail = &source[start..];
    let end = tail
        .find("pub async fn send_photo(")
        .expect("send_photo should follow raw downloader");
    let body = &tail[..end];
    assert!(
        body.contains("resolve_download_url"),
        "external media fallback must resolve and validate outbound targets before requesting them"
    );
    assert!(
        body.contains("Policy::none()"),
        "external media fallback must disable redirects so a public URL cannot pivot to a private target"
    );
    assert!(
        body.contains(".no_proxy()"),
        "external media fallback must not let ambient proxies bypass DNS pinning"
    );
}

#[test]
fn parsed_expandable_blockquote_serializes_with_10_3_discriminator() {
    let blocks = parser::parse_markdown_to_rich_blocks("**> Catatan penting yang dapat dilipat");
    let Some(RichBlock::ExpandableBlockQuotation { .. }) = blocks.first() else {
        panic!("expected expandable blockquote");
    };
    let rich_message = models::InputRichMessage::new(blocks);
    assert!(rich_message.validate().is_ok());
    let value = serde_json::to_value(&rich_message).expect("should serialize rich message");
    assert_eq!(value["blocks"][0]["type"], "expandable_blockquote");
    assert_eq!(
        value["blocks"][0]["text"],
        "Catatan penting yang dapat dilipat"
    );
}

#[test]
fn parsed_rich_inline_spoiler_and_strikethrough_serialize() {
    let blocks = parser::parse_markdown_to_rich_blocks("Hasil: ||jawaban rahasia|| dan ~~coret~~");
    let rich_message = models::InputRichMessage::new(blocks);
    assert!(rich_message.validate().is_ok());
    let value = serde_json::to_value(&rich_message).expect("should serialize rich message");
    let serialized = value.to_string();
    assert!(serialized.contains(r#""type":"spoiler""#));
    assert!(serialized.contains(r#""type":"strikethrough""#));
}

#[test]
fn raw_client_exposes_explicit_delete_ephemeral_message() {
    let source = include_str!("../src/bot/client/raw.rs");
    assert!(
        source.contains("pub async fn delete_ephemeral_message("),
        "raw client must provide explicit delete_ephemeral_message API"
    );
    assert!(
        source.contains("\"deleteEphemeralMessage\""),
        "raw client must target Telegram deleteEphemeralMessage method"
    );
}

#[test]
fn parsed_collage_and_slideshow_serialize_with_10_3_discriminators() {
    let collage_block = RichBlock::Collage {
        blocks: vec![
            serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/p1.jpg"}}),
            serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/p2.jpg"}}),
        ],
        caption: Some(models::RichBlockCaption::new(serde_json::json!(
            "Galeri Foto"
        ))),
    };
    let slideshow_block = RichBlock::Slideshow {
        blocks: vec![
            serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/s1.jpg"}}),
            serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/s2.jpg"}}),
        ],
        caption: None,
    };
    let rich_message = models::InputRichMessage::new(vec![collage_block, slideshow_block]);
    assert!(rich_message.validate().is_ok());
    let value =
        serde_json::to_value(&rich_message).expect("should serialize collage and slideshow");
    assert_eq!(value["blocks"][0]["type"], "collage");
    assert_eq!(value["blocks"][0]["blocks"].as_array().unwrap().len(), 2);
    assert_eq!(value["blocks"][1]["type"], "slideshow");
    assert_eq!(value["blocks"][1]["blocks"].as_array().unwrap().len(), 2);
}

#[test]
fn telegram_command_registration_is_exclusively_start() {
    let source = include_str!("../src/main.rs");
    let cmd_start = source
        .find("// Register Bot Commands")
        .expect("bot command registration block");
    let cmd_end = source[cmd_start..]
        .find("bot.set_my_commands")
        .map(|offset| cmd_start + offset)
        .expect("bot.set_my_commands call");
    let block = &source[cmd_start..cmd_end];
    assert!(block.contains("BotCommand::ephemeral(\"start\","));
    assert!(!block.contains("\"menu\""));
    assert!(!block.contains("\"help\""));
    assert!(!block.contains("\"clear\""));
    assert!(!block.contains("\"image\""));
    assert!(!block.contains("\"session\""));
    assert!(!block.contains("\"context\""));
}
