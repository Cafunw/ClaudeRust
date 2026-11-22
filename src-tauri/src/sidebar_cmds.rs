//! The sidebar's own organisation of conversations: pins, the archive,
//! unread marks and sections (groups in the Code tab).

use crate::db::{Conversation, Db, SearchHit, Section};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Runtime, State};

type St<'a> = State<'a, Arc<AppState>>;

/// The conversation on screen (`conversation_opened`): a turn ending there leaves it read.
static OPEN: Mutex<Option<String>> = Mutex::new(None);

/// A turn of `conv_id` ended (engine.rs): unread, unless it's the one on screen.
pub fn turn_finished(st: &AppState, conv_id: &str) {
    mark_unless_open(&st.db, conv_id, OPEN.lock().as_deref());
}

fn mark_unless_open(db: &Db, conv_id: &str, open: Option<&str>) {
    if open != Some(conv_id) {
        let _ = db.conv_set_unread(conv_id, true);
    }
}

fn changed<R: Runtime>(app: &AppHandle<R>, st: &AppState, id: &str) -> AppResult<Conversation> {
    crate::engine::emit_conv_updated(app, id);
    st.db.conv_get(id)
}

#[tauri::command]
pub async fn conversation_set_pinned<R: Runtime>(app: AppHandle<R>, state: St<'_>, id: String, pinned: bool) -> AppResult<Conversation> {
    state.db.conv_set_pinned(&id, pinned)?;
    changed(&app, &state, &id)
}

#[tauri::command]
pub async fn conversation_set_archived<R: Runtime>(app: AppHandle<R>, state: St<'_>, id: String, archived: bool) -> AppResult<Conversation> {
    state.db.conv_set_archived(&id, archived)?;
    changed(&app, &state, &id)
}

/// "Mark as unread" (and back).
#[tauri::command]
pub async fn conversation_set_unread<R: Runtime>(app: AppHandle<R>, state: St<'_>, id: String, unread: bool) -> AppResult<Conversation> {
    state.db.conv_set_unread(&id, unread)?;
    changed(&app, &state, &id)
}

/// Into a section of the conversation's kind, or (`None`) out of any.
#[tauri::command]
pub async fn conversation_set_section<R: Runtime>(app: AppHandle<R>, state: St<'_>, id: String, section_id: Option<String>) -> AppResult<Conversation> {
    state.db.conv_set_section(&id, section_id.as_deref())?;
    changed(&app, &state, &id)
}

/// The UI shows conversation `id` (None: none, another page is open): it is read.
#[tauri::command]
pub async fn conversation_opened<R: Runtime>(app: AppHandle<R>, state: St<'_>, id: Option<String>) -> AppResult<()> {
    *OPEN.lock() = id.clone();
    if let Some(id) = id.filter(|id| state.db.conv_get(id).is_ok_and(|c| c.unread)) {
        state.db.conv_set_unread(&id, false)?;
        crate::engine::emit_conv_updated(&app, &id);
    }
    Ok(())
}

/// The archived conversations, as `list_conversations` lists the others.
#[tauri::command]
pub async fn list_archived_conversations(state: St<'_>, kind: Option<String>) -> AppResult<Vec<Value>> {
    let st = state.inner().clone();
    let list = tauri::async_runtime::spawn_blocking(move || st.db.conv_list_archived(kind.as_deref())).await.map_err(|e| AppError::msg(e.to_string()))??;
    Ok(list.iter().map(entry).collect())
}

fn entry(c: &Conversation) -> Value {
    json!({
        "id": c.id, "kind": c.kind, "title": c.title, "model": c.model,
        "project_id": c.project_id, "cwd": c.cwd, "starred": c.starred,
        "created_at": c.created_at, "updated_at": c.updated_at,
        "pinned_at": c.pinned_at, "archived_at": c.archived_at, "unread": c.unread, "section_id": c.section_id,
    })
}

/// `search`, with the archived conversations too when `include_archived`.
#[tauri::command]
pub async fn search_conversations(state: St<'_>, query: String, include_archived: bool) -> AppResult<Vec<SearchHit>> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || st.db.search_in(&query, 40, include_archived)).await.map_err(|e| AppError::msg(e.to_string()))?
}

fn kind_of(kind: &str) -> AppResult<&str> {
    match kind {
        "chat" | "code" => Ok(kind),
        _ => Err(AppError::msg("Sections hold chats or code sessions.")),
    }
}

fn section_name(name: &str) -> AppResult<String> {
    let n = name.trim();
    match n.chars().count() {
        0 => Err(crate::memory_synth::coded("section_name_empty", "Give the section a name.")),
        1..=80 => Ok(n.to_string()),
        _ => Err(crate::memory_synth::coded("section_name_long", "Section names are at most 80 characters.")),
    }
}

#[tauri::command]
pub async fn sections_list(state: St<'_>, kind: String) -> AppResult<Vec<Section>> {
    state.db.section_list(kind_of(&kind)?)
}

#[tauri::command]
pub async fn section_create(state: St<'_>, kind: String, name: String) -> AppResult<Section> {
    state.db.section_create(kind_of(&kind)?, &section_name(&name)?)
}

#[tauri::command]
pub async fn section_rename(state: St<'_>, id: String, name: String) -> AppResult<()> {
    state.db.section_rename(&id, &section_name(&name)?)
}

/// Its conversations stay, outside any section.
#[tauri::command]
pub async fn section_delete(state: St<'_>, id: String) -> AppResult<()> {
    state.db.section_delete(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Manager;

    #[test]
    fn a_turn_ending_elsewhere_marks_the_conversation_unread() {
        let db = Db::open_in_memory().unwrap();
        let a = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        mark_unless_open(&db, &a.id, Some(&a.id));
        assert!(!db.conv_get(&a.id).unwrap().unread, "on screen: read");
        mark_unless_open(&db, &a.id, Some("other"));
        assert!(db.conv_get(&a.id).unwrap().unread);
        db.conv_set_unread(&a.id, false).unwrap();
        mark_unless_open(&db, &a.id, None);
        assert!(db.conv_get(&a.id).unwrap().unread, "no conversation open");
    }

    #[test]
    fn commands_check_what_they_are_given() {
        let dir = std::env::temp_dir().join(format!("cr-sidebar-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = Arc::new(AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
        let app = tauri::test::mock_app();
        app.manage(st.clone());
        fn run<F: std::future::Future>(f: F) -> F::Output {
            tauri::async_runtime::block_on(f)
        }
        assert!(run(section_create(app.state(), "project".into(), "x".into())).is_err());
        assert!(run(section_create(app.state(), "chat".into(), "  ".into())).is_err());
        assert!(run(section_create(app.state(), "chat".into(), "x".repeat(81))).is_err());
        let s = run(section_create(app.state(), "chat".into(), " Work ".into())).unwrap();
        assert_eq!(s.name, "Work");
        assert!(run(section_rename(app.state(), s.id.clone(), "".into())).is_err());
        let c = st.db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let got = run(conversation_set_section(app.handle().clone(), app.state(), c.id.clone(), Some(s.id.clone()))).unwrap();
        assert_eq!(got.section_id, Some(s.id.clone()));
        let got = run(conversation_set_archived(app.handle().clone(), app.state(), c.id.clone(), true)).unwrap();
        assert!(got.archived_at.is_some());
        let listed = run(list_archived_conversations(app.state(), Some("chat".into()))).unwrap();
        assert_eq!((listed.len(), listed[0]["section_id"].as_str()), (1, Some(s.id.as_str())));
        assert!(listed[0].get("options").is_none(), "no options in a list");
        st.db.conv_set_unread(&c.id, true).unwrap();
        run(conversation_opened(app.handle().clone(), app.state(), Some(c.id.clone()))).unwrap();
        assert!(!st.db.conv_get(&c.id).unwrap().unread, "opening it reads it");
        run(conversation_opened(app.handle().clone(), app.state(), None)).unwrap();
        assert!(run(conversation_set_pinned(app.handle().clone(), app.state(), "gone".into(), true)).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
