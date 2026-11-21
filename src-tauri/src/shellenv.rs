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

/// Wait, off the async runtime, until the PATH is known. Finding it runs
/// the login shell (up to 4 s with a slow profile, at start) and every
/// `path`, `command` or `which` meanwhile blocks its thread: async code
/// that may run that early (the panes a restored session opens with)
/// awaits this first, so no runtime worker waits for the shell.
pub async fn ready() {
    if PATH.get().is_none() {
        let _ = tokio::task::spawn_blocking(init).await;
    }
}

/// Where tools like node, npx, gh or claude usually live (Homebrew on Apple
/// silicon and on Intel, the system's own folders): added when they exist,
/// so they are found even when the login shell can't tell us its PATH.
#[cfg(unix)]
const COMMON_BIN_DIRS: [&str; 4] = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"];

fn compute() -> OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    #[cfg(unix)]
    {
        let login = login_shell_path();
        if login.is_none() {
            // a slow or broken shell profile: a GUI launch then only has
            // launchd's bare PATH, which lacks Homebrew and /usr/local
            log::info!("couldn't read the login shell's PATH; adding the common install folders to the app's own");
        }
        let common: Vec<std::path::PathBuf> = COMMON_BIN_DIRS.iter().map(std::path::PathBuf::from).collect();
        if let Some(joined) = merge_paths(login.as_deref(), &current, &common) {
            return joined;
        }
    }
    current
}

/// The login shell's PATH (when known), then the app's own, then those of
/// `common` that exist: each folder once, empty entries left out.
#[cfg(unix)]
fn merge_paths(login: Option<&std::ffi::OsStr>, current: &std::ffi::OsStr, common: &[std::path::PathBuf]) -> Option<OsString> {
    let mut parts: Vec<std::path::PathBuf> = vec![];
    let listed = login.into_iter().chain(std::iter::once(current)).flat_map(std::env::split_paths);
    for p in listed.chain(common.iter().filter(|p| p.exists()).cloned()) {
        if !p.as_os_str().is_empty() && !parts.contains(&p) {
            parts.push(p);
        }
    }
    std::env::join_paths(parts).ok()
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

    #[tokio::test]
    async fn ready_resolves_the_path() {
        ready().await;
        assert!(PATH.get().is_some());
        assert_eq!(path(), *PATH.get().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn common_folders_are_added_even_without_the_login_path() {
        let brew = std::env::temp_dir().join(format!("cr-brew-{}", crate::util::new_id()));
        std::fs::create_dir_all(&brew).unwrap();
        let missing = std::path::PathBuf::from("/nonexistent/cr-bin");
        let common = vec![brew.clone(), missing.clone(), "/usr/bin".into()];
        let split = |p: OsString| std::env::split_paths(&p).collect::<Vec<_>>();
        // the login shell's PATH couldn't be read: launchd's bare PATH, plus the common folders
        let p = split(merge_paths(None, std::ffi::OsStr::new("/usr/bin:/bin"), &common).unwrap());
        assert_eq!(p, vec![std::path::PathBuf::from("/usr/bin"), "/bin".into(), brew.clone()]);
        // with it: login first, then the app's, then what is missing; no repeats, no empty entries
        let p = split(merge_paths(Some(std::ffi::OsStr::new("/login/bin::/usr/bin")), std::ffi::OsStr::new(""), &common).unwrap());
        assert_eq!(p, vec![std::path::PathBuf::from("/login/bin"), "/usr/bin".into(), brew.clone()]);
        let _ = std::fs::remove_dir_all(&brew);
    }

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
