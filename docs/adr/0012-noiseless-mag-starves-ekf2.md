# ADR 0012: A noiseless magnetometer starves PX4's mag pipeline

**Status**: Accepted (v0.1)
**Context**: With ideal (zero-noise) sensors — the I-1 "boot not confounded
by realism" configuration — PX4's EKF2 never fuses the magnetometer: `cs_mag`
stays 0, yaw never aligns, GNSS position/velocity fusion stays blocked, and
`xy_valid` never becomes true (EKF2 falls back to fake-position fusion). The
vehicle arms but can never fly, and the local-position estimate drifts. The
mag *data itself* is correct (units, frame, magnitude verified against the
sim's model: [0.195, 0.004, 0.455] gauss).

Empirical A/B: adding `noise_gauss = 0.005` to the mag model (everything else
identical) flips `cs_yaw_align`, `cs_mag`, `cs_gnss_pos`, `cs_gnss_vel` to 1
and `xy_valid` to 1 within ~15 s. The mag path upstream of EKF2 (driver /
calibration / selection) requires non-constant data; a perfectly constant
field is indistinguishable from a stuck sensor.

**Decision**:
- The I-1 scenario keeps ideal sensors (its assertions are boot-gate only).
- Flight-oriented scenarios and the mavfleet per-vehicle sim wrapper
  (`run_sitsim_vehicle.sh`) set `noise_gauss = 0.005` with a pointer here.
- The spec's default mag noise stays at its realistic model value.

**Consequences**: "perfect sensors" are not a valid bring-up simplification
for the mag channel; the wrapper documents the empirical reason inline. This
also explains why boot-time health messages (baro/compass) look healthy while
arming still fails — the failure is in fusion, not presence.
