use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: Option<String>,
    pub tools_allow: Option<Vec<String>>,
    pub tools_deny: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub metadata: SkillMetadata,
    pub content: String,
    pub source_path: PathBuf,
}

pub struct SkillLoader {
    pub search_paths: Vec<PathBuf>,
}

impl SkillLoader {
    /// 建立 loader，指定搜尋路徑順序
    pub fn new(search_paths: Vec<PathBuf>) -> Self {
        Self { search_paths }
    }

    /// 根據名稱搜尋並載入 skill
    /// 搜尋順序：自訂 search_paths -> $RUNE_HOME/skills/{name}/SKILL.md -> ~/.rune/skills/{name}/SKILL.md -> .rune/skills/{name}/SKILL.md
    pub fn load(&self, name: &str) -> Result<Skill> {
        // candidate locations
        let mut candidates: Vec<PathBuf> = Vec::new();

        for p in &self.search_paths {
            candidates.push(p.join(name).join("SKILL.md"));
        }

        if let Ok(rune_home) = env::var("RUNE_HOME") {
            candidates.push(PathBuf::from(rune_home).join("skills").join(name).join("SKILL.md"));
        }

        if let Ok(home) = env::var("HOME") {
            candidates.push(PathBuf::from(home).join(".rune").join("skills").join(name).join("SKILL.md"));
        }

        if let Ok(cur) = env::current_dir() {
            candidates.push(cur.join(".rune").join("skills").join(name).join("SKILL.md"));
        }

        // find first existing candidate
        let path = candidates
            .into_iter()
            .find(|p| p.exists() && p.is_file())
            .ok_or_else(|| anyhow::anyhow!("skill '{}' not found in search paths", name))?;

        let raw = fs::read_to_string(&path).with_context(|| format!("reading SKILL.md at {}", path.display()))?;

        let (metadata, body) = parse_frontmatter(&raw, &path, Some(name));

        Ok(Skill {
            metadata,
            content: body,
            source_path: path,
        })
    }

    /// 從 prompt 文字中提取 @skill_name 引用
    pub fn extract_skill_refs(prompt: &str) -> Vec<String> {
        let mut refs = Vec::new();
        let mut seen = HashSet::new();

        let mut chars = prompt.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '@' {
                let mut token = String::new();
                while let Some(&next) = chars.peek() {
                    if next.is_alphanumeric() || next == '_' || next == '-' || next == '.' {
                        token.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if !token.is_empty() && !seen.contains(&token) {
                    seen.insert(token.clone());
                    refs.push(token);
                }
            }
        }

        refs
    }

    /// 載入所有被引用的 skills 並組合成系統提示片段
    pub fn resolve_skills(&self, prompt: &str) -> Result<Vec<Skill>> {
        let refs = Self::extract_skill_refs(prompt);
        let mut skills = Vec::new();
        for r in refs {
            let s = self.load(&r)?;
            skills.push(s);
        }
        Ok(skills)
    }
}

/// 解析簡單的 YAML-like frontmatter（不使用 yaml crate）
fn parse_frontmatter(content: &str, source_path: &Path, default_name: Option<&str>) -> (SkillMetadata, String) {
    let all_lines: Vec<&str> = content.lines().collect();
    if let Some(first) = all_lines.get(0) {
        if first.trim() == "---" {
            // find closing
            let mut end_idx: Option<usize> = None;
            for (i, line) in all_lines.iter().enumerate().skip(1) {
                if line.trim() == "---" {
                    end_idx = Some(i);
                    break;
                }
            }

            if let Some(ei) = end_idx {
                let front_lines = &all_lines[1..ei];
                let body = all_lines[(ei + 1)..].join("\n");

                // parse keys
                let mut name: Option<String> = None;
                let mut description: Option<String> = None;
                let mut tools_allow: Option<Vec<String>> = None;
                let mut tools_deny: Option<Vec<String>> = None;

                for raw in front_lines.iter() {
                    let line = raw.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some(colon) = line.find(':') {
                        let key = line[..colon].trim().to_lowercase();
                        let mut val = line[(colon + 1)..].trim().to_string();

                        // handle list on same line: key: [a, b]
                        if val.starts_with('[') && val.ends_with(']') {
                            let inner = val[1..(val.len() - 1)].trim();
                            let items: Vec<String> = if inner.is_empty() {
                                Vec::new()
                            } else {
                                inner
                                    .split(',')
                                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                                    .collect()
                            };
                            match key.as_str() {
                                "tools_allow" | "tools-allow" => tools_allow = Some(items),
                                "tools_deny" | "tools-deny" => tools_deny = Some(items),
                                _ => {}
                            }
                        } else {
                            // string value
                            // strip surrounding quotes if present
                            if (val.starts_with('"') && val.ends_with('"')) || (val.starts_with('\'') && val.ends_with('\'')) {
                                val = val[1..(val.len() - 1)].to_string();
                            }
                            match key.as_str() {
                                "name" => name = Some(val.clone()),
                                "description" => description = Some(val.clone()),
                                "tools_allow" | "tools-allow" => {
                                    // support comma-separated on single line without brackets
                                    let items: Vec<String> = val
                                        .split(',')
                                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                                        .filter(|s| !s.is_empty())
                                        .collect();
                                    tools_allow = Some(items);
                                }
                                "tools_deny" | "tools-deny" => {
                                    let items: Vec<String> = val
                                        .split(',')
                                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                                        .filter(|s| !s.is_empty())
                                        .collect();
                                    tools_deny = Some(items);
                                }
                                _ => {}
                            }
                        }
                    }
                }

                // derive name if missing
                let final_name = name.or_else(|| {
                    // try parent directory name
                    source_path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|os| os.to_str())
                        .map(|s| s.to_string())
                        .or_else(|| default_name.map(|s| s.to_string()))
                })
                .unwrap_or_else(|| default_name.unwrap_or("unknown").to_string());

                let metadata = SkillMetadata {
                    name: final_name,
                    description,
                    tools_allow,
                    tools_deny,
                };

                return (metadata, body);
            }
        }
    }

    // no frontmatter: body is whole content
    let metadata_name = source_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|os| os.to_str())
        .map(|s| s.to_string())
        .or_else(|| default_name.map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    let metadata = SkillMetadata {
        name: metadata_name,
        description: None,
        tools_allow: None,
        tools_deny: None,
    };

    (metadata, content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_extract_skill_refs() {
        let s = "This references @alpha and @beta-1 and @alpha again.";
        let refs = SkillLoader::extract_skill_refs(s);
        assert_eq!(refs, vec!["alpha".to_string(), "beta-1".to_string()]);
    }

    #[test]
    fn test_parse_frontmatter_with_frontmatter() {
        let content = "---\nname: \"my-skill\"\ndescription: 'A skill'\ntools_allow: [a, b]\n---\nSkill body line\nSecond line";
        let path = PathBuf::from("/tmp/skills/my-skill/SKILL.md");
        let (meta, body) = parse_frontmatter(content, &path, Some("my-skill"));
        assert_eq!(meta.name, "my-skill");
        assert_eq!(meta.description.as_deref(), Some("A skill"));
        assert_eq!(meta.tools_allow.as_ref().map(|v| v.clone()).unwrap(), vec!["a".to_string(), "b".to_string()]);
        assert!(body.contains("Skill body line"));
    }

    #[test]
    fn test_parse_frontmatter_without_frontmatter() {
        let content = "No frontmatter here\nJust body";
        let path = PathBuf::from("/tmp/skills/simple/SKILL.md");
        let (meta, body) = parse_frontmatter(content, &path, Some("simple"));
        assert_eq!(meta.name, "simple");
        assert!(meta.description.is_none());
        assert_eq!(body, content.to_string());
    }
}

