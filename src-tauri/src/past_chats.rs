//! Search and reference past chats: the `conversation_search` and `recent_chats` tools a chat gets while the
//! setting is on. The API engine offers them as its own tools; the account engine as an SDK-hosted MCP server
//! of the app's (`SERVER`), which the CLI reaches through us. Only what the database lists is searched: the
//! conversation asking is left out, and so are incognito chats (db.rs keeps them out of these queries).

use crate::mcp::McpTool;
use crate::render;
use crate::state::AppState;
use serde_json::{json, Value};

pub const NAMES: [&str; 2] = ["conversation_search", "recent_chats"];

/// The SDK-hosted server that carries the tools to the CLI, so it calls them `mcp__past_chats__<tool>`.
pub const SERVER: &str = "past_chats";

pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "conversation_search",
            "description": "Search the user's past conversations in this app by keywords. Use it when the user refers to something discussed before (for example \"as we discussed\", \"my project from last week\", \"what did you tell me about X\") or when earlier context would clearly help.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Keywords to search for (names, topics, distinctive words)."},
                    "max_results": {"type": "integer", "description": "Maximum number of results (default 5, max 10)."}
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "recent_chats",
            "description": "List the user's most recent conversations with an excerpt of how each started. Use it when the user asks about their recent or previous chats without giving specific keywords.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "n": {"type": "integer", "description": "How many chats to list (default 5, max 20)."}
                }
            }
        }),
    ]
}

/// The tools as the account engine's SDK-hosted server lists them.
pub fn mcp_tools() -> Vec<McpTool> {
    crate::mcp::app_tools(SERVER, definitions())
}

/// One of these tools' answer to a call from `conv_id`: text blocks, and whether it failed.
pub fn run(st: &AppState, conv_id: &str, name: &str, input: &Value) -> (Vec<Value>, bool) {
    let text = |s: String| vec![json!({"type": "text", "text": s})];
    let fmt_date = |ms: i64| {
        chrono::DateTime::from_timestamp_millis(ms)
            .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
            .unwrap_or_default()
    };
    if name == "recent_chats" {
        let n = input["n"].as_u64().unwrap_or(5).clamp(1, 20) as usize;
        return match st.db.recent_with_first_message(conv_id, n) {
            Ok(rows) if !rows.is_empty() => {
                let mut out = String::new();
                for (_, title, updated, first) in rows {
                    let first_text = serde_json::from_str::<Value>(&first).map(|v| render::plain_text(&v)).unwrap_or(first);
                    out.push_str(&format!(
                        "<chat title=\"{}\" updated=\"{}\">\n{}\n</chat>\n",
                        if title.is_empty() { "Untitled" } else { &title },
                        fmt_date(updated),
                        crate::util::truncate_chars(first_text.trim(), 600)
                    ));
                }
                (text(out), false)
            }
            Ok(_) => (text("No previous chats found.".into()), false),
            Err(e) => (text(e.to_string()), true),
        };
    }
    let q = input["query"].as_str().unwrap_or("").trim();
    if q.is_empty() {
        return (text("query is required".into()), true);
    }
    let n = input["max_results"].as_u64().unwrap_or(5).clamp(1, 10) as usize;
    match st.db.search_messages(q, conv_id, n) {
        Ok(rows) if !rows.is_empty() => {
            let mut out = String::new();
            for (_, title, at, role, body) in rows {
                out.push_str(&format!(
                    "<result chat=\"{}\" date=\"{}\" from=\"{}\">\n{}\n</result>\n",
                    if title.is_empty() { "Untitled" } else { &title },
                    fmt_date(at),
                    if role == "user" { "user" } else { "assistant" },
                    crate::util::truncate_chars(body.trim(), 1500)
                ));
            }
            (text(out), false)
        }
        Ok(_) => (text(format!("No past conversations matched \"{q}\".")), false),
        Err(e) => (text(e.to_string()), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Message;

    #[test]
    fn past_chats_are_found_but_not_the_one_asking() {
        let dir = std::env::temp_dir().join(format!("cr-past-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        let say = |conv: &str, text: &str| {
            let m = Message { id: crate::util::new_id(), conversation_id: conv.into(), parent_id: None, role: "user".into(), content: json!([{"type": "text", "text": text}]), meta: json!({}), created_at: 1 };
            st.db.msg_insert(&m).unwrap();
            st.db.fts_index(conv, &m.id, text).unwrap();
        };
        let old = st.db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        st.db.conv_set_title(&old.id, "Garden").unwrap();
        say(&old.id, "My tomatoes need staking before the storm");
        let secret = st.db.conv_create("chat", "m", None, None, &json!({"incognito": true})).unwrap();
        st.db.conv_set_title(&secret.id, "Hidden").unwrap();
        say(&secret.id, "Incognito tomatoes, never to be found");
        let now = st.db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        say(&now.id, "Tell me about tomatoes again");

        // the other chat, not the one asking nor an incognito one
        let (found, err) = run(&st, &now.id, "conversation_search", &json!({"query": "tomatoes"}));
        let found = found[0]["text"].as_str().unwrap();
        assert!(!err && found.contains("chat=\"Garden\"") && found.contains("staking") && !found.contains("again") && !found.contains("Incognito"), "{found}");
        let (recent, err) = run(&st, &now.id, "recent_chats", &json!({"n": 50}));
        let recent = recent[0]["text"].as_str().unwrap();
        assert!(!err && recent.contains("<chat title=\"Garden\"") && !recent.contains("Hidden"), "{recent}");
        assert!(run(&st, &now.id, "conversation_search", &json!({"query": " "})).1);
        assert_eq!(run(&st, &now.id, "conversation_search", &json!({"query": "zucchini"})).0[0]["text"], "No past conversations matched \"zucchini\".");

        // as the account engine's server lists them: the bare tool name, which no MCP server's tool has
        let tools = mcp_tools();
        assert_eq!(tools.iter().map(|t| (t.server.as_str(), t.api_name.as_str())).collect::<Vec<_>>(), [(SERVER, "conversation_search"), (SERVER, "recent_chats")]);
        assert!(tools.iter().all(|t| t.read_only && t.input_schema["type"] == "object"));
        assert!(!NAMES.contains(&crate::mcp::api_tool_name(SERVER, "conversation_search").as_str()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
