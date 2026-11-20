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
pub fn list(root: &Path) -> FileList {
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
    let p: Vec<char> = path.chars().collect();
    let pl: Vec<char> = path.to_lowercase().chars().collect();
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
    for m in mentions(text).into_iter().take(10) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_file_names_and_consecutive_runs() {
        let a = score("main", "src/main.rs").unwrap();
        let b = score("main", "src/domain/manifest.rs").unwrap();
        assert!(a > b, "{a} {b}");
        assert!(score("xyz", "src/main.rs").is_none());
        assert!(score("smr", "src/main.rs").is_some());
        let exact = score("lib.rs", "src/lib.rs").unwrap();
        let other = score("lib.rs", "src/library/stuff.rs").unwrap();
        assert!(exact > other);
    }

    #[test]
    fn mention_parsing() {
        assert_eq!(mentions("look at @src/main.rs and @\"my file.txt\", not a@b.c"), vec!["src/main.rs", "my file.txt"]);
        assert_eq!(mentions("(@a.rs) @b.rs."), vec!["a.rs", "b.rs"]);
        assert!(mentions("email me@x.com").is_empty());
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
