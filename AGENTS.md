# AGENTS.md — Rune Agent Architecture


## Overview

Rune is a zero-trust AI agent runtime written in Rust. The **Agent** is the orchestration layer that manages conversation state, calls the LLM provider, executes tool calls through a sandboxed environment, and repeats until it produces a final answer (or hits a configured limit).

## Run Loop

```
Agent.run(user_input)
  ├─ Resolve @skill references → inject as system message
  ├─ Append user message
  └─ Loop:
       ├─ Check limits (max_steps, token_budget) — skip if not configured
       ├─ Call LLM provider with messages + tool definitions
       ├─ No tool_calls → return FinalAnswer
       └─ Execute each tool_call (sandboxed) → append result → continue
```

**Stop reasons:** `FinalAnswer` | `MaxSteps` | `TokenBudgetExhausted` | `Error` | `UserInterrupt`

## Configuration Defaults

All limits are **optional** — if not set, the agent runs without artificial caps:

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `max_steps` | `Option<u32>` | None (unlimited) | Max agent loop iterations |
| `token_budget` | `Option<u32>` | None (unlimited) | Cumulative token usage cap |
| `timeout_secs` | `Option<u64>` | None (unlimited) | Global session timeout |

Per-command sandbox timeout (default 30s) is separate and always enforced.

### Embedding Configuration

```toml
[embedding]
enabled = true
model = "text-embedding-3-small"  # auto-detected from provider
threshold = 0.6                    # cosine similarity threshold
```

### Concourse Resource Type
- `check` / `in` / `out` now run the same sandboxed agent pipeline used by pipe mode.
- Copilot tokens (`ghu_` / `ghp_`) are auto-refreshed before LLM calls.
- Sandbox allowlists can be provided in the resource source (`network.allowed_domains`, `filesystem.read_write_paths`, `filesystem.read_only_paths`).

## Built-in Tools (6)

| Tool | Sandboxed | Dangerous* | Notes |
|------|-----------|-----------|-------|
| `read_file` | ✓ | ✓† | 32KB truncation |
| `write_file` | ✓ | ✓† | — |
| `list_dir` | ✓ | ✗ | Always auto-approved |
| `execute_cmd` | ✓ | ✓ | Per-cmd timeout, pipeline-aware policy |
| `fetch_url` | ✓ | ✓ | Domain allowlist enforced |
| `inspect_process` | ✓ | ✗ | — |

*"Dangerous" = requires user confirmation in `confirm` mode.

†**Path-based auto-allow:** `read_file` is auto-approved if the resolved path falls under `allowed_paths_ro` or `allowed_paths_rw`. `write_file` is auto-approved if under `allowed_paths_rw`. CWD is implicitly added to `allowed_paths_ro` at startup.

## Policy & Confirm Flow

```
is_dangerous_tool(name)?
  └─ Yes → is_already_allowed(name, args)?
              ├─ read_file:   path in allowed_paths_ro ∪ allowed_paths_rw → skip
              ├─ write_file:  path in allowed_paths_rw → skip
              ├─ fetch_url:   domain in allowed_domains → skip
              ├─ execute_cmd: binary in allowed_commands → skip
              └─ Otherwise → prompt [Y/n/A(lways)]
                               └─ Always → persist to ~/.rune/rune.toml
```

**Policy modes:**
- `confirm` — interactive Y/n/A prompts (default)
- `allowlist` — only explicitly allowed operations run
- `unrestricted` — no restrictions (dev only, never use with secrets)

## Sandbox Layers

Up to 5 isolation layers per tool invocation:

1. **Network namespace** (`unshare --user --net`) — no network by default
2. **cgroups** (`systemd-run`) — memory/PID limits
3. **Seccomp BPF** (`rune-seccomp`) — syscall filtering
4. **Landlock** (`rune-landlock`) — filesystem restriction
5. **DNS/Domain allowlist** — selective network access

## Skills

Skills are `@name`-referenced bundles (`SKILL.md` + metadata). On reference:
1. SkillLoader resolves and loads the skill content
2. If skill defines `tools_allow`, tool availability is restricted for that turn
3. Content is injected as a system-role message

## Providers

Supported backends (via ProviderRegistry):
- GitHub Copilot (auto token refresh from `ghu_`/`ghp_` keys)
- Google Gemini (`AIza` keys)
- OpenRouter (`sk-or-` keys)
- OpenAI-compatible (generic `sk-` keys or custom `base_url`)
- MCP client (stdio JSON-RPC, configurable in `[mcp_servers]`)

## Trace Recording

When `trace = true`, the agent records structured JSON traces to `.rune/traces/`:
- LLM request/response pairs
- Tool calls with redacted arguments
- Stop reason and exit code

## CLI Commands

| Command | Description |
|---------|-------------|
| `/info` | Session stats (tokens, steps, skills) |
| `/config` | Show active configuration |
| `/tools` | List available tools and policy |
| `/skills` | List discovered skills |
| `/policy` | Show policy summary |
| `/policy full` | Detailed policy with all lists |
| `/trace` | Toggle trace recording |
| `/add-dir <path>` | Add path to allowed_paths_ro |
| `/add-rw-dir <path>` | Add path to allowed_paths_rw |
| `/compact` | Summarize older messages to reduce context |

## File Map

```
src/
├── agent/mod.rs     — run loop, confirm flow, skill injection, trace
├── tools/mod.rs     — tool registry, policy enforcement, implementations
├── sandbox/         — SandboxExecutor, layer implementations
├── provider/        — LLM backends (Copilot, Gemini, OpenRouter, generic)
├── skills/          — SkillLoader + tools_allow/tools_deny enforcement
├── config/          — PolicyConfig, persistence, TOML loading
├── embedding/       — EmbeddingEngine + VectorStore + cosine search
├── concourse/       — AI-driven resource type (sandboxed check/in/out + Copilot refresh)
├── mcp/             — MCP client (stdio JSON-RPC)
├── cli/             — interactive CLI, slash commands, persistent history
├── setup.rs         — `rune init` wizard
└── bin/
    ├── rune-landlock.rs
    └── rune-seccomp.rs
```

## Extending

**New tool:** implement in `src/tools/mod.rs` → add JSON schema to `tool_definitions()` → sandbox handles execution automatically.

**New provider:** implement chat trait in `src/provider/` → register in ProviderRegistry.

**New skill:** create `skills/<name>/SKILL.md` with optional frontmatter (`tools_allow`, description).

## Testing

```bash
cargo test                    # 95 unit tests
./tests/e2e.sh               # 16 E2E integration tests
cargo llvm-cov --summary-only # coverage report
cargo build --release         # release build (~5MB)
```

CI runs: `fmt` → `clippy` → `test+coverage` → `build` → `e2e`.
Coverage uploaded as artifact + displayed in GitHub Actions summary.

Flags for CI/automation: `--json` (structured output) + `--yes` (auto-approve all).
