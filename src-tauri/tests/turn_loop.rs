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

    fn paths(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|(p, _)| p.clone()).collect()
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
    let reply = if method == "GET" && path.starts_with("/v1/files/") {
        script(&json!({"get": path}))
    } else if method != "POST" {
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

const EVENTS: [&str; 7] = ["generation-state", "turn", "permission-request", "permission-resolved", "conversation-updated", "generation-retry", "research-progress"];

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
        self.send_with(text, json!({}))
    }

    /// The same with the message's `meta` (what the send request sets, as `research`).
    fn send_with(&self, text: &str, meta: Value) -> String {
        let id = clauderust_lib::util::new_id();
        let m = Message {
            id: id.clone(),
            conversation_id: self.conv.clone(),
            parent_id: None,
            role: "user".into(),
            content: json!([{"type": "text", "text": text}]),
            meta,
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
        // sent just after the state event, behind the turn's own housekeeping writes: a slow machine needs a moment
        let t0 = Instant::now();
        while self.events("conversation-updated").is_empty() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
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
        // the thinking with how long it took (from its start to its stop)
        let ms = a.content[0]["thinking_ms"].clone();
        assert!(ms.is_u64(), "{}", a.content);
        assert_eq!(a.content, json!([{"type": "thinking", "thinking": "Let me think", "signature": "sig", "thinking_ms": ms}, {"type": "text", "text": "Hello world"}]));
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

/// Wait for the conversation's `n`th turn to end.
async fn ended_times(h: &Harness, n: usize) {
    until("the turn to end", || (h.events("generation-state").iter().filter(|e| e["running"] == false).count() >= n).then_some(())).await;
    until("the turn to let go", || (!h.st.runs.lock().contains_key(&h.conv) && !h.st.live.lock().contains_key(&h.conv)).then_some(())).await;
}

/// Is this request the one that asks for the rest of a reply that was cut off?
fn asks_to_go_on(b: &Value) -> bool {
    b["messages"].as_array().unwrap().last().unwrap()["content"][0]["text"] == engine::CONTINUE_PROMPT
}

#[test]
fn a_cut_off_reply_continues_and_ends_as_one_message() {
    block_on(async {
        let api = api(|b| {
            if asks_to_go_on(b) {
                // it starts again a word back, as models sometimes do
                return Reply::Stream(reply(b, vec![text_block(0, &["answer", " goes on."])], "end_turn"));
            }
            let mut v = vec![message_start(b)];
            v.extend(text_block(0, &["Partial ", "answer"]).into_iter().take(3));
            Reply::Drop(v)
        })
        .await;
        let h = chat(&api);
        h.st.db.conv_set_model(&h.conv, "claude-haiku-5-5").unwrap();
        h.st.db.conv_set_options(&h.conv, &json!({"thinking": true})).unwrap();
        h.send("Hi");
        assert!(h.ended().await["error"]["message"].is_string());
        let cut = h.path()[1].clone();
        assert!(engine::continuable(&cut), "{:?}", cut.meta);

        let id = engine::continue_reply(h.app.handle(), &h.st, &h.conv).await.unwrap();
        assert_eq!(id, cut.id);
        ended_times(&h, 2).await;
        let last = h.events("generation-state").pop().unwrap();
        assert_eq!(last["error"], Value::Null);

        // one message: the same row, its text joined without the repeat
        let path = h.path();
        assert_eq!(path.len(), 2);
        let a = &path[1];
        assert_eq!(a.id, cut.id);
        assert_eq!(a.content, json!([{"type": "text", "text": "Partial answer goes on."}]));
        assert_eq!(a.meta["stop_reason"], "end_turn");
        assert!(a.meta["error"].is_null() && a.meta["stopped"].is_null(), "{}", a.meta);
        assert_eq!(a.meta["usage"]["input_tokens"], 20, "both requests are counted: {}", a.meta["usage"]);
        assert_eq!(a.meta["model"], "claude-haiku-5-5");
        assert_eq!(h.st.db.search("goes", 5).unwrap().len(), 1, "indexed again, once");

        // asked with the reply so far and a turn asking for the rest, thinking off
        let sent = api.streams();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[1]["messages"],
            json!([
                {"role": "user", "content": [{"type": "text", "text": "Hi"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Partial answer"}]},
                {"role": "user", "content": [{"type": "text", "text": engine::CONTINUE_PROMPT}]},
            ])
        );
        assert_eq!(sent[0]["thinking"]["type"], "adaptive");
        assert_eq!(sent[1]["thinking"], json!({"type": "disabled"}));
        assert_eq!(sent[1]["model"], "claude-haiku-5-5");

        // a reply that ended can't be continued again
        assert!(engine::continue_reply(h.app.handle(), &h.st, &h.conv).await.is_err());
    });
}

/// The gallery after a reply cut off in an artifact was continued with `rest` (the API engine joins it to the reply's text).
fn gallery_after_continuing_an_artifact(rest: &'static str) -> Vec<(String, String)> {
    block_on(async {
        let api = api(move |b| {
            if asks_to_go_on(b) {
                return Reply::Stream(reply(b, vec![text_block(0, &[rest])], "end_turn"));
            }
            let mut v = vec![message_start(b)];
            v.extend(text_block(0, &["<antArtifact identifier=\"story\" type=\"text/markdown\" title=\"Story\">\n# Story\n\nOnce upon"]).into_iter().take(2));
            Reply::Drop(v)
        })
        .await;
        let h = chat(&api);
        h.send("Write a story");
        assert!(h.ended().await["error"]["message"].is_string());
        engine::continue_reply(h.app.handle(), &h.st, &h.conv).await.unwrap();
        ended_times(&h, 2).await;
        h.st.db.artifact_list(10).unwrap().into_iter().filter(|a| a.conversation_id == h.conv).map(|a| (a.identifier, a.content)).collect()
    })
}

#[test]
fn an_artifact_cut_off_and_continued_reaches_the_gallery() {
    let story = vec![("story".to_string(), "# Story\n\nOnce upon a time.".to_string())];
    // the rest of it, in the same text
    assert_eq!(gallery_after_continuing_an_artifact(" a time.\n</antArtifact>"), story);
    // or it opened again as an update of what was cut off: that one ends where the update starts
    let update = "<antArtifact identifier=\"story\" type=\"text/markdown\" title=\"Story\" command=\"update\">\n<old_str>Once upon</old_str>\n<new_str>Once upon a time.</new_str>\n</antArtifact>";
    assert_eq!(gallery_after_continuing_an_artifact(update), story);
}

#[test]
fn a_continuation_that_is_stopped_keeps_both_parts() {
    block_on(async {
        let api = api(|b| {
            let mut v = vec![message_start(b)];
            let piece = if asks_to_go_on(b) { " and more" } else { "Partial answer" };
            v.extend(text_block(0, &[piece]).into_iter().take(2));
            Reply::Stall(v)
        })
        .await;
        let h = chat(&api);
        h.send("Hi");
        until("the reply to stream", || h.path().get(1).filter(|m| m.content[0]["text"] == "Partial answer").map(|_| ())).await;
        engine::stop(&h.st, &h.conv);
        ended_times(&h, 1).await;
        assert_eq!(h.path()[1].meta["stopped"], true);

        engine::continue_reply(h.app.handle(), &h.st, &h.conv).await.unwrap();
        // what a view opened now is shown: the reply as one text
        let joined = json!([{"type": "text", "text": "Partial answer and more"}]);
        until("the continuation to stream", || h.st.live.lock().get(&h.conv).filter(|l| l.content == joined).map(|_| ())).await;
        // saved as it streams, so a crash keeps it, no longer marked stopped
        let saved = until("the continuation to be saved", || h.path().get(1).filter(|m| m.content == joined).cloned()).await;
        assert!(saved.meta["stopped"].is_null() && saved.meta["stop_reason"].is_null(), "{}", saved.meta);
        engine::stop(&h.st, &h.conv);
        ended_times(&h, 2).await;
        let a = &h.path()[1];
        assert_eq!(a.content, json!([{"type": "text", "text": "Partial answer and more"}]));
        assert_eq!(a.meta["stopped"], true);
        assert!(engine::continuable(a), "stopped again: it can go on again");
        assert_eq!(a.meta["usage"]["input_tokens"], 20);
    });
}

#[test]
fn a_continue_on_a_finished_reply_is_refused() {
    block_on(async {
        let api = api(|b| Reply::Stream(reply(b, vec![text_block(0, &["Hello"])], "end_turn"))).await;
        let h = chat(&api);
        h.send("Hi");
        assert_eq!(h.ended().await["error"], Value::Null);
        let before = h.path();
        let err = engine::continue_reply(h.app.handle(), &h.st, &h.conv).await.unwrap_err();
        assert!(err.to_string().contains("cut off"), "{err}");
        assert_eq!(api.streams().len(), 1, "nothing was asked of the API");
        h.assert_reset();
        let after = h.path();
        assert_eq!((after.len(), &after[1].content, &after[1].meta), (before.len(), &before[1].content, &before[1].meta));
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
        // quiet in the middle of a block: no later event makes the save due, only time
        let api = api(|b| {
            let mut v = vec![message_start(b)];
            v.extend(text_block(0, &["Partial ", "answer"]).into_iter().take(3));
            Reply::Stall(v)
        })
        .await;
        let dir = temp_dir();
        let file = dir.join("app.db");
        let h = Harness::new(&api, Db::open(&file).unwrap(), dir);
        let user = h.send("Hi");
        // the database as the next start finds it, while the reply is still streaming;
        // the reply's row is the leaf only once the turn has stored it
        let next = Db::open(&file).unwrap();
        let saved = until("the reply so far to be saved", || {
            let leaf = next.conv_get(&h.conv).unwrap().current_leaf?;
            next.msg_get(&leaf).ok().filter(|m| m.content[0]["text"] == "Partial answer")
        })
        .await;
        assert_eq!(saved.parent_id.as_deref(), Some(user.as_str()));
        assert!(saved.meta["stop_reason"].is_null(), "unfinished");
        assert_eq!(next.mark_interrupted().unwrap(), 1, "what every start runs");
        let saved = next.msg_get(&saved.id).unwrap();
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
                // all 40 blocks and the whole text of the last one: Stop keeps what has been read, and a slow machine may not have read the end yet
                let whole = long_text(39).len();
                until("the reply to stream", || h.path().get(1).filter(|m| m.content.as_array().unwrap().len() == 40 && m.content[39]["text"].as_str().is_some_and(|t| t.len() == whole)).map(|_| ())).await;
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
        let reasons: Vec<Value> = h.events("generation-retry").iter().map(|r| r["reason"].clone()).collect();
        assert_eq!(reasons, vec![json!("rate_limited")]);
    });
}

#[test]
fn a_server_error_is_retried_and_the_wait_is_announced() {
    block_on(async {
        let n = AtomicUsize::new(0);
        let api = api(move |b| match n.fetch_add(1, Ordering::SeqCst) {
            0 => Reply::Plain(500, vec![("retry-after", "0")], json!({"type": "error", "error": {"type": "api_error", "message": "Internal server error"}}).to_string()),
            _ => Reply::Stream(reply(b, vec![text_block(0, &["Hello"])], "end_turn")),
        })
        .await;
        let h = chat(&api);
        h.send("Hi");
        assert_eq!(h.ended().await["error"], Value::Null);
        h.assert_reset();
        assert_eq!(h.path()[1].content, json!([{"type": "text", "text": "Hello"}]));
        let retries = h.events("generation-retry");
        assert_eq!(retries.len(), 1, "{retries:?}");
        let wait = retries[0]["wait_ms"].as_u64().unwrap();
        assert!(wait < 1000, "retry-after 0 waits only the jitter, not {wait} ms");
        assert_eq!(retries[0], json!({"conversation_id": h.conv, "attempt": 1, "max_attempts": 4, "wait_ms": wait, "reason": "server_error"}));
        // what takes the notice down: the reply's own turn events, after it
        let ev = h.events.lock().unwrap();
        let at = ev.iter().position(|(n, _)| n == "generation-retry").unwrap();
        assert!(ev[at..].iter().any(|(n, p)| n == "turn" && p["conversation_id"] == h.conv.as_str()), "no turn event after the retry");
    });
}

#[test]
fn a_broken_reply_from_an_openai_compatible_endpoint_keeps_what_had_arrived() {
    block_on(async {
        let api = api(|_| Reply::Drop(vec![json!({"choices": [{"delta": {"content": "Partial "}}]}), json!({"choices": [{"delta": {"content": "answer"}}]})])).await;
        let h = chat(&api);
        h.st.settings.write().oai_base_url = format!("{}/v1", api.url);
        h.st.db.conv_set_model(&h.conv, "oai:local").unwrap();
        h.send("Hi");
        let end = h.ended().await;
        assert!(end["error"]["message"].is_string(), "{end}");
        h.assert_reset();
        let a = &h.path()[1];
        assert_eq!(a.content, json!([{"type": "text", "text": "Partial answer"}]));
        assert_eq!(a.meta["error"]["message"], end["error"]["message"]);
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
        assert!(h.events("generation-retry").is_empty(), "the API said not to retry");
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

/// A Files API file: its metadata and content.
fn file_reply(get: &str) -> Reply {
    let meta = |name: &str, size: u64| Reply::Plain(200, vec![], json!({"id": "f", "type": "file", "filename": name, "mime_type": "text/plain", "size_bytes": size, "downloadable": true}).to_string());
    match get {
        "/v1/files/file_ok" => meta("report.txt", 10),
        "/v1/files/file_ok/content" => Reply::Plain(200, vec![], "hello file".into()),
        "/v1/files/file_big" => meta("big.bin", 60 * 1024 * 1024),
        "/v1/files/file_evil" => meta("../../evil.txt", 4),
        _ => Reply::Plain(404, vec![], json!({"type": "error", "error": {"type": "not_found_error", "message": "no"}}).to_string()),
    }
}

#[test]
fn files_made_by_code_execution_are_downloaded_and_attached() {
    block_on(async {
        let api = api(|b| {
            if let Some(get) = b["get"].as_str() {
                return file_reply(get);
            }
            if b["container"]["id"] == "container_1" {
                return Reply::Stream(reply(b, vec![text_block(0, &["Here is your report."])], "end_turn"));
            }
            let outputs: Vec<Value> = ["file_ok", "file_big", "file_evil", "file_../../x"].iter().map(|id| json!({"type": "bash_code_execution_output", "file_id": id})).collect();
            Reply::Stream(vec![
                message_start(b),
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "id": "srvtoolu_1", "name": "bash_code_execution", "input": {}}}),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"command\": \"python make.py\"}"}}),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "bash_code_execution_tool_result", "tool_use_id": "srvtoolu_1",
                    "content": {"type": "bash_code_execution_result", "stdout": "done", "stderr": "", "return_code": 0, "content": outputs}}}),
                json!({"type": "content_block_stop", "index": 1}),
                json!({"type": "message_delta", "delta": {"stop_reason": "pause_turn", "container": {"id": "container_1", "expires_at": "2026-11-22T12:00:00Z"}}, "usage": {"output_tokens": 5}}),
                json!({"type": "message_stop"}),
            ])
        })
        .await;
        let h = chat(&api);
        h.send("Make me a report file");
        assert_eq!(h.ended().await["error"], Value::Null);
        h.assert_reset();

        let sent = api.streams();
        assert_eq!(sent.len(), 2, "the paused turn is continued");
        assert!(sent[0]["tools"].as_array().unwrap().iter().any(|t| t["type"] == "code_execution_20260521"), "{}", sent[0]["tools"]);
        let skills: Vec<&str> = sent[0]["container"]["skills"].as_array().unwrap().iter().map(|s| s["skill_id"].as_str().unwrap()).collect();
        assert_eq!(skills, ["xlsx", "pptx", "docx", "pdf"]);
        assert!(sent[0]["container"].get("id").is_none(), "a new container");
        assert_eq!(sent[1]["container"]["id"], "container_1", "the continuation runs in the same container");
        assert_eq!(h.st.db.conv_get(&h.conv).unwrap().options["container"]["id"], "container_1", "kept for the next turn");

        let a = &h.path()[1];
        assert_eq!(a.content.as_array().unwrap().last().unwrap(), &json!({"type": "text", "text": "Here is your report."}));
        let files = a.meta["files"].as_array().unwrap();
        let dir = h.dir.0.join("outputs").join(&h.conv);
        assert_eq!(files[0]["name"], "report.txt");
        assert_eq!(files[0]["size"], 10);
        assert_eq!(files[0]["path"], dir.join("report.txt").display().to_string());
        assert_eq!(std::fs::read_to_string(dir.join("report.txt")).unwrap(), "hello file");
        let refused: Vec<(&str, &str)> = files[1..].iter().map(|f| (f["name"].as_str().unwrap_or(""), f["error"].as_str().unwrap())).collect();
        assert_eq!(refused, [("big.bin", "too_large"), ("../../evil.txt", "bad_name"), ("", "bad_id")]);
        // refused files are never fetched, and nothing lands outside the conversation's folder
        let paths = api.paths();
        assert!(!paths.iter().any(|p| p.contains("file_big/content") || p.contains("file_evil/content") || p.contains("..")), "{paths:?}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert!(!h.dir.0.join("evil.txt").exists() && !h.dir.0.join("outputs").join("evil.txt").exists());
    });
}

#[test]
fn an_expired_container_is_replaced_and_forgotten() {
    block_on(async {
        let api = api(|b| {
            if b["container"]["id"] == "container_old" {
                return Reply::Plain(400, vec![], json!({"type": "error", "error": {"type": "invalid_request_error", "message": "container container_old has expired"}}).to_string());
            }
            Reply::Stream(reply(b, vec![text_block(0, &["Hello"])], "end_turn"))
        })
        .await;
        let h = chat(&api);
        h.st.db.conv_set_options(&h.conv, &json!({"container": {"id": "container_old"}})).unwrap();
        h.send("Hi");
        assert_eq!(h.ended().await["error"], Value::Null);
        let ids: Vec<Value> = api.streams().iter().map(|b| b["container"]["id"].clone()).collect();
        assert_eq!(ids, [json!("container_old"), Value::Null]);
        assert!(h.st.db.conv_get(&h.conv).unwrap().options["container"].is_null(), "not asked for again");
        assert_eq!(h.path()[1].content, json!([{"type": "text", "text": "Hello"}]));
        assert!(!h.st.degraded.load(Ordering::Relaxed), "code execution stays on");
    });
}

#[test]
fn a_research_turn_plans_searches_in_parallel_and_writes_a_cited_report() {
    block_on(async {
        let api = api(|b| {
            let system = b["system"][0]["text"].as_str().unwrap_or("");
            if system.starts_with("You plan web research") {
                return Reply::Stream(reply(b, vec![text_block(0, &["1. Sub one\n2. Sub two"])], "end_turn"));
            }
            if system.starts_with("You are a research agent") {
                let sub = b["messages"][0]["content"][0]["text"].as_str().unwrap();
                let n = if sub.ends_with("Sub one") { 1 } else { 2 };
                let url = format!("https://example.com/{n}");
                return Reply::Stream(reply(b, vec![
                    vec![
                        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "id": format!("srvtoolu_{n}"), "name": "web_search", "input": {}}}),
                        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": json!({"query": format!("query {n}")}).to_string()}}),
                        json!({"type": "content_block_stop", "index": 0}),
                        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "web_search_tool_result", "tool_use_id": format!("srvtoolu_{n}"),
                            "content": [{"type": "web_search_result", "url": url, "title": format!("Page {n}")}]}}),
                        json!({"type": "content_block_stop", "index": 1}),
                        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": "",
                            "citations": [{"type": "web_search_result_location", "url": url, "title": format!("Page {n}"), "cited_text": "x"}]}}),
                        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": format!("Finding {n}.")}}),
                        json!({"type": "content_block_stop", "index": 2}),
                    ],
                ], "end_turn"));
            }
            Reply::Stream(reply(b, vec![text_block(0, &["Report: see [Page 1](https://example.com/1)."])], "end_turn"))
        })
        .await;
        let h = chat(&api);
        h.send_with("What happened?", json!({"research": true}));
        assert_eq!(h.ended().await["error"], Value::Null);
        h.assert_reset();

        let sent = api.streams();
        assert_eq!(sent.len(), 4, "a plan, two agents, the report");
        let system = |b: &Value| b["system"][0]["text"].as_str().unwrap().to_string();
        let agents: Vec<&Value> = sent.iter().filter(|b| system(b).starts_with("You are a research agent")).collect();
        assert_eq!(agents.len(), 2);
        for a in &agents {
            let tools: Vec<&str> = a["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
            assert_eq!(tools, ["web_search", "web_fetch"], "web tools only");
            assert!(a.get("container").is_none());
        }
        let report = sent.last().unwrap();
        assert_eq!(report["tool_choice"], json!({"type": "none"}), "the report calls no tools");
        let asked = report["messages"].as_array().unwrap().last().unwrap()["content"].as_array().unwrap();
        assert_eq!(asked[0]["text"], "What happened?");
        let findings = asked.last().unwrap()["text"].as_str().unwrap();
        for want in ["<research_findings>", "question=\"Sub one\"", "Finding 1.", "Finding 2.", "- Page 2: https://example.com/2", "Where sources disagree"] {
            assert!(findings.contains(want), "{want} not in {findings}");
        }

        let a = &h.path()[1];
        assert_eq!(a.content, json!([{"type": "text", "text": "Report: see [Page 1](https://example.com/1)."}]));
        assert_eq!(a.meta["research"]["questions"], json!(["Sub one", "Sub two"]));
        assert_eq!(a.meta["research"]["sources"], 2);
        assert_eq!(a.meta["fingerprint"], "research");
        assert!(a.meta["subagent_usage"]["input_tokens"].as_u64().unwrap() >= 30, "the plan's and agents' tokens: {}", a.meta["subagent_usage"]);
        let progress = h.events("research-progress");
        let phases: Vec<&str> = progress.iter().map(|p| p["phase"].as_str().unwrap()).collect();
        assert_eq!(phases.first(), Some(&"planning"));
        assert_eq!(phases.last(), Some(&"writing"));
        assert!(progress.iter().any(|p| p["queries"].as_array().unwrap().iter().any(|q| q == "query 1")), "{progress:?}");
        assert!(progress.iter().any(|p| p["done"] == 2 && p["total"] == 2));
    });
}

#[test]
fn stop_ends_a_research_turn_before_its_report() {
    block_on(async {
        let api = api(|b| match b["system"][0]["text"].as_str().unwrap_or("") {
            s if s.starts_with("You plan web research") => Reply::Stream(reply(b, vec![text_block(0, &["Sub one"])], "end_turn")),
            s if s.starts_with("You are a research agent") => stalled(b),
            _ => Reply::Stream(reply(b, vec![text_block(0, &["Report"])], "end_turn")),
        })
        .await;
        let h = chat(&api);
        h.send_with("What happened?", json!({"research": true}));
        until("the agent to start", || (api.streams().len() == 2).then_some(())).await;
        engine::stop(&h.st, &h.conv);
        assert_eq!(h.ended().await["error"], Value::Null, "Stop is not an error");
        h.assert_reset();
        assert_eq!(api.streams().len(), 2, "no report is asked for after Stop");
        let a = &h.path()[1];
        assert_eq!((a.meta["stopped"].clone(), a.content.clone()), (json!(true), json!([])));
    });
}

#[test]
fn fast_mode_falls_back_to_standard_speed() {
    block_on(async {
        let api = api(|b| {
            if b["speed"] == "fast" {
                return Reply::Plain(429, vec![("x-should-retry", "false")], json!({"type": "error", "error": {"type": "rate_limit_error", "message": "fast mode capacity"}}).to_string());
            }
            Reply::Stream(reply(b, vec![text_block(0, &["Hello"])], "end_turn"))
        })
        .await;
        let h = chat(&api);
        // the option the composer sets, which the account engine reads too
        h.st.db.conv_set_options(&h.conv, &json!({"fast": true})).unwrap();
        h.send("Hi");
        assert_eq!(h.ended().await["error"], Value::Null);
        h.assert_reset();
        let speeds: Vec<Value> = api.streams().iter().map(|b| b["speed"].clone()).collect();
        assert_eq!(speeds, [json!("fast"), Value::Null]);
        assert_eq!(h.path()[1].content, json!([{"type": "text", "text": "Hello"}]));
    });
}

#[test]
fn a_large_project_is_searched_through_a_tool() {
    block_on(async {
        let api = api(|b| {
            let msgs = b["messages"].as_array().unwrap();
            if let Some(r) = msgs.last().unwrap()["content"].as_array().unwrap().iter().find(|c| c["type"] == "tool_result") {
                let text = r["content"][0]["text"].as_str().unwrap_or("").to_string();
                return Reply::Stream(reply(b, vec![text_block(0, &[&text])], "end_turn"));
            }
            Reply::Stream(reply(b, vec![tool_block(0, "toolu_p", "project_knowledge_search", &json!({"query": "chloroplast"}))], "tool_use"))
        })
        .await;
        let mut h = chat(&api);
        let p = h.st.db.project_create("Biology", "").unwrap();
        let mut text: String = (0..4000).map(|i| format!("Line {i} of ordinary filler text in the notes.\n")).collect();
        text.push_str("Photosynthesis happens in the chloroplast.\n");
        h.st.db
            .project_file_add(&clauderust_lib::db::ProjectFile { id: "pf1".into(), project_id: p.id.clone(), name: "notes.md".into(), mime: "text/markdown".into(), size: text.len() as i64, kind: "text".into(), data: text, created_at: 1 })
            .unwrap();
        h.conv = h.st.db.conv_create("chat", "claude-opus-5-5", Some(&p.id), None, &json!({})).unwrap().id;
        h.st.db.conv_set_title(&h.conv, "Test").unwrap();
        h.send("Where does photosynthesis happen?");
        assert_eq!(h.ended().await["error"], Value::Null);
        let sent = api.streams();
        let system = sent[0]["system"][0]["text"].as_str().unwrap();
        assert!(system.contains("<project_knowledge_index>") && system.contains("- notes.md ("), "{}", &system[system.len().saturating_sub(800)..]);
        assert!(!system.contains("Line 3999 of ordinary filler"), "the files are not sent whole");
        assert!(sent[0]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "project_knowledge_search"));
        let answer = h.path().last().unwrap().content[0]["text"].as_str().unwrap().to_string();
        assert!(answer.starts_with("<passage file=\"notes.md\"") && answer.contains("Photosynthesis happens in the chloroplast."), "{answer}");
    });
}

/// The names of the tools a request offered.
fn offered(body: &Value) -> Vec<String> {
    body["tools"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str().map(String::from)).collect()
}

#[test]
fn a_chat_that_loads_tools_when_needed_sends_a_schema_once_the_model_loads_it() {
    block_on(async {
        let api = api(|b| {
            let msgs = b["messages"].as_array().unwrap();
            let results = msgs.iter().filter(|m| m["content"].as_array().is_some_and(|c| c.iter().any(|x| x["type"] == "tool_result"))).count();
            Reply::Stream(match results {
                0 => reply(b, vec![tool_block(0, "toolu_1", "load_tools", &json!({"names": ["web__echo", "web__nope"]}))], "tool_use"),
                1 => reply(b, vec![tool_block(0, "toolu_2", "web__echo", &json!({"text": "hi"}))], "tool_use"),
                _ => reply(b, vec![text_block(0, &["Done."])], "end_turn"),
            })
        })
        .await;
        let h = chat(&api);
        std::fs::write(h.dir.0.join("mcp.json"), json!({"mcpServers": {"web": {"url": format!("{}/mcp", api.url)}}}).to_string()).unwrap();
        h.st.mcp.start_all(&[]).await;
        h.st.settings.write().tool_policies.insert("web/echo".into(), json!("allow"));
        h.st.db.conv_set_options(&h.conv, &json!({"tool_access": "deferred"})).unwrap();
        h.send("Echo hi");
        assert_eq!(h.ended().await["error"], Value::Null);

        let sent = api.streams();
        assert_eq!(sent.len(), 3);
        // first only the tool's name and line in the prompt, and load_tools
        let first = offered(&sent[0]);
        assert!(first.contains(&"load_tools".to_string()) && !first.contains(&"web__echo".to_string()), "{first:?}");
        let system = sent[0]["system"].to_string();
        assert!(system.contains("- web__echo: Echo the text back"), "{system}");
        let loaded = sent[1]["messages"].as_array().unwrap().last().unwrap()["content"][0]["content"].to_string();
        assert!(loaded.contains("Loaded: web__echo.") && loaded.contains("Not available in this chat: web__nope."), "{loaded}");
        // then its schema, from the next request on; everything is loaded, so load_tools goes
        let second = offered(&sent[1]);
        assert!(second.contains(&"web__echo".to_string()) && !second.contains(&"load_tools".to_string()), "{second:?}");
        assert!(!sent[1]["system"].to_string().contains("<available_tools>"));
        assert_eq!(api.mcp_calls().len(), 1, "the loaded tool ran");
        assert_eq!(h.st.db.conv_get(&h.conv).unwrap().options["loaded_tools"], json!(["web__echo"]), "kept for the conversation");
    });
}

#[test]
fn a_switched_off_tool_is_neither_listed_nor_loadable() {
    block_on(async {
        let api = api(|b| Reply::Stream(reply(b, vec![text_block(0, &["Hi."])], "end_turn"))).await;
        let h = chat(&api);
        std::fs::write(h.dir.0.join("mcp.json"), json!({"mcpServers": {"web": {"url": format!("{}/mcp", api.url)}}}).to_string()).unwrap();
        h.st.mcp.start_all(&[]).await;
        h.st.db.conv_set_options(&h.conv, &json!({"tool_access": "deferred", "disabled_tools": ["web__echo"]})).unwrap();
        h.send("Hi");
        assert_eq!(h.ended().await["error"], Value::Null);
        let sent = api.streams();
        assert!(!offered(&sent[0]).iter().any(|n| n == "load_tools" || n == "web__echo"), "{:?}", offered(&sent[0]));
        assert!(!sent[0]["system"].to_string().contains("web__echo"));
    });
}
