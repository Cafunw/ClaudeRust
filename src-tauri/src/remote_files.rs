//! Files fetched from the web for a chat or a project (github_source.rs,
//! link_source.rs): a client that goes only to the service's own hosts,
//! capped reads, cleaned names, and the hand-over as attachments.

use crate::attachments::Attachment;
use crate::db::ProjectFile;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// Where a service's requests may go: https to its own hosts (`*.x` is any
/// subdomain of x), and the mock standing in for them in tests and debug builds.
#[derive(Clone)]
pub(crate) struct Fence {
    hosts: &'static [&'static str],
    mock: Option<url::Origin>,
}

impl Fence {
    pub(crate) fn new(hosts: &'static [&'static str], mock: Option<&Url>) -> Fence {
        Fence { hosts, mock: mock.map(Url::origin) }
    }

    pub(crate) fn allows(&self, u: &Url) -> bool {
        if self.mock.as_ref() == Some(&u.origin()) {
            return true;
        }
        let Some(url::Host::Domain(host)) = u.host() else { return false };
        let ours = self.hosts.iter().any(|p| match p.strip_prefix('*') {
            Some(suffix) => host.ends_with(suffix),
            None => host == *p,
        });
        ours && u.scheme() == "https" && u.port().is_none() && u.username().is_empty() && u.password().is_none()
    }
}

/// A client that follows at most 5 redirects, each only to an address the
/// fence allows: at any other it stops, and the caller gets that redirect.
pub(crate) fn client(fence: &Fence) -> AppResult<reqwest::Client> {
    let f = fence.clone();
    let mut b = reqwest::Client::builder()
        .user_agent("ClaudeRust")
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(move |a| if a.previous().len() < 5 && f.allows(a.url()) { a.follow() } else { a.stop() }));
    // the mock is on this machine: never through a proxy
    if fence.mock.is_some() {
        b = b.no_proxy();
    }
    Ok(b.build()?)
}

/// `base` with `segments` appended, each escaped as a path segment needs.
pub(crate) fn under<'a>(base: &Url, segments: impl IntoIterator<Item = &'a str>) -> Url {
    let mut u = base.clone();
    u.path_segments_mut().expect("an http(s) URL").pop_if_empty().extend(segments);
    u
}

/// The body of `r`, or None when it is over `cap` bytes (it stops reading there).
pub(crate) async fn body_capped(mut r: reqwest::Response, cap: usize) -> AppResult<Option<Vec<u8>>> {
    if r.content_length().is_some_and(|n| n > cap as u64) {
        return Ok(None);
    }
    let mut out = Vec::new();
    while let Some(c) = r.chunk().await? {
        if out.len() + c.len() > cap {
            return Ok(None);
        }
        out.extend_from_slice(&c);
    }
    Ok(Some(out))
}

const MAX_NAME: usize = 200;

/// A name from elsewhere made safe to show and store: without control or
/// direction-changing characters, at most `MAX_NAME` characters, its end
/// (with the extension, which decides the type) kept.
pub(crate) fn clean_name(raw: &str) -> String {
    let s: String = raw.chars().filter(|&c| !c.is_control() && !matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')).collect();
    let s = s.trim();
    s.chars().skip(s.chars().count().saturating_sub(MAX_NAME)).collect()
}

/// `s` with its %XX escapes decoded; None when one is broken or the result isn't UTF-8.
pub(crate) fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Hand `a` to the chat composer (staged, as `stage_paths` does) or add it to
/// the knowledge of project `project_id`; what the UI shows of it.
fn deliver(st: &AppState, project_id: Option<&str>, a: Attachment) -> AppResult<Value> {
    let Some(project_id) = project_id else {
        let v = serde_json::to_value(&a)?;
        st.staged.lock().insert(a.id.clone(), a);
        return Ok(v);
    };
    let f = ProjectFile {
        id: crate::util::new_id(),
        project_id: project_id.into(),
        name: a.name,
        mime: a.mime,
        size: a.size as i64,
        kind: a.kind,
        data: a.data,
        created_at: crate::util::now_ms(),
    };
    st.db.project_file_add(&f)?;
    Ok(serde_json::to_value(&f)?)
}

/// Attachments made with `attach` from `files` (a name, and its bytes or why
/// they couldn't be fetched), handed over (see `deliver`) on the blocking
/// pool: `{added, errors}`, each error with its kind and file name.
pub(crate) async fn hand_over(
    st: Arc<AppState>,
    project_id: Option<String>,
    files: Vec<(String, AppResult<Vec<u8>>)>,
    attach: fn(&str, &[u8]) -> AppResult<Attachment>,
) -> AppResult<Value> {
    if let Some(p) = &project_id {
        st.db.project_get(p)?;
    }
    tokio::task::spawn_blocking(move || {
        let mut added = vec![];
        let mut errors = vec![];
        for (name, bytes) in files {
            match bytes.and_then(|b| attach(&name, &b)).and_then(|a| deliver(&st, project_id.as_deref(), a)) {
                Ok(v) => added.push(v),
                Err(e) => errors.push(json!({"kind": e.kind(), "message": e.to_string(), "name": name})),
            }
        }
        Ok(json!({"added": added, "errors": errors}))
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))?
}

/// A local HTTP server for the tests of the sources.
#[cfg(test)]
pub(crate) mod mock {
    use parking_lot::Mutex;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A request as the mock got it.
    #[derive(Debug, Clone)]
    pub struct Seen {
        /// path and query
        pub path: String,
        pub auth: Option<String>,
        pub accept: Option<String>,
    }

    /// status, headers, body
    pub type Reply = (u16, Vec<(&'static str, String)>, Vec<u8>);

    /// Serve every request with `answer`: the server's base URL, and what it was asked.
    pub async fn serve(answer: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> (url::Url, Arc<Mutex<Vec<Seen>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}", l.local_addr().unwrap())).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (answer, log) = (Arc::new(answer), seen.clone());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let (answer, log) = (answer.clone(), log.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf).to_string();
                    let header = |name: &str| head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()));
                    let req = Seen { path: head.split_whitespace().nth(1).unwrap_or("").to_string(), auth: header("authorization"), accept: header("accept") };
                    log.lock().push(req.clone());
                    let (status, headers, body) = answer(&req);
                    let mut out = format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
                    for (k, v) in headers {
                        out += &format!("{k}: {v}\r\n");
                    }
                    out += "\r\n";
                    let _ = s.write_all(out.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                });
            }
        });
        (base, seen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fence_lets_through_only_its_hosts_over_https() {
        let mock = Url::parse("http://127.0.0.1:4100").unwrap();
        let f = Fence::new(&["drive.google.com", "*.googleusercontent.com"], Some(&mock));
        let ok = |s: &str| f.allows(&Url::parse(s).unwrap());
        assert!(ok("https://drive.google.com/uc?id=x"));
        assert!(ok("https://doc-0s-1c-docs.googleusercontent.com/x"));
        assert!(ok("http://127.0.0.1:4100/anything"), "the mock");
        for bad in [
            "http://drive.google.com/x",
            "https://drive.google.com:8443/x",
            "https://user:pw@drive.google.com/x",
            "https://drive.google.com.evil.example/x",
            "https://evil.example/drive.google.com/x",
            "https://googleusercontent.com.evil.example/x",
            "https://evilgoogleusercontent.com/x",
            "https://142.250.0.1/x",
            "https://[::1]/x",
            "http://127.0.0.1:4101/x",
            "http://localhost:4100/x",
        ] {
            assert!(!ok(bad), "{bad}");
        }
        assert!(!Fence::new(&["drive.google.com"], None).allows(&mock));
    }

    #[test]
    fn names_are_cleaned_and_keep_their_extension() {
        assert_eq!(clean_name("  report\u{202E}fdp.exe \n"), "reportfdp.exe");
        assert_eq!(clean_name("a\u{0}b\u{7}\u{2066}c.md"), "abc.md");
        let long = format!("{}.rs", "x".repeat(300));
        assert_eq!(clean_name(&long).chars().count(), MAX_NAME);
        assert!(clean_name(&long).ends_with("x.rs"));
        assert_eq!(clean_name("Ayşe’nin notları.md"), "Ayşe’nin notları.md");
    }

    #[test]
    fn escapes_are_decoded() {
        assert_eq!(unescape("my%20dir").as_deref(), Some("my dir"));
        assert_eq!(unescape("%C3%A7ay").as_deref(), Some("çay"));
        assert_eq!(unescape("a+b").as_deref(), Some("a+b"));
        assert_eq!(unescape("bad%2"), None);
        assert_eq!(unescape("bad%zz"), None);
        assert_eq!(unescape("%FF"), None, "not UTF-8");
    }

    #[tokio::test]
    async fn a_body_over_the_cap_is_not_read_whole() {
        let (base, _) = mock::serve(|_| (200, vec![], vec![b'a'; 5000])).await;
        let c = client(&Fence::new(&[], Some(&base))).unwrap();
        let r = c.get(base.clone()).send().await.unwrap();
        assert_eq!(body_capped(r, 5000).await.unwrap().map(|b| b.len()), Some(5000));
        let r = c.get(base).send().await.unwrap();
        assert_eq!(body_capped(r, 4999).await.unwrap(), None);
    }

    #[tokio::test]
    async fn files_go_to_the_composer_or_to_a_project() {
        let dir = std::env::temp_dir().join(format!("clauderust-remote-{}", crate::util::new_id()));
        let st = Arc::new(AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
        let attach: fn(&str, &[u8]) -> AppResult<Attachment> = |n, b| crate::attachments::from_bytes(n, b);
        let files = || vec![("a.md".to_string(), Ok(b"# A".to_vec())), ("b.bin".to_string(), Ok(vec![0, 1, 2, 3])), ("c.md".to_string(), Err(AppError::msg("gone")))];
        let r = hand_over(st.clone(), None, files(), attach).await.unwrap();
        assert_eq!(r["added"].as_array().unwrap().len(), 1);
        assert!(st.staged.lock().contains_key(r["added"][0]["id"].as_str().unwrap()));
        assert_eq!(r["errors"][0]["kind"], "unsupported_file");
        assert_eq!(r["errors"][0]["name"], "b.bin");
        assert_eq!(r["errors"][1]["message"], "gone");
        let p = st.db.project_create("P", "").unwrap();
        let r = hand_over(st.clone(), Some(p.id.clone()), files(), attach).await.unwrap();
        assert_eq!(r["added"][0]["name"], "a.md");
        assert_eq!(st.db.project_files(&p.id).unwrap().len(), 1);
        assert_eq!(st.staged.lock().len(), 1, "nothing more staged");
        assert!(hand_over(st, Some("no-such-project".into()), files(), attach).await.is_err());
    }
}
