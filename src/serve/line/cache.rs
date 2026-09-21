use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Clone)]
struct CachedProfile {
    display_name: String,
    fetched_at: Instant,
}

/// In-memory cache for LINE user profile display names with time-to-live (TTL).
#[derive(Clone)]
pub struct ProfileCache {
    entries: Arc<RwLock<HashMap<String, CachedProfile>>>,
    ttl: Duration,
}

impl Default for ProfileCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(86400)) // 24 hours default TTL
    }
}

impl ProfileCache {
    /// Create a new cache with a specified TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Retrieve cached display name if not expired.
    pub async fn get(&self, user_id: &str) -> Option<String> {
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(user_id) {
            if entry.fetched_at.elapsed() < self.ttl {
                return Some(entry.display_name.clone());
            }
        }
        None
    }

    /// Store or update display name in cache.
    pub async fn insert(&self, user_id: String, display_name: String) {
        let mut entries = self.entries.write().await;
        entries.insert(
            user_id,
            CachedProfile {
                display_name,
                fetched_at: Instant::now(),
            },
        );
    }

    /// Format nickname as `line:<displayName>` or fallback to `line:<userId prefix>`.
    pub fn format_nickname(display_name: Option<&str>, user_id: &str) -> String {
        match display_name {
            Some(name) if !name.trim().is_empty() => format!("line:{}", name.trim()),
            _ => {
                let prefix: String = user_id.chars().take(8).collect();
                format!("line:{}", prefix)
            }
        }
    }

    /// Format nickname with bot identifier as `line:<bot>:<displayName>` or fallback to `line:<bot>:<userId prefix>`.
    pub fn format_nickname_for_bot(
        bot_nickname: &str,
        display_name: Option<&str>,
        user_id: &str,
    ) -> String {
        let name_part = match display_name {
            Some(name) if !name.trim().is_empty() => name.trim().to_string(),
            _ => {
                let prefix: String = user_id.chars().take(8).collect();
                prefix
            }
        };
        let bot_trimmed = bot_nickname.trim();
        if bot_trimmed.is_empty() {
            format!("line:{}", name_part)
        } else {
            format!("line:{}:{}", bot_trimmed, name_part)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_profile_cache_hit_and_expiry() {
        let cache = ProfileCache::new(Duration::from_millis(50));
        cache
            .insert("U12345".to_string(), "Alice".to_string())
            .await;

        assert_eq!(cache.get("U12345").await, Some("Alice".to_string()));
        assert_eq!(cache.get("U99999").await, None);

        tokio::time::sleep(Duration::from_millis(70)).await;
        assert_eq!(cache.get("U12345").await, None);
    }

    #[test]
    fn test_format_nickname() {
        assert_eq!(
            ProfileCache::format_nickname(Some("Alice"), "U123456789"),
            "line:Alice"
        );
        assert_eq!(
            ProfileCache::format_nickname(Some("  Bob  "), "U123456789"),
            "line:Bob"
        );
        assert_eq!(
            ProfileCache::format_nickname(None, "U123456789"),
            "line:U1234567"
        );
        assert_eq!(
            ProfileCache::format_nickname(Some(""), "U123456789"),
            "line:U1234567"
        );
    }

    #[test]
    fn test_format_nickname_for_bot() {
        assert_eq!(
            ProfileCache::format_nickname_for_bot("LineBot", Some("Alice"), "U123456789"),
            "line:LineBot:Alice"
        );
        assert_eq!(
            ProfileCache::format_nickname_for_bot("CIBot", None, "U123456789"),
            "line:CIBot:U1234567"
        );
        assert_eq!(
            ProfileCache::format_nickname_for_bot("", Some("Alice"), "U123456789"),
            "line:Alice"
        );
    }
}
