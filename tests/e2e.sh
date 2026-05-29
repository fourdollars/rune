#!/bin/bash
# Rune E2E Test Suite
# Run from project root: ./tests/e2e.sh
set -euo pipefail

RUNE="./target/release/rune"
RUNE_DIR="."
PASS=0
FAIL=0

green() { printf "\033[32m%s\033[0m\n" "$1"; }
red() { printf "\033[31m%s\033[0m\n" "$1"; }
dim() { printf "\033[2m%s\033[0m\n" "$1"; }

assert_contains() {
    local output="$1" expected="$2" test_name="$3"
    if echo "$output" | grep -q "$expected"; then
        green "  ✓ $test_name"
        PASS=$((PASS + 1))
    else
        red "  ✗ $test_name (expected: '$expected')"
        dim "    got: ${output:0:200}"
        FAIL=$((FAIL + 1))
    fi
}

assert_not_contains() {
    local output="$1" unexpected="$2" test_name="$3"
    if echo "$output" | grep -q "$unexpected"; then
        red "  ✗ $test_name (unexpected: '$unexpected')"
        FAIL=$((FAIL + 1))
    else
        green "  ✓ $test_name"
        PASS=$((PASS + 1))
    fi
}

assert_exit_code() {
    local actual="$1" expected="$2" test_name="$3"
    if [ "$actual" -eq "$expected" ]; then
        green "  ✓ $test_name"
        PASS=$((PASS + 1))
    else
        red "  ✗ $test_name (expected exit $expected, got $actual)"
        FAIL=$((FAIL + 1))
    fi
}

echo "═══════════════════════════════════════"
echo "  ᚱ  Rune E2E Test Suite"
echo "═══════════════════════════════════════"
echo ""

# Ensure binary exists
if [ ! -f "$RUNE" ]; then
    red "Binary not found: $RUNE"
    red "Run: cargo build --release"
    exit 1
fi

# ── Test 1: CLI --help ────────────────────────────────────
echo "▸ CLI --help"
OUT=$($RUNE --help 2>&1)
assert_contains "$OUT" "zero-trust\|Zero-Trust" "help shows description"
assert_contains "$OUT" "RUNE_API_KEY" "help shows env vars"
assert_contains "$OUT" "rune init" "help mentions init subcommand"
assert_contains "$OUT" "\-\-json" "help shows --json flag"
assert_contains "$OUT" "\-\-yes" "help shows --yes flag"
assert_contains "$OUT" "bypass policy" "help clarifies --yes semantics"

# ── Test 2: CLI --version ─────────────────────────────────
echo "▸ CLI --version"
OUT=$($RUNE --version 2>&1)
assert_contains "$OUT" "rune 0.1.0" "version flag works"

# ── Test 3: Pipe mode — empty input ──────────────────────
echo "▸ Pipe mode — empty input"
set +e
OUT=$(printf "" | $RUNE --json 2>&1)
EC=$?
set -e
assert_exit_code "$EC" 1 "empty pipe exits with code 1"
assert_contains "$OUT" "No piped input" "empty pipe shows error message"

# ── Test 4: Pipe mode — no banner ────────────────────────
echo "▸ Pipe mode — no banner"
set +e
OUT=$(printf "hello" | $RUNE --json 2>&1)
set -e
assert_not_contains "$OUT" "ᛟ" "pipe mode does not show banner"

# ── Test 5: --json flag (no value needed) ────────────────
echo "▸ --json flag works without value"
OUT=$($RUNE --json --version 2>&1)
assert_contains "$OUT" "rune 0.1.0" "--json flag accepted without value"

# ── Test 6: --yes flag (no value needed) ─────────────────
echo "▸ --yes flag works without value"
OUT=$($RUNE --yes --version 2>&1)
assert_contains "$OUT" "rune 0.1.0" "--yes flag accepted without value"

# ── Test 7: Concourse check (no version) ─────────────────
echo "▸ Concourse check (no version)"
ln -sf "$(realpath $RUNE)" /tmp/check 2>/dev/null || true
OUT=$(echo '{"source":{}}' | /tmp/check 2>/dev/null || echo "empty")
assert_contains "$OUT" '\[' "check with no version returns empty array"

# ── Test 8: Concourse check (with version) ───────────────
echo "▸ Concourse check (with version, no prompt)"
OUT=$(echo '{"source":{},"version":{"ref":"abc123"}}' | /tmp/check 2>/dev/null || echo "")
assert_contains "$OUT" "latest" "check without prompt returns synthetic version"

# ── Test 9: Tool definitions ─────────────────────────────
echo "▸ Tool definitions (via --help)"
OUT=$($RUNE --help 2>&1)
assert_contains "$OUT" "execution" "help mentions tool execution"

# ── Test 10: Policy mode override ─────────────────────────
echo "▸ Policy mode override via env"
set +e
OUT=$(RUNE_POLICY_MODE=allowlist printf "run ls" | $RUNE --json --yes 2>&1)
set -e
assert_not_contains "$OUT" "Execute?" "allowlist mode skips confirm prompt"


# ── Test: --provider flag ─────────────────────────────────
echo "▸ --provider flag accepted"
OUT=$($RUNE --provider openai --version 2>&1)
assert_contains "$OUT" "rune 0.1.0" "--provider flag accepted with --version"

# ── Test: --provider in --help ────────────────────────────
echo "▸ --provider in help"
OUT=$($RUNE --help 2>&1)
assert_contains "$OUT" "\-\-provider" "help shows --provider option"
assert_contains "$OUT" "github-copilot" "help mentions github-copilot"

# ── Test: /image in --help text ───────────────────────────
echo "▸ /image mentioned in source"
OUT=$(grep -c "image\|/img" $RUNE_DIR/src/cli/mod.rs 2>/dev/null || echo "0")
if [ "$OUT" -gt "0" ]; then
    green "  ✓ /image command exists in source"
    PASS=$((PASS + 1))
else
    red "  ✗ /image command not found in source"
    FAIL=$((FAIL + 1))
fi

# ── Test: rune _net-guard subcommand exists ────────────────────
echo "▸ rune _net-guard subcommand"
set +e
OUT=$(./target/release/rune _net-guard 2>&1)
EC=$?
set -e
if [ $EC -eq 1 ] && echo "$OUT" | grep -q "allow-domains"; then
    green "  ✓ rune _net-guard subcommand works"
    PASS=$((PASS + 1))
else
    red "  ✗ rune _net-guard subcommand failed"
    FAIL=$((FAIL + 1))
fi

# ── Test: rune _net-guard usage ───────────────────────────
echo "▸ rune _net-guard usage"
set +e
OUT=$(./target/release/rune _net-guard 2>&1)
EC=$?
set -e
assert_exit_code "$EC" 1 "net-guard exits 1 without args"
assert_contains "$OUT" "allow-domains" "net-guard shows usage hint"

# ── Test: rune _net-guard blocks non-allowed domain ────────
echo "▸ rune _net-guard blocks"
set +e
OUT=$(./target/release/rune _net-guard --allow-domains "only-this.test" -- curl -s -m 2 http://example.com/ 2>&1)
EC=$?
set -e
# curl should fail (exit 7 = connect refused, or 6 = DNS fail depending on timing)
if [ $EC -ne 0 ]; then
    green "  ✓ net-guard blocked non-allowed domain (exit $EC)"
    PASS=$((PASS + 1))
else
    red "  ✗ net-guard did NOT block (exit 0)"
    FAIL=$((FAIL + 1))
fi

# ── Test: context_window config field ─────────────────────
echo "▸ context_window env var"
set +e
OUT=$(RUNE_CONTEXT_WINDOW=4096 printf "hi" | $RUNE --json --yes 2>&1)
set -e
# Should not crash — just verifies the env var is accepted
assert_not_contains "$OUT" "panic" "RUNE_CONTEXT_WINDOW does not panic"

# ── Test: RUNE_PROVIDER env var ───────────────────────────
echo "▸ RUNE_PROVIDER env var"
set +e
OUT=$(RUNE_PROVIDER=openai RUNE_API_KEY=sk-test printf "hi" | $RUNE --json --yes 2>&1)
set -e
# Will fail (invalid key) but should not panic or show "unknown provider"
assert_not_contains "$OUT" "unknown provider" "RUNE_PROVIDER=openai accepted"

# ── WS E2E Tests (login, archive, search) ─────────────────
echo ""
echo "── WebSocket E2E (SKIPPED — migrated to SSE)"
# Skipped: WS removed in SSE migration
if false; then
echo "old ──────────────────────"
WS_E2E="$(dirname "$0")/ws_e2e.py"
if command -v python3 >/dev/null 2>&1 && python3 -c "import websockets" 2>/dev/null; then
    if python3 "$WS_E2E" 2>&1; then
        PASS=$((PASS + 11))
    else
        WS_FAIL=$(python3 "$WS_E2E" 2>&1 | grep -c "✗" || true)
        WS_PASS=$(python3 "$WS_E2E" 2>&1 | grep -c "✓" || true)
        PASS=$((PASS + WS_PASS))
        FAIL=$((FAIL + WS_FAIL))
    fi
else
    dim "  (skipped: python3 or websockets not available)"
fi

fi
# ── Summary ───────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════"
TOTAL=$((PASS + FAIL))
if [ $FAIL -eq 0 ]; then
    green "  All $TOTAL tests passed! ᚱ"
else
    red "  $FAIL/$TOTAL tests failed"
fi
echo "═══════════════════════════════════════"

# Cleanup
rm -f /tmp/check

exit $FAIL
