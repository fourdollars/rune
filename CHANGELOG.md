# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased] — 2026-05-04

### Changed
- **Policy mode defaults by context:**
  - Interactive CLI: `confirm` (unchanged)
  - Pipe mode (`stdin`): now defaults to `allowlist` (was `confirm`)
  - Concourse CI (check/get/put): now defaults to `allowlist` (was `confirm`)
  - All modes can be overridden to `unrestricted` via `--policy-mode`, `RUNE_POLICY_MODE`, config, or Concourse `source.sandbox.policy_mode`
- **Unrestricted mode** now fully bypasses all policy checks (denied_paths, allowed_commands, allowed_domains, allowed_paths_rw)
- **Non-interactive tool errors** are now hard-stops (StopReason::Error) — sandbox violations cause Concourse steps to fail visibly
- **Sandbox layers** (Landlock + Seccomp) now chain correctly when both are available (was mutually exclusive)
- **Concourse build logs** now display the prompt at the start of each check/get/put step

## [Previous] — 2026-05-03

### Added
- **Embedding Engine (§19)** — `src/embedding/mod.rs`
  - `EmbeddingConfig`, `EmbeddingEngine`, `VectorStore`
  - Provider-agnostic `/v1/embeddings` API support
  - Cosine similarity search with configurable threshold
  - JSON persistence to `.rune/vectors/`
  - `[embedding]` config section in rune.toml
- **Persistent Prompt History** — readline history saved to `~/.rune/history`
- **Skill tools_deny Enforcement (§5.3)** — skills can now restrict tool access
  - `tools_allow`: only listed tools exposed to LLM and executable
  - `tools_deny`: listed tools hidden from LLM and blocked at execution
  - Both filter `tool_definitions()` sent to the provider
- **Concourse AI-Driven Resource Type** — complete rewrite
  - `check` / `in` / `out` all run the full sandboxed Rune agent pipeline (same behavior as pipe mode)
  - `check`: prompt → final answer → sha256 versioning (content-based triggers)
  - `in` (get): prompt → `payload.json` + `response.txt`
  - `out` (put): `params.prompt` → version + build log
  - GitHub Copilot token auto-refresh (`ghu_`/`ghp_` detection)
  - Sandbox allowlists supported in resource source (`allowed_domains`, `read_write_paths`, `read_only_paths`)
- **CI Coverage Report** — `cargo-llvm-cov` integration
  - Coverage summary in GitHub Actions job summary
  - `lcov.info` artifact upload (30-day retention)
  - Binary size reporting
- **Unit Tests** — 34 → 124 tests
  - `config/mod.rs`: 22 tests (pick, parse_boolish, defaults, load_toml, persist)
  - `trace/mod.rs`: 12 tests (redact patterns, disabled mode, all StepKinds) → 100% coverage
  - `skills/mod.rs`: 20 tests (extract_refs, frontmatter, loader, resolve)
  - `tools/mod.rs`: 17 tests (quote-aware parser, pipeline, redirect)
  - `embedding/mod.rs`: 13 tests (cosine similarity, vector store CRUD)
  - `concourse/mod.rs`: 8 tests (sha256, check/in/out behavior)
  - `provider/mod.rs`: 14 tests (provider backoff, token refresh)
  - `sandbox/mod.rs`: 6 tests (basic execution, timeout, nonzero exit, degraded mode)
  - `agent/mod.rs`: 4 tests (stop reasons, agent loop edge cases)
  - helper bins (src/bin/*.rs): 6 tests (net-guard, seccomp helper tests)

### Changed
- **Config limits now optional** — `max_steps`, `token_budget`, `timeout_secs` are `Option` types; unlimited when not set
- **Cargo deps upgraded** — colored 3, indicatif 0.18, rustyline 18, toml 1.1
- **Docker image runs as root** — required for Concourse resource volumes

### Fixed
- **Path-based auto-allow** — `read_file` skips confirm if path in `allowed_paths_ro`/`rw`; CWD auto-added to `allowed_paths_ro`
- **"Always" persist for paths** — pressing A now correctly persists parent dir to config
- **Quote-aware command parser** — grep patterns with `\|` no longer split on quoted pipes
- **Redirect operators** — `2>&1` no longer treated as command separator
- **Pipeline allowlist** — all binaries in pipeline checked (not just first)
- **Runtime allowlist sync** — "Always" now updates ToolRegistry in-memory immediately
- **Concourse first check** — returns synthetic version instead of empty array
- **Concourse arg parsing** — mode detected before clap to avoid positional arg rejection

## [0.1.0] — 2026-05-01

### Added
- Initial release: 6 built-in tools, 3 providers, Skills system, MCP Client
- 5-layer sandbox (netns + cgroups + seccomp + landlock + DNS allowlist)
- CLI with confirm/allowlist/unrestricted policy modes
- Concourse CI resource type (check/in/out via symlink)
- `rune init` setup wizard
- E2E test suite
- Docker images (debian/alpine/ubuntu)
- GitHub Actions CI + container push to ghcr.io
