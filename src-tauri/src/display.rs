//! Make the window open and render reliably over Remote Desktop, VNC, in
//! virtual machines and with GPU drivers the system WebView doesn't like:
//! choose WebView rendering flags before any window exists, and keep the
//! main window on a monitor that is actually there.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Same folder Tauri uses for `app_data_dir` (data dir + identifier); read
/// before Tauri starts, so it can't come from the path resolver.
const IDENTIFIER: &str = "dev.clauderust.desktop";
const FLAG_FILE: &str = "render.json";

static INFO: OnceLock<Value> = OnceLock::new();
/// Variables `prepare` set for our own WebView, removed again for child
/// processes (shell commands, MCP servers, the Claude Code CLI) so a GL app
/// or a WebView2 app the user runs isn't affected.
static SET_VARS: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

fn set_ours(key: &'static str, val: &str) {
    std::env::set_var(key, val);
    if let Ok(mut v) = SET_VARS.lock() {
        v.push(key);
    }
}

/// Remove the rendering variables we set from a child's environment.
pub fn scrub_child_env(cmd: &mut tokio::process::Command) {
    if let Ok(v) = SET_VARS.lock() {
        for k in v.iter() {
            cmd.env_remove(k);
        }
    }
}

fn flag_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(IDENTIFIER).join(FLAG_FILE))
}

fn env_true(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// "auto" | "on" | "off". `--disable-gpu` or `CLAUDERUST_DISABLE_GPU=1`
/// force "off", for when the window can't be opened to change the setting.
fn configured_mode() -> String {
    if std::env::args().any(|a| a == "--disable-gpu") || env_true("CLAUDERUST_DISABLE_GPU") {
        return "off".into();
    }
    flag_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["hardware_acceleration"].as_str().map(String::from))
        .filter(|m| matches!(m.as_str(), "auto" | "on" | "off"))
        .unwrap_or_else(|| "auto".into())
}

/// Remember the setting where `prepare` can read it on the next start.
pub fn write_mode(mode: &str) -> std::io::Result<()> {
    let Some(p) = flag_path() else { return Ok(()) };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, json!({"hardware_acceleration": mode}).to_string())
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
}

/// Is this session displayed remotely (RDP, VNC, xrdp, X forwarding...)?
pub fn is_remote_session() -> bool {
    #[cfg(windows)]
    {
        const SM_REMOTESESSION: i32 = 0x1000;
        // SAFETY: GetSystemMetrics has no preconditions.
        let rdp = unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0;
        rdp || std::env::var("SESSIONNAME").map(|s| s.to_ascii_uppercase().starts_with("RDP-")).unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        let set = |k: &str| std::env::var_os(k).map(|v| !v.is_empty()).unwrap_or(false);
        let forwarded_x = set("SSH_CONNECTION")
            && std::env::var("DISPLAY").map(|d| !d.starts_with(':')).unwrap_or(false);
        set("XRDP_SESSION") || set("VNCDESKTOP") || set("CHROME_REMOTE_DESKTOP_SESSION") || set("NX_SESSION_ID") || forwarded_x
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

#[allow(dead_code)]
fn set_default(key: &'static str, val: &str) {
    if std::env::var_os(key).is_none() {
        set_ours(key, val);
    }
}

/// Call first thing in `run()`, before the WebView starts and before other
/// threads exist (it sets environment variables).
pub fn prepare() {
    let mode = configured_mode();
    let remote = is_remote_session();
    let software = mode == "off" || (mode == "auto" && remote);

    #[cfg(windows)]
    if software {
        let user_set = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").ok();
        // Keep wry's own defaults: this variable can take the place of the
        // arguments it passes.
        let mut args = user_set
            .clone()
            .unwrap_or_else(|| "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection".into());
        if !args.contains("--disable-gpu") {
            args.push_str(" --disable-gpu --disable-gpu-compositing");
            if user_set.is_some() {
                std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", args.trim());
            } else {
                set_ours("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", args.trim());
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // WebKitGTK's DMA-BUF renderer gives blank or white windows on many
        // NVIDIA setups, in VMs and over xrdp/VNC.
        set_default("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        if software {
            set_default("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
        if mode == "off" {
            set_default("LIBGL_ALWAYS_SOFTWARE", "1");
        }
    }

    let _ = INFO.set(json!({
        "mode": mode,
        "remote": remote,
        "software": software,
    }));
}

/// What `prepare` decided, for Settings.
pub fn info() -> Value {
    INFO.get().cloned().unwrap_or_else(|| json!({"mode": "auto", "remote": false, "software": false}))
}

/// After the saved window position is restored, make sure the window is
/// reachable: if no monitor shows its title bar (a monitor was unplugged, the
/// Remote Desktop resolution is smaller...) center it, and shrink it if it is
/// larger than the screen.
pub fn ensure_on_screen(w: &tauri::WebviewWindow) {
    // The OS places maximized and fullscreen windows itself (and on Windows a
    // maximized window's outer size includes borders larger than the screen).
    if w.is_maximized().unwrap_or(false) || w.is_fullscreen().unwrap_or(false) {
        return;
    }
    let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) else { return };
    // GTK reports a zero size for a window that isn't mapped yet; the
    // delayed check in `ensure_on_screen_soon` covers that case.
    if size.width < 50 || size.height < 50 {
        return;
    }
    let monitors = w.available_monitors().unwrap_or_default();
    if monitors.is_empty() {
        return;
    }
    let title_visible = monitors.iter().any(|m| {
        let (mp, ms) = (m.position(), m.size());
        let x0 = pos.x.max(mp.x);
        let x1 = (pos.x + size.width as i32).min(mp.x + ms.width as i32);
        let y0 = pos.y.max(mp.y);
        let y1 = (pos.y + 40).min(mp.y + ms.height as i32);
        x1 - x0 >= 120 && y1 - y0 >= 20
    });
    let target = w
        .current_monitor()
        .ok()
        .flatten()
        .filter(|_| title_visible)
        .or_else(|| w.primary_monitor().ok().flatten())
        .or_else(|| monitors.first().cloned());
    let Some(m) = target else { return };
    let ms = m.size();
    let too_big = size.width > ms.width || size.height > ms.height;
    if title_visible && !too_big {
        return;
    }
    if too_big || !title_visible {
        let width = size.width.min(ms.width * 9 / 10).max(400);
        let height = size.height.min(ms.height * 9 / 10).max(300);
        let _ = w.set_size(tauri::PhysicalSize::new(width, height));
        // Center on the chosen monitor (center() would use the current one,
        // which may be the missing monitor).
        let x = m.position().x + (ms.width as i32 - width as i32) / 2;
        let y = m.position().y + (ms.height as i32 - height as i32) / 2;
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// Run `ensure_on_screen` again once the window is shown and its geometry
/// is real (needed on Linux, harmless elsewhere).
pub fn ensure_on_screen_soon(w: &tauri::WebviewWindow) {
    let w = w.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        let w2 = w.clone();
        let _ = w.run_on_main_thread(move || ensure_on_screen(&w2));
    });
}
