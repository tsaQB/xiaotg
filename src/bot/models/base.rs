#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ==========================================
// Inline & Reply Keyboards
// ==========================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizontal_accuracy: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoginUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_write_access: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwitchInlineQueryChosenChat {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_user_chats: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_bot_chats: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_group_chats: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_channel_chats: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputMedia {
    #[serde(rename = "photo")]
    Photo {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        show_caption_above_media: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        has_spoiler: Option<bool>,
    },
    #[serde(rename = "video")]
    Video {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        show_caption_above_media: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        height: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        has_spoiler: Option<bool>,
    },
    #[serde(rename = "animation")]
    Animation {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        show_caption_above_media: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        height: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        has_spoiler: Option<bool>,
    },
    #[serde(rename = "audio")]
    Audio {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        performer: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    #[serde(rename = "document")]
    Document {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<String>,
    },
    #[serde(rename = "voice")]
    VoiceNote {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<i32>,
    },
}

impl InputMedia {
    pub fn photo(
        media: impl Into<String>,
        caption: Option<String>,
        parse_mode: Option<String>,
    ) -> Self {
        InputMedia::Photo {
            media: media.into(),
            caption,
            parse_mode,
            show_caption_above_media: None,
            has_spoiler: None,
        }
    }

    pub fn video(
        media: impl Into<String>,
        caption: Option<String>,
        parse_mode: Option<String>,
    ) -> Self {
        InputMedia::Video {
            media: media.into(),
            caption,
            parse_mode,
            show_caption_above_media: None,
            width: None,
            height: None,
            duration: None,
            has_spoiler: None,
        }
    }

    pub fn audio(
        media: impl Into<String>,
        caption: Option<String>,
        parse_mode: Option<String>,
        title: Option<String>,
        performer: Option<String>,
    ) -> Self {
        InputMedia::Audio {
            media: media.into(),
            caption,
            parse_mode,
            duration: None,
            performer,
            title,
        }
    }

    pub fn document(
        media: impl Into<String>,
        caption: Option<String>,
        parse_mode: Option<String>,
    ) -> Self {
        InputMedia::Document {
            media: media.into(),
            caption,
            parse_mode,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputRichMessageMedia {
    pub id: String,
    pub media: InputMedia,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopyTextButton {
    pub text: String,
}

impl CopyTextButton {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboardButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_app: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_text: Option<CopyTextButton>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<LoginUrl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switch_inline_query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switch_inline_query_current_chat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switch_inline_query_chosen_chat: Option<SwitchInlineQueryChosenChat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<Value>,
}

impl InlineKeyboardButton {
    pub fn callback(text: impl Into<String>, callback_data: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            callback_data: Some(callback_data.into()),
            url: None,
            web_app: None,
            copy_text: None,
            login_url: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            disabled: None,
        }
    }

    pub fn callback_styled(
        text: impl Into<String>,
        callback_data: impl Into<String>,
        style: impl Into<String>,
    ) -> Self {
        let mut button = Self::callback(text, callback_data);
        button.style = Some(style.into());
        button
    }

    pub fn url_btn(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            callback_data: None,
            url: Some(url.into()),
            web_app: None,
            copy_text: None,
            login_url: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            disabled: None,
        }
    }

    pub fn copy(text: impl Into<String>, copy_text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            callback_data: None,
            url: None,
            web_app: None,
            copy_text: Some(CopyTextButton::new(copy_text)),
            login_url: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            disabled: None,
        }
    }

    pub fn disabled(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            callback_data: None,
            url: None,
            web_app: None,
            copy_text: None,
            login_url: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            disabled: Some(serde_json::json!({})),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboardMarkup {
    pub inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_reply: Option<bool>,
}

impl InlineKeyboardMarkup {
    pub fn new(rows: Vec<Vec<InlineKeyboardButton>>) -> Self {
        Self {
            inline_keyboard: rows,
            force_reply: None,
        }
    }

    pub fn with_force_reply(mut self, force_reply: bool) -> Self {
        self.force_reply = Some(force_reply);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_contact: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_location: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_app: Option<Value>,
}

impl KeyboardButton {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            request_contact: None,
            request_location: None,
            web_app: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyKeyboardMarkup {
    pub keyboard: Vec<Vec<KeyboardButton>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_persistent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resize_keyboard: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub one_time_keyboard: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_field_placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selective: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_reply: Option<bool>,
}

impl ReplyKeyboardMarkup {
    pub fn from_strings(
        rows: Vec<Vec<&str>>,
        is_persistent: bool,
        resize_keyboard: bool,
        placeholder: Option<&str>,
    ) -> Self {
        let keyboard = rows
            .into_iter()
            .map(|row| row.into_iter().map(KeyboardButton::new).collect())
            .collect();

        Self {
            keyboard,
            is_persistent: Some(is_persistent),
            resize_keyboard: Some(resize_keyboard),
            one_time_keyboard: Some(false),
            input_field_placeholder: placeholder.map(|s| s.to_string()),
            selective: None,
            force_reply: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyKeyboardRemove {
    pub remove_keyboard: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selective: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_ephemeral: Option<bool>,
}

impl BotCommand {
    pub fn new(command: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            description: description.into(),
            is_ephemeral: None,
        }
    }

    pub fn ephemeral(command: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            description: description.into(),
            is_ephemeral: Some(true),
        }
    }
}

// ==========================================
// Telegram Bot API 10.3: Rich Message Blocks
// ==========================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichTextButton {
    pub button: RichMessageButton,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichMessageButton {
    pub text: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_app: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_text: Option<CopyTextButton>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<LoginUrl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switch_inline_query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switch_inline_query_current_chat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switch_inline_query_chosen_chat: Option<SwitchInlineQueryChosenChat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<Value>,
}

impl RichMessageButton {
    pub fn callback(text: impl Into<String>, callback_data: impl Into<String>) -> Self {
        Self {
            text: Value::String(text.into()),
            style: None,
            url: None,
            callback_data: Some(callback_data.into()),
            web_app: None,
            copy_text: None,
            login_url: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            disabled: None,
        }
    }

    pub fn callback_styled(
        text: impl Into<String>,
        callback_data: impl Into<String>,
        style: impl Into<String>,
    ) -> Self {
        let mut button = Self::callback(text, callback_data);
        button.style = Some(style.into());
        button
    }

    pub fn url(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: Value::String(text.into()),
            style: None,
            url: Some(url.into()),
            callback_data: None,
            web_app: None,
            copy_text: None,
            login_url: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            disabled: None,
        }
    }

    pub fn copy(text: impl Into<String>, copy_text: impl Into<String>) -> Self {
        Self {
            text: Value::String(text.into()),
            style: None,
            url: None,
            callback_data: None,
            web_app: None,
            copy_text: Some(CopyTextButton::new(copy_text)),
            login_url: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            disabled: None,
        }
    }

    pub fn disabled(text: impl Into<String>) -> Self {
        Self {
            text: Value::String(text.into()),
            style: None,
            url: None,
            callback_data: None,
            web_app: None,
            copy_text: None,
            login_url: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            disabled: Some(serde_json::json!({})),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let action_count = usize::from(self.url.is_some())
            + usize::from(self.callback_data.is_some())
            + usize::from(self.web_app.is_some())
            + usize::from(self.copy_text.is_some())
            + usize::from(self.login_url.is_some())
            + usize::from(self.switch_inline_query.is_some())
            + usize::from(self.switch_inline_query_current_chat.is_some())
            + usize::from(self.switch_inline_query_chosen_chat.is_some())
            + usize::from(self.disabled.is_some());
        if action_count != 1 {
            return Err(format!(
                "RichMessageButton must contain exactly one action, found {action_count}"
            ));
        }
        if let Some(copy_button) = &self.copy_text {
            let len = copy_button.text.chars().count();
            if len == 0 || len > 256 {
                return Err(format!(
                    "RichMessageButton copy_text must contain 1-256 characters, found {len}"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichBlockCaption {
    pub text: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credit: Option<Value>,
}

impl RichBlockCaption {
    pub fn new(text: Value) -> Self {
        Self { text, credit: None }
    }

    pub fn with_credit(text: Value, credit: Value) -> Self {
        Self {
            text,
            credit: Some(credit),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichBlockTableCell {
    pub text: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_header: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    pub valign: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colspan: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rowspan: Option<usize>,
}

impl RichBlockTableCell {
    pub fn new(text: Value, is_header: bool, align: Option<&str>) -> Self {
        Self {
            text,
            is_header: if is_header { Some(true) } else { None },
            align: align.map(|a| a.to_string()),
            valign: "middle".to_string(),
            colspan: None,
            rowspan: None,
        }
    }

    pub fn text_only(text: &str, is_header: bool, align: Option<&str>) -> Self {
        Self::new(Value::String(text.to_string()), is_header, align)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichBlockListItem {
    pub blocks: Vec<Value>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_checkbox: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_checked: Option<bool>,
}

impl RichBlockListItem {
    pub fn bullet(blocks: Vec<Value>) -> Self {
        Self {
            blocks,
            kind: None,
            value: None,
            has_checkbox: None,
            is_checked: None,
        }
    }

    pub fn ordered(blocks: Vec<Value>, value: Option<i64>) -> Self {
        Self {
            blocks,
            kind: Some("1".to_string()),
            value,
            has_checkbox: None,
            is_checked: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RichBlock {
    #[serde(rename = "paragraph")]
    Paragraph { text: Value },

    #[serde(rename = "heading")]
    SectionHeading {
        text: Value,
        #[serde(rename = "size")]
        level: usize,
    },

    #[serde(rename = "pre")]
    Preformatted {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },

    #[serde(rename = "footer")]
    Footer { text: Value },

    #[serde(rename = "list")]
    List { items: Vec<RichBlockListItem> },

    #[serde(rename = "blockquote")]
    BlockQuotation {
        blocks: Vec<Value>, // [{"type": "paragraph", "text": ...}]
    },

    #[serde(rename = "expandable_blockquote")]
    ExpandableBlockQuotation {
        text: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        credit: Option<Value>,
    },

    #[serde(rename = "pullquote")]
    PullQuotation {
        text: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        credit: Option<Value>,
    },

    #[serde(rename = "divider")]
    Divider {},

    #[serde(rename = "mathematical_expression")]
    MathematicalExpression { expression: String },

    #[serde(rename = "table")]
    Table {
        cells: Vec<Vec<RichBlockTableCell>>,
        #[serde(skip)]
        has_header: bool,
        is_bordered: bool,
        is_striped: bool,
        is_compact: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },

    #[serde(rename = "buttons")]
    Buttons {
        buttons: Vec<RichMessageButton>,
        #[serde(skip_serializing_if = "Option::is_none")]
        align: Option<String>,
    },

    #[serde(rename = "document")]
    Document {
        document: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<RichBlockCaption>,
    },

    #[serde(rename = "photo")]
    Photo {
        photo: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<RichBlockCaption>,
    },

    #[serde(rename = "video")]
    Video {
        video: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<RichBlockCaption>,
    },

    #[serde(rename = "audio")]
    Audio {
        audio: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<RichBlockCaption>,
    },

    #[serde(rename = "voice_note")]
    VoiceNote {
        voice_note: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<RichBlockCaption>,
    },

    #[serde(rename = "animation")]
    Animation {
        animation: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<RichBlockCaption>,
    },

    #[serde(rename = "collage")]
    Collage {
        blocks: Vec<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<RichBlockCaption>,
    },

    #[serde(rename = "slideshow")]
    Slideshow {
        blocks: Vec<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<RichBlockCaption>,
    },

    #[serde(rename = "map")]
    Map {
        location: Location,
        #[serde(skip_serializing_if = "Option::is_none")]
        zoom: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        height: Option<i32>,
    },

    #[serde(rename = "details")]
    Details {
        summary: Value,
        blocks: Vec<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_open: Option<bool>,
    },

    #[serde(rename = "anchor")]
    Anchor { name: String },

    #[serde(rename = "thinking")]
    Thinking { text: Value },
}

impl RichBlock {
    pub fn is_media(&self) -> bool {
        matches!(
            self,
            RichBlock::Photo { .. }
                | RichBlock::Video { .. }
                | RichBlock::Audio { .. }
                | RichBlock::VoiceNote { .. }
                | RichBlock::Animation { .. }
                | RichBlock::Collage { .. }
                | RichBlock::Slideshow { .. }
                | RichBlock::Map { .. }
                | RichBlock::Document { .. }
        )
    }

    pub fn caption_text(&self) -> Option<String> {
        let cap = match self {
            RichBlock::Photo { caption, .. }
            | RichBlock::Video { caption, .. }
            | RichBlock::Audio { caption, .. }
            | RichBlock::VoiceNote { caption, .. }
            | RichBlock::Animation { caption, .. }
            | RichBlock::Collage { caption, .. }
            | RichBlock::Slideshow { caption, .. }
            | RichBlock::Document { caption, .. } => caption.as_ref()?,
            _ => return None,
        };
        match &cap.text {
            Value::String(s) => Some(s.clone()),
            Value::Array(arr) => {
                let mut out = String::new();
                for item in arr {
                    if let Some(s) = item.as_str() {
                        out.push_str(s);
                    } else if let Some(s) = item.get("text").and_then(Value::as_str) {
                        out.push_str(s);
                    }
                }
                if out.is_empty() {
                    None
                } else {
                    Some(out)
                }
            }
            _ => None,
        }
    }

    pub fn get_media_urls(&self) -> Vec<String> {
        let extract = |val: &Value| -> Option<String> {
            val.get("media")
                .and_then(Value::as_str)
                .or_else(|| val.as_str())
                .map(|s| s.to_string())
        };

        match self {
            RichBlock::Photo { photo, .. } => extract(photo).into_iter().collect(),
            RichBlock::Video { video, .. } => extract(video).into_iter().collect(),
            RichBlock::Audio { audio, .. } => extract(audio).into_iter().collect(),
            RichBlock::VoiceNote { voice_note, .. } => extract(voice_note).into_iter().collect(),
            RichBlock::Animation { animation, .. } => extract(animation).into_iter().collect(),
            RichBlock::Document { document, .. } => extract(document).into_iter().collect(),
            RichBlock::Collage { blocks, .. } | RichBlock::Slideshow { blocks, .. } => {
                let mut urls = Vec::new();
                for item in blocks {
                    if let Some(p) = item.get("photo") {
                        if let Some(u) = extract(p) {
                            urls.push(u);
                        }
                    } else if let Some(v) = item.get("video") {
                        if let Some(u) = extract(v) {
                            urls.push(u);
                        }
                    } else if let Some(u) = extract(item) {
                        urls.push(u);
                    }
                }
                urls
            }
            _ => Vec::new(),
        }
    }

    pub fn replace_media_urls<F: Fn(&str) -> Option<String>>(&mut self, replacer: &F) {
        let mutate = |val: &mut Value| {
            if let Some(obj) = val.as_object_mut() {
                if let Some(m) = obj.get("media").and_then(Value::as_str) {
                    if let Some(new_val) = replacer(m) {
                        obj.insert("media".to_string(), Value::String(new_val));
                    }
                }
            } else if let Some(s) = val.as_str() {
                if let Some(new_val) = replacer(s) {
                    *val = Value::String(new_val);
                }
            }
        };

        match self {
            RichBlock::Photo { photo, .. } => mutate(photo),
            RichBlock::Video { video, .. } => mutate(video),
            RichBlock::Audio { audio, .. } => mutate(audio),
            RichBlock::VoiceNote { voice_note, .. } => mutate(voice_note),
            RichBlock::Animation { animation, .. } => mutate(animation),
            RichBlock::Document { document, .. } => mutate(document),
            RichBlock::Collage { blocks, .. } | RichBlock::Slideshow { blocks, .. } => {
                for item in blocks.iter_mut() {
                    if let Some(p) = item.get_mut("photo") {
                        mutate(p);
                    } else if let Some(v) = item.get_mut("video") {
                        mutate(v);
                    } else {
                        mutate(item);
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InputRichMessage {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<RichBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_rtl: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_entity_detection: Option<bool>,
}

pub const RICH_MESSAGE_MAX_TEXT_CHARS: usize = 32_768;
pub const RICH_MESSAGE_MAX_BLOCKS: usize = 500;
pub const RICH_MESSAGE_MAX_NESTING: usize = 16;
pub const RICH_MESSAGE_MAX_MEDIA: usize = 50;
pub const RICH_MESSAGE_MAX_TABLE_COLUMNS: usize = 20;
pub const RICH_MESSAGE_MAX_BUTTONS_PER_ROW: usize = 8;

#[derive(Default)]
struct RichMessageStats {
    text_chars: usize,
    blocks: usize,
    max_depth: usize,
}

fn value_text_chars(value: &Value) -> usize {
    match value {
        Value::String(text) => text.chars().count(),
        Value::Array(values) => values.iter().map(value_text_chars).sum(),
        Value::Object(object) => object
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "type"
                        | "url"
                        | "callback_data"
                        | "web_app"
                        | "style"
                        | "align"
                        | "valign"
                        | "language"
                        | "name"
                        | "document"
                )
            })
            .map(|(_, value)| value_text_chars(value))
            .sum(),
        _ => 0,
    }
}

fn is_nested_block_type(kind: &str) -> bool {
    matches!(
        kind,
        "paragraph"
            | "heading"
            | "pre"
            | "footer"
            | "list"
            | "blockquote"
            | "expandable_blockquote"
            | "pullquote"
            | "divider"
            | "mathematical_expression"
            | "table"
            | "buttons"
            | "document"
            | "details"
            | "anchor"
            | "thinking"
            | "photo"
            | "video"
            | "audio"
            | "voice_note"
            | "animation"
            | "collage"
            | "slideshow"
            | "map"
    )
}

fn collect_nested_value_stats(value: &Value, depth: usize, stats: &mut RichMessageStats) {
    match value {
        Value::Array(values) => {
            for child in values {
                collect_nested_value_stats(child, depth, stats);
            }
        }
        Value::Object(object) => {
            let is_block = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(is_nested_block_type);
            let child_depth = if is_block { depth + 1 } else { depth };
            if is_block {
                stats.blocks += 1;
                stats.max_depth = stats.max_depth.max(child_depth);
            }
            for child in object.values() {
                collect_nested_value_stats(child, child_depth, stats);
            }
        }
        _ => {}
    }
}

impl InputRichMessage {
    pub fn new(blocks: Vec<RichBlock>) -> Self {
        Self {
            blocks,
            html: None,
            markdown: None,
            media: None,
            is_rtl: None,
            skip_entity_detection: None,
        }
    }

    pub fn has_media(&self) -> bool {
        self.blocks.iter().any(|b| b.is_media())
    }

    pub fn collect_media_urls(&self) -> Vec<String> {
        let mut urls = Vec::new();
        for block in &self.blocks {
            urls.extend(block.get_media_urls());
        }
        urls
    }

    pub fn replace_media_urls<F: Fn(&str) -> Option<String>>(&mut self, replacer: &F) {
        for block in &mut self.blocks {
            block.replace_media_urls(replacer);
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let representation_count = usize::from(!self.blocks.is_empty())
            + usize::from(self.html.is_some())
            + usize::from(self.markdown.is_some());
        if representation_count != 1 {
            return Err(format!(
                "InputRichMessage must contain exactly one of blocks, html, or markdown; found {representation_count}"
            ));
        }

        if self
            .media
            .as_ref()
            .is_some_and(|media| media.len() > RICH_MESSAGE_MAX_MEDIA)
        {
            return Err(format!(
                "Rich Message media count exceeds Telegram limit of {RICH_MESSAGE_MAX_MEDIA}"
            ));
        }

        let mut stats = RichMessageStats::default();
        if let Some(html) = &self.html {
            stats.text_chars = html.chars().count();
        } else if let Some(markdown) = &self.markdown {
            stats.text_chars = markdown.chars().count();
        }

        for block in &self.blocks {
            stats.blocks += 1;
            stats.max_depth = stats.max_depth.max(1);
            match block {
                RichBlock::Paragraph { text }
                | RichBlock::SectionHeading { text, .. }
                | RichBlock::Thinking { text }
                | RichBlock::Footer { text } => {
                    stats.text_chars += value_text_chars(text);
                    collect_nested_value_stats(text, 1, &mut stats);
                }
                RichBlock::Preformatted { text, .. } => stats.text_chars += text.chars().count(),
                RichBlock::List { items } => {
                    stats.blocks += items.len();
                    if !items.is_empty() {
                        stats.max_depth = stats.max_depth.max(2);
                    }
                    for item in items {
                        for value in &item.blocks {
                            stats.text_chars += value_text_chars(value);
                            collect_nested_value_stats(value, 2, &mut stats);
                        }
                    }
                }
                RichBlock::BlockQuotation { blocks } => {
                    for value in blocks {
                        stats.text_chars += value_text_chars(value);
                        collect_nested_value_stats(value, 1, &mut stats);
                    }
                }
                RichBlock::ExpandableBlockQuotation { text, credit }
                | RichBlock::PullQuotation { text, credit } => {
                    stats.text_chars += value_text_chars(text);
                    if let Some(credit) = credit {
                        stats.text_chars += value_text_chars(credit);
                    }
                }
                RichBlock::Divider {} | RichBlock::Anchor { .. } => {}
                RichBlock::MathematicalExpression { expression } => {
                    stats.text_chars += expression.chars().count();
                }
                RichBlock::Table { cells, caption, .. } => {
                    stats.blocks += cells.len();
                    if !cells.is_empty() {
                        stats.max_depth = stats.max_depth.max(2);
                    }
                    if cells
                        .iter()
                        .any(|row| row.len() > RICH_MESSAGE_MAX_TABLE_COLUMNS)
                    {
                        return Err(format!(
                            "Rich Message table exceeds Telegram limit of {RICH_MESSAGE_MAX_TABLE_COLUMNS} columns"
                        ));
                    }
                    for row in cells {
                        for cell in row {
                            stats.text_chars += value_text_chars(&cell.text);
                            collect_nested_value_stats(&cell.text, 2, &mut stats);
                        }
                    }
                    if let Some(caption) = caption {
                        stats.text_chars += caption.chars().count();
                    }
                }
                RichBlock::Buttons { buttons, .. } => {
                    if buttons.is_empty() || buttons.len() > RICH_MESSAGE_MAX_BUTTONS_PER_ROW {
                        return Err(format!(
                            "Rich Message button row must contain 1-{RICH_MESSAGE_MAX_BUTTONS_PER_ROW} buttons"
                        ));
                    }
                    for button in buttons {
                        button.validate()?;
                        stats.text_chars += value_text_chars(&button.text);
                    }
                }
                RichBlock::Document { caption, .. }
                | RichBlock::Photo { caption, .. }
                | RichBlock::Video { caption, .. }
                | RichBlock::Audio { caption, .. }
                | RichBlock::VoiceNote { caption, .. }
                | RichBlock::Animation { caption, .. } => {
                    if let Some(caption) = caption {
                        stats.text_chars += value_text_chars(&caption.text);
                        if let Some(credit) = &caption.credit {
                            stats.text_chars += value_text_chars(credit);
                        }
                    }
                }
                RichBlock::Collage { blocks, caption }
                | RichBlock::Slideshow { blocks, caption } => {
                    stats.blocks += blocks.len();
                    for val in blocks {
                        stats.text_chars += value_text_chars(val);
                        collect_nested_value_stats(val, 1, &mut stats);
                    }
                    if let Some(caption) = caption {
                        stats.text_chars += value_text_chars(&caption.text);
                        if let Some(credit) = &caption.credit {
                            stats.text_chars += value_text_chars(credit);
                        }
                    }
                }
                RichBlock::Map { .. } => {}
                RichBlock::Details {
                    summary, blocks, ..
                } => {
                    stats.text_chars += value_text_chars(summary);
                    for value in blocks {
                        stats.text_chars += value_text_chars(value);
                        collect_nested_value_stats(value, 1, &mut stats);
                    }
                }
            }
        }

        if stats.text_chars > RICH_MESSAGE_MAX_TEXT_CHARS {
            return Err(format!(
                "Rich Message text exceeds Telegram limit of {RICH_MESSAGE_MAX_TEXT_CHARS} characters"
            ));
        }
        if stats.blocks > RICH_MESSAGE_MAX_BLOCKS {
            return Err(format!(
                "Rich Message contains {} blocks; Telegram limit is {RICH_MESSAGE_MAX_BLOCKS}",
                stats.blocks
            ));
        }
        if stats.max_depth > RICH_MESSAGE_MAX_NESTING {
            return Err(format!(
                "Rich Message nesting depth {} exceeds Telegram limit of {RICH_MESSAGE_MAX_NESTING}",
                stats.max_depth
            ));
        }
        Ok(())
    }
}

pub fn deserialize_flexible_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct FlexibleI64Visitor;

    impl<'de> serde::de::Visitor<'de> for FlexibleI64Visitor {
        type Value = i64;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an integer or a string representing an integer")
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v)
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            i64::try_from(v).map_err(serde::de::Error::custom)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            v.trim().parse::<i64>().map_err(serde::de::Error::custom)
        }
    }

    deserializer.deserialize_any(FlexibleI64Visitor)
}

pub fn deserialize_flexible_opt_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct FlexibleOptI64Visitor;

    impl<'de> serde::de::Visitor<'de> for FlexibleOptI64Visitor {
        type Value = Option<i64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an optional integer or a string representing an integer")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserialize_flexible_i64(deserializer).map(Some)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }
    }

    deserializer.deserialize_option(FlexibleOptI64Visitor)
}

pub fn deserialize_flexible_i32<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct FlexibleI32Visitor;

    impl<'de> serde::de::Visitor<'de> for FlexibleI32Visitor {
        type Value = i32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an integer or a string representing an integer")
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            i32::try_from(v).map_err(serde::de::Error::custom)
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            i32::try_from(v).map_err(serde::de::Error::custom)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            v.trim().parse::<i32>().map_err(serde::de::Error::custom)
        }
    }

    deserializer.deserialize_any(FlexibleI32Visitor)
}

pub fn deserialize_flexible_opt_i32<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct FlexibleOptI32Visitor;

    impl<'de> serde::de::Visitor<'de> for FlexibleOptI32Visitor {
        type Value = Option<i32>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an optional integer or a string representing an integer")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserialize_flexible_i32(deserializer).map(Some)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }
    }

    deserializer.deserialize_option(FlexibleOptI32Visitor)
}

// ==========================================
// Telegram Updates & Message Payloads
// ==========================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub error_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Update {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub update_id: i64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
    pub stopped_message_generation: Option<MessageGenerationStopped>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageGenerationStopped {
    pub chat: Chat,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub message_thread_id: Option<i64>,
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub draft_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub date: i64,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub message_thread_id: Option<i64>,
    pub receiver_user: Option<User>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub ephemeral_message_id: Option<i64>,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub photo: Option<Vec<PhotoSize>>,
    pub document: Option<Document>,
    pub voice: Option<Voice>,
    pub audio: Option<Audio>,
    pub video: Option<Video>,
    pub video_note: Option<VideoNote>,
    pub reply_to_message: Option<Box<Message>>,
    pub community_chat_joined: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyParameters {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_flexible_opt_i64"
    )]
    pub message_id: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_flexible_opt_i64"
    )]
    pub ephemeral_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_sending_without_reply: Option<bool>,
}

impl ReplyParameters {
    pub fn new(message_id: i64) -> Self {
        Self {
            message_id: Some(message_id),
            ephemeral_message_id: None,
            allow_sending_without_reply: None,
        }
    }

    pub fn ephemeral(ephemeral_message_id: i64) -> Self {
        Self {
            message_id: None,
            ephemeral_message_id: Some(ephemeral_message_id),
            allow_sending_without_reply: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EphemeralMessageParameters {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub receiver_user_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_query_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace_callback_query_message: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub width: i32,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub height: i32,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub file_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Voice {
    pub file_id: String,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub duration: i32,
    pub mime_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Audio {
    pub file_id: String,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub duration: i32,
    pub performer: Option<String>,
    pub title: Option<String>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Video {
    pub file_id: String,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub width: i32,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub height: i32,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub duration: i32,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoNote {
    pub file_id: String,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub length: i32,
    #[serde(deserialize_with = "deserialize_flexible_i32")]
    pub duration: i32,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    pub message: Option<Message>,
    pub inline_message_id: Option<String>,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub file_id: String,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub file_size: Option<i64>,
    pub file_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_message_requires_exactly_one_representation() {
        let empty = InputRichMessage::default();
        assert!(empty.validate().is_err());

        let mut conflicting = InputRichMessage::new(vec![RichBlock::Paragraph {
            text: Value::String("hello".to_string()),
        }]);
        conflicting.markdown = Some("hello".to_string());
        assert!(conflicting.validate().is_err());

        let valid = InputRichMessage::new(vec![RichBlock::Paragraph {
            text: Value::String("hello".to_string()),
        }]);
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn rich_message_copy_text_button_validates_length_and_action() {
        let button = RichMessageButton::copy("Salin", "teks yang disalin");
        assert!(button.validate().is_ok());

        let value = serde_json::to_value(&button).unwrap();
        assert_eq!(value["copy_text"]["text"], "teks yang disalin");

        let inline_button = InlineKeyboardButton::copy("Salin Prompt", "prompt text");
        let inline_value = serde_json::to_value(&inline_button).unwrap();
        assert_eq!(inline_value["copy_text"]["text"], "prompt text");

        let mut invalid_button = RichMessageButton::copy("Salin", "a".repeat(257));
        assert!(invalid_button.validate().is_err());

        invalid_button.copy_text = Some(CopyTextButton::new(""));
        assert!(invalid_button.validate().is_err());
    }

    #[test]
    fn footer_and_pullquote_serialize_and_validate() {
        let message = InputRichMessage::new(vec![
            RichBlock::PullQuotation {
                text: Value::String("Kutipan penting".to_string()),
                credit: Some(Value::String("Penulis".to_string())),
            },
            RichBlock::Footer {
                text: Value::String("⚡ gpt-4o".to_string()),
            },
        ]);
        assert!(message.validate().is_ok());

        let serialized = serde_json::to_value(&message).unwrap();
        assert_eq!(serialized["blocks"][0]["type"], "pullquote");
        assert_eq!(serialized["blocks"][0]["text"], "Kutipan penting");
        assert_eq!(serialized["blocks"][1]["type"], "footer");
        assert_eq!(serialized["blocks"][1]["text"], "⚡ gpt-4o");
    }

    #[test]
    fn rich_message_button_requires_exactly_one_action() {
        let mut button = RichMessageButton::callback("Open", "open");
        assert!(button.validate().is_ok());

        button.url = Some("https://example.com".to_string());
        assert!(button.validate().is_err());

        button.callback_data = None;
        assert!(button.validate().is_ok());

        button.url = None;
        assert!(button.validate().is_err());
    }

    #[test]
    fn disabled_inline_button_serializes_as_empty_object() {
        let value = serde_json::to_value(InlineKeyboardButton::disabled("Unavailable")).unwrap();
        assert_eq!(value["disabled"], serde_json::json!({}));
        assert!(value.get("callback_data").is_none());
    }

    #[test]
    fn bot_api_10_3_stop_update_deserializes() {
        let update: Update = serde_json::from_value(serde_json::json!({
            "update_id": 42,
            "stopped_message_generation": {
                "chat": {"id": 7, "type": "private"},
                "draft_id": 99
            }
        }))
        .unwrap();
        let stopped = update.stopped_message_generation.unwrap();
        assert_eq!(stopped.chat.id, 7);
        assert_eq!(stopped.draft_id, 99);
    }

    #[test]
    fn rich_message_buttons_follow_10_3_shape() {
        let block = RichBlock::Buttons {
            buttons: vec![RichMessageButton::callback_styled(
                "Retry", "retry", "primary",
            )],
            align: Some("center".to_string()),
        };
        let value = serde_json::to_value(block).unwrap();
        assert_eq!(value["type"], "buttons");
        assert_eq!(value["buttons"][0]["callback_data"], "retry");
        assert_eq!(value["buttons"][0]["style"], "primary");
    }

    #[test]
    fn expandable_quote_follows_10_3_shape() {
        let block = RichBlock::ExpandableBlockQuotation {
            text: Value::String("detail".to_string()),
            credit: Some(Value::String("source".to_string())),
        };
        let value = serde_json::to_value(block).unwrap();
        assert_eq!(value["type"], "expandable_blockquote");
        assert_eq!(value["text"], "detail");
        assert_eq!(value["credit"], "source");
        assert!(value.get("blocks").is_none());
        assert!(value.get("is_open").is_none());
    }

    #[test]
    fn details_block_uses_summary_and_blocks() {
        let block = RichBlock::Details {
            summary: Value::String("More".to_string()),
            blocks: vec![serde_json::json!({"type": "paragraph", "text": "Body"})],
            is_open: Some(true),
        };
        let value = serde_json::to_value(block).unwrap();
        assert_eq!(value["summary"], "More");
        assert!(value["blocks"].is_array());
        assert_eq!(value["is_open"], true);
    }

    #[test]
    fn rich_message_text_limit_is_enforced_at_boundary() {
        let at_limit = InputRichMessage::new(vec![RichBlock::Paragraph {
            text: Value::String("x".repeat(RICH_MESSAGE_MAX_TEXT_CHARS)),
        }]);
        assert!(at_limit.validate().is_ok());
        let over = InputRichMessage::new(vec![RichBlock::Paragraph {
            text: Value::String("x".repeat(RICH_MESSAGE_MAX_TEXT_CHARS + 1)),
        }]);
        assert!(over.validate().is_err());
    }

    #[test]
    fn rich_message_block_limit_is_enforced_at_boundary() {
        let paragraph = || RichBlock::Paragraph {
            text: Value::String("x".to_string()),
        };
        let at_limit =
            InputRichMessage::new((0..RICH_MESSAGE_MAX_BLOCKS).map(|_| paragraph()).collect());
        assert!(at_limit.validate().is_ok());
        let over =
            InputRichMessage::new((0..=RICH_MESSAGE_MAX_BLOCKS).map(|_| paragraph()).collect());
        assert!(over.validate().is_err());
    }

    #[test]
    fn rich_message_media_table_and_button_limits_are_local() {
        let mut message = InputRichMessage::new(vec![RichBlock::Paragraph {
            text: Value::String("ok".to_string()),
        }]);
        message.media = Some(
            (0..RICH_MESSAGE_MAX_MEDIA)
                .map(|_| serde_json::json!({}))
                .collect(),
        );
        assert!(message.validate().is_ok());
        message.media.as_mut().unwrap().push(serde_json::json!({}));
        assert!(message.validate().is_err());

        let table = |columns: usize| {
            InputRichMessage::new(vec![RichBlock::Table {
                cells: vec![(0..columns)
                    .map(|_| RichBlockTableCell::text_only("x", false, None))
                    .collect()],
                has_header: false,
                is_bordered: true,
                is_striped: false,
                is_compact: true,
                caption: None,
            }])
        };
        assert!(table(RICH_MESSAGE_MAX_TABLE_COLUMNS).validate().is_ok());
        assert!(table(RICH_MESSAGE_MAX_TABLE_COLUMNS + 1)
            .validate()
            .is_err());

        let buttons = |count: usize| {
            InputRichMessage::new(vec![RichBlock::Buttons {
                buttons: (0..count)
                    .map(|index| {
                        RichMessageButton::callback(format!("b{index}"), format!("c{index}"))
                    })
                    .collect(),
                align: None,
            }])
        };
        assert!(buttons(RICH_MESSAGE_MAX_BUTTONS_PER_ROW).validate().is_ok());
        assert!(buttons(RICH_MESSAGE_MAX_BUTTONS_PER_ROW + 1)
            .validate()
            .is_err());
    }

    fn nested_details(depth: usize) -> Value {
        if depth == 0 {
            return serde_json::json!({"type": "paragraph", "text": "leaf"});
        }
        serde_json::json!({
            "type": "details",
            "summary": "nested",
            "blocks": [nested_details(depth - 1)]
        })
    }

    #[test]
    fn rich_media_blocks_serialize_and_validate() {
        let msg = InputRichMessage::new(vec![
            RichBlock::Photo {
                photo: serde_json::json!({"type": "photo", "media": "attach://photo1"}),
                caption: Some(RichBlockCaption::new(Value::String(
                    "Pemandangan".to_string(),
                ))),
            },
            RichBlock::Video {
                video: serde_json::json!({"type": "video", "media": "attach://video1"}),
                caption: None,
            },
            RichBlock::Map {
                location: Location {
                    latitude: -5.147665,
                    longitude: 119.432732,
                    horizontal_accuracy: Some(10.0),
                },
                zoom: Some(15),
                width: Some(600),
                height: Some(400),
            },
        ]);
        assert!(msg.validate().is_ok());

        let val = serde_json::to_value(&msg).unwrap();
        assert_eq!(val["blocks"][0]["type"], "photo");
        assert_eq!(val["blocks"][0]["caption"]["text"], "Pemandangan");
        assert_eq!(val["blocks"][1]["type"], "video");
        assert_eq!(val["blocks"][2]["type"], "map");
        assert_eq!(val["blocks"][2]["location"]["latitude"], -5.147665);
    }

    #[test]
    fn extended_button_actions_serialize_and_validate() {
        let mut btn = RichMessageButton::callback("Click", "data");
        assert!(btn.validate().is_ok());

        btn.callback_data = None;
        btn.login_url = Some(LoginUrl {
            url: "https://auth.example.com/login".to_string(),
            forward_text: Some("Log in".to_string()),
            bot_username: Some("xiao_bot".to_string()),
            request_write_access: Some(true),
        });
        assert!(btn.validate().is_ok());

        btn.login_url = None;
        btn.switch_inline_query_chosen_chat = Some(SwitchInlineQueryChosenChat {
            query: Some("search query".to_string()),
            allow_user_chats: Some(true),
            allow_bot_chats: Some(false),
            allow_group_chats: Some(true),
            allow_channel_chats: Some(false),
        });
        assert!(btn.validate().is_ok());

        let text_btn = RichTextButton {
            button: btn.clone(),
        };
        let text_val = serde_json::to_value(&text_btn).unwrap();
        assert!(text_val.get("button").is_some());
    }

    #[test]
    fn rich_message_nesting_limit_is_enforced() {
        // Top-level Details is depth 1, so fifteen nested block levels reaches 16.
        let at_limit = InputRichMessage::new(vec![RichBlock::Details {
            summary: Value::String("root".to_string()),
            blocks: vec![nested_details(RICH_MESSAGE_MAX_NESTING - 2)],
            is_open: None,
        }]);
        assert!(at_limit.validate().is_ok());
        let over = InputRichMessage::new(vec![RichBlock::Details {
            summary: Value::String("root".to_string()),
            blocks: vec![nested_details(RICH_MESSAGE_MAX_NESTING - 1)],
            is_open: None,
        }]);
        assert!(over.validate().is_err());
    }
}
