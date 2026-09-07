# sim/ — RustSim Core (rustsitsim)

## Purpose

Lockstep HIL flight simulator that drives unmodified PX4-Autopilot v1.16.2
SITL: owns the vehicle (6-DOF quadrotor dynamics), the sensor models, the
MAVLink v2 HIL wire codec, virtual time, replay, fault injection, and a
REST+WS control plane. It is the physics/protocol half of every live test in
this repo.

## Ownership

Owned here: dynamics math, sensor noise models, HIL message byte layouts and
lockstep semantics, fault engine, replay format, `sitsim-cli` behavior and
exit codes, the control-plane API schema (SPEC §4).

Not owned here: mission logic and fleet safety (see `../fleet/`), operator
UI (see `../console/`).

## Local Contracts

- `#![forbid(unsafe_code)]` in every crate; no external MAVLink crate — the
  codec is hand-rolled and golden-vector-tested against PX4's generated
  headers.
- Wire truth = PX4 v1.16.2's pinned dialect, not the official common.xml
  where they diverge (HIL_ACTUATOR_CONTROLS size-sorted layout, ADR-0015;
  HIL_SENSOR `fields_updated` u32; HIL_GPS `fix_type` position).
  Divergences are ADR'd in `docs/adr/`.
- Exit codes: 0 clean, 2 scenario failure, 3 PX4 disconnect, 4 config error,
  5 numerical divergence.
- Scenario TOML schema is frozen (SPEC §2, `deny_unknown_fields`).
- Control plane: JSON envelope `{"ok","data","error"}` on every endpoint; WS
  path tolerance for gateway-forwarded sockets (`/?XTransformPort=<port>`).

## Work Guidance

- `docs/PROTOCOL.md` is the ICD — update it (with live evidence) whenever
  wire behavior changes; new findings get an ADR in `docs/adr/`.
- Sensor realism is arming-critical: zero-noise IMU/mag starves PX4's EKF2
  (ADR-0012/0014) — do not "clean up" noise defaults.
- Determinism: same scenario + seed => same telemetry hash (SPEC §8.1); keep
  the per-sensor RNG streams.
- Integration harnesses must be single-invocation (start, assert, teardown in
  one shell call); follow `tests/run_i1.sh`.

## Verification

- `cargo test --workspace` — codec golden vectors, dynamics validation,
  sensor model tests, engine mapping regressions (incl. the PX4 v1.16 wire
  layout).
- Live: `tests/run_i1.sh` (PX4 boot gate), `tests/run_i2_flight.sh`
  (arm -> offboard climb -> hover -> land -> disarm, direct wire).
- `../docs/VERIFICATION.md` records the current PASS state.

## Child DOX Index

No child AGENTS.md files yet. Candidates when they become durable boundaries:
`crates/sitsim-mavlink` (wire codec + golden vectors), `crates/sitsim-sdk`
(engine + replay), `tests/` (live harnesses).
