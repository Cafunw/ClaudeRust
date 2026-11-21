//! Tauri commands invoked by the UI.

use crate::attachments;
use crate::db::{Message, ProjectFile};
use crate::engine;
use crate::error::{AppError, AppResult};
use crate::mcp;
use crate::models;
use crate::render;
use crate::settings;
use crate::state::{AppState, PermissionReply};
use crate::util::{new_id, now_ms};
use crate::view;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

type St<'a> = State<'a, Arc<AppState>>;

fn settings_json(st: &AppState) -> Value {
    let s = st.settings.read().clone();
    let key = st.api_key.read().clone();
    let mut v = serde_json::to_value(&s).unwrap_or(json!({}));
    v["has_api_key"] = json!(key.is_some());
    v["api_key_masked"] = json!(key.as_deref().map(settings::mask_key));
    v
}

#[tauri::command]
pub async fn bootstrap(app: AppHandle, state: St<'_>) -> AppResult<Value> {
    let st = state.inner().clone();
    Ok(json!({
        "settings": settings_json(&st),
        "models": models_view(&st).await,
        "platform": std::env::consts::OS,
        "version": env!("CARGO_PKG_VERSION"),
        "highlight_css": crate::highlight::theme_css(),
        "data_dir": st.data_dir.display().to_string(),
        "mcp_config_path": st.mcp.config_path.display().to_string(),
        "home": dirs::home_dir().map(|h| h.display().to_string()),
        "window": app.get_webview_window("main").map(|_| "main"),
        "render": crate::display::info(),
    }))
}

#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

// ---------------------------------------------------------------- settings

#[tauri::command]
pub async fn save_settings(app: AppHandle, state: St<'_>, patch: Value) -> AppResult<Value> {
    let st = state.inner().clone();
    let (old, new) = {
        let cur = st.settings.read().clone();
        let new = cur.patched(&patch)?;
        (cur, new)
    };
    new.save(&st.db)?;
    *st.settings.write() = new.clone();
    if old.base_url != new.base_url {
        st.degraded.store(false, std::sync::atomic::Ordering::Relaxed);
        *st.models.lock() = None;
    }
    if old.quick_entry_shortcut != new.quick_entry_shortcut || old.quick_entry_enabled != new.quick_entry_enabled {
        crate::register_shortcut(&app, &new);
    }
    if old.launch_at_login != new.launch_at_login {
        use tauri_plugin_autostart::ManagerExt;
        let m = app.autolaunch();
        let _ = if new.launch_at_login { m.enable() } else { m.disable() };
    }
    if old.hardware_acceleration != new.hardware_acceleration {
        let _ = crate::display::write_mode(&new.hardware_acceleration);
    }
    if old.disabled_mcp_servers != new.disabled_mcp_servers {
        reload_mcp_in_background(&app, &st);
    }
    Ok(settings_json(&st))
}

#[tauri::command]
pub async fn set_api_key(state: St<'_>, key: Option<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    let key = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    settings::store_api_key(&st.db, key.as_deref())?;
    *st.api_key.write() = key;
    *st.models.lock() = None;
    Ok(settings_json(&st))
}

#[tauri::command]
pub async fn test_connection(state: St<'_>) -> AppResult<Value> {
    let st = state.inner().clone();
    let client = st.client()?;
    let list = client.list_models().await?;
    *st.models.lock() = Some(list.clone());
    Ok(json!({"ok": true, "models": list.len()}))
}

async fn models_view(st: &AppState) -> Value {
    let api: Option<Vec<Value>> = st.models.lock().clone();
    let mut out: Vec<Value> = vec![];
    let catalog = models::catalog();
    let available: Option<Vec<String>> = api
        .as_ref()
        .map(|l| l.iter().filter_map(|m| m["id"].as_str().map(|s| s.to_string())).collect());
    for m in &catalog {
        let avail = available.as_ref().map(|a| a.contains(&m.id)).unwrap_or(true);
        let mut v = serde_json::to_value(m).unwrap_or(json!({}));
        v["available"] = json!(avail);
        out.push(v);
    }
    for id in st.oai_models.lock().iter() {
        out.push(json!({
            "id": format!("{}{id}", crate::oai::PREFIX), "name": id, "description": "OpenAI-compatible endpoint",
            "thinking": "none", "efforts": [], "featured": true, "available": true, "provider": "oai",
        }));
    }
    if let Some(list) = &api {
        for m in list {
            let id = m["id"].as_str().unwrap_or("");
            if id.is_empty() || catalog.iter().any(|c| c.id == id) {
                continue;
            }
            // skip dated aliases of catalog models
            if catalog.iter().any(|c| id.starts_with(&c.id)) {
                continue;
            }
            let mut info = models::info(id);
            if let Some(dn) = m["display_name"].as_str() {
                info.name = dn.trim_start_matches("Claude ").to_string();
            }
            let mut v = serde_json::to_value(&info).unwrap_or(json!({}));
            v["available"] = json!(true);
            out.push(v);
        }
    }
    Value::Array(out)
}

#[tauri::command]
pub async fn get_models(state: St<'_>, refresh: bool) -> AppResult<Value> {
    let st = state.inner().clone();
    if refresh || st.models.lock().is_none() {
        if let Ok(client) = st.client() {
            if let Ok(list) = client.list_models().await {
                *st.models.lock() = Some(list);
            }
        }
    }
    Ok(models_view(&st).await)
}

// ---------------------------------------------------------- conversations

#[tauri::command]
pub async fn list_conversations(state: St<'_>, kind: Option<String>, project_id: Option<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    let list = st.db.conv_list(kind.as_deref(), project_id.as_deref())?;
    let running: Vec<String> = st.runs.lock().keys().cloned().collect();
    let out: Vec<Value> = list
        .into_iter()
        .map(|c| {
            json!({
                "id": c.id, "kind": c.kind, "title": c.title, "model": c.model,
                "project_id": c.project_id, "cwd": c.cwd, "starred": c.starred,
                "created_at": c.created_at, "updated_at": c.updated_at,
                "running": running.contains(&c.id),
                "engine": c.options["engine"],
            })
        })
        .collect();
    Ok(Value::Array(out))
}

#[derive(Deserialize)]
pub struct NewConversation {
    pub kind: Option<String>,
    pub model: Option<String>,
    pub project_id: Option<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub options: Value,
}

fn create_conversation_inner(st: &AppState, req: NewConversation) -> AppResult<crate::db::Conversation> {
    let s = st.settings.read().clone();
    let kind = req.kind.unwrap_or_else(|| "chat".into());
    let model = req.model.filter(|m| !m.is_empty()).unwrap_or_else(|| {
        if kind == "code" {
            s.code_model.clone()
        } else {
            s.default_model.clone()
        }
    });
    let mut options = if req.options.is_object() { req.options } else { json!({}) };
    options["date"] = json!(engine::today());
    let mut code_cwd: Option<String> = None;
    if kind == "code" {
        if options["engine"].as_str().map(|e| e.is_empty() || e == "auto").unwrap_or(true) {
            let engine = match s.code_engine.as_str() {
                "builtin" => "builtin",
                "claude-code" => crate::claudecode::ENGINE,
                _ => {
                    if crate::claudecode::find_cli(&s.claude_code_path).is_some() {
                        crate::claudecode::ENGINE
                    } else {
                        "builtin"
                    }
                }
            };
            options["engine"] = json!(engine);
        }
        let mut cwd = req.cwd.clone().ok_or_else(|| AppError::msg("Choose a folder for the coding session."))?;
        let mut p = std::path::PathBuf::from(&cwd);
        if !p.is_dir() {
            return Err(AppError::msg(format!("Folder not found: {cwd}")));
        }
        // Isolated worktree: asked for explicitly, or by the setting
        // (always / only when another session in the repo is running).
        let want = match options["worktree"].as_bool() {
            Some(b) => b,
            None => match s.code_worktree.as_str() {
                "always" => crate::worktree::repo_root(&p).is_some(),
                "parallel" => match crate::worktree::repo_root(&p) {
                    Some(root) => {
                        let running: Vec<String> = st.runs.lock().keys().cloned().collect();
                        running.iter().filter_map(|id| st.db.conv_get(id).ok()).any(|c| {
                            c.kind == "code" && c.cwd.as_deref().map(|d| std::path::Path::new(d).starts_with(&root)).unwrap_or(false)
                        })
                    }
                    None => false,
                },
                _ => false,
            },
        };
        if let Some(m) = options.as_object_mut() {
            m.remove("worktree");
        }
        if want {
            let (wt_cwd, info) = crate::worktree::create(&p, &st.data_dir, &new_id())?;
            options["worktree"] = info;
            cwd = wt_cwd.to_string_lossy().to_string();
            p = wt_cwd;
        }
        options["env_snapshot"] = json!(crate::codetools::env_snapshot(&p, &model));
        options["memory"] = json!(crate::codetools::load_memory(&p));
        if options["permission_mode"].is_null() {
            options["permission_mode"] = json!(s.code_permission_mode);
        }
        code_cwd = Some(cwd);
    }
    st.db.conv_create(&kind, &model, req.project_id.as_deref(), code_cwd.as_deref().or(req.cwd.as_deref()), &options)
}

#[tauri::command]
pub async fn create_conversation(state: St<'_>, req: NewConversation) -> AppResult<Value> {
    let st = state.inner().clone();
    let c = create_conversation_inner(&st, req)?;
    Ok(serde_json::to_value(c)?)
}

#[tauri::command]
pub async fn get_conversation(state: St<'_>, id: String) -> AppResult<Value> {
    let st = state.inner().clone();
    view::conversation_view(&st, &id).await
}

#[derive(Deserialize)]
pub struct SendReq {
    pub conversation_id: Option<String>,
    pub new_conversation: Option<NewConversation>,
    pub text: String,
    #[serde(default)]
    pub attachment_ids: Vec<String>,
    /// For edits: the parent of the message being edited ("" = root).
    pub parent_id: Option<String>,
    #[serde(default)]
    pub edit: bool,
}

#[tauri::command]
pub async fn send_message(app: AppHandle, state: St<'_>, req: SendReq) -> AppResult<Value> {
    send_message_inner(&app, state.inner().clone(), req).await
}

pub async fn send_message_inner(app: &AppHandle, st: Arc<AppState>, req: SendReq) -> AppResult<Value> {
    let app = app.clone();
    let conv = match (&req.conversation_id, req.new_conversation) {
        (Some(id), _) => st.db.conv_get(id)?,
        (None, Some(nc)) => {
            let c = create_conversation_inner(&st, nc)?;
            engine::emit_conv_updated(&app, &c.id);
            c
        }
        (None, None) => create_conversation_inner(
            &st,
            NewConversation {
                kind: None,
                model: None,
                project_id: None,
                cwd: None,
                options: json!({}),
            },
        )?,
    };
    if st.runs.lock().contains_key(&conv.id) {
        return Err(AppError::msg("Claude is still responding. Stop the response or wait for it to finish."));
    }
    let atts: Vec<attachments::Attachment> = {
        let mut staged = st.staged.lock();
        req.attachment_ids.iter().filter_map(|id| staged.remove(id)).collect()
    };
    if req.text.trim().is_empty() && atts.is_empty() {
        return Err(AppError::msg("Message is empty."));
    }
    let mut content: Vec<Value> = atts.iter().map(attachments::to_block).collect();
    if !req.text.trim().is_empty() {
        content.push(json!({"type": "text", "text": req.text}));
    }
    // @mentions: include the files' contents (the Claude Code CLI expands
    // them itself).
    // Hooks (built-in agent): SessionStart on the first message, then
    // UserPromptSubmit, which can add context or block the message.
    if conv.kind == "code" && conv.options["engine"] != crate::claudecode::ENGINE {
        if let Some(cwd) = &conv.cwd {
            let root = std::path::PathBuf::from(cwd);
            let hooks = engine::hook_config(&st, &root);
            if let Some((r, hash, cmds)) = &hooks.untrusted {
                let _ = app.emit("hooks-untrusted", json!({"conversation_id": conv.id, "root": r, "hash": hash, "commands": cmds}));
            }
            let mut extra: Vec<String> = vec![];
            if conv.current_leaf.is_none() {
                let o = crate::hooks::run(&hooks, "SessionStart", "", json!({"session_id": conv.id, "source": "startup"}), &root).await;
                extra.extend(o.context);
            }
            let o = crate::hooks::run(&hooks, "UserPromptSubmit", "", json!({"session_id": conv.id, "prompt": req.text}), &root).await;
            if let Some(reason) = o.block {
                return Err(AppError::msg(format!("A UserPromptSubmit hook blocked this message: {reason}")));
            }
            extra.extend(o.context);
            for c in extra {
                content.push(json!({"type": "text", "text": format!("<system-reminder>{c}</system-reminder>")}));
            }
        }
    }
    // @https://… mentions: fetch the page (any conversation using the API
    // engines; the Claude Code CLI fetches by itself).
    if conv.options["engine"] != crate::claudecode::ENGINE {
        for u in crate::filesearch::url_mentions(&req.text) {
            if let Some(b) = crate::filesearch::fetch_url_block(&u).await {
                content.push(b);
            }
        }
    }
    // Pick up edits to CLAUDE.md / AGENTS.md since the session started.
    if conv.kind == "code" && conv.options["engine"] != crate::claudecode::ENGINE {
        if let Some(cwd) = &conv.cwd {
            let mem = crate::memory::load(std::path::Path::new(cwd));
            if conv.options["memory"].as_str() != Some(mem.as_str()) {
                engine::update_conv_options(&st, &conv.id, |o| o["memory"] = json!(mem));
            }
        }
    }
    if conv.kind == "code" && conv.options["engine"] != crate::claudecode::ENGINE && req.text.contains('@') {
        if let Some(cwd) = &conv.cwd {
            let session = st.code_session(&conv.id, cwd);
            content.extend(crate::filesearch::expand(&req.text, &session));
        }
    }
    let plan = conv.kind == "code"
        && conv.options["permission_mode"] == "plan"
        && conv.options["engine"] != crate::claudecode::ENGINE;
    if plan {
        content.push(json!({"type": "text", "text": crate::prompts::PLAN_MODE_REMINDER}));
    }
    let parent = if req.edit {
        req.parent_id.clone().filter(|p| !p.is_empty())
    } else {
        conv.current_leaf.clone()
    };
    let mut meta = json!({
        "attachments": atts.iter().map(attachments::display_meta).collect::<Vec<_>>(),
        "plan_mode": plan,
    });
    // A /compact summary made at this point of the conversation applies to
    // the message that follows it.
    if let Some(pc) = conv.options["pending_compact"].as_object() {
        if pc.get("upto").and_then(|v| v.as_str()) == parent.as_deref() {
            meta["compact_summary"] = pc.get("summary").cloned().unwrap_or(Value::Null);
        }
        engine::update_conv_options(&st, &conv.id, |o| {
            if let Some(m) = o.as_object_mut() {
                m.remove("pending_compact");
            }
        });
    }
    let msg = Message {
        id: new_id(),
        conversation_id: conv.id.clone(),
        parent_id: parent,
        role: "user".into(),
        content: Value::Array(content),
        meta,
        created_at: now_ms(),
    };
    st.db.msg_insert(&msg)?;
    st.db.conv_set_leaf(&conv.id, Some(&msg.id))?;
    let _ = st.db.fts_index(&conv.id, &msg.id, &req.text);
    engine::start(&app, &st, &conv.id, &msg.id)?;
    Ok(json!({"conversation_id": conv.id, "message_id": msg.id}))
}

#[tauri::command]
pub async fn regenerate(app: AppHandle, state: St<'_>, conversation_id: String, user_message_id: String, model: Option<String>) -> AppResult<()> {
    let st = state.inner().clone();
    if st.runs.lock().contains_key(&conversation_id) {
        return Err(AppError::msg("Claude is still responding. Stop the response or wait for it to finish."));
    }
    let msg = st.db.msg_get(&user_message_id)?;
    if msg.conversation_id != conversation_id {
        return Err(AppError::msg("Message not found in this conversation."));
    }
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        st.db.conv_set_model(&conversation_id, &m)?;
    }
    if msg.role != "user" {
        return Err(AppError::msg("Can only regenerate from a user message."));
    }
    st.db.conv_set_leaf(&conversation_id, Some(&user_message_id))?;
    engine::start(&app, &st, &conversation_id, &user_message_id)
}

#[tauri::command]
pub async fn switch_branch(state: St<'_>, conversation_id: String, message_id: String) -> AppResult<Value> {
    let st = state.inner().clone();
    if st.runs.lock().contains_key(&conversation_id) {
        return Err(AppError::msg("Wait for the response to finish before switching versions."));
    }
    let nodes = st.db.tree_nodes(&conversation_id)?;
    if !nodes.iter().any(|n| n.id == message_id) {
        return Err(AppError::msg("Message not found in this conversation."));
    }
    let leaf = view::latest_leaf(&nodes, &message_id);
    st.db.conv_set_leaf(&conversation_id, Some(&leaf))?;
    view::conversation_view(&st, &conversation_id).await
}

#[tauri::command]
pub async fn stop_generation(state: St<'_>, conversation_id: String) -> AppResult<()> {
    engine::stop(&state, &conversation_id);
    Ok(())
}

#[derive(Deserialize)]
pub struct ConvUpdate {
    pub id: String,
    pub title: Option<String>,
    pub starred: Option<bool>,
    pub model: Option<String>,
    /// Missing = unchanged; `null` = remove from project.
    #[serde(default, deserialize_with = "double_option")]
    pub project_id: Option<Option<String>>,
    pub options: Option<Value>,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[tauri::command]
pub async fn update_conversation(app: AppHandle, state: St<'_>, req: ConvUpdate) -> AppResult<Value> {
    let st = state.inner().clone();
    if let Some(t) = &req.title {
        st.db.conv_set_title(&req.id, t.trim())?;
    }
    if let Some(s) = req.starred {
        st.db.conv_set_starred(&req.id, s)?;
    }
    if let Some(m) = &req.model {
        st.db.conv_set_model(&req.id, m)?;
    }
    if let Some(p) = &req.project_id {
        st.db.conv_set_project(&req.id, p.as_deref())?;
    }
    if let Some(patch) = req.options.as_ref().and_then(|p| p.as_object()) {
        let c = st.db.conv_get(&req.id)?;
        let mut o = if c.options.is_object() { c.options } else { json!({}) };
        for (k, v) in patch {
            o[k] = v.clone();
        }
        st.db.conv_set_options(&req.id, &o)?;
    }
    engine::emit_conv_updated(&app, &req.id);
    Ok(serde_json::to_value(st.db.conv_get(&req.id)?)?)
}

#[tauri::command]
pub async fn delete_conversation(state: St<'_>, id: String) -> AppResult<()> {
    let st = state.inner().clone();
    engine::stop(&st, &id);
    let worktree = st.db.conv_get(&id).ok().map(|c| c.options["worktree"].clone()).filter(|w| w.is_object());
    st.db.conv_delete(&id)?;
    st.code_sessions.lock().remove(&id);
    st.shells.kill_conversation(&id);
    st.subagents.forget_conversation(&id);
    st.watches.stop(&id);
    st.terminals.close_conversation(&id);
    st.live.lock().remove(&id);
    // Prompts still waiting for this conversation: drop them (the run was
    // cancelled above, so nothing will answer them).
    let stale: Vec<String> = st
        .pending_perms
        .lock()
        .iter()
        .filter(|(_, p)| p["conversation_id"] == id.as_str())
        .map(|(k, _)| k.clone())
        .collect();
    for k in stale {
        st.pending_perms.lock().remove(&k);
        st.permissions.lock().remove(&k);
    }
    if let Some(info) = worktree {
        // after the shells and terminals using it have stopped
        tauri::async_runtime::spawn_blocking(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            if let Err(e) = crate::worktree::remove(&info) {
                log::warn!("removing worktree failed: {e}");
            }
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn message_feedback(state: St<'_>, message_id: String, value: Option<String>) -> AppResult<()> {
    let st = state.inner().clone();
    let m = st.db.msg_get(&message_id)?;
    let mut meta = m.meta;
    meta["feedback"] = json!(value);
    st.db.msg_update(&message_id, &m.content, &meta)?;
    Ok(())
}

#[tauri::command]
pub async fn export_conversation(state: St<'_>, id: String, format: String) -> AppResult<String> {
    let st = state.inner().clone();
    let conv = st.db.conv_get(&id)?;
    let path = match &conv.current_leaf {
        Some(l) => st.db.path_to(l)?,
        None => vec![],
    };
    if format == "json" {
        return Ok(serde_json::to_string_pretty(&json!({"conversation": conv, "messages": path}))?);
    }
    if format == "html" {
        return Ok(export_html(&st, &conv, &path).await);
    }
    let mut md = format!("# {}\n\n", if conv.title.is_empty() { "Conversation" } else { &conv.title });
    for m in &path {
        if render::is_tool_result_msg(m) {
            continue;
        }
        let text = render::plain_text(&m.content);
        if text.trim().is_empty() {
            continue;
        }
        md.push_str(if m.role == "user" { "## You\n\n" } else { "## Claude\n\n" });
        md.push_str(text.trim());
        md.push_str("\n\n");
    }
    Ok(md)
}

#[tauri::command]
pub async fn search(state: St<'_>, query: String) -> AppResult<Value> {
    let st = state.inner().clone();
    Ok(serde_json::to_value(st.db.search(&query, 40)?)?)
}

// -------------------------------------------------------------- attachments

#[tauri::command]
pub async fn stage_paths(state: St<'_>, paths: Vec<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    let mut out = vec![];
    let mut errors = vec![];
    for p in paths {
        match attachments::from_path(std::path::Path::new(&p)) {
            Ok(a) => {
                out.push(serde_json::to_value(&a)?);
                st.staged.lock().insert(a.id.clone(), a);
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    Ok(json!({"attachments": out, "errors": errors}))
}

#[tauri::command]
pub async fn stage_bytes(state: St<'_>, name: String, data: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let a = attachments::from_base64(&name, &data)?;
    let v = serde_json::to_value(&a)?;
    st.staged.lock().insert(a.id.clone(), a);
    Ok(v)
}

#[tauri::command]
pub async fn stage_text(state: St<'_>, name: String, text: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let a = attachments::from_bytes(&name, text.as_bytes())?;
    let v = serde_json::to_value(&a)?;
    st.staged.lock().insert(a.id.clone(), a);
    Ok(v)
}

#[tauri::command]
pub async fn unstage(state: St<'_>, id: String) -> AppResult<()> {
    state.staged.lock().remove(&id);
    Ok(())
}

#[tauri::command]
pub async fn pick_files(app: AppHandle) -> AppResult<Vec<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_files(move |files| {
        let paths: Vec<String> = files
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.into_path().ok().map(|p| p.display().to_string()))
            .collect();
        let _ = tx.send(paths);
    });
    Ok(rx.await.unwrap_or_default())
}

#[tauri::command]
pub async fn pick_directory(app: AppHandle) -> AppResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |f| {
        let _ = tx.send(f.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()));
    });
    Ok(rx.await.unwrap_or(None))
}

#[tauri::command]
pub async fn save_text_file(app: AppHandle, name: String, content: String) -> AppResult<bool> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_file_name(&name).save_file(move |p| {
        let _ = tx.send(p.and_then(|p| p.into_path().ok()));
    });
    match rx.await.unwrap_or(None) {
        Some(p) => {
            std::fs::write(p, content)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

// -------------------------------------------------------------- permissions

#[tauri::command]
pub async fn respond_permission(state: St<'_>, request_id: String, decision: String, message: Option<String>) -> AppResult<()> {
    let tx = state.permissions.lock().remove(&request_id);
    match tx {
        Some(tx) => {
            let _ = tx.send(PermissionReply { decision, message });
            Ok(())
        }
        None => Err(AppError::msg("This request is no longer pending.")),
    }
}

// ------------------------------------------------------------------ projects

#[tauri::command]
pub async fn list_projects(state: St<'_>) -> AppResult<Value> {
    let st = state.inner().clone();
    let projects = st.db.project_list()?;
    let convs = st.db.conv_list(Some("chat"), None)?;
    let out: Vec<Value> = projects
        .into_iter()
        .map(|p| {
            let n = convs.iter().filter(|c| c.project_id.as_deref() == Some(&p.id)).count();
            let mut v = serde_json::to_value(&p).unwrap_or(json!({}));
            v["conversation_count"] = json!(n);
            v
        })
        .collect();
    Ok(Value::Array(out))
}

#[tauri::command]
pub async fn create_project(state: St<'_>, name: String, description: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::msg("Project name is required."));
    }
    Ok(serde_json::to_value(st.db.project_create(name, description.trim())?)?)
}

#[tauri::command]
pub async fn get_project(state: St<'_>, id: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let p = st.db.project_get(&id)?;
    let files = st.db.project_files(&id)?;
    let total_chars: usize = files.iter().filter(|f| f.kind == "text").map(|f| f.data.len()).sum();
    Ok(json!({"project": p, "files": files, "knowledge_chars": total_chars}))
}

#[derive(Deserialize)]
pub struct ProjectUpdate {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub instructions: Option<String>,
    pub starred: Option<bool>,
}

#[tauri::command]
pub async fn update_project(state: St<'_>, req: ProjectUpdate) -> AppResult<Value> {
    let st = state.inner().clone();
    st.db.project_update(
        &req.id,
        req.name.as_deref(),
        req.description.as_deref(),
        req.instructions.as_deref(),
        req.starred,
    )?;
    Ok(serde_json::to_value(st.db.project_get(&req.id)?)?)
}

#[tauri::command]
pub async fn delete_project(state: St<'_>, id: String) -> AppResult<()> {
    state.db.project_delete(&id)
}

fn add_file_to_project(st: &AppState, project_id: &str, a: attachments::Attachment) -> AppResult<Value> {
    let f = ProjectFile {
        id: new_id(),
        project_id: project_id.to_string(),
        name: a.name,
        mime: a.mime,
        size: a.size as i64,
        kind: a.kind,
        data: a.data,
        created_at: now_ms(),
    };
    st.db.project_file_add(&f)?;
    Ok(serde_json::to_value(&f)?)
}

#[tauri::command]
pub async fn add_project_paths(state: St<'_>, project_id: String, paths: Vec<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    let mut added = vec![];
    let mut errors = vec![];
    for p in paths {
        match attachments::from_path(std::path::Path::new(&p)).and_then(|a| add_file_to_project(&st, &project_id, a)) {
            Ok(v) => added.push(v),
            Err(e) => errors.push(e.to_string()),
        }
    }
    Ok(json!({"added": added, "errors": errors}))
}

#[tauri::command]
pub async fn add_project_text(state: St<'_>, project_id: String, name: String, text: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let mut a = attachments::from_bytes(&name, text.as_bytes())?;
    a.kind = "text".into();
    add_file_to_project(&st, &project_id, a)
}

#[tauri::command]
pub async fn add_project_bytes(state: St<'_>, project_id: String, name: String, data: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let a = attachments::from_base64(&name, &data)?;
    add_file_to_project(&st, &project_id, a)
}

#[tauri::command]
pub async fn remove_project_file(state: St<'_>, id: String) -> AppResult<()> {
    state.db.project_file_delete(&id)
}

// ------------------------------------------------------------------ artifacts

#[tauri::command]
pub async fn list_artifacts(state: St<'_>) -> AppResult<Value> {
    let st = state.inner().clone();
    let rows = st.db.artifact_list(500)?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|a| {
            let label = crate::artifacts::kind_label(&a.kind, a.language.as_deref());
            let mut v = serde_json::to_value(&a).unwrap_or(json!({}));
            v["label"] = json!(label);
            v["previewable"] = json!(crate::artifacts::is_previewable(&a.kind));
            v["ext"] = json!(crate::artifacts::extension(&a.kind, a.language.as_deref()));
            v
        })
        .collect();
    Ok(Value::Array(out))
}

#[tauri::command]
pub async fn delete_artifact(state: St<'_>, conversation_id: String, identifier: String) -> AppResult<()> {
    state.db.artifact_delete(&conversation_id, &identifier)
}

/// Register an artifact document and return the token to load it from the
/// `artifact://` protocol.
#[tauri::command]
pub async fn serve_artifact(state: St<'_>, kind: String, content: String, dark: bool) -> AppResult<String> {
    let st = state.inner().clone();
    let doc = crate::artifacts::build_document(&kind, &content, dark);
    let token = new_id();
    let mut served = st.served.lock();
    if served.len() > 64 {
        served.clear();
    }
    served.insert(token.clone(), ("text/html; charset=utf-8".into(), doc.into_bytes()));
    Ok(token)
}

#[tauri::command]
pub async fn render_markdown(state: St<'_>, text: String) -> AppResult<String> {
    let math = state.settings.read().latex_enabled;
    Ok(crate::markdown::render(&text, crate::markdown::Opts { math, breaks: false }))
}

#[tauri::command]
pub async fn highlight_code(code: String, language: String) -> AppResult<String> {
    Ok(crate::highlight::highlight(&code, &language))
}

// ----------------------------------------------------------------------- MCP

#[tauri::command]
pub async fn mcp_status(state: St<'_>) -> AppResult<Value> {
    let st = state.inner().clone();
    let statuses = st.mcp.statuses().await;
    Ok(json!({
        "servers": statuses,
        "config_path": st.mcp.config_path.display().to_string(),
        "config": mcp::read_config(&st.mcp.config_path),
        "claude_desktop_config": mcp::claude_desktop_config_path().filter(|p| p.exists()).map(|p| p.display().to_string()),
    }))
}

#[tauri::command]
pub async fn mcp_reload(app: AppHandle, state: St<'_>) -> AppResult<Value> {
    let task = reload_mcp_in_background(&app, state.inner());
    // Give quick servers a moment so the caller sees them connected; slow
    // ones keep starting in the background and announce "mcp-changed".
    let _ = tokio::time::timeout(std::time::Duration::from_secs(6), task).await;
    mcp_status(state).await
}

/// Restart all MCP servers without waiting; emits "mcp-changed" when done.
pub fn reload_mcp_in_background(app: &AppHandle, st: &Arc<AppState>) -> tauri::async_runtime::JoinHandle<()> {
    let st = st.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let disabled = st.settings.read().disabled_mcp_servers.clone();
        st.mcp.start_all(&disabled).await;
        use tauri::Emitter;
        let _ = app.emit("mcp-changed", json!({}));
    })
}

#[tauri::command]
pub async fn mcp_save_config(app: AppHandle, state: St<'_>, config: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let v: Value = serde_json::from_str(&config).map_err(|e| AppError::msg(format!("Invalid JSON: {e}")))?;
    if !v["mcpServers"].is_object() {
        return Err(AppError::msg("The config needs an \"mcpServers\" object."));
    }
    mcp::write_config(&st.mcp.config_path, &v)?;
    mcp_reload(app, state).await
}

#[tauri::command]
pub async fn mcp_list_prompts(state: St<'_>) -> AppResult<Value> {
    let st = state.inner().clone();
    Ok(json!({"prompts": st.mcp.list_prompts().await, "resources": st.mcp.list_resources().await}))
}

#[tauri::command]
pub async fn mcp_get_prompt(state: St<'_>, server: String, name: String, args: Value) -> AppResult<Value> {
    let st = state.inner().clone();
    let text = st.mcp.get_prompt(&server, &name, &args).await?;
    let a = attachments::from_bytes(&format!("{name}.txt"), text.as_bytes())?;
    let v = serde_json::to_value(&a)?;
    st.staged.lock().insert(a.id.clone(), a);
    Ok(v)
}

#[tauri::command]
pub async fn mcp_read_resource(state: St<'_>, server: String, uri: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let (uri, text) = st.mcp.read_resource(&server, &uri).await?;
    let name = uri.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("resource").to_string();
    let a = attachments::from_bytes(&format!("{name}.txt"), text.as_bytes())?;
    let v = serde_json::to_value(&a)?;
    st.staged.lock().insert(a.id.clone(), a);
    Ok(v)
}

// ------------------------------------------------------------- Claude Code

#[tauri::command]
pub async fn claude_code_info(state: St<'_>, path: Option<String>) -> AppResult<Value> {
    let p = path.unwrap_or_else(|| state.settings.read().claude_code_path.clone());
    Ok(crate::claudecode::cli_info(&p).await)
}

#[tauri::command]
pub async fn claude_code_login(state: St<'_>) -> AppResult<()> {
    let p = state.settings.read().claude_code_path.clone();
    let cli = crate::claudecode::find_cli(&p)
        .ok_or_else(|| AppError::msg("Claude Code isn't installed. Install it from claude.com/claude-code first."))?;
    crate::claudecode::open_login_terminal(&cli)
}

// ---------------------------------------------------------------- extensions

#[tauri::command]
pub async fn pick_extension(app: AppHandle) -> AppResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Desktop Extension", &["mcpb", "dxt"])
        .pick_file(move |f| {
            let _ = tx.send(f.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()));
        });
    Ok(rx.await.unwrap_or(None))
}

#[tauri::command]
pub async fn extension_inspect(path: String) -> AppResult<Value> {
    crate::extensions::inspect(std::path::Path::new(&path))
}

#[tauri::command]
pub async fn extension_install(app: AppHandle, state: St<'_>, path: String, values: Value) -> AppResult<Value> {
    let st = state.inner().clone();
    let dir = st.data_dir.join("extensions");
    std::fs::create_dir_all(&dir)?;
    let name = crate::extensions::install(std::path::Path::new(&path), &dir, &st.mcp.config_path, &values)?;
    let mut status = mcp_reload(app, state).await?;
    status["installed"] = json!(name);
    Ok(status)
}

// ---------------------------------------------------------------------- misc

#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> AppResult<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")) {
        return Err(AppError::msg("Only web links can be opened."));
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url(url, None::<&str>).map_err(|e| AppError::msg(e.to_string()))
}

#[tauri::command]
pub async fn open_path(app: AppHandle, path: String) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(path, None::<&str>).map_err(|e| AppError::msg(e.to_string()))
}

#[tauri::command]
pub async fn reveal_path(app: AppHandle, path: String) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().reveal_item_in_dir(path).map_err(|e| AppError::msg(e.to_string()))
}

#[tauri::command]
pub async fn copy_text(app: AppHandle, text: String) -> AppResult<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| AppError::msg(e.to_string()))
}

#[tauri::command]
pub async fn set_zoom(app: AppHandle, state: St<'_>, zoom: f64) -> AppResult<()> {
    let z = zoom.clamp(0.5, 2.5);
    for w in app.webview_windows().values() {
        let _ = w.set_zoom(z);
    }
    let st = state.inner().clone();
    let mut s = st.settings.write();
    s.zoom = z;
    s.save(&st.db)
}

#[tauri::command]
pub async fn quick_entry_submit(app: AppHandle, text: String) -> AppResult<()> {
    crate::hide_quick_entry(&app);
    crate::show_main(&app);
    use tauri::Emitter;
    let _ = app.emit_to("main", "quick-entry", json!({"text": text}));
    Ok(())
}

#[tauri::command]
pub async fn quick_entry_hide(app: AppHandle) -> AppResult<()> {
    crate::hide_quick_entry(&app);
    Ok(())
}

#[tauri::command]
pub async fn code_info(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let conv = st.db.conv_get(&conversation_id)?;
    let cwd = conv.cwd.clone().unwrap_or_default();
    let session = st.code_session(&conversation_id, &cwd);
    let cur = session.cwd.lock().display().to_string();
    Ok(json!({
        "root": cwd,
        "cwd": cur,
        "permission_mode": conv.options["permission_mode"],
        "allowed_rules": conv.options["allowed_rules"],
        "has_memory": conv.options["memory"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
    }))
}

#[tauri::command]
pub async fn usage_stats(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let conv = st.db.conv_get(&conversation_id)?;
    let path = match &conv.current_leaf {
        Some(l) => st.db.path_to(l)?,
        None => vec![],
    };
    let mut tot = (0u64, 0u64, 0u64, 0u64, 0.0f64);
    // answers copied by a fork were paid for in the original conversation
    for m in path.iter().filter(|m| m.role == "assistant" && m.meta["forked_from"].is_null()) {
        let model = m.meta["model"].as_str().unwrap_or(&conv.model);
        for key in ["usage", "subagent_usage"] {
            let u = &m.meta[key];
            tot.0 += u["input_tokens"].as_u64().unwrap_or(0);
            tot.1 += u["output_tokens"].as_u64().unwrap_or(0);
            tot.2 += u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            tot.3 += u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            if u.is_object() && !model.starts_with(crate::oai::PREFIX) {
                tot.4 += usage_cost(model, u);
            }
        }
    }
    let start = crate::compact::start_index(&path).unwrap_or(0);
    let context = crate::compact::context_tokens(&path[start..]);
    let window = models::info(&conv.model).context;
    Ok(json!({"input_tokens": tot.0, "output_tokens": tot.1, "cache_read_input_tokens": tot.2, "cache_creation_input_tokens": tot.3, "cost_usd": tot.4,
              "context_tokens": context, "context_window": window, "auto_compact_pct": st.settings.read().auto_compact_pct,
              "pending_compact": conv.options["pending_compact"].is_object()}))
}

#[tauri::command]
pub async fn reset_onboarding(state: St<'_>) -> AppResult<()> {
    let st = state.inner().clone();
    let mut s = st.settings.write();
    s.onboarded = false;
    s.save(&st.db)
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_id_null_vs_missing() {
        let a: ConvUpdate = serde_json::from_value(json!({"id": "x"})).unwrap();
        assert!(a.project_id.is_none());
        let b: ConvUpdate = serde_json::from_value(json!({"id": "x", "project_id": null})).unwrap();
        assert_eq!(b.project_id, Some(None));
        let c: ConvUpdate = serde_json::from_value(json!({"id": "x", "project_id": "p"})).unwrap();
        assert_eq!(c.project_id, Some(Some("p".to_string())));
    }
}

// ------------------------------------------------------------- background shells

#[tauri::command]
pub async fn shells_list(state: St<'_>, conversation_id: Option<String>) -> AppResult<Value> {
    let list: Vec<Value> = state.shells.list(conversation_id.as_deref()).iter().map(|s| s.summary()).collect();
    Ok(json!(list))
}

#[tauri::command]
pub async fn shell_output(state: St<'_>, id: String) -> AppResult<Value> {
    let sh = state.shells.get(&id).ok_or_else(|| AppError::msg("This background command is gone."))?;
    let mut v = sh.summary();
    v["output"] = json!(sh.tail(200_000));
    Ok(v)
}

#[tauri::command]
pub async fn shell_kill(state: St<'_>, id: String) -> AppResult<bool> {
    Ok(state.shells.kill(&id))
}

#[tauri::command]
pub async fn shells_clear(state: St<'_>, conversation_id: String) -> AppResult<()> {
    state.shells.clear_finished(&conversation_id);
    Ok(())
}

// ------------------------------------------------------------- terminals

#[tauri::command]
pub async fn term_open(app: AppHandle, state: St<'_>, conversation_id: String, cwd: Option<String>, cols: u16, rows: u16) -> AppResult<Value> {
    let cwd = match cwd {
        Some(c) if !c.is_empty() => c,
        _ => state
            .db
            .conv_get(&conversation_id)
            .ok()
            .and_then(|c| c.cwd)
            .unwrap_or_default(),
    };
    state.terminals.open(&app, &conversation_id, &cwd, cols, rows)
}

#[tauri::command]
pub async fn term_write(state: St<'_>, id: String, data: String) -> AppResult<()> {
    state.terminals.write(&id, &data)
}

#[tauri::command]
pub async fn term_resize(state: St<'_>, id: String, cols: u16, rows: u16) -> AppResult<()> {
    state.terminals.resize(&id, cols, rows)
}

#[tauri::command]
pub async fn term_close(state: St<'_>, id: String) -> AppResult<()> {
    state.terminals.close(&id);
    Ok(())
}

#[tauri::command]
pub async fn term_list(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    Ok(json!(state.terminals.list(&conversation_id)))
}

// ------------------------------------------------------------- changes (git / session edits)

fn session_for(st: &AppState, conversation_id: &str) -> AppResult<Arc<crate::codetools::CodeSession>> {
    let conv = st.db.conv_get(conversation_id)?;
    let cwd = conv.cwd.ok_or_else(|| AppError::msg("This conversation has no project folder."))?;
    Ok(st.code_session(conversation_id, &cwd))
}

#[tauri::command]
pub async fn changes_list(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let s = session_for(&state, &conversation_id)?;
    crate::gitops::changes(&s).await
}

#[tauri::command]
pub async fn change_diff(state: St<'_>, conversation_id: String, path: String) -> AppResult<String> {
    let s = session_for(&state, &conversation_id)?;
    crate::gitops::file_diff(&s, &path).await
}

/// Undo one hunk of a file's uncommitted changes (`header` is the hunk's
/// `@@` line as shown, so a stale view can't revert the wrong lines).
#[tauri::command]
pub async fn change_revert_hunk(app: AppHandle, state: St<'_>, conversation_id: String, path: String, index: usize, header: String) -> AppResult<()> {
    let s = session_for(&state, &conversation_id)?;
    crate::gitops::revert_hunk(&s, &path, index, &header).await?;
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    Ok(())
}

#[tauri::command]
pub async fn change_revert(app: AppHandle, state: St<'_>, conversation_id: String, paths: Vec<String>) -> AppResult<()> {
    let s = session_for(&state, &conversation_id)?;
    for p in &paths {
        crate::gitops::revert_file(&s, p).await?;
    }
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    Ok(())
}

// ------------------------------------------------------------- @ mentions and / commands

fn root_of(st: &AppState, conversation_id: Option<&str>, cwd: Option<String>) -> Option<std::path::PathBuf> {
    cwd.filter(|c| !c.is_empty())
        .or_else(|| conversation_id.and_then(|id| st.db.conv_get(id).ok()).and_then(|c| c.cwd))
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
}

#[tauri::command]
pub async fn file_search(state: St<'_>, conversation_id: Option<String>, cwd: Option<String>, query: String) -> AppResult<Value> {
    let Some(root) = root_of(&state, conversation_id.as_deref(), cwd) else { return Ok(json!([])) };
    let r = tokio::task::spawn_blocking(move || crate::filesearch::search(&root, &query, 30))
        .await
        .map_err(|e| AppError::msg(e.to_string()))?;
    Ok(json!(r))
}

/// Messages typed before (all conversations), newest first, for Ctrl+R.
#[tauri::command]
pub async fn prompt_history(state: St<'_>, query: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let rows = tokio::task::spawn_blocking(move || st.db.prompt_history(&query, 60))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    Ok(json!(rows.into_iter().map(|(text, at, title)| json!({"text": text, "at": at, "title": title})).collect::<Vec<_>>()))
}

/// Open the system terminal in a folder, running the user's shell.
#[tauri::command]
pub async fn open_system_terminal(path: String) -> AppResult<()> {
    let dir = std::path::PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(AppError::msg(format!("Folder not found: {path}")));
    }
    #[cfg(windows)]
    let shell = std::path::PathBuf::from("cmd");
    #[cfg(not(windows))]
    let shell = std::path::PathBuf::from(std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into()));
    crate::claudecode::open_in_terminal(Some(&dir), &shell, &[])
}

/// Continue a Claude Code session in the system terminal (`claude --resume`).
#[tauri::command]
pub async fn cc_open_in_terminal(state: St<'_>, conversation_id: String) -> AppResult<()> {
    let st = state.inner().clone();
    let conv = st.db.conv_get(&conversation_id)?;
    let cwd = conv.cwd.clone().ok_or_else(|| AppError::msg("This conversation has no project folder."))?;
    let path = match &conv.current_leaf {
        Some(l) => st.db.path_to(l)?,
        None => vec![],
    };
    let sid = crate::claudecode::latest_session(&path).ok_or_else(|| AppError::msg("This conversation has no Claude Code session yet: send a message first."))?;
    let cli = crate::claudecode::find_cli(&st.settings.read().claude_code_path).ok_or_else(|| AppError::msg("Claude Code isn't installed."))?;
    crate::claudecode::open_in_terminal(Some(std::path::Path::new(&cwd)), &cli, &["--resume".to_string(), sid])
}

/// Runnable scripts the session's folder declares (Tasks pane).
#[tauri::command]
pub async fn project_scripts(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let Some(root) = root_of(&state, Some(&conversation_id), None) else { return Ok(json!([])) };
    let list = tokio::task::spawn_blocking(move || crate::scripts::detect(&root))
        .await
        .map_err(|e| AppError::msg(e.to_string()))?;
    Ok(serde_json::to_value(list)?)
}

/// TODO / FIXME / HACK / XXX / BUG comments in the session's project.
#[tauri::command]
pub async fn scan_todos(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let Some(root) = root_of(&state, Some(&conversation_id), None) else { return Err(AppError::msg("This session has no folder.")) };
    let (items, truncated) = tokio::task::spawn_blocking(move || crate::todos::scan(&root))
        .await
        .map_err(|e| AppError::msg(e.to_string()))?;
    Ok(json!({"items": items, "truncated": truncated}))
}

/// Find in files (Files pane): text or regex through the project.
#[tauri::command]
pub async fn search_in_files(state: St<'_>, conversation_id: String, query: String, regex: bool, case_sensitive: bool) -> AppResult<Value> {
    let Some(root) = root_of(&state, Some(&conversation_id), None) else { return Err(AppError::msg("This session has no folder.")) };
    let r = tokio::task::spawn_blocking(move || crate::grepsearch::search(&root, &query, regex, case_sensitive))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    Ok(serde_json::to_value(r)?)
}

/// Who last changed each line of a project file (Files viewer → Blame).
#[tauri::command]
pub async fn file_blame(state: St<'_>, conversation_id: String, path: String) -> AppResult<Value> {
    let Some(cwd) = root_of(&state, Some(&conversation_id), None) else { return Err(AppError::msg("This session has no folder.")) };
    let root = crate::gitops::repo_root(&cwd).await.ok_or_else(|| AppError::msg("This folder is not a git repository."))?;
    let abs = std::fs::canonicalize(cwd.join(path.trim_start_matches('/'))).map_err(|_| AppError::msg(format!("File not found: {path}")))?;
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let rel = abs.strip_prefix(&root).map_err(|_| AppError::msg("That file is outside the repository."))?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    let out = crate::gitops::git(&root, &["blame", "--porcelain", "--", &rel])
        .await
        .map_err(|e| AppError::msg(if e.to_string().contains("no such path") { "This file isn't in git yet.".to_string() } else { e.to_string() }))?;
    Ok(crate::gitops::parse_blame(&out))
}

/// Conversations that edited a project file (Files viewer).
#[tauri::command]
pub async fn file_sessions(state: St<'_>, conversation_id: String, path: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let Some(cwd) = root_of(&st, Some(&conversation_id), None) else { return Ok(json!([])) };
    let rel = path.trim_start_matches('/').to_string();
    let abs = cwd.join(&rel);
    let dir = cwd.to_string_lossy().to_string();
    let rows = tokio::task::spawn_blocking(move || st.db.conversations_editing(&abs.to_string_lossy(), &rel, &dir, 20))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    Ok(json!(rows.into_iter().map(|(id, title, at)| json!({"id": id, "title": title, "at": at})).collect::<Vec<_>>()))
}

/// One level of the project's file tree (for the Files pane).
#[tauri::command]
pub async fn project_dir(state: St<'_>, conversation_id: String, dir: Option<String>) -> AppResult<Value> {
    let Some(root) = root_of(&state, Some(&conversation_id), None) else { return Err(AppError::msg("This session has no folder.")) };
    let dir = dir.unwrap_or_default();
    let r = tokio::task::spawn_blocking(move || crate::filesearch::children(&root, &dir))
        .await
        .map_err(|e| AppError::msg(e.to_string()))?;
    Ok(json!(r.into_iter().map(|(p, d)| json!({"path": p, "dir": d})).collect::<Vec<_>>()))
}

/// A project file for the read-only viewer, highlighted.
#[tauri::command]
pub async fn project_file(state: St<'_>, conversation_id: String, path: String) -> AppResult<Value> {
    let Some(root) = root_of(&state, Some(&conversation_id), None) else { return Err(AppError::msg("This session has no folder.")) };
    tokio::task::spawn_blocking(move || crate::filesearch::view_file(&root, &path))
        .await
        .map_err(|e| AppError::msg(e.to_string()))?
}

#[tauri::command]
pub async fn symbol_search(state: St<'_>, conversation_id: Option<String>, cwd: Option<String>, query: String) -> AppResult<Value> {
    let Some(root) = root_of(&state, conversation_id.as_deref(), cwd) else { return Ok(json!([])) };
    let r = tokio::task::spawn_blocking(move || crate::symbols::search(&root, &query, 12))
        .await
        .map_err(|e| AppError::msg(e.to_string()))?;
    Ok(json!(r))
}

#[tauri::command]
pub async fn slash_list(state: St<'_>, conversation_id: Option<String>, cwd: Option<String>) -> AppResult<Value> {
    let root = root_of(&state, conversation_id.as_deref(), cwd);
    Ok(crate::slash::to_json(&crate::slash::list(root.as_deref())))
}

#[tauri::command]
pub async fn slash_expand(state: St<'_>, conversation_id: Option<String>, cwd: Option<String>, name: String, args: String) -> AppResult<Option<String>> {
    let root = root_of(&state, conversation_id.as_deref(), cwd);
    Ok(crate::slash::expand(root.as_deref(), &name, &args))
}

// ------------------------------------------------------------- worktrees

fn worktree_info(st: &AppState, conversation_id: &str) -> AppResult<Value> {
    let c = st.db.conv_get(conversation_id)?;
    let w = c.options["worktree"].clone();
    if !w.is_object() {
        return Err(AppError::msg("This session doesn't use a separate worktree."));
    }
    Ok(w)
}

#[tauri::command]
pub async fn worktree_apply(app: AppHandle, state: St<'_>, conversation_id: String) -> AppResult<String> {
    let w = worktree_info(&state, &conversation_id)?;
    let r = tokio::task::spawn_blocking(move || crate::worktree::apply_to_origin(&w))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    Ok(r)
}

#[tauri::command]
pub async fn worktree_commit(app: AppHandle, state: St<'_>, conversation_id: String, message: String) -> AppResult<String> {
    let w = worktree_info(&state, &conversation_id)?;
    let msg = if message.trim().is_empty() { "Changes from a ClaudeRust session".to_string() } else { message };
    let r = tokio::task::spawn_blocking(move || crate::worktree::commit_all(&w, &msg))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    Ok(r)
}

// ------------------------------------------------------------- checkpoints / rewind

/// Rewind a code session to just before `message_id` (a user message):
/// the files (if `code`), and/or the conversation (if `conversation`, the
/// next message then starts a new branch from there). Returns the message
/// text so the UI can put it back in the composer, and an undo id.
#[tauri::command]
pub async fn rewind(app: AppHandle, state: St<'_>, conversation_id: String, message_id: String, code: bool, conversation: bool) -> AppResult<Value> {
    let st = state.inner().clone();
    if st.runs.lock().contains_key(&conversation_id) {
        return Err(AppError::msg("Stop the current response before rewinding."));
    }
    let conv = st.db.conv_get(&conversation_id)?;
    let msg = st.db.msg_get(&message_id)?;
    if msg.conversation_id != conversation_id || msg.role != "user" {
        return Err(AppError::msg("Pick one of your messages to rewind to."));
    }
    let mut undo = Value::Null;
    if code {
        let sha = msg.meta["checkpoint"].as_str().ok_or_else(|| AppError::msg("There is no checkpoint for this message."))?.to_string();
        let cwd = conv.cwd.clone().ok_or_else(|| AppError::msg("This conversation has no project folder."))?;
        let data = st.data_dir.clone();
        let before = tokio::task::spawn_blocking(move || crate::checkpoints::Shadow::new(&data, std::path::Path::new(&cwd)).restore(&sha))
            .await
            .map_err(|e| AppError::msg(e.to_string()))??;
        undo = json!(before);
        // the agent must re-read files before editing them again
        if let Some(s) = st.code_sessions.lock().get(&conversation_id) {
            s.read_files.lock().clear();
        }
        let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    }
    if conversation {
        st.db.conv_set_leaf(&conversation_id, msg.parent_id.as_deref())?;
        if conv.options["engine"] == crate::claudecode::ENGINE {
            // the next message must fork the CLI session before this point
            engine::update_conv_options(&st, &conversation_id, |o| {
                if let Some(m) = o.as_object_mut() {
                    m.remove("cc_tip");
                }
            });
        }
        engine::emit_conv_updated(&app, &conversation_id);
    }
    let text = crate::render::plain_text(&Value::Array(
        msg.content.as_array().cloned().unwrap_or_default().into_iter().filter(|b| !b["text"].as_str().unwrap_or("").starts_with("<system-reminder>")).collect(),
    ));
    Ok(json!({"text": text, "undo": undo}))
}

/// Undo a code rewind (restore the snapshot taken just before it).
#[tauri::command]
pub async fn rewind_undo(app: AppHandle, state: St<'_>, conversation_id: String, undo: String) -> AppResult<()> {
    let conv = state.db.conv_get(&conversation_id)?;
    let cwd = conv.cwd.ok_or_else(|| AppError::msg("This conversation has no project folder."))?;
    let data = state.data_dir.clone();
    tokio::task::spawn_blocking(move || crate::checkpoints::Shadow::new(&data, std::path::Path::new(&cwd)).restore(&undo))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    Ok(())
}

/// What a rewind to `message_id` would restore: whether a checkpoint exists
/// and which files changed since.
#[tauri::command]
pub async fn rewind_info(state: St<'_>, conversation_id: String, message_id: String) -> AppResult<Value> {
    let conv = state.db.conv_get(&conversation_id)?;
    let msg = state.db.msg_get(&message_id)?;
    let Some(sha) = msg.meta["checkpoint"].as_str().map(String::from) else {
        return Ok(json!({"checkpoint": false, "files": []}));
    };
    let Some(cwd) = conv.cwd else { return Ok(json!({"checkpoint": false, "files": []})) };
    let data = state.data_dir.clone();
    let files = tokio::task::spawn_blocking(move || crate::checkpoints::Shadow::new(&data, std::path::Path::new(&cwd)).changed_since(&sha))
        .await
        .map_err(|e| AppError::msg(e.to_string()))?
        .unwrap_or_default();
    Ok(json!({"checkpoint": true, "files": files}))
}

// ------------------------------------------------------------- plan & todos

/// Latest todo list and plan of a conversation's current branch.
pub fn plan_state_of(path: &[crate::db::Message]) -> Value {
    let mut todos = Value::Null;
    let mut plan: Option<(String, String)> = None; // (tool_use_id, text)
    let mut results: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for m in path {
        for b in m.content.as_array().into_iter().flatten() {
            match b["type"].as_str() {
                Some("tool_use") => match b["name"].as_str() {
                    Some("TodoWrite") if b["input"]["todos"].is_array() => todos = b["input"]["todos"].clone(),
                    Some("ExitPlanMode") => {
                        if let Some(p) = b["input"]["plan"].as_str() {
                            plan = Some((b["id"].as_str().unwrap_or("").to_string(), p.to_string()));
                        }
                    }
                    _ => {}
                },
                Some("tool_result") => {
                    if let Some(id) = b["tool_use_id"].as_str() {
                        results.insert(id.to_string(), b["is_error"].as_bool() == Some(true));
                    }
                }
                _ => {}
            }
        }
    }
    let (plan_text, plan_status) = match &plan {
        Some((id, text)) => (
            json!(text),
            json!(match results.get(id) {
                Some(false) => "approved",
                Some(true) => "rejected",
                None => "pending",
            }),
        ),
        None => (Value::Null, Value::Null),
    };
    json!({"todos": todos, "plan": plan_text, "plan_status": plan_status})
}

#[tauri::command]
pub async fn plan_state(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let conv = state.db.conv_get(&conversation_id)?;
    let path = match &conv.current_leaf {
        Some(leaf) => state.db.path_to(leaf)?,
        None => vec![],
    };
    let mut v = plan_state_of(&path);
    v["mode"] = conv.options["permission_mode"].clone();
    Ok(v)
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    #[test]
    fn latest_todos_and_plan_status() {
        let m = |id: &str, role: &str, content: Value| crate::db::Message {
            id: id.into(), conversation_id: "c".into(), parent_id: None, role: role.into(), content, meta: json!({}), created_at: 0,
        };
        let path = vec![
            m("a1", "assistant", json!([{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[{"content":"x","status":"pending"}]}}])),
            m("u1", "user", json!([{"type":"tool_result","tool_use_id":"t1","content":"ok"}])),
            m("a2", "assistant", json!([{"type":"tool_use","id":"p1","name":"ExitPlanMode","input":{"plan":"1. do it"}},
                                         {"type":"tool_use","id":"t2","name":"TodoWrite","input":{"todos":[{"content":"x","status":"completed"}]}}])),
            m("u2", "user", json!([{"type":"tool_result","tool_use_id":"p1","content":"approved"}])),
        ];
        let v = plan_state_of(&path);
        assert_eq!(v["todos"][0]["status"], "completed");
        assert_eq!(v["plan"], "1. do it");
        assert_eq!(v["plan_status"], "approved");
        let v2 = plan_state_of(&path[..3]);
        assert_eq!(v2["plan_status"], "pending");
    }
}

// ------------------------------------------------------------- memory files

fn conv_cwd(st: &AppState, conversation_id: &str) -> AppResult<std::path::PathBuf> {
    let c = st.db.conv_get(conversation_id)?;
    c.cwd.map(std::path::PathBuf::from).ok_or_else(|| AppError::msg("This conversation has no project folder."))
}

#[tauri::command]
pub async fn memory_files(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    Ok(json!(crate::memory::files(&conv_cwd(&state, &conversation_id)?)))
}

#[tauri::command]
pub async fn memory_save(state: St<'_>, conversation_id: String, path: String, content: String) -> AppResult<()> {
    crate::memory::save(&conv_cwd(&state, &conversation_id)?, std::path::Path::new(&path), &content)
}

#[tauri::command]
pub async fn memory_append(state: St<'_>, conversation_id: String, scope: String, note: String) -> AppResult<String> {
    let p = crate::memory::append(&conv_cwd(&state, &conversation_id)?, &scope, &note)?;
    Ok(p.to_string_lossy().to_string())
}

// ------------------------------------------------------------- compaction

/// Summarize the conversation so far; the summary replaces it for the next
/// message (built-in engine).
#[tauri::command]
pub async fn compact_conversation(app: AppHandle, state: St<'_>, conversation_id: String, instructions: Option<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    if st.runs.lock().contains_key(&conversation_id) {
        return Err(AppError::msg("Wait for the current response to finish before compacting."));
    }
    let conv = st.db.conv_get(&conversation_id)?;
    if conv.model.starts_with(crate::oai::PREFIX) {
        return Err(AppError::msg("Compaction uses a Claude model; switch this conversation to a Claude model first."));
    }
    let leaf = conv.current_leaf.clone().ok_or_else(|| AppError::msg("There is nothing to compact yet."))?;
    let path = st.db.path_to(&leaf)?;
    let before = crate::compact::context_tokens(&path[crate::compact::start_index(&path).unwrap_or(0)..]);
    let _ = app.emit("compacting", json!({"conversation_id": conversation_id, "auto": false}));
    let summary = crate::compact::summarize(&st, &conv.model, &path, instructions.as_deref()).await;
    let summary = match summary {
        Ok(s) => s,
        Err(e) => {
            let _ = app.emit("compacted", json!({"conversation_id": conversation_id, "error": e.to_string()}));
            return Err(e);
        }
    };
    let s2 = summary.clone();
    engine::update_conv_options(&st, &conversation_id, |o| o["pending_compact"] = json!({"summary": s2, "upto": leaf}));
    let _ = app.emit("compacted", json!({"conversation_id": conversation_id, "auto": false}));
    Ok(json!({"summary": summary, "context_before": before}))
}

// ------------------------------------------------------------- hooks

/// Hooks configured for a conversation's project (for Settings / review).
#[tauri::command]
pub async fn hooks_info(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let cwd = conv_cwd(&state, &conversation_id)?;
    let cfg = engine::hook_config(&state, &cwd);
    let active: Vec<Value> = cfg
        .events
        .iter()
        .flat_map(|(e, hs)| hs.iter().map(move |h| json!({"event": e, "matcher": h.matcher, "command": h.command})))
        .collect();
    Ok(json!({"active": active, "untrusted": cfg.untrusted.map(|(r, h, c)| json!({"root": r, "hash": h, "commands": c}))}))
}

/// Trust (or stop trusting) a project's current hook configuration.
#[tauri::command]
pub async fn hooks_trust(state: St<'_>, root: String, hash: Option<String>) -> AppResult<()> {
    let st = state.inner().clone();
    let mut s = st.settings.write();
    match hash {
        Some(h) => {
            // only the configuration that is there now can be trusted
            let current = crate::hooks::project_hash(std::path::Path::new(&root)).map(|(h, _)| h);
            if current.as_deref() != Some(h.as_str()) {
                return Err(AppError::msg("The project's hooks changed; review them again."));
            }
            s.trusted_hooks.insert(root, json!(h));
        }
        None => {
            s.trusted_hooks.remove(&root);
        }
    }
    s.save(&st.db)?;
    Ok(())
}

// ------------------------------------------------------------- sub-agents

#[tauri::command]
pub async fn subagents_list(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    Ok(json!(state.subagents.list(&conversation_id)))
}

#[tauri::command]
pub async fn subagent_output(state: St<'_>, id: String) -> AppResult<String> {
    state.subagents.log(&id).ok_or_else(|| AppError::msg("This sub-agent is gone."))
}

#[tauri::command]
pub async fn subagent_stop(state: St<'_>, id: String) -> AppResult<bool> {
    Ok(state.subagents.stop(&id))
}

#[tauri::command]
pub async fn subagents_clear(state: St<'_>, conversation_id: String) -> AppResult<()> {
    state.subagents.clear_finished(&conversation_id);
    Ok(())
}

// ------------------------------------------------------------- /doctor

fn dir_size(p: &std::path::Path, budget: &mut usize) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        match e.file_type() {
            Ok(t) if t.is_dir() => n += dir_size(&e.path(), budget),
            Ok(_) => n += e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => {}
        }
    }
    n
}

/// Health checks for the app and (optionally) a code session.
#[tauri::command]
pub async fn doctor(state: St<'_>, conversation_id: Option<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    let settings = st.settings.read().clone();
    let mut checks: Vec<Value> = vec![];
    let mut add = |name: &str, status: &str, detail: String| checks.push(json!({"name": name, "status": status, "detail": detail}));

    add("App", "ok", format!("ClaudeRust {} on {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH));
    let has_key = st.api_key.read().is_some();
    add("API key", if has_key { "ok" } else { "warn" }, if has_key { format!("set · endpoint {}", settings.base_url) } else { "not set (chat and the built-in agent need one; Claude Code uses its own sign-in)".into() });
    if has_key {
        let r = match st.client() {
            Ok(c) => tokio::time::timeout(std::time::Duration::from_secs(10), c.list_models()).await.ok().map(|r| r.map(|m| m.len())),
            Err(_) => None,
        };
        match r {
            Some(Ok(n)) => add("API connection", "ok", format!("reachable · {n} models")),
            Some(Err(e)) => add("API connection", "error", e.to_string()),
            None => add("API connection", "error", "no answer within 10 s".into()),
        }
    }
    let cc = crate::claudecode::cli_info(&settings.claude_code_path).await;
    if cc["found"] == true {
        add("Claude Code CLI", "ok", format!("{} · {}", cc["version"].as_str().unwrap_or("?"), cc["path"].as_str().unwrap_or("")));
    } else {
        add("Claude Code CLI", "warn", "not found (needed for the Claude Code engine and subscription sign-in)".into());
    }
    match crate::gitops::git(&std::env::temp_dir(), &["--version"]).await {
        Ok(v) => add("git", "ok", v.trim().to_string()),
        Err(_) => add("git", "warn", "not found (changes, worktrees and checkpoints need git)".into()),
    }
    let shell = if cfg!(windows) { "powershell" } else if which::which("bash").is_ok() { "bash" } else { "sh" };
    add("Shell", "ok", shell.to_string());
    for s in st.mcp.statuses().await {
        let v = serde_json::to_value(&s).unwrap_or_default();
        let status = v["status"].as_str().unwrap_or("");
        add(&format!("MCP: {}", v["name"].as_str().unwrap_or("?")), if status == "connected" { "ok" } else if matches!(status, "connecting" | "starting" | "disabled") { "warn" } else { "error" },
            format!("{status}{}", v["error"].as_str().map(|e| format!(" — {e}")).unwrap_or_default()));
    }
    let mut budget = 20_000;
    let cp = dir_size(&st.data_dir.join("checkpoints"), &mut budget);
    add("Checkpoints", "ok", format!("{} · {:.1} MB", if settings.code_checkpoints { "on" } else { "off" }, cp as f64 / 1_048_576.0));
    if let Some(id) = conversation_id {
        if let Ok(conv) = st.db.conv_get(&id) {
            if let Some(cwd) = conv.cwd.clone() {
                let p = std::path::PathBuf::from(&cwd);
                add("Project folder", if p.is_dir() { "ok" } else { "error" }, cwd.clone());
                add("Git repository", "ok", crate::worktree::repo_root(&p).map(|r| r.display().to_string()).unwrap_or_else(|| "no (changes are tracked from the agent's edits)".into()));
                let mem: Vec<String> = crate::memory::files(&p).into_iter().filter(|f| f["exists"] == true).map(|f| f["path"].as_str().unwrap_or("").to_string()).collect();
                add("Memory files", "ok", if mem.is_empty() { "none".into() } else { mem.join(", ") });
                let hooks = engine::hook_config(&st, &p);
                let n: usize = hooks.events.values().map(|v| v.len()).sum();
                add("Hooks", if hooks.untrusted.is_some() { "warn" } else { "ok" }, format!("{n} active{}", if hooks.untrusted.is_some() { " · project hooks not trusted yet" } else { "" }));
                add("Engine", "ok", conv.options["engine"].as_str().unwrap_or("builtin").to_string());
            }
        }
    }
    Ok(json!(checks))
}

// ------------------------------------------------------------- preview

/// Local web addresses mentioned in a session's command output (dev servers).
pub fn find_local_urls(text: &str) -> Vec<String> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):\d{2,5}(?:/[^\s'\x22<>)\x1b]*)?").unwrap()
    });
    let mut out: Vec<String> = vec![];
    for m in RE.find_iter(text) {
        let u = m.as_str().replace("0.0.0.0", "localhost").trim_end_matches(['.', ',', ';']).to_string();
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

#[tauri::command]
pub async fn preview_urls(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let mut urls: Vec<Value> = vec![];
    for sh in state.shells.list(Some(&conversation_id)) {
        for u in find_local_urls(&sh.tail(200_000)) {
            if !urls.iter().any(|x| x["url"] == u.as_str()) {
                urls.push(json!({"url": u, "source": sh.description.clone(), "running": sh.running()}));
            }
        }
    }
    Ok(json!(urls))
}

#[cfg(test)]
mod preview_tests {
    #[test]
    fn finds_dev_server_urls() {
        let t = "  VITE v5  ready\n  ➜  Local:   http://localhost:5173/\n Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ...\nsee https://example.com:443 too";
        assert_eq!(super::find_local_urls(t), vec!["http://localhost:5173/".to_string(), "http://localhost:8000/".to_string()]);
    }
}

// ------------------------------------------------------------- routines

/// Start a routine now: a new code session in its folder with its prompt.
pub async fn run_routine(app: &AppHandle, st: &Arc<AppState>, r: &crate::routines::Routine) -> AppResult<String> {
    // the previous run's worktree goes away if nothing came of it
    if let Some(prev) = r.last_conversation_id.as_deref().and_then(|id| st.db.conv_get(id).ok()) {
        let w = prev.options["worktree"].clone();
        if w.is_object() && !st.runs.lock().contains_key(&prev.id) {
            let disposable = tokio::task::spawn_blocking({
                let w = w.clone();
                move || crate::worktree::is_disposable(&w)
            })
            .await
            .unwrap_or(false);
            if disposable {
                let _ = tokio::task::spawn_blocking(move || crate::worktree::remove(&w)).await;
            }
        }
    }
    let mut options = json!({"permission_mode": r.permission_mode, "routine_id": r.id, "worktree": r.worktree});
    if !r.engine.is_empty() {
        options["engine"] = json!(r.engine);
    }
    let req = SendReq {
        conversation_id: None,
        new_conversation: Some(NewConversation {
            kind: Some("code".into()),
            model: r.model.clone(),
            project_id: None,
            cwd: Some(r.cwd.clone()),
            options,
        }),
        text: r.prompt.clone(),
        attachment_ids: vec![],
        parent_id: None,
        edit: false,
    };
    let v = send_message_inner(app, st.clone(), req).await?;
    let id = v["conversation_id"].as_str().unwrap_or_default().to_string();
    let when = chrono::Local::now().format("%b %-d, %H:%M").to_string();
    let _ = st.db.conv_set_title(&id, &format!("{} · {when}", r.name));
    engine::emit_conv_updated(app, &id);
    Ok(id)
}

/// Check for due routines every 30 seconds.
pub fn start_routine_scheduler(app: AppHandle, st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        loop {
            let list = crate::routines::load(&st.db);
            for r in crate::routines::due(&list, now_ms()) {
                // one run at a time per routine
                let busy = r.last_conversation_id.as_deref().map(|c| st.runs.lock().contains_key(c)).unwrap_or(false);
                if busy {
                    continue;
                }
                match run_routine(&app, &st, &r).await {
                    Ok(conv) => crate::routines::mark_run(&st.db, &r.id, Some(conv), None),
                    Err(e) => crate::routines::mark_run(&st.db, &r.id, None, Some(e.to_string())),
                }
                let _ = app.emit("routines-changed", json!({}));
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

#[tauri::command]
pub async fn routines_list(state: St<'_>) -> AppResult<Value> {
    Ok(json!(crate::routines::load(&state.db).iter().map(crate::routines::to_json).collect::<Vec<_>>()))
}

#[tauri::command]
pub async fn routine_save(state: St<'_>, routine: crate::routines::Routine) -> AppResult<Value> {
    let r = crate::routines::upsert(&state.db, routine)?;
    Ok(crate::routines::to_json(&r))
}

#[tauri::command]
pub async fn routine_delete(state: St<'_>, id: String) -> AppResult<()> {
    crate::routines::delete(&state.db, &id)
}

#[tauri::command]
pub async fn routine_run(app: AppHandle, state: St<'_>, id: String) -> AppResult<String> {
    let st = state.inner().clone();
    let r = crate::routines::load(&st.db).into_iter().find(|r| r.id == id).ok_or_else(|| AppError::msg("Routine not found."))?;
    let res = run_routine(&app, &st, &r).await;
    match &res {
        Ok(c) => crate::routines::mark_run(&st.db, &id, Some(c.clone()), None),
        Err(e) => crate::routines::mark_run(&st.db, &id, None, Some(e.to_string())),
    }
    let _ = app.emit("routines-changed", json!({}));
    res
}

// ------------------------------------------------------------- watch mode

#[tauri::command]
pub async fn watch_status(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    if let Some(w) = state.watches.get(&conversation_id) {
        return Ok(w.summary(&conversation_id));
    }
    let cwd = conv_cwd(&state, &conversation_id)?;
    Ok(json!({"watching": false, "detected": crate::watch::detect(&cwd)}))
}

#[tauri::command]
pub async fn watch_start(app: AppHandle, state: St<'_>, conversation_id: String, command: Option<String>, auto_fix: Option<bool>) -> AppResult<Value> {
    let st = state.inner().clone();
    let cwd = conv_cwd(&st, &conversation_id)?;
    let command = command.filter(|c| !c.trim().is_empty()).or_else(|| crate::watch::detect(&cwd))
        .ok_or_else(|| AppError::msg("No check command found for this project; enter one (for example: npm test)."))?;
    engine::update_conv_options(&st, &conversation_id, |o| {
        o["watch_command"] = json!(command);
        o["watch_auto_fix"] = json!(auto_fix.unwrap_or(false));
        o["watch_enabled"] = json!(true);
    });
    let st_busy = st.clone();
    let conv_busy = conversation_id.clone();
    let busy: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || st_busy.runs.lock().contains_key(&conv_busy));
    // Auto-fix: when a check fails after Claude's turn, send the errors back (at most twice in a row).
    let st_fix = st.clone();
    let app_fix = app.clone();
    let conv_fix = conversation_id.clone();
    let streak = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let on_result: Arc<dyn Fn(Value) + Send + Sync> = Arc::new(move |summary: Value| {
        use std::sync::atomic::Ordering;
        if summary["status"] != "failed" {
            streak.store(0, Ordering::Relaxed);
            return;
        }
        if summary["auto_fix"] != true || streak.load(Ordering::Relaxed) >= 2 || st_fix.runs.lock().contains_key(&conv_fix) {
            return;
        }
        streak.fetch_add(1, Ordering::Relaxed);
        let text = watch_fix_prompt(&summary);
        let (st2, app2, conv2) = (st_fix.clone(), app_fix.clone(), conv_fix.clone());
        tauri::async_runtime::spawn(async move {
            let req = SendReq { conversation_id: Some(conv2), new_conversation: None, text, attachment_ids: vec![], parent_id: None, edit: false };
            let _ = send_message_inner(&app2, st2, req).await;
        });
    });
    let v = st.watches.start(&app, &conversation_id, &cwd, command, busy, on_result)?;
    if auto_fix == Some(true) {
        st.watches.set_auto_fix(&conversation_id, true);
    }
    Ok(v)
}

/// The prompt that asks Claude to fix a failing check.
pub fn watch_fix_prompt(summary: &Value) -> String {
    let out = summary["output"].as_str().unwrap_or("");
    let tail: String = {
        let n = out.chars().count();
        if n > 12_000 { out.chars().skip(n - 12_000).collect() } else { out.to_string() }
    };
    format!(
        "The project check `{}` fails ({} error line(s)). Please fix the errors.\n\n```\n{}\n```",
        summary["command"].as_str().unwrap_or(""),
        summary["errors"].as_u64().unwrap_or(0),
        tail.trim()
    )
}

#[tauri::command]
pub async fn watch_stop(state: St<'_>, conversation_id: String) -> AppResult<()> {
    state.watches.stop(&conversation_id);
    engine::update_conv_options(&state, &conversation_id, |o| o["watch_enabled"] = json!(false));
    Ok(())
}

#[tauri::command]
pub async fn watch_run(state: St<'_>, conversation_id: String) -> AppResult<()> {
    state.watches.run_now(&conversation_id);
    Ok(())
}

#[tauri::command]
pub async fn watch_auto_fix(state: St<'_>, conversation_id: String, on: bool) -> AppResult<()> {
    state.watches.set_auto_fix(&conversation_id, on);
    engine::update_conv_options(&state, &conversation_id, |o| o["watch_auto_fix"] = json!(on));
    Ok(())
}

// ------------------------------------------------------------- commit with a suggested message

pub const COMMIT_PROMPT: &str = "Write a git commit message for the diff below. First line: an imperative summary under 72 characters, no trailing period. Then, only if useful, a blank line and a short body (wrapped at 72) explaining what changed and why. Match the style of the recent commits if they follow a convention (e.g. Conventional Commits). Reply with the message only.";

/// Suggest a commit message for the session's uncommitted changes.
#[tauri::command]
pub async fn commit_message_suggest(state: St<'_>, conversation_id: String) -> AppResult<String> {
    let st = state.inner().clone();
    let cwd = conv_cwd(&st, &conversation_id)?;
    let root = crate::gitops::repo_root(&cwd).await.ok_or_else(|| AppError::msg("This folder is not a git repository."))?;
    let _ = crate::gitops::git(&root, &["add", "-A", "--intent-to-add"]).await;
    let diff = crate::gitops::git(&root, &["diff", "HEAD", "--no-color", "--no-ext-diff", "--stat", "--patch"]).await.unwrap_or_default();
    if diff.trim().is_empty() {
        return Err(AppError::msg("There are no changes to commit."));
    }
    let log = crate::gitops::git(&root, &["log", "--oneline", "-10"]).await.unwrap_or_default();
    let files = diff.lines().filter(|l| l.starts_with("diff --git")).count();
    let fallback = format!("Update {files} file{}", if files == 1 { "" } else { "s" });
    let Ok(client) = st.client() else { return Ok(fallback) };
    let model_id = st.settings.read().title_model.clone();
    let model = models::info(&model_id);
    let mut body = json!({
        "model": model_id,
        "max_tokens": 1024,
        "system": COMMIT_PROMPT,
        "messages": [{"role": "user", "content": format!("<recent_commits>\n{log}\n</recent_commits>\n\n<diff>\n{}\n</diff>", crate::util::truncate_middle(&diff, 60_000))}],
    });
    match model.thinking {
        models::Thinking::Disableable => body["thinking"] = json!({"type": "disabled"}),
        models::Thinking::BetweenTools => body["thinking"] = json!({"type": "between_tools"}),
        _ => {}
    }
    let r = tokio::time::timeout(std::time::Duration::from_secs(45), client.create(&body, &[])).await;
    let text = match r {
        Ok(Ok(v)) => v["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""),
        _ => String::new(),
    };
    let text = text.trim().trim_matches('`').trim().to_string();
    Ok(if text.is_empty() { fallback } else { text })
}

/// Local branches of the session's repository, most recently used first.
#[tauri::command]
pub async fn git_branches(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let root = session_repo(&state, &conversation_id).await?;
    let out = crate::gitops::git(&root, &["for-each-ref", "--sort=-committerdate", "--format=%(HEAD)%09%(refname:short)%09%(committerdate:unix)%09%(subject)", "refs/heads/"]).await?;
    let list: Vec<Value> = out
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '\t');
            let head = p.next()? == "*";
            let name = p.next()?.to_string();
            let at = p.next().and_then(|x| x.parse::<i64>().ok()).unwrap_or(0) * 1000;
            Some(json!({"name": name, "current": head, "at": at, "subject": p.next().unwrap_or("")}))
        })
        .collect();
    Ok(json!(list))
}

/// Stash the uncommitted changes (untracked files too), or apply the latest
/// stash back (`pop`).
#[tauri::command]
pub async fn git_stash(app: AppHandle, state: St<'_>, conversation_id: String, pop: bool) -> AppResult<String> {
    let root = session_repo(&state, &conversation_id).await?;
    let r = if pop {
        crate::gitops::git(&root, &["stash", "pop"]).await.map(|_| "Applied the latest stash.".to_string())
    } else {
        let msg = format!("ClaudeRust {}", chrono::Local::now().format("%Y-%m-%d %H:%M"));
        crate::gitops::git(&root, &["stash", "push", "--include-untracked", "-m", &msg]).await.map(|o| {
            if o.contains("No local changes") { "There's nothing to stash.".to_string() } else { "Changes stashed.".to_string() }
        })
    };
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    r
}

/// How many stashes the repository has.
#[tauri::command]
pub async fn git_stash_count(state: St<'_>, conversation_id: String) -> AppResult<usize> {
    let root = session_repo(&state, &conversation_id).await?;
    Ok(crate::gitops::git(&root, &["stash", "list"]).await.map(|s| s.lines().count()).unwrap_or(0))
}

/// Switch to a local branch, or create it (`create`) from the current commit.
/// Uncommitted changes come along when git can carry them.
#[tauri::command]
pub async fn git_switch(app: AppHandle, state: St<'_>, conversation_id: String, name: String, create: bool) -> AppResult<String> {
    let root = session_repo(&state, &conversation_id).await?;
    let name = name.trim().to_string();
    if name.is_empty() || name.starts_with('-') {
        return Err(AppError::msg("Give a branch name."));
    }
    crate::gitops::git(&root, &["check-ref-format", "--branch", &name]).await.map_err(|_| AppError::msg(format!("“{name}” isn't a valid branch name.")))?;
    let r = if create {
        crate::gitops::git(&root, &["switch", "-c", &name]).await
    } else {
        crate::gitops::git(&root, &["switch", &name]).await
    };
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    r.map(|_| format!("On branch {name}."))
}

/// Secrets in the session's uncommitted changes (added lines only).
#[tauri::command]
pub async fn secret_scan(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let root = session_repo(&state, &conversation_id).await?;
    // new files only show in the diff once git knows about them
    let _ = crate::gitops::git(&root, &["add", "-A", "--intent-to-add"]).await;
    let diff = crate::gitops::git(&root, &["diff", "HEAD", "--no-color", "--no-ext-diff"]).await.unwrap_or_default();
    Ok(serde_json::to_value(crate::secrets::scan_diff(&diff))?)
}

/// Commit all changes of the session (and optionally push).
#[tauri::command]
pub async fn git_commit(app: AppHandle, state: St<'_>, conversation_id: String, message: String, push: bool) -> AppResult<String> {
    let cwd = conv_cwd(&state, &conversation_id)?;
    let root = crate::gitops::repo_root(&cwd).await.ok_or_else(|| AppError::msg("This folder is not a git repository."))?;
    if message.trim().is_empty() {
        return Err(AppError::msg("Write a commit message."));
    }
    crate::gitops::git(&root, &["add", "-A"]).await?;
    crate::gitops::git(&root, &["commit", "-q", "-m", message.trim()]).await?;
    let sha = crate::gitops::git(&root, &["rev-parse", "--short", "HEAD"]).await.unwrap_or_default();
    let branch = crate::gitops::git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap_or_default();
    let mut msg = format!("Committed {} on {}.", sha.trim(), branch.trim());
    if push {
        let has_upstream = crate::gitops::git_raw(&root, &["rev-parse", "--abbrev-ref", "@{u}"]).await.map(|o| o.status.success()).unwrap_or(false);
        let r = if has_upstream {
            crate::gitops::git(&root, &["push"]).await
        } else {
            crate::gitops::git(&root, &["push", "-u", "origin", "HEAD"]).await
        };
        match r {
            Ok(_) => msg.push_str(" Pushed."),
            Err(e) => msg.push_str(&format!(" Push failed: {e}")),
        }
    }
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    Ok(msg)
}

// ------------------------------------------------------------- pull requests

async fn session_repo(st: &AppState, conversation_id: &str) -> AppResult<std::path::PathBuf> {
    let cwd = conv_cwd(st, conversation_id)?;
    crate::gitops::repo_root(&cwd).await.ok_or_else(|| AppError::msg("This folder is not a git repository."))
}

/// What a pull request from the session's branch would contain.
#[tauri::command]
pub async fn pr_info(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let root = session_repo(&state, &conversation_id).await?;
    crate::pullrequest::info(&root).await
}

/// Title and description for the pull request, drafted by Claude.
#[tauri::command]
pub async fn pr_suggest(state: St<'_>, conversation_id: String, base: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let root = session_repo(&st, &conversation_id).await?;
    let input = crate::pullrequest::draft_input(&root, &base).await;
    let client = st.client()?;
    let model_id = st.settings.read().title_model.clone();
    let model = models::info(&model_id);
    let mut body = json!({
        "model": model_id,
        "max_tokens": 2048,
        "system": crate::pullrequest::PR_PROMPT,
        "messages": [{"role": "user", "content": input}],
    });
    match model.thinking {
        models::Thinking::Disableable => body["thinking"] = json!({"type": "disabled"}),
        models::Thinking::BetweenTools => body["thinking"] = json!({"type": "between_tools"}),
        _ => {}
    }
    let v = tokio::time::timeout(std::time::Duration::from_secs(60), client.create(&body, &[]))
        .await
        .map_err(|_| AppError::msg("Drafting the description took too long."))??;
    let text: String = v["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
    let (title, body) = crate::pullrequest::split_draft(&text);
    Ok(json!({"title": title, "body": body}))
}

/// A pull request's details and diff (for `/review <number or URL>`).
#[tauri::command]
pub async fn pr_fetch(state: St<'_>, conversation_id: String, pr: String) -> AppResult<Value> {
    let root = session_repo(&state, &conversation_id).await?;
    crate::pullrequest::fetch(&root, &pr).await
}

/// Push and open the pull request (or return the host's compare page).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pr_create(app: AppHandle, state: St<'_>, conversation_id: String, title: String, body: String, base: String, draft: bool, new_branch: Option<String>) -> AppResult<Value> {
    let root = session_repo(&state, &conversation_id).await?;
    let r = crate::pullrequest::create(&root, &title, &body, &base, draft, new_branch.as_deref()).await;
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    r
}

// ------------------------------------------------------------- architecture map

#[tauri::command]
pub async fn arch_map(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let cwd = conv_cwd(&state, &conversation_id)?;
    let root = crate::gitops::repo_root(&cwd).await.unwrap_or(cwd);
    let mut v = tokio::task::spawn_blocking({
        let root = root.clone();
        move || crate::archmap::build(&root)
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))?;
    v["root"] = json!(root);
    Ok(v)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn slash_save(state: St<'_>, conversation_id: Option<String>, cwd: Option<String>, scope: String, name: String, description: String, body: String, overwrite: bool) -> AppResult<String> {
    let root = root_of(&state, conversation_id.as_deref(), cwd);
    let root = match root {
        Some(r) => Some(crate::gitops::repo_root(&r).await.unwrap_or(r)),
        None => None,
    };
    crate::slash::save(root.as_deref(), &scope, &name, &description, &body, overwrite).map(|p| p.to_string_lossy().to_string())
}

// ------------------------------------------------------------- OpenAI-compatible endpoint

/// Load the model list from the configured endpoint (empty when off).
#[tauri::command]
pub async fn oai_refresh(state: St<'_>) -> AppResult<Value> {
    let (base, key) = {
        let s = state.settings.read();
        (s.oai_base_url.trim().to_string(), s.oai_api_key.clone())
    };
    if base.is_empty() {
        state.oai_models.lock().clear();
        return Ok(json!([]));
    }
    let list = crate::oai::list_models(&base, &key).await;
    match list {
        Ok(l) => {
            *state.oai_models.lock() = l.clone();
            Ok(json!(l))
        }
        Err(e) => {
            state.oai_models.lock().clear();
            Err(e)
        }
    }
}

// ------------------------------------------------------------- usage overview

/// Cost of one response's usage with a model's prices (USD).
pub fn usage_cost(model: &str, u: &Value) -> f64 {
    let p = models::info(model).price;
    let g = |k: &str| u[k].as_u64().unwrap_or(0) as f64;
    (g("input_tokens") * p.0 + g("output_tokens") * p.1 + g("cache_read_input_tokens") * p.0 * 0.1 + g("cache_creation_input_tokens") * p.0 * 1.25) / 1_000_000.0
}

/// Tokens and cost across all conversations of the last `days` days.
#[tauri::command]
pub async fn usage_overview(state: St<'_>, days: Option<u32>) -> AppResult<Value> {
    use chrono::TimeZone;
    let days = days.unwrap_or(30).clamp(1, 365) as i64;
    let since = now_ms() - days * 86_400_000;
    let rows = state.db.assistant_usage_since(since)?;
    let mut by_day: std::collections::BTreeMap<String, (u64, u64, f64)> = std::collections::BTreeMap::new();
    let mut by_model: std::collections::HashMap<String, (u64, u64, f64, u64)> = std::collections::HashMap::new();
    let mut by_conv: std::collections::HashMap<String, (String, String, f64)> = std::collections::HashMap::new();
    let mut tot = (0u64, 0u64, 0.0f64, 0u64);
    for (at, conv_id, title, kind, conv_model, meta) in rows {
        let meta: Value = serde_json::from_str(&meta).unwrap_or_default();
        let model = meta["model"].as_str().unwrap_or(&conv_model).to_string();
        let mut cost = 0.0;
        let mut inp = 0;
        let mut out = 0;
        for key in ["usage", "subagent_usage"] {
            let u = &meta[key];
            if !u.is_object() {
                continue;
            }
            if !model.starts_with(crate::oai::PREFIX) {
                cost += usage_cost(&model, u);
            }
            inp += u["input_tokens"].as_u64().unwrap_or(0) + u["cache_read_input_tokens"].as_u64().unwrap_or(0) + u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            out += u["output_tokens"].as_u64().unwrap_or(0);
        }
        if inp == 0 && out == 0 {
            continue;
        }
        let day = chrono::Local.timestamp_millis_opt(at).single().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
        let d = by_day.entry(day).or_default();
        d.0 += inp;
        d.1 += out;
        d.2 += cost;
        let m = by_model.entry(model).or_default();
        m.0 += inp;
        m.1 += out;
        m.2 += cost;
        m.3 += 1;
        let c = by_conv.entry(conv_id).or_insert((title, kind, 0.0));
        c.2 += cost;
        tot.0 += inp;
        tot.1 += out;
        tot.2 += cost;
        tot.3 += 1;
    }
    let mut models_v: Vec<Value> = by_model
        .into_iter()
        .map(|(m, (i, o, c, n))| json!({"model": m, "name": m.strip_prefix(crate::oai::PREFIX).map(String::from).unwrap_or_else(|| models::info(&m).name), "input_tokens": i, "output_tokens": o, "cost_usd": c, "responses": n}))
        .collect();
    models_v.sort_by(|a, b| b["cost_usd"].as_f64().unwrap_or(0.0).total_cmp(&a["cost_usd"].as_f64().unwrap_or(0.0)));
    let mut convs: Vec<Value> = by_conv
        .into_iter()
        .map(|(id, (title, kind, c))| json!({"id": id, "title": title, "kind": kind, "cost_usd": c}))
        .collect();
    convs.sort_by(|a, b| b["cost_usd"].as_f64().unwrap_or(0.0).total_cmp(&a["cost_usd"].as_f64().unwrap_or(0.0)));
    convs.truncate(8);
    Ok(json!({
        "days": days,
        "by_day": by_day.into_iter().map(|(d, (i, o, c))| json!({"date": d, "input_tokens": i, "output_tokens": o, "cost_usd": c})).collect::<Vec<_>>(),
        "by_model": models_v,
        "top_conversations": convs,
        "totals": {"input_tokens": tot.0, "output_tokens": tot.1, "cost_usd": tot.2, "responses": tot.3},
    }))
}

#[cfg(test)]
mod usage_tests {
    #[test]
    fn cost_uses_model_prices() {
        let u = serde_json::json!({"input_tokens": 1_000_000, "output_tokens": 1_000_000});
        let p = crate::models::info("claude-opus-5-5").price;
        let c = super::usage_cost("claude-opus-5-5", &u);
        assert!((c - (p.0 + p.1)).abs() < 1e-9, "{c}");
        let cached = serde_json::json!({"cache_read_input_tokens": 1_000_000});
        assert!((super::usage_cost("claude-opus-5-5", &cached) - p.0 * 0.1).abs() < 1e-9);
    }
}

// ------------------------------------------------------------- HTML report

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A self-contained HTML report of a conversation: messages rendered as
/// Markdown, tool calls summarized, and for code sessions the changed files
/// and the cost.
pub async fn export_html(st: &AppState, conv: &crate::db::Conversation, path: &[crate::db::Message]) -> String {
    let opts = || crate::markdown::Opts { math: false, breaks: false };
    let title = if conv.title.is_empty() { "Conversation".to_string() } else { conv.title.clone() };
    let mut body = String::new();
    let mut tools = 0usize;
    let mut cost = 0.0f64;
    for m in path {
        if m.role == "assistant" {
            for key in ["usage", "subagent_usage"] {
                if m.meta[key].is_object() {
                    cost += usage_cost(m.meta["model"].as_str().unwrap_or(&conv.model), &m.meta[key]);
                }
            }
        }
        if render::is_tool_result_msg(m) {
            continue;
        }
        let blocks = m.content.as_array().cloned().unwrap_or_default();
        if m.role == "user" {
            let text: String = blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .filter(|t| !t.starts_with("<system-reminder>"))
                .collect::<Vec<_>>()
                .join("\n\n");
            if text.trim().is_empty() {
                continue;
            }
            body.push_str(&format!("<section class=\"user\"><div class=\"who\">You</div><div class=\"bubble\">{}</div></section>\n", html_escape(text.trim()).replace('\n', "<br>")));
            continue;
        }
        let mut parts = String::new();
        for b in &blocks {
            match b["type"].as_str() {
                Some("text") => parts.push_str(&crate::markdown::render(b["text"].as_str().unwrap_or(""), opts())),
                Some("tool_use") | Some("server_tool_use") => {
                    tools += 1;
                    let name = b["name"].as_str().unwrap_or("tool");
                    let input = &b["input"];
                    let arg = input["command"].as_str().or(input["file_path"].as_str()).or(input["pattern"].as_str()).or(input["query"].as_str()).or(input["description"].as_str()).unwrap_or("");
                    parts.push_str(&format!(
                        "<details class=\"tool\"><summary><b>{}</b> <code>{}</code></summary><pre>{}</pre></details>",
                        html_escape(name),
                        html_escape(&arg.chars().take(160).collect::<String>()),
                        html_escape(&crate::util::truncate_middle(&serde_json::to_string_pretty(input).unwrap_or_default(), 4000))
                    ));
                }
                _ => {}
            }
        }
        if !parts.trim().is_empty() {
            body.push_str(&format!("<section class=\"assistant\"><div class=\"who\">Claude</div>{parts}</section>\n"));
        }
    }
    let mut meta = vec![
        format!("Model: {}", html_escape(&models::info(&conv.model).name)),
        format!("{} messages", path.iter().filter(|m| !render::is_tool_result_msg(m)).count()),
    ];
    if tools > 0 {
        meta.push(format!("{tools} tool calls"));
    }
    if cost > 0.0 {
        meta.push(format!("estimated cost ${cost:.2}"));
    }
    let mut changes = String::new();
    if conv.kind == "code" {
        if let Some(cwd) = &conv.cwd {
            meta.push(format!("Folder: <code>{}</code>", html_escape(cwd)));
            let s = crate::codetools::CodeSession::new(std::path::PathBuf::from(cwd));
            if let Ok(c) = crate::gitops::changes(&s).await {
                let files = c["files"].as_array().cloned().unwrap_or_default();
                if !files.is_empty() {
                    changes.push_str("<h2>Uncommitted changes</h2><ul class=\"changes\">");
                    for f in files.iter().take(200) {
                        changes.push_str(&format!(
                            "<li><span class=\"st\">{}</span> <code>{}</code> <span class=\"add\">+{}</span> <span class=\"del\">-{}</span></li>",
                            html_escape(f["status"].as_str().unwrap_or("")),
                            html_escape(f["path"].as_str().unwrap_or("")),
                            f["added"].as_u64().unwrap_or(0),
                            f["removed"].as_u64().unwrap_or(0)
                        ));
                    }
                    changes.push_str("</ul>");
                }
            }
        }
    }
    let _ = st;
    let date = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    format!(r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{t}</title>
<style>
:root {{ --bg:#faf9f5; --fg:#1f1e1d; --muted:#6b6a68; --border:#e5e2d9; --accent:#c96442; --code:#f0eee6; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#1f1e1d; --fg:#ecebe6; --muted:#a3a19c; --border:#3a3936; --code:#2a2927; }} }}
body {{ margin:0; background:var(--bg); color:var(--fg); font:15px/1.6 -apple-system,"Segoe UI",system-ui,sans-serif; }}
main {{ max-width:820px; margin:0 auto; padding:32px 20px 60px; }}
h1 {{ font-size:24px; margin:0 0 6px; }} h2 {{ font-size:17px; margin-top:28px; }}
.meta {{ color:var(--muted); font-size:13px; margin-bottom:24px; }} .meta span+span:before {{ content:" · "; }}
section {{ margin:18px 0; }} .who {{ font-size:12px; font-weight:600; color:var(--muted); text-transform:uppercase; letter-spacing:.04em; margin-bottom:4px; }}
.user .bubble {{ background:var(--code); border-radius:12px; padding:10px 14px; display:inline-block; max-width:100%; }}
pre {{ background:var(--code); padding:10px 12px; border-radius:8px; overflow:auto; font-size:12.5px; }}
code {{ font-family:ui-monospace,Menlo,Consolas,monospace; font-size:.9em; }}
details.tool {{ border:1px solid var(--border); border-radius:8px; padding:6px 10px; margin:8px 0; font-size:13px; }}
details.tool summary {{ cursor:pointer; }} table {{ border-collapse:collapse; }} td,th {{ border:1px solid var(--border); padding:4px 8px; }}
.changes {{ list-style:none; padding:0; font-size:13px; }} .changes .st {{ display:inline-block; width:14px; font-weight:600; color:var(--accent); }}
.add {{ color:#3a9a5b; }} .del {{ color:#c0453a; }} a {{ color:var(--accent); }}
footer {{ color:var(--muted); font-size:12px; margin-top:40px; }}
button {{ display:none; }} .code-block .lang {{ font-size:12px; color:var(--muted); }}
{hl}
</style></head>
<body><main><h1>{t}</h1><div class="meta">{m}</div>{body}{changes}<footer>Exported from ClaudeRust · {date}</footer></main></body></html>
"#, hl = crate::highlight::theme_css(), t = html_escape(&title), m = meta.into_iter().map(|x| format!("<span>{x}</span>")).collect::<String>())
}

// ------------------------------------------------------------- git history

/// Recent commits of the session's repository.
#[tauri::command]
pub async fn git_log(state: St<'_>, conversation_id: String, limit: Option<u32>) -> AppResult<Value> {
    let cwd = conv_cwd(&state, &conversation_id)?;
    let root = crate::gitops::repo_root(&cwd).await.ok_or_else(|| AppError::msg("This folder is not a git repository."))?;
    let n = limit.unwrap_or(60).clamp(1, 500).to_string();
    let out = crate::gitops::git(&root, &["log", "-n", &n, "--date=unix", "--pretty=format:%H%x1f%h%x1f%an%x1f%at%x1f%s%x1f%D%x1e"]).await.unwrap_or_default();
    let commits: Vec<Value> = out
        .split('\x1e')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_matches(['\n', '\r']).split('\x1f').collect();
            (f.len() >= 5).then(|| json!({
                "sha": f[0], "short": f[1], "author": f[2], "time": f[3].parse::<i64>().unwrap_or(0) * 1000,
                "subject": f[4], "refs": f.get(5).copied().unwrap_or(""),
            }))
        })
        .collect();
    let branch = crate::gitops::git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap_or_default();
    Ok(json!({"branch": branch.trim(), "commits": commits}))
}

fn valid_sha(sha: &str) -> bool {
    (4..=64).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit())
}

/// One commit's patch, split per file.
#[tauri::command]
pub async fn git_show(state: St<'_>, conversation_id: String, sha: String) -> AppResult<Value> {
    if !valid_sha(&sha) {
        return Err(AppError::msg("Invalid commit id."));
    }
    let cwd = conv_cwd(&state, &conversation_id)?;
    let root = crate::gitops::repo_root(&cwd).await.ok_or_else(|| AppError::msg("This folder is not a git repository."))?;
    let body = crate::gitops::git(&root, &["show", "-s", "--format=%B", &sha]).await.unwrap_or_default();
    let patch = crate::gitops::git(&root, &["show", "--format=", "--no-color", "--no-ext-diff", "--patch", &sha]).await?;
    let patch = crate::util::truncate_middle(&patch, 400_000);
    let mut files: Vec<Value> = vec![];
    for chunk in patch.split("\ndiff --git ").enumerate().map(|(i, c)| if i == 0 { c.trim_start_matches("diff --git ").to_string() } else { c.to_string() }) {
        if chunk.trim().is_empty() {
            continue;
        }
        let first = chunk.lines().next().unwrap_or("");
        let path = first.rsplit(" b/").next().unwrap_or(first).to_string();
        let (mut add, mut del) = (0, 0);
        for l in chunk.lines() {
            if l.starts_with('+') && !l.starts_with("+++") {
                add += 1;
            } else if l.starts_with('-') && !l.starts_with("---") {
                del += 1;
            }
        }
        files.push(json!({"path": path, "added": add, "removed": del, "diff": chunk}));
    }
    Ok(json!({"sha": sha, "message": body.trim(), "files": files}))
}

/// Revert a commit with a new commit.
#[tauri::command]
pub async fn git_revert(app: AppHandle, state: St<'_>, conversation_id: String, sha: String) -> AppResult<String> {
    if !valid_sha(&sha) {
        return Err(AppError::msg("Invalid commit id."));
    }
    let cwd = conv_cwd(&state, &conversation_id)?;
    let root = crate::gitops::repo_root(&cwd).await.ok_or_else(|| AppError::msg("This folder is not a git repository."))?;
    let r = crate::gitops::git(&root, &["revert", "--no-edit", &sha]).await;
    if r.is_err() {
        let _ = crate::gitops::git(&root, &["revert", "--abort"]).await;
    }
    r?;
    let head = crate::gitops::git(&root, &["rev-parse", "--short", "HEAD"]).await.unwrap_or_default();
    let _ = app.emit("files-changed", json!({"conversation_id": conversation_id}));
    Ok(format!("Reverted in a new commit {}.", head.trim()))
}

// ------------------------------------------------------------- /context

/// What fills the context window of a conversation, by category (token
/// estimates from text length, scaled to the last real count when known).
#[tauri::command]
pub async fn context_breakdown(state: St<'_>, conversation_id: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let conv = st.db.conv_get(&conversation_id)?;
    let settings = st.settings.read().clone();
    let path = match &conv.current_leaf {
        Some(l) => st.db.path_to(l)?,
        None => vec![],
    };
    let est = |s: &str| (s.chars().count() as f64 / 3.6).ceil() as u64;
    let mut cats: Vec<(String, u64)> = vec![];
    if conv.model.starts_with(crate::oai::PREFIX) || conv.options["engine"] == crate::claudecode::ENGINE {
        // not built by us; only the totals are known
    } else {
        let rc = engine::build_request_context(&st, &conv, &settings).await?;
        let memory = conv.options["memory"].as_str().unwrap_or("");
        let sys_total = est(&rc.system);
        let mem = est(memory).min(sys_total);
        cats.push(("system".into(), sys_total - mem));
        if mem > 0 {
            cats.push(("memory".into(), mem));
        }
        let mcp: u64 = rc.tools.iter().filter(|t| rc.mcp_names.contains_key(t["name"].as_str().unwrap_or(""))).map(|t| est(&t.to_string())).sum();
        let all_tools: u64 = rc.tools.iter().map(|t| est(&t.to_string())).sum();
        cats.push(("tools".into(), all_tools - mcp));
        if mcp > 0 {
            cats.push(("mcp_tools".into(), mcp));
        }
        let msgs = crate::compact::apply(&path);
        let (mut conv_t, mut tool_t, mut summary_t) = (0u64, 0u64, 0u64);
        for m in &msgs {
            if let Some(sm) = m.meta["compact_summary"].as_str() {
                summary_t += est(sm);
            }
            for b in m.content.as_array().into_iter().flatten() {
                let n = est(&b.to_string());
                match b["type"].as_str() {
                    Some("tool_use") | Some("tool_result") | Some("server_tool_use") | Some("web_search_tool_result") => tool_t += n,
                    Some("image") => conv_t += 1600,
                    _ => conv_t += n,
                }
            }
        }
        cats.push(("messages".into(), conv_t));
        cats.push(("tool_calls".into(), tool_t));
        if summary_t > 0 {
            cats.push(("summary".into(), summary_t));
        }
    }
    let estimated: u64 = cats.iter().map(|c| c.1).sum();
    let start = crate::compact::start_index(&path).unwrap_or(0);
    let actual = crate::compact::context_tokens(&path[start..]);
    // scale estimates to the real count of the last response when we have it
    let scale = match actual {
        Some(a) if estimated > 0 => a as f64 / estimated as f64,
        _ => 1.0,
    };
    let window = models::info(&conv.model).context as u64;
    Ok(json!({
        "categories": cats.iter().map(|(k, v)| json!({"name": k, "tokens": ((*v as f64) * scale).round() as u64})).collect::<Vec<_>>(),
        "used": actual.unwrap_or(estimated),
        "measured": actual.is_some(),
        "window": window,
        "auto_compact_pct": settings.auto_compact_pct,
    }))
}

// ------------------------------------------------------------- open in editor

/// The editor command and its arguments for a file (and line).
pub fn editor_args(editor: &str, path: &str, line: Option<u32>) -> Option<(String, Vec<String>)> {
    let with_line = |p: &str| match line {
        Some(l) => format!("{p}:{l}"),
        None => p.to_string(),
    };
    let found = |b: &str| crate::shellenv::which(b);
    let pick = match editor {
        "auto" => ["code", "cursor", "windsurf", "zed", "subl"].into_iter().find(|b| found(b)).map(|b| b.to_string()),
        "system" | "" => None,
        other => Some(other.to_string()),
    }?;
    let args = match pick.as_str() {
        "code" | "cursor" | "windsurf" | "codium" => vec!["-g".to_string(), with_line(path)],
        "idea" | "pycharm" | "webstorm" | "goland" | "rustrover" | "clion" => match line {
            Some(l) => vec!["--line".to_string(), l.to_string(), path.to_string()],
            None => vec![path.to_string()],
        },
        _ => vec![with_line(path)],
    };
    Some((pick, args))
}

#[tauri::command]
pub async fn open_in_editor(app: AppHandle, state: St<'_>, path: String, line: Option<u32>) -> AppResult<String> {
    let editor = state.settings.read().editor.clone();
    if let Some((bin, args)) = editor_args(&editor, &path, line) {
        let mut cmd = crate::shellenv::command(&bin, &args);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        if cmd.spawn().is_ok() {
            return Ok(bin);
        }
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(&path, None::<&str>).map_err(|e| AppError::msg(e.to_string()))?;
    Ok("system".into())
}

#[cfg(test)]
mod editor_tests {
    #[test]
    fn editor_arguments() {
        let (b, a) = super::editor_args("code", "/p/a.rs", Some(12)).unwrap();
        assert_eq!((b.as_str(), a), ("code", vec!["-g".to_string(), "/p/a.rs:12".to_string()]));
        let (_, a) = super::editor_args("idea", "/p/a.rs", Some(3)).unwrap();
        assert_eq!(a, vec!["--line", "3", "/p/a.rs"]);
        assert!(super::editor_args("system", "/p/a.rs", None).is_none());
    }
}

// ------------------------------------------------------------- fork into a new conversation

/// Copy the conversation up to (and including) `message_id` into a new
/// conversation, so an alternative can be explored on its own.
#[tauri::command]
pub async fn fork_conversation(app: AppHandle, state: St<'_>, conversation_id: String, message_id: String) -> AppResult<Value> {
    let st = state.inner().clone();
    let conv = st.db.conv_get(&conversation_id)?;
    let path = st.db.path_to(&message_id)?;
    if path.is_empty() || path.last().map(|m| m.conversation_id != conversation_id).unwrap_or(true) {
        return Err(AppError::msg("Message not found in this conversation."));
    }
    let mut options = conv.options.clone();
    if let Some(m) = options.as_object_mut() {
        for k in ["cc_session", "cc_tip", "pending_compact", "worktree", "watch_enabled", "imported_from", "routine_id"] {
            m.remove(k);
        }
    }
    let base = if conv.title.is_empty() { "Conversation" } else { conv.title.trim_end_matches(" (fork)") };
    let title = format!("{base} (fork)");
    // A worktree belongs to its session and goes away with it: the fork
    // works in the folder the original session was started in.
    let cwd = conv.options["worktree"]["orig_cwd"]
        .as_str()
        .filter(|d| std::path::Path::new(d).is_dir())
        .map(str::to_string)
        .or(conv.cwd.clone());
    let new = st.db.conv_create(&conv.kind, &conv.model, conv.project_id.as_deref(), cwd.as_deref(), &options)?;
    // the CLI finds its sessions by folder: resume only in the same one
    let same_folder = cwd == conv.cwd;
    for (id, content) in st.db.copy_path(&path, &new.id, same_folder)? {
        let _ = st.db.fts_index(&new.id, &id, &render::plain_text(&content));
    }
    st.db.conv_set_title(&new.id, &title)?;
    engine::emit_conv_updated(&app, &new.id);
    Ok(json!({"conversation_id": new.id}))
}
