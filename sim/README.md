# rustsitsim

**A lockstep hardware-in-the-loop flight simulator for PX4 SITL, in a single Rust binary.**

rustsitsim runs an **unmodified PX4-Autopilot v1.16.2 SITL binary** on custom,
lightweight quadrotor dynamics — no Gazebo, no jMAVSim, no rendering, no physics-engine
dependencies. It speaks PX4's native simulator MAVLink interface (HIL_SENSOR /
HIL_STATE_QUATERNION / HIL_GPS in, HIL_ACTUATOR_CONTROLS out) over TCP in **lockstep
mode**: PX4 advances virtual time only when the simulator delivers sensor data, so
scenarios are deterministic, reproducible, and run faster or slower than wall clock
without destabilizing the flight stack.

> **Role post-2026-09-10 cleanup.** The sim stays as PX4's HIL physics
> engine. The GCS UI (`../console/`) no longer consumes the sim's
> internal data plane — the per-vehicle sim socket ladder
> (`__rsimTelemetry.simSockets`), the Sim Console overlay, the
> `useSimConsole` hook, the `SimFrame` / `ActiveFault` / `FAULT_CATALOG`
> types, the `mock-sim.ts` mock, and the G-19 (analyze sim) harness were
> all removed from `console/` in the 2026-09-10 cleanup. The sim's
> REST+WS plane on `:8200+i` still exists; only PX4 (over the HIL TCP
> link on `4560+i`) and the `fleet-supervisor`'s `mavfleet` process
> (over the sim's REST fault plane, when applicable) talk to it.

```
                      +-----------------------+
 mavfleet (per-vehicle link task) <-- TCP 4560 (listen)
 scripts / CI / REST  |   rustsitsim process  |--> PX4 simulator_mavlink (client)
                      |   dynamics + sensors  |
                      +-----------------------+
```

## Why

The standard PX4 simulation paths carry weight disproportionate to most development
tasks: Gazebo needs a rendering/physics toolchain and a GPU; its sim time is coupled to
wall-clock rendering, which rules out deterministic regression testing. jMAVSim is
JVM-based and single-vehicle in common usage. Neither is convenient to embed in a CI
pipeline that must boot the full PX4 stack, fly a scenario, and assert on the outcome.

rustsitsim occupies the gap between "unit test" and "full simulator": the minimum
external contract PX4's simulator module requires (four MAVLink messages, one TCP
socket, one rate-negotiation exchange) and nothing more. Seeded RNG streams make a
scenario bit-reproducible; faults start at an exact microsecond of simulation time, not
an approximate wall-clock moment. `docs/PROTOCOL.md` is also a standalone, field-level
ICD for the PX4 HIL lockstep protocol itself — useful even if you never run rustsitsim.

Feature highlights:

- **6-DOF quadrotor dynamics**: RK4 integration, quaternion attitude, first-order motor
  lag, quad-X thrust/torque mixing, ground contact, Dryden-style wind and turbulence
- **Sensor models**: BMI088-class IMU noise and bias random walk, magnetometer with
  inclination model, barometer with drift, GPS with EPH/EV, latency queue and denial
  windows, battery model
- **Fault injection** (spec §7): GPS denial/glitch, gyro/accel bias ramps, baro drift,
  mag disturbance, motor efficiency loss, wind gusts, sensor rate starvation —
  triggerable from the scenario timeline or REST at runtime
- **Determinism**: identical seed + config → identical 64-bit telemetry hash; replay
  files record the full sensor/actuator stream
- **Control plane**: HTTP + WebSocket on 8200 (`/api/status`, `/api/faults`,
  `/api/estop`, `/api/replay`, `/ws/telemetry` at 10 Hz)
- **Multi-vehicle**: independent instances on 4560+i / 8200+i (verified with 2)
- **CI-friendly**: full PX4 boot-to-telemetry cycle in ~90 s on 2 cores

## Quickstart

Requires: Rust 1.75+, a PX4-Autopilot v1.16.2 SITL build (see below), python3 with
`pymavlink` for the independent telemetry oracle in the integration harness.

```bash
# build
cargo build --workspace
cargo test --workspace          # 95 unit + integration tests

# run the boot-gate integration case against real PX4 (single command,
# starts the sim, boots PX4, asserts, tears down)
bash tests/run_i1.sh
```

`run_i1.sh` asserts: (a) PX4 rcS completed ("Startup script returned successfully"),
(b) EKF2 estimator output flowing, (c) heartbeats with the expected sysid, (d) the HIL
control loop closed (HIL_ACTUATOR_CONTROLS arriving at the sim), (e) a ULog was
written. Expected output ends with:

```
[I-1] PASS: PX4 booted, estimator streaming, loop closed, heartbeats ok
```

### Building PX4 (one time)

```bash
git clone --depth 1 --branch v1.16.2 https://github.com/PX4/PX4-Autopilot.git
cd PX4-Autopilot && git submodule update --init --recursive --depth 1
make px4_sitl_default            # needs cmake + ninja + python (pyyaml, jinja2, ...)
```

The pinned release lives in the `px4-version` file; integration tests refuse to run
against a mismatched build.

## Running a scenario

```bash
target/debug/sitsim-cli scenario-run docs/examples/reference.toml \
    --replay-out out.replay --telemetry-hash
target/debug/sitsim-cli replay-info out.replay
```

Flags: `--speed <F>` (wall-clock pacing; 1.0 = real time), `--duration-s <F>`,
`--replay-out <PATH>`, `--telemetry-hash` (print the final 64-bit determinism hash),
`--wait-timeout-s <N>` (fail with exit 4 if PX4 does not connect).

Scenario files are TOML — vehicle dynamics parameters, sensor models, environment,
fault timeline (see `docs/examples/`, e.g. the spec's Appendix B reference scenario
with a 30 s GPS denial window). Exit codes: 0 clean, 2 assertion failure, 3 PX4
disconnect, 4 configuration error.

## Control plane

```bash
curl http://127.0.0.1:8200/api/status          # phase, tick stats, counters
curl http://127.0.0.1:8200/api/faults          # active + scheduled faults
curl -X POST http://127.0.0.1:8200/api/faults -d '{ "type": "gps_denial", "duration_ms": 5000 }'
curl -X POST http://127.0.0.1:8200/api/estop
curl http://127.0.0.1:8200/api/replay --output run.replay
```

`/ws/telemetry` pushes a 10 Hz JSON frame with state, sensors, active faults, and tick
statistics — CI subscribes to this (the GCS UI's dashboard subscription was removed
2026-09-10; the sim's data plane is no longer consumed by `../console/`).

## Repository layout

```
crates/
  sitsim-core        6-DOF dynamics, quaternion math, RNG
  sitsim-env         atmosphere, wind, magnetic field, geodesy
  sitsim-sensors     IMU, mag, baro, GPS, battery models
  sitsim-fault       fault catalog + timeline scheduler
  sitsim-mavlink     MAVLink v2 codec + golden vectors
  sitsim-transport   TCP server + lockstep scheduler
  sitsim-sdk         config/scenario, engine assembly, replay, hash
  sitsim-cli         binary + REST/WS control plane
tests/               sitsim-integration: end-to-end harness + run_i1.sh
docs/                SPEC.md (engineering spec — see the post-2026-09-10 banner at
                     the top; the GCS UI no longer consumes the sim's data plane),
                     PROTOCOL.md (public ICD), adr/, examples/ (scenario TOMLs)
```

## Documentation

- **[docs/SPEC.md](docs/SPEC.md)** — full engineering specification: architecture, ICD,
  dynamics/sensor models, fault catalog, determinism, verification program, milestones
- **[docs/PROTOCOL.md](docs/PROTOCOL.md)** — the PX4 HIL lockstep protocol as a
  standalone ICD for simulator authors
- **[docs/adr/](docs/adr/)** — architecture decision records

## Status

v0.1 — M0/M1 complete: workspace green (95 tests), I-1 boot gate passing against real
PX4 v1.16.2, determinism harness live. Flight scenarios (I-2..I-5) and the dashboard are
in progress. License: Apache-2.0.
