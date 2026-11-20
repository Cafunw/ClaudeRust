//! Resolves the user's login-shell PATH so GUI launches can find tools like
//! `npx`, `uvx`, `node` or `git` (macOS/Linux GUI apps don't inherit it).

use once_cell::sync::OnceCell;
use std::ffi::OsString;

static PATH: OnceCell<OsString> = OnceCell::new();

pub fn init() {
    let _ = PATH.get_or_init(compute);
}

pub fn path() -> OsString {
    PATH.get_or_init(compute).clone()
}

fn compute() -> OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    #[cfg(unix)]
    {
        if let Some(login) = login_shell_path() {
            let mut parts: Vec<std::path::PathBuf> = std::env::split_paths(&login).collect();
            for p in std::env::split_paths(&current) {
                if !parts.contains(&p) {
                    parts.push(p);
                }
            }
            for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
                let p = std::path::PathBuf::from(extra);
                if p.exists() && !parts.contains(&p) {
                    parts.push(p);
                }
            }
            if let Ok(joined) = std::env::join_paths(parts) {
                return joined;
            }
        }
    }
    current
}

#[cfg(unix)]
fn login_shell_path() -> Option<OsString> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut child = Command::new(&shell)
        .args(["-ilc", "printf '__PATH__%s__END__' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(4) {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let start = s.find("__PATH__")? + 8;
    let end = s[start..].find("__END__")? + start;
    let p = s[start..end].trim();
    if p.is_empty() {
        None
    } else {
        Some(OsString::from(p))
    }
}

/// Build a std Command for `program`, resolving it on the login PATH and
/// handling Windows .cmd/.bat shims.
/// Whether a program can be found on the user's (login shell) PATH.
pub fn which(program: &str) -> bool {
    which::which_in(program, Some(path()), std::env::current_dir().unwrap_or_default()).is_ok()
}

pub fn command(program: &str, args: &[String]) -> tokio::process::Command {
    let path = path();
    let resolved = which::which_in(program, Some(&path), std::env::current_dir().unwrap_or_default())
        .map(|p| p.into_os_string())
        .unwrap_or_else(|_| OsString::from(program));
    #[cfg(windows)]
    let mut cmd = {
        let lower = resolved.to_string_lossy().to_ascii_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(&resolved).args(args);
            c
        } else {
            let mut c = tokio::process::Command::new(&resolved);
            c.args(args);
            c
        }
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new(&resolved);
        c.args(args);
        c
    };
    cmd.env("PATH", &path);
    crate::display::scrub_child_env(&mut cmd);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}
