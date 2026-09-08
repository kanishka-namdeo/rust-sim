#!/usr/bin/env bash
# =============================================================================
# G-7: Pre-arm + arm/disarm round-trip (GCS_SPEC.md §9, gate G-7).
#
# Single-invocation harness: start a Python mock PX4 (responds to
# COMMAND_LONG with COMMAND_ACK result=0; arm/disarm command 400 flips the
# heartbeat's MAV_MODE_FLAG_SAFETY_ARMED bit) + the real fleet-cli (mavfleet)
# on a 1-vehicle scenario, then drive the QGC Fly-View arm/disarm flow via
# the HTTP API and assert every step.
#
# Wire topology (matches fleet-mavlink/src/link.rs §3.1):
#   fleet-cli mavlink link binds 127.0.0.1:14540, sends to 127.0.0.1:14580
#   mock PX4 binds 127.0.0.1:14580, sends telemetry to 127.0.0.1:14540
#
# Asserts:
#   (a) `GET /api/vehicles/0/prearm-checks` returns 200 with:
#         - data.checks (array of 5 QGC-style pre-arm checks)
#         - data.all_passed (bool — the Fly View's ARM-button gate)
#       Each check has {name, passed, message}; the 5 names are EKF2, GPS
#       Fix, Mode, Fence, Battery (GCS_SPEC.md §5.2).
#   (b) `POST /api/vehicles/0/arm` with `{"arm": true}` returns 200 with
#         - data.armed == true
#         - data.accepted == true (the COMMAND_ACK round-trip succeeded)
#   (c) After arm, `GET /api/vehicles/0` shows data.armed == true (the
#       mock's heartbeat base_mode propagated MAV_MODE_FLAG_SAFETY_ARMED,
#       which the aggregator wrote into the snapshot).
#   (d) `POST /api/vehicles/0/arm` with `{"arm": false}` returns 200 with
#         - data.armed == false
#         - data.accepted == true
#   (e) After disarm, `GET /api/vehicles/0` shows data.armed == false.
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

PORT_GCS_BIND=14540    # Rust link's bind port (GCS receives heartbeats here)
PORT_PX4_LISTEN=14580  # PX4 onboard listen port (mock binds here)
PORT_API=8400           # fleet-cli control plane

WORK="$(mktemp -d -t g7_test.XXXXXX.dir)"
SCENARIO_TOML="$WORK/scenario.toml"
MOCK_STATE="$WORK/mock_state.json"
MOCK_LOG="$WORK/mock.log"
FC_LOG="$WORK/fleet.log"
VEH_JSON="$WORK/vehicle.json"
PREARM_JSON="$WORK/prearm.json"
ARM_ON_JSON="$WORK/arm_on.json"
ARM_OFF_JSON="$WORK/arm_off.json"

# Fake PX4 binary tree (see G-5 for the rationale).
FAKE_PX4_DIR="$WORK/fake-px4"
FAKE_PX4_BIN="$FAKE_PX4_DIR/build/px4_sitl_default/bin/px4"
FAKE_PX4_ETC="$FAKE_PX4_DIR/build/px4_sitl_default/etc"

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
    if [ -z "${G7_FAILED:-}" ]; then
        rm -rf "$WORK" 2>/dev/null || true
    else
        echo "[G-7] work dir preserved at $WORK (G7_FAILED=1)"
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-7] FAIL: $1"
    G7_FAILED=1
    echo "=========== diagnostics ==========="
    echo "--- mock.log (tail 30):"
    tail -30 "$MOCK_LOG" 2>/dev/null || echo "(none)"
    echo "--- fleet.log (tail 40):"
    tail -40 "$FC_LOG" 2>/dev/null || echo "(none)"
    echo "--- prearm.json:"
    cat "$PREARM_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$PREARM_JSON" 2>/dev/null || echo "(none)"
    echo "--- arm_on.json:"
    cat "$ARM_ON_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$ARM_ON_JSON" 2>/dev/null || echo "(none)"
    echo "--- arm_off.json:"
    cat "$ARM_OFF_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$ARM_OFF_JSON" 2>/dev/null || echo "(none)"
    echo "--- vehicle.json (final state):"
    cat "$VEH_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$VEH_JSON" 2>/dev/null || echo "(none)"
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

for p in "$PORT_API"; do
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
# Fake PX4 binary tree (see G-5).
# -----------------------------------------------------------------------------
mkdir -p "$(dirname "$FAKE_PX4_BIN")" "$FAKE_PX4_ETC"
cat > "$FAKE_PX4_BIN" <<'SH'
#!/bin/sh
sleep 600
SH
chmod +x "$FAKE_PX4_BIN"

# -----------------------------------------------------------------------------
# Scenario: 1 vehicle, no tasks (pre-arm + arm/disarm only — no mission),
# hold_for_setup so the FSM parks at READY.
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
max_time_s = 60
TOML

"$MAVFLEET_BIN" check "$SCENARIO_TOML" >/dev/null 2>&1 \
    || fail "scenario TOML failed mavfleet check"

echo "[G-7] mock:     $MOCK_BIN"
echo "[G-7] mavfleet: $MAVFLEET_BIN"
echo "[G-7] ports:    GCS bind :$PORT_GCS_BIND, PX4 listen :$PORT_PX4_LISTEN, API :$PORT_API"
echo "[G-7] work dir: $WORK"

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

ok=""
for _ in $(seq 1 50); do
    [ -f "$MOCK_STATE" ] && { ok=1; break; }
    kill -0 "$MOCK_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "mock PX4 did not write state file within 5s"
echo "[G-7] phase 1: mock PX4 up (PID $MOCK_PID)"

# -----------------------------------------------------------------------------
# Phase 2: start fleet-cli.
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
echo "[G-7] phase 2: fleet-cli control plane up on :$PORT_API (PID $FC_PID)"

# -----------------------------------------------------------------------------
# Phase 3: wait for the link to establish — heartbeat_seen must be true.
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
echo "[G-7] phase 3: link established — heartbeat_seen=true"

# -----------------------------------------------------------------------------
# Phase 4: GET /api/vehicles/0/prearm-checks — assert 5 checks + all_passed.
# The QGC Fly-View checklist (GCS_SPEC.md §5.2):
#   1. EKF2     — converged (no LINK_STALE / HEARTBEAT_LOST + local_position_seen)
#   2. GPS Fix  — 3D fix (lat/lon non-zero)
#   3. Mode     — STANDBY (disarmed, no mission)
#   4. Fence    — inside inclusion (no GEOFENCE_WARN)
#   5. Battery  — >= 20% (BATTERY_CRIT threshold)
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/prearm_http"
curl -s --max-time 3 -o "$PREARM_JSON" -w "%{http_code}" \
    "http://127.0.0.1:$PORT_API/api/vehicles/0/prearm-checks" >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-7] GET /prearm-checks → HTTP $HTTP_CODE"
[ "$HTTP_CODE" = "200" ] \
    || fail "GET /prearm-checks returned HTTP $HTTP_CODE (expected 200); body: $(cat "$PREARM_JSON" 2>/dev/null)"

PA_OK=$(jq -r '.ok // empty' "$PREARM_JSON")
[ "$PA_OK" = "true" ] || fail "prearm-checks response ok=$PA_OK (expected true); body: $(cat "$PREARM_JSON")"

N_CHECKS=$(jq -r '.data.checks | length' "$PREARM_JSON")
ALL_PASSED=$(jq -r '.data.all_passed // "null"' "$PREARM_JSON")
echo "[G-7] prearm-checks: $N_CHECKS checks, all_passed=$ALL_PASSED"
[ "$N_CHECKS" = "5" ] \
    || fail "prearm-checks returned $N_CHECKS checks (expected 5); body: $(cat "$PREARM_JSON")"

# all_passed must be present (true or false — the harness only requires the
# field exists; whether the Fly-View's ARM button is gated is a UI concern,
# not a wire-level assertion. The HTTP arm endpoint doesn't gate on this.)
[ "$ALL_PASSED" = "true" ] || [ "$ALL_PASSED" = "false" ] \
    || fail "prearm-checks all_passed field missing or not bool: '$ALL_PASSED'"

# Per-check shape: each must have name (str), passed (bool), message (str).
for i in 0 1 2 3 4; do
    NAME=$(jq -r ".data.checks[$i].name" "$PREARM_JSON")
    PASSED=$(jq -r ".data.checks[$i].passed" "$PREARM_JSON")
    MESSAGE=$(jq -r ".data.checks[$i].message" "$PREARM_JSON")
    echo "[G-7]   check[$i]: name=$NAME passed=$PASSED message=$MESSAGE"
    [ -n "$NAME" ] || fail "check[$i] has empty name"
    [ "$PASSED" = "true" ] || [ "$PASSED" = "false" ] \
        || fail "check[$i] '$NAME' passed field is not bool: '$PASSED'"
    [ -n "$MESSAGE" ] || fail "check[$i] '$NAME' has empty message"
done

# Sanity: the 5 check names must match GCS_SPEC.md §5.2's QGC checklist.
EXPECTED_NAMES="EKF2 GPS Fix Mode Fence Battery"
ACTUAL_NAMES=$(jq -r '.data.checks[].name' "$PREARM_JSON" | tr '\n' ' ' | sed 's/ $//')
echo "[G-7] check names: '$ACTUAL_NAMES' (expected '$EXPECTED_NAMES')"
[ "$ACTUAL_NAMES" = "$EXPECTED_NAMES" ] \
    || fail "prearm-checks names '$ACTUAL_NAMES' != expected '$EXPECTED_NAMES'"

echo "[G-7] phase 4: prearm-checks OK (5 checks, all_passed=$ALL_PASSED)"

# -----------------------------------------------------------------------------
# Phase 5: POST /api/vehicles/0/arm with {"arm": true}. The endpoint:
#   - calls guided_gates() (vehicle in range + link registered + not flying)
#   - calls link.send_command(COMPONENT_ARM_DISARM, [1.0, 0, 0, ...])
#   - the link sends COMMAND_LONG(400, arm=1) and waits for COMMAND_ACK
#   - the mock replies COMMAND_ACK(result=0) → CmdAck::Accepted
#   - returns data = {command: "arm", result: 0, accepted: true, index: 0, armed: true}
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/arm_on_http"
curl -s --max-time 5 -o "$ARM_ON_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/vehicles/0/arm" \
    -H "Content-Type: application/json" \
    --data '{"arm": true}' >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-7] POST /arm {arm:true} → HTTP $HTTP_CODE; body: $(cat "$ARM_ON_JSON")"
[ "$HTTP_CODE" = "200" ] \
    || fail "arm POST returned HTTP $HTTP_CODE (expected 200)"

ARM_OK=$(jq -r '.ok // empty' "$ARM_ON_JSON")
# jq gotcha: `.data.armed // "null"` returns "null" when armed=false because
# `//` triggers on both null AND false. Use an explicit type check instead.
ARM_RESP_ARMED=$(jq -r 'if .data.armed == null then "null" elif .data.armed then "true" else "false" end' "$ARM_ON_JSON")
ARM_ACCEPTED=$(jq -r '.data.accepted // "null"' "$ARM_ON_JSON")
ARM_RESULT=$(jq -r '.data.result // "null"' "$ARM_ON_JSON")
echo "[G-7] arm response: ok=$ARM_OK armed=$ARM_RESP_ARMED accepted=$ARM_ACCEPTED result=$ARM_RESULT"
[ "$ARM_OK" = "true" ] || fail "arm response ok=$ARM_OK (expected true)"
[ "$ARM_RESP_ARMED" = "true" ] || fail "arm response armed=$ARM_RESP_ARMED (expected true)"
[ "$ARM_ACCEPTED" = "true" ] \
    || fail "arm response accepted=$ARM_ACCEPTED (expected true — the COMMAND_ACK round-trip must succeed)"
[ "$ARM_RESULT" = "0" ] \
    || fail "arm response result=$ARM_RESULT (expected 0 = MAV_RESULT_ACCEPTED)"

echo "[G-7] phase 5: arm request ACK-ed"

# -----------------------------------------------------------------------------
# Phase 6: poll GET /api/vehicles/0 for armed=true. The mock's heartbeat loop
# runs at 1 Hz, so the next heartbeat (within 1 s) carries
# base_mode |= MAV_MODE_FLAG_SAFETY_ARMED; the aggregator writes s.armed=true
# into the snapshot. Allow 3 s of polling to absorb the heartbeat interval.
# -----------------------------------------------------------------------------
ok=""
for _ in $(seq 1 30); do
    curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/vehicles/0" >"$VEH_JSON" 2>/dev/null || true
    if [ -s "$VEH_JSON" ] && jq -e '.ok == true and .data.armed == true' "$VEH_JSON" >/dev/null 2>&1; then
        ok=1; break
    fi
    sleep 0.1
done
[ -n "$ok" ] \
    || fail "vehicle 0 did not transition to armed=true within 3 s; last json: $(cat "$VEH_JSON" 2>/dev/null || echo none)"
# Re-fetch the snapshot right before the log line (the loop's last VEH_JSON
# might be a tick stale by the time we reach here).
curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/vehicles/0" >"$VEH_JSON" 2>/dev/null || true
VEH_ARMED=$(jq -r 'if .data.armed == null then "null" elif .data.armed then "true" else "false" end' "$VEH_JSON" 2>/dev/null || echo "null")
echo "[G-7] vehicle armed state after arm: $VEH_ARMED"

echo "[G-7] phase 6: vehicle snapshot confirms armed=true"

# -----------------------------------------------------------------------------
# Phase 7: POST /api/vehicles/0/arm with {"arm": false}. The mock flips its
# armed flag back to false; the next heartbeat clears SAFETY_ARMED; the
# aggregator writes s.armed=false into the snapshot.
# -----------------------------------------------------------------------------
HTTP_FILE="$WORK/arm_off_http"
curl -s --max-time 5 -o "$ARM_OFF_JSON" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_API/api/vehicles/0/arm" \
    -H "Content-Type: application/json" \
    --data '{"arm": false}' >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
echo "[G-7] POST /arm {arm:false} → HTTP $HTTP_CODE; body: $(cat "$ARM_OFF_JSON")"
[ "$HTTP_CODE" = "200" ] \
    || fail "disarm POST returned HTTP $HTTP_CODE (expected 200)"

DISARM_OK=$(jq -r '.ok // empty' "$ARM_OFF_JSON")
DISARM_RESP_ARMED=$(jq -r 'if .data.armed == null then "null" elif .data.armed then "true" else "false" end' "$ARM_OFF_JSON")
DISARM_ACCEPTED=$(jq -r '.data.accepted // "null"' "$ARM_OFF_JSON")
echo "[G-7] disarm response: ok=$DISARM_OK armed=$DISARM_RESP_ARMED accepted=$DISARM_ACCEPTED"
[ "$DISARM_OK" = "true" ] || fail "disarm response ok=$DISARM_OK (expected true)"
[ "$DISARM_RESP_ARMED" = "false" ] || fail "disarm response armed=$DISARM_RESP_ARMED (expected false)"
[ "$DISARM_ACCEPTED" = "true" ] \
    || fail "disarm response accepted=$DISARM_ACCEPTED (expected true)"

echo "[G-7] phase 7: disarm request ACK-ed"

# -----------------------------------------------------------------------------
# Phase 8: poll GET /api/vehicles/0 for armed=false.
# -----------------------------------------------------------------------------
ok=""
for _ in $(seq 1 30); do
    curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/vehicles/0" >"$VEH_JSON" 2>/dev/null || true
    if [ -s "$VEH_JSON" ] && jq -e '.ok == true and .data.armed == false' "$VEH_JSON" >/dev/null 2>&1; then
        ok=1; break
    fi
    sleep 0.1
done
[ -n "$ok" ] \
    || fail "vehicle 0 did not transition to armed=false within 3 s; last json: $(cat "$VEH_JSON" 2>/dev/null || echo none)"
# Re-fetch the snapshot right before the assertion so the log line reflects
# the latest vehicle state (the loop's last VEH_JSON might be a tick stale).
curl -s --max-time 1 "http://127.0.0.1:$PORT_API/api/vehicles/0" >"$VEH_JSON" 2>/dev/null || true
VEH_ARMED=$(jq -r 'if .data.armed == null then "null" elif .data.armed then "true" else "false" end' "$VEH_JSON" 2>/dev/null || echo "null")
echo "[G-7] vehicle armed state after disarm: $VEH_ARMED"

echo "[G-7] phase 8: vehicle snapshot confirms armed=false (disarm round-trip complete)"

echo "[G-7] PASS"
exit 0
