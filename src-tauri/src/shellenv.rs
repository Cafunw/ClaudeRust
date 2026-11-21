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

/// The script an npm `.cmd` shim runs (`"%dp0%\node_modules\…\cli.js"`),
/// resolved against the shim's folder.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn npm_shim_target(text: &str, dir: &std::path::Path) -> Option<std::path::PathBuf> {
    const MARK: &str = "\"%dp0%\\";
    // the quoted `%dp0%\…` paths in it (node.exe comes first), the script one
    let rel = text.match_indices(MARK).filter_map(|(i, _)| {
        let rest = &text[i + MARK.len()..];
        let rel = &rest[..rest.find('"')?];
        let lower = rel.to_ascii_lowercase();
        (lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs")).then_some(rel)
    }).next()?;
    if rel.contains("..") {
        return None;
    }
    let p = rel.split(['\\', '/']).filter(|s| !s.is_empty()).fold(dir.to_path_buf(), |p, s| p.join(s));
    p.is_file().then_some(p)
}

/// An argument as cmd.exe can carry it: on one line, without `"`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn cmd_safe(a: &str) -> String {
    a.replace(['\r', '\n'], " ").replace('"', "'")
}

pub fn command(program: &str, args: &[String]) -> tokio::process::Command {
    let path = path();
    let resolved = which::which_in(program, Some(&path), std::env::current_dir().unwrap_or_default())
        .map(|p| p.into_os_string())
        .unwrap_or_else(|_| OsString::from(program));
    #[cfg(windows)]
    let mut cmd = {
        let lower = resolved.to_string_lossy().to_ascii_lowercase();
        let shim = std::path::Path::new(&resolved);
        // an npm shim (`claude.cmd`): run its script with node directly, as
        // cmd.exe would cut arguments at a line break and re-read & | < >
        let npm = if lower.ends_with(".cmd") {
            std::fs::read_to_string(shim).ok().zip(shim.parent()).and_then(|(text, dir)| npm_shim_target(&text, dir)).and_then(|js| {
                let local = shim.parent().map(|d| d.join("node.exe")).filter(|n| n.is_file());
                let node = local.or_else(|| which::which_in("node", Some(&path), std::env::current_dir().unwrap_or_default()).ok())?;
                Some((node, js))
            })
        } else {
            None
        };
        if let Some((node, js)) = npm {
            let mut c = tokio::process::Command::new(node);
            c.arg(js).args(args);
            c
        } else if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            // last resort through cmd.exe: keep every argument on one line,
            // and no `"` that would end a quoted argument early
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(&resolved).args(args.iter().map(|a| cmd_safe(a)));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_shims() {
        let d = std::env::temp_dir().join(format!("cr-shim-{}", crate::util::new_id()));
        let js = d.join("node_modules").join("@anthropic-ai").join("claude-code").join("cli.js");
        std::fs::create_dir_all(js.parent().unwrap()).unwrap();
        std::fs::write(&js, "").unwrap();
        let shim = "@ECHO off\r\nGOTO start\r\n:start\r\nSETLOCAL\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n)\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n";
        assert_eq!(npm_shim_target(shim, &d), Some(js.clone()));
        assert_eq!(npm_shim_target("@echo off\r\nclaude.exe %*", &d), None);
        assert_eq!(npm_shim_target("\"%dp0%\\..\\x.js\"", &d), None);
        assert_eq!(cmd_safe("a\nb \"c\" & d"), "a b 'c' & d");
        let _ = std::fs::remove_dir_all(&d);
    }
}
