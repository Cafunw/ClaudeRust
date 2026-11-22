//! File attachments: type detection, image downscaling, docx/xlsx/pptx text
//! extraction, PDF limits, and conversion to Messages API content blocks.

use crate::error::{AppError, AppResult};
use crate::util::new_id;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const MAX_IMAGE_SIDE: u32 = 2000;
const MAX_IMAGE_BYTES: usize = 3_700_000;
/// The API takes 32 MB a request, and a PDF goes base64-encoded (a third
/// larger) with the rest of the message.
const MAX_PDF_BYTES: usize = 22 * 1024 * 1024;
/// The API's page limit for a PDF (`check_message`: 100 on the 200K-context models).
const MAX_PDF_PAGES: u32 = 600;
const MAX_PDF_PAGES_200K: u32 = 100;
/// What one message's attachments may add up to, base64-encoded: the API
/// takes 32 MB a request, the system prompt and the conversation go too.
const MAX_MESSAGE_BYTES: usize = 30 * 1024 * 1024;
/// The most inflated of a PDF's object streams while counting its pages.
const MAX_PDF_INFLATE: u64 = 64 * 1024 * 1024;
/// Text over ~1 MB is digested (see `logdigest`), so this only bounds memory.
const MAX_TEXT_BYTES: usize = 40 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// image | pdf | text
    pub kind: String,
    /// base64 for image/pdf, plain text for text
    #[serde(skip_serializing)]
    pub data: String,
    /// small data URL preview for images
    pub preview: Option<String>,
    /// Set when the text was too large and was digested:
    /// `{lines, errors, warnings}` of the original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Value>,
}

const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "csv", "tsv", "json", "jsonl", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf",
    "log", "html", "htm", "css", "scss", "less", "js", "mjs", "cjs", "jsx", "ts", "tsx", "py", "rb", "rs", "go",
    "java", "kt", "kts", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "php", "pl", "lua", "r", "sql", "sh", "bash",
    "zsh", "fish", "ps1", "bat", "cmd", "vue", "svelte", "dart", "scala", "ex", "exs", "erl", "hs", "ml", "clj",
    "tex", "rst", "adoc", "org", "env", "gitignore", "dockerfile", "makefile", "gradle", "properties", "svg", "srt",
    "vtt", "ipynb", "graphql", "proto", "tf", "nix", "zig", "jl", "m", "mm", "out", "err", "crash", "ips", "trace",
    "stacktrace", "logcat",
];

fn ext_of(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.rsplit_once('.') {
        Some((_, e)) => e.to_string(),
        None => lower,
    }
}

fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn downscale(bytes: &[u8], mime: &str) -> AppResult<(Vec<u8>, String)> {
    let img = image::load_from_memory(bytes).map_err(|e| refuse("image_unreadable", format!("Could not read image: {e}")))?;
    let (w, h) = (img.width(), img.height());
    let needs_resize = w.max(h) > MAX_IMAGE_SIDE;
    if !needs_resize && bytes.len() <= MAX_IMAGE_BYTES {
        return Ok((bytes.to_vec(), mime.to_string()));
    }
    let img = if needs_resize {
        img.resize(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = std::io::Cursor::new(Vec::new());
    if img.color().has_alpha() && mime == "image/png" {
        img.write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| AppError::msg(e.to_string()))?;
        let v = out.into_inner();
        if v.len() <= MAX_IMAGE_BYTES {
            return Ok((v, "image/png".into()));
        }
        out = std::io::Cursor::new(Vec::new());
    }
    let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
    let mut q = 88u8;
    loop {
        out.get_mut().clear();
        out.set_position(0);
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q);
        rgb.write_with_encoder(enc).map_err(|e| AppError::msg(e.to_string()))?;
        if out.get_ref().len() <= MAX_IMAGE_BYTES || q <= 40 {
            break;
        }
        q -= 12;
    }
    Ok((out.into_inner(), "image/jpeg".into()))
}

/// Make a base64 image safe to send to the API (size/dimension limits).
/// Returns `None` if it can't be decoded.
pub fn normalize_image_b64(data: &str, mime: &str) -> Option<(String, String)> {
    if data.len() <= 4_000_000 && !data.is_empty() {
        // small enough: still verify dimensions cheaply
        let bytes = B64.decode(data.trim()).ok()?;
        if let Ok((w, h)) = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?.into_dimensions() {
            if w.max(h) <= 8000 {
                return Some((data.to_string(), mime.to_string()));
            }
        }
        let (b, m) = downscale(&bytes, mime).ok()?;
        return Some((B64.encode(b), m));
    }
    let bytes = B64.decode(data.trim()).ok()?;
    let (b, m) = downscale(&bytes, mime).ok()?;
    Some((B64.encode(b), m))
}

fn thumbnail(bytes: &[u8]) -> Option<String> {
    let img = image::load_from_memory(bytes).ok()?;
    let t = img.thumbnail(240, 240);
    let mut out = std::io::Cursor::new(Vec::new());
    let rgb = image::DynamicImage::ImageRgb8(t.to_rgb8());
    rgb.write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80))
        .ok()?;
    Some(format!("data:image/jpeg;base64,{}", B64.encode(out.into_inner())))
}

/// The most of word/document.xml read from a .docx (it's compressed: a small
/// file can claim or expand to gigabytes).
const MAX_DOCX_XML_BYTES: u64 = 64 * 1024 * 1024;

/// Extract plain text from a .docx (word/document.xml).
fn docx_text(bytes: &[u8]) -> AppResult<String> {
    docx_text_limited(bytes, MAX_DOCX_XML_BYTES)
}

fn docx_text_limited(bytes: &[u8], limit: u64) -> AppResult<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| refuse("office_invalid", format!("Not a valid Word document: {e}")))?;
    let entry = zip.by_name("word/document.xml").map_err(|e| refuse("office_invalid", format!("Not a valid Word document: {e}")))?;
    let declared = entry.size();
    let xml = crate::util::read_capped(entry, declared, limit)?.ok_or_else(|| {
        refuse("office_too_large", format!("This Word document's text is too large to attach (over {} uncompressed).", crate::util::human_size(limit)))
    })?;
    let xml = String::from_utf8(xml).map_err(|_| refuse("office_damaged", "This Word document's text isn't valid UTF-8.".into()))?;
    let mut out = String::new();
    let mut i = 0;
    let b = xml.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            let Some(end) = xml[i..].find('>').map(|j| i + j) else { break };
            let tag = &xml[i + 1..end];
            if tag.starts_with("/w:p") && !tag.starts_with("/w:pPr") {
                out.push('\n');
            } else if tag.starts_with("w:tab") {
                out.push('\t');
            } else if tag.starts_with("w:br") || tag.starts_with("w:cr") {
                out.push('\n');
            }
            i = end + 1;
        } else {
            let end = xml[i..].find('<').map(|j| i + j).unwrap_or(b.len());
            out.push_str(&xml[i..end]);
            i = end;
        }
    }
    Ok(out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&"))
}

pub fn from_bytes(name: &str, bytes: &[u8]) -> AppResult<Attachment> {
    from_bytes_at(name, bytes, None)
}

/// `path` is where the file lives on disk, when known (mentioned in digests).
fn from_bytes_at(name: &str, bytes: &[u8], path: Option<&std::path::Path>) -> AppResult<Attachment> {
    let ext = ext_of(name);
    let size = bytes.len() as u64;
    if let Some(mime) = sniff_image(bytes) {
        let (data, mime) = downscale(bytes, mime)?;
        return Ok(Attachment {
            id: new_id(),
            name: name.to_string(),
            preview: thumbnail(&data),
            digest: None,
            mime,
            size,
            kind: "image".into(),
            data: B64.encode(&data),
        });
    }
    if ext == "pdf" && !bytes.starts_with(b"%PDF") {
        return Err(refuse("pdf_invalid", format!("“{name}” is not a valid PDF file.")));
    }
    if bytes.starts_with(b"%PDF") {
        if bytes.len() > MAX_PDF_BYTES {
            let size = crate::util::human_size(size);
            return Err(refuse("pdf_too_large", format!("“{name}” is {size}; a PDF can be at most {} MB.", MAX_PDF_BYTES >> 20)));
        }
        if let Some(pages) = pdf_pages(bytes, MAX_PDF_INFLATE).filter(|&p| p > MAX_PDF_PAGES) {
            return Err(refuse(
                "pdf_too_many_pages",
                format!("“{name}” has {pages} pages; Claude reads PDFs of up to {MAX_PDF_PAGES} pages (100 with the 200K-context models)."),
            ));
        }
        return Ok(Attachment {
            id: new_id(),
            name: name.to_string(),
            mime: "application/pdf".into(),
            size,
            kind: "pdf".into(),
            data: B64.encode(bytes),
            preview: None,
            digest: None,
        });
    }
    let office = match ext.as_str() {
        "docx" => Some(docx_text(bytes)),
        "xlsx" => Some(crate::office::xlsx_text(bytes)),
        "pptx" => Some(crate::office::pptx_text(bytes)),
        _ => None,
    };
    if let Some(text) = office {
        return Ok(text_attachment(name, "text/plain".into(), size, text?, path));
    }
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(refuse("file_too_large", "This file is too large to attach (40 MB maximum).".into()));
    }
    let text_ext = TEXT_EXTS.contains(&ext.as_str());
    let (text, mime) = match decode_text_file(bytes, text_ext) {
        // a text type is kept even when binary-looking: logs cut off by a crash end in NULs
        Some((text, _)) if text_ext || !looks_binary(&text) => (text, mime_guess::from_path(name).first_or_text_plain().essence_str().to_string()),
        None if text_ext => (String::from_utf8_lossy(bytes).to_string(), "text/plain".to_string()),
        _ => {
            return Err(refuse(
                "unsupported_file",
                format!("“{name}” isn't a supported file type. You can attach images, PDFs, Word, Excel and PowerPoint files, and text files."),
            ))
        }
    };
    Ok(text_attachment(name, mime, size, text, path))
}

/// Refuse a message to `model` the API would refuse for its attachments
/// together: over `MAX_MESSAGE_BYTES`, or a PDF of more pages than the
/// model reads. Each one alone passed `from_bytes`.
// shortcut: the conversation's earlier attachments are sent again with each turn and aren't counted; count them if 413s show up.
pub fn check_message(atts: &[Attachment], model: &str) -> AppResult<()> {
    let total: usize = atts.iter().map(|a| a.data.len()).sum();
    if total > MAX_MESSAGE_BYTES {
        let size = crate::util::human_size(total as u64);
        return Err(refuse("message_too_large", format!("This message's attachments add up to {size}; one message can carry about {} MB. Send them in separate messages.", MAX_MESSAGE_BYTES >> 20)));
    }
    if crate::models::info(model).context > 200_000 {
        return Ok(());
    }
    for a in atts.iter().filter(|a| a.kind == "pdf") {
        let pages = B64.decode(&a.data).ok().and_then(|pdf| pdf_pages(&pdf, MAX_PDF_INFLATE));
        if let Some(pages) = pages.filter(|&p| p > MAX_PDF_PAGES_200K) {
            return Err(refuse(
                "pdf_too_many_pages_for_model",
                format!("“{}” has {pages} pages; this model reads PDFs of up to {MAX_PDF_PAGES_200K} pages. Choose a newer model, or attach fewer pages.", a.name),
            ));
        }
    }
    Ok(())
}

/// A refusal the UI can tell apart by its `kind` (and show in the person's language).
pub(crate) fn refuse(kind: &str, message: String) -> AppError {
    AppError::Api { status: 0, kind: kind.into(), message }
}

/// Binary data that happens to be valid UTF-8 (a header padded with NULs):
/// a NUL in the first 8000 bytes, as git decides, or many other control
/// characters there.
fn looks_binary(text: &str) -> bool {
    let head = &text.as_bytes()[..text.len().min(8000)];
    let control = head.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b)).count();
    head.contains(&0) || control * 10 > head.len()
}

/// Pages of a PDF from its page tree: the largest /Count of a /Type /Pages
/// node (the root counts every page), in the file and in its compressed
/// object streams, inflating at most `budget` bytes of those. None when no
/// page tree is found (encrypted, damaged, another compression).
// shortcut: a page tree an incremental update shrank still counts its old size; parse the xref if that refuses real files.
fn pdf_pages(pdf: &[u8], mut budget: u64) -> Option<u32> {
    use std::io::Read;
    let mut nodes = MAX_PAGE_NODES;
    let mut most = page_tree_count(pdf, &mut nodes);
    let mut at = 0;
    while budget > 0 {
        let Some(i) = find(&pdf[at..], b"/ObjStm") else { break };
        at += i;
        let Some(s) = find(&pdf[at..], b"stream") else { break };
        let start = at + s + 6;
        let Some(len) = find(&pdf[start..], b"endstream") else { break };
        at = start + len;
        let data = &pdf[start..at];
        let data = data.strip_prefix(b"\r\n").or_else(|| data.strip_prefix(b"\n")).unwrap_or(data);
        let mut out = Vec::new();
        // a damaged stream still gives what inflated before the damage
        let _ = flate2::read::ZlibDecoder::new(data).take(budget).read_to_end(&mut out);
        budget -= out.len() as u64;
        most = most.max(page_tree_count(&out, &mut nodes));
    }
    most
}

/// How many page tree nodes are looked at, and how far around each one for
/// its dictionary: bounds the work on a file made to look like many.
const MAX_PAGE_NODES: usize = 4096;
const PAGE_NODE_REACH: usize = 64 * 1024;

fn page_tree_count(pdf: &[u8], nodes: &mut usize) -> Option<u32> {
    let mut most = None;
    let mut at = 0;
    while let Some(i) = find(&pdf[at..], b"/Pages") {
        let p = at + i;
        at = p + 6;
        // "/Type /Pages", not the catalog's /Pages key or a longer name
        if pdf[..p].trim_ascii_end().ends_with(b"/Type") && !pdf.get(at).is_some_and(u8::is_ascii_alphanumeric) {
            if *nodes == 0 {
                break;
            }
            *nodes -= 1;
            most = most.max(dict_count(pdf, p));
        }
    }
    most
}

/// The /Count of the dictionary around `at`, at that dictionary's own level.
fn dict_count(pdf: &[u8], at: usize) -> Option<u32> {
    let mut depth = 0;
    let mut i = at;
    let floor = at.saturating_sub(PAGE_NODE_REACH);
    loop {
        if i == floor {
            return None;
        }
        i -= 1;
        match &pdf[i..i + 2] {
            b">>" => depth += 1,
            b"<<" if depth == 0 => break,
            b"<<" => depth -= 1,
            _ => {}
        }
    }
    i += 2;
    let end = (at + PAGE_NODE_REACH).min(pdf.len());
    while i < end {
        let rest = &pdf[i..];
        if rest.starts_with(b"<<") {
            depth += 1;
            i += 2;
        } else if rest.starts_with(b">>") {
            if depth == 0 {
                return None;
            }
            depth -= 1;
            i += 2;
        } else if depth == 0 && rest.starts_with(b"/Count") {
            let n = rest[6..].trim_ascii_start();
            let digits = n.iter().take_while(|c| c.is_ascii_digit()).count();
            return std::str::from_utf8(&n[..digits]).ok()?.parse().ok();
        } else {
            i += 1;
        }
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The text of a file: UTF-8 (a byte-order mark is dropped), or UTF-16 with
/// a byte-order mark as Windows tools write it (PowerShell's `>`, Notepad's
/// "Unicode") — that one only for text file types, where an invalid
/// character is replaced rather than the file refused. `None` otherwise.
fn decode_text_file(bytes: &[u8], text_ext: bool) -> Option<(String, crate::util::TextEncoding)> {
    use crate::util::TextEncoding;
    let utf16 = bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]);
    if utf16 && !text_ext {
        return None;
    }
    if let Some(d) = crate::util::decode_text(bytes) {
        return Some(d);
    }
    if !utf16 {
        return None;
    }
    // damaged UTF-16 (odd length, unpaired surrogates): still UTF-16
    let be = bytes[0] == 0xFE;
    let units: Vec<u16> = bytes[2..].chunks(2).map(|c| {
        let pair = [c[0], *c.get(1).unwrap_or(&0)];
        if be { u16::from_be_bytes(pair) } else { u16::from_le_bytes(pair) }
    }).collect();
    Some((String::from_utf16_lossy(&units), if be { TextEncoding::Utf16Be } else { TextEncoding::Utf16Le }))
}

/// A text attachment; text too large for the context window becomes a
/// digest (errors grouped, context around them, start and end).
fn text_attachment(name: &str, mime: String, size: u64, text: String, path: Option<&std::path::Path>) -> Attachment {
    let (data, digest) = if crate::logdigest::should_digest(name, &text) {
        let d = crate::logdigest::digest(name, &text, path);
        (d.text, Some(json!({"lines": d.lines, "errors": d.errors, "warnings": d.warnings})))
    } else {
        (text, None)
    };
    Attachment {
        id: new_id(),
        name: name.to_string(),
        mime,
        size,
        kind: "text".into(),
        data,
        preview: None,
        digest,
    }
}

pub fn from_path(path: &std::path::Path) -> AppResult<Attachment> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(refuse("not_a_file", format!("{} is not a regular file.", path.display())));
    }
    if meta.len() > 40 * 1024 * 1024 {
        return Err(refuse("file_too_large", "This file is too large to attach (40 MB maximum).".into()));
    }
    let bytes = std::fs::read(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    from_bytes_at(&name, &bytes, Some(path))
}

pub fn from_base64(name: &str, b64: &str) -> AppResult<Attachment> {
    let bytes = B64
        .decode(b64.trim())
        .map_err(|e| AppError::msg(format!("bad data: {e}")))?;
    from_bytes(name, &bytes)
}

/// Messages API content block for an attachment.
pub fn to_block(a: &Attachment) -> Value {
    match a.kind.as_str() {
        "image" => json!({
            "type": "image",
            "source": {"type": "base64", "media_type": a.mime, "data": a.data}
        }),
        "pdf" => json!({
            "type": "document",
            "source": {"type": "base64", "media_type": "application/pdf", "data": a.data},
            "title": a.name
        }),
        _ => json!({
            "type": "document",
            "source": {"type": "text", "media_type": "text/plain", "data": if a.data.is_empty() { " ".to_string() } else { a.data.clone() }},
            "title": a.name
        }),
    }
}

/// Display metadata stored with the message.
pub fn display_meta(a: &Attachment) -> Value {
    json!({"name": a.name, "kind": a.kind, "mime": a.mime, "size": a.size, "preview": a.preview, "digest": a.digest})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detect_types() {
        let a = from_bytes("notes.md", b"# hello").unwrap();
        assert_eq!(a.kind, "text");
        assert_eq!(to_block(&a)["source"]["type"], "text");
        let a = from_bytes("x.pdf", b"%PDF-1.4 ...").unwrap();
        assert_eq!(a.kind, "pdf");
        assert!(from_bytes("x.bin", &[0xff, 0xfe, 0x00, 0x81]).is_err());
        // tiny png
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(3000, 10)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        // a large log is attached as a digest
        let log: String = (0..30_000).map(|i| format!("2026-11-20 01:00:00 INFO tick {i}\n")).collect::<String>() + "2026-11-20 01:00:01 ERROR disk full\n";
        let a = from_bytes("app.log", log.as_bytes()).unwrap();
        assert!(a.data.len() < log.len() / 4);
        assert!(a.data.contains("disk full"));
        assert_eq!(a.size, log.len() as u64);
        assert_eq!(a.digest.as_ref().unwrap()["errors"], 1);
        assert_eq!(display_meta(&a)["digest"]["lines"], 30_001);
        let a = from_bytes("small.log", b"2026-11-20 01:00:01 ERROR x\n").unwrap();
        assert!(a.digest.is_none());
        let a = from_bytes("big.png", png.get_ref()).unwrap();
        assert_eq!(a.kind, "image");
        assert!(a.preview.is_some());
    }

    /// How long making an attachment takes (what `stage_bytes` and
    /// `stage_paths` held a runtime worker for):
    /// `cargo test --release -p clauderust --lib bench_attachments -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_attachments() {
        // a 12-megapixel photo-like picture (noise compresses poorly)
        let mut seed = 1u32;
        let img = image::RgbImage::from_fn(4000, 3000, |x, y| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            image::Rgb([(x % 256) as u8, (y % 256) as u8, (seed >> 24) as u8])
        });
        let mut jpg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut jpg, image::ImageFormat::Jpeg).unwrap();
        let jpg = jpg.into_inner();
        let t = std::time::Instant::now();
        let a = from_bytes("photo.jpg", &jpg).unwrap();
        println!("12 MP JPEG ({:.1} MB): {:?}, {} bytes of base64", jpg.len() as f64 / 1e6, t.elapsed(), a.data.len());
        let log: String = (0..200_000).map(|i| format!("2026-11-20 01:00:00 INFO request {i} served in 3 ms\n")).collect();
        let t = std::time::Instant::now();
        let a = from_bytes("server.log", log.as_bytes()).unwrap();
        println!("{:.1} MB log: {:?} (digested: {})", log.len() as f64 / 1e6, t.elapsed(), a.digest.is_some());
    }

    #[test]
    fn utf16_text_files_are_decoded() {
        let text = "name,city\nAyşe,İzmir\n";
        let le = crate::util::encode_text(text, crate::util::TextEncoding::Utf16Le);
        let a = from_bytes("data.csv", &le).unwrap();
        assert_eq!(a.data, text, "not NUL-interleaved");
        let be = crate::util::encode_text(text, crate::util::TextEncoding::Utf16Be);
        assert_eq!(from_bytes("notes.txt", &be).unwrap().data, text);
        // a UTF-8 byte-order mark is dropped
        assert_eq!(from_bytes("a.log", b"\xEF\xBB\xBFhello").unwrap().data, "hello");
        // damaged UTF-16 is still read as UTF-16
        let mut odd = le.clone();
        odd.push(b'x');
        assert!(from_bytes("data.csv", &odd).unwrap().data.starts_with("name,city"));
    }

    fn docx(xml: &str) -> Vec<u8> {
        crate::office::zip_of(&[("word/document.xml", xml)])
    }

    #[test]
    fn docx_bombs_are_refused() {
        let ok = docx("<w:document><w:body><w:p><w:r><w:t>Hello &amp; bye</w:t></w:r></w:p></w:body></w:document>");
        assert_eq!(from_bytes("a.docx", &ok).unwrap().data, "Hello & bye\n");
        // expands past the limit: refused, with a clear message
        let big = docx(&" ".repeat(4096));
        let err = docx_text_limited(&big, 1024).unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
        // a header that understates the size doesn't get past the limit
        let liar = crate::office::lie_about_size(big, 10);
        assert!(docx_text_limited(&liar, 1024).is_err());
    }

    /// What a file is attached as (kind, mime), or the kind of its refusal.
    fn kind_of(name: &str, bytes: &[u8]) -> Result<(String, String), String> {
        from_bytes(name, bytes).map(|a| (a.kind, a.mime)).map_err(|e| e.kind().to_string())
    }

    fn ok(kind: &str, mime: &str) -> Result<(String, String), String> {
        Ok((kind.to_string(), mime.to_string()))
    }

    /// Every type a file can be attached as, and the refusals.
    #[test]
    fn attachment_type_matrix() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        let jpg = [0xFF, 0xD8, 0xFF, 0xE0];
        let xlsx = crate::office::xlsx_of(&[("S", "<row><c><v>1</v></c></row>")], &[]);
        let pptx = crate::office::pptx_of(&[&["Hi"]]);
        let docx = docx("<w:p><w:t>Hi</w:t></w:p>");
        let utf16 = crate::util::encode_text("a,b\n", crate::util::TextEncoding::Utf16Le);
        let heic = [&[0, 0, 0, 0x18][..], b"ftypheic", &[0; 64]].concat();
        assert_eq!(kind_of("a.png", &png), ok("image", "image/png"));
        // the content decides, not the name
        assert_eq!(kind_of("photo.txt", &png), ok("image", "image/png"));
        assert_eq!(kind_of("a.pdf", b"%PDF-1.7\n"), ok("pdf", "application/pdf"));
        assert_eq!(kind_of("a.docx", &docx), ok("text", "text/plain"));
        assert_eq!(kind_of("a.xlsx", &xlsx), ok("text", "text/plain"));
        assert_eq!(kind_of("a.pptx", &pptx), ok("text", "text/plain"));
        assert_eq!(from_bytes("a.xlsx", &xlsx).unwrap().data, "## Sheet: S\n1");
        assert_eq!(from_bytes("a.pptx", &pptx).unwrap().data, "## Slide 1\nHi");
        assert_eq!(kind_of("a.csv", b"a,b\n1,2\n"), ok("text", "text/csv"));
        assert_eq!(kind_of("a.csv", &utf16), ok("text", "text/csv"));
        assert_eq!(kind_of("notes.md", "héllo".as_bytes()), ok("text", "text/markdown"));
        assert_eq!(kind_of("README", b"plain words"), ok("text", "text/plain"));
        // a crash-truncated log keeps its NULs and is still a log
        assert_eq!(kind_of("app.log", b"started\n\0\0\0\0"), ok("text", "text/plain"));
        // binary that is valid UTF-8, or other binary: refused as unsupported
        assert_eq!(kind_of("photo.heic", &heic), Err("unsupported_file".into()));
        assert_eq!(kind_of("a.bin", &[1, 2, 3, 4, 5, 6, 7, 8, b'a']), Err("unsupported_file".into()));
        assert_eq!(kind_of("a.exe", &[0x4D, 0x5A, 0x90, 0x00, 0xFF]), Err("unsupported_file".into()));
        assert_eq!(kind_of("a.xls", &[0xD0, 0xCF, 0x11, 0xE0, 0xA1]), Err("unsupported_file".into()));
        assert_eq!(kind_of("data.dat", &utf16), Err("unsupported_file".into()));
        assert!(from_bytes("photo.heic", &heic).unwrap_err().to_string().contains("Excel and PowerPoint"));
        // a JPEG that doesn't decode, a damaged workbook, a fake PDF
        assert_eq!(kind_of("a.jpg", &jpg), Err("image_unreadable".into()));
        assert_eq!(kind_of("a.xlsx", b"PK\x03\x04"), Err("office_invalid".into()));
        assert_eq!(kind_of("a.docx", b"PK\x03\x04"), Err("office_invalid".into()));
        assert_eq!(kind_of("a.pdf", b"<html>"), Err("pdf_invalid".into()));
        assert_eq!(kind_of("a.txt", &b"a".repeat(MAX_TEXT_BYTES + 1)), Err("file_too_large".into()));
        assert_eq!(from_path(&std::env::temp_dir()).unwrap_err().kind(), "not_a_file");
    }

    #[test]
    fn a_message_is_refused_for_what_its_attachments_add_up_to() {
        let pdf_of = |pages: u32| from_bytes("a.pdf", pdf(pages).as_bytes()).unwrap();
        let (short, long) = (pdf_of(100), pdf_of(101));
        assert!(check_message(&[short.clone(), long.clone()], "claude-opus-5-5").is_ok(), "1M context: up to 600 pages");
        assert!(check_message(std::slice::from_ref(&short), "claude-haiku-4-5").is_ok());
        let e = check_message(&[short.clone(), long.clone()], "claude-haiku-4-5").unwrap_err();
        assert_eq!(e.kind(), "pdf_too_many_pages_for_model");
        assert!(e.to_string().contains("101 pages") && e.to_string().contains("100"), "{e}");
        // two PDFs that each fit, but not in one request
        let mut big = pdf(1).into_bytes();
        big.resize(MAX_PDF_BYTES, b' ');
        let big = from_bytes("big.pdf", &big).unwrap();
        assert!(check_message(std::slice::from_ref(&big), "claude-opus-5-5").is_ok());
        assert_eq!(check_message(&[big.clone(), big], "claude-opus-5-5").unwrap_err().kind(), "message_too_large");
        assert!(check_message(&[], "claude-haiku-4-5").is_ok());
    }

    #[test]
    fn binary_sniffing() {
        assert!(looks_binary("ab\0cd"));
        assert!(looks_binary(&"\x01\x02\x03 ".repeat(10)));
        assert!(!looks_binary("tab\tnew\r\nline \x1b[31mred\x1b[0m page\x0c"));
        assert!(!looks_binary(""));
        // only the head is looked at
        assert!(!looks_binary(&("a".repeat(8000) + "\0")));
    }

    /// A PDF whose page tree root says `count`, with an outline and a
    /// nested dictionary to look past.
    fn pdf(count: u32) -> String {
        format!(
            "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Resources << /Font << /F1 5 0 R >> /Count 9999 >> /Kids [3 0 R] /Count {count} >> endobj\n\
             3 0 obj <</Type/Page/Parent 2 0 R>> endobj\n4 0 obj << /Type /Outlines /Count 9000 >> endobj\n%%EOF"
        )
    }

    /// A PDF 1.5 whose objects, page tree too, are in a compressed object stream.
    fn pdf_objstm(count: u32) -> Vec<u8> {
        use std::io::Write;
        let filler: String = (0..300).map(|i| format!(" {i}")).collect();
        let objects = format!("2 0 3 40 << /Type /Pages /Kids [3 0 R] /Count {count} >> << /Type /Page /Parent 2 0 R /X [{filler}] >>");
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(objects.as_bytes()).unwrap();
        let data = z.finish().unwrap();
        let head = format!("%PDF-1.5\n7 0 obj << /Type /ObjStm /N 2 /First 8 /Length {} /Filter /FlateDecode >>\nstream\r\n", data.len());
        [head.as_bytes(), &data, b"\r\nendstream endobj\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n%%EOF"].concat()
    }

    #[test]
    fn pdf_page_counts() {
        assert_eq!(pdf_pages(pdf(12).as_bytes(), MAX_PDF_INFLATE), Some(12));
        assert_eq!(pdf_pages(&pdf_objstm(700), MAX_PDF_INFLATE), Some(700));
        assert_eq!(pdf_pages(b"%PDF-1.4 nothing here", MAX_PDF_INFLATE), None);
        // an intermediate node counts less than the root
        let two = pdf(5) + "\n8 0 obj << /Type /Pages /Parent 2 0 R /Count 3 >> endobj";
        assert_eq!(pdf_pages(two.as_bytes(), MAX_PDF_INFLATE), Some(5));
        // a damaged object stream is still read up to the damage, and the
        // inflating stops at the budget
        let mut cut = pdf_objstm(42);
        let end = find(&cut, b"\r\nendstream").unwrap();
        cut.drain(end - 6..end);
        assert_eq!(pdf_pages(&cut, MAX_PDF_INFLATE), Some(42));
        assert_eq!(pdf_pages(&pdf_objstm(42), 10), None);
        // the work is bounded on files made to look like many nodes or streams
        let nodes: String = (1..=10).map(|n| format!("<< /Type /Pages /Count {n} >>")).collect();
        assert_eq!(page_tree_count(nodes.as_bytes(), &mut 3), Some(3));
        let far = format!("<< /Kids [{}] /Type /Pages /Count 5 >>", " ".repeat(PAGE_NODE_REACH));
        assert_eq!(page_tree_count(far.as_bytes(), &mut 10), None);
        assert_eq!(pdf_pages("/ObjStm stream ".repeat(100_000).as_bytes(), MAX_PDF_INFLATE), None);
    }

    #[test]
    fn pdfs_over_the_api_limits_are_refused_early() {
        assert_eq!(kind_of("ok.pdf", pdf(600).as_bytes()), ok("pdf", "application/pdf"));
        assert_eq!(kind_of("long.pdf", pdf(601).as_bytes()), Err("pdf_too_many_pages".into()));
        assert_eq!(kind_of("long.pdf", &pdf_objstm(900)), Err("pdf_too_many_pages".into()));
        let err = from_bytes("long.pdf", pdf(601).as_bytes()).unwrap_err().to_string();
        assert!(err.contains("601 pages") && err.contains("600"), "{err}");
        let mut big = pdf(1).into_bytes();
        big.resize(MAX_PDF_BYTES + 1, b' ');
        let err = from_bytes("big.pdf", &big).unwrap_err();
        assert_eq!(err.kind(), "pdf_too_large");
        assert!(err.to_string().contains("at most 22 MB"), "{err}");
        // what the API gets fits its 32 MB request limit
        big.truncate(MAX_PDF_BYTES);
        assert!(to_block(&from_bytes("big.pdf", &big).unwrap())["source"]["data"].as_str().unwrap().len() < 31_000_000);
    }
}
