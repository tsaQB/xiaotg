# xiao

xiao adalah bot Telegram asynchronous berbasis Rust untuk endpoint AI yang kompatibel dengan OpenAI. xiao v0.3.0 menambahkan **role-based multimodal model routing, observable capability discovery, model-aware image generation, dan native Rich command UI Telegram 10.3** di atas hardening session/security 0.2.0.

> xiao tidak mengklaim mengimplementasikan seluruh Telegram Bot API. Client hanya memodelkan method dan update yang dibutuhkan aplikasi.

## Fitur Utama

- Rich Message Telegram dengan AST heading, paragraph, list, quote, code, math, divider, table, buttons, document block, dan expandable quotation.
- Streaming draft jawaban AI melalui `sendRichMessageDraft`, termasuk partial answer selama provider masih menghasilkan respons.
- Native generation stop Bot API 10.3 di private chat owner: draft memakai `can_stop`/`keep_on_stop`, lalu update `stopped_message_generation` membatalkan stream provider aktif. Di group, Stop tidak diaktifkan karena event stop tidak membawa identitas user penekan tombol.
- Native disabled inline button dan Rich Message buttons Bot API 10.3.
- Session persisten di SQLite dengan stable `session_id`, monotonic ID allocation, dan durable `revision`/generation epoch. Rename/clear/remove/switch/append mempublikasikan perubahan ke RAM hanya setelah transaksi SQLite berhasil.
- Telegram update masuk ke durable inbox, lalu diklasifikasikan ke **control lane** atau **generation lane**. Generation owner tetap diserialisasi, control lane tetap responsif, dan native Stop membypass kedua queue untuk cancellation segera.
- Provider OpenAI-compatible, discovery model `/models`, STT `/audio/transcriptions`, SSE chat completion, dan image generation.
- Owner-only authorization melalui `OWNER_USER_ID`; `ALLOWED_CHAT_IDS` hanya mengatur chat tambahan tempat owner boleh menggunakan xiao.
- Batas download media dan fallback image eksternal yang default-nya nonaktif.
- Unicode-safe truncation/prefix handling serta HTML escaping untuk data dinamis yang masuk ke `parse_mode=HTML`.


### Routing model v0.3.0

Xiao mempunyai lima role:

- **Main Model** — chat/final answer dan pemilik canonical history.
- **Vision Model** — image dan halaman PDF scan.
- **Video Model** — video understanding.
- **Audio STT Model** — native Main audio atau transcription route.
- **Image Generation Model** — text-to-image.

Empat addon memakai tepat tiga state: **Main Model**, **Specific Model**, atau **Disabled**. Default adalah Main Model. `Main Model` adalah referensi hidup: mengganti Main otomatis memengaruhi addon yang memakai Main, sedangkan Specific tidak berubah. Capability tetap fail-closed; route boleh tersimpan walaupun capability belum tersedia.

Jika specialist berbeda provider/model, Xiao mengirim media + pertanyaan saat ini saja, menerima observation/transcript yang dibatasi, lalu meminta Main membuat jawaban final. Full session history tidak dikirim ke specialist secara default dan intermediate specialist tidak menjadi canonical assistant turn.

## Telegram Bot API 10.3 yang Digunakan

xiao v0.3.0 memakai subset Telegram Bot API 10.3 yang relevan untuk UI/AI flow:

- `can_stop` dan `keep_on_stop` pada streaming draft.
- update `stopped_message_generation` / `MessageGenerationStopped`.
- `DisabledButton` pada inline keyboard.
- `force_reply` pada keyboard markup yang dimodelkan xiao.
- `RichMessageButton`, `RichBlockButtons`, dan style button.
- `RichBlockExpandableBlockQuotation`.
- `RichBlockDocument`.
- `is_compact` pada Rich Table.
- `EphemeralMessageParameters` pada method yang dipakai client.

## Security & Data Integrity 0.2.0

### Owner wajib

`OWNER_USER_ID` adalah hard invariant. xiao menolak start jika owner belum dikonfigurasi.

```bash
xiao gateway
# atau
xiao setup
```

Secara default owner hanya dapat menggunakan bot di private chat miliknya. Tambahkan group/chat ID secara eksplisit melalui `ALLOWED_CHAT_IDS` bila diperlukan.

### Session identity

Session tidak lagi menggunakan posisi vector sebagai identitas. `session_id` stabil disimpan di SQLite dan ID baru berasal dari persistent high-water counter. Setiap session juga memiliki revision monotonik yang durable. Generation menangkap `(user_id, session_id, revision)`; `/clear` menaikkan revision sehingga completion lama ditolak secara transaksional, delete membuat origin tidak valid, dan switch active session tidak pernah mengalihkan late output ke session baru.

### Credential/logging

URL Telegram yang mengandung bot token tidak dicetak pada error log. `BOT_TOKEN` dan API key provider tidak disimpan sebagai nilai plaintext di row konfigurasi normal. SQLite menyimpan `secret://...` reference, sedangkan secret material disimpan terpisah di `~/.local/share/xiaoai/secrets/` dengan parent directory/private-file permissions yang diperketat pada Unix. Migrasi legacy menulis dan memverifikasi secret baru sebelum menghapus row/file plaintext lama. Ini adalah isolasi file lokal, **bukan klaim enkripsi at-rest atau OS keyring**.

Hindari menjalankan binary dengan logging HTTP library yang sangat verbose bila log akan dibagikan ke pihak lain.

### External image fallback

Fallback ke layanan image eksternal nonaktif secara default:

```dotenv
IMAGE_FALLBACK_PROVIDER=none
```

Untuk opt-in Pollinations:

```dotenv
IMAGE_FALLBACK_PROVIDER=pollinations
```

Mengaktifkannya berarti prompt image dapat dikirim ke provider eksternal tersebut bila provider aktif gagal/tidak mendukung image generation.

## Multimedia & Document Boundaries

- Image dikirim sebagai data URL ke input vision bila endpoint mendukungnya.
- Audio/voice dapat ditranskripsi melalui `/audio/transcriptions` bila provider mendukung STT.
- Video dapat diteruskan sebagai data URL `video/*` melalui jalur kompatibilitas saat endpoint memang menerima format tersebut.
- Dokumen text-like (`text/*`, Markdown, JSON, CSV, XML, source code) dibaca sebagai UTF-8 dengan batas ukuran.
- PDF text-native diekstrak lokal; DOCX dan XLSX diekstrak dari container XML dengan batas entry/worksheet untuk mencegah resource exhaustion.
- PDF scan/image-only dirender maksimal 6 halaman melalui `pdftoppm` dan dianalisis oleh vision model. Pada Linux, instal `poppler-utils` untuk jalur ini.
- Attachment image/audio/video dan halaman PDF scan dipersist per-session dengan permission ketat, lalu dapat direhidrasi pada turn berikutnya sesuai capability model dan budget context.
- Capability memakai state `Supported / Unsupported / Unknown` dengan freshness per capability. `/models` hanya merupakan catalog/metadata evidence dan **tidak** otomatis membuktikan text-chat. Probe aman mencakup text, Vision merah/biru, Video MP4 kecil, tools, structured output, dan native audio/STT; active image-generation probe hanya dijalankan secara eksplisit karena dapat memakai kredit dan hasilnya harus lolos validator gambar runtime. Semua media baru maupun rehidrasi tetap fail-closed bila capability Unknown/stale.

## Struktur Direktori

```text
xiao/
├── Cargo.toml
├── src/
│   ├── main.rs           # access policy, durable inbox router, control/generation lanes, Telegram UI
│   ├── cli.rs            # terminal setup/provider/model/Telegram commands
│   ├── document.rs       # text/PDF/DOCX/XLSX + scanned-PDF render pipeline
│   ├── attachments.rs    # bounded per-session multimodal persistence
│   ├── util.rs           # Unicode/HTML safety helpers
│   ├── bot/
│   │   ├── mod.rs
│   │   ├── client.rs     # Telegram HTTP client + Bot API 10.3 surface used by xiao
│   │   └── models.rs     # Telegram/Rich Message serde models
│   ├── ai/
│   │   ├── capability.rs # capability heuristics/metadata projection
│   │   ├── provider.rs   # single-owner provider registry + active probes
│   │   ├── storage.rs    # SQLite persistence + spawn_blocking boundary
│   │   ├── stream.rs     # tested SSE state machine
│   │   ├── http.rs       # retry/backoff policy
│   │   └── service.rs    # chat/session orchestration, STT, image generation
│   ├── parser.rs         # Markdown -> Rich Message AST
│   └── timeline.rs       # Draft streaming state/timeline
├── .github/workflows/build.yml
├── .env.example
└── CHANGELOG.md
```

## Perintah CLI

```bash
xiao start          # Jalankan bot daemon
xiao setup          # Wizard konfigurasi awal (AI Provider ➔ Gateway)
xiao status         # Dashboard status sistem lengkap & ringkas

xiao gateway        # Kelola gateway chat (Telegram token & owner) [Full Interaktif]
xiao provider       # Kelola AI provider (List, Tambah, Hapus, Switch) [Full Interaktif]

xiao model [query]  # Pilih atau cari Main Model aktif [Full Interaktif]
xiao pick           # Pilih daftar model untuk menu Telegram (maks 10) [Full Interaktif]
xiao addon          # Kelola routing spesialis multimodal (Vision/Audio/..) [Full Interaktif]

xiao probe          # Pusat diagnostik kapabilitas & live functional test [Full Interaktif]
xiao help           # Tampilkan panduan perintah resmi
```

`xiao addon` menyediakan antarmuka TUI interaktif untuk mengarahkan specialist multimodal (*Vision, Video, Audio STT, Image Gen*) ke Main Model, menonaktifkannya (*Disabled*), atau memilih model spesifik.

`xiao probe` menyediakan pusat pengujian diagnostik interaktif untuk mengaudit kapabilitas seluruh model aktif, melakukan live test sampel gambar/suara, serta mengecek cache kapabilitas SQLite.

`xiao pick` mengelola whitelist model Telegram di SQLite. File JSON legacy hanya digunakan sebagai sumber migrasi kompatibilitas bila masih ditemukan. Environment/.env dapat menjadi bootstrap input, tetapi SQLite adalah runtime source of truth.

## Konfigurasi Environment

Salin `.env.example` menjadi `.env`:

```dotenv
BOT_TOKEN=123456:telegram-bot-token
OWNER_USER_ID=123456789
ALLOWED_CHAT_IDS=
AI_ENDPOINT=https://provider.example/v1
AI_API_KEY=provider-api-key
AI_MODEL=model-name
IMAGE_FALLBACK_PROVIDER=none
AI_PROVIDER_CONNECT_TIMEOUT_SECS=10
IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS=10
IMAGE_GENERATION_TIMEOUT_SECS=120
IMAGE_DOWNLOAD_TIMEOUT_SECS=30
```

`AI_ENDPOINT` dan `AI_API_KEY` dipakai untuk subset protokol OpenAI-compatible yang didukung xiao (`/chat/completions`, `/audio/transcriptions`, dan `/images/generations`); ini bukan klaim dukungan untuk semua protokol/provider OpenAI-compatible.

`AI_PROVIDER_CONNECT_TIMEOUT_SECS` mengatur connect timeout umum traffic AI. `IMAGE_GENERATION_TIMEOUT_SECS` dan `IMAGE_DOWNLOAD_TIMEOUT_SECS` hanya mengatur operasi image; `IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS` dipertahankan sebagai knob khusus koneksi download image agar konfigurasi lama tetap aman.

## Build & Validation

```bash
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets --all-features
cargo build --release --locked
```

GitHub Actions menjalankan quality gates tersebut sebelum build Linux ARM64 dan Android ARM64. Action pihak ketiga dipin ke commit SHA dan checkout tidak menyimpan credential repository.

Untuk instalasi lokal:

```bash
cargo build --release --locked
cp target/release/xiao "$PREFIX/bin/xiao"
chmod +x "$PREFIX/bin/xiao"
```

## Perintah Bot Telegram

| Perintah / aksi | Deskripsi |
| --- | --- |
| Kirim pesan | Chat dengan model aktif |
| Kirim image/audio/video/text document | Multimodal sesuai dukungan provider dan batas xiao |
| `/start` | Menu sambutan dan status model |
| `/menu` | Menu navigasi |
| `/model` | Pilih model dari whitelist |
| `/image` | Generate image |
| `/context` | Estimasi context dan capability |
| `/session` | Session manager |
| `/new` | Session baru |
| `/clear` | Hapus history session aktif setelah konfirmasi |
| `/cancel` | Batalkan wizard/aksi interaktif; generation dibatalkan lewat native Stop Telegram |
| `/help` | Bantuan |

## Reliability Notes

- Runtime SQLite operations invoked by the bot are routed through `spawn_blocking`; synchronous helpers remain for startup/CLI compatibility only. Public mutations report success only after the durable write/transaction commits; irreversible attachment cleanup happens afterwards.
- Telegram intake is durable **at-least-once** processing, not exactly-once. A claimed update keeps its payload until the completed checkpoint; startup returns abandoned `processing` rows to `pending`. Completed tombstones deduplicate Telegram redelivery. A crash after an external side effect but before the completion checkpoint can still repeat that effect, so the documentation intentionally does not claim exactly-once side effects.
- Provider SSE handling has independent absolute ceilings for visible answer, hidden reasoning, and total streamed wire bytes. Exceeding a ceiling stops consumption and prevents a bounded/truncated turn from becoming ordinary canonical history.
- Streaming draft rendering separates stable completed Markdown from a provisional tail. Stable content uses the normal Rich Message AST parser; the provisional tail is sanitized so incomplete `**`, backticks, headings, dividers, and links do not flash raw syntax. Completion sends one permanent canonical answer; there is no second full-draft repaint.
- Permanent chat/text output follows `Rich AST → sendRichMessage → safe HTML chunking → semantic plain text rendered from the AST`; raw model Markdown is never the ultimate fallback. Rich Message structural budgets are validated locally before network I/O.
- Successful `/image` delivery intentionally uses a native Telegram photo message with a safe HTML caption and inline buttons. Image status/error/dashboard surfaces may use Rich Message blocks; image success is not claimed to be a Rich Message AST body.
- Provider retry policy covers transient connect/timeout, HTTP 408/429/502/503/504, and honors `Retry-After`. Mid-stream interruption is preserved as a partial result rather than retried blindly.
- Media is still encoded as base64 when an OpenAI-compatible JSON payload requires it, but downloads/persistence are bounded and owner generations are serialized, preventing unbounded concurrent generation growth.
- Runtime media authorization uses fresh capability evidence, not legacy raw booleans. Evidence precedence is UserOverride > ActiveProbe > KnownProviderProfile > ProviderMetadata; TTLs are 7 days for ActiveProbe, 30 days for KnownProviderProfile, and 6 hours for ProviderMetadata, while UserOverride remains until explicitly replaced/removed.
- Context sizing remains an estimate because tokenizer behavior differs by provider, but history selection is context-budget-aware and reserves output/system headroom.
