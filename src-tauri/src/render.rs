//! Turns stored API messages into UI "turns" made of display parts.

use crate::artifacts::{self, Segment};
use crate::db::Message;
use crate::markdown::{self, Opts};
use crate::util::{escape_html, truncate_middle};
use std::sync::LazyLock;
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

impl RenderCtx<'_> {
    /// A tool call's server (MCP tools only) and tool name: the app's MCP tools by their API name, Claude Code's
    /// by its `mcp__<server>__<tool>`.
    fn tool_names(&self, name: &str) -> (Option<String>, String) {
        if let Some((s, t)) = self.mcp_names.get(name) {
            return (Some(s.clone()), t.clone());
        }
        match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            Some((s, t)) if !s.is_empty() && !t.is_empty() => (Some(s.into()), t.into()),
            _ => (None, name.into()),
        }
    }
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

static CITE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("\u{E000}(\\d+)\u{E001}").unwrap());

fn domain_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_else(|| url.to_string())
}

#[derive(Default, Clone)]
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

/// `live`: the text ends with a block that is still streaming, so its last
/// part changes on every update and isn't worth a place in the render cache.
fn finish_text(acc: TextAcc, ctx: &RenderCtx, tracker: &mut ArtifactTracker, artifacts_out: &mut Vec<Value>, live: bool) -> Option<Value> {
    if acc.raw.trim().is_empty() {
        return None;
    }
    let mut html = String::new();
    // the first piece is taken as it is (most texts are one piece)
    let mut add = |piece: String| {
        if html.is_empty() {
            html = piece;
        } else {
            html.push_str(&piece);
        }
    };
    let opts = Opts { math: ctx.math, breaks: true };
    let segments = artifacts::split(&acc.raw);
    let last = segments.len().saturating_sub(1);
    for (i, seg) in segments.into_iter().enumerate() {
        match seg {
            Segment::Markdown(md) if live && i == last => add(markdown::render_live(&md, opts)),
            Segment::Markdown(md) => add(markdown::render(&md, opts)),
            Segment::Artifact(a) => {
                let v = tracker.next(&a.identifier);
                add(artifact_card(&a, v));
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
        let sources = &acc.sources;
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
    let markers = ['\u{E000}', '\u{E001}'];
    let raw = if acc.raw.contains(markers) { acc.raw.replace(markers, "") } else { acc.raw };
    // moved in: `json!` would copy them, and they are most of a text part
    let mut part = json!({"kind": "text"});
    part["html"] = Value::String(html);
    part["sources"] = Value::Array(sources);
    part["raw"] = Value::String(raw);
    Some(part)
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

/// Tool results of a turn's tool_result messages, by tool_use id (a later
/// result for the same id wins).
fn tool_results<'a>(msgs: impl IntoIterator<Item = &'a Message>) -> HashMap<&'a str, &'a Value> {
    let mut results = HashMap::new();
    for m in msgs.into_iter().filter(|m| m.role == "user") {
        for b in m.content.as_array().into_iter().flatten() {
            if b["type"] == "tool_result" {
                if let Some(id) = b["tool_use_id"].as_str() {
                    results.insert(id, b);
                }
            }
        }
    }
    results
}

/// Status of a tool call that has no result (yet).
fn pending_tool_status(id: &str, ctx: &RenderCtx, is_streaming: bool) -> String {
    if ctx.streaming_tools.contains(id) {
        "streaming".to_string()
    } else if let Some(s) = ctx.live_tools.get(id) {
        s.clone()
    } else if is_streaming {
        "pending".to_string()
    } else {
        "cancelled".to_string()
    }
}

/// The message a turn takes its id, parent and time from.
#[derive(Clone, Default)]
struct Head {
    id: String,
    parent_id: Option<String>,
    created_at: i64,
}

impl Head {
    fn of(m: &Message) -> Head {
        Head { id: m.id.clone(), parent_id: m.parent_id.clone(), created_at: m.created_at }
    }
}

/// Builds the UI turn of an assistant turn one message at a time, so the
/// live renderer can keep the state after a turn's finished messages and
/// add only the message being streamed on each update (`TurnPrefix`).
#[derive(Default)]
struct TurnBuilder {
    tracker: ArtifactTracker,
    parts: Vec<Value>,
    artifacts: Vec<Value>,
    text: Option<TextAcc>,
    /// the pending text ends with the last block of a streaming message
    text_live: bool,
    msg_ids: Vec<Value>,
    streaming: bool,
    usage_in: u64,
    usage_out: u64,
    usage_cache_read: u64,
    usage_cache_write: u64,
    cost_usd: f64,
    // the Claude Code CLI reports a whole run's cost on its last row; it
    // replaces the estimates of that run's other rows
    reported_cost: f64,
    duration_ms: u64,
    last_meta: Value,
    /// client-side switch to another model (the first assistant message's)
    overload: Option<Value>,
    first_assistant: Option<Head>,
    first_message: Option<Head>,
}

impl TurnBuilder {
    fn new(tracker: ArtifactTracker) -> Self {
        TurnBuilder { tracker, ..Default::default() }
    }

    /// A builder that goes on from this one's state. The parts, artifacts
    /// and message ids (the bulk of it) are moved, not copied: `TurnPrefix`
    /// takes them back from the rendered turn.
    fn lend(&mut self) -> TurnBuilder {
        TurnBuilder {
            tracker: self.tracker.clone(),
            parts: std::mem::take(&mut self.parts),
            artifacts: std::mem::take(&mut self.artifacts),
            text: self.text.clone(),
            text_live: self.text_live,
            msg_ids: std::mem::take(&mut self.msg_ids),
            streaming: self.streaming,
            usage_in: self.usage_in,
            usage_out: self.usage_out,
            usage_cache_read: self.usage_cache_read,
            usage_cache_write: self.usage_cache_write,
            cost_usd: self.cost_usd,
            reported_cost: self.reported_cost,
            duration_ms: self.duration_ms,
            last_meta: self.last_meta.clone(),
            overload: self.overload.clone(),
            first_assistant: self.first_assistant.clone(),
            first_message: self.first_message.clone(),
        }
    }

    fn flush_text(&mut self, ctx: &RenderCtx) {
        if let Some(acc) = self.text.take() {
            let live = std::mem::take(&mut self.text_live);
            if let Some(p) = finish_text(acc, ctx, &mut self.tracker, &mut self.artifacts, live) {
                self.parts.push(p);
            }
        }
    }

    fn push(&mut self, m: &Message, ctx: &RenderCtx, results: &HashMap<&str, &Value>) {
        if self.first_message.is_none() {
            self.first_message = Some(Head::of(m));
        }
        self.msg_ids.push(Value::String(m.id.clone()));
        if m.role != "assistant" {
            return;
        }
        // Claude Code shows each of its messages apart: a workflow's report isn't run into the answer before it
        if m.meta["engine"] == crate::claudecode::ENGINE {
            self.flush_text(ctx);
        }
        if self.first_assistant.is_none() {
            self.first_assistant = Some(Head::of(m));
        }
        if self.overload.is_none() {
            self.overload = m.meta.get("overload_fallback").filter(|v| v.is_object()).cloned();
        }
        let is_streaming = ctx.streaming_msg == Some(m.id.as_str());
        self.streaming |= is_streaming;
        self.last_meta = m.meta.clone();
        if let Some(u) = m.meta.get("usage") {
            self.usage_in += u["input_tokens"].as_u64().unwrap_or(0);
            self.usage_out += u["output_tokens"].as_u64().unwrap_or(0);
            self.usage_cache_read += u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            self.usage_cache_write += u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        }
        // what this answer cost: the CLI reports it; otherwise list prices
        // (sub-agents included; local models are free)
        let model = m.meta["model"].as_str().unwrap_or("");
        if let Some(c) = m.meta["cost_usd"].as_f64() {
            self.reported_cost += c;
        } else if !model.is_empty() && !model.starts_with(crate::oai::PREFIX) {
            for key in ["usage", "subagent_usage"] {
                if m.meta[key].is_object() {
                    self.cost_usd += crate::commands::usage_cost(model, &m.meta[key]);
                }
            }
        }
        self.duration_ms += m.meta["duration_ms"].as_u64().unwrap_or(0);
        let blocks: &[Value] = m.content.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let server_results: HashMap<&str, &Value> = blocks
            .iter()
            .filter(|b| b["type"].as_str().map(|t| t.ends_with("_tool_result")).unwrap_or(false))
            .filter_map(|b| b["tool_use_id"].as_str().map(|id| (id, b)))
            .collect();
        let n = blocks.len();
        for (bi, b) in blocks.iter().enumerate() {
            let last_block = bi + 1 == n;
            match b["type"].as_str().unwrap_or("") {
                "text" => {
                    self.text.get_or_insert_with(TextAcc::default).push_block(b);
                    self.text_live = is_streaming && last_block;
                }
                "thinking" | "redacted_thinking" => {
                    self.flush_text(ctx);
                    let t = b["thinking"].as_str().unwrap_or("");
                    let redacted = b["type"] == "redacted_thinking";
                    let live = is_streaming && last_block;
                    // thinking still streaming changes on every update: not worth caching
                    let md = |s: &str| {
                        let opts = Opts { math: ctx.math, breaks: true };
                        if live {
                            markdown::render_live(s, opts)
                        } else {
                            markdown::render(s, opts)
                        }
                    };
                    if let Some(prev) = self.parts.last_mut().filter(|p| p["kind"] == "thinking") {
                        let raw = format!("{}\n\n{}", prev["raw"].as_str().unwrap_or(""), t);
                        prev["html"] = Value::String(md(raw.trim()));
                        prev["raw"] = Value::String(raw);
                        prev["streaming"] = json!(live);
                    } else {
                        let mut part = json!({
                            "kind": "thinking",
                            "raw": t,
                            "redacted": redacted,
                            "streaming": live,
                        });
                        part["html"] = Value::String(md(t));
                        self.parts.push(part);
                    }
                }
                "tool_use" => {
                    self.flush_text(ctx);
                    let id = b["id"].as_str().unwrap_or("").to_string();
                    let name = b["name"].as_str().unwrap_or("").to_string();
                    let (server, tool) = ctx.tool_names(&name);
                    let (status, result) = match results.get(id.as_str()) {
                        Some(r) => {
                            let rv = tool_result_view(r);
                            let st = if rv["is_error"].as_bool() == Some(true) { "error" } else { "done" };
                            (st.to_string(), rv)
                        }
                        None => (pending_tool_status(&id, ctx, is_streaming), Value::Null),
                    };
                    self.parts.push(json!({
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
                    self.flush_text(ctx);
                    let id = b["id"].as_str().unwrap_or("").to_string();
                    let name = b["name"].as_str().unwrap_or("").to_string();
                    let res = server_results.get(id.as_str());
                    let status = if res.is_some() {
                        "done"
                    } else if is_streaming {
                        "running"
                    } else {
                        "cancelled"
                    };
                    let rv = res.map(|r| server_result_view(&name, r)).unwrap_or(Value::Null);
                    match name.as_str() {
                        "web_search" => self.parts.push(json!({
                            "kind": "search", "id": id, "query": b["input"]["query"],
                            "status": if rv["error"].is_string() { "error" } else { status },
                            "results": rv["results"], "error": rv["error"],
                        })),
                        "web_fetch" => self.parts.push(json!({
                            "kind": "fetch", "id": id, "url": b["input"]["url"],
                            "status": if rv["error"].is_string() { "error" } else { status },
                            "title": rv["title"], "error": rv["error"],
                        })),
                        _ => self.parts.push(json!({
                            "kind": "tool", "id": id, "name": name, "tool": name, "server": "anthropic",
                            "input": b["input"], "status": status, "result": rv,
                        })),
                    }
                }
                "fallback" => {
                    self.flush_text(ctx);
                    self.parts.push(json!({
                        "kind": "notice", "code": "fallback",
                        "from": b["from"]["model"], "to": b["to"]["model"],
                    }));
                }
                "compaction" => {
                    self.flush_text(ctx);
                    self.parts.push(json!({"kind": "notice", "code": "compaction"}));
                }
                _ => {}
            }
        }
    }

    /// The finished turn, and the artifact versions counted up to its end.
    fn finish(mut self, ctx: &RenderCtx) -> (Value, ArtifactTracker) {
        self.flush_text(ctx);
        let first = self.first_assistant.take().or(self.first_message.take()).unwrap_or_default();
        let last_meta = &self.last_meta;
        let mut parts = self.parts;
        let stop_reason = last_meta["stop_reason"].clone();
        if stop_reason == "refusal" {
            parts.push(json!({"kind": "notice", "code": "refusal", "details": last_meta["stop_details"]}));
        } else if stop_reason == "max_tokens" {
            parts.push(json!({"kind": "notice", "code": "max_tokens"}));
        }
        if let Some(err) = last_meta.get("error").filter(|e| !e.is_null()) {
            parts.push(json!({"kind": "notice", "code": "error", "error": err}));
        }
        if let Some(fb) = &self.overload {
            parts.insert(0, json!({"kind": "notice", "code": "overloaded_fallback", "from": fb["from"], "to": fb["to"]}));
        }
        if last_meta["budget_stop"].is_object() {
            parts.push(json!({"kind": "notice", "code": "budget", "budget": last_meta["budget_stop"]["budget"], "spent": last_meta["budget_stop"]["spent"]}));
        }
        if last_meta["stopped"].as_bool() == Some(true) {
            parts.push(json!({"kind": "notice", "code": "stopped"}));
        }
        let model = last_meta["model"].clone();
        let mut t = json!({
            "id": first.id,
            "role": "assistant",
            "parent_id": first.parent_id,
            "streaming": self.streaming,
            "created_at": first.created_at,
            "meta": {
                "model": model,
                "stop_reason": stop_reason,
                "usage": {
                    "input_tokens": self.usage_in,
                    "output_tokens": self.usage_out,
                    "cache_read_input_tokens": self.usage_cache_read,
                    "cache_creation_input_tokens": self.usage_cache_write,
                },
                "feedback": last_meta["feedback"],
                "cost_usd": if self.reported_cost > 0.0 { self.reported_cost } else { self.cost_usd },
                "duration_ms": self.duration_ms,
            },
        });
        // moved in (not copied: they are most of the turn)
        t["msg_ids"] = Value::Array(self.msg_ids);
        t["parts"] = Value::Array(parts);
        t["artifacts"] = Value::Array(self.artifacts);
        (t, self.tracker)
    }
}

pub fn assistant_turn(msgs: &[&Message], ctx: &RenderCtx, tracker: &mut ArtifactTracker) -> Value {
    let results = tool_results(msgs.iter().copied());
    let mut b = TurnBuilder::new(std::mem::take(tracker));
    for m in msgs {
        b.push(m, ctx, &results);
    }
    let (t, counted) = b.finish(ctx);
    *tracker = counted;
    t
}

/// What a render lent out of a `TurnPrefix`, to take it back.
struct Lent {
    parts: usize,
    artifacts: usize,
    msg_ids: usize,
    overload: bool,
    /// the last part as it was, when the new message may have changed it
    /// (thinking that goes on in the next message is merged into it)
    last_part: Option<Value>,
}

/// A turn's finished messages rendered once, to render the turn again with
/// a different last message: while a reply streams, the turn is rendered
/// every 45 ms and only the reply changes, so the rest of the turn (tool
/// calls with their outputs, text with code blocks) isn't rendered again on
/// every update. The result is the same as `assistant_turn` over all the
/// messages; whatever the cached part depends on is checked by `fits`.
pub struct TurnPrefix {
    built: TurnBuilder,
    math: bool,
    /// tool calls of the prefix that have a result in it
    answered: HashSet<String>,
    /// tool calls of the prefix without a result, with the status they were
    /// rendered with (it comes from the live state)
    unanswered: Vec<(String, String)>,
    lent: Option<Lent>,
}

impl TurnPrefix {
    /// `prefix`: the turn's messages before the one that changes; `ctx`
    /// must keep the same `mcp_names` for as long as this is used.
    pub fn new(prefix: &[Message], ctx: &RenderCtx, tracker: &ArtifactTracker) -> TurnPrefix {
        let results = tool_results(prefix);
        let mut built = TurnBuilder::new(tracker.clone());
        for m in prefix {
            built.push(m, ctx, &results);
        }
        let mut unanswered = vec![];
        for m in prefix.iter().filter(|m| m.role == "assistant") {
            let is_streaming = ctx.streaming_msg == Some(m.id.as_str());
            for b in m.content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                let id = b["id"].as_str().unwrap_or("");
                if !results.contains_key(id) {
                    unanswered.push((id.to_string(), pending_tool_status(id, ctx, is_streaming)));
                }
            }
        }
        TurnPrefix {
            built,
            math: ctx.math,
            answered: results.keys().map(|k| k.to_string()).collect(),
            unanswered,
            lent: None,
        }
    }

    /// Whether `render` with this context gives what a full render would:
    /// the live state the cached part was rendered with is unchanged.
    pub fn fits(&self, prefix: &[Message], ctx: &RenderCtx) -> bool {
        self.lent.is_none()
            && self.math == ctx.math
            && ctx.streaming_msg.is_none_or(|s| !prefix.iter().any(|m| m.id == s))
            && self.unanswered.iter().all(|(id, status)| *status == pending_tool_status(id, ctx, false))
    }

    /// The turn of `prefix` (the same messages this was made from) and
    /// `current`, an assistant message. Hand the result to `give_back` once
    /// it has been sent, so the next render needn't start over.
    pub fn render(&mut self, prefix: &[Message], current: &Message, ctx: &RenderCtx) -> Value {
        let mut b = self.built.lend();
        let lent = Lent {
            parts: b.parts.len(),
            artifacts: b.artifacts.len(),
            msg_ids: b.msg_ids.len(),
            overload: false,
            last_part: b.parts.last().filter(|p| p["kind"] == "thinking").cloned(),
        };
        // results for the new message's tool calls (normally none: they
        // come in the message after it)
        let asks_answered = current
            .content
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| b["type"] == "tool_use" && b["id"].as_str().is_some_and(|id| self.answered.contains(id)));
        let results = if asks_answered { tool_results(prefix) } else { HashMap::new() };
        b.push(current, ctx, &results);
        let overload = b.overload.is_some();
        let (t, _) = b.finish(ctx);
        self.lent = Some(Lent { overload, ..lent });
        t
    }

    /// Take back the prefix's parts from a turn `render` returned. `false`
    /// when they can't be (the turn was changed): this must be made anew.
    pub fn give_back(&mut self, mut turn: Value) -> bool {
        // nothing lent: not a turn of this prefix (kept as it is)
        let Some(lent) = self.lent.take() else { return true };
        let (Value::Array(mut parts), Value::Array(mut artifacts), Value::Array(mut msg_ids)) =
            (turn["parts"].take(), turn["artifacts"].take(), turn["msg_ids"].take())
        else {
            return false;
        };
        if lent.overload {
            if parts.is_empty() {
                return false;
            }
            parts.remove(0);
        }
        if parts.len() < lent.parts || artifacts.len() < lent.artifacts || msg_ids.len() < lent.msg_ids {
            return false;
        }
        parts.truncate(lent.parts);
        if let (Some(p), Some(last)) = (lent.last_part, parts.last_mut()) {
            *last = p;
        }
        artifacts.truncate(lent.artifacts);
        msg_ids.truncate(lent.msg_ids);
        self.built.parts = parts;
        self.built.artifacts = artifacts;
        self.built.msg_ids = msg_ids;
        true
    }
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

    #[test]
    fn mcp_tools_show_their_server_and_tool() {
        let (lt, st) = (HashMap::new(), HashSet::new());
        let mut mn = HashMap::new();
        mn.insert("weather__get_forecast".to_string(), ("my weather".to_string(), "get_forecast".to_string()));
        let ctx = RenderCtx { math: false, live_tools: &lt, streaming_tools: &st, mcp_names: &mn, streaming_msg: None };
        let names = |n: &str| ctx.tool_names(n);
        assert_eq!(names("weather__get_forecast"), (Some("my weather".into()), "get_forecast".into()));
        // Claude Code's name for an MCP tool (a chat's SDK server, or a Code session's own server)
        assert_eq!(names("mcp__weather__get_forecast"), (Some("weather".into()), "get_forecast".into()));
        assert_eq!(names("mcp__claude_ai_Gmail__search__threads"), (Some("claude_ai_Gmail".into()), "search__threads".into()));
        for plain in ["Bash", "AskUserQuestion", "mcp__", "mcp__x", "mcp____t", "mcp__s__"] {
            assert_eq!(names(plain), (None, plain.into()), "{plain}");
        }
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
        // Claude Code: an answer and the workflow report that follows it stay two texts; the built-in agent's
        // text that goes on after a Stop hook stays one
        let two = |engine: &str| {
            let meta = json!({"engine": engine});
            let path = vec![
                msg("u1", "user", json!([{"type":"text","text":"go"}]), json!({})),
                msg("a1", "assistant", json!([{"type":"text","text":"Started."}]), meta.clone()),
                msg("a2", "assistant", json!([{"type":"text","text":"It finished."}]), meta),
            ];
            build_turns(&path, &ctx, &mut ArtifactTracker::default())[1]["parts"].as_array().unwrap().iter().filter(|p| p["kind"] == "text").count()
        };
        assert_eq!(two(crate::claudecode::ENGINE), 2);
        assert_eq!(two("builtin"), 1);
        assert_eq!(turns[1]["artifacts"][0]["version"], 1);
        assert_eq!(turns[1]["msg_ids"].as_array().unwrap().len(), 3);
    }

    /// One render of a turn: the live state it is rendered with.
    struct Live {
        tools: HashMap<String, String>,
        streaming: HashSet<String>,
        /// skip `give_back` after this render (the next one starts over)
        keep: bool,
    }

    fn live(tools: &[(&str, &str)], streaming: &[&str]) -> Live {
        Live {
            tools: tools.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            streaming: streaming.iter().map(|s| s.to_string()).collect(),
            keep: false,
        }
    }

    /// Render `prefix` + each of `currents` with a kept `TurnPrefix` (as the
    /// live renderer does) and with the old renderer: byte-for-byte equal.
    /// The new `assistant_turn` must equal the old one too (with the
    /// artifact versions it counts).
    fn same_as_before(prefix: &[Message], currents: &[(Message, Live)], tracker: &ArtifactTracker) {
        let mut mcp = HashMap::new();
        mcp.insert("mcp__github__get_issue".to_string(), ("github".to_string(), "get_issue".to_string()));
        for math in [true, false] {
            let mut kept: Option<TurnPrefix> = None;
            for (i, (cur, l)) in currents.iter().enumerate() {
                let ctx = RenderCtx { math, live_tools: &l.tools, streaming_tools: &l.streaming, mcp_names: &mcp, streaming_msg: Some(&cur.id) };
                let mut msgs: Vec<&Message> = prefix.iter().collect();
                msgs.push(cur);
                let mut old_tracker = tracker.clone();
                let want = serde_json::to_string(&reference::assistant_turn(&msgs, &ctx, &mut old_tracker)).unwrap();
                let mut new_tracker = tracker.clone();
                let full = serde_json::to_string(&assistant_turn(&msgs, &ctx, &mut new_tracker)).unwrap();
                assert_eq!(full, want, "assistant_turn, render {i}");
                assert_eq!(format!("{:?}", sorted(&new_tracker)), format!("{:?}", sorted(&old_tracker)));
                let p = match kept.take() {
                    Some(p) if p.fits(prefix, &ctx) => kept.insert(p),
                    _ => kept.insert(TurnPrefix::new(prefix, &ctx, tracker)),
                };
                let t = p.render(prefix, cur, &ctx);
                assert_eq!(serde_json::to_string(&t).unwrap(), want, "TurnPrefix, render {i} (math {math})");
                if !l.keep && !p.give_back(t) {
                    kept = None;
                }
            }
        }
    }

    fn sorted(t: &ArtifactTracker) -> Vec<(String, u32)> {
        let mut v: Vec<(String, u32)> = t.versions.iter().map(|(k, v)| (k.clone(), *v)).collect();
        v.sort();
        v
    }

    /// The versions of a reply as it streams in: its content cut after each
    /// block, and each text block cut at a few lengths.
    fn growing(id: &str, content: &[Value], meta: Value) -> Vec<Message> {
        let mut out = vec![];
        for n in 0..=content.len() {
            let mut blocks: Vec<Value> = content[..n].to_vec();
            if let Some(last) = blocks.last().cloned() {
                for key in ["text", "thinking"] {
                    if let Some(s) = last[key].as_str() {
                        let cuts: Vec<usize> = (1..4).map(|k| s.len() * k / 4).filter(|&c| s.is_char_boundary(c)).collect();
                        for c in cuts {
                            let mut b = last.clone();
                            b[key] = json!(&s[..c]);
                            *blocks.last_mut().unwrap() = b;
                            out.push(fixture::msg(id, "assistant", Value::Array(blocks.clone()), meta.clone()));
                        }
                        *blocks.last_mut().unwrap() = last.clone();
                    }
                }
            }
            out.push(fixture::msg(id, "assistant", Value::Array(blocks), meta.clone()));
        }
        out
    }

    fn renders(msgs: Vec<Message>) -> Vec<(Message, Live)> {
        msgs.into_iter().map(|m| (m, live(&[], &[]))).collect()
    }

    use super::fixture;

    #[test]
    fn live_turns_render_as_before_long_turn() {
        let prefix = fixture::turn_prefix(7);
        let reply = vec![
            json!({"type": "thinking", "thinking": "Plan: guard the slice, then test it.", "signature": "s"}),
            json!({"type": "text", "text": fixture::reply_text(8, 0)}),
            json!({"type": "tool_use", "id": "w1", "name": "Write", "input": {"file_path": "/a.rs", "content": "fn a() {}\n"}}),
            json!({"type": "tool_use", "id": "m1", "name": "mcp__github__get_issue", "input": {"owner": "o", "repo": "r", "number": 1}}),
        ];
        let meta = json!({"model": "claude-opus-5-5", "fingerprint": "fp"});
        let mut currents = renders(growing("am", &reply, meta.clone()));
        // tool calls going through their live states
        let done = fixture::msg("am", "assistant", Value::Array(reply.clone()), json!({"model": "claude-opus-5-5", "stop_reason": "tool_use", "usage": {"input_tokens": 5, "output_tokens": 7}}));
        currents.push((done.clone(), live(&[], &["m1"])));
        currents.push((done.clone(), live(&[("w1", "awaiting_permission")], &[])));
        currents.push((done.clone(), live(&[("w1", "running")], &[])));
        currents.push((done.clone(), live(&[("m1", "running")], &[])));
        currents.push((done, live(&[], &[])));
        same_as_before(&prefix, &currents, &ArtifactTracker::default());
    }

    #[test]
    fn live_turns_render_as_before_edge_cases() {
        let m = fixture::msg;
        let ameta = |extra: Value| {
            let mut v = json!({"model": "claude-opus-5-5", "usage": {"input_tokens": 3, "output_tokens": 4}, "duration_ms": 9});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        let artifact = "<antArtifact identifier=\"page\" type=\"text/html\" title=\"Page\"><p>v</p></antArtifact>";
        // the prefix ends with thinking (a stopped response), then a tool
        // result; text pending across messages; an unanswered tool; server
        // tools; artifacts (numbering goes on in the reply); an overload switch
        let prefix = vec![
            m("a1", "assistant", json!([
                {"type": "text", "text": format!("First {artifact} and https://example.com/x."), "citations": [{"url": "https://www.example.com/a", "title": "A"}, {"url": "javascript:x", "title": "bad"}]},
                {"type": "server_tool_use", "id": "s1", "name": "web_fetch", "input": {"url": "https://example.com"}},
                {"type": "web_fetch_tool_result", "tool_use_id": "s1", "content": {"type": "web_fetch_result", "url": "https://example.com", "content": {"title": "Ex"}}},
                {"type": "server_tool_use", "id": "s2", "name": "code_execution", "input": {"code": "print(1)"}},
                {"type": "code_execution_tool_result", "tool_use_id": "s2", "content": {"type": "code_execution_result", "stdout": "1\n", "stderr": "warn"}},
                {"type": "server_tool_use", "id": "s3", "name": "web_search", "input": {"query": "q"}},
                {"type": "web_search_tool_result", "tool_use_id": "s3", "content": {"type": "web_search_tool_result_error", "error_code": "too_many_requests"}},
                {"type": "server_tool_use", "id": "s4", "name": "web_search", "input": {"query": "never answered"}},
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
                {"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "/x"}},
                {"type": "fallback", "from": {"model": "a"}, "to": {"model": "b"}},
                {"type": "compaction"},
                {"type": "redacted_thinking", "data": "xx"},
                {"type": "thinking", "thinking": "still thinking about **it**"}
            ]), ameta(json!({"overload_fallback": {"from": "x", "to": "y"}, "stop_reason": "tool_use", "cost_usd": 0.5}))),
            m("u1", "user", json!([
                {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "a"}, {"type": "document", "title": "doc"}, {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "QUJD"}}]},
                {"type": "tool_result", "tool_use_id": "w9", "is_error": true, "content": "answered before it is asked"}
            ]), json!({"tool_results": true})),
        ];
        let reply = vec![
            json!({"type": "thinking", "thinking": "more thinking, merged with the last"}),
            json!({"type": "text", "text": format!("Answer with ```rust\nfn x() {{}}\n``` and {artifact} then <antArtifact identifier=\"page\" type=\"text/html\" title=\"Page\"><p>half")}),
            json!({"type": "tool_use", "id": "w9", "name": "Write", "input": {}}),
            json!({"type": "server_tool_use", "id": "s5", "name": "web_search", "input": {"query": "live"}}),
        ];
        let mut currents = renders(growing("am", &reply, json!({"model": "claude-opus-5-5"})));
        // the prefix's unanswered tool changes state (the kept rendering
        // must not be used then), renders not given back
        let full = fixture::msg("am", "assistant", Value::Array(reply.clone()), json!({"model": "claude-opus-5-5"}));
        currents.push((full.clone(), live(&[("t2", "running")], &["w9"])));
        currents.push((full.clone(), Live { keep: true, ..live(&[("t2", "running")], &[]) }));
        currents.push((full.clone(), live(&[("t2", "running")], &["t2"])));
        currents.push((full.clone(), live(&[], &[])));
        // the endings of a reply
        for extra in [
            json!({"stop_reason": "refusal", "stop_details": {"category": "x"}}),
            json!({"stop_reason": "max_tokens"}),
            json!({"error": {"message": "boom", "kind": "error"}}),
            json!({"error": null, "budget_stop": {"budget": 1.0, "spent": 1.5}}),
            json!({"stopped": true, "feedback": "up", "cost_usd": 0.25}),
            json!({"overload_fallback": {"from": "p", "to": "q"}, "subagent_usage": {"input_tokens": 100}}),
        ] {
            let mut done = full.clone();
            done.meta = ameta(extra);
            currents.push((done, live(&[], &[])));
        }
        let mut tracker = ArtifactTracker::default();
        tracker.bump("page");
        same_as_before(&prefix, &currents, &tracker);
        // no earlier messages (the OpenAI-compatible endpoint), a local model
        let oai = renders(growing("am", &[json!({"type": "text", "text": "Hello **there**, `x`"})], json!({"model": "oai:llama", "usage": {"input_tokens": 1}})));
        same_as_before(&[], &oai, &ArtifactTracker::default());
        // text pending at the end of the prefix (a Stop hook asked for more)
        let hook = vec![
            m("a1", "assistant", json!([{"type": "text", "text": "Done, I think. "}]), ameta(json!({"stop_reason": "end_turn"}))),
            m("u1", "user", json!([{"type": "text", "text": "Stop hook feedback: run the tests"}]), json!({"tool_results": true, "hook_feedback": true})),
        ];
        let more = renders(growing("am", &[json!({"type": "text", "text": "Running them now."}), json!({"type": "thinking", "thinking": "hmm"})], json!({})));
        same_as_before(&hook, &more, &ArtifactTracker::default());
        // a prefix that ends with thinking only whitespace text in between
        let ws = vec![m("a1", "assistant", json!([{"type": "thinking", "thinking": "a"}, {"type": "text", "text": "  "}]), ameta(json!({})))];
        same_as_before(&ws, &renders(growing("am", &[json!({"type": "thinking", "thinking": "b"})], json!({}))), &ArtifactTracker::default());
    }

    #[test]
    fn kept_prefix_is_rebuilt_when_not_given_back() {
        let prefix = fixture::turn_prefix(3);
        let lt = HashMap::new();
        let st = HashSet::new();
        let mn = HashMap::new();
        let ctx = RenderCtx { math: true, live_tools: &lt, streaming_tools: &st, mcp_names: &mn, streaming_msg: Some("am") };
        let mut p = TurnPrefix::new(&prefix, &ctx, &ArtifactTracker::default());
        let cur = fixture::msg("am", "assistant", json!([{"type": "text", "text": "hi"}]), json!({}));
        let t = p.render(&prefix, &cur, &ctx);
        assert!(!p.fits(&prefix, &ctx), "its parts are out");
        let mut changed = t.clone();
        changed["parts"] = json!("not a list");
        assert!(!p.give_back(changed));
        let mut p = TurnPrefix::new(&prefix, &ctx, &ArtifactTracker::default());
        let t2 = p.render(&prefix, &cur, &ctx);
        assert_eq!(t, t2);
        assert!(p.give_back(t2));
        assert!(p.fits(&prefix, &ctx));
        // a "reply" that is one of the prefix's messages isn't rendered from it
        let ctx2 = RenderCtx { streaming_msg: Some("a1"), ..ctx };
        assert!(!p.fits(&prefix, &ctx2));
    }
}

/// A long agentic turn and a streamed reply, to measure (and check) the
/// live rendering that runs every 45 ms while a reply streams.
#[cfg(test)]
pub(crate) mod fixture {
    use crate::db::Message;
    use serde_json::{json, Value};

    pub fn msg(id: &str, role: &str, content: Value, meta: Value) -> Message {
        Message {
            id: id.into(),
            conversation_id: "c".into(),
            parent_id: Some(format!("p-{id}")),
            role: role.into(),
            content,
            meta,
            created_at: 1_700_000_000_000,
        }
    }

    pub fn code_lines(n: usize, tag: usize) -> String {
        (0..n)
            .map(|i| format!("    let value_{tag}_{i} = compute(&items[{i}], \"<{tag}>\") * {i}; // note {i}\n"))
            .collect()
    }

    fn prose(i: usize) -> String {
        format!(
            "Now I'll check **`src/module_{i}.rs`**: it holds the parser for step {i}, and the \
             failing test calls it with an empty slice. See https://example.com/docs/{i}. \
             The fix keeps the old behaviour for callers that pass `None`.\n\n\
             - first, read the file\n- then run the tests\n- finally, edit the guard\n\n"
        )
    }

    /// Messages of the turn so far: `rounds` tool calls (text, a tool call,
    /// its result of a few KB), with thinking, a web search, an artifact, MCP
    /// tools, errors and an image result along the way.
    pub fn turn_prefix(rounds: usize) -> Vec<Message> {
        let mut out = vec![];
        for i in 0..rounds {
            let mut content = vec![];
            if i == 0 {
                content.push(json!({"type": "thinking", "thinking": format!("Let me think about the plan. {}", "Consider the parser. ".repeat(40)), "signature": "sig"}));
            }
            let mut text = prose(i);
            if i % 3 == 0 {
                text.push_str(&format!("```rust\nfn step_{i}() {{\n{}}}\n```\n", code_lines(12, i)));
            }
            if i == 8 {
                text.push_str("<antArtifact identifier=\"report\" type=\"text/html\" title=\"Report\"><html><body><h1>Report</h1><p>All good.</p></body></html></antArtifact>\n");
            }
            let mut tc = json!({"type": "text", "text": text});
            if i == 5 {
                tc["citations"] = json!([{"type": "web_search_result_location", "url": "https://www.rust-lang.org/learn", "title": "Learn Rust"}]);
                content.push(json!({"type": "server_tool_use", "id": format!("srv{i}"), "name": "web_search", "input": {"query": "rust parser empty slice"}}));
                content.push(json!({"type": "web_search_tool_result", "tool_use_id": format!("srv{i}"), "content": [
                    {"type": "web_search_result", "url": "https://www.rust-lang.org/learn", "title": "Learn Rust", "page_age": "2 days"},
                    {"type": "web_search_result", "url": "https://doc.rust-lang.org/std/", "title": "std", "page_age": null}
                ]}));
            }
            content.push(tc);
            let id = format!("t{i}");
            let tool = match i % 5 {
                0 => json!({"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": format!("/repo/src/module_{i}.rs")}}),
                1 => json!({"type": "tool_use", "id": id, "name": "Bash", "input": {"command": format!("cargo test -p core module_{i} -- --nocapture"), "description": "Run the tests"}}),
                2 => json!({"type": "tool_use", "id": id, "name": "Edit", "input": {"file_path": format!("/repo/src/module_{i}.rs"), "old_string": code_lines(3, i), "new_string": code_lines(4, i + 100)}}),
                3 => json!({"type": "tool_use", "id": id, "name": "mcp__github__get_issue", "input": {"owner": "acme", "repo": "core", "number": i}}),
                _ => json!({"type": "tool_use", "id": id, "name": "Grep", "input": {"pattern": format!("fn step_{i}"), "path": "/repo"}}),
            };
            content.push(tool);
            out.push(msg(
                &format!("a{i}"),
                "assistant",
                Value::Array(content),
                json!({"model": "claude-opus-5-5", "fingerprint": "fp", "stop_reason": "tool_use", "duration_ms": 1500 + i,
                       "usage": {"input_tokens": 20_000 + i, "output_tokens": 300, "cache_read_input_tokens": 18_000, "cache_creation_input_tokens": 500}}),
            ));
            let body = code_lines(40, i);
            let result = if i % 10 == 9 {
                json!({"type": "tool_result", "tool_use_id": format!("t{i}"), "content": [
                    {"type": "text", "text": body},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo".repeat(150)}}
                ]})
            } else if i % 7 == 6 {
                json!({"type": "tool_result", "tool_use_id": format!("t{i}"), "is_error": true, "content": format!("error: test failed\n{body}")})
            } else {
                json!({"type": "tool_result", "tool_use_id": format!("t{i}"), "content": body})
            };
            out.push(msg(&format!("u{i}"), "user", json!([result]), json!({"tool_results": true})));
        }
        out
    }

    /// The text of the reply being streamed: prose and three code blocks of
    /// `lines` lines each (about; the first is longest). Another `variant`
    /// has the same shape and other text.
    pub fn reply_text(lines: usize, variant: usize) -> String {
        let v = variant * 1000;
        let mut s = String::from("I found the cause. Here is the plan:\n\n1. Guard the empty slice.\n2. Add a test.\n\n");
        s.push_str(&format!("```rust\npub fn parse(items: &[Item]) -> Result<Ast> {{\n    /* the guard\n       comes first */\n{}}}\n```\n\n", code_lines(lines * 3 / 2, v + 1)));
        s.push_str(&prose(99));
        s.push_str(&format!("```ts\nexport function render(items: Item[]): string {{\n{}  return `a ${{x}}\n  b`;\n}}\n```\n\n", code_lines(lines, v + 2)));
        s.push_str("And the script that checks it:\n\n");
        s.push_str(&format!("```python\ndef check(items):\n    \"\"\"Doc\n    string.\"\"\"\n{}\n```\n\nThat's all.", code_lines(lines * 3 / 4, v + 3)));
        s
    }

    pub fn chunks(s: &str, n: usize) -> Vec<String> {
        let mut out = vec![];
        let mut cur = String::new();
        for c in s.chars() {
            cur.push(c);
            if cur.len() >= n {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// SSE events of the streamed reply: thinking, the text (code blocks of
    /// about `lines` lines), then a Write call whose input streams as
    /// partial JSON.
    pub fn reply_events(lines: usize, variant: usize) -> Vec<Value> {
        let mut ev = vec![json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-opus-5-5", "content": [], "usage": {"input_tokens": 40_000}}})];
        ev.push(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}));
        for c in chunks(&"The empty slice reaches the parser unguarded. ".repeat(20), 40) {
            ev.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": c}}));
        }
        ev.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}));
        ev.push(json!({"type": "content_block_stop", "index": 0}));
        ev.push(json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}));
        for c in chunks(&reply_text(lines, variant), 40) {
            ev.push(json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": c}}));
        }
        ev.push(json!({"type": "content_block_stop", "index": 1}));
        ev.push(json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "t_write", "name": "Write", "input": {}}}));
        let input = json!({"file_path": "/repo/src/parse.rs", "content": code_lines(lines * 2, variant * 1000 + 7)}).to_string();
        for c in chunks(&input, 60) {
            ev.push(json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": c}}));
        }
        ev.push(json!({"type": "content_block_stop", "index": 2}));
        ev.push(json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 2500}}));
        ev.push(json!({"type": "message_stop"}));
        ev
    }
}

/// The turn renderer as it was before `TurnBuilder` and `TurnPrefix`
/// (kept verbatim), to check that they give byte-for-byte the same turns.
#[cfg(test)]
pub(crate) mod reference {
    use super::{domain_of, ArtifactTracker, RenderCtx, CITE_RE};
    use crate::artifacts::{self, Segment};
    use crate::db::Message;
    use crate::markdown::{self, Opts};
    use crate::util::{escape_html, truncate_middle};
    use serde_json::{json, Value};
    use std::collections::{HashMap, HashSet};

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

    /// With markdown rendered without the caches and code highlighted from
    /// its start, so that nothing comes from what the new code computed.
    pub fn assistant_turn(msgs: &[&Message], ctx: &RenderCtx, tracker: &mut ArtifactTracker) -> Value {
        crate::markdown::FROM_SCRATCH.with(|f| f.set(true));
        let t = assistant_turn_as_it_was(msgs, ctx, tracker);
        crate::markdown::FROM_SCRATCH.with(|f| f.set(false));
        t
    }

    fn assistant_turn_as_it_was(msgs: &[&Message], ctx: &RenderCtx, tracker: &mut ArtifactTracker) -> Value {
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
        // the Claude Code CLI reports a whole run's cost on its last row; it
        // replaces the estimates of that run's other rows
        let mut reported_cost = 0.0f64;
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
                reported_cost += c;
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
                        let (server, tool) = ctx.tool_names(&name);
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
                "cost_usd": if reported_cost > 0.0 { reported_cost } else { cost_usd },
                "duration_ms": duration_ms,
            },
        })
    }
}
