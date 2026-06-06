# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Rune is a single-binary Rust application that is **three things in one**, chosen at runtime:

1. **Interactive / pipe CLI** — zero-trust AI agent (default invocation `rune` / `echo ... | rune`)
2. **Concourse CI resource type** — same binary, behavior selected by `argv[0]` (`check` / `in` / `out` symlinks)
3. **Notes web server** — `rune notes` starts an embedded Axum HTTP server with collaborative Markdown UI

`src/main.rs` dispatches all three modes (plus internal sandbox subcommands) before clap parses anything. When changing entry-point behavior, check the dispatch ladder in `main.rs` first — clap does not see those args.

## Build, Test, Lint

```bash
cargo build --release           # release binary (~12MB, opt-level=z, LTO, strip)
cargo test --all --no-fail-fast # unit tests
cargo fmt --all                 # required before every commit (CI enforces --check)
cargo clippy --all-targets -- -D warnings  # CI fails on any warning
./tests/e2e.sh                  # E2E tests; require release build first
make check-all                  # unit + e2e
cargo llvm-cov --summary-only   # coverage (matches CI)
```

Run a single unit test: `cargo test <test_name>` (use module path for disambiguation, e.g. `cargo test agent::tests::run_loop`).

CI pipeline order (`.github/workflows/ci.yml`): `fmt --check` → `clippy -D warnings` → `llvm-cov` → `cargo build --release` → `./tests/e2e.sh`. A formatting or clippy failure short-circuits everything else.

**Pre-commit requirement:** code must pass `cargo fmt --all -- --check`. Run `cargo fmt --all` before committing.

## Sandbox Architecture (most non-obvious part)

Sandboxing is implemented by **re-exec'ing the same binary** with hidden subcommands. The dispatch in `main.rs` handles these synchronously before tokio starts:

| argv[1] | File | Purpose |
|---------|------|---------|
| `_landlock` | `src/sandbox/landlock.rs` | Apply Landlock ruleset, then exec the target command |
| `_seccomp` | `src/sandbox/seccomp.rs` | Install seccomp-BPF filter, then exec |
| `_net-guard` | `src/sandbox/net_guard.rs` | Seccomp user-notification network filter |

`src/sandbox/mod.rs` (`SandboxExecutor`) composes up to 5 layers around each tool invocation: cgroups (`systemd-run --scope`), network isolation (`unshare --user --net` or net-guard), seccomp BPF, Landlock, DNS/domain allowlist. Layers are **best-effort** — if a kernel feature is missing the layer is skipped, not failed. When debugging "command works outside sandbox but fails inside," check which layers were actually applied (trace output / `/policy full`).

## Agent Run Loop

`src/agent/mod.rs` (~4800 lines) is the orchestration core. The loop:

1. Resolve `@skill_name` references in the prompt → inject as system message
2. Build system prompt (custom override or default) — **`AGENTS.md` is always appended** to whatever system prompt is active
3. Call provider → if no `tool_calls`, return `FinalAnswer`; else execute each via `SandboxExecutor` and append results
4. Repeat until `FinalAnswer` / `MaxSteps` / `TokenBudgetExhausted` / `Error` / `UserInterrupt`

Because `AGENTS.md` is always appended, edits to it directly affect agent behavior at runtime. Keep it accurate.

## Policy Modes (auto-detected by context)

| Context | Default mode |
|---------|--------------|
| Interactive CLI (TTY) | `confirm` — prompts `Execute? [Y/n]` before dangerous tools; blocked resources trigger separate "Add to allowlist?" prompts that persist to `~/.rune/rune.toml` |
| Pipe mode (stdin not a TTY) | `allowlist` — silent block of anything not whitelisted |
| Concourse `check`/`in`/`out` | `allowlist` |
| `--unrestricted` flag | bypasses all policy |

Path-based auto-allow: `read_file`/`read_markdown` skip the prompt if the resolved path is under `allowed_paths_ro` ∪ `allowed_paths_rw`; `write_file`/`write_markdown` skip if under `allowed_paths_rw`. CWD is implicitly added to `allowed_paths_ro` at startup.

## Tools

10 built-in tools live in `src/tools/mod.rs`. Six (`read_file`, `write_file`, `list_dir`, `execute_cmd`, `fetch_url`, `inspect_process`) are always present. Four (`search_chat`, `list_markdown`, `read_markdown`, `write_markdown`) are **only registered in `rune notes` serve mode** — do not assume they exist in CLI flows.

Adding a tool: implement in `src/tools/mod.rs`, add JSON schema in `tool_definitions()`. The sandbox layer handles execution; no per-tool sandbox plumbing needed.

## Notes Serve Mode

`src/serve/` is the Axum HTTP server. Frontend assets in `web/` are **embedded at compile time via `include_str!`** (`src/serve/static_files.rs`) — to ship a frontend change you must rebuild. SQLite persistence in `src/serve/db.rs` (bundled rusqlite, no external libsqlite needed). Live updates use Server-Sent Events; see `AGENTS.md` for the complete SSE event-name list.

Three-role access control (admin / user / guest) is enforced in `src/serve/mod.rs` auth middleware. Guest is read-only for *public* notes/files only. Recent fixes (`git log`) touched the guest auth path — when changing middleware, run `tests/public_preview.js` and `tests/ws_e2e.py`.

## Providers

`src/provider/` — provider auto-detection from API key prefix (`ghu_`/`ghp_` → GitHub Copilot with token refresh, `AIza` → Gemini native, `sk-or-` → OpenRouter, generic `sk-` / custom `base_url` → OpenAI-compatible). MCP stdio JSON-RPC client lives in `src/mcp/`.

## File Layout Conventions

- `src/<module>/mod.rs` — single-file modules; no submodule directories except `sandbox/` and `serve/` which have multiple files
- `skills/` and `.rune/` are gitignored — never commit user-installed skills or local config
- `docs/` is a static GitHub Pages site, not source documentation
- `tests/` mixes shell (`e2e.sh`), JavaScript (`*.js` Playwright/Puppeteer probes), and Python (`ws_e2e.py`); only `e2e.sh` runs in CI

## Things to Watch

- **Do not break `cargo fmt`** — CI fails immediately and skips everything else.
- **Do not add `// removed`/`// TODO` shims** when deleting code; delete cleanly.
- **AGENTS.md edits affect runtime behavior** (always appended to system prompt). Treat it as code, not just docs.
- **`include_str!` embeds `web/` assets** — frontend changes require `cargo build`.
- **Sandbox internal subcommands** (`_landlock`/`_seccomp`/`_net-guard`) are not exposed in CLI help; they are part of the re-exec protocol and must not be renamed without updating `SandboxExecutor` call sites.
