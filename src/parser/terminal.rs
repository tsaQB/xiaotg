use regex::Regex;

/// Renders markdown text into an elegantly styled, ANSI-escaped terminal representation.
///
/// Features:
/// - Media tags ([photo: ...], [video: ...], [document: ...], etc.) -> Styled terminal asset cards
/// - Fenced code blocks (```lang ... ```) -> Bordered syntax boxes
/// - Markdown tables (| a | b |) -> Clean Unicode grid tables
/// - Headings (#, ##, ###) -> Bold colored section bars
/// - Blockquotes (>) & GitHub Alerts ([!NOTE], [!WARNING], etc.) -> Styled callouts
/// - Bullet & numbered lists -> Colored bullets and aligned items
/// - Inline styling (**bold**, *italic*, `code`, ~~strike~~, links)
/// - Strips leaked thinking tokens or raw tool calls
pub fn render_terminal_markdown(input: &str) -> String {
    if input.trim().is_empty() {
        return String::new();
    }

    // 1. Sanitize leaked artifacts (<think>, <thought>, <tool_call>, etc.)
    let cleaned = sanitize_terminal_input(input);
    if cleaned.trim().is_empty() {
        return String::new();
    }

    // 2. Isolate embedded media tags into standalone lines
    let isolated = super::markdown::isolate_embedded_media_blocks(&cleaned);

    let lines: Vec<&str> = isolated.lines().collect();
    let n = lines.len();
    let mut i = 0;
    let mut out: Vec<String> = Vec::new();

    let heading_re = Regex::new(r"^(#{1,6})\s*([^\s#].*)$").ok();
    let divider_re = Regex::new(r"^(\-{3,}|\*{3,}|_{3,}|─{3,}|—{2,})$").ok();
    let bullet_re = Regex::new(r"^[-*•]\s+(.+)$").ok();
    let numbered_re = Regex::new(r"^(\d+[\.)])\s+(.+)$").ok();

    while i < n {
        let line = lines[i];
        let trimmed = line.trim();

        // Blank lines
        if trimmed.is_empty() {
            out.push(String::new());
            i += 1;
            continue;
        }

        // Fenced code blocks
        if let Some(after_fence) = trimmed.strip_prefix("```") {
            let lang = after_fence.trim();
            let lang_label = if lang.is_empty() { "code" } else { lang };
            let mut code_lines = Vec::new();
            i += 1;
            while i < n && !lines[i].trim().starts_with("```") {
                code_lines.push(lines[i]);
                i += 1;
            }
            if i < n && lines[i].trim().starts_with("```") {
                i += 1;
            }

            out.push(format!(
                "  \x1b[38;5;240m┌─\x1b[0m \x1b[1;38;5;153m{lang_label}\x1b[0m \x1b[38;5;240m─────────────────────────────────────────\x1b[0m"
            ));
            for cl in code_lines {
                out.push(format!(
                    "  \x1b[38;5;240m│\x1b[0m \x1b[38;5;223m{cl}\x1b[0m"
                ));
            }
            out.push(
                "  \x1b[38;5;240m└────────────────────────────────────────────────\x1b[0m"
                    .to_string(),
            );
            continue;
        }

        // Markdown tables
        if trimmed.contains('|') && i + 1 < n && is_table_separator(lines[i + 1].trim()) {
            let mut table_rows: Vec<Vec<String>> = Vec::new();
            // Header row
            table_rows.push(parse_table_cells(trimmed));
            i += 2; // Skip header and separator

            while i < n {
                let row_str = lines[i].trim();
                if row_str.is_empty() || !row_str.contains('|') {
                    break;
                }
                table_rows.push(parse_table_cells(row_str));
                i += 1;
            }

            if !table_rows.is_empty() {
                out.push(render_terminal_table(&table_rows));
            }
            continue;
        }

        // Media blocks & document blocks
        if let Some(media_rendered) = try_render_terminal_media(trimmed) {
            out.push(media_rendered);
            i += 1;
            continue;
        }

        // Headings
        if let Some(caps) = heading_re.as_ref().and_then(|r| r.captures(trimmed)) {
            let level = caps.get(1).map(|m| m.as_str().len()).unwrap_or(1);
            let heading_text = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            let styled_text = render_terminal_inline(heading_text);
            let (bar_color, text_color) = match level {
                1 => ("\x1b[1;38;5;81m", "\x1b[1;38;5;231m"),
                2 => ("\x1b[1;38;5;117m", "\x1b[1;38;5;231m"),
                3 => ("\x1b[1;38;5;153m", "\x1b[1;38;5;255m"),
                _ => ("\x1b[1;38;5;189m", "\x1b[1;38;5;252m"),
            };
            out.push(format!(
                "{bar_color}▌\x1b[0m {text_color}{styled_text}\x1b[0m"
            ));
            i += 1;
            continue;
        }

        // Dividers
        if divider_re.as_ref().is_some_and(|r| r.is_match(trimmed)) {
            out.push(
                "  \x1b[38;5;238m────────────────────────────────────────────────\x1b[0m"
                    .to_string(),
            );
            i += 1;
            continue;
        }

        // Blockquotes & GitHub Alerts
        if trimmed.starts_with('>') {
            let mut quote_lines = Vec::new();
            while i < n && lines[i].trim().starts_with('>') {
                let q = lines[i].trim();
                let stripped_q = q
                    .strip_prefix(">>>")
                    .unwrap_or_else(|| q.strip_prefix('>').unwrap_or(q))
                    .trim_start();
                quote_lines.push(stripped_q);
                i += 1;
            }

            if !quote_lines.is_empty() {
                let first = quote_lines[0];
                let alerts = [
                    ("[!NOTE]", "\x1b[1;38;5;81mℹ️  Catatan:\x1b[0m"),
                    ("[!note]", "\x1b[1;38;5;81mℹ️  Catatan:\x1b[0m"),
                    ("[!TIP]", "\x1b[1;38;5;114m💡 Tips:\x1b[0m"),
                    ("[!tip]", "\x1b[1;38;5;114m💡 Tips:\x1b[0m"),
                    ("[!IMPORTANT]", "\x1b[1;38;5;203m📌 Penting:\x1b[0m"),
                    ("[!important]", "\x1b[1;38;5;203m📌 Penting:\x1b[0m"),
                    ("[!WARNING]", "\x1b[1;38;5;214m⚠️  Peringatan:\x1b[0m"),
                    ("[!warning]", "\x1b[1;38;5;214m⚠️  Peringatan:\x1b[0m"),
                    ("[!CAUTION]", "\x1b[1;38;5;196m🚨 Perhatian:\x1b[0m"),
                    ("[!caution]", "\x1b[1;38;5;196m🚨 Perhatian:\x1b[0m"),
                ];

                let mut alert_prefix = None;
                for (marker, replacement) in alerts {
                    if let Some(stripped) = first.strip_prefix(marker) {
                        let rest = stripped.trim();
                        alert_prefix = Some((replacement, rest));
                        break;
                    }
                }

                if let Some((alert_tag, rest)) = alert_prefix {
                    if !rest.is_empty() {
                        out.push(format!("  {alert_tag} {}", render_terminal_inline(rest)));
                    } else {
                        out.push(format!("  {alert_tag}"));
                    }
                    for ql in &quote_lines[1..] {
                        out.push(format!(
                            "  \x1b[38;5;242m│\x1b[0m {}",
                            render_terminal_inline(ql)
                        ));
                    }
                } else {
                    for ql in quote_lines {
                        out.push(format!(
                            "  \x1b[38;5;242m│\x1b[0m \x1b[3m{}\x1b[0m",
                            render_terminal_inline(ql)
                        ));
                    }
                }
            }
            continue;
        }

        // Bullet lists
        if let Some(caps) = bullet_re.as_ref().and_then(|r| r.captures(trimmed)) {
            let item_text = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            out.push(format!(
                "  \x1b[38;5;81m•\x1b[0m {}",
                render_terminal_inline(item_text)
            ));
            i += 1;
            continue;
        }

        // Numbered lists
        if let Some(caps) = numbered_re.as_ref().and_then(|r| r.captures(trimmed)) {
            let num = caps.get(1).map(|m| m.as_str()).unwrap_or("1.");
            let item_text = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            out.push(format!(
                "  \x1b[38;5;81m{num}\x1b[0m {}",
                render_terminal_inline(item_text)
            ));
            i += 1;
            continue;
        }

        // Regular paragraph line with inline styling
        out.push(render_terminal_inline(line));
        i += 1;
    }

    out.join("\n").trim_end().to_string()
}

fn sanitize_terminal_input(input: &str) -> String {
    let step1 = Regex::new(r"(?is)<think>.*?</think>")
        .map(|r| r.replace_all(input, "").into_owned())
        .unwrap_or_else(|_| input.to_string());
    let step2 = Regex::new(r"(?is)<thought>.*?</thought>")
        .map(|r| r.replace_all(&step1, "").into_owned())
        .unwrap_or(step1);
    let step3 = Regex::new(r"(?is)<reasoning>.*?</reasoning>")
        .map(|r| r.replace_all(&step2, "").into_owned())
        .unwrap_or(step2);
    let step4 = Regex::new(r"(?is)<tool_call>.*?</tool_call>")
        .map(|r| r.replace_all(&step3, "").into_owned())
        .unwrap_or(step3);
    let step5 = Regex::new(r"(?is)<function_calls?>.*?</function_calls?>")
        .map(|r| r.replace_all(&step4, "").into_owned())
        .unwrap_or(step4);

    // Unclosed <think> or <thought>
    let mut cleaned = step5;
    for open_tag in ["<think>", "<thought>", "<reasoning>"] {
        if let Some(pos) = cleaned.to_lowercase().find(open_tag) {
            cleaned = cleaned[..pos].trim().to_string();
        }
    }

    // Clean stray tags
    if let Ok(re) = Regex::new(r"(?i)</?(?:think|thought|reasoning|tool_call|function_calls?)>") {
        cleaned = re.replace_all(&cleaned, "").into_owned();
    }

    cleaned
}

pub fn render_terminal_inline(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // Convert HTML tags to markdown before ANSI processing
    let mut s = text.to_string();
    if s.contains("<b") || s.contains("<strong") {
        if let Ok(re) = Regex::new(r"(?is)<(?:b|strong)(?:\s+[^>]*)?>(.*?)</(?:b|strong)>") {
            s = re.replace_all(&s, "**$1**").into_owned();
        }
    }
    if s.contains("<i") || s.contains("<em") {
        if let Ok(re) = Regex::new(r"(?is)<(?:i|em)(?:\s+[^>]*)?>(.*?)</(?:i|em)>") {
            s = re.replace_all(&s, "*$1*").into_owned();
        }
    }
    if s.contains("<code") {
        if let Ok(re) = Regex::new(r"(?is)<code(?:\s+[^>]*)?>(.*?)</code>") {
            s = re.replace_all(&s, "`$1`").into_owned();
        }
    }
    if s.contains("<u") || s.contains("<ins") {
        if let Ok(re) = Regex::new(r"(?is)<(?:u|ins)(?:\s+[^>]*)?>(.*?)</(?:u|ins)>") {
            s = re.replace_all(&s, "++$1++").into_owned();
        }
    }
    if s.contains("<s") || s.contains("<strike") || s.contains("<del") {
        if let Ok(re) = Regex::new(r"(?is)<(?:s|strike|del)(?:\s+[^>]*)?>(.*?)</(?:s|strike|del)>")
        {
            s = re.replace_all(&s, "~~$1~~").into_owned();
        }
    }
    if s.contains("<tg-spoiler") || s.contains("spoiler") {
        if let Ok(re) = Regex::new(r"(?is)<tg-spoiler(?:\s+[^>]*)?>(.*?)</tg-spoiler>") {
            s = re.replace_all(&s, "||$1||").into_owned();
        }
    }
    if s.contains("<a ") {
        if let Ok(re) = Regex::new(r#"(?is)<a\s+[^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>"#) {
            s = re.replace_all(&s, "[$2]($1)").into_owned();
        }
    }

    // Clean remaining HTML tags
    if let Ok(re) = Regex::new(r"</?[a-zA-Z][^>]*>") {
        s = re.replace_all(&s, "").into_owned();
    }
    s = html_escape::decode_html_entities(&s).to_string();

    // Inline media tag replacement in text
    if let Ok(re) = Regex::new(
        r#"(?i)!?\[(?:photo|foto|image|img|gambar|picture|pic)\s*:\s*([^\]]+)\]\s*\(([^)]+)\)"#,
    ) {
        s = re
            .replace_all(
                &s,
                "\x1b[1;38;5;117m📷 [Foto: $1]\x1b[0m (\x1b[4;38;5;39m$2\x1b[0m)",
            )
            .into_owned();
    }
    if let Ok(re) = Regex::new(r#"(?i)!?\[(?:video|vid)\s*:\s*([^\]]+)\]\s*\(([^)]+)\)"#) {
        s = re
            .replace_all(
                &s,
                "\x1b[1;38;5;214m🎬 [Video: $1]\x1b[0m (\x1b[4;38;5;39m$2\x1b[0m)",
            )
            .into_owned();
    }
    if let Ok(re) =
        Regex::new(r#"(?i)!?\[(?:audio|musik|music|lagu|song)\s*:\s*([^\]]+)\]\s*\(([^)]+)\)"#)
    {
        s = re
            .replace_all(
                &s,
                "\x1b[1;38;5;183m🎵 [Audio: $1]\x1b[0m (\x1b[4;38;5;39m$2\x1b[0m)",
            )
            .into_owned();
    }
    if let Ok(re) = Regex::new(
        r#"(?i)!?\[(?:voice|voicenote|voice_note|suara|rekaman|vn)\s*:\s*([^\]]+)\]\s*\(([^)]+)\)"#,
    ) {
        s = re
            .replace_all(
                &s,
                "\x1b[1;38;5;150m🎙️ [Voice: $1]\x1b[0m (\x1b[4;38;5;39m$2\x1b[0m)",
            )
            .into_owned();
    }
    if let Ok(re) =
        Regex::new(r#"(?i)!?\[(?:document|dokumen|doc|file|berkas)\s*:\s*([^\]]+)\]\s*\(([^)]+)\)"#)
    {
        s = re
            .replace_all(
                &s,
                "\x1b[1;38;5;222m📄 [Dokumen: $1]\x1b[0m (\x1b[4;38;5;39m$2\x1b[0m)",
            )
            .into_owned();
    }
    if let Ok(re) = Regex::new(r#"(?i)\[(?:map|location|lokasi|peta|geo)\s*:\s*([^\]]+)\]"#) {
        s = re
            .replace_all(&s, "\x1b[1;38;5;203m📍 [Lokasi: $1]\x1b[0m")
            .into_owned();
    }
    if let Ok(re) = Regex::new(r#"!\[([^\]]*)\]\(([^)]+)\)"#) {
        s = re
            .replace_all(&s, |caps: &regex::Captures| {
                let alt = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                let url = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                let label = if alt.is_empty() {
                    "Foto".to_string()
                } else if alt.to_lowercase().starts_with("foto:")
                    || alt.to_lowercase().starts_with("photo:")
                {
                    alt.to_string()
                } else {
                    format!("Foto: {alt}")
                };
                format!("\x1b[1;38;5;117m📷 [{label}]\x1b[0m (\x1b[4;38;5;39m{url}\x1b[0m)")
            })
            .into_owned();
    }

    // Standard markdown link [label](url)
    if let Ok(re) = Regex::new(r#"\[([^\]]+)\]\((https?://[^\s)]+|tg://[^\s)]+)\)"#) {
        s = re
            .replace_all(&s, "\x1b[38;5;81m$1\x1b[0m (\x1b[4;38;5;39m$2\x1b[0m)")
            .into_owned();
    }

    // Bold **text** or __text__
    if let Ok(re) = Regex::new(r"\*\*([^*]+)\*\*") {
        s = re.replace_all(&s, "\x1b[1m$1\x1b[0m").into_owned();
    }
    if let Ok(re) = Regex::new(r"__([^_]+)__") {
        s = re.replace_all(&s, "\x1b[1m$1\x1b[0m").into_owned();
    }

    // Underline ++text++
    if let Ok(re) = Regex::new(r"\+\+([^+]+)\+\+") {
        s = re.replace_all(&s, "\x1b[4m$1\x1b[0m").into_owned();
    }

    // Spoiler ||text||
    if let Ok(re) = Regex::new(r"\|\|([^|]+)\|\|") {
        s = re.replace_all(&s, "\x1b[7m$1\x1b[0m").into_owned();
    }

    // Strikethrough ~~text~~
    if let Ok(re) = Regex::new(r"~~([^~]+)~~") {
        s = re.replace_all(&s, "\x1b[9m$1\x1b[0m").into_owned();
    }

    // Inline code `code`
    if let Ok(re) = Regex::new(r"`([^`]+)`") {
        s = re.replace_all(&s, "\x1b[38;5;222m$1\x1b[0m").into_owned();
    }

    // Italic *text* or _text_ (limit to word-boundary like patterns)
    if let Ok(re) = Regex::new(r"(?:\*([^*]+)\*|\b_([^_]+)_\b)") {
        s = re
            .replace_all(&s, |caps: &regex::Captures| {
                let m = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .map(|v| v.as_str())
                    .unwrap_or("");
                format!("\x1b[3m{m}\x1b[0m")
            })
            .into_owned();
    }

    s
}

fn try_render_terminal_media(line: &str) -> Option<String> {
    let s = line.trim();
    let s_clean = s.trim_end_matches(['.', ',', ';', ':']);

    // Map: [map: lat, lon] or [map: label](coords)
    let map_re = Regex::new(
        r#"(?i)^\[(?:map|location|lokasi|peta|geo)\s*:\s*([^\]]+)\](?:\s*\(([^)]+)\))?$"#,
    )
    .ok()?;
    if let Some(caps) = map_re.captures(s_clean) {
        let label = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let coords = caps.get(2).map(|m| m.as_str().trim()).unwrap_or(label);
        let clean_coords = coords.trim().trim_start_matches("geo:").trim();
        let map_url = format!("https://www.google.com/maps?q={clean_coords}");
        return Some(format!(
            "  \x1b[1;38;5;203m📍 [Lokasi: {label}]\x1b[0m \x1b[4;38;5;39m{map_url}\x1b[0m"
        ));
    }

    // Media tag: [photo: label](url), [video: ...], etc.
    let media_re = Regex::new(r#"(?i)^!?\[(photo|foto|image|img|gambar|picture|pic|video|vid|audio|musik|music|lagu|song|voice|voicenote|voice_note|suara|rekaman|vn|animation|animasi|gif|collage|kolase|gallery|galeri|album|slideshow|slide|document|dokumen|doc|file|berkas)\s*:\s*([^\]]+)\]\s*\(([^)]+)\)$"#).ok()?;
    if let Some(caps) = media_re.captures(s_clean) {
        let tag = caps
            .get(1)
            .map(|m| m.as_str().to_lowercase())
            .unwrap_or_default();
        let label = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        let url = caps.get(3).map(|m| m.as_str().trim()).unwrap_or("");
        let clean_url = url.trim_start_matches('<').trim_end_matches('>').trim();

        return match tag.as_str() {
            "photo" | "foto" | "image" | "img" | "gambar" | "picture" | "pic" => Some(format!(
                "  \x1b[1;38;5;117m📷 [Foto: {label}]\x1b[0m \x1b[4;38;5;39m{clean_url}\x1b[0m"
            )),
            "video" | "vid" => Some(format!(
                "  \x1b[1;38;5;214m🎬 [Video: {label}]\x1b[0m \x1b[4;38;5;39m{clean_url}\x1b[0m"
            )),
            "audio" | "musik" | "music" | "lagu" | "song" => Some(format!(
                "  \x1b[1;38;5;183m🎵 [Audio: {label}]\x1b[0m \x1b[4;38;5;39m{clean_url}\x1b[0m"
            )),
            "voice" | "voicenote" | "voice_note" | "suara" | "rekaman" | "vn" => Some(format!(
                "  \x1b[1;38;5;150m🎙️ [Voice: {label}]\x1b[0m \x1b[4;38;5;39m{clean_url}\x1b[0m"
            )),
            "animation" | "animasi" | "gif" => Some(format!(
                "  \x1b[1;38;5;153m🎞️ [Animasi: {label}]\x1b[0m \x1b[4;38;5;39m{clean_url}\x1b[0m"
            )),
            "document" | "dokumen" | "doc" | "file" | "berkas" => Some(format!(
                "  \x1b[1;38;5;222m📄 [Dokumen: {label}]\x1b[0m \x1b[4;38;5;39m{clean_url}\x1b[0m"
            )),
            "collage" | "kolase" | "gallery" | "galeri" | "album" | "slideshow" | "slide" => {
                let urls: Vec<&str> = clean_url
                    .split(',')
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .collect();
                let mut lines = vec![format!(
                    "  \x1b[1;38;5;117m🖼️ [Galeri: {label}]\x1b[0m \x1b[38;5;244m({} item)\x1b[0m",
                    urls.len()
                )];
                for (idx, u) in urls.into_iter().enumerate() {
                    lines.push(format!(
                        "    \x1b[38;5;244m{}.\x1b[0m \x1b[4;38;5;39m{u}\x1b[0m",
                        idx + 1
                    ));
                }
                Some(lines.join("\n"))
            }
            _ => None,
        };
    }

    // Markdown image ![alt](url)
    let img_re = Regex::new(r#"^!\[([^\]]*)\]\(([^)]+)\)$"#).ok()?;
    if let Some(caps) = img_re.captures(s_clean) {
        let alt = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let url = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        let label = if alt.is_empty() { "Gambar" } else { alt };
        let clean_url = url.trim_start_matches('<').trim_end_matches('>').trim();
        return Some(format!(
            "  \x1b[1;38;5;117m📷 [Foto: {label}]\x1b[0m \x1b[4;38;5;39m{clean_url}\x1b[0m"
        ));
    }

    None
}

fn is_table_separator(line: &str) -> bool {
    let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
    !cells.is_empty()
        && cells
            .iter()
            .any(|c| !c.is_empty() && c.trim_matches(':').chars().all(|ch| ch == '-' || ch == '='))
        && cells.iter().all(|c| {
            if c.is_empty() {
                return true;
            }
            let trimmed = c.trim_matches(':');
            !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '-' || ch == '=')
        })
}

fn parse_table_cells(line: &str) -> Vec<String> {
    line.trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

fn render_terminal_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        return String::new();
    }

    // Compute column widths based on display length (ignoring ANSI)
    let mut col_widths = vec![3usize; num_cols];
    for row in rows {
        for (col_idx, cell) in row.iter().enumerate() {
            let len = cell.chars().count();
            if len > col_widths[col_idx] {
                col_widths[col_idx] = len.min(40); // Cap column width at 40
            }
        }
    }

    let mut out = Vec::new();

    // Top border: ┌───┬───┐
    let top_border = format!(
        "  \x1b[38;5;240m┌{}┐\x1b[0m",
        col_widths
            .iter()
            .map(|w| "─".repeat(*w + 2))
            .collect::<Vec<_>>()
            .join("┬")
    );
    out.push(top_border);

    // Header row
    if let Some(header) = rows.first() {
        let mut row_cells = Vec::new();
        for (col_idx, &width) in col_widths.iter().enumerate() {
            let cell = header.get(col_idx).map(|s| s.as_str()).unwrap_or("");
            let styled = format!("\x1b[1;38;5;81m{cell}\x1b[0m");
            let pad = width.saturating_sub(cell.chars().count());
            row_cells.push(format!(" {styled}{} ", " ".repeat(pad)));
        }
        out.push(format!(
            "  \x1b[38;5;240m│\x1b[0m{}\x1b[38;5;240m│\x1b[0m",
            row_cells.join("\x1b[38;5;240m│\x1b[0m")
        ));

        // Separator: ├───┼───┤
        let mid_border = format!(
            "  \x1b[38;5;240m├{}┤\x1b[0m",
            col_widths
                .iter()
                .map(|w| "─".repeat(*w + 2))
                .collect::<Vec<_>>()
                .join("┼")
        );
        out.push(mid_border);
    }

    // Data rows
    for row in &rows[1..] {
        let mut row_cells = Vec::new();
        for (col_idx, &width) in col_widths.iter().enumerate() {
            let cell = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
            let styled = render_terminal_inline(cell);
            let pad = width.saturating_sub(cell.chars().count());
            row_cells.push(format!(" {styled}{} ", " ".repeat(pad)));
        }
        out.push(format!(
            "  \x1b[38;5;240m│\x1b[0m{}\x1b[38;5;240m│\x1b[0m",
            row_cells.join("\x1b[38;5;240m│\x1b[0m")
        ));
    }

    // Bottom border: └───┴───┘
    let bot_border = format!(
        "  \x1b[38;5;240m└{}┘\x1b[0m",
        col_widths
            .iter()
            .map(|w| "─".repeat(*w + 2))
            .collect::<Vec<_>>()
            .join("┴")
    );
    out.push(bot_border);

    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_terminal_media_cards() {
        let input = "Foto: [photo: Kucing Lucu](https://example.com/cat.jpg)\nDokumen: [dokumen: Laporan](https://example.com/doc.pdf)";
        let output = render_terminal_markdown(input);
        assert!(output.contains("📷 [Foto: Kucing Lucu]"));
        assert!(output.contains("📄 [Dokumen: Laporan]"));
        assert!(output.contains("https://example.com/cat.jpg"));
    }

    #[test]
    fn renders_terminal_code_box() {
        let input = "```rust\nfn hello() {\n    println!(\"world\");\n}\n```";
        let output = render_terminal_markdown(input);
        assert!(output.contains("┌─"));
        assert!(output.contains("rust"));
        assert!(output.contains("println!"));
        assert!(output.contains("└─"));
    }

    #[test]
    fn renders_terminal_table_grid() {
        let input = "| Model | Speed |\n|---|---|\n| GPT-4o | Fast |\n| Claude | Deep |";
        let output = render_terminal_markdown(input);
        assert!(output.contains("┌"));
        assert!(output.contains("Model"));
        assert!(output.contains("Speed"));
        assert!(output.contains("GPT-4o"));
        assert!(output.contains("Claude"));
        assert!(output.contains("┘"));
    }

    #[test]
    fn renders_terminal_alerts_and_lists() {
        let input = "> [!NOTE] Catatan sistem\n- Fitur 1\n- Fitur 2";
        let output = render_terminal_markdown(input);
        assert!(output.contains("ℹ️  Catatan:"));
        assert!(output.contains("•"));
        assert!(output.contains("Fitur 1"));
    }

    #[test]
    fn strips_thinking_from_terminal_output() {
        let input = "<think>\nInternal reasoning here\n</think>\nHalo, ada yang bisa dibantu?";
        let output = render_terminal_markdown(input);
        assert!(!output.contains("Internal reasoning"));
        assert!(output.contains("Halo, ada yang bisa dibantu?"));
    }
}
