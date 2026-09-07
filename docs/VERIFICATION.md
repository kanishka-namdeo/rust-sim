# Verification — live evidence against real PX4

Everything below was executed against **unmodified PX4-Autopilot v1.16.2**
built from source (`make px4_sitl_default`) inside this repository's
harnesses. Each harness is single-invocation: start, assert, teardown in one
shell call, with CI-classifiable exit codes.

| Case | Harness | What it proves | Status |
|------|---------|----------------|--------|
| **I-1** | `sim/tests/run_i1.sh` | PX4 boot gate over the HIL wire: rcS completes, EKF2 estimator streaming (ESTIMATOR_STATUS / ATTITUDE / LOCAL_POSITION_NED on UDP 14540), heartbeats with the right sysid, HIL loop closed, ULog written | **PASS** |
| **I-2** | `sim/tests/run_i2_flight.sh` | **Physical flight, direct wire**: arm -> OFFBOARD climb -> hover -> descend -> land -> disarm, with ground truth from the sim replay (not the EKF); all ticks finite; sim exits clean on PX4 disconnect | **PASS** (z_min -1.85 m, land + disarm observed, 15,476 ticks finite) |
| **F-1** | `fleet/tests/run_f1.sh` | Fleet bring-up: 2 vehicles READY with live health (sysid, decoded modes, heartbeat ages, link counters), operator e-stop -> ABORTED(2), run report + event log written, teardown leaves all ports free | **PASS** |
| **F-2** | `fleet/tests/run_f2.sh` | **Fleet flies with real dynamics**: 2 vehicles x full rustsitsim instances (direct HIL wire, no proxy), auction, offboard missions, hover observed at the waypoint, RTL, land, disarm — asserted from replay ground truth (z <= -2 m flight, final |z| < 0.5 m, all finite) | **PASS** (both vehicles flew + landed; run 92.9 s; manager exit 0) |
| **Browser-live** | `scripts/browser_live_test.sh` | The operator console, end-to-end through the preview gateway: Sim Console LIVE (WS :8200, 10 Hz telemetry), telemetry demonstrably moving (two snapshots differ), Fleet C2 LIVE (vehicles in OFFBOARD mid-mission), screenshots captured | **PASS** |

## Unit tests

- `sim/`: 99+ tests — codec golden vectors (byte-exact vs PX4's generated
  headers), dynamics validation (hover equilibrium, contact stability
  sizing), sensor models (latency FIFO, denial ramp, glitch), engine mapping
  regressions incl. the PX4 v1.16.2 actuator wire layout + armed-frame
  decode.
- `fleet/`: 126+ tests — FSM transitions, policy ladder ordering property,
  allocator optimality, runner profile math, router integration tests
  (incl. WS upgrades on the gateway's `/?XTransformPort=` shape), wire-goal
  repro pinning NED setpoints on the wire.

## Hard-won protocol facts (all live-captured, all ADR'd)

1. PX4 v1.16.2 packs `HIL_ACTUATOR_CONTROLS` with size-sorted core fields
   (`flags@8, controls@16, mode@80`), diverging from official common.xml
   (`sim/docs/adr/0015`).
2. PX4 v1.16 sends per-motor normalized thrust [0,1] — not the jMAVSim-era
   [-1,1] (`sim/docs/adr/0011`).
3. A noiseless magnetometer (or IMU) starves EKF2's fusion pipeline and
   blocks arming — sensor noise is a correctness feature, not cosmetic
   (`sim/docs/adr/0012`, `0014`).
4. `DO_SET_MODE` params decompose as param2=main/param3=sub; the packed mode
   word silently no-ops (`fleet/docs/adr/0010`).
5. Arming requires a 1 Hz GCS heartbeat pump (datalink-loss gate).
6. Co-located grounded spawns must not trigger the separation
   AltitudeDiverge override — grounded vehicles cannot comply, and the
   hijacked goal stream pins the fleet on the ground (`fleet/docs/adr/0011`).

Re-run any of this yourself: see [OPERATIONS.md](OPERATIONS.md).
