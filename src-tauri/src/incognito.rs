//! Incognito chats. Their rows exist only while one is open (the engines
//! read and write them like any chat's); every listing, search, usage total,
//! artifact gallery and memory leaves them out (db.rs), and nothing of them
//! stays once closed: closing one goes through delete_conversation (its rows,
//! CLI transcripts and prompt file), and `sweep` removes those a quit or a
//! crash left, with the files their code execution made.

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, State};

/// The incognito chat `id` was closed (another one opened, the window
/// closed): it is deleted. Refuses any other chat.
#[tauri::command]
pub async fn close_incognito(app: AppHandle, state: State<'_, Arc<AppState>>, id: String) -> AppResult<()> {
    closable(&state.db, &id)?;
    crate::commands::delete_conversation(app, state, id).await
}

fn closable(db: &Db, id: &str) -> AppResult<()> {
    if !db.conv_get(id)?.incognito {
        return Err(AppError::msg("Only an incognito chat is removed when it's closed."));
    }
    Ok(())
}

/// Remove every incognito chat still stored, with what the account's CLI
/// kept of it: at start (a crash's leftovers, before any turn runs) and at
/// exit (once the turns stopped). Returns how many went.
pub fn sweep(db: &Db, data_dir: &Path) -> usize {
    let mut gone = 0;
    for id in db.incognito_ids().unwrap_or_default() {
        let Ok(conv) = db.conv_get(&id) else { continue };
        // read while its messages are there; a session another chat still uses is kept
        let sessions = crate::claudecode::chat_sessions(db, &conv).unwrap_or_default();
        if let Err(e) = db.conv_delete(&id) {
            log::warn!("removing an incognito chat failed: {e}");
            continue;
        }
        let unused = crate::claudecode::unreferenced(db, sessions).unwrap_or_default();
        crate::claudecode::remove_chat_files(data_dir, &id, &unused);
        crate::code_exec::remove_outputs(data_dir, &id);
        gone += 1;
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_an_incognito_chat_is_closed() {
        let db = Db::open_in_memory().unwrap();
        let chat = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let inc = db.conv_create("chat", "m", None, None, &json!({"incognito": true})).unwrap();
        assert!(closable(&db, &chat.id).is_err());
        assert!(closable(&db, "gone").is_err());
        assert!(closable(&db, &inc.id).is_ok());
    }
}
