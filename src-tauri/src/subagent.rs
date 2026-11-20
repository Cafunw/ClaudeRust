//! Sub-agents for the built-in code agent: the `Task` tool starts a
//! separate agent with a fresh context to research or carry out one part
//! of the work and report back. Several `Task` calls in one response run in
//! parallel. Running sub-agents are listed in the Tasks pane and can be
//! stopped; their tool activity streams into the Task card.

use crate::codetools::{self, Perm};
use crate::engine::{base_body, stream_with_recovery, ReqCtx};
use crate::state::{AppState, PermissionReply};
use crate::util::{new_id, now_ms};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const MAX_STEPS: usize = 60;

pub const TASK_TOOL: &str = "Task";

pub fn definition() -> Value {
    json!({
        "name": TASK_TOOL,
        "description": "Launch a sub-agent with a fresh context to handle a self-contained part of the work: researching a question across many files, exploring an unfamiliar area, or carrying out an independent change. The sub-agent has the same coding tools (except Task and TodoWrite) and permissions, and returns one final report; it does not see this conversation, so give it everything it needs in the prompt. Launch several in one response to run them in parallel. Use subagent_type \"explore\" for read-only research.",
        "input_schema": {
            "type": "object",
            "properties": {
                "description": {"type": "string", "description": "A short (3-6 word) description of the task."},
                "prompt": {"type": "string", "description": "The complete task for the sub-agent, including what to report back."},
                "subagent_type": {"type": "string", "enum": ["general-purpose", "explore"], "description": "general-purpose (default) can edit files and run commands; explore is read-only."}
            },
            "required": ["description", "prompt"]
        }
    })
}

const SUBAGENT_NOTE: &str = "\n\n# Sub-agent\nYou are a sub-agent launched by the main agent to handle one task. You cannot ask the user questions. Work autonomously with your tools, then reply with a concise final report: what you found or changed (with file paths and line numbers), and anything left undone. Only your final message is returned to the main agent.";

#[derive(Clone)]
pub struct SubAgent {
    pub id: String,
    pub conv_id: String,
    pub parent_msg: String,
    pub tool_use_id: String,
    pub description: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub log: Vec<String>,
    pub usage: serde_json::Map<String, Value>,
    pub cancel: CancellationToken,
}

impl SubAgent {
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id, "conversation_id": self.conv_id, "description": self.description, "status": self.status,
            "started_at": self.started_at, "finished_at": self.finished_at, "steps": self.log.len(), "tool_use_id": self.tool_use_id,
        })
    }
}

#[derive(Default)]
pub struct Registry {
    map: Mutex<HashMap<String, SubAgent>>,
}

impl Registry {
    pub fn list(&self, conv_id: &str) -> Vec<Value> {
        let mut v: Vec<SubAgent> = self.map.lock().values().filter(|s| s.conv_id == conv_id).cloned().collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        v.iter().map(|s| s.summary()).collect()
    }
    pub fn log(&self, id: &str) -> Option<String> {
        self.map.lock().get(id).map(|s| s.log.join("\n"))
    }
    pub fn stop(&self, id: &str) -> bool {
        match self.map.lock().get(id) {
            Some(s) if s.status == "running" => {
                s.cancel.cancel();
                true
            }
            _ => false,
        }
    }
    pub fn clear_finished(&self, conv_id: &str) {
        self.map.lock().retain(|_, s| s.conv_id != conv_id || s.status == "running");
    }
    pub fn forget_conversation(&self, conv_id: &str) {
        let mut m = self.map.lock();
        for s in m.values() {
            if s.conv_id == conv_id {
                s.cancel.cancel();
            }
        }
        m.retain(|_, s| s.conv_id != conv_id);
    }
    /// Token usage of the sub-agents a response started (added to its cost).
    pub fn usage_for(&self, parent_msg: &str) -> serde_json::Map<String, Value> {
        let mut tot = serde_json::Map::new();
        for s in self.map.lock().values().filter(|s| s.parent_msg == parent_msg) {
            for (k, v) in &s.usage {
                if let Some(n) = v.as_u64() {
                    let prev = tot.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                    tot.insert(k.clone(), json!(prev + n));
                }
            }
        }
        tot
    }
    fn update(&self, id: &str, f: impl FnOnce(&mut SubAgent)) {
        if let Some(s) = self.map.lock().get_mut(id) {
            f(s);
        }
    }
}

fn tool_line(name: &str, input: &Value) -> String {
    let arg = match name {
        "Bash" => input["command"].as_str().unwrap_or("").to_string(),
        "Read" | "Write" | "Edit" => input["file_path"].as_str().unwrap_or("").to_string(),
        "Glob" | "Grep" => input["pattern"].as_str().unwrap_or("").to_string(),
        _ => String::new(),
    };
    let arg: String = arg.chars().take(140).collect();
    format!("{name} {arg}").trim().to_string()
}

/// Ask the user about a sub-agent's tool call (the regular permission card).
async fn ask(app: &AppHandle, st: &AppState, conv_id: &str, payload: Value, cancel: &CancellationToken) -> Option<PermissionReply> {
    let req_id = new_id();
    let (tx, rx) = oneshot::channel();
    st.permissions.lock().insert(req_id.clone(), tx);
    let mut p = payload;
    p["request_id"] = json!(req_id);
    p["conversation_id"] = json!(conv_id);
    st.pending_perms.lock().insert(req_id.clone(), p.clone());
    let _ = app.emit("permission-request", &p);
    crate::engine::notify_attention(app, st, conv_id, &p);
    let r = tokio::select! {
        r = rx => r.ok(),
        _ = cancel.cancelled() => None,
    };
    st.permissions.lock().remove(&req_id);
    st.pending_perms.lock().remove(&req_id);
    let _ = app.emit("permission-resolved", json!({"request_id": req_id, "conversation_id": conv_id}));
    r
}

/// Run one `Task` call; returns the tool result blocks and whether it failed.
#[allow(clippy::too_many_arguments)]
pub async fn run_task(
    app: &AppHandle,
    st: &Arc<AppState>,
    conv_id: &str,
    parent_msg: &str,
    tool_use_id: &str,
    rc: &ReqCtx,
    input: &Value,
    cancel: &CancellationToken,
) -> (Vec<Value>, bool) {
    let text = |s: String| vec![json!({"type": "text", "text": s})];
    let Some(session) = rc.code.clone() else { return (text("Sub-agents are only available in code sessions.".into()), true) };
    let prompt = input["prompt"].as_str().unwrap_or("").trim().to_string();
    if prompt.is_empty() {
        return (text("prompt is required".into()), true);
    }
    let description = input["description"].as_str().unwrap_or("Sub-agent").to_string();
    let explore = input["subagent_type"].as_str() == Some("explore");
    let Ok(conv) = st.db.conv_get(conv_id) else { return (text("Conversation not found.".into()), true) };
    let settings = st.settings.read().clone();
    let mode = if explore { "plan".to_string() } else { conv.options["permission_mode"].as_str().unwrap_or(&settings.code_permission_mode).to_string() };

    let id = format!("agent_{}", &new_id()[..8]);
    let my_cancel = cancel.child_token();
    st.subagents.map.lock().insert(id.clone(), SubAgent {
        id: id.clone(),
        conv_id: conv_id.to_string(),
        parent_msg: parent_msg.to_string(),
        tool_use_id: tool_use_id.to_string(),
        description: description.clone(),
        status: "running".into(),
        started_at: now_ms(),
        finished_at: None,
        log: vec![],
        usage: Default::default(),
        cancel: my_cancel.clone(),
    });
    let changed = |app: &AppHandle| {
        let _ = app.emit("subagents-changed", json!({"conversation_id": conv_id}));
    };
    changed(app);

    let tools: Vec<Value> = codetools::definitions(false)
        .into_iter()
        .filter(|t| !matches!(t["name"].as_str(), Some("TodoWrite" | "ExitPlanMode")))
        .filter(|t| !explore || matches!(t["name"].as_str(), Some("Read" | "Glob" | "Grep" | "Bash")))
        .collect();
    let sub_rc = ReqCtx {
        model_id: rc.model_id.clone(),
        model: rc.model.clone(),
        system: format!("{}{SUBAGENT_NOTE}", rc.system),
        tools,
        fingerprint: format!("sub-{}", rc.fingerprint),
        prefix_blocks: vec![],
        code: rc.code.clone(),
        mcp_names: HashMap::new(),
        mcp_read_only: Default::default(),
        first_party: rc.first_party,
    };
    let client = match st.client() {
        Ok(c) => c,
        Err(e) => return (text(e.to_string()), true),
    };
    let mut messages: Vec<Value> = vec![json!({"role": "user", "content": [{"type": "text", "text": prompt}]})];
    let mut final_text = String::new();
    let mut failed: Option<String> = None;
    let progress = |app: &AppHandle, log: &[String]| {
        let _ = app.emit("tool-progress", json!({"conversation_id": conv_id, "tool_use_id": tool_use_id, "text": log.join("\n")}));
    };
    let mut log: Vec<String> = vec![];
    for _step in 0..MAX_STEPS {
        if my_cancel.is_cancelled() {
            failed = Some("The sub-agent was stopped.".into());
            break;
        }
        let (mut body, mut betas) = base_body(&sub_rc, &settings, &conv, messages.clone());
        let mut noop = |_: &crate::anthropic::Accumulator, _: Option<usize>| {};
        let out = match stream_with_recovery(st, &client, &mut body, &mut betas, &my_cancel, &mut noop).await {
            Ok(o) => o,
            Err(e) => {
                failed = Some(e.to_string());
                break;
            }
        };
        let acc = out.acc;
        let usage = acc.usage.clone();
        st.subagents.update(&id, |s| {
            for (k, v) in usage.iter() {
                if let Some(n) = v.as_u64() {
                    let prev = s.usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                    s.usage.insert(k.clone(), json!(prev + n));
                }
            }
        });
        let raw = Value::Array(acc.content());
        let content: Vec<Value> = crate::engine::sanitize_assistant(&raw, &crate::engine::all_tool_ids(&raw));
        if let Some(e) = out.error {
            failed = Some(e.to_string());
            break;
        }
        let texts: Vec<String> = content.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str().map(String::from)).collect();
        if !texts.is_empty() {
            final_text = texts.join("\n");
        }
        let uses: Vec<Value> = content.iter().filter(|b| b["type"] == "tool_use").cloned().collect();
        messages.push(json!({"role": "assistant", "content": content}));
        if acc.stop_reason.as_deref() != Some("tool_use") || uses.is_empty() {
            break;
        }
        let mut results = vec![];
        for tu in uses {
            let name = tu["name"].as_str().unwrap_or("").to_string();
            let tid = tu["id"].as_str().unwrap_or("").to_string();
            let tin = tu["input"].clone();
            log.push(tool_line(&name, &tin));
            st.subagents.update(&id, |s| s.log = log.clone());
            progress(app, &log);
            let allowed = match codetools::check_permission(&mode, &name, &tin, &crate::engine::allowed_rules(&conv), Some(&session)) {
                Perm::Allow => Ok(()),
                Perm::Deny(m) => Err(m),
                Perm::Ask => {
                    let diff = codetools::preview_diff(Some(&session), &name, &tin);
                    let payload = json!({"kind": "code", "tool": name, "input": tin, "diff": diff, "cwd": session.root,
                        "description": format!("Sub-agent \"{description}\""), "tool_use_id": tool_use_id});
                    match ask(app, st, conv_id, payload, &my_cancel).await.map(|r| r.decision) {
                        Some(d) if matches!(d.as_str(), "once" | "session" | "chat" | "always" | "approve_edits") => Ok(()),
                        _ => Err("The user denied this tool call.".into()),
                    }
                }
            };
            let (blocks, is_error) = match allowed {
                Err(m) => (text(m), true),
                Ok(()) => {
                    let o = codetools::execute(session.clone(), &name, &tin, &my_cancel, Duration::from_secs(settings.bash_timeout_secs), None).await;
                    (o.content, o.is_error)
                }
            };
            if matches!(name.as_str(), "Write" | "Edit" | "Bash") {
                let _ = app.emit("files-changed", json!({"conversation_id": conv_id}));
            }
            let mut r = json!({"type": "tool_result", "tool_use_id": tid, "content": blocks});
            if is_error {
                r["is_error"] = json!(true);
            }
            results.push(r);
        }
        messages.push(json!({"role": "user", "content": results}));
    }
    let status = if my_cancel.is_cancelled() { "stopped" } else if failed.is_some() { "failed" } else { "completed" };
    st.subagents.update(&id, |s| {
        s.status = status.into();
        s.finished_at = Some(now_ms());
        s.log = log.clone();
    });
    changed(app);
    match failed {
        Some(e) if final_text.is_empty() => (text(format!("The sub-agent did not finish: {e}")), true),
        Some(e) => (text(format!("{final_text}\n\n(The sub-agent did not finish: {e})")), true),
        None if final_text.trim().is_empty() => (text("The sub-agent finished without a report.".into()), false),
        None => (text(final_text), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_lines_and_usage() {
        assert_eq!(tool_line("Bash", &json!({"command": "cargo test"})), "Bash cargo test");
        assert_eq!(tool_line("Read", &json!({"file_path": "/a/b.rs"})), "Read /a/b.rs");
        let r = Registry::default();
        let mut usage = serde_json::Map::new();
        usage.insert("input_tokens".into(), json!(100));
        for i in 0..2 {
            r.map.lock().insert(format!("a{i}"), SubAgent {
                id: format!("a{i}"), conv_id: "c".into(), parent_msg: "m".into(), tool_use_id: "t".into(), description: "d".into(),
                status: "completed".into(), started_at: i, finished_at: None, log: vec![], usage: usage.clone(), cancel: CancellationToken::new(),
            });
        }
        assert_eq!(r.usage_for("m")["input_tokens"], 200);
        assert_eq!(r.list("c").len(), 2);
        r.clear_finished("c");
        assert!(r.list("c").is_empty());
        assert_eq!(definition()["name"], "Task");
    }
}
