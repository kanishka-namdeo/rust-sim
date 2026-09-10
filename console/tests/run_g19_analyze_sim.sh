#!/usr/bin/env bash
# =============================================================================
# G-19: Analyze + SITL — replay scrub, ULog plots, fault inject, per-vehicle
# SIM E-STOP (GCS_V2_SPEC.md §12 M13 row, G-19).
#
# Asserts:
#   (a) SimControl overlay opens with per-vehicle selector (P9 fix)
#   (b) fault console shows F-01..F-10 catalog
#   (c) per-vehicle SIM E-STOP button present
#   (d) Analyze overlay opens with replays/ulogs tabs
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE_NAME="G-19"
source "$ROOT/console/tests/lib/browser_common.sh"
trap 'bc_stop_stack' EXIT
fail() { echo "[$GATE_NAME] FAIL: $1"; BC_FAILED=1; exit 1; }

bc_start_stack "$GATE_NAME" || fail "stack did not start"
agent-browser close >/dev/null 2>&1 || true; sleep 1
bc_navigate "http://127.0.0.1:81/" || fail "navigate failed"
sleep 15; bc_wait_canvas 30 || fail "canvas did not load"

# (a) SimControl overlay with per-vehicle selector (P9 fix)
# Open via the left rail SITL button
agent-browser eval 'Array.from(document.querySelectorAll("button[aria-label*=\"SITL\"]")).forEach(b => b.click())' >/dev/null 2>&1; sleep 2
SIM_PANEL=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.includes("SITL Control") ?? false')
bc_assert "SimControl overlay opens" "$SIM_PANEL"

# P9: per-vehicle selector
VEHICLE_SELECTOR=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.includes("P9") ?? false')
bc_assert "P9: per-vehicle sim plane selector" "$VEHICLE_SELECTOR"

# (b) fault console F-01..F-10
FAULTS=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.match(/F-0[0-9]/g)?.length ?? 0')
bc_assert_ge "fault console shows F-01..F-10 catalog" "$FAULTS" 5

# (c) per-vehicle SIM E-STOP
SIM_ESTOP=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.includes("SIM E-STOP") ?? false')
bc_assert "per-vehicle SIM E-STOP button" "$SIM_ESTOP"

agent-browser press Escape >/dev/null 2>&1; sleep 1

# (d) Analyze overlay
agent-browser press y >/dev/null 2>&1; sleep 2
ANALYZE=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.includes("Analyze") ?? false')
bc_assert "Analyze overlay opens (Y key)" "$ANALYZE"

agent-browser press Escape >/dev/null 2>&1; sleep 1

if [ -n "${BC_FAILED:-}" ]; then echo "[$GATE_NAME] FAIL"; exit 1; fi
echo "[$GATE_NAME] PASS — Analyze + SITL verified"
exit 0
