# mavfleet: Multi-Vehicle PX4 SITL Fleet Manager

**Engineering Specification** | Version 0.1 (draft for implementation) | September 2026

**Reference stack:** rustsitsim v0.1 (this spec's sibling, its Section 3 defines the
per-vehicle simulator interface) and PX4-Autopilot v1.16.2 (pinned in both repos). Facts
inherited from the sibling specification are cited as "rustsitsim 3.x" and are already
empirically verified; facts specific to fleet operation carry their own verification
identifiers (V-10, V-11, ...) so the two numbering spaces never collide.

## 1. Introduction

### 1.1 Purpose

This document is the complete engineering specification for **mavfleet**, a multi-vehicle
fleet manager for PX4 SITL: it spawns and supervises fleets of unmodified PX4 instances,
each flying on a rustsitsim simulator instance; it aggregates fleet state; it allocates
missions through an auction; it streams offboard setpoints, enforces safety policy, and
presents the whole fleet through a Next.js command interface. It is written to be
sufficient for implementation without further design work, in the same style and with the
same evidentiary discipline as the rustsitsim specification.

mavfleet is the layer where "a simulator" becomes "a distributed system". Each vehicle is
three cooperating processes (px4, rustsitsim, the manager's per-vehicle link task) with
independent failure modes, and the manager's job is to make that collection behave like a
fleet: observable, allocatable, failsafe, and eventually - within the modest scope of v0.1 -
coordinated in formation.

### 1.2 Background and Motivation

Multi-vehicle PX4 testing today is ad hoc: scripts from Tools/simulation spawn N instances,
and then every operator writes their own glue to connect, command, and watch them. That
glue is where multi-vehicle work goes to die: it is unpinned, unmonitored, and unaudited;
it has no allocation logic (who flies which task is hardcoded), no failsafe policy beyond
PX4's internal ones, and no determinism story. At the other end of the spectrum,
full GCS-grade mission systems are heavy, UI-first, and assume human-in-the-loop pacing
that automated regression testing cannot tolerate.

The middle ground is what autonomy teams actually need for CI-grade fleet regression: a
headless manager with a clean API that boots a fleet in seconds, allocates tasks by
mechanism rather than by hand, enforces a safety supervisor independent of vehicle
cooperation, and records everything. The per-vehicle substrate already exists - rustsitsim
proves that N independent lockstep sims run at 15 MB each - so the marginal problem is
coordination, and coordination is exactly what this specification pins down.

### 1.3 Goals

| ID | Goal | Measure |
|---|---|---|
| G-10 | Spawn and supervise a fleet of N vehicles (2-8) | Fleet READY under 90 s from cold start; per-vehicle health visible |
| G-11 | Aggregate fleet state with staleness control | 10 Hz supervisor tick; per-vehicle telemetry staleness under 1.5 s flags the vehicle |
| G-12 | Mission allocation by auction | Allocation within 5 percent of the Hungarian optimum on random task sets; deterministic given seed |
| G-13 | Offboard control per vehicle | 20 Hz setpoint streams; offboard engage and RTL on command |
| G-14 | Independent safety supervisor | Geofence, heartbeat-loss ladder, battery ladder, E-stop; supervisor actions logged and enforced even against a wedged vehicle process |
| G-15 | Formation flight demo | 4 vehicles in a 10 m grid hold plus leader-directed transit |
| G-16 | Fleet scenario DSL and CI harness | Link-loss, fault, and completion assertions in one TOML; runs headless under 10 min |
| G-17 | Operator dashboard | Fleet map with geofence, vehicle cards, task board, event log |

### 1.4 Non-Goals

v0.1 is deliberately a fleet manager, not a fleet autonomy stack. Cut list with reasons:
**Onboard (vehicle-side) swarm software** - mesh radios, distributed consensus on vehicles -
is out; the coordinator is a ground segment, and that is an honest architecture for SITL
where a manager process is natural. **Full collision avoidance (ACAP-style)** - out; the
allocator deconflicts by altitude layer and the safety supervisor enforces separation
minimums at planning time, but reactive sense-and-avoid is a research problem, not a
regression-test dependency. **Camera/mission payloads** - out; the Anduril-shaped payload
integration question is served by the port map (2.2) reserving payload links, not by
simulated payload hardware in v0.1. **Dynamic replanning** - out; allocation runs at
mission start and at explicit re-allocation triggers, not continuously. **Cross-machine
distribution** - out; one manager process, one host. The manager's process model is
however message-driven end to end, so growing beyond one host later means changing the
transport, not the architecture.

### 1.5 Target Users

Autonomy and mission software engineers who want a fleet-in-a-loop regression rig;
PX4 contributors testing multi-instance behaviors (companion links, sysid separation,
bandwidth); and the operator reviewing fleet behavior through the dashboard. The design
assumes its most demanding user is CI: every capability is scriptable, every decision is
logged with a cause, and every run is reproducible from a scenario file plus two binaries.

### 1.6 Glossary

| Term | Meaning |
|---|---|
| Supervisor | mavfleet's control loop: 10 Hz tick over all vehicles, policy enforcement |
| Vehicle | The triple (px4 instance i, rustsitsim instance i, link task in the manager) |
| Task | A mission unit (v0.1: waypoint visit) with position and reward metadata |
| Auction / CNT | Contract-net protocol: announce, bid, award rounds |
| Hungarian algorithm | Optimal assignment for linear-sum cost; the allocator's baseline |
| Offboard | PX4 flight mode where the vehicle tracks an external setpoint stream |
| type_mask | Bitmask in SET_POSITION_TARGET_LOCAL_NED marking which fields are active |
| Geofence | Inclusion polygon plus altitude ceiling enforced by the supervisor |
| RTL | Return to launch: PX4 autonomous flight mode, also supervisor-enforced |
| Custom mode | PX4's MAVLink mode encoding: (main mode << 8) or sub-mode, in COMMAND_LONG 176 |

## 2. System Architecture

### 2.1 System Context

One manager process supervises N vehicles. Per vehicle i (0-based): a px4 process launched
per the rustsitsim 3.3 recipe (instance dir, -i i, PX4_SIM_MODEL=gazebo-classic_iris), a
rustsitsim process listening on TCP 4560+i, and the manager's link task speaking MAVLink
over the instance's telemetry UDP endpoint. The manager additionally hosts the fleet
REST/WS API and the dashboard's data feed. rustsitsim's own control plane (its Section 4)
is used by mavfleet for per-vehicle fault injection, so fleet scenarios can kill a vehicle's
GPS or degrade a motor through the same API the single-vehicle dashboard uses.

```text
                     +--------------------------------------+
  Next.js dashboard <-|        mavfleet manager process      |
  REST / WS clients   |                                      |
                      |  supervisor tick (10 Hz)             |
                      |  allocator (auction)   safety        |
                      |  offboard setpoint streams (20 Hz)   |
                      +--+--------------+--------------+-----+
                         |              |              |
              vehicle 0  |   vehicle 1  |   vehicle 2  |  ...
            +------------+ +------------+ +------------+
            | px4  -i 0  | | px4  -i 1  | | px4  -i 2  |
            | rustsitsim | | rustsitsim | | rustsitsim |
            |  :4560+i   | |  :4561     | |  :4562     |
            +------------+ +------------+ +------------+
                  MAVLink UDP 14540+i (link tasks bind here)
```

The manager never talks to the HIL links; those belong to the sims. Its MAVLink footprint
is the same one any GCS would use - heartbeat, command, setpoint, telemetry - applied N
times with disciplined source addressing (manager sysid 255, component 190, one
MAVLink channel per vehicle).

### 2.2 Crate Decomposition

| Crate | Responsibility | Key dependencies |
|---|---|---|
| fleet-core | Vehicle registry, supervisor tick, health model, event log | tokio |
| fleet-mavlink | Link tasks: MAVLink v2 codec (shared subset), per-vehicle connection, heartbeat, command client, setpoint stream, telemetry decode | sitsim-mavlink (codec reuse), tokio |
| fleet-modes | PX4 custom mode constants and SET_MODE encoding, unit-tested against the pinned PX4 header | none |
| fleet-alloc | Task model, bid function, contract-net rounds, Hungarian baseline, greedy fallback | none |
| fleet-safety | Geofence (polygon + ceiling), policy engine (heartbeat ladder, battery ladder, separation), action issuance | fleet-core |
| fleet-mission | Scenario DSL parse, mission compiler (task graph, altitude deconfliction), run orchestrator | fleet-alloc, sitsim-sdk |
| fleet-simctl | Process supervision: spawn px4 + rustsitsim per vehicle, port allocation, teardown, crash restart policy | tokio, sitsim-sdk |
| fleet-cli | Binary: scenario load, fleet bring-up, REST/WS plane, exit codes | all above, axum |

The codec is shared with rustsitsim by git dependency on the sitsim-mavlink crate, extended
in fleet-mavlink with the GCS-side messages (HEARTBEAT 0, SYS_STATUS 1, ATTITUDE 30,
LOCAL_POSITION_NED 32, GLOBAL_POSITION_INT 33, BATTERY_STATUS 147, STATUSTEXT 253,
COMMAND_LONG 76, COMMAND_ACK 77, SET_POSITION_TARGET_LOCAL_NED 84, HOME_POSITION 242).
This is a deliberate ICD reuse decision: one MAVLink v2 codec, golden-tested once, used
from both sides of the wire.

### 2.3 Supervisor Tick and Data Flow

The supervisor is a 10 Hz loop over the vehicle registry. Each tick, per vehicle: the link
task's latest decoded snapshot (already in shared state via a lock-free per-vehicle slot)
is freshness-checked; the safety engine evaluates its policy set (Section 8) against the
snapshot and the vehicle's FSM (Section 4); any resulting action (mode change, setpoint
override, terminate) is queued to the link task; the allocator is invoked if a
(re)allocation trigger is armed; the aggregate view is serialized for the WS plane. The
tick is strictly bounded: no blocking I/O, no awaits on links - everything is state reads
and channel writes - so tick time is a function of N and the policy evaluation cost, which
is the 10.x budget's concern.

Setpoint streaming runs outside the tick at 20 Hz per vehicle in the link task's own
timer, because PX4's offboard mode requires a stream faster than 2 Hz and the tick must
not be coupled to it. Heartbeats are sent per link at 1 Hz. Telemetry decode is event
driven. This gives three per-vehicle cadences (decode: as-arrives, setpoint: 20 Hz,
supervisor: 10 Hz) plus the manager's REST plane, all on one tokio runtime with the
supervisor task pinned at higher priority.

### 2.4 Process Supervision Model

fleet-simctl owns process topology. Bring-up order per vehicle: (1) create the PX4
instance directory (copy pattern per Tools/simulation/sitl_multiple_run.sh, verified in
the rustsitsim bring-up); (2) start rustsitsim with its scenario file and port 4560+i
assigned; (3) start px4 with -i i, -d etc, PX4_SIM_MODEL=gazebo-classic_iris; (4) wait for
the vehicle's BOOTING condition (Section 4) - heartbeats on 14540+i and local position
estimate validity. Ordering matters: the sim must be listening before px4's
simulator_mavlink connect attempt, and px4's rcS blocks until HIL_SENSOR flows
(rustsitsim 3.3), so in practice the manager waits on the sim's status API, then starts
px4, then waits for telemetry.

Crash policy: a dead px4 or rustsitsim process transitions its vehicle to FAULT, logs a
supervisor event with the exit code, and (policy permitting: scenario flag
restart_on_fault) triggers a full vehicle restart (new instance directory, same ports
after a 2 s socket-drain). The manager itself never crashes silently: it runs the whole
fleet scenario to completion (or failure), writes the event log and run report, and exits
with a code the CI driver can classify.

## 3. Fleet Interfaces (ICD)

### 3.1 Per-Vehicle MAVLink Links

Each link task binds UDP 127.0.0.1:14540+i - the endpoint PX4's normal rcS mavlink
instance streams to [OBS; rustsitsim 3.1 port table]. PX4 sends telemetry to whatever
socket has bound that port (verified with pymavlink and with a Rust prototype bind).
The manager sends commands and setpoints to PX4's listen port 14580+i from the same
socket. Manager addressing: SYSID 255, COMPID 190 (MAV_COMP_ID_ONBOARD_COMPUTER
convention), one MAVLink v2 channel per vehicle with independent sequence numbers.
Vehicle addressing is dictated by PX4: sysid i+1, component 1.

Every link task sends HEARTBEAT at 1 Hz with type MAV_TYPE_GCS, autopilot
MAV_AUTOPILOT_INVALID, and immediately on connection requests the telemetry set it needs
via COMMAND_LONG 511 (SET_MESSAGE_INTERVAL): ATTITUDE and LOCAL_POSITION_NED at 20 Hz,
SYS_STATUS, GLOBAL_POSITION_INT, BATTERY_STATUS, HOME_POSITION, and STATUSTEXT at
default cadence [V-10: confirm which of these are already streamed by the SITL rcS mavlink
instance without explicit request, to minimize control traffic; the request path is
conformant either way].

### 3.2 Command Outbound Contract

All vehicle commands flow through COMMAND_LONG (76) with COMMAND_ACK (77) awaited, 500 ms
retry, three attempts, then a supervisor-level escalation (the action is marked FAILED
and the safety engine decides whether the vehicle is unresponsive). Command set for v0.1:

| Command | MAV_CMD | Params used | Purpose |
|---|---|---|---|
| Arm / disarm | 400 | p1 = 1/0 | Arming gate before offboard or takeoff |
| Takeoff | 22 | p7 = target altitude | Used in the non-offboard bring-up profile |
| Land | 21 | - | Terminal descend at position |
| RTL | 20 | - | Supervisor-enforced return |
| Set mode | 176 | p1 = MAV_MODE_FLAG_CUSTOM, p2 = custom mode word | Mode transitions incl. offboard |
| Set message interval | 511 | p1 = msg id, p2 = interval us | Telemetry subscription |

Mode words are assembled in fleet-modes from the PX4 custom mode scheme: custom =
(main << 8) or sub, with main modes MANUAL 1, ALTCTL 2, POSCTL 3, AUTO 4, ACRO 5,
OFFBOARD 6, STABILIZED 7, and AUTO sub-modes READY 1, TAKEOFF 2, LOITER 3, MISSION 4,
RTL 5, LAND 6 [V-11: verify the full constant table against the pinned
ROMFS/px4fmu_common/init.d-posix/px4_custom_mode.h at M0; the constants become a
unit-tested table that fails build on drift]. The manager uses POSCTL, AUTO RTL, AUTO
LAND, and OFFBOARD only.

### 3.3 Offboard Setpoint Stream

SET_POSITION_TARGET_LOCAL_NED (84) at 20 Hz per vehicle in offboard. Fields: time_boot_ms
(from the link's clock), target_system = i+1, target_component = 1, coordinate_frame =
MAV_FRAME_LOCAL_NED (1), type_mask (below), x, y, z (NED meters), vx, vy, vz (m/s), afx..
(not used), yaw (rad), yaw_rate (rad/s). Setpoints are computed by the mission runner or
formation controller as NED positions relative to the vehicle's home, which is how PX4's
local frame is anchored in SITL [OBS: home position set at boot].

type_mask bits (1 = ignore the field group): bits 1-3 ignore x, y, z; bits 4-6 ignore
vx, vy, vz; bits 7-9 ignore accel; bit 10 force; bit 11 yaw; bit 12 yaw_rate [V-12:
encode the exact mask values used by PX4's flight_mode_manager acceptance path as
constants POSITION_ONLY and VELOCITY_YAWRATE, and verify with a 30 s offboard hold at M2
of the mavfleet plan]. Semantics used in v0.1: position setpoints with yaw (mask ignores
velocity and accel), and velocity setpoints with yaw_rate (mask ignores position and
accel). Offboard engage sequence: start streaming setpoints (hold at current position),
wait 200 ms, then COMMAND_LONG 176 to OFFBOARD, then treat mode feedback as the engage
confirmation; if PX4 drops out of offboard (heartbeat mode change), the stream restarts
the sequence.

### 3.4 Fleet Control Plane (REST / WS)

One axum server on 127.0.0.1:8400 (configurable). Envelope identical to rustsitsim's
(ok/data/error), so dashboard code shares one client. Endpoints:

| Endpoint | Method | Purpose |
|---|---|---|
| /api/fleet | GET | Fleet summary: per-vehicle FSM state, health, battery, position, plus fleet phase |
| /api/fleet | PUT | Load and start a fleet scenario (body: scenario TOML as JSON) |
| /api/fleet/estop | POST | E-stop: every vehicle LAND immediately, scenario marked ABORTED |
| /api/vehicles/{i} | GET | Full vehicle detail incl. telemetry snapshot age, link counters |
| /api/vehicles/{i}/mode | POST | Request mode change (supervisor policy still applies) |
| /api/vehicles/{i}/faults | POST | Inject a rustsitsim fault on vehicle i (proxied to its 8200+i plane) |
| /api/tasks | GET / POST | Inspect task set; append tasks at runtime (triggers reallocation) |
| /api/events | GET | Event log tail (supervisor decisions, fault injections, state changes) |
| /ws/fleet | WS | 5 Hz fleet state frames + event push |

The WS frame carries the same fleet summary as GET /api/fleet at 5 Hz plus incremental
events; the dashboard subscribes once and drives all views from it. Frame schema is
versioned in a shared JSON schema like the rustsitsim frame.

## 4. Vehicle State Machine

Each vehicle runs one explicit FSM in fleet-core. States and transitions (trigger on the
arrow, action in the state):

| State | Meaning | Exits |
|---|---|---|
| INIT | Registry entry allocated, nothing spawned | → SPAWNING (supervisor starts bring-up) |
| SPAWNING | Processes starting, sim listening | → BOOTING (px4 launched, sim TCP accepted) |
| BOOTING | px4 rcS gated on HIL stream; waiting telemetry | → READY (heartbeats + local position valid + home set); → FAULT (process death) |
| READY | Vehicle available for tasking | → ACTIVE (task assigned and accepted); → FAULT |
| ACTIVE | Flying a task: offboard stream or auto mode | → RTL (task complete / battery / geofence / supervisor); → FAULT |
| RTL | Returning to home (PX4 AUTO RTL or supervisor-driven) | → LANDED (disarm observed); → FAULT |
| LANDED | On ground, disarmed | → READY (re-arm cycle allowed by scenario); terminal otherwise |
| FAULT | Process death, command escalation, or estop-during-spawn | → SPAWNING (only if restart_on_fault) |

Transition discipline: every arc is recorded in the event log with (vehicle, from, to,
cause, virtual time). The FSM is the vocabulary of every other subsystem: the allocator
bids only from READY, the safety engine escalates along FSM arcs, and CI asserts on FSM
traces (Section 11). A vehicle whose telemetry goes stale for more than 1.5 s is flagged
STALE in health while its FSM state is retained - staleness is an input to policy, not a
state, by design, so a flapping link does not thrash the FSM.

## 5. Telemetry Aggregation and Health

### 5.1 VehicleState Model

The manager's per-vehicle shared state is one struct, written by the link task's decode
path (single writer) and read by everything else (snapshot copy under a seqlock-style
reader), versioned and stamped with the arrival time:

```text
VehicleState {
  sysid, component, fsm: FsmState,
  position_ned_m: [f32;3],        // from LOCAL_POSITION_NED (vehicle's own estimate)
  attitude_q_wxyz: [f32;4],       // from ATTITUDE
  velocity_ned_ms: [f32;3],       // from LOCAL_POSITION_NED
  global_fix: lat_degE7, lon_degE7, alt_mm,   // GLOBAL_POSITION_INT
  battery_pct, voltage_v,         // SYS_STATUS / BATTERY_STATUS
  home_ned_m: [f32;3],            // HOME_POSITION
  mode, armed: bool,              // HEARTBEAT decode
  last_heartbeat_ms, last_msg_ms, // staleness inputs
  link: { sent, recv, retries, cmd_failures },
  sim: { phase, tick_p95_us, faults_active }  // polled from rustsitsim status
}
```

Ground truth (the simulator's state, not the vehicle's estimate) is deliberately kept in
the sim block, refreshed at 2 Hz from each rustsitsim status API. Assertions in CI compare
vehicle estimates against sim truth per vehicle, the same discipline as rustsitsim 10.3;
the manager itself never conflates the two.

### 5.2 Health Flags and Event Log

Health is a flag set, computed per tick: LINK_STALE (no telemetry 1.5 s), HEARTBEAT_LOST
(no heartbeat 3 s), BATTERY_LOW (30 percent), BATTERY_CRIT (20 percent), GEOFENCE_WARN
(within 10 m of a boundary), OFFBOARD_DROPOUT (mode left offboard unexpectedly),
SIM_DEGRADED (sim status reports tick p95 over budget). Flags raise supervisor events
once on rising edge, never repeatedly - the event log is for decisions, not spam.

The event log is an append-only in-memory ring (10,000 events) plus a newline-delimited
JSON file per run: {t, kind, vehicle, detail}. Kinds: FSM transition, supervisor action,
fault injection, task award, link escalation, run boundary. It is the run report's spine;
CI post-processing asserts on it directly because it is smaller and more structured than
telemetry.

## 6. Mission Allocation

### 6.1 Task Model

A task is a waypoint visit in v0.1: {id, position_ned_m, hover_s, reward, deadline_s}.
The mission compiler converts the scenario's task list into allocation-ready inputs:
waypoint deconfliction by altitude layer (vehicle k flies at 10 + 5k m AGL during
transit, converging to the task's hover altitude only on final approach when the layer
is clear - a planner-level deconfliction, honestly not reactive avoidance), and per-task
reachability given the geofence and battery budget. Tasks that no vehicle can reach
within battery reserve are rejected at compile time with an event log entry, not
discovered mid-flight.

### 6.2 Bid Function

Vehicle v bids cost(v, t) in seconds of equivalent mission time:

cost = travel_time(v, t) + queue_ahead(v) + battery_penalty(v, t)

where travel_time uses the vehicle's current position to task position at cruise speed
(4 m/s default) with a 25 percent path factor for non-homotopic routes (simple padding,
not path planning), queue_ahead is the remaining time of already-awarded tasks, and
battery_penalty is a soft wall: zero above 45 percent charge, growing quadratically to a
large value at 25 percent, so the auction never assigns a task the battery ladder will
abort. The bid is deterministic given fleet state; the auction's only nondeterminism
input is evaluation order, which is fixed by vehicle index.

### 6.3 Auction Protocol (Contract-Net)

Allocation runs as sequential single-item auctions: for each unassigned task (ordered by
deadline then reward), every READY or ACTIVE vehicle submits a bid; the manager awards
the minimum and appends the task to that vehicle's queue; bids are recomputed after each
award (queue effects). This is the classic sequential auction: it is not optimal in
general, but it is fast (O(T x V) bids), greedy-optimal on identical vehicles, and within
a small factor of optimal on the instance families tested (6.4). A full contract-net
negotiation (announce, bid, award messages on a real message bus) is intentionally not
simulated: the manager is both auctioneer and bidder-side proxy, and faking message
rounds would add latency without adding information. What is real is the bid computation
per vehicle (run against that vehicle's state) and the award dispatch (per-vehicle task
queues that the offboard stream executes).

### 6.4 Optimality Baseline and Acceptance

fleet-alloc also implements the Hungarian algorithm (O(n cubed), hand-rolled, unit-tested
against brute-force permutations for n up to 6) over the linear cost
sum over assignments of travel_time (queue effects dropped, since Hungarian does not
model them). Acceptance criterion for the auction: on 1,000 random instances (T in
10-50, V in 2-8, uniform field 200 m square), the auction's total travel cost is within
5 percent of the Hungarian optimum in at least 95 percent of instances, and never worse
than the greedy-nearest baseline. This is a testable, honest claim about a greedy
allocator, and it is the number CI enforces every run.

### 6.5 Runtime Reallocation

Two triggers arm reallocation: a vehicle transitioning to FAULT or RTL with tasks still
queued (its remaining tasks return to the pool), and an operator POST /api/tasks
appending tasks. Reallocation is the same sequential auction over the pool, restricted to
vehicles that can still accept work. Mid-task abort: a vehicle currently flying a task
that loses the task (never happens - tasks are only pooled from queues, not from the
active task) or completes with pool remaining picks up the next queued item
automatically.

## 7. Offboard Control and Formations

### 7.1 Setpoint Generation

The mission runner walks each vehicle's task queue: climb to transit altitude (layer per
6.1), fly to the target's horizontal position, descend to hover altitude, hold for
hover_s, mark complete, next. Setpoints are position targets (type_mask POSITION_ONLY)
with a velocity-limited profile: the generator saturates commanded displacement per
20 Hz step so effective velocity is capped at cruise - a trapezoidal-lite profile without
acceleration shaping, sufficient for estimator-grade testing and honest about it.
Yaw targets face the direction of travel, or hold the current heading when hovering.

### 7.2 Offboard Engage and Streaming Discipline

Engage follows 3.3's sequence; the discipline that matters is failure handling. If the
mode command is not ACKed, the stream keeps flowing (PX4 will not enter offboard without
the stream, and an old stream is harmless) and the supervisor escalates per 3.2 after
three failures. If PX4 exits offboard spontaneously, the mission runner pauses the
profile, re-anchors the setpoint to the current position, and re-engages; a second
consecutive dropout on the same task transitions the vehicle to RTL with an
OFFBOARD_DROPOUT event. All setpoints are inside the geofence by construction - the
generator clamps to the polygon shrunk by 2 m, so the safety supervisor's geofence
response (8.2) never sees a violation caused by the manager's own commands.

### 7.3 Formation Flight

v0.1 ships one formation: a 4-vehicle square grid with 10 m spacing, leader-relative.
The leader is vehicle 0 flying the mission profile; followers hold slots at fixed body-
frame offsets (rotating with the leader's yaw, so the formation translates and yaws as a
rigid body). Follower control is a 2D separation-hold: each follower's position setpoint
is the leader's position plus the rotated offset, with a P-controller term on slot error
saturating the velocity cap. Altitude holds individual layers minus 1 m overlap guard.
The formation is a demo target and a stress test: it exercises per-vehicle setpoint
streams, mode management, and the safety supervisor's separation monitor simultaneously.
It is explicitly not swarm autonomy; it is choreography with a monitor.

## 8. Safety Supervisor

### 8.1 Policy Engine

The supervisor evaluates, per vehicle per tick, an ordered policy list; the first
triggered policy issues its action and short-circuits the rest (order is the escalation
ladder). All actions are FSM transitions plus a MAVLink command plus an event log entry.

| Priority | Policy | Condition | Action |
|---|---|---|---|
| 1 | E-stop | operator estop or scenario estop | LAND all vehicles immediately, ABORT run |
| 2 | Geofence breach | position outside polygon or above ceiling (estimate AND sim truth agree) | RTL now; if outside by more than 10 m, LAND at position |
| 3 | Heartbeat loss | no heartbeat 3 s | RTL (PX4 internal failsafe also fires; supervisor verifies mode change, escalates at 10 s to LAND command retry) |
| 4 | Battery critical | under 20 percent | LAND |
| 5 | Battery low | under 30 percent | RTL after current task completes (no new tasks assigned) |
| 6 | Staleness | telemetry stale 1.5 s | flag; if stale while ACTIVE beyond 5 s, RTL by policy 3 logic |
| 7 | Separation | two ACTIVE vehicles within 4 m horizontally and 2 m vertically (from sim truth) | altitude-divergence setpoint override (plus layer discipline making it rare by construction) |
| 8 | Command failure | 3 consecutive command failures | RTL; if command path dead too, FAULT the vehicle |

The battery ladder only has teeth when battery simulation is enabled in the vehicles'
rustsitsim configs (its 6.5); scenario profiles that disable discharge disable policies
5 and 4 with a warning event, rather than silently never triggering.

### 8.2 Geofence Model

Inclusion polygon (convex or concave, self-intersection rejected at parse) in local NED
plus a ceiling and a floor. Point-in-polygon is the ray-casting algorithm, unit-tested
against 1,000 random points with ground truth from an independent winding-number
implementation. Breach detection uses the vehicle's position estimate (that is what a
real supervisor has) but requires sim-truth concurrence before a hard action, because a
GPS glitch (injectable via rustsitsim F-06) must not weaponize the supervisor into
landing a healthy aircraft - the disagreement itself is a supervisor event and, if it
persists 3 s, treated as breach anyway (fail-safe over fail-certain).

## 9. Scenario DSL

### 9.1 Schema

A fleet scenario is one TOML: fleet parameters, per-vehicle overrides, the geofence, the
task set, the timeline (faults and link events), and success criteria. Per-vehicle rust
sitsim configs are generated by the manager from defaults plus overrides, so a scenario
file never repeats 60 lines of sensor parameters per vehicle unless it wants to.

| Key | Type | Default | Meaning |
|---|---|---|---|
| [fleet] count | u8 | 2 | Vehicles to spawn (1-8) |
| [fleet] battery_sim | bool | true | Enable per-vehicle battery discharge (safety ladder needs it) |
| [fleet] restart_on_fault | bool | false | Auto-restart FAULT vehicles |
| [fleet] tick_hz | u8 | 10 | Supervisor rate (fixed 10 in v0.1; key reserved) |
| [env] geofence.points_ned_m | [[f32;2]] | 100 m square | Inclusion polygon vertices |
| [env] geofence.ceiling_m, floor_m | f32 | 60, 0 | Altitude box |
| [env] wind_steady_ms, turbulence | [f32;3], enum | 0, moderate | Passed to every vehicle's sim config |
| [tasks] entries | array | - | Task list: id, pos_ned_m, hover_s, reward, deadline_s |
| [[vehicle_override]] | table array | - | index + partial rustsitsim config patch |
| [[event]] | table array | - | Timeline: fault (proxied to a vehicle's sim), link_loss (drop manager heartbeat reception for vehicle i, duration), estop |
| [success] | table | - | Criteria: all_tasks_done, all_landed, max_time_s, no_geofence_breach, fsm_trace (ordered arc assertions) |

### 9.2 Success Criteria and Run Report

The run report (JSON, plus the event log) is written at run end regardless of outcome:
fleet summary at end, per-vehicle FSM trace, task completion table, supervisor actions,
and criterion evaluation. Success criteria are pure predicates over the report's own
data, evaluated by the same code path in CI and in the dashboard's run review, so what CI
asserts is exactly what the operator sees. The canonical fleet scenario (Appendix B)
exercises the full ladder: 2 vehicles, 4 tasks, a GPS denial, a link loss, battery
ladder triggers, and completion assertions.

## 10. Performance and Determinism

### 10.1 Budgets

| Metric | Budget | Rationale |
|---|---|---|
| Supervisor tick, p95, 8 vehicles | 5 ms | 20x headroom on the 100 ms tick |
| MAVLink decode per message, p95 | 50 us | 20 Hz telemetry plus heartbeats is under 200 msg/s per vehicle |
| Setpoint stream jitter, p95 | 10 ms | 20 Hz stream with tokio timer slack |
| Fleet READY latency, 2 vehicles | 90 s | Spawn + boot + GPS lock budget from rustsitsim's 90 s CI cycle |
| Manager RSS, 8 vehicles | under 150 MB | 8 x (px4 15 MB + sim 20 MB) is the substrate's ~280 MB; the manager's own share must stay small |
| Event log write, p99 | 1 ms | Append-only, no fsync in the hot path |

CPU budget is the honest constraint: each PX4 under lockstep plus each rustsitsim at
200 Hz consumes a fraction of a core when idle-booted (measured prototype: 2 instances
comfortable on 2 cores), but ACTIVE flight raises PX4's compute. The scaling claim this
spec makes is therefore modest and measured, not aspirational: 8 vehicles on an 8-core
host with the canonical scenario profile; the CI matrix runs 2 and 4.

### 10.2 Determinism

The manager is deterministic in its decisions (bid function, auction order, policy
order) and nondeterministic in timing (real processes, real network), and the spec
treats that split as the contract: allocation outcomes and FSM traces are reproducible
for a given scenario and event schedule, wall-clock timings are not. Scenario timelines
(faults, link losses) fire on supervisor virtual time (the manager's fleet clock,
advanced from the earliest vehicle sim clock), so the causal order of injected events
is stable across runs. CI assertions therefore target event ordering and report data,
never timestamps.

## 11. Verification and Test Plan

### 11.1 Unit Tests

fleet-alloc: Hungarian vs brute force to n = 6 (all 720 permutations), auction gap
statistics on seeded instance families (the 6.4 criterion as a hard test with the seed
pinned), bid-function monotonicity (battery down, bid up), reallocation pooling
invariants. fleet-safety: point-in-polygon cross-implementation, policy ordering
(property: no policy with lower priority number ever fires when a higher one is
triggered), geofence parse rejection (self-intersection, degenerate). fleet-modes:
constants table against the pinned px4_custom_mode.h values (the V-11 test), mode word
encoding round-trip. fleet-core: FSM arc coverage (every transition legal, every
illegal transition rejected with an event), seqlock reader under concurrent writer
(loom-style stress test, bounded time). Link codec: golden vectors for the GCS-side
messages, extending rustsitsim's suite.

### 11.2 Integration Tests (Fleet, Real PX4 + Real sims)

Run with FLEET_PX4_DIR and FLEET_SITSIM_BIN set; the harness is the CLI itself driven
by a scenario file, so integration tests exercise exactly the shipped code path.

**Case F-1, two-vehicle bring-up.** Scenario: count 2, no tasks. Assert: both vehicles
reach READY within 90 s; both sims report loop-closed; event log contains the full spawn
trace; teardown leaves no processes and no bound ports (probe check).

**Case F-2, auction mission.** Scenario: 2 vehicles, 4 tasks spread across the field,
battery sim on. Assert: all tasks complete; each vehicle flew at least one task (the
auction actually split work - with 4 tasks at spread positions, one vehicle taking all
four should cost strictly more, and the test asserts the split); every task's hover
observed in telemetry (position within 1 m of the task point for hover_s); run report
evaluates all_tasks_done and all_landed true; est-vs-truth per vehicle under 1 m p95
(the rustsitsim I-3 discipline, fleet-wide).

**Case F-3, link loss failsafe.** Scenario: F-2 plus a link_loss event on vehicle 1 at
40 s for 15 s. Assert: vehicle 1 transitions ACTIVE to RTL within 4 s of loss (3 s
heartbeat ladder plus a tick); on link restoration it is in RTL or LANDED, never
resumed autonomous flight without supervisor knowledge; the remaining vehicle completed
the pooled tasks; event log shows the escalation arcs in order.

**Case F-4, GPS denial, supervisor sanity.** Scenario: F-2 plus rustsitsim F-05 denial
on vehicle 0 for 20 s. Assert: no geofence false breach (the 8.2 concurrence rule -
the GPS glitch path must not trigger RTL); vehicle completes its tasks or degrades
gracefully; disagreement events logged.

**Case F-5, battery ladder.** Scenario: count 2 with accelerated battery sim (a
vehicle_override patching the discharge rate), tasks requiring long loiter. Assert:
BATTERY_LOW RTL and BATTERY_CRIT LAND both fire in order, all vehicles end LANDED, no
task is still claimed at end (pool drained or explicitly failed).

**Case F-6, four-vehicle formation.** Scenario: count 4, formation demo profile.
Assert: grid maintained within 1.5 m RMS of slot positions over a 60 s transit;
separation never under the 4 m/2 m policy floor (monitor events allowed, breaches
forbidden); all vehicles RTL and land at end.

**Case F-7, estop.** Scenario: F-2 with an estop at 60 s. Assert: all ACTIVE vehicles
receive LAND within one tick (event timestamps), scenario marked ABORTED, exit code 2
with the report intact.

### 11.3 CI Matrix and Cadence

Push CI: unit tests, clippy --deny warnings, dashboard typecheck. Nightly: F-1 through
F-7 with the PX4 artifact from the rustsitsim cache (the same pinned binary, one cache,
both repos). Weekly stress: F-2 with count 4 on the 8-core runner, plus a soak (F-2
looped three times) to catch socket and process leakage across runs. Fleet integration
runtime budget: each case under 10 minutes wall clock including spawn and teardown
(measured basis: 2 instances boot in seconds once sims stream; the budget covers the
slow paths).

## 12. Sandbox Environment and Multi-Instance Topology

### 12.1 Spawn Topology (Verified Pattern)

Per vehicle i, the manager creates build-style instance directories (the
sitl_multiple_run.sh pattern, verified during the rustsitsim bring-up): a working
directory per instance under the run's sandbox (px4 writes ULogs, params, and its
airframe state there), px4 launched with -i i and -d pointing at the shared etc
directory, rustsitsim with tcp_port 4560+i and api_port 8200+i. The manager's own ports:
API 8400, plus the per-vehicle sim API proxies. The complete port map for one vehicle is
rustsitsim 3.1's table; the manager enforces the allocation by construction (it is the
only process that spawns anything, so it can never double-allocate).

### 12.2 Resource Observations and Sizing

From the reference-platform bring-up: two fully booted PX4 instances plus two streaming
sims ran comfortably within a 2-core budget with ~15 MB RSS per px4 instance and telemetry
flowing at the rcS default rates. Extrapolation to 8 vehicles is budgeted, not assumed:
the CI matrix caps at 4 until F-2-at-4 measurements exist, and the scaling note in 10.1
records the exact claim made. Memory is the comfortable axis (8 x ~35 MB total substrate
plus manager is well under a typical 2 GB container); CPU under simultaneous ACTIVE
flight is the one to measure at M3 of the milestone plan.

### 12.3 Single-Invocation Orchestration

The reference platform's constraints (rustsitsim 11.3) apply to fleet runs a fortiori:
the CLI executes an entire scenario - spawn, boot, fly, assert, teardown - as one
process invocation that finishes well inside the 10-minute cap, writes the run report
and event log to disk, and reaps every child process it started (teardown is verified by
F-1's port-and-process probe). There is no daemon mode and no cross-run state; the
manager's only persistence is files under the run directory, named fleet-run-<timestamp>/
with subdirs per vehicle.

## 13. Dashboard (Next.js)

One app, four views, same stack as the rustsitsim dashboard (shared component library
in a private package; one WS client class drives both). **Fleet map:** north-up canvas
with the geofence polygon, ceiling/floor annotation, vehicle icons colored by FSM
state with 2 s breadcrumb trails, task markers with award-assignment and completion
state, fault-event markers. **Vehicle cards:** one card per vehicle - state, battery,
staleness, link counters, current task, expandable into the single-vehicle telemetry
charts (identical components to rustsitsim's live view, driven by the fleet frame's
per-vehicle block). **Task board:** task table with assignment, progress, and auction
bid summary per allocation round (replayed from the event log). **Event log:** the
supervisor's event stream as a filterable, severity-colored table - the operator's
flight recorder. The dashboard issues estop in one click (POST /api/fleet/estop),
fault injections through forms proxied per vehicle, and task appends through the task
board; everything else is read-only by design, because an operator console that can
silently edit policy mid-run is a liability in regression testing.

## 14. Milestones and Acceptance Criteria

| Milestone | Scope | Exit criteria | Est. |
|---|---|---|---|
| M0 | Workspace, fleet-modes + codec + unit tests | V-11 resolved (constants table verified); golden vectors green | 3 d |
| M1 | Process supervision + links | Case F-1 passes: 2 vehicles to READY, clean teardown | 5 d |
| M2 | Offboard + safety core | One vehicle flies a 3-waypoint offboard route with geofence clamp; F-2's single-vehicle subset passes; V-10/V-12 resolved | 6 d |
| M3 | Allocation + fleet mission | Case F-2 passes in full; allocator gap test in CI; 4-vehicle sizing measurements recorded | 6 d |
| M4 | Failsafe ladder | Cases F-3, F-4, F-5 pass | 4 d |
| M5 | Formation + estop | Cases F-6, F-7 pass | 4 d |
| M6 | Dashboard + docs + release | All four views live against a running fleet; run report review UI; README, ADRs; v0.1 tag | 5 d |

Total: roughly 5 weeks sequential, deliberately overlapped in practice with rustsitsim's
M3-M6 (the fleet manager consumes the sim's stable surfaces: status API, fault API,
replay). Cross-repo sequencing constraint: mavfleet M1 requires rustsitsim M1 (a booted
PX4); mavfleet M2+ requires rustsitsim M2+ (flyable dynamics); the combined flagship
(F-2 full) requires rustsitsim M3.

## 15. Risk Register

| ID | Risk | L | I | Mitigation |
|---|---|---|---|---|
| R-10 | CPU saturation at 4+ simultaneous ACTIVE vehicles | M | H | Measure at M3 (sizing note 12.2); CI caps at measured comfort; stagger task starts by 2 s in profiles |
| R-11 | Offboard timing drift under load (setpoint stream gaps) | M | H | 20 Hz stream with 4x margin over PX4's 2 Hz requirement; stream health counter; F-2 asserts stream continuity |
| R-12 | Mode constant drift vs PX4 header (V-11) | L | H | Constants table unit-tested against the pinned repo file; same pin discipline as rustsitsim R-1 |
| R-13 | Auction claims overstated (gap test fails) | L | M | The 6.4 criterion is the test; if it fails, ship with the measured number and greedy framing |
| R-14 | Port collision with host services | L | M | Manager owns allocation by construction; F-1 port probe; ports documented in one table |
| R-15 | PX4 internal failsafes racing the supervisor | M | M | Supervisor verifies mode changes rather than assuming; escalation ladder with 10 s land retry (8.1) |
| R-16 | Scenario DSL scope creep | M | M | 9.1 key set frozen for v0.1; additions via ADR |
| R-17 | Cross-repo API drift (sim status/fault planes) | M | H | Shared JSON schema package; both repos consume it; version-pinned git dependency |

## 16. Repository Layout and Cross-Repo Integration

```text
mavfleet/
  Cargo.toml                # workspace
  crates/
    fleet-core/             # registry, tick, health, event log (Sections 4, 5)
    fleet-mavlink/          # link tasks, GCS-side codec (Section 3)
    fleet-modes/            # PX4 mode constants (3.2, V-11)
    fleet-alloc/            # auction + Hungarian (Section 6)
    fleet-safety/           # geofence + policy engine (Section 8)
    fleet-mission/          # DSL, compiler, orchestrator (Section 9)
    fleet-simctl/           # process supervision (2.4, Section 12)
    fleet-cli/              # binary + REST/WS plane (3.4)
  dashboard/                # Next.js fleet app (Section 13)
  tests/                    # fleet integration cases F-1..F-7
  docs/
    SPEC.md                 # this document
    SCENARIOS.md            # DSL reference with examples
    adr/
  schemas/                  # shared JSON schemas (fleet frame, run report)
  .github/workflows/
  px4-version               # identical content to rustsitsim's pin
```

Dependency on rustsitsim: git dependency in Cargo.toml pinned to a tag (sitsim-sdk for
scenario generation and process control, sitsim-mavlink for the codec), plus runtime
dependency on the sitsim-cli binary located via FLEET_SITSIM_BIN or PATH. The two repos
pin the same PX4 version file, and CI in both refuses to run the integration suite when
the pins disagree - one fleet, one protocol baseline, enforced mechanically. License:
Apache-2.0, matching rustsitsim. Publication order: rustsitsim v0.1 first (it is
independently useful), mavfleet v0.1 immediately after, cross-referenced in both READMEs.

## 17. Appendix A: Per-Instance Port Map (Complete)

Consolidated allocation for vehicle i (0-based), from the verified rcS conventions and
the manager's own planes:

| Plane | Port / value | Owner |
|---|---|---|
| HIL TCP | 4560 + i | rustsitsim listens; px4 connects |
| Telemetry bind (manager link) | 14540 + i | mavfleet binds; px4 streams |
| Onboard MAVLink listen | 14580 + i | px4; manager sends commands here |
| GCS link | 18570 + i | px4 rcS; reserved, unused by the manager |
| Payload link | 14280 + i / 14030 + i | px4 rcS; reserved for future payload work |
| Gimbal link | 13030 + i / 13280 + i | px4 rcS; reserved |
| Sim control API | 8200 + i | rustsitsim axum; mavfleet proxies fault injection |
| MAV_SYS_ID | i + 1 | px4 |
| uXRCE-DDS key | i + 1 (agent 127.0.0.1:8888) | px4; one shared agent process optional |
| Fleet API / WS | 8400 (single) | mavfleet |
| Manager sysid / compid | 255 / 190 | mavfleet on every vehicle link |

## 18. Appendix B: Canonical Fleet Scenario

The F-2/F-3 driver scenario (2 vehicles, 4 tasks, link loss, success criteria):

```toml
[fleet]
count = 2
battery_sim = true
restart_on_fault = false

[env]
geofence = { points_ned_m = [[-100,-100],[100,-100],[100,100],[-100,100]], ceiling_m = 60, floor_m = 0 }
wind_steady_ms = [1.0, 0.0, 0.0]
turbulence = "light"

[[tasks]]
id = "wp_n"
pos_ned_m = [-60.0, 0.0, -12.0]
hover_s = 5

[[tasks]]
id = "wp_e"
pos_ned_m = [0.0, 60.0, -12.0]
hover_s = 5

[[tasks]]
id = "wp_s"
pos_ned_m = [60.0, 10.0, -12.0]
hover_s = 5

[[tasks]]
id = "wp_w"
pos_ned_m = [-10.0, -60.0, -12.0]
hover_s = 5

[[vehicle_override]]
index = 1
battery = { discharge_scale = 1.6 }   # accelerates the ladder test path

[[event]]
kind = "link_loss"
vehicle = 1
start_s = 40
duration_s = 15

[[event]]
kind = "fault"
vehicle = 0
fault = { type = "gps_denial", start_ms = 30000, duration_ms = 20000 }

[success]
all_tasks_done = true
all_landed = true
max_time_s = 300
no_geofence_breach = true
fsm_trace = [
  { vehicle = 1, from = "ACTIVE", to = "RTL", cause = "heartbeat_loss" }
]
```

The scenario is checked in as docs/examples/fleet-basic.toml and executed by nightly CI
end to end. Like its rustsitsim counterpart, the example is the regression test: a
specification whose appendix runs in CI cannot quietly rot.