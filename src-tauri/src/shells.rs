//! Background shell commands started by the agent (`Bash` with
//! `run_in_background`), and the registry the Tasks pane, `BashOutput` and
//! `KillShell` work with.
//!
//! Each shell runs in its own process group (process tree on Windows) so
//! stopping it also stops everything it started. Output from stdout and
//! stderr is merged into one buffer that keeps the most recent
//! `OUTPUT_CAP` bytes.

use crate::codetools::CodeSession;
use crate::util::{new_id, now_ms};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

const OUTPUT_CAP: usize = 1_000_000;
/// Background commands that ran at least this long notify when they end.
const NOTIFY_AFTER_MS: i64 = 20_000;

pub struct BgShell {
    pub id: String,
    pub conv_id: String,
    pub command: String,
    pub description: String,
    pub cwd: PathBuf,
    pub started_at: i64,
    out: Mutex<OutBuf>,
    pub exit: Mutex<Option<i32>>,
    pub finished_at: Mutex<Option<i64>>,
    pub killed: AtomicBool,
    cancel: CancellationToken,
}

#[derive(Default)]
struct OutBuf {
    text: String,
    /// Bytes dropped from the front to stay under the cap.
    dropped: usize,
    /// Absolute position (dropped + index) `BashOutput` has read up to.
    read_pos: usize,
}

impl BgShell {
    pub fn running(&self) -> bool {
        self.exit.lock().is_none() && !self.killed.load(Ordering::Relaxed)
    }

    fn push(&self, s: &str) {
        let mut o = self.out.lock();
        o.text.push_str(s);
        if o.text.len() > OUTPUT_CAP {
            let mut cut = o.text.len() - OUTPUT_CAP;
            while !o.text.is_char_boundary(cut) {
                cut += 1;
            }
            o.text.drain(..cut);
            o.dropped += cut;
        }
    }

    /// Output not returned by an earlier call, then marks it read.
    pub fn take_new(&self) -> String {
        let mut o = self.out.lock();
        let end = o.dropped + o.text.len();
        let from = o.read_pos.max(o.dropped) - o.dropped;
        let mut from = from.min(o.text.len());
        while !o.text.is_char_boundary(from) {
            from += 1;
        }
        let s = o.text[from..].to_string();
        o.read_pos = end;
        s
    }

    /// The last `max` bytes of output (for the UI).
    pub fn tail(&self, max: usize) -> String {
        let o = self.out.lock();
        if o.text.len() <= max {
            return o.text.clone();
        }
        let mut from = o.text.len() - max;
        while !o.text.is_char_boundary(from) {
            from += 1;
        }
        o.text[from..].to_string()
    }

    pub fn status(&self) -> &'static str {
        if self.killed.load(Ordering::Relaxed) {
            "killed"
        } else {
            match *self.exit.lock() {
                None => "running",
                Some(0) => "completed",
                Some(_) => "failed",
            }
        }
    }

    pub fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "conversation_id": self.conv_id,
            "command": self.command,
            "description": self.description,
            "cwd": self.cwd,
            "started_at": self.started_at,
            "finished_at": *self.finished_at.lock(),
            "exit_code": *self.exit.lock(),
            "status": self.status(),
        })
    }

    pub fn kill(&self) {
        self.killed.store(true, Ordering::Relaxed);
        self.cancel.cancel();
    }
}

/// The notification text for a background command that ended, or None when
/// it shouldn't notify (stopped on purpose, or too quick to be worth it).
fn finished_note(sh: &BgShell, code: i32, took_ms: i64, turkish: bool) -> Option<String> {
    if sh.killed.load(Ordering::Relaxed) || took_ms < NOTIFY_AFTER_MS {
        return None;
    }
    let what = if sh.description.trim().is_empty() { sh.command.as_str() } else { sh.description.as_str() };
    let what: String = what.chars().take(80).collect();
    let secs = took_ms / 1000;
    if turkish {
        let dur = if secs >= 60 { format!("{} dk {} sn", secs / 60, secs % 60) } else { format!("{secs} sn") };
        return Some(if code == 0 { format!("{what} {dur} sonra bitti.") } else { format!("{what} {dur} sonra başarısız oldu (çıkış kodu {code}).") });
    }
    let dur = if secs >= 60 { format!("{}m {}s", secs / 60, secs % 60) } else { format!("{secs}s") };
    Some(if code == 0 { format!("{what} finished after {dur}.") } else { format!("{what} failed (exit code {code}) after {dur}.") })
}

#[derive(Default)]
pub struct Shells {
    map: Mutex<HashMap<String, Arc<BgShell>>>,
    app: OnceLock<AppHandle>,
}

impl Shells {
    pub fn set_app(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    fn changed(&self, conv_id: &str) {
        if let Some(app) = self.app.get() {
            let _ = app.emit("shells-changed", json!({"conversation_id": conv_id}));
        }
    }

    /// A desktop notification when a long background command ends while the
    /// window isn't in front (not when it was stopped on purpose).
    fn notify_finished(&self, sh: &BgShell, code: i32) {
        let Some(app) = self.app.get() else { return };
        use tauri::Manager;
        let Some(st) = app.try_state::<Arc<crate::state::AppState>>() else { return };
        if !st.settings.read().notifications {
            return;
        }
        let turkish = st.turkish();
        let Some(body) = finished_note(sh, code, now_ms() - sh.started_at, turkish) else { return };
        let focused = app.get_webview_window("main").map(|w| w.is_focused().unwrap_or(false) && w.is_visible().unwrap_or(false)).unwrap_or(false);
        if focused {
            return;
        }
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title(if turkish { "Arka plan komutu" } else { "Background command" }).body(body).show();
    }

    pub fn get(&self, id: &str) -> Option<Arc<BgShell>> {
        self.map.lock().get(id).cloned()
    }

    pub fn list(&self, conv_id: Option<&str>) -> Vec<Arc<BgShell>> {
        let mut v: Vec<Arc<BgShell>> = self
            .map
            .lock()
            .values()
            .filter(|s| conv_id.map(|c| s.conv_id == c).unwrap_or(true))
            .cloned()
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        v
    }

    pub fn kill(&self, id: &str) -> bool {
        match self.get(id) {
            Some(s) if s.running() => {
                s.kill();
                self.changed(&s.conv_id);
                true
            }
            _ => false,
        }
    }

    /// Stop every shell of a conversation (on delete) and forget them.
    pub fn kill_conversation(&self, conv_id: &str) {
        let ids: Vec<String> = self.list(Some(conv_id)).iter().map(|s| s.id.clone()).collect();
        for id in &ids {
            if let Some(s) = self.get(id) {
                s.kill();
            }
        }
        self.map.lock().retain(|_, s| s.conv_id != conv_id);
    }

    pub fn kill_all(&self) {
        for s in self.map.lock().values() {
            s.kill();
        }
    }

    /// Whether any background command is still running (not yet reaped).
    pub fn any_running(&self) -> bool {
        self.map.lock().values().any(|s| s.exit.lock().is_none())
    }

    /// Remove finished shells of a conversation from the list.
    pub fn clear_finished(&self, conv_id: &str) {
        self.map.lock().retain(|_, s| s.conv_id != conv_id || s.running());
        self.changed(conv_id);
    }

    pub fn spawn(self: &Arc<Self>, session: &CodeSession, command: &str, description: &str) -> crate::error::AppResult<Arc<BgShell>> {
        let cwd = {
            let c = session.cwd.lock().clone();
            if c.is_dir() {
                c
            } else {
                session.root.clone()
            }
        };
        let (mut cmd, pwd_file) = crate::codetools::shell_command(&cwd, command);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| crate::error::AppError::msg(format!("Failed to start shell: {e}")))?;
        let shell = Arc::new(BgShell {
            id: format!("bash_{}", &new_id()[..8]),
            conv_id: session.conv_id.clone(),
            command: command.to_string(),
            description: description.to_string(),
            cwd,
            started_at: now_ms(),
            out: Mutex::new(OutBuf::default()),
            exit: Mutex::new(None),
            finished_at: Mutex::new(None),
            killed: AtomicBool::new(false),
            cancel: CancellationToken::new(),
        });
        self.map.lock().insert(shell.id.clone(), shell.clone());
        self.changed(&shell.conv_id);

        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let s2 = shell.clone();
        let me = self.clone();
        tokio::spawn(async move {
            async fn pump<R: tokio::io::AsyncRead + Unpin>(r: Option<R>, s: Arc<BgShell>) {
                let Some(mut r) = r else { return };
                let mut buf = vec![0u8; 16 * 1024];
                let mut carry: Vec<u8> = vec![];
                loop {
                    match r.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            carry.extend_from_slice(&buf[..n]);
                            let valid = match std::str::from_utf8(&carry) {
                                Ok(_) => carry.len(),
                                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                                Err(_) => carry.len(),
                            };
                            let text = String::from_utf8_lossy(&carry[..valid]).to_string();
                            carry.drain(..valid);
                            s.push(&text);
                        }
                    }
                }
                if !carry.is_empty() {
                    s.push(&String::from_utf8_lossy(&carry));
                }
            }
            let io = async {
                tokio::join!(pump(stdout, s2.clone()), pump(stderr, s2.clone()));
                child.wait().await
            };
            let code = tokio::select! {
                st = io => st.ok().and_then(|s| s.code()).unwrap_or(-1),
                _ = s2.cancel.cancelled() => {
                    if let Some(p) = pid {
                        #[cfg(unix)]
                        crate::codetools::kill_process_group(p);
                        #[cfg(windows)]
                        crate::codetools::kill_process_tree(p);
                    }
                    -1
                }
            };
            if let Some(pf) = pwd_file {
                let _ = std::fs::remove_file(pf);
            }
            *s2.exit.lock() = Some(code);
            *s2.finished_at.lock() = Some(now_ms());
            me.changed(&s2.conv_id);
            me.notify_finished(&s2, code);
        });
        Ok(shell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_notes() {
        let mut sh = shell();
        sh.description = "Build".into();
        assert_eq!(finished_note(&sh, 0, 5_000, false), None, "quick commands don't notify");
        assert_eq!(finished_note(&sh, 0, 95_000, false).as_deref(), Some("Build finished after 1m 35s."));
        assert_eq!(finished_note(&sh, 2, 30_000, false).as_deref(), Some("Build failed (exit code 2) after 30s."));
        assert_eq!(finished_note(&sh, 0, 95_000, true).as_deref(), Some("Build 1 dk 35 sn sonra bitti."));
        assert_eq!(finished_note(&sh, 2, 30_000, true).as_deref(), Some("Build 30 sn sonra başarısız oldu (çıkış kodu 2)."));
        sh.killed.store(true, Ordering::Relaxed);
        assert_eq!(finished_note(&sh, -1, 95_000, false), None, "stopped on purpose");
    }

    fn shell() -> BgShell {
        BgShell {
            id: "b".into(),
            conv_id: "c".into(),
            command: "x".into(),
            description: String::new(),
            cwd: PathBuf::from("/"),
            started_at: 0,
            out: Mutex::new(OutBuf::default()),
            exit: Mutex::new(None),
            finished_at: Mutex::new(None),
            killed: AtomicBool::new(false),
            cancel: CancellationToken::new(),
        }
    }

    #[test]
    fn take_new_returns_only_unread_output() {
        let s = shell();
        s.push("hello ");
        assert_eq!(s.take_new(), "hello ");
        assert_eq!(s.take_new(), "");
        s.push("world");
        assert_eq!(s.take_new(), "world");
        assert_eq!(s.status(), "running");
        s.kill();
        assert_eq!(s.status(), "killed");
    }

    #[test]
    fn buffer_is_capped_at_char_boundaries() {
        let s = shell();
        let big = "é".repeat(OUTPUT_CAP / 2 + 10);
        s.push(&big);
        s.push("end");
        let t = s.tail(usize::MAX);
        assert!(t.len() <= OUTPUT_CAP + 1);
        assert!(t.ends_with("end"));
        assert!(s.take_new().ends_with("end"));
    }

    #[test]
    fn output_dropped_by_the_cap_is_not_returned_later() {
        let s = shell();
        s.push("old");
        assert_eq!(s.take_new(), "old");
        // pushes the read "old" and unread bytes out of the front
        s.push(&"x".repeat(OUTPUT_CAP));
        s.push("new");
        let t = s.take_new();
        assert_eq!(t.len(), OUTPUT_CAP);
        assert!(t.ends_with("new") && !t.contains("old"));
        assert_eq!(s.take_new(), "");
        assert_eq!(s.tail(3), "new");
    }

    #[test]
    fn status_follows_the_exit_code_and_notes_fall_back_to_the_command() {
        let mut sh = shell();
        *sh.exit.lock() = Some(0);
        assert_eq!((sh.status(), sh.running()), ("completed", false));
        *sh.exit.lock() = Some(2);
        assert_eq!(sh.summary()["status"], "failed");
        assert_eq!(sh.summary()["exit_code"], 2);
        sh.command = "y".repeat(100);
        let note = finished_note(&sh, 0, 30_000, false).unwrap();
        assert_eq!(note, format!("{} finished after 30s.", "y".repeat(80)));
    }

    /// A shell for conversation `conv` in a new folder whose name has a space.
    fn session(conv: &str, shells: &Arc<Shells>) -> CodeSession {
        let root = std::env::temp_dir().join(format!("cr shells {}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        CodeSession::for_conversation(root, conv, shells.clone())
    }

    async fn exit_of(sh: &BgShell) -> Option<i32> {
        let start = std::time::Instant::now();
        while sh.exit.lock().is_none() && start.elapsed() < std::time::Duration::from_secs(60) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        *sh.exit.lock()
    }

    const LONG: &str = if cfg!(windows) { "Start-Sleep 60" } else { "sleep 60" };

    #[tokio::test]
    async fn a_background_command_runs_in_the_session_folder() {
        let shells = Arc::new(Shells::default());
        let s = session("c", &shells);
        std::fs::write(s.root.join("marker.txt"), "").unwrap();
        // a working directory that's gone: the command runs in the folder
        *s.cwd.lock() = s.root.join("deleted");
        let sh = shells.spawn(&s, "echo \"hello world\"; ls; exit 3", "List").unwrap();
        assert_eq!(sh.cwd, s.root);
        assert_eq!(sh.status(), "running");
        assert!(shells.any_running());
        assert_eq!(exit_of(&sh).await, Some(3));
        assert!(sh.finished_at.lock().is_some());
        assert_eq!(sh.status(), "failed");
        let out = sh.take_new();
        assert!(out.contains("hello world") && out.contains("marker.txt"), "{out}");
        assert_eq!(sh.take_new(), "");
        assert!(!shells.any_running());
        assert!(!shells.kill(&sh.id), "a finished command can't be killed");
        assert_eq!(shells.list(Some("c")).len(), 1);
        assert!(shells.list(Some("other")).is_empty());
        shells.clear_finished("c");
        assert!(shells.get(&sh.id).is_none());
        let _ = std::fs::remove_dir_all(&s.root);
    }

    #[tokio::test]
    async fn killing_a_background_command_stops_it() {
        let shells = Arc::new(Shells::default());
        let s = session("c", &shells);
        let sh = shells.spawn(&s, LONG, "").unwrap();
        let started = std::time::Instant::now();
        assert!(shells.kill(&sh.id));
        assert_eq!(sh.status(), "killed");
        assert_eq!(exit_of(&sh).await, Some(-1));
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "it ran on after the kill");
        assert!(!shells.any_running());
        assert!(!shells.kill(&sh.id));
        assert!(!shells.kill("bash_missing"));
        // a killed command is finished: cleared from the list
        shells.clear_finished("c");
        assert!(shells.list(None).is_empty());
        let _ = std::fs::remove_dir_all(&s.root);
    }

    #[tokio::test]
    async fn deleting_a_conversation_stops_only_its_shells() {
        let shells = Arc::new(Shells::default());
        let (a, b) = (session("a", &shells), session("b", &shells));
        let (sa, sb) = (shells.spawn(&a, LONG, "").unwrap(), shells.spawn(&b, LONG, "").unwrap());
        shells.kill_conversation("a");
        assert!(shells.list(Some("a")).is_empty());
        assert_eq!(sa.status(), "killed");
        assert!(sb.running());
        // running shells stay when finished ones are cleared
        shells.clear_finished("b");
        assert_eq!(shells.list(Some("b")).len(), 1);
        shells.kill_all();
        assert_eq!(sb.status(), "killed");
        assert_eq!((exit_of(&sa).await, exit_of(&sb).await), (Some(-1), Some(-1)));
        assert!(!shells.any_running());
        for s in [a, b] {
            let _ = std::fs::remove_dir_all(&s.root);
        }
    }
}
