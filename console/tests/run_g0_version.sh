#!/usr/bin/env bash
# =============================================================================
# G-0: PX4 version check (GCS_SPEC.md §9, ADR-0029).
#
# Single-invocation harness: start mock :8400 + real :8301 catalog, assert the
# version-gate behaviour, tear down, exit 0 on PASS / non-zero on FAIL.
#
# Asserts:
#   (a) :8300 rejects an upload to a vehicle reporting a PX4 version other
#       than the pinned v1.16.2 → HTTP 426 + error.code = PX4_VERSION_MISMATCH
#       (mock :8400 returns px4_version = "v1.18.0").
#   (b) :8300 accepts the upload (HTTP 200) when the vehicle reports v1.16.2
#       (mock :8401 returns px4_version = "v1.16.2" and the catalog is
#       restarted with --fleet-url http://127.0.0.1:8401).
#
# No real PX4 is required — this gate tests the version-check logic only;
# the actual MAVLink upload is M2/G-3.
#
# Environment overrides:
#   CATALOG_BIN  (default <repo>/fleet/target/debug/fleet-catalog)
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CATALOG_BIN="${CATALOG_BIN:-$ROOT/fleet/target/debug/fleet-catalog}"
PINNED_VERSION="v1.16.2"

PORT_CATALOG_A=8301
PORT_MOCK_BAD=8400
PORT_MOCK_OK=8401

WORK="$(mktemp -d -t g0_test.XXXXXX.dir)"
CATALOG_DIR_A="$WORK/cat-a"
CATALOG_DIR_B="$WORK/cat-b"
MOCK_BAD_LOG="$WORK/mock-bad.log"
MOCK_OK_LOG="$WORK/mock-ok.log"
FC_A_LOG="$WORK/fc-a.log"
FC_B_LOG="$WORK/fc-b.log"
MISSION_TOML="$WORK/mission.toml"
MOCK_PY="$WORK/mock_fleet.py"

# PIDs to clean up.
FC_PID=""
MOCK_BAD_PID=""
MOCK_OK_PID=""

cleanup() {
    for pid in "$FC_PID" "$MOCK_BAD_PID" "$MOCK_OK_PID"; do
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            for _ in $(seq 1 20); do
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.1
            done
            kill -9 "$pid" 2>/dev/null || true
        fi
    done
    wait 2>/dev/null || true
    rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

fail() {
    echo "[G-0] FAIL: $1"
    echo "=========== diagnostics ==========="
    echo "--- mock-bad.log (tail 5):"
    tail -5 "$MOCK_BAD_LOG" 2>/dev/null || echo "(none)"
    echo "--- mock-ok.log (tail 5):"
    tail -5 "$MOCK_OK_LOG" 2>/dev/null || echo "(none)"
    echo "--- fc-a.log (tail 10):"
    tail -10 "$FC_A_LOG" 2>/dev/null || echo "(none)"
    echo "--- fc-b.log (tail 10):"
    tail -10 "$FC_B_LOG" 2>/dev/null || echo "(none)"
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -x "$CATALOG_BIN" ] || fail "fleet-catalog binary missing at $CATALOG_BIN (build fleet workspace first)"
command -v curl >/dev/null || fail "curl not available"
command -v jq   >/dev/null || fail "jq not available"
command -v python3 >/dev/null || fail "python3 not available"

# Refuse to run if either port is already taken (avoid cross-test interference).
for p in "$PORT_CATALOG_A" "$PORT_MOCK_BAD" "$PORT_MOCK_OK"; do
    if curl -s --max-time 1 "http://127.0.0.1:$p/" >/dev/null 2>&1; then
        fail "port $p already in use (previous run?)"
    fi
done

# -----------------------------------------------------------------------------
# Mission body: 1 waypoint strictly inside a 4-vertex inclusion fence.
# (Top-level [[waypoints]] per ADR-0019; the [mission] id is required by the
# parser but the server assigns its own ULID and ignores our value.)
# -----------------------------------------------------------------------------
cat > "$MISSION_TOML" <<'TOML'
[mission]
id = "g0-placeholder"
name = "g0-version-probe"
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

# -----------------------------------------------------------------------------
# Mock fleet manager: GET /api/vehicles/<i> returns the supplied px4_version.
# Serves exactly one version (passed as argv[2]); other paths return 404.
# -----------------------------------------------------------------------------
cat > "$MOCK_PY" <<'PYEOF'
import json, http.server, socketserver, sys

PORT = int(sys.argv[1])
VERSION = sys.argv[2]

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _send(self, code, body):
        b = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        if self.path.startswith("/api/vehicles/"):
            self._send(200, {"ok": True, "data": {"px4_version": VERSION}})
        else:
            self._send(404, {"ok": False, "error": {"code": "NOT_FOUND"}})

# allow_reuse_address sets SO_REUSEADDR so a quick restart (e.g. back-to-back
# G-0 invocations) does not trip over TIME_WAIT sockets left by the previous
# mock server's closed connections.
class ReusableTCPServer(socketserver.TCPServer):
    allow_reuse_address = True

with ReusableTCPServer(("127.0.0.1", PORT), H) as srv:
    srv.serve_forever()
PYEOF

echo "[G-0] binary:    $CATALOG_BIN"
echo "[G-0] pinned PX4: $PINNED_VERSION"
echo "[G-0] work dir:  $WORK"

# -----------------------------------------------------------------------------
# Phase 1: bad version (v1.18.0) → expect HTTP 426 + PX4_VERSION_MISMATCH.
# -----------------------------------------------------------------------------
echo "[G-0] phase 1: mock fleet at :$PORT_MOCK_BAD returns PX4 v1.18.0 → expect 426"

python3 "$MOCK_PY" "$PORT_MOCK_BAD" "v1.18.0" >"$MOCK_BAD_LOG" 2>&1 &
MOCK_BAD_PID=$!
sleep 0.5

mkdir -p "$CATALOG_DIR_A"
"$CATALOG_BIN" --port "$PORT_CATALOG_A" --catalog-dir "$CATALOG_DIR_A" \
    --fleet-url "http://127.0.0.1:$PORT_MOCK_BAD" >"$FC_A_LOG" 2>&1 &
FC_PID=$!

# Wait for the catalog health endpoint.
ok=""
for _ in $(seq 1 50); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG_A/api/health" \
            | grep -q '"service":"fleet-catalog"'; then
        ok=1; break
    fi
    kill -0 "$FC_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "fleet-catalog did not come up on :$PORT_CATALOG_A"

# Create the mission; capture the server-assigned ULID.
RESP=$(curl -s --max-time 3 -X POST "http://127.0.0.1:$PORT_CATALOG_A/api/missions" \
    --data-binary "@$MISSION_TOML")
MISSION_ID=$(echo "$RESP" | jq -r '.data.mission.id // empty')
[ -n "$MISSION_ID" ] || fail "POST /api/missions did not return an id; resp: $RESP"
echo "[G-0] created mission: $MISSION_ID"

# Trigger the upload; capture HTTP code + JSON body separately.
HTTP_FILE="$WORK/http_code"
BODY_FILE="$WORK/upload_bad.json"
curl -s --max-time 5 -o "$BODY_FILE" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_CATALOG_A/api/vehicles/0/mission/upload?mission_id=$MISSION_ID" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
ERR_CODE=$(jq -r '.error.code // empty' "$BODY_FILE" 2>/dev/null || echo "")

echo "[G-0] upload to v1.18.0 vehicle → HTTP $HTTP_CODE, error.code=$ERR_CODE"

if [ "$HTTP_CODE" != "426" ]; then
    fail "expected HTTP 426 for v1.18.0 vehicle, got $HTTP_CODE; body: $(cat "$BODY_FILE")"
fi
if [ "$ERR_CODE" != "PX4_VERSION_MISMATCH" ]; then
    fail "expected error.code PX4_VERSION_MISMATCH, got '$ERR_CODE'; body: $(cat "$BODY_FILE")"
fi

# Also assert the reported_version and required_version are present in details.
REPORTED=$(jq -r '.error.details.reported_version // empty' "$BODY_FILE")
REQUIRED=$(jq -r '.error.details.required_version // empty' "$BODY_FILE")
[ "$REPORTED" = "v1.18.0" ] || fail "reported_version mismatch: '$REPORTED' != v1.18.0"
[ "$REQUIRED" = "$PINNED_VERSION" ] || fail "required_version mismatch: '$REQUIRED' != $PINNED_VERSION"

echo "[G-0] phase 1 PASS: HTTP 426 + PX4_VERSION_MISMATCH (reported v1.18.0, required $PINNED_VERSION)"

# -----------------------------------------------------------------------------
# Phase 2: switch to a mock returning v1.16.2; restart catalog against the same
# mission (re-create on a fresh catalog dir, since the first one is throwaway).
# The catalog's fleet-url flag changes which mock it queries.
# -----------------------------------------------------------------------------
echo "[G-0] phase 2: mock fleet at :$PORT_MOCK_OK returns PX4 v1.16.2 → expect 200"

# Tear down phase 1.
kill "$FC_PID" 2>/dev/null || true
for _ in $(seq 1 20); do kill -0 "$FC_PID" 2>/dev/null || break; sleep 0.1; done
kill -9 "$FC_PID" 2>/dev/null || true
FC_PID=""
kill "$MOCK_BAD_PID" 2>/dev/null || true
for _ in $(seq 1 20); do kill -0 "$MOCK_BAD_PID" 2>/dev/null || break; sleep 0.1; done
kill -9 "$MOCK_BAD_PID" 2>/dev/null || true
MOCK_BAD_PID=""

# Start the v1.16.2 mock on the second port.
python3 "$MOCK_PY" "$PORT_MOCK_OK" "v1.16.2" >"$MOCK_OK_LOG" 2>&1 &
MOCK_OK_PID=$!
sleep 0.5

# Restart catalog pointing at the OK mock; reuse a fresh catalog dir so the
# mission needs to be re-created (the catalog dir is the persistence layer,
# not the URL — phase 2 is a clean-slate version-OK test).
mkdir -p "$CATALOG_DIR_B"
"$CATALOG_BIN" --port "$PORT_CATALOG_A" --catalog-dir "$CATALOG_DIR_B" \
    --fleet-url "http://127.0.0.1:$PORT_MOCK_OK" >"$FC_B_LOG" 2>&1 &
FC_PID=$!

ok=""
for _ in $(seq 1 50); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG_A/api/health" \
            | grep -q '"service":"fleet-catalog"'; then
        ok=1; break
    fi
    kill -0 "$FC_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "fleet-catalog did not come up on :$PORT_CATALOG_A (phase 2)"

RESP=$(curl -s --max-time 3 -X POST "http://127.0.0.1:$PORT_CATALOG_A/api/missions" \
    --data-binary "@$MISSION_TOML")
MISSION_ID=$(echo "$RESP" | jq -r '.data.mission.id // empty')
[ -n "$MISSION_ID" ] || fail "POST /api/missions did not return an id (phase 2); resp: $RESP"
echo "[G-0] created mission (phase 2): $MISSION_ID"

HTTP_FILE="$WORK/http_code_ok"
BODY_FILE="$WORK/upload_ok.json"
curl -s --max-time 5 -o "$BODY_FILE" -w "%{http_code}" \
    -X POST "http://127.0.0.1:$PORT_CATALOG_A/api/vehicles/0/mission/upload?mission_id=$MISSION_ID" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
OK_FIELD=$(jq -r '.ok // empty' "$BODY_FILE" 2>/dev/null || echo "")
STATUS=$(jq -r '.data.status // empty' "$BODY_FILE" 2>/dev/null || echo "")

echo "[G-0] upload to v1.16.2 vehicle → HTTP $HTTP_CODE, ok=$OK_FIELD, status=$STATUS"

if [ "$HTTP_CODE" != "200" ]; then
    fail "expected HTTP 200 for v1.16.2 vehicle, got $HTTP_CODE; body: $(cat "$BODY_FILE")"
fi
if [ "$OK_FIELD" != "true" ]; then
    fail "expected ok=true for v1.16.2 upload, got ok=$OK_FIELD; body: $(cat "$BODY_FILE")"
fi
# Sanity: the catalog should have recorded the OK version check in the body.
REPORTED=$(jq -r '.data.version_check.reported_version // empty' "$BODY_FILE")
REQUIRED=$(jq -r '.data.version_check.required_version // empty' "$BODY_FILE")
[ "$REPORTED" = "$PINNED_VERSION" ] || fail "reported_version mismatch (phase 2): '$REPORTED' != $PINNED_VERSION"
[ "$REQUIRED" = "$PINNED_VERSION" ] || fail "required_version mismatch (phase 2): '$REQUIRED' != $PINNED_VERSION"

echo "[G-0] phase 2 PASS: HTTP 200, version_check.status=Ok (vehicle reports $PINNED_VERSION)"

echo "[G-0] PASS"
exit 0
