//! Policy, Security, Deny-List, and Rate Limiting for Rune Vim integration.

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Security Policy Checker for Vim Buffers (§9).
#[derive(Debug)]
pub struct VimPolicy {
    disabled_filetypes: HashSet<String>,
}

impl Default for VimPolicy {
    fn default() -> Self {
        let mut disabled = HashSet::new();
        for ft in &["gitcommit", "gitrebase", "fugitive", "netrw", "help"] {
            disabled.insert((*ft).to_string());
        }
        Self {
            disabled_filetypes: disabled,
        }
    }
}

impl VimPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if the file or path is denied by security rules or filetype filters.
    pub fn is_excluded(&self, filepath: &str, filetype: &str) -> bool {
        if self.disabled_filetypes.contains(&filetype.to_lowercase()) {
            return true;
        }

        let path = Path::new(filepath);
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Hard-deny list rules (§9)
        if file_name.starts_with(".env")
            || file_name.ends_with(".pem")
            || file_name.ends_with(".key")
            || file_name.starts_with("id_rsa")
            || file_name.contains("credentials")
            || file_name.contains("secret")
            || file_name == ".netrc"
            || file_name == ".npmrc"
        {
            return true;
        }

        // Parent path components check (e.g. .aws/*)
        for component in path.components() {
            let comp_str = component.as_os_str().to_string_lossy().to_lowercase();
            if comp_str == ".aws" || comp_str == ".ssh" {
                return true;
            }
        }

        false
    }
}

/// Token Bucket Rate Limiter (§9.1).
/// Default: capacity = 15, refill = 30 tokens / min (0.5 tokens/sec).
#[derive(Debug)]
pub struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_rate_per_sec: f64,
    last_refill: Instant,
}

impl Default for TokenBucket {
    fn default() -> Self {
        Self::new(15.0, 30.0)
    }
}

impl TokenBucket {
    pub fn new(capacity: f64, refill_per_min: f64) -> Self {
        Self {
            capacity,
            tokens: capacity,
            refill_rate_per_sec: refill_per_min / 60.0,
            last_refill: Instant::now(),
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;

        self.tokens = (self.tokens + elapsed * self.refill_rate_per_sec).min(self.capacity);
    }

    /// Attempt to consume 1 token. Returns true if allowed, false if rate-limited.
    pub fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Refund 1 token (used on cancelled requests §5.3 / §9.1).
    pub fn refund(&mut self) {
        self.refill();
        self.tokens = (self.tokens + 1.0).min(self.capacity);
    }

    /// Remaining available tokens and capacity.
    pub fn budget(&mut self) -> (u32, u32) {
        self.refill();
        (self.tokens.floor() as u32, self.capacity as u32)
    }

    pub fn time_until_next_token(&mut self) -> Duration {
        self.refill();
        if self.tokens >= 1.0 {
            Duration::from_secs(0)
        } else {
            let needed = 1.0 - self.tokens;
            let secs = needed / self.refill_rate_per_sec;
            Duration::from_secs_f64(secs)
        }
    }
}

pub struct SharedRateLimiter {
    inner: Mutex<TokenBucket>,
}

impl Default for SharedRateLimiter {
    fn default() -> Self {
        Self {
            inner: Mutex::new(TokenBucket::default()),
        }
    }
}

impl SharedRateLimiter {
    pub fn try_consume(&self) -> bool {
        self.inner.lock().unwrap().try_consume()
    }

    pub fn refund(&self) {
        self.inner.lock().unwrap().refund();
    }

    pub fn budget(&self) -> (u32, u32) {
        self.inner.lock().unwrap().budget()
    }

    pub fn refill_in_ms(&self) -> u64 {
        self.inner
            .lock()
            .unwrap()
            .time_until_next_token()
            .as_millis() as u64
    }
}

/// Redact text content for logging (§9.2).
pub fn redact_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let hash = hex::encode(hasher.finalize());
    let short_hash = if hash.len() > 8 { &hash[..8] } else { &hash };
    format!("<redacted: {} bytes, sha256:{}…>", text.len(), short_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_deny_list() {
        let policy = VimPolicy::new();
        assert!(policy.is_excluded(".env", "rust"));
        assert!(policy.is_excluded("/path/to/server.key", "rust"));
        assert!(policy.is_excluded("/path/to/id_rsa", "rust"));
        assert!(policy.is_excluded("/home/user/.aws/credentials", "rust"));
        assert!(policy.is_excluded("commit.txt", "gitcommit"));
        assert!(!policy.is_excluded("src/main.rs", "rust"));
    }

    #[test]
    fn test_token_bucket() {
        let mut bucket = TokenBucket::new(2.0, 60.0);
        assert!(bucket.try_consume());
        assert!(bucket.try_consume());
        assert!(!bucket.try_consume()); // Empty
        bucket.refund();
        assert!(bucket.try_consume());
    }

    #[test]
    fn test_redact_text() {
        let redacted = redact_text("secret_code");
        assert!(redacted.contains("redacted: 11 bytes"));
    }
}
