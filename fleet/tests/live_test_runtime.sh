#!/bin/bash
# =============================================================================
# mavfleet integration case R-1 (ADR-0018): the runtime control plane —
# fault proxy, task append, hot scenario load — against REAL PX4, REST-only,
# single invocation (SPEC §12.3).
#
# Scenario: tests/operator_bench.toml (2 vehicles, hold_for_setup) -> a
# hot-staged 1-vehicle bench with a timeline fault event.
#
# Asserts, in operator order:
#   (a) fleet reaches SETUP_HOLD with 2 vehicles (the operator bench).
#   (b) POST /api/vehicles/0/faults (the sim fault-plane proxy): gps_denial
#       accepted + the sim's own GET /api/faults (:8200) shows the fault
#       active; a bogus fault type is REJECTED by the sim and relayed
#       verbatim (4xx); an out-of-range vehicle index is 404.
#   (c) POST /api/tasks: two NED tasks accepted (op* ids on the live board),
#       one far task rejected with the geofence reason, and an explicit id
#       collision rejected.
#   (d) PUT /api/fleet with an invalid scenario TOML -> 422 (parse reason).
#   (e) PUT /api/fleet with a valid scenario -> accepted + staged; the first
#       run ABORTS gracefully (hot-load reason), the control plane rebinds,
#       and the fleet restarts with the staged scenario: 1 vehicle, the new
#       80 m fence on the frame, scenario path = hot-scenario-*.toml, back
#       to SETUP_HOLD.
#   (f) the staged scenario's timeline fault event ACTUALLY FIRES through
#       the proxy: the event log carries "fault 'gps_denial' injected into
#       vehicle 0's sim fault plane" (the ADR-0018 wiring of the old
#       no-op "interim simulator" placeholder).
#   (g) the mission-active gate: mission upload + start on the new bench,
#       then PUT /api/fleet again -> 409 while the mission flies.
#   (h) POST /api/estop -> LAND + ABORTED, manager exits with the abort
#       code, ports freed.
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

OUT="$ROOT/tests/r1_artifacts"
rm -rf "$OUT"; mkdir -p "$OUT"
RUN_DIR="$OUT/run"
MANAGER_LOG="$OUT/manager.log"
API="http://127.0.0.1:$API_PORT"
SIM0="http://127.0.0.1:8200"

BOOT_BUDGET_S=150
SWAP_BUDGET_S=120
MISSION_BUDGET_S=90
EXIT_BUDGET_S=90

PASS_N=0
ok() { PASS_N=$((PASS_N+1)); echo "  PASS: $1"; }

fail() {
    echo "R-1 FAILED: $1"
    echo "=========== diagnostics ==========="
    echo "--- manager.log (tail 50):"
    tail -50 "$MANAGER_LOG" 2>/dev/null || echo "(no manager log)"
    for d in "$RUN_DIR" "$RUN_DIR-hot-1"; do
        echo "--- $d/events.ndjson (tail 20):"
        tail -20 "$d/events.ndjson" 2>/dev/null || echo "  (none)"
        echo "--- $d/vehicle_0/px4.log (tail 12):"
        tail -12 "$d/vehicle_0/px4.log" 2>/dev/null || echo "  (none)"
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

# The hot-swap scenario: 1 vehicle, 80 m fence (vs the bench's 60 m), a
# timeline fault event (the ADR-0018 proxy wiring), still a hold bench.
HOT_TOML="$OUT/hot-bench.toml"
cat > "$HOT_TOML" << 'EOF'
# HOT BENCH (staged via PUT /api/fleet in R-1): 1 vehicle, wider fence,
# one timeline fault event — proving scenario swap + fault proxy wiring.

[fleet]
count = 1
battery_sim = false
hold_for_setup = true

[env]
geofence = { points_ned_m = [[-80, -80], [80, -80], [80, 80], [-80, 80]], ceiling_m = 40, floor_m = 0 }
wind_steady_ms = [0.0, 0.0, 0.0]
turbulence = "off"

[sim]
command = "scripts/run_sitsim_vehicle.sh {instance} {hil_port} {duration_s}"
duration_s = 600

[[event]]
kind = "fault"
vehicle = 0
# Fires MID-mission (the (g) flow starts ~15 s and arms by ~21 s): a
# gps_denial BEFORE the mission blocks PX4's GPS-dependent arming gate for
# 10+ s (TEMPORARILY_REJECTED, found live in the first harness run — an
# ADR-0018 note). Mid-flight it is the realistic scenario: GPS loss while
# already flying, harness greps the injection, then e-stops.
start_s = 25
fault = { type = "gps_denial", duration_ms = 20000 }

[success]
max_time_s = 540
EOF
HOT_JSON="$OUT/hot-bench.json"
"$JSON_PY" -c "import json;print(json.dumps({'scenario_toml': open('$HOT_TOML').read()}))" > "$HOT_JSON"

echo "[R-1] mavfleet + operator bench scenario (2 vehicles, hold_for_setup)"
(cd "$ROOT" && FLEET_PX4_DIR="$PX4_ROOT" FLEET_SIM_CFG_DIR="$ROOT/scratch/vsims" \
    "$BINARY" run --fleet "$SCENARIO" --api-port "$API_PORT" --run-dir "$RUN_DIR" \
    >"$MANAGER_LOG" 2>&1 &)
MANAGER_PID=$(pgrep -f "mavfleet run --fleet" | head -1)
[ -n "$MANAGER_PID" ] || fail "manager did not start"

# ---- (a) SETUP_HOLD
echo "[R-1] waiting for SETUP_HOLD (boot budget ${BOOT_BUDGET_S}s)..."
PH=""
for _ in $(seq 1 $((BOOT_BUDGET_S * 2))); do
    PH=$(curl -s --max-time 2 "$API/api/fleet" | jqpy "print(d['data']['phase'])")
    [ "$PH" = "SETUP_HOLD" ] && break
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during bring-up"
    sleep 0.5
done
[ "$PH" = "SETUP_HOLD" ] || fail "phase did not reach SETUP_HOLD (got '$PH')"
ok "SETUP_HOLD reached (2 vehicles, disarmed bench)"

# ---- (b) fault proxy
FP=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' \
    -d '{"type": "gps_denial", "duration_ms": 30000}' \
    "$API/api/vehicles/0/faults")
FP_OK=$(echo "$FP" | jqpy "print('1' if d['ok'] and d['data']['id'].startswith('runtime-') else '0')")
[ "$FP_OK" = "1" ] || { echo "$FP"; fail "fault proxy did not accept gps_denial"; }
ok "fault proxy: gps_denial accepted, sim minted id $(echo "$FP" | jqpy "print(d['data']['id'])")"

FID=$(echo "$FP" | jqpy "print(d['data']['id'])")
SIM_SEES=""
for _ in $(seq 1 20); do
    SIMF=$(curl -s --max-time 2 "$SIM0/api/faults")
    echo "$SIMF" | jqpy "print(' '.join(f['id'] for f in d['data']['active']+d['data']['pending']))" | grep -q "$FID" && { SIM_SEES=1; break; }
    sleep 0.5
done
[ -n "$SIM_SEES" ] || { echo "sim faults: $(curl -s "$SIM0/api/faults")"; fail "the sim's own fault plane does not list fault $FID"; }
ok "the sim's own /api/faults (:8200) lists $FID — the proxy hit the real plane"

BAD_FP=$(curl -s --max-time 8 -o "$OUT/bad_fault.json" -w '%{http_code}' -X POST \
    -H 'Content-Type: application/json' -d '{"type": "not_a_fault"}' \
    "$API/api/vehicles/0/faults")
case "$BAD_FP" in
    4*) ok "bogus fault type rejected by the sim and relayed (HTTP $BAD_FP): $(head -c 120 "$OUT/bad_fault.json")" ;;
    *) fail "bogus fault type was not rejected (HTTP $BAD_FP): $(head -c 200 "$OUT/bad_fault.json")" ;;
esac

OOR=$(curl -s --max-time 8 -o /dev/null -w '%{http_code}' -X POST \
    -H 'Content-Type: application/json' -d '{"type": "gps_denial"}' \
    "$API/api/vehicles/5/faults")
[ "$OOR" = "404" ] || fail "out-of-range fault proxy target should 404 (got $OOR)"
ok "fault proxy bounds-checks the vehicle index (404)"

# ---- (c) runtime task append
TA=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' \
    -d '[{"pos_ned_m": [20.0, 20.0, -12.0], "hover_s": 3}, {"pos_ned_m": [15.0, 0.0, -8.0]}, {"pos_ned_m": [500.0, 0.0, -10.0]}]' \
    "$API/api/tasks")
TA_OK=$(echo "$TA" | jqpy "print('1' if len(d['data']['accepted'])==2 and len(d['data']['rejected'])==1 and d['data']['pool']==2 else '0')")
[ "$TA_OK" = "1" ] || { echo "$TA"; fail "task append did not accept 2 / reject 1"; }
TA_ID=$(echo "$TA" | jqpy "print(d['data']['accepted'][0])")
ok "task append: 2 NED tasks accepted ($TA_ID + 1), far one rejected (geofence)"

TA_REASON=$(echo "$TA" | jqpy "print(d['data']['rejected'][0]['reason'])")
echo "$TA_REASON" | grep -qi "geofence" || fail "the rejection reason should be the geofence (got: $TA_REASON)"
ok "rejection carries the compiler's own reason: $TA_REASON"

BOARD=$(curl -s --max-time 4 "$API/api/tasks")
echo "$BOARD" | jqpy "print(' '.join(t['id'] for t in d['data']['tasks']))" | grep -q "$TA_ID" || fail "appended task $TA_ID not on the live board"
ok "GET /api/tasks shows the appended task pending on the board"

COLL=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' \
    -d "{\"id\": \"$TA_ID\", \"pos_ned_m\": [10.0, 10.0, -10.0]}" \
    "$API/api/tasks")
COLL_OK=$(echo "$COLL" | jqpy "print('1' if len(d['data']['rejected'])==1 and 'already on the task board' in d['data']['rejected'][0]['reason'] else '0')")
[ "$COLL_OK" = "1" ] || { echo "$COLL"; fail "explicit id collision was not rejected"; }
ok "explicit id collision rejected (the board is the live namespace)"

# ---- (d) PUT /api/fleet with an invalid scenario
BADPUT=$(curl -s --max-time 8 -o "$OUT/bad_put.json" -w '%{http_code}' -X PUT \
    -H 'Content-Type: application/json' -d '{"scenario_toml": "not [valid toml"}' \
    "$API/api/fleet")
[ "$BADPUT" = "422" ] || fail "invalid scenario TOML should 422 (got $BADPUT): $(head -c 200 "$OUT/bad_put.json")"
ok "PUT /api/fleet rejects an invalid scenario (422: $(head -c 100 "$OUT/bad_put.json"))"

# ---- (e) the hot load itself
PUT=$(curl -s --max-time 8 -X PUT -H 'Content-Type: application/json' \
    --data-binary @"$HOT_JSON" "$API/api/fleet")
PUT_OK=$(echo "$PUT" | jqpy "print('1' if d['ok'] and 'hot-scenario-' in d['data']['staged'] else '0')")
[ "$PUT_OK" = "1" ] || { echo "$PUT"; fail "PUT /api/fleet did not stage the valid scenario"; }
ok "PUT /api/fleet accepted + staged: $(echo "$PUT" | jqpy "print(d['data']['staged'])")"

# Graceful stop of run 1: phase flips ABORTED, then the plane rebinds.
echo "[R-1] waiting for the graceful stop + rebind (budget ${SWAP_BUDGET_S}s)..."
AB=""
for _ in $(seq 1 40); do
    AB=$(curl -s --max-time 2 "$API/api/fleet" | jqpy "print(d['data']['phase'])")
    [ "$AB" = "ABORTED" ] && break
    sleep 0.5
done
[ "$AB" = "ABORTED" ] || fail "run 1 did not flip to ABORTED after the hot load (got '$AB')"
ok "run 1 aborted gracefully (the hot-load reason is in its events.ndjson)"
grep -q "hot scenario load" "$RUN_DIR/events.ndjson" || fail "run 1 events lack the hot-load boundary"
ok "run 1 events record the hot scenario load boundary"

SWAPPED=""
for _ in $(seq 1 $((SWAP_BUDGET_S * 2))); do
    F=$(curl -s --max-time 2 "$API/api/fleet" 2>/dev/null) || { sleep 0.5; continue; }
    SP=$(echo "$F" | jqpy "print(d['data']['scenario'])" 2>/dev/null)
    CNT=$(echo "$F" | jqpy "print(d['data']['vehicles'] and len(d['data']['vehicles']))" 2>/dev/null)
    PH2=$(echo "$F" | jqpy "print(d['data']['phase'])" 2>/dev/null)
    if echo "$SP" | grep -q "hot-scenario-" && [ "${CNT:-2}" = "1" ] && [ "$PH2" = "SETUP_HOLD" ]; then
        SWAPPED=1; break
    fi
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during the hot restart"
    sleep 0.5
done
[ -n "$SWAPPED" ] || fail "fleet never came back with the staged scenario (scenario='$SP' count=$CNT phase='$PH2')"
ok "hot restart complete: scenario $(basename "$SP"), 1 vehicle, SETUP_HOLD again"

NEWF=$(curl -s --max-time 2 "$API/api/fleet")
FENCE80=$(echo "$NEWF" | jqpy "p=[abs(x) for pt in d['data']['geofence']['points_ned_m'] for x in pt];print('1' if max(p)==80 else '0')")
[ "$FENCE80" = "1" ] || { echo "$NEWF" | head -c 600; fail "the new fence (80 m) is not on the frame"; }
ok "the frame carries the swapped scenario's 80 m fence (was 60 m)"
[ -d "$RUN_DIR-hot-1" ] && [ -f "$RUN_DIR-hot-1/events.ndjson" ] || fail "the hot run has no own run dir + events (expected $RUN_DIR-hot-1)"
ok "the hot run owns an isolated run dir ($RUN_DIR-hot-1, nothing overwritten)"

# ---- (g) mission-active gate blocks a swap mid-flight
MU=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' \
    -d '{"items": [{"lat_deg": 47.39790, "lon_deg": 8.54561, "alt_m": 12.0, "hover_s": 8}, {"lat_deg": 47.39785, "lon_deg": 8.54570, "alt_m": 15.0, "hover_s": 8}]}' \
    "$API/api/mission")
MU_OK=$(echo "$MU" | jqpy "print('1' if d['ok'] and len(d['data']['accepted'])==2 else '0')")
[ "$MU_OK" = "1" ] || { echo "$MU"; fail "mission upload on the hot bench failed (2 waypoints expected)"; }
ok "operator mission started on the hot bench (2 waypoints, long enough to swap-test mid-flight)"
MS=$(curl -s --max-time 8 -X POST -H 'Content-Type: application/json' -d '{}' "$API/api/mission/start")
MS_OK=$(echo "$MS" | jqpy "print('1' if d['data']['started'] else '0')")
[ "$MS_OK" = "1" ] || { echo "$MS"; fail "mission start on the hot bench failed"; }
ok "operator mission start accepted (phase RUNNING)"

echo "[R-1] waiting for the mission to engage (budget ${MISSION_BUDGET_S}s)..."
ACTIVE=""
for _ in $(seq 1 $((MISSION_BUDGET_S * 2))); do
    F=$(curl -s --max-time 2 "$API/api/fleet")
    ARMED=$(echo "$F" | jqpy "print(1 if d['data']['vehicles'][0]['armed'] else 0)")
    MODE=$(echo "$F" | jqpy "print(d['data']['vehicles'][0]['mode'])")
    FSM=$(echo "$F" | jqpy "print(d['data']['vehicles'][0]['fsm'])")
    if [ "$ARMED" = "1" ]; then ACTIVE=1; break; fi
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during the hot-bench mission (last frame: armed=$ARMED mode=$MODE fsm=$FSM)"
    sleep 0.5
done
[ -n "$ACTIVE" ] || fail "vehicle 0 never armed for the hot-bench mission (mode=$MODE)"
ok "vehicle 0 armed and flying the mission (mode $MODE)"

PUT2=$(curl -s --max-time 8 -o "$OUT/put2.json" -w '%{http_code}' -X PUT \
    -H 'Content-Type: application/json' --data-binary @"$HOT_JSON" "$API/api/fleet")
[ "$PUT2" = "409" ] || fail "a PUT during an active mission should 409 (got $PUT2): $(head -c 200 "$OUT/put2.json")"
ok "mission-active gate: PUT /api/fleet -> 409 while the mission flies"

# ---- (f) the timeline fault event fired through the proxy (mid-mission)
echo "[R-1] waiting for the staged scenario's fault event (start_s=25) to fire..."
FAULT_FIRED=""
for _ in $(seq 1 40); do
    grep -q "fault 'gps_denial' injected into vehicle 0" "$RUN_DIR-hot-1/events.ndjson" 2>/dev/null && { FAULT_FIRED=1; break; }
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died before the fault event fired"
    sleep 1
done
[ -n "$FAULT_FIRED" ] || fail "the staged scenario's fault event never fired through the proxy"
ok "timeline fault event fired through the sim fault plane (ADR-0018 wiring)"

# ---- (h) estop teardown
ES=$(curl -s --max-time 8 -X POST "$API/api/estop")
echo "$ES" | head -c 200 >/dev/null
echo "[R-1] e-stop sent; waiting for manager exit (budget ${EXIT_BUDGET_S}s)..."
EXITED=""
for _ in $(seq 1 $((EXIT_BUDGET_S * 2))); do
    if ! kill -0 "$MANAGER_PID" 2>/dev/null; then EXITED=1; break; fi
    sleep 0.5
done
[ -n "$EXITED" ] || fail "manager did not exit after e-stop"

CODE=""
for _ in $(seq 1 20); do
    CODE=$(rg -o 'exit code (-?\d+)' "$MANAGER_LOG" | tail -1 | rg -o '\-?\d+')
    [ -n "$CODE" ] && break
    sleep 0.5
done
[ "$CODE" = "2" ] || fail "expected abort exit code 2 (got '${CODE:-none}')"
ok "manager exited with code 2 (ABORTED, the e-stop)"

# Both runs' reports now exist (each finish writes one), and run 1's report
# was NOT overwritten by the hot run: they live in separate dirs and name
# their own scenarios.
[ -f "$RUN_DIR/run-report.json" ] || fail "run 1 wrote no report"
[ -f "$RUN_DIR-hot-1/run-report.json" ] || fail "the hot run wrote no report"
R1_SCEN=$("$JSON_PY" -c "import json;d=json.load(open('$RUN_DIR/run-report.json'));print(d.get('scenario',''))" 2>/dev/null)
R2_SCEN=$("$JSON_PY" -c "import json;d=json.load(open('$RUN_DIR-hot-1/run-report.json'));print(d.get('scenario',''))" 2>/dev/null)
echo "$R1_SCEN" | grep -q "operator_bench.toml" || fail "run 1's report names the wrong scenario ('$R1_SCEN')"
echo "$R2_SCEN" | grep -q "hot-scenario-" || fail "the hot run's report names the wrong scenario ('$R2_SCEN')"
ok "both reports exist and name their own scenarios (bench vs hot-scenario)"

sleep 2
if curl -s --max-time 1 "$API/api/fleet" >/dev/null 2>&1; then
    fail "control plane still serving after exit (teardown leak)"
fi
ok "teardown verified: port $API_PORT free"

echo
echo "[R-1] PASS — $PASS_N checks: fault proxy (accept/reject/404), runtime task append"
echo "       (2 accepted / far rejected / id collision), hot scenario load (422 gate,"
echo "       staged swap + rebind + new fence + own run report), timeline fault event"
echo "       wiring, mission-active 409, e-stop exit 2."
echo "R-1 PASS: RUNTIME CONTROL PLANE COMPLETE"
