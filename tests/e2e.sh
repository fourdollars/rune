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

# ── Test 1: Banner & Version ──────────────────────────────
echo "▸ Banner & Version"
OUT=$(printf "/version\n/exit\n" | $RUNE 2>&1)
assert_contains "$OUT" "ᚱ" "banner shows rune symbol"
assert_contains "$OUT" "v0.1.0" "version displayed"
assert_contains "$OUT" "Goodbye" "clean exit"

# ── Test 2: Help ──────────────────────────────────────────
echo "▸ Help"
OUT=$(printf "/help\n/exit\n" | $RUNE 2>&1)
assert_contains "$OUT" "/config" "help lists /config"
assert_contains "$OUT" "/tools" "help lists /tools"
assert_contains "$OUT" "/info" "help lists /info"

# ── Test 3: Config ────────────────────────────────────────
echo "▸ Config"
OUT=$(printf "/config\n/exit\n" | $RUNE 2>&1)
assert_contains "$OUT" "model:" "config shows model"
assert_contains "$OUT" "skills_dir:" "config shows skills_dir"

# ── Test 4: Tools ─────────────────────────────────────────
echo "▸ Tools"
OUT=$(printf "/tools\n/exit\n" | $RUNE 2>&1)
assert_contains "$OUT" "read_file" "tools lists read_file"
assert_contains "$OUT" "write_file" "tools lists write_file"
assert_contains "$OUT" "fetch_url" "tools lists fetch_url"
assert_contains "$OUT" "run_terminal_cmd" "tools lists run_terminal_cmd"
assert_contains "$OUT" "list_dir" "tools lists list_dir"

# ── Test 5: Skills ────────────────────────────────────────
echo "▸ Skills"
OUT=$(printf "/skills\n/exit\n" | $RUNE 2>&1)
assert_contains "$OUT" "sysadmin" "skills finds sysadmin"
assert_contains "$OUT" "launchpad" "skills finds launchpad"

# ── Test 6: Info / Sandbox Status ─────────────────────────
echo "▸ Info"
OUT=$(printf "/info\n/exit\n" | $RUNE 2>&1)
assert_contains "$OUT" "Network Isolation" "info shows network section"
assert_contains "$OUT" "Filesystem Access" "info shows filesystem section"
assert_contains "$OUT" "Tool Restrictions" "info shows tool restrictions"

# ── Test 7: Concourse check ───────────────────────────────
echo "▸ Concourse check (no version)"
ln -sf "$(realpath $RUNE)" /tmp/check 2>/dev/null || true
OUT=$(echo '{"source":{}}' | /tmp/check 2>/dev/null || echo "empty")
assert_contains "$OUT" '\[' "check with no version returns empty array"

echo "▸ Concourse check (with version)"
OUT=$(echo '{"source":{},"version":{"ref":"abc123"}}' | /tmp/check 2>/dev/null || echo "")
assert_contains "$OUT" "abc123" "check echoes version back"

# ── Test 8: CLI argument --help ───────────────────────────
echo "▸ CLI --help"
OUT=$($RUNE --help 2>&1)
assert_contains "$OUT" "zero-trust\|Zero-Trust" "help shows description"
assert_contains "$OUT" "RUNE_API_KEY" "help shows env vars"
assert_contains "$OUT" "rune init" "help mentions init subcommand"

# ── Test 9: CLI --version ─────────────────────────────────
echo "▸ CLI --version"
OUT=$($RUNE --version 2>&1)
assert_contains "$OUT" "rune 0.1.0" "version flag works"

# ── Test 10: UTF-8 handling ───────────────────────────────
echo "▸ UTF-8 input handling"
OUT=$(printf "你好\n/exit\n" | $RUNE 2>&1)
assert_not_contains "$OUT" "Read error" "UTF-8 input does not cause read error"
assert_contains "$OUT" "Goodbye" "exits cleanly after UTF-8 input"

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
