use std::io::{Cursor, Read};
use zip::ZipArchive;

pub const MAX_ARCHIVE_TOTAL_UNCOMPRESSED_BYTES: usize = 30 * 1024 * 1024;
pub const MAX_ARCHIVE_SINGLE_ENTRY_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ARCHIVE_OUTPUT_CHARS: usize = 1_500_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
    SevenZ,
}

impl ArchiveKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Zip => "ZIP",
            Self::Tar => "TAR",
            Self::TarGz => "TAR.GZ",
            Self::SevenZ => "7Z",
        }
    }
}

pub fn detect_archive_kind(mime: &str, name: &str) -> Option<ArchiveKind> {
    let clean_mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let lower_name = name.trim().to_ascii_lowercase();

    if clean_mime == "application/zip"
        || clean_mime == "application/x-zip-compressed"
        || clean_mime == "multipart/x-zip"
    {
        return Some(ArchiveKind::Zip);
    }
    if clean_mime == "application/x-tar" || clean_mime == "application/tar" {
        return Some(ArchiveKind::Tar);
    }
    if clean_mime == "application/gzip"
        || clean_mime == "application/x-gzip"
        || clean_mime == "application/x-compressed-tar"
        || clean_mime == "application/x-tgz"
    {
        return Some(ArchiveKind::TarGz);
    }
    if clean_mime == "application/x-7z-compressed" || clean_mime == "application/7z" {
        return Some(ArchiveKind::SevenZ);
    }

    if lower_name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if lower_name.ends_with(".tar.gz") || lower_name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if lower_name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else if lower_name.ends_with(".7z") {
        Some(ArchiveKind::SevenZ)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveEntryCategory {
    Directory,
    Text,
    NestedArchive,
    Binary,
}

pub fn classify_archive_entry(path: &str) -> ArchiveEntryCategory {
    let clean = path.replace('\\', "/");
    let trimmed = clean.trim();
    if trimmed.ends_with('/') || trimmed.is_empty() {
        return ArchiveEntryCategory::Directory;
    }

    let lower = trimmed.to_ascii_lowercase();
    let file_name = lower.split('/').next_back().unwrap_or("");

    // Nested archives: Single-Level Flat rule (do not unpack recursively)
    if [
        ".zip", ".tar", ".tar.gz", ".tgz", ".7z", ".rar", ".gz", ".bz2", ".xz", ".zst",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
    {
        return ArchiveEntryCategory::NestedArchive;
    }

    // Known text & code extensions
    const TEXT_EXTENSIONS: &[&str] = &[
        ".txt",
        ".md",
        ".markdown",
        ".rst",
        ".log",
        ".json",
        ".jsonl",
        ".csv",
        ".tsv",
        ".xml",
        ".yaml",
        ".yml",
        ".toml",
        ".ini",
        ".env",
        ".conf",
        ".cfg",
        ".properties",
        ".html",
        ".htm",
        ".css",
        ".scss",
        ".sass",
        ".less",
        ".svg",
        ".rs",
        ".py",
        ".go",
        ".js",
        ".mjs",
        ".cjs",
        ".ts",
        ".tsx",
        ".jsx",
        ".c",
        ".cpp",
        ".cc",
        ".cxx",
        ".h",
        ".hpp",
        ".hxx",
        ".java",
        ".kt",
        ".kts",
        ".swift",
        ".rb",
        ".php",
        ".cs",
        ".sh",
        ".bash",
        ".zsh",
        ".fish",
        ".ps1",
        ".bat",
        ".cmd",
        ".sql",
        ".r",
        ".lua",
        ".zig",
        ".dart",
        ".scala",
        ".pl",
        ".pm",
        ".proto",
        ".graphql",
        ".gql",
        ".diff",
        ".patch",
    ];

    if TEXT_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        return ArchiveEntryCategory::Text;
    }

    // Known text filenames without extensions
    const TEXT_FILENAMES: &[&str] = &[
        "dockerfile",
        "makefile",
        "cmakelists.txt",
        "license",
        "licence",
        "readme",
        "gemfile",
        "procfile",
        "vagrantfile",
        "rakefile",
        ".gitignore",
        ".dockerignore",
        ".editorconfig",
        ".npmignore",
        ".gitattributes",
        ".prettierrc",
        ".eslintrc",
    ];

    if TEXT_FILENAMES.contains(&file_name) {
        return ArchiveEntryCategory::Text;
    }

    // Known binary extensions
    const BINARY_EXTENSIONS: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".bmp", ".tiff", ".pdf", ".docx",
        ".xlsx", ".pptx", ".doc", ".xls", ".ppt", ".exe", ".dll", ".so", ".dylib", ".bin", ".iso",
        ".apk", ".aab", ".jar", ".war", ".ear", ".class", ".pyc", ".pyo", ".pyd", ".o", ".a",
        ".lib", ".wasm", ".mp3", ".mp4", ".wav", ".ogg", ".opus", ".m4a", ".flac", ".webm", ".mov",
        ".avi", ".mkv", ".woff", ".woff2", ".ttf", ".eot", ".otf", ".db", ".sqlite", ".sqlite3",
        ".dat",
    ];

    if BINARY_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        return ArchiveEntryCategory::Binary;
    }

    // Fallback for unknown extension: will be validated against byte stream
    ArchiveEntryCategory::Text
}

pub fn sanitize_archive_path(raw_path: &str) -> String {
    let normalized = raw_path.replace('\\', "/");
    let is_dir = normalized.ends_with('/');
    let mut segments = Vec::new();
    for seg in normalized.split('/') {
        let seg = seg.trim();
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            // Prevent path traversal
            continue;
        }
        if seg.len() == 2 && seg.ends_with(':') {
            // Strip Windows drive letters (e.g. C:)
            continue;
        }
        segments.push(seg);
    }
    if segments.is_empty() {
        if is_dir {
            "root/".to_string()
        } else {
            "root".to_string()
        }
    } else {
        let path = segments.join("/");
        if is_dir {
            format!("{path}/")
        } else {
            path
        }
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        let kb = bytes as f64 / 1024.0;
        format!("{kb:.1} KB")
    } else {
        let mb = bytes as f64 / (1024.0 * 1024.0);
        format!("{mb:.1} MB")
    }
}

#[derive(Debug, Clone)]
pub struct ExtractedArchiveItem {
    pub path: String,
    pub size: u64,
    pub category: ArchiveEntryCategory,
    pub text_content: Option<String>,
}

pub fn extract_archive(data: &[u8], kind: ArchiveKind, name: &str) -> Result<String, String> {
    let (items, budget_exhausted) = match kind {
        ArchiveKind::Zip => extract_zip(data)?,
        ArchiveKind::Tar => extract_tar(data, false)?,
        ArchiveKind::TarGz => extract_tar(data, true)?,
        ArchiveKind::SevenZ => extract_7z(data)?,
    };

    Ok(build_archive_text_output(
        name,
        kind,
        items,
        budget_exhausted.as_deref(),
    ))
}

fn extract_zip(data: &[u8]) -> Result<(Vec<ExtractedArchiveItem>, Option<String>), String> {
    let cursor = Cursor::new(data);
    let mut archive = ZipArchive::new(cursor).map_err(|err| format!("ZIP invalid: {err}"))?;

    let mut items = Vec::with_capacity(archive.len());
    let mut total_uncompressed: usize = 0;
    let mut total_extracted_chars: usize = 0;
    let mut budget_exhausted: Option<String> = None;

    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| format!("Gagal membaca entri ZIP index {index}: {err}"))?;

        let raw_name = file.name().to_string();
        let path = sanitize_archive_path(&raw_name);
        let size = file.size();
        let is_dir = file.is_dir() || raw_name.ends_with('/');

        if is_dir {
            items.push(ExtractedArchiveItem {
                path,
                size,
                category: ArchiveEntryCategory::Directory,
                text_content: None,
            });
            continue;
        }

        let mut category = classify_archive_entry(&path);
        let mut text_content = None;

        if category == ArchiveEntryCategory::Text {
            if budget_exhausted.is_some() {
                // Keep file in manifest, skip reading content
            } else if size as usize > MAX_ARCHIVE_SINGLE_ENTRY_BYTES {
                category = ArchiveEntryCategory::Binary;
            } else if total_uncompressed.saturating_add(size as usize)
                > MAX_ARCHIVE_TOTAL_UNCOMPRESSED_BYTES
            {
                budget_exhausted = Some(
                    "Batas dekompresi 30 MB tercapai; sisa berkas hanya dicatat pada struktur direktori."
                        .to_string(),
                );
            } else {
                let mut buf = Vec::with_capacity(size.min(65536) as usize);
                let read_res = (&mut file)
                    .take(MAX_ARCHIVE_SINGLE_ENTRY_BYTES as u64 + 1)
                    .read_to_end(&mut buf);

                if let Ok(bytes_read) = read_res {
                    total_uncompressed = total_uncompressed.saturating_add(bytes_read);
                    if bytes_read <= MAX_ARCHIVE_SINGLE_ENTRY_BYTES && is_valid_text_bytes(&buf) {
                        if let Ok(text) = String::from_utf8(buf) {
                            if total_extracted_chars.saturating_add(text.len())
                                > MAX_ARCHIVE_OUTPUT_CHARS
                            {
                                budget_exhausted = Some(
                                    "Batas kapasitas konteks teks tercapai; sisa berkas hanya dicatat pada struktur direktori."
                                        .to_string(),
                                );
                            } else {
                                total_extracted_chars =
                                    total_extracted_chars.saturating_add(text.len());
                                text_content = Some(text);
                            }
                        } else {
                            category = ArchiveEntryCategory::Binary;
                        }
                    } else {
                        category = ArchiveEntryCategory::Binary;
                    }
                }
            }
        }

        items.push(ExtractedArchiveItem {
            path,
            size,
            category,
            text_content,
        });
    }

    Ok((items, budget_exhausted))
}

fn extract_tar(
    data: &[u8],
    is_gz: bool,
) -> Result<(Vec<ExtractedArchiveItem>, Option<String>), String> {
    let mut items = Vec::new();
    let mut total_uncompressed: usize = 0;
    let mut total_extracted_chars: usize = 0;
    let mut budget_exhausted: Option<String> = None;

    let reader: Box<dyn Read> = if is_gz {
        Box::new(flate2::read::GzDecoder::new(Cursor::new(data)))
    } else {
        Box::new(Cursor::new(data))
    };

    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|err| format!("TAR invalid: {err}"))?;

    for entry_res in entries {
        let mut entry = entry_res.map_err(|err| format!("Entri TAR invalid: {err}"))?;
        let entry_type = entry.header().entry_type();

        let path_str = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "invalid_path".to_string());
        let path = sanitize_archive_path(&path_str);
        let size = entry.header().size().unwrap_or(0);

        if entry_type.is_dir() || path_str.ends_with('/') {
            items.push(ExtractedArchiveItem {
                path,
                size,
                category: ArchiveEntryCategory::Directory,
                text_content: None,
            });
            continue;
        }

        if entry_type.is_symlink() || entry_type.is_hard_link() {
            continue;
        }

        let mut category = classify_archive_entry(&path);
        let mut text_content = None;

        if category == ArchiveEntryCategory::Text {
            if budget_exhausted.is_some() {
                // Skip reading content once budget is hit
            } else if size as usize > MAX_ARCHIVE_SINGLE_ENTRY_BYTES {
                category = ArchiveEntryCategory::Binary;
            } else if total_uncompressed.saturating_add(size as usize)
                > MAX_ARCHIVE_TOTAL_UNCOMPRESSED_BYTES
            {
                budget_exhausted = Some(
                    "Batas dekompresi 30 MB tercapai; sisa berkas hanya dicatat pada struktur direktori."
                        .to_string(),
                );
            } else {
                let mut buf = Vec::with_capacity(size.min(65536) as usize);
                let read_res = (&mut entry)
                    .take(MAX_ARCHIVE_SINGLE_ENTRY_BYTES as u64 + 1)
                    .read_to_end(&mut buf);

                if let Ok(bytes_read) = read_res {
                    total_uncompressed = total_uncompressed.saturating_add(bytes_read);
                    if bytes_read <= MAX_ARCHIVE_SINGLE_ENTRY_BYTES && is_valid_text_bytes(&buf) {
                        if let Ok(text) = String::from_utf8(buf) {
                            if total_extracted_chars.saturating_add(text.len())
                                > MAX_ARCHIVE_OUTPUT_CHARS
                            {
                                budget_exhausted = Some(
                                    "Batas kapasitas konteks teks tercapai; sisa berkas hanya dicatat pada struktur direktori."
                                        .to_string(),
                                );
                            } else {
                                total_extracted_chars =
                                    total_extracted_chars.saturating_add(text.len());
                                text_content = Some(text);
                            }
                        } else {
                            category = ArchiveEntryCategory::Binary;
                        }
                    } else {
                        category = ArchiveEntryCategory::Binary;
                    }
                }
            }
        }

        items.push(ExtractedArchiveItem {
            path,
            size,
            category,
            text_content,
        });
    }

    Ok((items, budget_exhausted))
}

fn extract_7z(data: &[u8]) -> Result<(Vec<ExtractedArchiveItem>, Option<String>), String> {
    let cursor = Cursor::new(data);
    let len = data.len() as u64;
    let mut reader = sevenz_rust::SevenZReader::new(cursor, len, sevenz_rust::Password::empty())
        .map_err(|err| format!("7Z invalid: {err}"))?;

    let mut items = Vec::new();
    let mut total_uncompressed: usize = 0;
    let mut total_extracted_chars: usize = 0;
    let mut budget_exhausted: Option<String> = None;

    reader
        .for_each_entries(|entry, entry_reader| {
            let path_str = entry.name().to_string();
            let path = sanitize_archive_path(&path_str);
            let size = entry.size();
            let is_dir = entry.is_directory() || path_str.ends_with('/');

            if is_dir {
                items.push(ExtractedArchiveItem {
                    path,
                    size,
                    category: ArchiveEntryCategory::Directory,
                    text_content: None,
                });
                return Ok(true);
            }

            let mut category = classify_archive_entry(&path);
            let mut text_content = None;

            if category == ArchiveEntryCategory::Text {
                if budget_exhausted.is_some() {
                    // Skip reading stream
                } else if size as usize > MAX_ARCHIVE_SINGLE_ENTRY_BYTES {
                    category = ArchiveEntryCategory::Binary;
                } else if total_uncompressed.saturating_add(size as usize)
                    > MAX_ARCHIVE_TOTAL_UNCOMPRESSED_BYTES
                {
                    budget_exhausted = Some(
                        "Batas dekompresi 30 MB tercapai; sisa berkas hanya dicatat pada struktur direktori."
                            .to_string(),
                    );
                } else {
                    let mut buf = Vec::with_capacity(size.min(65536) as usize);
                    let read_res = entry_reader
                        .take(MAX_ARCHIVE_SINGLE_ENTRY_BYTES as u64 + 1)
                        .read_to_end(&mut buf);

                    if let Ok(bytes_read) = read_res {
                        total_uncompressed = total_uncompressed.saturating_add(bytes_read);
                        if bytes_read <= MAX_ARCHIVE_SINGLE_ENTRY_BYTES && is_valid_text_bytes(&buf)
                        {
                            if let Ok(text) = String::from_utf8(buf) {
                                if total_extracted_chars.saturating_add(text.len())
                                    > MAX_ARCHIVE_OUTPUT_CHARS
                                {
                                    budget_exhausted = Some(
                                        "Batas kapasitas konteks teks tercapai; sisa berkas hanya dicatat pada struktur direktori."
                                            .to_string(),
                                    );
                                } else {
                                    total_extracted_chars =
                                        total_extracted_chars.saturating_add(text.len());
                                    text_content = Some(text);
                                }
                            } else {
                                category = ArchiveEntryCategory::Binary;
                            }
                        } else {
                            category = ArchiveEntryCategory::Binary;
                        }
                    }
                }
            }

            items.push(ExtractedArchiveItem {
                path,
                size,
                category,
                text_content,
            });

            Ok(true)
        })
        .map_err(|err| format!("Gagal mengekstrak berkas 7Z: {err}"))?;

    Ok((items, budget_exhausted))
}

fn is_valid_text_bytes(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    // Reject binary null bytes
    if bytes.contains(&0) {
        return false;
    }
    std::str::from_utf8(bytes).is_ok()
}

pub fn build_archive_text_output(
    archive_name: &str,
    kind: ArchiveKind,
    items: Vec<ExtractedArchiveItem>,
    budget_exhausted: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "=== ARSIP: {archive_name} ({}, Total {} Entri) ===\n\n",
        kind.label(),
        items.len()
    ));

    out.push_str("[STRUKTUR DIREKTORI]\n");
    if items.is_empty() {
        out.push_str("(Arsip kosong)\n\n");
    } else {
        for item in &items {
            let size_str = format_size(item.size);
            match item.category {
                ArchiveEntryCategory::Directory => {
                    out.push_str(&format!("📁 {} (Direktori)\n", item.path));
                }
                ArchiveEntryCategory::NestedArchive => {
                    out.push_str(&format!(
                        "📦 {} ({size_str}) [Arsip - Dilewati]\n",
                        item.path
                    ));
                }
                ArchiveEntryCategory::Binary => {
                    out.push_str(&format!("📄 {} ({size_str}) [Biner / Media]\n", item.path));
                }
                ArchiveEntryCategory::Text => {
                    out.push_str(&format!("📄 {} ({size_str})\n", item.path));
                }
            }
        }
        out.push('\n');
    }

    let text_files: Vec<&ExtractedArchiveItem> = items
        .iter()
        .filter(|item| item.text_content.is_some())
        .collect();

    if !text_files.is_empty() {
        out.push_str("[KONTEN BERKAS (TEKS & KODE)]\n");
        for item in text_files {
            out.push_str(&format!("\n--- BERKAS: {} ---\n", item.path));
            if let Some(content) = item.text_content.as_deref() {
                out.push_str(content.trim_end());
                out.push('\n');
            }
        }
    }

    if let Some(warning) = budget_exhausted {
        out.push_str(&format!("\n⚠️ [{warning}]\n"));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_archive_kinds_by_mime_and_extension() {
        assert_eq!(
            detect_archive_kind("application/zip", "project.zip"),
            Some(ArchiveKind::Zip)
        );
        assert_eq!(
            detect_archive_kind("application/x-tar", "data.tar"),
            Some(ArchiveKind::Tar)
        );
        assert_eq!(
            detect_archive_kind("application/gzip", "bundle.tar.gz"),
            Some(ArchiveKind::TarGz)
        );
        assert_eq!(
            detect_archive_kind("", "package.tgz"),
            Some(ArchiveKind::TarGz)
        );
        assert_eq!(
            detect_archive_kind("application/x-7z-compressed", "docs.7z"),
            Some(ArchiveKind::SevenZ)
        );
        assert_eq!(detect_archive_kind("application/pdf", "file.pdf"), None);
    }

    #[test]
    fn classifies_text_binary_nested_and_directory() {
        assert_eq!(
            classify_archive_entry("src/main.rs"),
            ArchiveEntryCategory::Text
        );
        assert_eq!(
            classify_archive_entry("README.md"),
            ArchiveEntryCategory::Text
        );
        assert_eq!(
            classify_archive_entry("docs/"),
            ArchiveEntryCategory::Directory
        );
        assert_eq!(
            classify_archive_entry("assets/logo.png"),
            ArchiveEntryCategory::Binary
        );
        assert_eq!(
            classify_archive_entry("vendor/library.zip"),
            ArchiveEntryCategory::NestedArchive
        );
        assert_eq!(
            classify_archive_entry("bundle.tar.gz"),
            ArchiveEntryCategory::NestedArchive
        );
    }

    #[test]
    fn sanitizes_traversal_paths() {
        assert_eq!(sanitize_archive_path("../../etc/passwd"), "etc/passwd");
        assert_eq!(
            sanitize_archive_path("C:\\Windows\\System32\\file.txt"),
            "Windows/System32/file.txt"
        );
        assert_eq!(sanitize_archive_path("./src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn extracts_zip_archive_in_memory() {
        let cursor = Cursor::new(Vec::<u8>::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        writer.start_file("README.md", options).unwrap();
        writer.write_all(b"# Hello Xiao\nThis is a test.").unwrap();

        writer.start_file("data.json", options).unwrap();
        writer.write_all(b"{\"version\": 1}").unwrap();

        writer.start_file("image.png", options).unwrap();
        writer
            .write_all(&[0, 137, 80, 78, 71, 13, 10, 26, 10])
            .unwrap(); // Contains null byte

        let bytes = writer.finish().unwrap().into_inner();
        let result = extract_archive(&bytes, ArchiveKind::Zip, "test.zip").unwrap();

        assert!(result.contains("=== ARSIP: test.zip (ZIP, Total 3 Entri) ==="));
        assert!(result.contains("README.md"));
        assert!(result.contains("data.json"));
        assert!(result.contains("image.png"));
        assert!(result.contains("[Biner / Media]"));
        assert!(result.contains("--- BERKAS: README.md ---"));
        assert!(result.contains("# Hello Xiao"));
        assert!(result.contains("--- BERKAS: data.json ---"));
        assert!(result.contains("{\"version\": 1}"));
    }

    #[test]
    fn extracts_tar_gz_archive_in_memory() {
        let mut tar_builder = tar::Builder::new(Vec::new());

        let mut header = tar::Header::new_gnu();
        let content = b"fn main() { println!(\"Hi\"); }";
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, "src/main.rs", &content[..])
            .unwrap();

        let tar_bytes = tar_builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let tar_gz_bytes = encoder.finish().unwrap();

        let result = extract_archive(&tar_gz_bytes, ArchiveKind::TarGz, "project.tar.gz").unwrap();
        assert!(result.contains("=== ARSIP: project.tar.gz (TAR.GZ, Total 1 Entri) ==="));
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("--- BERKAS: src/main.rs ---"));
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn extracts_7z_archive_in_memory() {
        let cursor = Cursor::new(Vec::new());
        let mut sz = sevenz_rust::SevenZWriter::new(cursor).unwrap();

        let mut entry = sevenz_rust::SevenZArchiveEntry::default();
        entry.name = "hello.rs".to_string();
        entry.is_directory = false;
        entry.has_stream = true;

        let data = b"pub fn greet() -> &'static str { \"Halo Xiao\" }";
        sz.push_archive_entry(entry, Some(&data[..])).unwrap();
        let bytes = sz.finish().unwrap().into_inner();
        let result = extract_archive(&bytes, ArchiveKind::SevenZ, "code.7z").unwrap();

        assert!(result.contains("=== ARSIP: code.7z (7Z, Total 1 Entri) ==="));
        assert!(result.contains("hello.rs"));
        assert!(result.contains("--- BERKAS: hello.rs ---"));
        assert!(result.contains("Halo Xiao"));
    }
}
