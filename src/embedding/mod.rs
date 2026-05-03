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
}

fn default_true() -> bool {
    true
}

fn default_threshold() -> f32 {
    0.6
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: None,
            base_url: None,
            api_key: None,
            threshold: 0.6,
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
    pub fn search(&self, query_vector: &[f32], threshold: f32, max_results: usize) -> Vec<(f32, &VectorEntry)> {
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
pub struct EmbeddingEngine {
    pub config: EmbeddingConfig,
    client: reqwest::Client,
}

impl EmbeddingEngine {
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
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

        let api_key = self
            .config
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("embedding api_key not configured"))?;

        let body = serde_json::json!({
            "input": texts,
            "model": model,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

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
        assert!((cfg.threshold - 0.6).abs() < 1e-6);
    }
}
