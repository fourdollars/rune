//! SQLite persistence for chat history.
//!
//! Schema:
//!   messages(id, note_id, role, nickname, content, created_at)
//!
//! All blocking SQLite calls are wrapped in tokio::task::spawn_blocking.

use rusqlite::{backup::Backup, params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::warn;

/// A single stored chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRecord {
    pub id: i64,
    pub note_id: String,
    #[serde(default = "default_session_id")]
    pub session_id: String,
    pub role: String,     // "user" | "assistant" | "system"
    pub nickname: String, // user nickname, or "ᚱᚢᚾᛖ" for assistant
    pub content: String,
    pub created_at: i64, // unix timestamp (seconds)
    /// Model name used for this response (assistant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Prompt tokens consumed (assistant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<i32>,
    /// Completion tokens generated (assistant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<i32>,
    /// Number of agent steps (assistant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<i32>,
    /// Number of tool calls made (assistant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<i32>,
    /// Thinking/reasoning level used (assistant only, None = off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Total context tokens at the time of this response (assistant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<i32>,
    /// Execution duration in milliseconds (assistant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Archive filename if this record was found in archives (search only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_file: Option<String>,
}

fn default_session_id() -> String {
    "main".to_string()
}

/// A stored session entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteRecord {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    pub public: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// A stored scheduled cron / interval job entry for a note.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteCronJobRecord {
    pub id: String,
    pub note_id: String,
    pub name: String,
    pub schedule_type: String,  // "cron" | "interval"
    pub schedule_value: String, // e.g. "*/30 * * * *" or "30m"
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub silent_if_no_action: bool,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

/// Metadata and overrides for a chat session within a note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionMeta {
    pub note_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default)]
    pub message_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<i64>,
}

/// Thread-safe SQLite connection wrapper.
#[derive(Clone)]
pub struct ChatDb {
    conn: Arc<Mutex<Connection>>,
    /// If Some, DB is in-memory and should be persisted to this path on first write.
    deferred_path: Arc<Mutex<Option<std::path::PathBuf>>>,
}

fn init_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=NORMAL;
        CREATE TABLE IF NOT EXISTS messages (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id        TEXT    NOT NULL DEFAULT 'default',
            session_id     TEXT    NOT NULL DEFAULT 'main',
            role           TEXT    NOT NULL,
            nickname       TEXT    NOT NULL,
            content        TEXT    NOT NULL,
            created_at     INTEGER NOT NULL,
            model          TEXT,
            tokens_in      INTEGER,
            tokens_out     INTEGER,
            steps          INTEGER,
            tool_calls     INTEGER,
            thinking       TEXT,
            context_tokens INTEGER,
            duration_ms    INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_messages_session
            ON messages(note_id, id);
        CREATE TABLE IF NOT EXISTS sessions (
            id             TEXT PRIMARY KEY,
            name           TEXT NOT NULL,
            created_at     INTEGER NOT NULL,
            created_by     TEXT,
            public         INTEGER DEFAULT 0,
            model_override TEXT,
            icon           TEXT
        );
        CREATE TABLE IF NOT EXISTS user_sessions (
            id         TEXT PRIMARY KEY,
            login      TEXT NOT NULL,
            role       TEXT NOT NULL,
            avatar_url TEXT NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS oauth_tokens (
            token      TEXT PRIMARY KEY,
            role       TEXT NOT NULL,
            login      TEXT NOT NULL DEFAULT '',
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS file_visibility (
            note_id  TEXT NOT NULL,
            filename TEXT NOT NULL,
            public   INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (note_id, filename)
        );
        CREATE TABLE IF NOT EXISTS note_settings (
            note_id  TEXT PRIMARY KEY,
            thinking TEXT
        );
        CREATE TABLE IF NOT EXISTS note_cron_jobs (
            id                  TEXT PRIMARY KEY,
            note_id             TEXT NOT NULL,
            name                TEXT NOT NULL,
            schedule_type       TEXT NOT NULL,
            schedule_value      TEXT NOT NULL,
            prompt              TEXT NOT NULL,
            model               TEXT,
            silent_if_no_action INTEGER NOT NULL DEFAULT 1,
            enabled             INTEGER NOT NULL DEFAULT 1,
            last_run_at         TEXT,
            last_status         TEXT,
            created_at          TEXT NOT NULL,
            updated_at          TEXT NOT NULL,
            timeout_secs        INTEGER DEFAULT 60,
            thinking            TEXT
        );
        CREATE TABLE IF NOT EXISTS chat_sessions (
            note_id      TEXT NOT NULL,
            session_id   TEXT NOT NULL,
            title        TEXT,
            custom_title TEXT,
            model        TEXT,
            thinking     TEXT,
            created_at   INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at   INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            PRIMARY KEY (note_id, session_id)
        );
        CREATE INDEX IF NOT EXISTS idx_chat_sessions_note
            ON chat_sessions(note_id);
    ",
    )?;
    // Add new columns to existing DBs (idempotent — each statement executed individually so one failure does not abort the rest)
    let migrations = [
        "ALTER TABLE messages ADD COLUMN session_id TEXT NOT NULL DEFAULT 'main'",
        "ALTER TABLE messages ADD COLUMN model TEXT",
        "ALTER TABLE messages ADD COLUMN tokens_in INTEGER",
        "ALTER TABLE messages ADD COLUMN tokens_out INTEGER",
        "ALTER TABLE messages ADD COLUMN steps INTEGER",
        "ALTER TABLE messages ADD COLUMN tool_calls INTEGER",
        "ALTER TABLE messages ADD COLUMN thinking TEXT",
        "ALTER TABLE messages ADD COLUMN context_tokens INTEGER",
        "ALTER TABLE messages ADD COLUMN duration_ms INTEGER",
        "ALTER TABLE sessions ADD COLUMN public INTEGER DEFAULT 0",
        "ALTER TABLE sessions ADD COLUMN model_override TEXT",
        "ALTER TABLE sessions ADD COLUMN icon TEXT",
        "ALTER TABLE oauth_tokens ADD COLUMN login TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE note_cron_jobs ADD COLUMN timeout_secs INTEGER DEFAULT 60",
        "ALTER TABLE note_cron_jobs ADD COLUMN thinking TEXT",
        "ALTER TABLE chat_sessions ADD COLUMN title TEXT",
        "ALTER TABLE chat_sessions ADD COLUMN custom_title TEXT",
    ];
    for migration in migrations {
        let _ = conn.execute(migration, []);
    }
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_messages_note_session ON messages(note_id, session_id, id)",
        [],
    );
    Ok(())
}

impl ChatDb {
    /// Open (or create) the chat database at the given path.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            deferred_path: Arc::new(Mutex::new(None)),
        })
    }

    /// Open file if it exists, otherwise in-memory with deferred persistence.
    pub fn open_lazy(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Self::open(path)
        } else {
            let conn = Connection::open_in_memory()?;
            init_schema(&conn)?;
            Ok(ChatDb {
                conn: Arc::new(Mutex::new(conn)),
                deferred_path: Arc::new(Mutex::new(Some(path.to_path_buf()))),
            })
        }
    }

    /// If DB is in-memory with a deferred path, persist it to disk now.
    /// Uses SQLite backup API to copy memory → file, then reopens from file.
    /// No-op if already file-based.
    pub fn ensure_persistent(&self) -> anyhow::Result<()> {
        let path = {
            let mut dp = self.deferred_path.lock().unwrap();
            match dp.take() {
                Some(p) => p,
                None => return Ok(()), // already persistent
            }
        };
        // Create parent dirs
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Use SQLite backup API to copy in-memory → file
        let conn = self.conn.lock().unwrap();
        let mut backup_conn = Connection::open(&path)?;
        backup_conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
        ",
        )?;
        let backup = Backup::new(&*conn, &mut backup_conn)?;
        backup.run_to_completion(100, std::time::Duration::from_millis(10), None)?;
        drop(backup);
        // Checkpoint WAL to ensure data is visible to new connections
        backup_conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        drop(conn);
        // Use the backup connection directly (already has all data)
        let mut conn_guard = self.conn.lock().unwrap();
        *conn_guard = backup_conn;
        Ok(())
    }

    /// Returns true if currently in-memory (not yet persisted to file).
    pub fn is_memory(&self) -> bool {
        self.deferred_path.lock().unwrap().is_some()
    }

    /// Insert a message for a specific thread/session. Returns the new row id.
    pub fn insert_session(
        &self,
        note_id: &str,
        session_id: &str,
        role: &str,
        nickname: &str,
        content: &str,
    ) -> anyhow::Result<i64> {
        self.insert_session_with_meta(
            note_id, session_id, role, nickname, content, None, None, None, None, None, None, None,
            None,
        )
    }

    /// Insert a message with optional model/token/duration metadata into a specific session.
    pub fn insert_session_with_meta(
        &self,
        note_id: &str,
        session_id: &str,
        role: &str,
        nickname: &str,
        content: &str,
        model: Option<&str>,
        tokens_in: Option<i32>,
        tokens_out: Option<i32>,
        steps: Option<i32>,
        tool_calls: Option<i32>,
        thinking: Option<&str>,
        context_tokens: Option<i32>,
        duration_ms: Option<u64>,
    ) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let s_id = if session_id.trim().is_empty() {
            "main"
        } else {
            session_id
        };
        conn.execute(
            "INSERT INTO messages (note_id, session_id, role, nickname, content, created_at, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens, duration_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                note_id,
                s_id,
                role,
                nickname,
                content,
                ts,
                model,
                tokens_in,
                tokens_out,
                steps,
                tool_calls,
                thinking,
                context_tokens,
                duration_ms.map(|d| d as i64)
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a message into default ("main") session. Returns the new row id.
    pub fn insert(
        &self,
        note_id: &str,
        role: &str,
        nickname: &str,
        content: &str,
    ) -> anyhow::Result<i64> {
        self.insert_session(note_id, "main", role, nickname, content)
    }

    /// Insert a message with optional model/token/duration metadata into default ("main") session.
    pub fn insert_with_meta(
        &self,
        note_id: &str,
        role: &str,
        nickname: &str,
        content: &str,
        model: Option<&str>,
        tokens_in: Option<i32>,
        tokens_out: Option<i32>,
        steps: Option<i32>,
        tool_calls: Option<i32>,
        thinking: Option<&str>,
        context_tokens: Option<i32>,
        duration_ms: Option<u64>,
    ) -> anyhow::Result<i64> {
        self.insert_session_with_meta(
            note_id,
            "main",
            role,
            nickname,
            content,
            model,
            tokens_in,
            tokens_out,
            steps,
            tool_calls,
            thinking,
            context_tokens,
            duration_ms,
        )
    }

    /// Load the last `limit` messages for a note's session (ordered oldest first).
    pub fn load_recent_session(
        &self,
        note_id: &str,
        session_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<ChatRecord>> {
        let conn = self.conn.lock().unwrap();
        let s_id = if session_id.trim().is_empty() {
            "main"
        } else {
            session_id
        };
        let mut stmt = conn.prepare(
            "SELECT id, note_id, session_id, role, nickname, content, created_at, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens, duration_ms
             FROM messages
             WHERE note_id = ?1 AND session_id = ?2
             ORDER BY id DESC
             LIMIT ?3",
        )?;
        let rows: Vec<ChatRecord> = stmt
            .query_map(params![note_id, s_id, limit as i64], |row| {
                Ok(ChatRecord {
                    id: row.get(0)?,
                    note_id: row.get(1)?,
                    session_id: row.get(2)?,
                    role: row.get(3)?,
                    nickname: row.get(4)?,
                    content: row.get(5)?,
                    created_at: row.get(6)?,
                    model: row.get(7)?,
                    tokens_in: row.get(8)?,
                    tokens_out: row.get(9)?,
                    steps: row.get(10)?,
                    tool_calls: row.get(11)?,
                    thinking: row.get(12).ok().flatten(),
                    context_tokens: row.get(13).ok().flatten(),
                    duration_ms: row
                        .get::<_, Option<i64>>(14)
                        .ok()
                        .flatten()
                        .map(|v| v as u64),
                    archive_file: None,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        // Reverse so oldest first
        let mut rows = rows;
        rows.reverse();
        Ok(rows)
    }

    /// Load the last `limit` messages for a session (ordered oldest first).
    pub fn load_recent(&self, note_id: &str, limit: usize) -> anyhow::Result<Vec<ChatRecord>> {
        self.load_recent_session(note_id, "main", limit)
    }

    /// List all distinct session IDs for a note.
    pub fn list_sessions_for_note(&self, note_id: &str) -> anyhow::Result<Vec<String>> {
        let meta = self.list_chat_sessions_meta(note_id)?;
        Ok(meta.into_iter().map(|s| s.session_id).collect())
    }

    /// Get model and thinking override for a specific session.
    pub fn get_session_meta(
        &self,
        note_id: &str,
        session_id: &str,
    ) -> Option<(Option<String>, Option<String>)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT model, thinking FROM chat_sessions WHERE note_id = ?1 AND session_id = ?2",
            )
            .ok()?;
        stmt.query_row(params![note_id, session_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .ok()
    }

    /// Set model and thinking override for a specific session.
    pub fn set_session_meta(
        &self,
        note_id: &str,
        session_id: &str,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO chat_sessions (note_id, session_id, model, thinking, updated_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%s', 'now'))
             ON CONFLICT(note_id, session_id) DO UPDATE SET
                 model = COALESCE(?3, chat_sessions.model),
                 thinking = COALESCE(?4, chat_sessions.thinking),
                 updated_at = strftime('%s', 'now')",
            params![note_id, session_id, model, thinking],
        )?;
        Ok(())
    }

    /// Ensure a session exists in chat_sessions table.
    pub fn ensure_session(&self, note_id: &str, session_id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let s_id = if session_id.trim().is_empty() {
            "main"
        } else {
            session_id.trim()
        };
        conn.execute(
            "INSERT INTO chat_sessions (note_id, session_id, updated_at)
             VALUES (?1, ?2, strftime('%s', 'now'))
             ON CONFLICT(note_id, session_id) DO NOTHING",
            params![note_id, s_id],
        )?;
        Ok(())
    }

    /// Set or update the auto-discovered title (e.g. from LINE API) for a session.
    pub fn set_session_title(
        &self,
        note_id: &str,
        session_id: &str,
        title: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let s_id = if session_id.trim().is_empty() {
            "main"
        } else {
            session_id.trim()
        };
        if let Some(t) = title {
            if !t.trim().is_empty() {
                conn.execute(
                    "INSERT INTO chat_sessions (note_id, session_id, title, updated_at)
                     VALUES (?1, ?2, ?3, strftime('%s', 'now'))
                     ON CONFLICT(note_id, session_id) DO UPDATE SET
                         title = ?3,
                         updated_at = strftime('%s', 'now')",
                    params![note_id, s_id, t.trim()],
                )?;
                return Ok(());
            }
        }
        conn.execute(
            "INSERT INTO chat_sessions (note_id, session_id, updated_at)
             VALUES (?1, ?2, strftime('%s', 'now'))
             ON CONFLICT(note_id, session_id) DO NOTHING",
            params![note_id, s_id],
        )?;
        Ok(())
    }

    /// Set or update the user-customized title for a session.
    /// If `custom_title` is None or empty, it clears the custom title so it falls back to `title`.
    pub fn set_session_custom_title(
        &self,
        note_id: &str,
        session_id: &str,
        custom_title: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let clean = custom_title.map(str::trim).filter(|s| !s.is_empty());
        conn.execute(
            "INSERT INTO chat_sessions (note_id, session_id, custom_title, updated_at)
             VALUES (?1, ?2, ?3, strftime('%s', 'now'))
             ON CONFLICT(note_id, session_id) DO UPDATE SET
                 custom_title = ?3,
                 updated_at = strftime('%s', 'now')",
            params![note_id, session_id, clean],
        )?;
        Ok(())
    }

    /// List all chat sessions with their metadata for a note.
    pub fn list_chat_sessions_meta(&self, note_id: &str) -> anyhow::Result<Vec<ChatSessionMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.session_id,
                    cs.title,
                    cs.custom_title,
                    cs.model,
                    cs.thinking,
                    COUNT(m.id) as msg_count,
                    MAX(m.created_at) as last_act
             FROM (
                 SELECT session_id FROM messages WHERE note_id = ?1
                 UNION
                 SELECT session_id FROM chat_sessions WHERE note_id = ?1
                 UNION
                 SELECT 'main' AS session_id
             ) s
             LEFT JOIN chat_sessions cs ON cs.note_id = ?1 AND cs.session_id = s.session_id
             LEFT JOIN messages m ON m.note_id = ?1 AND m.session_id = s.session_id
             GROUP BY s.session_id
             ORDER BY CASE WHEN s.session_id = 'main' THEN 0 ELSE 1 END, s.session_id ASC",
        )?;
        let rows = stmt
            .query_map(params![note_id], |row| {
                let session_id: String = row.get(0)?;
                let title: Option<String> = row.get(1)?;
                let custom_title: Option<String> = row.get(2)?;
                let model: Option<String> = row.get(3)?;
                let thinking: Option<String> = row.get(4)?;
                let message_count: usize = row.get(5)?;
                let last_activity: Option<i64> = row.get(6)?;
                Ok(ChatSessionMeta {
                    note_id: note_id.to_string(),
                    session_id,
                    title,
                    custom_title,
                    model,
                    thinking,
                    message_count,
                    last_activity,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Async wrapper for insert_session.
    pub async fn insert_session_async(
        &self,
        note_id: String,
        session_id: String,
        role: String,
        nickname: String,
        content: String,
    ) {
        self.insert_session_with_meta_async(
            note_id, session_id, role, nickname, content, None, None, None, None, None, None, None,
            None,
        )
        .await;
    }

    /// Async wrapper for insert_session_with_meta.
    pub async fn insert_session_with_meta_async(
        &self,
        note_id: String,
        session_id: String,
        role: String,
        nickname: String,
        content: String,
        model: Option<String>,
        tokens_in: Option<i32>,
        tokens_out: Option<i32>,
        steps: Option<i32>,
        tool_calls: Option<i32>,
        thinking: Option<String>,
        context_tokens: Option<i32>,
        duration_ms: Option<u64>,
    ) {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.insert_session_with_meta(
                &note_id,
                &session_id,
                &role,
                &nickname,
                &content,
                model.as_deref(),
                tokens_in,
                tokens_out,
                steps,
                tool_calls,
                thinking.as_deref(),
                context_tokens,
                duration_ms,
            ) {
                warn!("Failed to persist chat message: {}", e);
            }
        })
        .await
        .ok();
    }

    /// Async wrapper for insert (runs on blocking thread pool).
    pub async fn insert_async(
        &self,
        note_id: String,
        role: String,
        nickname: String,
        content: String,
    ) {
        self.insert_session_async(note_id, "main".to_string(), role, nickname, content)
            .await;
    }

    /// Async wrapper for insert_with_meta.
    pub async fn insert_with_meta_async(
        &self,
        note_id: String,
        role: String,
        nickname: String,
        content: String,
        model: Option<String>,
        tokens_in: Option<i32>,
        tokens_out: Option<i32>,
        steps: Option<i32>,
        tool_calls: Option<i32>,
        thinking: Option<String>,
        context_tokens: Option<i32>,
        duration_ms: Option<u64>,
    ) {
        self.insert_session_with_meta_async(
            note_id,
            "main".to_string(),
            role,
            nickname,
            content,
            model,
            tokens_in,
            tokens_out,
            steps,
            tool_calls,
            thinking,
            context_tokens,
            duration_ms,
        )
        .await;
    }

    /// Async wrapper for load_recent_session.
    pub async fn load_recent_session_async(
        &self,
        note_id: String,
        session_id: String,
        limit: usize,
    ) -> Vec<ChatRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.load_recent_session(&note_id, &session_id, limit)
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }

    /// Async wrapper for load_recent.
    pub async fn load_recent_async(&self, note_id: String, limit: usize) -> Vec<ChatRecord> {
        self.load_recent_session_async(note_id, "main".to_string(), limit)
            .await
    }

    /// Async wrapper for list_sessions_for_note.
    pub async fn list_sessions_for_note_async(
        &self,
        note_id: String,
    ) -> anyhow::Result<Vec<String>> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.list_sessions_for_note(&note_id)).await?
    }

    /// Async wrapper for get_session_meta.
    pub async fn get_session_meta_async(
        &self,
        note_id: String,
        session_id: String,
    ) -> Option<(Option<String>, Option<String>)> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.get_session_meta(&note_id, &session_id))
            .await
            .ok()
            .flatten()
    }

    /// Async wrapper for set_session_meta.
    pub async fn set_session_meta_async(
        &self,
        note_id: String,
        session_id: String,
        model: Option<String>,
        thinking: Option<String>,
    ) -> anyhow::Result<()> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.set_session_meta(&note_id, &session_id, model.as_deref(), thinking.as_deref())
        })
        .await?
    }

    /// Async wrapper for ensure_session.
    pub async fn ensure_session_async(
        &self,
        note_id: String,
        session_id: String,
    ) -> anyhow::Result<()> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.ensure_session(&note_id, &session_id)).await?
    }

    /// Async wrapper for set_session_title.
    pub async fn set_session_title_async(
        &self,
        note_id: String,
        session_id: String,
        title: Option<String>,
    ) -> anyhow::Result<()> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.set_session_title(&note_id, &session_id, title.as_deref())
        })
        .await?
    }

    /// Async wrapper for set_session_custom_title.
    pub async fn set_session_custom_title_async(
        &self,
        note_id: String,
        session_id: String,
        custom_title: Option<String>,
    ) -> anyhow::Result<()> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.set_session_custom_title(&note_id, &session_id, custom_title.as_deref())
        })
        .await?
    }

    /// Async wrapper for list_chat_sessions_meta.
    pub async fn list_chat_sessions_meta_async(
        &self,
        note_id: String,
    ) -> anyhow::Result<Vec<ChatSessionMeta>> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.list_chat_sessions_meta(&note_id)).await?
    }

    pub fn save_session(
        &self,
        id: &str,
        login: &str,
        role: &str,
        avatar_url: &str,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO user_sessions (id, login, role, avatar_url, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET login=?2, role=?3, avatar_url=?4, expires_at=?5",
            params![id, login, role, avatar_url, expires_at],
        )?;
        Ok(())
    }

    pub fn remove_session(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM user_sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn load_active_sessions(
        &self,
    ) -> anyhow::Result<Vec<(String, String, String, String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let now = now_secs();
        let mut stmt = conn.prepare(
            "SELECT id, login, role, avatar_url, expires_at FROM user_sessions WHERE expires_at > ?1",
        )?;
        let rows = stmt.query_map(params![now], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?;
        let mut res = Vec::new();
        for r in rows {
            if let Ok(item) = r {
                res.push(item);
            }
        }
        Ok(res)
    }

    pub fn save_oauth_token(
        &self,
        token: &str,
        role: &str,
        login: &str,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO oauth_tokens (token, role, login, expires_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(token) DO UPDATE SET role=?2, login=?3, expires_at=?4",
            params![token, role, login, expires_at],
        )?;
        Ok(())
    }

    pub fn remove_oauth_token(&self, token: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM oauth_tokens WHERE token = ?1", params![token])?;
        Ok(())
    }

    pub fn load_active_oauth_tokens(&self) -> anyhow::Result<Vec<(String, String, String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let now = now_secs();
        let mut stmt = conn.prepare(
            "SELECT token, role, login, expires_at FROM oauth_tokens WHERE expires_at > ?1",
        )?;
        let rows = stmt.query_map(params![now], |row| {
            let token: String = row.get(0)?;
            let role: String = row.get(1)?;
            let login: String = row.get(2).unwrap_or_default();
            let expires_at: i64 = row.get(3)?;
            Ok((token, role, login, expires_at))
        })?;
        let mut res = Vec::new();
        for r in rows {
            if let Ok(item) = r {
                res.push(item);
            }
        }
        Ok(res)
    }

    pub fn sweep_expired_auth(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_secs();
        let _ = conn.execute(
            "DELETE FROM user_sessions WHERE expires_at <= ?1",
            params![now],
        );
        let _ = conn.execute(
            "DELETE FROM oauth_tokens WHERE expires_at <= ?1",
            params![now],
        );
        Ok(())
    }

    // ── Session CRUD ──────────────────────────────────────────────────────

    /// Create a new session. Returns Ok(()) or error if id already exists.
    pub fn create_note(
        &self,
        id: &str,
        name: &str,
        created_by: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO sessions (id, name, created_at, created_by) VALUES (?1,?2,?3,?4)",
            params![id, name, ts, created_by],
        )?;
        Ok(())
    }

    /// List all sessions ordered by created_at.
    pub fn list_notes(&self) -> anyhow::Result<Vec<NoteRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, created_by, COALESCE(public, 0), model_override, icon FROM sessions ORDER BY name ASC"
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(NoteRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    created_by: row.get(3)?,
                    public: row.get::<_, i32>(4).unwrap_or(0) != 0,
                    model_override: row.get(5)?,
                    icon: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Rename a session (updates both id and name, since id = name).
    /// If new_name already exists, merges: old messages are re-tagged to new_name,
    /// old session row is deleted. Filesystem merge is handled by the caller.
    /// Returns Ok(Some(new_id)) on success, Ok(None) if source not found.
    pub fn rename_note(
        &self,
        id: &str,
        new_name: &str,
        icon: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        // Check source exists
        let src_exists: bool = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get::<_, i64>(0),
        )? > 0;
        if !src_exists {
            return Ok(None);
        }
        // no-op: same name (but update icon if provided)
        if id == new_name {
            conn.execute(
                "UPDATE sessions SET icon = ?1 WHERE id = ?2",
                params![icon, id],
            )?;
            return Ok(Some(new_name.to_string()));
        }
        let target_exists: bool = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            params![new_name],
            |row| row.get::<_, i64>(0),
        )? > 0;

        if target_exists {
            // Merge: re-tag old messages to new_name, delete old session row
            conn.execute(
                "UPDATE sessions SET icon = ?1 WHERE id = ?2",
                params![icon, new_name],
            )?;
            conn.execute(
                "UPDATE messages SET note_id = ?1 WHERE note_id = ?2",
                params![new_name, id],
            )?;
            conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        } else {
            // Simple rename: update session row + re-tag messages
            conn.execute(
                "UPDATE sessions SET id = ?1, name = ?1, icon = ?2 WHERE id = ?3",
                params![new_name, icon, id],
            )?;
            conn.execute(
                "UPDATE messages SET note_id = ?1 WHERE note_id = ?2",
                params![new_name, id],
            )?;
        }
        Ok(Some(new_name.to_string()))
    }

    /// Delete a session (metadata only; does NOT delete chat messages).
    pub fn delete_note(&self, id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }

    /// Get a single session by id.
    pub fn get_session(&self, id: &str) -> anyhow::Result<Option<NoteRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, created_by, COALESCE(public, 0), model_override, icon FROM sessions WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(NoteRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                created_by: row.get(3)?,
                public: row.get::<_, i32>(4).unwrap_or(0) != 0,
                model_override: row.get(5)?,
                icon: row.get(6)?,
            })
        })?;
        Ok(rows.next().and_then(|r| r.ok()))
    }
    // ── End Session CRUD ─────────────────────────────────────────────────

    // ── Per-Note Model Override ───────────────────────────────────────────────

    /// Set per-note model override. Pass None to clear (fallback to global default).
    pub fn set_note_model(&self, id: &str, model: Option<&str>) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET model_override = ?1 WHERE id = ?2",
            params![model, id],
        )?;
        Ok(())
    }

    /// Get per-note model override. Returns None if not set (use global default).
    pub fn get_note_model(&self, id: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT model_override FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    // ── Visibility ────────────────────────────────────────────────────────────

    // ── Per-Note Thinking Override ────────────────────────────────────────────

    /// Set per-note thinking override. Pass None to clear (fall back to config.thinking).
    pub fn set_note_thinking(&self, note_id: &str, thinking: Option<&str>) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "CREATE TABLE IF NOT EXISTS note_settings (note_id TEXT PRIMARY KEY, thinking TEXT)",
            [],
        );
        if let Some(t) = thinking {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO note_settings (note_id, thinking) VALUES (?1, ?2)",
                params![note_id, t],
            );
        } else {
            let _ = conn.execute(
                "DELETE FROM note_settings WHERE note_id = ?1",
                params![note_id],
            );
        }
    }

    /// Get per-note thinking override. Returns None if not set.
    pub fn get_note_thinking(&self, note_id: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "CREATE TABLE IF NOT EXISTS note_settings (note_id TEXT PRIMARY KEY, thinking TEXT)",
            [],
        );
        conn.query_row(
            "SELECT thinking FROM note_settings WHERE note_id = ?1",
            params![note_id],
            |row| row.get(0),
        )
        .ok()
    }

    // ── Visibility ────────────────────────────────────────────────────────────

    /// Check if a note is public.
    pub fn is_note_public(&self, id: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT public FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get::<_, i32>(0),
        )
        .map(|v| v != 0)
        .unwrap_or(false)
    }

    /// Set note-level public flag. Returns new state.
    pub fn set_note_public(&self, id: &str, public: bool) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET public = ?1 WHERE id = ?2",
            params![public as i32, id],
        )?;
        Ok(public)
    }

    /// Set file-level public flag.
    pub fn set_file_public(
        &self,
        note_id: &str,
        filename: &str,
        public: bool,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO file_visibility (note_id, filename, public) VALUES (?1,?2,?3)
             ON CONFLICT(note_id, filename) DO UPDATE SET public = ?3",
            params![note_id, filename, public as i32],
        )?;
        Ok(())
    }

    /// Check if a specific file is public.
    pub fn is_file_public(&self, note_id: &str, filename: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT public FROM file_visibility WHERE note_id = ?1 AND filename = ?2",
            params![note_id, filename],
            |row| row.get::<_, i32>(0),
        )
        .unwrap_or(0)
            != 0
    }

    /// List all public files for a note.
    pub fn list_public_files(&self, note_id: &str) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT filename FROM file_visibility WHERE note_id = ?1 AND public = 1 ORDER BY filename ASC"
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![note_id], |row| row.get::<_, String>(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Remove file visibility record when a file is deleted.
    pub fn remove_file_visibility(&self, note_id: &str, filename: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM file_visibility WHERE note_id = ?1 AND filename = ?2",
            params![note_id, filename],
        )?;
        Ok(())
    }

    /// Get all files for a note with their public state.
    pub fn get_file_visibility(&self, note_id: &str) -> Vec<(String, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT filename, public FROM file_visibility WHERE note_id = ?1 ORDER BY filename ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![note_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)? != 0))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Create a new cron job record in the database.
    pub fn create_cron_job(&self, job: &NoteCronJobRecord) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO note_cron_jobs (id, note_id, name, schedule_type, schedule_value, prompt, model, silent_if_no_action, enabled, last_run_at, last_status, created_at, updated_at, timeout_secs, thinking)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                job.id,
                job.note_id,
                job.name,
                job.schedule_type,
                job.schedule_value,
                job.prompt,
                job.model,
                job.silent_if_no_action as i32,
                job.enabled as i32,
                job.last_run_at,
                job.last_status,
                job.created_at,
                job.updated_at,
                job.timeout_secs.map(|t| t as i64),
                job.thinking
            ],
        )?;
        Ok(())
    }

    /// Retrieve a single cron job by ID.
    pub fn get_cron_job(&self, job_id: &str) -> anyhow::Result<Option<NoteCronJobRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, note_id, name, schedule_type, schedule_value, prompt, model, silent_if_no_action, enabled, last_run_at, last_status, created_at, updated_at, timeout_secs, thinking
             FROM note_cron_jobs WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![job_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(NoteCronJobRecord {
                id: row.get(0)?,
                note_id: row.get(1)?,
                name: row.get(2)?,
                schedule_type: row.get(3)?,
                schedule_value: row.get(4)?,
                prompt: row.get(5)?,
                model: row.get(6)?,
                silent_if_no_action: row.get::<_, i32>(7)? != 0,
                enabled: row.get::<_, i32>(8)? != 0,
                last_run_at: row.get(9)?,
                last_status: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                timeout_secs: row.get::<_, Option<i64>>(13)?.map(|v| v.max(1) as u64),
                thinking: row.get(14)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// List all cron jobs for a specific note.
    pub fn list_cron_jobs_for_note(&self, note_id: &str) -> anyhow::Result<Vec<NoteCronJobRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, note_id, name, schedule_type, schedule_value, prompt, model, silent_if_no_action, enabled, last_run_at, last_status, created_at, updated_at, timeout_secs, thinking
             FROM note_cron_jobs WHERE note_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![note_id], |row| {
            Ok(NoteCronJobRecord {
                id: row.get(0)?,
                note_id: row.get(1)?,
                name: row.get(2)?,
                schedule_type: row.get(3)?,
                schedule_value: row.get(4)?,
                prompt: row.get(5)?,
                model: row.get(6)?,
                silent_if_no_action: row.get::<_, i32>(7)? != 0,
                enabled: row.get::<_, i32>(8)? != 0,
                last_run_at: row.get(9)?,
                last_status: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                timeout_secs: row.get::<_, Option<i64>>(13)?.map(|v| v.max(1) as u64),
                thinking: row.get(14)?,
            })
        })?;
        let mut jobs = Vec::new();
        for job in rows {
            jobs.push(job?);
        }
        Ok(jobs)
    }

    /// List all enabled cron jobs across all notes.
    pub fn list_all_enabled_cron_jobs(&self) -> anyhow::Result<Vec<NoteCronJobRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, note_id, name, schedule_type, schedule_value, prompt, model, silent_if_no_action, enabled, last_run_at, last_status, created_at, updated_at, timeout_secs, thinking
             FROM note_cron_jobs WHERE enabled = 1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(NoteCronJobRecord {
                id: row.get(0)?,
                note_id: row.get(1)?,
                name: row.get(2)?,
                schedule_type: row.get(3)?,
                schedule_value: row.get(4)?,
                prompt: row.get(5)?,
                model: row.get(6)?,
                silent_if_no_action: row.get::<_, i32>(7)? != 0,
                enabled: row.get::<_, i32>(8)? != 0,
                last_run_at: row.get(9)?,
                last_status: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                timeout_secs: row.get::<_, Option<i64>>(13)?.map(|v| v.max(1) as u64),
                thinking: row.get(14)?,
            })
        })?;
        let mut jobs = Vec::new();
        for job in rows {
            jobs.push(job?);
        }
        Ok(jobs)
    }

    /// Update an existing cron job. Returns true if a row was updated.
    pub fn update_cron_job(&self, job: &NoteCronJobRecord) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE note_cron_jobs
             SET name = ?1, schedule_type = ?2, schedule_value = ?3, prompt = ?4, model = ?5,
                 silent_if_no_action = ?6, enabled = ?7, updated_at = ?8, timeout_secs = ?9,
                 thinking = ?10
             WHERE id = ?11",
            params![
                job.name,
                job.schedule_type,
                job.schedule_value,
                job.prompt,
                job.model,
                job.silent_if_no_action as i32,
                job.enabled as i32,
                job.updated_at,
                job.timeout_secs.map(|t| t as i64),
                job.thinking,
                job.id
            ],
        )?;
        Ok(count > 0)
    }

    /// Delete a cron job by ID. Returns true if deleted.
    pub fn delete_cron_job(&self, job_id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute("DELETE FROM note_cron_jobs WHERE id = ?1", params![job_id])?;
        Ok(count > 0)
    }

    /// Update the last execution status and timestamp of a cron job.
    pub fn update_cron_job_status(
        &self,
        job_id: &str,
        last_run_at: &str,
        last_status: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE note_cron_jobs SET last_run_at = ?1, last_status = ?2 WHERE id = ?3",
            params![last_run_at, last_status, job_id],
        )?;
        Ok(())
    }

    // Async wrappers
    pub async fn create_cron_job_async(&self, job: NoteCronJobRecord) -> anyhow::Result<()> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.create_cron_job(&job)).await?
    }

    pub async fn get_cron_job_async(
        &self,
        job_id: String,
    ) -> anyhow::Result<Option<NoteCronJobRecord>> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.get_cron_job(&job_id)).await?
    }

    pub async fn list_cron_jobs_for_note_async(
        &self,
        note_id: String,
    ) -> anyhow::Result<Vec<NoteCronJobRecord>> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.list_cron_jobs_for_note(&note_id)).await?
    }

    pub async fn list_all_enabled_cron_jobs_async(&self) -> anyhow::Result<Vec<NoteCronJobRecord>> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.list_all_enabled_cron_jobs()).await?
    }

    pub async fn update_cron_job_async(&self, job: NoteCronJobRecord) -> anyhow::Result<bool> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.update_cron_job(&job)).await?
    }

    pub async fn delete_cron_job_async(&self, job_id: String) -> anyhow::Result<bool> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.delete_cron_job(&job_id)).await?
    }

    pub async fn update_cron_job_status_async(
        &self,
        job_id: String,
        last_run_at: String,
        last_status: String,
    ) -> anyhow::Result<()> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.update_cron_job_status(&job_id, &last_run_at, &last_status)
        })
        .await?
    }

    /// Dump all messages for a session to JSONL, then delete them from the DB.
    /// Returns the number of messages archived.
    pub fn archive_session(
        &self,
        note_id: &str,
        session_id: &str,
        archive_path: &Path,
    ) -> anyhow::Result<usize> {
        use std::io::Write;
        let conn = self.conn.lock().unwrap();
        let s_id = if session_id.trim().is_empty() {
            "main"
        } else {
            session_id
        };
        // Load all messages for this session
        let mut stmt = conn.prepare(
            "SELECT id, note_id, session_id, role, nickname, content, created_at, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens, duration_ms
             FROM messages WHERE note_id = ?1 AND session_id = ?2 ORDER BY id ASC",
        )?;
        let records: Vec<ChatRecord> = stmt
            .query_map(params![note_id, s_id], |row| {
                Ok(ChatRecord {
                    id: row.get(0)?,
                    note_id: row.get(1)?,
                    session_id: row.get(2)?,
                    role: row.get(3)?,
                    nickname: row.get(4)?,
                    content: row.get(5)?,
                    created_at: row.get(6)?,
                    model: row.get(7)?,
                    tokens_in: row.get(8)?,
                    tokens_out: row.get(9)?,
                    steps: row.get(10)?,
                    tool_calls: row.get(11)?,
                    thinking: row.get(12).ok().flatten(),
                    context_tokens: row.get(13).ok().flatten(),
                    duration_ms: row
                        .get::<_, Option<i64>>(14)
                        .ok()
                        .flatten()
                        .map(|v| v as u64),
                    archive_file: None,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        // Delete session metadata and messages from DB
        let _ = conn.execute(
            "DELETE FROM chat_sessions WHERE note_id = ?1 AND session_id = ?2",
            params![note_id, s_id],
        );
        let _ = conn.execute(
            "DELETE FROM messages WHERE note_id = ?1 AND session_id = ?2",
            params![note_id, s_id],
        );

        if records.is_empty() {
            return Ok(0);
        }

        if let Some(parent) = archive_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open archive file in append mode
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(archive_path)?;

        for rec in &records {
            let line = serde_json::to_string(rec)?;
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.flush()?;

        Ok(records.len())
    }

    /// Dump all messages for "main" session to JSONL, then delete them from the DB.
    pub fn archive(&self, note_id: &str, archive_path: &Path) -> anyhow::Result<usize> {
        self.archive_session(note_id, "main", archive_path)
    }

    /// Search chat history for a note (both disk archives and live DB).
    ///
    /// Returns matching `ChatRecord`s matching query (case-insensitive) across all archives + live DB,
    /// sorted newest-first.
    pub fn search(
        &self,
        note_id: &str,
        query: &str,
        archive_dir: &Path,
    ) -> anyhow::Result<Vec<ChatRecord>> {
        use std::io::BufRead;
        let query_lower = query.to_lowercase();
        let mut results: Vec<ChatRecord> = Vec::new();

        // 1. Search disk archives (*.jsonl in archive_dir)
        if archive_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(archive_dir) {
                let mut paths: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
                    .collect();
                paths.sort();
                for path in paths {
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string();
                    if let Ok(file) = std::fs::File::open(&path) {
                        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                            if let Ok(mut rec) = serde_json::from_str::<ChatRecord>(&line) {
                                if rec.note_id == note_id
                                    && (rec.content.to_lowercase().contains(&query_lower)
                                        || rec.nickname.to_lowercase().contains(&query_lower))
                                {
                                    rec.archive_file = Some(filename.clone());
                                    results.push(rec);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Search live DB
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, note_id, session_id, role, nickname, content, created_at, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens, duration_ms
             FROM messages WHERE note_id = ?1 ORDER BY id ASC",
        )?;
        let live: Vec<ChatRecord> = stmt
            .query_map(params![note_id], |row| {
                Ok(ChatRecord {
                    id: row.get(0)?,
                    note_id: row.get(1)?,
                    session_id: row.get(2)?,
                    role: row.get(3)?,
                    nickname: row.get(4)?,
                    content: row.get(5)?,
                    created_at: row.get(6)?,
                    model: row.get(7)?,
                    tokens_in: row.get(8)?,
                    tokens_out: row.get(9)?,
                    steps: row.get(10)?,
                    tool_calls: row.get(11)?,
                    thinking: row.get(12).ok().flatten(),
                    context_tokens: row.get(13).ok().flatten(),
                    duration_ms: row
                        .get::<_, Option<i64>>(14)
                        .ok()
                        .flatten()
                        .map(|v| v as u64),
                    archive_file: None,
                })
            })?
            .filter_map(|r| r.ok())
            .filter(|r| {
                r.content.to_lowercase().contains(&query_lower)
                    || r.nickname.to_lowercase().contains(&query_lower)
            })
            .collect();
        results.extend(live);
        // Sort newest first (most recent on top, oldest at the bottom)
        results.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });

        Ok(results)
    }

    /// Restore messages from an archive file into active messages table for a specific session.
    /// Replaces the current active messages for `note_id` + `session_id`.
    pub fn restore_session(
        &self,
        note_id: &str,
        session_id: &str,
        archive_path: &Path,
    ) -> anyhow::Result<usize> {
        use std::io::BufRead;
        if !archive_path.exists() {
            return Err(anyhow::anyhow!(
                "Archive file does not exist: {:?}",
                archive_path
            ));
        }
        let file = std::fs::File::open(archive_path)?;
        let mut records: Vec<ChatRecord> = Vec::new();
        let s_id = if session_id.trim().is_empty() {
            "main"
        } else {
            session_id
        };
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            if let Ok(mut rec) = serde_json::from_str::<ChatRecord>(&line) {
                if rec.note_id == note_id {
                    rec.session_id = s_id.to_string();
                    records.push(rec);
                }
            }
        }
        if records.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        // Clear current active messages for this session
        tx.execute(
            "DELETE FROM messages WHERE note_id = ?1 AND session_id = ?2",
            params![note_id, s_id],
        )?;

        for r in &records {
            tx.execute(
                "INSERT INTO messages (note_id, session_id, role, nickname, content, created_at, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    r.note_id,
                    r.session_id,
                    r.role,
                    r.nickname,
                    r.content,
                    r.created_at,
                    r.model,
                    r.tokens_in,
                    r.tokens_out,
                    r.steps,
                    r.tool_calls,
                    r.thinking,
                    r.context_tokens,
                    r.duration_ms.map(|d| d as i64),
                ],
            )?;
        }
        tx.commit()?;
        Ok(records.len())
    }

    /// Restore messages from an archive file into active messages table ("main" session).
    pub fn restore(&self, note_id: &str, archive_path: &Path) -> anyhow::Result<usize> {
        self.restore_session(note_id, "main", archive_path)
    }

    /// Async wrapper for restore_session.
    pub async fn restore_session_async(
        &self,
        note_id: String,
        session_id: String,
        archive_path: std::path::PathBuf,
    ) -> anyhow::Result<usize> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.restore_session(&note_id, &session_id, &archive_path)
        })
        .await?
    }

    /// Async wrapper for restore.
    pub async fn restore_async(
        &self,
        note_id: String,
        archive_path: std::path::PathBuf,
    ) -> anyhow::Result<usize> {
        self.restore_session_async(note_id, "main".to_string(), archive_path)
            .await
    }

    /// Async wrapper for archive_session.
    pub async fn archive_session_async(
        &self,
        note_id: String,
        session_id: String,
        archive_path: std::path::PathBuf,
    ) -> anyhow::Result<usize> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.archive_session(&note_id, &session_id, &archive_path)
        })
        .await?
    }

    /// Async wrapper for archive.
    pub async fn archive_async(
        &self,
        note_id: String,
        archive_path: std::path::PathBuf,
    ) -> anyhow::Result<usize> {
        self.archive_session_async(note_id, "main".to_string(), archive_path)
            .await
    }

    /// Async wrapper for search.
    pub async fn search_async(
        &self,
        note_id: String,
        query: String,
        archive_dir: std::path::PathBuf,
    ) -> Vec<ChatRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.search(&note_id, &query, &archive_dir)
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn in_memory_db() -> ChatDb {
        ChatDb::open(Path::new(":memory:")).expect("in-memory db")
    }

    #[test]
    fn test_insert_and_load() {
        let db = in_memory_db();
        db.insert("default", "user", "alice", "hello").unwrap();
        db.insert("default", "assistant", "ᚱᚢᚾᛖ", "hi there")
            .unwrap();
        let rows = db.load_recent("default", 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].role, "user");
        assert_eq!(rows[0].nickname, "alice");
        assert_eq!(rows[0].content, "hello");
        assert_eq!(rows[1].role, "assistant");
        assert_eq!(rows[1].content, "hi there");
    }

    #[test]
    fn test_load_recent_limit() {
        let db = in_memory_db();
        for i in 0..10 {
            db.insert("default", "user", "bob", &format!("msg {}", i))
                .unwrap();
        }
        let rows = db.load_recent("default", 5).unwrap();
        assert_eq!(rows.len(), 5);
        // Should be the last 5, oldest first
        assert_eq!(rows[0].content, "msg 5");
        assert_eq!(rows[4].content, "msg 9");
    }

    #[test]
    fn test_load_recent_oldest_first() {
        let db = in_memory_db();
        db.insert("default", "user", "alice", "first").unwrap();
        db.insert("default", "user", "bob", "second").unwrap();
        db.insert("default", "user", "carol", "third").unwrap();
        let rows = db.load_recent("default", 10).unwrap();
        assert_eq!(rows[0].content, "first");
        assert_eq!(rows[2].content, "third");
    }

    #[test]
    fn test_multiple_sessions() {
        let db = in_memory_db();
        db.insert("room-a", "user", "alice", "hello room a")
            .unwrap();
        db.insert("room-b", "user", "bob", "hello room b").unwrap();
        let a = db.load_recent("room-a", 10).unwrap();
        let b = db.load_recent("room-b", 10).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].content, "hello room a");
        assert_eq!(b[0].content, "hello room b");
    }

    #[test]
    fn test_insert_returns_incremental_id() {
        let db = in_memory_db();
        let id1 = db.insert("default", "user", "alice", "msg1").unwrap();
        let id2 = db.insert("default", "user", "alice", "msg2").unwrap();
        assert!(id2 > id1);
    }

    #[test]
    fn test_empty_session_returns_empty() {
        let db = in_memory_db();
        let rows = db.load_recent("nonexistent", 10).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn test_created_at_is_set() {
        let db = in_memory_db();
        db.insert("default", "user", "alice", "test").unwrap();
        let rows = db.load_recent("default", 1).unwrap();
        assert!(rows[0].created_at > 0);
    }

    #[test]
    fn test_archive_writes_jsonl_and_clears_db() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        let db = ChatDb::open(&db_path).unwrap();
        db.insert("default", "user", "alice", "hello").unwrap();
        db.insert("default", "assistant", "ᚱᚢᚾᛖ", "hi").unwrap();

        let archive_path = dir.path().join("arc.jsonl");
        let count = db.archive("default", &archive_path).unwrap();
        assert_eq!(count, 2);

        // DB should be empty now
        let rows = db.load_recent("default", 10).unwrap();
        assert!(rows.is_empty(), "DB should be cleared after archive");

        // JSONL should have 2 lines
        let file = std::fs::File::open(&archive_path).unwrap();
        let lines: Vec<_> = std::io::BufReader::new(file).lines().collect();
        assert_eq!(lines.len(), 2);
        let rec: ChatRecord = serde_json::from_str(&lines[0].as_ref().unwrap()).unwrap();
        assert_eq!(rec.content, "hello");
    }

    #[test]
    fn test_archive_empty_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        let db = ChatDb::open(&db_path).unwrap();
        let archive_path = dir.path().join("arc.jsonl");
        let count = db.archive("default", &archive_path).unwrap();
        assert_eq!(count, 0);
        assert!(
            !archive_path.exists(),
            "No file should be created for empty archive"
        );
    }

    #[test]
    fn test_search_live_messages() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        let db = ChatDb::open(&db_path).unwrap();
        db.insert("default", "user", "alice", "hello world")
            .unwrap();
        db.insert("default", "assistant", "ᚱᚢᚾᛖ", "goodbye")
            .unwrap();
        db.insert("default", "user", "alice", "hello again")
            .unwrap();

        let arc_dir = dir.path().join("archives");
        let results = db.search("default", "hello", &arc_dir).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.content.contains("hello")));
        assert_eq!(results[0].content, "hello again");
        assert_eq!(results[1].content, "hello world");
    }

    #[test]
    fn test_search_across_archive_and_live() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        let db = ChatDb::open(&db_path).unwrap();

        // Create an archive JSONL with one matching record
        let arc_dir = dir.path().join("archives");
        std::fs::create_dir_all(&arc_dir).unwrap();
        let arc_path = arc_dir.join("old.jsonl");
        let old_rec = ChatRecord {
            id: 1,
            note_id: "default".into(),
            session_id: "main".into(),
            role: "user".into(),
            nickname: "bob".into(),
            content: "search me".into(),
            created_at: 1000,
            model: None,
            tokens_in: None,
            tokens_out: None,
            steps: None,
            tool_calls: None,
            thinking: None,
            context_tokens: None,
            duration_ms: None,
            archive_file: None,
        };
        let mut f = std::fs::File::create(&arc_path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&old_rec).unwrap()).unwrap();

        // Live DB also has one match
        db.insert("default", "user", "alice", "search me too")
            .unwrap();
        db.insert("default", "user", "alice", "nothing here")
            .unwrap();

        let results = db.search("default", "search me", &arc_dir).unwrap();
        assert_eq!(results.len(), 2, "Should find 1 archive + 1 live result");
        assert_eq!(
            results[0].content, "search me too",
            "Newer live result first"
        );
        assert_eq!(results[1].content, "search me", "Older archive result last");
        assert_eq!(
            results[1].archive_file.as_deref(),
            Some("old.jsonl"),
            "Archive filename should be populated"
        );
    }

    #[test]
    fn test_restore_from_archive() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        let db = ChatDb::open(&db_path).unwrap();

        // 1. Initial messages
        db.insert("note-1", "user", "alice", "old message 1")
            .unwrap();
        db.insert("note-1", "assistant", "ᚱᚢᚾᛖ", "old response 1")
            .unwrap();

        // 2. Archive to file
        let archive_path = dir.path().join("archive_1.jsonl");
        let archived_count = db.archive("note-1", &archive_path).unwrap();
        assert_eq!(archived_count, 2);
        assert!(db.load_recent("note-1", 10).unwrap().is_empty());

        // 3. New conversation happens in note-1
        db.insert("note-1", "user", "bob", "new active conversation")
            .unwrap();
        assert_eq!(db.load_recent("note-1", 10).unwrap().len(), 1);

        // 4. Restore the old archive
        let restored_count = db.restore("note-1", &archive_path).unwrap();
        assert_eq!(restored_count, 2);

        let active_now = db.load_recent("note-1", 10).unwrap();
        assert_eq!(active_now.len(), 2);
        assert_eq!(active_now[0].content, "old message 1");
        assert_eq!(active_now[1].content, "old response 1");
    }

    #[tokio::test]
    async fn test_insert_async_and_load_async() {
        let db = in_memory_db();
        db.insert_async(
            "default".into(),
            "user".into(),
            "alice".into(),
            "async msg".into(),
        )
        .await;
        let rows = db.load_recent_async("default".into(), 10).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "async msg");
    }

    #[test]
    fn test_insert_with_meta_persists_model_tokens() {
        let db = in_memory_db();
        db.insert_with_meta(
            "default",
            "assistant",
            "ᚱᚢᚾᛖ",
            "hello",
            Some("gpt-5-mini"),
            Some(100),
            Some(42),
            Some(3),
            Some(2),
            None,
            None,
            None,
        )
        .unwrap();
        let rows = db.load_recent("default", 1).unwrap();
        assert_eq!(rows[0].model.as_deref(), Some("gpt-5-mini"));
        assert_eq!(rows[0].tokens_in, Some(100));
        assert_eq!(rows[0].tokens_out, Some(42));
        assert_eq!(rows[0].steps, Some(3));
        assert_eq!(rows[0].tool_calls, Some(2));
    }

    #[test]
    fn test_insert_with_meta_persists_context_tokens() {
        let db = in_memory_db();
        db.insert_with_meta(
            "default",
            "assistant",
            "ᚱᚢᚾᛖ",
            "hello",
            Some("gpt-5-mini"),
            Some(100),
            Some(42),
            Some(3),
            Some(2),
            None,
            Some(4200),
            None,
        )
        .unwrap();
        let rows = db.load_recent("default", 1).unwrap();
        assert_eq!(rows[0].context_tokens, Some(4200));
    }

    #[test]
    fn test_insert_with_meta_persists_duration_ms() {
        let db = in_memory_db();
        db.insert_with_meta(
            "default",
            "assistant",
            "ᚱᚢᚾᛖ",
            "hello",
            Some("gpt-5-mini"),
            Some(100),
            Some(42),
            Some(3),
            Some(2),
            None,
            Some(4200),
            Some(80123),
        )
        .unwrap();
        let rows = db.load_recent("default", 1).unwrap();
        assert_eq!(rows[0].duration_ms, Some(80123));
    }

    #[test]
    fn test_insert_with_meta_context_tokens_none() {
        let db = in_memory_db();
        db.insert_with_meta(
            "default",
            "assistant",
            "ᚱᚢᚾᛖ",
            "hello",
            Some("gpt-5-mini"),
            Some(100),
            Some(42),
            Some(3),
            Some(2),
            None,
            None,
            None,
        )
        .unwrap();
        let rows = db.load_recent("default", 1).unwrap();
        assert!(rows[0].context_tokens.is_none());
        assert!(rows[0].duration_ms.is_none());
    }

    #[test]
    fn test_insert_without_meta_has_none_fields() {
        let db = in_memory_db();
        db.insert("default", "user", "alice", "hi").unwrap();
        let rows = db.load_recent("default", 1).unwrap();
        assert!(rows[0].model.is_none());
        assert!(rows[0].tokens_in.is_none());
        assert!(rows[0].tokens_out.is_none());
        assert!(rows[0].duration_ms.is_none());
    }

    #[test]
    fn test_archive_preserves_meta_in_jsonl() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        let db = ChatDb::open(&db_path).unwrap();
        db.insert_with_meta(
            "default",
            "assistant",
            "ᚱᚢᚾᛖ",
            "reply",
            Some("gpt-4o"),
            Some(50),
            Some(25),
            Some(1),
            Some(0),
            None,
            None,
            Some(1234),
        )
        .unwrap();
        let archive_path = dir.path().join("arc.jsonl");
        db.archive("default", &archive_path).unwrap();
        let file = std::fs::File::open(&archive_path).unwrap();
        let line = std::io::BufReader::new(file)
            .lines()
            .next()
            .unwrap()
            .unwrap();
        let rec: ChatRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(rec.model.as_deref(), Some("gpt-4o"));
        assert_eq!(rec.tokens_in, Some(50));
        assert_eq!(rec.tokens_out, Some(25));
    }

    // ── Session CRUD tests ───────────────────────────────────────────────

    #[test]
    fn test_create_and_list_notes() {
        let db = in_memory_db();
        db.create_note("proj-a", "Project A", Some("admin"))
            .unwrap();
        db.create_note("proj-b", "Project B", None).unwrap();
        let sessions = db.list_notes().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "proj-a");
        assert_eq!(sessions[0].name, "Project A");
        assert_eq!(sessions[0].created_by.as_deref(), Some("admin"));
        assert_eq!(sessions[1].id, "proj-b");
        assert!(sessions[1].created_by.is_none());
    }

    #[test]
    fn test_rename_note() {
        let db = in_memory_db();
        db.create_note("s1", "s1", None).unwrap();
        let result = db.rename_note("s1", "new-name", Some("📝")).unwrap();
        assert_eq!(result, Some("new-name".to_string()));
        // Old id gone, new id exists
        assert!(db.get_session("s1").unwrap().is_none());
        let s = db.get_session("new-name").unwrap().unwrap();
        assert_eq!(s.name, "new-name");
        assert_eq!(s.id, "new-name");
        assert_eq!(s.icon.as_deref(), Some("📝"));
    }

    #[test]
    fn test_rename_note_updates_messages() {
        let db = in_memory_db();
        db.create_note("old", "old", None).unwrap();
        db.insert("old", "user", "alice", "hello").unwrap();
        let _ = db.rename_note("old", "new", None).unwrap();
        // Messages should now be under "new"
        let msgs = db.load_recent("new", 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "hello");
        // Old note_id has no messages
        let old_msgs = db.load_recent("old", 10).unwrap();
        assert!(old_msgs.is_empty());
    }

    #[test]
    fn test_rename_nonexistent_returns_none() {
        let db = in_memory_db();
        assert_eq!(db.rename_note("nope", "X", None).unwrap(), None);
    }

    #[test]
    fn test_rename_note_merge() {
        let db = in_memory_db();
        db.create_note("a", "a", None).unwrap();
        db.create_note("b", "b", None).unwrap();
        // Insert a message under "a"
        db.insert("a", "user", "nick", "hello from a").unwrap();
        // Rename a -> b: target exists, so merge
        assert_eq!(
            db.rename_note("a", "b", Some("🌟")).unwrap(),
            Some("b".into())
        );
        // "a" session should be gone
        assert!(db.get_session("a").unwrap().is_none());
        // "b" session still exists
        let s_b = db.get_session("b").unwrap().unwrap();
        assert_eq!(s_b.icon.as_deref(), Some("🌟"));
        // The message originally under "a" should now be under "b"
        let msgs = db.load_recent("b", 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "hello from a");
    }

    #[test]
    fn test_rename_note_source_not_found() {
        let db = in_memory_db();
        // Renaming non-existent source returns None
        assert_eq!(db.rename_note("ghost", "anything", None).unwrap(), None);
    }

    #[test]
    fn test_rename_note_same_name_update_icon() {
        let db = in_memory_db();
        db.create_note("s1", "s1", None).unwrap();
        assert_eq!(db.get_session("s1").unwrap().unwrap().icon, None);

        // Update icon, name is same
        let result = db.rename_note("s1", "s1", Some("🔥")).unwrap();
        assert_eq!(result, Some("s1".to_string()));

        let s = db.get_session("s1").unwrap().unwrap();
        assert_eq!(s.icon.as_deref(), Some("🔥"));
    }

    #[test]
    fn test_delete_nonexistent_returns_false() {
        let db = in_memory_db();
        assert!(!db.delete_note("nope").unwrap());
    }

    #[test]
    fn test_duplicate_note_id_fails() {
        let db = in_memory_db();
        db.create_note("dup", "First", None).unwrap();
        assert!(db.create_note("dup", "Second", None).is_err());
    }

    #[test]
    fn test_get_session_nonexistent_returns_none() {
        let db = in_memory_db();
        assert!(db.get_session("nope").unwrap().is_none());
    }
    #[test]
    fn test_note_model_persist_and_fallback() {
        let db = in_memory_db();
        db.create_note("test-note", "Test Note", Some("tester"))
            .unwrap();

        // Initially no model override
        assert_eq!(db.get_note_model("test-note"), None);

        // Set model override
        db.set_note_model("test-note", Some("gpt-5.5")).unwrap();
        assert_eq!(db.get_note_model("test-note"), Some("gpt-5.5".to_string()));

        // Update model override
        db.set_note_model("test-note", Some("claude-opus")).unwrap();
        assert_eq!(
            db.get_note_model("test-note"),
            Some("claude-opus".to_string())
        );

        // Clear model override (fallback to default)
        db.set_note_model("test-note", None).unwrap();
        assert_eq!(db.get_note_model("test-note"), None);

        // Non-existent note returns None
        assert_eq!(db.get_note_model("no-such-note"), None);
    }

    #[test]
    fn test_note_model_in_list_notes() {
        let db = in_memory_db();
        db.create_note("n1", "Note 1", None).unwrap();
        db.create_note("n2", "Note 2", None).unwrap();

        db.set_note_model("n1", Some("gpt-5")).unwrap();

        let notes = db.list_notes().unwrap();
        let n1 = notes.iter().find(|n| n.id == "n1").unwrap();
        let n2 = notes.iter().find(|n| n.id == "n2").unwrap();

        assert_eq!(n1.model_override, Some("gpt-5".to_string()));
        assert_eq!(n2.model_override, None);
    }

    #[test]
    fn test_set_and_list_file_public() {
        let db = in_memory_db();
        db.create_note("note1", "Note 1", None).unwrap();

        db.set_file_public("note1", "a.md", true).unwrap();
        db.set_file_public("note1", "b.md", false).unwrap();
        db.set_file_public("note1", "c.md", true).unwrap();

        let public = db.list_public_files("note1");
        assert!(public.contains(&"a.md".to_string()));
        assert!(!public.contains(&"b.md".to_string()));
        assert!(public.contains(&"c.md".to_string()));
    }

    #[test]
    fn test_remove_file_visibility_clears_record() {
        let db = in_memory_db();
        db.create_note("note1", "Note 1", None).unwrap();

        db.set_file_public("note1", "doc.md", true).unwrap();
        assert!(db
            .list_public_files("note1")
            .contains(&"doc.md".to_string()));

        db.remove_file_visibility("note1", "doc.md").unwrap();
        assert!(!db
            .list_public_files("note1")
            .contains(&"doc.md".to_string()));
    }

    #[test]
    fn test_remove_file_visibility_nonexistent_is_ok() {
        let db = in_memory_db();
        db.create_note("note1", "Note 1", None).unwrap();

        // Removing a record that never existed should not error
        assert!(db.remove_file_visibility("note1", "ghost.md").is_ok());
    }

    #[test]
    fn test_remove_file_visibility_does_not_affect_other_files() {
        let db = in_memory_db();
        db.create_note("note1", "Note 1", None).unwrap();

        db.set_file_public("note1", "keep.md", true).unwrap();
        db.set_file_public("note1", "remove.md", true).unwrap();

        db.remove_file_visibility("note1", "remove.md").unwrap();

        let public = db.list_public_files("note1");
        assert!(public.contains(&"keep.md".to_string()));
        assert!(!public.contains(&"remove.md".to_string()));
    }

    #[test]
    fn test_remove_file_visibility_does_not_affect_other_notes() {
        let db = in_memory_db();
        db.create_note("note1", "Note 1", None).unwrap();
        db.create_note("note2", "Note 2", None).unwrap();

        db.set_file_public("note1", "shared.md", true).unwrap();
        db.set_file_public("note2", "shared.md", true).unwrap();

        db.remove_file_visibility("note1", "shared.md").unwrap();

        assert!(!db
            .list_public_files("note1")
            .contains(&"shared.md".to_string()));
        assert!(db
            .list_public_files("note2")
            .contains(&"shared.md".to_string()));
    }

    #[test]
    fn test_open_lazy_initial_state_and_context_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        assert!(!db_path.exists());

        let db = ChatDb::open_lazy(&db_path).unwrap();
        assert!(db.is_memory());

        // Insert message with meta including context_tokens in lazy mode
        db.insert_with_meta(
            "default",
            "assistant",
            "rune",
            "hello lazy",
            Some("gpt-4o"),
            Some(10),
            Some(20),
            Some(1),
            Some(0),
            None,
            Some(500),
            None,
        )
        .expect("insert_with_meta on lazy db should succeed with context_tokens");

        let recent = db.load_recent("default", 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].content, "hello lazy");
        assert_eq!(recent[0].context_tokens, Some(500));

        // Persist to disk
        db.ensure_persistent().unwrap();
        assert!(!db.is_memory());
        assert!(db_path.exists());

        // Reopen from disk and check data
        let disk_db = ChatDb::open(&db_path).unwrap();
        let disk_recent = disk_db.load_recent("default", 10).unwrap();
        assert_eq!(disk_recent.len(), 1);
        assert_eq!(disk_recent[0].content, "hello lazy");
        assert_eq!(disk_recent[0].context_tokens, Some(500));
    }

    #[tokio::test]
    async fn test_note_cron_jobs_crud() {
        let db = in_memory_db();
        let job = NoteCronJobRecord {
            id: "job_123".to_string(),
            note_id: "test-note".to_string(),
            name: "Heartbeat Periodic Check".to_string(),
            schedule_type: "interval".to_string(),
            schedule_value: "30m".to_string(),
            prompt: "Check HEARTBEAT.md".to_string(),
            model: Some("deepseek-chat".to_string()),
            silent_if_no_action: true,
            enabled: true,
            last_run_at: None,
            last_status: None,
            created_at: "2026-09-21T14:00:00Z".to_string(),
            updated_at: "2026-09-21T14:00:00Z".to_string(),
            timeout_secs: Some(60),
            thinking: Some("low".to_string()),
        };

        // Create
        db.create_cron_job_async(job.clone()).await.unwrap();

        // Get
        let fetched = db.get_cron_job_async("job_123".to_string()).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.name, "Heartbeat Periodic Check");
        assert_eq!(fetched.schedule_type, "interval");
        assert_eq!(fetched.schedule_value, "30m");
        assert_eq!(fetched.thinking, Some("low".to_string()));
        assert!(fetched.silent_if_no_action);
        assert!(fetched.enabled);

        // List for note
        let note_jobs = db
            .list_cron_jobs_for_note_async("test-note".to_string())
            .await
            .unwrap();
        assert_eq!(note_jobs.len(), 1);

        // List all enabled
        let all_enabled = db.list_all_enabled_cron_jobs_async().await.unwrap();
        assert_eq!(all_enabled.len(), 1);

        // Update status
        db.update_cron_job_status_async(
            "job_123".to_string(),
            "2026-09-21T14:30:00Z".to_string(),
            "silent_ok".to_string(),
        )
        .await
        .unwrap();
        let updated_status = db
            .get_cron_job_async("job_123".to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            updated_status.last_run_at,
            Some("2026-09-21T14:30:00Z".to_string())
        );
        assert_eq!(updated_status.last_status, Some("silent_ok".to_string()));

        // Update job fields
        let mut modified = job.clone();
        modified.name = "Renamed Heartbeat".to_string();
        modified.enabled = false;
        let ok = db.update_cron_job_async(modified).await.unwrap();
        assert!(ok);

        let enabled_after_disable = db.list_all_enabled_cron_jobs_async().await.unwrap();
        assert_eq!(enabled_after_disable.len(), 0);

        // Delete
        let deleted = db
            .delete_cron_job_async("job_123".to_string())
            .await
            .unwrap();
        assert!(deleted);
        let fetched_after_delete = db.get_cron_job_async("job_123".to_string()).await.unwrap();
        assert!(fetched_after_delete.is_none());
    }

    #[tokio::test]
    async fn test_legacy_db_migration_adds_timeout_secs_column() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy.db");

        // 1. Manually create an older schema DB where messages already has model, but note_cron_jobs lacks timeout_secs
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    note_id TEXT NOT NULL DEFAULT 'default',
                    role TEXT NOT NULL,
                    nickname TEXT NOT NULL,
                    content TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    model TEXT
                );
                CREATE TABLE note_cron_jobs (
                    id                  TEXT PRIMARY KEY,
                    note_id             TEXT NOT NULL,
                    name                TEXT NOT NULL,
                    schedule_type       TEXT NOT NULL,
                    schedule_value      TEXT NOT NULL,
                    prompt              TEXT NOT NULL,
                    model               TEXT,
                    silent_if_no_action INTEGER NOT NULL DEFAULT 1,
                    enabled             INTEGER NOT NULL DEFAULT 1,
                    last_run_at         TEXT,
                    last_status         TEXT,
                    created_at          TEXT NOT NULL,
                    updated_at          TEXT NOT NULL
                );
                INSERT INTO note_cron_jobs VALUES (
                    'job_old', 'my-note', 'Old Job', 'interval', '30m', 'Old prompt', NULL, 1, 1, NULL, NULL, '2026-09-20', '2026-09-20'
                );
            ",
            )
            .unwrap();
        }

        // 2. Open via ChatDb::open, which triggers init_schema and migrations
        let db = ChatDb::open(&db_path).expect("ChatDb::open should succeed on legacy database");

        // 3. Query cron jobs — should succeed without 'no such column: timeout_secs' error
        let jobs = db
            .list_cron_jobs_for_note_async("my-note".to_string())
            .await
            .expect("list_cron_jobs_for_note_async should succeed on migrated DB");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "job_old");
        assert_eq!(jobs[0].name, "Old Job");
        assert_eq!(jobs[0].timeout_secs, Some(60));
    }

    #[tokio::test]
    async fn test_session_isolation_and_listing() {
        let db = in_memory_db();
        db.insert_session("note-a", "main", "user", "alice", "hello main")
            .unwrap();
        db.insert_session(
            "note-a",
            "user:U1234",
            "user",
            "bob",
            "hello from line user",
        )
        .unwrap();
        db.insert_session(
            "note-a",
            "group:C5678",
            "user",
            "charlie",
            "hello from line group",
        )
        .unwrap();

        let main_msgs = db.load_recent_session("note-a", "main", 10).unwrap();
        assert_eq!(main_msgs.len(), 1);
        assert_eq!(main_msgs[0].content, "hello main");
        assert_eq!(main_msgs[0].session_id, "main");

        let user_msgs = db.load_recent_session("note-a", "user:U1234", 10).unwrap();
        assert_eq!(user_msgs.len(), 1);
        assert_eq!(user_msgs[0].content, "hello from line user");
        assert_eq!(user_msgs[0].session_id, "user:U1234");

        let sessions = db.list_sessions_for_note("note-a").unwrap();
        assert!(sessions.contains(&"group:C5678".to_string()));
        assert!(sessions.contains(&"main".to_string()));
        assert!(sessions.contains(&"user:U1234".to_string()));
    }

    #[tokio::test]
    async fn test_chat_sessions_meta() {
        let db = in_memory_db();
        db.set_session_meta("note-a", "user:U100", Some("openai/gpt-4o"), Some("high"))
            .unwrap();

        let meta = db.get_session_meta("note-a", "user:U100").unwrap();
        assert_eq!(meta.0, Some("openai/gpt-4o".to_string()));
        assert_eq!(meta.1, Some("high".to_string()));

        let list = db.list_chat_sessions_meta("note-a").unwrap();
        assert!(list.iter().any(|s| s.session_id == "main"));
        let u100 = list.iter().find(|s| s.session_id == "user:U100").unwrap();
        assert_eq!(u100.model, Some("openai/gpt-4o".to_string()));
        assert_eq!(u100.thinking, Some("high".to_string()));
    }

    #[tokio::test]
    async fn test_chat_sessions_title_and_custom_override() {
        let db = in_memory_db();
        // 1. Auto-sync title from LINE
        db.set_session_title("note-a", "group:C123", Some("DevOps Group"))
            .unwrap();
        let list = db.list_chat_sessions_meta("note-a").unwrap();
        let c123 = list.iter().find(|s| s.session_id == "group:C123").unwrap();
        assert_eq!(c123.title, Some("DevOps Group".to_string()));
        assert_eq!(c123.custom_title, None);

        // 2. User sets custom title
        db.set_session_custom_title("note-a", "group:C123", Some("War Room 2026"))
            .unwrap();
        let list = db.list_chat_sessions_meta("note-a").unwrap();
        let c123 = list.iter().find(|s| s.session_id == "group:C123").unwrap();
        assert_eq!(c123.title, Some("DevOps Group".to_string()));
        assert_eq!(c123.custom_title, Some("War Room 2026".to_string()));

        // 3. Auto-sync updates default title without touching custom_title
        db.set_session_title("note-a", "group:C123", Some("New LINE Group Name"))
            .unwrap();
        let list = db.list_chat_sessions_meta("note-a").unwrap();
        let c123 = list.iter().find(|s| s.session_id == "group:C123").unwrap();
        assert_eq!(c123.title, Some("New LINE Group Name".to_string()));
        assert_eq!(c123.custom_title, Some("War Room 2026".to_string()));

        // 4. User clears custom title -> custom_title becomes None, title is still preserved!
        db.set_session_custom_title("note-a", "group:C123", None)
            .unwrap();
        let list = db.list_chat_sessions_meta("note-a").unwrap();
        let c123 = list.iter().find(|s| s.session_id == "group:C123").unwrap();
        assert_eq!(c123.title, Some("New LINE Group Name".to_string()));
        assert_eq!(c123.custom_title, None);
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
