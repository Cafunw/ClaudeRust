//! Minimal Claude API client over raw HTTP (there is no official Rust SDK):
//! streaming Messages with SSE parsing, retries, and a generic content-block
//! accumulator that preserves every block type so history can be echoed back
//! unchanged.

use crate::error::{AppError, AppResult};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const API_VERSION: &str = "2026-10-21";

/// Retries of a request that failed for a reason that may pass (rate limit,
/// overload, server error, network).
pub const MAX_RETRIES: u32 = 4;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    quiet: Quiet,
    on_retry: Option<RetryHook>,
}

type RetryHook = Arc<dyn Fn(&Retry) + Send + Sync>;

/// A retry about to be waited for.
pub struct Retry {
    /// which retry this is, 1 to `max_attempts`
    pub attempt: u32,
    pub max_attempts: u32,
    pub wait_ms: u64,
    /// `rate_limited`, `overloaded`, `server_error` or `network`
    pub reason: &'static str,
}

fn retry_reason(e: &AppError) -> &'static str {
    match (e.status(), e.kind()) {
        (529, _) | (_, "overloaded_error") => "overloaded",
        (429, _) => "rate_limited",
        (0, _) => "network",
        _ => "server_error",
    }
}

/// How long a stream may send nothing before it counts as stalled: the
/// connection died without a word (sleep and resume, a network switch).
/// While text streams, deltas come many times a second, so 90 s of nothing
/// there is never the model. Before the first block, in thinking, tool input
/// and server tools the model may go quiet for long, and the API documents
/// its keep-alive pings as "any number" of them, not a rate: 5 minutes there.
#[derive(Clone, Copy)]
struct Quiet {
    text: Duration,
    other: Duration,
}

const QUIET: Quiet = Quiet { text: Duration::from_secs(90), other: Duration::from_secs(300) };

impl Quiet {
    fn allowed(&self, acc: &Accumulator) -> Duration {
        if acc.blocks.last().is_some_and(|b| b["type"] == "text") {
            self.text
        } else {
            self.other
        }
    }
}

/// While a stream sends nothing, `on_event` is called this often anyway.
const TICK: Duration = Duration::from_secs(1);

/// A stream that sent nothing for `quiet`: a network error (the UI words it
/// like a broken connection), and the only `Api` error with status 0.
fn stalled(quiet: Duration) -> AppError {
    AppError::Api {
        status: 0,
        kind: "network".into(),
        message: format!("The connection stalled: nothing arrived for {} s.", quiet.as_secs()),
    }
}

pub struct StreamOutcome {
    pub acc: Accumulator,
    /// Error that interrupted an already-started stream (partial content kept).
    pub error: Option<AppError>,
}

/// An HTTP client shared by every request, so the system's root
/// certificates aren't loaded again each time (slow on macOS and Windows)
/// and connections are reused instead of a new TLS handshake per request.
/// The proxy settings (the system's too) and the certificates are read when
/// a client is built, so it is built again once it is `SHARED_FOR` old: a
/// change to them still takes effect while the app runs.
pub(crate) struct SharedHttp(parking_lot::Mutex<Option<(std::time::Instant, reqwest::Client)>>);

/// How long a shared client is used (connections idle for 90 s are closed
/// anyway).
const SHARED_FOR: Duration = Duration::from_secs(60);

impl SharedHttp {
    pub(crate) const fn new() -> SharedHttp {
        SharedHttp(parking_lot::const_mutex(None))
    }

    /// The client, built with `build` when there is none or it is too old.
    pub(crate) fn get(&self, build: impl FnOnce() -> reqwest::Result<reqwest::Client>) -> reqwest::Result<reqwest::Client> {
        self.get_for(SHARED_FOR, build)
    }

    fn get_for(&self, keep: Duration, build: impl FnOnce() -> reqwest::Result<reqwest::Client>) -> reqwest::Result<reqwest::Client> {
        let mut slot = self.0.lock();
        match slot.as_ref() {
            Some((at, c)) if at.elapsed() < keep => Ok(c.clone()),
            _ => {
                let c = build()?;
                *slot = Some((std::time::Instant::now(), c.clone()));
                Ok(c)
            }
        }
    }
}

/// The client for the Claude API. A `Client` is made for every step of a
/// turn, title and model list; the key and endpoint are its own, nothing
/// per key is kept in the shared one.
fn shared_http() -> AppResult<reqwest::Client> {
    static HTTP: SharedHttp = SharedHttp::new();
    HTTP.get(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(600))
            .user_agent(concat!("ClaudeRust/", env!("CARGO_PKG_VERSION")))
            .build()
    })
    .map_err(AppError::from)
}

impl Client {
    pub fn new(base_url: &str, api_key: &str) -> AppResult<Client> {
        let http = shared_http()?;
        Ok(Client {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            quiet: QUIET,
            on_retry: None,
        })
    }

    /// `f` is told of every retry before it is waited for.
    pub fn on_retry(mut self, f: impl Fn(&Retry) + Send + Sync + 'static) -> Client {
        self.on_retry = Some(Arc::new(f));
        self
    }

    /// Wait before retry `attempt` after `err` (`wait`: the seconds the
    /// server asked for), told to `on_retry` first.
    async fn backoff(&self, attempt: u32, wait: Option<f64>, err: &AppError, cancel: Option<&CancellationToken>) -> AppResult<()> {
        let secs = wait.unwrap_or_else(|| (0.8 * 2f64.powi(attempt as i32 - 1)).min(20.0));
        let backoff = Duration::from_millis((secs.min(60.0) * 1000.0) as u64 + jitter_ms());
        log::warn!("request failed ({err}); retry {attempt}/{MAX_RETRIES} in {backoff:?}");
        if let Some(f) = &self.on_retry {
            f(&Retry { attempt, max_attempts: MAX_RETRIES, wait_ms: backoff.as_millis() as u64, reason: retry_reason(err) });
        }
        match cancel {
            Some(c) => tokio::select! {
                _ = c.cancelled() => Err(AppError::Cancelled),
                _ = tokio::time::sleep(backoff) => Ok(()),
            },
            None => {
                tokio::time::sleep(backoff).await;
                Ok(())
            }
        }
    }

    fn headers(&self, betas: &[String]) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Ok(v) = HeaderValue::from_str(&self.api_key) {
            h.insert("x-api-key", v);
        }
        h.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
        if !betas.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&betas.join(",")) {
                h.insert("anthropic-beta", v);
            }
        }
        h
    }

    /// POST with retries on transient failures. Returns the successful response.
    async fn post_retry(
        &self,
        path: &str,
        body: &Value,
        betas: &[String],
        cancel: Option<&CancellationToken>,
    ) -> AppResult<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let mut attempt = 0u32;
        loop {
            let req = self
                .http
                .post(&url)
                .headers(self.headers(betas))
                .body(body.to_string());
            let res = match cancel {
                Some(c) => tokio::select! {
                    _ = c.cancelled() => return Err(AppError::Cancelled),
                    r = req.send() => r,
                },
                None => req.send().await,
            };
            let (retryable, wait, err) = match res {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if resp.status().is_success() {
                        return Ok(resp);
                    }
                    let should_retry_hdr = resp
                        .headers()
                        .get("x-should-retry")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.trim().parse::<f64>().ok());
                    let text = resp.text().await.unwrap_or_default();
                    let err = api_error(status, &text);
                    let mut retryable = matches!(status, 408 | 409 | 429 | 500 | 502 | 503 | 504 | 529);
                    match should_retry_hdr.as_deref() {
                        Some("true") => retryable = true,
                        Some("false") => retryable = false,
                        _ => {}
                    }
                    (retryable, retry_after, err)
                }
                Err(e) => {
                    let retryable = e.is_connect() || e.is_timeout() || e.is_request();
                    (retryable, None, AppError::Http(e))
                }
            };
            if !retryable || attempt >= MAX_RETRIES {
                return Err(err);
            }
            attempt += 1;
            self.backoff(attempt, wait, &err, cancel).await?;
        }
    }

    /// Non-streaming Messages request.
    pub async fn create(&self, body: &Value, betas: &[String]) -> AppResult<Value> {
        let resp = self.post_retry("/v1/messages", body, betas, None).await?;
        Ok(resp.json::<Value>().await?)
    }

    /// Streaming Messages request. `on_event` is called after every applied
    /// event with the accumulator state and the index of the changed block,
    /// and every `TICK` while nothing arrives (with `None`). A stream that
    /// stalls (see `Quiet`) is asked for again while nothing of the reply
    /// has come; after that it ends with a network error.
    pub async fn stream<F>(
        &self,
        body: &Value,
        betas: &[String],
        cancel: &CancellationToken,
        mut on_event: F,
    ) -> AppResult<StreamOutcome>
    where
        F: FnMut(&Accumulator, Option<usize>),
    {
        let mut body = body.clone();
        body["stream"] = Value::Bool(true);
        let mut stalls = 0u32;
        loop {
            let resp = self.post_retry("/v1/messages", &body, betas, Some(cancel)).await?;
            let out = self.read_stream(resp, cancel, &mut on_event).await;
            // stalled before any of the reply came: nothing is lost by asking again
            match &out.error {
                Some(e @ AppError::Api { status: 0, .. }) if stalls < MAX_RETRIES && out.acc.content().is_empty() => {
                    stalls += 1;
                    self.backoff(stalls, None, e, Some(cancel)).await?;
                }
                _ => return Ok(out),
            }
        }
    }

    /// The events of one response (see `stream`).
    async fn read_stream<F>(&self, resp: reqwest::Response, cancel: &CancellationToken, on_event: &mut F) -> StreamOutcome
    where
        F: FnMut(&Accumulator, Option<usize>),
    {
        let mut acc = Accumulator::default();
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut parser = SseParser::default();
        let mut heard = Instant::now();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    return StreamOutcome { acc, error: Some(AppError::Cancelled) };
                }
                c = tokio::time::timeout(TICK, stream.next()) => c,
            };
            let Ok(chunk) = chunk else {
                let quiet = self.quiet.allowed(&acc);
                if heard.elapsed() < quiet {
                    on_event(&acc, None);
                    continue;
                }
                return StreamOutcome { acc, error: Some(stalled(quiet)) };
            };
            heard = Instant::now();
            match chunk {
                None => break,
                Some(Err(e)) => {
                    return StreamOutcome {
                        acc,
                        error: Some(AppError::Http(e)),
                    }
                }
                Some(Ok(bytes)) => {
                    buf.extend_from_slice(&bytes);
                    for ev in parser.feed(&mut buf) {
                        if ev.data.is_empty() {
                            continue;
                        }
                        let data: Value = match serde_json::from_str(&ev.data) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        match acc.apply(&data) {
                            Ok(changed) => on_event(&acc, changed),
                            Err(e) => return StreamOutcome { acc, error: Some(e) },
                        }
                        if acc.done {
                            return StreamOutcome { acc, error: None };
                        }
                    }
                }
            }
        }
        if !acc.done && acc.stop_reason.is_none() {
            return StreamOutcome {
                acc,
                error: Some(AppError::msg("The connection closed before the response finished.")),
            };
        }
        StreamOutcome { acc, error: None }
    }

    /// GET `path` (no retries: the Files API calls it serves are free to ask again).
    async fn get(&self, path: &str, cancel: &CancellationToken) -> AppResult<reqwest::Response> {
        let req = self.http.get(format!("{}{path}", self.base_url)).headers(self.headers(&[]));
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            r = req.send() => r?,
        };
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(api_error(status, &resp.text().await.unwrap_or_default()));
        }
        Ok(resp)
    }

    pub async fn get_json(&self, path: &str, cancel: &CancellationToken) -> AppResult<Value> {
        Ok(self.get(path, cancel).await?.json().await?)
    }

    /// The bytes at `path`, or None as soon as there are more than `max` of them.
    pub async fn download(&self, path: &str, max: u64, cancel: &CancellationToken) -> AppResult<Option<Vec<u8>>> {
        let mut stream = self.get(path, cancel).await?.bytes_stream();
        let mut out = vec![];
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(AppError::Cancelled),
                c = stream.next() => c,
            };
            let Some(chunk) = chunk else { return Ok(Some(out)) };
            let chunk = chunk?;
            if (out.len() + chunk.len()) as u64 > max {
                return Ok(None);
            }
            out.extend_from_slice(&chunk);
        }
    }

    /// GET /v1/models (all pages).
    pub async fn list_models(&self) -> AppResult<Vec<Value>> {
        let mut out = vec![];
        let mut after: Option<String> = None;
        for _ in 0..10 {
            let mut url = format!("{}/v1/models?limit=100", self.base_url);
            if let Some(a) = &after {
                url.push_str(&format!("&after_id={a}"));
            }
            let resp = self.http.get(&url).headers(self.headers(&[])).send().await?;
            let status = resp.status().as_u16();
            let text = resp.text().await?;
            if !(200..300).contains(&status) {
                return Err(api_error(status, &text));
            }
            let v: Value = serde_json::from_str(&text)?;
            if let Some(arr) = v["data"].as_array() {
                out.extend(arr.iter().cloned());
            }
            if v["has_more"].as_bool() == Some(true) {
                after = v["last_id"].as_str().map(|s| s.to_string());
                if after.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(out)
    }
}

fn jitter_ms() -> u64 {
    (crate::util::now_ms() % 250) as u64
}

pub fn api_error(status: u16, body: &str) -> AppError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let kind = v["error"]["type"].as_str().unwrap_or("api_error").to_string();
    let mut message = v["error"]["message"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let t = body.trim();
            if t.is_empty() {
                format!("HTTP {status}")
            } else {
                crate::util::truncate_chars(t, 400)
            }
        });
    if status == 401 && !message.to_lowercase().contains("key") {
        message = format!("Invalid API key ({message})");
    }
    AppError::Api {
        status,
        kind,
        message,
    }
}

// ------------------------------------------------------------------ SSE

/// Server-sent events, from bytes as they arrive (LF or CRLF line endings).
#[derive(Default)]
pub(crate) struct SseParser {
    event: String,
    data: Vec<String>,
}

pub(crate) struct SseEvent {
    #[allow(dead_code)]
    pub(crate) event: String,
    pub(crate) data: String,
}

impl SseParser {
    /// The end of the stream: what is left in `buf` (a last line without a
    /// line break, an event without the blank line that ends it).
    pub(crate) fn finish(&mut self, buf: &mut Vec<u8>) -> Vec<SseEvent> {
        buf.extend_from_slice(b"\n\n");
        self.feed(buf)
    }

    /// Consume complete lines from `buf`, returning finished events.
    pub(crate) fn feed(&mut self, buf: &mut Vec<u8>) -> Vec<SseEvent> {
        let mut out = vec![];
        let mut start = 0usize;
        let mut i = 0usize;
        while i < buf.len() {
            if buf[i] == b'\n' {
                let mut line = &buf[start..i];
                if line.last() == Some(&b'\r') {
                    line = &line[..line.len() - 1];
                }
                let line = String::from_utf8_lossy(line).to_string();
                if line.is_empty() {
                    if !self.data.is_empty() || !self.event.is_empty() {
                        out.push(SseEvent {
                            event: std::mem::take(&mut self.event),
                            data: std::mem::take(&mut self.data).join("\n"),
                        });
                    }
                } else if let Some(rest) = line.strip_prefix("data:") {
                    self.data.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                } else if let Some(rest) = line.strip_prefix("event:") {
                    self.event = rest.trim().to_string();
                }
                start = i + 1;
            }
            i += 1;
        }
        buf.drain(..start);
        out
    }
}

// ------------------------------------------------------------ accumulator

#[derive(Default, Clone)]
pub struct Accumulator {
    pub message: Map<String, Value>,
    pub blocks: Vec<Value>,
    partial: HashMap<usize, String>,
    /// tool_use id -> raw input that failed to parse as JSON
    pub invalid_json: HashMap<String, String>,
    pub stop_reason: Option<String>,
    pub stop_details: Value,
    pub usage: Map<String, Value>,
    pub done: bool,
}

impl Accumulator {
    /// Raw (possibly incomplete) JSON input of a tool block still streaming.
    pub fn partial_input(&self, idx: usize) -> Option<&str> {
        self.partial.get(&idx).map(|s| s.as_str())
    }

    pub fn model(&self) -> Option<String> {
        self.message.get("model").and_then(|v| v.as_str()).map(|s| s.to_string())
    }

    /// The code execution container the response ran in (`{id, expires_at, …}`).
    pub fn container(&self) -> Option<&Value> {
        self.message.get("container").filter(|c| c["id"].is_string())
    }

    /// Apply one SSE data payload. Returns the index of the block that changed.
    pub fn apply(&mut self, ev: &Value) -> AppResult<Option<usize>> {
        let t = ev["type"].as_str().unwrap_or("");
        match t {
            "message_start" => {
                if let Some(m) = ev["message"].as_object() {
                    self.message = m.clone();
                    if let Some(c) = m.get("content").and_then(|c| c.as_array()) {
                        self.blocks = c.clone();
                    }
                    if let Some(u) = m.get("usage").and_then(|u| u.as_object()) {
                        self.usage = u.clone();
                    }
                    self.message.remove("content");
                }
                Ok(None)
            }
            "content_block_start" => {
                let idx = match ev["index"].as_u64() {
                    None => self.blocks.len(),
                    // blocks arrive in order; a far-off index (a broken proxy)
                    // must not make us allocate up to it
                    Some(i) if i <= (self.blocks.len() + MAX_BLOCK_GAP) as u64 => i as usize,
                    Some(i) => return Err(malformed(format!("content block {i} started after only {} blocks", self.blocks.len()))),
                };
                let block = ev["content_block"].clone();
                if !block.is_object() {
                    return Err(malformed(format!("content block {idx} is not an object")));
                }
                while self.blocks.len() <= idx {
                    self.blocks.push(Value::Null);
                }
                let bt = block["type"].as_str().unwrap_or("");
                if bt.ends_with("tool_use") {
                    self.partial.insert(idx, String::new());
                }
                self.blocks[idx] = block;
                Ok(Some(idx))
            }
            "content_block_delta" => {
                let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                if idx >= self.blocks.len() {
                    return Ok(None);
                }
                let d = &ev["delta"];
                // a delta only applies to a block that started (an object)
                let Some(b) = self.blocks[idx].as_object_mut() else {
                    return Err(malformed(format!("delta for content block {idx}, which never started")));
                };
                match d["type"].as_str().unwrap_or("") {
                    "text_delta" => append_str(b, "text", d["text"].as_str().unwrap_or("")),
                    "thinking_delta" => append_str(b, "thinking", d["thinking"].as_str().unwrap_or("")),
                    "signature_delta" => {
                        b.insert("signature".into(), d["signature"].clone());
                    }
                    "input_json_delta" => {
                        self.partial
                            .entry(idx)
                            .or_default()
                            .push_str(d["partial_json"].as_str().unwrap_or(""));
                    }
                    "citations_delta" => {
                        let list = b.entry("citations").or_insert_with(|| json!([]));
                        if !list.is_array() {
                            *list = json!([]);
                        }
                        if let Some(arr) = list.as_array_mut() {
                            arr.push(d["citation"].clone());
                        }
                    }
                    _ => {
                        // Unknown delta: merge any string fields conservatively.
                        if let Some(text) = d["text"].as_str() {
                            append_str(b, "text", text);
                        }
                    }
                }
                Ok(Some(idx))
            }
            "content_block_stop" => {
                let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                if let Some(raw) = self.partial.remove(&idx) {
                    if let Some(b) = self.blocks.get_mut(idx).and_then(|b| b.as_object_mut()) {
                        if raw.trim().is_empty() {
                            if !b.get("input").is_some_and(|i| i.is_object()) {
                                b.insert("input".into(), json!({}));
                            }
                        } else {
                            match serde_json::from_str::<Value>(&raw) {
                                Ok(v) if v.is_object() => {
                                    b.insert("input".into(), v);
                                }
                                _ => {
                                    if let Some(id) = b.get("id").and_then(|i| i.as_str()) {
                                        self.invalid_json.insert(id.to_string(), raw);
                                    }
                                    b.insert("input".into(), json!({}));
                                }
                            }
                        }
                    }
                }
                Ok(Some(idx))
            }
            "message_delta" => {
                if let Some(sr) = ev["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(sr.to_string());
                }
                if !ev["delta"]["stop_details"].is_null() {
                    self.stop_details = ev["delta"]["stop_details"].clone();
                }
                if ev["delta"]["container"].is_object() {
                    self.message.insert("container".into(), ev["delta"]["container"].clone());
                }
                if let Some(u) = ev["usage"].as_object() {
                    for (k, v) in u {
                        if !v.is_null() {
                            self.usage.insert(k.clone(), v.clone());
                        }
                    }
                }
                Ok(None)
            }
            "message_stop" => {
                self.done = true;
                Ok(None)
            }
            "error" => {
                let kind = ev["error"]["type"].as_str().unwrap_or("api_error").to_string();
                let message = ev["error"]["message"]
                    .as_str()
                    .unwrap_or("The API returned an error")
                    .to_string();
                let status = match kind.as_str() {
                    "overloaded_error" => 529,
                    "rate_limit_error" => 429,
                    _ => 500,
                };
                Err(AppError::Api {
                    status,
                    kind,
                    message,
                })
            }
            _ => Ok(None),
        }
    }

    /// Finished content blocks (incomplete tool inputs are finalized).
    pub fn content(&self) -> Vec<Value> {
        let mut blocks = self.blocks.clone();
        for (idx, raw) in &self.partial {
            if let Some(b) = blocks.get_mut(*idx).and_then(|b| b.as_object_mut()) {
                if let Ok(v) = serde_json::from_str::<Value>(raw) {
                    if v.is_object() {
                        b.insert("input".into(), v);
                        continue;
                    }
                }
                if !b.get("input").is_some_and(|i| i.is_object()) {
                    b.insert("input".into(), json!({}));
                }
            }
        }
        // only real blocks: never the gaps, nor anything else a server sent
        blocks.into_iter().filter(|b| b.is_object()).collect()
    }

    pub fn usage_value(&self) -> Value {
        Value::Object(self.usage.clone())
    }
}

/// How far ahead of the blocks seen so far a new block's index may be.
const MAX_BLOCK_GAP: usize = 64;

fn malformed(what: String) -> AppError {
    AppError::msg(format!("The API sent a malformed stream ({what})."))
}

fn append_str(b: &mut Map<String, Value>, key: &str, s: &str) {
    if s.is_empty() {
        return;
    }
    // in place: copying the text so far on every delta made a long block
    // cost quadratic time while it streamed
    match b.get_mut(key) {
        Some(Value::String(cur)) => cur.push_str(s),
        // missing, or not a string (an odd block from a server): start anew
        _ => {
            b.insert(key.to_string(), Value::String(s.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_str_grows_in_place_and_replaces_odd_values() {
        let mut b = Map::new();
        append_str(&mut b, "text", "");
        assert!(b.get("text").is_none(), "nothing added for an empty delta");
        for piece in ["Hel", "lo, ", "", "world"] {
            append_str(&mut b, "text", piece);
        }
        assert_eq!(b["text"], "Hello, world");
        b.insert("thinking".into(), json!(42));
        append_str(&mut b, "thinking", "ok");
        assert_eq!(b["thinking"], "ok");
    }

    #[test]
    fn sse_parse_and_accumulate() {
        let raw = concat!(
            "event: message_start\r\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-opus-5-5\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":10}}}\r\n\r\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"x\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"a\\\":\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"1}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let mut p = SseParser::default();
        let mut acc = Accumulator::default();
        // feed in odd-sized chunks
        let bytes = raw.as_bytes();
        let mut buf = vec![];
        for chunk in bytes.chunks(7) {
            buf.extend_from_slice(chunk);
            for ev in p.feed(&mut buf) {
                let v: Value = serde_json::from_str(&ev.data).unwrap();
                acc.apply(&v).unwrap();
            }
        }
        assert!(acc.done);
        let c = acc.content();
        assert_eq!(c[0]["thinking"], "hmm");
        assert_eq!(c[0]["signature"], "sig");
        assert_eq!(c[1]["text"], "Hello");
        assert_eq!(c[2]["input"]["a"], 1);
        assert_eq!(acc.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(acc.usage["output_tokens"], 7);
        assert_eq!(acc.usage["input_tokens"], 10);
        assert_eq!(acc.model().as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn hostile_block_indexes_and_shapes_are_stream_errors() {
        // an index far beyond the blocks seen must not allocate up to it
        let mut acc = Accumulator::default();
        let e = acc.apply(&json!({"type": "content_block_start", "index": 4294967295u64, "content_block": {"type": "text", "text": ""}}));
        assert!(e.is_err(), "{:?}", e.ok());
        assert!(acc.blocks.is_empty());
        // a small gap is fine (the missing block is simply left out)
        acc.apply(&json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": "a"}})).unwrap();
        assert_eq!(acc.content(), vec![json!({"type": "text", "text": "a"})]);
        // a delta for a block that never started is an error, not a panic
        assert!(acc.apply(&json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "x"}})).is_err());

        // blocks that aren't objects
        let mut acc = Accumulator::default();
        assert!(acc.apply(&json!({"type": "content_block_start", "index": 0, "content_block": "text"})).is_err());
        assert!(acc.apply(&json!({"type": "content_block_start", "index": 0, "content_block": [1, 2]})).is_err());
        let mut acc = Accumulator::default();
        acc.apply(&json!({"type": "message_start", "message": {"content": ["x", 5, {"type": "text", "text": "ok"}]}})).unwrap();
        for (i, delta) in [
            json!({"type": "text_delta", "text": "y"}),
            json!({"type": "signature_delta", "signature": "s"}),
            json!({"type": "citations_delta", "citation": {}}),
            json!({"type": "something_new", "text": "z"}),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(acc.apply(&json!({"type": "content_block_delta", "index": i % 2, "delta": delta})).is_err());
        }
        assert_eq!(acc.content(), vec![json!({"type": "text", "text": "ok"})]);
        // a tool block replaced by a non-object before it stops
        let mut acc = Accumulator::default();
        acc.apply(&json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t", "name": "x", "input": {}}})).unwrap();
        acc.apply(&json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"a\":1"}})).unwrap();
        acc.apply(&json!({"type": "message_start", "message": {"content": ["x"]}})).unwrap();
        assert!(acc.content().is_empty());
        acc.apply(&json!({"type": "content_block_stop", "index": 0})).unwrap();
        assert!(acc.content().is_empty());
    }

    #[test]
    fn sse_finish_flushes_the_last_event() {
        let mut p = SseParser::default();
        let mut buf = b"data: {\"a\":1}\r\n\r\ndata: {\"b\":2}".to_vec();
        let first = p.feed(&mut buf);
        assert_eq!(first.len(), 1);
        let rest = p.finish(&mut buf);
        assert_eq!(rest.iter().map(|e| e.data.as_str()).collect::<Vec<_>>(), vec!["{\"b\":2}"]);
        let mut buf = b"data: x\n".to_vec();
        let mut p = SseParser::default();
        assert!(p.feed(&mut buf).is_empty());
        assert_eq!(p.finish(&mut buf).len(), 1);
    }

    #[test]
    fn error_parsing() {
        let e = api_error(400, r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#);
        assert_eq!(e.kind(), "invalid_request_error");
        assert_eq!(e.to_string(), "bad");
    }

    /// A local HTTP/1.1 server answering every request on a connection
    /// (keep-alive) with an empty model list; counts the connections.
    async fn keep_alive_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        std::env::set_var("NO_PROXY", "127.0.0.1");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let conns = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c2 = conns.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let Ok(n) = s.read(&mut chunk).await else { return };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        while let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            buf.drain(..end + 4);
                            let body = r#"{"data":[],"has_more":false}"#;
                            let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len());
                            if s.write_all(resp.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                    }
                });
            }
        });
        (format!("http://{addr}"), conns)
    }

    #[test]
    fn the_container_comes_with_the_message_delta() {
        let mut acc = Accumulator::default();
        acc.apply(&json!({"type": "message_start", "message": {"id": "m", "content": [], "container": null}})).unwrap();
        assert!(acc.container().is_none());
        acc.apply(&json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "container": {"id": "container_1", "expires_at": "2026-11-22T10:00:00Z"}}})).unwrap();
        assert_eq!(acc.container().unwrap()["id"], "container_1");
    }

    #[tokio::test]
    async fn a_download_stops_at_its_cap() {
        let (base, _) = keep_alive_server().await;
        let c = Client::new(&base, "k").unwrap();
        let none = CancellationToken::new();
        let body = br#"{"data":[],"has_more":false}"#;
        assert_eq!(c.download("/v1/files/file_a/content", body.len() as u64, &none).await.unwrap().as_deref(), Some(&body[..]));
        assert_eq!(c.download("/v1/files/file_a/content", body.len() as u64 - 1, &none).await.unwrap(), None);
        assert_eq!(c.get_json("/v1/files/file_a", &none).await.unwrap()["has_more"], false);
    }

    #[tokio::test]
    async fn clients_share_connections() {
        let (base, conns) = keep_alive_server().await;
        for _ in 0..3 {
            // a new `Client` each time, as the app makes one per step of a turn
            let c = Client::new(&base, "k").unwrap();
            assert!(c.list_models().await.unwrap().is_empty());
        }
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1, "one connection, reused");
    }

    /// A local Messages API that answers its `i`th request with the SSE
    /// events of `replies[i]`; a reply without `message_stop` then stays
    /// open and silent. Counts the requests.
    async fn sse_server(replies: Vec<Vec<Value>>) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::Ordering::SeqCst;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        std::env::set_var("NO_PROXY", "127.0.0.1");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n2 = n.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let events = replies.get(n2.fetch_add(1, SeqCst)).cloned().unwrap_or_default();
                tokio::spawn(async move {
                    // all of the request, so closing never resets the connection
                    let mut buf = vec![];
                    let mut chunk = [0u8; 8192];
                    loop {
                        let Ok(k) = s.read(&mut chunk).await else { return };
                        buf.extend_from_slice(&chunk[..k]);
                        let head = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
                        let len = head.map(|h| String::from_utf8_lossy(&buf[..h]).to_ascii_lowercase());
                        let len = len.and_then(|h| h.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))));
                        if k == 0 || head.is_some_and(|h| buf.len() >= h + len.unwrap_or(0)) {
                            break;
                        }
                    }
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n").await;
                    for ev in &events {
                        let data = format!("event: {}\ndata: {ev}\n\n", ev["type"].as_str().unwrap_or(""));
                        let _ = s.write_all(format!("{:x}\r\n{data}\r\n", data.len()).as_bytes()).await;
                    }
                    if events.last().is_some_and(|e| e["type"] == "message_stop") {
                        let _ = s.write_all(b"0\r\n\r\n").await;
                    } else {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                });
            }
        });
        (format!("http://{addr}"), n)
    }

    fn start() -> Value {
        json!({"type": "message_start", "message": {"id": "m", "model": "x", "role": "assistant", "content": [], "usage": {"input_tokens": 1}}})
    }

    fn text_start_and(piece: &str) -> Vec<Value> {
        vec![
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": piece}}),
        ]
    }

    async fn stream_quietly(c: &Client) -> (StreamOutcome, usize) {
        let mut ticks = 0;
        let out = tokio::time::timeout(Duration::from_secs(20), c.stream(&json!({}), &[], &CancellationToken::new(), |acc: &Accumulator, changed| {
            ticks += usize::from(changed.is_none() && !acc.blocks.is_empty());
        }))
        .await
        .expect("the stalled stream hung")
        .unwrap();
        (out, ticks)
    }

    #[tokio::test]
    async fn a_stream_that_goes_quiet_before_the_reply_is_asked_for_again() {
        let mut whole = vec![start()];
        whole.extend(text_start_and("Hello"));
        whole.extend([json!({"type": "content_block_stop", "index": 0}), json!({"type": "message_stop"})]);
        let (base, requests) = sse_server(vec![vec![start()], whole]).await;
        let retries = std::sync::Arc::new(parking_lot::Mutex::new(vec![]));
        let seen = retries.clone();
        let mut c = Client::new(&base, "k").unwrap().on_retry(move |r| seen.lock().push((r.attempt, r.max_attempts, r.reason)));
        // before any block the long window holds: the short one must not apply
        c.quiet = Quiet { text: Duration::from_secs(60), other: Duration::from_millis(300) };
        let (out, _) = stream_quietly(&c).await;
        assert!(out.error.is_none(), "{:?}", out.error.map(|e| e.to_string()));
        assert_eq!(out.acc.content(), vec![json!({"type": "text", "text": "Hello"})]);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(*retries.lock(), vec![(1, MAX_RETRIES, "network")]);
    }

    #[tokio::test]
    async fn a_stream_that_goes_quiet_in_the_reply_ends_with_what_came() {
        let mut v = vec![start()];
        v.extend(text_start_and("Hel"));
        let (base, requests) = sse_server(vec![v]).await;
        let mut c = Client::new(&base, "k").unwrap();
        // in text the short window holds: the long one must not apply
        c.quiet = Quiet { text: Duration::from_millis(1500), other: Duration::from_secs(60) };
        let (out, ticks) = stream_quietly(&c).await;
        let e = out.error.expect("a stalled stream is an error");
        assert_eq!((e.kind(), e.status()), ("network", 0), "{e}");
        assert!(e.to_string().contains("stalled"), "{e}");
        assert_eq!(out.acc.content(), vec![json!({"type": "text", "text": "Hel"})]);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1, "what came would be lost by asking again");
        assert!(ticks >= 1, "on_event was not called while the stream was quiet");
    }

    #[test]
    fn retries_say_why() {
        let api = |status, kind: &str| AppError::Api { status, kind: kind.into(), message: String::new() };
        assert_eq!(retry_reason(&api(429, "rate_limit_error")), "rate_limited");
        assert_eq!(retry_reason(&api(529, "overloaded_error")), "overloaded");
        assert_eq!(retry_reason(&api(500, "overloaded_error")), "overloaded");
        assert_eq!(retry_reason(&api(503, "api_error")), "server_error");
        assert_eq!(retry_reason(&stalled(Duration::from_secs(90))), "network");
    }

    #[test]
    fn a_shared_client_is_built_again_once_old() {
        let shared = SharedHttp::new();
        let builds = std::cell::Cell::new(0);
        let build = || {
            builds.set(builds.get() + 1);
            reqwest::Client::builder().build()
        };
        for _ in 0..3 {
            shared.get_for(Duration::from_secs(60), build).unwrap();
        }
        assert_eq!(builds.get(), 1);
        // too old (proxy settings or certificates may have changed): a new one
        shared.get_for(Duration::ZERO, build).unwrap();
        assert_eq!(builds.get(), 2);
        shared.get_for(Duration::from_secs(60), build).unwrap();
        assert_eq!(builds.get(), 2, "and that one is kept");
    }

    /// `cargo test --release -p clauderust --lib bench_client -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn bench_client() {
        use std::time::Instant;
        let n = 30u32;
        let t = Instant::now();
        for _ in 0..n {
            let _ = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .read_timeout(Duration::from_secs(600))
                .user_agent("x")
                .build()
                .unwrap();
        }
        println!("reqwest::Client::builder().build(): {:?} each", t.elapsed() / n);
        let _ = Client::new("http://127.0.0.1:1", "k").unwrap();
        let t = Instant::now();
        for _ in 0..n {
            let _ = Client::new("http://127.0.0.1:1", "k").unwrap();
        }
        println!("Client::new: {:?} each", t.elapsed() / n);
        let (base, conns) = keep_alive_server().await;
        let n = 200u32;
        let t = Instant::now();
        for _ in 0..n {
            Client::new(&base, "k").unwrap().list_models().await.unwrap();
        }
        println!("Client::new + GET /v1/models (local): {:?} each, {} connections for {n} requests", t.elapsed() / n, conns.load(std::sync::atomic::Ordering::SeqCst));
    }
}
