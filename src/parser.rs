use serde_json::Value;

use crate::bot::models::{InputRichMessage, RichBlock};

#[path = "parser/markdown.rs"]
pub(crate) mod markdown;

#[path = "parser/terminal.rs"]
pub mod terminal;
#[allow(unused_imports)]
pub use terminal::render_terminal_markdown;

#[allow(dead_code)]
pub fn parse_inline(input_str: &str) -> Value {
    markdown::parse_inline(input_str)
}

#[allow(dead_code)]
pub fn parse_streaming_markdown_to_rich_blocks(text: &str) -> Vec<RichBlock> {
    let mut blocks = markdown::parse_streaming_markdown_to_rich_blocks(text);
    normalize_bot_api_10_3_media(&mut blocks);
    blocks
}

#[allow(dead_code)]
pub fn parse_markdown_to_rich_blocks(text: &str) -> Vec<RichBlock> {
    let mut blocks = markdown::parse_markdown_to_rich_blocks(text);
    normalize_bot_api_10_3_media(&mut blocks);
    blocks
}

#[allow(dead_code)]
pub fn build_full_rich_message(answer_text: &str, footer_text: Option<&str>) -> InputRichMessage {
    let mut message = markdown::build_full_rich_message(answer_text, footer_text);
    normalize_bot_api_10_3_media(&mut message.blocks);
    message
}

fn normalize_bot_api_10_3_media(blocks: &mut [RichBlock]) {
    for block in blocks {
        if let RichBlock::VoiceNote { voice_note, .. } = block {
            if let Some(object) = voice_note.as_object_mut() {
                object.insert("type".to_string(), Value::String("voice_note".to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_voice_note_wire_discriminator() {
        let blocks =
            parse_markdown_to_rich_blocks("[voice: Rekaman](https://example.com/sample.ogg)");
        let Some(RichBlock::VoiceNote { voice_note, .. }) = blocks.first() else {
            panic!("expected voice-note block");
        };
        assert_eq!(voice_note["type"], "voice_note");
    }
}
