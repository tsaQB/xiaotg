use futures_util::StreamExt;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};
use std::ops::Deref;
use std::time::Duration;
use tracing::{info, warn};

use super::client_raw as raw;
use super::models::{
    ApiResponse, BotCommand, EphemeralMessageParameters, FileInfo, InputMedia, InputRichMessage,
    ReplyParameters, RichBlock, Update, User,
};
use super::transport_policy::{
    fallback_allowed_error, fallback_allowed_response, retry_delay_from_error,
    retry_delay_from_response, MAX_TELEGRAM_ATTEMPTS,
};

pub use raw::TelegramDeliveryContext;

const MAX_TELEGRAM_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;
const MAX_TELEGRAM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

tokio::task_local! {
    static REPLACE_CALLBACK_QUERY_MESSAGE: bool;
}

#[derive(Clone)]
pub struct TelegramBotClient {
    inner: raw::TelegramBotClient,
    token: String,
    base_url: String,
    client: Client,
}

impl Deref for TelegramBotClient {
    type Target = raw::TelegramBotClient;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[allow(dead_code)]
impl TelegramBotClient {
    pub fn new(token: impl Into<String>) -> Self {
        let token = token.into().trim().to_string();
        let inner = raw::TelegramBotClient::new(token.clone());
        let base_url = format!("https://api.telegram.org/bot{token}");
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            inner,
            token,
            base_url,
            client,
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub async fn with_delivery_context<F, T>(context: TelegramDeliveryContext, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        raw::TelegramBotClient::with_delivery_context(context, future).await
    }

    pub async fn with_replace_callback_query_message<F, T>(replace: bool, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        REPLACE_CALLBACK_QUERY_MESSAGE.scope(replace, future).await
    }

    pub fn current_delivery_context() -> TelegramDeliveryContext {
        raw::TelegramBotClient::current_delivery_context()
    }

    fn replace_callback_query_message() -> Option<bool> {
        REPLACE_CALLBACK_QUERY_MESSAGE.try_with(|value| *value).ok()
    }

    fn telegram_api_error(method: &str, response: &Value) -> String {
        let code = response
            .get("error_code")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let description = response
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("Telegram API request failed");
        let retry_after = response
            .pointer("/parameters/retry_after")
            .and_then(Value::as_i64)
            .map(|seconds| format!(" retry_after={seconds}s"))
            .unwrap_or_default();
        format!("Telegram API error [{method}] code={code}: {description}{retry_after}")
    }

    fn apply_delivery_context(payload: &mut Value, include_ephemeral: bool) {
        let context = Self::current_delivery_context();
        if payload.get("message_thread_id").is_none() {
            if let Some(thread_id) = context.message_thread_id {
                payload["message_thread_id"] = json!(thread_id);
            }
        }
        if include_ephemeral
            && payload.get("ephemeral_message_parameters").is_none()
            && context.receiver_user_id.is_some()
        {
            payload["ephemeral_message_parameters"] =
                serde_json::to_value(EphemeralMessageParameters {
                    receiver_user_id: context.receiver_user_id.unwrap_or_default(),
                    callback_query_id: context.callback_query_id.clone(),
                    replace_callback_query_message: Self::replace_callback_query_message(),
                })
                .unwrap_or(json!({}));
        }
        if include_ephemeral
            && payload.get("reply_parameters").is_none()
            && context.source_ephemeral_message_id.is_some()
        {
            payload["reply_parameters"] = serde_json::to_value(ReplyParameters::ephemeral(
                context.source_ephemeral_message_id.unwrap_or_default(),
            ))
            .unwrap_or(json!({}));
        }
    }

    fn apply_form_delivery_context(
        &self,
        mut form: Form,
        include_ephemeral: bool,
        receiver_override: Option<i64>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Form, String> {
        let delivery = Self::current_delivery_context();
        if let Some(thread_id) = delivery.message_thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        if include_ephemeral {
            if let Some(receiver_user_id) = receiver_override.or(delivery.receiver_user_id) {
                let ephemeral = serde_json::to_string(&EphemeralMessageParameters {
                    receiver_user_id,
                    callback_query_id: delivery.callback_query_id.clone(),
                    replace_callback_query_message: Self::replace_callback_query_message(),
                })
                .map_err(|error| error.to_string())?;
                form = form.text("ephemeral_message_parameters", ephemeral);
            }
            let reply_parameters = delivery
                .source_ephemeral_message_id
                .map(ReplyParameters::ephemeral)
                .or_else(|| reply_to_message_id.map(ReplyParameters::new));
            if let Some(reply_parameters) = reply_parameters {
                form = form.text(
                    "reply_parameters",
                    serde_json::to_string(&reply_parameters).map_err(|error| error.to_string())?,
                );
            }
        } else if let Some(reply_to_message_id) = reply_to_message_id {
            form = form.text(
                "reply_parameters",
                serde_json::to_string(&ReplyParameters::new(reply_to_message_id))
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(form)
    }

    async fn read_bounded_json(response: reqwest::Response) -> Result<Value, String> {
        if response
            .content_length()
            .is_some_and(|length| length > MAX_TELEGRAM_RESPONSE_BYTES as u64)
        {
            return Err(format!(
                "response exceeded {MAX_TELEGRAM_RESPONSE_BYTES} bytes"
            ));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| Self::reqwest_error_kind(&error).to_string())?;
            if bytes.len().saturating_add(chunk.len()) > MAX_TELEGRAM_RESPONSE_BYTES {
                return Err(format!(
                    "response exceeded {MAX_TELEGRAM_RESPONSE_BYTES} bytes"
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid JSON: {error}"))
    }

    fn reqwest_error_kind(error: &reqwest::Error) -> &'static str {
        if error.is_timeout() {
            "timeout"
        } else if error.is_connect() {
            "connection failure"
        } else if error.is_request() {
            "request failure"
        } else if error.is_body() {
            "body failure"
        } else if error.is_decode() {
            "decode failure"
        } else {
            "transport failure"
        }
    }

    async fn post_json_raw(&self, method: &str, payload: Value) -> Result<Value, String> {
        let url = format!("{}/{method}", self.base_url);
        let mut last_error = None;
        for attempt in 0..MAX_TELEGRAM_ATTEMPTS {
            match self.client.post(&url).json(&payload).send().await {
                Ok(response) => match Self::read_bounded_json(response).await {
                    Ok(value) => {
                        if let Some(delay) = retry_delay_from_response(&value, attempt) {
                            warn!(
                                method,
                                attempt = attempt + 1,
                                delay_ms = delay.as_millis(),
                                "Telegram request is transiently rate-limited/unavailable; retrying"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        return Ok(value);
                    }
                    Err(error) => {
                        if let Some(delay) = retry_delay_from_error(&error, attempt) {
                            last_error = Some(error);
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        return Err(error);
                    }
                },
                Err(error) => {
                    let normalized = format!(
                        "HTTP error for {method}: {}",
                        Self::reqwest_error_kind(&error)
                    );
                    if let Some(delay) = retry_delay_from_error(&normalized, attempt) {
                        last_error = Some(normalized);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(normalized);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| format!("Telegram request [{method}] exhausted retries")))
    }

    async fn post_json(&self, method: &str, payload: Value) -> Result<Value, String> {
        let response = self.post_json_raw(method, payload).await?;
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(response)
        } else {
            Err(Self::telegram_api_error(method, &response))
        }
    }

    async fn post_multipart<F>(&self, method: &str, mut build: F) -> Result<Value, String>
    where
        F: FnMut() -> Result<Form, String>,
    {
        let url = format!("{}/{method}", self.base_url);
        let mut last_error = None;
        for attempt in 0..MAX_TELEGRAM_ATTEMPTS {
            let form = build()?;
            match self.client.post(&url).multipart(form).send().await {
                Ok(response) => match Self::read_bounded_json(response).await {
                    Ok(value) => {
                        if value.get("ok").and_then(Value::as_bool) == Some(true) {
                            return Ok(value);
                        }
                        if let Some(delay) = retry_delay_from_response(&value, attempt) {
                            last_error = Some(Self::telegram_api_error(method, &value));
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        return Err(Self::telegram_api_error(method, &value));
                    }
                    Err(error) => {
                        if let Some(delay) = retry_delay_from_error(&error, attempt) {
                            last_error = Some(error);
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        return Err(error);
                    }
                },
                Err(error) => {
                    let normalized = format!(
                        "{method} multipart error: {}",
                        Self::reqwest_error_kind(&error)
                    );
                    if let Some(delay) = retry_delay_from_error(&normalized, attempt) {
                        last_error = Some(normalized);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(normalized);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| format!("Telegram request [{method}] exhausted retries")))
    }

    pub async fn get_me(&self) -> Result<ApiResponse<User>, String> {
        let value = self.post_json("getMe", json!({})).await?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    pub async fn get_file(&self, file_id: &str) -> Result<ApiResponse<FileInfo>, String> {
        let value = self
            .post_json("getFile", json!({"file_id": file_id}))
            .await?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    pub async fn get_file_bytes(&self, file_id: &str) -> Option<(Vec<u8>, String)> {
        let file_res = self.get_file(file_id).await.ok()?;
        if !file_res.ok {
            return None;
        }
        let info = file_res.result?;
        if info
            .file_size
            .and_then(|size| usize::try_from(size).ok())
            .is_some_and(|size| size > MAX_TELEGRAM_DOWNLOAD_BYTES)
        {
            return None;
        }
        let file_path = info.file_path?;
        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.token, file_path
        );
        let response = self.client.get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.ok()?;
            if bytes.len().saturating_add(chunk.len()) > MAX_TELEGRAM_DOWNLOAD_BYTES {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        Some((bytes, file_path))
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        limit: i32,
        timeout: i32,
        allowed_updates: Option<Vec<String>>,
    ) -> Result<ApiResponse<Vec<Update>>, String> {
        let mut payload = json!({"limit": limit, "timeout": timeout});
        if let Some(offset) = offset {
            payload["offset"] = json!(offset);
        }
        if let Some(allowed_updates) = allowed_updates {
            payload["allowed_updates"] = json!(allowed_updates);
        }
        let value = self.post_json("getUpdates", payload).await?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        receiver_user_id: Option<i64>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let chunks = if text.chars().count() > 4000 {
            self.inner.split_text_chunks(text, 3800)
        } else {
            vec![text.to_string()]
        };
        let total = chunks.len();
        let mut last = json!({"ok": true});
        for (index, chunk) in chunks.into_iter().enumerate() {
            let mut payload = json!({"chat_id": chat_id, "text": chunk});
            if let Some(parse_mode) = parse_mode {
                payload["parse_mode"] = json!(parse_mode);
            }
            if index + 1 == total {
                if let Some(ref reply_markup) = reply_markup {
                    payload["reply_markup"] = reply_markup.clone();
                }
            }
            if let Some(receiver_user_id) = receiver_user_id {
                payload["ephemeral_message_parameters"] =
                    serde_json::to_value(EphemeralMessageParameters {
                        receiver_user_id,
                        callback_query_id: Self::current_delivery_context().callback_query_id,
                        replace_callback_query_message: Self::replace_callback_query_message(),
                    })
                    .unwrap_or(json!({}));
            }
            if index == 0 {
                if let Some(reply_to_message_id) = reply_to_message_id {
                    payload["reply_parameters"] =
                        serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                            .unwrap_or(json!({}));
                }
            }
            Self::apply_delivery_context(&mut payload, true);
            last = self.post_json("sendMessage", payload).await?;
        }
        Ok(last)
    }

    pub async fn download_media_bytes(
        &self,
        url: &str,
        max_bytes: usize,
    ) -> Option<(Vec<u8>, String, String)> {
        self.inner.download_media_bytes(url, max_bytes).await
    }

    pub async fn send_photo_bytes(
        &self,
        chat_id: i64,
        photo_bytes: Vec<u8>,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let (file_name, mime_type) = if photo_bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            ("image.png", "image/png")
        } else if photo_bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            ("image.jpg", "image/jpeg")
        } else if photo_bytes.starts_with(b"GIF87a") || photo_bytes.starts_with(b"GIF89a") {
            ("image.gif", "image/gif")
        } else if photo_bytes.len() >= 12
            && &photo_bytes[..4] == b"RIFF"
            && &photo_bytes[8..12] == b"WEBP"
        {
            ("image.webp", "image/webp")
        } else {
            return Err("sendPhoto rejected bytes with an unsupported image signature".to_string());
        };
        let caption = caption.map(str::to_string);
        let parse_mode = parse_mode.map(str::to_string);
        self.post_multipart("sendPhoto", || {
            let part = Part::bytes(photo_bytes.clone())
                .file_name(file_name)
                .mime_str(mime_type)
                .map_err(|error| error.to_string())?;
            let mut form = Form::new()
                .text("chat_id", chat_id.to_string())
                .part("photo", part);
            if let Some(ref caption) = caption {
                form = form.text("caption", caption.clone());
            }
            if let Some(ref parse_mode) = parse_mode {
                form = form.text("parse_mode", parse_mode.clone());
            }
            if let Some(ref reply_markup) = reply_markup {
                form = form.text("reply_markup", reply_markup.to_string());
            }
            self.apply_form_delivery_context(form, true, None, reply_to_message_id)
        })
        .await
    }

    pub async fn send_photo(
        &self,
        chat_id: i64,
        photo: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let mut payload = json!({"chat_id": chat_id, "photo": photo});
        Self::add_caption_fields(&mut payload, caption, parse_mode, reply_markup.as_ref());
        if let Some(reply_to_message_id) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        match self.post_json("sendPhoto", payload).await {
            Ok(value) => Ok(value),
            Err(error)
                if fallback_allowed_error(&error)
                    && (photo.starts_with("http://") || photo.starts_with("https://")) =>
            {
                let Some((bytes, _, _)) = self
                    .download_media_bytes(photo, MAX_TELEGRAM_DOWNLOAD_BYTES)
                    .await
                else {
                    return Err(error);
                };
                self.send_photo_bytes(
                    chat_id,
                    bytes,
                    caption,
                    parse_mode,
                    reply_markup,
                    reply_to_message_id,
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn send_media_group(
        &self,
        chat_id: i64,
        media: &[InputMedia],
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        InputMedia::validate_media_group(media)?;
        let mut payload = json!({
            "chat_id": chat_id,
            "media": serde_json::to_value(media).map_err(|error| error.to_string())?,
        });
        if let Some(reply_to_message_id) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, false);
        match self.post_json("sendMediaGroup", payload).await {
            Ok(value) => Ok(value),
            Err(error) if fallback_allowed_error(&error) => {
                let mut updated = media.to_vec();
                let mut attachments = Vec::new();
                for (index, item) in updated.iter_mut().enumerate() {
                    let source = Self::media_reference(item).to_string();
                    if source.starts_with("http://") || source.starts_with("https://") {
                        if let Some((bytes, mime, file_name)) = self
                            .download_media_bytes(&source, MAX_TELEGRAM_DOWNLOAD_BYTES)
                            .await
                        {
                            let attach_name = format!("file_{index}");
                            Self::set_media_reference(item, format!("attach://{attach_name}"));
                            attachments.push((attach_name, bytes, mime, file_name));
                        }
                    }
                }
                if attachments.is_empty() {
                    return Err(error);
                }
                self.post_multipart("sendMediaGroup", || {
                    let mut form = Form::new().text("chat_id", chat_id.to_string()).text(
                        "media",
                        serde_json::to_string(&updated).map_err(|error| error.to_string())?,
                    );
                    form =
                        self.apply_form_delivery_context(form, false, None, reply_to_message_id)?;
                    for (attach_name, bytes, mime, file_name) in &attachments {
                        let part = Part::bytes(bytes.clone())
                            .file_name(file_name.clone())
                            .mime_str(mime)
                            .map_err(|error| error.to_string())?;
                        form = form.part(attach_name.clone(), part);
                    }
                    Ok(form)
                })
                .await
            }
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_audio(
        &self,
        chat_id: i64,
        audio: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        title: Option<&str>,
        performer: Option<&str>,
        duration: Option<i32>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let mut extras = Vec::new();
        if let Some(title) = title {
            extras.push(("title", title.to_string()));
        }
        if let Some(performer) = performer {
            extras.push(("performer", performer.to_string()));
        }
        if let Some(duration) = duration {
            extras.push(("duration", duration.to_string()));
        }
        self.send_media_with_url_fallback(
            "sendAudio",
            "audio",
            chat_id,
            audio,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            extras,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_voice(
        &self,
        chat_id: i64,
        voice: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        duration: Option<i32>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let extras = duration
            .map(|duration| vec![("duration", duration.to_string())])
            .unwrap_or_default();
        self.send_media_with_url_fallback(
            "sendVoice",
            "voice",
            chat_id,
            voice,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            extras,
        )
        .await
    }

    pub async fn send_video(
        &self,
        chat_id: i64,
        video: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        self.send_media_with_url_fallback(
            "sendVideo",
            "video",
            chat_id,
            video,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            Vec::new(),
        )
        .await
    }

    pub async fn send_animation(
        &self,
        chat_id: i64,
        animation: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        self.send_media_with_url_fallback(
            "sendAnimation",
            "animation",
            chat_id,
            animation,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            Vec::new(),
        )
        .await
    }

    pub async fn send_document(
        &self,
        chat_id: i64,
        document: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        self.send_media_with_url_fallback(
            "sendDocument",
            "document",
            chat_id,
            document,
            caption,
            parse_mode,
            reply_markup,
            reply_to_message_id,
            Vec::new(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_media_with_url_fallback(
        &self,
        method: &str,
        field: &str,
        chat_id: i64,
        media: &str,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
        extras: Vec<(&'static str, String)>,
    ) -> Result<Value, String> {
        let mut payload = json!({"chat_id": chat_id});
        payload[field] = json!(media);
        Self::add_caption_fields(&mut payload, caption, parse_mode, reply_markup.as_ref());
        for (key, value) in &extras {
            payload[*key] = json!(value);
        }
        if let Some(reply_to_message_id) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        match self.post_json(method, payload).await {
            Ok(value) => Ok(value),
            Err(error)
                if fallback_allowed_error(&error)
                    && (media.starts_with("http://") || media.starts_with("https://")) =>
            {
                let Some((bytes, mime, file_name)) = self
                    .download_media_bytes(media, MAX_TELEGRAM_DOWNLOAD_BYTES)
                    .await
                else {
                    return Err(error);
                };
                let caption = caption.map(str::to_string);
                let parse_mode = parse_mode.map(str::to_string);
                self.post_multipart(method, || {
                    let part = Part::bytes(bytes.clone())
                        .file_name(file_name.clone())
                        .mime_str(&mime)
                        .map_err(|error| error.to_string())?;
                    let mut form = Form::new()
                        .text("chat_id", chat_id.to_string())
                        .part(field.to_string(), part);
                    if let Some(ref caption) = caption {
                        form = form.text("caption", caption.clone());
                    }
                    if let Some(ref parse_mode) = parse_mode {
                        form = form.text("parse_mode", parse_mode.clone());
                    }
                    if let Some(ref reply_markup) = reply_markup {
                        form = form.text("reply_markup", reply_markup.to_string());
                    }
                    for (key, value) in &extras {
                        form = form.text((*key).to_string(), value.clone());
                    }
                    self.apply_form_delivery_context(form, true, None, reply_to_message_id)
                })
                .await
            }
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_location(
        &self,
        chat_id: i64,
        latitude: f64,
        longitude: f64,
        horizontal_accuracy: Option<f64>,
        live_period: Option<i32>,
        reply_markup: Option<Value>,
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "latitude": latitude,
            "longitude": longitude,
        });
        if let Some(horizontal_accuracy) = horizontal_accuracy {
            payload["horizontal_accuracy"] = json!(horizontal_accuracy);
        }
        if let Some(live_period) = live_period {
            payload["live_period"] = json!(live_period);
        }
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        if let Some(reply_to_message_id) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(reply_to_message_id))
                    .unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        self.post_json("sendLocation", payload).await
    }

    pub async fn edit_message_text(
        &self,
        chat_id: Option<i64>,
        message_id: Option<i64>,
        text: &str,
        parse_mode: Option<&str>,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let delivery = Self::current_delivery_context();
        let mut payload = json!({"text": text});
        let method = if let (Some(chat_id), Some((receiver_user_id, ephemeral_message_id))) = (
            chat_id,
            delivery
                .receiver_user_id
                .zip(delivery.source_ephemeral_message_id),
        ) {
            payload["chat_id"] = json!(chat_id);
            payload["receiver_user_id"] = json!(receiver_user_id);
            payload["ephemeral_message_id"] = json!(ephemeral_message_id);
            "editEphemeralMessageText"
        } else {
            if let Some(chat_id) = chat_id {
                payload["chat_id"] = json!(chat_id);
            }
            if let Some(message_id) = message_id {
                payload["message_id"] = json!(message_id);
            }
            "editMessageText"
        };
        if let Some(parse_mode) = parse_mode {
            payload["parse_mode"] = json!(parse_mode);
        }
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        self.post_json(method, payload).await
    }

    pub async fn edit_rich_message(
        &self,
        chat_id: i64,
        message_id: i64,
        rich_message: &InputRichMessage,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        rich_message.validate()?;
        let delivery = Self::current_delivery_context();
        let rich_json = serde_json::to_value(rich_message).map_err(|error| error.to_string())?;
        let (method, mut payload) = if let Some((receiver_user_id, ephemeral_message_id)) = delivery
            .receiver_user_id
            .zip(delivery.source_ephemeral_message_id)
        {
            (
                "editEphemeralMessageText",
                json!({
                    "chat_id": chat_id,
                    "receiver_user_id": receiver_user_id,
                    "ephemeral_message_id": ephemeral_message_id,
                    "rich_message": rich_json,
                }),
            )
        } else {
            (
                "editMessageText",
                json!({"chat_id": chat_id, "message_id": message_id, "rich_message": rich_json}),
            )
        };
        if let Some(ref reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup.clone();
        }
        let response = self.post_json_raw(method, payload).await?;
        if response.get("ok").and_then(Value::as_bool) == Some(true)
            || response
                .get("description")
                .and_then(Value::as_str)
                .is_some_and(|description| {
                    description
                        .to_ascii_lowercase()
                        .contains("message is not modified")
                })
        {
            return Ok(response);
        }
        if !fallback_allowed_response(&response) {
            return Err(Self::telegram_api_error(method, &response));
        }
        let html = self.inner.render_blocks_to_html(&rich_message.blocks);
        self.edit_message_text(
            Some(chat_id),
            Some(message_id),
            &html,
            Some("HTML"),
            reply_markup,
        )
        .await
    }

    pub async fn edit_ephemeral_message_media(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
        media: &InputMedia,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "receiver_user_id": receiver_user_id,
            "ephemeral_message_id": ephemeral_message_id,
            "media": serde_json::to_value(media).map_err(|error| error.to_string())?,
        });
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        self.post_json("editEphemeralMessageMedia", payload).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_ephemeral_message_media_bytes(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
        media: &InputMedia,
        bytes: Vec<u8>,
        file_name: &str,
        mime: &str,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut attached_media = media.clone();
        Self::set_media_reference(&mut attached_media, "attach://media".to_string());
        let media_json =
            serde_json::to_string(&attached_media).map_err(|error| error.to_string())?;
        let file_name = file_name.to_string();
        let mime = mime.to_string();
        self.post_multipart("editEphemeralMessageMedia", || {
            let part = Part::bytes(bytes.clone())
                .file_name(file_name.clone())
                .mime_str(&mime)
                .map_err(|error| error.to_string())?;
            let mut form = Form::new()
                .text("chat_id", chat_id.to_string())
                .text("receiver_user_id", receiver_user_id.to_string())
                .text("ephemeral_message_id", ephemeral_message_id.to_string())
                .text("media", media_json.clone())
                .part("media", part);
            if let Some(ref reply_markup) = reply_markup {
                form = form.text("reply_markup", reply_markup.to_string());
            }
            Ok(form)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_ephemeral_message_caption(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        show_caption_above_media: Option<bool>,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "receiver_user_id": receiver_user_id,
            "ephemeral_message_id": ephemeral_message_id,
        });
        if let Some(caption) = caption {
            payload["caption"] = json!(caption);
        }
        if let Some(parse_mode) = parse_mode {
            payload["parse_mode"] = json!(parse_mode);
        }
        if let Some(show_caption_above_media) = show_caption_above_media {
            payload["show_caption_above_media"] = json!(show_caption_above_media);
        }
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        self.post_json("editEphemeralMessageCaption", payload).await
    }

    pub async fn edit_ephemeral_message_reply_markup(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
        reply_markup: Option<Value>,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "receiver_user_id": receiver_user_id,
            "ephemeral_message_id": ephemeral_message_id,
        });
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup;
        }
        self.post_json("editEphemeralMessageReplyMarkup", payload)
            .await
    }

    pub async fn send_rich_message_with_media(
        &self,
        chat_id: i64,
        rich_message: &InputRichMessage,
        attached_files: Vec<(String, Vec<u8>, String)>,
        reply_markup: Option<Value>,
        receiver_user_id: Option<i64>,
    ) -> Result<Value, String> {
        if attached_files.is_empty() {
            return self
                .send_rich_message(chat_id, rich_message, reply_markup, receiver_user_id)
                .await;
        }
        rich_message.validate()?;
        let rich_json = serde_json::to_string(rich_message).map_err(|error| error.to_string())?;
        self.post_multipart("sendRichMessage", || {
            let mut form = Form::new()
                .text("chat_id", chat_id.to_string())
                .text("rich_message", rich_json.clone());
            if let Some(ref reply_markup) = reply_markup {
                form = form.text("reply_markup", reply_markup.to_string());
            }
            form = self.apply_form_delivery_context(form, true, receiver_user_id, None)?;
            for (attach_name, bytes, mime) in &attached_files {
                let part = Part::bytes(bytes.clone())
                    .file_name(attach_name.clone())
                    .mime_str(mime)
                    .map_err(|error| error.to_string())?;
                form = form.part(attach_name.clone(), part);
            }
            Ok(form)
        })
        .await
    }

    pub async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<Value, String> {
        let delivery = Self::current_delivery_context();
        if let Some(receiver_user_id) = delivery.receiver_user_id {
            if let Some(source_id) = delivery.source_ephemeral_message_id {
                if message_id == source_id {
                    return self
                        .delete_ephemeral_message(chat_id, receiver_user_id, source_id)
                        .await;
                }
            }
        }
        self.post_json(
            "deleteMessage",
            json!({"chat_id": chat_id, "message_id": message_id}),
        )
        .await
    }

    pub async fn delete_ephemeral_message(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
    ) -> Result<Value, String> {
        self.post_json(
            "deleteEphemeralMessage",
            json!({
                "chat_id": chat_id,
                "receiver_user_id": receiver_user_id,
                "ephemeral_message_id": ephemeral_message_id,
            }),
        )
        .await
    }

    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
        show_alert: bool,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "callback_query_id": callback_query_id,
            "show_alert": show_alert,
        });
        if let Some(text) = text {
            payload["text"] = json!(text);
        }
        self.post_json("answerCallbackQuery", payload).await
    }

    pub async fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<Value, String> {
        let mut payload = json!({"chat_id": chat_id, "action": action});
        Self::apply_delivery_context(&mut payload, false);
        self.post_json("sendChatAction", payload).await
    }

    pub async fn send_rich_message_draft(
        &self,
        chat_id: i64,
        draft_id: i64,
        rich_message: &InputRichMessage,
        can_stop: bool,
        keep_on_stop: bool,
    ) -> Result<Value, String> {
        rich_message.validate()?;
        let mut payload = json!({
            "chat_id": chat_id,
            "draft_id": draft_id,
            "rich_message": serde_json::to_value(rich_message).map_err(|error| error.to_string())?,
            "can_stop": can_stop,
            "keep_on_stop": keep_on_stop,
        });
        Self::apply_delivery_context(&mut payload, false);
        let response = self.post_json_raw("sendRichMessageDraft", payload).await?;
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(response);
        }
        if !fallback_allowed_response(&response) {
            return Err(Self::telegram_api_error("sendRichMessageDraft", &response));
        }
        let text = rich_message
            .blocks
            .first()
            .and_then(|block| match block {
                RichBlock::Thinking { text } => text.as_str(),
                _ => None,
            })
            .unwrap_or("Thinking...");
        self.send_message_draft(
            chat_id,
            draft_id,
            text,
            Some("HTML"),
            can_stop,
            keep_on_stop,
        )
        .await
    }

    pub async fn send_message_draft(
        &self,
        chat_id: i64,
        draft_id: i64,
        text: &str,
        parse_mode: Option<&str>,
        can_stop: bool,
        keep_on_stop: bool,
    ) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "draft_id": draft_id,
            "text": text,
            "can_stop": can_stop,
            "keep_on_stop": keep_on_stop,
        });
        if let Some(parse_mode) = parse_mode {
            payload["parse_mode"] = json!(parse_mode);
        }
        Self::apply_delivery_context(&mut payload, false);
        self.post_json("sendMessageDraft", payload).await
    }

    pub async fn send_rich_message(
        &self,
        chat_id: i64,
        rich_message: &InputRichMessage,
        reply_markup: Option<Value>,
        receiver_user_id: Option<i64>,
    ) -> Result<Value, String> {
        let validation = rich_message.validate();
        if validation.is_ok() {
            let mut payload = json!({
                "chat_id": chat_id,
                "rich_message": serde_json::to_value(rich_message).map_err(|error| error.to_string())?,
            });
            if let Some(ref reply_markup) = reply_markup {
                payload["reply_markup"] = reply_markup.clone();
            }
            if let Some(receiver_user_id) = receiver_user_id {
                payload["ephemeral_message_parameters"] =
                    serde_json::to_value(EphemeralMessageParameters {
                        receiver_user_id,
                        callback_query_id: Self::current_delivery_context().callback_query_id,
                        replace_callback_query_message: Self::replace_callback_query_message(),
                    })
                    .unwrap_or(json!({}));
            }
            Self::apply_delivery_context(&mut payload, true);
            let response = self.post_json_raw("sendRichMessage", payload).await?;
            if response.get("ok").and_then(Value::as_bool) == Some(true) {
                return Ok(response);
            }
            if !fallback_allowed_response(&response) {
                return Err(Self::telegram_api_error("sendRichMessage", &response));
            }
            info!("Telegram rejected Rich Message with a bad request; degrading to safe HTML");
        } else if let Err(error) = validation {
            if rich_message.blocks.is_empty() {
                return Err(error);
            }
            info!("Rich Message validation required degradation: {error}");
        }

        let html_chunks = self
            .inner
            .render_blocks_to_html_chunks(&rich_message.blocks, 3800);
        let total = html_chunks.len();
        let mut html_last = json!({"ok": true});
        for (index, chunk) in html_chunks.into_iter().enumerate() {
            match self
                .send_message(
                    chat_id,
                    &chunk,
                    Some("HTML"),
                    if index + 1 == total {
                        reply_markup.clone()
                    } else {
                        None
                    },
                    receiver_user_id,
                    None,
                )
                .await
            {
                Ok(response) => html_last = response,
                Err(error) if fallback_allowed_error(&error) => {
                    let plain_chunks = self
                        .inner
                        .render_blocks_to_plain_chunks(&rich_message.blocks, 4000);
                    let plain_total = plain_chunks.len();
                    let mut plain_last = json!({"ok": true});
                    for (plain_index, plain) in plain_chunks.into_iter().enumerate() {
                        plain_last = self
                            .send_message(
                                chat_id,
                                &plain,
                                None,
                                if plain_index + 1 == plain_total {
                                    reply_markup.clone()
                                } else {
                                    None
                                },
                                receiver_user_id,
                                None,
                            )
                            .await?;
                    }
                    return Ok(plain_last);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(html_last)
    }

    pub async fn set_my_commands(&self, commands: &[BotCommand]) -> Result<Value, String> {
        self.post_json(
            "setMyCommands",
            json!({"commands": serde_json::to_value(commands).unwrap_or(json!([]))}),
        )
        .await
    }

    fn add_caption_fields(
        payload: &mut Value,
        caption: Option<&str>,
        parse_mode: Option<&str>,
        reply_markup: Option<&Value>,
    ) {
        if let Some(caption) = caption {
            payload["caption"] = json!(caption);
        }
        if let Some(parse_mode) = parse_mode {
            payload["parse_mode"] = json!(parse_mode);
        }
        if let Some(reply_markup) = reply_markup {
            payload["reply_markup"] = reply_markup.clone();
        }
    }

    fn media_reference(media: &InputMedia) -> &str {
        match media {
            InputMedia::Photo { media, .. }
            | InputMedia::Video { media, .. }
            | InputMedia::Animation { media, .. }
            | InputMedia::Audio { media, .. }
            | InputMedia::Document { media, .. }
            | InputMedia::VoiceNote { media, .. } => media,
        }
    }

    fn set_media_reference(media: &mut InputMedia, value: String) {
        match media {
            InputMedia::Photo { media, .. }
            | InputMedia::Video { media, .. }
            | InputMedia::Animation { media, .. }
            | InputMedia::Audio { media, .. }
            | InputMedia::Document { media, .. }
            | InputMedia::VoiceNote { media, .. } => *media = value,
        }
    }
}
