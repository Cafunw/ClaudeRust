//! Import from the official Claude apps.
//!
//! - Claude Desktop: MCP servers from `claude_desktop_config.json` and the
//!   desktop extensions it has installed.
//! - Claude Code (the CLI and the Code tab of Claude Desktop): sessions under
//!   `~/.claude/projects`, imported so they can be resumed with the Claude
//!   Code engine, and user-scope MCP servers from `~/.claude.json`.
//! - claude.ai: the data export (Settings → Privacy → Export data), a zip
//!   with `conversations.json` and `projects.json`. Claude Desktop keeps chat
//!   history on claude.ai, not on disk, so this is how chats come over.

use crate::db::{Message, ProjectFile};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::util::{new_id, now_ms};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

type St<'a> = State<'a, Arc<AppState>>;

/// Where to ask for a claude.ai data export.
pub const EXPORT_URL: &str = "https://claude.ai/settings/data-privacy-controls";
const MAX_SESSIONS_LISTED: usize = 500;

// ------------------------------------------------------------------ paths

fn claude_desktop_dir() -> Option<PathBuf> {
    crate::mcp::claude_desktop_config_path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

fn config_dir_override() -> Option<PathBuf> {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
}

fn claude_code_dir() -> Option<PathBuf> {
    config_dir_override().or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
}

fn claude_code_global_config() -> Option<PathBuf> {
    match config_dir_override() {
        Some(d) => Some(d.join(".claude.json")),
        None => dirs::home_dir().map(|h| h.join(".claude.json")),
    }
}

// ------------------------------------------------------------------ helpers

fn ts_ms(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&s.replacen(' ', "T", 1)))
        .ok()
        .map(|d| d.timestamp_millis())
}

fn mtime_ms(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn short_title(s: &str) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut t: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        t.push('…');
    }
    t
}

fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn progress(app: &AppHandle, done: usize, total: usize) {
    let _ = app.emit("import-progress", json!({"done": done, "total": total}));
}

/// Insert rows as a chain under a new conversation; returns the last row id.
/// Timestamps are kept strictly increasing so the tree order is stable.
fn insert_chain(st: &AppState, conv_id: &str, rows: Vec<(String, Vec<Value>, Value, i64)>) -> AppResult<Option<String>> {
    let mut parent: Option<String> = None;
    let mut last_ts = 0i64;
    for (role, content, meta, ts) in rows {
        let ts = ts.max(last_ts + 1);
        last_ts = ts;
        let m = Message {
            id: new_id(),
            conversation_id: conv_id.to_string(),
            parent_id: parent.clone(),
            role: role.clone(),
            content: Value::Array(content.clone()),
            meta,
            created_at: ts,
        };
        st.db.msg_insert(&m)?;
        if m.meta["tool_results"] != true {
            let _ = st.db.fts_index(conv_id, &m.id, &text_of(&content));
        }
        parent = Some(m.id);
    }
    Ok(parent)
}

// ------------------------------------------------------- Claude Code sessions

/// Cheap summary of one session file for the picker.
fn scan_cc_session(path: &Path) -> Option<Value> {
    let f = std::fs::File::open(path).ok()?;
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let reader = std::io::BufReader::with_capacity(1 << 16, f);
    let (mut cwd, mut first, mut custom, mut ai, mut summary) = (None::<String>, None::<String>, None, None, None);
    let mut prompts = 0usize;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.contains("\"type\":\"user\"") {
            if line.contains("\"isSidechain\":true") || line.contains("\"isMeta\":true") || line.contains("\"tool_result\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["isCompactSummary"] == true {
                continue;
            }
            let text = match &v["message"]["content"] {
                Value::String(s) => s.clone(),
                Value::Array(a) => text_of(a),
                _ => String::new(),
            };
            let text = clean_cc_prompt(&text);
            if text.is_empty() {
                continue;
            }
            prompts += 1;
            if cwd.is_none() {
                cwd = v["cwd"].as_str().map(String::from);
            }
            if first.is_none() {
                first = Some(short_title(&text));
            }
        } else if line.contains("\"type\":\"custom-title\"") || line.contains("\"type\":\"ai-title\"") || line.contains("\"type\":\"summary\"") {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            match v["type"].as_str() {
                Some("custom-title") => custom = v["customTitle"].as_str().map(String::from),
                Some("ai-title") => ai = v["aiTitle"].as_str().map(String::from),
                Some("summary") => summary = v["summary"].as_str().map(String::from),
                _ => {}
            }
        }
    }
    if prompts == 0 {
        return None;
    }
    let title = custom.or(ai).or(summary).or(first.clone()).unwrap_or_default();
    Some(json!({
        "path": path.display().to_string(),
        "session_id": path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        "cwd": cwd,
        "title": title,
        "prompts": prompts,
        "size": size,
        "updated_at": mtime_ms(path),
    }))
}

fn cc_session_files() -> Vec<PathBuf> {
    let Some(dir) = claude_code_dir() else { return vec![] };
    let mut files = vec![];
    if let Ok(projects) = std::fs::read_dir(dir.join("projects")) {
        for p in projects.flatten() {
            if !p.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(p.path()) {
                for e in entries.flatten() {
                    let path = e.path();
                    if path.extension().and_then(|x| x.to_str()) == Some("jsonl") && path.is_file() {
                        files.push(path);
                    }
                }
            }
        }
    }
    files.sort_by_cached_key(|p| std::cmp::Reverse(mtime_ms(p)));
    files
}

/// Text the CLI stores for slash commands and local command output.
fn clean_cc_prompt(text: &str) -> String {
    let t = text.trim();
    if t.starts_with("<local-command-stdout>")
        || t.starts_with("<local-command-stderr>")
        || t.starts_with("<local-command-caveat>")
        || t.starts_with("[Request interrupted by user")
        || t.starts_with("<system-reminder>")
    {
        return String::new();
    }
    if t.contains("<command-name>") {
        let grab = |tag: &str| -> String {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            t.find(&open)
                .and_then(|i| {
                    let rest = &t[i + open.len()..];
                    rest.find(&close).map(|j| rest[..j].trim().to_string())
                })
                .unwrap_or_default()
        };
        let name = grab("command-name");
        let args = grab("command-args");
        let name = if name.starts_with('/') { name } else { format!("/{name}") };
        return if args.is_empty() { name } else { format!("{name} {args}") };
    }
    t.to_string()
}

pub struct ParsedSession {
    pub session_id: String,
    pub cwd: Option<String>,
    pub title: String,
    pub model: Option<String>,
    /// role, content, meta, timestamp
    pub rows: Vec<(String, Vec<Value>, Value, i64)>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Read a Claude Code session file and rebuild the main conversation thread
/// (from the newest message back through `parentUuid`), as rows shaped like
/// the ones the Claude Code engine stores.
pub fn parse_cc_session(path: &Path) -> AppResult<ParsedSession> {
    let text = std::fs::read_to_string(path)?;
    let mut entries: Vec<Value> = vec![];
    let mut by_uuid: HashMap<String, usize> = HashMap::new();
    let (mut custom, mut ai, mut summary) = (None, None, None);
    let mut session_id = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut sid_seen = false;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["type"].as_str() {
            Some("custom-title") => custom = v["customTitle"].as_str().map(String::from),
            Some("ai-title") => ai = v["aiTitle"].as_str().map(String::from),
            Some("summary") => summary = v["summary"].as_str().map(String::from),
            _ => {}
        }
        if let Some(u) = v["uuid"].as_str() {
            if !sid_seen {
                if let Some(s) = v["sessionId"].as_str() {
                    session_id = s.to_string();
                    sid_seen = true;
                }
            }
            by_uuid.insert(u.to_string(), entries.len());
            entries.push(v);
        }
    }
    let is_msg = |e: &Value| matches!(e["type"].as_str(), Some("user") | Some("assistant")) && e["isSidechain"] != true;
    let leaf = entries
        .iter()
        .rposition(is_msg)
        .ok_or_else(|| AppError::msg("The session has no messages."))?;
    let mut chain = vec![];
    let mut seen = HashSet::new();
    let mut cur = Some(leaf);
    while let Some(i) = cur {
        if !seen.insert(i) {
            break;
        }
        chain.push(i);
        let e = &entries[i];
        let parent = e["parentUuid"].as_str().or_else(|| e["logicalParentUuid"].as_str());
        cur = parent.and_then(|p| by_uuid.get(p).copied());
    }
    chain.reverse();

    let mut rows: Vec<(String, Vec<Value>, Value, i64)> = vec![];
    let mut cur_api: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cwd: Option<String> = None;
    let (mut created, mut updated) = (i64::MAX, 0i64);
    for i in chain {
        let e = &entries[i];
        if !is_msg(e) || e["isMeta"] == true || e["isCompactSummary"] == true || e["isVisibleInTranscriptOnly"] == true {
            continue;
        }
        let ts = ts_ms(&e["timestamp"]).unwrap_or_else(now_ms);
        created = created.min(ts);
        updated = updated.max(ts);
        if cwd.is_none() {
            cwd = e["cwd"].as_str().map(String::from);
        }
        let uuid = e["uuid"].clone();
        let msg = &e["message"];
        let blocks: Vec<Value> = match &msg["content"] {
            Value::String(s) => vec![json!({"type": "text", "text": s})],
            Value::Array(a) => a.clone(),
            _ => vec![],
        };
        if e["type"] == "assistant" {
            let api_id = msg["id"].as_str().unwrap_or("").to_string();
            if let Some(m) = msg["model"].as_str().filter(|m| !m.starts_with('<')) {
                model = Some(m.to_string());
            }
            let same = cur_api.as_deref() == Some(api_id.as_str())
                && !api_id.is_empty()
                && rows.last().map(|r| r.0 == "assistant").unwrap_or(false);
            if same {
                let r = rows.last_mut().unwrap();
                r.1.extend(blocks);
                r.2["cc_uuid"] = uuid;
                if !msg["usage"].is_null() {
                    r.2["usage"] = msg["usage"].clone();
                }
            } else {
                let mut meta = json!({"engine": crate::claudecode::ENGINE, "cc_uuid": uuid, "cc_session": session_id});
                if let Some(m) = msg["model"].as_str() {
                    meta["model"] = json!(m);
                }
                if !msg["usage"].is_null() {
                    meta["usage"] = msg["usage"].clone();
                }
                rows.push(("assistant".into(), blocks, meta, ts));
            }
            cur_api = Some(api_id);
            continue;
        }
        // user
        cur_api = None;
        if blocks.iter().any(|b| b["type"] == "tool_result") {
            let meta = json!({"tool_results": true, "cc_uuid": uuid, "cc_session": session_id});
            rows.push(("user".into(), blocks, meta, ts));
            continue;
        }
        let mut content = vec![];
        let mut atts = vec![];
        for b in blocks {
            match b["type"].as_str() {
                Some("text") => {
                    let t = clean_cc_prompt(b["text"].as_str().unwrap_or(""));
                    if !t.is_empty() {
                        content.push(json!({"type": "text", "text": t}));
                    }
                }
                Some("image") => {
                    let mime = b["source"]["media_type"].as_str().unwrap_or("image/png").to_string();
                    let size = b["source"]["data"].as_str().map(|d| d.len() * 3 / 4).unwrap_or(0);
                    atts.push(json!({"name": format!("image.{}", mime.rsplit('/').next().unwrap_or("png")), "kind": "image", "mime": mime, "size": size}));
                    content.push(b);
                }
                Some("document") => {
                    let name = b["title"].as_str().unwrap_or("document").to_string();
                    let kind = if b["source"]["media_type"] == "application/pdf" { "pdf" } else { "text" };
                    atts.push(json!({"name": name, "kind": kind, "size": b["source"]["data"].as_str().map(|d| d.len()).unwrap_or(0)}));
                    content.push(b);
                }
                _ => {}
            }
        }
        if content.is_empty() {
            continue;
        }
        let mergeable = rows
            .last()
            .map(|r| r.0 == "user" && r.2["tool_results"] != true)
            .unwrap_or(false);
        if mergeable {
            let r = rows.last_mut().unwrap();
            r.1.extend(content);
            if let Some(a) = r.2["attachments"].as_array_mut() {
                a.extend(atts);
            }
            r.2["cc_uuid"] = uuid;
        } else {
            rows.push(("user".into(), content, json!({"attachments": atts, "cc_uuid": uuid, "cc_session": session_id}), ts));
        }
    }
    // Imported replies are complete; record why each one ended so they are
    // not mistaken for responses cut off by a crash.
    for r in rows.iter_mut().filter(|r| r.0 == "assistant") {
        if r.2["stop_reason"].is_null() {
            let ends_in_tool = r.1.last().map(|b| b["type"] == "tool_use").unwrap_or(false);
            r.2["stop_reason"] = json!(if ends_in_tool { "tool_use" } else { "end_turn" });
        }
    }
    // A thread starts with a prompt.
    let start = rows
        .iter()
        .position(|r| r.0 == "user" && r.2["tool_results"] != true)
        .ok_or_else(|| AppError::msg("The session has no prompts."))?;
    rows.drain(..start);
    let first_prompt = text_of(&rows[0].1);
    let title = custom.or(ai).or(summary).unwrap_or_else(|| short_title(&first_prompt));
    Ok(ParsedSession {
        session_id,
        cwd,
        title,
        model,
        rows,
        created_at: if created == i64::MAX { now_ms() } else { created },
        updated_at: updated.max(created.min(now_ms())),
    })
}

fn import_cc_session(st: &AppState, path: &Path, done: &mut HashSet<String>) -> AppResult<Option<String>> {
    let p = parse_cc_session(path)?;
    let key = format!("claude-code:{}", p.session_id);
    if done.contains(&key) {
        return Ok(None);
    }
    let settings = st.settings.read().clone();
    let model = p.model.clone().unwrap_or_else(|| settings.code_model.clone());
    let cwd = p
        .cwd
        .clone()
        .or_else(|| dirs::home_dir().map(|h| h.display().to_string()))
        .unwrap_or_default();
    let options = json!({
        "engine": crate::claudecode::ENGINE,
        "permission_mode": settings.code_permission_mode,
        "cc_session": p.session_id,
        "imported_from": key,
        "date": crate::engine::today(),
    });
    let conv = st.db.conv_create("code", &model, None, Some(&cwd), &options)?;
    let res = (|| -> AppResult<()> {
        let last = insert_chain(st, &conv.id, p.rows)?;
        st.db.conv_set_leaf(&conv.id, last.as_deref())?;
        let mut o = options.clone();
        if let Some(l) = &last {
            o["cc_tip"] = json!(l);
        }
        st.db.conv_set_options(&conv.id, &o)?;
        st.db.conv_set_title(&conv.id, &p.title)?;
        st.db.conv_set_times(&conv.id, p.created_at, p.updated_at)?;
        Ok(())
    })();
    if let Err(e) = res {
        let _ = st.db.conv_delete(&conv.id);
        return Err(e);
    }
    done.insert(key);
    Ok(Some(conv.id))
}

// ------------------------------------------------------------ claude.ai export

fn read_zip_json(zip: &mut zip::ZipArchive<std::fs::File>, suffix: &str) -> AppResult<Option<Value>> {
    let name = (0..zip.len()).find_map(|i| {
        let f = zip.by_index(i).ok()?;
        let n = f.name().to_string();
        let base = n.rsplit('/').next().unwrap_or("");
        (base == suffix && !n.contains("__MACOSX")).then_some(n)
    });
    let Some(name) = name else { return Ok(None) };
    let mut s = String::new();
    zip.by_name(&name)
        .map_err(|e| AppError::msg(format!("Could not read {name}: {e}")))?
        .read_to_string(&mut s)?;
    Ok(Some(serde_json::from_str(&s).map_err(|e| AppError::msg(format!("{name} is not valid JSON: {e}")))?))
}

fn as_list(v: Option<Value>, key: &str) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a,
        Some(Value::Object(mut o)) => match o.remove(key) {
            Some(Value::Array(a)) => a,
            _ => vec![],
        },
        _ => vec![],
    }
}

/// (conversations, projects) from a zip, a folder, or a conversations.json.
pub fn load_export(path: &Path) -> AppResult<(Vec<Value>, Vec<Value>)> {
    let read = |p: &Path| -> AppResult<Option<Value>> {
        if !p.is_file() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(p)?;
        Ok(Some(serde_json::from_str(&s).map_err(|e| AppError::msg(format!("{} is not valid JSON: {e}", p.display())))?))
    };
    if path.is_dir() {
        return Ok((
            as_list(read(&path.join("conversations.json"))?, "conversations"),
            as_list(read(&path.join("projects.json"))?, "projects"),
        ));
    }
    let is_zip = {
        let mut magic = [0u8; 4];
        std::fs::File::open(path)?.read(&mut magic).unwrap_or(0) == 4 && &magic == b"PK\x03\x04"
    };
    if is_zip {
        let f = std::fs::File::open(path)?;
        let mut zip = zip::ZipArchive::new(f).map_err(|e| AppError::msg(format!("Not a valid zip file: {e}")))?;
        let convs = read_zip_json(&mut zip, "conversations.json")?;
        if convs.is_none() {
            return Err(AppError::msg("This zip has no conversations.json. Use the export you get from claude.ai → Settings → Privacy → Export data."));
        }
        let projects = read_zip_json(&mut zip, "projects.json")?;
        return Ok((as_list(convs, "conversations"), as_list(projects, "projects")));
    }
    let v = read(path)?;
    let looks_like_projects = path.file_name().map(|n| n == "projects.json").unwrap_or(false);
    if looks_like_projects {
        Ok((vec![], as_list(v, "projects")))
    } else {
        Ok((as_list(v, "conversations"), vec![]))
    }
}

type ArtifactState = HashMap<String, (String, String, Option<String>, String)>;

fn attr(s: &str) -> String {
    s.replace('"', "&quot;").replace('>', "›")
}

/// claude.ai stores artifacts as `artifacts` tool calls; turn them back into
/// the inline tags this app renders.
fn artifact_tag(input: &Value, state: &mut ArtifactState) -> Option<String> {
    let id = input["id"].as_str().filter(|s| !s.is_empty())?.to_string();
    let prev = state.get(&id).cloned();
    let kind = input["type"]
        .as_str()
        .map(String::from)
        .or_else(|| prev.as_ref().map(|p| p.0.clone()))
        .unwrap_or_else(|| "text/markdown".into());
    let title = input["title"]
        .as_str()
        .map(String::from)
        .or_else(|| prev.as_ref().map(|p| p.1.clone()))
        .unwrap_or_else(|| id.clone());
    let language = input["language"]
        .as_str()
        .map(String::from)
        .or_else(|| prev.as_ref().and_then(|p| p.2.clone()));
    let content = match input["command"].as_str().unwrap_or("create") {
        "update" => {
            let mut c = prev.as_ref().map(|p| p.3.clone()).unwrap_or_default();
            let old = input["old_str"].as_str().unwrap_or("");
            let new = input["new_str"].as_str().unwrap_or("");
            if !old.is_empty() && c.contains(old) {
                c = c.replacen(old, new, 1);
            }
            c
        }
        _ => input["content"]
            .as_str()
            .map(String::from)
            .or_else(|| prev.as_ref().map(|p| p.3.clone()))
            .unwrap_or_default(),
    };
    state.insert(id.clone(), (kind.clone(), title.clone(), language.clone(), content.clone()));
    let lang = language.map(|l| format!(" language=\"{}\"", attr(&l))).unwrap_or_default();
    Some(format!(
        "<antArtifact identifier=\"{}\" type=\"{}\" title=\"{}\"{lang}>\n{content}\n</antArtifact>",
        attr(&id),
        attr(&kind),
        attr(&title)
    ))
}

fn convert_human(m: &Value) -> (Vec<Value>, Value) {
    let mut content = vec![];
    let mut atts = vec![];
    for a in m["attachments"].as_array().into_iter().flatten() {
        let name = a["file_name"].as_str().unwrap_or("attachment.txt");
        let text = a["extracted_content"].as_str().unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        content.push(json!({
            "type": "document",
            "source": {"type": "text", "media_type": "text/plain", "data": text},
            "title": name
        }));
        atts.push(json!({"name": name, "kind": "text", "mime": "text/plain", "size": a["file_size"].as_u64().unwrap_or(text.len() as u64)}));
    }
    // Uploaded images and PDFs: the export lists them but has no file data.
    let files = m["files_v2"].as_array().or(m["files"].as_array());
    let mut seen = HashSet::new();
    for f in files.into_iter().flatten() {
        let name = f["file_name"].as_str().unwrap_or("file");
        if seen.insert(name.to_string()) {
            atts.push(json!({"name": name, "kind": "file", "size": 0, "missing": true}));
        }
    }
    let mut text = m["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        text = m["text"].as_str().unwrap_or("").to_string();
    }
    if !text.trim().is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }
    (content, json!({"attachments": atts}))
}

fn convert_assistant(m: &Value, arts: &mut ArtifactState) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    let push_text = |out: &mut Vec<Value>, t: &str| {
        if t.is_empty() {
            return;
        }
        if let Some(last) = out.last_mut().filter(|b| b["type"] == "text") {
            let prev = last["text"].as_str().unwrap_or("").to_string();
            let sep = if prev.ends_with('\n') || t.starts_with('\n') { "\n" } else { "\n\n" };
            last["text"] = json!(format!("{prev}{sep}{t}"));
        } else {
            out.push(json!({"type": "text", "text": t}));
        }
    };
    let blocks = m["content"].as_array().cloned().unwrap_or_default();
    for b in &blocks {
        match b["type"].as_str() {
            Some("text") => push_text(&mut out, b["text"].as_str().unwrap_or("")),
            Some("thinking") => {
                let t = b["thinking"].as_str().unwrap_or("");
                if !t.is_empty() {
                    out.push(json!({"type": "thinking", "thinking": t, "signature": ""}));
                }
            }
            Some("tool_use") if b["name"] == "artifacts" => {
                if let Some(tag) = artifact_tag(&b["input"], arts) {
                    push_text(&mut out, &tag);
                }
            }
            _ => {}
        }
    }
    if !out.iter().any(|b| b["type"] == "text") {
        if let Some(t) = m["text"].as_str().filter(|t| !t.trim().is_empty()) {
            push_text(&mut out, t);
        }
    }
    out
}

fn import_ai_conversation(
    st: &AppState,
    c: &Value,
    projects: &HashMap<String, String>,
    done: &mut HashSet<String>,
) -> AppResult<bool> {
    let Some(uuid) = c["uuid"].as_str() else { return Ok(false) };
    let key = format!("claude.ai:{uuid}");
    if done.contains(&key) {
        return Ok(false);
    }
    let mut msgs = c["chat_messages"].as_array().cloned().unwrap_or_default();
    if msgs.is_empty() {
        return Ok(false);
    }
    // Parents must be inserted before their children.
    if msgs.iter().any(|m| m["parent_message_uuid"].is_string()) {
        msgs.sort_by_key(|m| ts_ms(&m["created_at"]).unwrap_or(i64::MAX));
    }
    let settings = st.settings.read().clone();
    let model = c["model"]
        .as_str()
        .filter(|m| !m.is_empty())
        .map(String::from)
        .unwrap_or_else(|| settings.default_model.clone());
    let project_id = c["project_uuid"]
        .as_str()
        .or_else(|| c["project"]["uuid"].as_str())
        .and_then(|u| projects.get(u))
        .cloned();
    let options = json!({"imported_from": key, "date": crate::engine::today()});
    let conv = st.db.conv_create("chat", &model, project_id.as_deref(), None, &options)?;
    let res = (|| -> AppResult<()> {
        let tree = msgs.iter().any(|m| m["parent_message_uuid"].is_string());
        let mut ids: HashMap<String, Option<String>> = HashMap::new();
        let mut prev: Option<String> = None;
        let mut last_ts = 0i64;
        let mut arts = ArtifactState::new();
        let mut first_text = String::new();
        for m in &msgs {
            let human = m["sender"] == "human";
            let parent = if tree {
                m["parent_message_uuid"]
                    .as_str()
                    .and_then(|p| ids.get(p).cloned())
                    .flatten()
            } else {
                prev.clone()
            };
            let (content, meta) = if human {
                convert_human(m)
            } else {
                (convert_assistant(m, &mut arts), json!({"imported": true, "stop_reason": "end_turn"}))
            };
            let my_uuid = m["uuid"].as_str().unwrap_or("").to_string();
            if content.is_empty() {
                // Keep children attached to something sensible.
                ids.insert(my_uuid, parent.clone());
                continue;
            }
            let ts = ts_ms(&m["created_at"]).unwrap_or_else(now_ms).max(last_ts + 1);
            last_ts = ts;
            let row = Message {
                id: new_id(),
                conversation_id: conv.id.clone(),
                parent_id: parent,
                role: if human { "user".into() } else { "assistant".into() },
                content: Value::Array(content.clone()),
                meta,
                created_at: ts,
            };
            st.db.msg_insert(&row)?;
            let text = text_of(&content);
            let _ = st.db.fts_index(&conv.id, &row.id, &text);
            if human && first_text.is_empty() {
                first_text = text.clone();
            }
            if !human {
                crate::engine::save_artifacts(st, &conv.id, &row.id, &text);
            }
            ids.insert(my_uuid, Some(row.id.clone()));
            prev = Some(row.id);
        }
        let leaf = c["current_leaf_message_uuid"]
            .as_str()
            .and_then(|u| ids.get(u).cloned())
            .flatten()
            .or(prev);
        if leaf.is_none() {
            return Err(AppError::msg("empty"));
        }
        st.db.conv_set_leaf(&conv.id, leaf.as_deref())?;
        let title = c["name"]
            .as_str()
            .filter(|t| !t.trim().is_empty())
            .map(String::from)
            .unwrap_or_else(|| short_title(&first_text));
        st.db.conv_set_title(&conv.id, &title)?;
        let created = ts_ms(&c["created_at"]).unwrap_or(last_ts);
        let updated = ts_ms(&c["updated_at"]).unwrap_or(last_ts).max(created);
        st.db.conv_set_times(&conv.id, created, updated)?;
        Ok(())
    })();
    match res {
        Ok(()) => {
            done.insert(key);
            Ok(true)
        }
        Err(e) => {
            let _ = st.db.conv_delete(&conv.id);
            if e.to_string() == "empty" { Ok(false) } else { Err(e) }
        }
    }
}

/// Projects from projects.json; returns claude.ai uuid → our project id, and
/// how many were created.
fn import_ai_projects(st: &AppState, projects: &[Value]) -> AppResult<(HashMap<String, String>, usize)> {
    let mut by_name: HashMap<String, String> =
        st.db.project_list()?.into_iter().map(|p| (p.name.clone(), p.id)).collect();
    let mut map = HashMap::new();
    let mut created = 0;
    for p in projects {
        if p["is_starter_project"] == true {
            continue;
        }
        let Some(uuid) = p["uuid"].as_str() else { continue };
        let name = p["name"].as_str().unwrap_or("Imported project").to_string();
        if let Some(id) = by_name.get(&name) {
            map.insert(uuid.to_string(), id.clone());
            continue;
        }
        let proj = st.db.project_create(&name, p["description"].as_str().unwrap_or(""))?;
        if let Some(instr) = p["prompt_template"].as_str().filter(|s| !s.trim().is_empty()) {
            st.db.project_update(&proj.id, None, None, Some(instr), None)?;
        }
        for d in p["docs"].as_array().into_iter().flatten() {
            let content = d["content"].as_str().unwrap_or("");
            st.db.project_file_add(&ProjectFile {
                id: new_id(),
                project_id: proj.id.clone(),
                name: d["filename"].as_str().unwrap_or("document.txt").to_string(),
                mime: "text/plain".into(),
                size: content.len() as i64,
                kind: "text".into(),
                data: content.to_string(),
                created_at: ts_ms(&d["created_at"]).unwrap_or_else(now_ms),
            })?;
        }
        map.insert(uuid.to_string(), proj.id.clone());
        by_name.insert(name, proj.id);
        created += 1;
    }
    Ok((map, created))
}

// ------------------------------------------------------------- MCP servers

fn desktop_extensions(ours: &Value) -> Vec<Value> {
    let Some(dir) = claude_desktop_dir() else { return vec![] };
    let mut out = vec![];
    let Ok(entries) = std::fs::read_dir(dir.join("Claude Extensions")) else { return out };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(text) = std::fs::read_to_string(path.join("manifest.json")) else { continue };
        let Ok(m) = serde_json::from_str::<Value>(&text) else { continue };
        let id = e.file_name().to_string_lossy().to_string();
        let settings = std::fs::read_to_string(dir.join("Claude Extensions Settings").join(format!("{id}.json")))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .unwrap_or(Value::Null);
        let name = crate::extensions::sanitize_name(m["name"].as_str().unwrap_or(&id));
        out.push(json!({
            "id": id,
            "name": m["display_name"].as_str().or(m["name"].as_str()).unwrap_or(&id),
            "version": m["version"],
            "description": m["description"],
            "enabled": settings["isEnabled"].as_bool().unwrap_or(true),
            "installed": ours["mcpServers"].get(&name).is_some(),
        }));
    }
    out
}

/// Claude Code's user-scope servers, converted to the desktop config format.
fn cc_mcp_servers() -> Map<String, Value> {
    let mut out = Map::new();
    let Some(p) = claude_code_global_config() else { return out };
    let cfg = crate::mcp::read_config(&p);
    for (name, s) in cfg["mcpServers"].as_object().into_iter().flatten() {
        let ty = s["type"].as_str().unwrap_or(if s["url"].is_string() { "http" } else { "stdio" });
        let entry = match ty {
            "stdio" if s["command"].is_string() => {
                let mut e = json!({"command": s["command"], "args": s["args"].as_array().cloned().unwrap_or_default()});
                if s["env"].as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                    e["env"] = s["env"].clone();
                }
                e
            }
            "http" if s["url"].is_string() => {
                let mut e = json!({"url": s["url"]});
                if s["headers"].is_object() {
                    e["headers"] = s["headers"].clone();
                }
                e
            }
            // Legacy SSE servers aren't supported by the MCP client here.
            _ => continue,
        };
        out.insert(name.clone(), entry);
    }
    out
}

fn server_list(servers: &Map<String, Value>, ours: &Value) -> Vec<Value> {
    servers
        .iter()
        .map(|(k, v)| {
            json!({
                "name": k,
                "transport": if v["url"].is_string() { "http" } else { "stdio" },
                "exists": ours["mcpServers"].get(k).is_some(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn import_scan(state: St<'_>) -> AppResult<Value> {
    let st = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let ours = crate::mcp::read_config(&st.mcp.config_path);
        let imported = st.db.imported_keys().unwrap_or_default();

        let desktop_cfg = crate::mcp::claude_desktop_config_path().filter(|p| p.exists());
        let desktop_servers = desktop_cfg
            .as_ref()
            .map(|p| crate::mcp::read_config(p)["mcpServers"].as_object().cloned().unwrap_or_default())
            .unwrap_or_default();
        let desktop_dir = claude_desktop_dir().filter(|d| d.is_dir());

        let files = cc_session_files();
        let total = files.len();
        let mut sessions: Vec<Value> = files
            .iter()
            .take(MAX_SESSIONS_LISTED)
            .filter_map(|f| scan_cc_session(f))
            .collect();
        for s in &mut sessions {
            let key = format!("claude-code:{}", s["session_id"].as_str().unwrap_or(""));
            s["imported"] = json!(imported.contains(&key));
        }
        let cc_dir = claude_code_dir().filter(|d| d.is_dir());
        json!({
            "desktop": {
                "found": desktop_dir.is_some(),
                "dir": desktop_dir.map(|d| d.display().to_string()),
                "config": desktop_cfg.map(|p| p.display().to_string()),
                "mcp_servers": server_list(&desktop_servers, &ours),
                "extensions": desktop_extensions(&ours),
            },
            "claude_code": {
                "found": cc_dir.is_some(),
                "dir": cc_dir.map(|d| d.display().to_string()),
                "sessions": sessions,
                "total_sessions": total,
                "mcp_servers": server_list(&cc_mcp_servers(), &ours),
            },
            "export_url": EXPORT_URL,
        })
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))
}

/// Copy MCP servers from Claude Desktop ("desktop") or Claude Code
/// ("claude-code") into this app's config. Existing names are left alone.
#[tauri::command]
pub async fn import_mcp_servers(app: AppHandle, state: St<'_>, source: String, names: Option<Vec<String>>) -> AppResult<Value> {
    let st = state.inner().clone();
    let theirs: Map<String, Value> = match source.as_str() {
        "desktop" => {
            let p = crate::mcp::claude_desktop_config_path()
                .filter(|p| p.exists())
                .ok_or_else(|| AppError::msg("Claude Desktop's config file was not found on this computer."))?;
            crate::mcp::read_config(&p)["mcpServers"].as_object().cloned().unwrap_or_default()
        }
        "claude-code" => cc_mcp_servers(),
        _ => return Err(AppError::msg("Unknown import source.")),
    };
    crate::mcp::ensure_config(&st.mcp.config_path)?;
    let mut ours = crate::mcp::read_config(&st.mcp.config_path);
    if !ours["mcpServers"].is_object() {
        ours["mcpServers"] = json!({});
    }
    let mut n = 0;
    if let Some(dst) = ours["mcpServers"].as_object_mut() {
        for (k, v) in theirs {
            if names.as_ref().map(|ns| !ns.contains(&k)).unwrap_or(false) || dst.contains_key(&k) {
                continue;
            }
            dst.insert(k, v);
            n += 1;
        }
    }
    std::fs::write(&st.mcp.config_path, serde_json::to_string_pretty(&ours)?)?;
    if n > 0 {
        crate::commands::reload_mcp_in_background(&app, &st);
    }
    Ok(json!({"imported": n}))
}

/// Install Claude Desktop's extensions (by folder id) here, with the settings
/// the user gave them there.
#[tauri::command]
pub async fn import_desktop_extensions(app: AppHandle, state: St<'_>, ids: Vec<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    let dir = claude_desktop_dir().ok_or_else(|| AppError::msg("Claude Desktop was not found on this computer."))?;
    let (installed, errors) = {
        let st = st.clone();
        tokio::task::spawn_blocking(move || {
            let ext_dir = st.data_dir.join("extensions");
            let _ = std::fs::create_dir_all(&ext_dir);
            let mut installed = vec![];
            let mut errors = vec![];
            for id in ids {
                if id.contains('/') || id.contains('\\') || id.starts_with('.') {
                    continue;
                }
                let src = dir.join("Claude Extensions").join(&id);
                let values = std::fs::read_to_string(dir.join("Claude Extensions Settings").join(format!("{id}.json")))
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .map(|v| v["userConfig"].clone())
                    .filter(|v| v.is_object())
                    .unwrap_or_else(|| json!({}));
                match crate::extensions::install_from_dir(&src, &ext_dir, &st.mcp.config_path, &values) {
                    Ok(name) => installed.push(name),
                    Err(e) => errors.push(json!({"id": id, "error": e.to_string()})),
                }
            }
            (installed, errors)
        })
        .await
        .map_err(|e| AppError::msg(e.to_string()))?
    };
    if !installed.is_empty() {
        crate::commands::reload_mcp_in_background(&app, &st);
    }
    Ok(json!({"installed": installed, "errors": errors}))
}

/// Import Claude Code sessions (paths from `import_scan`).
#[tauri::command]
pub async fn import_claude_code_sessions(app: AppHandle, state: St<'_>, paths: Vec<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    let root = claude_code_dir()
        .map(|d| d.join("projects"))
        .and_then(|d| d.canonicalize().ok())
        .ok_or_else(|| AppError::msg("Claude Code's data folder was not found."))?;
    tokio::task::spawn_blocking(move || {
        let mut done = st.db.imported_keys().unwrap_or_default();
        let (mut imported, mut skipped) = (0, 0);
        let mut errors = vec![];
        let total = paths.len();
        for (i, p) in paths.iter().enumerate() {
            progress(&app, i, total);
            let path = match PathBuf::from(p).canonicalize() {
                Ok(c) if c.starts_with(&root) => c,
                _ => {
                    errors.push(json!({"path": p, "error": "Not a Claude Code session file."}));
                    continue;
                }
            };
            match import_cc_session(&st, &path, &mut done) {
                Ok(Some(_)) => imported += 1,
                Ok(None) => skipped += 1,
                Err(e) => errors.push(json!({"path": p, "error": e.to_string()})),
            }
        }
        progress(&app, total, total);
        Ok(json!({"imported": imported, "skipped": skipped, "errors": errors}))
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))?
}

/// Import a claude.ai data export (zip, folder or conversations.json).
#[tauri::command]
pub async fn import_claude_export(app: AppHandle, state: St<'_>, path: String) -> AppResult<Value> {
    let st = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let (convs, projects) = load_export(Path::new(&path))?;
        if convs.is_empty() && projects.is_empty() {
            return Err(AppError::msg("No conversations or projects were found in this file."));
        }
        let (map, projects_created) = import_ai_projects(&st, &projects)?;
        let mut done = st.db.imported_keys().unwrap_or_default();
        let (mut imported, mut skipped) = (0, 0);
        let mut errors = vec![];
        let total = convs.len();
        for (i, c) in convs.iter().enumerate() {
            if i % 10 == 0 {
                progress(&app, i, total);
            }
            match import_ai_conversation(&st, c, &map, &mut done) {
                Ok(true) => imported += 1,
                Ok(false) => skipped += 1,
                Err(e) => errors.push(json!({"title": c["name"], "error": e.to_string()})),
            }
        }
        progress(&app, total, total);
        Ok(json!({
            "imported": imported,
            "skipped": skipped,
            "projects": projects_created,
            "errors": errors,
        }))
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))?
}

#[tauri::command]
pub async fn pick_import_file(app: AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Claude data export", &["zip", "json"])
        .pick_file(move |f| {
            let _ = tx.send(f.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()));
        });
    Ok(rx.await.unwrap_or(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: Value) -> String {
        v.to_string() + "\n"
    }

    #[test]
    fn cc_session_rebuilds_main_thread() {
        let dir = std::env::temp_dir().join(format!("ccimp-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s1.jsonl");
        let mut s = String::new();
        s += &line(json!({"type":"mode","sessionId":"s1"}));
        s += &line(json!({"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s1","cwd":"/tmp/p","timestamp":"2026-01-02T03:04:05.000Z","message":{"role":"user","content":"fix the bug"}}));
        s += &line(json!({"type":"attachment","uuid":"at1","parentUuid":"u1","sessionId":"s1"}));
        s += &line(json!({"type":"assistant","uuid":"a1","parentUuid":"at1","sessionId":"s1","timestamp":"2026-01-02T03:04:06.000Z","message":{"id":"m1","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"Looking."}]}}));
        s += &line(json!({"type":"assistant","uuid":"a2","parentUuid":"a1","sessionId":"s1","timestamp":"2026-01-02T03:04:07.000Z","message":{"id":"m1","role":"assistant","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/tmp/p/a"}}]}}));
        s += &line(json!({"type":"user","uuid":"r1","parentUuid":"a2","sessionId":"s1","timestamp":"2026-01-02T03:04:08.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}}));
        // a sidechain (subagent) entry that must be ignored
        s += &line(json!({"type":"assistant","uuid":"sc","parentUuid":"r1","isSidechain":true,"sessionId":"s1","message":{"id":"mx","role":"assistant","content":[{"type":"text","text":"side"}]}}));
        s += &line(json!({"type":"assistant","uuid":"a3","parentUuid":"r1","sessionId":"s1","timestamp":"2026-01-02T03:04:09.000Z","message":{"id":"m2","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"Fixed."}]}}));
        s += &line(json!({"type":"user","uuid":"c1","parentUuid":"a3","sessionId":"s1","isMeta":true,"message":{"role":"user","content":"Caveat: ..."}}));
        s += &line(json!({"type":"user","uuid":"c2","parentUuid":"c1","sessionId":"s1","timestamp":"2026-01-02T03:05:00.000Z","message":{"role":"user","content":"<command-name>/review</command-name>\n<command-args>now</command-args>"}}));
        s += &line(json!({"type":"custom-title","customTitle":"Bug fix","sessionId":"s1"}));
        std::fs::write(&f, s).unwrap();

        let p = parse_cc_session(&f).unwrap();
        assert_eq!(p.session_id, "s1");
        assert_eq!(p.title, "Bug fix");
        assert_eq!(p.cwd.as_deref(), Some("/tmp/p"));
        assert_eq!(p.model.as_deref(), Some("claude-opus-5-5"));
        let roles: Vec<_> = p.rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant", "user"]);
        assert_eq!(p.rows[1].1.len(), 2, "blocks of one API message are merged");
        assert_eq!(p.rows[1].2["cc_uuid"], "a2");
        assert_eq!(p.rows[2].2["tool_results"], true);
        assert_eq!(p.rows[1].2["stop_reason"], "tool_use");
        assert_eq!(p.rows[3].2["stop_reason"], "end_turn");
        assert_eq!(text_of(&p.rows[4].1), "/review now");

        let info = scan_cc_session(&f).unwrap();
        assert_eq!(info["title"], "Bug fix");
        assert_eq!(info["prompts"], 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn artifacts_are_rebuilt_with_updates() {
        let mut st = ArtifactState::new();
        let m = json!({"content": [
            {"type":"text","text":"Here you go."},
            {"type":"tool_use","name":"artifacts","input":{"id":"app","command":"create","type":"text/html","title":"My \"App\"","content":"<p>hi</p>"}},
            {"type":"tool_result","name":"artifacts","content":[]},
            {"type":"text","text":"Done."}
        ]});
        let out = convert_assistant(&m, &mut st);
        assert_eq!(out.len(), 1);
        let t = out[0]["text"].as_str().unwrap();
        assert!(t.starts_with("Here you go.\n\n<antArtifact identifier=\"app\" type=\"text/html\" title=\"My &quot;App&quot;\">"));
        assert!(t.ends_with("</antArtifact>\n\nDone."));
        let m2 = json!({"content": [
            {"type":"tool_use","name":"artifacts","input":{"id":"app","command":"update","old_str":"hi","new_str":"bye"}}
        ]});
        let out2 = convert_assistant(&m2, &mut st);
        let t2 = out2[0]["text"].as_str().unwrap();
        assert!(t2.contains("<p>bye</p>") && t2.contains("type=\"text/html\""));
        let segs = crate::artifacts::split(t);
        assert!(segs.iter().any(|s| matches!(s, crate::artifacts::Segment::Artifact(a) if a.title == "My \"App\"" && a.content.contains("<p>hi</p>"))));
    }

    #[test]
    fn export_formats() {
        let dir = std::env::temp_dir().join(format!("aiexp-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conv = json!([{"uuid":"c1","name":"Hello","created_at":"2025-03-01T10:00:00.123456Z","updated_at":"2025-03-01T10:05:00Z",
            "chat_messages":[
                {"uuid":"m1","sender":"human","text":"hi","content":[{"type":"text","text":"hi"}],"created_at":"2025-03-01T10:00:00Z",
                 "attachments":[{"file_name":"a.txt","file_size":3,"extracted_content":"abc"}],"files":[{"file_name":"pic.png"}]},
                {"uuid":"m2","sender":"assistant","text":"hello","content":[{"type":"text","text":"hello"}],"created_at":"2025-03-01T10:00:01Z"}
            ]}]);
        let zpath = dir.join("export.zip");
        {
            use std::io::Write;
            let f = std::fs::File::create(&zpath).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("data-2025/conversations.json", o).unwrap();
            z.write_all(conv.to_string().as_bytes()).unwrap();
            z.start_file("data-2025/projects.json", o).unwrap();
            z.write_all(json!([{"uuid":"p1","name":"P","docs":[]}]).to_string().as_bytes()).unwrap();
            z.finish().unwrap();
        }
        let (c, p) = load_export(&zpath).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(p.len(), 1);
        let (content, meta) = convert_human(&c[0]["chat_messages"][0]);
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "document");
        assert_eq!(meta["attachments"].as_array().unwrap().len(), 2);
        assert_eq!(ts_ms(&json!("2025-03-01T10:00:00.123456Z")), Some(1740823200123));
        std::fs::remove_dir_all(&dir).ok();
    }
}
