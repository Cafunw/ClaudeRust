//! `window.claude.callMcpTool` for artifacts: a tool of the person's connectors,
//! called from an artifact's sandboxed frame. The frame can't reach the app:
//! its request comes through ui/js/artifacts.js, which asks the person once
//! per artifact and connector, and sends the result back to that frame only.
//! Here each call goes through the same policy as the model's call of that
//! tool (a blocked tool stays blocked, one that asks still asks), and is
//! rate limited and size capped.

use super::apps::{decide, Calls, Decision, MAX_RESULT_BYTES};
use super::McpTool;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

/// The largest arguments an artifact may send to a tool.
pub const MAX_ARGS_BYTES: usize = 256 * 1024;

static CALLS: LazyLock<Mutex<Calls>> = LazyLock::new(Default::default);

type St<'a> = tauri::State<'a, Arc<AppState>>;

/// The tool an artifact names: as the model's tool list does (`server__tool`),
/// or the Claude Code CLI's (`mcp__server__tool`).
fn find<'a>(tools: &'a [McpTool], name: &str) -> Option<&'a McpTool> {
    let bare = name.strip_prefix("mcp__").unwrap_or(name);
    tools.iter().find(|t| t.api_name == name).or_else(|| tools.iter().find(|t| t.api_name == bare))
}

/// Whether `tool` may run for an artifact of `conv`, as the model's call of it
/// would: turned off in this chat, blocked or read-only in plan mode are
/// refused, else the policy decides (`approved`: the person said yes to this call).
fn decision(st: &AppState, conv: &crate::db::Conversation, tool: &McpTool, approved: bool) -> AppResult<Decision> {
    let listed = |key: &str, v: &str| conv.options[key].as_array().into_iter().flatten().any(|x| x.as_str() == Some(v));
    if listed("disabled_servers", &tool.server) {
        return Err(AppError::coded("artifact_tool_server_off", json!({"server": tool.server}), format!("{} is turned off in this chat.", tool.server)));
    }
    if listed("disabled_tools", &tool.api_name) {
        return Err(AppError::coded("artifact_tool_off", json!({"tool": tool.name}), format!("{} is turned off in this chat.", tool.name)));
    }
    let s = st.settings.read();
    let mode = conv.options["permission_mode"].as_str().unwrap_or(&s.code_permission_mode);
    if conv.kind == "code" && mode == "plan" && !tool.read_only {
        return Err(AppError::coded("artifact_tool_plan_mode", json!({"tool": tool.name}), format!("{} can make changes, and plan mode doesn't run such tools.", tool.name)));
    }
    let policy = s.tool_policies.get(&format!("{}/{}", tool.server, tool.name)).and_then(Value::as_str).unwrap_or("ask");
    match decide(policy, listed("allowed_tools", &tool.api_name), approved) {
        Decision::Deny => Err(AppError::coded("artifact_tool_blocked", json!({"tool": tool.name}), format!("{} is blocked in your connector settings.", tool.name))),
        d => Ok(d),
    }
}

async fn resolve(st: &AppState, conversation_id: &str, name: &str, approved: bool) -> AppResult<(McpTool, Decision)> {
    let conv = st.db.conv_get(conversation_id)?;
    let tools = st.mcp.tools().await;
    let tool = find(&tools, name).cloned().ok_or_else(|| AppError::coded("artifact_tool_unknown", json!({"name": name}), format!("There's no connected tool named {name}.")))?;
    let d = decision(st, &conv, &tool, approved)?;
    Ok((tool, d))
}

/// The tool `name` that artifact `artifact_id` of `conversation_id` asks to
/// call, before the person is asked: its server, whether it only reads, and
/// whether its policy asks about each call. A refused tool is an error, and
/// each check counts toward the artifact's calls a minute, so a page can't
/// raise a stream of questions.
#[tauri::command]
pub async fn artifact_tool_check(state: St<'_>, conversation_id: String, artifact_id: String, name: String) -> AppResult<Value> {
    let (tool, d) = resolve(&state, &conversation_id, &name, false).await?;
    CALLS.lock().take(&format!("{conversation_id}/{artifact_id}"), &tool.server, Instant::now())?;
    Ok(json!({"server": tool.server, "tool": tool.name, "title": tool.title, "read_only": tool.read_only, "asks": d == Decision::Ask}))
}

/// Call tool `name` with `arguments` for an artifact of `conversation_id`,
/// once the person allowed it (`approved` where its policy asks): the MCP
/// result as Claude Desktop's artifacts get it, `{content, structuredContent, isError}`.
#[tauri::command]
pub async fn artifact_tool_call(state: St<'_>, conversation_id: String, name: String, arguments: Value, approved: bool) -> AppResult<Value> {
    if !(arguments.is_object() || arguments.is_null()) {
        return Err(AppError::coded("artifact_tool_args_not_object", Value::Null, "Tool arguments must be an object."));
    }
    if arguments.to_string().len() > MAX_ARGS_BYTES {
        return Err(AppError::coded("artifact_tool_args_too_large", Value::Null, "The tool's arguments are larger than 256 KB."));
    }
    let (tool, d) = resolve(&state, &conversation_id, &name, approved).await?;
    if d == Decision::Ask {
        return Err(AppError::coded("artifact_tool_not_allowed", Value::Null, "The user didn't allow this call."));
    }
    let r = state.mcp.call_raw(&tool.server, &tool.name, &arguments).await?;
    result(r)
}

fn result(r: Value) -> AppResult<Value> {
    if r.to_string().len() > MAX_RESULT_BYTES {
        return Err(AppError::coded("artifact_tool_result_too_large", Value::Null, "The tool's result is too large for the artifact (over 2 MB)."));
    }
    let mut out = json!({"content": r["content"].as_array().cloned().unwrap_or_default(), "isError": r["isError"] == true});
    if r["structuredContent"].is_object() {
        out["structuredContent"] = r["structuredContent"].clone();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn tool(server: &str, name: &str, read_only: bool) -> McpTool {
        McpTool { server: server.into(), name: name.into(), api_name: super::super::api_tool_name(server, name), title: None, description: String::new(), input_schema: json!({}), read_only }
    }

    fn state() -> AppState {
        let dir = std::env::temp_dir().join(format!("cr-artifact-tools-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"))
    }

    #[test]
    fn a_tool_is_named_as_the_model_or_the_cli_names_it() {
        let tools = [tool("notes", "add", false), tool("mcp", "x", true), tool("weather", "get", true)];
        assert_eq!(find(&tools, "notes__add").unwrap().name, "add");
        assert_eq!(find(&tools, "mcp__notes__add").unwrap().name, "add");
        // a server called "mcp" keeps its own name
        assert_eq!(find(&tools, "mcp__x").unwrap().server, "mcp");
        for missing in ["add", "notes__", "mcp__", "", "notes__add ", "weather__add"] {
            assert!(find(&tools, missing).is_none(), "{missing}");
        }
    }

    #[test]
    fn the_chats_policy_for_the_tool_decides() {
        let st = state();
        let t = tool("notes", "add", false);
        let chat = |options: Value| st.db.conv_get(&st.db.conv_create("chat", "m", None, None, &options).unwrap().id).unwrap();
        let plain = chat(json!({}));
        // no policy asks; the person's yes runs it
        assert_eq!(decision(&st, &plain, &t, false).unwrap(), Decision::Ask);
        assert_eq!(decision(&st, &plain, &t, true).unwrap(), Decision::Run);
        // allowed for this chat runs without asking
        assert_eq!(decision(&st, &chat(json!({"allowed_tools": [t.api_name]})), &t, false).unwrap(), Decision::Run);
        // a server turned off in this chat is refused, even with a yes
        assert!(decision(&st, &chat(json!({"disabled_servers": ["notes"]})), &t, true).unwrap_err().to_string().contains("turned off"));
        // and so is one of its tools switched off there (its API name), even with a yes or a chat's allowance
        assert!(decision(&st, &chat(json!({"disabled_tools": [t.api_name], "allowed_tools": [t.api_name]})), &t, true).unwrap_err().to_string().contains("turned off"));
        st.settings.write().tool_policies.insert("notes/add".into(), json!("allow"));
        assert_eq!(decision(&st, &plain, &t, false).unwrap(), Decision::Run);
        // blocked stays blocked, whatever the person answered
        st.settings.write().tool_policies.insert("notes/add".into(), json!("deny"));
        assert!(decision(&st, &plain, &t, true).unwrap_err().to_string().contains("blocked"));
        // plan mode runs only tools that read
        let plan = st.db.conv_get(&st.db.conv_create("code", "m", None, None, &json!({"permission_mode": "plan"})).unwrap().id).unwrap();
        st.settings.write().tool_policies.clear();
        assert!(decision(&st, &plan, &t, true).is_err());
        assert_eq!(decision(&st, &plan, &tool("w", "get", true), true).unwrap(), Decision::Run);
    }

    #[test]
    fn the_artifact_gets_the_result_in_desktops_shape_within_the_cap() {
        let r = result(json!({"content": [{"type": "text", "text": "hi"}], "structuredContent": {"n": 1}, "_meta": {"x": 1}})).unwrap();
        assert_eq!(r, json!({"content": [{"type": "text", "text": "hi"}], "structuredContent": {"n": 1}, "isError": false}));
        assert_eq!(result(json!({"isError": true})).unwrap()["isError"], true);
        assert_eq!(result(json!({"structuredContent": "not an object"})).unwrap(), json!({"content": [], "isError": false}));
        assert!(result(json!({"content": [{"type": "text", "text": "x".repeat(MAX_RESULT_BYTES)}]})).is_err());
    }
}
