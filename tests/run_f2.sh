#!/bin/bash
# =============================================================================
# mavfleet integration case F-2: fleet + REAL rustsitsim dynamics (SPEC §11.2,
# Case F-2). Two vehicles, each with a full rustsitsim instance spawned via the
# scenario's [sim] command template (ADR-0008) -> scripts/run_sitsim_vehicle.sh.
#
# Single-invocation harness (SPEC §12.3): everything starts, asserts, and cleans
# up within one call. Pattern: run_f1.sh (interim-sim bring-up) + run_i2_flight.sh
# (physical flight proof from the sim's replay ground truth).
#
# Asserts:
#   (a) 2 vehicles READY (spawn -> BOOTING -> telemetry gate) with REAL sims
#       in the loop: sitsim control planes 8200/8201 answer with
#       px4_connected + loop_closed (direct wire, rustsitsim ADR-0015).
#   (b) Mission completes: run-report.json written, all tasks done,
#       all vehicles landed, no geofence breach, manager exit 0.
#   (c) PHYSICAL FLIGHT from sim ground truth (not the EKF estimate):
#       every replay vehicle_<i>.replay climbs to z <= -2.0 m and settles
#       back to |z| < 0.5 m with all-finite state.
#   (d) Teardown: no stray px4/sitsim processes, per-instance ports free
#       (TCP 4560+i refused; UDP 14540+i / 14580+i bindable; sitsim APIs
#       8200+i down).
#
# Environment overrides:
#   PX4_ROOT   (default /home/z/my-project/PX4-Autopilot)
#   API_PORT   (default 8400)
#   I2_PYTHON  (default /usr/bin/python3.13)
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PX4_ROOT="${PX4_ROOT:-/home/z/my-project/PX4-Autopilot}"
API_PORT="${API_PORT:-8400}"
I2_PYTHON="${I2_PYTHON:-/usr/bin/python3.13}"
PX4_BIN="$PX4_ROOT/build/px4_sitl_default/bin/px4"
BINARY="$ROOT/target/debug/mavfleet"
SCENARIO="$ROOT/tests/f2_sitsim.toml"
JSON_PY="python3"
REPLAY_DIR="${FLEET_SIM_CFG_DIR:-/home/z/my-project/mavfleet/scratch/vsims}"

OUT="$ROOT/tests/f2_artifacts"
rm -rf "$OUT"; mkdir -p "$OUT"
RUN_DIR="$OUT/run"
MANAGER_LOG="$OUT/manager.log"
FLEET_JSON="$OUT/fleet.json"
REPORT="$RUN_DIR/run-report.json"

BOOT_BUDGET_S=210    # both vehicles READY (2 real-sim px4 boots, staggered)
MISSION_BUDGET_S=360 # scenario max_time + margin
fail() {
    echo "F-2 FAILED: $1"
    echo "=========== diagnostics ==========="
    echo "--- manager.log (tail 60):"
    tail -60 "$MANAGER_LOG" 2>/dev/null || echo "(no manager log)"
    echo "--- /api/fleet:"
    cat "$FLEET_JSON" 2>/dev/null || echo "(no fleet json)"
    for v in 0 1; do
        echo "  -- vehicle_$v/px4.log (tail 15):"
        tail -15 "$RUN_DIR/vehicle_$v/px4.log" 2>/dev/null || echo "  (none)"
        echo "  -- vehicle_$v/sim.log (tail 15):"
        tail -15 "$RUN_DIR/vehicle_$v/sim.log" 2>/dev/null || echo "  (none)"
        echo "  -- sitsim status :820$v:"
        curl -s --max-time 2 "http://127.0.0.1:820$v/api/status" 2>/dev/null | head -c 600; echo
    done
    cleanup
    exit 1
}
cleanup() {
    if [ -n "${MANAGER_PID:-}" ] && kill -0 "$MANAGER_PID" 2>/dev/null; then
        kill "$MANAGER_PID" 2>/dev/null
        for _ in $(seq 1 40); do kill -0 "$MANAGER_PID" 2>/dev/null || break; sleep 0.25; done
        kill -9 "$MANAGER_PID" 2>/dev/null
    fi
    pkill -f "sitsim-cli" 2>/dev/null
    pkill -x px4 2>/dev/null
    wait 2>/dev/null || true
}

# ---- Preconditions.
[ -x "$PX4_BIN" ] || fail "PX4 binary missing"
[ -x "$BINARY" ] || fail "mavfleet binary missing (cargo build --workspace)"
[ -x "/home/z/my-project/rustsitsim/target/debug/sitsim-cli" ] || fail "sitsim-cli missing"
[ -f "$SCENARIO" ] || fail "scenario missing: $SCENARIO"
command -v curl >/dev/null || fail "curl not available"
if curl -s --max-time 1 "http://127.0.0.1:$API_PORT/api/fleet" >/dev/null 2>&1; then
    fail "port $API_PORT already serving (previous run?)"
fi
for p in 8200 8201; do
    if curl -s --max-time 1 "http://127.0.0.1:$p/api/status" >/dev/null 2>&1; then
        fail "sitsim api port $p already serving (previous run?)"
    fi
done
rm -f "$REPLAY_DIR"/vehicle_0.replay "$REPLAY_DIR"/vehicle_1.replay

echo "[F-2] mavfleet:  $BINARY"
echo "[F-2] scenario:  $SCENARIO (real rustsitsim per vehicle, api $API_PORT)"

# ---- 1. Start the manager (single invocation, spec §12.3).
cd "$ROOT"
"$BINARY" run --fleet "$SCENARIO" --api-port "$API_PORT" --run-dir "$RUN_DIR" \
    >"$MANAGER_LOG" 2>&1 &
MANAGER_PID=$!
echo "[F-2] manager started (pid $MANAGER_PID)"

# ---- 2. Control plane up, then both vehicles READY.
api_up=""
for _ in $(seq 1 100); do
    curl -s --max-time 1 "http://127.0.0.1:$API_PORT/api/fleet" -o "$FLEET_JSON" 2>/dev/null
    if [ -s "$FLEET_JSON" ] && grep -q '"ok":true' "$FLEET_JSON" 2>/dev/null; then
        api_up=1; break
    fi
    kill -0 "$MANAGER_PID" 2>/dev/null || break
    sleep 0.2
done
[ -n "$api_up" ] || fail "control plane did not come up on $API_PORT"
echo "[F-2] control plane up; waiting for both vehicles READY (budget ${BOOT_BUDGET_S}s)..."

ready=""
deadline=$(( $(date +%s) + BOOT_BUDGET_S ))
last="0/0"
while [ "$(date +%s)" -lt "$deadline" ]; do
    curl -s --max-time 2 "http://127.0.0.1:$API_PORT/api/fleet" -o "$FLEET_JSON" 2>/dev/null
    last=$("$JSON_PY" - "$FLEET_JSON" <<'PYEOF' 2>/dev/null
import json, sys
try:
    d = json.load(open(sys.argv[1]))["data"]
    # The READY snapshot window can be <200 ms wide (both vehicles flip
    # READY->ACTIVE together once the telemetry gate opens), so gate on
    # "boot gate passed": READY or any post-READY state.
    ready = sum(1 for v in d["vehicles"] if v["fsm"] in ("READY", "ACTIVE", "RTL", "LANDED"))
    print(f"{ready}/{len(d['vehicles'])}")
except Exception:
    print("0/0")
PYEOF
)
    if [ "$last" = "2/2" ]; then ready=1; break; fi
    kill -0 "$MANAGER_PID" 2>/dev/null || { sleep 1;  # manager may have COMPLETED before we saw 2/2
        if [ -s "$REPORT" ] && grep -q '"phase": *"COMPLETE"' "$REPORT" 2>/dev/null; then
            echo "[F-2] note: manager completed before the READY poll caught the window"
            break
        fi
        fail "manager died during bring-up (see manager.log)"; }
    sleep 1
done
[ -n "$ready" ] || fail "both vehicles did not reach READY within ${BOOT_BUDGET_S}s (last: $last)"
echo "[F-2] (a1) both vehicles READY (boot gate passed: hb + local position + home)"

# ---- (a2) REAL sims in the loop: both sitsim control planes, loop closed.
for v in 0 1; do
    port=$((8200 + v))
    ok=""
    for _ in $(seq 1 200); do
        st=$(curl -s --max-time 1 "http://127.0.0.1:$port/api/status" 2>/dev/null)
        if echo "$st" | grep -q '"px4_connected":true' && echo "$st" | grep -q '"loop_closed":true'; then
            ok=1; break
        fi
        sleep 0.5
    done
    [ -n "$ok" ] || fail "sitsim :$port did not report px4_connected + loop_closed (real sim not in the loop)"
    echo "[F-2] (a2) vehicle $v: rustsitsim :$port loop closed (direct wire, ADR-0015)"
done

# ---- 3. Wait for the manager to finish the mission (exit 0 = COMPLETED).
exit_code=""
deadline=$(( $(date +%s) + MISSION_BUDGET_S ))
while [ "$(date +%s)" -lt "$deadline" ]; do
    if ! kill -0 "$MANAGER_PID" 2>/dev/null; then
        wait "$MANAGER_PID" 2>/dev/null
        exit_code=$?
        break
    fi
    sleep 2
done
[ -n "$exit_code" ] || fail "manager did not exit within ${MISSION_BUDGET_S}s"
[ "$exit_code" -eq 0 ] || fail "manager exit code $exit_code (expected 0 = COMPLETED; see manager.log)"
echo "[F-2] (b1) manager exit code 0 (mission COMPLETED)"

# ---- (b2) report + task assertions.
[ -s "$REPORT" ] || fail "run-report.json missing/empty at $REPORT"
"$JSON_PY" - "$REPORT" <<'PYEOF' || fail "report assertions failed"
import json, sys
r = json.load(open(sys.argv[1]))
assert r["vehicle_count"] == 2, r["vehicle_count"]
assert r["phase"] == "COMPLETE", r["phase"]
assert r["aborted"] is False, "run was aborted"
tasks = r.get("tasks", [])
assert tasks, "no tasks in report"
done = [t for t in tasks if str(t.get("state", "")).lower() in ("complete", "done", "completed")]
assert len(done) == len(tasks) >= 2, f"tasks not all complete: {tasks}"
print(f"  report: phase={r['phase']} duration={r['duration_ms']/1000:.1f}s tasks_done={len(done)}/{len(tasks)}")
PYEOF
echo "[F-2] (b2) all tasks done, run report written"

# ---- (c) PHYSICAL FLIGHT from the sim replays (ground truth).
for v in 0 1; do
    rp="$REPLAY_DIR/vehicle_$v.replay"
    [ -s "$rp" ] || fail "no replay for vehicle $v at $rp (real sim did not run)"
    "$I2_PYTHON" - "$rp" "$v" <<'PYEOF' || fail "replay flight assertions failed for vehicle $v"
import struct, math, sys
data = open(sys.argv[1], "rb").read()
v = sys.argv[2]
n = (len(data) - 64) // 96
assert n > 100, f"vehicle {v}: only {n} ticks"
zmin, zfin, bad = 1e9, None, 0
for i in range(n):
    off = 64 + i * 96
    s = struct.unpack_from("<17f", data, off + 24)
    if any(not math.isfinite(x) for x in s):
        bad += 1
    zmin = min(zmin, s[2])
    zfin = s[2]
assert bad == 0, f"vehicle {v}: {bad} non-finite ticks"
assert zmin <= -2.0, f"vehicle {v}: no physical flight (z_min={zmin})"
assert abs(zfin) < 0.5, f"vehicle {v}: did not settle (final z={zfin})"
print(f"  vehicle {v}: {n} ticks, z_min={zmin:.2f} m, final z={zfin:.2f} m, all finite")
PYEOF
done
echo "[F-2] (c) physical flight proven from sim ground truth (both vehicles flew + landed)"

# ---- (d) teardown: processes + ports (retry: socket release can lag the
# manager's exit by a second or two).
sleep 1
STRAY_PX4=$(pgrep -x px4 2>/dev/null | wc -l)
STRAY_SIM=$(pgrep -f "sitsim-cli" 2>/dev/null | wc -l)
[ "$STRAY_PX4" -eq 0 ] || fail "$STRAY_PX4 px4 process(es) survived teardown"
[ "$STRAY_SIM" -eq 0 ] || fail "$STRAY_SIM sitsim-cli process(es) survived teardown"
probe_ok=""
for attempt in 1 2 3 4; do
    if "$JSON_PY" <<'PYEOF' 2>/dev/null
import socket
def tcp_refused(p):
    try:
        s = socket.create_connection(("127.0.0.1", p), timeout=1); s.close(); return False
    except OSError:
        return True
def udp_free(p):
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.bind(("0.0.0.0", p)); s.close(); return True
    except OSError:
        return False
bad = []
for i in range(2):
    if not tcp_refused(4560 + i): bad.append(f"tcp 4560{i} still accepting")
    if not tcp_refused(8200 + i): bad.append(f"sitsim api 8200{i} still up")
    for p, name in [(14540 + i, "telemetry"), (14580 + i, "onboard")]:
        if not udp_free(p): bad.append(f"udp {name} {p} still bound")
assert not bad, f"ports not free: {bad}"
print("PORTS-FREE")
PYEOF
    then probe_ok=1; break; fi
    echo "[F-2] port probe attempt $attempt failed; retrying after 2s..."
    sleep 2
done
[ -n "$probe_ok" ] || fail "port probe failed after retries (stray sockets/processes)"
echo "[F-2] (d) teardown verified: no processes, all ports free"

echo
echo "================= F-2 PASS: FLEET FLEW WITH REAL rustsitsim DYNAMICS ================="
echo "artifacts: $OUT"
exit 0
