# mavfleet / fleet-catalog / fleet-supervisor

**Multi-vehicle PX4 SITL fleet management in Rust: MAVLink supervision,
operator-driven flight, passive geofence — against real PX4 autopilots.**

The fleet workspace ships **three** binaries:

- **`mavfleet`** — spawns and supervises a fleet of unmodified
  PX4-Autopilot v1.16.2 SITL instances (each paired with a lockstep
  simulator), connects to every vehicle over its own MAVLink UDP link,
  and runs the operator-driven mission loop: 10 Hz FleetFrame
  aggregation, operator REST (arm/takeoff/land/rtl/hold/goto/mission
  upload/start/clear), per-vehicle mission bindings + fleet-start
  (parallel + sequential), passive geofence-breach detection (raises
  the `GEOFENCE_WARN` health flag), e-stop, QGC-style vehicle setup
  (params/airframe/calibrate/mode/prearm-checks). REST + WS on `:8400`.
- **`fleet-catalog`** (built from `fleet-mission`) — the `:8300`
  mission-catalog server: mission CRUD + ULog browse + param presets
  (ADR-0027). Started by `scripts/stack_up.sh start` and stays up
  across fleet restarts.
- **`fleet-supervisor`** (new 2026-09-10, ADR-0030) — the `:8500` SITL
  lifecycle manager. Owns the `mavfleet` child process; the GCS UI's
  SITL Manager overlay panel POSTs `/api/sitl/start` / `/api/sitl/stop`
  on it. Started by `scripts/stack_up.sh start` BEFORE the fleet; the
  fleet is started on-demand.

```
                      +------------------------------+
 Next.js Fleet C2  <--|        mavfleet process      |--- UDP 14540+i  (bind, per vehicle)
 scripts / CI / REST  | registry | FSM | health |    |--> UDP 14580+i  (commands, per vehicle)
                      | geofence (passive) | 10 Hz   |--- spawn/supervise: px4 -i, simulator,
                      +------------------------------+    (rustsitsim / prototype), per vehicle
                              ↑ spawned on-demand by
                              |
                      +------------------------------+
 Next.js SITL Mgr  <--|    fleet-supervisor process  |--- REST :8500 (start/stop/status/scenarios)
 scripts/stack_up.sh  +------------------------------+
```

The 2026-09-10 cleanup (Task 7a/7b/7c-finish) removed end-to-end:
fleet-alloc (auction + Hungarian), fleet-safety/policy (the 8-policy
safety ladder), fleet-mission scenario DSL + compile + runner + report,
the runtime control plane routes (`PUT /api/fleet` hot-swap,
`POST /api/tasks` append, `POST /api/vehicles/{i}/faults` fault proxy,
`GET /api/fleet/patterns*` swarming), `mavfleet check` (validated the
removed scenario DSL), and the F-2 + R-1 harnesses. See
`../docs/VERIFICATION.md` for the deletion rationale.

## Why

Fleet autonomy software is usually tested against synthetic vehicles
that share the developer's assumptions. mavfleet's bet: run the real
PX4 stack per vehicle (real commander, real EKF2, real flight-mode
manager, real failsafes) and make the fleet manager earn its behavior
— mode commands must be ACKed, offboard engage must follow PX4's
actual sequencing, heartbeat loss must be detected on a real link, and
the geofence breach detector must raise the `GEOFENCE_WARN` flag before
PX4's own failsafes race it. Honest about what it is not (no auction,
no policy ladder, no scenario DSL, no run report, no path planning,
no reactive collision avoidance — transit altitude layers are the
advisory deconfliction).

Highlights:

- **Per-vehicle MAVLink GCS-side stack** in pure Rust: v2 codec with
  golden vectors, UDP link tasks, heartbeat health,
  COMMAND_LONG/ACK tracking
- **Operator-driven flight**: arm/takeoff/land/rtl/hold/goto + mission
  upload/start/clear (ADR-0017); per-vehicle mission bindings +
  fleet-start (parallel + sequential)
- **Passive geofence** (`fleet-safety/geofence.rs`): point-in-polygon +
  altitude box; a breach raises the `GEOFENCE_WARN` health flag (the
  8-policy ladder that used to escalate to RTL/LAND was removed
  2026-09-10)
- **Operator-driven SITL lifecycle** (ADR-0030): `stack_up.sh start`
  brings up catalog + supervisor + console (NO fleet); the supervisor
  spawns mavfleet on demand — the operator clicks the GCS UI's SITL
  Manager button or POSTs `/api/sitl/start` on `:8500`
- **Control plane**: REST + WS on `:8400` — the operator-driven routes
  kept (per-vehicle arm/takeoff/land/rtl/hold/goto/mission upload +
  start + download + prearm-checks + params + airframe + calibrate +
  mode + setup; `/api/fleet/start`, `/api/fleet/mission-bindings`,
  `/api/fleet/estop`, `/api/mission`, `/api/events`, `/api/airframes`,
  `/api/modes`, `/ws/fleet` at 10 Hz). The removed routes
  (`PUT /api/fleet`, `POST /api/tasks`, `POST /api/vehicles/{i}/faults`,
  `GET /api/fleet/patterns*`, `GET /api/replays*`) return 404/405.
- **Catalog plane**: REST on `:8300` — mission CRUD + ULog browse +
  param presets (ADR-0027; the `/api/replays*` routes were removed
  2026-09-10 with the Analyze `.replay` tab).

## Quickstart

Requires: Rust 1.98+, a PX4-Autopilot v1.16.2 SITL build, python3 (for
the interim simulator prototype; swap in the
[rustsitsim](../sim) binary via `FLEET_SITSIM_BIN` once built).

```bash
cargo build --workspace
cargo test --workspace           # 285 tests post-2026-09-10 cleanup
                                 # (FSM, geofence, codec golden vectors,
                                 # setpoint masks, router, catalog)

# F-1: two real PX4 vehicles, manager, live fleet state over REST
bash tests/run_f1.sh
```

`run_f1.sh` spawns 2 vehicles (PX4 `-i 0/1` + simulator each), starts
the manager, waits for both links to come alive, asserts `GET /api/fleet`
shows both vehicles with live health, then tears everything down and
verifies all per-instance ports are free.

### The persistent operator stack (ADR-0030)

```bash
# from the repo root
scripts/stack_up.sh start       # catalog :8300 + supervisor :8500 + console :3000 (NO fleet)
scripts/stack_up.sh status      # four-plane state
# start SITL on demand — either from the GCS UI (SITL Manager overlay)
# or via the CLI:
scripts/stack_up.sh start-fleet # POST :8500/api/sitl/start — spawns mavfleet run
scripts/stack_up.sh stop-fleet # POST :8500/api/sitl/stop
scripts/stack_up.sh stop        # stop-fleet + supervisor + catalog + console
```

### Running a fleet scenario (CLI)

```bash
target/debug/mavfleet run --fleet tests/demo_live.toml --api-port 8400
```

(The `mavfleet check <file>` subcommand was removed 2026-09-10 with
the scenario DSL; the lean `config.rs` parser accepts unknown keys for
forward-compat, so `[[tasks]]` / `[[event]]` / `[success]` blocks in
the persistent fixtures are silently ignored.)

### Control plane

```bash
curl http://127.0.0.1:8400/api/fleet
curl http://127.0.0.1:8400/api/vehicles/1
curl http://127.0.0.1:8400/api/events
# operator mission upload + start (ADR-0017 — the operator-driven path):
curl -X POST http://127.0.0.1:8400/api/mission \
  -d '{"items":[{"lat_deg":47.397889,"lon_deg":8.545734,"alt_m":15,"hover_s":5}]}'
curl -X POST http://127.0.0.1:8400/api/mission/start
curl -X POST http://127.0.0.1:8400/api/fleet/estop

# SITL lifecycle (ADR-0030) on the supervisor:
curl http://127.0.0.1:8500/api/sitl/status
curl -X POST http://127.0.0.1:8500/api/sitl/start -d '{"scenario":"operator_session.toml"}'
curl -X POST http://127.0.0.1:8500/api/sitl/stop
curl http://127.0.0.1:8500/api/sitl/scenarios
```

`/ws/fleet` pushes 10 Hz fleet frames plus events — the Operations
Canvas drives every overlay from it.

## Repository layout

```
crates/
  fleet-core        registry, supervisor tick, health, vehicle FSM, event log,
                    task (TaskStatus — moved here 2026-09-10 from the removed
                    fleet-mission::report), events (unix_now — same move)
  fleet-mavlink     GCS-side MAVLink v2 codec + UDP link tasks (+ golden vectors)
  fleet-modes       PX4 custom mode constants (drift-checked)
  fleet-safety      geofence (passive; the 8-policy ladder was removed 2026-09-10)
  fleet-mission     GCS catalog (the scenario DSL + compiler + runner + report
                    were removed 2026-09-10; fleet-mission is now catalog-only)
  fleet-simctl      per-vehicle process supervision (px4 + simulator, port map)
  fleet-cli         composition root: three binaries — mavfleet run,
                    fleet-supervisor (:8500, ADR-0030), and (via fleet-mission)
                    fleet-catalog (:8300, ADR-0027)
                    (the simproxy + report modules + OperatorTask / HotLoadAck
                    state + OperatorCmd::Append / HotLoad variants were removed
                    2026-09-10)
tests/              run_f1.sh integration harness (run_f2.sh + R-1 deleted
                    2026-09-10)
scripts/            sim_stream.py (interim simulator prototype, ADR-0001),
                    run_sitsim_vehicle.sh, gen_airframes.py, gen_golden.py,
                    live_test_setup.sh
docs/               SPEC.md (Historical — v0.1 design record), SCENARIOS.md
                    (Historical — scenario DSL removed), adr/, examples/
```

## Documentation

- **[docs/SPEC.md](docs/SPEC.md)** — the v0.1 engineering specification
  (Historical record post-2026-09-10; the banner at the top lists the
  cleanup removal). Architecture, MAVLink ICD, FSM, allocation
  (removed), safety ladder (removed), verification program.
- **[docs/SCENARIOS.md](docs/SCENARIOS.md)** — the scenario DSL
  reference (Historical — the scenario DSL was removed 2026-09-10;
  kept as the design record. The `[fleet]` / `[env]` / `[sim]` /
  `[[vehicle_override]]` config parts are still parsed by the lean
  `config.rs` parser).
- **[docs/adr/](docs/adr/)** — architecture decision records
  (ADR-0018 is marked Superseded/Historical — the runtime control
  plane was removed 2026-09-10).

Companion project: **[rustsitsim](../sim)** — the lockstep HIL flight
simulator each vehicle runs against. Both repos pin the same PX4
version (`px4-version`).

## Status

v0.2 — library layer complete and green (285 tests post-2026-09-10
cleanup); the three binaries (mavfleet + fleet-catalog +
fleet-supervisor) compose the persistent operator stack
(ADR-0030). License: Apache-2.0.
