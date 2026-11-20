//! Code sessions powered by the official Claude Code CLI.
//!
//! The Claude desktop app's Code tab runs Claude Code. Here we do the same:
//! we start the user's installed `claude` binary in headless mode
//! (`-p --input-format stream-json --output-format stream-json`), stream its
//! events into the conversation tree and answer its permission requests from
//! our UI. Authentication is entirely the CLI's own (Claude subscription or
//! API key, whatever the user logged in with); this app never reads or
//! handles those credentials.

use crate::anthropic::Accumulator;
use crate::db::Message;
use crate::engine::{emit_conv_updated, emit_turn, update_conv_options};
use crate::error::{AppError, AppResult};
use crate::render::{self, ArtifactTracker, RenderCtx};
use crate::state::{AppState, LiveRun, PermissionReply};
use crate::util::{new_id, now_ms, truncate_chars};
use crate::view;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub const ENGINE: &str = "claude-code";

/// Locate the `claude` executable.
pub fn find_cli(override_path: &str) -> Option<PathBuf> {
    let o = override_path.trim();
    if !o.is_empty() {
        let p = PathBuf::from(o);
        return if p.exists() { Some(p) } else { None };
    }
    let path = crate::shellenv::path();
    let cwd = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    if let Ok(p) = which::which_in("claude", Some(&path), &cwd) {
        return Some(p);
    }
    let home = dirs::home_dir()?;
    let mut cands = vec![
        home.join(".claude").join("local").join("claude"),
        home.join(".local").join("bin").join("claude"),
        home.join(".claude").join("local").join("claude.exe"),
        home.join(".local").join("bin").join("claude.exe"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ];
    if let Some(appdata) = std::env::var_os("APPDATA") {
        cands.push(PathBuf::from(appdata).join("npm").join("claude.cmd"));
    }
    cands.into_iter().find(|p| p.exists())
}

/// Path and version of the CLI, for the settings screen.
pub async fn cli_info(override_path: &str) -> Value {
    let Some(p) = find_cli(override_path) else {
        return json!({"found": false});
    };
    let mut cmd = crate::shellenv::command(&p.to_string_lossy(), &["--version".to_string()]);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let version = match cmd.spawn() {
        Ok(child) => tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    json!({"found": true, "path": p.display().to_string(), "version": version})
}

/// Open a terminal running `claude` so the user can sign in with /login.
pub fn open_login_terminal(cli: &std::path::Path) -> AppResult<()> {
    let cli = cli.display().to_string();
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "cmd", "/K", &cli])
            .spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Terminal\" to do script \"{}\"\ntell application \"Terminal\" to activate",
            cli.replace('\\', "\\\\").replace('"', "\\\"")
        );
        std::process::Command::new("osascript").args(["-e", &script]).spawn()?;
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let terms: [(&str, &[&str]); 6] = [
            ("x-terminal-emulator", &["-e"]),
            ("gnome-terminal", &["--"]),
            ("konsole", &["-e"]),
            ("xfce4-terminal", &["-x"]),
            ("kitty", &[]),
            ("xterm", &["-e"]),
        ];
        let path = crate::shellenv::path();
        for (t, pre) in terms {
            if let Ok(bin) = which::which_in(t, Some(&path), "/") {
                let mut c = std::process::Command::new(bin);
                c.args(pre).arg(&cli);
                if c.spawn().is_ok() {
                    return Ok(());
                }
            }
        }
        Err(AppError::msg(format!(
            "No terminal app found. Open a terminal and run: {cli}"
        )))
    }
}

fn is_reminder(b: &Value) -> bool {
    b["type"] == "text" && b["text"].as_str().map(|t| t.starts_with("<system-reminder>")).unwrap_or(false)
}

struct TurnState {
    rows: Vec<Message>,
    /// The assistant row currently being filled: (API message id, index).
    /// Reset when a tool-result row is added, so a later message always
    /// gets its own row.
    current: Option<(String, usize)>,
    streaming: Option<(usize, Accumulator)>,
    first_id: Option<String>,
    siblings: Value,
    tracker: ArtifactTracker,
    last_emit: Instant,
}

impl TurnState {
    fn display_rows(&self) -> Vec<Message> {
        let mut rows = self.rows.clone();
        if let Some((i, acc)) = &self.streaming {
            let live = acc.content();
            if live.len() >= rows[*i].content.as_array().map(|a| a.len()).unwrap_or(0) {
                rows[*i].content = Value::Array(live);
            }
        }
        rows
    }

    fn running_tools(&self) -> HashMap<String, String> {
        let mut uses = vec![];
        let mut done = HashSet::new();
        for r in &self.rows {
            for b in r.content.as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("tool_use") => uses.push(b["id"].as_str().unwrap_or("").to_string()),
                    Some("tool_result") => {
                        done.insert(b["tool_use_id"].as_str().unwrap_or("").to_string());
                    }
                    _ => {}
                }
            }
        }
        uses.into_iter()
            .filter(|u| !done.contains(u))
            .map(|u| (u, "running".to_string()))
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn emit(app: &AppHandle, st: &AppState, conv_id: &str, ts: &mut TurnState, mcp: &HashMap<String, (String, String)>, math: bool, extra_live: &HashMap<String, String>, force: bool) {
    if !force && ts.last_emit.elapsed() < Duration::from_millis(45) {
        return;
    }
    ts.last_emit = Instant::now();
    if ts.rows.is_empty() {
        return;
    }
    let rows = ts.display_rows();
    let mut live_tools = ts.running_tools();
    for (k, v) in extra_live {
        live_tools.insert(k.clone(), v.clone());
    }
    let streaming_id = ts.streaming.as_ref().map(|(i, _)| rows[*i].id.clone());
    let empty = HashSet::new();
    let ctx = RenderCtx {
        math,
        live_tools: &live_tools,
        streaming_tools: &empty,
        mcp_names: mcp,
        streaming_msg: streaming_id.as_deref(),
    };
    let refs: Vec<&Message> = rows.iter().collect();
    let mut tracker = ts.tracker.clone();
    let mut t = render::assistant_turn(&refs, &ctx, &mut tracker);
    if let Some(f) = &ts.first_id {
        t["id"] = json!(f);
    }
    t["siblings"] = ts.siblings.clone();
    t["streaming"] = json!(true);
    emit_turn(app, conv_id, &t);
    let live_idx = ts.streaming.as_ref().map(|(i, _)| *i).unwrap_or(rows.len() - 1);
    st.live.lock().insert(
        conv_id.to_string(),
        LiveRun {
            msg_id: rows[live_idx].id.clone(),
            content: rows[live_idx].content.clone(),
            live_tools,
            streaming_tools: HashSet::new(),
        },
    );
}

async fn write_line(stdin: &mut Option<tokio::process::ChildStdin>, v: &Value) {
    if let Some(w) = stdin.as_mut() {
        let _ = w.write_all(format!("{v}\n").as_bytes()).await;
        let _ = w.flush().await;
    }
}

/// Run one user turn through the Claude Code CLI.
/// How a turn ended, when it didn't fail outright.
enum Outcome {
    Done,
    /// The CLI no longer has the session we tried to resume (transcripts are
    /// cleaned up after a while); nothing was stored for this attempt.
    ResumeLost,
}

pub async fn run(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, user_msg_id: String, cancel: &CancellationToken) -> AppResult<()> {
    match run_once(app, st, conv_id, user_msg_id.clone(), cancel, false).await? {
        Outcome::Done => Ok(()),
        // Start over in a new session that gets the earlier conversation as context.
        Outcome::ResumeLost => run_once(app, st, conv_id, user_msg_id, cancel, true).await.map(|_| ()),
    }
}

/// Is there still a transcript for this session id in Claude Code's data?
fn transcript_exists(session_id: &str) -> bool {
    let Some(dir) = crate::import::claude_code_dir() else { return true };
    let Ok(projects) = std::fs::read_dir(dir.join("projects")) else { return true };
    let file = format!("{session_id}.jsonl");
    projects.flatten().any(|p| p.path().join(&file).is_file())
}

/// The earlier conversation as plain text, for a new session that can't
/// resume the old one.
fn history_context(path: &[Message]) -> String {
    let mut parts: Vec<String> = vec![];
    for m in path {
        if m.meta["tool_results"] == true || render::is_tool_result_msg(m) {
            continue;
        }
        let content = Value::Array(m.content.as_array().cloned().unwrap_or_default().into_iter().filter(|b| !is_reminder(b)).collect());
        let text = render::plain_text(&content);
        if text.trim().is_empty() {
            continue;
        }
        let who = if m.role == "user" { "User" } else { "Assistant" };
        parts.push(format!("{who}: {}", text.trim()));
    }
    let all = parts.join("\n\n");
    let n = all.chars().count();
    if n > 40_000 {
        all.chars().skip(n - 40_000).collect()
    } else {
        all
    }
}

/// Permission rules the user allowed "for this session", turned into
/// `--allowedTools` entries so they hold on every later turn of this chat.
fn allowed_rules(suggestions: &Value) -> Vec<String> {
    let mut out = vec![];
    for sug in suggestions.as_array().into_iter().flatten() {
        if sug["type"] != "addRules" || sug["behavior"] != "allow" {
            continue;
        }
        for r in sug["rules"].as_array().into_iter().flatten() {
            let Some(tool) = r["toolName"].as_str().filter(|t| !t.is_empty()) else { continue };
            match r["ruleContent"].as_str().filter(|c| !c.is_empty()) {
                Some(c) => out.push(format!("{tool}({c})")),
                None => out.push(tool.to_string()),
            }
        }
    }
    out
}

/// A finished (or cancelled) permission prompt, handed back to the loop that
/// owns the CLI's stdin.
struct PermDone {
    cli_request_id: Value,
    req_id: String,
    tool: String,
    input: Value,
    tool_use_id: String,
    suggestions: Value,
    reply: Option<PermissionReply>,
}

fn is_resume_lost(text: &str) -> bool {
    text.contains("No conversation found with session ID") || text.contains("No message found with message.uuid")
}

#[allow(unused_assignments)]
async fn run_once(
    app: &AppHandle,
    st: &Arc<AppState>,
    conv_id: &str,
    user_msg_id: String,
    cancel: &CancellationToken,
    fresh: bool,
) -> AppResult<Outcome> {
    let conv = st.db.conv_get(conv_id)?;
    let settings = st.settings.read().clone();
    let user = st.db.msg_get(&user_msg_id)?;
    let path = st.db.path_to(&user_msg_id)?;
    let cwd = conv.cwd.clone().unwrap_or_default();

    let fail = |msg: String| -> AppResult<Outcome> {
        let m = Message {
            id: new_id(),
            conversation_id: conv_id.to_string(),
            parent_id: Some(user_msg_id.clone()),
            role: "assistant".into(),
            content: json!([]),
            meta: json!({"error": {"message": msg, "kind": "claude_code"}}),
            created_at: now_ms(),
        };
        st.db.msg_insert(&m)?;
        st.db.conv_set_leaf(conv_id, Some(&m.id))?;
        Err(AppError::Api { status: 0, kind: "claude_code".into(), message: msg })
    };

    let Some(cli) = find_cli(&settings.claude_code_path) else {
        return fail("Claude Code isn't installed (or wasn't found). Install it from claude.com/claude-code, sign in once by running `claude` in a terminal, then try again. You can also set its path in Settings → Code.".into());
    };
    if !std::path::Path::new(&cwd).is_dir() {
        return fail(format!("The project folder no longer exists: {cwd}"));
    }

    // Resume the CLI session at the right point in our tree.
    let prev = path[..path.len().saturating_sub(1)]
        .iter()
        .rev()
        .find(|m| m.meta["cc_uuid"].is_string());
    let mode = conv.options["permission_mode"].as_str().unwrap_or("default").to_string();
    let mut args: Vec<String> = [
        "-p", "--output-format", "stream-json", "--verbose", "--input-format", "stream-json",
        "--include-partial-messages", "--permission-prompt-tool=stdio", "--permission-mode",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // Auto mode is ours: the CLI asks (default mode) and we answer safe
    // requests ourselves.
    args.push(if mode == "auto" { "default".to_string() } else { mode.clone() });
    if !conv.model.trim().is_empty() && conv.model != "default" {
        args.push("--model".into());
        args.push(conv.model.clone());
    }
    let budget = crate::engine::turn_budget(&conv, &st.settings.read());
    if budget > 0.0 {
        args.push("--max-budget-usd".into());
        args.push(budget.to_string());
    }
    let mut with_history = false;
    if let Some(p) = prev {
        if let Some(sid) = p.meta["cc_session"].as_str() {
            if fresh || !transcript_exists(sid) {
                with_history = true;
            } else {
                args.push("--resume".into());
                args.push(sid.to_string());
                let at_tip = conv.options["cc_tip"].as_str() == Some(p.id.as_str());
                if !at_tip {
                    args.push("--resume-session-at".into());
                    args.push(p.meta["cc_uuid"].as_str().unwrap_or("").to_string());
                    args.push("--fork-session".into());
                }
            }
        }
    }
    // The session's tip moves with this turn; until it's known again, the
    // next message must not assume it continues from the old tip.
    update_conv_options(st, conv_id, |o| {
        if let Some(m) = o.as_object_mut() {
            m.remove("cc_tip");
        }
    });
    // Grants from "allow for this session" (kept in this chat, not written
    // to the project's Claude Code settings). Variadic flags go last.
    let dirs: Vec<String> = conv.options["cc_add_dirs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d.as_str().map(String::from))
        .collect();
    if !dirs.is_empty() {
        args.push("--add-dir".into());
        args.extend(dirs);
    }
    let rules: Vec<String> = conv.options["cc_allowed_tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d.as_str().map(String::from))
        .collect();
    if !rules.is_empty() {
        args.push("--allowedTools".into());
        args.extend(rules);
    }

    let mut content: Vec<Value> = user
        .content
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|b| !is_reminder(b))
        .collect();
    if with_history {
        let h = history_context(&path[..path.len().saturating_sub(1)]);
        if !h.is_empty() {
            content.insert(0, json!({"type": "text", "text": format!(
                "<earlier_conversation>\n{h}\n</earlier_conversation>\n\nThe Claude Code session for the conversation above is no longer available, so this continues in a new session. Use it as context for the message below."
            )}));
        }
    }
    let first_line = json!({
        "type": "user",
        "message": {"role": "user", "content": content},
        "parent_tool_use_id": null,
        "session_id": ""
    });

    let mut cmd = crate::shellenv::command(&cli.to_string_lossy(), &args);
    cmd.current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Don't make the CLI think it's nested inside another Claude Code session.
    for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID"] {
        cmd.env_remove(k);
    }
    // Own process group, so the CLI and everything it started (background
    // shells, MCP servers) can be stopped together.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("Could not start Claude Code ({}): {e}", cli.display())),
    };
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let err_tail: Arc<parking_lot::Mutex<VecDeque<String>>> = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    {
        let err_tail = err_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut t = err_tail.lock();
                if t.len() > 40 {
                    t.pop_front();
                }
                t.push_back(l);
            }
        });
    }
    write_line(&mut stdin, &first_line).await;

    let nodes = st.db.tree_nodes(conv_id)?;
    let turn_start = path
        .iter()
        .rposition(|m| m.role == "user" && !render::is_tool_result_msg(m))
        .unwrap_or(0);
    let mut ts = TurnState {
        rows: vec![],
        current: None,
        streaming: None,
        first_id: None,
        siblings: json!({"index": 0, "count": 1, "ids": []}),
        tracker: view::tracker_for(&path[..turn_start]),
        last_emit: Instant::now() - Duration::from_secs(1),
    };
    let _ = nodes;
    let mcp: HashMap<String, (String, String)> = HashMap::new();
    let math = settings.latex_enabled;
    let mut session_id: Option<String> = None;
    let mut result: Option<Value> = None;
    let mut interrupted = false;
    let mut interrupt_deadline: Option<Instant> = None;
    // When the CLI reported its result. It can stay alive afterwards (for
    // example while a background task runs); the turn still ends here.
    let mut result_at: Option<Instant> = None;
    let mut lines = BufReader::new(stdout).lines();
    let mut awaiting: HashMap<String, String> = HashMap::new();
    let (perm_tx, mut perm_rx) = tokio::sync::mpsc::unbounded_channel::<PermDone>();
    // Open permission prompts: CLI request id → (our request id, tool_use_id).
    let mut open_perms: HashMap<String, (String, String)> = HashMap::new();
    let rid_key = |v: &Value| v.as_str().map(String::from).unwrap_or_else(|| v.to_string());

    // Parent for the next row we create.
    let mut parent = user_msg_id.clone();

    macro_rules! new_row {
        ($role:expr, $content:expr, $meta:expr) => {{
            let m = Message {
                id: new_id(),
                conversation_id: conv_id.to_string(),
                parent_id: Some(parent.clone()),
                role: $role.into(),
                content: $content,
                meta: $meta,
                created_at: now_ms(),
            };
            st.db.msg_insert(&m)?;
            st.db.conv_set_leaf(conv_id, Some(&m.id))?;
            parent = m.id.clone();
            if ts.first_id.is_none() && m.role == "assistant" {
                ts.first_id = Some(m.id.clone());
                let nodes = st.db.tree_nodes(conv_id)?;
                ts.siblings = view::siblings_of(&nodes, &m.id);
            }
            ts.rows.push(m);
            ts.rows.len() - 1
        }};
    }

    loop {
        let line = tokio::select! {
            l = lines.next_line() => l,
            _ = cancel.cancelled(), if !interrupted => {
                interrupted = true;
                interrupt_deadline = Some(Instant::now() + Duration::from_secs(5));
                write_line(&mut stdin, &json!({"type": "control_request", "request_id": new_id(), "request": {"subtype": "interrupt"}})).await;
                continue;
            }
            Some(done) = perm_rx.recv() => {
                let key = rid_key(&done.cli_request_id);
                // Already cancelled by the CLI: nothing to answer.
                if open_perms.remove(&key).is_none() {
                    continue;
                }
                st.permissions.lock().remove(&done.req_id);
                st.pending_perms.lock().remove(&done.req_id);
                awaiting.remove(&done.tool_use_id);
                let _ = app.emit("permission-resolved", json!({"request_id": done.req_id, "conversation_id": conv_id}));
                let response = permission_response(app, st, conv_id, &done);
                write_line(&mut stdin, &json!({"type": "control_response", "response": {"subtype": "success", "request_id": done.cli_request_id, "response": response}})).await;
                emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, true);
                continue;
            }
            _ = tokio::time::sleep(Duration::from_millis(250)), if interrupt_deadline.is_some() || result_at.is_some() => {
                let now = Instant::now();
                let interrupt_over = interrupt_deadline.map(|d| now > d).unwrap_or(false);
                let result_over = result_at.map(|t| now.duration_since(t) > Duration::from_secs(3)).unwrap_or(false);
                if interrupt_over || result_over {
                    break;
                }
                continue;
            }
        };
        let Ok(Some(line)) = line else { break };
        let Ok(ev) = serde_json::from_str::<Value>(&line) else { continue };
        let top_level = ev["parent_tool_use_id"].is_null();
        match ev["type"].as_str().unwrap_or("") {
            "system" => {
                if ev["subtype"] == "init" {
                    if let Some(sid) = ev["session_id"].as_str() {
                        session_id = Some(sid.to_string());
                        let sid = sid.to_string();
                        update_conv_options(st, conv_id, |o| o["cc_session"] = json!(sid));
                    }
                }
            }
            "stream_event" if top_level => {
                let se = &ev["event"];
                if se["type"] == "message_start" {
                    let api_id = se["message"]["id"].as_str().unwrap_or("").to_string();
                    let idx = match &ts.current {
                        Some((id, i)) if *id == api_id => *i,
                        _ => {
                            let model = se["message"]["model"].clone();
                            let i = new_row!("assistant", json!([]), json!({"model": model, "engine": ENGINE}));
                            ts.current = Some((api_id, i));
                            i
                        }
                    };
                    let mut acc = Accumulator::default();
                    let _ = acc.apply(se);
                    ts.streaming = Some((idx, acc));
                } else if let Some((_, acc)) = ts.streaming.as_mut() {
                    let _ = acc.apply(se);
                    if se["type"] == "message_stop" {
                        ts.streaming = None;
                    }
                }
                emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, false);
            }
            "assistant" if top_level => {
                let msg = &ev["message"];
                let api_id = msg["id"].as_str().unwrap_or("").to_string();
                let blocks = msg["content"].as_array().cloned().unwrap_or_default();
                let idx = match &ts.current {
                    Some((id, i)) if *id == api_id => *i,
                    _ => {
                        let i = new_row!("assistant", json!([]), json!({"model": msg["model"], "engine": ENGINE}));
                        ts.current = Some((api_id.clone(), i));
                        i
                    }
                };
                let row = &mut ts.rows[idx];
                let mut stored = row.content.as_array().cloned().unwrap_or_default();
                stored.extend(blocks);
                row.content = Value::Array(stored);
                row.meta["cc_uuid"] = ev["uuid"].clone();
                row.meta["cc_session"] = json!(ev["session_id"].as_str().or(session_id.as_deref()));
                if !msg["usage"].is_null() {
                    row.meta["usage"] = msg["usage"].clone();
                }
                st.db.msg_update(&row.id, &row.content, &row.meta)?;
                emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, true);
            }
            "user" if top_level => {
                let c = ev["message"]["content"].as_array().cloned().unwrap_or_default();
                if c.iter().any(|b| b["type"] == "tool_result") {
                    let meta = json!({
                        "tool_results": true,
                        "cc_uuid": ev["uuid"],
                        "cc_session": ev["session_id"].as_str().or(session_id.as_deref()),
                    });
                    let _ = new_row!("user", Value::Array(c), meta);
                    let _ = app.emit("files-changed", json!({"conversation_id": conv_id}));
                    ts.current = None;
                    ts.streaming = None;
                    emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, true);
                }
            }
            "control_request" => {
                let req = &ev["request"];
                let rid = ev["request_id"].clone();
                if req["subtype"] == "can_use_tool" {
                    let tool = req["tool_name"].as_str().unwrap_or("").to_string();
                    let input = req["input"].clone();
                    let tool_use_id = req["tool_use_id"].as_str().unwrap_or("").to_string();
                    let kind = if tool == "ExitPlanMode" { "plan" } else { "code" };
                    // Auto mode: approve safe requests without asking.
                    let mode_now = st.db.conv_get(conv_id).ok().and_then(|c| c.options["permission_mode"].as_str().map(String::from)).unwrap_or_default();
                    if mode_now == "auto" && tool != "ExitPlanMode" {
                        let sess = crate::codetools::CodeSession::new(PathBuf::from(&cwd));
                        if crate::codetools::check_permission("auto", &tool, &input, &[], Some(&sess)) == crate::codetools::Perm::Allow {
                            write_line(&mut stdin, &json!({"type": "control_response", "response": {"subtype": "success", "request_id": rid,
                                "response": {"behavior": "allow", "updatedInput": input, "toolUseID": tool_use_id}}})).await;
                            continue;
                        }
                    }
                    let diff = {
                        let session = crate::codetools::CodeSession::new(PathBuf::from(&cwd));
                        let (tool, input) = (tool.clone(), input.clone());
                        tokio::task::spawn_blocking(move || crate::codetools::preview_diff(Some(&session), &tool, &input))
                            .await
                            .ok()
                            .flatten()
                    };
                    let req_id = new_id();
                    let (tx, rx) = oneshot::channel::<PermissionReply>();
                    st.permissions.lock().insert(req_id.clone(), tx);
                    let payload = json!({
                        "kind": kind, "tool": tool, "input": input, "diff": diff, "cwd": cwd,
                        "request_id": req_id, "conversation_id": conv_id, "tool_use_id": tool_use_id,
                        "description": req["description"],
                    });
                    st.pending_perms.lock().insert(req_id.clone(), payload.clone());
                    awaiting.insert(tool_use_id.clone(), "awaiting_permission".into());
                    open_perms.insert(rid_key(&rid), (req_id.clone(), tool_use_id.clone()));
                    emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, true);
                    let _ = app.emit("permission-request", &payload);
                    crate::engine::notify_attention(app, st, conv_id, &payload);
                    // Wait for the answer without blocking the event stream.
                    let tx = perm_tx.clone();
                    let cancel = cancel.clone();
                    let suggestions = req["permission_suggestions"].clone();
                    tokio::spawn(async move {
                        let reply = tokio::select! {
                            r = rx => r.ok(),
                            _ = cancel.cancelled() => None,
                        };
                        let _ = tx.send(PermDone { cli_request_id: rid, req_id, tool, input, tool_use_id, suggestions, reply });
                    });
                } else {
                    write_line(&mut stdin, &json!({"type": "control_response", "response": {"subtype": "error", "request_id": rid, "error": "Not supported by ClaudeRust"}})).await;
                }
            }
            "control_cancel_request" => {
                // The CLI no longer needs an answer (a hook decided, or the
                // tool call was aborted): take the card down.
                let rid = if ev["request_id"].is_null() { ev["request"]["request_id"].clone() } else { ev["request_id"].clone() };
                if let Some((req_id, tool_use_id)) = open_perms.remove(&rid_key(&rid)) {
                    st.permissions.lock().remove(&req_id);
                    st.pending_perms.lock().remove(&req_id);
                    awaiting.remove(&tool_use_id);
                    let _ = app.emit("permission-resolved", json!({"request_id": req_id, "conversation_id": conv_id}));
                    emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, true);
                }
            }
            "result" => {
                result = Some(ev.clone());
                result_at = Some(Instant::now());
                // closing stdin lets the CLI exit
                stdin = None;
            }
            _ => {}
        }
    }

    // Prompts the CLI never got an answer to.
    for (_, (req_id, _)) in open_perms.drain() {
        st.permissions.lock().remove(&req_id);
        st.pending_perms.lock().remove(&req_id);
        let _ = app.emit("permission-resolved", json!({"request_id": req_id, "conversation_id": conv_id}));
    }
    // Let the CLI exit on its own, then stop it and everything it started.
    drop(stdin);
    if tokio::time::timeout(Duration::from_secs(2), child.wait()).await.is_err() {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            crate::codetools::signal_process_group(pid, libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(3), child.wait()).await.is_err() {
            if let Some(pid) = child.id() {
                #[cfg(unix)]
                crate::codetools::kill_process_group(pid);
                #[cfg(windows)]
                crate::codetools::kill_process_tree(pid);
            }
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
    }
    ts.streaming = None;

    // Bookkeeping on the last assistant row.
    let last_assistant = ts.rows.iter().rposition(|r| r.role == "assistant");
    let err_text = {
        let t = err_tail.lock();
        t.iter().rev().take(8).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    let mut error: Option<String> = None;
    let mut budget_stop: Option<Value> = None;
    match &result {
        // the CLI stopped at the spending limit: a pause, not a failure
        Some(r) if r["subtype"].as_str().is_some_and(|t| t.contains("budget")) && !interrupted => {
            budget_stop = Some(json!({"budget": crate::engine::turn_budget(&conv, &st.settings.read()), "spent": r["total_cost_usd"]}));
        }
        Some(r) => {
            if r["is_error"].as_bool() == Some(true) && !interrupted {
                error = Some(
                    r["result"].as_str().map(|s| s.to_string()).filter(|s| !s.is_empty())
                        .or_else(|| r["errors"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n")))
                        .unwrap_or_else(|| format!("Claude Code reported an error ({}).", r["subtype"].as_str().unwrap_or("error"))),
                );
            }
        }
        None if !interrupted => {
            error = Some(if err_text.is_empty() {
                "Claude Code exited unexpectedly. Run `claude` in a terminal to check that it's installed and signed in.".into()
            } else {
                format!("Claude Code exited unexpectedly:\n{}", truncate_chars(&err_text, 1500))
            });
        }
        None => {}
    }
    if !fresh && ts.rows.is_empty() && !interrupted {
        let text = format!("{} {}", error.clone().unwrap_or_default(), err_text);
        if is_resume_lost(&text) {
            return Ok(Outcome::ResumeLost);
        }
    }
    match last_assistant {
        Some(i) => {
            let row = &mut ts.rows[i];
            if let Some(r) = &result {
                if !r["usage"].is_null() {
                    row.meta["usage"] = r["usage"].clone();
                }
                row.meta["cost_usd"] = r["total_cost_usd"].clone();
                row.meta["duration_ms"] = r["duration_ms"].clone();
            }
            row.meta["stop_reason"] = json!(if interrupted { Value::Null } else { json!("end_turn") });
            if interrupted {
                row.meta["stopped"] = json!(true);
            }
            if let Some(e) = &error {
                row.meta["error"] = json!({"message": e, "kind": "claude_code"});
            }
            if let Some(b) = &budget_stop {
                row.meta["budget_stop"] = b.clone();
            }
            st.db.msg_update(&row.id, &row.content, &row.meta)?;
            let text = render::plain_text(&row.content);
            let _ = st.db.fts_index(conv_id, &row.id, &text);
        }
        None => {
            if let Some(e) = &error {
                return fail(e.clone());
            }
            if interrupted {
                let i = new_row!("assistant", json!([]), json!({"stopped": true, "engine": ENGINE}));
                let _ = i;
            }
        }
    }
    // Remember where the CLI session ends so the next message resumes there.
    if let Some(tip) = ts.rows.iter().rev().find(|r| r.meta["cc_uuid"].is_string()) {
        let tip_id = tip.id.clone();
        update_conv_options(st, conv_id, |o| o["cc_tip"] = json!(tip_id));
    }
    if let Some(r) = ts.rows.last() {
        st.db.conv_set_leaf(conv_id, Some(&r.id))?;
    }
    emit(app, st, conv_id, &mut ts, &mcp, math, &HashMap::new(), true);
    match error {
        Some(e) => Err(AppError::Api { status: 0, kind: "claude_code".into(), message: e }),
        None => Ok(Outcome::Done),
    }
}

/// The CLI's answer for a permission prompt the user (or Stop) resolved.
fn permission_response(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, done: &PermDone) -> Value {
    let decision = done.reply.as_ref().map(|r| r.decision.as_str());
    let input = &done.input;
    let mut response = match decision {
        Some("once") | Some("approve") => json!({"behavior": "allow", "updatedInput": input}),
        Some("session") | Some("chat") | Some("always") => {
            // The CLI suggests rules saved to the project's
            // .claude/settings.local.json; keep them to this chat instead.
            let mut updates: Vec<Value> = done
                .suggestions
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|mut u| {
                    u["destination"] = json!("session");
                    u
                })
                .collect();
            let mut rules = allowed_rules(&Value::Array(updates.clone()));
            if updates.is_empty() {
                let rule = match (done.tool.as_str(), input["command"].as_str()) {
                    ("Bash", Some(c)) => format!("Bash({c})"),
                    (t, _) => t.to_string(),
                };
                rules.push(rule.clone());
                let (tool, content) = match rule.split_once('(') {
                    Some((t, c)) => (t.to_string(), Some(c.trim_end_matches(')').to_string())),
                    None => (rule.clone(), None),
                };
                let mut r = json!({"toolName": tool});
                if let Some(c) = content {
                    r["ruleContent"] = json!(c);
                }
                updates.push(json!({"type": "addRules", "rules": [r], "behavior": "allow", "destination": "session"}));
            }
            let dirs: Vec<String> = updates
                .iter()
                .filter(|u| u["type"] == "addDirectories")
                .flat_map(|u| u["directories"].as_array().cloned().unwrap_or_default())
                .filter_map(|d| d.as_str().map(String::from))
                .collect();
            let mode = updates
                .iter()
                .find(|u| u["type"] == "setMode")
                .and_then(|u| u["mode"].as_str().map(String::from));
            update_conv_options(st, conv_id, |o| {
                for (key, vals) in [("cc_allowed_tools", &rules), ("cc_add_dirs", &dirs)] {
                    let mut list: Vec<Value> = o[key].as_array().cloned().unwrap_or_default();
                    for v in vals {
                        if !list.iter().any(|x| x.as_str() == Some(v)) {
                            list.push(json!(v));
                        }
                    }
                    o[key] = Value::Array(list);
                }
                if let Some(m) = &mode {
                    o["permission_mode"] = json!(m);
                }
            });
            if mode.is_some() {
                emit_conv_updated(app, conv_id);
            }
            json!({"behavior": "allow", "updatedInput": input, "updatedPermissions": updates})
        }
        Some("approve_edits") => {
            update_conv_options(st, conv_id, |o| o["permission_mode"] = json!("acceptEdits"));
            emit_conv_updated(app, conv_id);
            json!({"behavior": "allow", "updatedInput": input,
                   "updatedPermissions": [{"type": "setMode", "mode": "acceptEdits", "destination": "session"}]})
        }
        _ => {
            let mut msg = String::from("The user denied this tool call.");
            if let Some(m) = done.reply.as_ref().and_then(|r| r.message.clone()).filter(|m| !m.trim().is_empty()) {
                msg.push_str(&format!(" The user said: {}", m.trim()));
            }
            json!({"behavior": "deny", "message": msg, "interrupt": done.reply.is_none()})
        }
    };
    response["toolUseID"] = json!(done.tool_use_id);
    if done.tool == "ExitPlanMode" && response["behavior"] == "allow" && decision == Some("approve") {
        update_conv_options(st, conv_id, |o| o["permission_mode"] = json!("default"));
        emit_conv_updated(app, conv_id);
        response["updatedPermissions"] = json!([{"type": "setMode", "mode": "default", "destination": "session"}]);
    }
    response
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_grants_become_allowed_tools() {
        let sugg = json!([
            {"type": "addRules", "behavior": "allow", "destination": "localSettings",
             "rules": [{"toolName": "Bash", "ruleContent": "npm test:*"}, {"toolName": "WebFetch"}]},
            {"type": "addRules", "behavior": "deny", "destination": "localSettings", "rules": [{"toolName": "Bash", "ruleContent": "rm:*"}]},
            {"type": "setMode", "mode": "acceptEdits", "destination": "session"}
        ]);
        assert_eq!(allowed_rules(&sugg), vec!["Bash(npm test:*)".to_string(), "WebFetch".to_string()]);
        assert!(is_resume_lost("Error: No conversation found with session ID: abc"));
        assert!(is_resume_lost("No message found with message.uuid of: x"));
        assert!(!is_resume_lost("rate limited"));
    }

    #[test]
    fn history_skips_tool_rows_and_reminders() {
        let m = |id: &str, role: &str, content: Value, meta: Value| Message {
            id: id.into(), conversation_id: "c".into(), parent_id: None, role: role.into(),
            content, meta, created_at: 0,
        };
        let path = vec![
            m("u1", "user", json!([{"type":"text","text":"<system-reminder>x</system-reminder>"},{"type":"text","text":"fix it"}]), json!({})),
            m("a1", "assistant", json!([{"type":"text","text":"Looking."},{"type":"tool_use","id":"t","name":"Read","input":{}}]), json!({})),
            m("r1", "user", json!([{"type":"tool_result","tool_use_id":"t","content":"data"}]), json!({"tool_results": true})),
            m("a2", "assistant", json!([{"type":"text","text":"Done."}]), json!({})),
        ];
        let h = history_context(&path);
        assert_eq!(h, "User: fix it\n\nAssistant: Looking.\n\nAssistant: Done.");
    }
}
