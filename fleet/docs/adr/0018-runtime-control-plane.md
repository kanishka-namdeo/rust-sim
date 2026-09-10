# ADR-0018: Runtime control plane — fault proxy, task append, hot scenario load

- Status: **SUPERSEDED / HISTORICAL (2026-09-10 cleanup)** — the entire
  runtime control plane described in this ADR (the `PUT /api/fleet`
  hot scenario load, `POST /api/tasks` runtime task append,
  `POST /api/vehicles/{i}/faults` fault proxy, the `simproxy.rs`
  module, the `OperatorTask` / `HotLoadAck` state structs, the
  `OperatorCmd::Append` / `OperatorCmd::HotLoad` variants, the
  `fire_timeline` fault-event firer, the `mavfleet check` parse+compile
  gate, the R-1 live harness, the 14 router/simproxy unit tests) was
  removed end-to-end in the 2026-09-10 cleanup (Tasks 7a/7b/7c-finish).
  The spec text below is kept as the v0.1 design record.
- Date: 2026-09-08 (accepted); 2026-09-10 (superseded by the cleanup)
- Replaced by: ADR-0030 (operator-driven SITL lifecycle — the
  supervisor on `:8500` owns the mavfleet process; the
  `POST /api/sitl/start` / `POST /api/sitl/stop` verbs replace the
  hot-swap + task-append + fault-proxy verbs). See
  `../console/docs/adr/0030-sitl-supervisor.md`.
- Original supersedes note: the "designed, deferred" endpoint note in
  SPEC §3.4 (which this ADR originally resolved); the scenario `fault`
  event placeholder of ADR-0001's interim era ("NOT injected: the
  interim simulator has no fault plane"). Both are now moot — the
  scenario DSL is removed and the fault proxy is removed.

## Context

SPEC §3.4 carried three designed-but-unbuilt routes since v0.1:
`PUT /api/fleet` (hot scenario load), `POST /api/tasks` (runtime task
append), and `POST /api/vehicles/{i}/faults` (fault injection proxied to the
per-vehicle sim). The DOX pass of 2026-09-08 made that deferral explicit
rather than implicit. Meanwhile the scenario DSL's `fault` timeline events
still logged "NOT injected" — a placeholder from the pre-rustsitsim interim
simulator era, quietly stale ever since real sims (with a full REST fault
plane on :8200+i) became the fleet's dynamics.

The operator map (ADR-0017) established the discipline this ADR reuses: REST
handlers queue commands; the supervisor — the single writer of mission state
— drains them on its 10 Hz tick and answers on oneshot acks.

## Decision

### 1. `POST /api/vehicles/{i}/faults` — the sim fault-plane proxy

The fleet endpoint proxies the request body **verbatim** to vehicle i's sim
control plane (`fleet_simctl::ports::sim_api(i)` → `POST /api/faults`) and
relays the sim's HTTP status + `{"ok","data"|"error"}` envelope back. The sim
owns the fault catalog (serde-validated `FaultType`), mints the `runtime-N`
ids, and produces the rejection reasons — one source of truth, no catalog
duplication in the fleet. The proxy is a hand-rolled minimal HTTP/1.1 client
over tokio TcpStream (`fleet-cli/src/simproxy.rs`, ~90 lines): the repo
carries no heavyweight HTTP-client dependency, the peer is always localhost,
and `Connection: close` keeps the response framing trivial.

**Live-captured protocol fact:** hyper aborts a response when it sees the
client half-close its write side mid-request. The first proxy version called
`TcpStream::shutdown()` after writing (the naive read-to-EOF server
discipline) and every proxied POST returned "no header terminator" — the sim
sent nothing. The fix is to rely on `Connection: close` instead of a
half-close; the unit-test server reads by Content-Length (hyper's framing).

### 2. Scenario `fault` events — the same proxy, from the timeline

`fire_timeline`'s `fault` arm now builds `{"type": kind, "duration_ms"}`
from the event's `FaultSpec` and POSTs it through the same proxy. The
supervisor's tick is `async` for exactly this call. Events fire on the
process-wide fleet clock: a hot-loaded scenario's events that are already
"past" fire at swap-in.

**Retry window (10 s from the first attempt):** right after a hot restart
the sim's control plane can still be booting (or in WAIT phase) when the
supervisor's first ticks run; a transient failure retries silently on
subsequent ticks. The window is anchored to the *first attempt*, never to
the event's `start_ms` — an epoch-relative window would expire before the
first try for exactly the hot-restart case it exists for.

**Live-captured PX4 fact (arming interplay):** a `gps_denial` in effect
*before* the mission blocks PX4's GPS-dependent arming gate —
`COMPONENT_ARM_DISARM` returns TEMPORARILY_REJECTED ("GPS fix too low",
"horizontal position unstable") for 10+ s after the denial ends, while the
OFFBOARD *mode* change is still accepted: a vehicle can sit in OFFBOARD,
disarmed, while its runner flies the profile blind (tasks complete via
deadline, not hover). Scenario authors must keep fault windows clear of the
arming phase; the R-1 harness's staged scenario fires its denial mid-flight
instead (the realistic GPS-loss-while-flying case).

### 3. `POST /api/tasks` — runtime NED task append

Body: a single task object or an array; fields `id?`, `pos_ned_m` (required,
NED, z negative up), `hover_s`, `reward`, `deadline_s`. The handler queues
`OperatorCmd::Append`; the supervisor validates with the compiler's own
rules (fence polygon, altitude box, reachability) plus an id-collision guard
against the live board, then injects through the same three structures as
the mission-upload path (alloc Task + runner RunnerTask + TaskStatus) and
grows the auction pool — the next round reallocates (spec §6.5), including
mid-mission. Auto ids keep the `op*` namespace. Ack shape identical to the
mission upload's (`accepted` / `rejected[{label,reason}]` / `pool`).

### 4. `PUT /api/fleet` — validated hot scenario load

Body: `{"scenario_toml": "<full TOML text>"}`. The handler runs the same
parse+compile gate as `mavfleet check` (422 with reasons on failure), checks
quiescence (409 if any vehicle has a live mission runner), stages the TOML
into the current run dir (`hot-scenario-<epoch>.toml`), and queues
`OperatorCmd::HotLoad`. The supervisor re-checks quiescence at drain time
(the REST view can be a hair stale; e-stop pending also wins) and on accept
routes through `abort_run` — the existing graceful-stop machinery: any
airborne vehicle lands + disarms, the report is written, teardown frees the
ports.

`run_scenario` became a loop: each iteration is one complete run; on a
staged scenario the control-plane server task is aborted (listener freed)
and the next run rebinds the same port with a fresh AppState. Consequences,
accepted deliberately:

- **Rebind window:** REST/WS clients see connection-refused for a few
  seconds during the swap. The console tolerates this (12 s live probe, WS
  reconnect). Documented in the endpoint response (`"note"`).
- **Run-dir isolation:** the first run honors `--run-dir`; hot runs get
  `<dir>-hot-N` siblings, so reports/events never overwrite each other.
- **Frame observability:** `FleetFrame` gained an additive `scenario` field
  (the live scenario's source path) so a client can tell which scenario the
  fleet is running — the harness asserts the swap by it.
- **Teardown probe fix:** the manager's own telemetry/onboard sockets are
  now dropped (`ctxs.clear()` + AppState links cleared) *before* the
  §12.3 port-free verification — the link tasks exit when their handles
  drop — otherwise the probe warned about ports the dying run itself still
  held.

That run's report records the abort honestly (phase ABORTED, reason "hot
scenario load", exit 2); the process's final exit code is the last run's.

## Verification

- Unit: 14 new router/simproxy tests (queue+drain+ack loops, validation
  gates, nack-clears-staging, TCP round-trip against a hyper-style listener,
  closed-port 503 path). Fleet total 169 green.
- Live: `fleet/tests/live_test_runtime.sh` (**R-1**, 23 checks) against real
  PX4 v1.16.2 — fault proxy accept/reject/404 + the sim's own plane listing
  the fault; task append 2-accepted/1-fence-rejected/id-collision; PUT 422
  gate; the staged swap (graceful abort, rebind, new fence + count +
  scenario on the frame, isolated run dir, both reports naming their own
  scenarios); the timeline fault event through the real plane; the
  mission-active 409; e-stop exit 2 with clean teardown.
- Regression: F-1, F-2, O-1, O-2 re-run PASS after the supervisor changes
  (O-2's harness gained an any-vehicle engage check — the auction can
  legitimately fence-edge vehicle 0 out of the bid set at boot).
