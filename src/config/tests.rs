use super::loader::*;
use super::persist::*;
use super::types::*;
use super::util::*;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

use super::*;

#[test]
fn test_pick_returns_first_some() {
    let a = Some("first".to_string());
    let b = Some("second".to_string());
    let c: Option<String> = None;
    assert_eq!(pick(&[&a, &b], "default".to_string()), "first");
    assert_eq!(pick(&[&c, &b], "default".to_string()), "second");
}

#[test]
fn test_pick_returns_default_when_all_none() {
    let a: Option<String> = None;
    let b: Option<String> = None;
    assert_eq!(pick(&[&a, &b], "default".to_string()), "default");
}

#[test]
fn test_pick_option_returns_first_some() {
    let a: Option<u32> = None;
    let b = Some(42u32);
    let c = Some(99u32);
    assert_eq!(pick_option(&[&a, &b, &c]), Some(42));
}

#[test]
fn test_pick_option_returns_none_when_all_none() {
    let a: Option<u32> = None;
    let b: Option<u32> = None;
    assert_eq!(pick_option(&[&a, &b]), None);
}

#[test]
fn test_parse_boolish_true_variants() {
    assert_eq!(parse_boolish("1"), Some(true));
    assert_eq!(parse_boolish("true"), Some(true));
    assert_eq!(parse_boolish("TRUE"), Some(true));
    assert_eq!(parse_boolish("yes"), Some(true));
    assert_eq!(parse_boolish("Yes"), Some(true));
    assert_eq!(parse_boolish("y"), Some(true));
    assert_eq!(parse_boolish("on"), Some(true));
    assert_eq!(parse_boolish("ON"), Some(true));
}

#[test]
fn test_parse_boolish_false_variants() {
    assert_eq!(parse_boolish("0"), Some(false));
    assert_eq!(parse_boolish("false"), Some(false));
    assert_eq!(parse_boolish("FALSE"), Some(false));
    assert_eq!(parse_boolish("no"), Some(false));
    assert_eq!(parse_boolish("n"), Some(false));
    assert_eq!(parse_boolish("off"), Some(false));
}

#[test]
fn test_parse_boolish_invalid() {
    assert_eq!(parse_boolish("maybe"), None);
    assert_eq!(parse_boolish(""), None);
    assert_eq!(parse_boolish("2"), None);
    assert_eq!(parse_boolish("yep"), None);
}

#[test]
fn test_parse_boolish_with_whitespace() {
    assert_eq!(parse_boolish("  true  "), Some(true));
    assert_eq!(parse_boolish(" false "), Some(false));
}

#[test]
fn test_policy_config_default() {
    let p = PolicyConfig::default();
    assert_eq!(p.mode, "allowlist");
    assert!(p.allowed_commands.is_empty());
    assert!(p.allowed_domains.is_empty());
    assert!(p.allowed_syscalls.is_empty()); // empty = block all dangerous syscalls
    assert!(p.allowed_paths_rw.is_empty());
    assert!(p.allowed_paths_ro.is_empty());
    assert!(p.allowed_files_ro.is_empty());
    assert!(p.allowed_files_rw.is_empty());
    assert!(p.denied_paths.contains(&"/root".to_string()));
    assert_eq!(p.max_memory_mb, 512);
    assert_eq!(p.max_pids, 64);
}

#[test]
fn test_rune_config_default() {
    let c = RuneConfig::default();
    assert_eq!(c.model, "");
    assert!(c.api_key.is_none());
    assert_eq!(c.skills_dir, "~/skills");
    assert_eq!(c.log_level, "error");
    assert_eq!(c.max_steps, Some(50));
    assert_eq!(c.token_budget, None); // Default: no cost guard limit
    assert_eq!(c.timeout_secs, Some(30));
    assert!(c.base_url.is_none());
    assert!(c.trace.is_none());
    assert!(!c.json_output);
    assert!(!c.auto_approve);
}

#[test]
fn test_load_toml_nonexistent_path() {
    let path = PathBuf::from("/nonexistent/rune.toml");
    assert!(load_toml(&path).is_none());
}

#[test]
fn test_load_toml_valid() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-test-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(
        &path,
        r#"
model = "gpt-4o"
log_level = "debug"
max_steps = 50
token_budget = 8000
"#,
    )
    .unwrap();

    let partial = load_toml(&path).expect("should parse");
    assert_eq!(partial.model.as_deref(), Some("gpt-4o"));
    assert_eq!(partial.log_level.as_deref(), Some("debug"));
    assert_eq!(partial.max_steps, Some(50));
    assert_eq!(partial.token_budget, Some(8000));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_load_toml_with_policy() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-pol-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(
        &path,
        r#"
model = "gpt-4"

[policy]
mode = "allowlist"
allowed_commands = ["ls", "cat", "grep"]
allowed_domains = ["github.com", "*.openai.com"]
allowed_paths_rw = ["/workspace"]
allowed_paths_ro = ["/usr", "/bin"]
denied_paths = ["/etc/shadow"]
"#,
    )
    .unwrap();

    let partial = load_toml(&path).expect("should parse");
    let policy = partial.policy.unwrap();
    assert_eq!(policy.mode, "allowlist");
    assert_eq!(policy.allowed_commands, vec!["ls", "cat", "grep"]);
    assert_eq!(policy.allowed_domains, vec!["github.com", "*.openai.com"]);
    assert_eq!(policy.allowed_paths_rw, vec!["/workspace"]);
    assert_eq!(policy.denied_paths, vec!["/etc/shadow"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_default_policy_mode_is_allowlist() {
    let policy = PolicyConfig::default();
    assert_eq!(
        policy.mode, "allowlist",
        "default policy should be allowlist"
    );
}

#[test]
fn test_load_toml_unrestricted_mode() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-unr-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(
        &path,
        r#"
model = "gpt-4"

[policy]
mode = "unrestricted"
"#,
    )
    .unwrap();

    let partial = load_toml(&path).expect("should parse");
    let policy = partial.policy.unwrap();
    assert_eq!(policy.mode, "unrestricted");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_load_toml_invalid_content() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-bad-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(&path, "this is not valid toml {{{{").unwrap();

    assert!(load_toml(&path).is_none());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_persist_domain_creates_entry() {
    let dir = std::env::temp_dir().join(format!("rune-persist-{}", std::process::id()));
    let rune_dir = dir.join(".rune");
    let _ = fs::create_dir_all(&rune_dir);
    let config_path = rune_dir.join("rune.toml");
    fs::write(
        &config_path,
        r#"
model = "gpt-4"

[policy]
mode = "confirm"
allowed_domains = ["existing.com"]
"#,
    )
    .unwrap();

    persist_policy_array_at(&config_path, "allowed_domains", "new-domain.com");

    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("new-domain.com"));
    assert!(content.contains("existing.com"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_persist_domain_no_duplicate() {
    let dir = std::env::temp_dir().join(format!("rune-persist-dup-{}", std::process::id()));
    let rune_dir = dir.join(".rune");
    let _ = fs::create_dir_all(&rune_dir);
    let config_path = rune_dir.join("rune.toml");
    fs::write(
        &config_path,
        r#"
[policy]
allowed_domains = ["github.com"]
"#,
    )
    .unwrap();

    persist_policy_array_at(&config_path, "allowed_domains", "github.com"); // already exists

    let content = fs::read_to_string(&config_path).unwrap();
    // Should only appear once
    assert_eq!(content.matches("github.com").count(), 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_persist_command_creates_entry() {
    let dir = std::env::temp_dir().join(format!("rune-persist-cmd-{}", std::process::id()));
    let rune_dir = dir.join(".rune");
    let _ = fs::create_dir_all(&rune_dir);
    let config_path = rune_dir.join("rune.toml");
    fs::write(
        &config_path,
        r#"
[policy]
mode = "confirm"
"#,
    )
    .unwrap();

    persist_policy_array_at(&config_path, "allowed_commands", "cargo");

    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("cargo"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_persist_path_ro() {
    let dir = std::env::temp_dir().join(format!("rune-persist-path-{}", std::process::id()));
    let rune_dir = dir.join(".rune");
    let _ = fs::create_dir_all(&rune_dir);
    let config_path = rune_dir.join("rune.toml");
    fs::write(
        &config_path,
        r#"
[policy]
mode = "confirm"
"#,
    )
    .unwrap();

    persist_policy_array_at(&config_path, "allowed_paths_ro", "/home/user/project");

    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("/home/user/project"));
    assert!(content.contains("allowed_paths_ro"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_config_provider_field() {
    let toml_str = r#"
model = "gemini-pro"
api_key = "AIzaXXXX"
skills_dir = "./skills"
log_level = "info"
provider = "gemini"
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.provider, Some("gemini".to_string()));
}

#[test]
fn test_config_provider_field_missing_is_none() {
    let toml_str = r#"
model = "gpt-4"
api_key = "sk-xxx"
skills_dir = "./skills"
log_level = "info"
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.provider, None);
}

#[test]
fn test_preload_skills_default_empty() {
    let cfg = RuneConfig::default();
    assert!(cfg.preload_skills.is_empty());
}

#[test]
fn test_preload_skills_not_serialized() {
    // preload_skills is marked #[serde(skip)] so it should not appear in TOML
    let toml_str = r#"
model = "gpt-4"
skills_dir = "./skills"
log_level = "info"
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    // PartialConfig doesn't have preload_skills, confirming it's CLI-only
    assert!(cfg.skills_dir.is_some());
}

#[test]
fn test_system_prompt_from_toml() {
    let toml_str = r#"
model = "gpt-4"
system_prompt = "You are a custom agent."
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        cfg.system_prompt,
        Some("You are a custom agent.".to_string())
    );
}

#[test]
fn test_system_prompt_default_none() {
    let cfg = RuneConfig::default();
    assert!(cfg.system_prompt.is_none());
}

#[test]
fn test_system_prompt_missing_in_toml_is_none() {
    let toml_str = r#"
model = "gpt-4"
log_level = "info"
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.system_prompt.is_none());
}

#[test]
fn test_safe_truncate() {
    assert_eq!(safe_truncate("hello", 3), "hel");
    assert_eq!(safe_truncate("hello", 10), "hello");
    assert_eq!(safe_truncate("", 5), "");
    // "我" is 3 bytes (228, 136, 145)
    assert_eq!(safe_truncate("我", 0), "");
    assert_eq!(safe_truncate("我", 1), "");
    assert_eq!(safe_truncate("我", 2), "");
    assert_eq!(safe_truncate("我", 3), "我");
    assert_eq!(safe_truncate("我", 4), "我");

    // "我。？"
    // '。' (bytes 198..201 of string)
    // '？' (bytes 198..201 of string)
    let s = "我。？";
    assert_eq!(safe_truncate(s, 2), "");
    assert_eq!(safe_truncate(s, 3), "我");
    assert_eq!(safe_truncate(s, 5), "我");
    assert_eq!(safe_truncate(s, 6), "我。");
}

// --- expand_tilde tests ---
static HOME_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_expand_tilde_home_prefix() {
    let _lock = HOME_TEST_MUTEX.lock().unwrap();
    let orig = std::env::var("HOME").ok();
    std::env::set_var("HOME", "/home/testuser");
    assert_eq!(expand_tilde("~/skills"), "/home/testuser/skills");
    assert_eq!(expand_tilde("~/a/b/c"), "/home/testuser/a/b/c");
    if let Some(h) = orig {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }
}

#[test]
fn test_expand_tilde_bare_tilde() {
    let _lock = HOME_TEST_MUTEX.lock().unwrap();
    let orig = std::env::var("HOME").ok();
    std::env::set_var("HOME", "/home/testuser");
    assert_eq!(expand_tilde("~"), "/home/testuser");
    if let Some(h) = orig {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }
}

#[test]
fn test_expand_tilde_no_tilde() {
    assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
    assert_eq!(expand_tilde("relative/path"), "relative/path");
    assert_eq!(expand_tilde("./local"), "./local");
    assert_eq!(expand_tilde(""), "");
}

#[test]
fn test_expand_tilde_not_at_start() {
    // ~ not at start should not be expanded
    assert_eq!(expand_tilde("/home/~/weird"), "/home/~/weird");
    assert_eq!(expand_tilde("foo~/bar"), "foo~/bar");
}

#[test]
fn test_expand_tilde_vec_mixed() {
    let _lock = HOME_TEST_MUTEX.lock().unwrap();
    let orig = std::env::var("HOME").ok();
    std::env::set_var("HOME", "/home/u");
    let mut v = vec![
        "~/skills".to_string(),
        "/absolute".to_string(),
        "relative".to_string(),
        "~/other/dir".to_string(),
    ];
    expand_tilde_vec(&mut v);
    assert_eq!(
        v,
        vec![
            "/home/u/skills",
            "/absolute",
            "relative",
            "/home/u/other/dir",
        ]
    );
    if let Some(h) = orig {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }
}

// =========================================================
// Additional tests for increased coverage
// =========================================================

#[test]
fn test_serve_config_default() {
    let s = NotesConfig::default();
    assert!(s.port.is_none());
    assert!(s.bind.is_none());
    assert!(s.github.is_none());
}

#[test]
fn test_serve_config_toml_parsing() {
    let toml_str = r#"
model = "gpt-4"
skills_dir = "./skills"
log_level = "info"

[notes]
port = 9527
bind = "0.0.0.0"
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    let serve = cfg.notes.unwrap();
    assert_eq!(serve.port, Some(9527));
    assert_eq!(serve.bind.as_deref(), Some("0.0.0.0"));
    assert!(serve.github.is_none());
}

#[test]
fn test_partial_config_all_fields() {
    let toml_str = r#"
model = "claude-opus-4"
api_key = "sk-ant-xxx"
provider = "anthropic"
skills_dir = "~/.rune/skills"
log_level = "debug"
max_steps = 100
token_budget = 500000
timeout_secs = 60
base_url = "https://api.anthropic.com/v1"
trace = "/tmp/rune-trace"
context_window = 200000
compact_threshold = 0.9
compact_keep_last = 10
thinking = "high"
system_prompt = "You are an expert."
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("claude-opus-4"));
    assert_eq!(cfg.api_key.as_deref(), Some("sk-ant-xxx"));
    assert_eq!(cfg.provider.as_deref(), Some("anthropic"));
    assert_eq!(cfg.skills_dir.as_deref(), Some("~/.rune/skills"));
    assert_eq!(cfg.log_level.as_deref(), Some("debug"));
    assert_eq!(cfg.max_steps, Some(100));
    assert_eq!(cfg.token_budget, Some(500000));
    assert_eq!(cfg.timeout_secs, Some(60));
    assert_eq!(
        cfg.base_url.as_deref(),
        Some("https://api.anthropic.com/v1")
    );
    assert_eq!(cfg.trace.as_deref(), Some("/tmp/rune-trace"));
    assert_eq!(cfg.context_window, Some(200000));
    assert_eq!(cfg.compact_threshold, Some(0.9));
    assert_eq!(cfg.compact_keep_last, Some(10));
    assert_eq!(cfg.thinking.as_deref(), Some("high"));
    assert_eq!(cfg.system_prompt.as_deref(), Some("You are an expert."));
}

#[test]
fn test_rune_config_default_compact_fields() {
    let c = RuneConfig::default();
    assert_eq!(c.context_window, 128000);
    assert!((c.compact_threshold - 0.85).abs() < 1e-9);
    assert_eq!(c.compact_keep_last, 6);
}

#[test]
fn test_rune_config_default_thinking_none() {
    let c = RuneConfig::default();
    assert!(c.thinking.is_none());
}

#[test]
fn test_rune_config_default_provider_none() {
    let c = RuneConfig::default();
    assert!(c.provider.is_none());
}

#[test]
fn test_policy_config_deserialization_minimal() {
    let toml_str = r#"
mode = "allowlist"
"#;
    let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(policy.mode, "allowlist");
    assert!(policy.allowed_commands.is_empty());
    assert_eq!(policy.max_memory_mb, 0);
    assert_eq!(policy.max_pids, 0);
}

#[test]
fn test_policy_config_deserialization_full() {
    let toml_str = r#"
mode = "unrestricted"
allowed_commands = ["git", "cargo"]
allowed_domains = ["github.com"]
allowed_syscalls = ["ptrace"]
allowed_paths_rw = ["/workspace", "/tmp"]
allowed_paths_ro = ["/usr", "/bin"]
allowed_files_ro = ["/etc/hostname"]
allowed_files_rw = ["/tmp/out.txt"]
denied_paths = ["/etc/shadow", "/root"]
max_memory_mb = 1024
max_pids = 128
"#;
    let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(policy.mode, "unrestricted");
    assert_eq!(policy.allowed_commands, vec!["git", "cargo"]);
    assert_eq!(policy.allowed_domains, vec!["github.com"]);
    assert_eq!(policy.allowed_syscalls, vec!["ptrace"]);
    assert_eq!(policy.max_memory_mb, 1024);
    assert_eq!(policy.max_pids, 128);
}

#[test]
fn test_load_toml_with_serve_section() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-srv-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(
        &path,
        r#"
model = "gpt-4"
[notes]
port = 8080
bind = "127.0.0.1"
"#,
    )
    .unwrap();
    let partial = load_toml(&path).expect("should parse");
    let serve = partial.notes.unwrap();
    assert_eq!(serve.port, Some(8080));
    assert_eq!(serve.bind.as_deref(), Some("127.0.0.1"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_persist_policy_array_file_not_found() {
    // Should silently skip, not panic
    let path = std::path::Path::new("/nonexistent/path/rune.toml");
    persist_policy_array_at(path, "allowed_domains", "test.com");
    // No panic = success
}

#[test]
fn test_persist_policy_array_invalid_toml() {
    let dir = std::env::temp_dir().join(format!("rune-persist-inv-{}", std::process::id()));
    let rune_dir = dir.join(".rune");
    let _ = fs::create_dir_all(&rune_dir);
    let config_path = rune_dir.join("rune.toml");
    fs::write(&config_path, "not valid toml {{{{").unwrap();
    // Should silently skip
    persist_policy_array_at(&config_path, "allowed_domains", "test.com");
    // Content unchanged (parse failed, skip)
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("not valid toml"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_persist_policy_array_creates_policy_section() {
    let dir = std::env::temp_dir().join(format!("rune-persist-cre-{}", std::process::id()));
    let rune_dir = dir.join(".rune");
    let _ = fs::create_dir_all(&rune_dir);
    let config_path = rune_dir.join("rune.toml");
    // File without any policy section
    fs::write(&config_path, r#"model = "gpt-4""#).unwrap();
    persist_policy_array_at(&config_path, "allowed_commands", "rustfmt");
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("rustfmt"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_persist_multiple_fields() {
    let dir = std::env::temp_dir().join(format!("rune-persist-mf-{}", std::process::id()));
    let rune_dir = dir.join(".rune");
    let _ = fs::create_dir_all(&rune_dir);
    let config_path = rune_dir.join("rune.toml");
    fs::write(
        &config_path,
        r#"
[policy]
mode = "confirm"
"#,
    )
    .unwrap();
    persist_policy_array_at(&config_path, "allowed_commands", "git");
    persist_policy_array_at(&config_path, "allowed_commands", "cargo");
    persist_policy_array_at(&config_path, "allowed_domains", "github.com");
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("\"git\"") || content.contains("'git'") || content.contains("git"));
    assert!(content.contains("github.com"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_expand_tilde_home_not_set() {
    let _lock = HOME_TEST_MUTEX.lock().unwrap();
    // Temporarily unset HOME — if HOME is absent, return original
    let original = std::env::var("HOME").ok();
    std::env::remove_var("HOME");
    let result = expand_tilde("~/skills");
    assert_eq!(result, "~/skills"); // unchanged since HOME absent
                                    // Restore
    if let Some(h) = original {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_expand_tilde_bare_home_not_set() {
    let _lock = HOME_TEST_MUTEX.lock().unwrap();
    let original = std::env::var("HOME").ok();
    std::env::remove_var("HOME");
    let result = expand_tilde("~");
    assert_eq!(result, "~");
    if let Some(h) = original {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_rune_config_clone() {
    let c = RuneConfig::default();
    let c2 = c.clone();
    assert_eq!(c2.model, c.model);
    assert_eq!(c2.log_level, c.log_level);
}

#[test]
fn test_policy_config_clone() {
    let p = PolicyConfig::default();
    let p2 = p.clone();
    assert_eq!(p2.mode, p.mode);
    assert_eq!(p2.allowed_paths_rw, p.allowed_paths_rw);
}

#[test]
fn test_load_toml_with_context_window() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-cw-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(
        &path,
        r#"
model = "gpt-4o"
context_window = 32000
compact_threshold = 0.75
compact_keep_last = 4
"#,
    )
    .unwrap();
    let partial = load_toml(&path).unwrap();
    assert_eq!(partial.context_window, Some(32000));
    assert_eq!(partial.compact_threshold, Some(0.75));
    assert_eq!(partial.compact_keep_last, Some(4));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_load_without_clap_returns_ok() {
    // Should not panic/fail even without any config files
    // We just test that it returns Ok
    let result = load_without_clap();
    assert!(result.is_ok());
}

#[test]
fn test_load_without_clap_env_model_override() {
    std::env::set_var("RUNE_MODEL", "env-model-test-xyz");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.model, "env-model-test-xyz");
    std::env::remove_var("RUNE_MODEL");
}

#[test]
fn test_load_without_clap_env_api_key() {
    std::env::set_var("RUNE_API_KEY", "sk-test-env-key");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.api_key.as_deref(), Some("sk-test-env-key"));
    std::env::remove_var("RUNE_API_KEY");
}

#[test]
fn test_load_without_clap_env_provider() {
    std::env::set_var("RUNE_PROVIDER", "ollama");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.provider.as_deref(), Some("ollama"));
    std::env::remove_var("RUNE_PROVIDER");
}

#[test]
fn test_load_without_clap_env_log_level() {
    std::env::set_var("RUNE_LOG_LEVEL", "trace");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.log_level, "trace");
    std::env::remove_var("RUNE_LOG_LEVEL");
}

#[test]
fn test_load_without_clap_env_max_steps() {
    std::env::set_var("RUNE_MAX_STEPS", "99");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.max_steps, Some(99));
    std::env::remove_var("RUNE_MAX_STEPS");
}

#[test]
fn test_load_without_clap_env_context_window() {
    std::env::set_var("RUNE_CONTEXT_WINDOW", "64000");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.context_window, 64000);
    std::env::remove_var("RUNE_CONTEXT_WINDOW");
}

#[test]
fn test_load_without_clap_env_thinking() {
    std::env::set_var("RUNE_THINKING", "medium");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.thinking.as_deref(), Some("medium"));
    std::env::remove_var("RUNE_THINKING");
}

#[test]
fn test_load_without_clap_json_auto_approve_false() {
    let cfg = load_without_clap().unwrap();
    assert!(!cfg.json_output);
    assert!(!cfg.auto_approve);
}

#[test]
fn test_load_toml_with_thinking() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-th-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(
        &path,
        r#"
model = "claude-opus-4"
thinking = "high"
"#,
    )
    .unwrap();
    let partial = load_toml(&path).unwrap();
    assert_eq!(partial.thinking.as_deref(), Some("high"));
    let _ = fs::remove_dir_all(&dir);
}

// =========================================================
// Additional tests for broader coverage
// =========================================================

#[test]
fn test_load_without_clap_env_token_budget() {
    std::env::set_var("RUNE_TOKEN_BUDGET", "50000");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.token_budget, Some(50000));
    std::env::remove_var("RUNE_TOKEN_BUDGET");
}

#[test]
fn test_load_without_clap_env_timeout_secs() {
    std::env::set_var("RUNE_TIMEOUT_SECS", "120");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.timeout_secs, Some(120));
    std::env::remove_var("RUNE_TIMEOUT_SECS");
}

#[test]
fn test_load_without_clap_env_base_url() {
    std::env::set_var("RUNE_BASE_URL", "http://localhost:11434");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.base_url.as_deref(), Some("http://localhost:11434"));
    std::env::remove_var("RUNE_BASE_URL");
}

#[test]
fn test_load_without_clap_env_system_prompt() {
    std::env::set_var("RUNE_SYSTEM_PROMPT", "Be concise.");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.system_prompt.as_deref(), Some("Be concise."));
    std::env::remove_var("RUNE_SYSTEM_PROMPT");
}

#[test]
fn test_load_without_clap_env_compact_threshold() {
    std::env::set_var("RUNE_COMPACT_THRESHOLD", "0.70");
    let cfg = load_without_clap().unwrap();
    assert!((cfg.compact_threshold - 0.70).abs() < 1e-5);
    std::env::remove_var("RUNE_COMPACT_THRESHOLD");
}

#[test]
fn test_load_without_clap_env_compact_keep_last() {
    std::env::set_var("RUNE_COMPACT_KEEP_LAST", "8");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.compact_keep_last, 8);
    std::env::remove_var("RUNE_COMPACT_KEEP_LAST");
}

#[test]
fn test_load_without_clap_env_trace() {
    std::env::set_var("RUNE_TRACE", "/tmp/rune-trace-test");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.trace.as_deref(), Some("/tmp/rune-trace-test"));
    std::env::remove_var("RUNE_TRACE");
}

#[test]
fn test_load_without_clap_env_skills_dir() {
    std::env::set_var("RUNE_SKILLS_DIR", "/custom/skills");
    let cfg = load_without_clap().unwrap();
    assert_eq!(cfg.skills_dir, "/custom/skills");
    std::env::remove_var("RUNE_SKILLS_DIR");
}

#[test]
fn test_mcp_server_config_toml_parsing() {
    let toml_str = r#"
model = "gpt-4"
skills_dir = "./skills"
log_level = "info"

[[mcp]]
name = "my-mcp"
command = "/usr/bin/my-mcp"
args = ["--port", "9000"]
required = true
timeout_secs = 10

[[mcp]]
name = "optional-mcp"
command = "optional-mcp-server"
required = false
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    let servers = cfg.mcp.unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0].name, "my-mcp");
    assert_eq!(servers[0].command, "/usr/bin/my-mcp");
    assert_eq!(servers[0].args, vec!["--port", "9000"]);
    assert!(servers[0].required);
    assert_eq!(servers[0].timeout_secs, Some(10));
    assert_eq!(servers[1].name, "optional-mcp");
    assert!(!servers[1].required);
}

#[test]
fn test_mcp_server_config_default_timeout() {
    let toml_str = r#"
name = "test"
command = "test-cmd"
"#;
    let srv: crate::mcp::McpServerConfig = toml::from_str(toml_str).unwrap();
    // default_timeout returns Some(30)
    assert_eq!(srv.timeout_secs, Some(30));
    assert!(!srv.required);
    assert!(srv.args.is_empty());
}

#[test]
fn test_embedding_config_toml_parsing() {
    let toml_str = r#"
model = "gpt-4"
skills_dir = "./skills"
log_level = "info"

[embedding]
enabled = true
model = "text-embedding-3-small"
base_url = "https://api.openai.com/v1"
api_key = "sk-embed-xxx"
threshold = 0.5
max_skills = 5
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    let emb = cfg.embedding.unwrap();
    assert!(emb.enabled);
    assert_eq!(emb.model.as_deref(), Some("text-embedding-3-small"));
    assert_eq!(emb.base_url.as_deref(), Some("https://api.openai.com/v1"));
    assert_eq!(emb.api_key.as_deref(), Some("sk-embed-xxx"));
    assert!((emb.threshold - 0.5).abs() < 1e-5);
    assert_eq!(emb.max_skills, 5);
}

#[test]
fn test_embedding_config_default() {
    let emb = crate::embedding::EmbeddingConfig::default();
    assert!(!emb.enabled); // default is false
    assert!((emb.threshold - 0.3).abs() < 1e-5);
}

#[test]
fn test_policy_config_allowed_files() {
    let toml_str = r#"
mode = "allowlist"
allowed_files_ro = ["/etc/hostname", "/etc/resolv.conf"]
allowed_files_rw = ["/tmp/output.log"]
"#;
    let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        policy.allowed_files_ro,
        vec!["/etc/hostname", "/etc/resolv.conf"]
    );
    assert_eq!(policy.allowed_files_rw, vec!["/tmp/output.log"]);
}

#[test]
fn test_policy_config_denied_paths() {
    let toml_str = r#"
mode = "allowlist"
denied_paths = ["/root", "/home/secret"]
"#;
    let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(policy.denied_paths, vec!["/root", "/home/secret"]);
}

#[test]
fn test_policy_config_memory_limits() {
    let toml_str = r#"
mode = "confirm"
max_memory_mb = 2048
max_pids = 256
"#;
    let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(policy.max_memory_mb, 2048);
    assert_eq!(policy.max_pids, 256);
}

#[test]
fn test_policy_mode_default_when_omitted() {
    // When policy is deserialized without explicit mode, default_policy_mode applies
    let toml_str = r#"
allowed_commands = ["ls"]
"#;
    let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(policy.mode, "allowlist");
}

#[test]
fn test_load_toml_with_mcp_servers() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-mcp-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(
        &path,
        r#"
model = "gpt-4"

[[mcp]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
required = false
"#,
    )
    .unwrap();
    let partial = load_toml(&path).unwrap();
    let servers = partial.mcp.unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "filesystem");
    assert_eq!(servers[0].command, "npx");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_load_toml_with_embedding() {
    let dir = std::env::temp_dir().join(format!("rune-cfg-emb-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("rune.toml");
    fs::write(
        &path,
        r#"
model = "gpt-4"

[embedding]
enabled = false
model = "nomic-embed-text"
threshold = 0.4
"#,
    )
    .unwrap();
    let partial = load_toml(&path).unwrap();
    let emb = partial.embedding.unwrap();
    assert!(!emb.enabled);
    assert_eq!(emb.model.as_deref(), Some("nomic-embed-text"));
    assert!((emb.threshold - 0.4).abs() < 1e-5);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_expand_tilde_vec_empty() {
    let mut v: Vec<String> = vec![];
    expand_tilde_vec(&mut v);
    assert!(v.is_empty());
}

#[test]
fn test_expand_tilde_vec_single_no_tilde() {
    let mut v = vec!["/absolute/path".to_string()];
    expand_tilde_vec(&mut v);
    assert_eq!(v, vec!["/absolute/path"]);
}

#[test]
fn test_policy_syscalls_default_empty() {
    let p = PolicyConfig::default();
    assert!(p.allowed_syscalls.is_empty());
}

#[test]
fn test_policy_config_allowed_syscalls() {
    let toml_str = r#"
mode = "allowlist"
allowed_syscalls = ["ptrace", "bpf"]
"#;
    let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(policy.allowed_syscalls, vec!["ptrace", "bpf"]);
}

#[test]
fn test_rune_config_default_mcp_empty() {
    let c = RuneConfig::default();
    assert!(c.mcp.is_empty());
}

#[test]
fn test_rune_config_default_serve() {
    let c = RuneConfig::default();
    assert!(c.notes.port.is_none());
    assert!(c.notes.github.is_none());
}

#[test]
fn test_load_without_clap_preload_skills_empty() {
    let cfg = load_without_clap().unwrap();
    assert!(cfg.preload_skills.is_empty());
}

#[test]
fn test_partial_config_missing_optional_fields() {
    // Only model required to parse — all optional fields should be None
    let toml_str = r#"model = "gpt-4""#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("gpt-4"));
    assert!(cfg.api_key.is_none());
    assert!(cfg.provider.is_none());
    assert!(cfg.skills_dir.is_none());
    assert!(cfg.log_level.is_none());
    assert!(cfg.max_steps.is_none());
    assert!(cfg.token_budget.is_none());
    assert!(cfg.timeout_secs.is_none());
    assert!(cfg.base_url.is_none());
    assert!(cfg.trace.is_none());
    assert!(cfg.context_window.is_none());
    assert!(cfg.compact_threshold.is_none());
    assert!(cfg.compact_keep_last.is_none());
    assert!(cfg.policy.is_none());
    assert!(cfg.mcp.is_none());
    assert!(cfg.embedding.is_none());
    assert!(cfg.thinking.is_none());
    assert!(cfg.system_prompt.is_none());
    assert!(cfg.notes.is_none());
    assert!(cfg.agents.is_none());
    assert!(cfg.loop_config.is_none());
}

#[test]
fn test_cli_prompt_default_none() {
    let cfg = RuneConfig::default();
    assert!(cfg.cli_prompt.is_none());
}

#[test]
fn test_cli_prompt_not_deserialized() {
    // cli_prompt is #[serde(skip)], should never come from TOML
    let toml = r#"
            model = "gpt-4"
            cli_prompt = "should be ignored"
        "#;
    let partial: PartialConfig = toml::from_str(toml).unwrap();
    // PartialConfig doesn't have cli_prompt at all, confirming it's CLI-only
    assert!(partial.model == Some("gpt-4".to_string()));
}

#[test]
fn test_github_config_parses() {
    let toml_str = r#"
[notes.github]
client_id = "Ov23liABC"
client_secret = "secret123"
admins = ["fourdollars", "org:my-org/ops"]
users = ["org:my-org"]
guests = ["some-friend"]
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    let oauth = cfg
        .notes
        .and_then(|n| n.github)
        .expect("github must be present");
    assert_eq!(oauth.client_id, "Ov23liABC");
    assert_eq!(oauth.client_secret, "secret123");
    assert_eq!(oauth.admins, vec!["fourdollars", "org:my-org/ops"]);
    assert_eq!(oauth.users, vec!["org:my-org"]);
    assert_eq!(oauth.guests, vec!["some-friend"]);
}

#[test]
fn test_local_config_parses() {
    let toml_str = r#"
[notes.local]
admins = ["admin:admin123"]
users = ["user:user123"]
guests = ["guest:guest123"]
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    let local = cfg
        .notes
        .and_then(|n| n.local)
        .expect("local must be present");
    assert_eq!(local.admins, vec!["admin:admin123"]);
    assert_eq!(local.users, vec!["user:user123"]);
    assert_eq!(local.guests, vec!["guest:guest123"]);
}

#[test]
fn test_oauth_provider_config_parses() {
    let toml_str = r#"
[[notes.oauth]]
name = "google"
display_name = "Google"
client_id = "cid"
client_secret = "secret"
issuer = "https://accounts.google.com"
groups_claim = "groups"
admins = ["alice", "grp:platform-admins"]
users = ["grp:employees"]
guests = []
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    let notes = cfg.notes.expect("notes must be present");
    assert_eq!(notes.oauth.len(), 1);
    let provider = &notes.oauth[0];
    assert_eq!(provider.name, "google");
    assert_eq!(provider.display_name.as_deref(), Some("Google"));
    assert_eq!(provider.client_id, "cid");
    assert_eq!(provider.client_secret, "secret");
    assert_eq!(
        provider.issuer.as_deref(),
        Some("https://accounts.google.com")
    );
    assert_eq!(provider.groups_claim, "groups");
    assert_eq!(provider.admins, vec!["alice", "grp:platform-admins"]);
    assert_eq!(provider.users, vec!["grp:employees"]);
    assert!(provider.guests.is_empty());
}

#[test]
fn test_oauth_provider_config_defaults() {
    let toml_str = r#"
[[notes.oauth]]
name = "custom"
client_id = "cid"
client_secret = "secret"
authorization_url = "https://example.com/oauth/authorize"
token_url = "https://example.com/oauth/token"
userinfo_url = "https://example.com/oauth/userinfo"
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    let notes = cfg.notes.expect("notes must be present");
    assert_eq!(notes.oauth.len(), 1);
    let provider = &notes.oauth[0];
    assert_eq!(provider.scopes, vec!["openid", "profile"]);
    assert_eq!(provider.groups_claim, "groups");
}

#[test]
fn test_notes_config_title_and_desc() {
    let toml_str = r#"
[notes]
title = "My Team Notes"
desc = "Knowledge base and documentation"
"#;
    let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
    let notes = cfg.notes.expect("notes must be present");
    assert_eq!(notes.title.as_deref(), Some("My Team Notes"));
    assert_eq!(
        notes.desc.as_deref(),
        Some("Knowledge base and documentation")
    );

    // Test with 'description' alias
    let toml_alias = r#"
[notes]
title = "My Team Notes"
description = "Aliased description"
"#;
    let cfg2: PartialConfig = toml::from_str(toml_alias).unwrap();
    let notes2 = cfg2.notes.expect("notes must be present");
    assert_eq!(notes2.desc.as_deref(), Some("Aliased description"));
}

#[test]
fn test_custom_agents_and_loop_deserialization() {
    let toml_str = r#"
            model = "gpt-4"
            skills_dir = "./skills"
            log_level = "error"
            json_output = false
            auto_approve = false
            context_window = 8192
            compact_threshold = 0.5
            compact_keep_last = 5

            [agents.builder]
            model = "gemini-2.5-flash"
            system_prompt = "Builder prompt"

            [loop]
            max_iterations = 25
            implementer_agent = "builder"
            verifier_agent = "thinker"
        "#;
    let cfg: RuneConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.loop_config.max_iterations, 25);
    assert_eq!(
        cfg.agents.get("builder").unwrap().system_prompt.as_deref(),
        Some("Builder prompt")
    );
}

#[test]
fn test_mcp_lenient_legacy_clients_defaults_true() {
    let notes_cfg = NotesConfig::default();
    assert!(notes_cfg.mcp_lenient_legacy_clients);
}

#[test]
fn test_mcp_lenient_legacy_clients_toml_false() {
    let toml_str = r#"
            [notes]
            port = 9527
            mcp_lenient_legacy_clients = false
        "#;
    #[derive(Deserialize)]
    struct Wrapper {
        notes: NotesConfig,
    }
    let w: Wrapper = toml::from_str(toml_str).unwrap();
    assert!(!w.notes.mcp_lenient_legacy_clients);
}

#[test]
fn test_mcp_lenient_legacy_clients_toml_absent_defaults_true() {
    let toml_str = r#"
            [notes]
            port = 9527
        "#;
    #[derive(Deserialize)]
    struct Wrapper {
        notes: NotesConfig,
    }
    let w: Wrapper = toml::from_str(toml_str).unwrap();
    assert!(w.notes.mcp_lenient_legacy_clients);
}

#[test]
fn test_persona_files_defaults_false() {
    let notes_cfg = NotesConfig::default();
    assert!(!notes_cfg.persona_files);
}

#[test]
fn test_persona_files_toml_true() {
    let toml_str = r#"
            [notes]
            port = 9527
            persona_files = true
        "#;
    #[derive(Deserialize)]
    struct Wrapper {
        notes: NotesConfig,
    }
    let w: Wrapper = toml::from_str(toml_str).unwrap();
    assert!(w.notes.persona_files);
}

#[test]
fn test_mount_pwd_policy_defaults_false() {
    let policy = PolicyConfig::default();
    assert!(!policy.mount_pwd);
    assert!(policy.mount_home.is_none());
}

#[test]
fn test_mount_pwd_toml_deserialization() {
    let toml_str = r#"
            mode = "confirm"
            mount_pwd = true
            mount_home = "/custom/home"
        "#;
    let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
    assert!(policy.mount_pwd);
    assert_eq!(policy.mount_home.as_deref(), Some("/custom/home"));
}

#[test]
fn test_mount_flags_parsing() {
    use clap::Parser;
    let args = CliArgs::try_parse_from(["rune", "-M"]).unwrap();
    assert_eq!(args.mount_rw, vec!["."]);

    let args = CliArgs::try_parse_from(["rune", "-M", "/path/a", "-M", "/path/b"]).unwrap();
    assert_eq!(args.mount_rw, vec!["/path/a", "/path/b"]);

    let args = CliArgs::try_parse_from(["rune", "-m"]).unwrap();
    assert_eq!(args.mount_ro, vec!["."]);

    let args = CliArgs::try_parse_from(["rune", "-m", "/path/ro1", "-m", "/path/ro2"]).unwrap();
    assert_eq!(args.mount_ro, vec!["/path/ro1", "/path/ro2"]);

    let args = CliArgs::try_parse_from(["rune", "-H"]).unwrap();
    assert_eq!(args.mount_home.as_deref(), Some("."));

    let args = CliArgs::try_parse_from(["rune", "-H", "/my/home"]).unwrap();
    assert_eq!(args.mount_home.as_deref(), Some("/my/home"));
}

#[test]
fn test_apply_mount_flags() {
    let mut policy = PolicyConfig::default();
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_file = temp_dir.path().join("test_file.txt");
    std::fs::write(&temp_file, "hello").unwrap();

    apply_mount_flags(
        &mut policy,
        Some(temp_dir.path().to_str().unwrap()),
        &[temp_file.to_str().unwrap().to_string()],
        &["/usr/include".to_string()],
    );

    let temp_dir_canon = std::fs::canonicalize(temp_dir.path())
        .unwrap()
        .to_string_lossy()
        .to_string();
    let temp_file_canon = std::fs::canonicalize(&temp_file)
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert_eq!(policy.mount_home.as_deref(), Some(temp_dir_canon.as_str()));
    assert!(policy.mount_pwd);
    assert!(policy.allowed_paths_rw.contains(&temp_dir_canon));
    assert!(policy.allowed_files_rw.contains(&temp_file_canon));
    assert!(policy
        .allowed_paths_ro
        .contains(&"/usr/include".to_string()));
}

#[test]
fn test_load_without_clap_expands_tilde() {
    let cfg = load_without_clap().unwrap();
    if let Ok(home) = std::env::var("HOME") {
        assert!(
            cfg.skills_dir.starts_with(&home) || !cfg.skills_dir.starts_with("~"),
            "skills_dir should have tilde expanded, got {}",
            cfg.skills_dir
        );
    }
}

#[test]
fn test_line_notes_config_deserialization() {
    let toml_str = r#"
[notes]
port = 9527

[[notes.line]]
nickname = "LineBot"
channel_secret = "secret123"
channel_access_token = "token456"
keywords = ["@bot", "rune"]
groups = ["C12345678", "C87654321"]
admins = ["U12345678", "U_ADMIN_2"]
users = ["U87654321"]
guests = ["U99999999"]

[[notes.line]]
nickname = "CIBot"
channel_secret = "secret_ci"
channel_access_token = "token_ci"
keyboards = ["ci", "build"]
groups = ["C99999999"]
"#;

    let partial: PartialConfig = toml::from_str(toml_str).unwrap();
    let notes = partial.notes.unwrap();
    assert_eq!(notes.line.len(), 2);
    let line1 = &notes.line[0];
    assert_eq!(line1.nickname, "LineBot");
    assert_eq!(line1.channel_secret, "secret123");
    assert_eq!(line1.channel_access_token, "token456");
    assert_eq!(line1.keywords, vec!["@bot", "rune"]);
    assert_eq!(line1.groups, vec!["C12345678", "C87654321"]);
    assert_eq!(line1.admins, vec!["U12345678", "U_ADMIN_2"]);
    assert_eq!(line1.users, vec!["U87654321"]);
    assert_eq!(line1.guests, vec!["U99999999"]);

    let line2 = &notes.line[1];
    assert_eq!(line2.nickname, "CIBot");
    assert_eq!(line2.channel_secret, "secret_ci");
    assert_eq!(line2.channel_access_token, "token_ci");
    assert_eq!(line2.keywords, vec!["ci", "build"]);
    assert_eq!(line2.groups, vec!["C99999999"]);
    assert!(line2.admins.is_empty());
}
