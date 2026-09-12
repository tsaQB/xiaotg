use regex::Regex;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, REFERER, USER_AGENT};
use serde_json::{json, Value};
use std::env;
use std::time::Duration;
use tracing::{info, warn};

pub fn get_tools_definition() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Cari informasi terkini atau referensi dari internet menggunakan mesin pencari.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Kata kunci pencarian yang jelas dan spesifik"
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "fetch_url",
                "description": "Ambil dan baca konten teks lengkap dari sebuah tautan URL web.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Tautan URL web yang ingin dibaca (diawali http:// atau https://)"
                        }
                    },
                    "required": ["url"]
                }
            }
        }
    ])
}

pub fn get_search_engine_status() -> (String, String) {
    let exa_key = env::var("EXA_API_KEY")
        .or_else(|_| env::var("EXA_KEY"))
        .ok()
        .filter(|s| !s.trim().is_empty());
    let tavily_key = env::var("TAVILY_API_KEY")
        .or_else(|_| env::var("TAVILY_KEY"))
        .ok()
        .filter(|s| !s.trim().is_empty());
    let brave_key = env::var("BRAVE_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let engine_name = if brave_key.is_some() {
        "Brave Search (API Key Active)".to_string()
    } else if tavily_key.is_some() {
        "Tavily AI (API Key Active)".to_string()
    } else if exa_key.is_some() {
        "Exa AI (REST API Key Active)".to_string()
    } else {
        "Exa MCP (Keyless) \u{2192} DuckDuckGo / Wikipedia".to_string()
    };

    let mcp_url = env::var("EXA_MCP_URL").unwrap_or_else(|_| "https://mcp.exa.ai/".to_string());
    (engine_name, mcp_url)
}

pub async fn execute_web_search(query: &str) -> String {
    let q = query.trim();
    if q.is_empty() {
        return "Query pencarian tidak boleh kosong.".to_string();
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    // 1. Check Brave Search API
    if let Ok(brave_key) = env::var("BRAVE_API_KEY") {
        let key = brave_key.trim();
        if !key.is_empty() {
            info!("Using Brave Search API for query: {q}");
            match search_brave(&client, key, q).await {
                Ok(res) => return res,
                Err(e) => warn!("Brave search failed ({e}), falling back to other providers"),
            }
        }
    }

    // 2. Check Tavily API
    let tavily_key = env::var("TAVILY_API_KEY")
        .or_else(|_| env::var("TAVILY_KEY"))
        .unwrap_or_default();
    let key = tavily_key.trim();
    if !key.is_empty() {
        info!("Using Tavily API for query: {q}");
        match search_tavily(&client, key, q).await {
            Ok(res) => return res,
            Err(e) => warn!("Tavily search failed ({e}), falling back to other providers"),
        }
    }

    // 3. Check Exa REST API
    let exa_key = env::var("EXA_API_KEY")
        .or_else(|_| env::var("EXA_KEY"))
        .unwrap_or_default();
    let key = exa_key.trim();
    if !key.is_empty() {
        info!("Using Exa API for query: {q}");
        match search_exa_api(&client, key, q).await {
            Ok(res) => return res,
            Err(e) => warn!("Exa API search failed ({e}), falling back to other providers"),
        }
    }

    // 4. Default / Keyless Exa MCP
    let mcp_url = env::var("EXA_MCP_URL").unwrap_or_else(|_| "https://mcp.exa.ai/".to_string());
    info!("Trying Exa Keyless MCP for query: {q}");
    match search_exa_mcp(&client, &mcp_url, q).await {
        Ok(res) => return res,
        Err(e) => {
            warn!("Gagal menghubungi Exa MCP ({e}), beralih ke DuckDuckGo...");
        }
    }

    // 5. DuckDuckGo Search Fallback
    info!("Using DuckDuckGo for query: {q}");
    match search_duckduckgo(&client, q).await {
        Ok(res) => return res,
        Err(e) => {
            warn!("DuckDuckGo did not return results or was blocked; trying Wikipedia knowledge base: {q}");
            warn!("Koneksi ke DuckDuckGo gagal ({e}). Catatan: Domain DuckDuckGo diblokir oleh beberapa ISP/Kominfo di Indonesia. Disarankan menggunakan TAVILY_API_KEY, EXA_API_KEY, atau BRAVE_API_KEY untuk hasil yang cepat.");
        }
    }

    // 6. Wikipedia Knowledge Base Fallback
    match search_wikipedia(&client, q).await {
        Ok(res) => res,
        Err(e) => format!("Tidak ditemukan hasil pencarian untuk \"{q}\": {e}"),
    }
}

async fn search_brave(client: &reqwest::Client, api_key: &str, query: &str) -> Result<String, String> {
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count=5",
        urlencoding::encode(query)
    );

    let resp = client
        .get(&url)
        .header("X-Subscription-Token", api_key)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Brave Search API: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Brave Search API returned HTTP {status}"));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Gagal membaca JSON Brave: {e}"))?;

    let mut out = String::new();
    if let Some(results) = body
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(Value::as_array)
    {
        for (i, item) in results.iter().take(5).enumerate() {
            let title = item.get("title").and_then(Value::as_str).unwrap_or("Tanpa Judul");
            let url = item.get("url").and_then(Value::as_str).unwrap_or("");
            let desc = item.get("description").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!(
                "{}. **{}**\n   URL: {}\n   Ringkasan: {}\n\n",
                i + 1,
                title,
                url,
                desc
            ));
        }
    }

    if out.trim().is_empty() {
        Ok(format!("Tidak ada hasil ditemukan di Brave untuk query \"{query}\"."))
    } else {
        Ok(format!("[Hasil Pencarian Brave untuk \"{query}\"]\n\n{out}").trim().to_string())
    }
}

async fn search_tavily(client: &reqwest::Client, api_key: &str, query: &str) -> Result<String, String> {
    let resp = client
        .post("https://api.tavily.com/search")
        .json(&json!({
            "api_key": api_key,
            "query": query,
            "include_answer": true,
            "max_results": 5,
            "search_depth": "basic"
        }))
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Tavily API: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Tavily API returned HTTP {status}"));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Gagal membaca JSON Tavily: {e}"))?;

    let mut out = String::new();
    if let Some(answer) = body.get("answer").and_then(Value::as_str).filter(|a| !a.is_empty()) {
        out.push_str(&format!("💡 **Jawaban Ringkas**: {}\n\n", answer));
    }

    if let Some(results) = body.get("results").and_then(Value::as_array) {
        for (i, item) in results.iter().take(5).enumerate() {
            let title = item.get("title").and_then(Value::as_str).unwrap_or("Tanpa Judul");
            let url = item.get("url").and_then(Value::as_str).unwrap_or("");
            let content = item.get("content").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!(
                "{}. **{}**\n   URL: {}\n   Ringkasan: {}\n\n",
                i + 1,
                title,
                url,
                content
            ));
        }
    }

    if out.trim().is_empty() {
        Ok(format!("Tidak ada hasil ditemukan di Tavily untuk query \"{query}\"."))
    } else {
        Ok(format!("[Hasil Pencarian Tavily untuk \"{query}\"]\n\n{out}").trim().to_string())
    }
}

async fn search_exa_api(client: &reqwest::Client, api_key: &str, query: &str) -> Result<String, String> {
    let resp = client
        .post("https://api.exa.ai/search")
        .header("x-api-key", api_key)
        .header(ACCEPT, "application/json")
        .json(&json!({
            "query": query,
            "numResults": 5,
            "highlights": true
        }))
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Exa API: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Exa API returned HTTP {status}"));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Gagal membaca JSON Exa: {e}"))?;

    let mut out = String::new();
    if let Some(results) = body.get("results").and_then(Value::as_array) {
        for (i, item) in results.iter().take(5).enumerate() {
            let title = item.get("title").and_then(Value::as_str).unwrap_or("Tanpa Judul");
            let url = item.get("url").and_then(Value::as_str).unwrap_or("");
            let highlight = item
                .get("highlights")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(Value::as_str)
                .or_else(|| item.get("text").and_then(Value::as_str))
                .unwrap_or("");
            out.push_str(&format!(
                "{}. **{}**\n   URL: {}\n   Ringkasan: {}\n\n",
                i + 1,
                title,
                url,
                highlight
            ));
        }
    }

    if out.trim().is_empty() {
        Ok(format!("Tidak ada hasil ditemukan di Exa untuk query \"{query}\"."))
    } else {
        Ok(format!("[Hasil Pencarian Exa AI untuk \"{query}\"]\n\n{out}").trim().to_string())
    }
}

async fn search_exa_mcp(client: &reqwest::Client, mcp_url: &str, query: &str) -> Result<String, String> {
    let resp = client
        .post(mcp_url)
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": query
                }
            }
        }))
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Exa MCP ({e})"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Exa MCP returned HTTP {status}"));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| format!("Gagal membaca stream Exa MCP: {e}"))?;

    // Exa MCP may return JSON or SSE with event/data
    let parsed_text = if let Ok(val) = serde_json::from_str::<Value>(&text) {
        if let Some(content) = val
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(Value::as_array)
        {
            content
                .iter()
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            String::new()
        }
    } else {
        // Parse SSE data: {...}
        let mut extracted: Vec<String> = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.trim();
                if let Ok(val) = serde_json::from_str::<Value>(rest) {
                    if let Some(c_arr) = val
                        .get("result")
                        .and_then(|r| r.get("content"))
                        .and_then(Value::as_array)
                    {
                        for c in c_arr {
                            if let Some(t) = c.get("text").and_then(Value::as_str) {
                                extracted.push(t.to_string());
                            }
                        }
                    }
                }
            }
        }
        extracted.join("\n\n")
    };

    if parsed_text.trim().is_empty() {
        Err("Exa MCP tidak mengembalikan konten yang valid.".to_string())
    } else {
        Ok(format!("[Hasil Pencarian Exa AI untuk \"{query}\"]\n\n{parsed_text}").trim().to_string())
    }
}

async fn search_duckduckgo(client: &reqwest::Client, query: &str) -> Result<String, String> {
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );

    let resp = client
        .get(&url)
        .header(
            USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        )
        .header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(ACCEPT_LANGUAGE, "id,en-US;q=0.9,en;q=0.8")
        .header(REFERER, "https://duckduckgo.com/")
        .send()
        .await
        .map_err(|e| format!("Gagal mencari di DuckDuckGo: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("DuckDuckGo mengembalikan status HTTP {status}"));
    }

    let html = resp
        .text()
        .await
        .map_err(|e| format!("Gagal membaca respon DuckDuckGo: {e}"))?;

    let title_regex = Regex::new(r#"<a class="result__url"[^>]*href="(?P<url>[^"]+)"[^>]*>"#).ok();
    let snippet_regex = Regex::new(r#"<a class="result__snippet"[^>]*>(?P<snippet>.*?)</a>"#).ok();

    let mut out = String::new();
    if let (Some(t_re), Some(s_re)) = (title_regex, snippet_regex) {
        let urls: Vec<String> = t_re
            .captures_iter(&html)
            .take(5)
            .filter_map(|c| c.name("url").map(|m| m.as_str().trim().to_string()))
            .collect();
        let snippets: Vec<String> = s_re
            .captures_iter(&html)
            .take(5)
            .filter_map(|c| {
                c.name("snippet").map(|m| {
                    let cleaned = m.as_str().replace("<b>", "").replace("</b>", "");
                    html_escape::decode_html_entities(&cleaned).to_string()
                })
            })
            .collect();

        for i in 0..urls.len().min(snippets.len()) {
            out.push_str(&format!(
                "{}. **Hasil Pencarian**\n   URL: {}\n   Ringkasan: {}\n\n",
                i + 1,
                urls[i],
                snippets[i]
            ));
        }
    }

    if out.trim().is_empty() {
        Err("Tidak ditemukan hasil pencarian".to_string())
    } else {
        Ok(format!("[Hasil Pencarian Web untuk \"{query}\"]\n\n{out}").trim().to_string())
    }
}

async fn search_wikipedia(client: &reqwest::Client, query: &str) -> Result<String, String> {
    let url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&utf8=1&format=json",
        urlencoding::encode(query)
    );

    let resp = client
        .get(&url)
        .header(USER_AGENT, "XiaoAI/0.3.0 (Telegram Bot Assistant)")
        .send()
        .await
        .map_err(|e| format!("Gagal menghubungi Wikipedia: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Wikipedia API returned HTTP {status}"));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Gagal membaca JSON Wikipedia: {e}"))?;

    let mut out = String::new();
    if let Some(results) = body
        .get("query")
        .and_then(|q| q.get("search"))
        .and_then(Value::as_array)
    {
        for (i, item) in results.iter().take(5).enumerate() {
            let title = item.get("title").and_then(Value::as_str).unwrap_or("");
            let snippet = item.get("snippet").and_then(Value::as_str).unwrap_or("");
            let clean_snippet = snippet
                .replace("<span class=\"searchmatch\">", "")
                .replace("</span>", "");
            let decoded_snippet = html_escape::decode_html_entities(&clean_snippet);
            let page_url = format!(
                "https://en.wikipedia.org/wiki/{}",
                urlencoding::encode(title)
            );
            out.push_str(&format!(
                "{}. **{}**\n   URL: {}\n   Ringkasan: {}\n\n",
                i + 1,
                title,
                page_url,
                decoded_snippet
            ));
        }
    }

    if out.trim().is_empty() {
        Err(format!("Tidak ada hasil ditemukan di ensiklopedia untuk query: \"{query}\""))
    } else {
        Ok(format!("[Hasil Informasi Ensiklopedia Web untuk \"{query}\"]\n\n{out}").trim().to_string())
    }
}

pub async fn fetch_web_content(url: &str) -> Result<String, String> {
    let u = url.trim();
    if !u.starts_with("http://") && !u.starts_with("https://") {
        return Err("URL harus diawali dengan http:// atau https://".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let resp = client
        .get(u)
        .header(USER_AGENT, "XiaoAI/0.3.0 (Telegram Bot Assistant)")
        .header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.5",
        )
        .header(ACCEPT_LANGUAGE, "id,en-US;q=0.9,en;q=0.8")
        .send()
        .await
        .map_err(|e| format!("Gagal mengunduh halaman web: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Halaman web mengembalikan status HTTP {status}"));
    }

    let html = resp
        .text()
        .await
        .map_err(|e| format!("Gagal membaca konten web: {e}"))?;

    // Clean scripts, styles, and html tags
    let no_script = Regex::new(r"(?is)<script.*?</script>").unwrap().replace_all(&html, "");
    let no_style = Regex::new(r"(?is)<style.*?</style>").unwrap().replace_all(&no_script, "");
    let no_head = Regex::new(r"(?is)<head.*?</head>").unwrap().replace_all(&no_style, "");
    let no_tags = Regex::new(r"<[^>]+>").unwrap().replace_all(&no_head, " ");
    let decoded = html_escape::decode_html_entities(&no_tags);

    // Collapse whitespace
    let re_space = Regex::new(r"\s+").unwrap();
    let cleaned = re_space.replace_all(&decoded, " ").trim().to_string();

    if cleaned.is_empty() {
        return Err("Halaman web tidak menghasilkan konten teks yang dapat dibaca.".to_string());
    }

    let max_len = 8000;
    if cleaned.len() > max_len {
        let truncated: String = cleaned.chars().take(max_len).collect();
        Ok(format!(
            "{}\n\n[...Konten web dipotong karena melebihi batas panjang teks XiaoAI...]",
            truncated
        ))
    } else {
        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tools_definition_contains_expected_tools() {
        let tools = get_tools_definition();
        let array = tools.as_array().expect("tools should be an array");
        assert_eq!(array.len(), 2);

        let names: Vec<_> = array
            .iter()
            .filter_map(|t| t.get("function")?.get("name")?.as_str())
            .collect();
        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"fetch_url"));
    }

    #[test]
    fn test_search_engine_status_not_empty() {
        let (engine, detail) = get_search_engine_status();
        assert!(!engine.is_empty());
        assert!(!detail.is_empty());
    }

    #[test]
    fn test_html_cleaning_logic() {
        let raw_html = "<html><head><style>body{color:red;}</style></head><body><h1>Hello &amp; Welcome</h1><script>alert(1);</script><p>This is a test.</p></body></html>";
        let no_script = Regex::new(r"(?is)<script.*?</script>").unwrap().replace_all(raw_html, "");
        let no_style = Regex::new(r"(?is)<style.*?</style>").unwrap().replace_all(&no_script, "");
        let no_head = Regex::new(r"(?is)<head.*?</head>").unwrap().replace_all(&no_style, "");
        let no_tags = Regex::new(r"<[^>]+>").unwrap().replace_all(&no_head, " ");
        let decoded = html_escape::decode_html_entities(&no_tags);
        let re_space = Regex::new(r"\s+").unwrap();
        let cleaned = re_space.replace_all(&decoded, " ").trim().to_string();

        assert_eq!(cleaned, "Hello & Welcome This is a test.");
    }
}
