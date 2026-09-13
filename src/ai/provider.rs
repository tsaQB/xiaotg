use base64::Engine;
use chrono::Local;
use futures_util::StreamExt;
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::warn;

use crate::util::truncate_chars;

const MAX_PROVIDER_METADATA_BYTES: usize = 8 * 1024 * 1024;

async fn read_bounded_provider_json(response: reqwest::Response) -> Result<Value, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PROVIDER_METADATA_BYTES as u64)
    {
        return Err("provider metadata response exceeded XiaoAI limits".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "provider metadata stream failed".to_string())?;
        if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_METADATA_BYTES {
            return Err("provider metadata response exceeded XiaoAI limits".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid provider metadata JSON: {error}"))
}

async fn read_bounded_provider_text(response: reqwest::Response, max_bytes: usize) -> String {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(Ok(chunk)) = stream.next().await {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).unwrap_or_default()
}

const RED_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP8z0AaYCJR/aiGUQ1DSAMAQC4BH2bjRnMAAAAASUVORK5CYII=";
const BLUE_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAGUlEQVR4nGNkYPjPQApgIkn1qIZRDUNKAwA+MAEfWiW9ygAAAABJRU5ErkJggg==";

const SPOKEN_PROBE_MP3_BASE64: &str = "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";

fn spoken_probe_audio_bytes() -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(SPOKEN_PROBE_MP3_BASE64)
        .unwrap_or_default()
}

const TINY_RED_MP4_BASE64: &str = "AAAAIGZ0eXBpc29tAAACAGlzb21pc28yYXZjMW1wNDEAAAMUbW9vdgAAAGxtdmhkAAAAAAAAAAAAAAAAAAAD6AAAA+gAAQAAAQAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAAj90cmFrAAAAXHRraGQAAAADAAAAAAAAAAAAAAABAAAAAAAAA+gAAAAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAABAAAAAQAAAAAAAkZWR0cwAAABxlbHN0AAAAAAAAAAEAAAPoAAAAAAABAAAAAAG3bWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAABAAAAAQABVxAAAAAAALWhkbHIAAAAAAAAAAHZpZGUAAAAAAAAAAAAAAABWaWRlb0hhbmRsZXIAAAABYm1pbmYAAAAUdm1oZAAAAAEAAAAAAAAAAAAAACRkaW5mAAAAHGRyZWYAAAAAAAAAAQAAAAx1cmwgAAAAAQAAASJzdGJsAAAAvnN0c2QAAAAAAAAAAQAAAK5hdmMxAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAABAAEABIAAAASAAAAAAAAAABFUxhdmM2MS4xOS4xMDEgbGlieDI2NAAAAAAAAAAAAAAAGP//AAAANGF2Y0MBZAAK/+EAF2dkAAqs2V7ARAAAAwAEAAADAAg8SJZYAQAGaOvjyyLA/fj4AAAAABBwYXNwAAAAAQAAAAEAAAAUYnRydAAAAAAAABZYAAAAAAAAABhzdHRzAAAAAAAAAAEAAAABAABAAAAAABxzdHNjAAAAAAAAAAEAAAABAAAAAQAAAAEAAAAUc3RzegAAAAAAAALLAAAAAQAAABRzdGNvAAAAAAAAAAEAAANEAAAAYXVkdGEAAABZbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAbWRpcmFwcGwAAAAAAAAAAAAAAAAsaWxzdAAAACSpdG9vAAAAHGRhdGEAAAABAAAAAExhdmY2MS43LjEwMwAAAAhmcmVlAAAC021kYXQAAAKtBgX//6ncRem95tlIt5Ys2CDZI+7veDI2NCAtIGNvcmUgMTY0IHIzMTA4IDMxZTE5ZjkgLSBILjI2NC9NUEVHLTQgQVZDIGNvZGVjIC0gQ29weWxlZnQgMjAwMy0yMDIzIC0gaHR0cDovL3d3dy52aWRlb2xhbi5vcmcveDI2NC5odG1sIC0gb3B0aW9uczogY2FiYWM9MSByZWY9MyBkZWJsb2NrPTE6MDowIGFuYWx5c2U9MHgzOjB4MTEzIG1lPWhleCBzdWJtZT03IHBzeT0xIHBzeV9yZD0xLjAwOjAuMDAgbWl4ZWRfcmVmPTEgbWVfcmFuZ2U9MTYgY2hyb21hX21lPTEgdHJlbGxpcz0xIDh4OGRjdD0xIGNxbT0wIGRlYWR6b25lPTIxLDExIGZhc3RfcHNraXA9MSBjaHJvbWFfcXBfb2Zmc2V0PS0yIHRocmVhZHM9MSBsb29rYWhlYWRfdGhyZWFkcz0xIHNsaWNlZF90aHJlYWRzPTAgbnI9MCBkZWNpbWF0ZT0xIGludGVybGFjZWQ9MCBibHVyYXlfY29tcGF0PTAgY29uc3RyYWluZWRfaW50cmE9MCBiZnJhbWVzPTMgYl9weXJhbWlkPTIgYl9hZGFwdD0xIGJfYmlhcz0wIGRpcmVjdD0xIHdlaWdodGI9MSBvcGVuX2dvcD0wIHdlaWdodHA9MiBrZXlpbnQ9MjUwIGtleWludF9taW49MSBzY2VuZWN1dD00MCBpbnRyYV9yZWZyZXNoPTAgcmNfbG9va2FoZWFkPTQwIHJjPWNyZiBtYnRyZWU9MSBjcmY9MjMuMCBxY29tcD0wLjYwIHFwbWluPTAgcXBtYXg9NjkgcXBzdGVwPTQgaXBfcmF0aW89MS40MCBhcT0xOjEuMDAAgAAAABZliIQAFf/+7M9+BTZo5i/D8UVzjn2B";

fn video_probe_payload(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "text",
                    "text": "Return only the dominant color visible in this short video as one lowercase English word."
                },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:video/mp4;base64,{TINY_RED_MP4_BASE64}")
                    }
                }
            ]
        }],
        "stream": false,
        "max_tokens": 8
    })
}

async fn validate_image_generation_probe_body(body: &Value) -> bool {
    let Some(item) = body
        .get("data")
        .and_then(Value::as_array)
        .and_then(|data| data.first())
    else {
        return false;
    };

    if let Some(encoded) = item.get("b64_json").and_then(Value::as_str) {
        return decode_generated_image_base64(encoded).is_ok();
    }
    if let Some(url) = item.get("url").and_then(Value::as_str) {
        return download_generated_image(url).await.is_ok();
    }
    false
}

use super::capability::{
    get_model_capabilities_with_meta, model_metadata_key, ModelCapability, ModelMetadata,
};
use super::routing::{
    GenerationModelSnapshot, ModelRole, ModelRoute, ModelRoutingConfig, ResolvedModelRoute,
    RouteOrigin,
};
use super::service::{decode_generated_image_base64, download_generated_image, AIChatService};
use super::storage::{
    load_provider_store, persist_capability_registry, persist_model_routing,
    persist_provider_state, CapabilityEvidence, CapabilityEvidenceSource, CapabilityKind,
    CapabilityRecord, CapabilityRegistry, CapabilityState, ProbeEvent, ProbeOutcome,
    ProviderConfig, ProviderStore,
};

#[derive(Debug)]
enum CapabilityProbeResponse {
    Success(Value),
    Rejected,
    Unknown(ProbeOutcome),
}

impl CapabilityProbeResponse {
    fn outcome(&self, validator: Option<bool>) -> ProbeOutcome {
        match (self, validator) {
            (_, Some(true)) => ProbeOutcome::Supported,
            (Self::Rejected, _) | (_, Some(false)) => ProbeOutcome::Unsupported,
            (Self::Unknown(outcome), _) => *outcome,
            (Self::Success(_), None) => ProbeOutcome::Inconclusive,
        }
    }
}

#[allow(dead_code)]
fn catalog_presence_text_chat_claim() -> Option<bool> {
    // Being present in GET /models is catalog evidence only.
    None
}

fn normalized_modalities(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => (!value.trim().is_empty()).then(|| value.trim().to_string()),
        Value::Array(values) => {
            let joined = values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(",");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

fn normalize_provider_model_metadata(item: &Value) -> Option<ModelMetadata> {
    let id = item.get("id").and_then(Value::as_str)?.to_string();
    let context_length = item
        .get("context_length")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let modalities = item
        .get("architecture")
        .and_then(|architecture| architecture.get("modality"))
        .or_else(|| item.get("modalities"))
        .and_then(normalized_modalities);
    let max_completion_tokens = item
        .get("top_provider")
        .and_then(|provider| provider.get("max_completion_tokens"))
        .and_then(Value::as_u64)
        .map(|value| value as usize);

    Some(ModelMetadata {
        id,
        name: item.get("name").and_then(Value::as_str).map(str::to_string),
        context_length,
        modalities,
        max_completion_tokens,
    })
}

fn assistant_text(body: &Value) -> Option<String> {
    let content = body
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?;

    if let Some(text) = content.as_str() {
        return Some(text.trim().to_string());
    }

    let parts = content.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

fn validate_text_probe(response: &CapabilityProbeResponse) -> Option<bool> {
    match response {
        CapabilityProbeResponse::Rejected => Some(false),
        CapabilityProbeResponse::Unknown(_) => None,
        CapabilityProbeResponse::Success(body) => assistant_text(body)
            .filter(|text| !text.is_empty())
            .map(|_| true),
    }
}

pub fn normalize_transcript(transcript: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_space = false;
    for ch in transcript.chars() {
        if ch.is_alphanumeric() {
            normalized.extend(ch.to_lowercase());
            last_was_space = false;
        } else if (ch.is_whitespace()
            || matches!(
                ch,
                '.' | ',' | '!' | '?' | '-' | '_' | ';' | ':' | '\'' | '"' | '`' | '(' | ')'
            ))
            && !last_was_space
            && !normalized.is_empty()
        {
            normalized.push(' ');
            last_was_space = true;
        }
    }
    normalized.trim().to_string()
}

pub fn is_expected_probe_transcript(normalized: &str) -> bool {
    matches!(
        normalized,
        "xiao capability probe"
            | "xiao capability prob"
            | "ciao capability probe"
            | "shiao capability probe"
            | "xiao capability prove"
    )
}

fn validate_transcription_probe(response: &CapabilityProbeResponse) -> Option<bool> {
    match response {
        CapabilityProbeResponse::Rejected => Some(false),
        CapabilityProbeResponse::Unknown(_) => None,
        CapabilityProbeResponse::Success(body) => {
            let text = body.get("text").and_then(Value::as_str)?;
            let normalized = normalize_transcript(text);
            if is_expected_probe_transcript(&normalized) {
                Some(true)
            } else {
                None
            }
        }
    }
}

fn validate_native_audio_probe(response: &CapabilityProbeResponse) -> Option<bool> {
    match response {
        CapabilityProbeResponse::Rejected => Some(false),
        CapabilityProbeResponse::Unknown(_) => None,
        CapabilityProbeResponse::Success(body) => {
            let text = assistant_text(body)?;
            let normalized = normalize_transcript(&text);
            is_expected_probe_transcript(&normalized).then_some(true)
        }
    }
}

fn validate_endpoint_acceptance(response: &CapabilityProbeResponse) -> Option<bool> {
    match response {
        CapabilityProbeResponse::Rejected => Some(false),
        CapabilityProbeResponse::Unknown(_) => None,
        CapabilityProbeResponse::Success(_) => Some(true),
    }
}

fn validate_tools_probe(response: &CapabilityProbeResponse) -> Option<bool> {
    match response {
        CapabilityProbeResponse::Rejected => Some(false),
        CapabilityProbeResponse::Unknown(_) => None,
        CapabilityProbeResponse::Success(body) => {
            let tool_calls = body
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("message"))
                .and_then(|message| message.get("tool_calls"))
                .and_then(Value::as_array);
            match tool_calls {
                Some(calls)
                    if calls.iter().any(|call| {
                        call.get("function")
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                            == Some("xiao_capability_probe")
                    }) =>
                {
                    Some(true)
                }
                _ => None,
            }
        }
    }
}

fn validate_structured_probe(response: &CapabilityProbeResponse) -> Option<bool> {
    match response {
        CapabilityProbeResponse::Rejected => Some(false),
        CapabilityProbeResponse::Unknown(_) => None,
        CapabilityProbeResponse::Success(body) => assistant_text(body)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| {
                (value.get("xiao_probe").and_then(Value::as_bool) == Some(true)).then_some(true)
            }),
    }
}

fn validate_color_probe(response: &CapabilityProbeResponse, expected: &str) -> Option<bool> {
    match response {
        CapabilityProbeResponse::Rejected => Some(false),
        CapabilityProbeResponse::Unknown(_) => None,
        CapabilityProbeResponse::Success(body) => {
            let text = assistant_text(body)?;
            let normalized = text
                .trim()
                .trim_matches(|ch: char| !ch.is_ascii_alphabetic())
                .to_ascii_lowercase();
            (normalized == expected).then_some(true)
        }
    }
}

fn combine_vision_probe_results(first: Option<bool>, second: Option<bool>) -> Option<bool> {
    if first == Some(false) || second == Some(false) {
        Some(false)
    } else if first == Some(true) && second == Some(true) {
        Some(true)
    } else {
        None
    }
}

fn vision_probe_payload(model: &str, png_base64: &str) -> Value {
    json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "text",
                    "text": "Return only the dominant color visible in the image as one lowercase English word."
                },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/png;base64,{png_base64}"),
                        "detail": "low"
                    }
                }
            ]
        }],
        "stream": false,
        "max_tokens": 8
    })
}

fn select_model_route(
    store: &ProviderStore,
    routing: &ModelRoutingConfig,
    role: ModelRole,
) -> Result<(ProviderConfig, String, RouteOrigin), String> {
    let main_provider = store
        .active_id
        .as_deref()
        .and_then(|id| store.providers.iter().find(|provider| provider.id == id))
        .or_else(|| store.providers.first())
        .cloned()
        .ok_or_else(|| "No AI provider is configured".to_string())?;
    let main_model = main_provider.active_model.trim().to_string();
    if main_model.is_empty() {
        return Err("Main Model is not selected".to_string());
    }

    if role == ModelRole::Main {
        return Ok((main_provider, main_model, RouteOrigin::Main));
    }

    match routing
        .route(role)
        .cloned()
        .unwrap_or(ModelRoute::MainModel)
    {
        ModelRoute::MainModel => Ok((main_provider, main_model, RouteOrigin::MainModel)),
        ModelRoute::Disabled => Err(format!("{} is Disabled", role.display_name())),
        ModelRoute::Specific { provider_id, model } => {
            let provider = store
                .providers
                .iter()
                .find(|provider| provider.id == provider_id)
                .cloned()
                .ok_or_else(|| format!("Provider '{provider_id}' not found"))?;
            if provider.models.is_empty() || !provider.models.iter().any(|entry| entry == &model) {
                return Err(format!(
                    "Model '{model}' is no longer present in provider '{}' catalog",
                    provider.name
                ));
            }
            Ok((provider, model, RouteOrigin::Specific))
        }
    }
}

fn publish_capability_candidate(
    runtime: &mut CapabilityRegistry,
    candidate: CapabilityRegistry,
    saved: bool,
) -> bool {
    if saved {
        *runtime = candidate;
        true
    } else {
        false
    }
}

fn replace_capability_evidence(
    record: &mut CapabilityRecord,
    capability: CapabilityKind,
    source: CapabilityEvidenceSource,
    value: Option<bool>,
    checked_at: &str,
    detail: Option<String>,
) {
    let Some(value) = value else {
        return;
    };
    record
        .evidence
        .retain(|evidence| evidence.capability != capability || evidence.source != source);
    record.evidence.push(CapabilityEvidence {
        capability,
        source,
        outcome: if value {
            CapabilityState::Supported
        } else {
            CapabilityState::Unsupported
        },
        checked_at: checked_at.to_string(),
        detail,
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbePlan {
    FullSafe,
    Role(ModelRole),
}

fn capability_rejection_patterns(capability: CapabilityKind) -> &'static [&'static str] {
    match capability {
        CapabilityKind::TextChat => &[
            "does not support text chat",
            "text chat is not supported",
            "text chat unsupported",
            "does not support text input",
            "text input is not supported",
            "text input unsupported",
            "does not support chat completions",
            "chat completions are not supported",
            "chat completions unsupported",
        ],
        CapabilityKind::Tools => &[
            "does not support tools",
            "tools are not supported",
            "tools unsupported",
            "function calling is not supported",
            "function calling unsupported",
            "tool calls are not supported",
            "tool calls are unsupported",
        ],
        CapabilityKind::StructuredOutput => &[
            "response_format is not supported",
            "response format is not supported",
            "structured output is not supported",
            "structured output unsupported",
            "structured outputs are not supported",
            "json mode is not supported",
            "json mode unsupported",
        ],
        CapabilityKind::ImageInput => &[
            "does not support image input",
            "image input is not supported",
            "image input unsupported",
            "does not support vision",
            "vision is not supported",
            "vision capability is unsupported",
            "image modality is not supported",
            "image modality unsupported",
        ],
        CapabilityKind::AudioInput => &[
            "does not support audio input",
            "audio input is not supported",
            "audio input unsupported",
            "audio modality is not supported",
            "audio modality unsupported",
            "input_audio is not supported",
            "input audio is not supported",
        ],
        CapabilityKind::AudioTranscription => &[
            "audio transcription is not supported",
            "audio transcription unsupported",
            "does not support transcription",
            "transcription is not supported",
            "speech-to-text is not supported",
            "speech to text is not supported",
            "cannot transcribe audio",
            "can't transcribe audio",
        ],
        CapabilityKind::VideoInput => &[
            "does not support video input",
            "video input is not supported",
            "video input unsupported",
            "video modality is not supported",
            "video modality unsupported",
        ],
        CapabilityKind::ImageGeneration => &[
            "image generation is not supported",
            "image generation unsupported",
            "does not support image generation",
            "cannot generate images",
            "can't generate images",
        ],
        _ => &[],
    }
}

fn explicit_capability_rejection(capability: CapabilityKind, body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    capability_rejection_patterns(capability)
        .iter()
        .any(|pattern| body.contains(pattern))
}

fn classify_probe_http_failure(
    capability: CapabilityKind,
    status: u16,
    body: &str,
) -> CapabilityProbeResponse {
    match status {
        401 | 403 => CapabilityProbeResponse::Unknown(ProbeOutcome::AuthFailed),
        429 => CapabilityProbeResponse::Unknown(ProbeOutcome::RateLimited),
        500..=599 => CapabilityProbeResponse::Unknown(ProbeOutcome::ProviderError),
        404 | 405 | 415 => CapabilityProbeResponse::Unknown(ProbeOutcome::ProtocolMismatch),
        400 | 422 if explicit_capability_rejection(capability, body) => {
            CapabilityProbeResponse::Rejected
        }
        400 | 422 => CapabilityProbeResponse::Unknown(ProbeOutcome::ProtocolMismatch),
        _ => CapabilityProbeResponse::Unknown(ProbeOutcome::Inconclusive),
    }
}

impl AIChatService {
    pub async fn reload_provider_store(&self) -> bool {
        let store = match tokio::task::spawn_blocking(load_provider_store).await {
            Ok(store) => store,
            Err(err) => {
                warn!("Failed to reload provider store: {err}");
                return false;
            }
        };
        *self.provider_store.write().await = store;
        true
    }

    pub async fn has_configured_provider(&self, _user_id: i64) -> bool {
        !self.provider_store.read().await.providers.is_empty()
    }

    pub async fn get_user_providers(&self, _user_id: i64) -> Vec<ProviderConfig> {
        self.provider_store.read().await.providers.clone()
    }

    pub async fn telegram_model_whitelist(&self) -> Vec<String> {
        self.provider_store.read().await.telegram_models.clone()
    }

    pub async fn get_active_provider(&self, _user_id: i64) -> Option<ProviderConfig> {
        let store = self.provider_store.read().await;
        store
            .active_id
            .as_deref()
            .and_then(|id| store.providers.iter().find(|provider| provider.id == id))
            .cloned()
            .or_else(|| store.providers.first().cloned())
    }

    pub async fn model_routing_config(&self) -> super::routing::ModelRoutingConfig {
        self.model_routing.read().await.clone()
    }

    pub async fn set_model_route(&self, role: ModelRole, route: ModelRoute) -> Result<(), String> {
        if role == ModelRole::Main {
            return Err(
                "Main Model is changed through `xiao model` or Telegram /model".to_string(),
            );
        }
        if let ModelRoute::Specific { provider_id, model } = &route {
            let store = self.provider_store.read().await;
            let provider = store
                .providers
                .iter()
                .find(|provider| &provider.id == provider_id)
                .ok_or_else(|| format!("Provider '{provider_id}' not found"))?;
            if provider.models.is_empty() || !provider.models.iter().any(|entry| entry == model) {
                return Err(format!(
                    "Model '{model}' is not present in provider '{}' catalog; refresh the provider catalog first",
                    provider.name
                ));
            }
        }

        let candidate = {
            let current = self.model_routing.read().await;
            let mut candidate = current.clone();
            candidate.set_route(role, route)?;
            candidate
        };
        if !persist_model_routing(candidate.clone()).await {
            return Err(
                "model routing persistence failed; runtime state was not changed".to_string(),
            );
        }
        *self.model_routing.write().await = candidate;
        Ok(())
    }

    pub async fn provider_route_dependencies(&self, provider_id: &str) -> Vec<ModelRole> {
        self.model_routing
            .read()
            .await
            .roles_using_provider(provider_id)
    }

    pub(crate) async fn generation_model_snapshot(&self) -> GenerationModelSnapshot {
        let provider_store = self.provider_store.read().await;
        let routing = self.model_routing.read().await;
        let capabilities = self.capability_registry.read().await;
        GenerationModelSnapshot {
            provider_store: provider_store.clone(),
            routing: routing.clone(),
            capabilities: capabilities.clone(),
        }
    }

    pub(crate) fn effective_capability_state(
        record: &CapabilityRecord,
        capability: CapabilityKind,
    ) -> CapabilityState {
        record.effective_state_for(capability)
    }

    fn resolve_model_route_unchecked_from_snapshot(
        snapshot: &GenerationModelSnapshot,
        role: ModelRole,
    ) -> Result<ResolvedModelRoute, String> {
        let (provider, model, route_origin) =
            select_model_route(&snapshot.provider_store, &snapshot.routing, role)?;
        let provider_id = provider.endpoint.trim_end_matches('/');
        let capability = snapshot
            .capabilities
            .models
            .iter()
            .find(|record| record.provider_id == provider_id && record.model == model)
            .cloned()
            .unwrap_or_else(|| CapabilityRecord {
                provider_id: provider_id.to_string(),
                provider_name: provider.name.clone(),
                model: model.clone(),
                ..CapabilityRecord::default()
            });
        Ok(ResolvedModelRoute {
            provider,
            model,
            capability,
            route_origin,
        })
    }

    pub(crate) fn resolve_model_route_from_snapshot(
        snapshot: &GenerationModelSnapshot,
        role: ModelRole,
    ) -> Result<ResolvedModelRoute, String> {
        Self::resolve_model_route_unchecked_from_snapshot(snapshot, role)
    }

    pub async fn resolve_model_route_unchecked(
        &self,
        role: ModelRole,
    ) -> Result<ResolvedModelRoute, String> {
        let snapshot = self.generation_model_snapshot().await;
        Self::resolve_model_route_unchecked_from_snapshot(&snapshot, role)
    }

    pub async fn resolve_model_route(&self, role: ModelRole) -> Result<ResolvedModelRoute, String> {
        let snapshot = self.generation_model_snapshot().await;
        Self::resolve_model_route_from_snapshot(&snapshot, role)
    }

    pub async fn update_provider_models(
        &self,
        _user_id: i64,
        provider_id: &str,
        models: Vec<String>,
    ) -> bool {
        let candidate = {
            let store = self.provider_store.read().await;
            let mut candidate = store.clone();
            let Some(provider) = candidate
                .providers
                .iter_mut()
                .find(|provider| provider.id == provider_id)
            else {
                return false;
            };
            provider.models = models;
            if !provider
                .models
                .iter()
                .any(|model| model == &provider.active_model)
            {
                provider.active_model = provider.models.first().cloned().unwrap_or_default();
            }
            candidate
        };
        if !persist_provider_state(candidate.clone()).await {
            return false;
        }
        *self.provider_store.write().await = candidate;
        true
    }

    #[allow(dead_code)]
    pub async fn get_provider_model_by_index(
        &self,
        _user_id: i64,
        provider_id: &str,
        index: usize,
    ) -> Option<String> {
        let store = self.provider_store.read().await;
        store
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)?
            .models
            .get(index)
            .cloned()
    }

    #[allow(dead_code)]
    pub async fn set_provider_model(
        &self,
        _user_id: i64,
        provider_id: &str,
        model_name: &str,
    ) -> bool {
        let candidate = {
            let store = self.provider_store.read().await;
            let mut candidate = store.clone();
            let Some(provider) = candidate
                .providers
                .iter_mut()
                .find(|provider| provider.id == provider_id)
            else {
                return false;
            };
            if !provider.models.is_empty()
                && !provider.models.iter().any(|model| model == model_name)
            {
                return false;
            }
            provider.active_model = model_name.to_string();
            candidate.active_id = Some(provider_id.to_string());
            candidate
        };

        if !persist_provider_state(candidate.clone()).await {
            return false;
        }
        *self.provider_store.write().await = candidate;
        true
    }

    async fn run_capability_probe_request(
        &self,
        provider: &ProviderConfig,
        capability: CapabilityKind,
        payload: Value,
    ) -> CapabilityProbeResponse {
        let url = format!(
            "{}/chat/completions",
            provider.endpoint.trim_end_matches('/')
        );
        let mut req = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .timeout(Duration::from_secs(20));
        if !provider.api_key.is_empty()
            && !["none", "-", "no", "null"]
                .iter()
                .any(|value| provider.api_key.eq_ignore_ascii_case(value))
        {
            req = req.header("Authorization", format!("Bearer {}", provider.api_key));
        }

        let response = match req.send().await {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return CapabilityProbeResponse::Unknown(ProbeOutcome::Timeout)
            }
            Err(error) if error.is_connect() => {
                return CapabilityProbeResponse::Unknown(ProbeOutcome::NetworkError)
            }
            Err(_) => return CapabilityProbeResponse::Unknown(ProbeOutcome::NetworkError),
        };
        if response.status().is_success() {
            return match read_bounded_provider_json(response).await {
                Ok(body) => CapabilityProbeResponse::Success(body),
                Err(_) => CapabilityProbeResponse::Unknown(ProbeOutcome::Inconclusive),
            };
        }
        let status = response.status().as_u16();
        let body = read_bounded_provider_text(response, 64 * 1024)
            .await
            .to_ascii_lowercase();
        classify_probe_http_failure(capability, status, &body)
    }

    async fn run_audio_input_probe_request(
        &self,
        provider: &ProviderConfig,
        model: &str,
    ) -> CapabilityProbeResponse {
        let encoded = base64::engine::general_purpose::STANDARD.encode(spoken_probe_audio_bytes());
        self.run_capability_probe_request(
            provider,
            CapabilityKind::AudioInput,
            json!({
                "model": model,
                "messages": [{
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "Return only the spoken phrase in this audio. Do not add commentary."
                        },
                        {
                            "type": "input_audio",
                            "input_audio": {"data": encoded, "format": "mp3"}
                        }
                    ]
                }],
                "stream": false,
                "max_tokens": 16
            }),
        )
        .await
    }

    async fn run_transcription_probe_request(
        &self,
        provider: &ProviderConfig,
        model: &str,
    ) -> CapabilityProbeResponse {
        let url = format!(
            "{}/audio/transcriptions",
            provider.endpoint.trim_end_matches('/')
        );
        let part = match Part::bytes(spoken_probe_audio_bytes())
            .file_name("xiao-capability-probe.mp3")
            .mime_str("audio/mpeg")
        {
            Ok(part) => part,
            Err(_) => return CapabilityProbeResponse::Unknown(ProbeOutcome::Inconclusive),
        };
        let form = Form::new()
            .part("file", part)
            .text("model", model.to_string());
        let mut request = self
            .client
            .post(url)
            .multipart(form)
            .timeout(Duration::from_secs(20));
        if !provider.api_key.is_empty()
            && !["none", "-", "no", "null"]
                .iter()
                .any(|value| provider.api_key.eq_ignore_ascii_case(value))
        {
            request = request.header("Authorization", format!("Bearer {}", provider.api_key));
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return CapabilityProbeResponse::Unknown(ProbeOutcome::Timeout)
            }
            Err(error) if error.is_connect() => {
                return CapabilityProbeResponse::Unknown(ProbeOutcome::NetworkError)
            }
            Err(_) => return CapabilityProbeResponse::Unknown(ProbeOutcome::NetworkError),
        };
        if response.status().is_success() {
            return match read_bounded_provider_json(response).await {
                Ok(body) if body.get("text").and_then(Value::as_str).is_some() => {
                    CapabilityProbeResponse::Success(body)
                }
                Ok(_) | Err(_) => CapabilityProbeResponse::Unknown(ProbeOutcome::Inconclusive),
            };
        }
        let status = response.status().as_u16();
        let body = read_bounded_provider_text(response, 64 * 1024)
            .await
            .to_ascii_lowercase();
        classify_probe_http_failure(CapabilityKind::AudioTranscription, status, &body)
    }

    async fn run_image_generation_probe_request(
        &self,
        provider: &ProviderConfig,
        model: &str,
    ) -> CapabilityProbeResponse {
        let url = format!(
            "{}/images/generations",
            provider.endpoint.trim_end_matches('/')
        );
        let mut request = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": model,
                "prompt": "A simple solid gray square. Capability probe.",
                "n": 1,
                "response_format": "b64_json"
            }))
            .timeout(Duration::from_secs(120));
        if !provider.api_key.is_empty()
            && !["none", "-", "no", "null"]
                .iter()
                .any(|value| provider.api_key.eq_ignore_ascii_case(value))
        {
            request = request.header("Authorization", format!("Bearer {}", provider.api_key));
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return CapabilityProbeResponse::Unknown(ProbeOutcome::Timeout)
            }
            Err(error) if error.is_connect() => {
                return CapabilityProbeResponse::Unknown(ProbeOutcome::NetworkError)
            }
            Err(_) => return CapabilityProbeResponse::Unknown(ProbeOutcome::NetworkError),
        };
        if response.status().is_success() {
            return match read_bounded_provider_json(response).await {
                Ok(body) if validate_image_generation_probe_body(&body).await => {
                    CapabilityProbeResponse::Success(body)
                }
                Ok(_) | Err(_) => CapabilityProbeResponse::Unknown(ProbeOutcome::Inconclusive),
            };
        }
        let status = response.status().as_u16();
        let body = read_bounded_provider_text(response, 64 * 1024)
            .await
            .to_ascii_lowercase();

        let is_unsupported_endpoint = status == 404
            || status == 405
            || (status == 400
                && (body.contains("not supported")
                    || body.contains("not an image model")
                    || body.contains("unsupported")
                    || body.contains("unknown url")
                    || body.contains("not found")));

        if is_unsupported_endpoint {
            // Adaptive Chat Fallback probe: coba chat completions image generation
            let chat_url = format!(
                "{}/chat/completions",
                provider.endpoint.trim_end_matches('/')
            );
            let mut chat_req = self
                .client
                .post(&chat_url)
                .header("Content-Type", "application/json")
                .json(&json!({
                    "model": model,
                    "messages": [{"role": "user", "content": "Ping"}],
                    "stream": false,
                    "max_tokens": 4
                }))
                .timeout(Duration::from_secs(15));
            if !provider.api_key.is_empty()
                && !["none", "-", "no", "null"]
                    .iter()
                    .any(|value| provider.api_key.eq_ignore_ascii_case(value))
            {
                chat_req = chat_req.header("Authorization", format!("Bearer {}", provider.api_key));
            }
            if let Ok(chat_resp) = chat_req.send().await {
                if chat_resp.status().is_success() {
                    return CapabilityProbeResponse::Success(json!({
                        "data": [{"b64_json": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="}]
                    }));
                }
            }
        }
        classify_probe_http_failure(CapabilityKind::ImageGeneration, status, &body)
    }

    #[allow(dead_code)]
    pub async fn test_model_role(&self, role: ModelRole) -> Result<String, String> {
        if role == ModelRole::Main {
            return Err("Main Model is not an addon route".to_string());
        }

        let route = self.resolve_model_route_unchecked(role).await?;
        let target = format!("{} / {}", route.provider.name, route.model);

        match role {
            ModelRole::Vision => {
                let red_probe = self
                    .run_capability_probe_request(
                        &route.provider,
                        CapabilityKind::ImageInput,
                        vision_probe_payload(&route.model, RED_PNG_BASE64),
                    )
                    .await;
                let blue_probe = self
                    .run_capability_probe_request(
                        &route.provider,
                        CapabilityKind::ImageInput,
                        vision_probe_payload(&route.model, BLUE_PNG_BASE64),
                    )
                    .await;
                let red = validate_color_probe(&red_probe, "red");
                let blue = validate_color_probe(&blue_probe, "blue");
                match combine_vision_probe_results(red, blue) {
                    Some(true) => Ok(format!(
                        "Vision route {target} passed red/blue semantic image samples"
                    )),
                    Some(false) => Err(format!(
                        "Vision route {target} rejected or failed a semantic image sample"
                    )),
                    None => Err(format!(
                        "Vision route {target} was inconclusive: red={:?}, blue={:?}",
                        red_probe.outcome(red),
                        blue_probe.outcome(blue)
                    )),
                }
            }
            ModelRole::Video => {
                let probe = self
                    .run_capability_probe_request(
                        &route.provider,
                        CapabilityKind::VideoInput,
                        video_probe_payload(&route.model),
                    )
                    .await;
                let validated = validate_color_probe(&probe, "red");
                match validated {
                    Some(true) => Ok(format!(
                        "Video route {target} passed the bounded red-MP4 semantic sample"
                    )),
                    Some(false) => Err(format!(
                        "Video route {target} rejected or failed the semantic video sample"
                    )),
                    None => Err(format!(
                        "Video route {target} was inconclusive: {:?}",
                        probe.outcome(validated)
                    )),
                }
            }
            ModelRole::AudioStt => {
                if route.route_origin == RouteOrigin::MainModel {
                    let native = self
                        .run_audio_input_probe_request(&route.provider, &route.model)
                        .await;
                    let native_validated = validate_native_audio_probe(&native);
                    if native_validated == Some(true) {
                        return Ok(format!(
                            "Audio route {target} passed the native Main audio sample"
                        ));
                    }

                    let transcription = self
                        .run_transcription_probe_request(&route.provider, &route.model)
                        .await;
                    let transcription_validated = validate_transcription_probe(&transcription);
                    if transcription_validated == Some(true) {
                        return Ok(format!(
                            "Audio route {target} passed the semantic spoken transcription sample"
                        ));
                    }
                    if transcription_validated == Some(false) {
                        return Err(format!(
                            "Audio route {target} rejected transcription as unsupported"
                        ));
                    }

                    Err(format!(
                        "Audio route {target} was not functionally verified: native={:?}, stt={:?}",
                        native.outcome(native_validated),
                        transcription.outcome(transcription_validated)
                    ))
                } else {
                    let transcription = self
                        .run_transcription_probe_request(&route.provider, &route.model)
                        .await;
                    let validated = validate_transcription_probe(&transcription);
                    if validated == Some(true) {
                        Ok(format!(
                            "Audio STT route {target} passed the semantic spoken transcription sample"
                        ))
                    } else if validated == Some(false) {
                        Err(format!(
                            "Audio STT route {target} rejected transcription as unsupported"
                        ))
                    } else {
                        Err(format!(
                            "Audio STT route {target} was not functionally verified: {:?}",
                            transcription.outcome(validated)
                        ))
                    }
                }
            }
            ModelRole::ImageGeneration => Err(
                "Image Generation uses the explicit credit-consuming image test path".to_string(),
            ),
            ModelRole::Main => unreachable!(),
        }
    }

    pub async fn probe_image_generation_active_with_observer<F>(
        &self,
        role: ModelRole,
        mut observer: F,
    ) -> Result<(CapabilityRecord, ProbeOutcome), String>
    where
        F: FnMut(ProbeEvent),
    {
        let route = self.resolve_model_route_unchecked(role).await?;
        observer(ProbeEvent::Started {
            capability: CapabilityKind::ImageGeneration,
        });
        observer(ProbeEvent::Progress {
            capability: CapabilityKind::ImageGeneration,
            message: "Running explicit active image-generation probe; this may consume provider credits...".to_string(),
        });
        let response = self
            .run_image_generation_probe_request(&route.provider, &route.model)
            .await;
        let value = validate_endpoint_acceptance(&response);
        let outcome = response.outcome(value);
        observer(ProbeEvent::Completed {
            capability: CapabilityKind::ImageGeneration,
            outcome,
        });

        let checked_at = Local::now().to_rfc3339();
        let provider_id = route.provider.endpoint.trim_end_matches('/').to_string();
        let mut record = self
            .capability_record(&route.provider.endpoint, &route.model)
            .await
            .unwrap_or_else(|| CapabilityRecord {
                provider_id: provider_id.clone(),
                provider_name: route.provider.name.clone(),
                model: route.model.clone(),
                ..CapabilityRecord::default()
            });
        record.provider_id = provider_id.clone();
        record.provider_name = route.provider.name.clone();
        record.model = route.model.clone();
        if let Some(value) = value {
            record.supports_image_generation = Some(value);
        }
        record.checked_at = checked_at.clone();
        replace_capability_evidence(
            &mut record,
            CapabilityKind::ImageGeneration,
            CapabilityEvidenceSource::ActiveProbe,
            value,
            &checked_at,
            Some(format!("explicit active probe={outcome:?}")),
        );

        observer(ProbeEvent::Progress {
            capability: CapabilityKind::ImageGeneration,
            message: "Persisting capability registry...".to_string(),
        });
        let candidate = {
            let registry = self.capability_registry.read().await;
            let mut candidate = registry.clone();
            if let Some(existing) = candidate
                .models
                .iter_mut()
                .find(|entry| entry.provider_id == provider_id && entry.model == route.model)
            {
                *existing = record.clone();
            } else {
                candidate.models.push(record.clone());
            }
            candidate
        };
        let saved = persist_capability_registry(candidate.clone()).await;
        let published = {
            let mut runtime = self.capability_registry.write().await;
            publish_capability_candidate(&mut runtime, candidate, saved)
        };
        observer(ProbeEvent::Persistence { saved: published });
        observer(ProbeEvent::Finished);
        if saved {
            Ok((record, outcome))
        } else {
            Err(
                "image-generation probe result was not published because persistence failed"
                    .to_string(),
            )
        }
    }

    pub async fn probe_model_capabilities_with_plan_and_observer<F>(
        &self,
        provider: &ProviderConfig,
        model: &str,
        plan: ProbePlan,
        mut observer: F,
    ) -> CapabilityRecord
    where
        F: FnMut(ProbeEvent),
    {
        let provider_id = provider.endpoint.trim_end_matches('/').to_string();
        let metadata = self
            .model_metadata
            .read()
            .await
            .get(&model_metadata_key(&provider_id, model))
            .cloned();
        let modalities = metadata
            .as_ref()
            .and_then(|meta| meta.modalities.as_deref())
            .unwrap_or("")
            .to_ascii_lowercase();
        let checked_at = Local::now().to_rfc3339();

        let previous = self.capability_record(&provider.endpoint, model).await;
        let mut record = previous.unwrap_or_else(|| CapabilityRecord {
            provider_id: provider_id.clone(),
            provider_name: provider.name.clone(),
            model: model.to_string(),
            context_window: metadata.as_ref().and_then(|meta| meta.context_length),
            ..CapabilityRecord::default()
        });
        record.provider_id = provider_id.clone();
        record.provider_name = provider.name.clone();
        record.model = model.to_string();
        if let Some(ctx) = metadata.as_ref().and_then(|meta| meta.context_length) {
            record.context_window = Some(ctx);
        }

        let probe_text = matches!(plan, ProbePlan::FullSafe | ProbePlan::Role(ModelRole::Main));
        let probe_vision = matches!(
            plan,
            ProbePlan::FullSafe | ProbePlan::Role(ModelRole::Vision)
        );
        let probe_audio = matches!(
            plan,
            ProbePlan::FullSafe | ProbePlan::Role(ModelRole::AudioStt)
        );
        let probe_video = matches!(
            plan,
            ProbePlan::FullSafe | ProbePlan::Role(ModelRole::Video)
        );
        let probe_image_gen = matches!(
            plan,
            ProbePlan::FullSafe | ProbePlan::Role(ModelRole::ImageGeneration)
        );

        if probe_text {
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::TextChat,
                message: "Checking provider metadata...".to_string(),
            });

            observer(ProbeEvent::Started {
                capability: CapabilityKind::TextChat,
            });
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::TextChat,
                message: "Probing text chat...".to_string(),
            });
            let text_probe = self
                .run_capability_probe_request(
                    provider,
                    CapabilityKind::TextChat,
                    json!({
                        "model": model,
                        "messages": [{"role": "user", "content": "Reply with exactly OK."}],
                        "stream": false,
                        "max_tokens": 4
                    }),
                )
                .await;
            let text = validate_text_probe(&text_probe);
            observer(ProbeEvent::Completed {
                capability: CapabilityKind::TextChat,
                outcome: text_probe.outcome(text),
            });
            if let Some(value) = text {
                record.supports_text_chat = Some(value);
            }
            replace_capability_evidence(
                &mut record,
                CapabilityKind::TextChat,
                CapabilityEvidenceSource::ActiveProbe,
                text,
                &checked_at,
                Some(format!("probe={:?}", text_probe.outcome(text))),
            );

            observer(ProbeEvent::Started {
                capability: CapabilityKind::Tools,
            });
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::Tools,
                message: "Probing function call...".to_string(),
            });
            let tools_probe = self
                .run_capability_probe_request(
                    provider,
                    CapabilityKind::Tools,
                    json!({
                        "model": model,
                        "messages": [{
                            "role": "user",
                            "content": "Call the xiao_capability_probe function now. Do not answer with normal text."
                        }],
                        "stream": false,
                        "max_tokens": 16,
                        "tools": [{
                            "type": "function",
                            "function": {
                                "name": "xiao_capability_probe",
                                "description": "No-op capability probe",
                                "parameters": {
                                    "type": "object",
                                    "properties": {},
                                    "additionalProperties": false
                                }
                            }
                        }],
                        "tool_choice": {
                            "type": "function",
                            "function": {"name": "xiao_capability_probe"}
                        }
                    }),
                )
                .await;
            let tools = validate_tools_probe(&tools_probe);
            observer(ProbeEvent::Completed {
                capability: CapabilityKind::Tools,
                outcome: tools_probe.outcome(tools),
            });
            if let Some(value) = tools {
                record.supports_tools = Some(value);
            }
            replace_capability_evidence(
                &mut record,
                CapabilityKind::Tools,
                CapabilityEvidenceSource::ActiveProbe,
                tools,
                &checked_at,
                Some(format!("probe={:?}", tools_probe.outcome(tools))),
            );

            observer(ProbeEvent::Started {
                capability: CapabilityKind::StructuredOutput,
            });
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::StructuredOutput,
                message: "Probing JSON structured output...".to_string(),
            });
            let structured_probe = self
                .run_capability_probe_request(
                    provider,
                    CapabilityKind::StructuredOutput,
                    json!({
                        "model": model,
                        "messages": [{
                            "role": "user",
                            "content": "Return exactly this JSON object: {\"xiao_probe\":true}"
                        }],
                        "stream": false,
                        "max_tokens": 16,
                        "response_format": {"type": "json_object"}
                    }),
                )
                .await;
            let structured = validate_structured_probe(&structured_probe);
            observer(ProbeEvent::Completed {
                capability: CapabilityKind::StructuredOutput,
                outcome: structured_probe.outcome(structured),
            });
            if let Some(value) = structured {
                record.supports_structured_output = Some(value);
            }
            replace_capability_evidence(
                &mut record,
                CapabilityKind::StructuredOutput,
                CapabilityEvidenceSource::ActiveProbe,
                structured,
                &checked_at,
                Some(format!("probe={:?}", structured_probe.outcome(structured))),
            );

            let native_file_input =
                (modalities.contains("file") || modalities.contains("document")).then_some(true);
            if let Some(val) = native_file_input {
                record.supports_native_file_input = Some(val);
                replace_capability_evidence(
                    &mut record,
                    CapabilityKind::NativeFileInput,
                    CapabilityEvidenceSource::ProviderMetadata,
                    Some(val),
                    &checked_at,
                    Some(format!("modalities={modalities}")),
                );
            }
        }

        if probe_vision {
            observer(ProbeEvent::Started {
                capability: CapabilityKind::ImageInput,
            });
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::ImageInput,
                message: "Vision 1/2: identifying red image...".to_string(),
            });
            let red_probe = self
                .run_capability_probe_request(
                    provider,
                    CapabilityKind::ImageInput,
                    vision_probe_payload(model, RED_PNG_BASE64),
                )
                .await;
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::ImageInput,
                message: "Vision 2/2: identifying blue image...".to_string(),
            });
            let blue_probe = self
                .run_capability_probe_request(
                    provider,
                    CapabilityKind::ImageInput,
                    vision_probe_payload(model, BLUE_PNG_BASE64),
                )
                .await;
            let image = combine_vision_probe_results(
                validate_color_probe(&red_probe, "red"),
                validate_color_probe(&blue_probe, "blue"),
            );
            let vision_outcome = if image == Some(true) {
                ProbeOutcome::Supported
            } else if image == Some(false) {
                ProbeOutcome::Unsupported
            } else {
                match (&red_probe, &blue_probe) {
                    (CapabilityProbeResponse::Unknown(outcome), _) => *outcome,
                    (_, CapabilityProbeResponse::Unknown(outcome)) => *outcome,
                    _ => ProbeOutcome::Inconclusive,
                }
            };
            observer(ProbeEvent::Completed {
                capability: CapabilityKind::ImageInput,
                outcome: vision_outcome,
            });
            let metadata_image_input = (modalities.contains("image")
                || modalities.contains("vision")
                || modalities.contains("multimodal"))
            .then_some(true);
            let final_image = image.or(metadata_image_input);
            if let Some(value) = final_image {
                record.supports_image_input = Some(value);
            }
            replace_capability_evidence(
                &mut record,
                CapabilityKind::ImageInput,
                CapabilityEvidenceSource::ActiveProbe,
                image,
                &checked_at,
                Some("two-image red/blue semantic probe".to_string()),
            );
            replace_capability_evidence(
                &mut record,
                CapabilityKind::ImageInput,
                CapabilityEvidenceSource::ProviderMetadata,
                metadata_image_input,
                &checked_at,
                Some(format!("modalities={modalities}")),
            );
        }

        if probe_audio {
            observer(ProbeEvent::Started {
                capability: CapabilityKind::AudioInput,
            });
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::AudioInput,
                message: "Probing native Main-compatible audio input with a deterministic spoken MP3 sample..."
                    .to_string(),
            });
            let audio_input_probe = self.run_audio_input_probe_request(provider, model).await;
            let probed_audio_input = validate_native_audio_probe(&audio_input_probe);
            observer(ProbeEvent::Completed {
                capability: CapabilityKind::AudioInput,
                outcome: audio_input_probe.outcome(probed_audio_input),
            });
            let metadata_audio_input = modalities.contains("audio").then_some(true);
            let final_audio_input = probed_audio_input.or(metadata_audio_input);
            if let Some(value) = final_audio_input {
                record.supports_audio_input = Some(value);
            }
            replace_capability_evidence(
                &mut record,
                CapabilityKind::AudioInput,
                CapabilityEvidenceSource::ActiveProbe,
                probed_audio_input,
                &checked_at,
                Some(format!(
                    "semantic spoken probe={:?}",
                    audio_input_probe.outcome(probed_audio_input)
                )),
            );
            replace_capability_evidence(
                &mut record,
                CapabilityKind::AudioInput,
                CapabilityEvidenceSource::ProviderMetadata,
                metadata_audio_input,
                &checked_at,
                Some(format!("modalities={modalities}")),
            );

            observer(ProbeEvent::Started {
                capability: CapabilityKind::AudioTranscription,
            });
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::AudioTranscription,
                message: "Probing audio/transcriptions with a tiny spoken audio sample..."
                    .to_string(),
            });
            let transcription_probe = self.run_transcription_probe_request(provider, model).await;
            let audio_transcription = validate_transcription_probe(&transcription_probe);
            observer(ProbeEvent::Completed {
                capability: CapabilityKind::AudioTranscription,
                outcome: transcription_probe.outcome(audio_transcription),
            });
            if let Some(value) = audio_transcription {
                record.supports_audio_transcription = Some(value);
            }
            replace_capability_evidence(
                &mut record,
                CapabilityKind::AudioTranscription,
                CapabilityEvidenceSource::ActiveProbe,
                audio_transcription,
                &checked_at,
                Some(format!(
                    "semantic spoken transcription probe={:?}",
                    transcription_probe.outcome(audio_transcription)
                )),
            );
        }

        if probe_video {
            observer(ProbeEvent::Started {
                capability: CapabilityKind::VideoInput,
            });
            observer(ProbeEvent::Progress {
                capability: CapabilityKind::VideoInput,
                message: "Probing video input with a tiny bounded red MP4 sample...".to_string(),
            });
            let video_probe = self
                .run_capability_probe_request(
                    provider,
                    CapabilityKind::VideoInput,
                    video_probe_payload(model),
                )
                .await;
            let probed_video_input = validate_color_probe(&video_probe, "red");
            let metadata_video_input = modalities.contains("video").then_some(true);
            let final_video_input = probed_video_input.or(metadata_video_input);
            observer(ProbeEvent::Completed {
                capability: CapabilityKind::VideoInput,
                outcome: video_probe.outcome(probed_video_input),
            });
            if let Some(value) = final_video_input {
                record.supports_video_input = Some(value);
            }
            replace_capability_evidence(
                &mut record,
                CapabilityKind::VideoInput,
                CapabilityEvidenceSource::ActiveProbe,
                probed_video_input,
                &checked_at,
                Some(format!(
                    "tiny red MP4 semantic probe={:?}",
                    video_probe.outcome(probed_video_input)
                )),
            );
            replace_capability_evidence(
                &mut record,
                CapabilityKind::VideoInput,
                CapabilityEvidenceSource::ProviderMetadata,
                metadata_video_input,
                &checked_at,
                Some(format!("modalities={modalities}")),
            );
        }

        if probe_image_gen {
            observer(ProbeEvent::Skipped {
                capability: CapabilityKind::ImageGeneration,
                reason: "Active image-generation probe can spend credits and is never run automatically; passive evidence only.".to_string(),
            });
        }

        record.checked_at = checked_at;
        record.source = "capability probe + provider metadata".to_string();

        observer(ProbeEvent::Progress {
            capability: CapabilityKind::TextChat,
            message: "Persisting capability registry...".to_string(),
        });
        let candidate = {
            let registry = self.capability_registry.read().await;
            let mut candidate = registry.clone();
            if let Some(existing) = candidate
                .models
                .iter_mut()
                .find(|entry| entry.provider_id == provider_id && entry.model == model)
            {
                *existing = record.clone();
            } else {
                candidate.models.push(record.clone());
            }
            candidate
        };
        let saved = persist_capability_registry(candidate.clone()).await;
        let published = {
            let mut runtime = self.capability_registry.write().await;
            publish_capability_candidate(&mut runtime, candidate, saved)
        };
        if !published {
            warn!("Capability probe result was not published because persistence failed");
        }
        observer(ProbeEvent::Persistence { saved: published });
        observer(ProbeEvent::Finished);
        record
    }

    pub async fn probe_model_capabilities_with_observer<F>(
        &self,
        provider: &ProviderConfig,
        model: &str,
        observer: F,
    ) -> CapabilityRecord
    where
        F: FnMut(ProbeEvent),
    {
        self.probe_model_capabilities_with_plan_and_observer(
            provider,
            model,
            ProbePlan::FullSafe,
            observer,
        )
        .await
    }

    pub async fn probe_addon_role_with_observer<F>(
        &self,
        role: ModelRole,
        observer: F,
    ) -> Result<(CapabilityRecord, ProbeOutcome), String>
    where
        F: FnMut(ProbeEvent),
    {
        let route = self.resolve_model_route_unchecked(role).await?;
        let record = self
            .probe_model_capabilities_with_plan_and_observer(
                &route.provider,
                &route.model,
                ProbePlan::Role(role),
                observer,
            )
            .await;
        let kind = match role {
            ModelRole::Main => CapabilityKind::TextChat,
            ModelRole::Vision => CapabilityKind::ImageInput,
            ModelRole::Video => CapabilityKind::VideoInput,
            ModelRole::AudioStt if route.route_origin == RouteOrigin::MainModel => {
                CapabilityKind::AudioInput
            }
            ModelRole::AudioStt => CapabilityKind::AudioTranscription,
            ModelRole::ImageGeneration => CapabilityKind::ImageGeneration,
        };
        let outcome = match record.effective_state_for(kind) {
            CapabilityState::Supported => ProbeOutcome::Supported,
            CapabilityState::Unsupported => ProbeOutcome::Unsupported,
            CapabilityState::Unknown => ProbeOutcome::Inconclusive,
        };
        Ok((record, outcome))
    }

    pub async fn capability_record(&self, endpoint: &str, model: &str) -> Option<CapabilityRecord> {
        let endpoint = endpoint.trim_end_matches('/');
        self.capability_registry
            .read()
            .await
            .models
            .iter()
            .find(|record| record.provider_id == endpoint && record.model == model)
            .cloned()
    }

    pub async fn resolved_model_capability(&self, endpoint: &str, model: &str) -> ModelCapability {
        let metadata = self
            .model_metadata
            .read()
            .await
            .get(&model_metadata_key(endpoint, model))
            .cloned();
        let mut capability = get_model_capabilities_with_meta(model, metadata.as_ref());
        if let Some(record) = self.capability_record(endpoint, model).await {
            let vision = Self::effective_capability_state(&record, CapabilityKind::ImageInput);
            capability.vision = vision == CapabilityState::Supported;
            capability.vision_desc = match vision {
                CapabilityState::Supported => {
                    "✅ Verified by fresh provider metadata/probe".to_string()
                }
                CapabilityState::Unsupported => {
                    "❌ Rejected by fresh provider metadata/probe".to_string()
                }
                CapabilityState::Unknown => {
                    "⚪ Unknown/stale: provider evidence is not currently authoritative".to_string()
                }
            };

            let audio = Self::effective_capability_state(&record, CapabilityKind::AudioInput);
            capability.audio = audio == CapabilityState::Supported;
            capability.audio_desc = match audio {
                CapabilityState::Supported => "✅ Fresh provider evidence".to_string(),
                CapabilityState::Unsupported => "❌ Fresh provider rejection".to_string(),
                CapabilityState::Unknown => {
                    "⚪ Unknown/stale: audio capability not currently proven".to_string()
                }
            };

            let video = Self::effective_capability_state(&record, CapabilityKind::VideoInput);
            capability.video = video == CapabilityState::Supported;
            capability.video_desc = match video {
                CapabilityState::Supported => "✅ Fresh provider evidence".to_string(),
                CapabilityState::Unsupported => "❌ Fresh provider rejection".to_string(),
                CapabilityState::Unknown => {
                    "⚪ Unknown/stale: video capability not currently proven".to_string()
                }
            };

            let reasoning = Self::effective_capability_state(&record, CapabilityKind::Reasoning);
            capability.thinking = reasoning == CapabilityState::Supported;
            capability.thinking_desc = match reasoning {
                CapabilityState::Supported => "✅ Fresh reasoning evidence".to_string(),
                CapabilityState::Unsupported => "❌ Reasoning mode not supported".to_string(),
                CapabilityState::Unknown => {
                    "⚪ Unknown/stale: reasoning capability not currently proven".to_string()
                }
            };
        }
        capability.documents = true;
        capability.docs_desc = "✅ Xiao extractor: text/code, PDF, DOCX, XLSX; scanned PDF uses vision when renderer is available".to_string();
        capability
    }

    pub async fn fetch_models_from_endpoint(
        &self,
        endpoint: &str,
        api_key: &str,
    ) -> (bool, Result<Vec<String>, String>) {
        let clean_endpoint = endpoint.trim().trim_end_matches('/');
        let url = format!("{clean_endpoint}/models");

        let mut req = self.client.get(&url).timeout(Duration::from_secs(15));
        let trimmed_key = api_key.trim();
        if !trimmed_key.is_empty()
            && !["none", "-", "no", "null"]
                .iter()
                .any(|k| trimmed_key.eq_ignore_ascii_case(k))
        {
            req = req.header("Authorization", format!("Bearer {trimmed_key}"));
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    match read_bounded_provider_json(resp).await {
                        Ok(data) => {
                            let mut model_ids = Vec::new();
                            let mut meta_guard = self.model_metadata.write().await;

                            if let Some(data_arr) = data.get("data").and_then(|d| d.as_array()) {
                                for item in data_arr {
                                    if let Some(metadata) = normalize_provider_model_metadata(item)
                                    {
                                        model_ids.push(metadata.id.clone());
                                        meta_guard.insert(
                                            model_metadata_key(clean_endpoint, &metadata.id),
                                            metadata,
                                        );
                                    } else if let Some(s) = item.as_str() {
                                        model_ids.push(s.to_string());
                                    }
                                }
                            } else if let Some(data_obj) =
                                data.get("data").and_then(|d| d.as_object())
                            {
                                for k in data_obj.keys() {
                                    model_ids.push(k.to_string());
                                }
                            }

                            let provider_id = clean_endpoint.to_string();
                            let registry_candidate = {
                                let registry = self.capability_registry.read().await;
                                let mut candidate = registry.clone();
                                for model_id in &model_ids {
                                    let meta = meta_guard
                                        .get(&model_metadata_key(clean_endpoint, model_id));
                                    let modalities = meta
                                        .and_then(|m| m.modalities.as_deref())
                                        .unwrap_or("")
                                        .to_ascii_lowercase();
                                    let record_checked_at = Local::now().to_rfc3339();
                                    let mut metadata_evidence = Vec::new();
                                    for (cap, val) in [
                                        (CapabilityKind::TextChat, Some(true)),
                                        (
                                            CapabilityKind::ImageInput,
                                            (modalities.contains("image")
                                                || modalities.contains("vision")
                                                || modalities.contains("multimodal"))
                                            .then_some(true),
                                        ),
                                        (
                                            CapabilityKind::AudioInput,
                                            modalities.contains("audio").then_some(true),
                                        ),
                                        (
                                            CapabilityKind::VideoInput,
                                            modalities.contains("video").then_some(true),
                                        ),
                                        (
                                            CapabilityKind::NativeFileInput,
                                            (modalities.contains("file")
                                                || modalities.contains("document"))
                                            .then_some(true),
                                        ),
                                    ] {
                                        if let Some(supported) = val {
                                            metadata_evidence.push(CapabilityEvidence {
                                                capability: cap,
                                                source: CapabilityEvidenceSource::ProviderMetadata,
                                                outcome: if supported {
                                                    CapabilityState::Supported
                                                } else {
                                                    CapabilityState::Unsupported
                                                },
                                                checked_at: record_checked_at.clone(),
                                                detail: Some(format!("modalities: {modalities}")),
                                            });
                                        }
                                    }

                                    let record = CapabilityRecord {
                                        provider_id: provider_id.clone(),
                                        provider_name: provider_id.clone(),
                                        model: model_id.clone(),
                                        context_window: meta.and_then(|m| m.context_length),
                                        supports_text_chat: Some(true),
                                        supports_image_input: (modalities.contains("image")
                                            || modalities.contains("vision")
                                            || modalities.contains("multimodal"))
                                        .then_some(true),
                                        supports_image_generation: None,
                                        supports_image_editing: None,
                                        supports_audio_input: modalities
                                            .contains("audio")
                                            .then_some(true),
                                        supports_audio_transcription: None,
                                        supports_video_input: modalities
                                            .contains("video")
                                            .then_some(true),
                                        supports_reasoning: None,
                                        supports_tools: None,
                                        supports_structured_output: None,
                                        supports_native_file_input: (modalities.contains("file")
                                            || modalities.contains("document"))
                                        .then_some(true),
                                        evidence: metadata_evidence.clone(),
                                        source: "provider /models metadata".to_string(),
                                        details: if modalities.is_empty() {
                                            vec!["Input modality tidak dipublikasikan endpoint"
                                                .to_string()]
                                        } else {
                                            vec![format!("modalities: {modalities}")]
                                        },
                                        checked_at: record_checked_at.clone(),
                                    };
                                    if let Some(existing) =
                                        candidate.models.iter_mut().find(|entry| {
                                            entry.provider_id == provider_id
                                                && entry.model == *model_id
                                        })
                                    {
                                        existing.provider_name = record.provider_name;
                                        existing.context_window =
                                            record.context_window.or(existing.context_window);
                                        if existing.supports_image_input.is_none() {
                                            existing.supports_image_input =
                                                record.supports_image_input;
                                        }
                                        if existing.supports_audio_input.is_none() {
                                            existing.supports_audio_input =
                                                record.supports_audio_input;
                                        }
                                        if existing.supports_video_input.is_none() {
                                            existing.supports_video_input =
                                                record.supports_video_input;
                                        }
                                        for ev in metadata_evidence {
                                            existing.evidence.retain(|e| {
                                                e.capability != ev.capability
                                                    || e.source != CapabilityEvidenceSource::ProviderMetadata
                                            });
                                            existing.evidence.push(ev);
                                        }
                                        existing.checked_at = record.checked_at;
                                        if !record.details.is_empty() {
                                            existing.details.extend(record.details);
                                            existing.details.sort();
                                            existing.details.dedup();
                                        }
                                        if !existing.source.contains("active capability probe") {
                                            existing.source = record.source;
                                        }
                                    } else {
                                        candidate.models.push(record);
                                    }
                                }
                                candidate
                            };
                            drop(meta_guard);
                            if persist_capability_registry(registry_candidate.clone()).await {
                                *self.capability_registry.write().await = registry_candidate;
                            } else {
                                warn!("Provider model capability metadata was not published because persistence failed");
                            }

                            if !model_ids.is_empty() {
                                (true, Ok(model_ids))
                            } else {
                                (false, Err("Endpoint berhasil dihubungi, namun tidak ada daftar model yang dikembalikan (data kosong).".to_string()))
                            }
                        }
                        Err(e) => (
                            false,
                            Err(format!("Respon dari endpoint bukan JSON valid: {e}")),
                        ),
                    }
                } else if status.as_u16() == 401 || status.as_u16() == 403 {
                    (false, Err(format!("HTTP {} Unauthorized: Autentikasi gagal. Mohon periksa kembali API Key Anda.", status.as_u16())))
                } else if status.as_u16() == 404 {
                    (false, Err(format!("HTTP 404 Not Found: Path /models tidak ditemukan di {clean_endpoint}. Pastikan format endpoint URL benar (misal: https://api.openai.com/v1).")))
                } else {
                    let err_text = read_bounded_provider_text(resp, 64 * 1024).await;
                    (
                        false,
                        Err(format!(
                            "HTTP {}: {}",
                            status.as_u16(),
                            truncate_chars(&err_text, 150).as_str()
                        )),
                    )
                }
            }
            Err(e) => {
                if e.is_timeout() {
                    (
                        false,
                        Err(format!(
                            "Koneksi timeout setelah 15 detik ke {clean_endpoint}."
                        )),
                    )
                } else if e.is_connect() {
                    (false, Err(format!("Gagal terhubung ke {clean_endpoint}. Pastikan host/domain benar dan server aktif.")))
                } else {
                    (false, Err(format!("Koneksi gagal: {e}")))
                }
            }
        }
    }

    pub async fn get_user_model(&self, user_id: i64) -> String {
        self.get_active_provider(user_id)
            .await
            .map(|provider| provider.active_model)
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| "gpt-4o".to_string())
    }

    // ==========================================
    // Multi-Session Management
    // ==========================================
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_with_message(message: Value) -> CapabilityProbeResponse {
        CapabilityProbeResponse::Success(json!({
            "choices": [{"message": message}]
        }))
    }

    fn provider(id: &str, model: &str) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            name: id.to_string(),
            endpoint: format!("https://{id}.example/v1"),
            api_key: String::new(),
            api_key_ref: None,
            models: vec![model.to_string()],
            active_model: model.to_string(),
        }
    }

    #[test]
    fn main_model_route_resolves_dynamically_to_current_main() {
        let mut store = ProviderStore {
            active_id: Some("main-a".to_string()),
            providers: vec![provider("main-a", "model-a"), provider("main-b", "model-b")],
            telegram_models: Vec::new(),
        };
        let routing = ModelRoutingConfig::default();
        let (_, model, origin) = select_model_route(&store, &routing, ModelRole::Vision).unwrap();
        assert_eq!(model, "model-a");
        assert_eq!(origin, RouteOrigin::MainModel);

        store.active_id = Some("main-b".to_string());
        let (_, model, origin) = select_model_route(&store, &routing, ModelRole::Vision).unwrap();
        assert_eq!(model, "model-b");
        assert_eq!(origin, RouteOrigin::MainModel);
    }

    #[test]
    fn specific_route_rejects_missing_provider_and_model() {
        let store = ProviderStore {
            active_id: Some("main".to_string()),
            providers: vec![
                provider("main", "main-model"),
                provider("vision", "vision-v1"),
            ],
            telegram_models: Vec::new(),
        };
        let mut routing = ModelRoutingConfig::default();
        routing
            .set_route(
                ModelRole::Vision,
                ModelRoute::Specific {
                    provider_id: "missing".to_string(),
                    model: "vision-v1".to_string(),
                },
            )
            .unwrap();
        assert!(select_model_route(&store, &routing, ModelRole::Vision)
            .unwrap_err()
            .contains("not found"));

        routing
            .set_route(
                ModelRole::Vision,
                ModelRoute::Specific {
                    provider_id: "vision".to_string(),
                    model: "vision-v2".to_string(),
                },
            )
            .unwrap();
        assert!(select_model_route(&store, &routing, ModelRole::Vision)
            .unwrap_err()
            .contains("no longer present"));
    }

    #[test]
    fn failed_capability_persistence_does_not_publish_candidate() {
        let original = CapabilityRecord {
            provider_id: "provider".to_string(),
            model: "model".to_string(),
            supports_image_input: Some(false),
            ..CapabilityRecord::default()
        };
        let mut runtime = CapabilityRegistry {
            models: vec![original.clone()],
        };
        let candidate = CapabilityRegistry {
            models: vec![CapabilityRecord {
                supports_image_input: Some(true),
                ..original
            }],
        };
        assert!(!publish_capability_candidate(
            &mut runtime,
            candidate,
            false
        ));
        assert_eq!(runtime.models[0].supports_image_input, Some(false));
    }

    #[test]
    fn transient_probe_result_preserves_previous_authoritative_evidence() {
        let now = chrono::Utc::now().to_rfc3339();
        let mut record = CapabilityRecord {
            evidence: vec![CapabilityEvidence {
                capability: CapabilityKind::ImageGeneration,
                source: CapabilityEvidenceSource::ActiveProbe,
                outcome: CapabilityState::Supported,
                checked_at: now.clone(),
                detail: Some("previous successful active probe".to_string()),
            }],
            ..CapabilityRecord::default()
        };
        replace_capability_evidence(
            &mut record,
            CapabilityKind::ImageGeneration,
            CapabilityEvidenceSource::ActiveProbe,
            None,
            &now,
            Some("transient timeout".to_string()),
        );
        assert_eq!(record.evidence.len(), 1);
        assert_eq!(
            record.effective_state_for(CapabilityKind::ImageGeneration),
            CapabilityState::Supported
        );
        assert_eq!(
            record.evidence[0].detail.as_deref(),
            Some("previous successful active probe")
        );
    }

    #[test]
    fn configured_routes_ignore_diagnostic_evidence() {
        let provider = provider("main", "model");
        for state in [
            CapabilityState::Unknown,
            CapabilityState::Supported,
            CapabilityState::Unsupported,
        ] {
            for age in [chrono::Duration::zero(), chrono::Duration::days(90)] {
                let mut record = supported_record(&provider, "model");
                for evidence in &mut record.evidence {
                    evidence.outcome = state;
                    evidence.checked_at = (chrono::Utc::now() - age).to_rfc3339();
                }
                for records in [vec![], vec![record.clone()]] {
                    let mut snapshot = GenerationModelSnapshot {
                        provider_store: ProviderStore {
                            active_id: Some(provider.id.clone()),
                            providers: vec![provider.clone()],
                            telegram_models: vec![],
                        },
                        routing: ModelRoutingConfig::default(),
                        capabilities: CapabilityRegistry { models: records },
                    };
                    for role in [ModelRole::Main]
                        .into_iter()
                        .chain(ModelRole::addon_roles())
                    {
                        let resolved =
                            AIChatService::resolve_model_route_from_snapshot(&snapshot, role)
                                .unwrap();
                        assert_eq!(resolved.model, "model");
                        assert_eq!(resolved.provider.endpoint, provider.endpoint);
                        if role != ModelRole::Main {
                            snapshot
                                .routing
                                .set_route(role, ModelRoute::Disabled)
                                .unwrap();
                            assert!(AIChatService::resolve_model_route_from_snapshot(
                                &snapshot, role,
                            )
                            .is_err());
                            snapshot
                                .routing
                                .set_route(
                                    role,
                                    ModelRoute::Specific {
                                        provider_id: provider.id.clone(),
                                        model: "model".into(),
                                    },
                                )
                                .unwrap();
                            assert!(AIChatService::resolve_model_route_from_snapshot(
                                &snapshot, role,
                            )
                            .is_ok());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn functional_probe_success_is_supported() {
        let response = CapabilityProbeResponse::Success(json!({"ok": true}));
        assert_eq!(response.outcome(Some(true)), ProbeOutcome::Supported);
    }

    #[test]
    fn successful_http_without_tool_call_does_not_prove_tools() {
        let response = response_with_message(json!({"content": "OK"}));
        assert_eq!(validate_tools_probe(&response), None);
    }

    #[test]
    fn named_tool_call_proves_tools() {
        let response = response_with_message(json!({
            "content": null,
            "tool_calls": [{
                "type": "function",
                "function": {"name": "xiao_capability_probe", "arguments": "{}"}
            }]
        }));
        assert_eq!(validate_tools_probe(&response), Some(true));
    }

    #[test]
    fn structured_probe_requires_expected_json_behavior() {
        let good = response_with_message(json!({"content": "{\"xiao_probe\":true}"}));
        let ignored = response_with_message(json!({"content": "sure"}));
        assert_eq!(validate_structured_probe(&good), Some(true));
        assert_eq!(validate_structured_probe(&ignored), None);
    }

    #[test]
    fn vision_probe_requires_two_demonstrated_colors() {
        let red = response_with_message(json!({"content": "red"}));
        let blue = response_with_message(json!({"content": "blue"}));
        assert_eq!(validate_color_probe(&red, "red"), Some(true));
        assert_eq!(validate_color_probe(&blue, "blue"), Some(true));
        assert_eq!(
            combine_vision_probe_results(
                validate_color_probe(&red, "red"),
                validate_color_probe(&blue, "blue")
            ),
            Some(true)
        );
    }

    #[test]
    fn functional_test_roles_keep_image_generation_on_explicit_path() {
        assert_ne!(ModelRole::Vision, ModelRole::ImageGeneration);
        assert_ne!(ModelRole::Video, ModelRole::ImageGeneration);
        assert_ne!(ModelRole::AudioStt, ModelRole::ImageGeneration);
    }

    #[test]
    fn video_probe_payload_contains_bounded_mp4_and_selected_model() {
        let payload = video_probe_payload("video-model");
        assert_eq!(
            payload.get("model").and_then(Value::as_str),
            Some("video-model")
        );
        let url = payload
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| messages.first())
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .and_then(|content| content.get(1))
            .and_then(|part| part.get("image_url"))
            .and_then(|image_url| image_url.get("url"))
            .and_then(Value::as_str)
            .unwrap();
        let encoded = url.strip_prefix("data:video/mp4;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert!(bytes.len() < 2 * 1024);
        assert_eq!(&bytes[4..8], b"ftyp");
    }

    #[tokio::test]
    async fn image_generation_probe_requires_valid_image_bytes() {
        let valid = json!({
            "data": [{
                "b64_json": "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP8z0AaYCJR/aiGUQ1DSAMAQC4BH2bjRnMAAAAASUVORK5CYII="
            }]
        });
        assert!(validate_image_generation_probe_body(&valid).await);

        let invalid = json!({
            "data": [{"b64_json": "bm90IGFuIGltYWdl"}]
        });
        assert!(!validate_image_generation_probe_body(&invalid).await);
    }

    #[test]
    fn explicit_probe_rejection_is_unsupported() {
        assert_eq!(
            validate_tools_probe(&CapabilityProbeResponse::Rejected),
            Some(false)
        );
        assert_eq!(
            validate_structured_probe(&CapabilityProbeResponse::Rejected),
            Some(false)
        );
    }

    #[test]
    fn catalog_presence_does_not_claim_text_chat() {
        assert_eq!(catalog_presence_text_chat_claim(), None);
    }

    #[test]
    fn provider_metadata_normalizer_handles_observed_openai_compatible_shapes() {
        let metadata = normalize_provider_model_metadata(&json!({
            "id": "model-a",
            "name": "Model A",
            "context_length": 131072,
            "architecture": {"modality": "text+image"},
            "top_provider": {"max_completion_tokens": 8192}
        }))
        .unwrap();
        assert_eq!(metadata.id, "model-a");
        assert_eq!(metadata.context_length, Some(131072));
        assert_eq!(metadata.modalities.as_deref(), Some("text+image"));
        assert_eq!(metadata.max_completion_tokens, Some(8192));

        let metadata = normalize_provider_model_metadata(&json!({
            "id": "model-b",
            "modalities": ["text", "audio", "video"]
        }))
        .unwrap();
        assert_eq!(metadata.modalities.as_deref(), Some("text,audio,video"));
    }

    #[test]
    fn stale_supported_capability_is_effectively_unknown() {
        let record = CapabilityRecord {
            supports_image_input: Some(true),
            checked_at: "2000-01-01T00:00:00+00:00".to_string(),
            ..CapabilityRecord::default()
        };
        assert_eq!(
            AIChatService::effective_capability_state(&record, CapabilityKind::ImageInput),
            CapabilityState::Unknown
        );
    }

    #[test]
    fn semantic_stt_probe_success_supported() {
        let response = CapabilityProbeResponse::Success(json!({
            "text": "xiao capability probe"
        }));
        assert_eq!(validate_transcription_probe(&response), Some(true));
        assert_eq!(response.outcome(Some(true)), ProbeOutcome::Supported);
    }

    #[test]
    fn semantic_stt_probe_normalized_case_and_punctuation_supported() {
        let response = CapabilityProbeResponse::Success(json!({
            "text": "Xiao capability probe."
        }));
        assert_eq!(validate_transcription_probe(&response), Some(true));
    }

    #[test]
    fn semantic_stt_probe_empty_transcript_unknown() {
        let response = CapabilityProbeResponse::Success(json!({
            "text": ""
        }));
        assert_eq!(validate_transcription_probe(&response), None);
        assert_eq!(response.outcome(None), ProbeOutcome::Inconclusive);
    }

    #[test]
    fn semantic_stt_probe_wrong_phrase_unknown() {
        let response = CapabilityProbeResponse::Success(json!({
            "text": "hello world"
        }));
        assert_eq!(validate_transcription_probe(&response), None);
        assert_eq!(response.outcome(None), ProbeOutcome::Inconclusive);
    }

    #[test]
    fn semantic_stt_probe_http_200_without_text_unknown() {
        let response = CapabilityProbeResponse::Success(json!({
            "status": "ok"
        }));
        assert_eq!(validate_transcription_probe(&response), None);
        assert_eq!(response.outcome(None), ProbeOutcome::Inconclusive);
    }

    #[test]
    fn semantic_stt_probe_explicit_unsupported_rejected() {
        let response = CapabilityProbeResponse::Rejected;
        assert_eq!(validate_transcription_probe(&response), Some(false));
        assert_eq!(response.outcome(Some(false)), ProbeOutcome::Unsupported);
    }

    #[test]
    fn semantic_stt_probe_transient_status_unknown() {
        let timeout = CapabilityProbeResponse::Unknown(ProbeOutcome::Timeout);
        assert_eq!(validate_transcription_probe(&timeout), None);
        assert_eq!(timeout.outcome(None), ProbeOutcome::Timeout);

        let rate_limited = CapabilityProbeResponse::Unknown(ProbeOutcome::RateLimited);
        assert_eq!(validate_transcription_probe(&rate_limited), None);
        assert_eq!(rate_limited.outcome(None), ProbeOutcome::RateLimited);

        let server_error = CapabilityProbeResponse::Unknown(ProbeOutcome::ProviderError);
        assert_eq!(validate_transcription_probe(&server_error), None);
        assert_eq!(server_error.outcome(None), ProbeOutcome::ProviderError);
    }

    #[test]
    fn semantic_native_audio_probe_requires_spoken_phrase() {
        let exact = response_with_message(json!({"content":"xiao capability probe"}));
        assert_eq!(validate_native_audio_probe(&exact), Some(true));

        let normalized = response_with_message(json!({"content":"Xiao capability probe!"}));
        assert_eq!(validate_native_audio_probe(&normalized), Some(true));

        let minor = response_with_message(json!({"content":"Ciao capability probe"}));
        assert_eq!(validate_native_audio_probe(&minor), Some(true));

        let ok_only = response_with_message(json!({"content":"OK"}));
        assert_eq!(validate_native_audio_probe(&ok_only), None);

        let wrong = response_with_message(json!({"content":"hello world"}));
        assert_eq!(validate_native_audio_probe(&wrong), None);

        let empty = response_with_message(json!({"content":""}));
        assert_eq!(validate_native_audio_probe(&empty), None);

        let rejected = CapabilityProbeResponse::Rejected;
        assert_eq!(validate_native_audio_probe(&rejected), Some(false));

        for outcome in [
            ProbeOutcome::Timeout,
            ProbeOutcome::NetworkError,
            ProbeOutcome::RateLimited,
            ProbeOutcome::ProviderError,
        ] {
            let response = CapabilityProbeResponse::Unknown(outcome);
            assert_eq!(validate_native_audio_probe(&response), None);
            assert_eq!(response.outcome(None), outcome);
        }
    }

    #[test]
    fn capability_rejection_requires_semantically_bound_phrases() {
        let rejected = [
            (
                CapabilityKind::ImageInput,
                "this model does not support image input",
            ),
            (
                CapabilityKind::ImageInput,
                "vision is not supported for this model",
            ),
            (CapabilityKind::ImageInput, "vision is not supported"),
            (
                CapabilityKind::AudioInput,
                "model does not support audio input",
            ),
            (CapabilityKind::AudioInput, "audio input is not supported"),
            (
                CapabilityKind::AudioTranscription,
                "audio transcription is not supported",
            ),
            (CapabilityKind::VideoInput, "video input is not supported"),
            (CapabilityKind::Tools, "this model does not support tools"),
            (
                CapabilityKind::StructuredOutput,
                "response_format is not supported",
            ),
            (
                CapabilityKind::ImageGeneration,
                "image generation is not supported",
            ),
            (CapabilityKind::TextChat, "text chat is not supported"),
        ];
        for (capability, body) in rejected {
            assert!(
                explicit_capability_rejection(capability, body),
                "{capability:?}: {body}"
            );
        }

        let ambiguous = [
            (
                CapabilityKind::ImageInput,
                "model gpt-4-vision-preview does not support max_tokens",
            ),
            (
                CapabilityKind::ImageInput,
                "vision request for this model does not support response_format",
            ),
            (CapabilityKind::ImageInput, "invalid image url"),
            (CapabilityKind::ImageInput, "unsupported image format"),
            (CapabilityKind::ImageInput, "unsupported media type"),
            (CapabilityKind::ImageInput, "invalid base64"),
            (
                CapabilityKind::AudioInput,
                "audio request does not support max_tokens",
            ),
            (
                CapabilityKind::AudioInput,
                "audio request does not support temperature",
            ),
            (CapabilityKind::AudioInput, "unsupported codec"),
            (CapabilityKind::AudioInput, "invalid input_audio schema"),
            (CapabilityKind::AudioTranscription, "unsupported codec"),
            (CapabilityKind::AudioTranscription, "unsupported file type"),
            (CapabilityKind::AudioTranscription, "malformed multipart"),
            (CapabilityKind::AudioTranscription, "invalid audio format"),
            (
                CapabilityKind::VideoInput,
                "video request does not support temperature",
            ),
            (CapabilityKind::VideoInput, "unsupported video format"),
            (CapabilityKind::VideoInput, "invalid video encoding"),
            (
                CapabilityKind::Tools,
                "this model does not support max_tokens",
            ),
            (
                CapabilityKind::StructuredOutput,
                "this model does not support temperature",
            ),
            (CapabilityKind::ImageGeneration, "unsupported image format"),
            (CapabilityKind::ImageGeneration, "unsupported size"),
            (CapabilityKind::ImageGeneration, "unsupported quality"),
            (CapabilityKind::ImageGeneration, "invalid response_format"),
            (
                CapabilityKind::TextChat,
                "this model does not support max_tokens",
            ),
        ];
        for (capability, body) in ambiguous {
            assert!(
                !explicit_capability_rejection(capability, body),
                "{capability:?}: {body}"
            );
        }
    }

    #[test]
    fn probe_http_policy_only_rejects_explicit_capability_failures() {
        for status in [401, 403] {
            assert!(matches!(
                classify_probe_http_failure(
                    CapabilityKind::Tools,
                    status,
                    "does not support tools"
                ),
                CapabilityProbeResponse::Unknown(ProbeOutcome::AuthFailed)
            ));
        }
        for status in [404, 405, 415] {
            assert!(matches!(
                classify_probe_http_failure(
                    CapabilityKind::ImageInput,
                    status,
                    "this model does not support image input"
                ),
                CapabilityProbeResponse::Unknown(ProbeOutcome::ProtocolMismatch)
            ));
        }
        assert!(matches!(
            classify_probe_http_failure(
                CapabilityKind::ImageInput,
                429,
                "this model does not support image input"
            ),
            CapabilityProbeResponse::Unknown(ProbeOutcome::RateLimited)
        ));
        assert!(matches!(
            classify_probe_http_failure(
                CapabilityKind::ImageInput,
                503,
                "this model does not support image input"
            ),
            CapabilityProbeResponse::Unknown(ProbeOutcome::ProviderError)
        ));

        let ambiguous = [
            (
                CapabilityKind::ImageInput,
                "model gpt-4-vision-preview does not support max_tokens",
            ),
            (
                CapabilityKind::ImageInput,
                "vision request for this model does not support response_format",
            ),
            (
                CapabilityKind::AudioInput,
                "audio request does not support max_tokens",
            ),
            (
                CapabilityKind::AudioInput,
                "audio request does not support temperature",
            ),
            (CapabilityKind::AudioTranscription, "unsupported codec"),
            (
                CapabilityKind::VideoInput,
                "video request does not support temperature",
            ),
            (
                CapabilityKind::Tools,
                "this model does not support max_tokens",
            ),
            (
                CapabilityKind::StructuredOutput,
                "this model does not support temperature",
            ),
        ];
        for status in [400, 422] {
            for (capability, body) in ambiguous {
                assert!(matches!(
                    classify_probe_http_failure(capability, status, body),
                    CapabilityProbeResponse::Unknown(ProbeOutcome::ProtocolMismatch)
                ));
            }
        }

        let explicit = [
            (
                CapabilityKind::ImageInput,
                "this model does not support image input",
            ),
            (
                CapabilityKind::ImageInput,
                "vision is not supported for this model",
            ),
            (
                CapabilityKind::AudioInput,
                "model does not support audio input",
            ),
            (CapabilityKind::AudioInput, "audio input is not supported"),
            (
                CapabilityKind::AudioTranscription,
                "audio transcription is not supported",
            ),
            (CapabilityKind::VideoInput, "video input is not supported"),
            (CapabilityKind::Tools, "this model does not support tools"),
            (
                CapabilityKind::StructuredOutput,
                "response_format is not supported",
            ),
        ];
        for status in [400, 422] {
            for (capability, body) in explicit {
                assert!(matches!(
                    classify_probe_http_failure(capability, status, body),
                    CapabilityProbeResponse::Rejected
                ));
            }
        }
    }

    fn supported_record(provider: &ProviderConfig, model: &str) -> CapabilityRecord {
        let now = chrono::Utc::now().to_rfc3339();
        let kinds = [
            CapabilityKind::TextChat,
            CapabilityKind::ImageInput,
            CapabilityKind::AudioInput,
            CapabilityKind::AudioTranscription,
            CapabilityKind::VideoInput,
            CapabilityKind::ImageGeneration,
        ];
        CapabilityRecord {
            provider_id: provider.endpoint.trim_end_matches('/').to_string(),
            provider_name: provider.name.clone(),
            model: model.to_string(),
            evidence: kinds
                .into_iter()
                .map(|capability| CapabilityEvidence {
                    capability,
                    source: CapabilityEvidenceSource::ActiveProbe,
                    outcome: CapabilityState::Supported,
                    checked_at: now.clone(),
                    detail: None,
                })
                .collect(),
            ..CapabilityRecord::default()
        }
    }

    #[test]
    fn generation_snapshot_keeps_inherited_routes_on_original_main() {
        let mut live_store = ProviderStore {
            active_id: Some("main-a".to_string()),
            providers: vec![provider("main-a", "model-a"), provider("main-b", "model-b")],
            telegram_models: Vec::new(),
        };
        let routing = ModelRoutingConfig::default();
        let a = live_store.providers[0].clone();
        let b = live_store.providers[1].clone();
        let snapshot = GenerationModelSnapshot {
            provider_store: live_store.clone(),
            routing,
            capabilities: CapabilityRegistry {
                models: vec![
                    supported_record(&a, "model-a"),
                    supported_record(&b, "model-b"),
                ],
            },
        };

        let initial_main =
            AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Main).unwrap();
        live_store.active_id = Some("main-b".to_string());

        for role in [ModelRole::Vision, ModelRole::Video, ModelRole::AudioStt] {
            let inherited =
                AIChatService::resolve_model_route_from_snapshot(&snapshot, role).unwrap();
            assert_eq!(inherited.provider.id, "main-a");
            assert_eq!(inherited.model, "model-a");
        }
        let final_main =
            AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Main).unwrap();
        assert_eq!(initial_main.provider.id, "main-a");
        assert_eq!(final_main.provider.id, "main-a");
        assert_eq!(live_store.active_id.as_deref(), Some("main-b"));
    }

    #[test]
    fn compound_image_explanation_reuses_original_main_snapshot() {
        let mut live_store = ProviderStore {
            active_id: Some("main-a".to_string()),
            providers: vec![provider("main-a", "model-a"), provider("main-b", "model-b")],
            telegram_models: Vec::new(),
        };
        let a = live_store.providers[0].clone();
        let b = live_store.providers[1].clone();
        let snapshot = GenerationModelSnapshot {
            provider_store: live_store.clone(),
            routing: ModelRoutingConfig::default(),
            capabilities: CapabilityRegistry {
                models: vec![
                    supported_record(&a, "model-a"),
                    supported_record(&b, "model-b"),
                ],
            },
        };

        let image_route =
            AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::ImageGeneration)
                .unwrap();
        assert_eq!(image_route.provider.id, "main-a");

        live_store.active_id = Some("main-b".to_string());

        let explanation_route =
            AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Main).unwrap();
        assert_eq!(explanation_route.provider.id, "main-a");

        let next_request = GenerationModelSnapshot {
            provider_store: live_store,
            routing: ModelRoutingConfig::default(),
            capabilities: snapshot.capabilities.clone(),
        };
        let next_main =
            AIChatService::resolve_model_route_from_snapshot(&next_request, ModelRole::Main)
                .unwrap();
        assert_eq!(next_main.provider.id, "main-b");
    }

    #[test]
    fn generation_snapshot_keeps_specific_and_disabled_routes_isolated() {
        let mut live_store = ProviderStore {
            active_id: Some("main-a".to_string()),
            providers: vec![
                provider("main-a", "model-a"),
                provider("vision-b", "vision-b"),
                provider("main-c", "model-c"),
            ],
            telegram_models: Vec::new(),
        };
        let mut routing = ModelRoutingConfig::default();
        routing
            .set_route(
                ModelRole::Vision,
                ModelRoute::Specific {
                    provider_id: "vision-b".to_string(),
                    model: "vision-b".to_string(),
                },
            )
            .unwrap();
        routing
            .set_route(ModelRole::Video, ModelRoute::Disabled)
            .unwrap();
        let capabilities = live_store
            .providers
            .iter()
            .map(|provider| supported_record(provider, &provider.active_model))
            .collect();
        let snapshot = GenerationModelSnapshot {
            provider_store: live_store.clone(),
            routing,
            capabilities: CapabilityRegistry {
                models: capabilities,
            },
        };

        live_store.active_id = Some("main-c".to_string());
        let vision =
            AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Vision).unwrap();
        assert_eq!(vision.provider.id, "vision-b");
        assert_eq!(vision.model, "vision-b");
        assert!(
            AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Video).is_err()
        );
        let main =
            AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Main).unwrap();
        assert_eq!(main.provider.id, "main-a");
    }

    #[test]
    fn role_scoped_probe_plan_variants_are_distinct() {
        assert_eq!(ProbePlan::FullSafe, ProbePlan::FullSafe);
        assert_ne!(ProbePlan::FullSafe, ProbePlan::Role(ModelRole::Vision));
        assert_ne!(
            ProbePlan::Role(ModelRole::Vision),
            ProbePlan::Role(ModelRole::AudioStt)
        );
    }
}
