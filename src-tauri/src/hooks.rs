//! Hooks: shell commands run at points of an agent turn, configured in
//! Claude Code's settings format (`hooks` in `~/.claude/settings.json`,
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

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

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

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// Hash of a project's hook configuration (what the user trusts).
pub fn project_hash(root: &Path) -> Option<(String, Vec<String>)> {
    let mut found = HashMap::new();
    for name in ["settings.json", "settings.local.json"] {
        if let Some(v) = read_json(&root.join(".claude").join(name)) {
            parse(&v, &mut found);
        }
    }
    if found.is_empty() {
        return None;
    }
    let mut cmds: Vec<String> = found.iter().flat_map(|(e, hs)| hs.iter().map(move |h| format!("{e} [{}]: {}", h.matcher, h.command))).collect();
    cmds.sort();
    let hash = hex::encode(Sha256::digest(cmds.join("\n").as_bytes()));
    Some((hash, cmds))
}

/// The hooks that apply in `cwd`. `trusted` maps project roots to the
/// trusted configuration hash.
pub fn load(cwd: &Path, trusted: &HashMap<String, String>) -> Config {
    let mut cfg = Config::default();
    if let Some(home) = dirs::home_dir() {
        if let Some(v) = read_json(&home.join(".claude").join("settings.json")) {
            parse(&v, &mut cfg.events);
        }
    }
    let root = crate::worktree::repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    if let Some((hash, cmds)) = project_hash(&root) {
        let key = root.to_string_lossy().to_string();
        if trusted.get(&key) == Some(&hash) {
            for name in ["settings.json", "settings.local.json"] {
                if let Some(v) = read_json(&root.join(".claude").join(name)) {
                    parse(&v, &mut cfg.events);
                }
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
    if let Some(mut stdin) = child.stdin.take() {
        let data = input.to_string();
        let _ = stdin.write_all(data.as_bytes()).await;
    }
    let pid = child.id();
    let r = tokio::time::timeout(Duration::from_secs(h.timeout), child.wait_with_output()).await;
    if let Some(p) = pwd {
        let _ = std::fs::remove_file(p);
    }
    match r {
        Ok(Ok(o)) => (o.status.code(), String::from_utf8_lossy(&o.stdout).to_string(), String::from_utf8_lossy(&o.stderr).to_string()),
        Ok(Err(e)) => (None, String::new(), e.to_string()),
        Err(_) => {
            if let Some(p) = pid {
                #[cfg(unix)]
                crate::codetools::kill_process_group(p);
                #[cfg(windows)]
                crate::codetools::kill_process_tree(p);
            }
            (None, String::new(), format!("hook timed out after {}s: {}", h.timeout, h.command))
        }
    }
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
        // untrusted: nothing runs
        let cfg = load(&dir, &HashMap::new());
        let (root, hash, cmds) = cfg.untrusted.clone().unwrap();
        assert_eq!(cmds.len(), 1);
        assert!(run(&cfg, "PreToolUse", "Bash", json!({"tool_input": {"command": "rm -rf x"}}), &dir).await.block.is_none());
        // trusted
        let mut trusted = HashMap::new();
        trusted.insert(root.to_string_lossy().to_string(), hash);
        let cfg = load(&dir, &trusted);
        assert!(cfg.untrusted.is_none());
        let o = run(&cfg, "PreToolUse", "Bash", json!({"tool_input": {"command": "rm -rf x"}}), &dir).await;
        assert_eq!(o.block.as_deref(), Some("dangerous"));
        let o = run(&cfg, "PreToolUse", "Bash", json!({"tool_input": {"command": "ls"}}), &dir).await;
        assert!(o.block.is_none());
        let o = run(&cfg, "PreToolUse", "Edit", json!({}), &dir).await;
        assert_eq!(o, Outcome::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
