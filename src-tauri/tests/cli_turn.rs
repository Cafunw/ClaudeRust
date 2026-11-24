//! The account engine's turn loop end to end: `claudecode::run` on Tauri's mock runtime with a real
//! database, against a stand-in for the Claude Code CLI that reads the stream-json lines the app writes
//! and answers with the lines each test scripts. No model runs and nothing is spent.
//!
//! The stand-in is a node script: node is on every CI runner, and on Windows the app starts it through an
//! npm-style `claude.cmd` shim exactly as it starts an npm-installed CLI. The app finds it through the
//! Claude Code path setting (Settings → Code), the one a person sets for their own CLI.

use clauderust_lib::claudecode::{self, Cmd};
use clauderust_lib::db::{Db, Message};
use clauderust_lib::engine;
use clauderust_lib::error::AppResult;
use clauderust_lib::render;
use clauderust_lib::state::{AppState, PermissionReply};
use clauderust_lib::util::new_id;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};
use tauri::test::MockRuntime;
use tauri::Listener;
use tokio_util::sync::CancellationToken;

// ------------------------------------------------------------- the stand-in CLI

/// Plays the steps of `script.json` (one list per start of the CLI, each start takes the first) and logs
/// its arguments, every line it reads and how it exited to `log.jsonl`. Steps: `["out", line]` writes a
/// line, `["err", text]` writes to stderr, `["read", pattern]` waits for a line that has the pattern's
/// fields, `["exit", code]` exits, `["stay"]` keeps it running after its input ends (as a background
/// process of the CLI's can), `["sleep", ms]` waits. Then, as the CLI does, it ends when its input does.
/// Input that isn't JSON (a title's conversation) is logged as its text.
const FAKE_CLI: &str = r#"
const fs = require("fs");
const path = require("path");
const at = (f) => path.join(__dirname, f);
const log = (o) => fs.appendFileSync(at("log.jsonl"), JSON.stringify(o) + "\n");
const runs = JSON.parse(fs.readFileSync(at("script.json"), "utf8"));
const steps = runs.shift() || [];
fs.writeFileSync(at("script.json"), JSON.stringify(runs));
log({ argv: process.argv.slice(2), pid: process.pid });
process.on("exit", (code) => log({ exit: code }));
const input = [];
let closed = false;
let wake = () => {};
require("readline").createInterface({ input: process.stdin })
  .on("line", (l) => { let v; try { v = JSON.parse(l); } catch { v = l; } log({ in: v }); input.push(v); wake(); })
  .on("close", () => { closed = true; wake(); });
const like = (v, p) => (p !== null && typeof p === "object" ? v != null && Object.keys(p).every((k) => like(v[k], p[k])) : v === p);
const until = async (ready) => { while (!ready()) { if (closed) process.exit(3); await new Promise((r) => (wake = r)); } };
const flushed = () => Promise.all([process.stdout, process.stderr].map((s) => new Promise((r) => s.write("", r))));
(async () => {
  for (const [op, arg] of steps) {
    if (op === "out") process.stdout.write(JSON.stringify(arg) + "\n");
    else if (op === "err") process.stderr.write(arg + "\n");
    else if (op === "read") { let i = -1; await until(() => (i = input.findIndex((v) => like(v, arg))) >= 0); input.splice(0, i + 1); }
    else if (op === "exit") { await flushed(); process.exit(arg); }
    else if (op === "stay") setInterval(() => {}, 1 << 30);
    else if (op === "sleep") await new Promise((r) => setTimeout(r, arg));
  }
  while (!closed) await new Promise((r) => (wake = r));
})();
"#;

struct Cli {
    dir: PathBuf,
    program: PathBuf,
}

/// One start of the stand-in: its arguments, the lines it read, its exit code (None: it was killed).
struct Run {
    args: Vec<String>,
    pid: u32,
    input: Vec<Value>,
    exit: Option<i64>,
}

impl Run {
    /// Are these words among its arguments, one after the other?
    fn has(&self, words: &[&str]) -> bool {
        self.args.windows(words.len()).any(|w| w.iter().zip(words).all(|(a, b)| a == b))
    }

    /// The first line it read of `type`.
    fn first(&self, kind: &str) -> &Value {
        self.input.iter().find(|l| l["type"] == kind).unwrap_or_else(|| panic!("no {kind} line in {:?}", self.input))
    }

    /// The `response` of the app's answer to the CLI's request `id`.
    fn answer(&self, id: &str) -> &Value {
        &self.input.iter().find(|l| l["type"] == "control_response" && l["response"]["request_id"] == id).unwrap_or_else(|| panic!("no answer to {id}"))["response"]
    }
}

impl Cli {
    fn new(dir: PathBuf) -> Cli {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fake.cjs"), FAKE_CLI).unwrap();
        #[cfg(windows)]
        let (program, shim) = (dir.join("claude.cmd"), "@ECHO off\r\nSET dp0=%~dp0\r\nnode \"%dp0%\\fake.cjs\" %*\r\n");
        #[cfg(not(windows))]
        let (program, shim) = (dir.join("claude"), "#!/bin/sh\nexec node \"$(dirname \"$0\")/fake.cjs\" \"$@\"\n");
        std::fs::write(&program, shim).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Cli { dir, program }
    }

    /// What the next starts of the CLI play, one list of steps each.
    fn script(&self, runs: Vec<Vec<Value>>) {
        std::fs::write(self.dir.join("script.json"), json!(runs).to_string()).unwrap();
    }

    fn runs(&self) -> Vec<Run> {
        let text = std::fs::read_to_string(self.dir.join("log.jsonl")).unwrap_or_default();
        let mut out: Vec<Run> = vec![];
        for v in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            if let Some(a) = v["argv"].as_array() {
                let args = a.iter().map(|x| x.as_str().unwrap_or_default().to_string()).collect();
                out.push(Run { args, pid: v["pid"].as_u64().unwrap_or_default() as u32, input: vec![], exit: None });
            } else if let Some(r) = out.last_mut() {
                if v["in"].is_null() {
                    r.exit = v["exit"].as_i64();
                } else {
                    r.input.push(v["in"].clone());
                }
            }
        }
        out
    }
}

fn out(line: Value) -> Value {
    json!(["out", line])
}

fn read(pattern: Value) -> Value {
    json!(["read", pattern])
}

/// The CLI reads the person's message before it answers.
fn read_user() -> Value {
    read(json!({"type": "user"}))
}

fn text(t: &str) -> Value {
    json!([{"type": "text", "text": t}])
}

// ------------------------------------------------------------- stream-json lines

fn init(sid: &str) -> Value {
    out(json!({"type": "system", "subtype": "init", "session_id": sid, "model": "claude-opus-5-5", "tools": [], "mcp_servers": []}))
}

fn system(sid: &str, mut ev: Value) -> Value {
    ev["type"] = json!("system");
    ev["session_id"] = json!(sid);
    out(ev)
}

fn stream(sid: &str, event: Value) -> Value {
    out(json!({"type": "stream_event", "event": event, "parent_tool_use_id": null, "session_id": sid, "uuid": new_id()}))
}

/// A message streaming in: its start and a text block of `pieces`, finished (`done`) or not.
fn streamed_text(sid: &str, msg_id: &str, pieces: &[&str], done: bool) -> Vec<Value> {
    let start = json!({"type": "message_start", "message": {"id": msg_id, "type": "message", "role": "assistant", "model": "claude-opus-5-5", "content": [], "usage": {"input_tokens": 10, "output_tokens": 1}}});
    let mut v = vec![stream(sid, start), stream(sid, json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}))];
    for p in pieces {
        v.push(stream(sid, json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": p}})));
    }
    if done {
        v.push(stream(sid, json!({"type": "content_block_stop", "index": 0})));
    }
    v
}

/// A finished message (the CLI writes one as each of its blocks completes).
fn assistant(sid: &str, msg_id: &str, uuid: &str, content: Value) -> Value {
    out(json!({"type": "assistant", "message": {"id": msg_id, "type": "message", "role": "assistant", "model": "claude-opus-5-5", "content": content,
        "usage": {"input_tokens": 10, "output_tokens": 5}}, "parent_tool_use_id": null, "session_id": sid, "uuid": uuid}))
}

fn tool_result(sid: &str, uuid: &str, tool_use_id: &str, result: &str) -> Value {
    out(json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_use_id, "content": result}]},
        "parent_tool_use_id": null, "session_id": sid, "uuid": uuid}))
}

/// The end of a turn, with the session's running cost.
fn result(sid: &str, total_cost: f64) -> Value {
    out(json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 1234, "num_turns": 1, "result": "", "session_id": sid,
        "total_cost_usd": total_cost, "usage": {"input_tokens": 10, "output_tokens": 5}}))
}

// ------------------------------------------------------------- the app

type Events = Arc<Mutex<Vec<(String, Value)>>>;

const EVENTS: [&str; 6] = ["turn", "permission-request", "permission-resolved", "usage-limits-changed", "cc-waiting", "workflows-changed"];

/// Folders removed when dropped.
struct TempDirs(Vec<PathBuf>);

impl Drop for TempDirs {
    fn drop(&mut self) {
        for d in &self.0 {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// Claude Code's own data for these tests (`CLAUDE_CONFIG_DIR`): where the app looks for transcripts.
fn config_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli-turn-claude")
}

struct Harness {
    app: tauri::App<MockRuntime>,
    st: Arc<AppState>,
    events: Events,
    cli: Cli,
    conv: String,
    /// this test's transcripts
    projects: PathBuf,
    // last: removed once the database in `st` is closed
    _dirs: TempDirs,
}

impl Harness {
    /// A conversation of `kind` ("chat" or "code", in a folder of its own) on the stand-in CLI.
    fn new(kind: &str) -> Harness {
        static ENV: Once = Once::new();
        ENV.call_once(|| {
            // never the person's ~/.claude
            std::env::set_var("CLAUDE_CONFIG_DIR", config_dir());
            // a fake key: the app's keyring is never read
            std::env::set_var("CLAUDERUST_API_KEY", "test-key");
        });
        let id = new_id();
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-turn-{id}"));
        let projects = config_dir().join("projects").join(&id);
        std::fs::create_dir_all(dir.join("project")).unwrap();
        let st = Arc::new(AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
        let cli = Cli::new(dir.join("cli"));
        {
            let mut s = st.settings.write();
            s.claude_code_path = cli.program.display().to_string();
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
        let cwd = (kind == "code").then(|| dir.join("project").display().to_string());
        let conv = st.db.conv_create(kind, "claude-opus-5-5", None, cwd.as_deref(), &json!({"engine": "claude-code"})).unwrap();
        st.db.conv_set_title(&conv.id, "Test").unwrap();
        Harness { app, st, events, cli, conv: conv.id, projects: projects.clone(), _dirs: TempDirs(vec![dir, projects]) }
    }

    /// Store `text` as the person's message under `parent` (the conversation's leaf when None).
    fn say(&self, text_: &str, parent: Option<&str>) -> String {
        let parent = parent.map(String::from).or_else(|| self.st.db.conv_get(&self.conv).unwrap().current_leaf);
        let m = Message { id: new_id(), conversation_id: self.conv.clone(), parent_id: parent, role: "user".into(), content: text(text_), meta: json!({}), created_at: 1 };
        self.st.db.msg_insert(&m).unwrap();
        self.st.db.conv_set_leaf(&self.conv, Some(&m.id)).unwrap();
        m.id
    }

    /// Start the turn for the message `id`, registered as running so that `engine::stop` stops it.
    fn start(&self, id: &str) -> tauri::async_runtime::JoinHandle<AppResult<()>> {
        let cancel = CancellationToken::new();
        self.st.runs.lock().insert(self.conv.clone(), cancel.clone());
        let (app, st, conv, id) = (self.app.handle().clone(), self.st.clone(), self.conv.clone(), id.to_string());
        tauri::async_runtime::spawn(async move {
            let r = claudecode::run(&app, &st, &conv, id, &cancel).await;
            st.runs.lock().remove(&conv);
            r
        })
    }

    /// Say `text_` and run its turn to the end.
    async fn turn(&self, text_: &str, parent: Option<&str>) -> Result<(), String> {
        let id = self.say(text_, parent);
        finish(self.start(&id)).await
    }

    /// The transcript the CLI keeps of session `sid`, with its running cost (all the app reads of it).
    fn transcript(&self, sid: &str, total_cost: f64) {
        std::fs::create_dir_all(&self.projects).unwrap();
        std::fs::write(self.projects.join(format!("{sid}.jsonl")), format!("{}\n", json!({"type": "cost-state", "totalCostUSD": total_cost}))).unwrap();
    }

    /// The payloads of this conversation's `name` events.
    fn events(&self, name: &str) -> Vec<Value> {
        let ev = self.events.lock().unwrap();
        ev.iter().filter(|(n, p)| n == name && p["conversation_id"] == self.conv.as_str()).map(|(_, p)| p.clone()).collect()
    }

    fn path(&self) -> Vec<Message> {
        let leaf = self.st.db.conv_get(&self.conv).unwrap().current_leaf.unwrap();
        self.st.db.path_to(&leaf).unwrap()
    }

    fn texts(&self) -> Vec<Value> {
        self.path().into_iter().map(|m| m.content).collect()
    }
}

/// The turn's end, or a failed test when it takes over 30 s.
async fn finish(turn: tauri::async_runtime::JoinHandle<AppResult<()>>) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(30), turn).await.expect("timed out waiting for the turn to end").expect("the turn panicked").map_err(|e| e.to_string())
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

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-9)
}

/// Is process `pid` still running?
fn alive(pid: u32) -> bool {
    #[cfg(windows)]
    let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
    #[cfg(not(windows))]
    let out = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "pid="]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).split_whitespace().any(|w| w == pid.to_string())
}

// ------------------------------------------------------------- tests

#[test]
fn a_chat_turn_streams_its_reply_and_stores_it_with_its_cost() {
    block_on(async {
        let h = Harness::new("chat");
        let sid = new_id();
        let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "recent_chats", "arguments": {}}});
        let mcp = |id: &str, message: &Value| out(json!({"type": "control_request", "request_id": id, "request": {"subtype": "mcp_message", "server_name": "past_chats", "message": message}}));
        let mut steps = vec![
            read_user(),
            init(&sid),
            // the past-chats tools the app hosts for the CLI: their list, and a call, both answered by the app
            mcp("mcp-1", &list),
            read(json!({"type": "control_response", "response": {"request_id": "mcp-1"}})),
            mcp("mcp-2", &call),
            read(json!({"type": "control_response", "response": {"request_id": "mcp-2"}})),
        ];
        steps.extend(streamed_text(&sid, "msg_1", &["Hello", " world"], true));
        steps.extend([assistant(&sid, "msg_1", "uuid-1", text("Hello world")), stream(&sid, json!({"type": "message_stop"})), result(&sid, 0.0123)]);
        h.cli.script(vec![steps]);
        let user = h.say("Hi", None);
        finish(h.start(&user)).await.unwrap();

        let path = h.path();
        assert_eq!(path.len(), 2);
        let a = &path[1];
        assert_eq!((a.role.as_str(), a.parent_id.as_deref()), ("assistant", Some(user.as_str())));
        assert_eq!(a.content, text("Hello world"));
        assert_eq!((a.meta["engine"].as_str(), a.meta["cc_session"].as_str(), a.meta["cc_uuid"].as_str()), (Some("claude-code"), Some(sid.as_str()), Some("uuid-1")));
        assert_eq!((a.meta["cost_usd"].as_f64(), a.meta["cc_cost_total"].as_f64(), a.meta["duration_ms"].as_u64()), (Some(0.0123), Some(0.0123), Some(1234)));
        assert_eq!(a.meta["stop_reason"], "end_turn");
        let conv = h.st.db.conv_get(&h.conv).unwrap();
        assert_eq!((conv.options["cc_session"].as_str(), conv.options["cc_tip"].as_str()), (Some(sid.as_str()), Some(a.id.as_str())));
        assert_eq!(h.st.db.search("world", 5).unwrap().len(), 1, "indexed for search");
        assert!(h.events("turn").len() >= 2, "streamed to the window");

        // a chat runs the CLI on its stream-json protocol, with our prompt and none of Claude Code's own tools
        let run = &h.cli.runs()[0];
        for words in [&["-p"][..], &["--input-format", "stream-json"], &["--output-format", "stream-json"], &["--include-partial-messages"], &["--permission-prompt-tool=stdio"], &["--model", "claude-opus-5-5"], &["--strict-mcp-config"], &["--disable-slash-commands"]] {
            assert!(run.has(words), "{words:?} in {:?}", run.args);
        }
        assert!(!run.has(&["--resume"]));
        let prompt = &run.args[run.args.iter().position(|a| a == "--system-prompt-file").expect("a prompt file") + 1];
        assert!(!Path::new(prompt).exists(), "the prompt file is removed after the turn");
        assert_eq!((&run.input[0]["request"]["subtype"], &run.input[0]["request"]["sdkMcpServers"]), (&json!("initialize"), &json!(["past_chats"])));
        assert_eq!(run.first("user")["message"]["content"], text("Hi"));
        assert!(run.first("user")["origin"].is_null(), "only a code session's message is marked as typed (keywords such as ultracode)");
        let listed = run.answer("mcp-1")["response"]["mcp_response"]["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).map(String::from).collect::<Vec<_>>();
        assert!(listed.contains(&"recent_chats".to_string()), "{listed:?}");
        assert_eq!(run.answer("mcp-2")["response"]["mcp_response"]["result"], json!({"content": [{"type": "text", "text": "No previous chats found."}], "isError": false}));
        assert_eq!(run.exit, Some(0), "it ended by itself once its input closed");
    });
}

/// A code session's turn whose Bash call the CLI asks permission for, answered `decision`: the answer the
/// CLI got, the conversation afterwards and its options.
fn permission_round_trip(decision: &str, message: Option<&str>) -> (Value, Vec<Message>, Value) {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        let call = json!({"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "echo hi"}});
        h.cli.script(vec![vec![
            read_user(),
            init(&sid),
            assistant(&sid, "msg_1", "uuid-1", json!([{"type": "text", "text": "Let me check."}, call])),
            out(json!({"type": "control_request", "request_id": "perm-1", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "echo hi"}, "tool_use_id": "toolu_1"}})),
            read(json!({"type": "control_response", "response": {"request_id": "perm-1"}})),
            tool_result(&sid, "uuid-2", "toolu_1", "hi"),
            assistant(&sid, "msg_2", "uuid-3", text("Done.")),
            result(&sid, 0.02),
        ]]);
        let user = h.say("Run echo hi", None);
        let turn = h.start(&user);
        let ask = until("the permission prompt", || h.events("permission-request").pop()).await;
        assert_eq!((ask["kind"].as_str(), ask["tool"].as_str(), ask["tool_use_id"].as_str()), (Some("code"), Some("Bash"), Some("toolu_1")));
        assert_eq!(ask["input"], json!({"command": "echo hi"}));
        let req = ask["request_id"].as_str().unwrap().to_string();
        assert!(h.st.pending_perms.lock().contains_key(&req), "shown to a view opened now");
        // what the permission_respond command does
        let tx = h.st.permissions.lock().remove(&req).expect("waiting for the answer");
        tx.send(PermissionReply { decision: decision.into(), message: message.map(Into::into) }).unwrap();
        finish(turn).await.unwrap();

        assert!(h.st.pending_perms.lock().is_empty(), "the card is taken down");
        assert_eq!(h.events("permission-resolved").iter().map(|e| e["request_id"].clone()).collect::<Vec<_>>(), vec![json!(req)]);
        let run = &h.cli.runs()[0];
        assert_eq!(run.first("user")["origin"], json!({"kind": "human"}), "a code session's message is the person's own");
        let answer = run.answer("perm-1");
        assert_eq!(answer["subtype"], "success");
        (answer["response"].clone(), h.path(), h.st.db.conv_get(&h.conv).unwrap().options)
    })
}

#[test]
fn an_allowed_tool_call_is_answered_and_the_turn_goes_on() {
    let (answer, path, options) = permission_round_trip("session", None);
    assert_eq!((answer["behavior"].as_str(), answer["toolUseID"].as_str()), (Some("allow"), Some("toolu_1")));
    assert_eq!(answer["updatedInput"], json!({"command": "echo hi"}));
    // allowed for the session: kept in this conversation (later turns pass it as --allowedTools), never in the project's settings
    assert_eq!(answer["updatedPermissions"], json!([{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "echo hi"}], "behavior": "allow", "destination": "session"}]));
    assert_eq!(options["cc_allowed_tools"], json!(["Bash(echo hi)"]));
    let roles: Vec<&str> = path.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
    for w in path.windows(2) {
        assert_eq!(w[1].parent_id.as_ref(), Some(&w[0].id));
    }
    assert_eq!(path[1].meta["stop_reason"], "tool_use");
    assert_eq!((&path[2].meta["tool_results"], &path[2].content[0]["content"]), (&json!(true), &json!("hi")));
    assert_eq!((&path[3].content, &path[3].meta["stop_reason"]), (&text("Done."), &json!("end_turn")));
}

#[test]
fn a_denied_tool_call_tells_the_cli_why() {
    let (answer, path, options) = permission_round_trip("deny", Some("not now"));
    assert_eq!(answer, json!({"behavior": "deny", "message": "The user denied this tool call. The user said: not now", "interrupt": false, "toolUseID": "toolu_1"}));
    assert!(options["cc_allowed_tools"].is_null(), "nothing allowed");
    assert_eq!(path.len(), 4, "the CLI goes on after it");
}

#[test]
fn the_next_message_resumes_the_session_and_one_further_back_forks_it() {
    block_on(async {
        let h = Harness::new("code");
        let (sid, fork) = (new_id(), new_id());
        let answer = |sid: &str, n: &str, cost: f64| vec![read_user(), init(sid), assistant(sid, &format!("msg_{n}"), &format!("uuid-{n}"), text(&format!("Answer {n}"))), result(sid, cost)];
        h.cli.script(vec![answer(&sid, "1", 0.01), answer(&sid, "2", 0.03), answer(&fork, "3", 0.05)]);
        h.turn("First", None).await.unwrap();
        let first = h.path()[1].clone();
        h.transcript(&sid, 0.01);
        h.turn("Second", None).await.unwrap();
        let second = h.path()[3].clone();
        h.transcript(&sid, 0.03);
        // an edit of the second message: a branch from the first answer, which isn't the session's tip
        h.turn("Second, again", Some(&first.id)).await.unwrap();

        let runs = h.cli.runs();
        assert_eq!(runs.len(), 3);
        assert!(!runs[0].has(&["--resume"]));
        assert!(runs[1].has(&["--resume", &sid]) && !runs[1].has(&["--fork-session"]), "{:?}", runs[1].args);
        assert!(runs[2].has(&["--resume", &sid, "--resume-session-at", "uuid-1", "--fork-session"]), "{:?}", runs[2].args);
        // a resumed session holds the conversation: only the new message goes in
        for (r, said) in runs[1..].iter().zip(["Second", "Second, again"]) {
            assert_eq!(r.first("user")["message"]["content"], text(said));
        }
        assert_eq!(h.texts(), vec![text("First"), text("Answer 1"), text("Second, again"), text("Answer 3")]);
        // each answer's own cost from the session's running total: at the tip the stored one, at a fork the transcript's
        assert!(close(second.meta["cost_usd"].as_f64(), 0.02), "{}", second.meta);
        assert!(close(h.path()[3].meta["cost_usd"].as_f64(), 0.02), "{}", h.path()[3].meta);
        let conv = h.st.db.conv_get(&h.conv).unwrap();
        assert_eq!((conv.options["cc_session"].as_str(), conv.options["cc_tip"].as_str()), (Some(fork.as_str()), Some(h.path()[3].id.as_str())));
    });
}

#[test]
fn a_cli_that_dies_mid_reply_leaves_the_reply_so_far_with_its_error() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        let mut steps = vec![read_user(), init(&sid)];
        steps.extend(streamed_text(&sid, "msg_1", &["Partial ", "answer"], false));
        steps.extend([json!(["err", "API Error: Connection error."]), json!(["exit", 1])]);
        h.cli.script(vec![steps]);
        let err = h.turn("Hi", None).await.unwrap_err();
        assert!(err.starts_with("Claude Code exited unexpectedly") && err.contains("API Error: Connection error."), "{err}");
        let path = h.path();
        assert_eq!(path.len(), 2);
        let a = &path[1];
        assert_eq!(a.content, text("Partial answer"), "what had streamed in is kept");
        assert_eq!(a.meta["error"], json!({"message": err, "kind": "claude_code"}));
        assert!(a.meta["stop_reason"].is_null() && a.meta["stopped"].is_null(), "unfinished, not stopped: {}", a.meta);
        assert_eq!(h.cli.runs()[0].exit, Some(1));
    });
}

#[test]
fn a_cut_off_reply_goes_on_in_its_session() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        // its first block finished (the CLI's transcript has it); the second had only streamed in when the CLI died
        let mut steps = vec![read_user(), init(&sid)];
        steps.extend(streamed_text(&sid, "msg_1", &["Part one."], true));
        steps.extend([
            assistant(&sid, "msg_1", "uuid-1", text("Part one.")),
            stream(&sid, json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})),
            stream(&sid, json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Part two, cut"}})),
            json!(["exit", 1]),
        ]);
        let rest = vec![read_user(), init(&sid), assistant(&sid, "msg_2", "uuid-2", text("off here.")), result(&sid, 0.02)];
        h.cli.script(vec![steps, rest]);
        assert!(h.turn("Write two parts", None).await.is_err());
        let reply = h.path()[1].clone();
        assert_eq!(reply.content, json!([{"type": "text", "text": "Part one."}, {"type": "text", "text": "Part two, cut"}]));
        h.transcript(&sid, 0.01);
        // what send_message's continue_reply starts: a turn from the reply itself
        finish(h.start(&reply.id)).await.unwrap();

        let run = &h.cli.runs()[1];
        assert!(run.has(&["--resume", &sid]) && !run.has(&["--fork-session"]), "{:?}", run.args);
        let asked = run.first("user");
        assert!(asked["origin"].is_null(), "the app's request, not words the person typed");
        let ask = asked["message"]["content"][0]["text"].as_str().unwrap();
        assert!(ask.starts_with("Your previous reply was cut off.") && ask.ends_with("<reply_end>\nPart one.\nPart two, cut\n</reply_end>"), "{ask}");
        let path = h.path();
        assert_eq!(path.len(), 3, "no message of the person's added");
        let (a, b) = (&path[1], &path[2]);
        assert_eq!((b.parent_id.as_deref(), &b.content), (Some(a.id.as_str()), &text("off here.")));
        assert!(a.meta["error"].is_null() && a.meta["stopped"].is_null(), "no longer cut off: {}", a.meta);
        assert_eq!((a.meta["stop_reason"].as_str(), b.meta["stop_reason"].as_str()), (Some("end_turn"), Some("end_turn")));
        assert!(close(b.meta["cost_usd"].as_f64(), 0.01), "the cost from the transcript's total: {}", b.meta);
    });
}

#[test]
fn a_rate_limit_event_updates_the_plan_usage() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        // 2100-01-01, in seconds as the CLI reports it
        let resets = 4_102_444_800i64;
        let info = json!({"status": "allowed_warning", "rateLimitType": "five_hour", "resetsAt": resets, "utilization": 0.91, "surpassedThreshold": 0.9});
        h.cli.script(vec![vec![
            read_user(),
            init(&sid),
            out(json!({"type": "rate_limit_event", "rate_limit_info": info, "session_id": sid, "uuid": new_id()})),
            assistant(&sid, "msg_1", "uuid-1", text("Hi")),
            result(&sid, 0.01),
        ]]);
        h.turn("Hi", None).await.unwrap();
        let limits = h.st.usage_limits.lock().clone();
        assert_eq!(
            (limits["status"].as_str(), limits["limiting"].as_str(), limits["near"].as_bool(), limits["exceeded"].as_bool()),
            (Some("allowed_warning"), Some("five_hour"), Some(true), Some(false))
        );
        assert_eq!(limits["limits"], json!([{"key": "five_hour", "utilization": 0.91, "resets_at": resets * 1000, "status": "allowed_warning", "threshold": 0.9}]));
        let told: Vec<Value> = h.events.lock().unwrap().iter().filter(|(n, _)| n == "usage-limits-changed").map(|(_, p)| p.clone()).collect();
        assert_eq!(told, vec![limits], "the window is told");
    });
}

#[test]
fn stop_interrupts_the_cli_and_keeps_the_reply_so_far() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        let mut steps = vec![read_user(), init(&sid)];
        steps.extend(streamed_text(&sid, "msg_1", &["Partial ", "answer"], false));
        // the CLI on an interrupt: the request is aborted and the turn ends with its result
        steps.push(read(json!({"type": "control_request", "request": {"subtype": "interrupt"}})));
        steps.push(out(json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "duration_ms": 900, "session_id": sid,
            "total_cost_usd": 0.004, "usage": {"input_tokens": 10, "output_tokens": 2}})));
        h.cli.script(vec![steps]);
        let user = h.say("Hi", None);
        let turn = h.start(&user);
        until("the reply to start", || h.st.live.lock().contains_key(&h.conv).then_some(())).await;
        engine::stop(&h.st, &h.conv);
        finish(turn).await.expect("Stop is not an error");

        let a = &h.path()[1];
        assert_eq!(a.content, text("Partial answer"));
        assert_eq!((a.meta["stopped"].as_bool(), a.meta["cost_usd"].as_f64()), (Some(true), Some(0.004)));
        assert!(a.meta["stop_reason"].is_null() && a.meta["error"].is_null(), "{}", a.meta);
        let run = &h.cli.runs()[0];
        assert_eq!(run.input.iter().filter(|l| l["request"]["subtype"] == "interrupt").count(), 1);
        assert_eq!(run.exit, Some(0), "it ended by itself once its input closed");
        assert!(!h.st.cc_ctl.lock().contains_key(&h.conv), "the line to the CLI is gone");
    });
}

#[test]
fn a_cli_that_stays_after_its_result_is_stopped() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        h.cli.script(vec![vec![read_user(), init(&sid), assistant(&sid, "msg_1", "uuid-1", text("Done.")), result(&sid, 0.01), json!(["stay"])]]);
        h.turn("Hi", None).await.unwrap();
        assert_eq!(h.path()[1].content, text("Done."));
        let run = &h.cli.runs()[0];
        assert_eq!(run.exit, None, "it didn't end by itself");
        assert!(run.pid > 0 && !alive(run.pid), "process {} is still running", run.pid);
    });
}

#[test]
fn a_session_the_cli_lost_starts_over_with_the_conversation_as_context() {
    block_on(async {
        let h = Harness::new("code");
        let (old, new) = (new_id(), new_id());
        h.cli.script(vec![vec![read_user(), init(&old), assistant(&old, "msg_1", "uuid-1", text("First answer")), result(&old, 0.01)]]);
        h.turn("First", None).await.unwrap();
        h.transcript(&old, 0.01);
        // the CLI cleaned up its transcript meanwhile: it says so and exits
        h.cli.script(vec![
            vec![json!(["err", format!("No conversation found with session ID: {old}")]), json!(["exit", 1])],
            vec![read_user(), init(&new), assistant(&new, "msg_2", "uuid-2", text("Second answer")), result(&new, 0.02)],
        ]);
        h.turn("Second", None).await.unwrap();

        let runs = h.cli.runs();
        assert_eq!(runs.len(), 3);
        assert!(runs[1].has(&["--resume", &old]), "{:?}", runs[1].args);
        assert!(!runs[2].has(&["--resume"]), "{:?}", runs[2].args);
        let content = &runs[2].first("user")["message"]["content"];
        let context = content[0]["text"].as_str().unwrap();
        assert!(context.starts_with("<earlier_conversation>\nUser: First\n\nAssistant: First answer\n</earlier_conversation>"), "{context}");
        assert_eq!(content[1], json!({"type": "text", "text": "Second"}));
        // nothing of the attempt that failed is stored
        assert_eq!(h.texts(), vec![text("First"), text("First answer"), text("Second"), text("Second answer")]);
        assert_eq!(h.st.db.conv_get(&h.conv).unwrap().options["cc_session"], json!(new));
    });
}

#[test]
fn a_workflow_keeps_the_cli_open_and_its_stop_reaches_the_cli() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        let report = json!({"type": "message_start", "message": {"id": "msg_2", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "content": [], "usage": {}}});
        h.cli.script(vec![vec![
            read_user(),
            init(&sid),
            assistant(&sid, "msg_1", "uuid-1", text("Starting a review.")),
            system(&sid, json!({"subtype": "task_started", "task_id": "wf1", "task_type": "local_workflow", "workflow_name": "review", "description": "Review the change"})),
            result(&sid, 0.01),
            // the turn is over, the workflow isn't: the CLI stays, and takes the workflow's Stop
            read(json!({"type": "control_request", "request": {"subtype": "stop_task", "task_id": "wf1"}})),
            system(&sid, json!({"subtype": "task_notification", "task_id": "wf1", "status": "killed", "output_file": "wf1.md"})),
            // its report, as a turn of its own
            stream(&sid, report),
            assistant(&sid, "msg_2", "uuid-2", text("The review was stopped.")),
            result(&sid, 0.015),
        ]]);
        let user = h.say("Review this", None);
        let turn = h.start(&user);
        let waiting = until("the turn to wait on its workflow", || h.events("cc-waiting").first().cloned()).await;
        assert_eq!(waiting["waiting"], true);
        let wf = h.st.workflows.lock()[&h.conv].clone();
        assert_eq!((wf[0]["name"].as_str(), wf[0]["status"].as_str()), (Some("review"), Some("running")));
        assert!(h.st.cc_ctl.lock()[&h.conv].waiting, "a message now goes straight to the CLI");
        // the Tasks pane's Stop for the workflow
        h.st.cc_ctl.lock()[&h.conv].tx.send(Cmd::StopTask("wf1".into())).unwrap();
        finish(turn).await.unwrap();

        let wf = h.st.workflows.lock()[&h.conv].clone();
        assert_eq!((wf[0]["status"].as_str(), wf[0]["output_file"].as_str()), (Some("killed"), Some("wf1.md")));
        assert!(!h.events("workflows-changed").is_empty());
        assert_eq!(h.events("cc-waiting").iter().map(|e| e["waiting"].clone()).collect::<Vec<_>>(), vec![json!(true), json!(false)]);
        let path = h.path();
        assert_eq!(h.texts(), vec![text("Review this"), text("Starting a review."), text("The review was stopped.")]);
        assert_eq!(path[1].meta["stop_reason"], "end_turn");
        assert_eq!(path[2].meta["cost_usd"].as_f64(), Some(0.015));
        assert!(path[2].meta["duration_ms"].is_u64(), "the wall time of a turn that waited on a workflow");
        assert_eq!(h.cli.runs()[0].exit, Some(0));
        assert!(!h.st.cc_ctl.lock().contains_key(&h.conv));
    });
}

// ------------------------------------------------------------- what the API engine shows too

fn artifact(id: &str, body: &str, update: bool) -> String {
    format!("<antArtifact identifier=\"{id}\" type=\"text/html\" title=\"Page\"{}>{body}</antArtifact>", if update { " command=\"update\"" } else { "" })
}

/// The Artifacts page's rows of the test's conversation: (identifier, the latest version's content).
fn gallery(h: &Harness) -> Vec<(String, String)> {
    let mut g: Vec<_> = h.st.db.artifact_list(100).unwrap().into_iter().filter(|a| a.conversation_id == h.conv).map(|a| (a.identifier, a.content)).collect();
    g.sort();
    g
}

/// The turn the window shows for these messages.
fn rendered(msgs: &[Message]) -> Value {
    let (live, streaming, names) = (HashMap::new(), HashSet::new(), HashMap::new());
    let ctx = render::RenderCtx { math: false, live_tools: &live, streaming_tools: &streaming, mcp_names: &names, streaming_msg: None };
    render::assistant_turn(&msgs.iter().collect::<Vec<_>>(), &ctx, &mut render::ArtifactTracker::default())
}

#[test]
fn a_turns_artifacts_go_to_the_gallery_and_leave_with_the_chat() {
    block_on(async {
        let h = Harness::new("chat");
        let sid = new_id();
        let search = json!({"type": "tool_use", "id": "toolu_1", "name": "WebSearch", "input": {"query": "q"}});
        // an artifact before a tool call and its update in the answer after it; then a resumed turn makes another
        let first = vec![
            read_user(),
            init(&sid),
            assistant(&sid, "msg_1", "uuid-1", json!([{"type": "text", "text": format!("Here: {}", artifact("page", "<p>v1</p>", false))}, search])),
            tool_result(&sid, "uuid-2", "toolu_1", "No results."),
            assistant(&sid, "msg_2", "uuid-3", text(&format!("Better: {}", artifact("page", "<old_str>v1</old_str><new_str>v2</new_str>", true)))),
            result(&sid, 0.01),
        ];
        let second = vec![read_user(), init(&sid), assistant(&sid, "msg_3", "uuid-4", text(&artifact("notes", "<p>n</p>", false))), result(&sid, 0.02)];
        h.cli.script(vec![first, second]);
        h.turn("Make a page", None).await.unwrap();
        assert_eq!(gallery(&h), [("page".to_string(), "<p>v2</p>".to_string())], "the update applied to the version before it");
        assert_eq!(h.st.db.search("Here", 5).unwrap().len(), 1, "every answer of the turn is searched, not only its last");
        h.transcript(&sid, 0.01);
        h.turn("And notes", None).await.unwrap();
        assert!(h.cli.runs()[1].has(&["--resume", &sid]));
        assert_eq!(gallery(&h).iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["notes", "page"]);
        h.st.db.conv_delete(&h.conv).unwrap();
        assert!(gallery(&h).is_empty(), "the chat's artifacts go with it");
    });
}

#[test]
fn a_continued_reply_keeps_its_artifact() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        let cut = vec![read_user(), init(&sid), assistant(&sid, "msg_1", "uuid-1", text("Part one.")), json!(["exit", 1])];
        let rest = vec![read_user(), init(&sid), assistant(&sid, "msg_2", "uuid-2", text(&artifact("page", "<p>done</p>", false))), result(&sid, 0.02)];
        h.cli.script(vec![cut, rest]);
        assert!(h.turn("Write a page", None).await.is_err());
        let reply = h.path()[1].clone();
        h.transcript(&sid, 0.01);
        // what send_message's continue_reply starts: a turn from the reply itself
        finish(h.start(&reply.id)).await.unwrap();
        assert_eq!(gallery(&h), [("page".to_string(), "<p>done</p>".to_string())]);
    });
}

#[test]
fn thinking_comes_summarized_with_how_long_it_took() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        let start = json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "content": [], "usage": {"input_tokens": 10, "output_tokens": 1}}});
        let thought = "Plan: read the file first.";
        h.cli.script(vec![vec![
            read_user(),
            init(&sid),
            stream(&sid, start),
            stream(&sid, json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}})),
            stream(&sid, json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": thought}})),
            json!(["sleep", 150]),
            stream(&sid, json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}})),
            stream(&sid, json!({"type": "content_block_stop", "index": 0})),
            assistant(&sid, "msg_1", "uuid-1", json!([{"type": "thinking", "thinking": thought, "signature": "sig"}])),
            stream(&sid, json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})),
            stream(&sid, json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Done."}})),
            json!(["sleep", 500]),
            stream(&sid, json!({"type": "content_block_stop", "index": 1})),
            assistant(&sid, "msg_1", "uuid-2", text("Done.")),
            stream(&sid, json!({"type": "message_stop"})),
            result(&sid, 0.01),
        ]]);
        h.turn("Hi", None).await.unwrap();
        let run = &h.cli.runs()[0];
        assert!(run.has(&["--thinking-display", "summarized"]), "{:?}", run.args);
        let a = &h.path()[1];
        assert_eq!((&a.content[0]["thinking"], &a.content[1]["text"]), (&json!(thought), &json!("Done.")));
        let ms = a.content[0]["thinking_ms"].as_u64().expect("its time is stored");
        // from its start to its stop (about 150 ms), not the reply's end (650 ms or more); the lower bound is
        // engine.rs's test (a reader the runner delays could see the start late)
        assert!(ms < 640, "from its start to its stop, not the reply's end: {ms}");
        assert!(a.content[1].get("thinking_ms").is_none());
        let part = &rendered(&h.path()[1..])["parts"][0];
        assert_eq!((&part["kind"], &part["raw"], &part["thinking_ms"]), (&json!("thinking"), &json!(thought), &json!(ms)));
    });
}

#[test]
fn a_cli_without_thinking_display_runs_without_it() {
    block_on(async {
        let h = Harness::new("chat");
        let sid = new_id();
        h.cli.script(vec![
            vec![json!(["err", "error: unknown option '--thinking-display'"]), json!(["exit", 1])],
            vec![read_user(), init(&sid), assistant(&sid, "msg_1", "uuid-1", text("Hello")), result(&sid, 0.01)],
            vec![read_user(), init(&sid), assistant(&sid, "msg_2", "uuid-2", text("Again")), result(&sid, 0.01)],
        ]);
        h.turn("Hi", None).await.unwrap();
        h.turn("More", None).await.unwrap();
        let runs = h.cli.runs();
        assert!(runs[0].has(&["--thinking-display", "summarized"]));
        for r in &runs[1..] {
            assert!(!r.args.iter().any(|a| a == "--thinking-display"), "{:?}", r.args);
        }
        assert_eq!(h.texts(), vec![text("Hi"), text("Hello"), text("More"), text("Again")], "nothing of the refused start is stored");
    });
}

#[test]
fn a_web_search_shows_its_links_as_sources_and_the_answer_cites_them() {
    block_on(async {
        let h = Harness::new("chat");
        let sid = new_id();
        let url = "https://blog.rust-lang.org/2024/07/25/Rust-1.80.0.html";
        let links = json!([{"title": "Announcing Rust 1.80.0", "url": url}]);
        let output = format!("Web search results for query: \"rust 1.80\"\n\nLinks: {links}\n\nREMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks.");
        h.cli.script(vec![vec![
            read_user(),
            init(&sid),
            assistant(&sid, "msg_1", "uuid-1", json!([{"type": "tool_use", "id": "toolu_1", "name": "WebSearch", "input": {"query": "rust 1.80"}}])),
            tool_result(&sid, "uuid-2", "toolu_1", &output),
            assistant(&sid, "msg_2", "uuid-3", text(&format!("It shipped [in July]({url})."))),
            result(&sid, 0.01),
        ]]);
        h.turn("When did Rust 1.80 ship?", None).await.unwrap();
        let t = rendered(&h.path()[1..]);
        let search = &t["parts"][0];
        assert_eq!((&search["kind"], &search["query"], &search["status"]), (&json!("search"), &json!("rust 1.80"), &json!("done")));
        assert_eq!(search["results"], json!([{"title": "Announcing Rust 1.80.0", "url": url, "domain": "blog.rust-lang.org", "page_age": null}]));
        let answer = &t["parts"][1];
        let html = answer["html"].as_str().unwrap();
        assert!(html.contains(&format!("in July<a class=\"cite\" href=\"{url}\" title=\"Announcing Rust 1.80.0\">blog.rust-lang.org</a>")), "{html}");
        assert_eq!(answer["sources"], json!([{"url": url, "title": "Announcing Rust 1.80.0", "domain": "blog.rust-lang.org"}]));
    });
}

#[test]
fn switched_off_tools_are_denied_to_the_cli() {
    block_on(async {
        let h = Harness::new("code");
        let sid = new_id();
        let mut options = h.st.db.conv_get(&h.conv).unwrap().options;
        options["disabled_tools"] = json!(["github__create_issue", "weather__get_forecast"]);
        options["disabled_servers"] = json!(["my notes"]);
        h.st.db.conv_set_options(&h.conv, &options).unwrap();
        h.cli.script(vec![vec![read_user(), init(&sid), assistant(&sid, "msg_1", "uuid-1", text("Hi")), result(&sid, 0.01)]]);
        h.turn("Hi", None).await.unwrap();
        let run = &h.cli.runs()[0];
        // a whole server is its tools' prefix, in the characters the CLI keeps in a name
        assert!(run.has(&["--disallowedTools", "mcp__my_notes", "mcp__github__create_issue", "mcp__weather__get_forecast"]), "{:?}", run.args);
    });
}

#[test]
fn a_title_comes_without_the_models_label() {
    block_on(async {
        let h = Harness::new("code");
        h.cli.script(vec![vec![out(json!({"type": "result", "subtype": "success", "is_error": false, "result": "**Title:** Replace TODO comment in calc.py."}))]]);
        let conv = h.st.db.conv_get(&h.conv).unwrap();
        let title = claudecode::title(&h.st, &conv, "Replace the TODO in calc.py", "Done.").await;
        assert_eq!(title.as_deref(), Some("Replace TODO comment in calc.py"));
        assert!(h.cli.runs()[0].has(&["--model", "haiku"]));
    });
}
