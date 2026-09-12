# GOAL: xiao CLI Simplification & Modernization

## 🎯 Objective
Sederhanakan seluruh arsitektur antarmuka baris perintah (CLI) xiao agar memiliki satu perintah baku (*Single Canonical Command*), hirarki rata (*flat hierarchy*), dan berbasis antarmuka visual terminal interaktif (*Full Interactive TUI First*). Menghilangkan sub-command bertingkat yang rumit, menghapus log mentah / ASCII clutter, dan menyajikan output minimalis, keren, singkat, dan mudah dibaca.

---

## 📋 Final Canonical Command Set

| Command | Sifat | Deskripsi |
|---|---|---|
| `xiao start` | Langsung | Menjalankan bot daemon Telegram |
| `xiao setup` | Wizard 2-Tahap | Wizard konfigurasi cepat (AI Provider ➔ Gateway Telegram) |
| `xiao status` | Instant Dashboard | Menampilkan kesehatan sistem, gateway, model aktif, & routing addon |
| `xiao gateway` | Full Interaktif | Kelola gateway perpesanan (Cek koneksi, ganti Token, ganti Owner ID) |
| `xiao provider` | Full Interaktif | Kelola AI provider (Daftar, ganti aktif, tambah endpoint, hapus) |
| `xiao model [query]` | Full Interaktif | Pilih & cari Main Model aktif via live-search TUI |
| `xiao pick` | Full Interaktif | Multi-select maksimal 10 model untuk menu `/model` Telegram |
| `xiao addon` | Full Interaktif | Kelola routing 4 spesialis multimodal (Vision, Video, Audio STT, Image Gen) |
| `xiao probe` | Full Interaktif | Pusat diagnostik kapabilitas model & live functional test |
| `xiao help` | Teks Bersih | Tampilkan panduan perintah resmi |

---

## 🛠️ Implementation Steps

1. **`GOAL.md` & `AGENTS.md` Setup:** Dokumentasikan sasaran dan pedoman arsitektur baru.
2. **Refactor `src/cli.rs`:**
   - Implementasikan `run_cli_gateway_menu` (Full interactive TUI untuk gateway Telegram).
   - Implementasikan `run_cli_addon_menu` (Full interactive TUI untuk multimodal routing).
   - Implementasikan `run_cli_probe_menu` (Full interactive TUI untuk diagnostik kapabilitas & live test).
   - Perbarui alur `run_cli_quickstart_wizard` (Step 1 AI Provider ➔ Step 2 Konfirmasi & Seleksi Gateway).
   - Perbarui tampilan `run_cli_status` & `print_cli_help` ke format minimalis modern (`●`, `○`, `◆`, `✔`, `✖`).
3. **Refactor `src/main.rs` CLI Dispatcher:**
   - Bersihkan routing command di `main()` agar hanya mengeksekusi canonical commands.
   - Tambahkan global strict rejection untuk perintah yang tidak dikenal.
4. **Dokumentasi & Quality Gates:**
   - Sinkronisasi `README.md` dan `CHANGELOG.md`.
   - Validasi dengan `cargo check` dan `cargo test`.
