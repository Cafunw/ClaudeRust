//! TODO / FIXME / HACK / XXX / BUG comments in a project, for the `/todos`
//! list (gitignore-aware, via the cached file list).

use std::sync::LazyLock;
use regex::Regex;
use serde::Serialize;
use std::path::Path;

/// A marker as a whole uppercase word, then optional `(who)` and `:`.
static MARK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(TODO|FIXME|HACK|XXX|BUG)\b(?:\([^)]{0,40}\))?:?\s*(.*)").unwrap());
/// Something that starts a comment before the marker.
static COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(//|#|/\*|\*|<!--|--|;|%|\{-|\(\*|\bREM\b)").unwrap());

const MAX_FILE: u64 = 1024 * 1024;
const MAX_ITEMS: usize = 2000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Todo {
    pub path: String,
    pub line: usize,
    pub kind: String,
    pub text: String,
}

/// The marker on one line, when it sits in a comment.
pub fn parse_line(line: &str) -> Option<(String, String)> {
    let c = MARK_RE.captures(line)?;
    let m = c.get(1)?;
    // the marker must come after a comment opener on the same line
    // (so identifiers like `TODO_LIST` or strings mentioning it are skipped)
    let before = &line[..m.start()];
    if !COMMENT_RE.is_match(before) {
        return None;
    }
    let text = c.get(2).map(|t| t.as_str()).unwrap_or("").trim();
    // "TODO / FIXME …", "TODO, FIXME": the markers being listed, not used
    if text.starts_with('/') || text.starts_with(',') {
        return None;
    }
    let text = text.trim_end_matches("*/").trim_end_matches("-->").trim();
    Some((m.as_str().to_string(), text.chars().take(200).collect()))
}

/// All markers under `root`, in path and line order. Returns the items and
/// whether the list was cut off.
pub fn scan(root: &Path) -> (Vec<Todo>, bool) {
    let files = crate::filesearch::list(root);
    let mut out = Vec::new();
    for (rel, is_dir) in files.iter() {
        if *is_dir {
            continue;
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
        for (i, line) in text.lines().enumerate() {
            if line.len() > 2000 {
                continue; // minified code
            }
            if let Some((kind, t)) = parse_line(line) {
                out.push(Todo { path: rel.clone(), line: i + 1, kind, text: t });
                if out.len() >= MAX_ITEMS {
                    return (out, true);
                }
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_in_comments_only() {
        assert_eq!(parse_line("    // TODO: handle errors"), Some(("TODO".into(), "handle errors".into())));
        assert_eq!(parse_line("# FIXME(ana): slow path"), Some(("FIXME".into(), "slow path".into())));
        assert_eq!(parse_line("/* HACK until v2 */"), Some(("HACK".into(), "until v2".into())));
        assert_eq!(parse_line("x = 1  # XXX"), Some(("XXX".into(), "".into())));
        assert_eq!(parse_line("<!-- TODO add alt text -->"), Some(("TODO".into(), "add alt text".into())));
        assert_eq!(parse_line("let todo = TODO_LIST;"), None);
        assert_eq!(parse_line("println!(\"TODO\");"), None);
        assert_eq!(parse_line("// todo lowercase is ignored"), None);
        assert_eq!(parse_line(" * BUG: off by one"), Some(("BUG".into(), "off by one".into())));
        assert_eq!(parse_line("/// Finds TODO / FIXME comments"), None);
        assert_eq!(parse_line("// TODO, FIXME and HACK markers"), None);
    }

    #[test]
    fn scan_a_tree() {
        let d = std::env::temp_dir().join(format!("cr-todos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("node_modules/x")).unwrap();
        std::fs::write(d.join("src/b.rs"), "fn a() {}\n// TODO: second\n").unwrap();
        std::fs::write(d.join("src/a.py"), "# FIXME first\nx = 1\n").unwrap();
        std::fs::write(d.join("node_modules/x/i.js"), "// TODO vendored\n").unwrap();
        std::fs::write(d.join("blob.bin"), [0u8, b'/', b'/', b' ', b'T', b'O', b'D', b'O']).unwrap();
        let (items, cut) = scan(&d);
        assert!(!cut);
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!((items[0].path.as_str(), items[0].line, items[0].kind.as_str()), ("src/a.py", 1, "FIXME"));
        assert_eq!((items[1].path.as_str(), items[1].line, items[1].text.as_str()), ("src/b.rs", 2, "second"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
