//! Keeps the computer from sleeping while Claude works: a turn in a chat or a code session (a Claude Code
//! workflow runs inside its turn, a routine's run is a turn too). Reference-counted: the first hold asks the
//! system to stay awake, the last release lets it sleep again. The display may still turn off.

use parking_lot::Mutex;

/// While one is alive the system stays awake, unless the setting is off.
pub struct KeepAwake(());

pub fn acquire() -> KeepAwake {
    STATE.lock().hold();
    KeepAwake(())
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        STATE.lock().release();
    }
}

/// The "Keep awake while Claude works" setting (`keep_awake`); it applies to the turns already running too.
pub fn apply(s: &crate::settings::Settings) {
    STATE.lock().set_enabled(s.keep_awake);
}

static STATE: Mutex<Holds> = Mutex::new(Holds { count: 0, enabled: true, os: system });

struct Holds {
    count: usize,
    enabled: bool,
    /// Asks the system to stay awake (true) or lets it sleep again (false).
    os: fn(bool),
}

impl Holds {
    fn hold(&mut self) {
        self.count += 1;
        if self.count == 1 && self.enabled {
            (self.os)(true);
        }
    }

    fn release(&mut self) {
        self.count -= 1;
        if self.count == 0 && self.enabled {
            (self.os)(false);
        }
    }

    fn set_enabled(&mut self, on: bool) {
        if on != self.enabled {
            self.enabled = on;
            if self.count > 0 {
                (self.os)(on);
            }
        }
    }
}

#[cfg(windows)]
fn system(on: bool) {
    use std::sync::mpsc::{channel, Sender};
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    // the state belongs to the thread that sets it (and ends with it): one thread of ours sets and clears it
    static LINE: OnceLock<Option<Sender<bool>>> = OnceLock::new();
    let line = LINE.get_or_init(|| {
        let (tx, rx) = channel::<bool>();
        let worker = std::thread::Builder::new().name("keep-awake".into()).spawn(move || {
            for on in rx {
                // SAFETY: a plain call that sets this thread's execution state; no memory is shared
                unsafe { SetThreadExecutionState(if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS }) };
            }
        });
        worker.ok().map(|_| tx)
    });
    if let Some(tx) = line {
        let _ = tx.send(on);
    }
}

/// A child process that holds the system awake (macOS `caffeinate`, Linux `systemd-inhibit`, best effort:
/// skipped where it isn't installed). Each ends by itself when the app does, however the app ends.
#[cfg(unix)]
fn system(on: bool) {
    static CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);
    let mut child = CHILD.lock();
    if let Some(mut c) = child.take() {
        crate::codetools::kill_process_group(c.id());
        let _ = c.wait();
    }
    if on {
        *child = inhibitor().ok();
    }
}

#[cfg(unix)]
fn inhibitor() -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let pid = std::process::id().to_string();
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("caffeinate");
        c.args(["-i", "-w", pid.as_str()]);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut cmd = {
        let mut c = Command::new("systemd-inhibit");
        c.args(["--what=idle:sleep", "--who=ClaudeRust", "--why=Claude is working", "--mode=block", "sh", "-c", "while kill -0 \"$0\" 2>/dev/null; do sleep 10; done", pid.as_str()]);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    static CALLS: Mutex<Vec<bool>> = Mutex::new(Vec::new());

    fn record(on: bool) {
        CALLS.lock().push(on);
    }

    #[test]
    fn overlapping_holds_keep_the_system_awake_until_the_last_ends() {
        let mut h = Holds { count: 0, enabled: true, os: record };
        let calls = || std::mem::take(&mut *CALLS.lock());
        // two turns at once: one request, released by the second to end
        h.hold();
        h.hold();
        h.release();
        assert_eq!(calls(), [true]);
        h.release();
        assert_eq!(calls(), [false]);
        // the setting turned off while a turn runs lets the system sleep at once, and back on holds it again
        h.hold();
        h.set_enabled(false);
        h.set_enabled(false);
        h.set_enabled(true);
        h.release();
        assert_eq!(calls(), [true, false, true, false]);
        // off: turns ask nothing; turning it on with none running asks nothing either
        h.set_enabled(false);
        h.hold();
        h.release();
        h.set_enabled(true);
        assert!(calls().is_empty());
        assert_eq!(h.count, 0);
    }
}
