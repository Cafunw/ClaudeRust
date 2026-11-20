//! Isolated git worktrees for code sessions, so sessions working on the same
//! repository in parallel don't step on each other.
//!
//! A session's worktree lives under the app's data folder
//! (`worktrees/<repo>-<id>`) on its own branch `clauderust/<id>`, started
//! from the repository's current `HEAD`. Deleting the conversation removes
//! the worktree; the branch is deleted too unless it has commits of its own.

use crate::error::{AppError, AppResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> AppResult<String> {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .args(args)
        .env("PATH", crate::shellenv::path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().map_err(|e| AppError::msg(format!("Could not run git: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::msg(if err.is_empty() { format!("git {} failed", args.join(" ")) } else { err }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Top level of the repository containing `dir`, if any.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).ok().filter(|s| !s.is_empty()).map(PathBuf::from)
}

fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    s.trim_matches('-').chars().take(40).collect()
}

/// Create a worktree for a session started in `cwd`. Returns the session's
/// working directory inside the worktree and the info to store.
pub fn create(cwd: &Path, data_dir: &Path, id: &str) -> AppResult<(PathBuf, Value)> {
    let root = repo_root(cwd).ok_or_else(|| AppError::msg("This folder is not in a git repository, so it can't get its own worktree."))?;
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let rel = std::fs::canonicalize(cwd)
        .ok()
        .and_then(|c| c.strip_prefix(&root).ok().map(|r| r.to_path_buf()))
        .unwrap_or_default();
    let base = git(&root, &["rev-parse", "HEAD"]).map_err(|_| AppError::msg("The repository has no commits yet; make a first commit to use worktrees."))?;
    let base_branch = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let repo_name = sanitize(&root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into()));
    let slug: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    let slug = if slug.is_empty() { "session".to_string() } else { slug };
    let dir = data_dir.join("worktrees").join(format!("{repo_name}-{slug}"));
    std::fs::create_dir_all(dir.parent().unwrap_or(data_dir))?;
    let branch = format!("clauderust/{slug}");
    git(&root, &["worktree", "add", "-b", &branch, &dir.to_string_lossy(), &base])?;
    let session_cwd = if rel.as_os_str().is_empty() { dir.clone() } else { dir.join(&rel) };
    let info = json!({
        "path": dir,
        "branch": branch,
        "repo_root": root,
        "base": base,
        "base_branch": base_branch,
        "orig_cwd": cwd,
    });
    Ok((session_cwd, info))
}

/// Remove a session's worktree; delete its branch unless it has commits
/// that aren't on the base. Returns a note when the branch was kept.
pub fn remove(info: &Value) -> AppResult<Option<String>> {
    let (Some(path), Some(root), Some(branch)) = (info["path"].as_str(), info["repo_root"].as_str(), info["branch"].as_str()) else {
        return Ok(None);
    };
    let root = Path::new(root);
    if !root.is_dir() {
        let _ = std::fs::remove_dir_all(path);
        return Ok(None);
    }
    if Path::new(path).exists() {
        if git(root, &["worktree", "remove", "--force", path]).is_err() {
            let _ = std::fs::remove_dir_all(path);
            let _ = git(root, &["worktree", "prune"]);
        }
    } else {
        let _ = git(root, &["worktree", "prune"]);
    }
    let base = info["base"].as_str().unwrap_or("HEAD");
    let own = git(root, &["rev-list", "--count", &format!("{base}..{branch}")]).ok().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
    if own > 0 {
        return Ok(Some(format!("Kept branch {branch}: it has {own} commit(s) of its own.")));
    }
    let _ = git(root, &["branch", "-D", branch]);
    Ok(None)
}

/// Apply the worktree's changes (committed and uncommitted, new files
/// included) to the original working tree with a 3-way apply.
pub fn apply_to_origin(info: &Value) -> AppResult<String> {
    let path = Path::new(info["path"].as_str().ok_or_else(|| AppError::msg("No worktree."))?);
    let root = Path::new(info["repo_root"].as_str().ok_or_else(|| AppError::msg("No repository."))?);
    let base = info["base"].as_str().unwrap_or("HEAD");
    // Stage everything (intent-to-add for new files) so one diff covers it.
    git(path, &["add", "-A", "--intent-to-add"])?;
    let patch = git(path, &["diff", "--binary", base])?;
    if patch.trim().is_empty() {
        return Ok("There are no changes to apply.".into());
    }
    let tmp = std::env::temp_dir().join(format!("clauderust-{}.patch", crate::util::new_id()));
    std::fs::write(&tmp, format!("{patch}\n"))?;
    let r = git(root, &["apply", "--3way", "--whitespace=nowarn", &tmp.to_string_lossy()]);
    let _ = std::fs::remove_file(&tmp);
    r?;
    let files = patch.lines().filter(|l| l.starts_with("diff --git")).count();
    Ok(format!("Applied changes to {} file(s) in {}.", files, root.display()))
}

/// Commit everything in the worktree on its branch.
pub fn commit_all(info: &Value, message: &str) -> AppResult<String> {
    let path = Path::new(info["path"].as_str().ok_or_else(|| AppError::msg("No worktree."))?);
    git(path, &["add", "-A"])?;
    let status = git(path, &["status", "--porcelain"])?;
    if status.is_empty() {
        return Ok("Nothing to commit.".into());
    }
    git(path, &["commit", "-q", "-m", message])?;
    let sha = git(path, &["rev-parse", "--short", "HEAD"])?;
    Ok(format!("Committed {sha} on {}.", info["branch"].as_str().unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cr-wt-{}", crate::util::new_id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        git(&dir, &["config", "user.email", "t@example.com"]).unwrap();
        git(&dir, &["config", "user.name", "t"]).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.join("sub/b.txt"), "b\n").unwrap();
        git(&dir, &["add", "."]).unwrap();
        git(&dir, &["commit", "-q", "-m", "init"]).unwrap();
        dir
    }

    #[test]
    fn create_apply_and_remove() {
        if which::which("git").is_err() {
            return;
        }
        let repo = init_repo();
        let data = std::env::temp_dir().join(format!("cr-wtdata-{}", crate::util::new_id()));
        let (cwd, info) = create(&repo.join("sub"), &data, "abcd1234-xyz").unwrap();
        assert!(cwd.ends_with("sub") && cwd.join("b.txt").exists(), "{cwd:?}");
        let wt = PathBuf::from(info["path"].as_str().unwrap());
        // change in the worktree doesn't touch the original
        std::fs::write(wt.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(wt.join("new.txt"), "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one\n");
        let msg = apply_to_origin(&info).unwrap();
        assert!(msg.contains("2 file"), "{msg}");
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one\ntwo\n");
        assert!(repo.join("new.txt").exists());
        // uncommitted only: branch is deleted with the worktree
        assert_eq!(remove(&info).unwrap(), None);
        assert!(!wt.exists());
        assert!(git(&repo, &["rev-parse", "--verify", info["branch"].as_str().unwrap()]).is_err());
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn branch_with_commits_is_kept() {
        if which::which("git").is_err() {
            return;
        }
        let repo = init_repo();
        let data = std::env::temp_dir().join(format!("cr-wtdata-{}", crate::util::new_id()));
        let (_, info) = create(&repo, &data, "ffff0000").unwrap();
        let wt = PathBuf::from(info["path"].as_str().unwrap());
        git(&wt, &["config", "user.email", "t@example.com"]).unwrap();
        git(&wt, &["config", "user.name", "t"]).unwrap();
        std::fs::write(wt.join("c.txt"), "c\n").unwrap();
        assert!(commit_all(&info, "work").unwrap().starts_with("Committed"));
        let note = remove(&info).unwrap();
        assert!(note.unwrap().contains("Kept branch"));
        assert!(git(&repo, &["rev-parse", "--verify", info["branch"].as_str().unwrap()]).is_ok());
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&data);
    }
}
