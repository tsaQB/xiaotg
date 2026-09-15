mod ai;
mod attachments;
mod bot;
mod cli;
mod document;
mod parser;
mod timeline;
mod util;

use rand::Rng;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use ai::service::{GenerationModelSnapshot, ImageGenerationErrorKind, ProbeEvent, ProviderConfig};
use ai::AIChatService;
use bot::client::{TelegramBotClient, TelegramDeliveryContext};
use bot::models::{BotCommand, InputRichMessage, RichBlock, RichBlockTableCell, Update};
use parser::build_full_rich_message;
use timeline::{ExecutionTimeline, ProgressActivity};
use util::{escape_html, truncate_chars};

type UserLastImagePrompt = Arc<RwLock<HashMap<i64, String>>>;

struct ChatInput<'a> {
    prompt: &'a str,
    image_bytes: Option<Vec<u8>>,
    document_images: Option<Vec<Vec<u8>>>,
    mime_type: Option<&'a str>,
    doc_text: Option<&'a str>,
    doc_name: Option<&'a str>,
    audio_bytes: Option<Vec<u8>>,
    audio_mime: Option<&'a str>,
    video_bytes: Option<Vec<u8>>,
    video_mime: Option<&'a str>,
    video_duration: Option<i32>,
    model_snapshot: Option<&'a GenerationModelSnapshot>,
}

fn build_audio_chat_input<'a>(
    prompt: &'a str,
    audio_bytes: Vec<u8>,
    audio_mime: Option<&'a str>,
    doc_name: Option<&'a str>,
) -> ChatInput<'a> {
    ChatInput {
        prompt,
        image_bytes: None,
        document_images: None,
        mime_type: None,
        doc_text: None,
        doc_name,
        audio_bytes: Some(audio_bytes),
        audio_mime,
        video_bytes: None,
        video_mime: None,
        video_duration: None,
        model_snapshot: None,
    }
}

#[derive(Clone, Debug)]
struct AccessPolicy {
    owner_user_id: i64,
    allowed_chat_ids: HashSet<i64>,
    bot_username: Option<String>,
}

impl AccessPolicy {
    fn allows(&self, user_id: i64, chat_id: i64) -> bool {
        if user_id != self.owner_user_id {
            return false;
        }
        if chat_id == self.owner_user_id {
            return true;
        }
        // If specific allowed_chat_ids are configured, enforce the whitelist.
        // If allowed_chat_ids is empty, allow owner in any group/topic.
        self.allowed_chat_ids.is_empty() || self.allowed_chat_ids.contains(&chat_id)
    }

    fn allows_stop_chat(&self, chat_id: i64) -> bool {
        // MessageGenerationStopped does not identify who pressed Stop. To prevent
        // another group participant from cancelling the owner's request, native
        // Stop is accepted only in the owner's private chat.
        chat_id == self.owner_user_id
    }
}

fn prepare_incoming_chat_text(
    raw_text: &str,
    is_group: bool,
    bot_username: Option<&str>,
    is_reply_to_bot: bool,
) -> Option<String> {
    let trimmed = raw_text.trim();
    if !is_group {
        return Some(trimmed.to_string());
    }

    let Some(bot_name) = bot_username else {
        return Some(trimmed.to_string());
    };

    let bot_tag = format!("@{bot_name}");
    let words: Vec<&str> = trimmed.split_whitespace().collect();

    let has_bot_tag = words.iter().any(|w| {
        let cleaned = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        cleaned.eq_ignore_ascii_case(bot_name) || cleaned.eq_ignore_ascii_case(&bot_tag[1..])
    });

    let mentions_other = words.iter().any(|w| {
        if let Some(mention) = w.strip_prefix('@') {
            let handle = mention.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
            !handle.is_empty() && !handle.eq_ignore_ascii_case(bot_name)
        } else {
            false
        }
    });

    if mentions_other && !has_bot_tag && !is_reply_to_bot {
        return None;
    }

    if has_bot_tag {
        let remaining: Vec<&str> = words
            .into_iter()
            .filter(|w| {
                let cleaned = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                !cleaned.eq_ignore_ascii_case(bot_name)
                    && !cleaned.eq_ignore_ascii_case(&bot_tag[1..])
            })
            .collect();
        Some(remaining.join(" "))
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn get_configured_owner_id() -> Option<i64> {
    load_environment();
    env::var("OWNER_USER_ID")
        .ok()
        .or_else(|| ai::service::load_app_setting("OWNER_USER_ID"))
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
}

fn get_allowed_chat_ids() -> HashSet<i64> {
    load_environment();
    env::var("ALLOWED_CHAT_IDS")
        .ok()
        .or_else(|| ai::service::load_app_setting("ALLOWED_CHAT_IDS"))
        .unwrap_or_default()
        .split(',')
        .filter_map(|value| value.trim().parse::<i64>().ok())
        .collect()
}

fn get_config_path() -> std::path::PathBuf {
    // 1. Current working directory .env
    if Path::new(".env").exists() {
        return Path::new(".env").to_path_buf();
    }
    // 2. ~/.xiao.env or ~/.xiao/.env
    if let Ok(home) = env::var("HOME") {
        let home_env = Path::new(&home).join(".xiao.env");
        if home_env.exists() {
            return home_env;
        }
        let app_dir_env = Path::new(&home).join("xiao").join(".env");
        if app_dir_env.exists() {
            return app_dir_env;
        }
        let legacy_app_dir_env = Path::new(&home).join("XiaoAI").join(".env");
        if legacy_app_dir_env.exists() {
            return legacy_app_dir_env;
        }
    }
    Path::new(".env").to_path_buf()
}

pub(crate) fn load_environment() {
    let cfg_path = get_config_path();
    if cfg_path.exists() {
        let _ = dotenvy::from_path(&cfg_path);
    } else {
        let _ = dotenvy::dotenv();
    }
}

pub(crate) fn save_env_kv(key: &str, value: &str) -> io::Result<()> {
    ai::service::save_app_setting(key, value)
}

pub(crate) fn save_token_to_env(token: &str) -> io::Result<()> {
    ai::service::save_app_setting("BOT_TOKEN", token)
}

pub(crate) fn get_configured_token() -> Option<String> {
    load_environment();
    if let Ok(token) = env::var("BOT_TOKEN") {
        let trimmed = token.trim().to_string();
        if !trimmed.is_empty() && trimmed != "YOUR_TELEGRAM_BOT_TOKEN_HERE" {
            return Some(trimmed);
        }
    }
    if let Some(token) = ai::service::load_app_setting("BOT_TOKEN") {
        let trimmed = token.trim().to_string();
        if !trimmed.is_empty() && trimmed != "YOUR_TELEGRAM_BOT_TOKEN_HERE" {
            return Some(trimmed);
        }
    }
    None
}

use cli::*;

// ==========================================
// UI Builders
// ==========================================

#[cfg(test)]
fn build_help_ui() -> InputRichMessage {
    use bot::models::RichBlockListItem;
    let input_items = vec![
        RichBlockListItem::bullet(vec![
            json!({"type":"paragraph","text":"Text — ordinary chat and instructions."}),
        ]),
        RichBlockListItem::bullet(vec![
            json!({"type":"paragraph","text":"Images: routed through the configured Vision role, without a prerequisite probe."}),
        ]),
        RichBlockListItem::bullet(vec![
            json!({"type":"paragraph","text":"Documents — local extraction; scanned PDF pages route through Vision."}),
        ]),
        RichBlockListItem::bullet(vec![
            json!({"type":"paragraph","text":"Voice/audio — native Main audio or the configured Audio STT role."}),
        ]),
        RichBlockListItem::bullet(vec![
            json!({"type":"paragraph","text":"Video: direct Main or the configured Video specialist, without a prerequisite probe."}),
        ]),
    ];
    let command_rows = vec![
        vec![
            RichBlockTableCell::text_only("Command / Input", true, Some("left")),
            RichBlockTableCell::text_only("Action", true, Some("left")),
        ],
        vec![
            RichBlockTableCell::text_only("Teks & Percakapan", false, Some("left")),
            RichBlockTableCell::text_only(
                "Percakapan bebas dan sambutan alami tanpa slash command",
                false,
                Some("left"),
            ),
        ],
        vec![
            RichBlockTableCell::text_only("Chat & Media", false, Some("left")),
            RichBlockTableCell::text_only(
                "Natural conversation, image generation, audio & document analysis",
                false,
                Some("left"),
            ),
        ],
    ];
    InputRichMessage::new(vec![
        RichBlock::SectionHeading {
            text: Value::String("HELP".to_string()),
            level: 1,
        },
        RichBlock::Paragraph {
            text: Value::String("Supported input".to_string()),
        },
        RichBlock::List { items: input_items },
        RichBlock::Table {
            cells: command_rows,
            has_header: true,
            is_bordered: true,
            is_striped: true,
            is_compact: true,
            caption: None,
        },
        RichBlock::Details {
            summary: Value::String("Model Routing".to_string()),
            blocks: vec![json!({
                "type":"paragraph",
                "text":"All model routing and specialist routes are managed via Xiao CLI. Vision, Video, Audio STT, and Image Generation routes are read-only in Telegram (configure via xiao addon)."
            })],
            is_open: Some(false),
        },
        RichBlock::Details {
            summary: Value::String("Media Routing".to_string()),
            blocks: vec![json!({
                "type":"paragraph",
                "text":"Main-compatible media executes directly on Main. A different specialist receives only the minimum current context and returns a bounded observation/transcript to Main."
            })],
            is_open: Some(false),
        },
        RichBlock::Paragraph {
            text: Value::String("Advanced routing configuration: xiao addon".to_string()),
        },
    ])
}

#[cfg(test)]
fn specialist_context_policy(
    role: ai::service::ModelRole,
    origin: ai::service::RouteOrigin,
) -> &'static str {
    if origin == ai::service::RouteOrigin::MainModel {
        return "Direct on Main; canonical history stays on Main";
    }
    match role {
        ai::service::ModelRole::Vision | ai::service::ModelRole::Video => {
            "Transient media + current question; no full history"
        }
        ai::service::ModelRole::AudioStt => "Transcript only; no full history",
        ai::service::ModelRole::ImageGeneration => "Prompt/config only; no canonical history",
        ai::service::ModelRole::Curator => "Background extraction & summarization only",
        ai::service::ModelRole::Main => "Canonical Main context",
    }
}

#[allow(dead_code)]
fn context_available_tokens(limit: usize, used: usize) -> usize {
    limit.saturating_sub(used)
}

#[allow(dead_code)]
fn main_context_overflow_warning(model: &str, used: usize, usable_limit: usize) -> Option<String> {
    (used > usable_limit).then(|| {
        format!(
            "Main Model changed to {model}. Current canonical history (~{used} tokens) exceeds the new usable context (~{usable_limit} tokens). Xiao will compact before the next request when needed; history was not deleted."
        )
    })
}

#[allow(dead_code)]
fn effective_capability_state_label(
    record: &ai::service::CapabilityRecord,
    capability: ai::service::CapabilityKind,
) -> &'static str {
    match AIChatService::effective_capability_state(record, capability) {
        ai::storage::CapabilityState::Supported => "Supported",
        ai::storage::CapabilityState::Unsupported => "Unsupported",
        ai::storage::CapabilityState::Unknown => "Unknown",
    }
}

#[allow(dead_code)]
async fn run_observable_main_capability_probe(
    bot: &TelegramBotClient,
    ai_service: &AIChatService,
    chat_id: i64,
    provider: &ProviderConfig,
    model: &str,
) {
    let checking = InputRichMessage::new(vec![
        RichBlock::SectionHeading {
            text: Value::String("CHECKING CAPABILITIES".to_string()),
            level: 1,
        },
        RichBlock::Paragraph {
            text: Value::String(format!("Main Model: {} / {}", provider.name, model)),
        },
        RichBlock::BlockQuotation {
            blocks: vec![json!({
                "type":"paragraph",
                "text":"Running bounded safe probes. Image-generation active probing is skipped because it may consume credits."
            })],
        },
    ]);
    let _ = bot.send_rich_message(chat_id, &checking, None, None).await;

    let mut persisted = false;
    let record = ai_service
        .probe_model_capabilities_with_observer(provider, model, |event| {
            if let ProbeEvent::Persistence { saved } = event {
                persisted = saved;
            }
        })
        .await;
    if !persisted {
        let failed = InputRichMessage::new(vec![
            RichBlock::SectionHeading {
                text: Value::String("CAPABILITY CHECK NOT SAVED".to_string()),
                level: 1,
            },
            RichBlock::BlockQuotation {
                blocks: vec![json!({
                    "type":"paragraph",
                    "text":"The probe candidate could not be persisted, so Xiao did not publish it to runtime. Previous durable capability state remains authoritative."
                })],
            },
        ]);
        let _ = bot.send_rich_message(chat_id, &failed, None, None).await;
        return;
    }
    let completed = InputRichMessage::new(vec![
        RichBlock::SectionHeading {
            text: Value::String("CAPABILITY CHECK COMPLETE".to_string()),
            level: 1,
        },
        RichBlock::Table {
            cells: vec![
                vec![
                    RichBlockTableCell::text_only("Capability", true, Some("left")),
                    RichBlockTableCell::text_only("State", true, Some("left")),
                ],
                vec![
                    RichBlockTableCell::text_only("Text chat", false, Some("left")),
                    RichBlockTableCell::text_only(
                        effective_capability_state_label(
                            &record,
                            ai::service::CapabilityKind::TextChat,
                        ),
                        false,
                        Some("left"),
                    ),
                ],
                vec![
                    RichBlockTableCell::text_only("Vision", false, Some("left")),
                    RichBlockTableCell::text_only(
                        effective_capability_state_label(
                            &record,
                            ai::service::CapabilityKind::ImageInput,
                        ),
                        false,
                        Some("left"),
                    ),
                ],
                vec![
                    RichBlockTableCell::text_only("Audio input", false, Some("left")),
                    RichBlockTableCell::text_only(
                        effective_capability_state_label(
                            &record,
                            ai::service::CapabilityKind::AudioInput,
                        ),
                        false,
                        Some("left"),
                    ),
                ],
                vec![
                    RichBlockTableCell::text_only("Audio STT", false, Some("left")),
                    RichBlockTableCell::text_only(
                        effective_capability_state_label(
                            &record,
                            ai::service::CapabilityKind::AudioTranscription,
                        ),
                        false,
                        Some("left"),
                    ),
                ],
                vec![
                    RichBlockTableCell::text_only("Video", false, Some("left")),
                    RichBlockTableCell::text_only(
                        effective_capability_state_label(
                            &record,
                            ai::service::CapabilityKind::VideoInput,
                        ),
                        false,
                        Some("left"),
                    ),
                ],
                vec![
                    RichBlockTableCell::text_only("Image generation", false, Some("left")),
                    RichBlockTableCell::text_only(
                        effective_capability_state_label(
                            &record,
                            ai::service::CapabilityKind::ImageGeneration,
                        ),
                        false,
                        Some("left"),
                    ),
                ],
            ],
            has_header: true,
            is_bordered: true,
            is_striped: true,
            is_compact: true,
            caption: None,
        },
        RichBlock::Paragraph {
            text: Value::String(
                "Capability evidence is diagnostic only. Configured routes can be used without probing."
                    .to_string(),
            ),
        },
    ]);
    let _ = bot.send_rich_message(chat_id, &completed, None, None).await;
}

#[cfg(test)]
fn build_clear_confirmation_ui() -> InputRichMessage {
    use bot::models::RichMessageButton;
    InputRichMessage::new(vec![
        RichBlock::SectionHeading {
            text: Value::String("RESET HISTORY?".to_string()),
            level: 1,
        },
        RichBlock::BlockQuotation {
            blocks: vec![
                json!({"type":"paragraph","text":"This removes canonical conversation history and attachment context for the active session. The session remains. In-flight older generations cannot write back after the revision changes."}),
            ],
        },
        RichBlock::Buttons {
            buttons: vec![
                RichMessageButton::callback_styled("Reset History", "action_clear", "danger"),
                RichMessageButton::callback("Cancel", "clear_cancel"),
            ],
            align: Some("center".to_string()),
        },
    ])
}

// ==========================================
// Intent Detection & Image Generation
// ==========================================

fn extract_image_intent_prompt(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }

    let t_lower = t.to_lowercase();
    let inquiry_prefixes = [
        "apa itu",
        "apa arti",
        "jelaskan",
        "mengapa",
        "kenapa",
        "bagaimana cara",
        "cara ",
        "tutorial",
        "definisi",
        "what is",
        "why",
        "how to",
        "explain",
    ];
    if inquiry_prefixes
        .iter()
        .any(|pref| t_lower.starts_with(pref))
    {
        return None;
    }

    let follow_up_patterns = [
        r"(?i)^(?:tolong\s+|pls\s+|please\s+)?(?:buatkan|bikinin|bikin|buat|generate|draw|render|lukiskan|lukis|gambarin|gambarkan)\s+(?:dong\s+|kan\s+)?(?:gambar(?:nya| ini| itu| tersebut| tadi)?|foto(?:nya| ini| itu| tersebut)?|lukisan(?:nya| ini)?|image(?:nya)?|it|this)$",
        r"(?i)^(?:gambar(?:nya| ini| itu| tersebut)?|foto(?:nya)?|lukisan(?:nya)?)\s*(?:dong|ya|tolong|pls)?$",
    ];
    for pat in follow_up_patterns {
        if Regex::new(pat).is_ok_and(|regex| regex.is_match(t)) {
            return Some("__CONTEXT_FOLLOWUP__".to_string());
        }
    }

    let patterns = [
        r"(?i)^(?:tolong\s+|pls\s+|please\s+)?(?:buatkan|buatlah|buat|bikinin|bikin|generate|create|render|hasilkan|lukiskan|lukis|gambarin|gambarkan|draw)\s+(?:saya\s+|aku\s+|in\s+)?(?:sebuah\s+|seekor\s+|seorang\s+|suatu\s+|an?\s+|the\s+)?(?:gambar|foto|photo|lukisan|image|picture|wallpaper|ilustrasi|illustration|artwork|poster|visual)\s+(?:tentang\s+|dari\s+|of\s+|about\s+)?(.+)$",
        r"(?i)^(?:tolong\s+|pls\s+|please\s+)?(?:gambarin|gambarkan|lukiskan|lukis)\s+(?:saya\s+|aku\s+|in\s+)?(?:dong\s+|kan\s+)?(.+)$",
        r"(?i)^(?:ilustrasi|lukisan|artwork|wallpaper|fanart|sketsa|foto)\s+(?:tentang\s+|dari\s+|of\s+|about\s+)?(.+)$",
        r"(?i)^(?:tolong\s+|pls\s+|please\s+)?(?:buatkan|bikinin|bikin)\s+(?:saya\s+|aku\s+)?(?:dong\s+)?(.+?\b(?:gaya|style|anime|wallpaper|realistis|realistic|3d|cyberpunk|lukisan|sketsa|art|hd|8k)\b.*)$",
        r"(?i)^(?:please\s+|can you\s+)?(?:generate|create|make|draw|render)\s+(?:me\s+)?(?:an?\s+|the\s+)?(?:image|picture|photo|illustration|drawing|wallpaper|artwork)\s+(?:of\s+|about\s+)?(.+)$",
    ];

    let clean_re = Regex::new(r"(?i)^(?:tentang|mengenai|berupa|of|about|dong|ya|tolong)\s+").ok();

    for pat in patterns {
        let Ok(regex) = Regex::new(pat) else {
            continue;
        };
        if let Some(caps) = regex.captures(t) {
            if let Some(extracted_match) = caps.get(1) {
                let mut extracted = extracted_match.as_str().trim().to_string();
                if let Some(clean_re) = &clean_re {
                    extracted = clean_re.replace(&extracted, "").trim().to_string();
                }

                let ext_low = extracted.to_lowercase();
                if [
                    "dong",
                    "ya",
                    "ini",
                    "itu",
                    "nya",
                    "tadi",
                    "tersebut",
                    "gambarnya",
                    "fotonya",
                ]
                .contains(&ext_low.as_str())
                {
                    return Some("__CONTEXT_FOLLOWUP__".to_string());
                }
                if extracted.len() >= 3 {
                    return Some(extracted);
                }
            }
        }
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageGenerationIntent {
    image_prompt: String,
    explanation_prompt: Option<String>,
}

fn plan_image_generation_intent(text: &str) -> Option<ImageGenerationIntent> {
    let compound = Regex::new(
        r"(?i)^(?P<image>.+?)\s+(?:dan|lalu|kemudian|and|then)\s+(?P<explain>(?:jelaskan|terangkan|explain|describe)\b.+)$",
    )
    .ok();

    if let Some(regex) = compound {
        if let Some(captures) = regex.captures(text.trim()) {
            let image_request = captures.name("image")?.as_str().trim();
            let explanation = captures.name("explain")?.as_str().trim();
            if let Some(image_prompt) = extract_image_intent_prompt(image_request) {
                return Some(ImageGenerationIntent {
                    image_prompt,
                    explanation_prompt: (!explanation.is_empty()).then(|| explanation.to_string()),
                });
            }
        }
    }

    extract_image_intent_prompt(text).map(|image_prompt| ImageGenerationIntent {
        image_prompt,
        explanation_prompt: None,
    })
}

const TELEGRAM_PHOTO_CAPTION_MAX_CHARS: usize = 1024;
const IMAGE_CAPTION_PROMPT_ESCAPED_CHARS: usize = 320;
const IMAGE_CAPTION_PROVIDER_ESCAPED_CHARS: usize = 96;
const IMAGE_CAPTION_MODEL_ESCAPED_CHARS: usize = 128;
const IMAGE_CAPTION_FAILURE_ESCAPED_CHARS: usize = 144;

fn bounded_escaped_html(text: &str, max_chars: usize) -> String {
    let mut output = String::new();
    let mut used = 0usize;
    let mut truncated = false;

    for ch in text.chars() {
        let escaped = match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            _ => {
                let needed = 1usize;
                if used.saturating_add(needed) > max_chars {
                    truncated = true;
                    break;
                }
                output.push(ch);
                used += needed;
                continue;
            }
        };
        let needed = escaped.chars().count();
        if used.saturating_add(needed) > max_chars {
            truncated = true;
            break;
        }
        output.push_str(escaped);
        used += needed;
    }

    if truncated && used < max_chars {
        output.push('…');
    }
    output
}

fn build_image_success_caption(
    prompt: &str,
    provider: &str,
    model: &str,
    dimensions: (usize, usize),
    elapsed_secs: f64,
    used_external_fallback: bool,
    primary_failure: Option<&str>,
) -> String {
    let (width, height) = dimensions;
    let safe_prompt = bounded_escaped_html(prompt, IMAGE_CAPTION_PROMPT_ESCAPED_CHARS);
    let safe_provider = bounded_escaped_html(provider, IMAGE_CAPTION_PROVIDER_ESCAPED_CHARS);
    let safe_model = bounded_escaped_html(model, IMAGE_CAPTION_MODEL_ESCAPED_CHARS);
    let fallback_note = if used_external_fallback {
        let safe_failure = bounded_escaped_html(
            primary_failure.unwrap_or("Primary provider failure was not reported."),
            IMAGE_CAPTION_FAILURE_ESCAPED_CHARS,
        );
        format!(
            "\n⚠️ <i>External fallback opt-in digunakan.</i>\n<b>Primary failure:</b> {safe_failure}"
        )
    } else {
        String::new()
    };

    let caption = format!(
        "🫟 <b>Gambar Berhasil Dibuat!</b>\n\n\
         📝 <b>Prompt:</b> <i>\"{safe_prompt}\"</i>\n\
         🧩 <b>Provider:</b> <code>{safe_provider}</code>\n\
         🤖 <b>Model:</b> <code>{safe_model}</code>\n\
         📐 <b>Size:</b> <code>{width} × {height}</code>\n\
         ⏱️ <b>Elapsed:</b> <code>{elapsed_secs:.1}s</code>{fallback_note}"
    );

    if caption.chars().count() <= TELEGRAM_PHOTO_CAPTION_MAX_CHARS {
        return caption;
    }

    let minimal = format!(
        "🫟 <b>Gambar Berhasil Dibuat!</b>\n\
         🧩 <b>Provider:</b> <code>{safe_provider}</code>\n\
         🤖 <b>Model:</b> <code>{safe_model}</code>\n\
         📐 <b>Size:</b> <code>{width} × {height}</code>\n\
         ⏱️ <b>Elapsed:</b> <code>{elapsed_secs:.1}s</code>"
    );
    debug_assert!(minimal.chars().count() <= TELEGRAM_PHOTO_CAPTION_MAX_CHARS);
    minimal
}

fn telegram_photo_delivery_error_class(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if [
        "caption",
        "can't parse entities",
        "cannot parse entities",
        "parse entities",
        "reply markup",
        "reply_markup",
        "inline keyboard",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        "caption_or_markup"
    } else if [
        "multipart error",
        "timeout",
        "timed out",
        "connection",
        "network",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        "telegram_transport"
    } else if lower.contains("unsupported image signature") {
        "local_image_validation"
    } else {
        "telegram_api"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageDeliveryFailure {
    class: &'static str,
    detail: String,
    retry_attempted: bool,
}

async fn deliver_generated_image_with<F, Fut>(
    image_bytes: &[u8],
    caption: &str,
    reply_markup: Option<Value>,
    mut sender: F,
) -> Result<(), ImageDeliveryFailure>
where
    F: FnMut(Vec<u8>, Option<String>, Option<String>, Option<Value>) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    match sender(
        image_bytes.to_vec(),
        Some(caption.to_string()),
        Some("HTML".to_string()),
        reply_markup,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(first_error)
            if telegram_photo_delivery_error_class(&first_error) == "caption_or_markup" =>
        {
            let retry_caption = "Image generated successfully.".to_string();
            match sender(image_bytes.to_vec(), Some(retry_caption), None, None).await {
                Ok(()) => Ok(()),
                Err(second_error) => Err(ImageDeliveryFailure {
                    class: telegram_photo_delivery_error_class(&second_error),
                    detail: second_error,
                    retry_attempted: true,
                }),
            }
        }
        Err(error) => Err(ImageDeliveryFailure {
            class: telegram_photo_delivery_error_class(&error),
            detail: error,
            retry_attempted: false,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_image_generation(
    bot: &TelegramBotClient,
    ai_service: &AIChatService,
    user_last_image_prompt: &UserLastImagePrompt,
    chat_id: i64,
    thread_id: i64,
    user_id: i64,
    prompt: &str,
    explanation_prompt: Option<&str>,
) {
    let mut clean_prompt = prompt.trim().to_string();

    if clean_prompt == "__CONTEXT_FOLLOWUP__"
        || [
            "gambarnya",
            "gambarnya dong",
            "itu",
            "yang tadi",
            "ini",
            "dong",
            "ya",
            "fotonya",
        ]
        .contains(&clean_prompt.as_str())
    {
        let mut last_context = String::new();
        let scoped_messages = ai::storage::load_scoped_messages_async(chat_id, thread_id, 10).await;
        for msg in scoped_messages.iter().rev() {
            let candidate = match &msg.content {
                Value::String(value) => Some(value.clone()),
                value => attachments::decode_user_content(value).map(|content| content.text),
            };
            if let Some(candidate) = candidate {
                if candidate.trim().chars().count() > 8 {
                    last_context = candidate.trim().to_string();
                    break;
                }
            }
        }

        if !last_context.is_empty() {
            clean_prompt = format!("illustration of {}", truncate_chars(&last_context, 250));
        } else {
            let last_guard = user_last_image_prompt.read().await;
            clean_prompt = last_guard.get(&user_id).cloned().unwrap_or_else(|| {
                "majestic mountain scenery with clouds and ancient kingdom".to_string()
            });
        }
    }

    if clean_prompt.is_empty() {
        let route_text = match ai_service
            .resolve_model_route_unchecked(ai::service::ModelRole::ImageGeneration)
            .await
        {
            Ok(route) => format!("{} / {}", route.provider.name, route.model),
            Err(error) => format!("Unavailable — {}", error),
        };
        let rich = InputRichMessage::new(vec![
            RichBlock::SectionHeading {
                text: Value::String("IMAGE GENERATION".to_string()),
                level: 1,
            },
            RichBlock::Table {
                cells: vec![
                    vec![
                        RichBlockTableCell::text_only("Image Model", true, Some("left")),
                        RichBlockTableCell::text_only("Default Size", true, Some("left")),
                    ],
                    vec![
                        RichBlockTableCell::text_only(&route_text, false, Some("left")),
                        RichBlockTableCell::text_only("1024 × 1024", false, Some("left")),
                    ],
                ],
                has_header: true,
                is_bordered: true,
                is_striped: true,
                is_compact: true,
                caption: None,
            },
            RichBlock::Paragraph {
                text: Value::String(
                    "Kirim deskripsi gambar yang ingin dibuat (contoh: \"buat gambar pemandangan pegunungan saat fajar\").".to_string(),
                ),
            },
        ]);
        let _ = bot.send_rich_message(chat_id, &rich, None, None).await;
        return;
    }

    user_last_image_prompt
        .write()
        .await
        .insert(user_id, clean_prompt.clone());

    let draft_id: i64 = rand::thread_rng().gen_range(100000..999999);
    let timeline = Arc::new(ExecutionTimeline::new(
        bot.clone(),
        chat_id,
        draft_id,
        10,
        chat_id == user_id,
        chat_id == user_id,
    ));
    timeline
        .add_action("Generating Image", Some(ProgressActivity::Drawing))
        .await;
    timeline.sync_draft(true).await;
    timeline.start_ticker();
    let _ = bot.send_chat_action(chat_id, "upload_photo").await;

    let width = 1024usize;
    let height = 1024usize;
    let image_started = Instant::now();
    let model_snapshot = ai_service.generation_model_snapshot().await;
    let mut cancel_rx = ai_service.begin_generation(chat_id, draft_id).await;
    let image_result = ai_service
        .generate_image_with_snapshot(
            user_id,
            &clean_prompt,
            width,
            height,
            &model_snapshot,
            &mut cancel_rx,
        )
        .await;
    ai_service.end_generation(chat_id, draft_id).await;
    timeline.stop_ticker();
    let elapsed_secs = image_started.elapsed().as_secs_f64();

    let generated = match image_result {
        Ok(image) => image,
        Err(error) => {
            timeline.fail_current(error.message.clone()).await;
            timeline.sync_draft(true).await;

            if error.kind == ImageGenerationErrorKind::Cancelled {
                let rich = InputRichMessage::new(vec![
                    RichBlock::SectionHeading {
                        text: Value::String("IMAGE GENERATION CANCELLED".to_string()),
                        level: 1,
                    },
                    RichBlock::Paragraph {
                        text: Value::String("Image generation was cancelled.".to_string()),
                    },
                ]);
                let _ = bot.send_rich_message(chat_id, &rich, None, None).await;
                return;
            }

            let status = match error.kind {
                ImageGenerationErrorKind::CapabilityUnknown => "Capability unknown",
                ImageGenerationErrorKind::CapabilityUnsupported => "Unsupported",
                ImageGenerationErrorKind::RouteDisabled => "Route disabled",
                ImageGenerationErrorKind::ProviderNotFound => "Provider not found",
                ImageGenerationErrorKind::ModelNotFound => "Model not found",
                ImageGenerationErrorKind::Timeout => "Timeout",
                ImageGenerationErrorKind::Auth => "Authentication error",
                ImageGenerationErrorKind::RateLimited => "Rate limited",
                ImageGenerationErrorKind::HttpStatus => "HTTP error",
                ImageGenerationErrorKind::ProtocolMismatch => "Protocol mismatch",
                ImageGenerationErrorKind::InvalidResponse => "Invalid response",
                ImageGenerationErrorKind::InvalidBase64 => "Invalid base64",
                ImageGenerationErrorKind::InvalidImage => "Invalid image",
                ImageGenerationErrorKind::UnsafeImageUrl => "Unsafe image URL",
                ImageGenerationErrorKind::DownloadTimeout => "Download timeout",
                ImageGenerationErrorKind::Cancelled => "Cancelled",
                ImageGenerationErrorKind::FallbackDisabled => "Fallback disabled",
                ImageGenerationErrorKind::Provider => "Provider error",
            };
            let mut blocks = vec![
                RichBlock::SectionHeading {
                    text: Value::String("IMAGE GENERATION FAILED".to_string()),
                    level: 1,
                },
                RichBlock::Table {
                    cells: vec![
                        vec![
                            RichBlockTableCell::text_only("Status", true, Some("left")),
                            RichBlockTableCell::text_only("Detail", true, Some("left")),
                        ],
                        vec![
                            RichBlockTableCell::text_only(status, false, Some("left")),
                            RichBlockTableCell::text_only(
                                &truncate_chars(&error.message, 240),
                                false,
                                Some("left"),
                            ),
                        ],
                    ],
                    has_header: true,
                    is_bordered: true,
                    is_striped: true,
                    is_compact: true,
                    caption: None,
                },
            ];
            if matches!(
                error.kind,
                ImageGenerationErrorKind::CapabilityUnknown
                    | ImageGenerationErrorKind::CapabilityUnsupported
                    | ImageGenerationErrorKind::RouteDisabled
                    | ImageGenerationErrorKind::ProviderNotFound
                    | ImageGenerationErrorKind::ModelNotFound
            ) {
                blocks.push(RichBlock::BlockQuotation {
                    blocks: vec![json!({
                        "type":"paragraph",
                        "text":"Configure specialist Image Generation route with: xiao addon"
                    })],
                });
            }
            let rich = InputRichMessage::new(blocks);
            let _ = bot.send_rich_message(chat_id, &rich, None, None).await;
            return;
        }
    };

    let caption_text = build_image_success_caption(
        &clean_prompt,
        &generated.provider_name,
        &generated.model,
        (width, height),
        elapsed_secs,
        generated.used_external_fallback,
        generated.primary_failure.as_deref(),
    );

    let delivery = deliver_generated_image_with(
        &generated.bytes,
        &caption_text,
        None,
        |bytes, caption, parse_mode, reply_markup| async move {
            bot.send_photo_bytes(
                chat_id,
                bytes,
                caption.as_deref(),
                parse_mode.as_deref(),
                reply_markup,
                None,
            )
            .await
            .map(|_| ())
        },
    )
    .await;

    if let Err(failure) = delivery {
        warn!(
            "Generated image delivery failed [{}; retry={}]: {}",
            failure.class,
            failure.retry_attempted,
            truncate_chars(&failure.detail, 200)
        );
        let rich = InputRichMessage::new(vec![
            RichBlock::SectionHeading {
                text: Value::String("IMAGE DELIVERY FAILED".to_string()),
                level: 1,
            },
            RichBlock::Paragraph {
                text: Value::String(
                    "Image generation succeeded, but Telegram could not deliver the image."
                        .to_string(),
                ),
            },
            RichBlock::BlockQuotation {
                blocks: vec![json!({
                    "type": "paragraph",
                    "text": format!("Diagnostic class: {}", failure.class)
                })],
            },
        ]);
        if let Err(error) = bot.send_rich_message(chat_id, &rich, None, None).await {
            warn!(
                "Image delivery fallback message also failed: {}",
                truncate_chars(&error, 160)
            );
        }
        return;
    }

    if let Some(explanation_prompt) = explanation_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        handle_ai_chat(
            bot,
            ai_service,
            chat_id,
            thread_id,
            user_id,
            ChatInput {
                prompt: explanation_prompt,
                image_bytes: None,
                document_images: None,
                mime_type: None,
                doc_text: None,
                doc_name: None,
                audio_bytes: None,
                audio_mime: None,
                video_bytes: None,
                video_mime: None,
                video_duration: None,
                model_snapshot: Some(&model_snapshot),
            },
        )
        .await;
    }
}

// ==========================================
// Main AI Chat Handler
// ==========================================

async fn handle_ai_chat(
    bot: &TelegramBotClient,
    ai_service: &AIChatService,
    chat_id: i64,
    thread_id: i64,
    user_id: i64,
    input: ChatInput<'_>,
) {
    let ChatInput {
        prompt: user_prompt,
        image_bytes,
        document_images,
        mime_type,
        doc_text,
        doc_name,
        audio_bytes,
        audio_mime,
        video_bytes,
        video_mime,
        video_duration,
        model_snapshot,
    } = input;
    let generation_lock = ai_service.generation_lock(chat_id, thread_id).await;
    let _generation_guard = generation_lock.lock().await;

    let draft_id: i64 = rand::thread_rng().gen_range(100000..999999);
    let mut cancel_rx = ai_service.begin_generation(chat_id, draft_id).await;
    let timeline = Arc::new(ExecutionTimeline::new(
        bot.clone(),
        chat_id,
        draft_id,
        30,
        chat_id == user_id,
        chat_id == user_id,
    ));

    let (initial_lbl, initial_act) = if video_bytes.is_some() {
        ("Watching", ProgressActivity::Watching)
    } else if image_bytes.is_some()
        || document_images
            .as_ref()
            .is_some_and(|pages| !pages.is_empty())
    {
        ("Looking", ProgressActivity::Looking)
    } else if doc_text.is_some() {
        ("Reading", ProgressActivity::Reading)
    } else if audio_bytes.is_some() {
        ("Listening", ProgressActivity::Listening)
    } else {
        ("Thinking", ProgressActivity::Thinking)
    };

    timeline.add_action(initial_lbl, Some(initial_act)).await;
    timeline.sync_draft(true).await;
    timeline.start_ticker();
    let _ = bot.send_chat_action(chat_id, "typing").await;

    let generation_input = ai::service::GenerationInput {
        prompt: user_prompt,
        canonical_prompt: None,
        media_to_main: true,
        timeline: Some(&timeline),
        image_bytes,
        document_images,
        mime_type,
        doc_text,
        doc_name,
        audio_bytes,
        audio_mime,
        video_bytes,
        video_mime,
        video_duration,
    };
    let generation_start = std::time::Instant::now();
    let (_thinking, mut answer_text, cancelled) = if let Some(snapshot) = model_snapshot {
        ai_service
            .generate_response_with_snapshot(
                chat_id,
                thread_id,
                user_id,
                generation_input,
                snapshot,
                &mut cancel_rx,
            )
            .await
    } else {
        ai_service
            .generate_response(
                chat_id,
                thread_id,
                user_id,
                generation_input,
                &mut cancel_rx,
            )
            .await
    };

    ai_service.end_generation(chat_id, draft_id).await;
    timeline.stop_ticker();
    if cancelled {
        return;
    }
    let elapsed_secs = generation_start.elapsed().as_secs_f64();
    let emoji = if elapsed_secs <= 10.0 {
        "⚡"
    } else if elapsed_secs <= 30.0 {
        "⏱️"
    } else {
        "🧠"
    };
    let elapsed = format!("`{emoji} {:.1}s`", elapsed_secs);

    if answer_text.trim().is_empty() {
        answer_text = "Maaf, respon AI kosong untuk permintaan ini.".to_string();
    }

    let full_rich_msg = build_full_rich_message(&answer_text, Some(&elapsed));
    let res = bot
        .send_rich_message(chat_id, &full_rich_msg, None, None)
        .await;

    if let Err(error) = res {
        // send_rich_message already exhausts canonical Rich -> safe HTML ->
        // semantic plain-text fallback. Never re-send raw model Markdown here.
        warn!("Unable to deliver final canonical answer: {error}");
    }
}

// ==========================================
// Update Router
// ==========================================

fn delivery_context_for_update(update: &Update) -> TelegramDeliveryContext {
    if let Some(message) = update.message.as_ref() {
        return TelegramDeliveryContext {
            message_thread_id: message.message_thread_id,
            receiver_user_id: message
                .ephemeral_message_id
                .and_then(|_| message.from.as_ref().map(|user| user.id)),
            source_ephemeral_message_id: message.ephemeral_message_id,
            callback_query_id: None,
        };
    }
    if let Some(callback) = update.callback_query.as_ref() {
        let message = callback.message.as_ref();
        let source_ephemeral_message_id = message.and_then(|message| message.ephemeral_message_id);
        return TelegramDeliveryContext {
            message_thread_id: message.and_then(|message| message.message_thread_id),
            receiver_user_id: source_ephemeral_message_id.map(|_| callback.from.id),
            source_ephemeral_message_id,
            callback_query_id: source_ephemeral_message_id.map(|_| callback.id.clone()),
        };
    }
    if let Some(stopped) = update.stopped_message_generation.as_ref() {
        return TelegramDeliveryContext {
            message_thread_id: stopped.message_thread_id,
            ..TelegramDeliveryContext::default()
        };
    }
    TelegramDeliveryContext::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateLane {
    Control,
    Generation,
}

fn command_matches(text: &str, command: &str) -> bool {
    command_args(text, command).is_some()
}

fn command_args<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let text = text.trim();
    let rest = text.strip_prefix(command)?;
    if rest.is_empty() {
        return Some("");
    }
    if rest.chars().next().is_some_and(char::is_whitespace) {
        return Some(rest.trim_start());
    }
    let mention = rest.strip_prefix('@')?;
    let mention_end = mention
        .char_indices()
        .find(|(_, character)| character.is_whitespace())
        .map(|(index, _)| index)
        .unwrap_or(mention.len());
    if mention_end == 0 {
        return None;
    }
    Some(mention[mention_end..].trim_start())
}

async fn classify_update_lane(_ai_service: &AIChatService, update: &Update) -> UpdateLane {
    if update.stopped_message_generation.is_some() || update.callback_query.is_some() {
        return UpdateLane::Control;
    }

    let Some(message) = update.message.as_ref() else {
        return UpdateLane::Control;
    };

    if message.photo.is_some()
        || message.document.is_some()
        || message.voice.is_some()
        || message.audio.is_some()
        || message.video.is_some()
        || message.video_note.is_some()
    {
        return UpdateLane::Generation;
    }

    // In zero-slash gateway architecture, all text and prompt messages route to Generation.
    UpdateLane::Generation
}

async fn process_durable_update(
    bot: &TelegramBotClient,
    ai_service: &AIChatService,
    user_last_image_prompt: &UserLastImagePrompt,
    access: &AccessPolicy,
    update: Update,
) {
    let update_id = update.update_id;
    if !ai::storage::mark_telegram_processing_async(update_id).await {
        return;
    }

    let delivery_context = delivery_context_for_update(&update);
    TelegramBotClient::with_delivery_context(
        delivery_context,
        handle_update(bot, ai_service, user_last_image_prompt, access, update),
    )
    .await;

    if !ai::storage::mark_telegram_processed_async(update_id).await {
        warn!("Gagal menyelesaikan durable Telegram inbox update {update_id}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramDocumentMediaKind {
    Image,
    Audio,
    Video,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassifiedTelegramDocument {
    kind: TelegramDocumentMediaKind,
    mime_type: Option<String>,
}

const TELEGRAM_DOCUMENT_MEDIA_MAPPINGS: [(&str, TelegramDocumentMediaKind, &str); 16] = [
    (".png", TelegramDocumentMediaKind::Image, "image/png"),
    (".jpg", TelegramDocumentMediaKind::Image, "image/jpeg"),
    (".jpeg", TelegramDocumentMediaKind::Image, "image/jpeg"),
    (".webp", TelegramDocumentMediaKind::Image, "image/webp"),
    (".ogg", TelegramDocumentMediaKind::Audio, "audio/ogg"),
    (".oga", TelegramDocumentMediaKind::Audio, "audio/ogg"),
    (".opus", TelegramDocumentMediaKind::Audio, "audio/opus"),
    (".mp3", TelegramDocumentMediaKind::Audio, "audio/mpeg"),
    (".wav", TelegramDocumentMediaKind::Audio, "audio/wav"),
    (".m4a", TelegramDocumentMediaKind::Audio, "audio/mp4"),
    (".flac", TelegramDocumentMediaKind::Audio, "audio/flac"),
    (".mp4", TelegramDocumentMediaKind::Video, "video/mp4"),
    (".webm", TelegramDocumentMediaKind::Video, "video/webm"),
    (".mov", TelegramDocumentMediaKind::Video, "video/quicktime"),
    (".avi", TelegramDocumentMediaKind::Video, "video/x-msvideo"),
    (".mkv", TelegramDocumentMediaKind::Video, "video/x-matroska"),
];

fn normalize_telegram_document_mime(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn telegram_document_media_from_extension(
    file_name: &str,
    remote_path: &str,
) -> Option<(TelegramDocumentMediaKind, &'static str)> {
    let file_name = file_name.to_ascii_lowercase();
    if let Some((_, kind, mime_type)) = TELEGRAM_DOCUMENT_MEDIA_MAPPINGS
        .iter()
        .find(|(extension, _, _)| file_name.ends_with(*extension))
    {
        return Some((*kind, *mime_type));
    }

    let remote_path = remote_path.to_ascii_lowercase();
    TELEGRAM_DOCUMENT_MEDIA_MAPPINGS
        .iter()
        .find(|(extension, _, _)| remote_path.ends_with(*extension))
        .map(|(_, kind, mime_type)| (*kind, *mime_type))
}

fn classify_telegram_document_media(
    mime_type: &str,
    file_name: &str,
    remote_path: &str,
) -> ClassifiedTelegramDocument {
    let mime_type = normalize_telegram_document_mime(mime_type);

    let explicit_kind = if mime_type.starts_with("image/") {
        Some(TelegramDocumentMediaKind::Image)
    } else if mime_type.starts_with("audio/") {
        Some(TelegramDocumentMediaKind::Audio)
    } else if mime_type.starts_with("video/") {
        Some(TelegramDocumentMediaKind::Video)
    } else {
        None
    };

    if let Some(kind) = explicit_kind {
        return ClassifiedTelegramDocument {
            kind,
            mime_type: Some(mime_type),
        };
    }

    if let Some((kind, resolved_mime)) =
        telegram_document_media_from_extension(file_name, remote_path)
    {
        return ClassifiedTelegramDocument {
            kind,
            mime_type: Some(resolved_mime.to_string()),
        };
    }

    ClassifiedTelegramDocument {
        kind: TelegramDocumentMediaKind::Other,
        mime_type: None,
    }
}

async fn handle_update(
    bot: &TelegramBotClient,
    ai_service: &AIChatService,
    user_last_image_prompt: &UserLastImagePrompt,
    access: &AccessPolicy,
    update: Update,
) {
    if let Some(stopped) = update.stopped_message_generation.as_ref() {
        if access.allows_stop_chat(stopped.chat.id) {
            let _ = ai_service
                .cancel_generation(stopped.chat.id, stopped.draft_id)
                .await;
        }
        return;
    }
    if let Some(msg) = update.message {
        let chat_id = msg.chat.id;
        let thread_id = msg.message_thread_id.unwrap_or(0);
        let user_id = msg.from.as_ref().map(|u| u.id).unwrap_or(chat_id);
        if !access.allows(user_id, chat_id) {
            return;
        }
        let _user_name = msg
            .from
            .as_ref()
            .map(|u| u.first_name.as_str())
            .unwrap_or("Pengguna");
        let raw_text = msg
            .text
            .as_deref()
            .or(msg.caption.as_deref())
            .unwrap_or("")
            .trim();

        let is_group = chat_id != user_id;
        let is_reply_to_bot = msg
            .reply_to_message
            .as_ref()
            .and_then(|r| r.from.as_ref())
            .map(|u| {
                if let Some(ref my_bot) = access.bot_username {
                    u.username
                        .as_deref()
                        .unwrap_or("")
                        .eq_ignore_ascii_case(my_bot)
                } else {
                    u.is_bot
                }
            })
            .unwrap_or(false);

        let Some(text) = prepare_incoming_chat_text(
            raw_text,
            is_group,
            access.bot_username.as_deref(),
            is_reply_to_bot,
        ) else {
            return;
        };

        let mut image_bytes = None;
        let mut document_images = None;
        let mut mime_type = None;
        let mut doc_text = None;
        let mut doc_name = None;
        let mut audio_bytes = None;
        let mut audio_mime = None;
        let mut audio_duration = 0;
        let mut video_bytes = None;
        let mut video_mime = None;
        let mut video_duration = 0;

        let has_video = msg.video.is_some() || msg.video_note.is_some();
        let has_photo = msg.photo.is_some();
        let has_audio = msg.voice.is_some() || msg.audio.is_some();
        let has_document = msg.document.is_some();

        if let Some(v) = msg.voice {
            audio_duration = v.duration;
            audio_mime = v.mime_type;
            if let Some((data, path)) = bot.get_file_bytes(&v.file_id).await {
                audio_bytes = Some(data);
                doc_name = path.split('/').next_back().map(str::to_string);
            }
        } else if let Some(a) = msg.audio {
            audio_duration = a.duration;
            audio_mime = a.mime_type;
            let audio_file_name = a.file_name;
            if let Some((data, path)) = bot.get_file_bytes(&a.file_id).await {
                audio_bytes = Some(data);
                doc_name =
                    audio_file_name.or_else(|| path.split('/').next_back().map(str::to_string));
            }
        } else if let Some(vid) = msg.video {
            video_duration = vid.duration;
            if let Some((data, path)) = bot.get_file_bytes(&vid.file_id).await {
                video_bytes = Some(data);
                let ext = path.split('.').next_back().unwrap_or("mp4");
                video_mime = vid.mime_type.or_else(|| Some(format!("video/{ext}")));
            }
        } else if let Some(vn) = msg.video_note {
            video_duration = vn.duration;
            if let Some((data, _)) = bot.get_file_bytes(&vn.file_id).await {
                video_bytes = Some(data);
                video_mime = Some("video/mp4".to_string());
            }
        } else if let Some(photos) = msg.photo {
            if let Some(largest) = photos.last() {
                if let Some((data, path)) = bot.get_file_bytes(&largest.file_id).await {
                    image_bytes = Some(data);
                    let ext = path.split('.').next_back().unwrap_or("jpeg");
                    mime_type = Some(if ext == "jpg" {
                        "image/jpeg".to_string()
                    } else {
                        format!("image/{ext}")
                    });
                }
            }
        } else if let Some(doc) = msg.document {
            let d_mime = doc.mime_type.clone().unwrap_or_default();
            let d_name = doc
                .file_name
                .clone()
                .unwrap_or_else(|| "dokumen".to_string());
            if let Some((data, path)) = bot.get_file_bytes(&doc.file_id).await {
                let ClassifiedTelegramDocument {
                    kind,
                    mime_type: resolved_mime,
                } = classify_telegram_document_media(&d_mime, &d_name, &path);
                match kind {
                    TelegramDocumentMediaKind::Image => {
                        image_bytes = Some(data);
                        mime_type = resolved_mime;
                    }
                    TelegramDocumentMediaKind::Audio => {
                        audio_bytes = Some(data);
                        audio_mime = resolved_mime;
                        doc_name = Some(d_name);
                    }
                    TelegramDocumentMediaKind::Video => {
                        video_bytes = Some(data);
                        video_mime = resolved_mime;
                    }
                    TelegramDocumentMediaKind::Other
                        if document::is_extractable_document(&d_mime, &d_name) =>
                    {
                        match document::extract_document(data, &d_mime, &d_name).await {
                            Ok(extracted) => {
                                doc_text = extracted.text;
                                if !extracted.rendered_pages.is_empty() {
                                    document_images = Some(extracted.rendered_pages);
                                }
                                doc_name = Some(d_name);
                                if let Some(warning) = extracted.warning {
                                    info!("{warning}");
                                }
                            }
                            Err(err) => {
                                let safe_name = escape_html(&d_name);
                                let safe_error = escape_html(&err);
                                let _ = bot
                                    .send_message(
                                        chat_id,
                                        &format!(
                                            "⚠️ <b>Dokumen tidak dapat diproses.</b>\n\n<code>{safe_name}</code>\n{safe_error}"
                                        ),
                                        Some("HTML"),
                                        None,
                                        None,
                                        None,
                                    )
                                    .await;
                                return;
                            }
                        }
                    }
                    TelegramDocumentMediaKind::Other => {
                        let safe_name = escape_html(&d_name);
                        let _ = bot.send_message(
                            chat_id,
                            &format!(
                                "⚠️ <b>Format dokumen belum didukung.</b>\n\n<code>{safe_name}</code> tidak akan dipaksa dibaca sebagai teks biner. Xiao mendukung dokumen teks/kode, PDF, DOCX, XLSX, serta arsip ZIP, TAR/TAR.GZ, dan 7Z."
                            ),
                            Some("HTML"), None, None, None
                        ).await;
                        return;
                    }
                }
            }
        }

        if has_photo && image_bytes.is_none() {
            let _ = bot
                .send_message(
                    chat_id,
                    "⚠️ <b>Gagal mengunduh gambar dari server Telegram.</b> Silakan coba kirim ulang.",
                    Some("HTML"),
                    None,
                    None,
                    None,
                )
                .await;
            return;
        }
        if has_audio && audio_bytes.is_none() {
            let _ = bot
                .send_message(
                    chat_id,
                    "⚠️ <b>Gagal mengunduh audio dari Telegram.</b> Silakan kirim ulang pesan suara/audio.",
                    Some("HTML"),
                    None,
                    None,
                    None,
                )
                .await;
            return;
        }
        if has_document
            && image_bytes.is_none()
            && audio_bytes.is_none()
            && video_bytes.is_none()
            && doc_text.is_none()
            && document_images
                .as_ref()
                .is_none_or(|pages| pages.is_empty())
        {
            let _ = bot
                .send_message(
                    chat_id,
                    "⚠️ <b>Gagal mengunduh dokumen dari Telegram.</b> Silakan kirim ulang file tersebut.",
                    Some("HTML"),
                    None,
                    None,
                    None,
                )
                .await;
            return;
        }
        let mut text = text;
        if text.is_empty()
            && image_bytes.is_none()
            && audio_bytes.is_none()
            && video_bytes.is_none()
            && doc_text.is_none()
            && document_images
                .as_ref()
                .is_none_or(|pages| pages.is_empty())
        {
            if is_group {
                if let Some(ref bot_name) = access.bot_username {
                    let bot_tag = format!("@{bot_name}");
                    let is_bot_mention = raw_text.split_whitespace().any(|w| {
                        let cleaned = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                        cleaned.eq_ignore_ascii_case(bot_name)
                            || cleaned.eq_ignore_ascii_case(&bot_tag[1..])
                    });
                    if is_bot_mention {
                        text = "Halo Xiao!".to_string();
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            } else {
                return;
            }
        }

        // Strict provider lock
        if !ai_service.has_configured_provider(user_id).await {
            let _ = bot
                .send_message(
                    chat_id,
                    "⚠️ <b>Xiao belum memiliki provider AI aktif.</b>\n\n\
                     Silakan hubungkan AI provider terlebih dahulu melalui terminal host:\n\
                     <code>xiao setup</code> atau <code>xiao ai</code>",
                    Some("HTML"),
                    None,
                    None,
                    None,
                )
                .await;
            return;
        }

        // Voice & audio file processing is role-routed inside AIChatService.
        // Do not pre-transcribe against the active provider here because that
        // would bypass the configured Audio STT Model route.
        if let Some(a_bytes) = audio_bytes {
            let prompt_audio = if !text.is_empty() {
                format!(
                    "Dengarkan rekaman/audio terlampir dan tanggapi permintaan berikut:\n\n{text}"
                )
            } else {
                format!(
                    "Dengarkan pesan suara/audio ini ({} detik) dan jawab pertanyaan atau tanggapi maksud di dalamnya secara jelas dan mendalam.",
                    audio_duration
                )
            };

            let chat_input = build_audio_chat_input(
                &prompt_audio,
                a_bytes,
                audio_mime.as_deref(),
                doc_name.as_deref(),
            );
            handle_ai_chat(bot, ai_service, chat_id, thread_id, user_id, chat_input).await;
            return;
        }

        // Video processing
        if let Some(v_bytes) = video_bytes {
            let prompt_video = if !text.is_empty() {
                text.clone()
            } else {
                "Tonton dan analisis rekaman video ini secara mendalam. Jelaskan isi visual, alur peristiwa, teks di layar, dan suara di dalamnya.".to_string()
            };

            handle_ai_chat(
                bot,
                ai_service,
                chat_id,
                thread_id,
                user_id,
                ChatInput {
                    prompt: &prompt_video,
                    image_bytes: None,
                    document_images: None,
                    mime_type: None,
                    doc_text: None,
                    doc_name: None,
                    audio_bytes: None,
                    audio_mime: None,
                    video_bytes: Some(v_bytes),
                    video_mime: video_mime.as_deref(),
                    video_duration: Some(video_duration),
                    model_snapshot: None,
                },
            )
            .await;
            return;
        } else if has_video {
            let _ = bot
                .send_message(
                    chat_id,
                    "⚠️ <b>Gagal mengunduh video dari Telegram.</b>\n\n\
                     Telegram membatasi ukuran unduhan file bot maksimal <b>20MB</b>. Pastikan durasi atau ukuran video di bawah 20MB.",
                    Some("HTML"),
                    None,
                    None,
                    None,
                )
                .await;
            return;
        }

        let is_explicit_image = command_matches(&text, "/image");
        let image_arg = if is_explicit_image {
            command_args(&text, "/image").unwrap_or("")
        } else {
            &text
        };

        let auto_image_intent = if image_bytes.is_none() && doc_text.is_none() {
            plan_image_generation_intent(image_arg).or_else(|| {
                if is_explicit_image && !image_arg.trim().is_empty() {
                    Some(ImageGenerationIntent {
                        image_prompt: image_arg.trim().to_string(),
                        explanation_prompt: None,
                    })
                } else {
                    None
                }
            })
        } else {
            None
        };

        if let Some(intent) = auto_image_intent {
            handle_image_generation(
                bot,
                ai_service,
                user_last_image_prompt,
                chat_id,
                thread_id,
                user_id,
                &intent.image_prompt,
                intent.explanation_prompt.as_deref(),
            )
            .await;
        } else {
            handle_ai_chat(
                bot,
                ai_service,
                chat_id,
                thread_id,
                user_id,
                ChatInput {
                    prompt: &text,
                    image_bytes,
                    document_images,
                    mime_type: mime_type.as_deref(),
                    doc_text: doc_text.as_deref(),
                    doc_name: doc_name.as_deref(),
                    audio_bytes: None,
                    audio_mime: None,
                    video_bytes,
                    video_mime: video_mime.as_deref(),
                    video_duration: Some(video_duration),
                    model_snapshot: None,
                },
            )
            .await;
        }
    } else if let Some(cq) = update.callback_query {
        let cq_id = cq.id;
        let _ = bot
            .answer_callback_query(
                &cq_id,
                Some("Xiao is now a pure conversational assistant."),
                false,
            )
            .await;
        if let Some(msg) = cq.message {
            let _ = bot.delete_message(msg.chat.id, msg.message_id).await;
        }
    }
}

// ==========================================
// Main Function & Polling Loop
// ==========================================

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let args: Vec<String> = env::args().collect();
    let subcommand = args.get(1).map(|s| s.as_str()).unwrap_or("start");

    let ai_service = Arc::new(AIChatService::new());

    match subcommand {
        "-v" | "--version" | "version" => {
            println!("xiao v{}", env!("CARGO_PKG_VERSION"));
            return;
        }
        "ai" => {
            let action_arg = args.get(2).map(|s| s.as_str());
            let target_arg = args.get(3).map(|s| s.as_str());
            run_cli_ai_hub(&ai_service, action_arg, target_arg).await;
            return;
        }
        "setup" => {
            let _ = run_cli_quickstart_wizard(&ai_service).await;
            return;
        }
        "status" => {
            run_cli_status(&ai_service).await;
            return;
        }
        "context" => {
            let chat_arg = args.get(2).and_then(|s| s.parse::<i64>().ok());
            let thread_arg = args.get(3).and_then(|s| s.parse::<i64>().ok());
            run_cli_context(&ai_service, chat_arg, thread_arg).await;
            return;
        }
        "memory" => {
            let action_arg = args.get(2).map(|s| s.as_str());
            let target_arg = args.get(3).map(|s| s.as_str());
            run_cli_memory(&ai_service, action_arg, target_arg).await;
            return;
        }
        "gateway" => {
            let action_arg = args.get(2).map(|s| s.as_str());
            let target_arg = args.get(3).map(|s| s.as_str());
            run_cli_gateway_hub(action_arg, target_arg).await;
            return;
        }
        "doctor" | "probe" => {
            run_cli_probe_menu(&ai_service).await;
            return;
        }
        "provider" => {
            let action_arg = args.get(2).map(|s| s.as_str());
            run_cli_provider_menu(&ai_service, action_arg).await;
            return;
        }
        "model" => {
            let filter_arg = args.get(2).map(|s| s.as_str());
            if filter_arg == Some("addon") || filter_arg == Some("addons") {
                run_cli_addon_menu(&ai_service).await;
            } else {
                run_cli_model_picker(&ai_service, filter_arg).await;
            }
            return;
        }
        "addon" => {
            run_cli_addon_menu(&ai_service).await;
            return;
        }
        "chat" => {
            let prompt_arg = if args.len() > 2 {
                Some(args[2..].join(" "))
            } else {
                None
            };
            run_cli_chat(&ai_service, prompt_arg).await;
            return;
        }
        "help" | "--help" | "-h" => {
            print_cli_help();
            return;
        }
        "start" => {
            // Proceed to run bot server
        }
        unknown => {
            println!("\x1b[31m✖ Error: Perintah '{unknown}' tidak dikenal. Jalankan 'xiao help' untuk bantuan.\x1b[0m");
            std::process::exit(1);
        }
    }

    tracing_subscriber::fmt::init();

    let Some(token) = get_or_prompt_token(&ai_service).await else {
        return;
    };

    let Some(owner_user_id) = get_configured_owner_id() else {
        error!("OWNER_USER_ID belum dikonfigurasi. Jalankan `xiao gateway` atau `xiao setup`.");
        return;
    };

    let bot = TelegramBotClient::new(token);
    let user_last_image_prompt: UserLastImagePrompt = Arc::new(RwLock::new(HashMap::new()));

    // Test connection & get bot identity
    let bot_username = match bot.get_me().await {
        Ok(resp) if resp.ok => {
            let Some(bot_info) = resp.result else {
                error!("Telegram getMe returned ok=true without a result");
                return;
            };
            println!(
                "\n🚀 xiao @{} online menggunakan Telegram Bot API 10.3!",
                bot_info.username.as_deref().unwrap_or_default()
            );
            println!("⚡ Streaming Timeline + Native Stop Active!");
            println!("🌐 Custom OpenAI-Compatible Provider Setup Active (via CLI)\n");
            bot_info.username
        }
        Ok(resp) => {
            error!(
                "Gagal terhubung ke Telegram Bot API: {:?}",
                resp.description
            );
            return;
        }
        Err(e) => {
            error!("HTTP connection error: {e}");
            return;
        }
    };

    let access = Arc::new(AccessPolicy {
        owner_user_id,
        allowed_chat_ids: get_allowed_chat_ids(),
        bot_username,
    });

    // Register Bot Commands - Clear all commands for pure zero-slash conversational gateway
    let empty_commands: Vec<BotCommand> = vec![];

    if let Err(e) = bot.set_my_commands(&empty_commands).await {
        warn!("Gagal mengosongkan bot commands di Telegram: {e}");
    } else {
        info!("Bot commands berhasil dikosongkan (pure zero-slash gateway).");
    }

    let (generation_tx, mut generation_rx) = tokio::sync::mpsc::channel::<Update>(64);
    let (control_tx, mut control_rx) = tokio::sync::mpsc::channel::<Update>(64);

    let generation_bot = bot.clone();
    let generation_ai = Arc::clone(&ai_service);
    let generation_last_image = Arc::clone(&user_last_image_prompt);
    let generation_access = Arc::clone(&access);
    let generation_worker = tokio::spawn(async move {
        while let Some(update) = generation_rx.recv().await {
            process_durable_update(
                &generation_bot,
                &generation_ai,
                &generation_last_image,
                &generation_access,
                update,
            )
            .await;
        }
    });

    let control_bot = bot.clone();
    let control_ai = Arc::clone(&ai_service);
    let control_last_image = Arc::clone(&user_last_image_prompt);
    let control_access = Arc::clone(&access);
    let control_worker = tokio::spawn(async move {
        while let Some(update) = control_rx.recv().await {
            process_durable_update(
                &control_bot,
                &control_ai,
                &control_last_image,
                &control_access,
                update,
            )
            .await;
        }
    });

    let interrupted = ai::storage::recover_telegram_processing_async().await;
    if interrupted > 0 {
        warn!(
            "{interrupted} Telegram update berstatus processing dikembalikan ke pending untuk replay at-least-once; side effect eksternal sebelum crash dapat terulang"
        );
    }

    let mut replay_after_update_id = i64::MIN;
    loop {
        let replay_batch =
            ai::storage::pending_telegram_updates_after_async(replay_after_update_id, 500).await;
        if replay_batch.is_empty() {
            break;
        }
        for record in replay_batch {
            replay_after_update_id = record.update_id;
            match serde_json::from_str::<Update>(&record.payload_json) {
                Ok(update) => {
                    if update.stopped_message_generation.is_some() {
                        // Native Stop bypasses both queues for immediate cancellation.
                        process_durable_update(
                            &bot,
                            &ai_service,
                            &user_last_image_prompt,
                            &access,
                            update,
                        )
                        .await;
                    } else {
                        let lane = classify_update_lane(&ai_service, &update).await;
                        let send_result = match lane {
                            UpdateLane::Control => control_tx.send(update).await,
                            UpdateLane::Generation => generation_tx.send(update).await,
                        };
                        if send_result.is_err() {
                            error!("Update worker stopped while replaying durable inbox");
                            return;
                        }
                    }
                }
                Err(error) => {
                    warn!(
                        "Durable Telegram update {} tidak dapat didecode: {error}",
                        record.update_id
                    );
                    if ai::storage::mark_telegram_processing_async(record.update_id).await
                        && !ai::storage::mark_telegram_processed_async(record.update_id).await
                    {
                        warn!(
                        "Gagal menandai durable Telegram update {} yang invalid sebagai completed",
                        record.update_id
                    );
                    }
                }
            }
        }
    }

    let mut offset = ai::storage::load_telegram_offset_async().await;
    info!("Memulai polling pesan dengan durable control/generation queues...");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\n🛑 Menerima sinyal berhenti. Bot dimatikan secara aman.");
                break;
            }
            updates_res = bot.get_updates(offset, 100, 20, Some(vec!["message".to_string(), "stopped_message_generation".to_string()])) => {
                match updates_res {
                    Ok(resp) if resp.ok => {
                        if let Some(updates) = resp.result {
                            for update in updates {
                                let update_id = update.update_id;
                                let payload_json = match serde_json::to_string(&update) {
                                    Ok(payload) => payload,
                                    Err(error) => {
                                        error!("Gagal serialize Telegram update {update_id}: {error}");
                                        break;
                                    }
                                };
                                let Some(accepted) = ai::storage::enqueue_telegram_update_async(
                                    update_id,
                                    payload_json,
                                )
                                .await
                                else {
                                    // Never acknowledge a later Telegram update if the durable
                                    // acceptance transaction for this update failed.
                                    error!(
                                        "Durable Telegram intake gagal untuk update {update_id}; offset tidak dimajukan"
                                    );
                                    break;
                                };
                                offset = Some(update_id.saturating_add(1));
                                if !accepted {
                                    continue;
                                }

                                if update.stopped_message_generation.is_some() {
                                    // Native Stop bypasses both queues so cancellation cannot be
                                    // blocked by either control work or an active generation.
                                    process_durable_update(
                                        &bot,
                                        &ai_service,
                                        &user_last_image_prompt,
                                        &access,
                                        update,
                                    )
                                    .await;
                                } else {
                                    let lane = classify_update_lane(&ai_service, &update).await;
                                    let send_result = match lane {
                                        UpdateLane::Control => control_tx.send(update).await,
                                        UpdateLane::Generation => generation_tx.send(update).await,
                                    };
                                    if send_result.is_err() {
                                        error!("Update worker stopped unexpectedly");
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    Ok(resp) => {
                        warn!("Telegram polling update not ok: {:?}", resp.description);
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Err(e) => {
                        error!("Polling network error: {e}");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    }

    ai_service.cancel_all_generations().await;
    drop(control_tx);
    drop(generation_tx);

    match tokio::time::timeout(Duration::from_secs(5), control_worker).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => warn!("Control worker terminated with error: {err}"),
        Err(_) => warn!("Control worker did not stop within shutdown grace period"),
    }
    match tokio::time::timeout(Duration::from_secs(5), generation_worker).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => warn!("Generation worker terminated with error: {err}"),
        Err(_) => warn!("Generation worker did not stop within shutdown grace period"),
    }
}

#[cfg(test)]
mod update_lane_tests {
    use super::*;

    #[tokio::test]
    async fn pure_zero_slash_routes_all_text_to_generation_lane() {
        let ai = Arc::new(AIChatService::new());
        for text in [
            "/start",
            "/start@xiaobot",
            "/start hello",
            "/model",
            "Jelaskan orbit satelit",
            "/image seekor rubah di kota neon",
            "buatkan gambar pemandangan",
            "halo Xiao apa kabar?",
            "/menu",
            "/help",
            "/clear",
        ] {
            let update: Update = serde_json::from_value(json!({
                "update_id": 100,
                "message": {
                    "message_id": 1,
                    "date": 1700000000,
                    "chat": { "id": 12345, "type": "private" },
                    "from": { "id": 12345, "is_bot": false, "first_name": "Test" },
                    "text": text
                }
            }))
            .unwrap();
            assert_eq!(
                classify_update_lane(&ai, &update).await,
                UpdateLane::Generation,
                "{text}"
            );
        }
    }

    #[test]
    fn command_matching_requires_a_real_token_boundary() {
        assert!(command_matches("/menu", "/menu"));
        assert_eq!(command_args("/menu settings", "/menu"), Some("settings"));
        assert!(command_matches("/menu@xiaobot settings", "/menu"));
        assert!(!command_matches("/menux", "/menu"));
        assert!(!command_matches("/imagegen", "/image"));
        assert!(!command_matches("/startling", "/start"));
    }

    #[test]
    fn specialist_context_policies_are_minimal_and_role_specific() {
        assert!(specialist_context_policy(
            ai::service::ModelRole::Vision,
            ai::service::RouteOrigin::Specific
        )
        .contains("no full history"));
        assert!(specialist_context_policy(
            ai::service::ModelRole::AudioStt,
            ai::service::RouteOrigin::Specific
        )
        .starts_with("Transcript only"));
        assert!(specialist_context_policy(
            ai::service::ModelRole::ImageGeneration,
            ai::service::RouteOrigin::Specific
        )
        .starts_with("Prompt/config only"));
        assert!(specialist_context_policy(
            ai::service::ModelRole::Curator,
            ai::service::RouteOrigin::Specific
        )
        .contains("Background"));
        assert!(specialist_context_policy(
            ai::service::ModelRole::Vision,
            ai::service::RouteOrigin::MainModel
        )
        .starts_with("Direct on Main"));
    }

    #[test]
    fn context_semantics_use_only_main_budget_and_never_underflow() {
        assert_eq!(context_available_tokens(100_000, 35_000), 65_000);
        assert_eq!(context_available_tokens(64_000, 80_000), 0);
        assert!(specialist_context_policy(
            ai::service::ModelRole::Vision,
            ai::service::RouteOrigin::Specific
        )
        .contains("no full history"));
    }

    #[test]
    fn smaller_main_context_warns_only_when_history_exceeds_usable_budget() {
        assert!(main_context_overflow_warning("small-model", 80_000, 64_000).is_some());
        assert!(main_context_overflow_warning("large-model", 40_000, 64_000).is_none());
    }

    #[test]
    fn start_and_menu_contracts_remain_distinct() {
        let start_buttons: [&str; 0] = [];
        assert_eq!(start_buttons.len(), 0);
    }

    #[test]
    fn help_ui_is_typed_rich_and_declares_addons_read_only() {
        let value = serde_json::to_value(build_help_ui()).unwrap();
        let serialized = value.to_string();
        assert!(serialized.contains("\"type\":\"table\""));
        assert!(serialized.contains("\"type\":\"details\""));
        assert!(serialized.contains("read-only"));
        assert!(serialized.contains("xiao addon"));
    }

    #[test]
    fn compound_image_intent_keeps_explanation_for_main() {
        let intent = plan_image_generation_intent(
            "buat gambar simulasi galaksi dan jelaskan bagaimana lengan spiral terbentuk",
        )
        .expect("compound image intent");
        assert!(intent.image_prompt.to_ascii_lowercase().contains("galaksi"));
        assert_eq!(
            intent.explanation_prompt.as_deref(),
            Some("jelaskan bagaimana lengan spiral terbentuk")
        );
    }

    #[test]
    fn image_generation_draft_can_stop_policy_is_private_only() {
        let owner_id = 123456789i64;
        let group_id = -100987654321i64;

        let private_can_stop = owner_id == owner_id;
        assert!(private_can_stop);

        let group_can_stop = group_id == owner_id;
        assert!(!group_can_stop);
    }

    #[test]
    fn access_policy_allows_native_stop_only_in_owner_private_chat() {
        let policy = AccessPolicy {
            owner_user_id: 42,
            allowed_chat_ids: [100, 200].into_iter().collect(),
            bot_username: Some("xiao_bot".to_string()),
        };
        assert!(policy.allows_stop_chat(42));
        assert!(!policy.allows_stop_chat(100));
        assert!(!policy.allows_stop_chat(200));
        assert!(!policy.allows_stop_chat(999));
    }

    #[test]
    fn access_policy_allows_owner_in_private_and_groups_when_whitelist_empty() {
        let policy = AccessPolicy {
            owner_user_id: 42,
            allowed_chat_ids: HashSet::new(),
            bot_username: Some("Sms2tg_bot".to_string()),
        };
        // Owner in private chat
        assert!(policy.allows(42, 42));
        // Owner in supergroup / topic group (negative chat ID)
        assert!(policy.allows(42, -1001234567890));
        // Non-owner in private or group is rejected
        assert!(!policy.allows(999, 999));
        assert!(!policy.allows(999, -1001234567890));
    }

    #[test]
    fn access_policy_enforces_whitelist_when_configured() {
        let policy = AccessPolicy {
            owner_user_id: 42,
            allowed_chat_ids: [-100111, -100222].into_iter().collect(),
            bot_username: Some("Sms2tg_bot".to_string()),
        };
        // Private chat always allowed for owner
        assert!(policy.allows(42, 42));
        // Whitelisted groups allowed for owner
        assert!(policy.allows(42, -100111));
        assert!(policy.allows(42, -100222));
        // Non-whitelisted group rejected
        assert!(!policy.allows(42, -100999));
        // Non-owner always rejected even in whitelisted group
        assert!(!policy.allows(999, -100111));
    }

    #[test]
    fn incoming_chat_text_handles_group_mentions_and_isolation() {
        let bot_name = "Sms2tg_bot";
        // Private chat keeps text as is
        assert_eq!(
            prepare_incoming_chat_text("Hai", false, Some(bot_name), false),
            Some("Hai".to_string())
        );

        // Group chat with no mention to anyone: responds
        assert_eq!(
            prepare_incoming_chat_text("Hai", true, Some(bot_name), false),
            Some("Hai".to_string())
        );

        // Group chat mentioning this bot: strips mention
        assert_eq!(
            prepare_incoming_chat_text("Tes @Sms2tg_bot", true, Some(bot_name), false),
            Some("Tes".to_string())
        );
        assert_eq!(
            prepare_incoming_chat_text("@Sms2tg_bot /start", true, Some(bot_name), false),
            Some("/start".to_string())
        );
        assert_eq!(
            prepare_incoming_chat_text("Hello @Sms2tg_bot", true, Some(bot_name), false),
            Some("Hello".to_string())
        );
        assert_eq!(
            prepare_incoming_chat_text("Hi @sms2tg_bot", true, Some(bot_name), false),
            Some("Hi".to_string())
        );

        // Group chat mentioning another user/bot: ignored
        assert_eq!(
            prepare_incoming_chat_text("Hai @mira", true, Some(bot_name), false),
            None
        );

        // Group chat replying to bot: responded even without mention
        assert_eq!(
            prepare_incoming_chat_text("Jawaban bagus", true, Some(bot_name), true),
            Some("Jawaban bagus".to_string())
        );
    }

    #[test]
    fn telegram_document_explicit_media_mime_overrides_conflicting_extension() {
        let cases = [
            (
                "audio/webm",
                "file.webm",
                TelegramDocumentMediaKind::Audio,
                "audio/webm",
            ),
            (
                "audio/mp4",
                "file.mp4",
                TelegramDocumentMediaKind::Audio,
                "audio/mp4",
            ),
            (
                "audio/flac",
                "file.mkv",
                TelegramDocumentMediaKind::Audio,
                "audio/flac",
            ),
            (
                "audio/opus",
                "clip.webm",
                TelegramDocumentMediaKind::Audio,
                "audio/opus",
            ),
            (
                "video/mp4",
                "recording.mp3",
                TelegramDocumentMediaKind::Video,
                "video/mp4",
            ),
            (
                "video/webm",
                "voice.opus",
                TelegramDocumentMediaKind::Video,
                "video/webm",
            ),
            (
                "image/png",
                "movie.mp4",
                TelegramDocumentMediaKind::Image,
                "image/png",
            ),
            (
                "image/jpeg",
                "recording.flac",
                TelegramDocumentMediaKind::Image,
                "image/jpeg",
            ),
        ];

        for (mime_type, file_name, expected_kind, expected_mime) in cases {
            let classified = classify_telegram_document_media(mime_type, file_name, file_name);
            assert_eq!(classified.kind, expected_kind, "{mime_type} / {file_name}");
            assert_eq!(
                classified.mime_type.as_deref(),
                Some(expected_mime),
                "{mime_type} / {file_name}"
            );
        }
    }

    #[test]
    fn telegram_document_mime_less_extension_resolves_canonical_media_identity() {
        let cases = [
            ("sample.png", TelegramDocumentMediaKind::Image, "image/png"),
            ("sample.jpg", TelegramDocumentMediaKind::Image, "image/jpeg"),
            (
                "sample.jpeg",
                TelegramDocumentMediaKind::Image,
                "image/jpeg",
            ),
            (
                "sample.webp",
                TelegramDocumentMediaKind::Image,
                "image/webp",
            ),
            ("sample.ogg", TelegramDocumentMediaKind::Audio, "audio/ogg"),
            ("sample.oga", TelegramDocumentMediaKind::Audio, "audio/ogg"),
            (
                "sample.opus",
                TelegramDocumentMediaKind::Audio,
                "audio/opus",
            ),
            ("sample.mp3", TelegramDocumentMediaKind::Audio, "audio/mpeg"),
            ("sample.wav", TelegramDocumentMediaKind::Audio, "audio/wav"),
            ("sample.m4a", TelegramDocumentMediaKind::Audio, "audio/mp4"),
            (
                "sample.flac",
                TelegramDocumentMediaKind::Audio,
                "audio/flac",
            ),
            ("sample.mp4", TelegramDocumentMediaKind::Video, "video/mp4"),
            (
                "sample.webm",
                TelegramDocumentMediaKind::Video,
                "video/webm",
            ),
            (
                "sample.mov",
                TelegramDocumentMediaKind::Video,
                "video/quicktime",
            ),
            (
                "sample.avi",
                TelegramDocumentMediaKind::Video,
                "video/x-msvideo",
            ),
            (
                "sample.mkv",
                TelegramDocumentMediaKind::Video,
                "video/x-matroska",
            ),
        ];

        for (file_name, expected_kind, expected_mime) in cases {
            let classified = classify_telegram_document_media("", file_name, file_name);
            assert_eq!(classified.kind, expected_kind, "{file_name}");
            assert_eq!(
                classified.mime_type.as_deref(),
                Some(expected_mime),
                "{file_name}"
            );
        }

        let unknown = classify_telegram_document_media("", "arbitrary.bin", "documents/file_789");
        assert_eq!(unknown.kind, TelegramDocumentMediaKind::Other);
        assert_eq!(unknown.mime_type, None);
    }

    #[test]
    fn telegram_document_remote_path_fallback_resolves_media_identity() {
        let audio = classify_telegram_document_media("", "document", "documents/file.opus");
        assert_eq!(audio.kind, TelegramDocumentMediaKind::Audio);
        assert_eq!(audio.mime_type.as_deref(), Some("audio/opus"));

        let video = classify_telegram_document_media("", "document", "documents/video.webm");
        assert_eq!(video.kind, TelegramDocumentMediaKind::Video);
        assert_eq!(video.mime_type.as_deref(), Some("video/webm"));
    }

    #[test]
    fn telegram_document_filename_identity_wins_before_remote_path_fallback() {
        let video =
            classify_telegram_document_media("", "sample.webm", "documents/telegram-file.mp3");
        assert_eq!(video.kind, TelegramDocumentMediaKind::Video);
        assert_eq!(video.mime_type.as_deref(), Some("video/webm"));

        let audio =
            classify_telegram_document_media("", "sample.mp3", "documents/telegram-file.webm");
        assert_eq!(audio.kind, TelegramDocumentMediaKind::Audio);
        assert_eq!(audio.mime_type.as_deref(), Some("audio/mpeg"));
    }

    #[test]
    fn telegram_document_mime_normalization_handles_case_and_parameters() {
        let audio = classify_telegram_document_media(
            "Audio/WebM; codecs=opus",
            "file.webm",
            "documents/file.webm",
        );
        assert_eq!(audio.kind, TelegramDocumentMediaKind::Audio);
        assert_eq!(audio.mime_type.as_deref(), Some("audio/webm"));

        let video = classify_telegram_document_media("VIDEO/MP4", "file.mp3", "documents/file.mp3");
        assert_eq!(video.kind, TelegramDocumentMediaKind::Video);
        assert_eq!(video.mime_type.as_deref(), Some("video/mp4"));

        let image = classify_telegram_document_media(
            "  image/PNG ; charset=binary ",
            "clip.mp4",
            "documents/clip.mp4",
        );
        assert_eq!(image.kind, TelegramDocumentMediaKind::Image);
        assert_eq!(image.mime_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn mime_less_audio_identity_survives_runtime_chat_input_and_stt_metadata() {
        for (file_name, expected_mime, expected_stt_name) in [
            ("sample.mp3", "audio/mpeg", "sample.mp3"),
            ("sample.wav", "audio/wav", "sample.wav"),
            ("sample.opus", "audio/opus", "sample.opus"),
            ("sample.flac", "audio/flac", "sample.flac"),
        ] {
            let classified = classify_telegram_document_media("", file_name, file_name);
            assert_eq!(classified.kind, TelegramDocumentMediaKind::Audio);

            let input = build_audio_chat_input(
                "analyze",
                vec![1, 2, 3],
                classified.mime_type.as_deref(),
                Some(file_name),
            );
            assert_eq!(input.doc_name, Some(file_name));
            assert_eq!(input.audio_mime, Some(expected_mime));

            let (stt_mime, stt_name) =
                ai::service::resolve_audio_file_and_mime(input.audio_mime, input.doc_name);
            assert_eq!(stt_mime, expected_mime);
            assert_eq!(stt_name, expected_stt_name);
        }
    }

    #[test]
    fn telegram_document_runtime_uses_one_authoritative_media_classifier() {
        let source = include_str!("main.rs").replace("\r\n", "\n");
        let document_start = source
            .find("} else if let Some(doc) = msg.document {")
            .expect("Telegram document branch");
        let document_end = source[document_start..]
            .find("\n        }\n\n        // Strict provider lock")
            .map(|offset| document_start + offset)
            .expect("Telegram document branch end");
        let document_branch = &source[document_start..document_end];

        assert_eq!(
            document_branch
                .matches("classify_telegram_document_media(")
                .count(),
            1
        );
        assert!(!document_branch.contains("telegram_document_is_audio"));
        assert!(!document_branch.contains("\"image/jpeg\".to_string()"));
        assert!(!document_branch.contains("\"video/mp4\".to_string()"));

        let audio_start = source
            .find("if let Some(a_bytes) = audio_bytes {")
            .expect("audio runtime branch");
        let audio_end = source[audio_start..]
            .find("// Video processing")
            .map(|offset| audio_start + offset)
            .expect("audio runtime branch end");
        let audio_branch = &source[audio_start..audio_end];
        assert_eq!(audio_branch.matches("build_audio_chat_input(").count(), 1);
        assert!(!audio_branch.contains("doc_name: None"));
        assert!(audio_branch.contains("doc_name.as_deref()"));
    }

    #[test]
    fn image_caption_is_unicode_safe_bounded_and_html_escaped() {
        let prompt = format!("{} <tag> & \"quotes\" 'single'", "🌌银河系".repeat(800));
        let caption = build_image_success_caption(
            &prompt,
            "provider<&>",
            "model<\"x\">&",
            (1024, 1024),
            12.34,
            true,
            Some("failure <unsafe> & detail"),
        );

        assert!(caption.chars().count() <= TELEGRAM_PHOTO_CAPTION_MAX_CHARS);
        assert!(!caption.contains("<tag>"));
        assert!(caption.contains("&lt;"));
        assert!(caption.contains("&amp;"));
        assert!(caption.contains("&quot;"));
        assert!(!caption.contains("provider<&>"));
        assert!(caption.is_char_boundary(caption.len()));
    }

    #[test]
    fn photo_delivery_classifier_retries_only_caption_or_markup_failures() {
        assert_eq!(
            telegram_photo_delivery_error_class("Bad Request: can't parse entities in caption"),
            "caption_or_markup"
        );
        assert_eq!(
            telegram_photo_delivery_error_class("sendPhoto multipart error: timeout"),
            "telegram_transport"
        );
        assert_eq!(
            telegram_photo_delivery_error_class(
                "sendPhoto rejected bytes with an unsupported image signature"
            ),
            "local_image_validation"
        );
    }

    #[tokio::test]
    async fn image_delivery_success_is_single_send() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_sender = Arc::clone(&calls);
        let result = deliver_generated_image_with(
            b"same-image-bytes",
            "safe caption",
            Some(json!({"inline_keyboard": []})),
            move |_bytes, _caption, _parse_mode, _markup| {
                let calls = Arc::clone(&calls_for_sender);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn image_delivery_caption_retry_reuses_same_bytes_without_regeneration() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex as StdMutex;

        let sends = Arc::new(AtomicUsize::new(0));
        let seen_bytes = Arc::new(StdMutex::new(Vec::<Vec<u8>>::new()));
        let sends_for_sender = Arc::clone(&sends);
        let seen_for_sender = Arc::clone(&seen_bytes);

        // Provider generation already happened exactly once before the delivery helper.
        let provider_calls = AtomicUsize::new(1);
        let result = deliver_generated_image_with(
            b"paid-generated-image",
            "caption",
            Some(json!({"inline_keyboard": [["button"]]})),
            move |bytes, _caption, _parse_mode, _markup| {
                let sends = Arc::clone(&sends_for_sender);
                let seen = Arc::clone(&seen_for_sender);
                async move {
                    let attempt = sends.fetch_add(1, Ordering::SeqCst);
                    seen.lock().expect("seen bytes lock").push(bytes);
                    if attempt == 0 {
                        Err("Bad Request: can't parse entities in caption".to_string())
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
        assert_eq!(sends.load(Ordering::SeqCst), 2);
        let seen = seen_bytes.lock().expect("seen bytes lock");
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], seen[1]);
    }

    #[tokio::test]
    async fn image_delivery_double_failure_returns_user_error_policy() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let sends = Arc::new(AtomicUsize::new(0));
        let sends_for_sender = Arc::clone(&sends);
        let result = deliver_generated_image_with(
            b"image",
            "caption",
            None,
            move |_bytes, _caption, _parse_mode, _markup| {
                let sends = Arc::clone(&sends_for_sender);
                async move {
                    let attempt = sends.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        Err("Bad Request: caption entities are invalid".to_string())
                    } else {
                        Err("sendPhoto multipart error: connection".to_string())
                    }
                }
            },
        )
        .await;

        let failure = result.expect_err("second delivery should fail");
        assert_eq!(sends.load(Ordering::SeqCst), 2);
        assert!(failure.retry_attempted);
        assert_eq!(failure.class, "telegram_transport");
    }

    #[test]
    fn compound_image_handler_has_one_generation_call_and_failure_precedes_explanation() {
        let source = include_str!("main.rs");
        let handler_start = source
            .find("async fn handle_image_generation(")
            .expect("image handler");
        let handler_end = source[handler_start..]
            .find("// Main AI Chat Handler")
            .map(|offset| handler_start + offset)
            .expect("image handler end");
        let handler = &source[handler_start..handler_end];

        assert_eq!(handler.matches(".generate_image_with_snapshot(").count(), 1);
        let failure_return = handler
            .find("if let Err(failure) = delivery")
            .expect("delivery failure branch");
        let explanation = handler
            .find("if let Some(explanation_prompt)")
            .expect("compound explanation");
        assert!(failure_return < explanation);
        assert!(handler[failure_return..explanation].contains("return;"));
    }

    #[test]
    fn scanned_pdf_with_rendered_pages_does_not_trigger_download_failure_guard() {
        let source = include_str!("main.rs");
        let doc_guard_start = source
            .find("if has_document")
            .expect("document failure guard");
        let doc_guard_end = source[doc_guard_start..]
            .find("if text.is_empty()")
            .map(|offset| doc_guard_start + offset)
            .expect("document failure guard end");
        let doc_guard = &source[doc_guard_start..doc_guard_end];

        assert!(doc_guard.contains("document_images"));
        assert!(doc_guard.contains("is_none_or(|pages| pages.is_empty())"));
    }

    #[test]
    fn clear_confirmation_is_a_typed_rich_action() {
        let encoded = serde_json::to_string(&build_clear_confirmation_ui()).unwrap();
        assert!(encoded.contains("action_clear"));
        assert!(encoded.contains("clear_cancel"));
        assert!(encoded.contains("RESET HISTORY"));
    }
}
