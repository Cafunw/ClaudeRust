//! Updates: newer versions are published as signed GitHub Releases (a
//! `latest.json` manifest next to the installers); the Tauri updater plugin
//! checks the manifest, verifies the download's signature against the public
//! key in `tauri.conf.json`, and installs it.
//!
//! The app checks on its own a little after start and then every few hours
//! (Settings → About → "Check for updates automatically"); Settings → About
//! can also check, download and install by hand. On Windows the installer
//! takes over and restarts the app; elsewhere the UI offers a restart.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// Where the releases are, for "download it yourself" when installing fails.
pub const RELEASES_URL: &str = "https://github.com/Cafunw/ClaudeRust/releases/latest";

/// First automatic check after start, then the interval between checks.
const FIRST_CHECK: Duration = Duration::from_secs(20);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Limit for the whole update check (a small manifest).
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest the download may go without receiving anything. The installer
/// itself may take as long as the connection needs, so it has no overall
/// limit, but a stalled download fails instead of hanging the install.
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

fn updater(app: &AppHandle) -> AppResult<tauri_plugin_updater::Updater> {
    #[allow(unused_mut)]
    let mut b = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        // applies to the check and the download alike
        .configure_client(|c| c.read_timeout(DOWNLOAD_IDLE_TIMEOUT));
    // debug builds can be pointed at a test manifest
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("CLAUDERUST_UPDATE_ENDPOINT") {
        let url = url.parse().map_err(|e| AppError::msg(format!("CLAUDERUST_UPDATE_ENDPOINT: {e}")))?;
        b = b.endpoints(vec![url]).map_err(|e| AppError::msg(e.to_string()))?;
    }
    b.build().map_err(|e| AppError::coded("update_unavailable", Value::Null, format!("Updates aren't available in this build: {e}")))
}

fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

/// The update on offer, if any: `{available, current, version?, notes?, date?, releases_url}`.
pub async fn check(app: &AppHandle) -> AppResult<Value> {
    let found = updater(app)?.check().await.map_err(|e| AppError::coded("update_check_failed", Value::Null, format!("Couldn't check for updates: {e}")))?;
    Ok(describe(&current_version(app), found.as_ref().map(|u| (u.version.as_str(), u.body.as_deref(), u.date.map(|d| d.to_string())))))
}

fn describe(current: &str, update: Option<(&str, Option<&str>, Option<String>)>) -> Value {
    match update {
        Some((version, notes, date)) => json!({
            "available": true, "current": current, "version": version,
            "notes": notes.unwrap_or("").trim(), "date": date, "releases_url": RELEASES_URL,
        }),
        None => json!({"available": false, "current": current, "releases_url": RELEASES_URL}),
    }
}

/// Download, verify and install the update on offer, reporting progress as
/// `update-progress` events (`{downloaded, total}`). On Windows the installer
/// then restarts the app; elsewhere the caller restarts it.
pub async fn install(app: &AppHandle) -> AppResult<()> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err(AppError::coded("update_busy", Value::Null, "An update is already being installed."));
    }
    let r = install_inner(app).await;
    INSTALLING.store(false, Ordering::SeqCst);
    r
}

async fn install_inner(app: &AppHandle) -> AppResult<()> {
    let mut update = updater(app)?
        .check()
        .await
        .map_err(|e| AppError::coded("update_check_failed", Value::Null, format!("Couldn't check for updates: {e}")))?
        .ok_or_else(|| AppError::coded("update_none", Value::Null, "ClaudeRust is up to date."))?;
    // The check's limit is for the check: as a total request timeout on the
    // download it would fail any installer that takes longer to arrive. (The
    // plugin documents the builder's timeout as covering downloads too;
    // 2.14.1 happens not to pass it on to the update it returns.)
    update.timeout = None;
    let progress_app = app.clone();
    let mut downloaded: u64 = 0;
    let mut last = Instant::now() - Duration::from_secs(1);
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                // a few events a second are enough for a progress bar
                if last.elapsed() >= Duration::from_millis(150) || total == Some(downloaded) {
                    last = Instant::now();
                    let _ = progress_app.emit("update-progress", json!({"downloaded": downloaded, "total": total}));
                }
            },
            || {},
        )
        .await
        .map_err(|e| AppError::coded("update_install_failed", Value::Null, format!("Couldn't install the update: {e}")))
}

/// Check now and then while the app runs (when the setting is on), and tell
/// the UI once per new version (`update-available`).
pub fn start_background_checks(app: AppHandle, st: Arc<AppState>) {
    // a development build has nothing to update to, unless a test manifest is set
    if cfg!(debug_assertions) && std::env::var_os("CLAUDERUST_UPDATE_ENDPOINT").is_none() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        let mut told: Option<String> = None;
        loop {
            if st.settings.read().auto_update {
                match check(&app).await {
                    Ok(v) if v["available"] == true => {
                        let version = v["version"].as_str().unwrap_or_default().to_string();
                        if told.as_deref() != Some(version.as_str()) {
                            let _ = app.emit("update-available", &v);
                            told = Some(version);
                        }
                    }
                    Ok(_) => {}
                    // offline, rate-limited, no release yet: try again later
                    Err(e) => log::info!("update check: {e}"),
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_updates() {
        let v = describe("0.1.0", Some(("0.2.0", Some("  Faster.\n"), Some("2026-11-21".into()))));
        assert_eq!(v["available"], true);
        assert_eq!((v["current"].as_str(), v["version"].as_str(), v["notes"].as_str()), (Some("0.1.0"), Some("0.2.0"), Some("Faster.")));
        let none = describe("0.1.0", None);
        assert_eq!(none["available"], false);
        assert_eq!(none["releases_url"], RELEASES_URL);
    }

    #[test]
    fn third_party_licenses_are_current() {
        // regenerate with `python3 dev/third_party_licenses.py` after changing dependencies
        let notices = include_str!("../../THIRD_PARTY_LICENSES.md");
        let lock = include_str!("../../Cargo.lock");
        let mut missing = Vec::new();
        let mut name = "";
        for line in lock.lines() {
            if let Some(n) = line.strip_prefix("name = ") {
                name = n.trim_matches('"');
            } else if let Some(v) = line.strip_prefix("version = ") {
                let v = v.trim_matches('"');
                if !name.is_empty() && name != "clauderust" && !notices.contains(&format!("- {name} {v} ")) {
                    missing.push(format!("{name} {v}"));
                }
            }
        }
        assert!(missing.is_empty(), "THIRD_PARTY_LICENSES.md is out of date (run dev/third_party_licenses.py): {missing:?}");
        for lib in ["KaTeX", "xterm.js", "Feather"] {
            assert!(notices.contains(lib), "{lib}");
        }
    }

    #[test]
    fn config_has_a_key_and_an_https_endpoint() {
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let up = &conf["plugins"]["updater"];
        assert!(up["pubkey"].as_str().is_some_and(|k| k.len() > 50), "public key set");
        let eps = up["endpoints"].as_array().unwrap();
        assert!(!eps.is_empty() && eps.iter().all(|e| e.as_str().is_some_and(|u| u.starts_with("https://"))));
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
        assert!(RELEASES_URL.starts_with(eps[0].as_str().unwrap().split("/download/").next().unwrap()), "same repository");
    }
}
