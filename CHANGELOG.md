# Changelog

All notable changes to this project will be documented in this file.
Format based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.1.0] - Unreleased

### Added

- **CLI Mode**: Interactive REPL with runic banner, colored output, spinner
- **Prompt History**: ↑/↓ to browse previous prompts (via rustyline)
- **Commands**: /help, /config, /tools, /skills, /info, /trace, /version, /reset, /clear, /multi, /compact, /policy, /exit
- **Agent Loop**: LLM → tool-calling → LLM cycle with configurable max_steps and token_budget
- **Built-in Tools** (6): read_file, write_file, list_dir, execute_cmd, fetch_url, inspect_process
- **Skills System**: SKILL.md loading via @skill_name refs, frontmatter parsing, multi-path search
- **LLM Providers**: OpenAI-compatible, GitHub Copilot (auto token refresh), Google Gemini
- **Provider Registry**: Multi-provider with fallback chain (transient errors only)
- **MCP Client**: Stdio JSON-RPC client for Model Context Protocol servers
- **Concourse CI Resource Type**: check/in/out via argv[0] symlink routing
- **Configuration**: Layered loading (CLI flags > env vars > .rune/rune.toml > ~/.rune/rune.toml > defaults)
- **Setup Wizard**: `rune init` interactive config generator
- **DNS Allowlist**: Domain-based whitelist for fetch_url with wildcard support
- **Trace Recording**: --trace flag, JSON trace files in .rune/traces/
- **Structured Logging**: tracing-subscriber with env filter
- **UTF-8 Support**: Full CJK and unicode character handling
- **Pre-commands**: Sequential execution with bail-on-failure
- **Docker**: Debian/Alpine/Ubuntu multi-stage Dockerfiles
- **E2E Tests**: end-to-end tests + unit tests
- **JSON Output**: `--json` flag for machine-readable output
- **Auto-Approve**: `--yes` / `-y` flag to skip confirm prompts
- **Pipe Mode**: Non-interactive one-shot execution when stdin is piped

### Security

- **Zero-Trust Sandbox**: ALL tool executions run in isolated Linux namespaces
- **Network Isolation**: `unshare --user --net` blocks all outbound connections by default
- **User Namespace**: UID remapping prevents access to privileged files
- **Seccomp BPF**: Real syscall filter via `rune-seccomp` helper binary
- **cgroups v2**: Memory and process limits via systemd-run --scope
- **Landlock**: Filesystem restriction via `rune-landlock` helper binary
- **DNS Allowlist**: Only explicitly whitelisted domains can be accessed
- **Policy Fail-Fast**: Blocked tool calls immediately stop the agent (no further LLM calls)
- **File Truncation**: read_file capped at 32KB to prevent memory exhaustion
- **Timeout Enforcement**: All sandboxed commands have configurable deadlines
