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
#[allow(unused_assignments)]
pub async fn run(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, user_msg_id: String, cancel: &CancellationToken) -> AppResult<()> {
    let conv = st.db.conv_get(conv_id)?;
    let settings = st.settings.read().clone();
    let user = st.db.msg_get(&user_msg_id)?;
    let path = st.db.path_to(&user_msg_id)?;
    let cwd = conv.cwd.clone().unwrap_or_default();

    let fail = |msg: String| -> AppResult<()> {
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
    args.push(mode.clone());
    if !conv.model.trim().is_empty() && conv.model != "default" {
        args.push("--model".into());
        args.push(conv.model.clone());
    }
    if let Some(p) = prev {
        if let Some(sid) = p.meta["cc_session"].as_str() {
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

    let content: Vec<Value> = user
        .content
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|b| !is_reminder(b))
        .collect();
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
    let mut lines = BufReader::new(stdout).lines();
    let mut awaiting: HashMap<String, String> = HashMap::new();

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
            _ = tokio::time::sleep(Duration::from_millis(500)), if interrupt_deadline.is_some() => {
                if interrupt_deadline.map(|d| Instant::now() > d).unwrap_or(false) {
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
                    let diff = crate::codetools::preview_diff(None, &tool, &input);
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
                    emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, true);
                    let _ = app.emit("permission-request", &payload);
                    let reply = tokio::select! {
                        r = rx => r.ok(),
                        _ = cancel.cancelled() => None,
                    };
                    st.permissions.lock().remove(&req_id);
                    st.pending_perms.lock().remove(&req_id);
                    awaiting.remove(&tool_use_id);
                    let _ = app.emit("permission-resolved", json!({"request_id": req_id, "conversation_id": conv_id}));
                    let suggestions = req["permission_suggestions"].clone();
                    let response = match reply.as_ref().map(|r| r.decision.as_str()) {
                        Some("once") | Some("approve") => json!({"behavior": "allow", "updatedInput": input}),
                        Some("session") | Some("chat") | Some("always") => {
                            let mut r = json!({"behavior": "allow", "updatedInput": input});
                            if suggestions.is_array() {
                                r["updatedPermissions"] = suggestions;
                            }
                            r
                        }
                        Some("approve_edits") => {
                            let m = "acceptEdits".to_string();
                            update_conv_options(st, conv_id, |o| o["permission_mode"] = json!(m));
                            emit_conv_updated(app, conv_id);
                            json!({"behavior": "allow", "updatedInput": input,
                                   "updatedPermissions": [{"type": "setMode", "mode": "acceptEdits", "destination": "session"}]})
                        }
                        _ => {
                            let mut msg = String::from("The user denied this tool call.");
                            if let Some(m) = reply.as_ref().and_then(|r| r.message.clone()).filter(|m| !m.trim().is_empty()) {
                                msg.push_str(&format!(" The user said: {}", m.trim()));
                            }
                            json!({"behavior": "deny", "message": msg, "interrupt": reply.is_none()})
                        }
                    };
                    let mut response = response;
                    response["toolUseID"] = json!(tool_use_id);
                    if tool == "ExitPlanMode" && response["behavior"] == "allow" && reply.as_ref().map(|r| r.decision.as_str()) == Some("approve") {
                        update_conv_options(st, conv_id, |o| o["permission_mode"] = json!("default"));
                        emit_conv_updated(app, conv_id);
                        response["updatedPermissions"] = json!([{"type": "setMode", "mode": "default", "destination": "session"}]);
                    }
                    write_line(&mut stdin, &json!({"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": response}})).await;
                    emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, true);
                } else {
                    write_line(&mut stdin, &json!({"type": "control_response", "response": {"subtype": "error", "request_id": rid, "error": "Not supported by ClaudeRust"}})).await;
                }
            }
            "result" => {
                result = Some(ev.clone());
                // closing stdin lets the CLI exit
                stdin = None;
            }
            _ => {}
        }
    }

    // Wait for the process to finish (briefly), then make sure it's gone.
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    let _ = child.start_kill();
    ts.streaming = None;

    // Bookkeeping on the last assistant row.
    let last_assistant = ts.rows.iter().rposition(|r| r.role == "assistant");
    let err_text = {
        let t = err_tail.lock();
        t.iter().rev().take(8).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    let mut error: Option<String> = None;
    match &result {
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
        None => Ok(()),
    }
}
