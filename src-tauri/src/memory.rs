//! Project rules and memory for code sessions: CLAUDE.md files (user,
//! project ancestors, `.claude/CLAUDE.md`, `CLAUDE.local.md`) and AGENTS.md,
//! with `@path` imports; plus quick additions ("# remember this") and an
//! editor for these files.

use crate::error::{AppError, AppResult};
use crate::util::{truncate_middle, TextEncoding};
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

/// A memory file's text and how it was encoded; `Ok(None)` when it doesn't
/// exist. Text that is neither UTF-8 nor UTF-16 with a byte-order mark (a
/// legacy code page) is an error, so it's never taken for an empty file.
fn read_memory(p: &Path) -> AppResult<Option<(String, TextEncoding)>> {
    let bytes = match std::fs::read(p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::msg(format!("{} can't be read: {e}", p.display()))),
    };
    crate::util::decode_text(&bytes)
        .map(Some)
        .ok_or_else(|| AppError::msg(format!("{} isn't UTF-8 or UTF-16 text.", p.display())))
}

/// Why a memory file can't be written: it isn't one of the session's files
/// inside the project, or what's there can't be read back faithfully.
fn writable(cwd: &Path, path: &Path) -> AppResult<Option<TextEncoding>> {
    let f = files(cwd).into_iter().find(|f| f["path"].as_str().map(|p| Path::new(p) == path).unwrap_or(false));
    let Some(f) = f else {
        return Err(AppError::msg("That isn't one of this session's memory files."));
    };
    if f["scope"] != "user" && !stays_inside(path, &project_root(cwd)) {
        return Err(AppError::msg(format!("{} links outside the project folder, so it isn't changed from here.", path.display())));
    }
    match read_memory(path) {
        Ok(found) => Ok(found.map(|(_, enc)| enc)),
        Err(e) => Err(AppError::msg(format!("{e} It was left unchanged; save it as UTF-8 in another editor to edit it here."))),
    }
}

/// Does `p` (followed through symlinks) stay inside `folder`? A missing file
/// counts by its nearest existing folder; a dangling symlink doesn't stay.
fn stays_inside(p: &Path, folder: &Path) -> bool {
    let folder = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    if std::fs::symlink_metadata(p).is_ok() {
        return std::fs::canonicalize(p).map(|c| c.starts_with(&folder)).unwrap_or(false);
    }
    let mut cur = p.parent();
    while let Some(d) = cur {
        if let Ok(c) = std::fs::canonicalize(d) {
            return c.starts_with(&folder);
        }
        cur = d.parent();
    }
    false
}

/// `limit`: project files (and their imports) must stay inside this folder,
/// also through symlinks (a cloned repository must not pull ~/.ssh or ~/.aws
/// into the prompt); `None` for the user's own memory file.
fn add_file(p: &Path, label: &str, parts: &mut Vec<String>, seen: &mut HashSet<PathBuf>, total: &mut usize, depth: usize, limit: Option<&Path>) {
    // missing (or a dangling link): nothing to add
    let Ok(key) = std::fs::canonicalize(p) else { return };
    // (already included, e.g. ~/.claude/CLAUDE.md as the user's file)
    if seen.contains(&key) || *total > TOTAL {
        return;
    }
    if let Some(lim) = limit {
        if !stays_inside(p, lim) {
            if depth == 0 {
                parts.push(format!("({} skipped: it links outside the project folder.)", p.display()));
            }
            return;
        }
    }
    seen.insert(key);
    let t = match read_memory(p) {
        Ok(Some((t, _))) => t,
        Ok(None) => return,
        Err(e) => {
            parts.push(format!("({e} It was skipped.)"));
            return;
        }
    };
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

fn project_root(cwd: &Path) -> PathBuf {
    crate::worktree::repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

/// The memory files relevant to a session, existing or not. A file that
/// can't be shown faithfully has an `error` (and is read-only): one that
/// isn't UTF-8/UTF-16 text, or a project file linking outside the project.
pub fn files(cwd: &Path) -> Vec<Value> {
    let root = project_root(cwd);
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
        .map(|(scope, p)| {
            let (content, error) = if scope != "user" && !stays_inside(&p, &root) {
                (String::new(), Some(format!("{} links outside the project folder, so it isn't shown or changed here.", p.display())))
            } else {
                match read_memory(&p) {
                    Ok(t) => (t.map(|(t, _)| t).unwrap_or_default(), None),
                    Err(e) => (String::new(), Some(format!("{e} It isn't shown or changed here; save it as UTF-8 in another editor."))),
                }
            };
            let mut f = json!({"scope": scope, "path": p, "exists": p.is_file(), "content": content});
            if let Some(e) = error {
                f["error"] = json!(e);
                f["readonly"] = json!(true);
            }
            f
        })
        .collect()
}

/// Overwrite one of the session's memory files (in the encoding it has).
pub fn save(cwd: &Path, path: &Path, content: &str) -> AppResult<()> {
    let enc = writable(cwd, path)?;
    write_memory(path, content, enc)
}

fn write_memory(path: &Path, content: &str, enc: Option<TextEncoding>) -> AppResult<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let bytes = crate::util::encode_text(content, enc.unwrap_or(TextEncoding::Utf8));
    crate::util::write_atomic(path, &bytes)?;
    Ok(())
}

/// Add a note ("# …" from the composer) to a memory file by scope.
pub fn append(cwd: &Path, scope: &str, note: &str) -> AppResult<PathBuf> {
    let f = files(cwd)
        .into_iter()
        .find(|f| f["scope"] == scope)
        .ok_or_else(|| AppError::msg("Unknown memory file."))?;
    let path = PathBuf::from(f["path"].as_str().unwrap_or_default());
    let enc = writable(cwd, &path)?;
    let mut text = read_memory(&path)?.map(|(t, _)| t).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if text.is_empty() {
        text.push_str("# Memory\n\n");
    }
    let note = note.trim().trim_start_matches('#').trim();
    text.push_str(&format!("- {note}\n"));
    write_memory(&path, &text, enc)?;
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

    #[test]
    fn files_in_other_encodings_are_kept() {
        let dir = std::env::temp_dir().join(format!("cr-mem3-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        // UTF-16 with a BOM, as PowerShell's `>` writes it: read, and appended to in UTF-16
        let p = dir.join("CLAUDE.md");
        std::fs::write(&p, crate::util::encode_text("Use tabs.\n", TextEncoding::Utf16Le)).unwrap();
        assert!(load(&dir).contains("Use tabs."));
        append(&dir, "project", "# run the linter").unwrap();
        let (text, enc) = crate::util::decode_text(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!((text.as_str(), enc), ("Use tabs.\n- run the linter\n", TextEncoding::Utf16Le));
        // a legacy code page (cp1254): never read as empty and overwritten
        let legacy = b"# Kurallar\nT\xfcrk\xe7e yaz\n".to_vec();
        std::fs::write(&p, &legacy).unwrap();
        assert!(append(&dir, "project", "note").unwrap_err().to_string().contains("left unchanged"));
        assert!(save(&dir, &p, "replaced").is_err());
        assert_eq!(std::fs::read(&p).unwrap(), legacy);
        let f = files(&dir).into_iter().find(|f| f["scope"] == "project").unwrap();
        assert!(f["error"].is_string() && f["readonly"] == true && f["exists"] == true, "{f}");
        assert!(load(&dir).contains("skipped"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn project_memory_files_linking_outside_are_not_read_or_written() {
        let base = std::env::temp_dir().join(format!("cr-mem4-{}", crate::util::new_id()));
        let dir = base.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        let secret = base.join("id_rsa");
        std::fs::write(&secret, "-----BEGIN PRIVATE KEY-----\n").unwrap();
        std::os::unix::fs::symlink("../id_rsa", dir.join("CLAUDE.md")).unwrap();
        let m = load(&dir);
        assert!(!m.contains("PRIVATE KEY") && m.contains("skipped"), "{m}");
        assert!(append(&dir, "project", "# note").is_err());
        assert!(save(&dir, &dir.join("CLAUDE.md"), "x").is_err());
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "-----BEGIN PRIVATE KEY-----\n");
        let f = files(&dir).into_iter().find(|f| f["scope"] == "project").unwrap();
        assert!(f["content"] == "" && f["error"].is_string(), "{f}");
        // a link inside the project is fine, and is written through
        std::fs::remove_file(dir.join("CLAUDE.md")).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Shared rules.\n").unwrap();
        std::os::unix::fs::symlink("AGENTS.md", dir.join("CLAUDE.md")).unwrap();
        assert!(load(&dir).contains("Shared rules."));
        append(&dir, "project", "more").unwrap();
        assert!(std::fs::symlink_metadata(dir.join("CLAUDE.md")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(), "Shared rules.\n- more\n");
        let _ = std::fs::remove_dir_all(base);
    }
}
