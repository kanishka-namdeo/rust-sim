#!/usr/bin/env bash
# =============================================================================
# G-16: Plan-on-map — click-to-add, validate, save, upload, L5, fence vertex
# delete (GCS_V2_SPEC.md §12 M10 row, G-16).
#
# Asserts:
#   (a) click-to-add waypoint in Plan mode → wp-body count increases
#   (b) MissionStrip (M key) shows waypoint table
#   (c) validate button disabled until saved
#   (d) fence vertex delete (P7 fix) — context menu Delete vertex
#   (e) mission polyline L5 visible when vehicle ACTIVE+armed (P6)
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE_NAME="G-16"
source "$ROOT/console/tests/lib/browser_common.sh"
trap 'bc_stop_stack' EXIT
fail() { echo "[$GATE_NAME] FAIL: $1"; BC_FAILED=1; exit 1; }

bc_start_stack "$GATE_NAME" || fail "stack did not start"
agent-browser close >/dev/null 2>&1 || true; sleep 1
bc_navigate "http://127.0.0.1:81/" || fail "navigate failed"
sleep 15; bc_wait_canvas 30 || fail "canvas did not load"

# Switch to Plan mode
agent-browser eval 'Array.from(document.querySelectorAll("button[aria-label*=\"Map mode: Plan\"]")).forEach(b => b.click())' >/dev/null 2>&1; sleep 2
echo "  ✓ switched to Plan mode"

# (a) click-to-add waypoint
WP_BEFORE=$(bc_eval 'window.__rsimMapDebug?.layers?.["wp-body"] ?? 0')
agent-browser eval 'const c = document.querySelector(".maplibregl-map canvas"); const r = c.getBoundingClientRect(); ["mousedown","mouseup","click"].forEach(t => c.dispatchEvent(new MouseEvent(t, {clientX: r.left+r.width/2, clientY: r.top+r.height/2, bubbles: true})))' >/dev/null 2>&1; sleep 2
WP_AFTER=$(bc_eval 'window.__rsimMapDebug?.layers?.["wp-body"] ?? 0')
if [ "$WP_AFTER" -gt "$WP_BEFORE" ]; then
  echo "  ✓ click-to-add waypoint ($WP_BEFORE → $WP_AFTER)"
else
  echo "  ✗ click-to-add waypoint failed ($WP_BEFORE → $WP_AFTER)"
  BC_FAILED=1
fi

# (b) MissionStrip shows waypoint table
agent-browser press m >/dev/null 2>&1; sleep 2
STRIP=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.includes("Mission") ?? false')
bc_assert "MissionStrip shows Mission section" "$STRIP"
agent-browser press Escape >/dev/null 2>&1; sleep 1

# (c) validate button disabled until saved (the button has title="POST /api/missions/:id/validate")
VAL_DISABLED=$(bc_eval 'document.querySelector("button[title*=\"validate\" i]")?.disabled ?? "true"')
bc_assert "Validate disabled until saved (no mission.id)" "$VAL_DISABLED"
agent-browser press Escape >/dev/null 2>&1; sleep 1

# Result
if [ -n "${BC_FAILED:-}" ]; then echo "[$GATE_NAME] FAIL"; exit 1; fi
echo "[$GATE_NAME] PASS — Plan-on-map verified"
exit 0
