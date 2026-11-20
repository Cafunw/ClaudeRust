//! Interactive terminals (a real PTY running the user's shell) for the
//! Terminal pane of code sessions.
//!
//! Output is pushed to the UI as `term-output` events (UTF-8 text, split
//! only at character boundaries); `term-exit` fires when the shell ends.

use crate::error::{AppError, AppResult};
use crate::util::new_id;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

pub struct Term {
    pub id: String,
    pub conv_id: String,
    pub cwd: String,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

#[derive(Default)]
pub struct Terminals {
    map: Mutex<HashMap<String, Arc<Term>>>,
}

/// The user's interactive shell.
fn default_shell() -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        let ps = which::which("pwsh").map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "powershell.exe".into());
        (ps, vec!["-NoLogo".into()])
    }
    #[cfg(not(windows))]
    {
        let sh = std::env::var("SHELL").ok().filter(|s| Path::new(s).exists()).unwrap_or_else(|| {
            if Path::new("/bin/bash").exists() {
                "/bin/bash".into()
            } else {
                "/bin/sh".into()
            }
        });
        (sh, vec!["-l".into()])
    }
}

/// Longest prefix of `buf` that is complete UTF-8 (invalid bytes count as
/// complete so they are emitted, lossily, rather than held forever).
fn utf8_complete(buf: &[u8]) -> usize {
    match std::str::from_utf8(buf) {
        Ok(_) => buf.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => buf.len(),
    }
}

impl Terminals {
    pub fn open(&self, app: &AppHandle, conv_id: &str, cwd: &str, cols: u16, rows: u16) -> AppResult<Value> {
        let pty = native_pty_system();
        let size = PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 };
        let pair = pty.openpty(size).map_err(|e| AppError::msg(format!("Could not open a terminal: {e}")))?;
        let (shell, args) = default_shell();
        let mut cmd = CommandBuilder::new(&shell);
        cmd.args(&args);
        let dir = if Path::new(cwd).is_dir() { cwd.to_string() } else { dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default() };
        cmd.cwd(&dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("PATH", crate::shellenv::path());
        for k in crate::display::vars_we_set() {
            cmd.env_remove(k);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AppError::msg(format!("Could not start {shell}: {e}")))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| AppError::msg(e.to_string()))?;
        let writer = pair.master.take_writer().map_err(|e| AppError::msg(e.to_string()))?;
        let id = format!("term_{}", &new_id()[..8]);
        let term = Arc::new(Term {
            id: id.clone(),
            conv_id: conv_id.to_string(),
            cwd: dir.clone(),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
        });
        self.map.lock().insert(id.clone(), term.clone());

        let app2 = app.clone();
        let id2 = id.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut carry: Vec<u8> = vec![];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        carry.extend_from_slice(&buf[..n]);
                        let k = utf8_complete(&carry);
                        if k > 0 {
                            let text = String::from_utf8_lossy(&carry[..k]).to_string();
                            carry.drain(..k);
                            let _ = app2.emit("term-output", json!({"id": id2, "data": text}));
                        }
                    }
                }
            }
            let code = term.child.lock().wait().ok().map(|s| s.exit_code());
            let _ = app2.emit("term-exit", json!({"id": id2, "code": code}));
        });
        Ok(json!({"id": id, "shell": shell, "cwd": dir}))
    }

    fn get(&self, id: &str) -> AppResult<Arc<Term>> {
        self.map.lock().get(id).cloned().ok_or_else(|| AppError::msg("This terminal has closed."))
    }

    pub fn write(&self, id: &str, data: &str) -> AppResult<()> {
        let t = self.get(id)?;
        let mut w = t.writer.lock();
        w.write_all(data.as_bytes())?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let t = self.get(id)?;
        let r = t.master.lock().resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 });
        r.map_err(|e| AppError::msg(e.to_string()))
    }

    pub fn close(&self, id: &str) {
        if let Some(t) = self.map.lock().remove(id) {
            let mut child = t.child.lock();
            // hang up the whole job (background jobs started in the shell
            // too), like closing a terminal window
            #[cfg(unix)]
            if let Some(pid) = child.process_id() {
                crate::codetools::signal_process_group(pid, libc::SIGHUP);
            }
            let _ = child.kill();
        }
    }

    pub fn list(&self, conv_id: &str) -> Vec<Value> {
        self.map
            .lock()
            .values()
            .filter(|t| t.conv_id == conv_id)
            .map(|t| json!({"id": t.id, "cwd": t.cwd}))
            .collect()
    }

    pub fn close_conversation(&self, conv_id: &str) {
        let ids: Vec<String> = self.map.lock().values().filter(|t| t.conv_id == conv_id).map(|t| t.id.clone()).collect();
        for id in ids {
            self.close(&id);
        }
    }

    pub fn close_all(&self) {
        let ids: Vec<String> = self.map.lock().keys().cloned().collect();
        for id in ids {
            self.close(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_boundaries() {
        let s = "aé".as_bytes();
        assert_eq!(utf8_complete(s), 3);
        assert_eq!(utf8_complete(&s[..2]), 1);
        assert_eq!(utf8_complete(&[0xff, b'a']), 2);
    }

    #[cfg(unix)]
    #[test]
    fn pty_runs_a_shell() {
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "echo pty-ok"]);
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut r = pair.master.try_clone_reader().unwrap();
        let mut out = String::new();
        let mut buf = [0u8; 1024];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
            if out.contains("pty-ok") {
                break;
            }
        }
        let _ = child.wait();
        assert!(out.contains("pty-ok"), "{out}");
    }
}
