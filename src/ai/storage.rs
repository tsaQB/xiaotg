use chrono::Local;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::io::{self, Write};
use tracing::warn;

#[derive(Debug, Clone)]
pub(crate) struct TelegramInboxRecord {
    pub update_id: i64,
    pub payload_json: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    #[serde(default, skip_serializing)]
    pub api_key: String,
    #[serde(default)]
    pub api_key_ref: Option<String>,
    pub models: Vec<String>,
    pub active_model: String,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("endpoint", &self.endpoint)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field("api_key_ref", &self.api_key_ref)
            .field("models", &self.models)
            .field("active_model", &self.active_model)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderStore {
    pub active_id: Option<String>,
    pub providers: Vec<ProviderConfig>,
}

const SECRET_SCHEME_PREFIX: &str = "secret://";

fn xiao_data_dir() -> std::path::PathBuf {
    let base = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    base.join(".local/share/xiaoai")
}

fn secret_store_dir() -> std::path::PathBuf {
    xiao_data_dir().join("secrets")
}

fn secret_path_in_dir(dir: &std::path::Path, secret_ref: &str) -> io::Result<std::path::PathBuf> {
    if !secret_ref.starts_with(SECRET_SCHEME_PREFIX) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid XiaoAI secret reference",
        ));
    }
    use base64::Engine;
    let name = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret_ref.as_bytes());
    Ok(dir.join(name))
}

fn create_secret_ref(namespace: &str, id: &str) -> String {
    use rand::Rng;
    let nonce: u64 = rand::thread_rng().gen();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let safe_id: String = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    format!("secret://{namespace}/{safe_id}/{now:x}-{nonce:x}")
}

fn write_secret_in_dir(dir: &std::path::Path, secret_ref: &str, value: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    harden_dir_mode(dir);
    let final_path = secret_path_in_dir(dir, secret_ref)?;
    let tmp_path = dir.join(format!(
        ".tmp-{}-{:x}",
        std::process::id(),
        rand::random::<u64>()
    ));

    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp_path)?;
    if let Err(error) = (|| -> io::Result<()> {
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp_path, &final_path)?;
        harden_file_mode(&final_path);
        if let Ok(dir_handle) = std::fs::File::open(dir) {
            let _ = dir_handle.sync_all();
        }
        Ok(())
    })() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    Ok(())
}

fn write_secret(secret_ref: &str, value: &str) -> io::Result<()> {
    write_secret_in_dir(&secret_store_dir(), secret_ref, value)
}

fn read_secret_in_dir(dir: &std::path::Path, secret_ref: &str) -> io::Result<String> {
    let path = secret_path_in_dir(dir, secret_ref)?;
    harden_file_mode(&path);
    std::fs::read_to_string(path)
}

fn read_secret(secret_ref: &str) -> io::Result<String> {
    read_secret_in_dir(&secret_store_dir(), secret_ref)
}

fn remove_secret_in_dir(dir: &std::path::Path, secret_ref: &str) {
    let Ok(path) = secret_path_in_dir(dir, secret_ref) else {
        return;
    };
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != io::ErrorKind::NotFound {
            warn!("Failed to remove superseded XiaoAI secret: {error}");
        }
    }
}

fn remove_secret(secret_ref: &str) {
    remove_secret_in_dir(&secret_store_dir(), secret_ref);
}

pub fn get_providers_store_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return std::path::Path::new(&home).join(".xiao_providers.json");
    }
    std::path::Path::new(".xiao_providers.json").to_path_buf()
}

pub fn load_provider_store() -> ProviderStore {
    if let Ok(conn) = open_session_db() {
        if let Ok(value) = conn.query_row(
            "SELECT value FROM settings WHERE key='provider_store'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            if let Ok(store) = serde_json::from_str::<ProviderStore>(&value) {
                let has_legacy_plaintext = store
                    .providers
                    .iter()
                    .any(|provider| !provider.api_key.is_empty());
                if has_legacy_plaintext {
                    if let Err(error) = save_provider_state_db(&store) {
                        warn!("Failed to migrate plaintext provider secrets: {error}");
                        return store;
                    }
                    if let Ok(canonical) = conn.query_row(
                        "SELECT value FROM settings WHERE key='provider_store'",
                        [],
                        |row| row.get::<_, String>(0),
                    ) {
                        if let Ok(store) = serde_json::from_str::<ProviderStore>(&canonical) {
                            return hydrate_provider_store(store);
                        }
                    }
                }
                return hydrate_provider_store(store);
            }
        }
    }
    let p = get_providers_store_path();
    if p.exists() {
        if let Ok(content) = std::fs::read_to_string(&p) {
            if let Ok(store) = serde_json::from_str::<ProviderStore>(&content) {
                if save_provider_store(&store).is_ok() {
                    // The legacy JSON contains provider API keys in plaintext.
                    // Remove it only after the secret copy + SQLite reference
                    // migration committed successfully.
                    if let Err(err) = std::fs::remove_file(&p) {
                        warn!("Failed to remove migrated legacy provider file: {err}");
                    }
                    return load_provider_store();
                }
                return store;
            }
        }
    }
    ProviderStore::default()
}

pub fn save_provider_store(store: &ProviderStore) -> std::io::Result<()> {
    save_provider_state_db(store)
}

fn save_provider_state_db(store: &ProviderStore) -> std::io::Result<()> {
    let mut conn = open_session_db().map_err(|e| std::io::Error::other(e.to_string()))?;
    let existing_store = conn
        .query_row(
            "SELECT value FROM settings WHERE key='provider_store'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|value| serde_json::from_str::<ProviderStore>(&value).ok())
        .unwrap_or_default();
    let existing_refs: HashMap<String, String> = existing_store
        .providers
        .into_iter()
        .filter_map(|provider| {
            provider
                .api_key_ref
                .map(|secret_ref| (provider.id, secret_ref))
        })
        .collect();

    let mut sanitized = store.clone();
    let mut superseded_refs = Vec::new();
    let mut newly_written_refs: Vec<String> = Vec::new();
    for provider in &mut sanitized.providers {
        let old_ref = provider
            .api_key_ref
            .clone()
            .or_else(|| existing_refs.get(&provider.id).cloned());
        let has_secret = !provider.api_key.is_empty()
            && !["none", "-", "no"]
                .iter()
                .any(|sentinel| provider.api_key.eq_ignore_ascii_case(sentinel));

        if has_secret {
            let reusable = old_ref
                .as_deref()
                .and_then(|secret_ref| read_secret(secret_ref).ok())
                .is_some_and(|current| current == provider.api_key);
            if reusable {
                provider.api_key_ref = old_ref;
            } else {
                let new_ref = create_secret_ref("provider", &provider.id);
                if let Err(error) = write_secret(&new_ref, &provider.api_key) {
                    for secret_ref in &newly_written_refs {
                        remove_secret(secret_ref);
                    }
                    return Err(error);
                }
                match read_secret(&new_ref) {
                    Ok(verified) if verified == provider.api_key => {}
                    Ok(_) => {
                        remove_secret(&new_ref);
                        for secret_ref in &newly_written_refs {
                            remove_secret(secret_ref);
                        }
                        return Err(io::Error::other("provider secret verification mismatch"));
                    }
                    Err(error) => {
                        remove_secret(&new_ref);
                        for secret_ref in &newly_written_refs {
                            remove_secret(secret_ref);
                        }
                        return Err(error);
                    }
                }
                newly_written_refs.push(new_ref.clone());
                if let Some(old_ref) = old_ref {
                    superseded_refs.push(old_ref);
                }
                provider.api_key_ref = Some(new_ref);
            }
        } else {
            if let Some(old_ref) = old_ref {
                superseded_refs.push(old_ref);
            }
            provider.api_key_ref = None;
        }
        provider.api_key.clear();
    }

    let commit_result = (|| -> io::Result<()> {
        let json_str = serde_json::to_string_pretty(&sanitized).map_err(io::Error::other)?;
        let tx = conn
            .transaction()
            .map_err(|e| io::Error::other(e.to_string()))?;
        tx.execute(
            "INSERT INTO settings(key,value) VALUES('provider_store',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![json_str],
        )
        .map_err(|e| io::Error::other(e.to_string()))?;

        let active = sanitized
            .active_id
            .as_deref()
            .and_then(|id| {
                sanitized
                    .providers
                    .iter()
                    .find(|provider| provider.id == id)
            })
            .or_else(|| sanitized.providers.first());
        for (key, value) in [
            (
                "AI_ENDPOINT",
                active.map(|p| p.endpoint.as_str()).unwrap_or(""),
            ),
            (
                "AI_MODEL",
                active.map(|p| p.active_model.as_str()).unwrap_or(""),
            ),
        ] {
            tx.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![format!("app:{key}"), value],
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
        }

        if let Some(secret_ref) = active.and_then(|provider| provider.api_key_ref.as_deref()) {
            tx.execute(
                "INSERT INTO settings(key,value) VALUES('app:AI_API_KEY_REF',?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![secret_ref],
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
        } else {
            tx.execute("DELETE FROM settings WHERE key='app:AI_API_KEY_REF'", [])
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        tx.execute("DELETE FROM settings WHERE key='app:AI_API_KEY'", [])
            .map_err(|e| io::Error::other(e.to_string()))?;
        tx.commit().map_err(|e| io::Error::other(e.to_string()))
    })();

    if let Err(error) = commit_result {
        for secret_ref in &newly_written_refs {
            remove_secret(secret_ref);
        }
        return Err(error);
    }

    let live_refs: std::collections::HashSet<&str> = sanitized
        .providers
        .iter()
        .filter_map(|provider| provider.api_key_ref.as_deref())
        .collect();
    superseded_refs.sort();
    superseded_refs.dedup();
    for secret_ref in superseded_refs {
        if !live_refs.contains(secret_ref.as_str()) {
            remove_secret(&secret_ref);
        }
    }
    Ok(())
}

fn hydrate_provider_store(mut store: ProviderStore) -> ProviderStore {
    for provider in &mut store.providers {
        provider.api_key = provider
            .api_key_ref
            .as_deref()
            .and_then(|secret_ref| match read_secret(secret_ref) {
                Ok(value) => Some(value),
                Err(error) => {
                    warn!(
                        "Unable to load provider credential reference for '{}': {error}",
                        provider.id
                    );
                    None
                }
            })
            .unwrap_or_default();
    }
    store
}

pub(super) async fn persist_provider_state(store: ProviderStore) -> bool {
    match tokio::task::spawn_blocking(move || save_provider_state_db(&store)).await {
        Ok(Ok(())) => true,
        Ok(Err(err)) => {
            warn!("Failed to persist provider state: {err}");
            false
        }
        Err(err) => {
            warn!("Provider persistence task failed: {err}");
            false
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TopicScope {
    pub chat_id: i64,
    pub thread_id: i64,
}

#[allow(dead_code)]
impl TopicScope {
    pub fn new(chat_id: i64, thread_id: i64) -> Self {
        Self { chat_id, thread_id }
    }

    pub fn is_private(&self) -> bool {
        self.thread_id == 0 && self.chat_id > 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: usize,
    pub name: String,
    pub messages: Vec<ChatMessage>,
    pub created_at: String,
    #[serde(default)]
    pub revision: u64,
}

#[cfg(unix)]
fn harden_dir_mode(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        if let Err(err) = std::fs::set_permissions(path, permissions) {
            warn!("Failed to harden XiaoAI data directory permissions: {err}");
        }
    }
}

#[cfg(not(unix))]
fn harden_dir_mode(_path: &std::path::Path) {}

#[cfg(unix)]
fn harden_file_mode(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        if let Err(err) = std::fs::set_permissions(path, permissions) {
            warn!("Failed to harden XiaoAI data file permissions: {err}");
        }
    }
}

#[cfg(not(unix))]
fn harden_file_mode(_path: &std::path::Path) {}

fn session_db_path() -> std::path::PathBuf {
    xiao_data_dir().join("xiaoai.db")
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    alter_sql: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    if !exists {
        conn.execute_batch(alter_sql)?;
    }
    Ok(())
}

fn open_session_db() -> rusqlite::Result<Connection> {
    let path = session_db_path();
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            warn!("Failed to create XiaoAI data directory: {err}");
        }
        harden_dir_mode(parent);
    }
    let conn = Connection::open(&path)?;
    harden_file_mode(&path);
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
        CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS sessions (
            user_id INTEGER NOT NULL, session_id INTEGER NOT NULL, name TEXT NOT NULL,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(user_id, session_id)
        );
        CREATE TABLE IF NOT EXISTS messages (
            user_id INTEGER NOT NULL, session_id INTEGER NOT NULL DEFAULT 0,
            chat_id INTEGER NOT NULL DEFAULT 0, thread_id INTEGER NOT NULL DEFAULT 0,
            role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS active_sessions (
            user_id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS session_counters (
            user_id INTEGER PRIMARY KEY, next_session_id INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS telegram_state (
            key TEXT PRIMARY KEY, value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS telegram_inbox (
            update_id INTEGER PRIMARY KEY,
            payload_json TEXT NOT NULL,
            status TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            received_at TEXT NOT NULL,
            last_error TEXT
        );
        CREATE TABLE IF NOT EXISTS user_memories (
            user_id INTEGER NOT NULL,
            key TEXT NOT NULL,
            fact TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(user_id, key)
        );
        CREATE TABLE IF NOT EXISTS scoped_summaries (
            chat_id INTEGER NOT NULL,
            thread_id INTEGER NOT NULL DEFAULT 0,
            summary TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(chat_id, thread_id)
        );
        CREATE INDEX IF NOT EXISTS idx_messages_user_session ON messages(user_id, session_id);
        CREATE INDEX IF NOT EXISTS idx_user_memories_user ON user_memories(user_id);
        CREATE INDEX IF NOT EXISTS idx_telegram_inbox_status_update_id ON telegram_inbox(status, update_id);",
    )?;
    ensure_column(
        &conn,
        "sessions",
        "revision",
        "ALTER TABLE sessions ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;",
    )?;
    ensure_column(
        &conn,
        "messages",
        "chat_id",
        "ALTER TABLE messages ADD COLUMN chat_id INTEGER NOT NULL DEFAULT 0;",
    )?;
    ensure_column(
        &conn,
        "messages",
        "thread_id",
        "ALTER TABLE messages ADD COLUMN thread_id INTEGER NOT NULL DEFAULT 0;",
    )?;
    let _ = conn.execute(
        "UPDATE messages SET chat_id = user_id WHERE chat_id = 0 AND user_id != 0;",
        [],
    );
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_messages_chat_thread ON messages(chat_id, thread_id);",
    )?;
    // WAL/SHM files may be created lazily. The private 0700 parent directory
    // is the primary boundary; harden sidecars whenever they already exist.
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        harden_file_mode(&parent.join(format!("{name}-wal")));
        harden_file_mode(&parent.join(format!("{name}-shm")));
    }
    Ok(conn)
}

fn load_sessions_db(user_id: i64) -> rusqlite::Result<Vec<ChatSession>> {
    let conn = open_session_db()?;
    let mut stmt = conn.prepare(
        "SELECT session_id,name,created_at,revision FROM sessions WHERE user_id=?1 ORDER BY session_id",
    )?;
    let mut rows = stmt.query(params![user_id])?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next()? {
        let id: usize = row.get(0)?;
        let mut session = ChatSession {
            id,
            name: row.get(1)?,
            messages: Vec::new(),
            created_at: row.get(2)?,
            revision: row.get(3)?,
        };
        let mut msg_stmt = conn.prepare(
            "SELECT role,content FROM messages WHERE user_id=?1 AND session_id=?2 ORDER BY rowid",
        )?;
        let mut msg_rows = msg_stmt.query(params![user_id, id])?;
        while let Some(msg) = msg_rows.next()? {
            let content: String = msg.get(1)?;
            session.messages.push(ChatMessage {
                role: msg.get(0)?,
                content: serde_json::from_str(&content).unwrap_or(Value::String(content)),
            });
        }
        sessions.push(session);
    }
    Ok(sessions)
}

pub(super) fn compute_next_session_id(stored_next: Option<usize>, max_existing: usize) -> usize {
    stored_next
        .unwrap_or_else(|| max_existing.saturating_add(1))
        .max(max_existing.saturating_add(1))
        .max(1)
}

pub(super) fn legacy_active_session_id(
    legacy_index: Option<usize>,
    sessions: &[ChatSession],
) -> Option<usize> {
    if sessions.is_empty() {
        return None;
    }
    Some(
        legacy_index
            .and_then(|index| sessions.get(index).map(|session| session.id))
            .unwrap_or(sessions[0].id),
    )
}

fn allocate_session_id_tx(tx: &rusqlite::Transaction<'_>, user_id: i64) -> rusqlite::Result<usize> {
    let max_existing: usize = tx.query_row(
        "SELECT COALESCE(MAX(session_id),0) FROM sessions WHERE user_id=?1",
        params![user_id],
        |row| row.get(0),
    )?;
    let stored_next = tx
        .query_row(
            "SELECT next_session_id FROM session_counters WHERE user_id=?1",
            params![user_id],
            |row| row.get::<_, usize>(0),
        )
        .ok();
    let next_id = compute_next_session_id(stored_next, max_existing);
    tx.execute(
        "INSERT INTO session_counters(user_id,next_session_id) VALUES(?1,?2)
         ON CONFLICT(user_id) DO UPDATE SET next_session_id=excluded.next_session_id",
        params![user_id, next_id.saturating_add(1)],
    )?;
    Ok(next_id)
}

fn save_session_metadata_db(user_id: i64, session: &ChatSession) -> rusqlite::Result<()> {
    let conn = open_session_db()?;
    let now = Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO sessions(user_id,session_id,name,created_at,updated_at,revision) VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(user_id,session_id) DO UPDATE SET name=excluded.name,updated_at=excluded.updated_at,revision=excluded.revision",
        params![user_id, session.id, session.name, session.created_at, now, session.revision],
    )?;
    Ok(())
}

fn replace_session_messages_if_revision_db(
    user_id: i64,
    expected_revision: u64,
    session: &ChatSession,
) -> rusqlite::Result<bool> {
    let mut conn = open_session_db()?;
    replace_session_messages_if_revision_on_conn(&mut conn, user_id, expected_revision, session)
}

fn replace_session_messages_if_revision_on_conn(
    conn: &mut Connection,
    user_id: i64,
    expected_revision: u64,
    session: &ChatSession,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let now = Local::now().to_rfc3339();
    let changed = tx.execute(
        "UPDATE sessions SET name=?3,updated_at=?4,revision=?5
         WHERE user_id=?1 AND session_id=?2 AND revision=?6",
        params![
            user_id,
            session.id,
            session.name,
            now,
            session.revision,
            expected_revision
        ],
    )?;
    if changed != 1 {
        return Ok(false);
    }
    tx.execute(
        "DELETE FROM messages WHERE user_id=?1 AND session_id=?2",
        params![user_id, session.id],
    )?;
    for message in &session.messages {
        let content = serde_json::to_string(&message.content).unwrap_or_default();
        tx.execute(
            "INSERT INTO messages(user_id,session_id,role,content,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![user_id, session.id, message.role, content, now],
        )?;
    }
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
fn append_session_messages_on_conn(
    conn: &mut Connection,
    user_id: i64,
    expected_revision: u64,
    session: &ChatSession,
    messages: &[ChatMessage],
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let now = Local::now().to_rfc3339();
    let changed = tx.execute(
        "UPDATE sessions SET name=?3,updated_at=?4 WHERE user_id=?1 AND session_id=?2 AND revision=?5",
        params![user_id, session.id, session.name, now, expected_revision],
    )?;
    if changed != 1 {
        return Ok(false);
    }
    for message in messages {
        let content = serde_json::to_string(&message.content).unwrap_or_default();
        tx.execute(
            "INSERT INTO messages(user_id,session_id,role,content,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![user_id, session.id, message.role, content, now],
        )?;
    }
    tx.commit()?;
    Ok(true)
}

fn save_active_session_db(user_id: i64, session_id: usize) -> rusqlite::Result<()> {
    let conn = open_session_db()?;
    conn.execute(
        "INSERT INTO active_sessions(user_id,session_id) VALUES(?1,?2)
         ON CONFLICT(user_id) DO UPDATE SET session_id=excluded.session_id",
        params![user_id, session_id],
    )?;
    Ok(())
}

fn switch_active_session_db(user_id: i64, session_id: usize) -> rusqlite::Result<bool> {
    let mut conn = open_session_db()?;
    let tx = conn.transaction()?;
    let exists = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE user_id=?1 AND session_id=?2)",
        params![user_id, session_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Ok(false);
    }
    tx.execute(
        "INSERT INTO active_sessions(user_id,session_id) VALUES(?1,?2)
         ON CONFLICT(user_id) DO UPDATE SET session_id=excluded.session_id",
        params![user_id, session_id],
    )?;
    tx.commit()?;
    Ok(true)
}

fn create_session_and_activate_db(
    user_id: i64,
    name: &str,
    created_at: &str,
) -> rusqlite::Result<ChatSession> {
    let mut conn = open_session_db()?;
    let tx = conn.transaction()?;
    let session_id = allocate_session_id_tx(&tx, user_id)?;
    let now = Local::now().to_rfc3339();
    tx.execute(
        "INSERT INTO sessions(user_id,session_id,name,created_at,updated_at,revision)
         VALUES(?1,?2,?3,?4,?5,0)",
        params![user_id, session_id, name, created_at, now],
    )?;
    tx.execute(
        "INSERT INTO active_sessions(user_id,session_id) VALUES(?1,?2)
         ON CONFLICT(user_id) DO UPDATE SET session_id=excluded.session_id",
        params![user_id, session_id],
    )?;
    tx.commit()?;
    Ok(ChatSession {
        id: session_id,
        name: name.to_string(),
        messages: Vec::new(),
        created_at: created_at.to_string(),
        revision: 0,
    })
}

#[derive(Debug, Clone)]
pub(super) struct RemoveSessionOutcome {
    pub new_active_id: usize,
    pub replacement: Option<ChatSession>,
}

fn remove_session_transaction_db(
    user_id: i64,
    session_id: usize,
    replacement_name: &str,
    replacement_created_at: &str,
) -> rusqlite::Result<Option<RemoveSessionOutcome>> {
    let mut conn = open_session_db()?;
    remove_session_transaction_on_conn(
        &mut conn,
        user_id,
        session_id,
        replacement_name,
        replacement_created_at,
    )
}

fn remove_session_transaction_on_conn(
    conn: &mut Connection,
    user_id: i64,
    session_id: usize,
    replacement_name: &str,
    replacement_created_at: &str,
) -> rusqlite::Result<Option<RemoveSessionOutcome>> {
    let tx = conn.transaction()?;
    let exists = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE user_id=?1 AND session_id=?2)",
        params![user_id, session_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let count: usize = tx.query_row(
        "SELECT COUNT(*) FROM sessions WHERE user_id=?1",
        params![user_id],
        |row| row.get(0),
    )?;
    let current_active = tx
        .query_row(
            "SELECT session_id FROM active_sessions WHERE user_id=?1",
            params![user_id],
            |row| row.get::<_, usize>(0),
        )
        .ok();

    let replacement = if count == 1 {
        let replacement_id = allocate_session_id_tx(&tx, user_id)?;
        let now = Local::now().to_rfc3339();
        tx.execute(
            "INSERT INTO sessions(user_id,session_id,name,created_at,updated_at,revision)
             VALUES(?1,?2,?3,?4,?5,0)",
            params![
                user_id,
                replacement_id,
                replacement_name,
                replacement_created_at,
                now
            ],
        )?;
        Some(ChatSession {
            id: replacement_id,
            name: replacement_name.to_string(),
            messages: Vec::new(),
            created_at: replacement_created_at.to_string(),
            revision: 0,
        })
    } else {
        None
    };

    tx.execute(
        "DELETE FROM messages WHERE user_id=?1 AND session_id=?2",
        params![user_id, session_id],
    )?;
    tx.execute(
        "DELETE FROM sessions WHERE user_id=?1 AND session_id=?2",
        params![user_id, session_id],
    )?;

    let new_active_id = if let Some(replacement) = &replacement {
        replacement.id
    } else if current_active == Some(session_id)
        || current_active.is_none()
        || !current_active.is_some_and(|active_id| {
            tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE user_id=?1 AND session_id=?2)",
                params![user_id, active_id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false)
        })
    {
        tx.query_row(
            "SELECT session_id FROM sessions WHERE user_id=?1 ORDER BY session_id LIMIT 1",
            params![user_id],
            |row| row.get::<_, usize>(0),
        )?
    } else {
        current_active.unwrap_or_default()
    };

    tx.execute(
        "INSERT INTO active_sessions(user_id,session_id) VALUES(?1,?2)
         ON CONFLICT(user_id) DO UPDATE SET session_id=excluded.session_id",
        params![user_id, new_active_id],
    )?;
    tx.commit()?;
    Ok(Some(RemoveSessionOutcome {
        new_active_id,
        replacement,
    }))
}

fn ensure_session_identity_v2_db(user_id: i64, sessions: &[ChatSession]) -> rusqlite::Result<()> {
    if sessions.is_empty() {
        return Ok(());
    }
    let conn = open_session_db()?;
    let marker = format!("session_identity_v2:{user_id}");
    let migrated = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![&marker],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .is_some();

    if !migrated {
        let legacy_value = conn
            .query_row(
                "SELECT session_id FROM active_sessions WHERE user_id=?1",
                params![user_id],
                |row| row.get::<_, usize>(0),
            )
            .ok();
        let stable_id = legacy_active_session_id(legacy_value, sessions).unwrap_or(sessions[0].id);
        save_active_session_db(user_id, stable_id)?;
        conn.execute(
            "INSERT INTO settings(key,value) VALUES(?1,'1') ON CONFLICT(key) DO UPDATE SET value='1'",
            params![&marker],
        )?;
    }

    let next_id = sessions
        .iter()
        .map(|session| session.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1);
    conn.execute(
        "INSERT INTO session_counters(user_id,next_session_id) VALUES(?1,?2)
         ON CONFLICT(user_id) DO UPDATE SET next_session_id=MAX(next_session_id,excluded.next_session_id)",
        params![user_id, next_id],
    )?;
    Ok(())
}

async fn run_db<T, F>(operation: &'static str, task: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> rusqlite::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(err)) => {
            warn!("SQLite operation {operation} failed: {err}");
            None
        }
        Err(err) => {
            warn!("SQLite task {operation} failed: {err}");
            None
        }
    }
}

fn load_telegram_offset_db() -> rusqlite::Result<Option<i64>> {
    let conn = open_session_db()?;
    let value = conn
        .query_row(
            "SELECT value FROM telegram_state WHERE key='offset'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    Ok(value.and_then(|value| value.parse::<i64>().ok()))
}

fn enqueue_telegram_update_db(update_id: i64, payload_json: &str) -> rusqlite::Result<bool> {
    let mut conn = open_session_db()?;
    enqueue_telegram_update_on_conn(&mut conn, update_id, payload_json)
}

fn enqueue_telegram_update_on_conn(
    conn: &mut Connection,
    update_id: i64,
    payload_json: &str,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO telegram_inbox(update_id,payload_json,status,attempts,received_at)
         VALUES(?1,?2,'pending',0,?3)",
        params![update_id, payload_json, Local::now().to_rfc3339()],
    )? == 1;
    tx.execute(
        "INSERT INTO telegram_state(key,value) VALUES('offset',?1)
         ON CONFLICT(key) DO UPDATE SET value=
           CASE
             WHEN CAST(excluded.value AS INTEGER) > CAST(value AS INTEGER)
             THEN excluded.value ELSE value
           END",
        params![update_id.saturating_add(1).to_string()],
    )?;
    // Completed tombstones deduplicate a repeated Telegram delivery. Keep a
    // bounded recent window so the inbox itself cannot grow forever.
    tx.execute(
        "DELETE FROM telegram_inbox
         WHERE status='completed' AND update_id < (
           SELECT COALESCE(MAX(update_id),0) - 5000 FROM telegram_inbox
         )",
        [],
    )?;
    tx.commit()?;
    Ok(inserted)
}

fn recover_telegram_processing_db() -> rusqlite::Result<usize> {
    let conn = open_session_db()?;
    recover_telegram_processing_on_conn(&conn)
}

fn pending_telegram_updates_after_db(
    after_update_id: i64,
    limit: usize,
) -> rusqlite::Result<Vec<TelegramInboxRecord>> {
    let conn = open_session_db()?;
    pending_telegram_updates_after_on_conn(&conn, after_update_id, limit)
}

fn pending_telegram_updates_after_on_conn(
    conn: &Connection,
    after_update_id: i64,
    limit: usize,
) -> rusqlite::Result<Vec<TelegramInboxRecord>> {
    let mut stmt = conn.prepare(
        "SELECT update_id,payload_json
         FROM telegram_inbox
         WHERE status='pending' AND update_id>?1
         ORDER BY update_id
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![after_update_id, limit as i64], |row| {
        Ok(TelegramInboxRecord {
            update_id: row.get(0)?,
            payload_json: row.get(1)?,
        })
    })?;
    rows.collect()
}

fn recover_telegram_processing_on_conn(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE telegram_inbox
         SET status='pending',last_error='recovered after daemon stopped while processing'
         WHERE status='processing'",
        [],
    )
}

fn mark_telegram_processing_db(update_id: i64) -> rusqlite::Result<bool> {
    let conn = open_session_db()?;
    mark_telegram_processing_on_conn(&conn, update_id)
}

fn mark_telegram_processing_on_conn(conn: &Connection, update_id: i64) -> rusqlite::Result<bool> {
    // Keep the payload until the handler reaches its completed checkpoint. If
    // XiaoAI crashes immediately after this claim, startup recovery can safely
    // make the update pending again instead of losing it forever. This gives
    // the inbox explicit at-least-once processing semantics; a crash after an
    // external side effect but before completion can still repeat that effect.
    Ok(conn.execute(
        "UPDATE telegram_inbox
         SET status='processing',attempts=attempts+1,last_error=NULL
         WHERE update_id=?1 AND status='pending'",
        params![update_id],
    )? == 1)
}

fn mark_telegram_processed_db(update_id: i64) -> rusqlite::Result<bool> {
    let conn = open_session_db()?;
    mark_telegram_processed_on_conn(&conn, update_id)
}

fn mark_telegram_processed_on_conn(conn: &Connection, update_id: i64) -> rusqlite::Result<bool> {
    let scrubbed = serde_json::json!({
        "update_id": update_id,
        "payload": "redacted_after_completion"
    })
    .to_string();
    Ok(conn.execute(
        "UPDATE telegram_inbox
         SET status='completed',payload_json=?2,last_error=NULL
         WHERE update_id=?1 AND status='processing'",
        params![update_id, scrubbed],
    )? == 1)
}

pub(crate) async fn load_telegram_offset_async() -> Option<i64> {
    run_db("load_telegram_offset", load_telegram_offset_db)
        .await
        .flatten()
}

pub(crate) async fn enqueue_telegram_update_async(
    update_id: i64,
    payload_json: String,
) -> Option<bool> {
    run_db("enqueue_telegram_update", move || {
        enqueue_telegram_update_db(update_id, &payload_json)
    })
    .await
}

pub(crate) async fn pending_telegram_updates_after_async(
    after_update_id: i64,
    limit: usize,
) -> Vec<TelegramInboxRecord> {
    run_db("pending_telegram_updates_after", move || {
        pending_telegram_updates_after_db(after_update_id, limit)
    })
    .await
    .unwrap_or_default()
}

pub(crate) async fn recover_telegram_processing_async() -> usize {
    run_db(
        "recover_telegram_processing",
        recover_telegram_processing_db,
    )
    .await
    .unwrap_or_default()
}

pub(crate) async fn mark_telegram_processing_async(update_id: i64) -> bool {
    run_db("mark_telegram_processing", move || {
        mark_telegram_processing_db(update_id)
    })
    .await
    .unwrap_or(false)
}

pub(crate) async fn mark_telegram_processed_async(update_id: i64) -> bool {
    run_db("mark_telegram_processed", move || {
        mark_telegram_processed_db(update_id)
    })
    .await
    .unwrap_or(false)
}

pub(super) async fn load_sessions_db_async(user_id: i64) -> Vec<ChatSession> {
    run_db("load_sessions", move || load_sessions_db(user_id))
        .await
        .unwrap_or_default()
}

pub(super) async fn save_session_metadata_db_async(user_id: i64, session: ChatSession) -> bool {
    run_db("save_session_metadata", move || {
        save_session_metadata_db(user_id, &session)
    })
    .await
    .is_some()
}

pub(super) async fn replace_session_messages_if_revision_db_async(
    user_id: i64,
    expected_revision: u64,
    session: ChatSession,
) -> Option<bool> {
    run_db("replace_session_messages_if_revision", move || {
        replace_session_messages_if_revision_db(user_id, expected_revision, &session)
    })
    .await
}

pub(super) async fn switch_active_session_db_async(
    user_id: i64,
    session_id: usize,
) -> Option<bool> {
    run_db("switch_active_session", move || {
        switch_active_session_db(user_id, session_id)
    })
    .await
}

pub(super) async fn create_session_and_activate_db_async(
    user_id: i64,
    name: String,
    created_at: String,
) -> Option<ChatSession> {
    run_db("create_session_and_activate", move || {
        create_session_and_activate_db(user_id, &name, &created_at)
    })
    .await
}

pub(super) async fn remove_session_transaction_db_async(
    user_id: i64,
    session_id: usize,
    replacement_name: String,
    replacement_created_at: String,
) -> Option<Option<RemoveSessionOutcome>> {
    run_db("remove_session_transaction", move || {
        remove_session_transaction_db(
            user_id,
            session_id,
            &replacement_name,
            &replacement_created_at,
        )
    })
    .await
}

pub(super) async fn ensure_session_identity_v2_db_async(
    user_id: i64,
    sessions: Vec<ChatSession>,
) -> bool {
    run_db("ensure_session_identity_v2", move || {
        ensure_session_identity_v2_db(user_id, &sessions)
    })
    .await
    .is_some()
}

pub(super) async fn load_active_session_id_db_async(user_id: i64) -> Option<usize> {
    run_db("load_active_session", move || {
        let conn = open_session_db()?;
        conn.query_row(
            "SELECT session_id FROM active_sessions WHERE user_id=?1",
            params![user_id],
            |row| row.get::<_, usize>(0),
        )
    })
    .await
}

pub fn load_scoped_messages(
    chat_id: i64,
    thread_id: i64,
    limit: usize,
) -> rusqlite::Result<Vec<ChatMessage>> {
    let conn = open_session_db()?;
    load_scoped_messages_on_conn(&conn, chat_id, thread_id, limit)
}

fn load_scoped_messages_on_conn(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    limit: usize,
) -> rusqlite::Result<Vec<ChatMessage>> {
    let mut stmt = conn.prepare(
        "SELECT role, content FROM (
            SELECT rowid, role, content FROM messages
            WHERE chat_id = ?1 AND thread_id = ?2
            ORDER BY rowid DESC
            LIMIT ?3
        ) ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map(params![chat_id, thread_id, limit as i64], |row| {
        let role: String = row.get(0)?;
        let content: String = row.get(1)?;
        let content_val = serde_json::from_str(&content).unwrap_or(Value::String(content));
        Ok(ChatMessage {
            role,
            content: content_val,
        })
    })?;
    rows.collect()
}

pub fn count_scoped_messages(chat_id: i64, thread_id: i64) -> rusqlite::Result<usize> {
    let conn = open_session_db()?;
    count_scoped_messages_on_conn(&conn, chat_id, thread_id)
}

fn count_scoped_messages_on_conn(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
) -> rusqlite::Result<usize> {
    conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND thread_id = ?2",
        params![chat_id, thread_id],
        |row| row.get(0),
    )
}

pub fn save_scoped_message(
    chat_id: i64,
    thread_id: i64,
    user_id: i64,
    role: &str,
    content: &str,
) -> rusqlite::Result<()> {
    let conn = open_session_db()?;
    save_scoped_message_on_conn(&conn, chat_id, thread_id, user_id, role, content)
}

fn save_scoped_message_on_conn(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    user_id: i64,
    role: &str,
    content: &str,
) -> rusqlite::Result<()> {
    let now = Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO messages(user_id, session_id, chat_id, thread_id, role, content, created_at)
         VALUES(?1, 0, ?2, ?3, ?4, ?5, ?6)",
        params![user_id, chat_id, thread_id, role, content, now],
    )?;
    Ok(())
}

pub fn get_user_memories(user_id: i64) -> rusqlite::Result<Vec<(String, String)>> {
    let conn = open_session_db()?;
    get_user_memories_on_conn(&conn, user_id)
}

fn get_user_memories_on_conn(
    conn: &Connection,
    user_id: i64,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt =
        conn.prepare("SELECT key, fact FROM user_memories WHERE user_id = ?1 ORDER BY key ASC")?;
    let rows = stmt.query_map(params![user_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

pub fn save_user_memory(user_id: i64, key: &str, fact: &str) -> rusqlite::Result<()> {
    let conn = open_session_db()?;
    save_user_memory_on_conn(&conn, user_id, key, fact)
}

fn save_user_memory_on_conn(
    conn: &Connection,
    user_id: i64,
    key: &str,
    fact: &str,
) -> rusqlite::Result<()> {
    let now = Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO user_memories(user_id, key, fact, updated_at) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, key) DO UPDATE SET fact = excluded.fact, updated_at = excluded.updated_at",
        params![user_id, key, fact, now],
    )?;
    Ok(())
}

pub fn delete_user_memory(user_id: i64, key: &str) -> rusqlite::Result<()> {
    let conn = open_session_db()?;
    delete_user_memory_on_conn(&conn, user_id, key)
}

fn delete_user_memory_on_conn(conn: &Connection, user_id: i64, key: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM user_memories WHERE user_id = ?1 AND key = ?2",
        params![user_id, key],
    )?;
    Ok(())
}

pub fn clear_user_memories(user_id: i64) -> rusqlite::Result<()> {
    let conn = open_session_db()?;
    clear_user_memories_on_conn(&conn, user_id)
}

fn clear_user_memories_on_conn(conn: &Connection, user_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM user_memories WHERE user_id = ?1",
        params![user_id],
    )?;
    Ok(())
}

pub async fn load_scoped_messages_async(
    chat_id: i64,
    thread_id: i64,
    limit: usize,
) -> Vec<ChatMessage> {
    run_db("load_scoped_messages", move || {
        load_scoped_messages(chat_id, thread_id, limit)
    })
    .await
    .unwrap_or_default()
}

pub async fn count_scoped_messages_async(chat_id: i64, thread_id: i64) -> usize {
    run_db("count_scoped_messages", move || {
        count_scoped_messages(chat_id, thread_id)
    })
    .await
    .unwrap_or_default()
}

pub async fn save_scoped_message_async(
    chat_id: i64,
    thread_id: i64,
    user_id: i64,
    role: String,
    content: String,
) -> bool {
    run_db("save_scoped_message", move || {
        save_scoped_message(chat_id, thread_id, user_id, &role, &content)
    })
    .await
    .is_some()
}

pub async fn get_user_memories_async(user_id: i64) -> Vec<(String, String)> {
    run_db("get_user_memories", move || get_user_memories(user_id))
        .await
        .unwrap_or_default()
}

pub async fn save_user_memory_async(user_id: i64, key: String, fact: String) -> bool {
    run_db("save_user_memory", move || {
        save_user_memory(user_id, &key, &fact)
    })
    .await
    .is_some()
}

pub async fn delete_user_memory_async(user_id: i64, key: String) -> bool {
    run_db("delete_user_memory", move || {
        delete_user_memory(user_id, &key)
    })
    .await
    .is_some()
}

pub async fn clear_user_memories_async(user_id: i64) -> bool {
    run_db("clear_user_memories", move || clear_user_memories(user_id))
        .await
        .is_some()
}

pub fn clear_scoped_messages(chat_id: i64, thread_id: i64) -> rusqlite::Result<()> {
    let conn = open_session_db()?;
    conn.execute(
        "DELETE FROM messages WHERE chat_id=?1 AND thread_id=?2",
        params![chat_id, thread_id],
    )?;
    conn.execute(
        "DELETE FROM scoped_summaries WHERE chat_id=?1 AND thread_id=?2",
        params![chat_id, thread_id],
    )?;
    Ok(())
}

pub async fn clear_scoped_messages_async(chat_id: i64, thread_id: i64) -> bool {
    run_db("clear_scoped_messages", move || {
        clear_scoped_messages(chat_id, thread_id)
    })
    .await
    .is_some()
}

pub fn get_scoped_summary(chat_id: i64, thread_id: i64) -> rusqlite::Result<Option<String>> {
    let conn = open_session_db()?;
    get_scoped_summary_on_conn(&conn, chat_id, thread_id)
}

fn get_scoped_summary_on_conn(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT summary FROM scoped_summaries WHERE chat_id=?1 AND thread_id=?2",
        params![chat_id, thread_id],
        |row| row.get(0),
    )
    .optional()
}

pub fn save_scoped_summary(chat_id: i64, thread_id: i64, summary: &str) -> rusqlite::Result<()> {
    let conn = open_session_db()?;
    save_scoped_summary_on_conn(&conn, chat_id, thread_id, summary)
}

fn save_scoped_summary_on_conn(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    summary: &str,
) -> rusqlite::Result<()> {
    let now = Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO scoped_summaries(chat_id, thread_id, summary, updated_at) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(chat_id, thread_id) DO UPDATE SET summary=excluded.summary, updated_at=excluded.updated_at",
        params![chat_id, thread_id, summary, now],
    )?;
    Ok(())
}

pub async fn get_scoped_summary_async(chat_id: i64, thread_id: i64) -> Option<String> {
    run_db("get_scoped_summary", move || {
        get_scoped_summary(chat_id, thread_id)
    })
    .await
    .flatten()
}

pub async fn save_scoped_summary_async(chat_id: i64, thread_id: i64, summary: String) -> bool {
    run_db("save_scoped_summary", move || {
        save_scoped_summary(chat_id, thread_id, &summary)
    })
    .await
    .is_some()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    TextChat,
    ImageInput,
    ImageGeneration,
    ImageEditing,
    AudioInput,
    AudioTranscription,
    VideoInput,
    NativeFileInput,
    Tools,
    StructuredOutput,
    Reasoning,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityEvidenceSource {
    ProviderMetadata,
    ActiveProbe,
    KnownProviderProfile,
    UserOverride,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityEvidence {
    pub capability: CapabilityKind,
    pub source: CapabilityEvidenceSource,
    pub outcome: CapabilityState,
    pub checked_at: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceFreshness {
    Fresh,
    Stale,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeRunStatus {
    Waiting,
    CheckingMetadata,
    Probing,
    Completed,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    Supported,
    Unsupported,
    Inconclusive,
    AuthFailed,
    RateLimited,
    Timeout,
    NetworkError,
    ProtocolMismatch,
    ProviderError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ProbeEvent {
    Started {
        capability: CapabilityKind,
    },
    Progress {
        capability: CapabilityKind,
        message: String,
    },
    Completed {
        capability: CapabilityKind,
        outcome: ProbeOutcome,
    },
    Skipped {
        capability: CapabilityKind,
        reason: String,
    },
    Persistence {
        saved: bool,
    },
    Finished,
}

#[allow(dead_code)]
impl ProbeEvent {
    pub fn run_status(&self) -> ProbeRunStatus {
        match self {
            Self::Progress { message, .. } if message.starts_with("Checking provider metadata") => {
                ProbeRunStatus::CheckingMetadata
            }
            Self::Progress { message, .. } if message.starts_with("Persisting") => {
                ProbeRunStatus::Waiting
            }
            Self::Started { .. } | Self::Progress { .. } => ProbeRunStatus::Probing,
            Self::Completed { .. } | Self::Finished => ProbeRunStatus::Completed,
            Self::Skipped { .. } => ProbeRunStatus::Skipped,
            Self::Persistence { saved: false } => ProbeRunStatus::Failed,
            Self::Persistence { saved: true } => ProbeRunStatus::Completed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityRecord {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub context_window: Option<usize>,
    #[serde(default, alias = "supports_text")]
    pub supports_text_chat: Option<bool>,
    #[serde(default, alias = "supports_image")]
    pub supports_image_input: Option<bool>,
    #[serde(default)]
    pub supports_image_generation: Option<bool>,
    #[serde(default)]
    pub supports_image_editing: Option<bool>,
    #[serde(default, alias = "supports_audio")]
    pub supports_audio_input: Option<bool>,
    #[serde(default)]
    pub supports_audio_transcription: Option<bool>,
    #[serde(default, alias = "supports_video")]
    pub supports_video_input: Option<bool>,
    #[serde(default, alias = "supports_file_input")]
    pub supports_native_file_input: Option<bool>,
    #[serde(default)]
    pub supports_reasoning: Option<bool>,
    #[serde(default)]
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub supports_structured_output: Option<bool>,
    #[serde(default)]
    pub evidence: Vec<CapabilityEvidence>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub details: Vec<String>,
    #[serde(default)]
    pub checked_at: String,
}

impl CapabilityRecord {
    #[cfg(test)]
    fn timestamp_freshness(checked_at: &str, ttl: std::time::Duration) -> EvidenceFreshness {
        let Ok(checked_at) = chrono::DateTime::parse_from_rfc3339(checked_at) else {
            return EvidenceFreshness::Stale;
        };
        let age = chrono::Utc::now().signed_duration_since(checked_at.with_timezone(&chrono::Utc));
        if age.num_seconds() >= 0 && age.to_std().is_ok_and(|age| age <= ttl) {
            EvidenceFreshness::Fresh
        } else {
            EvidenceFreshness::Stale
        }
    }

    fn evidence_source_ttl(source: CapabilityEvidenceSource) -> Option<std::time::Duration> {
        match source {
            CapabilityEvidenceSource::ProviderMetadata => {
                Some(std::time::Duration::from_secs(6 * 60 * 60))
            }
            CapabilityEvidenceSource::ActiveProbe => {
                Some(std::time::Duration::from_secs(7 * 24 * 60 * 60))
            }
            CapabilityEvidenceSource::KnownProviderProfile => {
                Some(std::time::Duration::from_secs(30 * 24 * 60 * 60))
            }
            CapabilityEvidenceSource::UserOverride => None,
        }
    }

    fn evidence_source_precedence(source: CapabilityEvidenceSource) -> u8 {
        match source {
            CapabilityEvidenceSource::ProviderMetadata => 1,
            CapabilityEvidenceSource::KnownProviderProfile => 2,
            CapabilityEvidenceSource::ActiveProbe => 3,
            CapabilityEvidenceSource::UserOverride => 4,
        }
    }

    fn evidence_is_fresh(evidence: &CapabilityEvidence) -> bool {
        let Ok(checked_at) = chrono::DateTime::parse_from_rfc3339(&evidence.checked_at) else {
            return false;
        };
        let age = chrono::Utc::now().signed_duration_since(checked_at.with_timezone(&chrono::Utc));
        if age.num_seconds() < 0 {
            return false;
        }
        match Self::evidence_source_ttl(evidence.source) {
            Some(ttl) => age.to_std().is_ok_and(|age| age <= ttl),
            None => true,
        }
    }

    pub fn effective_evidence_for(
        &self,
        capability: CapabilityKind,
    ) -> Option<&CapabilityEvidence> {
        self.evidence
            .iter()
            .filter(|evidence| evidence.capability == capability)
            .filter(|evidence| Self::evidence_is_fresh(evidence))
            .max_by(|left, right| {
                let left_key = (
                    Self::evidence_source_precedence(left.source),
                    chrono::DateTime::parse_from_rfc3339(&left.checked_at)
                        .map(|timestamp| timestamp.timestamp_millis())
                        .unwrap_or(i64::MIN),
                );
                let right_key = (
                    Self::evidence_source_precedence(right.source),
                    chrono::DateTime::parse_from_rfc3339(&right.checked_at)
                        .map(|timestamp| timestamp.timestamp_millis())
                        .unwrap_or(i64::MIN),
                );
                left_key.cmp(&right_key)
            })
    }

    pub fn effective_state_for(&self, capability: CapabilityKind) -> CapabilityState {
        self.effective_evidence_for(capability)
            .map(|evidence| evidence.outcome)
            .unwrap_or(CapabilityState::Unknown)
    }

    #[cfg(test)]
    pub fn freshness_for(
        &self,
        capability: CapabilityKind,
        ttl: std::time::Duration,
    ) -> EvidenceFreshness {
        let latest = self
            .evidence
            .iter()
            .filter(|evidence| evidence.capability == capability)
            .filter_map(|evidence| {
                chrono::DateTime::parse_from_rfc3339(&evidence.checked_at)
                    .ok()
                    .map(|timestamp| (timestamp, evidence.checked_at.as_str()))
            })
            .max_by_key(|(timestamp, _)| timestamp.timestamp_millis())
            .map(|(_, checked_at)| checked_at);

        // Freshness is strictly capability-scoped. Unrelated metadata or
        // catalog refreshes must never re-authorize stale legacy or missing
        // capability evidence. If no typed evidence exists for this capability,
        // it is always Stale (fail-closed).
        match latest {
            Some(checked_at) => Self::timestamp_freshness(checked_at, ttl),
            None => EvidenceFreshness::Stale,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityRegistry {
    pub models: Vec<CapabilityRecord>,
}

fn decode_model_routing(value: Option<&str>) -> (crate::ai::routing::ModelRoutingConfig, bool) {
    match value.and_then(|value| {
        serde_json::from_str::<crate::ai::routing::ModelRoutingConfig>(value).ok()
    }) {
        Some(config) => (config, false),
        None => (crate::ai::routing::ModelRoutingConfig::default(), true),
    }
}

pub fn load_model_routing() -> crate::ai::routing::ModelRoutingConfig {
    let stored = open_session_db().ok().and_then(|conn| {
        conn.query_row(
            "SELECT value FROM settings WHERE key='model_routing'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
    });
    let (config, needs_persist) = decode_model_routing(stored.as_deref());
    if needs_persist {
        if let Err(error) = save_model_routing(&config) {
            warn!("Failed to persist default model routing: {error}");
        }
    }
    config
}

pub fn save_model_routing(config: &crate::ai::routing::ModelRoutingConfig) -> std::io::Result<()> {
    let value = serde_json::to_string_pretty(config)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let conn = open_session_db().map_err(|error| std::io::Error::other(error.to_string()))?;
    conn.execute(
        "INSERT INTO settings(key,value) VALUES('model_routing',?1) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![value],
    )
    .map(|_| ())
    .map_err(|error| std::io::Error::other(error.to_string()))
}

pub(super) async fn persist_model_routing(config: crate::ai::routing::ModelRoutingConfig) -> bool {
    match tokio::task::spawn_blocking(move || save_model_routing(&config)).await {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            warn!("Failed to persist model routing: {error}");
            false
        }
        Err(error) => {
            warn!("Model routing persistence task failed: {error}");
            false
        }
    }
}

pub fn get_capability_registry_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return std::path::Path::new(&home).join(".xiao_model_capabilities.json");
    }
    std::path::Path::new(".xiao_model_capabilities.json").to_path_buf()
}

pub fn load_capability_registry() -> CapabilityRegistry {
    if let Ok(conn) = open_session_db() {
        if let Ok(value) = conn.query_row(
            "SELECT value FROM settings WHERE key='capability_registry'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            if let Ok(registry) = serde_json::from_str(&value) {
                return registry;
            }
        }
    }
    let path = get_capability_registry_path();
    let registry = std::fs::read_to_string(path)
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default();
    if let Err(error) = save_capability_registry(&registry) {
        eprintln!("[WARN] Failed to migrate capability registry into SQLite: {error}");
    }
    registry
}

pub fn save_capability_registry(registry: &CapabilityRegistry) -> std::io::Result<()> {
    let value =
        serde_json::to_string_pretty(registry).map_err(|e| std::io::Error::other(e.to_string()))?;
    let conn = open_session_db().map_err(|e| std::io::Error::other(e.to_string()))?;
    conn.execute("INSERT INTO settings(key,value) VALUES('capability_registry',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![value])
        .map(|_| ())
        .map_err(|e| std::io::Error::other(e.to_string()))
}

pub(super) async fn persist_capability_registry(registry: CapabilityRegistry) -> bool {
    match tokio::task::spawn_blocking(move || save_capability_registry(&registry)).await {
        Ok(Ok(())) => true,
        Ok(Err(err)) => {
            warn!("Failed to persist capability registry: {err}");
            false
        }
        Err(err) => {
            warn!("Capability persistence task failed: {err}");
            false
        }
    }
}

fn secret_setting_namespace(key: &str) -> Option<&'static str> {
    match key {
        "BOT_TOKEN" => Some("telegram"),
        "AI_API_KEY" => Some("app-provider"),
        _ => None,
    }
}

fn migrate_legacy_secret_setting_on_conn(
    conn: &mut Connection,
    secret_dir: &std::path::Path,
    key: &str,
    namespace: &str,
    legacy: &str,
) -> io::Result<String> {
    let ref_key = format!("app:{key}_REF");
    let raw_key = format!("app:{key}");
    let secret_ref = create_secret_ref(namespace, "main");
    write_secret_in_dir(secret_dir, &secret_ref, legacy)?;

    // First commit only the reference. The legacy plaintext remains available
    // until the newly written secret has been read back successfully.
    let reference_commit = (|| -> io::Result<()> {
        let tx = conn
            .transaction()
            .map_err(|error| io::Error::other(error.to_string()))?;
        tx.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![&ref_key, &secret_ref],
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        tx.commit()
            .map_err(|error| io::Error::other(error.to_string()))
    })();
    if let Err(error) = reference_commit {
        remove_secret_in_dir(secret_dir, &secret_ref);
        return Err(error);
    }

    let verified = match read_secret_in_dir(secret_dir, &secret_ref) {
        Ok(value) if value == legacy => value,
        Ok(_) => {
            if let Err(cleanup_error) =
                conn.execute("DELETE FROM settings WHERE key=?1", params![&ref_key])
            {
                warn!("Failed to roll back unverified secret reference {ref_key}: {cleanup_error}");
            }
            remove_secret_in_dir(secret_dir, &secret_ref);
            return Err(io::Error::other("secret migration verification mismatch"));
        }
        Err(error) => {
            if let Err(cleanup_error) =
                conn.execute("DELETE FROM settings WHERE key=?1", params![&ref_key])
            {
                warn!("Failed to roll back unreadable secret reference {ref_key}: {cleanup_error}");
            }
            remove_secret_in_dir(secret_dir, &secret_ref);
            return Err(error);
        }
    };

    // Plaintext removal is intentionally a second durable step after read-back
    // verification. If this delete fails, keeping the legacy row is safer than
    // losing the credential; the reference remains valid and can be retried.
    conn.execute("DELETE FROM settings WHERE key=?1", params![&raw_key])
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(verified)
}

fn load_secret_app_setting(key: &str, namespace: &str) -> Option<String> {
    let mut conn = open_session_db().ok()?;
    let ref_key = format!("app:{key}_REF");
    if let Ok(secret_ref) = conn.query_row(
        "SELECT value FROM settings WHERE key=?1",
        params![&ref_key],
        |row| row.get::<_, String>(0),
    ) {
        match read_secret(&secret_ref) {
            Ok(value) => return Some(value),
            Err(error) => {
                // A legacy plaintext row may still exist if an earlier migration
                // committed the reference but crashed before cleanup. Fall back
                // to it rather than losing access to the credential.
                warn!("Unable to resolve secret setting {key}: {error}; checking legacy fallback");
            }
        }
    }

    let raw_key = format!("app:{key}");
    let legacy = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![&raw_key],
            |row| row.get::<_, String>(0),
        )
        .ok()?;
    if legacy.is_empty() {
        return Some(legacy);
    }

    match migrate_legacy_secret_setting_on_conn(
        &mut conn,
        &secret_store_dir(),
        key,
        namespace,
        &legacy,
    ) {
        Ok(value) => Some(value),
        Err(error) => {
            warn!("Failed to migrate legacy secret setting {key}: {error}");
            Some(legacy)
        }
    }
}

fn save_secret_app_setting(key: &str, namespace: &str, value: &str) -> io::Result<()> {
    let mut conn = open_session_db().map_err(|error| io::Error::other(error.to_string()))?;
    let ref_key = format!("app:{key}_REF");
    let raw_key = format!("app:{key}");
    let old_ref = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![&ref_key],
            |row| row.get::<_, String>(0),
        )
        .ok();

    let reusable = old_ref
        .as_deref()
        .and_then(|secret_ref| read_secret(secret_ref).ok())
        .is_some_and(|current| current == value);
    let new_ref = if value.is_empty() {
        None
    } else if reusable {
        old_ref.clone()
    } else {
        let secret_ref = create_secret_ref(namespace, "main");
        write_secret(&secret_ref, value)?;
        let verified = read_secret(&secret_ref)?;
        if verified != value {
            remove_secret(&secret_ref);
            return Err(io::Error::other("secret write verification mismatch"));
        }
        Some(secret_ref)
    };

    let commit_result = (|| -> io::Result<()> {
        let tx = conn
            .transaction()
            .map_err(|error| io::Error::other(error.to_string()))?;
        if let Some(secret_ref) = new_ref.as_deref() {
            tx.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![&ref_key, secret_ref],
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
        } else {
            tx.execute("DELETE FROM settings WHERE key=?1", params![&ref_key])
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        tx.execute("DELETE FROM settings WHERE key=?1", params![&raw_key])
            .map_err(|error| io::Error::other(error.to_string()))?;
        tx.commit()
            .map_err(|error| io::Error::other(error.to_string()))
    })();
    if let Err(error) = commit_result {
        if new_ref.as_deref() != old_ref.as_deref() {
            if let Some(secret_ref) = new_ref.as_deref() {
                remove_secret(secret_ref);
            }
        }
        return Err(error);
    }
    if old_ref.as_deref() != new_ref.as_deref() {
        if let Some(secret_ref) = old_ref.as_deref() {
            remove_secret(secret_ref);
        }
    }
    Ok(())
}

pub fn load_app_setting(key: &str) -> Option<String> {
    if let Some(namespace) = secret_setting_namespace(key) {
        return load_secret_app_setting(key, namespace);
    }
    open_session_db()
        .ok()?
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![format!("app:{key}")],
            |row| row.get(0),
        )
        .ok()
}

pub fn save_app_setting(key: &str, value: &str) -> std::io::Result<()> {
    if let Some(namespace) = secret_setting_namespace(key) {
        return save_secret_app_setting(key, namespace, value);
    }
    let conn = open_session_db().map_err(|e| std::io::Error::other(e.to_string()))?;
    conn.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("app:{key}"), value])
        .map(|_| ())
        .map_err(|e| std::io::Error::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::routing::ModelRoutingConfig;

    fn session_test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                user_id INTEGER NOT NULL, session_id INTEGER NOT NULL, name TEXT NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(user_id, session_id)
            );
            CREATE TABLE messages (
                user_id INTEGER NOT NULL, session_id INTEGER NOT NULL DEFAULT 0,
                chat_id INTEGER NOT NULL DEFAULT 0, thread_id INTEGER NOT NULL DEFAULT 0,
                role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL
            );
            CREATE TABLE active_sessions (user_id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL);
            CREATE TABLE session_counters (user_id INTEGER PRIMARY KEY, next_session_id INTEGER NOT NULL);
            CREATE TABLE telegram_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE telegram_inbox (
                update_id INTEGER PRIMARY KEY, payload_json TEXT NOT NULL, status TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0, received_at TEXT NOT NULL, last_error TEXT
            );
            CREATE TABLE user_memories (
                user_id INTEGER NOT NULL,
                key TEXT NOT NULL,
                fact TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(user_id, key)
            );
            CREATE TABLE scoped_summaries (
                chat_id INTEGER NOT NULL,
                thread_id INTEGER NOT NULL DEFAULT 0,
                summary TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(chat_id, thread_id)
            );
            CREATE INDEX idx_messages_chat_thread ON messages(chat_id, thread_id);
            CREATE INDEX idx_user_memories_user ON user_memories(user_id);",
        )
        .unwrap();
        conn
    }

    fn seed_session(conn: &Connection, revision: u64) {
        conn.execute(
            "INSERT INTO sessions(user_id,session_id,name,created_at,updated_at,revision)
             VALUES(7,3,'Original','now','now',?1)",
            params![revision],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO active_sessions(user_id,session_id) VALUES(7,3)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(user_id,session_id,role,content,created_at)
             VALUES(7,3,'user','\"hello\"','now')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn clear_transaction_failure_leaves_revision_and_history_unchanged() {
        let mut conn = session_test_conn();
        seed_session(&conn, 4);
        conn.execute_batch(
            "CREATE TRIGGER fail_clear BEFORE DELETE ON messages
             BEGIN SELECT RAISE(ABORT, 'clear failpoint'); END;",
        )
        .unwrap();
        let candidate = ChatSession {
            id: 3,
            name: "Original".to_string(),
            messages: Vec::new(),
            created_at: "now".to_string(),
            revision: 5,
        };
        assert!(replace_session_messages_if_revision_on_conn(&mut conn, 7, 4, &candidate).is_err());
        let revision: u64 = conn
            .query_row(
                "SELECT revision FROM sessions WHERE user_id=7 AND session_id=3",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let messages: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE user_id=7 AND session_id=3",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision, 4);
        assert_eq!(messages, 1);
    }

    #[test]
    fn append_failure_does_not_publish_partial_durable_turn() {
        let mut conn = session_test_conn();
        seed_session(&conn, 2);
        conn.execute_batch(
            "CREATE TRIGGER fail_append BEFORE INSERT ON messages
             BEGIN SELECT RAISE(ABORT, 'append failpoint'); END;",
        )
        .unwrap();
        let candidate = ChatSession {
            id: 3,
            name: "Candidate title".to_string(),
            messages: Vec::new(),
            created_at: "now".to_string(),
            revision: 2,
        };
        let appended = vec![ChatMessage {
            role: "assistant".to_string(),
            content: Value::String("answer".to_string()),
        }];
        assert!(append_session_messages_on_conn(&mut conn, 7, 2, &candidate, &appended).is_err());
        let name: String = conn
            .query_row(
                "SELECT name FROM sessions WHERE user_id=7 AND session_id=3",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let messages: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE user_id=7 AND session_id=3",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Original");
        assert_eq!(messages, 1);
    }

    #[test]
    fn stale_generation_revision_is_rejected_without_writes() {
        let mut conn = session_test_conn();
        seed_session(&conn, 9);
        let candidate = ChatSession {
            id: 3,
            name: "Original".to_string(),
            messages: Vec::new(),
            created_at: "now".to_string(),
            revision: 8,
        };
        let result = append_session_messages_on_conn(&mut conn, 7, 8, &candidate, &[]).unwrap();
        assert!(!result);
        let messages: usize = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(messages, 1);
    }

    #[test]
    fn delete_failure_leaves_session_active_state_and_counter_intact() {
        let mut conn = session_test_conn();
        seed_session(&conn, 1);
        conn.execute(
            "INSERT INTO session_counters(user_id,next_session_id) VALUES(7,4)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_delete BEFORE DELETE ON sessions
             BEGIN SELECT RAISE(ABORT, 'delete failpoint'); END;",
        )
        .unwrap();
        assert!(
            remove_session_transaction_on_conn(&mut conn, 7, 3, "Replacement", "later").is_err()
        );
        let session_count: usize = conn
            .query_row("SELECT COUNT(*) FROM sessions WHERE user_id=7", [], |row| {
                row.get(0)
            })
            .unwrap();
        let active: usize = conn
            .query_row(
                "SELECT session_id FROM active_sessions WHERE user_id=7",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let next_id: usize = conn
            .query_row(
                "SELECT next_session_id FROM session_counters WHERE user_id=7",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 1);
        assert_eq!(active, 3);
        assert_eq!(next_id, 4);
    }

    #[test]
    fn telegram_claim_crash_is_recoverable_and_completed_updates_deduplicate() {
        let mut conn = session_test_conn();
        let payload = r#"{"update_id":42,"message":{"text":"hello"}}"#;
        assert!(enqueue_telegram_update_on_conn(&mut conn, 42, payload).unwrap());
        assert!(mark_telegram_processing_on_conn(&conn, 42).unwrap());
        let claimed: (String, String) = conn
            .query_row(
                "SELECT status,payload_json FROM telegram_inbox WHERE update_id=42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(claimed.0, "processing");
        assert_eq!(claimed.1, payload);

        assert_eq!(recover_telegram_processing_on_conn(&conn).unwrap(), 1);
        let recovered: String = conn
            .query_row(
                "SELECT status FROM telegram_inbox WHERE update_id=42",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recovered, "pending");

        assert!(mark_telegram_processing_on_conn(&conn, 42).unwrap());
        assert!(mark_telegram_processed_on_conn(&conn, 42).unwrap());
        assert_eq!(recover_telegram_processing_on_conn(&conn).unwrap(), 0);
        assert!(!enqueue_telegram_update_on_conn(&mut conn, 42, payload).unwrap());
        let completed: (String, String) = conn
            .query_row(
                "SELECT status,payload_json FROM telegram_inbox WHERE update_id=42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(completed.0, "completed");
        assert!(!completed.1.contains("hello"));
    }

    #[test]
    fn telegram_pending_updates_page_past_first_500_without_replaying_queued_rows() {
        let mut conn = session_test_conn();
        for update_id in 1..=501 {
            let payload = format!(r#"{{"update_id":{update_id}}}"#);
            assert!(enqueue_telegram_update_on_conn(&mut conn, update_id, &payload).unwrap());
        }

        let first = pending_telegram_updates_after_on_conn(&conn, i64::MIN, 500).unwrap();
        assert_eq!(first.len(), 500);
        assert_eq!(first.first().map(|record| record.update_id), Some(1));
        assert_eq!(first.last().map(|record| record.update_id), Some(500));

        let second = pending_telegram_updates_after_on_conn(&conn, 500, 500).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].update_id, 501);
    }

    #[test]
    fn telegram_pending_updates_replayed_and_marked_processing_then_processed() {
        let mut conn = session_test_conn();
        assert!(enqueue_telegram_update_on_conn(&mut conn, 10, r#"{"update_id":10}"#).unwrap());
        assert!(enqueue_telegram_update_on_conn(&mut conn, 11, r#"{"update_id":11}"#).unwrap());

        let pending = pending_telegram_updates_after_on_conn(&conn, i64::MIN, 10).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].update_id, 10);
        assert_eq!(pending[1].update_id, 11);

        assert!(mark_telegram_processing_on_conn(&conn, 10).unwrap());
        let pending_after_first =
            pending_telegram_updates_after_on_conn(&conn, i64::MIN, 10).unwrap();
        assert_eq!(pending_after_first.len(), 1);
        assert_eq!(pending_after_first[0].update_id, 11);

        assert!(mark_telegram_processed_on_conn(&conn, 10).unwrap());
        assert_eq!(
            pending_telegram_updates_after_on_conn(&conn, i64::MIN, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn provider_serialization_and_debug_never_include_raw_api_key() {
        let secret = "sk-test-super-secret";
        let provider = ProviderConfig {
            id: "p1".to_string(),
            name: "Provider".to_string(),
            endpoint: "https://example.invalid/v1".to_string(),
            api_key: secret.to_string(),
            api_key_ref: Some("secret://provider/p1/ref".to_string()),
            models: vec!["model".to_string()],
            active_model: "model".to_string(),
        };
        let json = serde_json::to_string(&ProviderStore {
            active_id: Some("p1".to_string()),
            providers: vec![provider.clone()],
        })
        .unwrap();
        assert!(!json.contains(secret));
        assert!(json.contains("secret://provider/p1/ref"));
        assert!(!format!("{provider:?}").contains(secret));
    }

    #[test]
    fn legacy_bot_token_migration_is_lossless_and_removes_plaintext_only_after_reference() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        let token = "123456:test-secret-token";
        conn.execute(
            "INSERT INTO settings(key,value) VALUES('app:BOT_TOKEN',?1)",
            params![token],
        )
        .unwrap();

        let dir = std::env::temp_dir().join(format!(
            "xiaoai-secret-migration-{}-{:x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let migrated =
            migrate_legacy_secret_setting_on_conn(&mut conn, &dir, "BOT_TOKEN", "telegram", token)
                .unwrap();
        assert_eq!(migrated, token);

        let raw_count: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key='app:BOT_TOKEN'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw_count, 0);
        let secret_ref: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='app:BOT_TOKEN_REF'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(secret_ref.starts_with("secret://telegram/"));
        assert_eq!(read_secret_in_dir(&dir, &secret_ref).unwrap(), token);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_model_routing_migrates_to_main_model_defaults() {
        let (routing, needs_persist) = decode_model_routing(None);
        assert!(needs_persist);
        for role in crate::ai::routing::ModelRole::addon_roles() {
            assert_eq!(
                routing.route(role),
                Some(&crate::ai::routing::ModelRoute::MainModel)
            );
        }
    }

    #[test]
    fn valid_model_routing_does_not_request_rewrite() {
        let json =
            serde_json::to_string(&crate::ai::routing::ModelRoutingConfig::default()).unwrap();
        let (_, needs_persist) = decode_model_routing(Some(&json));
        assert!(!needs_persist);
    }

    #[test]
    fn capability_freshness_is_scoped_to_its_own_evidence() {
        let now = chrono::Utc::now().to_rfc3339();
        let old = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let record = CapabilityRecord {
            supports_text_chat: Some(true),
            supports_image_generation: Some(true),
            checked_at: now.clone(),
            evidence: vec![
                CapabilityEvidence {
                    capability: CapabilityKind::TextChat,
                    source: CapabilityEvidenceSource::ActiveProbe,
                    outcome: CapabilityState::Supported,
                    checked_at: now,
                    detail: None,
                },
                CapabilityEvidence {
                    capability: CapabilityKind::ImageGeneration,
                    source: CapabilityEvidenceSource::ActiveProbe,
                    outcome: CapabilityState::Supported,
                    checked_at: old,
                    detail: None,
                },
            ],
            ..CapabilityRecord::default()
        };
        let ttl = std::time::Duration::from_secs(7 * 24 * 60 * 60);
        assert_eq!(
            record.freshness_for(CapabilityKind::TextChat, ttl),
            EvidenceFreshness::Fresh
        );
        assert_eq!(
            record.freshness_for(CapabilityKind::ImageGeneration, ttl),
            EvidenceFreshness::Stale
        );
        assert_eq!(
            record.freshness_for(CapabilityKind::AudioTranscription, ttl),
            EvidenceFreshness::Stale
        );
    }

    #[test]
    fn source_aware_freshness_uses_each_evidence_own_ttl() {
        let now = chrono::Utc::now();
        let record = CapabilityRecord {
            evidence: vec![
                CapabilityEvidence {
                    capability: CapabilityKind::ImageInput,
                    source: CapabilityEvidenceSource::ActiveProbe,
                    outcome: CapabilityState::Unsupported,
                    checked_at: (now - chrono::Duration::days(8)).to_rfc3339(),
                    detail: None,
                },
                CapabilityEvidence {
                    capability: CapabilityKind::ImageInput,
                    source: CapabilityEvidenceSource::ProviderMetadata,
                    outcome: CapabilityState::Supported,
                    checked_at: (now - chrono::Duration::hours(2)).to_rfc3339(),
                    detail: None,
                },
            ],
            ..CapabilityRecord::default()
        };
        assert_eq!(
            record.effective_state_for(CapabilityKind::ImageInput),
            CapabilityState::Supported
        );
    }

    #[test]
    fn fresh_active_probe_remains_authoritative_when_metadata_is_stale() {
        let now = chrono::Utc::now();
        let record = CapabilityRecord {
            evidence: vec![
                CapabilityEvidence {
                    capability: CapabilityKind::ImageInput,
                    source: CapabilityEvidenceSource::ActiveProbe,
                    outcome: CapabilityState::Supported,
                    checked_at: (now - chrono::Duration::days(1)).to_rfc3339(),
                    detail: None,
                },
                CapabilityEvidence {
                    capability: CapabilityKind::ImageInput,
                    source: CapabilityEvidenceSource::ProviderMetadata,
                    outcome: CapabilityState::Unsupported,
                    checked_at: (now - chrono::Duration::hours(7)).to_rfc3339(),
                    detail: None,
                },
            ],
            ..CapabilityRecord::default()
        };
        assert_eq!(
            record.effective_state_for(CapabilityKind::ImageInput),
            CapabilityState::Supported
        );
    }

    #[test]
    fn fresh_active_probe_overrides_fresh_metadata_deterministically() {
        let now = chrono::Utc::now().to_rfc3339();
        let record = CapabilityRecord {
            evidence: vec![
                CapabilityEvidence {
                    capability: CapabilityKind::AudioInput,
                    source: CapabilityEvidenceSource::ProviderMetadata,
                    outcome: CapabilityState::Supported,
                    checked_at: now.clone(),
                    detail: None,
                },
                CapabilityEvidence {
                    capability: CapabilityKind::AudioInput,
                    source: CapabilityEvidenceSource::ActiveProbe,
                    outcome: CapabilityState::Unsupported,
                    checked_at: now,
                    detail: None,
                },
            ],
            ..CapabilityRecord::default()
        };
        assert_eq!(
            record.effective_state_for(CapabilityKind::AudioInput),
            CapabilityState::Unsupported
        );
    }

    #[test]
    fn stale_metadata_does_not_inherit_active_probe_ttl() {
        let now = chrono::Utc::now();
        let record = CapabilityRecord {
            evidence: vec![
                CapabilityEvidence {
                    capability: CapabilityKind::VideoInput,
                    source: CapabilityEvidenceSource::ProviderMetadata,
                    outcome: CapabilityState::Supported,
                    checked_at: (now - chrono::Duration::hours(7)).to_rfc3339(),
                    detail: None,
                },
                CapabilityEvidence {
                    capability: CapabilityKind::VideoInput,
                    source: CapabilityEvidenceSource::ActiveProbe,
                    outcome: CapabilityState::Supported,
                    checked_at: (now - chrono::Duration::days(8)).to_rfc3339(),
                    detail: None,
                },
            ],
            ..CapabilityRecord::default()
        };
        assert_eq!(
            record.effective_state_for(CapabilityKind::VideoInput),
            CapabilityState::Unknown
        );
    }

    #[test]
    fn unrelated_fresh_probe_cannot_refresh_other_capability_metadata() {
        let now = chrono::Utc::now();
        let record = CapabilityRecord {
            evidence: vec![
                CapabilityEvidence {
                    capability: CapabilityKind::TextChat,
                    source: CapabilityEvidenceSource::ActiveProbe,
                    outcome: CapabilityState::Supported,
                    checked_at: now.to_rfc3339(),
                    detail: None,
                },
                CapabilityEvidence {
                    capability: CapabilityKind::ImageInput,
                    source: CapabilityEvidenceSource::ProviderMetadata,
                    outcome: CapabilityState::Supported,
                    checked_at: (now - chrono::Duration::hours(7)).to_rfc3339(),
                    detail: None,
                },
            ],
            ..CapabilityRecord::default()
        };
        assert_eq!(
            record.effective_state_for(CapabilityKind::ImageInput),
            CapabilityState::Unknown
        );
        assert_eq!(
            record.effective_state_for(CapabilityKind::TextChat),
            CapabilityState::Supported
        );
    }

    #[test]
    fn legacy_capability_fields_migrate_without_granting_new_capabilities() {
        let legacy = serde_json::json!({
            "provider_id": "legacy-provider",
            "provider_name": "Legacy",
            "model": "legacy-model",
            "supports_text": true,
            "supports_image": true,
            "supports_audio": true,
            "supports_video": false,
            "supports_file_input": true
        });
        let record: CapabilityRecord = serde_json::from_value(legacy).unwrap();

        assert_eq!(record.supports_text_chat, Some(true));
        assert_eq!(record.supports_image_input, Some(true));
        assert_eq!(record.supports_audio_input, Some(true));
        assert_eq!(record.supports_video_input, Some(false));
        assert_eq!(record.supports_native_file_input, Some(true));
        assert_eq!(record.supports_image_generation, None);
        assert_eq!(record.supports_image_editing, None);
        assert_eq!(record.supports_audio_transcription, None);
    }

    #[test]
    fn legacy_supports_image_without_evidence_and_stale_timestamp_remains_stale() {
        let old = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let record = CapabilityRecord {
            supports_image_input: Some(true),
            evidence: Vec::new(),
            checked_at: old,
            ..CapabilityRecord::default()
        };
        let ttl = std::time::Duration::from_secs(7 * 24 * 60 * 60);
        assert_eq!(
            record.freshness_for(CapabilityKind::ImageInput, ttl),
            EvidenceFreshness::Stale
        );
    }

    #[test]
    fn legacy_audio_not_refreshed_by_unrelated_text_metadata() {
        let now = chrono::Utc::now().to_rfc3339();
        let record = CapabilityRecord {
            supports_audio_input: Some(true),
            evidence: vec![CapabilityEvidence {
                capability: CapabilityKind::TextChat,
                source: CapabilityEvidenceSource::ProviderMetadata,
                outcome: CapabilityState::Supported,
                checked_at: now.clone(),
                detail: None,
            }],
            checked_at: now,
            ..CapabilityRecord::default()
        };
        let ttl = std::time::Duration::from_secs(7 * 24 * 60 * 60);
        assert_eq!(
            record.freshness_for(CapabilityKind::AudioInput, ttl),
            EvidenceFreshness::Stale
        );
        assert_eq!(
            record.freshness_for(CapabilityKind::TextChat, ttl),
            EvidenceFreshness::Fresh
        );
    }

    #[test]
    fn fresh_provider_metadata_specifically_for_image_input_is_fresh() {
        let now = chrono::Utc::now().to_rfc3339();
        let record = CapabilityRecord {
            supports_image_input: Some(true),
            evidence: vec![CapabilityEvidence {
                capability: CapabilityKind::ImageInput,
                source: CapabilityEvidenceSource::ProviderMetadata,
                outcome: CapabilityState::Supported,
                checked_at: now.clone(),
                detail: Some("modalities: text,image".to_string()),
            }],
            checked_at: now,
            ..CapabilityRecord::default()
        };
        let ttl = std::time::Duration::from_secs(6 * 60 * 60);
        assert_eq!(
            record.freshness_for(CapabilityKind::ImageInput, ttl),
            EvidenceFreshness::Fresh
        );
    }

    #[test]
    fn fresh_text_chat_evidence_does_not_make_image_generation_fresh() {
        let now = chrono::Utc::now().to_rfc3339();
        let record = CapabilityRecord {
            supports_text_chat: Some(true),
            supports_image_generation: Some(true),
            evidence: vec![CapabilityEvidence {
                capability: CapabilityKind::TextChat,
                source: CapabilityEvidenceSource::ActiveProbe,
                outcome: CapabilityState::Supported,
                checked_at: now.clone(),
                detail: None,
            }],
            checked_at: now,
            ..CapabilityRecord::default()
        };
        let ttl = std::time::Duration::from_secs(7 * 24 * 60 * 60);
        assert_eq!(
            record.freshness_for(CapabilityKind::ImageGeneration, ttl),
            EvidenceFreshness::Stale
        );
    }

    #[test]
    fn stale_active_probe_for_image_generation_with_fresh_model_catalog_metadata_stays_stale() {
        let now = chrono::Utc::now().to_rfc3339();
        let old = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let record = CapabilityRecord {
            supports_image_generation: Some(true),
            evidence: vec![CapabilityEvidence {
                capability: CapabilityKind::ImageGeneration,
                source: CapabilityEvidenceSource::ActiveProbe,
                outcome: CapabilityState::Supported,
                checked_at: old,
                detail: None,
            }],
            checked_at: now,
            ..CapabilityRecord::default()
        };
        let ttl = std::time::Duration::from_secs(7 * 24 * 60 * 60);
        assert_eq!(
            record.freshness_for(CapabilityKind::ImageGeneration, ttl),
            EvidenceFreshness::Stale
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_secret_store_uses_private_directory_and_file_modes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "xiaoai-secret-modes-{}-{:x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let secret_ref = "secret://test/mode-check";
        write_secret_in_dir(&dir, secret_ref, "secret").unwrap();
        let path = secret_path_in_dir(&dir, secret_ref).unwrap();

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_secret_storage_write_aborts_without_persisting() {
        let invalid_dir = std::env::temp_dir().join(format!(
            "xiaoai-secret-parent-file-{}-{:x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(&invalid_dir, b"not a directory").unwrap();
        let secret_ref = "secret://test/fail-check";
        let res = write_secret_in_dir(&invalid_dir, secret_ref, "test");
        assert!(res.is_err());
        let _ = std::fs::remove_file(invalid_dir);
    }

    #[test]
    fn model_routing_additive_setup_preserves_custom_routes() {
        let mut config = ModelRoutingConfig::default();
        let _ = config.set_route(
            crate::ai::routing::ModelRole::Vision,
            crate::ai::routing::ModelRoute::Disabled,
        );
        let _ = config.set_route(
            crate::ai::routing::ModelRole::ImageGeneration,
            crate::ai::routing::ModelRoute::Specific {
                provider_id: "prov_custom".to_string(),
                model: "flux-pro".to_string(),
            },
        );

        // Simulate additive setup: only set routes for roles that are None
        for role in crate::ai::routing::ModelRole::addon_roles() {
            if config.route(role).is_none() {
                let _ = config.set_route(role, crate::ai::routing::ModelRoute::MainModel);
            }
        }

        assert_eq!(
            config.route(crate::ai::routing::ModelRole::Vision),
            Some(&crate::ai::routing::ModelRoute::Disabled)
        );
        assert_eq!(
            config.route(crate::ai::routing::ModelRole::ImageGeneration),
            Some(&crate::ai::routing::ModelRoute::Specific {
                provider_id: "prov_custom".to_string(),
                model: "flux-pro".to_string(),
            })
        );
        assert_eq!(
            config.route(crate::ai::routing::ModelRole::Video),
            Some(&crate::ai::routing::ModelRoute::MainModel)
        );
        assert_eq!(
            config.route(crate::ai::routing::ModelRole::AudioStt),
            Some(&crate::ai::routing::ModelRoute::MainModel)
        );
        assert_eq!(
            config.route(crate::ai::routing::ModelRole::Curator),
            Some(&crate::ai::routing::ModelRoute::MainModel)
        );
    }

    #[test]
    fn test_scoped_messages_and_threads() {
        let conn = session_test_conn();
        // Chat 100, Thread 0 (private chat)
        save_scoped_message_on_conn(&conn, 100, 0, 100, "user", "\"Halo!\"").unwrap();
        save_scoped_message_on_conn(&conn, 100, 0, 100, "assistant", "\"Hai ada apa?\"").unwrap();
        // Supergroup -100123, Thread 42
        save_scoped_message_on_conn(&conn, -100123, 42, 100, "user", "\"Topik 42\"").unwrap();
        // Supergroup -100123, Thread 99
        save_scoped_message_on_conn(&conn, -100123, 99, 100, "user", "\"Topik 99\"").unwrap();

        assert_eq!(count_scoped_messages_on_conn(&conn, 100, 0).unwrap(), 2);
        assert_eq!(
            count_scoped_messages_on_conn(&conn, -100123, 42).unwrap(),
            1
        );
        assert_eq!(
            count_scoped_messages_on_conn(&conn, -100123, 99).unwrap(),
            1
        );

        let msgs = load_scoped_messages_on_conn(&conn, 100, 0, 10).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, Value::String("Halo!".to_string()));
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content, Value::String("Hai ada apa?".to_string()));

        let thread_msgs = load_scoped_messages_on_conn(&conn, -100123, 42, 10).unwrap();
        assert_eq!(thread_msgs.len(), 1);
        assert_eq!(
            thread_msgs[0].content,
            Value::String("Topik 42".to_string())
        );
    }

    #[test]
    fn test_user_memories_crud() {
        let conn = session_test_conn();
        let user_id = 999;
        assert!(get_user_memories_on_conn(&conn, user_id)
            .unwrap()
            .is_empty());

        save_user_memory_on_conn(&conn, user_id, "Name", "Alice").unwrap();
        save_user_memory_on_conn(&conn, user_id, "Language", "Rust").unwrap();

        let memories = get_user_memories_on_conn(&conn, user_id).unwrap();
        assert_eq!(memories.len(), 2);
        assert_eq!(memories[0], ("Language".to_string(), "Rust".to_string()));
        assert_eq!(memories[1], ("Name".to_string(), "Alice".to_string()));

        // Upsert
        save_user_memory_on_conn(&conn, user_id, "Language", "Rust & Go").unwrap();
        let memories = get_user_memories_on_conn(&conn, user_id).unwrap();
        assert_eq!(memories.len(), 2);
        assert_eq!(
            memories[0],
            ("Language".to_string(), "Rust & Go".to_string())
        );

        // Delete single
        delete_user_memory_on_conn(&conn, user_id, "Language").unwrap();
        let memories = get_user_memories_on_conn(&conn, user_id).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0], ("Name".to_string(), "Alice".to_string()));

        // Clear all
        clear_user_memories_on_conn(&conn, user_id).unwrap();
        let memories = get_user_memories_on_conn(&conn, user_id).unwrap();
        assert!(memories.is_empty());
    }

    #[test]
    fn test_scoped_summary_crud() {
        let conn = session_test_conn();
        assert_eq!(get_scoped_summary_on_conn(&conn, 100, 0).unwrap(), None);

        save_scoped_summary_on_conn(&conn, 100, 0, "Discussed Rust async patterns").unwrap();
        assert_eq!(
            get_scoped_summary_on_conn(&conn, 100, 0).unwrap(),
            Some("Discussed Rust async patterns".to_string())
        );

        // Different thread
        assert_eq!(get_scoped_summary_on_conn(&conn, 100, 42).unwrap(), None);
        save_scoped_summary_on_conn(&conn, 100, 42, "Topic 42 summary").unwrap();
        assert_eq!(
            get_scoped_summary_on_conn(&conn, 100, 42).unwrap(),
            Some("Topic 42 summary".to_string())
        );

        // Upsert
        save_scoped_summary_on_conn(&conn, 100, 0, "Updated summary").unwrap();
        assert_eq!(
            get_scoped_summary_on_conn(&conn, 100, 0).unwrap(),
            Some("Updated summary".to_string())
        );
    }

    #[test]
    fn test_topic_scope_helpers() {
        let private_scope = TopicScope::new(12345, 0);
        assert!(private_scope.is_private());
        assert_eq!(private_scope.chat_id, 12345);
        assert_eq!(private_scope.thread_id, 0);

        let group_topic_scope = TopicScope::new(-1001234567, 99);
        assert!(!group_topic_scope.is_private());
        assert_eq!(group_topic_scope.chat_id, -1001234567);
        assert_eq!(group_topic_scope.thread_id, 99);
    }
}
