#![allow(dead_code)]

use chrono::Local;
use futures_util::StreamExt;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex, RwLock};
use tracing::{error, warn};

use crate::attachments::{
    decode_user_content, delete_attachment_refs, delete_session_attachments, encode_user_content,
    load_attachment, persist_attachment,
};
use crate::bot::url_policy::is_unsafe_remote_ip;
use crate::util::{truncate_chars, truncate_chars_with_ellipsis};

use super::http::{is_retryable_status, retry_delay, MAX_PROVIDER_ATTEMPTS};
use super::stream::{SseDecoder, StreamEvent};
use crate::timeline::{ExecutionTimeline, ProgressActivity, ProgressState};

use super::capability::model_metadata_key;
pub use super::capability::{ModelCapability, ModelMetadata};
pub use super::routing::{
    GenerationModelSnapshot, ModelRole, ModelRoute, ModelRoutingConfig, ResolvedModelRoute,
    RouteOrigin,
};

use super::storage::{
    append_session_messages_db_async, create_session_and_activate_db_async,
    ensure_session_identity_v2_db_async, load_active_session_id_db_async, load_sessions_db_async,
    remove_session_transaction_db_async, replace_session_messages_if_revision_db_async,
    save_session_metadata_db_async, switch_active_session_db_async,
};
pub use super::storage::{
    load_app_setting, load_capability_registry, load_model_routing, load_provider_store,
    save_app_setting, save_provider_store, CapabilityKind, CapabilityRecord, CapabilityRegistry,
    CapabilityState, ChatMessage, ChatSession, ProbeEvent, ProbeOutcome, ProviderConfig,
    ProviderStore,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessageItem {
    pub index: usize,
    pub role: String,
    pub preview: String,
    pub chars: usize,
    pub tokens: usize,
}

struct SpecialistObservationInput<'a> {
    prompt: &'a str,
    image_bytes: Option<&'a [u8]>,
    document_images: Option<&'a [Vec<u8>]>,
    mime_type: Option<&'a str>,
    video_bytes: Option<&'a [u8]>,
    video_mime: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextStats {
    pub session_name: String,
    pub session_id: usize,
    pub created_at: String,
    pub model_name: String,
    pub capabilities: ModelCapability,
    pub limit_tokens: usize,
    pub limit_str: String,
    pub total_messages: usize,
    pub total_turns: usize,
    pub attachment_count: usize,
    pub total_tokens: usize,
    pub output_reserve_tokens: usize,
    pub total_chars: usize,
    pub usage_pct: f64,
    pub progress_bar: String,
    pub messages_breakdown: Vec<ContextMessageItem>,
}

fn estimate_text_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4).max(1)
}

fn estimate_stored_content_tokens(content: &Value) -> usize {
    if let Some(persisted) = decode_user_content(content) {
        let media_cost = persisted.attachments.len().saturating_mul(1_500);
        return estimate_text_tokens(&persisted.text).saturating_add(media_cost);
    }
    match content {
        Value::String(text) => estimate_text_tokens(text),
        value => estimate_text_tokens(&value.to_string()),
    }
}

pub fn resolve_audio_file_and_mime(
    mime_type: Option<&str>,
    suggested_filename: Option<&str>,
) -> (&'static str, String) {
    let clean_mime = mime_type.unwrap_or("").trim().to_ascii_lowercase();
    let (mime_str, ext) =
        if clean_mime.starts_with("audio/ogg") || clean_mime.starts_with("application/ogg") {
            ("audio/ogg", "ogg")
        } else if clean_mime.starts_with("audio/opus") {
            ("audio/opus", "opus")
        } else if clean_mime.starts_with("audio/mpeg") || clean_mime.starts_with("audio/mp3") {
            ("audio/mpeg", "mp3")
        } else if clean_mime.starts_with("audio/x-m4a") {
            ("audio/x-m4a", "m4a")
        } else if clean_mime.starts_with("audio/mp4") || clean_mime.starts_with("audio/m4a") {
            ("audio/mp4", "m4a")
        } else if clean_mime.starts_with("audio/wav")
            || clean_mime.starts_with("audio/x-wav")
            || clean_mime.starts_with("audio/wave")
        {
            ("audio/wav", "wav")
        } else if clean_mime.starts_with("audio/flac") || clean_mime.starts_with("audio/x-flac") {
            ("audio/flac", "flac")
        } else if clean_mime.starts_with("audio/webm") {
            ("audio/webm", "webm")
        } else {
            let lower_fn = suggested_filename.unwrap_or("").to_ascii_lowercase();
            if lower_fn.ends_with(".ogg") || lower_fn.ends_with(".oga") {
                ("audio/ogg", "ogg")
            } else if lower_fn.ends_with(".opus") {
                ("audio/opus", "opus")
            } else if lower_fn.ends_with(".mp3") {
                ("audio/mpeg", "mp3")
            } else if lower_fn.ends_with(".m4a") {
                ("audio/mp4", "m4a")
            } else if lower_fn.ends_with(".wav") {
                ("audio/wav", "wav")
            } else if lower_fn.ends_with(".flac") {
                ("audio/flac", "flac")
            } else if lower_fn.ends_with(".webm") {
                ("audio/webm", "webm")
            } else {
                ("application/octet-stream", "bin")
            }
        };

    let filename = if let Some(suggested) = suggested_filename {
        let trimmed = suggested.trim();
        let safe_name: String = trimmed
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .take(64)
            .collect();
        if safe_name.to_ascii_lowercase().ends_with(&format!(".{ext}")) {
            safe_name
        } else if !safe_name.is_empty() && safe_name != "." {
            format!("{safe_name}.{ext}")
        } else {
            format!("audio.{ext}")
        }
    } else {
        format!("audio.{ext}")
    };

    (mime_str, filename)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeAudioFormatError {
    Unknown,
}

impl std::fmt::Display for NativeAudioFormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(
                formatter,
                "audio format is unknown; Xiao will not guess a native input_audio format"
            ),
        }
    }
}

fn native_audio_input_format(
    mime_type: Option<&str>,
    file_name: Option<&str>,
) -> Result<&'static str, NativeAudioFormatError> {
    let clean_mime = mime_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    let mime_result = match clean_mime.as_str() {
        "" => None,
        "audio/mpeg" | "audio/mp3" => Some(Ok("mp3")),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some(Ok("wav")),
        "audio/ogg" | "application/ogg" => Some(Ok("ogg")),
        "audio/opus" => Some(Ok("opus")),
        "audio/mp4" => Some(Ok("mp4")),
        "audio/m4a" | "audio/x-m4a" | "audio/aac" => Some(Ok("m4a")),
        "audio/flac" | "audio/x-flac" => Some(Ok("flac")),
        "audio/webm" => Some(Ok("webm")),
        _ => None,
    };
    if let Some(result) = mime_result {
        return result;
    }

    let lower_name = file_name.unwrap_or("").trim().to_ascii_lowercase();
    if lower_name.ends_with(".mp3") {
        Ok("mp3")
    } else if lower_name.ends_with(".wav") {
        Ok("wav")
    } else if lower_name.ends_with(".ogg") || lower_name.ends_with(".oga") {
        Ok("ogg")
    } else if lower_name.ends_with(".opus") {
        Ok("opus")
    } else if lower_name.ends_with(".m4a") || lower_name.ends_with(".aac") {
        Ok("m4a")
    } else if lower_name.ends_with(".mp4") {
        Ok("mp4")
    } else if lower_name.ends_with(".flac") {
        Ok("flac")
    } else if lower_name.ends_with(".webm") {
        Ok("webm")
    } else {
        Err(NativeAudioFormatError::Unknown)
    }
}

fn native_audio_input_part(
    bytes: &[u8],
    mime_type: Option<&str>,
    file_name: Option<&str>,
) -> Result<Value, NativeAudioFormatError> {
    use base64::Engine;
    let format = native_audio_input_format(mime_type, file_name)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(json!({
        "type": "input_audio",
        "input_audio": {"data": encoded, "format": format}
    }))
}

fn historical_native_audio_input_part(
    bytes: &[u8],
    mime_type: &str,
    file_name: Option<&str>,
) -> Result<Value, NativeAudioFormatError> {
    native_audio_input_part(bytes, Some(mime_type), file_name)
}

fn resolved_runtime_media_mime(
    mime_type: Option<&str>,
    expected_prefix: &str,
    media_name: &str,
) -> Result<String, String> {
    let normalized = mime_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if normalized.starts_with(expected_prefix) {
        Ok(normalized)
    } else {
        Err(format!(
            "{media_name} MIME type is unknown or does not match the media kind; refusing to invent media metadata"
        ))
    }
}

fn media_data_url(
    bytes: &[u8],
    mime_type: Option<&str>,
    expected_prefix: &str,
    media_name: &str,
) -> Result<String, String> {
    use base64::Engine;
    let mime_type = resolved_runtime_media_mime(mime_type, expected_prefix, media_name)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime_type};base64,{encoded}"))
}

fn resolved_audio_persistence_mime(mime_type: Option<&str>, file_name: Option<&str>) -> String {
    let normalized = mime_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if normalized.starts_with("audio/") {
        normalized
    } else {
        resolve_audio_file_and_mime(None, file_name).0.to_string()
    }
}

const AI_PROVIDER_CONNECT_TIMEOUT_ENV: &str = "AI_PROVIDER_CONNECT_TIMEOUT_SECS";
const IMAGE_PROVIDER_CONNECT_TIMEOUT_ENV: &str = "IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS";
const IMAGE_GENERATION_TIMEOUT_ENV: &str = "IMAGE_GENERATION_TIMEOUT_SECS";
const IMAGE_DOWNLOAD_TIMEOUT_ENV: &str = "IMAGE_DOWNLOAD_TIMEOUT_SECS";

const MAX_GENERATED_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_PROVIDER_JSON_BYTES: usize = 32 * 1024 * 1024;
const MAX_STREAM_VISIBLE_BYTES: usize = 8 * 1024 * 1024;
const MAX_STREAM_REASONING_BYTES: usize = 8 * 1024 * 1024;
const MAX_STREAM_WIRE_BYTES: usize = 32 * 1024 * 1024;

fn push_bounded(target: &mut String, chunk: &str, max_bytes: usize) -> bool {
    if target.len().saturating_add(chunk.len()) > max_bytes {
        return false;
    }
    target.push_str(chunk);
    true
}

fn require_verified_capability(
    capability: Option<&CapabilityRecord>,
    kind: CapabilityKind,
    capability_name: &str,
) -> Result<(), String> {
    match capability
        .map(|record| record.effective_state_for(kind))
        .unwrap_or(CapabilityState::Unknown)
    {
        CapabilityState::Supported => Ok(()),
        CapabilityState::Unsupported => {
            Err(format!("{capability_name} tidak didukung oleh model aktif"))
        }
        CapabilityState::Unknown => Err(format!(
            "capability {capability_name} belum terverifikasi atau evidence sudah stale; XiaoAI menolak input ini sampai probe/metadata fresh mengonfirmasi dukungan"
        )),
    }
}

fn history_attachment_authorized(
    capability: Option<&CapabilityRecord>,
    attachment_kind: &str,
) -> bool {
    let kind = match attachment_kind {
        "image" | "document_page" => CapabilityKind::ImageInput,
        "audio" => CapabilityKind::AudioInput,
        "video" => CapabilityKind::VideoInput,
        _ => return false,
    };
    capability.is_some_and(|record| record.effective_state_for(kind) == CapabilityState::Supported)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioExecutionMode {
    Native,
    Transcription,
}

fn select_audio_execution_mode(
    capability: &CapabilityRecord,
    inherited_main: bool,
    mime_type: Option<&str>,
    file_name: Option<&str>,
) -> Result<AudioExecutionMode, String> {
    let native = capability.effective_state_for(CapabilityKind::AudioInput);
    let transcription = capability.effective_state_for(CapabilityKind::AudioTranscription);
    let native_format = native_audio_input_format(mime_type, file_name);

    if inherited_main && native == CapabilityState::Supported && native_format.is_ok() {
        Ok(AudioExecutionMode::Native)
    } else if transcription == CapabilityState::Supported {
        Ok(AudioExecutionMode::Transcription)
    } else if inherited_main && native == CapabilityState::Supported {
        Err(format!(
            "Main Model supports native audio, but this file cannot be represented safely by Xiao's native input_audio contract ({}) and AudioTranscription is not fresh Supported.",
            native_format.unwrap_err()
        ))
    } else {
        Err(
            "Audio STT route has no fresh Supported native-audio or transcription capability."
                .to_string(),
        )
    }
}

fn generation_revision_matches(
    session: Option<&ChatSession>,
    session_id: usize,
    revision: u64,
) -> bool {
    session.is_some_and(|session| session.id == session_id && session.revision == revision)
}

fn validate_generated_image_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("generated image response was empty".to_string());
    }
    if bytes.len() > MAX_GENERATED_IMAGE_BYTES {
        return Err("generated image exceeded XiaoAI byte limits".to_string());
    }
    let supported = bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xff, 0xd8, 0xff])
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        || (bytes.len() >= 8 && &bytes[4..8] == b"ftyp");
    if !supported {
        return Err("generated image response has an unsupported file signature".to_string());
    }
    Ok(())
}

pub(super) fn decode_generated_image_base64(
    encoded: &str,
) -> Result<Vec<u8>, ImageGenerationError> {
    use base64::Engine;
    let max_encoded_len = MAX_GENERATED_IMAGE_BYTES
        .saturating_mul(4)
        .div_ceil(3)
        .saturating_add(8);
    if encoded.len() > max_encoded_len {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorKind::InvalidImage,
            "Provider mengembalikan base64 gambar melebihi batas Xiao.",
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| {
            ImageGenerationError::new(
                ImageGenerationErrorKind::InvalidBase64,
                "Provider mengembalikan base64 gambar yang rusak.",
            )
        })?;
    validate_generated_image_bytes(&bytes).map_err(|error| {
        ImageGenerationError::new(ImageGenerationErrorKind::InvalidImage, error)
    })?;
    Ok(bytes)
}

fn parse_generated_image_url(url: &str) -> Result<url::Url, ImageGenerationError> {
    let parsed = url::Url::parse(url).map_err(|_| {
        ImageGenerationError::new(
            ImageGenerationErrorKind::UnsafeImageUrl,
            "provider returned an invalid image URL",
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorKind::UnsafeImageUrl,
            "provider image URL must use http or https with a host",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorKind::UnsafeImageUrl,
            "provider image URL must not contain embedded credentials",
        ));
    }
    Ok(parsed)
}

fn timeout_image_error(label: &str, timeout: Duration) -> ImageGenerationError {
    ImageGenerationError::new(
        ImageGenerationErrorKind::Timeout,
        format!("{label} melewati batas waktu {} detik.", timeout.as_secs()),
    )
}

fn signal_generation_cancel(sender: Option<GenerationCancelSender>) -> bool {
    sender
        .map(|sender| sender.send(true).is_ok())
        .unwrap_or(false)
}

async fn cancelled_chat_result(
    timeline: Option<&Arc<ExecutionTimeline>>,
) -> (Option<String>, String, bool) {
    if let Some(timeline) = timeline {
        timeline.fail_current("Stopped by user".to_string()).await;
        timeline.stop_ticker();
    }
    (
        None,
        "⏹️ Generasi dihentikan oleh pengguna.".to_string(),
        true,
    )
}

pub(super) async fn download_generated_image(url: &str) -> Result<Vec<u8>, ImageGenerationError> {
    let parsed = parse_generated_image_url(url)?;
    let host = parsed.host_str().ok_or_else(|| {
        ImageGenerationError::new(
            ImageGenerationErrorKind::UnsafeImageUrl,
            "provider image URL has no host",
        )
    })?;
    let port = parsed.port_or_known_default().ok_or_else(|| {
        ImageGenerationError::new(
            ImageGenerationErrorKind::UnsafeImageUrl,
            "provider image URL has no usable port",
        )
    })?;

    let resolved = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| {
            ImageGenerationError::new(
                ImageGenerationErrorKind::UnsafeImageUrl,
                "provider image host could not be resolved",
            )
        })?
        .collect::<Vec<_>>();
    if resolved.is_empty() || resolved.iter().any(|addr| is_unsafe_remote_ip(addr.ip())) {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorKind::UnsafeImageUrl,
            "provider image URL resolved to a blocked network address",
        ));
    }

    let client = reqwest::Client::builder()
        .connect_timeout(timeout_from_env(IMAGE_PROVIDER_CONNECT_TIMEOUT_ENV, 10))
        .timeout(timeout_from_env(IMAGE_DOWNLOAD_TIMEOUT_ENV, 30))
        // Redirect targets cannot be revalidated against the DNS/IP policy
        // above without a custom resolver. Disable redirects to prevent a
        // trusted public URL from pivoting into a private network address.
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve(host, resolved[0])
        .build()
        .map_err(|_| {
            ImageGenerationError::new(
                ImageGenerationErrorKind::Provider,
                "failed to build bounded image downloader",
            )
        })?;
    let response = client.get(parsed).send().await.map_err(|error| {
        if error.is_timeout() {
            ImageGenerationError::new(
                ImageGenerationErrorKind::DownloadTimeout,
                "provider image download timed out",
            )
        } else {
            ImageGenerationError::new(
                ImageGenerationErrorKind::Provider,
                "provider image download failed",
            )
        }
    })?;
    if !response.status().is_success() {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorKind::HttpStatus,
            format!(
                "provider image download returned status {}",
                response.status().as_u16()
            ),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_GENERATED_IMAGE_BYTES as u64)
    {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorKind::InvalidImage,
            "provider image exceeded XiaoAI byte limits",
        ));
    }
    if !response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("image/"))
    {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorKind::InvalidImage,
            "provider image URL did not return an image content type",
        ));
    }

    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            ImageGenerationError::new(
                ImageGenerationErrorKind::Provider,
                "provider image stream failed",
            )
        })?;
        if bytes.len().saturating_add(chunk.len()) > MAX_GENERATED_IMAGE_BYTES {
            return Err(ImageGenerationError::new(
                ImageGenerationErrorKind::InvalidImage,
                "provider image exceeded XiaoAI byte limits",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    validate_generated_image_bytes(&bytes).map_err(|error| {
        ImageGenerationError::new(ImageGenerationErrorKind::InvalidImage, error)
    })?;
    Ok(bytes)
}

async fn read_bounded_response_bytes(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("provider response exceeded {max_bytes} bytes"));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "provider response stream failed".to_string())?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("provider response exceeded {max_bytes} bytes"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_bounded_json(response: reqwest::Response) -> Result<Value, String> {
    let bytes = read_bounded_response_bytes(response, MAX_PROVIDER_JSON_BYTES).await?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid provider JSON: {error}"))
}

fn canonical_persisted_prompt<'a>(canonical: Option<&'a str>, runtime_prompt: &'a str) -> &'a str {
    canonical.unwrap_or(runtime_prompt)
}

fn specialist_chat_payload(model: &str, content: Vec<Value>) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "stream": false,
        "max_tokens": 1200
    })
}

fn external_image_fallback_enabled(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("pollinations")
}

fn bounded_timeout_secs(raw: Option<&str>, default_secs: u64) -> u64 {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_secs)
        .min(600)
}

fn provider_url(endpoint: &str, path: &str) -> String {
    format!(
        "{}/{}",
        endpoint.trim().trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn max_output_tokens_for_model(model: &str) -> usize {
    let lower = model.to_ascii_lowercase();
    if lower.contains("claude") {
        64_000
    } else if lower.contains("gemini")
        || ["o1", "o3", "gpt-4o", "gpt-5", "sol", "terra", "luna"]
            .iter()
            .any(|needle| lower.contains(needle))
    {
        65_536
    } else {
        16_384
    }
}

type GenerationKey = (i64, i64);
type GenerationCancelSender = watch::Sender<bool>;
type ActiveGenerations = Arc<RwLock<HashMap<GenerationKey, GenerationCancelSender>>>;

pub struct GenerationInput<'a> {
    pub prompt: &'a str,
    pub canonical_prompt: Option<&'a str>,
    pub media_to_main: bool,
    pub timeline: Option<&'a Arc<ExecutionTimeline>>,
    pub image_bytes: Option<Vec<u8>>,
    pub document_images: Option<Vec<Vec<u8>>>,
    pub mime_type: Option<&'a str>,
    pub doc_text: Option<&'a str>,
    pub doc_name: Option<&'a str>,
    pub audio_bytes: Option<Vec<u8>>,
    pub audio_mime: Option<&'a str>,
    pub video_bytes: Option<Vec<u8>>,
    pub video_mime: Option<&'a str>,
    pub video_duration: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageGenerationProtocol {
    OpenAiImages,
}

impl ImageGenerationProtocol {
    fn endpoint(self, base: &str) -> String {
        match self {
            Self::OpenAiImages => provider_url(base, "images/generations"),
        }
    }

    fn payload(self, model: &str, prompt: &str, width: usize, height: usize) -> Value {
        match self {
            Self::OpenAiImages => json!({
                "model": model,
                "prompt": prompt,
                "n": 1,
                "size": format!("{width}x{height}"),
                "response_format": "b64_json"
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageGenerationErrorKind {
    CapabilityUnknown,
    CapabilityUnsupported,
    RouteDisabled,
    ProviderNotFound,
    ModelNotFound,
    Timeout,
    Auth,
    RateLimited,
    HttpStatus,
    ProtocolMismatch,
    InvalidResponse,
    InvalidBase64,
    InvalidImage,
    UnsafeImageUrl,
    DownloadTimeout,
    Cancelled,
    FallbackDisabled,
    Provider,
}

#[derive(Debug, Clone)]
pub struct ImageGenerationError {
    pub kind: ImageGenerationErrorKind,
    pub message: String,
}

impl ImageGenerationError {
    fn new(kind: ImageGenerationErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

fn classify_image_route_error(message: &str) -> ImageGenerationErrorKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("disabled") {
        ImageGenerationErrorKind::RouteDisabled
    } else if lower.contains("provider") && lower.contains("not found") {
        ImageGenerationErrorKind::ProviderNotFound
    } else if lower.contains("model")
        && (lower.contains("not found") || lower.contains("no longer present"))
    {
        ImageGenerationErrorKind::ModelNotFound
    } else if lower.contains("unsupported") {
        ImageGenerationErrorKind::CapabilityUnsupported
    } else {
        ImageGenerationErrorKind::CapabilityUnknown
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub provider_name: String,
    pub model: String,
    pub used_external_fallback: bool,
    pub primary_failure: Option<String>,
}

fn timeout_from_env(key: &str, default_secs: u64) -> Duration {
    let raw = std::env::var(key).ok();
    Duration::from_secs(bounded_timeout_secs(raw.as_deref(), default_secs))
}

#[derive(Clone)]
pub struct AIChatService {
    pub(super) client: Client,
    user_sessions: Arc<RwLock<HashMap<i64, Vec<ChatSession>>>>,
    active_session_id: Arc<RwLock<HashMap<i64, usize>>>,
    generation_locks: Arc<RwLock<HashMap<i64, Arc<Mutex<()>>>>>,
    session_locks: Arc<RwLock<HashMap<i64, Arc<Mutex<()>>>>>,
    active_generations: ActiveGenerations,
    pub user_waiting_rename: Arc<RwLock<HashMap<i64, usize>>>,
    pub user_rename_msg_id: Arc<RwLock<HashMap<i64, i64>>>,
    pub user_session_msg_id: Arc<RwLock<HashMap<i64, i64>>>,
    pub(super) provider_store: Arc<RwLock<ProviderStore>>,
    pub(super) capability_registry: Arc<RwLock<CapabilityRegistry>>,
    pub(super) model_routing: Arc<RwLock<ModelRoutingConfig>>,
    pub user_wizard_state: Arc<RwLock<HashMap<i64, HashMap<String, String>>>>,
    pub model_metadata: Arc<RwLock<HashMap<String, ModelMetadata>>>,
    pub model_picker_query: Arc<RwLock<HashMap<i64, String>>>,
}

#[derive(Debug, Clone, Default)]
pub struct PendingToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl AIChatService {
    pub fn new() -> Self {
        for key in [
            "BOT_TOKEN",
            "AI_ENDPOINT",
            "AI_API_KEY",
            "AI_MODEL",
            "OWNER_USER_ID",
            "ALLOWED_CHAT_IDS",
            "IMAGE_FALLBACK_PROVIDER",
        ] {
            if load_app_setting(key).is_none() {
                if let Ok(value) = std::env::var(key) {
                    if !value.trim().is_empty() {
                        if let Err(error) = save_app_setting(key, &value) {
                            eprintln!(
                                "[WARN] Failed to migrate environment setting {key}: {error}"
                            );
                        }
                    }
                }
            }
        }
        let provider_store = load_provider_store();
        let capability_registry = load_capability_registry();
        let model_routing = load_model_routing();
        let client = Client::builder()
            .connect_timeout(timeout_from_env(AI_PROVIDER_CONNECT_TIMEOUT_ENV, 10))
            .timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            user_sessions: Arc::new(RwLock::new(HashMap::new())),
            active_session_id: Arc::new(RwLock::new(HashMap::new())),
            generation_locks: Arc::new(RwLock::new(HashMap::new())),
            session_locks: Arc::new(RwLock::new(HashMap::new())),
            active_generations: Arc::new(RwLock::new(HashMap::new())),
            user_waiting_rename: Arc::new(RwLock::new(HashMap::new())),
            user_rename_msg_id: Arc::new(RwLock::new(HashMap::new())),
            user_session_msg_id: Arc::new(RwLock::new(HashMap::new())),
            provider_store: Arc::new(RwLock::new(provider_store)),
            capability_registry: Arc::new(RwLock::new(capability_registry)),
            model_routing: Arc::new(RwLock::new(model_routing)),
            user_wizard_state: Arc::new(RwLock::new(HashMap::new())),
            model_metadata: Arc::new(RwLock::new(HashMap::new())),
            model_picker_query: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // ==========================================
    // Provider Management
    // ==========================================
    //
    // XiaoAI is intentionally single-owner. Provider configuration therefore
    // has one owner-global source of truth instead of a pseudo multi-user cache
    // layered over global SQLite settings.

    async fn rehydrate_history_content(
        &self,
        user_id: i64,
        session_id: usize,
        value: &Value,
        capability: Option<&CapabilityRecord>,
    ) -> Value {
        let Some(persisted) = decode_user_content(value) else {
            return value.clone();
        };

        let mut text = persisted.text;
        let mut parts = Vec::new();
        let mut total_loaded = 0usize;
        for attachment in persisted.attachments {
            let allowed = history_attachment_authorized(capability, attachment.kind.as_str());
            if !allowed {
                text.push_str(&format!(
                    "\n[Attachment '{}' omitted because current model capability is unsupported/unknown.]",
                    attachment.name.as_deref().unwrap_or(&attachment.kind)
                ));
                continue;
            }

            let bytes = match load_attachment(user_id, session_id, &attachment).await {
                Ok(bytes) => bytes,
                Err(err) => {
                    warn!("Unable to reload persisted attachment: {err}");
                    text.push_str("\n[Previously attached media is no longer available.]");
                    continue;
                }
            };
            total_loaded = total_loaded.saturating_add(bytes.len());
            if total_loaded > 12 * 1024 * 1024 {
                text.push_str(
                    "\n[Older attachments omitted because the history media budget was reached.]",
                );
                break;
            }

            match attachment.kind.as_str() {
                "image" | "document_page" => {
                    match media_data_url(
                        &bytes,
                        Some(&attachment.mime_type),
                        "image/",
                        "historical image",
                    ) {
                        Ok(data_url) => parts.push(json!({
                            "type": "image_url",
                            "image_url": {
                                "url": data_url,
                                "detail": "auto"
                            }
                        })),
                        Err(error) => text.push_str(&format!(
                            "\n[Historical image '{}' omitted: {error}.]",
                            attachment.name.as_deref().unwrap_or("image")
                        )),
                    }
                }
                "audio" => {
                    match historical_native_audio_input_part(
                        &bytes,
                        &attachment.mime_type,
                        attachment.name.as_deref(),
                    ) {
                        Ok(part) => parts.push(part),
                        Err(error) => {
                            text.push_str(&format!(
                                "\n[Historical audio '{}' omitted from native history: {error}.]",
                                attachment.name.as_deref().unwrap_or("audio")
                            ));
                        }
                    }
                }
                "video" => {
                    match media_data_url(
                        &bytes,
                        Some(&attachment.mime_type),
                        "video/",
                        "historical video",
                    ) {
                        Ok(data_url) => parts.push(json!({
                            "type": "image_url",
                            "image_url": {
                                "url": data_url
                            }
                        })),
                        Err(error) => text.push_str(&format!(
                            "\n[Historical video '{}' omitted: {error}.]",
                            attachment.name.as_deref().unwrap_or("video")
                        )),
                    }
                }
                _ => {}
            }
        }

        if parts.is_empty() {
            Value::String(text)
        } else {
            let mut content = vec![json!({"type": "text", "text": text})];
            content.extend(parts);
            Value::Array(content)
        }
    }

    async fn session_lock(&self, user_id: i64) -> Arc<Mutex<()>> {
        if let Some(lock) = self.session_locks.read().await.get(&user_id).cloned() {
            return lock;
        }
        let mut locks = self.session_locks.write().await;
        locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn get_sessions(&self, user_id: i64) -> Vec<ChatSession> {
        if let Some(list) = self.user_sessions.read().await.get(&user_id).cloned() {
            return list;
        }

        let init_lock = self.session_lock(user_id).await;
        let _guard = init_lock.lock().await;
        if let Some(list) = self.user_sessions.read().await.get(&user_id).cloned() {
            return list;
        }

        let mut existing = load_sessions_db_async(user_id).await;
        if existing.is_empty() {
            let now_str = Local::now().format("%d %b %H:%M").to_string();
            let Some(session) = create_session_and_activate_db_async(
                user_id,
                format!("Session {now_str}"),
                now_str,
            )
            .await
            else {
                warn!("Session initialization deferred because durable creation failed");
                return Vec::new();
            };
            existing.push(session);
        }
        if !ensure_session_identity_v2_db_async(user_id, existing.clone()).await {
            warn!("Session identity migration/check could not be persisted");
        }
        self.user_sessions
            .write()
            .await
            .insert(user_id, existing.clone());
        existing
    }

    pub async fn get_active_session_id(&self, user_id: i64) -> Option<usize> {
        let sessions = self.get_sessions(user_id).await;
        if sessions.is_empty() {
            return None;
        }
        if let Some(id) = self.active_session_id.read().await.get(&user_id).copied() {
            if sessions.iter().any(|session| session.id == id) {
                return Some(id);
            }
        }

        if let Some(stored_id) = load_active_session_id_db_async(user_id)
            .await
            .filter(|id| sessions.iter().any(|session| session.id == *id))
        {
            self.active_session_id
                .write()
                .await
                .insert(user_id, stored_id);
            return Some(stored_id);
        }

        let fallback_id = sessions[0].id;
        match switch_active_session_db_async(user_id, fallback_id).await {
            Some(true) => {
                self.active_session_id
                    .write()
                    .await
                    .insert(user_id, fallback_id);
                Some(fallback_id)
            }
            _ => {
                warn!("Active session fallback was not published because persistence failed");
                None
            }
        }
    }

    pub async fn get_active_session_index(&self, user_id: i64) -> usize {
        let sessions = self.get_sessions(user_id).await;
        let Some(active_id) = self.get_active_session_id(user_id).await else {
            return 0;
        };
        sessions
            .iter()
            .position(|session| session.id == active_id)
            .unwrap_or(0)
    }

    pub async fn get_active_session(&self, user_id: i64) -> Option<ChatSession> {
        let sessions = self.get_sessions(user_id).await;
        let active_id = self.get_active_session_id(user_id).await?;
        sessions
            .iter()
            .find(|session| session.id == active_id)
            .cloned()
    }

    pub async fn create_new_session(
        &self,
        user_id: i64,
        custom_name: Option<&str>,
    ) -> Option<ChatSession> {
        let _ = self.get_sessions(user_id).await;
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let now_str = Local::now().format("%d %b %H:%M").to_string();
        let name = custom_name
            .map(|value| truncate_chars(value.trim(), 60))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("Session {now_str}"));
        let session = create_session_and_activate_db_async(user_id, name, now_str).await?;
        {
            let mut sessions_map = self.user_sessions.write().await;
            sessions_map
                .entry(user_id)
                .or_default()
                .push(session.clone());
        }
        self.active_session_id
            .write()
            .await
            .insert(user_id, session.id);
        Some(session)
    }

    pub async fn switch_session_by_id(&self, user_id: i64, session_id: usize) -> bool {
        let _ = self.get_sessions(user_id).await;
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let exists = self
            .user_sessions
            .read()
            .await
            .get(&user_id)
            .is_some_and(|sessions| sessions.iter().any(|session| session.id == session_id));
        if !exists {
            return false;
        }
        if switch_active_session_db_async(user_id, session_id).await != Some(true) {
            return false;
        }
        self.active_session_id
            .write()
            .await
            .insert(user_id, session_id);
        true
    }

    pub async fn switch_session(&self, user_id: i64, index: usize) -> bool {
        let sessions = self.get_sessions(user_id).await;
        let Some(session_id) = sessions.get(index).map(|session| session.id) else {
            return false;
        };
        self.switch_session_by_id(user_id, session_id).await
    }

    pub async fn remove_session_by_id(&self, user_id: i64, session_id: usize) -> bool {
        let _ = self.get_sessions(user_id).await;
        let _ = self.get_active_session_id(user_id).await;
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let exists = self
            .user_sessions
            .read()
            .await
            .get(&user_id)
            .is_some_and(|sessions| sessions.iter().any(|session| session.id == session_id));
        if !exists {
            return false;
        }
        let now_str = Local::now().format("%d %b %H:%M").to_string();
        let Some(Some(outcome)) = remove_session_transaction_db_async(
            user_id,
            session_id,
            format!("Session {now_str}"),
            now_str,
        )
        .await
        else {
            return false;
        };

        {
            let mut sessions_map = self.user_sessions.write().await;
            if let Some(list) = sessions_map.get_mut(&user_id) {
                list.retain(|session| session.id != session_id);
                if let Some(replacement) = outcome.replacement.clone() {
                    list.push(replacement);
                    list.sort_by_key(|session| session.id);
                }
            } else {
                // Durable success is authoritative. Evicting an absent cache is
                // sufficient; reporting failure here would be a false failure
                // after the destructive transaction already committed.
                warn!("Durable session removal committed while RAM cache was unavailable; cache will rehydrate");
                sessions_map.remove(&user_id);
            }
        }
        self.active_session_id
            .write()
            .await
            .insert(user_id, outcome.new_active_id);
        delete_session_attachments(user_id, session_id).await;
        true
    }

    pub async fn remove_session(&self, user_id: i64, index: usize) -> bool {
        let sessions = self.get_sessions(user_id).await;
        let Some(session_id) = sessions.get(index).map(|session| session.id) else {
            return false;
        };
        self.remove_session_by_id(user_id, session_id).await
    }

    pub async fn rename_session_by_id(
        &self,
        user_id: i64,
        session_id: usize,
        new_name: &str,
    ) -> bool {
        let name = truncate_chars(new_name.trim(), 60);
        if name.is_empty() {
            return false;
        }
        let _ = self.get_sessions(user_id).await;
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let mut candidate = {
            let sessions_map = self.user_sessions.read().await;
            let Some(session) = sessions_map
                .get(&user_id)
                .and_then(|list| list.iter().find(|session| session.id == session_id))
            else {
                return false;
            };
            session.clone()
        };
        candidate.name = name;
        if !save_session_metadata_db_async(user_id, candidate.clone()).await {
            return false;
        }
        let mut sessions_map = self.user_sessions.write().await;
        if let Some(session) = sessions_map
            .get_mut(&user_id)
            .and_then(|list| list.iter_mut().find(|session| session.id == session_id))
        {
            *session = candidate;
        } else {
            warn!("Durable session rename committed while RAM cache changed; evicting cache");
            sessions_map.remove(&user_id);
        }
        true
    }

    pub async fn rename_session(&self, user_id: i64, index: usize, new_name: &str) -> bool {
        let sessions = self.get_sessions(user_id).await;
        let Some(session_id) = sessions.get(index).map(|session| session.id) else {
            return false;
        };
        self.rename_session_by_id(user_id, session_id, new_name)
            .await
    }

    pub async fn clear_history(&self, user_id: i64) -> bool {
        let Some(_) = self.get_active_session_id(user_id).await else {
            return false;
        };
        let session_lock = self.session_lock(user_id).await;
        let _guard = session_lock.lock().await;
        let Some(active_id) = self.active_session_id.read().await.get(&user_id).copied() else {
            return false;
        };
        let mut candidate = {
            let sessions_map = self.user_sessions.read().await;
            let Some(session) = sessions_map
                .get(&user_id)
                .and_then(|list| list.iter().find(|session| session.id == active_id))
            else {
                return false;
            };
            session.clone()
        };
        let expected_revision = candidate.revision;
        candidate.revision = candidate.revision.saturating_add(1);
        candidate.messages.clear();
        match replace_session_messages_if_revision_db_async(
            user_id,
            expected_revision,
            candidate.clone(),
        )
        .await
        {
            Some(true) => {
                let mut sessions_map = self.user_sessions.write().await;
                if let Some(session) = sessions_map
                    .get_mut(&user_id)
                    .and_then(|list| list.iter_mut().find(|session| session.id == active_id))
                {
                    *session = candidate;
                } else {
                    warn!(
                        "Durable history clear committed while RAM cache changed; evicting cache"
                    );
                    sessions_map.remove(&user_id);
                }
                drop(sessions_map);
                delete_session_attachments(user_id, active_id).await;
                true
            }
            Some(false) => {
                warn!("Clear history rejected because session revision changed");
                false
            }
            None => false,
        }
    }

    pub async fn generation_lock(&self, user_id: i64) -> Arc<Mutex<()>> {
        if let Some(lock) = self.generation_locks.read().await.get(&user_id).cloned() {
            return lock;
        }
        let mut locks = self.generation_locks.write().await;
        locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn begin_generation(&self, chat_id: i64, draft_id: i64) -> watch::Receiver<bool> {
        let (sender, receiver) = watch::channel(false);
        self.active_generations
            .write()
            .await
            .insert((chat_id, draft_id), sender);
        receiver
    }

    pub async fn cancel_generation(&self, chat_id: i64, draft_id: i64) -> bool {
        let sender = self
            .active_generations
            .read()
            .await
            .get(&(chat_id, draft_id))
            .cloned();
        signal_generation_cancel(sender)
    }

    pub async fn end_generation(&self, chat_id: i64, draft_id: i64) {
        self.active_generations
            .write()
            .await
            .remove(&(chat_id, draft_id));
    }

    pub async fn cancel_all_generations(&self) {
        let senders: Vec<GenerationCancelSender> = self
            .active_generations
            .read()
            .await
            .values()
            .cloned()
            .collect();
        for sender in senders {
            let _ = sender.send(true);
        }
    }

    pub async fn get_context_stats(&self, user_id: i64) -> ContextStats {
        let active_sess = self
            .get_active_session(user_id)
            .await
            .unwrap_or(ChatSession {
                id: 0,
                name: "Storage unavailable".to_string(),
                messages: Vec::new(),
                created_at: "-".to_string(),
                revision: 0,
            });
        let active_model = self.get_user_model(user_id).await;
        let endpoint = self
            .get_active_provider(user_id)
            .await
            .map(|provider| provider.endpoint)
            .unwrap_or_default();
        let cap = self
            .resolved_model_capability(&endpoint, &active_model)
            .await;
        let limit_tokens = cap.context_limit;
        let limit_str = cap.context_str.clone();

        let mut total_chars = 0;
        let mut msg_stats = Vec::new();

        for (i, m) in active_sess.messages.iter().enumerate() {
            let c_str = match &m.content {
                Value::String(s) => s.clone(),
                value => decode_user_content(value)
                    .map(|persisted| {
                        if persisted.attachments.is_empty() {
                            persisted.text
                        } else {
                            format!(
                                "{}\n[{} persisted attachment(s)]",
                                persisted.text,
                                persisted.attachments.len()
                            )
                        }
                    })
                    .unwrap_or_else(|| value.to_string()),
            };
            let chars = c_str.chars().count();
            let toks = estimate_text_tokens(&c_str);
            total_chars += chars;

            let preview = truncate_chars_with_ellipsis(&c_str, 90);

            msg_stats.push(ContextMessageItem {
                index: i + 1,
                role: m.role.clone(),
                preview,
                chars,
                tokens: toks,
            });
        }

        let attachment_count = active_sess
            .messages
            .iter()
            .filter_map(|message| decode_user_content(&message.content))
            .map(|persisted| persisted.attachments.len())
            .sum();
        let total_tokens = active_sess
            .messages
            .iter()
            .map(|message| estimate_stored_content_tokens(&message.content))
            .sum();
        let mut output_reserve_tokens =
            max_output_tokens_for_model(&active_model).min(limit_tokens.saturating_div(2).max(1));
        if let Some(metadata_limit) = self
            .model_metadata
            .read()
            .await
            .get(&model_metadata_key(&endpoint, &active_model))
            .and_then(|metadata| metadata.max_completion_tokens)
            .filter(|limit| *limit > 0)
        {
            output_reserve_tokens = output_reserve_tokens.min(metadata_limit);
        }
        let usage_pct = ((total_tokens as f64 / limit_tokens.max(1) as f64) * 100.0).min(100.0);

        let mut filled_blocks = (usage_pct / 10.0).floor() as usize;
        if usage_pct > 0.0 && filled_blocks == 0 {
            filled_blocks = 1;
        }
        filled_blocks = filled_blocks.min(10);
        let bar = format!(
            "{}{}",
            "█".repeat(filled_blocks),
            "░".repeat(10 - filled_blocks)
        );

        ContextStats {
            session_name: active_sess.name,
            session_id: active_sess.id,
            created_at: active_sess.created_at,
            model_name: active_model,
            capabilities: cap,
            limit_tokens,
            limit_str,
            total_messages: active_sess.messages.len(),
            total_turns: active_sess.messages.len().div_ceil(2),
            attachment_count,
            total_tokens,
            output_reserve_tokens,
            total_chars,
            usage_pct,
            progress_bar: bar,
            messages_breakdown: msg_stats,
        }
    }

    // ==========================================
    // Audio Transcription (Whisper)
    // ==========================================

    pub async fn transcribe_audio(
        &self,
        _user_id: i64,
        audio_bytes: Vec<u8>,
        file_name: &str,
        mime_type: Option<&str>,
    ) -> (bool, Result<String, String>) {
        let route = match self.resolve_model_route(ModelRole::AudioStt).await {
            Ok(route) => route,
            Err(error) => return (false, Err(error)),
        };
        if route
            .capability
            .effective_state_for(CapabilityKind::AudioTranscription)
            != CapabilityState::Supported
        {
            return (
                false,
                Err(
                    "Audio STT Model menggunakan input audio native dan tidak menyediakan endpoint transkripsi terverifikasi."
                        .to_string(),
                ),
            );
        }
        match self
            .transcribe_audio_resolved(&route, audio_bytes, file_name, mime_type)
            .await
        {
            Ok(text) => (true, Ok(text)),
            Err(error) => (false, Err(error)),
        }
    }

    // ==========================================
    // Streaming Chat Completions & Reasoning
    // ==========================================

    async fn run_specialist_observation(
        &self,
        route: &ResolvedModelRoute,
        input: SpecialistObservationInput<'_>,
    ) -> Result<String, String> {
        use base64::Engine;

        let SpecialistObservationInput {
            prompt,
            image_bytes,
            document_images,
            mime_type,
            video_bytes,
            video_mime,
        } = input;

        let mut content = vec![json!({
            "type": "text",
            "text": if prompt.trim().is_empty() {
                "Observe the supplied media accurately. Return a concise factual observation for another model to use. Do not answer beyond what is visible/audible in the media."
            } else {
                prompt
            }
        })];
        if let Some(pages) = document_images.filter(|pages| !pages.is_empty()) {
            for page in pages.iter().take(8) {
                let encoded = base64::engine::general_purpose::STANDARD.encode(page);
                content.push(json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:image/png;base64,{encoded}"), "detail": "high"}
                }));
            }
        } else if let Some(bytes) = video_bytes {
            let data_url = media_data_url(bytes, video_mime, "video/", "video")?;
            content.push(json!({
                "type": "image_url",
                "image_url": {"url": data_url}
            }));
        } else if let Some(bytes) = image_bytes {
            let data_url = media_data_url(bytes, mime_type, "image/", "image")?;
            content.push(json!({
                "type": "image_url",
                "image_url": {"url": data_url, "detail": "auto"}
            }));
        }

        let url = provider_url(&route.provider.endpoint, "chat/completions");
        let mut request = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .json(&specialist_chat_payload(&route.model, content))
            .timeout(Duration::from_secs(90));
        if !route.provider.api_key.is_empty()
            && !["none", "-", "no", "null"]
                .iter()
                .any(|value| route.provider.api_key.eq_ignore_ascii_case(value))
        {
            request = request.header(
                "Authorization",
                format!("Bearer {}", route.provider.api_key),
            );
        }
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                "specialist request timed out; capability remains unchanged".to_string()
            } else {
                "specialist request failed".to_string()
            }
        })?;
        if !response.status().is_success() {
            return Err(format!(
                "specialist {} / {} returned HTTP {}",
                route.provider.name,
                route.model,
                response.status().as_u16()
            ));
        }
        let body = read_bounded_json(response).await?;
        let content = body
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"));
        let text = if let Some(text) = content.and_then(Value::as_str) {
            text.to_string()
        } else if let Some(parts) = content.and_then(Value::as_array) {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        } else {
            String::new()
        };
        let text = text.trim();
        if text.is_empty() {
            return Err("specialist returned an empty observation".to_string());
        }
        Ok(truncate_chars(text, 12_000))
    }

    async fn transcribe_audio_resolved(
        &self,
        route: &ResolvedModelRoute,
        audio_bytes: Vec<u8>,
        file_name: &str,
        mime_type: Option<&str>,
    ) -> Result<String, String> {
        let (safe_mime, safe_filename) = resolve_audio_file_and_mime(mime_type, Some(file_name));
        let stt_url = provider_url(&route.provider.endpoint, "audio/transcriptions");
        let part = Part::bytes(audio_bytes)
            .file_name(safe_filename)
            .mime_str(safe_mime)
            .map_err(|error| format!("multipart audio error: {error}"))?;
        let form = Form::new()
            .part("file", part)
            .text("model", route.model.clone());
        let mut request = self
            .client
            .post(stt_url)
            .multipart(form)
            .timeout(Duration::from_secs(90));
        if !route.provider.api_key.is_empty()
            && !["none", "-", "no", "null"]
                .iter()
                .any(|value| route.provider.api_key.eq_ignore_ascii_case(value))
        {
            request = request.header(
                "Authorization",
                format!("Bearer {}", route.provider.api_key),
            );
        }
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                "audio transcription timed out; timeout is not Unsupported".to_string()
            } else {
                "audio transcription request failed".to_string()
            }
        })?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            if status == 404 || status == 405 {
                return Err(format!(
                    "Audio STT route {} / {} returned HTTP {}: endpoint /audio/transcriptions is not supported by this provider protocol.",
                    route.provider.name, route.model, status
                ));
            }
            return Err(format!(
                "Audio STT {} / {} returned HTTP {}",
                route.provider.name, route.model, status
            ));
        }
        let body = read_bounded_json(response).await?;
        let text = body
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            return Err("audio transcription returned empty text".to_string());
        }
        Ok(truncate_chars(text, 32_000))
    }

    pub async fn generate_response(
        &self,
        user_id: i64,
        input: GenerationInput<'_>,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> (Option<String>, String, bool) {
        let snapshot = self.generation_model_snapshot().await;
        self.generate_response_with_snapshot(user_id, input, &snapshot, cancel_rx)
            .await
    }

    pub(crate) async fn generate_response_with_snapshot(
        &self,
        user_id: i64,
        input: GenerationInput<'_>,
        snapshot: &GenerationModelSnapshot,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> (Option<String>, String, bool) {
        let GenerationInput {
            prompt,
            canonical_prompt: _,
            media_to_main: _,
            timeline,
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
        } = input;

        if *cancel_rx.borrow() {
            return cancelled_chat_result(timeline).await;
        }

        let main = match Self::resolve_model_route_from_snapshot(snapshot, ModelRole::Main) {
            Ok(route) => route,
            Err(error) => return (None, format!("Main Model is unavailable: {error}"), false),
        };

        let has_vision = image_bytes.is_some()
            || document_images
                .as_ref()
                .is_some_and(|pages| !pages.is_empty());
        let role = if has_vision {
            Some(ModelRole::Vision)
        } else if video_bytes.is_some() {
            Some(ModelRole::Video)
        } else if audio_bytes.is_some() {
            Some(ModelRole::AudioStt)
        } else {
            None
        };

        let Some(role) = role else {
            return self
                .generate_response_on_main(
                    user_id,
                    &main,
                    GenerationInput {
                        prompt,
                        canonical_prompt: None,
                        media_to_main: true,
                        timeline,
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
                    },
                    cancel_rx,
                )
                .await;
        };

        let specialist = match Self::resolve_model_route_from_snapshot(snapshot, role) {
            Ok(route) => route,
            Err(error) => {
                return (
                    None,
                    format!("{} unavailable: {error}", role.display_name()),
                    false,
                )
            }
        };

        let same_as_main =
            specialist.provider.id == main.provider.id && specialist.model == main.model;

        if role == ModelRole::AudioStt {
            let inherited_main = same_as_main && specialist.route_origin == RouteOrigin::MainModel;
            let audio_mode = match select_audio_execution_mode(
                &specialist.capability,
                inherited_main,
                audio_mime,
                doc_name,
            ) {
                Ok(mode) => mode,
                Err(error) => return (None, error, false),
            };

            if audio_mode == AudioExecutionMode::Native {
                return self
                    .generate_response_on_main(
                        user_id,
                        &main,
                        GenerationInput {
                            prompt,
                            canonical_prompt: None,
                            media_to_main: true,
                            timeline,
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
                        },
                        cancel_rx,
                    )
                    .await;
            }

            let Some(bytes) = audio_bytes.clone() else {
                return (None, "Audio input is missing.".to_string(), false);
            };
            let transcript_result = tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        return cancelled_chat_result(timeline).await;
                    }
                    Err("Kanal pembatalan audio ditutup.".to_string())
                }
                result = self.transcribe_audio_resolved(
                    &specialist,
                    bytes,
                    doc_name.unwrap_or("audio"),
                    audio_mime,
                ) => result
            };
            let transcript = match transcript_result {
                Ok(transcript) => transcript,
                Err(error) => return (None, error, false),
            };
            let synthesis_prompt = if prompt.trim().is_empty() {
                format!("Transcript from Audio STT specialist:\n\n{transcript}\n\nRespond to the user based on this transcript.")
            } else {
                format!(
                    "User request:\n{prompt}\n\nTranscript from Audio STT specialist:\n{transcript}\n\nAnswer the user request using the transcript as an execution artifact."
                )
            };
            return self
                .generate_response_on_main(
                    user_id,
                    &main,
                    GenerationInput {
                        prompt: &synthesis_prompt,
                        canonical_prompt: Some(prompt),
                        media_to_main: false,
                        timeline,
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
                    },
                    cancel_rx,
                )
                .await;
        }

        if same_as_main && specialist.route_origin == RouteOrigin::MainModel {
            return self
                .generate_response_on_main(
                    user_id,
                    &main,
                    GenerationInput {
                        prompt,
                        canonical_prompt: None,
                        media_to_main: true,
                        timeline,
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
                    },
                    cancel_rx,
                )
                .await;
        }

        let required_capability = match role {
            ModelRole::Vision => Some((CapabilityKind::ImageInput, "vision/image")),
            ModelRole::Video => Some((CapabilityKind::VideoInput, "video")),
            ModelRole::AudioStt | ModelRole::Main | ModelRole::ImageGeneration => None,
        };
        if let Some((kind, name)) = required_capability {
            if let Err(error) =
                require_verified_capability(Some(&specialist.capability), kind, name)
            {
                return (
                    None,
                    format!(
                        "{} / {} tidak dapat memproses media: {error}.",
                        specialist.provider.name, specialist.model
                    ),
                    false,
                );
            }
        }

        let observation_result = tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    return cancelled_chat_result(timeline).await;
                }
                Err("Kanal pembatalan media ditutup.".to_string())
            }
            result = self.run_specialist_observation(
                &specialist,
                SpecialistObservationInput {
                    prompt,
                    image_bytes: image_bytes.as_deref(),
                    document_images: document_images.as_deref(),
                    mime_type,
                    video_bytes: video_bytes.as_deref(),
                    video_mime,
                },
            ) => result
        };
        let observation = match observation_result {
            Ok(observation) => observation,
            Err(error) => return (None, error, false),
        };
        let synthesis_prompt = format!(
            "User request:\n{}\n\nBounded {} observation from {} / {}:\n{}\n\nUse the observation as an execution artifact. Do not claim access to media beyond it.",
            if prompt.trim().is_empty() { "Analyze the supplied media." } else { prompt },
            role.display_name(),
            specialist.provider.name,
            specialist.model,
            observation
        );
        self.generate_response_on_main(
            user_id,
            &main,
            GenerationInput {
                prompt: &synthesis_prompt,
                canonical_prompt: Some(prompt),
                media_to_main: false,
                timeline,
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
            },
            cancel_rx,
        )
        .await
    }

    async fn generate_response_on_main(
        &self,
        user_id: i64,
        main_route: &ResolvedModelRoute,
        input: GenerationInput<'_>,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> (Option<String>, String, bool) {
        let GenerationInput {
            prompt,
            canonical_prompt,
            media_to_main,
            timeline,
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
        } = input;

        let provider = &main_route.provider;
        let model = &main_route.model;
        let capability = &main_route.capability;

        let required_capability = if media_to_main
            && (document_images
                .as_ref()
                .is_some_and(|pages| !pages.is_empty())
                || image_bytes.is_some())
        {
            require_verified_capability(
                Some(capability),
                CapabilityKind::ImageInput,
                "vision/image",
            )
        } else if media_to_main && video_bytes.is_some() {
            require_verified_capability(Some(capability), CapabilityKind::VideoInput, "video")
        } else if media_to_main && audio_bytes.is_some() {
            require_verified_capability(Some(capability), CapabilityKind::AudioInput, "audio")
        } else {
            Ok(())
        };
        if let Err(reason) = required_capability {
            return (
                None,
                format!(
                    "Endpoint '{}' / model '{}' tidak dapat menerima media ini: {reason}.",
                    provider.name, model
                ),
                false,
            );
        }

        let Some(active_sess) = self.get_active_session(user_id).await else {
            return (
                None,
                "Penyimpanan session sedang tidak tersedia. XiaoAI tidak akan membuat ID session sementara yang berisiko dipakai ulang. Coba lagi setelah storage pulih.".to_string(),
                false,
            );
        };
        let request_session_id = active_sess.id;
        let request_session_revision = active_sess.revision;

        let mut clean_prompt = prompt.trim().to_string();
        if let Some(doc) = doc_text {
            let d_name = doc_name.unwrap_or("Dokumen");
            let doc_header = format!("[Dokumen Terlampir: {d_name}]\n{}\n\n", doc.trim());
            clean_prompt = if clean_prompt.is_empty() {
                format!("{doc_header}Baca, analisis, dan jelaskan isi dokumen ini.")
            } else {
                format!("{doc_header}{clean_prompt}")
            };
        } else if document_images
            .as_ref()
            .is_some_and(|pages| !pages.is_empty())
            && clean_prompt.is_empty()
        {
            let d_name = doc_name.unwrap_or("PDF scan");
            clean_prompt = format!(
                "Baca dan analisis halaman hasil render dari dokumen '{d_name}'. Lakukan OCR visual pada teks yang terlihat dan jelaskan isi dokumen secara akurat."
            );
        } else if video_bytes.is_some() && clean_prompt.is_empty() {
            let dur_str = video_duration
                .map(|d| format!(" ({d} detik)"))
                .unwrap_or_default();
            clean_prompt = format!("Tonton dan analisis rekaman video ini{dur_str} secara mendalam. Jelaskan isi visual, alur peristiwa, teks di layar, dan suara di dalamnya.");
        } else if image_bytes.is_some() && clean_prompt.is_empty() {
            clean_prompt = "Jelaskan dan analisis gambar ini secara detail.".to_string();
        } else if audio_bytes.is_some() && clean_prompt.is_empty() {
            clean_prompt = "Dengarkan rekaman suara ini dan jawab pertanyaan atau instruksi di dalamnya secara lengkap.".to_string();
        }
        let canonical_history_prompt = canonical_prompt.map(str::to_string);

        let resolved_capability = self
            .resolved_model_capability(&provider.endpoint, model)
            .await;
        let metadata_max_completion_tokens = self
            .model_metadata
            .read()
            .await
            .get(&model_metadata_key(&provider.endpoint, model))
            .and_then(|metadata| metadata.max_completion_tokens);
        let mut max_output_tokens = max_output_tokens_for_model(model)
            .min(resolved_capability.context_limit.saturating_div(2).max(1));
        if let Some(limit) = metadata_max_completion_tokens.filter(|limit| *limit > 0) {
            max_output_tokens = max_output_tokens.min(limit);
        }
        let max_prompt_tokens = resolved_capability
            .context_limit
            .saturating_sub(max_output_tokens)
            .saturating_sub(2_048)
            .max(1);
        if estimate_text_tokens(&clean_prompt) > max_prompt_tokens {
            let max_chars = max_prompt_tokens.saturating_mul(4);
            clean_prompt = truncate_chars(&clean_prompt, max_chars);
            clean_prompt.push_str("\n\n[Input dipotong Xiao agar muat di context window model.]");
        }
        let enhanced_prompt = clean_prompt.clone();

        let reserved_tokens = max_output_tokens
            .saturating_add(estimate_text_tokens(&enhanced_prompt))
            .saturating_add(2_048);
        let history_budget = resolved_capability
            .context_limit
            .saturating_sub(reserved_tokens);

        let mut selected_history = Vec::new();
        let mut used_history_tokens = 0usize;
        for message in active_sess.messages.iter().rev().take(50) {
            let estimated = estimate_stored_content_tokens(&message.content).saturating_add(8);
            if !selected_history.is_empty()
                && used_history_tokens.saturating_add(estimated) > history_budget
            {
                break;
            }
            if estimated > history_budget && selected_history.is_empty() {
                continue;
            }
            used_history_tokens = used_history_tokens.saturating_add(estimated);
            selected_history.push(message);
        }
        selected_history.reverse();

        let mut history = Vec::with_capacity(selected_history.len());
        for message in selected_history {
            let content = if message.role == "user" {
                self.rehydrate_history_content(
                    user_id,
                    request_session_id,
                    &message.content,
                    Some(capability),
                )
                .await
            } else {
                message.content.clone()
            };
            history.push(json!({ "role": message.role, "content": content }));
        }

        let mut messages = vec![json!({
            "role": "system",
            "content": "Kamu adalah asisten AI yang cerdas, komunikatif, dan ramah. \
                        Gunakan input multimodal hanya ketika input tersebut benar-benar disediakan dan endpoint/model mendukungnya. \
                        Dokumen Xiao diekstrak menjadi teks bila memungkinkan; PDF scan dapat diberikan sebagai halaman hasil render untuk OCR visual. \
                        Lakukan penalaran secara internal dan berikan hanya jawaban yang berguna bagi pengguna; jangan menampilkan chain-of-thought tersembunyi. \
                        Gunakan gaya bahasa yang alami dan format teks yang elegan. \
                        Jika membuat tabel atau data berkolom, gunakan Markdown Table standar agar Xiao dapat merendernya sebagai tabel Telegram. \
                        Jika diminta menyajikan visual atau media publik yang relevan, kamu dapat menyisipkan format blok media berikut:\n\
                        - Foto: [photo: Judul](https://url-gambar) atau ![Judul](https://url-gambar)\n\
                        - Galeri/Kolase Foto: [collage: Judul](https://url-1, https://url-2)\n\
                        - Slide Foto: [slideshow: Judul](https://url-1, https://url-2)\n\
                        - Audio/Musik: [audio: Judul Lagu](https://url-audio)\n\
                        - Rekaman Suara: [voice: Catatan Suara](https://url-audio)\n\
                        - Video: [video: Judul Video](https://url-video)\n\
                        - Peta/Lokasi: [map: latitude, longitude]\n\
                        - Dokumen: [document: Nama Dokumen](https://url-dokumen)"
        })];

        messages.extend(history);

        if media_to_main {
            if let Some(pages) = document_images.as_ref().filter(|pages| !pages.is_empty()) {
                use base64::Engine;
                let mut content = vec![json!({ "type": "text", "text": enhanced_prompt })];
                for page in pages {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(page);
                    content.push(json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:image/png;base64,{encoded}"),
                            "detail": "high"
                        }
                    }));
                }
                messages.push(json!({ "role": "user", "content": content }));
            } else if let Some(v_bytes) = video_bytes.as_ref() {
                let data_url = match media_data_url(v_bytes, video_mime, "video/", "video") {
                    Ok(data_url) => data_url,
                    Err(error) => return (None, error, false),
                };
                messages.push(json!({
                    "role": "user",
                    "content": [
                        { "type": "text", "text": enhanced_prompt },
                        { "type": "image_url", "image_url": { "url": data_url } }
                    ]
                }));
            } else if let Some(i_bytes) = image_bytes.as_ref() {
                let data_url = match media_data_url(i_bytes, mime_type, "image/", "image") {
                    Ok(data_url) => data_url,
                    Err(error) => return (None, error, false),
                };
                messages.push(json!({
                    "role": "user",
                    "content": [
                        { "type": "text", "text": enhanced_prompt },
                        { "type": "image_url", "image_url": { "url": data_url, "detail": "auto" } }
                    ]
                }));
            } else if let Some(a_bytes) = audio_bytes.as_ref() {
                let audio_part = match native_audio_input_part(a_bytes, audio_mime, doc_name) {
                    Ok(part) => part,
                    Err(error) => {
                        return (
                            None,
                            format!(
                                "Native audio payload was blocked because its format cannot be represented safely: {error}."
                            ),
                            false,
                        )
                    }
                };
                messages.push(json!({
                    "role": "user",
                    "content": [
                        { "type": "text", "text": enhanced_prompt },
                        audio_part
                    ]
                }));
            } else {
                messages.push(json!({
                    "role": "user",
                    "content": enhanced_prompt
                }));
            }
        } else {
            messages.push(json!({
                "role": "user",
                "content": enhanced_prompt
            }));
        }

        let cap_record = self.capability_record(&provider.endpoint, model).await;
        let supports_tools = cap_record
            .as_ref()
            .map(|r| r.effective_state_for(CapabilityKind::Tools) != CapabilityState::Unsupported)
            .unwrap_or(true);

        let url = provider_url(&provider.endpoint, "chat/completions");
        let mut payload = json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if metadata_max_completion_tokens.is_some() {
            payload["max_completion_tokens"] = json!(max_output_tokens);
        } else {
            payload["max_tokens"] = json!(max_output_tokens);
        }

        let use_auth = !provider.api_key.is_empty()
            && !["none", "-", "no"]
                .iter()
                .any(|k| provider.api_key.eq_ignore_ascii_case(k));

        let mut accumulated_raw = String::new();
        let mut accumulated_reasoning = String::new();
        let mut cancelled = false;
        let mut stream_bounded = false;
        let mut stream_interrupted = false;
        let mut has_started_answer = false;

        for turn in 0..2 {
            accumulated_raw.clear();
            accumulated_reasoning.clear();
            let mut accumulated_tool_calls: Vec<PendingToolCall> = Vec::new();
            let mut streamed_wire_bytes = 0usize;
            let mut stream_done = false;

            if turn == 0 && supports_tools {
                payload["tools"] = crate::ai::tools::get_tools_definition();
            } else {
                payload.as_object_mut().map(|m| m.remove("tools"));
            }

            let mut response = None;
            let mut terminal_transport_failure = false;
            for attempt in 0..MAX_PROVIDER_ATTEMPTS {
                let mut req = self
                    .client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .timeout(Duration::from_secs(180));
                if use_auth {
                    req = req.header("Authorization", format!("Bearer {}", provider.api_key));
                }

                let mut send_future = Box::pin(req.send());
                let send_result = tokio::select! {
                    changed = cancel_rx.changed() => {
                        if changed.is_ok() && *cancel_rx.borrow() {
                            if let Some(tl) = timeline {
                                tl.fail_current("Stopped by user".to_string()).await;
                                tl.stop_ticker();
                            }
                            return (None, "⏹️ Generasi dihentikan oleh pengguna.".to_string(), true);
                        }
                        send_future.as_mut().await
                    }
                    result = send_future.as_mut() => result,
                };

                match send_result {
                    Ok(resp)
                        if is_retryable_status(resp.status())
                            && attempt + 1 < MAX_PROVIDER_ATTEMPTS =>
                    {
                        let status = resp.status();
                        let delay = retry_delay(resp.headers(), attempt);
                        drop(resp);
                        warn!(
                            "Transient provider status {}; retrying attempt {}/{}",
                            status.as_u16(),
                            attempt + 2,
                            MAX_PROVIDER_ATTEMPTS
                        );
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {}
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    if let Some(tl) = timeline {
                                        tl.fail_current("Stopped by user".to_string()).await;
                                        tl.stop_ticker();
                                    }
                                    return (None, "⏹️ Generasi dihentikan oleh pengguna.".to_string(), true);
                                }
                            }
                        }
                    }
                    Ok(resp) if resp.status().as_u16() == 400 && payload.get("tools").is_some() => {
                        drop(resp);
                        warn!("Provider rejected tools parameter (HTTP 400); retrying without tools");
                        payload.as_object_mut().unwrap().remove("tools");
                        continue;
                    }
                    Ok(resp) => {
                        response = Some(resp);
                        break;
                    }
                    Err(e) => {
                        let retryable_transport = e.is_timeout() || e.is_connect();
                        if retryable_transport && attempt + 1 < MAX_PROVIDER_ATTEMPTS {
                            let delay =
                                Duration::from_millis(500_u64.saturating_mul(1_u64 << attempt.min(5)));
                            warn!(
                                "Transient provider transport failure; retrying attempt {}/{}",
                                attempt + 2,
                                MAX_PROVIDER_ATTEMPTS
                            );
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {}
                                changed = cancel_rx.changed() => {
                                    if changed.is_ok() && *cancel_rx.borrow() {
                                        if let Some(tl) = timeline {
                                            tl.fail_current("Stopped by user".to_string()).await;
                                            tl.stop_ticker();
                                        }
                                        return (None, "⏹️ Generasi dihentikan oleh pengguna.".to_string(), true);
                                    }
                                }
                            }
                        } else {
                            error!(
                                "Error sending AI completion request: {}",
                                if e.is_timeout() {
                                    "timeout"
                                } else {
                                    "transport failure"
                                }
                            );
                            terminal_transport_failure = true;
                            break;
                        }
                    }
                }
            }

            let Some(resp) = response else {
                if let Some(tl) = timeline {
                    tl.fail_current("Provider connection failed".to_string())
                        .await;
                    tl.sync_draft(true).await;
                }
                return (
                    None,
                    if terminal_transport_failure {
                        "⚠️ Terjadi kendala saat memproses jawaban AI.".to_string()
                    } else {
                        "⚠️ Provider tidak merespons setelah beberapa percobaan.".to_string()
                    },
                    false,
                );
            };

            if !resp.status().is_success() {
                let status_code = resp.status().as_u16();
                drop(resp);
                error!("AI endpoint returned status {status_code}");
                if let Some(tl) = timeline {
                    tl.fail_current(format!("API status {status_code}")).await;
                    tl.sync_draft(true).await;
                }
                return (
                    None,
                    format!("⚠️ Gagal menghubungi AI proxy: {status_code}"),
                    false,
                );
            }

            let mut stream = resp.bytes_stream();
            let mut decoder = SseDecoder::default();

            'streaming: while !stream_done {
                let mut next_future = Box::pin(stream.next());
                let next_item = tokio::select! {
                    changed = cancel_rx.changed() => {
                        if changed.is_ok() && *cancel_rx.borrow() {
                            cancelled = true;
                            None
                        } else {
                            next_future.as_mut().await
                        }
                    }
                    item = next_future.as_mut() => item,
                };

                let Some(item) = next_item else {
                    break;
                };
                let bytes = match item {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        stream_interrupted = true;
                        warn!("AI response stream interrupted");
                        break;
                    }
                };
                streamed_wire_bytes = streamed_wire_bytes.saturating_add(bytes.len());
                if streamed_wire_bytes > MAX_STREAM_WIRE_BYTES {
                    stream_bounded = true;
                    stream_interrupted = true;
                    warn!("AI response exceeded XiaoAI's absolute streamed payload limit");
                    break;
                }

                let events = match decoder.push(&bytes) {
                    Ok(events) => events,
                    Err(error) => {
                        stream_interrupted = true;
                        warn!("AI response SSE decode failed: {error}");
                        break;
                    }
                };
                for event in events {
                    match event {
                        StreamEvent::Done => {
                            stream_done = true;
                            break;
                        }
                        StreamEvent::Json(data) => {
                            let Some(delta) = data
                                .get("choices")
                                .and_then(|choices| choices.get(0))
                                .and_then(|choice| choice.get("delta"))
                            else {
                                continue;
                            };

                            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                                for tc in tool_calls {
                                    let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                                    while accumulated_tool_calls.len() <= index {
                                        accumulated_tool_calls.push(PendingToolCall::default());
                                    }
                                    if let Some(id) = tc.get("id").and_then(Value::as_str) {
                                        accumulated_tool_calls[index].id.push_str(id);
                                    }
                                    if let Some(func) = tc.get("function") {
                                        if let Some(name) = func.get("name").and_then(Value::as_str) {
                                            accumulated_tool_calls[index].name.push_str(name);
                                        }
                                        if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                                            accumulated_tool_calls[index].arguments.push_str(args);
                                        }
                                    }
                                }
                            }

                            if let Some(reasoning_chunk) =
                                delta.get("reasoning_content").and_then(Value::as_str)
                            {
                                if !push_bounded(
                                    &mut accumulated_reasoning,
                                    reasoning_chunk,
                                    MAX_STREAM_REASONING_BYTES,
                                ) {
                                    stream_bounded = true;
                                    stream_interrupted = true;
                                    warn!("AI reasoning exceeded XiaoAI's absolute output limit");
                                    break 'streaming;
                                }
                            }

                            let content_chunk =
                                delta.get("content").and_then(Value::as_str).unwrap_or("");
                            if content_chunk.is_empty() {
                                continue;
                            }
                            if !push_bounded(
                                &mut accumulated_raw,
                                content_chunk,
                                MAX_STREAM_VISIBLE_BYTES,
                            ) {
                                stream_bounded = true;
                                stream_interrupted = true;
                                warn!("AI answer exceeded XiaoAI's absolute output limit");
                                break 'streaming;
                            }

                            let visible_partial =
                                if let Some(close_pos) = accumulated_raw.rfind("</think>") {
                                    accumulated_raw[close_pos + "</think>".len()..].trim()
                                } else if accumulated_raw.contains("<think>") {
                                    ""
                                } else {
                                    accumulated_raw.trim()
                                };

                            if !visible_partial.is_empty() {
                                if !has_started_answer {
                                    has_started_answer = true;
                                    if let Some(tl) = timeline {
                                        tl.add_action("Writing", Some(ProgressActivity::Writing))
                                            .await;
                                    }
                                }
                                if let Some(tl) = timeline {
                                    tl.set_partial_answer(visible_partial).await;
                                    tl.sync_draft(false).await;
                                }
                            }
                        }
                    }
                }
            }

            if !stream_done && !cancelled && !stream_interrupted {
                match decoder.finish() {
                    Ok(events) => {
                        for event in events {
                            if matches!(event, StreamEvent::Done) {
                                stream_done = true;
                            }
                        }
                        if !stream_done {
                            stream_interrupted = true;
                        }
                    }
                    Err(error) => {
                        warn!("AI response SSE final decode failed: {error}");
                        stream_interrupted = true;
                    }
                }
            }

            if turn == 0 && !accumulated_tool_calls.is_empty() && !cancelled {
                let mut tool_results = Vec::new();
                for tc in accumulated_tool_calls.iter() {
                    let name = tc.name.trim();
                    let tool_id = if tc.id.is_empty() {
                        "call_default".to_string()
                    } else {
                        tc.id.clone()
                    };
                    let result = if name == "web_search" {
                        if let Some(tl) = timeline {
                            tl.add_action("Searching", Some(ProgressActivity::Searching))
                                .await;
                            tl.sync_draft(false).await;
                        }
                        let parsed_query = serde_json::from_str::<Value>(&tc.arguments)
                            .ok()
                            .and_then(|v| {
                                v.get("query")
                                    .and_then(Value::as_str)
                                    .map(|s| s.to_string())
                            })
                            .unwrap_or_else(|| tc.arguments.clone());

                        tokio::select! {
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    cancelled = true;
                                }
                                "Pencarian dibatalkan.".to_string()
                            }
                            res = crate::ai::tools::execute_web_search(&parsed_query) => res,
                        }
                    } else if name == "fetch_url" {
                        if let Some(tl) = timeline {
                            tl.add_action("Fetching", Some(ProgressActivity::Fetching))
                                .await;
                            tl.sync_draft(false).await;
                        }
                        let parsed_url = serde_json::from_str::<Value>(&tc.arguments)
                            .ok()
                            .and_then(|v| {
                                v.get("url")
                                    .and_then(Value::as_str)
                                    .map(|s| s.to_string())
                            })
                            .unwrap_or_else(|| tc.arguments.clone());

                        tokio::select! {
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    cancelled = true;
                                }
                                "Pengambilan web dibatalkan.".to_string()
                            }
                            res = crate::ai::tools::fetch_web_content(&parsed_url) => {
                                res.unwrap_or_else(|e| format!("Gagal membaca URL: {e}"))
                            }
                        }
                    } else {
                        format!("Tool '{name}' tidak didukung.")
                    };
                    tool_results.push((tool_id, name.to_string(), tc.arguments.clone(), result));
                    if cancelled {
                        break;
                    }
                }

                if cancelled {
                    break;
                }

                let tool_calls_json = tool_results
                    .iter()
                    .map(|(id, name, args, _)| {
                        json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": args
                            }
                        })
                    })
                    .collect::<Vec<_>>();

                messages.push(json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": tool_calls_json
                }));

                for (id, _, _, res) in &tool_results {
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": res
                    }));
                }

                messages.push(json!({
                    "role": "user",
                    "content": "Berdasarkan data dan ringkasan hasil pencarian web di atas, jawab pertanyaan awal pengguna secara lengkap dan jelas."
                }));

                payload["messages"] = json!(messages);
                payload.as_object_mut().unwrap().remove("tools");

                if let Some(tl) = timeline {
                    tl.add_action("Writing", Some(ProgressActivity::Writing))
                        .await;
                    tl.set_partial_answer("Menyusun jawaban...").await;
                    tl.sync_draft(false).await;
                }

                continue;
            }

            break;
        }

        // Post-process final output
        let mut thinking_text = None;
        let mut answer_text = accumulated_raw.clone();

        if !accumulated_reasoning.is_empty() {
            thinking_text = Some(accumulated_reasoning.trim().to_string());
            answer_text = accumulated_raw.trim().to_string();
        } else {
            if let Ok(think_re) = regex::Regex::new(r"(?s)<think>(.*?)</think>") {
                if let Some(caps) = think_re.captures(&accumulated_raw) {
                    thinking_text = caps.get(1).map(|m| m.as_str().trim().to_string());
                    answer_text = think_re
                        .replace_all(&accumulated_raw, "")
                        .trim()
                        .to_string();
                } else if accumulated_raw.contains("<think>") {
                    let parts: Vec<&str> = accumulated_raw.split("<think>").collect();
                    let before_think = parts[0].trim();
                    let inside_think = parts.get(1).copied().unwrap_or("").trim();
                    thinking_text = (!inside_think.is_empty()).then(|| inside_think.to_string());
                    answer_text = before_think.to_string();
                }
            } else if accumulated_raw.contains("<think>") {
                let parts: Vec<&str> = accumulated_raw.split("<think>").collect();
                let before_think = parts[0].trim();
                let inside_think = parts.get(1).copied().unwrap_or("").trim();
                thinking_text = (!inside_think.is_empty()).then(|| inside_think.to_string());
                answer_text = before_think.to_string();
            }
        }

        if let Ok(tag_clean_re) = regex::Regex::new(r"(?i)</?think>") {
            answer_text = tag_clean_re
                .replace_all(&answer_text, "")
                .trim()
                .to_string();
        } else {
            answer_text = answer_text.trim().to_string();
        }

        if cancelled {
            if answer_text.trim().is_empty() {
                answer_text = "⏹️ Generasi dihentikan oleh pengguna.".to_string();
            } else {
                answer_text.push_str("\n\n_⏹️ Generasi dihentikan oleh pengguna._");
            }
        } else if stream_bounded {
            if answer_text.trim().is_empty() {
                answer_text =
                    "⚠️ Respons provider melewati batas ukuran aman XiaoAI dan dihentikan."
                        .to_string();
            } else {
                answer_text.push_str(
                    "\n\n_⚠️ Respons dihentikan karena melewati batas ukuran aman XiaoAI._",
                );
            }
        } else if stream_interrupted {
            if answer_text.trim().is_empty() {
                answer_text = "⚠️ Stream provider terputus sebelum jawaban diterima.".to_string();
            } else {
                answer_text
                    .push_str("\n\n_⚠️ Stream provider terputus; jawaban mungkin tidak lengkap._");
            }
        } else if answer_text.is_empty() {
            answer_text = "Maaf, respon AI kosong untuk permintaan ini.".to_string();
        }

        if let Some(tl) = timeline {
            if cancelled {
                tl.set_partial_answer(&answer_text).await;
                tl.fail_current("Stopped by user".to_string()).await;
                tl.stop_ticker();
            } else if stream_interrupted {
                tl.set_partial_answer(&answer_text).await;
                tl.fail_current(if stream_bounded {
                    "Provider output exceeded safety limit".to_string()
                } else {
                    "Provider stream interrupted".to_string()
                })
                .await;
                tl.sync_draft(true).await;
            } else {
                if !has_started_answer {
                    tl.add_action("Writing", Some(ProgressActivity::Writing))
                        .await;
                }
                // Do not force a second canonical draft repaint at completion.
                // The caller immediately sends exactly one permanent final Rich
                // Message from the canonical AST. The last streamed draft may be
                // slightly behind because of throttling, which is preferable to
                // a visible draft refresh immediately before the final message.
                tl.finish_all(ProgressState::Done).await;
            }
        }

        // Cancelled/interrupted output is presentation-only. Do not make a
        // partial answer canonical history: retry/follow-up context must only
        // see completed assistant turns.
        if cancelled || stream_interrupted {
            return (thinking_text, answer_text, cancelled);
        }

        // Persist only to the exact session revision that originated this request.
        // All destructive session mutations share this lock and bump the durable
        // revision before publishing RAM, so a late pre-clear generation cannot
        // reappear after /clear or a session replacement.
        let session_lock = self.session_lock(user_id).await;
        let _session_guard = session_lock.lock().await;
        let current_session = {
            let sessions_map = self.user_sessions.read().await;
            sessions_map.get(&user_id).and_then(|list| {
                list.iter()
                    .find(|session| session.id == request_session_id)
                    .cloned()
            })
        };
        if !generation_revision_matches(
            current_session.as_ref(),
            request_session_id,
            request_session_revision,
        ) {
            warn!(
                "Discarding late AI result for deleted or stale session {request_session_id} revision {request_session_revision}"
            );
            return (thinking_text, answer_text, cancelled);
        }
        let Some(mut candidate_session) = current_session else {
            return (thinking_text, answer_text, cancelled);
        };

        // Multimodal attachments are stored outside SQLite and referenced from
        // the user message. If the SQLite append fails, only the newly created
        // references are removed; pre-existing session attachments stay intact.
        let mut attachment_refs = Vec::new();
        if let Some(pages) = document_images.as_ref() {
            for (index, page) in pages.iter().enumerate() {
                let page_name = format!(
                    "{} page {}",
                    doc_name.unwrap_or("PDF scan"),
                    index.saturating_add(1)
                );
                match persist_attachment(
                    user_id,
                    request_session_id,
                    "document_page",
                    "image/png",
                    Some(&page_name),
                    page,
                )
                .await
                {
                    Ok(reference) => attachment_refs.push(reference),
                    Err(err) => warn!("Failed to persist rendered PDF page: {err}"),
                }
            }
        } else if let Some(bytes) = image_bytes.as_ref() {
            match resolved_runtime_media_mime(mime_type, "image/", "persisted image") {
                Ok(resolved_mime) => {
                    match persist_attachment(
                        user_id,
                        request_session_id,
                        "image",
                        &resolved_mime,
                        None,
                        bytes,
                    )
                    .await
                    {
                        Ok(reference) => attachment_refs.push(reference),
                        Err(err) => warn!("Failed to persist image attachment: {err}"),
                    }
                }
                Err(err) => warn!("Refusing to persist image with false media identity: {err}"),
            }
        } else if let Some(bytes) = audio_bytes.as_ref() {
            let resolved_mime = resolved_audio_persistence_mime(audio_mime, doc_name);
            match persist_attachment(
                user_id,
                request_session_id,
                "audio",
                &resolved_mime,
                doc_name,
                bytes,
            )
            .await
            {
                Ok(reference) => attachment_refs.push(reference),
                Err(err) => warn!("Failed to persist audio attachment: {err}"),
            }
        } else if let Some(bytes) = video_bytes.as_ref() {
            match resolved_runtime_media_mime(video_mime, "video/", "persisted video") {
                Ok(resolved_mime) => {
                    match persist_attachment(
                        user_id,
                        request_session_id,
                        "video",
                        &resolved_mime,
                        None,
                        bytes,
                    )
                    .await
                    {
                        Ok(reference) => attachment_refs.push(reference),
                        Err(err) => warn!("Failed to persist video attachment: {err}"),
                    }
                }
                Err(err) => warn!("Refusing to persist video with false media identity: {err}"),
            }
        }

        let user_message = ChatMessage {
            role: "user".to_string(),
            content: encode_user_content(
                canonical_persisted_prompt(canonical_history_prompt.as_deref(), &clean_prompt),
                attachment_refs.clone(),
            ),
        };
        let assistant_message = ChatMessage {
            role: "assistant".to_string(),
            content: Value::String(answer_text.clone()),
        };
        let appended = vec![user_message, assistant_message];
        if candidate_session.messages.is_empty() && candidate_session.name.starts_with("Session ") {
            let title_source =
                canonical_persisted_prompt(canonical_history_prompt.as_deref(), &clean_prompt);
            let clean_title = title_source.trim().replace('\n', " ");
            let short_title = truncate_chars_with_ellipsis(&clean_title, 32);
            if !short_title.is_empty() {
                candidate_session.name = short_title;
            }
        }
        candidate_session.messages.extend(appended.iter().cloned());

        match append_session_messages_db_async(
            user_id,
            request_session_revision,
            candidate_session.clone(),
            appended,
        )
        .await
        {
            Some(true) => {
                let mut sessions_map = self.user_sessions.write().await;
                if let Some(session) = sessions_map.get_mut(&user_id).and_then(|list| {
                    list.iter_mut().find(|session| {
                        session.id == request_session_id
                            && session.revision == request_session_revision
                    })
                }) {
                    *session = candidate_session;
                } else {
                    // The shared session lock makes this unreachable for normal
                    // in-process mutations. Evict instead of inventing a RAM-only
                    // canonical state if an external DB writer changed identity.
                    sessions_map.remove(&user_id);
                    warn!("Session cache changed after durable append; evicted RAM cache");
                }
            }
            Some(false) => {
                delete_attachment_refs(user_id, request_session_id, &attachment_refs).await;
                warn!(
                    "Discarding AI history append because session {request_session_id} revision changed before commit"
                );
            }
            None => {
                delete_attachment_refs(user_id, request_session_id, &attachment_refs).await;
                warn!(
                    "AI answer was generated but canonical history persistence failed for session {request_session_id}"
                );
            }
        }

        (thinking_text, answer_text, cancelled)
    }

    // ==========================================
    // Image Generation (role-aware OpenAI Images + explicit fallback)
    // ==========================================

    pub async fn generate_image(
        &self,
        user_id: i64,
        prompt: &str,
        width: usize,
        height: usize,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<GeneratedImage, ImageGenerationError> {
        let snapshot = self.generation_model_snapshot().await;
        self.generate_image_with_snapshot(user_id, prompt, width, height, &snapshot, cancel_rx)
            .await
    }

    pub(crate) async fn generate_image_with_snapshot(
        &self,
        _user_id: i64,
        prompt: &str,
        width: usize,
        height: usize,
        snapshot: &GenerationModelSnapshot,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<GeneratedImage, ImageGenerationError> {
        let clean_prompt = prompt.trim();
        let route = Self::resolve_model_route_from_snapshot(snapshot, ModelRole::ImageGeneration)
            .map_err(|error| {
            ImageGenerationError::new(classify_image_route_error(&error), error)
        })?;

        let generation_timeout = timeout_from_env(IMAGE_GENERATION_TIMEOUT_ENV, 120);
        let protocol = ImageGenerationProtocol::OpenAiImages;
        let gen_url = protocol.endpoint(&route.provider.endpoint);
        let payload = protocol.payload(&route.model, clean_prompt, width, height);
        let mut req = self
            .client
            .post(&gen_url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .timeout(generation_timeout);

        if !route.provider.api_key.is_empty()
            && !["none", "-", "no"]
                .iter()
                .any(|key| route.provider.api_key.eq_ignore_ascii_case(key))
        {
            req = req.header(
                "Authorization",
                format!("Bearer {}", route.provider.api_key),
            );
        }

        let provider_result = tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    return Err(ImageGenerationError::new(
                        ImageGenerationErrorKind::Cancelled,
                        "Pembuatan gambar dibatalkan.",
                    ));
                }
                Err(ImageGenerationError::new(
                    ImageGenerationErrorKind::Provider,
                    "Kanal pembatalan image generation ditutup.",
                ))
            }
            response = req.send() => {
                match response {
                    Err(error) if error.is_timeout() => Err(timeout_image_error(
                        "Image Generation Model",
                        generation_timeout,
                    )),
                    Err(error) => Err(ImageGenerationError::new(
                        ImageGenerationErrorKind::Provider,
                        format!("Koneksi ke Image Generation Model gagal: {error}"),
                    )),
                    Ok(response) if !response.status().is_success() => {
                        let status = response.status();
                        let detail = read_bounded_response_bytes(response, 64 * 1024)
                            .await
                            .ok()
                            .and_then(|bytes| String::from_utf8(bytes).ok())
                            .unwrap_or_default();
                        let kind = match status.as_u16() {
                            401 | 403 => ImageGenerationErrorKind::Auth,
                            429 => ImageGenerationErrorKind::RateLimited,
                            404 | 405 => ImageGenerationErrorKind::ProtocolMismatch,
                            400 if detail.to_ascii_lowercase().contains("not supported")
                                || detail.to_ascii_lowercase().contains("not an image model")
                                || detail.to_ascii_lowercase().contains("unsupported") => {
                                ImageGenerationErrorKind::ProtocolMismatch
                            }
                            _ => ImageGenerationErrorKind::HttpStatus,
                        };
                        Err(ImageGenerationError::new(
                            kind,
                            format!(
                                "Image Generation Model mengembalikan HTTP {}: {}",
                                status.as_u16(),
                                truncate_chars(&detail, 160)
                            ),
                        ))
                    }
                    Ok(response) => {
                        let body = read_bounded_json(response).await.map_err(|error| {
                            ImageGenerationError::new(
                                ImageGenerationErrorKind::InvalidResponse,
                                format!("Respons image generation tidak valid: {error}"),
                            )
                        })?;
                        let data = body
                            .get("data")
                            .and_then(|value| value.get(0))
                            .ok_or_else(|| {
                                ImageGenerationError::new(
                                    ImageGenerationErrorKind::InvalidResponse,
                                    "Respons image generation tidak memiliki data gambar.",
                                )
                            })?;

                        let bytes = if let Some(encoded) =
                            data.get("b64_json").and_then(|value| value.as_str())
                        {
                            decode_generated_image_base64(encoded)?
                        } else if let Some(url) = data.get("url").and_then(|value| value.as_str()) {
                            download_generated_image(url).await?
                        } else {
                            return Err(ImageGenerationError::new(
                                ImageGenerationErrorKind::InvalidResponse,
                                "Provider tidak mengembalikan b64_json atau URL gambar.",
                            ));
                        };

                        Ok(GeneratedImage {
                            bytes,
                            provider_name: route.provider.name.clone(),
                            model: route.model.clone(),
                            used_external_fallback: false,
                            primary_failure: None,
                        })
                    }
                }
            }
        };

        let provider_result = match provider_result {
            Ok(img) => Ok(img),
            Err(err) if err.kind == ImageGenerationErrorKind::ProtocolMismatch => {
                match self
                    .generate_image_via_chat_completion(&route, clean_prompt, cancel_rx)
                    .await
                {
                    Ok(img) => Ok(img),
                    Err(_) => Err(err),
                }
            }
            Err(err) => Err(err),
        };

        let primary_failure = match provider_result {
            Ok(image) => return Ok(image),
            Err(error) if error.kind == ImageGenerationErrorKind::Cancelled => return Err(error),
            Err(error) if error.kind == ImageGenerationErrorKind::Timeout => return Err(error),
            Err(provider_error) => {
                let fallback =
                    std::env::var("IMAGE_FALLBACK_PROVIDER").unwrap_or_else(|_| "none".to_string());
                if !external_image_fallback_enabled(&fallback) {
                    return Err(provider_error);
                }
                truncate_chars(&provider_error.message, 240)
            }
        };

        let encoded_prompt = urlencoding::encode(clean_prompt);
        let fallback_model = "flux";
        let poll_url = format!(
            "https://image.pollinations.ai/prompt/{}?width={}&height={}&model={}&nologo=true&enhance=true",
            encoded_prompt, width, height, fallback_model
        );
        let fallback_timeout = timeout_from_env(IMAGE_GENERATION_TIMEOUT_ENV, 120);
        let request = self.client.get(&poll_url).timeout(fallback_timeout);
        let response = tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    return Err(ImageGenerationError::new(
                        ImageGenerationErrorKind::Cancelled,
                        "Pembuatan gambar dibatalkan.",
                    ));
                }
                return Err(ImageGenerationError::new(
                    ImageGenerationErrorKind::Provider,
                    "Kanal pembatalan image generation ditutup.",
                ));
            }
            response = request.send() => response
        }
        .map_err(|error| {
            if error.is_timeout() {
                ImageGenerationError::new(
                    ImageGenerationErrorKind::Timeout,
                    format!(
                        "Fallback image generation melewati batas waktu {} detik.",
                        fallback_timeout.as_secs()
                    ),
                )
            } else {
                ImageGenerationError::new(
                    ImageGenerationErrorKind::Provider,
                    format!("Fallback image generation gagal: {error}"),
                )
            }
        })?;

        if !response.status().is_success() {
            return Err(ImageGenerationError::new(
                ImageGenerationErrorKind::Provider,
                format!(
                    "Fallback image generation mengembalikan HTTP {}.",
                    response.status().as_u16()
                ),
            ));
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("image/"))
        {
            return Err(ImageGenerationError::new(
                ImageGenerationErrorKind::InvalidResponse,
                "Fallback tidak mengembalikan content-type gambar.",
            ));
        }
        let bytes = read_bounded_response_bytes(response, MAX_GENERATED_IMAGE_BYTES)
            .await
            .map_err(|error| {
                ImageGenerationError::new(ImageGenerationErrorKind::InvalidResponse, error)
            })?;
        validate_generated_image_bytes(&bytes).map_err(|error| {
            ImageGenerationError::new(ImageGenerationErrorKind::InvalidImage, error)
        })?;

        Ok(GeneratedImage {
            bytes,
            provider_name: "Pollinations fallback".to_string(),
            model: fallback_model.to_string(),
            used_external_fallback: true,
            primary_failure: Some(primary_failure),
        })
    }

    async fn generate_image_via_chat_completion(
        &self,
        route: &ResolvedModelRoute,
        prompt: &str,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<GeneratedImage, ImageGenerationError> {
        let chat_url = provider_url(&route.provider.endpoint, "chat/completions");
        let messages = vec![json!({
            "role": "user",
            "content": format!(
                "Generate an image for: {prompt}. Output the image directly as a markdown image link, direct URL, or inline data URL."
            )
        })];

        let mut req = self
            .client
            .post(&chat_url)
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": route.model,
                "messages": messages,
                "stream": false,
            }))
            .timeout(Duration::from_secs(90));

        if !route.provider.api_key.is_empty()
            && !["none", "-", "no"]
                .iter()
                .any(|key| route.provider.api_key.eq_ignore_ascii_case(key))
        {
            req = req.header(
                "Authorization",
                format!("Bearer {}", route.provider.api_key),
            );
        }

        let response = tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    return Err(ImageGenerationError::new(
                        ImageGenerationErrorKind::Cancelled,
                        "Pembuatan gambar dibatalkan.",
                    ));
                }
                return Err(ImageGenerationError::new(
                    ImageGenerationErrorKind::Provider,
                    "Kanal pembatalan image generation ditutup.",
                ));
            }
            res = req.send() => res.map_err(|e| {
                ImageGenerationError::new(
                    ImageGenerationErrorKind::Provider,
                    format!("Chat image generation request gagal: {e}"),
                )
            })?
        };

        if !response.status().is_success() {
            let status = response.status();
            return Err(ImageGenerationError::new(
                ImageGenerationErrorKind::HttpStatus,
                format!("Chat image generation mengembalikan HTTP {status}"),
            ));
        }

        let body = read_bounded_json(response).await.map_err(|e| {
            ImageGenerationError::new(
                ImageGenerationErrorKind::InvalidResponse,
                format!("Respons JSON tidak valid: {e}"),
            )
        })?;

        let reply_text = assistant_text_from_value(&body).unwrap_or_default();

        if let Some(data_pos) = reply_text.find("data:image/") {
            if let Some(comma_pos) = reply_text[data_pos..].find(',') {
                let after_comma = &reply_text[data_pos + comma_pos + 1..];
                let b64_end = after_comma
                    .find(|c: char| {
                        c.is_whitespace() || c == ')' || c == '"' || c == '\'' || c == ']'
                    })
                    .unwrap_or(after_comma.len());
                let b64_str = after_comma[..b64_end].trim();
                if let Ok(bytes) = decode_generated_image_base64(b64_str) {
                    return Ok(GeneratedImage {
                        bytes,
                        provider_name: route.provider.name.clone(),
                        model: route.model.clone(),
                        used_external_fallback: false,
                        primary_failure: None,
                    });
                }
            }
        }

        if let Some(url) = extract_image_url(&reply_text) {
            if let Ok(bytes) = download_generated_image(&url).await {
                return Ok(GeneratedImage {
                    bytes,
                    provider_name: route.provider.name.clone(),
                    model: route.model.clone(),
                    used_external_fallback: false,
                    primary_failure: None,
                });
            }
        }

        Err(ImageGenerationError::new(
            ImageGenerationErrorKind::InvalidResponse,
            format!(
                "Model tidak mengembalikan gambar valid: {}",
                truncate_chars(&reply_text, 160)
            ),
        ))
    }
}

fn assistant_text_from_value(body: &Value) -> Option<String> {
    let choices = body.get("choices")?.as_array()?;
    let first = choices.first()?;
    let content = first.get("message")?.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    if let Some(arr) = content.as_array() {
        let text = arr
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        return Some(text);
    }
    None
}

fn extract_image_url(text: &str) -> Option<String> {
    if let Some(start) = text.find("![") {
        if let Some(paren_start) = text[start..].find("](") {
            let after_paren = &text[start + paren_start + 2..];
            if let Some(paren_end) = after_paren.find(')') {
                let url = after_paren[..paren_end].trim();
                if url.starts_with("http://") || url.starts_with("https://") {
                    return Some(url.to_string());
                }
            }
        }
    }
    for word in text.split_whitespace() {
        let clean = word.trim_matches(|c: char| {
            c == '(' || c == ')' || c == '"' || c == '\'' || c == '<' || c == '>'
        });
        if clean.starts_with("https://") || clean.starts_with("http://") {
            let lower = clean.to_ascii_lowercase();
            if lower.ends_with(".png")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.ends_with(".webp")
                || lower.contains("googleusercontent.com")
                || lower.contains("oaidalleapiprodscus.blob.core.windows.net")
            {
                return Some(clean.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: usize) -> ChatSession {
        ChatSession {
            id,
            name: format!("Session {id}"),
            messages: Vec::new(),
            created_at: "now".to_string(),
            revision: 0,
        }
    }

    #[test]
    fn legacy_active_index_maps_to_stable_session_id() {
        let sessions = vec![session(3), session(8), session(20)];
        assert_eq!(
            crate::ai::storage::legacy_active_session_id(Some(1), &sessions),
            Some(8)
        );
        assert_eq!(
            crate::ai::storage::legacy_active_session_id(Some(99), &sessions),
            Some(3)
        );
    }

    #[test]
    fn session_id_counter_never_reuses_deleted_high_water_mark() {
        assert_eq!(crate::ai::storage::compute_next_session_id(Some(21), 8), 21);
        assert_eq!(crate::ai::storage::compute_next_session_id(Some(4), 8), 9);
        assert_eq!(crate::ai::storage::compute_next_session_id(None, 8), 9);
    }

    #[test]
    fn clear_revision_invalidates_old_generation_and_new_generation_matches() {
        let mut current = session(11);
        current.revision = 14;
        assert!(generation_revision_matches(Some(&current), 11, 14));
        current.revision += 1;
        assert!(!generation_revision_matches(Some(&current), 11, 14));
        assert!(generation_revision_matches(Some(&current), 11, 15));
    }

    #[test]
    fn deleted_session_discards_late_generation_and_switch_never_redirects_it() {
        let origin = session(7);
        let active_after_switch = session(9);
        assert!(!generation_revision_matches(None, 7, 0));
        assert!(!generation_revision_matches(
            Some(&active_after_switch),
            7,
            0
        ));
        assert!(generation_revision_matches(Some(&origin), 7, 0));
    }

    fn evidence_record(
        kind: CapabilityKind,
        outcome: CapabilityState,
        age: chrono::Duration,
    ) -> CapabilityRecord {
        let mut record = CapabilityRecord::default();
        match kind {
            CapabilityKind::ImageInput => {
                record.supports_image_input = Some(outcome == CapabilityState::Supported)
            }
            CapabilityKind::AudioInput => {
                record.supports_audio_input = Some(outcome == CapabilityState::Supported)
            }
            CapabilityKind::AudioTranscription => {
                record.supports_audio_transcription = Some(outcome == CapabilityState::Supported)
            }
            CapabilityKind::VideoInput => {
                record.supports_video_input = Some(outcome == CapabilityState::Supported)
            }
            _ => {}
        }
        record
            .evidence
            .push(crate::ai::storage::CapabilityEvidence {
                capability: kind,
                source: crate::ai::storage::CapabilityEvidenceSource::ActiveProbe,
                outcome,
                checked_at: (chrono::Utc::now() - age).to_rfc3339(),
                detail: None,
            });
        record
    }

    #[test]
    fn multimodal_runtime_authorization_is_freshness_aware() {
        let fresh = evidence_record(
            CapabilityKind::ImageInput,
            CapabilityState::Supported,
            chrono::Duration::hours(1),
        );
        assert!(
            require_verified_capability(Some(&fresh), CapabilityKind::ImageInput, "image",).is_ok()
        );
        assert!(history_attachment_authorized(Some(&fresh), "image"));
        assert!(history_attachment_authorized(Some(&fresh), "document_page"));

        let mut stale = evidence_record(
            CapabilityKind::ImageInput,
            CapabilityState::Supported,
            chrono::Duration::days(8),
        );
        stale.evidence.push(crate::ai::storage::CapabilityEvidence {
            capability: CapabilityKind::TextChat,
            source: crate::ai::storage::CapabilityEvidenceSource::ActiveProbe,
            outcome: CapabilityState::Supported,
            checked_at: chrono::Utc::now().to_rfc3339(),
            detail: None,
        });
        assert_eq!(
            stale.effective_state_for(CapabilityKind::TextChat),
            CapabilityState::Supported
        );
        assert_eq!(
            stale.effective_state_for(CapabilityKind::ImageInput),
            CapabilityState::Unknown
        );
        assert!(
            require_verified_capability(Some(&stale), CapabilityKind::ImageInput, "image",)
                .is_err()
        );
        assert!(!history_attachment_authorized(Some(&stale), "image"));
        assert!(!history_attachment_authorized(
            Some(&stale),
            "document_page"
        ));

        let unsupported = evidence_record(
            CapabilityKind::ImageInput,
            CapabilityState::Unsupported,
            chrono::Duration::hours(1),
        );
        assert!(!history_attachment_authorized(Some(&unsupported), "image"));
        assert!(!history_attachment_authorized(None, "image"));
    }

    #[test]
    fn stale_historical_audio_and_video_are_omitted() {
        let stale_audio = evidence_record(
            CapabilityKind::AudioInput,
            CapabilityState::Supported,
            chrono::Duration::days(8),
        );
        let stale_video = evidence_record(
            CapabilityKind::VideoInput,
            CapabilityState::Supported,
            chrono::Duration::days(8),
        );
        assert!(!history_attachment_authorized(Some(&stale_audio), "audio"));
        assert!(!history_attachment_authorized(Some(&stale_video), "video"));
    }

    #[test]
    fn media_data_urls_preserve_resolved_mime_and_fail_closed() {
        for (mime_type, expected_prefix) in [
            ("image/png", "data:image/png;base64,"),
            ("image/webp", "data:image/webp;base64,"),
            ("video/mp4", "data:video/mp4;base64,"),
            ("video/webm", "data:video/webm;base64,"),
            ("video/x-matroska", "data:video/x-matroska;base64,"),
        ] {
            let expected_kind = if mime_type.starts_with("image/") {
                "image/"
            } else {
                "video/"
            };
            let data_url =
                media_data_url(b"media", Some(mime_type), expected_kind, "test media").unwrap();
            assert!(data_url.starts_with(expected_prefix), "{mime_type}");
        }

        assert!(media_data_url(b"media", None, "image/", "image").is_err());
        assert!(media_data_url(b"media", Some("video/webm"), "image/", "image").is_err());
        assert!(media_data_url(b"media", Some("image/png"), "video/", "video").is_err());
    }

    #[test]
    fn audio_persistence_mime_recovers_known_filename_without_overwriting_explicit_mime() {
        assert_eq!(
            resolved_audio_persistence_mime(None, Some("sample.mp3")),
            "audio/mpeg"
        );
        assert_eq!(
            resolved_audio_persistence_mime(None, Some("sample.wav")),
            "audio/wav"
        );
        assert_eq!(
            resolved_audio_persistence_mime(None, Some("sample.opus")),
            "audio/opus"
        );
        assert_eq!(
            resolved_audio_persistence_mime(None, Some("sample.flac")),
            "audio/flac"
        );
        assert_eq!(
            resolved_audio_persistence_mime(Some("Audio/WebM; codecs=opus"), Some("sample.mp3")),
            "audio/webm"
        );
    }

    #[test]
    fn native_audio_format_mapping_is_protocol_safe() {
        for mime in ["audio/mpeg", "audio/mp3"] {
            assert_eq!(native_audio_input_format(Some(mime), None), Ok("mp3"));
        }
        for mime in ["audio/wav", "audio/x-wav", "audio/wave"] {
            assert_eq!(native_audio_input_format(Some(mime), None), Ok("wav"));
        }
        for mime in ["audio/ogg", "application/ogg"] {
            assert_eq!(native_audio_input_format(Some(mime), None), Ok("ogg"));
        }
        assert_eq!(
            native_audio_input_format(Some("audio/opus"), None),
            Ok("opus")
        );
        assert_eq!(
            native_audio_input_format(Some("audio/m4a"), None),
            Ok("m4a")
        );
        assert_eq!(
            native_audio_input_format(Some("audio/flac"), None),
            Ok("flac")
        );
        assert_eq!(
            native_audio_input_format(Some("audio/webm"), None),
            Ok("webm")
        );

        assert_eq!(
            native_audio_input_format(None, Some("voice.wav")),
            Ok("wav")
        );
        assert_eq!(
            native_audio_input_format(None, Some("voice.ogg")),
            Ok("ogg")
        );
        assert_eq!(
            native_audio_input_format(None, Some("voice.oga")),
            Ok("ogg")
        );
        assert_eq!(
            native_audio_input_format(None, Some("track.m4a")),
            Ok("m4a")
        );
        assert_eq!(
            native_audio_input_format(Some("application/octet-stream"), Some("voice.mp3")),
            Ok("mp3")
        );
        assert!(matches!(
            native_audio_input_format(Some("application/octet-stream"), Some("voice.bin")),
            Err(NativeAudioFormatError::Unknown)
        ));
        assert!(matches!(
            native_audio_input_format(None, None),
            Err(NativeAudioFormatError::Unknown)
        ));
    }

    #[test]
    fn current_and_historical_native_audio_parts_share_safe_mapping() {
        let current = native_audio_input_part(b"audio", Some("audio/mpeg"), Some("clip.mp3"))
            .expect("mp3 should be native-safe");
        assert_eq!(
            current
                .get("input_audio")
                .and_then(|value| value.get("format"))
                .and_then(Value::as_str),
            Some("mp3")
        );

        let historical =
            historical_native_audio_input_part(b"audio", "audio/x-wav", Some("clip.wav"))
                .expect("wav should be native-safe");
        assert_eq!(
            historical
                .get("input_audio")
                .and_then(|value| value.get("format"))
                .and_then(Value::as_str),
            Some("wav")
        );

        let ogg_part = historical_native_audio_input_part(b"audio", "audio/ogg", Some("voice.ogg"))
            .expect("ogg should be native-safe");
        assert_eq!(
            ogg_part
                .get("input_audio")
                .and_then(|value| value.get("format"))
                .and_then(Value::as_str),
            Some("ogg")
        );
    }

    #[test]
    fn audio_execution_uses_format_and_fresh_effective_capability() {
        fn combined(
            native: (CapabilityState, chrono::Duration),
            stt: (CapabilityState, chrono::Duration),
        ) -> CapabilityRecord {
            let mut record = evidence_record(CapabilityKind::AudioInput, native.0, native.1);
            let stt_record = evidence_record(CapabilityKind::AudioTranscription, stt.0, stt.1);
            record.evidence.extend(stt_record.evidence);
            record.supports_audio_transcription = stt_record.supports_audio_transcription;
            record
        }

        let fresh = chrono::Duration::hours(1);
        let stale = chrono::Duration::days(8);

        assert_eq!(
            select_audio_execution_mode(
                &combined(
                    (CapabilityState::Supported, fresh),
                    (CapabilityState::Supported, fresh),
                ),
                true,
                Some("audio/mpeg"),
                Some("clip.mp3"),
            ),
            Ok(AudioExecutionMode::Native)
        );

        assert_eq!(
            select_audio_execution_mode(
                &combined(
                    (CapabilityState::Supported, fresh),
                    (CapabilityState::Supported, fresh),
                ),
                true,
                Some("audio/ogg"),
                Some("voice.ogg"),
            ),
            Ok(AudioExecutionMode::Native)
        );

        assert_eq!(
            select_audio_execution_mode(
                &combined(
                    (CapabilityState::Supported, fresh),
                    (CapabilityState::Supported, fresh),
                ),
                true,
                Some("audio/opus"),
                Some("sample.opus"),
            ),
            Ok(AudioExecutionMode::Native)
        );

        assert_eq!(
            select_audio_execution_mode(
                &combined(
                    (CapabilityState::Supported, stale),
                    (CapabilityState::Supported, fresh),
                ),
                true,
                Some("audio/mpeg"),
                Some("clip.mp3"),
            ),
            Ok(AudioExecutionMode::Transcription)
        );

        assert_eq!(
            select_audio_execution_mode(
                &combined(
                    (CapabilityState::Unsupported, fresh),
                    (CapabilityState::Supported, fresh),
                ),
                true,
                Some("audio/wav"),
                Some("clip.wav"),
            ),
            Ok(AudioExecutionMode::Transcription)
        );

        assert!(select_audio_execution_mode(
            &combined(
                (CapabilityState::Supported, stale),
                (CapabilityState::Supported, stale),
            ),
            true,
            Some("audio/mp3"),
            Some("clip.mp3"),
        )
        .is_err());
    }

    #[test]
    fn stream_accumulation_has_absolute_bounds() {
        let mut visible = String::new();
        assert!(push_bounded(&mut visible, "abc", 3));
        assert!(!push_bounded(&mut visible, "d", 3));
        assert_eq!(visible, "abc");

        let mut reasoning = String::new();
        assert!(push_bounded(&mut reasoning, "🧠", 4));
        assert!(!push_bounded(&mut reasoning, "x", 4));
        assert_eq!(reasoning, "🧠");
    }

    #[test]
    fn selected_image_model_is_propagated_to_openai_images_payload() {
        let payload = ImageGenerationProtocol::OpenAiImages.payload(
            "black-forest-labs/FLUX.1-schnell",
            "galaxy",
            1024,
            1024,
        );
        assert_eq!(
            payload.get("model").and_then(Value::as_str),
            Some("black-forest-labs/FLUX.1-schnell")
        );
        assert_eq!(
            payload.get("size").and_then(Value::as_str),
            Some("1024x1024")
        );
    }

    #[test]
    fn timeout_configuration_is_scoped_and_safely_bounded() {
        assert_eq!(
            AI_PROVIDER_CONNECT_TIMEOUT_ENV,
            "AI_PROVIDER_CONNECT_TIMEOUT_SECS"
        );
        assert_eq!(
            IMAGE_PROVIDER_CONNECT_TIMEOUT_ENV,
            "IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS"
        );
        assert_eq!(
            IMAGE_GENERATION_TIMEOUT_ENV,
            "IMAGE_GENERATION_TIMEOUT_SECS"
        );
        assert_eq!(IMAGE_DOWNLOAD_TIMEOUT_ENV, "IMAGE_DOWNLOAD_TIMEOUT_SECS");
        assert_ne!(
            AI_PROVIDER_CONNECT_TIMEOUT_ENV,
            IMAGE_PROVIDER_CONNECT_TIMEOUT_ENV
        );

        assert_eq!(bounded_timeout_secs(None, 120), 120);
        assert_eq!(bounded_timeout_secs(Some("0"), 120), 120);
        assert_eq!(bounded_timeout_secs(Some("bad"), 120), 120);
        assert_eq!(bounded_timeout_secs(Some("75"), 120), 75);
        assert_eq!(bounded_timeout_secs(Some("99999"), 120), 600);
    }

    #[test]
    fn fallback_result_retains_primary_failure_provenance() {
        let image = GeneratedImage {
            bytes: b"\x89PNG\r\n\x1a\nrest".to_vec(),
            provider_name: "Pollinations fallback".to_string(),
            model: "flux".to_string(),
            used_external_fallback: true,
            primary_failure: Some("Primary provider returned HTTP 503".to_string()),
        };
        assert!(image.used_external_fallback);
        assert_eq!(
            image.primary_failure.as_deref(),
            Some("Primary provider returned HTTP 503")
        );
    }

    #[test]
    fn external_image_fallback_is_explicit_opt_in_only() {
        assert!(external_image_fallback_enabled("pollinations"));
        assert!(external_image_fallback_enabled(" POLLINATIONS "));
        assert!(!external_image_fallback_enabled("auto"));
        assert!(!external_image_fallback_enabled(""));
        assert!(!external_image_fallback_enabled("none"));
        assert!(!external_image_fallback_enabled("false"));
        assert!(!external_image_fallback_enabled("off"));
    }

    #[test]
    fn specialist_payload_contains_only_the_current_user_message() {
        let payload = specialist_chat_payload(
            "vision-model",
            vec![json!({"type":"text","text":"current question"})],
        );
        let messages = payload.get("messages").and_then(Value::as_array).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].get("role").and_then(Value::as_str),
            Some("user")
        );
        assert_eq!(
            payload.get("model").and_then(Value::as_str),
            Some("vision-model")
        );
    }

    #[test]
    fn specialist_runtime_prompt_does_not_replace_canonical_user_prompt() {
        assert_eq!(
            canonical_persisted_prompt(
                Some("what is in this image?"),
                "internal specialist synthesis"
            ),
            "what is in this image?"
        );
        assert_eq!(
            canonical_persisted_prompt(None, "ordinary chat"),
            "ordinary chat"
        );
    }

    #[test]
    fn generated_image_base64_rejects_oversized_input_before_decode() {
        let oversized = "A".repeat(
            MAX_GENERATED_IMAGE_BYTES
                .saturating_mul(4)
                .div_ceil(3)
                .saturating_add(16),
        );
        let error = decode_generated_image_base64(&oversized).unwrap_err();
        assert_eq!(error.kind, ImageGenerationErrorKind::InvalidImage);
    }

    #[test]
    fn generated_image_base64_validation_is_typed() {
        let error = decode_generated_image_base64("%%%not-base64%%%").unwrap_err();
        assert_eq!(error.kind, ImageGenerationErrorKind::InvalidBase64);

        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nrest");
        assert!(decode_generated_image_base64(&encoded).is_ok());
    }

    #[test]
    fn generated_image_url_validation_rejects_unsafe_schemes_and_private_ips() {
        assert_eq!(
            parse_generated_image_url("file:///etc/passwd")
                .unwrap_err()
                .kind,
            ImageGenerationErrorKind::UnsafeImageUrl
        );
        assert!(parse_generated_image_url("https://example.com/image.png").is_ok());
        assert!(is_unsafe_remote_ip("127.0.0.1".parse().unwrap()));
        assert!(is_unsafe_remote_ip("10.1.2.3".parse().unwrap()));
        assert!(is_unsafe_remote_ip("100.64.0.1".parse().unwrap()));
        assert!(is_unsafe_remote_ip("::1".parse().unwrap()));
        assert!(is_unsafe_remote_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_unsafe_remote_ip("1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn image_timeout_is_a_typed_timeout_not_unsupported() {
        let error = timeout_image_error("Image Generation Model", Duration::from_secs(120));
        assert_eq!(error.kind, ImageGenerationErrorKind::Timeout);
        assert!(error.message.contains("120"));
    }

    #[tokio::test]
    async fn generation_cancel_signal_reaches_registered_receiver() {
        let (sender, mut receiver) = watch::channel(false);
        assert!(signal_generation_cancel(Some(sender)));
        receiver.changed().await.unwrap();
        assert!(*receiver.borrow());
    }

    #[test]
    fn generated_image_validation_rejects_non_image_bytes() {
        assert!(validate_generated_image_bytes(b"not an image").is_err());
        assert!(validate_generated_image_bytes(b"\x89PNG\r\n\x1a\nrest").is_ok());
    }

    #[test]
    fn image_route_errors_keep_capability_and_route_failures_distinct() {
        assert_eq!(
            classify_image_route_error("Image Generation Model is Disabled"),
            ImageGenerationErrorKind::RouteDisabled
        );
        assert_eq!(
            classify_image_route_error("Image Generation Model is explicitly Unsupported"),
            ImageGenerationErrorKind::CapabilityUnsupported
        );
        assert_eq!(
            classify_image_route_error("Image Generation Model capability is Unknown"),
            ImageGenerationErrorKind::CapabilityUnknown
        );
    }

    #[test]
    fn audio_mime_mapping_covers_all_standard_formats() {
        let (mime, name) = resolve_audio_file_and_mime(Some("audio/ogg"), Some("voice"));
        assert_eq!(mime, "audio/ogg");
        assert_eq!(name, "voice.ogg");

        let (mime, name) = resolve_audio_file_and_mime(Some("audio/opus"), Some("note"));
        assert_eq!(mime, "audio/opus");
        assert_eq!(name, "note.opus");

        let (mime, name) = resolve_audio_file_and_mime(Some("audio/mpeg"), Some("speech"));
        assert_eq!(mime, "audio/mpeg");
        assert_eq!(name, "speech.mp3");

        let (mime, name) = resolve_audio_file_and_mime(Some("audio/mp4"), Some("recording"));
        assert_eq!(mime, "audio/mp4");
        assert_eq!(name, "recording.m4a");

        let (mime, name) = resolve_audio_file_and_mime(Some("audio/x-m4a"), Some("memo"));
        assert_eq!(mime, "audio/x-m4a");
        assert_eq!(name, "memo.m4a");

        let (mime, name) = resolve_audio_file_and_mime(Some("audio/wav"), Some("sample"));
        assert_eq!(mime, "audio/wav");
        assert_eq!(name, "sample.wav");

        let (mime, name) = resolve_audio_file_and_mime(Some("audio/x-wav"), Some("test"));
        assert_eq!(mime, "audio/wav");
        assert_eq!(name, "test.wav");

        let (mime, name) = resolve_audio_file_and_mime(Some("application/custom"), Some("data"));
        assert_eq!(mime, "application/octet-stream");
        assert_eq!(name, "data.bin");
        assert_ne!(mime, "audio/ogg");
    }

    #[test]
    fn mime_less_audio_resolver_drives_native_and_stt_formats_truthfully() {
        let cases = [
            ("sample.mp3", "audio/mpeg", "sample.mp3"),
            ("sample.wav", "audio/wav", "sample.wav"),
            ("sample.opus", "audio/opus", "sample.opus"),
            ("sample.flac", "audio/flac", "sample.flac"),
        ];

        for (file_name, expected_mime, expected_name) in cases {
            let (mime_type, safe_name) = resolve_audio_file_and_mime(None, Some(file_name));
            assert_eq!(mime_type, expected_mime, "{file_name}");
            assert_eq!(safe_name, expected_name, "{file_name}");
        }

        assert_eq!(
            native_audio_input_format(Some("audio/mpeg"), Some("sample.mp3")),
            Ok("mp3")
        );
        assert_eq!(
            native_audio_input_format(Some("audio/wav"), Some("sample.wav")),
            Ok("wav")
        );
        assert_eq!(
            native_audio_input_format(Some("audio/opus"), Some("sample.opus")),
            Ok("opus")
        );
        assert_eq!(
            native_audio_input_format(Some("audio/flac"), Some("sample.flac")),
            Ok("flac")
        );

        let (mime_type, safe_name) =
            resolve_audio_file_and_mime(Some("audio/webm"), Some("sample.webm"));
        assert_eq!(mime_type, "audio/webm");
        assert_eq!(safe_name, "sample.webm");
    }

    #[test]
    fn persistence_path_has_no_false_media_default() {
        let source = include_str!("service.rs");
        let persistence_start = source
            .find("// Multimodal attachments are stored outside SQLite")
            .expect("attachment persistence block");
        let persistence_end = source[persistence_start..]
            .find("let user_message = ChatMessage")
            .map(|offset| persistence_start + offset)
            .expect("attachment persistence block end");
        let persistence = &source[persistence_start..persistence_end];

        assert!(!persistence.contains("mime_type.unwrap_or(\"image/jpeg\")"));
        assert!(!persistence.contains("video_mime.unwrap_or(\"video/mp4\")"));
        assert!(!persistence.contains("audio_mime.unwrap_or(\"application/octet-stream\")"));
        assert!(persistence.contains("resolved_audio_persistence_mime(audio_mime, doc_name)"));
    }

    #[test]
    fn main_route_snapshot_keeps_provider_model_and_capability_stable() {
        let route = ResolvedModelRoute {
            provider: ProviderConfig {
                id: "prov-a".to_string(),
                name: "Provider A".to_string(),
                endpoint: "https://a.example/v1".to_string(),
                api_key: String::new(),
                api_key_ref: None,
                models: vec!["model-a".to_string()],
                active_model: "model-a".to_string(),
            },
            model: "model-a".to_string(),
            capability: CapabilityRecord {
                provider_id: "https://a.example/v1".to_string(),
                model: "model-a".to_string(),
                supports_text_chat: Some(true),
                ..CapabilityRecord::default()
            },
            route_origin: RouteOrigin::Main,
        };
        assert_eq!(route.provider.id, "prov-a");
        assert_eq!(route.model, "model-a");
        assert_eq!(route.capability.supports_text_chat, Some(true));
    }
}
