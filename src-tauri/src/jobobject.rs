//! Child processes end with the app, also when it crashes or is killed.
//!
//! Windows: at start the app's process joins a job object that kills every
//! process in it once its last handle closes, which the kernel does when the
//! app's process ends, however it ends. Children start in their parent's job,
//! so the Claude Code CLI, MCP servers, terminals and shells need nothing at
//! their spawn sites. A deliberate exit (quit, restart, the updater) lifts the
//! kill first, so what is meant to outlive the app does: the new instance, the
//! installer, a browser or editor the app opened (the app has stopped its own
//! children by then). A process that asks to leave the job may.
//!
//! Linux: PR_SET_PDEATHSIG is not used. It fires when the *thread* that
//! spawned the child ends, and children are spawned from tokio threads that
//! end on their own (an idle blocking thread after 10 s), so it would kill
//! live children; it would also have to be set in every spawn site's pre_exec.
//! macOS has no parent-death signal. On both, the app stops its children on an
//! orderly exit (`RunEvent::Exit` in lib.rs); after a crash they keep running.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// The app's job. Kept in the app's resource table, which Tauri clears on
/// every deliberate exit (`cleanup_before_exit`): dropping it lifts the kill.
pub struct Job(isize);

impl tauri::Resource for Job {}

impl Drop for Job {
    fn drop(&mut self) {
        if let Err(e) = limit(Some(self.0), JOB_OBJECT_LIMIT_BREAKAWAY_OK, false) {
            log::warn!("job object: lifting the kill failed: {e}");
        }
    }
}

/// Puts the app's process in a new job that ends it and its descendants
/// together. None (logged) when Windows refuses, for example when the app
/// runs in a job that allows no job inside it.
pub fn start() -> Option<Job> {
    match limit(None, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK, true) {
        Ok(h) => Some(Job(h)),
        Err(e) => {
            log::warn!("job object: child processes may outlive a crash of the app: {e}");
            None
        }
    }
}

/// Sets `flags` as the limits of `job` (a new job when None) and, with
/// `join`, puts the app's process in it. Returns the job's handle.
fn limit(job: Option<isize>, flags: u32, join: bool) -> std::io::Result<isize> {
    // SAFETY: `job` is a handle this module made and never closes (closing
    // it is what kills); GetCurrentProcess is a pseudo handle, always valid;
    // `info` is a C struct for which all zeroes is valid, and it outlives the
    // call that reads it, with its exact size.
    unsafe {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = flags;
        let h = job.map_or_else(|| CreateJobObjectW(std::ptr::null(), std::ptr::null()), |j| j as HANDLE);
        let ok = !h.is_null()
            && SetInformationJobObject(h, JobObjectExtendedLimitInformation, (&raw const info).cast(), size_of_val(&info) as u32) != 0
            && (!join || AssignProcessToJobObject(h, GetCurrentProcess()) != 0);
        if ok {
            return Ok(h as isize);
        }
        let e = std::io::Error::last_os_error();
        if job.is_none() && !h.is_null() {
            CloseHandle(h);
        }
        Err(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{ChildStdout, Command, Stdio};
    use std::time::Duration;

    /// Run by the tests below in a process of its own, as the app: it joins
    /// a job, starts a grandchild that writes to the test's pipe, and waits
    /// to be killed.
    #[test]
    #[ignore]
    fn helper() {
        let Ok(mode) = std::env::var("JOBOBJECT_HELPER") else { return };
        let job = start().expect("job object");
        let mut child = Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdin(Stdio::null()).stdout(Stdio::inherit()).spawn().unwrap();
        if mode == "exit" {
            drop(job); // what cleanup_before_exit does
        } else {
            std::mem::forget(job);
        }
        println!("grandchild {}", child.id());
        let _ = child.wait();
    }

    /// The helper, its grandchild's pid and the pipe both write to.
    #[allow(clippy::zombie_processes)] // the callers kill it and wait for it
    fn app(mode: &str) -> (std::process::Child, u32, BufReader<ChildStdout>) {
        let mut app = Command::new(std::env::current_exe().unwrap())
            .args(["jobobject::tests::helper", "--exact", "--ignored", "--nocapture"])
            .env("JOBOBJECT_HELPER", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(app.stdout.take().unwrap());
        let mut line = vec![];
        loop {
            line.clear();
            // bytes: ping writes in the console's code page
            assert!(out.read_until(b'\n', &mut line).unwrap() > 0, "the helper ended early");
            if let Some(pid) = String::from_utf8_lossy(&line).trim().strip_prefix("grandchild ") {
                return (app, pid.parse().unwrap(), out);
            }
        }
    }

    /// Whether every writer of `out` ends within `t`.
    fn closes_within(mut out: BufReader<ChildStdout>, t: Duration) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut out, &mut std::io::sink());
            let _ = tx.send(());
        });
        rx.recv_timeout(t).is_ok()
    }

    #[test]
    fn children_end_with_a_crashed_app() {
        let (mut app, _, out) = app("crash");
        app.kill().unwrap(); // TerminateProcess: nothing of the app runs, as in a crash
        let _ = app.wait();
        assert!(closes_within(out, Duration::from_secs(10)), "the grandchild outlived the app");
    }

    #[test]
    fn a_deliberate_exit_lets_them_live() {
        let (mut app, pid, out) = app("exit");
        app.kill().unwrap();
        let _ = app.wait();
        let alive = !closes_within(out, Duration::from_secs(3));
        let _ = Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output();
        assert!(alive, "the grandchild ended with the app");
    }

    /// What lifting the kill relies on.
    #[test]
    fn tauri_drops_the_app_resources_before_a_deliberate_exit() {
        struct Flag(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl tauri::Resource for Flag {}
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        use tauri::Manager;
        let app = tauri::test::mock_app();
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        app.resources_table().add(Flag(dropped.clone()));
        app.handle().cleanup_before_exit();
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }
}
