#!/usr/bin/env bash
# =============================================================================
# G-18: Setup drawer — param download, write, calibrate, airframe, presets
# (GCS_V2_SPEC.md §12 M12 row, G-18).
#
# Asserts:
#   (a) Setup drawer opens (S key) with sections
#   (b) summary section shows vehicle state
#   (c) params section renders search + param table
#   (d) presets section renders save/load/delete
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE_NAME="G-18"
source "$ROOT/console/tests/lib/browser_common.sh"
trap 'bc_stop_stack' EXIT
fail() { echo "[$GATE_NAME] FAIL: $1"; BC_FAILED=1; exit 1; }

bc_start_stack "$GATE_NAME" || fail "stack did not start"
agent-browser close >/dev/null 2>&1 || true; sleep 1
bc_navigate "http://127.0.0.1:81/" || fail "navigate failed"
sleep 15; bc_wait_canvas 30 || fail "canvas did not load"

# (a) Setup drawer opens
agent-browser press s >/dev/null 2>&1; sleep 3
SETUP=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.includes("Vehicle Setup") ?? false')
bc_assert "Setup drawer opens (S key)" "$SETUP"

# (b) summary shows vehicle state (allow time for the REST fetch)
sleep 3
SUMMARY=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.match(/FSM|Mode|Airframe|Armed/i)?.length > 0')
bc_assert "summary section shows vehicle state" "$SUMMARY"

# (c) params section — click the "params" tab button
agent-browser eval 'Array.from(document.querySelectorAll("button")).find(b => b.textContent === "params")?.click()' >/dev/null 2>&1; sleep 2
PARAMS=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.match(/search|param/i)?.length > 0')
bc_assert "params section renders search/param" "$PARAMS"

# (d) presets section
agent-browser eval 'Array.from(document.querySelectorAll("button")).find(b => b.textContent === "presets")?.click()' >/dev/null 2>&1; sleep 2
PRESETS=$(bc_eval 'document.querySelector("[data-rsim-zone=F]")?.textContent?.includes("preset") ?? false')
bc_assert "presets section renders" "$PRESETS"

agent-browser press Escape >/dev/null 2>&1; sleep 1

if [ -n "${BC_FAILED:-}" ]; then echo "[$GATE_NAME] FAIL"; exit 1; fi
echo "[$GATE_NAME] PASS — Setup drawer verified"
exit 0
