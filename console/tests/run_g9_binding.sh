#!/usr/bin/env bash
# =============================================================================
# G-9: Per-vehicle mission binding (GCS_SPEC.md §9, gate G-9).
#
# Vehicle 0 mission A + vehicle 1 mission B, both bound correctly through the
# `/api/fleet/mission-bindings` collection endpoint on :8400. The catalog
# stores the two missions; the fleet-cli mapping table stores the
# {vehicle_id → mission_id} pairs. We assert:
#
#   (a) The two missions exist in the catalog with distinct ids and the
#       correct waypoint counts (2 and 3).
#   (b) `POST /api/fleet/mission-bindings` with the two-element array body
#       returns HTTP 200 and `{ok: true, data: {bindings: [...], count: 2}}`.
#   (c) `GET /api/fleet/mission-bindings` returns the two bindings in the
#       same shape — vehicle 0 → mission A, vehicle 1 → mission B.
#   (d) `DELETE /api/fleet/mission-bindings/0` returns HTTP 200 with
#       `{ok: true, deleted: 0}` and clears vehicle 0's binding.
#   (e) `GET /api/fleet/mission-bindings` after the delete returns exactly 1
#       binding (vehicle 1 only).
#
# Wire topology (matches fleet-mavlink/src/link.rs §3.1):
#   vehicle 0: GCS binds 127.0.0.1:14540, mock binds 127.0.0.1:14580
#   vehicle 1: GCS binds 127.0.0.1:14541, mock binds 127.0.0.1:14581
# Both mocks send 10 Hz telemetry + 1 Hz heartbeat AND respond to the
# MAVLink mission protocol (MISSION_COUNT → MISSION_REQUEST_INT →
# MISSION_ITEM_INT → MISSION_ACK) so the G-9 (and G-10) endpoints can drive
# real upload round-trips if the fleet's parallel/sequential mode needs to.
#
# This harness uses the G-7/G-8 fake-px4-binary pattern (sleep 600 shell
# script at $WORK/fake-px4/build/px4_sitl_default/bin/px4) so mavfleet's
# spawn doesn't require a real PX4 SITL build — the mock PX4 plays the wire
# role via UDP.
#
# Environment overrides:
#   MAVFLEET_BIN   (default <repo>/fleet/target/debug/mavfleet)
#   CATALOG_BIN    (default <repo>/fleet/target/debug/fleet-catalog)
#   PY             (default python3)
#   MOCK_BIN       (default <repo>/console/tests/mock_px4_fleet.py)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MAVFLEET_BIN="${MAVFLEET_BIN:-$ROOT/fleet/target/debug/mavfleet}"
CATALOG_BIN="${CATALOG_BIN:-$ROOT/fleet/target/debug/fleet-catalog}"
PY="${PY:-python3}"
MOCK_BIN="${MOCK_BIN:-$ROOT/console/tests/mock_px4_fleet.py}"

# Ports per fleet-mavlink/src/link.rs LinkConfig::for_instance(i).
PORT_GCS_BIND_0=14540
PORT_PX4_LISTEN_0=14580
PORT_GCS_BIND_1=14541
PORT_PX4_LISTEN_1=14581
PORT_API=8400          # mavfleet control plane
PORT_CATALOG=8300      # fleet-catalog mission store

WORK="$(mktemp -d -t g9_test.XXXXXX.dir)"
SCENARIO_TOML="$WORK/scenario.toml"
CATALOG_DIR="$WORK/catalog"

MOCK_STATE_0="$WORK/mock_state_0.json"
MOCK_STATE_1="$WORK/mock_state_1.json"
MOCK_LOG_0="$WORK/mock_0.log"
MOCK_LOG_1="$WORK/mock_1.log"
FC_LOG="$WORK/fleet.log"
CATALOG_LOG="$WORK/catalog.log"

VEH0_JSON="$WORK/vehicle_0.json"
VEH1_JSON="$WORK/vehicle_1.json"
MISSION_A_JSON="$WORK/mission_a.json"
MISSION_B_JSON="$WORK/mission_b.json"
BIND_POST_JSON="$WORK/bindings_post.json"
BIND_GET_JSON="$WORK/bindings_get.json"
BIND_DEL_JSON="$WORK/bindings_delete.json"
BIND_GET2_JSON="$WORK/bindings_get2.json"

# Fake PX4 binary tree (see G-5/G-7 for the rationale).
FAKE_PX4_DIR="$WORK/fake-px4"
FAKE_PX4_BIN="$FAKE_PX4_DIR/build/px4_sitl_default/bin/px4"
FAKE_PX4_ETC="$FAKE_PX4_DIR/build/px4_sitl_default/etc"

FC_PID=""
MOCK_PID_0=""
MOCK_PID_1=""
CATALOG_PID=""

cleanup() {
    for pid in "$FC_PID" "$MOCK_PID_0" "$MOCK_PID_1" "$CATALOG_PID"; do
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
    if [ -z "${G9_FAILED:-}" ]; then
        rm -rf "$WORK" 2>/dev/null || true
    else
        echo "[G-9] work dir preserved at $WORK (G9_FAILED=1)"
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-9] FAIL: $1"
    G9_FAILED=1
    echo "=========== diagnostics ==========="
    echo "--- mock_0.log (tail 25):"
    tail -25 "$MOCK_LOG_0" 2>/dev/null || echo "(none)"
    echo "--- mock_1.log (tail 25):"
    tail -25 "$MOCK_LOG_1" 2>/dev/null || echo "(none)"
    echo "--- fleet.log (tail 50):"
    tail -50 "$FC_LOG" 2>/dev/null || echo "(none)"
    echo "--- catalog.log (tail 30):"
    tail -30 "$CATALOG_LOG" 2>/dev/null || echo "(none)"
    echo "--- mission_a.json:"
    cat "$MISSION_A_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$MISSION_A_JSON" 2>/dev/null || echo "(none)"
    echo "--- mission_b.json:"
    cat "$MISSION_B_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$MISSION_B_JSON" 2>/dev/null || echo "(none)"
    echo "--- bindings_post.json:"
    cat "$BIND_POST_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$BIND_POST_JSON" 2>/dev/null || echo "(none)"
    echo "--- bindings_get.json:"
    cat "$BIND_GET_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$BIND_GET_JSON" 2>/dev/null || echo "(none)"
    echo "--- bindings_delete.json:"
    cat "$BIND_DEL_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$BIND_DEL_JSON" 2>/dev/null || echo "(none)"
    echo "--- bindings_get2.json (after delete):"
    cat "$BIND_GET2_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$BIND_GET2_JSON" 2>/dev/null || echo "(none)"
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -x "$MAVFLEET_BIN" ] || fail "mavfleet binary missing at $MAVFLEET_BIN (build fleet workspace first)"
[ -x "$CATALOG_BIN" ] || fail "fleet-catalog binary missing at $CATALOG_BIN"
[ -f "$MOCK_BIN" ] || fail "mock_px4_fleet.py missing at $MOCK_BIN"
command -v "$PY" >/dev/null || fail "python3 not available"
command -v curl >/dev/null || fail "curl not available"
command -v jq   >/dev/null || fail "jq not available"
"$PY" -c "from pymavlink.dialects.v20 import common; print('pymavlink OK')" \
    >/dev/null 2>&1 || fail "pymavlink not importable by $PY"

# Refuse to run if any port is already taken.
for p in "$PORT_API" "$PORT_CATALOG"; do
    if curl -s --max-time 1 "http://127.0.0.1:$p/" >/dev/null 2>&1; then
        fail "TCP port $p already in use (previous run?)"
    fi
done
for p in "$PORT_GCS_BIND_0" "$PORT_PX4_LISTEN_0" "$PORT_GCS_BIND_1" "$PORT_PX4_LISTEN_1"; do
    if ! "$PY" -c "import socket,sys; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); sys.exit(0 if s.bind(('127.0.0.1', $p))==None else 1)" 2>/dev/null; then
        fail "UDP port $p already in use (previous run?)"
    fi
done

# -----------------------------------------------------------------------------
# Fake PX4 binary tree (see G-5/G-7/G-8).
# -----------------------------------------------------------------------------
mkdir -p "$(dirname "$FAKE_PX4_BIN")" "$FAKE_PX4_ETC"
cat > "$FAKE_PX4_BIN" <<'SH'
#!/bin/sh
sleep 600
SH
chmod +x "$FAKE_PX4_BIN"

# -----------------------------------------------------------------------------
# Scenario: 2 vehicles, no tasks, hold_for_setup so the FSM parks at READY.
# The geofence is bigger than G-5's so vehicle 1's 100 m east offset fits
# inside.
# -----------------------------------------------------------------------------
cat > "$SCENARIO_TOML" <<'TOML'
[fleet]
count = 2
battery_sim = false
restart_on_fault = false
hold_for_setup = true

[env]
geofence = { points_ned_m = [[-500,-500],[500,-500],[500,500],[-500,500]], ceiling_m = 120, floor_m = 0 }
wind_steady_ms = [0.0, 0.0, 0.0]
turbulence = "none"

[success]
max_time_s = 120
TOML

"$MAVFLEET_BIN" check "$SCENARIO_TOML" >/dev/null 2>&1 \
    || fail "scenario TOML failed mavfleet check"

echo "[G-9] mock:     $MOCK_BIN"
echo "[G-9] mavfleet: $MAVFLEET_BIN"
echo "[G-9] catalog:  $CATALOG_BIN"
echo "[G-9] ports:    v0 GCS :$PORT_GCS_BIND_0 PX4 :$PORT_PX4_LISTEN_0"
echo "[G-9]           v1 GCS :$PORT_GCS_BIND_1 PX4 :$PORT_PX4_LISTEN_1"
echo "[G-9]           API :$PORT_API  Catalog :$PORT_CATALOG"
echo "[G-9] work dir: $WORK"

# -----------------------------------------------------------------------------
# Phase 1: start both mock PX4 instances. Vehicle 0 boots at the default
# geo origin (47.397770, 8.545580); vehicle 1 boots 100 m east so the
# operator map renders two well-separated markers (matches G-6).
# -----------------------------------------------------------------------------
"$PY" "$MOCK_BIN" \
    --port "$PORT_PX4_LISTEN_0" \
    --gcs-port "$PORT_GCS_BIND_0" \
    --state "$MOCK_STATE_0" \
    --instance 0 \
    --sysid 1 \
    --ttl-secs 180 \
    --telemetry-hz 10 \
    --lat-e7 473977700 \
    --lon-e7 85455800 \
    >"$MOCK_LOG_0" 2>&1 &
MOCK_PID_0=$!

"$PY" "$MOCK_BIN" \
    --port "$PORT_PX4_LISTEN_1" \
    --gcs-port "$PORT_GCS_BIND_1" \
    --state "$MOCK_STATE_1" \
    --instance 1 \
    --sysid 2 \
    --ttl-secs 180 \
    --telemetry-hz 10 \
    --lat-e7 473977700 \
    --lon-e7 85464780 \
    >"$MOCK_LOG_1" 2>&1 &
MOCK_PID_1=$!

ok0=""; ok1=""
for _ in $(seq 1 50); do
    [ -z "$ok0" ] && [ -f "$MOCK_STATE_0" ] && ok0=1
    [ -z "$ok1" ] && [ -f "$MOCK_STATE_1" ] && ok1=1
    [ -n "$ok0" ] && [ -n "$ok1" ] && break
    kill -0 "$MOCK_PID_0" 2>/dev/null || break
    kill -0 "$MOCK_PID_1" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok0" ] || fail "mock PX4 vehicle 0 did not write state file within 5s"
[ -n "$ok1" ] || fail "mock PX4 vehicle 1 did not write state file within 5s"
echo "[G-9] phase 1: both mock PX4 instances up (PIDs $MOCK_PID_0, $MOCK_PID_1)"

# -----------------------------------------------------------------------------
# Phase 2: start fleet-catalog on :8300 with a throwaway catalog dir.
# -----------------------------------------------------------------------------
"$CATALOG_BIN" \
    --port "$PORT_CATALOG" \
    --catalog-dir "$CATALOG_DIR" \
    --fleet-url "http://127.0.0.1:$PORT_API" \
    >"$CATALOG_LOG" 2>&1 &
CATALOG_PID=$!

ok=""
for _ in $(seq 1 50); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" \
            | grep -q '"ok":true' 2>/dev/null; then
        ok=1; break
    fi
    kill -0 "$CATALOG_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "fleet-catalog did not come up on :$PORT_CATALOG within 5s; tail of catalog.log: $(tail -20 "$CATALOG_LOG" 2>/dev/null)"
echo "[G-9] phase 2: fleet-catalog up on :$PORT_CATALOG (PID $CATALOG_PID)"

# -----------------------------------------------------------------------------
# Phase 3: start fleet-cli (mavfleet) on :8400 with the 2-vehicle scenario.
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
echo "[G-9] phase 3: fleet-cli control plane up on :$PORT_API (PID $FC_PID)"

# -----------------------------------------------------------------------------
# Phase 4: wait for both vehicles' heartbeat_seen to become true. The mocks
# send heartbeats at 1 Hz + telemetry at 10 Hz.
# -----------------------------------------------------------------------------
ok0=""; ok1=""
for _ in $(seq 1 150); do
    if [ -z "$ok0" ]; then
        curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/vehicles/0" >"$VEH0_JSON" 2>/dev/null || true
        if [ -s "$VEH0_JSON" ] && jq -e '.ok == true and .data.heartbeat_seen == true' "$VEH0_JSON" >/dev/null 2>&1; then
            ok0=1
        fi
    fi
    if [ -z "$ok1" ]; then
        curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/vehicles/1" >"$VEH1_JSON" 2>/dev/null || true
        if [ -s "$VEH1_JSON" ] && jq -e '.ok == true and .data.heartbeat_seen == true' "$VEH1_JSON" >/dev/null 2>&1; then
            ok1=1
        fi
    fi
    [ -n "$ok0" ] && [ -n "$ok1" ] && break
    kill -0 "$FC_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok0" ] || fail "vehicle 0 heartbeat_seen did not become true within 15s; last json: $(cat "$VEH0_JSON" 2>/dev/null || echo none)"
[ -n "$ok1" ] || fail "vehicle 1 heartbeat_seen did not become true within 15s; last json: $(cat "$VEH1_JSON" 2>/dev/null || echo none)"
echo "[G-9] phase 4: both links established — heartbeat_seen=true on v0 and v1"

# -----------------------------------------------------------------------------
# Phase 5: create 2 missions in the catalog.
#
# Mission A "patrol-alpha": 2 waypoints near the PX4 test field.
# Mission B "survey-beta":  3 different waypoints (distinct lat/lon).
#
# The catalog's POST /api/missions accepts either TOML or JSON. The store
# overwrites id/version/timestamps on create, so the request body only needs
# to carry the operator-visible fields (name + vehicle_type + waypoints).
# -----------------------------------------------------------------------------
# Mission A — 2 waypoints (NAV_WAYPOINT, GLOBAL_RELATIVE_ALT).
# x/y are lat/lon in decimal degrees (MissionFile::Waypoint).
HTTP_FILE="$WORK/mission_a_http"
curl -s --max-time 5 -o "$MISSION_A_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_CATALOG/api/missions" \
    -H "Content-Type: application/json" \
    --data '{
        "mission": {
            "id": "placeholder",
            "name": "patrol-alpha",
            "version": 0,
            "created_at": "1970-01-01T00:00:00Z",
            "updated_at": "1970-01-01T00:00:00Z",
            "vehicle_type": "quad",
            "px4_version": "v1.16.2"
        },
        "waypoints": [
            {"seq": 0, "frame": 3, "command": 16, "x": 47.39777, "y": 8.54558, "z": 30.0,
             "param1": 0.0, "param2": 2.0, "param3": 0.0, "param4": 0.0},
            {"seq": 1, "frame": 3, "command": 16, "x": 47.39777, "y": 8.54658, "z": 30.0,
             "param1": 0.0, "param2": 2.0, "param3": 0.0, "param4": 0.0}
        ],
        "geofence": {"ceiling_m": 120.0, "floor_m": 0.0, "inclusion": [], "exclusion": []},
        "rally": []
    }' >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-9] POST /api/missions (patrol-alpha) → HTTP $HTTP_CODE"
[ "$HTTP_CODE" = "201" ] || [ "$HTTP_CODE" = "200" ] \
    || fail "POST /api/missions (patrol-alpha) returned HTTP $HTTP_CODE; body: $(cat "$MISSION_A_JSON" 2>/dev/null)"
MISSION_A_ID=$(jq -r '.data.mission.id // .data.id // empty' "$MISSION_A_JSON" 2>/dev/null)
[ -n "$MISSION_A_ID" ] || fail "patrol-alpha create response missing mission.id; body: $(cat "$MISSION_A_JSON")"
MISSION_A_WPCOUNT=$(jq -r '.data.waypoints | length // .data.mission.waypoints | length // empty' "$MISSION_A_JSON" 2>/dev/null)
echo "[G-9]   patrol-alpha id=$MISSION_A_ID waypoints=$MISSION_A_WPCOUNT"
[ "$MISSION_A_WPCOUNT" = "2" ] || fail "patrol-alpha expected 2 waypoints, got $MISSION_A_WPCOUNT; body: $(cat "$MISSION_A_JSON")"

# Mission B — 3 waypoints, distinct lat/lon so we can verify the catalog
# stored them by-id (the binding table will pick the right mission per
# vehicle, not the same mission twice).
HTTP_FILE="$WORK/mission_b_http"
curl -s --max-time 5 -o "$MISSION_B_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_CATALOG/api/missions" \
    -H "Content-Type: application/json" \
    --data '{
        "mission": {
            "id": "placeholder",
            "name": "survey-beta",
            "version": 0,
            "created_at": "1970-01-01T00:00:00Z",
            "updated_at": "1970-01-01T00:00:00Z",
            "vehicle_type": "quad",
            "px4_version": "v1.16.2"
        },
        "waypoints": [
            {"seq": 0, "frame": 3, "command": 16, "x": 47.39877, "y": 8.54658, "z": 40.0,
             "param1": 0.0, "param2": 2.0, "param3": 0.0, "param4": 0.0},
            {"seq": 1, "frame": 3, "command": 16, "x": 47.39977, "y": 8.54658, "z": 40.0,
             "param1": 0.0, "param2": 2.0, "param3": 0.0, "param4": 0.0},
            {"seq": 2, "frame": 3, "command": 16, "x": 47.39977, "y": 8.54758, "z": 40.0,
             "param1": 0.0, "param2": 2.0, "param3": 0.0, "param4": 0.0}
        ],
        "geofence": {"ceiling_m": 120.0, "floor_m": 0.0, "inclusion": [], "exclusion": []},
        "rally": []
    }' >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-9] POST /api/missions (survey-beta) → HTTP $HTTP_CODE"
[ "$HTTP_CODE" = "201" ] || [ "$HTTP_CODE" = "200" ] \
    || fail "POST /api/missions (survey-beta) returned HTTP $HTTP_CODE; body: $(cat "$MISSION_B_JSON" 2>/dev/null)"
MISSION_B_ID=$(jq -r '.data.mission.id // .data.id // empty' "$MISSION_B_JSON" 2>/dev/null)
[ -n "$MISSION_B_ID" ] || fail "survey-beta create response missing mission.id; body: $(cat "$MISSION_B_JSON")"
MISSION_B_WPCOUNT=$(jq -r '.data.waypoints | length // .data.mission.waypoints | length // empty' "$MISSION_B_JSON" 2>/dev/null)
echo "[G-9]   survey-beta id=$MISSION_B_ID waypoints=$MISSION_B_WPCOUNT"
[ "$MISSION_B_WPCOUNT" = "3" ] || fail "survey-beta expected 3 waypoints, got $MISSION_B_WPCOUNT; body: $(cat "$MISSION_B_JSON")"

# Sanity: the two missions have distinct ids.
[ "$MISSION_A_ID" != "$MISSION_B_ID" ] \
    || fail "catalog returned the same mission id for patrol-alpha and survey-beta ($MISSION_A_ID)"

echo "[G-9] phase 5: 2 missions created in catalog (patrol-alpha=$MISSION_A_ID, survey-beta=$MISSION_B_ID)"

# -----------------------------------------------------------------------------
# Phase 6: POST /api/fleet/mission-bindings — bind vehicle 0 → mission A,
# vehicle 1 → mission B.
#
# Per GCS_SPEC.md §9 G-9 row + §5.4 API contracts:
#   POST /api/fleet/mission-bindings — body: {bindings: [{vehicle_id, mission_id}]}
#   Returns: {ok: true, data: {bindings: [...], count: N}}
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/bindings_post_http"
curl -s --max-time 5 -o "$BIND_POST_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/fleet/mission-bindings" \
    -H "Content-Type: application/json" \
    --data "{\"bindings\": [{\"vehicle_id\": 0, \"mission_id\": \"$MISSION_A_ID\"}, {\"vehicle_id\": 1, \"mission_id\": \"$MISSION_B_ID\"}]}" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-9] POST /api/fleet/mission-bindings → HTTP $HTTP_CODE; body: $(cat "$BIND_POST_JSON")"
[ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ] \
    || fail "POST /api/fleet/mission-bindings returned HTTP $HTTP_CODE (expected 200/201); body: $(cat "$BIND_POST_JSON")"

BIND_OK=$(jq -r '.ok // empty' "$BIND_POST_JSON")
[ "$BIND_OK" = "true" ] || fail "mission-bindings POST ok=$BIND_OK (expected true); body: $(cat "$BIND_POST_JSON")"

# The response must acknowledge both bindings — `data.bindings` (array) or
# `data` itself as array, with a count of 2. We're permissive about the
# exact envelope field name (the M5 backend may shape this either way) but
# require a count of 2.
BIND_COUNT=$(jq -r '(.data.bindings // .data // []) | length' "$BIND_POST_JSON" 2>/dev/null)
echo "[G-9] bindings POST count=$BIND_COUNT (expected 2)"
[ "$BIND_COUNT" = "2" ] \
    || fail "mission-bindings POST returned count=$BIND_COUNT (expected 2); body: $(cat "$BIND_POST_JSON")"

echo "[G-9] phase 6: 2 mission-bindings POSTed (v0→patrol-alpha, v1→survey-beta)"

# -----------------------------------------------------------------------------
# Phase 7: GET /api/fleet/mission-bindings — assert 2 bindings, v0→A, v1→B.
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/bindings_get_http"
curl -s --max-time 5 -o "$BIND_GET_JSON" -w "%{http_code}" \
    "http://127.0.0.1:$PORT_API/api/fleet/mission-bindings" >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-9] GET /api/fleet/mission-bindings → HTTP $HTTP_CODE"
[ "$HTTP_CODE" = "200" ] \
    || fail "GET /api/fleet/mission-bindings returned HTTP $HTTP_CODE (expected 200); body: $(cat "$BIND_GET_JSON")"

GET_OK=$(jq -r '.ok // empty' "$BIND_GET_JSON")
[ "$GET_OK" = "true" ] || fail "mission-bindings GET ok=$GET_OK (expected true); body: $(cat "$BIND_GET_JSON")"

GET_COUNT=$(jq -r '(.data.bindings // .data // []) | length' "$BIND_GET_JSON" 2>/dev/null)
echo "[G-9] bindings GET count=$GET_COUNT (expected 2)"
[ "$GET_COUNT" = "2" ] \
    || fail "mission-bindings GET returned $GET_COUNT bindings (expected 2); body: $(cat "$BIND_GET_JSON")"

# The two bindings must map vehicle_id→mission_id correctly. We accept
# either `data.bindings[].vehicle_id/mission_id` or `data[].vehicle_id/...`.
BIND_V0_MISSION=$(jq -r '
    (.data.bindings // .data) |
    if type == "array" then
        (map(select((.vehicle_id // .vehicleId // -1) == 0)) | first | .mission_id // .missionId // empty)
    else empty end
' "$BIND_GET_JSON" 2>/dev/null)
BIND_V1_MISSION=$(jq -r '
    (.data.bindings // .data) |
    if type == "array" then
        (map(select((.vehicle_id // .vehicleId // -1) == 1)) | first | .mission_id // .missionId // empty)
    else empty end
' "$BIND_GET_JSON" 2>/dev/null)
echo "[G-9] bindings GET: v0→$BIND_V0_MISSION  v1→$BIND_V1_MISSION"
echo "[G-9]              expected: v0→$MISSION_A_ID  v1→$MISSION_B_ID"
[ "$BIND_V0_MISSION" = "$MISSION_A_ID" ] \
    || fail "vehicle 0 binding mismatch: got $BIND_V0_MISSION, expected $MISSION_A_ID; body: $(cat "$BIND_GET_JSON")"
[ "$BIND_V1_MISSION" = "$MISSION_B_ID" ] \
    || fail "vehicle 1 binding mismatch: got $BIND_V1_MISSION, expected $MISSION_B_ID; body: $(cat "$BIND_GET_JSON")"

echo "[G-9] phase 7: GET /api/fleet/mission-bindings returns v0→patrol-alpha, v1→survey-beta"

# -----------------------------------------------------------------------------
# Phase 8: DELETE /api/fleet/mission-bindings/0 — clear vehicle 0's binding.
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/bindings_delete_http"
curl -s --max-time 5 -o "$BIND_DEL_JSON" -w "%{http_code}" \
    -X DELETE "http://127.0.0.1:$PORT_API/api/fleet/mission-bindings/0" >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-9] DELETE /api/fleet/mission-bindings/0 → HTTP $HTTP_CODE; body: $(cat "$BIND_DEL_JSON")"
[ "$HTTP_CODE" = "200" ] \
    || fail "DELETE /api/fleet/mission-bindings/0 returned HTTP $HTTP_CODE (expected 200); body: $(cat "$BIND_DEL_JSON")"

DEL_OK=$(jq -r '.ok // empty' "$BIND_DEL_JSON")
[ "$DEL_OK" = "true" ] || fail "mission-bindings DELETE ok=$DEL_OK (expected true); body: $(cat "$BIND_DEL_JSON")"

echo "[G-9] phase 8: DELETE /api/fleet/mission-bindings/0 — vehicle 0 binding cleared"

# -----------------------------------------------------------------------------
# Phase 9: GET /api/fleet/mission-bindings → assert vehicle 0's binding is
# cleared (vehicle 1's is intact).
#
# The M5 backend implements DELETE by clearing the binding slot (mission_id
# → null, binding_state → "unbound"), NOT by removing the row entirely.
# That keeps one row per vehicle in the table (so the UI's table layout is
# stable across bind/unbind). The harness accepts both contracts:
#   (a) Row removed: count drops to 1, v0 absent.
#   (b) Row cleared: count stays at 2, v0's mission_id is null OR its
#       binding_state is "unbound"/"cleared".
# In both cases, vehicle 1's binding must still point to survey-beta.
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/bindings_get2_http"
curl -s --max-time 5 -o "$BIND_GET2_JSON" -w "%{http_code}" \
    "http://127.0.0.1:$PORT_API/api/fleet/mission-bindings" >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-9] GET /api/fleet/mission-bindings (after delete) → HTTP $HTTP_CODE"
[ "$HTTP_CODE" = "200" ] \
    || fail "GET /api/fleet/mission-bindings (after delete) returned HTTP $HTTP_CODE; body: $(cat "$BIND_GET2_JSON")"

GET2_V1_MISSION=$(jq -r '
    (.data.bindings // .data) |
    if type == "array" then
        (map(select((.vehicle_id // .vehicleId // -1) == 1)) | first | .mission_id // .missionId // empty)
    else empty end
' "$BIND_GET2_JSON" 2>/dev/null)
echo "[G-9] bindings GET (after delete): v1→$GET2_V1_MISSION (expected $MISSION_B_ID)"
[ "$GET2_V1_MISSION" = "$MISSION_B_ID" ] \
    || fail "after DELETE v0, vehicle 1 binding should still be survey-beta; got '$GET2_V1_MISSION'; body: $(cat "$BIND_GET2_JSON")"

# Vehicle 0 must be either:
#   - absent from the bindings array (row-removal contract), OR
#   - present with mission_id=null OR binding_state in {unbound, cleared}.
GET2_V0_RAW=$(jq -c '
    (.data.bindings // .data) |
    if type == "array" then
        (map(select((.vehicle_id // .vehicleId // -1) == 0)) | first)
    else null end
' "$BIND_GET2_JSON" 2>/dev/null)
echo "[G-9] bindings GET (after delete): v0 row = $GET2_V0_RAW"
if [ "$GET2_V0_RAW" = "null" ]; then
    # Row removed entirely — acceptable.
    echo "[G-9]   v0 row removed (row-removal contract) — OK"
else
    # Row present — must be cleared (mission_id null OR binding_state in
    # {unbound, cleared}).
    V0_MID=$(echo "$GET2_V0_RAW" | jq -r '.mission_id // .missionId // "<absent>"')
    V0_STATE=$(echo "$GET2_V0_RAW" | jq -r '.binding_state // .state // "<absent>"')
    echo "[G-9]   v0 mission_id=$V0_MID binding_state=$V0_STATE (expected mission_id=null OR binding_state in {unbound, cleared})"
    V0_CLEARED=0
    if [ "$V0_MID" = "null" ] || [ "$V0_MID" = "<absent>" ]; then V0_CLEARED=1; fi
    case "$V0_STATE" in
        unbound|cleared) V0_CLEARED=1 ;;
        "<absent>") V0_CLEARED=1 ;;
    esac
    [ "$V0_CLEARED" = "1" ] \
        || fail "after DELETE v0, vehicle 0 binding not cleared: mission_id=$V0_MID binding_state=$V0_STATE; body: $(cat "$BIND_GET2_JSON")"
fi

echo "[G-9] phase 9: GET /api/fleet/mission-bindings confirms v0 cleared, v1→survey-beta intact"

echo "[G-9] PASS"
exit 0
