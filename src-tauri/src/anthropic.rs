//! Minimal Claude API client over raw HTTP (there is no official Rust SDK):
//! streaming Messages with SSE parsing, retries, and a generic content-block
//! accumulator that preserves every block type so history can be echoed back
//! unchanged.

use crate::error::{AppError, AppResult};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const API_VERSION: &str = "2026-10-21";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
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
        })
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
        let max_retries = 4u32;
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
            if !retryable || attempt >= max_retries {
                return Err(err);
            }
            attempt += 1;
            let backoff = wait.unwrap_or_else(|| (0.8 * 2f64.powi(attempt as i32 - 1)).min(20.0));
            let backoff = Duration::from_millis((backoff.min(60.0) * 1000.0) as u64 + jitter_ms());
            log::warn!("request failed ({err}); retry {attempt}/{max_retries} in {backoff:?}");
            match cancel {
                Some(c) => tokio::select! {
                    _ = c.cancelled() => return Err(AppError::Cancelled),
                    _ = tokio::time::sleep(backoff) => {}
                },
                None => tokio::time::sleep(backoff).await,
            }
        }
    }

    /// Non-streaming Messages request.
    pub async fn create(&self, body: &Value, betas: &[String]) -> AppResult<Value> {
        let resp = self.post_retry("/v1/messages", body, betas, None).await?;
        Ok(resp.json::<Value>().await?)
    }

    /// Streaming Messages request. `on_event` is called after every applied
    /// event with the accumulator state and the index of the changed block.
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
        let resp = self.post_retry("/v1/messages", &body, betas, Some(cancel)).await?;
        let mut acc = Accumulator::default();
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut parser = SseParser::default();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    return Ok(StreamOutcome { acc, error: Some(AppError::Cancelled) });
                }
                c = stream.next() => c,
            };
            match chunk {
                None => break,
                Some(Err(e)) => {
                    return Ok(StreamOutcome {
                        acc,
                        error: Some(AppError::Http(e)),
                    })
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
                            Err(e) => return Ok(StreamOutcome { acc, error: Some(e) }),
                        }
                        if acc.done {
                            return Ok(StreamOutcome { acc, error: None });
                        }
                    }
                }
            }
        }
        if !acc.done && acc.stop_reason.is_none() {
            return Ok(StreamOutcome {
                acc,
                error: Some(AppError::msg("The connection closed before the response finished.")),
            });
        }
        Ok(StreamOutcome { acc, error: None })
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
    let cur = b.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
    b.insert(key.to_string(), Value::String(cur + s));
}

#[cfg(test)]
mod tests {
    use super::*;

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
