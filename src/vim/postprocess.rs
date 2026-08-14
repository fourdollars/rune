//! 5-Stage FIM Post-Processing Pipeline (§7).

/// Post-process raw completion text from LLM provider.
pub fn postprocess_completion(
    raw: &str,
    prefix: &str,
    suffix: &str,
    max_lines: Option<usize>,
) -> Option<String> {
    let mut text = raw.to_string();

    // Stage 1: Strip leading/trailing markdown code fences and language tags
    text = strip_code_fences(&text);

    // Stage 2: Strip any echoed copy of the prefix from the head
    text = strip_prefix_echo(&text, prefix);

    // Stage 3: Suffix de-duplication (trim overlapping suffix head)
    text = strip_suffix_overlap(&text, suffix);

    // Stage 4: Trim unbalanced trailing closers that already exist in suffix
    text = trim_unbalanced_closers(&text, suffix);

    // Optional line limit truncation (e.g. R3/R5 rendering limits)
    if let Some(max_l) = max_lines {
        let lines: Vec<&str> = text.lines().take(max_l).collect();
        if !lines.is_empty() {
            text = lines.join("\n");
        }
    }

    // Stage 5: Drop candidate if empty or whitespace
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn strip_code_fences(text: &str) -> String {
    let mut s = text.trim();
    if s.starts_with("```") {
        if let Some(first_line_end) = s.find('\n') {
            s = &s[first_line_end + 1..];
        } else {
            return String::new();
        }
    }
    if s.ends_with("```") {
        s = s.trim_end_matches('`').trim();
    }
    s.to_string()
}

fn strip_prefix_echo(text: &str, prefix: &str) -> String {
    let text_trimmed = text.trim_start();
    let prefix_trimmed = prefix.trim_end();

    if !prefix_trimmed.is_empty() && text_trimmed.starts_with(prefix_trimmed) {
        return text_trimmed[prefix_trimmed.len()..].to_string();
    }

    if let Some(last_line) = prefix.lines().last() {
        let last_line_trimmed = last_line.trim_start();
        if !last_line_trimmed.is_empty() && text_trimmed.starts_with(last_line_trimmed) {
            return text_trimmed[last_line_trimmed.len()..].to_string();
        }
    }

    // Strip any longest overlap between prefix tail and text head (e.g. prefix ends with "join!", text starts with "join!(a, b);")
    let max_check = text_trimmed.len().min(prefix_trimmed.len());
    for len in (1..=max_check).rev() {
        let prefix_tail = &prefix_trimmed[prefix_trimmed.len() - len..];
        let text_head = &text_trimmed[..len];
        if prefix_tail == text_head {
            return text_trimmed[len..].to_string();
        }
    }

    text.to_string()
}

fn strip_suffix_overlap(text: &str, suffix: &str) -> String {
    let suffix_trimmed = suffix.trim_start();
    if suffix_trimmed.is_empty() || text.is_empty() {
        return text.to_string();
    }

    let max_check = text.len().min(suffix_trimmed.len());
    for len in (1..=max_check).rev() {
        let tail = &text[text.len() - len..];
        let head = &suffix_trimmed[..len];
        if tail == head {
            return text[..text.len() - len].to_string();
        }
    }

    text.to_string()
}

fn trim_unbalanced_closers(text: &str, suffix: &str) -> String {
    let suffix_head = suffix.trim_start();
    let mut t = text.to_string();

    let closers = [')', ']', '}', ';'];
    for &c in &closers {
        if suffix_head.starts_with(c) && t.ends_with(c) {
            // Count occurrences
            let text_open = t.chars().filter(|&ch| ch == matching_open(c)).count();
            let text_close = t.chars().filter(|&ch| ch == c).count();

            if text_close > text_open {
                t.pop();
            }
        }
    }

    t
}

fn matching_open(c: char) -> char {
    match c {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fence_strip() {
        let raw = "```rust\nlet x = 42;\n```";
        let out = postprocess_completion(raw, "", "", None);
        assert_eq!(out, Some("let x = 42;".to_string()));
    }

    #[test]
    fn test_suffix_overlap() {
        let raw = "App::new();";
        let suffix = ");\n}";
        let out = postprocess_completion(raw, "let app = ", suffix, None);
        assert_eq!(out, Some("App::new(".to_string()));
    }

    #[test]
    fn test_empty_drop() {
        let raw = "   \n  ";
        let out = postprocess_completion(raw, "", "", None);
        assert_eq!(out, None);
    }

    #[test]
    fn test_prefix_echo_token_overlap() {
        let raw = "join!(a, b);";
        let prefix = "let res = tokio::join!";
        let out = postprocess_completion(raw, prefix, "", None);
        assert_eq!(out, Some("(a, b);".to_string()));
    }
}
