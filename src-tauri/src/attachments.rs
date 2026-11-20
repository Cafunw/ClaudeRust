//! File attachments: type detection, image downscaling, docx text
//! extraction, and conversion to Messages API content blocks.

use crate::error::{AppError, AppResult};
use crate::util::new_id;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const MAX_IMAGE_SIDE: u32 = 2000;
const MAX_IMAGE_BYTES: usize = 3_700_000;
const MAX_PDF_BYTES: usize = 32 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

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
}

const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "csv", "tsv", "json", "jsonl", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf",
    "log", "html", "htm", "css", "scss", "less", "js", "mjs", "cjs", "jsx", "ts", "tsx", "py", "rb", "rs", "go",
    "java", "kt", "kts", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "php", "pl", "lua", "r", "sql", "sh", "bash",
    "zsh", "fish", "ps1", "bat", "cmd", "vue", "svelte", "dart", "scala", "ex", "exs", "erl", "hs", "ml", "clj",
    "tex", "rst", "adoc", "org", "env", "gitignore", "dockerfile", "makefile", "gradle", "properties", "svg", "srt",
    "vtt", "ipynb", "graphql", "proto", "tf", "nix", "zig", "jl", "m", "mm",
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
    let img = image::load_from_memory(bytes).map_err(|e| AppError::msg(format!("Could not read image: {e}")))?;
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
    if data.len() <= 4_000_000 && data.len() > 0 {
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

/// Extract plain text from a .docx (word/document.xml).
fn docx_text(bytes: &[u8]) -> AppResult<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| AppError::msg(e.to_string()))?;
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .map_err(|e| AppError::msg(e.to_string()))?
        .read_to_string(&mut xml)?;
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
    let ext = ext_of(name);
    let size = bytes.len() as u64;
    if let Some(mime) = sniff_image(bytes) {
        let (data, mime) = downscale(bytes, mime)?;
        return Ok(Attachment {
            id: new_id(),
            name: name.to_string(),
            preview: thumbnail(&data),
            mime,
            size,
            kind: "image".into(),
            data: B64.encode(&data),
        });
    }
    if ext == "pdf" && !bytes.starts_with(b"%PDF") {
        return Err(AppError::msg(format!("“{name}” is not a valid PDF file.")));
    }
    if bytes.starts_with(b"%PDF") {
        if bytes.len() > MAX_PDF_BYTES {
            return Err(AppError::msg("PDF files must be smaller than 32 MB."));
        }
        return Ok(Attachment {
            id: new_id(),
            name: name.to_string(),
            mime: "application/pdf".into(),
            size,
            kind: "pdf".into(),
            data: B64.encode(bytes),
            preview: None,
        });
    }
    if ext == "docx" {
        let text = docx_text(bytes)?;
        return Ok(Attachment {
            id: new_id(),
            name: name.to_string(),
            mime: "text/plain".into(),
            size,
            kind: "text".into(),
            data: text,
            preview: None,
        });
    }
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(AppError::msg("This file is too large to attach."));
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(Attachment {
            id: new_id(),
            name: name.to_string(),
            mime: mime_guess::from_path(name).first_or_text_plain().essence_str().to_string(),
            size,
            kind: "text".into(),
            data: s.to_string(),
            preview: None,
        }),
        Err(_) if TEXT_EXTS.contains(&ext.as_str()) => Ok(Attachment {
            id: new_id(),
            name: name.to_string(),
            mime: "text/plain".into(),
            size,
            kind: "text".into(),
            data: String::from_utf8_lossy(bytes).to_string(),
            preview: None,
        }),
        Err(_) => Err(AppError::msg(format!(
            "“{name}” isn't a supported file type. You can attach images, PDFs, Word documents and text files."
        ))),
    }
}

pub fn from_path(path: &std::path::Path) -> AppResult<Attachment> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(AppError::msg(format!("{} is not a regular file.", path.display())));
    }
    if meta.len() > 40 * 1024 * 1024 {
        return Err(AppError::msg("This file is too large to attach (40 MB maximum)."));
    }
    let bytes = std::fs::read(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    from_bytes(&name, &bytes)
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
    json!({"name": a.name, "kind": a.kind, "mime": a.mime, "size": a.size, "preview": a.preview})
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
        let a = from_bytes("big.png", png.get_ref()).unwrap();
        assert_eq!(a.kind, "image");
        assert!(a.preview.is_some());
    }
}
