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
| **S-1** | `fleet/scripts/live_test_setup.sh` | The ADR-0016 vehicle-setup plane against real PX4, REST-level: full parameter download (887 params, PARAM_REQUEST_LIST), typed param write with echo confirm, gyro calibration (MAV_CMD 241 ACCEPTED), flight-mode switch (ALTCTL + heartbeat echo), airframe apply Iris -> Boat (USV 1070) -> Iris with controlled pair restarts, parameter persistence across both reboots, 422/404 error paths, graceful estop teardown with no leaked processes | **PASS** (44/44 checks) |
| **S-2** | `scripts/browser_setup_test.sh` | The QGC-style Vehicle Setup tab driven end-to-end in a real browser through the gateway: tab LIVE + disarm badge, param download via the UI button, typed rows in the params table, airframe catalog (UAV/USV/UUV groups), calibration rows, mode switch via button, Boat apply via filter + confirm dialog, post-restart resolution to Boat, screenshots | **PASS** (13/13 checks) |
| **O-1** | `fleet/tests/live_test_operator.sh` | The ADR-0017 operator map control plane against real PX4, REST-level: fleet frame carries geo_origin + geofence + per-vehicle GLOBAL_POSITION_INT fixes (degE7), go-to flight (engage ladder -> OFFBOARD -> 18 m transit, armed + lat/lon moving), hold (AUTO.LOITER) + land (disarm observed), mission upload with fence-validated accept/reject (3 + 1 at 12 km breach), mission start -> auction-flown op* tasks, e-stop teardown with clean exit | **PASS** (15 checks) |
| **O-2** | `scripts/browser_map_test.sh` | The Operator Map tab driven end-to-end in a real browser through the gateway: Leaflet map LIVE with the real fence fitted + vehicle markers from GPS fixes, 3 waypoints placed by real map clicks (coordinate mouse events), waypoint table + geo-sanity guard, Upload -> op* tasks on the live board, Start mission -> phase RUNNING + vehicle armed/ACTIVE in flight on the map, screenshots | **PASS** (16/16 checks) |
| **R-1** | `fleet/tests/live_test_runtime.sh` | The ADR-0018 runtime control plane against real PX4, REST-level: fault proxy (gps_denial accepted with the sim's runtime-N id + the sim's own :8200 plane listing it, bogus type relayed as the sim's 4xx, index 404), runtime NED task append (2 accepted / far one geofence-rejected with the compiler's reason / explicit id collision rejected, board visible), hot scenario load (invalid TOML 422, staged swap -> graceful ABORTED -> same-port rebind with new count + 80 m fence + hot-scenario path on the frame, isolated -hot-1 run dir, both reports naming their own scenarios), the staged scenario's timeline fault event ACTUALLY injected through the sim fault plane, mission-active 409 on a mid-flight PUT, e-stop exit 2 + port-free teardown | **PASS** (23 checks) |

## Unit tests

- `sim/`: 99 tests — codec golden vectors (byte-exact vs PX4's generated
  headers), dynamics validation (hover equilibrium, contact stability
  sizing), sensor models (latency FIFO, denial ramp, glitch), engine mapping
  regressions incl. the PX4 v1.16.2 actuator wire layout + armed-frame
  decode. (Re-run 2026-09-08: 99/99 PASS.)
- `fleet/`: 169 tests — FSM transitions, policy ladder ordering property,
  allocator optimality, runner profile math, router integration tests
  (incl. WS upgrades on the gateway's `/?XTransformPort=` shape), wire-goal
  repro pinning NED setpoints on the wire, the vehicle-setup plane (param
  store ingest/staleness, typed INT32 bit-cast round-trips, airframe
  resolution against the ROMFS catalog, setup-endpoint envelopes, error
  gates), the hold-for-setup scenario key, and the operator control plane
  (geodesy round-trips incl. the AGL waypoint convention, geo blocks on
  the fleet frame, mission upload queue/ack/validation gates, guided
  command gates incl. the mission-active 409), plus the runtime control
  plane (ADR-0018: task-append queue/ack + validation, hot-load staging
  + nack-clears-staging, fault-proxy bounds/body gates, the simproxy HTTP
  framing round-trip + closed-port path). (Re-run 2026-09-08:
  169/169 PASS.)

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
7. `PARAM_VALUE`/`PARAM_SET` wire order is generated-header order
   (value f32 @0 ...), not XML declaration order — the v0.1 decoder read
   XML order and every echo decoded to a garbage id (`fleet/docs/adr/0016`).
8. PX4 v1.16's param protocol is **typed**: INT32 params travel as their
   bit pattern inside the f32 field and `param_type` carries the v2
   dialect's wire constants (REAL32 = **9**, INT32 = 6); the receiver
   rejects type mismatches, so QGC-style writes must be typed
   (`fleet/docs/adr/0016`).
9. PX4's param autosave is deferred ~300 ms (`autosave.cpp`
   ScheduleDelayed) — an immediate post-write reboot races the flash and
   boots the old airframe; the apply-restart path waits it out
   (`fleet/docs/adr/0016`).
10. PX4 v1.16 battery params carry the `BAT1_` instance prefix
    (`BAT1_N_CELLS`, `BAT1_V_EMPTY`, ...); the unprefixed `BAT_*` ids of
    earlier versions do not exist.
11. A hyper server aborts its response when the client half-closes its
    write side mid-request — the fleet's fault proxy relies on
    `Connection: close` instead (`fleet/docs/adr/0018`).
12. A `gps_denial` in effect before the mission blocks PX4's arming gate
    (TEMPORARILY_REJECTED, 10+ s after the denial ends) while OFFBOARD
    mode changes are still accepted — a vehicle can sit in OFFBOARD
    disarmed while its runner's tasks time out via deadline. Keep fault
    windows clear of the arming phase (`fleet/docs/adr/0018`).

Re-run any of this yourself: see [OPERATIONS.md](OPERATIONS.md).

## GCS v1 verification ladder (G-0 through G-13)

The G-ladder was added in the GCS v1 spec (`docs/GCS_SPEC.md` §9) and
extends the existing I/F/S/O/R ladder with 14 new single-invocation
harnesses under `console/tests/`. All 14 gates are green as of the
latest run (2026-09-09).

| Gate | Name | Harness | What it asserts | Status |
|------|------|---------|----------------|--------|
| **G-0** | PX4 version check | `console/tests/run_g0_version.sh` | `:8300` rejects upload to a vehicle reporting PX4 ≠ v1.16.2 with HTTP 426 + `PX4_VERSION_MISMATCH`; accepts v1.16.2 | **PASS** |
| **G-1** | Mission validation | `console/tests/run_g1_validation.sh` | Schema + geofence geometry + rally containment; rejects empty/out-of-fence missions; vertex-drag stress test (50+ vertices, no UI freeze) | **PASS** |
| **G-2** | Mission persistence | `console/tests/run_g2_persistence.sh` | CRUD round-trip (create, list, fetch, update, delete); survives catalog restart; version history retained | **PASS** |
| **G-3** | Mission upload (3 types) | `console/tests/run_g3_upload.sh` | MAVLink mission protocol: count → items → ack for all three `MAV_MISSION_TYPE` values; all items ack'd by mock PX4; rollback on failure | **PASS** |
| **G-4** | Mission download (3 types) | `console/tests/run_g4_download.sh` | Download from PX4 after upload; round-trip equality (seq, command, x, y, z) for all three types | **PASS** |
| **G-5** | Fly View 1-vehicle telemetry | `console/tests/run_g5_flyview.sh` | 10 Hz telemetry moves on map + strip + attitude HUD for 10 s; armed=false; health array present | **PASS** |
| **G-6** | Multi-vehicle Fly View | `console/tests/run_g6_multivehicle.sh` | 2 vehicles on map simultaneously, distinct sysid (1+2), distinct positions (100 m east offset verified) | **PASS** |
| **G-7** | Pre-arm + arm/disarm | `console/tests/run_g7_arm.sh` | Pre-arm checks run (5 QGC-style: EKF2/GPS/Mode/Fence/Battery), block arm when failing, pass when fixed, arm→disarm round-trip | **PASS** |
| **G-8** | Vehicle Setup extensions | `console/tests/run_g8_setup_ext.sh` | Param search filter (ROLLRATE → 3), group filter (MPC → 6), diff-against-defaults (is_changed), preset save/load round-trip | **PASS** |
| **G-9** | Fleet mission binding | `console/tests/run_g9_binding.sh` | Vehicle 0 → mission A, vehicle 1 → mission B; DELETE clears v0, v1 intact; 9 phases, 25 assertions | **PASS** |
| **G-10** | Fleet orchestration | `console/tests/run_g10_orchestration.sh` | Parallel mode: 2 vehicles attempted; sequential mode: v0 first, v1 after; both "started" (CRC fix verified) | **PASS** |
| **G-11** | ULog browse + plot | `console/tests/run_g11_ulog.sh` | List `.ulg` files, list topics, fetch topic data, replay list/meta/topics/data + 3 negative paths (404, 404, 400) | **PASS** |
| **G-12** | Replay scrub + overlay | `console/tests/run_g12_replay.sh` | Load `.replay`, scrub timeline (first half [0..50], second half [50..100]), monotonicity, q_wxyz identity quaternion | **PASS** |
| **G-13** | Survey/corridor/perimeter patterns | `console/tests/run_g13_patterns.sh` | Generate each pattern on a known polygon; 19 checks: waypoint count, all-inside-polygon, no gaps > leg spacing, mission editor integration | **PASS** |

### G-ladder notes

- The G-3/G-4/G-9/G-10 harnesses use a Python mock PX4
  (`mock_px4_fleet.py` / `mock_px4_mission.py`) that speaks the full
  MAVLink mission protocol — no real PX4 SITL build required for these
  gates. The G-5/G-6/G-7/G-8 harnesses use a mock that also sends 10 Hz
  telemetry + responds to arm/disarm + PARAM_REQUEST_LIST.
- The M5 known issue (mission_upload timeout against concurrent
  telemetry) was fixed in commit `51ef509`: the root cause was wrong
  MAVLink CRC extra values for MISSION_REQUEST_INT (51) and
  MISSION_REQUEST (40). Cross-checked against pymavlink 2.4.49.
- G-10 now shows both vehicles "started" (was "failed" before the CRC
  fix). The `try_recv` pre-check in the link task prevents command
  starvation by the 10 Hz telemetry flood.

### Updated unit test baselines

- `sim/`: 99 tests (unchanged).
- `fleet/`: **285 tests** (was 169 — +116 from M1-M7: 69 GCS catalog
  tests + 25 M4 param extension tests + 9 M5 fleet orchestration tests +
  21 M6 replay/ULog tests + 5 M3 prearm tests + 7 M2 mission upload
  tests).
- `console/`: `npm run lint` + `npm run build` clean; 14 G-ladder
  harnesses all PASS.

Re-run any of this yourself: see [OPERATIONS.md](OPERATIONS.md).
