//! "Browse extensions": a curated list of public `.mcpb` releases, shipped in
//! the binary (extension_directory.json) and never fetched. Each entry is a
//! publisher's own GitHub release asset, its size and SHA-256 as GitHub's
//! release API gave them on 2026-11-22 and 2026-11-24 (only stable releases
//! whose bundle carries its own code: none that fetches the server from npm or
//! PyPI when it starts). A bundle is downloaded only when the
//! user presses Install, must have that exact hash, and is installed only
//! after the user confirmed the usual preview.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const LIST: &str = include_str!("extension_directory.json");
pub const CATEGORIES: [&str; 7] = ["dev", "data", "web", "productivity", "cloud", "security", "design"];
/// The largest bundle in the list is 63 MB.
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;
/// Where GitHub sends a release download.
const GITHUB_CDN: [&str; 3] = ["objects.githubusercontent.com", "release-assets.githubusercontent.com", "github-releases.githubusercontent.com"];

fn list() -> Vec<Value> {
    // debug builds only: a test can list bundles on a local server
    #[cfg(debug_assertions)]
    if let Some(s) = std::env::var_os("CLAUDERUST_EXTENSION_DIRECTORY").and_then(|p| std::fs::read_to_string(p).ok()) {
        return serde_json::from_str(&s).unwrap_or_default();
    }
    serde_json::from_str(LIST).unwrap_or_default()
}

fn platform() -> String {
    let os = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}-{arch}")
}

/// The asset of `entry` for `platform` ("win32-x64"…), or the one for any.
fn asset_for<'a>(entry: &'a Value, platform: &str) -> Option<&'a Value> {
    let assets = entry["assets"].as_array()?;
    assets.iter().find(|a| a["platform"] == platform).or_else(|| assets.iter().find(|a| a["platform"] == "any"))
}

/// "1.10.0" > "1.9.2"; None when either isn't dotted numbers.
fn newer(a: &str, b: &str) -> Option<bool> {
    let parse = |v: &str| v.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<u64>>>();
    Some(parse(a)? > parse(b)?)
}

/// The directory as the dialog shows it: each entry with its asset for this
/// platform (`asset`, null when none) and, when installed (from the
/// directory, or by its manifest name), `installed` {name, version} and
/// `update` (a newer version is listed).
pub fn entries(config: &Value, platform: &str) -> Vec<Value> {
    let servers = config["mcpServers"].as_object().cloned().unwrap_or_default();
    list()
        .into_iter()
        .map(|mut e| {
            let by_source = servers.iter().find(|(_, s)| s["extension"]["source"]["id"] == e["id"]).map(|(n, s)| (n.clone(), s["extension"]["source"]["version"].clone()));
            let by_name = || {
                let n = crate::extensions::sanitize_name(e["manifest_name"].as_str()?);
                let x = &servers.get(&n)?["extension"];
                x.is_object().then(|| (n, x["version"].clone()))
            };
            let installed = by_source.or_else(by_name);
            e["update"] = json!(installed.as_ref().and_then(|(_, v)| newer(e["version"].as_str()?, v.as_str()?)).unwrap_or(false));
            e["installed"] = installed.map_or(Value::Null, |(name, version)| json!({"name": name, "version": version}));
            e["asset"] = asset_for(&e, platform).cloned().unwrap_or(Value::Null);
            if let Some(o) = e.as_object_mut() {
                o.remove("assets");
            }
            e
        })
        .collect()
}

/// Redirects only to https on the first request's host or, from github.com,
/// to GitHub's release download hosts; at most 5.
fn redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|a| {
        let first = a.previous().first().and_then(|u| u.host_str()).unwrap_or_default().to_string();
        let to = a.url().host_str().unwrap_or_default();
        let ok = a.url().scheme() == "https" && (to == first || (first == "github.com" && GITHUB_CDN.contains(&to)));
        if a.previous().len() < 5 && ok {
            a.follow()
        } else {
            a.stop()
        }
    })
}

/// Whether `url` may be downloaded: https, or (debug builds, for tests) http on this machine.
fn allowed(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.scheme() == "https" || (cfg!(debug_assertions) && u.scheme() == "http" && matches!(u.host_str(), Some("127.0.0.1" | "localhost"))))
}

fn downloads(state: &AppState) -> PathBuf {
    state.data_dir.join("extensions").join(".downloads")
}

static RUNNING: parking_lot::Mutex<Option<CancellationToken>> = parking_lot::Mutex::new(None);

/// Download `asset` into `dir` (emptied first: one download is kept at a
/// time) and check its hash; the bundle's path.
async fn download(client: &reqwest::Client, asset: &Value, dir: &Path, cancel: &CancellationToken, progress: &(dyn Fn(u64, Option<u64>) + Send + Sync)) -> AppResult<PathBuf> {
    let url = asset["url"].as_str().unwrap_or_default();
    if !allowed(url) {
        return Err(AppError::coded("ed_not_https", Value::Null, format!("Only https downloads are allowed: {url}")));
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    let file = url.rsplit('/').next().filter(|f| f.ends_with(".mcpb") || f.ends_with(".dxt")).unwrap_or("extension.mcpb");
    let path = dir.join(crate::extensions::sanitize_name(file.trim_end_matches(".mcpb").trim_end_matches(".dxt")) + ".mcpb");
    let max = asset["size"].as_u64().map_or(MAX_DOWNLOAD, |s| s.min(MAX_DOWNLOAD));
    let (_, sha) = crate::node_runtime::fetch_to(client, url, &path, max, cancel, progress).await?;
    if asset["sha256"].as_str() != Some(sha.as_str()) {
        let _ = std::fs::remove_file(&path);
        return Err(AppError::coded("ed_checksum", Value::Null, format!("The download from {url} doesn't have the SHA-256 the directory lists (it has {sha}); it was deleted and not installed.")));
    }
    Ok(path)
}

/// The installed extensions the directory lists a newer version of, with a build for `platform`:
/// {id, server (its name in the config), name, from, to}.
pub fn updates(config: &Value, platform: &str) -> Vec<Value> {
    entries(config, platform)
        .into_iter()
        .filter(|e| e["update"] == true && !e["asset"].is_null())
        .map(|e| json!({"id": e["id"], "server": e["installed"]["name"], "name": e["name"], "from": e["installed"]["version"], "to": e["version"]}))
        .collect()
}

/// After start and then once a day: the installed extensions that have an update
/// ("extension-updates" {available, updated}); with Update automatically on, the
/// ones that can go in without asking (extensions::quiet_update) are installed first.
pub fn start_update_checks(app: tauri::AppHandle, st: Arc<AppState>) {
    use tauri::{Emitter, Manager};
    tauri::async_runtime::spawn(async move {
        // the window is listening by then, and the start isn't slowed
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        loop {
            let available = || updates(&crate::mcp::read_config(&st.mcp.config_path), &platform());
            let mut updated = vec![];
            if st.settings.read().extensions_auto_update {
                for u in available() {
                    match install_quietly(&app, &st, u["id"].as_str().unwrap_or_default()).await {
                        Ok(true) => updated.push(json!({"name": u["name"], "to": u["to"]})),
                        Ok(false) => {}
                        Err(e) => log::warn!("updating extension {}: {e}", u["id"]),
                    }
                }
                if !updated.is_empty() {
                    let _ = crate::commands::mcp_reload(app.clone(), app.state::<Arc<AppState>>()).await;
                }
            }
            let _ = app.emit("extension-updates", json!({"available": available(), "updated": updated}));
            tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
        }
    });
}

/// Download entry `id`'s update and install it if it needs no one to look at it; whether it did.
async fn install_quietly(app: &tauri::AppHandle, st: &Arc<AppState>, id: &str) -> AppResult<bool> {
    let (entry, _, path) = fetch(app, st, id).await?;
    let p = path.clone();
    let info = crate::extensions::on_pool(move || crate::extensions::inspect(&p)).await?;
    if !crate::extensions::quiet_update(&info, &st.mcp.config_path) {
        let _ = std::fs::remove_file(&path);
        return Ok(false);
    }
    let (ext, config) = (st.data_dir.join("extensions"), st.mcp.config_path.clone());
    crate::extensions::on_pool(move || install_entry(&entry, &path, &ext, &config, &json!({}))).await?;
    Ok(true)
}

#[tauri::command]
pub async fn extension_directory(state: tauri::State<'_, Arc<AppState>>) -> AppResult<Value> {
    Ok(Value::Array(entries(&crate::mcp::read_config(&state.mcp.config_path), &platform())))
}

/// Download directory entry `id` for this platform ("extension-download-progress"
/// events carry {downloaded, total}); its install preview (extensions::inspect)
/// with `path`, `source_url`, `entry` and `quiet` (an update that can go in
/// without the preview, see extensions::quiet_update).
#[tauri::command]
pub async fn extension_directory_download(app: tauri::AppHandle, state: tauri::State<'_, Arc<AppState>>, id: String) -> AppResult<Value> {
    let (entry, asset, path) = fetch(&app, &state, &id).await?;
    let p = path.clone();
    let mut info = crate::extensions::on_pool(move || crate::extensions::inspect(&p)).await?;
    info["quiet"] = json!(crate::extensions::quiet_update(&info, &state.mcp.config_path));
    info["path"] = json!(path.display().to_string());
    info["source_url"] = asset["url"].clone();
    info["entry"] = json!({"id": entry["id"], "name": entry["name"], "publisher": entry["publisher"], "version": entry["version"], "homepage": entry["homepage"], "repository": entry["repository"], "license": entry["license"]});
    Ok(info)
}

/// Download directory entry `id`'s bundle for this platform (one download at a
/// time, cancelled by extension_directory_cancel): the entry, its asset, the bundle.
async fn fetch(app: &tauri::AppHandle, state: &AppState, id: &str) -> AppResult<(Value, Value, PathBuf)> {
    use tauri::Emitter;
    let entry = list().into_iter().find(|e| e["id"] == id).ok_or_else(|| AppError::coded("ed_unknown", Value::Null, format!("“{id}” isn't in the extension directory.")))?;
    let asset = asset_for(&entry, &platform()).cloned().ok_or_else(|| AppError::coded("ed_no_build", Value::Null, "This extension has no build for this computer's system."))?;
    let cancel = {
        let mut r = RUNNING.lock();
        if r.is_some() {
            return Err(AppError::coded("ed_busy", Value::Null, "Another extension is being downloaded."));
        }
        r.insert(CancellationToken::new()).clone()
    };
    let dir = downloads(state);
    let res = async {
        let public = asset["url"].as_str().is_some_and(|u| u.starts_with("https://"));
        let client = crate::mcp_oauth::download_client(redirects(), public)?;
        download(&client, &asset, &dir, &cancel, &|done, total| {
            let _ = app.emit("extension-download-progress", json!({"id": id, "downloaded": done, "total": total}));
        })
        .await
    }
    .await;
    *RUNNING.lock() = None;
    Ok((entry, asset, res?))
}

/// Install `bundle` (downloaded for directory `entry`), noting where it came from;
/// the bundle is deleted once installed.
fn install_entry(entry: &Value, bundle: &Path, ext: &Path, config: &Path, values: &Value) -> AppResult<String> {
    let (_, sha) = crate::extension_sign::check_file(bundle)?;
    let source = json!({"id": entry["id"], "version": entry["version"], "sha256": sha});
    let r = crate::extensions::install_from(bundle, ext, config, values, Some(&source));
    // a failed install keeps the bundle, so the form can be corrected and sent again
    if r.is_ok() {
        let _ = std::fs::remove_file(bundle);
    }
    r
}

#[tauri::command]
pub async fn extension_directory_cancel() -> AppResult<()> {
    if let Some(t) = RUNNING.lock().as_ref() {
        t.cancel();
    }
    Ok(())
}

/// Install the bundle `extension_directory_download` left at `path` for
/// entry `id`, noting it came from the directory; the bundle is deleted after.
#[tauri::command]
pub async fn extension_directory_install(app: tauri::AppHandle, state: tauri::State<'_, Arc<AppState>>, id: String, path: String, values: Value) -> AppResult<Value> {
    let dir = downloads(&state);
    let bundle = PathBuf::from(&path);
    if bundle.parent() != Some(dir.as_path()) {
        return Err(AppError::coded("ed_not_downloaded", Value::Null, "Only a bundle the directory downloaded can be installed this way."));
    }
    let entry = list().into_iter().find(|e| e["id"] == id.as_str()).ok_or_else(|| AppError::coded("ed_unknown", Value::Null, format!("“{id}” isn't in the extension directory.")))?;
    let (ext, config) = (state.data_dir.join("extensions"), state.mcp.config_path.clone());
    let name = crate::extensions::on_pool(move || install_entry(&entry, &bundle, &ext, &config, &values)).await?;
    let mut status = crate::commands::mcp_reload(app, state).await?;
    status["installed"] = json!(name);
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_entry_is_complete_pinned_and_from_its_publishers_release() {
        let list: Vec<Value> = serde_json::from_str(LIST).expect("valid JSON");
        assert!(list.len() >= 10, "{} entries", list.len());
        let mut ids = HashSet::new();
        for e in &list {
            let id = e["id"].as_str().unwrap_or_default();
            assert!(!id.is_empty() && ids.insert(id), "id {id:?} empty or twice");
            for k in ["name", "publisher", "license", "version", "homepage", "repository"] {
                assert!(!e[k].as_str().unwrap_or_default().trim().is_empty(), "{id}: {k}");
            }
            assert!(CATEGORIES.contains(&e["category"].as_str().unwrap_or_default()), "{id}: category");
            // the dialog says what each one needs installed (ed_needs_<runtime>); a binary needs nothing
            assert!(["node", "uv", "python", "binary"].contains(&e["runtime"].as_str().unwrap_or_default()), "{id}: runtime");
            for lang in ["en", "tr"] {
                assert!(!e["description"][lang].as_str().unwrap_or_default().trim().is_empty(), "{id}: description.{lang}");
            }
            let repo = e["repository"].as_str().unwrap();
            // the links the dialog opens are https, and the bundles come from the publisher's GitHub repository
            assert!(repo.starts_with("https://github.com/") && e["homepage"].as_str().unwrap().starts_with("https://"), "{id}: links");
            let assets = e["assets"].as_array().unwrap_or_else(|| panic!("{id}: assets"));
            assert!(!assets.is_empty());
            for a in assets {
                let url = a["url"].as_str().unwrap_or_default();
                assert!(url.starts_with(&format!("{repo}/releases/download/")), "{id}: {url} is a release asset of {repo}");
                assert!(url.ends_with(".mcpb") || url.ends_with(".dxt"), "{id}: {url}");
                let sha = a["sha256"].as_str().unwrap_or_default();
                assert!(sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{id}: sha256");
                assert!(a["size"].as_u64().is_some_and(|s| s > 0 && s <= MAX_DOWNLOAD), "{id}: size");
                let p = a["platform"].as_str().unwrap_or_default();
                assert!(p == "any" || ["win32", "darwin", "linux"].iter().any(|o| ["x64", "arm64"].iter().any(|c| p == format!("{o}-{c}"))), "{id}: platform {p}");
            }
            assert!(newer(e["version"].as_str().unwrap(), "0").is_some(), "{id}: version is dotted numbers");
        }
    }

    #[test]
    fn every_category_has_entries_and_a_label_in_both_languages() {
        let list: Vec<Value> = serde_json::from_str(LIST).unwrap();
        let i18n = include_str!("../../ui/js/i18n.js");
        for c in CATEGORIES {
            assert!(list.iter().any(|e| e["category"] == c), "{c}: no entry");
            assert_eq!(i18n.matches(&format!("ed_cat_{c}:")).count(), 2, "{c}: ed_cat_{c} in English and Turkish");
        }
    }

    #[test]
    fn installed_entries_and_updates() {
        assert_eq!(newer("1.10.0", "1.9.2"), Some(true));
        assert_eq!(newer("v2.0", "2.0.0"), Some(false));
        assert_eq!(newer("1.0.0", "{dynamic}"), None);
        let config = json!({"mcpServers": {
            // from the directory, an older version
            "apify-mcp-server": {"command": "node", "extension": {"path": "/x", "version": "0.21.0", "source": {"id": "apify", "version": "0.21.0"}}},
            // from a file, found by its manifest name, current
            "desktop-commander": {"command": "node", "extension": {"path": "/y", "version": "0.3.4"}},
            // a plain server that happens to have an entry's name: not an extension
            "mcp-pinot": {"command": "uvx"},
        }});
        let list = entries(&config, "win32-x64");
        let by = |id: &str| list.iter().find(|e| e["id"] == id).unwrap().clone();
        assert_eq!(by("apify")["installed"], json!({"name": "apify-mcp-server", "version": "0.21.0"}));
        assert_eq!(by("apify")["update"], true);
        assert_eq!(by("desktop-commander")["installed"]["version"], "0.3.4");
        assert_eq!(by("desktop-commander")["update"], false);
        assert!(by("pinot")["installed"].is_null());
        // the asset for this platform, else the one for any
        assert!(by("kubernetes")["asset"]["url"].as_str().unwrap().ends_with("windows-x64.mcpb"));
        assert!(entries(&config, "darwin-arm64").iter().find(|e| e["id"] == "kubernetes").unwrap()["asset"]["url"].as_str().unwrap().ends_with("darwin-arm64.mcpb"));
        assert!(entries(&config, "freebsd-x64").iter().find(|e| e["id"] == "kubernetes").unwrap()["asset"].is_null());
        assert_eq!(by("apify")["asset"]["platform"], "any");
        assert!(by("apify").get("assets").is_none());
        assert_eq!(entries(&json!({"mcpServers": []}), "win32-x64").len(), list.len(), "a broken config marks nothing");
    }

    #[test]
    fn updates_are_the_installed_entries_with_a_newer_build_here() {
        let config = json!({"mcpServers": {
            "apify-mcp-server": {"command": "node", "extension": {"path": "/x", "version": "0.21.0", "source": {"id": "apify", "version": "0.21.0"}}},
            "desktop-commander": {"command": "node", "extension": {"path": "/y", "version": "999.0.0"}},
        }});
        let u = updates(&config, "win32-x64");
        assert_eq!(u.len(), 1, "{u:?}");
        assert_eq!((u[0]["id"].as_str(), u[0]["server"].as_str(), u[0]["from"].as_str()), (Some("apify"), Some("apify-mcp-server"), Some("0.21.0")));
        assert!(updates(&json!({}), "win32-x64").is_empty());
    }

    #[test]
    fn only_https_and_local_test_servers_are_downloaded() {
        assert!(allowed("https://github.com/a/b/releases/download/v1/x.mcpb"));
        assert!(!allowed("http://github.com/a/b/releases/download/v1/x.mcpb"));
        assert!(!allowed("file:///C:/x.mcpb"));
        assert_eq!(allowed("http://127.0.0.1:9/x.mcpb"), cfg!(debug_assertions));
        assert!(!allowed("http://10.0.0.1/x.mcpb"));
    }

    async fn serve(body: Vec<u8>, redirect_to: Option<String>) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let (body, redirect_to) = (body.clone(), redirect_to.clone());
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let head = match redirect_to {
                        Some(to) => format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                        None => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()),
                    };
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_download_must_have_the_listed_hash_size_and_host() {
        use sha2::Digest;
        let d = std::env::temp_dir().join(format!("ed-dl-{}", crate::util::new_id()));
        let bytes = include_bytes!("testdata/signed_ca.mcpb").to_vec();
        let sha = hex::encode(sha2::Sha256::digest(&bytes));
        let base = serve(bytes.clone(), None).await;
        let client = reqwest::Client::builder().no_proxy().redirect(redirects()).build().unwrap();
        let none = CancellationToken::new();
        let asset = json!({"url": format!("{base}/rel/demo-1.0.mcpb"), "size": bytes.len(), "sha256": sha});
        let path = download(&client, &asset, &d, &none, &|_, _| {}).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(path.file_name().unwrap(), "demo-1-0.mcpb");
        let info = crate::extensions::inspect(&path).unwrap();
        assert_eq!((info["sha256"].as_str(), info["signature"]["status"].as_str()), (Some(sha.as_str()), Some("untrusted")));
        // another hash: deleted
        let wrong = json!({"url": asset["url"], "size": bytes.len(), "sha256": "00".repeat(32)});
        assert_eq!(download(&client, &wrong, &d, &none, &|_, _| {}).await.unwrap_err().kind(), "ed_checksum");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        // larger than listed
        let small = json!({"url": asset["url"], "size": 10, "sha256": sha});
        assert_eq!(download(&client, &small, &d, &none, &|_, _| {}).await.unwrap_err().kind(), "download_too_large");
        // a redirect to another host isn't followed
        let away = serve(vec![], Some(format!("{}/x.mcpb", base.replace("127.0.0.1", "localhost")))).await;
        let moved = json!({"url": format!("{away}/x.mcpb"), "size": bytes.len(), "sha256": sha});
        assert_eq!(download(&client, &moved, &d, &none, &|_, _| {}).await.unwrap_err().kind(), "download_redirect");
        // not https
        let plain = json!({"url": "http://example.com/x.mcpb", "size": 1, "sha256": sha});
        assert_eq!(download(&client, &plain, &d, &none, &|_, _| {}).await.unwrap_err().kind(), "ed_not_https");
        let _ = std::fs::remove_dir_all(d);
    }
}
