# ADR 0013: RK4 ground-contact stability — sized substeps from the dominant contact root

**Status**: Accepted
**Context**: The unilateral spring-damper ground contact (SPEC §5.4,
k_g = 4000 N/m, c_g = 240 N·s/m per contact, 4 contacts at the arm ends)
is integrated by explicit RK4 inside the 200 Hz tick. Two stiff modes
result:

- **vertical**: the full mass on 4 parallel contacts —
  ω_n = √(4k_g/m) ≈ 103 rad/s, ζ ≈ 3.1, fast root ≈ **6.2e2 s⁻¹**;
- **rotational (roll/pitch)**: Jx/Jy = 0.02 kg·m² reacting to the same
  contacts at moment arms Σr_y² ≈ Σr_x² ≈ 0.101 m² —
  ω_n,rot ≈ 142 rad/s, ζ_rot ≈ 4.3, fast root ≈ **1.2e3 s⁻¹**.

The rotational root is ~2× stiffer than the vertical one because the
contact-reaction inertia (J) is far lighter than the mass at the same
stiffness. An earlier fixed policy, N = ceil(h / 2.5 ms) = 2 substeps
(h_sub = 2.5 ms), placed the vertical root at |λ·h| = 1.6 (stable) but
the rotational root at **|λ·h| = 3.0 — outside RK4's 2.785 real-axis
stability limit** (amplification ≈ 1.43× per substep, 400 substeps/s).
Empirically the one-sided clamp (f_n ≥ 0) tames this in the
open-loop rocking regime (a probe grid of 81 initial-condition/policy
combinations showed no divergence), but a marginally-unstable integrator
under closed-loop excitation (PX4's rate controller pushing against
loaded contacts) is not a defensible operating point for a simulator.

**Decision** (`sitsim-core::stable_substeps`):

- `contact_fast_root(p)` computes the dominant fast root in closed form
  (max of the vertical and both rotational roots, overdamped fast root
  ω_n(ζ + √(ζ²−1)), underdamped magnitude ω_n).
- `stable_substeps(p, h) = clamp(ceil(λ_max·h / 1.5), 1, 64)` — 46%
  margin to the 2.785 limit, covering mode mixing during penetration
  chatter. At SPEC defaults and h = 5 ms this gives **N = 4**
  (h_sub = 1.25 ms, λ·h = 1.5).
- N is a pure function of (parameters, h): the determinism contract
  (SPEC §8.1) is preserved.
- **Divergence is never silent**: `QuadDynamics::diverged` latches on the
  first non-finite state component; the engine emits a one-line
  diagnostic (tick, inputs, substeps, contact-root estimate) and the CLI
  exits **5** (new exit code, SPEC §4.3). NaN frames never reach PX4 —
  the previous behavior (streaming NaN until the EKF dead-reckoned and
  the truth silently froze) is exactly what hid the I-2 failure for as
  long as it did.

**Verification**:

- `model_validation::contact_root_and_substep_sizing` — the dominant
  root lands in the expected band (~1.2e3 s⁻¹) and every sized substep
  keeps λ·h_sub ≤ 1.5.
- `model_validation::pd_stabilized_takeoff_soak_stays_finite` — a PD
  damped takeoff/climb/landing soak under the sized policy: finite
  throughout, |ω| bounded, climbs, settles back on the ground.
- The I-2 root cause itself turned out to be the actuator wire mapping
  (ADR 0011r), NOT contact instability; this ADR is the hardening that
  made the failure observable (exit 5 + diagnostic) instead of silent.

**Cost**: 4 RK4 substeps of the 18-dim ODE per tick at 200 Hz ≈ 3 µs —
negligible. Replay/telemetry hashes change vs v0.1 (different integration
trajectory); determinism is preserved run-to-run.
