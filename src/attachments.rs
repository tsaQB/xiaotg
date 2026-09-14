use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

const MAX_PERSISTED_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentRef {
    pub kind: String,
    pub mime_type: String,
    pub name: Option<String>,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedUserContent {
    pub xiao_content_version: u8,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
}

fn attachment_root() -> PathBuf {
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join(".local/share/xiaoai/attachments")
}

fn scope_dir(chat_id: i64, thread_id: i64) -> PathBuf {
    attachment_root()
        .join(chat_id.to_string())
        .join(thread_id.to_string())
}

pub async fn persist_attachment(
    chat_id: i64,
    thread_id: i64,
    kind: &str,
    mime_type: &str,
    name: Option<&str>,
    bytes: &[u8],
) -> Result<AttachmentRef, String> {
    if bytes.is_empty() {
        return Err("attachment is empty".to_string());
    }
    if bytes.len() > MAX_PERSISTED_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment exceeds persistence limit ({} bytes)",
            bytes.len()
        ));
    }

    let random: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(char::from)
        .collect();
    let extension = extension_for_mime(mime_type);
    let file_name = format!("{random}.{extension}");
    let dir = scope_dir(chat_id, thread_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| format!("create attachment directory: {err}"))?;
    harden_dir_permissions(&dir).await?;
    let path = dir.join(&file_name);
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|err| format!("persist attachment: {err}"))?;
    harden_file_permissions(&path).await?;

    Ok(AttachmentRef {
        kind: kind.to_string(),
        mime_type: mime_type.to_string(),
        name: name.map(str::to_string),
        file_name,
    })
}

#[cfg(unix)]
async fn harden_dir_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .await
        .map_err(|err| format!("secure attachment directory: {err}"))
}

#[cfg(not(unix))]
async fn harden_dir_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
async fn harden_file_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .await
        .map_err(|err| format!("secure attachment file: {err}"))
}

#[cfg(not(unix))]
async fn harden_file_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

pub async fn load_attachment(
    chat_id: i64,
    thread_id: i64,
    attachment: &AttachmentRef,
) -> Result<Vec<u8>, String> {
    if attachment.file_name.contains('/')
        || attachment.file_name.contains('\\')
        || attachment.file_name.contains("..")
    {
        return Err("invalid attachment filename".to_string());
    }
    let path = scope_dir(chat_id, thread_id).join(&attachment.file_name);
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|err| format!("attachment metadata: {err}"))?;
    if metadata.len() as usize > MAX_PERSISTED_ATTACHMENT_BYTES {
        return Err("persisted attachment exceeds safety limit".to_string());
    }
    tokio::fs::read(path)
        .await
        .map_err(|err| format!("read attachment: {err}"))
}

pub async fn delete_scoped_attachments(chat_id: i64, thread_id: i64) {
    let _ = tokio::fs::remove_dir_all(scope_dir(chat_id, thread_id)).await;
}

#[allow(dead_code)]
pub async fn delete_session_attachments(user_id: i64, session_id: usize) {
    let base = attachment_root()
        .join(user_id.to_string())
        .join(session_id.to_string());
    let _ = tokio::fs::remove_dir_all(base).await;
}

#[allow(dead_code)]
pub async fn delete_attachment_refs(
    user_id: i64,
    session_id: usize,
    attachments: &[AttachmentRef],
) {
    let dir = scope_dir(user_id, session_id as i64);
    for attachment in attachments {
        if attachment.file_name.contains('/')
            || attachment.file_name.contains('\\')
            || attachment.file_name.contains("..")
        {
            continue;
        }
        let _ = tokio::fs::remove_file(dir.join(&attachment.file_name)).await;
    }
}

pub fn encode_user_content(text: &str, attachments: Vec<AttachmentRef>) -> Value {
    if attachments.is_empty() {
        return Value::String(text.to_string());
    }
    serde_json::to_value(PersistedUserContent {
        xiao_content_version: 1,
        text: text.to_string(),
        attachments,
    })
    .unwrap_or_else(|_| Value::String(text.to_string()))
}

pub fn decode_user_content(value: &Value) -> Option<PersistedUserContent> {
    let parsed: PersistedUserContent = serde_json::from_value(value.clone()).ok()?;
    (parsed.xiao_content_version == 1).then_some(parsed)
}

fn extension_for_mime(mime: &str) -> &'static str {
    let normalized = mime.to_ascii_lowercase();
    match normalized.as_str() {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/jpeg" | "image/jpg" => "jpg",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/webm" => "webm",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "video/x-msvideo" => "avi",
        "video/x-matroska" => "mkv",
        "application/pdf" => "pdf",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_content_round_trips() {
        let value = encode_user_content(
            "describe this",
            vec![AttachmentRef {
                kind: "image".to_string(),
                mime_type: "image/png".to_string(),
                name: Some("x.png".to_string()),
                file_name: "abc.png".to_string(),
            }],
        );
        let decoded = decode_user_content(&value).unwrap();
        assert_eq!(decoded.text, "describe this");
        assert_eq!(decoded.attachments.len(), 1);
    }

    #[test]
    fn audio_storage_extensions_never_guess_mp3() {
        let cases = [
            ("audio/ogg", "ogg"),
            ("application/ogg", "ogg"),
            ("audio/opus", "opus"),
            ("audio/mpeg", "mp3"),
            ("audio/mp3", "mp3"),
            ("audio/mp4", "m4a"),
            ("audio/m4a", "m4a"),
            ("audio/x-m4a", "m4a"),
            ("audio/wav", "wav"),
            ("audio/x-wav", "wav"),
            ("audio/wave", "wav"),
            ("audio/flac", "flac"),
            ("audio/x-flac", "flac"),
            ("audio/webm", "webm"),
            ("audio/x-unknown", "bin"),
            ("application/octet-stream", "bin"),
        ];

        for (mime, expected) in cases {
            assert_eq!(extension_for_mime(mime), expected, "{mime}");
        }
    }

    #[test]
    fn persisted_media_identity_uses_exact_known_extensions() {
        let cases = [
            ("audio/mpeg", "mp3"),
            ("audio/opus", "opus"),
            ("image/png", "png"),
            ("image/webp", "webp"),
            ("video/mp4", "mp4"),
            ("video/webm", "webm"),
            ("video/quicktime", "mov"),
            ("video/x-msvideo", "avi"),
            ("video/x-matroska", "mkv"),
        ];

        for (mime, expected_extension) in cases {
            assert_eq!(extension_for_mime(mime), expected_extension, "{mime}");
        }

        assert_eq!(extension_for_mime("video/x-unknown"), "bin");
        assert_eq!(extension_for_mime("application/octet-stream"), "bin");
    }

    #[tokio::test]
    async fn rejects_path_traversal_names() {
        let attachment = AttachmentRef {
            kind: "image".to_string(),
            mime_type: "image/png".to_string(),
            name: None,
            file_name: "../secret".to_string(),
        };
        let result = load_attachment(1, 1, &attachment).await;
        assert!(result.is_err());
    }
}
