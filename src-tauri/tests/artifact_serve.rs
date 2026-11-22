//! Artifact previews through the app's public API: `serve_artifact` builds the
//! sandboxed document (`artifacts::build_document`) and keeps it under a new
//! token for the `artifact://` protocol, the newest 64 at most.

use clauderust_lib::artifacts::build_document;
use clauderust_lib::commands;
use clauderust_lib::db::Db;
use clauderust_lib::state::AppState;
use std::path::PathBuf;
use std::sync::{Arc, Once};
use tauri::test::MockRuntime;
use tauri::Manager;

const KINDS: [&str; 4] = ["text/html", "image/svg+xml", "application/vnd.ant.mermaid", "application/vnd.ant.react"];

fn app() -> (tauri::App<MockRuntime>, Arc<AppState>) {
    static ENV: Once = Once::new();
    // a fake key: the app's keyring is never read
    ENV.call_once(|| std::env::set_var("CLAUDERUST_API_KEY", "test-key"));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("artifact-serve");
    let st = Arc::new(AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
    let app = tauri::test::mock_app();
    app.manage(st.clone());
    (app, st)
}

fn serve(app: &tauri::App<MockRuntime>, kind: &str, content: &str, dark: bool) -> String {
    tauri::async_runtime::block_on(commands::serve_artifact(app.state(), kind.into(), content.into(), dark)).unwrap()
}

#[test]
fn a_served_artifact_is_its_built_document_under_a_new_token() {
    let (app, st) = app();
    let mut tokens = vec![];
    for kind in KINDS {
        for dark in [false, true] {
            let content = "<p>hello</p>";
            let token = serve(&app, kind, content, dark);
            let served = st.served.lock();
            let (mime, doc) = &served[&token];
            assert_eq!(mime, "text/html; charset=utf-8");
            assert_eq!(String::from_utf8_lossy(doc), build_document(kind, content, dark), "{kind}, dark: {dark}");
            tokens.push(token);
        }
    }
    let n = tokens.len();
    tokens.sort();
    tokens.dedup();
    assert_eq!(tokens.len(), n, "every document has a token of its own");
}

#[test]
fn the_newest_64_documents_stay_served() {
    let (app, st) = app();
    let tokens: Vec<String> = (0..70).map(|i| serve(&app, "text/html", &format!("<p>{i}</p>"), false)).collect();
    let served = st.served.lock();
    assert_eq!(served.len(), 64);
    assert!(tokens[..6].iter().all(|t| !served.contains_key(t)), "the oldest went first");
    assert!(tokens[6..].iter().all(|t| served.contains_key(t)), "the newer ones, an open frame's among them, still load");
}

#[test]
fn artifact_source_cannot_break_out_of_its_document() {
    // React source is data in a script element, Mermaid source text in a <pre>:
    // neither element can be closed from inside
    for (kind, attack, breakout) in [
        ("application/vnd.ant.react", "export default () => '</script><script>window.pwned = 1</script>'", "</script><script>window.pwned"),
        ("application/vnd.ant.mermaid", "graph TD; A-->B</pre><script>window.pwned = 1</script>", "</pre><script>window.pwned"),
    ] {
        for dark in [false, true] {
            assert!(!build_document(kind, attack, dark).contains(breakout), "{kind}: the source escaped its element");
        }
    }
    // an HTML fragment is the page itself, in a complete document
    for dark in [false, true] {
        let doc = build_document("text/html", "<h1>Title</h1><script>document.title = 'ran'</script>", dark);
        assert!(doc.to_ascii_lowercase().starts_with("<!doctype html>"), "{doc}");
        assert!(doc.contains("<h1>Title</h1><script>document.title = 'ran'</script>"));
        assert!(doc.trim_end().ends_with("</html>"));
    }
}
