#!/bin/bash
# =============================================================================
# RustSim BROWSER LIVE TEST — the full stack, live, through the gateway
# (Caddy :81 -> console :3000; ?XTransformPort=<port> -> Rust backends).
#
# Prerequisites (see docs/OPERATIONS.md):
#   - sim + fleet binaries built (cargo build --workspace in each)
#   - PX4-Autopilot v1.16.2 built (PX4_ROOT, default ../PX4-Autopilot)
#   - console production build (cd console && npm install && npm run build)
#   - Caddy running with console/Caddyfile.example on :81
#   - agent-browser (https://github.com/vercel-labs/agent-browser) on PATH
#
# Single-invocation orchestration: start, assert, teardown in one call.
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAV="$ROOT/fleet"
CON="$ROOT/console"
PX4_ROOT="${PX4_ROOT:-$(cd "$ROOT/.." && pwd)/PX4-Autopilot}"
export PX4_ROOT
OUT="$MAV/tests/demo_artifacts"
SHOT="${RSIM_SHOT_DIR:-$ROOT/docs/images}"
SCENARIO="$MAV/tests/demo_live.toml"
API=8400

mkdir -p "$OUT" "$SHOT"
rm -rf "$OUT"; mkdir -p "$OUT"
LOG=$OUT/browser_live.log
MANAGER_PID=""

fail() {
    echo "BROWSER-LIVE FAILED: $1" | tee -a "$LOG"
    echo "--- manager.log tail:" >> "$LOG"; tail -25 "$OUT/manager.log" >> "$LOG" 2>/dev/null
    cleanup
    exit 1
}
cleanup() {
    agent-browser close >/dev/null 2>&1
    [ -n "$MANAGER_PID" ] && kill "$MANAGER_PID" 2>/dev/null
    pkill -f "sitsim-cli" 2>/dev/null
    pkill -x px4 2>/dev/null
    pkill -f "next dev" 2>/dev/null
    pkill -f "next-server" 2>/dev/null
    pkill -f "standalone/server.js" 2>/dev/null
    wait 2>/dev/null || true
}

echo "[live] preflight..." | tee -a "$LOG"
[ -x "$MAV/target/debug/mavfleet" ] || fail "mavfleet binary missing (cargo build --workspace in fleet/)"
[ -x "$ROOT/sim/target/debug/sitsim-cli" ] || fail "sitsim-cli binary missing (cargo build --workspace in sim/)"
[ -f "$SCENARIO" ] || fail "scenario missing: $SCENARIO"
# Locate the standalone entry: with an in-project node_modules it lands at
# .next/standalone/server.js; with a workspace-shared (symlinked) install
# Turbopack roots the trace at the workspace and nests it (see
# console/scripts/finish-standalone.mjs).
CON_SERVER=$(find "$CON/.next/standalone" -name server.js -type f -not -path "*/node_modules/*" 2>/dev/null | head -1)
[ -n "$CON_SERVER" ] || fail "console build missing (npm run build in console/)"
[ -x "$PX4_ROOT/build/px4_sitl_default/bin/px4" ] || fail "PX4 binary missing (PX4_ROOT=$PX4_ROOT)"
command -v agent-browser >/dev/null || fail "agent-browser missing"
for p in 8400 8200 8201 3000; do
    curl -s --max-time 1 "http://127.0.0.1:$p/" >/dev/null 2>&1 && fail "port $p busy (previous run?)"
done
# Gateway must be alive: :81 answers 502 (console down) — proof of routing.
caddy_code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 3 http://127.0.0.1:81/legacy)
[ "$caddy_code" = "502" ] || fail "caddy :81 not routing (code $caddy_code; run caddy with console/Caddyfile.example)"

# ---- 1. fleet manager (2 vehicles x real rustsitsim + PX4)
cd "$MAV"
"$MAV/target/debug/mavfleet" run --fleet "$SCENARIO" --api-port $API --run-dir "$OUT/run" \
    >"$OUT/manager.log" 2>&1 &
MANAGER_PID=$!
echo "[live] manager pid $MANAGER_PID" | tee -a "$LOG"

# ---- 2. console (production standalone server — ~150 MB, boots in ~2 s)
(PORT=3000 HOSTNAME=127.0.0.1 NODE_ENV=production node "$CON_SERVER" >"$OUT/next.log" 2>&1 &)

# ---- 3. wait for both planes
ok=""
for _ in $(seq 1 150); do
    if curl -s --max-time 1 "http://127.0.0.1:$API/api/fleet" 2>/dev/null | grep -q '"ok":true'; then ok=1; break; fi
    kill -0 "$MANAGER_PID" 2>/dev/null || break
    sleep 0.4
done
[ -n "$ok" ] || fail "fleet control plane did not come up"
echo "[live] fleet :8400 up" | tee -a "$LOG"

ok=""
for _ in $(seq 1 100); do
    code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 2 http://127.0.0.1:3000/ 2>/dev/null)
    [ "$code" = "200" ] && { ok=1; break; }
    sleep 1
done
[ -n "$ok" ] || fail "console did not serve :3000"
code81=$(curl -s -o /dev/null -w "%{http_code}" --max-time 3 http://127.0.0.1:81/legacy)
[ "$code81" = "200" ] || fail "gateway :81 did not serve the console (code $code81)"
echo "[live] console up: :3000 direct=200, :81 via gateway=$code81" | tee -a "$LOG"

ok=""
for _ in $(seq 1 200); do
    n=$(curl -s --max-time 2 "http://127.0.0.1:$API/api/fleet" 2>/dev/null | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)['data']
    print(sum(1 for v in d['vehicles'] if v['fsm'] not in ('INIT','SPAWNING','BOOTING')))
except Exception: print(0)")
    [ "$n" = "2" ] && { ok=1; break; }
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during bring-up (see manager.log)"
    sleep 1
done
[ -n "$ok" ] || fail "vehicles did not pass the boot gate"
echo "[live] both vehicles booted (px4 + sitsim pairs live)" | tee -a "$LOG"

# ---- 4. drive the browser through the GATEWAY
agent-browser set viewport 1440 900 >/dev/null 2>&1
agent-browser open http://127.0.0.1:81/legacy >/dev/null 2>&1 || fail "browser could not open :81"
agent-browser wait --load networkidle >/dev/null 2>&1
agent-browser wait 6000 >/dev/null 2>&1  # WS + first telemetry frames

# GCS v1 mounts 7 tabs and defaults to Plan; non-active tabpanels are
# CSS-hidden, so the Sim Console LIVE badge never shows in the initial
# snapshot. Click the tab first (same pattern as the Fleet C2 section).
SIM_REF=$(agent-browser snapshot -i 2>/dev/null | grep -i "Sim Console" | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$SIM_REF" ] || { agent-browser snapshot > "$OUT/sim_snapshot_0.txt" 2>&1; head -40 "$OUT/sim_snapshot_0.txt" >> "$LOG"; fail "Sim Console tab not found"; }
agent-browser click "$SIM_REF" >/dev/null 2>&1 || fail "could not click Sim Console tab"
agent-browser wait 4000 >/dev/null 2>&1  # tab panel render + first telemetry frames

S1=$OUT/sim_snapshot_1.txt
S2=$OUT/sim_snapshot_2.txt
agent-browser snapshot > "$S1" 2>&1
grep -q "LIVE" "$S1" || { head -40 "$S1" >> "$LOG"; fail "Sim Console did not reach LIVE (no LIVE badge)"; }
echo "[live] Sim Console is LIVE (badge :8200 via gateway WS)" | tee -a "$LOG"

agent-browser wait 12000 >/dev/null 2>&1
agent-browser snapshot > "$S2" 2>&1
if cmp -s "$S1" "$S2"; then
    fail "telemetry frozen: two snapshots 12 s apart are identical"
fi
echo "[live] Sim Console telemetry is flowing (snapshots differ)" | tee -a "$LOG"
agent-browser screenshot "$SHOT/rustsim-simconsole-live.png" >/dev/null 2>&1

# ---- 5. Fleet C2 tab
REF=$(agent-browser snapshot -i 2>/dev/null | grep -i "Fleet C2" | head -1 | rg -o 'ref=e[0-9]+' | head -1 | sed 's/ref=/@/')
[ -n "$REF" ] || { grep -i "tab\|Fleet" "$S1" | head -5 >> "$LOG"; fail "Fleet C2 tab not found"; }
agent-browser click "$REF" >/dev/null 2>&1 || fail "could not click Fleet C2 tab"
agent-browser wait 6000 >/dev/null 2>&1
S3=$OUT/fleet_snapshot.txt
agent-browser snapshot > "$S3" 2>&1
grep -q "LIVE" "$S3" || fail "Fleet C2 did not reach LIVE (:8400)"
grep -qE "OFFBOARD|POSCTL|AUTO\.RTL|RTL|LANDED" "$S3" || { head -40 "$S3" >> "$LOG"; fail "no vehicle modes visible in Fleet C2"; }
echo "[live] Fleet C2 is LIVE: vehicles + modes visible" | tee -a "$LOG"
agent-browser wait 10000 >/dev/null 2>&1
agent-browser screenshot "$SHOT/rustsim-fleetc2-live.png" >/dev/null 2>&1

# mission-progress evidence for the log
grep -oE "task '[a-z_]+' (start|complete)[^\"']*|offboard engaged[^\"']*|RTL|LANDED" "$OUT/manager.log" 2>/dev/null | tail -8 | tee -a "$LOG"

cleanup
echo
echo "===== BROWSER LIVE TEST PASS =====" | tee -a "$LOG"
echo "screenshots: $SHOT/rustsim-simconsole-live.png, $SHOT/rustsim-fleetc2-live.png"
exit 0
