#!/usr/bin/env bash
# =============================================================================
# G-1: Mission validation (GCS_SPEC.md §9, ADR-0026).
#
# Single-invocation harness: start the catalog on :8302 with a temp dir, POST
# each failing/passing mission, call /validate, and assert each V-rule fires
# with the correct rule code and valid flag. Tears down, exits 0 on PASS /
# non-zero on FAIL.
#
# Rules exercised (subset of ADR-0026's V-1..V-13, per the M1 spec):
#   V-1   empty waypoints                       → rule "V-1", valid:false
#   V-2   101 waypoints                          → rule "V-2", valid:false
#   V-3   waypoint outside inclusion fence       → rule "V-3", valid:false
#   V-5   altitude 200 m on a quad               → rule "V-5", valid:false
#   V-7   self-intersecting fence (bowtie)       → rule "V-7", valid:false
#   V-9   6 rally points                         → rule "V-9", valid:false
#   V-10  rally point outside inclusion fence    → rule "V-10", valid:false
#   V-12  unsupported command 999                → rule "V-12", valid:false
#   (valid) 4-waypoint mission inside fence + 1 rally → valid:true, errors:[]
#
# The vertex-drag stress test (R-9, G-1's browser component) is out of scope
# for this bash harness — it runs in the browser test suite.
#
# Environment overrides:
#   CATALOG_BIN  (default <repo>/fleet/target/debug/fleet-catalog)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CATALOG_BIN="${CATALOG_BIN:-$ROOT/fleet/target/debug/fleet-catalog}"
PORT_CATALOG=8302

WORK="$(mktemp -d -t g1_test.XXXXXX.dir)"
CATALOG_DIR="$WORK/cat"
FC_LOG="$WORK/fc.log"
FC_PID=""
RULES_DIR="$WORK/rules"

cleanup() {
    if [ -n "$FC_PID" ] && kill -0 "$FC_PID" 2>/dev/null; then
        kill "$FC_PID" 2>/dev/null || true
        for _ in $(seq 1 20); do
            kill -0 "$FC_PID" 2>/dev/null || break
            sleep 0.1
        done
        kill -9 "$FC_PID" 2>/dev/null || true
    fi
    wait 2>/dev/null || true
    rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

FAIL_COUNT=0

# ----- fail(reason) : print the failure line for the most-recent rule -----
rule_fail() {
    local label=$1; shift
    local reason=$1; shift
    echo "  ✗ $label: FAIL: $reason"
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

# ----- preconditions -----
[ -x "$CATALOG_BIN" ] || { echo "[G-1] FAIL: fleet-catalog binary missing at $CATALOG_BIN"; exit 1; }
command -v curl >/dev/null || { echo "[G-1] FAIL: curl not available"; exit 1; }
command -v jq   >/dev/null || { echo "[G-1] FAIL: jq not available"; exit 1; }
command -v python3 >/dev/null || { echo "[G-1] FAIL: python3 not available"; exit 1; }

if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" >/dev/null 2>&1; then
    echo "[G-1] FAIL: port $PORT_CATALOG already serving (previous run?)"
    exit 1
fi

mkdir -p "$RULES_DIR"

# ----- shared mission fragments -----
# Inclusion fence: a 4-vertex square at lat 37.4130..37.4140, lon -122.1020..-122.1010.
# Waypoints placed strictly INSIDE the fence (lat 37.4133/37.4137, lon -122.1017/-122.1013)
# so the boundary-on-the-edge cases (V-3 strict containment) don't false-trigger.

build_mission() {
    # Build a mission TOML file from named fragments. Arg 1 = output file.
    local out=$1; shift
    local name=$1; shift
    local vehicle_type=$1; shift
    local wp_block=$1; shift       # path to a file with the [[waypoints]] block (or empty)
    local fence_block=$1; shift    # path to a file with the [geofence] block
    local rally_block=$1; shift    # path to a file with the [[rally]] block (or empty)
    {
        cat <<TOML
[mission]
id = "g1-placeholder"
name = "$name"
version = 1
created_at = "2026-09-09T10:00:00Z"
updated_at = "2026-09-09T10:00:00Z"
vehicle_type = "$vehicle_type"
px4_version = "v1.16.2"

TOML
        [ -s "$wp_block" ] && cat "$wp_block"
        cat "$fence_block"
        [ -s "$rally_block" ] && cat "$rally_block"
    } > "$out"
}

# ----- shared fence / rally / waypoint fragments -----
# Standard inclusion fence (square, 4 vertices, simple polygon).
FENCE_OK="$RULES_DIR/fence_ok.toml"
cat > "$FENCE_OK" <<'TOML'

[geofence]
ceiling_m = 60
floor_m = 0
inclusion = [[37.4130, -122.1020], [37.4140, -122.1020], [37.4140, -122.1010], [37.4130, -122.1010]]
exclusion = []
TOML

# Self-intersecting bowtie fence (V-7): vertex 2 swapped with vertex 3
# produces two crossing diagonals.
FENCE_BOWTIE="$RULES_DIR/fence_bowtie.toml"
cat > "$FENCE_BOWTIE" <<'TOML'

[geofence]
ceiling_m = 60
floor_m = 0
inclusion = [[37.4130, -122.1020], [37.4140, -122.1010], [37.4140, -122.1020], [37.4130, -122.1010]]
exclusion = []
TOML

# 1 rally point inside the fence (used by all the failing tests so we don't
# accidentally trip V-10 when testing V-5, V-7, etc.).
RALLY_OK="$RULES_DIR/rally_ok.toml"
cat > "$RALLY_OK" <<'TOML'

[[rally]]
seq = 0
lat = 37.4135
lon = -122.1015
alt_m = 0.0
TOML

# 1 waypoint strictly inside the fence (command 16, frame 3, alt 12 m).
WP_OK="$RULES_DIR/wp_ok.toml"
cat > "$WP_OK" <<'TOML'

[[waypoints]]
seq = 0
frame = 3
command = 16
x = 37.4133
y = -122.1017
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0
TOML

# ----- build per-rule mission files -----

# V-1: empty waypoints (no [[waypoints]] blocks at all).
: > "$RULES_DIR/wp_empty.toml"  # empty file (no waypoints)
build_mission "$RULES_DIR/v1.toml" "v1-empty" "quad" \
    "$RULES_DIR/wp_empty.toml" "$FENCE_OK" "$RALLY_OK"

# V-2: 101 waypoints.
python3 - > "$RULES_DIR/wp_101.toml" <<'PYEOF'
parts = []
for i in range(101):
    parts.append(f"""
[[waypoints]]
seq = {i}
frame = 3
command = 16
x = 37.4133
y = -122.1017
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0""")
print("".join(parts))
PYEOF
build_mission "$RULES_DIR/v2.toml" "v2-101wp" "quad" \
    "$RULES_DIR/wp_101.toml" "$FENCE_OK" "$RALLY_OK"

# V-3: 1 waypoint outside the inclusion fence.
cat > "$RULES_DIR/wp_outside.toml" <<'TOML'

[[waypoints]]
seq = 0
frame = 3
command = 16
x = 37.4133
y = -122.1017
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0

[[waypoints]]
seq = 1
frame = 3
command = 16
x = 37.4150
y = -122.1010
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0
TOML
build_mission "$RULES_DIR/v3.toml" "v3-outside" "quad" \
    "$RULES_DIR/wp_outside.toml" "$FENCE_OK" "$RALLY_OK"

# V-5: altitude 200 m for a quad (max is 120). Ceiling bumped to 250 so V-6
# doesn't trip first.
FENCE_HIGH="$RULES_DIR/fence_high.toml"
cat > "$FENCE_HIGH" <<'TOML'

[geofence]
ceiling_m = 250
floor_m = 0
inclusion = [[37.4130, -122.1020], [37.4140, -122.1020], [37.4140, -122.1010], [37.4130, -122.1010]]
exclusion = []
TOML
cat > "$RULES_DIR/wp_alt200.toml" <<'TOML'

[[waypoints]]
seq = 0
frame = 3
command = 16
x = 37.4133
y = -122.1017
z = 200.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0
TOML
build_mission "$RULES_DIR/v5.toml" "v5-alt200" "quad" \
    "$RULES_DIR/wp_alt200.toml" "$FENCE_HIGH" "$RALLY_OK"

# V-7: self-intersecting fence (bowtie).
build_mission "$RULES_DIR/v7.toml" "v7-bowtie" "quad" \
    "$WP_OK" "$FENCE_BOWTIE" "$RALLY_OK"

# V-9: 6 rally points.
python3 - > "$RULES_DIR/rally_6.toml" <<'PYEOF'
parts = []
for i in range(6):
    parts.append(f"""
[[rally]]
seq = {i}
lat = 37.4135
lon = -122.1015
alt_m = 0.0""")
print("".join(parts))
PYEOF
build_mission "$RULES_DIR/v9.toml" "v9-rally6" "quad" \
    "$WP_OK" "$FENCE_OK" "$RULES_DIR/rally_6.toml"

# V-10: rally point outside the fence.
cat > "$RULES_DIR/rally_outside.toml" <<'TOML'

[[rally]]
seq = 0
lat = 37.4150
lon = -122.1010
alt_m = 0.0
TOML
build_mission "$RULES_DIR/v10.toml" "v10-rally-out" "quad" \
    "$WP_OK" "$FENCE_OK" "$RULES_DIR/rally_outside.toml"

# V-12: unsupported command 999.
cat > "$RULES_DIR/wp_cmd999.toml" <<'TOML'

[[waypoints]]
seq = 0
frame = 3
command = 999
x = 37.4133
y = -122.1017
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0
TOML
build_mission "$RULES_DIR/v12.toml" "v12-badcmd" "quad" \
    "$RULES_DIR/wp_cmd999.toml" "$FENCE_OK" "$RALLY_OK"

# Valid mission: 4 waypoints strictly inside the fence + 1 rally inside.
cat > "$RULES_DIR/wp_valid4.toml" <<'TOML'

[[waypoints]]
seq = 0
frame = 3
command = 16
x = 37.4133
y = -122.1017
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0

[[waypoints]]
seq = 1
frame = 3
command = 16
x = 37.4137
y = -122.1017
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0

[[waypoints]]
seq = 2
frame = 3
command = 16
x = 37.4137
y = -122.1013
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0

[[waypoints]]
seq = 3
frame = 3
command = 16
x = 37.4133
y = -122.1013
z = 12.0
param1 = 0.5
param2 = 2.0
param3 = 0.0
param4 = 0.0
TOML
build_mission "$RULES_DIR/valid.toml" "valid-square" "quad" \
    "$RULES_DIR/wp_valid4.toml" "$FENCE_OK" "$RALLY_OK"

# ----- start catalog -----
echo "[G-1] binary:    $CATALOG_BIN"
echo "[G-1] port:      :$PORT_CATALOG"
echo "[G-1] work dir:  $WORK"

mkdir -p "$CATALOG_DIR"
"$CATALOG_BIN" --port "$PORT_CATALOG" --catalog-dir "$CATALOG_DIR" \
    --fleet-url "http://127.0.0.1:8400" >"$FC_LOG" 2>&1 &
FC_PID=$!

ok=""
for _ in $(seq 1 50); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" \
            | grep -q '"service":"fleet-catalog"'; then
        ok=1; break
    fi
    kill -0 "$FC_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || { echo "[G-1] FAIL: catalog did not come up on :$PORT_CATALOG"; cat "$FC_LOG"; exit 1; }

# ----- validate_one(label, file, expected_rule | "VALID") -----
# POSTs the mission, then POSTs /validate; prints ✓/✗ per rule.
validate_one() {
    local label=$1; shift
    local mission_file=$1; shift
    local expected_rule=$1; shift   # "V-1".."V-12" or "VALID"

    local resp id body http_code
    resp=$(curl -s --max-time 3 -X POST "http://127.0.0.1:$PORT_CATALOG/api/missions" \
        --data-binary "@$mission_file")
    id=$(echo "$resp" | jq -r '.data.mission.id // empty')
    if [ -z "$id" ]; then
        rule_fail "$label" "POST /api/missions did not return id; resp: $resp"
        return
    fi

    body=$(curl -s --max-time 3 -X POST "http://127.0.0.1:$PORT_CATALOG/api/missions/$id/validate")

    if [ "$expected_rule" = "VALID" ]; then
        local valid=$(echo "$body" | jq -r '.data.valid')
        local n_errors=$(echo "$body" | jq -r '.data.errors | length')
        if [ "$valid" = "true" ] && [ "$n_errors" = "0" ]; then
            echo "  ✓ $label: PASS (valid=true, 0 errors)"
        else
            rule_fail "$label" "expected valid=true & 0 errors, got valid=$valid errors=$n_errors; body: $body"
        fi
        return
    fi

    # Failing case: assert valid=false and the expected rule appears in errors.
    local valid=$(echo "$body" | jq -r '.data.valid')
    local matched=$(echo "$body" | jq -r --arg r "$expected_rule" \
        '[.data.errors[]? | select(.rule == $r)] | length')

    if [ "$valid" = "false" ] && [ "$matched" -ge 1 ] 2>/dev/null; then
        local msg=$(echo "$body" | jq -r --arg r "$expected_rule" \
            '.data.errors[] | select(.rule == $r) | .message' | head -1)
        echo "  ✓ $label: PASS (rule $expected_rule — $msg)"
    else
        rule_fail "$label" "expected valid=false + rule=$expected_rule, got valid=$valid matched=$matched; body: $body"
    fi
}

# ----- run each rule -----
validate_one "V-1  (empty waypoints)"         "$RULES_DIR/v1.toml"    "V-1"
validate_one "V-2  (101 waypoints)"            "$RULES_DIR/v2.toml"    "V-2"
validate_one "V-3  (waypoint outside fence)"   "$RULES_DIR/v3.toml"    "V-3"
validate_one "V-5  (altitude 200 m on quad)"   "$RULES_DIR/v5.toml"    "V-5"
validate_one "V-7  (self-intersecting fence)"  "$RULES_DIR/v7.toml"    "V-7"
validate_one "V-9  (6 rally points)"           "$RULES_DIR/v9.toml"    "V-9"
validate_one "V-10 (rally outside fence)"      "$RULES_DIR/v10.toml"   "V-10"
validate_one "V-12 (unsupported command 999)"  "$RULES_DIR/v12.toml"   "V-12"
validate_one "valid 4-waypoint mission"        "$RULES_DIR/valid.toml" "VALID"

# ----- summary -----
if [ "$FAIL_COUNT" -eq 0 ]; then
    echo "[G-1] PASS"
    exit 0
else
    echo "[G-1] FAIL: $FAIL_COUNT rule(s) failed"
    exit 1
fi
