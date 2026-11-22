//! Shared application state.

use crate::attachments::Attachment;
use crate::codetools::CodeSession;
use crate::db::Db;
use crate::mcp::McpManager;
use crate::settings::Settings;
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionReply {
    /// once | chat | session | always | deny | approve | approve_edits
    pub decision: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// Live (not yet persisted) state of a running generation.
#[derive(Default, Clone)]
pub struct LiveRun {
    pub msg_id: String,
    pub content: Value,
    pub live_tools: HashMap<String, String>,
    pub streaming_tools: HashSet<String>,
}

pub struct AppState {
    pub db: Arc<Db>,
    pub settings: RwLock<Settings>,
    pub api_key: RwLock<Option<String>>,
    pub mcp: Arc<McpManager>,
    pub runs: Mutex<HashMap<String, CancellationToken>>,
    pub live: Mutex<HashMap<String, LiveRun>>,
    /// The Claude Code CLI turn running in a conversation: the line to it.
    pub cc_ctl: Mutex<HashMap<String, crate::claudecode::Ctl>>,
    /// Claude Code's workflows per conversation (the Tasks pane), finished ones until cleared.
    pub workflows: Mutex<HashMap<String, Vec<Value>>>,
    pub permissions: Mutex<HashMap<String, oneshot::Sender<PermissionReply>>>,
    pub pending_perms: Mutex<HashMap<String, Value>>,
    pub staged: Mutex<HashMap<String, Attachment>>,
    pub served: Mutex<HashMap<String, (String, Vec<u8>)>>,
    pub code_sessions: Mutex<HashMap<String, Arc<CodeSession>>>,
    pub shells: Arc<crate::shells::Shells>,
    pub terminals: crate::terminal::Terminals,
    pub subagents: crate::subagent::Registry,
    pub watches: crate::watch::Watches,
    pub oai_models: Mutex<Vec<String>>,
    pub models: Mutex<Option<Vec<Value>>>,
    /// The plan's usage limits as the account engine last reported them (usage.rs); null until then.
    pub usage_limits: Mutex<Value>,
    /// Set when the configured endpoint rejected optional request features.
    pub degraded: AtomicBool,
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn new(db: Db, data_dir: PathBuf, mcp_config: PathBuf) -> Self {
        let db = Arc::new(db);
        let settings = Settings::load(&db);
        crate::keepawake::apply(&settings);
        let key = crate::settings::load_api_key(&db);
        AppState {
            db,
            settings: RwLock::new(settings),
            api_key: RwLock::new(key),
            mcp: Arc::new(McpManager::new(mcp_config)),
            runs: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            cc_ctl: Mutex::new(HashMap::new()),
            workflows: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            pending_perms: Mutex::new(HashMap::new()),
            staged: Mutex::new(HashMap::new()),
            served: Mutex::new(HashMap::new()),
            code_sessions: Mutex::new(HashMap::new()),
            shells: Arc::new(crate::shells::Shells::default()),
            terminals: crate::terminal::Terminals::default(),
            subagents: crate::subagent::Registry::default(),
            watches: crate::watch::Watches::default(),
            oai_models: Mutex::new(Vec::new()),
            models: Mutex::new(None),
            usage_limits: Mutex::new(Value::Null),
            degraded: AtomicBool::new(false),
            data_dir,
        }
    }

    pub fn client(&self) -> crate::error::AppResult<crate::anthropic::Client> {
        let key = self
            .api_key
            .read()
            .clone()
            .ok_or_else(|| crate::error::AppError::Api {
                status: 401,
                kind: "no_api_key".into(),
                message: "Add your Anthropic API key in Settings → Account, or sign in with your Claude account, to start chatting.".into(),
            })?;
        let base = self.settings.read().base_url.clone();
        crate::anthropic::Client::new(&base, &key)
    }

    pub fn code_session(&self, conv_id: &str, cwd: &str) -> Arc<CodeSession> {
        let mut m = self.code_sessions.lock();
        m.entry(conv_id.to_string())
            .or_insert_with(|| {
                let mut s = CodeSession::for_conversation(PathBuf::from(cwd), conv_id, self.shells.clone());
                // Auto mode's "written in the session" also spans app restarts
                if let Ok(c) = self.db.conv_get(conv_id) {
                    s.started = CodeSession::conversation_start(c.created_at);
                }
                Arc::new(s)
            })
            .clone()
    }

    /// Whether the UI speaks Turkish, for the few texts the backend shows itself (desktop notifications).
    pub fn turkish(&self) -> bool {
        ui_turkish(&self.settings.read().language, system_language())
    }
}

/// The language setting (auto | en | tr) as the UI resolves it: "auto" follows the system's language.
fn ui_turkish(setting: &str, system: Option<String>) -> bool {
    match setting {
        "tr" => true,
        "en" => false,
        _ => system.is_some_and(|l| l.to_ascii_lowercase().starts_with("tr")),
    }
}

/// The system's display language (as the WebView's `navigator.language` follows it), e.g. "tr-TR".
fn system_language() -> Option<String> {
    #[cfg(windows)]
    {
        let first = |path: &str, name: &str| windows_registry::CURRENT_USER.open(path).ok()?.get_multi_string(name).ok()?.into_iter().next();
        if let Some(l) = first(r"Control Panel\Desktop", "PreferredUILanguages").or_else(|| first(r"Control Panel\Desktop\MuiCached", "MachinePreferredUILanguages")) {
            return Some(l);
        }
    }
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ui_language_follows_the_setting_then_the_system() {
        assert!(ui_turkish("tr", Some("en-US".into())) && !ui_turkish("en", Some("tr-TR".into())));
        assert!(ui_turkish("auto", Some("tr-TR".into())) && ui_turkish("auto", Some("tr_TR.UTF-8".into())));
        assert!(!ui_turkish("auto", Some("en-GB".into())) && !ui_turkish("auto", None));
    }

    #[test]
    fn a_code_session_starts_with_its_conversation() {
        let dir = std::env::temp_dir().join(format!("cr-state-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        let conv = st.db.conv_create("code", "m", None, Some(&dir.to_string_lossy()), &serde_json::json!({})).unwrap();
        let s = st.code_session(&conv.id, &dir.to_string_lossy());
        assert_eq!(s.started, CodeSession::conversation_start(conv.created_at));
        let _ = std::fs::remove_dir_all(dir);
    }
}
