# PX4 SITL HIL Lockstep Protocol — Interface Control Document

Version 0.1 | Target: PX4-Autopilot **v1.16.2** | Source: live bring-up on a source build
(headless, multi-instance, verified end-to-end)

This is the standalone interface control document for the protocol PX4-Autopilot's
`simulator_mavlink` module speaks with an external simulator over TCP in lockstep mode.
It is extracted from the rustsitsim engineering specification (Section 3) so that
**any** simulator author — not just rustsitsim users — can boot and fly an unmodified
PX4 SITL binary without reading PX4 source code.

Facts are stated declaratively, each with an origin tag: **[SRC]** a PX4 source citation,
**[OBS]** direct empirical observation against a live build, or **[V-n]** a verification
item to re-confirm on your own build. Any implementation that satisfies every MUST in
this document will boot and fly PX4; this is the contract, not a suggestion.

## 1. Transport and connection semantics

The simulator is the **TCP server**; PX4 is the **TCP client**. This is the single most
surprising fact in the protocol and getting it backwards wastes a day. PX4's
`simulator_mavlink` module, started by
`ROMFS/px4fmu_common/init.d-posix/px4-rc.mavlinksim` as
`simulator_mavlink start -c $((4560+px4_instance))`, actively connects out to the
simulator [SRC: px4-rc.mavlinksim; OBS]. Therefore the simulator MUST listen on
`127.0.0.1:4560 + i` where `i` is the PX4 instance index, and SHOULD accept exactly one
connection per run. PX4 redirects its target through the `PX4_SIM_HOSTNAME` /
`PX4_SIM_HOST_ADDR` environment variables [SRC].

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

## 2. Framing: MAVLink v2 is mandatory

The HIL link MUST speak **MAVLink v2** (0xFD magic) frames. The reason is narrow and
binding: `HIL_SENSOR.id` (the uint8 sensor ID) is a **MAVLink2 extension field** in
common.xml [SRC], and PX4's boot gate requires `imu.id == 0` [SRC:
SimulatorMavlink.cpp:484, handle_message_hil_sensor]. MAVLink v1 frames cannot carry
extension fields at all, so a v1 speaker will stream HIL_SENSOR forever while PX4 sits
blocked in `simulator_mavlink start` waiting for `_has_initialized` [SRC:
SimulatorMavlink.cpp:1638, under ENABLE_LOCKSTEP_SCHEDULER]. The failure mode is silent
on the simulator side and fatal on the PX4 side: rcS never proceeds, no modules start,
and no telemetry ever appears on 14540+i [OBS].

The codec MUST therefore always emit v2 framing on the HIL link: STX 0xFD, LEN,
INCOMPAT FLAGS (0), COMPAT FLAGS (0), SEQ, SYSID, COMPID, MSGID (24-bit
little-endian), PAYLOAD, CRC16-X.25 seeded with the message's CRC_EXTRA,
little-endian. The simulator uses SYSID 2, COMPID 1 on the HIL link (matching the
pymavlink prototype), and MUST accept any source sysid on receive, since PX4 uses
sysid = i + 1. Signature and truncation incompat flags are never set.

## 3. Boot sequence and initialization gate

Headless PX4 with no external simulator is launched per the official multi-instance
pattern [OBS, matching Tools/simulation/sitl_multiple_run.sh]:

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
consequence for the simulator: it MUST begin streaming HIL_SENSOR at full rate
immediately after TCP accept, using its own default rate, without waiting for any
request from PX4 [OBS: the rate request arrives after the stream starts].

Verified boot evidence on this path: "Startup script returned successfully" on the PX4
console, ESTIMATOR_STATUS and ATTITUDE at 50 Hz on the telemetry link, home position
set, heartbeats with the expected sysid, and ULog files written into the instance
directory [OBS].

## 4. Rate negotiation (COMMAND_LONG 511)

Immediately after TCP connect, PX4 sends COMMAND_LONG (76) frames with command = 511
(MAV_CMD_SET_MESSAGE_INTERVAL). Observed instance: param1 = 115
(HIL_STATE_QUATERNION), param2 = 5000 (microseconds, i.e. 200 Hz) [OBS]. A
well-behaved simulator MUST answer each with COMMAND_ACK (77) carrying command = 511
and result = 0 (ACCEPTED) [OBS; silent ignoring also works today but is
non-conforming].

Treat the requested interval as advisory but honored: set your tick rate from your own
configuration, and if a 511 request asks for a different interval for message 115,
adapt the HIL_STATE_QUATERNION cadence to the request (within sane bounds, e.g.
50-400 Hz) and log the change. Answer all other COMMAND_LONG commands with
COMMAND_ACK result = 3 (UNSUPPORTED) so PX4 does not retry.

## 5. Message set summary

Four messages carry the entire loop. IDs verified against the MAVLink v2 common
dialect:

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
is not required for control via MAVLink commands and offboard setpoints.

## 6. HIL_SENSOR field contract

Units and semantics per common.xml; consumption per SimulatorMavlink.cpp [SRC; OBS]:

| Field | Type | Unit | Sim source | Notes |
|---|---|---|---|---|
| time_usec | uint64 | us | virtual clock | Drives PX4 lockstep time: CLOCK_MONOTONIC is set from this value; MUST be monotonic non-decreasing |
| xacc, yacc, zacc | float32 | m/s² | IMU model, body frame | Include gravity per MAVLink convention; specific force, NED-body axes |
| xgyro, ygyro, zgyro | float32 | rad/s | gyro model, body frame | True rates plus noise and bias |
| xmag, ymag, zmag | float32 | gauss | magnetometer model, body frame | Field in gauss, not tesla, not milligauss |
| abs_pressure | float32 | hPa | baro model | Sea-level-referenced absolute pressure |
| diff_pressure | float32 | hPa | pitot model (0 acceptable) | Reserved for airspeed; zero is accepted by PX4 |
| pressure_alt | float32 | m | baro altitude | Pressure-derived altitude |
| temperature | float32 | degC | ambient | Surface temperature |
| fields_updated | uint16 | bitmask | sensor schedule | Bit per sensor group; 0b011111111 observed working; bits set when the corresponding fields carry fresh data this frame |
| id | uint8 (v2 ext) | - | constant 0 | THE boot gate: PX4 requires imu.id == 0; cannot be transmitted in v1 framing |

**V-1:** Confirm the exact fields_updated bit assignment consumed by v1.16.2
(SimulatorMavlink publishing into sensor_combined) when per-sensor decimation is
introduced; until then set all bits every frame.

## 7. HIL_STATE_QUATERNION field contract

This message carries ground truth to PX4's ekf2 and attitude modules. Three unit traps
are documented in PX4's own handling code and verified live [SRC; OBS]: acceleration
fields are **int16 millig** (mG, 1 mG = 0.00980665 m/s²), velocities are **int16
cm/s**, and airspeeds are **uint16 cm/s**. A simulator that emits m/s² in these fields
saturates or wraps and destroys the estimate within seconds.

| Field | Type | Unit | Sim source | Notes |
|---|---|---|---|---|
| time_usec | uint64 | us | virtual clock | Same clock as HIL_SENSOR |
| attitude_quaternion[4] | float32 | - | attitude, w, x, y, z order | Body-to-NED rotation; normalized |
| rollspeed, pitchspeed, yawspeed | float32 | rad/s | body rates | Truth rates (no noise) |
| lat, lon | int32 | degE7 | geodetic position | 1e7-scaled degrees |
| alt | int32 | mm | MSL altitude | 1000-scaled meters |
| vx, vy, vz | int16 | cm/s | NED velocity | SIGNED; watch the sign of vz (down positive) |
| ind_airspeed, true_airspeed | uint16 | cm/s | airspeed model (0 acceptable) | Zero accepted |
| xacc, yacc, zacc | int16 | mG | specific force, body frame | Gravity-consistent with HIL_SENSOR xacc; quantization step 0.0098 m/s² |

**V-2:** The order of quaternion components (w-first) as consumed by v1.16.2's
handle_message_hil_state_quaternion is w, x, y, z; re-confirm with one golden vector
on your build.

## 8. HIL_GPS field contract

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

The eph/epv uint16 range is a real constraint: values above 65535 cm clamp; a GPS
denial transition should ramp eph upward toward the clamp rather than teleport it, so
PX4's innovation tests degrade gracefully instead of gating hard.

## 9. HIL_ACTUATOR_CONTROLS contract

Arriving frame (PX4 to sim): time_usec (uint64), controls[16] (float32), mode (uint8,
MAV_MODE_FLAG bits), flags (uint64). **[V-3 RESOLVED, live-captured against PX4
v1.16.2 — ADR 0011r]** PX4 v1.16 sends **per-motor NORMALIZED thrust [0, 1]** in
`controls[]`: `SimulatorMavlink` subscribes `actuator_outputs_sim` (NOT the PWM
topic), which `PWMSim::updateOutputs` publishes as
`(pwm − 1000) / 1000` for non-reversible Motor outputs — armed idle ≈ 0.002,
offboard climb ramps to ~1.0 — and 0 for disarmed channels (magic 900 skipped).
The **armed bit is mode & 0x80** (0x81 = armed+lockstep, 0x0001 = disarmed);
disarmed frames are all-zero. Observed live: motors occupy controls[0..3]
(CA_ROTOR0..3), the other 12 channels stay 0. The correct mapping is
`u = clamp(c, 0, 1)`; keep a PWM fallback (`c >= 900 → u = (c − 1000)/1000`)
for stacks that send raw PWM. The jMAVSim-era [-1, +1] convention (`u = (c+1)/2`)
must NOT be applied to v1.16 frames: it turns armed idle 0.002 into a phantom
u = 0.5 (2/3 of hover thrust on "stopped" motors) — the exact defect that
diverged the I-2 takeoff (ADR 0011r). Disarm (armed bit clear) means the rotors
STOP (wind down), not idle-spin.

Related v1.16 bring-up facts found the hard way (see docs/adr/ in the rustsitsim
repo):
- **The 1 Hz GCS heartbeat matters for arming**: without it PX4 flags the
  datalink lost and denies arming (the fleet driver and any GCS emulation
  must pump MAVLink heartbeats).
- **DO_SET_MODE (COMMAND_LONG 176)** is decoded by PX4 as separate params —
  `custom_main_mode = (uint8_t)param2; custom_sub_mode = (uint8_t)param3`
  (Commander.cpp:787-790) — NOT the packed 32-bit custom-mode word used in
  HEARTBEAT. Sending the packed word truncates to 0 and silently no-ops (still
  ACKed).
- **The magnetometer must not be perfectly constant.** A zero-noise mag
  (identical value every sample) is never fused by EKF2 — yaw alignment and
  GNSS position fusion stay blocked and the vehicle can never fly. ~0.005
  gauss of noise restores normal fusion (ADR-0012).
- While ARMED, mode changes are gated by `canRun(target)`: OFFBOARD entry
  requires the setpoint stream fresh (within COM_OF_LOSS_T = 1 s) and a valid
  local position estimate; early attempts convert silently to a LOITER
  fallback.

## 10. Lockstep virtual time and timing contract

Virtual time is owned by the simulator: PX4's CLOCK_MONOTONIC under the lockstep
scheduler is advanced by the HIL_SENSOR time_usec stream [SRC; OBS]. The contract the
simulator MUST uphold:

1. time_usec MUST advance by exactly 1e6/rate between consecutive HIL_SENSOR frames
   (5000 us at 200 Hz). Jitter or regression in the timestamp sequence violates the
   scheduler's assumptions [SRC].
2. Every sensor emission MUST carry a timestamp consistent with its sample time; the
   GPS stream, sent at 5 Hz, carries the timestamp of its own sample slot, not the
   current tick [design decision, keeps downstream fusion honest].
3. The simulator SHOULD complete tick computation in well under the real-time budget;
   lockstep does not require real-time execution, but wildly exceeding it makes
   interactive scenarios unusable.
4. TCP disconnect at any point is legal; treat it as end-of-run.

## 11. Minimal bring-up checklist

1. Build PX4 SITL from source once (`make px4_sitl_default`); binary at
   `build/px4_sitl_default/bin/px4`.
2. Listen on TCP 4560; on accept, immediately start emitting 200 Hz MAVLink v2
   HIL_SENSOR with `id = 0`, `fields_updated = 0b011111111`, `time_usec` advancing by
   5000 per frame.
3. Emit HIL_STATE_QUATERNION (115) at 200 Hz with the unit conversions above; emit
   HIL_GPS (113) at 5 Hz with fix_type = 3.
4. Answer COMMAND_LONG 511 with COMMAND_ACK(511, result 0); answer everything else
   with result 3.
5. Launch PX4 per Section 3 and watch for "Startup script returned successfully";
   bind UDP 14540 to observe telemetry (HEARTBEAT, ESTIMATOR_STATUS, ATTITUDE,
   LOCAL_POSITION_NED).
6. Receiving HIL_ACTUATOR_CONTROLS means the loop is closed: PX4 is flying on your
   dynamics.

Report deviations per build as new V-items; that is how this document stays honest.
