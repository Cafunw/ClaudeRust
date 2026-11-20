//! Project rules and memory for code sessions: CLAUDE.md files (user,
//! project ancestors, `.claude/CLAUDE.md`, `CLAUDE.local.md`) and AGENTS.md,
//! with `@path` imports; plus quick additions ("# remember this") and an
//! editor for these files.

use crate::error::{AppError, AppResult};
use crate::util::truncate_middle;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const PER_FILE: usize = 40_000;
const TOTAL: usize = 120_000;

fn user_file() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("CLAUDE.md"))
}

/// `@path` imports in a memory file (outside code fences), resolved against
/// the file's folder (or `~`).
fn imports(text: &str, base: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for (i, _) in line.match_indices('@') {
            if i > 0 && !line[..i].ends_with(char::is_whitespace) {
                continue;
            }
            let rest = &line[i + 1..];
            let tok: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != '`' && *c != ')').collect();
            let tok = tok.trim_end_matches(['.', ',', ':', ';']);
            if tok.is_empty() || !(tok.contains('/') || tok.contains('.')) {
                continue;
            }
            let p = if let Some(r) = tok.strip_prefix("~/") {
                match dirs::home_dir() {
                    Some(h) => h.join(r),
                    None => continue,
                }
            } else {
                base.join(tok)
            };
            if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

/// `limit`: imports from project files must stay inside this folder (a
/// cloned repository must not pull ~/.ssh or ~/.aws into the prompt);
/// `None` for the user's own memory file.
fn add_file(p: &Path, label: &str, parts: &mut Vec<String>, seen: &mut HashSet<PathBuf>, total: &mut usize, depth: usize, limit: Option<&Path>) {
    let key = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if !seen.insert(key) || *total > TOTAL {
        return;
    }
    let Ok(t) = std::fs::read_to_string(p) else { return };
    let t = t.trim();
    if t.is_empty() {
        return;
    }
    let body = truncate_middle(t, PER_FILE);
    *total += body.len();
    parts.push(format!("Contents of {} ({label}):\n\n{body}", p.display()));
    if depth < 4 {
        let base = p.parent().unwrap_or(Path::new("."));
        for imp in imports(t, base) {
            if let Some(lim) = limit {
                let canon = std::fs::canonicalize(&imp).unwrap_or_else(|_| imp.clone());
                let lim = std::fs::canonicalize(lim).unwrap_or_else(|_| lim.to_path_buf());
                if !canon.starts_with(&lim) {
                    parts.push(format!("(Import of {} from {} skipped: it is outside the project folder.)", imp.display(), p.display()));
                    continue;
                }
            }
            add_file(&imp, "imported", parts, seen, total, depth + 1, limit);
        }
    }
}

/// The memory text for a session in `cwd` (what goes into the system prompt).
pub fn load(cwd: &Path) -> String {
    let mut parts = vec![];
    let mut seen = HashSet::new();
    let mut total = 0;
    if let Some(u) = user_file() {
        add_file(&u, "user instructions for all projects", &mut parts, &mut seen, &mut total, 0, None);
    }
    let mut dirs_up: Vec<PathBuf> = cwd.ancestors().map(|d| d.to_path_buf()).collect();
    dirs_up.reverse();
    for dir in dirs_up {
        for (name, label) in [
            ("CLAUDE.md", "project instructions"),
            (".claude/CLAUDE.md", "project instructions"),
            ("AGENTS.md", "project instructions for agents"),
            ("CLAUDE.local.md", "private project instructions"),
        ] {
            add_file(&dir.join(name), label, &mut parts, &mut seen, &mut total, 0, Some(&dir));
        }
    }
    parts.join("\n\n")
}

/// The memory files relevant to a session, existing or not.
pub fn files(cwd: &Path) -> Vec<Value> {
    let root = crate::worktree::repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let mut v = vec![];
    if let Some(u) = user_file() {
        v.push(("user", u));
    }
    v.push(("project", root.join("CLAUDE.md")));
    v.push(("local", root.join("CLAUDE.local.md")));
    v.push(("agents", root.join("AGENTS.md")));
    // CLAUDE.md files in folders between the root and the session folder
    if cwd != root {
        for d in cwd.ancestors().take_while(|d| d.starts_with(&root) && *d != root) {
            v.push(("folder", d.join("CLAUDE.md")));
        }
    }
    v.into_iter()
        .map(|(scope, p)| json!({"scope": scope, "path": p, "exists": p.is_file(), "content": std::fs::read_to_string(&p).unwrap_or_default()}))
        .collect()
}

fn allowed(cwd: &Path, path: &Path) -> bool {
    files(cwd).iter().any(|f| f["path"].as_str().map(|p| Path::new(p) == path).unwrap_or(false))
}

/// Overwrite one of the session's memory files.
pub fn save(cwd: &Path, path: &Path, content: &str) -> AppResult<()> {
    if !allowed(cwd, path) {
        return Err(AppError::msg("That isn't one of this session's memory files."));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Add a note ("# …" from the composer) to a memory file by scope.
pub fn append(cwd: &Path, scope: &str, note: &str) -> AppResult<PathBuf> {
    let f = files(cwd)
        .into_iter()
        .find(|f| f["scope"] == scope)
        .ok_or_else(|| AppError::msg("Unknown memory file."))?;
    let path = PathBuf::from(f["path"].as_str().unwrap_or_default());
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if text.is_empty() {
        text.push_str("# Memory\n\n");
    }
    let note = note.trim().trim_start_matches('#').trim();
    text.push_str(&format!("- {note}\n"));
    save(cwd, &path, &text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_claude_and_agents_with_imports_once() {
        let dir = std::env::temp_dir().join(format!("cr-mem-{}", crate::util::new_id()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "Use tabs.\nSee @docs/style.md and @AGENTS.md\n```\n@not/imported.md\n```\n").unwrap();
        std::fs::write(dir.join("docs/style.md"), "Prefer short functions.").unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Run cargo test before finishing.").unwrap();
        let m = load(&dir);
        assert!(m.contains("Use tabs.") && m.contains("Prefer short functions.") && m.contains("Run cargo test"), "{m}");
        assert_eq!(m.matches("Run cargo test").count(), 1, "AGENTS.md included once");
        // imports leaving the project folder are skipped
        let secret = std::env::temp_dir().join(format!("cr-secret-{}.txt", crate::util::new_id()));
        std::fs::write(&secret, "TOP-SECRET").unwrap();
        std::fs::write(dir.join("CLAUDE.md"), format!("See @{}\n", secret.display())).unwrap();
        let m = load(&dir);
        assert!(!m.contains("TOP-SECRET") && m.contains("skipped"), "{m}");
        let _ = std::fs::remove_file(secret);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn append_and_save_are_limited_to_memory_files() {
        let dir = std::env::temp_dir().join(format!("cr-mem2-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = append(&dir, "project", "# always use pnpm").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "# Memory\n\n- always use pnpm\n");
        append(&dir, "project", "write tests").unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().ends_with("- always use pnpm\n- write tests\n"));
        assert!(save(&dir, &dir.join("other.txt"), "x").is_err());
        assert!(save(&dir, &dir.join("CLAUDE.local.md"), "private").is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
