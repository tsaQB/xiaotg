use regex::Regex;
use serde_json::{json, Value};

use crate::bot::models::{
    InputRichMessage, Location, RichBlock, RichBlockCaption, RichBlockListItem, RichBlockTableCell,
};

pub fn parse_inline(input_str: &str) -> Value {
    if input_str.is_empty() {
        return Value::String(String::new());
    }

    // Normalize well-formed inline HTML tags before cleaning leaked residual HTML
    let mut normalized = input_str.to_string();
    if normalized.contains("spoiler") || normalized.contains("<tg-spoiler") {
        if let Ok(re) = Regex::new(r"(?is)<tg-spoiler(?:\s+[^>]*)?>(.*?)</tg-spoiler>") {
            normalized = re.replace_all(&normalized, "||$1||").into_owned();
        }
        if let Ok(re) = Regex::new(r#"(?is)<span\s+class=["']?(?:tg-)?spoiler["']?>(.*?)</span>"#) {
            normalized = re.replace_all(&normalized, "||$1||").into_owned();
        }
    }
    if normalized.contains("<s") || normalized.contains("<strike") || normalized.contains("<del") {
        if let Ok(re) = Regex::new(r"(?is)<(?:s|strike|del)(?:\s+[^>]*)?>(.*?)</(?:s|strike|del)>")
        {
            normalized = re.replace_all(&normalized, "~~$1~~").into_owned();
        }
    }
    if normalized.contains("<u") || normalized.contains("<ins") {
        if let Ok(re) = Regex::new(r"(?is)<(?:u|ins)(?:\s+[^>]*)?>(.*?)</(?:u|ins)>") {
            normalized = re.replace_all(&normalized, "++${1}++").into_owned();
        }
    }
    if normalized.contains("<b") || normalized.contains("<strong") {
        if let Ok(re) = Regex::new(r"(?is)<(?:b|strong)(?:\s+[^>]*)?>(.*?)</(?:b|strong)>") {
            normalized = re.replace_all(&normalized, "**$1**").into_owned();
        }
    }
    if normalized.contains("<i") || normalized.contains("<em") {
        if let Ok(re) = Regex::new(r"(?is)<(?:i|em)(?:\s+[^>]*)?>(.*?)</(?:i|em)>") {
            normalized = re.replace_all(&normalized, "*$1*").into_owned();
        }
    }
    if normalized.contains("<code") {
        if let Ok(re) = Regex::new(r"(?is)<code(?:\s+[^>]*)?>(.*?)</code>") {
            normalized = re.replace_all(&normalized, "`$1`").into_owned();
        }
    }
    if normalized.contains("<a ") || normalized.contains("<a>") {
        if let Ok(re) = Regex::new(r#"(?is)<a\s+[^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>"#) {
            normalized = re.replace_all(&normalized, "[$2]($1)").into_owned();
        }
    }
    if normalized.contains("<br") {
        if let Ok(re) = Regex::new(r"(?i)<br\s*/?>") {
            normalized = re.replace_all(&normalized, "\n").into_owned();
        }
    }

    // Clean leaked HTML tags
    let cleaned =
        Regex::new(r"(?i)</?(?:b|strong|i|em|s|strike|del|u|ins|code|pre|blockquote|a|tg-spoiler|span|p|div|mark|kbd)(?:\s+[^>]*)?>")
            .map(|regex| regex.replace_all(&normalized, "").into_owned())
            .unwrap_or_else(|_| normalized);
    let unescaped = html_escape::decode_html_entities(&cleaned).to_string();

    let mut out: Vec<Value> = Vec::new();
    let mut rest = unescaped.as_str();

    while !rest.is_empty() {
        // 1. Bold **text**
        if rest.starts_with("**") {
            if let Some(end) = rest[2..].find("**") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "bold",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 2c. Underline <u>text</u> (++text++)
        if rest.starts_with("++") {
            if let Some(end) = rest[2..].find("++") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "underline",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 2. Bold __text__
        if rest.starts_with("__") {
            if let Some(end) = rest[2..].find("__") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "bold",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 2a. Spoiler ||text||
        if rest.starts_with("||") {
            if let Some(end) = rest[2..].find("||") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "spoiler",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 2b. Strikethrough ~~text~~
        if rest.starts_with("~~") {
            if let Some(end) = rest[2..].find("~~") {
                let inner = &rest[2..2 + end];
                out.push(json!({
                    "type": "strikethrough",
                    "text": parse_inline(inner)
                }));
                rest = &rest[2 + end + 2..];
                continue;
            }
        }

        // 3. Inline code `code`
        if rest.starts_with('`') {
            if let Some(end) = rest[1..].find('`') {
                let inner = &rest[1..1 + end];
                out.push(json!({
                    "type": "code",
                    "text": inner
                }));
                rest = &rest[1 + end + 1..];
                continue;
            }
        }

        // 4. Italic *text*
        if rest.starts_with('*') && !rest.starts_with("**") {
            if let Some(end) = rest[1..].find('*') {
                if end > 0 && !rest[1..].starts_with('*') {
                    let inner = &rest[1..1 + end];
                    out.push(json!({
                        "type": "italic",
                        "text": parse_inline(inner)
                    }));
                    rest = &rest[1 + end + 1..];
                    continue;
                }
            }
        }

        // 5. Italic _text_
        if rest.starts_with('_') && !rest.starts_with("__") {
            if let Some(end) = rest[1..].find('_') {
                if end > 0 {
                    let inner = &rest[1..1 + end];
                    out.push(json!({
                        "type": "italic",
                        "text": parse_inline(inner)
                    }));
                    rest = &rest[1 + end + 1..];
                    continue;
                }
            }
        }

        // 6a. Markdown image ![alt](url)
        if rest.starts_with("![") {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let raw_url = &rest[close + 2..close + 2 + end];
                    let url = raw_url
                        .trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .trim();
                    if url.starts_with("https://")
                        || url.starts_with("http://")
                        || url.starts_with("tg://")
                    {
                        let alt = &rest[2..close];
                        let alt_trimmed = alt.trim();
                        let display_label = if alt_trimmed.is_empty() {
                            "📷 Foto".to_string()
                        } else {
                            format!("📷 {alt_trimmed}")
                        };
                        out.push(json!({
                            "type": "url",
                            "text": parse_inline(&display_label),
                            "url": url
                        }));
                        rest = &rest[close + 3 + end..];
                        continue;
                    }
                }
            }
        }

        // 6b. Links [text](url)
        if rest.starts_with('[') {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let raw_url = &rest[close + 2..close + 2 + end];
                    let url = raw_url
                        .trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .trim();
                    if url.starts_with("https://")
                        || url.starts_with("http://")
                        || url.starts_with("tg://")
                    {
                        let inner = &rest[1..close];
                        let display_label = normalize_inline_media_label(inner);
                        out.push(json!({
                            "type": "url",
                            "text": parse_inline(&display_label),
                            "url": url
                        }));
                        rest = &rest[close + 3 + end..];
                        continue;
                    }
                }
            }
        }

        // 7. Inline math $...$
        if rest.starts_with('$') && !rest.starts_with("$$") {
            if let Some(end) = rest[1..].find('$') {
                if end > 0 && !rest[1..].starts_with('$') {
                    let inner = rest[1..1 + end].trim();
                    if !inner.is_empty() {
                        out.push(json!({
                            "type": "mathematical_expression",
                            "expression": inner
                        }));
                        rest = &rest[1 + end + 1..];
                        continue;
                    }
                }
            }
        }

        // 8. Inline math \( ... \)
        if rest.starts_with(r"\(") {
            if let Some(end) = rest[2..].find(r"\)") {
                let inner = rest[2..2 + end].trim();
                if !inner.is_empty() {
                    out.push(json!({
                        "type": "mathematical_expression",
                        "expression": inner
                    }));
                    rest = &rest[2 + end + 2..];
                    continue;
                }
            }
        }

        // 9. Plain text chunk until next token
        let mut next_pos = rest.len();
        for delim in &[
            "**", "__", "||", "~~", "++", "`", "*", "_", "![", "[", "$", r"\(",
        ] {
            if let Some(idx) = rest.find(delim) {
                if idx > 0 && idx < next_pos {
                    next_pos = idx;
                }
            }
        }

        if next_pos == rest.len() {
            out.push(Value::String(rest.to_string()));
            break;
        } else {
            out.push(Value::String(rest[..next_pos].to_string()));
            rest = &rest[next_pos..];
        }
    }

    // Merge adjacent strings
    let mut merged: Vec<Value> = Vec::new();
    for item in out {
        if let Value::String(s) = item {
            if let Some(Value::String(prev)) = merged.last_mut() {
                prev.push_str(&s);
            } else if !s.is_empty() {
                merged.push(Value::String(s));
            }
        } else {
            merged.push(item);
        }
    }

    if merged.is_empty() {
        Value::String(String::new())
    } else if merged.len() == 1 {
        merged.pop().unwrap_or_else(|| Value::String(String::new()))
    } else {
        Value::Array(merged)
    }
}

fn normalize_inline_media_label(label: &str) -> String {
    let t = label.trim();
    if let Some((tag, rest)) = t.split_once(':') {
        let tag_clean = tag.trim().to_lowercase();
        let rest_clean = rest.trim();
        let name = if rest_clean.is_empty() {
            tag.trim()
        } else {
            rest_clean
        };
        match tag_clean.as_str() {
            "photo" | "foto" | "image" | "img" | "gambar" | "picture" | "pic" => {
                return format!("📷 {name}");
            }
            "video" | "vid" => {
                return format!("🎬 {name}");
            }
            "audio" | "musik" | "music" | "lagu" | "song" => {
                return format!("🎵 {name}");
            }
            "voice" | "voicenote" | "voice_note" | "suara" | "rekaman" | "vn" => {
                return format!("🎙️ {name}");
            }
            "animation" | "animasi" | "gif" => {
                return format!("🎞️ {name}");
            }
            "document" | "dokumen" | "doc" | "file" | "berkas" => {
                return format!("📄 {name}");
            }
            "map" | "location" | "lokasi" | "peta" | "geo" => {
                return format!("📍 {name}");
            }
            _ => {}
        }
    }
    t.to_string()
}

fn is_border_line(line: &str) -> bool {
    let s = line.trim();
    if s.is_empty() {
        return true;
    }
    s.chars()
        .all(|c| "┌╔┏┬┰├┝┼╂└╚┗┴┸┤┥─━═+-=_ \t┐┘┒┙╗╝┚┖┓┛│|║┃".contains(c))
}

fn parse_coords_pair(text: &str) -> Option<(f64, f64, Option<i32>)> {
    let clean = text.trim().trim_matches(['(', ')', '[', ']']);
    let clean = clean.strip_prefix("geo:").unwrap_or(clean);
    let (coords_part, zoom_part) = if let Some((c, z)) = clean.split_once("?z=") {
        (c, z.parse::<i32>().ok())
    } else if let Some((c, z)) = clean.split_once("zoom=") {
        (c.trim_end_matches([',', ' ']), z.parse::<i32>().ok())
    } else {
        (clean, None)
    };
    let parts: Vec<&str> = coords_part.split(',').map(str::trim).collect();
    if parts.len() >= 2 {
        let lat = parts[0].parse::<f64>().ok()?;
        let lon = parts[1].parse::<f64>().ok()?;
        let zoom = zoom_part.or_else(|| {
            parts.get(2).and_then(|z| {
                z.strip_prefix("zoom=")
                    .unwrap_or(z)
                    .trim()
                    .parse::<i32>()
                    .ok()
            })
        });
        return Some((lat, lon, zoom));
    }
    None
}

fn try_parse_map_block(line: &str) -> Option<RichBlock> {
    let s = line.trim();
    let s_clean = s.trim_end_matches(['.', ',', ';', ':']);

    // Tag based: [map: ...], [location: ...], [lokasi: ...], [peta: ...], [geo: ...]
    let candidate = s_clean.strip_prefix('!').unwrap_or(s_clean);
    if candidate.starts_with('[') {
        if let Some(bracket_end) = candidate.find(']') {
            let tag_part = &candidate[1..bracket_end];
            if let Some((tag_name, label)) = tag_part.split_once(':') {
                let t = tag_name.trim().to_lowercase();
                if matches!(t.as_str(), "map" | "location" | "lokasi" | "peta" | "geo") {
                    let right = candidate[bracket_end + 1..].trim();
                    let right_clean = right.trim_end_matches(['.', ',', ';', ':', ' ']);
                    let coords_source = if let Some(inner) = right_clean
                        .strip_prefix('(')
                        .and_then(|r| r.strip_suffix(')'))
                    {
                        let link = inner
                            .trim()
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .trim();
                        if link.starts_with("http") {
                            link.split("?q=").nth(1).unwrap_or(link)
                        } else {
                            link
                        }
                    } else {
                        label.trim()
                    };

                    if let Some((lat, lon, zoom)) = parse_coords_pair(coords_source) {
                        return Some(RichBlock::Map {
                            location: Location {
                                latitude: lat,
                                longitude: lon,
                                horizontal_accuracy: None,
                            },
                            zoom,
                            width: None,
                            height: None,
                        });
                    }
                }
            }
        }
    }

    // ![map](geo:...) or ![location](geo:...)
    if let Some(geo) = s_clean
        .strip_prefix("![map](geo:")
        .or_else(|| s_clean.strip_prefix("![location](geo:"))
        .or_else(|| s_clean.strip_prefix("![lokasi](geo:"))
        .and_then(|r| r.strip_suffix(')'))
    {
        if let Some((lat, lon, zoom)) = parse_coords_pair(geo) {
            return Some(RichBlock::Map {
                location: Location {
                    latitude: lat,
                    longitude: lon,
                    horizontal_accuracy: None,
                },
                zoom,
                width: None,
                height: None,
            });
        }
    }

    // <tg-map lat="..." lon="..."/>
    if let Some(rest) = s.strip_prefix("<tg-map") {
        let trimmed = rest.trim().trim_end_matches('>').trim_end_matches('/');
        let lat_s = trimmed
            .split("lat=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("");
        let lon_s = trimmed
            .split("lon=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("");
        let zoom_s = trimmed
            .split("zoom=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("");
        let lat = lat_s.parse::<f64>().ok()?;
        let lon = lon_s.parse::<f64>().ok()?;
        let zoom = zoom_s.parse::<i32>().ok();
        return Some(RichBlock::Map {
            location: Location {
                latitude: lat,
                longitude: lon,
                horizontal_accuracy: None,
            },
            zoom,
            width: None,
            height: None,
        });
    }

    None
}

fn split_bracket_and_parenthesis(text: &str) -> Option<(&str, &str)> {
    let (left, right) = text.split_once(']')?;
    let right = right.trim();
    let right_cleaned = right.trim_end_matches(['.', ',', ';', ':', ' ']);
    let inner_right = right_cleaned.strip_prefix('(')?.strip_suffix(')')?.trim();
    let inner_right = inner_right
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    Some((left.trim(), inner_right))
}

fn classify_media_tag(tag: &str) -> Option<&'static str> {
    let t = tag.trim().to_lowercase();
    match t.as_str() {
        "photo" | "foto" | "image" | "img" | "gambar" | "picture" | "pic" => Some("photo"),
        "video" | "vid" => Some("video"),
        "audio" | "musik" | "music" | "lagu" | "song" => Some("audio"),
        "voice" | "voicenote" | "voice_note" | "suara" | "rekaman" | "vn" => Some("voice"),
        "animation" | "animasi" | "gif" => Some("animation"),
        "collage" | "kolase" | "gallery" | "galeri" | "album" => Some("collage"),
        "slideshow" | "slide" => Some("slideshow"),
        "document" | "dokumen" | "doc" | "file" | "berkas" => Some("document"),
        "map" | "location" | "lokasi" | "peta" | "geo" => Some("map"),
        _ => None,
    }
}

pub fn is_streaming_web_video(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("youtube.com/")
        || lower.contains("youtu.be/")
        || lower.contains("vimeo.com/")
        || lower.contains("dailymotion.com/")
        || lower.contains("twitch.tv/")
        || lower.contains("tiktok.com/")
}

fn extract_html_attribute<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let needle_double = format!("{attr}=\"");
    let needle_single = format!("{attr}='");
    if let Some(rest) = tag.split(&needle_double).nth(1) {
        return rest.split('"').next().map(str::trim);
    }
    if let Some(rest) = tag.split(&needle_single).nth(1) {
        return rest.split('\'').next().map(str::trim);
    }
    None
}

fn try_parse_doc_block(line: &str) -> Option<RichBlock> {
    let s = line.trim();
    let s_clean = s.trim_end_matches(['.', ',', ';', ':']);

    if s_clean.starts_with('[') || s_clean.starts_with("![") {
        let candidate = s_clean.strip_prefix('!').unwrap_or(s_clean);
        if let Some(bracket_end) = candidate.find(']') {
            let tag_part = &candidate[1..bracket_end];
            if let Some((tag_name, name)) = tag_part.split_once(':') {
                let t = tag_name.trim().to_lowercase();
                if matches!(
                    t.as_str(),
                    "document" | "dokumen" | "doc" | "file" | "berkas"
                ) {
                    let right = candidate[bracket_end + 1..].trim();
                    let right_clean = right.trim_end_matches(['.', ',', ';', ':', ' ']);
                    if let Some(inner) = right_clean
                        .strip_prefix('(')
                        .and_then(|r| r.strip_suffix(')'))
                    {
                        let link = inner
                            .trim()
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .trim();
                        let name = name.trim();
                        return Some(RichBlock::Document {
                            document: json!({"type": "document", "media": link}),
                            caption: (!name.is_empty())
                                .then(|| RichBlockCaption::new(parse_inline(name))),
                        });
                    }
                }
            }
        }
    }

    if let Some(rest) = s.strip_prefix("<tg-document") {
        let trimmed = rest.trim().trim_end_matches('>').trim_end_matches('/');
        let link = trimmed
            .split("src=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .or_else(|| {
                trimmed
                    .split("src='")
                    .nth(1)
                    .and_then(|s| s.split('\'').next())
            })
            .unwrap_or("");
        let name = trimmed
            .split("name=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .or_else(|| {
                trimmed
                    .split("name='")
                    .nth(1)
                    .and_then(|s| s.split('\'').next())
            })
            .unwrap_or("");
        if !link.is_empty() {
            return Some(RichBlock::Document {
                document: json!({"type": "document", "media": link}),
                caption: (!name.is_empty()).then(|| RichBlockCaption::new(parse_inline(name))),
            });
        }
    }
    None
}

fn try_parse_media_block(line: &str) -> Option<RichBlock> {
    let s = line.trim();
    let s_clean = s.trim_end_matches(['.', ',', ';', ':']);

    // 1. Bracketed media tags [photo: ...] or ![photo: ...]
    let candidate = s_clean.strip_prefix('!').unwrap_or(s_clean).trim_start();
    if candidate.starts_with('[') {
        if let Some(bracket_end) = candidate.find(']') {
            let tag_part = &candidate[1..bracket_end];
            if let Some((tag_name, label)) = tag_part.split_once(':') {
                if let Some(kind) = classify_media_tag(tag_name) {
                    let label = label.trim();
                    let right = candidate[bracket_end + 1..].trim();
                    let right_clean = right.trim_end_matches(['.', ',', ';', ':', ' ']);

                    // Map without parenthesis: [map: -6.2, 106.8]
                    if kind == "map" && right_clean.is_empty() {
                        if let Some((lat, lon, zoom)) = parse_coords_pair(label) {
                            return Some(RichBlock::Map {
                                location: Location {
                                    latitude: lat,
                                    longitude: lon,
                                    horizontal_accuracy: None,
                                },
                                zoom,
                                width: None,
                                height: None,
                            });
                        }
                    }

                    if let Some(inner_right) = right_clean
                        .strip_prefix('(')
                        .and_then(|r| r.strip_suffix(')'))
                    {
                        let link = inner_right
                            .trim()
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .trim();
                        let caption =
                            (!label.is_empty()).then(|| RichBlockCaption::new(parse_inline(label)));

                        match kind {
                            "photo" => {
                                return Some(RichBlock::Photo {
                                    photo: json!({"type": "photo", "media": link}),
                                    caption,
                                });
                            }
                            "video" => {
                                if is_streaming_web_video(link) {
                                    let cap_text = if label.is_empty() {
                                        "Tonton Video"
                                    } else {
                                        label
                                    };
                                    return Some(RichBlock::Paragraph {
                                        text: parse_inline(&format!("🎬 [{cap_text}]({link})")),
                                    });
                                }
                                return Some(RichBlock::Video {
                                    video: json!({"type": "video", "media": link}),
                                    caption,
                                });
                            }
                            "audio" => {
                                return Some(RichBlock::Audio {
                                    audio: json!({"type": "audio", "media": link}),
                                    caption,
                                });
                            }
                            "voice" => {
                                return Some(RichBlock::VoiceNote {
                                    voice_note: json!({"type": "voice_note", "media": link}),
                                    caption,
                                });
                            }
                            "animation" => {
                                return Some(RichBlock::Animation {
                                    animation: json!({"type": "animation", "media": link}),
                                    caption,
                                });
                            }
                            "document" => {
                                return Some(RichBlock::Document {
                                    document: json!({"type": "document", "media": link}),
                                    caption,
                                });
                            }
                            "collage" => {
                                let urls: Vec<&str> = link
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|u| !u.is_empty())
                                    .collect();
                                let blocks: Vec<Value> = urls
                                    .into_iter()
                                    .map(|u| json!({"type": "photo", "photo": {"type": "photo", "media": u}}))
                                    .collect();
                                return Some(RichBlock::Collage { blocks, caption });
                            }
                            "slideshow" => {
                                let urls: Vec<&str> = link
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|u| !u.is_empty())
                                    .collect();
                                let blocks: Vec<Value> = urls
                                    .into_iter()
                                    .map(|u| json!({"type": "photo", "photo": {"type": "photo", "media": u}}))
                                    .collect();
                                return Some(RichBlock::Slideshow { blocks, caption });
                            }
                            "map" => {
                                let coords_str = if link.starts_with("http") {
                                    link.split("?q=").nth(1).unwrap_or(link)
                                } else {
                                    link
                                };
                                if let Some((lat, lon, zoom)) = parse_coords_pair(coords_str) {
                                    return Some(RichBlock::Map {
                                        location: Location {
                                            latitude: lat,
                                            longitude: lon,
                                            horizontal_accuracy: None,
                                        },
                                        zoom,
                                        width: None,
                                        height: None,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // 2. Plain Markdown image syntax ![alt](url)
    if let Some(rest) = s_clean.strip_prefix("![") {
        let lower = s_clean.to_lowercase();
        if !lower.starts_with("![map")
            && !lower.starts_with("![location")
            && !lower.starts_with("![lokasi")
            && !lower.starts_with("![peta")
        {
            if let Some((alt, link)) = split_bracket_and_parenthesis(rest) {
                if link.starts_with("http://")
                    || link.starts_with("https://")
                    || link.starts_with("tg://")
                {
                    if is_streaming_web_video(link) {
                        let cap_text = if alt.is_empty() { "Tonton Video" } else { alt };
                        return Some(RichBlock::Paragraph {
                            text: parse_inline(&format!("🎬 [{cap_text}]({link})")),
                        });
                    }

                    let clean_url = link.split('?').next().unwrap_or(link);
                    let lower_link = clean_url.to_lowercase();
                    let caption =
                        (!alt.is_empty()).then(|| RichBlockCaption::new(parse_inline(alt)));
                    if lower_link.ends_with(".mp4")
                        || lower_link.ends_with(".webm")
                        || lower_link.ends_with(".mov")
                    {
                        return Some(RichBlock::Video {
                            video: json!({"type": "video", "media": link}),
                            caption,
                        });
                    } else if lower_link.ends_with(".mp3")
                        || lower_link.ends_with(".ogg")
                        || lower_link.ends_with(".wav")
                        || lower_link.ends_with(".m4a")
                    {
                        return Some(RichBlock::Audio {
                            audio: json!({"type": "audio", "media": link}),
                            caption,
                        });
                    } else if lower_link.ends_with(".gif") {
                        return Some(RichBlock::Animation {
                            animation: json!({"type": "animation", "media": link}),
                            caption,
                        });
                    } else {
                        return Some(RichBlock::Photo {
                            photo: json!({"type": "photo", "media": link}),
                            caption,
                        });
                    }
                }
            }
        }
    }

    // 3. Telegram native HTML media tags: <tg-photo ...>, <tg-video ...>, <tg-audio ...>, <img ...>
    if s.starts_with("<tg-photo")
        || s.starts_with("<tg-video")
        || s.starts_with("<tg-audio")
        || s.starts_with("<img")
    {
        if let Some(block) = try_parse_html_media_tag(s) {
            return Some(block);
        }
    }

    None
}

fn try_parse_html_media_tag(tag: &str) -> Option<RichBlock> {
    let s = tag.trim();
    let src = extract_html_attribute(s, "src").unwrap_or("");
    if src.is_empty() {
        return None;
    }
    let cap_attr = extract_html_attribute(s, "caption")
        .or_else(|| extract_html_attribute(s, "alt"))
        .or_else(|| extract_html_attribute(s, "title"));
    let inner_text = s
        .split('>')
        .nth(1)
        .and_then(|t| t.split("</").next())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let caption_text = cap_attr.or(inner_text).unwrap_or("");
    let caption =
        (!caption_text.is_empty()).then(|| RichBlockCaption::new(parse_inline(caption_text)));

    if s.starts_with("<tg-photo") || s.starts_with("<img") {
        return Some(RichBlock::Photo {
            photo: json!({"type": "photo", "media": src}),
            caption,
        });
    }

    if s.starts_with("<tg-video") {
        if is_streaming_web_video(src) {
            let label = if caption_text.is_empty() {
                "Tonton Video"
            } else {
                caption_text
            };
            return Some(RichBlock::Paragraph {
                text: parse_inline(&format!("🎬 [{label}]({src})")),
            });
        }
        return Some(RichBlock::Video {
            video: json!({"type": "video", "media": src}),
            caption,
        });
    }

    if s.starts_with("<tg-audio") {
        return Some(RichBlock::Audio {
            audio: json!({"type": "audio", "media": src}),
            caption,
        });
    }

    None
}

fn try_parse_container_media_block(
    lines: &[String],
    start_idx: usize,
) -> Option<(RichBlock, usize)> {
    let first_line = lines[start_idx].trim();
    let is_html = first_line.starts_with('<');
    let is_slideshow = first_line.to_lowercase().contains("slideshow");

    let close_tag = if is_html {
        if is_slideshow {
            "</tg-slideshow>"
        } else {
            "</tg-collage>"
        }
    } else if is_slideshow {
        "[/slideshow]"
    } else if first_line.to_lowercase().contains("kolase") {
        "[/kolase]"
    } else {
        "[/collage]"
    };

    let mut collected = Vec::new();
    let mut i = start_idx;
    let n = lines.len();

    if is_html && first_line.contains(close_tag) {
        collected.push(first_line.to_string());
        i += 1;
    } else {
        while i < n {
            let line = lines[i].trim();
            collected.push(line.to_string());
            i += 1;
            if line.contains(close_tag) {
                break;
            }
        }
    }

    let full_content = collected.join("\n");
    let caption_text = if is_html {
        extract_html_attribute(first_line, "caption")
            .or_else(|| extract_html_attribute(first_line, "title"))
            .or_else(|| extract_html_attribute(first_line, "alt"))
    } else {
        first_line
            .strip_prefix('[')
            .and_then(|s| s.split_once(']'))
            .map(|(tag_part, _)| tag_part)
            .and_then(|t| t.split_once(':'))
            .map(|(_, cap)| cap.trim())
            .filter(|c| !c.is_empty())
    };

    let caption = caption_text.map(|c| RichBlockCaption::new(parse_inline(c)));

    let mut sub_blocks = Vec::new();
    let src_re =
        Regex::new(r#"(?i)(?:src=["']([^"']+)["']|!?\[[^\]]*\]\(([^)]+)\)|https?://[^\s"'<>()]+)"#)
            .ok()?;
    for caps in src_re.captures_iter(&full_content) {
        let url = caps
            .get(1)
            .or_else(|| caps.get(2))
            .or_else(|| caps.get(0))
            .map(|m| m.as_str().trim())
            .unwrap_or("");
        let clean_url = url.trim_matches(['"', '\'', '<', '>']);
        if clean_url.starts_with("http://")
            || clean_url.starts_with("https://")
            || clean_url.starts_with("tg://")
        {
            let lower = clean_url.to_lowercase();
            if lower.ends_with(".mp4") || lower.ends_with(".webm") || lower.ends_with(".mov") {
                sub_blocks
                    .push(json!({"type": "video", "video": {"type": "video", "media": clean_url}}));
            } else if !lower.ends_with(".html")
                && !lower.ends_with(".htm")
                && !is_streaming_web_video(clean_url)
            {
                sub_blocks
                    .push(json!({"type": "photo", "photo": {"type": "photo", "media": clean_url}}));
            }
        }
    }

    if sub_blocks.len() >= 2 {
        let block = if is_slideshow {
            RichBlock::Slideshow {
                blocks: sub_blocks,
                caption,
            }
        } else {
            RichBlock::Collage {
                blocks: sub_blocks,
                caption,
            }
        };
        Some((block, i))
    } else if sub_blocks.len() == 1 {
        let first = sub_blocks.pop().unwrap();
        let block = if first["type"] == "video" {
            RichBlock::Video {
                video: first["video"].clone(),
                caption,
            }
        } else {
            RichBlock::Photo {
                photo: first["photo"].clone(),
                caption,
            }
        };
        Some((block, i))
    } else {
        None
    }
}

pub fn isolate_embedded_media_blocks(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut output = String::with_capacity(text.len() + 64);
    let mut in_code_block = false;

    let media_regex = Regex::new(
        r#"(?i)(!?\[(?:photo|foto|image|img|gambar|picture|pic|video|vid|audio|musik|music|lagu|song|voice|voicenote|voice_note|suara|rekaman|vn|animation|animasi|gif|collage|kolase|gallery|galeri|album|slideshow|slide|document|dokumen|doc|file|berkas|map|location|lokasi|peta|geo)\s*:[^\]]+\](?:\s*\([^\)]+\))?[.,;:]?|!\[[^\]]*\]\s*\([^\)]+\)[.,;:]?|<tg-(?:photo|video|audio|document|map|collage|slideshow)[^>]*>|</tg-(?:photo|video|audio|document|map|collage|slideshow)>|<img[^>]*>)"#
    ).ok();

    for line in text.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
            continue;
        }

        if in_code_block || trimmed.is_empty() || media_regex.is_none() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
            continue;
        }

        let regex = media_regex.as_ref().unwrap();
        if !regex.is_match(line) {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
            continue;
        }

        // Split line around matches, putting each media block on its own line
        let mut last_end = 0;
        for mat in regex.find_iter(line) {
            let start = mat.start();
            let end = mat.end();

            let before = line[last_end..start].trim();
            if !before.is_empty() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(before);
            }

            let matched_tag = line[start..end].trim();
            if !matched_tag.is_empty() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(matched_tag);
            }

            last_end = end;
        }

        let after = line[last_end..].trim();
        if !after.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(after);
        }
    }

    output
}

pub fn sanitize_leaked_llm_artifacts(text: &str) -> String {
    // 1. Strip closed thinking / reflection blocks
    let step1 = Regex::new(r"(?is)<think>.*?</think>")
        .map(|r| r.replace_all(text, "").into_owned())
        .unwrap_or_else(|_| text.to_string());
    let step2 = Regex::new(r"(?is)<thought>.*?</thought>")
        .map(|r| r.replace_all(&step1, "").into_owned())
        .unwrap_or(step1);
    let step3 = Regex::new(r"(?is)<reasoning>.*?</reasoning>")
        .map(|r| r.replace_all(&step2, "").into_owned())
        .unwrap_or(step2);
    let step4 = Regex::new(r"(?is)<reflection>.*?</reflection>")
        .map(|r| r.replace_all(&step3, "").into_owned())
        .unwrap_or(step3);
    let step5 = Regex::new(r"(?is)<tool_call>.*?</tool_call>")
        .map(|r| r.replace_all(&step4, "").into_owned())
        .unwrap_or(step4);
    let step6 = Regex::new(r"(?is)<function_calls?>.*?</function_calls?>")
        .map(|r| r.replace_all(&step5, "").into_owned())
        .unwrap_or(step5);

    // 2. Unclosed <think>, <thought>, <reasoning>
    let mut cleaned = step6;
    for open_tag in ["<think>", "<thought>", "<reasoning>", "<reflection>"] {
        if let Some(pos) = cleaned.to_lowercase().find(open_tag) {
            cleaned = cleaned[..pos].trim().to_string();
        }
    }

    // 3. Leaked tags or tokens
    if let Ok(re) =
        Regex::new(r"(?i)</?(?:think|thought|reasoning|reflection|tool_call|function_calls?)>")
    {
        cleaned = re.replace_all(&cleaned, "").into_owned();
    }
    cleaned = cleaned
        .replace("<|im_start|>", "")
        .replace("<|im_end|>", "")
        .replace("<|endoftext|>", "");

    cleaned
}

fn try_parse_table(
    lines: &[String],
    i: usize,
) -> (Option<Vec<Vec<RichBlockTableCell>>>, bool, usize) {
    let n = lines.len();
    let line = lines[i].trim();

    // 1. Standard Markdown Table (| Col 1 | Col 2 |\n| --- | --- |)
    if line.contains('|') && i + 1 < n {
        let next_line = lines[i + 1].trim();
        let sep_cells: Vec<&str> = next_line
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim())
            .collect();

        let is_sep = !sep_cells.is_empty()
            && sep_cells.iter().any(|c| {
                !c.is_empty() && c.trim_matches(':').chars().all(|ch| ch == '-' || ch == '=')
            })
            && sep_cells.iter().all(|c| {
                if c.is_empty() {
                    return true;
                }
                let trimmed = c.trim_matches(':');
                !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '-' || ch == '=')
            });

        if is_sep {
            let mut aligns: Vec<&str> = Vec::new();
            for c in &sep_cells {
                if c.starts_with(':') && c.ends_with(':') {
                    aligns.push("center");
                } else if c.ends_with(':') {
                    aligns.push("right");
                } else {
                    aligns.push("left");
                }
            }

            let unescaped_marker = "\u{E000}";
            let safe_line = line.replace(r"\|", unescaped_marker);
            let header_raw: Vec<String> = safe_line
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().replace(unescaped_marker, "|"))
                .collect();

            let header_row: Vec<RichBlockTableCell> = header_raw
                .into_iter()
                .enumerate()
                .map(|(idx, h)| {
                    let align = aligns.get(idx).copied().unwrap_or("left");
                    RichBlockTableCell::new(parse_inline(&h), true, Some(align))
                })
                .collect();

            let mut table_cells = vec![header_row];
            let mut idx_line = i + 2;

            while idx_line < n {
                let row_str = lines[idx_line].trim();
                if row_str.is_empty() || !row_str.contains('|') {
                    break;
                }
                let safe_row = row_str.replace(r"\|", unescaped_marker);
                let row_raw: Vec<String> = safe_row
                    .trim_matches('|')
                    .split('|')
                    .map(|c| c.trim().replace(unescaped_marker, "|"))
                    .collect();

                let data_row: Vec<RichBlockTableCell> = row_raw
                    .into_iter()
                    .enumerate()
                    .map(|(idx, c)| {
                        let align = aligns.get(idx).copied().unwrap_or("left");
                        RichBlockTableCell::new(parse_inline(&c), false, Some(align))
                    })
                    .collect();

                table_cells.push(data_row);
                idx_line += 1;
            }

            return (Some(table_cells), true, idx_line);
        }
    }

    // 2. Unicode Box or ASCII Grid Table (┌─┬─┐ or +---+---+)
    let is_unicode_box = line.chars().any(|c| "┌╔┏┬┰├┝┼╂".contains(c))
        || line
            .strip_prefix('│')
            .is_some_and(|rest| rest.contains('│'));
    let is_ascii_grid = line
        .strip_prefix('+')
        .is_some_and(|rest| rest.contains('+'))
        && (line.contains('-') || line.contains('='));

    if is_unicode_box || is_ascii_grid {
        let mut table_lines = Vec::new();
        let mut curr_i = i;

        while curr_i < n {
            let curr = lines[curr_i].trim();
            if curr.is_empty() {
                break;
            }
            if curr
                .chars()
                .any(|c| "┌╔┏┬┰├┝┼╂└╚┗┴┸┤┥│║┃|┐┘┒┙╗╝┚┖┓┛".contains(c))
                || curr
                    .strip_prefix('+')
                    .is_some_and(|rest| rest.contains('+'))
            {
                table_lines.push(curr);
                curr_i += 1;
            } else {
                break;
            }
        }

        if table_lines.len() >= 2 {
            let mut table_cells = Vec::new();
            let mut has_header = false;
            let mut first_row_done = false;

            for l in &table_lines {
                if is_border_line(l) {
                    if first_row_done {
                        has_header = true;
                    }
                    continue;
                }
                let mut row_content = *l;
                row_content = row_content.trim_start_matches(|c| "│|║┃".contains(c));
                row_content = row_content.trim_end_matches(|c| "│|║┃".contains(c));

                let cols: Vec<&str> = row_content
                    .split(|c| "│|║┃".contains(c))
                    .map(|col| col.trim())
                    .collect();

                if !cols.is_empty() && cols.iter().any(|c| !c.is_empty()) {
                    let row: Vec<RichBlockTableCell> = cols
                        .into_iter()
                        .map(|c| RichBlockTableCell::new(parse_inline(c), false, Some("left")))
                        .collect();
                    table_cells.push(row);
                    first_row_done = true;
                }
            }

            if let Some(first_row) = table_cells.first_mut() {
                if has_header {
                    for cell in first_row.iter_mut() {
                        cell.is_header = Some(true);
                    }
                }
            }

            if table_cells.len() >= 2 || (!table_cells.is_empty() && has_header) {
                return (Some(table_cells), has_header, curr_i);
            }
        }
    }

    // 3. Plain Underline Table: Header \n ---------------- \n Data
    let underline_re = Regex::new(r"^-{3,}$").ok();
    let space_split_re = Regex::new(r"\s{2,}|\t+").ok();
    let underline_match = underline_re
        .as_ref()
        .is_some_and(|regex| i + 1 < n && regex.is_match(lines[i + 1].trim()));
    if underline_match {
        let cols_hdr: Vec<&str> = space_split_re
            .as_ref()
            .map(|regex| regex.split(line).collect())
            .unwrap_or_else(|| line.split_whitespace().collect());
        let cols_hdr: Vec<&str> = cols_hdr
            .into_iter()
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .collect();

        if cols_hdr.len() >= 2 {
            let header_row: Vec<RichBlockTableCell> = cols_hdr
                .into_iter()
                .map(|c| RichBlockTableCell::new(parse_inline(c), true, Some("left")))
                .collect();
            let mut table_cells = vec![header_row];
            let mut curr_i = i + 2;

            while curr_i < n {
                let curr = lines[curr_i].trim();
                if curr.is_empty() {
                    break;
                }
                if underline_re
                    .as_ref()
                    .is_some_and(|regex| regex.is_match(curr))
                {
                    curr_i += 1;
                    continue;
                }
                let data_cols: Vec<&str> = space_split_re
                    .as_ref()
                    .map(|regex| regex.split(curr).collect())
                    .unwrap_or_else(|| curr.split_whitespace().collect());
                let data_cols: Vec<&str> = data_cols
                    .into_iter()
                    .map(|c| c.trim())
                    .filter(|c| !c.is_empty())
                    .collect();
                if !data_cols.is_empty() {
                    let row: Vec<RichBlockTableCell> = data_cols
                        .into_iter()
                        .map(|c| RichBlockTableCell::new(parse_inline(c), false, Some("left")))
                        .collect();
                    table_cells.push(row);
                }
                curr_i += 1;
            }

            if table_cells.len() >= 2 {
                return (Some(table_cells), true, curr_i);
            }
        }
    }

    (None, false, i)
}

fn extract_html_cite(text: &str) -> (String, Option<String>) {
    if let Some(start) = text.find("<cite>") {
        if let Some(end) = text[start + 6..].find("</cite>") {
            let credit = text[start + 6..start + 6 + end].trim().to_string();
            let mut body = text[..start].to_string();
            body.push_str(&text[start + 6 + end + 7..]);
            let credit_opt = if credit.is_empty() {
                None
            } else {
                Some(credit)
            };
            return (body.trim().to_string(), credit_opt);
        }
    }
    (text.to_string(), None)
}

fn extract_quote_credit(lines: &[String]) -> (String, Option<String>) {
    let combined = lines.join("\n");
    let (body, cite) = extract_html_cite(&combined);
    if cite.is_some() {
        return (body, cite);
    }
    if lines.len() > 1 {
        if let Some(last) = lines.last() {
            let trimmed = last.trim();
            for prefix in &["— ", "– ", "-- "] {
                if let Some(credit) = trimmed.strip_prefix(prefix) {
                    let credit = credit.trim();
                    if !credit.is_empty() {
                        let text = lines[..lines.len() - 1].join("\n");
                        return (text, Some(credit.to_string()));
                    }
                }
            }
        }
    }
    (combined, None)
}

/// Parse an accumulated streaming Markdown buffer without exposing syntax that
/// is still provisional. Completed syntax is rendered through the canonical
/// Rich Message parser; an incomplete tail is reduced to safe semantic text.
/// This lets a draft converge naturally without a second completion repaint.
pub fn parse_streaming_markdown_to_rich_blocks(text: &str) -> Vec<RichBlock> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let unstable_at = provisional_markdown_start(text).unwrap_or(text.len());
    let mut blocks = parse_markdown_to_rich_blocks(&text[..unstable_at]);
    if unstable_at < text.len() {
        let provisional = sanitize_provisional_markdown(&text[unstable_at..]);
        if !provisional.trim().is_empty() {
            blocks.push(RichBlock::Paragraph {
                text: Value::String(provisional),
            });
        }
    }
    blocks
}

fn provisional_markdown_start(text: &str) -> Option<usize> {
    let mut openings = Vec::new();

    // Fenced code dominates all inline syntax until the matching fence.
    let mut fence_open: Option<usize> = None;
    let mut offset = 0usize;
    for segment in text.split_inclusive('\n') {
        let trimmed = segment.trim_start();
        if trimmed.starts_with("```") {
            let marker = offset + (segment.len() - trimmed.len());
            if fence_open.is_some() {
                fence_open = None;
            } else {
                fence_open = Some(marker);
            }
        }
        offset += segment.len();
    }
    if let Some(index) = fence_open {
        openings.push(index);
    }

    // Inline code and emphasis are deliberately conservative: if a delimiter
    // is unmatched, the entire construct remains provisional rather than
    // flashing the raw opener to Telegram.
    for marker in ["**", "__", "`", "||", "~~", "++"] {
        let mut open: Option<usize> = None;
        let mut cursor = 0usize;
        while let Some(relative) = text[cursor..].find(marker) {
            let index = cursor + relative;
            if marker == "`" && text[index..].starts_with("```") {
                cursor = index + 3;
                continue;
            }
            open = if open.is_some() { None } else { Some(index) };
            cursor = index + marker.len();
        }
        if let Some(index) = open {
            openings.push(index);
        }
    }

    // A single underscore used as an emphasis opener is provisional. Limit
    // detection to word-boundary-ish positions so identifiers such as foo_bar
    // are not unnecessarily hidden.
    let mut underscore_open: Option<usize> = None;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (position, (index, ch)) in chars.iter().enumerate() {
        if *ch != '_' {
            continue;
        }
        let prev = position
            .checked_sub(1)
            .and_then(|p| chars.get(p))
            .map(|(_, c)| *c);
        let next = chars.get(position + 1).map(|(_, c)| *c);
        let delimiter_like = prev.is_none_or(|c| c.is_whitespace() || "([{>".contains(c))
            || next.is_none_or(|c| c.is_whitespace() || ".,!?;:)]}".contains(c));
        if delimiter_like {
            underscore_open = if underscore_open.is_some() {
                None
            } else {
                Some(*index)
            };
        }
    }
    if let Some(index) = underscore_open {
        openings.push(index);
    }

    // Line-oriented Markdown markers can themselves arrive split across chunks.
    // Keep an otherwise marker-only current line provisional until it becomes
    // a valid heading/divider/list item or ordinary text.
    let line_start = text.rfind('\n').map_or(0, |index| index + 1);
    let current_line = &text[line_start..];
    let leading_ws = current_line.len() - current_line.trim_start().len();
    let marker_start = line_start + leading_ws;
    let marker = current_line.trim();
    let incomplete_heading =
        !marker.is_empty() && marker.chars().all(|ch| ch == '#') && marker.chars().count() <= 6;
    let incomplete_divider = matches!(
        marker,
        "-" | "--" | "*" | "**" | "_" | "__" | "|" | "||" | "~" | "~~"
    );
    let incomplete_quote = matches!(marker, ">" | "**>" | ">>" | ">>>");
    let numeric_list_prefix = marker
        .strip_suffix('.')
        .or_else(|| marker.strip_suffix(')'));
    let incomplete_list = marker == "-"
        || marker == "*"
        || numeric_list_prefix.is_some_and(|prefix| {
            !prefix.is_empty() && prefix.chars().all(|ch| ch.is_ascii_digit())
        });
    if incomplete_heading || incomplete_divider || incomplete_list || incomplete_quote {
        openings.push(marker_start);
    }

    // Incomplete links: keep from `[` provisional until both `](` and `)` are
    // available. Nested link destinations are intentionally treated
    // conservatively rather than attempting a full Markdown grammar here.
    let mut search = 0usize;
    while let Some(rel) = text[search..].find('[') {
        let start = search + rel;
        let rest = &text[start + 1..];
        match rest.find("](") {
            Some(label_end) => {
                let destination = start + 1 + label_end + 2;
                if !text[destination..].contains(')') {
                    openings.push(start);
                    break;
                }
                search = destination + text[destination..].find(')').unwrap_or(0) + 1;
            }
            None => {
                openings.push(start);
                break;
            }
        }
    }

    openings.into_iter().min()
}

fn sanitize_provisional_markdown(tail: &str) -> String {
    let mut safe = tail
        .replace("```", "")
        .replace("**", "")
        .replace("__", "")
        .replace("||", "")
        .replace("~~", "")
        .replace("++", "");
    safe = safe.replace('`', "");

    // These are static programmer-owned patterns, but draft rendering must not
    // gain a production panic path if a future edit makes one invalid.
    if let Ok(re) = Regex::new(r"\[([^\]]*)\]\([^\)]*$") {
        safe = re.replace_all(&safe, "$1").into_owned();
    }
    if let Ok(re) = Regex::new(r"(?m)^\s*#{1,6}\s*") {
        safe = re.replace_all(&safe, "").into_owned();
    }
    if let Ok(re) = Regex::new(r"(?m)^\s*(?:[-*•]|\d+[.)])\s+") {
        safe = re.replace_all(&safe, "").into_owned();
    }
    if let Ok(re) = Regex::new(r"(?m)^\s*(?:-{1,}|\*{3,}|_{3,})\s*$") {
        safe = re.replace_all(&safe, "").into_owned();
    }
    if let Ok(re) = Regex::new(r"(?m)^\s*(?:\*\*>|>>>|>)\s*") {
        safe = re.replace_all(&safe, "").into_owned();
    }

    // Remove only obvious unmatched edge delimiters; do not blanket-delete
    // underscores from identifiers or ordinary punctuation.
    let trimmed = safe
        .trim_start_matches(['_', '*', '[', '|', '~', '>'])
        .trim_end_matches(['_', '*', '[', ']', '|', '~', '>']);
    trimmed.to_string()
}

pub fn parse_markdown_to_rich_blocks(text: &str) -> Vec<RichBlock> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let sanitized = sanitize_leaked_llm_artifacts(text);
    if sanitized.trim().is_empty() {
        return Vec::new();
    }

    let isolated = isolate_embedded_media_blocks(&sanitized);
    let lines: Vec<String> = isolated
        .replace("\r\n", "\n")
        .split('\n')
        .map(|s| s.to_string())
        .collect();
    let mut blocks: Vec<RichBlock> = Vec::new();

    let mut i = 0;
    let n = lines.len();

    let heading_re = Regex::new(r"^(#{1,6})\s*([^\s#].*)$").ok();
    let divider_re = Regex::new(r"^(\-{3,}|\*{3,}|_{3,}|─{3,}|—{2,})$").ok();
    let bullet_re = Regex::new(r"^[-*•]\s+").ok();
    let numbered_re = Regex::new(r"^\d+[\.)]\s+").ok();

    while i < n {
        let line = &lines[i];
        let stripped = line.trim();

        // 1. Skip blank lines
        if stripped.is_empty() {
            i += 1;
            continue;
        }

        // 2. Fenced Code Block (```lang ... ```)
        if let Some(after_fence) = stripped.strip_prefix("```") {
            let lang = after_fence.trim();
            let language = if lang.is_empty() {
                None
            } else {
                Some(lang.to_string())
            };
            let mut code_lines = Vec::new();
            i += 1;
            while i < n && !lines[i].trim().starts_with("```") {
                code_lines.push(lines[i].clone());
                i += 1;
            }
            if i < n && lines[i].trim().starts_with("```") {
                i += 1;
            }
            blocks.push(RichBlock::Preformatted {
                text: code_lines.join("\n"),
                language,
            });
            continue;
        }

        // 3. Math Block ($$...$$ or \[...\])
        if stripped.starts_with("$$") || stripped.starts_with(r"\[") {
            let is_bracket = stripped.starts_with(r"\[");
            let closing_token = if is_bracket { r"\]" } else { "$$" };
            let start_len = 2;
            let mut math_lines = Vec::new();

            if stripped.ends_with(closing_token) && stripped.len() > (start_len * 2) {
                math_lines.push(
                    stripped[start_len..stripped.len() - closing_token.len()]
                        .trim()
                        .to_string(),
                );
                i += 1;
            } else {
                if stripped.len() > start_len {
                    math_lines.push(stripped[start_len..].trim().to_string());
                }
                i += 1;
                while i < n && !lines[i].trim().ends_with(closing_token) {
                    math_lines.push(lines[i].clone());
                    i += 1;
                }
                if i < n && lines[i].trim().ends_with(closing_token) {
                    let end_line = lines[i].trim();
                    if end_line.len() > closing_token.len() {
                        math_lines.push(
                            end_line[..end_line.len() - closing_token.len()]
                                .trim()
                                .to_string(),
                        );
                    }
                    i += 1;
                }
            }

            let expr = math_lines.join("\n").trim().to_string();
            if !expr.is_empty() {
                blocks.push(RichBlock::MathematicalExpression { expression: expr });
            }
            continue;
        }

        // Standalone LaTeX math formula line (\text{...} or \frac{...})
        if (stripped.starts_with(r"\text{")
            || stripped.starts_with(r"\frac")
            || stripped.starts_with(r"\sqrt"))
            && (stripped.contains(r"\frac")
                || stripped.contains('=')
                || stripped.contains(r"\times"))
        {
            let mut math_lines = vec![stripped.to_string()];
            i += 1;
            while i < n {
                let curr_s = lines[i].trim();
                if curr_s.is_empty()
                    || ![
                        r"\frac", r"\text", "=", r"\times", r"\sqrt", "^", "_", "+", "-", "{", "}",
                    ]
                    .iter()
                    .any(|k| curr_s.contains(k))
                {
                    break;
                }
                math_lines.push(curr_s.to_string());
                i += 1;
            }
            let expr = math_lines.join("\n").trim().to_string();
            if !expr.is_empty() {
                blocks.push(RichBlock::MathematicalExpression { expression: expr });
            }
            continue;
        }

        // Map Block ([map: lat, lon] or <tg-map .../>)
        if stripped.starts_with("[map:")
            || stripped.starts_with("[location:")
            || stripped.starts_with("![map]")
            || stripped.starts_with("![location]")
            || stripped.starts_with("<tg-map")
        {
            if let Some(map_block) = try_parse_map_block(stripped) {
                blocks.push(map_block);
                i += 1;
                continue;
            }
        }

        // Document Block ([document: name](tg://...) or <tg-document .../>)
        if stripped.starts_with("[document:") || stripped.starts_with("<tg-document") {
            if let Some(doc_block) = try_parse_doc_block(stripped) {
                blocks.push(doc_block);
                i += 1;
                continue;
            }
        }

        // Multi-line or container Media Block (<tg-collage>...</tg-collage>, etc.)
        if stripped.starts_with("<tg-collage")
            || stripped.starts_with("<tg-slideshow")
            || (stripped.starts_with("[collage") && !stripped.contains('('))
            || (stripped.starts_with("[kolase") && !stripped.contains('('))
            || (stripped.starts_with("[slideshow") && !stripped.contains('('))
        {
            if let Some((container_block, next_i)) = try_parse_container_media_block(&lines, i) {
                blocks.push(container_block);
                i = next_i;
                continue;
            }
        }

        // Media Block (Photo, Video, Audio, VoiceNote, Animation, Collage, Slideshow)
        if let Some(media_block) = try_parse_media_block(stripped) {
            blocks.push(media_block);
            i += 1;
            continue;
        }

        // 4. Horizontal Divider (---, ***, ___, ───)
        if divider_re
            .as_ref()
            .is_some_and(|regex| regex.is_match(stripped))
        {
            blocks.push(RichBlock::Divider {});
            i += 1;
            continue;
        }

        // 5. Section Heading (# Heading, ## Subheading, etc.)
        if let Some(caps) = heading_re
            .as_ref()
            .and_then(|regex| regex.captures(stripped))
        {
            let level = caps.get(1).map(|m| m.as_str().len()).unwrap_or(1);
            let heading_text = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            blocks.push(RichBlock::SectionHeading {
                text: parse_inline(heading_text),
                level: level.min(6),
            });
            i += 1;
            continue;
        }

        // 6a. Pullquote (>>> quote)
        if stripped.starts_with(">>>") {
            let mut quote_lines = Vec::new();
            while i < n && lines[i].trim().starts_with(">>>") {
                let q = lines[i].trim();
                let stripped_q = q.strip_prefix(">>>").unwrap_or(q).trim_start();
                quote_lines.push(stripped_q.to_string());
                i += 1;
            }
            let (quote_text, credit) = extract_quote_credit(&quote_lines);
            blocks.push(RichBlock::PullQuotation {
                text: parse_inline(&quote_text),
                credit: credit.map(|c| parse_inline(&c)),
            });
            continue;
        }

        // 6b. Expandable Blockquote (**> quote or <blockquote expandable>)
        if stripped.starts_with("**>") {
            let mut quote_lines = Vec::new();
            while i < n {
                let curr_stripped = lines[i].trim();
                if curr_stripped.starts_with("**>") {
                    quote_lines.push(
                        curr_stripped
                            .strip_prefix("**>")
                            .unwrap_or(curr_stripped)
                            .trim_start()
                            .to_string(),
                    );
                    i += 1;
                } else if curr_stripped.starts_with('>') && !curr_stripped.starts_with(">>>") {
                    quote_lines.push(
                        curr_stripped
                            .strip_prefix('>')
                            .unwrap_or(curr_stripped)
                            .trim_start()
                            .to_string(),
                    );
                    i += 1;
                } else {
                    break;
                }
            }
            let (quote_text, credit) = extract_quote_credit(&quote_lines);
            blocks.push(RichBlock::ExpandableBlockQuotation {
                text: parse_inline(&quote_text),
                credit: credit.map(|c| parse_inline(&c)),
            });
            continue;
        }

        if stripped.starts_with("<blockquote") && stripped.contains("expandable") {
            let mut quote_lines = Vec::new();
            let mut first_line = stripped.to_string();
            if let Some(pos) = first_line.find('>') {
                first_line = first_line[pos + 1..].to_string();
            }
            if let Some(end) = first_line.find("</blockquote>") {
                let inner = first_line[..end].trim();
                let (quote_text, credit) = extract_html_cite(inner);
                blocks.push(RichBlock::ExpandableBlockQuotation {
                    text: parse_inline(&quote_text),
                    credit: credit.map(|c| parse_inline(&c)),
                });
                i += 1;
                continue;
            }
            if !first_line.trim().is_empty() {
                quote_lines.push(first_line.trim().to_string());
            }
            i += 1;
            while i < n {
                let curr = lines[i].trim();
                if let Some(end) = curr.find("</blockquote>") {
                    let before = curr[..end].trim();
                    if !before.is_empty() {
                        quote_lines.push(before.to_string());
                    }
                    i += 1;
                    break;
                }
                quote_lines.push(curr.to_string());
                i += 1;
            }
            let (quote_text, credit) = extract_quote_credit(&quote_lines);
            blocks.push(RichBlock::ExpandableBlockQuotation {
                text: parse_inline(&quote_text),
                credit: credit.map(|c| parse_inline(&c)),
            });
            continue;
        }

        // 6c. HTML Blockquote (<blockquote> ... </blockquote>)
        if stripped.starts_with("<blockquote") && !stripped.contains("expandable") {
            let mut quote_lines = Vec::new();
            let mut first_line = stripped.to_string();
            if let Some(pos) = first_line.find('>') {
                first_line = first_line[pos + 1..].to_string();
            }
            if let Some(end) = first_line.find("</blockquote>") {
                let inner = first_line[..end].trim();
                blocks.push(RichBlock::BlockQuotation {
                    blocks: vec![json!({
                        "type": "paragraph",
                        "text": parse_inline(inner)
                    })],
                });
                i += 1;
                continue;
            }
            if !first_line.trim().is_empty() {
                quote_lines.push(first_line.trim().to_string());
            }
            i += 1;
            while i < n {
                let curr = lines[i].trim();
                if let Some(end) = curr.find("</blockquote>") {
                    let before = curr[..end].trim();
                    if !before.is_empty() {
                        quote_lines.push(before.to_string());
                    }
                    i += 1;
                    break;
                }
                quote_lines.push(curr.to_string());
                i += 1;
            }
            blocks.push(RichBlock::BlockQuotation {
                blocks: vec![json!({
                    "type": "paragraph",
                    "text": parse_inline(&quote_lines.join("\n"))
                })],
            });
            continue;
        }

        // 6. Blockquote (> quote)
        if stripped.starts_with('>') {
            let mut quote_lines = Vec::new();
            while i < n && lines[i].trim().starts_with('>') && !lines[i].trim().starts_with(">>>") {
                let q = lines[i].trim();
                let stripped_q = q.strip_prefix('>').unwrap_or(q).trim_start();
                quote_lines.push(stripped_q.to_string());
                i += 1;
            }
            if let Some(first) = quote_lines.first_mut() {
                let alerts = [
                    ("[!NOTE]", "ℹ️ **Catatan:**"),
                    ("[!note]", "ℹ️ **Catatan:**"),
                    ("[!TIP]", "💡 **Tips:**"),
                    ("[!tip]", "💡 **Tips:**"),
                    ("[!IMPORTANT]", "📌 **Penting:**"),
                    ("[!important]", "📌 **Penting:**"),
                    ("[!WARNING]", "⚠️ **Peringatan:**"),
                    ("[!warning]", "⚠️ **Peringatan:**"),
                    ("[!CAUTION]", "🚨 **Perhatian:**"),
                    ("[!caution]", "🚨 **Perhatian:**"),
                ];
                for (marker, replacement) in alerts {
                    if first.starts_with(marker) {
                        let rest = first[marker.len()..].trim();
                        if rest.is_empty() {
                            *first = replacement.to_string();
                        } else {
                            *first = format!("{replacement} {rest}");
                        }
                        break;
                    }
                }
            }
            blocks.push(RichBlock::BlockQuotation {
                blocks: vec![json!({
                    "type": "paragraph",
                    "text": parse_inline(&quote_lines.join("\n"))
                })],
            });
            continue;
        }

        // 7. Table (Markdown, Unicode, ASCII, Underline)
        if let Some(inner) = stripped
            .strip_prefix("[table:")
            .or_else(|| stripped.strip_prefix("[caption:"))
            .and_then(|r| r.strip_suffix(']'))
        {
            let cap = inner.trim();
            if !cap.is_empty() && i + 1 < n {
                let (t_cells, has_hdr, next_i) = try_parse_table(&lines, i + 1);
                if let Some(cells) = t_cells {
                    blocks.push(RichBlock::Table {
                        cells,
                        has_header: has_hdr,
                        is_bordered: true,
                        is_striped: true,
                        is_compact: true,
                        caption: Some(cap.to_string()),
                    });
                    i = next_i;
                    continue;
                }
            }
        }

        let (t_cells, has_hdr, next_i) = try_parse_table(&lines, i);
        if let Some(cells) = t_cells {
            let mut caption: Option<String> = None;
            let mut final_next_i = next_i;
            if final_next_i < n {
                let next_line = lines[final_next_i].trim();
                if let Some(inner) = next_line
                    .strip_prefix("[table:")
                    .or_else(|| next_line.strip_prefix("[caption:"))
                    .and_then(|r| r.strip_suffix(']'))
                {
                    let cap = inner.trim();
                    if !cap.is_empty() {
                        caption = Some(cap.to_string());
                        final_next_i += 1;
                    }
                }
            }
            blocks.push(RichBlock::Table {
                cells,
                has_header: has_hdr,
                is_bordered: true,
                is_striped: true,
                is_compact: true,
                caption,
            });
            i = final_next_i;
            continue;
        }

        // 8. List Items (- item, * item, 1. item)
        let is_bullet = bullet_re
            .as_ref()
            .is_some_and(|regex| regex.is_match(stripped));
        let is_numbered = numbered_re
            .as_ref()
            .is_some_and(|regex| regex.is_match(stripped));

        if is_bullet || is_numbered {
            let mut list_items = Vec::new();
            let is_ordered = is_numbered;

            while i < n {
                let curr = lines[i].trim();
                if curr.is_empty() {
                    break;
                }
                if is_ordered
                    && numbered_re
                        .as_ref()
                        .is_some_and(|regex| regex.is_match(curr))
                {
                    let item_text = numbered_re
                        .as_ref()
                        .map(|regex| regex.replace(curr, "").into_owned())
                        .unwrap_or_else(|| curr.to_string())
                        .trim()
                        .to_string();
                    let value = curr
                        .split_once(['.', ')'])
                        .and_then(|(prefix, _)| prefix.parse::<i64>().ok());
                    list_items.push(RichBlockListItem::ordered(
                        vec![json!({
                            "type": "paragraph",
                            "text": parse_inline(&item_text)
                        })],
                        value,
                    ));
                    i += 1;
                } else if !is_ordered
                    && bullet_re.as_ref().is_some_and(|regex| regex.is_match(curr))
                {
                    let item_text = bullet_re
                        .as_ref()
                        .map(|regex| regex.replace(curr, "").into_owned())
                        .unwrap_or_else(|| curr.to_string())
                        .trim()
                        .to_string();
                    list_items.push(RichBlockListItem::bullet(vec![json!({
                        "type": "paragraph",
                        "text": parse_inline(&item_text)
                    })]));
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(RichBlock::List { items: list_items });
            continue;
        }

        // 9. Regular Paragraph
        let mut para_lines = Vec::new();
        while i < n {
            let curr = &lines[i];
            let s_curr = curr.trim();
            if s_curr.is_empty()
                || s_curr.starts_with("```")
                || s_curr.starts_with("$$")
                || heading_re
                    .as_ref()
                    .is_some_and(|regex| regex.is_match(s_curr))
                || s_curr.starts_with("**>")
                || s_curr.starts_with("<blockquote")
                || s_curr.starts_with('>')
                || s_curr.starts_with(">>>")
                || s_curr.starts_with("[table:")
                || s_curr.starts_with("[caption:")
                || s_curr.starts_with("<tg-map")
                || s_curr.starts_with("<tg-document")
                || try_parse_media_block(s_curr).is_some()
                || try_parse_doc_block(s_curr).is_some()
                || try_parse_map_block(s_curr).is_some()
                || bullet_re
                    .as_ref()
                    .is_some_and(|regex| regex.is_match(s_curr))
                || numbered_re
                    .as_ref()
                    .is_some_and(|regex| regex.is_match(s_curr))
                || divider_re
                    .as_ref()
                    .is_some_and(|regex| regex.is_match(s_curr))
                || try_parse_table(&lines, i).0.is_some()
            {
                break;
            }
            para_lines.push(curr.clone());
            i += 1;
        }

        if !para_lines.is_empty() {
            blocks.push(RichBlock::Paragraph {
                text: parse_inline(&para_lines.join("\n")),
            });
        }
    }

    blocks
}

pub fn build_full_rich_message(answer_text: &str, footer_text: Option<&str>) -> InputRichMessage {
    let mut blocks = parse_markdown_to_rich_blocks(answer_text);
    if blocks.is_empty() {
        blocks.push(RichBlock::Paragraph {
            text: parse_inline(answer_text.trim()),
        });
    }
    if let Some(footer) = footer_text.map(str::trim).filter(|m| !m.is_empty()) {
        blocks.push(RichBlock::Footer {
            text: parse_inline(footer),
        });
    }
    InputRichMessage::new(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_box_table_parses_without_byte_boundary_slicing() {
        let input =
            "┌──────┬──────┐\n│ Nama │ Ikon │\n├──────┼──────┤\n│ 世界 │ 😊   │\n└──────┴──────┘";
        let blocks = parse_markdown_to_rich_blocks(input);
        assert!(blocks
            .iter()
            .any(|block| matches!(block, RichBlock::Table { .. })));
    }

    #[test]
    fn ordered_list_preserves_native_ordering_metadata() {
        let blocks = parse_markdown_to_rich_blocks("5. lima\n6. enam");
        let RichBlock::List { items } = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(items[0].kind.as_deref(), Some("1"));
        assert_eq!(items[0].value, Some(5));
        assert_eq!(items[1].value, Some(6));
    }

    #[test]
    fn emoji_and_multibyte_inline_text_survive_parser() {
        let value = parse_inline("Halo █ 😊 世界 **tebal**");
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(serialized.contains("世界"));
        assert!(serialized.contains("😊"));
    }

    #[test]
    fn streaming_markdown_never_exposes_provisional_serialization_markers() {
        let cases = [
            "Ini **gaya gravitasi** selesai",
            "Ini _italic_ selesai",
            "Gunakan `kode` sekarang",
            "```rust\nfn main() {}\n```",
            "### Heading tumbuh",
            "---",
            "[OpenAI](https://example.com/path)",
            "1. pertama\n2. kedua",
            "- satu\n- dua",
            "Emoji 😊 世界 **tebal**",
        ];

        for source in cases {
            let mut boundaries: Vec<usize> =
                source.char_indices().map(|(index, _)| index).collect();
            boundaries.push(source.len());
            boundaries.sort_unstable();
            boundaries.dedup();
            for end in boundaries.into_iter().filter(|end| *end > 0) {
                let prefix = &source[..end];
                let blocks = parse_streaming_markdown_to_rich_blocks(prefix);
                let wire = serde_json::to_string(&blocks).unwrap();
                assert!(
                    !wire.contains("**"),
                    "bold marker leaked for {prefix:?}: {wire}"
                );
                assert!(
                    !wire.contains("__"),
                    "emphasis marker leaked for {prefix:?}: {wire}"
                );
                assert!(
                    !wire.contains("```"),
                    "fence marker leaked for {prefix:?}: {wire}"
                );
                assert!(
                    !wire.contains("]("),
                    "link serialization leaked for {prefix:?}: {wire}"
                );
                if prefix.trim().chars().all(|ch| ch == '#') {
                    assert!(
                        !wire.contains('#'),
                        "heading marker leaked for {prefix:?}: {wire}"
                    );
                }
                if matches!(prefix.trim(), "-" | "--") {
                    assert!(
                        !wire.contains(prefix.trim()),
                        "divider marker leaked for {prefix:?}: {wire}"
                    );
                }
            }
        }
    }

    #[test]
    fn collage_slideshow_audio_voice_parse_correctly() {
        let text = "[audio: Judul Musik](https://example.com/song.mp3)

[voice: Rekaman Suara](tg://audio?id=rec1)

[collage: Galeri](url1, url2)

[slideshow: Slide](url3, url4)";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert!(blocks.iter().any(|b| matches!(b, RichBlock::Audio { .. })));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, RichBlock::VoiceNote { .. })));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, RichBlock::Collage { .. })));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, RichBlock::Slideshow { .. })));
    }

    #[test]
    fn media_blocks_tolerate_whitespace_between_bracket_and_parenthesis() {
        let text = "[audio: Suara Contoh] (https://upload.wikimedia.org/wikipedia/commons/c/c8/Example.ogg)\n\n[photo: Foto Indah]  (https://example.com/pic.jpg)\n\n[collage: Galeri] (url1, url2)";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], RichBlock::Audio { .. }));
        assert!(matches!(blocks[1], RichBlock::Photo { .. }));
        assert!(matches!(blocks[2], RichBlock::Collage { .. }));
    }

    #[test]
    fn map_and_document_blocks_parse_correctly() {
        let text = "[map: -6.175392, 106.827153, zoom=15]

[document: Laporan.pdf](tg://document?id=laporan_1)";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert!(blocks.iter().any(|b| matches!(b, RichBlock::Map { .. })));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, RichBlock::Document { .. })));
    }

    #[test]
    fn pullquote_and_footer_parse_correctly() {
        let text = ">>> Ini adalah kutipan penting

Paragraf normal";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert!(blocks
            .iter()
            .any(|b| matches!(b, RichBlock::PullQuotation { .. })));

        let full = build_full_rich_message("Jawaban AI", Some("`⚡ 3.0s`"));
        let footer = full
            .blocks
            .iter()
            .find_map(|b| match b {
                RichBlock::Footer { text } => Some(text),
                _ => None,
            })
            .expect("footer block should exist");
        let serialized = serde_json::to_string(footer).unwrap();
        assert!(serialized.contains("3.0s"));
        assert!(serialized.contains("⚡"));
        assert!(serialized.contains("code"));
    }

    #[test]
    fn markdown_image_and_media_is_media_check() {
        let text = "Penjelasan aurora:\n\n![Cahaya Aurora](https://picsum.photos/1000/600)\n\n[photo: Tromso](https://picsum.photos/800/600)";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 3);
        assert!(!blocks[0].is_media());
        assert!(blocks[1].is_media());
        assert!(blocks[2].is_media());
        assert!(matches!(blocks[1], RichBlock::Photo { .. }));
        assert!(matches!(blocks[2], RichBlock::Photo { .. }));
    }

    #[test]
    fn completed_streaming_markdown_converges_to_canonical_parser() {
        let source = "## Judul\n\n**tebal** dan _miring_\n\n---\n\n1. satu\n2. dua";
        let streaming = parse_streaming_markdown_to_rich_blocks(source);
        let canonical = parse_markdown_to_rich_blocks(source);
        assert_eq!(
            serde_json::to_value(streaming).unwrap(),
            serde_json::to_value(canonical).unwrap()
        );
    }

    #[test]
    fn spoiler_and_strikethrough_parse_correctly() {
        let markdown = "Info: ||rahasia besar|| dan ~~harga lama~~";
        let parsed = parse_inline(markdown);
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert!(serialized.contains(r#""type":"spoiler""#));
        assert!(serialized.contains("rahasia besar"));
        assert!(serialized.contains(r#""type":"strikethrough""#));
        assert!(serialized.contains("harga lama"));

        let html = "Tag: <tg-spoiler>kunci rahasia</tg-spoiler> dan <s>coret html</s>";
        let parsed_html = parse_inline(html);
        let serialized_html = serde_json::to_string(&parsed_html).unwrap();
        assert!(serialized_html.contains(r#""type":"spoiler""#));
        assert!(serialized_html.contains("kunci rahasia"));
        assert!(serialized_html.contains(r#""type":"strikethrough""#));
        assert!(serialized_html.contains("coret html"));
    }

    #[test]
    fn expandable_blockquote_parses_correctly() {
        let markdown = "**> Baris penalaran pertama\n**> Baris penalaran kedua\n**> — As-tsaqib";
        let blocks = parse_markdown_to_rich_blocks(markdown);
        assert_eq!(blocks.len(), 1);
        let Some(RichBlock::ExpandableBlockQuotation { text, credit }) = blocks.first() else {
            panic!("expected expandable blockquote");
        };
        let text_str = serde_json::to_string(text).unwrap();
        assert!(text_str.contains("Baris penalaran pertama"));
        assert!(text_str.contains("Baris penalaran kedua"));
        assert!(credit.is_some());
        let credit_str = serde_json::to_string(&credit).unwrap();
        assert!(credit_str.contains("As-tsaqib"));

        let html =
            "<blockquote expandable>Catatan terlipat penting<cite>Dokumentasi</cite></blockquote>";
        let blocks_html = parse_markdown_to_rich_blocks(html);
        assert_eq!(blocks_html.len(), 1);
        let Some(RichBlock::ExpandableBlockQuotation {
            text: h_text,
            credit: h_credit,
        }) = blocks_html.first()
        else {
            panic!("expected HTML expandable blockquote");
        };
        assert!(serde_json::to_string(h_text)
            .unwrap()
            .contains("Catatan terlipat penting"));
        assert!(serde_json::to_string(h_credit)
            .unwrap()
            .contains("Dokumentasi"));
    }

    #[test]
    fn table_compact_and_caption_parse_correctly() {
        let text = "[table: Perbandingan Spesifikasi]\n| Model | Konteks |\n| :--- | :---: |\n| GPT-4o | 128k |\n| Claude | 200k |";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 1);
        let Some(RichBlock::Table {
            cells,
            is_compact,
            caption,
            has_header,
            ..
        }) = blocks.first()
        else {
            panic!("expected rich table block");
        };
        assert!(*is_compact);
        assert!(*has_header);
        assert_eq!(caption.as_deref(), Some("Perbandingan Spesifikasi"));
        assert_eq!(cells.len(), 3);
    }

    #[test]
    fn tg_document_links_and_underline_parse_correctly() {
        let text = "Tautan: [Buka File](tg://document?id=doc_abc123) dan <u>garis bawah</u> serta ++format ins++";
        let parsed = parse_inline(text);
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert!(serialized.contains(r#""type":"url""#));
        assert!(serialized.contains("tg://document?id=doc_abc123"));
        assert!(serialized.contains("Buka File"));
        assert!(serialized.contains(r#""type":"underline""#));
        assert!(serialized.contains("garis bawah"));
        assert!(serialized.contains("format ins"));
    }

    #[test]
    fn indonesian_and_case_insensitive_media_tags_parse_correctly() {
        let text = "[foto: Kucing Anggora](https://example.com/cat.jpg)\n\n[Foto : Kucing Lucu]  ( https://example.com/cat2.jpg ).\n\n[gambar: Pantai](https://example.com/beach.jpg)\n\n[dokumen: Laporan Keuangan](https://example.com/laporan.pdf)\n\n[file: Data Excel](https://example.com/data.xlsx)\n\n[musik: Suara Hujan](https://example.com/rain.mp3)\n\n[rekaman: Catatan Suara](https://example.com/voice.ogg)\n\n[lokasi: Monas, Jakarta](-6.175392, 106.827153)\n\n[kolase: Liburan](url1, url2)";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 9);
        assert!(matches!(blocks[0], RichBlock::Photo { .. }));
        assert!(matches!(blocks[1], RichBlock::Photo { .. }));
        assert!(matches!(blocks[2], RichBlock::Photo { .. }));
        assert!(matches!(blocks[3], RichBlock::Document { .. }));
        assert!(matches!(blocks[4], RichBlock::Document { .. }));
        assert!(matches!(blocks[5], RichBlock::Audio { .. }));
        assert!(matches!(blocks[6], RichBlock::VoiceNote { .. }));
        assert!(matches!(blocks[7], RichBlock::Map { .. }));
        assert!(matches!(blocks[8], RichBlock::Collage { .. }));
    }

    #[test]
    fn embedded_media_blocks_in_paragraphs_are_isolated_and_parsed() {
        let text = "Ini fotonya: [photo: Kucing](https://example.com/cat.jpg) Kucing ini lucu.";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], RichBlock::Paragraph { .. }));
        assert!(matches!(blocks[1], RichBlock::Photo { .. }));
        assert!(matches!(blocks[2], RichBlock::Paragraph { .. }));

        let text_doc = "Silakan unduh dokumen [dokumen: Panduan](https://example.com/doc.pdf) yang telah kami siapkan.";
        let blocks_doc = parse_markdown_to_rich_blocks(text_doc);
        assert_eq!(blocks_doc.len(), 3);
        assert!(matches!(blocks_doc[0], RichBlock::Paragraph { .. }));
        assert!(matches!(blocks_doc[1], RichBlock::Document { .. }));
        assert!(matches!(blocks_doc[2], RichBlock::Paragraph { .. }));
    }

    #[test]
    fn html_tags_convert_to_rich_formatting() {
        let input = "Teks <b>tebal</b> dan <strong>kuat</strong> serta <i>miring</i> dan <code>kode()</code> serta <a href=\"https://example.com\">Tautan</a>";
        let value = parse_inline(input);
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(serialized.contains(r#""type":"bold""#));
        assert!(serialized.contains("tebal"));
        assert!(serialized.contains("kuat"));
        assert!(serialized.contains(r#""type":"italic""#));
        assert!(serialized.contains("miring"));
        assert!(serialized.contains(r#""type":"code""#));
        assert!(serialized.contains("kode()"));
        assert!(serialized.contains(r#""type":"url""#));
        assert!(serialized.contains("https://example.com"));
    }

    #[test]
    fn github_alert_callouts_parse_correctly() {
        let text = "> [!NOTE]\n> Ini catatan penting sistem.";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 1);
        let serialized = serde_json::to_string(&blocks[0]).unwrap();
        assert!(serialized.contains("Catatan:"));
        assert!(serialized.contains("Ini catatan penting sistem."));
    }

    #[test]
    fn headings_without_space_parse_correctly() {
        let text = "###Fitur Baru\n\nPenjelasan fitur.";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(
            blocks[0],
            RichBlock::SectionHeading { level: 3, .. }
        ));
    }

    #[test]
    fn leaked_thinking_and_tool_calls_are_stripped() {
        let text = "<think>\nInternal secret reasoning\n</think>\n<tool_call>\n{\"name\": \"search\"}\n</tool_call>\nHalo! Ada yang bisa dibantu?";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 1);
        let serialized = serde_json::to_string(&blocks[0]).unwrap();
        assert!(!serialized.contains("Internal secret reasoning"));
        assert!(!serialized.contains("tool_call"));
        assert!(serialized.contains("Halo! Ada yang bisa dibantu?"));
    }

    #[test]
    fn streaming_video_urls_do_not_produce_raw_video_blocks() {
        let text = "[video: Belajar Rust](https://www.youtube.com/watch?v=5C_HPTJg5ek)\n\n![Tutorial](https://youtu.be/abc12345)";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 2);
        // Should parse as Paragraphs with styled links so Telegram link preview works without API 400 rejection
        assert!(matches!(blocks[0], RichBlock::Paragraph { .. }));
        assert!(matches!(blocks[1], RichBlock::Paragraph { .. }));
        let s0 = serde_json::to_string(&blocks[0]).unwrap();
        let s1 = serde_json::to_string(&blocks[1]).unwrap();
        assert!(s0.contains("Belajar Rust") && s0.contains("youtube.com"));
        assert!(s1.contains("Tutorial") && s1.contains("youtu.be"));
    }

    #[test]
    fn direct_video_files_produce_native_video_blocks() {
        let text = "[video: Animasi Robot](https://example.com/demo.mp4)\n\n![Clip](https://example.com/sample.webm)";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], RichBlock::Video { .. }));
        assert!(matches!(blocks[1], RichBlock::Video { .. }));
    }

    #[test]
    fn telegram_html_media_tags_parse_into_rich_blocks() {
        let text = "<tg-photo src=\"https://example.com/cat.jpg\" caption=\"Kucing Manis\"/>\n\n<tg-audio src=\"https://example.com/audio.mp3\" caption=\"Lagu Pengantar\"/>\n\n<img src=\"https://example.com/pic.png\" alt=\"Foto Profil\">";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], RichBlock::Photo { .. }));
        assert!(matches!(blocks[1], RichBlock::Audio { .. }));
        assert!(matches!(blocks[2], RichBlock::Photo { .. }));
        let cap = blocks[0].caption_text().unwrap();
        assert_eq!(cap, "Kucing Manis");
    }

    #[test]
    fn multi_line_tg_collage_and_slideshow_parse_correctly() {
        let collage_html = r#"<tg-collage caption="Koleksi Logo">
<tg-photo src="https://example.com/logo1.png"/>
<tg-photo src="https://example.com/logo2.png"/>
</tg-collage>"#;
        let blocks = parse_markdown_to_rich_blocks(collage_html);
        assert_eq!(blocks.len(), 1);
        let RichBlock::Collage {
            blocks: items,
            caption: _,
        } = &blocks[0]
        else {
            panic!("expected collage block");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(blocks[0].caption_text().as_deref(), Some("Koleksi Logo"));

        let slideshow_html = r#"<tg-slideshow caption="Alur Slide">
<tg-photo src="https://example.com/s1.jpg"/>
<tg-photo src="https://example.com/s2.jpg"/>
</tg-slideshow>"#;
        let s_blocks = parse_markdown_to_rich_blocks(slideshow_html);
        assert_eq!(s_blocks.len(), 1);
        assert!(matches!(s_blocks[0], RichBlock::Slideshow { .. }));
    }

    #[test]
    fn multiple_consecutive_photos_parse_into_separate_rich_blocks() {
        let text = "Berikut logonya:\n\n[photo: Logo Rust](https://example.com/rust.png)\n[photo: Logo Go](https://example.com/go.png)\n[photo: Logo Python](https://example.com/py.png)";
        let blocks = parse_markdown_to_rich_blocks(text);
        assert_eq!(blocks.len(), 4);
        assert!(matches!(blocks[0], RichBlock::Paragraph { .. }));
        assert!(matches!(blocks[1], RichBlock::Photo { .. }));
        assert!(matches!(blocks[2], RichBlock::Photo { .. }));
        assert!(matches!(blocks[3], RichBlock::Photo { .. }));
    }
}
