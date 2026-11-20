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
use tauri::{AppHandle, Manager, State};
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
        let cwd = req.cwd.clone().ok_or_else(|| AppError::msg("Choose a folder for the coding session."))?;
        let p = std::path::PathBuf::from(&cwd);
        if !p.is_dir() {
            return Err(AppError::msg(format!("Folder not found: {cwd}")));
        }
        options["env_snapshot"] = json!(crate::codetools::env_snapshot(&p, &model));
        options["memory"] = json!(crate::codetools::load_memory(&p));
        if options["permission_mode"].is_null() {
            options["permission_mode"] = json!(s.code_permission_mode);
        }
    }
    st.db.conv_create(&kind, &model, req.project_id.as_deref(), req.cwd.as_deref(), &options)
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
    let st = state.inner().clone();
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
    let msg = Message {
        id: new_id(),
        conversation_id: conv.id.clone(),
        parent_id: parent,
        role: "user".into(),
        content: Value::Array(content),
        meta: json!({
            "attachments": atts.iter().map(attachments::display_meta).collect::<Vec<_>>(),
            "plan_mode": plan,
        }),
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
    st.db.conv_delete(&id)?;
    st.code_sessions.lock().remove(&id);
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
    std::fs::write(&st.mcp.config_path, serde_json::to_string_pretty(&v)?)?;
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
    for m in path.iter().filter(|m| m.role == "assistant") {
        let u = &m.meta["usage"];
        let model = models::info(m.meta["model"].as_str().unwrap_or(&conv.model));
        let i = u["input_tokens"].as_u64().unwrap_or(0);
        let o = u["output_tokens"].as_u64().unwrap_or(0);
        let cr = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
        let cw = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        tot.0 += i;
        tot.1 += o;
        tot.2 += cr;
        tot.3 += cw;
        tot.4 += (i as f64 * model.price.0 + o as f64 * model.price.1 + cr as f64 * model.price.0 * 0.1 + cw as f64 * model.price.0 * 1.25) / 1_000_000.0;
    }
    Ok(json!({"input_tokens": tot.0, "output_tokens": tot.1, "cache_read_input_tokens": tot.2, "cache_creation_input_tokens": tot.3, "cost_usd": tot.4}))
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
