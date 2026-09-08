#!/usr/bin/env bash
# =============================================================================
# G-8: Vehicle Setup extensions (GCS_SPEC.md §9, gate G-8).
#
# Single-invocation harness: start a Python mock PX4 (extends M3's
# mock_px4_fly.py with PARAM_REQUEST_LIST + PARAM_SET handlers — emits
# 20 PARAM_VALUE frames for the GCS's full param download, echoes
# PARAM_VALUE back on every param write) + the real fleet-cli (mavfleet)
# on a 1-vehicle scenario + the real fleet-catalog on :8300 with a
# throwaway catalog dir, then drive the QGC Vehicle-Setup param flow
# via HTTP and assert every step.
#
# Wire topology (matches fleet-mavlink/src/link.rs §3.1):
#   fleet-cli mavlink link binds 127.0.0.1:14540, sends to 127.0.0.1:14580
#   mock PX4 binds 127.0.0.1:14580, sends telemetry to 127.0.0.1:14540
#   fleet-cli control plane listens on :8400 (vehicle params endpoint)
#   fleet-catalog listens on :8300 (param-presets endpoints)
#
# Asserts (GCS_SPEC.md §5.3, ADR-0016):
#   (a) Param search filter: GET /api/vehicles/0/params?search=ROLLRATE
#       returns only params whose id contains ROLLRATE (case-insensitive
#       substring), each with the right group + default + is_changed=false.
#   (b) Group filter: GET /api/vehicles/0/params?group=MPC returns only
#       params in the MPC group.
#   (c) Diff-against-defaults: every param has group/default/is_changed
#       fields; right after a fresh param download all is_changed=false
#       (values match PX4's compiled-in defaults).
#   (d) Write a param + verify is_changed: POST /params writes
#       MPC_XY_VEL_MAX=8.0; afterwards GET /params?search=MPC_XY_VEL_MAX
#       returns value=8.0, default=12.0, is_changed=true.
#   (e) Preset save/load round-trip: POST /param-presets on :8300 saves
#       2 params; GET /param-presets lists it; POST
#       /param-presets/test-preset/load returns the saved params; DELETE
#       removes it; GET /param-presets → empty list.
#
# Environment overrides:
#   MAVFLEET_BIN   (default <repo>/fleet/target/debug/mavfleet)
#   CATALOG_BIN    (default <repo>/fleet/target/debug/fleet-catalog)
#   PY             (default python3)
#   MOCK_BIN       (default <repo>/console/tests/mock_px4_fly.py)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MAVFLEET_BIN="${MAVFLEET_BIN:-$ROOT/fleet/target/debug/mavfleet}"
CATALOG_BIN="${CATALOG_BIN:-$ROOT/fleet/target/debug/fleet-catalog}"
PY="${PY:-python3}"
MOCK_BIN="${MOCK_BIN:-$ROOT/console/tests/mock_px4_fly.py}"

PORT_GCS_BIND=14540    # Rust link's bind port (GCS receives heartbeats here)
PORT_PX4_LISTEN=14580   # PX4 onboard listen port (mock binds here)
PORT_API=8400           # fleet-cli control plane (vehicle params endpoint)
PORT_CATALOG=8300       # fleet-catalog (param-presets endpoint)

WORK="$(mktemp -d -t g8_test.XXXXXX.dir)"
SCENARIO_TOML="$WORK/scenario.toml"
MOCK_STATE="$WORK/mock_state.json"
MOCK_LOG="$WORK/mock.log"
FC_LOG="$WORK/fleet.log"
CATALOG_DIR="$WORK/catalog"
CATALOG_LOG="$WORK/catalog.log"
PARAMS_JSON="$WORK/params.json"
SEARCH_JSON="$WORK/search.json"
GROUP_JSON="$WORK/group.json"
WRITE_JSON="$WORK/write.json"
PRESET_SAVE_JSON="$WORK/preset_save.json"
PRESET_LIST_JSON="$WORK/preset_list.json"
PRESET_LOAD_JSON="$WORK/preset_load.json"
PRESET_DEL_JSON="$WORK/preset_del.json"
PRESET_LIST2_JSON="$WORK/preset_list2.json"

# Fake PX4 binary tree (see G-5/G-7 for the rationale).
FAKE_PX4_DIR="$WORK/fake-px4"
FAKE_PX4_BIN="$FAKE_PX4_DIR/build/px4_sitl_default/bin/px4"
FAKE_PX4_ETC="$FAKE_PX4_DIR/build/px4_sitl_default/etc"

FC_PID=""
CATALOG_PID=""
MOCK_PID=""

cleanup() {
    for pid in "$FC_PID" "$CATALOG_PID" "$MOCK_PID"; do
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            for _ in $(seq 1 20); do
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.1
            done
            kill -9 "$pid" 2>/dev/null || true
        fi
    done
    wait 2>/dev/null || true
    if [ -z "${G8_FAILED:-}" ]; then
        rm -rf "$WORK" 2>/dev/null || true
    else
        echo "[G-8] work dir preserved at $WORK (G8_FAILED=1)"
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-8] FAIL: $1"
    G8_FAILED=1
    echo "=========== diagnostics ==========="
    echo "--- mock.log (tail 30):"
    tail -30 "$MOCK_LOG" 2>/dev/null || echo "(none)"
    echo "--- fleet.log (tail 40):"
    tail -40 "$FC_LOG" 2>/dev/null || echo "(none)"
    echo "--- catalog.log (tail 20):"
    tail -20 "$CATALOG_LOG" 2>/dev/null || echo "(none)"
    echo "--- mock_state.json:"
    cat "$MOCK_STATE" 2>/dev/null | "$PY" -m json.tool 2>/dev/null \
        || cat "$MOCK_STATE" 2>/dev/null || echo "(none)"
    echo "--- params.json (last):"
    cat "$PARAMS_JSON" 2>/dev/null | "$PY" -m json.tool 2>/dev/null \
        || cat "$PARAMS_JSON" 2>/dev/null || echo "(none)"
    echo "--- search.json (last):"
    cat "$SEARCH_JSON" 2>/dev/null | "$PY" -m json.tool 2>/dev/null \
        || cat "$SEARCH_JSON" 2>/dev/null || echo "(none)"
    echo "--- preset_save.json:"
    cat "$PRESET_SAVE_JSON" 2>/dev/null | "$PY" -m json.tool 2>/dev/null \
        || cat "$PRESET_SAVE_JSON" 2>/dev/null || echo "(none)"
    echo "--- preset_load.json:"
    cat "$PRESET_LOAD_JSON" 2>/dev/null | "$PY" -m json.tool 2>/dev/null \
        || cat "$PRESET_LOAD_JSON" 2>/dev/null || echo "(none)"
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -x "$MAVFLEET_BIN" ]  || fail "mavfleet binary missing at $MAVFLEET_BIN (build fleet workspace first)"
[ -x "$CATALOG_BIN" ]   || fail "fleet-catalog binary missing at $CATALOG_BIN (build fleet workspace first)"
[ -f "$MOCK_BIN" ]      || fail "mock_px4_fly.py missing at $MOCK_BIN"
command -v "$PY" >/dev/null || fail "python3 not available"
command -v curl >/dev/null || fail "curl not available"
command -v jq   >/dev/null || fail "jq not available"
"$PY" -c "from pymavlink.dialects.v20 import common; print('pymavlink OK')" \
    >/dev/null 2>&1 || fail "pymavlink not importable by $PY"

for p in "$PORT_API" "$PORT_CATALOG"; do
    if curl -s --max-time 1 "http://127.0.0.1:$p/" >/dev/null 2>&1; then
        fail "TCP port $p already in use (previous run?)"
    fi
done
for p in "$PORT_GCS_BIND" "$PORT_PX4_LISTEN"; do
    if ! "$PY" -c "import socket,sys; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); sys.exit(0 if s.bind(('127.0.0.1', $p))==None else 1)" 2>/dev/null; then
        fail "UDP port $p already in use (previous run?)"
    fi
done

# -----------------------------------------------------------------------------
# Fake PX4 binary tree (see G-5/G-7).
# -----------------------------------------------------------------------------
mkdir -p "$(dirname "$FAKE_PX4_BIN")" "$FAKE_PX4_ETC"
cat > "$FAKE_PX4_BIN" <<'SH'
#!/bin/sh
sleep 600
SH
chmod +x "$FAKE_PX4_BIN"

# -----------------------------------------------------------------------------
# Scenario: 1 vehicle, no tasks (vehicle setup only — no mission),
# hold_for_setup so the FSM parks at READY (params are downloadable as
# soon as the link establishes).
# -----------------------------------------------------------------------------
cat > "$SCENARIO_TOML" <<'TOML'
[fleet]
count = 1
battery_sim = false
restart_on_fault = false
hold_for_setup = true

[env]
geofence = { points_ned_m = [[-200,-200],[200,-200],[200,200],[-200,200]], ceiling_m = 120, floor_m = 0 }
wind_steady_ms = [0.0, 0.0, 0.0]
turbulence = "none"

[success]
max_time_s = 90
TOML

"$MAVFLEET_BIN" check "$SCENARIO_TOML" >/dev/null 2>&1 \
    || fail "scenario TOML failed mavfleet check"

echo "[G-8] mock:     $MOCK_BIN"
echo "[G-8] mavfleet: $MAVFLEET_BIN"
echo "[G-8] catalog:  $CATALOG_BIN"
echo "[G-8] ports:    GCS bind :$PORT_GCS_BIND, PX4 listen :$PORT_PX4_LISTEN, API :$PORT_API, catalog :$PORT_CATALOG"
echo "[G-8] work dir: $WORK"

# -----------------------------------------------------------------------------
# Phase 1: start the mock PX4. It binds :14580 and sends telemetry to :14540.
# G-8 also arms the mock's PARAM_REQUEST_LIST + PARAM_SET handlers (added
# in mock_px4_fly.py for the G-8 harness — the param store is seeded with
# the 20 PX4 defaults the backend's PARAM_DEFAULTS table also knows about).
# -----------------------------------------------------------------------------
"$PY" "$MOCK_BIN" \
    --port "$PORT_PX4_LISTEN" \
    --gcs-port "$PORT_GCS_BIND" \
    --state "$MOCK_STATE" \
    --instance 0 \
    --sysid 1 \
    --ttl-secs 90 \
    --telemetry-hz 10 \
    >"$MOCK_LOG" 2>&1 &
MOCK_PID=$!

ok=""
for _ in $(seq 1 50); do
    [ -f "$MOCK_STATE" ] && { ok=1; break; }
    kill -0 "$MOCK_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "mock PX4 did not write state file within 5s"
echo "[G-8] phase 1: mock PX4 up (PID $MOCK_PID)"

# -----------------------------------------------------------------------------
# Phase 2: start fleet-cli (mavfleet). The control plane comes up on
# :$PORT_API; the mavlink link binds :$PORT_GCS_BIND and sends to
# :$PORT_PX4_LISTEN.
# -----------------------------------------------------------------------------
FLEET_PX4_DIR="$FAKE_PX4_DIR" \
FLEET_SIM_COMMAND="/bin/sleep {duration_s}" \
"$MAVFLEET_BIN" run \
    --fleet "$SCENARIO_TOML" \
    --api-port "$PORT_API" \
    --run-dir "$WORK/fleet-run" \
    >"$FC_LOG" 2>&1 &
FC_PID=$!

ok=""
for _ in $(seq 1 100); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/fleet" \
            | grep -q '"ok":true' 2>/dev/null; then
        ok=1; break
    fi
    kill -0 "$FC_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "fleet-cli control plane did not come up on :$PORT_API within 10s"
echo "[G-8] phase 2: fleet-cli control plane up on :$PORT_API (PID $FC_PID)"

# -----------------------------------------------------------------------------
# Phase 3: start fleet-catalog on :$PORT_CATALOG. The catalog owns the
# param-presets CRUD endpoints (POST /api/vehicles/{i}/param-presets etc.),
# persisted under a throwaway catalog dir.
# -----------------------------------------------------------------------------
mkdir -p "$CATALOG_DIR"
"$CATALOG_BIN" --port "$PORT_CATALOG" --catalog-dir "$CATALOG_DIR" \
    --fleet-url "http://127.0.0.1:$PORT_API" >"$CATALOG_LOG" 2>&1 &
CATALOG_PID=$!

ok=""
for _ in $(seq 1 50); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" \
            | grep -q '"service":"fleet-catalog"' 2>/dev/null; then
        ok=1; break
    fi
    kill -0 "$CATALOG_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "fleet-catalog did not come up on :$PORT_CATALOG within 5s"
echo "[G-8] phase 3: fleet-catalog up on :$PORT_CATALOG (PID $CATALOG_PID)"

# -----------------------------------------------------------------------------
# Phase 4: wait for the link to establish — heartbeat_seen must be true.
# -----------------------------------------------------------------------------
VEH_JSON="$WORK/vehicle.json"
ok=""
for _ in $(seq 1 100); do
    curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/vehicles/0" >"$VEH_JSON" 2>/dev/null || true
    if [ -s "$VEH_JSON" ] && jq -e '.ok == true and .data.heartbeat_seen == true' "$VEH_JSON" >/dev/null 2>&1; then
        ok=1; break
    fi
    kill -0 "$FC_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "vehicle 0 heartbeat_seen did not become true within 10s; last vehicle json: $(cat "$VEH_JSON" 2>/dev/null || echo none)"
echo "[G-8] phase 4: link established — heartbeat_seen=true"

# -----------------------------------------------------------------------------
# Phase 5: kick off the full param download. The mock responds to
# PARAM_REQUEST_LIST by streaming 20 PARAM_VALUE frames (the G-8 param
# store seeded from MOCK_PARAMS — the same 20 ids/values the backend's
# PARAM_DEFAULTS table also knows about). The link's param store flips
# to Complete once received_unique >= total.
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/refresh_http"
curl -s --max-time 5 -o /dev/null -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/vehicles/0/params/refresh" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-8] phase 5: POST /params/refresh → HTTP $HTTP_CODE"
[ "$HTTP_CODE" = "200" ] \
    || fail "params refresh returned HTTP $HTTP_CODE (expected 200)"

# Poll GET /params until state == "Complete" (the link's param store
# flips once the 20th PARAM_VALUE lands).
ok=""
for _ in $(seq 1 100); do
    curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/vehicles/0/params" >"$PARAMS_JSON" 2>/dev/null || true
    if [ -s "$PARAMS_JSON" ]; then
        STATE=$(jq -r '.data.state // empty' "$PARAMS_JSON" 2>/dev/null || echo "")
        RECEIVED=$(jq -r '.data.received // 0' "$PARAMS_JSON" 2>/dev/null || echo "0")
        if [ "$STATE" = "Complete" ] || [ "$RECEIVED" -ge 20 ]; then
            ok=1; break
        fi
    fi
    kill -0 "$FC_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "param download did not complete within 10s; last params.json: $(cat "$PARAMS_JSON" 2>/dev/null || echo none)"

RECEIVED=$(jq -r '.data.received' "$PARAMS_JSON")
STATE=$(jq -r '.data.state' "$PARAMS_JSON")
N_PARAMS=$(jq -r '.data.params | length' "$PARAMS_JSON")
echo "[G-8] phase 5: param download complete — received=$RECEIVED state=$STATE params=$N_PARAMS"
[ "$RECEIVED" -ge 20 ] \
    || fail "param download received only $RECEIVED params (expected >= 20)"

# =============================================================================
# Test 1: Param search filter (GCS_SPEC.md §5.3, AC-5.3.1)
#   GET /api/vehicles/0/params?search=ROLLRATE on :8400
#   Assert: response contains only params with "ROLLRATE" in the id
#   Assert: each param has group="MC", default present, is_changed=false
# =============================================================================
echo "[G-8] Test 1: param search filter (search=ROLLRATE)"
curl -s --max-time 3 "http://127.0.0.1:$PORT_API/api/vehicles/0/params?search=ROLLRATE" >"$SEARCH_JSON"
SEARCH_COUNT=$(jq -r '.data.params | length' "$SEARCH_JSON")
echo "[G-8]   search=ROLLRATE → $SEARCH_COUNT params"
[ "$SEARCH_COUNT" = "3" ] \
    || fail "search=ROLLRATE returned $SEARCH_COUNT params (expected 3: MC_ROLLRATE_P/I/D)"

# All returned ids must contain ROLLRATE.
BAD_IDS=$(jq -r '.data.params[] | select(.id | ascii_downcase | test("rollrate") | not) | .id' "$SEARCH_JSON")
[ -z "$BAD_IDS" ] || fail "search=ROLLRATE returned non-matching ids: $BAD_IDS"

# All three expected ids must be present.
EXPECTED_ROLLRATE="MC_ROLLRATE_D MC_ROLLRATE_I MC_ROLLRATE_P"
ACTUAL_ROLLRATE=$(jq -r '.data.params[].id' "$SEARCH_JSON" | sort | tr '\n' ' ' | sed 's/ $//')
[ "$ACTUAL_ROLLRATE" = "$EXPECTED_ROLLRATE" ] \
    || fail "search=ROLLRATE ids mismatch: got '$ACTUAL_ROLLRATE', expected '$EXPECTED_ROLLRATE'"

# Each param must have group=MC, default present, is_changed=false.
for id in MC_ROLLRATE_P MC_ROLLRATE_I MC_ROLLRATE_D; do
    GROUP=$(jq -r ".data.params[] | select(.id==\"$id\") | .group" "$SEARCH_JSON")
    DEFAULT=$(jq -r ".data.params[] | select(.id==\"$id\") | .default" "$SEARCH_JSON")
    CHANGED=$(jq -r ".data.params[] | select(.id==\"$id\") | .is_changed" "$SEARCH_JSON")
    [ "$GROUP" = "MC" ] \
        || fail "param $id group='$GROUP' (expected 'MC')"
    [ "$DEFAULT" != "null" ] \
        || fail "param $id default is null (expected a value)"
    [ "$CHANGED" = "false" ] \
        || fail "param $id is_changed=$CHANGED (expected false — values match defaults)"
done
echo "[G-8]   PASS: 3 ROLLRATE params, all group=MC, default set, is_changed=false"

# =============================================================================
# Test 2: Group filter
#   GET /api/vehicles/0/params?group=MPC on :8400
#   Assert: all returned params have group="MPC" (MPC_XY_VEL_MAX, etc.)
# =============================================================================
echo "[G-8] Test 2: group filter (group=MPC)"
curl -s --max-time 3 "http://127.0.0.1:$PORT_API/api/vehicles/0/params?group=MPC" >"$GROUP_JSON"
GROUP_COUNT=$(jq -r '.data.params | length' "$GROUP_JSON")
echo "[G-8]   group=MPC → $GROUP_COUNT params"
[ "$GROUP_COUNT" -ge 6 ] \
    || fail "group=MPC returned $GROUP_COUNT params (expected >=6: MPC_XY_VEL_MAX, MPC_Z_VEL_MAX_UP, MPC_Z_VEL_MAX_DN, MPC_XY_CRUISE, MPC_CRUISE_90, MPC_TKO_SPEED)"

# Every returned param must have group=MPC.
BAD_GROUPS=$(jq -r '.data.params[] | select(.group != "MPC") | "\(.id)=\(.group)"' "$GROUP_JSON")
[ -z "$BAD_GROUPS" ] || fail "group=MPC returned non-MPC params: $BAD_GROUPS"

# Spot-check: MPC_XY_VEL_MAX (default 12.0) must be present.
HAS_XY_VEL=$(jq -r '[.data.params[] | select(.id=="MPC_XY_VEL_MAX")] | length' "$GROUP_JSON")
[ "$HAS_XY_VEL" = "1" ] \
    || fail "MPC_XY_VEL_MAX not in group=MPC results"
echo "[G-8]   PASS: $GROUP_COUNT MPC-group params, all group=MPC, MPC_XY_VEL_MAX present"

# =============================================================================
# Test 3: Diff-against-defaults (no filter)
#   GET /api/vehicles/0/params (no filter)
#   Assert: every param has group, default, is_changed fields
#   Assert: is_changed is false for all params (values match defaults)
# =============================================================================
echo "[G-8] Test 3: diff-against-defaults (no filter)"
curl -s --max-time 3 "http://127.0.0.1:$PORT_API/api/vehicles/0/params" >"$PARAMS_JSON"
TOTAL=$(jq -r '.data.params | length' "$PARAMS_JSON")
echo "[G-8]   full list → $TOTAL params"
[ "$TOTAL" -ge 20 ] \
    || fail "full params list has $TOTAL params (expected >=20)"

# Every param must have group (string), default (present), is_changed (bool).
MISSING_GROUP=$(jq -r '[.data.params[] | select(.group == null or .group == "")] | length' "$PARAMS_JSON")
[ "$MISSING_GROUP" = "0" ] \
    || fail "$MISSING_GROUP params missing 'group' field"

MISSING_DEFAULT=$(jq -r '[.data.params[] | select(.default == null)] | length' "$PARAMS_JSON")
# All 20 mock params are in PARAM_DEFAULTS, so default should never be null.
[ "$MISSING_DEFAULT" = "0" ] \
    || fail "$MISSING_DEFAULT params missing 'default' field (all 20 mock params should be in PARAM_DEFAULTS)"

MISSING_CHANGED=$(jq -r '[.data.params[] | select(.is_changed == null)] | length' "$PARAMS_JSON")
[ "$MISSING_CHANGED" = "0" ] \
    || fail "$MISSING_CHANGED params missing 'is_changed' field"

# is_changed must be false for every param (values match PX4 defaults).
CHANGED_COUNT=$(jq -r '[.data.params[] | select(.is_changed == true)] | length' "$PARAMS_JSON")
[ "$CHANGED_COUNT" = "0" ] \
    || fail "$CHANGED_COUNT params have is_changed=true (expected 0 right after download — values match defaults)"
echo "[G-8]   PASS: $TOTAL params, all have group/default/is_changed, all is_changed=false"

# =============================================================================
# Test 4: Write a param + verify is_changed
#   POST /api/vehicles/0/params with {"id": "MPC_XY_VEL_MAX", "value": 8.0, "param_type": 9}
#   Wait 1s for the PARAM_VALUE echo
#   GET /api/vehicles/0/params?search=MPC_XY_VEL_MAX
#   Assert: value=8.0, default=12.0, is_changed=true
# =============================================================================
echo "[G-8] Test 4: write MPC_XY_VEL_MAX=8.0 + verify is_changed"
HTTP_FILE="$WORK/write_http"
curl -s --max-time 5 -o "$WRITE_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/vehicles/0/params" \
    -H "Content-Type: application/json" \
    --data '{"id":"MPC_XY_VEL_MAX","value":8.0,"param_type":9}' >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-8]   POST /params MPC_XY_VEL_MAX=8.0 → HTTP $HTTP_CODE; body: $(cat "$WRITE_JSON")"
[ "$HTTP_CODE" = "200" ] \
    || fail "param write returned HTTP $HTTP_CODE (expected 200)"

# The write response must include ok=true and confirmed value.
WRITE_OK=$(jq -r '.ok // empty' "$WRITE_JSON")
WRITE_CONFIRMED=$(jq -r '.data.confirmed // empty' "$WRITE_JSON")
[ "$WRITE_OK" = "true" ] \
    || fail "param write response ok=$WRITE_OK (expected true)"
[ -n "$WRITE_CONFIRMED" ] \
    || fail "param write response missing confirmed value (PARAM_VALUE echo)"

# Wait for the link to ingest the PARAM_VALUE echo into its param store.
# The mock sends the echo in the same recv cycle as the PARAM_SET, but the
# link's ingest path goes through its async task — give it 1 s of slack.
sleep 1

# Re-fetch — value should be 8.0, default 12.0, is_changed true.
curl -s --max-time 3 "http://127.0.0.1:$PORT_API/api/vehicles/0/params?search=MPC_XY_VEL_MAX" >"$SEARCH_JSON"
SEARCH_COUNT=$(jq -r '.data.params | length' "$SEARCH_JSON")
[ "$SEARCH_COUNT" = "1" ] \
    || fail "search=MPC_XY_VEL_MAX returned $SEARCH_COUNT params (expected 1)"

VALUE=$(jq -r '.data.params[0].value' "$SEARCH_JSON")
DEFAULT=$(jq -r '.data.params[0].default' "$SEARCH_JSON")
CHANGED=$(jq -r '.data.params[0].is_changed' "$SEARCH_JSON")
echo "[G-8]   MPC_XY_VEL_MAX: value=$VALUE default=$DEFAULT is_changed=$CHANGED"
# Use awk for float compare (8.0 == 8).
[ "$(awk -v a="$VALUE" -v b=8 'BEGIN{print (a==b)?1:0}')" = "1" ] \
    || fail "MPC_XY_VEL_MAX value=$VALUE (expected 8.0)"
[ "$(awk -v a="$DEFAULT" -v b=12 'BEGIN{print (a==b)?1:0}')" = "1" ] \
    || fail "MPC_XY_VEL_MAX default=$DEFAULT (expected 12.0)"
[ "$CHANGED" = "true" ] \
    || fail "MPC_XY_VEL_MAX is_changed=$CHANGED (expected true — value differs from default)"
echo "[G-8]   PASS: MPC_XY_VEL_MAX=8.0, default=12.0, is_changed=true"

# =============================================================================
# Test 5: Preset save/load round-trip
#   POST /api/vehicles/0/param-presets on :8300 with 2 params
#   Assert: {ok: true, name: "test-preset", param_count: 2}
#   GET /api/vehicles/0/param-presets → list contains test-preset
#   POST /api/vehicles/0/param-presets/test-preset/load → 2 params, matching values
#   DELETE /api/vehicles/0/param-presets/test-preset
#   GET /api/vehicles/0/param-presets → empty list
# =============================================================================
echo "[G-8] Test 5: preset save/load round-trip"

HTTP_FILE="$WORK/preset_save_http"
curl -s --max-time 5 -o "$PRESET_SAVE_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_CATALOG/api/vehicles/0/param-presets" \
    -H "Content-Type: application/json" \
    --data '{"name":"test-preset","params":[{"id":"MPC_XY_VEL_MAX","value":8.0,"type":9},{"id":"MC_ROLLRATE_P","value":7.5,"type":9}]}' \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-8]   POST /param-presets → HTTP $HTTP_CODE; body: $(cat "$PRESET_SAVE_JSON")"
[ "$HTTP_CODE" = "200" ] \
    || fail "preset save returned HTTP $HTTP_CODE (expected 200)"

PRESET_OK=$(jq -r '.ok // empty' "$PRESET_SAVE_JSON")
PRESET_NAME=$(jq -r '.name // empty' "$PRESET_SAVE_JSON")
PRESET_COUNT=$(jq -r '.param_count // empty' "$PRESET_SAVE_JSON")
[ "$PRESET_OK" = "true" ] \
    || fail "preset save ok=$PRESET_OK (expected true)"
[ "$PRESET_NAME" = "test-preset" ] \
    || fail "preset save name='$PRESET_NAME' (expected 'test-preset')"
[ "$PRESET_COUNT" = "2" ] \
    || fail "preset save param_count=$PRESET_COUNT (expected 2)"
echo "[G-8]   PASS: saved 'test-preset' with 2 params"

# GET the list — must contain test-preset with param_count=2.
curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/vehicles/0/param-presets" >"$PRESET_LIST_JSON"
LIST_OK=$(jq -r '.ok // empty' "$PRESET_LIST_JSON")
LIST_LEN=$(jq -r '.data | length' "$PRESET_LIST_JSON")
LIST_HAS=$(jq -r '[.data[] | select(.name=="test-preset")] | length' "$PRESET_LIST_JSON")
LIST_COUNT=$(jq -r '[.data[] | select(.name=="test-preset") | .param_count] | .[0] // empty' "$PRESET_LIST_JSON")
echo "[G-8]   GET /param-presets → ok=$LIST_OK, $LIST_LEN presets, test-preset present=$LIST_HAS count=$LIST_COUNT"
[ "$LIST_OK" = "true" ] || fail "preset list ok=$LIST_OK (expected true)"
[ "$LIST_HAS" = "1" ] || fail "preset list missing 'test-preset'"
[ "$LIST_COUNT" = "2" ] || fail "preset list test-preset param_count=$LIST_COUNT (expected 2)"
echo "[G-8]   PASS: list contains 'test-preset' with param_count=2"

# Load the preset — must return the 2 saved params with matching values.
HTTP_FILE="$WORK/preset_load_http"
curl -s --max-time 5 -o "$PRESET_LOAD_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_CATALOG/api/vehicles/0/param-presets/test-preset/load" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-8]   POST /param-presets/test-preset/load → HTTP $HTTP_CODE"
[ "$HTTP_CODE" = "200" ] \
    || fail "preset load returned HTTP $HTTP_CODE (expected 200); body: $(cat "$PRESET_LOAD_JSON")"

LOAD_OK=$(jq -r '.ok // empty' "$PRESET_LOAD_JSON")
LOAD_NAME=$(jq -r '.name // empty' "$PRESET_LOAD_JSON")
LOAD_LEN=$(jq -r '.params | length' "$PRESET_LOAD_JSON")
LOAD_P1_ID=$(jq -r '.params[0].id' "$PRESET_LOAD_JSON")
LOAD_P1_VAL=$(jq -r '.params[0].value' "$PRESET_LOAD_JSON")
LOAD_P2_ID=$(jq -r '.params[1].id' "$PRESET_LOAD_JSON")
LOAD_P2_VAL=$(jq -r '.params[1].value' "$PRESET_LOAD_JSON")
echo "[G-8]   loaded: name=$LOAD_NAME params=$LOAD_LEN"
echo "[G-8]     [$LOAD_P1_ID=$LOAD_P1_VAL, $LOAD_P2_ID=$LOAD_P2_VAL]"
[ "$LOAD_OK" = "true" ] || fail "preset load ok=$LOAD_OK (expected true)"
[ "$LOAD_NAME" = "test-preset" ] || fail "preset load name='$LOAD_NAME' (expected 'test-preset')"
[ "$LOAD_LEN" = "2" ] || fail "preset load returned $LOAD_LEN params (expected 2)"
[ "$LOAD_P1_ID" = "MPC_XY_VEL_MAX" ] || fail "preset load param[0].id='$LOAD_P1_ID' (expected MPC_XY_VEL_MAX)"
[ "$(awk -v a="$LOAD_P1_VAL" -v b=8 'BEGIN{print (a==b)?1:0}')" = "1" ] \
    || fail "preset load param[0].value=$LOAD_P1_VAL (expected 8.0)"
[ "$LOAD_P2_ID" = "MC_ROLLRATE_P" ] || fail "preset load param[1].id='$LOAD_P2_ID' (expected MC_ROLLRATE_P)"
[ "$(awk -v a="$LOAD_P2_VAL" -v b=7.5 'BEGIN{print (a==b)?1:0}')" = "1" ] \
    || fail "preset load param[1].value=$LOAD_P2_VAL (expected 7.5)"
echo "[G-8]   PASS: load returned 2 params with matching values (round-trip preserved)"

# Delete the preset — must return ok.
HTTP_FILE="$WORK/preset_del_http"
curl -s --max-time 5 -o "$PRESET_DEL_JSON" -w "%{http_code}" \
    -X DELETE "http://127.0.0.1:$PORT_CATALOG/api/vehicles/0/param-presets/test-preset" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-8]   DELETE /param-presets/test-preset → HTTP $HTTP_CODE; body: $(cat "$PRESET_DEL_JSON")"
[ "$HTTP_CODE" = "200" ] \
    || fail "preset delete returned HTTP $HTTP_CODE (expected 200)"
DEL_OK=$(jq -r '.ok // empty' "$PRESET_DEL_JSON")
DEL_NAME=$(jq -r '.deleted // empty' "$PRESET_DEL_JSON")
[ "$DEL_OK" = "true" ] || fail "preset delete ok=$DEL_OK (expected true)"
[ "$DEL_NAME" = "test-preset" ] || fail "preset delete deleted='$DEL_NAME' (expected 'test-preset')"

# GET the list again — must be empty.
curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/vehicles/0/param-presets" >"$PRESET_LIST2_JSON"
LIST2_LEN=$(jq -r '.data | length' "$PRESET_LIST2_JSON")
echo "[G-8]   GET /param-presets (post-delete) → $LIST2_LEN presets"
[ "$LIST2_LEN" = "0" ] \
    || fail "preset list post-delete has $LIST2_LEN presets (expected 0)"
echo "[G-8]   PASS: preset list is empty after delete"

# -----------------------------------------------------------------------------
echo "[G-8] PASS"
exit 0
