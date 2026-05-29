//! SQLite persistence for chat history.
//!
//! Schema:
//!   messages(id, note_id, role, nickname, content, created_at)
//!
//! All blocking SQLite calls are wrapped in tokio::task::spawn_blocking.

use rusqlite::{params, Connection, backup::Backup};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::warn;

/// A single stored chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRecord {
    pub id: i64,
    pub note_id: String,
    pub role: String,    // "user" | "assistant" | "system"
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
}

/// A stored session entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteRecord {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

/// Thread-safe SQLite connection wrapper.
#[derive(Clone)]
pub struct ChatDb {
    conn: Arc<Mutex<Connection>>,
    /// If Some, DB is in-memory and should be persisted to this path on first write.
    deferred_path: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl ChatDb {
    /// Open (or create) the chat database at the given path.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS messages (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                note_id  TEXT    NOT NULL DEFAULT 'default',
                role        TEXT    NOT NULL,
                nickname    TEXT    NOT NULL,
                content     TEXT    NOT NULL,
                created_at  INTEGER NOT NULL,
                model       TEXT,
                tokens_in   INTEGER,
                tokens_out  INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session
                ON messages(note_id, id);
            CREATE TABLE IF NOT EXISTS sessions (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                created_at  INTEGER NOT NULL,
                created_by  TEXT
            );
        ")?;
        // Migrate existing DBs that lack the new columns (idempotent)
        let _ = conn.execute_batch("
            ALTER TABLE messages ADD COLUMN model      TEXT;
            ALTER TABLE messages ADD COLUMN tokens_in  INTEGER;
            ALTER TABLE messages ADD COLUMN tokens_out INTEGER;
        ");
        // Migration: drop workspace column if it exists
        let _ = conn.execute_batch("
            ALTER TABLE sessions DROP COLUMN workspace;
        ");
        // Migration: rename session_id -> note_id in messages table
        let _ = conn.execute_batch("
            ALTER TABLE messages RENAME COLUMN session_id TO note_id;
        ");
        Ok(Self { conn: Arc::new(Mutex::new(conn)), deferred_path: Arc::new(Mutex::new(None)) })
    }

    /// Open file if it exists, otherwise in-memory with deferred persistence.
    pub fn open_lazy(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Self::open(path)
        } else {
            let conn = Connection::open_in_memory()?;
            conn.execute_batch("
                PRAGMA journal_mode=WAL;
                PRAGMA synchronous=NORMAL;
                CREATE TABLE IF NOT EXISTS messages (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    note_id  TEXT    NOT NULL DEFAULT 'default',
                    role        TEXT    NOT NULL,
                    nickname    TEXT    NOT NULL,
                    content     TEXT    NOT NULL,
                    created_at  INTEGER NOT NULL,
                    model       TEXT,
                    tokens_in   INTEGER,
                    tokens_out  INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_messages_session
                    ON messages(note_id, id);
                CREATE TABLE IF NOT EXISTS sessions (
                    id          TEXT PRIMARY KEY,
                    name        TEXT NOT NULL,
                        created_at  INTEGER NOT NULL,
                    created_by  TEXT
                );
            ")?;
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
        backup_conn.execute_batch("
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
        ")?;
        let backup = Backup::new(&*conn, &mut backup_conn)?;
        backup.run_to_completion(100, std::time::Duration::from_millis(10), None)?;
        // Replace connection with file-based one
        drop(backup);
        drop(conn);
        // Reopen from file and swap
        let new_conn = Connection::open(&path)?;
        new_conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let mut conn_guard = self.conn.lock().unwrap();
        *conn_guard = new_conn;
        Ok(())
    }

    /// Returns true if currently in-memory (not yet persisted to file).
    pub fn is_memory(&self) -> bool {
        self.deferred_path.lock().unwrap().is_some()
    }

    /// Insert a message. Returns the new row id.
    pub fn insert(&self, note_id: &str, role: &str, nickname: &str, content: &str) -> anyhow::Result<i64> {
        self.insert_with_meta(note_id, role, nickname, content, None, None, None)
    }

    /// Insert a message with optional model/token metadata.
    pub fn insert_with_meta(
        &self, note_id: &str, role: &str, nickname: &str, content: &str,
        model: Option<&str>, tokens_in: Option<i32>, tokens_out: Option<i32>,
    ) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO messages (note_id, role, nickname, content, created_at, model, tokens_in, tokens_out)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![note_id, role, nickname, content, ts, model, tokens_in, tokens_out],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Load the last `limit` messages for a session (ordered oldest first).
    pub fn load_recent(&self, note_id: &str, limit: usize) -> anyhow::Result<Vec<ChatRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, note_id, role, nickname, content, created_at, model, tokens_in, tokens_out
             FROM messages
             WHERE note_id = ?1
             ORDER BY id DESC
             LIMIT ?2"
        )?;
        let rows: Vec<ChatRecord> = stmt.query_map(params![note_id, limit as i64], |row| {
            Ok(ChatRecord {
                id:         row.get(0)?,
                note_id: row.get(1)?,
                role:       row.get(2)?,
                nickname:   row.get(3)?,
                content:    row.get(4)?,
                created_at: row.get(5)?,
                model:      row.get(6)?,
                tokens_in:  row.get(7)?,
                tokens_out: row.get(8)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
        // Reverse so oldest first
        let mut rows = rows;
        rows.reverse();
        Ok(rows)
    }

    /// Async wrapper for insert (runs on blocking thread pool).
    pub async fn insert_async(&self, note_id: String, role: String, nickname: String, content: String) {
        self.insert_with_meta_async(note_id, role, nickname, content, None, None, None).await;
    }

    /// Async wrapper for insert_with_meta.
    pub async fn insert_with_meta_async(
        &self,
        note_id: String, role: String, nickname: String, content: String,
        model: Option<String>, tokens_in: Option<i32>, tokens_out: Option<i32>,
    ) {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.insert_with_meta(
                &note_id, &role, &nickname, &content,
                model.as_deref(), tokens_in, tokens_out,
            ) {
                warn!("Failed to persist chat message: {}", e);
            }
        }).await.ok();
    }

    /// Async wrapper for load_recent.
    pub async fn load_recent_async(&self, note_id: String, limit: usize) -> Vec<ChatRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.load_recent(&note_id, limit).unwrap_or_default()
        }).await.unwrap_or_default()
    }

    // ── Session CRUD ──────────────────────────────────────────────────────

    /// Create a new session. Returns Ok(()) or error if id already exists.
    pub fn create_note(&self, id: &str, name: &str, created_by: Option<&str>) -> anyhow::Result<()> {
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
            "SELECT id, name, created_at, created_by FROM sessions ORDER BY name ASC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(NoteRecord {
                id:         row.get(0)?,
                name:       row.get(1)?,
                created_at: row.get(2)?,
                created_by: row.get(3)?,
            })
        })?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }

    /// Rename a session (updates both id and name, since id = name).
    /// Also updates note_id in messages table.
    /// Returns the new id if successful, None if not found or conflict.
    pub fn rename_note(&self, id: &str, new_name: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        // Check if new id already exists
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            params![new_name],
            |row| row.get::<_, i64>(0),
        )? > 0;
        if exists && new_name != id {
            return Ok(None); // conflict
        }
        // Update session row (id + name)
        let changed = conn.execute(
            "UPDATE sessions SET id = ?1, name = ?1 WHERE id = ?2",
            params![new_name, id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        // Update messages to new note_id
        conn.execute(
            "UPDATE messages SET note_id = ?1 WHERE note_id = ?2",
            params![new_name, id],
        )?;
        Ok(Some(new_name.to_string()))
    }


    /// Delete a session (metadata only; does NOT delete chat messages).
    pub fn delete_note(&self, id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "DELETE FROM sessions WHERE id = ?1",
            params![id],
        )?;
        Ok(changed > 0)
    }

    /// Get a single session by id.
    pub fn get_session(&self, id: &str) -> anyhow::Result<Option<NoteRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, created_by FROM sessions WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(NoteRecord {
                id:         row.get(0)?,
                name:       row.get(1)?,
                created_at: row.get(2)?,
                created_by: row.get(3)?,
            })
        })?;
        Ok(rows.next().and_then(|r| r.ok()))
    }
    // ── End Session CRUD ─────────────────────────────────────────────────

    /// Dump all messages for a session to JSONL, then delete them from the DB.
    /// Returns the number of messages archived.
    pub fn archive(&self, note_id: &str, archive_path: &Path) -> anyhow::Result<usize> {
        use std::io::Write;
        let conn = self.conn.lock().unwrap();
        // Load all messages for this session
        let mut stmt = conn.prepare(
            "SELECT id, note_id, role, nickname, content, created_at, model, tokens_in, tokens_out
             FROM messages WHERE note_id = ?1 ORDER BY id ASC"
        )?;
        let records: Vec<ChatRecord> = stmt.query_map(params![note_id], |row| {
            Ok(ChatRecord {
                id:         row.get(0)?,
                note_id: row.get(1)?,
                role:       row.get(2)?,
                nickname:   row.get(3)?,
                content:    row.get(4)?,
                created_at: row.get(5)?,
                model:      row.get(6)?,
                tokens_in:  row.get(7)?,
                tokens_out: row.get(8)?,
            })
        })?.filter_map(|r| r.ok()).collect();

        if records.is_empty() {
            return Ok(0);
        }

        // Write JSONL
        if let Some(parent) = archive_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(archive_path)?;
        for rec in &records {
            let line = serde_json::to_string(rec)?;
            writeln!(file, "{}", line)?;
        }

        // Delete archived messages from DB
        conn.execute(
            "DELETE FROM messages WHERE note_id = ?1",
            params![note_id],
        )?;

        Ok(records.len())
    }

    /// Full-text search across current DB + all JSONL archive files in archive_dir.
    /// Returns matching records sorted oldest first (archives first, then live).
    pub fn search(&self, note_id: &str, query: &str, archive_dir: &Path) -> anyhow::Result<Vec<ChatRecord>> {
        let query_lower = query.to_lowercase();
        let mut results: Vec<ChatRecord> = Vec::new();

        // 1. Search archive JSONL files
        if archive_dir.exists() {
            let mut entries: Vec<_> = std::fs::read_dir(archive_dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "jsonl").unwrap_or(false))
                .collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    for line in text.lines() {
                        if let Ok(rec) = serde_json::from_str::<ChatRecord>(line) {
                            if rec.note_id == note_id
                                && (rec.content.to_lowercase().contains(&query_lower)
                                    || rec.nickname.to_lowercase().contains(&query_lower))
                            {
                                results.push(rec);
                            }
                        }
                    }
                }
            }
        }

        // 2. Search live DB
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, note_id, role, nickname, content, created_at, model, tokens_in, tokens_out
             FROM messages WHERE note_id = ?1 ORDER BY id ASC"
        )?;
        let live: Vec<ChatRecord> = stmt.query_map(params![note_id], |row| {
            Ok(ChatRecord {
                id:         row.get(0)?,
                note_id: row.get(1)?,
                role:       row.get(2)?,
                nickname:   row.get(3)?,
                content:    row.get(4)?,
                created_at: row.get(5)?,
                model:      row.get(6)?,
                tokens_in:  row.get(7)?,
                tokens_out: row.get(8)?,
            })
        })?.filter_map(|r| r.ok())
          .filter(|r| r.content.to_lowercase().contains(&query_lower)
                   || r.nickname.to_lowercase().contains(&query_lower))
          .collect();
        results.extend(live);

        Ok(results)
    }

    /// Async wrapper for archive.
    pub async fn archive_async(&self, note_id: String, archive_path: std::path::PathBuf) -> anyhow::Result<usize> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.archive(&note_id, &archive_path)
        }).await?
    }

    /// Async wrapper for search.
    pub async fn search_async(&self, note_id: String, query: String, archive_dir: std::path::PathBuf) -> Vec<ChatRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.search(&note_id, &query, &archive_dir).unwrap_or_default()
        }).await.unwrap_or_default()
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
        db.insert("default", "assistant", "ᚱᚢᚾᛖ", "hi there").unwrap();
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
            db.insert("default", "user", "bob", &format!("msg {}", i)).unwrap();
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
        db.insert("room-a", "user", "alice", "hello room a").unwrap();
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
        assert!(!archive_path.exists(), "No file should be created for empty archive");
    }

    #[test]
    fn test_search_live_messages() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        let db = ChatDb::open(&db_path).unwrap();
        db.insert("default", "user", "alice", "hello world").unwrap();
        db.insert("default", "assistant", "ᚱᚢᚾᛖ", "goodbye").unwrap();
        db.insert("default", "user", "alice", "hello again").unwrap();

        let arc_dir = dir.path().join("archives");
        let results = db.search("default", "hello", &arc_dir).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.content.contains("hello")));
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
        let old_rec = ChatRecord { id: 1, note_id: "default".into(), role: "user".into(),
            nickname: "bob".into(), content: "search me".into(), created_at: 1000,
            model: None, tokens_in: None, tokens_out: None };
        let mut f = std::fs::File::create(&arc_path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&old_rec).unwrap()).unwrap();

        // Live DB also has one match
        db.insert("default", "user", "alice", "search me too").unwrap();
        db.insert("default", "user", "alice", "nothing here").unwrap();

        let results = db.search("default", "search me", &arc_dir).unwrap();
        assert_eq!(results.len(), 2, "Should find 1 archive + 1 live result");
    }

    #[tokio::test]
    async fn test_insert_async_and_load_async() {
        let db = in_memory_db();
        db.insert_async("default".into(), "user".into(), "alice".into(), "async msg".into()).await;
        let rows = db.load_recent_async("default".into(), 10).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "async msg");
    }

    #[test]
    fn test_insert_with_meta_persists_model_tokens() {
        let db = in_memory_db();
        db.insert_with_meta("default", "assistant", "ᚱᚢᚾᛖ", "hello", Some("gpt-5-mini"), Some(100), Some(42)).unwrap();
        let rows = db.load_recent("default", 1).unwrap();
        assert_eq!(rows[0].model.as_deref(), Some("gpt-5-mini"));
        assert_eq!(rows[0].tokens_in,  Some(100));
        assert_eq!(rows[0].tokens_out, Some(42));
    }

    #[test]
    fn test_insert_without_meta_has_none_fields() {
        let db = in_memory_db();
        db.insert("default", "user", "alice", "hi").unwrap();
        let rows = db.load_recent("default", 1).unwrap();
        assert!(rows[0].model.is_none());
        assert!(rows[0].tokens_in.is_none());
        assert!(rows[0].tokens_out.is_none());
    }

    #[test]
    fn test_archive_preserves_meta_in_jsonl() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.db");
        let db = ChatDb::open(&db_path).unwrap();
        db.insert_with_meta("default", "assistant", "ᚱᚢᚾᛖ", "reply", Some("gpt-4o"), Some(50), Some(25)).unwrap();
        let archive_path = dir.path().join("arc.jsonl");
        db.archive("default", &archive_path).unwrap();
        let file = std::fs::File::open(&archive_path).unwrap();
        let line = std::io::BufReader::new(file).lines().next().unwrap().unwrap();
        let rec: ChatRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(rec.model.as_deref(), Some("gpt-4o"));
        assert_eq!(rec.tokens_in,  Some(50));
        assert_eq!(rec.tokens_out, Some(25));
    }

    // ── Session CRUD tests ───────────────────────────────────────────────

    #[test]
    fn test_create_and_list_notes() {
        let db = in_memory_db();
        db.create_note("proj-a", "Project A", Some("admin")).unwrap();
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
        let result = db.rename_note("s1", "new-name").unwrap();
        assert_eq!(result, Some("new-name".to_string()));
        // Old id gone, new id exists
        assert!(db.get_session("s1").unwrap().is_none());
        let s = db.get_session("new-name").unwrap().unwrap();
        assert_eq!(s.name, "new-name");
        assert_eq!(s.id, "new-name");
    }

    #[test]
    fn test_rename_note_updates_messages() {
        let db = in_memory_db();
        db.create_note("old", "old", None).unwrap();
        db.insert("old", "user", "alice", "hello").unwrap();
        let _ = db.rename_note("old", "new").unwrap();
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
        assert_eq!(db.rename_note("nope", "X").unwrap(), None);
    }

    #[test]
    fn test_rename_note_conflict() {
        let db = in_memory_db();
        db.create_note("a", "a", None).unwrap();
        db.create_note("b", "b", None).unwrap();
        // Can't rename a → b (b already exists)
        assert_eq!(db.rename_note("a", "b").unwrap(), None);
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
}
