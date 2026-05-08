---
name: rune
description: >-
  Interact with the Rune AI Agent CLI for sandboxed command execution, file operations,
  and URL fetching with zero-trust security. Use when you need to run commands in an
  isolated environment with network restrictions, filesystem sandboxing, and syscall filtering.
metadata:
  version: "0.1.0"
  repository: "https://github.com/fourdollars/rune"
  license: "MIT"
---

# Rune — Zero-Trust AI Agent

## Overview

Rune is a high-performance, zero-trust AI agent built in Rust. It provides sandboxed tool execution with 5 isolation layers: network namespace, cgroups, seccomp BPF, Landlock filesystem, and DNS allowlist.

Use this skill when you need to:
- Execute commands in a security-isolated environment
- Read/write files with filesystem restrictions
- Fetch URLs with domain-level access control
- Inspect processes safely

## Installation

```bash
# From source
git clone https://github.com/fourdollars/rune.git
cd rune && cargo build --release
cp target/release/rune ~/.local/bin/

# First-time setup
rune init
```

## Usage

### Interactive Mode
```bash
rune
```

Use ↑/↓ to browse previous prompts in interactive mode.

### JSON Mode (for programmatic use)
```bash
echo "your prompt here" | rune --json
# Output: {"answer":"...","tools_used":[...],"steps":N,"tokens":N}
```

### Non-interactive pipe mode
```bash
echo "Get weather for Taoyuan from wttr.in" | rune --json --yes
```

When stdin is piped into Rune, it runs once and exits immediately. It does not enter interactive mode. If confirm mode would require approval, rerun with `--yes`. In pipe mode, the policy defaults to `allowlist` unless explicitly overridden with `--unrestricted`.

### With specific model/provider
```bash
rune --model gpt-4o
rune --provider gemini
```

## CLI Flags

| Flag | Env Var | Description |
|------|---------|-------------|
| `--provider <name>` | `RUNE_PROVIDER` | LLM provider [github-copilot, gemini, openai, openrouter, ollama, anthropic] |
| `--model <name>` | `RUNE_MODEL` | Model name [e.g. gpt-4o, gemini-2.0-flash, claude-3.5-sonnet] |
| `--api-key <key>` | `RUNE_API_KEY` | API key for the LLM provider |
| `--base-url <url>` | `RUNE_BASE_URL` | Provider base URL (auto-detected for Copilot/Gemini) |
| `--unrestricted` | — | Disable all security policy checks (sandbox, allowlists, confirm prompts) |
| `--yes`, `-y` | — | Auto-approve dangerous tool calls (does NOT bypass policy allowlist) |
| `--max-steps <n>` | `RUNE_MAX_STEPS` | Maximum agent loop iterations [default: 50, 0 = unlimited] |
| `--token-budget <n>` | `RUNE_TOKEN_BUDGET` | Maximum tokens per run [default: 256k, 0 = unlimited] |
| `--timeout-secs <n>` | `RUNE_TIMEOUT_SECS` | Command timeout in seconds [default: 30, 0 = unlimited] |
| `--json` | — | Output in JSON format (machine-readable, for scripting) |
| `--trace <dir>` | `RUNE_TRACE` | Enable trace recording to specified directory |
| `--compact-keep-last <n>` | `RUNE_COMPACT_KEEP_LAST` | Keep last N messages during compact [default: 6] |
| `--log-level <lvl>` | `RUNE_LOG_LEVEL` | Log level [trace, debug, info, warn, error] |
| `--thinking <level>` | `RUNE_THINKING` | Thinking/reasoning effort level [off\|low\|medium\|high\|xhigh] |
| `--skills-dir <path>` | `RUNE_SKILLS_DIR` | Directory containing skill definitions |
| `--skills a,b` | `RUNE_SKILLS` | Preload specific skills (comma-separated names); disables @ref and semantic discovery |

## Configuration

Config file: `~/.rune/rune.toml` (also reads `./.rune/rune.toml` for per-project overrides)

```toml
model = "gpt-4o"
provider = "github-copilot"
api_key = "ghu_..."        # GitHub Copilot PAT (auto-detected)
thinking = "high"          # off|low|medium|high|xhigh

[policy]
mode = "confirm"           # confirm | allowlist | unrestricted
allowed_commands = ["ls", "cat", "head", "ps", "echo", "date"]
allowed_domains = ["wttr.in", "*.github.com"]
allowed_paths_rw = ["/tmp", "/workspace"]
allowed_paths_ro = ["/bin", "/usr", "/lib"]
allowed_files_ro = ["/home/user/.netrc"]
allowed_files_rw = ["/tmp/data.json"]
denied_paths = ["/root", "/etc/shadow"]

[embedding]
enabled = true             # default: false
model = "text-embedding-3-small"
threshold = 0.3            # similarity threshold for skill matching

[[mcp_servers]]
name = "my-server"
command = "npx"
args = ["-y", "@my/mcp-server"]
timeout_secs = 30
required = false
```

## CLI Commands (Interactive Mode)

### Input

| Command | Description |
|---------|-------------|
| `<text>` | Send a prompt to the agent |
| `/multi` | Enter multi-line mode (end with `;;`) |
| `/image <path>` or `/img <path>` | Attach an image for vision models |

### Session

| Command | Description |
|---------|-------------|
| `/info` | Show session status (model, provider, context) |
| `/info context` | Show detailed context usage (tokens, %) |
| `/compact` | Compact (summarize) older conversation context |
| `/reset` | Reset conversation history |
| `/clear` | Clear the screen |

### Configuration

| Command | Description |
|---------|-------------|
| `/config` | Show current configuration |
| `/policy` | Show policy summary |
| `/policy full` | Show full sandbox + policy status |
| `/tools` | List available built-in tools |
| `/skills` | List loaded skills |
| `/skills full` | Show skill details (frontmatter, tools) |
| `/mcps` | MCP servers summary |
| `/mcps full` | MCP servers full details (tools, schema) |
| `/trace` | Show trace output directory |

### Runtime

| Command | Description |
|---------|-------------|
| `/add-dir <path>` | Add directory to read-only paths (saved to config) |
| `/add-rw-dir <path>` | Add directory to read-write paths (saved to config) |

### Other

| Command | Description |
|---------|-------------|
| `/thinking [level]` | Show/set thinking level: off\|low\|medium\|high\|xhigh |
| `/version` | Show version info |
| `/help` | Show help |
| `/exit`, `/quit` | Exit the CLI |

Use ↑/↓ to browse previous prompts in interactive mode.

## Security Model

All tool executions are sandboxed:

| Layer | Protection |
|-------|-----------|
| Network namespace | No outbound connections (unless domain allowed) |
| cgroups v2 | Memory limit (default 512 MB), max processes (default 64) |
| Seccomp BPF | Blocks ptrace, mount, kexec_load, bpf, setns, unshare |
| Landlock | Only allowed paths readable/writable |
| DNS allowlist | Only listed domains resolvable (wildcard support) |

### Command Policy Modes

| Mode | Behavior |
|------|----------|
| `confirm` | Interactive CLI default. Prompts `Execute? [Y/n]` before dangerous tool calls. When a command is blocked (permission denied), Rune uses strace to probe which files were denied and prompts: `Add to allowed_files_ro/rw? [Y/n]`. Approved paths are persisted to config. |
| `allowlist` | Pipe/CI default. Only whitelisted commands can execute; everything else is blocked silently. Blocked resources (domains, commands) trigger interactive allowlist prompts in interactive mode. |
| `unrestricted` | All policy checks skipped — sandbox, allowlists, confirm prompts are disabled. Development only. |

### Automatic Allowlist Expansion (confirm mode)

When a tool call is blocked in `confirm` mode, Rune automatically:

1. Detects the permission error (command not allowed, domain blocked, file access denied)
2. For file access: re-runs the command under `strace` to identify exactly which files triggered EACCES
3. Prompts the user: `Add to allowed_files_ro/rw? [Y/n]` (or `allowed_domains` / `allowed_commands`)
4. On approval, persists the entry to `~/.rune/rune.toml`

## Built-in Tools

| Tool | Description |
|------|-------------|
| `read_file` | Read file contents (sandboxed, 32KB limit) |
| `write_file` | Write to file (sandboxed, allowed dirs only) |
| `list_dir` | List directory contents |
| `execute_cmd` | Execute shell command (sandboxed) |
| `fetch_url` | Fetch URL content (requires domain in allowlist) |
| `inspect_process` | Inspect process by PID |

## Skills

Rune supports loading skills via `@skill_name` in prompts:

```
ᚱ› Use @sysadmin skill. Show system uptime.
```

Skills are stored in the `skills_dir` (default: `./skills`). Skill files (`SKILL.md`) are discovered recursively up to 3 levels deep within the skills directory.

### Preloading Skills

Use `--skills a,b` to preload specific skills by name at startup. When set, only the specified skills are injected into context; `@ref` dynamic loading and semantic search are disabled.

### Semantic Skill Matching

When the `[embedding]` section is enabled in config, skills can be matched semantically based on the user's prompt — Rune automatically selects relevant skills without explicit `@ref` invocation.

## Container Usage

Run Rune via Docker without installing locally:

```bash
# First-time setup
docker run --rm -it -v ~/.rune:/home/rune/.rune ghcr.io/fourdollars/rune init

# Interactive with skills
docker run --rm -it \
  -v ~/.rune:/home/rune/.rune \
  -v ./skills:/home/rune/skills \
  ghcr.io/fourdollars/rune

# Run against a project directory
docker run --rm -it \
  -v ~/.rune:/home/rune/.rune \
  -v $(pwd):/workspace -w /workspace \
  ghcr.io/fourdollars/rune
```

## Concourse CI Resource Type

Rune can be used as a Concourse CI resource type. Minimal example:

```yaml
resource_types:
  - name: rune-agent
    type: registry-image
    source:
      repository: ghcr.io/fourdollars/rune
      tag: latest

resources:
  - name: weather
    type: rune-agent
    check_every: 1h
    source:
      api_key: ((copilot-pat))
      model: gpt-4o-mini
      prompt: "Fetch the weather for Taoyuan from wttr.in using curl."
      policy:
        allowed_commands: ["curl"]
        allowed_domains: ["wttr.in"]

jobs:
  - name: weather-check
    plan:
      - get: weather
        trigger: true
      - task: show
        config:
          platform: linux
          image_resource:
            type: registry-image
            source: { repository: ghcr.io/fourdollars/rune, tag: latest }
          inputs: [{name: weather}]
          run:
            path: sh
            args: [-c, "cat weather/response.txt"]
```

## Examples

```bash
# Quick question
echo "What time is it?" | rune --json

# With a skill
echo "Use @sysadmin. Check disk usage." | rune --json --yes

# Fetch weather (requires wttr.in in allowed_domains)
echo "Get weather for Tokyo from wttr.in" | rune --json --yes

# Use specific thinking level
rune --thinking high

# Preload specific skills only
rune --skills jira,launchpad
```
