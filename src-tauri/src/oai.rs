//! OpenAI-compatible chat endpoints (Ollama, LM Studio, llama.cpp server,
//! vLLM, OpenRouter, …) for chats with local or third-party models.
//!
//! Models from the configured endpoint appear in the model picker as
//! `oai:<model id>`. These chats are text (and image) only: no tools,
//! artifacts still render from the reply text.

use crate::db::Message;
use crate::error::{AppError, AppResult};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const PREFIX: &str = "oai:";

fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::msg(e.to_string()))
}

fn url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path)
}

/// Model ids offered by the endpoint (`GET /models`).
pub async fn list_models(base: &str, key: &str) -> AppResult<Vec<String>> {
    let mut req = client()?.get(url(base, "models")).timeout(Duration::from_secs(15));
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let r = req.send().await.map_err(|e| AppError::msg(format!("Can't reach {base}: {e}")))?;
    let status = r.status();
    let v: Value = r.json().await.map_err(|e| AppError::msg(format!("Unexpected answer from {base}: {e}")))?;
    if !status.is_success() {
        return Err(AppError::msg(format!("{base} answered {status}: {}", v["error"]["message"].as_str().unwrap_or(""))));
    }
    let mut ids: Vec<String> = v["data"]
        .as_array()
        .or(v["models"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().or(m["name"].as_str()).map(String::from))
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Our stored conversation path as OpenAI chat messages.
pub fn to_messages(system: &str, path: &[Message]) -> Vec<Value> {
    let mut out = vec![];
    if !system.trim().is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }
    for m in path {
        let blocks = m.content.as_array().cloned().unwrap_or_default();
        if m.role == "assistant" {
            let text: String = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("");
            if !text.trim().is_empty() {
                out.push(json!({"role": "assistant", "content": text}));
            }
            continue;
        }
        let mut parts: Vec<Value> = vec![];
        for b in &blocks {
            match b["type"].as_str() {
                Some("text") => {
                    let t = b["text"].as_str().unwrap_or("");
                    if !t.is_empty() && !t.starts_with("<system-reminder>") {
                        parts.push(json!({"type": "text", "text": t}));
                    }
                }
                Some("image") if b["source"]["type"] == "base64" => {
                    let mt = b["source"]["media_type"].as_str().unwrap_or("image/png");
                    let data = b["source"]["data"].as_str().unwrap_or("");
                    parts.push(json!({"type": "image_url", "image_url": {"url": format!("data:{mt};base64,{data}")}}));
                }
                Some("document") => {
                    let title = b["title"].as_str().unwrap_or("document");
                    if let Some(t) = b["source"]["data"].as_str().filter(|_| b["source"]["type"] == "text") {
                        parts.push(json!({"type": "text", "text": format!("<document title=\"{title}\">\n{t}\n</document>")}));
                    }
                }
                _ => {}
            }
        }
        if parts.is_empty() {
            continue;
        }
        // plain string when it's only text (widest compatibility)
        if parts.iter().all(|p| p["type"] == "text") {
            let text = parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n\n");
            out.push(json!({"role": "user", "content": text}));
        } else {
            out.push(json!({"role": "user", "content": parts}));
        }
    }
    out
}

pub struct Outcome {
    pub text: String,
    pub finish: Option<String>,
    pub usage: Value,
    pub cancelled: bool,
}

/// Stream a chat completion; `on_text` gets the text so far.
pub async fn stream(base: &str, key: &str, model: &str, messages: Vec<Value>, max_tokens: u32, cancel: &CancellationToken, mut on_text: impl FnMut(&str)) -> AppResult<Outcome> {
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "max_tokens": max_tokens,
        "stream_options": {"include_usage": true},
    });
    let mut req = client()?.post(url(base, "chat/completions")).json(&body);
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let resp = tokio::select! {
        r = req.send() => r.map_err(|e| AppError::msg(format!("Can't reach {base}: {e}")))?,
        _ = cancel.cancelled() => return Ok(Outcome { text: String::new(), finish: None, usage: Value::Null, cancelled: true }),
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let t = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&t).ok().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or(t);
        return Err(AppError::msg(format!("{base} answered {status}: {}", msg.chars().take(500).collect::<String>())));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut text = String::new();
    let mut finish = None;
    let mut usage = Value::Null;
    loop {
        let chunk = tokio::select! {
            c = stream.next() => c,
            _ = cancel.cancelled() => return Ok(Outcome { text, finish, usage, cancelled: true }),
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| AppError::msg(format!("The connection to {base} broke: {e}")))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf.drain(..=nl);
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(Outcome { text, finish, usage, cancelled: false });
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(e) = v["error"]["message"].as_str() {
                return Err(AppError::msg(e.to_string()));
            }
            if let Some(d) = v["choices"][0]["delta"]["content"].as_str() {
                if !d.is_empty() {
                    text.push_str(d);
                    on_text(&text);
                }
            }
            if let Some(f) = v["choices"][0]["finish_reason"].as_str() {
                finish = Some(f.to_string());
            }
            if v["usage"].is_object() {
                usage = json!({
                    "input_tokens": v["usage"]["prompt_tokens"],
                    "output_tokens": v["usage"]["completion_tokens"],
                });
            }
        }
    }
    Ok(Outcome { text, finish, usage, cancelled: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: &str, content: Value) -> Message {
        Message { id: "x".into(), conversation_id: "c".into(), parent_id: None, role: role.into(), content, meta: json!({}), created_at: 0 }
    }

    #[test]
    fn converts_history() {
        let path = vec![
            m("user", json!([{"type":"text","text":"hi"},{"type":"text","text":"<system-reminder>x</system-reminder>"}])),
            m("assistant", json!([{"type":"thinking","thinking":"..."},{"type":"text","text":"hello"}])),
            m("user", json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAA"}},{"type":"text","text":"what is this"}])),
        ];
        let v = to_messages("be nice", &path);
        assert_eq!(v[0]["role"], "system");
        assert_eq!(v[1], json!({"role":"user","content":"hi"}));
        assert_eq!(v[2], json!({"role":"assistant","content":"hello"}));
        assert_eq!(v[3]["content"][0]["image_url"]["url"], "data:image/png;base64,AAA");
    }

    #[tokio::test]
    async fn streams_from_a_local_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.starts_with("GET /v1/models") {
                    r#"{"data":[{"id":"llama3.2"},{"id":"qwen2.5"}]}"#.to_string()
                } else {
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n".to_string()
                };
                let ct = if req.starts_with("GET") { "application/json" } else { "text/event-stream" };
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: {ct}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        let base = format!("http://{addr}/v1");
        std::env::set_var("NO_PROXY", "127.0.0.1");
        assert_eq!(list_models(&base, "").await.unwrap(), vec!["llama3.2", "qwen2.5"]);
        let mut seen = vec![];
        let o = stream(&base, "k", "llama3.2", vec![json!({"role":"user","content":"hi"})], 100, &CancellationToken::new(), |t| seen.push(t.to_string())).await.unwrap();
        assert_eq!(o.text, "Hello");
        assert_eq!(o.finish.as_deref(), Some("stop"));
        assert_eq!(o.usage["output_tokens"], 2);
        assert_eq!(seen, vec!["Hel", "Hello"]);
    }
}
