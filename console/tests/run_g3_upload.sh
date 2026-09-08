#!/usr/bin/env bash
# =============================================================================
# G-3: MAVLink mission upload protocol — count → items → ack for all three
# MAV_MISSION_TYPE values; all items ack'd by PX4; rollback on failure
# (GCS_SPEC.md §9, gate G-3).
#
# Single-invocation harness: start a Python mock PX4 (pymavlink 2.4.49 v2.0
# common dialect, the same wire format the Rust fleet-mavlink codec is pinned
# to in messages.rs), drive the GCS side with a pymavlink test client, and
# assert every step of the MAVLink mission-protocol upload round-trips
# successfully.
#
# Wire topology (matches fleet-mavlink/src/link.rs §3.1):
#   GCS link binds 127.0.0.1:14540 (vehicle 0)
#   PX4 (mock) binds 127.0.0.1:14580 (PX4 onboard listen port)
#   Both directions: GCS sends MISSION_COUNT etc. to :14580; mock replies
#   (MISSION_REQUEST_INT / MISSION_ACK) to the source of the latest GCS
#   datagram — which is :14540 in the Rust link's case, or an ephemeral port
#   for a pymavlink GCS.
#
# Asserts:
#   (a) Upload 3 mission items of MAV_MISSION_TYPE=0 (mission) → MISSION_ACK(0)
#   (b) Upload 3 mission items of MAV_MISSION_TYPE=1 (fence)   → MISSION_ACK(0)
#   (c) Upload 3 mission items of MAV_MISSION_TYPE=2 (rally)   → MISSION_ACK(0)
#   (d) Rollback probe: upload 1 item with command=999 → MISSION_ACK(3)
#       (MAV_MISSION_UNSUPPORTED), and the mock discards the partial upload.
#   (e) Mock's state.json shows 3 received items per mission_type (0/1/2), and
#       4 entries in transaction_log (3 accept + 1 unsupported).
#
# Why a Python GCS instead of the Rust fleet-cli link:
#   The :8400 `/api/vehicles/0/mission/upload` HTTP endpoint that drives the
#   Rust `LinkHandle::mission_upload()` is being built in parallel by another
#   agent (M2-API) and isn't ready at harness-author time. The wire protocol
#   is what G-3 verifies, and the Rust codec is independently pinned by
#   `tests/golden.rs` byte-vectors — so a pymavlink GCS talking to this mock
#   is the spec-endorsed alternative ("Test the mock PX4 directly with a
#   Python test script"). When M2-API lands, the same mock PX4 will serve a
#   parallel end-to-end Rust test without modification (port convention is
#   identical to LinkConfig::for_instance(0)).
#
# Environment overrides:
#   PY         (default python3)
#   MOCK_BIN   (default <repo>/console/tests/mock_px4_mission.py)
#   GCS_BIN    (default <repo>/console/tests/gcs_mission_client.py)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PY="${PY:-python3}"
MOCK_BIN="${MOCK_BIN:-$ROOT/console/tests/mock_px4_mission.py}"
GCS_BIN="${GCS_BIN:-$ROOT/console/tests/gcs_mission_client.py}"

# Ports per fleet-mavlink/src/link.rs LinkConfig::for_instance(0).
PORT_GCS_BIND=14540   # Rust link's bind port (GCS receives heartbeats here)
PORT_PX4_LISTEN=14580 # PX4 onboard listen port (mock binds here)

WORK="$(mktemp -d -t g3_test.XXXXXX.dir)"
STATE_JSON="$WORK/state.json"
GCS_JSON="$WORK/gcs.json"
MOCK_LOG="$WORK/mock.log"
GCS_LOG="$WORK/gcs.log"

MOCK_PID=""

cleanup() {
    if [ -n "$MOCK_PID" ] && kill -0 "$MOCK_PID" 2>/dev/null; then
        kill "$MOCK_PID" 2>/dev/null || true
        for _ in $(seq 1 20); do
            kill -0 "$MOCK_PID" 2>/dev/null || break
            sleep 0.1
        done
        kill -9 "$MOCK_PID" 2>/dev/null || true
    fi
    wait 2>/dev/null || true
    rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

fail() {
    echo "[G-3] FAIL: $1"
    echo "=========== diagnostics ==========="
    echo "--- mock.log (tail 30):"
    tail -30 "$MOCK_LOG" 2>/dev/null || echo "(none)"
    echo "--- gcs.log (tail 20):"
    tail -20 "$GCS_LOG" 2>/dev/null || echo "(none)"
    echo "--- gcs.json:"
    cat "$GCS_JSON" 2>/dev/null | python3 -m json.tool 2>/dev/null \
        || cat "$GCS_JSON" 2>/dev/null || echo "(none)"
    echo "--- state.json (transaction_log + received_items counts):"
    if [ -f "$STATE_JSON" ]; then
        jq '{transaction_log, counts: (.received_items | map_values(length))}' \
            "$STATE_JSON" 2>/dev/null || cat "$STATE_JSON"
    else
        echo "(none)"
    fi
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -x "$MOCK_BIN" ] || [ -f "$MOCK_BIN" ] || fail "mock_px4_mission.py missing at $MOCK_BIN"
[ -f "$GCS_BIN" ] || fail "gcs_mission_client.py missing at $GCS_BIN"
command -v "$PY" >/dev/null || fail "python3 not available"
command -v jq >/dev/null || fail "jq not available"
"$PY" -c "from pymavlink.dialects.v20 import common; print('pymavlink OK')" \
    >/dev/null 2>&1 || fail "pymavlink not importable by $PY"

# Refuse to run if either UDP port is already taken.
for p in "$PORT_GCS_BIND" "$PORT_PX4_LISTEN"; do
    # Quick test bind — if it fails, the port is taken.
    if ! "$PY" -c "import socket,sys; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); sys.exit(0 if s.bind(('127.0.0.1', $p))==None else 1)" 2>/dev/null; then
        fail "UDP port $p already in use (previous run?)"
    fi
done

echo "[G-3] mock:     $MOCK_BIN"
echo "[G-3] gcs:      $GCS_BIN"
echo "[G-3] ports:    GCS bind :$PORT_GCS_BIND, PX4 listen :$PORT_PX4_LISTEN"
echo "[G-3] work dir: $WORK"

# -----------------------------------------------------------------------------
# Phase 1: start the mock PX4. It binds :14580 and sends heartbeats to :14540.
# -----------------------------------------------------------------------------
"$PY" "$MOCK_BIN" \
    --state "$STATE_JSON" \
    --port "$PORT_PX4_LISTEN" \
    --gcs-port "$PORT_GCS_BIND" \
    --ttl-secs 60 >"$MOCK_LOG" 2>&1 &
MOCK_PID=$!

# Wait for the state file to appear (mock ready signal).
ok=""
for _ in $(seq 1 50); do
    [ -f "$STATE_JSON" ] && { ok=1; break; }
    kill -0 "$MOCK_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "mock PX4 did not write state file within 5s"

echo "[G-3] phase 1: mock PX4 up (PID $MOCK_PID)"

# -----------------------------------------------------------------------------
# Phase 2: run the GCS upload probe (3 mission types + rollback).
# -----------------------------------------------------------------------------
"$PY" "$GCS_BIN" upload \
    --bind-port "$PORT_GCS_BIND" \
    --px4-port "$PORT_PX4_LISTEN" \
    --timeout 8 >"$GCS_JSON" 2>"$GCS_LOG" || true

# The GCS client exits 0 on success, non-zero on failure. Either way, the JSON
# summary line on stdout tells us exactly which assertions failed.
[ -s "$GCS_JSON" ] || fail "GCS client produced no output (see $GCS_LOG)"

GCS_OK=$(jq -r '.ok // false' "$GCS_JSON")
GCS_HB=$(jq -r '.heartbeat // false' "$GCS_JSON")
[ "$GCS_HB" = "true" ] || fail "GCS never received a heartbeat from the mock PX4"

# Inspect each transaction's ack result.
declare -A EXPECTED_RESULT=( [0]=0 [1]=0 [2]=0 )
for t in 0 1 2; do
    RESULT=$(jq -r ".transactions[] | select(.type == $t) | .ack.result" "$GCS_JSON")
    ACKED=$(jq -r ".transactions[] | select(.type == $t) | .ack.acked" "$GCS_JSON")
    ITEMS=$(jq -r ".transactions[] | select(.type == $t) | .items_count" "$GCS_JSON")
    echo "[G-3] type=$t: items=$ITEMS acked=$ACKED result=$RESULT (expected ${EXPECTED_RESULT[$t]})"
    [ "$RESULT" = "${EXPECTED_RESULT[$t]}" ] \
        || fail "mission_type=$t upload result=$RESULT, expected ${EXPECTED_RESULT[$t]}"
    [ "$ACKED" = "true" ] || fail "mission_type=$t upload not acked"
    [ "$ITEMS" = "3" ] || fail "mission_type=$t items_count=$ITEMS, expected 3"
done

# Rollback probe: expect result=3 (MAV_MISSION_UNSUPPORTED).
RB_RESULT=$(jq -r '.transactions[] | select(.type == "rollback") | .ack.result' "$GCS_JSON")
RB_ACKED=$(jq -r '.transactions[] | select(.type == "rollback") | .ack.acked' "$GCS_JSON")
echo "[G-3] rollback probe: acked=$RB_ACKED result=$RB_RESULT (expected 3 = MAV_MISSION_UNSUPPORTED)"
[ "$RB_RESULT" = "3" ] \
    || fail "rollback probe result=$RB_RESULT, expected 3 (MAV_MISSION_UNSUPPORTED)"
[ "$RB_ACKED" = "true" ] || fail "rollback probe was not acked"

# -----------------------------------------------------------------------------
# Phase 3: cross-check the mock's state.json — every item the GCS sent must
# have been received and stored by the mock.
# -----------------------------------------------------------------------------
# Mock must have 3 received_items per type (0/1/2). The rollback probe (cmd=999)
# must have been discarded (received_items[0] still has the original 3).
N_TYPE0=$(jq -r '.received_items["0"] | length' "$STATE_JSON")
N_TYPE1=$(jq -r '.received_items["1"] | length' "$STATE_JSON")
N_TYPE2=$(jq -r '.received_items["2"] | length' "$STATE_JSON")
echo "[G-3] mock received_items: type0=$N_TYPE0 type1=$N_TYPE1 type2=$N_TYPE2"
[ "$N_TYPE0" = "3" ] || fail "mock received $N_TYPE0 items for mission_type=0 (expected 3 — rollback must not corrupt prior state)"
[ "$N_TYPE1" = "3" ] || fail "mock received $N_TYPE1 items for mission_type=1 (expected 3)"
[ "$N_TYPE2" = "3" ] || fail "mock received $N_TYPE2 items for mission_type=2 (expected 3)"

# Mock's transaction_log must have 4 entries: 3 accepts (result=0) + 1 unsupported (result=3).
N_TXN=$(jq -r '.transaction_log | length' "$STATE_JSON")
echo "[G-3] mock transaction_log: $N_TXN entries (expected 4)"
[ "$N_TXN" = "4" ] || fail "mock transaction_log has $N_TXN entries (expected 4: 3 accepts + 1 unsupported)"

# All 4 must be 'upload' kind; results must be [0, 0, 0, 3] in order.
TXN_KINDS=$(jq -r '.transaction_log | map(.kind) | unique | join(",")' "$STATE_JSON")
[ "$TXN_KINDS" = "upload" ] || fail "unexpected transaction kinds: $TXN_KINDS (expected only 'upload')"

TXN_RESULTS=$(jq -r '.transaction_log | map(.result) | join(",")' "$STATE_JSON")
echo "[G-3] mock transaction results: $TXN_RESULTS (expected 0,0,0,3)"
[ "$TXN_RESULTS" = "0,0,0,3" ] \
    || fail "mock transaction results '$TXN_RESULTS' != expected '0,0,0,3'"

# Spot-check one round-trip item: type=0 seq=0 command=16 x=374133000 y=-1221017000.
ITEM0_X=$(jq -r '.received_items["0"][0].x' "$STATE_JSON")
ITEM0_Y=$(jq -r '.received_items["0"][0].y' "$STATE_JSON")
ITEM0_CMD=$(jq -r '.received_items["0"][0].command' "$STATE_JSON")
echo "[G-3] spot-check type0/seq0: cmd=$ITEM0_CMD x=$ITEM0_X y=$ITEM0_Y"
[ "$ITEM0_CMD" = "16" ] || fail "type0/seq0 command mismatch: $ITEM0_CMD"
[ "$ITEM0_X" = "374133000" ] || fail "type0/seq0 x mismatch: $ITEM0_X"
[ "$ITEM0_Y" = "-1221017000" ] || fail "type0/seq0 y mismatch: $ITEM0_Y"

echo "[G-3] PASS"
exit 0
