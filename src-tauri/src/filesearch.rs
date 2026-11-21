//! Fast fuzzy file search for `@` mentions, and expansion of mentions into
//! file contents when a message is sent.
//!
//! The file list of a folder (respecting .gitignore) is cached for a few
//! seconds so typing doesn't rescan the tree on every keystroke.

use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const MAX_FILES: usize = 60_000;
const CACHE_TTL: Duration = Duration::from_secs(8);

/// Relative path and whether it is a directory.
type FileList = std::sync::Arc<Vec<(String, bool)>>;

static CACHE: LazyLock<Mutex<HashMap<PathBuf, (Instant, FileList)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Relative paths under `root` (files and directories), .gitignore-aware.
static SCAN: Mutex<()> = parking_lot::const_mutex(());

pub fn list(root: &Path) -> FileList {
    if let Some((t, v)) = CACHE.lock().get(root) {
        if t.elapsed() < CACHE_TTL {
            return v.clone();
        }
    }
    // one scan at a time; callers waiting on it reuse its result
    let _scan = SCAN.lock();
    if let Some((t, v)) = CACHE.lock().get(root) {
        if t.elapsed() < CACHE_TTL {
            return v.clone();
        }
    }
    let mut out = vec![];
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(n == ".git" || n == "node_modules" || n == "target" || n == ".DS_Store")
        })
        .build();
    for e in walker.flatten() {
        if e.depth() == 0 {
            continue;
        }
        let Ok(rel) = e.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push((rel, is_dir));
        if out.len() >= MAX_FILES {
            break;
        }
    }
    let v = std::sync::Arc::new(out);
    let mut c = CACHE.lock();
    if c.len() > 16 {
        c.clear();
    }
    c.insert(root.to_path_buf(), (Instant::now(), v.clone()));
    v
}

/// Fuzzy score of `query` against `path` (higher is better), or None if the
/// query's characters don't appear in order.
pub fn score(query: &str, path: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(-(path.len() as i64));
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    // lower-case per character so both vectors have the same length (some
    // characters, like İ, lower-case to two)
    let p: Vec<char> = path.chars().collect();
    let pl: Vec<char> = p.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let base_start = path.rfind('/').map(|i| path[..i + 1].chars().count()).unwrap_or(0);
    let mut score: i64 = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    for (i, c) in pl.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            let mut s = 1;
            if prev == Some(i.wrapping_sub(1)) {
                s += 6; // consecutive
            }
            let boundary = i == 0 || matches!(p[i - 1], '/' | '_' | '-' | '.' | ' ') || (p[i].is_uppercase() && p[i - 1].is_lowercase());
            if boundary {
                s += 4;
            }
            if i >= base_start {
                s += 3; // in the file name
            }
            score += s;
            prev = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    let name = &path[path.rfind('/').map(|i| i + 1).unwrap_or(0)..];
    if name.to_lowercase().starts_with(&query.to_lowercase()) {
        score += 15;
    }
    if name.eq_ignore_ascii_case(query) {
        score += 25;
    }
    Some(score * 100 - path.len() as i64)
}

pub fn search(root: &Path, query: &str, limit: usize) -> Vec<Value> {
    let files = list(root);
    let mut hits: Vec<(i64, &String, bool)> = files
        .iter()
        .filter_map(|(p, d)| score(query, p).map(|s| (s, p, *d)))
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter()
        .take(limit)
        .map(|(_, p, d)| json!({"path": p, "dir": d}))
        .collect()
}

/// `@path` / `@"path with spaces"` tokens in a message.
pub fn mentions(text: &str) -> Vec<String> {
    let mut out = vec![];
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(') {
            let mut j = i + 1;
            let token: String;
            if j < chars.len() && chars[j] == '"' {
                j += 1;
                let start = j;
                while j < chars.len() && chars[j] != '"' {
                    j += 1;
                }
                token = chars[start..j].iter().collect();
                j += 1;
            } else {
                let start = j;
                while j < chars.len() && !chars[j].is_whitespace() && !matches!(chars[j], ',' | ')' | ';' | '`') {
                    j += 1;
                }
                token = chars[start..j].iter().collect::<String>().trim_end_matches(['.', ':', '!', '?']).to_string();
            }
            if !token.is_empty() && !out.contains(&token) {
                out.push(token);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Text blocks (hidden reminders) with the content of files the message
/// mentions; mentioned files count as read for the session.
pub fn expand(text: &str, session: &crate::codetools::CodeSession) -> Vec<Value> {
    let mut blocks = vec![];
    let mut total = 0usize;
    for m in mentions(text).into_iter().filter(|m| !m.contains("://")).take(10) {
        // `path#L120` mentions a symbol: include the code around that line
        let (m, line) = match m.rsplit_once("#L") {
            Some((p, n)) if n.parse::<usize>().is_ok() => (p.to_string(), n.parse::<usize>().ok()),
            _ => (m, None),
        };
        let p = {
            let pb = PathBuf::from(&m);
            if pb.is_absolute() {
                pb
            } else {
                session.root.join(&m)
            }
        };
        let Ok(meta) = std::fs::metadata(&p) else { continue };
        if meta.is_dir() {
            let mut names: Vec<String> = std::fs::read_dir(&p)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| {
                            let n = e.file_name().to_string_lossy().to_string();
                            if e.path().is_dir() {
                                format!("{n}/")
                            } else {
                                n
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names.truncate(200);
            blocks.push(json!({"type": "text", "text": format!(
                "<system-reminder>The user mentioned the directory {}. Its entries:\n{}</system-reminder>", p.display(), names.join("\n"))}));
            continue;
        }
        if meta.len() > 256 * 1024 || total > 600 * 1024 {
            blocks.push(json!({"type": "text", "text": format!(
                "<system-reminder>The user mentioned {} ({} bytes); it is too large to include, read the parts you need.</system-reminder>", p.display(), meta.len())}));
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        if bytes.iter().take(8000).any(|&b| b == 0) {
            continue;
        }
        let content = String::from_utf8_lossy(&bytes);
        if let Some(line) = line {
            let from = line.saturating_sub(10).max(1);
            let to = line + 80;
            let numbered: String = content
                .lines()
                .enumerate()
                .filter(|(i, _)| *i + 1 >= from && *i < to)
                .map(|(i, l)| format!("{:>6}\t{l}\n", i + 1))
                .collect();
            total += numbered.len();
            blocks.push(json!({"type": "text", "text": format!(
                "<system-reminder>The user mentioned the code at {}:{line}. Lines {from}-{to}:\n{}</system-reminder>", p.display(), numbered)}));
            continue;
        }
        total += bytes.len();
        let numbered: String = content
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>6}\t{l}\n", i + 1))
            .collect();
        session.read_files.lock().insert(p.clone());
        blocks.push(json!({"type": "text", "text": format!(
            "<system-reminder>The user mentioned {}. Its current contents (already read; you can edit it directly):\n{}</system-reminder>", p.display(), numbered)}));
    }
    blocks
}

/// `@https://…` mentions in a message.
pub fn url_mentions(text: &str) -> Vec<String> {
    mentions(text).into_iter().filter(|m| m.starts_with("http://") || m.starts_with("https://")).take(3).collect()
}

/// Fetch a mentioned web page as text (hidden reminder block).
pub async fn fetch_url_block(url: &str) -> Option<serde_json::Value> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).user_agent("ClaudeRust").build().ok()?;
    let body = match client.get(url).send().await {
        Ok(r) => {
            let status = r.status();
            let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let bytes = r.bytes().await.ok()?;
            let bytes = &bytes[..bytes.len().min(3_000_000)];
            let text = if ct.contains("html") || ct.is_empty() {
                html2text::from_read(bytes, 100).unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string())
            } else if ct.starts_with("text/") || ct.contains("json") || ct.contains("xml") || ct.contains("javascript") {
                String::from_utf8_lossy(bytes).to_string()
            } else {
                format!("(binary content: {ct}, {} bytes)", bytes.len())
            };
            if status.is_success() {
                crate::util::truncate_middle(text.trim(), 100_000)
            } else {
                format!("(the page answered {status})\n{}", crate::util::truncate_middle(text.trim(), 4_000))
            }
        }
        Err(e) => format!("(could not fetch the page: {e})"),
    };
    Some(json!({"type": "text", "text": format!("<system-reminder>The user mentioned {url}. Its content:\n{body}</system-reminder>")}))
}

// ------------------------------------------------------------ file browser

/// Entries directly inside `dir` (relative to `root`; "" = the root):
/// directories first, then files, each by name (case-insensitive).
pub fn children(root: &Path, dir: &str) -> Vec<(String, bool)> {
    let dir = dir.trim_matches('/');
    // one level at a time (the cached full list stops at MAX_FILES, so a big
    // folder early in the walk would hide later ones); never outside `root`
    if Path::new(dir).components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return vec![];
    }
    let base = if dir.is_empty() { root.to_path_buf() } else { root.join(dir) };
    let walker = ignore::WalkBuilder::new(&base)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(n == ".git" || n == "node_modules" || n == "target" || n == ".DS_Store")
        })
        .build();
    let mut out: Vec<(String, bool)> = Vec::new();
    for e in walker.flatten() {
        if e.depth() == 0 {
            continue;
        }
        let Ok(rel) = e.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        out.push((rel, e.file_type().map(|t| t.is_dir()).unwrap_or(false)));
        if out.len() >= MAX_FILES {
            break;
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    out
}

/// Largest file shown in the viewer.
const VIEW_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Lines shown (and highlighted) at most.
const VIEW_MAX_LINES: usize = 20_000;

/// A project file for the read-only viewer: highlighted HTML (or a note for
/// binary / too-large files). `rel` must stay inside `root`.
pub fn view_file(root: &Path, rel: &str) -> crate::error::AppResult<Value> {
    use crate::error::AppError;
    let root = std::fs::canonicalize(root)?;
    let path = std::fs::canonicalize(root.join(rel.trim_start_matches('/'))).map_err(|_| AppError::msg(format!("File not found: {rel}")))?;
    if !path.starts_with(&root) {
        return Err(AppError::msg("That file is outside the project folder."));
    }
    let meta = std::fs::metadata(&path)?;
    if !meta.is_file() {
        return Err(AppError::msg(format!("{rel} is not a file.")));
    }
    let rel = path.strip_prefix(&root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    let size = meta.len();
    let base = json!({"path": rel, "abs": path, "size": size});
    if size > VIEW_MAX_BYTES {
        return Ok(merge(base, json!({"too_large": true})));
    }
    let bytes = std::fs::read(&path)?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Ok(merge(base, json!({"binary": true})));
    }
    let text = String::from_utf8_lossy(&bytes);
    let total = text.lines().count();
    let shown: String = if total > VIEW_MAX_LINES {
        text.lines().take(VIEW_MAX_LINES).collect::<Vec<_>>().join("\n")
    } else {
        text.trim_end_matches('\n').to_string()
    };
    let ext = Path::new(&rel)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| Path::new(&rel).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    let html = crate::highlight::highlight(&shown, &ext);
    Ok(merge(base, json!({"lines": total, "shown": total.min(VIEW_MAX_LINES), "html": html, "lang": crate::highlight::language_label(&ext)})))
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_and_view() {
        let d = std::env::temp_dir().join(format!("cr-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src/inner")).unwrap();
        std::fs::create_dir_all(d.join("Docs")).unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
        std::fs::write(d.join("src/inner/a.rs"), "").unwrap();
        std::fs::write(d.join("b.txt"), "x").unwrap();
        std::fs::write(d.join("A.md"), "# t").unwrap();
        std::fs::write(d.join("blob.bin"), [0u8, 1, 2, 0]).unwrap();
        let names = |dir: &str| children(&d, dir).into_iter().map(|(p, isd)| format!("{p}{}", if isd { "/" } else { "" })).collect::<Vec<_>>();
        assert_eq!(names(""), vec!["Docs/", "src/", "A.md", "b.txt", "blob.bin"]);
        assert_eq!(names("src"), vec!["src/inner/", "src/main.rs"]);
        assert_eq!(names("src/inner/"), vec!["src/inner/a.rs"]);
        assert!(names("../").is_empty());
        assert!(names("/etc").is_empty());
        std::fs::write(d.join(".gitignore"), "src/inner/\n").unwrap();
        assert_eq!(names("src"), vec!["src/main.rs"], "parent .gitignore applies");

        let v = view_file(&d, "src/main.rs").unwrap();
        assert_eq!(v["lines"], 3);
        assert_eq!(v["path"], "src/main.rs");
        assert!(v["html"].as_str().unwrap().contains("println"));
        assert_eq!(view_file(&d, "blob.bin").unwrap()["binary"], true);
        assert!(view_file(&d, "../../etc/passwd").is_err() || !d.join("../../etc/passwd").exists());
        assert!(view_file(&d, "src").is_err());
        assert!(view_file(&d, "nope.rs").is_err());
        let outside = std::env::temp_dir().join(format!("cr-outside-{}.txt", std::process::id()));
        std::fs::write(&outside, "secret").unwrap();
        let rel = format!("../{}", outside.file_name().unwrap().to_string_lossy());
        assert!(view_file(&d, &rel).is_err());
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fuzzy_prefers_file_names_and_consecutive_runs() {
        let a = score("main", "src/main.rs").unwrap();
        let b = score("main", "src/domain/manifest.rs").unwrap();
        assert!(a > b, "{a} {b}");
        assert!(score("xyz", "src/main.rs").is_none());
        assert!(score("smr", "src/main.rs").is_some());
        assert!(score("rs", "src/İzmir.rs").is_some());
        assert!(score("izm", "src/İzmir.rs").is_some());
        let exact = score("lib.rs", "src/lib.rs").unwrap();
        let other = score("lib.rs", "src/library/stuff.rs").unwrap();
        assert!(exact > other);
    }

    #[test]
    fn mention_parsing() {
        assert_eq!(mentions("look at @src/main.rs and @\"my file.txt\", not a@b.c"), vec!["src/main.rs", "my file.txt"]);
        assert_eq!(mentions("(@a.rs) @b.rs."), vec!["a.rs", "b.rs"]);
        assert!(mentions("email me@x.com").is_empty());
        assert_eq!(url_mentions("read @https://example.com/a and @src/x.rs"), vec!["https://example.com/a"]);
    }

    #[test]
    fn search_and_expand() {
        let dir = std::env::temp_dir().join(format!("cr-fs-{}", crate::util::new_id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        let r = search(&dir, "main", 5);
        assert_eq!(r[0]["path"], "src/main.rs");
        let s = crate::codetools::CodeSession::new(dir.clone());
        let b = expand("fix @src/main.rs please", &s);
        assert_eq!(b.len(), 1);
        assert!(b[0]["text"].as_str().unwrap().contains("fn main()"));
        assert!(s.read_files.lock().contains(&dir.join("src/main.rs")));
        let b = expand("see @src/main.rs#L1", &s);
        assert!(b[0]["text"].as_str().unwrap().contains("src/main.rs:1. Lines 1-81"), "{b:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
