//! Give the WebView's memory back when nobody needs it: while the main
//! window is hidden (in the tray) or minimised, WebView2's memory target is
//! Low, so Edge drops renderer and GPU caches and trims its working sets;
//! shown again it is Normal. `trim_memory` does the same for a moment after
//! the page let go of a long conversation.

use std::sync::atomic::{AtomicU8, Ordering};

const UNSET: u8 = 0;
const NORMAL: u8 = 1;
const LOW: u8 = 2;
/// How long a trim keeps the target Low: Edge purges when the level drops,
/// and the person is back on a normal page well before they notice.
const TRIM_MS: u64 = 1500;

/// The level last given to the main window's WebView, so window events
/// (every step of a resize) don't call into WebView2 again for nothing.
static LEVEL: AtomicU8 = AtomicU8::new(UNSET);

/// Low while the window can't be seen.
pub fn level_for(visible: bool, minimized: bool) -> u8 {
    if !visible || minimized {
        LOW
    } else {
        NORMAL
    }
}

/// Bring the WebView's memory target in line with the window's state.
pub fn sync(w: &tauri::WebviewWindow) {
    let level = level_for(w.is_visible().unwrap_or(true), w.is_minimized().unwrap_or(false));
    if LEVEL.swap(level, Ordering::SeqCst) != level {
        set_level(w, level);
    }
}

/// The page dropped a long conversation: purge what the WebView kept for it.
#[tauri::command]
pub fn trim_memory(window: tauri::WebviewWindow) {
    // already Low (hidden or minimised): sync puts it back when it is shown
    if LEVEL.swap(LOW, Ordering::SeqCst) == LOW {
        return;
    }
    set_level(&window, LOW);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(TRIM_MS));
        let w = window.clone();
        let _ = window.run_on_main_thread(move || sync(&w));
    });
}

#[cfg(windows)]
fn set_level(w: &tauri::WebviewWindow, level: u8) {
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL};
    use windows_core::{w, Interface};
    let low = level == LOW;
    let target = if low { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
    let _ = w.with_webview(move |pw| {
        // SAFETY: COM calls on the WebView's own UI thread, where with_webview runs this
        unsafe {
            let Ok(core) = pw.controller().CoreWebView2() else { return };
            if low {
                // the Low target only trims working sets; this is what Chromium does when Windows
                // runs short of memory, and makes the renderer and GPU processes drop their caches
                let done = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = core.CallDevToolsProtocolMethod(w!("Memory.simulatePressureNotification"), w!(r#"{"level":"critical"}"#), &done);
            }
            // a WebView2 Runtime older than 114 has no memory target: it keeps its default
            if let Ok(v) = core.cast::<ICoreWebView2_19>() {
                let _ = v.SetMemoryUsageTargetLevel(target);
            }
        }
    });
}

#[cfg(not(windows))]
fn set_level(_: &tauri::WebviewWindow, _: u8) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_only_while_the_window_cant_be_seen() {
        assert_eq!(level_for(true, false), NORMAL);
        assert_eq!(level_for(false, false), LOW);
        assert_eq!(level_for(true, true), LOW);
        assert_eq!(level_for(false, true), LOW);
    }
}
