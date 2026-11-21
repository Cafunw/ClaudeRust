//! Watch mode for code sessions: when files in the project change, run a
//! check command (build / type check / lint) in the background and report
//! the result, so errors can go straight back to Claude.
//!
//! The command is detected from the project (Cargo.toml → cargo check,
//! tsconfig.json → tsc, go.mod → go build/vet, …) or set by the user.
//! Runs are debounced, never overlap (a new change restarts the run), and
//! wait while the agent is in the middle of a turn.

use crate::error::{AppError, AppResult};
use crate::util::now_ms;
use notify::{RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

const IGNORED: &[&str] = &[".git", "target", "node_modules", "dist", "build", ".next", "__pycache__", ".venv", "venv", ".cache", ".turbo", "coverage"];

/// A sensible check command for the project in `root`, if one is obvious.
pub fn detect(root: &Path) -> Option<String> {
    let has = |f: &str| root.join(f).exists();
    if has("Cargo.toml") {
        return Some("cargo check --all-targets --message-format short".into());
    }
    if has("tsconfig.json") {
        return Some("npx --no-install tsc --noEmit --pretty false".into());
    }
    if has("go.mod") {
        return Some("go build ./... && go vet ./...".into());
    }
    if has("package.json") {
        let pj: Value = std::fs::read_to_string(root.join("package.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        for script in ["typecheck", "check", "lint", "build"] {
            if pj["scripts"][script].is_string() {
                return Some(format!("npm run -s {script}"));
            }
        }
    }
    if has("pyproject.toml") || has("setup.py") || has("requirements.txt") {
        return Some("python3 -m compileall -q -x '(\\.venv|venv|node_modules)' .".into());
    }
    if has("Makefile") {
        return Some("make -q || make".into());
    }
    None
}

/// Lines that look like errors in a check's output.
pub fn error_lines(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| {
            let t = l.trim_start();
            let lower = t.to_ascii_lowercase();
            lower.starts_with("error")
                || lower.contains(": error")
                || lower.contains(" error ts")
                || lower.contains("error[e")
                || lower.starts_with("syntaxerror")
                || (lower.contains(".go:") && lower.matches(':').count() >= 3 && !lower.contains("warning"))
        })
        .map(|l| l.trim().chars().take(300).collect())
        .collect()
}

#[derive(Default)]
struct WatchState {
    command: String,
    status: String, // idle | running | ok | failed | error
    output: String,
    errors: Vec<String>,
    finished_at: Option<i64>,
    duration_ms: u64,
    auto_fix: bool,
    runs: u64,
}

pub struct Watch {
    root: PathBuf,
    state: Mutex<WatchState>,
    stop: CancellationToken,
    kick: tokio::sync::Notify,
    _watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl Watch {
    fn new(root: &Path, command: String) -> Arc<Watch> {
        Arc::new(Watch {
            root: root.to_path_buf(),
            state: Mutex::new(WatchState { command, status: "idle".into(), ..Default::default() }),
            stop: CancellationToken::new(),
            kick: tokio::sync::Notify::new(),
            _watcher: Mutex::new(None),
        })
    }

    pub fn summary(&self, conv_id: &str) -> Value {
        let s = self.state.lock();
        json!({
            "conversation_id": conv_id, "root": self.root, "command": s.command, "status": s.status,
            "errors": s.errors.len(), "error_lines": s.errors.iter().take(30).collect::<Vec<_>>(),
            "output": tail(&s.output, 20_000), "finished_at": s.finished_at, "duration_ms": s.duration_ms,
            "auto_fix": s.auto_fix, "runs": s.runs, "watching": true,
        })
    }
    pub fn output(&self) -> String {
        self.state.lock().output.clone()
    }
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut i = s.len() - n;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    s[i..].to_string()
}

#[derive(Default)]
pub struct Watches {
    map: Mutex<HashMap<String, Arc<Watch>>>,
}

fn relevant(p: &Path, root: &Path) -> bool {
    let Ok(rel) = p.strip_prefix(root) else { return true };
    !rel.components().any(|c| IGNORED.contains(&c.as_os_str().to_string_lossy().as_ref()))
}

impl Watches {
    pub fn get(&self, conv_id: &str) -> Option<Arc<Watch>> {
        self.map.lock().get(conv_id).cloned()
    }

    pub fn stop(&self, conv_id: &str) {
        if let Some(w) = self.map.lock().remove(conv_id) {
            w.stop.cancel();
        }
    }

    /// Make `w` the watch of `conv_id` and stop the one it replaces, which
    /// can be there although `start` stopped it: an overlapping `start`
    /// may have put its own in while this one was building its watcher.
    fn install(&self, conv_id: &str, w: Arc<Watch>) {
        let replaced = self.map.lock().insert(conv_id.to_string(), w);
        if let Some(old) = replaced {
            old.stop.cancel();
        }
    }

    pub fn set_auto_fix(&self, conv_id: &str, on: bool) {
        if let Some(w) = self.get(conv_id) {
            w.state.lock().auto_fix = on;
        }
    }

    pub fn run_now(&self, conv_id: &str) {
        if let Some(w) = self.get(conv_id) {
            w.kick.notify_one();
        }
    }

    /// Start watching `root` for a conversation (replaces an earlier watch).
    /// `busy` tells whether the agent is mid-turn (checks wait for it);
    /// `on_result` gets each finished run's summary.
    pub fn start(
        &self,
        app: &AppHandle,
        conv_id: &str,
        root: &Path,
        command: String,
        busy: Arc<dyn Fn() -> bool + Send + Sync>,
        on_result: Arc<dyn Fn(Value) + Send + Sync>,
    ) -> AppResult<Value> {
        // the old watch stops checking now, not once the new one is ready
        self.stop(conv_id);
        let w = Watch::new(root, command);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let root2 = root.to_path_buf();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                if ev.paths.iter().any(|p| relevant(p, &root2)) {
                    let _ = tx.send(());
                }
            }
        })
        .map_err(|e| AppError::msg(format!("Can't watch files: {e}")))?;
        // On Linux a recursive watch costs one inotify watch per folder,
        // including node_modules/target; watch only the project's own folders.
        #[cfg(target_os = "linux")]
        {
            let mut n = 0;
            let walker = ignore::WalkBuilder::new(root)
                .hidden(false)
                .require_git(false)
                .filter_entry(|e| !IGNORED.contains(&e.file_name().to_string_lossy().as_ref()))
                .build();
            for e in walker.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) && watcher.watch(e.path(), RecursiveMode::NonRecursive).is_ok() {
                    n += 1;
                    if n >= 4000 {
                        break;
                    }
                }
            }
            if n == 0 {
                return Err(AppError::msg(format!("Can't watch {}", root.display())));
            }
        }
        #[cfg(not(target_os = "linux"))]
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| AppError::msg(format!("Can't watch {}: {e}", root.display())))?;
        *w._watcher.lock() = Some(watcher);
        self.install(conv_id, w.clone());
        let summary = w.summary(conv_id);

        let app = app.clone();
        let conv = conv_id.to_string();
        tokio::spawn(async move {
            let emit = |w: &Watch| {
                let _ = app.emit("watch-status", w.summary(&conv));
            };
            // replaced by an overlapping start before it began
            if w.stop.is_cancelled() {
                return;
            }
            emit(&w);
            let mut first = true;
            loop {
                if !first {
                    // wait for a change (or a manual run), then let changes settle
                    tokio::select! {
                        _ = w.stop.cancelled() => break,
                        Some(_) = rx.recv() => {},
                        _ = w.kick.notified() => {},
                    }
                    loop {
                        tokio::select! {
                            _ = w.stop.cancelled() => return,
                            r = tokio::time::timeout(Duration::from_millis(900), rx.recv()) => {
                                if r.is_err() { break; }
                            }
                        }
                    }
                }
                first = false;
                // don't check half-finished work
                while busy() && !w.stop.is_cancelled() {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    while rx.try_recv().is_ok() {}
                }
                if w.stop.is_cancelled() {
                    break;
                }
                let cmd_text = w.state.lock().command.clone();
                w.state.lock().status = "running".into();
                emit(&w);
                let started = std::time::Instant::now();
                let (code, out) = run_check(&w.root, &cmd_text, &w.stop).await;
                {
                    let mut s = w.state.lock();
                    s.output = out.clone();
                    s.errors = error_lines(&out);
                    s.status = match code {
                        Some(0) => "ok".into(),
                        Some(_) => "failed".into(),
                        None => "error".into(),
                    };
                    s.finished_at = Some(now_ms());
                    s.duration_ms = started.elapsed().as_millis() as u64;
                    s.runs += 1;
                }
                // events caused by the check itself (build outputs) don't count
                while rx.try_recv().is_ok() {}
                emit(&w);
                on_result(w.summary(&conv));
            }
        });
        Ok(summary)
    }

    pub fn stop_all(&self) {
        let ids: Vec<String> = self.map.lock().keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }
}

async fn run_check(root: &Path, command: &str, stop: &CancellationToken) -> (Option<i32>, String) {
    let (mut cmd, pwd) = crate::codetools::shell_command(root, command);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    cmd.env("CARGO_TERM_COLOR", "never").env("NO_COLOR", "1").env("FORCE_COLOR", "0");
    let Ok(mut child) = cmd.spawn() else { return (None, format!("Could not run: {command}")) };
    let pid = child.id();
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    // read everything (so the check never blocks on a full pipe) but keep
    // at most 400 KB of each stream
    async fn drain<R: tokio::io::AsyncRead + Unpin>(r: Option<&mut R>, keep: &mut Vec<u8>) {
        let Some(r) = r else { return };
        let mut buf = [0u8; 16 * 1024];
        loop {
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = 400_000usize.saturating_sub(keep.len());
                    keep.extend_from_slice(&buf[..n.min(room)]);
                }
            }
        }
    }
    let io = async {
        let mut a = Vec::new();
        let mut b = Vec::new();
        tokio::join!(drain(so.as_mut(), &mut a), drain(se.as_mut(), &mut b));
        let st = child.wait().await.ok().and_then(|s| s.code());
        let mut out = String::from_utf8_lossy(&a).to_string();
        let err = String::from_utf8_lossy(&b);
        if !err.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&err);
        }
        (st, out)
    };
    let kill = || {
        if let Some(p) = pid {
            #[cfg(unix)]
            crate::codetools::kill_process_group(p);
            #[cfg(windows)]
            crate::codetools::kill_process_tree(p);
        }
    };
    let r = tokio::select! {
        r = tokio::time::timeout(Duration::from_secs(600), io) => match r {
            Ok(v) => v,
            Err(_) => { kill(); (None, "The check took longer than 10 minutes and was stopped.".into()) }
        },
        _ = stop.cancelled() => { kill(); (None, "stopped".into()) }
    };
    if let Some(p) = pwd {
        let _ = std::fs::remove_file(p);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_commands_and_errors() {
        let dir = std::env::temp_dir().join(format!("cr-watch-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(detect(&dir), None);
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"lint":"eslint ."}}"#).unwrap();
        assert_eq!(detect(&dir).as_deref(), Some("npm run -s lint"));
        std::fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        assert!(detect(&dir).unwrap().starts_with("cargo check"));
        let out = "src/main.rs:3:5: error[E0425]: cannot find value `x`\nwarning: unused\nsrc/a.ts(1,7): error TS2322: Type 'number'\n./x.go:4:2: undefined: y\nerror: could not compile `app`";
        let e = error_lines(out);
        assert_eq!(e.len(), 4, "{e:?}");
        assert!(relevant(&dir.join("src/a.rs"), &dir));
        assert!(!relevant(&dir.join("target/debug/x"), &dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_watch_replaced_by_an_overlapping_start_is_stopped() {
        let dir = std::env::temp_dir();
        let ws = Watches::default();
        let (a, b) = (Watch::new(&dir, "true".into()), Watch::new(&dir, "true".into()));
        // two starts at once: both stop what is there (nothing yet), build
        // their watcher, then register
        ws.stop("c");
        ws.stop("c");
        ws.install("c", a.clone());
        ws.install("c", b.clone());
        assert!(a.stop.is_cancelled(), "the first watch would run on, unreachable");
        assert!(!b.stop.is_cancelled());
        assert!(Arc::ptr_eq(&ws.get("c").unwrap(), &b));
        ws.stop_all();
        assert!(b.stop.is_cancelled());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn check_runs_and_reports() {
        let dir = std::env::temp_dir();
        let stop = CancellationToken::new();
        let (code, out) = run_check(&dir, "echo 'error: broken' >&2; exit 1", &stop).await;
        assert_eq!(code, Some(1));
        assert_eq!(error_lines(&out), vec!["error: broken".to_string()]);
        let (code, _) = run_check(&dir, "true", &stop).await;
        assert_eq!(code, Some(0));
        // lots of output must not block the check
        let (code, out) = tokio::time::timeout(Duration::from_secs(20), run_check(&dir, "head -c 3000000 /dev/zero | tr '\\0' x; echo; exit 3", &stop)).await.unwrap();
        assert_eq!(code, Some(3));
        assert!(out.len() <= 400_001);
    }
}
