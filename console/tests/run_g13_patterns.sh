#!/usr/bin/env bash
# =============================================================================
# G-13: Survey / corridor / perimeter pattern generators (GCS_SPEC.md §9,
# §5.6 AC-5.6.1..AC-5.6.4, ADR-0028 — patterns generated client-side in
# console/src/lib/patterns.ts).
#
# Single-invocation harness: compile patterns.ts + test_patterns.ts with tsc,
# run the test binary with node, report PASS/FAIL. No backend, no ports, no
# background processes.
#
# Asserts (4 tests, 15 checks):
#   Test 1 — Survey Grid (AC-5.6.1):
#     - ≥10 legs generated on a 100 m × 100 m polygon with legSpacingM=10
#     - all waypoints inside the polygon (point-in-polygon check)
#     - all waypoints have altitudeM=30
#     - ≥1 camera-trigger waypoint (command=200)
#   Test 2 — Corridor Pattern (AC-5.6.2):
#     - all waypoints within corridorWidthM/2 of the polyline
#     - no gaps wider than legSpacingM along the corridor direction
#     - corridor spans the full polyline length
#   Test 3 — Perimeter Pattern (AC-5.6.3):
#     - exactly 4 waypoints on a 4-vertex polygon
#     - all waypoints inside the original polygon (offset is inward)
#     - waypoints form a non-degenerate closed loop
#     - each vertex offset inward by ≥ offsetM
#   Test 4 — Mission editor integration (AC-5.6.4):
#     - all waypoints have frame=3 (GLOBAL_RELATIVE_ALT)
#     - all waypoints have command ∈ {16, 200}
#     - all waypoints inherit default altitude
#     - NAV_WAYPOINT WPs have default accept-radius
#
# Environment overrides:
#   TSC_BIN   (default <repo>/console/node_modules/.bin/tsc)
#   NODE_BIN  (default /usr/bin/env node)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONSOLE_DIR="$ROOT/console"
TSC_BIN="${TSC_BIN:-$CONSOLE_DIR/node_modules/.bin/tsc}"
NODE_BIN="${NODE_BIN:-node}"

PATTERNS_SRC="$CONSOLE_DIR/src/lib/patterns.ts"
TEST_SRC="$CONSOLE_DIR/tests/test_patterns.ts"

WORK="$(mktemp -d -t g13_test.XXXXXX.dir)"
OUT_DIR="$WORK/out"
TEST_LOG="$WORK/test.log"
TSC_LOG="$WORK/tsc.log"

cleanup() {
    rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

fail() {
    echo "[G-13] FAIL: $1"
    echo "=========== diagnostics ==========="
    echo "--- tsc.log (tail 20):"
    tail -20 "$TSC_LOG" 2>/dev/null || echo "(none)"
    echo "--- test.log (full):"
    cat "$TEST_LOG" 2>/dev/null || echo "(none)"
    echo "==================================="
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -f "$PATTERNS_SRC" ] || fail "patterns.ts missing at $PATTERNS_SRC"
[ -f "$TEST_SRC" ] || fail "test_patterns.ts missing at $TEST_SRC"
[ -x "$TSC_BIN" ] || fail "tsc not found at $TSC_BIN (run 'npm install' in console/)"
command -v "$NODE_BIN" >/dev/null || fail "node not available"

echo "[G-13] console:    $CONSOLE_DIR"
echo "[G-13] patterns:   $PATTERNS_SRC"
echo "[G-13] test src:   $TEST_SRC"
echo "[G-13] out dir:    $OUT_DIR"

# -----------------------------------------------------------------------------
# Phase 1: compile patterns.ts + test_patterns.ts to $OUT_DIR.
#
# Flag rationale:
#   --module nodenext            Node ESM output (uses .js extensions in imports)
#   --moduleResolution nodenext Required for nodenext module mode
#   --target es2022             Modern JS (top-level await, etc. — not strictly
#                               needed, but matches the console tsconfig target
#                               family of ES2017+)
#   --strict                    Catch undefined / null / implicit-any at compile
#   --skipLibCheck              Don't typecheck @types/node etc. (faster)
#   --outDir $OUT_DIR           Emit next to each other so relative imports
#                               resolve at runtime.
#
# Note: the test file declares `console` and `process` with minimal ambient
# types so tsc does NOT need @types/node (which would require --typeRoots
# since the explicit-file-args invocation bypasses tsconfig.json's `types`).
#
# Input files preserve their common-root-relative paths in the output, so:
#   console/src/lib/patterns.ts → $OUT_DIR/src/lib/patterns.js
#   console/tests/test_patterns.ts → $OUT_DIR/tests/test_patterns.js
# -----------------------------------------------------------------------------
echo "[G-13] phase 1: compile patterns + tests via tsc"

if ! "$TSC_BIN" \
    "$PATTERNS_SRC" "$TEST_SRC" \
    --outDir "$OUT_DIR" \
    --module nodenext \
    --moduleResolution nodenext \
    --target es2022 \
    --strict \
    --skipLibCheck \
    >"$TSC_LOG" 2>&1; then
    fail "tsc compilation failed (see tsc.log)"
fi

COMPILED_PATTERNS="$OUT_DIR/src/lib/patterns.js"
COMPILED_TEST="$OUT_DIR/tests/test_patterns.js"
[ -f "$COMPILED_PATTERNS" ] || fail "patterns.js not emitted at $COMPILED_PATTERNS"
[ -f "$COMPILED_TEST" ] || fail "test_patterns.js not emitted at $COMPILED_TEST"
echo "[G-13] phase 1 PASS: compiled → $COMPILED_TEST"

# -----------------------------------------------------------------------------
# Phase 2: run the test binary; capture stdout+stderr.
# -----------------------------------------------------------------------------
echo "[G-13] phase 2: run test_patterns.js"
if ! "$NODE_BIN" "$COMPILED_TEST" >"$TEST_LOG" 2>&1; then
    fail "test_patterns.js exited non-zero (see test.log)"
fi

# Echo the test output for visibility, then assert the closing banner.
cat "$TEST_LOG"

if ! grep -q "\[G-13\] all checks passed" "$TEST_LOG"; then
    fail "test_patterns.js did not emit the PASS banner"
fi

echo "[G-13] PASS"
exit 0
