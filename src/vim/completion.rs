//! Inline FIM Completion Engine for Rune Vim (§5.2 & §6).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use crate::provider::{LlmMessage, LlmRequest, ProviderRegistry};
use crate::vim::fim::{build_fim_prompt, DefaultFimCapability, FimCapability};
use crate::vim::postprocess::postprocess_completion;
use crate::vim::protocol::{Candidate, CompletionParams, CompletionResult};

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct CacheKey {
    buffer_id: u64,
    version: u64,
    prefix_hash: u64,
    suffix_hash: u64,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    candidates: Vec<Candidate>,
}

/// Version-keyed Completion Cache.
pub struct CompletionCache {
    entries: Mutex<HashMap<CacheKey, CacheEntry>>,
    latest_version: Mutex<HashMap<u64, u64>>,
}

impl Default for CompletionCache {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            latest_version: Mutex::new(HashMap::new()),
        }
    }
}

impl CompletionCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn hash_str(s: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }

    pub fn get(&self, params: &CompletionParams) -> Option<Vec<Candidate>> {
        let key = CacheKey {
            buffer_id: params.buffer_id,
            version: params.version,
            prefix_hash: Self::hash_str(&params.prefix),
            suffix_hash: Self::hash_str(&params.suffix),
        };
        let guard = self.entries.lock().unwrap();
        guard.get(&key).map(|e| e.candidates.clone())
    }

    pub fn put(&self, params: &CompletionParams, candidates: Vec<Candidate>) {
        let key = CacheKey {
            buffer_id: params.buffer_id,
            version: params.version,
            prefix_hash: Self::hash_str(&params.prefix),
            suffix_hash: Self::hash_str(&params.suffix),
        };

        // Evict older versions for this buffer_id
        {
            let mut ver_guard = self.latest_version.lock().unwrap();
            let prev_ver = ver_guard.entry(params.buffer_id).or_insert(params.version);
            if params.version > *prev_ver {
                *prev_ver = params.version;
                let mut entries_guard = self.entries.lock().unwrap();
                entries_guard
                    .retain(|k, _| k.buffer_id != params.buffer_id || k.version == params.version);
            }
        }

        let mut guard = self.entries.lock().unwrap();
        guard.insert(key, CacheEntry { candidates });
    }
}

/// Truncate prefix and suffix per Context Window Policy (§6).
pub fn truncate_context(prefix: &str, suffix: &str) -> (String, String, bool, bool) {
    const MAX_PREFIX_LINES: usize = 120;
    const MAX_SUFFIX_LINES: usize = 60;
    const MAX_PREFIX_BYTES: usize = 8192;
    const MAX_SUFFIX_BYTES: usize = 4096;

    let (trunc_p_text, trunc_p_flag) = truncate_head(prefix, MAX_PREFIX_LINES, MAX_PREFIX_BYTES);
    let (trunc_s_text, trunc_s_flag) = truncate_tail(suffix, MAX_SUFFIX_LINES, MAX_SUFFIX_BYTES);

    (trunc_p_text, trunc_s_text, trunc_p_flag, trunc_s_flag)
}

fn truncate_head(text: &str, max_lines: usize, max_bytes: usize) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let mut truncated = false;

    let take_lines = if lines.len() > max_lines {
        truncated = true;
        &lines[lines.len() - max_lines..]
    } else {
        &lines[..]
    };

    let joined = take_lines.join("\n");
    if joined.len() > max_bytes {
        let start = joined.len() - max_bytes;
        // Adjust to char boundary
        let char_start = joined
            .char_indices()
            .find(|&(i, _)| i >= start)
            .map(|(i, _)| i)
            .unwrap_or(0);
        (joined[char_start..].to_string(), true)
    } else {
        (joined, truncated)
    }
}

fn truncate_tail(text: &str, max_lines: usize, max_bytes: usize) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let mut truncated = false;

    let take_lines = if lines.len() > max_lines {
        truncated = true;
        &lines[..max_lines]
    } else {
        &lines[..]
    };

    let joined = take_lines.join("\n");
    if joined.len() > max_bytes {
        // Adjust to char boundary
        let char_end = joined
            .char_indices()
            .take_while(|&(i, _)| i <= max_bytes)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(joined.len());
        (joined[..char_end].to_string(), true)
    } else {
        (joined, truncated)
    }
}

/// Execute FIM completion query via ProviderRegistry.
pub async fn execute_completion(
    params: &CompletionParams,
    cache: &CompletionCache,
    providers: &ProviderRegistry,
    model: &str,
) -> anyhow::Result<CompletionResult> {
    // 1. Check cache
    if let Some(candidates) = cache.get(params) {
        return Ok(CompletionResult {
            buffer_id: params.buffer_id,
            version: params.version,
            cached: true,
            candidates,
        });
    }

    // 2. Truncate context
    let (prefix, suffix, _, _) = truncate_context(&params.prefix, &params.suffix);

    // 3. Determine FIM capability & prompt
    let fim_cap = DefaultFimCapability;
    let fim_mode = fim_cap.fim_mode(model);
    let (prompt, _stop_seqs) = build_fim_prompt(&fim_mode, &prefix, &suffix, &params.language);

    // 4. Send request to LLM provider
    let request = LlmRequest {
        model: model.to_string(),
        messages: vec![LlmMessage {
            role: "user".to_string(),
            name: None,
            content: Some(prompt),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        tools: None,
        max_tokens: Some(64),
        thinking: None,
    };

    let response = providers.chat(request).await?;
    let raw_text = response.content.unwrap_or_default();

    // 5. Post-process
    let mut candidates = Vec::new();
    if let Some(clean_text) = postprocess_completion(&raw_text, &prefix, &suffix, None) {
        let display_lines = clean_text.lines().count().max(1);
        candidates.push(Candidate {
            text: clean_text,
            display_lines,
            source: "model".to_string(),
        });
    }

    // 6. Cache result
    cache.put(params, candidates.clone());

    Ok(CompletionResult {
        buffer_id: params.buffer_id,
        version: params.version,
        cached: false,
        candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_context_lines_and_bytes() {
        let mut long_prefix = String::new();
        for i in 0..200 {
            long_prefix.push_str(&format!("line {}\n", i));
        }

        let (p_text, s_text, p_trunc, s_trunc) =
            truncate_context(&long_prefix, "suffix line 1\nsuffix line 2");
        assert!(p_trunc);
        assert!(!s_trunc);
        assert!(p_text.lines().count() <= 120);
        assert_eq!(s_text, "suffix line 1\nsuffix line 2");
    }

    #[test]
    fn test_cache_version_eviction() {
        let cache = CompletionCache::new();
        let params_v1 = CompletionParams {
            buffer_id: 1,
            version: 10,
            filepath: "test.rs".into(),
            language: "rust".into(),
            prefix: "fn test() {".into(),
            suffix: "}".into(),
            truncated: None,
            line: 1,
            character: 11,
            max_candidates: 1,
        };

        let candidate = vec![Candidate {
            text: "println!();".into(),
            display_lines: 1,
            source: "model".into(),
        }];

        cache.put(&params_v1, candidate.clone());
        assert!(cache.get(&params_v1).is_some());

        // Put higher version for same buffer_id -> v1 evicted
        let params_v2 = CompletionParams {
            version: 11,
            ..params_v1.clone()
        };

        cache.put(&params_v2, candidate.clone());
        assert!(cache.get(&params_v1).is_none());
        assert!(cache.get(&params_v2).is_some());
    }
}
