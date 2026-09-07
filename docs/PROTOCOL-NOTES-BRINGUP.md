# rustsitsim — PX4 SITL Lockstep HIL Protocol Notes (empirically verified)

Established 2026-09-07 in this sandbox against PX4-Autopilot v1.16.2 (source build).
These notes are the seed ICD for the `rustsitsim` Rust simulator.

## 1. Transport
- PX4 `simulator_mavlink` module connects as **TCP client** to `127.0.0.1:4560 + instance`
  (launched by `ROMFS/px4fmu_common/init.d-posix/px4-rc.mavlinksim`:
  `simulator_mavlink start -c $((4560+px4_instance))`)
- The simulator (us) is the **TCP listener/server**.
- PX4 env vars: `PX4_SIM_HOSTNAME` / `PX4_SIM_HOST_ADDR` can redirect the target.

## 2. Framing — MAVLink v2 REQUIRED
- `HIL_SENSOR.id` (uint8, sensor id) is a **MAVLink2 extension field** in
  `common.xml`. PX4's boot gate is `imu.id == 0`
  (`SimulatorMavlink.cpp:484 handle_message_hil_sensor`).
- MAVLink v1 frames cannot carry `id` -> boot hangs forever at
  `simulator_mavlink start` (which blocks until `_has_initialized`, see
  `SimulatorMavlink.cpp:1638` under `ENABLE_LOCKSTEP_SCHEDULER`).
- => Rust sim must speak MAVLink v2 on this link, with HIL_SENSOR.id = 0.

## 3. Rate negotiation
- Right after TCP connect, PX4 sends `COMMAND_LONG` cmd=511
  (MAV_CMD_SET_MESSAGE_INTERVAL), e.g. param1=115 (HIL_STATE_QUATERNION),
  param2=5000 (us) => 200 Hz.
- A well-behaved sim answers with `COMMAND_ACK(511, 0)`.

## 4. Message set (minimum to boot + fly)
| Message | Rate | Notes |
|---|---|---|
| HIL_SENSOR (107) | 200 Hz | time_usec drives **lockstep virtual time** (CLOCK_MONOTONIC set from it); id=0 gates init |
| HIL_STATE_QUATERNION (115) | 200 Hz | acc fields are **int16 mG**! vx/vy/vz int16 cm/s; PX4 requests this one explicitly |
| HIL_GPS (109) | 5 Hz | fix_type=3, lat/lon degE7, alt mm, eph/epv cm (uint16 max 65535) |
| HIL_ACTUATOR_CONTROLS (92) | <- from PX4 | motor outputs; confirms the control loop closed |

## 5. Boot mechanics (headless, per official sitl_multiple_run.sh)
```
cd build/px4_sitl_default/instance_N
PX4_SIM_MODEL=gazebo-classic_iris ../bin/px4 -i N -d <ABS>/build/px4_sitl_default/etc
```
- `-d` = data dir (etc/ with rcS); NOT the rcS file itself.
- Model `gazebo-classic_iris` -> SYS_AUTOSTART=10015, PX4_SIMULATOR unset ->
  px4-rc.mavlinksim branch (our TCP path).
- `simulator_mavlink start` BLOCKS the rest of rcS until first valid
  HIL_SENSOR(id=0) arrives (lockstep scheduler).

## 6. Per-instance port map (rcS: px4-rc.mavlink + px4-rc.mavlinksim)
| Link | Port formula (i = instance) |
|---|---|
| Sim TCP | 4560 + i |
| MAVLink onboard listen / remote | 14580+i / 14540+i (pymavlink: bind 0.0.0.0:14540+i) |
| GCS | 18570+i |
| Payload (onboard) | 14280+i / 14030+i |
| Gimbal | 13030+i / 13280+i |
| MAV_SYS_ID | i+1 |
| uXRCE-DDS key | i+1 (agent 127.0.0.1:8888) |

## 7. Sandbox build recipe (no sudo, no system cmake)
- rustup -> Rust 1.98 (PATH via ~/.cargo/bin)
- portable cmake 3.30.5 (GitHub release tarball) + `pip install ninja`
- pip (python3.13!): empy==3.3.4 kconfiglib jinja2 pyserial pyyaml jsonschema
  pyros-genmsg pyulog pymavlink
- shim `python3` -> python3.13 (deps installed there; default python3 is a
  3.12 venv without them)
- `git submodule update --init --recursive --depth 1 --force` to completion
  (incomplete submodule init = "Cannot find source file" cmake errors)
- NuttX tags needed by version header: `git -C platforms/nuttx/NuttX/nuttx
  fetch --tags --depth 1`
- `yes | make px4_sitl_default` (auto-answers submodule version prompts)
- Build: ~10 min on 2 cores, incremental via ninja, artifact:
  build/px4_sitl_default/bin/px4

## 8. Verified end-to-end results
- Single instance: full boot ("Startup script returned successfully"),
  EKF2 streaming (ESTIMATOR_STATUS, ATTITUDE @50Hz), home position set,
  ULog files written to instance_N/log/.
- 2 instances: sysid 1/2, independent TCP 4560/4561 HIL links, heartbeats +
  telemetry on 14540/14541. ~15 MB RSS per vehicle.
- pymavlink connect: `mavutil.mavlink_connection('0.0.0.0:14540+i')` (bind);
  PX4 also streams to the bound socket.

## Sensor-noise realism (ADR-0012 / ADR-0014 — the zero-noise starvation class)

Three instances of the same lesson: PX4's estimators need *realistic*
noise, not perfect data:
- **mag**: noiseless mag is never fused → no yaw alignment → arming
  denied (0.005 G noise fixes it);
- **accel**: noiseless accel leaves the EKF2 bias estimate degenerate →
  "Preflight Fail: High Accelerometer Bias" arming denials that flip
  randomly between boots (0.0025 m/s²/√Hz noise fixes it);
- **baro values**: don't hold values constant across frames with fresh
  timestamps — refresh the value every HIL_SENSOR (PX4's air data still
  decimates to 50 Hz internally, but its health check watches the value
  stream).

## I-2 physical flight (post ADR-0011r/0013/0014) — PASSING

`tests/run_i2_flight.sh`: real PX4 v1.16.2 arms (first attempt),
OFFBOARD engages with zero re-engages, ground truth climbs to
z = −3.10 m with EKF tracking to 3 cm, descends, lands, disarms;
16,706 ticks finite; telemetry hash recorded per run in
`tests/i2_artifacts/sim_stdout.txt`.
