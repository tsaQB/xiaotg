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
    super::markdown::sanitize_leaked_llm_artifacts(input)
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

fn extract_tag_attr<'a>(s: &'a str, attr: &str) -> Option<&'a str> {
    let needle_double = format!("{attr}=\"");
    let needle_single = format!("{attr}='");
    if let Some(rest) = s.split(&needle_double).nth(1) {
        return rest.split('"').next().map(str::trim);
    }
    if let Some(rest) = s.split(&needle_single).nth(1) {
        return rest.split('\'').next().map(str::trim);
    }
    None
}

fn try_render_terminal_media(line: &str) -> Option<String> {
    let s = line.trim();
    let s_clean = s.trim_end_matches(['.', ',', ';', ':']);

    // 1. Telegram native document tag: <tg-document src="..." name="..."/>
    if s.starts_with("<tg-document") {
        let trimmed = s.trim_end_matches('>').trim_end_matches('/');
        let link = extract_tag_attr(trimmed, "src").unwrap_or("");
        let name = extract_tag_attr(trimmed, "name").unwrap_or("Dokumen");
        if !link.is_empty() {
            return Some(format!(
                "  \x1b[1;38;5;222m📄 [Dokumen: {name}]\x1b[0m \x1b[4;38;5;39m{link}\x1b[0m"
            ));
        }
    }

    // 2. Telegram native map tag: <tg-map lat="..." lon="..." title="..."/>
    if s.starts_with("<tg-map") {
        let trimmed = s.trim_end_matches('>').trim_end_matches('/');
        let lat = extract_tag_attr(trimmed, "lat").unwrap_or("");
        let lon = extract_tag_attr(trimmed, "lon").unwrap_or("");
        let title = extract_tag_attr(trimmed, "title").unwrap_or("Peta");
        if !lat.is_empty() && !lon.is_empty() {
            let map_url = format!("https://www.google.com/maps?q={lat},{lon}");
            return Some(format!(
                "  \x1b[1;38;5;203m📍 [Lokasi: {title}]\x1b[0m \x1b[4;38;5;39m{map_url}\x1b[0m"
            ));
        }
    }

    // 3. Telegram native photo/image tag: <tg-photo ...>, <img ...>
    if s.starts_with("<tg-photo") || s.starts_with("<img") {
        let trimmed = s.trim_end_matches('>').trim_end_matches('/');
        let link = extract_tag_attr(trimmed, "src").unwrap_or("");
        let cap = extract_tag_attr(trimmed, "caption")
            .or_else(|| extract_tag_attr(trimmed, "alt"))
            .unwrap_or("Foto");
        if !link.is_empty() {
            return Some(format!(
                "  \x1b[1;38;5;117m📷 [Foto: {cap}]\x1b[0m \x1b[4;38;5;39m{link}\x1b[0m"
            ));
        }
    }

    // 4. Telegram native video tag: <tg-video ...>
    if s.starts_with("<tg-video") {
        let trimmed = s.trim_end_matches('>').trim_end_matches('/');
        let link = extract_tag_attr(trimmed, "src").unwrap_or("");
        let cap = extract_tag_attr(trimmed, "caption").unwrap_or("Video");
        if !link.is_empty() {
            return Some(format!(
                "  \x1b[1;38;5;214m🎬 [Video: {cap}]\x1b[0m \x1b[4;38;5;39m{link}\x1b[0m"
            ));
        }
    }

    // 5. Telegram native audio tag: <tg-audio ...>
    if s.starts_with("<tg-audio") {
        let trimmed = s.trim_end_matches('>').trim_end_matches('/');
        let link = extract_tag_attr(trimmed, "src").unwrap_or("");
        let cap = extract_tag_attr(trimmed, "caption").unwrap_or("Audio");
        if !link.is_empty() {
            return Some(format!(
                "  \x1b[1;38;5;183m🎵 [Audio: {cap}]\x1b[0m \x1b[4;38;5;39m{link}\x1b[0m"
            ));
        }
    }

    // 6. Telegram native collage / slideshow container tags
    if s.starts_with("<tg-collage") || s.starts_with("<tg-slideshow") {
        let trimmed = s.trim_end_matches('>').trim_end_matches('/');
        let cap = extract_tag_attr(trimmed, "caption").unwrap_or("Galeri Foto");
        return Some(format!("  \x1b[1;38;5;117m🖼️ [Galeri: {cap}]\x1b[0m"));
    }

    // 3. Map: [map: lat, lon] or [map: label](coords)
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

fn strip_ansi_codes(s: &str) -> String {
    if let Ok(re) = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]") {
        re.replace_all(s, "").into_owned()
    } else {
        s.to_string()
    }
}

fn visible_width(s: &str) -> usize {
    strip_ansi_codes(s).chars().count()
}

fn render_terminal_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        return String::new();
    }

    let max_col_width = 45usize;

    // 1. Process and format cells, clipping raw text to max_col_width
    let mut processed_rows: Vec<Vec<(String, usize)>> = Vec::new();
    let mut col_widths = vec![3usize; num_cols];

    for (row_idx, row) in rows.iter().enumerate() {
        let mut proc_row = Vec::new();
        for (col_idx, width_ref) in col_widths.iter_mut().enumerate().take(num_cols) {
            let raw = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
            let clipped = if raw.chars().count() > max_col_width {
                let mut c: String = raw.chars().take(max_col_width - 1).collect();
                c.push('…');
                c
            } else {
                raw.to_string()
            };
            let styled = if row_idx == 0 {
                format!("\x1b[1;38;5;81m{clipped}\x1b[0m")
            } else {
                render_terminal_inline(&clipped)
            };
            let vis_w = visible_width(&styled);
            if vis_w > *width_ref {
                *width_ref = vis_w;
            }
            proc_row.push((styled, vis_w));
        }
        processed_rows.push(proc_row);
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
    if let Some(header) = processed_rows.first() {
        let mut row_cells = Vec::new();
        for (col_idx, &width) in col_widths.iter().enumerate() {
            let (styled, vis_w) = header.get(col_idx).cloned().unwrap_or_default();
            let pad = width.saturating_sub(vis_w);
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
    for row in &processed_rows[1..] {
        let mut row_cells = Vec::new();
        for (col_idx, &width) in col_widths.iter().enumerate() {
            let (styled, vis_w) = row.get(col_idx).cloned().unwrap_or_default();
            let pad = width.saturating_sub(vis_w);
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

    #[test]
    fn strips_reflection_and_unclosed_reasoning_from_terminal_output() {
        let input = "<reflection>\nInternal reflection here\n</reflection>\n<thought>\nUnclosed thought\nHalo dari model!";
        let output = render_terminal_markdown(input);
        assert!(!output.contains("Internal reflection"));
        assert!(!output.contains("Unclosed thought"));
    }

    #[test]
    fn renders_tg_document_and_tg_map_cards() {
        let input = "<tg-document src=\"https://example.com/spec.pdf\" name=\"Spec Dokumen\"/>\n<tg-map lat=\"-6.2\" lon=\"106.8\" title=\"Monas\"/>";
        let output = render_terminal_markdown(input);
        assert!(output.contains("📄 [Dokumen: Spec Dokumen]"));
        assert!(output.contains("https://example.com/spec.pdf"));
        assert!(output.contains("📍 [Lokasi: Monas]"));
        assert!(output.contains("maps?q=-6.2,106.8"));
    }

    #[test]
    fn renders_table_with_bold_cells_without_border_misalignment() {
        let input = "| Item | Keterangan |\n|---|---|\n| **Model Sangat Cepat** | Deskripsi singkat |\n| Normal | Keterangan lainnya |";
        let output = render_terminal_markdown(input);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines.len() >= 5);
        // Verify borders and rows match exact visible display width
        let top_w = visible_width(lines[0]);
        let header_w = visible_width(lines[1]);
        let sep_w = visible_width(lines[2]);
        let row1_w = visible_width(lines[3]);
        let row2_w = visible_width(lines[4]);
        let bot_w = visible_width(lines[5]);
        assert_eq!(top_w, header_w);
        assert_eq!(header_w, sep_w);
        assert_eq!(sep_w, row1_w);
        assert_eq!(row1_w, row2_w);
        assert_eq!(row2_w, bot_w);
    }
}
