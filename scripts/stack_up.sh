#!/bin/bash
# =============================================================================
# RustSim operator stack launcher — the persistent planes, daemonized.
#
#   scripts/stack_up.sh start        catalog :8300 + fleet :8400 + console :3000
#   scripts/stack_up.sh start-fleet  (re)launch just the fleet plane
#   scripts/stack_up.sh stop-fleet   tear the fleet down (manager + sims + px4)
#   scripts/stack_up.sh stop         everything down, ports verified
#   scripts/stack_up.sh status       pids, ports, fleet phase
#
# Why daemonize: agent sandboxes reap the process tree of every shell
# invocation, so plain `&`/nohup children die between calls. A classic
# double-fork daemon (fork, setsid, fork, exec) escapes the reaping and
# survives across invocations (verified live, SANDBOX_SETUP.md reval
# 2026-09-09). The integration harnesses stay single-invocation by design —
# this launcher is the operator-facing counterpart: it puts the stack up and
# leaves it up while you browse the console through the gateway (:81).
#
# The fleet runs tests/operator_session.toml (ADR-0017 bench semantics: 2
# vehicles hold READY + disarmed; the Operator Map flies them on demand).
# When your mission completes the manager exits by design — relaunch with
# `start-fleet`. Catalog + console stay up.
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
# The exec'd server keeps the PID written to <pidfile>; stdio -> <logfile>.
daemonize() {
    local pidfile="$1" log="$2"; shift 2
    python3 - "$pidfile" "$log" "$@" <<'PYEOF'
import os, sys
pidfile, log = sys.argv[1], sys.argv[2]
pid = os.fork()
if pid > 0:
    sys.exit(0)                    # parent returns to the shell
os.setsid()                        # detach session/tty
pid = os.fork()
if pid > 0:
    sys.exit(0)                    # first child exits; grandchild is reparented
fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
os.dup2(fd, 1); os.dup2(fd, 2); os.close(fd)
with open(pidfile, "w") as f:
    f.write(str(os.getpid()))
os.execvp(sys.argv[3], sys.argv[3:])  # replace image: server keeps this PID
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

start_console() {
    port_up 3000 && { say "console already up on :3000"; return 0; }
    [ -n "$CON_SERVER" ] || die "console build missing (npm run build in console/)"
    daemonize "$STATE/console.pid" "$LOGS/console.log" \
        env PORT=3000 HOSTNAME=127.0.0.1 NODE_ENV=production \
        node "$CON_SERVER" \
        || die "could not daemonize console server"
    wait_http 3000 "console" 60 || die "console did not come up"
    say "console up on :3000 (pid $(cat "$STATE/console.pid")) — browse via the :81 gateway"
}

start_fleet() {
    port_up 8400 && { say "fleet already up on :8400"; return 0; }
    [ -x "$FLEET/target/debug/mavfleet" ] || die "mavfleet binary missing (cargo build --workspace in fleet/)"
    [ -f "$SCENARIO" ] || die "scenario missing: $SCENARIO"
    [ -x "$FLEET_PX4_DIR/build/px4_sitl_default/bin/px4" ] || die "PX4 binary missing (PX4_ROOT=$PX4_ROOT)"
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
    kill_pidfile "$STATE/catalog.pid" "catalog"
    kill_pidfile "$STATE/console.pid" "console"
    local busy=0
    for p in 3000 8300 8400; do port_up "$p" && { say "WARNING: :$p still answering"; busy=1; }; done
    [ "$busy" = "0" ] && say "all planes down, ports free"
}

status() {
    echo "RustSim operator stack — $(date '+%F %T')"
    for spec in "3000:console" "8300:catalog" "8400:fleet"; do
        p="${spec%%:*}"; name="${spec##*:}"
        if pid_alive "$STATE/$name.pid"; then
            echo "  $name :$p  pid $(cat "$STATE/$name.pid")  http $(port_http "$p")"
        else
            echo "  $name :$p  DOWN"
        fi
    done
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
    start)       start_catalog; start_fleet; start_console; status ;;
    start-fleet) stop_fleet; start_fleet ;;
    stop-fleet)  stop_fleet ;;
    stop)        stop_all ;;
    status)      status ;;
    *) echo "usage: $0 {start|start-fleet|stop-fleet|stop|status}" >&2; exit 2 ;;
esac
