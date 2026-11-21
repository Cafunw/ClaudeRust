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
    crate::shellenv::ready().await;
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

/// Asks a CLI started like a turn's (with nothing to answer) what it applies for `model` here: the effort
/// it runs at when none was chosen, whether Ultracode can be switched on (a model with xhigh and
/// dynamic workflows turned on), and whether settings turn it on for every session. An empty object
/// when it can't say.
pub async fn session_info(override_path: &str, model: &str, cwd: &std::path::Path) -> Value {
    crate::shellenv::ready().await;
    let Some(cli) = find_cli(override_path) else { return json!({}) };
    let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--input-format", "stream-json"].iter().map(|s| s.to_string()).collect();
    if !model.trim().is_empty() && model != "default" {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    let mut cmd = crate::shellenv::command(&cli.to_string_lossy(), &args);
    cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID"] {
        cmd.env_remove(k);
    }
    let Ok(mut child) = cmd.spawn() else { return json!({}) };
    let mut stdin = child.stdin.take();
    let Some(stdout) = child.stdout.take() else { return json!({}) };
    let rid = new_id();
    write_line(&mut stdin, &json!({"type": "control_request", "request_id": rid, "request": {"subtype": "get_settings"}})).await;
    let mut lines = BufReader::new(stdout).lines();
    let applied = tokio::time::timeout(Duration::from_secs(15), async {
        while let Ok(Some(l)) = lines.next_line().await {
            let Ok(ev) = serde_json::from_str::<Value>(&l) else { continue };
            // only `applied`: the rest of get_settings carries the settings' env values
            if ev["type"] == "control_response" && ev["response"]["request_id"].as_str() == Some(rid.as_str()) {
                return Some(ev["response"]["response"]["applied"].clone());
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    stop_cli(child, stdin, false).await;
    match applied {
        Some(a) => json!({"effort": a["effort"], "ultracode_available": a["ultracodeAvailable"], "ultracode": a["ultracodeRequested"]}),
        None => json!({}),
    }
}

/// Open a terminal running `claude` so the user can sign in with /login.
pub fn open_login_terminal(cli: &std::path::Path) -> AppResult<()> {
    open_in_terminal(None, cli, &[])
}

const NOT_INSTALLED: &str = "Claude Code isn't installed. Install it from claude.com/claude-code first.";

/// Run `claude <args>` to the end (stdin closed) and keep what it printed.
async fn cli_output(cli: &std::path::Path, args: &[&str], secs: u64) -> Option<std::process::Output> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let mut cmd = crate::shellenv::command(&cli.to_string_lossy(), &args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd.spawn().ok()?;
    tokio::time::timeout(Duration::from_secs(secs), child.wait_with_output()).await.ok()?.ok()
}

/// Who the CLI is signed in as (`claude auth status`). That account is what
/// Code sessions, and chats set to "Claude account", run on; the sign-in itself
/// is the CLI's: this app never sees the token.
pub async fn auth_status(override_path: &str) -> Value {
    crate::shellenv::ready().await;
    let Some(cli) = find_cli(override_path) else {
        return json!({"found": false});
    };
    // not JSON from a CLI too old to have `auth status`
    let v: Value = cli_output(&cli, &["auth", "status", "--json"], 15).await.and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or(Value::Null);
    json!({"found": true, "logged_in": v["loggedIn"] == true, "method": v["authMethod"], "email": v["email"], "plan": v["subscriptionType"], "tier": v["planTier"]})
}

/// The running sign-in, so a second one (or Cancel) can stop it.
static LOGIN: parking_lot::Mutex<Option<CancellationToken>> = parking_lot::Mutex::new(None);

/// `claude auth login`: the CLI opens the browser and ends when the sign-in
/// does. Resolves with the new status, or `{"cancelled": true}`.
pub async fn auth_login(override_path: &str) -> AppResult<Value> {
    crate::shellenv::ready().await;
    let cli = find_cli(override_path).ok_or_else(|| AppError::msg(NOT_INSTALLED))?;
    let cancel = CancellationToken::new();
    if let Some(old) = LOGIN.lock().replace(cancel.clone()) {
        old.cancel();
    }
    let args: Vec<String> = ["auth", "login", "--claudeai"].iter().map(|s| s.to_string()).collect();
    let mut cmd = crate::shellenv::command(&cli.to_string_lossy(), &args);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID"] {
        cmd.env_remove(k);
    }
    let mut child = cmd.spawn().map_err(|e| AppError::msg(format!("Could not start Claude Code ({}): {e}", cli.display())))?;
    let mut stderr = child.stderr.take();
    // shortcut: no way to paste the code the browser page shows (the CLI's fallback when it can't catch the redirect), so a sign-in that needs it times out; use the terminal sign-in then
    let end = tokio::select! {
        s = child.wait() => s.map(Some),
        _ = cancel.cancelled() => Ok(None),
        _ = tokio::time::sleep(Duration::from_secs(300)) => Err(std::io::Error::other("Sign-in didn't finish within 5 minutes.")),
    };
    if !cancel.is_cancelled() {
        LOGIN.lock().take();
    }
    match end {
        Ok(Some(status)) if status.success() => Ok(auth_status(override_path).await),
        Ok(Some(_)) => {
            let mut text = String::new();
            if let Some(e) = stderr.as_mut() {
                let _ = tokio::io::AsyncReadExt::read_to_string(e, &mut text).await;
            }
            Err(AppError::msg(if text.trim().is_empty() { "Sign-in failed.".to_string() } else { truncate_chars(text.trim(), 500) }))
        }
        Ok(None) => {
            let _ = child.kill().await;
            Ok(json!({"cancelled": true}))
        }
        Err(e) => {
            let _ = child.kill().await;
            Err(e.into())
        }
    }
}

pub fn auth_login_cancel() {
    if let Some(t) = LOGIN.lock().take() {
        t.cancel();
    }
}

/// `claude auth logout`. It signs out the CLI itself, for Claude Code in a terminal too.
pub async fn auth_logout(override_path: &str) -> AppResult<()> {
    crate::shellenv::ready().await;
    let cli = find_cli(override_path).ok_or_else(|| AppError::msg(NOT_INSTALLED))?;
    match cli_output(&cli, &["auth", "logout"], 20).await {
        Some(o) if o.status.success() => Ok(()),
        Some(o) => {
            let text = String::from_utf8_lossy(if o.stderr.is_empty() { &o.stdout } else { &o.stderr }).trim().to_string();
            Err(AppError::msg(if text.is_empty() { "Sign-out failed.".to_string() } else { truncate_chars(&text, 500) }))
        }
        None => Err(AppError::msg("Sign-out didn't finish.")),
    }
}

/// An empty folder of ours where chat conversations run the CLI, so no
/// project's files, memory or settings are within its reach.
fn chat_dir(st: &AppState) -> String {
    let d = st.data_dir.join("chat-cli");
    let _ = std::fs::create_dir_all(&d);
    d.display().to_string()
}

/// Does a turn of this conversation run through the CLI (and so on the Claude
/// account) rather than the API?
pub async fn uses_cli(st: &AppState, conv: &crate::db::Conversation) -> bool {
    if conv.model.starts_with(crate::oai::PREFIX) {
        return false;
    }
    if conv.kind == "code" {
        return conv.options["engine"] == ENGINE;
    }
    crate::shellenv::ready().await;
    let s = st.settings.read();
    match s.chat_engine.as_str() {
        "claude-code" => true,
        "api" => false,
        // no API key: the account, when there is a CLI to sign in with
        _ => st.api_key.read().is_none() && find_cli(&s.claude_code_path).is_some(),
    }
}

/// Open the system terminal running `program args…`, in `cwd` when given.
/// The folder and arguments are passed as separate words (never pasted into
/// a shell line), so spaces and quotes in them are safe.
pub fn open_in_terminal(cwd: Option<&std::path::Path>, program: &std::path::Path, args: &[String]) -> AppResult<()> {
    let prog = program.display().to_string();
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        // cmd.exe re-reads its command line: every word is quoted (so & | < >
        // ^ in a path stay literal) and the whole line wrapped once more for
        // /S; the folder is the new console's working directory, not a word
        let line = std::iter::once(prog.as_str()).chain(args.iter().map(String::as_str)).map(|w| format!("\"{}\"", crate::shellenv::cmd_safe(w))).collect::<Vec<_>>().join(" ");
        let mut c = std::process::Command::new("cmd");
        c.raw_arg(format!("/S /K \"{line}\"")).creation_flags(CREATE_NEW_CONSOLE);
        if let Some(d) = cwd {
            c.current_dir(d);
        }
        c.spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        // a POSIX-quoted shell line for Terminal's `do script`
        let sq = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let mut line = String::new();
        if let Some(d) = cwd {
            line.push_str(&format!("cd {} && ", sq(&d.display().to_string())));
        }
        line.push_str(&std::iter::once(sq(&prog)).chain(args.iter().map(|a| sq(a))).collect::<Vec<_>>().join(" "));
        let script = format!(
            "tell application \"Terminal\" to do script \"{}\"\ntell application \"Terminal\" to activate",
            line.replace('\\', "\\\\").replace('"', "\\\"")
        );
        std::process::Command::new("osascript").args(["-e", &script]).spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // terminals known to take the command as separate words first; the
        // ones that want it as one string (lxterminal, and possibly the
        // distribution's `x-terminal-emulator`) get a quoted shell line
        let terms: [(&str, &[&str], bool); 9] = [
            ("gnome-terminal", &["--"], false),
            ("konsole", &["-e"], false),
            ("xfce4-terminal", &["-x"], false),
            ("kitty", &[], false),
            ("alacritty", &["-e"], false),
            ("wezterm", &["start", "--"], false),
            ("xterm", &["-e"], false),
            ("lxterminal", &["-e"], true),
            ("x-terminal-emulator", &["-e"], true),
        ];
        // sh -c 'cd "$0" && exec "$@"' <dir> <program> <args…>
        let dir = cwd.map(|d| d.display().to_string()).unwrap_or_else(|| ".".into());
        let words: Vec<String> = ["sh", "-c", "cd \"$0\" && exec \"$@\"", &dir, &prog].iter().map(|s| s.to_string()).chain(args.iter().cloned()).collect();
        let sq = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let one_line = words.iter().map(|w| sq(w)).collect::<Vec<_>>().join(" ");
        let path = crate::shellenv::path();
        for (t, pre, as_line) in terms {
            if let Ok(bin) = which::which_in(t, Some(&path), "/") {
                let mut c = std::process::Command::new(bin);
                c.args(pre);
                if as_line {
                    c.arg(&one_line);
                } else {
                    c.args(&words);
                }
                if let Some(d) = cwd {
                    c.current_dir(d);
                }
                if c.spawn().is_ok() {
                    return Ok(());
                }
            }
        }
        let mut cmd = prog.clone();
        for a in args {
            cmd.push(' ');
            cmd.push_str(a);
        }
        Err(AppError::msg(format!(
            "No terminal app found. Open a terminal{} and run: {cmd}",
            cwd.map(|d| format!(" in {}", d.display())).unwrap_or_default()
        )))
    }
}

/// The Claude Code session a conversation's latest answer belongs to.
pub fn latest_session(path: &[Message]) -> Option<String> {
    path.iter().rev().find_map(|m| m.meta["cc_session"].as_str().map(str::to_string))
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
    /// What the CLI had spent before this turn: its results report a running total.
    cost_before: f64,
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

/// Stop the CLI at the end of a turn: closing its stdin lets it exit on its
/// own; if it doesn't, it and everything it started (its process group) get
/// SIGTERM, then are killed. `broke_off`: the turn ended on an error, so it
/// isn't given time to finish by itself.
async fn stop_cli(mut child: tokio::process::Child, stdin: Option<tokio::process::ChildStdin>, broke_off: bool) {
    drop(stdin);
    if !broke_off && tokio::time::timeout(Duration::from_secs(2), child.wait()).await.is_ok() {
        return;
    }
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
    /// A message was stored as the CLI went away and no turn took it: its own turn.
    Next(String),
}

pub async fn run(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, user_msg_id: String, cancel: &CancellationToken) -> AppResult<()> {
    crate::shellenv::ready().await;
    let (mut id, mut fresh) = (user_msg_id, false);
    loop {
        match run_once(app, st, conv_id, id.clone(), cancel, fresh).await? {
            Outcome::Done => return Ok(()),
            // Start over in a new session that gets the earlier conversation as context.
            Outcome::ResumeLost => fresh = true,
            Outcome::Next(next) => (id, fresh) = (next, false),
        }
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

/// The effort and ultracode flags for a turn. Only a level the model has goes
/// through: one left from another model is dropped, as the built-in engine does.
/// Ultracode lives only as long as the CLI process (`--resume` doesn't bring it
/// back), so a choice goes with every spawn: on, or off over settings that turn
/// it on for every session; none leaves the CLI's own. Chats have no tools, so no Workflow tool.
pub fn cli_session_flags(options: &Value, model: &str, chat: bool, data_dir: &std::path::Path) -> Vec<String> {
    let m = crate::models::info(model);
    let mut out = vec![];
    if let Some(e) = options["effort"].as_str().filter(|e| m.efforts.contains(e)) {
        out.extend(["--effort".to_string(), e.to_string()]);
    }
    if let Some(on) = options["ultracode"].as_bool().filter(|_| !chat && m.ultracode) {
        out.extend(["--settings".to_string(), ultracode_settings_file(data_dir, on).display().to_string()]);
    }
    out
}

/// The settings file that switches ultracode on or off. `--settings` takes a path or
/// inline JSON; a path survives the quoting of cmd.exe and of a terminal
/// line, which turn the JSON's quotes into something else.
fn ultracode_settings_file(data_dir: &std::path::Path, on: bool) -> PathBuf {
    let body = format!(r#"{{"ultracode":{on}}}"#);
    let p = data_dir.join(if on { "claude-ultracode.json" } else { "claude-ultracode-off.json" });
    if std::fs::read_to_string(&p).map_or(true, |s| s.trim() != body) {
        let _ = std::fs::write(&p, body);
    }
    p
}

/// What the running CLI turn can be told from outside.
pub enum Cmd {
    /// Stop one workflow (the CLI's `stop_task` request).
    StopTask(String),
    /// A user message, already stored, for the CLI while a workflow runs.
    Say(String),
    /// The person's Stop while workflows run: it ends the busy turn and spares
    /// them, as Claude Code does where each task has its own Stop.
    StopTurn,
}

/// The line to a conversation's running CLI turn, whether the turn is waiting
/// on a workflow (idle, so a message can go straight in), and whether a Stop
/// now ends only the busy turn (workflows run beside it).
pub struct Ctl {
    pub tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    pub waiting: bool,
    pub spare: bool,
}

/// Takes the conversation's line down when its run ends, however it ends.
struct CtlGuard {
    st: Arc<AppState>,
    conv_id: String,
}

impl Drop for CtlGuard {
    fn drop(&mut self) {
        self.st.cc_ctl.lock().remove(&self.conv_id);
    }
}

fn end_workflow(w: &mut Value, status: &str) {
    if w["status"] == "running" {
        w["status"] = json!(status);
        w["finished_at"] = json!(now_ms());
    }
}

/// Keeps what the Tasks pane shows of a workflow, from the CLI's task events
/// (name, phases, every agent's state and cost). True when something changed.
fn record_workflow(list: &mut Vec<Value>, ev: &Value) -> bool {
    let id = ev["task_id"].as_str().unwrap_or("");
    let at = list.iter().position(|w| w["id"] == id);
    match (ev["subtype"].as_str().unwrap_or(""), at) {
        ("task_started", None) if ev["task_type"] == "local_workflow" && ev["ambient"] != true && ev["off_host"] != true && !id.is_empty() => {
            list.push(json!({
                "id": id, "name": ev["workflow_name"], "description": ev["description"], "status": "running",
                "started_at": now_ms(), "finished_at": null, "tokens": 0, "tool_uses": 0,
                "phases": [], "agents": [], "output_file": null,
            }));
            if list.len() > 30 {
                list.remove(0);
            }
            true
        }
        ("task_progress", Some(i)) => {
            let w = &mut list[i];
            w["tokens"] = ev["usage"]["total_tokens"].clone();
            w["tool_uses"] = ev["usage"]["tool_uses"].clone();
            let (mut phases, mut agents) = (vec![], vec![]);
            for p in ev["workflow_progress"].as_array().into_iter().flatten() {
                match p["type"].as_str() {
                    Some("workflow_phase") => phases.push(json!({"index": p["index"], "title": p["title"]})),
                    Some("workflow_agent") => agents.push(json!({
                        "label": p["label"], "phase": p["phaseTitle"], "state": p["state"], "model": p["model"],
                        "tokens": p["tokens"], "tool_calls": p["toolCalls"], "duration_ms": p["durationMs"],
                        "last_tool": p["lastToolSummary"].as_str().or(p["lastToolName"].as_str()),
                        "result": p["resultPreview"].as_str().map(|s| truncate_chars(s, 400)),
                        "error": p["error"].as_str().map(|s| truncate_chars(s, 400)),
                    })),
                    _ => {}
                }
            }
            if !phases.is_empty() {
                w["phases"] = json!(phases);
            }
            if !agents.is_empty() {
                w["agents"] = json!(agents);
            }
            true
        }
        ("task_updated", Some(i)) => match ev["patch"]["status"].as_str() {
            Some(s @ ("completed" | "failed" | "killed")) => {
                end_workflow(&mut list[i], s);
                true
            }
            _ => false,
        },
        ("task_notification", Some(i)) => {
            let w = &mut list[i];
            end_workflow(w, match ev["status"].as_str() { Some("completed") => "completed", Some("failed") => "failed", _ => "killed" });
            w["output_file"] = ev["output_file"].clone();
            if !ev["usage"].is_null() {
                w["tokens"] = ev["usage"]["total_tokens"].clone();
                w["tool_uses"] = ev["usage"]["tool_uses"].clone();
            }
            true
        }
        _ => false,
    }
}

/// How long to wait, after the last workflow ended, for the follow-up turn the
/// CLI starts to report it (it normally starts at once).
const REPORT_WAIT: Duration = Duration::from_secs(30);

/// Follows the CLI's background workflows (ultracode's `local_workflow`
/// tasks; not shells, live-update watchers or remote tasks) from its system
/// events. True when one ended.
fn track_workflows(bg: &mut HashSet<String>, ev: &Value) -> bool {
    let wf = |t: &Value| t["task_type"] == "local_workflow" && t["ambient"] != true && t["off_host"] != true;
    let id = ev["task_id"].as_str().unwrap_or("");
    let n = bg.len();
    match ev["subtype"].as_str().unwrap_or("") {
        "task_started" if wf(ev) && !id.is_empty() => {
            bg.insert(id.to_string());
        }
        "task_notification" => {
            bg.remove(id);
        }
        "task_updated" if matches!(ev["patch"]["status"].as_str(), Some("completed" | "failed" | "killed")) => {
            bg.remove(id);
        }
        // the whole set of live tasks
        "background_tasks_changed" => {
            let old = std::mem::take(bg);
            *bg = ev["tasks"].as_array().into_iter().flatten().filter(|t| wf(t)).filter_map(|t| t["task_id"].as_str().map(String::from)).collect();
            return old.iter().any(|k| !bg.contains(k));
        }
        _ => {}
    }
    bg.len() < n
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
    // a chat conversation: plain chat on the Claude account, in a folder of ours
    let chat = conv.kind != "code";
    let cwd = if chat { chat_dir(st) } else { conv.cwd.clone().unwrap_or_default() };

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
    if let Some(fb) = crate::models::overload_fallback(&conv.model, &st.settings.read().overload_fallback) {
        args.push("--fallback-model".into());
        args.push(fb);
    }
    let flags = cli_session_flags(&conv.options, &conv.model, chat, &st.data_dir);
    let ultracode_asked = conv.options["ultracode"] == true && flags.iter().any(|a| a == "--settings");
    args.extend(flags);
    if chat {
        // our chat prompt (instructions, style, project knowledge) replaces the coding one, and
        // nothing else is switched on: no tools, skills, MCP servers or the CLI's own settings and hooks.
        // The prompt goes by file: a project's knowledge can pass the command-line limit.
        // shortcut: no web search or memory search in these chats; the CLI would ask permission for them
        let mut c = conv.clone();
        c.options["web_search"] = json!(false);
        let mut s = settings.clone();
        s.chat_search = false;
        let rc = crate::engine::build_request_context(st, &c, &s).await?;
        let file = std::path::Path::new(&cwd).join(format!("system-{conv_id}.txt"));
        if let Err(e) = std::fs::write(&file, rc.system) {
            return fail(format!("Could not write the chat prompt ({}): {e}", file.display()));
        }
        for a in ["--tools", "", "--disable-slash-commands", "--strict-mcp-config", "--setting-sources", ""] {
            args.push(a.into());
        }
        args.push("--system-prompt-file".into());
        args.push(file.display().to_string());
    } else if let Some(block) = crate::engine::conversation_instructions(&conv.options) {
        args.push("--append-system-prompt".into());
        args.push(block);
    }
    let budget = crate::engine::turn_budget(&conv, &st.settings.read());
    if budget > 0.0 {
        args.push("--max-budget-usd".into());
        args.push(budget.to_string());
    }
    // earlier messages but no CLI session to resume (a fork into another
    // folder, or a conversation begun with the built-in engine): replay them
    let mut with_history = prev.is_none() && path.len() > 1;
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
    let mut first_line = json!({
        "type": "user",
        "message": {"role": "user", "content": content},
        "parent_tool_use_id": null,
        "session_id": ""
    });
    // A person typed it: the CLI then honours what only a person may type, such as the "ultracode"
    // keyword. Not over a replayed conversation, whose earlier words aren't this message's.
    if !chat && !with_history {
        first_line["origin"] = json!({"kind": "human"});
    }

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
            // read to the end, whatever the encoding (a message in the system
            // code page must not stop it): the CLI blocks on a full pipe
            let mut r = BufReader::new(stderr);
            while let Ok(Some((l, _))) = crate::mcp::read_line_lossy(&mut r, 8 * 1024).await {
                let mut t = err_tail.lock();
                if t.len() > 40 {
                    t.pop_front();
                }
                t.push_back(l);
            }
        });
    }
    // the line to this run: Stop for one workflow, a message while one runs
    let (ctl_tx, mut ctl_rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    st.cc_ctl.lock().insert(conv_id.to_string(), Ctl { tx: ctl_tx, waiting: false, spare: false });
    let _ctl = CtlGuard { st: st.clone(), conv_id: conv_id.to_string() };
    write_line(&mut stdin, &first_line).await;
    // The CLI drops ultracode without a word when the account has dynamic
    // workflows off: ask what it applied.
    let uc_check = ultracode_asked.then(new_id);
    if let Some(id) = &uc_check {
        write_line(&mut stdin, &json!({"type": "control_request", "request_id": id, "request": {"subtype": "get_settings"}})).await;
    }

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
        cost_before: 0.0,
    };
    let mcp: HashMap<String, (String, String)> = HashMap::new();
    let math = settings.latex_enabled;
    let mut session_id: Option<String> = None;
    let mut result: Option<Value> = None;
    let mut interrupted = false;
    let mut interrupt_deadline: Option<Instant> = None;
    // When the CLI reported its result, and how long it then gets to exit. It
    // can stay alive afterwards (a background shell, say); the turn still ends
    // then, unless a workflow (ultracode) runs: those report in a later turn.
    let mut result_at: Option<Instant> = None;
    let mut result_grace = Duration::from_secs(3);
    // Live background workflows, and whether one ended since the last model
    // call (its report may still come as a follow-up turn).
    let mut bg: HashSet<String> = HashSet::new();
    let mut report_due = false;
    // A workflow's wait is in no turn's own duration_ms: the footer shows the wall time instead.
    let run_start = Instant::now();
    let mut ran_workflow = false;
    // between a result and the next model call
    let mut idle = false;
    // Messages typed while a workflow runs go to the CLI at once, each as a turn of its own: those not yet
    // handed over, whether the line is advertised as waiting, whether any was handed over, and the last row
    // of earlier turns that the CLI session continues from.
    let mut pending_say: VecDeque<String> = VecDeque::new();
    let mut waiting = false;
    // whether a Stop now ends only the busy turn, and whether the turn going on was ended so
    let mut spare = false;
    let mut turn_stopped = false;
    let mut delivered = false;
    let mut tip: Option<String> = None;
    let mut last_wf_emit = Instant::now() - Duration::from_secs(1);
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

    // Idle with a workflow running: a message can go straight in (the composer sends instead of queueing).
    // Busy with one running: a Stop ends the turn only (a second one, if the turn doesn't end, everything).
    macro_rules! sync_waiting {
        () => {
            let live = !bg.is_empty() && stdin.is_some() && !interrupted;
            let was_waiting = waiting;
            if (live && idle, live && !idle && !turn_stopped) != (waiting, spare) {
                (waiting, spare) = (live && idle, live && !idle && !turn_stopped);
                if let Some(c) = st.cc_ctl.lock().get_mut(conv_id) {
                    (c.waiting, c.spare) = (waiting, spare);
                }
            }
            if waiting != was_waiting {
                let _ = app.emit("cc-waiting", json!({"conversation_id": conv_id, "waiting": waiting}));
            }
        };
    }

    // Hand the next stored message to the CLI once its turn is idle: the turn that ended is settled, the
    // message starts a turn of its own (its own bubble, cost and time), and the CLI keeps its workflows.
    macro_rules! deliver_say {
        () => {
            while idle && stdin.is_some() && !interrupted {
                let Some(mid) = pending_say.pop_front() else { break };
                settle_rows(st, conv_id, &mut ts, &result, None, std::mem::take(&mut turn_stopped), None, None)?;
                if let Some(t) = ts.rows.iter().rev().find(|r| r.meta["cc_uuid"].is_string()) {
                    tip = Some(t.id.clone());
                }
                let msg = st.db.msg_get(&mid)?;
                let upto = st.db.path_to(&mid)?;
                let from = upto.iter().rposition(|m| m.role == "user" && !render::is_tool_result_msg(m)).unwrap_or(0);
                ts = TurnState {
                    rows: vec![],
                    current: None,
                    streaming: None,
                    first_id: None,
                    siblings: json!({"index": 0, "count": 1, "ids": []}),
                    tracker: view::tracker_for(&upto[..from]),
                    last_emit: Instant::now() - Duration::from_secs(1),
                    cost_before: result.as_ref().and_then(|r| r["total_cost_usd"].as_f64()).unwrap_or(ts.cost_before),
                };
                parent = mid;
                let blocks: Vec<Value> = msg.content.as_array().cloned().unwrap_or_default().into_iter().filter(|b| !is_reminder(b)).collect();
                write_line(&mut stdin, &json!({"type": "user", "message": {"role": "user", "content": blocks}, "parent_tool_use_id": null, "session_id": "", "origin": {"kind": "human"}})).await;
                result = None;
                result_at = None;
                idle = false;
                report_due = false;
                delivered = true;
                crate::turnstream::end(conv_id);
                emit_conv_updated(app, conv_id);
            }
        };
    }

    // The event loop. Its errors (storing a row fails, say because the chat
    // was deleted mid-turn) end it like the CLI ending does, so the cleanup
    // after it always runs: open prompts taken down, the CLI stopped.
    let events: AppResult<()> = async {
        loop {
            let line = tokio::select! {
                l = lines.next_line() => l,
                _ = cancel.cancelled(), if !interrupted => {
                    interrupted = true;
                    interrupt_deadline = Some(Instant::now() + Duration::from_secs(5));
                    for id in &bg {
                        write_line(&mut stdin, &json!({"type": "control_request", "request_id": new_id(), "request": {"subtype": "stop_task", "task_id": id}})).await;
                    }
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
                Some(cmd) = ctl_rx.recv() => {
                    match cmd {
                        Cmd::StopTask(id) => {
                            write_line(&mut stdin, &json!({"type": "control_request", "request_id": new_id(), "request": {"subtype": "stop_task", "task_id": id}})).await;
                        }
                        Cmd::Say(id) => {
                            pending_say.push_back(id);
                            deliver_say!();
                            sync_waiting!();
                        }
                        // the turn may have ended meanwhile: then there is nothing left to stop
                        Cmd::StopTurn if !idle && !turn_stopped => {
                            turn_stopped = true;
                            write_line(&mut stdin, &json!({"type": "control_request", "request_id": new_id(), "request": {"subtype": "interrupt", "spare_background_tasks": true}})).await;
                            sync_waiting!();
                        }
                        Cmd::StopTurn => {}
                    }
                    continue;
                }
                _ = tokio::time::sleep(Duration::from_millis(250)), if interrupt_deadline.is_some() || result_at.is_some() => {
                    let now = Instant::now();
                    let interrupt_over = interrupt_deadline.map(|d| now > d).unwrap_or(false);
                    let result_over = result_at.map(|t| now.duration_since(t) > result_grace).unwrap_or(false);
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
                    let sub = ev["subtype"].as_str().unwrap_or("");
                    if matches!(sub, "task_started" | "task_progress" | "task_updated" | "task_notification") {
                        let changed = record_workflow(st.workflows.lock().entry(conv_id.to_string()).or_default(), &ev);
                        // progress comes several times a second: the pane gets it at a calmer pace, the ends at once
                        if changed && (sub != "task_progress" || last_wf_emit.elapsed() > Duration::from_millis(400)) {
                            last_wf_emit = Instant::now();
                            let _ = app.emit("workflows-changed", json!({"conversation_id": conv_id}));
                        }
                    }
                    let ended = track_workflows(&mut bg, &ev);
                    ran_workflow |= ended || !bg.is_empty();
                    if ended {
                        report_due = true;
                        if bg.is_empty() && idle && stdin.is_some() {
                            // the last one ended after the turn's result: wait for its report
                            // turn, which a message_start below takes over from
                            result_at = Some(Instant::now());
                            result_grace = REPORT_WAIT;
                        }
                    }
                    // the report's request is being retried (rate limit, overload): it is still coming
                    if ev["subtype"] == "api_retry" && idle && stdin.is_some() && result_grace == REPORT_WAIT && result_at.is_some() {
                        result_at = Some(Instant::now());
                    }
                }
                "stream_event" if top_level => {
                    let se = &ev["event"];
                    if se["type"] == "message_start" {
                        report_due = false;
                        if idle && stdin.is_some() {
                            // a follow-up turn (a workflow's report): read it to its own result
                            result_at = None;
                            turn_stopped = false;
                        }
                        idle = false;
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
                "control_response" => {
                    let r = &ev["response"];
                    // only `applied`: the rest of get_settings carries the settings' env values
                    let a = &r["response"]["applied"];
                    if uc_check.is_some() && r["request_id"].as_str() == uc_check.as_deref() && a["ultracodeRequested"] == true && a["ultracode"] != true {
                        let _ = app.emit("ultracode-unavailable", json!({"conversation_id": conv_id}));
                    }
                }
                "result" => {
                    result = Some(ev.clone());
                    idle = true;
                    // the spending limit ends the turn, workflows or not
                    let over_budget = ev["subtype"].as_str().is_some_and(|t| t.contains("budget"));
                    if interrupted || over_budget || (bg.is_empty() && !report_due) {
                        result_at = Some(Instant::now());
                        result_grace = Duration::from_secs(3);
                        // closing stdin lets the CLI exit
                        stdin = None;
                    } else if bg.is_empty() {
                        // a workflow ended during this turn; its report may come as one more turn
                        result_at = Some(Instant::now());
                        result_grace = REPORT_WAIT;
                    } else {
                        // workflows still run: keep stdin open (their permission prompts, and the
                        // CLI's 10-minute cap once input closes) until they report back
                        result_at = None;
                    }
                    deliver_say!();
                }
                _ => {}
            }
            sync_waiting!();
        }
        Ok(())
    }
    .await;

    // Workflows that went with the CLI.
    if let Some(list) = st.workflows.lock().get_mut(conv_id) {
        for w in list.iter_mut() {
            end_workflow(w, "killed");
        }
    }
    let _ = app.emit("workflows-changed", json!({"conversation_id": conv_id}));
    // Prompts the CLI never got an answer to.
    for (_, (req_id, _)) in open_perms.drain() {
        st.permissions.lock().remove(&req_id);
        st.pending_perms.lock().remove(&req_id);
        let _ = app.emit("permission-resolved", json!({"request_id": req_id, "conversation_id": conv_id}));
    }
    // Let the CLI exit on its own, then stop it and everything it started.
    stop_cli(child, stdin, events.is_err()).await;
    events?;
    ts.streaming = None;
    // a turn ended by a Stop that spared the workflows ends stopped too, not failed
    let stopped = interrupted || turn_stopped;

    // Bookkeeping on the last assistant row.
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
            if r["is_error"].as_bool() == Some(true) && !stopped {
                error = Some(
                    r["result"].as_str().map(|s| s.to_string()).filter(|s| !s.is_empty())
                        .or_else(|| r["errors"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n")))
                        .unwrap_or_else(|| format!("Claude Code reported an error ({}).", r["subtype"].as_str().unwrap_or("error"))),
                );
            }
        }
        None if !stopped => {
            error = Some(if err_text.is_empty() {
                "Claude Code exited unexpectedly. Run `claude` in a terminal to check that it's installed and signed in.".into()
            } else {
                format!("Claude Code exited unexpectedly:\n{}", truncate_chars(&err_text, 1500))
            });
        }
        None => {}
    }
    if error.as_deref().is_some_and(|e| e.contains("Not logged in") || e.contains("/login")) {
        error = Some("You're not signed in to Claude. Open Settings → Account and sign in with your Claude account (or add an API key).".into());
    }
    if !fresh && ts.rows.is_empty() && !interrupted && !delivered {
        let text = format!("{} {}", error.clone().unwrap_or_default(), err_text);
        if is_resume_lost(&text) {
            return Ok(Outcome::ResumeLost);
        }
    }
    // a workflow's wait is in no turn's own duration_ms: a turn that spanned one shows the wall time
    let wall_ms = result_at.filter(|_| ran_workflow && !delivered).map(|t| t.duration_since(run_start).as_millis() as u64);
    let settled = settle_rows(st, conv_id, &mut ts, &result, wall_ms, stopped, error.as_deref(), budget_stop.as_ref())?;
    if !settled {
        if let Some(e) = &error {
            if !delivered {
                return fail(e.clone());
            }
            // the error belongs under the message of the turn that failed, not the run's first one
            let _ = new_row!("assistant", json!([]), json!({"error": {"message": e, "kind": "claude_code"}, "engine": ENGINE}));
        } else if stopped {
            let _ = new_row!("assistant", json!([]), json!({"stopped": true, "engine": ENGINE}));
        }
    }
    // Remember where the CLI session ends so the next message resumes there.
    let tip_id = ts.rows.iter().rev().find(|r| r.meta["cc_uuid"].is_string()).map(|r| r.id.clone()).or(tip);
    if let Some(tip_id) = tip_id {
        update_conv_options(st, conv_id, |o| o["cc_tip"] = json!(tip_id));
    }
    if let Some(r) = ts.rows.last() {
        st.db.conv_set_leaf(conv_id, Some(&r.id))?;
    }
    emit(app, st, conv_id, &mut ts, &mcp, math, &HashMap::new(), true);
    // a message that came in as the CLI went away (it is stored, but no turn took it): its own run
    let mut orphan = pending_say.pop_back();
    while let Ok(c) = ctl_rx.try_recv() {
        if let Cmd::Say(id) = c {
            orphan = Some(id);
        }
    }
    match error {
        Some(e) => Err(AppError::Api { status: 0, kind: "claude_code".into(), message: e }),
        None => Ok(match orphan.filter(|_| !interrupted) {
            Some(id) => Outcome::Next(id),
            None => Outcome::Done,
        }),
    }
}

/// Bookkeeping on the last assistant row of a turn that ended: what the CLI's
/// result said about it (cost, time, usage), and how it ended. False when the
/// turn has no assistant row.
#[allow(clippy::too_many_arguments)]
fn settle_rows(
    st: &AppState,
    conv_id: &str,
    ts: &mut TurnState,
    result: &Option<Value>,
    wall_ms: Option<u64>,
    interrupted: bool,
    error: Option<&str>,
    budget_stop: Option<&Value>,
) -> AppResult<bool> {
    ts.streaming = None;
    let Some(i) = ts.rows.iter().rposition(|r| r.role == "assistant") else { return Ok(false) };
    let row = &mut ts.rows[i];
    if let Some(r) = result {
        if !r["usage"].is_null() {
            row.meta["usage"] = r["usage"].clone();
        }
        row.meta["cost_usd"] = turn_cost(r, ts.cost_before);
        row.meta["duration_ms"] = wall_ms.map(|ms| json!(ms)).unwrap_or_else(|| r["duration_ms"].clone());
    }
    row.meta["stop_reason"] = json!(if interrupted { Value::Null } else { json!("end_turn") });
    if interrupted {
        row.meta["stopped"] = json!(true);
    }
    if let Some(e) = error {
        row.meta["error"] = json!({"message": e, "kind": "claude_code"});
    }
    if let Some(b) = budget_stop {
        row.meta["budget_stop"] = b.clone();
    }
    st.db.msg_update(&row.id, &row.content, &row.meta)?;
    let text = render::plain_text(&row.content);
    let _ = st.db.fts_index(conv_id, &row.id, &text);
    Ok(true)
}

/// A turn's own cost, from a result whose `total_cost_usd` is what the CLI process spent so far.
fn turn_cost(result: &Value, before: f64) -> Value {
    result["total_cost_usd"].as_f64().map_or(Value::Null, |t| json!((t - before).max(0.0)))
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
        let msg = |meta: Value| Message { id: "x".into(), conversation_id: "c".into(), parent_id: None, role: "assistant".into(), content: json!([]), meta, created_at: 0 };
        assert_eq!(latest_session(&[msg(json!({"cc_session": "s1"})), msg(json!({})), msg(json!({"cc_session": "s2"}))]).as_deref(), Some("s2"));
        assert_eq!(latest_session(&[msg(json!({"cc_session": "s1"})), msg(json!({"error": {}}))]).as_deref(), Some("s1"));
        assert_eq!(latest_session(&[]), None);
        assert!(is_resume_lost("Error: No conversation found with session ID: abc"));
        assert!(is_resume_lost("No message found with message.uuid of: x"));
        assert!(!is_resume_lost("rate limited"));
    }

    #[test]
    fn effort_and_ultracode_flags() {
        let dir = std::env::temp_dir().join(format!("clauderust-flags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = |o: Value, model: &str, chat: bool| cli_session_flags(&o, model, chat, &dir);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let (uc, off) = (dir.join("claude-ultracode.json").display().to_string(), dir.join("claude-ultracode-off.json").display().to_string());
        let (uc, off) = (uc.as_str(), off.as_str());
        assert_eq!(f(json!({"effort": "xhigh"}), "claude-opus-5-5", false), s(&["--effort", "xhigh"]));
        assert_eq!(f(json!({"effort": "low"}), "claude-opus-5-5", true), s(&["--effort", "low"]));
        // stale or unknown levels are dropped
        assert!(f(json!({"effort": "xhigh"}), "claude-sonnet-4-6", false).is_empty());
        assert!(f(json!({"effort": "high"}), "claude-haiku-4-5", false).is_empty());
        assert!(f(json!({"effort": "ultracode"}), "claude-opus-5-5", false).is_empty());
        assert!(f(json!({"effort": null}), "claude-opus-5-5", false).is_empty());
        // ultracode: code sessions on a model that has it, at the chosen level
        assert_eq!(f(json!({"effort": "high", "ultracode": true}), "claude-opus-5-5", false), s(&["--effort", "high", "--settings", uc]));
        assert_eq!(f(json!({"ultracode": true}), "claude-sonnet-5-5", false), s(&["--settings", uc]));
        // off is said too (settings can turn it on for every session); unset leaves the CLI's own
        assert_eq!(f(json!({"effort": "high", "ultracode": false}), "claude-opus-5-5", false), s(&["--effort", "high", "--settings", off]));
        assert_eq!(f(json!({"effort": "high", "ultracode": null}), "claude-opus-5-5", false), s(&["--effort", "high"]));
        assert_eq!(f(json!({"effort": "high", "ultracode": "true"}), "claude-opus-5-5", false), s(&["--effort", "high"]));
        assert_eq!(f(json!({"effort": "high", "ultracode": true}), "claude-opus-5-5", true), s(&["--effort", "high"]));
        assert_eq!(f(json!({"effort": "high", "ultracode": true}), "claude-sonnet-4-6", false), s(&["--effort", "high"]));
        assert!(f(json!({"ultracode": true}), "default", false).is_empty());
        // the files the flag points at switch ultracode on and off
        assert_eq!(std::fs::read_to_string(uc).unwrap(), r#"{"ultracode":true}"#);
        assert_eq!(std::fs::read_to_string(off).unwrap(), r#"{"ultracode":false}"#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_workflow_is_recorded_from_its_events() {
        let mut list = vec![];
        let started = json!({"subtype": "task_started", "task_id": "w1", "task_type": "local_workflow", "workflow_name": "audit", "description": "Audit it"});
        assert!(record_workflow(&mut list, &started));
        // the same start twice, shells and ambient workflows add nothing
        assert!(!record_workflow(&mut list, &started));
        assert!(!record_workflow(&mut list, &json!({"subtype": "task_started", "task_id": "b1", "task_type": "local_bash"})));
        assert!(!record_workflow(&mut list, &json!({"subtype": "task_started", "task_id": "w2", "task_type": "local_workflow", "ambient": true})));
        assert_eq!(list.len(), 1);
        assert_eq!((list[0]["name"].as_str(), list[0]["status"].as_str()), (Some("audit"), Some("running")));
        let progress = json!({"subtype": "task_progress", "task_id": "w1", "usage": {"total_tokens": 900, "tool_uses": 4}, "workflow_progress": [
            {"type": "workflow_phase", "index": 1, "title": "Find"},
            {"type": "workflow_agent", "label": "find:a.js", "phaseTitle": "Find", "state": "progress", "model": "m", "tokens": 500, "toolCalls": 2, "durationMs": 1200, "lastToolName": "Read"},
            {"type": "workflow_agent", "label": "find:b.js", "phaseTitle": "Find", "state": "error", "error": "API Error: 529"},
        ]});
        assert!(record_workflow(&mut list, &progress));
        assert_eq!((list[0]["tokens"].as_i64(), list[0]["phases"][0]["title"].as_str()), (Some(900), Some("Find")));
        assert_eq!((list[0]["agents"][0]["label"].as_str(), list[0]["agents"][0]["last_tool"].as_str()), (Some("find:a.js"), Some("Read")));
        assert_eq!((list[0]["agents"][1]["state"].as_str(), list[0]["agents"][1]["error"].as_str()), (Some("error"), Some("API Error: 529")));
        // progress for a workflow we never saw start is ignored; an end sticks once
        assert!(!record_workflow(&mut list, &json!({"subtype": "task_progress", "task_id": "zz", "usage": {}})));
        assert!(record_workflow(&mut list, &json!({"subtype": "task_updated", "task_id": "w1", "patch": {"status": "completed"}})));
        assert!(record_workflow(&mut list, &json!({"subtype": "task_notification", "task_id": "w1", "status": "failed", "output_file": "o.txt"})));
        assert_eq!((list[0]["status"].as_str(), list[0]["output_file"].as_str()), (Some("completed"), Some("o.txt")));
        assert!(list[0]["finished_at"].is_number());
    }

    #[test]
    fn workflows_are_tracked_until_they_report() {
        let mut bg = HashSet::new();
        let mut ev = |e: Value| track_workflows(&mut bg, &e);
        assert!(!ev(json!({"subtype": "task_started", "task_id": "w1", "task_type": "local_workflow", "tool_use_id": "tu"})));
        // shells, watchers and remote tasks don't hold the turn
        assert!(!ev(json!({"subtype": "task_started", "task_id": "b1", "task_type": "local_bash"})));
        assert!(!ev(json!({"subtype": "task_started", "task_id": "w2", "task_type": "local_workflow", "ambient": true})));
        assert!(!ev(json!({"subtype": "task_progress", "task_id": "w1"})));
        assert!(!ev(json!({"subtype": "task_updated", "task_id": "w1", "patch": {"status": "running"}})));
        assert!(!ev(json!({"subtype": "init"})));
        assert!(ev(json!({"subtype": "task_notification", "task_id": "w1", "status": "completed"})));
        assert!(bg.is_empty());
        // the live-task set replaces what we knew
        let mut ev = |e: Value| track_workflows(&mut bg, &e);
        assert!(!ev(json!({"subtype": "background_tasks_changed", "tasks": [
            {"task_id": "w3", "task_type": "local_workflow"},
            {"task_id": "w4", "task_type": "local_workflow", "off_host": true},
            {"task_id": "b2", "task_type": "local_bash"},
        ]})));
        assert!(ev(json!({"subtype": "task_updated", "task_id": "w3", "patch": {"status": "killed"}})));
        assert!(!ev(json!({"subtype": "background_tasks_changed", "tasks": [{"task_id": "w5", "task_type": "local_workflow"}]})));
        assert!(ev(json!({"subtype": "background_tasks_changed", "tasks": []})));
        assert!(bg.is_empty());
    }

    #[test]
    fn a_turn_costs_what_came_after_the_one_before() {
        // results of one CLI process (a message typed while a workflow ran): running totals
        let cost = |total: Value, before: f64| turn_cost(&json!({"total_cost_usd": total}), before).as_f64();
        assert_eq!(cost(json!(0.03), 0.0), Some(0.03));
        assert!((cost(json!(0.153), 0.142).unwrap() - 0.011).abs() < 1e-9);
        assert_eq!(cost(json!(0.1), 0.2), Some(0.0));
        assert!(turn_cost(&json!({}), 0.1).is_null());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn stopping_the_cli_stops_its_process_group() {
        let dir = std::env::temp_dir().join(format!("cr-cc-stop-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("bg.pid");
        // like the CLI with a background shell, ignoring the end of its input
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 60 & echo $! > \"$0\"; trap '' TERM; while :; do sleep 1; done")
            .arg(&pidfile)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take();
        while !pidfile.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_string();
        let started = Instant::now();
        // the turn broke off: no waiting for the CLI to finish by itself
        stop_cli(child, stdin, true).await;
        assert!(started.elapsed() < Duration::from_secs(6), "{:?}", started.elapsed());
        let running = || std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit(')').next().is_some_and(|r| !r.trim_start().starts_with('Z')));
        for _ in 0..30 {
            if !running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!running(), "the CLI's background process {pid} survived");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn stderr_that_isnt_utf8_is_read_to_the_end() {
        // what the stderr task reads: a code-page line must not end it
        let data = b"warn: C:\\Users\\\xdeule\nlater line\n".to_vec();
        let mut r = BufReader::new(&data[..]);
        let mut got = vec![];
        while let Ok(Some((l, _))) = crate::mcp::read_line_lossy(&mut r, 8 * 1024).await {
            got.push(l);
        }
        assert_eq!(got, vec!["warn: C:\\Users\\\u{fffd}ule".to_string(), "later line".to_string()]);
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
