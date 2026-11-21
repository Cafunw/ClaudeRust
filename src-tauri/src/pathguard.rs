//! Which paths the webview may hand to the OS (`open_path`, `reveal_path`):
//! a page running injected script must not get any file on the disk opened.

use crate::error::{AppError, AppResult};
use std::path::{Component, Path, PathBuf, Prefix};

/// `path` with links and `..` resolved; None for a relative or missing path
/// and, without looking at it, for a UNC or device path (looking connects to
/// the server).
fn resolve(path: &Path) -> Option<PathBuf> {
    let remote = matches!(path.components().next(), Some(Component::Prefix(x)) if !matches!(x.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)));
    if remote || !path.is_absolute() {
        return None;
    }
    path.canonicalize().ok()
}

fn inside(p: &Path, root: &Path) -> bool {
    resolve(root).is_some_and(|r| p.starts_with(r))
}

fn refused(path: &str) -> AppError {
    AppError::msg(format!("ClaudeRust only opens its own data and the folders of your sessions, not {path}."))
}

/// For the system's default app: the MCP config file and folders of the
/// app's data (the data dir, its logs). A file elsewhere could be a program.
pub fn openable(path: &str, data_dir: &Path, mcp_config: &Path) -> AppResult<()> {
    let ok = resolve(Path::new(path)).is_some_and(|p| (p.is_dir() && inside(&p, data_dir)) || resolve(mcp_config) == Some(p));
    ok.then_some(()).ok_or_else(|| refused(path))
}

/// For the file manager: the MCP config file, anything in the app's data
/// and in a session's folder (`roots`).
pub fn revealable(path: &str, data_dir: &Path, mcp_config: &Path, roots: &[String]) -> AppResult<()> {
    let ok = resolve(Path::new(path)).is_some_and(|p| {
        resolve(mcp_config).as_ref() == Some(&p) || std::iter::once(data_dir).chain(roots.iter().map(Path::new)).any(|r| inside(&p, r))
    });
    ok.then_some(()).ok_or_else(|| refused(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_app_data_and_session_folders() {
        let base = std::env::temp_dir().join(format!("pathguard-{}", crate::util::new_id()));
        let (data, proj, other) = (base.join("data"), base.join("proj"), base.join("other"));
        for d in [data.join("logs"), proj.join("src"), other.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        let config = base.join("config").join("claude_desktop_config.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        for f in [&config, &data.join("clauderust.db"), &proj.join("src").join("run.bat"), &other.join("x.exe")] {
            std::fs::write(f, "x").unwrap();
        }
        let s = |p: &Path| p.display().to_string();
        let open = |p: &Path| openable(&s(p), &data, &config).is_ok();
        let roots = [s(&proj), String::new(), "relative/dir".into()];
        let reveal = |p: &Path| revealable(&s(p), &data, &config, &roots).is_ok();

        assert!(open(&data) && open(&data.join("logs")) && open(&config));
        // no file but the config, nothing outside the data dir, nothing missing or relative
        assert!(!open(&data.join("clauderust.db")) && !open(&proj) && !open(&proj.join("src").join("run.bat")));
        assert!(!open(&data.join("logs").join("..").join("..").join("other")) && !open(&data.join("nope")));
        assert!(openable("data", &data, &config).is_err());

        assert!(reveal(&proj) && reveal(&proj.join("src").join("run.bat")) && reveal(&data.join("clauderust.db")) && reveal(&config));
        assert!(!reveal(&other) && !reveal(&other.join("x.exe")) && !reveal(&base));
        assert!(!reveal(&proj.join("..").join("other").join("x.exe")) && !reveal(&proj.join("missing")));
        let err = revealable(&s(&other), &data, &config, &roots).unwrap_err().to_string();
        assert!(err.contains("only opens") && err.contains(&s(&other)), "{err}");
        #[cfg(windows)]
        assert!(revealable(r"\\server\share\x", &data, &config, &[r"\\server\share".into()]).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
