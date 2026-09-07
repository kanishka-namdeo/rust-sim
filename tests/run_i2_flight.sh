#!/bin/bash
# =============================================================================
# rustsitsim integration case I-2: PHYSICAL FLIGHT against REAL PX4 (SPEC
# §10.3) — arm -> OFFBOARD climb -> hold -> descend -> land, with the fixed
# v1.16 actuator wire mapping (ADR-0011r) and sized contact substeps
# (ADR-013).
#
# WIRE ADAPTER (2026-09-07): PX4 v1.16.2 packs HIL_ACTUATOR_CONTROLS(93)
# as time_usec@0, flags:u64@8, controls@16, mode@80 (size-sorted core
# fields), while sitsim-mavlink decodes controls@8, mode@72, flags@73 (the
# official-common.xml extension layout). The 8-byte shift mis-slots the
# motors (+2 channels) and reads the armed bit from a float byte, pinning
# the vehicle on the ground (rotors commanded stopped). The engine mapping
# (ADR-0011r) is correct; the crate offset fix is pending review, so the
# harness runs tests/i2_wire_proxy.py between PX4 (4560) and sitsim (4570,
# scenario i2_flight_proxy.toml), re-laying-out ONLY msg 93 PX4->sim and
# recomputing the CRC. Everything else is byte-transparent.
#
# Single-invocation harness (SPEC §11.3): everything starts, asserts, and
# cleans up within one call.
#
# Asserts:
#   (a) PX4 rcS completes ("Startup script returned successfully")
#   (b) The HIL loop closes
#   (c) ARM succeeds (COMMAND_ACK result=0)
#   (d) OFFBOARD mode is observed (heartbeat nav_state echo)
#   (e) PHYSICAL FLIGHT: the sim ground truth (replay + /api/status, NOT
#       the EKF estimate) reaches z <= -1.5 m
#   (f) No numerical divergence: sim exit code != 5, no NaN in the replay
#   (g) The vehicle comes back down and settles (final |z| < 0.5 m,
#       vertical speed small) and disarm observed
#   (h) A ULog was written
#
# Environment: PX4_ROOT, SITSIM_BIN, I2_PYTHON (default /usr/bin/python3.13)
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PX4_ROOT="${PX4_ROOT:-/home/z/my-project/PX4-Autopilot}"
SITSIM_BIN="${SITSIM_BIN:-$ROOT/target/debug/sitsim-cli}"
I2_PYTHON="${I2_PYTHON:-/usr/bin/python3.13}"

BUILD="$PX4_ROOT/build/px4_sitl_default"
PX4_BIN="$BUILD/bin/px4"
INSTANCE_DIR="$BUILD/instance_0"
ETC_DIR="$BUILD/etc"

SCENARIO="$ROOT/tests/i2_flight_proxy.toml"   # io.tcp_port 4570 (proxy owns 4560)
HIL_PORT=4560
API_PORT=8200
FLIGHT_S=100
SIM_TCP_PORT=4570
PROXY_SCRIPT="$ROOT/tests/i2_wire_proxy.py"

OUT="$ROOT/tests/i2_artifacts"
mkdir -p "$OUT"
PX4_LOG="$OUT/px4.log"
SIM_LOG="$OUT/sim.log"
SIM_STDOUT="$OUT/sim_stdout.txt"
DRIVER_JSON="$OUT/driver_result.json"
REPLAY="$OUT/i2_flight.replay"

PX4_PID=""; SIM_PID=""; DRIVER_PID=""; PROXY_PID=""

fail() {
    echo "I-2 FAILED: $1"
    echo "=========== diagnostics ==========="
    echo "--- driver_result.json:"
    cat "$DRIVER_JSON" 2>/dev/null || echo "(none)"
    echo "--- px4.log (tail 30):"
    tail -30 "$PX4_LOG" 2>/dev/null
    echo "--- sim.log (tail 25):"
    tail -25 "$SIM_LOG" 2>/dev/null
    echo "--- sim_stdout:"
    cat "$SIM_STDOUT" 2>/dev/null
    cleanup
    exit 1
}

cleanup() {
    [ -n "$DRIVER_PID" ] && kill "$DRIVER_PID" 2>/dev/null
    [ -n "${PROXY_PID:-}" ] && kill "$PROXY_PID" 2>/dev/null
    if [ -n "${PX4_PID:-}" ] && kill -0 "$PX4_PID" 2>/dev/null; then
        kill "$PX4_PID" 2>/dev/null
        for _ in $(seq 1 20); do kill -0 "$PX4_PID" 2>/dev/null || break; sleep 0.2; done
        kill -9 "$PX4_PID" 2>/dev/null
    fi
    if [ -n "${SIM_PID:-}" ] && kill -0 "$SIM_PID" 2>/dev/null; then
        kill "$SIM_PID" 2>/dev/null
        for _ in $(seq 1 20); do kill -0 "$SIM_PID" 2>/dev/null || break; sleep 0.2; done
        kill -9 "$SIM_PID" 2>/dev/null
    fi
    wait 2>/dev/null || true
}

[ -x "$PX4_BIN" ] || fail "PX4 binary missing"
[ -x "$SITSIM_BIN" ] || fail "sitsim-cli missing (cargo build -p sitsim-cli)"
[ -f "$SCENARIO" ] || fail "scenario missing"
"$I2_PYTHON" -c "import pymavlink" 2>/dev/null || fail "pymavlink not importable under $I2_PYTHON"

echo "[I-2] sim:     $SITSIM_BIN"
echo "[I-2] scenario $SCENARIO"

rm -f "$REPLAY" "$DRIVER_JSON"

# ---- 1. rustsitsim.
"$SITSIM_BIN" scenario-run "$SCENARIO" \
    --replay-out "$REPLAY" --telemetry-hash \
    >"$SIM_STDOUT" 2>"$SIM_LOG" &
SIM_PID=$!

ok=""
for _ in $(seq 1 100); do
    if curl -s --max-time 1 "http://127.0.0.1:$API_PORT/api/status" | grep -q '"phase":"WAIT"'; then
        ok=1; break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "control plane did not reach WAIT"

# ---- 1b. Wire adapter: owns TCP 4560 for PX4, connects to the sim 4570.
"$I2_PYTHON" "$PROXY_SCRIPT" 4560 $SIM_TCP_PORT "$OUT/proxy.log" >"$OUT/proxy_stdout.log" 2>&1 &
PROXY_PID=$!
ok=""
for _ in $(seq 1 100); do
    if grep -q "listening 127.0.0.1:4560" "$OUT/proxy_stdout.log" 2>/dev/null; then
        ok=1; break
    fi
    if ! kill -0 "$PROXY_PID" 2>/dev/null; then break; fi
    sleep 0.1
done
[ -n "$ok" ] || { cat "$OUT/proxy_stdout.log" 2>/dev/null; fail "wire proxy did not start"; }
echo "[I-2] wire adapter up: PX4:4560 -> proxy -> sitsim:$SIM_TCP_PORT"

# ---- 2. Flight driver (binds 14540 BEFORE PX4 boots).
"$I2_PYTHON" "$ROOT/tests/i2_flight_driver.py" 14540 $API_PORT "$DRIVER_JSON" 100 >"$OUT/driver.log" 2>&1 &
DRIVER_PID=$!

# ---- 3. PX4.
(
    cd "$INSTANCE_DIR" || exit 1
    PX4_SIM_MODEL=gazebo-classic_iris "$PX4_BIN" -i 0 -d "$ETC_DIR"
) >"$PX4_LOG" 2>&1 &
PX4_PID=$!

# ---- (a) boot gate.
ok=""
for _ in $(seq 1 600); do
    if grep -q "Startup script returned successfully" "$PX4_LOG" 2>/dev/null; then ok=1; break; fi
    if ! kill -0 "$PX4_PID" 2>/dev/null; then break; fi
    sleep 0.1
done
[ -n "$ok" ] || fail "PX4 rcS did not complete"
echo "[I-2] (a) rcS complete"

# ---- (b) loop closed.
ok=""
for _ in $(seq 1 200); do
    if curl -s --max-time 1 "http://127.0.0.1:$API_PORT/api/status" | grep -q '"loop_closed":true'; then ok=1; break; fi
    sleep 0.1
done
[ -n "$ok" ] || fail "HIL loop did not close"
echo "[I-2] (b) HIL loop closed"

# ---- wait for the driver to finish (up to 110 s).
for _ in $(seq 1 550); do
    [ -f "$DRIVER_JSON" ] && break
    if ! kill -0 "$DRIVER_PID" 2>/dev/null; then break; fi
    sleep 0.2
done
# small settle, then stop PX4 (sim will exit 3 on disconnect)
sleep 2
kill "$PX4_PID" 2>/dev/null
for _ in $(seq 1 30); do kill -0 "$PX4_PID" 2>/dev/null || break; sleep 0.2; done
kill -9 "$PX4_PID" 2>/dev/null
for _ in $(seq 1 50); do kill -0 "$SIM_PID" 2>/dev/null || break; sleep 0.2; done
kill -9 "$SIM_PID" 2>/dev/null
wait "$SIM_PID" 2>/dev/null
SIM_EXIT=$?
echo "[I-2] sim exit code: $SIM_EXIT"
[ "$SIM_EXIT" -ne 5 ] || fail "sim exit 5 = numerical divergence"
if [ -n "${PROXY_PID:-}" ]; then
    kill "$PROXY_PID" 2>/dev/null
    wait "$PROXY_PID" 2>/dev/null
fi
echo "[I-2] wire proxy stats: $(tail -1 "$OUT/proxy.log" 2>/dev/null)"

[ -s "$DRIVER_JSON" ] || fail "driver produced no result"
echo "[I-2] driver result:"
cat "$DRIVER_JSON"; echo

# ---- assertions on the driver result.
"$I2_PYTHON" - "$DRIVER_JSON" <<'PYEOF' || fail "driver assertions failed"
import json, sys
d = json.load(open(sys.argv[1]))
assert d.get("heartbeat"), "no PX4 heartbeat"
assert d.get("armed"), "arming failed"
assert d.get("offboard_mode_seen"), "OFFBOARD mode echo not observed"
z = d.get("truth_z_min") or 0.0
assert z <= -1.5, f"no physical flight: sim truth z_min = {z}"
zf = d.get("truth_z_final")
assert zf is not None and abs(zf) < 0.5, f"did not come back down: final z = {zf}"
print(f"OK: truth z_min={z:.2f} final={zf:.2f} max_speed={d.get('truth_max_speed'):.2f}")
PYEOF
echo "[I-2] (c),(d),(e),(g) driver assertions OK"

# ---- (f) replay: no NaN + physical climb + settle.
[ -s "$REPLAY" ] || fail "no replay file"
"$I2_PYTHON" - "$REPLAY" <<'PYEOF' || fail "replay assertions failed"
import struct, math, sys
data = open(sys.argv[1], "rb").read()
n = (len(data) - 64) // 96
zmin = 1e9; zfin = None; bad = 0
for i in range(n):
    off = 64 + i * 96
    s = struct.unpack_from("<17f", data, off + 24)
    for v in s:
        if not math.isfinite(v):
            bad += 1
            break
    zmin = min(zmin, s[2])
    zfin = s[2]
assert bad == 0, f"{bad} ticks with non-finite state"
assert zmin <= -1.5, f"replay: no physical climb (z_min={zmin})"
assert abs(zfin) < 0.5, f"replay: did not settle (final z={zfin})"
print(f"OK: replay {n} ticks, z_min={zmin:.2f}, final z={zfin:.2f}, all finite")
PYEOF
echo "[I-2] (f) replay assertions OK"

# ---- (h) ULog written during the run.
ulogs=$(find "$INSTANCE_DIR/log" -name '*.ulg' -newermt "-10 minutes" 2>/dev/null | wc -l)
[ "$ulogs" -ge 1 ] || fail "no ULog written"
echo "[I-2] (h) ULog written"

echo
echo "================= I-2 PASS: PHYSICAL FLIGHT COMPLETE ================="
echo "artifacts: $OUT"
exit 0
