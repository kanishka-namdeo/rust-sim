#!/bin/bash
# =============================================================================
# RustSim operator stack launcher — the persistent planes, daemonized.
#
#   scripts/stack_up.sh start        catalog :8300 + supervisor :8500 + console :3000
#                                   (NO fleet — operator starts SITL on demand
#                                    via the GCS UI or scripts/stack_up.sh start-fleet)
#   scripts/stack_up.sh start-fleet  (re)launch just the fleet plane (:8400 + PX4 SITL)
#   scripts/stack_up.sh stop-fleet   tear the fleet down (manager + sims + px4)
#   scripts/stack_up.sh stop         everything down, ports verified
#   scripts/stack_up.sh status       pids, ports, fleet phase
#
# Why no auto-fleet: QGC and Mission Planner do NOT auto-spawn SITL when
# the GCS launches — the operator starts SITL on demand. The GCS UI now
# has a "SITL Manager" overlay panel that calls the supervisor (:8500) to
# start/stop the mavfleet process. The `start-fleet` command remains for
# CLI users who want to skip the UI.
#
# Why daemonize: agent sandboxes reap the process tree of every shell
# invocation, so plain `&`/nohup children die between calls. A classic
# double-fork daemon (fork, setsid, fork, exec) escapes the reaping and
# survives across invocations (verified live, SANDBOX_SETUP.md reval
# 2026-09-09). The integration harnesses stay single-invocation by design —
# this launcher is the operator-facing counterpart: it puts the stack up and
# leaves it up while you browse the console through the gateway (:81).
#
# Preconditions (docs/SANDBOX_SETUP.md / docs/DEPLOYMENT.md):
#   - cargo build --workspace in sim/ and fleet/
#   - console production build (.next/standalone)
#   - PX4-Autopilot v1.16.2 built at $PX4_ROOT (default ../PX4-Autopilot)
#   - a gateway on :81 routing -> :3000 and ?XTransformPort=<port>
#     (Caddyfile.example, or the sandbox system gateway)
# =============================================================================
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FLEET="$ROOT/fleet"
CON="$ROOT/console"
STATE=/tmp/rustsim-stack
LOGS="$STATE/logs"
CATALOG_DIR="$FLEET/scratch/catalog"          # gitignored, persists missions
SCENARIO="$FLEET/tests/operator_session.toml"
export PX4_ROOT="${PX4_ROOT:-$(cd "$ROOT/.." && pwd)/PX4-Autopilot}"
export FLEET_PX4_DIR="$PX4_ROOT"
export FLEET_SIM_CFG_DIR="${FLEET_SIM_CFG_DIR:-$FLEET/scratch/vsims}"

mkdir -p "$STATE" "$LOGS" "$CATALOG_DIR" "$FLEET_SIM_CFG_DIR"

CON_SERVER=$(find "$CON/.next/standalone" -name server.js -type f -not -path "*/node_modules/*" 2>/dev/null | head -1)

say()  { echo "[stack] $*"; }
die()  { echo "[stack] FAIL: $*" >&2; exit 1; }

port_up()  { curl -s --max-time 1 "http://127.0.0.1:$1/" >/dev/null 2>&1; }
port_http(){ curl -s -o /dev/null -w "%{http_code}" --max-time 1 "http://127.0.0.1:$1/"; }
pid_alive(){ [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null; }

wait_http() { # <port> <label> <tries>
    local port="$1" label="$2" tries="${3:-100}" code=""
    for _ in $(seq 1 "$tries"); do
        code=$(port_http "$port")
        [ "$code" != "000" ] && [ -n "$code" ] && return 0
        sleep 1
    done
    say "$label never answered on :$port (last code '$code')"
    return 1
}

# daemonize <pidfile> <logfile> <cmd> [args...] — double-fork, setsid, exec.
daemonize() {
    local pidfile="$1" log="$2"; shift 2
    python3 - "$pidfile" "$log" "$@" <<'PYEOF'
import os, sys
pidfile, log = sys.argv[1], sys.argv[2]
pid = os.fork()
if pid > 0:
    sys.exit(0)
os.setsid()
pid = os.fork()
if pid > 0:
    sys.exit(0)
fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
os.dup2(fd, 1); os.dup2(fd, 2); os.close(fd)
with open(pidfile, "w") as f:
    f.write(str(os.getpid()))
os.execvp(sys.argv[3], sys.argv[3:])
PYEOF
}

kill_pidfile() { # <pidfile> <name>
    local pidfile="$1" name="$2"
    if pid_alive "$pidfile"; then
        local pid; pid=$(cat "$pidfile")
        kill "$pid" 2>/dev/null
        for _ in $(seq 1 30); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
        kill -9 "$pid" 2>/dev/null || true
        say "$name stopped (pid $pid)"
    fi
    rm -f "$pidfile"
}

start_catalog() {
    port_up 8300 && { say "catalog already up on :8300"; return 0; }
    [ -x "$FLEET/target/debug/fleet-catalog" ] || die "fleet-catalog binary missing (cargo build --workspace in fleet/)"
    daemonize "$STATE/catalog.pid" "$LOGS/catalog.log" \
        env RSIM_CATALOG_DIR="$CATALOG_DIR" RSIM_FLEET_URL="http://127.0.0.1:8400" \
        "$FLEET/target/debug/fleet-catalog" --port 8300 --catalog-dir "$CATALOG_DIR" \
        || die "could not daemonize fleet-catalog"
    wait_http 8300 "catalog" 30 || die "catalog did not come up"
    say "catalog up on :8300 (pid $(cat "$STATE/catalog.pid"), dir $CATALOG_DIR)"
}

start_supervisor() {
    port_up 8500 && { say "supervisor already up on :8500"; return 0; }
    [ -x "$FLEET/target/debug/fleet-supervisor" ] || die "fleet-supervisor binary missing (cargo build --bin fleet-supervisor in fleet/)"
    daemonize "$STATE/supervisor.pid" "$LOGS/supervisor.log" \
        env FLEET_PX4_DIR="$FLEET_PX4_DIR" FLEET_SIM_CFG_DIR="$FLEET_SIM_CFG_DIR" \
        "$FLEET/target/debug/fleet-supervisor" --port 8500 --fleet-dir "$FLEET" \
        || die "could not daemonize fleet-supervisor"
    wait_http 8500 "supervisor" 30 || die "supervisor did not come up (see $LOGS/supervisor.log)"
    say "supervisor up on :8500 (pid $(cat "$STATE/supervisor.pid")) — start SITL from the GCS UI or 'scripts/stack_up.sh start-fleet'"
}

start_console() {
    port_up 3000 && { say "console already up on :3000"; return 0; }
    [ -n "$CON_SERVER" ] || die "console build missing (npm run build in console/)"
    daemonize "$STATE/console.pid" "$LOGS/console.log" \
        env PORT=3000 HOSTNAME=127.0.0.1 NODE_ENV=production \
        node "$CON_SERVER" \
        || die "could not daemonize console server"
    wait_http 3000 "console" 60 || die "console did not come up"
    say "console up on :3000 (pid $(cat "$STATE/console.pid")) — browse via the :81 gateway (SITL not started; use the SITL Manager panel)"
}

start_fleet() {
    port_up 8400 && { say "fleet already up on :8400"; return 0; }
    [ -x "$FLEET/target/debug/mavfleet" ] || die "mavfleet binary missing (cargo build --workspace in fleet/)"
    [ -f "$SCENARIO" ] || die "scenario missing: $SCENARIO"
    [ -x "$FLEET_PX4_DIR/build/px4_sitl_default/bin/px4" ] || die "PX4 binary missing (PX4_ROOT=$PX4_ROOT)"
    # ULog growth reality: PX4's SITL logger writes ~0.5-0.7 MB/s/vehicle in
    # mode=all; a 2 h 2-vehicle session is ~4 GB. Prune old fleet-run dirs
    # (keep the newest one) at every fleet (re)launch, and restart the fleet
    # (`start-fleet`) when disk gets tight — a completed operator mission
    # ends the manager anyway, so relaunching is the natural cycle.
    ls -1dt "$STATE"/fleet-run-* 2>/dev/null | tail -n +2 | xargs -r rm -rf
    local rundir="$STATE/fleet-run-$(date +%s)"
    mkdir -p "$rundir"
    ( cd "$FLEET" && daemonize "$STATE/fleet.pid" "$LOGS/fleet.log" \
        env FLEET_PX4_DIR="$FLEET_PX4_DIR" FLEET_SIM_CFG_DIR="$FLEET_SIM_CFG_DIR" \
        "$FLEET/target/debug/mavfleet" run --fleet "$SCENARIO" --api-port 8400 --run-dir "$rundir" \
        || die "could not daemonize mavfleet" )
    wait_http 8400 "fleet" 120 || die "fleet control plane did not come up (see $LOGS/fleet.log)"
    say "fleet manager up on :8400 (pid $(cat "$STATE/fleet.pid"), run dir $rundir)"
    # vehicles pass the boot gate some seconds later
    for _ in $(seq 1 90); do
        n=$(curl -s --max-time 2 "http://127.0.0.1:8400/api/fleet" 2>/dev/null | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)['data']
    print(sum(1 for v in d['vehicles'] if v['fsm'] not in ('INIT','SPAWNING','BOOTING')))
except Exception: print(0)" 2>/dev/null || echo 0)
        [ "$n" = "2" ] && { say "both vehicles READY (px4 + sitsim pairs live)"; return 0; }
        sleep 2
    done
    say "vehicles still booting — check $LOGS/fleet.log; status will show the phase"
}

stop_fleet() {
    # Prefer the supervisor's stop endpoint (cleaner — it owns the mavfleet
    # child). Fall back to kill_pidfile + pkill for CLI-only environments.
    if port_up 8500; then
        curl -s -X POST http://127.0.0.1:8500/api/sitl/stop >/dev/null 2>&1 || true
        sleep 1
    fi
    kill_pidfile "$STATE/fleet.pid" "fleet manager"
    # manager teardown normally reaps its px4/sim children; clean squatters
    pkill -f "run_sitsim_vehicle.sh" 2>/dev/null || true
    pkill -x px4 2>/dev/null || true
    pkill -f "sitsim-cli" 2>/dev/null || true
    sleep 1
    port_up 8400 && say "WARNING: :8400 still answering after stop" || say "fleet down, :8400 free"
}

stop_all() {
    stop_fleet
    kill_pidfile "$STATE/supervisor.pid" "supervisor"
    kill_pidfile "$STATE/catalog.pid" "catalog"
    kill_pidfile "$STATE/console.pid" "console"
    local busy=0
    for p in 3000 8300 8400 8500; do port_up "$p" && { say "WARNING: :$p still answering"; busy=1; }; done
    [ "$busy" = "0" ] && say "all planes down, ports free"
}

status() {
    echo "RustSim operator stack — $(date '+%F %T')"
    for spec in "3000:console" "8300:catalog" "8500:supervisor" "8400:fleet"; do
        p="${spec%%:*}"; name="${spec##*:}"
        if pid_alive "$STATE/$name.pid"; then
            echo "  $name :$p  pid $(cat "$STATE/$name.pid")  http $(port_http "$p")"
        else
            echo "  $name :$p  DOWN"
        fi
    done
    if port_up 8500; then
        # SITL lifecycle status from the supervisor
        sitl_status=$(curl -s --max-time 2 http://127.0.0.1:8500/api/sitl/status 2>/dev/null | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)['data']
    if d.get('running'):
        print(f\"SITL RUNNING · {d.get('vehicle_count',0)} vehicles · pid {d.get('pid')} · scenario {d.get('scenario')}\")
    else:
        print('SITL STOPPED — start from the GCS UI (SITL Manager panel) or scripts/stack_up.sh start-fleet')
except Exception: print('?')" 2>/dev/null)
        echo "  sitl:  $sitl_status"
    fi
    if port_up 8400; then
        phase=$(curl -s --max-time 2 http://127.0.0.1:8400/api/fleet 2>/dev/null | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)['data']
    vs='; '.join(f\"v{v['index']} {v['fsm']}\" for v in d['vehicles'])
    print(f\"{d.get('phase','?')} | {vs}\")
except Exception: print('?')" 2>/dev/null)
        echo "  fleet: $phase"
    fi
    gw=$(port_http 81)
    echo "  gateway :81 http $gw (502 = stack down, 200 = console served)"
}

case "${1:-status}" in
    start)       start_catalog; start_supervisor; start_console; status ;;
    start-fleet) stop_fleet; start_fleet ;;
    stop-fleet)  stop_fleet ;;
    stop)        stop_all ;;
    status)      status ;;
    *) echo "usage: $0 {start|start-fleet|stop-fleet|stop|status}" >&2; exit 2 ;;
esac
