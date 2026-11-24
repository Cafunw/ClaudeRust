//! Agent Skills (folders with a SKILL.md) for Settings: the personal ones in
//! `~/.claude/skills`, a project's in `<project>/.claude/skills`, and those of
//! installed Claude Code plugins (read only: the CLI manages them). A skill
//! is turned off the way the CLI's own /skills menu does it, with
//! `"skillOverrides": {"<name>": "off"}` in the settings next to it, which is
//! undone by removing the key. Installing from a .zip or .skill takes two
//! calls: a preview for the trust prompt, then the install of exactly the
//! file that was shown (by its hash).

use crate::error::{AppError, AppResult};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The part of SKILL.md read for its name and description.
const MAX_FRONT_MATTER_BYTES: u64 = 64 * 1024;
/// The CLI lists this much of a description.
const MAX_DESCRIPTION_CHARS: usize = 1536;
/// SKILL.md as the trust prompt shows it.
const MAX_PREVIEW_BYTES: u64 = 256 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ENTRIES: usize = 1000;
const MAX_UNPACKED_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// "personal", "project" or "plugin"
    pub source: String,
    /// The plugin a plugin skill comes with (`name@marketplace`).
    pub plugin: Option<String>,
    pub path: String,
    pub enabled: bool,
}

/// `name` and `description` of a SKILL.md's front matter (`---` lines at the
/// top, simple YAML: plain, quoted or block scalars).
pub fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, None);
    }
    let body: Vec<&str> = lines.take_while(|l| l.trim_end() != "---").collect();
    let field = |key: &str| -> Option<String> {
        let i = body.iter().position(|l| l.strip_prefix(key).is_some_and(|r| r.trim_start().starts_with(':')))?;
        let raw = body[i][key.len()..].trim_start()[1..].trim();
        let value = if raw.starts_with('|') || raw.starts_with('>') {
            let more: Vec<&str> = body[i + 1..].iter().take_while(|l| l.trim().is_empty() || l.starts_with(' ') || l.starts_with('\t')).map(|l| l.trim()).collect();
            more.join(if raw.starts_with('|') { "\n" } else { " " }).trim().to_string()
        } else if raw.len() >= 2 && ((raw.starts_with('"') && raw.ends_with('"')) || (raw.starts_with('\'') && raw.ends_with('\''))) {
            raw[1..raw.len() - 1].replace("\\\"", "\"").replace("''", "'")
        } else {
            raw.to_string()
        };
        Some(value).filter(|v| !v.is_empty())
    };
    (field("name"), field("description").map(|d| crate::util::truncate_chars(&d, MAX_DESCRIPTION_CHARS)))
}

fn read_head(path: &Path, max: u64) -> Option<String> {
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(max).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// A name the CLI accepts for a skill folder: no control, invisible or
/// direction characters, no `<>` or path syntax, not a reserved one.
fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.chars().count() <= 64
        && n.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
        && !n.starts_with('.')
        && n.trim() == n
        && !n.ends_with('.')
        && n != "synced"
        && !n.starts_with("anthropic-skills")
}

fn skills_in(dir: &Path, source: &str, plugin: Option<&str>, overrides: &Value) -> Vec<Skill> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<Skill> = rd
        .flatten()
        .filter(|e| e.path().join("SKILL.md").is_file())
        .filter_map(|e| {
            let folder = e.file_name().to_string_lossy().into_owned();
            if folder == "synced" || folder.starts_with('.') {
                return None;
            }
            let (name, description) = front_matter(&read_head(&e.path().join("SKILL.md"), MAX_FRONT_MATTER_BYTES)?);
            let name = name.unwrap_or(folder);
            Some(Skill {
                enabled: plugin.is_some() || overrides[&name].as_str() != Some("off"),
                description: description.unwrap_or_default(),
                source: source.into(),
                plugin: plugin.map(String::from),
                path: e.path().display().to_string(),
                name,
            })
        })
        .collect();
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

fn claude_dir() -> AppResult<PathBuf> {
    crate::import::claude_code_dir().ok_or_else(|| AppError::msg("No home folder to keep skills in."))
}

/// The skills folder and the settings file of `scope` ("personal", or "project" with `project`).
fn place(scope: &str, project: Option<&str>) -> AppResult<(PathBuf, PathBuf)> {
    match (scope, project) {
        ("personal", _) => {
            let d = claude_dir()?;
            Ok((d.join("skills"), d.join("settings.json")))
        }
        ("project", Some(p)) if Path::new(p).is_absolute() && Path::new(p).is_dir() => {
            let d = Path::new(p).join(".claude");
            // where the CLI's /skills menu writes them for a project
            Ok((d.join("skills"), d.join("settings.local.json")))
        }
        _ => Err(AppError::msg("Choose where the skill goes: your skills, or a project folder.")),
    }
}

fn read_settings(path: &Path) -> AppResult<Value> {
    match std::fs::read_to_string(path) {
        Ok(t) => {
            let v: Value = serde_json::from_str(t.strip_prefix('\u{feff}').unwrap_or(&t))
                .map_err(|e| AppError::msg(format!("{} isn't valid JSON ({e}), so it was left as it is.", path.display())))?;
            if v.is_object() { Ok(v) } else { Err(AppError::msg(format!("{} must contain a JSON object.", path.display()))) }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e.into()),
    }
}

/// Every skill: personal, the project's (when `project` is a folder), and the plugins'.
pub fn list(project: Option<&str>) -> Vec<Skill> {
    let mut out = vec![];
    for scope in ["personal", "project"] {
        if let Ok((dir, settings)) = place(scope, project) {
            let overrides = read_settings(&settings).map(|s| s["skillOverrides"].clone()).unwrap_or_default();
            out.extend(skills_in(&dir, scope, None, &overrides));
        }
    }
    if let Ok(d) = claude_dir() {
        let installed = read_settings(&d.join("plugins").join("installed_plugins.json")).unwrap_or_default();
        for (plugin, installs) in installed["plugins"].as_object().into_iter().flatten() {
            for i in installs.as_array().into_iter().flatten() {
                if let Some(p) = i["installPath"].as_str() {
                    out.extend(skills_in(&Path::new(p).join("skills"), "plugin", Some(plugin), &Value::Null));
                }
            }
        }
    }
    out
}

/// Turn skill `name` of `scope` on or off for Claude Code sessions. The
/// settings file is written only when this changes it (its keys then come out
/// in alphabetical order: the JSON here keeps no order).
pub fn set_enabled(name: &str, scope: &str, project: Option<&str>, enabled: bool) -> AppResult<()> {
    if !valid_name(name) {
        return Err(AppError::msg("That isn't a skill name."));
    }
    let (_, path) = place(scope, project)?;
    let mut s = read_settings(&path)?;
    if (s["skillOverrides"][name].as_str() != Some("off")) == enabled {
        return Ok(());
    }
    let map = s.as_object_mut().unwrap().entry("skillOverrides").or_insert_with(|| json!({}));
    let Some(map) = map.as_object_mut() else { return Err(AppError::msg(format!("\"skillOverrides\" in {} isn't an object.", path.display()))) };
    if enabled {
        map.remove(name);
    } else {
        map.insert(name.into(), json!("off"));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&s)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// A skill archive, read and checked: its skill's name, SKILL.md, and files.
struct Archive {
    name: String,
    description: String,
    skill_md: String,
    /// (path inside the skill folder, archive entry index, size)
    files: Vec<(String, usize, u64)>,
    sha256: String,
}

/// Read archive `path` (a .zip or .skill) and refuse anything unsafe: too big,
/// too many entries, absolute or `..` paths, links, no single SKILL.md.
fn open_archive(path: &Path) -> AppResult<(zip::ZipArchive<std::io::Cursor<Vec<u8>>>, Archive)> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_ARCHIVE_BYTES {
        return Err(AppError::msg(format!("The skill file is larger than {}.", crate::util::human_size(MAX_ARCHIVE_BYTES))));
    }
    let bytes = std::fs::read(path)?;
    let sha256 = crate::util::sha256_hex(&bytes);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| AppError::msg(format!("Not a valid skill file (a zip): {e}")))?;
    if zip.len() > MAX_ENTRIES {
        return Err(AppError::msg(format!("The skill file has more than {MAX_ENTRIES} entries.")));
    }
    let mut entries = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let e = zip.by_index(i).map_err(|e| AppError::msg(e.to_string()))?;
        let Some(rel) = e.enclosed_name() else {
            return Err(AppError::msg(format!("The skill file has a path outside its folder: {}", e.name())));
        };
        if e.is_symlink() {
            return Err(AppError::msg(format!("The skill file has a link, which isn't installed: {}", e.name())));
        }
        if e.is_dir() {
            continue;
        }
        total = total.saturating_add(e.size());
        if total > MAX_UNPACKED_BYTES {
            return Err(AppError::msg(format!("The skill unpacks to more than {}.", crate::util::human_size(MAX_UNPACKED_BYTES))));
        }
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        // archive tools add these; they aren't the skill's
        if parts.iter().any(|p| p == "__MACOSX" || p == ".DS_Store") {
            continue;
        }
        // a second entry of a path would replace the one the trust prompt shows (on Windows, in any case);
        // a ':' would write an NTFS stream
        if !seen.insert(parts.join("/").to_lowercase()) || parts.iter().any(|p| p.contains(':')) {
            return Err(AppError::msg(format!("The skill file has an entry it can't install as it is: {}", e.name())));
        }
        entries.push((parts, i, e.size()));
    }
    // SKILL.md at the top, or in the one folder at the top
    let top: Option<String> = match entries.iter().find(|(p, ..)| p.len() == 1 && p[0] == "SKILL.md") {
        Some(_) => None,
        None => {
            let tops: std::collections::HashSet<&String> = entries.iter().map(|(p, ..)| &p[0]).collect();
            match (tops.len(), entries.iter().find(|(p, ..)| p.len() == 2 && p[1] == "SKILL.md")) {
                (1, Some((p, ..))) => Some(p[0].clone()),
                _ => return Err(AppError::msg("The skill file needs a SKILL.md, at its top or in its one folder.")),
            }
        }
    };
    let files: Vec<(String, usize, u64)> = entries
        .into_iter()
        .filter_map(|(p, i, size)| {
            let inner = if top.is_some() { &p[1..] } else { &p[..] };
            (!inner.is_empty()).then(|| (inner.join("/"), i, size))
        })
        .collect();
    let md_index = files.iter().find(|(p, ..)| p == "SKILL.md").map(|f| f.1).unwrap_or_default();
    let mut skill_md = String::new();
    zip.by_index(md_index)
        .map_err(|e| AppError::msg(e.to_string()))?
        .take(MAX_PREVIEW_BYTES)
        .read_to_string(&mut skill_md)
        .map_err(|_| AppError::msg("SKILL.md isn't UTF-8 text."))?;
    let (fm_name, description) = front_matter(&skill_md);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    let name = fm_name.or(top).or(stem).unwrap_or_default();
    if !valid_name(&name) {
        return Err(AppError::msg(format!("“{name}” can't be a skill's name: use letters, digits, spaces, - _ or .")));
    }
    Ok((zip, Archive { name, description: description.unwrap_or_default(), skill_md, files, sha256 }))
}

/// For the trust prompt: what installing `path` into `scope` would add.
pub fn preview(path: &Path, scope: &str, project: Option<&str>) -> AppResult<Value> {
    let (_, a) = open_archive(path)?;
    let (dir, _) = place(scope, project)?;
    let target = dir.join(&a.name);
    Ok(json!({
        "name": a.name,
        "description": a.description,
        "skill_md": a.skill_md,
        "files": a.files.iter().map(|(p, _, size)| json!({"path": p, "size": size})).collect::<Vec<_>>(),
        "total_size": a.files.iter().map(|f| f.2).sum::<u64>(),
        "target": target.display().to_string(),
        "exists": std::fs::symlink_metadata(&target).is_ok(),
        "sha256": a.sha256,
    }))
}

/// Install the archive `path` the person saw in `preview` (its `sha256`): a
/// file changed since is refused, and so is a skill already there.
pub fn install(path: &Path, scope: &str, project: Option<&str>, sha256: &str) -> AppResult<Skill> {
    let (mut zip, a) = open_archive(path)?;
    if a.sha256 != sha256 {
        return Err(AppError::msg("The skill file changed since it was shown; open it again."));
    }
    let (dir, settings) = place(scope, project)?;
    let target = dir.join(&a.name);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(AppError::msg(format!("A skill named “{}” is already there ({}); remove it first.", a.name, target.display())));
    }
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".{}.installing-{}", a.name, &crate::util::new_id()[..8]));
    let mut unpack = || -> AppResult<()> {
        let mut written: u64 = 0;
        for (rel, i, _) in &a.files {
            let out = tmp.join(rel);
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut e = zip.by_index(*i).map_err(|e| AppError::msg(e.to_string()))?;
            let room = MAX_UNPACKED_BYTES - written;
            // the header's size can lie: count what is really written
            let n = std::io::copy(&mut (&mut e).take(room + 1), &mut std::fs::File::create(&out)?)?;
            if n > room {
                return Err(AppError::msg(format!("The skill unpacks to more than {}.", crate::util::human_size(MAX_UNPACKED_BYTES))));
            }
            written += n;
        }
        std::fs::rename(&tmp, &target)?;
        Ok(())
    };
    if let Err(e) = unpack() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let overrides = read_settings(&settings).map(|s| s["skillOverrides"].clone()).unwrap_or_default();
    Ok(Skill {
        enabled: overrides[&a.name].as_str() != Some("off"),
        name: a.name,
        description: a.description,
        source: scope.into(),
        plugin: None,
        path: target.display().to_string(),
    })
}

/// Delete skill folder `name` of `scope` (a real folder with a SKILL.md, right
/// inside the skills folder; a link is left for the person to remove).
pub fn uninstall(name: &str, scope: &str, project: Option<&str>) -> AppResult<()> {
    let (dir, _) = place(scope, project)?;
    let folder = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("SKILL.md").is_file() && (p.file_name().is_some_and(|f| f == name) || read_head(&p.join("SKILL.md"), MAX_FRONT_MATTER_BYTES).is_some_and(|t| front_matter(&t).0.as_deref() == Some(name))))
        .ok_or_else(|| AppError::msg(format!("There is no skill “{name}” in {}.", dir.display())))?;
    if std::fs::symlink_metadata(&folder)?.file_type().is_symlink() {
        return Err(AppError::msg(format!("{} is a link to a folder elsewhere; remove the link yourself.", folder.display())));
    }
    if folder.parent() != Some(dir.as_path()) {
        return Err(AppError::msg("That skill isn't in the skills folder."));
    }
    std::fs::remove_dir_all(&folder)?;
    Ok(())
}

// ------------------------------------------------------------------ commands

async fn on_pool<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::msg(e.to_string()))?
}

#[tauri::command]
pub async fn skills_list(project: Option<String>) -> AppResult<Vec<Skill>> {
    on_pool(move || Ok(list(project.as_deref()))).await
}

#[tauri::command]
pub async fn skill_set_enabled(name: String, scope: String, project: Option<String>, enabled: bool) -> AppResult<()> {
    on_pool(move || set_enabled(&name, &scope, project.as_deref(), enabled)).await
}

#[tauri::command]
pub async fn skill_pick_file(app: tauri::AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().add_filter(if crate::turkish(&app) { "Beceri" } else { "Skill" }, &["zip", "skill"]).pick_file(move |f| {
        let _ = tx.send(f.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()));
    });
    Ok(rx.await.unwrap_or(None))
}

#[tauri::command]
pub async fn skill_install_preview(path: String, scope: String, project: Option<String>) -> AppResult<Value> {
    on_pool(move || preview(Path::new(&path), &scope, project.as_deref())).await
}

#[tauri::command]
pub async fn skill_install(path: String, scope: String, project: Option<String>, sha256: String) -> AppResult<Skill> {
    on_pool(move || install(Path::new(&path), &scope, project.as_deref(), &sha256)).await
}

#[tauri::command]
pub async fn skill_uninstall(name: String, scope: String, project: Option<String>) -> AppResult<()> {
    on_pool(move || uninstall(&name, &scope, project.as_deref())).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn front_matter_is_read_leniently() {
        assert_eq!(front_matter("---\nname: deploy\ndescription: Ship it\n---\nbody"), (Some("deploy".into()), Some("Ship it".into())));
        assert_eq!(front_matter("\u{feff}---\r\nname: \"q: x\"\r\ndescription: 'it''s'\r\n---\r\n"), (Some("q: x".into()), Some("it's".into())));
        let folded = front_matter("---\ndescription: >\n  one\n  two\nname: n\n---\n");
        assert_eq!(folded, (Some("n".into()), Some("one two".into())));
        assert_eq!(front_matter("---\ndescription: |\n  a\n  b\n---").1.as_deref(), Some("a\nb"));
        // not at the top, unterminated, or a different key that starts the same
        assert_eq!(front_matter("x\n---\nname: n\n---"), (None, None));
        assert_eq!(front_matter("---\nnames: n\n").0, None);
        let long = front_matter(&format!("---\ndescription: {}\n---", "x".repeat(5000))).1.unwrap();
        assert!(long.chars().count() <= MAX_DESCRIPTION_CHARS + 1);
    }

    fn zip_of(path: &Path, files: &[(&str, &[u8])]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("skills-{tag}-{}", crate::util::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const MD: &[u8] = b"---\nname: pdf-tools\ndescription: Work with PDFs\n---\nRun scripts/x.py";

    #[test]
    fn a_skill_is_shown_then_installed_into_a_project_and_removed() {
        let t = temp("install");
        let project = t.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let p = project.to_str().unwrap();
        let file = t.join("pdf.skill");
        zip_of(&file, &[("pdf-tools/SKILL.md", MD), ("pdf-tools/scripts/x.py", &b"print(1)"[..]), ("__MACOSX/._x", &b""[..])]);
        let pv = preview(&file, "project", Some(p)).unwrap();
        assert_eq!(pv["name"], "pdf-tools");
        assert_eq!(pv["skill_md"], String::from_utf8_lossy(MD).as_ref());
        assert_eq!(pv["files"], json!([{"path": "SKILL.md", "size": MD.len()}, {"path": "scripts/x.py", "size": 8}]));
        assert_eq!(pv["exists"], false);
        assert!(list(Some(p)).iter().all(|s| s.name != "pdf-tools"), "a preview installs nothing");
        // only the file that was shown
        assert!(install(&file, "project", Some(p), "0").unwrap_err().to_string().contains("changed"));
        let s = install(&file, "project", Some(p), pv["sha256"].as_str().unwrap()).unwrap();
        assert_eq!((s.name.as_str(), s.source.as_str(), s.enabled), ("pdf-tools", "project", true));
        assert_eq!(std::fs::read(project.join(".claude/skills/pdf-tools/scripts/x.py")).unwrap(), b"print(1)");
        assert!(install(&file, "project", Some(p), pv["sha256"].as_str().unwrap()).unwrap_err().to_string().contains("already there"));
        // off and on again, the way the CLI reads it
        set_enabled("pdf-tools", "project", Some(p), false).unwrap();
        let settings = read_settings(&project.join(".claude/settings.local.json")).unwrap();
        assert_eq!(settings["skillOverrides"]["pdf-tools"], "off");
        assert!(!list(Some(p)).iter().find(|s| s.name == "pdf-tools").unwrap().enabled);
        set_enabled("pdf-tools", "project", Some(p), true).unwrap();
        assert_eq!(read_settings(&project.join(".claude/settings.local.json")).unwrap()["skillOverrides"], json!({}));
        uninstall("pdf-tools", "project", Some(p)).unwrap();
        assert!(!project.join(".claude/skills/pdf-tools").exists());
        let _ = std::fs::remove_dir_all(t);
    }

    #[test]
    fn hostile_archives_are_refused() {
        let t = temp("hostile");
        let file = t.join("x.zip");
        let refused = |files: &[(&str, &[u8])], why: &str| {
            zip_of(&file, files);
            let e = open_archive(&file).map(|_| ()).unwrap_err().to_string();
            assert!(e.contains(why), "{why}: {e}");
        };
        refused(&[("../evil/SKILL.md", MD)], "outside its folder");
        refused(&[("/abs/SKILL.md", MD)], "outside its folder");
        refused(&[("readme.txt", &b"no skill"[..])], "needs a SKILL.md");
        refused(&[("a/SKILL.md", MD), ("b/SKILL.md", MD)], "needs a SKILL.md");
        refused(&[("SKILL.md", &b"---\nname: ../up\n---"[..])], "can't be a skill's name");
        refused(&[("SKILL.md", &b"---\nname: synced\n---"[..])], "can't be a skill's name");
        // a second SKILL.md in another case would replace the one shown, on a case-insensitive disk
        refused(&[("SKILL.md", MD), ("skill.MD", &b"---\nname: evil\n---"[..])], "can't install as it is");
        refused(&[("s/SKILL.md", MD), ("s/Scripts/a.py", &b""[..]), ("s/scripts/A.py", &b""[..])], "can't install as it is");
        // a link
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            z.start_file("SKILL.md", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(MD).unwrap();
            z.add_symlink("link", "/etc/passwd", zip::write::SimpleFileOptions::default()).unwrap();
            z.finish().unwrap();
        }
        assert!(open_archive(&file).map(|_| ()).unwrap_err().to_string().contains("link"));
        // too many entries, and a bomb that inflates past the cap
        let many: Vec<(String, Vec<u8>)> = (0..=MAX_ENTRIES).map(|i| (format!("f{i}"), vec![])).collect();
        let many: Vec<(&str, &[u8])> = many.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        refused(&many, "entries");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            z.start_file("SKILL.md", o).unwrap();
            z.write_all(MD).unwrap();
            z.start_file("big", o).unwrap();
            let chunk = vec![0u8; 1024 * 1024];
            for _ in 0..=(MAX_UNPACKED_BYTES >> 20) {
                z.write_all(&chunk).unwrap();
            }
            z.finish().unwrap();
        }
        assert!(std::fs::metadata(&file).unwrap().len() < MAX_ARCHIVE_BYTES);
        assert!(open_archive(&file).map(|_| ()).unwrap_err().to_string().contains("unpacks to more than"));
        let _ = std::fs::remove_dir_all(t);
    }

    #[test]
    fn a_broken_settings_file_is_never_overwritten() {
        let t = temp("settings");
        std::fs::create_dir_all(t.join(".claude")).unwrap();
        std::fs::write(t.join(".claude/settings.local.json"), "{ not json").unwrap();
        let p = t.to_str().unwrap();
        assert!(set_enabled("x", "project", Some(p), false).unwrap_err().to_string().contains("left as it is"));
        assert_eq!(std::fs::read_to_string(t.join(".claude/settings.local.json")).unwrap(), "{ not json");
        // other settings are kept
        std::fs::write(t.join(".claude/settings.local.json"), r#"{"permissions": {"allow": ["Bash(ls)"]}}"#).unwrap();
        set_enabled("x", "project", Some(p), false).unwrap();
        let s = read_settings(&t.join(".claude/settings.local.json")).unwrap();
        assert_eq!(s["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(s["skillOverrides"]["x"], "off");
        assert!(set_enabled("../x", "project", Some(p), false).is_err());
        assert!(place("project", Some("relative")).is_err());
        let _ = std::fs::remove_dir_all(t);
    }
}
