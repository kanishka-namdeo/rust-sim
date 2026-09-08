#!/usr/bin/env bash
# =============================================================================
# G-8: Vehicle Setup extensions — param search/filter, preset save/load
# round-trip, diff-against-defaults (GCS_SPEC.md §9, gate G-8).
#
# Single-invocation harness: start a Python mock PX4 (sends telemetry +
# responds to PARAM_REQUEST_LIST + PARAM_SET), start mavfleet (:8400),
# start fleet-catalog (:8300), exercise the 5 G-8 sub-tests, teardown.
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MOCK_PY="$ROOT/console/tests/mock_px4_fly.py"
WORK="$(mktemp -d -t g8_test.XXXXXX.dir)"
CATALOG_DIR="$WORK/catalog"

# Background PIDs collected for cleanup
PIDS=()

cleanup() {
    for pid in "${PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

log() { printf '[G-8] %s\n' "$*"; }
fail() { printf '[G-8] FAIL: %s\n' "$*"; exit 1; }

# Retry a curl call up to 3 times with 1s sleep
curl_retry() {
    local url="$1"
    local resp=""
    for i in 1 2 3; do
        resp=$(curl -s "$url" 2>/dev/null || true)
        [ -n "$resp" ] && break
        sleep 1
    done
    echo "$resp"
}

# ---- port-in-use preflight -------------------------------------------------
PORTS=(14580 14540 8400 8300)
for p in "${PORTS[@]}"; do
    if ss -lun 2>/dev/null | grep -q ":$p " ; then
        fail "port $p in use; clear it before running"
    fi
done

# ---- 1. Start mock PX4 -----------------------------------------------------
log "starting mock PX4 (param-list + param-set echo)"
export G8_PARAM_LIST=1  # mock_px4_fly.py honours this to enable param responses
python3 "$MOCK_PY" --instance 0 --ttl-secs 300 > "$WORK/mock.log" 2>&1 &
MOCK_PID=$!
PIDS+=("$MOCK_PID")
sleep 2
kill -0 "$MOCK_PID" 2>/dev/null || fail "mock PX4 failed to start (see $WORK/mock.log)"

# ---- 2. Start mavfleet (:8400) with 1 vehicle -----------------------------
CATALOG_PORT=8300
FLEET_PORT=8400

# Build a 1-vehicle scenario that holds for setup (no mission, just READY)
SCENARIO="$WORK/g8_scenario.toml"
cat > "$SCENARIO" <<'EOF'
[fleet]
count = 1
battery_sim = false
hold_for_setup = true

[env]
geofence = { points_ned_m = [[-110,-110],[110,-110],[110,110],[-110,110]], ceiling_m = 60, floor_m = 0 }

[sim]
command = "/bin/sleep {duration_s}"
duration_s = 600
EOF

cd "$ROOT/fleet"
source "$HOME/.cargo/env" 2>/dev/null || true

# Create a fake px4 binary (the G-7 pattern) so mavfleet's spawn succeeds
# without a real PX4 SITL build.
FAKE_PX4_DIR="$WORK/fake-px4"
FAKE_PX4_BIN="$FAKE_PX4_DIR/build/px4_sitl_default/bin/px4"
FAKE_PX4_ETC="$FAKE_PX4_DIR/build/px4_sitl_default/etc"
mkdir -p "$(dirname "$FAKE_PX4_BIN")" "$FAKE_PX4_ETC"
cat > "$FAKE_PX4_BIN" <<'PEOF'
#!/bin/sh
# Fake px4 binary — just sleeps so mavfleet's spawn doesn't fail.
# The mock PX4 (mock_px4_fly.py) plays the real MAVLink role over UDP.
sleep 600
PEOF
chmod +x "$FAKE_PX4_BIN"

FLEET_PX4_DIR="$FAKE_PX4_DIR" \
FLEET_SIM_COMMAND="/bin/sleep {duration_s}" \
    ./target/debug/mavfleet run --fleet "$SCENARIO" --api-port $FLEET_PORT > "$WORK/mavfleet.log" 2>&1 &
FLEET_PID=$!
PIDS+=("$FLEET_PID")
sleep 4
kill -0 "$FLEET_PID" 2>/dev/null || fail "mavfleet failed to start (see $WORK/mavfleet.log)"

# Wait for the link to establish, then trigger param download
log "waiting for link to establish"
for i in $(seq 1 15); do
    RESP=$(curl -s "http://127.0.0.1:$FLEET_PORT/api/vehicles/0" 2>/dev/null || echo '{}')
    HB=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('data',{}).get('heartbeat_seen', False))" 2>/dev/null || echo False)
    [ "$HB" = "True" ] && break
    sleep 1
done

log "triggering param download (PARAM_REQUEST_LIST)"
curl -s -X POST "http://127.0.0.1:$FLEET_PORT/api/vehicles/0/params/refresh" > /dev/null 2>&1 || true

log "waiting for param download"
for i in $(seq 1 30); do
    RESP=$(curl -s "http://127.0.0.1:$FLEET_PORT/api/vehicles/0/params" 2>/dev/null || echo '{}')
    RECV=$(echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('data',{}).get('received',0))" 2>/dev/null || echo 0)
    if [ "$RECV" -gt 0 ]; then
        log "params received: $RECV"
        break
    fi
    sleep 1
done
[ "$RECV" -gt 0 ] || fail "no params received from mock PX4 after 30s"

# ---- 3. Start fleet-catalog (:8300) ----------------------------------------
log "starting fleet-catalog on :$CATALOG_PORT"
"$ROOT/fleet/target/debug/fleet-catalog" \
    --port $CATALOG_PORT \
    --catalog-dir "$CATALOG_DIR" > "$WORK/catalog.log" 2>&1 &
CATALOG_PID=$!
PIDS+=("$CATALOG_PID")
sleep 2
kill -0 "$CATALOG_PID" 2>/dev/null || fail "fleet-catalog failed to start"
curl -s "http://127.0.0.1:$CATALOG_PORT/api/health" | grep -q '"ok":true' || fail "catalog health check failed"

# ---- Test 1: Param search --------------------------------------------------
log "Test 1: param search (search=ROLLRATE)"
RESP=$(curl -s "http://127.0.0.1:$FLEET_PORT/api/vehicles/0/params?search=ROLLRATE")
COUNT=$(echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
params = d.get('data',{}).get('params',[])
print(len(params))
# Assert all have ROLLRATE in id
for p in params:
    assert 'ROLLRATE' in p['id'].upper(), f\"search returned non-matching: {p['id']}\"
# Assert all have group, default, is_changed
for p in params:
    assert 'group' in p, f\"missing group on {p['id']}\"
    assert 'default' in p, f\"missing default on {p['id']}\"
    assert 'is_changed' in p, f\"missing is_changed on {p['id']}\"
print('OK', file=sys.stderr)
" 2>"$WORK/t1.err")
echo "$COUNT" | grep -qE '^[0-9]+$' || fail "Test 1: search response invalid"
[ "$COUNT" -ge 3 ] || fail "Test 1: expected ≥3 ROLLRATE params, got $COUNT"
log "  ✓ search returned $COUNT ROLLRATE params with group/default/is_changed"

# ---- Test 2: Group filter --------------------------------------------------
log "Test 2: group filter (group=MPC)"
RESP=$(curl -s "http://127.0.0.1:$FLEET_PORT/api/vehicles/0/params?group=MPC")
COUNT=$(echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
params = d.get('data',{}).get('params',[])
for p in params:
    assert p.get('group') == 'MPC', f\"group filter returned non-MPC: {p.get('id')} group={p.get('group')}\"
print(len(params))
")
[ "$COUNT" -ge 4 ] || fail "Test 2: expected ≥4 MPC params, got $COUNT"
log "  ✓ group filter returned $COUNT MPC params"

# ---- Test 3: Diff-against-defaults (is_changed) ---------------------------
log "Test 3: diff-against-defaults (all params have fields, is_changed=false)"
RESP=$(curl -s --retry 3 --retry-delay 1 "http://127.0.0.1:$FLEET_PORT/api/vehicles/0/params")
[ -n "$RESP" ] || fail "Test 3: empty response from params endpoint"
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
params = d.get('data',{}).get('params',[])
assert len(params) >= 15, f'expected ≥15 params, got {len(params)}'
for p in params:
    assert 'group' in p, f\"missing group on {p['id']}\"
    assert 'default' in p, f\"missing default on {p['id']}\"
    assert 'is_changed' in p, f\"missing is_changed on {p['id']}\"
    # Values match defaults → is_changed should be false
    if p.get('default') is not None:
        assert p['is_changed'] == False, f\"{p['id']} should not be changed (value={p['value']}, default={p['default']})\"
print('OK')
" || fail "Test 3: diff-against-defaults fields missing or incorrect"
log "  ✓ all params have group/default/is_changed; is_changed=false (values match defaults)"

# ---- Test 4: Write a param + verify is_changed ----------------------------
log "Test 4: write MPC_XY_VEL_MAX=8.0, verify is_changed=true"
curl -s -X POST "http://127.0.0.1:$FLEET_PORT/api/vehicles/0/params" \
    -H "Content-Type: application/json" \
    -d '{"id":"MPC_XY_VEL_MAX","value":8.0,"param_type":9}' > /dev/null
sleep 2  # wait for PARAM_VALUE echo

RESP=$(curl -s --retry 3 --retry-delay 1 "http://127.0.0.1:$FLEET_PORT/api/vehicles/0/params?search=MPC_XY_VEL_MAX")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
params = d.get('data',{}).get('params',[])
assert len(params) == 1, f'expected 1, got {len(params)}'
p = params[0]
assert abs(p['value'] - 8.0) < 0.01, f\"value={p['value']} expected 8.0\"
assert abs(p['default'] - 12.0) < 0.01, f\"default={p['default']} expected 12.0\"
assert p['is_changed'] == True, f\"is_changed should be true (value=8.0, default=12.0)\"
print('OK')
" || fail "Test 4: is_changed not true after writing different value"
log "  ✓ MPC_XY_VEL_MAX=8.0, default=12.0, is_changed=true"

# ---- Test 5: Preset save/load round-trip ----------------------------------
log "Test 5: preset save/load round-trip"
# Save
curl -s -X POST "http://127.0.0.1:$CATALOG_PORT/api/vehicles/0/param-presets" \
    -H "Content-Type: application/json" \
    -d '{"name":"test-preset","params":[{"id":"MPC_XY_VEL_MAX","value":8.0,"type":9},{"id":"MC_ROLLRATE_P","value":7.5,"type":9}]}' \
    | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') == True, f'save failed: {d}'
assert d.get('param_count') == 2, f\"param_count={d.get('param_count')} expected 2\"
print('saved')
" || fail "Test 5a: preset save failed"

# List
RESP=$(curl -s "http://127.0.0.1:$CATALOG_PORT/api/vehicles/0/param-presets")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
presets = d if isinstance(d, list) else d.get('data', d)
assert any(p['name'] == 'test-preset' for p in presets), f'test-preset not in list: {presets}'
print('listed')
" || fail "Test 5b: preset list failed"

# Load
curl -s -X POST "http://127.0.0.1:$CATALOG_PORT/api/vehicles/0/param-presets/test-preset/load" \
    | python3 -c "
import sys, json
d = json.load(sys.stdin)
data = d.get('data', d)
params = data.get('params', [])
assert len(params) == 2, f'expected 2 params, got {len(params)}'
assert params[0]['id'] == 'MPC_XY_VEL_MAX'
assert abs(params[0]['value'] - 8.0) < 0.01
assert params[1]['id'] == 'MC_ROLLRATE_P'
assert abs(params[1]['value'] - 7.5) < 0.01
print('loaded')
" || fail "Test 5c: preset load failed"

# Delete
curl -s -X DELETE "http://127.0.0.1:$CATALOG_PORT/api/vehicles/0/param-presets/test-preset" \
    | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') == True, f'delete failed: {d}'
print('deleted')
" || fail "Test 5d: preset delete failed"

# Verify deleted
RESP=$(curl -s "http://127.0.0.1:$CATALOG_PORT/api/vehicles/0/param-presets")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
presets = d if isinstance(d, list) else d.get('data', d)
assert len(presets) == 0, f'preset list not empty after delete: {presets}'
print('gone')
" || fail "Test 5e: preset not deleted"

log "  ✓ preset save → list → load → delete round-trip complete"

# ---- Done ------------------------------------------------------------------
log "PASS"
exit 0
