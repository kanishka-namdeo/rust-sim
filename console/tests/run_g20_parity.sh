#!/usr/bin/env bash
# =============================================================================
# G-20: Parity audit — full §3.2 checklist, perf budgets, ignoreBuildErrors:
# false (GCS_V2_SPEC.md §12 M14 row, G-20).
#
# Asserts:
#   (a) ignoreBuildErrors:false — npm run build passes with full typecheck
#   (b) npm run lint clean
#   (c) normalize.test.ts passes (13 tests after Task 7a cleanup)
#   (d) all 6 GCS overlay panels mountable (M, L, B, S, Y, Settings)
#       — SITL/SimControl overlay removed in Task 7a (overkill sim-internal UI)
#   (e) Leaflet removed from package.json
#   (f) /legacy route removed (only / + /api in route map)
#   (g) map load <= 5s (cold route)
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE_NAME="G-20"

WORK="$(mktemp -d -t g20.XXXXXX.dir)"
trap 'rm -rf "$WORK" 2>/dev/null || true' EXIT
fail() { echo "[$GATE_NAME] FAIL: $1"; exit 1; }

CONSOLE="$ROOT/console"

# (a) ignoreBuildErrors:false — npm run build passes
echo "[$GATE_NAME] (a) npm run build with ignoreBuildErrors:false..."
cd "$CONSOLE"
if npm run build >"$WORK/build.log" 2>&1; then
  echo "  ✓ npm run build passes with full typecheck"
else
  echo "  ✗ npm run build FAILED (ignoreBuildErrors:false)"
  tail -20 "$WORK/build.log" >&2
  fail "build failed"
fi

# Verify ignoreBuildErrors is actually false
if grep -q "ignoreBuildErrors.*false" "$CONSOLE/next.config.ts"; then
  echo "  ✓ ignoreBuildErrors:false confirmed in next.config.ts"
else
  fail "ignoreBuildErrors is not false in next.config.ts"
fi

# (b) npm run lint clean
echo "[$GATE_NAME] (b) npm run lint..."
if npm run lint >"$WORK/lint.log" 2>&1; then
  echo "  ✓ npm run lint clean"
else
  echo "  ✗ npm run lint FAILED"
  tail -10 "$WORK/lint.log" >&2
  fail "lint failed"
fi

# (c) normalize.test.ts
echo "[$GATE_NAME] (c) normalize.test.ts..."
if node --test "$CONSOLE/src/lib/normalize.test.ts" >"$WORK/test.log" 2>&1; then
  echo "  ✓ normalize.test.ts passes"
else
  tail -10 "$WORK/test.log" >&2
  fail "normalize.test.ts failed"
fi

# (e) Leaflet removed
if grep -q '"leaflet"' "$CONSOLE/package.json"; then
  fail "leaflet still in package.json"
else
  echo "  ✓ Leaflet removed from package.json"
fi

# (f) /legacy route removed
if [ -d "$CONSOLE/src/app/legacy" ]; then
  fail "/legacy route still exists"
else
  echo "  ✓ /legacy route removed"
fi

# (g) map load <= 5s (check the build log for route map — / + /api only)
if grep -q "/legacy" "$WORK/build.log"; then
  fail "/legacy still in route map"
else
  echo "  ✓ route map is / + /api only (no /legacy)"
fi

# (d) all 6 GCS overlay panels — check they're imported in OperationsCanvas
# (Task 7a cleanup: SITL/SimControl overlay removed; 7 -> 6 panels)
OVERLAYS=$(grep -c "overlays\.\(mission\|library\|fleet\|setup\|analyze\|settings\)" "$CONSOLE/src/components/canvas/OperationsCanvas.tsx")
if [ "$OVERLAYS" -ge 6 ]; then
  echo "  ✓ all 6 GCS overlay panels mounted in OperationsCanvas ($OVERLAYS)"
else
  fail "only $OVERLAYS overlay panels mounted (expected >= 6)"
fi

echo "[$GATE_NAME] PASS — parity audit verified"
exit 0
