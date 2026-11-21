//! The generation engine: builds requests from the stored conversation tree,
//! streams responses to the UI, runs tools (MCP and Code-mode tools) with
//! permission prompts, and loops until the model finishes its turn.

use crate::anthropic::{Accumulator, Client, StreamOutcome};
use crate::attachments;
use crate::codetools::{self, CodeSession, Perm};
use crate::db::{Conversation, Message};
use crate::error::{AppError, AppResult};
use crate::models::{self, ModelInfo, Thinking};
use crate::prompts;
use crate::render::{self, ArtifactTracker, RenderCtx};
use crate::settings::Settings;
use crate::state::{AppState, LiveRun, PermissionReply};
use crate::util::{new_id, now_ms, sha256_hex};
use crate::view;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

pub struct ReqCtx {
    pub model_id: String,
    pub model: ModelInfo,
    pub system: String,
    pub tools: Vec<Value>,
    pub fingerprint: String,
    pub prefix_blocks: Vec<Value>,
    pub code: Option<Arc<CodeSession>>,
    pub mcp_names: HashMap<String, (String, String)>,
    /// MCP tools (API names) that declare `readOnlyHint`.
    pub mcp_read_only: HashSet<String>,
    pub first_party: bool,
}

// ------------------------------------------------------------------ events

pub fn emit_turn(app: &AppHandle, conv_id: &str, turn: &Value) {
    // keyframes and deltas (see turnstream.rs; ui/js/turnstream.js expands them)
    crate::turnstream::emit_turn(app, conv_id, turn);
}

fn emit_state(app: &AppHandle, conv_id: &str, running: bool, error: Option<&AppError>) {
    let _ = app.emit(
        "generation-state",
        json!({"conversation_id": conv_id, "running": running, "error": error}),
    );
}

pub fn emit_conv_updated(app: &AppHandle, conv_id: &str) {
    let _ = app.emit("conversation-updated", json!({"conversation_id": conv_id}));
}

// ------------------------------------------------------------- request ctx

fn opt_bool(o: &Value, k: &str, default: bool) -> bool {
    o[k].as_bool().unwrap_or(default)
}

fn opt_strs(o: &Value, k: &str) -> Vec<String> {
    o[k].as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}

pub async fn build_request_context(st: &AppState, conv: &Conversation, settings: &Settings) -> AppResult<ReqCtx> {
    let model_id = conv.model.clone();
    let model = models::info(&model_id);
    let first_party = settings.is_first_party() && !st.degraded.load(Ordering::Relaxed);
    let eager = first_party;
    let opts = &conv.options;
    let disabled_servers = opt_strs(opts, "disabled_servers");
    let mcp_tools: Vec<_> = st
        .mcp
        .tools()
        .await
        .into_iter()
        .filter(|t| !disabled_servers.contains(&t.server))
        .filter(|t| {
            settings
                .tool_policies
                .get(&format!("{}/{}", t.server, t.name))
                .and_then(|v| v.as_str())
                != Some("deny")
        })
        .collect();
    let mcp_names: HashMap<String, (String, String)> = mcp_tools
        .iter()
        .map(|t| (t.api_name.clone(), (t.server.clone(), t.name.clone())))
        .collect();
    let mcp_read_only: HashSet<String> = mcp_tools.iter().filter(|t| t.read_only).map(|t| t.api_name.clone()).collect();
    let mut tools: Vec<Value> = vec![];
    let mut prefix_blocks = vec![];
    let mut code = None;
    let system = if conv.kind == "code" {
        let cwd = conv.cwd.clone().unwrap_or_default();
        code = Some(st.code_session(&conv.id, &cwd));
        tools.extend(codetools::definitions(eager));
        tools.push(crate::subagent::definition());
        tools.extend(models::web_tools(&model));
        prompts::code_system(&prompts::CodePromptInput {
            cwd: &cwd,
            env_snapshot: opts["env_snapshot"].as_str().unwrap_or(""),
            memory: opts["memory"].as_str().unwrap_or(""),
            settings,
        })
    } else {
        let project = match &conv.project_id {
            Some(p) => st.db.project_get(p).ok(),
            None => None,
        };
        let files = match &project {
            Some(p) => st.db.project_files(&p.id)?,
            None => vec![],
        };
        for f in files.iter().filter(|f| f.kind != "text") {
            let a = attachments::Attachment {
                id: f.id.clone(),
                name: f.name.clone(),
                mime: f.mime.clone(),
                size: f.size as u64,
                kind: f.kind.clone(),
                data: f.data.clone(),
                preview: None,
                digest: None,
            };
            prefix_blocks.push(attachments::to_block(&a));
        }
        let web = opt_bool(opts, "web_search", settings.web_search_default);
        if web {
            tools.extend(models::web_tools(&model));
        }
        if settings.chat_search {
            tools.extend(memory_tools(eager));
        }
        let date = opts["date"].as_str().map(|s| s.to_string()).unwrap_or_else(today);
        prompts::chat_system(&prompts::ChatPromptInput {
            settings,
            date: &date,
            style: opts["style"].as_str().unwrap_or("normal"),
            project: project.as_ref(),
            project_files: &files,
            web_search: web,
        })
    };
    let system = match conversation_instructions(opts) {
        Some(block) => format!("{system}\n\n{block}"),
        None => system,
    };
    for t in &mcp_tools {
        let mut desc = match &t.title {
            Some(title) if !t.description.contains(title.as_str()) => format!("{title}. {}", t.description),
            _ => t.description.clone(),
        };
        if desc.trim().is_empty() {
            desc = format!("Tool {} from the {} MCP server", t.name, t.server);
        }
        let mut def = json!({
            "name": t.api_name,
            "description": crate::util::truncate_chars(&desc, 4000),
            "input_schema": t.input_schema,
        });
        if eager {
            def["eager_input_streaming"] = json!(true);
        }
        tools.push(def);
    }
    let mut fp_src = system.clone();
    fp_src.push('\u{0}');
    fp_src.push_str(&serde_json::to_string(&tools)?);
    fp_src.push('\u{0}');
    fp_src.push_str(&serde_json::to_string(&prefix_blocks)?);
    let fingerprint = sha256_hex(fp_src.as_bytes())[..24].to_string();
    Ok(ReqCtx {
        model_id,
        model,
        system,
        tools,
        fingerprint,
        prefix_blocks,
        code,
        mcp_names,
        mcp_read_only,
        first_party,
    })
}

pub const MEMORY_TOOLS: [&str; 2] = ["conversation_search", "recent_chats"];

fn memory_tools(eager: bool) -> Vec<Value> {
    let mut v = vec![
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
    ];
    if eager {
        for t in v.iter_mut() {
            t["eager_input_streaming"] = json!(true);
        }
    }
    v
}

fn run_memory_tool(st: &AppState, conv_id: &str, name: &str, input: &Value) -> (Vec<Value>, bool) {
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
                    let first_text = serde_json::from_str::<Value>(&first)
                        .map(|v| render::plain_text(&v))
                        .unwrap_or(first);
                    out.push_str(&format!(
                        "<chat title=\"{}\" updated=\"{}\">\n{}\n</chat>\n",
                        if title.is_empty() { "Untitled" } else { &title },
                        fmt_date(updated),
                        crate::util::truncate_chars(first_text.trim(), 600)
                    ));
                }
                (text_block(out), false)
            }
            Ok(_) => (text_block("No previous chats found."), false),
            Err(e) => (text_block(e.to_string()), true),
        };
    }
    let q = input["query"].as_str().unwrap_or("").trim();
    if q.is_empty() {
        return (text_block("query is required"), true);
    }
    let n = input["max_results"].as_u64().unwrap_or(5).clamp(1, 10) as usize;
    match st.db.search_messages(q, conv_id, n) {
        Ok(rows) if !rows.is_empty() => {
            let mut out = String::new();
            for (_, title, at, role, text) in rows {
                out.push_str(&format!(
                    "<result chat=\"{}\" date=\"{}\" from=\"{}\">\n{}\n</result>\n",
                    if title.is_empty() { "Untitled" } else { &title },
                    fmt_date(at),
                    if role == "user" { "user" } else { "assistant" },
                    crate::util::truncate_chars(text.trim(), 1500)
                ));
            }
            (text_block(out), false)
        }
        Ok(_) => (text_block(format!("No past conversations matched \"{q}\".")), false),
        Err(e) => (text_block(e.to_string()), true),
    }
}

pub fn today() -> String {
    chrono::Local::now().format("%A, %B %-d, %Y").to_string()
}

// --------------------------------------------------------------- messages

fn is_thinking(b: &Value) -> bool {
    matches!(b["type"].as_str(), Some("thinking") | Some("redacted_thinking"))
}

/// Make a stored assistant message valid to send back:
/// - after a mid-output refusal fallback, drop model-internal blocks before
///   the final `fallback` block (the block itself stays in place);
/// - drop unsigned (interrupted) thinking, empty text, tool calls without a
///   result in the next message, and unpaired server tool calls.
pub fn sanitize_assistant(content: &Value, answered: &HashSet<String>) -> Vec<Value> {
    // only the blocks kept are copied
    let blocks: &[Value] = content.as_array().map(Vec::as_slice).unwrap_or_default();
    let last_fallback = blocks.iter().rposition(|b| b["type"] == "fallback");
    let server_results: HashSet<&str> = blocks
        .iter()
        .filter(|b| b["type"].as_str().map(|t| t.ends_with("_tool_result")).unwrap_or(false))
        .filter_map(|b| b["tool_use_id"].as_str())
        .collect();
    let known = |t: &str| {
        matches!(t, "text" | "thinking" | "redacted_thinking" | "tool_use" | "server_tool_use" | "fallback" | "compaction")
            || t.ends_with("_tool_result")
    };
    let mut out = vec![];
    for (i, b) in blocks.iter().enumerate() {
        let t = b["type"].as_str().unwrap_or("");
        let before_fallback = last_fallback.map(|f| i < f).unwrap_or(false);
        if before_fallback {
            match t {
                "thinking" | "redacted_thinking" | "tool_use" => continue,
                "server_tool_use" if !server_results.contains(b["id"].as_str().unwrap_or("")) => continue,
                x if !known(x) => continue,
                _ => {}
            }
        }
        match t {
            "thinking" => {
                if b["signature"].as_str().map(|s| s.is_empty()).unwrap_or(true) {
                    continue;
                }
            }
            "redacted_thinking" => {
                if b["data"].as_str().map(|s| s.is_empty()).unwrap_or(true) {
                    continue;
                }
            }
            "text" => {
                if b["text"].as_str().map(|s| s.is_empty()).unwrap_or(true) {
                    continue;
                }
            }
            "tool_use" => {
                if !answered.contains(b["id"].as_str().unwrap_or("")) {
                    continue;
                }
            }
            "server_tool_use"
                if !server_results.contains(b["id"].as_str().unwrap_or("")) => {
                    continue;
                }
            _ => {}
        }
        out.push(b.clone());
    }
    out
}

fn tool_result_ids(m: &Message) -> HashSet<String> {
    m.content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "tool_result")
        .filter_map(|b| b["tool_use_id"].as_str().map(|s| s.to_string()))
        .collect()
}

/// Convert the stored branch into the API `messages` array.
pub fn build_messages(path: &[Message], rc: &ReqCtx) -> Vec<Value> {
    let compacted = crate::compact::sent(path);
    let path: &[Message] = &compacted;
    // Thinking blocks produced under a different system prompt / tool set must
    // go: remove the leading run of blocks up to the last mismatching one so
    // the remaining blocks stay valid for preserved thinking.
    let mut strip_upto: Option<usize> = None;
    for (i, m) in path.iter().enumerate() {
        if m.role == "assistant"
            && m.content.as_array().map(|a| a.iter().any(is_thinking)).unwrap_or(false)
            && m.meta["fingerprint"].as_str() != Some(rc.fingerprint.as_str())
        {
            strip_upto = Some(i);
        }
    }
    let mut out: Vec<Value> = vec![];
    let mut kept_tool_ids: HashSet<String> = HashSet::new();
    let mut first_user_done = false;
    for (i, m) in path.iter().enumerate() {
        if m.role == "assistant" {
            let answered = path
                .get(i + 1)
                .filter(|n| render::is_tool_result_msg(n))
                .map(tool_result_ids)
                .unwrap_or_default();
            let mut blocks = sanitize_assistant(&m.content, &answered);
            if strip_upto.map(|s| i <= s).unwrap_or(false) {
                blocks.retain(|b| !is_thinking(b));
            }
            kept_tool_ids = blocks
                .iter()
                .filter(|b| b["type"] == "tool_use")
                .filter_map(|b| b["id"].as_str().map(|s| s.to_string()))
                .collect();
            if blocks.is_empty() {
                // a failed response, or one paused at the spending limit
                // before its request was sent
                if !m.meta["error"].is_null() || m.meta["budget_stop"].is_object() {
                    continue;
                }
                blocks.push(json!({"type": "text", "text": "(no response)"}));
            }
            out.push(api_message("assistant", blocks));
        } else {
            let mut content: Vec<Value> = match &m.content {
                Value::Array(a) => a.clone(),
                Value::String(s) => vec![json!({"type": "text", "text": s})],
                _ => vec![],
            };
            if render::is_tool_result_msg(m) {
                content.retain(|b| {
                    b["type"] != "tool_result"
                        || kept_tool_ids.contains(b["tool_use_id"].as_str().unwrap_or(""))
                });
            } else if !first_user_done {
                first_user_done = true;
                if !rc.prefix_blocks.is_empty() {
                    let mut c = rc.prefix_blocks.clone();
                    c.extend(content);
                    content = c;
                }
            }
            content.retain(|b| !(b["type"] == "text" && b["text"].as_str().map(|s| s.is_empty()).unwrap_or(true)));
            if content.is_empty() {
                continue;
            }
            out.push(api_message("user", content));
        }
    }
    // Merge consecutive messages with the same role.
    let mut merged: Vec<Value> = Vec::with_capacity(out.len());
    for mut m in out {
        if let Some(last) = merged.last_mut() {
            if last["role"] == m["role"] {
                if let (Some(a), Some(b)) = (last["content"].as_array_mut(), m["content"].as_array_mut()) {
                    a.append(b);
                    continue;
                }
            }
        }
        merged.push(m);
    }
    merged
}

/// `{"role": role, "content": blocks}`, the blocks moved in (`json!` would
/// copy them: they are the bulk of every request).
fn api_message(role: &str, blocks: Vec<Value>) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("role".into(), json!(role));
    m.insert("content".into(), Value::Array(blocks));
    Value::Object(m)
}

pub(crate) fn base_body(rc: &ReqCtx, settings: &Settings, conv: &Conversation, messages: Vec<Value>) -> (Value, Vec<String>) {
    let thinking_default = if conv.kind == "code" { true } else { settings.thinking_default };
    let thinking_on = opt_bool(&conv.options, "thinking", thinking_default);
    let effort = conv.options["effort"].as_str();
    let max_tokens = if conv.kind == "code" { settings.max_tokens.max(64_000) } else { settings.max_tokens };
    let tp = models::thinking_params(&rc.model, thinking_on, effort, max_tokens);
    let mut body = json!({
        "model": rc.model_id,
        "max_tokens": tp.max_tokens,
        "system": [{"type": "text", "text": rc.system}],
        "messages": messages,
    });
    let mut betas = vec![];
    if !rc.tools.is_empty() {
        body["tools"] = json!(rc.tools);
    }
    if let Some(t) = tp.thinking {
        body["thinking"] = t;
    }
    if let Some(e) = tp.effort {
        body["output_config"] = json!({"effort": e});
    }
    if rc.first_party && settings.prompt_caching {
        body["cache_control"] = json!({"type": "ephemeral"});
    }
    if rc.first_party && settings.refusal_fallbacks && rc.model.fallbacks {
        body["fallbacks"] = json!("default");
        betas.push(FALLBACK_BETA.to_string());
    }
    (body, betas)
}

/// Remove optional request features (for endpoints that reject them).
fn strip_optional(body: &mut Value, betas: &mut Vec<String>) {
    if let Some(o) = body.as_object_mut() {
        o.remove("cache_control");
        o.remove("fallbacks");
        o.remove("context_management");
    }
    if let Some(t) = body["thinking"].as_object_mut() {
        t.remove("display");
    }
    if let Some(tools) = body["tools"].as_array_mut() {
        for t in tools {
            if let Some(o) = t.as_object_mut() {
                o.remove("eager_input_streaming");
            }
        }
    }
    betas.clear();
}

fn strip_all_thinking(body: &mut Value) {
    if let Some(msgs) = body["messages"].as_array_mut() {
        for m in msgs.iter_mut() {
            if m["role"] == "assistant" {
                if let Some(c) = m["content"].as_array_mut() {
                    c.retain(|b| !is_thinking(b));
                    if c.is_empty() {
                        c.push(json!({"type": "text", "text": "(no response)"}));
                    }
                }
            }
        }
    }
}

pub(crate) async fn stream_with_recovery<F>(
    st: &AppState,
    client: &Client,
    body: &mut Value,
    betas: &mut Vec<String>,
    cancel: &CancellationToken,
    on_event: &mut F,
) -> AppResult<StreamOutcome>
where
    F: FnMut(&Accumulator, Option<usize>),
{
    let mut tried_optional = false;
    let mut tried_thinking = false;
    loop {
        match client.stream(body, betas, cancel, &mut *on_event).await {
            Err(AppError::Api { status: 400, kind, message }) => {
                let lower = message.to_lowercase();
                if !tried_thinking
                    && lower.contains("thinking")
                    && (lower.contains("prefix") || lower.contains("binding") || lower.contains("signature") || lower.contains("modified"))
                {
                    tried_thinking = true;
                    strip_all_thinking(body);
                    continue;
                }
                let optional_terms = ["cache_control", "fallback", "eager_input_streaming", "display", "beta", "context_management", "extra inputs"];
                if !tried_optional && optional_terms.iter().any(|t| lower.contains(t)) {
                    tried_optional = true;
                    st.degraded.store(true, Ordering::Relaxed);
                    strip_optional(body, betas);
                    continue;
                }
                return Err(AppError::Api { status: 400, kind, message });
            }
            other => return other,
        }
    }
}

// ----------------------------------------------------------- live renderer

struct TurnRenderer {
    first_id: Option<String>,
    siblings: Value,
    prefix: Vec<Message>,
    tracker: ArtifactTracker,
    mcp_names: HashMap<String, (String, String)>,
    math: bool,
    /// `prefix` rendered once: the turn is rendered again on every update of
    /// the reply (every 45 ms while it streams) and only the reply changes.
    done: parking_lot::Mutex<Option<render::TurnPrefix>>,
}

impl TurnRenderer {
    fn new(first_id: Option<String>, siblings: Value, prefix: Vec<Message>, tracker: ArtifactTracker, mcp_names: HashMap<String, (String, String)>, math: bool) -> Self {
        TurnRenderer { first_id, siblings, prefix, tracker, mcp_names, math, done: Default::default() }
    }

    /// The turn with `current` (the reply) as its last message. Hand it to
    /// `give_back` once sent: it holds the cached rendering of the prefix.
    fn render(&self, current: &Message, live_tools: &HashMap<String, String>, streaming_tools: &HashSet<String>) -> Value {
        let ctx = RenderCtx {
            math: self.math,
            live_tools,
            streaming_tools,
            mcp_names: &self.mcp_names,
            streaming_msg: Some(&current.id),
        };
        let mut t = if current.role == "assistant" {
            let mut done = self.done.lock();
            let prefix = match done.take() {
                Some(p) if p.fits(&self.prefix, &ctx) => done.insert(p),
                _ => done.insert(render::TurnPrefix::new(&self.prefix, &ctx, &self.tracker)),
            };
            prefix.render(&self.prefix, current, &ctx)
        } else {
            let mut msgs: Vec<&Message> = self.prefix.iter().collect();
            msgs.push(current);
            let mut tracker = self.tracker.clone();
            render::assistant_turn(&msgs, &ctx, &mut tracker)
        };
        if let Some(f) = &self.first_id {
            t["id"] = json!(f);
        }
        t["siblings"] = self.siblings.clone();
        t
    }

    fn give_back(&self, turn: Value) {
        let mut done = self.done.lock();
        if let Some(p) = done.as_mut() {
            if !p.give_back(turn) {
                *done = None;
            }
        }
    }

    /// Render the turn with `current` as its last message and send it.
    fn emit(&self, app: &AppHandle, conv_id: &str, current: &Message, live: &LiveRun) {
        let t = self.render(current, &live.live_tools, &live.streaming_tools);
        emit_turn(app, conv_id, &t);
        self.give_back(t);
    }
}

/// One update of the reply streaming in `cur`: its content so far goes to
/// `st.live` (for a view opened meanwhile) and the turn to `emit`.
fn live_tick(st: &AppState, conv_id: &str, renderer: &TurnRenderer, cur: &mut Message, base: &[Value], acc: &Accumulator, emit: impl FnOnce(&Value)) {
    let (c, streaming) = live_content(base, acc);
    cur.content = c;
    let live_tools = st.live.lock().get(conv_id).map(|l| l.live_tools.clone()).unwrap_or_default();
    let t = renderer.render(cur, &live_tools, &streaming);
    {
        // moved, not copied: the next update makes the content anew
        let mut lives = st.live.lock();
        let l = lives.entry(conv_id.to_string()).or_default();
        if l.msg_id != cur.id {
            l.msg_id = cur.id.clone();
        }
        l.content = std::mem::take(&mut cur.content);
        l.streaming_tools = streaming;
    }
    emit(&t);
    renderer.give_back(t);
}

fn live_content(base: &[Value], acc: &Accumulator) -> (Value, HashSet<String>) {
    let mut blocks: Vec<Value> = base.to_vec();
    let mut streaming = HashSet::new();
    for (idx, b) in acc.blocks.iter().enumerate() {
        if b.is_null() {
            continue;
        }
        let mut b = b.clone();
        if let Some(raw) = acc.partial_input(idx) {
            if let Some(id) = b["id"].as_str() {
                streaming.insert(id.to_string());
            }
            b["input"] = render::close_partial_json(raw).unwrap_or_else(|| json!({}));
        }
        blocks.push(b);
    }
    (Value::Array(blocks), streaming)
}

// ------------------------------------------------------------ the engine

/// A conversation reserved for one operation (a turn being prepared, a
/// rewind, a compaction): nothing else can start a turn in it meanwhile.
/// The reservation ends when this is dropped, unless it was handed to
/// `start_reserved`. Stop cancels its token like a running turn's.
pub struct RunSlot {
    st: Arc<AppState>,
    conv_id: String,
    pub cancel: CancellationToken,
    held: bool,
}

impl RunSlot {
    /// Reserve `conv_id`, or fail with `busy` when a turn (or another
    /// reserved operation) is under way there.
    pub fn reserve(st: &Arc<AppState>, conv_id: &str, busy: &str) -> AppResult<RunSlot> {
        let cancel = CancellationToken::new();
        let mut runs = st.runs.lock();
        if runs.contains_key(conv_id) {
            return Err(AppError::msg(busy));
        }
        runs.insert(conv_id.to_string(), cancel.clone());
        Ok(RunSlot { st: st.clone(), conv_id: conv_id.to_string(), cancel, held: true })
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        if self.held {
            self.st.runs.lock().remove(&self.conv_id);
        }
    }
}

pub fn start(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, parent_id: &str) -> AppResult<()> {
    let slot = RunSlot::reserve(st, conv_id, "Claude is already responding in this conversation.")?;
    start_reserved(app, slot, parent_id);
    Ok(())
}

/// Start the turn for a conversation reserved with `RunSlot::reserve`.
pub fn start_reserved(app: &AppHandle, mut slot: RunSlot, parent_id: &str) {
    // the running turn owns the reservation from here (its task removes it)
    slot.held = false;
    let st = slot.st.clone();
    let conv_id = slot.conv_id.clone();
    let cancel = slot.cancel.clone();
    drop(slot);
    emit_state(app, &conv_id, true, None);
    let app = app.clone();
    let parent_id = parent_id.to_string();
    tauri::async_runtime::spawn(async move {
        use futures_util::FutureExt;
        let res = std::panic::AssertUnwindSafe(run(&app, &st, &conv_id, parent_id, &cancel))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(AppError::msg("Something went wrong while generating this response.")));
        st.runs.lock().remove(&conv_id);
        st.live.lock().remove(&conv_id);
        crate::turnstream::end(&conv_id);
        st.pending_perms.lock().retain(|_, v| v["conversation_id"] != conv_id.as_str());
        let err = res.as_ref().err().filter(|e| !matches!(e, AppError::Cancelled));
        if let Some(e) = err {
            log::error!("generation failed: {e}");
        }
        emit_state(&app, &conv_id, false, err);
        emit_conv_updated(&app, &conv_id);
        notify_done(&app, &st, &conv_id, err.is_some());
        if res.is_ok() {
            let needs_title = st.db.conv_get(&conv_id).map(|c| c.title.is_empty()).unwrap_or(false);
            if needs_title {
                generate_title(&app, &st, &conv_id).await;
            }
        }
    });
}

/// Desktop notification when a session needs the user (a permission
/// prompt) while the app isn't in front.
pub(crate) fn notify_attention(app: &AppHandle, st: &AppState, conv_id: &str, payload: &Value) {
    if !st.settings.read().notifications {
        return;
    }
    use tauri::Manager;
    let focused = app
        .get_webview_window("main")
        .map(|w| w.is_focused().unwrap_or(false) && w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    if focused {
        return;
    }
    let title = st
        .db
        .conv_get(conv_id)
        .map(|c| if c.title.is_empty() { "ClaudeRust".to_string() } else { c.title })
        .unwrap_or_else(|_| "ClaudeRust".into());
    let tool = payload["tool"].as_str().unwrap_or("a tool");
    let detail = payload["input"]["command"]
        .as_str()
        .or(payload["input"]["file_path"].as_str())
        .map(|d| format!(": {}", d.chars().take(80).collect::<String>()))
        .unwrap_or_default();
    let body = match payload["kind"].as_str() {
        Some("plan") => "Claude has a plan ready for your review.".to_string(),
        Some("budget") => format!("Paused: this turn reached its ${:.2} spending limit.", payload["budget"].as_f64().unwrap_or(0.0)),
        _ => format!("Claude wants to use {tool}{detail}"),
    };
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

fn notify_done(app: &AppHandle, st: &AppState, conv_id: &str, failed: bool) {
    if !st.settings.read().notifications {
        return;
    }
    use tauri::Manager;
    let focused = app
        .get_webview_window("main")
        .map(|w| w.is_focused().unwrap_or(false) && w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    if focused {
        return;
    }
    let title = st
        .db
        .conv_get(conv_id)
        .map(|c| if c.title.is_empty() { "ClaudeRust".to_string() } else { c.title })
        .unwrap_or_else(|_| "ClaudeRust".into());
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(if failed { "The response failed. Open ClaudeRust to retry." } else { "Claude finished responding." })
        .show();
}

pub fn stop(st: &AppState, conv_id: &str) {
    if let Some(c) = st.runs.lock().get(conv_id) {
        c.cancel();
    }
}

/// Hooks that apply to a code session (project hooks only when trusted).
pub(crate) fn hook_config(st: &AppState, cwd: &std::path::Path) -> crate::hooks::Config {
    let trusted: HashMap<String, String> = st
        .settings
        .read()
        .trusted_hooks
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    crate::hooks::load(cwd, &trusted)
}

/// Snapshot the project before a code turn starts (once per user message).
async fn checkpoint_turn(st: &Arc<AppState>, conv_id: &str, parent: &str, cancel: &CancellationToken) {
    let Ok(conv) = st.db.conv_get(conv_id) else { return };
    if conv.kind != "code" || !st.settings.read().code_checkpoints {
        return;
    }
    let Some(cwd) = conv.cwd.clone() else { return };
    let Ok(path) = st.db.path_to(parent) else { return };
    let Some(user) = path.iter().rev().find(|m| m.role == "user" && !render::is_tool_result_msg(m)).cloned() else { return };
    if user.meta["checkpoint"].is_string() {
        return;
    }
    let data = st.data_dir.clone();
    let label = format!("{conv_id} {}", user.id);
    let snap = tokio::task::spawn_blocking(move || {
        let sh = crate::checkpoints::Shadow::new(&data, std::path::Path::new(&cwd));
        sh.suitable().map_err(crate::error::AppError::msg)?;
        sh.snapshot(&label)
    });
    // a slow disk must not hold up the turn for long, and Stop ends the wait
    let r = tokio::select! {
        r = tokio::time::timeout(Duration::from_secs(90), snap) => match r {
            Ok(r) => r,
            Err(_) => {
                log::warn!("checkpoint took too long; continuing without it");
                return;
            }
        },
        _ = cancel.cancelled() => return,
    };
    match r {
        Ok(Ok(sha)) => {
            let mut meta = user.meta.clone();
            meta["checkpoint"] = json!(sha);
            let _ = st.db.msg_update(&user.id, &user.content, &meta);
        }
        Ok(Err(e)) => log::warn!("checkpoint failed: {e}"),
        Err(e) => log::warn!("checkpoint failed: {e}"),
    }
}

/// Before a turn of the built-in engine: if the context is nearly full,
/// summarize everything before the new user message.
async fn auto_compact(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, parent: &str, cancel: &CancellationToken) {
    let pct = st.settings.read().auto_compact_pct;
    if pct == 0 {
        return;
    }
    let Ok(conv) = st.db.conv_get(conv_id) else { return };
    if conv.options["engine"] == crate::claudecode::ENGINE || conv.model.starts_with(crate::oai::PREFIX) {
        return;
    }
    let Ok(path) = st.db.path_to(parent) else { return };
    let Some(ui) = path.iter().rposition(|m| m.role == "user" && !render::is_tool_result_msg(m)) else { return };
    if ui + 1 != path.len() || path[ui].meta["compact_summary"].is_string() {
        return; // only at the start of a turn
    }
    let before = &path[..ui];
    let start = crate::compact::start_index(before).unwrap_or(0);
    let Some(used) = crate::compact::context_tokens(&before[start..]) else { return };
    let window = models::info(&conv.model).context as u64;
    if used * 100 < window * pct as u64 {
        return;
    }
    let _ = app.emit("compacting", json!({"conversation_id": conv_id, "auto": true}));
    // Stop ends the summary too (nothing is written or charged further)
    let summary = tokio::select! {
        s = crate::compact::summarize(st, &conv.model, before, None) => s,
        _ = cancel.cancelled() => Err(AppError::Cancelled),
    };
    match summary {
        Ok(summary) => {
            let user = &path[ui];
            let mut meta = user.meta.clone();
            meta["compact_summary"] = json!(summary);
            meta["compact_auto"] = json!(true);
            let _ = st.db.msg_update(&user.id, &user.content, &meta);
            let _ = app.emit("compacted", json!({"conversation_id": conv_id, "auto": true}));
        }
        Err(AppError::Cancelled) => {
            let _ = app.emit("compacted", json!({"conversation_id": conv_id, "auto": true}));
        }
        Err(e) => {
            log::warn!("auto-compaction failed: {e}");
            let _ = app.emit("compacted", json!({"conversation_id": conv_id, "auto": true, "error": e.to_string()}));
        }
    }
}

/// The fallback model for an overloaded `conv.model` and the request rebuilt
/// for it (its own thinking / effort / tool settings).
async fn overload_switch(st: &AppState, conv: &Conversation, settings: &Settings, path: &[Message]) -> Option<(String, Value, Vec<String>)> {
    let fb = crate::models::overload_fallback(&conv.model, &settings.overload_fallback)?;
    let mut c2 = conv.clone();
    c2.model = fb.clone();
    let rc2 = build_request_context(st, &c2, settings).await.ok()?;
    log::warn!("{} is overloaded; retrying with {fb}", conv.model);
    let (body, betas) = base_body(&rc2, settings, &c2, build_messages(path, &rc2));
    Some((fb, body, betas))
}

/// The API said the model is overloaded (after the client's own retries).
fn is_overloaded(e: &AppError) -> bool {
    e.status() == 529 || e.kind() == "overloaded_error"
}

/// The user's instructions for one conversation (conversation menu), as a
/// system-prompt block.
pub(crate) fn conversation_instructions(opts: &Value) -> Option<String> {
    let ins = opts["instructions"].as_str().map(str::trim).filter(|s| !s.is_empty())?;
    Some(format!(
        "<conversation_instructions>\nThe user set these instructions for this conversation; follow them throughout:\n{}\n</conversation_instructions>",
        crate::util::truncate_chars(ins, 8000)
    ))
}

/// Spending limit for one agent turn in this conversation (USD; 0 = none):
/// the conversation's own (`/budget`) or the setting.
pub(crate) fn turn_budget(conv: &Conversation, settings: &Settings) -> f64 {
    let b = conv.options["budget_usd"].as_f64().unwrap_or(settings.turn_budget_usd);
    if b.is_finite() && b > 0.0 { b } else { 0.0 }
}

async fn run(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, mut parent: String, cancel: &CancellationToken) -> AppResult<()> {
    checkpoint_turn(st, conv_id, &parent, cancel).await;
    auto_compact(app, st, conv_id, &parent, cancel).await;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    {
        let conv = st.db.conv_get(conv_id)?;
        if conv.model.starts_with(crate::oai::PREFIX) {
            return run_oai(app, st, conv_id, parent, cancel).await;
        }
        if conv.kind == "code" && conv.options["engine"] == crate::claudecode::ENGINE {
            return crate::claudecode::run(app, st, conv_id, parent, cancel).await;
        }
    }
    let mut steps = 0usize;
    let mut stop_continues = 0usize;
    // what this turn has cost so far (responses and sub-agents)
    let mut turn_cost = 0.0f64;
    // set when the model was overloaded: the rest of the turn uses this one
    let mut turn_model: Option<String> = None;
    loop {
        steps += 1;
        let mut conv = st.db.conv_get(conv_id)?;
        if let Some(m) = &turn_model {
            conv.model = m.clone();
        }
        let max_steps = if conv.kind == "code" { 400 } else { 60 };
        let settings = st.settings.read().clone();
        let path = st.db.path_to(&parent)?;
        let rc = build_request_context(st, &conv, &settings).await?;
        let messages = build_messages(&path, &rc);
        let (mut body, mut betas) = base_body(&rc, &settings, &conv, messages);

        // The assistant message we are about to stream into.
        let am_id = new_id();
        let mut am = Message {
            id: am_id.clone(),
            conversation_id: conv_id.to_string(),
            parent_id: Some(parent.clone()),
            role: "assistant".into(),
            content: json!([]),
            meta: json!({"model": rc.model_id, "fingerprint": rc.fingerprint}),
            created_at: now_ms(),
        };
        st.db.msg_insert(&am)?;
        st.db.conv_set_leaf(conv_id, Some(&am_id))?;

        // Messages already in this assistant turn (previous tool rounds).
        let turn_start = path
            .iter()
            .rposition(|m| m.role == "user" && !render::is_tool_result_msg(m))
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix: Vec<Message> = path[turn_start..].to_vec();
        let first_id = prefix.iter().find(|m| m.role == "assistant").map(|m| m.id.clone());
        let nodes = st.db.tree_nodes(conv_id)?;
        let siblings = view::siblings_of(&nodes, first_id.as_deref().unwrap_or(&am_id));
        let renderer = TurnRenderer::new(first_id.clone(), siblings, prefix, view::tracker_for(&path[..turn_start]), rc.mcp_names.clone(), settings.latex_enabled);
        let mut live = LiveRun {
            msg_id: am_id.clone(),
            content: json!([]),
            ..Default::default()
        };
        st.live.lock().insert(conv_id.to_string(), live.clone());
        renderer.emit(app, conv_id, &am, &live);

        if steps > max_steps {
            am.meta["error"] = json!({"message": "Stopped after too many tool calls in one response.", "kind": "error"});
            st.db.msg_update(&am_id, &am.content, &am.meta)?;
            renderer.emit(app, conv_id, &am, &live);
            return Ok(());
        }
        let budget = turn_budget(&conv, &settings);
        if budget > 0.0 && turn_cost >= budget {
            am.meta["budget_stop"] = json!({"budget": budget, "spent": turn_cost});
            st.db.msg_update(&am_id, &am.content, &am.meta)?;
            renderer.emit(app, conv_id, &am, &live);
            notify_attention(app, st, conv_id, &json!({"kind": "budget", "budget": budget}));
            return Ok(());
        }

        let client = match st.client() {
            Ok(c) => c,
            Err(e) => {
                am.meta["error"] = serde_json::to_value(&e)?;
                st.db.msg_update(&am_id, &am.content, &am.meta)?;
                renderer.emit(app, conv_id, &am, &live);
                return Err(e);
            }
        };

        // ---- stream (with pause_turn continuations)
        let started = Instant::now();
        let mut content: Vec<Value> = vec![];
        let mut stop_reason: Option<String> = None;
        let mut stop_details = Value::Null;
        let mut usage = serde_json::Map::new();
        let mut model_used: Option<String> = None;
        let mut invalid_json: HashMap<String, String> = HashMap::new();
        let mut error: Option<AppError> = None;
        let mut continuations = 0;
        loop {
            let base = content.clone();
            let mut last_emit = Instant::now() - Duration::from_secs(1);
            let st2 = st.clone();
            let app2 = app.clone();
            // the reply as rendered: `am` with the content streamed so far
            let mut cur = am.clone();
            let mut on_event = |acc: &Accumulator, _changed: Option<usize>| {
                if last_emit.elapsed() < Duration::from_millis(45) {
                    return;
                }
                last_emit = Instant::now();
                live_tick(&st2, conv_id, &renderer, &mut cur, &base, acc, |t| emit_turn(&app2, conv_id, t));
            };
            let res = stream_with_recovery(st, &client, &mut body, &mut betas, cancel, &mut on_event).await;
            match res {
                Err(e) => {
                    // Overloaded before anything was produced: try another model.
                    if content.is_empty() && is_overloaded(&e) && am.meta["overload_fallback"].is_null() {
                        if let Some((fb, b2, bt2)) = overload_switch(st, &conv, &settings, &path).await {
                            body = b2;
                            betas = bt2;
                            am.meta["overload_fallback"] = json!({"from": conv.model, "to": fb});
                            am.meta["model"] = json!(fb);
                            turn_model = Some(fb);
                            continue;
                        }
                    }
                    error = Some(e);
                    break;
                }
                Ok(out) => {
                    let acc = out.acc;
                    // the same when the overload arrives as an error event in
                    // the stream, before any content
                    if let Some(e) = &out.error {
                        if content.is_empty() && acc.content().is_empty() && is_overloaded(e) && am.meta["overload_fallback"].is_null() {
                            if let Some((fb, b2, bt2)) = overload_switch(st, &conv, &settings, &path).await {
                                body = b2;
                                betas = bt2;
                                am.meta["overload_fallback"] = json!({"from": conv.model, "to": fb});
                                am.meta["model"] = json!(fb);
                                turn_model = Some(fb);
                                continue;
                            }
                        }
                    }
                    content.extend(acc.content());
                    if let Some(m) = acc.model() {
                        model_used = Some(m);
                    }
                    for (k, v) in acc.usage.iter() {
                        let prev = usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                        match v.as_u64() {
                            Some(n) => {
                                usage.insert(k.clone(), json!(prev + n));
                            }
                            None => {
                                usage.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    invalid_json.extend(acc.invalid_json.clone());
                    stop_reason = acc.stop_reason.clone();
                    stop_details = acc.stop_details.clone();
                    if let Some(e) = out.error {
                        error = Some(e);
                        break;
                    }
                    if stop_reason.as_deref() == Some("pause_turn") && continuations < 5 && !cancel.is_cancelled() {
                        continuations += 1;
                        let msgs = body["messages"].as_array_mut().unwrap();
                        let send = sanitize_assistant(&Value::Array(content.clone()), &HashSet::new());
                        // replace a previously appended paused turn
                        if continuations > 1 {
                            msgs.pop();
                        }
                        msgs.push(json!({"role": "assistant", "content": send}));
                        continue;
                    }
                    break;
                }
            }
        }

        // ---- persist
        am.content = Value::Array(content.clone());
        if let Some(m) = &model_used {
            am.meta["model"] = json!(m);
        }
        am.meta["usage"] = Value::Object(usage);
        turn_cost += crate::commands::usage_cost(am.meta["model"].as_str().unwrap_or(&conv.model), &am.meta["usage"]);
        am.meta["stop_reason"] = json!(stop_reason);
        if !stop_details.is_null() {
            am.meta["stop_details"] = stop_details;
        }
        am.meta["duration_ms"] = json!(started.elapsed().as_millis() as u64);
        let cancelled = matches!(error, Some(AppError::Cancelled)) || cancel.is_cancelled();
        if cancelled {
            am.meta["stopped"] = json!(true);
        } else if let Some(e) = &error {
            am.meta["error"] = serde_json::to_value(e)?;
        }
        st.db.msg_update(&am_id, &am.content, &am.meta)?;
        let text = render::plain_text(&am.content);
        let _ = st.db.fts_index(conv_id, &am_id, &text);
        save_artifacts(st, conv_id, &am_id, &text);
        live.content = am.content.clone();
        live.streaming_tools.clear();
        st.live.lock().insert(conv_id.to_string(), live.clone());
        renderer.emit(app, conv_id, &am, &live);

        if cancelled {
            return Ok(());
        }
        if let Some(e) = error {
            return Err(e);
        }
        if stop_reason.as_deref() != Some("tool_use") {
            // Stop hooks may ask Claude to keep going (at most 3 times a turn).
            if conv.kind == "code" && stop_reason.as_deref() == Some("end_turn") && stop_continues < 3 {
                if let Some(cwd) = conv.cwd.as_deref() {
                    let root = std::path::PathBuf::from(cwd);
                    let hooks = hook_config(st, &root);
                    if hooks.events.contains_key("Stop") {
                        let o = crate::hooks::run(&hooks, "Stop", "", json!({"session_id": conv_id, "stop_hook_active": stop_continues > 0}), &root).await;
                        if let Some(reason) = o.block {
                            stop_continues += 1;
                            let fb = Message {
                                id: new_id(),
                                conversation_id: conv_id.to_string(),
                                parent_id: Some(am_id.clone()),
                                role: "user".into(),
                                content: json!([{"type": "text", "text": format!("Stop hook feedback: {reason}")}]),
                                meta: json!({"tool_results": true, "hook_feedback": true}),
                                created_at: now_ms(),
                            };
                            st.db.msg_insert(&fb)?;
                            st.db.conv_set_leaf(conv_id, Some(&fb.id))?;
                            parent = fb.id;
                            continue;
                        }
                    }
                }
            }
            return Ok(());
        }

        // ---- run tools
        let send_form = sanitize_assistant(&am.content, &all_tool_ids(&am.content));
        let tool_uses: Vec<Value> = send_form.into_iter().filter(|b| b["type"] == "tool_use").collect();
        if tool_uses.is_empty() {
            return Ok(());
        }
        let mut results: Vec<Value> = vec![];
        // Sub-agents (Task) run first, all in parallel.
        let mut task_results: HashMap<String, (Vec<Value>, bool)> = HashMap::new();
        let tasks: Vec<&Value> = tool_uses
            .iter()
            .filter(|t| t["name"] == crate::subagent::TASK_TOOL && !invalid_json.contains_key(t["id"].as_str().unwrap_or("")))
            .collect();
        if !tasks.is_empty() && rc.code.is_some() && !cancel.is_cancelled() {
            for t in &tasks {
                live.live_tools.insert(t["id"].as_str().unwrap_or("").to_string(), "running".into());
            }
            st.live.lock().insert(conv_id.to_string(), live.clone());
            renderer.emit(app, conv_id, &am, &live);
            let futs = tasks.iter().map(|t| {
                let tid = t["id"].as_str().unwrap_or("").to_string();
                let rc = &rc;
                let am_id = am_id.clone();
                async move {
                    let r = crate::subagent::run_task(app, st, conv_id, &am_id, &tid, rc, &t["input"], cancel).await;
                    (tid, r)
                }
            });
            for (tid, r) in futures_util::future::join_all(futs).await {
                live.live_tools.remove(&tid);
                task_results.insert(tid, r);
            }
            let su = st.subagents.usage_for(&am_id);
            if !su.is_empty() {
                let su = Value::Object(su);
                turn_cost += crate::commands::usage_cost(am.meta["model"].as_str().unwrap_or(&conv.model), &su);
                am.meta["subagent_usage"] = su;
                st.db.msg_update(&am_id, &am.content, &am.meta)?;
            }
        }
        for tu in &tool_uses {
            let id = tu["id"].as_str().unwrap_or("").to_string();
            if let Some((blocks, is_error)) = task_results.remove(&id) {
                let mut r = json!({"type": "tool_result", "tool_use_id": id, "content": blocks});
                if is_error {
                    r["is_error"] = json!(true);
                }
                results.push(r);
                continue;
            }
            if cancel.is_cancelled() {
                results.push(json!({"type": "tool_result", "tool_use_id": id, "is_error": true, "content": "The user interrupted this tool call."}));
                continue;
            }
            if let Some(raw) = invalid_json.get(&id) {
                results.push(json!({"type": "tool_result", "tool_use_id": id, "is_error": true,
                    "content": json!({"INVALID_JSON": raw}).to_string()}));
                continue;
            }
            live.live_tools.insert(id.clone(), "running".into());
            st.live.lock().insert(conv_id.to_string(), live.clone());
            renderer.emit(app, conv_id, &am, &live);
            let (blocks, is_error) = execute_tool(app, st, conv_id, tu, &rc, &mut live, &renderer, &am, cancel).await;
            live.live_tools.remove(&id);
            let mut r = json!({"type": "tool_result", "tool_use_id": id, "content": blocks});
            if is_error {
                r["is_error"] = json!(true);
            }
            results.push(r);
        }
        let conv_now = st.db.conv_get(conv_id)?;
        let plan_mode = conv_now.kind == "code" && conv_now.options["permission_mode"] == "plan";
        if plan_mode {
            results.push(json!({"type": "text", "text": prompts::PLAN_MODE_REMINDER}));
        }
        let tr = Message {
            id: new_id(),
            conversation_id: conv_id.to_string(),
            parent_id: Some(am_id.clone()),
            role: "user".into(),
            content: Value::Array(results),
            meta: json!({"tool_results": true}),
            created_at: now_ms(),
        };
        st.db.msg_insert(&tr)?;
        st.db.conv_set_leaf(conv_id, Some(&tr.id))?;
        // render with results attached
        {
            live.live_tools.clear();
            let mut msgs: Vec<&Message> = renderer.prefix.iter().collect();
            msgs.push(&am);
            msgs.push(&tr);
            let ctx = RenderCtx {
                math: renderer.math,
                live_tools: &live.live_tools,
                streaming_tools: &live.streaming_tools,
                mcp_names: &renderer.mcp_names,
                streaming_msg: None,
            };
            let mut tracker = renderer.tracker.clone();
            let mut t = render::assistant_turn(&msgs, &ctx, &mut tracker);
            if let Some(f) = &renderer.first_id {
                t["id"] = json!(f);
            }
            t["siblings"] = renderer.siblings.clone();
            t["streaming"] = json!(true);
            emit_turn(app, conv_id, &t);
        }
        if cancel.is_cancelled() {
            return Ok(());
        }
        parent = tr.id;
    }
}

/// A chat turn with a model from the OpenAI-compatible endpoint (text only).
async fn run_oai(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, parent: String, cancel: &CancellationToken) -> AppResult<()> {
    let conv = st.db.conv_get(conv_id)?;
    let settings = st.settings.read().clone();
    let model = conv.model.trim_start_matches(crate::oai::PREFIX).to_string();
    let path = crate::compact::apply(&st.db.path_to(&parent)?);
    let rc = build_request_context(st, &conv, &settings).await?;
    let am_id = new_id();
    let mut am = Message {
        id: am_id.clone(),
        conversation_id: conv_id.to_string(),
        parent_id: Some(parent.clone()),
        role: "assistant".into(),
        content: json!([]),
        meta: json!({"model": conv.model, "provider": "oai"}),
        created_at: now_ms(),
    };
    st.db.msg_insert(&am)?;
    st.db.conv_set_leaf(conv_id, Some(&am_id))?;
    let nodes = st.db.tree_nodes(conv_id)?;
    let renderer = TurnRenderer::new(None, view::siblings_of(&nodes, &am_id), vec![], view::tracker_for(&path), HashMap::new(), settings.latex_enabled);
    let mut live = LiveRun { msg_id: am_id.clone(), content: json!([]), ..Default::default() };
    st.live.lock().insert(conv_id.to_string(), live.clone());
    renderer.emit(app, conv_id, &am, &live);
    if conv.kind == "code" {
        am.meta["error"] = json!({"message": "Models from an OpenAI-compatible endpoint can chat but can't run the coding agent. Pick a Claude model for code sessions.", "kind": "error"});
        st.db.msg_update(&am_id, &am.content, &am.meta)?;
        renderer.emit(app, conv_id, &am, &live);
        return Ok(());
    }
    let messages = crate::oai::to_messages(&rc.system, &path);
    let started = Instant::now();
    let mut last = Instant::now() - Duration::from_secs(1);
    let res = crate::oai::stream(&settings.oai_base_url, &settings.oai_api_key, &model, messages, settings.max_tokens, cancel, |text| {
        if last.elapsed() < Duration::from_millis(45) {
            return;
        }
        last = Instant::now();
        let mut cur = am.clone();
        cur.content = json!([{"type": "text", "text": text}]);
        let t = renderer.render(&cur, &live.live_tools, &live.streaming_tools);
        // kept current for a view opened mid-answer (it reads `live`)
        if let Some(x) = st.live.lock().get_mut(conv_id) {
            x.content = std::mem::take(&mut cur.content);
        }
        emit_turn(app, conv_id, &t);
        renderer.give_back(t);
    })
    .await;
    am.meta["duration_ms"] = json!(started.elapsed().as_millis() as u64);
    let err = match res {
        Ok(o) => {
            am.content = json!([{"type": "text", "text": o.text}]);
            am.meta["usage"] = o.usage;
            am.meta["stop_reason"] = json!(match o.finish.as_deref() { Some("length") => "max_tokens", _ => "end_turn" });
            if o.cancelled {
                am.meta["stopped"] = json!(true);
            }
            None
        }
        Err(e) => {
            am.meta["error"] = serde_json::to_value(&e)?;
            Some(e)
        }
    };
    st.db.msg_update(&am_id, &am.content, &am.meta)?;
    let text = render::plain_text(&am.content);
    let _ = st.db.fts_index(conv_id, &am_id, &text);
    save_artifacts(st, conv_id, &am_id, &text);
    live.content = am.content.clone();
    st.live.lock().insert(conv_id.to_string(), live.clone());
    renderer.emit(app, conv_id, &am, &live);
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

pub(crate) fn all_tool_ids(content: &Value) -> HashSet<String> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "tool_use")
        .filter_map(|b| b["id"].as_str().map(|s| s.to_string()))
        .collect()
}

pub(crate) fn save_artifacts(st: &AppState, conv_id: &str, msg_id: &str, text: &str) {
    use crate::artifacts::Segment;
    for seg in crate::artifacts::split(text) {
        if let Segment::Artifact(a) = seg {
            if !a.complete {
                continue;
            }
            let _ = st.db.artifact_upsert(&crate::db::ArtifactRow {
                id: new_id(),
                conversation_id: conv_id.to_string(),
                message_id: msg_id.to_string(),
                identifier: a.identifier,
                kind: a.kind,
                title: a.title,
                language: a.language,
                content: a.content,
                created_at: now_ms(),
                conversation_title: None,
            });
        }
    }
}

// ------------------------------------------------------------- tools

#[allow(clippy::too_many_arguments)]
async fn ask_permission(
    app: &AppHandle,
    st: &AppState,
    conv_id: &str,
    tool_use_id: &str,
    payload: Value,
    live: &mut LiveRun,
    renderer: &TurnRenderer,
    am: &Message,
    cancel: &CancellationToken,
) -> Option<PermissionReply> {
    let req_id = new_id();
    let (tx, rx) = oneshot::channel();
    st.permissions.lock().insert(req_id.clone(), tx);
    let mut ev = payload;
    ev["request_id"] = json!(req_id);
    ev["conversation_id"] = json!(conv_id);
    ev["tool_use_id"] = json!(tool_use_id);
    st.pending_perms.lock().insert(req_id.clone(), ev.clone());
    live.live_tools.insert(tool_use_id.to_string(), "awaiting_permission".into());
    st.live.lock().insert(conv_id.to_string(), live.clone());
    renderer.emit(app, conv_id, am, live);
    let _ = app.emit("permission-request", &ev);
    notify_attention(app, st, conv_id, &ev);
    let reply = tokio::select! {
        r = rx => r.ok(),
        _ = cancel.cancelled() => None,
    };
    st.permissions.lock().remove(&req_id);
    st.pending_perms.lock().remove(&req_id);
    let _ = app.emit("permission-resolved", json!({"request_id": req_id, "conversation_id": conv_id}));
    live.live_tools.insert(tool_use_id.to_string(), "running".into());
    st.live.lock().insert(conv_id.to_string(), live.clone());
    renderer.emit(app, conv_id, am, live);
    reply
}

fn text_block(s: impl Into<String>) -> Vec<Value> {
    vec![json!({"type": "text", "text": s.into()})]
}

fn denial(reply: &Option<PermissionReply>) -> Vec<Value> {
    match reply {
        None => text_block("The user interrupted this tool call."),
        Some(r) => {
            let mut s = String::from("The user denied this tool call. Do not retry it unchanged.");
            if let Some(m) = r.message.as_ref().filter(|m| !m.trim().is_empty()) {
                s.push_str(&format!(" The user said: {}", m.trim()));
            }
            text_block(s)
        }
    }
}

/// The conversation's "allow for this session" rules.
pub(crate) fn allowed_rules(conv: &Conversation) -> Vec<String> {
    opt_strs(&conv.options, "allowed_rules")
}

/// Change a conversation's options (read, modify, write as one step: the
/// engine and the UI both write them, and a change in between was lost).
pub(crate) fn update_conv_options(st: &AppState, conv_id: &str, f: impl FnOnce(&mut Value)) {
    if let Err(e) = try_update_conv_options(st, conv_id, f) {
        log::warn!("updating the options of {conv_id} failed: {e}");
    }
}

pub(crate) fn try_update_conv_options(st: &AppState, conv_id: &str, f: impl FnOnce(&mut Value)) -> AppResult<()> {
    static OPTIONS: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _one_at_a_time = OPTIONS.lock();
    let c = st.db.conv_get(conv_id)?;
    let mut o = c.options;
    if !o.is_object() {
        o = json!({});
    }
    f(&mut o);
    st.db.conv_set_options(conv_id, &o)
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool(
    app: &AppHandle,
    st: &Arc<AppState>,
    conv_id: &str,
    tu: &Value,
    rc: &ReqCtx,
    live: &mut LiveRun,
    renderer: &TurnRenderer,
    am: &Message,
    cancel: &CancellationToken,
) -> (Vec<Value>, bool) {
    let name = tu["name"].as_str().unwrap_or("");
    let input = &tu["input"];
    let id = tu["id"].as_str().unwrap_or("");
    let conv = match st.db.conv_get(conv_id) {
        Ok(c) => c,
        Err(e) => return (text_block(e.to_string()), true),
    };
    let settings = st.settings.read().clone();

    if let (true, Some(session)) = (codetools::is_code_tool(name), rc.code.as_ref()) {
        let mode = conv.options["permission_mode"]
            .as_str()
            .unwrap_or(&settings.code_permission_mode)
            .to_string();
        if name == "ExitPlanMode" {
            let reply = ask_permission(
                app, st, conv_id, id,
                json!({"kind": "plan", "tool": name, "input": input}),
                live, renderer, am, cancel,
            )
            .await;
            return match reply.as_ref().map(|r| r.decision.as_str()) {
                Some("approve") | Some("approve_edits") | Some("once") => {
                    let new_mode = if reply.as_ref().unwrap().decision == "approve_edits" { "acceptEdits" } else { "default" };
                    update_conv_options(st, conv_id, |o| o["permission_mode"] = json!(new_mode));
                    emit_conv_updated(app, conv_id);
                    (text_block("The user approved the plan. Plan mode is off: start implementing it now, keeping the todo list up to date."), false)
                }
                _ => {
                    let mut s = String::from("The user did not approve the plan yet; stay in plan mode.");
                    if let Some(m) = reply.and_then(|r| r.message).filter(|m| !m.trim().is_empty()) {
                        s.push_str(&format!(" Their feedback: {}", m.trim()));
                    }
                    (text_block(s), true)
                }
            };
        }
        let rules = opt_strs(&conv.options, "allowed_rules");
        let root = session.root.clone();
        let hooks = hook_config(st, &root);
        let hook_input = json!({"session_id": conv_id, "tool_name": name, "tool_input": input});
        let pre = crate::hooks::run(&hooks, "PreToolUse", name, hook_input.clone(), &root).await;
        if let Some(reason) = &pre.block {
            return (text_block(format!("A PreToolUse hook blocked this tool call: {reason}")), true);
        }
        let mut perm = codetools::check_permission(&mode, name, input, &rules, Some(session));
        match pre.permission.as_deref() {
            Some("allow") if perm == Perm::Ask => perm = Perm::Allow,
            Some("ask") if perm == Perm::Allow => perm = Perm::Ask,
            _ => {}
        }
        match perm {
            Perm::Deny(msg) => return (text_block(msg), true),
            Perm::Ask => {
                // reads the file and diffs it (up to half a second): not on
                // a runtime worker
                let diff = {
                    let (session, name, input) = (session.clone(), name.to_string(), input.clone());
                    tokio::task::spawn_blocking(move || codetools::preview_diff(Some(&session), &name, &input)).await.ok().flatten()
                };
                let reply = ask_permission(
                    app, st, conv_id, id,
                    json!({"kind": "code", "tool": name, "input": input, "diff": diff, "cwd": session.cwd.lock().display().to_string()}),
                    live, renderer, am, cancel,
                )
                .await;
                match reply.as_ref().map(|r| r.decision.as_str()) {
                    Some("once") => {}
                    Some("session") | Some("chat") | Some("always") => {
                        let new_rules = codetools::session_rules(session, name, input);
                        update_conv_options(st, conv_id, |o| {
                            let mut arr = o["allowed_rules"].as_array().cloned().unwrap_or_default();
                            for rule in new_rules {
                                if !arr.iter().any(|x| x == &json!(rule)) {
                                    arr.push(json!(rule));
                                }
                            }
                            o["allowed_rules"] = json!(arr);
                        });
                    }
                    Some("approve_edits") => {
                        update_conv_options(st, conv_id, |o| o["permission_mode"] = json!("acceptEdits"));
                        emit_conv_updated(app, conv_id);
                    }
                    _ => return (denial(&reply), true),
                }
            }
            Perm::Allow => {}
        }
        let timeout = Duration::from_secs(settings.bash_timeout_secs);
        let progress: Option<codetools::ProgressFn> = if name == "Bash" {
            let (app2, conv2, id2) = (app.clone(), conv_id.to_string(), id.to_string());
            Some(Arc::new(move |text: &str| {
                let _ = app2.emit("tool-progress", json!({"conversation_id": conv2, "tool_use_id": id2, "text": text}));
            }))
        } else {
            None
        };
        let out = codetools::execute(session.clone(), name, input, cancel, timeout, progress).await;
        if matches!(name, "Write" | "Edit" | "Bash") {
            let _ = app.emit("files-changed", json!({"conversation_id": conv_id}));
        }
        let mut content = out.content;
        if hooks.events.contains_key("PostToolUse") {
            let mut post_in = hook_input;
            post_in["tool_response"] = json!(render::plain_text(&Value::Array(content.clone())));
            let post = crate::hooks::run(&hooks, "PostToolUse", name, post_in, &root).await;
            if let Some(reason) = post.block {
                content.push(json!({"type": "text", "text": format!("PostToolUse hook feedback: {reason}")}));
            }
            for c in post.context {
                content.push(json!({"type": "text", "text": format!("<system-reminder>{c}</system-reminder>")}));
            }
        }
        return (content, out.is_error);
    }

    if MEMORY_TOOLS.contains(&name) && conv.kind != "code" {
        return run_memory_tool(st, conv_id, name, input);
    }

    // MCP tool
    if let Some((server, tool)) = rc.mcp_names.get(name) {
        let key = format!("{server}/{tool}");
        let policy = settings
            .tool_policies
            .get(&key)
            .and_then(|v| v.as_str())
            .unwrap_or("ask")
            .to_string();
        let allowed_chat = opt_strs(&conv.options, "allowed_tools").iter().any(|t| t == name);
        let code_mode = if conv.kind == "code" {
            conv.options["permission_mode"].as_str().unwrap_or(&settings.code_permission_mode).to_string()
        } else {
            String::new()
        };
        let read_only = rc.mcp_read_only.contains(name);
        let bypass = code_mode == "bypassPermissions";
        if policy == "deny" {
            return (text_block("This tool is disabled by the user."), true);
        }
        if code_mode == "plan" && !read_only {
            return (text_block(codetools::PLAN_MCP_DENIED), true);
        }
        // Auto mode runs tools that declare themselves read-only.
        let auto_ok = code_mode == "auto" && read_only;
        if policy != "allow" && !allowed_chat && !bypass && !auto_ok {
            let reply = ask_permission(
                app, st, conv_id, id,
                json!({"kind": "mcp", "tool": name, "server": server, "tool_name": tool, "input": input}),
                live, renderer, am, cancel,
            )
            .await;
            match reply.as_ref().map(|r| r.decision.as_str()) {
                Some("once") => {}
                Some("chat") | Some("session") => {
                    let n = name.to_string();
                    update_conv_options(st, conv_id, |o| {
                        let mut arr = o["allowed_tools"].as_array().cloned().unwrap_or_default();
                        arr.push(json!(n));
                        o["allowed_tools"] = json!(arr);
                    });
                }
                Some("always") => {
                    {
                        let mut s = st.settings.write();
                        s.tool_policies.insert(key.clone(), json!("allow"));
                        if let Err(e) = s.save(&st.db) {
                            log::warn!("saving the tool policy failed: {e}");
                        }
                    }
                    // the settings window shows (and saves) its own copy
                    let _ = app.emit("settings-updated", ());
                }
                _ => return (denial(&reply), true),
            }
        }
        if let (true, Some(session)) = (conv.kind == "code", rc.code.as_ref()) {
            let hooks = hook_config(st, &session.root);
            let pre = crate::hooks::run(&hooks, "PreToolUse", &format!("mcp__{name}"), json!({"session_id": conv_id, "tool_name": format!("mcp__{name}"), "tool_input": input}), &session.root).await;
            if let Some(reason) = pre.block {
                return (text_block(format!("A PreToolUse hook blocked this tool call: {reason}")), true);
            }
        }
        let call = st.mcp.call(name, input);
        let r = tokio::select! {
            r = call => r,
            _ = cancel.cancelled() => return (text_block("The user interrupted this tool call."), true),
        };
        return match r {
            Ok((blocks, is_error)) => (blocks, is_error),
            Err(e) => (text_block(format!("Error: {e}")), true),
        };
    }
    (text_block(format!("Tool `{name}` is not available.")), true)
}

// ------------------------------------------------------------- titles

pub async fn generate_title(app: &AppHandle, st: &Arc<AppState>, conv_id: &str) {
    let Ok(conv) = st.db.conv_get(conv_id) else { return };
    let Some(leaf) = conv.current_leaf.clone() else { return };
    let Ok(path) = st.db.path_to(&leaf) else { return };
    let user_text = path
        .iter()
        .find(|m| m.role == "user" && !render::is_tool_result_msg(m))
        .map(|m| render::plain_text(&m.content))
        .unwrap_or_default();
    let assistant_text = path
        .iter()
        .find(|m| m.role == "assistant")
        .map(|m| render::plain_text(&m.content))
        .unwrap_or_default();
    let fallback = {
        let first = user_text.lines().find(|l| !l.trim().is_empty()).unwrap_or("New chat").trim();
        crate::util::truncate_chars(first, 60)
    };
    let title = match title_via_api(st, &user_text, &assistant_text).await {
        Some(t) => t,
        None => fallback,
    };
    // Only set if still untitled (user may have renamed meanwhile)
    if st.db.conv_get(conv_id).map(|c| c.title.is_empty()).unwrap_or(false) {
        let _ = st.db.conv_set_title(conv_id, &title);
        emit_conv_updated(app, conv_id);
    }
}

async fn title_via_api(st: &AppState, user_text: &str, assistant_text: &str) -> Option<String> {
    let client = st.client().ok()?;
    let model_id = st.settings.read().title_model.clone();
    let model = models::info(&model_id);
    let mut body = json!({
        "model": model_id,
        "max_tokens": 1024,
        "system": prompts::TITLE_PROMPT,
        "messages": [{"role": "user", "content": format!(
            "<conversation>\nUser: {}\n\nAssistant: {}\n</conversation>",
            crate::util::truncate_chars(user_text, 3000),
            crate::util::truncate_chars(assistant_text, 1500)
        )}],
    });
    match model.thinking {
        Thinking::Disableable => {
            body["thinking"] = json!({"type": "disabled"});
        }
        Thinking::BetweenTools => {
            body["thinking"] = json!({"type": "between_tools"});
        }
        _ => {}
    }
    if model.efforts.contains(&"low") {
        body["output_config"] = json!({"effort": "low"});
    }
    let r = tokio::time::timeout(Duration::from_secs(30), client.create(&body, &[])).await.ok()?.ok()?;
    if r["stop_reason"] == "refusal" {
        return None;
    }
    let text: String = r["content"]
        .as_array()?
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("");
    let t = text
        .lines()
        .find(|l| !l.trim().is_empty())?
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '*' || c == '#')
        .trim()
        .trim_end_matches('.')
        .to_string();
    if t.is_empty() {
        None
    } else {
        Some(crate::util::truncate_chars(&t, 80))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn test_state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("cr-state-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")))
    }

    #[test]
    fn a_reserved_conversation_refuses_another_turn_until_released() {
        let st = test_state();
        let a = RunSlot::reserve(&st, "c", "busy").unwrap();
        // what a second send / regenerate / rewind / compact would hit
        assert_eq!(RunSlot::reserve(&st, "c", "busy").err().map(|e| e.to_string()), Some("busy".into()));
        assert!(RunSlot::reserve(&st, "other", "busy").is_ok(), "other conversations aren't affected");
        // Stop reaches a reserved conversation like a running one
        stop(&st, "c");
        assert!(a.cancelled());
        drop(a);
        assert!(RunSlot::reserve(&st, "c", "busy").is_ok(), "released on drop (any early return)");
    }

    #[test]
    fn concurrent_option_updates_are_all_kept() {
        let st = test_state();
        let c = st.db.conv_create("code", "m", None, None, &json!({})).unwrap();
        // the engine (cc_session, allowed_rules…) and the UI (permission mode,
        // instructions) writing different keys at the same time
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let st = st.clone();
                let id = c.id.clone();
                std::thread::spawn(move || {
                    for n in 0..50 {
                        update_conv_options(&st, &id, |o| o[format!("k{i}")] = json!(n));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let o = st.db.conv_get(&c.id).unwrap().options;
        for i in 0..8 {
            assert_eq!(o[format!("k{i}")], 49, "update of k{i} lost: {o}");
        }
    }

    fn rc(fp: &str) -> ReqCtx {
        ReqCtx {
            model_id: "claude-opus-5-5".into(),
            model: models::info("claude-opus-5-5"),
            system: "s".into(),
            tools: vec![],
            fingerprint: fp.into(),
            prefix_blocks: vec![json!({"type":"image","source":{}})],
            code: None,
            mcp_names: HashMap::new(),
            mcp_read_only: HashSet::new(),
            first_party: true,
        }
    }

    fn m(id: &str, role: &str, content: Value, meta: Value) -> Message {
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
    fn fallback_echo_rule() {
        let c = json!([
            {"type":"thinking","thinking":"a","signature":"s1"},
            {"type":"text","text":"partial"},
            {"type":"tool_use","id":"t0","name":"x","input":{}},
            {"type":"fallback","from":{"model":"a"},"to":{"model":"b"}},
            {"type":"thinking","thinking":"b","signature":"s2"},
            {"type":"text","text":"answer"},
            {"type":"text","text":""}
        ]);
        let out = sanitize_assistant(&c, &HashSet::from(["t0".to_string()]));
        let types: Vec<&str> = out.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(types, vec!["text", "fallback", "thinking", "text"]);
    }

    #[test]
    fn thinking_strip_and_tool_pairing() {
        let path = vec![
            m("u1", "user", json!([{"type":"text","text":"hi"}]), json!({})),
            m("a1", "assistant", json!([{"type":"thinking","thinking":"x","signature":"s"},{"type":"text","text":"yo"}]), json!({"fingerprint":"old"})),
            m("u2", "user", json!([{"type":"text","text":"again"}]), json!({})),
            m("a2", "assistant", json!([{"type":"thinking","thinking":"y","signature":"s"},{"type":"tool_use","id":"t1","name":"n","input":{}},{"type":"tool_use","id":"t2","name":"n","input":{}}]), json!({"fingerprint":"new"})),
            m("u3", "user", json!([{"type":"tool_result","tool_use_id":"t1","content":"ok"}]), json!({"tool_results":true})),
        ];
        let msgs = build_messages(&path, &rc("new"));
        assert_eq!(msgs.len(), 5);
        // first user message got the project prefix block
        assert_eq!(msgs[0]["content"][0]["type"], "image");
        // a1 thinking stripped (fingerprint changed), a2 kept
        assert_eq!(msgs[1]["content"].as_array().unwrap().len(), 1);
        assert_eq!(msgs[3]["content"][0]["type"], "thinking");
        // unanswered tool_use t2 dropped
        assert_eq!(msgs[3]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn failed_assistant_is_skipped_and_users_merge() {
        let path = vec![
            m("u1", "user", json!([{"type":"text","text":"hi"}]), json!({})),
            m("a1", "assistant", json!([]), json!({"error":{"message":"x"}})),
            m("u2", "user", json!([{"type":"text","text":"again"}]), json!({})),
        ];
        let mut r = rc("f");
        r.prefix_blocks.clear();
        let msgs = build_messages(&path, &r);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn budget_pause_then_continue() {
        let path = vec![
            m("u1", "user", json!([{"type":"text","text":"build it"}]), json!({})),
            m("a1", "assistant", json!([{"type":"tool_use","id":"t1","name":"Bash","input":{}}]), json!({})),
            m("u2", "user", json!([{"type":"tool_result","tool_use_id":"t1","content":"ok"}]), json!({"tool_results":true})),
            m("a2", "assistant", json!([]), json!({"budget_stop":{"budget":1.0,"spent":1.2}})),
            m("u3", "user", json!([{"type":"text","text":"Continue"}]), json!({})),
        ];
        let mut r = rc("f");
        r.prefix_blocks.clear();
        let msgs = build_messages(&path, &r);
        // the paused (empty) response is left out; the tool result and
        // "Continue" go in one user message
        assert_eq!(msgs.len(), 3);
        let last = msgs[2]["content"].as_array().unwrap();
        assert_eq!(last[0]["type"], "tool_result");
        assert_eq!(last[1]["text"], "Continue");
    }

    #[test]
    fn conversation_instructions_block() {
        assert_eq!(conversation_instructions(&json!({})), None);
        assert_eq!(conversation_instructions(&json!({"instructions": "  "})), None);
        let b = conversation_instructions(&json!({"instructions": "Answer in Turkish."})).unwrap();
        assert!(b.starts_with("<conversation_instructions>") && b.contains("Answer in Turkish.") && b.ends_with("</conversation_instructions>"));
    }

    #[test]
    fn turn_budget_prefers_the_conversation() {
        let mut s = Settings::default();
        let mut c = Conversation { options: json!({}), ..Default::default() };
        assert_eq!(turn_budget(&c, &s), 0.0);
        s.turn_budget_usd = 2.0;
        assert_eq!(turn_budget(&c, &s), 2.0);
        c.options = json!({"budget_usd": 0.5});
        assert_eq!(turn_budget(&c, &s), 0.5);
        c.options = json!({"budget_usd": 0});
        assert_eq!(turn_budget(&c, &s), 0.0);
        c.options = json!({"budget_usd": null});
        assert_eq!(turn_budget(&c, &s), 2.0);
    }

    /// `build_messages` and `sanitize_assistant` as they were (verbatim),
    /// before they stopped copying the conversation three times over.
    mod before {
        use super::super::{is_thinking, tool_result_ids, ReqCtx};
        use crate::db::Message;
        use crate::render;
        use serde_json::{json, Value};
        use std::collections::HashSet;

        fn sanitize_assistant(content: &Value, answered: &HashSet<String>) -> Vec<Value> {
            let blocks = content.as_array().cloned().unwrap_or_default();
            let last_fallback = blocks.iter().rposition(|b| b["type"] == "fallback");
            let server_results: HashSet<String> = blocks
                .iter()
                .filter(|b| b["type"].as_str().map(|t| t.ends_with("_tool_result")).unwrap_or(false))
                .filter_map(|b| b["tool_use_id"].as_str().map(|s| s.to_string()))
                .collect();
            let known = |t: &str| {
                matches!(t, "text" | "thinking" | "redacted_thinking" | "tool_use" | "server_tool_use" | "fallback" | "compaction")
                    || t.ends_with("_tool_result")
            };
            let mut out = vec![];
            for (i, b) in blocks.into_iter().enumerate() {
                let t = b["type"].as_str().unwrap_or("").to_string();
                let before_fallback = last_fallback.map(|f| i < f).unwrap_or(false);
                if before_fallback {
                    match t.as_str() {
                        "thinking" | "redacted_thinking" | "tool_use" => continue,
                        "server_tool_use" if !server_results.contains(b["id"].as_str().unwrap_or("")) => continue,
                        x if !known(x) => continue,
                        _ => {}
                    }
                }
                match t.as_str() {
                    "thinking" => {
                        if b["signature"].as_str().map(|s| s.is_empty()).unwrap_or(true) {
                            continue;
                        }
                    }
                    "redacted_thinking" => {
                        if b["data"].as_str().map(|s| s.is_empty()).unwrap_or(true) {
                            continue;
                        }
                    }
                    "text" => {
                        if b["text"].as_str().map(|s| s.is_empty()).unwrap_or(true) {
                            continue;
                        }
                    }
                    "tool_use" => {
                        if !answered.contains(b["id"].as_str().unwrap_or("")) {
                            continue;
                        }
                    }
                    "server_tool_use"
                        if !server_results.contains(b["id"].as_str().unwrap_or("")) => {
                            continue;
                        }
                    _ => {}
                }
                out.push(b);
            }
            out
        }
        pub(super) fn build_messages(path: &[Message], rc: &ReqCtx) -> Vec<Value> {
            let compacted = crate::compact::apply(path);
            let path: &[Message] = &compacted;
            // Thinking blocks produced under a different system prompt / tool set must
            // go: remove the leading run of blocks up to the last mismatching one so
            // the remaining blocks stay valid for preserved thinking.
            let mut strip_upto: Option<usize> = None;
            for (i, m) in path.iter().enumerate() {
                if m.role == "assistant"
                    && m.content.as_array().map(|a| a.iter().any(is_thinking)).unwrap_or(false)
                    && m.meta["fingerprint"].as_str() != Some(rc.fingerprint.as_str())
                {
                    strip_upto = Some(i);
                }
            }
            let mut out: Vec<Value> = vec![];
            let mut kept_tool_ids: HashSet<String> = HashSet::new();
            let mut first_user_done = false;
            for (i, m) in path.iter().enumerate() {
                if m.role == "assistant" {
                    let answered = path
                        .get(i + 1)
                        .filter(|n| render::is_tool_result_msg(n))
                        .map(tool_result_ids)
                        .unwrap_or_default();
                    let mut blocks = sanitize_assistant(&m.content, &answered);
                    if strip_upto.map(|s| i <= s).unwrap_or(false) {
                        blocks.retain(|b| !is_thinking(b));
                    }
                    kept_tool_ids = blocks
                        .iter()
                        .filter(|b| b["type"] == "tool_use")
                        .filter_map(|b| b["id"].as_str().map(|s| s.to_string()))
                        .collect();
                    if blocks.is_empty() {
                        // a failed response, or one paused at the spending limit
                        // before its request was sent
                        if !m.meta["error"].is_null() || m.meta["budget_stop"].is_object() {
                            continue;
                        }
                        blocks.push(json!({"type": "text", "text": "(no response)"}));
                    }
                    out.push(json!({"role": "assistant", "content": blocks}));
                } else {
                    let mut content: Vec<Value> = match &m.content {
                        Value::Array(a) => a.clone(),
                        Value::String(s) => vec![json!({"type": "text", "text": s})],
                        _ => vec![],
                    };
                    if render::is_tool_result_msg(m) {
                        content.retain(|b| {
                            b["type"] != "tool_result"
                                || kept_tool_ids.contains(b["tool_use_id"].as_str().unwrap_or(""))
                        });
                    } else if !first_user_done {
                        first_user_done = true;
                        if !rc.prefix_blocks.is_empty() {
                            let mut c = rc.prefix_blocks.clone();
                            c.extend(content);
                            content = c;
                        }
                    }
                    content.retain(|b| !(b["type"] == "text" && b["text"].as_str().map(|s| s.is_empty()).unwrap_or(true)));
                    if content.is_empty() {
                        continue;
                    }
                    out.push(json!({"role": "user", "content": content}));
                }
            }
            // Merge consecutive messages with the same role.
            let mut merged: Vec<Value> = vec![];
            for m in out {
                if let Some(last) = merged.last_mut() {
                    if last["role"] == m["role"] {
                        if let (Some(a), Some(b)) = (last["content"].as_array_mut(), m["content"].as_array()) {
                            a.extend(b.iter().cloned());
                            continue;
                        }
                    }
                }
                merged.push(m);
            }
            merged
        }
    }

    #[test]
    fn request_messages_are_the_same_as_before() {
        let mut path = vec![m("u0", "user", json!([{"type": "text", "text": "Fix the parser."}]), json!({}))];
        path.extend(crate::render::fixture::turn_prefix(25));
        // a refusal fallback, interrupted thinking, unanswered and server
        // tool calls, an empty text, a failed response, a string content
        path.push(m("a-x", "assistant", json!([
            {"type": "thinking", "thinking": "x", "signature": ""},
            {"type": "server_tool_use", "id": "s1", "name": "web_search", "input": {}},
            {"type": "tool_use", "id": "gone", "name": "Bash", "input": {}},
            {"type": "fallback", "from": {"model": "a"}, "to": {"model": "b"}},
            {"type": "server_tool_use", "id": "s2", "name": "web_search", "input": {}},
            {"type": "web_search_tool_result", "tool_use_id": "s2", "content": []},
            {"type": "redacted_thinking", "data": "d"},
            {"type": "text", "text": ""},
            {"type": "mystery"}
        ]), json!({"fingerprint": "other"})));
        path.push(m("u-x", "user", json!("plain string content"), json!({})));
        path.push(m("a-y", "assistant", json!([]), json!({"error": {"message": "x"}})));
        path.push(m("u-y", "user", json!([{"type": "text", "text": ""}, {"type": "text", "text": "again"}]), json!({})));
        let mut compacted = path.clone();
        compacted[10].meta["compact_summary"] = json!("what happened before");
        for fp in ["fp", "other"] {
            let mut r = rc(fp);
            for p in [&path, &compacted] {
                let want = serde_json::to_string(&before::build_messages(p, &r)).unwrap();
                assert_eq!(serde_json::to_string(&build_messages(p, &r)).unwrap(), want);
                r.prefix_blocks.clear();
                let want = serde_json::to_string(&before::build_messages(p, &r)).unwrap();
                assert_eq!(serde_json::to_string(&build_messages(p, &r)).unwrap(), want);
            }
        }
    }

    /// Time the request messages built at each step of an agent turn, from
    /// a conversation of 200 tool calls (a few KB of output each).
    #[test]
    #[ignore]
    fn bench_build_messages() {
        let mut path = vec![m("u0", "user", json!([{"type": "text", "text": "Fix the parser."}]), json!({}))];
        path.extend(crate::render::fixture::turn_prefix(200));
        let r = rc("fp");
        let _ = build_messages(&path, &r);
        let n = 50;
        let t0 = Instant::now();
        for _ in 0..n {
            std::hint::black_box(build_messages(&path, &r));
        }
        let bytes = serde_json::to_string(&build_messages(&path, &r)).unwrap().len();
        eprintln!("build_messages: {:.2} ms per step ({} KB of messages)", t0.elapsed().as_secs_f64() * 1000.0 / n as f64, bytes / 1024);
    }

    /// Each streaming update renders the turn byte-for-byte as before (when
    /// the whole turn was rendered from its start, with the old renderer)
    /// and leaves the same live state for a view opened meanwhile.
    #[test]
    fn live_updates_render_as_before() {
        use crate::render::fixture;
        let st = test_state();
        let conv_id = "c";
        let prefix = fixture::turn_prefix(2);
        let mut mcp_names = HashMap::new();
        mcp_names.insert("mcp__github__get_issue".to_string(), ("github".to_string(), "get_issue".to_string()));
        let siblings = json!({"index": 1, "count": 2, "ids": ["x", "a0"]});
        let mut tracker = ArtifactTracker::default();
        tracker.bump("report");
        let renderer = TurnRenderer::new(Some("a0".into()), siblings.clone(), prefix.clone(), tracker.clone(), mcp_names.clone(), true);
        let am = fixture::msg("am", "assistant", json!([]), json!({"model": "claude-opus-5-5", "fingerprint": "fp"}));
        // what the content of an earlier pause_turn part looks like
        let paused = vec![json!({"type": "text", "text": "Searching first. "}), json!({"type": "server_tool_use", "id": "s9", "name": "web_search", "input": {"query": "q"}})];
        for base in [vec![], paused] {
            st.live.lock().insert(conv_id.to_string(), LiveRun { msg_id: "am".into(), content: json!([]), ..Default::default() });
            let mut cur = am.clone();
            let mut acc = Accumulator::default();
            for (i, ev) in fixture::reply_events(8, 0).into_iter().enumerate() {
                acc.apply(&ev).unwrap();
                // every block boundary, and enough deltas (the old renderer is slow)
                if ev["type"] == "content_block_delta" && i % 5 != 0 {
                    continue;
                }
                // before: the content cloned into a copy of the message and
                // into `st.live`, and all of the turn rendered
                let (c, streaming) = live_content(&base, &acc);
                let mut old = am.clone();
                old.content = c.clone();
                let lr = LiveRun { msg_id: "am".into(), content: c, live_tools: HashMap::new(), streaming_tools: streaming };
                let mut msgs: Vec<&Message> = prefix.iter().collect();
                msgs.push(&old);
                let ctx = RenderCtx { math: true, live_tools: &lr.live_tools, streaming_tools: &lr.streaming_tools, mcp_names: &mcp_names, streaming_msg: Some(&old.id) };
                let mut t = render::reference::assistant_turn(&msgs, &ctx, &mut tracker.clone());
                t["id"] = json!("a0");
                t["siblings"] = siblings.clone();
                let want = serde_json::to_string(&json!({"conversation_id": conv_id, "turn": t})).unwrap();
                let mut got = String::new();
                live_tick(&st, conv_id, &renderer, &mut cur, &base, &acc, |t| got = serde_json::to_string(&json!({"conversation_id": conv_id, "turn": t})).unwrap());
                assert_eq!(got, want, "after {ev}");
                let lives = st.live.lock();
                let l = &lives[conv_id];
                assert_eq!((&l.msg_id, &l.content, &l.streaming_tools, &l.live_tools), (&lr.msg_id, &lr.content, &lr.streaming_tools, &lr.live_tools));
            }
            // then the final update and the tool's live states
            let mut done = am.clone();
            done.content = Value::Array(acc.content());
            for tools in [vec![], vec![("t_write", "awaiting_permission")], vec![("t_write", "running")]] {
                let live = LiveRun { live_tools: tools.into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), ..Default::default() };
                let mut msgs: Vec<&Message> = prefix.iter().collect();
                msgs.push(&done);
                let ctx = RenderCtx { math: true, live_tools: &live.live_tools, streaming_tools: &live.streaming_tools, mcp_names: &mcp_names, streaming_msg: Some(&done.id) };
                let mut want = render::reference::assistant_turn(&msgs, &ctx, &mut tracker.clone());
                want["id"] = json!("a0");
                want["siblings"] = siblings.clone();
                let got = renderer.render(&done, &live.live_tools, &live.streaming_tools);
                assert_eq!(serde_json::to_string(&got).unwrap(), serde_json::to_string(&want).unwrap());
                renderer.give_back(got);
            }
        }
    }

    /// Time (and, under DHAT, count the allocations of) the streaming
    /// updates of a reply at the end of a long agentic turn: 40 tool calls
    /// before it, then thinking, a long text with code blocks and a Write
    /// call whose input streams. `BENCH_TICKS=0` runs the setup only;
    /// `BENCH_NO_EMIT=1` leaves out what emit_turn does with the turn.
    #[test]
    #[ignore]
    fn bench_live_ticks() {
        use crate::render::fixture;
        let st = test_state();
        let conv_id = "c";
        let rounds: usize = std::env::var("BENCH_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        let max_ticks: usize = std::env::var("BENCH_TICKS").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
        let no_emit = std::env::var("BENCH_NO_EMIT").is_ok();
        let prefix = fixture::turn_prefix(rounds);
        let mut mcp_names = HashMap::new();
        mcp_names.insert("mcp__github__get_issue".to_string(), ("github".to_string(), "get_issue".to_string()));
        let renderer = TurnRenderer::new(Some("a0".into()), json!({"index": 0, "count": 1, "ids": ["a0"]}), prefix, ArtifactTracker::default(), mcp_names, true);
        let mut cur = fixture::msg("am", "assistant", json!([]), json!({"model": "claude-opus-5-5", "fingerprint": "fp"}));
        let live = LiveRun { msg_id: "am".into(), content: json!([]), ..Default::default() };
        st.live.lock().insert(conv_id.to_string(), live.clone());
        let base: Vec<Value> = vec![];
        // what happened earlier in the app: syntaxes loaded (each compiles
        // its patterns on first use), caches filled by the earlier steps;
        // a reply of the same shape streamed once, with other text
        let mut warm = Accumulator::default();
        for ev in fixture::reply_events(40, 1) {
            warm.apply(&ev).unwrap();
            live_tick(&st, conv_id, &renderer, &mut cur, &base, &warm, |_| {});
        }
        let events = fixture::reply_events(40, 0);
        let mut acc = Accumulator::default();
        // per phase of the reply (thinking, text, tool input): ticks, render ns, emit ns, bytes
        let mut phases = [(0u64, 0u128, 0u128, 0usize); 3];
        let mut ticks = 0usize;
        for (i, ev) in events.iter().enumerate() {
            acc.apply(ev).unwrap();
            // one update per two events: about what 45 ms of streaming holds
            if i % 2 == 1 && ticks < max_ticks {
                ticks += 1;
                let phase = acc.blocks.len().clamp(1, 3) - 1;
                let (mut emit_ns, mut len) = (0u128, 0usize);
                let t0 = Instant::now();
                live_tick(&st, conv_id, &renderer, &mut cur, &base, &acc, |turn| {
                    if no_emit {
                        return;
                    }
                    // what emit_turn does with it (to_value, then the event's JSON)
                    let e0 = Instant::now();
                    let payload = json!({"conversation_id": conv_id, "turn": turn});
                    len = serde_json::to_string(&payload).unwrap().len();
                    drop(payload);
                    emit_ns = e0.elapsed().as_nanos();
                });
                let total = t0.elapsed().as_nanos();
                let p = &mut phases[phase];
                p.0 += 1;
                p.1 += total - emit_ns;
                p.2 += emit_ns;
                p.3 += len;
            }
        }
        let all = phases.iter().fold((0u64, 0u128, 0u128, 0usize), |a, p| (a.0 + p.0, a.1 + p.1, a.2 + p.2, a.3 + p.3));
        for (name, p) in ["thinking", "text", "tool input", "all"].iter().zip(phases.iter().chain([&all])) {
            if p.0 > 0 {
                eprintln!(
                    "{rounds} rounds, {name}: {} ticks, render {:.1} us/tick, emit {:.1} us/tick, {} bytes/tick",
                    p.0,
                    p.1 as f64 / p.0 as f64 / 1000.0,
                    p.2 as f64 / p.0 as f64 / 1000.0,
                    p.3 / p.0 as usize
                );
            }
        }
    }
}
