# xiao — Rust Telegram AI Assistant

xiao is a standalone asynchronous Rust application using the **Telegram Bot API 10.3 subset required by xiao**, Rich Message AST formatting, cancellable streaming drafts, SQLite sessions, and multi-provider OpenAI-compatible AI routing. Do not describe the client as a full Bot API implementation.

## Required Quality Gates

Run before declaring a change ready:

- `cargo fmt --all -- --check`
- `cargo check --locked`
- `cargo test --locked`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo audit --deny warnings`
- `cargo build --release --locked`

## Essential CLI Commands

- `xiao start` — Run Telegram bot daemon
- `xiao setup` — 2-step setup wizard (AI Provider -> Gateway)
- `xiao status` — System health & routing status dashboard
- `xiao gateway` — Interactive chat gateway manager (Telegram token & owner)
- `xiao provider` — Interactive AI provider manager (list, switch, add, rm)
- `xiao model [query]` — Interactive Main Model selector with live search
- `xiao pick` — Interactive Telegram menu model whitelist picker (max 10)
- `xiao addon` — Interactive multimodal specialist routing (Vision, Video, Audio STT, Image Gen)
- `xiao probe` — Interactive capability diagnostic & live testing center
- `xiao help` — Display official command guide

## Security & Session Invariants

1. `OWNER_USER_ID` is required. Only that Telegram user may operate xiao; `ALLOWED_CHAT_IDS` extends where the owner may use the bot, not who may use it.
2. Never log a URL that contains `BOT_TOKEN`.
3. Session identity is the stable SQLite `session_id`, never a vector/list index.
4. New session IDs must come from the persistent high-water sequence and must not reuse deleted IDs.
5. Every AI request captures its originating `session_id`; late output must never be written into whichever session is active later.
6. Every generation captures the originating session revision. Clear/reset increments it durably; append must compare the captured revision in SQLite before writing.
7. If the originating session is deleted while generation is running, discard the late persistence write. Switching active session must never redirect the late output.
8. Public persistence mutations are durable-first: commit SQLite before publishing RAM; delete attachments/secrets only after durable commit.
9. User generations are serialized so concurrent prompts cannot reorder the same owner's history.
10. Telegram `stopped_message_generation` must cancel the matching `(chat_id, draft_id)` provider stream.
11. Binary documents must use explicit bounded extractors. Never reinterpret PDF/DOCX/XLSX bytes as UTF-8; scanned PDFs must use the render-to-vision path.
12. Multimodal capability is fail-closed: runtime media authorization requires effective `Supported` state from fresh, capability-scoped evidence. A raw legacy `Some(true)` field alone never authorizes media. Unknown, missing, or stale evidence must be probed safely or rejected.
13. Provider API keys and `BOT_TOKEN` must persist through the SecretStore reference abstraction, never as ordinary plaintext config values. Do not claim the local file SecretStore is encrypted or equivalent to an OS keyring.
14. External image fallback is opt-in only (`IMAGE_FALLBACK_PROVIDER=pollinations`).
15. Main Model owns canonical history. Different-provider Vision/Video/STT specialists receive only the current media/question and return bounded execution artifacts to Main.
16. Addon routes are persisted separately from ProviderStore as Main Model / Specific / Disabled. Telegram may edit Main only; addon configuration is CLI-only in v0.3.0.
17. Capability freshness is per evidence item and source: UserOverride > ActiveProbe > KnownProviderProfile > ProviderMetadata. ProviderMetadata expires after 6 hours, ActiveProbe after 7 days, KnownProviderProfile after 30 days, and UserOverride remains authoritative until explicitly replaced/removed (but still requires a valid non-future evidence timestamp). A new text/vision probe must not refresh unrelated image-generation/STT evidence, and stale metadata never inherits an ActiveProbe TTL. Unknown or stale evidence remains fail-closed.
18. Image generation must resolve the Image Generation Model, propagate the actual model in provider requests, use bounded configurable timeouts, validate all returned image bytes/URLs, and preserve explicit fallback opt-in.
19. Provider protocol scope is the Xiao-supported OpenAI-compatible subset: Image Generation uses `POST /images/generations` (or explicit Pollinations fallback), Audio STT uses `POST /audio/transcriptions`, and Chat/Vision/Video/Audio uses `POST /chat/completions` with bounded media payload shapes. Protocol/sample-shape mismatches (including 404/405 and generic HTTP 415) remain Unknown/ProtocolMismatch; only an unambiguous model/modality rejection may become Unsupported.

## Architecture

```text
src/
├── main.rs         # access policy, durable inbox routing, control/generation lanes
├── cli.rs          # terminal setup/provider/model/Telegram commands
├── document.rs     # bounded PDF/DOCX/XLSX extraction + scanned-PDF rendering
├── attachments.rs  # per-session multimodal persistence
├── util.rs         # Unicode-safe truncation and HTML escaping
├── bot/
│   ├── client.rs   # Telegram HTTP client + 10.3 methods used by xiao
│   └── models.rs   # Telegram/Rich Message serde models
├── ai/
│   ├── provider.rs # single-owner provider state + capability probes
│   ├── routing.rs  # five model roles + durable addon route representation
│   ├── storage.rs  # SQLite persistence / blocking boundary
│   ├── stream.rs   # SSE state machine
│   ├── http.rs     # retry/backoff policy
│   └── service.rs  # chat/session orchestration, STT/image
├── parser.rs       # Markdown -> Telegram Rich Message AST
└── timeline.rs     # Cancellable/streaming draft presentation state
```

## Telegram 10.3 Notes

The currently modeled 10.3 surface includes native generation stop (`can_stop`, `keep_on_stop`, `stopped_message_generation`), disabled buttons, Rich Message buttons, expandable quotations, document blocks, compact tables, `force_reply`, and `EphemeralMessageParameters` where used by the client.

Execution/status UI must describe observable application state. Do not infer fake tool execution such as “Searching”, “Testing”, or “Coding” solely from prompt/reasoning keywords.

## Persistence Notes

SQLite is the runtime source of truth for sessions, active session identity, non-secret settings, provider/model selections, and migration markers. Secret settings persist as `secret://...` references whose material is stored separately by the local SecretStore. Legacy JSON/.env/plaintext rows may be imported for compatibility only after the replacement secret is durably written; do not delete the legacy value first.

## Runtime Boundaries

Bot-time SQLite work must go through the async wrappers in `ai/storage.rs` so blocking `rusqlite` calls do not run on Tokio workers. Provider configuration is single-owner global state; do not reintroduce pseudo-multi-user provider maps. Capability claims must remain tri-state and identify metadata/probe evidence.

Telegram ingestion is a durable inbox. After intake, updates are classified into a responsive **control lane** and serialized **generation lane**; native Stop bypasses both for immediate cancellation. Inbox delivery semantics are explicitly **at-least-once**: retain a claimed payload until the completed checkpoint and recover abandoned `processing` rows after restart. Do not document this as exactly-once.

Streaming drafts must render stable Markdown through native Rich blocks while sanitizing incomplete provisional syntax. The permanent final is emitted once from the canonical AST. Final fallback is Rich → safe HTML → AST-derived semantic plain text, never raw model Markdown.
