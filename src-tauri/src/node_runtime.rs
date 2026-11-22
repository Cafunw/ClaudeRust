//! The Node.js runtime for desktop extensions (Claude Desktop ships one):
//! downloaded only when the user asks, a pinned LTS build from nodejs.org
//! whose SHA-256 is in this binary and must also be the one nodejs.org's
//! SHASUMS256.txt lists; only its `node` binary is unpacked, to
//! <data>/runtimes/node-<version>/. Extensions whose command is `node` run it;
//! other servers only when the user's PATH has no node (see `resolve`).

use crate::error::{AppError, AppResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub const VERSION: &str = "26.13.1";
const DIST: &str = "https://nodejs.org/dist";

pub struct Build {
    pub file: &'static str,
    pub sha256: &'static str,
    /// The download's size, rounded, for "about N MB".
    pub mb: u64,
}

/// (std::env::consts::OS, ARCH, build), from
/// https://nodejs.org/dist/v26.13.1/SHASUMS256.txt and the sizes listed beside it.
const BUILDS: [(&str, &str, Build); 6] = [
    ("windows", "x86_64", Build { file: "node-v26.13.1-win-x64.zip", sha256: "e2edda75bfb089654705cf65a13b58933bfd5f57148e8ebda916f58a2474f1f7", mb: 41 }),
    ("windows", "aarch64", Build { file: "node-v26.13.1-win-arm64.zip", sha256: "181ebe431b90787113a9e4fc80d38ce86d37888a353a0386de44d2872e48cb97", mb: 37 }),
    ("macos", "x86_64", Build { file: "node-v26.13.1-darwin-x64.tar.gz", sha256: "82bb96078a24ca50b9c9a38f2379e6be6d87171fcd400fc14c8934b3bce96e2f", mb: 60 }),
    ("macos", "aarch64", Build { file: "node-v26.13.1-darwin-arm64.tar.gz", sha256: "aec2514ea1ceb2ba52455e9ec65986f6749a06dd759e5db2a72e6889668c9735", mb: 58 }),
    ("linux", "x86_64", Build { file: "node-v26.13.1-linux-x64.tar.gz", sha256: "518674de2b621728a5e4486f5fb3b7d2900bb3cc4aa97969edaa463db99f9778", mb: 63 }),
    ("linux", "aarch64", Build { file: "node-v26.13.1-linux-arm64.tar.gz", sha256: "80a330b35415382878df18d41afbd106ea89bfb3be0087d6608e289a14c5cc7d", mb: 63 }),
];

/// An archive has a few thousand entries (npm comes with it) and node is under 200 MB.
const MAX_ENTRIES: usize = 50_000;
const MAX_BINARY: u64 = 400 * 1024 * 1024;

pub fn build_for(os: &str, arch: &str) -> Option<&'static Build> {
    BUILDS.iter().find(|(o, a, _)| *o == os && *a == arch).map(|(_, _, b)| b)
}

fn this_build() -> Option<&'static Build> {
    build_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The binary's path in the archive: `node-v…-win-x64/node.exe`, `node-v…-darwin-arm64/bin/node`.
fn member(b: &Build) -> String {
    let stem = b.file.trim_end_matches(".zip").trim_end_matches(".tar.gz");
    if b.file.ends_with(".zip") {
        format!("{stem}/node.exe")
    } else {
        format!("{stem}/bin/node")
    }
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "node.exe"
    } else {
        "node"
    }
}

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Where runtimes go: `<data_dir>/runtimes`. Called once at start.
pub fn init(data_dir: &Path) {
    let _ = ROOT.set(data_dir.join("runtimes"));
}

fn root() -> AppResult<&'static Path> {
    ROOT.get().map(PathBuf::as_path).ok_or_else(|| AppError::msg("The app's data folder isn't known yet."))
}

fn home(root: &Path) -> PathBuf {
    root.join(format!("node-{VERSION}"))
}

/// The downloaded node, when it is there.
pub fn installed() -> Option<PathBuf> {
    ROOT.get().map(|r| home(r).join(binary_name())).filter(|p| p.is_file())
}

fn on_path(program: &str) -> Option<PathBuf> {
    which::which_in(program, Some(crate::shellenv::path()), std::env::current_dir().unwrap_or_default()).ok()
}

/// The program to start for a server whose `command` is this: for `node`,
/// the downloaded one for an `extension` (a known version) and for others
/// when the PATH has none; for Python on Windows `py` or `python` before the
/// Microsoft Store's stand-in. A runtime that can't be found is a coded
/// error naming it (`needs_node` and so on).
pub fn resolve(command: &str, extension: bool) -> AppResult<String> {
    resolve_with(command, &on_path, installed(), extension, cfg!(windows))
}

fn store_stub(p: &Path) -> bool {
    p.to_string_lossy().to_ascii_lowercase().contains("\\windowsapps\\")
}

fn resolve_with(command: &str, find: &dyn Fn(&str) -> Option<PathBuf>, builtin: Option<PathBuf>, extension: bool, windows: bool) -> AppResult<String> {
    if command.contains(['/', '\\']) {
        return Ok(command.to_string());
    }
    let lower = command.to_ascii_lowercase();
    let found = |p: &str| find(p).is_some();
    match lower.strip_suffix(".exe").unwrap_or(&lower) {
        "node" if !(extension && builtin.is_some()) && found(command) => Ok(command.to_string()),
        "node" => builtin.map(|p| p.display().to_string()).ok_or_else(|| {
            AppError::coded("needs_node", Value::Null, "This server needs Node.js, which isn't installed. Download it in Settings → Developer, or install it from https://nodejs.org.")
        }),
        "python" | "python3" | "py" => {
            let order: &[&str] = if windows { &[command, "py", "python", "python3"] } else { &[command, "python3", "python"] };
            let hits: Vec<(&str, PathBuf)> = order.iter().filter_map(|c| find(c).map(|p| (*c, p))).collect();
            hits.iter()
                .find(|(_, p)| !store_stub(p))
                .or(hits.first())
                .map(|(c, _)| c.to_string())
                .ok_or_else(|| AppError::coded("needs_python", Value::Null, "This server needs Python 3, which isn't installed. Install it from https://www.python.org/downloads/, then restart the servers."))
        }
        "uv" | "uvx" if !found(command) => Err(AppError::coded("needs_uv", Value::Null, "This server needs uv, which isn't installed. Install it from https://docs.astral.sh/uv/, then restart the servers.")),
        "npx" | "npm" if !found(command) => Err(AppError::coded("needs_npx", Value::Null, "This server runs npx, which comes with Node.js. Install Node.js from https://nodejs.org, then restart the servers.")),
        _ => Ok(command.to_string()),
    }
}

/// The runtime a coded start failure (`resolve`) names: "node", "python"…
pub fn missing_runtime(kind: &str) -> Option<String> {
    // (not "needs_auth", a remote server's sign-in)
    kind.strip_prefix("needs_").filter(|r| ["node", "python", "uv", "npx"].contains(r)).map(str::to_string)
}

/// Copy `r` to `out`, refusing more than `max` bytes.
fn copy_capped(r: &mut impl Read, out: &Path, max: u64) -> AppResult<()> {
    let mut f = std::fs::File::create(out)?;
    let n = std::io::copy(&mut r.take(max + 1), &mut f)?;
    if n > max {
        drop(f);
        let _ = std::fs::remove_file(out);
        return Err(AppError::coded("node_bad_archive", Value::Null, "The Node.js archive holds an unexpectedly large node; nothing was installed."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Unpack only `member` (a regular file, never a link) from the zip or
/// tar.gz at `archive` to `out`: no other path is written, so entries that
/// climb out (`../`) or are absolute never land anywhere.
fn extract_binary(archive: &Path, member: &str, out: &Path, max_entries: usize, max_bytes: u64) -> AppResult<()> {
    let bad = |why: &str| AppError::coded("node_bad_archive", Value::Null, format!("The Node.js archive isn't as expected ({why}); nothing was installed."));
    let f = std::fs::File::open(archive)?;
    if archive.to_string_lossy().ends_with(".zip") {
        let mut z = zip::ZipArchive::new(f).map_err(|e| bad(&e.to_string()))?;
        if z.len() > max_entries {
            return Err(bad("too many entries"));
        }
        let mut e = z.by_name(member).map_err(|_| bad("no node in it"))?;
        if e.is_symlink() || !e.is_file() {
            return Err(bad("node isn't a regular file"));
        }
        return copy_capped(&mut e, out, max_bytes);
    }
    let mut a = tar::Archive::new(flate2::read::GzDecoder::new(f));
    for (i, e) in a.entries()?.enumerate() {
        if i >= max_entries {
            return Err(bad("too many entries"));
        }
        let mut e = e?;
        if &*e.path_bytes() == member.as_bytes() {
            if e.header().entry_type() != tar::EntryType::Regular {
                return Err(bad("node isn't a regular file"));
            }
            return copy_capped(&mut e, out, max_bytes);
        }
    }
    Err(bad("no node in it"))
}

/// GET `url` into `path`: at most `max` bytes, `progress(done, total)` as it
/// comes, stopped by `cancel`. Returns the size and the SHA-256 (hex).
pub(crate) async fn fetch_to(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    max: u64,
    cancel: &CancellationToken,
    progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
) -> AppResult<(u64, String)> {
    let too_large = || AppError::coded("download_too_large", Value::Null, format!("{url} is larger than expected; the download was stopped."));
    let mut resp = tokio::select! {
        r = client.get(url).send() => r?.error_for_status()?,
        _ = cancel.cancelled() => return Err(AppError::Cancelled),
    };
    if resp.status().is_redirection() {
        return Err(AppError::coded("download_redirect", Value::Null, format!("{url} redirects to an address the app doesn't download from.")));
    }
    let total = resp.content_length();
    if total.is_some_and(|t| t > max) {
        return Err(too_large());
    }
    let mut file = tokio::fs::File::create(path).await?;
    let (mut done, mut shown, mut hash) = (0u64, 0u64, Sha256::new());
    let r: AppResult<()> = async {
        loop {
            let chunk = tokio::select! {
                c = resp.chunk() => c?,
                _ = cancel.cancelled() => return Err(AppError::Cancelled),
            };
            let Some(chunk) = chunk else { break };
            done += chunk.len() as u64;
            if done > max {
                return Err(too_large());
            }
            hash.update(&chunk);
            file.write_all(&chunk).await?;
            // every 256 kB, not every chunk: each is an event to the UI
            if done - shown >= 256 * 1024 {
                progress(done, total);
                shown = done;
            }
        }
        file.flush().await?;
        progress(done, total);
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = r {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok((done, hex::encode(hash.finalize())))
}

/// Download build `b` from `base` (nodejs.org's dist folder) and put its
/// node in `root`/node-<version>/. Nothing is kept unless both checksums agree.
async fn download(client: &reqwest::Client, base: &str, b: &Build, root: &Path, cancel: &CancellationToken, progress: &(dyn Fn(u64, Option<u64>) + Send + Sync)) -> AppResult<PathBuf> {
    let sums = tokio::select! {
        r = async { client.get(format!("{base}/v{VERSION}/SHASUMS256.txt")).send().await?.error_for_status()?.text().await } => r?,
        _ = cancel.cancelled() => return Err(AppError::Cancelled),
    };
    let listed = sums.lines().find_map(|l| l.split_once(char::is_whitespace).filter(|(_, f)| f.trim() == b.file).map(|(h, _)| h.to_ascii_lowercase()));
    if listed.as_deref() != Some(b.sha256) {
        return Err(AppError::coded("node_checksum_list", Value::Null, format!("nodejs.org lists another checksum for {} than this app was built with; nothing was downloaded.", b.file)));
    }
    std::fs::create_dir_all(root)?;
    // its extension kept: extract_binary reads a .zip or a .tar.gz by it
    let part = root.join(format!(".part-{}", b.file));
    let (_, sha) = fetch_to(client, &format!("{base}/v{VERSION}/{}", b.file), &part, b.mb * 2 * 1024 * 1024, cancel, progress).await?;
    if sha != b.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(AppError::coded("node_checksum", Value::Null, format!("The downloaded {} doesn't have the checksum it should ({sha}); it was deleted and nothing was installed.", b.file)));
    }
    let (member, root2, part2) = (member(b), root.to_path_buf(), part.clone());
    let unpacked = crate::extensions::on_pool(move || -> AppResult<PathBuf> {
        let tmp = root2.join(format!(".node-{}", &crate::util::new_id()[..8]));
        std::fs::create_dir_all(&tmp)?;
        let r = extract_binary(&part2, &member, &tmp.join(binary_name()), MAX_ENTRIES, MAX_BINARY).and_then(|_| {
            let dest = home(&root2);
            if dest.exists() {
                std::fs::remove_dir_all(&dest)?;
            }
            std::fs::rename(&tmp, &dest)?;
            Ok(dest.join(binary_name()))
        });
        if r.is_err() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        r
    })
    .await;
    let _ = std::fs::remove_file(&part);
    unpacked
}

fn dist() -> String {
    // debug builds only: lets a test point the download at a local server
    #[cfg(debug_assertions)]
    if let Ok(u) = std::env::var("CLAUDERUST_NODE_DIST") {
        return u.trim_end_matches('/').to_string();
    }
    DIST.into()
}

static RUNNING: parking_lot::Mutex<Option<CancellationToken>> = parking_lot::Mutex::new(None);

async fn status() -> Value {
    crate::shellenv::ready().await;
    let b = this_build();
    json!({
        "version": VERSION,
        "supported": b.is_some(),
        "size_mb": b.map(|b| b.mb),
        "source": b.map(|b| format!("{DIST}/v{VERSION}/{}", b.file)),
        "sha256": b.map(|b| b.sha256),
        "installed": installed().map(|p| p.display().to_string()),
        "on_path": on_path("node").is_some(),
        "downloading": RUNNING.lock().is_some(),
    })
}

#[tauri::command]
pub async fn node_runtime_status() -> AppResult<Value> {
    Ok(status().await)
}

/// Download and unpack Node.js; "node-runtime-progress" events carry {downloaded, total}.
#[tauri::command]
pub async fn node_runtime_install(app: tauri::AppHandle) -> AppResult<Value> {
    use tauri::Emitter;
    let b = this_build().ok_or_else(|| AppError::coded("node_unsupported", Value::Null, "nodejs.org has no Node.js build for this computer's system."))?;
    let root = root()?;
    let cancel = {
        let mut r = RUNNING.lock();
        if r.is_some() {
            return Err(AppError::coded("node_busy", Value::Null, "Node.js is already being downloaded."));
        }
        r.insert(CancellationToken::new()).clone()
    };
    let base = dist();
    let res = async {
        let client = crate::mcp_oauth::download_client(crate::mcp_oauth::same_origin_redirects(), base == DIST)?;
        download(&client, &base, b, root, &cancel, &|done, total| {
            let _ = app.emit("node-runtime-progress", json!({"downloaded": done, "total": total}));
        })
        .await
    }
    .await;
    *RUNNING.lock() = None;
    res?;
    Ok(status().await)
}

#[tauri::command]
pub async fn node_runtime_cancel() -> AppResult<()> {
    if let Some(t) = RUNNING.lock().as_ref() {
        t.cancel();
    }
    Ok(())
}

/// Delete the downloaded Node.js; the servers are stopped first (a running
/// node can't be deleted on Windows) and started again after.
#[tauri::command]
pub async fn node_runtime_remove(app: tauri::AppHandle, state: tauri::State<'_, std::sync::Arc<crate::state::AppState>>) -> AppResult<Value> {
    if RUNNING.lock().is_some() {
        return Err(AppError::coded("node_busy", Value::Null, "Node.js is already being downloaded."));
    }
    let dir = home(root()?);
    state.mcp.stop_all().await;
    let removed = crate::extensions::on_pool(move || {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    })
    .await;
    crate::commands::mcp_reload(app, state).await?;
    removed?;
    Ok(status().await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("node-rt-{tag}-{}", crate::util::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn every_supported_system_has_a_build_and_others_none() {
        for (os, arch) in [("windows", "x86_64"), ("windows", "aarch64"), ("macos", "x86_64"), ("macos", "aarch64"), ("linux", "x86_64"), ("linux", "aarch64")] {
            let b = build_for(os, arch).unwrap_or_else(|| panic!("{os} {arch}"));
            assert!(b.file.contains(VERSION) && b.sha256.len() == 64 && b.sha256.chars().all(|c| c.is_ascii_hexdigit()), "{}", b.file);
            assert!(b.file.ends_with(if os == "windows" { ".zip" } else { ".tar.gz" }));
        }
        assert!(build_for("freebsd", "x86_64").is_none() && build_for("windows", "x86").is_none());
        assert_eq!(member(build_for("windows", "x86_64").unwrap()), "node-v26.13.1-win-x64/node.exe");
        assert_eq!(member(build_for("macos", "aarch64").unwrap()), "node-v26.13.1-darwin-arm64/bin/node");
        let hashes: std::collections::HashSet<_> = BUILDS.iter().map(|(_, _, b)| b.sha256).collect();
        assert_eq!(hashes.len(), BUILDS.len());
    }

    #[test]
    fn choosing_the_runtime() {
        let builtin = Some(PathBuf::from("/data/runtimes/node-26/node"));
        let path = |names: &'static [(&'static str, &'static str)]| move |p: &str| names.iter().find(|(n, _)| *n == p).map(|(_, at)| PathBuf::from(at));
        // another server: the user's own node first, the downloaded one only without it
        assert_eq!(resolve_with("node", &path(&[("node", "/usr/bin/node")]), builtin.clone(), false, false).unwrap(), "node");
        assert_eq!(resolve_with("node", &path(&[]), builtin.clone(), false, false).unwrap(), "/data/runtimes/node-26/node");
        assert_eq!(resolve_with("node.exe", &path(&[]), builtin.clone(), false, true).unwrap(), "/data/runtimes/node-26/node");
        assert_eq!(resolve_with("node", &path(&[]), None, false, false).unwrap_err().kind(), "needs_node");
        // an extension runs the downloaded one even beside the user's, and the user's when there's none downloaded
        assert_eq!(resolve_with("node", &path(&[("node", "/usr/bin/node")]), builtin.clone(), true, false).unwrap(), "/data/runtimes/node-26/node");
        assert_eq!(resolve_with("node", &path(&[("node", "/usr/bin/node")]), None, true, false).unwrap(), "node");
        assert_eq!(resolve_with("node", &path(&[]), None, true, false).unwrap_err().kind(), "needs_node");
        // Windows: py or python before the Store stand-in for python3
        const STUB: &str = r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\python3.exe";
        assert_eq!(resolve_with("python3", &path(&[("python3", STUB), ("py", r"C:\Windows\py.exe")]), None, false, true).unwrap(), "py");
        assert_eq!(resolve_with("python3", &path(&[("python3", STUB), ("python", r"C:\Python314\python.exe")]), None, false, true).unwrap(), "python");
        assert_eq!(resolve_with("python3", &path(&[("python3", STUB)]), None, false, true).unwrap(), "python3", "a Store Python may be real");
        assert_eq!(resolve_with("python", &path(&[]), None, false, true).unwrap_err().kind(), "needs_python");
        assert_eq!(resolve_with("python", &path(&[("python3", "/usr/bin/python3")]), None, false, false).unwrap(), "python3");
        assert_eq!(resolve_with("uv", &path(&[]), None, false, false).unwrap_err().kind(), "needs_uv");
        assert_eq!(resolve_with("npx", &path(&[]), builtin.clone(), false, false).unwrap_err().kind(), "needs_npx");
        // other commands and paths are left alone
        assert_eq!(resolve_with("docker", &path(&[]), None, false, false).unwrap(), "docker");
        assert_eq!(resolve_with("/opt/node/bin/node", &path(&[]), None, false, false).unwrap(), "/opt/node/bin/node");
        assert_eq!(missing_runtime("needs_node").as_deref(), Some("node"));
        assert_eq!(missing_runtime("network"), None);
        assert_eq!(missing_runtime("needs_auth"), None);
    }

    fn zip_with(path: &Path, entries: &[(&str, &[u8], bool)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data, link) in entries {
            let o = zip::write::SimpleFileOptions::default();
            if *link {
                z.add_symlink(*name, std::str::from_utf8(data).unwrap(), o).unwrap();
            } else {
                z.start_file(*name, o).unwrap();
                z.write_all(data).unwrap();
            }
        }
        z.finish().unwrap();
    }

    #[test]
    fn only_the_node_binary_is_unpacked_from_a_hostile_zip() {
        let d = temp("zip");
        let m = "node-v26.13.1-win-x64/node.exe";
        let out = d.join("out").join("node.exe");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        let hostile = d.join("hostile.zip");
        zip_with(&hostile, &[("../escape.txt", b"x", false), ("/abs.txt", b"x", false), ("node-v26.13.1-win-x64/../../up.exe", b"x", false), (m, b"MZ node", false)]);
        extract_binary(&hostile, m, &out, 100, 1024).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"MZ node");
        assert!(!d.join("escape.txt").exists() && !d.parent().unwrap().join("up.exe").exists());
        assert_eq!(std::fs::read_dir(out.parent().unwrap()).unwrap().count(), 1, "nothing else was written");
        // a link where node should be, a missing node, too many entries, a node too large
        let link = d.join("link.zip");
        zip_with(&link, &[(m, b"C:/Windows/System32/cmd.exe", true)]);
        assert_eq!(extract_binary(&link, m, &out, 100, 1024).unwrap_err().kind(), "node_bad_archive");
        assert!(extract_binary(&hostile, "other/node.exe", &out, 100, 1024).is_err());
        assert!(extract_binary(&hostile, m, &out, 3, 1024).unwrap_err().to_string().contains("too many"));
        assert!(extract_binary(&hostile, m, &out, 100, 4).unwrap_err().to_string().contains("large"));
        assert!(!out.exists(), "a node over the cap is removed");
        std::fs::write(d.join("broken.zip"), b"PK\x03\x04 not really").unwrap();
        assert!(extract_binary(&d.join("broken.zip"), m, &out, 100, 1024).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn only_a_regular_node_is_unpacked_from_a_tarball() {
        let d = temp("tar");
        let m = "node-v26.13.1-linux-x64/bin/node";
        let tgz = |name: &str, entries: &[(&str, tar::EntryType, &[u8])]| {
            let p = d.join(name);
            let gz = flate2::write::GzEncoder::new(std::fs::File::create(&p).unwrap(), flate2::Compression::fast());
            let mut t = tar::Builder::new(gz);
            for (path, kind, data) in entries {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(*kind);
                h.set_size(if *kind == tar::EntryType::Regular { data.len() as u64 } else { 0 });
                h.set_mode(0o755);
                if *kind == tar::EntryType::Symlink {
                    h.set_link_name(std::str::from_utf8(data).unwrap()).unwrap();
                    t.append_data(&mut h, path, std::io::empty()).unwrap();
                } else {
                    t.append_data(&mut h, path, *data).unwrap();
                }
            }
            t.into_inner().unwrap().finish().unwrap();
            p
        };
        let out = d.join("node");
        let long = format!("node-v26.13.1-linux-x64/lib/node_modules/{}/x.js", "deep/".repeat(40));
        let good = tgz("good.tar.gz", &[(&long, tar::EntryType::Regular, b"js"), ("node-v26.13.1-linux-x64/bin/npm", tar::EntryType::Symlink, b"../lib/npm-cli.js"), (m, tar::EntryType::Regular, b"\x7fELF node")]);
        extract_binary(&good, m, &out, 100, 1024).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"\x7fELF node");
        let link = tgz("link.tar.gz", &[(m, tar::EntryType::Symlink, b"/bin/sh")]);
        assert_eq!(extract_binary(&link, m, &out, 100, 1024).unwrap_err().kind(), "node_bad_archive");
        assert!(extract_binary(&good, m, &out, 2, 1024).unwrap_err().to_string().contains("too many"));
        assert!(extract_binary(&good, m, &out, 100, 3).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    /// A server for `routes` (path, status, body); every request answered once per connection.
    async fn serve(routes: Vec<(String, u16, Vec<u8>)>) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { break };
                let routes = routes.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                    let (code, body) = routes.iter().find(|(p, _, _)| *p == path).map(|(_, c, b)| (*c, b.clone())).unwrap_or((404, b"no".to_vec()));
                    let head = format!("HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[tokio::test]
    async fn the_download_is_checked_against_both_checksums() {
        let d = temp("dl");
        let zip = d.join("src.zip");
        zip_with(&zip, &[("node-v26.13.1-win-x64/node.exe", b"MZ fake node", false)]);
        let bytes = std::fs::read(&zip).unwrap();
        let sha: &'static str = Box::leak(hex::encode(Sha256::digest(&bytes)).into_boxed_str());
        let b = Build { file: "node-v26.13.1-win-x64.zip", sha256: sha, mb: 1 };
        let sums = format!("0000  other.zip\n{sha}  node-v26.13.1-win-x64.zip\n");
        let base = serve(vec![(format!("/v{VERSION}/SHASUMS256.txt"), 200, sums.into_bytes()), (format!("/v{VERSION}/{}", b.file), 200, bytes.clone())]).await;
        let seen = std::sync::Mutex::new(0u64);
        let root = d.join("runtimes");
        let node = download(&client(), &base, &b, &root, &CancellationToken::new(), &|done, _| *seen.lock().unwrap() = done).await.unwrap();
        assert_eq!(node, home(&root).join(binary_name()));
        assert_eq!(std::fs::read(&node).unwrap(), b"MZ fake node");
        assert_eq!(*seen.lock().unwrap(), bytes.len() as u64, "progress reached the size");
        assert!(!root.join(format!(".part-{}", b.file)).exists());

        // nodejs.org listing another checksum: nothing is downloaded
        let other = Build { file: b.file, sha256: "ab".repeat(32).leak(), mb: 1 };
        let e = download(&client(), &base, &other, &d.join("r2"), &CancellationToken::new(), &|_, _| {}).await.unwrap_err();
        assert_eq!(e.kind(), "node_checksum_list");
        assert!(!d.join("r2").exists());
        // the list agrees but the file served is another one
        let base2 = serve(vec![(format!("/v{VERSION}/SHASUMS256.txt"), 200, format!("{sha}  {}\n", b.file).into_bytes()), (format!("/v{VERSION}/{}", b.file), 200, b"tampered".to_vec())]).await;
        let e = download(&client(), &base2, &b, &d.join("r3"), &CancellationToken::new(), &|_, _| {}).await.unwrap_err();
        assert_eq!(e.kind(), "node_checksum");
        assert!(!home(&d.join("r3")).exists() && std::fs::read_dir(d.join("r3")).unwrap().count() == 0, "the bad download is deleted");
        // offline, and cancelled
        assert_eq!(download(&client(), "http://127.0.0.1:9", &b, &d.join("r4"), &CancellationToken::new(), &|_, _| {}).await.unwrap_err().kind(), "network");
        let c = CancellationToken::new();
        c.cancel();
        assert_eq!(download(&client(), &base, &b, &d.join("r5"), &c, &|_, _| {}).await.unwrap_err().kind(), "cancelled");
        let _ = std::fs::remove_dir_all(d);
    }
}
