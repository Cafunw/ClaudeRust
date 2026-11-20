//! Symbol index for `@` mentions: functions, types and classes defined in
//! the project's source files (Rust, JS/TS, Python, Go), found with
//! lightweight patterns and cached briefly per folder.

use parking_lot::Mutex;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str,
    pub path: String,
    pub line: usize,
}

type Index = Arc<Vec<Symbol>>;
static CACHE: LazyLock<Mutex<HashMap<PathBuf, (Instant, Index)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

struct Pat {
    re: Regex,
    kind: &'static str,
}

fn pats(ext: &str) -> &'static [Pat] {
    static RUST: LazyLock<Vec<Pat>> = LazyLock::new(|| {
        vec![
            Pat { re: Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([A-Za-z_]\w*)").unwrap(), kind: "fn" },
            Pat { re: Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_]\w*)").unwrap(), kind: "struct" },
            Pat { re: Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_]\w*)").unwrap(), kind: "enum" },
            Pat { re: Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_]\w*)").unwrap(), kind: "trait" },
            Pat { re: Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_]\w*)").unwrap(), kind: "type" },
            Pat { re: Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Z_][A-Z0-9_]*)\s*:").unwrap(), kind: "const" },
        ]
    });
    static JS: LazyLock<Vec<Pat>> = LazyLock::new(|| {
        vec![
            Pat { re: Regex::new(r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\*?\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "function" },
            Pat { re: Regex::new(r"^\s*(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "class" },
            Pat { re: Regex::new(r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>").unwrap(), kind: "function" },
            Pat { re: Regex::new(r"^\s*(?:export\s+)?(?:interface|type)\s+([A-Za-z_$][\w$]*)").unwrap(), kind: "type" },
        ]
    });
    static PY: LazyLock<Vec<Pat>> = LazyLock::new(|| {
        vec![
            Pat { re: Regex::new(r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)").unwrap(), kind: "def" },
            Pat { re: Regex::new(r"^\s*class\s+([A-Za-z_]\w*)").unwrap(), kind: "class" },
        ]
    });
    static GO: LazyLock<Vec<Pat>> = LazyLock::new(|| {
        vec![
            Pat { re: Regex::new(r"^func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)").unwrap(), kind: "func" },
            Pat { re: Regex::new(r"^type\s+([A-Za-z_]\w*)").unwrap(), kind: "type" },
        ]
    });
    match ext {
        "rs" => &RUST,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" => &JS,
        "py" => &PY,
        "go" => &GO,
        _ => &[],
    }
}

/// Symbols defined in one file's text.
pub fn scan(path: &str, text: &str) -> Vec<Symbol> {
    let ext = path.rsplit('.').next().unwrap_or("");
    let ps = pats(ext);
    if ps.is_empty() {
        return vec![];
    }
    let mut out = vec![];
    for (i, line) in text.lines().enumerate() {
        if line.len() > 400 {
            continue;
        }
        for p in ps {
            if let Some(c) = p.re.captures(line) {
                out.push(Symbol { name: c[1].to_string(), kind: p.kind, path: path.to_string(), line: i + 1 });
                break;
            }
        }
    }
    out
}

/// All symbols of a project (cached for 20 s).
static BUILD: Mutex<()> = parking_lot::const_mutex(());

pub fn index(root: &Path) -> Index {
    if let Some((t, v)) = CACHE.lock().get(root) {
        if t.elapsed() < Duration::from_secs(20) {
            return v.clone();
        }
    }
    let _build = BUILD.lock();
    if let Some((t, v)) = CACHE.lock().get(root) {
        if t.elapsed() < Duration::from_secs(20) {
            return v.clone();
        }
    }
    let mut all = vec![];
    for (p, is_dir) in crate::filesearch::list(root).iter() {
        if *is_dir || pats(p.rsplit('.').next().unwrap_or("")).is_empty() {
            continue;
        }
        let full = root.join(p);
        if std::fs::metadata(&full).map(|m| m.len() > 800_000).unwrap_or(true) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&full) {
            all.extend(scan(p, &text));
        }
        if all.len() > 200_000 {
            break;
        }
    }
    let v = Arc::new(all);
    let mut c = CACHE.lock();
    if c.len() > 8 {
        c.clear();
    }
    c.insert(root.to_path_buf(), (Instant::now(), v.clone()));
    v
}

pub fn search(root: &Path, query: &str, limit: usize) -> Vec<Value> {
    if query.len() < 2 {
        return vec![];
    }
    let idx = index(root);
    let ql = query.to_lowercase();
    let mut hits: Vec<(i64, &Symbol)> = idx
        .iter()
        .filter_map(|s| {
            let nl = s.name.to_lowercase();
            let score = if nl == ql {
                1000
            } else if nl.starts_with(&ql) {
                800 - nl.len() as i64
            } else if nl.contains(&ql) {
                500 - nl.len() as i64
            } else {
                crate::filesearch::score(&ql, &s.name).map(|x| x / 1000).filter(|x| *x > 0)?
            };
            Some((score, s))
        })
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter()
        .take(limit)
        .map(|(_, s)| json!({"name": s.name, "kind": s.kind, "path": s.path, "line": s.line}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_definitions() {
        let rs = "pub struct Engine {}\nimpl Engine {\n    pub async fn run(&self) {}\n}\nconst MAX_STEPS: usize = 3;\nenum Mode { A }\n";
        let names: Vec<(String, &str, usize)> = scan("a.rs", rs).into_iter().map(|s| (s.name, s.kind, s.line)).collect();
        assert_eq!(names, vec![("Engine".into(), "struct", 1), ("run".into(), "fn", 3), ("MAX_STEPS".into(), "const", 5), ("Mode".into(), "enum", 6)]);
        let js = "export function openPanel() {}\nexport const send = async (x) => {}\nclass Store {}\ninterface Props {}\n";
        let n: Vec<String> = scan("a.ts", js).into_iter().map(|s| s.name).collect();
        assert_eq!(n, vec!["openPanel", "send", "Store", "Props"]);
        let py = "class Agent:\n    def run(self):\n        pass\nasync def main():\n";
        assert_eq!(scan("a.py", py).len(), 3);
        assert_eq!(scan("a.go", "func (s *S) Handle() {}\ntype S struct{}\n").len(), 2);
        assert!(scan("a.md", "fn x()").is_empty());
    }

    #[test]
    fn searches_a_project() {
        let dir = std::env::temp_dir().join(format!("cr-sym-{}", crate::util::new_id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn build_messages() {}\npub fn build() {}\n").unwrap();
        let r = search(&dir, "build", 5);
        assert_eq!(r[0]["name"], "build");
        assert_eq!(r[1]["name"], "build_messages");
        assert_eq!(r[1]["line"], 1);
        assert!(search(&dir, "b", 5).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
