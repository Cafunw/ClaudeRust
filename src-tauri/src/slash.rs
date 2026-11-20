//! Custom slash commands in Claude Code's format: Markdown files in
//! `<project>/.claude/commands/` and `~/.claude/commands/` (subfolders become
//! `folder:name`), with optional front matter (`description`,
//! `argument-hint`). `$ARGUMENTS` and `$1`…`$9` are replaced on use.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Command {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    pub source: &'static str,
    pub path: PathBuf,
}

/// Split `---` front matter from the body.
pub fn front_matter(text: &str) -> (Vec<(String, String)>, String) {
    let t = text.trim_start_matches('\u{feff}');
    if let Some(rest) = t.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let head = &rest[..end];
            let body = rest[end + 4..].trim_start_matches(['\r', '\n']).to_string();
            let fields = head
                .lines()
                .filter_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    Some((k.trim().to_string(), v.trim().trim_matches('"').trim_matches('\'').to_string()))
                })
                .collect();
            return (fields, body);
        }
    }
    (vec![], t.to_string())
}

fn collect(dir: &Path, prefix: &str, source: &'static str, out: &mut Vec<Command>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() && depth < 3 {
            collect(&p, &format!("{prefix}{name}:"), source, out, depth + 1);
        } else if let Some(stem) = name.strip_suffix(".md") {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let (fm, body) = front_matter(&text);
            let get = |k: &str| fm.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default();
            let mut description = get("description");
            if description.is_empty() {
                description = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start_matches('#').trim().chars().take(100).collect();
            }
            out.push(Command {
                name: format!("{prefix}{stem}"),
                description,
                argument_hint: get("argument-hint"),
                source,
                path: p,
            });
        }
    }
}

pub fn list(project: Option<&Path>) -> Vec<Command> {
    let mut out = vec![];
    if let Some(p) = project {
        collect(&p.join(".claude").join("commands"), "", "project", &mut out, 0);
    }
    if let Some(h) = dirs::home_dir() {
        collect(&h.join(".claude").join("commands"), "", "user", &mut out, 0);
    }
    // project commands win over user commands with the same name
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.name.clone()));
    out
}

pub fn to_json(cmds: &[Command]) -> Value {
    json!(cmds
        .iter()
        .map(|c| json!({"name": c.name, "description": c.description, "argument_hint": c.argument_hint, "source": c.source}))
        .collect::<Vec<_>>())
}

/// Replace `$ARGUMENTS` and positional `$1`..`$9`.
pub fn substitute(body: &str, args: &str) -> String {
    let words = shell_words::split(args).unwrap_or_else(|_| args.split_whitespace().map(String::from).collect());
    let mut s = body.replace("$ARGUMENTS", args);
    for i in (1..=9).rev() {
        s = s.replace(&format!("${i}"), words.get(i - 1).map(|w| w.as_str()).unwrap_or(""));
    }
    if !body.contains("$ARGUMENTS") && !args.trim().is_empty() && !(1..=9).any(|i| body.contains(&format!("${i}"))) {
        s.push_str(&format!("\n\nARGUMENTS: {args}"));
    }
    s
}

/// The prompt for `/name args`, if `name` is a custom command.
pub fn expand(project: Option<&Path>, name: &str, args: &str) -> Option<String> {
    let cmd = list(project).into_iter().find(|c| c.name == name)?;
    let text = std::fs::read_to_string(&cmd.path).ok()?;
    let (_, body) = front_matter(&text);
    Some(substitute(&body, args).trim_end().to_string())
}

/// Save a prompt as a custom command (`.claude/commands/<name>.md` in the
/// project, or `~/.claude/commands/` for every project). `a:b` names go to
/// sub-folders. Returns the file written.
pub fn save(project: Option<&Path>, scope: &str, name: &str, description: &str, body: &str, overwrite: bool) -> crate::error::AppResult<PathBuf> {
    use crate::error::AppError;
    let name = name.trim().trim_start_matches('/');
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.split(':').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    if !valid {
        return Err(AppError::msg("Use letters, digits, - and _ for the command name (a:b for a folder)."));
    }
    if body.trim().is_empty() {
        return Err(AppError::msg("The command needs a prompt."));
    }
    let base = match scope {
        "user" => dirs::home_dir().ok_or_else(|| AppError::msg("No home folder."))?.join(".claude").join("commands"),
        _ => project.ok_or_else(|| AppError::msg("This conversation has no project folder."))?.join(".claude").join("commands"),
    };
    let mut path = base;
    let parts: Vec<&str> = name.split(':').collect();
    for p in &parts[..parts.len() - 1] {
        path = path.join(p);
    }
    path = path.join(format!("{}.md", parts[parts.len() - 1]));
    if path.exists() && !overwrite {
        return Err(AppError::msg(format!("/{name} already exists.")));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let desc = description.trim().replace('\n', " ");
    let text = if desc.is_empty() { format!("{}\n", body.trim()) } else { format!("---\ndescription: {desc}\n---\n{}\n", body.trim()) };
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_and_substitution() {
        let (fm, body) = front_matter("---\ndescription: Fix an issue\nargument-hint: [issue]\n---\nFix issue $1 in $ARGUMENTS\n");
        assert_eq!(fm[0], ("description".into(), "Fix an issue".into()));
        assert_eq!(body, "Fix issue $1 in $ARGUMENTS\n");
        assert_eq!(substitute(&body, "42 now"), "Fix issue 42 in 42 now\n");
        assert_eq!(substitute("Review the code", "carefully"), "Review the code\n\nARGUMENTS: carefully");
    }

    #[test]
    fn lists_project_commands_with_namespaces() {
        let dir = std::env::temp_dir().join(format!("cr-slash-{}", crate::util::new_id()));
        let cmds = dir.join(".claude/commands/git");
        std::fs::create_dir_all(&cmds).unwrap();
        std::fs::write(dir.join(".claude/commands/test.md"), "Run the tests and fix failures\n").unwrap();
        std::fs::write(cmds.join("pr.md"), "---\ndescription: Open a PR\n---\nOpen a pull request for $ARGUMENTS").unwrap();
        let l = list(Some(&dir));
        let names: Vec<&str> = l.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"test") && names.contains(&"git:pr"), "{names:?}");
        assert_eq!(l.iter().find(|c| c.name == "test").unwrap().description, "Run the tests and fix failures");
        assert_eq!(expand(Some(&dir), "git:pr", "fix-1").unwrap(), "Open a pull request for fix-1");
        assert!(expand(Some(&dir), "nope", "").is_none());
        let p = save(Some(&dir), "project", "team:deploy", "Deploy it", "Deploy $ARGUMENTS now", false).unwrap();
        assert!(p.ends_with(".claude/commands/team/deploy.md"));
        assert_eq!(expand(Some(&dir), "team:deploy", "staging").unwrap(), "Deploy staging now");
        assert!(save(Some(&dir), "project", "team:deploy", "", "x", false).is_err());
        assert!(save(Some(&dir), "project", "bad name", "", "x", false).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
