#!/usr/bin/env bash
# =============================================================================
# G-6: Multi-vehicle Fly View (GCS_SPEC.md §9, gate G-6).
#
# Single-invocation harness: start TWO Python mock PX4 instances (one per
# vehicle) + the real fleet-cli (mavfleet) on a 2-vehicle scenario, then
# poll `GET /api/vehicles/0` and `GET /api/vehicles/1` and assert:
#   (a) Both vehicles return data (both links established).
#   (b) The two vehicles have distinct sysid (1 for vehicle 0, 2 for vehicle 1,
#       per fleet-mavlink/src/link.rs LinkConfig::for_instance(i).sysid = i+1).
#   (c) The two vehicles have distinct positions (the mocks boot at different
#       lat/lon so the operator map can render them as two distinct markers).
#
# The spec gate also says "select-active switch <100 ms" — that is a UI
# performance assertion (the operator clicks one vehicle's marker, the Fly
# View's "active" vehicle switches within 100 ms). The HTTP API exposes the
# vehicle table as a flat list; the select-active state is a frontend concern
# (the WS frame's `vehicles` array is identical whether one vehicle is
# "active" or none is). This harness verifies the multi-vehicle *telemetry*
# half of the gate; the <100 ms select-active UI test is part of the
# console-side Playwright suite (browser_map_test.sh).
#
# Wire topology (matches fleet-mavlink/src/link.rs §3.1):
#   vehicle 0: GCS binds 127.0.0.1:14540, mock binds 127.0.0.1:14580
#   vehicle 1: GCS binds 127.0.0.1:14541, mock binds 127.0.0.1:14581
# Both mocks send telemetry at 10 Hz + heartbeats at 1 Hz, like G-5.
#
# Mocks boot at distinct geo origins (vehicle 1 is offset 100 m east of
# vehicle 0) so the operator map renders two distinct markers and the
# harness can assert lat/lon differ between the two vehicles' snapshots.
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

# Ports per fleet-mavlink/src/link.rs LinkConfig::for_instance(i).
PORT_GCS_BIND_0=14540
PORT_PX4_LISTEN_0=14580
PORT_GCS_BIND_1=14541
PORT_PX4_LISTEN_1=14581
PORT_API=8400

WORK="$(mktemp -d -t g6_test.XXXXXX.dir)"
SCENARIO_TOML="$WORK/scenario.toml"
MOCK_STATE_0="$WORK/mock_state_0.json"
MOCK_STATE_1="$WORK/mock_state_1.json"
MOCK_LOG_0="$WORK/mock_0.log"
MOCK_LOG_1="$WORK/mock_1.log"
FC_LOG="$WORK/fleet.log"
VEH0_JSON="$WORK/vehicle_0.json"
VEH1_JSON="$WORK/vehicle_1.json"

# Fake PX4 binary tree (see G-5 for the rationale — the mock PX4 plays the
# wire role via UDP; the spawned px4/sim processes just have to stay alive).
FAKE_PX4_DIR="$WORK/fake-px4"
FAKE_PX4_BIN="$FAKE_PX4_DIR/build/px4_sitl_default/bin/px4"
FAKE_PX4_ETC="$FAKE_PX4_DIR/build/px4_sitl_default/etc"

# PIDs to clean up.
FC_PID=""
MOCK_PID_0=""
MOCK_PID_1=""

cleanup() {
    for pid in "$FC_PID" "$MOCK_PID_0" "$MOCK_PID_1"; do
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
    if [ -z "${G6_FAILED:-}" ]; then
        rm -rf "$WORK" 2>/dev/null || true
    else
        echo "[G-6] work dir preserved at $WORK (G6_FAILED=1)"
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-6] FAIL: $1"
    G6_FAILED=1
    echo "=========== diagnostics ==========="
    echo "--- mock_0.log (tail 20):"
    tail -20 "$MOCK_LOG_0" 2>/dev/null || echo "(none)"
    echo "--- mock_1.log (tail 20):"
    tail -20 "$MOCK_LOG_1" 2>/dev/null || echo "(none)"
    echo "--- fleet.log (tail 50):"
    tail -50 "$FC_LOG" 2>/dev/null || echo "(none)"
    echo "--- vehicle_0.json:"
    cat "$VEH0_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$VEH0_JSON" 2>/dev/null || echo "(none)"
    echo "--- vehicle_1.json:"
    cat "$VEH1_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$VEH1_JSON" 2>/dev/null || echo "(none)"
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
for p in "$PORT_API"; do
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
# Fake PX4 binary tree (see G-5).
# -----------------------------------------------------------------------------
mkdir -p "$(dirname "$FAKE_PX4_BIN")" "$FAKE_PX4_ETC"
cat > "$FAKE_PX4_BIN" <<'SH'
#!/bin/sh
sleep 600
SH
chmod +x "$FAKE_PX4_BIN"

# -----------------------------------------------------------------------------
# Scenario: 2 vehicles, no tasks, hold_for_setup so the FSM parks at READY.
# The geofence is bigger than G-5's so the second vehicle's 100 m east offset
# fits inside.
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
max_time_s = 60
TOML

"$MAVFLEET_BIN" check "$SCENARIO_TOML" >/dev/null 2>&1 \
    || fail "scenario TOML failed mavfleet check"

echo "[G-6] mock:     $MOCK_BIN"
echo "[G-6] mavfleet: $MAVFLEET_BIN"
echo "[G-6] ports:    v0 GCS :$PORT_GCS_BIND_0 PX4 :$PORT_PX4_LISTEN_0"
echo "[G-6]           v1 GCS :$PORT_GCS_BIND_1 PX4 :$PORT_PX4_LISTEN_1"
echo "[G-6]           API :$PORT_API"
echo "[G-6] work dir: $WORK"

# -----------------------------------------------------------------------------
# Phase 1: start both mock PX4 instances. Vehicle 0 boots at the default
# geo origin (47.397770, 8.545580); vehicle 1 boots 100 m east — 100 m
# east is ~+0.000898 deg lon ≈ +8980 lon_e7, distinct enough that the
# operator map renders two well-separated markers.
# -----------------------------------------------------------------------------
"$PY" "$MOCK_BIN" \
    --port "$PORT_PX4_LISTEN_0" \
    --gcs-port "$PORT_GCS_BIND_0" \
    --state "$MOCK_STATE_0" \
    --instance 0 \
    --sysid 1 \
    --ttl-secs 120 \
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
    --ttl-secs 120 \
    --telemetry-hz 10 \
    --lat-e7 473977700 \
    --lon-e7 85464780 \
    >"$MOCK_LOG_1" 2>&1 &
MOCK_PID_1=$!

# Wait for both state files (mocks up).
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
echo "[G-6] phase 1: both mock PX4 instances up (PIDs $MOCK_PID_0, $MOCK_PID_1)"

# -----------------------------------------------------------------------------
# Phase 2: start fleet-cli. The 2-vehicle scenario spawns two links + two
# (fake) px4/sim pairs; the link tasks bind :14540 and :14541 simultaneously.
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
echo "[G-6] phase 2: fleet-cli control plane up on :$PORT_API (PID $FC_PID)"

# -----------------------------------------------------------------------------
# Phase 3: wait for both vehicles' heartbeat_seen to become true. The mocks
# send heartbeats at 1 Hz, so within ~1-2 s of fleet-cli's link tasks binding
# :14540 and :14541, the first heartbeat should arrive.
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
echo "[G-6] phase 3: both links established — heartbeat_seen=true on v0 and v1"

# Refresh the snapshots so the position/sysid fields reflect the latest
# telemetry (the wait-for-heartbeat loop above captured an early sample).
sleep 1
curl -s --max-time 2 "http://127.0.0.1:$PORT_API/api/vehicles/0" | jq '.data' >"$VEH0_JSON" 2>/dev/null \
    || fail "GET /api/vehicles/0 failed during assertion"
curl -s --max-time 2 "http://127.0.0.1:$PORT_API/api/vehicles/1" | jq '.data' >"$VEH1_JSON" 2>/dev/null \
    || fail "GET /api/vehicles/1 failed during assertion"

# -----------------------------------------------------------------------------
# Assertions.
# -----------------------------------------------------------------------------
# (a) both vehicles have a sysid (heartbeat arrived and the aggregate set it).
SYSID_0=$(jq -r '.sysid' "$VEH0_JSON")
SYSID_1=$(jq -r '.sysid' "$VEH1_JSON")
echo "[G-6] v0 sysid=$SYSID_0  v1 sysid=$SYSID_1 (expected 1 and 2)"
[ "$SYSID_0" = "1" ] || fail "vehicle 0 sysid=$SYSID_0 (expected 1)"
[ "$SYSID_1" = "2" ] || fail "vehicle 1 sysid=$SYSID_1 (expected 2)"

# (b) distinct sysid.
[ "$SYSID_0" != "$SYSID_1" ] \
    || fail "vehicle 0 and vehicle 1 have the same sysid ($SYSID_0) — multi-vehicle sysid collision"

# (c) both vehicles have a non-zero lat/lon (heartbeat + GLOBAL_POSITION_INT
#     have arrived). Lat/lon 0 means we got no GLOBAL_POSITION_INT yet.
LAT_0=$(jq -r '.lat_deg_e7' "$VEH0_JSON")
LON_0=$(jq -r '.lon_deg_e7' "$VEH0_JSON")
LAT_1=$(jq -r '.lat_deg_e7' "$VEH1_JSON")
LON_1=$(jq -r '.lon_deg_e7' "$VEH1_JSON")
echo "[G-6] v0 lat=$LAT_0 lon=$LON_0  v1 lat=$LAT_1 lon=$LON_1"
[ "$LAT_0" != "0" ] || fail "vehicle 0 lat_deg_e7=0 (no GLOBAL_POSITION_INT received)"
[ "$LAT_1" != "0" ] || fail "vehicle 1 lat_deg_e7=0 (no GLOBAL_POSITION_INT received)"

# (d) distinct positions (the mocks boot 100 m apart — see --lon-e7 offset
#     above — so the snapshots must differ even before any drift).
[ "$LAT_0" != "$LAT_1" ] || [ "$LON_0" != "$LON_1" ] \
    || fail "vehicles 0 and 1 have the same lat/lon ($LAT_0, $LON_0) — multi-vehicle marker collision"

# Sanity: the lon delta should be large (≈ +8980 lon_e7 for the 100 m offset)
# to confirm we're comparing the right vehicles' snapshots (not the same one).
LON_DELTA=$(( LON_1 - LON_0 ))
echo "[G-6] lon delta v1 - v0 = $LON_DELTA (expected ≈ +8980 for 100 m east offset)"
# Use abs() via the standard bash trick.
LON_DELTA_ABS=${LON_DELTA#-}
[ "$LON_DELTA_ABS" -ge 5000 ] \
    || fail "lon delta v1-v0 = $LON_DELTA — vehicles are <50 m apart (mocks must boot at distinct geo origins)"

echo "[G-6] PASS"
exit 0
