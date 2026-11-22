//! OAuth sign-in for remote MCP servers, as the MCP authorization spec
//! describes it: the server's 401 (or its well-known address) leads to its
//! protected-resource metadata (RFC 9728), that to the authorization
//! server's metadata (RFC 8414); the app registers itself (RFC 7591) unless
//! the config names a client id, and the user signs in in their own browser
//! (authorization code with PKCE, RFC 7636), which comes back to a one-shot
//! listener on 127.0.0.1. Tokens live in the OS secret store, never in the
//! config file or a log.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

const CALLBACK_PATH: &str = "/callback";
/// How long the user has to finish signing in in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// A token this close to its expiry is refreshed before it is sent.
const REFRESH_EARLY_SECS: i64 = 60;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub expires_at: Option<i64>,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    /// The `resource` (RFC 8707) the tokens were issued for.
    pub resource: String,
}

// never print a token, not even in a panic message
impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tokens { … }")
    }
}

fn now_secs() -> i64 {
    crate::util::now_ms() / 1000
}

impl Tokens {
    pub fn expires_soon(&self) -> bool {
        self.refresh_token.is_some() && self.expires_at.is_some_and(|e| e - now_secs() < REFRESH_EARLY_SECS)
    }
}

// ------------------------------------------------------------------ storage

/// Where the tokens of server `name` at `url` are kept: a new URL is a new
/// resource and needs its own sign-in.
pub fn token_key(name: &str, url: &str) -> String {
    format!("mcp-oauth-{}", &crate::util::sha256_hex(format!("{name}\u{0}{url}").as_bytes())[..16])
}

pub fn load(key: &str) -> Option<Tokens> {
    serde_json::from_str(&store::get(key)?).ok()
}

pub fn save(key: &str, t: &Tokens) -> AppResult<()> {
    store::set(key, &serde_json::to_string(t)?).map_err(|e| AppError::msg(format!("Keeping the sign-in in the system's credential store failed: {e}")))
}

pub fn forget(key: &str) {
    store::delete(key)
}

/// Whether the store keeps what it is given across restarts (not on Linux here, nor in tests).
pub(crate) const STORE_PERSISTS: bool = cfg!(all(any(target_os = "macos", target_os = "windows"), not(test)));
pub(crate) use store::{delete as secret_delete, get as secret_get, set as secret_set};

#[cfg(all(any(target_os = "macos", target_os = "windows"), not(test)))]
mod store {
    /// Windows keeps at most 2560 bytes per credential, and two JWTs can be
    /// longer: a value is split over several.
    const CHUNK: usize = 2048;

    fn entry(key: &str, part: &str) -> keyring::Result<keyring::Entry> {
        keyring::Entry::new(crate::settings::secret_service(), &format!("{key}.{part}"))
    }

    fn count(key: &str) -> Option<usize> {
        String::from_utf8(entry(key, "n").ok()?.get_secret().ok()?).ok()?.parse().ok()
    }

    pub fn get(key: &str) -> Option<String> {
        let mut out = Vec::new();
        for i in 0..count(key)? {
            out.extend(entry(key, &i.to_string()).ok()?.get_secret().ok()?);
        }
        String::from_utf8(out).ok()
    }

    pub fn set(key: &str, value: &str) -> Result<(), String> {
        delete(key);
        let parts: Vec<&[u8]> = value.as_bytes().chunks(CHUNK).collect();
        let put = |part: &str, bytes: &[u8]| entry(key, part).and_then(|e| e.set_secret(bytes)).map_err(|e| e.to_string());
        for (i, p) in parts.iter().enumerate() {
            put(&i.to_string(), p)?;
        }
        // the count goes last: a value is only found once all of it is there
        put("n", parts.len().to_string().as_bytes())
    }

    pub fn delete(key: &str) {
        let parts = count(key).unwrap_or(0);
        for part in (0..parts).map(|i| i.to_string()).chain(["n".to_string()]) {
            if let Ok(e) = entry(key, &part) {
                let _ = e.delete_credential();
            }
        }
    }
}

// shortcut: Linux has no secret-store dependency here (keyring is a macOS and
// Windows one), so a sign-in lasts until the app quits; tests use this too.
// Upgrade: keyring's Secret Service backend.
#[cfg(not(all(any(target_os = "macos", target_os = "windows"), not(test))))]
mod store {
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::LazyLock;

    static MEM: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

    pub fn get(key: &str) -> Option<String> {
        MEM.lock().get(key).cloned()
    }

    pub fn set(key: &str, value: &str) -> Result<(), String> {
        MEM.lock().insert(key.into(), value.into());
        Ok(())
    }

    pub fn delete(key: &str) {
        MEM.lock().remove(key);
    }
}

// ---------------------------------------------------------------- discovery

/// `s` as an address that credentials may go to: https, or http on this machine.
fn secure_url(s: &str) -> AppResult<Url> {
    let u = Url::parse(s).map_err(|e| AppError::msg(format!("Bad sign-in address '{s}': {e}")))?;
    let local = match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if u.scheme() == "https" || (u.scheme() == "http" && local) {
        Ok(u)
    } else {
        Err(AppError::msg(format!("Refusing to sign in through '{s}': only https addresses are allowed.")))
    }
}

/// This machine, a private or link-local network (cloud metadata lives at
/// 169.254.169.254), shared address space, or no address at all.
fn internal_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, ..] = v.octets();
            v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || a == 0 || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v) => {
            let s = v.segments()[0];
            // fc00::/7 unique local, fe80::/10 link-local, ::ffff:a.b.c.d mapped
            v.is_loopback() || v.is_unspecified() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80 || v.to_ipv4_mapped().is_some_and(|m| internal_ip(m.into()))
        }
    }
}

/// Whether `u` names this machine or a private network outright.
fn internal(u: &Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => internal_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => internal_ip(ip.into()),
        None => true,
    }
}

/// Whether the server at `u` is in the user's own network: by its address,
/// or by what its name resolves to (an intranet server).
async fn local_server(u: &Url) -> bool {
    if internal(u) {
        return true;
    }
    let Some(host) = u.host_str() else { return true };
    tokio::net::lookup_host((host, 0)).await.is_ok_and(|mut a| a.any(|a| internal_ip(a.ip())))
}

/// `s` as an address the sign-in may use: a secure one (`secure_url`), and
/// one in the user's own network only when the server is (`local`) too, so
/// a server on the internet can't make the app call into the user's network.
fn allowed(s: &str, local: bool) -> AppResult<Url> {
    let u = secure_url(s)?;
    if internal(&u) && !local {
        return Err(AppError::msg(format!("Refusing to sign in through '{s}': it is on this computer or a private network, and the server isn't.")));
    }
    Ok(u)
}

/// Resolves names to public addresses only: a name that leads into the
/// user's network is refused, also one that changes there after it was checked.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.filter(|a| !internal_ip(a.ip())).collect();
            if addrs.is_empty() {
                return Err(format!("{} is not a public address", name.as_str()).into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Redirects followed only within the first request's origin: one elsewhere
/// could carry a code, a token or a configured header to an address nobody
/// checked.
pub(crate) fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|a| {
        let first = a.previous().first().map(Url::origin);
        if a.previous().len() < 5 && first == Some(a.url().origin()) {
            a.follow()
        } else {
            a.stop()
        }
    })
}

/// A client for downloads the user started (Node.js, a directory
/// extension): `redirect` decides where it may follow, and with
/// `public_only` it connects to public addresses only.
pub(crate) fn download_client(redirect: reqwest::redirect::Policy, public_only: bool) -> AppResult<reqwest::Client> {
    let mut b = reqwest::Client::builder().redirect(redirect).connect_timeout(Duration::from_secs(20)).read_timeout(Duration::from_secs(60));
    if public_only {
        b = b.dns_resolver(Arc::new(PublicOnly));
    }
    Ok(b.build()?)
}

/// The HTTP client for the sign-in of the server at `url`. It follows
/// redirects within the same origin only and, for a server outside the
/// user's network, connects to public addresses only.
pub async fn client_for(url: &str) -> AppResult<reqwest::Client> {
    let u = secure_url(url)?;
    let mut b = reqwest::Client::builder().redirect(same_origin_redirects()).connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(30));
    if !local_server(&u).await {
        b = b.dns_resolver(Arc::new(PublicOnly));
    }
    Ok(b.build()?)
}

/// Whether metadata for `resource` applies to the server at `server`: the
/// same origin, and the server's path is the resource's or below it.
fn covers(resource: &Url, server: &Url) -> bool {
    let base = resource.path().trim_end_matches('/');
    let path = server.path();
    resource.origin() == server.origin() && (path.trim_end_matches('/') == base || path.starts_with(&format!("{base}/")))
}

fn same_issuer(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

fn with_path(base: &Url, path: &str) -> Url {
    let mut u = base.clone();
    u.set_path(path);
    u.set_query(None);
    u.set_fragment(None);
    u
}

/// Where a server's protected-resource metadata may be (RFC 9728 §3.1).
fn resource_metadata_urls(server: &Url) -> Vec<Url> {
    let path = server.path().trim_end_matches('/');
    let mut out = vec![];
    if !path.is_empty() {
        out.push(with_path(server, &format!("/.well-known/oauth-protected-resource{path}")));
    }
    out.push(with_path(server, "/.well-known/oauth-protected-resource"));
    out
}

/// Where an authorization server's metadata may be: RFC 8414 §3.1 first,
/// then the two places OpenID Connect discovery uses.
fn auth_server_metadata_urls(issuer: &Url) -> Vec<Url> {
    let path = issuer.path().trim_end_matches('/');
    if path.is_empty() {
        return vec![
            with_path(issuer, "/.well-known/oauth-authorization-server"),
            with_path(issuer, "/.well-known/openid-configuration"),
        ];
    }
    vec![
        with_path(issuer, &format!("/.well-known/oauth-authorization-server{path}")),
        with_path(issuer, &format!("/.well-known/openid-configuration{path}")),
        with_path(issuer, &format!("{path}/.well-known/openid-configuration")),
    ]
}

/// A parameter of a `WWW-Authenticate: Bearer …` challenge (RFC 6750 §3).
fn challenge_param(header: &str, name: &str) -> Option<String> {
    let mut rest = header;
    while let Some(i) = rest.find('=') {
        let key = rest[..i].rsplit(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("");
        let after = &rest[i + 1..];
        let (value, next) = match after.strip_prefix('"') {
            Some(q) => {
                let mut value = String::new();
                let mut escaped = false;
                let mut end = q.len();
                for (j, c) in q.char_indices() {
                    match c {
                        _ if escaped => {
                            value.push(c);
                            escaped = false;
                        }
                        '\\' => escaped = true,
                        '"' => {
                            end = j + 1;
                            break;
                        }
                        _ => value.push(c),
                    }
                }
                (value, &q[end..])
            }
            None => {
                let end = after.find(',').unwrap_or(after.len());
                (after[..end].trim().to_string(), &after[end..])
            }
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
        rest = next;
    }
    None
}

async fn get_json(client: &reqwest::Client, url: &Url) -> Option<Value> {
    let r = client.get(url.clone()).header("Accept", "application/json").send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json().await.ok().filter(Value::is_object)
}

/// The server's `WWW-Authenticate` challenge to a request without a token.
async fn fetch_challenge(client: &reqwest::Client, url: &str) -> AppResult<Option<String>> {
    let body = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": crate::mcp::PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "ClaudeRust", "version": env!("CARGO_PKG_VERSION")}
    }});
    let r = client.post(url).header("Accept", "application/json, text/event-stream").json(&body).send().await?;
    if r.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(None);
    }
    Ok(r.headers().get(reqwest::header::WWW_AUTHENTICATE).and_then(|v| v.to_str().ok()).map(String::from))
}

/// The protected-resource metadata of the server at `server`, if it has any.
async fn resource_metadata(client: &reqwest::Client, server: &Url, challenge: Option<&str>, local: bool) -> AppResult<Option<Value>> {
    let mut tries = vec![];
    if let Some(u) = challenge.and_then(|h| challenge_param(h, "resource_metadata")) {
        tries.push(allowed(&u, local)?);
    }
    tries.extend(resource_metadata_urls(server));
    for u in tries {
        let Some(m) = get_json(client, &u).await else { continue };
        let resource = m["resource"].as_str().and_then(|r| Url::parse(r).ok());
        if !resource.is_some_and(|r| covers(&r, server)) {
            return Err(AppError::msg(format!(
                "The sign-in metadata at {u} is for {}, not for {server}, so it was not used.",
                m["resource"]
            )));
        }
        return Ok(Some(m));
    }
    Ok(None)
}

async fn auth_server_metadata(client: &reqwest::Client, issuer: &str, local: bool) -> AppResult<Value> {
    let iss = allowed(issuer, local)?;
    for u in auth_server_metadata_urls(&iss) {
        let Some(m) = get_json(client, &u).await else { continue };
        if !same_issuer(m["issuer"].as_str().unwrap_or(""), issuer) {
            return Err(AppError::msg(format!("The sign-in server's metadata names issuer {} instead of {issuer}, so it was not used.", m["issuer"])));
        }
        for k in ["authorization_endpoint", "token_endpoint"] {
            allowed(m[k].as_str().unwrap_or(""), local)?;
        }
        if m["code_challenge_methods_supported"].as_array().is_some_and(|a| !a.iter().any(|x| x == "S256")) {
            return Err(AppError::msg(format!("The sign-in server {issuer} doesn't support PKCE with S256, which signing in requires.")));
        }
        return Ok(m);
    }
    Err(AppError::msg(format!("No OAuth metadata was found for the sign-in server {issuer}.")))
}

/// Register this app with the authorization server (RFC 7591): (id, secret).
async fn register(client: &reqwest::Client, endpoint: &str, redirect: &str, local: bool) -> AppResult<(String, Option<String>)> {
    let body = json!({
        "client_name": "ClaudeRust",
        "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let r = client.post(allowed(endpoint, local)?).json(&body).send().await?;
    if !r.status().is_success() {
        return Err(AppError::msg(format!("Registering with the sign-in server failed (HTTP {}).", r.status().as_u16())));
    }
    let v: Value = r.json().await?;
    if v["redirect_uris"].as_array().is_some_and(|a| !a.iter().any(|u| u == redirect)) {
        return Err(AppError::msg("The sign-in server registered a different redirect address, so the sign-in was stopped."));
    }
    let id = v["client_id"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| AppError::msg("The sign-in server's registration gave no client id."))?;
    Ok((id.to_string(), v["client_secret"].as_str().map(String::from)))
}

/// A PKCE verifier and its S256 challenge.
fn pkce() -> (String, String) {
    // 64 characters from the OS random source (two v4 UUIDs, 244 random bits)
    let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

// ------------------------------------------------------------- the callback

/// The request target of an HTTP GET (empty for anything else).
async fn read_target(s: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(2).any(|w| w == b"\r\n") && buf.len() < 8192 {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut words = text.lines().next().unwrap_or("").split(' ');
    Ok(match (words.next(), words.next()) {
        (Some("GET"), Some(t)) => t.to_string(),
        _ => String::new(),
    })
}

async fn respond(mut s: TcpStream, status: &str, text: &str) {
    let body = format!("<!doctype html><meta charset=utf-8><title>ClaudeRust</title><body style=\"font:16px system-ui;margin:3em\">{text}</body>");
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(resp.as_bytes()).await;
    let _ = s.shutdown().await;
}

/// The authorization code in the parameters the browser came back with.
fn check_callback(p: &HashMap<String, String>, state: &str, issuer: &str) -> AppResult<String> {
    if p.get("state").map(String::as_str) != Some(state) {
        return Err(AppError::msg("The sign-in answer didn't belong to this sign-in (its state didn't match), so it was rejected."));
    }
    if let Some(e) = p.get("error") {
        let why = p.get("error_description").map(|d| format!(": {d}")).unwrap_or_default();
        return Err(AppError::msg(format!("Sign-in failed ({e}{why}).")));
    }
    // RFC 9207: an answer that names its issuer must name ours
    if p.get("iss").is_some_and(|i| !same_issuer(i, issuer)) {
        return Err(AppError::msg("The sign-in answer came from another sign-in server, so it was rejected."));
    }
    p.get("code").filter(|c| !c.is_empty()).cloned().ok_or_else(|| AppError::msg("The sign-in answer had no authorization code."))
}

/// Wait for the browser to come back to the callback path. Other paths (a
/// favicon) get a 404 and are skipped; the first request to the callback
/// ends the wait, right `state` or not, and the listener is closed.
async fn wait_for_code(listener: TcpListener, state: &str, issuer: &str, timeout: Duration) -> AppResult<String> {
    let wait = async {
        loop {
            let (mut s, _) = listener.accept().await?;
            // a client that sends nothing must not hold up the browser
            let Ok(Ok(target)) = tokio::time::timeout(Duration::from_secs(5), read_target(&mut s)).await else { continue };
            let query = match target.strip_prefix(CALLBACK_PATH) {
                Some(rest) if rest.is_empty() || rest.starts_with('?') => rest.trim_start_matches('?').to_string(),
                _ => {
                    respond(s, "404 Not Found", "Not found.").await;
                    continue;
                }
            };
            let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
            let code = check_callback(&params, state, issuer);
            match &code {
                Ok(_) => respond(s, "200 OK", "Signed in. You can close this tab and go back to ClaudeRust.").await,
                Err(_) => respond(s, "400 Bad Request", "Sign-in failed. Go back to ClaudeRust for the details.").await,
            }
            return code;
        }
    };
    tokio::time::timeout(timeout, wait)
        .await
        .map_err(|_| AppError::msg("Signing in took too long: the browser never came back. Try again."))?
}

// ------------------------------------------------------------------- tokens

enum TokenError {
    /// The server refused the grant (an expired or revoked sign-in).
    Rejected(String),
    Failed(AppError),
}

async fn token_call(client: &reqwest::Client, endpoint: &str, form: &[(&str, &str)]) -> Result<Value, TokenError> {
    // bounded: a refresh holds up every request to its server
    let r = client
        .post(endpoint)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(30))
        .form(form)
        .send()
        .await
        .map_err(|e| TokenError::Failed(e.into()))?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(v);
    }
    // the server's own words, never the body (it can repeat what was sent)
    let why = match v["error"].as_str() {
        Some(e) => format!("{e}{}", v["error_description"].as_str().map(|d| format!(": {d}")).unwrap_or_default()),
        None => format!("HTTP {}", status.as_u16()),
    };
    if matches!(status.as_u16(), 400 | 401) {
        Err(TokenError::Rejected(why))
    } else {
        Err(TokenError::Failed(AppError::msg(format!("The sign-in server failed ({why})."))))
    }
}

/// `base` with the tokens of a token endpoint answer.
fn tokens_from(v: &Value, mut base: Tokens) -> AppResult<Tokens> {
    let at = v["access_token"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| AppError::msg("The sign-in server sent no access token."))?;
    if let Some(ty) = v["token_type"].as_str().filter(|ty| !ty.eq_ignore_ascii_case("bearer")) {
        return Err(AppError::msg(format!("The sign-in server sent a {ty} token; only Bearer tokens are supported.")));
    }
    base.access_token = at.to_string();
    // without a new refresh token the old one stays good (RFC 6749 §6)
    if let Some(r) = v["refresh_token"].as_str() {
        base.refresh_token = Some(r.to_string());
    }
    base.expires_at = v["expires_in"].as_i64().map(|s| now_secs() + s);
    Ok(base)
}

fn with_secret<'a>(mut form: Vec<(&'a str, &'a str)>, t: &'a Tokens) -> Vec<(&'a str, &'a str)> {
    // shortcut: a registered secret is always sent as client_secret_post; add
    // client_secret_basic if a server turns out to require it
    if let Some(s) = &t.client_secret {
        form.push(("client_secret", s));
    }
    form
}

/// New tokens for `t`, the sign-in of the server at `server`. `Ok(None)`:
/// the server refused, the sign-in is over.
pub async fn refresh(server: &str, t: &Tokens) -> AppResult<Option<Tokens>> {
    let Some(rt) = &t.refresh_token else { return Ok(None) };
    let client = client_for(server).await?;
    let form = with_secret(vec![("grant_type", "refresh_token"), ("refresh_token", rt.as_str()), ("client_id", t.client_id.as_str()), ("resource", t.resource.as_str())], t);
    match token_call(&client, &t.token_endpoint, &form).await {
        Ok(v) => tokens_from(&v, t.clone()).map(Some),
        Err(TokenError::Rejected(_)) => Ok(None),
        Err(TokenError::Failed(e)) => Err(e),
    }
}

/// The whole sign-in for the MCP server at `url`. `open` shows the
/// authorization page in the user's browser; the user has `timeout` to finish.
pub async fn sign_in(
    client: &reqwest::Client,
    url: &str,
    client_id: Option<&str>,
    open: impl FnOnce(&str) -> AppResult<()>,
    timeout: Duration,
) -> AppResult<Tokens> {
    let server = secure_url(url)?;
    let local = local_server(&server).await;
    let challenge = fetch_challenge(client, url).await?;
    let prm = resource_metadata(client, &server, challenge.as_deref(), local).await?;
    // without resource metadata (older servers) the server's origin is the issuer
    let (issuer, resource) = match &prm {
        Some(m) => (
            m["authorization_servers"][0]
                .as_str()
                .ok_or_else(|| AppError::msg("The server's sign-in metadata names no authorization server."))?
                .to_string(),
            m["resource"].as_str().unwrap_or(url).to_string(),
        ),
        None => (server.origin().ascii_serialization(), url.to_string()),
    };
    let meta = auth_server_metadata(client, &issuer, local).await?;
    let scope = challenge.as_deref().and_then(|h| challenge_param(h, "scope")).or_else(|| {
        let s: Vec<&str> = prm.as_ref()?["scopes_supported"].as_array()?.iter().filter_map(Value::as_str).collect();
        (!s.is_empty()).then(|| s.join(" "))
    });
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let redirect = format!("http://127.0.0.1:{}{CALLBACK_PATH}", listener.local_addr()?.port());
    let (id, secret) = match (client_id, meta["registration_endpoint"].as_str()) {
        (Some(id), _) => (id.to_string(), None),
        (None, Some(endpoint)) => register(client, endpoint, &redirect, local).await?,
        (None, None) => {
            return Err(AppError::msg(
                "This server's sign-in doesn't let apps register themselves. Add \"oauth\": {\"clientId\": \"…\"} with a client id registered for http://127.0.0.1 to its entry in the config.",
            ))
        }
    };
    let (verifier, code_challenge) = pkce();
    let state = uuid::Uuid::new_v4().simple().to_string();
    let mut auth = allowed(meta["authorization_endpoint"].as_str().unwrap_or(""), local)?;
    {
        let mut q = auth.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &resource);
        if let Some(s) = &scope {
            q.append_pair("scope", s);
        }
    }
    open(auth.as_str())?;
    let code = wait_for_code(listener, &state, &issuer, timeout).await?;
    let base = Tokens {
        access_token: String::new(),
        refresh_token: None,
        expires_at: None,
        client_id: id,
        client_secret: secret,
        token_endpoint: allowed(meta["token_endpoint"].as_str().unwrap_or(""), local)?.to_string(),
        resource,
    };
    let form = with_secret(
        vec![
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("client_id", base.client_id.as_str()),
            ("code_verifier", verifier.as_str()),
            ("resource", base.resource.as_str()),
        ],
        &base,
    );
    let v = match token_call(client, &base.token_endpoint, &form).await {
        Ok(v) => v,
        Err(TokenError::Rejected(why)) => return Err(AppError::msg(format!("The sign-in server refused the sign-in ({why})."))),
        Err(TokenError::Failed(e)) => return Err(e),
    };
    tokens_from(&v, base)
}

// ----------------------------------------------------------------- commands

type St<'a> = tauri::State<'a, Arc<AppState>>;

/// The URL and pre-registered client id of remote server `name`.
fn remote(st: &AppState, name: &str) -> AppResult<(String, Option<String>)> {
    let conf = crate::mcp::read_config(&st.mcp.config_path)["mcpServers"][name].clone();
    let url = conf["url"].as_str().ok_or_else(|| AppError::msg(format!("MCP server '{name}' isn't a remote (URL) server.")))?;
    Ok((url.to_string(), conf["oauth"]["clientId"].as_str().map(String::from)))
}

/// Sign in to remote server `server` in the browser, then reconnect it.
#[tauri::command]
pub async fn mcp_oauth_sign_in(app: tauri::AppHandle, state: St<'_>, server: String) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let st = state.inner().clone();
    let (url, client_id) = remote(&st, &server)?;
    let client = client_for(&url).await?;
    let open =|u: &str| app.opener().open_url(u, None::<&str>).map_err(|e| AppError::msg(e.to_string()));
    let tokens = sign_in(&client, &url, client_id.as_deref(), open, SIGN_IN_TIMEOUT).await?;
    save(&token_key(&server, &url), &tokens)?;
    let disabled = st.settings.read().disabled_mcp_servers.clone();
    st.mcp.restart(&server, &disabled).await;
    Ok(())
}

/// Forget the sign-in of remote server `server`, then reconnect it.
#[tauri::command]
pub async fn mcp_oauth_sign_out(state: St<'_>, server: String) -> AppResult<()> {
    let st = state.inner().clone();
    let (url, _) = remote(&st, &server)?;
    forget(&token_key(&server, &url));
    let disabled = st.settings.read().disabled_mcp_servers.clone();
    st.mcp.restart(&server, &disabled).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpManager;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use parking_lot::Mutex;

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[test]
    fn challenges_are_parsed() {
        let h = r#"Bearer realm="a, b", error="invalid_token", resource_metadata="https://x.example/.well-known/oauth-protected-resource/mcp", scope="files:read files:write""#;
        assert_eq!(challenge_param(h, "resource_metadata").as_deref(), Some("https://x.example/.well-known/oauth-protected-resource/mcp"));
        assert_eq!(challenge_param(h, "scope").as_deref(), Some("files:read files:write"));
        assert_eq!(challenge_param(h, "realm").as_deref(), Some("a, b"));
        assert_eq!(challenge_param(r#"Bearer error_description="say \"hi\"", Scope=x"#, "error_description").as_deref(), Some(r#"say "hi""#));
        assert_eq!(challenge_param(r#"Bearer error_description="say \"hi\"", Scope=x"#, "scope").as_deref(), Some("x"));
        assert_eq!(challenge_param("Bearer", "scope"), None);
        assert_eq!(challenge_param(r#"Bearer realm="scope=no""#, "scope"), None);
    }

    #[test]
    fn credentials_only_go_to_secure_places() {
        for ok in ["https://auth.example/authorize", "http://127.0.0.1:8080/token", "http://localhost/x", "http://[::1]:9/x"] {
            assert!(secure_url(ok).is_ok(), "{ok}");
        }
        for bad in ["http://auth.example/token", "javascript:alert(1)", "file:///etc/passwd", "ftp://x.example/", "not a url"] {
            assert!(secure_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn metadata_binds_to_its_resource() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(covers(&u("https://m.example/mcp"), &u("https://m.example/mcp")));
        assert!(covers(&u("https://m.example/"), &u("https://m.example/mcp")));
        assert!(covers(&u("https://m.example/api"), &u("https://m.example/api/mcp")));
        assert!(!covers(&u("https://m.example/api"), &u("https://m.example/apis")));
        assert!(!covers(&u("https://m.example/mcp"), &u("https://other.example/mcp")));
        assert!(!covers(&u("http://m.example/mcp"), &u("https://m.example/mcp")));
        assert!(!covers(&u("https://m.example/mcp/a"), &u("https://m.example/mcp")));
    }

    #[test]
    fn well_known_addresses() {
        let names = |v: Vec<Url>| v.into_iter().map(|u| u.to_string()).collect::<Vec<_>>();
        assert_eq!(
            names(resource_metadata_urls(&Url::parse("https://m.example/v1/mcp?x=1").unwrap())),
            ["https://m.example/.well-known/oauth-protected-resource/v1/mcp", "https://m.example/.well-known/oauth-protected-resource"]
        );
        assert_eq!(names(resource_metadata_urls(&Url::parse("https://m.example").unwrap())), ["https://m.example/.well-known/oauth-protected-resource"]);
        assert_eq!(
            names(auth_server_metadata_urls(&Url::parse("https://a.example/tenant1/").unwrap())),
            [
                "https://a.example/.well-known/oauth-authorization-server/tenant1",
                "https://a.example/.well-known/openid-configuration/tenant1",
                "https://a.example/tenant1/.well-known/openid-configuration"
            ]
        );
        assert_eq!(
            names(auth_server_metadata_urls(&Url::parse("https://a.example").unwrap())),
            ["https://a.example/.well-known/oauth-authorization-server", "https://a.example/.well-known/openid-configuration"]
        );
    }

    #[test]
    fn pkce_uses_s256() {
        let (v, c) = pkce();
        assert!((43..=128).contains(&v.len()) && v.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(c, URL_SAFE_NO_PAD.encode(Sha256::digest(v.as_bytes())));
        assert_ne!(pkce().0, v);
    }

    fn tokens(access: &str) -> Tokens {
        Tokens {
            access_token: access.into(),
            refresh_token: Some("rt-old".into()),
            expires_at: None,
            client_id: "c".into(),
            client_secret: None,
            token_endpoint: "https://a.example/token".into(),
            resource: "https://m.example/mcp".into(),
        }
    }

    #[test]
    fn token_answers() {
        let t = tokens_from(&json!({"access_token": "at-new", "token_type": "bearer", "expires_in": 30}), tokens("at-old")).unwrap();
        // no new refresh token: the old one stays
        assert_eq!((t.access_token.as_str(), t.refresh_token.as_deref()), ("at-new", Some("rt-old")));
        assert!(t.expires_soon());
        let t = tokens_from(&json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600}), tokens("x")).unwrap();
        assert!(!t.expires_soon() && t.refresh_token.as_deref() == Some("r"));
        assert!(tokens_from(&json!({"access_token": "a", "token_type": "DPoP"}), tokens("x")).is_err());
        assert!(tokens_from(&json!({"token_type": "Bearer"}), tokens("x")).is_err());
        // never printed
        assert_eq!(format!("{:?}", tokens("at-secret")), "Tokens { … }");
    }

    #[test]
    fn callback_parameters_are_checked() {
        let p = |q: &str| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect::<HashMap<String, String>>();
        let iss = "https://a.example/t";
        assert_eq!(check_callback(&p("code=c1&state=s"), "s", iss).unwrap(), "c1");
        assert_eq!(check_callback(&p("code=c1&state=s&iss=https%3A%2F%2Fa.example%2Ft"), "s", iss).unwrap(), "c1");
        assert!(check_callback(&p("code=c1&state=other"), "s", iss).unwrap_err().to_string().contains("state"));
        assert!(check_callback(&p("code=c1"), "s", iss).is_err());
        assert!(check_callback(&p("code=c1&state=s&iss=https%3A%2F%2Fevil.example"), "s", iss).unwrap_err().to_string().contains("another sign-in server"));
        assert!(check_callback(&p("error=access_denied&error_description=No&state=s"), "s", iss).unwrap_err().to_string().contains("access_denied: No"));
        assert!(check_callback(&p("state=s"), "s", iss).is_err());
    }

    #[tokio::test]
    async fn the_callback_takes_one_answer_with_the_right_state() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let wait = tokio::spawn(async move { wait_for_code(l, "s1", "https://a.example", Duration::from_secs(20)).await });
        let c = client();
        // a request that sends nothing doesn't hold up the real answer
        let _idle = TcpStream::connect(base.trim_start_matches("http://")).await.unwrap();
        // other paths are not the answer
        assert_eq!(c.get(format!("{base}/favicon.ico")).send().await.unwrap().status(), 404);
        // a forged answer is turned down and ends the sign-in
        let r = c.get(format!("{base}/callback?code=c&state=forged")).send().await.unwrap();
        assert_eq!(r.status(), 400);
        assert!(!r.text().await.unwrap().contains("forged"));
        assert!(wait.await.unwrap().unwrap_err().to_string().contains("state"));
        // nothing listens any more
        assert!(c.get(format!("{base}/callback?code=c&state=s1")).send().await.is_err());
        // and nobody coming back at all ends it too
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        assert!(wait_for_code(l, "s", "i", Duration::from_millis(50)).await.unwrap_err().to_string().contains("too long"));
    }

    // ------------------------------------------- a fake authorization server

    struct Req {
        method: String,
        path: String,
        query: HashMap<String, String>,
        auth: Option<String>,
        body: String,
    }

    struct Resp {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
    }

    fn json_resp(status: u16, v: Value) -> Resp {
        Resp { status, headers: vec![("content-type", "application/json".into())], body: v.to_string() }
    }

    /// A small HTTP/1.1 server on 127.0.0.1; its base address.
    async fn serve(handle: impl Fn(Req) -> Resp + Send + Sync + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
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
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let header = |name: &str| {
                        head.lines().find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
                        })
                    };
                    let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                    while buf.len() < head_end + len {
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let mut first = head.lines().next().unwrap_or("").split(' ');
                    let method = first.next().unwrap_or("").to_string();
                    let target = first.next().unwrap_or("").to_string();
                    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
                    let r = handle(Req {
                        method,
                        path: path.to_string(),
                        query: url::form_urlencoded::parse(q.as_bytes()).into_owned().collect(),
                        auth: header("authorization"),
                        body: String::from_utf8_lossy(&buf[head_end..head_end + len]).into_owned(),
                    });
                    let mut out = format!("HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n", r.status, r.body.len());
                    for (k, v) in r.headers {
                        out.push_str(&format!("{k}: {v}\r\n"));
                    }
                    out.push_str("\r\n");
                    out.push_str(&r.body);
                    let _ = s.write_all(out.as_bytes()).await;
                });
            }
        });
        base
    }

    /// What the fake authorization server and MCP server share.
    #[derive(Default)]
    struct World {
        as_base: String,
        mcp_url: String,
        /// The authorization server offers dynamic registration.
        dcr: bool,
        /// What the browser asked the authorization endpoint.
        authorize: HashMap<String, String>,
        pkce_ok: bool,
        issued: u32,
        /// The one access token the MCP server takes.
        valid: String,
        expires_in: i64,
        reject_refresh: bool,
        unauthorized: u32,
        /// Every Authorization header the MCP server got.
        seen: Vec<String>,
    }

    type W = Arc<Mutex<World>>;

    fn get(m: &HashMap<String, String>, k: &str) -> String {
        m.get(k).cloned().unwrap_or_default()
    }

    fn authorization_server(w: &W, r: Req) -> Resp {
        let mut w = w.lock();
        let issuer = format!("{}/tenant", w.as_base);
        match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/.well-known/oauth-authorization-server/tenant") => {
                let mut m = json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{}/authorize", w.as_base),
                    "token_endpoint": format!("{}/token", w.as_base),
                    "code_challenge_methods_supported": ["S256"],
                });
                if w.dcr {
                    m["registration_endpoint"] = json!(format!("{}/register", w.as_base));
                }
                json_resp(200, m)
            }
            ("POST", "/register") if w.dcr => {
                let v: Value = serde_json::from_str(&r.body).unwrap_or_default();
                let uri = v["redirect_uris"][0].as_str().unwrap_or("").to_string();
                // a loopback redirect, on 127.0.0.1 only, for a public client
                if !(uri.starts_with("http://127.0.0.1:") && uri.ends_with("/callback")) || v["token_endpoint_auth_method"] != "none" {
                    return json_resp(400, json!({"error": "invalid_redirect_uri"}));
                }
                json_resp(201, json!({"client_id": "dyn-1", "redirect_uris": [uri]}))
            }
            ("GET", "/authorize") => {
                let mut to = Url::parse(&get(&r.query, "redirect_uri")).unwrap();
                to.query_pairs_mut().append_pair("code", "code-1").append_pair("state", &get(&r.query, "state")).append_pair("iss", &issuer);
                w.authorize = r.query;
                Resp { status: 302, headers: vec![("location", to.to_string())], body: String::new() }
            }
            ("POST", "/token") => {
                let f: HashMap<String, String> = url::form_urlencoded::parse(r.body.as_bytes()).into_owned().collect();
                let a = w.authorize.clone();
                let ok = match get(&f, "grant_type").as_str() {
                    "authorization_code" => {
                        w.pkce_ok = URL_SAFE_NO_PAD.encode(Sha256::digest(get(&f, "code_verifier").as_bytes())) == get(&a, "code_challenge");
                        w.pkce_ok
                            && get(&f, "code") == "code-1"
                            && get(&f, "redirect_uri") == get(&a, "redirect_uri")
                            && get(&f, "client_id") == get(&a, "client_id")
                            && get(&f, "resource") == w.mcp_url
                    }
                    "refresh_token" => !w.reject_refresh && get(&f, "refresh_token") == format!("rt-{}", w.issued) && get(&f, "resource") == w.mcp_url,
                    _ => false,
                };
                if !ok {
                    return json_resp(400, json!({"error": "invalid_grant"}));
                }
                w.issued += 1;
                w.valid = format!("at-{}", w.issued);
                json_resp(200, json!({"access_token": w.valid, "refresh_token": format!("rt-{}", w.issued), "expires_in": w.expires_in, "token_type": "Bearer"}))
            }
            _ => json_resp(404, json!({})),
        }
    }

    fn mcp_server(w: &W, r: Req) -> Resp {
        let mut w = w.lock();
        let prm = w.mcp_url.replace("/mcp", "/.well-known/oauth-protected-resource/mcp");
        if r.path == "/.well-known/oauth-protected-resource/mcp" {
            return json_resp(200, json!({"resource": w.mcp_url, "authorization_servers": [format!("{}/tenant", w.as_base)], "scopes_supported": ["mcp:read"]}));
        }
        if let Some(a) = &r.auth {
            w.seen.push(a.clone());
        }
        if w.valid.is_empty() || r.auth != Some(format!("Bearer {}", w.valid)) {
            w.unauthorized += 1;
            let challenge = format!("Bearer error=\"invalid_token\", resource_metadata=\"{prm}\", scope=\"mcp:tools\"");
            return Resp { status: 401, headers: vec![("www-authenticate", challenge), ("content-type", "application/json".into())], body: "{}".into() };
        }
        let v: Value = serde_json::from_str(&r.body).unwrap_or_default();
        if v["id"].is_null() {
            return Resp { status: 202, headers: vec![], body: String::new() };
        }
        let result = match v["method"].as_str().unwrap_or("") {
            "initialize" => json!({"protocolVersion": crate::mcp::PROTOCOL_VERSION, "capabilities": {"tools": {}}, "serverInfo": {"name": "web"}}),
            "tools/list" => json!({"tools": [{"name": "echo", "inputSchema": {"type": "object"}}]}),
            "tools/call" => json!({"content": [{"type": "text", "text": v["params"]["arguments"]["text"]}]}),
            _ => json!({}),
        };
        json_resp(200, json!({"jsonrpc": "2.0", "id": v["id"], "result": result}))
    }

    async fn fake_servers(dcr: bool) -> W {
        let w: W = Arc::new(Mutex::new(World { dcr, expires_in: 3600, ..Default::default() }));
        let wa = w.clone();
        let as_base = serve(move |r| authorization_server(&wa, r)).await;
        let wm = w.clone();
        let mcp_base = serve(move |r| mcp_server(&wm, r)).await;
        let mut g = w.lock();
        g.as_base = as_base;
        g.mcp_url = format!("{mcp_base}/mcp");
        drop(g);
        w
    }

    /// The user's browser: it opens the page, which sends it back to the app.
    fn browser() -> impl FnOnce(&str) -> AppResult<()> {
        |u: &str| {
            let u = u.to_string();
            tokio::spawn(async move {
                let _ = client().get(u).send().await;
            });
            Ok(())
        }
    }

    #[tokio::test]
    async fn sign_in_refresh_and_sign_out_against_fake_servers() {
        std::env::set_var("NO_PROXY", "127.0.0.1");
        let w = fake_servers(true).await;
        let url = w.lock().mcp_url.clone();
        let dir = std::env::temp_dir().join(format!("mcp-oauth-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, json!({"mcpServers": {"web": {"url": url}}}).to_string()).unwrap();
        let m = McpManager::new(cfg);
        m.start_all(&[]).await;
        let st = m.statuses().await.remove(0);
        assert_eq!((st.status.as_str(), st.signed_in), ("needs_auth", false), "{st:?}");

        let t = sign_in(&client(), &url, None, browser(), Duration::from_secs(20)).await.unwrap();
        {
            let w = w.lock();
            let a = &w.authorize;
            assert!(w.pkce_ok);
            assert_eq!(get(a, "code_challenge_method"), "S256");
            assert_eq!(get(a, "response_type"), "code");
            assert_eq!(get(a, "client_id"), "dyn-1", "registered by itself");
            assert_eq!(get(a, "resource"), url);
            assert_eq!(get(a, "scope"), "mcp:tools", "the challenge's scope");
            assert!(get(a, "redirect_uri").starts_with("http://127.0.0.1:"));
        }
        assert_eq!((t.access_token.as_str(), t.refresh_token.as_deref()), ("at-1", Some("rt-1")));
        let key = token_key("web", &url);
        save(&key, &t).unwrap();
        m.restart("web", &[]).await;
        let st = m.statuses().await.remove(0);
        assert_eq!((st.status.as_str(), st.signed_in), ("connected", true), "{st:?}");
        let (b, _) = m.call("web__echo", &json!({"text": "hi"})).await.unwrap();
        assert_eq!(b[0]["text"], "hi");

        // the server stops taking the token: refreshed after its 401, the call goes again
        w.lock().valid = "revoked".into();
        let before = w.lock().unauthorized;
        let (b, _) = m.call("web__echo", &json!({"text": "again"})).await.unwrap();
        assert_eq!(b[0]["text"], "again");
        assert_eq!(w.lock().unauthorized, before + 1);
        assert_eq!(load(&key).unwrap().access_token, "at-2");

        // a token about to expire is refreshed before it is sent: no 401
        let mut t = load(&key).unwrap();
        t.expires_at = Some(now_secs());
        save(&key, &t).unwrap();
        let sent = w.lock().seen.len();
        m.restart("web", &[]).await;
        assert_eq!(m.statuses().await[0].status, "connected");
        assert_eq!(w.lock().unauthorized, before + 1);
        assert_eq!(load(&key).unwrap().access_token, "at-3");
        assert!(w.lock().seen[sent..].iter().all(|a| a == "Bearer at-3"), "{:?}", &w.lock().seen[sent..]);

        // no token or code in anything the UI gets (status, errors, logs)
        let shown = serde_json::to_string(&m.statuses().await).unwrap();
        for secret in ["at-1", "at-2", "at-3", "rt-1", "rt-2", "rt-3", "code-1"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }

        // a refresh token the server no longer takes ends the sign-in
        {
            let mut w = w.lock();
            w.valid = "revoked again".into();
            w.reject_refresh = true;
        }
        let e = m.call("web__echo", &json!({"text": "x"})).await.unwrap_err();
        assert_eq!(e.kind(), "needs_auth", "{e}");
        assert!(!e.to_string().contains("at-3"));
        let st = m.statuses().await.remove(0);
        assert_eq!((st.status.as_str(), st.signed_in), ("needs_auth", false), "{st:?}");
        assert!(load(&key).is_none());

        // sign in again, then out: the tokens are gone, and so is the connection
        w.lock().reject_refresh = false;
        let t = sign_in(&client(), &url, None, browser(), Duration::from_secs(20)).await.unwrap();
        save(&key, &t).unwrap();
        m.restart("web", &[]).await;
        assert_eq!(m.statuses().await[0].status, "connected");
        forget(&key);
        m.restart("web", &[]).await;
        let st = m.statuses().await.remove(0);
        assert_eq!((st.status.as_str(), st.signed_in), ("needs_auth", false), "{st:?}");
        m.stop_all().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn without_registration_the_configured_client_id_is_used() {
        let w = fake_servers(false).await;
        let url = w.lock().mcp_url.clone();
        let e = sign_in(&client(), &url, None, browser(), Duration::from_secs(20)).await.unwrap_err();
        assert!(e.to_string().contains("clientId"), "{e}");
        let t = sign_in(&client(), &url, Some("pre-1"), browser(), Duration::from_secs(20)).await.unwrap();
        assert_eq!(t.client_id, "pre-1");
        assert_eq!(get(&w.lock().authorize, "client_id"), "pre-1");
    }

    #[tokio::test]
    async fn metadata_for_another_resource_or_issuer_is_refused() {
        let cell: Arc<Mutex<String>> = Default::default();
        let b = cell.clone();
        let base = serve(move |r| {
            let base = b.lock().clone();
            let meta = |issuer: String, auth: &str, methods: Value| {
                json_resp(200, json!({"issuer": issuer, "authorization_endpoint": auth, "token_endpoint": "https://a.example/t", "code_challenge_methods_supported": methods}))
            };
            match r.path.as_str() {
                "/.well-known/oauth-protected-resource/mcp" => json_resp(200, json!({"resource": "http://127.0.0.1:1/mcp", "authorization_servers": ["https://a.example"]})),
                "/.well-known/oauth-authorization-server" => meta("https://evil.example".into(), "https://a.example/a", json!(["S256"])),
                "/.well-known/oauth-authorization-server/plain" => meta(format!("{base}/plain"), "https://a.example/a", json!(["plain"])),
                "/.well-known/oauth-authorization-server/http" => meta(format!("{base}/http"), "http://a.example/a", json!(["S256"])),
                "/.well-known/oauth-authorization-server/good" => meta(format!("{base}/good/"), "https://a.example/a", Value::Null),
                "/register" => json_resp(201, json!({"client_id": "c", "redirect_uris": ["https://evil.example/callback"]})),
                _ => json_resp(404, json!({})),
            }
        })
        .await;
        *cell.lock() = base.clone();
        let c = client();
        let server = Url::parse(&format!("{base}/mcp")).unwrap();
        assert!(resource_metadata(&c, &server, None, true).await.unwrap_err().to_string().contains("not for"));
        // an insecure metadata address in the challenge isn't followed
        assert!(resource_metadata(&c, &server, Some(r#"Bearer resource_metadata="http://evil.example/m""#), true).await.is_err());
        assert!(auth_server_metadata(&c, &base, true).await.unwrap_err().to_string().contains("issuer"));
        assert!(auth_server_metadata(&c, &format!("{base}/plain"), true).await.unwrap_err().to_string().contains("S256"));
        assert!(auth_server_metadata(&c, &format!("{base}/http"), true).await.unwrap_err().to_string().contains("only https"));
        assert!(auth_server_metadata(&c, &format!("{base}/good"), true).await.is_ok());
        assert!(auth_server_metadata(&c, &format!("{base}/none"), true).await.unwrap_err().to_string().contains("No OAuth metadata"));
        // a registration for some other redirect address is not used
        let e = register(&c, &format!("{base}/register"), "http://127.0.0.1:1/callback", true).await.unwrap_err();
        assert!(e.to_string().contains("different redirect"), "{e}");
    }

    #[test]
    fn internal_addresses_are_known() {
        for a in [
            "http://127.0.0.1:9/", "https://10.1.2.3/", "https://172.16.0.1/", "https://192.168.1.1/", "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/", "https://0.0.0.0/", "https://[::1]/", "https://[fd00::1]/", "https://[fe80::1]/", "https://[::ffff:10.0.0.1]/",
            "http://localhost/", "https://app.LOCALHOST/", "https://2130706433/",
        ] {
            assert!(internal(&Url::parse(a).unwrap()), "{a}");
        }
        for a in ["https://8.8.8.8/", "https://auth.example/", "https://[2606:4700::1111]/", "https://172.32.0.1/", "https://100.128.0.1/"] {
            assert!(!internal(&Url::parse(a).unwrap()), "{a}");
        }
    }

    #[tokio::test]
    async fn a_server_on_the_internet_cannot_lead_the_app_into_the_network() {
        for to in ["https://169.254.169.254/latest/meta-data", "http://127.0.0.1:8080/admin", "https://192.168.1.1/x", "http://localhost/x"] {
            assert!(allowed(to, false).unwrap_err().to_string().contains("private network"), "{to}");
        }
        // a server on this machine or the user's network may use its neighbours
        assert!(allowed("http://127.0.0.1:8080/token", true).is_ok() && allowed("https://auth.example/token", false).is_ok());
        // so the metadata of a public server can't send the app there, before anything is fetched
        let c = client();
        let server = Url::parse("https://mcp.example/mcp").unwrap();
        let e = resource_metadata(&c, &server, Some(r#"Bearer resource_metadata="https://169.254.169.254/latest/meta-data""#), false).await.unwrap_err();
        assert!(e.to_string().contains("private network"), "{e}");
        assert!(auth_server_metadata(&c, "https://10.0.0.1/tenant", false).await.unwrap_err().to_string().contains("private network"));
        assert!(register(&c, "http://127.0.0.1:1/register", "http://127.0.0.1:2/callback", false).await.unwrap_err().to_string().contains("private network"));
    }

    fn chain(e: &dyn std::error::Error) -> String {
        let mut out = e.to_string();
        let mut cur = e.source();
        while let Some(s) = cur {
            out.push_str(&format!(": {s}"));
            cur = s.source();
        }
        out
    }

    #[tokio::test]
    async fn a_public_servers_client_reaches_public_addresses_only() {
        let base = serve(|_| json_resp(200, json!({}))).await;
        let port = base.rsplit(':').next().unwrap().to_string();
        // .example names never resolve, so this server counts as one on the internet
        let c = client_for("https://mcp.example/mcp").await.unwrap();
        let e = c.get(format!("http://localhost:{port}/")).send().await.unwrap_err();
        assert!(chain(&e).contains("not a public address"), "{}", chain(&e));
        // a server on this machine reaches its neighbours
        let c = client_for(&format!("{base}/mcp")).await.unwrap();
        assert_eq!(c.get(format!("http://localhost:{port}/")).send().await.unwrap().status(), 200);
    }

    #[tokio::test]
    async fn redirects_stay_on_their_origin() {
        let other = serve(|_| json_resp(200, json!({"elsewhere": true}))).await;
        let to = format!("{other}/token");
        let base = serve(move |r| match r.path.as_str() {
            "/away" => Resp { status: 307, headers: vec![("location", to.clone())], body: String::new() },
            "/here" => Resp { status: 307, headers: vec![("location", "/token".into())], body: String::new() },
            _ => json_resp(200, json!({})),
        })
        .await;
        let c = client_for(&format!("{base}/mcp")).await.unwrap();
        // a token request sent on to another origin would take its form along
        assert_eq!(c.post(format!("{base}/away")).form(&[("refresh_token", "rt")]).send().await.unwrap().status(), 307);
        assert_eq!(c.post(format!("{base}/here")).form(&[("refresh_token", "rt")]).send().await.unwrap().status(), 200);
    }
}
