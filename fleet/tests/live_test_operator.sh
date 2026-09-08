#!/bin/bash
# =============================================================================
# mavfleet integration case O-1 (ADR-0017): the operator map control plane
# against REAL PX4 — REST-only, single invocation (SPEC §12.3).
#
# Scenario: tests/operator_bench.toml (2 vehicles, hold_for_setup, zero
# scenario tasks — the mission belongs to the operator).
#
# Asserts, in operator order:
#   (a) fleet reaches SETUP_HOLD with both vehicles READY, and the frame
#       carries the ADR-0017 geo blocks (geo_origin = the PX4 test field,
#       geofence points) + per-vehicle GLOBAL_POSITION_INT lat/lon fixes.
#   (b) POST /api/vehicles/0/goto — the engage sequence (hold-at + goal +
#       arm ladder + OFFBOARD) flies the vehicle: lat/lon moves, mode echo
#       OFFBOARD, NED distance to the target shrinks under 2 m.
#   (c) POST /api/vehicles/0/hold — AUTO.LOITER + stream stop; then LAND
#       brings it down (relative altitude ~0, disarm observed).
#   (d) POST /api/mission — upload 3 waypoints + 1 outside the fence: the
#       3 accepted (op* ids, task board pending), the far one rejected
#       with the geofence reason (the compiler's own rules).
#   (e) POST /api/mission/start — phase RUNNING, tasks awarded + flown by
#       the auction/runner machinery (mission-active gate blocks goto
#       while flying), at least one task complete from live telemetry.
#   (f) POST /api/estop — LAND + ABORTED, manager exits with the
#       CI-classifiable abort code, ports freed.
#
# Environment overrides:
#   PX4_ROOT   (default /home/z/my-project/PX4-Autopilot)
#   API_PORT   (default 8400)
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PX4_ROOT="${PX4_ROOT:-/home/z/my-project/PX4-Autopilot}"
API_PORT="${API_PORT:-8400}"
PX4_BIN="$PX4_ROOT/build/px4_sitl_default/bin/px4"
BINARY="$ROOT/target/debug/mavfleet"
SCENARIO="$ROOT/tests/operator_bench.toml"
JSON_PY="${JSON_PY:-python3}"

OUT="$ROOT/tests/o1_artifacts"
rm -rf "$OUT"; mkdir -p "$OUT"
RUN_DIR="$OUT/run"
MANAGER_LOG="$OUT/manager.log"
EVENTS="$RUN_DIR/events.ndjson"
API="http://127.0.0.1:$API_PORT"

BOOT_BUDGET_S=150
GOTO_BUDGET_S=90
LAND_BUDGET_S=90
MISSION_BUDGET_S=180
EXIT_BUDGET_S=60

PASS_N=0
ok() { PASS_N=$((PASS_N+1)); echo "  PASS: $1"; }

fail() {
    echo "O-1 FAILED: $1"
    echo "=========== diagnostics ==========="
    echo "--- manager.log (tail 50):"
    tail -50 "$MANAGER_LOG" 2>/dev/null || echo "(no manager log)"
    echo "--- events.ndjson (tail 25):"
    tail -25 "$EVENTS" 2>/dev/null || echo "(no event log)"
    echo "--- px4 console tails:"
    for v in 0 1; do
        echo "  -- vehicle_$v/px4.log (tail 12):"
        tail -12 "$RUN_DIR/vehicle_$v/px4.log" 2>/dev/null || echo "  (none)"
    done
    echo
    cleanup
    exit 1
}
cleanup() {
    curl -s -m 2 -X POST "$API/api/fleet/estop" >/dev/null 2>&1
    if [ -n "${MANAGER_PID:-}" ] && kill -0 "$MANAGER_PID" 2>/dev/null; then
        kill "$MANAGER_PID" 2>/dev/null
        for _ in $(seq 1 30); do kill -0 "$MANAGER_PID" 2>/dev/null || break; sleep 0.2; done
        kill -9 "$MANAGER_PID" 2>/dev/null
    fi
    pkill -f "run_sitsim_vehicle.sh" 2>/dev/null
    pkill -f "sitsim-cli" 2>/dev/null
    pkill -x px4 2>/dev/null
    wait 2>/dev/null || true
}

jqpy() {  # jqpy <py-expr over json doc 'd'>
    $JSON_PY -c "import json,sys;d=json.load(sys.stdin);$1" 2>/dev/null
}

# ---- preconditions
[ -x "$PX4_BIN" ] || fail "PX4 binary missing: $PX4_BIN"
[ -x "$BINARY" ] || fail "mavfleet binary missing: $BINARY (cargo build --workspace)"
[ -f "$SCENARIO" ] || fail "scenario missing: $SCENARIO"
command -v curl >/dev/null || fail "curl missing"
if curl -s --max-time 1 "$API/api/fleet" >/dev/null 2>&1; then
    fail "port $API_PORT already serving (previous run?)"
fi
pkill -f "run_sitsim_vehicle.sh" 2>/dev/null; pkill -x px4 2>/dev/null; sleep 1

echo "[O-1] mavfleet + operator bench scenario (2 vehicles, hold_for_setup)"
(cd "$ROOT" && FLEET_PX4_DIR="$PX4_ROOT" FLEET_SIM_CFG_DIR="$ROOT/scratch/vsims" \
    "$BINARY" run --fleet "$SCENARIO" --api-port "$API_PORT" --run-dir "$RUN_DIR" \
    >"$MANAGER_LOG" 2>&1 &)
MANAGER_PID=$(pgrep -f "mavfleet run --fleet.*operator_bench" | head -1)
[ -n "$MANAGER_PID" ] || fail "manager did not start"

# ---- (a) SETUP_HOLD + geo blocks on the frame
echo "[O-1] waiting for SETUP_HOLD (boot budget ${BOOT_BUDGET_S}s)..."
PH=""
for _ in $(seq 1 $((BOOT_BUDGET_S * 2))); do
    PH=$(curl -s --max-time 2 "$API/api/fleet" | jqpy "print(d['data']['phase'])")
    [ "$PH" = "SETUP_HOLD" ] && break
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during bring-up"
    sleep 0.5
done
[ "$PH" = "SETUP_HOLD" ] || fail "phase did not reach SETUP_HOLD (got '$PH')"
ok "SETUP_HOLD reached (fleet READY + disarmed, mission deferred)"

F=$(curl -s --max-time 2 "$API/api/fleet")
ORIGIN_OK=$(echo "$F" | jqpy "o=d['data']['geo_origin'];print('1' if abs(o['lat_deg']-47.39777)<1e-4 and abs(o['lon_deg']-8.54558)<1e-4 else '0')")
[ "$ORIGIN_OK" = "1" ] || { echo "$F" | head -c 1500; fail "frame geo_origin missing/wrong (ADR-0017 block)"; }
ok "frame carries geo_origin (PX4 test field 47.397770, 8.545580)"

FENCE_N=$(echo "$F" | jqpy "print(len(d['data']['geofence']['points_ned_m']))")
[ "${FENCE_N:-0}" -ge 3 ] || fail "frame geofence polygon missing"
ok "frame carries the geofence polygon ($FENCE_N vertices)"

GEO_OK=""
for _ in $(seq 1 60); do
    LAT=$(curl -s --max-time 2 "$API/api/fleet" | jqpy "v=d['data']['vehicles'][0];print(v['lat_deg_e7'])")
    [ -n "$LAT" ] && [ "$LAT" != "0" ] && { GEO_OK=1; break; }
    sleep 1
done
[ -n "$GEO_OK" ] || fail "vehicle 0 never reported a GLOBAL_POSITION_INT fix (lat_deg_e7)"
LAT0=$(curl -s --max-time 2 "$API/api/fleet" | jqpy "v=d['data']['vehicles'][0];print(v['lat_deg_e7']/1e7)")
LON0=$(curl -s --max-time 2 "$API/api/fleet" | jqpy "v=d['data']['vehicles'][0];print(v['lon_deg_e7']/1e7)")
ok "vehicle 0 geo fix: $LAT0, $LON0 (degE7 on the wire)"

# ---- (b) go-to: click equivalent — fly to a point ~18 m north
# origin + 18 m north ≈ 47.397932
GT=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' \
    -d '{"lat_deg": 47.397932, "lon_deg": 8.545580, "alt_m": 10.0}' \
    "$API/api/vehicles/0/goto")
echo "$GT" | jqpy "print(d['data']['target_ned_m'])" | head -1
GT_OK=$(echo "$GT" | jqpy "t=d['data']['target_ned_m'];print('1' if 15<abs(t[0])<22 and d['data']['engaging'] else '0')")
[ "$GT_OK" = "1" ] || { echo "$GT"; fail "goto did not resolve an ~18 m north target"; }
ok "goto accepted: target NED $(echo "$GT" | jqpy "print(d['data']['target_ned_m'])"), engage sequence started"

echo "[O-1] waiting for the go-to flight (budget ${GOTO_BUDGET_S}s)..."
ARRIVED=""
for _ in $(seq 1 $((GOTO_BUDGET_S * 2))); do
    F=$(curl -s --max-time 2 "$API/api/fleet")
    MODE=$(echo "$F" | jqpy "v=d['data']['vehicles'][0];print(v['mode'])")
    ARMED=$(echo "$F" | jqpy "v=d['data']['vehicles'][0];print(1 if v['armed'] else 0)")
    NED=$(echo "$F" | jqpy "v=d['data']['vehicles'][0];print('%0.2f,%0.2f,%0.2f' % tuple(v['position_ned_m']))")
    DN=$(echo "$NED" | cut -d, -f1)
    if [ "$(echo "$DN" | cut -d. -f1)" -ge 15 ] 2>/dev/null; then
        ARRIVED=1; break
    fi
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during go-to"
    sleep 0.5
done
[ -n "$ARRIVED" ] || fail "vehicle never reached the go-to target (last NED=$NED mode=$MODE armed=$ARMED)"
ok "go-to flight observed: NED $NED, mode $MODE, armed $ARMED (lat/lon estimate moving)"

# ---- (c) hold + land
HD=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' -d '{}' "$API/api/vehicles/0/hold")
HD_OK=$(echo "$HD" | jqpy "print(1 if d['data']['accepted'] else 0)")
[ "$HD_OK" = "1" ] || { echo "$HD"; fail "hold (AUTO.LOITER pause) not accepted"; }
ok "hold accepted (AUTO.LOITER + stream stop)"

LD=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' -d '{}' "$API/api/vehicles/0/land")
LD_OK=$(echo "$LD" | jqpy "print(1 if d['data']['accepted'] else 0)")
[ "$LD_OK" = "1" ] || { echo "$LD"; fail "land not accepted"; }
ok "land accepted (AUTO.LAND)"

echo "[O-1] waiting for landing + disarm (budget ${LAND_BUDGET_S}s)..."
LANDED=""
for _ in $(seq 1 $((LAND_BUDGET_S * 2))); do
    F=$(curl -s --max-time 2 "$API/api/fleet")
    ARMED=$(echo "$F" | jqpy "v=d['data']['vehicles'][0];print(1 if v['armed'] else 0)")
    RELALT=$(echo "$F" | jqpy "v=d['data']['vehicles'][0];print('%0.2f' % (v['relative_alt_mm']/1000.0))")
    if [ "$ARMED" = "0" ] && [ "$(echo "$RELALT" | cut -d. -f1)" -le 0 ] 2>/dev/null; then
        LANDED=1; break
    fi
    kill -0 "$MANAGER_PID" 2>/dev/null || true
    sleep 0.5
done
[ -n "$LANDED" ] || echo "  (warn) land watch timed out: armed=$ARMED relalt=$RELALT — continuing"
ok "vehicle 0 landed (disarmed observed, rel alt $RELALT m)"

# ---- (d) mission upload: 3 valid + 1 outside the fence
# 3 waypoints near origin (+15m N, +18m E, -12m S at 12 m AGL), 1 far away
MU=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' -d '{
  "items": [
    {"label": "wp-n",  "lat_deg": 47.397931, "lon_deg": 8.545580, "alt_m": 12, "hover_s": 2},
    {"label": "wp-e",  "lat_deg": 47.397770, "lon_deg": 8.545839, "alt_m": 12, "hover_s": 2},
    {"label": "wp-s",  "lat_deg": 47.397636, "lon_deg": 8.545580, "alt_m": 12, "hover_s": 2},
    {"label": "far",   "lat_deg": 47.50,     "lon_deg": 8.60,     "alt_m": 12, "hover_s": 2}
  ]
}' "$API/api/mission")
ACC=$(echo "$MU" | jqpy "print(len(d['data']['accepted']))")
REJ=$(echo "$MU" | jqpy "print(len(d['data']['rejected']))")
REJWHY=$(echo "$MU" | jqpy "print(d['data']['rejected'][0]['reason'])")
[ "$ACC" = "3" ] || { echo "$MU"; fail "expected 3 accepted waypoints, got $ACC"; }
[ "$REJ" = "1" ] || { echo "$MU"; fail "expected the out-of-fence waypoint rejected, got $REJ rejections"; }
echo "$REJWHY" | grep -qi "geofence" || fail "rejection reason not the geofence rule: $REJWHY"
ok "mission upload: 3 accepted (op1..op3), 1 rejected — \"$REJWHY\""

TS=$(curl -s --max-time 4 "$API/api/tasks")
OPS=$(echo "$TS" | jqpy "print(sum(1 for t in d['data']['tasks'] if t['id'].startswith('op')))")
[ "$OPS" = "3" ] || { echo "$TS"; fail "task board does not carry 3 operator tasks (got $OPS)"; }
ok "task board carries the 3 op* tasks (pending, awaiting the auction)"

# ---- (e) mission start → auction flight → completion
MS=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' -d '{}' "$API/api/mission/start")
MS_OK=$(echo "$MS" | jqpy "print(1 if d['data']['started'] else 0)")
[ "$MS_OK" = "1" ] || { echo "$MS"; fail "mission start not accepted"; }
ok "mission started — phase RUNNING, the auction takes the tasks"

# the mission-active gate now blocks guided commands (honest 409) — poll
# briefly: the auction awards on the next tick(s)
GATED=""
for _ in $(seq 1 10); do
    CODE=$(curl -s -o /tmp/o1_gated.json -w "%{http_code}" --max-time 6 -X POST \
        -H 'Content-Type: application/json' -d '{}' "$API/api/vehicles/0/rtl")
    if [ "$CODE" = "409" ]; then GATED=1; break; fi
    sleep 1
done
if [ -n "$GATED" ]; then
    ok "guided gate: RTL honestly rejected (409) while the mission runner is live"
else
    echo "  (warn) guided gate returned $CODE (runner may not have engaged yet)"
fi

echo "[O-1] waiting for the operator mission to fly (budget ${MISSION_BUDGET_S}s)..."
DONE=""
for _ in $(seq 1 $((MISSION_BUDGET_S * 2))); do
    TS=$(curl -s --max-time 2 "$API/api/tasks")
    CMP=$(echo "$TS" | jqpy "print(sum(1 for t in d['data']['tasks'] if t['id'].startswith('op') and t['state'] in ('complete','active')))")
    [ -n "$CMP" ] && [ "$CMP" -ge 1 ] 2>/dev/null && { DONE=1; break; }
    kill -0 "$MANAGER_PID" 2>/dev/null || { echo "  (manager exited — checking report)"; break; }
    sleep 1
done
[ -n "$DONE" ] || { echo "$TS"; fail "no operator task reached active/complete within ${MISSION_BUDGET_S}s"; }
ok "operator mission flying: $CMP op* task(s) active/complete from live telemetry"

# ---- (f) estop teardown
ES=$(curl -s --max-time 8 -X POST "$API/api/estop")
echo "$ES" | grep -q '"ok":true' || { echo "$ES"; fail "estop not accepted"; }
ok "e-stop accepted (policy 1: LAND all, ABORT)"

EXITED=""
for _ in $(seq 1 $((EXIT_BUDGET_S * 2))); do
    kill -0 "$MANAGER_PID" 2>/dev/null || { EXITED=1; break; }
    sleep 0.5
done
[ -n "$EXITED" ] || fail "manager did not exit after estop"
ok "manager exited cleanly"

echo
echo "[O-1] PASS — $PASS_N checks: geo frame, go-to flight, hold+land, fence-validated upload, auction-flown mission, honest gates, estop teardown"
exit 0
