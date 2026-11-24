//! `window.claude.complete` for artifacts: one short answer from the fast
//! model, through the API key or, in a chat on the Claude account, one call
//! of its CLI (claudecode::one_shot, as titles are made there), with no tools
//! either way. The artifact's sandboxed frame can't reach the app: its request
//! comes through ui/js/artifacts.js, which asks the person first and passes on
//! nothing but the prompt. So the frame chooses neither the model nor a system
//! prompt, sees nothing of any conversation, and gets back only the text.

use crate::error::{AppError, AppResult};
use crate::models;
use crate::state::AppState;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

pub const MAX_PROMPT_CHARS: usize = 20_000;
/// The app's system prompt for every call: the frame's prompt is only ever the user message.
pub const SYSTEM_PROMPT: &str = "You answer one request from an artifact: a small interactive app, made by Claude, that the user is running. Do what the request asks and output only the requested content, with no preamble. This is a single-turn call without tools, so you can't ask anything back: if the request is ambiguous, make a reasonable interpretation and proceed.";
const MAX_OUTPUT_TOKENS: u32 = 2048;
/// Calls one artifact may make in a minute.
const PER_ARTIFACT_PER_MINUTE: usize = 10;
/// Calls the artifacts of one conversation may make until the app restarts.
const PER_CONVERSATION: u32 = 100;
const TIMEOUT: Duration = Duration::from_secs(60);
const MINUTE: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Limits {
    /// (conversation, artifact) -> times of its calls in the last minute
    recent: HashMap<(String, String), VecDeque<Instant>>,
    /// conversation -> calls so far
    used: HashMap<String, u32>,
}

impl Limits {
    /// Count a call of `artifact` in `conv` at `now`, or say which limit it is over.
    fn take(&mut self, conv: &str, artifact: &str, now: Instant) -> Result<(), String> {
        if self.used.get(conv).copied().unwrap_or(0) >= PER_CONVERSATION {
            return Err(format!("This conversation's artifacts have asked Claude {PER_CONVERSATION} times, the most until ClaudeRust restarts."));
        }
        let recent = self.recent.entry((conv.to_string(), artifact.to_string())).or_default();
        while recent.front().is_some_and(|t| now.duration_since(*t) >= MINUTE) {
            recent.pop_front();
        }
        if recent.len() >= PER_ARTIFACT_PER_MINUTE {
            return Err(format!("This artifact asked Claude {PER_ARTIFACT_PER_MINUTE} times in a minute: wait a moment and try again."));
        }
        recent.push_back(now);
        *self.used.entry(conv.to_string()).or_default() += 1;
        Ok(())
    }
}

static LIMITS: LazyLock<Mutex<Limits>> = LazyLock::new(Default::default);

fn check_prompt(prompt: &str) -> Result<(), String> {
    if prompt.trim().is_empty() {
        return Err("window.claude.complete needs a prompt.".into());
    }
    let n = prompt.chars().count();
    if n > MAX_PROMPT_CHARS {
        return Err(format!("The prompt is {n} characters long; window.claude.complete takes at most {MAX_PROMPT_CHARS}."));
    }
    Ok(())
}

/// Where window.claude.complete runs for `conversation_id`: on the Claude
/// account ("account") or the API key ("api"); an error where it can't.
async fn engine(state: &AppState, conversation_id: &str) -> AppResult<&'static str> {
    let conv = state.db.conv_get(conversation_id)?;
    // shortcut: other providers' models aren't asked; add their one-shot call if artifacts there need it
    if conv.model.starts_with(crate::oai::PREFIX) {
        return Err(AppError::msg("window.claude.complete needs Claude, and this conversation runs on another provider."));
    }
    Ok(if crate::claudecode::uses_cli(state, &conv).await { "account" } else { "api" })
}

/// Asked before the person is: where it can't work, nobody is asked to allow
/// it; else where it runs, which the question names.
#[tauri::command]
pub async fn artifact_complete_check(state: tauri::State<'_, Arc<AppState>>, conversation_id: String) -> AppResult<&'static str> {
    engine(&state, &conversation_id).await
}

/// Answer `prompt` for the artifact `artifact_id` of `conversation_id`, once
/// the person allowed it (artifacts.js).
#[tauri::command]
pub async fn artifact_complete(state: tauri::State<'_, Arc<AppState>>, conversation_id: String, artifact_id: String, prompt: String) -> AppResult<String> {
    check_prompt(&prompt).map_err(AppError::msg)?;
    let account = engine(&state, &conversation_id).await? == "account";
    // without a key the call isn't counted
    let client = if account { None } else { Some(state.client()?) };
    LIMITS.lock().take(&conversation_id, &artifact_id, Instant::now()).map_err(AppError::msg)?;
    let Some(client) = client else {
        return crate::claudecode::one_shot(&state, SYSTEM_PROMPT, &prompt, MAX_OUTPUT_TOKENS, TIMEOUT).await;
    };
    let model_id = state.settings.read().title_model.clone();
    let model = models::info(&model_id);
    let mut body = json!({"model": model_id, "max_tokens": MAX_OUTPUT_TOKENS, "system": SYSTEM_PROMPT, "messages": [{"role": "user", "content": prompt}]});
    match model.thinking {
        models::Thinking::Disableable => body["thinking"] = json!({"type": "disabled"}),
        models::Thinking::BetweenTools => body["thinking"] = json!({"type": "between_tools"}),
        _ => {}
    }
    let v = tokio::time::timeout(TIMEOUT, client.create(&body, &[]))
        .await
        .map_err(|_| AppError::msg("Claude took too long to answer."))??;
    Ok(v["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_are_non_empty_and_bounded_in_characters() {
        assert!(check_prompt("").is_err());
        assert!(check_prompt(" \n ").is_err());
        assert!(check_prompt(&"a".repeat(MAX_PROMPT_CHARS)).is_ok());
        assert!(check_prompt(&"a".repeat(MAX_PROMPT_CHARS + 1)).is_err());
        // characters, not bytes
        assert!(check_prompt(&"ş".repeat(MAX_PROMPT_CHARS)).is_ok());
    }

    #[test]
    fn an_artifact_gets_a_few_calls_a_minute() {
        let mut l = Limits::default();
        let t0 = Instant::now();
        for i in 0..PER_ARTIFACT_PER_MINUTE {
            assert!(l.take("c", "a", t0 + Duration::from_secs(i as u64)).is_ok(), "call {i}");
        }
        assert!(l.take("c", "a", t0 + Duration::from_secs(30)).is_err());
        // another artifact of the conversation has its own
        assert!(l.take("c", "b", t0 + Duration::from_secs(30)).is_ok());
        // a minute after the first call, it has room for one again
        assert!(l.take("c", "a", t0 + MINUTE).is_ok());
        assert!(l.take("c", "a", t0 + MINUTE).is_err());
    }

    #[test]
    fn a_conversation_has_a_cap_for_all_its_artifacts() {
        let mut l = Limits::default();
        let t0 = Instant::now();
        // spread over time and artifacts, so the per-minute limit doesn't apply
        for i in 0..PER_CONVERSATION {
            let at = t0 + Duration::from_secs(u64::from(i) * 7);
            assert!(l.take("c", &format!("a{}", i % 3), at).is_ok(), "call {i}");
        }
        let later = t0 + Duration::from_secs(3600);
        assert!(l.take("c", "fresh", later).unwrap_err().contains("restarts"));
        // a refused call isn't counted, and other conversations are apart
        assert_eq!(l.used["c"], PER_CONVERSATION);
        assert!(l.take("d", "a0", later).is_ok());
    }

    #[test]
    fn the_check_names_the_engine_and_another_provider_is_refused_uncounted() {
        use tauri::Manager;
        let dir = std::env::temp_dir().join(format!("cr-artifact-ai-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = Arc::new(AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json")));
        // chats run on the Claude account (its CLI)
        st.settings.write().chat_engine = "claude-code".into();
        let app = tauri::test::mock_app();
        app.manage(st.clone());
        let ask = |conv: &str, prompt: &str| tauri::async_runtime::block_on(artifact_complete(app.state(), conv.into(), "a".into(), prompt.into())).unwrap_err().to_string();
        let check = |conv: &str| tauri::async_runtime::block_on(artifact_complete_check(app.state(), conv.into())).map_err(|e| e.to_string());
        let account = st.db.conv_create("chat", "claude-haiku-5-5", None, None, &json!({})).unwrap();
        let other_provider = st.db.conv_create("chat", &format!("{}gpt", crate::oai::PREFIX), None, None, &json!({})).unwrap();
        // the account answers (tests/artifact_account.rs runs it), and the dialog says so
        assert_eq!(check(&account.id).unwrap(), "account");
        assert!(ask(&account.id, "").contains("needs a prompt"));
        // another provider is refused, and the check the dialog waits for says the same, so nobody is asked
        assert!(ask(&other_provider.id, "hi").contains("another provider"));
        assert!(check(&other_provider.id).unwrap_err().contains("another provider"));
        for id in [&account.id, &other_provider.id] {
            assert!(!LIMITS.lock().used.contains_key(id.as_str()), "{id}");
        }
        st.settings.write().chat_engine = "api".into();
        assert_eq!(check(&account.id).unwrap(), "api");
    }
}
