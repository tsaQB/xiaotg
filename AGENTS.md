# AGENTS.md

Practical guide for AI coding agents working on the `xiao` codebase.

---

## 1. Project Overview & Technology Stack

`xiao` is an asynchronous daemon and CLI tool written in Rust (2021 edition) implementing a hardened, single-owner AI assistant gateway for **Telegram Bot API 10.3** with OpenAI-compatible backend providers.

- **Language & Runtime**: Rust 1.80+ (CI uses 1.98.0, 2021 edition), Tokio async runtime (`full` features).
- **Core Crates**: `reqwest` (rustls-tls, stream, multipart), `rusqlite` (bundled SQLite), `serde`/`serde_json`, `crossterm` (interactive TUI), `lopdf` + `zip` (document extraction), `tracing`/`tracing-subscriber`.
- **Target Platforms**: Linux x86_64, Linux ARM64 (`aarch64-unknown-linux-gnu`), Android Termux (`aarch64-linux-android`).
- **Data Directory**: `~/.local/share/xiaoai/` (database, secrets, attachments).

---

## 2. Essential Commands

### Build & Compilation
```bash
# Debug build
cargo build

# Release build (enforces locked Cargo.lock; LTO enabled, strip enabled in release profile)
cargo build --release --locked

# Fast syntax and type checking
cargo check --locked
cargo check --release --locked
```

### Testing
```bash
# Run all tests (unit + contract integration tests; currently 221+ passing tests)
cargo test --locked

# Run unit tests only
cargo test --bin xiao --locked

# Run Bot API 10.3 contract integration tests only
cargo test --test bot_api_10_3_contract --locked

# Run a specific test by name (with output)
cargo test <test_name_substring> --locked -- --nocapture
```

### Quality Gates (Required by CI)
All pull requests and commits must pass these gates with zero errors and zero warnings:
```bash
# 1. Formatting check
cargo fmt --all -- --check

# 2. Compilation check
cargo check --locked

# 3. Test suite
cargo test --locked

# 4. Clippy linter (-D warnings is strictly enforced)
cargo clippy --locked --all-targets --all-features -- -D warnings
```

### CLI Subcommands (Local Invocation)
The binary supports both interactive management and headless daemon modes:
```bash
cargo run -- start               # Run the bot daemon in foreground (default subcommand)
cargo run -- setup               # 2-step interactive configuration wizard (AI -> Telegram)
cargo run -- status              # Display system, database, and provider status
cargo run -- gateway             # Manage Telegram Bot Token and OWNER_USER_ID
cargo run -- provider [add|rm]   # Manage OpenAI-compatible AI providers
cargo run -- model [query]       # Search and set Main Model
cargo run -- addon               # Configure multimodal specialist routes (Vision, Video, STT, Image)
cargo run -- pick                # Configure model whitelist for Telegram /model picker
cargo run -- probe               # Live capability diagnostic center and probe tester
cargo run -- help                # Print CLI subcommand reference
```

---

## 3. Architecture & Data Flow

```text
                           Telegram Bot API 10.3 Long Polling
                                         │
                                         ▼
                     ┌───────────────────────────────────────┐
                     │    SQLite Durable Inbox Intake Queue  │
                     │          (telegram_inbox)             │
                     └───────────────────┬───────────────────┘
                                         │
               ┌─────────────────────────┼─────────────────────────┐
               │                         │                         │
     stopped_message_generation    Control Updates        Generation Updates
      (Native Stop Bypasses Queues)      │                         │
               │                         ▼                         ▼
               │                tokio::mpsc channel       tokio::mpsc channel
               │                 (Control Worker)        (Generation Worker)
               │                         │                         │
               └─────────────────────────┼─────────────────────────┘
                                         ▼
                               process_durable_update
                                         │
          ┌──────────────────────────────┼──────────────────────────────┐
          │                              │                              │
     Bot Commands                 AI Chat Route                  /image Prompt
    (/start, /help,             (Text & Multimodal)           (Text-to-Image Flow)
   /clear, /new, etc.)                   │                              │
          │                              ▼                              ▼
          │                     AIChatService Core              Image Generation
          │                      ├── Session Lock                      │
          │                      ├── Timeline & Draft Ticker           │
          │                      ├── Tool Loop (Search/Fetch)          │
          │                      └── OpenAI Streaming (SSE)            │
          │                                                            │
          └──────────────────────────────┬─────────────────────────────┘
                                         ▼
                             Telegram Client Transport
                               (sendRichMessage,
                           sendRichMessageDraft, etc.)
```

### Component Breakdown
- **`src/main.rs`**: Daemon entrypoint, signal handling, update intake loop, two-lane classification (`classify_update_lane`), command handlers, and UI builders for Telegram Rich Messages.
- **`src/bot/`**: Telegram client implementation.
  - `models/base.rs` & `models.rs`: Strongly typed Telegram Bot API 10.3 models (`InputRichMessage`, `RichBlock`, `Update`, `EphemeralMessageParameters`).
  - `client/raw.rs` & `client.rs`: Low-level and high-level HTTP client wrapping Telegram Bot API with retry policies and context propagation.
  - `transport_policy.rs`: Retry logic (429 with `retry_after`, 5xx backoff, 400 bad request detection).
  - `url_policy.rs`: SSRF protection, IP address resolution, and egress sanitization for media downloads.
- **`src/ai/`**: AI engine and backend integration.
  - `service.rs`: `AIChatService` managing session lifecycle, generation locks, SSE stream accumulation, audio formats, and image generation.
  - `routing.rs`: 5-role model routing (`Main`, `Vision`, `Video`, `AudioStt`, `ImageGeneration`) and snapshot generation.
  - `storage.rs`: SQLite persistence, migrations, schema definition, secret reference manager, and update queue operations.
  - `provider.rs`: OpenAI-compatible HTTP payloads, multi-modal part assembly, and capability probe execution.
  - `capability.rs`: Model capability heuristic catalog, metadata enrichment, and token limits.
  - `tools.rs`: Autonomous multi-tier web search (Brave -> Tavily -> Exa -> DDG -> Wikipedia) and `fetch_url` HTML cleaner.
  - `stream.rs`: Bounded SSE line/event parser preserving multi-byte UTF-8 across network chunk boundaries.
  - `http.rs`: Provider HTTP retry policy and exponential backoff calculations.
- **`src/timeline.rs`**: Real-time execution timeline and draft updater (`sendRichMessageDraft`) with periodic background status ticker.
- **`src/parser/`**: Markdown AST to Telegram Bot API 10.3 `InputRichMessage` block transformer (`src/parser/markdown.rs`).
- **`src/attachments.rs`**: Local filesystem storage and retrieval for session media attachments.
- **`src/document.rs`**: Multi-format document parser (Text/Code, PDF native + poppler scan rendering, DOCX, XLSX).
- **`src/cli.rs`**: Interactive terminal TUI using `crossterm` for headless setup, status, and configuration.

---

## 4. Critical Invariants & Gotchas

### 1. Telegram Bot API 10.3 Strict Contract
- **Never emit `draft_id: 0`**: In Telegram Bot API 10.3, permanent message sends (`sendMessage`, `sendRichMessage`, etc.) must **never** include `draft_id`. Sentinels like `"draft_id": 0` cause rejection. This is guarded by static source inspection tests in `tests/bot_api_10_3_contract.rs`.
- **Drafts are Private-Chat Only**: `sendRichMessageDraft` is only supported in private chats. In group chats, draft calls are rejected by Telegram. `timeline.draft_enabled` is set to `false` for non-private chats, skipping intermediate draft calls while preserving final message delivery.
- **Voice Note Wire Discriminator**: The wire JSON representation for `InputMedia::VoiceNote` and nested rich blocks must strictly use `"type": "voice_note"`.
- **Media Group (Album) Constraints**:
  - Albums require between 2 and 10 items.
  - Photos and Videos can be mixed together.
  - Audio and Documents must be sent in separate, homogeneous albums (cannot mix Audio with Photos/Docs).
  - Animations and Voice Notes can **never** be included in albums.

### 2. Native Stop Security & Queuing Invariant
- Telegram sends `stopped_message_generation` when the user taps the Stop button.
- **Queue Bypass**: Native Stop bypasses both the `generation` and `control` tokio channels to execute cancellation immediately without waiting in line.
- **Owner-Only Guard**: Because `MessageGenerationStopped` updates omit the sender's user ID, `access.allows_stop_chat(chat_id)` restricts native Stop handling exclusively to the owner's private chat (`chat_id == owner_user_id`). This prevents unprivileged group chat members from cancelling the owner's requests.

### 3. Session Isolation & Monotonic Revisioning
- Sessions use monotonic sequential IDs (`session_id`) tracked via `session_counters`. Deleted session IDs are never reused.
- Every session tracks an integer `revision`. Calling `/clear` increments this revision.
- When an asynchronous OpenAI stream finishes or yields tokens, it validates `generation_revision_matches(session, session_id, revision)`. If the user switched sessions or cleared history during generation, late chunks are discarded without writing to SQLite (*zero cross-session bleed*).

### 4. Secret Isolation (`secret://...`)
- API keys, provider credentials, and bot tokens are **never** stored in plaintext inside the SQLite database or emitted into debug logs.
- Secrets are written to disk under `~/.local/share/xiaoai/secrets/` with strict Unix permissions (`0700` dir, `0600` file).
- The SQLite `settings` table only references keys using a URI scheme: `secret://...`.
- `ProviderConfig::fmt` redacts keys as `<empty>` or `<redacted>`.

### 5. Media SSRF & Downloader Hardening
- All external media download URLs (e.g. image generation fallbacks, `fetch_url`) pass through `bot::url_policy::resolve_download_url`.
- Blocked target addresses include: loopback, private IPv4/IPv6, link-local, carrier-grade NAT (`100.64.0.0/10`), documentation IPs, multicast, and IPv4-mapped IPv6 ranges.
- Downloader client explicitly enforces `reqwest::redirect::Policy::none()` (preventing redirect-based SSRF pivoting) and `.no_proxy()` (preventing proxy bypasses).

### 6. OpenAI Endpoint Compatibility & Tool Fallback
- Many local model servers (e.g., Ollama, vLLM, text-generation-webui) reject requests containing the `tools` parameter with `HTTP 400 Bad Request`.
- The chat service intercepts HTTP 400 errors during tool-enabled chat and automatically retries the request without `tools`. When modifying provider calling code, never remove this fallback path.

### 7. 5-Role Multimodal Model Routing Rules
- The 5 roles are `Main`, `Vision`, `Video`, `AudioStt`, and `ImageGeneration`.
- In the Telegram UI (`/model`), only the **Main Model** can be modified. Addon roles are read-only in Telegram and must be configured via the CLI (`xiao addon`).
- An addon route configured as `MainModel` dynamically tracks changes to the Main Model. An addon configured as `Specific` or `Disabled` is never overwritten when the Main Model changes.
- Specialist invocations send only the immediate user input and relevant media to the specialist model; they do not send canonical session history to the specialist. Canonical history is maintained solely with the Main Model.

### 8. Enhanced Markdown AST & Bot API 10.1 - 10.3 Rich Entities
- Markdown links support Telegram in-app deep links via `tg://` (such as `tg://document?id=...`) and document blocks via `[document: ...]` or `<tg-document ...>`.
- Inline formatting entities support `||spoiler||`, `~~strikethrough~~`, `<u>underline</u>` / `++underline++`, and expandable quotes `**>...` / `<blockquote expandable>`.
- Tables default to `is_compact: true` with optional title captions via `[table: Caption]` / `[caption: Caption]`.

---

## 5. Storage & Database Schema

The SQLite database is located at `~/.local/share/xiaoai/xiaoai.db` and runs with `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;`.

Key tables:
- `settings`: Key-value store for app configuration, hydrated provider stores, capability cache, and model routing.
- `sessions`: Monotonic session records (`user_id`, `session_id`, `name`, `created_at`, `updated_at`, `revision`).
- `messages`: Canonical chat history per session (`user_id`, `session_id`, `role`, `content`, `created_at`).
- `active_sessions`: Active session pointer per user (`user_id`, `session_id`).
- `session_counters`: Monotonic session ID counter (`user_id`, `next_session_id`).
- `telegram_inbox`: Durable intake queue for Telegram updates (`update_id`, `payload_json`, `status`, `attempts`, `received_at`, `last_error`).
- `telegram_state`: Key-value tracking for Telegram polling offsets and runtime state.

---

## 6. Testing Patterns & Guidelines

1. **Unit Tests in Modules**: Unit tests live directly inside `#[cfg(test)] mod tests` within each respective module file.
2. **Contract Tests**: Protocol contracts and invariants live in `tests/bot_api_10_3_contract.rs`. When introducing or updating Telegram models, add contract validation tests here.
3. **Source Inspection Assertions**: Some tests verify architectural invariants by inspecting source files at test time (e.g., verifying that no permanent API send function contains `"draft_id": 0`). If you refactor client code, be mindful of these static checks.
4. **Always Run Clippy**: When making modifications, always run `cargo clippy --locked --all-targets --all-features -- -D warnings` before concluding your work.
