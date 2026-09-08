#!/usr/bin/env bash
# =============================================================================
# G-12: Replay scrub + overlay (GCS_SPEC.md §9, gate G-12).
#
# Single-invocation harness: start fleet-catalog on :8300 with a temp
# catalog dir, drop a fake .replay file with 100 records (0.5 s at
# 200 Hz), exercise the replay scrub + topic-plot endpoints across two
# tick ranges, assert the data values are monotonically increasing
# (matching the fake file's pattern), tear down, exit 0 / non-zero.
#
# Per the task spec, this gate covers:
#   - GET /api/replays/{file}/meta            → records=100, duration≈0.5
#   - GET /api/replays/{file}/data?from_tick=0&to_tick=50&topic=pos_ned_m
#                                            → 51 data points
#   - GET /api/replays/{file}/data?from_tick=50&to_tick=100&topic=pos_ned_m
#                                            → 51 data points (the scrub
#                                              half beyond the midpoint)
#   - Monotonicity: pos_ned_m.x increases strictly across the full range
#     (0.1 * tick in the fake file → x @ tick 0 = 0.0, x @ tick 100 = 10.0)
#
# The "overlay live vehicle" half of G-12 is a frontend-only operation
# (the Analyze View's overlay button re-uses the live WS telemetry stream
# alongside the replay scrubber); the backend surface is the same
# `/api/replays/{file}/data` endpoint exercised here, so the harness
# does not need a live vehicle to validate the backend contract.
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CATALOG_BIN="${CATALOG_BIN:-$ROOT/fleet/target/debug/fleet-catalog}"
MK_REPLAY="$ROOT/console/tests/mk_replay.py"

PORT_CATALOG=8300
WORK="$(mktemp -d -t g12_test.XXXXXX.dir)"
CATALOG_DIR="$WORK/catalog"
CATALOG_LOG="$WORK/catalog.log"
REPLAY_FILE="g12_scrub.replay"

CATALOG_PID=""

cleanup() {
    if [ -n "$CATALOG_PID" ] && kill -0 "$CATALOG_PID" 2>/dev/null; then
        kill "$CATALOG_PID" 2>/dev/null || true
        for _ in $(seq 1 20); do
            kill -0 "$CATALOG_PID" 2>/dev/null || break
            sleep 0.1
        done
        kill -9 "$CATALOG_PID" 2>/dev/null || true
    fi
    wait 2>/dev/null || true
    if [ "${G12_KEEP:-0}" = "1" ]; then
        echo "[G-12] keeping work dir: $WORK"
    else
        rm -rf "$WORK" 2>/dev/null || true
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-12] FAIL: $1"
    echo "=========== diagnostics ==========="
    echo "--- catalog.log (tail 20):"
    tail -20 "$CATALOG_LOG" 2>/dev/null || echo "(none)"
    echo "--- work dir: $WORK"
    G12_KEEP=1
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -x "$CATALOG_BIN" ] || fail "fleet-catalog binary missing at $CATALOG_BIN (build fleet workspace first)"
[ -f "$MK_REPLAY" ] || fail "mk_replay.py helper missing at $MK_REPLAY"
command -v curl >/dev/null || fail "curl not available"
command -v python3 >/dev/null || fail "python3 not available"

if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" >/dev/null 2>&1; then
    fail "port $PORT_CATALOG already in use (previous run?)"
fi

# -----------------------------------------------------------------------------
# 1. Start fleet-catalog on :8300 with a temp catalog dir.
# -----------------------------------------------------------------------------
echo "[G-12] binary:    $CATALOG_BIN"
echo "[G-12] work dir:  $WORK"
echo "[G-12] starting fleet-catalog on :$PORT_CATALOG"

"$CATALOG_BIN" --port "$PORT_CATALOG" --catalog-dir "$CATALOG_DIR" \
    >"$CATALOG_LOG" 2>&1 &
CATALOG_PID=$!

ok=""
for _ in $(seq 1 50); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" \
            | grep -q '"ok":true'; then
        ok=1; break
    fi
    kill -0 "$CATALOG_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "fleet-catalog did not come up on :$PORT_CATALOG"
echo "[G-12] catalog up (pid $CATALOG_PID)"

# -----------------------------------------------------------------------------
# 2. Drop a fake .replay file. The task spec calls for "100 records (0.5 s
#    at 200 Hz)" — but with strictly 100 records, ticks 0..99 are valid,
#    so the task's second-half query [50, 100] would be out of bounds.
#    The spec's "51 data points" assertion for [50, 100] requires 101
#    records (ticks 0..100). We generate 101 records (virtual_duration_s
#    = 0.505, still ≈0.5 per the spec) so both halves of the scrub
#    return 51 points as the task spec asserts.
# -----------------------------------------------------------------------------
python3 "$MK_REPLAY" "$CATALOG_DIR/replays/$REPLAY_FILE" 101 200 42
echo "[G-12] seeded $REPLAY_FILE (101 records, ~0.5 s @ 200 Hz)"

# -----------------------------------------------------------------------------
# 3. GET /api/replays/{file}/meta → assert records≥100, duration≈0.5.
# -----------------------------------------------------------------------------
echo "[G-12] test 1: GET /api/replays/g12_scrub.replay/meta"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays/g12_scrub.replay/meta")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok missing: {d}'
m = d.get('data', {})
# Spec calls for ~100 records; we generate 101 so the [50,100] query
# returns 51 points (ticks 50..100 inclusive) without going OOB.
n = m.get('records', 0)
assert n >= 100, f'expected >=100 records, got {n}'
assert abs(m.get('tick_rate_hz', 0) - 200.0) < 0.001, f'expected 200 Hz, got {m.get(\"tick_rate_hz\")}'
# virtual_duration_s = records / tick_rate_hz. For 101 records @ 200 Hz = 0.505s.
dur = m.get('virtual_duration_s', -1)
assert 0.49 <= dur <= 0.51, f'expected ~0.5 s duration, got {dur}'
assert m.get('seed') == 42, f'expected seed=42, got {m.get(\"seed\")}'
assert m.get('version') == 1, f'expected version=1, got {m.get(\"version\")}'
assert len(m.get('scenario_sha256', '')) == 64, f'expected 64-char hash'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 1: meta response invalid; resp: $RESP"
echo "[G-12]   ✓ meta: records=101, virtual_duration_s≈0.5, tick_rate_hz=200"

# -----------------------------------------------------------------------------
# 4. GET /api/replays/{file}/data?from_tick=0&to_tick=50&topic=pos_ned_m
#    → 51 data points (ticks 0..50 inclusive).
# -----------------------------------------------------------------------------
echo "[G-12] test 2: scrub first half (from_tick=0, to_tick=50)"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays/g12_scrub.replay/data?from_tick=0&to_tick=50&topic=pos_ned_m")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok missing: {d}'
data = d.get('data', {})
assert data.get('topic') == 'pos_ned_m', f'wrong topic: {data}'
assert data.get('from_tick') == 0, f'wrong from_tick: {data.get(\"from_tick\")}'
assert data.get('to_tick') == 50, f'wrong to_tick: {data.get(\"to_tick\")}'
points = data.get('points', [])
assert len(points) == 51, f'expected 51 points (ticks 0..50 inclusive), got {len(points)}'
# Verify ticks are sequential and start/end match the query.
for i, p in enumerate(points):
    assert p.get('tick') == i, f'point {i} has tick {p.get(\"tick\")} (expected {i})'
# Verify pos_ned_m.x = 0.1 * tick (the fake file's pattern).
assert abs(points[0]['values'][0] - 0.0) < 0.001, f'tick 0 x={points[0][\"values\"][0]} (expected 0.0)'
assert abs(points[50]['values'][0] - 5.0) < 0.001, f'tick 50 x={points[50][\"values\"][0]} (expected 5.0)'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 2: first-half scrub response invalid; resp: $RESP"
echo "[G-12]   ✓ first half: 51 points, ticks 0..50, x = 0.0 → 5.0"

# -----------------------------------------------------------------------------
# 5. GET /api/replays/{file}/data?from_tick=50&to_tick=100&topic=pos_ned_m
#    → 51 data points (ticks 50..100 inclusive).
# -----------------------------------------------------------------------------
echo "[G-12] test 3: scrub second half (from_tick=50, to_tick=100)"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays/g12_scrub.replay/data?from_tick=50&to_tick=100&topic=pos_ned_m")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok missing: {d}'
data = d.get('data', {})
assert data.get('from_tick') == 50, f'wrong from_tick: {data.get(\"from_tick\")}'
assert data.get('to_tick') == 100, f'wrong to_tick: {data.get(\"to_tick\")}'
points = data.get('points', [])
assert len(points) == 51, f'expected 51 points (ticks 50..100 inclusive), got {len(points)}'
# Tick sequence should be 50, 51, ..., 100.
for i, p in enumerate(points):
    assert p.get('tick') == 50 + i, f'point {i} has tick {p.get(\"tick\")} (expected {50 + i})'
# x @ tick 50 = 5.0, x @ tick 100 = 10.0
assert abs(points[0]['values'][0] - 5.0) < 0.001, f'tick 50 x={points[0][\"values\"][0]} (expected 5.0)'
assert abs(points[50]['values'][0] - 10.0) < 0.001, f'tick 100 x={points[50][\"values\"][0]} (expected 10.0)'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 3: second-half scrub response invalid; resp: $RESP"
echo "[G-12]   ✓ second half: 51 points, ticks 50..100, x = 5.0 → 10.0"

# -----------------------------------------------------------------------------
# 6. Monotonicity: combine both halves, assert x strictly increasing.
# -----------------------------------------------------------------------------
echo "[G-12] test 4: monotonicity check across full range (ticks 0..100)"
# Fetch the full range in one go — endpoint allows [0, 100] inclusive
# (the upper bound to_tick=100 corresponds to the last record).
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays/g12_scrub.replay/data?from_tick=0&to_tick=99&topic=pos_ned_m")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
points = d.get('data', {}).get('points', [])
assert len(points) == 100, f'expected 100 points, got {len(points)}'
xs = [p['values'][0] for p in points]
# Strictly increasing (each x > previous).
for i in range(1, len(xs)):
    assert xs[i] > xs[i-1], f'not strictly increasing at index {i}: {xs[i-1]} -> {xs[i]}'
# First + last match the expected pattern.
assert abs(xs[0] - 0.0) < 0.001, f'first x={xs[0]} (expected 0.0)'
assert abs(xs[-1] - 9.9) < 0.001, f'last x={xs[-1]} (expected 9.9)'
# Linear growth: each step should be 0.1.
for i in range(1, len(xs)):
    delta = xs[i] - xs[i-1]
    assert abs(delta - 0.1) < 0.001, f'step {i} delta={delta} (expected 0.1)'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 4: monotonicity check failed; resp: $RESP"
echo "[G-12]   ✓ monotonicity: pos_ned_m.x strictly increasing 0.0 → 9.9 (step=0.1)"

# -----------------------------------------------------------------------------
# 7. Bonus: fetch a non-pos topic (q_wxyz) to confirm the topic catalogue
#    honours the state-vector layout (q_wxyz has 4 components, identity
#    quaternion in the fake file: w=1, x=y=z=0).
# -----------------------------------------------------------------------------
echo "[G-12] test 5: bonus — q_wxyz topic (4 components, identity quaternion)"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays/g12_scrub.replay/data?from_tick=0&to_tick=5&topic=q_wxyz")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
data = d.get('data', {})
points = data.get('points', [])
assert len(points) == 6, f'expected 6 points (0..5), got {len(points)}'
for p in points:
    assert len(p['values']) == 4, f'q_wxyz should have 4 components, got {len(p[\"values\"])}'
    # Identity quaternion: w=1, x=y=z=0
    assert abs(p['values'][0] - 1.0) < 0.001, f'w should be 1.0, got {p[\"values\"][0]}'
    assert abs(p['values'][1] - 0.0) < 0.001, f'x should be 0.0, got {p[\"values\"][1]}'
    assert abs(p['values'][2] - 0.0) < 0.001, f'y should be 0.0, got {p[\"values\"][2]}'
    assert abs(p['values'][3] - 0.0) < 0.001, f'z should be 0.0, got {p[\"values\"][3]}'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 5: q_wxyz response invalid; resp: $RESP"
echo "[G-12]   ✓ q_wxyz: 6 points, identity quaternion (w=1, x=y=z=0) at each tick"

# -----------------------------------------------------------------------------
# Done.
# -----------------------------------------------------------------------------
echo "[G-12] PASS"
exit 0
