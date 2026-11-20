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
    pub first_party: bool,
}

// ------------------------------------------------------------------ events

pub fn emit_turn(app: &AppHandle, conv_id: &str, turn: &Value) {
    let _ = app.emit("turn", json!({"conversation_id": conv_id, "turn": turn}));
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
    let mut tools: Vec<Value> = vec![];
    let mut prefix_blocks = vec![];
    let mut code = None;
    let system = if conv.kind == "code" {
        let cwd = conv.cwd.clone().unwrap_or_default();
        code = Some(st.code_session(&conv.id, &cwd));
        tools.extend(codetools::definitions(eager));
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
            };
            prefix_blocks.push(attachments::to_block(&a));
        }
        let web = opt_bool(opts, "web_search", settings.web_search_default);
        if web {
            tools.extend(models::web_tools(&model));
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
        first_party,
    })
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
            "server_tool_use" => {
                if !server_results.contains(b["id"].as_str().unwrap_or("")) {
                    continue;
                }
            }
            _ => {}
        }
        out.push(b);
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
                if !m.meta["error"].is_null() {
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

fn base_body(rc: &ReqCtx, settings: &Settings, conv: &Conversation, messages: Vec<Value>) -> (Value, Vec<String>) {
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

async fn stream_with_recovery<F>(
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
}

impl TurnRenderer {
    fn render(&self, current: &Message, live: &LiveRun) -> Value {
        let mut msgs: Vec<&Message> = self.prefix.iter().collect();
        msgs.push(current);
        let ctx = RenderCtx {
            math: self.math,
            live_tools: &live.live_tools,
            streaming_tools: &live.streaming_tools,
            mcp_names: &self.mcp_names,
            streaming_msg: Some(&current.id),
        };
        let mut tracker = self.tracker.clone();
        let mut t = render::assistant_turn(&msgs, &ctx, &mut tracker);
        if let Some(f) = &self.first_id {
            t["id"] = json!(f);
        }
        t["siblings"] = self.siblings.clone();
        t
    }
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

pub fn start(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, parent_id: &str) -> AppResult<()> {
    let cancel = CancellationToken::new();
    {
        let mut runs = st.runs.lock();
        if runs.contains_key(conv_id) {
            return Err(AppError::msg("Claude is already responding in this conversation."));
        }
        runs.insert(conv_id.to_string(), cancel.clone());
    }
    emit_state(app, conv_id, true, None);
    let app = app.clone();
    let st = st.clone();
    let conv_id = conv_id.to_string();
    let parent_id = parent_id.to_string();
    tauri::async_runtime::spawn(async move {
        let res = run(&app, &st, &conv_id, parent_id, &cancel).await;
        st.runs.lock().remove(&conv_id);
        st.live.lock().remove(&conv_id);
        st.pending_perms.lock().retain(|_, v| v["conversation_id"] != conv_id.as_str());
        let err = res.as_ref().err().filter(|e| !matches!(e, AppError::Cancelled));
        if let Some(e) = err {
            log::error!("generation failed: {e}");
        }
        emit_state(&app, &conv_id, false, err);
        emit_conv_updated(&app, &conv_id);
        if res.is_ok() {
            let needs_title = st.db.conv_get(&conv_id).map(|c| c.title.is_empty()).unwrap_or(false);
            if needs_title {
                generate_title(&app, &st, &conv_id).await;
            }
        }
    });
    Ok(())
}

pub fn stop(st: &AppState, conv_id: &str) {
    if let Some(c) = st.runs.lock().get(conv_id) {
        c.cancel();
    }
}

async fn run(app: &AppHandle, st: &Arc<AppState>, conv_id: &str, mut parent: String, cancel: &CancellationToken) -> AppResult<()> {
    let mut steps = 0usize;
    loop {
        steps += 1;
        let conv = st.db.conv_get(conv_id)?;
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
        let renderer = TurnRenderer {
            first_id: first_id.clone(),
            siblings,
            tracker: view::tracker_for(&path[..turn_start]),
            prefix,
            mcp_names: rc.mcp_names.clone(),
            math: settings.latex_enabled,
        };
        let mut live = LiveRun {
            msg_id: am_id.clone(),
            content: json!([]),
            ..Default::default()
        };
        st.live.lock().insert(conv_id.to_string(), live.clone());
        emit_turn(app, conv_id, &renderer.render(&am, &live));

        if steps > max_steps {
            am.meta["error"] = json!({"message": "Stopped after too many tool calls in one response.", "kind": "error"});
            st.db.msg_update(&am_id, &am.content, &am.meta)?;
            emit_turn(app, conv_id, &renderer.render(&am, &live));
            return Ok(());
        }

        let client = match st.client() {
            Ok(c) => c,
            Err(e) => {
                am.meta["error"] = serde_json::to_value(&e)?;
                st.db.msg_update(&am_id, &am.content, &am.meta)?;
                emit_turn(app, conv_id, &renderer.render(&am, &live));
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
            let am_ref = am.clone();
            let mut on_event = |acc: &Accumulator, _changed: Option<usize>| {
                if last_emit.elapsed() < Duration::from_millis(45) {
                    return;
                }
                last_emit = Instant::now();
                let (c, streaming) = live_content(&base, acc);
                let mut cur = am_ref.clone();
                cur.content = c.clone();
                let lr = {
                    let mut lives = st2.live.lock();
                    let l = lives.entry(conv_id.to_string()).or_default();
                    l.msg_id = cur.id.clone();
                    l.content = c;
                    l.streaming_tools = streaming;
                    l.clone()
                };
                emit_turn(&app2, conv_id, &renderer.render(&cur, &lr));
            };
            let res = stream_with_recovery(st, &client, &mut body, &mut betas, cancel, &mut on_event).await;
            match res {
                Err(e) => {
                    error = Some(e);
                    break;
                }
                Ok(out) => {
                    let acc = out.acc;
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
        emit_turn(app, conv_id, &renderer.render(&am, &live));

        if cancelled {
            return Ok(());
        }
        if let Some(e) = error {
            return Err(e);
        }
        if stop_reason.as_deref() != Some("tool_use") {
            return Ok(());
        }

        // ---- run tools
        let send_form = sanitize_assistant(&am.content, &all_tool_ids(&am.content));
        let tool_uses: Vec<Value> = send_form.into_iter().filter(|b| b["type"] == "tool_use").collect();
        if tool_uses.is_empty() {
            return Ok(());
        }
        let mut results: Vec<Value> = vec![];
        for tu in &tool_uses {
            let id = tu["id"].as_str().unwrap_or("").to_string();
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
            emit_turn(app, conv_id, &renderer.render(&am, &live));
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
            let mut r2 = TurnRenderer {
                first_id: renderer.first_id.clone(),
                siblings: renderer.siblings.clone(),
                prefix: renderer.prefix.clone(),
                tracker: renderer.tracker.clone(),
                mcp_names: renderer.mcp_names.clone(),
                math: renderer.math,
            };
            r2.prefix.push(am.clone());
            live.live_tools.clear();
            let mut fake = tr.clone();
            fake.role = "user".into();
            let mut msgs: Vec<&Message> = r2.prefix.iter().collect();
            msgs.push(&fake);
            let ctx = RenderCtx {
                math: r2.math,
                live_tools: &live.live_tools,
                streaming_tools: &live.streaming_tools,
                mcp_names: &r2.mcp_names,
                streaming_msg: None,
            };
            let mut tracker = r2.tracker.clone();
            let mut t = render::assistant_turn(&msgs, &ctx, &mut tracker);
            if let Some(f) = &r2.first_id {
                t["id"] = json!(f);
            }
            t["siblings"] = r2.siblings.clone();
            t["streaming"] = json!(true);
            emit_turn(app, conv_id, &t);
        }
        if cancel.is_cancelled() {
            return Ok(());
        }
        parent = tr.id;
    }
}

fn all_tool_ids(content: &Value) -> HashSet<String> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "tool_use")
        .filter_map(|b| b["id"].as_str().map(|s| s.to_string()))
        .collect()
}

fn save_artifacts(st: &AppState, conv_id: &str, msg_id: &str, text: &str) {
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
    emit_turn(app, conv_id, &renderer.render(am, live));
    let _ = app.emit("permission-request", &ev);
    let reply = tokio::select! {
        r = rx => r.ok(),
        _ = cancel.cancelled() => None,
    };
    st.permissions.lock().remove(&req_id);
    st.pending_perms.lock().remove(&req_id);
    let _ = app.emit("permission-resolved", json!({"request_id": req_id, "conversation_id": conv_id}));
    live.live_tools.insert(tool_use_id.to_string(), "running".into());
    st.live.lock().insert(conv_id.to_string(), live.clone());
    emit_turn(app, conv_id, &renderer.render(am, live));
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

fn update_conv_options(st: &AppState, conv_id: &str, f: impl FnOnce(&mut Value)) {
    if let Ok(c) = st.db.conv_get(conv_id) {
        let mut o = c.options;
        if !o.is_object() {
            o = json!({});
        }
        f(&mut o);
        let _ = st.db.conv_set_options(conv_id, &o);
    }
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
        match codetools::check_permission(&mode, name, input, &rules) {
            Perm::Deny(msg) => return (text_block(msg), true),
            Perm::Ask => {
                let diff = codetools::preview_diff(Some(session), name, input);
                let reply = ask_permission(
                    app, st, conv_id, id,
                    json!({"kind": "code", "tool": name, "input": input, "diff": diff, "cwd": session.cwd.lock().display().to_string()}),
                    live, renderer, am, cancel,
                )
                .await;
                match reply.as_ref().map(|r| r.decision.as_str()) {
                    Some("once") => {}
                    Some("session") | Some("chat") | Some("always") => {
                        let rule = codetools::session_rule(name, input);
                        update_conv_options(st, conv_id, |o| {
                            let mut arr = o["allowed_rules"].as_array().cloned().unwrap_or_default();
                            if !arr.iter().any(|x| x == &json!(rule)) {
                                arr.push(json!(rule));
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
        let out = codetools::execute(session, name, input, cancel, timeout).await;
        return (out.content, out.is_error);
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
        let bypass = conv.kind == "code" && conv.options["permission_mode"] == "bypassPermissions";
        if policy == "deny" {
            return (text_block("This tool is disabled by the user."), true);
        }
        if policy != "allow" && !allowed_chat && !bypass {
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
                    let mut s = st.settings.write();
                    s.tool_policies.insert(key.clone(), json!("allow"));
                    let _ = s.save(&st.db);
                }
                _ => return (denial(&reply), true),
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
}
