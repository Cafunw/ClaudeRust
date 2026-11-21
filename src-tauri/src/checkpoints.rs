//! Checkpoints for code sessions: a snapshot of the project's files before
//! each user turn, so both the conversation and the files can be rewound.
//!
//! Snapshots live in a "shadow" git repository in the app's data folder
//! (`checkpoints/<hash of the folder>`), whose work tree is the project
//! folder. The project's own `.git` is never touched; its `.gitignore` files
//! are honoured, plus a few heavy folders that are always skipped. Works
//! the same for folders that aren't git repositories.

use crate::error::{AppError, AppResult};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

const EXCLUDES: &str = ".git/\nnode_modules/\ntarget/\ndist/\nbuild/\n.next/\n.venv/\nvenv/\n__pycache__/\n*.pyc\n.DS_Store\n*.log\n.cache/\n";

pub struct Shadow {
    git_dir: PathBuf,
    work: PathBuf,
}

/// One lock per shadow repository: sessions in the same folder (and the
/// rewind menu during a snapshot) share it, and git commands running at the
/// same time collide on its index.lock.
static LOCKS: std::sync::LazyLock<parking_lot::Mutex<HashMap<PathBuf, Arc<parking_lot::Mutex<()>>>>> =
    std::sync::LazyLock::new(Default::default);

impl Shadow {
    pub fn new(data_dir: &Path, work: &Path) -> Shadow {
        let work = std::fs::canonicalize(work).unwrap_or_else(|_| work.to_path_buf());
        let h = hex::encode(&Sha256::digest(work.to_string_lossy().as_bytes())[..8]);
        Shadow { git_dir: data_dir.join("checkpoints").join(h), work }
    }

    /// This shadow repository's lock (one per folder, kept for the app's life).
    fn lock(&self) -> Arc<parking_lot::Mutex<()>> {
        LOCKS.lock().entry(self.git_dir.clone()).or_default().clone()
    }

    fn git(&self, args: &[&str]) -> AppResult<String> {
        self.git_env(args, None)
    }

    /// `git`, with `index` as the index file instead of the repository's own.
    fn git_env(&self, args: &[&str], index: Option<&Path>) -> AppResult<String> {
        let mut c = Command::new("git");
        c.arg(format!("--git-dir={}", self.git_dir.display()))
            .arg(format!("--work-tree={}", self.work.display()))
            .args(["-c", "core.autocrlf=false", "-c", "core.safecrlf=false", "-c", "commit.gpgsign=false", "-c", "user.name=ClaudeRust", "-c", "user.email=checkpoints@clauderust.local", "-c", "core.quotepath=false"])
            .args(args)
            .current_dir(&self.work)
            .env("PATH", crate::shellenv::path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        if let Some(i) = index {
            c.env("GIT_INDEX_FILE", i);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        let out = c.output().map_err(|e| AppError::msg(format!("Could not run git: {e}")))?;
        if !out.status.success() {
            return Err(AppError::msg(String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn ensure(&self) -> AppResult<()> {
        if self.git_dir.join("HEAD").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.git_dir)?;
        self.git(&["init", "-q"])?;
        std::fs::create_dir_all(self.git_dir.join("info"))?;
        std::fs::write(self.git_dir.join("info").join("exclude"), EXCLUDES)?;
        // nested repositories are stored as plain files, not submodules
        let _ = self.git(&["config", "core.hooksPath", &self.git_dir.join("no-hooks").to_string_lossy()]);
        Ok(())
    }

    /// Whether the folder is reasonable to snapshot: not the home folder or
    /// a filesystem root, and not huge (files the project's .gitignore and
    /// our excludes don't skip).
    pub fn suitable(&self) -> Result<(), String> {
        let w = &self.work;
        if w.parent().is_none() || dirs::home_dir().map(|h| std::fs::canonicalize(&h).unwrap_or(h) == *w).unwrap_or(false) {
            return Err("the folder is the home folder or a drive root".into());
        }
        let (mut files, mut bytes) = (0u64, 0u64);
        let walker = ignore::WalkBuilder::new(w)
            .hidden(false)
            .require_git(false)
            .filter_entry(|e| !matches!(e.file_name().to_string_lossy().as_ref(), ".git" | "node_modules" | "target" | "dist" | "build" | ".venv" | "venv" | "__pycache__" | ".next" | ".cache"))
            .build();
        for e in walker.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                files += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                if files > 40_000 || bytes > 1_500_000_000 {
                    return Err(format!("the folder is too large to snapshot ({files}+ files, {} MB+)", bytes / 1_048_576));
                }
            }
        }
        Ok(())
    }

    /// Record the current state of the files; returns the snapshot id.
    pub fn snapshot(&self, label: &str) -> AppResult<String> {
        let lock = self.lock();
        let _held = lock.lock();
        self.snapshot_locked(label)
    }

    fn snapshot_locked(&self, label: &str) -> AppResult<String> {
        self.ensure()?;
        self.git(&["add", "-A", "--ignore-errors", "."])
            .or_else(|_| self.git(&["add", "-A", "."]))?;
        self.git(&["commit", "-q", "--allow-empty", "--no-verify", "-m", label])?;
        self.git(&["rev-parse", "HEAD"])
    }

    /// Make the files match snapshot `id` (files added since are removed).
    /// Takes a snapshot of the current state first and returns its id, so
    /// the rewind itself can be undone.
    pub fn restore(&self, id: &str) -> AppResult<String> {
        let lock = self.lock();
        let _held = lock.lock();
        self.ensure()?;
        self.git(&["cat-file", "-e", &format!("{id}^{{commit}}")])
            .map_err(|_| AppError::msg("This checkpoint is no longer available."))?;
        let before = self.snapshot_locked("before rewind")?;
        self.git(&["read-tree", "-u", "--reset", id])?;
        // make the index match the restored tree for the next snapshot
        self.git(&["reset", "-q", "--soft", &before])?;
        Ok(before)
    }

    /// Files that differ between a snapshot and the current files. Uses a
    /// copy of the index, so the shadow repository's own is left alone.
    pub fn changed_since(&self, id: &str) -> AppResult<Vec<String>> {
        let lock = self.lock();
        let _held = lock.lock();
        self.ensure()?;
        let index = self.git_dir.join(format!("index-changed-{}", &crate::util::new_id()[..8]));
        let real = self.git_dir.join("index");
        if real.is_file() {
            std::fs::copy(&real, &index)?;
        }
        let out = self
            .git_env(&["add", "-A", "--ignore-errors", "."], Some(&index))
            .or_else(|_| self.git_env(&["add", "-A", "."], Some(&index)))
            .and_then(|_| self.git_env(&["diff", "--cached", "--name-only", id], Some(&index)));
        let _ = std::fs::remove_file(&index);
        Ok(out?.lines().filter(|l| !l.is_empty()).map(String::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_and_restore_roundtrip() {
        if which::which("git").is_err() {
            return;
        }
        let work = std::env::temp_dir().join(format!("cr-cp-{}", crate::util::new_id()));
        let data = std::env::temp_dir().join(format!("cr-cpdata-{}", crate::util::new_id()));
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::create_dir_all(work.join("node_modules/x")).unwrap();
        std::fs::write(work.join("src/a.rs"), "v1\n").unwrap();
        std::fs::write(work.join("keep.txt"), "keep\n").unwrap();
        std::fs::write(work.join("node_modules/x/big.js"), "dep\n").unwrap();
        let s = Shadow::new(&data, &work);
        let c1 = s.snapshot("turn 1").unwrap();
        // the agent edits, creates and deletes files
        std::fs::write(work.join("src/a.rs"), "v2\n").unwrap();
        std::fs::write(work.join("src/new.rs"), "new\n").unwrap();
        std::fs::remove_file(work.join("keep.txt")).unwrap();
        let changed = s.changed_since(&c1).unwrap();
        assert_eq!(changed.len(), 3, "{changed:?}");
        let undo = s.restore(&c1).unwrap();
        assert_eq!(std::fs::read_to_string(work.join("src/a.rs")).unwrap(), "v1\n");
        assert!(!work.join("src/new.rs").exists());
        assert_eq!(std::fs::read_to_string(work.join("keep.txt")).unwrap(), "keep\n");
        assert!(work.join("node_modules/x/big.js").exists(), "excluded folders are left alone");
        // undo the rewind
        s.restore(&undo).unwrap();
        assert_eq!(std::fs::read_to_string(work.join("src/a.rs")).unwrap(), "v2\n");
        assert!(work.join("src/new.rs").exists());
        assert!(s.restore("0123456789abcdef0123456789abcdef01234567").is_err());
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn sessions_in_one_folder_can_snapshot_at_the_same_time() {
        if which::which("git").is_err() {
            return;
        }
        let work = std::env::temp_dir().join(format!("cr-cpc-{}", crate::util::new_id()));
        let data = std::env::temp_dir().join(format!("cr-cpcdata-{}", crate::util::new_id()));
        std::fs::create_dir_all(&work).unwrap();
        for i in 0..50 {
            std::fs::write(work.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
        }
        let first = Shadow::new(&data, &work).snapshot("start").unwrap();
        // several sessions (each with its own Shadow) and the rewind menu
        let threads: Vec<_> = (0..6)
            .map(|t| {
                let (work, data, first) = (work.clone(), data.clone(), first.clone());
                std::thread::spawn(move || {
                    let s = Shadow::new(&data, &work);
                    let mut errors = vec![];
                    for i in 0..4 {
                        let r = if t % 2 == 0 { s.snapshot(&format!("t{t} {i}")).map(|_| ()) } else { s.changed_since(&first).map(|_| ()) };
                        if let Err(e) = r {
                            errors.push(e.to_string());
                        }
                    }
                    errors
                })
            })
            .collect();
        let errors: Vec<String> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
        assert!(errors.is_empty(), "{errors:?}");
        // changed_since leaves the shadow index alone
        std::fs::write(work.join("f0.txt"), "changed\n").unwrap();
        let s = Shadow::new(&data, &work);
        assert_eq!(s.changed_since(&first).unwrap(), ["f0.txt"]);
        assert!(s.git(&["diff", "--cached", "--name-only"]).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&data);
    }
}
