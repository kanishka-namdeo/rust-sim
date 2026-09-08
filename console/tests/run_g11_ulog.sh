#!/usr/bin/env bash
# =============================================================================
# G-11: ULog browse + plot (GCS_SPEC.md §9, gate G-11).
#
# Single-invocation harness: start fleet-catalog on :8300 with a temp
# catalog dir, drop a fake .replay file + a stub .ulg file into the
# catalog's `replays/` and `ulogs/` dirs, exercise the six Analyze
# View endpoints, assert each returns valid JSON with the expected
# shape, tear down, exit 0 on PASS / non-zero on FAIL.
#
# Per the task spec, this gate covers:
#   - GET /api/ulogs              → list (empty or files)
#   - GET /api/ulogs/{file}/topics → 404 if file missing (pyulog not
#                                    required; we don't have a real
#                                    .ulg in the sandbox)
#   - GET /api/replays             → list (1 file)
#   - GET /api/replays/{file}/meta → header parse (records, tick_rate_hz,
#                                    virtual_duration_s)
#   - GET /api/replays/{file}/topics → static topic catalogue
#   - GET /api/replays/{file}/data?from_tick&to_tick&topic → JSON range
#
# No real PX4 / pyulog / SITL involved — the .replay file is generated
# by `mk_replay.py` (a pure-Python writer that emits the v1 binary
# format documented in `sim/docs/SPEC.md` §8.2 with correct CRC-16/X.25).
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CATALOG_BIN="${CATALOG_BIN:-$ROOT/fleet/target/debug/fleet-catalog}"
MK_REPLAY="$ROOT/console/tests/mk_replay.py"

PORT_CATALOG=8300
WORK="$(mktemp -d -t g11_test.XXXXXX.dir)"
CATALOG_DIR="$WORK/catalog"
CATALOG_LOG="$WORK/catalog.log"
REPLAY_FILE="g11_flight.replay"
ULOG_FILE="g11_sample.ulg"

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
    if [ "${G11_KEEP:-0}" = "1" ]; then
        echo "[G-11] keeping work dir: $WORK"
    else
        rm -rf "$WORK" 2>/dev/null || true
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-11] FAIL: $1"
    echo "=========== diagnostics ==========="
    echo "--- catalog.log (tail 20):"
    tail -20 "$CATALOG_LOG" 2>/dev/null || echo "(none)"
    echo "--- work dir: $WORK"
    G11_KEEP=1
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -x "$CATALOG_BIN" ] || fail "fleet-catalog binary missing at $CATALOG_BIN (build fleet workspace first)"
[ -f "$MK_REPLAY" ] || fail "mk_replay.py helper missing at $MK_REPLAY"
command -v curl >/dev/null || fail "curl not available"
command -v python3 >/dev/null || fail "python3 not available"

# Refuse to run if port already taken.
if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" >/dev/null 2>&1; then
    fail "port $PORT_CATALOG already in use (previous run?)"
fi

# -----------------------------------------------------------------------------
# 1. Start fleet-catalog on :8300 with a temp catalog dir.
# -----------------------------------------------------------------------------
echo "[G-11] binary:    $CATALOG_BIN"
echo "[G-11] work dir:  $WORK"
echo "[G-11] starting fleet-catalog on :$PORT_CATALOG"

"$CATALOG_BIN" --port "$PORT_CATALOG" --catalog-dir "$CATALOG_DIR" \
    >"$CATALOG_LOG" 2>&1 &
CATALOG_PID=$!

# Wait for the catalog health endpoint.
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
echo "[G-11] catalog up (pid $CATALOG_PID)"

# -----------------------------------------------------------------------------
# 2. Drop a fake .replay file (20 records, 200 Hz = 0.1 s virtual duration)
#    into the catalog's replays/ dir.
# -----------------------------------------------------------------------------
python3 "$MK_REPLAY" "$CATALOG_DIR/replays/$REPLAY_FILE" 20 200 42

# Drop a stub .ulg file (1 byte — not a real ULog, but listable on the
# filesystem). pyulog is NOT installed in the sandbox, so we cannot
# exercise the topics/data endpoints with a real .ulg; the G-11 task
# spec explicitly allows this and limits assertions to (a) the list
# endpoint and (b) the 404 path for nonexistent files.
printf '\x00' > "$CATALOG_DIR/ulogs/$ULOG_FILE"

echo "[G-11] seeded $REPLAY_FILE (20 records) + $ULOG_FILE (stub)"

# -----------------------------------------------------------------------------
# 3. GET /api/ulogs → assert list contains our stub .ulg file.
# -----------------------------------------------------------------------------
echo "[G-11] test 1: GET /api/ulogs"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/ulogs")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok field missing/false: {d}'
files = d.get('data', [])
assert any(f['filename'] == 'g11_sample.ulg' for f in files), \
    f'g11_sample.ulg not in list: {files}'
# Each entry should have filename + size_bytes + mtime
for f in files:
    assert 'filename' in f, f'missing filename: {f}'
    assert 'size_bytes' in f, f'missing size_bytes: {f}'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 1: /api/ulogs response invalid; resp: $RESP"
echo "[G-11]   ✓ /api/ulogs lists g11_sample.ulg"

# -----------------------------------------------------------------------------
# 4. GET /api/ulogs/nonexistent.ulg/topics → 404 (file not found).
# -----------------------------------------------------------------------------
echo "[G-11] test 2: GET /api/ulogs/nonexistent.ulg/topics → 404"
HTTP_FILE="$WORK/http_code"
BODY_FILE="$WORK/ulog_404.json"
curl -s --max-time 3 -o "$BODY_FILE" -w "%{http_code}" \
    "http://127.0.0.1:$PORT_CATALOG/api/ulogs/nonexistent.ulg/topics" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
ERR_CODE=$(python3 -c "import json; d=json.load(open('$BODY_FILE')); print(d.get('error',{}).get('code',''))" 2>/dev/null || echo "")
[ "$HTTP_CODE" = "404" ] || fail "test 2: expected HTTP 404 for nonexistent .ulg, got $HTTP_CODE; body: $(cat "$BODY_FILE")"
[ "$ERR_CODE" = "ULOG_NOT_FOUND" ] || fail "test 2: expected error.code=ULOG_NOT_FOUND, got '$ERR_CODE'; body: $(cat "$BODY_FILE")"
echo "[G-11]   ✓ 404 ULOG_NOT_FOUND for nonexistent.ulg"

# -----------------------------------------------------------------------------
# 5. GET /api/replays → list should contain our fake replay file.
# -----------------------------------------------------------------------------
echo "[G-11] test 3: GET /api/replays"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok field missing/false: {d}'
files = d.get('data', [])
assert len(files) == 1, f'expected 1 replay, got {len(files)}: {files}'
assert files[0]['filename'] == 'g11_flight.replay', f'wrong filename: {files[0]}'
# size = 64-byte header + 20 * 96-byte records = 1984 bytes
assert files[0]['size_bytes'] == 64 + 20 * 96, f'wrong size: {files[0][\"size_bytes\"]}'
assert 'mtime' in files[0], f'missing mtime: {files[0]}'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 3: /api/replays response invalid; resp: $RESP"
echo "[G-11]   ✓ /api/replays lists g11_flight.replay (1984 bytes)"

# -----------------------------------------------------------------------------
# 6. GET /api/replays/{file}/meta → header parse (records=20, rate=200, duration=0.1).
# -----------------------------------------------------------------------------
echo "[G-11] test 4: GET /api/replays/g11_flight.replay/meta"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays/g11_flight.replay/meta")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok missing: {d}'
m = d.get('data', {})
assert m.get('filename') == 'g11_flight.replay', f'wrong filename: {m}'
assert m.get('records') == 20, f'expected 20 records, got {m.get(\"records\")}'
assert abs(m.get('tick_rate_hz', 0) - 200.0) < 0.001, f'expected 200 Hz, got {m.get(\"tick_rate_hz\")}'
assert abs(m.get('virtual_duration_s', -1) - 0.1) < 0.001, \
    f'expected 0.1 s duration, got {m.get(\"virtual_duration_s\")}'
assert m.get('seed') == 42, f'expected seed=42, got {m.get(\"seed\")}'
assert len(m.get('scenario_sha256', '')) == 64, f'expected 64-char hash, got {m.get(\"scenario_sha256\")}'
assert m.get('version') == 1, f'expected version=1, got {m.get(\"version\")}'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 4: /api/replays/.../meta response invalid; resp: $RESP"
echo "[G-11]   ✓ meta: records=20, tick_rate_hz=200, virtual_duration_s=0.1, seed=42"

# -----------------------------------------------------------------------------
# 7. GET /api/replays/{file}/topics → static topic catalogue.
# -----------------------------------------------------------------------------
echo "[G-11] test 5: GET /api/replays/g11_flight.replay/topics"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays/g11_flight.replay/topics")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok missing: {d}'
topics = d.get('data', [])
names = {t['name'] for t in topics}
required = {'pos_ned_m', 'vel_ned_ms', 'q_wxyz', 'omega_rads', 'rotors'}
missing = required - names
assert not missing, f'missing topics: {missing}; got {names}'
for t in topics:
    assert 'label' in t, f'missing label on {t}'
    assert 'unit' in t, f'missing unit on {t}'
    assert 'components' in t, f'missing components on {t}'
# pos_ned_m must have 3 components (x, y, z)
pos = next(t for t in topics if t['name'] == 'pos_ned_m')
assert len(pos['components']) == 3, f'pos_ned_m should have 3 components, got {pos}'
# q_wxyz must have 4 components (w, x, y, z)
q = next(t for t in topics if t['name'] == 'q_wxyz')
assert len(q['components']) == 4, f'q_wxyz should have 4 components, got {q}'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 5: /api/replays/.../topics response invalid; resp: $RESP"
echo "[G-11]   ✓ topics: pos_ned_m(3), vel_ned_ms(3), q_wxyz(4), omega_rads(3), rotors(4)"

# -----------------------------------------------------------------------------
# 8. GET /api/replays/{file}/data?from_tick=0&to_tick=10&topic=pos_ned_m
#    → 11 data points, each with 3 component values.
# -----------------------------------------------------------------------------
echo "[G-11] test 6: GET /api/replays/g11_flight.replay/data?from_tick=0&to_tick=10&topic=pos_ned_m"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/replays/g11_flight.replay/data?from_tick=0&to_tick=10&topic=pos_ned_m")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok missing: {d}'
data = d.get('data', {})
assert data.get('topic') == 'pos_ned_m', f'wrong topic: {data.get(\"topic\")}'
assert data.get('from_tick') == 0, f'wrong from_tick: {data.get(\"from_tick\")}'
assert data.get('to_tick') == 10, f'wrong to_tick: {data.get(\"to_tick\")}'
points = data.get('points', [])
assert len(points) == 11, f'expected 11 points (ticks 0..10 inclusive), got {len(points)}'
# Each point should have tick, t_s, values (3 floats)
for i, p in enumerate(points):
    assert p.get('tick') == i, f'point {i} has tick {p.get(\"tick\")} (expected {i})'
    assert 't_s' in p, f'point {i} missing t_s'
    assert 'values' in p, f'point {i} missing values'
    assert len(p['values']) == 3, f'point {i} has {len(p[\"values\"])} values (expected 3)'
# pos_ned_m.x at tick 0 = 0.0, at tick 1 = 0.1, at tick 10 = 1.0
assert abs(points[0]['values'][0] - 0.0) < 0.001, f'tick 0 x={points[0][\"values\"][0]} (expected 0.0)'
assert abs(points[1]['values'][0] - 0.1) < 0.001, f'tick 1 x={points[1][\"values\"][0]} (expected 0.1)'
assert abs(points[10]['values'][0] - 1.0) < 0.001, f'tick 10 x={points[10][\"values\"][0]} (expected 1.0)'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 6: /api/replays/.../data response invalid; resp: $RESP"
echo "[G-11]   ✓ data: 11 points (ticks 0..10), pos_ned_m.x = 0.0, 0.1, ..., 1.0"

# -----------------------------------------------------------------------------
# 9. Negative tests: nonexistent replay file, unknown topic, range OOB.
# -----------------------------------------------------------------------------
echo "[G-11] test 7: negative paths"
# Nonexistent replay file → 404
HTTP_FILE="$WORK/http_404_meta"
BODY_FILE="$WORK/404_meta.json"
curl -s --max-time 3 -o "$BODY_FILE" -w "%{http_code}" \
    "http://127.0.0.1:$PORT_CATALOG/api/replays/nonexistent.replay/meta" >"$HTTP_FILE" || true
[ "$(cat "$HTTP_FILE")" = "404" ] || fail "test 7a: nonexistent replay meta should be 404, got $(cat "$HTTP_FILE")"

# Unknown topic → 404
HTTP_FILE="$WORK/http_404_topic"
BODY_FILE="$WORK/404_topic.json"
curl -s --max-time 3 -o "$BODY_FILE" -w "%{http_code}" \
    "http://127.0.0.1:$PORT_CATALOG/api/replays/g11_flight.replay/data?from_tick=0&to_tick=5&topic=not_a_topic" >"$HTTP_FILE" || true
[ "$(cat "$HTTP_FILE")" = "404" ] || fail "test 7b: unknown topic should be 404, got $(cat "$HTTP_FILE")"
ERR_CODE=$(python3 -c "import json; print(json.load(open('$BODY_FILE')).get('error',{}).get('code',''))" 2>/dev/null || echo "")
[ "$ERR_CODE" = "TOPIC_NOT_FOUND" ] || fail "test 7b: expected TOPIC_NOT_FOUND, got '$ERR_CODE'"

# Range out of bounds → 400
HTTP_FILE="$WORK/http_400_oob"
BODY_FILE="$WORK/400_oob.json"
curl -s --max-time 3 -o "$BODY_FILE" -w "%{http_code}" \
    "http://127.0.0.1:$PORT_CATALOG/api/replays/g11_flight.replay/data?from_tick=0&to_tick=20&topic=pos_ned_m" >"$HTTP_FILE" || true
[ "$(cat "$HTTP_FILE")" = "400" ] || fail "test 7c: range OOB should be 400, got $(cat "$HTTP_FILE")"
ERR_CODE=$(python3 -c "import json; print(json.load(open('$BODY_FILE')).get('error',{}).get('code',''))" 2>/dev/null || echo "")
[ "$ERR_CODE" = "RANGE_OUT_OF_BOUNDS" ] || fail "test 7c: expected RANGE_OUT_OF_BOUNDS, got '$ERR_CODE'"

echo "[G-11]   ✓ negative paths: 404 (missing replay), 404 (unknown topic), 400 (range OOB)"

# -----------------------------------------------------------------------------
# Done.
# -----------------------------------------------------------------------------
echo "[G-11] PASS"
exit 0
