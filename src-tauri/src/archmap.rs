//! Project architecture map: a graph of source files and the imports
//! between them (Rust `mod`/`use crate::`, JS/TS `import`/`require`,
//! Python `import`/`from`, Go package imports), for the Map pane.

use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

const MAX_FILES: usize = 1500;
const MAX_NODES: usize = 600;

fn lang(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    Some(match ext {
        "rs" => "rust",
        "js" | "mjs" | "cjs" | "jsx" => "js",
        "ts" | "tsx" | "mts" => "ts",
        "py" => "python",
        "go" => "go",
        _ => return None,
    })
}

static RS_USE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+crate::(\w+)").unwrap());
static RS_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bcrate::(\w+)::").unwrap());
static RS_MOD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;").unwrap());
static JS_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:import|export)\s[^'"]*?from\s*['"]([^'"]+)['"]|import\s*\(\s*['"]([^'"]+)['"]\s*\)|require\(\s*['"]([^'"]+)['"]\s*\)|import\s+['"]([^'"]+)['"]"#).unwrap()
});
static PY_FROM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*from\s+(\.*[\w.]*)\s+import").unwrap());
static PY_IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*import\s+([\w.]+)").unwrap());
static GO_IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([\w./-]+)""#).unwrap());

fn norm(path: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    out.join("/")
}

fn dir_of(p: &str) -> &str {
    p.rfind('/').map(|i| &p[..i]).unwrap_or("")
}

/// Build the graph for the project in `root`.
pub fn build(root: &Path) -> Value {
    let all = crate::filesearch::list(root);
    let files: Vec<String> = all.iter().filter(|(p, d)| !d && lang(p).is_some()).map(|(p, _)| p.clone()).take(MAX_FILES).collect();
    let set: HashSet<&str> = files.iter().map(|s| s.as_str()).collect();
    let go_module = std::fs::read_to_string(root.join("go.mod"))
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("module ").map(|m| m.trim().to_string())));

    let resolve_js = |from: &str, spec: &str| -> Option<String> {
        if !spec.starts_with('.') {
            return None;
        }
        let base = norm(&format!("{}/{}", dir_of(from), spec));
        let candidates = [
            base.clone(),
            format!("{base}.ts"), format!("{base}.tsx"), format!("{base}.js"), format!("{base}.jsx"), format!("{base}.mjs"),
            format!("{base}/index.ts"), format!("{base}/index.tsx"), format!("{base}/index.js"), format!("{base}/index.jsx"),
        ];
        candidates.into_iter().find(|c| set.contains(c.as_str()))
    };
    // Rust crate roots: the folder holding lib.rs/main.rs
    let rust_roots: Vec<String> = files.iter().filter(|f| f.ends_with("lib.rs") || f.ends_with("main.rs")).map(|f| dir_of(f).to_string()).collect();
    let rust_root_of = |file: &str| -> Option<&String> { rust_roots.iter().filter(|r| file.starts_with(r.as_str())).max_by_key(|r| r.len()) };
    let rust_mod = |dir: &str, name: &str| -> Option<String> {
        let a = norm(&format!("{dir}/{name}.rs"));
        let b = norm(&format!("{dir}/{name}/mod.rs"));
        [a, b].into_iter().find(|c| set.contains(c.as_str()))
    };

    let mut edges: HashSet<(String, String)> = HashSet::new();
    let mut loc: HashMap<String, usize> = HashMap::new();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(root.join(f)) else { continue };
        if text.len() > 1_000_000 {
            continue;
        }
        loc.insert(f.clone(), text.lines().count());
        let mut add = |to: Option<String>| {
            if let Some(t) = to {
                if &t != f {
                    edges.insert((f.clone(), t));
                }
            }
        };
        match lang(f) {
            Some("rust") => {
                let crate_dir = rust_root_of(f).cloned().unwrap_or_default();
                for c in RS_USE.captures_iter(&text).chain(RS_PATH.captures_iter(&text)) {
                    add(rust_mod(&crate_dir, &c[1]));
                }
                // `mod x;` in a.rs refers to a/x.rs (or x.rs next to lib.rs/main.rs/mod.rs)
                let here = dir_of(f);
                let stem = f.rsplit('/').next().unwrap_or("").trim_end_matches(".rs");
                let child_dir = if matches!(stem, "lib" | "main" | "mod") { here.to_string() } else { format!("{here}/{stem}") };
                for c in RS_MOD.captures_iter(&text) {
                    add(rust_mod(&child_dir, &c[1]));
                }
            }
            Some("js") | Some("ts") => {
                for c in JS_IMPORT.captures_iter(&text) {
                    let spec = c.get(1).or(c.get(2)).or(c.get(3)).or(c.get(4)).map(|m| m.as_str()).unwrap_or("");
                    add(resolve_js(f, spec));
                }
            }
            Some("python") => {
                for c in PY_FROM.captures_iter(&text).chain(PY_IMPORT.captures_iter(&text)) {
                    let m = &c[1];
                    let dots = m.chars().take_while(|c| *c == '.').count();
                    let rest = m[dots..].replace('.', "/");
                    let base = if dots > 0 {
                        let mut d = dir_of(f).to_string();
                        for _ in 1..dots {
                            d = dir_of(&d).to_string();
                        }
                        norm(&format!("{d}/{rest}"))
                    } else {
                        rest
                    };
                    let cands = [format!("{base}.py"), format!("{base}/__init__.py")];
                    add(cands.into_iter().find(|c| set.contains(c.as_str())));
                }
            }
            Some("go") => {
                if let Some(module) = &go_module {
                    for c in GO_IMPORT.captures_iter(&text) {
                        if let Some(rel) = c[1].strip_prefix(module.as_str()) {
                            let dir = rel.trim_start_matches('/');
                            // link to the first file of that package
                            let target = files.iter().find(|x| dir_of(x) == dir && x.ends_with(".go") && !x.ends_with("_test.go")).cloned();
                            add(target);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // keep the most connected files when the project is large
    let mut degree: HashMap<&str, usize> = HashMap::new();
    for (a, b) in &edges {
        *degree.entry(a.as_str()).or_default() += 1;
        *degree.entry(b.as_str()).or_default() += 1;
    }
    let mut nodes: Vec<&String> = files.iter().collect();
    nodes.sort_by_key(|f| std::cmp::Reverse(degree.get(f.as_str()).copied().unwrap_or(0)));
    nodes.truncate(MAX_NODES);
    let keep: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();
    let node_json: Vec<Value> = nodes
        .iter()
        .map(|f| {
            let group = f.split('/').next().filter(|_| f.contains('/')).unwrap_or(".");
            json!({"id": f, "lang": lang(f), "loc": loc.get(*f).copied().unwrap_or(0), "group": group})
        })
        .collect();
    let edge_json: Vec<Value> = edges
        .iter()
        .filter(|(a, b)| keep.contains(a.as_str()) && keep.contains(b.as_str()))
        .map(|(a, b)| json!({"from": a, "to": b}))
        .collect();
    json!({"nodes": node_json, "edges": edge_json, "truncated": files.len() > nodes.len() || all.len() >= 60_000})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphs_rust_js_and_python() {
        let dir = std::env::temp_dir().join(format!("cr-map-{}", crate::util::new_id()));
        for d in ["src/net", "web/lib", "py/pkg"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("src/main.rs"), "mod net;\nmod util;\nuse crate::util::helper;\nfn main(){}").unwrap();
        std::fs::write(dir.join("src/util.rs"), "pub fn helper(){}").unwrap();
        std::fs::write(dir.join("src/net/mod.rs"), "pub mod http;\nuse crate::util::helper;").unwrap();
        std::fs::write(dir.join("src/net/http.rs"), "fn x(){ crate::util::helper(); }").unwrap();
        std::fs::write(dir.join("web/app.ts"), "import { a } from './lib/a';\nimport x from 'react';\nconst b = require('./lib/b.js');").unwrap();
        std::fs::write(dir.join("web/lib/a.ts"), "export const a = 1;").unwrap();
        std::fs::write(dir.join("web/lib/b.js"), "module.exports = 1;").unwrap();
        std::fs::write(dir.join("py/pkg/__init__.py"), "").unwrap();
        std::fs::write(dir.join("py/pkg/core.py"), "from . import util\nfrom .util import f").unwrap();
        std::fs::write(dir.join("py/pkg/util.py"), "def f(): pass").unwrap();
        let g = build(&dir);
        let edges: HashSet<(String, String)> = g["edges"].as_array().unwrap().iter()
            .map(|e| (e["from"].as_str().unwrap().to_string(), e["to"].as_str().unwrap().to_string())).collect();
        let has = |a: &str, b: &str| edges.contains(&(a.to_string(), b.to_string()));
        assert!(has("src/main.rs", "src/net/mod.rs"), "{edges:?}");
        assert!(has("src/main.rs", "src/util.rs"));
        assert!(has("src/net/mod.rs", "src/net/http.rs"));
        assert!(has("src/net/http.rs", "src/util.rs"));
        assert!(has("web/app.ts", "web/lib/a.ts") && has("web/app.ts", "web/lib/b.js"));
        assert!(has("py/pkg/core.py", "py/pkg/util.py"));
        assert_eq!(g["nodes"].as_array().unwrap().len(), 10);
        let _ = std::fs::remove_dir_all(dir);
    }
}
