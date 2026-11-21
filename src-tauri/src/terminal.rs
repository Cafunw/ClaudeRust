//! Interactive terminals (a real PTY running the user's shell) for the
//! Terminal pane of code sessions.
//!
//! Output is pushed to the UI as `term-output` events (UTF-8 text, split
//! only at character boundaries); `term-exit` fires when the shell ends.

use crate::error::{AppError, AppResult};
use crate::util::new_id;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub struct Term {
    pub id: String,
    pub conv_id: String,
    pub cwd: String,
    /// `None` once the shell has ended or the terminal was closed (dropping
    /// it closes the pseudo console, which ends the reader on Windows).
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// Keystrokes for the writer thread. Writing to a PTY blocks while the
    /// program in it isn't reading, so it never happens on a command thread.
    input: SyncSender<Vec<u8>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
}

#[derive(Default)]
pub struct Terminals {
    map: Arc<Mutex<HashMap<String, Arc<Term>>>>,
}

/// Queued writes per terminal before typing is refused as "not reading".
const INPUT_QUEUE: usize = 256;
/// Output is sent to the UI at most this often, in pieces of at most this
/// size, so a flood (`yes`, `cat` of a big file) doesn't swamp the window.
const OUTPUT_EVERY: Duration = Duration::from_millis(16);
const OUTPUT_MAX: usize = 64 * 1024;

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

/// Collect output chunks from `rx` into batches (one per `OUTPUT_EVERY`,
/// at most `OUTPUT_MAX` bytes, cut at character boundaries) and hand each
/// to `emit`, until the sender is gone.
fn pump_output(rx: Receiver<Vec<u8>>, mut emit: impl FnMut(String)) {
    let mut carry: Vec<u8> = Vec::new();
    let mut open = true;
    while open {
        match rx.recv() {
            Ok(b) => carry.extend_from_slice(&b),
            Err(_) => break,
        }
        let deadline = Instant::now() + OUTPUT_EVERY;
        while carry.len() < OUTPUT_MAX {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(b) => carry.extend_from_slice(&b),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        let k = utf8_complete(&carry);
        if k > 0 {
            emit(String::from_utf8_lossy(&carry[..k]).into_owned());
            carry.drain(..k);
        }
    }
    if !carry.is_empty() {
        emit(String::from_utf8_lossy(&carry).into_owned());
    }
}

impl Terminals {
    pub fn open<R: tauri::Runtime>(&self, app: &AppHandle<R>, conv_id: &str, cwd: &str, cols: u16, rows: u16) -> AppResult<Value> {
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
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AppError::msg(format!("Could not start {shell}: {e}")))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| AppError::msg(e.to_string()))?;
        let mut writer = pair.master.take_writer().map_err(|e| AppError::msg(e.to_string()))?;
        let id = format!("term_{}", &new_id()[..8]);
        let (input, input_rx) = sync_channel::<Vec<u8>>(INPUT_QUEUE);
        let term = Arc::new(Term {
            id: id.clone(),
            conv_id: conv_id.to_string(),
            cwd: dir.clone(),
            master: Mutex::new(Some(pair.master)),
            input,
            killer: Mutex::new(child.clone_killer()),
            pid: child.process_id(),
        });
        self.map.lock().insert(id.clone(), term.clone());

        // keystrokes, in order; ends when the terminal is dropped or the PTY closes
        std::thread::spawn(move || {
            for data in input_rx {
                if writer.write_all(&data).and_then(|_| writer.flush()).is_err() {
                    break;
                }
            }
        });
        // output: the reader blocks on the PTY, the pump batches for the UI
        let (out_tx, out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        // the shell's end: forget the terminal (its pid may be reused by an
        // unrelated process, which must never be signalled on close), drop
        // the master (on Windows the reader only sees EOF once the pseudo
        // console is closed), then report the exit code
        let (exit_tx, exit_rx) = std::sync::mpsc::channel::<Option<u32>>();
        let waited = term.clone();
        let map = self.map.clone();
        std::thread::spawn(move || {
            let code = child.wait().ok().map(|s| s.exit_code());
            map.lock().remove(&waited.id);
            waited.master.lock().take();
            let _ = exit_tx.send(code);
        });
        let app2 = app.clone();
        let id2 = id.clone();
        std::thread::spawn(move || {
            pump_output(out_rx, |text| {
                let _ = app2.emit("term-output", json!({"id": id2, "data": text}));
            });
            let code = exit_rx.recv_timeout(Duration::from_secs(5)).ok().flatten();
            let _ = app2.emit("term-exit", json!({"id": id2, "code": code}));
        });
        Ok(json!({"id": id, "shell": shell, "cwd": dir}))
    }

    fn get(&self, id: &str) -> AppResult<Arc<Term>> {
        self.map.lock().get(id).cloned().ok_or_else(|| AppError::msg("This terminal has closed."))
    }

    /// Queue `data` for the terminal (never blocks: a program that isn't
    /// reading its input fills the queue, and further typing is refused).
    pub fn write(&self, id: &str, data: &str) -> AppResult<()> {
        let t = self.get(id)?;
        t.input.try_send(data.as_bytes().to_vec()).map_err(|e| match e {
            TrySendError::Full(_) => AppError::msg("The program in this terminal isn't reading input right now."),
            TrySendError::Disconnected(_) => AppError::msg("This terminal has closed."),
        })
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let t = self.get(id)?;
        let master = t.master.lock();
        let m = master.as_ref().ok_or_else(|| AppError::msg("This terminal has closed."))?;
        m.resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 }).map_err(|e| AppError::msg(e.to_string()))
    }

    pub fn close(&self, id: &str) {
        // out of the map first: killing can take a moment, and the map lock
        // shouldn't be held meanwhile
        let removed = self.map.lock().remove(id);
        if let Some(t) = removed {
            // hang up the whole job (background jobs started in the shell
            // too), like closing a terminal window
            #[cfg(unix)]
            if let Some(pid) = t.pid {
                crate::codetools::signal_process_group(pid, libc::SIGHUP);
            }
            let _ = t.killer.lock().kill();
            t.master.lock().take();
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

    #[test]
    fn output_is_batched_at_character_boundaries() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let e = "é".as_bytes().to_vec();
        tx.send(b"a".to_vec()).unwrap();
        tx.send(vec![e[0]]).unwrap(); // half of a character
        tx.send(vec![e[1]]).unwrap();
        tx.send(vec![b'x'; OUTPUT_MAX * 2]).unwrap();
        drop(tx);
        let mut got = Vec::new();
        pump_output(rx, |t| got.push(t));
        assert_eq!(got.concat(), format!("aé{}", "x".repeat(OUTPUT_MAX * 2)));
        assert!(got.len() <= 3, "batched, not one event per chunk: {}", got.len());
        assert!(got.iter().all(|t| !t.contains('\u{fffd}')));
    }

    #[cfg(unix)]
    #[test]
    fn writes_never_block_on_a_program_that_isnt_reading() {
        let terms = Terminals::default();
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "sleep 30"]);
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut writer = pair.master.take_writer().unwrap();
        let (input, input_rx) = sync_channel::<Vec<u8>>(INPUT_QUEUE);
        std::thread::spawn(move || {
            for d in input_rx {
                if writer.write_all(&d).is_err() {
                    break;
                }
            }
        });
        let t = Arc::new(Term { id: "t".into(), conv_id: "c".into(), cwd: ".".into(), master: Mutex::new(Some(pair.master)), input, killer: Mutex::new(child.clone_killer()), pid: child.process_id() });
        terms.map.lock().insert("t".into(), t);
        // far more than a PTY buffers: every call returns at once, and once the
        // queue is full typing is refused instead of hanging the caller
        let started = Instant::now();
        let chunk = "x".repeat(4096);
        let results: Vec<_> = (0..INPUT_QUEUE + 200).map(|_| terms.write("t", &chunk)).collect();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(results.iter().any(|r| r.as_ref().is_err_and(|e| e.to_string().contains("isn't reading"))));
        terms.close("t");
        let _ = child.wait();
        assert!(terms.write("t", "x").is_err(), "closed");
    }

    #[cfg(unix)]
    #[test]
    fn an_exited_shell_is_forgotten() {
        let terms = Terminals::default();
        let app = tauri::test::mock_app();
        let opened = terms.open(app.handle(), "c", ".", 80, 24).unwrap();
        let id = opened["id"].as_str().unwrap().to_string();
        terms.write(&id, "exit\r").unwrap();
        // once it has ended it's out of the map: closing the conversation
        // later can't signal whatever process reuses its pid
        let start = Instant::now();
        while terms.map.lock().contains_key(&id) && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!terms.map.lock().contains_key(&id));
        assert!(terms.list("c").is_empty());
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
