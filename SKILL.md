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
cp target/release/rune target/release/rune-seccomp target/release/rune-landlock ~/.local/bin/

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

When stdin is piped into Rune, it runs once and exits immediately. It does not enter interactive mode. If confirm mode would require approval, rerun with `--yes`.

### With specific model
```bash
rune --model gpt-4o
```

## Configuration

Config file: `~/.rune/rune.toml`

```toml
model = "gpt-4o"
api_key = "ghu_..."  # GitHub Copilot PAT (auto-detected)

[policy]
mode = "allowlist"
allowed_commands = ["ls", "cat", "head", "ps", "echo", "date"]
allowed_domains = ["wttr.in"]
```

## Security Model

All tool executions are sandboxed:

| Layer | Protection |
|-------|-----------|
| Network namespace | No outbound connections (unless domain allowed) |
| cgroups v2 | Memory 512MB, max 64 processes |
| Seccomp BPF | Blocks ptrace, mount, kexec_load, bpf, setns |
| Landlock | Only allowed paths readable/writable |
| DNS allowlist | Only listed domains resolvable |

### Command Policy Modes

- `confirm`: Interactive Y/n/A(lways) before dangerous operations
- `allowlist`: Only whitelisted commands can execute
- `unrestricted`: No restrictions (development only)

## Built-in Tools

| Tool | Description |
|------|-------------|
| `read_file` | Read file contents (sandboxed, 32KB limit) |
| `write_file` | Write to file (sandboxed, allowed dirs only) |
| `list_dir` | List directory contents |
| `execute_cmd` | Execute shell command (sandboxed) |
| `fetch_url` | Fetch URL content (requires domain in allowlist) |
| `inspect_process` | Inspect process by PID |

## CLI Commands

| Command | Description |
|---------|-------------|
| `<text>` | Send a prompt to the agent |
| `/help` | Show help |
| `/info` | Session status (model, context, skills) |
| `/info context` | Detailed context breakdown |
| `/policy` | Policy summary |
| `/policy full` | Full sandbox status |
| `/config` | Show configuration |
| `/tools` | List available tools |
| `/skills` | List loaded skills |
| `/trace` | Trace recording status |
| `/compact` | Compress conversation context |
| `/reset` | Clear conversation history |
| `/multi` | Multi-line input (end with `;;`) |
| `/version` | Show version |
| `/clear` | Clear screen |
| `/exit` | Quit |

Use ↑/↓ to browse previous prompts in interactive mode.

## Skills

Rune supports loading skills via `@skill_name` in prompts:

```
ᚱ› Use @sysadmin skill. Show system uptime.
```

Skills are stored in the `skills_dir` (default: `./skills`).



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
      sandbox:
        network:
          allowed_domains: ["api.githubcopilot.com", "wttr.in"]
        filesystem:
          read_write_paths: ["/tmp"]
          read_only_paths: ["/usr", "/bin", "/lib", "/etc"]

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
```
