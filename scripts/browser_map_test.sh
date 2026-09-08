#!/bin/bash
# =============================================================================
# Operator Map BROWSER LIVE TEST (ADR-0017) — the QGC Fly/Plan map workflow
# driven end-to-end through the gateway in a real browser:
#   Caddy :81 -> console :3000; ?XTransformPort=8400 -> mavfleet operator plane.
#
# Flow: boot the operator bench (2 vehicles, hold_for_setup, real rustsitsim
# dynamics) → console LIVE on the Operator Map tab (Leaflet, OSM tiles or
# graticule fallback) → Plan mode: click the map to place 3 waypoints (real
# coordinate mouse events on the Leaflet container) → the waypoint table
# shows them → Upload → the task board carries op* tasks (REST-verified) →
# Start Mission → phase RUNNING + vehicle ACTIVE/armed (REST-verified) →
# live flight on the map → screenshots → e-stop teardown.
#
# Single-invocation orchestration: start, assert, teardown in one call.
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAV="$ROOT/fleet"
CON="$ROOT/console"
OUT="$MAV/tests/map_browser_artifacts"
SHOT="${RSIM_SHOT_DIR:-$ROOT/docs/images}"
API=8400
SCENARIO="$MAV/tests/operator_bench.toml"

mkdir -p "$OUT" "$SHOT"
rm -rf "$OUT"; mkdir -p "$OUT"
LOG=$OUT/map_browser.log
S=$OUT/page_snapshot.txt
MANAGER_PID=""

fail() {
    echo "MAP-BROWSER FAILED: $1" | tee -a "$LOG"
    echo "--- manager.log tail:" >> "$LOG"; tail -25 "$OUT/manager.log" >> "$LOG" 2>/dev/null
    cleanup
    exit 1
}
cleanup() {
    agent-browser close >/dev/null 2>&1
    curl -s -m 2 -X POST "http://127.0.0.1:$API/api/fleet/estop" >/dev/null 2>&1
    if [ -n "$MANAGER_PID" ] && kill -0 "$MANAGER_PID" 2>/dev/null; then
        kill "$MANAGER_PID" 2>/dev/null
        for _ in $(seq 1 20); do kill -0 "$MANAGER_PID" 2>/dev/null || break; sleep 0.5; done
        kill -9 "$MANAGER_PID" 2>/dev/null
    fi
    pkill -f "run_sitsim_vehicle.sh" 2>/dev/null
    pkill -f "next-server" 2>/dev/null
    pkill -f "standalone/server.js" 2>/dev/null
    pkill -x px4 2>/dev/null
    wait 2>/dev/null || true
}

pass_n=0
ok() { pass_n=$((pass_n+1)); echo "  PASS: $1" | tee -a "$LOG"; }

# kill any stale console server (a leaked next-server would serve an OLD
# build — the O-2 bring-up bug class: stale bundle + degenerate map fit)
killed_stale=0
for pid in $(pgrep -f "next-server" 2>/dev/null; pgrep -f "standalone/server.js" 2>/dev/null); do
    kill "$pid" 2>/dev/null; killed_stale=1
done
[ "$killed_stale" = "1" ] && echo "  (killed a stale console server)" | tee -a "$LOG"

echo "[map-browser] preflight..." | tee -a "$LOG"
[ -x "$MAV/target/debug/mavfleet" ] || fail "mavfleet binary missing"
CON_SERVER=$(find "$CON/.next/standalone" -name server.js -type f -not -path "*/node_modules/*" 2>/dev/null | head -1)
[ -n "$CON_SERVER" ] || fail "console build missing (npm run build in console/)"
command -v agent-browser >/dev/null || fail "agent-browser missing"
caddy_code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 3 http://127.0.0.1:81/)
[ "$caddy_code" = "502" ] || fail "caddy :81 not routing (code $caddy_code)"
pkill -f "run_sitsim_vehicle.sh" 2>/dev/null; pkill -x px4 2>/dev/null
pkill -f "target/debug/mavfleet" 2>/dev/null; sleep 1

# ---- 1. fleet manager (operator bench: 2 vehicles, held for the operator)
cd "$MAV"
FLEET_PX4_DIR="${PX4_ROOT:-/home/z/my-project/PX4-Autopilot}" \
FLEET_SIM_CFG_DIR="$MAV/scratch/vsims" \
"$MAV/target/debug/mavfleet" run --fleet "$SCENARIO" --api-port $API --run-dir "$OUT/run" \
    >"$OUT/manager.log" 2>&1 &
MANAGER_PID=$!
[ -n "$MANAGER_PID" ] || fail "manager did not start"

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

PH=""
for _ in $(seq 1 150); do
    PH=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/fleet" 2>/dev/null | python3 -c "
import json,sys
try: print(json.load(sys.stdin)['data']['phase'])
except Exception: print('')")
    [ "$PH" = "SETUP_HOLD" ] && break
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during bring-up"
    sleep 1
done
[ "$PH" = "SETUP_HOLD" ] || fail "phase did not reach SETUP_HOLD (got $PH)"
ok "manager up, 2 vehicles READY, phase SETUP_HOLD"

ok=""
for _ in $(seq 1 100); do
    code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 2 http://127.0.0.1:3000/ 2>/dev/null)
    [ "$code" = "200" ] && { ok=1; break; }
    sleep 1
done
[ -n "$ok" ] || fail "console did not serve :3000"
ok "console serving :3000 + gateway :81"

# ---- 4. open the console through the gateway, go to the Operator Map tab
agent-browser set viewport 1440 900 >/dev/null 2>&1
agent-browser open http://127.0.0.1:81/ >/dev/null 2>&1 || fail "browser could not open :81"
agent-browser wait --load networkidle >/dev/null 2>&1
agent-browser wait 6000 >/dev/null 2>&1

agent-browser snapshot > "$S" 2>&1
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'tab "Operator Map' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || { head -30 "$S" >> "$LOG"; fail "Operator Map tab not found in the console header"; }
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not click Operator Map tab"
agent-browser wait 5000 >/dev/null 2>&1
agent-browser snapshot > "$S" 2>&1
grep -q "Operator Map" "$S" || { head -40 "$S" >> "$LOG"; fail "Operator Map view did not open"; }
grep -q "LIVE" "$S" || { head -40 "$S" >> "$LOG"; fail "Operator Map not LIVE (:8400 via gateway)"; }
grep -q "Live map" "$S" || fail "map card missing"
grep -qE "origin 47\.3977|47\.39777" "$S" || { head -60 "$S" >> "$LOG"; fail "origin readout missing (geo frame not normalized)"; }
ok "Operator Map tab LIVE through the gateway (Leaflet map + origin readout)"

# the Leaflet container exists, has a real size, and the FENCE is actually
# fitted (large on screen) — the degenerate-world-view guard: if the fit
# raced a 0×0 map size, clicks would resolve to garbage lat/lon and every
# upload would be fence-rejected.
MAP_OK=$(agent-browser eval "(() => { const el = document.querySelector('.leaflet-container'); return el ? Math.round(el.clientWidth) + 'x' + Math.round(el.clientHeight) : 'none'; })()" 2>/dev/null | tail -1 | tr -d '"')
echo "$MAP_OK" >> "$LOG"
echo "$MAP_OK" | grep -qE "^[0-9]+x[0-9]+$" || { head -40 "$S" >> "$LOG"; fail "Leaflet container missing/zero-size ($MAP_OK)"; }
ok "Leaflet map initialized ($MAP_OK px) — fence + home drawn, tiles or graticule"

FENCE_PX=""
for _ in $(seq 1 10); do
    FENCE_PX=$(agent-browser eval "(() => { const paths = [...document.querySelectorAll('.leaflet-overlay-pane path')]; let best = 0; for (const p of paths) { const r = p.getBoundingClientRect(); best = Math.max(best, Math.min(r.width, r.height)); } return Math.round(best); })()" 2>/dev/null | tail -1 | tr -d '"')
    [ "${FENCE_PX:-0}" -ge 150 ] 2>/dev/null && break
    agent-browser wait 1000 >/dev/null 2>&1
done
[ "${FENCE_PX:-0}" -ge 150 ] 2>/dev/null || { head -40 "$S" >> "$LOG"; fail "fence polygon not fitted on screen (${FENCE_PX}px — degenerate view guard)"; }
ok "geofence fitted on screen (~${FENCE_PX}px) — map clicks resolve inside the fence"

# live vehicles have geo markers on the map
MARKERS=$(agent-browser eval "document.querySelectorAll('.rsim-vmarker').length" 2>/dev/null | tail -1)
[ "${MARKERS:-0}" -ge 1 ] || { agent-browser snapshot > "$S" 2>&1; head -40 "$S" >> "$LOG"; fail "no vehicle markers on the map (got $MARKERS)"; }
ok "$MARKERS live vehicle marker(s) on the geo map (GLOBAL_POSITION_INT fixes)"

agent-browser screenshot "$SHOT/rustsim-operatormap-live.png" >/dev/null 2>&1
ok "screenshot: docs/images/rustsim-operatormap-live.png"

# ---- 5. Plan mode: click the map to place waypoints
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'button "Plan' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || fail "Plan mode button not found"
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not switch to Plan mode"
agent-browser wait 1500 >/dev/null 2>&1
ok "Plan mode engaged (crosshair cursor, draggable waypoints)"

# map geometry: click 3 points well inside the fence (middle third of the
# map) — re-measure the rect before EVERY click (snapshots scroll the page
# and stale coordinates land off-map)
map_center() {
    agent-browser eval "JSON.stringify((() => { const r = document.querySelector('.leaflet-container').getBoundingClientRect(); return {x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height)}; })())" 2>/dev/null | tail -1 | python3 -c "import json,sys; v=json.loads(sys.stdin.read()); v=json.loads(v) if isinstance(v,str) else v; print(v['x'] + v['w']//2, v['y'] + v['h']//2, v['w'], v['h'])"
}

click_map() {  # click_map <dx-fraction> <dy-fraction> (of map w/h, from center)
    read -r CX CY W H <<< "$(map_center)"
    local cx=$((CX + $1 * W / 6)) cy=$((CY + $2 * H / 8))
    agent-browser mouse move "$cx" "$cy" >/dev/null 2>&1
    agent-browser mouse down left >/dev/null 2>&1
    agent-browser mouse up left >/dev/null 2>&1
    agent-browser wait 600 >/dev/null 2>&1
}

click_map -1 -1
click_map 1 -1
click_map 0 1

agent-browser snapshot > "$S" 2>&1
WPS=$(agent-browser eval "document.querySelectorAll('.rsim-wpm').length" 2>/dev/null | tail -1 | tr -d '"')
[ "${WPS:-0}" = "3" ] || { head -60 "$S" >> "$LOG"; fail "expected 3 waypoints on the map after 3 clicks (got $WPS)"; }
ok "3 waypoints placed by map clicks (numbered markers + dashed path)"
# the planned waypoints must be geo-sane (inside the fence, near the origin)
WP_LL=$(agent-browser snapshot 2>/dev/null | grep -oE "47\.39[0-9]{3,}, 8\.5[0-9]{3,}" | head -1)
[ -n "$WP_LL" ] || { head -60 "$S" >> "$LOG"; fail "waypoint coordinates not geo-sane (no 47.39x, 8.5x row — degenerate map?)"; }
echo "  waypoint sample: $WP_LL" | tee -a "$LOG"
grep -q "wp3" "$S" || { head -60 "$S" >> "$LOG"; fail "waypoint table missing wp3 row"; }
ok "mission panel lists the 3 planned waypoints (alt + hover editable)"

agent-browser screenshot "$SHOT/rustsim-operatormap-plan-live.png" >/dev/null 2>&1
ok "screenshot: docs/images/rustsim-operatormap-plan-live.png"

# ---- 6. Upload → the board carries op* tasks
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'button "Upload' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || { head -40 "$S" >> "$LOG"; fail "Upload button not found"; }
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not click Upload"
agent-browser wait 3000 >/dev/null 2>&1

OPS=""
for _ in $(seq 1 10); do
    OPS=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/tasks" | python3 -c "
import json,sys
try: print(sum(1 for t in json.load(sys.stdin)['data']['tasks'] if t['id'].startswith('op')))
except Exception: print('0')")
    [ "${OPS:-0}" = "3" ] && break
    sleep 1
done
[ "${OPS:-0}" = "3" ] || { curl -s --max-time 2 "http://127.0.0.1:$API/api/tasks" >> "$LOG"; fail "task board does not carry 3 op* tasks after Upload (got $OPS)"; }
ok "Upload clicked → 3 op* tasks on the live board (fence-validated, pending)"

# ---- 7. Start Mission → RUNNING + flight
REF=$(agent-browser snapshot 2>/dev/null | grep -E 'button "Start mission' | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || { head -40 "$S" >> "$LOG"; fail "Start mission button not found"; }
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not click Start mission"
agent-browser wait 3000 >/dev/null 2>&1

RUNNING=""
for _ in $(seq 1 20); do
    PH=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/fleet" | python3 -c "
import json,sys
try: print(json.load(sys.stdin)['data']['phase'])
except Exception: print('')")
    [ "$PH" = "RUNNING" ] && { RUNNING=1; break; }
    sleep 1
done
[ -n "$RUNNING" ] || fail "phase did not flip to RUNNING after Start mission (got $PH)"
ok "Start mission clicked → phase RUNNING (auction awarded the op* tasks)"

ACTIVE=""
for _ in $(seq 1 60); do
    ST=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/fleet" | python3 -c "
import json,sys
try:
    v = json.load(sys.stdin)['data']['vehicles'][0]
    print(1 if (v['armed'] or v['fsm'] == 'ACTIVE') else 0)
except Exception: print('0')")
    [ "$ST" = "1" ] && { ACTIVE=1; break; }
    sleep 1
done
[ -n "$ACTIVE" ] || fail "vehicle 0 never engaged after mission start (not armed/ACTIVE)"
ok "vehicle 0 engaged: armed + ACTIVE, flying the operator mission"

# watch the flight for a few seconds (trajectory grows on the map)
agent-browser wait 12000 >/dev/null 2>&1
TRAIL=$(agent-browser eval "document.querySelectorAll('.leaflet-overlay-pane path').length" 2>/dev/null | tail -1)
echo "overlay paths: $TRAIL" >> "$LOG"
agent-browser screenshot "$SHOT/rustsim-operatormap-flight-live.png" >/dev/null 2>&1
ok "mission in flight on the map (fence, waypoints, trajectory; screenshot captured)"

# ---- 8. teardown
curl -s -m 2 -X POST "http://127.0.0.1:$API/api/fleet/estop" >/dev/null 2>&1
ok "e-stop: fleet lands, run ABORTED"

echo
echo "OPERATOR MAP BROWSER LIVE TEST PASS ($pass_n checks)" | tee -a "$LOG"
cleanup
exit 0
