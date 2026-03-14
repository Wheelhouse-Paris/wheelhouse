#!/usr/bin/env bash
# E2E: wh status and wh ps command tests.
#
# Requires wh and wh-broker binaries in $PATH or passed as WH / WH_BROKER env vars.
# Starts a real broker, exercises `wh status` and `wh ps` commands.
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
    [[ $BROKER_PID -gt 0 ]] && kill "$BROKER_PID" 2>/dev/null || true
    [[ $BROKER_PID -gt 0 ]] && wait "$BROKER_PID" 2>/dev/null || true
}
trap cleanup EXIT

# Wait for broker control socket to accept connections (max 5 s)
for i in $(seq 1 25); do
    if "$WH" stream list &>/dev/null; then break; fi
    sleep 0.2
done

echo "Running status and ps E2E tests..."

# ── 1. wh status exits 0 with running broker ──────────────────────────────────

if "$WH" status &>/dev/null; then
    pass "wh status exits 0 with running broker"
else
    fail "wh status exits 0 with running broker"
fi

# ── 2. wh status --format json returns valid JSON with status=ok ──────────────

STATUS_JSON=$("$WH" status --format json 2>/dev/null || echo "{}")
if echo "$STATUS_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('status') == 'ok', f'Expected status=ok, got: {d}'
" 2>/dev/null; then
    pass "wh status --format json returns {status: ok}"
else
    fail "wh status --format json returns {status: ok}"
fi

# ── 3. wh ps exits 0 with running broker (no state.json) ─────────────────────

if "$WH" ps &>/dev/null; then
    pass "wh ps exits 0 with running broker"
else
    fail "wh ps exits 0 with running broker"
fi

# ── 4. wh ps --format json returns valid JSON with expected structure ─────────

PS_JSON=$("$WH" ps --format json 2>/dev/null || echo "{}")
if echo "$PS_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('status') == 'ok', f'Expected status=ok, got: {d}'
data = d.get('data', {})
assert 'components' in data, f'Missing components key in: {data}'
assert 'summary' in data, f'Missing summary key in: {data}'
summary = data['summary']
assert 'total_agents' in summary, f'Missing total_agents in summary: {summary}'
assert 'running' in summary, f'Missing running in summary: {summary}'
assert 'stopped' in summary, f'Missing stopped in summary: {summary}'
" 2>/dev/null; then
    pass "wh ps --format json returns valid JSON with components and summary"
else
    fail "wh ps --format json returns valid JSON with components and summary"
fi

# ── 5. wh ps --format json summary counts are non-negative integers ───────────

if echo "$PS_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
summary = d.get('data', {}).get('summary', {})
assert isinstance(summary.get('total_agents'), int) and summary['total_agents'] >= 0
assert isinstance(summary.get('running'), int) and summary['running'] >= 0
assert isinstance(summary.get('stopped'), int) and summary['stopped'] >= 0
" 2>/dev/null; then
    pass "wh ps --format json summary counts are non-negative integers"
else
    fail "wh ps --format json summary counts are non-negative integers"
fi

# ── Result ────────────────────────────────────────────────────────────────────

trap - EXIT
cleanup

echo ""
echo "  $PASS passed · $FAIL failed"
[[ $FAIL -eq 0 ]]
