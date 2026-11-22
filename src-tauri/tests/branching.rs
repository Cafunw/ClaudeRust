//! Versions of a conversation: an edited message and a regenerated reply are
//! siblings in the message tree, and the `switch_branch` command moves the
//! conversation between them. Turns run through `engine::start` on Tauri's
//! mock runtime with a real database, against a local Claude API that
//! answers "Answer <n> to <the last user text>".

use clauderust_lib::commands;
use clauderust_lib::db::{Db, Message};
use clauderust_lib::engine;
use clauderust_lib::state::AppState;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};
use tauri::test::MockRuntime;
use tauri::{Listener, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ------------------------------------------------------------- mock API

/// The bodies of the streaming Messages requests, in order.
type Seen = Arc<Mutex<Vec<Value>>>;

async fn api() -> (String, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let _ = serve(s, &log).await;
            });
        }
    });
    (url, seen)
}

async fn serve(mut s: tokio::net::TcpStream, seen: &Seen) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
    let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
    while buf.len() < head_end + len {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body: Value = serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or(Value::Null);
    let (kind, out) = if body["stream"] != true {
        // a title
        let t = json!({"id": "msg_t", "type": "message", "role": "assistant", "model": body["model"],
            "content": [{"type": "text", "text": "Mock title"}], "stop_reason": "end_turn", "usage": {"input_tokens": 5, "output_tokens": 2}});
        ("application/json", t.to_string())
    } else {
        let n = {
            let mut seen = seen.lock().unwrap();
            seen.push(body.clone());
            seen.len()
        };
        let asked = body["messages"].as_array().and_then(|m| m.last()).and_then(|m| m["content"][0]["text"].as_str()).unwrap_or("").to_string();
        let events = [
            json!({"type": "message_start", "message": {"id": format!("msg_{n}"), "type": "message", "role": "assistant", "model": body["model"],
                "content": [], "usage": {"input_tokens": 10, "output_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": format!("Answer {n} to {asked}")}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}),
            json!({"type": "message_stop"}),
        ];
        let sse: String = events.iter().map(|ev| format!("event: {}\ndata: {ev}\n\n", ev["type"].as_str().unwrap_or(""))).collect();
        ("text/event-stream", sse)
    };
    let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}", out.len());
    s.write_all(resp.as_bytes()).await
}

// ------------------------------------------------------------- the app

/// A folder removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Chat {
    app: tauri::App<MockRuntime>,
    st: Arc<AppState>,
    conv: String,
    seen: Seen,
    /// turns of this conversation that have ended
    ended: Arc<AtomicUsize>,
    _dir: TempDir,
}

impl Chat {
    /// A titled chat on a fresh database, talking to a fresh mock API.
    async fn new() -> Chat {
        static ENV: Once = Once::new();
        ENV.call_once(|| {
            std::env::set_var("NO_PROXY", "127.0.0.1");
            // a fake key: the app's keyring is never read
            std::env::set_var("CLAUDERUST_API_KEY", "test-key");
        });
        let (url, seen) = api().await;
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("branching-{}", clauderust_lib::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = Arc::new(AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
        {
            let mut s = st.settings.write();
            s.base_url = url;
            s.chat_engine = "api".into();
            // the notification plugin isn't on the mock runtime
            s.notifications = false;
        }
        let app = tauri::test::mock_app();
        // what commands get as `State`
        app.manage(st.clone());
        let conv = st.db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap();
        st.db.conv_set_title(&conv.id, "Test").unwrap();
        let ended: Arc<AtomicUsize> = Arc::default();
        let (count, id) = (ended.clone(), conv.id.clone());
        app.listen_any("generation-state", move |e| {
            let p: Value = serde_json::from_str(e.payload()).unwrap_or(Value::Null);
            if p["conversation_id"] == id.as_str() && p["running"] == false {
                count.fetch_add(1, Ordering::SeqCst);
            }
        });
        Chat { app, st, conv: conv.id, seen, ended, _dir: TempDir(dir) }
    }

    /// Store a user message under `parent` and run its turn to the end, as
    /// `send_message` does (an edit when `parent` already has a reply).
    /// Returns the message's id.
    async fn send(&self, parent: Option<&str>, text: &str) -> String {
        let id = clauderust_lib::util::new_id();
        let m = Message {
            id: id.clone(),
            conversation_id: self.conv.clone(),
            parent_id: parent.map(Into::into),
            role: "user".into(),
            content: json!([{"type": "text", "text": text}]),
            meta: json!({}),
            created_at: clauderust_lib::util::now_ms(),
        };
        self.st.db.msg_insert(&m).unwrap();
        self.st.db.conv_set_leaf(&self.conv, Some(&id)).unwrap();
        self.run(&id).await;
        id
    }

    /// Answer `user_message` (again), as `regenerate` does, and wait for it.
    async fn run(&self, user_message: &str) {
        let before = self.ended.load(Ordering::SeqCst);
        self.st.db.conv_set_leaf(&self.conv, Some(user_message)).unwrap();
        engine::start(self.app.handle(), &self.st, &self.conv, user_message).unwrap();
        until("the turn to end", || (self.ended.load(Ordering::SeqCst) > before && !self.st.runs.lock().contains_key(&self.conv)).then_some(())).await;
    }

    fn leaf(&self) -> String {
        self.st.db.conv_get(&self.conv).unwrap().current_leaf.unwrap()
    }

    /// The texts of the current branch, root first.
    fn branch(&self) -> Vec<String> {
        self.st.db.path_to(&self.leaf()).unwrap().iter().map(|m| m.content[0]["text"].as_str().unwrap_or("").to_string()).collect()
    }

    /// `switch_branch`, then what `get_conversation` shows.
    async fn switch(&self, to: &str) -> Result<Value, String> {
        commands::switch_branch(self.app.state(), self.conv.clone(), to.to_string()).await.map_err(|e| e.to_string())?;
        Ok(self.view().await)
    }

    /// The whole conversation as `get_conversation` shows it.
    async fn view(&self) -> Value {
        commands::get_conversation(self.app.state(), self.conv.clone(), None, None, None).await.unwrap()
    }

    /// The messages the last API request sent.
    fn sent(&self) -> Value {
        self.seen.lock().unwrap().last().unwrap()["messages"].clone()
    }
}

/// The (siblings index, count) of each turn of a view.
fn versions(view: &Value) -> Vec<(u64, u64)> {
    view["turns"].as_array().unwrap().iter().map(|t| (t["siblings"]["index"].as_u64().unwrap(), t["siblings"]["count"].as_u64().unwrap())).collect()
}

async fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let t0 = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Run `f` on Tauri's async runtime (the one the turn runs on).
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tauri::async_runtime::block_on(f)
}

fn user(text: &str) -> Value {
    json!({"role": "user", "content": [{"type": "text", "text": text}]})
}

// ------------------------------------------------------------- tests

#[test]
fn switching_versions_of_an_edited_message_shows_each_branch_at_its_newest_reply() {
    block_on(async {
        let c = Chat::new().await;
        let first = c.send(None, "first").await;
        let first_reply = c.leaf();
        assert_eq!(c.branch(), ["first", "Answer 1 to first"]);
        // the edit: a sibling of the first message, and only its branch goes to the API
        let edited = c.send(None, "edited").await;
        assert_eq!(c.sent(), json!([user("edited")]));
        let reply = c.leaf();
        c.send(Some(&reply), "more").await;
        assert_eq!(c.branch(), ["edited", "Answer 2 to edited", "more", "Answer 3 to more"]);
        let newest = c.leaf();

        let v = c.switch(&first).await.unwrap();
        assert_eq!(c.leaf(), first_reply);
        assert_eq!(c.branch(), ["first", "Answer 1 to first"]);
        assert_eq!(versions(&v), [(0, 2), (0, 1)], "{v}");
        assert_eq!(v["turns"][0]["id"], first.as_str());
        assert_eq!(v["running"], false);

        // back to the edit: its branch is shown down to its newest message
        let v = c.switch(&edited).await.unwrap();
        assert_eq!(c.leaf(), newest);
        assert_eq!(versions(&v), [(1, 2), (0, 1), (0, 1), (0, 1)], "{v}");

        // a message in the middle of a branch: down to the newest message under it
        c.switch(&first).await.unwrap();
        c.switch(&reply).await.unwrap();
        assert_eq!(c.leaf(), newest);
    });
}

#[test]
fn switching_between_regenerated_replies() {
    block_on(async {
        let c = Chat::new().await;
        let q = c.send(None, "question").await;
        let a1 = c.leaf();
        // the regenerated reply is sent the same history again
        c.run(&q).await;
        assert_eq!(c.sent(), json!([user("question")]));
        let a2 = c.leaf();
        assert_ne!(a1, a2);
        assert_eq!(c.branch(), ["question", "Answer 2 to question"]);
        // a follow-up to the second reply
        c.send(Some(&a2), "thanks").await;
        let newest = c.leaf();

        let v = c.switch(&a1).await.unwrap();
        assert_eq!(c.leaf(), a1);
        assert_eq!(c.branch(), ["question", "Answer 1 to question"]);
        assert_eq!(versions(&v), [(0, 1), (0, 2)], "{v}");

        let v = c.switch(&a2).await.unwrap();
        assert_eq!(c.leaf(), newest);
        assert_eq!(versions(&v)[1], (1, 2), "{v}");
        assert_eq!(c.branch(), ["question", "Answer 2 to question", "thanks", "Answer 3 to thanks"]);
    });
}

#[test]
fn switch_branch_refuses_other_conversations_messages_and_a_running_turn() {
    block_on(async {
        let c = Chat::new().await;
        let q = c.send(None, "question").await;
        let leaf = c.leaf();
        let other = c.st.db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap();
        let stranger = Message {
            id: clauderust_lib::util::new_id(),
            conversation_id: other.id.clone(),
            parent_id: None,
            role: "user".into(),
            content: json!([{"type": "text", "text": "elsewhere"}]),
            meta: json!({}),
            created_at: 1,
        };
        c.st.db.msg_insert(&stranger).unwrap();
        assert_eq!(c.switch(&stranger.id).await.unwrap_err(), "Message not found in this conversation.");
        assert_eq!(c.switch("no-such-message").await.unwrap_err(), "Message not found in this conversation.");
        assert_eq!(c.leaf(), leaf);

        // while a turn runs (or is being prepared), the branch stays
        let slot = engine::RunSlot::reserve(&c.st, &c.conv, "busy").unwrap();
        assert_eq!(c.switch(&q).await.unwrap_err(), "Wait for the response to finish before switching versions.");
        assert_eq!(c.leaf(), leaf);
        drop(slot);
        // a switch holds the conversation only while it moves
        c.switch(&q).await.unwrap();
        assert!(!c.st.runs.lock().contains_key(&c.conv));
    });
}

// ------------------------------------------------------------- the commands

impl Chat {
    /// `send_message` with `text`; an edit with `edit_parent` (the edited
    /// message's parent, "" for the first message). Waits for the turn.
    async fn say(&self, text: &str, edit_parent: Option<&str>) -> Result<String, String> {
        self.say_with(text, edit_parent, false).await
    }

    /// `say`, with the request's research switch.
    async fn say_with(&self, text: &str, edit_parent: Option<&str>, research: bool) -> Result<String, String> {
        let before = self.ended.load(Ordering::SeqCst);
        let req = commands::SendReq {
            conversation_id: Some(self.conv.clone()),
            new_conversation: None,
            text: text.into(),
            attachment_ids: vec![],
            parent_id: edit_parent.map(Into::into),
            edit: edit_parent.is_some(),
            research,
        };
        let r = commands::send_message_inner(self.app.handle(), self.st.clone(), req).await.map_err(|e| e.to_string())?;
        until("the turn to end", || (self.ended.load(Ordering::SeqCst) > before && !self.st.runs.lock().contains_key(&self.conv)).then_some(())).await;
        Ok(r["message_id"].as_str().unwrap().to_string())
    }

    /// The `regenerate` command; waits for the turn.
    async fn regenerate(&self, user_message: &str, model: Option<&str>) -> Result<(), String> {
        let before = self.ended.load(Ordering::SeqCst);
        commands::regenerate(self.app.handle().clone(), self.app.state(), self.conv.clone(), user_message.into(), model.map(Into::into)).await.map_err(|e| e.to_string())?;
        until("the turn to end", || (self.ended.load(Ordering::SeqCst) > before && !self.st.runs.lock().contains_key(&self.conv)).then_some(())).await;
        Ok(())
    }

    fn parent(&self, id: &str) -> Option<String> {
        self.st.db.msg_get(id).unwrap().parent_id
    }
}

#[test]
fn a_message_sent_for_research_is_marked_so() {
    block_on(async {
        let c = Chat::new().await;
        let plain = c.say("hello", None).await.unwrap();
        assert!(c.st.db.msg_get(&plain).unwrap().meta.get("research").is_none());
        let asked = c.say_with("look it up", None, true).await.unwrap();
        // what research.rs starts from (it runs on the API engine this harness uses)
        assert_eq!(c.st.db.msg_get(&asked).unwrap().meta["research"], true);
        let reply = c.st.db.msg_get(&c.leaf()).unwrap();
        assert!(reply.meta["research"].is_object(), "{}", reply.meta);
    });
}

#[test]
fn an_edit_sent_with_send_message_is_a_new_version_of_the_message() {
    block_on(async {
        let c = Chat::new().await;
        let first = c.say("first", None).await.unwrap();
        let reply = c.leaf();
        let second = c.say("second", None).await.unwrap();
        assert_eq!(c.parent(&second).as_deref(), Some(reply.as_str()), "a plain message follows the branch");

        // the first message edited: "" is the root
        let edited = c.say("edited", Some("")).await.unwrap();
        assert_eq!(c.parent(&edited), None);
        assert_eq!(c.branch(), ["edited", "Answer 3 to edited"]);
        assert_eq!(c.sent(), json!([user("edited")]), "only the new branch goes to the API");
        let v = c.view().await;
        assert_eq!(versions(&v), [(1, 2), (0, 1)], "{v}");

        // a later message edited: a sibling of it, after the same reply
        c.switch(&first).await.unwrap();
        let edited2 = c.say("second, edited", Some(&reply)).await.unwrap();
        assert_eq!(c.parent(&edited2).as_deref(), Some(reply.as_str()));
        assert_eq!(c.branch(), ["first", "Answer 1 to first", "second, edited", "Answer 4 to second, edited"]);
        let sent = c.sent();
        assert_eq!(sent.as_array().unwrap().len(), 3, "{sent}");
        assert_eq!((&sent[0], &sent[2]), (&user("first"), &user("second, edited")));
        let v = c.view().await;
        assert_eq!(versions(&v), [(0, 2), (0, 1), (1, 2), (0, 1)], "{v}");

        // an empty edit is refused and stores nothing
        let n = c.st.db.tree_nodes(&c.conv).unwrap().len();
        assert_eq!(c.say("  ", Some(&reply)).await.unwrap_err(), "Message is empty.");
        assert_eq!(c.st.db.tree_nodes(&c.conv).unwrap().len(), n);
        assert!(!c.st.runs.lock().contains_key(&c.conv), "the conversation isn't left reserved");
    });
}

#[test]
fn regenerate_answers_a_message_again_as_another_version_of_the_reply() {
    block_on(async {
        let c = Chat::new().await;
        let q = c.say("question", None).await.unwrap();
        let a1 = c.leaf();
        c.say("more", None).await.unwrap();

        // from the first message, with another model: the rest of the branch stays on the first reply's side
        c.regenerate(&q, Some("claude-sonnet-5-5")).await.unwrap();
        let a2 = c.leaf();
        assert_ne!(a1, a2);
        assert_eq!(c.parent(&a2).as_deref(), Some(q.as_str()));
        assert_eq!(c.branch(), ["question", "Answer 3 to question"]);
        assert_eq!(c.sent(), json!([user("question")]));
        assert_eq!(c.seen.lock().unwrap().last().unwrap()["model"], "claude-sonnet-5-5");
        assert_eq!(c.st.db.conv_get(&c.conv).unwrap().model, "claude-sonnet-5-5");
        let v = c.view().await;
        assert_eq!(versions(&v), [(0, 1), (1, 2)], "{v}");

        // only from a user message of this conversation, and while nothing runs
        let other = c.st.db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap();
        let stranger = Message {
            id: clauderust_lib::util::new_id(),
            conversation_id: other.id.clone(),
            parent_id: None,
            role: "user".into(),
            content: json!([{"type": "text", "text": "elsewhere"}]),
            meta: json!({}),
            created_at: 1,
        };
        c.st.db.msg_insert(&stranger).unwrap();
        assert_eq!(c.regenerate(&a1, Some("claude-haiku-5-5")).await.unwrap_err(), "Can only regenerate from a user message.");
        assert_eq!(c.st.db.conv_get(&c.conv).unwrap().model, "claude-sonnet-5-5", "a refused regenerate keeps the model");
        assert_eq!(c.regenerate(&stranger.id, None).await.unwrap_err(), "Message not found in this conversation.");
        let slot = engine::RunSlot::reserve(&c.st, &c.conv, "busy").unwrap();
        assert_eq!(c.regenerate(&q, None).await.unwrap_err(), "Claude is still responding. Stop the response or wait for it to finish.");
        drop(slot);
        assert_eq!(c.leaf(), a2, "nothing moved");
        assert!(!c.st.runs.lock().contains_key(&c.conv), "the conversation isn't left reserved");
        assert_eq!(c.seen.lock().unwrap().len(), 3, "no request for a refused regenerate");
    });
}

#[test]
fn get_conversation_sends_the_last_turns_and_older_ones_on_request() {
    block_on(async {
        let c = Chat::new().await;
        let mut parent: Option<String> = None;
        for i in 0..3 {
            c.send(parent.as_deref(), &format!("q{i}")).await;
            parent = Some(c.leaf());
        }
        let page = |limit: Option<usize>, since: Option<&str>, before: Option<&str>| {
            commands::get_conversation(c.app.state(), c.conv.clone(), limit, since.map(Into::into), before.map(Into::into))
        };
        let all = c.view().await;
        let ids: Vec<&str> = all["turns"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert_eq!((ids.len(), &all["more"]), (6, &json!(false)));
        let last = page(Some(2), None, None).await.unwrap();
        assert_eq!((&last["turns"], &last["more"]), (&json!(all["turns"].as_array().unwrap()[4..]), &json!(true)));
        let older = page(Some(2), None, Some(ids[4])).await.unwrap();
        assert_eq!(older["turns"], json!(all["turns"].as_array().unwrap()[2..4]));
        let since = page(Some(2), Some(ids[2]), None).await.unwrap();
        assert_eq!(since["turns"], json!(all["turns"].as_array().unwrap()[2..]));
    });
}
