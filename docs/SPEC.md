# rustsitsim: Lockstep HIL Flight Simulator for PX4 SITL

**Engineering Specification** | Version 0.1 (draft for implementation) | September 2026

**Reference target:** PX4-Autopilot v1.16.2 (pinned). All protocol facts in Section 3 were verified
empirically against this release on a live build; items still requiring confirmation during
implementation are tagged with explicit verification identifiers (V-1, V-2, ...).

## 1. Introduction

### 1.1 Purpose

This document is the complete engineering specification for **rustsitsim**, a lockstep
hardware-in-the-loop (HIL) flight simulator for the PX4 software-in-the-loop (SITL) autopilot,
implemented in Rust. It defines the external interface contracts, functional and non-functional
requirements, architecture, verification program, and delivery milestones. The document is
written to be sufficient for implementation without further design decisions: a competent Rust
engineer who reads only this specification should be able to build, test, and release the tool.

rustsitsim runs an unmodified PX4 v1.16.2 autopilot binary on custom, lightweight flight
dynamics. It is a single static binary with no rendering, physics-engine, or simulator-framework
dependencies. It speaks PX4's native simulator MAVLink interface over TCP in lockstep mode,
which means PX4 advances virtual time only when the simulator delivers sensor data. This yields
deterministic, reproducible flight scenarios that run faster or slower than wall-clock time
without destabilizing the flight stack.

### 1.2 Background and Motivation

The standard PX4 simulation paths carry operational weight that is disproportionate to what
many development tasks actually need. Gazebo (classic and new) requires a full rendering and
physics toolchain, gigabytes of dependencies, and a GPU for reasonable performance; its
simulation time is coupled to wall-clock rendering, which makes it unsuitable for deterministic
regression testing. jMAVSim is lighter but JVM-based, single-vehicle in its common usage, and
lacks first-class fault injection. Neither is convenient to embed in a CI pipeline that must
boot the full PX4 stack, fly a scenario, and assert on the outcome within minutes.

rustsitsim occupies the gap between "unit test" and "full simulator". It implements the minimum
external contract that the PX4 simulator module requires (four MAVLink messages, one TCP
socket, one rate negotiation exchange) and nothing more. Because the dynamics and sensor
models are plain Rust code with seeded random number streams, a scenario is bit-reproducible
given the same seed and configuration. Because the sim controls virtual time, fault injection
is precise: a GPS denial window starts at an exact microsecond of simulation time, not an
approximate wall-clock moment.

The tool also serves as a protocol reference. Section 3 documents, at field level and with
source-code citations, the exact boot gate, framing, and unit conventions that the PX4
simulator module imposes. Much of this is tribal knowledge scattered across PX4 source files;
consolidating it here has value independent of the implementation.

### 1.3 Goals

| ID | Goal | Measure |
|---|---|---|
| G-1 | Run unmodified PX4 SITL headless on rustsitsim dynamics | Full rcS boot, EKF2 convergence, arm, takeoff, hold, land |
| G-2 | Deterministic, reproducible scenarios | Identical telemetry hash for identical seed and config |
| G-3 | Lightweight footprint | Under 20 MB RSS per simulated vehicle; static binary under 3 MB |
| G-4 | Real-time headroom | Tick computation p95 under 400 microseconds at 200 Hz |
| G-5 | First-class fault injection | Catalog of Section 7 faults, triggerable from scenario file and REST |
| G-6 | Multi-vehicle capable | Independent instances on 4560+i TCP ports (verified with 2) |
| G-7 | Operator dashboard | Next.js live telemetry, trajectory, tuning, and fault console |
| G-8 | CI-friendly | PX4 boot-to-scenario cycle under 90 seconds on 2 cores |

### 1.4 Non-Goals

The following are explicitly out of scope for v0.1. Each was cut for a reason, and the reason
is recorded so the decision can be revisited deliberately rather than accidentally.

- **Rendering or visualization of the vehicle.** The dashboard draws telemetry; it is not a 3D world.
- **Fixed-wing, VTOL, rover, or boat dynamics.** v0.1 ships the quadrotor-X model only. The
  dynamics crate is trait-based so airframes can be added, but none are promised.
- **Camera, lidar, optical flow, or range sensor simulation.** The EKF position loop in v0.1
  runs on GPS plus IMU, which is sufficient for outdoor-style scenarios.
- **HIL_RC_INPUTS (virtual RC).** Vehicle control in tests happens through MAVLink commands
  and offboard setpoints, which is the realistic path for autonomy software.
- **Flight controller hardware (hardware-in-the-loop on real FMU).** The name says HIL in the
  protocol sense (MAVLink HIL messages); the transport is the SITL TCP path.
- **Aerobatic fidelity.** The dynamics model is a rigid quadrotor with first-order motor lag,
  sufficient for EKF and controls integration testing, not for aerodynamics research.

### 1.5 Target Users

Three audiences shape the design. First, **PX4 flight-stack developers** who need a fast
inner loop for estimator and controls changes: boot, fly, assert, in under two minutes.
Second, **autonomy and mission software teams** who need a deterministic environment in which
fleet-level logic can be regression-tested against injected GPS denial, motor degradation,
and link faults. Third, **CI pipelines**, which need the whole rig to be a binary plus a PX4
binary, with no containers, GPUs, or display servers. A fourth, derived audience is anyone
integrating an external simulator with PX4: Section 3 doubles as standalone protocol
documentation.

### 1.6 Glossary

| Term | Meaning |
|---|---|
| SITL | Software-in-the-loop: PX4 compiled for the host OS, not embedded hardware |
| HIL | Hardware-in-the-loop, used here in the MAVLink message-name sense (HIL_SENSOR etc.) |
| Lockstep | Scheduling mode where the sim advances virtual time and PX4 blocks until sensor data arrives |
| ICD | Interface control document; Section 3 is the rustsitsim-to-PX4 ICD |
| uORB | PX4's internal publish/subscribe message bus |
| rcS | PX4 startup shell script that launches modules in order |
| EKF2 | PX4's attitude and position estimator |
| NED | North-East-Down coordinate frame, standard in PX4 |
| FR / NFR | Functional / non-functional requirement identifier prefix |
| V-n | Verification item: a fact to be re-confirmed during implementation |
| Dryden | Classical turbulence model family used for wind gusts (MIL-HDBK-1797 style) |

## 2. System Architecture

### 2.1 System Context

rustsitsim is one process with two interfaces. The southbound interface is the PX4 HIL
lockstep protocol: the simulator listens on a TCP port, PX4's simulator_mavlink module
connects as a client, and the pair exchanges MAVLink v2 frames. The northbound interface is
an operator plane: an embedded HTTP/WebSocket server that serves status, scenario control,
fault injection, and live telemetry to the dashboard or to scripts.

```text
                       +-----------------------+
  Next.js dashboard <--|  rustsitsim process   |--> TCP 4560+i (listen)
  scripts / CI / REST  |                       |<-- PX4 simulator_mavlink (client)
  (HTTP + WebSocket)   |  dynamics + sensors + |
                       |  fault engine + RNG   |--> MAVLink UDP 14540+i (observe,
                       +-----------------------+     read-only telemetry mirror)
                                 |
                                 v
                     PX4 process (unmodified, headless)
                     rcS -> simulator_mavlink start (blocks until
                     first valid HIL_SENSOR with id=0) -> sensors
                     -> ekf2 -> commander -> mc pos ctl -> navigator
```

The sim never writes to PX4's MAVLink UDP ports. PX4 telemetry needed for assertions is read
by test harnesses that bind 14540+i themselves; rustsitsim optionally mirrors that stream for
the dashboard, but the control loop of the sim is exclusively the HIL link.

### 2.2 Crate Decomposition

The repository is a Cargo workspace. Each crate is a publishable unit with a focused
responsibility, which keeps compile times low and lets mavfleet depend on the pieces it needs
without the CLI.

| Crate | Responsibility | Key dependencies |
|---|---|---|
| sitsim-core | 6-DOF rigid-body dynamics, quaternion integration, motor lag, ground contact | none (no_std-compatible) |
| sitsim-env | Wind and turbulence, ISA atmosphere, gravity, magnetic field, geodesy | sitsim-core |
| sitsim-sensors | IMU, magnetometer, baro, GPS, battery models; latency queues; noise RNG | sitsim-core, sitsim-env |
| sitsim-fault | Fault event catalog, timeline evaluation, actuation of model parameters | all model crates |
| sitsim-mavlink | MAVLink v2 codec for the HIL subset, framing, checksums, golden vectors | none |
| sitsim-transport | TCP listener, lockstep scheduler, rate negotiation, send/recv accounting | tokio, sitsim-mavlink |
| sitsim-sdk | Scenario configuration (TOML), simulator assembly, replay recorder | all above, serde |
| sitsim-cli | Binary: config load, run loop, axum control plane, structured logging | sitsim-sdk, axum, tracing |

The MAVLink codec is hand-written for the four-message HIL subset rather than generated from
the full dialect. The generated crates pull in the entire common dialect and compile slowly;
the subset is 4 messages and roughly 60 fields, and owning the codec keeps golden-vector
tests exact. The codec implements MAVLink v2 framing: magic 0xFD, payload length, incompat
and compat flags, sequence, system and component IDs, message ID (24-bit), payload, and CRC
X.25 with the message-specific seed (CRC_EXTRA).

### 2.3 Runtime Data Flow

One tick is the unit of execution. At the negotiated rate (200 Hz by default, see 3.4) the
sim performs the following steps, in order, and measures the wall-clock duration of each:

1. Receive: drain the TCP socket. Every HIL_ACTUATOR_CONTROLS (93) frame updates the motor
   command vector; COMMAND_LONG (76) frames with command 511 are answered per 3.4; any other
   frame is counted and dropped.
2. Act: update motor commands after range mapping (3.9), apply active fault effects (Section 7).
3. Step: integrate dynamics one fixed step h = 1/rate with RK4 (Section 5).
4. Sample: run sensor models at their own sub-rate schedule (6.x), producing IMU, mag, baro,
   GPS samples with virtual timestamps.
5. Encode and send: HIL_SENSOR on every tick; HIL_STATE_QUATERNION on every tick;
   HIL_GPS at its rate (5 Hz default). Sim time advances by h only after the sends complete.
6. Record: append the tick (inputs, state, outputs, fault state) to the in-memory ring buffer
   that backs the WebSocket stream and the replay file.

Steps 2-6 are pure functions of (previous state, inputs, RNG state). Only step 1 and step 5
touch the socket, and they run in the I/O thread; the sim-core thread never performs I/O.

### 2.4 Threading and Determinism Model

The process has exactly two threads of consequence. The **sim thread** owns dynamics,
sensors, faults, RNG, and virtual time; it is a plain synchronous loop with no locks and no
wall-clock reads (a monotonic clock is consulted only for performance metrics, never for
model behavior). The **I/O plane** is a tokio runtime owning the TCP listener, the axum
server, and the WebSocket fan-out; it communicates with the sim thread through bounded
channels (commands in, telemetry snapshots out at 10 Hz).

This split is the determinism mechanism. Everything that could introduce nondeterminism
(network timing, socket buffer boundaries, dashboard load) lives on the tokio side and can
only affect when ticks happen in wall-clock terms, never what a tick computes. Combined with
per-stream seeded RNG (8.1), identical scenario files yield identical state trajectories.

### 2.5 Process Lifecycle

The sim launches in five phases. **Configure:** parse TOML, validate ranges, allocate RNG
streams from the master seed. **Listen:** bind TCP 4560+i and the control plane ports; retry
with backoff if the port is taken, since instance directories from a crashed PX4 run may hold
sockets briefly. **Wait for PX4:** accept a connection; the sim does not spawn PX4 itself in
v0.1 (mavfleet does that), it just waits, and logs the peer address. **Handshake:** the first
valid HIL_SENSOR with id=0 unblocks PX4's rcS (3.3); the sim must be sending this stream
immediately after accept, before any rate request arrives. **Run:** the tick loop until the
scenario end condition (duration, landed-and-disarmed, or REST stop), then drain, flush the
replay file, and exit 0.

A PX4 process death manifests as a TCP EOF; the sim reacts by finishing the replay file,
logging the tick count and virtual duration, and exiting with code 3 (distinguished from
scenario-failure exit 2 and clean exit 0 so CI can classify).

## 3. External Interface: PX4 HIL Lockstep Protocol (ICD)

This section is the interface control document between rustsitsim and PX4-Autopilot v1.16.2.
Facts are stated declaratively, each with its origin: **[SRC]** a PX4 source citation, or
**[OBS]** direct empirical observation against a live build, or **[V-n]** a verification item
to re-confirm during M1 bring-up. Any implementation that satisfies every MUST in this
section will boot and fly PX4; this is the contract, not a suggestion.

### 3.1 Transport and Connection Semantics

The simulator is the **TCP server**; PX4 is the **TCP client**. This is the single most
surprising fact in the protocol and getting it backwards wastes a day. PX4's
`simulator_mavlink` module, started by `ROMFS/px4fmu_common/init.d-posix/px4-rc.mavlinksim`
as `simulator_mavlink start -c $((4560+px4_instance))`, actively connects out to the
simulator [SRC: px4-rc.mavlinksim; OBS]. Therefore rustsitsim MUST listen on
`127.0.0.1:4560 + i` where i is the PX4 instance index, and MUST accept exactly one
connection per run (subsequent connections are refused with a log line while the first
is live). PX4 redirects its target through the `PX4_SIM_HOSTNAME` / `PX4_SIM_HOST_ADDR`
environment variables [SRC], which rustsitsim documents but does not need to handle.

Per-instance port allocation follows the rcS conventions [OBS for i = 0, 1]:

| Link | Formula (i = instance) | i = 0 | i = 1 |
|---|---|---|---|
| Simulator TCP (HIL link) | 4560 + i | 4560 | 4561 |
| MAVLink onboard remote (telemetry bind) | 14540 + i | 14540 | 14541 |
| MAVLink onboard listen | 14580 + i | 14580 | 14581 |
| GCS link | 18570 + i | 18570 | 18571 |
| Payload link | 14280 + i / 14030 + i | 14280 / 14030 | 14281 / 14031 |
| Gimbal link | 13030 + i / 13280 + i | 13030 / 13280 | 13031 / 13281 |
| MAV_SYS_ID (system ID) | i + 1 | 1 | 2 |
| uXRCE-DDS client key (agent 127.0.0.1:8888) | i + 1 | 1 | 2 |

### 3.2 Framing: MAVLink v2 Is Mandatory

The HIL link MUST speak **MAVLink v2** (0xFD magic) frames. The reason is narrow and
binding: `HIL_SENSOR.id` (the uint8 sensor ID) is a **MAVLink2 extension field** in
common.xml [SRC], and PX4's boot gate requires `imu.id == 0` [SRC:
SimulatorMavlink.cpp:484, handle_message_hil_sensor]. MAVLink v1 frames cannot carry
extension fields at all, so a v1 speaker will stream HIL_SENSOR forever while PX4 sits
blocked in `simulator_mavlink start` waiting for `_has_initialized` [SRC:
SimulatorMavlink.cpp:1638, under ENABLE_LOCKSTEP_SCHEDULER]. The failure mode is silent
on the sim side and fatal on the PX4 side: rcS never proceeds, no modules start, and no
telemetry ever appears on 14540+i [OBS].

The codec MUST therefore always emit v2 framing on the HIL link: STX 0xFD, LEN, INCOMPAT
FLAGS (0), COMPAT FLAGS (0), SEQ, SYSID, COMPID, MSGID (24-bit little-endian), PAYLOAD,
CRC16-X.25 seeded with the message's CRC_EXTRA, little-endian. The sim uses SYSID 2,
COMPID 1 on the HIL link (matching the pymavlink prototype), and MUST accept any source
sysid on receive, since PX4 uses sysid = i + 1. Signature and truncation incompat flags
are never set.

### 3.3 Boot Sequence and Initialization Gate

Headless PX4 with no external simulator is launched per the official multi-instance pattern
[OBS, matching Tools/simulation/sitl_multiple_run.sh]:

```text
cd build/px4_sitl_default/instance_N
PX4_SIM_MODEL=gazebo-classic_iris ../bin/px4 -i N -d <ABS>/build/px4_sitl_default/etc
```

The `-d` flag points at the **directory** containing rcS (`etc/`), not at the rcS file.
Model `gazebo-classic_iris` selects SYS_AUTOSTART=10015 (quadrotor X airframe) and, with
`PX4_SIMULATOR` unset, routes startup into the `px4-rc.mavlinksim` branch, which is the
TCP HIL path (no Gazebo involved despite the model name) [OBS]. The critical sequencing
fact: `simulator_mavlink start` **blocks the rest of rcS** until the first valid
HIL_SENSOR with id=0 arrives [SRC: lockstep scheduler; OBS]. Module startup, EKF2,
commander, and the normal MAVLink endpoints on 14540+i all wait behind that gate. The
consequence for the sim: it MUST begin streaming HIL_SENSOR at full rate immediately
after TCP accept, using its own default rate, without waiting for any request from PX4
[OBS: rate request arrives after the stream starts].

Verified boot evidence on this path: "Startup script returned successfully" on the PX4
console, ESTIMATOR_STATUS and ATTITUDE at 50 Hz on the telemetry link, home position set,
heartbeats with the expected sysid, and ULog files written into the instance directory
[OBS].

### 3.4 Rate Negotiation (COMMAND_LONG 511)

Immediately after TCP connect, PX4 sends COMMAND_LONG (76) frames with command = 511
(MAV_CMD_SET_MESSAGE_INTERVAL). Observed instance: param1 = 115
(HIL_STATE_QUATERNION), param2 = 5000 (microseconds, i.e. 200 Hz) [OBS]. A well-behaved
simulator MUST answer each with COMMAND_ACK (77) carrying command = 511 and result = 0
(ACCEPTED) [OBS; silent ignoring also works today but is non-conforming].

rustsitsim treats the requested interval as **advisory but honored**: the tick rate is set
from the scenario file, and if a 511 request asks for a different interval for message 115,
the sim adapts the HIL_STATE_QUATERNION cadence to the request (within 50-400 Hz bounds)
and logs the change. The sensor stream rate (HIL_SENSOR) is not modulated by 511 requests
in v0.1, since PX4's lockstep scheduler consumes it at whatever cadence the sim emits.
All other COMMAND_LONG commands receive COMMAND_ACK result = 3 (UNSUPPORTED) so PX4 does
not retry.

### 3.5 Message Set Summary

Four messages carry the entire loop. IDs verified against the MAVLink v2 common dialect:

| Message | ID | Direction | Default rate | Purpose |
|---|---|---|---|---|
| HIL_SENSOR | 107 | sim to PX4 | 200 Hz | IMU, mag, baro sample; drives virtual time; id=0 gates boot |
| HIL_STATE_QUATERNION | 115 | sim to PX4 | 200 Hz | Ground-truth attitude, position, velocity; PX4 explicitly requests this one via 511 |
| HIL_GPS | 113 | sim to PX4 | 5 Hz | Position fix, velocity, satellite count |
| HIL_ACTUATOR_CONTROLS | 93 | PX4 to sim | loop-closed evidence | Motor outputs; its arrival confirms the control loop closed |
| COMMAND_LONG | 76 | PX4 to sim | on connect | Rate requests (511); answered with COMMAND_ACK |
| COMMAND_ACK | 77 | sim to PX4 | on request | Acknowledge result for 511 |

The practical minimum to reach "PX4 fully booted" is HIL_SENSOR (id = 0) plus answering
nothing; the practical minimum to close the loop is all four [OBS]. HIL_RC_INPUTS (92)
is not sent in v0.1 (see 1.4).

### 3.6 HIL_SENSOR Field Contract

Units and semantics per common.xml; consumption per SimulatorMavlink.cpp [SRC; OBS]:

| Field | Type | Unit | Sim source | Notes |
|---|---|---|---|---|
| time_usec | uint64 | us | virtual clock | Drives PX4 lockstep time: CLOCK_MONOTONIC is set from this value; MUST be monotonic non-decreasing |
| xacc, yacc, zacc | float32 | m/s² | IMU model, body frame | Include gravity per MAVLink convention; specific force, NED-body axes |
| xgyro, ygyro, zgyro | float32 | rad/s | gyro model, body frame | True rates plus noise and bias |
| xmag, ymag, zmag | float32 | gauss | magnetometer model, body frame | Field in gauss, not tesla, not milligauss |
| abs_pressure | float32 | hPa | baro model | Sea-level-referenced absolute pressure |
| diff_pressure | float32 | hPa | pitot model (0 in v0.1) | Reserved for airspeed; zero is accepted by PX4 |
| pressure_alt | float32 | m | baro altitude | Pressure-derived altitude |
| temperature | float32 | degC | ambient | Surface temperature |
| fields_updated | uint16 | bitmask | sensor schedule | Bit per sensor group, 0b011111111 observed working; bits set when the corresponding fields carry fresh data this frame |
| id | uint8 (v2 ext) | - | constant 0 | THE boot gate: PX4 requires imu.id == 0; cannot be transmitted in v1 framing |

**V-1:** Confirm the exact fields_updated bit assignment consumed by v1.16.2 (SimulatorMavlink
publishing into sensor_combined) when per-sensor decimation is introduced at M3; until then
the sim sets all bits every frame.

### 3.7 HIL_STATE_QUATERNION Field Contract

This message carries the sim's ground truth to PX4's ekf2 and attitude modules. Three unit
traps are documented in PX4's own handling code and verified live [SRC; OBS]: acceleration
fields are **int16 millig** (mG, 1 mG = 0.00980665 m/s²), velocities are **int16 cm/s**, and
airspeeds are **uint16 cm/s**. A simulator that emits m/s² in these fields saturates or
wraps and destroys the estimate within seconds.

| Field | Type | Unit | Sim source | Notes |
|---|---|---|---|---|
| time_usec | uint64 | us | virtual clock | Same clock as HIL_SENSOR |
| attitude_quaternion[4] | float32 | - | attitude, w, x, y, z order | Body-to-NED rotation; normalized |
| rollspeed, pitchspeed, yawspeed | float32 | rad/s | body rates | Truth rates (no noise) |
| lat, lon | int32 | degE7 | geodetic position | 1e7-scaled degrees |
| alt | int32 | mm | MSL altitude | 1000-scaled meters |
| vx, vy, vz | int16 | cm/s | NED velocity | SIGNED; watch the sign of vz (down positive) |
| ind_airspeed, true_airspeed | uint16 | cm/s | airspeed model (0 in v0.1) | Zero accepted |
| xacc, yacc, zacc | int16 | mG | specific force, body frame | Gravity-consistent with HIL_SENSOR xacc; quantization step 0.0098 m/s² |

**V-2:** The order of quaternion components (w-first) as consumed by v1.16.2's
handle_message_hil_state_quaternion is w, x, y, z; re-confirm with one golden vector at M1.

### 3.8 HIL_GPS Field Contract

| Field | Type | Unit | Sim source | Notes |
|---|---|---|---|---|
| time_usec | uint64 | us | virtual clock | Emission at the GPS sample time, not the tick time |
| fix_type | uint8 | enum | GPS model | 0 = no fix, 2 = 2D, 3 = 3D; use 3 when tracking, 0 during denial |
| lat, lon | int32 | degE7 | geodetic + noise | Jittered position |
| alt | int32 | mm | MSL altitude | Jittered |
| eph, epv | uint16 | cm | GPS model | Horizontal/vertical accuracy; uint16 caps at 65535 cm |
| vel | uint16 | cm/s | speed magnitude | 3D speed |
| vn, ve, vd | int16 | cm/s | NED velocity | SIGNED |
| cog | uint16 | cdeg | course over ground | 0-35999; 0 when stationary is accepted |
| satellites_visible | uint8 | count | GPS model | 10 observed working |
| id | uint8 (v2 ext) | - | constant 0 | Extension field; harmless but v2 framing required anyway |
| yaw | uint32 (v2 ext) | cdeg | not set (0) | Dual-antenna heading; not modeled |

The eph/epv uint16 range is a real constraint: values above 65535 cm clamp; the GPS denial
transition should ramp eph upward toward the clamp rather than teleport it, so PX4's
innovation tests degrade gracefully instead of gating hard [design decision].

### 3.9 HIL_ACTUATOR_CONTROLS Contract

Arriving frame (PX4 to sim): time_usec (uint64), controls[16] (float32), mode (uint8,
MAV_MODE_FLAG bits), flags (uint64). PX4 emits normalized outputs in **[-1, +1]**; the
quadrotor mixer's minimum output maps to -1 and maximum to +1. The sim maps actuator
command u_i = (controls[i] + 1) / 2, equivalent to the PWM convention 1000-2000 us with
u = 0 at 1000 us [SRC: PX4 simulator conventions]. Only controls[0..3] are consumed for the
quad-X; the remainder are logged.

**V-3:** Confirm against v1.16.2 at M1 that the -1..1 mapping for the gazebo-classic_iris
mixer is exact at the endpoints (i.e., disarm output is -1, full throttle +1), by sweeping
a static motor command and observing PX4's actuator_controls uORB topic. The arrival of
this message at the sim is itself the system's "loop closed" signal and is surfaced on
the status API.

### 3.10 Lockstep Virtual Time and Timing Contract

Virtual time is owned by the sim: PX4's CLOCK_MONOTONIC under the lockstep scheduler is
advanced by the HIL_SENSOR time_usec stream [SRC; OBS]. The contract the sim MUST uphold:

1. time_usec MUST advance by exactly 1e6/rate between consecutive HIL_SENSOR frames
   (5000 us at 200 Hz). Jitter or regression in the timestamp sequence violates the
   scheduler's assumptions [SRC].
2. Every sensor emission MUST carry a timestamp consistent with its sample time; the GPS
   stream, sent at 5 Hz, carries the timestamp of its own sample slot, not the current
   tick [design decision, keeps downstream fusion honest].
3. The sim SHOULD complete tick computation in well under the real-time budget (Section 8);
   lockstep does not require real-time execution, but wildly exceeding it makes interactive
   scenarios unusable.
4. TCP disconnect at any point is legal; the sim treats it as end-of-run (2.5).

## 4. Control and Telemetry Plane

### 4.1 HTTP API

The control plane is an axum server on 127.0.0.1:8200 + i (configurable), bound only to
loopback by default. All endpoints are JSON; all responses carry a common envelope
`{"ok": bool, "data": ..., "error": string|null}`. This plane is the ICD between the sim
and everything that is not PX4: dashboards, test harnesses, mavfleet.

| Endpoint | Method | Purpose | Request body |
|---|---|---|---|
| /api/status | GET | Phase, tick rate, virtual time, PX4 connected, loop closed, p95 tick us, message counters | - |
| /api/scenario | GET / PUT | Inspect or replace the active scenario (replacement only in WAIT phase) | scenario JSON |
| /api/faults | GET | Active fault effects and pending timeline events | - |
| /api/faults | POST | Inject a fault immediately (same schema as timeline events) | fault event JSON |
| /api/faults/{id} | DELETE | Clear a persistent fault effect | - |
| /api/estop | POST | Stop the run at the end of the current tick; flush replay | - |
| /api/replay | GET | Download the current replay file | - |
| /ws/telemetry | WS | Push stream, Section 4.2 | - |

### 4.2 WebSocket Telemetry Stream

The WS endpoint pushes a JSON frame every 100 ms (10 Hz) and on fault state changes. Frame
schema (v0.1):

```json
{
  "t_us": 125000000,
  "phase": "RUN",
  "state": {
    "pos_ned_m": [0.31, -1.02, -9.87],
    "vel_ned_ms": [0.02, 0.01, 0.00],
    "q_wxyz": [0.9998, 0.0, 0.017, 0.0],
    "omega_body_rads": [0.001, 0.002, 0.000],
    "motors": [0.54, 0.55, 0.54, 0.55],
    "battery_pct": 87.2
  },
  "sensors": {
    "accel_ms2": [0.11, 0.03, -9.79],
    "gyro_rads": [0.001, 0.002, 0.000],
    "baro_alt_m": 50.31,
    "gps_fix": 3,
    "gps_sat": 10
  },
  "faults_active": [{"type": "gps_denial", "id": "f2"}],
  "stats": {"tick_p95_us": 142, "sent": {"hil_sensor": 25000}, "recv": {"hil_actuator_controls": 24998}}
}
```

The stream is derived from the 10 Hz snapshot channel between the sim thread and the I/O
plane, so subscribing any number of clients never perturbs the sim loop. Sensor values in
the frame are the last emitted (noisy) values, which is what an operator debugging EKF
behavior wants to see; ground truth lives in state.

### 4.3 Error Model and Exit Codes

REST errors are distinguished by class: 400 for malformed requests (schema violation
details included), 409 for phase conflicts (replacing a scenario mid-RUN), 503 during
WAIT-before-accept. Process exit codes: 0 clean end, 2 scenario failure assertion (test
harness mode), 3 PX4 disconnect, 4 configuration error. The binary logs structured JSON
lines (tracing) to stderr with tick metrics every 10 s, suitable for ingestion by CI.

## 5. Flight Dynamics Model

### 5.1 State, Frames, and Integration

The vehicle is a rigid body with four fixed rotors in X configuration. The state vector is
x = [p (3), v (3), q (4), omega (3), omega_rotor (4), battery (1)], with position and
velocity in the NED local frame, q the body-to-NED unit quaternion (w-first, matching the
wire format), omega the body angular rates, and omega_rotor the rotor speeds. Integration
is fixed-step RK4 at the negotiated tick rate (h = 5 ms at 200 Hz); quaternion propagation
uses the exponential map of the body-rate vector with post-step renormalization, which
keeps norm drift at machine epsilon over hours of simulated time.

The frame conventions are chosen to match PX4 exactly: NED for world, FRD (front-right-
down) for body, z-axis thrust downward (negative z body is "up"). Sign errors here are the
classic first-day bug; the model validation suite (5.6) exists to catch them before any PX4
integration.

### 5.2 Rigid-Body Equations of Motion

Translational dynamics: p_dot = v; v_dot = R(q) * f_body / m + g_NED, where f_body is the
specific force (rotor thrusts plus aerodynamic drag, expressed in body frame) and g_NED =
(0, 0, 9.80665). Rotational dynamics: q_dot = q x [0, omega]/2; omega_dot = J^-1 *
(tau_body - omega x (J omega)), with J diagonal for the symmetric quad. Rotor forces and
moments per rotor i at arm position r_i with rotation sense s_i (+1/-1):

```text
thrust_i = c_T * omega_rotor_i^2          (body -z direction)
torque_i = s_i * c_M * omega_rotor_i^2    (body z)
tau_body = sum_i r_i x thrust_i_vec + torque_i_vec
```

Aerodynamic drag is linear-plus-quadratic in the body-frame airspeed (v_air = v_body -
wind_body): F_drag = -(k_lin * v_air + k_quad * |v_air| * v_air). This is deliberately
simple; the point of drag in this model is to make wind observable to the EKF and to give
cruise flight realistic power consumption, not to model blade aerodynamics.

### 5.3 Rotor and Actuator Model

Each rotor tracks its commanded speed with a first-order lag: omega_rotor_dot =
(u_i * omega_max - omega_rotor) / tau_motor. Motor command u_i in [0, 1] comes from the
HIL_ACTUATOR_CONTROLS mapping (3.9). A deadband and idle speed model the disarm state:
below u_idle the rotor spins at omega_idle (or zero if propellers stopped is configured),
producing negligible thrust. Fault effects multiply into u_i before the lag (Section 7),
so motor degradation behaves like an efficiency loss, which is what real degradation
looks like to a controller.

### 5.4 Ground Contact Model

Ground contact uses four point contacts at the arm ends with normal spring-damper
response and Coulomb friction: F_n = max(0, -k_g * z_c - c_g * z_dot_c) for contact
penetration z_c < 0, friction force clamped to mu * F_n opposing horizontal slip. The
model is stiff enough that landings settle within a second and soft enough that RK4 at
5 ms stays stable (k_g chosen so the natural contact period is above 10 steps). Takeoff
from rest and landing at descent rates under 1 m/s are the required behaviors; bouncing
and tipping on aggressive landing are emergent but not validated against anything.

### 5.5 Parameters and Defaults

Default values describe a 1.5 kg trainer-class quad in the iris family, chosen so hover
sits near 50 percent throttle (matching PX4's iris tuning expectations):

| Parameter | Symbol | Default | Unit |
|---|---|---|---|
| Mass | m | 1.5 | kg |
| Inertia (diagonal) | J | 0.02, 0.02, 0.04 | kg m² |
| Arm length | l | 0.225 | m |
| Thrust coefficient | c_T | 1.10e-5 | N per (rad/s)² |
| Moment coefficient | c_M | 1.25e-7 | N m per (rad/s)² |
| Max rotor speed | omega_max | 950 | rad/s |
| Motor time constant | tau_motor | 0.030 | s |
| Idle throttle | u_idle | 0.05 | - |
| Linear drag | k_lin | 0.10 | N s/m |
| Quadratic drag | k_quad | 0.85 | N s²/m² |
| Gravity | g | 9.80665 | m/s² |
| Ground stiffness | k_g | 4000 | N/m |
| Ground damping | c_g | 240 | N s/m |
| Friction coefficient | mu | 0.8 | - |
| Battery capacity | Q | 5200 | mAh (4S) |

Hover check: total thrust = m g = 14.71 N, per rotor 3.68 N, requires omega = 578 rad/s
= 0.61 omega_max, giving u approximately 0.55 after the quadratic thrust map; within the
iris envelope and stable with 2.2:1 thrust-to-weight.

### 5.6 Model Validation Criteria

Before any PX4 integration, the model itself must pass closed-form checks: (a) free fall
from rest reaches 9.80665 m/s in 1 s to within 1e-4; (b) symmetric four-motor hover command
holds position and attitude with zero drift for 60 s of simulated time; (c) a pure roll
doublet produces peak roll rate in the predicted second-order envelope; (d) energy:
specific kinetic + potential energy is conserved to 0.1 percent over ballistic arcs; (e)
quaternion norm stays within 1e-9 of unity for 1e6 steps. These are fast, exact unit tests
that catch sign and frame errors in seconds rather than debugging through PX4 symptoms.

## 6. Environment and Sensor Models

### 6.1 Wind and Turbulence

Wind is a two-component field: a steady vector plus a turbulence process. The steady wind
is constant in the scenario file, optionally ramped between waypoints over time. Turbulence
uses a first-order Gauss-Markov approximation to the Dryden spectra (MIL-HDBK-1797 family),
one independent process per horizontal axis and one for vertical: w_dot = -w / tau_d +
sigma_d * sqrt(2 / tau_d) * n(t), where n is unit Gaussian white noise from the wind RNG
stream. Intensity presets: light (sigma 1.0 m/s), moderate (2.5), severe (6.0), with
tau_d = 1.5 s horizontal, 1.0 s vertical at low altitude. The approximation is honest about
being an approximation: it matches Dryden low-frequency behavior, which is what matters for
position-hold and estimator testing, and it is a discrete-time exact solution (stable for
any h), which a raw filtered-white-noise approach is not.

### 6.2 Atmosphere and Geodesy

Barometric atmosphere is ISA: pressure p = p0 (1 - L z / T0)^(g R L) with p0 = 101325 hPa,
T0 = 288.15 K, L = 0.0065 K/m, producing 101325 hPa at the origin altitude by construction
(matching the observed working prototype values). The local origin is a geodetic fix
(default 47.397770 N, 8.545580 E - the PX4 test field, alt 500 m MSL as observed) with a
flat-earth tangent-plane conversion between NED meters and geodetic degrees using the WGS84
prime-radius of curvature at the origin latitude. Round-trip error of the conversion is
under 1 mm within a 10 km square, verified by unit test (ECEF intermediate form). Gravity
is constant; the magnetic field is a dipole-style local model with inclination 67 degrees,
declination 2 degrees, horizontal intensity 0.5 gauss, yielding a field vector of roughly
(0.19, 0.01, 0.47) gauss level in NED at the test latitude (tunable per scenario).

### 6.3 IMU Model

The IMU emits specific force and body rate at the tick rate. Error model, per axis,
modeled after BMI088-class MEMS defaults with all parameters tunable:

| Parameter | Gyro | Accelerometer | Unit |
|---|---|---|---|
| Noise density | 0.00035 | 0.0025 | per sqrt(Hz) of the respective unit |
| Bias random walk | 0.0002 | 0.0004 | unit per sqrt(s) |
| Turn-on bias sigma | 0.003 | 0.05 | unit (rad/s, m/s²) |
| Scale factor error | 0.002 | 0.002 | dimensionless |
| Axis misalignment | 0.1 | 0.1 | degree |
| Quantization | none | none | int16 path is only on HIL_STATE_QUATERNION |

Noise is sampled from a dedicated RNG stream (8.1) scaled by sqrt(rate) for correct
discrete-time density; bias follows a random walk updated per tick; scale factor and
misalignment are drawn once at initialization from their sigmas. The specific-force
output includes gravity by convention: at rest, level, the accelerometer reads +9.80665
on the body z axis (up in FRD), which is what PX4's EKF expects.

### 6.4 Magnetometer, Barometer, and GPS Models

**Magnetometer:** body-frame field from the NED model rotated by q, plus hard-iron offset
(drawn once, sigma 0.01 gauss per axis), soft-iron (identity plus small random symmetric
perturbation), and white noise (0.002 gauss sigma). Emitted at 50 Hz (every 4th tick).

**Barometer:** pressure and pressure-altitude at 50 Hz with white noise sigma 0.12 m on
altitude, a slow bias random walk (0.01 m per sqrt-minute), and an optional baro drift
fault that ramps the bias. Pressure is converted to hPa for HIL_SENSOR abs_pressure and
to meters for pressure_alt using the ISA relation, keeping the two fields consistent.

**GPS:** 5 Hz (default) fixes with multiplicative horizontal error (sigma 0.8 m) plus
vertical (sigma 1.5 m), velocity noise (sigma 0.2 m/s per axis), course-over-ground from
horizontal velocity, eph 100 cm, epv 150 cm, 10 satellites nominal. Latency is modeled as
a FIFO queue: each fix is timestamped at its sample time and delivered T_gps later
(default 120 ms), during which the vehicle has moved; this is the single most valuable
realism feature for EKF tuning because it recreates the lag-vs-innovation signature
operators see on real hardware. Cold start: the first fixes are absent (fix_type 0) for
a configurable lock time (default 0 for CI speed, 30 s when testing acquisition).
Denial windows (Section 7) suppress fixes entirely while ramping eph.

### 6.5 Battery Model

The battery integrates current draw: I = I_base + k_p * sum(thrust_i) (hover draws about
16 A on the default model), state of charge integrates I / Q with a simple two-segment
voltage curve (16.8 V full to 14.0 V at 20 percent, then steeper to 13.2 V empty) exposed
on the telemetry plane. mavfleet's failsafe ladder (its Section 8) consumes this via the
PX4 battery simulation pathway: PX4's SITL battery module itself reads a simulated drain
when enabled; rustsitsim mirrors its own model on the control plane for the dashboard.
Discharge is disabled by default (infinite battery) because most single-vehicle scenarios
do not want it.

## 7. Fault Injection

### 7.1 Fault Event Catalog

Faults are effects with parameters, a start time, and an end condition (or persistent
until cleared). All act on the model layer, upstream of the sensor and HIL emission
layers, so PX4 observes exactly what it would observe in flight.

| Type | ID | Parameters | Effect | Observable PX4-side |
|---|---|---|---|---|
| Motor efficiency | F-01 | motor 0-3, factor 0-1 | Multiplies u_i | Yaw drift, altitude sag, estimator innovations on IMU |
| Motor cut | F-02 | motor 0-3 | Sets u_i to 0 (with rotor wind-down) | Loss-of-control unless within envelope |
| IMU bias ramp | F-03 | axis, rate per s | Adds ramping bias to gyro or accel output | EKF innovation growth, eventual divergence |
| IMU saturation | F-04 | axis, limit | Clamps the sensor output | Stuck reading |
| GPS denial | F-05 | duration | Suppresses fixes, ramps eph to clamp | Position-fusion gating, position-hold degradation |
| GPS glitch | F-06 | offset meters, duration | Adds constant offset to reported position | Position jump, EKF innovation spike |
| Baro drift | F-07 | rate m/s | Ramps pressure-altitude bias | Altitude estimate creep |
| Wind event | F-08 | vector, rise time | Adds gust to steady wind | Tracking error, power draw change |
| Transport delay | F-09 | ms | Delays HIL frames on the wire | Loop timing stress; PX4 lockstep slows |
| Packet drop | F-10 | percent | Drops outgoing sensor frames | Sensor gaps, EKF resets |

F-09 and F-10 are transport-layer faults implemented in sitsim-transport; they are the
mechanism for testing PX4 robustness to simulator hiccup, and they double as the natural
stress test for the sim's own I/O accounting.

### 7.2 Scenario Timeline

Faults enter scenarios as events on the virtual-time timeline:

```toml
[[fault]]
id = "gps_denial_1"
type = "gps_denial"
start_ms = 30000
duration_ms = 20000

[[fault]]
id = "motor2_degrade"
type = "motor_efficiency"
start_ms = 90000
motor = 2
factor = 0.55
persistent = true
```

The fault engine evaluates the timeline once per tick before the act step; events are
idempotent and may overlap. Timeline semantics: start_ms and end (or duration_ms, or
persistent) define membership; parameters are frozen at event creation. The scenario
format is fully specified in Section 9.

### 7.3 Runtime Fault Control

The POST /api/faults endpoint accepts the same event schema with start_ms interpreted as
"now". Injected faults receive fresh IDs and appear on the WS stream with a fault-state
change frame. DELETE clears persistent effects at the end of the current tick. Runtime
injection is what the dashboard's fault console and mavfleet's scripted scenarios use;
the timeline is what CI uses for reproducibility.

## 8. Determinism, Replay, and Performance

### 8.1 Determinism Contract

The contract: given the same scenario file (including seed) and the same binary, the sim
produces a bit-identical state trajectory, defined as identical 64-bit hashes of a
canonical serialization of every 100th tick's state vector. The mechanisms, in order of
importance: (1) the model layer never reads wall-clock time, only virtual time; (2) all
stochastic streams are PCG64 seeded from a master seed with per-stream offsets (stream
0: IMU noise, 1: IMU turn-on draw, 2: mag, 3: baro, 4: GPS, 5: wind, 6: initial state
perturbation), so removing a sensor from the scenario changes no other stream; (3) all
floating-point is IEEE 754 sequential code on the sim thread with no reduction reordering
(no rayon, no SIMD assumptions, no fused-multiply-add across platform boundaries
documented as a caveat for cross-CPU bit-exactness); (4) fault effects are pure functions
of (virtual time, event parameters, state).

PX4 itself is not deterministic across runs (its boot has thread scheduling in it), so
the determinism guarantee is deliberately scoped to the sim side. Evaluator-level
assertions (10.3) that compare estimates to truth use tolerance bands rather than
bit equality.

### 8.2 Recording and Replay Format

The replay file is an append-only binary: a 64-byte header (magic, version, scenario
hash, seed, tick rate, start virtual time) followed by fixed 96-byte tick records:
(virtual time us, 16 motor inputs as u8, 17 state floats, 4 fault-active bits, CRC16).
At 200 Hz this is 19.2 kB/s; a ten-minute scenario is 11.5 MB, acceptable for CI
artifacts. The sitsim-sdk provides a reader used by the deterministic replay test
(10.5) and by the dashboard's run review mode. The replay format is not compressed in
v0.1; compression would break the append-only property and the reader simplicity.

### 8.3 Performance Budgets

| Metric | Budget | Rationale |
|---|---|---|
| Tick compute (dynamics + sensors + encode), p95 | 400 us | 12.5x headroom on the 5 ms lockstep period |
| Tick compute, p99.9 | 1.5 ms | Absorbs GC-free but scheduler-bumpy hosts |
| I/O plane tick-to-wire latency, p95 | 250 us | Sim thread hands frames to the writer via bounded channel |
| RSS per vehicle | under 20 MB | Measured prototype PX4 instance is about 15 MB; the sim must not dominate |
| Binary size (release, stripped) | under 3 MB | CI artifact friendliness |
| Cold start to accepting TCP | under 200 ms | Fleet bring-up ordering |

Measurement: the sim thread records per-tick duration in a 1024-sample ring; the status
API and the 10 s log line expose p50/p95/p99/p99.9 and max. Benchmarks (10.6) gate
regressions at the p95 budget with a 20 percent margin.

## 9. Configuration and Scenario Format

### 9.1 TOML Schema

One file, one vehicle, one scenario. Tables and keys (types, defaults, constraints):

| Key | Type | Default | Constraint | Meaning |
|---|---|---|---|---|
| [io] tcp_port | u16 | 4560 | 1024-65535 | HIL listen port (4560 + i convention) |
| [io] api_port | u16 | 8200 | 1024-65535 | Control plane port (8200 + i convention) |
| [io] bind | string | 127.0.0.1 | IP | Bind address |
| [sim] rate_hz | f32 | 200 | 50-400 | Tick and HIL_SENSOR rate |
| [sim] duration_s | f32 | 120 | > 0 | Scenario duration; 0 = until REST stop |
| [sim] seed | u64 | 42 | any | Master RNG seed |
| [vehicle] origin.lat_deg, lon_deg, alt_m | f64 | 47.397770, 8.545580, 500.0 | geodetic bounds | Local NED origin |
| [vehicle] initial.pos_ned_m | [f32;3] | [0,0,0] | alt >= -0.1 | Start position |
| [dynamics] (15 keys) | f32 | Section 5.5 table | positive | All Section 5.5 parameters |
| [sensors.imu] 6 keys | f32 | Section 6.3 | non-negative | Noise, walk, bias, scale, misalignment |
| [sensors.gps] rate_hz, eph_cm, epv_cm, latency_ms, lock_s, pos_noise_m, vel_noise_ms | mixed | 5, 100, 150, 120, 0, 0.8, 0.2 | per-key | GPS behavior |
| [sensors.baro] noise_m, walk_m_per_min | f32 | 0.12, 0.01 | non-negative | Baro behavior |
| [sensors.mag] noise_gauss, hard_iron_gauss | f32 | 0.002, 0.01 | non-negative | Mag behavior |
| [sensors.battery] enabled, capacity_mah | bool, u32 | false, 5200 | - | Discharge model |
| [env] wind_steady_ms | [f32;3] | [0,0,0] | abs <= 15 | NED steady wind |
| [env] turbulence | enum | moderate | light, moderate, severe, off | Dryden preset |
| [env] field.incl_deg, decl_deg, h_gauss | f32 | 67, 2, 0.5 | - | Magnetic model |
| [[fault]] | table array | empty | per Section 7.2 | Timeline fault events |

### 9.2 Validation Rules

Validation happens at load, before any socket is opened, and failures exit 4 with a
message naming the offending key and value. Rules: all rate fields satisfy their bounds;
sensor noise parameters are non-negative; fault references only valid motor indices and
fault types; duration_s = 0 requires REST control to be enabled (it always is); the
scenario hash (SHA-256 of the canonicalized config) is logged and embedded in the replay
header so a replay file always identifies the config that produced it. Unknown keys are
rejected (serde deny_unknown_fields) because a typo silently reverting to default is a
debugging trap, not a convenience.

## 10. Verification and Test Plan

### 10.1 Test Pyramid Overview

The pyramid has four layers, ordered by speed and by dependency on PX4: unit (pure Rust,
milliseconds, no PX4), codec golden vectors (pure Rust), integration (real PX4 binary,
marked #[ignore] by default and run with the PX4_BIN environment variable set), and
benchmarks (criterion). CI runs layers one and two on every push, layer three on a nightly
or on-demand basis with the PX4 artifact cached, and layer four on release branches. The
integration layer is the product: it is the test that says "unmodified PX4 flies on this
simulator", and every milestone acceptance criterion in Section 13 maps to one of its cases.

### 10.2 Unit Tests

Per crate: sitsim-core runs the Section 5.6 validation suite plus geodesy round-trips
(NED to geodetic to NED under 1 mm over a 10 km square) and quaternion norm/energy
invariants over long runs; sitsim-sensors runs statistical tests: the sample variance of
100,000 IMU noise draws falls within a chi-square 99 percent band of the configured sigma,
and the GPS latency queue delivers fixes exactly T_gps late with correct ordering;
sitsim-fault runs timeline membership boundary tests (start, end, overlapping, persistent);
sitsim-mavlink runs golden vectors (known byte strings for each message at known field
values, including the v2 extension-field truncation case and the CRC_EXTRA seed); the
config loader runs schema acceptance, rejection of unknown keys, and range violations.
Target: 90 percent line coverage on the model crates, enforced in CI.

### 10.3 Integration Tests Against Real PX4

The harness spawns a PX4 instance exactly per 3.3 (instance directory, -i N, -d etc),
starts rustsitsim against 4560+N, and drives PX4 through a MAVLink control connection
bound on 14540+N using the same hand-rolled codec (a small control-client module in the
test crate; no pymavlink dependency in the Rust tree). Boot wait: the harness polls
HEARTBEAT arrival and the "Startup script returned successfully" console line for up to
60 s; boot is expected in 2-4 s once HIL_SENSOR flows [OBS baseline: full boot with
streaming sensors achieved].

**Case I-1, boot gate.** Assert: rcS completes (heartbeats on 14540+i, ESTIMATOR_STATUS
streaming), HIL_ACTUATOR_CONTROLS arriving at the sim, ULog file created. This case
regression-tests 3.2/3.3: MAVLink v2 framing with id=0 is what unblocks boot.

**Case I-2, arm, takeoff, hold, land.** Sequence: wait for GPS 3D fix and local position
estimate validity; COMMAND_LONG(400) arm; COMMAND_LONG(22) takeoff to 8 m; hold 30 s;
COMMAND_LONG(21) land; wait disarmed. Assertions (evaluated from the sim's truth and the
control client's LOCAL_POSITION_NED stream): post-settle mean absolute position error
(estimate vs truth) under 0.5 m; altitude band 8 plus/minus 0.7 m during hold; touchdown
vertical speed under 1.0 m/s; total mission completes within 120 s. This is the flagship
test: it is the first time EKF2, commander, position controller, and the mixer all close
through the simulator.

**Case I-3, sensor realism re-run.** Identical to I-2 but with the full sensor error model
(noise, bias walk, GPS latency 120 ms, moderate turbulence). Same bands. Distinguishes
"flies because sensors are perfect" from "flies".

**Case I-4, fault scenarios.** (a) GPS denial 20 s at 30 s: assert EKF does not hard-fail
(position-hold degrades gracefully; innovation flags visible in STATUSTEXT); (b) motor
efficiency 0.55 on motor 2: assert the vehicle remains controllable (or crashes
deterministically if outside envelope - the spec requires the observed behavior to match
the model prediction, not heroics); (c) transport delay 50 ms: assert mission still
completes (lockstep tolerance).

**Case I-5, determinism smoke.** Run I-2 twice with the same seed; assert the two replay
files are byte-identical (full file hash), while PX4-side ULog hashes are allowed to
differ.

### 10.4 Benchmark Suite

criterion benches: single tick (dynamics + sensors + encode) over 10,000 iterations;
codec encode/decode per message; GPS queue churn. Gates: tick p95 under 320 us (80
percent of budget) on the CI runner class; regression alarm on 20 percent p95 growth.
Results are published as a JSON artifact per CI run so performance history is queryable.

### 10.5 CI Pipeline Design

GitHub Actions. Job 1 (every push): fmt + clippy --deny warnings, unit + codec tests on a
3-version matrix (stable, stable minus one, beta), coverage upload. Job 2 (nightly,
cache-keyed on the PX4 commit): fetch or build the pinned PX4 v1.16.2 artifact (the build
takes roughly 10 minutes on 2 cores, so the artifact is cached keyed on the repo SHA; the
exact recipe is Section 11's), then run integration cases I-1 through I-5 and attach
replay files and the PX4 ULogs on failure. Job 3 (release tags): benchmarks plus
cross-checked release build. The pinned PX4 version is a first-class input: a single
variable file (px4-version) and a conformance test that prints the running PX4 version
banner and fails on mismatch, because protocol drift across PX4 versions is the top risk
in the register (Section 14).

## 11. Build and Test Environment (Reference Platform)

### 11.1 Verified Toolchain Matrix

Everything in this matrix was exercised on the reference platform (constrained Linux
container, no sudo, no apt, no Docker, no display server) while bringing up the prototype
and the PX4 build:

| Capability | Status | Notes |
|---|---|---|
| Rust 1.98 via rustup (cargo, crates.io fetch) | verified | tokio test build compiled |
| g++ 14.2 with C++23 | verified | host compiler used by PX4 build |
| Portable CMake 3.30.5 (GitHub release tarball) | verified | no system cmake present |
| ninja via pip | verified | pip install ninja, on PATH |
| Python 3.13 + pip packages | verified | empy 3.3.4, kconfiglib, jinja2, pyserial, pyyaml, jsonschema, pyros-genmsg, pyulog, pymavlink |
| python3 shim to 3.13 | verified | default python3 (3.12 venv) lacks the packages; PX4 scripts invoke python3 |
| git 2.47 with shallow submodule fetch | verified | full recursive init required (see below) |
| TCP/UDP sockets, PTYs | verified | multi-process rig proven |
| Node.js 24 + npm | verified | dashboard toolchain |
| Docker, Gazebo, display server, gdb, sudo/apt | absent | the constraints this design is built around |

### 11.2 PX4 Source Build Recipe (Headless, No Simulator)

The complete recipe that produced a working px4 binary in under 15 minutes on 2 cores:

```text
1. git clone --depth 1 --branch v1.16.2 <px4 repo> PX4-Autopilot
2. cd PX4-Autopilot
   git submodule update --init --recursive --depth 1 --force
   (must run to completion; interrupted init produces
    "Cannot find source file" cmake errors later)
3. git -C platforms/nuttx/NuttX/nuttx fetch --tags --depth 1
   (version header generation needs the tags)
4. pip install --break-system-packages empy==3.3.4 kconfiglib jinja2 \
    pyserial pyyaml jsonschema pyros-genmsg pyulog pymavlink ninja
5. yes | make px4_sitl_default
   (the pipe auto-answers interactive submodule version prompts)
6. artifact: build/px4_sitl_default/bin/px4 plus ~80 module binaries
```

Incremental rebuilds via ninja resume cleanly after interruption, which matters on hosts
with short command timeouts. The `gazebo-classic_iris` model name in 3.3 does not imply
any Gazebo dependency at build time; the px4_sitl_default target alone produces the
headless binary the HIL path needs.

### 11.3 Environmental Constraints and Their Consequences

The reference platform kills long-running background work between invocations and caps
individual commands at roughly ten minutes. Three operational consequences, all verified:
PX4 builds run in the foreground with ninja's incremental resume absorbing any
interruption; scenario runs (PX4 + sim + assertions) are single-invocation scripts that
finish inside the cap (the 90 s CI budget of G-8 doubles as an environment budget); and
the tool assumes nothing persists across runs except files - no daemon mode, no cleanup
processes, sockets are always freshly bound.

### 11.4 Long-Running Build Pattern

For hosts where even incremental builds exceed the cap, the documented pattern is a
build-driver script that (a) invokes the build, (b) on timeout captures the ninja return
state, (c) re-invokes; ninja skips completed translation units, so each invocation makes
forward progress. The identical pattern applies to the integration suite when PX4 boot
plus scenario exceeds a cap: the driver writes a checkpoint of test-case completion and
re-enters only unexecuted cases. This is deliberately boring, scripted, and dependency-free,
because cleverness in build orchestration is where reproducibility goes to die.

## 12. Dashboard (Next.js)

### 12.1 Views

The dashboard is a single Next.js app (App Router) with five views routed client-side,
served from the sim's static assets or as a separate dev server in development. **Live
telemetry:** numeric tiles (virtual time, phase, tick p95, battery) plus strip charts
(spooling 60 s windows of altitude, attitude angles, motor commands, sensor raw vs truth
error). **Trajectory:** a canvas 2D north-up map with the flight path drawn from WS frames,
geofence outline when supplied, fault-event markers dropped at their timeline positions.
**Sensors:** tuning panels for the noise/bias/latency parameters, live-applied through
PUT /api/scenario during WAIT phase or queued for next run. **Faults:** the Section 7
catalog as buttons and forms with an active-fault list; injection is one POST. **Runs:**
replay file browser with scrub-through playback using the same chart components. No 3D
library, no WebGL dependency: everything renders in canvas 2D or plain DOM, which keeps
the dashboard inside the performance budget and usable over remote forwarded ports.

### 12.2 Technology Stack

Next.js 15 App Router, React function components with hooks, a thin zustand store fed by
one WebSocket, native canvas 2D for trajectory and charts (no charting framework - the
data shapes are fixed and the rendering budget is 10 Hz), Tailwind CSS for layout. The
dashboard is deliberately framework-light: it is a debugging instrument, not a product,
and every dependency would have to be justified in review. TypeScript strict mode; a
shared types package generated from the WS frame JSON schema so the Rust side and the
frontend cannot drift silently.

## 13. Milestones and Acceptance Criteria

Estimates assume one engineer, sequential execution, and the Section 11 environment.

| Milestone | Scope | Exit criteria | Est. |
|---|---|---|---|
| M0 | Workspace, CI skeleton, codec + golden vectors | Unit + codec tests green; clippy clean | 3 d |
| M1 | Transport + protocol bring-up | Integration case I-1 passes: real PX4 boots, loop closed, verification items V-1/2/3 resolved and folded back into this spec | 5 d |
| M2 | Dynamics + ideal sensors | Case I-2 passes with noise disabled: arm, takeoff, 8 m hold, land, est-vs-truth under 0.5 m | 7 d |
| M3 | Realistic sensor errors | Case I-3 passes with the full 6.x error model; noise params documented against BMI088 datasheet values | 5 d |
| M4 | Fault engine | Cases I-4a/4b/4c pass; catalog table complete; REST + WS fault surfaces live | 4 d |
| M5 | Determinism + replay + benchmarks | Case I-5 passes (byte-identical replays); bench gates in CI; 8.3 budgets measured and published | 3 d |
| M6 | Dashboard + docs + release | All five views functional against a live run; README, ADRs, examples; v0.1 tag | 6 d |

Total: roughly 5-6 weeks of focused work. M2 is the high-risk gate (EKF closed loop);
the schedule front-loads protocol (M1) precisely because every fact it depends on is
already verified.

## 14. Risk Register

| ID | Risk | L | I | Mitigation |
|---|---|---|---|---|
| R-1 | PX4 protocol drift on version bump (rcS path, message semantics) | M | H | Pin v1.16.2; conformance test prints version banner; upgrade is a deliberate, spec-reviewed change |
| R-2 | EKF2 diverges under realistic noise (case I-3 bands missed) | M | H | Staged realism M2 to M3; parameter tuning documented; fallback: ship I-2-class ideal-sensor mode as default profile |
| R-3 | Actuator range mapping off at endpoints (V-3) | M | M | M1 verification sweep; mixer table cross-check against PX4 source |
| R-4 | PX4 build weight in CI (10 min, cache invalidation) | M | M | Artifact cache keyed on px4-version file; nightly cadence; conformance test guards staleness |
| R-5 | Port collisions in fleet runs (multiple instances) | L | M | Port registry documented (3.1); sim retries with backoff; mavfleet owns allocation |
| R-6 | fields_updated semantics change with decimation (V-1) | L | M | Keep all-bits-until-verified; verify at M3 before shipping per-sensor rates |
| R-7 | Determinism broken by cross-CPU float differences | L | L | Document same-CPU guarantee; replay hash is per-artifact in CI, not cross-runner |
| R-8 | Dashboard scope creep | M | M | The 12.1 view list is frozen for v0.1; changes route through ADRs |

## 15. Repository Layout and Documentation Plan

```text
rustsitsim/
  Cargo.toml                # workspace
  crates/
    sitsim-core/            # dynamics (Section 5)
    sitsim-env/             # environment (6.1, 6.2)
    sitsim-sensors/         # sensor models (6.3-6.5)
    sitsim-fault/           # fault engine (Section 7)
    sitsim-mavlink/         # codec + golden vectors (2.2, 3.5-3.9)
    sitsim-transport/       # TCP + lockstep scheduler (3.1, 3.4, 3.10)
    sitsim-sdk/             # config, assembly, replay (8.2, 9)
    sitsim-cli/             # binary + control plane (Section 4)
  dashboard/                # Next.js app (Section 12)
  tests/                    # integration crate (10.3) + harness
  docs/
    SPEC.md                 # this document, rendered to PDF for review
    PROTOCOL.md             # Section 3 extracted, standalone ICD for external users
    adr/                    # architecture decision records
    examples/               # scenario TOML files per fault and per mission profile
  .github/workflows/        # CI per 10.5
  px4-version               # single line: the pinned PX4 tag
```

License: Apache-2.0 (permissive, patent grant, compatible with mixed ecosystem).
Documentation discipline: this specification lives in-repo as SPEC.md and is updated by
pull request with the same review bar as code; each milestone's exit includes folding
resolved V-items back into the spec text. PROTOCOL.md is the public extract of Section 3,
written for simulator authors who never use rustsitsim itself. Release cadence: v0.1 at
M6, then semver; the protocol ICD gets its own version that moves only with deliberate
PX4-version changes.

## 16. Appendix A: Protocol Verification Log

Empirical record from the reference-platform bring-up (PX4-Autopilot v1.16.2, source
build, headless, 2 instances), preserved as the baseline evidence for Section 3:

| Observation | Evidence |
|---|---|
| PX4 connects as TCP client to 4560+i | simulator_mavlink start -c in px4-rc.mavlinksim; live accept observed |
| MAVLink v2 required; v1 stalls boot at simulator_mavlink start | HIL_SENSOR.id is a v2 extension field; boot gate imu.id == 0 (SimulatorMavlink.cpp:484); lockstep wait at :1638 |
| Rate request arrives after connect | COMMAND_LONG 511 with param1 = 115, param2 = 5000 observed |
| COMMAND_ACK(511, 0) accepted | prototype replies; boot proceeds |
| HIL_STATE_QUATERNION acc are int16 mG, vel int16 cm/s | field-level verification against SimulatorMavlink handling + live estimate stability |
| Full boot achieved with 200 Hz HIL_SENSOR + HIL_STATE_QUATERNION, 5 Hz HIL_GPS | console "Startup script returned successfully"; ESTIMATOR_STATUS, ATTITUDE at 50 Hz; heartbeats sysid 1; ULog files written |
| 2 instances co-exist | instance dirs, -i 0/1, TCP 4560/4561, telemetry 14540/14541, sysid 1/2, ~15 MB RSS each |
| pymavlink observability | mavlink_connection("0.0.0.0:14540+i") binds; PX4 streams to the bound socket |

## 17. Appendix B: Reference Scenario File

The canonical hover-and-fault scenario used by integration cases I-2 through I-4:

```toml
[io]
tcp_port = 4560
api_port = 8200

[sim]
rate_hz = 200
duration_s = 150
seed = 42

[vehicle]
origin = { lat_deg = 47.397770, lon_deg = 8.545580, alt_m = 500.0 }
initial = { pos_ned_m = [0.0, 0.0, 0.0] }

[dynamics]
mass_kg = 1.5
inertia = [0.02, 0.02, 0.04]
arm_m = 0.225
c_T = 1.10e-5
c_M = 1.25e-7
omega_max = 950.0
tau_motor = 0.03

[sensors.gps]
rate_hz = 5
latency_ms = 120
pos_noise_m = 0.8
vel_noise_ms = 0.2
eph_cm = 100
epv_cm = 150

[env]
wind_steady_ms = [1.5, 0.0, 0.0]
turbulence = "moderate"

[[fault]]
id = "denial_1"
type = "gps_denial"
start_ms = 30000
duration_ms = 20000
```

Values shown are the integration-test defaults; the config schema (9.1) is the authority
for the full key set. The file is checked into docs/examples and executed by CI as the
I-2/I-3/I-4 driver, which is the strongest guarantee a spec can have that its example
actually works.
