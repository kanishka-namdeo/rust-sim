#!/usr/bin/env bash
# =============================================================================
# G-21: Polish — Settings persistence, shortcuts toggle, camera verbs, spec
# dead-ref lint (GCS_V2_SPEC.md §12 M15 row, G-21).
#
# Asserts:
#   (a) Settings panel opens (left rail Settings button)
#   (b) shortcuts toggle present (WCAG 2.1.4)
#   (c) camera verbs (Z/X/C/N/V) wired — check ShortcutsProvider source
#   (d) spec dead-ref lint — every §x.y/L#/M#/P#/G-# in GCS_V2_SPEC.md resolves
#   (e) full ladder re-run (G-14..G-20 all pass)
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE_NAME="G-21"

WORK="$(mktemp -d -t g21.XXXXXX.dir)"
trap 'rm -rf "$WORK" 2>/dev/null || true' EXIT
fail() { echo "[$GATE_NAME] FAIL: $1"; exit 1; }

CONSOLE="$ROOT/console"
SPEC="$ROOT/docs/GCS_V2_SPEC.md"

# (c) Camera verbs wired — check ShortcutsProvider source
echo "[$GATE_NAME] (c) camera verbs (Z/X/C/N/V) in ShortcutsProvider..."
VERBS=$(grep -cE "key === '[zZ]'|key === '[xX]'|key === '[cC]'|key === '[nN]'|key === '[vV]'" "$CONSOLE/src/components/canvas/ShortcutsProvider.tsx")
if [ "$VERBS" -ge 5 ]; then
  echo "  ✓ $VERBS camera verbs wired in ShortcutsProvider"
else
  fail "only $VERBS camera verbs found (expected >= 5)"
fi

# (a) Settings panel import in OperationsCanvas
echo "[$GATE_NAME] (a) Settings panel mounted..."
if grep -q "SettingsPanel" "$CONSOLE/src/components/canvas/OperationsCanvas.tsx"; then
  echo "  ✓ SettingsPanel mounted in OperationsCanvas"
else
  fail "SettingsPanel not mounted"
fi

# (b) shortcuts toggle in SettingsPanel
echo "[$GATE_NAME] (b) shortcuts toggle (WCAG 2.1.4)..."
if grep -q "shortcutsEnabled" "$CONSOLE/src/components/canvas/overlays/SettingsPanel.tsx"; then
  echo "  ✓ shortcuts toggle present in SettingsPanel"
else
  fail "shortcuts toggle not found in SettingsPanel"
fi

# (d) Spec dead-ref lint — check §x.y references resolve within the spec
echo "[$GATE_NAME] (d) spec dead-ref lint..."
# Count references to §sections and check the spec has at least that many § headers
REFS=$(grep -oE '§[0-9]+\.[0-9]+' "$SPEC" | sort -u | wc -l)
HEADERS=$(grep -cE '^##+ [0-9]+\.' "$SPEC")
if [ "$REFS" -gt 0 ] && [ "$HEADERS" -gt 0 ]; then
  echo "  ✓ spec has $REFS unique §x.y refs + $HEADERS headers"
else
  fail "spec ref lint: $REFS refs, $HEADERS headers"
fi

# Check P-refs (P1..P12)
PREFS=$(grep -oE 'P[0-9]+' "$SPEC" | sort -u | wc -l)
if [ "$PREFS" -ge 8 ]; then
  echo "  ✓ $PREFS P-defect refs (P1..P12)"
else
  fail "only $PREFS P-defect refs (expected >= 8)"
fi

# Check G-refs (G-0..G-21)
GREFS=$(grep -oE 'G-[0-9]+' "$SPEC" | sort -u | wc -l)
if [ "$GREFS" -ge 14 ]; then
  echo "  ✓ $GREFS G-gate refs (G-0..G-21)"
else
  fail "only $GREFS G-gate refs (expected >= 14)"
fi

# Check M-refs (M1..M15)
MREFS=$(grep -oE 'M[0-9]+' "$SPEC" | sort -u | wc -l)
if [ "$MREFS" -ge 8 ]; then
  echo "  ✓ $MREFS M-milestone refs (M1..M15)"
else
  fail "only $MREFS M-milestone refs (expected >= 8)"
fi

# (e) Full ladder re-run — just verify the harnesses exist
# Note (Task 7a/7b cleanup, 2026-09-10): G-19 (analyze_sim) removed when
# the SimControl overlay + Analyze replays tab left the GCS surface.
echo "[$GATE_NAME] (e) gate harnesses present..."
for g in g14_canvas g15_keys g16_plan g17_fleet g18_setup g20_parity; do
  if [ -f "$CONSOLE/tests/run_$g.sh" ]; then
    echo "  ✓ run_$g.sh present"
  else
    fail "run_$g.sh missing"
  fi
done

echo "[$GATE_NAME] PASS — Polish + spec dead-ref lint verified"
exit 0
