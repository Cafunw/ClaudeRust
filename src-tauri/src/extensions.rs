//! Desktop Extensions: install `.mcpb` / `.dxt` bundles (a zip with a
//! `manifest.json` describing an MCP server) into the app's data folder and
//! register them in `claude_desktop_config.json`.

use crate::error::{AppError, AppResult};
use crate::mcp;
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};

/// manifest.json is small; a larger one is refused rather than inflated.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// The most an extension may unpack to (a small zip can expand to fill the disk).
const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn not_a_bundle(e: impl std::fmt::Display) -> AppError {
    AppError::coded("ext_not_zip", Value::Null, format!("Not a valid extension file (not a zip archive, or a damaged one): {e}"))
}

fn manifest_too_large() -> AppError {
    AppError::coded("ext_manifest_large", Value::Null, "The extension's manifest.json is too large (over 1 MB).")
}

fn read_manifest(zip_path: &Path) -> AppResult<Value> {
    let file = std::fs::File::open(zip_path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(not_a_bundle)?;
    let entry = zip.by_name("manifest.json").map_err(|_| AppError::coded("ext_no_manifest", Value::Null, "The extension has no manifest.json."))?;
    let declared = entry.size();
    let bytes = crate::util::read_capped(entry, declared, MAX_MANIFEST_BYTES).map_err(not_a_bundle)?.ok_or_else(manifest_too_large)?;
    parse_manifest(&bytes)
}

fn parse_manifest(bytes: &[u8]) -> AppResult<Value> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| AppError::coded("ext_manifest_json", Value::Null, format!("Invalid manifest.json: {e}")))?;
    if !v["server"].is_object() || v["name"].as_str().unwrap_or("").is_empty() {
        return Err(AppError::coded("ext_manifest_fields", Value::Null, "manifest.json must define \"name\" and \"server\"."));
    }
    Ok(v)
}

pub(crate) fn sanitize_name(s: &str) -> String {
    let n: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let n = n.trim_matches('-').to_string();
    if n.is_empty() { "extension".into() } else { n }
}

/// Manifest summary for the install dialog: what it is, who signed it, what
/// it runs (and whether that runtime is here), and its settings.
pub fn inspect(zip_path: &Path) -> AppResult<Value> {
    let m = read_manifest(zip_path)?;
    let (signature, sha256) = crate::extension_sign::check_file(zip_path)?;
    let cfg = server_config(&m);
    let command = cfg["command"].as_str().unwrap_or("");
    let args = cfg["args"].as_array().into_iter().flatten().filter_map(Value::as_str);
    let platforms: Vec<&str> = m["compatibility"]["platforms"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    Ok(json!({
        "name": m["name"],
        "display_name": m["display_name"].as_str().or(m["name"].as_str()),
        "version": m["version"],
        "description": m["description"],
        "author": m["author"]["name"],
        "server_type": m["server"]["type"],
        "user_config": m["user_config"],
        "tools": m["tools"],
        "run": std::iter::once(command).chain(args).collect::<Vec<_>>().join(" "),
        "env": cfg["env"].as_object().map(|e| e.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
        "runtime_missing": crate::node_runtime::resolve(command, true).err().and_then(|e| crate::node_runtime::missing_runtime(e.kind())),
        "compatible": platforms.is_empty() || platforms.contains(&platform_key()),
        "signature": signature.to_json(),
        "sha256": sha256,
        "size": std::fs::metadata(zip_path)?.len(),
    }))
}

/// The server's `mcp_config` (derived from its type and entry point when
/// missing), this platform's overrides applied; `${…}` not filled in yet.
fn server_config(manifest: &Value) -> Value {
    let mut cfg = manifest["server"]["mcp_config"].clone();
    if !cfg.is_object() {
        let entry = manifest["server"]["entry_point"].as_str().unwrap_or("");
        let path = format!("${{__dirname}}/{entry}");
        cfg = match manifest["server"]["type"].as_str().unwrap_or("") {
            "node" => json!({"command": "node", "args": [path]}),
            "python" => json!({"command": "python3", "args": [path]}),
            _ => json!({"command": path, "args": []}),
        };
    }
    if let Some(ov) = cfg["platform_overrides"][platform_key()].as_object().cloned() {
        for (k, v) in ov {
            cfg[k] = v;
        }
    }
    cfg
}

fn platform_key() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// Values available for `${...}` substitution.
fn base_vars(dir: &Path) -> Map<String, Value> {
    let mut m = Map::new();
    let s = |p: Option<PathBuf>| json!(p.map(|p| p.display().to_string()).unwrap_or_default());
    m.insert("__dirname".into(), json!(dir.display().to_string()));
    m.insert("HOME".into(), s(dirs::home_dir()));
    m.insert("DESKTOP".into(), s(dirs::desktop_dir()));
    m.insert("DOCUMENTS".into(), s(dirs::document_dir()));
    m.insert("DOWNLOADS".into(), s(dirs::download_dir()));
    m.insert("pathSeparator".into(), json!(std::path::MAIN_SEPARATOR.to_string()));
    m.insert("/".into(), json!(std::path::MAIN_SEPARATOR.to_string()));
    m
}

fn lookup(key: &str, vars: &Map<String, Value>, user: &Map<String, Value>) -> Option<Value> {
    if let Some(k) = key.strip_prefix("user_config.") {
        user.get(k).cloned()
    } else {
        vars.get(key).cloned()
    }
}

/// Substitute `${var}` / `${user_config.key}` in one string. Returns several
/// strings when the whole value is a multi-valued user setting.
fn substitute(s: &str, vars: &Map<String, Value>, user: &Map<String, Value>) -> Vec<String> {
    // whole-value multi expansion: each item is substituted once more, but
    // never expanded again (an item that is itself `${user_config.items}`
    // would otherwise recurse until the stack overflows)
    if let Some(inner) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
        if let Some(Value::Array(items)) = lookup(inner, vars, user) {
            return items
                .iter()
                .map(|v| v.as_str().map(|x| x.to_string()).unwrap_or_else(|| v.to_string()))
                .map(|x| substitute_inline(&x, vars, user))
                .collect();
        }
    }
    vec![substitute_inline(s, vars, user)]
}

/// Substitute the `${...}` references inside `s` (a list joined with commas).
fn substitute_inline(s: &str, vars: &Map<String, Value>, user: &Map<String, Value>) -> String {
    let lookup = |key: &str| lookup(key, vars, user);
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find('}') {
            Some(j) => {
                let key = &rest[i + 2..i + 2 + j];
                match lookup(key) {
                    Some(Value::String(v)) => out.push_str(&v),
                    Some(Value::Array(a)) => out.push_str(
                        &a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(","),
                    ),
                    Some(Value::Null) | None => {}
                    Some(v) => out.push_str(&v.to_string()),
                }
                rest = &rest[i + 3 + j..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn unzip(zip_path: &Path, dest: &Path) -> AppResult<()> {
    unzip_limited(zip_path, dest, MAX_UNPACKED_BYTES)
}

fn unzip_limited(zip_path: &Path, dest: &Path, limit: u64) -> AppResult<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(not_a_bundle)?;
    let too_large = || AppError::coded("ext_too_large", Value::Null, format!("The extension unpacks to more than {}; it was not installed.", crate::util::human_size(limit)));
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(not_a_bundle)?;
        // enclosed_name rejects absolute paths and `..` (zip-slip)
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        if total.saturating_add(entry.size()) > limit {
            return Err(too_large());
        }
        let mut f = std::fs::File::create(&out)?;
        // the header's size can lie: count what is really written
        let room = limit - total;
        let n = std::io::copy(&mut (&mut entry).take(room + 1), &mut f)?;
        if n > room {
            return Err(too_large());
        }
        total += n;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755 | 0o600));
        }
    }
    Ok(())
}

/// Install an extension and add it to the MCP config. Returns the server name.
/// An installed version is replaced only once the new one has been unpacked
/// and its settings checked; until then it's left untouched.
pub fn install(zip_path: &Path, extensions_dir: &Path, config_path: &Path, user_values: &Value) -> AppResult<String> {
    install_from(zip_path, extensions_dir, config_path, user_values, None)
}

/// `install`, noting where the bundle came from (`source`: the extension
/// directory's {id, version, sha256}) in its config entry. A bundle whose
/// signature doesn't match it is refused.
pub fn install_from(zip_path: &Path, extensions_dir: &Path, config_path: &Path, user_values: &Value, source: Option<&Value>) -> AppResult<String> {
    let manifest = read_manifest(zip_path)?;
    if crate::extension_sign::check_file(zip_path)?.0.status == "invalid" {
        return Err(AppError::coded("ext_signature_invalid", Value::Null, "The extension's signature doesn't match the file: it was changed after it was signed. It was not installed."));
    }
    let name = sanitize_name(manifest["name"].as_str().unwrap_or("extension"));
    let tmp = staging_dir(extensions_dir, &name)?;
    let checked = unzip(zip_path, &tmp).and_then(|_| dry_run(&tmp, &name, &manifest, extensions_dir, user_values));
    if let Err(e) = checked {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    swap_in(&tmp, &extensions_dir.join(&name), |dir| register(dir, &name, &manifest, config_path, user_values, true, source))
}

/// Install an extension that is already unpacked in a folder (for example one
/// installed by the Claude desktop app): copy it and register it.
pub fn install_from_dir(src: &Path, extensions_dir: &Path, config_path: &Path, user_values: &Value) -> AppResult<String> {
    let mf = src.join("manifest.json");
    let size = std::fs::metadata(&mf).map_err(|_| AppError::coded("ext_no_manifest", Value::Null, "The extension has no manifest.json."))?.len();
    if size > MAX_MANIFEST_BYTES {
        return Err(manifest_too_large());
    }
    let manifest = parse_manifest(&std::fs::read(&mf)?)?;
    let name = sanitize_name(manifest["name"].as_str().unwrap_or("extension"));
    let tmp = staging_dir(extensions_dir, &name)?;
    // Validate the user config before replacing an existing install.
    let checked = copy_dir(src, &tmp).and_then(|_| dry_run(&tmp, &name, &manifest, extensions_dir, user_values));
    if let Err(e) = checked {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    swap_in(&tmp, &extensions_dir.join(&name), |dir| register(dir, &name, &manifest, config_path, user_values, true, None))
}

/// A fresh, empty folder next to the install to unpack into; leftovers of
/// earlier attempts are cleared first.
fn staging_dir(extensions_dir: &Path, name: &str) -> AppResult<PathBuf> {
    std::fs::create_dir_all(extensions_dir)?;
    if let Ok(rd) = std::fs::read_dir(extensions_dir) {
        let mut prefixes = vec![format!(".{name}.installing")];
        // a moved-aside version only goes once a version is installed again
        if extensions_dir.join(name).is_dir() {
            prefixes.push(format!(".{name}.old-"));
        }
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if prefixes.iter().any(|p| n.starts_with(p.as_str())) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let tmp = extensions_dir.join(format!(".{name}.installing-{}", &crate::util::new_id()[..8]));
    std::fs::create_dir_all(&tmp)?;
    Ok(tmp)
}

/// Check that `dir` would register (required settings given, a command to
/// run), writing to a throwaway config rather than the real one.
fn dry_run(dir: &Path, name: &str, manifest: &Value, extensions_dir: &Path, user_values: &Value) -> AppResult<()> {
    let scratch = extensions_dir.join(format!(".dry-run-{}.json", &crate::util::new_id()[..8]));
    let r = register(dir, name, manifest, &scratch, user_values, false, None);
    let _ = std::fs::remove_file(&scratch);
    r.map(|_| ())
}

/// Put the unpacked, checked `new` folder in place of `dir`. A version
/// already there is moved aside first and deleted only once `finish`
/// (registering the server) has succeeded; on any failure it's put back.
fn swap_in(new: &Path, dir: &Path, finish: impl FnOnce(&Path) -> AppResult<String>) -> AppResult<String> {
    let aside = |tag: &str| {
        let n = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        dir.with_file_name(format!(".{n}.old-{tag}{}", &crate::util::new_id()[..8]))
    };
    let old = if std::fs::symlink_metadata(dir).is_ok() {
        let a = aside("");
        if let Err(e) = std::fs::rename(dir, &a) {
            let _ = std::fs::remove_dir_all(new);
            return Err(AppError::msg(format!(
                "The installed version in {} couldn't be replaced ({e}); it was left as it is. If its server is running, stop it and try again.",
                dir.display()
            )));
        }
        Some(a)
    } else {
        None
    };
    let put_back = |old: &Option<PathBuf>| {
        if let Some(a) = old {
            if let Err(e) = std::fs::rename(a, dir) {
                log::warn!("could not restore the earlier install from {}: {e}", a.display());
            }
        }
    };
    if let Err(e) = std::fs::rename(new, dir) {
        put_back(&old);
        let _ = std::fs::remove_dir_all(new);
        return Err(e.into());
    }
    match finish(dir) {
        Ok(v) => {
            if let Some(a) = old {
                if let Err(e) = std::fs::remove_dir_all(&a) {
                    log::warn!("could not remove the earlier install at {}: {e}", a.display());
                }
            }
            Ok(v)
        }
        Err(e) => {
            if old.is_some() {
                let failed = aside("failed-");
                if std::fs::rename(dir, &failed).is_ok() {
                    put_back(&old);
                    let _ = std::fs::remove_dir_all(&failed);
                }
            } else {
                let _ = std::fs::remove_dir_all(dir);
            }
            Err(e)
        }
    }
}

fn copy_dir(src: &Path, dst: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else if ft.is_file() {
            std::fs::copy(entry.path(), &to)?;
        } else if ft.is_symlink() {
            // Keep links that stay inside the bundle (node_modules/.bin etc.).
            #[cfg(unix)]
            if let Ok(target) = std::fs::read_link(entry.path()) {
                if target.is_relative() && !target.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                    let _ = std::os::unix::fs::symlink(target, &to);
                }
            }
        }
    }
    Ok(())
}

/// `keep`: put secret settings (`sensitive`) in the credential store, the
/// config holding references (a dry run keeps nothing). `source`: see `install_from`.
fn register(dir: &Path, name: &str, manifest: &Value, config_path: &Path, user_values: &Value, keep: bool, source: Option<&Value>) -> AppResult<String> {
    let name = name.to_string();
    // user config: provided values, else defaults; check required ones
    let mut user = Map::new();
    let vars = base_vars(dir);
    if let Some(spec) = manifest["user_config"].as_object() {
        for (k, def) in spec {
            let provided = user_values.get(k).filter(|v| !v.is_null() && v.as_str() != Some(""));
            let val = match provided {
                Some(v) => v.clone(),
                None => def["default"].clone(),
            };
            if val.is_null() && def["required"].as_bool() == Some(true) {
                return Err(AppError::msg(format!(
                    "“{}” is required.",
                    def["title"].as_str().unwrap_or(k)
                )));
            }
            // defaults may themselves contain ${HOME} etc.
            let val = match val {
                Value::String(s) => json!(substitute(&s, &vars, &Map::new()).join("")),
                Value::Array(a) => Value::Array(
                    a.into_iter()
                        .map(|x| match x {
                            Value::String(s) => json!(substitute(&s, &vars, &Map::new()).join("")),
                            o => o,
                        })
                        .collect(),
                ),
                o => o,
            };
            let secret = (keep && def["sensitive"].as_bool() == Some(true)).then(|| val.as_str().and_then(mcp::keep_secret)).flatten();
            user.insert(k.clone(), secret.map_or(val, Value::from));
        }
    }

    let cfg = server_config(manifest);
    let command = substitute(cfg["command"].as_str().unwrap_or(""), &vars, &user).join(" ");
    if command.trim().is_empty() {
        return Err(AppError::coded("ext_no_command", Value::Null, "The extension does not define a command to run."));
    }
    let args: Vec<String> = cfg["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .flat_map(|x| substitute(x, &vars, &user))
                .collect()
        })
        .unwrap_or_default();
    let mut env = Map::new();
    if let Some(e) = cfg["env"].as_object() {
        for (k, v) in e {
            if let Some(s) = v.as_str() {
                env.insert(k.clone(), json!(substitute(s, &vars, &user).join("")));
            }
        }
    }
    let mut entry = json!({"command": command, "args": args});
    if !env.is_empty() {
        entry["env"] = Value::Object(env);
    }
    entry["extension"] = json!({
        "path": dir.display().to_string(),
        "version": manifest["version"],
        "display_name": manifest["display_name"].as_str().or(manifest["name"].as_str()),
        // for changing them later (reconfigure)
        "user_config": user,
    });
    if let Some(s) = source.filter(|s| s.is_object()) {
        entry["extension"]["source"] = s.clone();
    }

    let mut config = mcp::read_config_for_update(config_path)?;
    config["mcpServers"][&name] = entry;
    mcp::write_config(config_path, &config)?;
    Ok(name)
}

/// The folder of the installed extension `name` in `config`, and its
/// manifest. Only a folder right inside `extensions_dir` (links resolved):
/// the config can be edited by hand, and this folder gets deleted.
fn installed(name: &str, extensions_dir: &Path, config: &Value) -> AppResult<(PathBuf, Value)> {
    let path = config["mcpServers"][name]["extension"]["path"].as_str().ok_or_else(|| AppError::msg(format!("“{name}” isn't an installed extension.")))?;
    let dir = std::fs::canonicalize(path).map_err(|e| AppError::msg(format!("The folder of “{name}” can't be found ({e}): {path}")))?;
    if dir.parent() != Some(std::fs::canonicalize(extensions_dir)?.as_path()) {
        return Err(AppError::msg(format!("{} isn't in the extensions folder, so it is left alone.", dir.display())));
    }
    let mf = dir.join("manifest.json");
    if std::fs::metadata(&mf)?.len() > MAX_MANIFEST_BYTES {
        return Err(manifest_too_large());
    }
    Ok((dir, parse_manifest(&std::fs::read(&mf)?)?))
}

fn is_secret(spec: &Value) -> bool {
    spec["sensitive"].as_bool() == Some(true)
}

/// What Settings shows to change extension `name`: the manifest's settings,
/// the values given (a secret one only as `secrets_set`, never its value).
pub fn settings(name: &str, extensions_dir: &Path, config_path: &Path) -> AppResult<Value> {
    let config = mcp::read_config(config_path);
    let (_, manifest) = installed(name, extensions_dir, &config)?;
    let spec = manifest["user_config"].as_object().cloned().unwrap_or_default();
    let current = &config["mcpServers"][name]["extension"]["user_config"];
    let (mut values, mut secrets_set) = (Map::new(), vec![]);
    for (k, def) in &spec {
        let v = &current[k];
        if v.is_null() || v.as_str() == Some("") {
            continue;
        }
        if is_secret(def) {
            secrets_set.push(k.clone());
        } else {
            values.insert(k.clone(), v.clone());
        }
    }
    Ok(json!({
        "name": name,
        "display_name": manifest["display_name"].as_str().or(manifest["name"].as_str()),
        "user_config": spec,
        "values": values,
        "secrets_set": secrets_set,
    }))
}

/// Register extension `name` again with `values`; a secret setting left
/// empty keeps the one it has.
pub fn reconfigure(name: &str, extensions_dir: &Path, config_path: &Path, values: &Value) -> AppResult<()> {
    let config = mcp::read_config_for_update(config_path)?;
    let (_, manifest) = installed(name, extensions_dir, &config)?;
    // registered at the path it has, not the canonical one (on Windows a \\?\ path, which the args' "/" breaks)
    let dir = PathBuf::from(config["mcpServers"][name]["extension"]["path"].as_str().unwrap_or_default());
    let current = &config["mcpServers"][name]["extension"]["user_config"];
    let mut merged = Map::new();
    for (k, def) in manifest["user_config"].as_object().into_iter().flatten() {
        let given = values.get(k).filter(|v| !v.is_null() && v.as_str() != Some(""));
        if let Some(v) = given.or_else(|| Some(&current[k]).filter(|v| is_secret(def) && !v.is_null())) {
            merged.insert(k.clone(), v.clone());
        }
    }
    let source = config["mcpServers"][name]["extension"]["source"].clone();
    register(&dir, name, &manifest, config_path, &Value::Object(merged), true, Some(&source)).map(|_| ())
}

/// Take extension `name` out of the config (its secrets go with it); the
/// folder to delete once its server has stopped.
pub fn unregister(name: &str, extensions_dir: &Path, config_path: &Path) -> AppResult<PathBuf> {
    let mut config = mcp::read_config_for_update(config_path)?;
    let (dir, _) = installed(name, extensions_dir, &config)?;
    if let Some(m) = config["mcpServers"].as_object_mut() {
        m.remove(name);
    }
    mcp::write_config(config_path, &config)?;
    Ok(dir)
}

type St<'a> = tauri::State<'a, std::sync::Arc<crate::state::AppState>>;

pub(crate) async fn on_pool<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::msg(e.to_string()))?
}

#[tauri::command]
pub async fn extension_settings(state: St<'_>, name: String) -> AppResult<Value> {
    let (dir, config) = (state.data_dir.join("extensions"), state.mcp.config_path.clone());
    on_pool(move || settings(&name, &dir, &config)).await
}

#[tauri::command]
pub async fn extension_reconfigure(app: tauri::AppHandle, state: St<'_>, name: String, values: Value) -> AppResult<Value> {
    let (dir, config) = (state.data_dir.join("extensions"), state.mcp.config_path.clone());
    on_pool(move || reconfigure(&name, &dir, &config, &values)).await?;
    crate::commands::mcp_reload(app, state).await
}

#[tauri::command]
pub async fn extension_uninstall(app: tauri::AppHandle, state: St<'_>, name: String) -> AppResult<Value> {
    let (dir, config) = (state.data_dir.join("extensions"), state.mcp.config_path.clone());
    let folder = on_pool(move || unregister(&name, &dir, &config)).await?;
    // its server stopped with the reload, so its files can go
    let status = crate::commands::mcp_reload(app, state).await?;
    on_pool(move || {
        std::fs::remove_dir_all(&folder).map_err(|e| AppError::msg(format!("The extension was removed, but its folder {} couldn't be deleted: {e}", folder.display())))
    })
    .await?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn install_bundle() {
        let tmp = std::env::temp_dir().join(format!("ext-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let bundle = tmp.join("hello.mcpb");
        {
            let f = std::fs::File::create(&bundle).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("manifest.json", o).unwrap();
            z.write_all(
                json!({
                    "manifest_version": "0.2", "name": "hello world", "version": "1.0.0",
                    "server": {"type": "node", "entry_point": "server/index.js",
                        "mcp_config": {"command": "node", "args": ["${__dirname}/server/index.js", "${user_config.dirs}"],
                            "env": {"KEY": "${user_config.key}"}}},
                    "user_config": {
                        "key": {"type": "string", "title": "API key", "required": true, "sensitive": true},
                        "dirs": {"type": "directory", "multiple": true, "default": ["${HOME}/a"]}
                    }
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();
            z.start_file("server/index.js", o).unwrap();
            z.write_all(b"console.log('hi')").unwrap();
            z.start_file("../evil.txt", o).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        let info = inspect(&bundle).unwrap();
        assert_eq!(info["name"], "hello world");
        let cfg = tmp.join("config.json");
        assert!(install(&bundle, &tmp.join("ext"), &cfg, &json!({})).is_err()); // missing required key
        let name = install(&bundle, &tmp.join("ext"), &cfg, &json!({"key": "secret"})).unwrap();
        assert_eq!(name, "hello-world");
        let c = mcp::read_config(&cfg);
        let srv = &c["mcpServers"]["hello-world"];
        assert_eq!(srv["command"], "node");
        assert!(srv["args"][0].as_str().unwrap().ends_with("server/index.js"));
        assert!(srv["args"][1].as_str().unwrap().ends_with("/a"));
        assert_eq!(srv["env"]["KEY"], "secret");
        assert!(tmp.join("ext/hello-world/server/index.js").exists());
        assert!(!tmp.join("ext/evil.txt").exists() && !tmp.join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(tmp);
    }

    fn bundle(path: &Path, manifest: &Value, files: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("manifest.json", o).unwrap();
        z.write_all(manifest.to_string().as_bytes()).unwrap();
        for (name, data) in files {
            z.start_file(*name, o).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    fn manifest(version: &str, required: bool) -> Value {
        json!({"name": "up", "version": version,
            "server": {"type": "node", "entry_point": "index.js", "mcp_config": {"command": "node", "args": ["${__dirname}/index.js"], "env": {"K": "${user_config.key}"}}},
            "user_config": {"key": {"type": "string", "title": "Key", "required": required}}})
    }

    #[test]
    fn a_failed_upgrade_keeps_the_working_install() {
        let tmp = std::env::temp_dir().join(format!("ext-up-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (ext, cfg) = (tmp.join("ext"), tmp.join("config.json"));
        let v1 = tmp.join("v1.mcpb");
        bundle(&v1, &manifest("1.0.0", false), &[("index.js", b"v1")]);
        install(&v1, &ext, &cfg, &json!({})).unwrap();
        let installed = ext.join("up/index.js");
        assert_eq!(std::fs::read(&installed).unwrap(), b"v1");
        // v2 needs a setting that isn't given: refused before anything is replaced
        let v2 = tmp.join("v2.mcpb");
        bundle(&v2, &manifest("2.0.0", true), &[("index.js", b"v2")]);
        assert!(install(&v2, &ext, &cfg, &json!({})).unwrap_err().to_string().contains("required"));
        assert_eq!(std::fs::read(&installed).unwrap(), b"v1");
        // a bundle whose files are damaged after the manifest: same
        let bad = tmp.join("bad.mcpb");
        bundle(&bad, &manifest("3.0.0", false), &[("index.js", &vec![b'x'; 64 * 1024])]);
        let (at, len) = {
            let mut z = zip::ZipArchive::new(std::fs::File::open(&bad).unwrap()).unwrap();
            let e = z.by_name("index.js").unwrap();
            (e.data_start() as usize, e.compressed_size() as usize)
        };
        let mut bytes = std::fs::read(&bad).unwrap();
        for b in &mut bytes[at..at + len] {
            *b ^= 0x5a;
        }
        std::fs::write(&bad, bytes).unwrap();
        assert!(install(&bad, &ext, &cfg, &json!({})).is_err());
        assert_eq!(std::fs::read(&installed).unwrap(), b"v1");
        assert_eq!(mcp::read_config(&cfg)["mcpServers"]["up"]["extension"]["version"], "1.0.0");
        // a good upgrade replaces it and leaves nothing behind
        install(&v2, &ext, &cfg, &json!({"key": "k"})).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), b"v2");
        let left: Vec<String> = std::fs::read_dir(&ext).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        assert_eq!(left, ["up"], "{left:?}");
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn settings_show_no_secret_and_reconfigure_keeps_an_empty_one() {
        let tmp = std::env::temp_dir().join(format!("ext-conf-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (ext, cfg) = (tmp.join("ext"), tmp.join("config.json"));
        let b = tmp.join("c.mcpb");
        bundle(&b, &json!({"name": "conf", "server": {"type": "node", "mcp_config": {"command": "node", "args": ["--dir=${user_config.dir}"], "env": {"TOKEN": "${user_config.token}"}}},
            "user_config": {"token": {"type": "string", "sensitive": true, "required": true}, "dir": {"type": "directory", "default": "/a"}}}), &[]);
        install(&b, &ext, &cfg, &json!({"token": "s3cret"})).unwrap();
        let s = settings("conf", &ext, &cfg).unwrap();
        assert_eq!(s["values"], json!({"dir": "/a"}));
        assert_eq!(s["secrets_set"], json!(["token"]));
        assert!(!s.to_string().contains("s3cret"), "{s}");
        assert_eq!(s["user_config"]["token"]["sensitive"], true);
        // a new folder; the token left empty stays
        reconfigure("conf", &ext, &cfg, &json!({"dir": "/b", "token": ""})).unwrap();
        let srv = &mcp::read_config(&cfg)["mcpServers"]["conf"];
        assert_eq!(srv["args"], json!(["--dir=/b"]));
        assert_eq!(srv["extension"]["path"], ext.join("conf").display().to_string(), "the path it was installed at, not a canonical one");
        assert_eq!(srv["env"]["TOKEN"], "s3cret", "tests keep secrets in the file, as Linux does");
        reconfigure("conf", &ext, &cfg, &json!({"token": "n3w"})).unwrap();
        let srv = &mcp::read_config(&cfg)["mcpServers"]["conf"];
        assert_eq!((srv["env"]["TOKEN"].as_str(), srv["args"][0].as_str()), (Some("n3w"), Some("--dir=/a")), "an empty setting is its default again");
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn uninstall_takes_only_a_folder_inside_the_extensions_folder() {
        let tmp = std::env::temp_dir().join(format!("ext-rm-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (ext, cfg) = (tmp.join("ext"), tmp.join("config.json"));
        let b = tmp.join("u.mcpb");
        bundle(&b, &manifest("1.0.0", false), &[("index.js", b"x")]);
        install(&b, &ext, &cfg, &json!({})).unwrap();
        // a hand-edited entry pointing elsewhere, and a server that isn't an extension, are refused
        let mut c = mcp::read_config(&cfg);
        c["mcpServers"]["evil"] = json!({"command": "x", "extension": {"path": tmp.display().to_string()}});
        c["mcpServers"]["plain"] = json!({"command": "x"});
        mcp::write_config(&cfg, &c).unwrap();
        assert!(unregister("evil", &ext, &cfg).unwrap_err().to_string().contains("left alone"));
        assert!(unregister("plain", &ext, &cfg).unwrap_err().to_string().contains("isn't an installed extension"));
        assert!(settings("evil", &ext, &cfg).is_err());
        let folder = unregister("up", &ext, &cfg).unwrap();
        assert_eq!(folder, std::fs::canonicalize(ext.join("up")).unwrap());
        let c = mcp::read_config(&cfg);
        assert!(c["mcpServers"]["up"].is_null());
        assert!(c["mcpServers"]["evil"].is_object() && tmp.exists(), "the others are untouched");
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn self_referencing_list_settings_do_not_recurse() {
        let tmp = std::env::temp_dir().join(format!("ext-rec-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let b = tmp.join("rec.mcpb");
        // `$${x}{…}` becomes `${user_config.items}` once `${x}` is substituted
        bundle(&b, &json!({"name": "rec", "server": {"type": "node", "mcp_config": {"command": "node", "args": ["${user_config.items}"]}},
            "user_config": {"items": {"type": "string", "multiple": true, "default": ["$${x}{user_config.items}"]}}}), &[]);
        let cfg = tmp.join("config.json");
        install(&b, &tmp.join("ext"), &cfg, &json!({})).unwrap();
        // expanded once, not again
        assert_eq!(mcp::read_config(&cfg)["mcpServers"]["rec"]["args"], json!(["${user_config.items}"]));
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn oversized_manifests_and_contents_are_refused() {
        let tmp = std::env::temp_dir().join(format!("ext-bomb-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let b = tmp.join("bomb.mcpb");
        {
            let f = std::fs::File::create(&b).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("manifest.json", zip::write::SimpleFileOptions::default()).unwrap();
            // compresses to a few kB
            z.write_all(&vec![b' '; 2 * 1024 * 1024]).unwrap();
            z.finish().unwrap();
        }
        assert!(std::fs::metadata(&b).unwrap().len() < 100_000);
        assert!(inspect(&b).unwrap_err().to_string().contains("too large"));
        let ok = tmp.join("ok.mcpb");
        bundle(&ok, &manifest("1.0.0", false), &[("big.bin", &vec![0u8; 8192])]);
        let dest = tmp.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        assert!(unzip_limited(&ok, &dest, 4096).unwrap_err().to_string().contains("more than"));
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn broken_bundles_say_what_is_wrong() {
        let tmp = std::env::temp_dir().join(format!("ext-broken-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let kind = |name: &str, write: &dyn Fn(&Path)| {
            let p = tmp.join(name);
            write(&p);
            inspect(&p).unwrap_err().kind().to_string()
        };
        assert_eq!(kind("text.mcpb", &|p| std::fs::write(p, "not a zip").unwrap()), "ext_not_zip");
        assert_eq!(kind("empty.mcpb", &|p| bundle_without_manifest(p)), "ext_no_manifest");
        assert_eq!(kind("json.mcpb", &|p| raw_manifest(p, b"{ nope")), "ext_manifest_json");
        assert_eq!(kind("fields.mcpb", &|p| bundle(p, &json!({"name": "x"}), &[])), "ext_manifest_fields");
        assert_eq!(kind("noname.mcpb", &|p| bundle(p, &json!({"server": {"type": "node"}}), &[])), "ext_manifest_fields");
        let _ = std::fs::remove_dir_all(tmp);
    }

    fn bundle_without_manifest(path: &Path) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        z.start_file("index.js", zip::write::SimpleFileOptions::default()).unwrap();
        z.finish().unwrap();
    }

    fn raw_manifest(path: &Path, bytes: &[u8]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        z.start_file("manifest.json", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(bytes).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn the_preview_shows_what_runs_its_signature_and_hash() {
        let tmp = std::env::temp_dir().join(format!("ext-prev-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let b = tmp.join("p.mcpb");
        bundle(&b, &json!({"name": "prev", "version": "1.0.0", "compatibility": {"platforms": ["sunos"]},
            "server": {"type": "binary", "mcp_config": {"command": "${__dirname}/srv", "args": ["--port", "1"], "env": {"TOKEN": "${user_config.t}"},
                "platform_overrides": {platform_key(): {"args": ["--here"]}}}}}), &[]);
        let info = inspect(&b).unwrap();
        assert_eq!(info["run"], "${__dirname}/srv --here", "this platform's override");
        assert_eq!(info["env"], json!(["TOKEN"]));
        assert_eq!(info["compatible"], false);
        assert_eq!(info["signature"]["status"], "unsigned");
        assert_eq!(info["sha256"], hex::encode(<sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&b).unwrap())));
        assert_eq!(info["runtime_missing"], Value::Null, "a bundled program needs no runtime");
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn a_directory_install_keeps_its_source_and_a_tampered_signed_one_is_refused() {
        let tmp = std::env::temp_dir().join(format!("ext-src-{}", crate::util::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (ext, cfg) = (tmp.join("ext"), tmp.join("config.json"));
        let b = tmp.join("s.mcpb");
        bundle(&b, &manifest("1.0.0", false), &[("index.js", b"x")]);
        let source = json!({"id": "up-dir", "version": "1.0.0", "sha256": "ab"});
        install_from(&b, &ext, &cfg, &json!({}), Some(&source)).unwrap();
        assert_eq!(mcp::read_config(&cfg)["mcpServers"]["up"]["extension"]["source"], source);
        reconfigure("up", &ext, &cfg, &json!({"key": "k"})).unwrap();
        assert_eq!(mcp::read_config(&cfg)["mcpServers"]["up"]["extension"]["source"], source, "kept when reconfigured");
        install(&b, &ext, &cfg, &json!({})).unwrap();
        assert!(mcp::read_config(&cfg)["mcpServers"]["up"]["extension"]["source"].is_null(), "a file install isn't from the directory");
        // the signed test bundle with one byte changed
        let mut signed = include_bytes!("testdata/signed_ca.mcpb").to_vec();
        // in index.js, so the manifest still reads
        let at = signed.windows(8).position(|w| w == b"'signed'").unwrap() + 1;
        signed[at] ^= 1;
        let bad = tmp.join("bad.mcpb");
        std::fs::write(&bad, signed).unwrap();
        assert_eq!(install(&bad, &ext, &cfg, &json!({})).unwrap_err().kind(), "ext_signature_invalid");
        assert!(!ext.join("signed-demo").exists());
        let _ = std::fs::remove_dir_all(tmp);
    }
}
