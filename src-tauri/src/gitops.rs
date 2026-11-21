//! Change tracking for code sessions: which files differ from the baseline,
//! their diffs, and reverting them.
//!
//! In a git repository the baseline is `HEAD` (so edits made by the agent,
//! by Claude Code or by hand all show up). Outside git, the baseline is the
//! original content the built-in agent recorded before its first edit of
//! each file (`CodeSession::originals`).

use crate::codetools::CodeSession;
use crate::error::{AppError, AppResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Run git in `cwd` and return stdout. Non-zero exit is an error carrying stderr.
pub async fn git(cwd: &Path, args: &[&str]) -> AppResult<String> {
    let out = git_raw(cwd, args).await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::msg(if err.is_empty() { format!("git {} failed", args.join(" ")) } else { err }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub async fn git_raw(cwd: &Path, args: &[&str]) -> AppResult<std::process::Output> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut cmd = crate::shellenv::command("git", &args);
    cmd.current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let fut = cmd.output();
    match tokio::time::timeout(std::time::Duration::from_secs(60), fut).await {
        Ok(r) => r.map_err(|e| AppError::msg(format!("Could not run git: {e}"))),
        Err(_) => Err(AppError::msg("git took too long and was stopped.")),
    }
}

/// The repository's top-level directory, if `cwd` is inside a git work tree.
pub async fn repo_root(cwd: &Path) -> Option<PathBuf> {
    let out = git_raw(cwd, &["rev-parse", "--show-toplevel"]).await.ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// Path relative to the root.
    pub path: String,
    /// M modified, A added, D deleted, R renamed, ? untracked
    pub status: char,
    pub orig_path: Option<String>,
}

/// Parse `git status --porcelain=v1 -z`.
pub fn parse_porcelain(out: &str) -> Vec<Change> {
    let mut v = vec![];
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = it.next() {
        if entry.len() < 4 {
            continue;
        }
        let (x, y) = (entry.as_bytes()[0] as char, entry.as_bytes()[1] as char);
        let path = entry[3..].to_string();
        let mut orig = None;
        let status = if x == '?' {
            '?'
        } else if x == 'R' || y == 'R' || x == 'C' {
            orig = it.next().map(|s| s.to_string());
            'R'
        } else if x == 'D' || y == 'D' {
            'D'
        } else if x == 'A' {
            'A'
        } else {
            'M'
        };
        v.push(Change { path, status, orig_path: orig });
    }
    v
}

fn count_lines(s: &str) -> usize {
    s.lines().count()
}

/// +/- line counts between two texts.
pub fn line_stats(old: &str, new: &str) -> (usize, usize) {
    let diff = similar::TextDiff::from_lines(old, new);
    let (mut a, mut d) = (0, 0);
    for c in diff.iter_all_changes() {
        match c.tag() {
            similar::ChangeTag::Insert => a += 1,
            similar::ChangeTag::Delete => d += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    (a, d)
}

fn unified(path: &str, old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

fn read_text(p: &Path) -> Option<String> {
    let m = std::fs::metadata(p).ok()?;
    if !m.is_file() || m.len() > 8 * 1024 * 1024 {
        return None;
    }
    let b = std::fs::read(p).ok()?;
    if b.iter().take(8000).any(|&c| c == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&b).to_string())
}

/// The changes of a session: `{repo, root, branch, files: [{path, status, added, removed}]}`.
pub async fn changes(session: &CodeSession) -> AppResult<Value> {
    let root = session.root.clone();
    if let Some(top) = repo_root(&root).await {
        let out = git(&top, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]).await?;
        let mut files = parse_porcelain(&out);
        let numstat = git(&top, &["diff", "HEAD", "--numstat", "-z"]).await.unwrap_or_default();
        let mut stats = std::collections::HashMap::new();
        for rec in numstat.split('\0').filter(|s| !s.is_empty()) {
            let parts: Vec<&str> = rec.splitn(3, '\t').collect();
            if parts.len() == 3 {
                stats.insert(parts[2].to_string(), (parts[0].parse::<usize>().unwrap_or(0), parts[1].parse::<usize>().unwrap_or(0)));
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let branch = git(&top, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap_or_default().trim().to_string();
        let list: Vec<Value> = files
            .iter()
            .map(|c| {
                let (a, d) = match stats.get(&c.path) {
                    Some(s) => *s,
                    None if c.status == '?' => (read_text(&top.join(&c.path)).map(|t| count_lines(&t)).unwrap_or(0), 0),
                    None => (0, 0),
                };
                json!({"path": c.path, "status": c.status.to_string(), "orig_path": c.orig_path, "added": a, "removed": d})
            })
            .collect();
        return Ok(json!({"repo": true, "root": top, "branch": branch, "files": list}));
    }
    // Not a repository: what the built-in agent changed.
    let originals = session.originals.lock().clone();
    let mut list = vec![];
    let mut paths: Vec<&PathBuf> = originals.keys().collect();
    paths.sort();
    for p in paths {
        let orig = &originals[p];
        let now = std::fs::read(p).ok();
        if now.as_ref() == orig.as_ref() {
            continue;
        }
        let status = match (orig, &now) {
            (None, Some(_)) => "A",
            (Some(_), None) => "D",
            _ => "M",
        };
        let old = orig.as_ref().map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_default();
        let new = now.as_ref().map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_default();
        let (a, d) = line_stats(&old, &new);
        let rel = p.strip_prefix(&root).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| p.to_string_lossy().to_string());
        list.push(json!({"path": rel, "status": status, "added": a, "removed": d}));
    }
    Ok(json!({"repo": false, "root": root, "branch": null, "files": list}))
}

fn abs_in(root: &Path, rel: &str) -> AppResult<PathBuf> {
    let p = Path::new(rel);
    let full = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    if full.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(AppError::msg("Invalid path."));
    }
    Ok(full)
}

/// Unified diff of one file against the baseline.
pub async fn file_diff(session: &CodeSession, path: &str) -> AppResult<String> {
    if let Some(top) = repo_root(&session.root).await {
        let full = abs_in(&top, path)?;
        let tracked = git_raw(&top, &["--literal-pathspecs", "ls-files", "--error-unmatch", "--", path]).await.map(|o| o.status.success()).unwrap_or(false);
        if tracked {
            let d = git(&top, &["--literal-pathspecs", "diff", "HEAD", "--no-color", "--no-ext-diff", "--", path]).await?;
            if !d.trim().is_empty() {
                return Ok(d);
            }
        }
        let new = read_text(&full).unwrap_or_else(|| if full.exists() { "(binary file)\n".into() } else { String::new() });
        return Ok(unified(path, "", &new));
    }
    let full = abs_in(&session.root, path)?;
    let orig = session.originals.lock().get(&full).cloned().flatten();
    let old = orig.map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    let new = read_text(&full).unwrap_or_default();
    Ok(unified(path, &old, &new))
}

/// Put one file back to the baseline (deleting it if it didn't exist).
pub async fn revert_file(session: &CodeSession, path: &str) -> AppResult<()> {
    if let Some(top) = repo_root(&session.root).await {
        let full = abs_in(&top, path)?;
        let in_head = git_raw(&top, &["cat-file", "-e", &format!("HEAD:{path}")]).await.map(|o| o.status.success()).unwrap_or(false);
        if in_head {
            // restore index and work tree from HEAD
            let r = git_raw(&top, &["--literal-pathspecs", "restore", "--source=HEAD", "--staged", "--worktree", "--", path]).await?;
            if !r.status.success() {
                git(&top, &["--literal-pathspecs", "checkout", "HEAD", "--", path]).await?;
            }
        } else {
            let _ = git_raw(&top, &["--literal-pathspecs", "rm", "--cached", "--quiet", "--force", "--", path]).await;
            if full.is_file() {
                std::fs::remove_file(&full)?;
            }
        }
        session.originals.lock().remove(&full);
        return Ok(());
    }
    let full = abs_in(&session.root, path)?;
    let orig = session.originals.lock().get(&full).cloned();
    match orig {
        Some(Some(bytes)) => std::fs::write(&full, bytes)?,
        Some(None) => {
            if full.exists() {
                std::fs::remove_file(&full)?;
            }
        }
        None => return Err(AppError::msg("No earlier version of this file is known.")),
    }
    session.originals.lock().remove(&full);
    Ok(())
}

/// `git blame --porcelain` output as `{commits: [{sha, author, time,
/// summary}], lines: [index into commits, one per line]}`. Lines not
/// committed yet point at a commit with an all-zero sha.
pub fn parse_blame(out: &str) -> Value {
    let mut commits: Vec<Value> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut lines: Vec<usize> = Vec::new();
    let mut cur: Option<usize> = None;
    for l in out.lines() {
        if l.starts_with('\t') {
            if let Some(i) = cur {
                lines.push(i);
            }
            continue;
        }
        let mut parts = l.split(' ');
        let first = parts.next().unwrap_or("");
        if first.len() == 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            let i = *index.entry(first.to_string()).or_insert_with(|| {
                commits.push(json!({"sha": first, "author": "", "time": 0, "summary": ""}));
                commits.len() - 1
            });
            cur = Some(i);
            continue;
        }
        let Some(i) = cur else { continue };
        let rest = l.split_once(' ').map(|(_, r)| r).unwrap_or("");
        match first {
            "author" => commits[i]["author"] = json!(rest),
            "author-time" => commits[i]["time"] = json!(rest.parse::<i64>().unwrap_or(0) * 1000),
            "summary" => commits[i]["summary"] = json!(rest),
            _ => {}
        }
    }
    json!({"commits": commits, "lines": lines})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blame_porcelain() {
        let a = "a".repeat(40);
        let z = "0".repeat(40);
        let out = format!(
            "{a} 1 1 2\nauthor Ada\nauthor-time 1700000000\nsummary Add parser\nfilename x.rs\n\tfn a() {{}}\n{a} 2 2\n\tfn b() {{}}\n{z} 3 3 1\nauthor Not Committed Yet\nauthor-time 1800000000\nsummary Version of x.rs from x.rs\nfilename x.rs\n\tnew line\n"
        );
        let v = parse_blame(&out);
        assert_eq!(v["lines"], json!([0, 0, 1]));
        assert_eq!(v["commits"][0]["author"], "Ada");
        assert_eq!(v["commits"][0]["time"], 1_700_000_000_000i64);
        assert_eq!(v["commits"][0]["summary"], "Add parser");
        assert_eq!(v["commits"][1]["sha"], z);
    }

    #[test]
    fn porcelain_parsing() {
        let out = " M src/a.rs\0?? new.txt\0R  b.rs\0old_b.rs\0 D gone.rs\0A  added.rs\0";
        let v = parse_porcelain(out);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], Change { path: "src/a.rs".into(), status: 'M', orig_path: None });
        assert_eq!(v[1].status, '?');
        assert_eq!(v[2], Change { path: "b.rs".into(), status: 'R', orig_path: Some("old_b.rs".into()) });
        assert_eq!(v[3].status, 'D');
        assert_eq!(v[4].status, 'A');
        assert_eq!(line_stats("a\nb\n", "a\nc\nd\n"), (2, 1));
    }

    #[tokio::test]
    async fn git_repo_changes_diff_and_revert() {
        if which::which("git").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cr-git-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            git(&dir, &args).await.unwrap();
        }
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git(&dir, &["add", "."]).await.unwrap();
        git(&dir, &["commit", "-q", "-m", "init"]).await.unwrap();
        std::fs::write(dir.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        let s = CodeSession::new(dir.clone());
        let c = changes(&s).await.unwrap();
        assert_eq!(c["repo"], true);
        let files = c["files"].as_array().unwrap();
        assert_eq!(files.len(), 2, "{c}");
        assert_eq!(files[0]["path"], "a.txt");
        assert_eq!(files[0]["added"], 1);
        assert_eq!(files[1]["status"], "?");
        let d = file_diff(&s, "a.txt").await.unwrap();
        assert!(d.contains("+TWO") && d.contains("-two"), "{d}");
        let d2 = file_diff(&s, "b.txt").await.unwrap();
        assert!(d2.contains("+new"));
        revert_file(&s, "a.txt").await.unwrap();
        revert_file(&s, "b.txt").await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\ntwo\n");
        assert!(!dir.join("b.txt").exists());
        assert_eq!(changes(&s).await.unwrap()["files"].as_array().unwrap().len(), 0);
        assert!(file_diff(&s, "../x").await.is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn non_repo_uses_recorded_originals() {
        let dir = std::env::temp_dir().join(format!("cr-norepo-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.txt");
        std::fs::write(&f, "old\n").unwrap();
        let s = CodeSession::new(dir.clone());
        s.remember_original(&f);
        s.remember_original(&dir.join("new.txt"));
        std::fs::write(&f, "new\n").unwrap();
        std::fs::write(dir.join("new.txt"), "hi\n").unwrap();
        let c = changes(&s).await.unwrap();
        if c["repo"] == true {
            // temp dir happens to be inside a repository; nothing to check
            let _ = std::fs::remove_dir_all(dir);
            return;
        }
        assert_eq!(c["files"].as_array().unwrap().len(), 2, "{c}");
        revert_file(&s, "x.txt").await.unwrap();
        revert_file(&s, "new.txt").await.unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "old\n");
        assert!(!dir.join("new.txt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
