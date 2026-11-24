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
use crate::mcp::McpTool;
use crate::render::{self, ArtifactTracker, RenderCtx};
use crate::state::{AppState, LiveRun, PermissionReply};
use crate::util::{new_id, now_ms, truncate_chars};
use crate::view;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime};
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

fn not_installed() -> AppError {
    AppError::coded("cli_not_installed", Value::Null, "Claude Code isn't installed. Install it from claude.com/claude-code first.")
}

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
    let cli = find_cli(override_path).ok_or_else(not_installed)?;
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
    let mut child = cmd.spawn().map_err(|e| AppError::coded("cli_start_failed", json!({"path": cli}), format!("Could not start Claude Code ({}): {e}", cli.display())))?;
    let mut stderr = child.stderr.take();
    // shortcut: no way to paste the code the browser page shows (the CLI's fallback when it can't catch the redirect), so a sign-in that needs it times out; use the terminal sign-in then
    let end = tokio::select! {
        s = child.wait() => s.map(Some).map_err(AppError::from),
        _ = cancel.cancelled() => Ok(None),
        _ = tokio::time::sleep(Duration::from_secs(300)) => Err(AppError::coded("cli_sign_in_timeout", json!({"n": 5}), "Sign-in didn't finish within 5 minutes.")),
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
            Err(AppError::coded("cli_sign_in_failed", Value::Null, if text.trim().is_empty() { "Sign-in failed.".to_string() } else { truncate_chars(text.trim(), 500) }))
        }
        Ok(None) => {
            let _ = child.kill().await;
            Ok(json!({"cancelled": true}))
        }
        Err(e) => {
            let _ = child.kill().await;
            Err(e)
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
    let cli = find_cli(override_path).ok_or_else(not_installed)?;
    match cli_output(&cli, &["auth", "logout"], 20).await {
        Some(o) if o.status.success() => Ok(()),
        Some(o) => {
            let text = String::from_utf8_lossy(if o.stderr.is_empty() { &o.stdout } else { &o.stderr }).trim().to_string();
            Err(AppError::coded("cli_sign_out_failed", Value::Null, if text.is_empty() { "Sign-out failed.".to_string() } else { truncate_chars(&text, 500) }))
        }
        None => Err(AppError::coded("cli_sign_out_failed", Value::Null, "Sign-out didn't finish.")),
    }
}

/// What a title's one-shot may write. The CLI can't turn Haiku 5's thinking off (`MAX_THINKING_TOKENS=0` is
/// ignored there) and counts it in this cap: at 64 the CLI answered "exceeded the 64 output token maximum".
pub const TITLE_MAX_TOKENS: u32 = 1024;

/// A short title for a conversation that runs on the Claude account (no API key to ask): one `one_shot` call,
/// about 600 tokens in. None when the conversation doesn't run through the CLI or the call fails.
pub async fn title(st: &AppState, conv: &crate::db::Conversation, user_text: &str, assistant_text: &str) -> Option<String> {
    if !uses_cli(st, conv).await {
        return None;
    }
    let text = format!("<conversation>\nUser: {}\n\nAssistant: {}\n</conversation>", truncate_chars(user_text, 1500), truncate_chars(assistant_text, 600));
    crate::engine::clean_title(&one_shot(st, crate::prompts::TITLE_PROMPT, &text, TITLE_MAX_TOKENS, Duration::from_secs(45)).await.ok()?)
}

/// The `result` line of a `--output-format json` run: the whole output, or (with a warning line before it,
/// or as the array `--verbose` makes) the last result in it.
fn json_result(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let is_result = |v: &Value| v["type"] == "result" || v["result"].is_string();
    let all = serde_json::from_str::<Value>(text.trim()).ok().map(|v| match v {
        Value::Array(a) => a.into_iter().rev().find(is_result).unwrap_or_default(),
        v => v,
    });
    all.unwrap_or_else(|| text.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).find(is_result).unwrap_or_default())
}

/// One answer from the CLI's smallest model on the Claude account (titles, an artifact's `window.claude.complete`):
/// no tools, settings or saved session, low effort (thinking as little as the model allows), at most `max_tokens`
/// out (thinking included), `input` on stdin.
pub async fn one_shot(st: &AppState, system: &str, input: &str, max_tokens: u32, timeout: Duration) -> AppResult<String> {
    let cli = find_cli(&st.settings.read().claude_code_path).ok_or_else(|| AppError::coded("cli_not_installed", Value::Null, "Claude Code isn't installed."))?;
    let args: Vec<String> = [
        "-p", "--model", "haiku", "--effort", "low", "--output-format", "json", "--tools", "", "--strict-mcp-config", "--setting-sources", "",
        "--disable-slash-commands", "--no-session-persistence", "--system-prompt", system,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut cmd = crate::shellenv::command(&cli.to_string_lossy(), &args);
    cmd.current_dir(chat_dir(st)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID"] {
        cmd.env_remove(k);
    }
    cmd.env("MAX_THINKING_TOKENS", "0").env("CLAUDE_CODE_MAX_OUTPUT_TOKENS", max_tokens.to_string());
    let mut child = cmd.spawn().map_err(|e| AppError::coded("cli_start_failed", json!({"path": cli}), format!("Could not start Claude Code ({}): {e}", cli.display())))?;
    // what the person wrote goes in on stdin: no command-line quoting for it
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes()).await;
    }
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| AppError::coded("cli_timeout", json!({"s": timeout.as_secs()}), "Claude took too long to answer."))??;
    let v = json_result(&out.stdout);
    match v["result"].as_str() {
        Some(r) if v["is_error"] != true => Ok(r.to_string()),
        r => Err(AppError::coded("claude_code", Value::Null, format!("Claude Code couldn't answer: {}", truncate_chars(r.unwrap_or_default(), 300)))),
    }
}

/// An empty folder of ours where chat conversations run the CLI, so no
/// project's files, memory or settings are within its reach.
fn chat_dir(st: &AppState) -> String {
    let d = chat_path(&st.data_dir);
    let _ = std::fs::create_dir_all(&d);
    d.display().to_string()
}

fn chat_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("chat-cli")
}

/// The CLI sessions a chat refers to, on every branch. None for a code session: its transcripts are the
/// person's own Claude Code history, in their project's folder.
pub fn chat_sessions(db: &crate::db::Db, conv: &crate::db::Conversation) -> AppResult<HashSet<String>> {
    if conv.kind == "code" {
        return Ok(HashSet::new());
    }
    // the latest session, before its first answer is stored, is only in the options
    let mut out: HashSet<String> = conv.options["cc_session"].as_str().map(String::from).into_iter().collect();
    let nodes = db.tree_nodes(&conv.id)?;
    let parents: HashSet<&str> = nodes.iter().filter_map(|n| n.parent_id.as_deref()).collect();
    for leaf in nodes.iter().filter(|n| !parents.contains(n.id.as_str())) {
        out.extend(db.path_to(&leaf.id)?.into_iter().filter_map(|m| m.meta["cc_session"].as_str().map(String::from)));
    }
    Ok(out)
}

/// Those of `sessions` no conversation refers to any more (a fork resuming one keeps it). It reads every
/// message, so a deleted chat's cleanup runs it off the command.
pub fn unreferenced(db: &crate::db::Db, sessions: HashSet<String>) -> AppResult<Vec<String>> {
    if sessions.is_empty() {
        return Ok(vec![]);
    }
    let used = db.imported_keys()?;
    Ok(sessions.into_iter().filter(|s| !used.contains(&format!("claude-code:{s}"))).collect())
}

/// What a deleted chat leaves behind: its prompt file and the CLI transcripts of `sessions`.
pub fn remove_chat_files(data_dir: &std::path::Path, conv_id: &str, sessions: &[String]) {
    let chat = chat_path(data_dir);
    if plain_name(conv_id) {
        let _ = std::fs::remove_file(prompt_path(&chat, conv_id));
    }
    if let Some(dir) = crate::import::claude_code_dir().and_then(|c| transcripts_dir(&c, &chat)) {
        remove_transcripts(&dir, sessions);
    }
}

/// The prompt files that versions before 0.1.4 (or a crash) left behind; at start, before any turn runs.
pub fn sweep_prompt_files(data_dir: &std::path::Path) {
    let Ok(dir) = std::fs::read_dir(chat_path(data_dir)) else { return };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("system-") && name.ends_with(".txt") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// An id that is safe to join to a folder: no separators, dots or drive letters.
fn plain_name(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Where the CLI keeps the transcripts of sessions run in `cwd`: a folder named after it, each UTF-16 unit
/// but an ASCII letter or digit made a dash. shortcut: over 200 characters the CLI adds a hash of its own,
/// not matched here, so those transcripts stay; compute that hash if such data folders turn up.
fn transcripts_dir(config: &std::path::Path, cwd: &std::path::Path) -> Option<PathBuf> {
    let mut name = String::new();
    for c in cwd.to_string_lossy().chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else {
            name.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    (name.len() <= 200).then(|| config.join("projects").join(name))
}

fn remove_transcripts(dir: &std::path::Path, sessions: &[String]) {
    for s in sessions.iter().filter(|s| plain_name(s)) {
        let _ = std::fs::remove_file(dir.join(format!("{s}.jsonl")));
        // what the CLI keeps beside a transcript (sub-agents, large tool results)
        let _ = std::fs::remove_dir_all(dir.join(s));
    }
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
        Err(AppError::coded("no_terminal_app", json!({"cmd": cmd}), format!(
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
    /// how long the streaming message's thinking blocks took, for its stored blocks
    thinking: crate::engine::ThinkingClock,
    first_id: Option<String>,
    siblings: Value,
    tracker: ArtifactTracker,
    last_emit: Instant,
    /// The CLI's running total before this turn: its results report one, a resumed session's earlier turns included.
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
fn emit<R: Runtime>(app: &AppHandle<R>, st: &AppState, conv_id: &str, ts: &mut TurnState, mcp: &HashMap<String, (String, String)>, math: bool, extra_live: &HashMap<String, String>, force: bool) {
    if !force && ts.last_emit.elapsed() < crate::engine::EMIT_EVERY {
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
    /// The CLI refused a flag the app can do without (`OLD_CLIS`); nothing was stored.
    Again,
}

/// CLIs, by path, too old for `--thinking-display` (they exit on an unknown option).
/// shortcut: kept until the app restarts, so updating such a CLI gets summaries only after a restart.
static OLD_CLIS: parking_lot::Mutex<Vec<PathBuf>> = parking_lot::Mutex::new(Vec::new());

pub async fn run<R: Runtime>(app: &AppHandle<R>, st: &Arc<AppState>, conv_id: &str, user_msg_id: String, cancel: &CancellationToken) -> AppResult<()> {
    crate::shellenv::ready().await;
    let (mut id, mut fresh) = (user_msg_id, false);
    loop {
        match run_once(app, st, conv_id, id.clone(), cancel, fresh).await? {
            Outcome::Done => return Ok(()),
            // Start over in a new session that gets the earlier conversation as context.
            Outcome::ResumeLost => fresh = true,
            Outcome::Next(next) => (id, fresh) = (next, false),
            Outcome::Again => {}
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

/// The first line the CLI reads: the project files the session lacks, the earlier conversation (when a
/// new session replaces one), then the message without the API engine's reminders.
fn first_input_line(message: &Value, files: Vec<Value>, history: &str, human: bool) -> Value {
    let mut content = files;
    if !history.is_empty() {
        content.push(json!({"type": "text", "text": format!(
            "<earlier_conversation>\n{history}\n</earlier_conversation>\n\nThe Claude Code session for the conversation above is no longer available, so this continues in a new session. Use it as context for the message below."
        )}));
    }
    content.extend(message.as_array().into_iter().flatten().filter(|b| !is_reminder(b)).cloned());
    let mut line = json!({"type": "user", "message": {"role": "user", "content": content}, "parent_tool_use_id": null, "session_id": ""});
    if human {
        line["origin"] = json!({"kind": "human"});
    }
    line
}

/// The message that asks the CLI to go on with a reply that was cut off. Its session lacks what only
/// streamed in (the CLI keeps finished blocks), so the reply's last words go with it, as data.
fn continue_request(reply: &Value) -> Value {
    let text = render::plain_text(reply);
    let tail: String = text.chars().skip(text.chars().count().saturating_sub(2000)).collect();
    json!([{"type": "text", "text": format!(
        "Your previous reply was cut off. Continue it from exactly where it stopped, without repeating what it already says. It ended with:\n<reply_end>\n{}\n</reply_end>",
        tail.trim_end()
    )}])
}

/// The `fast-mode-unavailable` event for a turn that asked for fast mode and runs without it, from the CLI's
/// `init` line: off, and why (`fast_mode_disabled_reason`: no usage credits on the plan, the organisation
/// turned it off...). The turn goes on at the standard speed; a cooldown after a rate limit isn't reported.
fn fast_mode_notice(conv_id: &str, init: &Value) -> Option<Value> {
    (init["fast_mode_state"] == "off").then(|| json!({"conversation_id": conv_id, "reason": init["fast_mode_disabled_reason"]}))
}

/// The CLI's `api_retry` line as the `generation-retry` event the API engine sends (engine.rs).
fn retry_notice(conv_id: &str, ev: &Value) -> Value {
    let reason = match ev["error_status"].as_u64() {
        Some(429) => "rate_limited",
        Some(529) => "overloaded",
        Some(_) => "server_error",
        None => "network",
    };
    json!({"conversation_id": conv_id, "attempt": ev["attempt"], "max_attempts": ev["max_retries"], "wait_ms": ev["retry_delay_ms"], "reason": reason})
}

/// A project's PDFs and images for a turn's message, with the keys to note on it: all of them in a new
/// session; in a resumed one, those no message it `held` carried (its transcript keeps them, so they
/// aren't sent again), such as a file added to the project since.
fn unsent_files(blocks: Vec<Value>, held: &[Message]) -> (Vec<Value>, Vec<String>) {
    let sent: HashSet<&str> = held.iter().flat_map(|m| m.meta["cc_files"].as_array().into_iter().flatten()).filter_map(Value::as_str).collect();
    blocks
        .into_iter()
        .map(|b| {
            let key = crate::util::sha256_hex(b.to_string().as_bytes())[..16].to_string();
            (b, key)
        })
        .filter(|(_, key)| !sent.contains(key.as_str()))
        .unzip()
}

/// A chat's prompt file, removed when dropped: the CLI reads it as it starts and every turn writes it anew.
struct PromptFile(PathBuf);

impl Drop for PromptFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn prompt_path(dir: &std::path::Path, conv_id: &str) -> PathBuf {
    dir.join(format!("system-{conv_id}.txt"))
}

/// The prompt goes to the CLI by file: a project's knowledge can pass the command-line limit.
fn write_prompt(dir: &std::path::Path, conv_id: &str, system: &str) -> std::io::Result<PromptFile> {
    let file = prompt_path(dir, conv_id);
    std::fs::write(&file, system)?;
    Ok(PromptFile(file))
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
    /// "server/tool" of an app MCP tool, for an "allow always" to set its policy
    mcp_key: Option<String>,
    reply: Option<PermissionReply>,
}

fn is_resume_lost(text: &str) -> bool {
    text.contains("No conversation found with session ID") || text.contains("No message found with message.uuid")
}

/// The effort, ultracode, fast mode and thinking flags for a turn. Only a level the model has goes
/// through: one left from another model is dropped, as the built-in engine does.
/// Ultracode lives only as long as the CLI process (`--resume` doesn't bring it
/// back), so a choice goes with every spawn: on, or off over settings that turn
/// it on for every session; none leaves the CLI's own. Chats have no Workflow tool.
/// Fast mode is the CLI's only with `fastMode` in these flag settings (it says
/// `sdk_opt_in_required` otherwise). The thinking switch turned off goes as
/// `alwaysThinkingEnabled: false`, on the models where the CLI can turn it off.
pub fn cli_session_flags(options: &Value, model: &str, chat: bool, thinking_default: bool, data_dir: &std::path::Path) -> Vec<String> {
    let m = crate::models::info(model);
    let mut out = vec![];
    if let Some(e) = options["effort"].as_str().filter(|e| m.efforts.contains(e)) {
        out.extend(["--effort".to_string(), e.to_string()]);
    }
    let mut flags = serde_json::Map::new();
    if let Some(on) = options["ultracode"].as_bool().filter(|_| !chat && m.ultracode) {
        flags.insert("ultracode".into(), json!(on));
    }
    if options["fast"] == true && m.fast_price.is_some() {
        flags.insert("fastMode".into(), json!(true));
    }
    if !options["thinking"].as_bool().unwrap_or(thinking_default) && m.thinking_switch_cli {
        flags.insert("alwaysThinkingEnabled".into(), json!(false));
    }
    if !flags.is_empty() {
        out.extend(["--settings".to_string(), flag_settings_file(data_dir, &Value::Object(flags)).display().to_string()]);
    }
    out
}

/// A name in the characters the CLI keeps in an MCP tool's name (`mcp__<server>__<tool>`); it replaces the rest.
pub(crate) fn cli_safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// The app's MCP tools as a chat on the account offers them: SDK-hosted servers the CLI reaches through us
/// (`initialize`, then `mcp_message`), relayed to the app's own connections. Each is (server, tool, the app's
/// tool), named in the CLI's characters, so the CLI calls it `mcp__<server>__<tool>`.
type ChatTools = Vec<(String, String, McpTool)>;

fn chat_tools(tools: Vec<McpTool>) -> ChatTools {
    let mut out: ChatTools = vec![];
    for t in tools {
        let (s, n) = (cli_safe(&t.server), cli_safe(&t.name));
        // shortcut: of two tools whose names differ only in characters the CLI replaces, the second is left out
        if !out.iter().any(|(a, b, _)| *a == s && *b == n) {
            out.push((s, n, t));
        }
    }
    out
}

/// The tools a chat's SDK-hosted servers carry, and with "load tools when needed" (tool_load.rs) those of its MCP
/// tools not loaded yet (API names), which aren't listed or run until `load_tools` loads them.
#[derive(Default)]
struct SdkTools {
    tools: ChatTools,
    /// the chat's MCP tools (API names): what `load_tools` can load
    mcp: HashSet<String>,
    unloaded: parking_lot::Mutex<HashSet<String>>,
    /// how many times the CLI has listed a server's tools, woken each time
    lists: std::sync::atomic::AtomicUsize,
    listed: tokio::sync::Notify,
}

impl From<ChatTools> for SdkTools {
    fn from(tools: ChatTools) -> Self {
        SdkTools { tools, ..Default::default() }
    }
}

/// The app's own tools (past chats, project search, `load_tools`): read-only, run without asking.
fn app_tool(api_name: &str) -> bool {
    crate::past_chats::NAMES.contains(&api_name) || crate::project_search::is_tool(api_name) || api_name == crate::tool_load::LOAD_TOOL
}

/// What a chat on the account switches on in the CLI besides its prompt: web search and fetch while the
/// conversation's web search is on, and its MCP tools; none of Claude Code's own tools, skills, MCP servers,
/// settings or hooks. The second list runs without asking: web search and the app's own tools (as on the API), and
/// the MCP tools set to Allow in Settings or allowed for this chat. Web fetch and the other MCP tools ask; Block
/// ones aren't offered.
fn chat_flags(web: bool, tools: &ChatTools, policies: &serde_json::Map<String, Value>, chat_allowed: &[String]) -> (Vec<String>, Vec<String>) {
    let flags = ["--tools", if web { "WebSearch,WebFetch" } else { "" }, "--disable-slash-commands", "--strict-mcp-config", "--setting-sources", ""]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut allowed: Vec<String> = if web { vec!["WebSearch".into()] } else { vec![] };
    for (s, n, t) in tools {
        if app_tool(&t.api_name) || policies.get(&format!("{}/{}", t.server, t.name)).and_then(Value::as_str) == Some("allow") || chat_allowed.contains(&t.api_name) {
            allowed.push(format!("mcp__{s}__{n}"));
        }
    }
    (flags, allowed)
}

/// The SDK-hosted `server`'s answer to a JSON-RPC message from the CLI; `None` for a tool call, which the
/// app's connection to the real server answers.
fn sdk_mcp_reply(tools: &SdkTools, server: &str, msg: &Value) -> Option<Value> {
    let result = match msg["method"].as_str().unwrap_or("") {
        "tools/call" => return None,
        "initialize" => json!({"protocolVersion": msg["params"]["protocolVersion"], "capabilities": {"tools": {"listChanged": true}}, "serverInfo": {"name": server, "version": env!("CARGO_PKG_VERSION")}}),
        "tools/list" => {
            let unloaded = tools.unloaded.lock();
            let list: Vec<Value> = tools.tools.iter().filter(|(s, _, t)| s == server && !unloaded.contains(&t.api_name)).map(|(_, n, t)| json!({"name": n, "description": t.description, "inputSchema": t.input_schema})).collect();
            tools.lists.fetch_add(1, Ordering::SeqCst);
            tools.listed.notify_waiters();
            json!({"tools": list})
        }
        // a notification (no id) is answered with an empty result
        m if m == "ping" || msg["id"].is_null() => json!({}),
        m => return Some(json!({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32601, "message": format!("Method not found: {m}")}})),
    };
    Some(json!({"jsonrpc": "2.0", "id": msg["id"], "result": result}))
}

/// `load_tools` on the account engine: the tools it loads are kept in the conversation (as on the API engine) and
/// listed from now on. The CLI is told its servers' lists changed, and the answer waits until it has listed them
/// again (it then offers them in the very next request of the turn; Claude Code 2.1 re-lists some 300 ms after).
async fn load_sdk_tools(st: &AppState, conv_id: &str, tools: &SdkTools, args: &Value, to_cli: &tokio::sync::mpsc::UnboundedSender<Value>) -> (String, bool) {
    let available: Vec<&McpTool> = tools.tools.iter().map(|(.., t)| t).filter(|t| tools.mcp.contains(&t.api_name)).collect();
    let (text, failed, new) = crate::tool_load::load(st, conv_id, &available, args);
    let mut servers: Vec<&str> = tools.tools.iter().filter(|(.., t)| new.contains(&t.api_name)).map(|(s, ..)| s.as_str()).collect();
    servers.sort_unstable();
    servers.dedup();
    let target = tools.lists.load(Ordering::SeqCst) + servers.len();
    tools.unloaded.lock().retain(|n| !new.contains(n));
    for s in &servers {
        let _ = to_cli.send(json!({"type": "control_request", "request_id": new_id(), "request": {"subtype": "mcp_message", "server_name": s, "message": {"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}}}));
    }
    let relisted = async {
        loop {
            let wake = tools.listed.notified();
            if tools.lists.load(Ordering::SeqCst) >= target {
                break;
            }
            wake.await;
        }
    };
    // a CLI that doesn't list them again has them from the next turn on
    let _ = tokio::time::timeout(Duration::from_secs(5), relisted).await;
    (text, failed)
}

/// A tool call the CLI sent to an SDK-hosted server from `conv_id`'s turn, run on the app's MCP connection (the
/// app's own tools: by the app itself) and answered in MCP's terms.
async fn sdk_mcp_call(st: &AppState, conv_id: &str, tools: &SdkTools, server: &str, msg: &Value, to_cli: &tokio::sync::mpsc::UnboundedSender<Value>) -> Value {
    let name = msg["params"]["name"].as_str().unwrap_or("");
    let args = &msg["params"]["arguments"];
    let failed = |text: String| (vec![json!({"type": "text", "text": text})], true);
    let text = |(text, failed): (String, bool)| (vec![json!({"type": "text", "text": text})], failed);
    let (blocks, is_error) = match tools.tools.iter().find(|(s, n, _)| s == server && n == name) {
        Some((.., t)) if crate::past_chats::NAMES.contains(&t.api_name.as_str()) => crate::past_chats::run(st, conv_id, &t.name, args),
        Some((.., t)) if crate::project_search::is_tool(&t.api_name) => match st.db.conv_get(conv_id).ok().and_then(|c| c.project_id) {
            Some(pid) => text(crate::project_search::run(st.db.clone(), pid, t.name.clone(), args.clone()).await),
            None => failed("This chat isn't in a project.".into()),
        },
        Some((.., t)) if t.api_name == crate::tool_load::LOAD_TOOL => text(load_sdk_tools(st, conv_id, tools, args, to_cli).await),
        Some((.., t)) if tools.unloaded.lock().contains(&t.api_name) => failed(format!("Load {name} with {} first.", crate::tool_load::LOAD_TOOL)),
        Some((.., t)) => st.mcp.call(&t.api_name, args).await.unwrap_or_else(|e| failed(format!("Error: {e}"))),
        None => failed(format!("Tool {name} is not available.")),
    };
    json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"content": mcp_content(&blocks), "isError": is_error}})
}

/// The app's MCP results (tool_result blocks: text and images) back in MCP's content shape.
fn mcp_content(blocks: &[Value]) -> Vec<Value> {
    blocks
        .iter()
        .map(|b| match b["type"].as_str() {
            Some("image") => json!({"type": "image", "data": b["source"]["data"], "mimeType": b["source"]["media_type"]}),
            _ => json!({"type": "text", "text": b["text"].as_str().unwrap_or_default()}),
        })
        .collect()
}

fn mcp_response(request_id: &Value, reply: Value) -> Value {
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"mcp_response": reply}}})
}

/// The settings file for a turn's flag settings, one per set of them (a handful). `--settings` takes a path or
/// inline JSON; a path survives the quoting of cmd.exe and of a terminal
/// line, which turn the JSON's quotes into something else.
fn flag_settings_file(data_dir: &std::path::Path, flags: &Value) -> PathBuf {
    let body = flags.to_string();
    let p = data_dir.join(format!("claude-flags-{}.json", &crate::util::sha256_hex(body.as_bytes())[..12]));
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
async fn run_once<R: Runtime>(
    app: &AppHandle<R>,
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
    // a turn started from a reply that was cut off goes on with it (send_message's continue_reply)
    let continuing = user.role == "assistant";
    // the messages the CLI is to hold already: those before the new message, or the whole reply to go on with
    let before = if continuing { path.len() } else { path.len().saturating_sub(1) };
    // a chat conversation: plain chat on the Claude account, in a folder of ours
    let chat = conv.kind != "code";
    let cwd = if chat { chat_dir(st) } else { conv.cwd.clone().unwrap_or_default() };

    let fail = |e: AppError| -> AppResult<Outcome> {
        let m = Message {
            id: new_id(),
            conversation_id: conv_id.to_string(),
            parent_id: Some(user_msg_id.clone()),
            role: "assistant".into(),
            content: json!([]),
            meta: json!({"error": error_meta(&e)}),
            created_at: now_ms(),
        };
        st.db.msg_insert(&m)?;
        st.db.conv_set_leaf(conv_id, Some(&m.id))?;
        Err(e)
    };

    let Some(cli) = find_cli(&settings.claude_code_path) else {
        return fail(AppError::coded("cli_not_installed", Value::Null, "Claude Code isn't installed (or wasn't found). Install it from claude.com/claude-code, sign in once by running `claude` in a terminal, then try again. You can also set its path in Settings → Code."));
    };
    if !std::path::Path::new(&cwd).is_dir() {
        return fail(AppError::coded("folder_not_found", json!({"path": cwd}), format!("The project folder no longer exists: {cwd}")));
    }

    // Resume the CLI session at the right point in our tree.
    let prev_at = path[..before].iter().rposition(|m| m.meta["cc_uuid"].is_string());
    let mode = conv.options["permission_mode"].as_str().unwrap_or("default").to_string();
    let mut args: Vec<String> = [
        "-p", "--output-format", "stream-json", "--verbose", "--input-format", "stream-json",
        "--include-partial-messages", "--permission-prompt-tool=stdio", "--permission-mode",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // Auto mode is ours: the CLI asks (default mode) and we answer safe
    // requests ourselves. The CLI writes files itself: whatever changed since
    // the conversation began counts as written by it.
    args.push(if mode == "auto" { "default".to_string() } else { mode.clone() });
    let auto_since = crate::codetools::CodeSession::conversation_start(conv.created_at);
    if !conv.model.trim().is_empty() && conv.model != "default" {
        args.push("--model".into());
        args.push(conv.model.clone());
    }
    if let Some(fb) = crate::models::overload_fallback(&conv.model, &st.settings.read().overload_fallback) {
        args.push("--fallback-model".into());
        args.push(fb);
    }
    args.extend(cli_session_flags(&conv.options, &conv.model, chat, !chat || settings.thinking_default, &st.data_dir));
    // without it a print-mode CLI gets the API's default for newer models: thinking with no text ("omitted")
    if !OLD_CLIS.lock().contains(&cli) {
        args.extend(["--thinking-display".to_string(), "summarized".to_string()]);
    }
    let ultracode_asked = conv.options["ultracode"] == true && !chat && crate::models::info(&conv.model).ultracode;
    let fast_asked = conv.options["fast"] == true && crate::models::info(&conv.model).fast_price.is_some();
    let opt_strs = |key: &str| -> Vec<String> { conv.options[key].as_array().into_iter().flatten().filter_map(|d| d.as_str().map(String::from)).collect() };
    let mut sdk_tools: Arc<SdkTools> = Arc::default();
    let mut chat_allowed: Vec<String> = vec![];
    // a project's PDFs and images, and the chat prompt file (kept until the turn ends)
    let mut project_files: Vec<Value> = vec![];
    let mut _prompt_file: Option<PromptFile> = None;
    if chat {
        // our chat prompt (instructions, style, project knowledge) replaces the coding one
        let rc = crate::engine::build_request_context(st, &conv, &settings).await?;
        let file = match write_prompt(std::path::Path::new(&cwd), conv_id, &rc.system) {
            Ok(f) => f,
            Err(e) => return fail(AppError::coded("prompt_write_failed", json!({"path": cwd}), format!("Could not write the chat prompt (in {cwd}): {e}"))),
        };
        project_files = rc.prefix_blocks;
        // past chats while searching them is on, first (of two tools that end up with one name the first stays), then
        // the conversation's servers and tools, as the request context leaves them (disabled servers and Block out)
        let mut all = if settings.chat_search && !conv.incognito { crate::past_chats::mcp_tools() } else { vec![] };
        // a project too large for the prompt: its index is in the prompt file, its search a tool of ours
        let offered = |name: &str| rc.tools.iter().any(|t| t["name"] == name);
        if offered(crate::project_search::SEARCH_TOOL) {
            all.extend(crate::project_search::mcp_tools());
        }
        // tools loaded when needed: the request context offers `load_tools` and leaves the others' schemas out
        if offered(crate::tool_load::LOAD_TOOL) {
            all.extend(crate::tool_load::mcp_tools());
        }
        all.extend(st.mcp.tools().await.into_iter().filter(|t| rc.mcp_names.contains_key(&t.api_name)));
        let tools = chat_tools(all);
        let web = conv.options["web_search"].as_bool().unwrap_or(settings.web_search_default);
        let (f, allowed) = chat_flags(web, &tools, &settings.tool_policies, &opt_strs("allowed_tools"));
        args.extend(f);
        chat_allowed = allowed;
        let unloaded = rc.mcp_names.keys().filter(|n| !offered(n)).cloned().collect();
        sdk_tools = Arc::new(SdkTools { tools, mcp: rc.mcp_names.keys().cloned().collect(), unloaded: parking_lot::Mutex::new(unloaded), ..Default::default() });
        args.push("--system-prompt-file".into());
        args.push(file.0.display().to_string());
        _prompt_file = Some(file);
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
    let mut with_history = prev_at.is_none() && path.len() > 1;
    let mut cost_before = 0.0;
    // the messages the CLI session already holds: those up to where it resumes
    let mut held: &[Message] = &[];
    if let Some(i) = prev_at {
        let p = &path[i];
        if let Some(sid) = p.meta["cc_session"].as_str() {
            // (cc_replay: changed since the session holds it, an artifact edited in place)
            if fresh || p.meta["cc_replay"] == true || !transcript_exists(sid) {
                with_history = true;
            } else {
                held = &path[..=i];
                args.push("--resume".into());
                args.push(sid.to_string());
                let at_tip = conv.options["cc_tip"].as_str() == Some(p.id.as_str());
                cost_before = resumed_cost(at_tip, &p.meta, || restored_cost(sid));
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
    let dirs = opt_strs("cc_add_dirs");
    if !dirs.is_empty() {
        args.push("--add-dir".into());
        args.extend(dirs);
    }
    let mut rules = opt_strs("cc_allowed_tools");
    rules.extend(chat_allowed);
    if !rules.is_empty() {
        args.push("--allowedTools".into());
        args.extend(rules);
    }
    // the conversation's switched-off servers and tools (`server__tool`), as the API engine leaves them out: a chat's
    // aren't in its tool list either, a code session's are those of the CLI's own MCP servers
    let mut off: Vec<String> = opt_strs("disabled_servers").iter().map(|s| format!("mcp__{}", cli_safe(s))).collect();
    off.extend(opt_strs("disabled_tools").iter().map(|t| format!("mcp__{t}")));
    if !off.is_empty() {
        args.push("--disallowedTools".into());
        args.extend(off);
    }

    let history = if with_history { history_context(&path[..path.len().saturating_sub(1)]) } else { String::new() };
    let (files, file_keys) = unsent_files(project_files, held);
    // A person typed it: the CLI then honours what only a person may type, such as the "ultracode"
    // keyword. Not over a replayed conversation, whose earlier words aren't this message's.
    let message = if continuing { continue_request(&user.content) } else { user.content.clone() };
    let first_line = first_input_line(&message, files, &history, !chat && !with_history && !continuing);
    if !file_keys.is_empty() {
        let mut meta = user.meta.clone();
        meta["cc_files"] = json!(file_keys);
        st.db.msg_update(&user_msg_id, &user.content, &meta)?;
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
        Err(e) => return fail(AppError::coded("cli_start_failed", json!({"path": cli}), format!("Could not start Claude Code ({}): {e}", cli.display()))),
    };
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let err_tail: Arc<parking_lot::Mutex<VecDeque<String>>> = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let err_reader = {
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
        })
    };
    // the line to this run: Stop for one workflow, a message while one runs
    let (ctl_tx, mut ctl_rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    st.cc_ctl.lock().insert(conv_id.to_string(), Ctl { tx: ctl_tx, waiting: false, spare: false });
    let _ctl = CtlGuard { st: st.clone(), conv_id: conv_id.to_string() };
    let mut servers: Vec<&str> = sdk_tools.tools.iter().map(|(s, ..)| s.as_str()).collect();
    servers.sort_unstable();
    servers.dedup();
    if !servers.is_empty() {
        write_line(&mut stdin, &json!({"type": "control_request", "request_id": new_id(), "request": {"subtype": "initialize", "sdkMcpServers": servers}})).await;
    }
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
        thinking: Default::default(),
        first_id: None,
        siblings: json!({"index": 0, "count": 1, "ids": []}),
        tracker: view::tracker_for(&path[..turn_start]),
        last_emit: Instant::now() - Duration::from_secs(1),
        cost_before,
    };
    if continuing {
        // the reply goes on, no longer cut off: what comes is shown and settled with it
        let mut reply = st.db.msg_get(&user_msg_id)?;
        if let Some(m) = reply.meta.as_object_mut() {
            m.remove("error");
            m.remove("stopped");
        }
        st.db.msg_update(&reply.id, &reply.content, &reply.meta)?;
        ts.siblings = view::siblings_of(&st.db.tree_nodes(conv_id)?, &reply.id);
        ts.first_id = Some(reply.id.clone());
        ts.rows.push(reply);
    }
    let seeded = ts.rows.len();
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
    // answers to the CLI from tasks that run beside the event loop (MCP tool calls)
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
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
                    thinking: Default::default(),
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
                Some(line) = out_rx.recv() => {
                    write_line(&mut stdin, &line).await;
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
            if let Some(info) = crate::usage::info_of(&ev) {
                crate::usage::record(app, st, info);
            }
            let top_level = ev["parent_tool_use_id"].is_null();
            match ev["type"].as_str().unwrap_or("") {
                "system" => {
                    if ev["subtype"] == "init" {
                        if let Some(sid) = ev["session_id"].as_str() {
                            session_id = Some(sid.to_string());
                            let sid = sid.to_string();
                            update_conv_options(st, conv_id, |o| o["cc_session"] = json!(sid));
                        }
                        if let Some(n) = fast_mode_notice(conv_id, &ev).filter(|_| fast_asked) {
                            let _ = app.emit("fast-mode-unavailable", n);
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
                    if sub == "api_retry" {
                        let _ = app.emit("generation-retry", retry_notice(conv_id, &ev));
                        // the report's request is being retried (rate limit, overload): it is still coming
                        if idle && stdin.is_some() && result_grace == REPORT_WAIT && result_at.is_some() {
                            result_at = Some(Instant::now());
                        }
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
                        ts.thinking = Default::default();
                    } else if let Some((_, acc)) = ts.streaming.as_mut() {
                        if let Ok(changed) = acc.apply(se) {
                            ts.thinking.saw(&acc.blocks, changed);
                            ts.thinking.stamp(&mut acc.blocks);
                        }
                        if se["type"] == "message_stop" {
                            ts.streaming = None;
                        }
                    }
                    emit(app, st, conv_id, &mut ts, &mcp, math, &awaiting, false);
                }
                "assistant" if top_level => {
                    let msg = &ev["message"];
                    let api_id = msg["id"].as_str().unwrap_or("").to_string();
                    let mut blocks = msg["content"].as_array().cloned().unwrap_or_default();
                    let idx = match &ts.current {
                        Some((id, i)) if *id == api_id => *i,
                        _ => {
                            let i = new_row!("assistant", json!([]), json!({"model": msg["model"], "engine": ENGINE}));
                            ts.current = Some((api_id.clone(), i));
                            i
                        }
                    };
                    // the CLI writes each block as it ends, in order: its thinking blocks take the stream's times in turn
                    let had = ts.rows[idx].content.as_array().map_or(0, |c| c.iter().filter(|b| b["type"] == "thinking").count());
                    for (b, ms) in blocks.iter_mut().filter(|b| b["type"] == "thinking").zip(ts.thinking.durations().into_iter().skip(had)) {
                        b["thinking_ms"] = json!(ms);
                    }
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
                        let mcp_tool = sdk_tools.tools.iter().find(|(s, n, _)| tool == format!("mcp__{s}__{n}")).map(|(.., t)| t);
                        let kind = match tool.as_str() {
                            "ExitPlanMode" => "plan",
                            "AskUserQuestion" => "question",
                            _ if mcp_tool.is_some() => "mcp",
                            _ => "code",
                        };
                        // Auto mode: approve safe requests without asking.
                        let mode_now = st.db.conv_get(conv_id).ok().and_then(|c| c.options["permission_mode"].as_str().map(String::from)).unwrap_or_default();
                        if mode_now == "auto" && tool != "ExitPlanMode" {
                            let mut sess = crate::codetools::CodeSession::new(PathBuf::from(&cwd));
                            // the CLI's Bash is bash on Windows too, where `\` escapes (`.\./x` is `../x`)
                            sess.powershell = false;
                            sess.started = auto_since;
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
                            "server": mcp_tool.map(|t| &t.server), "tool_name": mcp_tool.map(|t| &t.name),
                        });
                        let mcp_key = mcp_tool.map(|t| format!("{}/{}", t.server, t.name));
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
                            let _ = tx.send(PermDone { cli_request_id: rid, req_id, tool, input, tool_use_id, suggestions, mcp_key, reply });
                        });
                    } else if req["subtype"] == "mcp_message" {
                        let server = req["server_name"].as_str().unwrap_or("").to_string();
                        let msg = req["message"].clone();
                        match sdk_mcp_reply(&sdk_tools, &server, &msg) {
                            Some(r) => write_line(&mut stdin, &mcp_response(&rid, r)).await,
                            // a tool call can take minutes: beside the loop, which keeps answering
                            None => {
                                let (st, tools, tx, cancel, conv_id) = (st.clone(), sdk_tools.clone(), out_tx.clone(), cancel.clone(), conv_id.to_string());
                                tokio::spawn(async move {
                                    tokio::select! {
                                        r = sdk_mcp_call(&st, &conv_id, &tools, &server, &msg, &tx) => { let _ = tx.send(mcp_response(&rid, r)); }
                                        _ = cancel.cancelled() => {}
                                    }
                                });
                            }
                        }
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
    // what it last wrote (why it exited, a lost session) is read before it's looked at; a process it left
    // running can keep the pipe open
    let _ = tokio::time::timeout(Duration::from_secs(1), err_reader).await;
    events?;
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
            budget_stop = Some(json!({"budget": crate::engine::turn_budget(&conv, &st.settings.read()), "spent": turn_cost(r, ts.cost_before)}));
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
    let error = error.map(|e| turn_error(e, &st.usage_limits.lock()));
    // a CLI from before --thinking-display refuses to start with it: once more without it, from now on
    if ts.rows.len() == seeded && !interrupted && !delivered && err_text.contains("--thinking-display") && !OLD_CLIS.lock().contains(&cli) {
        OLD_CLIS.lock().push(cli);
        // nothing ran: the session's tip is where it was
        let tip = conv.options["cc_tip"].clone();
        update_conv_options(st, conv_id, |o| o["cc_tip"] = tip);
        return Ok(Outcome::Again);
    }
    if !fresh && ts.rows.len() == seeded && !interrupted && !delivered {
        let text = format!("{} {}", error.as_ref().map(|e| e.to_string()).unwrap_or_default(), err_text);
        if is_resume_lost(&text) {
            return Ok(Outcome::ResumeLost);
        }
    }
    // a workflow's wait is in no turn's own duration_ms: a turn that spanned one shows the wall time
    let wall_ms = result_at.filter(|_| ran_workflow && !delivered).map(|t| t.duration_since(run_start).as_millis() as u64);
    let settled = settle_rows(st, conv_id, &mut ts, &result, wall_ms, stopped, error.as_ref(), budget_stop.as_ref())?;
    if !settled {
        match error {
            Some(e) if !delivered => return fail(e),
            // the error belongs under the message of the turn that failed, not the run's first one
            Some(ref e) => {
                let _ = new_row!("assistant", json!([]), json!({"error": error_meta(e), "engine": ENGINE}));
            }
            None if stopped => {
                let _ = new_row!("assistant", json!([]), json!({"stopped": true, "engine": ENGINE}));
            }
            None => {}
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
        Some(e) => Err(e),
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
    error: Option<&AppError>,
    budget_stop: Option<&Value>,
) -> AppResult<bool> {
    // what streamed in of a message the CLI never finished (it exited, or a Stop) is kept
    ts.rows = ts.display_rows();
    ts.streaming = None;
    let Some(i) = ts.rows.iter().rposition(|r| r.role == "assistant") else { return Ok(false) };
    // the turn's earlier answers are complete too: with no stop_reason, the next start would take them for cut off
    for row in ts.rows[..i].iter_mut().filter(|r| r.role == "assistant" && r.meta["stop_reason"].is_null()) {
        let ends_in_tool = row.content.as_array().and_then(|a| a.last()).is_some_and(|b| b["type"] == "tool_use");
        row.meta["stop_reason"] = json!(if ends_in_tool { "tool_use" } else { "end_turn" });
        st.db.msg_update(&row.id, &row.content, &row.meta)?;
    }
    let row = &mut ts.rows[i];
    if let Some(r) = result {
        // a budget stop reports no usage (all zeros): the streamed one stays
        if r["usage"]["output_tokens"].as_u64().is_some_and(|n| n > 0) {
            row.meta["usage"] = r["usage"].clone();
        }
        row.meta["cost_usd"] = turn_cost(r, ts.cost_before);
        // the session's running total, which the next resume starts from
        row.meta["cc_cost_total"] = r["total_cost_usd"].clone();
        row.meta["duration_ms"] = wall_ms.map(|ms| json!(ms)).unwrap_or_else(|| r["duration_ms"].clone());
    }
    // unfinished when stopped, or when the CLI went away mid-reply (as a dropped stream leaves it on the API)
    let cut_off = interrupted || (result.is_none() && error.is_some());
    row.meta["stop_reason"] = if cut_off { Value::Null } else { json!("end_turn") };
    if interrupted {
        row.meta["stopped"] = json!(true);
    }
    if let Some(e) = error {
        row.meta["error"] = error_meta(e);
        // the CLI's own words for the failure (a synthetic "Not logged in · Please run /login" reply): the error says it
        if result.as_ref().is_some_and(|r| echoes_error(&row.content, r)) {
            row.content = json!([]);
        }
    }
    if let Some(b) = budget_stop {
        row.meta["budget_stop"] = b.clone();
    }
    st.db.msg_update(&row.id, &row.content, &row.meta)?;
    // each answer of the turn is searched and its artifacts go to the gallery, as the API engine does with its messages
    for r in ts.rows.iter().filter(|r| r.role == "assistant") {
        let text = render::plain_text(&r.content);
        let _ = st.db.fts_index(conv_id, &r.id, &text);
        crate::engine::save_artifacts(st, conv_id, &r.id, &text);
    }
    Ok(true)
}

/// A failed turn as the UI says it (ui/js/errors.js): coded by what the CLI said, the usage limit with when it resets.
fn turn_error(message: String, limits: &Value) -> AppError {
    let lower = message.to_lowercase();
    if message.contains("Not logged in") || message.contains("/login") {
        return AppError::coded("cli_not_signed_in", Value::Null, "You're not signed in to Claude. Open Settings → Account and sign in with your Claude account (or add an API key).");
    }
    if lower.contains("usage limit") || lower.contains("hit your limit") {
        let key = limits["limiting"].as_str();
        let resets = limits["limits"].as_array().into_iter().flatten().find(|l| key.is_some() && l["key"].as_str() == key).map(|l| l["resets_at"].clone());
        return AppError::coded("cli_usage_limit", resets.filter(|r| r.is_number()).map_or(Value::Null, |r| json!({"resets_at": r})), message);
    }
    if lower.contains("credit balance is too low") {
        return AppError::coded("credit_low", Value::Null, message);
    }
    if is_resume_lost(&message) {
        return AppError::coded("cli_session_lost", Value::Null, message);
    }
    AppError::coded("claude_code", Value::Null, message)
}

/// A failure as a row's meta keeps it: what the UI reads to say it (`error.rs` serializes the same fields).
fn error_meta(e: &AppError) -> Value {
    let mut v = json!({"message": e.to_string(), "kind": e.kind()});
    let params = e.params();
    if !params.is_null() {
        v["params"] = params;
    }
    v
}

/// Whether a reply is only the text of the error its turn ended with.
fn echoes_error(content: &Value, result: &Value) -> bool {
    let said = result["result"].as_str().map(str::trim).unwrap_or("");
    result["is_error"] == true && !said.is_empty() && render::plain_text(content).trim() == said
}

/// A turn's own cost, from a result whose `total_cost_usd` is the session's running total so far.
fn turn_cost(result: &Value, before: f64) -> Value {
    result["total_cost_usd"].as_f64().map_or(Value::Null, |t| json!((t - before).max(0.0)))
}

/// The running total a resumed session starts from: the one stored with the session's tip, else what the
/// CLI restores from its transcript (a fork restores the session's latest total, not the fork point's;
/// rows from before the total was stored have none), else none known.
fn resumed_cost(at_tip: bool, tip_meta: &Value, transcript: impl FnOnce() -> Option<f64>) -> f64 {
    tip_meta["cc_cost_total"].as_f64().filter(|_| at_tip).or_else(transcript).unwrap_or(0.0)
}

/// The running total in a session's transcript: its last cost record, which the CLI writes as a run ends.
fn restored_cost(session_id: &str) -> Option<f64> {
    let file = format!("{session_id}.jsonl");
    let projects = std::fs::read_dir(crate::import::claude_code_dir()?.join("projects")).ok()?;
    let path = projects.flatten().map(|p| p.path().join(&file)).find(|p| p.is_file())?;
    transcript_cost(&path)
}

fn transcript_cost(path: &std::path::Path) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    // shortcut: only the last 256 KB are read; a run that died after writing more than that falls back to none
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(256 * 1024))).ok()?;
    let mut tail = vec![];
    f.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    let rec = text.lines().rev().filter(|l| l.contains("\"cost-state\"")).filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["type"] == "cost-state")?;
    rec["totalCostUSD"].as_f64()
}

/// AskUserQuestion's input with the person's answers, as the CLI takes them: question text → the option
/// picked, several comma-separated, or what they typed. `message` is the question card's JSON object;
/// anything in it that isn't text is dropped (the CLI refuses the whole answer over it).
fn answered(input: &Value, message: Option<&str>) -> Value {
    let picked: Value = message.and_then(|m| serde_json::from_str(m).ok()).unwrap_or_default();
    let answers: serde_json::Map<String, Value> = picked.as_object().into_iter().flatten().filter(|(_, v)| v.is_string()).map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut out = input.clone();
    if let Some(o) = out.as_object_mut() {
        o.insert("answers".into(), Value::Object(answers));
    }
    out
}

/// The CLI's answer for a permission prompt the user (or Stop) resolved.
fn permission_response<R: Runtime>(app: &AppHandle<R>, st: &Arc<AppState>, conv_id: &str, done: &PermDone) -> Value {
    let decision = done.reply.as_ref().map(|r| r.decision.as_str());
    let input = &done.input;
    let mut response = match decision {
        Some("once") | Some("approve") => json!({"behavior": "allow", "updatedInput": input}),
        Some("answer") if done.tool == "AskUserQuestion" => json!({"behavior": "allow", "updatedInput": answered(input, done.reply.as_ref().and_then(|r| r.message.as_deref()))}),
        Some("session") | Some("chat") | Some("always") => {
            if let (Some("always"), Some(key)) = (decision, &done.mcp_key) {
                let mut s = st.settings.write();
                s.tool_policies.insert(key.clone(), json!("allow"));
                if let Err(e) = s.save(&st.db) {
                    log::warn!("saving the tool policy failed: {e}");
                }
                drop(s);
                // the settings window shows (and saves) its own copy
                let _ = app.emit("settings-updated", ());
            }
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
            let mut msg = String::from(if done.tool == "AskUserQuestion" { "The user dismissed the question without answering." } else { "The user denied this tool call." });
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
    fn a_settled_turn_leaves_no_answer_looking_cut_off() {
        let dir = std::env::temp_dir().join(format!("cr-cc-settle-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        let conv = st.db.conv_create("code", "m", None, None, &json!({})).unwrap();
        let row = |role: &str, content: Value, meta: Value| Message { id: new_id(), conversation_id: conv.id.clone(), parent_id: None, role: role.into(), content, meta, created_at: 1 };
        let rows = vec![
            row("assistant", json!([{"type": "text", "text": "Reading"}, {"type": "tool_use", "id": "t", "name": "Read", "input": {}}]), json!({"engine": ENGINE})),
            row("user", json!([{"type": "tool_result", "tool_use_id": "t", "content": "ok"}]), json!({"tool_results": true})),
            row("assistant", json!([{"type": "thinking", "thinking": "hm"}]), json!({"engine": ENGINE})),
            row("assistant", json!([]), json!({"stop_reason": "max_tokens"})),
            row("assistant", json!([{"type": "text", "text": "Done"}]), json!({"engine": ENGINE})),
        ];
        for r in &rows {
            st.db.msg_insert(r).unwrap();
        }
        let mut ts = TurnState { rows, current: None, streaming: None, thinking: Default::default(), first_id: None, siblings: json!({}), tracker: Default::default(), last_emit: Instant::now(), cost_before: 0.0 };
        assert!(settle_rows(&st, &conv.id, &mut ts, &None, None, false, None, None).unwrap());
        let reasons: Vec<Value> = ts.rows.iter().map(|r| st.db.msg_get(&r.id).unwrap().meta["stop_reason"].clone()).collect();
        assert_eq!(reasons, vec![json!("tool_use"), Value::Null, json!("end_turn"), json!("max_tokens"), json!("end_turn")]);
        assert_eq!(st.db.mark_interrupted().unwrap(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_turn_is_coded_by_what_the_cli_said() {
        let limits = json!({"limiting": "five_hour", "limits": [{"key": "seven_day", "resets_at": 1}, {"key": "five_hour", "resets_at": 1_795_536_000_000i64}]});
        let coded = |m: &str, l: &Value| serde_json::to_value(turn_error(m.into(), l)).unwrap();
        let out = coded("Not logged in · Please run /login", &Value::Null);
        assert_eq!(out["kind"], "cli_not_signed_in");
        assert!(out["message"].as_str().unwrap().starts_with("You're not signed in"), "{out}");
        assert_eq!(coded("Claude AI usage limit reached|1795536000", &limits)["params"], json!({"resets_at": 1_795_536_000_000i64}));
        let unknown = coded("You've hit your limit · resets 5pm", &json!({"limiting": "opus"}));
        assert_eq!((unknown["kind"].as_str(), unknown.get("params")), (Some("cli_usage_limit"), None));
        assert_eq!(coded("Credit balance is too low", &Value::Null)["kind"], "credit_low");
        assert_eq!(coded("No conversation found with session ID: x", &Value::Null)["kind"], "cli_session_lost");
        let other = coded("API Error: 500", &Value::Null);
        assert_eq!((other["kind"].as_str(), other["message"].as_str()), (Some("claude_code"), Some("API Error: 500")));
        assert_eq!(error_meta(&turn_error("x".into(), &Value::Null)), json!({"message": "x", "kind": "claude_code"}));

        let said = json!({"is_error": true, "result": "Not logged in · Please run /login"});
        assert!(echoes_error(&json!([{"type": "text", "text": "Not logged in · Please run /login\n"}]), &said));
        assert!(!echoes_error(&json!([{"type": "text", "text": "Here is the answer"}]), &said));
        assert!(!echoes_error(&json!([{"type": "text", "text": "Done"}]), &json!({"is_error": false, "result": "Done"})));
        assert!(!echoes_error(&json!([]), &json!({"is_error": true, "result": ""})));

        // the reply that only repeats the error goes; the error stays under it
        let dir = std::env::temp_dir().join(format!("cr-cc-echo-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        let conv = st.db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let reply = Message { id: new_id(), conversation_id: conv.id.clone(), parent_id: None, role: "assistant".into(), content: json!([{"type": "text", "text": "Not logged in · Please run /login"}]), meta: json!({"engine": ENGINE}), created_at: 1 };
        st.db.msg_insert(&reply).unwrap();
        let mut ts = TurnState { rows: vec![reply.clone()], current: None, streaming: None, thinking: Default::default(), first_id: None, siblings: json!({}), tracker: Default::default(), last_emit: Instant::now(), cost_before: 0.0 };
        let e = turn_error("Not logged in · Please run /login".into(), &Value::Null);
        assert!(settle_rows(&st, &conv.id, &mut ts, &Some(said), None, false, Some(&e), None).unwrap());
        let stored = st.db.msg_get(&reply.id).unwrap();
        assert_eq!((stored.content, stored.meta["error"]["kind"].as_str()), (json!([]), Some("cli_not_signed_in")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn effort_ultracode_fast_and_thinking_flags() {
        let dir = std::env::temp_dir().join(format!("clauderust-flags-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        // a code session's thinking is on unless switched off, a chat's off (the defaults the engines use)
        let f = |o: Value, model: &str, chat: bool| cli_session_flags(&o, model, chat, !chat, &dir);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // the flag settings a turn gets: none, or the file `--settings` points at, last
        let settings = |args: &[String]| -> Value {
            match args.iter().position(|a| a == "--settings") {
                Some(i) => {
                    assert_eq!(i + 2, args.len(), "{args:?}");
                    serde_json::from_str(&std::fs::read_to_string(&args[i + 1]).unwrap()).unwrap()
                }
                None => Value::Null,
            }
        };
        assert_eq!(f(json!({"effort": "xhigh"}), "claude-opus-5-5", false), s(&["--effort", "xhigh"]));
        assert_eq!(f(json!({"effort": "low"}), "claude-opus-5-5", true), s(&["--effort", "low"]));
        // stale or unknown levels are dropped
        assert!(f(json!({"effort": "xhigh"}), "claude-sonnet-4-6", false).is_empty());
        assert!(f(json!({"effort": "high"}), "claude-haiku-4-5", false).is_empty());
        assert!(f(json!({"effort": "ultracode"}), "claude-opus-5-5", false).is_empty());
        assert!(f(json!({"effort": null}), "claude-opus-5-5", false).is_empty());
        // ultracode: code sessions on a model that has it, at the chosen level
        let a = f(json!({"effort": "high", "ultracode": true}), "claude-opus-5-5", false);
        assert_eq!((&a[..2], settings(&a)), (&s(&["--effort", "high"])[..], json!({"ultracode": true})));
        assert_eq!(settings(&f(json!({"ultracode": true}), "claude-sonnet-5-5", false)), json!({"ultracode": true}));
        // off is said too (settings can turn it on for every session); unset leaves the CLI's own
        assert_eq!(settings(&f(json!({"effort": "high", "ultracode": false}), "claude-opus-5-5", false)), json!({"ultracode": false}));
        for (o, model, chat) in [
            (json!({"effort": "high", "ultracode": null}), "claude-opus-5-5", false), (json!({"effort": "high", "ultracode": "true"}), "claude-opus-5-5", false),
            (json!({"effort": "high", "ultracode": true}), "claude-opus-5-5", true), (json!({"effort": "high", "ultracode": true}), "claude-sonnet-4-6", false),
        ] {
            assert_eq!(f(o, model, chat), s(&["--effort", "high"]));
        }
        assert!(f(json!({"ultracode": true}), "default", false).is_empty());
        // fast mode: switched on, on a model that has it, in chats and code sessions alike
        assert_eq!(settings(&f(json!({"fast": true}), "claude-opus-5-5", true)), json!({"fastMode": true}));
        assert_eq!(settings(&f(json!({"fast": true, "ultracode": true}), "claude-opus-5-5", false)), json!({"fastMode": true, "ultracode": true}));
        assert!(f(json!({"fast": true}), "claude-sonnet-5-5", false).is_empty());
        assert!(f(json!({"fast": "yes"}), "claude-opus-5-5", false).is_empty());
        // thinking off: only where the CLI can turn it off; the switch left alone keeps the engine's default
        assert_eq!(settings(&f(json!({"thinking": false}), "claude-opus-5", false)), json!({"alwaysThinkingEnabled": false}));
        assert_eq!(settings(&f(json!({}), "claude-opus-4-8", true)), json!({"alwaysThinkingEnabled": false}));
        assert!(f(json!({}), "claude-opus-4-8", false).is_empty());
        assert!(f(json!({"thinking": true}), "claude-opus-4-8", true).is_empty());
        for model in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-2"] {
            assert!(f(json!({"thinking": false}), model, true).is_empty(), "{model}");
        }
        // one file per set of flag settings, kept as it is
        let one = f(json!({"fast": true}), "claude-opus-5-5", true);
        assert_eq!(one, f(json!({"fast": true}), "claude-opus-5-5", false));
        assert_ne!(one, f(json!({"fast": true, "ultracode": false}), "claude-opus-5-5", false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tool(server: &str, name: &str) -> McpTool {
        McpTool {
            server: server.into(), name: name.into(), api_name: crate::mcp::api_tool_name(server, name), title: None,
            description: format!("{name} of {server}"), input_schema: json!({"type": "object"}), read_only: false,
        }
    }

    #[test]
    fn a_chat_gets_web_search_and_its_mcp_tools_but_no_coding_tools() {
        let tools = chat_tools(vec![tool("weather", "get_forecast"), tool("my files", "read.text"), tool("my_files", "read_text"), tool("db", "query")]);
        // named in the CLI's characters; a name that only differs in replaced characters keeps the first tool
        let names: Vec<(&str, &str, &str)> = tools.iter().map(|(s, n, t)| (s.as_str(), n.as_str(), t.server.as_str())).collect();
        assert_eq!(names, vec![("weather", "get_forecast", "weather"), ("my_files", "read_text", "my files"), ("db", "query", "db")]);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let mut policies = serde_json::Map::new();
        policies.insert("weather/get_forecast".into(), json!("allow"));
        policies.insert("db/query".into(), json!("ask"));
        let (flags, allowed) = chat_flags(true, &tools, &policies, &[]);
        assert_eq!(flags, s(&["--tools", "WebSearch,WebFetch", "--disable-slash-commands", "--strict-mcp-config", "--setting-sources", ""]));
        // web search and Allow tools run without asking; web fetch and Ask tools ask
        assert_eq!(allowed, s(&["WebSearch", "mcp__weather__get_forecast"]));
        // web search off: no web tools at all; a tool allowed for this chat (by its API name) runs too
        let (flags, allowed) = chat_flags(false, &tools, &policies, &s(&["my_files__read_text", "db__other"]));
        assert_eq!(flags, s(&["--tools", "", "--disable-slash-commands", "--strict-mcp-config", "--setting-sources", ""]));
        assert_eq!(allowed, s(&["mcp__weather__get_forecast", "mcp__my_files__read_text"]));
        assert!(chat_flags(false, &vec![], &serde_json::Map::new(), &[]).1.is_empty());
        // past-chat search runs without asking, as on the API; another server of that name asks as any other
        let mut all = crate::past_chats::mcp_tools();
        all.push(tool(crate::past_chats::SERVER, "query"));
        let (_, allowed) = chat_flags(false, &chat_tools(all), &serde_json::Map::new(), &[]);
        assert_eq!(allowed, s(&["mcp__past_chats__conversation_search", "mcp__past_chats__recent_chats"]));
    }

    #[test]
    fn the_sdk_servers_answer_the_cli() {
        let tools: SdkTools = chat_tools(vec![tool("weather", "get_forecast"), tool("db", "query")]).into();
        let init = sdk_mcp_reply(&tools, "weather", &json!({"jsonrpc": "2.0", "id": "a:0", "method": "initialize", "params": {"protocolVersion": "2025-11-25"}})).unwrap();
        assert_eq!((init["id"].as_str(), init["result"]["protocolVersion"].as_str(), init["result"]["serverInfo"]["name"].as_str()), (Some("a:0"), Some("2025-11-25"), Some("weather")));
        assert!(init["result"]["capabilities"]["tools"].is_object());
        let list = sdk_mcp_reply(&tools, "weather", &json!({"id": 1, "method": "tools/list"})).unwrap();
        assert_eq!(list["result"]["tools"], json!([{"name": "get_forecast", "description": "get_forecast of weather", "inputSchema": {"type": "object"}}]));
        // a notification and a ping get an empty result, an unknown request an error; a tool call is the connection's
        assert_eq!(sdk_mcp_reply(&tools, "weather", &json!({"method": "notifications/initialized"})).unwrap()["result"], json!({}));
        assert_eq!(sdk_mcp_reply(&tools, "weather", &json!({"id": 2, "method": "ping"})).unwrap()["result"], json!({}));
        assert_eq!(sdk_mcp_reply(&tools, "weather", &json!({"id": 3, "method": "resources/list"})).unwrap()["error"]["code"], -32601);
        assert!(sdk_mcp_reply(&tools, "weather", &json!({"id": 4, "method": "tools/call", "params": {"name": "get_forecast"}})).is_none());
        let r = mcp_response(&json!("req1"), json!({"id": 1}));
        assert_eq!((r["type"].as_str(), r["response"]["request_id"].as_str(), r["response"]["response"]["mcp_response"]["id"].as_i64()), (Some("control_response"), Some("req1"), Some(1)));
        assert_eq!(
            mcp_content(&[json!({"type": "text", "text": "sunny"}), json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBO"}})]),
            vec![json!({"type": "text", "text": "sunny"}), json!({"type": "image", "data": "iVBO", "mimeType": "image/png"})]
        );
    }

    #[tokio::test]
    async fn an_sdk_tool_call_goes_to_the_apps_connection() {
        let dir = std::env::temp_dir().join(format!("cr-cc-sdk-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        let mut all = crate::past_chats::mcp_tools();
        // a connected server that happens to have the name: its tools are its own
        all.extend([tool("weather", "get_forecast"), tool(crate::past_chats::SERVER, "query")]);
        let tools: SdkTools = chat_tools(all).into();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // a tool the conversation has, whose server isn't connected (any more): the connection's error
        for (server, name) in [("weather", "get_forecast"), (crate::past_chats::SERVER, "query")] {
            let r = sdk_mcp_call(&st, "c1", &tools, server, &json!({"id": 7, "method": "tools/call", "params": {"name": name, "arguments": {}}}), &tx).await;
            assert_eq!((r["id"].as_i64(), r["result"]["isError"].as_bool()), (Some(7), Some(true)));
            assert!(r["result"]["content"][0]["text"].as_str().unwrap().starts_with("Error: "), "{r}");
        }
        // one it doesn't have (another server's, or Blocked): never called
        let r = sdk_mcp_call(&st, "c1", &tools, "db", &json!({"id": 8, "params": {"name": "get_forecast"}}), &tx).await;
        assert_eq!(r["result"], json!({"content": [{"type": "text", "text": "Tool get_forecast is not available."}], "isError": true}));
        // past chats: the app answers (here: there are none)
        let r = sdk_mcp_call(&st, "c1", &tools, crate::past_chats::SERVER, &json!({"id": 9, "params": {"name": "recent_chats", "arguments": {}}}), &tx).await;
        assert_eq!(r["result"], json!({"content": [{"type": "text", "text": "No previous chats found."}], "isError": false}));
        // a title is asked of the CLI only for a conversation that runs on it (the call can go in a spawned task)
        let conv = st.db.conv_create("chat", &format!("{}llama3", crate::oai::PREFIX), None, None, &json!({})).unwrap();
        let ask = title(&st, &conv, "hi", "hello");
        fn spawnable<T: Send>(_: &T) {}
        spawnable(&ask);
        assert_eq!(ask.await, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The installed CLI, with no model call: a chat's flags, and its SDK-hosted servers answered as a turn does.
    /// `cargo test --no-default-features a_real_cli_connects_the_chat_tools -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn a_real_cli_connects_the_chat_tools() {
        crate::shellenv::ready().await;
        let cli = find_cli("").expect("Claude Code isn't installed");
        let mut all = crate::past_chats::mcp_tools();
        all.push(tool("weather", "get_forecast"));
        let tools: SdkTools = chat_tools(all).into();
        let (flags, allowed) = chat_flags(true, &tools.tools, &serde_json::Map::new(), &[]);
        let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--input-format", "stream-json", "--permission-prompt-tool=stdio"].map(String::from).to_vec();
        args.extend(flags);
        args.push("--allowedTools".into());
        args.extend(allowed);
        let mut cmd = crate::shellenv::command(&cli.to_string_lossy(), &args);
        cmd.current_dir(std::env::temp_dir()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID"] {
            cmd.env_remove(k);
        }
        let mut child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take();
        write_line(&mut stdin, &json!({"type": "control_request", "request_id": "i", "request": {"subtype": "initialize", "sdkMcpServers": [crate::past_chats::SERVER, "weather"]}})).await;
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let status = tokio::time::timeout(Duration::from_secs(60), async {
            while let Ok(Some(l)) = lines.next_line().await {
                let ev: Value = serde_json::from_str(&l).unwrap_or_default();
                let req = &ev["request"];
                if req["subtype"] == "mcp_message" {
                    let reply = sdk_mcp_reply(&tools, req["server_name"].as_str().unwrap_or(""), &req["message"]).unwrap();
                    write_line(&mut stdin, &mcp_response(&ev["request_id"], reply)).await;
                    if req["message"]["method"] == "tools/list" {
                        write_line(&mut stdin, &json!({"type": "control_request", "request_id": "s", "request": {"subtype": "mcp_status"}})).await;
                    }
                } else if ev["response"]["request_id"] == "s" {
                    // a server is listed once its connection settles, a moment after the handshake
                    if ev["response"]["response"]["mcpServers"].as_array().is_some_and(|a| a.len() == 2 && a.iter().all(|s| s["status"] == "connected")) {
                        return ev["response"]["response"].clone();
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    write_line(&mut stdin, &json!({"type": "control_request", "request_id": "s", "request": {"subtype": "mcp_status"}})).await;
                }
            }
            Value::Null
        })
        .await
        .expect("the CLI didn't report its MCP servers");
        stop_cli(child, stdin, true).await;
        let mut servers: Vec<(String, Vec<String>)> = status["mcpServers"]
            .as_array()
            .unwrap()
            .iter()
            .inspect(|s| assert_eq!(s["transport"], "sdk", "{status}"))
            .map(|s| (s["name"].as_str().unwrap().to_string(), s["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()))
            .collect();
        servers.sort();
        let names = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(servers, [("past_chats".to_string(), names(&["conversation_search", "recent_chats"])), ("weather".to_string(), names(&["get_forecast"]))], "{status}");
    }

    /// The installed CLI, signed in, writes a title: one Haiku call of about $0.0002.
    /// `cargo test --no-default-features a_real_cli_writes_a_title -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn a_real_cli_writes_a_title() {
        let dir = std::env::temp_dir().join(format!("cr-cc-title-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        st.settings.write().chat_engine = "claude-code".into();
        let conv = st.db.conv_create("chat", "claude-sonnet-5-5", None, None, &json!({})).unwrap();
        let t = title(&st, &conv, "How do I make soft-boiled eggs for breakfast?", "Boil water, cook the eggs for six minutes, then cool them.").await;
        let _ = std::fs::remove_dir_all(dir);
        let t = t.expect("no title");
        assert!((1..=10).contains(&t.split_whitespace().count()), "{t}");
    }

    #[test]
    fn answers_go_into_the_question_input() {
        let input = json!({"questions": [{"question": "Colour?", "header": "Colour", "options": [{"label": "Red"}, {"label": "Blue"}], "multiSelect": false}]});
        let out = answered(&input, Some(r#"{"Colour?": "Blue", "Fruit?": "Apple, Plum", "bad": 3, "worse": ["x"]}"#));
        assert_eq!(out["answers"], json!({"Colour?": "Blue", "Fruit?": "Apple, Plum"}));
        assert_eq!(out["questions"], input["questions"]);
        for m in [None, Some("not json"), Some("[1]")] {
            assert_eq!(answered(&input, m)["answers"], json!({}));
        }
        assert_eq!(answered(&json!([1]), Some(r#"{"a": "b"}"#)), json!([1]));
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

    #[test]
    fn a_resumed_run_starts_from_the_sessions_running_total() {
        let tip = json!({"cc_cost_total": 0.0168});
        // at the session's tip: the stored total, the transcript left unread
        assert_eq!(resumed_cost(true, &tip, || panic!("read the transcript")), 0.0168);
        // a fork: the CLI restores the session's latest total, which the fork point's row doesn't have
        assert_eq!(resumed_cost(false, &tip, || Some(0.0578)), 0.0578);
        // a row from before the total was stored
        assert_eq!(resumed_cost(true, &json!({"cost_usd": 0.01}), || Some(0.03)), 0.03);
        // neither known
        assert_eq!(resumed_cost(true, &json!({}), || None), 0.0);
        assert_eq!(resumed_cost(false, &tip, || None), 0.0);
    }

    #[test]
    fn the_transcripts_last_cost_record_is_its_running_total() {
        let dir = std::env::temp_dir().join(format!("cr-cc-cost-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rec = |t: f64| format!("{}\n", json!({"type": "cost-state", "sessionId": "s", "totalCostUSD": t}));
        let filler = format!("{}\n", json!({"type": "user", "message": {"content": "x".repeat(1000)}})).repeat(300);
        let file = |name: &str, text: String| {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            transcript_cost(&p)
        };
        // a message that talks about cost records isn't one
        let talk = format!("{}\n", json!({"type": "user", "message": {"content": "cost-state"}}));
        assert_eq!(file("a.jsonl", format!("{}{}{}{talk}", rec(0.0017), filler, rec(0.0028))), Some(0.0028));
        // longer than what is read: the first line read is cut, the records near the end still found
        assert_eq!(file("b.jsonl", format!("{}{}{}", rec(0.0017), filler, rec(0.0058))), Some(0.0058));
        // none near the end (a run that died after writing much more), or none at all
        assert_eq!(file("c.jsonl", format!("{}{}", rec(0.0017), filler)), None);
        assert_eq!(file("d.jsonl", talk), None);
        assert_eq!(transcript_cost(&dir.join("missing.jsonl")), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_resumed_turn_is_stored_with_its_own_cost() {
        let dir = std::env::temp_dir().join(format!("cr-cc-turncost-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        let conv = st.db.conv_create("code", "m", None, None, &json!({})).unwrap();
        let streamed = json!({"input_tokens": 2, "output_tokens": 74});
        let row = Message { id: new_id(), conversation_id: conv.id.clone(), parent_id: None, role: "assistant".into(), content: json!([]), meta: json!({"engine": ENGINE, "usage": streamed}), created_at: 1 };
        st.db.msg_insert(&row).unwrap();
        let mut ts = TurnState { rows: vec![row], current: None, streaming: None, thinking: Default::default(), first_id: None, siblings: json!({}), tracker: Default::default(), last_emit: Instant::now(), cost_before: 0.00514374 };
        // what the CLI reported at a budget stop on a resumed session: the running total, and no usage
        let r = json!({"subtype": "error_max_budget_usd", "total_cost_usd": 0.00578797, "usage": {"input_tokens": 0, "output_tokens": 0}});
        assert!(settle_rows(&st, &conv.id, &mut ts, &Some(r), None, false, None, None).unwrap());
        let meta = st.db.msg_get(&ts.rows[0].id).unwrap().meta;
        assert!((meta["cost_usd"].as_f64().unwrap() - 0.00064423).abs() < 1e-12);
        assert_eq!(meta["cc_cost_total"], json!(0.00578797));
        assert_eq!(meta["usage"], streamed);
        // a result with usage replaces the streamed one
        let r = json!({"total_cost_usd": 0.006, "usage": {"input_tokens": 4, "output_tokens": 77}});
        assert!(settle_rows(&st, &conv.id, &mut ts, &Some(r), None, false, None, None).unwrap());
        assert_eq!(st.db.msg_get(&ts.rows[0].id).unwrap().meta["usage"]["output_tokens"], json!(77));
        let _ = std::fs::remove_dir_all(dir);
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

    /// A project's text file goes in the chat's prompt file, its PDFs and images in the first line the CLI
    /// reads, once per session.
    #[tokio::test]
    async fn project_files_reach_a_cli_chat_once() {
        let dir = std::env::temp_dir().join(format!("cr-cc-proj-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        let p = st.db.project_create("Launch", "").unwrap();
        let file = |name: &str, mime: &str, kind: &str, data: &str, at: i64| crate::db::ProjectFile {
            id: new_id(), project_id: p.id.clone(), name: name.into(), mime: mime.into(), size: data.len() as i64, kind: kind.into(), data: data.into(), created_at: at,
        };
        st.db.project_file_add(&file("notes.md", "text/markdown", "text", "The launch is on Friday.", 1)).unwrap();
        st.db.project_file_add(&file("brief.pdf", "application/pdf", "pdf", "JVBERi0xLjQK", 2)).unwrap();
        let conv = st.db.conv_create("chat", "claude-opus-5-5", Some(&p.id), None, &json!({})).unwrap();
        let settings = crate::settings::Settings::default();
        let rc = crate::engine::build_request_context(&st, &conv, &settings).await.unwrap();

        // the prompt file: the text file, not the PDF; gone once the turn is over
        let f = write_prompt(&dir, &conv.id, &rc.system).unwrap();
        let written = std::fs::read_to_string(&f.0).unwrap();
        assert!(written.contains("<document name=\"notes.md\">\nThe launch is on Friday.") && !written.contains("JVBERi0"), "{written}");
        let path = f.0.clone();
        drop(f);
        assert!(!path.exists());

        // the first line: the PDF as a document block, then the message
        let (files, keys) = unsent_files(rc.prefix_blocks.clone(), &[]);
        let line = first_input_line(&json!([{"type": "text", "text": "When is the launch?"}]), files, "", false);
        let content = line["message"]["content"].as_array().unwrap();
        assert_eq!(content[0], json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQK"}, "title": "brief.pdf"}));
        assert_eq!(content[1], json!({"type": "text", "text": "When is the launch?"}));
        assert_eq!((content.len(), line["type"].as_str(), line["origin"].is_null(), keys.len()), (2, Some("user"), true, 1));

        // a resumed session holds it, so it isn't sent again; a file added to the project since is
        let m = |role: &str, meta: Value| Message { id: new_id(), conversation_id: conv.id.clone(), parent_id: None, role: role.into(), content: json!([]), meta, created_at: 0 };
        let held = [m("user", json!({"cc_files": keys})), m("assistant", json!({"cc_uuid": "u1", "cc_session": "s1"}))];
        assert!(unsent_files(rc.prefix_blocks.clone(), &held).0.is_empty());
        st.db.project_file_add(&file("chart.png", "image/png", "image", "iVBORw0KGgo=", 3)).unwrap();
        let rc = crate::engine::build_request_context(&st, &conv, &settings).await.unwrap();
        let (files, keys) = unsent_files(rc.prefix_blocks.clone(), &held);
        assert_eq!(files, vec![json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}})]);
        assert_eq!(keys.len(), 1);

        // a new session in place of a lost one: every file, the earlier conversation, then the message
        let (files, _) = unsent_files(rc.prefix_blocks, &[]);
        let line = first_input_line(&json!([{"type": "text", "text": "<system-reminder>r</system-reminder>"}, {"type": "text", "text": "And the venue?"}]), files, "User: When is the launch?", true);
        let content = line["message"]["content"].as_array().unwrap();
        assert_eq!(content.iter().map(|b| b["type"].as_str().unwrap()).collect::<Vec<_>>(), ["document", "image", "text", "text"]);
        assert!(content[2]["text"].as_str().unwrap().starts_with("<earlier_conversation>\nUser: When is the launch?\n</earlier_conversation>"));
        assert_eq!((content[3]["text"].as_str(), line["origin"]["kind"].as_str()), (Some("And the venue?"), Some("human")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cli_retries_are_announced_like_the_api_engines() {
        let ev = |status: Value| json!({"type": "system", "subtype": "api_retry", "attempt": 2, "max_retries": 10, "retry_delay_ms": 1500, "error_status": status});
        assert_eq!(
            retry_notice("c1", &ev(json!(429))),
            json!({"conversation_id": "c1", "attempt": 2, "max_attempts": 10, "wait_ms": 1500, "reason": "rate_limited"})
        );
        assert_eq!(retry_notice("c1", &ev(json!(529)))["reason"], "overloaded");
        assert_eq!(retry_notice("c1", &ev(json!(503)))["reason"], "server_error");
        assert_eq!(retry_notice("c1", &ev(Value::Null))["reason"], "network");
    }

    #[test]
    fn fast_mode_the_cli_cant_give_is_reported() {
        // init lines as the installed CLI (2.1.326) printed them: without the opt-in, with it, on a model without it
        let off = json!({"type": "system", "subtype": "init", "fast_mode_state": "off", "fast_mode_disabled_reason": "sdk_opt_in_required"});
        assert_eq!(fast_mode_notice("c1", &off), Some(json!({"conversation_id": "c1", "reason": "sdk_opt_in_required"})));
        assert_eq!(fast_mode_notice("c1", &json!({"subtype": "init", "fast_mode_state": "off"})).unwrap()["reason"], Value::Null);
        for state in ["on", "cooldown"] {
            assert_eq!(fast_mode_notice("c1", &json!({"subtype": "init", "fast_mode_state": state})), None);
        }
        // an older CLI that doesn't say
        assert_eq!(fast_mode_notice("c1", &json!({"subtype": "init"})), None);
    }

    /// Deleting a chat gives up the CLI sessions only it referred to, on every branch: a fork resuming one
    /// keeps it, and a code session's are the person's own.
    #[test]
    fn a_deleted_chat_gives_up_only_its_own_sessions() {
        let db = crate::db::Db::open_in_memory().unwrap();
        let add = |conv: &str, parent: Option<&str>, meta: Value| {
            let m = Message { id: new_id(), conversation_id: conv.into(), parent_id: parent.map(Into::into), role: "assistant".into(), content: json!([]), meta, created_at: 0 };
            db.msg_insert(&m).unwrap();
            m
        };
        let delete = |conv: &crate::db::Conversation| {
            let sessions = chat_sessions(&db, conv).unwrap();
            db.conv_delete(&conv.id).unwrap();
            let mut gone = unreferenced(&db, sessions).unwrap();
            gone.sort();
            gone
        };
        // the latest session, before its first answer is stored, is only in the options
        let a = db.conv_create("chat", "claude-opus-5-5", None, None, &json!({"cc_session": "s4"})).unwrap();
        let u = add(&a.id, None, json!({}));
        let r1 = add(&a.id, Some(&u.id), json!({"cc_uuid": "x1", "cc_session": "s1"}));
        add(&a.id, Some(&r1.id), json!({"cc_uuid": "x2", "cc_session": "s2"}));
        // a regenerated answer, on a branch of its own
        add(&a.id, Some(&u.id), json!({"cc_uuid": "x3", "cc_session": "s3"}));
        let b = db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap();
        db.copy_path(&[u, r1], &b.id, true).unwrap();
        assert_eq!(delete(&a), ["s2", "s3", "s4"]);
        assert!(db.conv_get(&a.id).is_err());
        assert_eq!(delete(&b), ["s1"]);

        let c = db.conv_create("code", "claude-opus-5-5", None, Some("/p"), &json!({"cc_session": "s9"})).unwrap();
        add(&c.id, None, json!({"cc_uuid": "x9", "cc_session": "s9"}));
        assert!(delete(&c).is_empty());
        // a chat that never ran on the CLI: nothing to read
        let d = db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap();
        add(&d.id, None, json!({}));
        assert!(delete(&d).is_empty());
    }

    #[test]
    fn a_deleted_chats_files_go_by_plain_name_only() {
        let cfg = std::env::temp_dir().join(format!("cr-cc-cfg-{}", new_id()));
        // the CLI's folder for the chats of a dev copy, as found on disk
        let chat = std::path::Path::new(r"C:\Users\x\AppData\Roaming\dev.clauderust.designdev\chat-cli");
        assert_eq!(transcripts_dir(&cfg, chat), Some(cfg.join("projects").join("C--Users-x-AppData-Roaming-dev-clauderust-designdev-chat-cli")));
        // a character outside the BMP is two UTF-16 units, so two dashes
        assert_eq!(transcripts_dir(&cfg, std::path::Path::new("/a\u{1F600}b")), Some(cfg.join("projects").join("-a--b")));
        assert_eq!(transcripts_dir(&cfg, std::path::Path::new(&"x".repeat(201))), None);

        let dir = cfg.join("projects").join("p");
        std::fs::create_dir_all(dir.join("s1").join("subagents")).unwrap();
        for f in ["s1.jsonl", "s2.jsonl", "keep.jsonl"] {
            std::fs::write(dir.join(f), "{}").unwrap();
        }
        std::fs::write(cfg.join("outside.jsonl"), "{}").unwrap();
        remove_transcripts(&dir, &["s1".into(), "s2".into(), "missing".into(), "../../outside".into(), String::new()]);
        assert!(!dir.join("s1.jsonl").exists() && !dir.join("s1").exists() && !dir.join("s2.jsonl").exists());
        assert!(dir.join("keep.jsonl").exists() && cfg.join("outside.jsonl").exists());

        // the prompt file of a deleted chat (none of its sessions: nothing in the real Claude Code folder is touched)
        let data = cfg.join("data");
        std::fs::create_dir_all(chat_path(&data)).unwrap();
        std::fs::write(chat_path(&data).join("system-c1.txt"), "x").unwrap();
        std::fs::write(data.join("y.txt"), "x").unwrap();
        remove_chat_files(&data, "c1", &[]);
        remove_chat_files(&data, "x/../../y", &[]);
        assert!(!chat_path(&data).join("system-c1.txt").exists() && data.join("y.txt").exists());
        // at start, the prompt files of any chat go; nothing else there does
        for f in ["system-a.txt", "system-b.txt", "notes.txt"] {
            std::fs::write(chat_path(&data).join(f), "x").unwrap();
        }
        sweep_prompt_files(&data);
        let left: Vec<_> = std::fs::read_dir(chat_path(&data)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["notes.txt"]);
        let _ = std::fs::remove_dir_all(cfg);
    }
}
