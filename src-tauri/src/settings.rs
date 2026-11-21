//! Application settings, persisted as one JSON document in the settings table.
//! The API key is kept out of this struct: it lives in the OS keychain
//! (macOS/Windows) or in a separate settings row (Linux).

use crate::db::Db;
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const DEFAULT_TITLE_MODEL: &str = "claude-haiku-5-5";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomStyle {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub base_url: String,
    pub default_model: String,
    pub title_model: String,
    pub max_tokens: u32,

    // Appearance
    pub theme: String,     // system | light | dark
    pub chat_font: String, // default | sans | system | dyslexic
    /// Width of the conversation column: normal | wide | full
    pub chat_width: String,
    pub language: String,  // auto | en | tr
    pub zoom: f64,

    // Profile
    pub full_name: String,
    pub nickname: String,
    pub occupation: String,
    pub preferences: String,

    // Features
    pub artifacts_enabled: bool,
    pub latex_enabled: bool,
    pub web_search_default: bool,
    pub thinking_default: bool,
    pub refusal_fallbacks: bool,
    pub prompt_caching: bool,
    pub custom_styles: Vec<CustomStyle>,
    pub send_on_enter: bool,
    /// Let Claude search and reference past chats (memory).
    pub chat_search: bool,
    /// Desktop notification when a response finishes in the background.
    pub notifications: bool,

    // Desktop
    pub quick_entry_enabled: bool,
    pub quick_entry_shortcut: String,
    pub keep_in_tray: bool,
    /// auto | on | off; read before the window opens (see display.rs)
    pub hardware_acceleration: String,
    pub launch_at_login: bool,

    // Code
    pub code_permission_mode: String, // default | acceptEdits | plan | bypassPermissions
    pub code_model: String,
    pub bash_timeout_secs: u64,
    /// auto | claude-code | builtin
    pub code_engine: String,
    /// Optional path to the `claude` executable.
    pub claude_code_path: String,
    /// Git worktrees for code sessions: off | parallel | always
    pub code_worktree: String,
    /// Snapshot project files before each turn (for rewind).
    pub code_checkpoints: bool,
    /// Summarize older messages when the context window is this full (percent; 0 = never).
    pub auto_compact_pct: u32,
    /// Pause an agent turn once it has cost this much (USD; 0 = no limit).
    /// A conversation can override it (`/budget`).
    pub turn_budget_usd: f64,
    /// Model to switch to when the chosen one is overloaded: auto | off | <model id>
    pub overload_fallback: String,

    // Tools: per tool policy, key "server/tool" -> ask | allow | deny
    pub tool_policies: serde_json::Map<String, Value>,
    /// Changed keyboard shortcuts: action -> binding ("Mod+Shift+O"; "" = off)
    pub keybindings: serde_json::Map<String, Value>,
    /// Project folders whose hook configuration the user trusted: root -> hash
    pub trusted_hooks: serde_json::Map<String, Value>,
    /// OpenAI-compatible endpoint for local / other models ("" = off), e.g. http://localhost:11434/v1
    pub oai_base_url: String,
    pub oai_api_key: String,
    /// Editor for "open in editor": auto | code | cursor | windsurf | zed | subl | idea | system
    pub editor: String,
    pub disabled_mcp_servers: Vec<String>,

    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            base_url: DEFAULT_BASE_URL.into(),
            default_model: DEFAULT_MODEL.into(),
            title_model: DEFAULT_TITLE_MODEL.into(),
            max_tokens: 32000,
            theme: "system".into(),
            chat_font: "default".into(),
            chat_width: "normal".into(),
            language: "auto".into(),
            zoom: 1.0,
            full_name: String::new(),
            nickname: String::new(),
            occupation: String::new(),
            preferences: String::new(),
            artifacts_enabled: true,
            latex_enabled: true,
            web_search_default: false,
            thinking_default: false,
            refusal_fallbacks: true,
            prompt_caching: true,
            custom_styles: vec![],
            send_on_enter: true,
            chat_search: true,
            notifications: true,
            quick_entry_enabled: true,
            quick_entry_shortcut: if cfg!(target_os = "macos") {
                "Alt+Space".into()
            } else {
                "CommandOrControl+Alt+Space".into()
            },
            keep_in_tray: true,
            hardware_acceleration: "auto".into(),
            launch_at_login: false,
            code_permission_mode: "default".into(),
            code_model: DEFAULT_MODEL.into(),
            bash_timeout_secs: 120,
            code_engine: "auto".into(),
            claude_code_path: String::new(),
            code_worktree: "parallel".into(),
            code_checkpoints: true,
            auto_compact_pct: 85,
            turn_budget_usd: 0.0,
            overload_fallback: "auto".into(),
            tool_policies: Default::default(),
            keybindings: Default::default(),
            trusted_hooks: Default::default(),
            oai_base_url: String::new(),
            oai_api_key: String::new(),
            editor: "auto".into(),
            disabled_mcp_servers: vec![],
            onboarded: false,
        }
    }
}

const KEY_SETTINGS: &str = "app_settings";
const KEY_API: &str = "api_key";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const KEYRING_SERVICE: &str = "dev.clauderust.desktop";

impl Settings {
    pub fn load(db: &Db) -> Settings {
        match db.setting_get(KEY_SETTINGS) {
            Ok(Some(s)) => serde_json::from_str(&s).unwrap_or_default(),
            _ => Settings::default(),
        }
    }

    pub fn save(&self, db: &Db) -> AppResult<()> {
        db.setting_set(KEY_SETTINGS, &serde_json::to_string(self)?)
    }

    /// Apply a partial JSON patch coming from the UI.
    pub fn patched(&self, patch: &Value) -> AppResult<Settings> {
        let mut v = serde_json::to_value(self)?;
        if let (Some(obj), Some(p)) = (v.as_object_mut(), patch.as_object()) {
            for (k, val) in p {
                obj.insert(k.clone(), val.clone());
            }
        }
        let mut s: Settings = serde_json::from_value(v)?;
        s.normalize();
        Ok(s)
    }

    pub fn normalize(&mut self) {
        let b = self.base_url.trim().trim_end_matches('/').to_string();
        self.base_url = if b.is_empty() { DEFAULT_BASE_URL.into() } else { b };
        self.max_tokens = self.max_tokens.clamp(1024, 128_000);
        self.zoom = self.zoom.clamp(0.5, 2.5);
        self.bash_timeout_secs = self.bash_timeout_secs.clamp(5, 600);
        if self.default_model.trim().is_empty() {
            self.default_model = DEFAULT_MODEL.into();
        }
        if self.code_model.trim().is_empty() {
            self.code_model = self.default_model.clone();
        }
    }

    pub fn is_first_party(&self) -> bool {
        self.base_url.starts_with("https://api.anthropic.com")
    }
}

// ---------------------------------------------------------------- API key

pub fn load_api_key(db: &Db) -> Option<String> {
    if let Ok(k) = std::env::var("CLAUDERUST_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEY_API) {
            if let Ok(k) = entry.get_password() {
                if !k.is_empty() {
                    return Some(k);
                }
            }
        }
    }
    db.setting_get(KEY_API).ok().flatten().filter(|k| !k.is_empty())
}

pub fn store_api_key(db: &Db, key: Option<&str>) -> AppResult<()> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEY_API) {
            match key {
                Some(k) => {
                    if entry.set_password(k).is_ok() {
                        let _ = db.setting_delete(KEY_API);
                        return Ok(());
                    }
                }
                None => {
                    let _ = entry.delete_credential();
                }
            }
        }
    }
    match key {
        Some(k) => db.setting_set(KEY_API, k),
        None => db.setting_delete(KEY_API),
    }
}

pub fn mask_key(k: &str) -> String {
    let n = k.chars().count();
    if n <= 12 {
        return "•".repeat(n);
    }
    let head: String = k.chars().take(10).collect();
    let tail: String = k.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patch_roundtrip() {
        let s = Settings::default();
        let p = s
            .patched(&serde_json::json!({"theme":"dark","base_url":"https://x.example/ ","max_tokens": 5}))
            .unwrap();
        assert_eq!(p.theme, "dark");
        assert_eq!(p.base_url, "https://x.example");
        assert_eq!(p.max_tokens, 1024);
    }
}
