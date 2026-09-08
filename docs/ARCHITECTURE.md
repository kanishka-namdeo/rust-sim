# RustSim Architecture

One repo, three components, one live-verified contract chain:

```
                       ┌──────────────────────────────┐
                       │        browser user          │
                       └──────────────┬───────────────┘
                                      │ HTTP/WS (relative paths)
                     ┌────────────────▼─────────────────┐
                     │   Caddy gateway :81 (XTransformPort)   │
                     └───────┬───────────────────┬──────┘
                             │ :8200+i           │ :8400
              ┌──────────────▼─────┐   ┌─────────▼────────────┐
              │ RustSim Core (sim) │   │ RustSim Fleet (fleet) │
              │  per vehicle       │   │  mission manager      │
              │  sitsim-cli        │◄──┤  mavfleet             │
              │  REST+WS + HIL TCP │   │  REST+WS control plane│
              └─────────┬──────────┘   └─────────┬────────────┘
                        │ TCP 4560+i (HIL, lockstep)  │ spawn + UDP links
              ┌─────────▼──────────┐   ┌─────────▼────────────┐
              │  PX4 SITL v1.16.2  │   │  PX4 SITL v1.16.2     │
              │  (unmodified)      │   │  (unmodified, -i)     │
              └────────────────────┘   └───────────────────────┘
```

- The **console** talks only to the two Rust control planes (REST for
  status/commands, WS for telemetry). The `XTransformPort` gateway pattern
  lets a single origin serve any backend port — the same routing the sandbox
  preview proxy uses; `console/Caddyfile.example` reproduces it locally.
- The **fleet manager** spawns one `sitsim-cli` + one `px4 -i <n>` pair per
  vehicle (the `[sim]` command template in the scenario TOML), drives them
  over MAVLink UDP, and supervises the mission.
- Each **sitsim-cli** owns one vehicle's physics: PX4 connects to it as a TCP
  client on 4560+i and the pair exchanges HIL_SENSOR / HIL_STATE_QUATERNION /
  HIL_GPS (sim -> PX4) and HIL_ACTUATOR_CONTROLS (PX4 -> sim) in lockstep at
  200 Hz virtual time. The whole flight stack — EKF2, commander, navigator,
  offboard — runs in **unmodified PX4**.

## Port map (contract)

| Port | Owner | Protocol |
|------|-------|----------|
| 4560 + i | sitsim-cli (listener) | TCP, MAVLink v2 HIL lockstep; PX4 connects as client |
| 8200 + i | sitsim-cli control plane | HTTP REST + WS (10 Hz telemetry frames) |
| 8400 | mavfleet control plane | HTTP REST + WS (10 Hz fleet frames) |
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

fleet/   mavfleet workspace — 8 crates
         fleet-core       FSM, registry, health, events, WGS84 geodesy
                          (GeoOrigin ECEF + Bowring, the lat/lon <-> NED
                          contract of the operator plane, ADR-0017)
         fleet-mavlink    links, command/ack ladder, 20 Hz setpoint pump,
                          typed param protocol + per-vehicle ParamStore
                          (QGC-style cache, ADR-0016)
         fleet-mission    scenario DSL, compiler, mission runner, report
         fleet-alloc      auction allocator (+ Hungarian baseline)
         fleet-safety     geofence + 8-policy ladder
         fleet-modes      PX4 custom-mode words, type masks
         fleet-simctl     per-vehicle process supervision, port probes,
                          controlled pair restart (airframe apply),
                          RSIM_ORIGIN_* geo-anchor export (ADR-0017)
         fleet-cli        the binary (manager, control plane,
                          vehicle-setup REST plane + ROMFS airframe catalog,
                          operator control plane — mission upload/start and
                          guided arm/takeoff/land/rtl/hold/goto, ADR-0017)

console/ Next.js 16 operator console (see console/README.md)
```

## Key data flows

1. **Lockstep physics**: sim ticks at `rate_hz` (200 Hz default) and emits
   sensor frames stamped with virtual time; PX4's scheduler advances with
   them; PX4 returns actuator controls; the sim applies them and advances.
   Disconnect = end of run (exit 3).
2. **Mission**: scenario -> compile (fence/task validation) -> sequential
   auction -> per-vehicle runner -> arm + OFFBOARD engage -> 20 Hz
   position setpoints (4 m/s capped ramps) -> hover observation -> RTL ->
   land -> disarm; the 10 Hz supervisor enforces the safety ladder over it
   all.
3. **Telemetry**: sitsim WS pushes 10 Hz JSON frames; fleet WS pushes 10 Hz
   fleet frames + events; the console normalizes both tolerantly and renders
   strip charts/maps/tables; commands (fault inject, estop) go back over
   REST through the same routing.
4. **Vehicle setup (ADR-0016)**: the QGroundControl/Mission-Planner-style
   configuration workflow. On connect the manager (or the console's Download
   button) sends PARAM_REQUEST_LIST; every PARAM_VALUE — download frames and
   write echoes alike — lands in the link's typed ParamStore (INT32 params
   bit-cast out of the wire f32). The setup REST plane (`/api/airframes`,
   `/api/modes`, `/api/vehicles/{i}/setup|params|airframe|calibrate|mode`)
   serves that live cache; writes are typed PARAM_SETs (echo-confirmed, PX4
   autosaves) and calibration/mode changes are COMMAND_LONGs (241/176).
   Airframe apply = write `SYS_AUTOSTART` + a controlled sim+px4 pair
   restart into the same workdir — rcS imports the persisted param, detects
   the change, and loads the new airframe (QGC's "Apply and Restart"). The
   console's Vehicle Setup tab is a thin view over this plane; a
   `hold_for_setup` scenario keeps the fleet disarmed for configuration
   work. UAV / USV / UUV / VTOL / Rover frames are all selectable from the
   ROMFS-derived catalog; only multirotor-class frames are
   physics-compatible with the current quad HIL dynamics (flagged honestly).
5. **Operator map control (ADR-0017)**: the QGC Fly/Plan-style map view
   where the user controls SITL themselves. The fleet frame now carries
   geo blocks (the scenario `[env] origin` every sim's HIL_GPS anchors to,
   plus the fence) and per-vehicle `GLOBAL_POSITION_INT` fixes, so the
   console's Leaflet map renders PX4's own geo estimate. Operator missions
   (`POST /api/mission`, waypoints in lat/lon + AGL) are geo->NED converted
   at the supervisor and validated with the compiler's own fence rules,
   then enter the same task board / sequential auction / offboard runners
   as scenario tasks; `POST /api/mission/start` flips the setup bench to
   RUNNING. Guided commands (`/api/vehicles/{i}/arm|takeoff|land|rtl|hold|goto`)
   write the same MAVLink the supervisor itself sends, gated on
   mission-active so they never fight a runner; `goto` is QGC's Go To
   Location (hold-at + goal + arm ladder + OFFBOARD). SimCtl exports the
   origin to the sim wrapper via `RSIM_ORIGIN_*`, so the fleet's conversion
   and the sim's HIL_GPS can never disagree.

## Why native HIL instead of Gazebo?

The goal is a **protocol-faithful, deterministic, dependency-free** testbed
for PX4-native integration work: no Gazebo/jmavsim, no C++ simulator in the
loop, full control of sensor physics and fault injection, byte-level
visibility of the HIL wire, and reproducible runs (scenario + seed ->
telemetry hash). The cost is honesty about wire reality: every divergence
between PX4's pinned dialect and the official common.xml is captured in
`sim/docs/PROTOCOL.md` + ADRs, with live-capture evidence.
