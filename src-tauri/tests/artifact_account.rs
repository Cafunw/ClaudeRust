//! window.claude.complete in a chat on the Claude account: the artifact's prompt goes to one call of the
//! Claude Code CLI (as titles do), against a stand-in CLI that logs what it was given and answers from a
//! file. No model runs and nothing is spent.

use clauderust_lib::artifact_ai::{self, SYSTEM_PROMPT};
use clauderust_lib::db::Db;
use clauderust_lib::state::AppState;
use clauderust_lib::util::new_id;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Once};
use tauri::Manager;

/// Reads its input to the end, logs it with its arguments and the limits it was started with, and
/// writes `reply.json` as the CLI's `--output-format json` would.
const FAKE_CLI: &str = r#"
const fs = require("fs");
const path = require("path");
let input = "";
process.stdin.on("data", (d) => (input += d)).on("end", () => {
  const env = { out: process.env.CLAUDE_CODE_MAX_OUTPUT_TOKENS, thinking: process.env.MAX_THINKING_TOKENS };
  fs.appendFileSync(path.join(__dirname, "log.jsonl"), JSON.stringify({ argv: process.argv.slice(2), input, env }) + "\n");
  process.stdout.write(fs.readFileSync(path.join(__dirname, "reply.json"), "utf8"));
});
"#;

struct Harness {
    app: tauri::App<tauri::test::MockRuntime>,
    st: Arc<AppState>,
    cli: PathBuf,
    conv: String,
}

impl Harness {
    fn new() -> Harness {
        static ENV: Once = Once::new();
        ENV.call_once(|| {
            // a fake key: the app's keyring is never read
            std::env::set_var("CLAUDERUST_API_KEY", "test-key");
        });
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("artifact-account-{}", new_id()));
        let cli = dir.join("cli");
        std::fs::create_dir_all(&cli).unwrap();
        std::fs::write(cli.join("fake.cjs"), FAKE_CLI).unwrap();
        #[cfg(windows)]
        let (program, shim) = (cli.join("claude.cmd"), "@ECHO off\r\nSET dp0=%~dp0\r\nnode \"%dp0%\\fake.cjs\" %*\r\n");
        #[cfg(not(windows))]
        let (program, shim) = (cli.join("claude"), "#!/bin/sh\nexec node \"$(dirname \"$0\")/fake.cjs\" \"$@\"\n");
        std::fs::write(&program, shim).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let st = Arc::new(AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
        {
            let mut s = st.settings.write();
            s.claude_code_path = program.display().to_string();
            // chats run on the Claude account, even with a key
            s.chat_engine = "claude-code".into();
        }
        let app = tauri::test::mock_app();
        app.manage(st.clone());
        let conv = st.db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap().id;
        Harness { app, st, cli, conv }
    }

    fn reply(&self, v: Value) {
        std::fs::write(self.cli.join("reply.json"), v.to_string()).unwrap();
    }

    fn complete(&self, artifact: &str, prompt: &str) -> Result<String, String> {
        tauri::async_runtime::block_on(artifact_ai::artifact_complete(self.app.state(), self.conv.clone(), artifact.into(), prompt.into())).map_err(|e| e.to_string())
    }

    fn runs(&self) -> Vec<Value> {
        std::fs::read_to_string(self.cli.join("log.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

fn has(argv: &Value, words: &[&str]) -> bool {
    let a: Vec<&str> = argv.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    a.windows(words.len()).any(|w| w == words)
}

#[test]
fn an_account_chat_asks_the_cli_with_the_apps_model_and_system_prompt() {
    let h = Harness::new();
    let check = tauri::async_runtime::block_on(artifact_ai::artifact_complete_check(h.app.state(), h.conv.clone())).unwrap();
    assert_eq!(check, "account", "the dialog says it runs on the account");
    h.reply(json!({"type": "result", "subtype": "success", "is_error": false, "result": "Paris"}));
    let prompt = "--model opus\nWhat is the capital of France? Answer in one word.";
    assert_eq!(h.complete("quiz", prompt).unwrap(), "Paris");
    let runs = h.runs();
    assert_eq!(runs.len(), 1);
    let r = &runs[0];
    // the prompt goes in on stdin, never as an argument: the frame chooses neither model nor system prompt
    assert_eq!(r["input"], prompt);
    assert!(!r["argv"].as_array().unwrap().iter().any(|a| a.as_str().unwrap().contains("capital")));
    assert!(has(&r["argv"], &["--model", "haiku"]), "{}", r["argv"]);
    assert!(has(&r["argv"], &["--system-prompt", SYSTEM_PROMPT]));
    // no tools, MCP, settings or saved session; no thinking, a bounded answer
    assert!(has(&r["argv"], &["--tools", ""]) && has(&r["argv"], &["--strict-mcp-config"]) && has(&r["argv"], &["--no-session-persistence"]));
    assert_eq!(r["env"], json!({"out": "2048", "thinking": "0"}));
    // the API key the settings hold isn't used for it
    assert!(h.st.api_key.read().is_some());
}

#[test]
fn the_clis_error_and_the_limits_reach_the_artifact() {
    let h = Harness::new();
    h.reply(json!({"type": "result", "subtype": "success", "is_error": true, "result": "Not logged in · Please run /login"}));
    assert!(h.complete("a", "hi").unwrap_err().contains("Not logged in"));
    // the per-minute limit counts the account's calls too (the failed one above included)
    h.reply(json!({"type": "result", "is_error": false, "result": "ok"}));
    for i in 1..10 {
        assert_eq!(h.complete("a", "hi").unwrap(), "ok", "call {i}");
    }
    assert!(h.complete("a", "hi").unwrap_err().contains("in a minute"));
    assert_eq!(h.runs().len(), 10, "a refused call starts no CLI");
    // an empty prompt never starts one either
    assert!(h.complete("b", " ").unwrap_err().contains("needs a prompt"));
    assert_eq!(h.runs().len(), 10);
}
