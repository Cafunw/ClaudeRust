//! Turns stored API messages into UI "turns" made of display parts.

use crate::artifacts::{self, Segment};
use crate::db::Message;
use crate::markdown::{self, Opts};
use crate::util::{escape_html, truncate_middle};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub struct RenderCtx<'a> {
    pub math: bool,
    /// tool_use id -> live status ("running", "awaiting_permission")
    pub live_tools: &'a HashMap<String, String>,
    /// tool_use ids whose input is still streaming
    pub streaming_tools: &'a HashSet<String>,
    /// API tool name -> (server, tool) for MCP tools
    pub mcp_names: &'a HashMap<String, (String, String)>,
    /// message id currently streaming
    pub streaming_msg: Option<&'a str>,
}

#[derive(Default, Clone)]
pub struct ArtifactTracker {
    versions: HashMap<String, u32>,
}

impl ArtifactTracker {
    pub fn bump(&mut self, id: &str) {
        self.next(id);
    }

    fn next(&mut self, id: &str) -> u32 {
        let v = self.versions.entry(id.to_string()).or_insert(0);
        *v += 1;
        *v
    }
}

pub fn is_tool_result_msg(m: &Message) -> bool {
    m.meta["tool_results"].as_bool().unwrap_or(false)
}

/// Best-effort parse of an incomplete JSON object (streamed tool input).
pub fn close_partial_json(s: &str) -> Option<Value> {
    let s = s.trim();
    if s.is_empty() {
        return Some(json!({}));
    }
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return Some(v);
    }
    let mut candidate = s.to_string();
    for _ in 0..6 {
        if let Some(v) = try_close(&candidate) {
            return Some(v);
        }
        // cut back to the last comma at any depth and retry
        let mut in_str = false;
        let mut esc = false;
        let mut last_comma = None;
        for (i, c) in candidate.char_indices() {
            if in_str {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    in_str = false;
                }
            } else if c == '"' {
                in_str = true;
            } else if c == ',' {
                last_comma = Some(i);
            }
        }
        {
            let i = last_comma?;
            candidate.truncate(i)
        }
    }
    None
}

fn try_close(s: &str) -> Option<Value> {
    let mut stack: Vec<char> = vec![];
    let mut in_str = false;
    let mut esc = false;
    for c in s.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut out = s.to_string();
    if in_str {
        if esc {
            out.pop();
        }
        out.push('"');
    }
    let trimmed = out.trim_end();
    if trimmed.ends_with(',') || trimmed.ends_with(':') {
        if trimmed.ends_with(':') {
            out = format!("{trimmed} null");
        } else {
            out = trimmed[..trimmed.len() - 1].to_string();
        }
    }
    while let Some(c) = stack.pop() {
        out.push(c);
    }
    serde_json::from_str::<Value>(&out).ok()
}

static CITE_RE: Lazy<Regex> = Lazy::new(|| Regex::new("\u{E000}(\\d+)\u{E001}").unwrap());

fn domain_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_else(|| url.to_string())
}

#[derive(Default)]
struct TextAcc {
    raw: String,
    sources: Vec<(String, String)>,
}

impl TextAcc {
    fn push_block(&mut self, b: &Value) {
        self.raw.push_str(b["text"].as_str().unwrap_or(""));
        if let Some(cits) = b["citations"].as_array() {
            let mut seen = HashSet::new();
            for c in cits {
                let url = c["url"].as_str().unwrap_or("");
                if crate::markdown::safe_url(url).is_none() || !seen.insert(url.to_string()) {
                    continue;
                }
                let title = c["title"].as_str().unwrap_or(url).to_string();
                let idx = match self.sources.iter().position(|(u, _)| u == url) {
                    Some(i) => i,
                    None => {
                        self.sources.push((url.to_string(), title));
                        self.sources.len() - 1
                    }
                };
                self.raw.push_str(&format!("\u{E000}{}\u{E001}", idx + 1));
            }
        }
    }
}

fn artifact_card(a: &artifacts::ArtifactSpec, version: u32) -> String {
    format!(
        "<div class=\"artifact-card\" role=\"button\" tabindex=\"0\" data-identifier=\"{id}\" data-version=\"{v}\" data-kind=\"{k}\" data-complete=\"{c}\"><div class=\"artifact-card-icon\"></div><div class=\"artifact-card-text\"><div class=\"artifact-card-title\">{t}</div><div class=\"artifact-card-sub\">{label}</div></div></div>",
        id = escape_html(&a.identifier),
        v = version,
        k = escape_html(&a.kind),
        c = a.complete,
        t = escape_html(if a.title.is_empty() { &a.identifier } else { &a.title }),
        label = escape_html(&artifacts::kind_label(&a.kind, a.language.as_deref())),
    )
}

fn finish_text(acc: TextAcc, ctx: &RenderCtx, tracker: &mut ArtifactTracker, artifacts_out: &mut Vec<Value>) -> Option<Value> {
    if acc.raw.trim().is_empty() {
        return None;
    }
    let mut html = String::new();
    let opts = Opts { math: ctx.math, breaks: true };
    for seg in artifacts::split(&acc.raw) {
        match seg {
            Segment::Markdown(md) => html.push_str(&markdown::render(&md, opts)),
            Segment::Artifact(a) => {
                let v = tracker.next(&a.identifier);
                html.push_str(&artifact_card(&a, v));
                artifacts_out.push(json!({
                    "identifier": a.identifier,
                    "version": v,
                    "type": a.kind,
                    "title": a.title,
                    "language": a.language,
                    "content": a.content,
                    "complete": a.complete,
                    "label": artifacts::kind_label(&a.kind, a.language.as_deref()),
                    "previewable": artifacts::is_previewable(&a.kind),
                    "ext": artifacts::extension(&a.kind, a.language.as_deref()),
                }));
            }
        }
    }
    if !acc.sources.is_empty() {
        let sources = acc.sources.clone();
        html = CITE_RE
            .replace_all(&html, |c: &regex::Captures| {
                let i: usize = c[1].parse().unwrap_or(1);
                match sources.get(i.saturating_sub(1)) {
                    Some((u, t)) => format!(
                        "<a class=\"cite\" href=\"{}\" title=\"{}\">{}</a>",
                        escape_html(u),
                        escape_html(t),
                        escape_html(&domain_of(u))
                    ),
                    None => String::new(),
                }
            })
            .to_string();
    } else if html.contains('\u{E000}') {
        html = CITE_RE.replace_all(&html, "").to_string();
    }
    let sources: Vec<Value> = acc
        .sources
        .iter()
        .map(|(u, t)| json!({"url": u, "title": t, "domain": domain_of(u)}))
        .collect();
    Some(json!({"kind": "text", "html": html, "sources": sources, "raw": acc.raw.replace(['\u{E000}', '\u{E001}'], "")}))
}

fn tool_result_view(block: &Value) -> Value {
    let is_error = block["is_error"].as_bool().unwrap_or(false);
    let mut text = String::new();
    let mut images = vec![];
    match &block["content"] {
        Value::String(s) => text.push_str(s),
        Value::Array(items) => {
            for it in items {
                match it["type"].as_str() {
                    Some("text") => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(it["text"].as_str().unwrap_or(""));
                    }
                    Some("image") => {
                        let src = &it["source"];
                        if src["type"] == "base64" {
                            images.push(format!(
                                "data:{};base64,{}",
                                src["media_type"].as_str().unwrap_or("image/png"),
                                src["data"].as_str().unwrap_or("")
                            ));
                        }
                    }
                    Some("document") => {
                        text.push_str(&format!("[document: {}]", it["title"].as_str().unwrap_or("file")));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    json!({"text": truncate_middle(&text, 40_000), "is_error": is_error, "images": images})
}

fn server_result_view(name: &str, block: &Value) -> Value {
    let content = &block["content"];
    match block["type"].as_str().unwrap_or("") {
        "web_search_tool_result" => {
            if let Some(arr) = content.as_array() {
                let results: Vec<Value> = arr
                    .iter()
                    .filter(|r| r["type"] == "web_search_result")
                    .filter(|r| crate::markdown::safe_url(r["url"].as_str().unwrap_or("")).is_some())
                    .map(|r| {
                        let url = r["url"].as_str().unwrap_or("");
                        json!({"title": r["title"], "url": url, "domain": domain_of(url), "page_age": r["page_age"]})
                    })
                    .collect();
                json!({"results": results})
            } else {
                json!({"error": content["error_code"].as_str().unwrap_or("error")})
            }
        }
        "web_fetch_tool_result" => {
            if content["type"] == "web_fetch_result" {
                json!({"url": content["url"], "title": content["content"]["title"]})
            } else {
                json!({"error": content["error_code"].as_str().unwrap_or("error")})
            }
        }
        _ => {
            // code execution and other server tools
            let mut text = String::new();
            if let Some(o) = content["stdout"].as_str() {
                text.push_str(o);
            }
            if let Some(e) = content["stderr"].as_str() {
                if !e.is_empty() {
                    text.push_str("\n[stderr]\n");
                    text.push_str(e);
                }
            }
            if text.is_empty() {
                text = serde_json::to_string_pretty(content).unwrap_or_default();
            }
            let err = content["type"].as_str().map(|t| t.ends_with("error")).unwrap_or(false);
            let _ = name;
            json!({"text": truncate_middle(&text, 20_000), "is_error": err})
        }
    }
}

fn user_parts(m: &Message) -> Vec<Value> {
    let mut parts = vec![];
    let mut text = String::new();
    let mut images: Vec<String> = vec![];
    if let Some(blocks) = m.content.as_array() {
        for b in blocks {
            match b["type"].as_str() {
                Some("text") => {
                    if b["internal"].as_bool() == Some(true) {
                        continue;
                    }
                    let t = b["text"].as_str().unwrap_or("");
                    if t.starts_with("<system-reminder>") {
                        continue;
                    }
                    if !text.is_empty() {
                        text.push_str("\n\n");
                    }
                    text.push_str(t);
                }
                Some("image") => {
                    let src = &b["source"];
                    if src["type"] == "base64" {
                        images.push(format!(
                            "data:{};base64,{}",
                            src["media_type"].as_str().unwrap_or("image/png"),
                            src["data"].as_str().unwrap_or("")
                        ));
                    }
                }
                _ => {}
            }
        }
    } else if let Some(s) = m.content.as_str() {
        text = s.to_string();
    }
    let atts = m.meta["attachments"].as_array().cloned().unwrap_or_default();
    let mut img_i = 0;
    let items: Vec<Value> = atts
        .into_iter()
        .map(|mut a| {
            if a["kind"] == "image" {
                if let Some(src) = images.get(img_i) {
                    a["src"] = json!(src);
                }
                img_i += 1;
            }
            a
        })
        .collect();
    if !items.is_empty() {
        parts.push(json!({"kind": "attachments", "items": items}));
    }
    if !text.is_empty() {
        parts.push(json!({"kind": "user_text", "text": text}));
    }
    parts
}

/// Group a root-to-leaf path into UI turns.
pub fn build_turns(path: &[Message], ctx: &RenderCtx, tracker: &mut ArtifactTracker) -> Vec<Value> {
    let mut groups: Vec<Vec<&Message>> = vec![];
    for m in path {
        let is_user_turn = m.role == "user" && !is_tool_result_msg(m);
        let new_group = match groups.last() {
            None => true,
            Some(g) => {
                let last_is_user = g[0].role == "user";
                if is_user_turn {
                    true
                } else {
                    last_is_user
                }
            }
        };
        if new_group {
            groups.push(vec![m]);
        } else {
            groups.last_mut().unwrap().push(m);
        }
    }
    groups
        .into_iter()
        .map(|g| {
            if g[0].role == "user" {
                user_turn(g[0])
            } else {
                assistant_turn(&g, ctx, tracker)
            }
        })
        .collect()
}

fn user_turn(m: &Message) -> Value {
    json!({
        "id": m.id,
        "role": "user",
        "msg_ids": [m.id],
        "parent_id": m.parent_id,
        "parts": user_parts(m),
        "created_at": m.created_at,
        "meta": {"plan_mode": m.meta["plan_mode"], "checkpoint": m.meta["checkpoint"].is_string(),
                 "compact_summary": m.meta["compact_summary"], "compact_auto": m.meta["compact_auto"]},
    })
}

pub fn assistant_turn(msgs: &[&Message], ctx: &RenderCtx, tracker: &mut ArtifactTracker) -> Value {
    // Collect tool results from tool_result user messages in the turn.
    let mut results: HashMap<String, Value> = HashMap::new();
    for m in msgs.iter().filter(|m| m.role == "user") {
        if let Some(arr) = m.content.as_array() {
            for b in arr {
                if b["type"] == "tool_result" {
                    if let Some(id) = b["tool_use_id"].as_str() {
                        results.insert(id.to_string(), b.clone());
                    }
                }
            }
        }
    }
    let mut parts: Vec<Value> = vec![];
    let mut artifacts_out: Vec<Value> = vec![];
    let mut text: Option<TextAcc> = None;
    let mut msg_ids = vec![];
    let mut streaming = false;
    let mut usage_in = 0u64;
    let mut usage_out = 0u64;
    let mut usage_cache_read = 0u64;
    let mut usage_cache_write = 0u64;
    let mut cost_usd = 0.0f64;
    let mut duration_ms = 0u64;
    let mut last_meta = Value::Null;

    macro_rules! flush_text {
        () => {
            if let Some(acc) = text.take() {
                if let Some(p) = finish_text(acc, ctx, tracker, &mut artifacts_out) {
                    parts.push(p);
                }
            }
        };
    }

    for m in msgs {
        msg_ids.push(m.id.clone());
        if m.role != "assistant" {
            continue;
        }
        let is_streaming = ctx.streaming_msg == Some(m.id.as_str());
        streaming |= is_streaming;
        last_meta = m.meta.clone();
        if let Some(u) = m.meta.get("usage") {
            usage_in += u["input_tokens"].as_u64().unwrap_or(0);
            usage_out += u["output_tokens"].as_u64().unwrap_or(0);
            usage_cache_read += u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            usage_cache_write += u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        }
        // what this answer cost: the CLI reports it; otherwise list prices
        // (sub-agents included; local models are free)
        let model = m.meta["model"].as_str().unwrap_or("");
        if let Some(c) = m.meta["cost_usd"].as_f64() {
            cost_usd += c;
        } else if !model.is_empty() && !model.starts_with(crate::oai::PREFIX) {
            for key in ["usage", "subagent_usage"] {
                if m.meta[key].is_object() {
                    cost_usd += crate::commands::usage_cost(model, &m.meta[key]);
                }
            }
        }
        duration_ms += m.meta["duration_ms"].as_u64().unwrap_or(0);
        let blocks = m.content.as_array().cloned().unwrap_or_default();
        let server_results: HashMap<String, Value> = blocks
            .iter()
            .filter(|b| b["type"].as_str().map(|t| t.ends_with("_tool_result")).unwrap_or(false))
            .filter_map(|b| b["tool_use_id"].as_str().map(|id| (id.to_string(), b.clone())))
            .collect();
        let n = blocks.len();
        for (bi, b) in blocks.iter().enumerate() {
            let last_block = bi + 1 == n;
            match b["type"].as_str().unwrap_or("") {
                "text" => {
                    text.get_or_insert_with(TextAcc::default).push_block(b);
                }
                "thinking" | "redacted_thinking" => {
                    flush_text!();
                    let t = b["thinking"].as_str().unwrap_or("");
                    let redacted = b["type"] == "redacted_thinking";
                    let live = is_streaming && last_block;
                    if let Some(prev) = parts.last_mut().filter(|p| p["kind"] == "thinking") {
                        let raw = format!("{}\n\n{}", prev["raw"].as_str().unwrap_or(""), t);
                        prev["html"] = json!(markdown::render(raw.trim(), Opts { math: ctx.math, breaks: true }));
                        prev["raw"] = json!(raw);
                        prev["streaming"] = json!(live);
                    } else {
                        parts.push(json!({
                            "kind": "thinking",
                            "html": markdown::render(t, Opts { math: ctx.math, breaks: true }),
                            "raw": t,
                            "redacted": redacted,
                            "streaming": live,
                        }));
                    }
                }
                "tool_use" => {
                    flush_text!();
                    let id = b["id"].as_str().unwrap_or("").to_string();
                    let name = b["name"].as_str().unwrap_or("").to_string();
                    let (server, tool) = match ctx.mcp_names.get(&name) {
                        Some((s, t)) => (Some(s.clone()), t.clone()),
                        None => (None, name.clone()),
                    };
                    let (status, result) = match results.get(&id) {
                        Some(r) => {
                            let rv = tool_result_view(r);
                            let st = if rv["is_error"].as_bool() == Some(true) { "error" } else { "done" };
                            (st.to_string(), rv)
                        }
                        None => {
                            let st = if ctx.streaming_tools.contains(&id) {
                                "streaming".to_string()
                            } else if let Some(s) = ctx.live_tools.get(&id) {
                                s.clone()
                            } else if is_streaming {
                                "pending".to_string()
                            } else {
                                "cancelled".to_string()
                            };
                            (st, Value::Null)
                        }
                    };
                    parts.push(json!({
                        "kind": "tool",
                        "id": id,
                        "name": name,
                        "tool": tool,
                        "server": server,
                        "input": b["input"],
                        "status": status,
                        "result": result,
                    }));
                }
                "server_tool_use" => {
                    flush_text!();
                    let id = b["id"].as_str().unwrap_or("").to_string();
                    let name = b["name"].as_str().unwrap_or("").to_string();
                    let res = server_results.get(&id);
                    let status = if res.is_some() {
                        "done"
                    } else if is_streaming {
                        "running"
                    } else {
                        "cancelled"
                    };
                    let rv = res.map(|r| server_result_view(&name, r)).unwrap_or(Value::Null);
                    match name.as_str() {
                        "web_search" => parts.push(json!({
                            "kind": "search", "id": id, "query": b["input"]["query"],
                            "status": if rv["error"].is_string() { "error" } else { status },
                            "results": rv["results"], "error": rv["error"],
                        })),
                        "web_fetch" => parts.push(json!({
                            "kind": "fetch", "id": id, "url": b["input"]["url"],
                            "status": if rv["error"].is_string() { "error" } else { status },
                            "title": rv["title"], "error": rv["error"],
                        })),
                        _ => parts.push(json!({
                            "kind": "tool", "id": id, "name": name, "tool": name, "server": "anthropic",
                            "input": b["input"], "status": status, "result": rv,
                        })),
                    }
                }
                "fallback" => {
                    flush_text!();
                    parts.push(json!({
                        "kind": "notice", "code": "fallback",
                        "from": b["from"]["model"], "to": b["to"]["model"],
                    }));
                }
                "compaction" => {
                    flush_text!();
                    parts.push(json!({"kind": "notice", "code": "compaction"}));
                }
                _ => {}
            }
        }
    }
    flush_text!();

    let first = msgs.iter().find(|m| m.role == "assistant").unwrap_or(&msgs[0]);
    let stop_reason = last_meta["stop_reason"].clone();
    if stop_reason == "refusal" {
        parts.push(json!({"kind": "notice", "code": "refusal", "details": last_meta["stop_details"]}));
    } else if stop_reason == "max_tokens" {
        parts.push(json!({"kind": "notice", "code": "max_tokens"}));
    }
    if let Some(err) = last_meta.get("error").filter(|e| !e.is_null()) {
        parts.push(json!({"kind": "notice", "code": "error", "error": err}));
    }
    // client-side switch to another model because the chosen one was overloaded
    if let Some(fb) = msgs.iter().filter(|m| m.role == "assistant").find_map(|m| m.meta.get("overload_fallback").filter(|v| v.is_object())) {
        parts.insert(0, json!({"kind": "notice", "code": "overloaded_fallback", "from": fb["from"], "to": fb["to"]}));
    }
    if last_meta["budget_stop"].is_object() {
        parts.push(json!({"kind": "notice", "code": "budget", "budget": last_meta["budget_stop"]["budget"], "spent": last_meta["budget_stop"]["spent"]}));
    }
    if last_meta["stopped"].as_bool() == Some(true) {
        parts.push(json!({"kind": "notice", "code": "stopped"}));
    }
    let model = last_meta["model"].clone();
    json!({
        "id": first.id,
        "role": "assistant",
        "msg_ids": msg_ids,
        "parent_id": first.parent_id,
        "parts": parts,
        "artifacts": artifacts_out,
        "streaming": streaming,
        "created_at": first.created_at,
        "meta": {
            "model": model,
            "stop_reason": stop_reason,
            "usage": {
                "input_tokens": usage_in,
                "output_tokens": usage_out,
                "cache_read_input_tokens": usage_cache_read,
                "cache_creation_input_tokens": usage_cache_write,
            },
            "feedback": last_meta["feedback"],
            "cost_usd": cost_usd,
            "duration_ms": duration_ms,
        },
    })
}

/// Plain text of a message (for search indexing and copying).
pub fn plain_text(content: &Value) -> String {
    let mut out = String::new();
    if let Some(arr) = content.as_array() {
        for b in arr {
            if b["type"] == "text" {
                let t = b["text"].as_str().unwrap_or("");
                if t.starts_with("<system-reminder>") {
                    continue;
                }
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
    } else if let Some(s) = content.as_str() {
        out.push_str(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json() {
        assert_eq!(close_partial_json(r#"{"a": "he"#).unwrap()["a"], "he");
        assert_eq!(close_partial_json(r#"{"a": 1, "b"#).unwrap()["a"], 1);
        assert_eq!(close_partial_json(r#"{"a": [1, 2"#).unwrap()["a"][1], 2);
        assert_eq!(close_partial_json(r#"{"a": "x\"#).unwrap()["a"], "x");
        assert!(close_partial_json("").unwrap().is_object());
    }

    fn msg(id: &str, role: &str, content: Value, meta: Value) -> Message {
        Message {
            id: id.into(),
            conversation_id: "c".into(),
            parent_id: None,
            role: role.into(),
            content,
            meta,
            created_at: 0,
        }
    }

    #[test]
    fn groups_tool_loop_into_one_turn() {
        let path = vec![
            msg("u1", "user", json!([{"type":"text","text":"hi"}]), json!({})),
            msg(
                "a1",
                "assistant",
                json!([{"type":"text","text":"Let me look."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"x"}}]),
                json!({"stop_reason":"tool_use"}),
            ),
            msg(
                "u2",
                "user",
                json!([{"type":"tool_result","tool_use_id":"t1","content":"file body"}]),
                json!({"tool_results": true}),
            ),
            msg(
                "a2",
                "assistant",
                json!([{"type":"text","text":"Done. <antArtifact identifier=\"a\" type=\"text/html\" title=\"Page\"><p>x</p></antArtifact>","citations":[{"type":"web_search_result_location","url":"https://www.rust-lang.org/x","title":"Rust"}]}]),
                json!({"stop_reason":"end_turn"}),
            ),
        ];
        let lt = HashMap::new();
        let st = HashSet::new();
        let mn = HashMap::new();
        let ctx = RenderCtx {
            math: true,
            live_tools: &lt,
            streaming_tools: &st,
            mcp_names: &mn,
            streaming_msg: None,
        };
        let mut tr = ArtifactTracker::default();
        let turns = build_turns(&path, &ctx, &mut tr);
        assert_eq!(turns.len(), 2);
        let parts = turns[1]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["kind"], "text");
        assert_eq!(parts[1]["kind"], "tool");
        assert_eq!(parts[1]["status"], "done");
        assert_eq!(parts[1]["result"]["text"], "file body");
        assert_eq!(parts[2]["kind"], "text");
        assert!(parts[2]["html"].as_str().unwrap().contains("artifact-card"));
        assert!(parts[2]["html"].as_str().unwrap().contains("rust-lang.org"));
        assert_eq!(turns[1]["artifacts"][0]["version"], 1);
        assert_eq!(turns[1]["msg_ids"].as_array().unwrap().len(), 3);
    }
}
