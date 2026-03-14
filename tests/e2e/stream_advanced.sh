#!/usr/bin/env bash
# E2E: Advanced stream tests.
#
# Requires wh and wh-broker binaries in $PATH or passed as WH / WH_BROKER env vars.
# Covers: multi-stream isolation, stream retention, tail --format json,
#         tail --filter, and stream subscribe round-trip.
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
TAIL_A_PID=0
TAIL_B_PID=0
BG_PID=0

cleanup() {
    [[ $TAIL_A_PID -gt 0 ]] && kill "$TAIL_A_PID" 2>/dev/null || true
    [[ $TAIL_B_PID -gt 0 ]] && kill "$TAIL_B_PID" 2>/dev/null || true
    [[ $BG_PID     -gt 0 ]] && kill "$BG_PID"     2>/dev/null || true
    [[ $BROKER_PID -gt 0 ]] && kill "$BROKER_PID" 2>/dev/null || true
    [[ $TAIL_A_PID -gt 0 ]] && wait "$TAIL_A_PID" 2>/dev/null || true
    [[ $TAIL_B_PID -gt 0 ]] && wait "$TAIL_B_PID" 2>/dev/null || true
    [[ $BG_PID     -gt 0 ]] && wait "$BG_PID"     2>/dev/null || true
    [[ $BROKER_PID -gt 0 ]] && wait "$BROKER_PID" 2>/dev/null || true
    rm -f /tmp/wh-e2e-adv-a.txt /tmp/wh-e2e-adv-b.txt \
          /tmp/wh-e2e-adv-json.txt /tmp/wh-e2e-adv-filter.txt \
          /tmp/wh-e2e-adv-subscribe.txt
}
trap cleanup EXIT

# Wait for broker control socket to accept connections (max 5 s)
for i in $(seq 1 25); do
    if "$WH" stream list &>/dev/null; then break; fi
    sleep 0.2
done

echo "Running advanced stream E2E tests..."

# ── 1. Multi-stream isolation ─────────────────────────────────────────────────
# Verify that messages published to stream-a do not appear in stream-b and vice versa.

"$WH" stream create e2e-adv-stream-a &>/dev/null
"$WH" stream create e2e-adv-stream-b &>/dev/null

"$WH" stream tail e2e-adv-stream-a >/tmp/wh-e2e-adv-a.txt 2>&1 &
TAIL_A_PID=$!
"$WH" stream tail e2e-adv-stream-b >/tmp/wh-e2e-adv-b.txt 2>&1 &
TAIL_B_PID=$!
sleep 0.3

"$WH" stream publish e2e-adv-stream-a "message-for-a" &>/dev/null
"$WH" stream publish e2e-adv-stream-b "message-for-b" &>/dev/null

# Wait up to 3 s for both messages to arrive
for i in $(seq 1 15); do
    if grep -q "message-for-a" /tmp/wh-e2e-adv-a.txt 2>/dev/null && \
       grep -q "message-for-b" /tmp/wh-e2e-adv-b.txt 2>/dev/null; then break; fi
    sleep 0.2
done
kill "$TAIL_A_PID" 2>/dev/null || true; TAIL_A_PID=0
kill "$TAIL_B_PID" 2>/dev/null || true; TAIL_B_PID=0

if grep -q "message-for-a" /tmp/wh-e2e-adv-a.txt 2>/dev/null; then
    pass "multi-stream: stream-a receives its own message"
else
    fail "multi-stream: stream-a receives its own message"
fi

if grep -q "message-for-b" /tmp/wh-e2e-adv-b.txt 2>/dev/null; then
    pass "multi-stream: stream-b receives its own message"
else
    fail "multi-stream: stream-b receives its own message"
fi

if ! grep -q "message-for-b" /tmp/wh-e2e-adv-a.txt 2>/dev/null; then
    pass "multi-stream: stream-a does NOT receive stream-b message"
else
    fail "multi-stream: stream-a does NOT receive stream-b message"
fi

if ! grep -q "message-for-a" /tmp/wh-e2e-adv-b.txt 2>/dev/null; then
    pass "multi-stream: stream-b does NOT receive stream-a message"
else
    fail "multi-stream: stream-b does NOT receive stream-a message"
fi

"$WH" stream delete e2e-adv-stream-a &>/dev/null
"$WH" stream delete e2e-adv-stream-b &>/dev/null

# ── 2. Stream create with --retention ─────────────────────────────────────────

if "$WH" stream create e2e-adv-retention --retention 7d &>/dev/null; then
    pass "stream create --retention 7d"
else
    fail "stream create --retention 7d"
fi

LIST_JSON=$("$WH" stream list --format json 2>/dev/null || echo "{}")
if echo "$LIST_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
streams = d.get('data', {}).get('streams', [])
matched = [s for s in streams if s.get('name') == 'e2e-adv-retention']
assert matched, f'e2e-adv-retention not in stream list: {[s[\"name\"] for s in streams]}'
retention = matched[0].get('retention')
assert retention is not None, 'retention field is null/missing'
assert '7' in str(retention), f'Expected 7d retention, got: {retention}'
" 2>/dev/null; then
    pass "stream list --format json shows 7d retention"
else
    fail "stream list --format json shows 7d retention"
fi

"$WH" stream delete e2e-adv-retention &>/dev/null

# ── 3. stream tail --format json emits valid NDJSON ──────────────────────────

"$WH" stream create e2e-adv-json-tail &>/dev/null

"$WH" stream tail e2e-adv-json-tail --format json >/tmp/wh-e2e-adv-json.txt 2>&1 &
BG_PID=$!
sleep 0.3

"$WH" stream publish e2e-adv-json-tail "hello-json-tail" &>/dev/null

# Wait for the message line (contains the type field)
for i in $(seq 1 15); do
    if grep -q '"TextMessage"' /tmp/wh-e2e-adv-json.txt 2>/dev/null; then break; fi
    sleep 0.2
done
kill "$BG_PID" 2>/dev/null || true; BG_PID=0

if grep -q '"TextMessage"' /tmp/wh-e2e-adv-json.txt 2>/dev/null; then
    pass "stream tail --format json emits TextMessage type in output"
else
    fail "stream tail --format json emits TextMessage type in output"
fi

MSG_LINE=$(grep '"TextMessage"' /tmp/wh-e2e-adv-json.txt 2>/dev/null | head -1 || true)
if [ -n "$MSG_LINE" ] && echo "$MSG_LINE" | python3 -c "
import sys, json
d = json.loads(sys.stdin.read())
assert 'timestamp' in d, f'Missing timestamp field in: {d}'
assert 'type' in d,      f'Missing type field in: {d}'
assert 'publisher' in d, f'Missing publisher field in: {d}'
assert 'payload' in d,   f'Missing payload field in: {d}'
assert d['type'] == 'TextMessage', f'Expected TextMessage, got: {d[\"type\"]}'
" 2>/dev/null; then
    pass "stream tail --format json message has required fields (timestamp, type, publisher, payload)"
else
    fail "stream tail --format json message has required fields (timestamp, type, publisher, payload)"
fi

"$WH" stream delete e2e-adv-json-tail &>/dev/null

# ── 4. stream tail --filter publisher=cli ────────────────────────────────────
# Messages published via wh CLI use publisher_id "cli" by default.
# The --filter publisher=cli filter should pass those messages through.

"$WH" stream create e2e-adv-filter-test &>/dev/null

"$WH" stream tail e2e-adv-filter-test --filter publisher=cli >/tmp/wh-e2e-adv-filter.txt 2>&1 &
BG_PID=$!
sleep 0.3

"$WH" stream publish e2e-adv-filter-test "filtered-message" &>/dev/null

for i in $(seq 1 15); do
    if grep -q "filtered-message" /tmp/wh-e2e-adv-filter.txt 2>/dev/null; then break; fi
    sleep 0.2
done
kill "$BG_PID" 2>/dev/null || true; BG_PID=0

if grep -q "filtered-message" /tmp/wh-e2e-adv-filter.txt 2>/dev/null; then
    pass "stream tail --filter publisher=cli passes matching message through"
else
    fail "stream tail --filter publisher=cli passes matching message through"
fi

"$WH" stream delete e2e-adv-filter-test &>/dev/null

# ── 5. stream subscribe round-trip ───────────────────────────────────────────
# Verifies the subscribe subcommand (different from tail: no TCP probe, auto-reconnect).

"$WH" stream create e2e-adv-subscribe-test &>/dev/null

"$WH" stream subscribe e2e-adv-subscribe-test >/tmp/wh-e2e-adv-subscribe.txt 2>&1 &
BG_PID=$!
sleep 0.3

"$WH" stream publish e2e-adv-subscribe-test "hello-subscribe" &>/dev/null

for i in $(seq 1 15); do
    if grep -q "hello-subscribe" /tmp/wh-e2e-adv-subscribe.txt 2>/dev/null; then break; fi
    sleep 0.2
done
kill "$BG_PID" 2>/dev/null || true; BG_PID=0

if grep -q "hello-subscribe" /tmp/wh-e2e-adv-subscribe.txt 2>/dev/null; then
    pass "stream subscribe receives published message"
else
    fail "stream subscribe receives published message"
fi

"$WH" stream delete e2e-adv-subscribe-test &>/dev/null

# ── Result ────────────────────────────────────────────────────────────────────

trap - EXIT
cleanup

echo ""
echo "  $PASS passed · $FAIL failed"
[[ $FAIL -eq 0 ]]
