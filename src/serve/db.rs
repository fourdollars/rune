//! SQLite persistence for chat history.
//!
//! Schema:
//!   messages(id, session_id, role, nickname, content, created_at)
//!
//! All blocking SQLite calls are wrapped in tokio::task::spawn_blocking.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::warn;

/// A single stored chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRecord {
    pub id: i64,
    pub session_id: String,
    pub role: String,    // "user" | "assistant" | "system"
    pub nickname: String, // user nickname, or "ᚱᚢᚾᛖ" for assistant
    pub content: String,
    pub created_at: i64, // unix timestamp (seconds)
}

/// Thread-safe SQLite connection wrapper.
#[derive(Clone)]
pub struct ChatDb {
    conn: Arc<Mutex<Connection>>,
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
                session_id  TEXT    NOT NULL DEFAULT 'default',
                role        TEXT    NOT NULL,
                nickname    TEXT    NOT NULL,
                content     TEXT    NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session
                ON messages(session_id, id);
        ")?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// Insert a message. Returns the new row id.
    pub fn insert(&self, session_id: &str, role: &str, nickname: &str, content: &str) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO messages (session_id, role, nickname, content, created_at) VALUES (?1,?2,?3,?4,?5)",
            params![session_id, role, nickname, content, ts],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Load the last `limit` messages for a session (ordered oldest first).
    pub fn load_recent(&self, session_id: &str, limit: usize) -> anyhow::Result<Vec<ChatRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, nickname, content, created_at
             FROM messages
             WHERE session_id = ?1
             ORDER BY id DESC
             LIMIT ?2"
        )?;
        let rows: Vec<ChatRecord> = stmt.query_map(params![session_id, limit as i64], |row| {
            Ok(ChatRecord {
                id:         row.get(0)?,
                session_id: row.get(1)?,
                role:       row.get(2)?,
                nickname:   row.get(3)?,
                content:    row.get(4)?,
                created_at: row.get(5)?,
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
    pub async fn insert_async(&self, session_id: String, role: String, nickname: String, content: String) {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.insert(&session_id, &role, &nickname, &content) {
                warn!("Failed to persist chat message: {}", e);
            }
        }).await.ok();
    }

    /// Async wrapper for load_recent.
    pub async fn load_recent_async(&self, session_id: String, limit: usize) -> Vec<ChatRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            db.load_recent(&session_id, limit).unwrap_or_default()
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

    #[tokio::test]
    async fn test_insert_async_and_load_async() {
        let db = in_memory_db();
        db.insert_async("default".into(), "user".into(), "alice".into(), "async msg".into()).await;
        let rows = db.load_recent_async("default".into(), 10).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "async msg");
    }
}
