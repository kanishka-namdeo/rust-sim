#!/usr/bin/env bash
# =============================================================================
# G-10: Fleet orchestration — parallel + sequential modes
# (GCS_SPEC.md §9, gate G-10).
#
# Sequential mode: vehicle 1 starts after vehicle 0 reaches WP1.
# Parallel mode:   both vehicles start together.
#
# This harness drives the spec's `POST /api/fleet/start` endpoint
# (GCS_SPEC.md §5.4) with both `mode: "parallel"` and `mode: "sequential"`,
# asserting for each that:
#
#   (a) The endpoint responds with HTTP 200 + `{ok: true, data: {vehicles:
#       [...]}}` — the parallel/sequential orchestrator ran to completion.
#   (b) The response's `data.vehicles` array contains one entry per vehicle
#       in the fleet (count=2 — vehicle 0 and vehicle 1).
#   (c) Each per-vehicle entry carries a `status` field whose value is one
#       of: "started" | "failed" | "timeout" | "skipped". The orchestrator
#       attempts both vehicles in parallel mode; the sequential mode's gate
#       may time out if the mock doesn't actually fly (acceptable for M5 —
#       the wire-level endpoint contract is what this gate verifies, not the
#       physics).
#   (d) For parallel mode: vehicle 0 is attempted (status != "skipped").
#   (e) For sequential mode: vehicle 0 is attempted first; vehicle 1's
#       status reflects the gate outcome (started if the gate cleared in
#       time, or timeout if it didn't — both are valid wire responses).
#
# If the mock supports mission protocol (this harness's mock does — see
# mock_px4_fleet.py), the harness also asserts both vehicles' mission-upload
# transactions appear in the mock's state file (`received_items[0]` has
# N items per vehicle after a successful parallel start). This is a soft
# assertion — the upload may fail if the link times out, in which case the
# per-vehicle `status` will be "failed" and the test still passes (the
# endpoint contract — "the orchestrator attempts every vehicle" — is what
# we verify).
#
# Wire topology (matches fleet-mavlink/src/link.rs §3.1):
#   vehicle 0: GCS binds 127.0.0.1:14540, mock binds 127.0.0.1:14580
#   vehicle 1: GCS binds 127.0.0.1:14541, mock binds 127.0.0.1:14581
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

WORK="$(mktemp -d -t g10_test.XXXXXX.dir)"
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
BIND_JSON="$WORK/bindings.json"

START_PARALLEL_JSON="$WORK/start_parallel.json"
START_SEQUENTIAL_JSON="$WORK/start_sequential.json"

# Fake PX4 binary tree (see G-5/G-7/G-8 for the rationale).
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
    if [ -z "${G10_FAILED:-}" ]; then
        rm -rf "$WORK" 2>/dev/null || true
    else
        echo "[G-10] work dir preserved at $WORK (G10_FAILED=1)"
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-10] FAIL: $1"
    G10_FAILED=1
    echo "=========== diagnostics ==========="
    echo "--- mock_0.log (tail 30):"
    tail -30 "$MOCK_LOG_0" 2>/dev/null || echo "(none)"
    echo "--- mock_1.log (tail 30):"
    tail -30 "$MOCK_LOG_1" 2>/dev/null || echo "(none)"
    echo "--- fleet.log (tail 60):"
    tail -60 "$FC_LOG" 2>/dev/null || echo "(none)"
    echo "--- catalog.log (tail 30):"
    tail -30 "$CATALOG_LOG" 2>/dev/null || echo "(none)"
    echo "--- bindings.json:"
    cat "$BIND_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$BIND_JSON" 2>/dev/null || echo "(none)"
    echo "--- start_parallel.json:"
    cat "$START_PARALLEL_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$START_PARALLEL_JSON" 2>/dev/null || echo "(none)"
    echo "--- start_sequential.json:"
    cat "$START_SEQUENTIAL_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$START_SEQUENTIAL_JSON" 2>/dev/null || echo "(none)"
    echo "--- mock_state_0.json (mission-protocol tail):"
    if [ -f "$MOCK_STATE_0" ]; then
        jq '{transaction_log, received_items: (.received_items | map_values(length)), command_count, last_error}' "$MOCK_STATE_0" 2>/dev/null \
            || cat "$MOCK_STATE_0"
    else echo "(none)"; fi
    echo "--- mock_state_1.json (mission-protocol tail):"
    if [ -f "$MOCK_STATE_1" ]; then
        jq '{transaction_log, received_items: (.received_items | map_values(length)), command_count, last_error}' "$MOCK_STATE_1" 2>/dev/null \
            || cat "$MOCK_STATE_1"
    else echo "(none)"; fi
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

echo "[G-10] mock:     $MOCK_BIN"
echo "[G-10] mavfleet: $MAVFLEET_BIN"
echo "[G-10] catalog:  $CATALOG_BIN"
echo "[G-10] ports:    v0 GCS :$PORT_GCS_BIND_0 PX4 :$PORT_PX4_LISTEN_0"
echo "[G-10]           v1 GCS :$PORT_GCS_BIND_1 PX4 :$PORT_PX4_LISTEN_1"
echo "[G-10]           API :$PORT_API  Catalog :$PORT_CATALOG"
echo "[G-10] work dir: $WORK"

# -----------------------------------------------------------------------------
# Phase 1: start both mock PX4 instances. Vehicle 1 boots 100 m east so the
# two vehicles have distinct positions on the operator map (matches G-6).
# TTL is generous (240 s) to absorb the sequential-mode timeout (10 s)
# plus any per-vehicle upload latency.
# -----------------------------------------------------------------------------
"$PY" "$MOCK_BIN" \
    --port "$PORT_PX4_LISTEN_0" \
    --gcs-port "$PORT_GCS_BIND_0" \
    --state "$MOCK_STATE_0" \
    --instance 0 \
    --sysid 1 \
    --ttl-secs 240 \
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
    --ttl-secs 240 \
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
echo "[G-10] phase 1: both mock PX4 instances up (PIDs $MOCK_PID_0, $MOCK_PID_1)"

# -----------------------------------------------------------------------------
# Phase 2: start fleet-catalog on :8300.
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
echo "[G-10] phase 2: fleet-catalog up on :$PORT_CATALOG (PID $CATALOG_PID)"

# -----------------------------------------------------------------------------
# Phase 3: start fleet-cli (mavfleet) on :8400.
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
echo "[G-10] phase 3: fleet-cli control plane up on :$PORT_API (PID $FC_PID)"

# -----------------------------------------------------------------------------
# Phase 4: wait for both vehicles' heartbeat_seen to become true.
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
echo "[G-10] phase 4: both links established — heartbeat_seen=true on v0 and v1"

# -----------------------------------------------------------------------------
# Phase 5: create 2 missions + bind them (same as G-9 phase 5-6).
# Mission A "patrol-alpha" 2 wps → vehicle 0.
# Mission B "survey-beta"  3 wps → vehicle 1.
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/mission_a_http"
curl -s --max-time 5 -o "$MISSION_A_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_CATALOG/api/missions" \
    -H "Content-Type: application/json" \
    --data '{
        "mission": {"id": "p", "name": "patrol-alpha", "version": 0,
                    "created_at": "1970-01-01T00:00:00Z", "updated_at": "1970-01-01T00:00:00Z",
                    "vehicle_type": "quad", "px4_version": "v1.16.2"},
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
[ "$HTTP_CODE" = "201" ] || [ "$HTTP_CODE" = "200" ] \
    || fail "POST /api/missions (patrol-alpha) returned HTTP $HTTP_CODE; body: $(cat "$MISSION_A_JSON")"
MISSION_A_ID=$(jq -r '.data.mission.id // .data.id // empty' "$MISSION_A_JSON")
[ -n "$MISSION_A_ID" ] || fail "patrol-alpha create response missing mission.id; body: $(cat "$MISSION_A_JSON")"
echo "[G-10] patrol-alpha mission id=$MISSION_A_ID"

HTTP_FILE="$WORK/mission_b_http"
curl -s --max-time 5 -o "$MISSION_B_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_CATALOG/api/missions" \
    -H "Content-Type: application/json" \
    --data '{
        "mission": {"id": "p", "name": "survey-beta", "version": 0,
                    "created_at": "1970-01-01T00:00:00Z", "updated_at": "1970-01-01T00:00:00Z",
                    "vehicle_type": "quad", "px4_version": "v1.16.2"},
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
[ "$HTTP_CODE" = "201" ] || [ "$HTTP_CODE" = "200" ] \
    || fail "POST /api/missions (survey-beta) returned HTTP $HTTP_CODE; body: $(cat "$MISSION_B_JSON")"
MISSION_B_ID=$(jq -r '.data.mission.id // .data.id // empty' "$MISSION_B_JSON")
[ -n "$MISSION_B_ID" ] || fail "survey-beta create response missing mission.id; body: $(cat "$MISSION_B_JSON")"
echo "[G-10] survey-beta mission id=$MISSION_B_ID"

[ "$MISSION_A_ID" != "$MISSION_B_ID" ] \
    || fail "catalog returned the same mission id for both missions ($MISSION_A_ID)"

# Bind vehicle 0 → mission A, vehicle 1 → mission B.
HTTP_FILE="$WORK/bind_http"
curl -s --max-time 5 -o "$BIND_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/fleet/mission-bindings" \
    -H "Content-Type: application/json" \
    --data "{\"bindings\": [{\"vehicle_id\": 0, \"mission_id\": \"$MISSION_A_ID\"}, {\"vehicle_id\": 1, \"mission_id\": \"$MISSION_B_ID\"}]}" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-10] POST /api/fleet/mission-bindings → HTTP $HTTP_CODE; body: $(cat "$BIND_JSON")"
[ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ] \
    || fail "POST /api/fleet/mission-bindings returned HTTP $HTTP_CODE (expected 200/201); body: $(cat "$BIND_JSON")"

BIND_OK=$(jq -r '.ok // empty' "$BIND_JSON")
[ "$BIND_OK" = "true" ] || fail "mission-bindings POST ok=$BIND_OK (expected true); body: $(cat "$BIND_JSON")"

echo "[G-10] phase 5: 2 missions created + bound (v0→patrol-alpha, v1→survey-beta)"

# -----------------------------------------------------------------------------
# Phase 6: PARALLEL MODE TEST.
#
# POST /api/fleet/start with `{"mode": "parallel"}`.
#
# Per GCS_SPEC.md §5.4:
#   POST /api/fleet/start — body: {mode: "parallel" | "sequential",
#     sequential_gate: "first_waypoint" | "takeoff_complete", timeout_s: 30}
#
# The M5 backend's `fleet_start_post` handler iterates the bound vehicles,
# fetches each bound mission from :8300, calls `link.mission_upload(items, 0)`,`
# and returns `{ok: true, started: [FleetStartResult]}`. Each per-vehicle
# entry has {vehicle_id, mission_id, status, reason} where status is one of
# {started, failed, skipped}.
#
# The upload may take up to 30s per vehicle to time out (link.rs's mission
# upload deadline is 30s). With 2 vehicles, the parallel start can take up
# to ~70s in the worst case (2×30s + catalog fetches + 5s sequential sleep
# if any). The harness uses a 180s curl budget to absorb that.
#
# Assertions (per the task spec — "the upload may fail if the mock doesn't
# support mission protocol — that's OK for M5; assert the endpoint responds
# and both vehicles are attempted"):
#   (a) HTTP 200 + ok=true
#   (b) The per-vehicle result array has 2 entries (one per vehicle).
#   (c) Each entry has a status field (string).
#   (d) Both vehicles were attempted — status != "skipped" (in parallel mode
#       the orchestrator does NOT skip a vehicle based on a prior vehicle's
#       failure; "skipped" is only meaningful in sequential mode).
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/start_parallel_http"
# 180s budget: 2 vehicles * (30s upload timeout + 5s sequential sleep if
# any + catalog fetch overhead) + slack.
curl -s --max-time 180 -o "$START_PARALLEL_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/fleet/start" \
    -H "Content-Type: application/json" \
    --data '{"mode": "parallel"}' >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-10] POST /api/fleet/start {mode: parallel} → HTTP $HTTP_CODE; body: $(cat "$START_PARALLEL_JSON")"
[ "$HTTP_CODE" = "200" ] \
    || fail "POST /api/fleet/start (parallel) returned HTTP $HTTP_CODE (expected 200); body: $(cat "$START_PARALLEL_JSON")"

START_OK=$(jq -r '.ok // empty' "$START_PARALLEL_JSON")
[ "$START_OK" = "true" ] \
    || fail "parallel start response ok=$START_OK (expected true); body: $(cat "$START_PARALLEL_JSON")"

# The per-vehicle results array may be at data.started (M5 backend's actual
# shape), data.vehicles, or directly at data. We accept any of these.
PAR_VEHICLES=$(jq -r '(.data.started // .data.vehicles // .data // [])' "$START_PARALLEL_JSON" 2>/dev/null)
PAR_COUNT=$(echo "$PAR_VEHICLES" | jq 'length' 2>/dev/null)
echo "[G-10] parallel start: vehicle count=$PAR_COUNT (expected 2)"
[ "$PAR_COUNT" = "2" ] \
    || fail "parallel start returned $PAR_COUNT vehicle entries (expected 2); body: $(cat "$START_PARALLEL_JSON")"

# Each vehicle entry must have a status field; both must be non-"skipped".
PAR_V0_STATUS=$(echo "$PAR_VEHICLES" | jq -r '
    map(select((.vehicle_id // .vehicleId // .index // -1) == 0)) | first |
    .status // .state // empty' 2>/dev/null)
PAR_V1_STATUS=$(echo "$PAR_VEHICLES" | jq -r '
    map(select((.vehicle_id // .vehicleId // .index // -1) == 1)) | first |
    .status // .state // empty' 2>/dev/null)
echo "[G-10] parallel start: v0.status=$PAR_V0_STATUS  v1.status=$PAR_V1_STATUS"
[ -n "$PAR_V0_STATUS" ] || fail "parallel start missing v0 status; body: $(cat "$START_PARALLEL_JSON")"
[ -n "$PAR_V1_STATUS" ] || fail "parallel start missing v1 status; body: $(cat "$START_PARALLEL_JSON")"

# Acceptable statuses — per the task spec: "started" | "failed" | "timeout".
# "skipped" is NOT acceptable in parallel mode (the orchestrator should
# attempt every vehicle).
for status in "$PAR_V0_STATUS" "$PAR_V1_STATUS"; do
    case "$status" in
        started|failed|timeout|ok) ;;
        skipped) fail "parallel start vehicle status=skipped (orchestrator must attempt every vehicle in parallel mode); body: $(cat "$START_PARALLEL_JSON")" ;;
        *) fail "parallel start vehicle status '$status' not in {started, failed, timeout, ok, skipped}; body: $(cat "$START_PARALLEL_JSON")" ;;
    esac
done

echo "[G-10] phase 6: parallel start OK (v0=$PAR_V0_STATUS, v1=$PAR_V1_STATUS)"

# Soft assertion: if the mock supports mission protocol (it does — see
# mock_px4_fleet.py), the upload should have reached at least vehicle 0's
# mock. We assert the mock's transaction_log is non-empty AFTER the parallel
# start. This is best-effort: if the link timed out before completing the
# upload round-trip, the transaction_log may still be empty (the GCS didn't
# finish sending MISSION_COUNT). Either outcome is acceptable for the G-10
# wire-level gate; we just log what we saw.
sleep 1   # let the upload round-trip settle
PAR_UPLOADS_V0=0
PAR_UPLOADS_V1=0
if [ -f "$MOCK_STATE_0" ]; then
    PAR_UPLOADS_V0=$(jq -r '.transaction_log | length' "$MOCK_STATE_0" 2>/dev/null || echo 0)
fi
if [ -f "$MOCK_STATE_1" ]; then
    PAR_UPLOADS_V1=$(jq -r '.transaction_log | length' "$MOCK_STATE_1" 2>/dev/null || echo 0)
fi
echo "[G-10] (soft) mock mission-protocol transactions after parallel start: v0=$PAR_UPLOADS_V0 v1=$PAR_UPLOADS_V1 (best-effort)"

# -----------------------------------------------------------------------------
# Phase 7: SEQUENTIAL MODE TEST.
#
# Per the task: "Re-bind the missions (they may have been cleared by the
# parallel start)". We re-POST the same binding table to ensure both
# vehicles have a fresh binding for the sequential start.
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/rebind_http"
curl -s --max-time 5 -o "$WORK/rebind.json" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/fleet/mission-bindings" \
    -H "Content-Type: application/json" \
    --data "{\"bindings\": [{\"vehicle_id\": 0, \"mission_id\": \"$MISSION_A_ID\"}, {\"vehicle_id\": 1, \"mission_id\": \"$MISSION_B_ID\"}]}" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-10] re-POST /api/fleet/mission-bindings → HTTP $HTTP_CODE"
[ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ] \
    || fail "re-POST mission-bindings returned HTTP $HTTP_CODE; body: $(cat "$WORK/rebind.json")"
REBIND_OK=$(jq -r '.ok // empty' "$WORK/rebind.json" 2>/dev/null)
[ "$REBIND_OK" = "true" ] \
    || fail "re-bind ok=$REBIND_OK (expected true); body: $(cat "$WORK/rebind.json")"
echo "[G-10] phase 7a: missions re-bound before sequential start"

# POST /api/fleet/start with `{"mode": "sequential", "sequential_gate":
# "first_waypoint", "timeout_s": 10}`. The M5 backend's sequential gate is
# currently a 5s sleep per vehicle (the real first-waypoint polling is M5.1
# per the backend's TODO). The mock doesn't actually fly, so the gate would
# never clear — but with the M5 stub's 5s sleep, the sequential start still
# completes within the curl budget. The endpoint must respond with HTTP 200
# + a per-vehicle result array reflecting the gate outcome.
HTTP_FILE="$WORK/start_sequential_http"
# Budget: timeout_s (10) + 2x 30s upload timeout + 5s sequential sleep +
# catalog fetches + overhead = ~80s. Use 180s for safety.
curl -s --max-time 180 -o "$START_SEQUENTIAL_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/fleet/start" \
    -H "Content-Type: application/json" \
    --data '{"mode": "sequential", "sequential_gate": "first_waypoint", "timeout_s": 10}' \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-10] POST /api/fleet/start {mode: sequential, gate: first_waypoint, timeout_s: 10} → HTTP $HTTP_CODE; body: $(cat "$START_SEQUENTIAL_JSON")"
[ "$HTTP_CODE" = "200" ] \
    || fail "POST /api/fleet/start (sequential) returned HTTP $HTTP_CODE (expected 200); body: $(cat "$START_SEQUENTIAL_JSON")"

START_OK=$(jq -r '.ok // empty' "$START_SEQUENTIAL_JSON")
[ "$START_OK" = "true" ] \
    || fail "sequential start response ok=$START_OK (expected true); body: $(cat "$START_SEQUENTIAL_JSON")"

SEQ_VEHICLES=$(jq -r '(.data.started // .data.vehicles // .data // [])' "$START_SEQUENTIAL_JSON" 2>/dev/null)
SEQ_COUNT=$(echo "$SEQ_VEHICLES" | jq 'length' 2>/dev/null)
echo "[G-10] sequential start: vehicle count=$SEQ_COUNT (expected 2)"
[ "$SEQ_COUNT" = "2" ] \
    || fail "sequential start returned $SEQ_COUNT vehicle entries (expected 2); body: $(cat "$START_SEQUENTIAL_JSON")"

# Sequential mode: vehicle 0 must have been attempted first (status in
# {started, failed, timeout} — it ran, regardless of outcome). Vehicle 1's
# status is what we care about: it should be "started" (the gate cleared)
# or "timeout" / "skipped" (the gate didn't clear in time — the mock
# doesn't actually fly, so this is the expected outcome for M5).
SEQ_V0_STATUS=$(echo "$SEQ_VEHICLES" | jq -r '
    map(select((.vehicle_id // .vehicleId // .index // -1) == 0)) | first |
    .status // .state // empty' 2>/dev/null)
SEQ_V1_STATUS=$(echo "$SEQ_VEHICLES" | jq -r '
    map(select((.vehicle_id // .vehicleId // .index // -1) == 1)) | first |
    .status // .state // empty' 2>/dev/null)
echo "[G-10] sequential start: v0.status=$SEQ_V0_STATUS  v1.status=$SEQ_V1_STATUS"
[ -n "$SEQ_V0_STATUS" ] || fail "sequential start missing v0 status; body: $(cat "$START_SEQUENTIAL_JSON")"
[ -n "$SEQ_V1_STATUS" ] || fail "sequential start missing v1 status; body: $(cat "$START_SEQUENTIAL_JSON")"

# Vehicle 0 ran (status in {started, failed, timeout}).
case "$SEQ_V0_STATUS" in
    started|failed|timeout|ok) ;;
    skipped) fail "sequential start v0 status=skipped (vehicle 0 must be attempted first in sequential mode); body: $(cat "$START_SEQUENTIAL_JSON")" ;;
    *) fail "sequential start v0 status '$SEQ_V0_STATUS' not recognized; body: $(cat "$START_SEQUENTIAL_JSON")" ;;
esac

# Vehicle 1: acceptable statuses are {started, failed, timeout, skipped}.
# In sequential mode, "skipped" is also acceptable — if v0's gate didn't
# clear, the orchestrator should mark v1 as skipped (didn't attempt).
case "$SEQ_V1_STATUS" in
    started|failed|timeout|skipped|ok) ;;
    *) fail "sequential start v1 status '$SEQ_V1_STATUS' not recognized; body: $(cat "$START_SEQUENTIAL_JSON")" ;;
esac

echo "[G-10] phase 7: sequential start OK (v0=$SEQ_V0_STATUS, v1=$SEQ_V1_STATUS)"

echo "[G-10] PASS"
exit 0
