# Rune ᚱ

A high-performance, zero-trust AI agent built in Rust. Single binary, dual mode: interactive CLI assistant and Concourse CI resource type.

## Features

- **Zero-Trust Sandbox** — All tool executions run inside isolated Linux namespaces (`unshare --user --net`). Network is blocked by default. No exceptions.
- **Tool Calling** — Built-in tools (read_file, write_file, list_dir, run_terminal_cmd, fetch_url) with full function-calling support via OpenAI-compatible APIs.
- **Skills System** — Load contextual abilities via `@skill_name` in prompts. Skills are defined as `SKILL.md` files following the [AgentSkills](https://agentskills.io) specification.
- **Provider Registry** — Connect to any OpenAI-compatible endpoint (OpenAI, OpenRouter, Anthropic proxies, local models). Automatic fallback chain on failure.
- **MCP Client** — Stdio-based JSON-RPC client for Model Context Protocol servers.
- **Concourse CI** — Same binary acts as a Concourse resource type (`check`, `in`, `out`) when invoked via symlink.
- **Tracing** — Structured logging + optional JSON trace files for full observability.

## Quick Start

```bash
# Build
cargo build --release

# Run with an API key
export RUNE_API_KEY="sk-..."
export RUNE_BASE_URL="https://openrouter.ai/api/v1"  # optional, defaults to OpenAI
export RUNE_MODEL="openai/gpt-4o-mini"               # optional, defaults to gpt-4
./target/release/rune
```

## CLI Usage

```
  ┌─────────────────────────────────────────┐
  │  ᚱ  R U N E   v0.1.0                    │
  │  High-performance Zero-Trust AI Agent   │
  └─────────────────────────────────────────┘

ᚱ› Use @sysadmin skill. Show me uptime and memory.
  📚 Loaded skill: sysadmin
  ⚙ run_terminal_cmd({"cmd": "uptime"})
  ✓ run_terminal_cmd...ok
  ⚙ run_terminal_cmd({"cmd": "free -h"})
  ✓ run_terminal_cmd...ok

────────────────────────────────────────────────────────────
- Uptime: 1 day, 17 hours
- Memory: 5.8 GiB used / 30 GiB total (25 GiB available)
────────────────────────────────────────────────────────────
```

### Commands

| Command | Description |
|---------|-------------|
| `<prompt>` | Send a prompt to the agent |
| `run <prompt>` | Explicit prompt execution |
| `/multi` | Multi-line input mode (end with `;;`) |
| `/config` | Show current configuration |
| `/tools` | List available tools |
| `/skills` | List available skills |
| `/trace` | Show trace directory |
| `/version` | Show version info |
| `/reset` | Clear conversation history |
| `/clear` | Clear screen |
| `help` | Show help |
| `exit` | Quit |

## Configuration

Configuration is loaded with the following precedence:

1. CLI flags (`--model`, `--api-key`, `--base-url`, etc.)
2. Environment variables (`RUNE_MODEL`, `RUNE_API_KEY`, `RUNE_BASE_URL`, etc.)
3. Project config (`.rune/rune.toml` in current directory)
4. User config (`~/.rune/rune.toml`)
5. Built-in defaults

### Example `rune.toml`

```toml
model = "openai/gpt-4o-mini"
api_key = "sk-..."
base_url = "https://openrouter.ai/api/v1"
skills_dir = "./skills"
log_level = "info"
max_steps = 100
token_budget = 8192
timeout_secs = 60
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `RUNE_API_KEY` | LLM provider API key |
| `RUNE_BASE_URL` | Provider base URL (default: OpenAI) |
| `RUNE_MODEL` | Model name |
| `RUNE_SKILLS_DIR` | Skills directory path |
| `RUNE_LOG_LEVEL` | Log level (trace/debug/info/warn/error) |
| `RUNE_MAX_STEPS` | Max agent loop iterations |
| `RUNE_TOKEN_BUDGET` | Max tokens per run |
| `RUNE_TIMEOUT_SECS` | Default command timeout |

## Skills

Skills extend the agent's knowledge. Place them in the skills directory:

```
skills/
├── sysadmin/
│   └── SKILL.md
└── launchpad/
    ├── SKILL.md
    └── references/
        ├── basics.md
        └── ...
```

Reference a skill in your prompt with `@skill_name`:

```
ᚱ› Use @sysadmin skill. Check disk usage.
  📚 Loaded skill: sysadmin
  ...
```

## Zero-Trust Sandbox

Every tool invocation runs inside an isolated environment:

```
unshare --user --net -- sh -c '<command>'
```

- **Network isolated**: No DNS, no outbound connections
- **User namespace**: Separate UID/GID mapping
- **Timeout enforced**: Commands killed after deadline
- **Graceful degradation**: Falls back to direct execution (with warning) if namespaces unavailable

This means sandboxed commands **cannot** access the network. This is by design.


### Sandbox Demo

All tools are sandboxed. Here is a live demonstration:

```
ᚱ› I want to test the sandbox. Please run these 3 commands and report each result:
    1. echo hello
    2. curl -s --max-time 3 https://example.com
    3. cat /etc/shadow

  ⚙ run_terminal_cmd({"cmd":"echo hello"})
  ✓ run_terminal_cmd...ok

  ⚙ run_terminal_cmd({"cmd":"curl -s --max-time 3 https://example.com"})
  ✗ exit_code: 6 — Could not resolve host (network blocked)

  ⚙ run_terminal_cmd({"cmd":"cat /etc/shadow"})
  ✗ exit_code: 1 — Permission denied

────────────────────────────────────────────────────────────
Results:
1. echo hello           → ✅ Succeeded (output: "hello")
2. curl https://...     → ❌ Failed (exit 6: network isolated, DNS unavailable)
3. cat /etc/shadow      → ❌ Failed (exit 1: permission denied in user namespace)
────────────────────────────────────────────────────────────
```

| Test | Result | Reason |
|------|--------|--------|
| Basic command | ✅ Pass | No network or privilege needed |
| Network access | ❌ Blocked | `unshare --user --net` isolates network namespace |
| Sensitive file | ❌ Denied | User namespace remaps UID, no root access |

## Concourse CI Resource Type

The same binary works as a Concourse resource when invoked as `check`, `in`, or `out`:

```yaml
resource_types:
  - name: rune-agent
    type: docker-image
    source:
      repository: my-registry/rune
      tag: debian

resources:
  - name: ai-dev
    type: rune-agent
    source:
      api_key: ((api_key))
      sandbox:
        network:
          allowed_domains: ["github.com", "api.openai.com"]
```

### Docker Images

```bash
make build-debian   # debian:bookworm-slim based
make build-alpine   # alpine:latest based
make build-ubuntu   # ubuntu:24.04 based
```

## Architecture

```
src/
├── main.rs          — Entry point, argv[0] routing, tracing init
├── agent/mod.rs     — Agent loop, LLM ↔ tool orchestration
├── cli/mod.rs       — Interactive CLI with spinner + commands
├── concourse/mod.rs — Concourse check/in/out handlers
├── config/mod.rs    — Layered config loading
├── mcp/mod.rs       — MCP client (stdio JSON-RPC)
├── precommands.rs   — Pre-command execution
├── provider/mod.rs  — LLM provider trait + OpenAI-compatible impl
├── sandbox/mod.rs   — Linux namespace isolation
├── skills/mod.rs    — SKILL.md loader + @skill refs
├── tools/mod.rs     — Built-in tools (all sandboxed)
└── trace/mod.rs     — JSON trace writer
```

## Development

```bash
# Check
cargo check

# Test
cargo test

# Build release
cargo build --release

# Run with trace logging
RUST_LOG=debug ./target/release/rune
```

## Requirements

- Rust 1.75+ (tested on 1.95)
- Linux (for sandbox namespace support)
- `curl` on PATH (used by provider and fetch_url)

## License

MIT
