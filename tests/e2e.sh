#!/bin/bash
# Rune E2E Test Suite
# Run from project root: ./tests/e2e.sh
set -euo pipefail

RUNE="./target/release/rune"
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
assert_contains "$OUT" "does not bypass policy" "help clarifies --yes semantics"

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
assert_contains "$OUT" "executions" "help mentions tool executions"

# ── Test 10: Policy mode override ─────────────────────────
echo "▸ Policy mode override via env"
set +e
OUT=$(RUNE_POLICY_MODE=allowlist printf "run ls" | $RUNE --json --yes 2>&1)
set -e
assert_not_contains "$OUT" "Execute?" "allowlist mode skips confirm prompt"

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
