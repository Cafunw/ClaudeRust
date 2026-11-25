//! Local SQLite storage: conversations (as a message tree), projects,
//! project knowledge, artifacts and settings.

use crate::error::{AppError, AppResult};
use crate::util::{new_id, now_ms};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// (conversation id, title, updated_at, first user message, kind)
pub type RecentChat = (String, String, i64, String, String);

/// (created_at, conversation id, title, kind, conversation model, message meta)
pub type UsageRow = (i64, String, String, String, String, String);

/// (text, created_at, conversation title) of a message the user typed
pub type Prompt = (String, i64, String);

pub struct Db {
    conn: Mutex<Connection>,
    /// A second connection to the same file for the long read-only scans
    /// (prompt history, search, usage, the conversation list…). In WAL mode
    /// it reads while `conn` writes, so a scan of a large history no longer
    /// holds up every other query: each command's lookups, the engine
    /// storing an answer. Every write is its own transaction, so it sees
    /// what `conn` wrote as soon as that write returned. `None` in memory
    /// (a connection there is its own database) or without WAL.
    reader: Option<Mutex<Connection>>,
    fts: bool,
    /// Messages stored by this process, with the search-index row each one
    /// got (conversation id, rowid) once indexed. Re-indexing first removes
    /// a message's old row; `message_id` isn't indexed, so finding it means
    /// reading the whole index (tens of ms on a large history, at every
    /// answer). A message stored by this process can only have the row
    /// noted here, which is removed by its rowid instead. Kept in step with
    /// every removal from the index.
    indexed: Mutex<std::collections::HashMap<String, Option<(String, i64)>>>,
    /// Ctrl+R's typed prompts (see prompt_history) and the count of rows
    /// `conn` had written when they were read: each keystroke filters them
    /// again, and only a write since (all go through `conn`) reads them anew.
    history: Mutex<Option<(u64, std::sync::Arc<Vec<Prompt>>)>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub model: String,
    pub project_id: Option<String>,
    pub cwd: Option<String>,
    pub starred: bool,
    pub current_leaf: Option<String>,
    pub options: Value,
    pub created_at: i64,
    pub updated_at: i64,
    /// Set at creation (`options.incognito` on a chat): listed, searched and
    /// remembered nowhere, and removed when it's closed or the app quits.
    pub incognito: bool,
    pub pinned_at: Option<i64>,
    /// Archived chats are left out of the list and of search (unless asked).
    pub archived_at: Option<i64>,
    /// A turn finished while another conversation was open.
    pub unread: bool,
    pub section_id: Option<String>,
}

/// A user-made sidebar section (a "group" in the Code tab) of `kind` chats or code sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub created_at: i64,
}

/// The chat memory of one scope (`memory_synth::GLOBAL` or a project id).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryRow {
    pub scope: String,
    pub text: String,
    /// The conversations it was written from.
    pub sources: Vec<String>,
    pub updated_at: i64,
    /// When the model last merged chats into it (0 = never).
    pub synthesized_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub content: Value,
    pub meta: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub is_tool_result: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub starred: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub kind: String,
    #[serde(skip_serializing)]
    pub data: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub identifier: String,
    pub kind: String,
    pub title: String,
    pub language: Option<String>,
    pub content: String,
    pub created_at: i64,
    pub conversation_title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub conversation_id: String,
    pub message_id: Option<String>,
    pub title: String,
    pub snippet: String,
    pub kind: String,
    pub updated_at: i64,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  instructions TEXT NOT NULL DEFAULT '',
  starred INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS project_files (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  mime TEXT NOT NULL,
  size INTEGER NOT NULL,
  kind TEXT NOT NULL,
  data TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL DEFAULT 'chat',
  title TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL,
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  cwd TEXT,
  starred INTEGER NOT NULL DEFAULT 0,
  current_leaf TEXT,
  options TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_conv_updated ON conversations(updated_at);
CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  parent_id TEXT,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  meta TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id);
CREATE INDEX IF NOT EXISTS idx_messages_parent ON messages(parent_id);
CREATE TABLE IF NOT EXISTS artifacts (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL,
  identifier TEXT NOT NULL,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  language TEXT,
  content TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_artifacts_conv ON artifacts(conversation_id);
"#;

/// Schema changes made after SCHEMA (version 1, never edited) in release
/// order: entry `i` takes a database from version `i + 1` to `i + 2`. A
/// shipped entry is never edited either; a change is a new entry.
const MIGRATIONS: &[&str] = &[
    // 2: the rows mark_interrupted (at every start) and Ctrl+R's history look
    // for, each in an index of its own instead of a read of every message
    // (the CASEs keep a malformed meta from failing every write)
    "CREATE INDEX idx_messages_unfinished ON messages(id) WHERE role = 'assistant' AND CASE WHEN json_valid(meta)
       THEN json_extract(meta, '$.stop_reason') IS NULL AND json_extract(meta, '$.error') IS NULL AND json_extract(meta, '$.stopped') IS NULL ELSE 0 END;
     CREATE INDEX idx_messages_typed ON messages(created_at) WHERE role = 'user' AND CASE WHEN json_valid(meta) AND json_type(meta) = 'object'
       THEN json_extract(meta, '$.tool_results') IS NULL AND json_extract(meta, '$.hook_feedback') IS NULL ELSE 1 END;",
    // 3: incognito chats (the index finds a crash's leftovers at start) and
    // the sidebar's pins, archive, unread marks and sections (deleting a
    // section keeps its conversations)
    "ALTER TABLE conversations ADD COLUMN incognito INTEGER NOT NULL DEFAULT 0;
     CREATE INDEX idx_conv_incognito ON conversations(id) WHERE incognito = 1;
     CREATE TABLE sections (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, created_at INTEGER NOT NULL);
     ALTER TABLE conversations ADD COLUMN pinned_at INTEGER;
     ALTER TABLE conversations ADD COLUMN archived_at INTEGER;
     ALTER TABLE conversations ADD COLUMN unread INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE conversations ADD COLUMN section_id TEXT REFERENCES sections(id) ON DELETE SET NULL;",
    // 4: chat memory, one text per scope
    "CREATE TABLE chat_memory (scope TEXT PRIMARY KEY, text TEXT NOT NULL, sources TEXT NOT NULL DEFAULT '[]',
       updated_at INTEGER NOT NULL, synthesized_at INTEGER NOT NULL DEFAULT 0);",
    // 5: what artifacts keep with window.storage, per artifact (scope "<conversation>/<artifact>"),
    // its shared keys apart; gone with the conversation (an incognito one's too)
    "CREATE TABLE artifact_storage (conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
       scope TEXT NOT NULL, shared INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (scope, shared, key));
     CREATE INDEX idx_artifact_storage_conv ON artifact_storage(conversation_id);",
];

fn conv_from_row(r: &Row) -> rusqlite::Result<Conversation> {
    let options: String = r.get("options")?;
    Ok(Conversation {
        id: r.get("id")?,
        kind: r.get("kind")?,
        title: r.get("title")?,
        model: r.get("model")?,
        project_id: r.get("project_id")?,
        cwd: r.get("cwd")?,
        starred: r.get::<_, i64>("starred")? != 0,
        current_leaf: r.get("current_leaf")?,
        options: serde_json::from_str(&options).unwrap_or(Value::Object(Default::default())),
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        incognito: r.get("incognito")?,
        pinned_at: r.get("pinned_at")?,
        archived_at: r.get("archived_at")?,
        unread: r.get("unread")?,
        section_id: r.get("section_id")?,
    })
}

fn memory_from_row(r: &Row) -> rusqlite::Result<MemoryRow> {
    let sources: String = r.get("sources")?;
    Ok(MemoryRow {
        scope: r.get("scope")?,
        text: r.get("text")?,
        sources: serde_json::from_str(&sources).unwrap_or_default(),
        updated_at: r.get("updated_at")?,
        synthesized_at: r.get("synthesized_at")?,
    })
}

fn msg_from_row(r: &Row) -> rusqlite::Result<Message> {
    RawMessage::from_row(r).map(RawMessage::parse)
}

/// A message row with its JSON not parsed yet.
struct RawMessage {
    id: String,
    conversation_id: String,
    parent_id: Option<String>,
    role: String,
    content: String,
    meta: String,
    created_at: i64,
}

impl RawMessage {
    fn from_row(r: &Row) -> rusqlite::Result<RawMessage> {
        Ok(RawMessage {
            id: r.get("id")?,
            conversation_id: r.get("conversation_id")?,
            parent_id: r.get("parent_id")?,
            role: r.get("role")?,
            content: r.get("content")?,
            meta: r.get("meta")?,
            created_at: r.get("created_at")?,
        })
    }

    fn parse(self) -> Message {
        Message {
            id: self.id,
            conversation_id: self.conversation_id,
            parent_id: self.parent_id,
            role: self.role,
            content: serde_json::from_str(&self.content).unwrap_or(Value::Array(vec![])),
            meta: serde_json::from_str(&self.meta).unwrap_or(Value::Object(Default::default())),
            created_at: self.created_at,
        }
    }
}

fn project_from_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get("id")?,
        name: r.get("name")?,
        description: r.get("description")?,
        instructions: r.get("instructions")?,
        starred: r.get::<_, i64>("starred")? != 0,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn file_from_row(r: &Row) -> rusqlite::Result<ProjectFile> {
    Ok(ProjectFile {
        id: r.get("id")?,
        project_id: r.get("project_id")?,
        name: r.get("name")?,
        mime: r.get("mime")?,
        size: r.get("size")?,
        kind: r.get("kind")?,
        data: r.get("data")?,
        created_at: r.get("created_at")?,
    })
}

/// Letters that search matches without their accent (the Turkish ones and the common Latin ones).
const UNACCENTED: [(&str, char); 19] = [
    ("àáâãäåāăą", 'a'), ("çćĉċč", 'c'), ("ďđ", 'd'), ("èéêëēĕėęě", 'e'), ("ĝğġģ", 'g'), ("ĥħ", 'h'), ("ìíîïĩīĭįı", 'i'), ("ĵ", 'j'), ("ķ", 'k'), ("ĺļľŀł", 'l'),
    ("ñńņň", 'n'), ("òóôõöøōŏő", 'o'), ("ŕŗř", 'r'), ("śŝşšș", 's'), ("ţťŧț", 't'), ("ùúûüũūŭůűų", 'u'), ("ŵ", 'w'), ("ýÿŷ", 'y'), ("źżž", 'z'),
];

/// `s` as search compares it: lower case, where I, İ and ı are all i (Turkish), and without accents.
pub(crate) fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| if matches!(c, 'I' | 'İ') { 'i'.to_lowercase() } else { c.to_lowercase() })
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .map(|c| if c.is_ascii() { c } else { UNACCENTED.iter().find(|(from, _)| from.contains(c)).map_or(c, |&(_, to)| to) })
        .collect()
}

/// One word of a query as an FTS5 prefix term. FTS5 folds case and accents but keeps ı apart
/// from i (and indexes "I" as i), so a word with i's matches with either at its first four.
fn fts_term(word: &str) -> String {
    let w: Vec<char> = word.replace('"', "\"\"").chars().map(|c| if matches!(c, 'I' | 'İ') { 'i' } else { c }).collect();
    let at: Vec<usize> = (0..w.len()).filter(|&i| matches!(w[i], 'i' | 'ı')).take(4).collect();
    let terms: Vec<String> = (0..1usize << at.len())
        .map(|mask| {
            let mut v = w.clone();
            for (bit, &i) in at.iter().enumerate() {
                v[i] = if (mask >> bit) & 1 == 1 { 'ı' } else { 'i' };
            }
            format!("\"{}\"*", v.into_iter().collect::<String>())
        })
        .collect();
    if terms.len() == 1 {
        terms.concat()
    } else {
        format!("({})", terms.join(" OR "))
    }
}

/// A user query for FTS5: every word must match, in any order (an explicit AND:
/// FTS5 takes no implicit one next to a parenthesized group).
fn fts_query(q: &str) -> String {
    q.split_whitespace().map(fts_term).collect::<Vec<_>>().join(" AND ")
}

/// Bring the schema up to date in one transaction. A database with no
/// version (new, or made before versioning) has SCHEMA, version 1; one
/// from a newer version is refused (its schema may not be readable here).
fn migrate(conn: &mut Connection) -> AppResult<()> {
    let latest = MIGRATIONS.len() as i64 + 1;
    let version = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?.max(1);
    if version > latest {
        return Err(AppError::msg(format!(
            "It was written by a newer version of ClaudeRust (database version {version}, this version reads up to {latest}). Updating ClaudeRust opens it again."
        )));
    }
    if version == latest {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch(SCHEMA)?;
    for m in &MIGRATIONS[version as usize - 1..] {
        tx.execute_batch(m)?;
    }
    tx.pragma_update(None, "user_version", latest)?;
    tx.commit()?;
    Ok(())
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Db> {
        let conn = Connection::open(path)?;
        let mut db = Self::init(conn)?;
        // only alongside WAL: with a rollback journal a reader and the
        // writer would lock each other out
        let wal = db.conn.lock().query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).map(|m| m.eq_ignore_ascii_case("wal")).unwrap_or(false);
        if wal {
            match Connection::open(path).and_then(|r| r.execute_batch("PRAGMA query_only=ON;").map(|_| r)) {
                Ok(r) => db.reader = Some(Mutex::new(r)),
                Err(e) => log::warn!("no separate reading connection to the database: {e}"),
            }
        }
        Ok(db)
    }

    pub fn open_in_memory() -> AppResult<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> AppResult<Db> {
        // secure_delete: what is deleted or replaced is zeroed in the file, so
        // a deleted (or incognito) chat's text doesn't stay in free pages
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL; PRAGMA secure_delete=ON;",
        )?;
        migrate(&mut conn)?;
        let fts = conn
            .execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text, conversation_id UNINDEXED, message_id UNINDEXED);",
            )
            .is_ok();
        Ok(Db {
            conn: Mutex::new(conn),
            reader: None,
            fts,
            indexed: Mutex::new(std::collections::HashMap::new()),
            history: Mutex::new(None),
        })
    }

    /// The connection for a long read-only scan (see `reader`).
    fn read_conn(&self) -> parking_lot::MutexGuard<'_, Connection> {
        match &self.reader {
            Some(r) => r.lock(),
            None => self.conn.lock(),
        }
    }

    // ---------------------------------------------------------------- settings

    pub fn setting_get(&self, key: &str) -> AppResult<Option<String>> {
        let c = self.conn.lock();
        let v = c.prepare_cached("SELECT value FROM settings WHERE key=?1")?.query_row([key], |r| r.get(0)).optional()?;
        Ok(v)
    }

    pub fn setting_set(&self, key: &str, value: &str) -> AppResult<()> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn setting_delete(&self, key: &str) -> AppResult<()> {
        self.conn.lock().execute("DELETE FROM settings WHERE key=?1", [key])?;
        Ok(())
    }

    /// Move the log into the file and empty it, so a secret just rewritten
    /// out of a row (zeroed by secure_delete) leaves no old copy in either.
    pub fn flush_log(&self) {
        let _ = self.conn.lock().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }

    // ----------------------------------------------------------- conversations

    pub fn conv_create(
        &self,
        kind: &str,
        model: &str,
        project_id: Option<&str>,
        cwd: Option<&str>,
        options: &Value,
    ) -> AppResult<Conversation> {
        let now = now_ms();
        let conv = Conversation {
            id: new_id(),
            kind: kind.to_string(),
            title: String::new(),
            model: model.to_string(),
            project_id: project_id.map(|s| s.to_string()),
            cwd: cwd.map(|s| s.to_string()),
            starred: false,
            current_leaf: None,
            options: options.clone(),
            created_at: now,
            updated_at: now,
            // only chats: a code session leaves worktrees, checkpoints and the person's own CLI history
            incognito: kind == "chat" && options["incognito"] == true,
            ..Default::default()
        };
        self.conn.lock().execute(
            "INSERT INTO conversations(id,kind,title,model,project_id,cwd,starred,current_leaf,options,created_at,updated_at,incognito)
             VALUES(?1,?2,?3,?4,?5,?6,0,NULL,?7,?8,?8,?9)",
            params![conv.id, conv.kind, conv.title, conv.model, conv.project_id, conv.cwd, options.to_string(), now, conv.incognito],
        )?;
        Ok(conv)
    }

    pub fn conv_get(&self, id: &str) -> AppResult<Conversation> {
        let c = self.conn.lock();
        let conv = c.prepare_cached("SELECT * FROM conversations WHERE id=?1")?.query_row([id], conv_from_row).optional()?;
        conv.ok_or_else(|| AppError::msg("Conversation not found"))
    }

    /// The conversations the sidebar lists: not incognito, not archived.
    pub fn conv_list(&self, kind: Option<&str>, project_id: Option<&str>) -> AppResult<Vec<Conversation>> {
        self.conv_list_in(false, kind, project_id)
    }

    /// The archived conversations (see `conv_list`).
    pub fn conv_list_archived(&self, kind: Option<&str>) -> AppResult<Vec<Conversation>> {
        self.conv_list_in(true, kind, None)
    }

    /// Every folder a conversation works in, archived ones' too.
    pub fn conv_cwds(&self) -> AppResult<Vec<String>> {
        let c = self.read_conn();
        let mut st = c.prepare("SELECT DISTINCT cwd FROM conversations WHERE cwd IS NOT NULL ORDER BY cwd")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn conv_list_in(&self, archived: bool, kind: Option<&str>, project_id: Option<&str>) -> AppResult<Vec<Conversation>> {
        let c = self.read_conn();
        let mut sql = format!("SELECT * FROM conversations WHERE incognito = 0 AND archived_at IS {}NULL", if archived { "NOT " } else { "" });
        let mut args: Vec<String> = vec![];
        if let Some(k) = kind {
            args.push(k.to_string());
            sql.push_str(&format!(" AND kind=?{}", args.len()));
        }
        if let Some(p) = project_id {
            args.push(p.to_string());
            sql.push_str(&format!(" AND project_id=?{}", args.len()));
        }
        sql.push_str(" ORDER BY updated_at DESC");
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), conv_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn conv_set_title(&self, id: &str, title: &str) -> AppResult<()> {
        self.conn
            .lock()
            .execute("UPDATE conversations SET title=?2 WHERE id=?1", params![id, title])?;
        Ok(())
    }

    pub fn conv_set_starred(&self, id: &str, starred: bool) -> AppResult<()> {
        self.conn.lock().execute(
            "UPDATE conversations SET starred=?2 WHERE id=?1",
            params![id, starred as i64],
        )?;
        Ok(())
    }

    pub fn conv_set_leaf(&self, id: &str, leaf: Option<&str>) -> AppResult<()> {
        self.conn
            .lock()
            .prepare_cached("UPDATE conversations SET current_leaf=?2, updated_at=?3 WHERE id=?1")?
            .execute(params![id, leaf, now_ms()])?;
        Ok(())
    }

    pub fn conv_set_options(&self, id: &str, options: &Value) -> AppResult<()> {
        // an incognito chat stays one (a fork copies the options, and with them the flag)
        self.conn
            .lock()
            .prepare_cached("UPDATE conversations SET options = CASE WHEN incognito = 1 THEN json_set(?2, '$.incognito', json('true')) ELSE ?2 END WHERE id=?1")?
            .execute(params![id, options.to_string()])?;
        Ok(())
    }

    pub fn conv_set_pinned(&self, id: &str, pinned: bool) -> AppResult<()> {
        self.conn.lock().execute("UPDATE conversations SET pinned_at=?2 WHERE id=?1", params![id, pinned.then(now_ms)])?;
        Ok(())
    }

    pub fn conv_set_archived(&self, id: &str, archived: bool) -> AppResult<()> {
        self.conn.lock().execute("UPDATE conversations SET archived_at=?2 WHERE id=?1", params![id, archived.then(now_ms)])?;
        Ok(())
    }

    pub fn conv_set_unread(&self, id: &str, unread: bool) -> AppResult<()> {
        self.conn.lock().prepare_cached("UPDATE conversations SET unread=?2 WHERE id=?1")?.execute(params![id, unread])?;
        Ok(())
    }

    /// Move a conversation into a section of its own kind (`None`: out of any).
    pub fn conv_set_section(&self, id: &str, section_id: Option<&str>) -> AppResult<()> {
        let n = self.conn.lock().execute(
            "UPDATE conversations SET section_id=?2 WHERE id=?1 AND (?2 IS NULL OR EXISTS (SELECT 1 FROM sections s WHERE s.id=?2 AND s.kind=conversations.kind))",
            params![id, section_id],
        )?;
        if n == 0 {
            return Err(AppError::msg("That conversation or section no longer exists."));
        }
        Ok(())
    }

    /// Incognito conversations still stored (a crash's leftovers, at start).
    pub fn incognito_ids(&self) -> AppResult<Vec<String>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT id FROM conversations INDEXED BY idx_conv_incognito WHERE incognito = 1")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn conv_set_model(&self, id: &str, model: &str) -> AppResult<()> {
        self.conn
            .lock()
            .execute("UPDATE conversations SET model=?2 WHERE id=?1", params![id, model])?;
        Ok(())
    }

    pub fn conv_set_project(&self, id: &str, project_id: Option<&str>) -> AppResult<()> {
        self.conn.lock().execute(
            "UPDATE conversations SET project_id=?2 WHERE id=?1",
            params![id, project_id],
        )?;
        Ok(())
    }

    /// Keep the original dates of an imported conversation.
    pub fn conv_set_times(&self, id: &str, created_at: i64, updated_at: i64) -> AppResult<()> {
        self.conn.lock().execute(
            "UPDATE conversations SET created_at=?2, updated_at=?3 WHERE id=?1",
            params![id, created_at, updated_at],
        )?;
        Ok(())
    }

    /// `options.imported_from` of every imported conversation, plus
    /// `claude-code:<session>` for sessions the Claude Code engine ran here,
    /// to skip duplicates.
    pub fn imported_keys(&self) -> AppResult<std::collections::HashSet<String>> {
        let c = self.read_conn();
        let mut st = c.prepare(
            "SELECT json_extract(options,'$.imported_from') FROM conversations WHERE options LIKE '%imported_from%'",
        )?;
        let rows = st.query_map([], |r| r.get::<_, Option<String>>(0))?;
        let mut out: std::collections::HashSet<String> = rows.filter_map(|r| r.ok().flatten()).collect();
        let mut st = c.prepare(
            "SELECT DISTINCT json_extract(meta,'$.cc_session') FROM messages WHERE meta LIKE '%cc_session%'",
        )?;
        let rows = st.query_map([], |r| r.get::<_, Option<String>>(0))?;
        out.extend(rows.filter_map(|r| r.ok().flatten()).map(|s| format!("claude-code:{s}")));
        Ok(out)
    }

    /// (created_at, conversation id, conversation title, kind, model of the
    /// conversation, message meta) of assistant messages since `since_ms`.
    pub fn assistant_usage_since(&self, since_ms: i64) -> AppResult<Vec<UsageRow>> {
        let c = self.read_conn();
        let mut st = c.prepare(
            "SELECT m.created_at, m.conversation_id, c.title, c.kind, c.model, m.meta FROM messages m JOIN conversations c ON c.id = m.conversation_id
             WHERE m.role = 'assistant' AND m.created_at >= ?1 AND json_extract(m.meta, '$.forked_from') IS NULL AND c.incognito = 0",
        )?;
        let rows = st.query_map([since_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Messages the user typed, newest first, without repeats: `(text,
    /// created_at, conversation title)`, filtered by `query` (case-insensitive,
    /// all words must appear).
    pub fn prompt_history(&self, query: &str, limit: usize) -> AppResult<Vec<Prompt>> {
        let written = self.conn.lock().total_changes();
        let cached = self.history.lock().as_ref().filter(|(at, _)| *at == written).map(|(_, all)| all.clone());
        let all = match cached {
            Some(all) => all,
            None => {
                let all = std::sync::Arc::new(self.typed_prompts()?);
                *self.history.lock() = Some((written, all.clone()));
                all
            }
        };
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        Ok(all
            .iter()
            .filter(|(text, _, _)| {
                let lower = text.to_lowercase();
                words.iter().all(|w| lower.contains(w.as_str()))
            })
            .take(limit)
            .cloned()
            .collect())
    }

    /// Every distinct prompt of the newest 5000 the user typed, newest first.
    fn typed_prompts(&self) -> AppResult<Vec<Prompt>> {
        let c = self.read_conn();
        // only the text blocks leave SQLite (attachments can be megabytes of base64)
        let mut st = c.prepare(
            // (CASE guards every json_extract: on anything but a JSON object
            // it fails the whole query, e.g. a bare string in a content array)
            "SELECT CASE WHEN NOT json_valid(m.content) THEN json_array(m.content)
                    WHEN json_type(m.content) = 'array'
                    THEN (SELECT json_group_array(json_extract(j.value, '$.text')) FROM json_each(m.content) j
                          WHERE CASE WHEN j.type = 'object' THEN json_extract(j.value, '$.type') = 'text' ELSE 0 END)
                    ELSE json_array(json_extract(m.content, '$')) END,
                    m.created_at, c.title
             FROM messages m INDEXED BY idx_messages_typed JOIN conversations c ON c.id = m.conversation_id
             WHERE m.role = 'user' AND c.incognito = 0 AND CASE WHEN json_valid(m.meta) AND json_type(m.meta) = 'object'
               THEN json_extract(m.meta, '$.tool_results') IS NULL AND json_extract(m.meta, '$.hook_feedback') IS NULL ELSE 1 END
             ORDER BY m.created_at DESC LIMIT 5000",
        )?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (texts, at, title) in rows.flatten() {
            let Ok(Value::Array(texts)) = serde_json::from_str::<Value>(&texts) else { continue };
            let typed = texts
                .iter()
                .filter_map(|t| t.as_str())
                .find(|t| !t.starts_with("<system-reminder") && !t.starts_with("<earlier_conversation") && !t.starts_with("This session is being continued"))
                .map(str::to_string);
            let Some(text) = typed.map(|t| t.trim().to_string()).filter(|t| t.len() >= 3) else { continue };
            if seen.insert(text.to_lowercase()) {
                out.push((text, at, title));
            }
        }
        Ok(out)
    }

    /// Conversations whose answers wrote or edited the file at `abs` (Write /
    /// Edit / MultiEdit tool calls), most recent first: `(id, title, at)`.
    /// `rel` is the same file relative to `cwd` (tool calls use either; a
    /// relative path only counts for conversations working in `cwd`).
    pub fn conversations_editing(&self, abs: &str, rel: &str, cwd: &str, limit: usize) -> AppResult<Vec<(String, String, i64)>> {
        // the relative path, as it appears inside stored JSON (both forms contain it)
        let q = serde_json::to_string(rel)?;
        let needle = q[1..q.len() - 1].to_string();
        let c = self.read_conn();
        let mut st = c.prepare(
            "SELECT m.conversation_id, c.title, m.created_at, m.content, coalesce(c.cwd, '') FROM messages m JOIN conversations c ON c.id = m.conversation_id
             WHERE m.role = 'assistant' AND instr(m.content, ?1) > 0 AND instr(m.content, '\"tool_use\"') > 0
               AND (instr(m.content, '\"Write\"') > 0 OR instr(m.content, 'Edit\"') > 0)
             ORDER BY m.created_at DESC LIMIT 2000",
        )?;
        let rows = st.query_map([&needle], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?;
        let mut out: Vec<(String, String, i64)> = Vec::new();
        for (id, title, at, content, conv_cwd) in rows.flatten() {
            let same_folder = conv_cwd.trim_end_matches('/') == cwd.trim_end_matches('/');
            if out.iter().any(|(x, _, _)| *x == id) {
                continue;
            }
            let Ok(Value::Array(blocks)) = serde_json::from_str::<Value>(&content) else { continue };
            let edits = blocks.iter().any(|b| {
                b["type"] == "tool_use"
                    && matches!(b["name"].as_str(), Some("Write" | "Edit" | "MultiEdit" | "NotebookEdit"))
                    && [b["input"]["file_path"].as_str(), b["input"]["notebook_path"].as_str()].into_iter().flatten().any(|p| p == abs || (p == rel && same_folder))
            });
            if edits {
                out.push((id, title, at));
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Remove conversations whose import was cut off (app closed mid-import).
    pub fn delete_unfinished_imports(&self) -> AppResult<usize> {
        let c = self.conn.lock();
        let ids: Vec<String> = {
            let mut st = c.prepare(
                "SELECT id FROM conversations WHERE options LIKE '%importing%' AND json_extract(options,'$.importing') = 1",
            )?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for id in &ids {
            if self.fts {
                c.execute("DELETE FROM messages_fts WHERE conversation_id=?1", [id])?;
                self.forget_indexed(id);
            }
            c.execute("DELETE FROM conversations WHERE id=?1", [id])?;
        }
        Ok(ids.len())
    }

    pub fn conv_touch(&self, id: &str) -> AppResult<()> {
        self.conn.lock().execute(
            "UPDATE conversations SET updated_at=?2 WHERE id=?1",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    pub fn conv_delete(&self, id: &str) -> AppResult<()> {
        let c = self.conn.lock();
        if self.fts {
            c.execute("DELETE FROM messages_fts WHERE conversation_id=?1", [id])?;
            self.forget_indexed(id);
        }
        let incognito = c.query_row("SELECT incognito FROM conversations WHERE id=?1", [id], |r| r.get::<_, bool>(0)).optional()?.unwrap_or(false);
        c.execute("DELETE FROM conversations WHERE id=?1", [id])?;
        if incognito {
            // the zeroed pages reach the file, and the log's old copies of them go
            let _ = c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
        Ok(())
    }

    /// The index rows of conversation `id` were removed: no rowid noted for
    /// them may be used again (a new row can get the same one).
    fn forget_indexed(&self, conv_id: &str) {
        self.indexed.lock().retain(|_, row| !matches!(row, Some((c, _)) if c == conv_id));
    }

    // ---------------------------------------------------------------- messages

    pub fn msg_insert(&self, m: &Message) -> AppResult<()> {
        let c = self.conn.lock();
        c.prepare_cached("INSERT INTO messages(id,conversation_id,parent_id,role,content,meta,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)")?.execute(params![
            m.id,
            m.conversation_id,
            m.parent_id,
            m.role,
            m.content.to_string(),
            m.meta.to_string(),
            m.created_at
        ])?;
        if self.fts {
            // new to the database: it has no index row yet
            self.indexed.lock().insert(m.id.clone(), None);
        }
        Ok(())
    }

    pub fn msg_update(&self, id: &str, content: &Value, meta: &Value) -> AppResult<()> {
        self.conn
            .lock()
            .prepare_cached("UPDATE messages SET content=?2, meta=?3 WHERE id=?1")?
            .execute(params![id, content.to_string(), meta.to_string()])?;
        Ok(())
    }

    pub fn msg_get(&self, id: &str) -> AppResult<Message> {
        let c = self.conn.lock();
        let m = c.prepare_cached("SELECT * FROM messages WHERE id=?1")?.query_row([id], msg_from_row).optional()?;
        m.ok_or_else(|| AppError::msg("Message not found"))
    }

    /// Lightweight info about every node of a conversation tree.
    pub fn tree_nodes(&self, conv_id: &str) -> AppResult<Vec<NodeInfo>> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached(
            "SELECT id,parent_id,role,created_at, json_extract(meta,'$.tool_results') AS tr
             FROM messages WHERE conversation_id=?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = st.query_map([conv_id], |r| {
            let tr: Option<i64> = r.get("tr").ok().flatten();
            Ok(NodeInfo {
                id: r.get("id")?,
                parent_id: r.get("parent_id")?,
                role: r.get("role")?,
                created_at: r.get("created_at")?,
                is_tool_result: tr.unwrap_or(0) != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Messages from the root to `leaf` (inclusive), in order.
    pub fn path_to(&self, leaf: &str) -> AppResult<Vec<Message>> {
        // read under the lock, parsed after it: parsing a long session's
        // JSON (tool results of a few MB) needn't hold up other queries
        let rows = {
            let c = self.conn.lock();
            let mut st = c.prepare_cached(
                "WITH RECURSIVE p(id, parent_id, depth) AS (
                    SELECT id, parent_id, 0 FROM messages WHERE id=?1
                    UNION ALL
                    SELECT m.id, m.parent_id, p.depth+1 FROM messages m JOIN p ON m.id = p.parent_id
                 )
                 SELECT messages.* FROM p JOIN messages ON messages.id = p.id ORDER BY p.depth DESC",
            )?;
            let rows = st.query_map([leaf], RawMessage::from_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows.into_iter().map(RawMessage::parse).collect())
    }

    /// Copy a root-to-leaf path of messages into conversation `to` with new
    /// ids (parents remapped) and point the conversation at the copied leaf.
    /// With `keep_cli`, Claude Code links (`cc_session`, `cc_uuid`) are kept,
    /// so the next turn resumes the CLI session at that point as a fork of
    /// it; without, the CLI replays the history in a new session. Returns
    /// `(new_id, content)` pairs so the caller can index them for search.
    pub fn copy_path(&self, path: &[Message], to: &str, keep_cli: bool) -> AppResult<Vec<(String, Value)>> {
        let mut ids: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        let mut out = Vec::with_capacity(path.len());
        for m in path {
            let mut c = m.clone();
            c.id = new_id();
            c.conversation_id = to.to_string();
            c.parent_id = m.parent_id.as_deref().and_then(|p| ids.get(p).cloned());
            if let Some(meta) = c.meta.as_object_mut() {
                if !keep_cli {
                    meta.remove("cc_session");
                    meta.remove("cc_uuid");
                }
                // spend was counted in the original conversation
                meta.insert("forked_from".into(), Value::String(m.id.clone()));
            }
            ids.insert(&m.id, c.id.clone());
            self.msg_insert(&c)?;
            out.push((c.id, c.content));
        }
        self.conv_set_leaf(to, out.last().map(|(id, _)| id.as_str()))?;
        Ok(out)
    }

    /// Mark assistant messages left unfinished by a previous crash/quit as stopped.
    pub fn mark_interrupted(&self) -> AppResult<usize> {
        let c = self.conn.lock();
        // the WHERE is the index's own, so it reads only those rows (and fails if they drift apart)
        Ok(c.execute(
            "UPDATE messages INDEXED BY idx_messages_unfinished SET meta = json_set(meta, '$.stopped', json('true'))
             WHERE role = 'assistant' AND CASE WHEN json_valid(meta)
               THEN json_extract(meta, '$.stop_reason') IS NULL AND json_extract(meta, '$.error') IS NULL AND json_extract(meta, '$.stopped') IS NULL ELSE 0 END",
            [],
        )?)
    }

    pub fn fts_index(&self, conv_id: &str, msg_id: &str, text: &str) -> AppResult<()> {
        if !self.fts {
            return Ok(());
        }
        let c = self.conn.lock();
        // an incognito chat's words are never written to the index
        if c.prepare_cached("SELECT incognito FROM conversations WHERE id=?1")?.query_row([conv_id], |r| r.get::<_, bool>(0)).optional()?.unwrap_or(false) {
            return Ok(());
        }
        let known = self.indexed.lock().get(msg_id).cloned();
        match &known {
            // stored by this process: at most the row it got here
            Some(Some((_, rowid))) => {
                c.prepare_cached("DELETE FROM messages_fts WHERE rowid=?1")?.execute([rowid])?;
            }
            Some(None) => {}
            // stored before this process started: look through the index
            None => {
                c.execute("DELETE FROM messages_fts WHERE message_id=?1", [msg_id])?;
            }
        }
        // its old row is gone: from here it has the new one or none (also
        // when adding it fails), never the old rowid
        let row = if text.trim().is_empty() {
            Ok(None)
        } else {
            c.prepare_cached("INSERT INTO messages_fts(text,conversation_id,message_id) VALUES(?1,?2,?3)")
                .and_then(|mut st| st.execute(params![text, conv_id, msg_id]))
                .map(|_| Some((conv_id.to_string(), c.last_insert_rowid())))
        };
        self.indexed.lock().insert(msg_id.to_string(), row.as_ref().ok().cloned().flatten());
        row?;
        Ok(())
    }

    /// Conversations by title and words, archived ones left out.
    pub fn search(&self, query: &str, limit: usize) -> AppResult<Vec<SearchHit>> {
        self.search_in(query, limit, false)
    }

    /// `search`, with the archived conversations too when `include_archived`.
    pub fn search_in(&self, query: &str, limit: usize, include_archived: bool) -> AppResult<Vec<SearchHit>> {
        let q = query.trim();
        let c = self.read_conn();
        let mut hits: Vec<SearchHit> = vec![];
        let shown = if include_archived { "c.incognito = 0" } else { "c.incognito = 0 AND c.archived_at IS NULL" };
        if q.is_empty() {
            let mut st = c.prepare(&format!(
                "SELECT id,title,kind,updated_at FROM conversations c WHERE {shown} ORDER BY updated_at DESC LIMIT ?1"
            ))?;
            let rows = st.query_map([limit as i64], |r| {
                Ok(SearchHit {
                    conversation_id: r.get(0)?,
                    message_id: None,
                    title: r.get(1)?,
                    snippet: String::new(),
                    kind: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            })?;
            return Ok(rows.collect::<Result<Vec<_>, _>>()?);
        }
        // Titles holding every word, in any order, first; compared folded (SQLite's LIKE folds ASCII only)
        let words: Vec<String> = fold(q).split_whitespace().map(str::to_string).collect();
        {
            let mut st = c.prepare(&format!("SELECT id,title,kind,updated_at FROM conversations c WHERE {shown} ORDER BY updated_at DESC"))?;
            let rows = st.query_map([], |r| {
                Ok(SearchHit {
                    conversation_id: r.get(0)?,
                    message_id: None,
                    title: r.get(1)?,
                    snippet: String::new(),
                    kind: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            })?;
            for h in rows.flatten() {
                let title = fold(&h.title);
                if words.iter().all(|w| title.contains(w.as_str())) {
                    hits.push(h);
                    if hits.len() >= limit {
                        break;
                    }
                }
            }
        }
        if self.fts {
            let mut st = c.prepare(&format!(
                "SELECT f.conversation_id, f.message_id, c.title, snippet(messages_fts, 0, '\u{1}', '\u{2}', '…', 12), c.kind, c.updated_at
                 FROM messages_fts f JOIN conversations c ON c.id = f.conversation_id
                 WHERE messages_fts MATCH ?1 AND {shown} ORDER BY rank LIMIT ?2"
            ))?;
            let fq = fts_query(q);
            if !fq.is_empty() {
                if let Ok(rows) = st.query_map(params![fq, limit as i64], |r| {
                    Ok(SearchHit {
                        conversation_id: r.get(0)?,
                        message_id: r.get(1)?,
                        title: r.get(2)?,
                        snippet: r.get(3)?,
                        kind: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                }) {
                    for h in rows.flatten() {
                        if !hits.iter().any(|x| x.conversation_id == h.conversation_id) {
                            hits.push(h);
                        }
                    }
                }
            }
        } else {
            // without FTS5: every word, as typed
            let likes: Vec<String> = q.split_whitespace().map(|w| format!("%{}%", w.replace('%', "\\%").replace('_', "\\_"))).collect();
            let every = vec!["m.content LIKE ? ESCAPE '\\'"; likes.len()].join(" AND ");
            let mut st = c.prepare(&format!(
                "SELECT m.conversation_id, m.id, c.title, substr(m.content, 1, 200), c.kind, c.updated_at
                 FROM messages m JOIN conversations c ON c.id = m.conversation_id
                 WHERE {shown} AND {every} ORDER BY c.updated_at DESC LIMIT {limit}"
            ))?;
            let rows = st.query_map(rusqlite::params_from_iter(&likes), |r| {
                Ok(SearchHit {
                    conversation_id: r.get(0)?,
                    message_id: r.get(1)?,
                    title: r.get(2)?,
                    snippet: r.get(3)?,
                    kind: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            })?;
            for h in rows.flatten() {
                if !hits.iter().any(|x| x.conversation_id == h.conversation_id) {
                    hits.push(h);
                }
            }
        }
        hits.truncate(limit);
        Ok(hits)
    }

    /// Full-text search over past messages (for the conversation_search tool).
    pub fn search_messages(&self, query: &str, exclude_conv: &str, limit: usize) -> AppResult<Vec<RecentChat>> {
        let c = self.read_conn();
        let mut out = vec![];
        if self.fts {
            let fq = fts_query(query);
            if fq.is_empty() {
                return Ok(out);
            }
            let mut st = c.prepare(
                "SELECT f.conversation_id, c.title, m.created_at, m.role, f.text
                 FROM messages_fts f JOIN conversations c ON c.id = f.conversation_id
                 JOIN messages m ON m.id = f.message_id
                 WHERE messages_fts MATCH ?1 AND f.conversation_id != ?2 ORDER BY rank LIMIT ?3",
            )?;
            let rows = st.query_map(params![fq, exclude_conv, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            for r in rows.flatten() {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// Most recent conversations with their first user message.
    pub fn recent_with_first_message(&self, exclude_conv: &str, limit: usize) -> AppResult<Vec<(String, String, i64, String)>> {
        let c = self.read_conn();
        let mut st = c.prepare(
            "SELECT c.id, c.title, c.updated_at,
                    (SELECT m.content FROM messages m WHERE m.conversation_id = c.id AND m.role = 'user'
                     ORDER BY m.created_at ASC LIMIT 1)
             FROM conversations c WHERE c.id != ?1 AND c.kind = 'chat' AND c.incognito = 0 ORDER BY c.updated_at DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![exclude_conv, limit as i64], |r| {
            let content: Option<String> = r.get(3)?;
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, content.unwrap_or_default()))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ---------------------------------------------------------------- projects

    pub fn project_create(&self, name: &str, description: &str) -> AppResult<Project> {
        let now = now_ms();
        let p = Project {
            id: new_id(),
            name: name.to_string(),
            description: description.to_string(),
            instructions: String::new(),
            starred: false,
            created_at: now,
            updated_at: now,
        };
        self.conn.lock().execute(
            "INSERT INTO projects(id,name,description,instructions,starred,created_at,updated_at) VALUES(?1,?2,?3,'',0,?4,?4)",
            params![p.id, p.name, p.description, now],
        )?;
        Ok(p)
    }

    pub fn project_list(&self) -> AppResult<Vec<Project>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT * FROM projects ORDER BY updated_at DESC")?;
        let rows = st.query_map([], project_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn project_get(&self, id: &str) -> AppResult<Project> {
        let c = self.conn.lock();
        c.query_row("SELECT * FROM projects WHERE id=?1", [id], project_from_row)
            .optional()?
            .ok_or_else(|| AppError::msg("Project not found"))
    }

    pub fn project_update(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        instructions: Option<&str>,
        starred: Option<bool>,
    ) -> AppResult<()> {
        let c = self.conn.lock();
        if let Some(v) = name {
            c.execute("UPDATE projects SET name=?2 WHERE id=?1", params![id, v])?;
        }
        if let Some(v) = description {
            c.execute("UPDATE projects SET description=?2 WHERE id=?1", params![id, v])?;
        }
        if let Some(v) = instructions {
            c.execute("UPDATE projects SET instructions=?2 WHERE id=?1", params![id, v])?;
        }
        if let Some(v) = starred {
            c.execute("UPDATE projects SET starred=?2 WHERE id=?1", params![id, v as i64])?;
        }
        c.execute(
            "UPDATE projects SET updated_at=?2 WHERE id=?1",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    pub fn project_delete(&self, id: &str) -> AppResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM projects WHERE id=?1", [id])?;
        c.execute("DELETE FROM chat_memory WHERE scope=?1", [id])?;
        Ok(())
    }

    pub fn project_file_add(&self, f: &ProjectFile) -> AppResult<()> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO project_files(id,project_id,name,mime,size,kind,data,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![f.id, f.project_id, f.name, f.mime, f.size, f.kind, f.data, f.created_at],
        )?;
        c.execute(
            "UPDATE projects SET updated_at=?2 WHERE id=?1",
            params![f.project_id, now_ms()],
        )?;
        Ok(())
    }

    pub fn project_files(&self, project_id: &str) -> AppResult<Vec<ProjectFile>> {
        let c = self.read_conn();
        let mut st = c.prepare(
            "SELECT * FROM project_files WHERE project_id=?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = st.query_map([project_id], file_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn project_file_delete(&self, id: &str) -> AppResult<()> {
        self.conn.lock().execute("DELETE FROM project_files WHERE id=?1", [id])?;
        Ok(())
    }

    // --------------------------------------------------------------- artifacts

    pub fn artifact_upsert(&self, a: &ArtifactRow) -> AppResult<()> {
        let c = self.conn.lock();
        c.execute(
            "DELETE FROM artifacts WHERE message_id=?1 AND identifier=?2",
            params![a.message_id, a.identifier],
        )?;
        c.execute(
            "INSERT INTO artifacts(id,conversation_id,message_id,identifier,kind,title,language,content,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![a.id, a.conversation_id, a.message_id, a.identifier, a.kind, a.title, a.language, a.content, a.created_at],
        )?;
        Ok(())
    }

    /// Latest version of each artifact identifier, per conversation.
    pub fn artifact_list(&self, limit: usize) -> AppResult<Vec<ArtifactRow>> {
        let c = self.read_conn();
        let mut st = c.prepare(
            "SELECT a.*, c.title AS conversation_title FROM artifacts a
             JOIN conversations c ON c.id = a.conversation_id
             WHERE c.incognito = 0 AND a.created_at = (SELECT MAX(b.created_at) FROM artifacts b WHERE b.conversation_id=a.conversation_id AND b.identifier=a.identifier)
             ORDER BY a.created_at DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| {
            Ok(ArtifactRow {
                id: r.get("id")?,
                conversation_id: r.get("conversation_id")?,
                message_id: r.get("message_id")?,
                identifier: r.get("identifier")?,
                kind: r.get("kind")?,
                title: r.get("title")?,
                language: r.get("language")?,
                content: r.get("content")?,
                created_at: r.get("created_at")?,
                conversation_title: r.get("conversation_title")?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn artifact_delete(&self, conv_id: &str, identifier: &str) -> AppResult<()> {
        self.conn.lock().execute(
            "DELETE FROM artifacts WHERE conversation_id=?1 AND identifier=?2",
            params![conv_id, identifier],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- sections

    pub fn section_create(&self, kind: &str, name: &str) -> AppResult<Section> {
        let s = Section { id: new_id(), kind: kind.to_string(), name: name.to_string(), created_at: now_ms() };
        self.conn.lock().execute("INSERT INTO sections(id,kind,name,created_at) VALUES(?1,?2,?3,?4)", params![s.id, s.kind, s.name, s.created_at])?;
        Ok(s)
    }

    pub fn section_list(&self, kind: &str) -> AppResult<Vec<Section>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT id,kind,name,created_at FROM sections WHERE kind=?1 ORDER BY created_at, rowid")?;
        let rows = st.query_map([kind], |r| Ok(Section { id: r.get(0)?, kind: r.get(1)?, name: r.get(2)?, created_at: r.get(3)? }))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn section_rename(&self, id: &str, name: &str) -> AppResult<()> {
        if self.conn.lock().execute("UPDATE sections SET name=?2 WHERE id=?1", params![id, name])? == 0 {
            return Err(AppError::msg("That section no longer exists."));
        }
        Ok(())
    }

    /// Its conversations stay, outside any section.
    pub fn section_delete(&self, id: &str) -> AppResult<()> {
        self.conn.lock().execute("DELETE FROM sections WHERE id=?1", [id])?;
        Ok(())
    }

    // ------------------------------------------------------------------ memory

    pub fn memory_get(&self, scope: &str) -> AppResult<Option<MemoryRow>> {
        Ok(self.conn.lock().query_row("SELECT * FROM chat_memory WHERE scope=?1", [scope], memory_from_row).optional()?)
    }

    pub fn memory_list(&self) -> AppResult<Vec<MemoryRow>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT * FROM chat_memory ORDER BY updated_at DESC")?;
        let rows = st.query_map([], memory_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The memory of `scope` as the person wrote it (its sources stay).
    pub fn memory_set(&self, scope: &str, text: &str) -> AppResult<()> {
        self.conn.lock().execute(
            "INSERT INTO chat_memory(scope,text,updated_at) VALUES(?1,?2,?3) ON CONFLICT(scope) DO UPDATE SET text=excluded.text, updated_at=excluded.updated_at",
            params![scope, text, now_ms()],
        )?;
        Ok(())
    }

    /// Store a synthesis written from the memory as it was at `read_at`
    /// (its `updated_at`; None: there was none). False, and nothing stored,
    /// when it changed meanwhile (the person edited it).
    pub fn memory_store_synthesis(&self, scope: &str, text: &str, sources: &[String], read_at: Option<i64>) -> AppResult<bool> {
        let (now, sources) = (now_ms(), serde_json::to_string(sources)?);
        let c = self.conn.lock();
        let n = match read_at {
            Some(at) => c.execute(
                "UPDATE chat_memory SET text=?2, sources=?3, updated_at=?4, synthesized_at=?4 WHERE scope=?1 AND updated_at=?5",
                params![scope, text, sources, now, at],
            )?,
            None => c.execute(
                "INSERT INTO chat_memory(scope,text,sources,updated_at,synthesized_at) VALUES(?1,?2,?3,?4,?4) ON CONFLICT(scope) DO NOTHING",
                params![scope, text, sources, now],
            )?,
        };
        Ok(n == 1)
    }

    pub fn memory_delete(&self, scope: &str) -> AppResult<()> {
        self.conn.lock().execute("DELETE FROM chat_memory WHERE scope=?1", [scope])?;
        Ok(())
    }

    // -------------------------------------------------------- artifact storage

    /// The value of `key` among what artifact `scope` keeps (its `shared` keys or its own).
    pub fn storage_get(&self, scope: &str, shared: bool, key: &str) -> AppResult<Option<String>> {
        let c = self.conn.lock();
        Ok(c.query_row("SELECT value FROM artifact_storage WHERE scope=?1 AND shared=?2 AND key=?3", params![scope, shared, key], |r| r.get(0)).optional()?)
    }

    /// Its keys that start with `prefix`, in order.
    pub fn storage_keys(&self, scope: &str, shared: bool, prefix: &str) -> AppResult<Vec<String>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT key FROM artifact_storage WHERE scope=?1 AND shared=?2 ORDER BY key")?;
        let keys = st.query_map(params![scope, shared], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(keys.into_iter().filter(|k| k.starts_with(prefix)).collect())
    }

    /// Keep `value` under `key` for artifact `scope` of conversation `conv`, unless
    /// all its keys and values, own and shared, would then hold more than
    /// `budget` bytes: false, and nothing changed.
    pub fn storage_set(&self, conv: &str, scope: &str, shared: bool, key: &str, value: &str, budget: usize) -> AppResult<bool> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let others: i64 = tx.query_row(
            "SELECT COALESCE(SUM(length(CAST(key AS BLOB)) + length(CAST(value AS BLOB))), 0) FROM artifact_storage WHERE scope=?1 AND NOT (shared=?2 AND key=?3)",
            params![scope, shared, key],
            |r| r.get(0),
        )?;
        if others as usize + key.len() + value.len() > budget {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO artifact_storage(conversation_id,scope,shared,key,value) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(scope,shared,key) DO UPDATE SET value=excluded.value",
            params![conv, scope, shared, key, value],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Remove `key`: whether it was there.
    pub fn storage_delete(&self, scope: &str, shared: bool, key: &str) -> AppResult<bool> {
        Ok(self.conn.lock().execute("DELETE FROM artifact_storage WHERE scope=?1 AND shared=?2 AND key=?3", params![scope, shared, key])? == 1)
    }

    /// Add what an older version kept elsewhere: (conversation, scope, shared, key, value)
    /// rows, a key already here kept as it is, those of a conversation that is gone
    /// left out. How many were added.
    pub fn storage_import(&self, rows: &[(String, String, bool, String, String)]) -> AppResult<usize> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let mut added = 0;
        for (conv, scope, shared, key, value) in rows {
            added += tx.execute(
                "INSERT OR IGNORE INTO artifact_storage(conversation_id,scope,shared,key,value) SELECT ?1,?2,?3,?4,?5 WHERE EXISTS (SELECT 1 FROM conversations WHERE id=?1)",
                params![conv, scope, shared, key, value],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// The chats a memory is written from, most recently updated first: not
    /// incognito, answered, updated after `since`; those of `project`, or
    /// (None) those outside any project, with `every_project` those of any
    /// project but the `excluded` ones too.
    pub fn memory_sources(&self, project: Option<&str>, every_project: bool, excluded: &[String], since: i64, limit: usize) -> AppResult<Vec<Conversation>> {
        let c = self.read_conn();
        let mut st = c.prepare(
            "SELECT * FROM conversations WHERE kind = 'chat' AND incognito = 0 AND current_leaf IS NOT NULL AND updated_at > ?1
               AND CASE WHEN ?2 IS NOT NULL THEN project_id IS ?2
                        WHEN ?3 THEN project_id IS NULL OR project_id NOT IN (SELECT value FROM json_each(?4))
                        ELSE project_id IS NULL END
             ORDER BY updated_at DESC LIMIT ?5",
        )?;
        let rows = st.query_map(params![since, project, every_project, serde_json::to_string(excluded)?, limit as i64], conv_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn conversations_that_edited_a_file() {
        let db = Db::open_in_memory().unwrap();
        let a = db.conv_create("code", "m", None, Some("/p"), &json!({})).unwrap();
        let b = db.conv_create("code", "m", None, Some("/p"), &json!({})).unwrap();
        let other = db.conv_create("code", "m", None, Some("/q"), &json!({})).unwrap();
        let mk = |id: &str, conv: &str, content: Value, t: i64| Message { id: id.into(), conversation_id: conv.into(), parent_id: None, role: "assistant".into(), content, meta: json!({}), created_at: t };
        db.msg_insert(&mk("6", &other.id, json!([{"type":"tool_use","id":"t","name":"Write","input":{"file_path":"src/x.rs","content":""}}]), 9)).unwrap();
        db.msg_insert(&mk("1", &a.id, json!([{"type":"tool_use","id":"t","name":"Edit","input":{"file_path":"/p/src/x.rs","old_string":"a","new_string":"b"}}]), 1)).unwrap();
        db.msg_insert(&mk("2", &b.id, json!([{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"/p/src/x.rs"}}]), 2)).unwrap();
        db.msg_insert(&mk("3", &b.id, json!([{"type":"tool_use","id":"t","name":"Write","input":{"file_path":"/p/src/x.rs.bak","content":""}}]), 3)).unwrap();
        db.msg_insert(&mk("4", &a.id, json!([{"type":"tool_use","id":"t","name":"Write","input":{"file_path":"/p/src/x.rs","content":""}}]), 4)).unwrap();
        db.msg_insert(&mk("5", &b.id, json!([{"type":"tool_use","id":"t","name":"Edit","input":{"file_path":"src/x.rs","old_string":"a","new_string":"b"}}]), 0)).unwrap();
        let r = db.conversations_editing("/p/src/x.rs", "src/x.rs", "/p", 10).unwrap();
        assert_eq!(r.len(), 2, "relative paths count too, but only in the same folder");
        let r: Vec<_> = r.into_iter().filter(|x| x.0 == a.id).collect();
        assert_eq!(r.len(), 1, "reads and other files don't count; one row per conversation");
        assert_eq!(r[0].0, a.id);
        assert_eq!(r[0].2, 4);
    }

    #[test]
    fn prompt_history_dedupes_and_filters() {
        let db = Db::open_in_memory().unwrap();
        let conv = db.conv_create("code", "m", None, None, &json!({})).unwrap();
        let mk = |id: &str, content: Value, meta: Value, t: i64| Message {
            id: id.into(),
            conversation_id: conv.id.clone(),
            parent_id: None,
            role: "user".into(),
            content,
            meta,
            created_at: t,
        };
        db.msg_insert(&mk("1", json!([{"type":"text","text":"Run the tests and fix failures"}]), json!({}), 1)).unwrap();
        db.msg_insert(&mk("2", json!([{"type":"tool_result","tool_use_id":"x","content":"ok"}]), json!({"tool_results": true}), 2)).unwrap();
        db.msg_insert(&mk("3", json!([{"type":"text","text":"review the diff"},{"type":"text","text":"<system-reminder>file</system-reminder>"}]), json!({}), 3)).unwrap();
        db.msg_insert(&mk("4", json!([{"type":"text","text":"run the tests and fix failures"}]), json!({}), 4)).unwrap();
        db.msg_insert(&mk("5", json!([{"type":"document","source":{}},{"type":"text","text":"summarize this log"}]), json!({}), 5)).unwrap();
        // odd rows (a bare string in the content array, plain-text meta) don't break the query
        db.msg_insert(&mk("6", json!(["stray", {"type":"text","text":"odd row"}]), json!("x"), 0)).unwrap();
        let all: Vec<String> = db.prompt_history("", 10).unwrap().into_iter().map(|r| r.0).collect();
        assert_eq!(all, vec!["summarize this log", "run the tests and fix failures", "review the diff", "odd row"]);
        let hits: Vec<String> = db.prompt_history("TESTS fix", 10).unwrap().into_iter().map(|r| r.0).collect();
        assert_eq!(hits, vec!["run the tests and fix failures"]);
        assert_eq!(db.prompt_history("", 1).unwrap().len(), 1);
        // keystrokes filter what the first call read, until something is written
        let read = || db.history.lock().as_ref().unwrap().1.clone();
        let before = read();
        db.prompt_history("log", 10).unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &read()));
        db.msg_insert(&mk("7", json!([{"type":"text","text":"a new log question"}]), json!({}), 7)).unwrap();
        let hits = db.prompt_history("log", 10).unwrap();
        assert_eq!(hits[0].0, "a new log question");
        assert!(!std::sync::Arc::ptr_eq(&before, &read()));
        db.conv_set_title(&conv.id, "renamed").unwrap();
        assert_eq!(db.prompt_history("log", 10).unwrap()[0].2, "renamed");
    }

    #[test]
    fn copy_path_forks_a_branch() {
        let db = Db::open_in_memory().unwrap();
        let conv = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let mk = |id: &str, parent: Option<&str>, role: &str| Message {
            id: id.into(),
            conversation_id: conv.id.clone(),
            parent_id: parent.map(|s| s.into()),
            role: role.into(),
            content: json!([{"type":"text","text":id}]),
            meta: json!({"cc_uuid": "x", "keep": 1}),
            created_at: 1,
        };
        db.msg_insert(&mk("u1", None, "user")).unwrap();
        db.msg_insert(&mk("a1", Some("u1"), "assistant")).unwrap();
        db.msg_insert(&mk("u2", Some("a1"), "user")).unwrap();
        db.msg_insert(&mk("a2", Some("u2"), "assistant")).unwrap();
        let fork = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let copied = db.copy_path(&db.path_to("a1").unwrap(), &fork.id, true).unwrap();
        assert_eq!(copied.len(), 2);
        let leaf = db.conv_get(&fork.id).unwrap().current_leaf.unwrap();
        assert_eq!(leaf, copied[1].0);
        let p = db.path_to(&leaf).unwrap();
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(|m| m.conversation_id == fork.id && m.meta["cc_uuid"] == "x" && m.meta["keep"] == 1));
        assert_eq!(p[1].meta["forked_from"], "a1");
        // without the CLI links (a fork into another folder)
        let fork2 = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let c2 = db.copy_path(&db.path_to("a1").unwrap(), &fork2.id, false).unwrap();
        assert!(db.path_to(&c2[1].0).unwrap().iter().all(|m| m.meta.get("cc_uuid").is_none() && m.meta["keep"] == 1));
        // copied answers aren't counted again in the usage totals
        let all = db.assistant_usage_since(0).unwrap();
        assert_eq!(all.len(), 2, "only the original's two answers");
        assert!(all.iter().all(|r| r.1 == conv.id));
        assert_eq!(p[0].parent_id, None);
        assert_eq!(p[1].parent_id.as_deref(), Some(p[0].id.as_str()));
        assert_eq!(p[1].content[0]["text"], "a1");
        // the original is untouched
        assert_eq!(db.path_to("a2").unwrap().len(), 4);
        assert!(db.path_to("a2").unwrap().iter().all(|m| m.conversation_id == conv.id));
    }

    fn fts_rows(db: &Db, msg_id: &str) -> Vec<String> {
        let c = db.conn.lock();
        let mut st = c.prepare("SELECT text FROM messages_fts WHERE message_id=?1").unwrap();
        let rows = st.query_map([msg_id], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    fn text_msg(id: &str, conv: &str) -> Message {
        Message { id: id.into(), conversation_id: conv.into(), parent_id: None, role: "user".into(), content: json!([{"type":"text","text":id}]), meta: json!({}), created_at: 1 }
    }

    #[test]
    fn indexing_a_message_again_replaces_its_row() {
        let dir = std::env::temp_dir().join(format!("cr-fts-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let db = Db::open(&path).unwrap();
        assert!(db.reader.is_some(), "a file database gets its reading connection");
        let conv = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        db.msg_insert(&text_msg("m1", &conv.id)).unwrap();
        db.fts_index(&conv.id, "m1", "alpha words").unwrap();
        db.fts_index(&conv.id, "m1", "beta words").unwrap();
        assert_eq!(fts_rows(&db, "m1"), vec!["beta words"]);
        db.fts_index(&conv.id, "m1", " ").unwrap();
        assert!(fts_rows(&db, "m1").is_empty(), "empty text: no row");
        db.fts_index(&conv.id, "m1", "gamma").unwrap();
        // a message indexed by an earlier run of the app (nothing noted)
        drop(db);
        let db = Db::open(&path).unwrap();
        db.fts_index(&conv.id, "m1", "delta").unwrap();
        assert_eq!(fts_rows(&db, "m1"), vec!["delta"]);
        db.fts_index(&conv.id, "m1", "epsilon").unwrap();
        assert_eq!(fts_rows(&db, "m1"), vec!["epsilon"]);
        assert_eq!(db.search("epsilon", 10).unwrap().len(), 1);
        assert!(db.search("delta", 10).unwrap().is_empty());
        // a deleted conversation's rowids can be given to new rows: indexing
        // one of its messages afterwards (a turn finishing after the delete)
        // must not remove another message's row
        let a = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        db.msg_insert(&text_msg("x1", &a.id)).unwrap();
        db.fts_index(&a.id, "x1", "first").unwrap();
        let rowid = |db: &Db, id: &str| db.conn.lock().query_row("SELECT rowid FROM messages_fts WHERE message_id=?1", [id], |r| r.get::<_, i64>(0)).unwrap();
        let x1_row = rowid(&db, "x1");
        db.conv_delete(&a.id).unwrap();
        let b = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        db.msg_insert(&text_msg("y1", &b.id)).unwrap();
        db.fts_index(&b.id, "y1", "second").unwrap();
        assert_eq!(rowid(&db, "y1"), x1_row, "the rowid was given again");
        db.fts_index(&a.id, "x1", "late").unwrap();
        assert_eq!(fts_rows(&db, "y1"), vec!["second"]);
        assert_eq!(fts_rows(&db, "x1"), vec!["late"]);
        assert_eq!(fts_rows(&db, "m1"), vec!["epsilon"]);
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_older_database_is_migrated_and_a_newer_one_refused() {
        let dir = std::env::temp_dir().join(format!("cr-mig-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let latest = MIGRATIONS.len() as i64 + 1;
        let version = |c: &Connection| c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap();
        let has_index = |c: &Connection, name: &str| c.query_row("SELECT count(*) FROM sqlite_master WHERE name=?1", [name], |r| r.get::<_, i64>(0)).unwrap() == 1;
        // as the versions before schema versioning left it: version 0, one unfinished answer
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(SCHEMA).unwrap();
            c.execute("INSERT INTO conversations(id,model,created_at,updated_at) VALUES('c','m',1,1)", []).unwrap();
            c.execute("INSERT INTO messages(id,conversation_id,role,content,meta,created_at) VALUES('a','c','assistant','[]','{}',1)", []).unwrap();
            // a migration that fails changes nothing: what it did before is rolled back too
            c.execute_batch("CREATE INDEX idx_messages_typed ON messages(role)").unwrap();
        }
        assert!(Db::open(&path).is_err());
        {
            let c = Connection::open(&path).unwrap();
            assert_eq!(version(&c), 0);
            assert!(!has_index(&c, "idx_messages_unfinished"));
            c.execute_batch("DROP INDEX idx_messages_typed").unwrap();
        }
        let db = Db::open(&path).unwrap();
        {
            let c = db.conn.lock();
            assert_eq!(version(&c), latest);
            assert!(has_index(&c, "idx_messages_unfinished") && has_index(&c, "idx_messages_typed") && has_index(&c, "idx_artifact_storage_conv"));
        }
        assert_eq!(db.conv_get("c").unwrap().model, "m");
        assert!(db.storage_set("c", "c/a", false, "k", "v", 100).unwrap(), "an older chat's artifacts can store");
        assert_eq!(db.mark_interrupted().unwrap(), 1, "the new index finds the old row");
        assert_eq!(db.msg_get("a").unwrap().meta["stopped"], true);
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(version(&db.conn.lock()), latest, "an up-to-date database opens as it is");
        // written by a newer version: refused, untouched (lib.rs then offers a new database)
        db.conn.lock().pragma_update(None, "user_version", latest + 1).unwrap();
        drop(db);
        let e = Db::open(&path).err().unwrap().to_string();
        assert!(e.contains("newer version") && e.contains(&format!("version {}", latest + 1)), "{e}");
        assert_eq!(version(&Connection::open(&path).unwrap()), latest + 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_reading_connection_sees_every_write_at_once() {
        let dir = std::env::temp_dir().join(format!("cr-rd-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("t.db")).unwrap();
        for i in 0..20 {
            let conv = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
            assert_eq!(db.conv_list(None, None).unwrap().len(), i + 1);
            let mut m = text_msg(&format!("typed message {i}"), &conv.id);
            m.id = new_id();
            db.msg_insert(&m).unwrap();
            assert_eq!(db.prompt_history("", 100).unwrap().len(), i + 1);
            db.fts_index(&conv.id, &m.id, &format!("needle{i}")).unwrap();
            assert_eq!(db.search(&format!("needle{i}"), 10).unwrap().len(), 1);
            db.conv_set_title(&conv.id, &format!("renamed {i}")).unwrap();
            assert_eq!(db.conv_list(None, None).unwrap().into_iter().find(|c| c.id == conv.id).unwrap().title, format!("renamed {i}"));
        }
        // it can't write
        assert!(db.read_conn().execute("DELETE FROM conversations", []).is_err());
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A database the size a heavy user builds up: `chats` chats of 20
    /// messages, `codes` code sessions of 150 (tool calls with file paths,
    /// tool results of a few KB, usage in the meta). Returns the longest
    /// session's leaf.
    fn fill_large(db: &Db, chats: usize, codes: usize) -> String {
        let pad = |n: usize| "lorem ipsum dolor sit amet, consectetur adipiscing elit ".repeat(n / 56 + 1)[..n].to_string();
        db.conn.lock().execute_batch("BEGIN").unwrap();
        let mut t = 0i64;
        let mut leaf = String::new();
        for (i, kind, n) in (0..chats).map(|i| (i, "chat", 20)).chain((0..codes).map(|i| (i, "code", 150))) {
            let conv = db.conv_create(kind, "claude-opus-5-5", None, Some("/home/u/project"), &json!({"memory": pad(3000), "env_snapshot": pad(800)})).unwrap();
            db.conv_set_title(&conv.id, &format!("{kind} {i} about things")).unwrap();
            let mut parent: Option<String> = None;
            for j in 0..n {
                t += 1;
                let id = new_id();
                let (role, content, meta) = if j % 2 == 0 {
                    if kind == "code" && j > 0 {
                        ("user", json!([{"type": "tool_result", "tool_use_id": "t", "content": pad(4000)}]), json!({"tool_results": true}))
                    } else {
                        ("user", json!([{"type": "text", "text": format!("question {i}/{j}: {}", pad(200))}]), json!({}))
                    }
                } else if kind == "code" {
                    let tool = ["Read", "Edit", "Write", "Bash"][j % 4];
                    let content = json!([{"type": "text", "text": pad(400)}, {"type": "tool_use", "id": "t", "name": tool, "input": {"file_path": format!("/home/u/project/src/file{}.rs", j % 40), "old_string": pad(300), "new_string": pad(300)}}]);
                    ("assistant", content, json!({"model": "claude-opus-5-5", "stop_reason": "tool_use", "usage": {"input_tokens": 1200, "output_tokens": 300, "cache_read_input_tokens": 40000}}))
                } else {
                    ("assistant", json!([{"type": "text", "text": pad(2000)}]), json!({"model": "claude-opus-5-5", "stop_reason": "end_turn", "usage": {"input_tokens": 900, "output_tokens": 500}}))
                };
                let m = Message { id: id.clone(), conversation_id: conv.id.clone(), parent_id: parent.clone(), role: role.into(), content, meta, created_at: t };
                db.msg_insert(&m).unwrap();
                // straight in (fts_index first deletes the message's old row)
                db.conn.lock().execute("INSERT INTO messages_fts(text,conversation_id,message_id) VALUES(?1,?2,?3)", params![render_text(&m.content), conv.id, id]).unwrap();
                parent = Some(id);
            }
            db.conv_set_leaf(&conv.id, parent.as_deref()).unwrap();
            leaf = parent.unwrap();
        }
        db.conn.lock().execute_batch("COMMIT").unwrap();
        leaf
    }

    fn render_text(content: &Value) -> String {
        content.as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n")
    }

    /// `cargo test --release -p clauderust --lib bench_db -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_db() {
        use std::time::{Duration, Instant};
        let dir = std::env::temp_dir().join(format!("cr-dbbench-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bench.db");
        let db = std::sync::Arc::new(Db::open(&path).unwrap());
        let t = Instant::now();
        let leaf = fill_large(&db, 900, 200);
        println!("filled in {:?}, {:.0} MB", t.elapsed(), std::fs::metadata(&path).unwrap().len() as f64 / 1e6);
        let time = |name: &str, f: &mut dyn FnMut()| {
            f();
            let n = 5;
            let t = Instant::now();
            for _ in 0..n {
                f();
            }
            println!("{name:<28} {:>9.2?}", t.elapsed() / n);
        };
        time("conv_list (all)", &mut || drop(db.conv_list(None, None).unwrap()));
        time("path_to (150 messages)", &mut || drop(db.path_to(&leaf).unwrap()));
        let conv_id = db.msg_get(&leaf).unwrap().conversation_id;
        time("tree_nodes", &mut || drop(db.tree_nodes(&conv_id).unwrap()));
        time("prompt_history", &mut || drop(db.prompt_history("", 60).unwrap()));
        time("conversations_editing", &mut || drop(db.conversations_editing("/home/u/project/src/file3.rs", "src/file3.rs", "/home/u/project", 20).unwrap()));
        time("assistant_usage_since(0)", &mut || drop(db.assistant_usage_since(0).unwrap()));
        time("search", &mut || drop(db.search("question ipsum", 40).unwrap()));
        time("conv_get", &mut || drop(db.conv_get(&conv_id).unwrap()));
        // as the engine stores an answer: a new message, then its index row
        time("msg_insert + fts_index", &mut || {
            let id = new_id();
            db.msg_insert(&Message { id: id.clone(), conversation_id: conv_id.clone(), parent_id: Some(leaf.clone()), role: "assistant".into(), content: json!([]), meta: json!({}), created_at: 0 }).unwrap();
            db.fts_index(&conv_id, &id, "an answer to index").unwrap();
        });
        // a cheap call (as every command makes) while heavy reads run
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (db, stop) = (db.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = db.prompt_history("", 60);
                    let _ = db.assistant_usage_since(0);
                }
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        let (mut worst, mut total) = (Duration::ZERO, Duration::ZERO);
        let m = db.msg_get(&leaf).unwrap();
        let n = 100;
        for _ in 0..n {
            let t = Instant::now();
            db.conv_get(&conv_id).unwrap();
            db.msg_update(&leaf, &m.content, &m.meta).unwrap();
            let e = t.elapsed();
            worst = worst.max(e);
            total += e;
            std::thread::sleep(Duration::from_millis(2));
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        reader.join().unwrap();
        println!("conv_get + msg_update during heavy reads: {:.2?} average, {:.2?} worst", total / n, worst);
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What a start costs on a large history (as lib.rs's setup runs it),
    /// first from a database made before versioning, then once up to date.
    /// `cargo test --no-default-features --lib bench_startup -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_startup() {
        use std::time::Instant;
        let dir = std::env::temp_dir().join(format!("cr-startbench-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bench.db");
        {
            let db = Db::open(&path).unwrap();
            fill_large(&db, 900, 200);
            let n: i64 = db.conn.lock().query_row("SELECT count(*) FROM messages", [], |r| r.get(0)).unwrap();
            println!("{n} messages, {:.0} MB", std::fs::metadata(&path).unwrap().len() as f64 / 1e6);
            // as left by the versions before schema versioning
            db.conn.lock().execute_batch("DROP INDEX IF EXISTS idx_messages_unfinished; DROP INDEX IF EXISTS idx_messages_typed; PRAGMA user_version = 0;").unwrap();
        }
        for run in 0..4 {
            let t = Instant::now();
            let db = Db::open(&path).unwrap();
            let open = t.elapsed();
            let t = Instant::now();
            let marked = db.mark_interrupted().unwrap();
            let mark = t.elapsed();
            let t = Instant::now();
            db.delete_unfinished_imports().unwrap();
            let imports = t.elapsed();
            println!("start {run}: open {open:.2?}, mark_interrupted {mark:.2?} ({marked} rows), delete_unfinished_imports {imports:.2?}");
        }
        // Ctrl+R: the popup opens, then each keystroke searches again
        let db = Db::open(&path).unwrap();
        let t = Instant::now();
        db.prompt_history("", 60).unwrap();
        let opened = t.elapsed();
        let t = Instant::now();
        for q in ["z", "zz", "zzz", "zzz-no-match"] {
            db.prompt_history(q, 60).unwrap();
        }
        println!("prompt_history: popup opens {opened:.2?}, each keystroke after it {:.2?}", t.elapsed() / 4);
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tree_and_paths() {
        let db = Db::open_in_memory().unwrap();
        let conv = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let mk = |id: &str, parent: Option<&str>, role: &str, t: i64| Message {
            id: id.into(),
            conversation_id: conv.id.clone(),
            parent_id: parent.map(|s| s.into()),
            role: role.into(),
            content: json!([{"type":"text","text":id}]),
            meta: json!({}),
            created_at: t,
        };
        db.msg_insert(&mk("u1", None, "user", 1)).unwrap();
        db.msg_insert(&mk("a1", Some("u1"), "assistant", 2)).unwrap();
        db.msg_insert(&mk("a1b", Some("u1"), "assistant", 3)).unwrap();
        db.msg_insert(&mk("u2", Some("a1b"), "user", 4)).unwrap();
        assert_eq!(db.mark_interrupted().unwrap(), 2);
        assert_eq!(db.msg_get("a1").unwrap().meta["stopped"], true);
        assert_eq!(db.mark_interrupted().unwrap(), 0);
        let p: Vec<String> = db.path_to("u2").unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(p, vec!["u1", "a1b", "u2"]);
        assert_eq!(db.tree_nodes(&conv.id).unwrap().len(), 4);
        db.fts_index(&conv.id, "u2", "hello rust world").unwrap();
        let hits = db.search("rust", 10).unwrap();
        assert_eq!(hits.len(), 1);
        let other = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        assert_eq!(db.search_messages("rust", &other.id, 5).unwrap().len(), 1);
        assert_eq!(db.search_messages("rust", &conv.id, 5).unwrap().len(), 0);
        let recent = db.recent_with_first_message(&other.id, 5).unwrap();
        assert_eq!(recent.len(), 1);
        assert!(recent[0].3.contains("u1"));
        db.conv_delete(&conv.id).unwrap();
        assert!(db.msg_get("u1").is_err());
    }

    fn artifact(conv: &str, msg: &str, content: &str) -> ArtifactRow {
        ArtifactRow { id: new_id(), conversation_id: conv.into(), message_id: msg.into(), identifier: "a".into(), kind: "text/markdown".into(), title: "t".into(), language: None, content: content.into(), created_at: 1, conversation_title: None }
    }

    #[test]
    fn an_incognito_chat_is_listed_nowhere_and_leaves_nothing_in_the_file() {
        let dir = std::env::temp_dir().join(format!("cr-incog-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let db = Db::open(&path).unwrap();
        // in the incognito chat only (a deleted chat's words stay in the search index's segments until they merge)
        let secret = "zebracorn4417";
        let word = "plainword";
        let normal = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let inc = db.conv_create("chat", "m", None, None, &json!({"incognito": true})).unwrap();
        assert!(inc.incognito && !normal.incognito && db.conv_get(&inc.id).unwrap().incognito);
        assert!(!db.conv_create("code", "m", None, Some("/p"), &json!({"incognito": true})).unwrap().incognito, "chats only");
        db.conv_set_title(&normal.id, &format!("{word} chat")).unwrap();
        db.conv_set_title(&inc.id, &format!("{word} {secret} title")).unwrap();
        for (conv, id, text) in [(&normal.id, "n1", word.to_string()), (&inc.id, "i1", format!("{word} {secret}"))] {
            let mut m = text_msg(id, conv);
            m.content = json!([{"type":"text","text": format!("{text} question")}]);
            db.msg_insert(&m).unwrap();
            db.msg_insert(&Message { id: format!("{id}a"), parent_id: Some(id.into()), role: "assistant".into(), meta: json!({"stop_reason": "end_turn", "usage": {"output_tokens": 3}}), created_at: 2, ..m.clone() }).unwrap();
            db.conv_set_leaf(conv, Some(&format!("{id}a"))).unwrap();
            db.fts_index(conv, id, &format!("{text} question")).unwrap();
            db.artifact_upsert(&artifact(conv, &format!("{id}a"), &format!("{text} doc"))).unwrap();
        }
        // the options can't lose the flag (a fork copies them)
        db.conv_set_options(&inc.id, &json!({"style": "x"})).unwrap();
        assert_eq!(db.conv_get(&inc.id).unwrap().options["incognito"], true);
        let only_normal = |ids: Vec<String>| assert_eq!(ids, vec![normal.id.clone()]);
        only_normal(db.conv_list(None, None).unwrap().into_iter().filter(|c| c.kind == "chat").map(|c| c.id).collect());
        only_normal(db.search(word, 10).unwrap().into_iter().map(|h| h.conversation_id).collect());
        assert!(db.search(secret, 10).unwrap().is_empty());
        only_normal(db.search("", 10).unwrap().into_iter().filter(|h| h.kind == "chat").map(|h| h.conversation_id).collect());
        only_normal(db.search_in(word, 10, true).unwrap().into_iter().map(|h| h.conversation_id).collect());
        only_normal(db.search_messages(word, "", 10).unwrap().into_iter().map(|r| r.0).collect());
        only_normal(db.recent_with_first_message("", 10).unwrap().into_iter().map(|r| r.0).collect());
        only_normal(db.assistant_usage_since(0).unwrap().into_iter().map(|r| r.1).collect());
        only_normal(db.artifact_list(10).unwrap().into_iter().map(|a| a.conversation_id).collect());
        only_normal(db.memory_sources(None, true, &[], 0, 10).unwrap().into_iter().map(|c| c.id).collect());
        assert_eq!(db.prompt_history(word, 10).unwrap().len(), 1);
        assert_eq!(fts_rows(&db, "i1"), Vec::<String>::new(), "never indexed");
        assert_eq!(db.incognito_ids().unwrap(), vec![inc.id.clone()]);
        let on_disk = || std::fs::read_dir(&dir).unwrap().flatten().any(|f| std::fs::read(f.path()).unwrap().windows(secret.len()).any(|w| w == secret.as_bytes()));
        assert!(on_disk(), "written while it runs");
        db.conv_delete(&inc.id).unwrap();
        db.conv_delete(&normal.id).unwrap();
        let count = |sql: &str| db.conn.lock().query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT count(*) FROM messages") + count("SELECT count(*) FROM artifacts") + count("SELECT count(*) FROM messages_fts"), 0);
        assert!(db.incognito_ids().unwrap().is_empty());
        // nor in the file's free pages or its write-ahead log
        assert!(!on_disk(), "the open database keeps the text");
        drop(db);
        assert!(!on_disk());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pins_archive_unread_and_sections() {
        let db = Db::open_in_memory().unwrap();
        let a = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let b = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
        let code = db.conv_create("code", "m", None, Some("/p"), &json!({})).unwrap();
        db.conv_set_title(&b.id, "archived plan").unwrap();
        db.conv_set_pinned(&a.id, true).unwrap();
        db.conv_set_unread(&a.id, true).unwrap();
        let got = db.conv_get(&a.id).unwrap();
        assert!(got.pinned_at.is_some() && got.unread);
        db.conv_set_pinned(&a.id, false).unwrap();
        db.conv_set_unread(&a.id, false).unwrap();
        let got = db.conv_get(&a.id).unwrap();
        assert!(got.pinned_at.is_none() && !got.unread);
        // archived: out of the list and the search, unless asked
        db.conv_set_archived(&b.id, true).unwrap();
        assert!(db.conv_list(Some("chat"), None).unwrap().iter().all(|c| c.id != b.id));
        assert_eq!(db.conv_list_archived(Some("chat")).unwrap().into_iter().map(|c| c.id).collect::<Vec<_>>(), vec![b.id.clone()]);
        assert!(db.search("archived", 10).unwrap().is_empty());
        assert_eq!(db.search_in("archived", 10, true).unwrap().len(), 1);
        assert!(db.conv_cwds().unwrap() == vec!["/p".to_string()]);
        db.conv_set_archived(&b.id, false).unwrap();
        assert_eq!(db.search("archived", 10).unwrap().len(), 1);
        // sections: one kind's conversations each, and deleting one keeps them
        let s = db.section_create("chat", "Work").unwrap();
        let g = db.section_create("code", "Repos").unwrap();
        db.conv_set_section(&a.id, Some(&s.id)).unwrap();
        assert!(db.conv_set_section(&code.id, Some(&s.id)).is_err(), "a chat section takes no code session");
        assert!(db.conv_set_section(&a.id, Some("gone")).is_err());
        assert!(db.conv_set_section("gone", None).is_err());
        db.conv_set_section(&code.id, Some(&g.id)).unwrap();
        db.section_rename(&s.id, "Home").unwrap();
        assert!(db.section_rename("gone", "x").is_err());
        assert_eq!(db.section_list("chat").unwrap().into_iter().map(|s| s.name).collect::<Vec<_>>(), vec!["Home"]);
        assert_eq!(db.conv_get(&a.id).unwrap().section_id.as_deref(), Some(s.id.as_str()));
        db.section_delete(&s.id).unwrap();
        let a2 = db.conv_get(&a.id).unwrap();
        assert!(a2.section_id.is_none(), "the chat stays, out of the section");
        assert!(db.section_list("chat").unwrap().is_empty());
        assert_eq!(db.conv_get(&code.id).unwrap().section_id.as_deref(), Some(g.id.as_str()));
        db.conv_set_section(&code.id, None).unwrap();
        assert!(db.conv_get(&code.id).unwrap().section_id.is_none());
    }

    #[test]
    fn memory_sources_and_a_synthesis_that_lost_a_race() {
        let db = Db::open_in_memory().unwrap();
        let p = db.project_create("P", "").unwrap();
        let q = db.project_create("Q", "").unwrap();
        let mk = |project: Option<&str>, options: Value| {
            let c = db.conv_create("chat", "m", project, None, &options).unwrap();
            db.msg_insert(&text_msg(&new_id(), &c.id)).unwrap();
            db.conv_set_leaf(&c.id, Some("x")).unwrap();
            c.id
        };
        let free = mk(None, json!({}));
        let in_p = mk(Some(&p.id), json!({}));
        let in_q = mk(Some(&q.id), json!({}));
        mk(None, json!({"incognito": true}));
        db.conv_create("chat", "m", None, None, &json!({})).unwrap(); // never answered
        let ids = |v: Vec<Conversation>| { let mut v: Vec<String> = v.into_iter().map(|c| c.id).collect(); v.sort(); v };
        let sorted = |mut v: Vec<String>| { v.sort(); v };
        assert_eq!(ids(db.memory_sources(None, false, &[], 0, 10).unwrap()), vec![free.clone()]);
        assert_eq!(ids(db.memory_sources(Some(&p.id), false, &[], 0, 10).unwrap()), vec![in_p.clone()]);
        assert_eq!(ids(db.memory_sources(None, true, std::slice::from_ref(&q.id), 0, 10).unwrap()), sorted(vec![free.clone(), in_p.clone()]));
        assert_eq!(ids(db.memory_sources(None, true, &[], 0, 10).unwrap()), sorted(vec![free.clone(), in_p, in_q]));
        assert_eq!(db.memory_sources(None, true, &[], 0, 2).unwrap().len(), 2);
        assert!(db.memory_sources(None, true, &[], now_ms() + 1000, 10).unwrap().is_empty(), "only those updated since");
        // a synthesis stores only over the memory it read
        assert!(db.memory_store_synthesis("global", "one", std::slice::from_ref(&free), None).unwrap());
        assert!(!db.memory_store_synthesis("global", "two", &[], None).unwrap(), "made meanwhile");
        let read = db.memory_get("global").unwrap().unwrap();
        assert_eq!((read.text.as_str(), read.sources.clone(), read.synthesized_at), ("one", vec![free], read.updated_at));
        std::thread::sleep(std::time::Duration::from_millis(5));
        db.memory_set("global", "edited").unwrap();
        assert!(!db.memory_store_synthesis("global", "three", &[], Some(read.updated_at)).unwrap(), "edited meanwhile");
        let now = db.memory_get("global").unwrap().unwrap();
        assert_eq!((now.text.as_str(), now.synthesized_at), ("edited", read.synthesized_at), "an edit keeps the sources and the synthesis time");
        assert!(db.memory_store_synthesis("global", "four", &[], Some(now.updated_at)).unwrap());
        // a project's memory goes with it
        db.memory_set(&p.id, "p notes").unwrap();
        db.project_delete(&p.id).unwrap();
        assert!(db.memory_get(&p.id).unwrap().is_none());
        db.memory_delete("global").unwrap();
        assert!(db.memory_list().unwrap().is_empty());
    }

    #[test]
    fn search_matches_every_word_in_any_order_turkish_aware() {
        assert_eq!(fold("İSTANBUL Işık ışık Çiçek ŞEKER Ünlü"), "istanbul isik isik cicek seker unlu");
        assert_eq!(fts_query("a\"b c"), "\"a\"\"b\"* AND \"c\"*");
        assert_eq!(fts_term("Işk"), "(\"işk\"* OR \"ışk\"*)");
        assert_eq!(fts_term("iiiiii").matches(" OR ").count(), 15, "the first four i's vary");
        let db = Db::open_in_memory().unwrap();
        let conv = |title: &str| {
            let c = db.conv_create("chat", "m", None, None, &json!({})).unwrap();
            db.conv_set_title(&c.id, title).unwrap();
            c.id
        };
        let filler = conv("Sidebar filler chat number 1 about tests");
        let city = conv("İstanbul gezisi");
        let light = conv("Işık ayarları");
        let body = conv("Bahçe");
        db.msg_insert(&text_msg("b1", &body)).unwrap();
        db.fts_index(&body, "b1", "Çiçekler ışıkta büyür, yapraklar güneşe döner.").unwrap();
        let ids = |q: &str| db.search(q, 10).unwrap().into_iter().map(|h| h.conversation_id).collect::<Vec<_>>();
        assert_eq!(ids("filler 1"), vec![filler.clone()]);
        assert_eq!(ids("1 FILLER number"), vec![filler.clone()], "any order, any case");
        assert!(ids("filler 2").is_empty(), "every word must match");
        assert_eq!(ids("istanbul"), vec![city]);
        // the title first, then the message that says "ışıkta"
        assert_eq!(ids("ISIK"), vec![light.clone(), body.clone()]);
        assert_eq!(ids("ışık ayar"), vec![light]);
        // message text: accents and i/ı folded, words in any order, a highlighted snippet
        let hits = db.search("isikta cicek", 10).unwrap();
        assert_eq!(hits.iter().map(|h| h.conversation_id.clone()).collect::<Vec<_>>(), vec![body.clone()]);
        assert!(hits[0].snippet.contains('\u{1}') && hits[0].snippet.contains('\u{2}'));
        assert_eq!(ids("GÜNEŞE yaprak"), vec![body]);
        assert!(ids("cicek gezisi").is_empty(), "the words of one title or one message");
    }

    #[test]
    fn artifact_storage_is_kept_per_artifact_within_its_budget() {
        let db = Db::open_in_memory().unwrap();
        let c = db.conv_create("chat", "m", None, None, &json!({})).unwrap().id;
        let (a, b) = (format!("{c}/a"), format!("{c}/b"));
        assert!(db.storage_set(&c, &a, false, "k", "mine", 100).unwrap());
        assert!(db.storage_set(&c, &a, true, "k", "ours", 100).unwrap());
        assert!(db.storage_set(&c, &a, false, "k2", "x", 100).unwrap());
        assert_eq!(db.storage_get(&a, false, "k").unwrap().as_deref(), Some("mine"));
        assert_eq!(db.storage_get(&a, true, "k").unwrap().as_deref(), Some("ours"));
        assert_eq!(db.storage_get(&b, false, "k").unwrap(), None);
        assert_eq!(db.storage_keys(&a, false, "").unwrap(), ["k", "k2"]);
        assert_eq!(db.storage_keys(&a, false, "k2").unwrap(), ["k2"]);
        // the budget counts the artifact's own and shared keys and values, a replaced value once
        assert!(!db.storage_set(&c, &a, false, "big", &"v".repeat(90), 100).unwrap());
        assert_eq!(db.storage_get(&a, false, "big").unwrap(), None);
        assert!(db.storage_set(&c, &a, false, "k", &"v".repeat(80), 100).unwrap());
        // another artifact has a budget of its own
        assert!(db.storage_set(&c, &b, false, "big", &"v".repeat(90), 100).unwrap());
        assert!(db.storage_delete(&a, false, "k2").unwrap());
        assert!(!db.storage_delete(&a, false, "k2").unwrap());
        // gone with the conversation
        db.conv_delete(&c).unwrap();
        assert_eq!(db.storage_get(&a, true, "k").unwrap(), None);
        assert_eq!(db.conn.lock().query_row("SELECT COUNT(*) FROM artifact_storage", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }

    #[test]
    fn artifact_storage_moved_from_an_older_version_skips_what_is_gone_or_already_here() {
        let db = Db::open_in_memory().unwrap();
        let c = db.conv_create("chat", "m", None, None, &json!({})).unwrap().id;
        let scope = format!("{c}/a");
        db.storage_set(&c, &scope, false, "k", "newer", 1000).unwrap();
        let row = |conv: &str, key: &str, value: &str| (conv.to_string(), format!("{conv}/a"), false, key.to_string(), value.to_string());
        let added = db.storage_import(&[row(&c, "k", "older"), row(&c, "k2", "v"), row("deleted-chat", "k", "v")]).unwrap();
        assert_eq!(added, 1);
        assert_eq!(db.storage_get(&scope, false, "k").unwrap().as_deref(), Some("newer"));
        assert_eq!(db.storage_get(&scope, false, "k2").unwrap().as_deref(), Some("v"));
    }
}
