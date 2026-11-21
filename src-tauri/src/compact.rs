//! Context compaction: replace the earlier part of a long conversation with
//! a summary, manually (`/compact`) or automatically when the context window
//! fills up.
//!
//! The summary is stored on the user message it precedes
//! (`meta.compact_summary`); when building a request, everything before
//! that message is dropped and the summary is put in front of it. Branches
//! and the full history stay in the database.

use crate::db::Message;
use crate::error::{AppError, AppResult};
use crate::models::{self, Thinking};
use crate::render;
use crate::state::AppState;
use crate::util::truncate_middle;
use serde_json::{json, Value};
use std::time::Duration;

pub const COMPACT_PROMPT: &str = "You are summarizing a coding/assistant conversation so it can continue in a fresh context window. Write a detailed summary that lets the work continue without the original messages. Include, in this order:\n1. The user's requests and intent, in their own words where it matters.\n2. Key technical decisions, constraints and preferences the user stated.\n3. Files and code: every file read, created or modified, with what changed and why; include short code snippets that are essential.\n4. Errors hit and how they were fixed.\n5. Current state: what was just completed.\n6. Pending tasks and the exact next step.\nBe specific (names, paths, commands, numbers). Do not invent anything. Reply with the summary only.";

pub const CONTINUATION_PREFIX: &str = "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n";

/// Index of the message from which a request starts (the last message that
/// carries a compaction summary), if any.
pub fn start_index(path: &[Message]) -> Option<usize> {
    path.iter().rposition(|m| m.meta["compact_summary"].is_string())
}

/// The path as sent to the model: messages before the last compaction point
/// dropped, its summary put in front of that message.
pub fn apply(path: &[Message]) -> Vec<Message> {
    sent(path).into_owned()
}

/// `apply` without copying the path when nothing was compacted (each step of
/// a turn builds its request from the whole conversation).
pub fn sent(path: &[Message]) -> std::borrow::Cow<'_, [Message]> {
    match start_index(path) {
        None => std::borrow::Cow::Borrowed(path),
        Some(i) => {
            let mut v = path[i..].to_vec();
            let first = &mut v[0];
            let summary = first.meta["compact_summary"].as_str().unwrap_or("").to_string();
            let mut content = vec![json!({"type": "text", "text": format!("{CONTINUATION_PREFIX}<summary>\n{summary}\n</summary>")})];
            content.extend(first.content.as_array().cloned().unwrap_or_default());
            first.content = Value::Array(content);
            std::borrow::Cow::Owned(v)
        }
    }
}

/// Tokens in the context at the end of the path (from the last response's usage).
pub fn context_tokens(path: &[Message]) -> Option<u64> {
    let m = path.iter().rev().find(|m| m.role == "assistant" && m.meta["usage"].is_object())?;
    let u = &m.meta["usage"];
    let n = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"]
        .iter()
        .map(|k| u[*k].as_u64().unwrap_or(0))
        .sum::<u64>();
    (n > 0).then_some(n)
}

/// The conversation as plain text for the summarizer.
pub fn transcript(path: &[Message]) -> String {
    let mut out = String::new();
    for m in path {
        if let Some(s) = m.meta["compact_summary"].as_str() {
            out.push_str(&format!("[Summary of the conversation before this point]\n{s}\n\n"));
        }
        let tool_results = render::is_tool_result_msg(m);
        for b in m.content.as_array().into_iter().flatten() {
            match b["type"].as_str() {
                Some("text") => {
                    let t = b["text"].as_str().unwrap_or("");
                    if t.starts_with("<system-reminder>") {
                        continue;
                    }
                    let who = if m.role == "user" { "User" } else { "Assistant" };
                    out.push_str(&format!("{who}: {}\n\n", t.trim()));
                }
                Some("tool_use") => {
                    let input = b["input"].to_string();
                    out.push_str(&format!("[Assistant called {} with {}]\n\n", b["name"].as_str().unwrap_or("tool"), truncate_middle(&input, 1500)));
                }
                Some("tool_result") if tool_results => {
                    let text = match &b["content"] {
                        Value::String(s) => s.clone(),
                        Value::Array(a) => a.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    let err = if b["is_error"] == true { " (error)" } else { "" };
                    out.push_str(&format!("[Tool result{err}]\n{}\n\n", truncate_middle(text.trim(), 2000)));
                }
                _ => {}
            }
        }
    }
    out
}

/// Ask the model for a summary of `path`.
pub async fn summarize(st: &AppState, model_id: &str, path: &[Message], instructions: Option<&str>) -> AppResult<String> {
    let client = st.client()?;
    let model = models::info(model_id);
    let text = truncate_middle(&transcript(path), 600_000);
    let mut ask = format!("<conversation>\n{text}\n</conversation>\n\nSummarize the conversation above as instructed.");
    if let Some(i) = instructions.filter(|i| !i.trim().is_empty()) {
        ask.push_str(&format!(" Additional instructions from the user: {}", i.trim()));
    }
    let mut body = json!({
        "model": model_id,
        "max_tokens": 12000u32.min(model.max_output.max(4096)),
        "system": COMPACT_PROMPT,
        "messages": [{"role": "user", "content": ask}],
    });
    match model.thinking {
        Thinking::Disableable => body["thinking"] = json!({"type": "disabled"}),
        Thinking::BetweenTools => body["thinking"] = json!({"type": "between_tools"}),
        _ => {}
    }
    let r = tokio::time::timeout(Duration::from_secs(300), client.create(&body, &[]))
        .await
        .map_err(|_| AppError::msg("Summarizing took too long."))??;
    let summary: String = r["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("");
    if summary.trim().is_empty() {
        return Err(AppError::msg("The model returned an empty summary."));
    }
    Ok(summary.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, role: &str, content: Value, meta: Value) -> Message {
        Message { id: id.into(), conversation_id: "c".into(), parent_id: None, role: role.into(), content, meta, created_at: 0 }
    }

    #[test]
    fn apply_drops_messages_before_the_summary() {
        let path = vec![
            m("u1", "user", json!([{"type":"text","text":"old question"}]), json!({})),
            m("a1", "assistant", json!([{"type":"text","text":"old answer"}]), json!({"usage": {"input_tokens": 1000, "output_tokens": 50}})),
            m("u2", "user", json!([{"type":"text","text":"new question"}]), json!({"compact_summary": "We discussed X."})),
            m("a2", "assistant", json!([{"type":"text","text":"new answer"}]), json!({"usage": {"input_tokens": 300, "cache_read_input_tokens": 200, "output_tokens": 20}})),
        ];
        let v = apply(&path);
        assert_eq!(v.len(), 2);
        let first = v[0].content.as_array().unwrap();
        assert!(first[0]["text"].as_str().unwrap().contains("We discussed X."));
        assert_eq!(first[1]["text"], "new question");
        assert_eq!(context_tokens(&path), Some(520));
        let t = transcript(&path);
        assert!(t.contains("User: old question") && t.contains("Assistant: old answer") && t.contains("We discussed X."));
        assert_eq!(apply(&path[..2]).len(), 2);
    }
}
