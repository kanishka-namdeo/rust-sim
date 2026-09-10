#!/usr/bin/env bash
# =============================================================================
# browser_common.sh — shared helpers for the G-14..G-21 v2 gate harnesses.
#
# Spec: docs/GCS_V2_SPEC.md §12 + §13.3 (T-D1: the M8 WebGL spike —
# agent-browser --cdp <port> attach to a self-launched chromium with
# --enable-unsafe-swiftshader).
#
# All v2 gates use agent-browser to drive the Operations Canvas at http://
# 127.0.0.1:81/ against a REAL PX4 SITL fleet (started via stack_up.sh
# start-fleet). The gates read __rsimMapDebug + __rsimTelemetry + DOM zones
# via `agent-browser eval` — pixel-presence assertions (never canvas
# readback, never fps — §12 R-2).
#
# Usage (sourced by run_g1[4-9]*.sh and run_g2[01]*.sh):
#   source "$(dirname "$0")/lib/browser_common.sh"
#
# Exported functions:
#   bc_start_stack   — start catalog + console + fleet (2-vehicle SITL)
#   bc_stop_stack    — tear down fleet + console + catalog
#   bc_eval          — agent-browser eval 'JS expression' → stdout
#   bc_navigate      — agent-browser navigate URL
#   bc_wait_canvas   — wait for __rsimMapDebug.loadedAtMs to be non-null
#   bc_assert        — assert a condition, fail the gate on false
# =============================================================================

# Don't source twice.
if [ -n "${_BROWSER_COMMON_SH:-}" ]; then return 0; fi
_BROWSER_COMMON_SH=1

# ROOT is set by the calling gate script; don't redefine it here.
# (When sourced, BASH_SOURCE[0] is this file, so dirname/../.. would resolve
# to console/ not the repo root. The gate scripts set ROOT correctly.)
export PX4_ROOT="${PX4_ROOT:-$(cd "${ROOT}/.." && pwd)/PX4-Autopilot}"
export FLEET_SIM_CFG_DIR="${FLEET_SIM_CFG_DIR:-$ROOT/fleet/scratch/vsims}"
export PATH="/home/z/.venv/bin:/home/z/.local/bin:$PATH"

BC_GATE_NAME=""
BC_WORK=""

# bc_start_stack <gate_name>
# Starts catalog :8300 + console :3000 + fleet :8400 (2 PX4 SITL vehicles).
# Idempotent — if the stack is already up, reuses it.
bc_start_stack() {
  BC_GATE_NAME="$1"
  BC_WORK="$(mktemp -d -t ${BC_GATE_NAME}.XXXXXX.dir)"
  echo "[$BC_GATE_NAME] starting operator stack (catalog + console + fleet)..."
  cd "$ROOT"
  bash scripts/stack_up.sh start >"$BC_WORK/stack_up.log" 2>&1 || {
    echo "[$BC_GATE_NAME] FAIL: stack_up.sh start failed"
    tail -20 "$BC_WORK/stack_up.log" >&2
    return 1
  }
  # Verify the stack is up.
  local ok=1
  for p in 3000 8300 8400; do
    if ! curl -s -o /dev/null -w '' --max-time 2 "http://127.0.0.1:$p/" 2>/dev/null; then
      echo "[$BC_GATE_NAME] WARN: port :$p not responding"
      ok=0
    fi
  done
  if [ "$ok" = "0" ]; then
    echo "[$BC_GATE_NAME] FAIL: stack ports not all up"
    return 1
  fi
  echo "[$BC_GATE_NAME] stack up: console :3000, catalog :8300, fleet :8400"
}

# bc_stop_stack — tear down fleet first (to prune ULogs), then console + catalog.
bc_stop_stack() {
  if [ -z "$BC_GATE_NAME" ]; then return 0; fi
  echo "[$BC_GATE_NAME] stopping operator stack..."
  cd "$ROOT"
  bash scripts/stack_up.sh stop >>"$BC_WORK/stack_up.log" 2>&1 || true
  rm -rf /tmp/rustsim-stack/fleet-run-* 2>/dev/null || true
  # Preserve $BC_WORK on failure for debugging.
  if [ -z "${BC_FAILED:-}" ]; then
    rm -rf "$BC_WORK" 2>/dev/null || true
  else
    echo "[$BC_GATE_NAME] work dir preserved at $BC_WORK (BC_FAILED=1)"
  fi
}

# bc_navigate <url> — navigate agent-browser to a URL.
bc_navigate() {
  agent-browser navigate "$1" >/dev/null 2>&1
}

# bc_eval <js> — run JS in the browser, return the result string.
bc_eval() {
  agent-browser eval "$1" 2>/dev/null | head -1
}

# bc_wait_canvas <timeout_s> — wait for the map to load (loadedAtMs non-null).
bc_wait_canvas() {
  local timeout="${1:-30}"
  local i=0
  while [ "$i" -lt "$timeout" ]; do
    local loaded
    loaded=$(bc_eval 'window.__rsimMapDebug?.loadedAtMs ?? null')
    if [ "$loaded" != "null" ] && [ -n "$loaded" ]; then
      echo "[$BC_GATE_NAME] canvas loaded at ${loaded}ms"
      return 0
    fi
    sleep 1
    i=$((i + 1))
  done
  echo "[$BC_GATE_NAME] FAIL: canvas did not load within ${timeout}s"
  return 1
}

# bc_assert <description> <condition> — assert truthy (non-empty, non-null, non-false).
bc_assert() {
  local desc="$1"
  local cond="$2"
  if [ -n "$cond" ] && [ "$cond" != "null" ] && [ "$cond" != "undefined" ] && [ "$cond" != "false" ]; then
    echo "  ✓ $desc"
    return 0
  fi
  echo "  ✗ $desc (got: $cond)"
  BC_FAILED=1
  return 1
}

# bc_assert_eq <description> <actual> <expected> — assert equality.
bc_assert_eq() {
  local desc="$1"
  local actual="$2"
  local expected="$3"
  if [ "$actual" = "$expected" ]; then
    echo "  ✓ $desc ($actual)"
    return 0
  fi
  echo "  ✗ $desc (expected: $expected, got: $actual)"
  BC_FAILED=1
  return 1
}

# bc_assert_ge <description> <actual> <minimum> — assert actual >= minimum (integers).
bc_assert_ge() {
  local desc="$1"
  local actual="$2"
  local minimum="$3"
  if [ "$actual" -ge "$minimum" ] 2>/dev/null; then
    echo "  ✓ $desc ($actual >= $minimum)"
    return 0
  fi
  echo "  ✗ $desc (expected >= $minimum, got: $actual)"
  BC_FAILED=1
  return 1
}
