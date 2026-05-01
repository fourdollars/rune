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

All tools are sandboxed with `unshare --user --net`. Here is a live demonstration:

#### ✅ ALLOWED — Operations that succeed inside the sandbox:

```
ᚱ› read_file /etc/hostname
  ⚙ read_file({"path": "/etc/hostname"})
  ✓ read_file...ok
  → Output: "u"

ᚱ› list_dir /tmp
  ⚙ list_dir({"path": "/tmp"})
  ✓ list_dir...ok
  → Output: (directory listing)

ᚱ› run_terminal_cmd: echo hello
  ⚙ run_terminal_cmd({"cmd": "echo hello"})
  ✓ run_terminal_cmd...ok
  → Output: "hello"

ᚱ› write_file to /tmp/rune_test.txt
  ⚙ write_file({"content": "sandbox test", "path": "/tmp/rune_test.txt"})
  ✓ write_file...ok
  → Output: "Written 12 bytes to /tmp/rune_test.txt"
```

#### ❌ BLOCKED — Operations that fail due to sandbox restrictions:

```
ᚱ› fetch_url https://example.com
  ⚙ fetch_url({"url": "https://example.com"})
  ✗ exit_code: 6
  → Error: curl: (6) Could not resolve host: example.com
  → Reason: Network namespace is isolated, no DNS available

ᚱ› run_terminal_cmd: curl -s --max-time 3 https://1.1.1.1
  ⚙ run_terminal_cmd({"cmd": "curl -s --max-time 3 https://1.1.1.1"})
  ✗ exit_code: 7
  → Error: Failed to connect (no network interface in namespace)
  → Reason: unshare --user --net removes all network interfaces

ᚱ› read_file /etc/shadow
  ⚙ read_file({"path": "/etc/shadow"})
  ✗ exit_code: 1
  → Error: head: cannot open '/etc/shadow': Permission denied
  → Reason: User namespace remaps UID, no privileged access

ᚱ› write_file to /root/evil.txt
  ⚙ write_file({"content": "hack", "path": "/root/evil.txt"})
  ✗ exit_code: 2
  → Error: cannot create /root/evil.txt: Permission denied
  → Reason: User namespace has no write access to /root
```

#### Summary

| Tool | Allowed | Blocked |
|------|---------|---------|
| `read_file` | ✅ Read files in accessible paths | ❌ Cannot read `/etc/shadow` or root-owned files |
| `write_file` | ✅ Write to `/tmp` or project dirs | ❌ Cannot write to `/root`, `/etc`, system dirs |
| `list_dir` | ✅ List any readable directory | ❌ Cannot list restricted directories |
| `run_terminal_cmd` | ✅ Run local commands (no network) | ❌ Cannot make network connections |
| `fetch_url` | ❌ Always blocked | ❌ DNS resolution fails (no network in namespace) |

**Design principle:** All tool invocations are network-isolated by default. File access is restricted by the user namespace UID remapping. This is enforced at the kernel level, not by the application.
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
