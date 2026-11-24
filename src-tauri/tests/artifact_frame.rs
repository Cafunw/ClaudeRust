//! The webview runs the app's start-up scripts in every frame, artifact frames
//! too (Windows ignores "main frame only"): their confirm returns a Promise,
//! their alert asks the app, their click handler opens links. Every served
//! artifact page runs src-tauri/src/artifact_guard.js first, which undoes that
//! (its behaviour: ui/test/artifact_hook.test.mjs).

use clauderust_lib::artifacts::ERROR_HOOK;
use clauderust_lib::commands;
use clauderust_lib::db::Db;
use clauderust_lib::state::AppState;
use std::path::PathBuf;
use std::sync::{Arc, Once};
use tauri::Manager;

#[test]
fn every_served_page_runs_the_hook_before_any_script_of_its_own() {
    static ENV: Once = Once::new();
    // a fake key: the app's keyring is never read
    ENV.call_once(|| std::env::set_var("CLAUDERUST_API_KEY", "test-key"));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("artifact-frame");
    let st = Arc::new(AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
    let app = tauri::test::mock_app();
    app.manage(st.clone());
    let page_script = "<script>if (confirm('Delete?')) document.title = 'deleted'</script>";
    for (kind, content) in [
        ("text/html", format!("<p>x</p>{page_script}")),
        ("text/html", format!("<!doctype html><html><head>{page_script}</head><body></body></html>")),
        ("image/svg+xml", format!("<svg xmlns=\"http://www.w3.org/2000/svg\"><a href=\"https://example.com\"><text>x</text></a>{page_script}</svg>")),
        ("application/vnd.ant.mermaid", "graph TD; A-->B".to_string()),
        ("application/vnd.ant.react", "export default () => { if (confirm('x')) return null; return null; }".to_string()),
        ("application/vnd.ant.slides", format!("<section><h1>x</h1>{page_script}</section>")),
        ("application/vnd.ant.document", format!("# x\n\n{page_script}")),
        ("application/vnd.ant.sheet", "a,b\n1,=A2+1".to_string()),
        ("application/vnd.ant.dashboard", "{\"kpis\": []}".to_string()),
    ] {
        for dark in [false, true] {
            let token = tauri::async_runtime::block_on(commands::serve_artifact(app.state(), kind.into(), content.clone(), dark)).unwrap();
            let doc = String::from_utf8(st.served.lock()[&token].1.clone()).unwrap();
            let hook = doc.find(ERROR_HOOK).unwrap_or_else(|| panic!("{kind}: no hook"));
            assert_eq!(doc.find("<script"), Some(hook), "{kind}: a script runs before the hook");
        }
    }
    for undo in ["window.confirm = function () { return false; };", "window.alert = function () {};", "window.open = function (url)", "__artifact: 'link'"] {
        assert!(ERROR_HOOK.contains(undo), "{undo}");
    }
}
