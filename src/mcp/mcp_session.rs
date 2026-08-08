use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Legacy MCP protocol versions that get a session-id + GET-stream compatibility path.
pub const LEGACY_SESSION_PROTOCOL_VERSIONS: &[&str] = &["2025-03-26", "2025-06-18", "2025-11-25"];

/// Default session TTL: 45 minutes of inactivity.
const SESSION_TTL: Duration = Duration::from_secs(45 * 60);

/// Returns true if `version` is one of the legacy protocol versions that should
/// get a `Mcp-Session-Id` + GET stream compatibility path (2025-03-26 ~ 2025-11-25).
pub fn is_legacy_session_protocol_version(version: &str) -> bool {
    LEGACY_SESSION_PROTOCOL_VERSIONS.contains(&version)
}

#[derive(Debug, Clone)]
pub struct McpSession {
    pub protocol_version: String,
    pub created_at: Instant,
    pub last_seen_at: Instant,
    /// Whether a GET SSE stream is currently open for this session.
    pub sse_open: bool,
}

impl McpSession {
    fn new(protocol_version: String) -> Self {
        let now = Instant::now();
        Self {
            protocol_version,
            created_at: now,
            last_seen_at: now,
            sse_open: false,
        }
    }

    fn is_expired(&self) -> bool {
        self.last_seen_at.elapsed() > SESSION_TTL
    }
}

/// In-memory store for legacy MCP sessions (protocol versions 2025-03-26 ~ 2025-11-25).
///
/// NOTE: single-process in-memory store. If rune-notes is ever deployed behind a
/// load balancer with multiple replicas, this store must be backed by a shared
/// store (e.g. Redis) instead, or GET/DELETE requests routed to a different
/// replica than the one that handled `initialize` will fail to find the session.
#[derive(Clone)]
pub struct McpSessionStore {
    inner: Arc<RwLock<HashMap<String, McpSession>>>,
}

impl McpSessionStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new session for the given (legacy) protocol version. Returns the new session id.
    pub async fn create(&self, protocol_version: &str) -> String {
        let id = crate::serve::oauth::generate_session_id();
        self.inner
            .write()
            .await
            .insert(id.clone(), McpSession::new(protocol_version.to_string()));
        id
    }

    /// Fetch a non-expired session by id, refreshing `last_seen_at`.
    pub async fn touch(&self, id: &str) -> Option<McpSession> {
        let mut map = self.inner.write().await;
        let expired = map.get(id).map(|s| s.is_expired()).unwrap_or(true);
        if expired {
            map.remove(id);
            return None;
        }
        if let Some(s) = map.get_mut(id) {
            s.last_seen_at = Instant::now();
            return Some(s.clone());
        }
        None
    }

    /// Fetch a non-expired session by id without updating `last_seen_at`.
    pub async fn get(&self, id: &str) -> Option<McpSession> {
        let map = self.inner.read().await;
        map.get(id).and_then(|s| {
            if s.is_expired() {
                None
            } else {
                Some(s.clone())
            }
        })
    }

    /// Mark whether an SSE GET stream is open for this session.
    pub async fn set_sse_open(&self, id: &str, open: bool) {
        if let Some(s) = self.inner.write().await.get_mut(id) {
            s.sse_open = open;
        }
    }

    /// Remove a session (e.g. on DELETE). Returns true if it existed.
    pub async fn remove(&self, id: &str) -> bool {
        self.inner.write().await.remove(id).is_some()
    }

    /// Remove all expired sessions. Intended to be called periodically.
    pub async fn sweep_expired(&self) {
        self.inner.write().await.retain(|_, s| !s.is_expired());
    }

    #[cfg(test)]
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }
}

impl Default for McpSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_legacy_session_protocol_version() {
        assert!(is_legacy_session_protocol_version("2025-03-26"));
        assert!(is_legacy_session_protocol_version("2025-06-18"));
        assert!(is_legacy_session_protocol_version("2025-11-25"));
        assert!(!is_legacy_session_protocol_version("2026-07-28"));
        assert!(!is_legacy_session_protocol_version("2024-11-05"));
        assert!(!is_legacy_session_protocol_version("bogus"));
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let store = McpSessionStore::new();
        let id = store.create("2025-11-25").await;
        assert_eq!(store.len().await, 1);

        let session = store.get(&id).await.expect("session should exist");
        assert_eq!(session.protocol_version, "2025-11-25");
        assert!(!session.sse_open);
    }

    #[tokio::test]
    async fn test_get_unknown_session_returns_none() {
        let store = McpSessionStore::new();
        assert!(store.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_touch_updates_last_seen() {
        let store = McpSessionStore::new();
        let id = store.create("2025-11-25").await;
        let first = store.get(&id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let touched = store.touch(&id).await.unwrap();
        assert!(touched.last_seen_at >= first.last_seen_at);
    }

    #[tokio::test]
    async fn test_touch_unknown_session_returns_none() {
        let store = McpSessionStore::new();
        assert!(store.touch("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_set_sse_open() {
        let store = McpSessionStore::new();
        let id = store.create("2025-11-25").await;
        store.set_sse_open(&id, true).await;
        let session = store.get(&id).await.unwrap();
        assert!(session.sse_open);

        store.set_sse_open(&id, false).await;
        let session = store.get(&id).await.unwrap();
        assert!(!session.sse_open);
    }

    #[tokio::test]
    async fn test_remove_session() {
        let store = McpSessionStore::new();
        let id = store.create("2025-11-25").await;
        assert!(store.remove(&id).await);
        assert!(store.get(&id).await.is_none());
        // Removing again returns false
        assert!(!store.remove(&id).await);
    }

    #[tokio::test]
    async fn test_sweep_expired_removes_only_expired() {
        let store = McpSessionStore::new();
        let id = store.create("2025-11-25").await;

        // Manually mark session as expired by mutating internal state.
        {
            let mut map = store.inner.write().await;
            let s = map.get_mut(&id).unwrap();
            s.last_seen_at = Instant::now() - Duration::from_secs(46 * 60);
        }

        store.sweep_expired().await;
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn test_sweep_expired_keeps_fresh_sessions() {
        let store = McpSessionStore::new();
        let _id = store.create("2025-11-25").await;
        store.sweep_expired().await;
        assert_eq!(store.len().await, 1);
    }

    #[tokio::test]
    async fn test_multiple_sessions_independent() {
        let store = McpSessionStore::new();
        let id1 = store.create("2025-03-26").await;
        let id2 = store.create("2025-11-25").await;
        assert_ne!(id1, id2);

        store.set_sse_open(&id1, true).await;
        let s1 = store.get(&id1).await.unwrap();
        let s2 = store.get(&id2).await.unwrap();
        assert!(s1.sse_open);
        assert!(!s2.sse_open);
    }
}
