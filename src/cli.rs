use rand::Rng;
use std::env;
use std::io::{self, BufRead, Write};

use crate::ai::AIChatService;
use crate::bot::client::TelegramBotClient;
use crate::{
    get_configured_owner_id, get_configured_token, load_environment, save_env_kv, save_token_to_env,
};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::ai::service::{
    load_provider_store, save_provider_store, CapabilityKind, CapabilityState, ModelRole,
    ModelRoute, ProbeEvent, ProbeOutcome, ProviderConfig,
};
use crate::ai::storage::ProviderStore;

struct CleanRawMode;
impl CleanRawMode {
    fn new() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        let _ = execute!(stdout, EnterAlternateScreen, cursor::Hide);
        Ok(Self)
    }
}
impl Drop for CleanRawMode {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
        let _ = terminal::disable_raw_mode();
    }
}

pub fn terminal_interactive_select(
    title: &str,
    items: &[String],
    initial_idx: usize,
    allow_search: bool,
    initial_query: Option<&str>,
) -> Option<usize> {
    if items.is_empty() {
        return None;
    }

    let _raw_guard = CleanRawMode::new().ok()?;
    let mut stdout = io::stdout();

    let mut query = initial_query.unwrap_or("").to_string();
    let mut selected_pos = initial_idx.min(items.len() - 1);
    let page_size = 20usize;
    let mut top_idx = 0usize;

    loop {
        let filtered: Vec<(usize, &String)> = if query.is_empty() {
            items.iter().enumerate().collect()
        } else {
            let q_low = query.to_lowercase();
            items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.to_lowercase().contains(&q_low))
                .collect()
        };

        if selected_pos >= filtered.len() {
            selected_pos = filtered.len().saturating_sub(1);
        }

        if selected_pos < top_idx {
            top_idx = selected_pos;
        } else if selected_pos >= top_idx + page_size {
            top_idx = selected_pos + 1 - page_size;
        }

        let mut buffer = Vec::new();
        buffer.push(format!("\x1b[1;36m{}\x1b[0m", title));
        if allow_search {
            buffer.push(format!(
                "\x1b[33mFilter:\x1b[0m \x1b[1;37m{}\x1b[38;5;244m_  \x1b[38;5;240m[▲/▼ Geser · Enter Pilih · Esc Batal]\x1b[0m",
                query
            ));
        } else {
            buffer.push("\x1b[38;5;243m[▲/▼ Geser · Enter Pilih · Esc Batal]\x1b[0m".to_string());
        }
        buffer.push(
            "\x1b[38;5;238m────────────────────────────────────────────────────────────\x1b[0m"
                .to_string(),
        );

        if filtered.is_empty() {
            buffer.push("  \x1b[31mTidak ada pilihan yang cocok dengan filter.\x1b[0m".to_string());
        } else {
            let end_idx = (top_idx + page_size).min(filtered.len());
            if top_idx > 0 {
                buffer.push("  \x1b[38;5;240m▲ (lebih banyak di atas)\x1b[0m".to_string());
            }
            for (curr_idx, (orig_idx, item_text)) in filtered[top_idx..end_idx].iter().enumerate() {
                let actual_idx = top_idx + curr_idx;
                let is_sel = actual_idx == selected_pos;
                if is_sel {
                    buffer.push(format!(
                        "\x1b[1;32m ❯ \x1b[1;37m{:>2}. {}\x1b[0m",
                        orig_idx + 1,
                        item_text
                    ));
                } else {
                    buffer.push(format!(
                        "   \x1b[38;5;244m{:>2}. \x1b[38;5;250m{}\x1b[0m",
                        orig_idx + 1,
                        item_text
                    ));
                }
            }
            if end_idx < filtered.len() {
                buffer.push(format!(
                    "  \x1b[38;5;240m▼ ({} pilihan lagi di bawah)\x1b[0m",
                    filtered.len() - end_idx
                ));
            }
        }
        buffer.push(
            "\x1b[38;5;238m────────────────────────────────────────────────────────────\x1b[0m"
                .to_string(),
        );

        let _ = execute!(
            stdout,
            cursor::MoveTo(0, 0),
            Clear(ClearType::All),
            Print(buffer.join("\r\n"))
        );
        let _ = stdout.flush();

        if let Ok(Event::Key(KeyEvent {
            code, modifiers, ..
        })) = event::read()
        {
            if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
                return None;
            }
            match code {
                KeyCode::Esc => return None,
                KeyCode::Enter => {
                    if let Some(&(orig_idx, _)) = filtered.get(selected_pos) {
                        return Some(orig_idx);
                    }
                }
                KeyCode::Up => selected_pos = selected_pos.saturating_sub(1),
                KeyCode::Down => {
                    if !filtered.is_empty() && selected_pos + 1 < filtered.len() {
                        selected_pos += 1;
                    }
                }
                KeyCode::PageUp => selected_pos = selected_pos.saturating_sub(page_size),
                KeyCode::PageDown => {
                    if !filtered.is_empty() {
                        selected_pos = (selected_pos + page_size).min(filtered.len() - 1);
                    }
                }
                KeyCode::Backspace if allow_search => {
                    query.pop();
                    selected_pos = 0;
                }
                KeyCode::Char(c) if allow_search => {
                    query.push(c);
                    selected_pos = 0;
                }
                _ => {}
            }
        }
    }
}

#[allow(dead_code)]
pub fn terminal_interactive_multi_select(
    title: &str,
    items: &[String],
    selected: &[bool],
    max_selected: usize,
) -> Option<Vec<usize>> {
    if items.is_empty() {
        return None;
    }
    let _raw_guard = CleanRawMode::new().ok()?;
    let mut stdout = io::stdout();
    let mut cursor_idx = 0usize;
    let mut top_idx = 0usize;
    let page_size = 20usize;
    let mut query = String::new();
    let mut picked = selected.to_vec();
    picked.resize(items.len(), false);

    loop {
        let filtered: Vec<usize> = if query.is_empty() {
            (0..items.len()).collect()
        } else {
            let q = query.to_lowercase();
            items
                .iter()
                .enumerate()
                .filter_map(|(idx, item)| item.to_lowercase().contains(&q).then_some(idx))
                .collect()
        };
        if filtered.is_empty() {
            cursor_idx = 0;
            top_idx = 0;
        } else {
            cursor_idx = cursor_idx.min(filtered.len() - 1);
            if cursor_idx < top_idx {
                top_idx = cursor_idx;
            }
            if cursor_idx >= top_idx + page_size {
                top_idx = cursor_idx + 1 - page_size;
            }
        }
        let end_idx = (top_idx + page_size).min(filtered.len());
        let mut buffer = vec![format!("\x1b[1;36m{}\x1b[0m", title)];
        buffer.push(format!(
            "\x1b[33mFilter:\x1b[0m \x1b[1;37m{}_\x1b[0m  \x1b[38;5;240m[▲/▼ Geser · Spasi Centang · Enter Simpan · Esc Batal]\x1b[0m",
            query
        ));
        buffer.push(format!(
            "\x1b[38;5;244mTerpilih: {}/{} · Menampilkan {}-{} dari {}\x1b[0m",
            picked.iter().filter(|v| **v).count(),
            max_selected,
            if filtered.is_empty() { 0 } else { top_idx + 1 },
            end_idx,
            filtered.len()
        ));
        buffer.push(
            "\x1b[38;5;238m────────────────────────────────────────────────────────────\x1b[0m"
                .to_string(),
        );
        for (display_idx, idx) in filtered[top_idx..end_idx].iter().enumerate() {
            let marker = if picked[*idx] { "[x]" } else { "[ ]" };
            let is_cursor = top_idx + display_idx == cursor_idx;
            if is_cursor {
                buffer.push(format!(
                    "\x1b[1;32m ❯ \x1b[1;37m{} {:>3}. {}\x1b[0m",
                    marker,
                    idx + 1,
                    items[*idx]
                ));
            } else {
                buffer.push(format!(
                    "   \x1b[38;5;244m{} {:>3}. \x1b[38;5;250m{}\x1b[0m",
                    marker,
                    idx + 1,
                    items[*idx]
                ));
            }
        }
        buffer.push(
            "\x1b[38;5;238m────────────────────────────────────────────────────────────\x1b[0m"
                .to_string(),
        );

        let _ = execute!(
            stdout,
            cursor::MoveTo(0, 0),
            Clear(ClearType::All),
            Print(buffer.join("\r\n"))
        );
        let _ = stdout.flush();

        if let Ok(Event::Key(KeyEvent {
            code, modifiers, ..
        })) = event::read()
        {
            if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
                return None;
            }
            match code {
                KeyCode::Esc => return None,
                KeyCode::Up => cursor_idx = cursor_idx.saturating_sub(1),
                KeyCode::Down => {
                    cursor_idx = (cursor_idx + 1).min(filtered.len().saturating_sub(1))
                }
                KeyCode::PageUp => cursor_idx = cursor_idx.saturating_sub(page_size),
                KeyCode::PageDown => {
                    cursor_idx = (cursor_idx + page_size).min(filtered.len().saturating_sub(1))
                }
                KeyCode::Char(' ') => {
                    if let Some(&idx) = filtered.get(cursor_idx) {
                        if picked[idx] || picked.iter().filter(|v| **v).count() < max_selected {
                            picked[idx] = !picked[idx];
                        }
                    }
                }
                KeyCode::Enter => {
                    return Some(
                        picked
                            .iter()
                            .enumerate()
                            .filter_map(|(idx, chosen)| chosen.then_some(idx))
                            .collect(),
                    )
                }
                KeyCode::Backspace => {
                    query.pop();
                    cursor_idx = 0;
                    top_idx = 0;
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    cursor_idx = 0;
                    top_idx = 0;
                }
                _ => {}
            }
        }
    }
}

pub(crate) async fn run_cli_quickstart_wizard(ai_service: &AIChatService) -> Option<String> {
    println!("\n\x1b[1;36mxiao Setup Wizard\x1b[0m");
    println!(
        "\x1b[38;5;244mKonfigurasi awal AI Provider dan Gateway. Tekan Ctrl+C untuk batal.\x1b[0m"
    );
    println!("\x1b[38;5;238m────────────────────────────────────────────────────────────\x1b[0m");

    let stdin = io::stdin();
    let mut reader = stdin.lock();

    // Step 1: AI Provider & Main Model
    println!("\n\x1b[1;37m[1/2] AI Provider & Main Model\x1b[0m");
    let endpoint = loop {
        print!("  \x1b[1;37mEndpoint URL\x1b[0m \x1b[38;5;244m(e.g. https://openrouter.ai/api/v1):\x1b[0m ");
        let _ = io::stdout().flush();
        let mut input = String::new();
        if reader.read_line(&mut input).is_err() {
            println!("\n\x1b[38;5;244mSetup dibatalkan.\x1b[0m");
            return None;
        }
        let clean = input.trim().trim_end_matches('/').to_string();
        if clean.starts_with("http://") || clean.starts_with("https://") {
            break clean;
        }
        println!("  \x1b[31m✖ Error: Endpoint harus diawali dengan http:// atau https://\x1b[0m");
    };

    print!("  \x1b[1;37mAPI Key\x1b[0m \x1b[38;5;244m(Enter jika lokal / tanpa key):\x1b[0m ");
    let _ = io::stdout().flush();
    let mut key_input = String::new();
    let _ = reader.read_line(&mut key_input);
    let mut api_key = key_input.trim().to_string();
    if api_key.is_empty() {
        api_key = "none".to_string();
    }

    print!("  \x1b[1;37mProvider Name\x1b[0m \x1b[38;5;244m(Enter untuk default):\x1b[0m ");
    let _ = io::stdout().flush();
    let mut alias_input = String::new();
    let _ = reader.read_line(&mut alias_input);
    let raw_alias = alias_input.trim();
    let clean_alias = if raw_alias.is_empty() {
        if let Ok(u) = url::Url::parse(&endpoint) {
            u.host_str().unwrap_or("Custom Provider").to_string()
        } else {
            "Custom Provider".to_string()
        }
    } else {
        raw_alias.to_string()
    };

    println!("  \x1b[38;5;244mMenghubungkan ke endpoint...\x1b[0m");
    let (ok, res) = ai_service
        .fetch_models_from_endpoint(&endpoint, &api_key)
        .await;
    if !ok {
        let err = res.err().unwrap_or_else(|| "Unknown error".to_string());
        println!("  \x1b[31m✖ Error: Gagal terhubung ke provider ({err})\x1b[0m");
        return None;
    }

    let models = res.unwrap_or_else(|_| vec!["gpt-4o".to_string()]);
    println!(
        "  \x1b[1;32m✔ Terhubung! Ditemukan {} model.\x1b[0m",
        models.len()
    );

    let selected_idx = terminal_interactive_select(
        "Pilih Main Model untuk Provider Ini:",
        &models,
        0,
        true,
        None,
    );

    let active_model = if let Some(idx) = selected_idx {
        models[idx].clone()
    } else {
        models
            .first()
            .cloned()
            .unwrap_or_else(|| "gpt-4o".to_string())
    };

    let random_suffix: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(6)
        .map(char::from)
        .collect();
    let provider_id = format!("prov_{}", random_suffix.to_lowercase());

    let provider = ProviderConfig {
        id: provider_id.clone(),
        name: clean_alias.clone(),
        endpoint: endpoint.clone(),
        api_key: api_key.clone(),
        api_key_ref: None,
        models: models.clone(),
        active_model: active_model.clone(),
    };

    let mut store = load_provider_store();
    store.providers.push(provider.clone());
    store.active_id = Some(provider_id);
    if let Err(error) = save_provider_store(&store) {
        println!("  \x1b[31m✖ Error: Konfigurasi provider gagal disimpan: {error}\x1b[0m");
        return None;
    }
    if !ai_service.reload_provider_store().await {
        println!("  \x1b[31m✖ Error: Gagal memuat ulang provider di memori runtime.\x1b[0m");
        return None;
    }

    // Preserving existing routes: only initialize missing routes to Main Model (additive setup)
    let existing_routing = ai_service.model_routing_config().await;
    for role in ModelRole::addon_roles() {
        if existing_routing.route(role).is_none() {
            if let Err(err) = ai_service
                .set_model_route(role, ModelRoute::MainModel)
                .await
            {
                println!(
                    "  \x1b[31m✖ Error: Gagal menginisialisasi route {}: {err}\x1b[0m",
                    role.display_name()
                );
                return None;
            }
        }
    }

    println!(
        "  \x1b[1;32m✔ Main Model diset ke     : {}\x1b[0m",
        active_model
    );

    // Step 2: Gateway Setup
    println!("\n\x1b[1;37m[2/2] Gateway Setup\x1b[0m");
    print!("  \x1b[1;37mKonfigurasi Gateway sekarang? [Y/n]:\x1b[0m ");
    let _ = io::stdout().flush();
    let mut gateway_ans = String::new();
    let _ = reader.read_line(&mut gateway_ans);

    let mut bot_username_opt = None;
    let mut final_token_opt = None;
    let owner_id_opt;

    if !gateway_ans.trim().eq_ignore_ascii_case("n") {
        println!("\n  \x1b[1;36mPilih Gateway:\x1b[0m");
        println!("    \x1b[1;32m❯ • Telegram\x1b[0m\n");

        let (final_token, bot_username) = loop {
            print!("  \x1b[1;37mTelegram Bot Token:\x1b[0m ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            if reader.read_line(&mut input).is_err() {
                println!("\n\x1b[38;5;244mSetup dibatalkan.\x1b[0m");
                return None;
            }
            let user_token = input.trim().to_string();
            if user_token.is_empty() {
                println!("  \x1b[31m✖ Error: Token tidak boleh kosong.\x1b[0m");
                continue;
            }

            let temp_bot = TelegramBotClient::new(&user_token);
            match temp_bot.get_me().await {
                Ok(resp) if resp.ok => {
                    let Some(bot_info) = resp.result else {
                        println!(
                            "  \x1b[31m✖ Error: Telegram tidak mengembalikan info bot.\x1b[0m"
                        );
                        continue;
                    };
                    let uname = bot_info.username.unwrap_or_else(|| "Unknown".to_string());
                    if let Err(e) = save_token_to_env(&user_token) {
                        println!("  \x1b[31m✖ Error: Gagal menyimpan token ke storage: {e}\x1b[0m");
                        return None;
                    }
                    println!(
                        "  \x1b[1;32m✔ Token valid! Terhubung ke @{} ({})\x1b[0m",
                        uname, bot_info.first_name
                    );
                    break (user_token, uname);
                }
                Ok(resp) => {
                    let desc = resp
                        .description
                        .unwrap_or_else(|| "Invalid token".to_string());
                    println!("  \x1b[31m✖ Error: Token tidak valid ({desc})\x1b[0m");
                }
                Err(e) => {
                    println!("  \x1b[31m✖ Error: Gagal terhubung ke Telegram API ({e})\x1b[0m");
                }
            }
        };

        let owner_user_id = loop {
            print!("  \x1b[1;37mOwner User ID:\x1b[0m ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            if reader.read_line(&mut input).is_err() {
                println!("\n\x1b[38;5;244mSetup dibatalkan.\x1b[0m");
                return None;
            }
            match input.trim().parse::<i64>() {
                Ok(value) if value > 0 => break value,
                _ => {
                    println!("  \x1b[31m✖ Error: Owner User ID harus berupa angka positif.\x1b[0m")
                }
            }
        };
        if let Err(e) = save_env_kv("OWNER_USER_ID", &owner_user_id.to_string()) {
            println!("  \x1b[31m✖ Error: Gagal menyimpan Owner ID: {e}\x1b[0m");
            return None;
        }
        println!("  \x1b[1;32m✔ Owner ID diset ke: {}\x1b[0m", owner_user_id);

        bot_username_opt = Some(bot_username);
        final_token_opt = Some(final_token);
        owner_id_opt = Some(owner_user_id);
    } else {
        println!("  \x1b[38;5;244m○ Konfigurasi gateway dilewati.\x1b[0m");
        // Reuse existing gateway token if present
        if let Some(token) = get_configured_token() {
            final_token_opt = Some(token);
        }
        owner_id_opt = get_configured_owner_id();
    }

    println!("\n\x1b[38;5;238m────────────────────────────────────────────────────────────\x1b[0m");
    println!("\x1b[1;32mSetup Selesai!\x1b[0m\n");
    println!("  Provider   ● {} ({})", clean_alias, endpoint);
    println!("  Model      ◆ {}", active_model);
    if let (Some(uname), Some(oid)) = (bot_username_opt, owner_id_opt) {
        println!("  Gateway    ● Telegram (@{} · Owner: {})", uname, oid);
    } else if let Some(oid) = owner_id_opt {
        if final_token_opt.is_some() {
            println!("  Gateway    ● Telegram (existing · Owner: {})", oid);
        } else {
            println!("  Gateway    ○ Belum dikonfigurasi (Gunakan 'xiao gateway')");
        }
    } else {
        println!("  Gateway    ○ Belum dikonfigurasi (Gunakan 'xiao gateway')");
    }
    println!("  Addons     ○ Mengikuti konfigurasi addon (atur via 'xiao addon')");
    println!("\n\x1b[1;36mJalankan bot sekarang dengan perintah:\x1b[0m");
    println!("  \x1b[1;37mxiao start\x1b[0m\n");

    final_token_opt
}

pub(crate) async fn get_or_prompt_token(ai_service: &AIChatService) -> Option<String> {
    if let Some(token) = get_configured_token() {
        if ai_service.has_configured_provider(0).await {
            return Some(token);
        }
    }
    println!("\x1b[33mKonfigurasi belum lengkap. Membuka Setup Wizard...\x1b[0m");
    run_cli_quickstart_wizard(ai_service).await
}

pub(crate) async fn run_cli_status(ai_service: &AIChatService) {
    load_environment();
    println!("\n\x1b[1;36mxiao Status\x1b[0m\n");

    let token = env::var("BOT_TOKEN")
        .ok()
        .or_else(|| crate::ai::service::load_app_setting("BOT_TOKEN"))
        .unwrap_or_default();
    let owner_id = get_configured_owner_id();

    // 1. Gateway Status
    if token.is_empty() || token == "YOUR_TELEGRAM_BOT_TOKEN_HERE" {
        println!("  Gateway      ○ Telegram: Belum dikonfigurasi (Jalankan 'xiao gateway')");
    } else {
        let bot = TelegramBotClient::new(&token);
        match bot.get_me().await {
            Ok(resp) if resp.ok => {
                if let Some(info) = resp.result {
                    let uname = info.username.unwrap_or_else(|| "Unknown".to_string());
                    let owner_str = owner_id
                        .map(|id| format!(" · Owner: {id}"))
                        .unwrap_or_default();
                    println!("  Gateway      ● Telegram (@{uname}{owner_str})");
                } else {
                    println!("  Gateway      ✖ Telegram: Tidak ada info bot");
                }
            }
            Ok(_) | Err(_) => {
                println!("  Gateway      ✖ Telegram: Token tidak valid / Error koneksi");
            }
        }
    }

    // 2. Provider & Model Status (evidence-based)
    let store = load_provider_store();
    let active_p = if let Some(ref aid) = store.active_id {
        store.providers.iter().find(|p| &p.id == aid).cloned()
    } else {
        store.providers.first().cloned()
    };

    if let Some(p) = active_p {
        let (ok, res) = ai_service
            .fetch_models_from_endpoint(&p.endpoint, &p.api_key)
            .await;
        let provider_health = if ok {
            format!(
                "\x1b[32mHealthy\x1b[0m ({} models available)",
                res.map(|m| m.len()).unwrap_or(p.models.len())
            )
        } else {
            let err = res.err().unwrap_or_else(|| "unreachable".to_string());
            format!("\x1b[31mUnhealthy\x1b[0m ({err})")
        };

        println!("  Provider     ● {} — {}", p.name, provider_health);
        println!(
            "  Main Model   ◆ {} ({} configured models)",
            p.active_model,
            p.models.len()
        );

        let cap_record = ai_service
            .capability_record(&p.endpoint, &p.active_model)
            .await;
        println!("\n  \x1b[1;37mModel Capabilities (Evidence-Based):\x1b[0m");

        let text_chat_state = cap_record
            .as_ref()
            .map(|r| r.effective_state_for(CapabilityKind::TextChat))
            .unwrap_or(CapabilityState::Unknown);
        let vision_state = cap_record
            .as_ref()
            .map(|r| r.effective_state_for(CapabilityKind::ImageInput))
            .unwrap_or(CapabilityState::Unknown);
        let video_state = cap_record
            .as_ref()
            .map(|r| r.effective_state_for(CapabilityKind::VideoInput))
            .unwrap_or(CapabilityState::Unknown);
        let audio_state = cap_record
            .as_ref()
            .map(|r| r.effective_state_for(CapabilityKind::AudioInput))
            .unwrap_or(CapabilityState::Unknown);
        let tools_state = cap_record
            .as_ref()
            .map(|r| r.effective_state_for(CapabilityKind::Tools))
            .unwrap_or(CapabilityState::Unknown);

        let format_cap_line = |name: &str, state: CapabilityState, kind: CapabilityKind| {
            let src = cap_record
                .as_ref()
                .and_then(|r| r.effective_evidence_for(kind))
                .map(|e| match e.source {
                    crate::ai::storage::CapabilityEvidenceSource::ProviderMetadata => {
                        "provider metadata"
                    }
                    crate::ai::storage::CapabilityEvidenceSource::ActiveProbe => "active probe",
                    crate::ai::storage::CapabilityEvidenceSource::KnownProviderProfile => {
                        "provider profile"
                    }
                    crate::ai::storage::CapabilityEvidenceSource::UserOverride => "user override",
                })
                .unwrap_or("no evidence");
            match state {
                CapabilityState::Supported => {
                    format!(
                        "    • {:<16}: \x1b[32m✔ Supported\x1b[0m   \x1b[38;5;244m({src})\x1b[0m",
                        name
                    )
                }
                CapabilityState::Unsupported => {
                    format!(
                        "    • {:<16}: \x1b[31m✖ Unsupported\x1b[0m \x1b[38;5;244m({src})\x1b[0m",
                        name
                    )
                }
                CapabilityState::Unknown => {
                    format!(
                        "    • {:<16}: \x1b[38;5;244m○ Unknown       ({src})\x1b[0m",
                        name
                    )
                }
            }
        };

        println!(
            "{}",
            format_cap_line("Text Chat", text_chat_state, CapabilityKind::TextChat)
        );
        println!(
            "{}",
            format_cap_line("Vision (Image)", vision_state, CapabilityKind::ImageInput)
        );
        println!(
            "{}",
            format_cap_line("Video Frames", video_state, CapabilityKind::VideoInput)
        );
        println!(
            "{}",
            format_cap_line("Audio Input", audio_state, CapabilityKind::AudioInput)
        );
        println!(
            "{}",
            format_cap_line("Tools / JSON", tools_state, CapabilityKind::Tools)
        );
        if let Some(ctx) = cap_record.as_ref().and_then(|r| r.context_window) {
            println!(
                "    • {:<16}: \x1b[36m{} tokens\x1b[0m",
                "Context Limit", ctx
            );
        }
    } else {
        println!("  Provider     ○ Belum ada AI Provider (Jalankan 'xiao provider')");
    }

    // 3. Addon Routing
    println!("\nAddon Routes:");
    let providers = ai_service.get_user_providers(0).await;
    let routing = ai_service.model_routing_config().await;
    for role in ModelRole::addon_roles() {
        let route = routing
            .route(role)
            .cloned()
            .unwrap_or(ModelRoute::MainModel);
        let route_text = addon_route_text(&route, &providers);
        let health = match ai_service.resolve_model_route(role).await {
            Ok(_) => "\x1b[32mavailable\x1b[0m",
            Err(_) => "\x1b[38;5;244munavailable\x1b[0m",
        };
        println!("  {:<12} → {} ({health})", role.display_name(), route_text);
    }

    // 4. Web Search & Tools Status
    println!("\nWeb Search & Tools:");
    let (search_engine_str, mcp_url) = crate::ai::tools::get_search_engine_status();
    println!("  Search Engine → {}", search_engine_str);
    println!("  MCP Hosted    → {}", mcp_url);
    println!("  Fetch Engine  → \x1b[32mEnabled\x1b[0m (Auto Link Reader & Extract)");
    println!();
}

pub(crate) async fn run_cli_gateway_menu() {
    load_environment();
    loop {
        let token = get_configured_token().unwrap_or_default();
        let owner_id = get_configured_owner_id();

        let tg_status = if token.is_empty() || token == "YOUR_TELEGRAM_BOT_TOKEN_HERE" {
            "○ Belum Terhubung".to_string()
        } else {
            let bot = TelegramBotClient::new(&token);
            match bot.get_me().await {
                Ok(resp) if resp.ok => {
                    let uname = resp
                        .result
                        .and_then(|i| i.username)
                        .unwrap_or_else(|| "Bot".to_string());
                    let owner_str = owner_id
                        .map(|id| format!(" · Owner: {id}"))
                        .unwrap_or_else(|| " · Owner: -".to_string());
                    format!("● @{uname}{owner_str}")
                }
                _ => "✖ Token Tidak Valid".to_string(),
            }
        };

        let items = vec![
            format!("Telegram [{tg_status}]"),
            "✕ Selesai / Keluar".to_string(),
        ];

        let sel = terminal_interactive_select("Kelola Gateway Perpesanan:", &items, 0, false, None);

        let Some(idx) = sel else {
            break;
        };

        if idx == 0 {
            run_cli_gateway_telegram_submenu().await;
        } else {
            break;
        }
    }
}

async fn run_cli_gateway_telegram_submenu() {
    loop {
        let token = get_configured_token().unwrap_or_default();
        let owner_id = get_configured_owner_id();

        let summary = format!(
            "== Gateway: Telegram ==\r\n\
             • Token:    {}\r\n\
             • Owner ID: {}",
            if token.is_empty() {
                "Belum dikonfigurasi"
            } else {
                "Tersimpan"
            },
            owner_id
                .map(|i| i.to_string())
                .unwrap_or_else(|| "Belum diset".to_string())
        );

        let actions = vec![
            "🔍 Cek Koneksi / Ping Telegram API".to_string(),
            "🔑 Ubah Telegram Bot Token".to_string(),
            "👤 Ubah Telegram Owner User ID".to_string(),
            "← Kembali".to_string(),
        ];

        let sel = terminal_interactive_select(&summary, &actions, 0, false, None);
        let Some(choice) = sel else {
            break;
        };

        match choice {
            0 => {
                run_cli_telegram_check().await;
                print!("\x1b[38;5;244mTekan Enter untuk kembali...\x1b[0m");
                let _ = io::stdout().flush();
                let mut tmp = String::new();
                let _ = io::stdin().read_line(&mut tmp);
            }
            1 => {
                run_cli_telegram_bind(None).await;
            }
            2 => {
                run_cli_telegram_owner(None).await;
            }
            _ => break,
        }
    }
}

pub(crate) async fn run_cli_gateway_hub(action: Option<&str>, target: Option<&str>) {
    load_environment();
    match action {
        None => {
            run_cli_gateway_menu().await;
        }
        Some("check") | Some("test") | Some("status") => {
            run_cli_telegram_check().await;
        }
        Some("token") | Some("bind") => {
            run_cli_telegram_bind(target).await;
        }
        Some("owner") => {
            run_cli_telegram_owner(target).await;
        }
        Some("help") | Some("--help") | Some("-h") => {
            println!("\n\x1b[1;36mxiao gateway — Telegram Messaging Gateway Management\x1b[0m\n");
            println!("\x1b[1;37mUsage:\x1b[0m");
            println!("  xiao gateway [action] [target]\n");
            println!("\x1b[1;37mSubcommands for 'gateway':\x1b[0m");
            println!(
                "     \x1b[36mxiao gateway\x1b[0m                Open Interactive Gateway Manager"
            );
            println!("     \x1b[36mxiao gateway check\x1b[0m          Verify bot token connectivity (getMe)");
            println!(
                "     \x1b[36mxiao gateway token <TOKEN>\x1b[0m  Bind and verify Telegram Bot Token"
            );
            println!(
                "     \x1b[36mxiao gateway owner <ID>\x1b[0m     Set Telegram Owner User ID\n"
            );
        }
        Some(unknown) => {
            println!("\x1b[31m✖ Error: Sub-perintah 'gateway {unknown}' tidak dikenal.\x1b[0m");
            println!("  Jalankan 'xiao gateway help' atau 'xiao help' untuk bantuan.\n");
        }
    }
}

pub(crate) async fn run_cli_telegram_check() {
    load_environment();
    println!("\n\x1b[1;36mTelegram Gateway Status\x1b[0m");

    let token = get_configured_token().unwrap_or_default();
    if token.is_empty() || token == "YOUR_TELEGRAM_BOT_TOKEN_HERE" {
        println!("  \x1b[31m✖ BOT_TOKEN belum dikonfigurasi.\x1b[0m\n");
        return;
    }

    let bot = TelegramBotClient::new(&token);
    match bot.get_me().await {
        Ok(resp) if resp.ok => {
            if let Some(info) = resp.result {
                let uname = info.username.unwrap_or_else(|| "Unknown".to_string());
                println!("  \x1b[1;32m✔ Status:\x1b[0m   Terhubung & Terverifikasi (API 10.3)");
                println!("  \x1b[1;37mBot Name:\x1b[0m {}", info.first_name);
                println!("  \x1b[1;37mUsername:\x1b[0m @{}", uname);
                println!("  \x1b[1;37mBot ID:\x1b[0m   {}", info.id);
                if let Some(owner) = get_configured_owner_id() {
                    println!("  \x1b[1;37mOwner ID:\x1b[0m {}", owner);
                } else {
                    println!("  \x1b[31m✖ OWNER_USER_ID belum dikonfigurasi.\x1b[0m");
                }
            }
        }
        Ok(resp) => {
            println!(
                "  \x1b[31m✖ Token tidak valid ({:?})\x1b[0m",
                resp.description
            );
        }
        Err(e) => {
            println!("  \x1b[31m✖ Gagal terhubung ke Telegram API ({e})\x1b[0m");
        }
    }
    println!();
}

pub(crate) async fn run_cli_telegram_bind(manual_token: Option<&str>) {
    load_environment();
    let token = if let Some(t) = manual_token {
        t.trim().to_string()
    } else {
        print!("\n\x1b[1;37mMasukkan Telegram Bot Token:\x1b[0m ");
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return;
        }
        input.trim().to_string()
    };

    if token.is_empty() {
        println!("\x1b[31m✖ Token tidak boleh kosong.\x1b[0m\n");
        return;
    }

    println!("  \x1b[38;5;244mMemverifikasi token...\x1b[0m");
    let bot = TelegramBotClient::new(&token);
    match bot.get_me().await {
        Ok(resp) if resp.ok => {
            let Some(info) = resp.result else {
                println!("  \x1b[31m✖ Gagal membaca data bot dari Telegram.\x1b[0m\n");
                return;
            };
            let uname = info.username.unwrap_or_else(|| "Unknown".to_string());
            if let Err(e) = save_token_to_env(&token) {
                println!("  \x1b[31m✖ Gagal menyimpan token: {e}\x1b[0m\n");
            } else {
                println!(
                    "  \x1b[1;32m✔ Token valid! Terhubung ke @{} ({})\x1b[0m\n",
                    uname, info.first_name
                );
            }
        }
        Ok(resp) => {
            println!(
                "  \x1b[31m✖ Token tidak valid: {:?}\x1b[0m\n",
                resp.description
            );
        }
        Err(e) => {
            println!("  \x1b[31m✖ Error koneksi: {e}\x1b[0m\n");
        }
    }
}

pub(crate) async fn run_cli_telegram_owner(owner_arg: Option<&str>) {
    let owner = if let Some(value) = owner_arg {
        value.trim().parse::<i64>().ok()
    } else {
        print!("\n\x1b[1;37mMasukkan Telegram Owner User ID:\x1b[0m ");
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            None
        } else {
            input.trim().parse::<i64>().ok()
        }
    };

    match owner.filter(|value| *value > 0) {
        Some(owner_id) => match save_env_kv("OWNER_USER_ID", &owner_id.to_string()) {
            Ok(()) => println!("  \x1b[1;32m✔ Telegram Owner ID diset ke: {owner_id}\x1b[0m\n"),
            Err(error) => println!("  \x1b[31m✖ Gagal menyimpan Owner ID: {error}\x1b[0m\n"),
        },
        None => println!("  \x1b[31m✖ Owner User ID harus berupa angka positif.\x1b[0m\n"),
    }
}

pub(crate) async fn run_cli_provider_menu(ai_service: &AIChatService, action: Option<&str>) {
    load_environment();
    if action == Some("add") {
        run_cli_provider_add(ai_service).await;
        return;
    }
    if action == Some("rm") || action == Some("remove") {
        run_cli_provider_remove(ai_service).await;
        return;
    }

    loop {
        let store = load_provider_store();
        if store.providers.is_empty() {
            println!("\n\x1b[33mBelum ada AI Provider yang terdaftar.\x1b[0m");
            print!("Tambah provider sekarang? [Y/n]: ");
            let _ = io::stdout().flush();
            let mut ans = String::new();
            let _ = io::stdin().read_line(&mut ans);
            if ans.trim().eq_ignore_ascii_case("n") {
                return;
            }
            run_cli_provider_add(ai_service).await;
            return;
        }

        let mut menu_items: Vec<String> = store
            .providers
            .iter()
            .map(|p| {
                let is_act = store.active_id.as_deref() == Some(p.id.as_str());
                if is_act {
                    format!("{} \x1b[1;32m[AKTIF]\x1b[0m ({})", p.name, p.active_model)
                } else {
                    format!("{} ({})", p.name, p.active_model)
                }
            })
            .collect();

        menu_items.push("➕ Tambah Provider Baru".to_string());
        menu_items.push("🗑️  Hapus Provider".to_string());
        menu_items.push("✕ Selesai / Keluar".to_string());

        let sel = terminal_interactive_select("Kelola AI Provider:", &menu_items, 0, false, None);

        let Some(idx) = sel else {
            break;
        };

        if idx < store.providers.len() {
            let target_prov = &store.providers[idx];
            let is_act = store.active_id.as_deref() == Some(target_prov.id.as_str());

            let title_summary = format!(
                "== Provider: {} ==\r\n\
                 • Endpoint:     {}\r\n\
                 • Active Model: \x1b[1;36m{}\x1b[0m\r\n\
                 • Total Models: {} models\r\n\
                 • Status:       {}",
                target_prov.name,
                target_prov.endpoint,
                target_prov.active_model,
                target_prov.models.len(),
                if is_act {
                    "\x1b[1;32mAKTIF\x1b[0m"
                } else {
                    "INAKTIF"
                }
            );

            let mut sub_actions = Vec::new();
            if !is_act {
                sub_actions.push(format!(
                    "Set sebagai Active Provider ({})",
                    target_prov.name
                ));
            }
            sub_actions.push("Pilih / Ganti Model untuk Provider ini".to_string());
            sub_actions.push(format!("Hapus Provider ({})", target_prov.name));
            sub_actions.push("← Kembali".to_string());

            let sub_sel = terminal_interactive_select(&title_summary, &sub_actions, 0, false, None);
            let Some(action_idx) = sub_sel else {
                continue;
            };

            let chosen_action = if is_act { action_idx + 1 } else { action_idx };

            match chosen_action {
                0 => {
                    let mut updated_store = load_provider_store();
                    updated_store.active_id = Some(target_prov.id.clone());
                    if let Err(e) = save_provider_store(&updated_store) {
                        println!("\n\x1b[31m✖ Error: Gagal menyimpan provider aktif: {e}\x1b[0m\n");
                        continue;
                    }
                    if !ai_service.reload_provider_store().await {
                        println!(
                            "\n\x1b[31m✖ Error: Gagal memuat ulang provider di runtime.\x1b[0m\n"
                        );
                    }
                }
                1 => {
                    let (ok, res) = ai_service
                        .fetch_models_from_endpoint(&target_prov.endpoint, &target_prov.api_key)
                        .await;
                    let models = if ok {
                        res.unwrap_or_default()
                    } else {
                        target_prov.models.clone()
                    };
                    if !models.is_empty() {
                        let curr_idx = models
                            .iter()
                            .position(|m| m == &target_prov.active_model)
                            .unwrap_or(0);
                        if let Some(m_idx) = terminal_interactive_select(
                            &format!("Pilih Model untuk '{}':", target_prov.name),
                            &models,
                            curr_idx,
                            true,
                            None,
                        ) {
                            let chosen_model = models[m_idx].clone();
                            let mut updated_store = load_provider_store();
                            if let Some(p) = updated_store
                                .providers
                                .iter_mut()
                                .find(|p| p.id == target_prov.id)
                            {
                                p.active_model = chosen_model;
                                p.models = models;
                            }
                            if let Err(e) = save_provider_store(&updated_store) {
                                println!("\n\x1b[31m✖ Error: Gagal menyimpan model provider: {e}\x1b[0m\n");
                                continue;
                            }
                            if !ai_service.reload_provider_store().await {
                                println!("\n\x1b[31m✖ Error: Gagal memuat ulang provider di runtime.\x1b[0m\n");
                            }
                        }
                    }
                }
                2 => {
                    let dependencies = ai_service
                        .provider_route_dependencies(&target_prov.id)
                        .await;
                    if !dependencies.is_empty() {
                        println!(
                            "\n\x1b[31m✖ Provider '{}' masih dipakai oleh Addon spesifik.\x1b[0m",
                            target_prov.name
                        );
                        print!("\x1b[38;5;244mTekan Enter untuk kembali...\x1b[0m");
                        let _ = io::stdout().flush();
                        let mut tmp = String::new();
                        let _ = io::stdin().read_line(&mut tmp);
                        continue;
                    }
                    let mut updated_store = load_provider_store();
                    if let Some(pos) = updated_store
                        .providers
                        .iter()
                        .position(|p| p.id == target_prov.id)
                    {
                        let removed = updated_store.providers.remove(pos);
                        if updated_store.active_id.as_deref() == Some(removed.id.as_str()) {
                            updated_store.active_id =
                                updated_store.providers.first().map(|p| p.id.clone());
                        }
                        if let Err(e) = save_provider_store(&updated_store) {
                            println!("\n\x1b[31m✖ Error: Gagal menghapus provider: {e}\x1b[0m\n");
                            continue;
                        }
                        if !ai_service.reload_provider_store().await {
                            println!("\n\x1b[31m✖ Error: Gagal memuat ulang provider di runtime.\x1b[0m\n");
                        }
                    }
                }
                _ => {}
            }
        } else if idx == store.providers.len() {
            run_cli_provider_add(ai_service).await;
        } else if idx == store.providers.len() + 1 {
            run_cli_provider_remove(ai_service).await;
        } else {
            break;
        }
    }
}

pub(crate) async fn run_cli_provider_add(ai_service: &AIChatService) {
    println!("\n\x1b[1;36mTambah AI Provider Baru\x1b[0m");
    println!("\x1b[38;5;238m────────────────────────────────────────────────────────────\x1b[0m");

    let stdin = io::stdin();
    let mut reader = stdin.lock();

    print!("  \x1b[1;37mEndpoint URL:\x1b[0m ");
    let _ = io::stdout().flush();
    let mut endpoint_input = String::new();
    if reader.read_line(&mut endpoint_input).is_err() {
        return;
    }
    let endpoint = endpoint_input.trim().trim_end_matches('/').to_string();
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        println!("  \x1b[31m✖ Error: Format Endpoint URL tidak valid!\x1b[0m\n");
        return;
    }

    print!("  \x1b[1;37mAPI Key\x1b[0m \x1b[38;5;244m(Enter jika tanpa key):\x1b[0m ");
    let _ = io::stdout().flush();
    let mut key_input = String::new();
    if reader.read_line(&mut key_input).is_err() {
        return;
    }
    let mut api_key = key_input.trim().to_string();
    if api_key.is_empty() {
        api_key = "none".to_string();
    }

    print!("  \x1b[1;37mProvider Name / Alias:\x1b[0m ");
    let _ = io::stdout().flush();
    let mut alias_input = String::new();
    if reader.read_line(&mut alias_input).is_err() {
        return;
    }
    let raw_alias = alias_input.trim();
    let alias = if raw_alias.is_empty() {
        if let Ok(u) = url::Url::parse(&endpoint) {
            u.host_str().unwrap_or("Custom Provider").to_string()
        } else {
            "Custom Provider".to_string()
        }
    } else {
        raw_alias.to_string()
    };

    println!("  \x1b[38;5;244mMenghubungkan ke endpoint...\x1b[0m");
    let (ok, res) = ai_service
        .fetch_models_from_endpoint(&endpoint, &api_key)
        .await;
    if !ok {
        let err = res.err().unwrap_or_else(|| "Unknown error".to_string());
        println!("  \x1b[31m✖ Error: Gagal terhubung ke provider ({err})\x1b[0m\n");
        return;
    }

    let models = res.unwrap_or_else(|_| vec!["gpt-4o".to_string()]);
    println!(
        "  \x1b[1;32m✔ Terhubung! Ditemukan {} model.\x1b[0m",
        models.len()
    );

    let selected_idx = terminal_interactive_select(
        "Pilih Active Model untuk Provider Ini:",
        &models,
        0,
        true,
        None,
    );

    let active_model = if let Some(idx) = selected_idx {
        models[idx].clone()
    } else {
        models
            .first()
            .cloned()
            .unwrap_or_else(|| "gpt-4o".to_string())
    };

    let random_suffix: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(6)
        .map(char::from)
        .collect();
    let provider_id = format!("prov_{}", random_suffix.to_lowercase());

    let provider = ProviderConfig {
        id: provider_id.clone(),
        name: alias.clone(),
        endpoint: endpoint.clone(),
        api_key: api_key.clone(),
        api_key_ref: None,
        models: models.clone(),
        active_model: active_model.clone(),
    };

    let mut store = load_provider_store();
    store.providers.push(provider.clone());
    store.active_id = Some(provider_id);
    if let Err(error) = save_provider_store(&store) {
        println!("  \x1b[31m✖ Error: Konfigurasi provider gagal disimpan: {error}\x1b[0m\n");
        return;
    }
    if !ai_service.reload_provider_store().await {
        println!("  \x1b[31m✖ Error: Gagal memuat ulang provider di runtime.\x1b[0m\n");
        return;
    }

    println!(
        "\n  \x1b[1;32m✔ Provider '{}' berhasil ditambahkan dan diaktifkan!\x1b[0m",
        alias
    );
    println!("    Active Model: \x1b[1;36m{}\x1b[0m\n", active_model);
}

pub(crate) async fn run_cli_provider_remove(ai_service: &AIChatService) {
    let mut store = load_provider_store();
    if store.providers.is_empty() {
        println!("\n\x1b[33mBelum ada provider yang tersimpan.\x1b[0m\n");
        return;
    }

    let items: Vec<String> = store
        .providers
        .iter()
        .map(|p| {
            let is_act = store.active_id.as_deref() == Some(p.id.as_str());
            if is_act {
                format!("{} \x1b[1;32m[AKTIF]\x1b[0m", p.name)
            } else {
                p.name.clone()
            }
        })
        .collect();

    let selected =
        terminal_interactive_select("Pilih Provider yang Ingin Dihapus:", &items, 0, false, None);

    if let Some(idx) = selected {
        let target = store.providers[idx].clone();
        let dependencies = ai_service.provider_route_dependencies(&target.id).await;
        if !dependencies.is_empty() {
            println!(
                "\n\x1b[31m✖ Provider '{}' masih dipakai oleh Addon spesifik.\x1b[0m\n",
                target.name
            );
            return;
        }
        let removed = store.providers.remove(idx);
        if store.active_id.as_deref() == Some(removed.id.as_str()) {
            store.active_id = store.providers.first().map(|p| p.id.clone());
        }
        if let Err(e) = save_provider_store(&store) {
            println!("\n\x1b[31m✖ Error: Gagal menyimpan perubahan provider: {e}\x1b[0m\n");
            return;
        }
        if !ai_service.reload_provider_store().await {
            println!("\n\x1b[31m✖ Error: Gagal memuat ulang provider di runtime.\x1b[0m\n");
            return;
        }
        println!(
            "\n\x1b[1;32m✔ Provider '{}' berhasil dihapus.\x1b[0m\n",
            removed.name
        );
    }
}

pub(crate) async fn run_cli_model_picker(ai_service: &AIChatService, initial_filter: Option<&str>) {
    load_environment();
    let mut store = load_provider_store();

    if store.providers.is_empty() {
        println!(
            "\n\x1b[33mBelum ada AI Provider yang terdaftar. Jalankan 'xiao provider'.\x1b[0m\n"
        );
        return;
    }

    for prov in store.providers.iter_mut() {
        if prov.models.len() <= 1 && !prov.endpoint.is_empty() {
            if let (true, Ok(fetched)) = ai_service
                .fetch_models_from_endpoint(&prov.endpoint, &prov.api_key)
                .await
            {
                if !fetched.is_empty() {
                    prov.models = fetched;
                }
            }
        }
    }
    if let Err(e) = save_provider_store(&store) {
        println!("\n\x1b[31m✖ Error: Gagal menyimpan katalog model provider: {e}\x1b[0m\n");
        return;
    }

    let active_prov_id = store.active_id.clone().unwrap_or_default();
    let current_model = env::var("AI_MODEL").unwrap_or_default();

    let mut catalog: Vec<(String, String, String, bool)> = Vec::new();
    for prov in &store.providers {
        let is_prov_active = prov.id == active_prov_id;
        for m in &prov.models {
            let is_model_active =
                is_prov_active && (m == &prov.active_model || m == &current_model);
            catalog.push((
                prov.id.clone(),
                prov.name.clone(),
                m.clone(),
                is_model_active,
            ));
        }
    }

    if catalog.is_empty() {
        println!("\n\x1b[33mTidak ada model yang ditemukan dari provider terdaftar.\x1b[0m\n");
        return;
    }

    let is_multi = store.providers.len() > 1;
    let items: Vec<String> = catalog
        .iter()
        .map(|(_, prov_name, model_name, is_act)| {
            let act_tag = if *is_act {
                " \x1b[1;32m[AKTIF]\x1b[0m"
            } else {
                ""
            };
            if is_multi {
                format!(
                    "{} \x1b[38;5;244m({})\x1b[0m{}",
                    model_name, prov_name, act_tag
                )
            } else {
                format!("{}{}", model_name, act_tag)
            }
        })
        .collect();

    let curr_idx = catalog
        .iter()
        .position(|(_, _, _, is_act)| *is_act)
        .unwrap_or(0);
    let title = format!(
        "Pilih Main Model (Total {} model dari {} provider):",
        catalog.len(),
        store.providers.len()
    );

    let selected_idx = terminal_interactive_select(&title, &items, curr_idx, true, initial_filter);

    if let Some(idx) = selected_idx {
        if let Some((prov_id, prov_name, chosen_model, _)) = catalog.get(idx) {
            let mut updated_store = load_provider_store();
            updated_store.active_id = Some(prov_id.clone());
            if let Some(p) = updated_store
                .providers
                .iter_mut()
                .find(|p| &p.id == prov_id)
            {
                p.active_model = chosen_model.clone();
            }
            if let Err(e) = save_provider_store(&updated_store) {
                println!("\n\x1b[31m✖ Error: Gagal menyimpan active model: {e}\x1b[0m\n");
                return;
            }
            if !ai_service.reload_provider_store().await {
                println!("\n\x1b[31m✖ Error: Gagal memuat ulang provider di runtime.\x1b[0m\n");
                return;
            }
            println!(
                "\n\x1b[1;32m✔ Main Model diset ke: {}\x1b[0m ({})\n",
                chosen_model, prov_name
            );
        }
    }
}

fn addon_role_short_label(role: ModelRole) -> &'static str {
    match role {
        ModelRole::Vision => "Vision",
        ModelRole::Video => "Video",
        ModelRole::AudioStt => "Audio STT",
        ModelRole::ImageGeneration => "Image Gen",
        ModelRole::Main => "Main",
    }
}

pub(crate) async fn run_cli_addon_menu(ai_service: &AIChatService) {
    load_environment();
    loop {
        let providers = ai_service.get_user_providers(0).await;
        let routing = ai_service.model_routing_config().await;

        let mut menu_items = Vec::new();
        for role in ModelRole::addon_roles() {
            let route = routing
                .route(role)
                .cloned()
                .unwrap_or(ModelRoute::MainModel);
            let target_str = match &route {
                ModelRoute::MainModel => "○ Main Model".to_string(),
                ModelRoute::Disabled => "✖ Disabled".to_string(),
                ModelRoute::Specific { provider_id, model } => {
                    let prov_name = providers
                        .iter()
                        .find(|p| &p.id == provider_id)
                        .map(|p| p.name.as_str())
                        .unwrap_or(provider_id);
                    format!("◆ {prov_name} :: {model}")
                }
            };
            let label = addon_role_short_label(role);
            menu_items.push(format!("{:<12} [{target_str}]", label));
        }
        menu_items.push("🧪 Uji Kapabilitas Semua Model Addon Aktif".to_string());
        menu_items.push("↺ Reset Semua Addon ke Main Model".to_string());
        menu_items.push("✕ Selesai / Keluar".to_string());

        let sel = terminal_interactive_select(
            "Kelola Addon Multimodal (Pilih Role):",
            &menu_items,
            0,
            false,
            None,
        );

        let Some(idx) = sel else {
            break;
        };

        let roles = ModelRole::addon_roles();
        if idx < roles.len() {
            let role = roles[idx];
            run_cli_addon_role_submenu(ai_service, role).await;
        } else if idx == roles.len() {
            run_cli_addon_test_all_routes(ai_service).await;
        } else if idx == roles.len() + 1 {
            let mut failed = false;
            for r in ModelRole::addon_roles() {
                if let Err(e) = ai_service.set_model_route(r, ModelRoute::MainModel).await {
                    println!(
                        "\n\x1b[31m✖ Error: Gagal mereset addon {}: {e}\x1b[0m\n",
                        r.display_name()
                    );
                    failed = true;
                    break;
                }
            }
            if !failed {
                println!("\n\x1b[1;32m✔ Seluruh role addon di-reset ke Main Model.\x1b[0m\n");
            }
        } else {
            break;
        }
    }
}

async fn run_cli_addon_role_submenu(ai_service: &AIChatService, role: ModelRole) {
    let providers = ai_service.get_user_providers(0).await;
    let routing = ai_service.model_routing_config().await;
    let curr_route = routing
        .route(role)
        .cloned()
        .unwrap_or(ModelRoute::MainModel);
    let route_label = addon_route_text(&curr_route, &providers);

    let summary = format!(
        "== Addon Role: {} ==\r\n\
         • Route Saat Ini: \x1b[1;36m{}\x1b[0m",
        role.display_name(),
        route_label
    );

    let options = vec![
        "○ Gunakan Main Model (Inherited / Default)".to_string(),
        "✖ Nonaktifkan Role Ini (Disabled)".to_string(),
        "◆ Pilih Model Spesifik dari Provider...".to_string(),
        "← Kembali".to_string(),
    ];

    let sel = terminal_interactive_select(&summary, &options, 0, false, None);
    let Some(choice) = sel else {
        return;
    };

    match choice {
        0 => {
            if let Err(e) = ai_service
                .set_model_route(role, ModelRoute::MainModel)
                .await
            {
                println!(
                    "\n\x1b[31m✖ Error: Gagal menyimpan route {}: {e}\x1b[0m\n",
                    role.display_name()
                );
            }
        }
        1 => {
            if let Err(e) = ai_service.set_model_route(role, ModelRoute::Disabled).await {
                println!(
                    "\n\x1b[31m✖ Error: Gagal menonaktifkan route {}: {e}\x1b[0m\n",
                    role.display_name()
                );
            }
        }
        2 => {
            let mut choices = Vec::new();
            let mut routes = Vec::new();
            for prov in &providers {
                for m in &prov.models {
                    choices.push(format!("{} :: {}", prov.name, m));
                    routes.push(ModelRoute::Specific {
                        provider_id: prov.id.clone(),
                        model: m.clone(),
                    });
                }
            }
            if choices.is_empty() {
                println!("\n\x1b[31m✖ Tidak ada model provider yang tersedia.\x1b[0m\n");
                return;
            }
            if let Some(m_idx) = terminal_interactive_select(
                &format!("Pilih Model Spesifik untuk {}:", role.display_name()),
                &choices,
                0,
                true,
                None,
            ) {
                let chosen_route = routes[m_idx].clone();
                let chosen_label = choices[m_idx].clone();
                if let Err(e) = ai_service.set_model_route(role, chosen_route).await {
                    println!(
                        "\n\x1b[31m✖ Error: Gagal menyimpan route {}: {e}\x1b[0m\n",
                        role.display_name()
                    );
                } else {
                    println!(
                        "\n\x1b[1;32m✔ Route {} diarahkan ke: {}\x1b[0m",
                        role.display_name(),
                        chosen_label
                    );
                }
            }
        }
        _ => {}
    }
}

async fn run_cli_addon_test_all_routes(ai_service: &AIChatService) {
    println!("\n\x1b[1;36mDiagnostik & Uji Kapabilitas Seluruh Rute Addon Aktif...\x1b[0m\n");
    let providers = ai_service.get_user_providers(0).await;
    let routing = ai_service.model_routing_config().await;

    for role in ModelRole::addon_roles() {
        let route = routing
            .route(role)
            .cloned()
            .unwrap_or(ModelRoute::MainModel);
        let route_str = addon_route_text(&route, &providers);

        if route == ModelRoute::Disabled {
            println!(
                "  ● {:<22} : \x1b[38;5;244m✖ Disabled (Dilewati)\x1b[0m\n",
                role.display_name()
            );
            continue;
        }

        println!("  ● \x1b[1m{}\x1b[0m → {}", role.display_name(), route_str);

        if role == ModelRole::ImageGeneration {
            print!("    Uji Image Generation? (dapat menggunakan kuota API) [y/N]: ");
            let _ = io::stdout().flush();
            let mut ans = String::new();
            let _ = io::stdin().read_line(&mut ans);
            if !ans.trim().eq_ignore_ascii_case("y") {
                println!("    \x1b[38;5;244m○ Uji Image Generation dilewati.\x1b[0m\n");
                continue;
            }
            match ai_service
                .probe_image_generation_active_with_observer(
                    ModelRole::ImageGeneration,
                    print_probe_event,
                )
                .await
            {
                Ok((_rec, ProbeOutcome::Supported)) => {
                    println!(
                        "    \x1b[1;32m✔ Sukses: Image Generation terverifikasi & berfungsi normal.\x1b[0m"
                    );
                }
                Ok((rec, outcome)) => {
                    println!(
                        "    \x1b[31m✖ Gagal: Hasil probe {:?}, status tersimpan {:?}.\x1b[0m",
                        outcome,
                        rec.effective_state_for(CapabilityKind::ImageGeneration)
                    );
                }
                Err(e) => {
                    println!("    \x1b[31m✖ Error: {e}\x1b[0m");
                }
            }
        } else {
            match ai_service
                .probe_addon_role_with_observer(role, print_probe_event)
                .await
            {
                Ok((record, status)) => match status {
                    ProbeOutcome::Supported => {
                        println!(
                            "    \x1b[32m✔ Kapabilitas terverifikasi & tersimpan di SQLite ({})\x1b[0m",
                            record.checked_at
                        );
                    }
                    ProbeOutcome::Unsupported => {
                        println!(
                            "    \x1b[31m✖ Model menolak kapabilitas ini (Unsupported).\x1b[0m"
                        );
                    }
                    _ => {
                        println!(
                            "    \x1b[33m○ Status kapabilitas belum terbukti ({status:?}).\x1b[0m"
                        );
                    }
                },
                Err(e) => {
                    println!("    \x1b[31m✖ Error probe: {e}\x1b[0m");
                }
            }
        }
        println!();
    }

    println!("\x1b[1;32m✔ Selesai memeriksa seluruh rute addon.\x1b[0m");
    print_press_enter();
}

pub(crate) async fn run_cli_probe_menu(ai_service: &AIChatService) {
    load_environment();
    loop {
        let menu_items = vec![
            "🔍 Audit & Refresh Semua Model Aktif".to_string(),
            "👁️  Uji Spesialis Vision (Live Test)".to_string(),
            "🎬 Uji Spesialis Video (Live Test)".to_string(),
            "🎙️  Uji Spesialis Audio STT (Live Test)".to_string(),
            "🎨 Uji Spesialis Image Gen (Live Test Gambar)".to_string(),
            "📋 Lihat Cache Kapabilitas SQLite".to_string(),
            "✕ Selesai / Keluar".to_string(),
        ];

        let sel = terminal_interactive_select(
            "Pusat Diagnostik & Probe Kapabilitas:",
            &menu_items,
            0,
            false,
            None,
        );

        let Some(idx) = sel else {
            break;
        };

        match idx {
            0 => {
                run_cli_probe_all_active(ai_service).await;
                print_press_enter();
            }
            1 => {
                run_cli_probe_test_role(ai_service, ModelRole::Vision).await;
                print_press_enter();
            }
            2 => {
                run_cli_probe_test_role(ai_service, ModelRole::Video).await;
                print_press_enter();
            }
            3 => {
                run_cli_probe_test_role(ai_service, ModelRole::AudioStt).await;
                print_press_enter();
            }
            4 => {
                run_cli_probe_test_image_gen(ai_service).await;
                print_press_enter();
            }
            5 => {
                run_cli_probe_show_registry().await;
                print_press_enter();
            }
            _ => break,
        }
    }
}

fn print_press_enter() {
    print!("\n\x1b[38;5;244mTekan Enter untuk kembali...\x1b[0m");
    let _ = io::stdout().flush();
    let mut tmp = String::new();
    let _ = io::stdin().read_line(&mut tmp);
}

fn capability_display_label(cap: CapabilityKind) -> &'static str {
    match cap {
        CapabilityKind::TextChat => "Text Chat",
        CapabilityKind::ImageInput => "Vision (Image)",
        CapabilityKind::ImageGeneration => "Image Generation",
        CapabilityKind::ImageEditing => "Image Editing",
        CapabilityKind::AudioInput => "Audio Native",
        CapabilityKind::AudioTranscription => "Audio STT",
        CapabilityKind::VideoInput => "Video Frames",
        CapabilityKind::NativeFileInput => "Native File",
        CapabilityKind::Tools => "Tools / Function",
        CapabilityKind::StructuredOutput => "Structured JSON",
        CapabilityKind::Reasoning => "Reasoning",
    }
}

fn format_probe_outcome_badge(outcome: ProbeOutcome) -> String {
    match outcome {
        ProbeOutcome::Supported => "\x1b[1;32m✔ Supported\x1b[0m".to_string(),
        ProbeOutcome::Unsupported => "\x1b[1;31m✖ Unsupported\x1b[0m".to_string(),
        ProbeOutcome::Inconclusive => "\x1b[33m○ Inconclusive\x1b[0m".to_string(),
        ProbeOutcome::Timeout => "\x1b[33m! Timeout\x1b[0m".to_string(),
        ProbeOutcome::NetworkError => "\x1b[31m✖ NetworkError\x1b[0m".to_string(),
        ProbeOutcome::ProtocolMismatch => "\x1b[33m! ProtocolMismatch\x1b[0m".to_string(),
        ProbeOutcome::AuthFailed => "\x1b[31m✖ AuthFailed\x1b[0m".to_string(),
        ProbeOutcome::RateLimited => "\x1b[33m! RateLimited\x1b[0m".to_string(),
        ProbeOutcome::ProviderError => "\x1b[31m✖ ProviderError\x1b[0m".to_string(),
    }
}

fn format_cap_bool_badge(val: Option<bool>) -> &'static str {
    match val {
        Some(true) => "\x1b[32m✔ Supported\x1b[0m",
        Some(false) => "\x1b[31m✖ Unsupported\x1b[0m",
        None => "\x1b[38;5;244m○ Unknown\x1b[0m",
    }
}

pub(crate) async fn run_cli_probe_all_active(ai_service: &AIChatService) {
    println!("\n\x1b[1;36mMemeriksa Kapabilitas Model Aktif...\x1b[0m\n");
    let providers = ai_service.get_user_providers(0).await;
    if providers.is_empty() {
        println!("  \x1b[33m✖ Belum ada AI provider yang terdaftar.\x1b[0m");
        return;
    }

    for prov in &providers {
        let model = &prov.active_model;
        if model.is_empty() {
            continue;
        }
        println!("  ● Provider: \x1b[1m{}\x1b[0m ({})", prov.name, model);
        if let Some(record) = run_persisted_capability_probe(ai_service, prov, model).await {
            println!("    \x1b[1;37mRingkasan Diagnostik Kapabilitas:\x1b[0m");
            println!(
                "      • Text Chat        : {}",
                format_cap_bool_badge(record.supports_text_chat)
            );
            println!(
                "      • Vision (Image)   : {}",
                format_cap_bool_badge(record.supports_image_input)
            );
            println!(
                "      • Structured JSON  : {}",
                format_cap_bool_badge(record.supports_structured_output)
            );
            println!(
                "      • Tools / Function : {}",
                format_cap_bool_badge(record.supports_tools)
            );
            println!(
                "      • Audio Native     : {}",
                format_cap_bool_badge(record.supports_audio_input)
            );
            println!(
                "      • Audio STT        : {}",
                format_cap_bool_badge(record.supports_audio_transcription)
            );
            println!(
                "      • Video Frames     : {}",
                format_cap_bool_badge(record.supports_video_input)
            );
            if let Some(ctx) = record.context_window {
                println!("      • Context Limit    : \x1b[36m{} tokens\x1b[0m", ctx);
            }
        } else {
            println!(
                "    \x1b[31m✖ Verifikasi kapabilitas gagal / endpoint tidak merespons.\x1b[0m"
            );
        }
        println!();
    }
    println!("\x1b[1;32m✔ Diagnostik selesai. Hasil tidak membatasi penggunaan route.\x1b[0m");
}

async fn run_cli_probe_test_role(ai_service: &AIChatService, role: ModelRole) {
    println!(
        "\n\x1b[1;36mDiagnostik & Live Test: {}\x1b[0m",
        role.display_name()
    );
    println!("  Diagnostik opsional route, bukan syarat penggunaan...");
    match ai_service
        .probe_addon_role_with_observer(role, print_probe_event)
        .await
    {
        Ok((record, status)) => match status {
            ProbeOutcome::Supported => {
                println!(
                    "  ✔ Observed Supported (checked: {}); see persistence result above",
                    record.checked_at
                );
            }
            ProbeOutcome::Unsupported => {
                println!("  ✖ Probe executed: Model explicitly rejected capability.");
            }
            ProbeOutcome::Inconclusive
            | ProbeOutcome::AuthFailed
            | ProbeOutcome::RateLimited
            | ProbeOutcome::Timeout
            | ProbeOutcome::NetworkError
            | ProbeOutcome::ProtocolMismatch
            | ProbeOutcome::ProviderError => {
                println!("  ⚪ Completed but not verified: Result is inconclusive/stale.");
            }
        },
        Err(e) => {
            println!("  ✖ Error / PersistenceFailed: {e}");
        }
    }
}

async fn run_cli_probe_test_image_gen(ai_service: &AIChatService) {
    println!("\n\x1b[1;36mLive Test: Image Generation\x1b[0m");
    println!("\x1b[33mPerhatian: Pengujian ini akan membuat gambar uji dan dapat menggunakan kredit API.\x1b[0m");
    print!("Lanjutkan pengujian? [y/N]: ");
    let _ = io::stdout().flush();
    let mut ans = String::new();
    let _ = io::stdin().read_line(&mut ans);
    if !ans.trim().eq_ignore_ascii_case("y") {
        println!("○ Pengujian dibatalkan.");
        return;
    }

    println!("Membuat gambar uji...");
    match ai_service
        .probe_image_generation_active_with_observer(ModelRole::ImageGeneration, print_probe_event)
        .await
    {
        Ok((_rec, ProbeOutcome::Supported)) => {
            println!(
                "  \x1b[1;32m✔ Sukses: Gambar berhasil dibuat dan lolos validasi runtime.\x1b[0m"
            );
        }
        Ok((rec, outcome)) => {
            println!(
                "  \x1b[31m✖ Gagal: Hasil probe {:?}, status tersimpan {:?}.\x1b[0m",
                outcome,
                rec.effective_state_for(CapabilityKind::ImageGeneration)
            );
        }
        Err(e) => {
            println!("  \x1b[31m✖ Error: {e}\x1b[0m");
        }
    }
}

async fn run_cli_probe_show_registry() {
    let registry = crate::ai::service::load_capability_registry();
    println!(
        "\n\x1b[1;36mCapability Registry (Total {} model):\x1b[0m\n",
        registry.models.len()
    );
    if registry.models.is_empty() {
        println!(
            "  \x1b[38;5;244mBelum ada kapabilitas model yang tersimpan di registry.\x1b[0m\n"
        );
        return;
    }
    for r in &registry.models {
        let vision = if r.supports_image_input == Some(true) {
            "\x1b[32m✔ Vision\x1b[0m"
        } else {
            "\x1b[38;5;244m○ Vision\x1b[0m"
        };
        let audio = if r.supports_audio_input == Some(true)
            || r.supports_audio_transcription == Some(true)
        {
            "\x1b[32m✔ Audio\x1b[0m"
        } else {
            "\x1b[38;5;244m○ Audio\x1b[0m"
        };
        let tools = if r.supports_tools == Some(true) {
            "\x1b[32m✔ Tools\x1b[0m"
        } else {
            "\x1b[38;5;244m○ Tools\x1b[0m"
        };
        let ctx_str = r
            .context_window
            .map(|c| format!(" · Ctx: {c}"))
            .unwrap_or_default();
        println!(
            "  ● \x1b[1m{}\x1b[0m ({})\n    [{vision} · {audio} · {tools}{ctx_str}] \x1b[38;5;244m· {}\x1b[0m",
            r.model, r.provider_name, r.checked_at
        );
    }
}

fn addon_route_text(route: &ModelRoute, providers: &[ProviderConfig]) -> String {
    match route {
        ModelRoute::MainModel => "Main Model".to_string(),
        ModelRoute::Disabled => "Disabled".to_string(),
        ModelRoute::Specific { provider_id, model } => {
            let provider = providers
                .iter()
                .find(|p| &p.id == provider_id)
                .map(|p| p.name.as_str())
                .unwrap_or(provider_id.as_str());
            format!("{} :: {}", provider, model)
        }
    }
}

fn print_probe_event(event: ProbeEvent) {
    match event {
        ProbeEvent::Started { .. } => {}
        ProbeEvent::Progress {
            capability,
            message,
        } => {
            if message.starts_with("Vision 1/2") || message.starts_with("Vision 2/2") {
                println!(
                    "    ├─ {:<20} : \x1b[38;5;244m{}\x1b[0m",
                    capability_display_label(capability),
                    message
                );
            }
        }
        ProbeEvent::Completed {
            capability,
            outcome,
        } => {
            println!(
                "    ├─ {:<20} : {}",
                capability_display_label(capability),
                format_probe_outcome_badge(outcome)
            );
        }
        ProbeEvent::Skipped { capability, reason } => {
            println!(
                "    ├─ {:<20} : \x1b[38;5;244m○ Skipped ({})\x1b[0m",
                capability_display_label(capability),
                reason
            );
        }
        ProbeEvent::Persistence { saved } => {
            if saved {
                println!("    └─ Persist Registry     : \x1b[32m✔ Saved to SQLite\x1b[0m");
            } else {
                println!("    └─ Persist Registry     : \x1b[31m✖ Persistence Failed\x1b[0m");
            }
        }
        ProbeEvent::Finished => {}
    }
}

async fn run_persisted_capability_probe(
    ai_service: &AIChatService,
    provider: &ProviderConfig,
    model: &str,
) -> Option<crate::ai::service::CapabilityRecord> {
    let candidate = ai_service
        .probe_model_capabilities_with_observer(provider, model, print_probe_event)
        .await;
    let persisted = ai_service
        .capability_record(&provider.endpoint, model)
        .await;
    match persisted {
        Some(record) if record.checked_at == candidate.checked_at => Some(record),
        _ => None,
    }
}

fn find_matching_model_in_provider(prov: &ProviderConfig, query: &str) -> Option<String> {
    if let Some(m) = prov.models.iter().find(|m| *m == query) {
        return Some(m.clone());
    }
    if let Some(m) = prov.models.iter().find(|m| m.eq_ignore_ascii_case(query)) {
        return Some(m.clone());
    }
    if prov.active_model.eq_ignore_ascii_case(query) && !prov.active_model.trim().is_empty() {
        return Some(prov.active_model.clone());
    }
    None
}

pub(crate) fn find_model_in_store<'a>(
    store: &'a ProviderStore,
    target: &str,
) -> Option<(&'a ProviderConfig, String)> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }

    if let Some((prov_query, model_query)) = target.split_once('/') {
        let prov_query = prov_query.trim();
        let model_query = model_query.trim();

        let matched_prov = store
            .providers
            .iter()
            .find(|p| p.id == prov_query || p.name == prov_query)
            .or_else(|| {
                store.providers.iter().find(|p| {
                    p.id.eq_ignore_ascii_case(prov_query) || p.name.eq_ignore_ascii_case(prov_query)
                })
            });

        if let Some(prov) = matched_prov {
            if let Some(m) = find_matching_model_in_provider(prov, model_query) {
                return Some((prov, m));
            }
        }
    }

    let active_prov = store
        .active_id
        .as_deref()
        .and_then(|aid| store.providers.iter().find(|p| p.id == aid))
        .or_else(|| store.providers.first());

    if let Some(prov) = active_prov {
        if let Some(m) = find_matching_model_in_provider(prov, target) {
            return Some((prov, m));
        }
    }

    let active_prov_id = active_prov.map(|p| p.id.as_str()).unwrap_or("");
    for prov in &store.providers {
        if prov.id == active_prov_id {
            continue;
        }
        if let Some(m) = find_matching_model_in_provider(prov, target) {
            return Some((prov, m));
        }
    }

    None
}

pub(crate) async fn run_cli_ai_hub(
    ai_service: &AIChatService,
    action: Option<&str>,
    target: Option<&str>,
) {
    load_environment();
    match action {
        None => loop {
            let store = load_provider_store();
            let routing = ai_service.model_routing_config().await;

            let active_provider = if let Some(ref aid) = store.active_id {
                store.providers.iter().find(|p| &p.id == aid)
            } else {
                store.providers.first()
            };

            let (active_prov_name, active_model_name) = match active_provider {
                Some(p) => (
                    p.name.as_str(),
                    if p.active_model.trim().is_empty() {
                        "Belum diset"
                    } else {
                        p.active_model.as_str()
                    },
                ),
                None => ("Belum ada", "Belum diset"),
            };

            let total_providers = store.providers.len();

            let mut addon_parts = Vec::new();
            for role in ModelRole::addon_roles() {
                let route = routing
                    .route(role)
                    .cloned()
                    .unwrap_or(ModelRoute::MainModel);
                let route_desc = match &route {
                    ModelRoute::MainModel => "Main".to_string(),
                    ModelRoute::Disabled => "Off".to_string(),
                    ModelRoute::Specific { model, .. } => model.clone(),
                };
                let label = match role {
                    ModelRole::Vision => "Vision",
                    ModelRole::Video => "Video",
                    ModelRole::AudioStt => "STT",
                    ModelRole::ImageGeneration => "Image",
                    _ => role.display_name(),
                };
                addon_parts.push(format!("{label}: {route_desc}"));
            }
            let addon_summary = addon_parts.join(" | ");

            let title = format!(
                "== Xiao AI Management Hub ==\r\n\
                     • Active Model   : {}\r\n\
                     • Active Provider: {} (Total: {})\r\n\
                     • Addon Routes   : {}",
                active_model_name, active_prov_name, total_providers, addon_summary
            );

            let menu_items = vec![
                "🎯 Select / Switch Main Model".to_string(),
                "🔌 Manage Providers (Add / Remove / Switch)".to_string(),
                "🧩 Manage Multimodal Addons (Vision, STT, Video, Image)".to_string(),
                "🧪 Run AI Diagnostics (Live Probe)".to_string(),
                "✕ Exit".to_string(),
            ];

            let sel = terminal_interactive_select(&title, &menu_items, 0, false, None);
            let Some(idx) = sel else {
                break;
            };

            match idx {
                0 => {
                    run_cli_model_picker(ai_service, None).await;
                }
                1 => {
                    run_cli_provider_menu(ai_service, None).await;
                }
                2 => {
                    run_cli_addon_menu(ai_service).await;
                }
                3 => {
                    run_cli_probe_menu(ai_service).await;
                }
                _ => break,
            }
        },
        Some("use") => {
            let target = match target {
                Some(t) if !t.trim().is_empty() => t.trim(),
                _ => {
                    run_cli_model_picker(ai_service, None).await;
                    return;
                }
            };

            let mut store = load_provider_store();
            if let Some((matched_provider, matched_model)) = find_model_in_store(&store, target) {
                let prov_id = matched_provider.id.clone();
                let prov_name = matched_provider.name.clone();
                let prov_endpoint = matched_provider.endpoint.clone();
                let model_name = matched_model;

                store.active_id = Some(prov_id.clone());
                if let Some(p) = store.providers.iter_mut().find(|p| p.id == prov_id) {
                    p.active_model = model_name.clone();
                }

                if let Err(e) = save_provider_store(&store) {
                    println!("\x1b[31m✖ Error: Gagal menyimpan konfigurasi provider: {e}\x1b[0m");
                    return;
                }

                if !ai_service.reload_provider_store().await {
                    println!("\x1b[31m✖ Error: Gagal memuat ulang provider di runtime.\x1b[0m");
                    return;
                }

                println!(
                    "\x1b[32m✔\x1b[0m Main Model successfully switched to: \x1b[1m{}\x1b[0m",
                    model_name
                );
                println!("  • Provider : {} ({})", prov_name, prov_endpoint);
            } else {
                println!(
                    "\x1b[31m✖\x1b[0m Model '{}' not found in any registered provider.",
                    target
                );
                println!("  Run 'xiao ai list' to see available models or 'xiao ai add' to register a new provider.");
            }
        }
        Some("list") => {
            let store = load_provider_store();
            if store.providers.is_empty() {
                println!("\n\x1b[33mBelum ada AI Provider yang terdaftar.\x1b[0m");
                println!("  Jalankan 'xiao ai add' untuk menambahkan provider baru.\n");
                return;
            }

            let active_id = store.active_id.as_deref().unwrap_or("");
            let rows: Vec<(String, String, String, String, bool)> = store
                .providers
                .iter()
                .map(|p| {
                    let is_active = if active_id.is_empty() {
                        store
                            .providers
                            .first()
                            .map(|fp| fp.id == p.id)
                            .unwrap_or(false)
                    } else {
                        p.id == active_id
                    };
                    let status = if is_active {
                        "[ACTIVE]".to_string()
                    } else {
                        "INACTIVE".to_string()
                    };
                    let active_model = if p.active_model.trim().is_empty() {
                        "-".to_string()
                    } else {
                        p.active_model.clone()
                    };
                    (
                        p.name.clone(),
                        active_model,
                        p.models.len().to_string(),
                        status,
                        is_active,
                    )
                })
                .collect();

            let col_prov = rows
                .iter()
                .map(|r| r.0.len())
                .max()
                .unwrap_or(8)
                .max("PROVIDER".len());
            let col_model = rows
                .iter()
                .map(|r| r.1.len())
                .max()
                .unwrap_or(12)
                .max("ACTIVE MODEL".len());
            let col_total = rows
                .iter()
                .map(|r| r.2.len())
                .max()
                .unwrap_or(12)
                .max("TOTAL MODELS".len());
            let col_status = "STATUS".len().max(8);

            println!(
                "\n\x1b[1;37m{:<w_prov$}  {:<w_model$}  {:>w_total$}  {:<w_status$}\x1b[0m",
                "PROVIDER",
                "ACTIVE MODEL",
                "TOTAL MODELS",
                "STATUS",
                w_prov = col_prov,
                w_model = col_model,
                w_total = col_total,
                w_status = col_status,
            );
            let total_width = col_prov + col_model + col_total + col_status + 6;
            println!("\x1b[38;5;238m{}\x1b[0m", "─".repeat(total_width));

            for (prov, model, total, status, is_active) in rows {
                let status_styled = if is_active {
                    format!(
                        "\x1b[1;32m{:<w_status$}\x1b[0m",
                        status,
                        w_status = col_status
                    )
                } else {
                    format!(
                        "\x1b[38;5;244m{:<w_status$}\x1b[0m",
                        status,
                        w_status = col_status
                    )
                };
                println!(
                    "{:<w_prov$}  {:<w_model$}  {:>w_total$}  {}",
                    prov,
                    model,
                    total,
                    status_styled,
                    w_prov = col_prov,
                    w_model = col_model,
                    w_total = col_total,
                );
            }

            println!("\nUse 'xiao ai use <model>' to switch models. Use 'xiao ai add' to add a provider.\n");
        }
        Some("add") => {
            run_cli_provider_add(ai_service).await;
        }
        Some("rm") | Some("remove") => {
            run_cli_provider_remove(ai_service).await;
        }
        Some("provider") | Some("providers") => {
            run_cli_provider_menu(ai_service, target).await;
        }
        Some("addon") | Some("addons") => {
            run_cli_addon_menu(ai_service).await;
        }
        Some("test") | Some("probe") => match target {
            Some("all") => {
                run_cli_probe_all_active(ai_service).await;
                print_press_enter();
            }
            Some("vision") => {
                run_cli_probe_test_role(ai_service, ModelRole::Vision).await;
                print_press_enter();
            }
            Some("video") => {
                run_cli_probe_test_role(ai_service, ModelRole::Video).await;
                print_press_enter();
            }
            Some("stt") | Some("audio") => {
                run_cli_probe_test_role(ai_service, ModelRole::AudioStt).await;
                print_press_enter();
            }
            Some("image") | Some("img") => {
                run_cli_probe_test_image_gen(ai_service).await;
                print_press_enter();
            }
            _ => {
                run_cli_probe_menu(ai_service).await;
            }
        },
        Some("help") | Some("--help") | Some("-h") => {
            println!("\n\x1b[1;36mxiao ai — Unified AI Management Hub\x1b[0m\n");
            println!("\x1b[1;37mUsage:\x1b[0m");
            println!("  xiao ai [action]\n");
            println!("\x1b[1;37mSubcommands for 'ai':\x1b[0m");
            println!("     \x1b[36mxiao ai\x1b[0m             Open Interactive AI Center Hub");
            println!("     \x1b[36mxiao ai use <model>\x1b[0m Switch Main Model directly");
            println!(
                "     \x1b[36mxiao ai list\x1b[0m        Print table of registered providers and models"
            );
            println!("     \x1b[36mxiao ai [add|rm]\x1b[0m    Add or remove an OpenAI-compatible AI provider");
            println!("     \x1b[36mxiao ai addon\x1b[0m       Configure multimodal specialist routes (Vision, STT, Video, Image)");
            println!("     \x1b[36mxiao ai test [role]\x1b[0m Open live diagnostic probe center (or test: vision, stt, video, image, all)\n");
        }
        Some(unknown) => {
            println!("\x1b[31m✖ Error: Sub-perintah 'ai {unknown}' tidak dikenal.\x1b[0m");
            println!("  Jalankan 'xiao ai help' atau 'xiao help' untuk bantuan.");
        }
    }
}

pub(crate) fn print_cli_help() {
    println!(
        "\n\x1b[1;36mxiao v{} — AI Assistant Bot\x1b[0m\n",
        env!("CARGO_PKG_VERSION")
    );
    println!("\x1b[1;37mUsage:\x1b[0m");
    println!("  xiao <command>\n");
    println!("\x1b[1;37mCommands:\x1b[0m");
    println!("  \x1b[36mstart\x1b[0m               Run bot daemon (default)");
    println!(
        "  \x1b[36msetup\x1b[0m               Interactive initial setup wizard (AI -> Telegram)"
    );
    println!("  \x1b[36mstatus\x1b[0m              Display system, database, and provider status dashboard");
    println!("  \x1b[36mai [action]\x1b[0m         Unified AI management hub (Model, Provider, Addon) [Interactive/One-Liner]");
    println!("  \x1b[36mgateway [action]\x1b[0m    Manage Telegram messaging gateway (Token & Owner ID) [Interactive/One-Liner]");
    println!("  \x1b[36mversion, -v\x1b[0m         Display binary version");
    println!("  \x1b[36mhelp\x1b[0m                Show this help message\n");
    println!("\x1b[1;37mSubcommands for 'ai':\x1b[0m");
    println!("     \x1b[36mxiao ai\x1b[0m             Open Interactive AI Center Hub");
    println!("     \x1b[36mxiao ai use <model>\x1b[0m Switch Main Model directly");
    println!(
        "     \x1b[36mxiao ai list\x1b[0m        Print table of registered providers and models"
    );
    println!(
        "     \x1b[36mxiao ai [add|rm]\x1b[0m    Add or remove an OpenAI-compatible AI provider"
    );
    println!("     \x1b[36mxiao ai addon\x1b[0m       Configure multimodal specialist routes (Vision, STT, Video, Image)");
    println!("     \x1b[36mxiao ai test [role]\x1b[0m Open live diagnostic probe center (or test: vision, stt, video, image, all)\n");
    println!("\x1b[1;37mSubcommands for 'gateway':\x1b[0m");
    println!("     \x1b[36mxiao gateway\x1b[0m                Open Interactive Gateway Manager");
    println!(
        "     \x1b[36mxiao gateway check\x1b[0m          Verify bot token connectivity (getMe)"
    );
    println!("     \x1b[36mxiao gateway token <TOKEN>\x1b[0m  Bind and verify Telegram Bot Token");
    println!("     \x1b[36mxiao gateway owner <ID>\x1b[0m     Set Telegram Owner User ID\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_model_in_store_exact_and_cross_provider() {
        let store = ProviderStore {
            active_id: Some("groq-1".to_string()),
            providers: vec![
                ProviderConfig {
                    id: "groq-1".to_string(),
                    name: "Groq".to_string(),
                    endpoint: "https://api.groq.com/openai/v1".to_string(),
                    api_key: "".to_string(),
                    api_key_ref: None,
                    models: vec![
                        "llama-3.3-70b-versatile".to_string(),
                        "llama-3.1-8b-instant".to_string(),
                    ],
                    active_model: "llama-3.3-70b-versatile".to_string(),
                },
                ProviderConfig {
                    id: "openai-1".to_string(),
                    name: "OpenAI".to_string(),
                    endpoint: "https://api.openai.com/v1".to_string(),
                    api_key: "".to_string(),
                    api_key_ref: None,
                    models: vec![
                        "gpt-4o".to_string(),
                        "gpt-4o-mini".to_string(),
                        "meta-llama/llama-3.1-8b-instruct".to_string(),
                    ],
                    active_model: "gpt-4o".to_string(),
                },
            ],
        };

        // Match in active provider
        let res = find_model_in_store(&store, "llama-3.1-8b-instant");
        assert!(res.is_some());
        let (p, m) = res.unwrap();
        assert_eq!(p.id, "groq-1");
        assert_eq!(m, "llama-3.1-8b-instant");

        // Model name containing a slash when prefix is not a provider
        let res = find_model_in_store(&store, "meta-llama/llama-3.1-8b-instruct");
        assert!(res.is_some());
        let (p, m) = res.unwrap();
        assert_eq!(p.id, "openai-1");
        assert_eq!(m, "meta-llama/llama-3.1-8b-instruct");

        // Match across other provider
        let res = find_model_in_store(&store, "gpt-4o-mini");
        assert!(res.is_some());
        let (p, m) = res.unwrap();
        assert_eq!(p.id, "openai-1");
        assert_eq!(m, "gpt-4o-mini");

        // Case-insensitive match across provider
        let res = find_model_in_store(&store, "GPT-4O-MINI");
        assert!(res.is_some());
        let (p, m) = res.unwrap();
        assert_eq!(p.id, "openai-1");
        assert_eq!(m, "gpt-4o-mini");

        // Delimited provider/model match with name
        let res = find_model_in_store(&store, "openai/gpt-4o");
        assert!(res.is_some());
        let (p, m) = res.unwrap();
        assert_eq!(p.id, "openai-1");
        assert_eq!(m, "gpt-4o");

        // Delimited provider/model match with ID
        let res = find_model_in_store(&store, "groq-1/llama-3.1-8b-instant");
        assert!(res.is_some());
        let (p, m) = res.unwrap();
        assert_eq!(p.id, "groq-1");
        assert_eq!(m, "llama-3.1-8b-instant");

        // Delimited with non-existent model in that provider
        let res = find_model_in_store(&store, "groq/gpt-4o");
        assert!(res.is_none());

        // Model not found anywhere
        let res = find_model_in_store(&store, "claude-3-5-sonnet");
        assert!(res.is_none());
    }

    #[test]
    fn print_cli_help_executes() {
        print_cli_help();
    }
}
