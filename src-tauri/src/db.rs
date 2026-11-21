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

pub struct Db {
    conn: Mutex<Connection>,
    fts: bool,
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
    })
}

fn msg_from_row(r: &Row) -> rusqlite::Result<Message> {
    let content: String = r.get("content")?;
    let meta: String = r.get("meta")?;
    Ok(Message {
        id: r.get("id")?,
        conversation_id: r.get("conversation_id")?,
        parent_id: r.get("parent_id")?,
        role: r.get("role")?,
        content: serde_json::from_str(&content).unwrap_or(Value::Array(vec![])),
        meta: serde_json::from_str(&meta).unwrap_or(Value::Object(Default::default())),
        created_at: r.get("created_at")?,
    })
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

/// Escape a user query for FTS5: every token becomes a quoted prefix term.
fn fts_query(q: &str) -> String {
    q.split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Db> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> AppResult<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> AppResult<Db> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;",
        )?;
        conn.execute_batch(SCHEMA)?;
        let fts = conn
            .execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text, conversation_id UNINDEXED, message_id UNINDEXED);",
            )
            .is_ok();
        Ok(Db {
            conn: Mutex::new(conn),
            fts,
        })
    }

    // ---------------------------------------------------------------- settings

    pub fn setting_get(&self, key: &str) -> AppResult<Option<String>> {
        let c = self.conn.lock();
        Ok(c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0))
            .optional()?)
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
        };
        self.conn.lock().execute(
            "INSERT INTO conversations(id,kind,title,model,project_id,cwd,starred,current_leaf,options,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,0,NULL,?7,?8,?8)",
            params![conv.id, conv.kind, conv.title, conv.model, conv.project_id, conv.cwd, options.to_string(), now],
        )?;
        Ok(conv)
    }

    pub fn conv_get(&self, id: &str) -> AppResult<Conversation> {
        let c = self.conn.lock();
        c.query_row("SELECT * FROM conversations WHERE id=?1", [id], conv_from_row)
            .optional()?
            .ok_or_else(|| AppError::msg("Conversation not found"))
    }

    pub fn conv_list(&self, kind: Option<&str>, project_id: Option<&str>) -> AppResult<Vec<Conversation>> {
        let c = self.conn.lock();
        let mut sql = String::from("SELECT * FROM conversations WHERE 1=1");
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
        self.conn.lock().execute(
            "UPDATE conversations SET current_leaf=?2, updated_at=?3 WHERE id=?1",
            params![id, leaf, now_ms()],
        )?;
        Ok(())
    }

    pub fn conv_set_options(&self, id: &str, options: &Value) -> AppResult<()> {
        self.conn.lock().execute(
            "UPDATE conversations SET options=?2 WHERE id=?1",
            params![id, options.to_string()],
        )?;
        Ok(())
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
        let c = self.conn.lock();
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
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT m.created_at, m.conversation_id, c.title, c.kind, c.model, m.meta FROM messages m JOIN conversations c ON c.id = m.conversation_id
             WHERE m.role = 'assistant' AND m.created_at >= ?1 AND json_extract(m.meta, '$.forked_from') IS NULL",
        )?;
        let rows = st.query_map([since_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
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
        }
        c.execute("DELETE FROM conversations WHERE id=?1", [id])?;
        Ok(())
    }

    // ---------------------------------------------------------------- messages

    pub fn msg_insert(&self, m: &Message) -> AppResult<()> {
        self.conn.lock().execute(
            "INSERT INTO messages(id,conversation_id,parent_id,role,content,meta,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                m.id,
                m.conversation_id,
                m.parent_id,
                m.role,
                m.content.to_string(),
                m.meta.to_string(),
                m.created_at
            ],
        )?;
        Ok(())
    }

    pub fn msg_update(&self, id: &str, content: &Value, meta: &Value) -> AppResult<()> {
        self.conn.lock().execute(
            "UPDATE messages SET content=?2, meta=?3 WHERE id=?1",
            params![id, content.to_string(), meta.to_string()],
        )?;
        Ok(())
    }

    pub fn msg_get(&self, id: &str) -> AppResult<Message> {
        let c = self.conn.lock();
        c.query_row("SELECT * FROM messages WHERE id=?1", [id], msg_from_row)
            .optional()?
            .ok_or_else(|| AppError::msg("Message not found"))
    }

    /// Lightweight info about every node of a conversation tree.
    pub fn tree_nodes(&self, conv_id: &str) -> AppResult<Vec<NodeInfo>> {
        let c = self.conn.lock();
        let mut st = c.prepare(
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
        let c = self.conn.lock();
        let mut st = c.prepare(
            "WITH RECURSIVE p(id, parent_id, depth) AS (
                SELECT id, parent_id, 0 FROM messages WHERE id=?1
                UNION ALL
                SELECT m.id, m.parent_id, p.depth+1 FROM messages m JOIN p ON m.id = p.parent_id
             )
             SELECT messages.* FROM p JOIN messages ON messages.id = p.id ORDER BY p.depth DESC",
        )?;
        let rows = st.query_map([leaf], msg_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
        Ok(c.execute(
            "UPDATE messages SET meta = json_set(meta, '$.stopped', json('true'))
             WHERE role = 'assistant'
               AND json_extract(meta, '$.stop_reason') IS NULL
               AND json_extract(meta, '$.error') IS NULL
               AND json_extract(meta, '$.stopped') IS NULL",
            [],
        )?)
    }

    pub fn fts_index(&self, conv_id: &str, msg_id: &str, text: &str) -> AppResult<()> {
        if !self.fts {
            return Ok(());
        }
        let c = self.conn.lock();
        c.execute("DELETE FROM messages_fts WHERE message_id=?1", [msg_id])?;
        if !text.trim().is_empty() {
            c.execute(
                "INSERT INTO messages_fts(text,conversation_id,message_id) VALUES(?1,?2,?3)",
                params![text, conv_id, msg_id],
            )?;
        }
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> AppResult<Vec<SearchHit>> {
        let q = query.trim();
        let c = self.conn.lock();
        let mut hits: Vec<SearchHit> = vec![];
        if q.is_empty() {
            let mut st = c.prepare(
                "SELECT id,title,kind,updated_at FROM conversations ORDER BY updated_at DESC LIMIT ?1",
            )?;
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
        // Title matches first.
        let like = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
        {
            let mut st = c.prepare(
                "SELECT id,title,kind,updated_at FROM conversations WHERE title LIKE ?1 ESCAPE '\\' ORDER BY updated_at DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![like, limit as i64], |r| {
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
                hits.push(h);
            }
        }
        if self.fts {
            let mut st = c.prepare(
                "SELECT f.conversation_id, f.message_id, c.title, snippet(messages_fts, 0, '\u{1}', '\u{2}', '…', 12), c.kind, c.updated_at
                 FROM messages_fts f JOIN conversations c ON c.id = f.conversation_id
                 WHERE messages_fts MATCH ?1 ORDER BY rank LIMIT ?2",
            )?;
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
            let mut st = c.prepare(
                "SELECT m.conversation_id, m.id, c.title, substr(m.content, 1, 200), c.kind, c.updated_at
                 FROM messages m JOIN conversations c ON c.id = m.conversation_id
                 WHERE m.content LIKE ?1 ESCAPE '\\' ORDER BY c.updated_at DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![like, limit as i64], |r| {
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
        let c = self.conn.lock();
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
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT c.id, c.title, c.updated_at,
                    (SELECT m.content FROM messages m WHERE m.conversation_id = c.id AND m.role = 'user'
                     ORDER BY m.created_at ASC LIMIT 1)
             FROM conversations c WHERE c.id != ?1 AND c.kind = 'chat' ORDER BY c.updated_at DESC LIMIT ?2",
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
        self.conn.lock().execute("DELETE FROM projects WHERE id=?1", [id])?;
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
        let c = self.conn.lock();
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
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT a.*, c.title AS conversation_title FROM artifacts a
             JOIN conversations c ON c.id = a.conversation_id
             WHERE a.created_at = (SELECT MAX(b.created_at) FROM artifacts b WHERE b.conversation_id=a.conversation_id AND b.identifier=a.identifier)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
