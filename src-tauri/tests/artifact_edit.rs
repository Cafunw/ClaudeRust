//! A typed artifact edited in place (`artifact_edit`): the person's content
//! becomes the newest version on the branch shown, in the last reply (where
//! Claude reads it, marked theirs), the conversation's artifacts and the
//! gallery; what can't be edited is refused with a code.

use clauderust_lib::artifacts::artifact_edit;
use clauderust_lib::commands;
use clauderust_lib::db::{Db, Message};
use clauderust_lib::state::AppState;
use clauderust_lib::util::{new_id, now_ms};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Once};
use tauri::test::MockRuntime;
use tauri::Manager;

fn app() -> (tauri::App<MockRuntime>, Arc<AppState>) {
    static ENV: Once = Once::new();
    // a fake key: the app's keyring is never read
    ENV.call_once(|| std::env::set_var("CLAUDERUST_API_KEY", "test-key"));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("artifact-edit");
    let st = Arc::new(AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
    let app = tauri::test::mock_app();
    app.manage(st.clone());
    (app, st)
}

fn add(st: &AppState, conv: &str, parent: Option<&str>, role: &str, content: Value) -> String {
    let id = new_id();
    st.db
        .msg_insert(&Message { id: id.clone(), conversation_id: conv.into(), parent_id: parent.map(Into::into), role: role.into(), content, meta: json!({}), created_at: now_ms() })
        .unwrap();
    st.db.conv_set_leaf(conv, Some(&id)).unwrap();
    id
}

/// A chat whose reply made a sheet and a web page, and whose next reply only talked.
fn chat(st: &AppState) -> (String, String) {
    let conv = st.db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap().id;
    let u = add(st, &conv, None, "user", json!([{"type": "text", "text": "Make a budget"}]));
    let first = "Here.\n<antArtifact identifier=\"budget\" type=\"application/vnd.ant.sheet\" title=\"Budget\">\nItem,Cost\nRent,1000\n</antArtifact>\n<antArtifact identifier=\"page\" type=\"text/html\" title=\"Page\"><p>x</p></antArtifact>";
    let a = add(st, &conv, Some(&u), "assistant", json!([{"type": "text", "text": first}]));
    let u2 = add(st, &conv, Some(&a), "user", json!([{"type": "text", "text": "Thanks"}]));
    let last = add(st, &conv, Some(&u2), "assistant", json!([{"type": "thinking", "thinking": "t", "signature": "s"}, {"type": "text", "text": "You're welcome."}]));
    (conv, last)
}

fn edit(app: &tauri::App<MockRuntime>, conv: &str, id: &str, content: &str) -> Result<(), String> {
    tauri::async_runtime::block_on(artifact_edit(app.state(), conv.into(), id.into(), content.into())).map_err(|e| e.kind().to_string())
}

#[test]
fn an_edit_is_the_newest_version_in_the_last_reply_claude_reads() {
    let (app, st) = app();
    let (conv, last) = chat(&st);
    edit(&app, &conv, "budget", "Item,Cost\nRent,900\nFood,=SUM(B2:B2)").unwrap();
    // the last reply keeps what it had (its thinking block untouched) and gains the version, marked the person's
    let reply = st.db.msg_get(&last).unwrap().content;
    assert_eq!(reply[0], json!({"type": "thinking", "thinking": "t", "signature": "s"}));
    assert_eq!(reply[1]["text"], "You're welcome.");
    let added = reply[2]["text"].as_str().unwrap();
    assert!(added.starts_with("<antArtifact identifier=\"budget\" type=\"application/vnd.ant.sheet\" title=\"Budget\" source=\"user\">"), "{added}");
    // the conversation lists it as version 2, after the reply's version 1
    let view = tauri::async_runtime::block_on(commands::get_conversation(app.state(), conv.clone(), None, None, None)).unwrap();
    let versions: Vec<(u64, String)> = view["turns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["artifacts"].as_array().cloned().unwrap_or_default())
        .filter(|a| a["identifier"] == "budget")
        .map(|a| (a["version"].as_u64().unwrap(), a["content"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(versions, [(1, "Item,Cost\nRent,1000".to_string()), (2, "Item,Cost\nRent,900\nFood,=SUM(B2:B2)".to_string())]);
    // the gallery shows the newest
    let gallery = tauri::async_runtime::block_on(commands::list_artifacts(app.state())).unwrap();
    let budget = gallery.as_array().unwrap().iter().find(|a| a["identifier"] == "budget").unwrap();
    assert_eq!(budget["content"], "Item,Cost\nRent,900\nFood,=SUM(B2:B2)");
    // a second edit builds on the first, as an update from Claude would
    edit(&app, &conv, "budget", "Item,Cost\nRent,800").unwrap();
    let texts: Vec<String> = st.db.path_to(&last).unwrap().iter().filter(|m| m.role == "assistant").map(|m| clauderust_lib::render::plain_text(&m.content)).collect();
    assert_eq!(clauderust_lib::artifacts::latest_version(texts, "budget").unwrap().content, "Item,Cost\nRent,800");
}

#[test]
fn what_cant_be_edited_is_refused_and_nothing_changes() {
    let (app, st) = app();
    let (conv, last) = chat(&st);
    let before = st.db.msg_get(&last).unwrap().content;
    assert_eq!(edit(&app, &conv, "page", "<p>y</p>"), Err("artifact_edit_untyped".into()), "a web page is Claude's to rewrite");
    assert_eq!(edit(&app, &conv, "nothing", "x"), Err("artifact_edit_missing".into()));
    assert_eq!(edit(&app, &conv, "budget", &"x".repeat(3 * 1024 * 1024)), Err("artifact_edit_too_large".into()));
    st.runs.lock().insert(conv.clone(), Default::default());
    assert_eq!(edit(&app, &conv, "budget", "a,b"), Err("artifact_edit_busy".into()), "not while Claude is answering");
    st.runs.lock().remove(&conv);
    assert_eq!(st.db.msg_get(&last).unwrap().content, before);
    // a conversation without messages has nothing to edit
    let empty = st.db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap().id;
    assert_eq!(edit(&app, &empty, "budget", "a"), Err("artifact_edit_missing".into()));
}
