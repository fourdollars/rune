use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Embedding configuration from rune.toml [embedding] section.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    /// Maximum number of skills to inject via semantic search (default: 3).
    #[serde(default = "default_max_skills")]
    pub max_skills: usize,
}

fn default_true() -> bool {
    true
}

fn default_threshold() -> f32 {
    0.3
}

fn default_max_skills() -> usize {
    3
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: None,
            base_url: None,
            api_key: None,
            threshold: 0.3,
            max_skills: 3,
        }
    }
}

/// A single embedding entry in the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorEntry {
    pub key: String,
    pub text: String,
    pub vector: Vec<f32>,
    pub updated_at: u64,
}

/// The vector store persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorStore {
    pub model: String,
    pub dimensions: usize,
    pub entries: Vec<VectorEntry>,
}

impl VectorStore {
    /// Load from a JSON file, or return empty store.
    pub fn load(path: &PathBuf) -> Self {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(store) = serde_json::from_str(&content) {
                    return store;
                }
            }
        }
        Self {
            model: String::new(),
            dimensions: 0,
            entries: Vec::new(),
        }
    }

    /// Save to a JSON file.
    pub fn save(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Find entries with cosine similarity above threshold.
    pub fn search(
        &self,
        query_vector: &[f32],
        threshold: f32,
        max_results: usize,
    ) -> Vec<(f32, &VectorEntry)> {
        let mut results: Vec<(f32, &VectorEntry)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let sim = cosine_similarity(query_vector, &entry.vector);
                if sim >= threshold {
                    Some((sim, entry))
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(max_results);
        results
    }

    /// Check if store model matches the configured model (invalidation check).
    pub fn is_valid_for_model(&self, model: &str) -> bool {
        !self.model.is_empty() && self.model == model
    }

    /// Update or insert an entry by key.
    pub fn upsert(&mut self, key: String, text: String, vector: Vec<f32>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if let Some(existing) = self.entries.iter_mut().find(|e| e.key == key) {
            existing.text = text;
            existing.vector = vector;
            existing.updated_at = now;
        } else {
            self.entries.push(VectorEntry {
                key,
                text,
                vector,
                updated_at: now,
            });
        }
    }

    /// Remove an entry by key.
    pub fn remove(&mut self, key: &str) {
        self.entries.retain(|e| e.key != key);
    }
}

/// Cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    dot / (mag_a * mag_b)
}

/// Embedding provider — calls /v1/embeddings endpoint.
/// Supports Copilot token refresh when copilot_pat is set.
pub struct EmbeddingEngine {
    pub config: EmbeddingConfig,
    client: reqwest::Client,
    /// If set, this is a GitHub Copilot PAT that needs token refresh.
    copilot_pat: Option<String>,
    /// Cached Copilot session token.
    copilot_token: std::sync::Arc<tokio::sync::Mutex<Option<(String, u64)>>>,
}

impl EmbeddingEngine {
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            copilot_pat: None,
            copilot_token: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Create an embedding engine configured for GitHub Copilot.
    pub fn new_copilot(config: EmbeddingConfig, pat: String) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            copilot_pat: Some(pat),
            copilot_token: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Get a valid Bearer token (refreshing Copilot token if needed).
    async fn get_bearer_token(&self) -> Result<String> {
        if let Some(ref pat) = self.copilot_pat {
            // Check cached token
            let mut cache = self.copilot_token.lock().await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if let Some((ref token, expires)) = *cache {
                if now < expires.saturating_sub(60) {
                    return Ok(token.clone());
                }
            }
            // Refresh token
            let resp = self
                .client
                .get("https://api.github.com/copilot_internal/v2/token")
                .header("Authorization", format!("token {}", pat))
                .header("User-Agent", "rune/0.1.0")
                .header("editor-version", "vscode/1.96.0")
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await?;
            if !resp.status().is_success() {
                anyhow::bail!("copilot token refresh failed: {}", resp.status());
            }
            let body: serde_json::Value = resp.json().await?;
            let token = body
                .get("token")
                .and_then(|t| t.as_str())
                .ok_or_else(|| anyhow::anyhow!("no token in copilot response"))?
                .to_string();
            let expires_at = body
                .get("expires_at")
                .and_then(|e| e.as_u64())
                .unwrap_or(now + 1800);
            *cache = Some((token.clone(), expires_at));
            Ok(token)
        } else {
            // Use the configured api_key directly
            self.config
                .api_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("embedding api_key not configured"))
        }
    }

    /// Embed one or more texts. Returns vectors.
    pub async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if !self.config.enabled {
            anyhow::bail!("embedding is disabled");
        }
        let model = self
            .config
            .model
            .as_deref()
            .unwrap_or("text-embedding-3-small");

        let base_url = self
            .config
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");

        let url = format!("{}/embeddings", base_url.trim_end_matches('/'));

        let api_key = self.get_bearer_token().await?;

        let body = serde_json::json!({
            "input": texts,
            "model": model,
        });

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json");

        // Copilot requires editor-version header
        if self.copilot_pat.is_some() {
            req = req.header("editor-version", "vscode/1.96.0");
            req = req.header("Copilot-Integration-Id", "vscode-chat");
        }

        let resp = req.json(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("embedding API error {}: {}", status, text);
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let data = resp_json
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow::anyhow!("invalid embedding response: missing data array"))?;

        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| anyhow::anyhow!("invalid embedding entry"))?;
            let vec: Vec<f32> = embedding
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            vectors.push(vec);
        }

        Ok(vectors)
    }

    /// Embed a single text.
    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.embed(&[text]).await?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no embedding returned"))
    }
}

impl Clone for EmbeddingEngine {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            client: self.client.clone(),
            copilot_pat: self.copilot_pat.clone(),
            copilot_token: self.copilot_token.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_vector_store_upsert_and_search() {
        let mut store = VectorStore {
            model: "test".to_string(),
            dimensions: 3,
            entries: Vec::new(),
        };

        store.upsert("a".into(), "hello".into(), vec![1.0, 0.0, 0.0]);
        store.upsert("b".into(), "world".into(), vec![0.0, 1.0, 0.0]);
        store.upsert("c".into(), "similar".into(), vec![0.9, 0.1, 0.0]);

        let query = vec![1.0, 0.0, 0.0];
        let results = store.search(&query, 0.5, 10);

        assert!(results.len() >= 2); // "a" and "c" should match
        assert_eq!(results[0].1.key, "a"); // exact match first
    }

    #[test]
    fn test_vector_store_upsert_update() {
        let mut store = VectorStore {
            model: "test".to_string(),
            dimensions: 2,
            entries: Vec::new(),
        };

        store.upsert("key1".into(), "old text".into(), vec![1.0, 0.0]);
        store.upsert("key1".into(), "new text".into(), vec![0.0, 1.0]);

        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].text, "new text");
        assert_eq!(store.entries[0].vector, vec![0.0, 1.0]);
    }

    #[test]
    fn test_vector_store_remove() {
        let mut store = VectorStore {
            model: "test".to_string(),
            dimensions: 2,
            entries: Vec::new(),
        };

        store.upsert("keep".into(), "keep".into(), vec![1.0, 0.0]);
        store.upsert("remove".into(), "remove".into(), vec![0.0, 1.0]);
        store.remove("remove");

        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].key, "keep");
    }

    #[test]
    fn test_vector_store_save_load() {
        let dir = std::env::temp_dir().join(format!("rune-vec-{}", std::process::id()));
        let path = dir.join("test_store.json");

        let mut store = VectorStore {
            model: "text-embedding-3-small".to_string(),
            dimensions: 3,
            entries: Vec::new(),
        };
        store.upsert("test".into(), "test content".into(), vec![0.1, 0.2, 0.3]);
        store.save(&path).unwrap();

        let loaded = VectorStore::load(&path);
        assert_eq!(loaded.model, "text-embedding-3-small");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].key, "test");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vector_store_load_nonexistent() {
        let path = PathBuf::from("/nonexistent/store.json");
        let store = VectorStore::load(&path);
        assert!(store.entries.is_empty());
        assert!(store.model.is_empty());
    }

    #[test]
    fn test_vector_store_is_valid_for_model() {
        let store = VectorStore {
            model: "text-embedding-3-small".to_string(),
            dimensions: 1536,
            entries: Vec::new(),
        };
        assert!(store.is_valid_for_model("text-embedding-3-small"));
        assert!(!store.is_valid_for_model("other-model"));
    }

    #[test]
    fn test_embedding_config_default() {
        let cfg = EmbeddingConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.model.is_none());
        assert!((cfg.threshold - 0.3).abs() < 1e-6);
        assert_eq!(cfg.max_skills, 3);
    }

    #[test]
    fn test_embedding_config_custom_max_skills() {
        let toml_str = r#"
enabled = true
threshold = 0.5
max_skills = 5
"#;
        let cfg: EmbeddingConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.enabled);
        assert!((cfg.threshold - 0.5).abs() < 1e-6);
        assert_eq!(cfg.max_skills, 5);
    }

    #[test]
    fn test_embedding_config_max_skills_default_when_omitted() {
        let toml_str = r#"
enabled = true
"#;
        let cfg: EmbeddingConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.max_skills, 3);
    }
}
