//! A project's runnable scripts, for the Tasks pane: package.json scripts
//! (with the right package manager), Makefile targets, justfile recipes,
//! deno / composer tasks, and the usual Cargo / Go / pytest commands.

use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Script {
    /// Where it comes from: "package.json", "Makefile", "justfile", "Cargo", …
    pub source: String,
    pub name: String,
    /// The command to run in the project folder.
    pub command: String,
    /// What the script itself runs, when known (package.json body, …).
    pub detail: String,
}

const MAX: usize = 80;

fn s(source: &str, name: &str, command: String, detail: &str) -> Script {
    Script { source: source.into(), name: name.into(), command, detail: detail.into() }
}

/// A script name that is safe to put in a shell line as it is. Others (a
/// package.json key like `lint; curl x|sh`) are left out rather than run.
fn safe_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || ":_./@+-".contains(c))
}

/// JSON with comments and trailing commas (deno.jsonc), as plain JSON.
fn strip_jsonc(text: &str) -> String {
    // pass 1: comments out (outside strings)
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            plain.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    plain.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '/' && chars.peek() == Some(&'/') {
            for n in chars.by_ref() {
                if n == '\n' {
                    plain.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prev = ' ';
            for n in chars.by_ref() {
                if prev == '*' && n == '/' {
                    break;
                }
                prev = n;
            }
        } else {
            in_str = c == '"';
            plain.push(c);
        }
    }
    // pass 2: commas right before a closing bracket out (outside strings)
    let b: Vec<char> = plain.chars().collect();
    let mut out = String::with_capacity(plain.len());
    let mut in_str = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == '\\' && i + 1 < b.len() {
                out.push(c);
                i += 1;
                out.push(b[i]);
            } else {
                in_str = c != '"';
                out.push(c);
            }
        } else if c == ',' && matches!(b[i + 1..].iter().find(|x| !x.is_whitespace()), Some('}' | ']')) {
            // dropped
        } else {
            in_str = c == '"';
            out.push(c);
        }
        i += 1;
    }
    out
}

fn read(root: &Path, f: &str) -> Option<String> {
    let p = root.join(f);
    let m = std::fs::metadata(&p).ok()?;
    if !m.is_file() || m.len() > 1024 * 1024 {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

/// The package manager a JS project uses, from its lockfile.
fn js_runner(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

/// Make targets a person would run (no pattern rules, variables, or
/// special targets like `.PHONY`).
pub fn make_targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '#', '.']) {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else { continue };
        if rest.trim_start_matches(':').starts_with('=') || head.contains('=') || head.contains('%') || head.contains('$') {
            continue; // `X := y`, `X ::= y`, pattern rules, variables
        }
        for t in head.split_whitespace() {
            if t.chars().all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c)) && !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// Recipe names in a justfile.
pub fn just_recipes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#', '[', '@']) && !line.starts_with('@') {
            continue;
        }
        let l = line.trim_start_matches('@');
        let word: String = l.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
        if word.is_empty() || ["set", "alias", "export", "import", "mod"].contains(&word.as_str()) {
            continue;
        }
        let after = &l[word.len()..];
        // `name:` or `name arg1 arg2:` (but not `name := value`)
        if let Some(i) = after.find(':') {
            if after[i..].starts_with(":=") || after[..i].contains('=') && !after[..i].contains(' ') {
                continue;
            }
            if !out.contains(&word) {
                out.push(word);
            }
        }
    }
    out
}

/// Everything runnable that `root` declares.
pub fn detect(root: &Path) -> Vec<Script> {
    let mut out = Vec::new();
    if let Some(pj) = read(root, "package.json").and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        let runner = js_runner(root);
        if let Some(map) = pj["scripts"].as_object() {
            for (name, body) in map.iter().filter(|(n, _)| safe_name(n)) {
                let cmd = if runner == "npm" { format!("npm run {name}") } else { format!("{runner} run {name}") };
                out.push(s("package.json", name, cmd, body.as_str().unwrap_or("")));
            }
        }
    }
    for f in ["Makefile", "makefile", "GNUmakefile"] {
        if let Some(t) = read(root, f) {
            for name in make_targets(&t) {
                out.push(s(f, &name, format!("make {name}"), ""));
            }
            break;
        }
    }
    for f in ["justfile", "Justfile", ".justfile"] {
        if let Some(t) = read(root, f) {
            for name in just_recipes(&t) {
                out.push(s("justfile", &name, format!("just {name}"), ""));
            }
            break;
        }
    }
    for f in ["deno.json", "deno.jsonc"] {
        if let Some(v) = read(root, f).and_then(|t| serde_json::from_str::<Value>(&strip_jsonc(&t)).ok()) {
            if let Some(map) = v["tasks"].as_object() {
                for (name, body) in map.iter().filter(|(n, _)| safe_name(n)) {
                    out.push(s("deno", name, format!("deno task {name}"), body.as_str().unwrap_or("")));
                }
            }
        }
    }
    if let Some(v) = read(root, "composer.json").and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        if let Some(map) = v["scripts"].as_object() {
            for name in map.keys().filter(|n| safe_name(n)) {
                out.push(s("composer", name, format!("composer run {name}"), ""));
            }
        }
    }
    if root.join("Cargo.toml").exists() {
        for (n, c) in [("build", "cargo build"), ("test", "cargo test"), ("check", "cargo check"), ("clippy", "cargo clippy")] {
            out.push(s("Cargo", n, c.into(), ""));
        }
        if root.join("src/main.rs").exists() {
            out.push(s("Cargo", "run", "cargo run".into(), ""));
        }
    }
    if root.join("go.mod").exists() {
        for (n, c) in [("build", "go build ./..."), ("test", "go test ./..."), ("vet", "go vet ./...")] {
            out.push(s("Go", n, c.into(), ""));
        }
    }
    let py = read(root, "pyproject.toml").unwrap_or_default();
    if py.contains("pytest") || ((root.join("pyproject.toml").exists() || root.join("setup.py").exists()) && root.join("tests").is_dir()) {
        out.push(s("Python", "test", "pytest".into(), ""));
    }
    out.truncate(MAX);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_and_just() {
        let mk = "CC := gcc\nX ::= y\nY :::= z\nall: build\n\nbuild test: deps\n\t$(CC) x.c\n.PHONY: all build\n%.o: %.c\n\tcc\nlint:\n# comment: no\n";
        assert_eq!(make_targets(mk), vec!["all", "build", "test", "lint"]);
        let just = "set shell := [\"bash\", \"-c\"]\nversion := \"1\"\n\n# build it\nbuild:\n    cargo build\n\ntest filter='':\n    cargo test {{filter}}\n@fmt:\n    cargo fmt\nalias b := build\n";
        assert_eq!(just_recipes(just), vec!["build", "test", "fmt"]);
    }

    #[test]
    fn detects_a_mixed_project() {
        let d = std::env::temp_dir().join(format!("cr-scripts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("package.json"), r#"{"scripts":{"dev":"vite","test":"vitest run","lint; curl x|sh":"eslint .","build:prod":"vite build"}}"#).unwrap();
        std::fs::write(d.join("deno.jsonc"), "{\n  // tasks\n  \"tasks\": { \"serve\": \"deno run -A main.ts\", /* x */ \"url\": \"echo http://a\", // last\n },\n}\n").unwrap();
        std::fs::write(d.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(d.join("Makefile"), "release:\n\techo hi\n").unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main(){}").unwrap();
        let all = detect(&d);
        let cmds: Vec<&str> = all.iter().map(|s| s.command.as_str()).collect();
        assert!(cmds.contains(&"pnpm run dev"), "{cmds:?}");
        assert!(cmds.contains(&"pnpm run test"));
        assert!(cmds.contains(&"pnpm run build:prod"));
        assert!(!cmds.iter().any(|c| c.contains("curl")), "unsafe names are left out: {cmds:?}");
        assert!(cmds.contains(&"deno task serve") && cmds.contains(&"deno task url"), "{cmds:?}");
        assert_eq!(all.iter().find(|s| s.name == "url").unwrap().detail, "echo http://a");
        assert!(cmds.contains(&"make release"));
        assert!(cmds.contains(&"cargo test"));
        assert!(cmds.contains(&"cargo run"));
        assert_eq!(all.iter().find(|s| s.name == "dev").unwrap().detail, "vite");
        let _ = std::fs::remove_dir_all(&d);
        assert!(detect(&d).is_empty());
    }
}
