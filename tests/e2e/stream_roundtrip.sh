#!/usr/bin/env bash
# E2E: stream pub/sub round-trip tests.
#
# Requires wh and wh-broker binaries in $PATH or passed as WH / WH_BROKER env vars.
# Starts a real broker, exercises create / list / publish / tail / delete via CLI.
#
# Exit 0 = all tests passed. Exit 1 = failure (test name printed to stderr).

set -euo pipefail

WH="${WH:-wh}"
WH_BROKER="${WH_BROKER:-wh-broker}"
PASS=0
FAIL=0

pass() { echo "  ✓ $1"; PASS=$((PASS + 1)); }
fail() { echo "  ✗ $1" >&2; FAIL=$((FAIL + 1)); }

# ── Broker setup ──────────────────────────────────────────────────────────────

"$WH_BROKER" &
BROKER_PID=$!
cleanup() {
    kill "$BROKER_PID" 2>/dev/null || true
    kill "$TAIL_PID"   2>/dev/null || true
    wait "$BROKER_PID" 2>/dev/null || true
    wait "$TAIL_PID"   2>/dev/null || true
    rm -f /tmp/wh-e2e-tail.txt
}
TAIL_PID=0
trap cleanup EXIT

# Wait for broker control socket to accept connections (max 5 s)
for i in $(seq 1 25); do
    if "$WH" stream list &>/dev/null; then break; fi
    sleep 0.2
done

echo "Running stream E2E tests..."

# ── 1. stream create ──────────────────────────────────────────────────────────

if "$WH" stream create e2e-test &>/dev/null; then
    pass "stream create"
else
    fail "stream create"
fi

# ── 2. stream list ────────────────────────────────────────────────────────────

if "$WH" stream list | grep -q "e2e-test"; then
    pass "stream list shows created stream"
else
    fail "stream list shows created stream"
fi

# ── 3. stream list --format json ─────────────────────────────────────────────

JSON_OUT=$("$WH" stream list --format json 2>/dev/null || echo "{}")
if echo "$JSON_OUT" | python3 -c "
import sys, json
d = json.load(sys.stdin)
names = [s['name'] for s in d.get('data', {}).get('streams', [])]
assert 'e2e-test' in names, f'e2e-test not in {names}'
" 2>/dev/null; then
    pass "stream list --format json"
else
    fail "stream list --format json"
fi

# ── 4. pub/sub round-trip ─────────────────────────────────────────────────────

# Start tail in background; give it 0.3 s to subscribe before we publish
"$WH" stream tail e2e-test >/tmp/wh-e2e-tail.txt 2>&1 &
TAIL_PID=$!
sleep 0.3

"$WH" stream publish e2e-test "hello e2e" &>/dev/null

# Give tail up to 3 s to receive the message, then kill it
for i in $(seq 1 15); do
    if grep -q "hello e2e" /tmp/wh-e2e-tail.txt 2>/dev/null; then break; fi
    sleep 0.2
done
kill "$TAIL_PID" 2>/dev/null || true
TAIL_PID=0

if grep -q "hello e2e" /tmp/wh-e2e-tail.txt 2>/dev/null; then
    pass "pub/sub round-trip (publish → tail)"
else
    fail "pub/sub round-trip (publish → tail)"
fi

# ── 5. stream delete ──────────────────────────────────────────────────────────

if "$WH" stream delete e2e-test &>/dev/null; then
    pass "stream delete"
else
    fail "stream delete"
fi

if ! "$WH" stream list | grep -q "e2e-test"; then
    pass "stream list no longer shows deleted stream"
else
    fail "stream list no longer shows deleted stream"
fi

# ── Result ────────────────────────────────────────────────────────────────────

echo ""
echo "  $PASS passed · $FAIL failed"
[[ $FAIL -eq 0 ]]
