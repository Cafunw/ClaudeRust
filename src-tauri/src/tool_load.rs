//! "Tool access: load tools when needed". With many MCP tools connected, a chat sends the model each tool's
//! name and one-line description in its prompt instead of the full schemas, and gives it `load_tools`; a tool
//! the model loads joins the tool list with its schema for the rest of the conversation (the conversation's
//! `loaded_tools`). The API engine adds it to the request's tools from the next step on; the account engine's
//! CLI lists it on the SDK-hosted server and is told the list changed. Only the conversation's own tools can
//! be loaded: a switched-off server or tool, or one set to Block, isn't among them.

use crate::mcp::McpTool;
use crate::state::AppState;
use serde_json::{json, Value};

pub const LOAD_TOOL: &str = "load_tools";

/// The SDK-hosted server that carries `load_tools` to the account engine's CLI (`mcp__tool_access__load_tools`).
pub const SERVER: &str = "tool_access";

/// With more MCP tools than this, a chat loads them when needed unless it says otherwise (`tool_access`).
pub const DEFER_ABOVE: usize = 20;

/// Does a chat with these options and `count` MCP tools load them when needed? `tool_access` is "deferred"
/// (always), "all" (up front) or unset (above the threshold).
pub fn deferred(options: &Value, count: usize) -> bool {
    match options["tool_access"].as_str() {
        Some("deferred") => count > 0,
        Some("all") => false,
        _ => count > DEFER_ABOVE,
    }
}

/// The API names of the tools this conversation has loaded.
pub fn loaded(options: &Value) -> Vec<String> {
    options["loaded_tools"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
}

pub fn definition() -> Value {
    json!({
        "name": LOAD_TOOL,
        "description": "Load tools from the list of available tools in the system prompt, by name, so they can be called. Load every tool you need for the task in one call; a loaded tool stays available for the rest of the conversation.",
        "input_schema": {
            "type": "object",
            "properties": {"names": {"type": "array", "items": {"type": "string"}, "description": "Tool names exactly as the list gives them."}},
            "required": ["names"]
        }
    })
}

/// The tool as the account engine's SDK-hosted server lists it.
pub fn mcp_tools() -> Vec<McpTool> {
    crate::mcp::app_tools(SERVER, vec![definition()])
}

/// What the prompt says of the tools not loaded yet: a name and a one-line description each.
pub fn index_block<'a>(tools: impl Iterator<Item = &'a McpTool>) -> String {
    let mut s = format!("<available_tools>\nThe user's connected apps (MCP servers) offer these tools. They aren't loaded yet, so their parameters aren't shown: to use one, first call {LOAD_TOOL} with its name, then call it.\n");
    for t in tools {
        let desc = t.description.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
        s.push_str(&format!("- {}: {}\n", t.api_name, crate::util::truncate_chars(desc, 160)));
    }
    s.push_str("</available_tools>");
    s
}

/// `load_tools` for `conv_id`, whose tools are `available` (API names): the names it loads are kept in the
/// conversation (as the CLI writes a name, `mcp__<server>__<tool>`, too). The answer, whether it failed, and the
/// API names newly loaded.
pub fn load(st: &AppState, conv_id: &str, available: &[&McpTool], input: &Value) -> (String, bool, Vec<String>) {
    let asked: Vec<&str> = input["names"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::trim).collect();
    let mut found: Vec<String> = vec![];
    let mut missing: Vec<&str> = vec![];
    for name in &asked {
        let bare = name.strip_prefix("mcp__").unwrap_or(name);
        let cli = |t: &McpTool| format!("{}__{}", crate::claudecode::cli_safe(&t.server), crate::claudecode::cli_safe(&t.name));
        match available.iter().find(|t| t.api_name == *name || t.api_name == bare || cli(t) == bare) {
            Some(t) if !found.contains(&t.api_name) => found.push(t.api_name.clone()),
            Some(_) => {}
            None => missing.push(name),
        }
    }
    let mut new = vec![];
    if !found.is_empty() {
        crate::engine::update_conv_options(st, conv_id, |o| {
            let mut all = loaded(o);
            new = found.iter().filter(|n| !all.contains(n)).cloned().collect();
            all.extend(new.iter().cloned());
            o["loaded_tools"] = json!(all);
        });
    }
    let mut text = if found.is_empty() { String::new() } else { format!("Loaded: {}. Call them now.", found.join(", ")) };
    if !missing.is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!("Not available in this chat: {}.", missing.join(", ")));
    }
    if asked.is_empty() {
        text = "Give the names of the tools to load.".into();
    }
    (text, found.is_empty(), new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(server: &str, name: &str) -> McpTool {
        McpTool { server: server.into(), name: name.into(), api_name: format!("{server}__{name}"), title: None, description: format!("Does {name}.\nMore."), input_schema: json!({"type": "object"}), read_only: false }
    }

    #[test]
    fn many_tools_are_loaded_when_needed_unless_the_chat_says_otherwise() {
        assert!(!deferred(&json!({}), DEFER_ABOVE));
        assert!(deferred(&json!({}), DEFER_ABOVE + 1));
        assert!(!deferred(&json!({"tool_access": "all"}), 100));
        assert!(deferred(&json!({"tool_access": "deferred"}), 1));
        assert!(!deferred(&json!({"tool_access": "deferred"}), 0), "no tools: nothing to load");
    }

    #[test]
    fn the_index_lists_each_tool_on_one_line() {
        let (a, b) = (tool("github", "create_issue"), tool("my notes", "find"));
        let s = index_block([&a, &b].into_iter());
        assert!(s.contains("- github__create_issue: Does create_issue.\n- my notes__find: Does find.\n</available_tools>"), "{s}");
        assert!(!s.contains("More."));
    }

    #[test]
    fn only_the_chats_own_tools_are_loaded_and_kept() {
        let dir = std::env::temp_dir().join(format!("cr-load-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        let c = st.db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let (a, b, off) = (tool("github", "create_issue"), tool("my notes", "find"), tool("github", "delete_repo"));
        // `off` is switched off (or set to Block) in this chat: it isn't among the available ones
        let available = [&a, &b];
        let (text, failed, new) = load(&st, &c.id, &available, &json!({"names": ["github__create_issue", "mcp__my_notes__find", "github__delete_repo"]}));
        assert!(!failed);
        assert_eq!(new, ["github__create_issue", "my notes__find"]);
        assert!(text.contains("Not available in this chat: github__delete_repo."), "{text}");
        assert_eq!(loaded(&st.db.conv_get(&c.id).unwrap().options), ["github__create_issue", "my notes__find"]);
        // asked again: nothing new; only what can't be loaded fails
        let (_, failed, new) = load(&st, &c.id, &available, &json!({"names": ["github__create_issue"]}));
        assert!(!failed && new.is_empty());
        let (_, failed, _) = load(&st, &c.id, &available, &json!({"names": [off.api_name]}));
        assert!(failed);
        assert_eq!(loaded(&st.db.conv_get(&c.id).unwrap().options).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
