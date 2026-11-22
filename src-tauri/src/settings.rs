//! Application settings, persisted as one JSON document in the settings table.
//! The API key is kept out of this struct: it lives in the OS keychain
//! (macOS/Windows) or in a separate settings row (Linux).

use crate::db::Db;
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
    /// Chat memory (opt-in): a fast model writes notes about the user from
    /// their chats, and new chats get them (see memory_synth.rs).
    pub memory_enabled: bool,
    /// Each project keeps a memory of its own, written from its chats.
    pub memory_project_scope: bool,
    /// Projects whose chats are neither remembered nor given memory.
    pub memory_excluded_projects: Vec<String>,
    /// Fast mode switched on in new chats, on the models that have it (`options.fast`).
    pub fast_mode: bool,
    /// Code execution and file creation in chats on the API (code_exec.rs).
    pub code_execution: bool,

    // Desktop
    pub quick_entry_enabled: bool,
    pub quick_entry_shortcut: String,
    pub keep_in_tray: bool,
    /// auto | on | off; read before the window opens (see display.rs)
    pub hardware_acceleration: String,
    pub launch_at_login: bool,
    /// Look for a newer version now and then (Settings → About).
    pub auto_update: bool,
    /// Keep the computer awake while Claude works (a turn, a Claude Code workflow or a routine run).
    pub keep_awake: bool,

    // Code
    pub code_permission_mode: String, // default | acceptEdits | plan | bypassPermissions
    pub code_model: String,
    pub bash_timeout_secs: u64,
    /// auto | claude-code | builtin
    pub code_engine: String,
    /// Chat conversations: auto (API key, else the Claude account) | api | claude-code (the Claude account, through the official CLI).
    pub chat_engine: String,
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
            memory_enabled: false,
            memory_project_scope: true,
            memory_excluded_projects: vec![],
            fast_mode: false,
            code_execution: true,
            quick_entry_enabled: true,
            quick_entry_shortcut: if cfg!(target_os = "macos") {
                "Alt+Space".into()
            } else {
                "CommandOrControl+Alt+Space".into()
            },
            keep_in_tray: true,
            hardware_acceleration: "auto".into(),
            launch_at_login: false,
            auto_update: true,
            keep_awake: true,
            code_permission_mode: "default".into(),
            code_model: DEFAULT_MODEL.into(),
            bash_timeout_secs: 120,
            code_engine: "auto".into(),
            chat_engine: "auto".into(),
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
/// The last stored settings that couldn't be read as a whole.
const KEY_SETTINGS_BACKUP: &str = "app_settings.unreadable";
const KEY_API: &str = "api_key";
/// Marks a settings backup file.
const EXPORT_MARKER: &str = "clauderust_settings";
/// Kept out of backups: secrets, trust decisions, machine-specific paths.
const NOT_EXPORTED: &[&str] = &[
    "oai_api_key",
    "trusted_hooks",
    "claude_code_path",
    "onboarded",
    // a shared "backup" must not be able to send requests (with this
    // machine's API key) to another server, run a program as the editor, or
    // turn permission prompts off: these stay as set on this machine
    "base_url",
    "oai_base_url",
    "editor",
    "code_permission_mode",
    "tool_policies",
    // nor turn on what reads the chats and spends on them (memory); project ids are this machine's
    "memory_enabled",
    "memory_excluded_projects",
];
/// The installed app's identifier (`tauri.conf.json`).
pub const PRODUCTION_ID: &str = "dev.clauderust.desktop";

/// The app's identifier, the secret store's service name: a copy with
/// another one (dev, portable, test) keeps its own API key and MCP sign-ins
/// instead of reading and overwriting the installed app's. The installed
/// app's identifier is the name every earlier version used, so its entries
/// stay where they are.
static SECRET_SERVICE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Called once at start, before any secret is read.
pub fn set_app_identifier(identifier: &str) {
    let _ = SECRET_SERVICE.set(identifier.to_string());
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn secret_service() -> &'static str {
    // unset only where the app never started (tests): never the installed app's entries
    SECRET_SERVICE.get().map_or("dev.clauderust.tests", String::as_str)
}

impl Settings {
    pub fn load(db: &Db) -> Settings {
        match db.setting_get(KEY_SETTINGS) {
            Ok(Some(raw)) => match serde_json::from_str::<Settings>(&raw) {
                Ok(s) => s,
                Err(e) => {
                    // a value this version can't read (written by an older or
                    // newer version): keep a copy of what was stored and take
                    // every field that still fits, rather than starting over
                    // (and then saving the defaults over the user's settings)
                    log::warn!("settings didn't load as a whole ({e}); recovering them field by field");
                    let _ = db.setting_set(KEY_SETTINGS_BACKUP, &raw);
                    Settings::recovered(&raw)
                }
            },
            _ => Settings::default(),
        }
    }

    /// The fields of stored settings that still deserialize, on defaults.
    fn recovered(raw: &str) -> Settings {
        let mut s = Settings::default();
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(raw) {
            for (k, v) in o {
                if let Ok(n) = s.patched(&json!({ k: v })) {
                    s = n;
                }
            }
        }
        s
    }

    pub fn save(&self, db: &Db) -> AppResult<()> {
        db.setting_set(KEY_SETTINGS, &serde_json::to_string(self)?)
    }

    /// The settings for a backup file: everything except secrets, trust
    /// decisions and paths that only make sense on this machine.
    pub fn export_json(&self) -> AppResult<Value> {
        let mut v = serde_json::to_value(self)?;
        if let Some(o) = v.as_object_mut() {
            for k in NOT_EXPORTED {
                o.remove(*k);
            }
            o.insert(EXPORT_MARKER.into(), Value::from(1));
        }
        Ok(v)
    }

    /// These settings with a backup file's values applied (only known,
    /// exportable fields are taken).
    pub fn imported(&self, backup: &Value) -> AppResult<Settings> {
        let o = backup.as_object().filter(|o| o.contains_key(EXPORT_MARKER)).ok_or_else(|| crate::error::AppError::msg("That isn't a ClaudeRust settings file."))?;
        let known = serde_json::to_value(Settings::default())?;
        // field by field, so one value of the wrong type (an older or edited
        // file) is skipped instead of failing the whole restore
        let mut out = self.clone();
        for (k, v) in o.iter().filter(|(k, _)| !NOT_EXPORTED.contains(&k.as_str()) && known.get(k.as_str()).is_some()) {
            if let Ok(s) = out.patched(&json!({ k.clone(): v.clone() })) {
                out = s;
            }
        }
        Ok(out)
    }

    /// Apply a partial JSON patch coming from the UI. Per-tool permission
    /// policies are merged key by key (the UI sends its copy of the whole
    /// map, which can be older than an "always allow" given meanwhile).
    pub fn patched(&self, patch: &Value) -> AppResult<Settings> {
        let mut v = serde_json::to_value(self)?;
        if let (Some(obj), Some(p)) = (v.as_object_mut(), patch.as_object()) {
            for (k, val) in p {
                if k == "tool_policies" {
                    if let (Some(cur), Some(add)) = (obj.get_mut(k).and_then(|c| c.as_object_mut()), val.as_object()) {
                        for (tk, tv) in add {
                            cur.insert(tk.clone(), tv.clone());
                        }
                        continue;
                    }
                }
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
        if let Ok(entry) = keyring::Entry::new(secret_service(), KEY_API) {
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
        if let Ok(entry) = keyring::Entry::new(secret_service(), KEY_API) {
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
    use serde_json::json;
    #[test]
    fn backup_and_restore() {
        let mut s = Settings { oai_api_key: "secret".into(), theme: "dark".into(), claude_code_path: "/opt/claude".into(), ..Default::default() };
        s.keybindings.insert("find".into(), json!("Mod+Shift+G"));
        let b = s.export_json().unwrap();
        assert!(b.get("oai_api_key").is_none() && b.get("claude_code_path").is_none() && b.get("trusted_hooks").is_none());
        assert_eq!(b["theme"], "dark");
        let mut here = Settings { oai_api_key: "mine".into(), claude_code_path: "/usr/bin/claude".into(), ..Default::default() };
        here.theme = "light".into();
        let r = here.imported(&b).unwrap();
        assert_eq!(r.theme, "dark");
        assert_eq!(r.keybindings["find"], "Mod+Shift+G");
        assert_eq!(r.oai_api_key, "mine", "secrets on this machine are kept");
        assert_eq!(r.claude_code_path, "/usr/bin/claude");
        // a backup can't smuggle in excluded or unknown fields
        let mut evil = b.clone();
        evil["oai_api_key"] = json!("x");
        evil["not_a_setting"] = json!(1);
        assert_eq!(here.imported(&evil).unwrap().oai_api_key, "mine");
        assert!(here.imported(&json!({"theme": "dark"})).is_err(), "needs the marker");
        // nor redirect requests, run programs or lift permission prompts
        evil["base_url"] = json!("https://evil.example");
        evil["editor"] = json!("python3");
        evil["code_permission_mode"] = json!("bypassPermissions");
        evil["tool_policies"] = json!({"Bash": "allow"});
        evil["memory_enabled"] = json!(true);
        let r = here.imported(&evil).unwrap();
        assert!(!r.memory_enabled, "memory is turned on here only");
        assert!(!Settings::default().memory_enabled, "memory is opt-in");
        assert_eq!(r.base_url, here.base_url);
        assert_eq!(r.editor, here.editor);
        assert_eq!(r.code_permission_mode, here.code_permission_mode);
        assert!(r.tool_policies.is_empty());
        // a value of the wrong type is skipped, the rest still applies
        let mut odd = b.clone();
        odd["max_tokens"] = json!("32000");
        odd["theme"] = json!("dark");
        let r = here.imported(&odd).unwrap();
        assert_eq!((r.theme.as_str(), r.max_tokens), ("dark", here.max_tokens));
    }

    #[test]
    fn unreadable_settings_keep_what_still_fits() {
        let db = Db::open_in_memory().unwrap();
        // e.g. written by another version: one field has a type this one can't read
        let raw = r#"{"nickname":"Ada","theme":"dark","max_tokens":"lots","tool_policies":{"srv__t":"allow"}}"#;
        db.setting_set(KEY_SETTINGS, raw).unwrap();
        let s = Settings::load(&db);
        assert_eq!((s.nickname.as_str(), s.theme.as_str()), ("Ada", "dark"), "not reset to defaults");
        assert_eq!(s.tool_policies["srv__t"], "allow");
        assert_eq!(s.max_tokens, Settings::default().max_tokens);
        assert_eq!(db.setting_get(KEY_SETTINGS_BACKUP).unwrap().as_deref(), Some(raw), "the original is kept");
    }

    #[test]
    fn the_installed_app_keeps_its_secret_store_entries() {
        // the service name is the identifier: the installed app's is the name its earlier
        // versions used (a new one would need its entries moved), another copy's is its own
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], PRODUCTION_ID);
        // tests never start the app: they must not read or write its entries
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_ne!(secret_service(), PRODUCTION_ID);
    }

    #[test]
    fn tool_policies_merge_per_tool() {
        let mut s = Settings::default();
        s.tool_policies.insert("a__x".into(), json!("allow"));
        // the UI's older copy of the map, changing another tool
        let s = s.patched(&json!({"tool_policies": {"b__y": "deny"}})).unwrap();
        assert_eq!((s.tool_policies["a__x"].as_str(), s.tool_policies["b__y"].as_str()), (Some("allow"), Some("deny")));
    }

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
