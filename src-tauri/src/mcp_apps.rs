//! MCP Apps (the `io.modelcontextprotocol/ui` extension, spec 2026-01-26): a
//! tool that names a `ui://` resource in `_meta.ui.resourceUri` gets that
//! HTML page shown with its result. The page is untrusted: it runs in the
//! artifact sandbox (an opaque origin, no IPC), under a CSP built from what
//! its resource declares and this module checks (nothing else by default),
//! and talks to the app only over postMessage (ui/js/mcp_apps.js). Its tool
//! calls go to its own server, only to tools that server lets apps call, and
//! through the same allow/ask/deny policy as the model's, rate limited.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

pub const MIME: &str = "text/html;profile=mcp-app";
/// The largest app page, and the largest result kept for or sent to an app.
const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 2 * 1024 * 1024;
/// Tool calls one server's apps may make in a conversation per minute.
const CALLS_PER_MINUTE: usize = 20;
const MAX_SOURCES: usize = 32;

/// A tool's `_meta.ui`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolUi {
    pub resource_uri: Option<String>,
    /// Who may call it: the model, the server's app.
    pub model: bool,
    pub app: bool,
}

impl ToolUi {
    pub fn of(tool: &Value) -> Option<ToolUi> {
        let ui = &tool["_meta"]["ui"];
        // the flat key is the spec's older form
        let uri = ui["resourceUri"].as_str().or(tool["_meta"]["ui/resourceUri"].as_str());
        let visibility: Option<Vec<&str>> = ui["visibility"].as_array().map(|v| v.iter().filter_map(Value::as_str).collect());
        if uri.is_none() && visibility.is_none() {
            return None;
        }
        let has = |who: &str| visibility.as_ref().is_none_or(|v| v.contains(&who));
        Some(ToolUi { resource_uri: uri.filter(|u| valid_ui_uri(u)).map(String::from), model: has("model"), app: has("app") })
    }
}

/// A `ui://` URI as a server may name its page: short, no spaces or controls.
pub fn valid_ui_uri(u: &str) -> bool {
    u.len() <= 2048 && u.strip_prefix("ui://").is_some_and(|r| !r.is_empty()) && !u.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '\\'))
}

/// The key under which a call's whole result is kept for its app.
pub fn result_key(api_name: &str, args: &Value) -> String {
    crate::util::sha256_hex(format!("{api_name}\u{0}{args}").as_bytes())
}

/// A source an app declares (`https://host[:port]`, `*.` for subdomains;
/// `wss://` too where it connects), or `None`: anything else is dropped.
fn source(s: &str, connect: bool) -> Option<String> {
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("wss://").filter(|_| connect))?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => (h, Some(p)),
        _ => (rest.trim_end_matches('/'), None),
    };
    let bare = host.strip_prefix("*.").unwrap_or(host);
    let labels: Vec<&str> = bare.split('.').collect();
    let ok = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && !bare.eq_ignore_ascii_case("localhost");
    ok.then(|| format!("{}{}{}", &s[..s.len() - rest.len()], host.to_ascii_lowercase(), port.map(|p| format!(":{p}")).unwrap_or_default()))
}

fn sources(list: &Value, connect: bool) -> String {
    let v: Vec<String> = list.as_array().into_iter().flatten().filter_map(Value::as_str).filter_map(|s| source(s, connect)).take(MAX_SOURCES).collect();
    v.join(" ")
}

/// The page's Content Security Policy for its resource's `_meta.ui.csp`:
/// inline code and the declared, checked sources; no nested frames, no
/// plugins, no base URL. It narrows the policy the protocol sends with an
/// app page (artifacts::protocol_csp).
pub fn csp(declared: &Value) -> String {
    let res = sources(&declared["resourceDomains"], false);
    let connect = sources(&declared["connectDomains"], true);
    let with = |base: &str| if res.is_empty() { base.to_string() } else { format!("{base} {res}") };
    format!(
        "default-src 'none'; script-src {}; style-src {}; img-src {}; font-src {}; media-src {}; connect-src {}; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'",
        with("'unsafe-inline'"),
        with("'unsafe-inline'"),
        with("data: blob:"),
        with("data:"),
        with("data: blob:"),
        if connect.is_empty() { "'none'".into() } else { connect },
    )
}

/// Whether the app's call of a tool runs: the model's policy for that tool
/// ("allow", "deny", or "ask"), a grant for this chat, the person's answer.
#[derive(Debug, PartialEq)]
pub enum Decision {
    Run,
    Ask,
    Deny,
}

pub fn decide(policy: &str, chat_allowed: bool, approved: bool) -> Decision {
    match policy {
        "deny" => Decision::Deny,
        "allow" => Decision::Run,
        _ if chat_allowed || approved => Decision::Run,
        _ => Decision::Ask,
    }
}

#[derive(Default)]
struct Calls(HashMap<(String, String), VecDeque<Instant>>);

impl Calls {
    fn take(&mut self, conv: &str, server: &str, now: Instant) -> AppResult<()> {
        let q = self.0.entry((conv.into(), server.into())).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            q.pop_front();
        }
        if q.len() >= CALLS_PER_MINUTE {
            return Err(AppError::msg(format!("This app called its tools {CALLS_PER_MINUTE} times in a minute: wait a moment.")));
        }
        q.push_back(now);
        Ok(())
    }
}

static CALLS: LazyLock<Mutex<Calls>> = LazyLock::new(Default::default);

type St<'a> = tauri::State<'a, Arc<AppState>>;

/// The app of the MCP tool call `api_name`(`input`), if its tool has one:
/// server, tool, page, and the call's whole result while it is kept.
#[tauri::command]
pub async fn mcp_app_info(state: St<'_>, api_name: String, input: Value) -> AppResult<Value> {
    Ok(match state.mcp.app_of(&api_name, &input).await {
        Some((server, tool, uri, result)) => json!({"server": server, "tool": tool, "uri": uri, "result": result}),
        None => Value::Null,
    })
}

/// Read app page `uri` of `server` and serve it in the artifact sandbox: the token to load.
#[tauri::command]
pub async fn mcp_app_open(state: St<'_>, server: String, uri: String) -> AppResult<Value> {
    if !valid_ui_uri(&uri) {
        return Err(AppError::msg("That isn't an MCP App page (ui://)."));
    }
    let c = state.mcp.read_resource_raw(&server, &uri).await?;
    let mime = c["mimeType"].as_str().unwrap_or("").to_ascii_lowercase().replace(' ', "");
    if mime != MIME {
        return Err(AppError::msg(format!("{uri} isn't an MCP App page ({MIME}).")));
    }
    let html = match (c["text"].as_str(), c["blob"].as_str()) {
        (Some(t), _) => t.to_string(),
        (None, Some(b)) => {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD.decode(b).map_err(|e| AppError::msg(format!("{uri}: {e}")))?;
            String::from_utf8(bytes).map_err(|_| AppError::msg(format!("{uri} isn't UTF-8 text.")))?
        }
        _ => return Err(AppError::msg(format!("{uri} has no page."))),
    };
    if html.len() > MAX_PAGE_BYTES {
        return Err(AppError::msg(format!("{uri} is larger than {}.", crate::util::human_size(MAX_PAGE_BYTES as u64))));
    }
    let ui = &c["_meta"]["ui"];
    let page = crate::artifacts::insert_at_start(&html, &format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">", csp(&ui["csp"])));
    let token = crate::commands::serve_artifact(state.clone(), crate::artifacts::MCP_APP_KIND.into(), page, false).await?;
    // under its own policy: the artifacts' would block every source its server declared
    if let Some(doc) = state.served.lock().get_mut(&token) {
        doc.0 = crate::artifacts::MCP_APP_SERVED.into();
    }
    Ok(json!({"token": token, "prefers_border": ui["prefersBorder"]}))
}

/// `tools/call` from the app of `server` shown in `conversation_id`. Asked
/// about like the model's calls: `{"needs_approval": true}` until the person
/// said yes (`approved`) where the policy asks; the whole result after.
#[tauri::command]
pub async fn mcp_app_call(state: St<'_>, conversation_id: String, server: String, tool: String, arguments: Value, approved: bool) -> AppResult<Value> {
    // a tool the server doesn't offer its app is refused before anyone is asked about it
    state.mcp.app_callable(&server, &tool).await?;
    let api_name = crate::mcp::api_tool_name(&server, &tool);
    let policy = state.settings.read().tool_policies.get(&format!("{server}/{tool}")).and_then(|v| v.as_str()).unwrap_or("ask").to_string();
    let conv = state.db.conv_get(&conversation_id)?;
    let chat_allowed = conv.options["allowed_tools"].as_array().into_iter().flatten().any(|t| t.as_str() == Some(&api_name));
    match decide(&policy, chat_allowed, approved) {
        Decision::Deny => return Err(AppError::msg("This tool is disabled by the user.")),
        Decision::Ask => return Ok(json!({"needs_approval": true})),
        Decision::Run => {}
    }
    CALLS.lock().take(&conversation_id, &server, Instant::now())?;
    let r = state.mcp.app_call(&server, &tool, &arguments).await?;
    if r.to_string().len() > MAX_RESULT_BYTES {
        return Err(AppError::msg("The tool's result is too large for the app."));
    }
    Ok(json!({"result": r}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_names_its_page_and_who_may_call_it() {
        let t = json!({"name": "chart", "_meta": {"ui": {"resourceUri": "ui://charts/bar.html"}}});
        assert_eq!(ToolUi::of(&t), Some(ToolUi { resource_uri: Some("ui://charts/bar.html".into()), model: true, app: true }));
        let flat = json!({"_meta": {"ui/resourceUri": "ui://x/y"}});
        assert_eq!(ToolUi::of(&flat).unwrap().resource_uri.as_deref(), Some("ui://x/y"));
        let app_only = json!({"_meta": {"ui": {"visibility": ["app"]}}});
        assert_eq!(ToolUi::of(&app_only), Some(ToolUi { resource_uri: None, model: false, app: true }));
        assert_eq!(ToolUi::of(&json!({"name": "plain"})), None);
        // a page that isn't ui:// is no page
        let long = format!("ui://{}", "a".repeat(3000));
        for bad in ["https://evil.example/x.html", "ui://", "ui://a b", "ui://x\"><script>", "javascript:alert(1)", long.as_str()] {
            assert!(!valid_ui_uri(bad), "{bad}");
            assert_eq!(ToolUi::of(&json!({"_meta": {"ui": {"resourceUri": bad}}})).unwrap().resource_uri, None, "{bad}");
        }
    }

    #[test]
    fn the_csp_allows_only_the_checked_sources_it_declares() {
        let none = csp(&Value::Null);
        assert!(none.starts_with("default-src 'none';"));
        assert!(none.contains("connect-src 'none'") && none.contains("frame-src 'none'") && none.contains("object-src 'none'"));
        assert!(!none.contains("https:") && !none.contains('*'));
        let declared = json!({
            "resourceDomains": ["https://cdn.example.com", "https://*.assets.example.org", "http://insecure.example", "https://evil.example/'; script-src *", "*", "https://localhost", "https://com"],
            "connectDomains": ["https://API.example.com:8443", "wss://live.example.com", "data:"],
            "frameDomains": ["https://frames.example.com"]
        });
        let c = csp(&declared);
        assert!(c.contains("script-src 'unsafe-inline' https://cdn.example.com https://*.assets.example.org;"), "{c}");
        assert!(c.contains("connect-src https://api.example.com:8443 wss://live.example.com;"), "{c}");
        for dropped in ["insecure", "evil", "localhost", " *", "https://com", "frames.example.com", "data:;"] {
            assert!(!c.contains(dropped), "{dropped} in {c}");
        }
        // wss only where the page connects
        assert!(!csp(&json!({"resourceDomains": ["wss://live.example.com"]})).contains("wss:"));
        // a page can't declare its way past the cap
        let many: Vec<String> = (0..100).map(|i| format!("https://h{i}.example.com")).collect();
        assert_eq!(csp(&json!({"resourceDomains": many})).matches("https://h").count(), MAX_SOURCES * 5);
    }

    #[test]
    fn an_apps_calls_follow_the_tool_policy_and_are_rate_limited() {
        assert_eq!(decide("deny", true, true), Decision::Deny);
        assert_eq!(decide("allow", false, false), Decision::Run);
        assert_eq!(decide("ask", false, false), Decision::Ask);
        assert_eq!(decide("ask", true, false), Decision::Run);
        assert_eq!(decide("ask", false, true), Decision::Run);
        assert_eq!(decide("", false, false), Decision::Ask, "no policy asks");
        let mut c = Calls::default();
        let t0 = Instant::now();
        for _ in 0..CALLS_PER_MINUTE {
            c.take("c", "s", t0).unwrap();
        }
        assert!(c.take("c", "s", t0).is_err());
        assert!(c.take("c", "other", t0).is_ok());
        assert!(c.take("c", "s", t0 + Duration::from_secs(60)).is_ok());
    }
}
