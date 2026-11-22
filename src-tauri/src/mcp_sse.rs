//! The HTTP+SSE transport of MCP 2024-11-05, for servers that turn down the
//! streamable HTTP `initialize` POST (the spec's backwards-compatibility
//! procedure): a GET opens an event stream whose first event names the
//! endpoint messages are POSTed to, and everything the server sends comes on
//! that stream.

use super::*;
use crate::anthropic::{SseEvent, SseParser};

/// One session's event stream.
pub(super) struct Stream {
    /// Where messages are POSTed; its URL holds the session.
    pub(super) endpoint: String,
    /// Our requests, answered on the stream.
    pub(super) pending: Pending,
    /// Cancelled when the stream ends: what waits there won't be answered.
    pub(super) closed: CancellationToken,
    reader: tokio::task::AbortHandle,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// The endpoint an `endpoint` event names, which may be relative to the
/// server's URL. It must be on the server's origin: messages, and their
/// tokens, go there.
fn endpoint(server: &str, named: &str) -> AppResult<String> {
    let base = url::Url::parse(server).map_err(|e| AppError::msg(format!("{server}: {e}")))?;
    let u = base
        .join(named.trim())
        .map_err(|e| AppError::msg(format!("The MCP server named an endpoint that isn't a URL ({named}): {e}")))?;
    if u.origin() != base.origin() {
        return Err(AppError::msg(format!("The MCP server named an endpoint on another origin ({u}), so it was not used.")));
    }
    Ok(u.into())
}

/// Read `body` up to its `endpoint` event: what that names, and the events
/// after it. `None`: the stream ended first.
async fn named_endpoint<S, B>(body: &mut S, buf: &mut Vec<u8>, parser: &mut SseParser) -> AppResult<Option<(String, Vec<SseEvent>)>>
where
    S: futures_util::Stream<Item = reqwest::Result<B>> + Unpin,
    B: AsRef<[u8]>,
{
    loop {
        let mut events = parser.feed(buf).into_iter();
        if let Some(ev) = events.by_ref().find(|ev| ev.event == "endpoint") {
            return Ok(Some((ev.data, events.collect())));
        }
        match body.next().await {
            Some(chunk) => buf.extend_from_slice(chunk?.as_ref()),
            None => return Ok(None),
        }
    }
}

impl McpServer {
    /// A new session: open the event stream and learn its endpoint. `None`:
    /// the server wants a sign-in (HTTP 401).
    async fn open_sse(&self, bearer: Option<&str>, timeout: Duration) -> AppResult<Option<Arc<Stream>>> {
        let Transport::Http { client, url, headers, oauth, .. } = &self.transport else {
            return Err(self.not_running());
        };
        let mut req = client.get(url).header("Accept", "text/event-stream");
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        // not the request's own timeout, which would end the stream as well
        let resp = tokio::time::timeout(timeout, req.send())
            .await
            .map_err(|_| AppError::msg(format!("MCP server '{}' timed out opening its event stream", self.name)))??;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED && oauth.is_some() {
            return Ok(None);
        }
        if !status.is_success() {
            let t = resp.text().await.unwrap_or_default();
            return Err(AppError::msg(format!("MCP HTTP {}: {}", status.as_u16(), truncate_middle(&t, 300))));
        }
        let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
        if !ctype.contains("text/event-stream") {
            return Err(AppError::msg(format!(
                "MCP server '{}' speaks neither the streamable HTTP nor the HTTP+SSE transport (its GET answered {ctype}).",
                self.name
            )));
        }
        let mut body = resp.bytes_stream();
        let (mut buf, mut parser) = (Vec::new(), SseParser::default());
        let named = tokio::time::timeout(timeout, named_endpoint(&mut body, &mut buf, &mut parser))
            .await
            .map_err(|_| AppError::msg(format!("MCP server '{}' named no endpoint on its event stream within {timeout:?}", self.name)))??;
        let Some((named, early)) = named else {
            return Err(AppError::msg(format!("MCP server '{}' closed its event stream before naming an endpoint", self.name)));
        };
        let endpoint = endpoint(url, &named)?;
        let (pending, closed) = (Pending::default(), CancellationToken::new());
        let reader = {
            let (pending, closed, logs) = (pending.clone(), closed.clone(), self.logs.clone());
            let (tools_changed, requests) = (self.tools_changed.clone(), self.requests.clone());
            tokio::spawn(async move {
                let deliver = |events: Vec<SseEvent>| {
                    let messages = events.into_iter().filter(|ev| matches!(ev.event.as_str(), "" | "message"));
                    for v in messages.filter_map(|ev| serde_json::from_str::<Value>(&ev.data).ok()) {
                        route(v, &pending, &tools_changed, &requests);
                    }
                };
                deliver(early);
                let why = loop {
                    match body.next().await {
                        Some(Ok(chunk)) => {
                            buf.extend_from_slice(&chunk);
                            deliver(parser.feed(&mut buf));
                        }
                        Some(Err(e)) => break format!("its event stream failed: {e}"),
                        None => {
                            deliver(parser.finish(&mut buf));
                            break "it closed its event stream".to_string();
                        }
                    }
                };
                push_log(&logs, format!("[the session ended: {why}]"));
                // in this order, as for a stdio server (see request_sse)
                closed.cancel();
                pending.lock().clear();
            })
            .abort_handle()
        };
        Ok(Some(Arc::new(Stream { endpoint, pending, closed, reader })))
    }

    /// `request_raw` over this transport, on `stream` (`None`, or an
    /// `initialize`: on a new one). The message is POSTed to the endpoint,
    /// and its answer comes on the stream.
    pub(super) async fn request_sse(
        &self,
        stream: Option<Arc<Stream>>,
        method: &str,
        msg: &Value,
        bearer: Option<String>,
        timeout: Duration,
        flight: &mut InFlight<'_>,
    ) -> AppResult<Reply> {
        let Transport::Http { client, headers, session, sse, oauth, .. } = &self.transport else {
            return Err(self.not_running());
        };
        let until = tokio::time::Instant::now() + timeout;
        let stream = match stream {
            Some(s) if method != "initialize" => s,
            _ => {
                let Some(s) = self.open_sse(bearer.as_deref(), timeout).await? else {
                    flight.finished = true;
                    return Ok(Reply::Unauthorized(bearer));
                };
                // the endpoint stands for the session (see renew_session)
                *session.lock() = Some(s.endpoint.clone());
                *sse.lock() = Some(s.clone());
                s
            }
        };
        let id = flight.id;
        let (otx, rx) = oneshot::channel();
        stream.pending.lock().insert(id, otx);
        // The stream ended (the server restarted, the network dropped): a new
        // session, as when a streamable server forgets one. `closed` is set
        // before the reader drops what is pending, so this catches the rest.
        if stream.closed.is_cancelled() {
            stream.pending.lock().remove(&id);
            flight.finished = true;
            return Ok(Reply::SessionExpired(stream.endpoint.clone()));
        }
        let resp = match http_post(client, &stream.endpoint, headers, bearer.as_deref(), None, msg.to_string(), timeout).send().await {
            Ok(r) => r,
            Err(e) => {
                // one that timed out is withdrawn by the guard
                if !e.is_timeout() {
                    stream.pending.lock().remove(&id);
                    flight.finished = true;
                }
                return Err(e.into());
            }
        };
        let status = resp.status();
        if !status.is_success() {
            stream.pending.lock().remove(&id);
            flight.finished = true;
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(Reply::SessionExpired(stream.endpoint.clone()));
            }
            if status == reqwest::StatusCode::UNAUTHORIZED && oauth.is_some() {
                return Ok(Reply::Unauthorized(bearer));
            }
            let t = resp.text().await.unwrap_or_default();
            return Err(AppError::msg(format!("MCP HTTP {}: {}", status.as_u16(), truncate_middle(&t, 300))));
        }
        self.answer(rx, method, until, flight).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn the_endpoint_stays_on_the_servers_origin() {
        let base = "http://127.0.0.1:8080/sse";
        assert_eq!(endpoint(base, "/messages?session_id=a1").unwrap(), "http://127.0.0.1:8080/messages?session_id=a1");
        assert_eq!(endpoint(base, " messages/?s=1\n").unwrap(), "http://127.0.0.1:8080/messages/?s=1");
        assert_eq!(endpoint(base, "http://127.0.0.1:8080/m").unwrap(), "http://127.0.0.1:8080/m");
        for elsewhere in ["http://127.0.0.1:9090/m", "https://127.0.0.1:8080/m", "http://evil.example/m", "//evil.example/m", "data:text/plain,x"] {
            assert!(endpoint(base, elsewhere).unwrap_err().to_string().contains("another origin"), "{elsewhere}");
        }
    }

    /// What the fake server got from the client and keeps between requests.
    #[derive(Default)]
    struct Seen {
        /// session → its open stream (dropping the sender closes it)
        streams: HashMap<String, UnboundedSender<String>>,
        sessions: usize,
        /// GETs that opened a stream
        gets: usize,
        cancelled: Vec<Value>,
        /// our elicitation request's id → the tools/call waiting for its answer
        asking: HashMap<String, Value>,
    }

    type Shared = Arc<Mutex<Seen>>;

    /// Read one HTTP/1.1 request: (method, path, body).
    async fn read_request(s: &mut TcpStream) -> Option<(String, String, Value)> {
        let (mut buf, mut chunk) = (Vec::new(), [0u8; 8192]);
        let head_end = loop {
            match s.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut first = head.split_whitespace();
        let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
        let len: usize = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
        while buf.len() < head_end + len {
            match s.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        Some((method, path, serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or(Value::Null)))
    }

    async fn write(s: &mut TcpStream, text: &str) -> bool {
        tokio::io::AsyncWriteExt::write_all(s, text.as_bytes()).await.is_ok()
    }

    fn event(v: &Value) -> String {
        format!("event: message\r\ndata: {v}\r\n\r\n")
    }

    /// The answer to the client's request `m` on session `sid`, if one is due now.
    fn answer(seen: &mut Seen, sid: &str, m: &Value) -> Option<Value> {
        let text = |t: &str| json!({"content": [{"type": "text", "text": t}]});
        let result = match m["method"].as_str().unwrap_or("") {
            "initialize" => json!({"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "old"}}),
            "tools/list" => {
                let tools = ["echo", "slow", "drop", "ask"].map(|n| json!({"name": n, "inputSchema": {"type": "object"}}));
                json!({ "tools": tools })
            }
            "tools/call" => match m["params"]["name"].as_str().unwrap_or("") {
                "slow" => return None,
                // the server restarts: the session and its stream are gone
                "drop" => {
                    seen.streams.remove(sid);
                    return None;
                }
                "ask" => {
                    let ask = format!("e{}", m["id"]);
                    let req = json!({"jsonrpc": "2.0", "id": ask, "method": "elicitation/create", "params": {
                        "message": "Which name?",
                        "requestedSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]},
                    }});
                    let _ = seen.streams[sid].send(event(&req));
                    seen.asking.insert(ask, m.clone());
                    return None;
                }
                _ => text(&format!("{sid}:{}", m["params"]["arguments"]["text"].as_str().unwrap_or(""))),
            },
            _ => json!({}),
        };
        Some(json!({"jsonrpc": "2.0", "id": m["id"], "result": result}))
    }

    async fn serve(mut s: TcpStream, seen: Shared, named: Option<&'static str>) {
        let Some((method, path, body)) = read_request(&mut s).await else { return };
        match (method.as_str(), path.split('?').next().unwrap_or("")) {
            // not a streamable HTTP server
            ("POST", "/sse") => {
                write(&mut s, "HTTP/1.1 405 Method Not Allowed\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            }
            ("GET", "/sse") => {
                let (tx, mut rx) = unbounded_channel::<String>();
                let sid = {
                    let mut seen = seen.lock();
                    seen.sessions += 1;
                    seen.gets += 1;
                    let sid = format!("s{}", seen.sessions);
                    seen.streams.insert(sid.clone(), tx);
                    sid
                };
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n";
                let first = format!("event: endpoint\r\ndata: {}\r\n\r\n", named.map(String::from).unwrap_or(format!("/messages?session={sid}")));
                if !write(&mut s, &format!("{head}{first}")).await {
                    return;
                }
                while let Some(ev) = rx.recv().await {
                    if !write(&mut s, &ev).await {
                        return;
                    }
                }
            }
            ("POST", "/messages") => {
                let sid = path.split("session=").nth(1).unwrap_or("").to_string();
                let status = {
                    let mut seen = seen.lock();
                    match seen.streams.get(&sid).cloned() {
                        None => "404 Not Found",
                        Some(stream) => {
                            if body["method"] == "notifications/cancelled" {
                                seen.cancelled.push(body["params"]["requestId"].clone());
                            } else if body["method"].is_string() && !body["id"].is_null() {
                                if let Some(a) = answer(&mut seen, &sid, &body) {
                                    let _ = stream.send(event(&a));
                                }
                            } else if let Some(call) = body["id"].as_str().and_then(|id| seen.asking.remove(id)) {
                                // our answer to the elicitation finishes the tool call
                                let a = json!({"jsonrpc": "2.0", "id": call["id"], "result": {"content": [{"type": "text", "text": body["result"].to_string()}]}});
                                let _ = stream.send(event(&a));
                            }
                            "202 Accepted"
                        }
                    }
                };
                write(&mut s, &format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")).await;
            }
            _ => {
                write(&mut s, "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            }
        }
    }

    /// An HTTP+SSE server (MCP 2024-11-05) on 127.0.0.1: POST /sse gets 405,
    /// GET /sse opens a stream naming /messages?session=sN (or `named`), and
    /// each message POSTed there is answered on its stream. Tools: `echo`
    /// answers "sN:text", `slow` never answers, `drop` ends the session, and
    /// `ask` asks for a name (elicitation) and answers with what it got.
    async fn old_server(named: Option<&'static str>) -> (String, Shared) {
        std::env::set_var("NO_PROXY", "127.0.0.1");
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/sse", l.local_addr().unwrap());
        let seen = Shared::default();
        let shared = seen.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = l.accept().await {
                tokio::spawn(serve(s, shared.clone(), named));
            }
        });
        (url, seen)
    }

    async fn call(s: &McpServer, tool: &str, text: &str) -> AppResult<Value> {
        let r = s.request("tools/call", json!({"name": tool, "arguments": {"text": text}}), Duration::from_secs(10)).await?;
        Ok(r["content"][0]["text"].clone())
    }

    #[tokio::test]
    async fn a_server_on_the_old_transport_is_used_through_its_event_stream() {
        let (url, seen) = old_server(None).await;
        let s = connect("old", &json!({"url": url}), Default::default(), Default::default()).await.map_err(|(e, _)| e).unwrap();
        assert_eq!(s.info.lock()["serverInfo"]["name"], "old");
        assert_eq!(s.tools.lock().len(), 4);
        assert_eq!(call(&s, "echo", "hi").await.unwrap(), "s1:hi");
        assert!(s.logs.lock().iter().any(|l| l.contains("HTTP 405 to initialize")));

        // stopped: the request is forgotten, and the server told
        assert!(tokio::time::timeout(Duration::from_millis(300), call(&s, "slow", "")).await.is_err());
        let Transport::Http { sse, .. } = &s.transport else { unreachable!() };
        assert!(sse.lock().as_ref().unwrap().pending.lock().is_empty());
        for _ in 0..200 {
            if !seen.lock().cancelled.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(seen.lock().cancelled.len(), 1);

        // the server restarts: the next request starts a new session
        assert!(call(&s, "drop", "").await.is_err());
        assert_eq!(call(&s, "echo", "again").await.unwrap(), "s2:again");
        assert_eq!(seen.lock().gets, 2);

        // the server asks the user something in the middle of a tool call
        let asked = tokio::spawn(async move { call(&s, "ask", "").await });
        let id = loop {
            if let Some(id) = elicit::waiting_for("old") {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        elicit::respond(&id, "accept", json!({"name": "Ada"})).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(10), asked).await.unwrap().unwrap().unwrap();
        assert_eq!(serde_json::from_str::<Value>(got.as_str().unwrap()).unwrap(), json!({"action": "accept", "content": {"name": "Ada"}}));
    }

    #[tokio::test]
    async fn an_endpoint_on_another_origin_is_refused() {
        let (url, _) = old_server(Some("http://evil.example/messages")).await;
        let e = connect("old", &json!({"url": url}), Default::default(), Default::default()).await.map(|_| ()).unwrap_err().0;
        assert!(e.to_string().contains("another origin"), "{e}");
    }

    #[tokio::test]
    async fn a_redirect_to_another_origin_is_not_followed() {
        // what the other origin is sent: the configured headers would go with it
        let elsewhere = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let to = format!("http://{}/x", elsewhere.local_addr().unwrap());
        let reached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r = reached.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = elsewhere.accept().await {
                r.fetch_add(1, Ordering::SeqCst);
                let _ = read_request(&mut s).await;
                write(&mut s, "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            }
        });
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", l.local_addr().unwrap());
        std::env::set_var("NO_PROXY", "127.0.0.1");
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let to = to.clone();
                tokio::spawn(async move {
                    if read_request(&mut s).await.is_some() {
                        write(&mut s, &format!("HTTP/1.1 307 Temporary Redirect\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")).await;
                    }
                });
            }
        });
        let conf = json!({"url": url, "headers": {"X-Api-Key": "secret"}});
        assert!(connect("moved", &conf, Default::default(), Default::default()).await.is_err());
        assert_eq!(reached.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_server_that_turns_down_both_transports_says_so() {
        // a URL that is wrong for either transport: the GET's answer is reported
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/nothing", l.local_addr().unwrap());
        std::env::set_var("NO_PROXY", "127.0.0.1");
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    if read_request(&mut s).await.is_some() {
                        write(&mut s, "HTTP/1.1 404 Not Found\r\ncontent-length: 9\r\nconnection: close\r\n\r\nnot found").await;
                    }
                });
            }
        });
        let e = connect("gone", &json!({"url": url}), Default::default(), Default::default()).await.map(|_| ()).unwrap_err().0;
        assert_eq!(e.to_string(), "MCP HTTP 404: not found");
    }
}
