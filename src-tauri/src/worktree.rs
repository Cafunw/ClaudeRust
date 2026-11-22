//! Isolated git worktrees for code sessions, so sessions working on the same
//! repository in parallel don't step on each other.
//!
//! A session's worktree lives under the app's data folder
//! (`worktrees/<repo>-<id>`) on its own branch `clauderust/<id>`, started
//! from the repository's current `HEAD`. Deleting the conversation removes
//! the worktree; the branch is deleted too unless it has commits of its own.

use crate::error::{coded_notice, AppError, AppResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) -> AppResult<String> {
    git_bytes(dir, args).map(|b| String::from_utf8_lossy(&b).trim().to_string())
}

/// git's raw stdout (patches must not be trimmed or re-encoded). Bounded in
/// time like every other git call (`gitops::git_timeout`): `worktree add`
/// can run hooks and LFS downloads, and must not hang a session's start.
fn git_bytes(dir: &Path, args: &[&str]) -> AppResult<Vec<u8>> {
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
    let limit = crate::gitops::git_timeout(args);
    let out = output_within(c, limit)
        .map_err(crate::gitops::git_unavailable)?
        .ok_or_else(|| crate::gitops::git_timed_out(args, limit, format!("git {} didn't finish within {} minutes and was stopped.", args.join(" "), limit.as_secs().div_ceil(60))))?;
    if !out.status.success() {
        return Err(crate::gitops::git_failed(args, &out.stderr));
    }
    Ok(out.stdout)
}

/// Run `cmd` and collect its output, or stop it and everything it started
/// once `limit` has passed (`Ok(None)`): on Unix SIGTERM first, so git can
/// remove its lock files, then SIGKILL two seconds later.
fn output_within(mut cmd: Command, limit: Duration) -> std::io::Result<Option<std::process::Output>> {
    use std::io::Read;
    use std::process::Stdio;
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    // both pipes drained meanwhile, or a chatty command blocks on a full one
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(child.stdout.take().map(|r| Box::new(r) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|r| Box::new(r) as Box<dyn Read + Send>));
    let wait_until = |child: &mut std::process::Child, deadline: Instant| -> std::io::Result<Option<std::process::ExitStatus>> {
        let mut pause = Duration::from_millis(1);
        loop {
            if let Some(s) = child.try_wait()? {
                return Ok(Some(s));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_millis(50));
        }
    };
    let status = match wait_until(&mut child, Instant::now() + limit)? {
        Some(s) => s,
        None => {
            #[cfg(unix)]
            {
                crate::codetools::signal_process_group(child.id(), libc::SIGTERM);
                if wait_until(&mut child, Instant::now() + Duration::from_secs(2))?.is_none() {
                    crate::codetools::kill_process_group(child.id());
                }
            }
            #[cfg(windows)]
            crate::codetools::kill_process_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
    };
    Ok(Some(std::process::Output { status, stdout: stdout.join().unwrap_or_default(), stderr: stderr.join().unwrap_or_default() }))
}

/// Top level of the repository containing `dir`, if any.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).ok().filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// A folder's name as part of another: its letters and digits in any script, runs of anything else one '-'.
fn sanitize(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        let c = if c.is_alphanumeric() || c == '_' { c } else { '-' };
        if !(c == '-' && s.ends_with('-')) {
            s.push(c);
        }
    }
    s.trim_matches('-').chars().take(40).collect()
}

/// Create a worktree for a session started in `cwd`. Returns the session's
/// working directory inside the worktree and the info to store.
pub fn create(cwd: &Path, data_dir: &Path, id: &str) -> AppResult<(PathBuf, Value)> {
    let root = repo_root(cwd).ok_or_else(|| AppError::coded("worktree_not_git", Value::Null, "This folder is not in a git repository, so it can't get its own worktree."))?;
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let rel = std::fs::canonicalize(cwd)
        .ok()
        .and_then(|c| c.strip_prefix(&root).ok().map(|r| r.to_path_buf()))
        .unwrap_or_default();
    let base = git(&root, &["rev-parse", "HEAD"]).map_err(|_| AppError::coded("worktree_no_commits", Value::Null, "The repository has no commits yet; make a first commit to use worktrees."))?;
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

/// Commit everything uncommitted in the worktree at `wt` on its branch.
/// Returns whether a commit was made; an error when changes remain that
/// could not be saved (or the state can't be read), so the caller keeps the
/// worktree.
fn save_uncommitted(wt: &Path) -> AppResult<bool> {
    let kept = |why: String| AppError::coded("worktree_kept", json!({"path": wt}), format!("The worktree at {} was kept: {why}", wt.display()));
    let status = git(wt, &["status", "--porcelain"]).map_err(|e| kept(format!("its changes could not be checked ({e}).")))?;
    if status.is_empty() {
        return Ok(false);
    }
    // --ignore-errors: an unreadable file doesn't stop the others from
    // being saved; whatever is left over is caught by the check below
    let added = git(wt, &["add", "-A", "--ignore-errors"]);
    let committed = git(
        wt,
        &["-c", "user.name=ClaudeRust", "-c", "user.email=noreply@clauderust.local", "-c", "commit.gpgsign=false",
          "commit", "-q", "--no-verify", "-m", "Uncommitted work from a ClaudeRust session (saved when the session was deleted)"],
    );
    match git(wt, &["status", "--porcelain"]) {
        Ok(s) if s.is_empty() => Ok(committed.is_ok()),
        Ok(s) => {
            let reason = committed.err().or(added.err()).map(|e| e.to_string()).unwrap_or_else(|| "some files could not be committed".into());
            let first: Vec<&str> = s.lines().take(5).collect();
            Err(kept(format!("not all of its uncommitted changes could be saved ({reason}). Still uncommitted: {}", first.join(", "))))
        }
        Err(e) => Err(kept(format!("its changes could not be checked after saving them ({e})."))),
    }
}

/// Remove a session's worktree; delete its branch unless it has commits
/// that aren't on the base. Returns a note (`error::coded_notice`) when the branch was kept.
///
/// Uncommitted work is never thrown away: it's committed to the session's
/// branch first (which is then kept). When that isn't possible, or the
/// repository can't be found (a disconnected drive), the worktree stays
/// where it is and the error says where.
pub fn remove(info: &Value) -> AppResult<Option<Value>> {
    let (Some(path), Some(root), Some(branch)) = (info["path"].as_str(), info["repo_root"].as_str(), info["branch"].as_str()) else {
        return Ok(None);
    };
    let root = Path::new(root);
    let wt = Path::new(path);
    if !root.is_dir() {
        if std::fs::symlink_metadata(wt).is_ok() {
            return Err(AppError::coded("worktree_repo_missing", json!({"repo": root, "path": wt}), format!(
                "The repository {} can't be found (is its drive connected?), so the session's worktree at {} was kept.",
                root.display(),
                wt.display()
            )));
        }
        return Ok(None);
    }
    let saved = if wt.is_dir() { save_uncommitted(wt)? } else { false };
    if Path::new(path).exists() {
        if git(root, &["worktree", "remove", "--force", path]).is_err() {
            let _ = std::fs::remove_dir_all(path);
            let _ = git(root, &["worktree", "prune"]);
        }
    } else {
        let _ = git(root, &["worktree", "prune"]);
    }
    if saved {
        return Ok(Some(coded_notice("worktree_saved", json!({"branch": branch}), format!("Saved the session's uncommitted changes on branch {branch}."))));
    }
    if git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_err() {
        return Ok(None); // already gone
    }
    let base = info["base"].as_str().unwrap_or("HEAD");
    match git(root, &["rev-list", "--count", &format!("{base}..{branch}")]).ok().and_then(|n| n.parse::<u64>().ok()) {
        Some(0) => {
            let _ = git(root, &["branch", "-D", branch]);
            Ok(None)
        }
        Some(own) => Ok(Some(coded_notice("worktree_branch_kept", json!({"branch": branch, "n": own}), format!("Kept branch {branch}: it has {own} commit(s) of its own.")))),
        // the branch may hold work: keep it when that can't be checked
        None => Ok(Some(coded_notice("worktree_branch_unchecked", json!({"branch": branch}), format!("Kept branch {branch}: whether it has commits of its own couldn't be checked.")))),
    }
}

fn no_worktree() -> AppError {
    AppError::coded("no_worktree", Value::Null, "No worktree.")
}

/// Apply the worktree's changes (committed and uncommitted, new files
/// included) to the original working tree with a 3-way apply.
pub fn apply_to_origin(info: &Value) -> AppResult<String> {
    let path = Path::new(info["path"].as_str().ok_or_else(no_worktree)?);
    let root = Path::new(info["repo_root"].as_str().ok_or_else(|| AppError::coded("not_git_repo", Value::Null, "No repository."))?);
    let base = info["base"].as_str().unwrap_or("HEAD");
    // Stage everything (intent-to-add for new files) so one diff covers it.
    git(path, &["add", "-A", "--intent-to-add"])?;
    let patch = git_bytes(path, &["diff", "--binary", base])?;
    if patch.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok("There are no changes to apply.".into());
    }
    let tmp = std::env::temp_dir().join(format!("clauderust-{}.patch", crate::util::new_id()));
    std::fs::write(&tmp, &patch)?;
    let patch = String::from_utf8_lossy(&patch).to_string();
    let r = git(root, &["apply", "--3way", "--whitespace=nowarn", &tmp.to_string_lossy()]);
    let _ = std::fs::remove_file(&tmp);
    r?;
    let files = patch.lines().filter(|l| l.starts_with("diff --git")).count();
    Ok(format!("Applied changes to {} file(s) in {}.", files, root.display()))
}

/// Whether a worktree has nothing worth keeping (no changes, no commits).
pub fn is_disposable(info: &Value) -> bool {
    let (Some(path), Some(root), Some(branch)) = (info["path"].as_str(), info["repo_root"].as_str(), info["branch"].as_str()) else { return false };
    if !Path::new(path).is_dir() {
        return true;
    }
    let clean = git(Path::new(path), &["status", "--porcelain"]).map(|s| s.is_empty()).unwrap_or(false);
    let base = info["base"].as_str().unwrap_or("HEAD");
    let own = git(Path::new(root), &["rev-list", "--count", &format!("{base}..{branch}")]).ok().and_then(|n| n.parse::<u64>().ok()).unwrap_or(1);
    clean && own == 0
}

/// Commit everything in the worktree on its branch.
pub fn commit_all(info: &Value, message: &str) -> AppResult<String> {
    let path = Path::new(info["path"].as_str().ok_or_else(no_worktree)?);
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

    #[test]
    fn folder_names_keep_their_letters() {
        assert_eq!(sanitize("proje-çalışma ğüşiöç"), "proje-çalışma-ğüşiöç");
        assert_eq!(sanitize("  my repo (v2)!! "), "my-repo-v2");
        assert_eq!(sanitize("a_b--c"), "a_b-c");
        assert_eq!(sanitize("..."), "");
        assert_eq!(sanitize(&"x".repeat(60)).len(), 40);
    }

    #[cfg(unix)]
    #[test]
    fn a_command_past_its_time_is_stopped_with_what_it_started() {
        let pidfile = std::env::temp_dir().join(format!("cr-wt-slow-{}", crate::util::new_id()));
        let mut c = Command::new("sh");
        // like a hook or an LFS filter that hangs, with a child of its own
        c.arg("-c").arg("sleep 60 & echo $! > \"$0\"; echo started; wait").arg(&pidfile);
        let started = Instant::now();
        assert!(output_within(c, Duration::from_millis(400)).unwrap().is_none(), "reported as stopped");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_string();
        let alive = || std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit(')').next().is_some_and(|r| !r.trim_start().starts_with('Z')));
        for _ in 0..30 {
            if !alive() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if cfg!(target_os = "linux") {
            assert!(!alive(), "its child {pid} was left running");
        }
        let _ = std::fs::remove_file(pidfile);
        // a quick one is unaffected, output and all
        let mut ok = Command::new("sh");
        ok.arg("-c").arg("echo out; echo err >&2; exit 3");
        let out = output_within(ok, Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!((out.stdout.as_slice(), out.stderr.as_slice(), out.status.code()), (&b"out\n"[..], &b"err\n"[..], Some(3)));
    }

    fn init_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cr-wt-{}", crate::util::new_id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        git(&dir, &["config", "core.autocrlf", "false"]).unwrap(); // keep LF on Windows runners
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
        // uncommitted work is saved on the branch, which is kept
        let note = remove(&info).unwrap().unwrap();
        assert_eq!((note["kind"].as_str(), &note["params"]["branch"]), (Some("worktree_saved"), &info["branch"]));
        assert!(note["message"].as_str().unwrap().contains("Saved"), "{note}");
        assert!(!wt.exists());
        let branch = info["branch"].as_str().unwrap();
        assert!(git(&repo, &["show", &format!("{branch}:new.txt")]).unwrap().contains("new"));
        // a clean worktree goes away with its branch
        let (_, info2) = create(&repo, &data, "eeee1111").unwrap();
        assert!(is_disposable(&info2));
        assert_eq!(remove(&info2).unwrap(), None);
        assert!(git(&repo, &["rev-parse", "--verify", info2["branch"].as_str().unwrap()]).is_err());
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn work_that_cannot_be_saved_keeps_the_worktree() {
        if which::which("git").is_err() {
            return;
        }
        let repo = init_repo();
        let data = std::env::temp_dir().join(format!("cr-wtdata-{}", crate::util::new_id()));
        let (_, info) = create(&repo, &data, "abab1212").unwrap();
        let wt = PathBuf::from(info["path"].as_str().unwrap());
        std::fs::write(wt.join("work.txt"), "unsaved work\n").unwrap();
        // a stale index.lock: `git add` and `git commit` both fail
        let gitdir = PathBuf::from(git(&wt, &["rev-parse", "--absolute-git-dir"]).unwrap());
        std::fs::write(gitdir.join("index.lock"), "").unwrap();
        let err = remove(&info).unwrap_err().to_string();
        assert!(err.contains("was kept") && err.contains(&wt.display().to_string()), "{err}");
        assert_eq!(std::fs::read_to_string(wt.join("work.txt")).unwrap(), "unsaved work\n");
        // once the lock is gone, the work is saved and the worktree removed
        std::fs::remove_file(gitdir.join("index.lock")).unwrap();
        assert!(remove(&info).unwrap().unwrap()["message"].as_str().unwrap().contains("Saved"));
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn unreadable_state_or_missing_repository_keeps_the_worktree() {
        if which::which("git").is_err() {
            return;
        }
        let repo = init_repo();
        let data = std::env::temp_dir().join(format!("cr-wtdata-{}", crate::util::new_id()));
        let (_, info) = create(&repo, &data, "cdcd3434").unwrap();
        let wt = PathBuf::from(info["path"].as_str().unwrap());
        std::fs::write(wt.join("work.txt"), "unsaved\n").unwrap();
        // the repository's drive is gone
        let mut gone = info.clone();
        gone["repo_root"] = json!(repo.join("not-here").to_string_lossy());
        assert!(remove(&gone).unwrap_err().to_string().contains("can't be found"));
        assert!(wt.join("work.txt").exists());
        // `git status` fails in the worktree: not taken for "clean"
        let dotgit = std::fs::read_to_string(wt.join(".git")).unwrap();
        std::fs::write(wt.join(".git"), "garbage\n").unwrap();
        assert!(remove(&info).unwrap_err().to_string().contains("was kept"));
        assert!(wt.join("work.txt").exists());
        std::fs::write(wt.join(".git"), dotgit).unwrap();
        assert!(remove(&info).unwrap().unwrap()["message"].as_str().unwrap().contains("Saved"));
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
        let note = remove(&info).unwrap().unwrap();
        assert_eq!((note["kind"].as_str(), note["params"]["n"].as_u64()), (Some("worktree_branch_kept"), Some(1)));
        assert!(note["message"].as_str().unwrap().contains("Kept branch"));
        assert!(git(&repo, &["rev-parse", "--verify", info["branch"].as_str().unwrap()]).is_ok());
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&data);
    }
}
