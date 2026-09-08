#!/usr/bin/env bash
# ADR-0016 live end-to-end test: the vehicle-setup control plane against
# real PX4 SITL (single-shot: the sandbox reaps background processes
# between tool calls, so start → test → teardown happens in one run).
#
# Usage: bash fleet/scripts/live_test_setup.sh
set -u
FLEET_DIR="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
API=http://127.0.0.1:8400
LOG=/tmp/mavfleet-setup.log
RUN_DIR="$FLEET_DIR/fleet-runs/setup-bench"

pass=0; fail=0
ok()   { pass=$((pass+1)); echo "  PASS: $1"; }
bad()  { fail=$((fail+1)); echo "  FAIL: $1"; }
check() { # name expected actual
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected '$2', got '$3')"; fi
}
contains() { # name haystack needle
  if echo "$2" | grep -q "$3"; then ok "$1"; else bad "$1 (missing: $3)"; fi
}

jsonq() { python3 -c "import json,sys;d=json.load(sys.stdin);print(d$1)" 2>/dev/null; }

echo "== [0] build + start mavfleet (setup_bench.toml) =="
cd "$FLEET_DIR"
# pre-clean: orphans from an earlier run squat the per-instance ports and
# starve the new sim's TCP bind (process death -> FAULT -> abort)
pkill -f "bin/px4" 2>/dev/null; pkill -f "sim_stream.py" 2>/dev/null
pkill -f "target/debug/mavfleet" 2>/dev/null; sleep 1
rm -rf "$RUN_DIR"
cargo build -q 2>/dev/null
setsid nohup target/debug/mavfleet run --fleet tests/setup_bench.toml \
  --run-dir "$RUN_DIR" --api-port 8400 > "$LOG" 2>&1 < /dev/null &
MF_PID=$!

# wait for control plane + vehicle READY
for i in $(seq 1 60); do
  PHASE=$(curl -s -m 2 $API/api/fleet | jsonq "['data']['phase']" 2>/dev/null)
  [ "$PHASE" = "SETUP_HOLD" ] && break
  sleep 1
done
check "phase reaches SETUP_HOLD" "SETUP_HOLD" "$PHASE"
contains "vehicle 0 READY logged" "$(tail -20 $LOG)" "vehicle 0 READY"

echo "== [1] catalog endpoints =="
AF=$(curl -s $API/api/airframes)
COUNT=$(echo "$AF" | jsonq "['data']['count']")
[ "${COUNT:-0}" -gt 100 ] 2>/dev/null && ok "airframe catalog >100 frames ($COUNT)" || bad "catalog count $COUNT"
echo "$AF" | grep -q "Boat (USV)"      && ok "catalog has Boat (USV) group"      || bad "no USV group"
echo "$AF" | grep -q "Underwater (UUV)" && ok "catalog has Underwater (UUV) group" || bad "no UUV group"
echo "$AF" | grep -q "Multirotor (UAV)" && ok "catalog has Multirotor (UAV) group" || bad "no UAV group"

MODES=$(curl -s $API/api/modes)
contains "modes include AUTO.RTL" "$MODES" "AUTO.RTL"
contains "modes include OFFBOARD" "$MODES" "OFFBOARD"

echo "== [2] setup summary before param download =="
S0=$(curl -s $API/api/vehicles/0/setup)
check "summary index/sysid" "0/1" "$(echo "$S0" | jsonq "['data']['index']")/$(echo "$S0" | jsonq "['data']['sysid']")"
check "summary armed=false" "False" "$(echo "$S0" | jsonq "['data']['armed']")"
check "params section Idle pre-download" "Idle" "$(echo "$S0" | jsonq "['data']['params']['state']")"
check "airframe unknown pre-download" "Unknown airframe" "$(echo "$S0" | jsonq "['data']['airframe']['name']")"

echo "== [3] full parameter download (PARAM_REQUEST_LIST) =="
R=$(curl -s -X POST $API/api/vehicles/0/params/refresh)
check "refresh requested=true" "True" "$(echo "$R" | jsonq "['data']['requested']")"
STATE="Downloading"; TOTAL=0; RECV=0
for i in $(seq 1 45); do
  PJ=$(curl -s $API/api/vehicles/0/params)
  STATE=$(echo "$PJ" | jsonq "['data']['state']")
  RECV=$(echo "$PJ" | jsonq "['data']['received']")
  TOTAL=$(echo "$PJ" | jsonq "['data']['total']")
  [ "$STATE" = "Complete" ] && break
  sleep 1
done
check "download completes" "Complete" "$STATE"
echo "    received $RECV/$TOTAL params"
[ "${RECV:-0}" -ge 700 ] 2>/dev/null && ok "received >=700 params" || bad "only $RECV params"

echo "== [4] setup summary after download =="
S1=$(curl -s $API/api/vehicles/0/setup)
AF_NAME=$(echo "$S1" | jsonq "['data']['airframe']['name']")
contains "airframe resolves (Iris SITL)" "$AF_NAME" "Iris"
SA=$(echo "$S1" | jsonq "['data']['airframe']['sys_autostart']")
[ "$SA" = "10015" ] && ok "SYS_AUTOSTART=10015 pre-change" || bad "SYS_AUTOSTART=$SA"
check "dynamics_compatible=true (quad)" "True" "$(echo "$S1" | jsonq "['data']['airframe']['dynamics_compatible']")"
contains "power section has BAT1_N_CELLS" "$S1" "BAT1_N_CELLS"
contains "safety section has NAV_DLL_ACT" "$S1" "NAV_DLL_ACT"

echo "== [5] param write: BAT_N_CELLS (PARAM_SET + echo) =="
W=$(curl -s -X POST -H 'Content-Type: application/json' \
  -d '{"id":"BAT1_N_CELLS","value":6}' $API/api/vehicles/0/params)
check "write confirmed=6" "6" "$(echo "$W" | jsonq "['data']['confirmed']")"
PV=$(curl -s $API/api/vehicles/0/params | python3 -c "
import json,sys
d=json.load(sys.stdin)['data']['params']
print([p['value'] for p in d if p['id']=='BAT1_N_CELLS'][0])")
check "cache reflects BAT1_N_CELLS=6" "6" "$PV"

echo "== [6] restore BAT_N_CELLS=4 =="
W2=$(curl -s -X POST -H 'Content-Type: application/json' \
  -d '{"id":"BAT1_N_CELLS","value":4}' $API/api/vehicles/0/params)
check "restore confirmed=4" "4" "$(echo "$W2" | jsonq "['data']['confirmed']")"

echo "== [7] sensor calibration: gyro (MAV_CMD 241 p1=1) =="
C=$(curl -s -X POST -H 'Content-Type: application/json' \
  -d '{"sensor":"gyro"}' $API/api/vehicles/0/calibrate)
check "gyro calibration accepted" "True" "$(echo "$C" | jsonq "['data']['accepted']")"
check "calibration command id" "241" "$(echo "$C" | jsonq "['data']['command']")"

echo "== [8] flight-mode switch: ALTCTL =="
M=$(curl -s -X POST -H 'Content-Type: application/json' \
  -d '{"mode":"ALTCTL"}' $API/api/vehicles/0/mode)
check "mode ALTCTL accepted" "True" "$(echo "$M" | jsonq "['data']['accepted']")"
sleep 2
MODE_NOW=$(curl -s $API/api/vehicles/0/setup | jsonq "['data']['mode']")
check "mode echo in summary" "ALTCTL" "$MODE_NOW"
# back to POSCTL for consistency
curl -s -X POST -H 'Content-Type: application/json' -d '{"mode":"POSCTL"}' $API/api/vehicles/0/mode > /dev/null

echo "== [9] error paths =="
E1=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
  -d '{"sys_autostart":999999}' $API/api/vehicles/0/airframe)
check "unknown airframe -> 422" "422" "$E1"
E2=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
  -d '{"mode":"TURBO"}' $API/api/vehicles/0/mode)
check "unknown mode -> 422" "422" "$E2"
E3=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
  -d '{"sensor":"warp"}' $API/api/vehicles/0/calibrate)
check "unknown sensor -> 422" "422" "$E3"
E4=$(curl -s -o /dev/null -w '%{http_code}' $API/api/vehicles/9/setup)
check "out of range setup -> 404" "404" "$E4"

echo "== [10] airframe apply: Boat (USV, SYS_AUTOSTART=1070) + controlled restart =="
A=$(curl -s -X POST -H 'Content-Type: application/json' \
  -d '{"sys_autostart":1070}' $API/api/vehicles/0/airframe)
check "airframe write confirmed" "True" "$(echo "$A" | jsonq "['data']['write_confirmed']")"
check "restart queued" "True" "$(echo "$A" | jsonq "['data']['restart_queued']")"

# wait for the respawn + boot (settle 1.5s + px4 boot ~8s + margin)
echo "    waiting for respawn + reboot…"
NEW_AF=""
for i in $(seq 1 60); do
  sleep 2
  curl -s -X POST $API/api/vehicles/0/params/refresh > /dev/null
  PJ=$(curl -s $API/api/vehicles/0/params)
  ST=$(echo "$PJ" | jsonq "['data']['state']")
  RV=$(echo "$PJ" | jsonq "['data']['received']")
  if [ "$ST" = "Complete" ] && [ "${RV:-0}" -gt 100 ] 2>/dev/null; then
    NEW_AF=$(curl -s $API/api/vehicles/0/setup | jsonq "['data']['airframe']['name']")
    [ "$NEW_AF" = "Boat" ] && break
  fi
done
check "airframe now Boat (USV)" "Boat" "$NEW_AF"
S2=$(curl -s $API/api/vehicles/0/setup)
check "dynamics_compatible=false for Boat" "False" "$(echo "$S2" | jsonq "['data']['airframe']['dynamics_compatible']")"
contains "restart logged in event log" "$(curl -s "$API/api/events?tail=500")" "vehicle restart"
contains "manager respawned the pair" "$(cat $LOG)" "restarted (airframe apply"

echo "== [11] restore Iris (SYS_AUTOSTART=10015) =="
A2=$(curl -s -X POST -H 'Content-Type: application/json' \
  -d '{"sys_autostart":10015}' $API/api/vehicles/0/airframe)
check "restore write confirmed" "True" "$(echo "$A2" | jsonq "['data']['write_confirmed']")"
REST=""
for i in $(seq 1 60); do
  sleep 2
  curl -s -X POST $API/api/vehicles/0/params/refresh > /dev/null
  PJ=$(curl -s $API/api/vehicles/0/params)
  ST=$(echo "$PJ" | jsonq "['data']['state']")
  RV=$(echo "$PJ" | jsonq "['data']['received']")
  if [ "$ST" = "Complete" ] && [ "${RV:-0}" -gt 100 ] 2>/dev/null; then
    REST=$(curl -s $API/api/vehicles/0/setup | jsonq "['data']['airframe']['name']")
    [ "$REST" = "3DR Iris Quadrotor SITL" ] && break
  fi
done
check "airframe restored to Iris" "3DR Iris Quadrotor SITL" "$REST"
check "vehicle still not armed" "False" "$(curl -s $API/api/vehicles/0/setup | jsonq "['data']['armed']")"

echo "== [12] params survived the double restart (autosave proof) =="
SA2=$(curl -s $API/api/vehicles/0/params | python3 -c "
import json,sys
d=json.load(sys.stdin)['data']['params']
print([p['value'] for p in d if p['id']=='SYS_AUTOSTART'][0])")
check "SYS_AUTOSTART persisted as 10015" "10015" "$SA2"
BN=$(curl -s $API/api/vehicles/0/params | python3 -c "
import json,sys
d=json.load(sys.stdin)['data']['params']
print([p['value'] for p in d if p['id']=='BAT1_N_CELLS'][0])")
check "BAT1_N_CELLS restore=4 persisted" "4" "$BN"

echo "== [13] teardown (graceful: estop-abort -> finish() -> kill_all) =="
curl -s -X POST $API/api/fleet/estop > /dev/null
for i in $(seq 1 20); do pgrep -f "target/debug/mavfleet" >/dev/null 2>&1 || break; sleep 1; done
pgrep -f "target/debug/mavfleet" >/dev/null 2>&1 && { kill $MF_PID 2>/dev/null; sleep 2; } || true
sleep 1
pgrep -f "bin/px4" >/dev/null 2>&1 && bad "px4 processes leaked" || ok "px4 processes reaped"
pgrep -f "sim_stream.py" >/dev/null 2>&1 && bad "sim processes leaked" || ok "sim processes reaped"

echo
echo "=================================="
echo "LIVE SETUP TEST: $pass passed, $fail failed"
echo "=================================="
exit $([ $fail -eq 0 ] && echo 0 || echo 1)
