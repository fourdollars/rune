//! Fill-In-the-Middle (FIM) provider capabilities and prompt formatting (§7).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FimMode {
    Native {
        prefix_tok: &'static str,
        suffix_tok: &'static str,
        middle_tok: &'static str,
    },
    ChatFallback,
}

pub trait FimCapability {
    fn fim_mode(&self, model: &str) -> FimMode;
}

pub struct DefaultFimCapability;

impl FimCapability for DefaultFimCapability {
    fn fim_mode(&self, model: &str) -> FimMode {
        let m = model.to_lowercase();
        if m.contains("copilot")
            || m.contains("codex")
            || m.contains("starcoder")
            || m.contains("deepseek")
        {
            FimMode::Native {
                prefix_tok: "<PRE>",
                suffix_tok: "<SUF>",
                middle_tok: "<MID>",
            }
        } else {
            FimMode::ChatFallback
        }
    }
}

/// Construct prompt for FIM request.
pub fn build_fim_prompt(
    mode: &FimMode,
    prefix: &str,
    suffix: &str,
    language: &str,
) -> (String, Vec<String>) {
    match mode {
        FimMode::Native {
            prefix_tok,
            suffix_tok,
            middle_tok,
        } => {
            let prompt = format!(
                "{}{} {}{} {}",
                prefix_tok, prefix, suffix_tok, suffix, middle_tok
            );
            let stop = vec![
                "\n\n".to_string(),
                "<EOT>".to_string(),
                "<|endoftext|>".to_string(),
            ];
            (prompt, stop)
        }
        FimMode::ChatFallback => {
            let prompt = format!(
                "You are an inline code completion assistant for {}.\n\
                Provide ONLY the code snippet to insert immediately at the end of the Prefix (before Suffix).\n\
                Do NOT repeat the prefix. Do NOT output markdown code fences, comments, or explanations. Output ONLY raw code.\n\n\
                Prefix:\n{}\n\n\
                Suffix:\n{}",
                language, prefix, suffix
            );
            let stop = vec!["\n\n".to_string(), "```".to_string()];
            (prompt, stop)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fim_mode_detection() {
        let cap = DefaultFimCapability;
        assert!(matches!(
            cap.fim_mode("copilot-gpt-4o"),
            FimMode::Native { .. }
        ));
        assert!(matches!(
            cap.fim_mode("deepseek-coder"),
            FimMode::Native { .. }
        ));
        assert_eq!(cap.fim_mode("claude-3-5-sonnet"), FimMode::ChatFallback);
    }

    #[test]
    fn test_build_fim_prompt_native() {
        let mode = FimMode::Native {
            prefix_tok: "<PRE>",
            suffix_tok: "<SUF>",
            middle_tok: "<MID>",
        };
        let (prompt, stops) = build_fim_prompt(&mode, "let x = ", ";", "rust");
        assert_eq!(prompt, "<PRE>let x =  <SUF>; <MID>");
        assert!(stops.contains(&"<EOT>".to_string()));
    }
}
