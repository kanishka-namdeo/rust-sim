#!/bin/bash
# =============================================================================
# rustsitsim integration case I-1: boot gate against REAL PX4 (SPEC §10.3).
#
# Single-invocation harness (SPEC §11.3: background processes do not survive
# between commands — everything here starts, asserts, and cleans up within
# one call).
#
# Asserts:
#   (a) PX4 rcS completed: console line "Startup script returned successfully"
#   (b) Estimator output flowing: ESTIMATOR_STATUS / ATTITUDE /
#       LOCAL_POSITION_NED observed on the telemetry link (UDP 14540),
#       counted by an independent pymavlink oracle (python3.13)
#   (c) Heartbeats present with the expected sysid (1 for instance 0)
#   (d) The HIL control loop closed: HIL_ACTUATOR_CONTROLS arriving at the
#       sim (surfaces on /api/status)
#   (e) ULog file created in the instance directory after run start
#
# On failure: dumps diagnostics (px4 console tail, sim log tail, listener
# summary, control-plane status) and exits nonzero.
#
# Environment overrides:
#   PX4_ROOT   (default /home/z/my-project/PX4-Autopilot)
#   SITSIM_BIN (default <repo>/target/debug/sitsim-cli)
#   I1_PYTHON  (default /usr/bin/python3.13 — must have pymavlink)
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PX4_ROOT="${PX4_ROOT:-/home/z/my-project/PX4-Autopilot}"
SITSIM_BIN="${SITSIM_BIN:-$ROOT/target/debug/sitsim-cli}"
I1_PYTHON="${I1_PYTHON:-/usr/bin/python3.13}"

BUILD="$PX4_ROOT/build/px4_sitl_default"
PX4_BIN="$BUILD/bin/px4"
INSTANCE_DIR="$BUILD/instance_0"
ETC_DIR="$BUILD/etc"

SCENARIO="$ROOT/docs/examples/i1_boot.toml"
HIL_PORT=4560
API_PORT=8200
BOOT_TIMEOUT_S=60
LISTEN_S=12

OUT="$ROOT/tests/i1_artifacts"
mkdir -p "$OUT"
PX4_LOG="$OUT/px4.log"
SIM_LOG="$OUT/sim.log"
SIM_STDOUT="$OUT/sim_stdout.txt"
LISTEN_JSON="$OUT/listener.json"
REPLAY="$OUT/i1.replay"

fail() {
    echo "I-1 FAILED: $1"
    echo "=========== diagnostics ==========="
    echo "--- px4.log (tail 40):"
    tail -40 "$PX4_LOG" 2>/dev/null || echo "(no px4 log)"
    echo "--- sim.log (tail 20):"
    tail -20 "$SIM_LOG" 2>/dev/null || echo "(no sim log)"
    echo "--- listener.json:"
    cat "$LISTEN_JSON" 2>/dev/null || echo "(no listener output)"
    echo "--- control-plane /api/status:"
    curl -s --max-time 3 "http://127.0.0.1:$API_PORT/api/status" || echo "(unreachable)"
    echo
    cleanup
    exit 1
}

cleanup() {
    # PX4 first: its death surfaces as exit code 3 on the sim.
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

# ---- Preconditions.
[ -x "$PX4_BIN" ] || fail "PX4 binary missing: $PX4_BIN"
[ -x "$SITSIM_BIN" ] || fail "sitsim-cli binary missing: $SITSIM_BIN (run cargo build -p sitsim-cli)"
[ -f "$SCENARIO" ] || fail "scenario missing: $SCENARIO"
"$I1_PYTHON" -c "import pymavlink" 2>/dev/null || fail "pymavlink not importable under $I1_PYTHON"
command -v curl >/dev/null || fail "curl not available"

echo "[I-1] PX4:     $PX4_BIN"
echo "[I-1] sim:     $SITSIM_BIN"
echo "[I-1] scenario $SCENARIO (HIL $HIL_PORT, API $API_PORT)"

ulog_before=$(find "$INSTANCE_DIR/log" -name '*.ulg' -newer /dev/null 2>/dev/null | wc -l)
run_start=$(date +%s)

# ---- 1. rustsitsim: TCP 4560 + control plane 8200.
rm -f "$REPLAY"
"$SITSIM_BIN" scenario-run "$SCENARIO" \
    --replay-out "$REPLAY" --telemetry-hash \
    >"$SIM_STDOUT" 2>"$SIM_LOG" &
SIM_PID=$!

# Wait for the control plane (WAIT phase) before launching PX4.
ok=""
for _ in $(seq 1 100); do
    if curl -s --max-time 1 "http://127.0.0.1:$API_PORT/api/status" | grep -q '"phase":"WAIT"'; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "control plane did not reach WAIT on $API_PORT"
echo "[I-1] sim is listening on 127.0.0.1:$HIL_PORT (WAIT)"

# ---- 2. Telemetry oracle: bind UDP 14540 BEFORE PX4 boots.
"$I1_PYTHON" "$ROOT/tests/i1_listener.py" 14540 $LISTEN_S 1 >"$LISTEN_JSON" 2>"$OUT/listener.err" &
LISTEN_PID=$!

# ---- 3. PX4 (official multi-instance pattern, SPEC §3.3).
(
    cd "$INSTANCE_DIR" || exit 1
    PX4_SIM_MODEL=gazebo-classic_iris "$PX4_BIN" -i 0 -d "$ETC_DIR"
) >"$PX4_LOG" 2>&1 &
PX4_PID=$!
echo "[I-1] PX4 started (pid $PX4_PID), waiting for rcS..."

# ---- (a) Boot gate: console line, up to BOOT_TIMEOUT_S.
boot_line=""
for _ in $(seq 1 $((BOOT_TIMEOUT_S * 10))); do
    if grep -q "Startup script returned successfully" "$PX4_LOG" 2>/dev/null; then
        boot_line="Startup script returned successfully"
        break
    fi
    if ! kill -0 "$PX4_PID" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
[ -n "$boot_line" ] || fail "PX4 did not reach 'Startup script returned successfully' within ${BOOT_TIMEOUT_S}s"
echo "[I-1] (a) PX4 rcS complete: '$boot_line'"

# ---- (d) Loop closed: HIL_ACTUATOR_CONTROLS arriving at the sim.
ok=""
for _ in $(seq 1 200); do
    if curl -s --max-time 1 "http://127.0.0.1:$API_PORT/api/status" | grep -q '"loop_closed":true'; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "control loop did not close (no HIL_ACTUATOR_CONTROLS at the sim within 20s)"
echo "[I-1] (d) HIL loop closed (HIL_ACTUATOR_CONTROLS arriving at the sim)"

# ---- (b)+(c) Estimator + heartbeats: wait for the oracle to finish.
for _ in $(seq 1 $((LISTEN_S * 10 + 50))); do
    if ! kill -0 "$LISTEN_PID" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
kill "$LISTEN_PID" 2>/dev/null
wait "$LISTEN_PID" 2>/dev/null

[ -s "$LISTEN_JSON" ] || fail "telemetry oracle produced no output"
"$I1_PYTHON" - "$LISTEN_JSON" <<'PYEOF' || fail "telemetry assertions failed (see listener.json above)"
import json, sys
d = json.load(open(sys.argv[1]))
# Measured on this build (v1.16.2, onboard link 14540, 12 s window):
# ESTIMATOR_STATUS ~1 Hz, ATTITUDE ~50 Hz, LOCAL_POSITION_NED ~28 Hz,
# HEARTBEAT 1 Hz. Thresholds require sustained flow, not exact rates.
checks = [
    ("heartbeat_sysid", d["heartbeat_sysid"] == 1, f"sysid {d['heartbeat_sysid']} != 1"),
    ("heartbeat >= 5", d["heartbeat"] >= 5, f"only {d['heartbeat']}"),
    ("estimator_status >= 5", d["estimator_status"] >= 5, f"only {d['estimator_status']}"),
    ("attitude >= 100", d["attitude"] >= 100, f"only {d['attitude']}"),
    ("local_position_ned >= 20", d["local_position_ned"] >= 20, f"only {d['local_position_ned']}"),
]
bad = [f"{name}: {why}" for name, ok, why in checks if not ok]
if bad:
    print("TELEMETRY CHECKS FAILED: " + "; ".join(bad))
    print(json.dumps(d, indent=2))
    sys.exit(1)
print("telemetry: heartbeat sysid %d, ESTIMATOR_STATUS %d, ATTITUDE %d, LOCAL_POSITION_NED %d" % (
    d["heartbeat_sysid"], d["estimator_status"], d["attitude"], d["local_position_ned"]))
PYEOF
echo "[I-1] (b) estimator flowing; (c) heartbeats present (sysid 1)"

# ---- (e) ULog file created during this run.
ulog_new=$(find "$INSTANCE_DIR/log" -name '*.ulg' -newermt "@$run_start" 2>/dev/null | wc -l)
[ "$ulog_new" -ge 1 ] || fail "no new ULog file in $INSTANCE_DIR/log"
echo "[I-1] (e) ULog(s) written: $ulog_new"

# ---- Wrap up: status snapshot, then teardown.
STATUS=$(curl -s --max-time 2 "http://127.0.0.1:$API_PORT/api/status")
echo "[I-1] status: $(echo "$STATUS" | head -c 400)"
echo "$STATUS" > "$OUT/status.json"

echo "[I-1] killing PX4; sim should observe the disconnect (exit code 3)..."
kill "$PX4_PID" 2>/dev/null
sim_exit=""
for _ in $(seq 1 50); do
    if ! kill -0 "$SIM_PID" 2>/dev/null; then
        wait "$SIM_PID"
        sim_exit=$?
        break
    fi
    sleep 0.1
done
echo "[I-1] sim exit code: ${sim_exit:-killed-by-script}"

echo "[I-1] telemetry hash: $(grep telemetry_hash "$SIM_STDOUT" 2>/dev/null || echo n/a)"
echo "[I-1] replay: $(ls -la "$REPLAY" 2>/dev/null | awk '{print $5" bytes"}')"

cleanup
echo "[I-1] PASS: PX4 booted, estimator streaming, loop closed, heartbeats ok"
exit 0
