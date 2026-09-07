# mavfleet

**Multi-vehicle PX4 SITL fleet management in Rust: MAVLink supervision, auction-based
task allocation, offboard control, and a safety supervisor — against real PX4
autopilots.**

mavfleet spawns and supervises a fleet of **unmodified PX4-Autopilot v1.16.2 SITL
instances** (each paired with a lockstep simulator), connects to every vehicle over
its own MAVLink UDP link, and runs the full mission loop: health aggregation, vehicle
state machines, contract-net task allocation with a Hungarian-algorithm optimality
baseline, 20 Hz offboard setpoint streams, geofence + heartbeat + battery safety
policies, and a scenario DSL whose success criteria are evaluated into a run report
that CI can assert on.

```
                      +------------------------------+
 Next.js fleet C2 <--|        mavfleet process      |--- UDP 14540+i  (bind, per vehicle)
 scripts / CI / REST | registry | FSM | health |    |--> UDP 14580+i  (commands, per vehicle)
                      | auction  | safety  | runner |--- spawn/supervise: px4 -i, simulator,
                      +------------------------------+    (rustsitsim / prototype), per vehicle
```

## Why

Fleet autonomy software is usually tested against synthetic vehicles that share the
developer's assumptions. mavfleet's bet: run the real PX4 stack per vehicle (real
commander, real EKF2, real flight-mode manager, real failsafes) and make the fleet
manager earn its behavior — mode commands must be ACKed, offboard engage must follow
PX4's actual sequencing, heartbeat loss must be detected on a real link, and the
geofence policy must fire before PX4's own failsafes race it. Deterministic in its
decisions (bids, auction order, policy order), honest about what it is not (no path
planning, no reactive collision avoidance — transit altitude layers are the
deconfliction, and the spec says so).

Highlights:

- **Per-vehicle MAVLink GCS-side stack** in pure Rust: v2 codec with golden vectors,
  UDP link tasks, heartbeat health, COMMAND_LONG/ACK tracking
- **Auction allocation** (contract-net sequential auction) with a **hand-rolled
  Hungarian algorithm** baseline: on random instances the auction stays within 5% of
  optimal in ≥95% of cases, and never worse than greedy-nearest — enforced in CI
- **Offboard discipline**: 20 Hz SET_POSITION_TARGET_LOCAL_NED pumping with
  velocity-capped profiles, engage/re-engage ladder per PX4 semantics, two consecutive
  dropouts on a task → RTL
- **Safety ladder** (spec §8): geofence (point-in-polygon + altitude box), heartbeat
  loss, battery reserve, link-loss events — policy actions logged in an append-only
  event log
- **Scenario DSL** (TOML): fleet, tasks, timeline events (link loss, faults, estop),
  success criteria (all_tasks_done, FSM trace arcs, no geofence breach) evaluated into
  a JSON run report
- **Control plane**: REST + WS on 8400 (`/api/fleet`, `/api/vehicles/{i}`, `/api/tasks`,
  `/api/events`, `/api/fleet/estop`, `/ws/fleet` at 5 Hz)

## Quickstart

Requires: Rust 1.75+, a PX4-Autopilot v1.16.2 SITL build, python3 (for the interim
simulator prototype; swap in the [rustsitsim](../rustsitsim) binary via
`FLEET_SITSIM_BIN` once built).

```bash
cargo build --workspace
cargo test --workspace           # 105 unit tests (auction vs Hungarian, geofence,
                                 # FSM, DSL, codec golden vectors, setpoint masks)

# F-1: two real PX4 vehicles, manager, live fleet state over REST
bash tests/run_f1.sh
```

`run_f1.sh` spawns 2 vehicles (PX4 `-i 0/1` + simulator each), starts the manager,
waits for both links to come alive, asserts `GET /api/fleet` shows both vehicles with
live health, then tears everything down and verifies all per-instance ports are free.

### Running a fleet scenario

```bash
target/debug/mavfleet run --fleet docs/examples/fleet-basic.toml
target/debug/mavfleet check docs/examples/fleet-basic.toml    # validate only
```

The canonical scenario (spec Appendix B): 2 vehicles, 4 waypoint tasks, a 15 s link
loss on vehicle 1 (expect the heartbeat-loss → RTL ladder), a 20 s GPS denial on
vehicle 0, and success criteria including an FSM trace arc. Scenario reference with
all keys: [docs/SCENARIOS.md](docs/SCENARIOS.md).

### Control plane

```bash
curl http://127.0.0.1:8400/api/fleet
curl http://127.0.0.1:8400/api/vehicles/1
curl http://127.0.0.1:8400/api/events
curl -X POST http://127.0.0.1:8400/api/tasks -d '{"id":"wp_x","pos_ned_m":[20,20,-12],"hover_s":5}'
curl -X POST http://127.0.0.1:8400/api/fleet/estop
```

`/ws/fleet` pushes 5 Hz fleet frames plus events — the dashboard drives every view
from it.

## Repository layout

```
crates/
  fleet-core        registry, supervisor tick, health, vehicle FSM, event log
  fleet-mavlink     GCS-side MAVLink v2 codec + UDP link tasks (+ golden vectors)
  fleet-modes       PX4 custom mode constants (drift-checked)
  fleet-alloc       sequential auction + Hungarian baseline
  fleet-safety      geofence + safety policy engine
  fleet-mission     scenario DSL, mission compiler, per-vehicle runner, run report
  fleet-simctl      per-vehicle process supervision (px4 + simulator, port map)
  fleet-cli         composition root: `mavfleet run` + REST/WS plane
tests/              run_f1.sh / run_f2.sh integration harnesses
scripts/            sim_stream.py (interim simulator prototype, ADR-0001)
schemas/            JSON schemas (fleet frame, run report)
docs/               SPEC.md, SCENARIOS.md, adr/
dashboard/          Next.js fleet C2 app (see spec §13)
```

## Documentation

- **[docs/SPEC.md](docs/SPEC.md)** — full engineering specification: architecture,
  MAVLink ICD, FSM, allocation, safety ladder, verification program (F-1..F-7)
- **[docs/SCENARIOS.md](docs/SCENARIOS.md)** — scenario DSL reference with examples
- **[docs/adr/](docs/adr/)** — architecture decision records

Companion project: **[rustsitsim](../rustsitsim)** — the lockstep HIL flight simulator
each vehicle runs against. Both repos pin the same PX4 version (`px4-version`).

## Status

v0.1 — library layer complete and green (105 tests); fleet-cli composition root and
the F-1..F-3 harnesses are in bring-up against real PX4. License: Apache-2.0.
