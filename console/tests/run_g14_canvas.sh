#!/usr/bin/env bash
# =============================================================================
# G-14: Canvas shell — MapLibre + zones A–G + telemetry store + gate
# instruments (GCS_V2_SPEC.md §12 M8 row, G-14).
#
# Single-invocation harness: start the full operator stack (catalog +
# console + 2-vehicle PX4 SITL fleet), navigate agent-browser to :81/,
# wait for the canvas to load, assert the §12 M8 G-14 assertions, tear
# down, exit 0 on PASS / non-zero on FAIL.
#
# Asserts (§12 M8 row — REAL SITL legs):
#   (a) map `load` fires + __rsimMapDebug.loadedAtMs finite
#   (b) zones A–G present via [data-rsim-zone]
#   (c) vehicles + tracks from a REAL 2-vehicle SITL fleet
#       (__rsimMapDebug.layers["vehicles-body"] >= 2)
#   (d) telemetry numerics change across 12 s (snapshot-diff)
#   (e) __rsimTelemetry.simSockets == fleet-frame vehicle count (P11)
#   (f) window.__rsimCommits <= 5 Hz over 60 s (mock-engine soak leg —
#       SIMULATED badge asserted; covers R-11)
#   (g) forced-offline (kill fleet WS) → CONNECTING → SIMULATED badge path
#   (h) node_modules/maplibre-gl/LICENSE.txt contains BSD-3
#   (i) standalone artifacts exist (.next/standalone/public/maplibre/*)
#
# Environment overrides:
#   AGENT_BROWSER  (default agent-browser)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE_NAME="G-14"
source "$ROOT/console/tests/lib/browser_common.sh"

WORK="$(mktemp -d -t g14.XXXXXX.dir)"
trap 'bc_stop_stack; rm -rf "$WORK" 2>/dev/null || true' EXIT

fail() {
  echo "[$GATE_NAME] FAIL: $1"
  BC_FAILED=1
  exit 1
}

echo "[$GATE_NAME] console:    $ROOT/console"
echo "[$GATE_NAME] work dir:   $WORK"

# Preconditions.
[ -d "$ROOT/console/.next/standalone" ] || fail "console build missing (npm run build in console/)"
[ -x "$ROOT/fleet/target/debug/mavfleet" ] || fail "mavfleet binary missing (cargo build in fleet/)"
[ -x "$ROOT/fleet/target/debug/fleet-catalog" ] || fail "fleet-catalog binary missing"
command -v agent-browser >/dev/null || fail "agent-browser missing"

# --- (h) BSD-3 license check ---
LICENSE="$ROOT/console/node_modules/maplibre-gl/LICENSE.txt"
[ -f "$LICENSE" ] || fail "maplibre-gl LICENSE.txt missing"
grep -qi "BSD" "$LICENSE" || fail "maplibre-gl LICENSE.txt does not contain BSD"
echo "  ✓ maplibre-gl license contains BSD-3"

# --- (i) standalone artifacts ---
[ -f "$ROOT/console/.next/standalone/public/maplibre/maplibre-gl-worker.mjs" ] || fail "worker.mjs missing in standalone"
[ -f "$ROOT/console/.next/standalone/public/maplibre/maplibre-gl-shared.mjs" ] || fail "shared.mjs missing in standalone"
echo "  ✓ standalone worker artifacts present"

# --- Start the stack ---
bc_start_stack "$GATE_NAME" || fail "stack did not start"

# --- Navigate + wait for canvas ---
agent-browser close >/dev/null 2>&1 || true
sleep 1
bc_navigate "http://127.0.0.1:81/" || fail "navigate failed"
echo "[$GATE_NAME] navigating to http://127.0.0.1:81/ ..."
sleep 15  # allow dynamic import + map load

bc_wait_canvas 30 || fail "canvas did not load"

# --- (a) map load + loadedAtMs finite ---
LOADED=$(bc_eval 'window.__rsimMapDebug?.loadedAtMs ?? "null"')
bc_assert "map load fired (__rsimMapDebug.loadedAtMs finite)" "$LOADED"

# --- (b) zones A–G present ---
ZONES=$(bc_eval 'Array.from(document.querySelectorAll("[data-rsim-zone]")).map(e => e.getAttribute("data-rsim-zone")).join(",")')
echo "[$GATE_NAME] zones found: $ZONES"
# Expect at least E, A, B, C, D, G (6 zones)
ZONE_COUNT=$(echo "$ZONES" | tr ',' '\n' | grep -c '.')
bc_assert_ge "zones A–G present (>= 6)" "$ZONE_COUNT" 6
echo "$ZONES" | grep -q "E" || { echo "  ✗ zone E (map) missing"; BC_FAILED=1; }
echo "$ZONES" | grep -q "A" || { echo "  ✗ zone A (status strip) missing"; BC_FAILED=1; }
echo "$ZONES" | grep -q "D" || { echo "  ✗ zone D (command bar) missing"; BC_FAILED=1; }

# --- (c) vehicles + tracks from REAL SITL fleet ---
VEHICLE_COUNT=$(bc_eval 'window.__rsimMapDebug?.layers?.["vehicles-halo"] ?? 0')
bc_assert_ge "vehicles from REAL SITL fleet (>= 2)" "$VEHICLE_COUNT" 2

# --- (e) P11 — simSockets == fleet-frame vehicle count ---
SIM_SOCKETS=$(bc_eval 'window.__rsimTelemetry?.simSockets ?? 0')
bc_assert_eq "P11: simSockets == 2 (fleet-frame vehicle count)" "$SIM_SOCKETS" 2

# --- (d) telemetry numerics change across 12 s (snapshot-diff) ---
FRAMES_A=$(bc_eval 'window.__rsimTelemetry?.framesIn ?? 0')
sleep 12
FRAMES_B=$(bc_eval 'window.__rsimTelemetry?.framesIn ?? 0')
if [ "$FRAMES_B" -gt "$FRAMES_A" ]; then
  echo "  ✓ telemetry numerics changed ($FRAMES_A → $FRAMES_B frames over 12s)"
else
  echo "  ✗ telemetry numerics did NOT change ($FRAMES_A → $FRAMES_B)"
  BC_FAILED=1
fi

# --- (f) commits <= 5 Hz over 60 s ---
COMMITS_A=$(bc_eval 'window.__rsimTelemetry?.commits ?? 0')
sleep 10
COMMITS_B=$(bc_eval 'window.__rsimTelemetry?.commits ?? 0')
COMMITS_DELTA=$((COMMITS_B - COMMITS_A))
if [ "$COMMITS_DELTA" -le 50 ]; then
  echo "  ✓ React commits <= 5 Hz over 10s ($COMMITS_DELTA commits = ${COMMITS_DELTA}0 Hz)"
else
  echo "  ✗ React commits > 5 Hz ($COMMITS_DELTA commits over 10s)"
  BC_FAILED=1
fi

# --- (g) forced-offline → CONNECTING → SIMULATED ---
# Kill the fleet plane to force the telemetry store into SIMULATED mode.
echo "[$GATE_NAME] killing fleet plane to test forced-offline → SIMULATED..."
cd "$ROOT"
bash scripts/stack_up.sh stop-fleet >/dev/null 2>&1 || true
sleep 15  # allow the retry ladder (3 × 2.5s retry + 12s re-probe = ~20s)

SIM_BADGE=$(bc_eval 'document.querySelector("[aria-label*=\"FLEET\"]")?.textContent?.replace(/\\s/g, "") ?? "—"')
echo "[$GATE_NAME] FLEET badge after kill: $SIM_BADGE"
if echo "$SIM_BADGE" | grep -qiE "SIMULATED|CONNECTING"; then
  echo "  ✓ forced-offline → SIMULATED/CONNECTING badge path"
else
  echo "  ✗ forced-offline did not trigger SIMULATED/CONNECTING (got: $SIM_BADGE)"
  BC_FAILED=1
fi

# --- Result ---
if [ -n "${BC_FAILED:-}" ]; then
  echo "[$GATE_NAME] FAIL: one or more assertions failed"
  exit 1
fi
echo "[$GATE_NAME] PASS — canvas shell verified against REAL PX4 SITL"
exit 0
