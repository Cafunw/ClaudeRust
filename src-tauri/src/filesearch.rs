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
            blocks.push(json!({"type": "text", "text": format!(
                "<system-reminder>The user mentioned the directory {}. Its layout (.gitignore respected):\n{}</system-reminder>", p.display(), dir_tree(&p, 3, 300))}));
            continue;
        }
        // a device (`/dev/zero` never ends) or a FIFO (blocks until someone
        // writes) is not something to read into a message
        if !meta.is_file() {
            blocks.push(json!({"type": "text", "text": format!(
                "<system-reminder>The user mentioned {}, which is not a regular file (a device, pipe or socket); its contents were not included.</system-reminder>", p.display())}));
            continue;
        }
        if meta.len() > MENTION_MAX_BYTES || total > 600 * 1024 {
            blocks.push(json!({"type": "text", "text": format!(
                "<system-reminder>The user mentioned {} ({} bytes); it is too large to include, read the parts you need.</system-reminder>", p.display(), meta.len())}));
            continue;
        }
        // at most the limit, even if the file grew since it was measured
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&p).and_then(|f| std::io::Read::read_to_end(&mut std::io::Read::take(f, MENTION_MAX_BYTES), &mut bytes));
        if let Err(e) = read {
            blocks.push(json!({"type": "text", "text": format!(
                "<system-reminder>The user mentioned {}, but it could not be read: {e}</system-reminder>", p.display())}));
            continue;
        }
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

/// Most of a mentioned file that is read into a message.
const MENTION_MAX_BYTES: u64 = 256 * 1024;
/// Most of a mentioned web page that is downloaded.
const PAGE_MAX_BYTES: usize = 3_000_000;

/// Fetch a mentioned web page as text (hidden reminder block). `None` only
/// for something that isn't an http(s) URL; a page that can't be fetched
/// gives a block saying why, so the mention doesn't silently vanish.
pub async fn fetch_url_block(url: &str) -> Option<serde_json::Value> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let body = match fetch_page(url, std::time::Duration::from_secs(20)).await {
        Ok(text) => text,
        Err(e) => format!("(could not fetch the page: {e})"),
    };
    Some(json!({"type": "text", "text": format!("<system-reminder>The user mentioned {url}. Its content:\n{body}</system-reminder>")}))
}

/// A page as text, reading at most `PAGE_MAX_BYTES` of it (a page that
/// never ends, or a huge download, stops there).
async fn fetch_page(url: &str, timeout: std::time::Duration) -> Result<String, String> {
    let client = reqwest::Client::builder().timeout(timeout).user_agent("ClaudeRust").build().map_err(|e| e.to_string())?;
    let mut r = client.get(url).send().await.map_err(|e| if e.is_timeout() { format!("no answer within {} s", timeout.as_secs()) } else { e.to_string() })?;
    let status = r.status();
    let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut bytes: Vec<u8> = Vec::new();
    let mut cut = false;
    loop {
        match r.chunk().await {
            Ok(Some(c)) => {
                let room = PAGE_MAX_BYTES - bytes.len();
                if c.len() >= room {
                    bytes.extend_from_slice(&c[..room]);
                    cut = true;
                    break;
                }
                bytes.extend_from_slice(&c);
            }
            Ok(None) => break,
            Err(e) if e.is_timeout() => return Err(format!("the page didn't finish loading within {} s", timeout.as_secs())),
            Err(e) => return Err(format!("the connection broke while reading it ({e})")),
        }
    }
    let text = if ct.contains("html") || ct.is_empty() {
        html2text::from_read(&bytes[..], 100).unwrap_or_else(|_| String::from_utf8_lossy(&bytes).to_string())
    } else if ct.starts_with("text/") || ct.contains("json") || ct.contains("xml") || ct.contains("javascript") {
        String::from_utf8_lossy(&bytes).to_string()
    } else {
        format!("(binary content: {ct}, {}{} bytes)", if cut { "over " } else { "" }, bytes.len())
    };
    let note = if cut { format!("\n(only the first {} MB of the page were read)", PAGE_MAX_BYTES / 1_000_000) } else { String::new() };
    Ok(if status.is_success() {
        format!("{}{note}", crate::util::truncate_middle(text.trim(), 100_000))
    } else {
        format!("(the page answered {status})\n{}", crate::util::truncate_middle(text.trim(), 4_000))
    })
}

/// An indented listing of `dir` (`depth` levels, at most `max` entries),
/// .gitignore-aware, folders first.
pub fn dir_tree(dir: &Path, depth: usize, max: usize) -> String {
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(depth))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .sort_by_file_path(|a, b| {
            // folders first, then by name
            let ad = a.is_dir();
            let bd = b.is_dir();
            bd.cmp(&ad).then_with(|| a.file_name().cmp(&b.file_name()))
        })
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(n == ".git" || n == "node_modules" || n == "target" || n == ".DS_Store")
        })
        .build();
    let mut out = String::new();
    let mut n = 0;
    for e in walker.flatten() {
        if e.depth() == 0 {
            continue;
        }
        if n >= max {
            out.push_str("…\n");
            break;
        }
        let name = e.file_name().to_string_lossy();
        let slash = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) { "/" } else { "" };
        out.push_str(&format!("{}{name}{slash}\n", "  ".repeat(e.depth() - 1)));
        n += 1;
    }
    if out.is_empty() {
        out.push_str("(empty)\n");
    }
    out
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
    fn directory_trees() {
        let d = std::env::temp_dir().join(format!("cr-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src/deep/deeper/deepest")).unwrap();
        std::fs::create_dir_all(d.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(d.join("build")).unwrap();
        std::fs::write(d.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(d.join("README.md"), "").unwrap();
        std::fs::write(d.join("src/main.rs"), "").unwrap();
        std::fs::write(d.join("src/deep/deeper/deepest/x.rs"), "").unwrap();
        let t = dir_tree(&d, 3, 100);
        assert_eq!(t, "src/\n  deep/\n    deeper/\n  main.rs\n.gitignore\nREADME.md\n", "{t}");
        assert!(dir_tree(&d, 3, 2).ends_with("…\n"));
        let _ = std::fs::remove_dir_all(&d);
    }

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

    #[cfg(unix)]
    #[test]
    fn mentions_of_devices_and_pipes_are_not_read() {
        let dir = std::env::temp_dir().join(format!("cr-fs-fifo-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: mkfifo with a valid, NUL-terminated path
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let s = crate::codetools::CodeSession::new(dir.clone());
        // in a thread: reading /dev/zero never ends, opening a FIFO blocks
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(expand("look at @pipe and @/dev/zero", &s));
        });
        let b = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("expanding the mentions hung");
        assert_eq!(b.len(), 2, "{b:?}");
        assert!(b.iter().all(|x| x["text"].as_str().unwrap().contains("not a regular file")), "{b:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Answer one request with `head`, then `body`, then keep the connection
    /// open for a while (`linger`) or close it.
    async fn serve_once(head: String, body: Vec<u8>, linger: bool) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        std::env::set_var("NO_PROXY", "127.0.0.1");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(head.as_bytes()).await;
            let _ = s.write_all(&body).await;
            if linger {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
        format!("http://{addr}/page")
    }

    #[tokio::test]
    async fn a_huge_page_is_read_only_up_to_the_cap() {
        // 4 MB and then the page never ends
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\r\n".to_string();
        let url = serve_once(head, vec![b'a'; 4_000_000], true).await;
        let b = tokio::time::timeout(std::time::Duration::from_secs(10), fetch_url_block(&url)).await.expect("read past the cap");
        let text = b.expect("the mention vanished")["text"].as_str().unwrap().to_string();
        assert!(text.contains("only the first 3 MB"), "{}", &text[..200.min(text.len())]);
    }

    #[tokio::test]
    async fn a_page_that_fails_to_load_says_so() {
        // promises 1000 bytes, sends 10, hangs up
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 1000\r\n\r\n".to_string();
        let url = serve_once(head, b"0123456789".to_vec(), false).await;
        let b = fetch_url_block(&url).await.expect("the mention vanished");
        let text = b["text"].as_str().unwrap();
        assert!(text.contains(&url) && text.contains("could not fetch the page"), "{text}");
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
