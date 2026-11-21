//! "Find in files" for the Files pane: text or regex search through the
//! project's files (gitignore-aware, via the cached file list).

use crate::error::{AppError, AppResult};
use regex::RegexBuilder;
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

const MAX_FILE: u64 = 2 * 1024 * 1024;
const MAX_HITS: usize = 500;
const MAX_PER_FILE: usize = 50;
const TIME_LIMIT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hit {
    pub path: String,
    pub line: usize,
    /// The line (trimmed to ~240 characters around the match).
    pub text: String,
    /// Byte range of the match within `text`.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Serialize)]
pub struct Results {
    pub hits: Vec<Hit>,
    pub files: usize,
    /// Stopped early (too many hits or too slow).
    pub truncated: bool,
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// A window of `line` around `[s, e)`, and the match's range inside it.
fn excerpt(line: &str, s: usize, e: usize) -> (String, usize, usize) {
    const CTX: usize = 100;
    let from = floor_boundary(line, s.saturating_sub(CTX));
    let to = ceil_boundary(line, (e + CTX).max(s + 1));
    let lead = if from > 0 { "…" } else { "" };
    let tail = if to < line.len() { "…" } else { "" };
    let text = format!("{lead}{}{tail}", &line[from..to]);
    (text, lead.len() + (s - from), lead.len() + (e - from))
}

/// Search `root` for `query`.
pub fn search(root: &Path, query: &str, regex: bool, case_sensitive: bool) -> AppResult<Results> {
    if query.is_empty() {
        return Ok(Results { hits: vec![], files: 0, truncated: false });
    }
    let pat = if regex { query.to_string() } else { regex::escape(query) };
    let re = RegexBuilder::new(&pat)
        .case_insensitive(!case_sensitive)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| AppError::msg(format!("Invalid pattern: {e}")))?;
    let started = Instant::now();
    let list = crate::filesearch::list(root);
    let mut hits = Vec::new();
    let mut files = 0usize;
    let mut truncated = false;
    'files: for (rel, is_dir) in list.iter() {
        if *is_dir {
            continue;
        }
        if started.elapsed() > TIME_LIMIT {
            truncated = true;
            break;
        }
        let p = root.join(rel);
        let Ok(meta) = std::fs::metadata(&p) else { continue };
        if meta.len() > MAX_FILE || meta.len() == 0 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        if bytes.iter().take(8000).any(|b| *b == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        if !re.is_match(&text) {
            continue;
        }
        let mut in_file = 0;
        for (i, line) in text.lines().enumerate() {
            let Some(m) = re.find(line) else { continue };
            if m.start() == m.end() {
                continue; // empty match (e.g. `^`)
            }
            if in_file == 0 {
                files += 1;
            }
            let (t, s, e) = excerpt(line, m.start(), m.end());
            hits.push(Hit { path: rel.clone(), line: i + 1, text: t, start: s, end: e });
            in_file += 1;
            if hits.len() >= MAX_HITS {
                truncated = true;
                break 'files;
            }
            if in_file >= MAX_PER_FILE {
                break;
            }
        }
    }
    Ok(Results { hits, files, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_text_and_regex() {
        let d = std::env::temp_dir().join(format!("cr-grep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join("src/a.rs"), "fn load_user() {}\nlet x = LOAD_USER;\n// unrelated\n").unwrap();
        std::fs::write(d.join("src/b.py"), "def load_user(id):\n    pass\n").unwrap();
        std::fs::write(d.join("target/gen.rs"), "fn load_user() {}\n").unwrap();
        let r = search(&d, "load_user", false, false).unwrap();
        assert_eq!(r.files, 2);
        assert_eq!(r.hits.len(), 3);
        let h = &r.hits.iter().find(|h| h.path == "src/a.rs" && h.line == 2).unwrap();
        assert_eq!(&h.text[h.start..h.end], "LOAD_USER");
        let r = search(&d, "load_user", false, true).unwrap();
        assert_eq!(r.hits.len(), 2, "case-sensitive");
        let r = search(&d, r"def \w+\(", true, false).unwrap();
        assert_eq!(r.hits.len(), 1);
        assert_eq!(r.hits[0].path, "src/b.py");
        assert!(search(&d, "(", true, false).is_err());
        assert_eq!(search(&d, "(", false, false).unwrap().hits.len(), 2, "literal paren");
        assert!(search(&d, "^", true, false).unwrap().hits.is_empty(), "empty matches skipped");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn excerpts_keep_the_match() {
        let line = format!("{}needle{}", "ğ".repeat(300), "x".repeat(300));
        let s = line.find("needle").unwrap();
        let (t, a, b) = excerpt(&line, s, s + 6);
        assert_eq!(&t[a..b], "needle");
        assert!(t.starts_with('…') && t.ends_with('…'));
        assert!(t.len() < line.len());
    }
}
