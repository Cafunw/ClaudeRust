//! An incognito chat that a crash left behind: the next start removes it,
//! with the CLI transcripts and the prompt file the account engine kept of
//! it, and nothing of the other chats.

use clauderust_lib::db::{Db, Message};
use clauderust_lib::util::new_id;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The CLI's folder for the transcripts of sessions run in `cwd` (as claudecode.rs names it).
fn transcripts(config: &Path, cwd: &Path) -> PathBuf {
    let name: String = cwd.to_string_lossy().chars().flat_map(|c| if c.is_ascii_alphanumeric() { vec![c] } else { vec!['-'; c.len_utf16()] }).collect();
    config.join("projects").join(name)
}

fn answered(db: &Db, conv: &str, session: &str) {
    let m = Message { id: new_id(), conversation_id: conv.into(), parent_id: None, role: "user".into(), content: json!([{"type": "text", "text": "hello"}]), meta: json!({}), created_at: 1 };
    let a = Message { id: new_id(), parent_id: Some(m.id.clone()), role: "assistant".into(), meta: json!({"cc_session": session, "stop_reason": "end_turn"}), ..m.clone() };
    db.msg_insert(&m).unwrap();
    db.msg_insert(&a).unwrap();
    db.conv_set_leaf(conv, Some(&a.id)).unwrap();
}

#[test]
fn a_crash_leaves_no_incognito_chat_behind() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("incognito-{}", new_id()));
    let config = root.join("claude-config");
    // the CLI's folder is this test's own, never the person's (the only test in this binary)
    std::env::set_var("CLAUDE_CONFIG_DIR", &config);
    let data = root.join("data");
    let chat_cli = data.join("chat-cli");
    let dir = transcripts(&config, &chat_cli);
    std::fs::create_dir_all(dir.join("s-inc").join("subagents")).unwrap();
    std::fs::create_dir_all(&chat_cli).unwrap();
    for s in ["s-inc", "s-shared", "s-normal"] {
        std::fs::write(dir.join(format!("{s}.jsonl")), "{}").unwrap();
    }
    let (normal, inc, inc_fork) = {
        let db = Db::open(&data.join("clauderust.db")).unwrap();
        let normal = db.conv_create("chat", "m", None, None, &json!({"cc_session": "s-normal"})).unwrap();
        answered(&db, &normal.id, "s-shared");
        let inc = db.conv_create("chat", "m", None, None, &json!({"incognito": true, "cc_session": "s-inc"})).unwrap();
        answered(&db, &inc.id, "s-inc");
        answered(&db, &inc.id, "s-shared");
        // a fork of it is incognito too, and resumes the same session
        let mut o: Value = db.conv_get(&inc.id).unwrap().options;
        o["cc_session"] = Value::Null;
        let inc_fork = db.conv_create("chat", "m", None, None, &o).unwrap();
        assert!(inc_fork.incognito);
        answered(&db, &inc_fork.id, "s-inc");
        std::fs::write(chat_cli.join(format!("system-{}.txt", inc.id)), "the prompt").unwrap();
        // a file its code execution made
        std::fs::create_dir_all(data.join("outputs").join(&inc.id)).unwrap();
        std::fs::write(data.join("outputs").join(&inc.id).join("chart.png"), "png").unwrap();
        // the app dies here: the chat was never closed, no exit cleanup ran
        (normal.id, inc.id, inc_fork.id)
    };
    let db = Db::open(&data.join("clauderust.db")).unwrap();
    assert_eq!(clauderust_lib::incognito::sweep(&db, &data), 2);
    assert!(db.conv_get(&inc).is_err() && db.conv_get(&inc_fork).is_err());
    assert!(db.conv_get(&normal).is_ok());
    assert!(!dir.join("s-inc.jsonl").exists() && !dir.join("s-inc").exists(), "its transcript and what the CLI kept beside it");
    assert!(dir.join("s-shared.jsonl").exists(), "a session another chat uses stays");
    assert!(dir.join("s-normal.jsonl").exists());
    assert!(!chat_cli.join(format!("system-{inc}.txt")).exists(), "its prompt file");
    assert!(!data.join("outputs").join(&inc).exists(), "the files it made");
    assert_eq!(clauderust_lib::incognito::sweep(&db, &data), 0, "nothing left");
    drop(db);
    let _ = std::fs::remove_dir_all(root);
}
