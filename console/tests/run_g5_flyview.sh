#!/usr/bin/env bash
# =============================================================================
# G-5: Fly View 1-vehicle telemetry (GCS_SPEC.md §9, gate G-5).
#
# Single-invocation harness: start a Python mock PX4 (10 Hz telemetry) +
# the real fleet-cli (mavfleet) on a 1-vehicle scenario, then poll the
# Fly-View HTTP API (`GET /api/vehicles/0`) for 10 s and assert that the
# telemetry snapshots are visibly moving (lat/lon/attitude change between
# polls), the `armed` field reads false (disarmed), and the `health` field
# is present (an empty list is a healthy vehicle).
#
# Wire topology (matches fleet-mavlink/src/link.rs §3.1):
#   fleet-cli mavlink link binds 127.0.0.1:14540, sends to 127.0.0.1:14580
#   mock PX4 binds 127.0.0.1:14580, sends telemetry to 127.0.0.1:14540
#
# Mock telemetry cadence (matches PX4's rcS default stream set):
#   HEARTBEAT    1 Hz (sysid 1 for vehicle 0, base_mode w/o SAFETY_ARMED)
#   ATTITUDE     10 Hz (slow roll oscillation, yaw follows the circle)
#   LOCAL_POSITION_NED    10 Hz (5 m radius circle, 0.1 rad/s)
#   GLOBAL_POSITION_INT   10 Hz (lat/lon drifts in a 5 m circle)
#   SYS_STATUS   10 Hz (80% battery, 12 V)
#   HOME_POSITION 1 Hz (so the FSM transitions BOOTING -> READY without the
#                        45 s home-fallback gate)
#
# The mock responds to COMMAND_LONG with COMMAND_ACK result=0 (no commands
# are issued in G-5; the arm gate is G-7's job).
#
# Asserts:
#   (a) The fleet-cli control plane comes up on :8400 within 10 s.
#   (b) The vehicle's heartbeat_seen is true (the link task ingested the
#       mock's first HEARTBEAT).
#   (c) The vehicle's lat_deg_e7 / lon_deg_e7 change between two snapshots
#       taken 3 s apart ("telemetry moves on map").
#   (d) The vehicle's attitude_q_wxyz (or roll/pitch/yaw derived from it)
#       changes between the same two snapshots ("attitude HUD moves").
#   (e) The vehicle's `armed` field is false (the mock boots disarmed).
#   (f) The vehicle's `health` field is present (a Vec<String>).
#
# Environment overrides:
#   MAVFLEET_BIN  (default <repo>/fleet/target/debug/mavfleet)
#   PY            (default python3)
#   MOCK_BIN      (default <repo>/console/tests/mock_px4_fly.py)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MAVFLEET_BIN="${MAVFLEET_BIN:-$ROOT/fleet/target/debug/mavfleet}"
PY="${PY:-python3}"
MOCK_BIN="${MOCK_BIN:-$ROOT/console/tests/mock_px4_fly.py}"

# Ports per fleet-mavlink/src/link.rs LinkConfig::for_instance(0).
PORT_GCS_BIND=14540    # Rust link's bind port (GCS receives heartbeats here)
PORT_PX4_LISTEN=14580  # PX4 onboard listen port (mock binds here)
PORT_API=8400           # fleet-cli control plane (REST/WS, spec §3.4)

WORK="$(mktemp -d -t g5_test.XXXXXX.dir)"
SCENARIO_TOML="$WORK/scenario.toml"
MOCK_STATE="$WORK/mock_state.json"
MOCK_LOG="$WORK/mock.log"
FC_LOG="$WORK/fleet.log"
SNAP_A="$WORK/snap_a.json"
SNAP_B="$WORK/snap_b.json"
VEH_JSON="$WORK/vehicle.json"

# Fake PX4 binary tree (sleep script — satisfies spawn_vehicle's Path::exists
# check; the mock PX4 plays the real PX4's wire role via UDP, so the spawned
# process just has to stay alive while the link task runs).
FAKE_PX4_DIR="$WORK/fake-px4"
FAKE_PX4_BIN="$FAKE_PX4_DIR/build/px4_sitl_default/bin/px4"
FAKE_PX4_ETC="$FAKE_PX4_DIR/build/px4_sitl_default/etc"

# PIDs to clean up.
FC_PID=""
MOCK_PID=""

cleanup() {
    for pid in "$FC_PID" "$MOCK_PID"; do
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
    # Preserve $WORK on failure for debugging — only remove on success.
    if [ -z "${G5_FAILED:-}" ]; then
        rm -rf "$WORK" 2>/dev/null || true
    else
        echo "[G-5] work dir preserved at $WORK (G5_FAILED=1)"
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-5] FAIL: $1"
    G5_FAILED=1
    echo "=========== diagnostics ==========="
    echo "--- mock.log (tail 30):"
    tail -30 "$MOCK_LOG" 2>/dev/null || echo "(none)"
    echo "--- fleet.log (tail 40):"
    tail -40 "$FC_LOG" 2>/dev/null || echo "(none)"
    echo "--- snap_a.json (head 50):"
    head -50 "$SNAP_A" 2>/dev/null || echo "(none)"
    echo "--- snap_b.json (head 50):"
    head -50 "$SNAP_B" 2>/dev/null || echo "(none)"
    echo "--- mock_state.json:"
    cat "$MOCK_STATE" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$MOCK_STATE" 2>/dev/null || echo "(none)"
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -x "$MAVFLEET_BIN" ] || fail "mavfleet binary missing at $MAVFLEET_BIN (build fleet workspace first)"
[ -f "$MOCK_BIN" ] || fail "mock_px4_fly.py missing at $MOCK_BIN"
command -v "$PY" >/dev/null || fail "python3 not available"
command -v curl >/dev/null || fail "curl not available"
command -v jq   >/dev/null || fail "jq not available"
"$PY" -c "from pymavlink.dialects.v20 import common; print('pymavlink OK')" \
    >/dev/null 2>&1 || fail "pymavlink not importable by $PY"

# Refuse to run if any port is already taken.
for p in "$PORT_GCS_BIND" "$PORT_PX4_LISTEN" "$PORT_API"; do
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
# Fake PX4 binary tree (the mock PX4 plays PX4's wire role via UDP; the
# spawned process only needs to stay alive — sleep 600 satisfies the
# spawn_vehicle Path::exists check and survives long enough for the run).
# -----------------------------------------------------------------------------
mkdir -p "$(dirname "$FAKE_PX4_BIN")" "$FAKE_PX4_ETC"
cat > "$FAKE_PX4_BIN" <<'SH'
#!/bin/sh
# Stand-in for the real PX4 binary. The mock PX4 (mock_px4_fly.py) plays
# PX4's wire role via UDP :14580; this script just has to stay alive while
# fleet-cli's link task runs.
sleep 600
SH
chmod +x "$FAKE_PX4_BIN"

# -----------------------------------------------------------------------------
# Scenario: 1 vehicle, no tasks (Fly View telemetry only — no mission), short
# max_time_s so a runaway supervisor can't hang the harness.
# -----------------------------------------------------------------------------
cat > "$SCENARIO_TOML" <<'TOML'
[fleet]
count = 1
battery_sim = false
restart_on_fault = false
hold_for_setup = true   # park the FSM at READY; no mission runs

[env]
geofence = { points_ned_m = [[-200,-200],[200,-200],[200,200],[-200,200]], ceiling_m = 120, floor_m = 0 }
wind_steady_ms = [0.0, 0.0, 0.0]
turbulence = "none"

[success]
max_time_s = 60
TOML

"$MAVFLEET_BIN" check "$SCENARIO_TOML" >/dev/null 2>&1 \
    || fail "scenario TOML failed mavfleet check"

echo "[G-5] mock:     $MOCK_BIN"
echo "[G-5] mavfleet: $MAVFLEET_BIN"
echo "[G-5] ports:    GCS bind :$PORT_GCS_BIND, PX4 listen :$PORT_PX4_LISTEN, API :$PORT_API"
echo "[G-5] work dir: $WORK"

# -----------------------------------------------------------------------------
# Phase 1: start the mock PX4. It binds :14580 and sends telemetry to :14540.
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

# Wait for the mock's state file (ready-signal).
ok=""
for _ in $(seq 1 50); do
    [ -f "$MOCK_STATE" ] && { ok=1; break; }
    kill -0 "$MOCK_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "mock PX4 did not write state file within 5s"
echo "[G-5] phase 1: mock PX4 up (PID $MOCK_PID)"

# -----------------------------------------------------------------------------
# Phase 2: start fleet-cli (mavfleet). The env vars point at the fake px4 tree
# and override the sim command template with /bin/sleep — the mock plays the
# real PX4 wire role, so the spawned px4/sim processes just have to be alive.
# -----------------------------------------------------------------------------
FLEET_PX4_DIR="$FAKE_PX4_DIR" \
FLEET_SIM_COMMAND="/bin/sleep {duration_s}" \
"$MAVFLEET_BIN" run \
    --fleet "$SCENARIO_TOML" \
    --api-port "$PORT_API" \
    --run-dir "$WORK/fleet-run" \
    >"$FC_LOG" 2>&1 &
FC_PID=$!

# Wait for the control plane to come up.
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
echo "[G-5] phase 2: fleet-cli control plane up on :$PORT_API (PID $FC_PID)"

# -----------------------------------------------------------------------------
# Phase 3: wait for the link to establish — the vehicle's heartbeat_seen
# must be true (the link task ingested the mock's first HEARTBEAT, sent at
# 1 Hz from boot).
# -----------------------------------------------------------------------------
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
echo "[G-5] phase 3: link established — heartbeat_seen=true"

# -----------------------------------------------------------------------------
# Phase 4: snapshot A. Wait 3 s (the mock drifts ~5 m on the circle), then
# snapshot B. Assert lat/lon/attitude changed between them.
# -----------------------------------------------------------------------------
curl -s --max-time 2 "http://127.0.0.1:$PORT_API/api/vehicles/0" \
    | jq '.data' >"$SNAP_A" 2>/dev/null \
    || fail "snapshot A: GET /api/vehicles/0 failed"

[ -s "$SNAP_A" ] || fail "snapshot A empty"
LAT_A=$(jq -r '.lat_deg_e7' "$SNAP_A")
LON_A=$(jq -r '.lon_deg_e7' "$SNAP_A")
ATTW_A=$(jq -r '.attitude_q_wxyz[0]' "$SNAP_A")
ATTX_A=$(jq -r '.attitude_q_wxyz[1]' "$SNAP_A")
ATTY_A=$(jq -r '.attitude_q_wxyz[2]' "$SNAP_A")
ATTZ_A=$(jq -r '.attitude_q_wxyz[3]' "$SNAP_A")
echo "[G-5] snap A: lat=$LAT_A lon=$LON_A att_q=[$ATTW_A,$ATTX_A,$ATTY_A,$ATTZ_A]"

# Wait 3 s for the mock's circle drift (radius 5 m @ 0.1 rad/s → ~1.5 m per 3 s,
# enough to move lat/lon_e7 by ~150 — well above the int rounding noise floor).
sleep 3

curl -s --max-time 2 "http://127.0.0.1:$PORT_API/api/vehicles/0" \
    | jq '.data' >"$SNAP_B" 2>/dev/null \
    || fail "snapshot B: GET /api/vehicles/0 failed"

[ -s "$SNAP_B" ] || fail "snapshot B empty"
LAT_B=$(jq -r '.lat_deg_e7' "$SNAP_B")
LON_B=$(jq -r '.lon_deg_e7' "$SNAP_B")
ATTW_B=$(jq -r '.attitude_q_wxyz[0]' "$SNAP_B")
ATTX_B=$(jq -r '.attitude_q_wxyz[1]' "$SNAP_B")
ATTY_B=$(jq -r '.attitude_q_wxyz[2]' "$SNAP_B")
ATTZ_B=$(jq -r '.attitude_q_wxyz[3]' "$SNAP_B")
echo "[G-5] snap B: lat=$LAT_B lon=$LON_B att_q=[$ATTW_B,$ATTX_B,$ATTY_B,$ATTZ_B]"

# (c) lat/lon moved. Allow either to differ — the circle has axis-aligned
#     moments where only one of lat/lon is changing (e.g., at angle=0, only
#     lat moves); require at least one to differ from snapshot A.
[ "$LAT_A" != "$LAT_B" ] || [ "$LON_A" != "$LON_B" ] \
    || fail "telemetry not moving: lat/lon unchanged between snapshots (lat=$LAT_A lon=$LON_A)"

# (d) attitude moved. The mock's yaw follows the circle direction (0.1 rad/s),
#     so the quaternion's z component changes within 3 s. We compare any of
#     the four quaternion components — the roll oscillation and yaw rotation
#     together guarantee one of them moves.
ATT_CHANGED=0
for cmp in "$ATTW_A:$ATTW_B" "$ATTX_A:$ATTX_B" "$ATTY_A:$ATTY_B" "$ATTZ_A:$ATTZ_B"; do
    a="${cmp%%:*}"
    b="${cmp##*:}"
    if [ "$a" != "$b" ]; then
        ATT_CHANGED=1
        break
    fi
done
[ "$ATT_CHANGED" = "1" ] \
    || fail "telemetry not moving: attitude_q_wxyz unchanged between snapshots"

# (e) armed field is false (the mock boots disarmed).
ARMED_B=$(jq -r '.armed' "$SNAP_B")
echo "[G-5] snap B: armed=$ARMED_B (expected false)"
[ "$ARMED_B" = "false" ] || fail "vehicle armed=$ARMED_B (expected false — mock boots disarmed)"

# (f) health field is present (Vec<String> — empty list for a healthy vehicle).
HEALTH_KIND=$(jq -r '.health | type' "$SNAP_B")
echo "[G-5] snap B: health type=$HEALTH_KIND (expected array)"
[ "$HEALTH_KIND" = "array" ] \
    || fail "vehicle health field type=$HEALTH_KIND (expected array — even an empty one for a healthy vehicle)"

echo "[G-5] PASS"
exit 0
