# ADR 0014: realistic sensor noise is required for estimator determinism (zero-noise starvation, class extension of ADR-0012)

**Status**: Accepted
**Context**: I-2 flight bring-up showed two more instances of the
"perfect sensors starve PX4's estimators" failure class that ADR-0012
identified for the magnetometer:

1. **Arming nondeterminism with a noiseless IMU.** With
   `accel_noise_density = 0`, PX4's EKF2 accel-bias estimate is
   unobservable in the noise-free limit: nothing pins it, so it
   random-walks on its own process noise and intermittently trips
   "Preflight Fail: High Accelerometer Bias" (COM_ARM_EKF_AB gate). The
   same scenario armed in one boot and was denied through 8 retries in
   the next — pure EKF2-internal coin flips.
2. **Simulated baro value staleness.** HIL_SENSOR carries the baro
   fields every frame (FIELDS_UPDATED_ALL), but the engine refreshed the
   baro VALUE only every 4th tick (50 Hz, SPEC §6.4). PX4 observed fresh
   timestamps with held values and its sensors module flagged
   "BARO #0/#1 failed: STALE!"; EKF2 altitude then drifted ~1 m from
   truth (EKF z −2.51 vs truth z −1.44 in the first I-2 flight).

**Decision**:

- The flight scenario (and any scenario expected to ARM) uses the
  config-default BMI088-class IMU noise
  (`gyro_noise_density = 0.00035`, `accel_noise_density = 0.0025`
  (rad/s)/√Hz resp. (m/s²)/√Hz). Ideal/noiseless sensors remain
  available for boot tests (I-1) and physics debugging.
- The engine samples the baro at the FULL tick rate (200 Hz): the value
  stays consistent with its timestamp; PX4's own air-data path still
  decimates to 50 Hz internally, so the effective fusion cadence is
  unchanged.
- The mag keeps its 0.005 G noise (ADR-0012) and 50 Hz cadence.

**Verification** (live, real PX4 v1.16.2, `tests/run_i2_flight.sh`):

- Arming became deterministic (armed on the first post-settle attempt).
- EKF altitude tracks sim ground truth to ~3 cm during the climb
  (EKF z_min −3.07 vs truth −3.10); before the fix the error was ~1 m.
- Full I-2 PASS: arm → OFFBOARD (0 re-engages) → physical climb to
  z = −3.10 m → descend → land → disarm; 16,706 replay ticks all
  finite; sim exit 3 (teardown), never 5; ULog written.
- I-1 boot regression still PASS (telemetry hash re-baselined:
  2734384e5f162a4b); 98/98 unit tests green.

**Note**: the "BARO #0/#1 failed: STALE!" STATUSTEXT can still appear
transiently at boot before the first baro frames arrive; with the
full-rate refresh it no longer affects the flight or the estimate.
