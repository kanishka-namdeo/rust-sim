#!/bin/bash
# =============================================================================
# mavfleet integration case F-1: two-vehicle bring-up against REAL PX4
# (SPEC §11.2, Case F-1).
#
# Single-invocation harness (SPEC §12.3: background processes do not survive
# between bash calls — everything here starts, asserts, and cleans up within
# one call). Pattern: rustsitsim's proven tests/run_i1.sh.
#
# Asserts:
#   (a) `mavfleet run --fleet docs/examples/fleet-basic.toml` spawns
#       sim+px4 for 2 vehicles (fleet-simctl recipe) and drives both to
#       READY within the boot budget (spawn → BOOTING → telemetry gate).
#   (b) GET /api/fleet returns 2 vehicles with LIVE health: sysid i+1,
#       decoded mode, telemetry message counts, heartbeat age < 3 s.
#   (c) POST /api/estop lands ({"ok":true}); the manager ABORTS the run,
#       exits with the CI-classifiable code (2 = aborted per F-7), and
#       writes run-report.json + events.ndjson into the run dir.
#   (d) Teardown: no processes left, all §17 per-instance ports free
#       (TCP 4560+i refused; UDP 14540+i / 14580+i bindable).
#
# On failure: dumps diagnostics (manager log tail, /api/fleet output,
# px4 console tails, event log tail) and exits nonzero.
#
# Environment overrides:
#   PX4_ROOT   (default /home/z/my-project/PX4-Autopilot)
#   API_PORT   (default 8400)
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PX4_ROOT="${PX4_ROOT:-/home/z/my-project/PX4-Autopilot}"
API_PORT="${API_PORT:-8400}"
PX4_BIN="$PX4_ROOT/build/px4_sitl_default/bin/px4"
BINARY="$ROOT/target/debug/mavfleet"
SCENARIO="$ROOT/docs/examples/fleet-basic.toml"
JSON_PY="${JSON_PY:-python3}"   # stdlib json only

OUT="$ROOT/tests/f1_artifacts"
rm -rf "$OUT"; mkdir -p "$OUT"
RUN_DIR="$OUT/run"
MANAGER_LOG="$OUT/manager.log"
FLEET_JSON="$OUT/fleet.json"
REPORT="$OUT/run/run-report.json"
EVENTS="$OUT/run/events.ndjson"

BOOT_BUDGET_S=150   # both vehicles READY (2 px4 boots on 2 cores, staggered)
EXIT_BUDGET_S=60    # manager exit after estop
fail() {
    echo "F-1 FAILED: $1"
    echo "=========== diagnostics ==========="
    echo "--- manager.log (tail 50):"
    tail -50 "$MANAGER_LOG" 2>/dev/null || echo "(no manager log)"
    echo "--- /api/fleet:"
    cat "$FLEET_JSON" 2>/dev/null || echo "(no fleet json)"
    echo "--- px4 console tails:"
    for v in 0 1; do
        echo "  -- vehicle_$v/px4.log (tail 12):"
        tail -12 "$RUN_DIR/vehicle_$v/px4.log" 2>/dev/null || echo "  (none)"
    done
    echo "--- events.ndjson (tail 20):"
    tail -20 "$EVENTS" 2>/dev/null || echo "(no event log)"
    echo "--- run-report.json:"
    head -c 2000 "$REPORT" 2>/dev/null || echo "(no report)"
    echo
    cleanup
    exit 1
}
cleanup() {
    if [ -n "${MANAGER_PID:-}" ] && kill -0 "$MANAGER_PID" 2>/dev/null; then
        kill "$MANAGER_PID" 2>/dev/null
        for _ in $(seq 1 30); do kill -0 "$MANAGER_PID" 2>/dev/null || break; sleep 0.2; done
        kill -9 "$MANAGER_PID" 2>/dev/null
    fi
    # Belt and braces: reap any strays the manager should already have reaped.
    pkill -f "sim_stream.py" 2>/dev/null
    pkill -x px4 2>/dev/null
    wait 2>/dev/null || true
}

# ---- Preconditions.
[ -x "$PX4_BIN" ] || fail "PX4 binary missing: $PX4_BIN"
[ -x "$BINARY" ] || fail "mavfleet binary missing: $BINARY (run cargo build --workspace)"
[ -f "$SCENARIO" ] || fail "scenario missing: $SCENARIO"
command -v curl >/dev/null || fail "curl not available"
"$JSON_PY" -c "import json" 2>/dev/null || fail "python3 not usable for JSON"
if curl -s --max-time 1 "http://127.0.0.1:$API_PORT/api/fleet" >/dev/null 2>&1; then
    fail "port $API_PORT already serving (previous run?)"
fi
echo "[F-1] mavfleet:  $BINARY"
echo "[F-1] px4:       $PX4_BIN"
echo "[F-1] scenario:  $SCENARIO (api 127.0.0.1:$API_PORT)"

# ---- 1. Start the manager (single invocation, spec §12.3).
cd "$ROOT"
"$BINARY" run --fleet "$SCENARIO" --api-port "$API_PORT" --run-dir "$RUN_DIR" \
    >"$MANAGER_LOG" 2>&1 &
MANAGER_PID=$!
echo "[F-1] manager started (pid $MANAGER_PID)"

# ---- 2. Wait for the control plane, then for both vehicles to reach READY.
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
echo "[F-1] control plane up; waiting for both vehicles to connect (budget ${BOOT_BUDGET_S}s)..."

ready=""
deadline=$(( $(date +%s) + BOOT_BUDGET_S ))
while [ "$(date +%s)" -lt "$deadline" ]; do
    curl -s --max-time 2 "http://127.0.0.1:$API_PORT/api/fleet" -o "$FLEET_JSON" 2>/dev/null
    n=$("$JSON_PY" - "$FLEET_JSON" <<'PYEOF' 2>/dev/null
import json, sys
try:
    d = json.load(open(sys.argv[1]))["data"]
    ready = sum(1 for v in d["vehicles"] if v["fsm"] == "READY")
    print(f"{ready}/{len(d['vehicles'])}")
except Exception:
    print("0/0")
PYEOF
)
    if [ "$n" = "2/2" ]; then ready=1; break; fi
    kill -0 "$MANAGER_PID" 2>/dev/null || fail "manager died during bring-up (see manager.log)"
    sleep 1
done
[ -n "$ready" ] || fail "both vehicles did not reach READY within ${BOOT_BUDGET_S}s (last: $n)"
echo "[F-1] (a) both vehicles READY (boot gate passed: hb + local position + home)"

# ---- 3. Live-health assertions on the fleet frame (spec F-1).
curl -s --max-time 2 "http://127.0.0.1:$API_PORT/api/fleet" -o "$FLEET_JSON" 2>/dev/null
"$JSON_PY" - "$FLEET_JSON" <<'PYEOF' || fail "live health assertions failed (see fleet.json above)"
import json, sys
d = json.load(open(sys.argv[1]))["data"]
assert len(d["vehicles"]) == 2, f"expected 2 vehicles, got {len(d['vehicles'])}"
t_ms = d["t_ms"]
for v in d["vehicles"]:
    i = v["index"]
    assert v["sysid"] == i + 1, f"vehicle {i}: sysid {v['sysid']} != {i+1}"
    assert v["mode"] and v["mode"] != "UNKNOWN(0)", f"vehicle {i}: mode not decoded: {v['mode']!r}"
    mc = v["msg_counts"]
    assert mc.get("HEARTBEAT", 0) >= 2, f"vehicle {i}: only {mc.get('HEARTBEAT',0)} heartbeats"
    assert mc.get("LOCAL_POSITION_NED", 0) >= 20, f"vehicle {i}: only {mc.get('LOCAL_POSITION_NED',0)} position msgs"
    assert mc.get("ATTITUDE", 0) >= 20, f"vehicle {i}: only {mc.get('ATTITUDE',0)} attitude msgs"
    hb_age = t_ms - v["last_heartbeat_ms"]
    assert 0 <= hb_age < 3000, f"vehicle {i}: heartbeat age {hb_age} ms (stale)"
    assert v["link"]["recv"] > 100, f"vehicle {i}: link recv {v['link']['recv']}"
    assert v["home_set"], f"vehicle {i}: home not set"
    print(f"  vehicle {i}: sysid {v['sysid']}, mode {v['mode']}, hb_age {hb_age}ms, "
          f"recv {v['link']['recv']}, HEARTBEAT {mc.get('HEARTBEAT',0)}, "
          f"LOCAL_POSITION_NED {mc.get('LOCAL_POSITION_NED',0)}, battery {v['battery_pct']}%")
print(f"  fleet phase: {d['phase']}, tick {d['tick_count']}, events in frame tail: {len(d['events_tail'])}")
PYEOF
echo "[F-1] (b) GET /api/fleet: 2 vehicles, live telemetry, fresh heartbeats"

# ---- 4. E-stop through the REST plane (spec §3.4 / F-7's abort path).
ESTOP=$(curl -s --max-time 3 -X POST "http://127.0.0.1:$API_PORT/api/estop")
echo "$ESTOP" | grep -q '"ok":true' || fail "POST /api/estop did not return ok envelope: $ESTOP"
echo "[F-1] (c) POST /api/estop accepted: $ESTOP"

# ---- 5. Manager exits with a CI-classifiable code (2 = ABORTED per F-7).
exit_code=""
deadline=$(( $(date +%s) + EXIT_BUDGET_S ))
while [ "$(date +%s)" -lt "$deadline" ]; do
    if ! kill -0 "$MANAGER_PID" 2>/dev/null; then
        wait "$MANAGER_PID" 2>/dev/null
        exit_code=$?
        break
    fi
    sleep 0.5
done
[ -n "$exit_code" ] || { cleanup; fail "manager did not exit within ${EXIT_BUDGET_S}s after estop"; }
case "$exit_code" in
    2) echo "[F-1] manager exit code 2 (ABORTED by estop — F-7 classification)";;
    0) echo "[F-1] manager exit code 0 (run completed before estop landed)";;
    *) fail "unexpected manager exit code $exit_code (expected 2=ABORTED or 0)";;
esac

# ---- 6. Report + event log written regardless of outcome (§9.2).
[ -s "$REPORT" ] || fail "run-report.json missing/empty at $REPORT"
[ -s "$EVENTS" ] || fail "events.ndjson missing/empty at $EVENTS"
"$JSON_PY" - "$REPORT" "$EVENTS" <<'PYEOF' || fail "report/event-log assertions failed"
import json, sys
r = json.load(open(sys.argv[1]))
assert r["vehicle_count"] == 2, r["vehicle_count"]
assert r["aborted"] is True, "report should record ABORTED after estop"
assert r["phase"] == "ABORTED", r["phase"]
assert r["exit_code"] == 2, r["exit_code"]
kinds = {}
for line in open(sys.argv[2]):
    kinds[json.loads(line)["kind"]] = kinds.get(json.loads(line)["kind"], 0) + 1
assert kinds.get("fsm_transition", 0) >= 4, f"thin FSM trace: {kinds}"
assert kinds.get("run_boundary", 0) >= 3, f"no run boundaries: {kinds}"
print(f"  report: aborted={r['aborted']} phase={r['phase']} exit={r['exit_code']} "
      f"duration={r['duration_ms']/1000:.1f}s vehicles={len(r['vehicles'])}")
print(f"  events: {sum(kinds.values())} total {kinds}")
PYEOF
echo "[F-1] (c2) run report + append-only event log written and classified"

# ---- 7. Teardown probe: no processes, §17 ports free (F-1's hard assert).
sleep 1
STRAY_PX4=$(pgrep -x px4 2>/dev/null | wc -l)
STRAY_SIM=$(pgrep -f "sim_stream.py" 2>/dev/null | wc -l)
[ "$STRAY_PX4" -eq 0 ] || fail "$STRAY_PX4 px4 process(es) survived teardown"
[ "$STRAY_SIM" -eq 0 ] || fail "$STRAY_SIM sim process(es) survived teardown"
"$JSON_PY" <<'PYEOF' || fail "port probe failed (see above)"
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
    for p, name in [(14540 + i, "telemetry"), (14580 + i, "onboard")]:
        if not udp_free(p): bad.append(f"udp {name} {p} still bound")
assert not bad, f"ports not free: {bad}"
print("  ports: 4560-4561 tcp refused; 14540-14541, 14580-14581 udp free")
PYEOF
echo "[F-1] (d) teardown verified: no processes, all §17 ports free"

echo "[F-1] PASS: 2 vehicles READY with live health, estop → ABORTED(2), report written, clean teardown, ports free"
exit 0
