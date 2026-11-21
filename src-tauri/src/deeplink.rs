//! `clauderust://` links (`claude://` is Claude Desktop's). The OS starts the
//! app with the link as its only argument, or the single-instance plugin
//! hands it to the running app. A link only opens something in the window:
//!
//! - `clauderust://chat/<conversation id>`: that conversation
//! - `clauderust://new?prompt=<text>`: a new chat, `prompt` (optional) in its composer
//! - `clauderust://code?prompt=<text>`: the same for a new code session
//! - `clauderust://settings`: Settings
//!
//! Nothing is sent or run: the prompt only waits in the composer. Any other
//! link, or one with anything more (another parameter, a port, a fragment),
//! is ignored. The route waits here until the window takes it
//! ([`deep_link_take`]), on the `deep-link` event or when the page starts, so
//! the link that started the app isn't lost while the page loads.

use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Runtime};

const SCHEME: &str = "clauderust";
const PROMPT_MAX: usize = 10_000;

static PENDING: Mutex<Option<Value>> = Mutex::new(None);

/// What the window should do for `url`, or why it is ignored.
fn route(url: &url::Url) -> Result<Value, &'static str> {
    if url.scheme() != SCHEME {
        return Err("another scheme");
    }
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() || url.fragment().is_some() {
        return Err("unexpected parts");
    }
    let path = url.path();
    let query: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    match url.host_str().unwrap_or("") {
        "chat" => {
            let id = path.strip_prefix('/').unwrap_or("");
            let valid = (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            if !valid || !query.is_empty() {
                return Err("bad conversation link");
            }
            Ok(json!({"route": "chat", "id": id}))
        }
        kind @ ("new" | "code") => {
            if !matches!(path, "" | "/") || query.len() > 1 || query.iter().any(|(k, _)| k != "prompt") {
                return Err("bad new chat link");
            }
            let prompt = query.first().map(|(_, v)| v.as_str()).unwrap_or("");
            if prompt.chars().count() > PROMPT_MAX || prompt.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
                return Err("bad prompt");
            }
            Ok(json!({"route": kind, "prompt": prompt}))
        }
        "settings" if matches!(path, "" | "/") && query.is_empty() => Ok(json!({"route": "settings"})),
        _ => Err("unknown link"),
    }
}

/// Keeps the route of the last valid link in `urls` for the window and tells it.
pub fn open<R: Runtime>(app: &AppHandle<R>, urls: &[url::Url]) {
    for u in urls {
        match route(u) {
            Ok(r) => {
                log::info!("deep link: {}", r["route"]);
                *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                let _ = app.emit_to("main", "deep-link", ());
            }
            // the link itself isn't logged: it may carry a prompt
            Err(why) => log::warn!("deep link ignored: {why}"),
        }
    }
}

/// The route of the last link, once (null when there is none).
#[tauri::command]
pub fn deep_link_take() -> Option<Value> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Result<Value, &'static str> {
        route(&s.parse().unwrap())
    }

    #[test]
    fn known_links() {
        let id = "6f1c2d3e-0000-4abc-9def-0123456789ab";
        assert_eq!(r(&format!("clauderust://chat/{id}")), Ok(json!({"route": "chat", "id": id})));
        assert_eq!(r("clauderust://new"), Ok(json!({"route": "new", "prompt": ""})));
        assert_eq!(r("clauderust://new/?prompt=Hi%20%C3%A7%0Asecond%20line"), Ok(json!({"route": "new", "prompt": "Hi ç\nsecond line"})));
        assert_eq!(r("clauderust://code?prompt=fix+the+tests"), Ok(json!({"route": "code", "prompt": "fix the tests"})));
        assert_eq!(r("clauderust://settings"), Ok(json!({"route": "settings"})));
        assert_eq!(r("CLAUDERUST://settings/"), Ok(json!({"route": "settings"})));
    }

    #[test]
    fn everything_else_is_ignored() {
        let long = format!("clauderust://new?prompt={}", "a".repeat(PROMPT_MAX + 1));
        for bad in [
            "claude://new",
            "https://chat/x",
            "clauderust:new",
            "clauderust://Chat/abc",
            "clauderust://chat/",
            "clauderust://chat/a/b",
            "clauderust://chat/..%2Fx",
            "clauderust://chat/abc?prompt=x",
            &format!("clauderust://chat/{}", "a".repeat(65)),
            "clauderust://new?prompt=a&prompt=b",
            "clauderust://new?send=1",
            "clauderust://new?prompt=a&cwd=C:%5C",
            "clauderust://new/extra?prompt=a",
            "clauderust://new?prompt=a%00b",
            "clauderust://new?prompt=a%1Bb",
            &long,
            "clauderust://new#x",
            "clauderust://user:pw@new",
            "clauderust://new:8080",
            "clauderust://settings?tab=x",
            "clauderust://settings/general",
            "clauderust://install?url=x",
            "clauderust://",
        ] {
            assert!(r(bad).is_err(), "{bad}");
        }
        assert!(r(&format!("clauderust://new?prompt={}", "ğ".repeat(PROMPT_MAX))).is_ok());
    }

    #[test]
    fn the_scheme_is_the_one_registered() {
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["plugins"]["deep-link"]["desktop"]["schemes"], json!([SCHEME]));
    }

    #[test]
    fn the_window_takes_the_last_valid_route_once() {
        let app = tauri::test::mock_app();
        let urls: Vec<url::Url> = ["clauderust://new?prompt=a", "clauderust://settings", "clauderust://nope"].iter().map(|s| s.parse().unwrap()).collect();
        open(app.handle(), &urls);
        assert_eq!(deep_link_take(), Some(json!({"route": "settings"})));
        assert_eq!(deep_link_take(), None);
    }
}
