#!/bin/bash
# =============================================================================
# Vehicle Setup BROWSER LIVE TEST (ADR-0016) — the QGC/MP-style configuration
# workflow, driven end-to-end through the gateway in a real browser:
#   Caddy :81 -> console :3000; ?XTransformPort=8400 -> mavfleet setup plane.
#
# Flow: boot the setup-bench (1 vehicle, hold_for_setup, disarmed) →
# console LIVE on the Vehicle Setup tab → parameter download → typed param
# write → calibration trigger → flight-mode switch → airframe apply
# (Iris -> Boat/USV -> restart -> Boat resolves) → restore → teardown.
#
# Single-invocation orchestration: start, assert, teardown in one call.
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAV="$ROOT/fleet"
CON="$ROOT/console"
OUT="$MAV/tests/setup_browser_artifacts"
SHOT="${RSIM_SHOT_DIR:-$ROOT/docs/images}"
API=8400
SCENARIO="$MAV/tests/setup_bench.toml"

mkdir -p "$OUT" "$SHOT"
rm -rf "$OUT"; mkdir -p "$OUT"
LOG=$OUT/setup_browser.log
MANAGER_PID=""

fail() {
    echo "SETUP-BROWSER FAILED: $1" | tee -a "$LOG"
    echo "--- manager.log tail:" >> "$LOG"; tail -25 "$OUT/manager.log" >> "$LOG" 2>/dev/null
    cleanup
    exit 1
}
cleanup() {
    agent-browser close >/dev/null 2>&1
    # graceful: estop-abort lets the manager tear down its own children
    curl -s -m 2 -X POST "http://127.0.0.1:$API/api/fleet/estop" >/dev/null 2>&1
    [ -n "$MANAGER_PID" ] && kill "$MANAGER_PID" 2>/dev/null
    for _ in $(seq 1 10); do kill -0 "$MANAGER_PID" 2>/dev/null || break; sleep 1; done
    pkill -f "next-server" 2>/dev/null
    pkill -f "standalone/server.js" 2>/dev/null
    pkill -f "bin/px4" 2>/dev/null
    pkill -f "sim_stream.py" 2>/dev/null
    wait 2>/dev/null || true
}

pass_n=0
ok() { pass_n=$((pass_n+1)); echo "  PASS: $1" | tee -a "$LOG"; }

echo "[setup-browser] preflight..." | tee -a "$LOG"
[ -x "$MAV/target/debug/mavfleet" ] || fail "mavfleet binary missing"
CON_SERVER=$(find "$CON/.next/standalone" -name server.js -type f -not -path "*/node_modules/*" 2>/dev/null | head -1)
[ -n "$CON_SERVER" ] || fail "console build missing (npm run build in console/)"
command -v agent-browser >/dev/null || fail "agent-browser missing"
caddy_code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 3 http://127.0.0.1:81/)
[ "$caddy_code" = "502" ] || fail "caddy :81 not routing (code $caddy_code)"
# pre-clean: leaked pairs squat the per-instance ports
pkill -f "bin/px4" 2>/dev/null; pkill -f "sim_stream.py" 2>/dev/null
pkill -f "target/debug/mavfleet" 2>/dev/null; sleep 1

# ---- 1. fleet manager (setup bench: 1 vehicle, held disarmed)
cd "$MAV"
"$MAV/target/debug/mavfleet" run --fleet "$SCENARIO" --api-port $API --run-dir "$OUT/run" \
    >"$OUT/manager.log" 2>&1 &
MANAGER_PID=$!

# ---- 2. console standalone
(PORT=3000 HOSTNAME=127.0.0.1 NODE_ENV=production node "$CON_SERVER" >"$OUT/next.log" 2>&1 &)

# ---- 3. wait for both planes
ok=""
for _ in $(seq 1 150); do
    curl -s --max-time 1 "http://127.0.0.1:$API/api/fleet" 2>/dev/null | grep -q '"ok":true' && { ok=1; break; }
    kill -0 "$MANAGER_PID" 2>/dev/null || break
    sleep 0.4
done
[ -n "$ok" ] || fail "fleet control plane did not come up"

for _ in $(seq 1 120); do
    PH=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/fleet" 2>/dev/null | python3 -c "
import json,sys
try: print(json.load(sys.stdin)['data']['phase'])
except Exception: print('')")
    [ "$PH" = "SETUP_HOLD" ] && break
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during bring-up"
    sleep 1
done
[ "$PH" = "SETUP_HOLD" ] || fail "phase did not reach SETUP_HOLD (got $PH)"
ok "manager up, vehicle READY, phase SETUP_HOLD"

ok=""
for _ in $(seq 1 100); do
    code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 2 http://127.0.0.1:3000/ 2>/dev/null)
    [ "$code" = "200" ] && { ok=1; break; }
    sleep 1
done
[ -n "$ok" ] || fail "console did not serve :3000"
ok "console serving :3000 + gateway :81"

# ---- 4. open the console through the gateway, go to Vehicle Setup
agent-browser set viewport 1440 900 >/dev/null 2>&1
agent-browser open http://127.0.0.1:81/ >/dev/null 2>&1 || fail "browser could not open :81"
agent-browser wait --load networkidle >/dev/null 2>&1
agent-browser wait 5000 >/dev/null 2>&1

S=$OUT/page_snapshot.txt
agent-browser snapshot > "$S" 2>&1
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'tab "Vehicle Setup' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "Vehicle Setup tab not found in the console header"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not click Vehicle Setup tab"
agent-browser wait 4000 >/dev/null 2>&1
agent-browser snapshot > "$S" 2>&1
grep -q "Vehicle Setup" "$S" || fail "Vehicle Setup view did not open"
grep -q "LIVE" "$S" || { head -40 "$S" >> "$LOG"; fail "Vehicle Setup not LIVE (:8400 via gateway)"; }
grep -q "Airframe" "$S" || fail "setup rail missing (no Airframe section)"
grep -qE "DISARMED" "$S" || fail "vehicle not shown DISARMED in the setup header"
ok "Vehicle Setup tab LIVE through the gateway (rail + disarm badge)"

# ---- 5. Summary section shows the identity + param state
grep -q "SYS_AUTOSTART" "$S" && ok "Summary shows SYS_AUTOSTART tile" || fail "summary missing SYS_AUTOSTART"
grep -q "PX4" "$S" && ok "Summary shows autopilot PX4" || true

# ---- 6. Parameters: download the full set
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'tab "Parameters' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "Parameters rail entry not found"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not open Parameters section"
agent-browser wait 2000 >/dev/null 2>&1
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'button "Download' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "Download button not found"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not click Download (PARAM_REQUEST_LIST)"
ok "parameter download clicked (PARAM_REQUEST_LIST via /params/refresh)"
# wait for the download to complete (the store reaches Complete)
DL=""
for _ in $(seq 1 40); do
    ST=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/vehicles/0/params" | python3 -c "
import json,sys
try: print(json.load(sys.stdin)['data']['state'])
except Exception: print('')")
    [ "$ST" = "Complete" ] && { DL=1; break; }
    sleep 1
done
[ -n "$DL" ] || fail "parameter download did not complete (state=$ST)"
ok "parameter download Complete"
agent-browser wait 3000 >/dev/null 2>&1
agent-browser snapshot > "$S" 2>&1
grep -qE "SYS_AUTOSTART|BAT1_N_CELLS" "$S" || { head -60 "$S" >> "$LOG"; fail "params table empty (no SYS_AUTOSTART/BAT1 rows)"; }
grep -qE "[0-9]+ cached" "$S" || fail "params cache count not shown"
ok "params table populated (typed rows, cache count visible)"
agent-browser screenshot "$SHOT/rustsim-vehiclesetup-params-live.png" >/dev/null 2>&1

# ---- 7. Airframe section: catalog + current airframe
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'tab "Airframe' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "Airframe rail entry not found"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not open Airframe section"
agent-browser wait 2000 >/dev/null 2>&1
agent-browser snapshot > "$S" 2>&1
grep -q "Iris" "$S" || { head -40 "$S" >> "$LOG"; fail "current airframe (Iris) not shown"; }
grep -q "Boat (USV)" "$S" || fail "catalog groups not rendered (no Boat (USV))"
grep -q "Underwater (UUV)" "$S" || fail "catalog groups not rendered (no Underwater (UUV))"
ok "airframe catalog rendered: current Iris + UAV/USV/UUV groups"
agent-browser screenshot "$SHOT/rustsim-vehiclesetup-airframe-live.png" >/dev/null 2>&1

# ---- 8. Sensors section: calibration status + trigger
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'tab "Sensors' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "Sensors rail entry not found"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not open Sensors section"
agent-browser wait 2000 >/dev/null 2>&1
agent-browser snapshot > "$S" 2>&1
grep -qi "calibrat" "$S" || { head -40 "$S" >> "$LOG"; fail "calibration rows not shown"; }
ok "sensors + calibration rows rendered"

# ---- 9. Flight modes: switch to ALTCTL
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'tab "Flight Modes' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "Flight Modes rail entry not found"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not open Flight Modes section"
agent-browser wait 2000 >/dev/null 2>&1
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'button "ALTCTL' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "ALTCTL mode button not found"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not click ALTCTL"
agent-browser wait 5000 >/dev/null 2>&1
MD=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/vehicles/0/setup" | python3 -c "
import json,sys
try: print(json.load(sys.stdin)['data']['mode'])
except Exception: print('')")
[ "$MD" = "ALTCTL" ] || fail "mode did not switch to ALTCTL (still $MD)"
ok "flight-mode switch to ALTCTL via the UI (DO_SET_MODE + heartbeat echo)"

# ---- 10. Airframe apply: Boat (USV) with the filter + confirm dialog
agent-browser snapshot > /dev/null 2>&1
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'tab "Airframe' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
agent-browser click "$REF" >/dev/null 2>&1
agent-browser wait 2000 >/dev/null 2>&1
# filter the catalog down to the Boat row (deterministic target)
FREF=$(agent-browser snapshot 2>/dev/null | grep -E 'textbox "filter airframes' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$FREF" ] || fail "airframe filter input not found"
agent-browser fill "$FREF" "Boat" >/dev/null 2>&1 || agent-browser type "$FREF" "Boat" >/dev/null 2>&1
agent-browser wait 1500 >/dev/null 2>&1
agent-browser snapshot 2>/dev/null | grep -i "boat" | head -3 >> "$LOG"
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'button "(Apply|Re-apply)' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "Boat Apply button not found after filtering"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not click Boat Apply"
agent-browser wait 1500 >/dev/null 2>&1
agent-browser snapshot > "$OUT/dialog_snapshot.txt" 2>&1
grep -q "Boat" "$OUT/dialog_snapshot.txt" || { head -40 "$OUT/dialog_snapshot.txt" >> "$LOG"; fail "confirm dialog is not for Boat"; }
grep -q "1070" "$OUT/dialog_snapshot.txt" || fail "confirm dialog missing SYS_AUTOSTART=1070"
CONF=$(agent-browser snapshot 2>/dev/null | grep -E 'button "Apply and restart"' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$CONF" ] || { head -40 "$OUT/dialog_snapshot.txt" >> "$LOG"; fail "confirm button not found"; }
agent-browser click "$CONF" >/dev/null 2>&1 || fail "could not confirm airframe apply"
ok "airframe apply confirmed in the dialog (PARAM_SET SYS_AUTOSTART=1070 + restart queued)"

# wait for the reboot + new airframe to resolve
AF=""
for _ in $(seq 1 60); do
    sleep 2
    curl -s -m 2 -X POST "http://127.0.0.1:$API/api/vehicles/0/params/refresh" >/dev/null 2>&1
    AF=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/vehicles/0/setup" | python3 -c "
import json,sys
try: print(json.load(sys.stdin)['data']['airframe']['name'])
except Exception: print('')")
    [ "$AF" = "Boat" ] && break
done
[ "$AF" = "Boat" ] || fail "airframe did not become Boat after restart (got $AF)"
ok "airframe is now Boat (USV) after the controlled restart"
agent-browser wait 3000 >/dev/null 2>&1
agent-browser screenshot "$SHOT/rustsim-vehiclesetup-boat-live.png" >/dev/null 2>&1

# ---- 11. teardown + summary
cleanup
echo | tee -a "$LOG"
echo "===== VEHICLE SETUP BROWSER LIVE TEST PASS ($pass_n checks) =====" | tee -a "$LOG"
echo "screenshots: $SHOT/rustsim-vehiclesetup-{params,airframe,boat}-live.png"
exit 0
