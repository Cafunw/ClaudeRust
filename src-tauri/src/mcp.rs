//! Model Context Protocol client. Reads the same `mcpServers` format as
//! Claude Desktop's `claude_desktop_config.json` and connects to local
//! (stdio) and remote (streamable HTTP, or the older HTTP+SSE) servers.

#[path = "mcp_elicit.rs"]
pub mod elicit;
#[path = "mcp_sse.rs"]
mod sse;

use crate::error::{AppError, AppResult};
use crate::util::{sha256_hex, truncate_middle};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Longest stdout line (one JSON-RPC message) taken from a stdio server; a
/// longer one is dropped (and logged) instead of being buffered without end.
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Longest stderr line kept in a server's log.
const MAX_LOG_LINE_BYTES: usize = 8 * 1024;
/// How long a stdio server gets to exit by itself once its stdin is closed,
/// before it and everything it started are killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// A stdio server whose process exits is started again after 1, 2, 4, 8
/// and 16 s; after that many failed tries in a row it stays failed.
const RESTART_TRIES: u32 = 5;
#[cfg(not(test))]
const RESTART_FIRST_WAIT: Duration = Duration::from_secs(1);
#[cfg(test)]
const RESTART_FIRST_WAIT: Duration = Duration::from_millis(20);
/// A server that ran this long before it exited gets all its tries back.
const RESTART_RESET: Duration = Duration::from_secs(60);

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
    /// OAuth tokens are stored for this remote server.
    pub signed_in: bool,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, Value>>>>>;
type State = Arc<Mutex<(String, Option<String>)>>;
type Logs = Arc<Mutex<VecDeque<String>>>;
type Servers = Arc<tokio::sync::RwLock<HashMap<String, Arc<McpServer>>>>;

/// The OAuth sign-in of a remote server (none when its config sets an
/// Authorization header of its own).
struct OAuth {
    key: String,
    tokens: tokio::sync::Mutex<Option<crate::mcp_oauth::Tokens>>,
}

enum Transport {
    Stdio {
        /// Lines to write to the server's stdin. A writer task owns the pipe
        /// and closes it once this, the only strong sender, is dropped.
        tx: Mutex<Option<UnboundedSender<String>>>,
        // boxed: a Child is large on Windows (clippy::large_enum_variant)
        child: Box<Mutex<Option<tokio::process::Child>>>,
        pending: Pending,
        /// Cancelled when the server's stdout ends (the process exited or
        /// closed it): nothing it is asked can be answered any more.
        closed: CancellationToken,
    },
    Http {
        client: reqwest::Client,
        url: String,
        headers: Vec<(String, String)>,
        session: Mutex<Option<String>>,
        /// Held while a new session replaces one the server expired.
        renewing: tokio::sync::Mutex<()>,
        // boxed like `child`: the tokens are large (clippy::large_enum_variant)
        oauth: Option<Box<OAuth>>,
        /// The event stream of a server on the old HTTP+SSE transport (MCP
        /// 2024-11-05), which carries everything that server sends.
        sse: Mutex<Option<Arc<sse::Stream>>>,
    },
}

pub struct McpServer {
    pub name: String,
    transport: Transport,
    next_id: AtomicI64,
    state: State,
    tools: Mutex<Vec<McpTool>>,
    info: Mutex<Value>,
    logs: Logs,
    /// Told whenever the server changes by itself (see McpManager::changed).
    changed: Arc<Notify>,
    /// The server said its tool list changed (`notifications/tools/list_changed`).
    tools_changed: UnboundedSender<()>,
    /// Requests the server sent us, to be answered (see `connect`).
    requests: UnboundedSender<Value>,
}

fn push_log(logs: &Mutex<VecDeque<String>>, line: String) {
    let mut l = logs.lock();
    if l.len() >= 300 {
        l.pop_front();
    }
    l.push_back(line);
}

/// One line from `r` without its line ending, decoded leniently: bytes that
/// aren't UTF-8 (a server logging in the system code page) become U+FFFD
/// instead of ending the stream. At most `max` bytes are kept; the rest of a
/// longer line is read and dropped, and the flag says so. `None` at the end.
pub(crate) async fn read_line_lossy<R: AsyncBufRead + Unpin>(r: &mut R, max: usize) -> std::io::Result<Option<(String, bool)>> {
    fn keep(line: &mut Vec<u8>, cut: &mut bool, max: usize, bytes: &[u8]) {
        let room = max.saturating_sub(line.len());
        if bytes.len() > room {
            *cut = true;
        }
        line.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }
    let mut line: Vec<u8> = Vec::new();
    let mut cut = false;
    let mut any = false;
    // a read ended on "\r": part of a "\r\n" if the next one starts with "\n"
    let mut held_cr = false;
    loop {
        let buf = match r.fill_buf().await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            if !any {
                return Ok(None);
            }
            // a last line without a line break keeps everything
            if held_cr {
                keep(&mut line, &mut cut, max, b"\r");
            }
            break;
        }
        any = true;
        let nl = buf.iter().position(|&b| b == b'\n');
        if std::mem::take(&mut held_cr) && nl != Some(0) {
            keep(&mut line, &mut cut, max, b"\r");
        }
        let used = match nl {
            Some(i) => {
                let part = &buf[..i];
                keep(&mut line, &mut cut, max, part.strip_suffix(b"\r").unwrap_or(part));
                i + 1
            }
            None => {
                let part = match buf.strip_suffix(b"\r") {
                    Some(p) => {
                        held_cr = true;
                        p
                    }
                    None => buf,
                };
                keep(&mut line, &mut cut, max, part);
                buf.len()
            }
        };
        r.consume(used);
        if nl.is_some() {
            break;
        }
    }
    Ok(Some((String::from_utf8_lossy(&line).into_owned(), cut)))
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

/// Give every tool its own API name. Different servers or tools can map to
/// the same name ("my server" and "my_server", "files.read" and
/// "files_read"), and the API rejects a request whose tool names repeat.
/// In such a group the tool whose names needed no changes keeps the plain
/// name; the others get a short hash of their real server and tool name, so
/// the name is stable for as long as the clash lasts.
fn unique_api_names(mut tools: Vec<McpTool>) -> Vec<McpTool> {
    tools.sort_by(|a, b| (&a.server, &a.name).cmp(&(&b.server, &b.name)));
    let mut count: HashMap<String, usize> = HashMap::new();
    for t in &tools {
        *count.entry(t.api_name.clone()).or_default() += 1;
    }
    let mut taken: HashSet<String> = count.iter().filter(|(_, n)| **n == 1).map(|(k, _)| k.clone()).collect();
    // plain names first, so a hashed name can't take one
    let mut renamed = vec![];
    for (i, t) in tools.iter().enumerate() {
        if count[&t.api_name] == 1 {
            continue;
        }
        let exact = format!("{}__{}", t.server, t.name) == t.api_name;
        if !(exact && taken.insert(t.api_name.clone())) {
            renamed.push(i);
        }
    }
    for i in renamed {
        let t = &mut tools[i];
        let stem = t.api_name[..t.api_name.len().min(55)].to_string();
        let mut salt = 0u32;
        let name = loop {
            let h = sha256_hex(format!("{}\u{0}{}\u{0}{salt}", t.server, t.name).as_bytes());
            let n = format!("{stem}_{}", &h[..8]);
            if taken.insert(n.clone()) {
                break n;
            }
            salt += 1;
        };
        t.api_name = name;
    }
    tools.sort_by(|a, b| a.api_name.cmp(&b.api_name));
    tools
}

/// What a request got back over the transport.
enum Reply {
    Done(Value),
    /// HTTP 404 to a request that carried this session id: the server ended
    /// the session, and a new one has to be negotiated.
    SessionExpired(String),
    /// HTTP 401 to a request that carried this access token (or none).
    Unauthorized(Option<String>),
}

/// A request waiting for its answer. Dropped before the answer came (the
/// user pressed Stop, or it timed out), it tells the server to stop working
/// on it (`notifications/cancelled`) and forgets it.
struct InFlight<'a> {
    server: &'a McpServer,
    id: i64,
    /// `initialize` must never be cancelled (the spec forbids it).
    notify: bool,
    /// The exchange is over: an answer (or a definite failure) arrived.
    finished: bool,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.server.abandon(self.id, self.notify);
        }
    }
}

fn rpc_error(e: &Value) -> AppError {
    AppError::msg(format!("MCP error: {}", e["message"].as_str().map(String::from).unwrap_or_else(|| e.to_string())))
}

/// The answer to request `id` among JSON-RPC messages (one, or a batch).
fn find_reply(v: Value, id: i64) -> Option<Value> {
    let msgs = match v {
        Value::Array(a) => a,
        v => vec![v],
    };
    msgs.into_iter().find(|m| m["id"].as_i64() == Some(id) && (m.get("result").is_some() || m.get("error").is_some()))
}

fn reply_result(m: Value) -> AppResult<Value> {
    match m.get("error") {
        Some(e) => Err(rpc_error(e)),
        None => Ok(m["result"].clone()),
    }
}

fn http_post(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    bearer: Option<&str>,
    session: Option<&str>,
    body: String,
    timeout: Duration,
) -> reqwest::RequestBuilder {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .timeout(timeout)
        .body(body);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(b) = bearer {
        req = req.bearer_auth(b);
    }
    if let Some(s) = session {
        req = req.header("Mcp-Session-Id", s);
    }
    req
}

const TOOLS_CHANGED: &str = "notifications/tools/list_changed";

/// Read a `text/event-stream` answer until the reply to `id` arrives; the
/// server's requests on the way (an elicitation during a tool call) go to
/// `requests`. Events are delimited by blank lines with LF or CRLF line
/// endings (the official Python SDK sends CRLF).
async fn read_sse_reply(resp: reqwest::Response, id: i64, tools_changed: &UnboundedSender<()>, requests: &UnboundedSender<Value>) -> Result<Option<Value>, reqwest::Error> {
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut parser = crate::anthropic::SseParser::default();
    let found = |events: Vec<crate::anthropic::SseEvent>| {
        events
            .into_iter()
            .filter(|ev| !ev.data.is_empty())
            .filter_map(|ev| serde_json::from_str::<Value>(&ev.data).ok())
            .find_map(|v| {
                if v["method"] == TOOLS_CHANGED {
                    let _ = tools_changed.send(());
                } else if v["method"].is_string() && !v["id"].is_null() {
                    let _ = requests.send(v);
                    return None;
                }
                find_reply(v, id)
            })
    };
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
        if let Some(m) = found(parser.feed(&mut buf)) {
            return Ok(Some(m));
        }
    }
    Ok(found(parser.finish(&mut buf)))
}

/// Kill a stdio server and everything it started (`npx`/`uvx` run the real
/// server as a child). Call it before the child is reaped.
fn kill_server_tree(child: &tokio::process::Child) {
    let Some(pid) = child.id() else { return };
    #[cfg(unix)]
    {
        // Not `codetools::kill_process_group`: that one first checks that
        // `pid` still leads its group, which fails once the leader has exited
        // (macOS doesn't report a zombie's group), and its children are just
        // what is left to kill then. Signalling the group directly is safe:
        // the server leads its own group (`process_group(0)`) and, not yet
        // reaped, keeps that id from being reused.
        if let Ok(pgid) = libc::pid_t::try_from(pid) {
            if pgid > 1 {
                // SAFETY: a plain syscall on a process group we created.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
        }
    }
    #[cfg(windows)]
    crate::codetools::kill_process_tree(pid);
}

impl McpServer {
    fn not_running(&self) -> AppError {
        let why = self.state.lock().1.clone().map(|e| format!(" ({e})")).unwrap_or_default();
        AppError::msg(format!("MCP server '{}' is not running{why}. Reload it in Settings → Developer.", self.name))
    }

    /// Send one request and wait for its answer (no session renewal).
    async fn request_raw(&self, method: &str, params: &Value, timeout: Duration) -> AppResult<Reply> {
        let bearer = self.bearer().await;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut flight = InFlight { server: self, id, notify: method != "initialize", finished: false };
        match &self.transport {
            Transport::Stdio { tx, pending, closed, .. } => {
                let (otx, rx) = oneshot::channel();
                pending.lock().insert(id, otx);
                let sent = tx.lock().as_ref().is_some_and(|t| t.send(format!("{msg}\n")).is_ok());
                // `closed` is set before the reader drops what is pending, so
                // a request registered after that is caught here
                if !sent || closed.is_cancelled() {
                    pending.lock().remove(&id);
                    flight.finished = true;
                    return Err(self.not_running());
                }
                self.answer(rx, method, tokio::time::Instant::now() + timeout, &mut flight).await
            }
            Transport::Http {
                client,
                url,
                headers,
                session,
                oauth,
                sse,
                ..
            } => {
                let stream = sse.lock().clone();
                if stream.is_some() {
                    return self.request_sse(stream, method, &msg, bearer, timeout, &mut flight).await;
                }
                let sent_session = session.lock().clone();
                let resp = match http_post(client, url, headers, bearer.as_deref(), sent_session.as_deref(), msg.to_string(), timeout).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        // a request that timed out is withdrawn; one that never
                        // got through has nothing to withdraw
                        flight.finished = !e.is_timeout();
                        return Err(e.into());
                    }
                };
                if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
                    *session.lock() = Some(s.to_string());
                }
                let status = resp.status();
                if status == reqwest::StatusCode::NOT_FOUND {
                    if let Some(old) = sent_session {
                        flight.finished = true;
                        return Ok(Reply::SessionExpired(old));
                    }
                }
                if status == reqwest::StatusCode::UNAUTHORIZED && oauth.is_some() {
                    flight.finished = true;
                    return Ok(Reply::Unauthorized(bearer));
                }
                // how a server on the old HTTP+SSE transport turns down this POST
                // (the spec's backwards-compatibility procedure)
                if method == "initialize" && matches!(status.as_u16(), 400 | 404 | 405) {
                    push_log(&self.logs, format!("[HTTP {} to initialize: trying the older HTTP+SSE transport]", status.as_u16()));
                    return self.request_sse(None, method, &msg, bearer, timeout, &mut flight).await;
                }
                if !status.is_success() {
                    flight.finished = true;
                    let t = resp.text().await.unwrap_or_default();
                    return Err(AppError::msg(format!("MCP HTTP {}: {}", status.as_u16(), truncate_middle(&t, 300))));
                }
                let ctype = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let found = if ctype.contains("text/event-stream") {
                    read_sse_reply(resp, id, &self.tools_changed, &self.requests).await
                } else {
                    resp.text().await.map(|body| serde_json::from_str::<Value>(&body).ok().and_then(|v| find_reply(v, id)))
                };
                let found = match found {
                    Ok(f) => f,
                    Err(e) => {
                        flight.finished = !e.is_timeout();
                        return Err(e.into());
                    }
                };
                flight.finished = true;
                match found {
                    Some(m) => reply_result(m).map(Reply::Done),
                    None => Err(AppError::msg("MCP server returned no response")),
                }
            }
        }
    }

    /// Withdraw request `id` whose answer nobody waits for any more.
    fn abandon(&self, id: i64, notify: bool) {
        let note = json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": "The request was stopped."}});
        match &self.transport {
            Transport::Stdio { tx, pending, .. } => {
                // gone from `pending` means it was answered (or the server is gone)
                if pending.lock().remove(&id).is_some() && notify {
                    if let Some(t) = tx.lock().as_ref() {
                        let _ = t.send(format!("{note}\n"));
                    }
                }
            }
            Transport::Http { client, headers, oauth, sse, .. } => {
                // on the old transport the answer would come on the stream
                let answered = sse.lock().as_ref().is_some_and(|s| s.pending.lock().remove(&id).is_none());
                let Some((to, session)) = self.post_target().filter(|_| notify && !answered) else { return };
                // no refresh here: a token being refreshed is skipped
                let bearer = oauth.as_ref().and_then(|o| o.tokens.try_lock().ok()?.as_ref().map(|t| t.access_token.clone()));
                let req = http_post(client, &to, headers, bearer.as_deref(), session.as_deref(), note.to_string(), Duration::from_secs(15));
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move {
                        let _ = req.send().await;
                    });
                }
            }
        }
    }

    /// `request_raw`, sent once more with a refreshed token when the server
    /// turned down the one it got (HTTP 401). Never `Reply::Unauthorized`.
    async fn request_authorized(&self, method: &str, params: &Value, timeout: Duration) -> AppResult<Reply> {
        match self.request_raw(method, params, timeout).await? {
            Reply::Unauthorized(used) => {
                self.reauthorize(used.as_deref()).await?;
                match self.request_raw(method, params, timeout).await? {
                    Reply::Unauthorized(_) => Err(self.sign_in_needed()),
                    r => Ok(r),
                }
            }
            r => Ok(r),
        }
    }

    async fn request(&self, method: &str, params: Value, timeout: Duration) -> AppResult<Value> {
        let old = match self.request_authorized(method, &params, timeout).await? {
            Reply::SessionExpired(old) => old,
            Reply::Done(v) => return Ok(v),
            Reply::Unauthorized(_) => return Err(self.sign_in_needed()),
        };
        // The server ended the session (it restarted, or expired it): the spec
        // says to start a new one, after which the request is sent once more.
        self.renew_session(&old).await?;
        match self.request_authorized(method, &params, timeout).await? {
            Reply::Done(v) => Ok(v),
            Reply::SessionExpired(_) => Err(AppError::msg(format!(
                "MCP server '{}' ended its session again right after a new one started. Reload it in Settings → Developer.",
                self.name
            ))),
            Reply::Unauthorized(_) => Err(self.sign_in_needed()),
        }
    }

    /// The access token to send, refreshed first when it is about to expire.
    async fn bearer(&self) -> Option<String> {
        let Transport::Http { url, oauth: Some(o), .. } = &self.transport else { return None };
        let mut slot = o.tokens.lock().await;
        if slot.as_ref().is_some_and(|t| t.expires_soon()) {
            // a failed refresh keeps the old token: the server says whether it still works
            if let Err(e) = self.refresh(url, o, &mut slot).await {
                push_log(&self.logs, format!("[refreshing the sign-in failed: {e}]"));
            }
        }
        slot.as_ref().map(|t| t.access_token.clone())
    }

    /// Replace `slot`'s tokens with new ones from the authorization server.
    /// `Ok(false)`: it refused, and the sign-in is forgotten.
    async fn refresh(&self, url: &str, o: &OAuth, slot: &mut Option<crate::mcp_oauth::Tokens>) -> AppResult<bool> {
        let Some(cur) = slot.as_ref() else { return Ok(false) };
        match crate::mcp_oauth::refresh(url, cur).await? {
            Some(new) => {
                if let Err(e) = crate::mcp_oauth::save(&o.key, &new) {
                    push_log(&self.logs, format!("[{e}]"));
                }
                *slot = Some(new);
                Ok(true)
            }
            None => {
                crate::mcp_oauth::forget(&o.key);
                *slot = None;
                Ok(false)
            }
        }
    }

    /// After a 401 to a request sent with `used`: Ok once there is a token
    /// worth trying again.
    async fn reauthorize(&self, used: Option<&str>) -> AppResult<()> {
        let Transport::Http { url, oauth: Some(o), .. } = &self.transport else { return Err(self.sign_in_needed()) };
        let mut slot = o.tokens.lock().await;
        match slot.as_ref() {
            // another request got a new one meanwhile
            Some(t) if Some(t.access_token.as_str()) != used => return Ok(()),
            Some(t) if t.refresh_token.is_some() => {
                if self.refresh(url, o, &mut slot).await? {
                    return Ok(());
                }
            }
            Some(_) => {
                crate::mcp_oauth::forget(&o.key);
                *slot = None;
            }
            None => {}
        }
        Err(self.sign_in_needed())
    }

    fn sign_in_needed(&self) -> AppError {
        *self.state.lock() = ("needs_auth".into(), Some("Sign in to use this server.".into()));
        self.changed.notify_one();
        AppError::Api {
            status: 401,
            kind: "needs_auth".into(),
            message: format!("MCP server '{}' needs you to sign in (Settings → Developer).", self.name),
        }
    }

    async fn renew_session(&self, expired: &str) -> AppResult<()> {
        let Transport::Http { session, renewing, .. } = &self.transport else { return Ok(()) };
        let _one_at_a_time = renewing.lock().await;
        // another request may have renewed it meanwhile
        if session.lock().as_deref() != Some(expired) {
            return Ok(());
        }
        *session.lock() = None;
        self.handshake()
            .await
            .map_err(|e| AppError::msg(format!("MCP server '{}' ended its session, and starting a new one failed: {e}", self.name)))
    }

    /// The answer `rx` brings by `until` (stdio, and the old HTTP+SSE transport).
    async fn answer(&self, rx: oneshot::Receiver<Result<Value, Value>>, method: &str, until: tokio::time::Instant, flight: &mut InFlight<'_>) -> AppResult<Reply> {
        match tokio::time::timeout_at(until, rx).await {
            Ok(answer) => {
                flight.finished = true;
                match answer {
                    Ok(Ok(v)) => Ok(Reply::Done(v)),
                    Ok(Err(e)) => Err(rpc_error(&e)),
                    Err(_) => Err(AppError::msg(format!("MCP server '{}' disconnected", self.name))),
                }
            }
            // the guard withdraws the request
            Err(_) => Err(AppError::msg(format!("MCP server '{}' timed out on {method}", self.name))),
        }
    }

    /// Where messages to an HTTP server are POSTed, and the session id they
    /// carry: the endpoint the old transport's stream named (its URL holds the
    /// session), or the server's URL.
    fn post_target(&self) -> Option<(String, Option<String>)> {
        let Transport::Http { url, session, sse, .. } = &self.transport else { return None };
        if let Some(s) = sse.lock().as_ref() {
            return Some((s.endpoint.clone(), None));
        }
        Some((url.clone(), session.lock().clone()))
    }

    /// Send a message nothing answers: a notification, or our answer to a
    /// request of the server's.
    async fn send(&self, msg: Value) {
        match &self.transport {
            Transport::Stdio { tx, .. } => {
                if let Some(t) = tx.lock().as_ref() {
                    let _ = t.send(format!("{msg}\n"));
                }
            }
            Transport::Http { client, headers, .. } => {
                let bearer = self.bearer().await;
                let Some((to, session)) = self.post_target() else { return };
                let _ = http_post(client, &to, headers, bearer.as_deref(), session.as_deref(), msg.to_string(), Duration::from_secs(15)).send().await;
            }
        }
    }

    /// `initialize` and `notifications/initialized`: starts a session.
    async fn handshake(&self) -> AppResult<()> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"roots": {"listChanged": false}, "elicitation": {}},
            "clientInfo": {"name": "ClaudeRust", "version": env!("CARGO_PKG_VERSION")}
        });
        let r = match self.request_authorized("initialize", &params, Duration::from_secs(60)).await? {
            Reply::Done(v) => v,
            // only a request that carried a session id can see one expire
            Reply::SessionExpired(_) => return Err(AppError::msg("the server rejected the new session")),
            Reply::Unauthorized(_) => return Err(self.sign_in_needed()),
        };
        *self.info.lock() = json!({"serverInfo": r["serverInfo"], "capabilities": r["capabilities"], "instructions": r["instructions"]});
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})).await;
        Ok(())
    }

    async fn initialize(&self) -> AppResult<()> {
        self.handshake().await?;
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

    /// Stop the server. A stdio server first gets its stdin closed, the
    /// usual way to ask it to exit; whatever is still running after a short
    /// grace period (a server that doesn't exit, the real server behind an
    /// `npx` or `uvx` wrapper) is killed together with its process group.
    async fn shutdown(&self) {
        let Transport::Stdio { tx, child, pending, closed } = &self.transport else { return };
        drop(tx.lock().take());
        let c = child.lock().take();
        if let Some(mut c) = c {
            // its stdout closes once every process holding it has exited
            let _ = tokio::time::timeout(SHUTDOWN_GRACE, closed.cancelled()).await;
            kill_server_tree(&c);
            let _ = c.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), c.wait()).await;
        }
        pending.lock().clear();
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
            // busy: a refresh is under way, so there are tokens
            signed_in: match &self.transport {
                Transport::Http { oauth: Some(o), .. } => o.tokens.try_lock().map_or(true, |t| t.is_some()),
                _ => false,
            },
        }
    }
}

// ------------------------------------------------------------------ manager

pub struct McpManager {
    servers: Servers,
    /// See `changed`.
    changed: Arc<Notify>,
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

/// The text of a config file. Windows editors often start one with a byte
/// order mark (Notepad: UTF-8 with BOM; PowerShell 5 `>`: UTF-16), which
/// JSON parsers reject, so it is decoded and the mark dropped.
fn read_config_text(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let utf16 = |be: bool| crate::util::utf16(&bytes[2..], be).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid UTF-16 text"));
    match bytes.as_slice() {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        [0xFF, 0xFE, ..] => utf16(false),
        [0xFE, 0xFF, ..] => utf16(true),
        _ => String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
    }
}

pub fn read_config(path: &Path) -> Value {
    read_config_text(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"mcpServers": {}}))
}

/// The config file's text as it is on disk, for the editor in Settings, so a
/// file that doesn't parse is shown (and fixed) instead of replaced by an
/// empty config. Text that isn't UTF-8 is shown with replacement characters.
/// `None` when there is no file.
pub fn config_text(path: &Path) -> Option<String> {
    match read_config_text(path) {
        Ok(t) => Some(t),
        Err(_) => std::fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
    }
}

/// What is wrong with the config file, in words for Settings: `None` when
/// it is fine or doesn't exist yet. (`read_config` falls back to no servers
/// for a broken file; this says why none were started.)
pub fn config_problem(path: &Path) -> Option<String> {
    let text = match read_config_text(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Some(format!("{} isn't UTF-8 text ({e}); save it as UTF-8.", path.display()))
        }
        Err(e) => return Some(format!("{} can't be read: {e}", path.display())),
    };
    match serde_json::from_str::<Value>(&text) {
        Err(e) => Some(format!("{} isn't valid JSON ({e}), so no MCP servers were started from it.", path.display())),
        Ok(v) if !v.is_object() => Some(format!("{} must contain a JSON object.", path.display())),
        Ok(v) if !v["mcpServers"].is_null() && !v["mcpServers"].is_object() => {
            Some(format!("\"mcpServers\" in {} must be an object (server name → settings).", path.display()))
        }
        Ok(_) => None,
    }
}

/// Read the config in order to change it. Unlike `read_config`, a file that
/// exists but isn't valid JSON is an error, so it is never overwritten.
pub fn read_config_for_update(path: &Path) -> AppResult<Value> {
    match read_config_text(path) {
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
            servers: Servers::default(),
            changed: Arc::new(Notify::new()),
            failed: Mutex::new(vec![]),
            starting: Mutex::new(vec![]),
            generation: std::sync::atomic::AtomicU64::new(0),
            config_path,
        }
    }

    /// Returns once a server changed by itself since the last call: it was
    /// restarted or given up, needs a sign-in, or has new tools.
    pub async fn changed(&self) {
        self.changed.notified().await
    }

    /// (Re)start every configured server that is not disabled.
    pub async fn start_all(&self, disabled: &[String]) {
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
                    transport: transport_of(&conf).into(),
                    status: "disabled".into(),
                    error: None,
                    tools: vec![],
                    server_info: Value::Null,
                    logs: vec![],
                    disabled: true,
                    signed_in: false,
                });
                continue;
            }
            starting.push((name.clone(), transport_of(&conf).to_string()));
            let changed = self.changed.clone();
            tasks.push(async move {
                let res = connect(&name, &conf, changed, Logs::default()).await;
                (name, conf, res)
            });
        }
        *self.starting.lock() = starting;
        // Register each server as soon as it connects, so one slow server
        // doesn't hold back the others.
        let mut pending: futures_util::stream::FuturesUnordered<_> = tasks.into_iter().collect();
        while let Some((name, conf, res)) = pending.next().await {
            // a newer reload owns the list of servers still starting
            if self.generation.load(Ordering::SeqCst) == gen {
                self.starting.lock().retain(|(n, _)| *n != name);
            }
            self.register(gen, name, conf, res).await;
        }
    }

    /// Put a server that connected (or failed to) in its place, unless a
    /// newer start_all began meanwhile, and keep a stdio one running.
    async fn register(&self, gen: u64, name: String, conf: Value, res: Result<Arc<McpServer>, (AppError, Vec<String>)>) {
        match res {
            Ok(s) => {
                let mut servers = self.servers.write().await;
                if self.generation.load(Ordering::SeqCst) != gen {
                    drop(servers);
                    s.shutdown().await;
                    return;
                }
                let old = servers.insert(name.clone(), s.clone());
                drop(servers);
                if let Some(old) = old {
                    old.shutdown().await;
                }
                if matches!(s.transport, Transport::Stdio { .. }) {
                    tokio::spawn(supervise(self.servers.clone(), name, conf, s));
                }
            }
            Err((e, logs)) if self.generation.load(Ordering::SeqCst) == gen => {
                let signed_in = conf["url"].as_str().is_some_and(|u| crate::mcp_oauth::load(&crate::mcp_oauth::token_key(&name, u)).is_some());
                self.failed.lock().push(ServerStatus {
                    transport: transport_of(&conf).into(),
                    status: if matches!(&e, AppError::Api { kind, .. } if kind == "needs_auth") { "needs_auth" } else { "failed" }.into(),
                    error: Some(e.to_string()),
                    tools: vec![],
                    server_info: Value::Null,
                    logs,
                    disabled: false,
                    signed_in,
                    name,
                })
            }
            Err(_) => {}
        }
    }

    /// Reconnect server `name` alone (after a sign-in or sign-out), leaving
    /// the others running.
    pub async fn restart(&self, name: &str, disabled: &[String]) {
        let gen = self.generation.load(Ordering::SeqCst);
        let conf = read_config(&self.config_path)["mcpServers"][name].clone();
        if !conf.is_object() || disabled.iter().any(|d| d == name) || conf["disabled"].as_bool() == Some(true) {
            return;
        }
        self.failed.lock().retain(|s| s.name != name);
        let old = self.servers.write().await.remove(name);
        if let Some(old) = old {
            old.shutdown().await;
        }
        self.starting.lock().push((name.to_string(), transport_of(&conf).into()));
        self.changed.notify_one();
        let res = connect(name, &conf, self.changed.clone(), Logs::default()).await;
        self.starting.lock().retain(|(n, _)| n != name);
        self.register(gen, name.to_string(), conf, res).await;
        self.changed.notify_one();
    }

    pub async fn stop_all(&self) {
        let stopping: Vec<Arc<McpServer>> = self.servers.write().await.drain().map(|(_, s)| s).collect();
        // side by side: each may take its grace period to exit
        futures_util::future::join_all(stopping.iter().map(|s| s.shutdown())).await;
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
            signed_in: false,
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
        unique_api_names(out)
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
        // images are decoded, and a large one scaled down and encoded again
        // (a screenshot tool's): off the async runtime
        if r["content"].as_array().is_some_and(|c| c.iter().any(|b| b["type"] == "image")) {
            return tokio::task::spawn_blocking(move || convert_result(&r)).await.map_err(|e| AppError::msg(e.to_string()));
        }
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


/// One line of a stdio server's stdout: a message (see `route`) or log output.
fn handle_stdout_line(l: &str, pending: &Pending, logs: &Mutex<VecDeque<String>>, tools_changed: &UnboundedSender<()>, requests: &UnboundedSender<Value>) {
    match serde_json::from_str::<Value>(l) {
        Ok(v) => route(v, pending, tools_changed, requests),
        Err(_) => push_log(logs, format!("[stdout] {}", truncate_middle(l, MAX_LOG_LINE_BYTES))),
    }
}

/// A message from a server that sends everything on one stream (stdio, the
/// old HTTP+SSE transport): an answer goes to the request waiting for it, a
/// request for us to `requests`, a tool-list change to `tools_changed`.
fn route(v: Value, pending: &Pending, tools_changed: &UnboundedSender<()>, requests: &UnboundedSender<Value>) {
    if let Some(method) = v["method"].as_str() {
        if method == TOOLS_CHANGED {
            let _ = tools_changed.send(());
        } else if !v["id"].is_null() {
            let _ = requests.send(v);
        }
        return;
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

/// Our answer to request `v` that server `server` sent us.
async fn answer_request(server: &str, v: &Value) -> Value {
    let result = match v["method"].as_str().unwrap_or("") {
        "ping" => json!({}),
        // One connection serves every chat and code session at once, so there
        // is no single folder to offer.
        "roots/list" => json!({"roots": []}),
        "elicitation/create" => elicit::ask(server, &v["params"]).await,
        m => return json!({"jsonrpc": "2.0", "id": v["id"], "error": {"code": -32601, "message": format!("Method not supported: {m}")}}),
    };
    json!({"jsonrpc": "2.0", "id": v["id"], "result": result})
}

fn transport_of(conf: &Value) -> &'static str {
    if conf["url"].is_string() {
        "http"
    } else {
        "stdio"
    }
}

async fn connect(name: &str, conf: &Value, changed: Arc<Notify>, logs: Logs) -> Result<Arc<McpServer>, (AppError, Vec<String>)> {
    let state: State = Arc::new(Mutex::new(("starting".into(), None)));
    let (tools_changed, mut tools_changed_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let (requests, mut requests_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
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
            .redirect(crate::mcp_oauth::same_origin_redirects())
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| (AppError::Http(e), vec![]))?;
        let oauth = (!headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("authorization"))).then(|| {
            let key = crate::mcp_oauth::token_key(name, url);
            Box::new(OAuth { tokens: tokio::sync::Mutex::new(crate::mcp_oauth::load(&key)), key })
        });
        Transport::Http {
            client,
            url: url.to_string(),
            headers,
            session: Mutex::new(None),
            renewing: tokio::sync::Mutex::new(()),
            oauth,
            sse: Mutex::new(None),
        }
    } else {
        let command = conf["command"]
            .as_str()
            .ok_or_else(|| (AppError::msg("Missing \"command\" (or \"url\") in server config"), vec![]))?;
        let args: Vec<String> = conf["args"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        crate::shellenv::ready().await;
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
        // Its own process group, so the server and everything it starts can
        // be stopped together (on Windows: its process tree, see shutdown).
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd
            .spawn()
            .map_err(|e| (AppError::msg(format!("Could not start `{command}`: {e}")), vec![]))?;
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = CancellationToken::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        // writer: the only owner of stdin, which it closes (by dropping it)
        // once the last strong sender is gone
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });
        // stderr -> logs, read to the end: a server blocks on a full pipe
        {
            let logs = logs.clone();
            tokio::spawn(async move {
                let mut r = BufReader::new(stderr);
                loop {
                    match read_line_lossy(&mut r, MAX_LOG_LINE_BYTES).await {
                        Ok(Some((l, cut))) => push_log(&logs, if cut { format!("{l} […]") } else { l }),
                        Ok(None) => break,
                        Err(e) => {
                            push_log(&logs, format!("[reading the server's log failed: {e}]"));
                            break;
                        }
                    }
                }
            });
        }
        // stdout -> responses, and requests the server sends us
        {
            let pending = pending.clone();
            let logs = logs.clone();
            let state = state.clone();
            let closed = closed.clone();
            let tools_changed = tools_changed.clone();
            let requests = requests.clone();
            tokio::spawn(async move {
                let mut r = BufReader::new(stdout);
                let why = loop {
                    match read_line_lossy(&mut r, MAX_MESSAGE_BYTES).await {
                        Ok(Some((_, true))) => push_log(&logs, format!("[stdout] dropped a message over {} MB", MAX_MESSAGE_BYTES >> 20)),
                        Ok(Some((l, false))) => handle_stdout_line(&l, &pending, &logs, &tools_changed, &requests),
                        Ok(None) => break "the server process exited".to_string(),
                        Err(e) => break format!("reading the server's output failed: {e}"),
                    }
                };
                push_log(&logs, "[server process exited]".into());
                // Not "failed": a registered stdio server has a supervisor,
                // which restarts it or gives up, and only giving up is final.
                // (One that exits while connecting is never shown.) Set before
                // `closed`, which wakes the supervisor, so it reads this why.
                *state.lock() = ("reconnecting".into(), Some(why));
                // in this order: a request registered after `pending` was
                // emptied sees `closed` (see request_raw)
                closed.cancel();
                pending.lock().clear();
            });
        }
        Transport::Stdio {
            tx: Mutex::new(Some(tx)),
            child: Box::new(Mutex::new(Some(child))),
            pending,
            closed,
        }
    };
    let server = Arc::new(McpServer {
        name: name.to_string(),
        transport,
        next_id: AtomicI64::new(1),
        state,
        tools: Mutex::new(vec![]),
        info: Mutex::new(Value::Null),
        logs: logs.clone(),
        changed,
        tools_changed,
        requests,
    });
    // Answer what the server asks, from the handshake on, each request in a
    // task of its own: an elicitation waits for the user. Weak, so it ends
    // with the server.
    {
        let weak = Arc::downgrade(&server);
        let name = name.to_string();
        tokio::spawn(async move {
            while let Some(req) = requests_rx.recv().await {
                let (weak, name) = (weak.clone(), name.clone());
                tokio::spawn(async move {
                    let answer = answer_request(&name, &req).await;
                    if let Some(s) = weak.upgrade() {
                        s.send(answer).await;
                    }
                });
            }
        });
    }
    match server.initialize().await {
        Ok(()) => {
            let mut st = server.state.lock();
            // unless it has stopped again already
            if st.0 == "starting" {
                *st = ("connected".into(), None);
            }
            drop(st);
            // Fetch the tools again when the server says they changed. Weak:
            // this ends once the server is dropped (and its stdout closed).
            let weak = Arc::downgrade(&server);
            tokio::spawn(async move {
                while tools_changed_rx.recv().await.is_some() {
                    while tools_changed_rx.try_recv().is_ok() {}
                    let Some(s) = weak.upgrade() else { break };
                    match s.refresh_tools().await {
                        Ok(()) => s.changed.notify_one(),
                        Err(e) => push_log(&s.logs, format!("[fetching the changed tool list failed: {e}]")),
                    }
                }
            });
            Ok(server)
        }
        Err(e) => {
            server.shutdown().await;
            let l: Vec<String> = logs.lock().iter().cloned().collect();
            Err((e, l))
        }
    }
}

async fn is_current(servers: &Servers, name: &str, server: &Arc<McpServer>) -> bool {
    servers.read().await.get(name).is_some_and(|s| Arc::ptr_eq(s, server))
}

/// Keep a stdio server running: when its process exits, start it again after
/// 1, 2, 4… s, until it stays up or RESTART_TRIES tries in a row failed.
/// Ends once the server isn't the manager's any more (stopped, reloaded or
/// replaced), so a restart never races a reload.
async fn supervise(servers: Servers, name: String, conf: Value, mut server: Arc<McpServer>) {
    let mut tries = 0;
    loop {
        let Transport::Stdio { closed, .. } = &server.transport else { return };
        let closed = closed.clone();
        let up = Instant::now();
        closed.cancelled().await;
        if !is_current(&servers, &name, &server).await {
            return;
        }
        // reap it, and kill anything it left running
        server.shutdown().await;
        if up.elapsed() >= RESTART_RESET {
            tries = 0;
        }
        // kept here, not in the state: "failed" is shown only once given up
        let mut why = server.state.lock().1.clone().unwrap_or_default();
        let new = loop {
            if tries == RESTART_TRIES {
                *server.state.lock() = ("failed".into(), Some(format!("{why} (gave up after {RESTART_TRIES} restarts)")));
                server.changed.notify_one();
                return;
            }
            let wait = RESTART_FIRST_WAIT * 2u32.pow(tries);
            tries += 1;
            *server.state.lock() = ("reconnecting".into(), Some(format!("{why}; restarting in {wait:?} (try {tries} of {RESTART_TRIES})")));
            server.changed.notify_one();
            tokio::time::sleep(wait).await;
            if !is_current(&servers, &name, &server).await {
                return;
            }
            push_log(&server.logs, format!("[restarting: try {tries} of {RESTART_TRIES}]"));
            match connect(&name, &conf, server.changed.clone(), server.logs.clone()).await {
                Ok(new) => break new,
                Err((e, _)) => why = e.to_string(),
            }
        };
        let mut map = servers.write().await;
        if !map.get(&name).is_some_and(|s| Arc::ptr_eq(s, &server)) {
            drop(map);
            new.shutdown().await;
            return;
        }
        map.insert(name.clone(), new.clone());
        drop(map);
        server = new;
        server.changed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn tool_names() {
        assert_eq!(api_tool_name("file system", "read_file"), "file_system__read_file");
        let long = api_tool_name(&"s".repeat(40), &"t".repeat(40));
        assert!(long.len() <= 64);
    }

    fn tool(server: &str, name: &str) -> McpTool {
        McpTool {
            server: server.into(),
            name: name.into(),
            api_name: api_tool_name(server, name),
            title: None,
            description: String::new(),
            input_schema: json!({"type": "object"}),
            read_only: false,
        }
    }

    #[test]
    fn clashing_tool_names_are_made_unique() {
        let tools = vec![
            tool("my server", "files.read"),
            tool("my_server", "files_read"),
            tool("my_server", "files.read"),
            tool("my server", "files_read"),
            tool("other", "x"),
            tool(&"s".repeat(40), &"t".repeat(40)),
            tool(&"s".repeat(40), &format!("{}.", "t".repeat(39))),
        ];
        let out = unique_api_names(tools.clone());
        let names: HashSet<&str> = out.iter().map(|t| t.api_name.as_str()).collect();
        assert_eq!(names.len(), out.len(), "{names:?}");
        assert!(out.iter().all(|t| t.api_name.len() <= 64 && t.api_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')));
        let by = |s: &str, n: &str| out.iter().find(|t| t.server == s && t.name == n).unwrap().api_name.clone();
        // the tool that needed no renaming keeps its name; unclashing ones too
        assert_eq!(by("my_server", "files_read"), "my_server__files_read");
        assert_eq!(by("other", "x"), "other__x");
        assert!(by("my server", "files.read").starts_with("my_server__files_read_"));
        // stable, whatever order the servers come in
        let mut rev = tools.clone();
        rev.reverse();
        let again = unique_api_names(rev);
        for t in &out {
            assert!(again.iter().any(|u| u.server == t.server && u.name == t.name && u.api_name == t.api_name));
        }
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
    async fn lines_are_read_leniently_and_bounded() {
        // cp1254 "Şule", an over-long line, CRLF, and a last line without \n
        let mut data = b"C:\\Users\\\xdeule\n".to_vec();
        data.extend_from_slice(&[b'x'; 100]);
        data.extend_from_slice(b"\r\nok\r\nlast");
        let mut r = BufReader::new(&data[..]);
        let (l, cut) = read_line_lossy(&mut r, 50).await.unwrap().unwrap();
        assert_eq!((l.as_str(), cut), ("C:\\Users\\\u{fffd}ule", false));
        let (l, cut) = read_line_lossy(&mut r, 50).await.unwrap().unwrap();
        assert_eq!((l.len(), cut), (50, true));
        assert_eq!(read_line_lossy(&mut r, 50).await.unwrap(), Some(("ok".into(), false)));
        assert_eq!(read_line_lossy(&mut r, 50).await.unwrap(), Some(("last".into(), false)));
        assert_eq!(read_line_lossy(&mut r, 50).await.unwrap(), None);
        // tiny reads: CRLF split between them, a line exactly at the cap
        let data = b"abc\r\nxy\r\r\n12345\r\n".to_vec();
        let mut r = BufReader::with_capacity(4, &data[..]);
        let mut got = vec![];
        while let Some(l) = read_line_lossy(&mut r, 5).await.unwrap() {
            got.push(l);
        }
        assert_eq!(got, vec![("abc".into(), false), ("xy\r".into(), false), ("12345".into(), false)]);
    }

    #[test]
    fn config_with_a_byte_order_mark() {
        let dir = std::env::temp_dir().join(format!("mcp-bom-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("claude_desktop_config.json");
        let json = r#"{"mcpServers": {"fs": {"command": "npx"}}}"#;
        // Notepad: UTF-8 with BOM
        std::fs::write(&p, format!("\u{feff}{json}")).unwrap();
        assert_eq!(read_config(&p)["mcpServers"]["fs"]["command"], "npx");
        assert_eq!(read_config_for_update(&p).unwrap()["mcpServers"]["fs"]["command"], "npx");
        assert_eq!(config_problem(&p), None);
        // PowerShell 5 `>`: UTF-16 LE with BOM
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(json.encode_utf16().flat_map(|u| u.to_le_bytes()));
        std::fs::write(&p, utf16).unwrap();
        assert_eq!(read_config(&p)["mcpServers"]["fs"]["command"], "npx");
        // what Settings shows for a broken file
        std::fs::write(&p, "{\"mcpServers\": {").unwrap();
        let problem = config_problem(&p).unwrap();
        assert!(problem.contains("isn't valid JSON"), "{problem}");
        assert!(read_config_for_update(&p).is_err());
        // the editor gets the file's own text to fix, not the empty config it reads as
        assert_eq!(read_config(&p), json!({"mcpServers": {}}));
        assert_eq!(config_text(&p).as_deref(), Some("{\"mcpServers\": {"));
        std::fs::write(&p, "[]").unwrap();
        assert!(config_problem(&p).unwrap().contains("JSON object"));
        std::fs::write(&p, r#"{"mcpServers": []}"#).unwrap();
        assert!(config_problem(&p).is_some());
        std::fs::write(&p, [0xC3u8, 0x28]).unwrap();
        assert!(config_problem(&p).unwrap().contains("UTF-8"));
        assert_eq!(config_text(&p).as_deref(), Some("\u{fffd}("));
        assert_eq!(config_problem(&dir.join("missing.json")), None);
        assert_eq!(config_text(&dir.join("missing.json")), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ------------------------------------------------------------ stdio servers

    /// A small MCP server. argv: tag, comma-separated tool names. Tools:
    /// `slow` never answers, `cancelled` lists the ids of cancelled requests,
    /// `noisy` logs bytes that aren't UTF-8 (and more than a pipe holds),
    /// `exit` exits without answering, `image` answers a 1x1 PNG and a text,
    /// `grow` adds a tool `grown` and says its tool list changed;
    /// any other tool answers "tag:name:text".
    /// Env: MCP_TEST_PIDFILE (write its pid), MCP_TEST_EOFFILE (written on
    /// stdin EOF), MCP_TEST_STUBBORN=1 (keep running after stdin EOF),
    /// MCP_TEST_ONCE (a file: exit at once if it exists, else create it).
    const FAKE_SERVER: &str = r#"
import sys, json, os, time
tag = sys.argv[1] if len(sys.argv) > 1 else 'srv'
tools = sys.argv[2].split(',') if len(sys.argv) > 2 else ['echo']
once = os.environ.get('MCP_TEST_ONCE')
if once:
    if os.path.exists(once): sys.exit(1)
    open(once, 'w').close()
if os.environ.get('MCP_TEST_PIDFILE'):
    with open(os.environ['MCP_TEST_PIDFILE'], 'w') as f: f.write(str(os.getpid()))
out, err = sys.stdout.buffer, sys.stderr.buffer
def send(o):
    out.write((json.dumps(o) + '\n').encode()); out.flush()
def text(t): return {'content': [{'type': 'text', 'text': t}]}
cancelled = []
for raw in sys.stdin.buffer:
    m = json.loads(raw)
    meth = m.get('method')
    if meth == 'notifications/cancelled':
        cancelled.append(m['params']['requestId']); continue
    if meth is None or 'id' not in m: continue
    if meth == 'initialize':
        r = {'protocolVersion': '2025-06-18', 'capabilities': {'tools': {}}, 'serverInfo': {'name': tag, 'version': '1'}}
    elif meth == 'tools/list':
        r = {'tools': [{'name': t, 'inputSchema': {'type': 'object'}} for t in tools]}
    elif meth == 'tools/call':
        name = m['params']['name']
        if name == 'slow': continue
        if name == 'exit': sys.exit(0)
        if name == 'grow':
            tools.append('grown')
            send({'jsonrpc': '2.0', 'method': 'notifications/tools/list_changed'})
            r = text('grew')
        elif name == 'cancelled': r = text(json.dumps(cancelled))
        elif name == 'image':
            png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=='
            r = {'content': [{'type': 'image', 'mimeType': 'image/png', 'data': png}, {'type': 'text', 'text': 'shot'}]}
        elif name == 'noisy':
            err.write(b'C:\\Users\\\xdeule\\AppData\n'); err.flush()
            out.write(b'loading \xde\n'); out.flush()
            err.write(b'x' * 300000 + b'\n'); err.flush()
            r = text('done')
        else: r = text(tag + ':' + name + ':' + m['params']['arguments'].get('text', ''))
    else:
        r = {}
    send({'jsonrpc': '2.0', 'id': m['id'], 'result': r})
if os.environ.get('MCP_TEST_EOFFILE'):
    with open(os.environ['MCP_TEST_EOFFILE'], 'w') as f: f.write('eof')
while os.environ.get('MCP_TEST_STUBBORN') == '1':
    time.sleep(1)
"#;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mcp-{tag}-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A manager for `servers` (name → config), started.
    async fn manager(dir: &Path, servers: Value) -> McpManager {
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, json!({"mcpServers": servers}).to_string()).unwrap();
        let m = McpManager::new(cfg);
        m.start_all(&[]).await;
        m
    }

    /// A Python 3 that really runs, for the fake servers. On Windows
    /// `python3` can be the Microsoft Store stub: found on PATH, but it only
    /// prints an install hint.
    fn python() -> Option<&'static str> {
        static PY: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
        *PY.get_or_init(|| {
            let found = ["python3", "python"].into_iter().find(|p| {
                std::process::Command::new(p).args(["-c", "import sys; sys.exit(sys.version_info < (3,))"]).output().is_ok_and(|o| o.status.success())
            });
            if found.is_none() {
                eprintln!("SKIPPED: the stdio MCP tests need Python 3");
            }
            found
        })
    }

    fn fake(tag: &str, tools: &str) -> Value {
        json!({"command": python().unwrap(), "args": ["-c", FAKE_SERVER, tag, tools]})
    }

    #[cfg(unix)]
    fn alive(pid: i32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // a zombie waits to be reaped, it no longer runs
            Ok(stat) => stat.rsplit(')').next().is_some_and(|s| !s.trim_start().starts_with('Z')),
            // SAFETY: signal 0 only checks that the process exists
            Err(_) if Path::new("/proc/self").exists() => false,
            Err(_) => unsafe { libc::kill(pid, 0) == 0 },
        }
    }

    #[tokio::test]
    async fn stdio_roundtrip_with_fake_server() {
        // A tiny MCP server implemented in sh + python-free awk is fiddly; use
        // a Python one-liner only when Python exists.
        let Some(py) = python() else { return };
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
        let dir = temp_dir("test");
        let m = manager(&dir, json!({"fake": {"command": py, "args": ["-c", script]}})).await;
        let st = m.statuses().await;
        assert_eq!(st[0].status, "connected", "{:?}", st[0]);
        let tools = m.tools().await;
        assert_eq!(tools[0].api_name, "fake__echo");
        let (blocks, _) = m.call("fake__echo", &json!({"text": "hi"})).await.unwrap();
        assert_eq!(blocks[0]["text"], "hi");
        m.stop_all().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn output_that_isnt_utf8_doesnt_break_the_connection() {
        if python().is_none() {
            return;
        }
        let dir = temp_dir("noisy");
        let m = manager(&dir, json!({"x": fake("x", "noisy,echo,image")})).await;
        assert_eq!(m.statuses().await[0].status, "connected");
        // an answer with an image (converted on the blocking pool)
        let (b, err) = m.call("x__image", &json!({})).await.unwrap();
        assert!(!err);
        assert_eq!((b[0]["type"].as_str(), b[0]["source"]["media_type"].as_str(), b[1]["text"].as_str()), (Some("image"), Some("image/png"), Some("shot")), "{b:?}");
        // stderr must keep draining (300 KB fill any pipe), stdout keep reading
        let (b, _) = tokio::time::timeout(Duration::from_secs(20), m.call("x__noisy", &json!({}))).await.expect("server blocked").unwrap();
        assert_eq!(b[0]["text"], "done");
        let (b, _) = tokio::time::timeout(Duration::from_secs(20), m.call("x__echo", &json!({"text": "still here"}))).await.unwrap().unwrap();
        assert_eq!(b[0]["text"], "x:echo:still here");
        let logs = m.statuses().await[0].logs.clone();
        assert!(logs.iter().any(|l| l.contains("C:\\Users\\\u{fffd}ule")), "{logs:?}");
        assert!(logs.iter().any(|l| l.starts_with("[stdout] loading")), "{logs:?}");
        m.stop_all().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Wait up to 20 s for `f` to hold.
    async fn eventually<F: std::future::Future<Output = bool>>(what: &str, mut f: impl FnMut() -> F) {
        for _ in 0..400 {
            if f().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting until {what}");
    }

    fn pid(file: &Path) -> String {
        std::fs::read_to_string(file).unwrap_or_default().trim().to_string()
    }

    #[tokio::test]
    async fn a_server_that_exits_fails_fast_and_is_restarted() {
        if python().is_none() {
            return;
        }
        let dir = temp_dir("exit");
        let pidfile = dir.join("pid");
        let mut conf = fake("x", "exit,echo");
        conf["env"] = json!({"MCP_TEST_PIDFILE": pidfile});
        let m = manager(&dir, json!({"x": conf})).await;
        let first = m.servers.read().await.get("x").cloned().unwrap();
        let first_pid = pid(&pidfile);
        let r = tokio::time::timeout(Duration::from_secs(10), m.call("x__exit", &json!({}))).await.expect("hung");
        assert!(r.is_err());
        // asking the exited one anything fails right away
        let r = tokio::time::timeout(Duration::from_secs(2), first.request("tools/list", json!({}), Duration::from_secs(600))).await.expect("waited for the timeout");
        assert!(r.unwrap_err().to_string().contains("not running"));
        // a new process takes its place, and answers
        let (mr, fr) = (&m, &first);
        eventually("the server is back", || async move {
            mr.statuses().await[0].status == "connected" && !mr.servers.read().await.get("x").is_some_and(|s| Arc::ptr_eq(s, fr))
        })
        .await;
        assert_ne!(pid(&pidfile), first_pid);
        let (b, _) = m.call("x__echo", &json!({"text": "back"})).await.unwrap();
        assert_eq!(b[0]["text"], "x:echo:back");
        assert!(m.statuses().await[0].logs.iter().any(|l| l.contains("restarting: try 1 of 5")));
        #[cfg(unix)]
        assert!(!alive(first_pid.parse().unwrap()));
        // nothing holds on to the old one
        let old = Arc::downgrade(&first);
        drop(first);
        let old = &old;
        eventually("the old server is released", || async move { old.upgrade().is_none() }).await;
        m.stop_all().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_server_that_keeps_exiting_is_given_up() {
        if python().is_none() {
            return;
        }
        let dir = temp_dir("giveup");
        // the first process starts; every later one exits at once
        let mut conf = fake("x", "exit");
        conf["env"] = json!({"MCP_TEST_ONCE": dir.join("started")});
        let m = manager(&dir, json!({"x": conf})).await;
        assert_eq!(m.statuses().await[0].status, "connected");
        let server = m.servers.read().await["x"].clone();
        {
            // The supervisor can't take over while this lock is held (as it
            // couldn't on Linux while it reaped the process): what the exit
            // alone leaves must not read as given up.
            let _held = m.servers.write().await;
            let _ = server.request("tools/call", json!({"name": "exit", "arguments": {}}), Duration::from_secs(10)).await;
            assert_eq!(server.state.lock().0, "reconnecting");
        }
        let mr = &m;
        eventually("it is given up", || async move { mr.statuses().await[0].status == "failed" }).await;
        let st = m.statuses().await;
        assert!(st[0].error.as_deref().unwrap_or("").contains("gave up after 5 restarts"), "{:?}", st[0]);
        assert!(st[0].logs.iter().any(|l| l.contains("try 5 of 5")), "{:?}", st[0].logs);
        assert!(!st[0].logs.iter().any(|l| l.contains("try 6")));
        assert!(m.tools().await.is_empty());
        // the UI was told
        tokio::time::timeout(Duration::from_secs(1), m.changed()).await.expect("no change announced");
        m.stop_all().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_changed_tool_list_is_fetched_again() {
        if python().is_none() {
            return;
        }
        let dir = temp_dir("grow");
        let m = manager(&dir, json!({"x": fake("x", "grow")})).await;
        let names = |tools: Vec<McpTool>| tools.into_iter().map(|t| t.api_name).collect::<Vec<_>>();
        assert_eq!(names(m.tools().await), ["x__grow"]);
        let (b, _) = m.call("x__grow", &json!({})).await.unwrap();
        assert_eq!(b[0]["text"], "grew");
        tokio::time::timeout(Duration::from_secs(10), m.changed()).await.expect("no change announced");
        assert_eq!(names(m.tools().await), ["x__grow", "x__grown"]);
        // stopped, nothing (supervisor, tool-list task) keeps the server
        let server = Arc::downgrade(&m.servers.read().await["x"]);
        m.stop_all().await;
        let server = &server;
        eventually("the server is released", || async move { server.upgrade().is_none() }).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_closes_stdin_then_kills_the_whole_group() {
        let Some(py) = python() else { return };
        let dir = temp_dir("group");
        let pidfile = dir.join("pid");
        let eoffile = dir.join("eof");
        // like `npx`: a wrapper that runs the real server as its child, which
        // here also ignores the end of its input
        let conf = json!({
            "command": "sh",
            "args": ["-c", format!("{py} -c \"$0\" w echo; exit 0"), FAKE_SERVER],
            "env": {"MCP_TEST_PIDFILE": pidfile, "MCP_TEST_EOFFILE": eoffile, "MCP_TEST_STUBBORN": "1"},
        });
        let m = manager(&dir, json!({"w": conf})).await;
        assert_eq!(m.statuses().await[0].status, "connected", "{:?}", m.statuses().await[0]);
        let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert!(alive(pid));
        m.stop_all().await;
        // asked politely first: the server saw its stdin close …
        let mut saw_eof = false;
        for _ in 0..50 {
            saw_eof = eoffile.exists();
            if saw_eof {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(saw_eof, "stdin was never closed");
        // … and, as it didn't exit, it was killed although it isn't our child
        let mut gone = false;
        for _ in 0..50 {
            gone = !alive(pid);
            if gone {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(gone, "the real server (pid {pid}) survived");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_stopped_call_is_cancelled_and_forgotten() {
        if python().is_none() {
            return;
        }
        let dir = temp_dir("cancel");
        let m = manager(&dir, json!({"x": fake("x", "slow,cancelled")})).await;
        // the user presses Stop: the call's future is dropped
        assert!(tokio::time::timeout(Duration::from_millis(300), m.call("x__slow", &json!({}))).await.is_err());
        let server = m.servers.read().await.get("x").cloned().unwrap();
        let Transport::Stdio { pending, .. } = &server.transport else { unreachable!() };
        assert!(pending.lock().is_empty(), "the stopped call is still pending");
        let (b, _) = m.call("x__cancelled", &json!({})).await.unwrap();
        let ids: Vec<i64> = serde_json::from_str(b[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(ids.len(), 1, "{ids:?}");
        m.stop_all().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn clashing_names_route_to_the_right_tool() {
        if python().is_none() {
            return;
        }
        let dir = temp_dir("names");
        let m = manager(&dir, json!({"my server": fake("A", "files.read,files_read"), "my_server": fake("B", "files.read,files_read")})).await;
        let tools = m.tools().await;
        assert_eq!(tools.len(), 4);
        let names: HashSet<String> = tools.iter().map(|t| t.api_name.clone()).collect();
        assert_eq!(names.len(), 4, "{names:?}");
        for t in &tools {
            let (b, _) = m.call(&t.api_name, &json!({"text": "q"})).await.unwrap();
            let tag = if t.server == "my server" { "A" } else { "B" };
            assert_eq!(b[0]["text"], format!("{tag}:{}:q", t.name));
        }
        m.stop_all().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    // ------------------------------------------------------------ HTTP servers

    struct HttpReq {
        session: Option<String>,
        body: Value,
    }

    /// A minimal HTTP/1.1 server on 127.0.0.1; `handle` answers each request
    /// with (status, session id to set, content type, body).
    async fn http_server(handle: impl Fn(HttpReq) -> (u16, Option<String>, &'static str, String) + Send + Sync + 'static) -> String {
        std::env::set_var("NO_PROXY", "127.0.0.1");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = Arc::new(handle);
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let handle = handle.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let head_end = loop {
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
                    let header = |name: &str| head.lines().find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_string()));
                    let len: usize = header("content-length:").and_then(|v| v.parse().ok()).unwrap_or(0);
                    while buf.len() < head_end + len {
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let body = serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or(Value::Null);
                    let (status, set_session, ctype, text) = handle(HttpReq { session: header("mcp-session-id:"), body });
                    let mut resp = format!("HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n", text.len());
                    if let Some(sid) = set_session {
                        resp.push_str(&format!("mcp-session-id: {sid}\r\n"));
                    }
                    resp.push_str("\r\n");
                    resp.push_str(&text);
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut s, resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}/mcp")
    }

    /// What a test HTTP server answers for `method` (JSON-RPC result).
    fn http_result(method: &str, body: &Value) -> Value {
        match method {
            "initialize" => json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {"tools": {}}, "serverInfo": {"name": "web"}}),
            "tools/list" => json!({"tools": [{"name": "echo", "inputSchema": {"type": "object"}}]}),
            "tools/call" => json!({"content": [{"type": "text", "text": body["params"]["arguments"]["text"]}]}),
            _ => json!({}),
        }
    }

    #[tokio::test]
    async fn sse_answers_with_crlf_line_endings() {
        // the official Python SDK (sse-starlette) ends lines with CRLF and
        // often sends a notification before the answer
        let url = http_server(|req| {
            let method = req.body["method"].as_str().unwrap_or("").to_string();
            if req.body["id"].is_null() {
                return (202, None, "application/json", String::new());
            }
            let log = json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {"level": "info", "data": "working"}});
            let answer = json!({"jsonrpc": "2.0", "id": req.body["id"], "result": http_result(&method, &req.body)});
            let sse = format!("event: message\r\ndata: {log}\r\n\r\nevent: message\r\ndata: {answer}\r\n\r\n");
            (200, Some("s1".into()), "text/event-stream", sse)
        })
        .await;
        let s = connect("web", &json!({"url": url}), Default::default(), Default::default()).await.map_err(|(e, _)| e).unwrap();
        assert_eq!(s.tools.lock().len(), 1);
        let r = s.request("tools/call", json!({"name": "echo", "arguments": {"text": "hi"}}), Duration::from_secs(10)).await.unwrap();
        assert_eq!(r["content"][0]["text"], "hi");
    }

    #[tokio::test]
    async fn an_expired_http_session_is_renewed() {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        let sessions = Arc::new(AtomicUsize::new(0));
        let expired = Arc::new(AtomicBool::new(false));
        let (s2, e2) = (sessions.clone(), expired.clone());
        let url = http_server(move |req| {
            let method = req.body["method"].as_str().unwrap_or("").to_string();
            if method == "initialize" {
                assert!(req.session.is_none(), "a new session starts without the old id");
                let n = s2.fetch_add(1, Ordering::SeqCst) + 1;
                e2.store(false, Ordering::SeqCst);
                let answer = json!({"jsonrpc": "2.0", "id": req.body["id"], "result": http_result(&method, &req.body)});
                return (200, Some(format!("s{n}")), "application/json", answer.to_string());
            }
            let current = format!("s{}", s2.load(Ordering::SeqCst));
            if e2.load(Ordering::SeqCst) || req.session.as_deref() != Some(current.as_str()) {
                return (404, None, "application/json", r#"{"error":"session not found"}"#.into());
            }
            if req.body["id"].is_null() {
                return (202, None, "application/json", String::new());
            }
            (200, None, "application/json", json!({"jsonrpc": "2.0", "id": req.body["id"], "result": http_result(&method, &req.body)}).to_string())
        })
        .await;
        let s = connect("web", &json!({"url": url}), Default::default(), Default::default()).await.map_err(|(e, _)| e).unwrap();
        assert_eq!(sessions.load(Ordering::SeqCst), 1);
        // the server restarts and forgets the session
        expired.store(true, Ordering::SeqCst);
        let r = s.request("tools/call", json!({"name": "echo", "arguments": {"text": "again"}}), Duration::from_secs(10)).await.unwrap();
        assert_eq!(r["content"][0]["text"], "again");
        assert_eq!(sessions.load(Ordering::SeqCst), 2);
        let Transport::Http { session, .. } = &s.transport else { unreachable!() };
        assert_eq!(session.lock().as_deref(), Some("s2"));
    }
}
