# RustSim Architecture

One repo, three components + a GCS catalog server + a SITL supervisor,
one live-verified contract chain. **SITL is operator-driven** (the QGC/MP
pattern, ADR-0030): the GCS does NOT auto-spawn SITL on launch — the
supervisor (`:8500`) is the lifecycle entry point and the operator starts
SITL on demand from the GCS UI or `stack_up.sh start-fleet`.

```
                       ┌──────────────────────────────┐
                       │        browser user          │
                       │  Operations Canvas + 7      │
                       │  overlay panels (Mission,    │
                       │  Library, Fleet C2, SITL,    │
                       │  Setup, Analyze, PreFlight,   │
                       │  Settings, Cheat)            │
                       └──────────────┬───────────────┘
                                      │ HTTP/WS (relative paths)
                     ┌────────────────▼─────────────────┐
                     │   Caddy gateway :81 (XTransformPort)   │
                     └──┬──────────┬─────────┬──────────┬───┘
                        │ :8300    │ :8500   │ :3000   │ :8400 (on-demand)
              ┌─────────▼────┐  ┌──▼────────┐ │ ┌────────▼────────┐
              │ RustSim Core │  │ RustSim   │ │ │ RustSim Fleet   │
              │ (sim)        │  │ Supervisor│ │ │ manager          │
              │ per vehicle  │  │ fleet-     │ │ │ mavfleet          │
              │ sitsim-cli   │◄─┤ supervisor│ │ │ REST+WS control  │
              │ REST+WS +HIL │  │ SITL life │ │ │ plane             │
              │ TCP          │  │ cycle     │ │ │                   │
              └────────┬─────┘  └──────────┘ │ └─────────┬────────┘
                       │                     │            │
              ┌────────▼─────────┐   ┌─────▼─────┐  ┌────────▼──────────┐
              │  PX4 SITL v1.16.2 │   │ Console   │  │ PX4 SITL v1.16.2    │
              │  (unmodified)    │   │ Next.js   │  │ (unmodified, -i)    │
              └───────────────────┘   └──────────┘  └─────────────────────┘
```

- The **console** (Operations Canvas + 7 overlay panels) talks to four
  Rust planes:
  - `:8300` (catalog) — mission CRUD, ULog browse, presets (Mission, Analyze, Settings overlays)
  - `:8500` (supervisor) — SITL lifecycle: `POST /api/sitl/start`, `POST /api/sitl/stop`, `GET /api/sitl/status`, `GET /api/sitl/scenarios` (SITL Manager overlay, ADR-0030)
  - `:8400` (fleet, spawned on-demand by the supervisor) — vehicle state, arm/disarm, mission upload/download, fleet orchestration (Fleet C2, Operator Map, Vehicle Setup overlays)
  - `:8200+i` (sim, per-vehicle) — the sim stays as PX4's HIL physics engine; the GCS UI no longer consumes its internal data plane (post-2026-09-10 cleanup).
  - The `XTransformPort` gateway pattern lets a single origin serve any backend port.
- The **fleet-supervisor** (`:8500`, added 2026-09-10, ADR-0030) is the
  SITL lifecycle manager — the single owner of the `mavfleet` process tree.
  It is started by `scripts/stack_up.sh start` BEFORE the fleet; the fleet
  is started on-demand when the operator clicks the *Start SITL* button
  in the GCS UI (or POSTs `/api/sitl/start` directly, or runs
  `scripts/stack_up.sh start-fleet`).
- The **fleet manager** (`:8400`, spawned by the supervisor) spawns one
  `sitsim-cli` + one `px4 -i <n>` pair per vehicle, drives them over
  MAVLink UDP, and supervises the mission. It is no longer auto-started
  by `stack_up.sh start`.
- The **fleet-catalog** server (`:8300`, added in M1) is a separate
  binary (`fleet-mission` crate) that owns the mission file store (TOML
  on disk, ADR-0019), validation rules (ADR-0026), PX4 version gate
  (ADR-0029), and ULog serving via pyulog (ADR-0021). It is
  lifecycle-independent from the fleet manager — the catalog stays up
  while mavfleet is torn down between runs.
- Each **sitsim-cli** owns one vehicle's physics: PX4 connects to it as a
  TCP client on 4560+i and the pair exchanges HIL_SENSOR /
  HIL_STATE_QUATERNION / HIL_GPS (sim -> PX4) and HIL_ACTUATOR_CONTROLS
  (PX4 -> sim) in lockstep at 200 Hz virtual time.

## Port map (contract)

| Port | Owner | Protocol |
|------|-------|----------|
| 4560 + i | sitsim-cli (listener) | TCP, MAVLink v2 HIL lockstep; PX4 connects as client |
| 8200 + i | sitsim-cli control plane | HTTP REST + WS (10 Hz telemetry frames) — UI no longer consumes (post-2026-09-10) |
| 8300 | fleet-catalog (M1) | HTTP REST (mission CRUD, ULog, presets) |
| 8400 | mavfleet control plane (on-demand, spawned by :8500) | HTTP REST + WS (10 Hz fleet frames) |
| **8500** | **fleet-supervisor (ADR-0030)** | HTTP REST (SITL lifecycle: `/api/sitl/start`, `/stop`, `/status`, `/scenarios`) |
| 14540 + i | manager's telemetry link | UDP; PX4 streams telemetry here |
| 14580 + i | PX4 onboard link | UDP; the manager sends commands here |
| 3000 | console | HTTP (Next.js) |
| 81 | gateway | HTTP/WS reverse proxy (`?XTransformPort=<port>`) |

## Component map

```
sim/     rustsitsim workspace — 8 crates
         sitsim-core      6-DOF quadrotor dynamics, RK4 + contact substeps
         sitsim-env       atmosphere, wind, magnetic field, geodesy
         sitsim-sensors   IMU/mag/baro/GPS models (noise, latency, bias)
         sitsim-mavlink   hand-rolled MAVLink v2 codec, golden-vectored
         sitsim-transport lockstep TCP link + stats
         sitsim-fault     10-fault injection engine
         sitsim-sdk       scenario config, engine, replay, determinism hash
         sitsim-cli       the binary (REST+WS control plane)

fleet/   mavfleet workspace — 7 crates (fleet-alloc was removed 2026-09-10)
         fleet-core       FSM, registry, health, events, WGS84 geodesy
                          (GeoOrigin ECEF + Bowring, the lat/lon <-> NED
                          contract of the operator plane, ADR-0017) + TaskStatus
         fleet-mavlink    links, command/ack ladder, 20 Hz setpoint pump,
                          typed param protocol + per-vehicle ParamStore
                          (QGC-style cache, ADR-0016)
         fleet-mission    GCS catalog (`:8300`): mission_file, store,
                          validation, version_check, server, ulog, preset,
                          `fleet-catalog` binary (the scenario DSL /
                          compiler / runner / report modules were removed
                          2026-09-10)
         fleet-safety     geofence-only (the 8-policy ladder was removed
                          2026-09-10; PX4's own failsafes cover the
                          operator case)
         fleet-modes      PX4 custom-mode words, type masks
         fleet-simctl     per-vehicle process supervision, port probes,
                          controlled pair restart (airframe apply),
                          RSIM_ORIGIN_* geo-anchor export (ADR-0017)
         fleet-cli        the binaries: `mavfleet` (manager + control plane,
                          vehicle-setup REST plane + ROMFS airframe catalog,
                          operator control plane — mission upload/start and
                          guided arm/takeoff/land/rtl/hold/goto, ADR-0017)
                          and `fleet-supervisor` (the SITL lifecycle manager
                          on `:8500`, ADR-0030)

console/ Next.js 16 operator console (the Operations Canvas — see
         console/README.md + console/AGENTS.md)
```

## Key data flows

1. **Lockstep physics**: sim ticks at `rate_hz` (200 Hz default) and emits
   sensor frames stamped with virtual time; PX4's scheduler advances with
   them; PX4 returns actuator controls; the sim applies them and advances.
   Disconnect = end of run (exit 3). (Post-2026-09-10: the GCS UI no longer
   consumes the sim's internal data plane — the sim stays as PX4's HIL
   physics engine.)
2. **SITL lifecycle (ADR-0030)**: `stack_up.sh start` brings up catalog
   (:8300) + supervisor (:8500) + console (:3000) — NO fleet. The operator
   starts SITL on demand: either from the GCS UI's SITL Manager panel
   (hold-to-confirm Start, scenario dropdown) or `stack_up.sh start-fleet`
   (CLI). The supervisor spawns `mavfleet run --fleet <scenario> --api-port
   8400`; the manager spawns N PX4 SITL + sim pairs, binds N MAVLink links,
   aggregates 10 Hz FleetFrames, and serves the REST+WS plane. `stack_up.sh
   stop` calls `POST /api/sitl/stop` (clean teardown via the supervisor)
   before tearing down the supervisor + catalog + console.
3. **Mission (operator-driven)**: the operator authors a mission in the
   Mission overlay, uploads it via the MAVLink mission protocol
   (MISSION_COUNT → MISSION_ITEM_INT → MISSION_ACK for all three
   MAV_MISSION_TYPE values), and starts it via `POST /api/mission/start`
   (per-vehicle mission binding) or `POST /api/fleet/start` (parallel /
   sequential). The fleet manager drives offboard missions at a 20 Hz
   setpoint cadence; guided commands (arm/takeoff/land/rtl/hold/goto) are
   sent on demand. (The auction allocator, runner, 8-policy ladder,
   scenario DSL, hot-swap, task-append, and fault proxy were removed in
   the 2026-09-10 cleanup; see ADR-0030 + the worklog.)
4. **Telemetry**: fleet WS pushes 10 Hz fleet frames + events; the console
   normalizes tolerantly and renders map/strip/tables; commands (estop,
   goto, mission upload/start) go back over REST through the same routing.
5. **Vehicle setup (ADR-0016)**: the QGroundControl/Mission-Planner-style
   configuration workflow. On connect the manager (or the console's
   Download button) sends PARAM_REQUEST_LIST; every PARAM_VALUE — download
   frames and write echoes alike — lands in the link's typed ParamStore
   (INT32 params bit-cast out of the wire f32). The setup REST plane
   (`/api/airframes`, `/api/modes`, `/api/vehicles/{i}/setup|params|
   airframe|calibrate|mode`) serves that live cache; writes are typed
   PARAM_SETs (echo-confirmed, PX4 autosaves) and calibration/mode changes
   are COMMAND_LONGs (241/176). Airframe apply = write `SYS_AUTOSTART` +
   a controlled sim+px4 pair restart into the same workdir — rcS imports
   the persisted param, detects the change, and loads the new airframe
   (QGC's "Apply and Restart"). The console's Vehicle Setup overlay is a
   thin view over this plane; a `hold_for_setup` scenario keeps the fleet
   disarmed for configuration work. UAV / USV / UUV / VTOL / Rover frames
   are all selectable from the ROMFS-derived catalog; only multirotor-class
   frames are physics-compatible with the current quad HIL dynamics
   (flagged honestly).
6. **Operator map control (ADR-0017)**: the QGC Fly/Plan-style map view
   where the user controls SITL themselves. The fleet frame carries geo
   blocks (the scenario `[env] origin` every sim's HIL_GPS anchors to,
   plus the fence) and per-vehicle `GLOBAL_POSITION_INT` fixes, so the
   console's MapLibre GL map renders PX4's own geo estimate. Operator
   missions (`POST /api/mission`, waypoints in lat/lon + AGL) are
   geo->NED converted at the supervisor and validated with the compiler's
   own fence rules, then enter the task board as operator-uploaded
   missions; `POST /api/mission/start` flips the setup bench to RUNNING.
   Guided commands (`/api/vehicles/{i}/arm|takeoff|land|rtl|hold|goto`)
   write the same MAVLink the supervisor itself sends, gated on
   mission-active so they never fight a runner; `goto` is QGC's Go To
   Location (hold-at + goal + arm ladder + OFFBOARD). SimCtl exports the
   origin to the sim wrapper via `RSIM_ORIGIN_*`, so the fleet's conversion
   and the sim's HIL_GPS can never disagree.

> **Removed in the 2026-09-10 cleanup.** The runtime control plane
> (ADR-0018) — `PUT /api/fleet` (hot-swap), `POST /api/tasks` (append),
> `POST /api/vehicles/{i}/faults` (fault proxy), the auction allocator
> (Hungarian baseline), the 8-policy safety ladder, the scenario DSL /
> compiler / runner / report, and the `mavfleet check` subcommand — were
> removed end-to-end. See the worklog (Task 7a/7b) and ADR-0030 for the
> rationale.

## Why native HIL instead of Gazebo?

The goal is a **protocol-faithful, deterministic, dependency-free** testbed
for PX4-native integration work: no Gazebo/jmavsim, no C++ simulator in the
loop, full control of sensor physics and fault injection, byte-level
visibility of the HIL wire, and reproducible runs (scenario + seed ->
telemetry hash). The cost is honesty about wire reality: every divergence
between PX4's pinned dialect and the official common.xml is captured in
`sim/docs/PROTOCOL.md` + ADRs, with live-capture evidence.
