#!/usr/bin/env bash
# =============================================================================
# G-2: Mission persistence CRUD round-trip (GCS_SPEC.md §9, ADR-0020).
#
# Single-invocation harness: start the catalog on :8303 with a temp dir,
# exercise the full create → list → fetch → update → version-fetch →
# delete → soft-delete list → restart → list-again sequence, assert at
# each step. Tears down, exits 0 on PASS / non-zero on FAIL.
#
# Asserts (GCS_SPEC.md §9 G-2 row):
#   (a)  POST  /api/missions               → returns ULID id1 + version 1
#   (b)  GET   /api/missions               → 1 mission, name "alpha"
#   (c)  GET   /api/missions/{id1}         → name "alpha", version 1
#   (d)  PUT   /api/missions/{id1}          → version 2 (server bumps)
#   (e)  GET   /api/missions/{id1}         → name "alpha-v2", version 2
#   (f)  GET   /api/missions/{id1}/versions/1 → name "alpha" (old version retained)
#   (g)  POST  /api/missions               → returns ULID id2 ("beta")
#   (h)  GET   /api/missions               → 2 missions
#   (i)  DELETE /api/missions/{id1}        → ok:true
#   (j)  GET   /api/missions               → 1 mission (alpha excluded)
#   (k)  GET   /api/missions?include_deleted=true → 2 missions, alpha has deleted:true
#   (l)  RESTART catalog (same dir, fresh process)
#   (m)  GET   /api/missions               → 1 mission (beta) — survived restart
#   (n)  GET   /api/missions/{id2}         → name "beta"
#
# Environment overrides:
#   CATALOG_BIN  (default <repo>/fleet/target/debug/fleet-catalog)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CATALOG_BIN="${CATALOG_BIN:-$ROOT/fleet/target/debug/fleet-catalog}"
PORT_CATALOG=8303

WORK="$(mktemp -d -t g2_test.XXXXXX.dir)"
CATALOG_DIR="$WORK/catalog"   # persisted across restart
FC_LOG_A="$WORK/fc-a.log"
FC_LOG_B="$WORK/fc-b.log"
FC_PID=""

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

fail() {
    echo "[G-2] FAIL: $1"
    echo "=========== diagnostics ==========="
    echo "--- fc-a.log (tail 20):"
    tail -20 "$FC_LOG_A" 2>/dev/null || echo "(none)"
    echo "--- fc-b.log (tail 20):"
    tail -20 "$FC_LOG_B" 2>/dev/null || echo "(none)"
    echo "--- catalog dir listing:"
    ls -la "$CATALOG_DIR" 2>/dev/null || echo "(empty / missing)"
    exit 1
}

# ----- preconditions -----
[ -x "$CATALOG_BIN" ] || fail "fleet-catalog binary missing at $CATALOG_BIN (build fleet workspace first)"
command -v curl >/dev/null || fail "curl not available"
command -v jq   >/dev/null || fail "jq not available"
if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" >/dev/null 2>&1; then
    fail "port $PORT_CATALOG already serving (previous run?)"
fi

mkdir -p "$CATALOG_DIR"

# ----- mission file generator: writes a valid mission TOML with a given name.
# Waypoints are strictly inside the fence so the validator never trips V-3.
gen_mission_toml() {
    local name=$1; shift
    local out=$1; shift
    cat > "$out" <<TOML
[mission]
id = "g2-placeholder"
name = "$name"
version = 1
created_at = "2026-09-09T10:00:00Z"
updated_at = "2026-09-09T10:00:00Z"
vehicle_type = "quad"
px4_version = "v1.16.2"

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

[geofence]
ceiling_m = 60
floor_m = 0
inclusion = [[37.4130, -122.1020], [37.4140, -122.1020], [37.4140, -122.1010], [37.4130, -122.1010]]
exclusion = []

[[rally]]
seq = 0
lat = 37.4135
lon = -122.1015
alt_m = 0.0
TOML
}

gen_mission_toml "alpha" "$WORK/alpha.toml"
gen_mission_toml "alpha-v2" "$WORK/alpha-v2.toml"
gen_mission_toml "beta" "$WORK/beta.toml"

# ----- start catalog -----
echo "[G-2] binary:   $CATALOG_BIN"
echo "[G-2] port:     :$PORT_CATALOG"
echo "[G-2] cat dir:  $CATALOG_DIR"
echo "[G-2] work dir: $WORK"

"$CATALOG_BIN" --port "$PORT_CATALOG" --catalog-dir "$CATALOG_DIR" \
    --fleet-url "http://127.0.0.1:8400" >"$FC_LOG_A" 2>&1 &
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
[ -n "$ok" ] || fail "fleet-catalog did not come up on :$PORT_CATALOG (phase A)"

# ----- (a) POST /api/missions  → id1 + version 1 -----
RESP=$(curl -s --max-time 3 -X POST "http://127.0.0.1:$PORT_CATALOG/api/missions" \
    --data-binary "@$WORK/alpha.toml")
ID1=$(echo "$RESP" | jq -r '.data.mission.id // empty')
[ -n "$ID1" ] || fail "POST alpha did not return id; resp: $RESP"
V1=$(echo "$RESP" | jq -r '.data.mission.version')
[ "$V1" = "1" ] || fail "POST alpha returned version $V1, expected 1"
echo "[G-2] (a) POST alpha → id1=$ID1, version=$V1"

# ----- (b) GET /api/missions → 1 mission, name "alpha" -----
LIST=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions")
N=$(echo "$LIST" | jq 'length')
[ "$N" = "1" ] || fail "list after alpha POST has $N missions, expected 1; list: $LIST"
LIST_NAME=$(echo "$LIST" | jq -r '.[0].name')
[ "$LIST_NAME" = "alpha" ] || fail "list[0].name=$LIST_NAME, expected 'alpha'"
LIST_DELETED=$(echo "$LIST" | jq -r '.[0].deleted')
[ "$LIST_DELETED" = "false" ] || fail "list[0].deleted=$LIST_DELETED, expected false"
echo "[G-2] (b) list → 1 mission, name 'alpha', deleted=false"

# ----- (c) GET /api/missions/{id1} → name "alpha", version 1 -----
GOT=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions/$ID1")
GOT_NAME=$(echo "$GOT" | jq -r '.data.mission.name')
GOT_VER=$(echo "$GOT" | jq -r '.data.mission.version')
[ "$GOT_NAME" = "alpha" ] || fail "fetch id1 name=$GOT_NAME, expected 'alpha'"
[ "$GOT_VER" = "1" ] || fail "fetch id1 version=$GOT_VER, expected 1"
echo "[G-2] (c) fetch id1 → name 'alpha', version 1"

# ----- (d) PUT /api/missions/{id1} → version 2 -----
PUT_RESP=$(curl -s --max-time 3 -X PUT "http://127.0.0.1:$PORT_CATALOG/api/missions/$ID1" \
    --data-binary "@$WORK/alpha-v2.toml")
PUT_VER=$(echo "$PUT_RESP" | jq -r '.data.mission.version')
[ "$PUT_VER" = "2" ] || fail "PUT id1 returned version $PUT_VER, expected 2; resp: $PUT_RESP"
echo "[G-2] (d) PUT id1 (rename to alpha-v2) → version $PUT_VER"

# ----- (e) GET /api/missions/{id1} → name "alpha-v2", version 2 -----
GOT=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions/$ID1")
GOT_NAME=$(echo "$GOT" | jq -r '.data.mission.name')
GOT_VER=$(echo "$GOT" | jq -r '.data.mission.version')
[ "$GOT_NAME" = "alpha-v2" ] || fail "post-PUT fetch name=$GOT_NAME, expected 'alpha-v2'"
[ "$GOT_VER" = "2" ] || fail "post-PUT fetch version=$GOT_VER, expected 2"
echo "[G-2] (e) fetch id1 → name 'alpha-v2', version 2"

# ----- (f) GET /api/missions/{id1}/versions/1 → name "alpha" (old version retained) -----
GOT_V1=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions/$ID1/versions/1")
V1_NAME=$(echo "$GOT_V1" | jq -r '.data.mission.name')
V1_VER=$(echo "$GOT_V1" | jq -r '.data.mission.version')
[ "$V1_NAME" = "alpha" ] || fail "versions/1 name=$V1_NAME, expected 'alpha'"
[ "$V1_VER" = "1" ] || fail "versions/1 version=$V1_VER, expected 1"
echo "[G-2] (f) fetch id1 versions/1 → name 'alpha', version 1 (old version retained)"

# ----- (g) POST /api/missions → id2 ("beta") -----
RESP=$(curl -s --max-time 3 -X POST "http://127.0.0.1:$PORT_CATALOG/api/missions" \
    --data-binary "@$WORK/beta.toml")
ID2=$(echo "$RESP" | jq -r '.data.mission.id // empty')
[ -n "$ID2" ] || fail "POST beta did not return id; resp: $RESP"
[ "$ID2" != "$ID1" ] || fail "POST beta returned same id as alpha ($ID1)"
echo "[G-2] (g) POST beta → id2=$ID2"

# ----- (h) GET /api/missions → 2 missions -----
LIST=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions")
N=$(echo "$LIST" | jq 'length')
[ "$N" = "2" ] || fail "list after beta POST has $N missions, expected 2"
echo "[G-2] (h) list → $N missions (alpha + beta)"

# ----- (i) DELETE /api/missions/{id1} → ok:true -----
DEL=$(curl -s --max-time 3 -X DELETE "http://127.0.0.1:$PORT_CATALOG/api/missions/$ID1")
DEL_OK=$(echo "$DEL" | jq -r '.ok // empty')
[ "$DEL_OK" = "true" ] || fail "DELETE id1 returned ok=$DEL_OK, expected true; resp: $DEL"
echo "[G-2] (i) DELETE id1 → ok"

# ----- (j) GET /api/missions → 1 mission (alpha excluded) -----
LIST=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions")
N=$(echo "$LIST" | jq 'length')
[ "$N" = "1" ] || fail "list after delete has $N missions, expected 1"
LIST_ID=$(echo "$LIST" | jq -r '.[0].id')
[ "$LIST_ID" = "$ID2" ] || fail "list[0].id=$LIST_ID, expected id2 ($ID2) (alpha should be excluded)"
echo "[G-2] (j) list → 1 mission (beta), alpha excluded"

# ----- (k) GET /api/missions?include_deleted=true → 2 missions, alpha has deleted:true -----
LIST=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions?include_deleted=true")
N=$(echo "$LIST" | jq 'length')
[ "$N" = "2" ] || fail "include_deleted=true list has $N missions, expected 2"
ALPHA_DELETED=$(echo "$LIST" | jq -r --arg id "$ID1" \
    '.[] | select(.id == $id) | .deleted')
[ "$ALPHA_DELETED" = "true" ] \
    || fail "include_deleted list: alpha deleted=$ALPHA_DELETED, expected true; list: $LIST"
BETA_DELETED=$(echo "$LIST" | jq -r --arg id "$ID2" \
    '.[] | select(.id == $id) | .deleted')
[ "$BETA_DELETED" = "false" ] \
    || fail "include_deleted list: beta deleted=$BETA_DELETED, expected false"
echo "[G-2] (k) include_deleted=true → 2 missions, alpha.deleted=true, beta.deleted=false"

# ----- (l) RESTART catalog (kill + restart with same dir) -----
echo "[G-2] (l) restarting catalog with same catalog dir (persistence test)..."
kill "$FC_PID" 2>/dev/null || true
for _ in $(seq 1 20); do
    kill -0 "$FC_PID" 2>/dev/null || break
    sleep 0.1
done
kill -9 "$FC_PID" 2>/dev/null || true
FC_PID=""

sleep 0.5   # let the OS free the port

"$CATALOG_BIN" --port "$PORT_CATALOG" --catalog-dir "$CATALOG_DIR" \
    --fleet-url "http://127.0.0.1:8400" >"$FC_LOG_B" 2>&1 &
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
[ -n "$ok" ] || fail "fleet-catalog did not come up after restart (phase B)"

# ----- (m) GET /api/missions → 1 mission (beta) — survived restart -----
LIST=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions")
N=$(echo "$LIST" | jq 'length')
[ "$N" = "1" ] || fail "post-restart list has $N missions, expected 1 (beta only)"
LIST_ID=$(echo "$LIST" | jq -r '.[0].id')
[ "$LIST_ID" = "$ID2" ] || fail "post-restart list[0].id=$LIST_ID, expected id2 ($ID2)"
echo "[G-2] (m) post-restart list → 1 mission (beta), persistence survived"

# ----- (n) GET /api/missions/{id2} → name "beta" -----
GOT=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/missions/$ID2")
GOT_NAME=$(echo "$GOT" | jq -r '.data.mission.name')
[ "$GOT_NAME" = "beta" ] || fail "post-restart fetch id2 name=$GOT_NAME, expected 'beta'"
echo "[G-2] (n) fetch id2 → name 'beta'"

echo "[G-2] PASS"
exit 0
