# AGENTS.md — Rune Agent Architecture


## Overview

Rune is a zero-trust AI agent runtime written in Rust. The **Agent** is the orchestration layer that manages conversation state, calls the LLM provider, executes tool calls through a sandboxed environment, and repeats until it produces a final answer (or hits a configured limit).

## Run Loop

```
Agent.run(user_input)
  ├─ Resolve @skill references → inject as system message
  ├─ Build system prompt (custom if configured, else default; AGENTS.md always appended)
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
| `system_prompt` | `Option<String>` | None (use default) | Custom system prompt (replaces default; AGENTS.md still appended) |

Per-command sandbox timeout (default 30s) is separate and always enforced.

### Embedding Configuration

```toml
[embedding]
enabled = true
model = "text-embedding-3-small"  # auto-detected from provider
threshold = 0.6                    # cosine similarity threshold
```

### Notes (Serve Mode) Configuration

```toml
[notes]
port = 9527
bind = "0.0.0.0"
thinking = "high"

# GitHub OAuth 2.0 Login
[notes.github]
client_id = "Ov23liABCDEF12345678"
client_secret = "your_client_secret_here"
# GitHub logins or "org:org_name/team" entries (comma-separated)
admins = ["fourdollars", "org:my-org/ops"]
users = ["org:my-org"]
guests = []

# Local Static Password Login
[notes.local]
admins = ["admin:admin123"]
users = ["user:user123"]
guests = ["guest:guest123"]
```

### Concourse Resource Type
- `check` / `in` / `out` now run the same sandboxed agent pipeline used by pipe mode.
- Copilot tokens (`ghu_` / `ghp_`) are auto-refreshed before LLM calls.
- Sandbox allowlists can be provided in the resource source (`network.allowed_domains`, `filesystem.read_write_paths`, `filesystem.read_only_paths`).

## Built-in Tools (10)

| Tool | Sandboxed | Dangerous* | Notes |
|------|-----------|-----------|-------|
| `read_file` | ✓ | ✓† | 32KB truncation |
| `write_file` | ✓ | ✓† | — |
| `list_dir` | ✓ | ✗ | Always auto-approved |
| `execute_cmd` | ✓ | ✓ | Per-cmd timeout, pipeline-aware policy |
| `fetch_url` | ✓ | ✓ | Domain allowlist enforced |
| `inspect_process` | ✓ | ✗ | — |
| `search_chat` | ✓ | ✗ | Semantic search over conversation history |
| `list_markdown` | ✓ | ✗ | List notes/files in serve mode |
| `read_markdown` | ✓ | ✓† | Read a markdown note file |
| `write_markdown` | ✓ | ✓† | Write/update a markdown note file |

*"Dangerous" = requires user confirmation in `confirm` mode.

†**Path-based auto-allow:** `read_file` / `read_markdown` are auto-approved if the resolved path falls under `allowed_paths_ro` or `allowed_paths_rw`. `write_file` / `write_markdown` are auto-approved if under `allowed_paths_rw`. CWD is implicitly added to `allowed_paths_ro` at startup.

## Policy & Confirm Flow

```
is_dangerous_tool(name)?
  └─ Yes → is_already_allowed(name, args)?
              ├─ read_file:   path in allowed_paths_ro ∪ allowed_paths_rw → skip
              ├─ write_file:  path in allowed_paths_rw → skip
              ├─ fetch_url:   domain in allowed_domains → skip
              ├─ execute_cmd: binary in allowed_commands → skip
              └─ Otherwise → prompt Execute? [Y/n]
                               └─ Yes (or auto) → proceed
                               │   resource blocked → separate Add-to-allowlist prompt [Y/n]
                               │                       └─ Yes → persist to ~/.rune/rune.toml
                               └─ No → skip tool call
```

**Policy modes:**
| Mode | Behavior | Default for |
|------|----------|-------------|
| `confirm` | Interactive Y/n prompts | Interactive CLI (auto-detected) |
| `allowlist` | Auto-execute within allowlist, block the rest | Pipe mode, Concourse CI (auto-detected) |
| `unrestricted` | All policy checks bypassed | Opt-in only (`--unrestricted`) |

In Concourse pipelines, override via `source.policy.mode`.

## Sandbox Layers

Up to 5 isolation layers per tool invocation (best-effort; the executor applies available protections in a runtime-dependent order):

1. **cgroups** (`systemd-run`) — memory/PID limits
2. **Network isolation / net-guard** (`unshare --user --net` or internal net-guard subcommand) — namespace-based or domain-allowlist network controls
3. **Seccomp BPF** (internal `_seccomp` subcommand) — syscall filtering
4. **Landlock** (internal `_landlock` subcommand) — filesystem restriction
5. **DNS/Domain allowlist** — selective network access (represented via net-guard/allowed_domains)

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
| `/thinking [level]` | Show/set thinking level |
| `/version` | Show version info |
| `/help` | Show help |
| `/exit` | Exit |
| `/quit` | Exit |

**Serve mode** (starts the Notes web server):
```bash
rune notes --bind 0.0.0.0
```

## Rune Notes (Serve Mode)

`rune notes` starts an embedded HTTP server that serves a collaborative markdown notebook UI from the same binary. Features:

- **Multi-note collections** — multiple named note workspaces, each with its own set of markdown files
- **Per-note markdown files** — each file stored as a plain `.md` file on disk
- **Per-note model override** — each note can use a different LLM model
- **Per-note AI chat** — each note has its own AI assistant with file context
- **File visibility** — each file can be marked public or private
- **Real-time updates** — SSE (Server-Sent Events) for live streaming of AI responses, model changes, user presence, and system events
- **Public preview pages** — rendered markdown with Mermaid diagram support, KaTeX math, and syntax highlighting (highlight.js)
- **GitHub OAuth authentication** — sign in with GitHub; roles resolved from login allowlists and org/team membership
- **Role-based access control** — admin / user / guest roles with distinct permissions
- **Session cookies** — HttpOnly `rune_sid` cookie + JS-readable `rune_session_id` for 24h sessions

### Role Permissions

| Capability | Admin | User | Guest |
|-----------|-------|------|-------|
| Read public notes/files | ✓ | ✓ | ✓ |
| AI chat | ✓ | ✓ | ✗ |
| File CRUD | ✓ | ✓ | ✗ |
| Note create/delete | ✓ | ✗ | ✗ |
| Model switch | ✓ | ✗ | ✗ |
| Visibility toggle | ✓ | ✗ | ✗ |
| SSE thinking/model events | ✓ | ✓ | ✗ |

### Public Pages

| Path | Description |
|------|-------------|
| `/` | Login page (GitHub button & Local form) |
| `/auth/github` | Start GitHub OAuth flow |
| `/auth/github/callback` | GitHub OAuth callback |
| `/auth/local` | Local username/password validation |
| `/auth/logout` | Clear session and redirect to `/` |
| `/auth/denied` | Access denied / not on allowlist |
| `/notes/` | Authenticated editor SPA |
| `/api/me` | Current user info (login, role, avatar) |
| `/api/auth/config` | Exposes enabled authentication methods |
| `/api/public/raw/{note}/{file}` | Raw markdown content (no auth required) |
| `/public/` | Lists all public notes |
| `/public/{note}/` | Lists public files in a note |
| `/public/{note}/{file}` | Rendered markdown preview (client-side with marked.js) |

### SSE Events

The serve mode uses the following SSE event types:

`auth_result` · `model_list` · `model_changed` · `note_list` · `note_switched` · `history` · `file_list` · `file_content` · `file_deleted` · `chat_token` · `chat_done` · `chat_meta` · `chat_message` · `status` · `system` · `users_update` · `error` · `auth_error` · `approval_request` · `archive_done` · `search_results` · `dir_browse_result`

## File Map

```
src/
├── agent/mod.rs     — run loop, confirm flow, skill injection, trace
├── tools/mod.rs     — tool registry, policy enforcement, implementations
├── sandbox/
│   ├── mod.rs           — SandboxExecutor, layer implementations
│   ├── landlock.rs      — Landlock (internal _landlock subcommand)
│   ├── seccomp.rs       — Seccomp BPF (internal _seccomp subcommand)
│   └── net_guard.rs     — Net-guard (internal _net-guard subcommand)
├── provider/        — LLM backends (Copilot, Gemini, OpenRouter, generic)
├── skills/          — SkillLoader + tools_allow/tools_deny enforcement
├── config/          — PolicyConfig, persistence, TOML loading
├── embedding/       — EmbeddingEngine + VectorStore + cosine search
├── concourse/       — AI-driven resource type (sandboxed check/in/out + Copilot refresh)
├── mcp/             — MCP client (stdio JSON-RPC)
├── cli/             — interactive CLI, slash commands, persistent history
├── serve/           — Notes HTTP server (serve mode)
│   ├── mod.rs           — server setup, routing, middleware, session auth
│   ├── api.rs           — REST + SSE API handlers
│   ├── oauth.rs         — GitHub OAuth 2.0: sessions, role resolution, handlers
│   ├── db.rs            — note/file persistence and metadata
│   └── static_files.rs  — embedded static asset serving
├── setup.rs         — `rune init` wizard
web/                 — Frontend assets for serve mode (HTML/CSS/JS, embedded at compile time)
```

## Container Deployment

The container image `ghcr.io/fourdollars/rune` packages all binaries in a Debian-slim base with `curl` and `ca-certificates`.

```bash
# First-time setup — creates config at ~/.rune/rune.toml
docker run --rm -it -v ~/.rune:/home/rune/.rune ghcr.io/fourdollars/rune init

# Interactive agent with config + project directory
docker run --rm -it \
  -v ~/.rune:/home/rune/.rune \
  -v $(pwd):/workspace -w /workspace \
  ghcr.io/fourdollars/rune

# With custom skills
docker run --rm -it \
  -v ~/.rune:/home/rune/.rune \
  -v ./skills:/home/rune/skills \
  -v $(pwd):/workspace -w /workspace \
  ghcr.io/fourdollars/rune

# Pipe mode (non-interactive, one-shot)
echo "Explain this codebase" | docker run --rm -i \
  -v ~/.rune:/home/rune/.rune \
  -v $(pwd):/workspace -w /workspace \
  ghcr.io/fourdollars/rune --json --yes

# Serve mode (Notes web UI)
docker run --rm -it \
  -v ~/.rune:/home/rune/.rune \
  -v $(pwd)/notes:/notes \
  -p 9527:9527 \
  ghcr.io/fourdollars/rune notes --bind 0.0.0.0
```

Concourse CI resource type (symlinks pre-configured at `/opt/resource/{check,in,out}`):
```yaml
resource_types:
  - name: rune-agent
    type: registry-image
    source:
      repository: ghcr.io/fourdollars/rune
      tag: latest
```

## Extending

**New tool:** implement in `src/tools/mod.rs` → add JSON schema to `tool_definitions()` → sandbox handles execution automatically.

**New provider:** implement chat trait in `src/provider/` → register in ProviderRegistry.

**New skill:** create `skills/<name>/SKILL.md` with optional frontmatter (`tools_allow`, description).

## Testing

```bash
cargo test                    # 762 unit tests
./tests/e2e.sh               # 26 E2E integration tests
cargo llvm-cov --summary-only # coverage report
cargo build --release         # release build (~12MB)
```

CI runs: `fmt` → `clippy` → `test+coverage` → `build` → `e2e`.
Coverage uploaded as artifact + displayed in GitHub Actions summary.

**Pre-commit requirement:** All code must pass `cargo fmt --all -- --check` before committing.
Do NOT commit unformatted code. Run `cargo fmt --all` to auto-fix before each commit.

Flags for CI/automation: `--json` (structured output) + `--yes` (auto-approve all).
