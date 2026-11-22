//! Chat memory end to end, with no model run: against a local Claude API
//! (one bounded request from the chats it may read, the answer stored,
//! nothing past the day's cap) and a stand-in for the account's CLI.

use clauderust_lib::db::{Db, Message};
use clauderust_lib::memory_synth::{self, DAILY_RUNS, GLOBAL, SYNTH_PROMPT};
use clauderust_lib::state::AppState;
use clauderust_lib::util::new_id;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Seen = Arc<Mutex<Vec<Value>>>;

/// A Messages API answering every request with `reply`'s (text, stop reason).
async fn api(reply: Arc<Mutex<(String, String)>>) -> (String, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let (log, reply) = (log.clone(), reply.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let body_at = loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..body_at]).to_ascii_lowercase();
                let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                while buf.len() < body_at + len {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                log.lock().unwrap().push(serde_json::from_slice(&buf[body_at..body_at + len]).unwrap_or(Value::Null));
                let (text, stop) = reply.lock().unwrap().clone();
                let out = json!({"id": "m", "type": "message", "role": "assistant", "model": "x", "content": [{"type": "text", "text": text}], "stop_reason": stop, "usage": {"input_tokens": 1, "output_tokens": 1}}).to_string();
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}", out.len());
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    (url, seen)
}

fn temp_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("memory-{}", new_id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn state(dir: &std::path::Path) -> Arc<AppState> {
    static ENV: Once = Once::new();
    ENV.call_once(|| {
        std::env::set_var("NO_PROXY", "127.0.0.1");
        // a fake key: the app's keyring is never read
        std::env::set_var("CLAUDERUST_API_KEY", "test-key");
    });
    let st = Arc::new(AppState::new(Db::open_in_memory().unwrap(), dir.to_path_buf(), dir.join("mcp.json")));
    st.settings.write().memory_enabled = true;
    st
}

/// A chat whose user said `text`, updated now.
fn chat(db: &Db, project: Option<&str>, options: Value, text: &str) -> String {
    let c = db.conv_create("chat", "claude-opus-5-5", project, None, &options).unwrap();
    let u = Message { id: new_id(), conversation_id: c.id.clone(), parent_id: None, role: "user".into(), content: json!([{"type": "text", "text": text}]), meta: json!({}), created_at: 1 };
    let a = Message { id: new_id(), parent_id: Some(u.id.clone()), role: "assistant".into(), content: json!([{"type": "text", "text": "Noted."}]), meta: json!({"stop_reason": "end_turn"}), ..u.clone() };
    db.msg_insert(&u).unwrap();
    db.msg_insert(&a).unwrap();
    db.conv_set_leaf(&c.id, Some(&a.id)).unwrap();
    c.id
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_writes_the_memory_from_the_chats_it_may_read() {
    let reply = Arc::new(Mutex::new(("- Works as a marine biologist".to_string(), "end_turn".to_string())));
    let (url, seen) = api(reply.clone()).await;
    let dir = temp_dir();
    let st = state(&dir);
    {
        let mut s = st.settings.write();
        s.base_url = url;
        s.chat_engine = "api".into();
    }
    let excluded = st.db.project_create("Hidden", "").unwrap();
    let other = st.db.project_create("Own memory", "").unwrap();
    st.settings.write().memory_excluded_projects = vec![excluded.id.clone()];
    let a = chat(&st.db, None, json!({}), "I am a marine biologist");
    let b = chat(&st.db, None, json!({}), "Please answer briefly");
    chat(&st.db, None, json!({"incognito": true}), "INCOGNITO-SECRET");
    chat(&st.db, Some(&excluded.id), json!({}), "EXCLUDED-PROJECT");
    chat(&st.db, Some(&other.id), json!({}), "OTHER-PROJECT");

    let m = memory_synth::synthesize(&st, GLOBAL).await.unwrap().unwrap();
    assert_eq!(m.text, "- Works as a marine biologist");
    assert!(m.sources.contains(&a) && m.sources.contains(&b) && m.sources.len() == 2);
    let bodies = seen.lock().unwrap().clone();
    assert_eq!(bodies.len(), 1);
    let body = &bodies[0];
    assert_eq!(body["model"], st.settings.read().title_model.as_str());
    assert_eq!(body["system"], SYNTH_PROMPT);
    assert_eq!(body["max_tokens"], 2048);
    let input = body["messages"][0]["content"].as_str().unwrap();
    assert!(input.contains("I am a marine biologist") && input.contains("Please answer briefly"));
    for never in ["INCOGNITO-SECRET", "EXCLUDED-PROJECT", "OTHER-PROJECT"] {
        assert!(!input.contains(never), "{never} was sent");
    }
    // a new chat is given it, as data
    std::thread::sleep(std::time::Duration::from_millis(5));
    let c = st.db.conv_get(&chat(&st.db, None, json!({}), "hi")).unwrap();
    let block = memory_synth::memory_block(&st.db, &st.settings.read(), &c).unwrap();
    assert!(block.contains("<memory_data>\n- Works as a marine biologist\n</memory_data>"));
    // which is new for the next update; after that, no chat is
    memory_synth::synthesize(&st, GLOBAL).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 2);
    std::thread::sleep(std::time::Duration::from_millis(5));
    memory_synth::synthesize(&st, GLOBAL).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 2, "no chat since: no call");

    // a reply cut off at its token limit changes nothing
    *reply.lock().unwrap() = ("- half a".into(), "max_tokens".into());
    chat(&st.db, None, json!({}), "more");
    assert!(memory_synth::synthesize(&st, GLOBAL).await.unwrap_err().to_string().contains("cut off"));
    assert_eq!(st.db.memory_get(GLOBAL).unwrap().unwrap().text, "- Works as a marine biologist");

    // the day's cap: the failed call counted too
    *reply.lock().unwrap() = ("- ok".into(), "end_turn".into());
    while seen.lock().unwrap().len() < DAILY_RUNS as usize {
        std::thread::sleep(std::time::Duration::from_millis(3));
        chat(&st.db, None, json!({}), "again");
        memory_synth::synthesize(&st, GLOBAL).await.unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(3));
    chat(&st.db, None, json!({}), "one too many");
    assert!(memory_synth::synthesize(&st, GLOBAL).await.unwrap_err().to_string().contains("tomorrow"));
    assert_eq!(seen.lock().unwrap().len(), DAILY_RUNS as usize, "no call past the cap");

    // memory off: no call at all
    st.settings.write().memory_enabled = false;
    assert!(memory_synth::synthesize(&st, GLOBAL).await.is_err());
    let _ = std::fs::remove_dir_all(dir);
}

/// A stand-in for the `claude` CLI: keeps what it read on stdin, answers as `claude -p --output-format json`.
fn fake_cli(dir: &std::path::Path) -> PathBuf {
    let answer = r#"{"type":"result","is_error":false,"result":"- Prefers short answers"}"#;
    #[cfg(windows)]
    let (path, script) = (dir.join("claude.cmd"), format!("@echo off\r\nfindstr \"^\" > \"%~dp0stdin.txt\"\r\necho {answer}\r\n"));
    #[cfg(not(windows))]
    let (path, script) = (dir.join("claude"), format!("#!/bin/sh\ncat > \"$(dirname \"$0\")/stdin.txt\"\necho '{answer}'\n"));
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn chats_on_the_account_write_it_through_the_cli() {
    let dir = temp_dir();
    let st = state(&dir);
    let cli = fake_cli(&dir);
    {
        let mut s = st.settings.write();
        s.chat_engine = "claude-code".into();
        s.claude_code_path = cli.display().to_string();
    }
    // a project's own memory (the other test writes the global one)
    let p = st.db.project_create("P", "").unwrap();
    chat(&st.db, Some(&p.id), json!({}), "Keep it short, please");
    let m = memory_synth::synthesize(&st, &p.id).await.unwrap().unwrap();
    assert_eq!(m.text, "- Prefers short answers");
    let stdin = std::fs::read_to_string(dir.join("stdin.txt")).unwrap();
    assert!(stdin.contains("<current_memory>") && stdin.contains("Keep it short, please"), "{stdin}");
    let _ = std::fs::remove_dir_all(dir);
}
