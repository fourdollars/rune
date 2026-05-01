# Changelog

All notable changes to this project will be documented in this file.
Format based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.1.0] - Unreleased

### Added

- **CLI Mode**: Interactive REPL with runic banner, colored output, spinner
- **Commands**: /help, /config, /tools, /skills, /info, /trace, /version, /reset, /clear, /multi, /exit
- **Agent Loop**: LLM → tool-calling → LLM cycle with configurable max_steps and token_budget
- **Built-in Tools** (6): read_file, write_file, list_dir, run_terminal_cmd, fetch_url, inspect_process
- **Skills System**: SKILL.md loading via @skill_name refs, frontmatter parsing, multi-path search
- **LLM Providers**: OpenAI-compatible, GitHub Copilot (auto token refresh), Google Gemini
- **Provider Registry**: Multi-provider with automatic fallback chain
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
- **E2E Tests**: 26 end-to-end tests + 18 unit tests

### Security

- **Zero-Trust Sandbox**: ALL tool executions run in isolated Linux namespaces
- **Network Isolation**: `unshare --user --net` blocks all outbound connections by default
- **User Namespace**: UID remapping prevents access to privileged files
- **Seccomp**: `setpriv --no-new-privs` prevents privilege escalation
- **cgroups v2**: Memory and process limits via systemd-run --scope
- **Landlock**: Kernel detection ready (helper binary pending)
- **DNS Allowlist**: Only explicitly whitelisted domains can be accessed
- **File Truncation**: read_file capped at 32KB to prevent memory exhaustion
- **Timeout Enforcement**: All sandboxed commands have configurable deadlines
