use regex::Regex;
use std::io::{Cursor, Read};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use zip::ZipArchive;

const MAX_EXTRACTED_TEXT_CHARS: usize = 1_500_000;
const MAX_SCANNED_PDF_PAGES: usize = 6;
const MAX_RENDERED_PDF_BYTES: usize = 12 * 1024 * 1024;
const MAX_ZIP_XML_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PDF_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_XLSX_WORKSHEETS: usize = 64;
const MAX_XLSX_XML_BYTES_TOTAL: usize = 24 * 1024 * 1024;
const PDF_PAGE_RENDER_TIMEOUT: Duration = Duration::from_secs(12);
const PDF_RENDER_TOTAL_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_RENDERED_PAGE_DIMENSION: usize = 1600;

#[derive(Debug, Default)]
pub struct ExtractedDocument {
    pub text: Option<String>,
    pub rendered_pages: Vec<Vec<u8>>,
    pub warning: Option<String>,
}

pub fn is_extractable_document(mime: &str, name: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    mime.starts_with("text/")
        || mime == "application/json"
        || mime == "application/pdf"
        || mime == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        || mime == "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        || [
            ".txt",
            ".md",
            ".markdown",
            ".json",
            ".csv",
            ".log",
            ".rs",
            ".go",
            ".py",
            ".js",
            ".ts",
            ".tsx",
            ".jsx",
            ".toml",
            ".yaml",
            ".yml",
            ".xml",
            ".html",
            ".css",
            ".sh",
            ".sql",
            ".pdf",
            ".docx",
            ".xlsx",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

pub async fn extract_document(
    data: Vec<u8>,
    mime: &str,
    name: &str,
) -> Result<ExtractedDocument, String> {
    let mime = mime.to_ascii_lowercase();
    let name_lower = name.to_ascii_lowercase();

    if mime.starts_with("text/")
        || mime == "application/json"
        || [
            ".txt",
            ".md",
            ".markdown",
            ".json",
            ".csv",
            ".log",
            ".rs",
            ".go",
            ".py",
            ".js",
            ".ts",
            ".tsx",
            ".jsx",
            ".toml",
            ".yaml",
            ".yml",
            ".xml",
            ".html",
            ".css",
            ".sh",
            ".sql",
        ]
        .iter()
        .any(|suffix| name_lower.ends_with(suffix))
    {
        let text = String::from_utf8(data)
            .map_err(|_| "Dokumen teks harus menggunakan encoding UTF-8.".to_string())?;
        return Ok(ExtractedDocument {
            text: Some(limit_text(text)),
            ..Default::default()
        });
    }

    if mime == "application/pdf" || name_lower.ends_with(".pdf") {
        let pdf_bytes = data.clone();
        let parse_result =
            tokio::task::spawn_blocking(move || -> Result<(String, usize), String> {
                let document = lopdf::Document::load_mem_with_options(
                    &pdf_bytes,
                    lopdf::LoadOptions::with_max_decompressed_size(MAX_PDF_STREAM_BYTES),
                )
                .map_err(|err| format!("PDF tidak dapat dibaca: {err}"))?;
                let pages: Vec<u32> = document.get_pages().keys().copied().collect();
                let text = document
                    .extract_text_with_limit(&pages, MAX_PDF_STREAM_BYTES)
                    .map_err(|err| format!("Teks PDF tidak dapat diekstrak: {err}"))?;
                Ok((text, pages.len()))
            })
            .await;

        let page_count = match parse_result {
            Ok(Ok((extracted, count))) => {
                let cleaned = normalize_extracted_text(&extracted);
                if cleaned.chars().filter(|c| !c.is_whitespace()).count() >= 24 {
                    return Ok(ExtractedDocument {
                        text: Some(limit_text(cleaned)),
                        ..Default::default()
                    });
                }
                count
            }
            _ => MAX_SCANNED_PDF_PAGES,
        };

        match render_scanned_pdf_pages(&data, page_count).await {
            Ok(pages) if !pages.is_empty() => Ok(ExtractedDocument {
                text: None,
                rendered_pages: pages,
                warning: Some("PDF tampaknya berbasis gambar; halaman dirender dan akan dianalisis lewat vision model.".to_string()),
            }),
            Ok(_) => Err("PDF tidak memiliki teks yang dapat diekstrak dan renderer tidak menghasilkan halaman.".to_string()),
            Err(err) => Err(format!(
                "PDF tampaknya berupa scan/gambar dan memerlukan OCR/vision. {err}"
            )),
        }
    } else if mime == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        || name_lower.ends_with(".docx")
    {
        let bytes = data;
        let text = tokio::task::spawn_blocking(move || extract_docx_text(&bytes))
            .await
            .map_err(|err| format!("Task extractor DOCX gagal: {err}"))??;
        Ok(ExtractedDocument {
            text: Some(limit_text(text)),
            ..Default::default()
        })
    } else if mime == "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        || name_lower.ends_with(".xlsx")
    {
        let bytes = data;
        let text = tokio::task::spawn_blocking(move || extract_xlsx_text(&bytes))
            .await
            .map_err(|err| format!("Task extractor XLSX gagal: {err}"))??;
        Ok(ExtractedDocument {
            text: Some(limit_text(text)),
            ..Default::default()
        })
    } else {
        Err("Format dokumen belum didukung extractor Xiao.".to_string())
    }
}

fn extract_docx_text(data: &[u8]) -> Result<String, String> {
    let mut archive =
        ZipArchive::new(Cursor::new(data)).map_err(|err| format!("DOCX invalid: {err}"))?;
    let mut file = archive
        .by_name("word/document.xml")
        .map_err(|err| format!("DOCX tidak memiliki word/document.xml: {err}"))?;
    if file.size() > MAX_ZIP_XML_BYTES {
        return Err(
            "Entry word/document.xml melebihi batas ukuran dekompresi yang aman.".to_string(),
        );
    }
    let mut xml = String::new();
    let bytes_read = (&mut file)
        .take(MAX_ZIP_XML_BYTES + 1)
        .read_to_string(&mut xml)
        .map_err(|err| format!("Gagal membaca XML DOCX: {err}"))?;
    if bytes_read as u64 > MAX_ZIP_XML_BYTES {
        return Err(
            "Entry word/document.xml melebihi batas ukuran dekompresi yang aman.".to_string(),
        );
    }

    let paragraph_end = Regex::new(r"(?i)</w:p>").map_err(|err| err.to_string())?;
    let tab = Regex::new(r"(?i)<w:tab\s*/>").map_err(|err| err.to_string())?;
    let breaks = Regex::new(r"(?i)<w:(br|cr)\s*/>").map_err(|err| err.to_string())?;
    let tags = Regex::new(r"(?s)<[^>]+>").map_err(|err| err.to_string())?;
    let xml = paragraph_end.replace_all(&xml, "\n");
    let xml = tab.replace_all(&xml, "\t");
    let xml = breaks.replace_all(&xml, "\n");
    let stripped = tags.replace_all(&xml, "");
    Ok(normalize_extracted_text(
        html_escape::decode_html_entities(&stripped).as_ref(),
    ))
}

fn extract_xlsx_text(data: &[u8]) -> Result<String, String> {
    let mut archive =
        ZipArchive::new(Cursor::new(data)).map_err(|err| format!("XLSX invalid: {err}"))?;
    let shared = read_zip_text_optional(&mut archive, "xl/sharedStrings.xml")?;
    let mut xml_budget = 0usize;
    if let Some(shared_xml) = shared.as_deref() {
        add_xlsx_xml_budget(&mut xml_budget, shared_xml.len())?;
    }
    let shared_strings = shared
        .as_deref()
        .map(extract_shared_strings)
        .transpose()?
        .unwrap_or_default();

    let mut worksheet_names = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|err| err.to_string())?;
        let name = entry.name().to_string();
        if name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml") {
            worksheet_names.push(name);
            if worksheet_names.len() > MAX_XLSX_WORKSHEETS {
                return Err(format!("XLSX memiliki lebih dari {MAX_XLSX_WORKSHEETS} worksheet; ditolak untuk mencegah resource exhaustion."));
            }
        }
    }
    worksheet_names.sort();

    let row_re = Regex::new(r#"(?s)<row\b[^>]*>(.*?)</row>"#).map_err(|err| err.to_string())?;
    let cell_re = Regex::new(r#"(?s)<c\b([^>]*)>(.*?)</c>"#).map_err(|err| err.to_string())?;
    let value_re = Regex::new(r"(?s)<v>(.*?)</v>").map_err(|err| err.to_string())?;
    let inline_re = Regex::new(r"(?s)<t[^>]*>(.*?)</t>").map_err(|err| err.to_string())?;
    let mut output = String::new();

    for sheet_name in worksheet_names {
        let xml = read_zip_text(&mut archive, &sheet_name)?;
        add_xlsx_xml_budget(&mut xml_budget, xml.len())?;
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(&format!(
            "[{}]\n",
            Path::new(&sheet_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("sheet")
        ));
        let mut sheet_rows = Vec::new();
        for row_caps in row_re.captures_iter(&xml) {
            let row_body = row_caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let mut row_values = Vec::new();
            for captures in cell_re.captures_iter(row_body) {
                let attrs = captures.get(1).map(|m| m.as_str()).unwrap_or("");
                let body = captures.get(2).map(|m| m.as_str()).unwrap_or("");
                let value = if attrs.contains("t=\"s\"") {
                    value_re
                        .captures(body)
                        .and_then(|caps| caps.get(1))
                        .and_then(|m| m.as_str().trim().parse::<usize>().ok())
                        .and_then(|idx| shared_strings.get(idx).cloned())
                        .unwrap_or_default()
                } else if attrs.contains("t=\"inlineStr\"") {
                    inline_re
                        .captures(body)
                        .and_then(|caps| caps.get(1))
                        .map(|m| html_escape::decode_html_entities(m.as_str()).to_string())
                        .unwrap_or_default()
                } else {
                    value_re
                        .captures(body)
                        .and_then(|caps| caps.get(1))
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default()
                };
                if !value.is_empty() {
                    row_values.push(value);
                }
            }
            if !row_values.is_empty() {
                sheet_rows.push(row_values.join("\t"));
            }
        }
        if !sheet_rows.is_empty() {
            output.push_str(&sheet_rows.join("\n"));
        }
    }

    let normalized = normalize_extracted_text(&output);
    if normalized.trim().is_empty() {
        Err("XLSX tidak mengandung nilai sel yang dapat diekstrak.".to_string())
    } else {
        Ok(normalized)
    }
}

fn add_xlsx_xml_budget(total: &mut usize, additional: usize) -> Result<(), String> {
    let next = total.saturating_add(additional);
    if next > MAX_XLSX_XML_BYTES_TOTAL {
        return Err(format!(
            "Total XML XLSX melebihi batas aman {} MiB.",
            MAX_XLSX_XML_BYTES_TOTAL / (1024 * 1024)
        ));
    }
    *total = next;
    Ok(())
}

fn read_zip_text_optional<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<String>, String> {
    match archive.by_name(name) {
        Ok(mut file) => {
            if file.size() > MAX_ZIP_XML_BYTES {
                return Err(format!(
                    "Entry {name} terlalu besar untuk diekstrak dengan aman."
                ));
            }
            let mut value = String::new();
            let bytes_read = (&mut file)
                .take(MAX_ZIP_XML_BYTES + 1)
                .read_to_string(&mut value)
                .map_err(|err| format!("Gagal membaca {name}: {err}"))?;
            if bytes_read as u64 > MAX_ZIP_XML_BYTES {
                return Err(format!(
                    "Entry {name} melebihi batas ukuran dekompresi yang aman."
                ));
            }
            Ok(Some(value))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(err) => Err(format!("Gagal membuka {name}: {err}")),
    }
}

fn read_zip_text<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<String, String> {
    read_zip_text_optional(archive, name)?.ok_or_else(|| format!("{name} tidak ditemukan"))
}

fn extract_shared_strings(xml: &str) -> Result<Vec<String>, String> {
    let si_re = Regex::new(r#"(?s)<si\b[^>]*>(.*?)</si>"#).map_err(|err| err.to_string())?;
    let t_re = Regex::new(r#"(?s)<t\b[^>]*>(.*?)</t>"#).map_err(|err| err.to_string())?;
    let mut strings = Vec::new();
    for si_caps in si_re.captures_iter(xml) {
        let si_body = si_caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let mut entry = String::new();
        for t_caps in t_re.captures_iter(si_body) {
            if let Some(m) = t_caps.get(1) {
                entry.push_str(&html_escape::decode_html_entities(m.as_str()));
            }
        }
        strings.push(entry);
    }
    Ok(strings)
}

fn normalize_extracted_text(text: &str) -> String {
    let mut output = String::new();
    let mut previous_blank = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            if !previous_blank && !output.is_empty() {
                output.push('\n');
            }
            previous_blank = true;
        } else {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
            previous_blank = false;
        }
    }
    output.trim().to_string()
}

fn limit_text(text: String) -> String {
    if text.chars().count() <= MAX_EXTRACTED_TEXT_CHARS {
        text
    } else {
        let mut limited: String = text.chars().take(MAX_EXTRACTED_TEXT_CHARS).collect();
        limited.push_str("\n\n[Dokumen dipotong oleh batas konteks extractor Xiao]");
        limited
    }
}

struct TempDirGuard(std::path::PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn render_scanned_pdf_pages(data: &[u8], page_count: usize) -> Result<Vec<Vec<u8>>, String> {
    let suffix: u64 = rand::random();
    let temp_dir = std::env::temp_dir().join(format!("xiao-pdf-{suffix}"));
    let _guard = TempDirGuard(temp_dir.clone());
    let input = temp_dir.join("input.pdf");

    tokio::fs::create_dir(&temp_dir)
        .await
        .map_err(|err| format!("Gagal membuat direktori PDF sementara: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&temp_dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|err| format!("Gagal mengamankan direktori PDF sementara: {err}"))?;
    }
    tokio::fs::write(&input, data)
        .await
        .map_err(|err| format!("Gagal menulis PDF sementara: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|err| format!("Gagal mengamankan PDF sementara: {err}"))?;
    }

    let render_result = tokio::time::timeout(PDF_RENDER_TOTAL_TIMEOUT, async {
        let mut pages = Vec::new();
        let mut total = 0usize;
        let pages_to_render = page_count.min(MAX_SCANNED_PDF_PAGES);

        for index in 1..=pages_to_render {
            let prefix = temp_dir.join(format!("page-{index}"));
            let output_path = prefix.with_extension("png");
            let mut child = Command::new("pdftoppm")
                .arg("-png")
                .arg("-q")
                .arg("-scale-to")
                .arg(MAX_RENDERED_PAGE_DIMENSION.to_string())
                .arg("-f")
                .arg(index.to_string())
                .arg("-l")
                .arg(index.to_string())
                .arg("-singlefile")
                .arg(&input)
                .arg(&prefix)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|err| {
                    format!(
                        "Renderer pdftoppm tidak tersedia ({err}). Instal poppler-utils pada host Linux untuk OCR PDF scan."
                    )
                })?;

            let status = match tokio::time::timeout(PDF_PAGE_RENDER_TIMEOUT, child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(err)) => return Err(format!("pdftoppm gagal dijalankan: {err}")),
                Err(_) => {
                    let _ = child.kill().await;
                    return Err(format!(
                        "pdftoppm melebihi timeout {} detik per halaman.",
                        PDF_PAGE_RENDER_TIMEOUT.as_secs()
                    ));
                }
            };
            if !status.success() {
                return Err(format!("pdftoppm gagal saat merender halaman {index}."));
            }

            let metadata = tokio::fs::metadata(&output_path)
                .await
                .map_err(|err| format!("Output pdftoppm halaman {index} tidak tersedia: {err}"))?;
            let page_size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
            if page_size > MAX_RENDERED_PDF_BYTES
                || total.saturating_add(page_size) > MAX_RENDERED_PDF_BYTES
            {
                return Err(format!(
                    "Output render PDF melebihi batas aman {} MiB.",
                    MAX_RENDERED_PDF_BYTES / (1024 * 1024)
                ));
            }

            let bytes = tokio::fs::read(&output_path)
                .await
                .map_err(|err| format!("Gagal membaca render PDF halaman {index}: {err}"))?;
            total = total.saturating_add(bytes.len());
            pages.push(bytes);
            let _ = tokio::fs::remove_file(&output_path).await;
        }

        Ok(pages)
    })
    .await
    .unwrap_or_else(|_| {
        Err(format!(
            "Render PDF melebihi timeout total {} detik.",
            PDF_RENDER_TOTAL_TIMEOUT.as_secs()
        ))
    });

    render_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn recognizes_supported_documents() {
        assert!(is_extractable_document("application/pdf", "x.bin"));
        assert!(is_extractable_document("", "notes.docx"));
        assert!(is_extractable_document("text/plain", "file"));
        assert!(!is_extractable_document(
            "application/octet-stream",
            "archive.zip"
        ));
    }

    #[test]
    fn xlsx_aggregate_xml_budget_is_bounded() {
        let mut total = 0usize;
        assert!(add_xlsx_xml_budget(&mut total, MAX_XLSX_XML_BYTES_TOTAL).is_ok());
        assert!(add_xlsx_xml_budget(&mut total, 1).is_err());
    }

    #[test]
    fn docx_xml_is_extracted() {
        let cursor = Cursor::new(Vec::<u8>::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("word/document.xml", options).unwrap();
        writer
            .write_all(br#"<w:document><w:body><w:p><w:r><w:t>Hello &amp; world</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r></w:p></w:body></w:document>"#)
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let text = extract_docx_text(&bytes).unwrap();
        assert!(text.contains("Hello & world"));
        assert!(text.contains("Second"));
    }

    #[test]
    fn xlsx_extracts_rows_and_handles_rich_text_si() {
        let cursor = Cursor::new(Vec::<u8>::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();

        writer.start_file("xl/sharedStrings.xml", options).unwrap();
        writer
            .write_all(
                br#"<sst count="2" uniqueCount="2">
                    <si><r><t>Rich </t></r><r><t>Text</t></r></si>
                    <si><t>Second String</t></si>
                </sst>"#,
            )
            .unwrap();

        writer
            .start_file("xl/worksheets/sheet1.xml", options)
            .unwrap();
        writer
            .write_all(
                br#"<worksheet>
                    <sheetData>
                        <row r="1">
                            <c r="A1" t="s"><v>0</v></c>
                            <c r="B1" t="s"><v>1</v></c>
                        </row>
                        <row r="2">
                            <c r="A2"><v>100</v></c>
                            <c r="B2"><v>200</v></c>
                        </row>
                    </sheetData>
                </worksheet>"#,
            )
            .unwrap();

        let bytes = writer.finish().unwrap().into_inner();
        let text = extract_xlsx_text(&bytes).unwrap();
        assert!(text.contains("Rich Text\tSecond String"));
        assert!(text.contains("100\t200"));
        assert!(text.contains("Rich Text\tSecond String\n100\t200"));
    }

    #[test]
    fn zip_bomb_entry_over_decompression_limit_is_rejected() {
        let cursor = Cursor::new(Vec::<u8>::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("word/document.xml", options).unwrap();
        // Write repeating spaces/zeroes that compress to very few bytes but decompress beyond limit
        let big_chunk = vec![b' '; 1024 * 1024]; // 1 MiB chunk
        for _ in 0..9 {
            // 9 MiB > MAX_ZIP_XML_BYTES (8 MiB)
            writer.write_all(&big_chunk).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        let err = extract_docx_text(&bytes).unwrap_err();
        assert!(err.contains("melebihi batas ukuran dekompresi yang aman"));
    }

    #[test]
    fn zip_bomb_with_forged_header_is_rejected_by_stream_limit() {
        let cursor = Cursor::new(Vec::<u8>::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("word/document.xml", options).unwrap();
        let big_chunk = vec![b' '; 1024 * 1024];
        for _ in 0..9 {
            writer.write_all(&big_chunk).unwrap();
        }
        let mut bytes = writer.finish().unwrap().into_inner();
        if let Some(pos) = bytes.windows(4).position(|w| w == [0x50, 0x4b, 0x03, 0x04]) {
            bytes[pos + 22..pos + 26].copy_from_slice(&100_u32.to_le_bytes());
        }
        if let Some(pos) = bytes.windows(4).position(|w| w == [0x50, 0x4b, 0x01, 0x02]) {
            bytes[pos + 24..pos + 28].copy_from_slice(&100_u32.to_le_bytes());
        }
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let file = archive.by_name("word/document.xml").unwrap();
        assert_eq!(file.size(), 100);
        drop(file);
        drop(archive);

        let err = extract_docx_text(&bytes).unwrap_err();
        assert!(err.contains("melebihi batas ukuran dekompresi yang aman"));
    }
}
