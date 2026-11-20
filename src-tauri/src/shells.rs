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
        });
        Ok(shell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
