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
    pub permissions: Mutex<HashMap<String, oneshot::Sender<PermissionReply>>>,
    pub pending_perms: Mutex<HashMap<String, Value>>,
    pub staged: Mutex<HashMap<String, Attachment>>,
    pub served: Mutex<HashMap<String, (String, Vec<u8>)>>,
    pub code_sessions: Mutex<HashMap<String, Arc<CodeSession>>>,
    pub models: Mutex<Option<Vec<Value>>>,
    /// Set when the configured endpoint rejected optional request features.
    pub degraded: AtomicBool,
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn new(db: Db, data_dir: PathBuf, mcp_config: PathBuf) -> Self {
        let db = Arc::new(db);
        let settings = Settings::load(&db);
        let key = crate::settings::load_api_key(&db);
        AppState {
            db,
            settings: RwLock::new(settings),
            api_key: RwLock::new(key),
            mcp: Arc::new(McpManager::new(mcp_config)),
            runs: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            pending_perms: Mutex::new(HashMap::new()),
            staged: Mutex::new(HashMap::new()),
            served: Mutex::new(HashMap::new()),
            code_sessions: Mutex::new(HashMap::new()),
            models: Mutex::new(None),
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
                message: "Add your Anthropic API key in Settings to start chatting.".into(),
            })?;
        let base = self.settings.read().base_url.clone();
        crate::anthropic::Client::new(&base, &key)
    }

    pub fn code_session(&self, conv_id: &str, cwd: &str) -> Arc<CodeSession> {
        let mut m = self.code_sessions.lock();
        m.entry(conv_id.to_string())
            .or_insert_with(|| Arc::new(CodeSession::new(PathBuf::from(cwd))))
            .clone()
    }
}
