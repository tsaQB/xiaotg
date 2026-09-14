#![allow(dead_code)]

use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{error, info, warn};

use super::models::{
    ApiResponse, BotCommand, EphemeralMessageParameters, FileInfo, InputMedia, InputRichMessage,
    ReplyParameters, RichBlock, RichBlockCaption, RichBlockTableCell, Update, User,
};
use super::url_policy::resolve_download_url;
use futures_util::StreamExt;

const MAX_TELEGRAM_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;
const MAX_TELEGRAM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct TelegramDeliveryContext {
    pub message_thread_id: Option<i64>,
    pub receiver_user_id: Option<i64>,
    pub source_ephemeral_message_id: Option<i64>,
    pub callback_query_id: Option<String>,
}

tokio::task_local! {
    static TELEGRAM_DELIVERY_CONTEXT: TelegramDeliveryContext;
}

async fn read_bounded_json_response(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Value, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("response exceeded {max_bytes} bytes"));
    }

    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| reqwest_error_kind(&error).to_string())?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("response exceeded {max_bytes} bytes"));
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

#[derive(Clone)]
pub struct TelegramBotClient {
    token: String,
    base_url: String,
    client: Client,
}

impl TelegramBotClient {
    pub async fn with_delivery_context<F, T>(context: TelegramDeliveryContext, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        TELEGRAM_DELIVERY_CONTEXT.scope(context, future).await
    }

    pub fn current_delivery_context() -> TelegramDeliveryContext {
        TELEGRAM_DELIVERY_CONTEXT
            .try_with(Clone::clone)
            .unwrap_or_default()
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
                    replace_callback_query_message: None,
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

    pub fn new(token: impl Into<String>) -> Self {
        let token_str = token.into().trim().to_string();
        let base_url = format!("https://api.telegram.org/bot{}", token_str);
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            token: token_str,
            base_url,
            client,
        }
    }

    pub fn token(&self) -> &str {
        &self.token
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

    async fn post_json_raw(&self, method: &str, payload: Value) -> Result<Value, String> {
        let url = format!("{}/{}", self.base_url, method);
        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => match read_bounded_json_response(resp, MAX_TELEGRAM_RESPONSE_BYTES).await {
                Ok(json_res) => Ok(json_res),
                Err(err_msg) => {
                    error!("Failed to parse response JSON for {method}: {err_msg}");
                    Err(format!(
                        "Failed to parse response JSON for {method}: {err_msg}"
                    ))
                }
            },
            Err(e) => {
                // Do not format reqwest::Error directly here: it can contain the full
                // Telegram URL, and Telegram URLs contain the bot token.
                let err_msg = format!("HTTP error for {method}: {}", reqwest_error_kind(&e));
                error!("{err_msg}");
                Err(err_msg)
            }
        }
    }

    async fn post_json(&self, method: &str, payload: Value) -> Result<Value, String> {
        let response = self.post_json_raw(method, payload).await?;
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(response);
        }
        let error = Self::telegram_api_error(method, &response);
        warn!("{error}");
        Err(error)
    }

    // ==========================================
    // Basic Telegram API Methods
    // ==========================================

    pub async fn get_me(&self) -> Result<ApiResponse<User>, String> {
        let val = self.post_json("getMe", json!({})).await?;
        serde_json::from_value(val).map_err(|e| e.to_string())
    }

    pub async fn get_file(&self, file_id: &str) -> Result<ApiResponse<FileInfo>, String> {
        let val = self
            .post_json("getFile", json!({ "file_id": file_id }))
            .await?;
        serde_json::from_value(val).map_err(|e| e.to_string())
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
            warn!(
                "Telegram file rejected before download: size exceeds Xiao limit of {} bytes",
                MAX_TELEGRAM_DOWNLOAD_BYTES
            );
            return None;
        }
        let file_path = info.file_path?;
        let file_url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.token, file_path
        );

        let dl_client = Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .unwrap_or_else(|_| self.client.clone());

        match dl_client.get(&file_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                use futures_util::StreamExt;
                let mut stream = resp.bytes_stream();
                let mut bytes_buf = Vec::new();
                while let Some(chunk_res) = stream.next().await {
                    match chunk_res {
                        Ok(chunk) => {
                            if bytes_buf.len().saturating_add(chunk.len())
                                > MAX_TELEGRAM_DOWNLOAD_BYTES
                            {
                                warn!(
                                    "Telegram file download aborted: streamed body exceeded Xiao limit of {} bytes",
                                    MAX_TELEGRAM_DOWNLOAD_BYTES
                                );
                                return None;
                            }
                            bytes_buf.extend_from_slice(&chunk)
                        }
                        Err(e) => {
                            error!("Telegram file streaming error: {}", reqwest_error_kind(&e));
                            return None;
                        }
                    }
                }
                Some((bytes_buf, file_path))
            }
            Ok(resp) => {
                error!(
                    "Telegram file download failed with status {}",
                    resp.status()
                );
                None
            }
            Err(e) => {
                error!("Telegram file download error: {}", reqwest_error_kind(&e));
                None
            }
        }
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        limit: i32,
        timeout: i32,
        allowed_updates: Option<Vec<String>>,
    ) -> Result<ApiResponse<Vec<Update>>, String> {
        let mut payload = json!({
            "limit": limit,
            "timeout": timeout,
        });
        if let Some(off) = offset {
            payload["offset"] = json!(off);
        }
        if let Some(allowed) = allowed_updates {
            payload["allowed_updates"] = json!(allowed);
        }

        let val = self.post_json("getUpdates", payload).await?;
        serde_json::from_value(val).map_err(|e| e.to_string())
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
        if text.chars().count() > 4000 {
            let chunks = self.split_text_chunks(text, 3800);
            let mut last_res = json!({ "ok": true });
            let total = chunks.len();
            for (idx, chunk) in chunks.into_iter().enumerate() {
                let is_last = idx == total - 1;
                let is_first = idx == 0;

                let mut payload = json!({
                    "chat_id": chat_id,
                    "text": chunk,
                });
                if let Some(pm) = parse_mode {
                    payload["parse_mode"] = json!(pm);
                }
                if is_last {
                    if let Some(ref rm) = reply_markup {
                        payload["reply_markup"] = rm.clone();
                    }
                }
                if let Some(recv) = receiver_user_id {
                    payload["ephemeral_message_parameters"] =
                        serde_json::to_value(EphemeralMessageParameters {
                            receiver_user_id: recv,
                            callback_query_id: None,
                            replace_callback_query_message: None,
                        })
                        .unwrap_or(json!({}));
                }
                if is_first {
                    if let Some(rep) = reply_to_message_id {
                        payload["reply_parameters"] =
                            serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
                    }
                }
                Self::apply_delivery_context(&mut payload, true);
                last_res = self.post_json("sendMessage", payload).await?;
            }
            return Ok(last_res);
        }

        let mut payload = json!({
            "chat_id": chat_id,
            "text": text,
        });
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(rm) = reply_markup {
            payload["reply_markup"] = rm;
        }
        if let Some(recv) = receiver_user_id {
            payload["ephemeral_message_parameters"] =
                serde_json::to_value(EphemeralMessageParameters {
                    receiver_user_id: recv,
                    callback_query_id: None,
                    replace_callback_query_message: None,
                })
                .unwrap_or(json!({}));
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }

        Self::apply_delivery_context(&mut payload, true);
        self.post_json("sendMessage", payload).await
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
        let url = format!("{}/sendPhoto", self.base_url);
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
        let part = Part::bytes(photo_bytes)
            .file_name(file_name)
            .mime_str(mime_type)
            .map_err(|e| e.to_string())?;

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", part);

        if let Some(cap) = caption {
            form = form.text("caption", cap.to_string());
        }
        if let Some(pm) = parse_mode {
            form = form.text("parse_mode", pm.to_string());
        }
        let delivery = Self::current_delivery_context();
        if let Some(thread_id) = delivery.message_thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        if let Some(receiver_user_id) = delivery.receiver_user_id {
            let ephemeral = serde_json::to_string(&EphemeralMessageParameters {
                receiver_user_id,
                callback_query_id: delivery.callback_query_id.clone(),
                replace_callback_query_message: None,
            })
            .map_err(|e| e.to_string())?;
            form = form.text("ephemeral_message_parameters", ephemeral);
        }
        let reply_parameters =
            if let Some(ephemeral_message_id) = delivery.source_ephemeral_message_id {
                Some(ReplyParameters::ephemeral(ephemeral_message_id))
            } else {
                reply_to_message_id.map(ReplyParameters::new)
            };
        if let Some(reply_parameters) = reply_parameters {
            form = form.text(
                "reply_parameters",
                serde_json::to_string(&reply_parameters).map_err(|e| e.to_string())?,
            );
        }
        if let Some(rm) = reply_markup {
            form = form.text("reply_markup", rm.to_string());
        }

        match self.client.post(&url).multipart(form).send().await {
            Ok(resp) => {
                let response = resp.json::<Value>().await.map_err(|e| {
                    format!(
                        "sendPhoto response decode error: {}",
                        reqwest_error_kind(&e)
                    )
                })?;
                if response.get("ok").and_then(Value::as_bool) == Some(true) {
                    Ok(response)
                } else {
                    Err(Self::telegram_api_error("sendPhoto", &response))
                }
            }
            Err(e) => Err(format!(
                "sendPhoto multipart error: {}",
                reqwest_error_kind(&e)
            )),
        }
    }

    pub async fn download_media_bytes(
        &self,
        url: &str,
        max_bytes: usize,
    ) -> Option<(Vec<u8>, String, String)> {
        let resolved = resolve_download_url(url).await.ok()?;
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve(&resolved.host, resolved.address)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&resolved.host, resolved.address)
            .build()
            .ok()?;

        let resp = client.get(resolved.url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        if resp
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return None;
        }

        let mut stream = resp.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res.ok()?;
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }

        let file_name = if content_type.contains("png") || bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image.png"
        } else if content_type.contains("jpeg")
            || content_type.contains("jpg")
            || bytes.starts_with(&[0xff, 0xd8, 0xff])
        {
            "image.jpg"
        } else if content_type.contains("gif")
            || bytes.starts_with(b"GIF87a")
            || bytes.starts_with(b"GIF89a")
        {
            "image.gif"
        } else if content_type.contains("webp")
            || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        {
            "image.webp"
        } else if content_type.contains("ogg")
            || content_type.contains("opus")
            || bytes.starts_with(b"OggS")
        {
            "audio.ogg"
        } else if content_type.contains("mp3")
            || content_type.contains("mpeg")
            || bytes.starts_with(b"ID3")
            || bytes.starts_with(&[0xff, 0xfb])
        {
            "audio.mp3"
        } else if content_type.contains("mp4") || bytes.windows(4).take(8).any(|w| w == b"ftyp") {
            "video.mp4"
        } else if content_type.contains("pdf") || bytes.starts_with(b"%PDF-") {
            "document.pdf"
        } else {
            "file.bin"
        };

        Some((bytes, content_type, file_name.to_string()))
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
        let mut payload = json!({
            "chat_id": chat_id,
            "photo": photo,
        });
        if let Some(cap) = caption {
            payload["caption"] = json!(cap);
        }
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(ref rm) = reply_markup {
            payload["reply_markup"] = rm.clone();
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);

        match self.post_json("sendPhoto", payload).await {
            Ok(res) => Ok(res),
            Err(e) => {
                if photo.starts_with("http://") || photo.starts_with("https://") {
                    info!("sendPhoto direct URL failed ({e}); attempting download-and-upload...");
                    if let Some((bytes, _, _)) = self
                        .download_media_bytes(photo, MAX_TELEGRAM_DOWNLOAD_BYTES)
                        .await
                    {
                        return self
                            .send_photo_bytes(
                                chat_id,
                                bytes,
                                caption,
                                parse_mode,
                                reply_markup,
                                reply_to_message_id,
                            )
                            .await;
                    }
                }
                Err(e)
            }
        }
    }

    pub async fn send_media_group(
        &self,
        chat_id: i64,
        media: &[InputMedia],
        reply_to_message_id: Option<i64>,
    ) -> Result<Value, String> {
        if media.is_empty() {
            return Err("sendMediaGroup requires at least 1 media item".to_string());
        }

        let media_json = serde_json::to_value(media).map_err(|e| e.to_string())?;
        let mut payload = json!({
            "chat_id": chat_id,
            "media": media_json,
        });
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, false);

        match self.post_json("sendMediaGroup", payload).await {
            Ok(res) => Ok(res),
            Err(e) => {
                info!("sendMediaGroup direct URL failed ({e}); attempting multipart upload...");
                let mut form = Form::new().text("chat_id", chat_id.to_string());
                let delivery = Self::current_delivery_context();
                if let Some(thread_id) = delivery.message_thread_id {
                    form = form.text("message_thread_id", thread_id.to_string());
                }

                let mut updated_media = Vec::new();
                let mut attachments = Vec::new();

                for (idx, item) in media.iter().enumerate() {
                    let mut item_clone = item.clone();
                    let attach_key = format!("file_{idx}");
                    let target_url = match &item_clone {
                        InputMedia::Photo { media, .. } => media.clone(),
                        InputMedia::Video { media, .. } => media.clone(),
                        InputMedia::Audio { media, .. } => media.clone(),
                        InputMedia::Document { media, .. } => media.clone(),
                        InputMedia::Animation { media, .. } => media.clone(),
                        InputMedia::VoiceNote { media, .. } => media.clone(),
                    };

                    if target_url.starts_with("http://") || target_url.starts_with("https://") {
                        if let Some((bytes, mime, fname)) = self
                            .download_media_bytes(&target_url, MAX_TELEGRAM_DOWNLOAD_BYTES)
                            .await
                        {
                            match &mut item_clone {
                                InputMedia::Photo { media, .. }
                                | InputMedia::Video { media, .. }
                                | InputMedia::Audio { media, .. }
                                | InputMedia::Document { media, .. }
                                | InputMedia::Animation { media, .. }
                                | InputMedia::VoiceNote { media, .. } => {
                                    *media = format!("attach://{attach_key}");
                                }
                            }
                            attachments.push((attach_key, bytes, mime, fname));
                        }
                    }
                    updated_media.push(item_clone);
                }

                if attachments.is_empty() {
                    return Err(e);
                }

                let media_json_str =
                    serde_json::to_string(&updated_media).map_err(|e| e.to_string())?;
                form = form.text("media", media_json_str);

                for (attach_key, bytes, mime, fname) in attachments {
                    let part = Part::bytes(bytes)
                        .file_name(fname)
                        .mime_str(&mime)
                        .map_err(|e| e.to_string())?;
                    form = form.part(attach_key, part);
                }

                let url = format!("{}/sendMediaGroup", self.base_url);
                match self.client.post(&url).multipart(form).send().await {
                    Ok(resp) => {
                        let response = resp.json::<Value>().await.map_err(|e| {
                            format!(
                                "sendMediaGroup response decode error: {}",
                                reqwest_error_kind(&e)
                            )
                        })?;
                        if response.get("ok").and_then(Value::as_bool) == Some(true) {
                            Ok(response)
                        } else {
                            Err(Self::telegram_api_error("sendMediaGroup", &response))
                        }
                    }
                    Err(err) => Err(format!(
                        "sendMediaGroup multipart error: {}",
                        reqwest_error_kind(&err)
                    )),
                }
            }
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
        let mut payload = json!({
            "chat_id": chat_id,
            "audio": audio,
        });
        if let Some(cap) = caption {
            payload["caption"] = json!(cap);
        }
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(t) = title {
            payload["title"] = json!(t);
        }
        if let Some(p) = performer {
            payload["performer"] = json!(p);
        }
        if let Some(d) = duration {
            payload["duration"] = json!(d);
        }
        if let Some(ref rm) = reply_markup {
            payload["reply_markup"] = rm.clone();
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);

        match self.post_json("sendAudio", payload).await {
            Ok(res) => Ok(res),
            Err(e) => {
                if audio.starts_with("http://") || audio.starts_with("https://") {
                    info!("sendAudio direct URL failed ({e}); attempting download-and-upload...");
                    if let Some((bytes, mime, fname)) = self
                        .download_media_bytes(audio, MAX_TELEGRAM_DOWNLOAD_BYTES)
                        .await
                    {
                        let url = format!("{}/sendAudio", self.base_url);
                        let part = Part::bytes(bytes)
                            .file_name(fname)
                            .mime_str(&mime)
                            .map_err(|e| e.to_string())?;
                        let mut form = Form::new()
                            .text("chat_id", chat_id.to_string())
                            .part("audio", part);
                        if let Some(cap) = caption {
                            form = form.text("caption", cap.to_string());
                        }
                        if let Some(pm) = parse_mode {
                            form = form.text("parse_mode", pm.to_string());
                        }
                        if let Some(t) = title {
                            form = form.text("title", t.to_string());
                        }
                        if let Some(p) = performer {
                            form = form.text("performer", p.to_string());
                        }
                        if let Some(d) = duration {
                            form = form.text("duration", d.to_string());
                        }
                        if let Some(rm) = reply_markup {
                            form = form.text("reply_markup", rm.to_string());
                        }
                        if let Ok(resp) = self.client.post(&url).multipart(form).send().await {
                            if let Ok(response) = resp.json::<Value>().await {
                                if response.get("ok").and_then(Value::as_bool) == Some(true) {
                                    return Ok(response);
                                }
                            }
                        }
                    }
                }
                Err(e)
            }
        }
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
        let mut payload = json!({
            "chat_id": chat_id,
            "voice": voice,
        });
        if let Some(cap) = caption {
            payload["caption"] = json!(cap);
        }
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(d) = duration {
            payload["duration"] = json!(d);
        }
        if let Some(ref rm) = reply_markup {
            payload["reply_markup"] = rm.clone();
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);

        match self.post_json("sendVoice", payload).await {
            Ok(res) => Ok(res),
            Err(e) => {
                if voice.starts_with("http://") || voice.starts_with("https://") {
                    if let Some((bytes, mime, fname)) = self
                        .download_media_bytes(voice, MAX_TELEGRAM_DOWNLOAD_BYTES)
                        .await
                    {
                        let url = format!("{}/sendVoice", self.base_url);
                        let part = Part::bytes(bytes)
                            .file_name(fname)
                            .mime_str(&mime)
                            .map_err(|e| e.to_string())?;
                        let mut form = Form::new()
                            .text("chat_id", chat_id.to_string())
                            .part("voice", part);
                        if let Some(cap) = caption {
                            form = form.text("caption", cap.to_string());
                        }
                        if let Some(pm) = parse_mode {
                            form = form.text("parse_mode", pm.to_string());
                        }
                        if let Some(d) = duration {
                            form = form.text("duration", d.to_string());
                        }
                        if let Some(rm) = reply_markup {
                            form = form.text("reply_markup", rm.to_string());
                        }
                        if let Ok(resp) = self.client.post(&url).multipart(form).send().await {
                            if let Ok(response) = resp.json::<Value>().await {
                                if response.get("ok").and_then(Value::as_bool) == Some(true) {
                                    return Ok(response);
                                }
                            }
                        }
                    }
                }
                Err(e)
            }
        }
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
        let mut payload = json!({
            "chat_id": chat_id,
            "video": video,
        });
        if let Some(cap) = caption {
            payload["caption"] = json!(cap);
        }
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(ref rm) = reply_markup {
            payload["reply_markup"] = rm.clone();
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);

        match self.post_json("sendVideo", payload).await {
            Ok(res) => Ok(res),
            Err(e) => {
                if video.starts_with("http://") || video.starts_with("https://") {
                    if let Some((bytes, mime, fname)) = self
                        .download_media_bytes(video, MAX_TELEGRAM_DOWNLOAD_BYTES)
                        .await
                    {
                        let url = format!("{}/sendVideo", self.base_url);
                        let part = Part::bytes(bytes)
                            .file_name(fname)
                            .mime_str(&mime)
                            .map_err(|e| e.to_string())?;
                        let mut form = Form::new()
                            .text("chat_id", chat_id.to_string())
                            .part("video", part);
                        if let Some(cap) = caption {
                            form = form.text("caption", cap.to_string());
                        }
                        if let Some(pm) = parse_mode {
                            form = form.text("parse_mode", pm.to_string());
                        }
                        if let Some(rm) = reply_markup {
                            form = form.text("reply_markup", rm.to_string());
                        }
                        if let Ok(resp) = self.client.post(&url).multipart(form).send().await {
                            if let Ok(response) = resp.json::<Value>().await {
                                if response.get("ok").and_then(Value::as_bool) == Some(true) {
                                    return Ok(response);
                                }
                            }
                        }
                    }
                }
                Err(e)
            }
        }
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
        let mut payload = json!({
            "chat_id": chat_id,
            "animation": animation,
        });
        if let Some(cap) = caption {
            payload["caption"] = json!(cap);
        }
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(ref rm) = reply_markup {
            payload["reply_markup"] = rm.clone();
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);

        match self.post_json("sendAnimation", payload).await {
            Ok(res) => Ok(res),
            Err(e) => {
                if animation.starts_with("http://") || animation.starts_with("https://") {
                    if let Some((bytes, mime, fname)) = self
                        .download_media_bytes(animation, MAX_TELEGRAM_DOWNLOAD_BYTES)
                        .await
                    {
                        let url = format!("{}/sendAnimation", self.base_url);
                        let part = Part::bytes(bytes)
                            .file_name(fname)
                            .mime_str(&mime)
                            .map_err(|e| e.to_string())?;
                        let mut form = Form::new()
                            .text("chat_id", chat_id.to_string())
                            .part("animation", part);
                        if let Some(cap) = caption {
                            form = form.text("caption", cap.to_string());
                        }
                        if let Some(pm) = parse_mode {
                            form = form.text("parse_mode", pm.to_string());
                        }
                        if let Some(rm) = reply_markup {
                            form = form.text("reply_markup", rm.to_string());
                        }
                        if let Ok(resp) = self.client.post(&url).multipart(form).send().await {
                            if let Ok(response) = resp.json::<Value>().await {
                                if response.get("ok").and_then(Value::as_bool) == Some(true) {
                                    return Ok(response);
                                }
                            }
                        }
                    }
                }
                Err(e)
            }
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
        if let Some(ha) = horizontal_accuracy {
            payload["horizontal_accuracy"] = json!(ha);
        }
        if let Some(lp) = live_period {
            payload["live_period"] = json!(lp);
        }
        if let Some(rm) = reply_markup {
            payload["reply_markup"] = rm;
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);
        self.post_json("sendLocation", payload).await
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
        let mut payload = json!({
            "chat_id": chat_id,
            "document": document,
        });
        if let Some(cap) = caption {
            payload["caption"] = json!(cap);
        }
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(ref rm) = reply_markup {
            payload["reply_markup"] = rm.clone();
        }
        if let Some(rep) = reply_to_message_id {
            payload["reply_parameters"] =
                serde_json::to_value(ReplyParameters::new(rep)).unwrap_or(json!({}));
        }
        Self::apply_delivery_context(&mut payload, true);

        match self.post_json("sendDocument", payload).await {
            Ok(res) => Ok(res),
            Err(e) => {
                if document.starts_with("http://") || document.starts_with("https://") {
                    if let Some((bytes, mime, fname)) = self
                        .download_media_bytes(document, MAX_TELEGRAM_DOWNLOAD_BYTES)
                        .await
                    {
                        let url = format!("{}/sendDocument", self.base_url);
                        let part = Part::bytes(bytes)
                            .file_name(fname)
                            .mime_str(&mime)
                            .map_err(|e| e.to_string())?;
                        let mut form = Form::new()
                            .text("chat_id", chat_id.to_string())
                            .part("document", part);
                        if let Some(cap) = caption {
                            form = form.text("caption", cap.to_string());
                        }
                        if let Some(pm) = parse_mode {
                            form = form.text("parse_mode", pm.to_string());
                        }
                        if let Some(rm) = reply_markup {
                            form = form.text("reply_markup", rm.to_string());
                        }
                        if let Ok(resp) = self.client.post(&url).multipart(form).send().await {
                            if let Ok(response) = resp.json::<Value>().await {
                                if response.get("ok").and_then(Value::as_bool) == Some(true) {
                                    return Ok(response);
                                }
                            }
                        }
                    }
                }
                Err(e)
            }
        }
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
        let ephemeral_target = delivery
            .receiver_user_id
            .zip(delivery.source_ephemeral_message_id);
        let mut payload = json!({ "text": text });
        let method = if let (Some(cid), Some((receiver_user_id, ephemeral_message_id))) =
            (chat_id, ephemeral_target)
        {
            payload["chat_id"] = json!(cid);
            payload["receiver_user_id"] = json!(receiver_user_id);
            payload["ephemeral_message_id"] = json!(ephemeral_message_id);
            "editEphemeralMessageText"
        } else {
            if let Some(cid) = chat_id {
                payload["chat_id"] = json!(cid);
            }
            if let Some(mid) = message_id {
                payload["message_id"] = json!(mid);
            }
            "editMessageText"
        };
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(rm) = reply_markup {
            payload["reply_markup"] = rm;
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
        let rich_json = serde_json::to_value(rich_message).map_err(|e| e.to_string())?;
        let delivery = Self::current_delivery_context();
        let ephemeral_target = delivery
            .receiver_user_id
            .zip(delivery.source_ephemeral_message_id);
        let (method, mut payload) =
            if let Some((receiver_user_id, ephemeral_message_id)) = ephemeral_target {
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
                    json!({
                        "chat_id": chat_id,
                        "message_id": message_id,
                        "rich_message": rich_json,
                    }),
                )
            };
        if let Some(ref rm) = reply_markup {
            payload["reply_markup"] = rm.clone();
        }

        let res = self.post_json_raw(method, payload).await?;
        if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(res);
        }
        if res
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_ascii_lowercase().contains("message is not modified"))
            .unwrap_or(false)
        {
            return Ok(res);
        }

        // Fallback to editMessageText with HTML rendering
        let html_content = self.render_blocks_to_html(&rich_message.blocks);
        self.edit_message_text(
            Some(chat_id),
            Some(message_id),
            &html_content,
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
        let media_json = serde_json::to_value(media).map_err(|e| e.to_string())?;
        let mut payload = json!({
            "chat_id": chat_id,
            "receiver_user_id": receiver_user_id,
            "ephemeral_message_id": ephemeral_message_id,
            "media": media_json,
        });
        if let Some(rm) = reply_markup {
            payload["reply_markup"] = rm;
        }
        self.post_json("editEphemeralMessageMedia", payload).await
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
        if let Some(c) = caption {
            payload["caption"] = json!(c);
        }
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        if let Some(scam) = show_caption_above_media {
            payload["show_caption_above_media"] = json!(scam);
        }
        if let Some(rm) = reply_markup {
            payload["reply_markup"] = rm;
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
        if let Some(rm) = reply_markup {
            payload["reply_markup"] = rm;
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
            let rich_json = serde_json::to_value(rich_message).map_err(|e| e.to_string())?;
            let mut payload = json!({
                "chat_id": chat_id,
                "rich_message": rich_json,
            });
            if let Some(ref rm) = reply_markup {
                payload["reply_markup"] = rm.clone();
            }
            if let Some(recv) = receiver_user_id {
                payload["ephemeral_message_parameters"] =
                    serde_json::to_value(EphemeralMessageParameters {
                        receiver_user_id: recv,
                        callback_query_id: Self::current_delivery_context().callback_query_id,
                        replace_callback_query_message: None,
                    })
                    .unwrap_or(json!({}));
            }
            Self::apply_delivery_context(&mut payload, true);
            let res = self.post_json_raw("sendRichMessage", payload).await?;
            if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(res);
            } else {
                return Err(Self::telegram_api_error("sendRichMessage", &res));
            }
        }
        rich_message.validate()?;
        let rich_json = serde_json::to_string(rich_message).map_err(|e| e.to_string())?;

        let url = format!("{}/sendRichMessage", self.base_url);
        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .text("rich_message", rich_json);

        if let Some(rm) = reply_markup {
            form = form.text("reply_markup", rm.to_string());
        }
        let delivery = Self::current_delivery_context();
        if let Some(thread_id) = delivery.message_thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        let effective_receiver = receiver_user_id.or(delivery.receiver_user_id);
        if let Some(receiver_user_id) = effective_receiver {
            let ephemeral = serde_json::to_string(&EphemeralMessageParameters {
                receiver_user_id,
                callback_query_id: delivery.callback_query_id.clone(),
                replace_callback_query_message: None,
            })
            .map_err(|e| e.to_string())?;
            form = form.text("ephemeral_message_parameters", ephemeral);
        }
        if let Some(source_id) = delivery.source_ephemeral_message_id {
            let reply_params = serde_json::to_string(&ReplyParameters::ephemeral(source_id))
                .map_err(|e| e.to_string())?;
            form = form.text("reply_parameters", reply_params);
        }

        for (attach_name, bytes, mime) in attached_files {
            let part = Part::bytes(bytes)
                .file_name(attach_name.clone())
                .mime_str(&mime)
                .map_err(|e| e.to_string())?;
            form = form.part(attach_name, part);
        }

        match self.client.post(&url).multipart(form).send().await {
            Ok(resp) => {
                let response =
                    read_bounded_json_response(resp, MAX_TELEGRAM_RESPONSE_BYTES).await?;
                if response.get("ok").and_then(Value::as_bool) == Some(true) {
                    Ok(response)
                } else {
                    Err(Self::telegram_api_error("sendRichMessage", &response))
                }
            }
            Err(e) => Err(format!(
                "sendRichMessage multipart error: {}",
                reqwest_error_kind(&e)
            )),
        }
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
        let payload = json!({
            "chat_id": chat_id,
            "message_id": message_id,
        });
        self.post_json("deleteMessage", payload).await
    }

    pub async fn delete_ephemeral_message(
        &self,
        chat_id: i64,
        receiver_user_id: i64,
        ephemeral_message_id: i64,
    ) -> Result<Value, String> {
        let payload = json!({
            "chat_id": chat_id,
            "receiver_user_id": receiver_user_id,
            "ephemeral_message_id": ephemeral_message_id,
        });
        self.post_json("deleteEphemeralMessage", payload).await
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
        if let Some(t) = text {
            payload["text"] = json!(t);
        }
        self.post_json("answerCallbackQuery", payload).await
    }

    pub async fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<Value, String> {
        let mut payload = json!({
            "chat_id": chat_id,
            "action": action,
        });
        Self::apply_delivery_context(&mut payload, false);
        self.post_json("sendChatAction", payload).await
    }

    // ==========================================
    // Telegram Bot API 10.3: Rich Message & Draft Methods
    // ==========================================

    pub async fn send_rich_message_draft(
        &self,
        chat_id: i64,
        draft_id: i64,
        rich_message: &InputRichMessage,
        can_stop: bool,
        keep_on_stop: bool,
    ) -> Result<Value, String> {
        rich_message.validate()?;
        let rich_json = serde_json::to_value(rich_message).map_err(|e| e.to_string())?;
        let mut payload = json!({
            "chat_id": chat_id,
            "draft_id": draft_id,
            "rich_message": rich_json,
            "can_stop": can_stop,
            "keep_on_stop": keep_on_stop,
        });
        Self::apply_delivery_context(&mut payload, false);

        let res = self.post_json_raw("sendRichMessageDraft", payload).await?;
        let is_ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);

        if !is_ok {
            if res.get("error_code").and_then(|v| v.as_i64()) == Some(429) {
                return Ok(res);
            }
            // Fallback to sendMessageDraft
            let mut fallback_text = "Thinking...".to_string();
            if let Some(RichBlock::Thinking { text }) = rich_message.blocks.first() {
                fallback_text = text.as_str().unwrap_or("Thinking...").to_string();
            }
            return self
                .send_message_draft(
                    chat_id,
                    draft_id,
                    &fallback_text,
                    Some("HTML"),
                    can_stop,
                    keep_on_stop,
                )
                .await;
        }

        Ok(res)
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
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = json!(pm);
        }
        Self::apply_delivery_context(&mut payload, false);
        self.post_json("sendMessageDraft", payload).await
    }

    pub fn convert_remote_media_to_rich_links(
        &self,
        rich_message: &InputRichMessage,
    ) -> InputRichMessage {
        let mut converted = rich_message.clone();
        for block in &mut converted.blocks {
            match block {
                RichBlock::Photo { photo, caption } => {
                    let url = photo
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| photo.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Lihat Foto".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("🖼️ "),
                                json!({
                                    "type": "text_link",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Video { video, caption } => {
                    let url = video
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| video.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Tonton Video".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("🎬 "),
                                json!({
                                    "type": "text_link",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Audio { audio, caption } => {
                    let url = audio
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| audio.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Putar Audio".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("🎵 "),
                                json!({
                                    "type": "text_link",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Animation { animation, caption } => {
                    let url = animation
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| animation.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Animasi".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("🎞️ "),
                                json!({
                                    "type": "text_link",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Document { document, caption } => {
                    let url = document
                        .get("media")
                        .and_then(Value::as_str)
                        .or_else(|| document.as_str())
                        .unwrap_or("");
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let cap = caption
                            .as_ref()
                            .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "Dokumen".to_string());
                        *block = RichBlock::Paragraph {
                            text: Value::Array(vec![
                                json!("📄 "),
                                json!({
                                    "type": "text_link",
                                    "text": cap,
                                    "url": url,
                                }),
                            ]),
                        };
                    }
                }
                RichBlock::Collage {
                    blocks: items,
                    caption,
                } => {
                    let cap_str = caption
                        .as_ref()
                        .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "Galeri Foto".to_string());
                    let mut text_parts = vec![json!(format!("🖼️ [{cap_str}]: "))];
                    let mut count = 0;
                    for item in items.iter() {
                        let sub_url = item
                            .get("photo")
                            .and_then(|p| p.get("media"))
                            .and_then(Value::as_str)
                            .or_else(|| item.get("media").and_then(Value::as_str))
                            .unwrap_or("");
                        if !sub_url.is_empty() {
                            if count > 0 {
                                text_parts.push(json!(" • "));
                            }
                            count += 1;
                            text_parts.push(json!({
                                "type": "text_link",
                                "text": format!("Foto #{count}"),
                                "url": sub_url,
                            }));
                        }
                    }
                    *block = RichBlock::Paragraph {
                        text: Value::Array(text_parts),
                    };
                }
                RichBlock::Slideshow {
                    blocks: items,
                    caption,
                } => {
                    let cap_str = caption
                        .as_ref()
                        .map(|c| self.rich_caption_to_plain(&Some(c.clone())))
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "Slideshow".to_string());
                    let mut text_parts = vec![json!(format!("🖼️ [{cap_str}]: "))];
                    let mut count = 0;
                    for item in items.iter() {
                        let sub_url = item
                            .get("photo")
                            .and_then(|p| p.get("media"))
                            .and_then(Value::as_str)
                            .or_else(|| item.get("media").and_then(Value::as_str))
                            .unwrap_or("");
                        if !sub_url.is_empty() {
                            if count > 0 {
                                text_parts.push(json!(" • "));
                            }
                            count += 1;
                            text_parts.push(json!({
                                "type": "text_link",
                                "text": format!("Slide #{count}"),
                                "url": sub_url,
                            }));
                        }
                    }
                    *block = RichBlock::Paragraph {
                        text: Value::Array(text_parts),
                    };
                }
                _ => {}
            }
        }
        converted
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
            let rich_json = serde_json::to_value(rich_message).map_err(|e| e.to_string())?;
            let mut payload = json!({
                "chat_id": chat_id,
                "rich_message": rich_json,
            });
            if let Some(ref rm) = reply_markup {
                payload["reply_markup"] = rm.clone();
            }
            if let Some(recv) = receiver_user_id {
                payload["ephemeral_message_parameters"] =
                    serde_json::to_value(EphemeralMessageParameters {
                        receiver_user_id: recv,
                        callback_query_id: Self::current_delivery_context().callback_query_id,
                        replace_callback_query_message: None,
                    })
                    .unwrap_or(json!({}));
            }
            Self::apply_delivery_context(&mut payload, true);

            match self.post_json_raw("sendRichMessage", payload).await {
                Ok(res) if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) => {
                    return Ok(res);
                }
                Ok(res) => {
                    let desc = res
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    info!("Telegram rejected Rich Message ({desc}); checking zero-download link conversion.");
                }
                Err(error) => {
                    info!("Rich Message request failed ({error}); checking zero-download link conversion.");
                }
            }

            if rich_message.has_media() {
                let converted_msg = self.convert_remote_media_to_rich_links(rich_message);
                if let Ok(rich_json) = serde_json::to_value(&converted_msg) {
                    let mut retry_payload = json!({
                        "chat_id": chat_id,
                        "rich_message": rich_json,
                    });
                    if let Some(ref rm) = reply_markup {
                        retry_payload["reply_markup"] = rm.clone();
                    }
                    if let Some(recv) = receiver_user_id {
                        retry_payload["ephemeral_message_parameters"] =
                            serde_json::to_value(EphemeralMessageParameters {
                                receiver_user_id: recv,
                                callback_query_id: Self::current_delivery_context()
                                    .callback_query_id,
                                replace_callback_query_message: None,
                            })
                            .unwrap_or(json!({}));
                    }
                    Self::apply_delivery_context(&mut retry_payload, true);
                    match self.post_json_raw("sendRichMessage", retry_payload).await {
                        Ok(res) if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) => {
                            info!("Zero-download link conversion sendRichMessage succeeded seamlessly.");
                            return Ok(res);
                        }
                        Ok(res) => {
                            let desc = res
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown");
                            warn!("Zero-download sendRichMessage retry rejected ({desc}); degrading to safe HTML.");
                        }
                        Err(err) => {
                            warn!("Zero-download sendRichMessage retry request failed ({err}); degrading to safe HTML.");
                        }
                    }
                }
            }
        } else if let Err(error) = validation {
            if rich_message.blocks.is_empty() {
                return Err(error);
            }
            // Structural overflow is locally detected before network I/O. Block
            // ASTs can still be rendered deterministically through safer
            // representations rather than relying on Telegram rejection.
            info!("Rich Message validation required degradation: {error}");
        }

        let html_chunks = self.render_blocks_to_html_chunks(&rich_message.blocks, 3800);
        let total = html_chunks.len();
        let mut html_last = json!({ "ok": true });
        let mut html_failed = false;
        for (idx, chunk) in html_chunks.into_iter().enumerate() {
            let is_last = idx + 1 == total;
            match self
                .send_message(
                    chat_id,
                    &chunk,
                    Some("HTML"),
                    if is_last { reply_markup.clone() } else { None },
                    receiver_user_id,
                    None,
                )
                .await
            {
                Ok(response) => html_last = response,
                Err(error) => {
                    info!("HTML fallback failed ({error}); degrading to semantic plain text.");
                    html_failed = true;
                    break;
                }
            }
        }
        if !html_failed {
            return Ok(html_last);
        }

        let plain_chunks = self.render_blocks_to_plain_chunks(&rich_message.blocks, 4000);
        let total = plain_chunks.len();
        let mut plain_last = json!({ "ok": true });
        for (idx, chunk) in plain_chunks.into_iter().enumerate() {
            let is_last = idx + 1 == total;
            plain_last = self
                .send_message(
                    chat_id,
                    &chunk,
                    None,
                    if is_last { reply_markup.clone() } else { None },
                    receiver_user_id,
                    None,
                )
                .await?;
        }
        Ok(plain_last)
    }

    pub async fn set_my_commands(&self, commands: &[BotCommand]) -> Result<Value, String> {
        let cmds_json = serde_json::to_value(commands).unwrap_or(json!([]));
        let payload = json!({ "commands": cmds_json });
        self.post_json("setMyCommands", payload).await
    }

    // ==========================================
    // HTML Rendering Helpers & Chunking
    // ==========================================

    pub fn split_text_chunks(&self, text: &str, max_chunk_chars: usize) -> Vec<String> {
        if text.is_empty() || max_chunk_chars == 0 {
            return Vec::new();
        }

        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= max_chunk_chars {
            return vec![text.to_string()];
        }

        let mut chunks = Vec::new();
        let mut start = 0usize;
        while start < chars.len() {
            let hard_end = (start + max_chunk_chars).min(chars.len());
            let mut end = hard_end;

            if hard_end < chars.len() {
                // Prefer a natural boundary in the latter half of the chunk, but
                // always make progress even for a single huge token/code line.
                let soft_floor = start + (max_chunk_chars / 2);
                for idx in (soft_floor..hard_end).rev() {
                    if chars[idx] == '\n' {
                        end = idx + 1;
                        break;
                    }
                }
                if end == hard_end {
                    for idx in (soft_floor..hard_end).rev() {
                        if chars[idx].is_whitespace() {
                            end = idx + 1;
                            break;
                        }
                    }
                }
            }

            if end <= start {
                end = hard_end.max(start + 1);
            }
            chunks.push(chars[start..end].iter().collect());
            start = end;
        }

        chunks
    }

    pub fn render_blocks_to_html_chunks(
        &self,
        blocks: &[RichBlock],
        max_chars: usize,
    ) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current_text = String::new();
        let tag_clean_re = regex::Regex::new(r"</?[^>]+>").ok();

        for block in blocks {
            let b_html = self.render_single_block_html(block);
            if b_html.is_empty() {
                continue;
            }

            if b_html.chars().count() > max_chars {
                if !current_text.is_empty() {
                    chunks.push(current_text.trim().to_string());
                    current_text.clear();
                }

                // Never split raw Telegram HTML in the middle of a tag/entity.
                // Oversized single blocks degrade to escaped plain text chunks;
                // correctness is preferable to a parse-mode rejection.
                let stripped = tag_clean_re
                    .as_ref()
                    .map(|regex| regex.replace_all(&b_html, "").into_owned())
                    .unwrap_or_else(|| b_html.clone());
                let plain = html_escape::decode_html_entities(&stripped).into_owned();
                let mut escaped_chunk = String::new();
                let mut escaped_len = 0usize;
                for ch in plain.chars() {
                    let encoded = html_escape::encode_text(&ch.to_string()).into_owned();
                    let encoded_len = encoded.chars().count();
                    if !escaped_chunk.is_empty() && escaped_len + encoded_len > max_chars {
                        chunks.push(std::mem::take(&mut escaped_chunk));
                        escaped_len = 0;
                    }
                    escaped_chunk.push_str(&encoded);
                    escaped_len += encoded_len;
                }
                if !escaped_chunk.is_empty() {
                    chunks.push(escaped_chunk);
                }
                continue;
            }

            if current_text.chars().count() + b_html.chars().count() + 2 > max_chars
                && !current_text.is_empty()
            {
                chunks.push(current_text.trim().to_string());
                current_text.clear();
            }

            if !current_text.is_empty() {
                current_text.push('\n');
            }
            current_text.push_str(&b_html);
        }

        if !current_text.trim().is_empty() {
            chunks.push(current_text.trim().to_string());
        }

        if chunks.is_empty() {
            vec!["".to_string()]
        } else {
            chunks
        }
    }

    pub fn render_blocks_to_plain_chunks(
        &self,
        blocks: &[RichBlock],
        max_chars: usize,
    ) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current = String::new();
        for block in blocks {
            let rendered = self.render_single_block_plain(block);
            if rendered.trim().is_empty() {
                continue;
            }
            if rendered.chars().count() > max_chars {
                if !current.trim().is_empty() {
                    chunks.push(std::mem::take(&mut current));
                }
                chunks.extend(self.split_text_chunks(&rendered, max_chars));
                continue;
            }
            if !current.is_empty()
                && current.chars().count() + rendered.chars().count() + 1 > max_chars
            {
                chunks.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(rendered.trim());
        }
        if !current.trim().is_empty() {
            chunks.push(current);
        }
        if chunks.is_empty() {
            vec![String::new()]
        } else {
            chunks
        }
    }

    fn render_single_block_plain(&self, block: &RichBlock) -> String {
        match block {
            RichBlock::Paragraph { text }
            | RichBlock::SectionHeading { text, .. }
            | RichBlock::Thinking { text } => self.rich_value_to_plain(text),
            RichBlock::Footer { text } => format!("— {}", self.rich_value_to_plain(text)),
            RichBlock::Preformatted { text, .. } => text.clone(),
            RichBlock::List { items } => items
                .iter()
                .map(|item| {
                    let marker = item
                        .value
                        .map(|value| format!("{value}."))
                        .or_else(|| item.kind.as_deref().map(|_| "1.".to_string()))
                        .unwrap_or_else(|| "•".to_string());
                    let body = item
                        .blocks
                        .iter()
                        .map(|value| self.rich_value_to_plain(value))
                        .collect::<Vec<_>>()
                        .join("");
                    format!("{marker} {body}")
                })
                .collect::<Vec<_>>()
                .join(
                    "
",
                ),
            RichBlock::BlockQuotation { blocks } => blocks
                .iter()
                .map(|value| self.rich_value_to_plain(value))
                .collect::<Vec<_>>()
                .join(
                    "
",
                ),
            RichBlock::ExpandableBlockQuotation { text, credit }
            | RichBlock::PullQuotation { text, credit } => {
                let mut output = self.rich_value_to_plain(text);
                if let Some(credit) = credit {
                    let credit = self.rich_value_to_plain(credit);
                    if !credit.is_empty() {
                        output.push_str(
                            "
— ",
                        );
                        output.push_str(&credit);
                    }
                }
                output
            }
            RichBlock::Divider {} => "────────────────────────".to_string(),
            RichBlock::MathematicalExpression { expression } => expression.clone(),
            RichBlock::Table {
                cells,
                has_header,
                caption,
                ..
            } => {
                let ascii = self.render_table_to_ascii(cells, *has_header);
                if let Some(cap) = caption {
                    if !ascii.is_empty() {
                        format!("{cap}\n{ascii}")
                    } else {
                        cap.clone()
                    }
                } else {
                    ascii
                }
            }
            RichBlock::Buttons { buttons, .. } => buttons
                .iter()
                .map(|button| self.rich_value_to_plain(&button.text))
                .collect::<Vec<_>>()
                .join(" | "),
            RichBlock::Document { document, caption } => {
                let name = caption
                    .as_ref()
                    .map(|cap| self.rich_value_to_plain(&cap.text))
                    .filter(|val| !val.trim().is_empty())
                    .unwrap_or_else(|| {
                        document
                            .get("media")
                            .and_then(Value::as_str)
                            .map(|m| m.strip_prefix("tg://document?id=").unwrap_or(m))
                            .unwrap_or("document")
                            .to_string()
                    });
                format!("📄 {name}")
            }
            RichBlock::Photo { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Photo]".to_string()
                } else {
                    format!("[Photo] {cap}")
                }
            }
            RichBlock::Video { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Video]".to_string()
                } else {
                    format!("[Video] {cap}")
                }
            }
            RichBlock::Audio { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Audio]".to_string()
                } else {
                    format!("[Audio] {cap}")
                }
            }
            RichBlock::VoiceNote { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Voice Note]".to_string()
                } else {
                    format!("[Voice Note] {cap}")
                }
            }
            RichBlock::Animation { caption, .. } => {
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    "[Animation]".to_string()
                } else {
                    format!("[Animation] {cap}")
                }
            }
            RichBlock::Collage { blocks, caption } => {
                let items = blocks
                    .iter()
                    .map(|value| self.rich_value_to_plain(value))
                    .collect::<Vec<_>>()
                    .join(" ");
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    format!("[Collage: {items}]")
                } else {
                    format!(
                        "[Collage: {items}]
{cap}"
                    )
                }
            }
            RichBlock::Slideshow { blocks, caption } => {
                let items = blocks
                    .iter()
                    .map(|value| self.rich_value_to_plain(value))
                    .collect::<Vec<_>>()
                    .join(" ");
                let cap = self.rich_caption_to_plain(caption);
                if cap.is_empty() {
                    format!("[Slideshow: {items}]")
                } else {
                    format!(
                        "[Slideshow: {items}]
{cap}"
                    )
                }
            }
            RichBlock::Map { location, zoom, .. } => {
                let zoom_str = zoom.map(|z| format!(" zoom={z}")).unwrap_or_default();
                format!(
                    "[Map: lat={}, lon={}{}]",
                    location.latitude, location.longitude, zoom_str
                )
            }
            RichBlock::Details {
                summary, blocks, ..
            } => {
                let body = blocks
                    .iter()
                    .map(|value| self.rich_value_to_plain(value))
                    .collect::<Vec<_>>()
                    .join(
                        "
",
                    );
                format!(
                    "{}
{}",
                    self.rich_value_to_plain(summary),
                    body
                )
                .trim()
                .to_string()
            }
            RichBlock::Anchor { .. } => String::new(),
        }
    }

    pub fn render_blocks_to_html(&self, blocks: &[RichBlock]) -> String {
        let mut lines = Vec::new();
        for block in blocks {
            let h = self.render_single_block_html(block);
            if !h.is_empty() {
                lines.push(h);
            }
        }
        lines.join("\n").trim().to_string()
    }

    fn render_single_block_html(&self, block: &RichBlock) -> String {
        match block {
            RichBlock::Paragraph { text } => {
                let inner = self.rich_value_to_html(text);
                format!(
                    "{inner}
"
                )
            }
            RichBlock::Footer { text } => {
                let inner = self.rich_value_to_html(text);
                format!(
                    "
— <i>{inner}</i>
"
                )
            }
            RichBlock::SectionHeading { text, .. } => {
                let inner = self.rich_value_to_html(text);
                format!(
                    "
<b>{inner}</b>
"
                )
            }
            RichBlock::Preformatted { text, language } => {
                let lang_attr = language
                    .as_ref()
                    .map(|language| html_escape::encode_double_quoted_attribute(language))
                    .map(|language| format!(" class=\"language-{language}\""))
                    .unwrap_or_default();
                let esc = html_escape::encode_text(text);
                format!(
                    "<pre{lang_attr}>{esc}</pre>
"
                )
            }
            RichBlock::List { items } => {
                let mut list_lines = Vec::new();
                for item in items {
                    let item_str = item
                        .blocks
                        .iter()
                        .map(|block| self.rich_value_to_html(block))
                        .collect::<Vec<_>>()
                        .join("");
                    let marker = item
                        .value
                        .map(|value| format!("{value}."))
                        .or_else(|| item.kind.as_deref().map(|_| "1.".to_string()))
                        .unwrap_or_else(|| "•".to_string());
                    list_lines.push(format!("{marker} {item_str}"));
                }
                list_lines.join(
                    "
",
                )
            }
            RichBlock::BlockQuotation { blocks } => {
                let mut q_text = String::new();
                for b in blocks {
                    q_text.push_str(&self.rich_value_to_html(b));
                }
                format!(
                    "<blockquote>{q_text}</blockquote>
"
                )
            }
            RichBlock::ExpandableBlockQuotation { text, credit }
            | RichBlock::PullQuotation { text, credit } => {
                let q_text = self.rich_value_to_html(text);
                let credit_html = credit
                    .as_ref()
                    .map(|value| self.rich_value_to_html(value))
                    .filter(|value| !value.is_empty())
                    .map(|value| format!("<cite>{value}</cite>"))
                    .unwrap_or_default();
                format!(
                    "<blockquote expandable>{q_text}{credit_html}</blockquote>
"
                )
            }
            RichBlock::Divider {} => "────────────────────────
"
            .to_string(),
            RichBlock::MathematicalExpression { expression } => {
                let esc = html_escape::encode_text(expression);
                format!(
                    "<code>{esc}</code>
"
                )
            }
            RichBlock::Table {
                cells,
                has_header,
                caption,
                ..
            } => {
                let ascii_tbl = self.render_table_to_ascii(cells, *has_header);
                if !ascii_tbl.is_empty() {
                    if let Some(cap) = caption {
                        let safe_cap = html_escape::encode_text(cap);
                        format!(
                            "
<b>{safe_cap}</b>
{ascii_tbl}
"
                        )
                    } else {
                        format!(
                            "
{ascii_tbl}
"
                        )
                    }
                } else {
                    String::new()
                }
            }
            RichBlock::Buttons { buttons, .. } => buttons
                .iter()
                .map(|button| format!("[{}]", self.rich_value_to_html(&button.text)))
                .collect::<Vec<_>>()
                .join(" "),
            RichBlock::Document { document, caption } => {
                let name = caption
                    .as_ref()
                    .map(|cap| self.rich_value_to_html(&cap.text))
                    .filter(|val| !val.trim().is_empty())
                    .unwrap_or_else(|| {
                        let m = document
                            .get("media")
                            .and_then(Value::as_str)
                            .map(|m| m.strip_prefix("tg://document?id=").unwrap_or(m))
                            .unwrap_or("document");
                        html_escape::encode_text(m).to_string()
                    });
                let media = document
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| document.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    format!("📄 <b><a href=\"{safe_url}\">{name}</a></b>\n")
                } else {
                    format!("📄 <b>{name}</b>\n")
                }
            }
            RichBlock::Details {
                summary, blocks, ..
            } => {
                let summary_html = self.rich_value_to_html(summary);
                let body_html = blocks
                    .iter()
                    .map(|block| self.rich_value_to_html(block))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("<blockquote expandable><b>{summary_html}</b>\n{body_html}</blockquote>\n")
            }
            RichBlock::Thinking { text } => {
                let t_esc = self.rich_value_to_html(text);
                format!("<blockquote expandable>💭 <b>Thinking:</b>\n{t_esc}</blockquote>\n")
            }
            RichBlock::Photo { photo, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let media = photo
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| photo.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Lihat Foto".to_string()
                    } else {
                        cap
                    };
                    format!("🖼️ <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🖼️ <b>[Photo]</b>\n".to_string()
                } else {
                    format!("🖼️ <b>[Photo]</b>\n{cap}\n")
                }
            }
            RichBlock::Video { video, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let media = video
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| video.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Lihat Video".to_string()
                    } else {
                        cap
                    };
                    format!("🎥 <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🎥 <b>[Video]</b>\n".to_string()
                } else {
                    format!("🎥 <b>[Video]</b>\n{cap}\n")
                }
            }
            RichBlock::Audio { audio, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let media = audio
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| audio.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Dengarkan Audio".to_string()
                    } else {
                        cap
                    };
                    format!("🎵 <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🎵 <b>[Audio]</b>\n".to_string()
                } else {
                    format!("🎵 <b>[Audio]</b>\n{cap}\n")
                }
            }
            RichBlock::VoiceNote {
                voice_note,
                caption,
            } => {
                let cap = self.rich_caption_to_html(caption);
                let media = voice_note
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| voice_note.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Pesan Suara".to_string()
                    } else {
                        cap
                    };
                    format!("🎤 <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🎤 <b>[Voice Note]</b>\n".to_string()
                } else {
                    format!("🎤 <b>[Voice Note]</b>\n{cap}\n")
                }
            }
            RichBlock::Animation { animation, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let media = animation
                    .get("media")
                    .and_then(Value::as_str)
                    .or_else(|| animation.as_str())
                    .unwrap_or_default();
                if !media.is_empty() {
                    let safe_url = html_escape::encode_double_quoted_attribute(media);
                    let label = if cap.is_empty() {
                        "Lihat Animasi".to_string()
                    } else {
                        cap
                    };
                    format!("🎞️ <b><a href=\"{safe_url}\">{label}</a></b>\n")
                } else if cap.is_empty() {
                    "🎞️ <b>[Animation]</b>\n".to_string()
                } else {
                    format!("🎞️ <b>[Animation]</b>\n{cap}\n")
                }
            }
            RichBlock::Collage { blocks, caption } | RichBlock::Slideshow { blocks, caption } => {
                let cap = self.rich_caption_to_html(caption);
                let label = if cap.is_empty() {
                    "Galeri Foto".to_string()
                } else {
                    cap
                };
                let mut links = Vec::new();
                for (i, b) in blocks.iter().enumerate() {
                    let url = b
                        .get("photo")
                        .and_then(|p| p.get("media"))
                        .and_then(Value::as_str)
                        .or_else(|| b.get("media").and_then(Value::as_str))
                        .or_else(|| b.as_str())
                        .unwrap_or_default();
                    if !url.is_empty() {
                        let safe_url = html_escape::encode_double_quoted_attribute(url);
                        links.push(format!("<a href=\"{safe_url}\">Foto #{}</a>", i + 1));
                    }
                }
                if links.is_empty() {
                    format!("🖼️ <b>{label}</b>\n")
                } else {
                    format!("🖼️ <b>{label}</b>: {}\n", links.join(" • "))
                }
            }
            RichBlock::Map { location, zoom, .. } => {
                let zoom_str = zoom.map(|z| format!("?z={z}")).unwrap_or_default();
                let url = format!(
                    "https://www.google.com/maps?q={},{}",
                    location.latitude, location.longitude
                );
                let safe_url = html_escape::encode_double_quoted_attribute(&url);
                format!(
                    "📍 <b><a href=\"{safe_url}\">Lokasi Peta ({}, {}{})</a></b>\n",
                    location.latitude, location.longitude, zoom_str
                )
            }
            RichBlock::Anchor { .. } => String::new(),
        }
    }

    fn rich_caption_to_plain(&self, caption: &Option<RichBlockCaption>) -> String {
        let Some(caption) = caption else {
            return String::new();
        };
        let mut text = self.rich_value_to_plain(&caption.text);
        if let Some(credit) = &caption.credit {
            let credit_text = self.rich_value_to_plain(credit);
            if !credit_text.is_empty() {
                text.push_str(" — ");
                text.push_str(&credit_text);
            }
        }
        text
    }

    fn rich_caption_to_html(&self, caption: &Option<RichBlockCaption>) -> String {
        let Some(caption) = caption else {
            return String::new();
        };
        let mut text = self.rich_value_to_html(&caption.text);
        if let Some(credit) = &caption.credit {
            let credit_text = self.rich_value_to_html(credit);
            if !credit_text.is_empty() {
                text.push_str("<cite>");
                text.push_str(&credit_text);
                text.push_str("</cite>");
            }
        }
        text
    }

    fn rich_value_to_html(&self, v: &Value) -> String {
        match v {
            Value::String(s) => html_escape::encode_text(s).into_owned(),
            Value::Array(arr) => arr
                .iter()
                .map(|item| self.rich_value_to_html(item))
                .collect(),
            Value::Object(obj) => {
                let t = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");
                let inner = obj
                    .get("text")
                    .map(|sub| self.rich_value_to_html(sub))
                    .unwrap_or_default();
                match t {
                    "bold" => format!("<b>{inner}</b>"),
                    "italic" => format!("<i>{inner}</i>"),
                    "code" => format!("<code>{inner}</code>"),
                    "url" => {
                        let url = obj.get("url").and_then(|s| s.as_str()).unwrap_or("#");
                        let safe_url = html_escape::encode_double_quoted_attribute(url);
                        format!("<a href=\"{safe_url}\">{inner}</a>")
                    }
                    "spoiler" => format!("<tg-spoiler>{inner}</tg-spoiler>"),
                    "strikethrough" | "strike" => format!("<s>{inner}</s>"),
                    "underline" => format!("<u>{inner}</u>"),
                    "paragraph" => inner,
                    _ => inner,
                }
            }
            _ => v.to_string(),
        }
    }

    fn rich_value_to_plain(&self, v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr
                .iter()
                .map(|item| self.rich_value_to_plain(item))
                .collect(),
            Value::Object(obj) => {
                if let Some(text) = obj.get("text") {
                    self.rich_value_to_plain(text)
                } else if let Some(expr) = obj.get("expression") {
                    self.rich_value_to_plain(expr)
                } else {
                    String::new()
                }
            }
            _ => v.to_string(),
        }
    }

    fn char_display_width(c: char) -> usize {
        match c {
            '\u{1100}'..='\u{115F}'
            | '\u{2329}'
            | '\u{232A}'
            | '\u{2E80}'..='\u{303E}'
            | '\u{3040}'..='\u{A4CF}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE10}'..='\u{FE19}'
            | '\u{FE30}'..='\u{FE6F}'
            | '\u{FF00}'..='\u{FF60}'
            | '\u{FFE0}'..='\u{FFE6}'
            | '\u{1F300}'..='\u{1FAFF}'
            | '\u{2600}'..='\u{27BF}' => 2,
            _ => 1,
        }
    }

    fn str_display_width(s: &str) -> usize {
        s.chars().map(Self::char_display_width).sum()
    }

    fn truncate_display_width(s: &str, max_width: usize) -> String {
        if Self::str_display_width(s) <= max_width {
            return s.to_string();
        }
        if max_width <= 1 {
            return "…".to_string();
        }
        let mut out = String::new();
        let mut width = 0;
        for c in s.chars() {
            let c_width = Self::char_display_width(c);
            if width + c_width > max_width - 1 {
                break;
            }
            out.push(c);
            width += c_width;
        }
        out.push('…');
        out
    }

    fn render_table_to_ascii(&self, rows: &[Vec<RichBlockTableCell>], has_header: bool) -> String {
        if rows.is_empty() {
            return String::new();
        }

        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return String::new();
        }

        let tag_clean_re = regex::Regex::new(r"</?[^>]+>").ok();
        let mut norm_rows: Vec<Vec<(String, String)>> = Vec::new();
        for r in rows {
            let mut row_cells: Vec<(String, String)> = r
                .iter()
                .map(|c| {
                    let plain = self.rich_value_to_plain(&c.text);
                    let cleaned = tag_clean_re
                        .as_ref()
                        .map(|regex| regex.replace_all(&plain, "").into_owned())
                        .unwrap_or(plain);
                    let decoded = html_escape::decode_html_entities(&cleaned)
                        .replace(['\n', '\r', '\t'], " ")
                        .to_string();
                    let align = c.align.clone().unwrap_or_else(|| "left".to_string());
                    (decoded, align)
                })
                .collect();
            while row_cells.len() < num_cols {
                row_cells.push((String::new(), "left".to_string()));
            }
            norm_rows.push(row_cells);
        }

        let mut col_widths = vec![2usize; num_cols];
        for r in &norm_rows {
            for (i, (c, _)) in r.iter().enumerate() {
                col_widths[i] = col_widths[i].max(Self::str_display_width(c));
            }
        }

        const MAX_TABLE_WIDTH: usize = 64;
        let border_overhead = num_cols + 1 + (num_cols * 2);
        let available = MAX_TABLE_WIDTH.saturating_sub(border_overhead);
        let current: usize = col_widths.iter().sum();
        if current > available && available >= num_cols {
            let min_width = 3usize;
            let mut excess = current.saturating_sub(available);
            while excess > 0 {
                let mut changed = false;
                for width in &mut col_widths {
                    if excess == 0 {
                        break;
                    }
                    if *width > min_width {
                        *width -= 1;
                        excess -= 1;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
        }

        let separator = format!(
            "|-{}-|",
            col_widths
                .iter()
                .map(|w| "-".repeat(*w + 2))
                .collect::<Vec<_>>()
                .join("-|-")
        );

        let mut table_lines = Vec::new();
        for (idx, r) in norm_rows.iter().enumerate() {
            let mut line_cells = Vec::new();
            for i in 0..num_cols {
                let (cell_txt, align) = &r[i];
                let display_txt = Self::truncate_display_width(cell_txt, col_widths[i]);
                let width = Self::str_display_width(&display_txt);
                let total_pad = col_widths[i].saturating_sub(width);
                let esc_txt = html_escape::encode_text(&display_txt);

                let padded = match align.as_str() {
                    "center" => {
                        let l_pad = total_pad / 2;
                        let r_pad = total_pad - l_pad;
                        format!(" {}{}{} ", " ".repeat(l_pad), esc_txt, " ".repeat(r_pad))
                    }
                    "right" => {
                        format!(" {}{} ", " ".repeat(total_pad), esc_txt)
                    }
                    _ => {
                        format!(" {}{} ", esc_txt, " ".repeat(total_pad))
                    }
                };
                line_cells.push(padded);
            }
            table_lines.push(format!("|{}|", line_cells.join("|")));
            if idx == 0 && has_header && norm_rows.len() > 1 {
                table_lines.push(separator.clone());
            }
        }
        if !has_header {
            table_lines.insert(1.min(table_lines.len()), separator);
        }

        format!("<pre><code>{}</code></pre>", table_lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::models::Location;

    #[test]
    fn split_text_chunks_preserves_unicode_and_bounds() {
        let client = TelegramBotClient::new("test-token");
        let input = format!("{}\n{}", "😊世界".repeat(900), "x".repeat(5000));
        let chunks = client.split_text_chunks(&input, 3800);

        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 3800));
        assert_eq!(chunks.concat(), input);
    }

    #[test]
    fn oversized_rich_block_fallback_never_splits_html_entities() {
        let client = TelegramBotClient::new("test-token");
        let blocks = vec![RichBlock::Paragraph {
            text: Value::String("<&😊>".repeat(2000)),
        }];
        let chunks = client.render_blocks_to_html_chunks(&blocks, 256);

        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 256));
        assert!(chunks.iter().all(|chunk| {
            let amp = chunk.matches('&').count();
            amp == chunk.matches("&lt;").count()
                + chunk.matches("&gt;").count()
                + chunk.matches("&amp;").count()
                + chunk.matches("&quot;").count()
                + chunk.matches("&#x27;").count()
        }));
    }

    #[test]
    fn rich_media_blocks_render_to_plain_and_html_fallback() {
        let client = TelegramBotClient::new("test-token");
        let blocks = vec![
            RichBlock::Photo {
                photo: serde_json::json!({"type": "photo", "media": "photo_1"}),
                caption: Some(RichBlockCaption::new(Value::String(
                    "Foto sunset".to_string(),
                ))),
            },
            RichBlock::Video {
                video: serde_json::json!({"type": "video", "media": "video_1"}),
                caption: Some(RichBlockCaption::new(Value::String(
                    "Video clip".to_string(),
                ))),
            },
            RichBlock::Audio {
                audio: serde_json::json!({"type": "audio", "media": "audio_1"}),
                caption: None,
            },
            RichBlock::VoiceNote {
                voice_note: serde_json::json!({"type": "voice", "media": "voice_1"}),
                caption: None,
            },
            RichBlock::Animation {
                animation: serde_json::json!({"type": "animation", "media": "anim_1"}),
                caption: None,
            },
            RichBlock::Map {
                location: Location {
                    latitude: -5.14,
                    longitude: 119.43,
                    horizontal_accuracy: None,
                },
                zoom: Some(12),
                width: None,
                height: None,
            },
        ];
        let plain = client
            .render_blocks_to_plain_chunks(&blocks, 4000)
            .join("\n");
        assert!(plain.contains("[Photo] Foto sunset"));
        assert!(plain.contains("[Video] Video clip"));
        assert!(plain.contains("[Audio]"));
        assert!(plain.contains("[Voice Note]"));
        assert!(plain.contains("[Animation]"));
        assert!(plain.contains("[Map: lat=-5.14, lon=119.43 zoom=12]"));

        let html = client.render_blocks_to_html(&blocks);
        assert!(html.contains("🖼️ <b><a href=\"photo_1\">Foto sunset</a></b>"));
        assert!(html.contains("🎥 <b><a href=\"video_1\">Video clip</a></b>"));
        assert!(html.contains("🎵 <b><a href=\"audio_1\">Dengarkan Audio</a></b>"));
        assert!(html.contains("🎤 <b><a href=\"voice_1\">Pesan Suara</a></b>"));
        assert!(html.contains("🎞️ <b><a href=\"anim_1\">Lihat Animasi</a></b>"));
        assert!(html.contains("📍 <b><a href=\"https://www.google.com/maps?q=-5.14,119.43\">Lokasi Peta (-5.14, 119.43?z=12)</a></b>"));
    }

    #[test]
    fn semantic_plain_fallback_is_rendered_from_ast_not_raw_markdown() {
        let client = TelegramBotClient::new("test-token");
        let source = "### Heading\n\n**bold** and `code`\n\n---\n\n[link](https://example.com)";
        let blocks = crate::parser::parse_markdown_to_rich_blocks(source);
        let plain = client
            .render_blocks_to_plain_chunks(&blocks, 4000)
            .join("\n");
        assert!(plain.contains("Heading"));
        assert!(plain.contains("bold"));
        assert!(plain.contains("code"));
        assert!(plain.contains("link"));
        assert!(!plain.contains("###"));
        assert!(!plain.contains("**"));
        assert!(!plain.contains('`'));
        assert!(!plain.contains("]("));
    }

    #[test]
    fn convert_remote_media_to_rich_links_transforms_remote_blocks_without_local_download() {
        let client = TelegramBotClient::new("test-token");
        let blocks = vec![
            RichBlock::Photo {
                photo: serde_json::json!({"type": "photo", "media": "https://example.com/cat.jpg"}),
                caption: Some(RichBlockCaption::new(Value::String(
                    "Kucing Manis".to_string(),
                ))),
            },
            RichBlock::Video {
                video: serde_json::json!({"type": "video", "media": "https://example.com/movie.mp4"}),
                caption: None,
            },
            RichBlock::Collage {
                blocks: vec![
                    serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/p1.jpg"}}),
                    serde_json::json!({"type": "photo", "photo": {"type": "photo", "media": "https://example.com/p2.jpg"}}),
                ],
                caption: Some(RichBlockCaption::new(Value::String("Dua Foto".to_string()))),
            },
            RichBlock::Paragraph {
                text: Value::String("Teks biasa tetap utuh".to_string()),
            },
        ];
        let msg = InputRichMessage::new(blocks);
        let converted = client.convert_remote_media_to_rich_links(&msg);
        assert_eq!(converted.blocks.len(), 4);
        assert!(matches!(converted.blocks[0], RichBlock::Paragraph { .. }));
        assert!(matches!(converted.blocks[1], RichBlock::Paragraph { .. }));
        assert!(matches!(converted.blocks[2], RichBlock::Paragraph { .. }));
        assert!(matches!(converted.blocks[3], RichBlock::Paragraph { .. }));

        let s0 = serde_json::to_string(&converted.blocks[0]).unwrap();
        assert!(
            s0.contains("🖼️")
                && s0.contains("Kucing Manis")
                && s0.contains("https://example.com/cat.jpg")
        );

        let s1 = serde_json::to_string(&converted.blocks[1]).unwrap();
        assert!(
            s1.contains("🎬")
                && s1.contains("Tonton Video")
                && s1.contains("https://example.com/movie.mp4")
        );

        let s2 = serde_json::to_string(&converted.blocks[2]).unwrap();
        assert!(s2.contains("🖼️ [Dua Foto]:") && s2.contains("Foto #1") && s2.contains("Foto #2"));
    }
}
