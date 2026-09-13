# xiao — Rust Telegram AI Assistant

xiao is a standalone asynchronous Rust application using the **Telegram Bot API 10.3 subset required by xiao**, Rich Message AST formatting, cancellable streaming drafts, SQLite sessions, and multi-provider OpenAI-compatible AI routing. Do not describe the client as a full Bot API implementation.

## Working Context

This is a single Rust 2021 binary crate, not a workspace or library. `AGENT.md` redirects here; `GOAL.md` records the flat, interactive CLI design intent, not proof that every planned behavior is implemented. Check the dispatcher in `src/main.rs` and implementations in `src/cli.rs` when changing commands.

## Required Quality Gates

Run from the repository root before declaring a change ready:

- `cargo fmt --all -- --check`
- `cargo check --locked`
- `cargo test --locked`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo audit --deny warnings`
- `cargo build --release --locked`

`Cargo.lock` is tracked. Preserve `--locked` for validation; dependency changes must deliberately update the lockfile rather than silently resolving new versions during checks.

`.github/workflows/build.yml` is the executable CI reference: Ubuntu 22.04, Rust **1.98.0**, rustfmt/clippy, and `cargo-audit` **0.22.2** (installed with `cargo install cargo-audit --version 0.22.2 --locked`). README's Rust >=1.80 prerequisite is not a manifest-declared MSRV; do not treat it as verified compatibility. SQLite is bundled, so building requires a C toolchain; reqwest uses rustls rather than default TLS. Release builds enable LTO and `panic = "abort"`.

### Cross-builds and artifacts

Both artifact jobs depend on quality and security checks; the workflow uploads binaries rather than deploying a daemon.

- Linux ARM64: install the `aarch64-unknown-linux-gnu` Rust target and ARM64 GCC/libc cross-toolchain, set `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc`, then run `cargo build --release --locked --target aarch64-unknown-linux-gnu`. Artifact: `target/aarch64-unknown-linux-gnu/release/xiao`.
- Android ARM64: CI uses JDK 17, Android SDK, NDK **26.3.11579264**, Rust target `aarch64-linux-android`, and `cargo-ndk` **4.1.2**. Build with `cargo ndk -t arm64-v8a -P 26 build --release --locked`. Artifact: `target/aarch64-linux-android/release/xiao`.

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

The local release executable is `target/release/xiao`. With no subcommand it starts the daemon; unknown commands exit with status 1. **Even `help` constructs `AIChatService` before command dispatch**, which can initialize or migrate configuration. Do not use CLI invocations as harmless smoke tests against a real user's HOME. `probe` includes live provider tests, not just local inspection.

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
13. Provider API keys and `BOT_TOKEN` must persist through the SecretStore reference abstraction, never as ordinary plaintext config values. Do not claim the local file SecretStore is encrypted or equivalent to an OS keyring.
14. External image fallback is opt-in only (`IMAGE_FALLBACK_PROVIDER=pollinations`).
15. Main Model owns canonical history. Different-provider Vision/Video/STT specialists receive only the current media/question and return bounded execution artifacts to Main.
16. Addon routes are persisted separately from ProviderStore as Main Model / Specific / Disabled. Telegram may edit Main only; addon configuration is CLI-only in v0.3.0.
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

### Hidden module boundaries and change entry points

- `src/ai/capability.rs` contains capability inference; `src/ai/tools.rs` implements web search/fetch. `service.rs` also re-exports types from capability/routing/storage, so locate the definition before changing a type exposed through service.
- `src/bot/client.rs` is a wrapper with its own HTTP client that dereferences to the raw implementation in `src/bot/client/raw.rs`. The latter is declared as sibling module `bot::client_raw`, not `bot::client::raw`. Transport changes may need both layers; shared policies live in `src/bot/transport_policy.rs` and `src/bot/url_policy.rs`.
- `src/bot/models.rs` selectively re-exports `src/bot/models/base.rs`. Wire models use explicit serde discriminators and omit optional fields rather than emitting null. Validate serialized payloads, not just Rust variant names.
- `src/parser.rs` wraps `src/parser/markdown.rs` and repairs the Bot API 10.3 voice-note discriminator after parsing. Use the facade; bypassing it skips compatibility normalization.

### Control and data flow

1. Startup loads dotenv and constructs the service before CLI dispatch. Daemon startup then validates owner/token and Telegram connectivity.
2. Telegram updates enter the durable SQLite inbox with an advancing polling offset. Abandoned processing records are recovered and pending updates replayed before new polling.
3. Intake routes updates to control or generation workers; native Stop has a separate cancellation path. Classification in `main.rs` depends on wizard/rename state as well as message/callback type. Review the classifier when adding commands or callbacks, not only `handle_update`.
4. `AIChatService` coordinates originating session/revision, provider capability checks, addon execution, and canonical history. Streaming provider bytes pass through `ai/stream.rs`; `timeline.rs` and the parser produce draft/final presentation through the Telegram client.
5. Persistence commits precede in-memory publication. Stored message content may be a string or a versioned attachment-reference object; it is not necessarily a ready-to-send provider message payload.

Both lane queues are bounded at 64 and have sequential workers. Queue sends are awaited, so backpressure can delay intake of a later Stop even though Stop bypasses queues once encountered. Do not claim an unconditional cancellation-latency guarantee.

Delivery context (thread, ephemeral receiver, reply metadata) is task-local and scoped around update handling. New spawned work must preserve/re-establish the appropriate context; it is not carried by ordinary send-method arguments alone.

## Telegram 10.3 Notes

The currently modeled 10.3 surface includes native generation stop (`can_stop`, `keep_on_stop`, `stopped_message_generation`), disabled buttons, Rich Message buttons, expandable quotations, document blocks, compact tables, `force_reply`, and `EphemeralMessageParameters` where used by the client.

Execution/status UI must describe observable application state. Do not infer fake tool execution such as “Searching”, “Testing”, or “Coding” solely from prompt/reasoning keywords.

## Persistence Notes

SQLite is the runtime source of truth for sessions, active session identity, non-secret settings, provider/model selections, and migration markers. Secret settings persist as `secret://...` references whose material is stored separately by the local SecretStore. Legacy JSON/.env/plaintext rows may be imported for compatibility only after the replacement secret is durably written; do not delete the legacy value first.

### Configuration and filesystem gotchas

- Data lives under `$HOME/.local/share/xiaoai/`: `xiaoai.db`, `secrets/`, and `attachments/<user_id>/<session_id>/`. The legacy `xiaoai` directory name remains intentional in code despite the `xiao` binary name. This path uses HOME, not XDG; absent HOME falls back to the working directory. SQLite uses WAL and a 5000 ms busy timeout.
- SecretStore uses separate local files with Unix directory/file modes 0700/0600. Base64-encoded reference filenames are not encryption. Do not inspect or include users' secret files in diagnostics.
- `main()` first calls `dotenvy::dotenv()`. Later explicit config lookup selects the first existing file among working-directory `.env`, `$HOME/.xiao.env`, `$HOME/xiao/.env`, and `$HOME/XiaoAI/.env`. The implemented path is `xiao/.env`, not the `.xiao/.env` mentioned in a nearby source comment. A controlled HOME alone does not isolate working-directory/ancestor dotenv discovery.
- Token lookup prefers a usable environment value, then SQLite. Owner/chat settings prefer environment presence even when empty or invalid; an invalid `OWNER_USER_ID` can mask a valid stored owner. Startup imports selected nonempty environment settings only when the stored setting is missing.
- `save_env_kv` and `save_token_to_env` write through application storage, despite their names. Provider configuration loads SQLite first; legacy JSON is migration input. Editing environment variables is not equivalent to replacing an already stored provider selection.
- Persisted attachments are individually bounded to 20 MiB. Scanned-PDF rendering requires `pdftoppm` on PATH and an authorized Vision route; rendering is capped at six pages. Do not treat the optional renderer as a Rust dependency.
- Timeout settings are documented in `.env.example`. `IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS` controls image downloads, while general AI requests use `AI_PROVIDER_CONNECT_TIMEOUT_SECS`. Timeout parsing rejects zero/invalid values in favor of defaults and caps at 600 seconds. Image cancellation/timeouts do not trigger external fallback; opt-in Pollinations fallback sends the prompt to that service.

## Testing Approach

Most tests are colocated `#[cfg(test)]` modules; there is no library target. Useful focused commands before the full gates:

```sh
cargo test --locked --bin xiao ai::storage::tests::
cargo test --locked --bin xiao ai::provider::tests::
cargo test --locked --bin xiao parser::
cargo test --locked --bin xiao update_lane_tests::
cargo test --locked --bin xiao image_delivery_
cargo test --locked --test bot_api_10_3_contract
```

- Discover exact names with `cargo test --locked --bin xiao -- --list`. Async delivery tests inject sender closures and counters; storage tests use in-memory SQLite and SQL `RAISE(ABORT)` triggers for rollback failures. Secret tests use explicit temporary directories; document tests build synthetic ZIP payloads.
- Storage separates connection-taking `_on_conn` operations, connection-owning `_db` functions, and `_async` wrappers. Follow these seams for testable mutations. Hand-built test schemas do not exercise production schema initialization; update fixtures deliberately when changing schema-dependent behavior.
- New tests that call service constructors or production loaders can create/migrate real state. Prefer injected connections/directories. When production paths are necessary, use a disposable process-level HOME, clean configuration environment, and controlled working directory; serial execution alone does not isolate disk state.
- `tests/bot_api_10_3_contract.rs` imports models/parser/SSE directly with `#[path]`, so their colocated tests also compile into that integration target. It does not start the bot or prove live Telegram compatibility.
- Some contract tests inspect exact source strings and function order with `include_str!`, including transport safety checks. Structural refactors can fail these tests without changing behavior; review those assertions together with implementation changes without weakening the protected behavior.
- For wire/formatting changes, follow existing tests that assert both typed Rich AST and serialized JSON, especially discriminators, omitted fields, streaming fragments, and semantic fallbacks.

## Runtime Boundaries

Bot-time SQLite work must go through the async wrappers in `ai/storage.rs` so blocking `rusqlite` calls do not run on Tokio workers. Provider configuration is single-owner global state; do not reintroduce pseudo-multi-user provider maps. Capability claims remain tri-state and record diagnostic evidence, but never act as runtime authorization locks for configured routes. Active probing is optional and non-blocking.

Telegram ingestion is a durable inbox. After intake, updates are classified into a responsive **control lane** and serialized **generation lane**; native Stop bypasses both for immediate cancellation. Inbox delivery semantics are explicitly **at-least-once**: retain a claimed payload until the completed checkpoint and recover abandoned `processing` rows after restart. Do not document this as exactly-once.

Streaming drafts must render stable Markdown through native Rich blocks while sanitizing incomplete provisional syntax. The permanent final is emitted once from the canonical AST. Final fallback is Rich → safe HTML → AST-derived semantic plain text, never raw model Markdown.
