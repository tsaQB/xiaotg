<div align="center">

# ⚡ xiao

**Hardened, High-Performance Telegram AI Assistant & Gateway in Rust**

[![Rust](https://img.shields.io/badge/rust-2021_edition-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Telegram Bot API](https://img.shields.io/badge/Telegram_Bot_API-10.3-blue.svg?style=flat-square&logo=telegram)](https://core.telegram.org/bots/api)
[![Architecture](https://img.shields.io/badge/architecture-single--owner%20%7C%20durable--first-emerald.svg?style=flat-square)](#-keandalan--keamanan-sistem)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Android%20Termux-purple.svg?style=flat-square&logo=linux)](#-instalasi--kompilasi)
[![Version](https://img.shields.io/badge/version-v0.3.0-green.svg?style=flat-square)](#)

*Bot asisten AI Telegram asynchronous berbasis Rust untuk model OpenAI-compatible. Dilengkapi antarmuka Rich Message Telegram 10.3, streaming draft respons langsung, pembatalan instan pengguna, integrasi Web Search & MCP Tools mandiri, routing spesialis multimodal 5-role, dan persistensi sesi SQLite transaksional.*

[Fitur Unggulan](#-fitur-unggulan) • [Arsitektur Sistem](#-arsitektur-sistem) • [Web Search & MCP](#-web-search--mcp-tools) • [Perintah CLI](#-antarmuka-cli-modern) • [Instalasi](#-instalasi--kompilasi) • [Konfigurasi](#-konfigurasi-environment)

---

</div>

> [!NOTE]
> **xiao** sengaja tidak mengklaim mengimplementasikan seluruh spesifikasi Telegram Bot API. Client hanya memodelkan method, update, dan kontrak tipe yang secara presisi dibutuhkan oleh aplikasi.

---

## ✨ Fitur Unggulan

| Fitur | Keterangan |
| :--- | :--- |
| ⚡ **Telegram 10.3 Native Streaming** | Streaming draft jawaban real-time via `sendRichMessageDraft` tanpa menunggu respons model selesai. |
| 🛑 **Native Generation Stop** | Tombol stop terintegrasi Bot API 10.3 (`can_stop`, `keep_on_stop`) yang langsung memutus stream OpenAI tanpa latensi. |
| 🔍 **Web Search & MCP Calling** | AI dapat berselancar mandiri di web dengan 6 tier fallback (Brave ➔ Tavily ➔ Exa ➔ DDG ➔ Wikipedia) serta ekstraktor link URL otomatis. |
| 🧠 **5-Role Multimodal Routing** | Pemisahan beban kerja mandiri untuk **Main Model**, **Vision**, **Video**, **Audio STT**, dan **Image Generation**. |
| 🛡️ **Hardened Single-Owner Security** | Verifikasi kepemilikan ketat (`OWNER_USER_ID`), isolasi rahasia (`secret://...`), sanitasi format media, dan probe kapabilitas opsional sebagai diagnostik non-blocking. |
| 💾 **Durable SQLite Sessions** | Alokasi ID sesi sekuensial monotonik, isolasi multi-sesi, dan proteksi dari tumpang-tindih respons (*zero cross-session bleed*). |
| 🖥️ **Interactive Terminal TUI** | Pengelolaan gateway, provider, dan routing model melalui antarmuka visual terminal interaktif (*CLI-First*). |

---

## 🏗️ Arsitektur Sistem

```text
                           ┌───────────────────────────┐
                           │   Telegram Bot API 10.3   │
                           │  (Polling & Web Updates)  │
                           └─────────────┬─────────────┘
                                         │
                                         ▼
                     ┌───────────────────────────────────────┐
                     │          Durable Inbox Queue          │
                     ├───────────────────┬───────────────────┤
                     │   Control Lane    │  Generation Lane  │
                     │  (/cancel, Stop)  │ (Prompts, Media)  │
                     └─────────┬─────────┴─────────┬─────────┘
                               │                   │
                               ▼                   ▼
           ┌────────────────────────────────────────────────────────┐
           │                     xiao AI Engine                     │
           │  ┌──────────────────────────────────────────────────┐  │
           │  │ Multi-Turn Tool Loop (Web Search & Fetch URL)    │  │
           │  ├──────────────────────────────────────────────────┤  │
           │  │ Role Routing: Main / Vision / Video / STT / Gen  │  │
           │  └──────────────────────────────────────────────────┘  │
           └───────────────┬────────────────────────┬───────────────┘
                           │                        │
            ┌──────────────▼──────────┐  ┌──────────▼───────────────┐
            │   Search & MCP Tools    │  │   SQLite Durable Store   │
            │ • Brave / Tavily / Exa  │  │ • Sessions & Revisions   │
            │ • Keyless MCP Exa       │  │ • Capability Cache       │
            │ • DuckDuckGo / Wiki     │  │ • Secrets (secret://...) │
            │ • HTML Content Cleaner  │  │ • Attachments Isolation  │
            └─────────────────────────┘  └──────────────────────────┘
```

---

## 🌐 Web Search & MCP Tools

`xiao` mengadopsi mekanisme pencarian web otonom berjenjang (*Zero-Config Multi-Tier Fallback*) serta pembaca isi tautan web:

```text
Pencarian Web:
Brave Search API ──[jika gagal/tanpa key]──► Tavily API ──[jika gagal]──► Exa API
                                                                            │
Wikipedia Knowledge API ◄──[jika terblokir]── DuckDuckGo Scraper ◄──────────┘
```

1. **`web_search`**: AI mencari ringkasan informasi terkini dari internet saat membutuhkan data faktual (kurs, berita, dokumentasi terbaru).
2. **`fetch_url`**: AI mengekstrak dan membaca teks bersih dari URL web, membersihkan tag `<script>`, `<style>`, iklan, dan membatasi ukuran teks secara aman.
3. **HTTP 400 Fallback**: Bila provider atau model lokal yang digunakan tidak mendukung parameter `tools`, `xiao` secara cerdas mengulang permintaan tanpa parameter tersebut secara transparan.

---

## 🧠 Routing Model Multimodal v0.3.0

`xiao` membagi kapabilitas AI ke dalam 5 peran fungsional:

- **Main Model** — Menangani percakapan utama, penalaran teks, sintesis akhir, dan memegang *canonical history*.
- **Vision Model** — Membaca gambar, foto dokumen, dan hasil render scan PDF.
- **Video Model** — Analisis konteks dan pemahaman file video.
- **Audio STT Model** — Transkripsi pesan suara / audio voice note menjadi teks.
- **Image Generation Model** — Pembuatan gambar berbasis prompt teks (`/image`).

Empat addon spesialis dapat dikonfigurasi ke salah satu dari 3 status:
- **`Main Model`** *(Default)*: Mengikuti model utama secara dinamis.
- **`Specific Model`**: Diarahkan ke model spesifik tertentu (misal: `whisper-large-v3` untuk STT).
- **`Disabled`**: Menonaktifkan fungsionalitas terkait.

---

## 🖥️ Antarmuka CLI Modern

Semua administrasi bot dilakukan melalui perintah tunggal baku `xiao`:

```bash
# Operasional Bot & Chat Terminal
xiao start          # Jalankan bot daemon Telegram di foreground
xiao chat [prompt]  # Buka chat interaktif langsung di terminal atau one-shot prompt
xiao setup          # Wizard interaktif 2 tahap (Provider AI ➔ Gateway Telegram)
xiao status         # Dashboard status sistem lengkap & ringkas

# Konfigurasi Interaktif (TUI)
xiao gateway        # Kelola gateway Telegram (Token & Owner ID)
xiao provider       # Kelola daftar provider AI (List, Tambah, Hapus, Switch)
xiao model [query]  # Pilih atau cari Main Model via pencarian langsung
xiao addon          # Atur delegasi spesialis multimodal (Vision/Audio/Video/Gen)
xiao probe          # Pusat diagnostik kapabilitas, audit cache & live testing

# Bantuan
xiao help           # Panduan lengkap perintah CLI
```

---

## 📱 Perintah Bot Telegram

Antarmuka Telegram didesain ultra-clean sebagai ruang obrolan cerdas dan bebas distraksi tombol teknis. Konfigurasi model, provider, dan routing dikelola terpusat melalui CLI.

| Perintah | Tipe Respons | Deskripsi |
| :--- | :--- | :--- |
| *(Teks biasa)* | Streaming Draft | Percakapan reguler dengan Main Model aktif |
| *(Foto / Media)* | Multimodal | Analisis gambar, audio voice, video, atau dokumen |
| `/start` | Rich Message | Pesan pembuka dan status ringkas bot |
| `/clear` | Dialog Konfirmasi | Hapus seluruh riwayat percakapan sesi aktif |
| `/new` | Transaksional | Buat sesi percakapan baru yang bersih |
| `/image <prompt>` | Native Photo | Buat gambar baru dari deskripsi teks |
| `/help` | Rich Message | Panduan ringkas penggunaan bot Telegram |

---

## 🔒 Keandalan & Keamanan Sistem

### 1. Invariant Pemilik Tunggal (`OWNER_USER_ID`)
`OWNER_USER_ID` adalah *hard invariant*. `xiao` akan menolak berjalan jika ID pemilik belum dikonfigurasi. Penggunaan bot terbatas di private chat milik owner; grup tambahan hanya dapat diizinkan melalui whitelist `ALLOWED_CHAT_IDS`.

### 2. Isolasi Kredensial Lokal (`secret://...`)
Token bot dan kunci API provider tidak disimpan dalam teks polos di database konfigurasi. Database hanya menyimpan referensi URI bertipe `secret://...`, sementara nilai rahasia disimpan di direktori terisolasi `~/.local/share/xiaoai/secrets/` dengan izin akses Unix yang diperketat (`0700` direktori, `0600` berkas).

### 3. Integritas Sesi Transaksional
Sesi tidak diidentifikasi menggunakan indeks vektor melainkan `session_id` persisten dengan *monotonic revision*. Operasi `/clear` menaikkan nomor revisi sehingga jika model lama terlambat mengembalikan jawaban (*late stream*), jawaban tersebut secara otomatis ditolak dan tidak akan mengotori sesi baru.

### 4. Ekstraksi Dokumen Terproteksi
Dokumen teks dibaca dengan batasan ukuran memori yang ketat. File PDF native diekstrak secara lokal, DOCX dan XLSX dibaca melalui container XML dengan batas kuota worksheet, dan PDF hasil scan/gambar dirender maksimal 6 halaman melalui `pdftoppm` (*poppler-utils*) untuk dianalisis oleh Vision Model.

---

## ⚙️ Konfigurasi Environment

Salin template konfigurasi `.env.example` menjadi `.env`:

```bash
cp .env.example .env
```

| Variabel | Keterangan | Nilai Standar |
| :--- | :--- | :--- |
| `BOT_TOKEN` | Token otentikasi dari `@BotFather` | *(Wajib)* |
| `OWNER_USER_ID` | Telegram User ID pemilik bot | *(Wajib)* |
| `ALLOWED_CHAT_IDS` | Daftar ID chat/grup yang diizinkan (dipisah koma) | *(Kosong = Private Only)* |
| `AI_ENDPOINT` | URL dasar endpoint OpenAI-compatible | `https://api.openai.com/v1` |
| `AI_API_KEY` | Kunci API provider AI aktif | *(Opsional jika keyless)* |
| `AI_MODEL` | ID model utama yang digunakan | `gpt-4o-mini` |
| `IMAGE_FALLBACK_PROVIDER` | Layanan fallback untuk generate image | `none` *(opsi: `pollinations`)* |
| `BRAVE_API_KEY` | Kunci API Brave Search | *(Opsional)* |
| `TAVILY_API_KEY` | Kunci API Tavily Search | *(Opsional)* |
| `EXA_API_KEY` | Kunci API Exa AI | *(Opsional)* |
| `AI_PROVIDER_CONNECT_TIMEOUT_SECS` | Timeout koneksi awal request AI | `10` detik |
| `IMAGE_GENERATION_TIMEOUT_SECS` | Batas waktu operasi generate image | `120` detik |

---

## 🚀 Instalasi & Kompilasi

### Prasyarat:
- Rust toolchain (edisi 2021, versi 1.80 atau lebih baru)
- C compiler (`clang` / `gcc`) dan `libsqlite3` (atau bundled)
- *(Opsional)* `poppler-utils` untuk pemrosesan scan PDF

### 1. Kompilasi Release:
```bash
# Clone repositori
git clone https://github.com/tsaQB/xiaotg.git ~/xiao
cd ~/xiao

# Build biner teroptimasi
cargo build --release --locked
```

### 2. Pasang ke Sistem (Linux / Termux):
```bash
# Salin binary ke bin PATH
cp target/release/xiao "$PREFIX/bin/xiao"
chmod +x "$PREFIX/bin/xiao"

# Verifikasi instalasi
xiao status
```

---

## 🧪 Validasi & Quality Gates

Proyek ini menerapkan standar mutu pengujian yang ketat (*zero regression*):

```bash
# Pemeriksaan format kode
cargo fmt --all -- --check

# Pemeriksaan kompilasi
cargo check --release --locked

# Eksekusi seluruh rangkaian pengujian (221+ tes lulus)
cargo test --locked

# Analisis statis linter
cargo clippy --locked --all-targets --all-features -- -D warnings
```

---

<div align="center">

Dibuat dengan dedikasi tinggi untuk performa, privasi, dan keandalan.

**[xiao](https://github.com/tsaQB/xiaotg)** © 2026 Assaqib

</div>
