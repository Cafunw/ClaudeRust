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
    git_raw_env(cwd, args, &[]).await
}

/// `git_raw` with extra environment variables (e.g. `GIT_INDEX_FILE`).
async fn git_raw_env(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> AppResult<std::process::Output> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut cmd = crate::shellenv::command("git", &args);
    cmd.current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for (k, v) in envs {
        cmd.env(k, v);
    }
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
        let base = base_rev(&top).await;
        let numstat = git(&top, &["diff", base, "--numstat", "-z"]).await.unwrap_or_default();
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
            let base = base_rev(&top).await;
            let d = git(&top, &["--literal-pathspecs", "diff", base, "--no-color", "--no-ext-diff", "--no-textconv", PREFIX_A, PREFIX_B, "--", path]).await?;
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

/// The patch for hunk `index` of a unified diff (the file header plus that
/// hunk), when its `@@` line is still `expect` (the hunk the user saw).
pub fn hunk_patch(diff: &str, index: usize, expect: &str) -> Option<String> {
    // lines keep their endings: a file with CRLF line endings has `\r\n`
    // in its diff, and the patch must keep them to apply
    let lines: Vec<&str> = diff.split_inclusive('\n').collect();
    let first = lines.iter().position(|l| l.starts_with("@@"))?;
    let starts: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.starts_with("@@")).map(|(i, _)| i).collect();
    let start = *starts.get(index)?;
    if lines[start].trim() != expect.trim() {
        return None;
    }
    let end = starts.get(index + 1).copied().unwrap_or(lines.len());
    let mut out = lines[..first].concat();
    out.push_str(&lines[start..end].concat());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// Undo one hunk of a tracked file's uncommitted changes (`git apply -R`).
pub async fn revert_hunk(session: &CodeSession, path: &str, index: usize, expect: &str) -> AppResult<()> {
    let top = repo_root(&session.root).await.ok_or_else(|| AppError::msg("Reverting part of a file needs a git repository."))?;
    let tracked = git_raw(&top, &["--literal-pathspecs", "ls-files", "--error-unmatch", "--", path]).await.map(|o| o.status.success()).unwrap_or(false);
    if !tracked {
        return Err(AppError::msg("This is a new file: revert it as a whole."));
    }
    let base = base_rev(&top).await;
    let diff = git(&top, &["--literal-pathspecs", "diff", base, "--no-color", "--no-ext-diff", "--no-textconv", PREFIX_A, PREFIX_B, "--", path]).await?;
    let patch = hunk_patch(&diff, index, expect).ok_or_else(|| AppError::msg("The file changed since the diff was shown. Refresh and try again."))?;
    // when the whole file is staged as it is, the hunk is unstaged too (else
    // a later `git commit` would still commit it); a partly staged file's
    // index is left alone
    let staged_as_is = git_raw(&top, &["--literal-pathspecs", "diff", "--quiet", "--", path]).await.map(|o| o.status.success()).unwrap_or(false)
        && !git_raw(&top, &["--literal-pathspecs", "diff", "--cached", "--quiet", base, "--", path]).await.map(|o| o.status.success()).unwrap_or(true);
    let tmp = std::env::temp_dir().join(format!("clauderust-hunk-{}.patch", crate::util::new_id()));
    std::fs::write(&tmp, patch)?;
    let p = tmp.to_string_lossy().to_string();
    let r = git(&top, &["apply", "-R", "--whitespace=nowarn", &p]).await;
    if r.is_ok() && staged_as_is {
        let _ = git(&top, &["apply", "-R", "--cached", "--whitespace=nowarn", &p]).await;
    }
    let _ = std::fs::remove_file(&tmp);
    r.map(|_| ())
}

/// Diff prefixes, given explicitly so a user's `diff.noprefix` or
/// `diff.mnemonicPrefix` can't change the paths `git apply` sees.
const PREFIX_A: &str = "--src-prefix=a/";
const PREFIX_B: &str = "--dst-prefix=b/";
/// The empty tree: the baseline of a repository with no commits yet.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// What changes are measured against: `HEAD`, or the empty tree before the
/// first commit (where `git diff HEAD` fails).
pub async fn base_rev(root: &Path) -> &'static str {
    let has_head = git_raw(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await.map(|o| o.status.success()).unwrap_or(false);
    if has_head {
        "HEAD"
    } else {
        EMPTY_TREE
    }
}

/// `git diff HEAD <args>` (against the empty tree before the first commit)
/// including files git doesn't track yet. They're marked intent-to-add in a
/// copy of the index, so the real index is never touched: nothing is left
/// behind, and an add or commit running at the same time isn't disturbed.
pub async fn diff_including_new(root: &Path, args: &[&str]) -> AppResult<String> {
    let base = base_rev(root).await;
    let untracked: Vec<String> = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])
        .await
        .unwrap_or_default()
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let tmp_index = std::env::temp_dir().join(format!("clauderust-index-{}", crate::util::new_id()));
    let tmp_s = tmp_index.to_string_lossy().to_string();
    if !untracked.is_empty() {
        // relative to `root`, or absolute (e.g. in a linked worktree)
        let index = git(root, &["rev-parse", "--git-path", "index"]).await.unwrap_or_default();
        let index = root.join(index.trim_end_matches(['\r', '\n']));
        if index.is_file() {
            std::fs::copy(&index, &tmp_index)?;
        }
    }
    let env: Vec<(&str, &str)> = if untracked.is_empty() { vec![] } else { vec![("GIT_INDEX_FILE", tmp_s.as_str())] };
    let add = |files: Vec<String>| {
        let env = env.clone();
        async move {
            let mut a: Vec<&str> = vec!["--literal-pathspecs", "add", "--intent-to-add", "--"];
            a.extend(files.iter().map(String::as_str));
            git_raw_env(root, &a, &env).await.map(|o| o.status.success()).unwrap_or(false)
        }
    };
    for chunk in untracked.chunks(200) {
        // a file that vanished since the listing fails its whole chunk:
        // then add the rest one by one
        if !add(chunk.to_vec()).await {
            for f in chunk {
                add(vec![f.clone()]).await;
            }
        }
    }
    let mut full = vec!["diff", base, "--no-textconv", PREFIX_A, PREFIX_B];
    full.extend_from_slice(args);
    let out = git_raw_env(root, &full, &env).await;
    let _ = std::fs::remove_file(&tmp_index);
    let out = out?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::msg(if err.is_empty() { "git diff failed".to_string() } else { err }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Files only marked intent-to-add (left behind by older versions), unmarked.
pub async fn clear_intent_to_add(root: &Path) {
    let st = git(root, &["status", "--porcelain", "-z"]).await.unwrap_or_default();
    let ita: Vec<&str> = st.split('\0').filter(|e| e.starts_with(" A ")).map(|e| &e[3..]).collect();
    if ita.is_empty() {
        return;
    }
    let mut a = vec!["--literal-pathspecs", "reset", "-q", "--"];
    a.extend(ita);
    let _ = git(root, &a).await;
}

/// Apply a unified diff to the working tree of `root`: checked first, so a
/// patch that doesn't fit changes nothing. Returns the files it touches.
pub async fn apply_patch(root: &Path, patch: &str) -> AppResult<Vec<String>> {
    let with_nl = |mut t: String| {
        if !t.ends_with('\n') {
            t.push('\n');
        }
        t
    };
    let lf = with_nl(patch.replace("\r\n", "\n"));
    if !lf.lines().any(|l| l.starts_with("@@") || l.starts_with("GIT binary patch") || l.starts_with("rename from")) {
        return Err(AppError::msg("That doesn't look like a patch (no @@ hunks)."));
    }
    // as given first (a patch for a file with CRLF line endings needs its
    // `\r`s); with LF endings when that doesn't fit (a patch whose line
    // endings were changed by copying it around)
    let raw = with_nl(patch.to_string());
    let mut tries = vec![raw.clone()];
    if lf != raw {
        tries.push(lf);
    }
    let mut last_err = None;
    for text in tries {
        let tmp = std::env::temp_dir().join(format!("clauderust-apply-{}.patch", crate::util::new_id()));
        std::fs::write(&tmp, &text)?;
        let p = tmp.to_string_lossy().to_string();
        let r: AppResult<Vec<String>> = async {
            let files = git(root, &["apply", "--numstat", &p]).await.map_err(|e| AppError::msg(format!("Not a valid patch: {e}")))?;
            git(root, &["apply", "--check", "--whitespace=nowarn", &p]).await.map_err(|e| AppError::msg(format!("The patch doesn't apply here: {e}")))?;
            git(root, &["apply", "--whitespace=nowarn", &p]).await?;
            Ok(files.lines().filter_map(|l| l.splitn(3, '\t').nth(2).map(str::to_string)).collect())
        }
        .await;
        let _ = std::fs::remove_file(&tmp);
        match r {
            Ok(files) => return Ok(files),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::msg("The patch doesn't apply here.")))
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
    fn hunk_patches() {
        let diff = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n-a\n+A\n b\n@@ -9,2 +9,2 @@ fn x\n y\n-z\n+Z\n";
        let p = hunk_patch(diff, 1, "@@ -9,2 +9,2 @@ fn x").unwrap();
        assert_eq!(p, "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -9,2 +9,2 @@ fn x\n y\n-z\n+Z\n");
        assert!(hunk_patch(diff, 1, "@@ -9,3 +9,3 @@").is_none(), "stale header");
        assert!(hunk_patch(diff, 2, "").is_none());
    }

    #[tokio::test]
    async fn revert_one_hunk() {
        let d = std::env::temp_dir().join(format!("cr-hunk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let run = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&d).output().unwrap().status.success());
        run(&["init", "-q"]);
        let body: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        std::fs::write(d.join("f.txt"), &body).unwrap();
        run(&["add", "."]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
        std::fs::write(d.join("f.txt"), body.replace("line 2\n", "LINE TWO\n").replace("line 28\n", "LINE 28\n")).unwrap();
        let s = CodeSession::new(d.clone());
        let diff = file_diff(&s, "f.txt").await.unwrap();
        let second = diff.lines().filter(|l| l.starts_with("@@")).nth(1).unwrap().to_string();
        revert_hunk(&s, "f.txt", 1, &second).await.unwrap();
        let now = std::fs::read_to_string(d.join("f.txt")).unwrap();
        assert!(now.contains("LINE TWO") && now.contains("line 28\n") && !now.contains("LINE 28"), "{now}");
        assert!(revert_hunk(&s, "f.txt", 1, &second).await.is_err(), "that hunk is gone");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn new_files_in_diffs_leave_no_marks() {
        let d = std::env::temp_dir().join(format!("cr-ita-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let run = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&d).output().unwrap();
        run(&["init", "-q"]);
        std::fs::write(d.join("a.txt"), "a\n").unwrap();
        run(&["add", "."]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
        std::fs::write(d.join("a.txt"), "A\n").unwrap();
        std::fs::write(d.join("new file.txt"), "n\n").unwrap();
        let diff = diff_including_new(&d, &["--no-color"]).await.unwrap();
        assert!(diff.contains("+++ b/new file.txt") && diff.contains("+A"));
        let st = String::from_utf8_lossy(&run(&["status", "--porcelain"]).stdout).to_string();
        assert!(st.contains("?? \"new file.txt\"") || st.contains("?? new file.txt"), "back to untracked: {st}");
        // stash works afterwards
        assert!(run(&["stash", "push", "--include-untracked", "-m", "x"]).status.success());
        // leftovers from older versions are cleared
        run(&["stash", "pop"]);
        run(&["add", "-N", "new file.txt"]);
        clear_intent_to_add(&d).await;
        let st = String::from_utf8_lossy(&run(&["status", "--porcelain"]).stdout).to_string();
        assert!(!st.contains(" A "), "{st}");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn test_repo(name: &str) -> (PathBuf, impl Fn(&[&str]) -> std::process::Output) {
        let d = std::env::temp_dir().join(format!("cr-{name}-{}", crate::util::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        let dd = d.clone();
        let run = move |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&dd).output().unwrap();
        run(&["init", "-q"]);
        run(&["config", "user.name", "t"]);
        run(&["config", "user.email", "t@t"]);
        (d, run)
    }

    #[tokio::test]
    async fn diffs_before_the_first_commit() {
        let (d, run) = test_repo("empty");
        std::fs::write(d.join("staged.txt"), "s\n").unwrap();
        run(&["add", "staged.txt"]);
        std::fs::write(d.join(".env"), "API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789\n").unwrap();
        let diff = diff_including_new(&d, &["--no-color"]).await.unwrap();
        assert!(diff.contains("+++ b/staged.txt") && diff.contains("+++ b/.env"), "{diff}");
        // the real index is untouched: .env is still untracked, staged.txt still staged
        let st = String::from_utf8_lossy(&run(&["status", "--porcelain"]).stdout).to_string();
        assert!(st.contains("?? .env") && st.contains("A  staged.txt"), "{st}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn hunks_with_noprefix_crlf_and_staging() {
        let (d, run) = test_repo("hunk2");
        run(&["config", "diff.noprefix", "true"]);
        run(&["config", "core.autocrlf", "false"]);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("f.txt"), "top\n").unwrap();
        let body: String = (1..=30).map(|i| format!("line {i}\r\n")).collect();
        std::fs::write(d.join("src/f.txt"), &body).unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(d.join("src/f.txt"), body.replace("line 2\r\n", "LINE TWO\r\n").replace("line 28\r\n", "LINE 28\r\n")).unwrap();
        run(&["add", "src/f.txt"]);
        let s = CodeSession::new(d.clone());
        let diff = file_diff(&s, "src/f.txt").await.unwrap();
        let second = diff.lines().filter(|l| l.starts_with("@@")).nth(1).unwrap().to_string();
        revert_hunk(&s, "src/f.txt", 1, &second).await.unwrap();
        let now = std::fs::read_to_string(d.join("src/f.txt")).unwrap();
        assert!(now.contains("LINE TWO\r\n") && now.contains("line 28\r\n") && !now.contains("LINE 28"), "{now}");
        assert_eq!(std::fs::read_to_string(d.join("f.txt")).unwrap(), "top\n", "the top-level file is untouched");
        // it was staged as a whole, so it's unstaged too
        let staged = String::from_utf8_lossy(&run(&["diff", "--cached"]).stdout).to_string();
        assert!(staged.contains("LINE TWO") && !staged.contains("LINE 28"), "{staged}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn crlf_patches_apply() {
        let (d, run) = test_repo("crlfp");
        run(&["config", "core.autocrlf", "false"]);
        std::fs::write(d.join("w.txt"), "one\r\ntwo\r\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        let patch = "diff --git a/w.txt b/w.txt\n--- a/w.txt\n+++ b/w.txt\n@@ -1,2 +1,2 @@\n one\r\n-two\r\n+TWO\r\n";
        apply_patch(&d, patch).await.unwrap();
        assert_eq!(std::fs::read_to_string(d.join("w.txt")).unwrap(), "one\r\nTWO\r\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn applying_patches() {
        let d = std::env::temp_dir().join(format!("cr-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let run = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&d).output().unwrap().status.success());
        run(&["init", "-q"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        run(&["add", "."]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
        let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\n";
        let files = apply_patch(&d, patch).await.unwrap();
        assert_eq!(files, vec!["a.txt", "new.txt"]);
        assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "one\nTWO\nthree\n");
        assert!(d.join("new.txt").exists());
        // applying it again doesn't fit any more, and changes nothing
        assert!(apply_patch(&d, patch).await.unwrap_err().to_string().contains("doesn't apply"));
        assert!(apply_patch(&d, "just some text").await.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

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
