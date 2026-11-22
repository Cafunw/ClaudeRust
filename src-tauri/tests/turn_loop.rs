//! The turn loop end to end: `engine::start` on Tauri's mock runtime with a
//! real database, against a local Claude API (and MCP server) that answers
//! what each test scripts. The assertions are on what the database holds
//! afterwards and on the events the UI was sent.

use clauderust_lib::db::{Db, Message};
use clauderust_lib::engine;
use clauderust_lib::state::{AppState, PermissionReply};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};
use tauri::test::MockRuntime;
use tauri::Listener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ------------------------------------------------------------- mock API

/// What the mock API answers to one streaming Messages request.
enum Reply {
    /// these events, then the end of the stream
    Stream(Vec<Value>),
    /// these events, then the connection drops
    Drop(Vec<Value>),
    /// these events, then nothing (the connection stays open)
    Stall(Vec<Value>),
    /// a plain answer: status, extra headers, body
    Plain(u16, Vec<(&'static str, &'static str)>, String),
}

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

struct Api {
    url: String,
    /// every request: (path, JSON body)
    seen: Seen,
}

impl Api {
    /// The bodies of the streaming Messages requests, in order.
    fn streams(&self) -> Vec<Value> {
        let seen = self.seen.lock().unwrap();
        seen.iter().filter(|(p, b)| p == "/v1/messages" && b["stream"] == true).map(|(_, b)| b.clone()).collect()
    }

    fn mcp_calls(&self) -> Vec<Value> {
        let seen = self.seen.lock().unwrap();
        seen.iter().filter(|(p, b)| p == "/mcp" && b["method"] == "tools/call").map(|(_, b)| b.clone()).collect()
    }
}

/// Start the mock API: streaming Messages requests get `script`'s answer,
/// other Messages requests (titles) a title, `/mcp` an MCP server with one
/// `echo` tool.
async fn api(script: impl Fn(&Value) -> Reply + Send + Sync + 'static) -> Api {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let (script, log) = (Arc::new(script), seen.clone());
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let (script, log) = (script.clone(), log.clone());
            tokio::spawn(async move {
                let _ = serve(s, &*script, &log).await;
            });
        }
    });
    Api { url, seen }
}

async fn serve(mut s: tokio::net::TcpStream, script: &(dyn Fn(&Value) -> Reply + Send + Sync), seen: &Seen) -> std::io::Result<()> {
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
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut first = head.split_whitespace();
    let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let len: usize = head
        .lines()
        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body: Value = serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or(Value::Null);
    seen.lock().unwrap().push((path.clone(), body.clone()));
    let reply = if method != "POST" {
        Reply::Plain(405, vec![], String::new())
    } else if path == "/mcp" {
        mcp(&body)
    } else if body["stream"] != true {
        let title = json!({"id": "msg_t", "type": "message", "role": "assistant", "model": body["model"],
            "content": [{"type": "text", "text": "Mock title"}], "stop_reason": "end_turn", "usage": {"input_tokens": 5, "output_tokens": 2}});
        Reply::Plain(200, vec![], title.to_string())
    } else {
        script(&body)
    };
    let events = match reply {
        Reply::Plain(status, headers, text) => {
            let mut resp = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n", text.len());
            for (k, v) in headers {
                resp.push_str(&format!("{k}: {v}\r\n"));
            }
            resp.push_str("\r\n");
            resp.push_str(&text);
            return s.write_all(resp.as_bytes()).await;
        }
        Reply::Stream(ref e) | Reply::Drop(ref e) | Reply::Stall(ref e) => e.clone(),
    };
    s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n").await?;
    for ev in &events {
        let data = format!("event: {}\ndata: {ev}\n\n", ev["type"].as_str().unwrap_or(""));
        s.write_all(format!("{:x}\r\n{data}\r\n", data.len()).as_bytes()).await?;
        s.flush().await?;
    }
    match reply {
        Reply::Stream(_) => s.write_all(b"0\r\n\r\n").await,
        // closed without the last chunk: the stream breaks off
        Reply::Drop(_) => Ok(()),
        _ => {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        }
    }
}

/// A JSON-RPC answer of the MCP server.
fn mcp(body: &Value) -> Reply {
    if body["id"].is_null() {
        return Reply::Plain(202, vec![], String::new());
    }
    let result = match body["method"].as_str().unwrap_or("") {
        "initialize" => json!({"protocolVersion": body["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "web"}}),
        "tools/list" => json!({"tools": [{"name": "echo", "description": "Echo the text back", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]}),
        "tools/call" => json!({"content": [{"type": "text", "text": body["params"]["arguments"]["text"]}]}),
        _ => json!({}),
    };
    Reply::Plain(200, vec![], json!({"jsonrpc": "2.0", "id": body["id"], "result": result}).to_string())
}

// ------------------------------------------------------------- SSE events

fn message_start(body: &Value) -> Value {
    json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": body["model"],
        "content": [], "usage": {"input_tokens": 10, "output_tokens": 1}}})
}

fn text_block(index: usize, pieces: &[&str]) -> Vec<Value> {
    let mut v = vec![json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}})];
    for p in pieces {
        v.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": p}}));
    }
    v.push(json!({"type": "content_block_stop", "index": index}));
    v
}

fn tool_block(index: usize, id: &str, name: &str, input: &Value) -> Vec<Value> {
    vec![
        json!({"type": "content_block_start", "index": index, "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}}),
        json!({"type": "content_block_delta", "index": index, "delta": {"type": "input_json_delta", "partial_json": input.to_string()}}),
        json!({"type": "content_block_stop", "index": index}),
    ]
}

/// A whole reply: its blocks, then the stop reason and the end.
fn reply(body: &Value, blocks: Vec<Vec<Value>>, stop: &str) -> Vec<Value> {
    let mut v = vec![message_start(body)];
    v.extend(blocks.into_iter().flatten());
    v.push(json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 5}}));
    v.push(json!({"type": "message_stop"}));
    v
}

/// A reply that streams one text block, starts a second one and goes quiet.
fn stalled(body: &Value) -> Reply {
    let mut v = vec![message_start(body)];
    v.extend(text_block(0, &["Partial ", "answer"]));
    v.push(json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}));
    Reply::Stall(v)
}

// ------------------------------------------------------------- the app

type Events = Arc<Mutex<Vec<(String, Value)>>>;

const EVENTS: [&str; 5] = ["generation-state", "turn", "permission-request", "permission-resolved", "conversation-updated"];

struct Harness {
    app: tauri::App<MockRuntime>,
    st: Arc<AppState>,
    events: Events,
    conv: String,
    // last: removed once the database in `st` is closed
    dir: TempDir,
}

/// A folder removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("turn-{}", clauderust_lib::util::new_id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

impl Harness {
    /// The app state on `db`, talking to `api`, with a titled chat.
    fn new(api: &Api, db: Db, dir: PathBuf) -> Harness {
        static ENV: Once = Once::new();
        ENV.call_once(|| {
            std::env::set_var("NO_PROXY", "127.0.0.1");
            // a fake key: the app's keyring is never read
            std::env::set_var("CLAUDERUST_API_KEY", "test-key");
        });
        let st = Arc::new(AppState::new(db, dir.clone(), dir.join("mcp.json")));
        {
            let mut s = st.settings.write();
            s.base_url = api.url.clone();
            s.chat_engine = "api".into();
            // the notification plugin isn't on the mock runtime
            s.notifications = false;
        }
        let app = tauri::test::mock_app();
        let events: Events = Arc::default();
        for name in EVENTS {
            let log = events.clone();
            app.listen_any(name, move |e| {
                log.lock().unwrap().push((name.to_string(), serde_json::from_str(e.payload()).unwrap_or(Value::Null)));
            });
        }
        let conv = st.db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap();
        st.db.conv_set_title(&conv.id, "Test").unwrap();
        Harness { app, st, events, conv: conv.id, dir: TempDir(dir) }
    }

    /// Send `text` as the first message and start the turn.
    fn send(&self, text: &str) -> String {
        let id = clauderust_lib::util::new_id();
        let m = Message {
            id: id.clone(),
            conversation_id: self.conv.clone(),
            parent_id: None,
            role: "user".into(),
            content: json!([{"type": "text", "text": text}]),
            meta: json!({}),
            created_at: 1,
        };
        self.st.db.msg_insert(&m).unwrap();
        self.st.db.conv_set_leaf(&self.conv, Some(&id)).unwrap();
        engine::start(self.app.handle(), &self.st, &self.conv, &id).unwrap();
        id
    }

    /// The payloads of this conversation's `name` events.
    fn events(&self, name: &str) -> Vec<Value> {
        let ev = self.events.lock().unwrap();
        ev.iter().filter(|(n, p)| n == name && p["conversation_id"] == self.conv.as_str()).map(|(_, p)| p.clone()).collect()
    }

    /// Wait for the turn to end; its last `generation-state` event.
    async fn ended(&self) -> Value {
        until("the turn to end", || self.events("generation-state").into_iter().find(|e| e["running"] == false)).await
    }

    /// The conversation's current branch, root first.
    fn path(&self) -> Vec<Message> {
        let leaf = self.st.db.conv_get(&self.conv).unwrap().current_leaf.unwrap();
        self.st.db.path_to(&leaf).unwrap()
    }

    /// Nothing of the turn is left in the app state.
    fn assert_reset(&self) {
        let c = self.conv.as_str();
        assert!(!self.st.runs.lock().contains_key(c), "still marked running");
        assert!(!self.st.live.lock().contains_key(c), "live state left");
        assert!(self.st.pending_perms.lock().values().all(|p| p["conversation_id"] != c), "a permission prompt left");
        let states: Vec<bool> = self.events("generation-state").iter().map(|e| e["running"].as_bool().unwrap()).collect();
        assert_eq!(states, vec![true, false]);
        assert!(!self.events("conversation-updated").is_empty());
    }
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

fn chat(api: &Api) -> Harness {
    Harness::new(api, Db::open_in_memory().unwrap(), temp_dir())
}

// ------------------------------------------------------------- tests

#[test]
fn a_streamed_reply_is_stored_under_the_message() {
    block_on(async {
        let api = api(|b| {
            let thinking = vec![
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Let me think"}}),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
                json!({"type": "content_block_stop", "index": 0}),
            ];
            Reply::Stream(reply(b, vec![thinking, text_block(1, &["Hello", " world"])], "end_turn"))
        })
        .await;
        let h = chat(&api);
        h.st.db.conv_set_title(&h.conv, "").unwrap();
        let user = h.send("Hi");
        assert_eq!(h.ended().await["error"], Value::Null);
        h.assert_reset();

        let path = h.path();
        assert_eq!(path.len(), 2);
        let a = &path[1];
        assert_eq!((a.role.as_str(), a.parent_id.as_deref()), ("assistant", Some(user.as_str())));
        assert_eq!(a.content, json!([{"type": "thinking", "thinking": "Let me think", "signature": "sig"}, {"type": "text", "text": "Hello world"}]));
        assert_eq!(a.meta["stop_reason"], "end_turn");
        assert_eq!(a.meta["usage"], json!({"input_tokens": 10, "output_tokens": 5}));
        assert_eq!(h.st.db.search("world", 5).unwrap().len(), 1, "indexed for search");

        let sent = api.streams();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["model"], "claude-opus-5-5");
        assert_eq!(sent[0]["messages"], json!([{"role": "user", "content": [{"type": "text", "text": "Hi"}]}]));

        let turns = h.events("turn");
        assert!(turns.len() >= 2, "{} turn events", turns.len());
        assert!(turns[0]["base"].is_null(), "a new turn starts with a keyframe");
        // then the title, from the same API
        until("the title", || (h.st.db.conv_get(&h.conv).unwrap().title == "Mock title").then_some(())).await;
    });
}

/// A chat whose reply calls the MCP server's `echo` tool, answered with
/// `decision`: the calls the MCP server got and the tool result stored.
fn tool_round_trip(decision: &str, message: Option<&str>) -> (Vec<Value>, Value) {
    block_on(async {
        let api = api(|b| {
            let msgs = b["messages"].as_array().unwrap();
            let answered = msgs.last().unwrap()["content"].as_array().unwrap().iter().any(|c| c["type"] == "tool_result");
            if answered {
                return Reply::Stream(reply(b, vec![text_block(0, &["Done."])], "end_turn"));
            }
            let tool = b["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).find(|n| n.ends_with("echo")).unwrap().to_string();
            Reply::Stream(reply(b, vec![text_block(0, &["Let me check."]), tool_block(1, "toolu_1", &tool, &json!({"text": "hi"}))], "tool_use"))
        })
        .await;
        let h = chat(&api);
        std::fs::write(h.dir.0.join("mcp.json"), json!({"mcpServers": {"web": {"url": format!("{}/mcp", api.url)}}}).to_string()).unwrap();
        h.st.mcp.start_all(&[]).await;
        let tools = h.st.mcp.tools().await;
        assert_eq!(tools.len(), 1, "the MCP server connected");
        h.send("Echo hi");

        let ask = until("the permission prompt", || h.events("permission-request").pop()).await;
        assert_eq!((ask["kind"].as_str(), ask["server"].as_str(), ask["tool_name"].as_str()), (Some("mcp"), Some("web"), Some("echo")));
        assert_eq!((&ask["tool"], &ask["input"], &ask["tool_use_id"]), (&json!(tools[0].api_name), &json!({"text": "hi"}), &json!("toolu_1")));
        let req = ask["request_id"].as_str().unwrap();
        assert!(h.st.pending_perms.lock().contains_key(req), "shown to a view opened now");
        // what the permission_respond command does
        let tx = h.st.permissions.lock().remove(req).expect("waiting for the answer");
        tx.send(PermissionReply { decision: decision.into(), message: message.map(Into::into) }).unwrap();

        assert_eq!(h.ended().await["error"], Value::Null);
        h.assert_reset();
        assert_eq!(h.events("permission-resolved").iter().map(|e| e["request_id"].clone()).collect::<Vec<_>>(), vec![json!(req)]);
        let path = h.path();
        let roles: Vec<&str> = path.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
        for w in path.windows(2) {
            assert_eq!(w[1].parent_id.as_ref(), Some(&w[0].id));
        }
        assert_eq!(path[1].content, json!([{"type": "text", "text": "Let me check."}, {"type": "tool_use", "id": "toolu_1", "name": tools[0].api_name, "input": {"text": "hi"}}]));
        assert_eq!(path[1].meta["stop_reason"], "tool_use");
        assert_eq!(path[2].meta["tool_results"], true);
        assert_eq!(path[3].content, json!([{"type": "text", "text": "Done."}]));
        let sent = api.streams();
        assert_eq!(sent.len(), 2);
        let result = sent[1]["messages"].as_array().unwrap().last().unwrap()["content"][0].clone();
        assert_eq!(result, path[2].content[0], "the API got the stored tool result");
        (api.mcp_calls(), result)
    })
}

#[test]
fn an_allowed_tool_call_runs_and_the_turn_continues() {
    let (calls, result) = tool_round_trip("once", None);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["params"], json!({"name": "echo", "arguments": {"text": "hi"}}));
    assert_eq!(result, json!({"type": "tool_result", "tool_use_id": "toolu_1", "content": [{"type": "text", "text": "hi"}]}));
}

#[test]
fn a_denied_tool_call_is_not_run_and_claude_is_told() {
    let (calls, result) = tool_round_trip("deny", Some("not now"));
    assert!(calls.is_empty(), "the tool ran: {calls:?}");
    assert_eq!(result["is_error"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("The user denied this tool call.") && text.ends_with("The user said: not now"), "{text}");
}

#[test]
fn a_dropped_connection_ends_the_turn_and_keeps_the_partial_reply() {
    block_on(async {
        let api = api(|b| {
            let mut v = vec![message_start(b)];
            v.extend(text_block(0, &["Partial ", "answer"]).into_iter().take(3));
            Reply::Drop(v)
        })
        .await;
        let h = chat(&api);
        h.send("Hi");
        let end = h.ended().await;
        assert!(end["error"]["message"].is_string(), "{end}");
        h.assert_reset();
        let path = h.path();
        assert_eq!(path.len(), 2);
        assert_eq!(path[1].content, json!([{"type": "text", "text": "Partial answer"}]));
        assert_eq!(path[1].meta["error"]["message"], end["error"]["message"]);
        assert!(path[1].meta["stopped"].is_null());
    });
}

#[test]
fn stop_mid_stream_keeps_what_had_arrived() {
    block_on(async {
        let api = api(stalled).await;
        let h = chat(&api);
        h.send("Hi");
        until("the reply to stream", || h.path().get(1).filter(|m| m.content[0]["text"] == "Partial answer").map(|_| ())).await;
        engine::stop(&h.st, &h.conv);
        assert_eq!(h.ended().await["error"], Value::Null, "Stop is not an error");
        h.assert_reset();
        let a = &h.path()[1];
        assert_eq!(a.meta["stopped"], true);
        assert_eq!(a.content[0], json!({"type": "text", "text": "Partial answer"}));
        assert!(a.meta["error"].is_null());
    });
}

#[test]
fn a_reply_cut_off_by_a_crash_is_kept_and_shown_stopped() {
    block_on(async {
        let api = api(stalled).await;
        let dir = temp_dir();
        let file = dir.join("app.db");
        let h = Harness::new(&api, Db::open(&file).unwrap(), dir);
        h.send("Hi");
        // the database as the next start finds it, while the reply is still streaming
        let next = Db::open(&file).unwrap();
        let leaf = h.st.db.conv_get(&h.conv).unwrap().current_leaf.unwrap();
        until("the reply so far to be saved", || (next.msg_get(&leaf).unwrap().content[0]["text"] == "Partial answer").then_some(())).await;
        assert!(next.msg_get(&leaf).unwrap().meta["stop_reason"].is_null(), "unfinished");
        assert_eq!(next.mark_interrupted().unwrap(), 1, "what every start runs");
        let saved = next.msg_get(&leaf).unwrap();
        assert_eq!(saved.meta["stopped"], true);
        assert_eq!(saved.content[0], json!({"type": "text", "text": "Partial answer"}));
        engine::stop(&h.st, &h.conv);
        h.ended().await;
        until("the turn to let go of the database", || (Arc::strong_count(&h.st) == 1).then_some(())).await;
    });
}

/// Block `i` of a long reply: 20,000 characters, so each save of the reply so
/// far takes a while to write.
fn long_text(i: usize) -> String {
    format!("{i:02}{}", "x".repeat(20_000))
}

/// The 40 blocks of a long reply: each new one makes a save due, so saves are
/// still being written as the reply ends.
fn forty_blocks() -> Vec<Vec<Value>> {
    (0..40).map(|i| text_block(i, &[long_text(i).as_str()])).collect()
}

#[test]
fn the_final_write_lands_after_the_saves_made_while_streaming() {
    for how in ["end", "drop", "stop"] {
        block_on(async {
            let api = api(move |b| {
                let start = || std::iter::once(message_start(b)).chain(forty_blocks().into_iter().flatten()).collect();
                match how {
                    "end" => Reply::Stream(reply(b, forty_blocks(), "end_turn")),
                    "drop" => Reply::Drop(start()),
                    _ => Reply::Stall(start()),
                }
            })
            .await;
            let h = chat(&api);
            h.send("Hi");
            if how == "stop" {
                until("the reply to stream", || h.path().get(1).filter(|m| m.content.as_array().unwrap().len() == 40).map(|_| ())).await;
                engine::stop(&h.st, &h.conv);
            }
            let end = h.ended().await;
            // the saves' writer is the only other holder of the database
            assert_eq!(Arc::strong_count(&h.st.db), 1, "{how}: a save is still being written after the turn ended");
            h.assert_reset();
            let a = &h.path()[1];
            let texts: Vec<&str> = a.content.as_array().unwrap().iter().map(|b| b["text"].as_str().unwrap()).collect();
            let last = texts.last().map_or(0, |t| t.len());
            assert!(texts == (0..40).map(long_text).collect::<Vec<_>>(), "{how}: {} blocks stored, the last {last} characters long", texts.len());
            match how {
                "end" => assert_eq!(a.meta["stop_reason"], "end_turn"),
                "drop" => assert_eq!(a.meta["error"]["message"], end["error"]["message"]),
                _ => assert_eq!(a.meta["stopped"], true),
            }
        });
    }
}

#[test]
fn a_rate_limited_request_is_retried() {
    block_on(async {
        let n = AtomicUsize::new(0);
        let api = api(move |b| match n.fetch_add(1, Ordering::SeqCst) {
            0 => Reply::Plain(429, vec![("retry-after", "0")], json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}}).to_string()),
            _ => Reply::Stream(reply(b, vec![text_block(0, &["Hello"])], "end_turn")),
        })
        .await;
        let h = chat(&api);
        h.send("Hi");
        assert_eq!(h.ended().await["error"], Value::Null);
        h.assert_reset();
        assert_eq!(api.streams().len(), 2);
        let a = &h.path()[1];
        assert_eq!(a.content, json!([{"type": "text", "text": "Hello"}]));
        assert!(a.meta["error"].is_null());
    });
}

#[test]
fn an_overloaded_model_falls_back_to_another() {
    block_on(async {
        let api = api(|b| {
            if b["model"] == "claude-opus-5-5" {
                return Reply::Plain(529, vec![("x-should-retry", "false")], json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}).to_string());
            }
            Reply::Stream(reply(b, vec![text_block(0, &["Hello"])], "end_turn"))
        })
        .await;
        let h = chat(&api);
        h.send("Hi");
        assert_eq!(h.ended().await["error"], Value::Null);
        h.assert_reset();
        let models: Vec<Value> = api.streams().iter().map(|b| b["model"].clone()).collect();
        assert_eq!(models, vec![json!("claude-opus-5-5"), json!("claude-sonnet-5-5")]);
        let a = &h.path()[1];
        assert_eq!(a.meta["overload_fallback"], json!({"from": "claude-opus-5-5", "to": "claude-sonnet-5-5"}));
        assert_eq!(a.meta["model"], "claude-sonnet-5-5");
        assert_eq!(a.content, json!([{"type": "text", "text": "Hello"}]));
    });
}

#[test]
fn an_api_error_is_stored_and_reported() {
    block_on(async {
        let api = api(|_| {
            Reply::Plain(400, vec![], json!({"type": "error", "error": {"type": "invalid_request_error", "message": "max_tokens: must be positive"}}).to_string())
        })
        .await;
        let h = chat(&api);
        h.send("Hi");
        let want = json!({"message": "max_tokens: must be positive", "kind": "invalid_request_error", "status": 400});
        assert_eq!(h.ended().await["error"], want);
        h.assert_reset();
        assert_eq!(api.streams().len(), 1, "not retried");
        let a = &h.path()[1];
        assert_eq!(a.content, json!([]));
        assert_eq!(a.meta["error"], want);
    });
}
