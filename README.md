# Rune ᚱ

A high-performance, zero-trust AI agent built in Rust. Single binary, dual mode: interactive CLI assistant and Concourse CI resource type.

## Features

- **Zero-Trust Sandbox** — ALL tool executions run through 5 isolation layers:
  - Network namespace (`unshare --user --net`)
  - cgroups v2 resource limits (`systemd-run --scope`)
  - Seccomp BPF syscall filter (`rune-seccomp`)
  - Landlock filesystem restriction (`rune-landlock`)
  - DNS allowlist (domain-level network control)
- **Tool Calling** — 6 built-in tools: `read_file`, `write_file`, `list_dir`, `execute_cmd`, `fetch_url`, `inspect_process`
- **Command Policy** — Three modes: `confirm` (interactive Y/n/A), `allowlist` (whitelist only), `unrestricted`
- **Skills System** — Load contextual abilities via `@skill_name` in prompts
- **Provider Registry** — GitHub Copilot (auto token refresh), OpenRouter, Google Gemini, any OpenAI-compatible
- **MCP Client** — Stdio-based JSON-RPC client for Model Context Protocol servers
- **Concourse CI** — Same binary acts as a resource type (`check`, `in`, `out`) via symlink
- **Trace Recording** — JSON trace files with sensitive info redaction
- **JSON Output** — `--json` flag for machine-readable output

## Quick Start

```bash
# Build (produces 3 binaries: rune, rune-seccomp, rune-landlock)
cargo build --release

# Interactive setup
./target/release/rune init

# Or configure manually
mkdir -p ~/.rune
cat > ~/.rune/rune.toml << 'EOF'
model = "gpt-4o"
api_key = "ghu_your_github_copilot_pat"
skills_dir = "./skills"

[policy]
mode = "confirm"
allowed_domains = ["wttr.in"]
allowed_commands = ["ls", "cat", "head", "ps", "echo", "uname", "free", "df", "date", "hostname"]
EOF

# Run
./target/release/rune
```

## CLI Usage

```
        ᛟ ᚺ ᛊ ᛏ ᛒ ᛖ ᚹ ᛗ ᛚ ᛝ ᛟ
    ┌───────────────────────────────────┐
    │    ᚱ  ᚢ  ᚾ  ᛖ                     │
    │    Zero-Trust AI Agent            │
    │    v0.1.0 ⚡ sandboxed            │
    └───────────────────────────────────┘
        ᛟ ᚺ ᛊ ᛏ ᛒ ᛖ ᚹ ᛗ ᛚ ᛝ ᛟ

ᚱ› Use @sysadmin skill. Show me uptime and memory.
  📚 Loaded skill: sysadmin
  ⚙ execute_cmd({"cmd": "uptime"})
  ⚠ Execute? [Y/n/A(lways)] y
  ✓ execute_cmd...ok
  ⚙ execute_cmd({"cmd": "free -h"})
  ✓ execute_cmd...ok

────────────────────────────────────────────────────────────
- Uptime: 2 days, 1 hour
- Memory: 5.8 GiB used / 30 GiB total
────────────────────────────────────────────────────────────
  📋 commands executed: 2
    ▸ uptime
    ▸ free -h
  ⚡ [2 steps | 797 tokens | 2 tool calls]
```

### Commands

| Command | Description |
|---------|-------------|
| `<text>` | Send a prompt to the agent |
| `/help` | Show help |
| `/info` | Current session status (model, context, skills) |
| `/info context` | Detailed context breakdown |
| `/policy` | Show policy summary |
| `/policy full` | Full sandbox status |
| `/config` | Show configuration |
| `/tools` | List available tools |
| `/skills` | List available skills |
| `/trace` | Trace recording status |
| `/compact` | Compress conversation context |
| `/reset` | Clear conversation history |
| `/multi` | Multi-line input (end with `;;`) |
| `/version` | Show version |
| `/clear` | Clear screen |
| `/exit` | Quit |

## Configuration

```toml
# ~/.rune/rune.toml
model = "gpt-4o"
api_key = "ghu_..."          # GitHub Copilot (auto-detected)
# api_key = "AIza..."        # Google Gemini
# api_key = "sk-or-..."      # OpenRouter
# base_url = "https://..."   # Custom endpoint (not needed for Copilot)

skills_dir = "./skills"
log_level = "warn"
max_steps = 20
token_budget = 16384
timeout_secs = 30
trace = false

[policy]
mode = "confirm"             # confirm | allowlist | unrestricted
allowed_commands = ["ls", "cat", "head", "ps", "echo"]
allowed_domains = ["wttr.in", "api.github.com"]
denied_syscalls = ["ptrace", "mount", "kexec_load", "bpf", "setns"]
allowed_paths_rw = ["/tmp"]
allowed_paths_ro = ["/bin", "/usr", "/lib"]
denied_paths = ["/root", "/etc/shadow"]
max_memory_mb = 512
max_pids = 64

# MCP servers (optional)
# [[mcp_servers]]
# name = "example"
# command = "node"
# args = ["server.js"]
# required = false
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `RUNE_API_KEY` | LLM provider API key |
| `RUNE_MODEL` | Model name |
| `RUNE_BASE_URL` | Provider base URL |
| `RUNE_POLICY_MODE` | Policy mode override |
| `RUNE_LOG_LEVEL` | Log level |
| `RUNE_TRACE` | Enable trace (true/false) |
| `RUNE_JSON_OUTPUT` | JSON output mode |

## Zero-Trust Sandbox

Every tool invocation passes through up to 5 isolation layers:

```
┌─────────────────────────────────────────────┐
│  Layer 1: cgroups (memory + pids limits)    │
│  Layer 2: Network namespace (isolated)      │
│  Layer 3: Seccomp BPF (syscall filter)      │
│  Layer 4: Landlock (filesystem restriction) │
│  Layer 5: DNS allowlist (domain control)    │
└─────────────────────────────────────────────┘
```

### Sandbox Demo

#### ✅ ALLOWED — Operations that succeed:

```
ᚱ› (read /etc/hostname)
  ⚙ read_file({"path": "/etc/hostname"})
  ✓ read_file...ok → "u"

ᚱ› (write to /tmp)
  ⚙ write_file({"path": "/tmp/test.txt", "content": "hello"})
  ✓ write_file...ok → "Written 5 bytes"

ᚱ› (run allowed command)
  ⚙ execute_cmd({"cmd": "echo hello"})
  ✓ execute_cmd...ok → "hello"
```

#### ❌ BLOCKED — Operations that fail:

```
ᚱ› (fetch non-allowed URL)
  ⚙ fetch_url({"url": "https://example.com"})
  ✗ BLOCKED: domain 'example.com' is not in allowed_domains

ᚱ› (run non-allowed command in allowlist mode)
  ⚙ execute_cmd({"cmd": "rm -rf /"})
  ✗ BLOCKED by policy: command 'rm' is not in allowed_commands

ᚱ› (read sensitive file)
  ⚙ read_file({"path": "/etc/shadow"})
  ✗ Permission denied (Landlock + user namespace)

ᚱ› (ptrace attempt inside sandbox)
  → Seccomp BPF: Operation not permitted
```

### Command Policy

| Mode | Behavior |
|------|----------|
| `confirm` | Ask user Y/n/A(lways) before dangerous tools |
| `allowlist` | Only whitelisted commands can execute |
| `unrestricted` | No restrictions (development only) |

## JSON Output Mode

```bash
echo "What is 2+2?" | rune --json true
```

```json
{"answer":"4","steps":1,"tokens":348,"tools_used":[]}
```

## Skills

```
skills/
├── sysadmin/
│   └── SKILL.md
└── launchpad/
    ├── SKILL.md
    └── references/
```

Use `@skill_name` in prompts:
```
ᚱ› Use @sysadmin skill. Check disk usage.
  📚 Loaded skill: sysadmin
```

## Concourse CI Resource Type

```yaml
resource_types:
  - name: rune-agent
    type: docker-image
    source: { repository: my-registry/rune, tag: debian }

resources:
  - name: ai-dev
    type: rune-agent
    source:
      api_key: ((api_key))
      sandbox:
        network: { allowed_domains: ["github.com"] }
        filesystem: { read_write_paths: ["/workspace"] }
```

## Architecture

```
src/
├── main.rs              — Entry point, routing
├── agent/mod.rs         — Agent loop, tool orchestration, confirm flow
├── cli/mod.rs           — Interactive CLI, commands, JSON mode
├── concourse/mod.rs     — Concourse check/in/out
├── config/mod.rs        — Layered config + PolicyConfig
├── mcp/mod.rs           — MCP client (stdio JSON-RPC)
├── precommands.rs       — Pre-command execution
├── provider/mod.rs      — LLM providers + retry backoff
├── sandbox/mod.rs       — 5-layer sandbox orchestration
├── setup.rs             — rune init wizard
├── skills/mod.rs        — SKILL.md loader
├── tools/mod.rs         — 6 built-in tools (all sandboxed)
├── trace/mod.rs         — JSON trace + redaction
└── bin/
    ├── rune-seccomp.rs  — Seccomp BPF helper
    └── rune-landlock.rs — Landlock filesystem helper
```

## Development

```bash
cargo build --release    # Build all 3 binaries
cargo test               # Unit tests (18)
./tests/e2e.sh           # E2E tests (26)
make check-all           # Both
```

## Requirements

- Rust 1.75+ (tested on 1.95)
- Linux kernel 5.13+ (for Landlock ABI)
- `curl` on PATH

## License

MIT
