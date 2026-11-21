//! Model Context Protocol client. Reads the same `mcpServers` format as
//! Claude Desktop's `claude_desktop_config.json` and connects to local
//! (stdio) and remote (streamable HTTP) servers.

use crate::error::{AppError, AppResult};
use crate::util::{sha256_hex, truncate_middle};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone, Serialize)]
pub struct McpTool {
    pub server: String,
    pub name: String,
    pub api_name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    pub name: String,
    pub transport: String,
    pub status: String,
    pub error: Option<String>,
    pub tools: Vec<McpTool>,
    pub server_info: Value,
    pub logs: Vec<String>,
    pub disabled: bool,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, Value>>>>>;

enum Transport {
    Stdio {
        /// Lines to write to the server's stdin (a writer task owns the pipe).
        tx: tokio::sync::mpsc::UnboundedSender<String>,
        // boxed: a Child is large on Windows (clippy::large_enum_variant)
        child: Box<Mutex<Option<tokio::process::Child>>>,
        pending: Pending,
    },
    Http {
        client: reqwest::Client,
        url: String,
        headers: Vec<(String, String)>,
        session: Mutex<Option<String>>,
    },
}

pub struct McpServer {
    pub name: String,
    transport: Transport,
    next_id: AtomicI64,
    state: Mutex<(String, Option<String>)>,
    tools: Mutex<Vec<McpTool>>,
    info: Mutex<Value>,
    logs: Arc<Mutex<VecDeque<String>>>,
}

fn push_log(logs: &Mutex<VecDeque<String>>, line: String) {
    let mut l = logs.lock();
    if l.len() >= 300 {
        l.pop_front();
    }
    l.push_back(line);
}

fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if out.is_empty() {
        "x".into()
    } else {
        out
    }
}

pub fn api_tool_name(server: &str, tool: &str) -> String {
    let n = format!("{}__{}", sanitize(server), sanitize(tool));
    if n.len() <= 64 {
        n
    } else {
        let h = &sha256_hex(n.as_bytes())[..8];
        format!("{}_{}", &n[..55], h)
    }
}

impl McpServer {
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> AppResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match &self.transport {
            Transport::Stdio { tx, pending, .. } => {
                let (otx, rx) = oneshot::channel();
                pending.lock().insert(id, otx);
                if tx.send(format!("{}\n", msg)).is_err() {
                    pending.lock().remove(&id);
                    return Err(AppError::msg(format!("MCP server '{}' is not running", self.name)));
                }
                match tokio::time::timeout(timeout, rx).await {
                    Ok(Ok(Ok(v))) => Ok(v),
                    Ok(Ok(Err(e))) => Err(AppError::msg(format!(
                        "MCP error: {}",
                        e["message"].as_str().unwrap_or(&e.to_string())
                    ))),
                    Ok(Err(_)) => Err(AppError::msg(format!("MCP server '{}' disconnected", self.name))),
                    Err(_) => {
                        pending.lock().remove(&id);
                        Err(AppError::msg(format!("MCP server '{}' timed out on {method}", self.name)))
                    }
                }
            }
            Transport::Http {
                client,
                url,
                headers,
                session,
            } => {
                let mut req = client
                    .post(url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .header("MCP-Protocol-Version", PROTOCOL_VERSION)
                    .timeout(timeout)
                    .body(msg.to_string());
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                if let Some(s) = session.lock().clone() {
                    req = req.header("Mcp-Session-Id", s);
                }
                let resp = req.send().await?;
                if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
                    *session.lock() = Some(s.to_string());
                }
                let status = resp.status();
                if !status.is_success() {
                    let t = resp.text().await.unwrap_or_default();
                    return Err(AppError::msg(format!("MCP HTTP {}: {}", status.as_u16(), truncate_middle(&t, 300))));
                }
                let ctype = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let body = resp.text().await?;
                let msgs: Vec<Value> = if ctype.contains("text/event-stream") {
                    body.split("\n\n")
                        .filter_map(|ev| {
                            let data: Vec<&str> = ev
                                .lines()
                                .filter_map(|l| l.strip_prefix("data:").map(|d| d.trim_start()))
                                .collect();
                            if data.is_empty() {
                                None
                            } else {
                                serde_json::from_str(&data.join("\n")).ok()
                            }
                        })
                        .collect()
                } else {
                    match serde_json::from_str::<Value>(&body)? {
                        Value::Array(a) => a,
                        v => vec![v],
                    }
                };
                for m in msgs {
                    if m["id"].as_i64() == Some(id) {
                        if let Some(e) = m.get("error") {
                            return Err(AppError::msg(format!(
                                "MCP error: {}",
                                e["message"].as_str().unwrap_or(&e.to_string())
                            )));
                        }
                        return Ok(m["result"].clone());
                    }
                }
                Err(AppError::msg("MCP server returned no response"))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match &self.transport {
            Transport::Stdio { tx, .. } => {
                let _ = tx.send(format!("{}\n", msg));
            }
            Transport::Http {
                client,
                url,
                headers,
                session,
            } => {
                let mut req = client
                    .post(url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .header("MCP-Protocol-Version", PROTOCOL_VERSION)
                    .timeout(Duration::from_secs(15))
                    .body(msg.to_string());
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                if let Some(s) = session.lock().clone() {
                    req = req.header("Mcp-Session-Id", s);
                }
                let _ = req.send().await;
            }
        }
    }

    async fn initialize(&self) -> AppResult<()> {
        let r = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"roots": {"listChanged": false}},
                    "clientInfo": {"name": "ClaudeRust", "version": env!("CARGO_PKG_VERSION")}
                }),
                Duration::from_secs(60),
            )
            .await?;
        *self.info.lock() = json!({"serverInfo": r["serverInfo"], "capabilities": r["capabilities"], "instructions": r["instructions"]});
        self.notify("notifications/initialized", json!({})).await;
        self.refresh_tools().await
    }

    pub async fn refresh_tools(&self) -> AppResult<()> {
        let has_tools = !self.info.lock()["capabilities"]["tools"].is_null();
        if !has_tools {
            *self.tools.lock() = vec![];
            return Ok(());
        }
        let mut all = vec![];
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = self.request("tools/list", params, Duration::from_secs(60)).await?;
            if let Some(arr) = r["tools"].as_array() {
                for t in arr {
                    let name = t["name"].as_str().unwrap_or("").to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let mut schema = t["inputSchema"].clone();
                    if !schema.is_object() {
                        schema = json!({"type": "object", "properties": {}});
                    }
                    if schema["type"].is_null() {
                        schema["type"] = json!("object");
                    }
                    all.push(McpTool {
                        server: self.name.clone(),
                        api_name: api_tool_name(&self.name, &name),
                        title: t["title"].as_str().or(t["annotations"]["title"].as_str()).map(|s| s.to_string()),
                        description: t["description"].as_str().unwrap_or("").to_string(),
                        input_schema: schema,
                        read_only: t["annotations"]["readOnlyHint"].as_bool().unwrap_or(false),
                        name,
                    });
                }
            }
            cursor = r["nextCursor"].as_str().map(|s| s.to_string());
            if cursor.is_none() {
                break;
            }
        }
        *self.tools.lock() = all;
        Ok(())
    }

    async fn shutdown(&self) {
        if let Transport::Stdio { child, .. } = &self.transport {
            let c = child.lock().take();
            if let Some(mut c) = c {
                let _ = c.start_kill();
            }
        }
    }

    pub fn status(&self, disabled: bool) -> ServerStatus {
        let (status, error) = self.state.lock().clone();
        ServerStatus {
            name: self.name.clone(),
            transport: match self.transport {
                Transport::Stdio { .. } => "stdio".into(),
                Transport::Http { .. } => "http".into(),
            },
            status,
            error,
            tools: self.tools.lock().clone(),
            server_info: self.info.lock().clone(),
            logs: self.logs.lock().iter().cloned().collect(),
            disabled,
        }
    }
}

// ------------------------------------------------------------------ manager

pub struct McpManager {
    servers: tokio::sync::RwLock<HashMap<String, Arc<McpServer>>>,
    failed: Mutex<Vec<ServerStatus>>,
    /// Servers still connecting: (name, transport).
    starting: Mutex<Vec<(String, String)>>,
    /// Bumped by every start_all; connections from an older run are dropped.
    generation: std::sync::atomic::AtomicU64,
    pub config_path: PathBuf,
}

/// Paths where the official Claude Desktop app keeps its config, for import.
pub fn claude_desktop_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Application Support/Claude/claude_desktop_config.json"))
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir().map(|d| d.join("Claude").join("claude_desktop_config.json"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs::config_dir().map(|d| d.join("Claude").join("claude_desktop_config.json"))
    }
}

pub fn read_config(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"mcpServers": {}}))
}

/// Read the config in order to change it. Unlike `read_config`, a file that
/// exists but isn't valid JSON is an error, so it is never overwritten.
pub fn read_config_for_update(path: &Path) -> AppResult<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let mut v: Value = serde_json::from_str(&s).map_err(|e| {
                AppError::msg(format!(
                    "{} isn't valid JSON ({e}). Fix it in Settings → Developer → Edit config, then try again.",
                    path.display()
                ))
            })?;
            if !v.is_object() {
                return Err(AppError::msg(format!("{} must contain a JSON object.", path.display())));
            }
            if !v["mcpServers"].is_object() {
                v["mcpServers"] = json!({});
            }
            Ok(v)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({"mcpServers": {}})),
        Err(e) => Err(e.into()),
    }
}

/// Write the config through a temporary file so a crash can't truncate it.
pub fn write_config(path: &Path, v: &Value) -> AppResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn ensure_config(path: &Path) -> AppResult<()> {
    if !path.exists() {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&json!({"mcpServers": {}}))?)?;
    }
    Ok(())
}

impl McpManager {
    pub fn new(config_path: PathBuf) -> Self {
        McpManager {
            servers: tokio::sync::RwLock::new(HashMap::new()),
            failed: Mutex::new(vec![]),
            starting: Mutex::new(vec![]),
            generation: std::sync::atomic::AtomicU64::new(0),
            config_path,
        }
    }

    /// (Re)start every configured server that is not disabled.
    pub async fn start_all(&self, disabled: &[String]) {
        use std::sync::atomic::Ordering;
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.stop_all().await;
        let cfg = read_config(&self.config_path);
        let servers = cfg["mcpServers"].as_object().cloned().unwrap_or_default();
        let mut tasks = vec![];
        let mut starting = vec![];
        for (name, conf) in servers {
            if disabled.contains(&name) || conf["disabled"].as_bool() == Some(true) {
                self.failed.lock().push(ServerStatus {
                    name: name.clone(),
                    transport: if conf["url"].is_string() { "http".into() } else { "stdio".into() },
                    status: "disabled".into(),
                    error: None,
                    tools: vec![],
                    server_info: Value::Null,
                    logs: vec![],
                    disabled: true,
                });
                continue;
            }
            starting.push((name.clone(), if conf["url"].is_string() { "http".to_string() } else { "stdio".to_string() }));
            tasks.push(async move { (name.clone(), connect(&name, &conf).await) });
        }
        *self.starting.lock() = starting;
        // Register each server as soon as it connects, so one slow server
        // doesn't hold back the others.
        let mut pending: futures_util::stream::FuturesUnordered<_> = tasks.into_iter().collect();
        use futures_util::StreamExt;
        while let Some((name, res)) = pending.next().await {
            if self.generation.load(Ordering::SeqCst) != gen {
                // A newer reload started; it owns the server list now.
                if let Ok(s) = res {
                    s.shutdown().await;
                }
                continue;
            }
            self.starting.lock().retain(|(n, _)| *n != name);
            match res {
                Ok(s) => {
                    self.servers.write().await.insert(name, s);
                }
                Err((e, logs)) => self.failed.lock().push(ServerStatus {
                    name,
                    transport: String::new(),
                    status: "failed".into(),
                    error: Some(e.to_string()),
                    tools: vec![],
                    server_info: Value::Null,
                    logs,
                    disabled: false,
                }),
            }
        }
    }

    pub async fn stop_all(&self) {
        let mut s = self.servers.write().await;
        for (_, srv) in s.drain() {
            srv.shutdown().await;
        }
        self.failed.lock().clear();
    }

    pub async fn statuses(&self) -> Vec<ServerStatus> {
        let mut out: Vec<ServerStatus> = self.servers.read().await.values().map(|s| s.status(false)).collect();
        out.extend(self.failed.lock().iter().cloned());
        out.extend(self.starting.lock().iter().map(|(name, transport)| ServerStatus {
            name: name.clone(),
            transport: transport.clone(),
            status: "starting".into(),
            error: None,
            tools: vec![],
            server_info: Value::Null,
            logs: vec![],
            disabled: false,
        }));
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub async fn tools(&self) -> Vec<McpTool> {
        let mut out = vec![];
        for s in self.servers.read().await.values() {
            if s.state.lock().0 == "connected" {
                out.extend(s.tools.lock().iter().cloned());
            }
        }
        out.sort_by(|a, b| a.api_name.cmp(&b.api_name));
        out
    }

    pub async fn name_map(&self) -> HashMap<String, (String, String)> {
        self.tools()
            .await
            .into_iter()
            .map(|t| (t.api_name.clone(), (t.server.clone(), t.name.clone())))
            .collect()
    }

    pub async fn find_tool(&self, api_name: &str) -> Option<McpTool> {
        self.tools().await.into_iter().find(|t| t.api_name == api_name)
    }

    /// Call a tool and convert the MCP result into tool_result content blocks.
    pub async fn call(&self, api_name: &str, args: &Value) -> AppResult<(Vec<Value>, bool)> {
        let tool = self
            .find_tool(api_name)
            .await
            .ok_or_else(|| AppError::msg(format!("Tool {api_name} is not available")))?;
        let server = self
            .servers
            .read()
            .await
            .get(&tool.server)
            .cloned()
            .ok_or_else(|| AppError::msg("MCP server not connected"))?;
        let r = server
            .request(
                "tools/call",
                json!({"name": tool.name, "arguments": if args.is_object() { args.clone() } else { json!({}) }}),
                Duration::from_secs(600),
            )
            .await?;
        Ok(convert_result(&r))
    }

    /// List resources and prompts of a server (for the attach menu).
    pub async fn list_prompts(&self) -> Vec<Value> {
        let mut out = vec![];
        let servers: Vec<Arc<McpServer>> = self.servers.read().await.values().cloned().collect();
        for s in servers {
            if s.info.lock()["capabilities"]["prompts"].is_null() {
                continue;
            }
            if let Ok(r) = s.request("prompts/list", json!({}), Duration::from_secs(20)).await {
                for p in r["prompts"].as_array().cloned().unwrap_or_default() {
                    out.push(json!({"server": s.name, "name": p["name"], "description": p["description"], "arguments": p["arguments"]}));
                }
            }
        }
        out
    }

    pub async fn get_prompt(&self, server: &str, name: &str, args: &Value) -> AppResult<String> {
        let s = self
            .servers
            .read()
            .await
            .get(server)
            .cloned()
            .ok_or_else(|| AppError::msg("MCP server not connected"))?;
        let r = s
            .request("prompts/get", json!({"name": name, "arguments": args}), Duration::from_secs(60))
            .await?;
        let mut text = String::new();
        for m in r["messages"].as_array().cloned().unwrap_or_default() {
            let c = &m["content"];
            if c["type"] == "text" {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(c["text"].as_str().unwrap_or(""));
            } else if c["type"] == "resource" {
                if let Some(t) = c["resource"]["text"].as_str() {
                    text.push_str("\n\n");
                    text.push_str(t);
                }
            }
        }
        Ok(text)
    }

    pub async fn list_resources(&self) -> Vec<Value> {
        let mut out = vec![];
        let servers: Vec<Arc<McpServer>> = self.servers.read().await.values().cloned().collect();
        for s in servers {
            if s.info.lock()["capabilities"]["resources"].is_null() {
                continue;
            }
            if let Ok(r) = s.request("resources/list", json!({}), Duration::from_secs(20)).await {
                for res in r["resources"].as_array().cloned().unwrap_or_default() {
                    out.push(json!({"server": s.name, "uri": res["uri"], "name": res["name"], "mimeType": res["mimeType"], "description": res["description"]}));
                }
            }
        }
        out
    }

    pub async fn read_resource(&self, server: &str, uri: &str) -> AppResult<(String, String)> {
        let s = self
            .servers
            .read()
            .await
            .get(server)
            .cloned()
            .ok_or_else(|| AppError::msg("MCP server not connected"))?;
        let r = s
            .request("resources/read", json!({"uri": uri}), Duration::from_secs(60))
            .await?;
        let mut text = String::new();
        for c in r["contents"].as_array().cloned().unwrap_or_default() {
            if let Some(t) = c["text"].as_str() {
                text.push_str(t);
            }
        }
        Ok((uri.to_string(), text))
    }
}

pub fn convert_result(r: &Value) -> (Vec<Value>, bool) {
    let is_error = r["isError"].as_bool().unwrap_or(false);
    let mut blocks = vec![];
    for c in r["content"].as_array().cloned().unwrap_or_default() {
        match c["type"].as_str().unwrap_or("") {
            "text" => blocks.push(json!({"type": "text", "text": truncate_middle(c["text"].as_str().unwrap_or(""), 100_000)})),
            "image" => {
                let mime = c["mimeType"].as_str().unwrap_or("image/png");
                let normalized = if matches!(mime, "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
                    crate::attachments::normalize_image_b64(c["data"].as_str().unwrap_or(""), mime)
                } else {
                    None
                };
                if let Some((data, m)) = normalized {
                    blocks.push(json!({"type": "image", "source": {"type": "base64", "media_type": m, "data": data}}));
                } else {
                    blocks.push(json!({"type": "text", "text": format!("[image of type {mime}]")}));
                }
            }
            "resource" => {
                let res = &c["resource"];
                if let Some(t) = res["text"].as_str() {
                    blocks.push(json!({"type": "text", "text": format!("Resource {}:\n{}", res["uri"].as_str().unwrap_or(""), truncate_middle(t, 100_000))}));
                } else {
                    blocks.push(json!({"type": "text", "text": format!("[binary resource {}]", res["uri"].as_str().unwrap_or(""))}));
                }
            }
            "resource_link" => blocks.push(json!({"type": "text", "text": format!("Resource link: {} ({})", c["name"].as_str().unwrap_or(""), c["uri"].as_str().unwrap_or(""))})),
            "audio" => blocks.push(json!({"type": "text", "text": "[audio content]"})),
            _ => blocks.push(json!({"type": "text", "text": c.to_string()})),
        }
    }
    if blocks.is_empty() {
        if !r["structuredContent"].is_null() {
            blocks.push(json!({"type": "text", "text": r["structuredContent"].to_string()}));
        } else {
            blocks.push(json!({"type": "text", "text": "(no output)"}));
        }
    }
    (blocks, is_error)
}

async fn connect(name: &str, conf: &Value) -> Result<Arc<McpServer>, (AppError, Vec<String>)> {
    let logs: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let transport = if let Some(url) = conf["url"].as_str() {
        let headers: Vec<(String, String)> = conf["headers"]
            .as_object()
            .map(|h| {
                h.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| (AppError::Http(e), vec![]))?;
        Transport::Http {
            client,
            url: url.to_string(),
            headers,
            session: Mutex::new(None),
        }
    } else {
        let command = conf["command"]
            .as_str()
            .ok_or_else(|| (AppError::msg("Missing \"command\" (or \"url\") in server config"), vec![]))?;
        let args: Vec<String> = conf["args"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let mut cmd = crate::shellenv::command(command, &args);
        if let Some(env) = conf["env"].as_object() {
            for (k, v) in env {
                if let Some(s) = v.as_str() {
                    cmd.env(k, s);
                }
            }
        }
        if let Some(cwd) = conf["cwd"].as_str() {
            cmd.current_dir(cwd);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| (AppError::msg(format!("Could not start `{command}`: {e}")), vec![]))?;
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        // writer: the only owner of stdin
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });
        // stderr -> logs
        {
            let logs = logs.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    push_log(&logs, l);
                }
            });
        }
        // stdout -> responses, and replies to requests the server sends us
        {
            let pending = pending.clone();
            let logs = logs.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    let Ok(v) = serde_json::from_str::<Value>(&l) else {
                        push_log(&logs, format!("[stdout] {l}"));
                        continue;
                    };
                    if let Some(method) = v["method"].as_str() {
                        // A request from the server (has an id) needs an answer.
                        if !v["id"].is_null() {
                            let reply = match method {
                                "ping" => json!({"jsonrpc": "2.0", "id": v["id"], "result": {}}),
                                "roots/list" => json!({"jsonrpc": "2.0", "id": v["id"], "result": {"roots": []}}),
                                _ => json!({"jsonrpc": "2.0", "id": v["id"], "error": {"code": -32601, "message": format!("Method not supported: {method}")}}),
                            };
                            let _ = tx.send(format!("{reply}\n"));
                        }
                        continue;
                    }
                    if let Some(id) = v["id"].as_i64() {
                        if v.get("result").is_some() || v.get("error").is_some() {
                            if let Some(otx) = pending.lock().remove(&id) {
                                let _ = otx.send(match v.get("error") {
                                    Some(e) => Err(e.clone()),
                                    None => Ok(v["result"].clone()),
                                });
                            }
                        }
                    }
                }
                push_log(&logs, "[server process exited]".into());
                pending.lock().clear();
            });
        }
        Transport::Stdio {
            tx,
            child: Box::new(Mutex::new(Some(child))),
            pending,
        }
    };
    let server = Arc::new(McpServer {
        name: name.to_string(),
        transport,
        next_id: AtomicI64::new(1),
        state: Mutex::new(("starting".into(), None)),
        tools: Mutex::new(vec![]),
        info: Mutex::new(Value::Null),
        logs: logs.clone(),
    });
    match server.initialize().await {
        Ok(()) => {
            *server.state.lock() = ("connected".into(), None);
            Ok(server)
        }
        Err(e) => {
            server.shutdown().await;
            let l: Vec<String> = logs.lock().iter().cloned().collect();
            Err((e, l))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names() {
        assert_eq!(api_tool_name("file system", "read_file"), "file_system__read_file");
        let long = api_tool_name(&"s".repeat(40), &"t".repeat(40));
        assert!(long.len() <= 64);
    }

    #[test]
    fn result_conversion() {
        use base64::Engine;
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(png.get_ref());
        let (b, err) = convert_result(&json!({"content":[{"type":"text","text":"ok"},{"type":"image","data":data,"mimeType":"image/png"}]}));
        let (bad, _) = convert_result(&json!({"content":[{"type":"image","data":"AAA","mimeType":"image/png"}]}));
        assert_eq!(bad[0]["type"], "text");
        assert!(!err);
        assert_eq!(b[0]["text"], "ok");
        assert_eq!(b[1]["source"]["media_type"], "image/png");
    }

    #[tokio::test]
    async fn stdio_roundtrip_with_fake_server() {
        // A tiny MCP server implemented in sh + python-free awk is fiddly; use
        // a Python one-liner only when python3 exists.
        if which::which("python3").is_err() {
            return;
        }
        let script = r#"
import sys, json
got_roots = False
deferred = None
def send(o):
    print(json.dumps(o), flush=True)
TOOLS = {'tools': [{'name': 'echo', 'description': 'Echo', 'inputSchema': {'type': 'object', 'properties': {'text': {'type': 'string'}}}}]}
for line in sys.stdin:
    m = json.loads(line)
    if m.get('method') == 'notifications/initialized':
        send({'jsonrpc': '2.0', 'id': 'srv-1', 'method': 'roots/list'})
        continue
    if 'method' not in m:
        if m.get('id') == 'srv-1' and m.get('result') == {'roots': []}:
            got_roots = True
            if deferred is not None:
                send({'jsonrpc': '2.0', 'id': deferred, 'result': TOOLS})
        continue
    if 'id' not in m: continue
    if m['method'] == 'tools/list' and not got_roots:
        deferred = m['id']
        continue
    if m['method'] == 'initialize':
        r = {'protocolVersion': '2025-06-18', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fake', 'version': '1'}}
    elif m['method'] == 'tools/list':
        r = {'tools': [{'name': 'echo', 'description': 'Echo', 'inputSchema': {'type': 'object', 'properties': {'text': {'type': 'string'}}}}]}
    elif m['method'] == 'tools/call':
        r = {'content': [{'type': 'text', 'text': m['params']['arguments'].get('text', '')}]}
    else:
        r = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': m['id'], 'result': r}), flush=True)
"#;
        let dir = std::env::temp_dir().join(format!("mcp-test-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(
            &cfg,
            json!({"mcpServers": {"fake": {"command": "python3", "args": ["-c", script]}}}).to_string(),
        )
        .unwrap();
        let m = McpManager::new(cfg);
        m.start_all(&[]).await;
        let st = m.statuses().await;
        assert_eq!(st[0].status, "connected", "{:?}", st[0]);
        let tools = m.tools().await;
        assert_eq!(tools[0].api_name, "fake__echo");
        let (blocks, _) = m.call("fake__echo", &json!({"text": "hi"})).await.unwrap();
        assert_eq!(blocks[0]["text"], "hi");
        m.stop_all().await;
        let _ = std::fs::remove_dir_all(dir);
    }
}
