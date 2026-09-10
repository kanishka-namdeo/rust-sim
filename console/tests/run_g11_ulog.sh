#!/usr/bin/env bash
# =============================================================================
# G-11: ULog browse (GCS_SPEC.md §9, gate G-11).
#
# Single-invocation harness: start fleet-catalog on :8300 with a temp
# catalog dir, drop a stub .ulg file into the catalog's `ulogs/` dir,
# exercise the ULog browse endpoints, assert each returns valid JSON
# with the expected shape, tear down, exit 0 on PASS / non-zero on FAIL.
#
# Per the task spec (Task 7a/7b cleanup, 2026-09-10), this gate now covers:
#   - GET /api/ulogs              → list (the stub .ulg file)
#   - GET /api/ulogs/{file}/topics → 404 if file missing (pyulog not
#                                    required; we don't have a real
#                                    .ulg in the sandbox)
#
# The .replay browse endpoints were removed when the custom-sim replay
# format left the GCS surface (the Analyze overlay's replays tab is gone).
# ULog (.ulg) is the canonical PX4 analysis format.
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CATALOG_BIN="${CATALOG_BIN:-$ROOT/fleet/target/debug/fleet-catalog}"

PORT_CATALOG=8300
WORK="$(mktemp -d -t g11_test.XXXXXX.dir)"
CATALOG_DIR="$WORK/catalog"
CATALOG_LOG="$WORK/catalog.log"
ULOG_FILE="g11_sample.ulg"

CATALOG_PID=""

cleanup() {
    if [ -n "$CATALOG_PID" ] && kill -0 "$CATALOG_PID" 2>/dev/null; then
        kill "$CATALOG_PID" 2>/dev/null || true
        for _ in $(seq 1 20); do
            kill -0 "$CATALOG_PID" 2>/dev/null || break
            sleep 0.1
        done
        kill -9 "$CATALOG_PID" 2>/dev/null || true
    fi
    wait 2>/dev/null || true
    if [ "${G11_KEEP:-0}" = "1" ]; then
        echo "[G-11] keeping work dir: $WORK"
    else
        rm -rf "$WORK" 2>/dev/null || true
    fi
}
trap cleanup EXIT

fail() {
    echo "[G-11] FAIL: $1"
    echo "=========== diagnostics ==========="
    echo "--- catalog.log (tail 20):"
    tail -20 "$CATALOG_LOG" 2>/dev/null || echo "(none)"
    echo "--- work dir: $WORK"
    G11_KEEP=1
    exit 1
}

# -----------------------------------------------------------------------------
# Preconditions.
# -----------------------------------------------------------------------------
[ -x "$CATALOG_BIN" ] || fail "fleet-catalog binary missing at $CATALOG_BIN (build fleet workspace first)"
command -v curl >/dev/null || fail "curl not available"
command -v python3 >/dev/null || fail "python3 not available"

# Refuse to run if port already taken.
if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" >/dev/null 2>&1; then
    fail "port $PORT_CATALOG already in use (previous run?)"
fi

# -----------------------------------------------------------------------------
# 1. Start fleet-catalog on :8300 with a temp catalog dir.
# -----------------------------------------------------------------------------
echo "[G-11] binary:    $CATALOG_BIN"
echo "[G-11] work dir:  $WORK"
echo "[G-11] starting fleet-catalog on :$PORT_CATALOG"

"$CATALOG_BIN" --port "$PORT_CATALOG" --catalog-dir "$CATALOG_DIR" \
    >"$CATALOG_LOG" 2>&1 &
CATALOG_PID=$!

# Wait for the catalog health endpoint.
ok=""
for _ in $(seq 1 50); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT_CATALOG/api/health" \
            | grep -q '"ok":true'; then
        ok=1; break
    fi
    kill -0 "$CATALOG_PID" 2>/dev/null || break
    sleep 0.1
done
[ -n "$ok" ] || fail "fleet-catalog did not come up on :$PORT_CATALOG"
echo "[G-11] catalog up (pid $CATALOG_PID)"

# Drop a stub .ulg file (1 byte — not a real ULog, but listable on the
# filesystem). pyulog is NOT installed in the sandbox, so we cannot
# exercise the topics/data endpoints with a real .ulg; the G-11 task
# spec explicitly allows this and limits assertions to (a) the list
# endpoint and (b) the 404 path for nonexistent files.
mkdir -p "$CATALOG_DIR/ulogs"
printf '\x00' > "$CATALOG_DIR/ulogs/$ULOG_FILE"

echo "[G-11] seeded $ULOG_FILE (stub)"

# -----------------------------------------------------------------------------
# 2. GET /api/ulogs → assert list contains our stub .ulg file.
# -----------------------------------------------------------------------------
echo "[G-11] test 1: GET /api/ulogs"
RESP=$(curl -s --max-time 3 "http://127.0.0.1:$PORT_CATALOG/api/ulogs")
echo "$RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d.get('ok') is True, f'ok field missing/false: {d}'
files = d.get('data', [])
assert any(f['filename'] == 'g11_sample.ulg' for f in files), \
    f'g11_sample.ulg not in list: {files}'
# Each entry should have filename + size_bytes + mtime
for f in files:
    assert 'filename' in f, f'missing filename: {f}'
    assert 'size_bytes' in f, f'missing size_bytes: {f}'
print('OK', file=sys.stderr)
" 2>&1 || fail "test 1: /api/ulogs response invalid; resp: $RESP"
echo "[G-11]   ✓ /api/ulogs lists g11_sample.ulg"

# -----------------------------------------------------------------------------
# 3. GET /api/ulogs/nonexistent.ulg/topics → 404 (file not found).
# -----------------------------------------------------------------------------
echo "[G-11] test 2: GET /api/ulogs/nonexistent.ulg/topics → 404"
HTTP_FILE="$WORK/http_code"
BODY_FILE="$WORK/ulog_404.json"
curl -s --max-time 3 -o "$BODY_FILE" -w "%{http_code}" \
    "http://127.0.0.1:$PORT_CATALOG/api/ulogs/nonexistent.ulg/topics" \
    >"$HTTP_FILE" || true
HTTP_CODE=$(cat "$HTTP_FILE")
ERR_CODE=$(python3 -c "import json; d=json.load(open('$BODY_FILE')); print(d.get('error',{}).get('code',''))" 2>/dev/null || echo "")
[ "$HTTP_CODE" = "404" ] || fail "test 2: expected HTTP 404 for nonexistent .ulg, got $HTTP_CODE; body: $(cat "$BODY_FILE")"
[ "$ERR_CODE" = "ULOG_NOT_FOUND" ] || fail "test 2: expected error.code=ULOG_NOT_FOUND, got '$ERR_CODE'; body: $(cat "$BODY_FILE")"
echo "[G-11]   ✓ 404 ULOG_NOT_FOUND for nonexistent.ulg"

# -----------------------------------------------------------------------------
# Done.
# -----------------------------------------------------------------------------
echo "[G-11] PASS"
exit 0
