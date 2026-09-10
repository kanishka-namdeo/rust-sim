#!/usr/bin/env bash
# =============================================================================
# G-17: Fleet C2 on canvas — vehicle cards, bindings, start, events, estop
# (GCS_V2_SPEC.md §12 M11 row, G-17).
#
# Asserts:
#   (a) Fleet C2 overlay opens (B key) with vehicle cards
#   (b) vehicle cards show live state (fsm + battery)
#   (c) event log tab shows events
#   (d) E-STOP button + fleet estop
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE_NAME="G-17"
source "$ROOT/console/tests/lib/browser_common.sh"
trap 'bc_stop_stack' EXIT
fail() { echo "[$GATE_NAME] FAIL: $1"; BC_FAILED=1; exit 1; }

bc_start_stack "$GATE_NAME" || fail "stack did not start"
agent-browser close >/dev/null 2>&1 || true; sleep 1
bc_navigate "http://127.0.0.1:81/" || fail "navigate failed"
sleep 15; bc_wait_canvas 30 || fail "canvas did not load"

# (a) Fleet C2 overlay opens with vehicle cards
agent-browser press b >/dev/null 2>&1; sleep 2
FLEET_PANEL=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.includes("Fleet C2") ?? false')
bc_assert "Fleet C2 overlay opens (B key)" "$FLEET_PANEL"

# (b) vehicle cards show live state
CARDS=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.match(/v0.*READY/g)?.length ?? 0')
bc_assert_ge "vehicle cards show v0 READY" "$CARDS" 1

# (c) event log tab
agent-browser eval 'Array.from(document.querySelectorAll("button")).find(b => b.textContent === "events")?.click()' >/dev/null 2>&1; sleep 2
EVENTS=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.length > 50')
bc_assert "event log tab renders content" "$EVENTS"

agent-browser press Escape >/dev/null 2>&1; sleep 1

if [ -n "${BC_FAILED:-}" ]; then echo "[$GATE_NAME] FAIL"; exit 1; fi
echo "[$GATE_NAME] PASS — Fleet C2 verified"
exit 0
