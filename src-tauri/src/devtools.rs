//! Development-only automation (debug builds, opt-in through an environment
//! variable): lets test scripts drive the UI without a mouse.
//!
//! With `CLAUDERUST_EVAL_DIR=<dir>`, every `*.js` file dropped into `<dir>` is
//! evaluated in the main window and then deleted; scripts report back with
//! `__TAURI__.core.invoke('dev_report', {text})`, which appends to
//! `<dir>/report.log`. Compiled out of release builds.

#[cfg(debug_assertions)]
pub fn start(app: &tauri::AppHandle) {
    use tauri::Manager;
    let Some(dir) = std::env::var_os("CLAUDERUST_EVAL_DIR").map(std::path::PathBuf::from) else { return };
    let _ = std::fs::create_dir_all(&dir);
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "js").unwrap_or(false)).collect();
        files.sort();
        for f in files {
            if let Ok(js) = std::fs::read_to_string(&f) {
                let _ = std::fs::remove_file(&f);
                if let Some(w) = app.get_webview_window("main") {
                    let wrapped = format!(
                        "(async () => {{ try {{ {js}\n }} catch (e) {{ window.__TAURI__.core.invoke('dev_report', {{ text: 'ERROR ' + (e && e.stack || e) }}); }} }})();"
                    );
                    let _ = w.eval(&wrapped);
                }
            }
        }
    });
}

#[cfg(not(debug_assertions))]
pub fn start(_app: &tauri::AppHandle) {}

#[tauri::command]
pub fn dev_report(text: String) {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("CLAUDERUST_EVAL_DIR") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::path::Path::new(&dir).join("report.log")) {
            let _ = writeln!(f, "{text}");
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = text;
}
