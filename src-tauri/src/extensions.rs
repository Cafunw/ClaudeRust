//! Desktop Extensions: install `.mcpb` / `.dxt` bundles (a zip with a
//! `manifest.json` describing an MCP server) into the app's data folder and
//! register them in `claude_desktop_config.json`.

use crate::error::{AppError, AppResult};
use crate::mcp;
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};

fn read_manifest(zip_path: &Path) -> AppResult<Value> {
    let file = std::fs::File::open(zip_path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| AppError::msg(format!("Not a valid extension file: {e}")))?;
    let mut text = String::new();
    zip.by_name("manifest.json")
        .map_err(|_| AppError::msg("The extension has no manifest.json."))?
        .read_to_string(&mut text)?;
    let v: Value = serde_json::from_str(&text).map_err(|e| AppError::msg(format!("Invalid manifest.json: {e}")))?;
    if !v["server"].is_object() || v["name"].as_str().unwrap_or("").is_empty() {
        return Err(AppError::msg("manifest.json must define \"name\" and \"server\"."));
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

/// Manifest summary for the install dialog.
pub fn inspect(zip_path: &Path) -> AppResult<Value> {
    let m = read_manifest(zip_path)?;
    Ok(json!({
        "name": m["name"],
        "display_name": m["display_name"].as_str().or(m["name"].as_str()),
        "version": m["version"],
        "description": m["description"],
        "author": m["author"]["name"],
        "server_type": m["server"]["type"],
        "user_config": m["user_config"],
        "tools": m["tools"],
    }))
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

/// Substitute `${var}` / `${user_config.key}` in one string. Returns several
/// strings when the whole value is a multi-valued user setting.
fn substitute(s: &str, vars: &Map<String, Value>, user: &Map<String, Value>) -> Vec<String> {
    let lookup = |key: &str| -> Option<Value> {
        if let Some(k) = key.strip_prefix("user_config.") {
            user.get(k).cloned()
        } else {
            vars.get(key).cloned()
        }
    };
    // whole-value multi expansion
    if let Some(inner) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
        if let Some(Value::Array(items)) = lookup(inner) {
            return items
                .iter()
                .map(|v| v.as_str().map(|x| x.to_string()).unwrap_or_else(|| v.to_string()))
                .flat_map(|x| substitute(&x, vars, user))
                .collect();
        }
    }
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
    vec![out]
}

fn unzip(zip_path: &Path, dest: &Path) -> AppResult<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| AppError::msg(e.to_string()))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| AppError::msg(e.to_string()))?;
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
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755 | 0o600));
        }
    }
    Ok(())
}

/// Install an extension and add it to the MCP config. Returns the server name.
pub fn install(zip_path: &Path, extensions_dir: &Path, config_path: &Path, user_values: &Value) -> AppResult<String> {
    let manifest = read_manifest(zip_path)?;
    let name = sanitize_name(manifest["name"].as_str().unwrap_or("extension"));
    let dir = extensions_dir.join(&name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    unzip(zip_path, &dir)?;
    register(&dir, &name, &manifest, config_path, user_values)
}

/// Install an extension that is already unpacked in a folder (for example one
/// installed by the Claude desktop app): copy it and register it.
pub fn install_from_dir(src: &Path, extensions_dir: &Path, config_path: &Path, user_values: &Value) -> AppResult<String> {
    let text = std::fs::read_to_string(src.join("manifest.json"))
        .map_err(|_| AppError::msg("The extension has no manifest.json."))?;
    let manifest: Value = serde_json::from_str(&text).map_err(|e| AppError::msg(format!("Invalid manifest.json: {e}")))?;
    if !manifest["server"].is_object() || manifest["name"].as_str().unwrap_or("").is_empty() {
        return Err(AppError::msg("manifest.json must define \"name\" and \"server\"."));
    }
    let name = sanitize_name(manifest["name"].as_str().unwrap_or("extension"));
    let dir = extensions_dir.join(&name);
    let tmp = extensions_dir.join(format!(".{name}.importing"));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)?;
    }
    copy_dir(src, &tmp)?;
    // Validate the user config before replacing an existing install.
    let res = register(&tmp, &name, &manifest, &extensions_dir.join(".dry-run.json"), user_values);
    let _ = std::fs::remove_file(extensions_dir.join(".dry-run.json"));
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::rename(&tmp, &dir)?;
    register(&dir, &name, &manifest, config_path, user_values)
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

fn register(dir: &Path, name: &str, manifest: &Value, config_path: &Path, user_values: &Value) -> AppResult<String> {
    let name = name.to_string();
    // user config: provided values, else defaults; check required ones
    let mut user = Map::new();
    let vars = base_vars(&dir);
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
            user.insert(k.clone(), val);
        }
    }

    let mut cfg = manifest["server"]["mcp_config"].clone();
    if !cfg.is_object() {
        // derive from server type + entry point
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
    let command = substitute(cfg["command"].as_str().unwrap_or(""), &vars, &user).join(" ");
    if command.trim().is_empty() {
        return Err(AppError::msg("The extension does not define a command to run."));
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
    });

    let mut config = mcp::read_config_for_update(config_path)?;
    config["mcpServers"][&name] = entry;
    mcp::write_config(config_path, &config)?;
    Ok(name)
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
}
