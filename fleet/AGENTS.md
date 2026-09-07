# fleet/ — RustSim Fleet (mavfleet)

## Purpose

Multi-vehicle mission manager: spawns and supervises per-vehicle sim+PX4
pairs (scenario `[sim]` command template -> `scripts/run_sitsim_vehicle.sh`),
binds per-vehicle MAVLink telemetry/onboard links, allocates tasks with a
sequential auction (Hungarian-optimal baseline), flies offboard missions via
a 20 Hz setpoint pump, enforces the 8-policy safety ladder, serves the fleet
REST+WS control plane, and writes run reports + event logs with
CI-classifiable exit codes.

## Ownership

Owned here: FSM, health/policy engine, allocator, runner profile math,
fleet-mavlink command/ack semantics, fleet control-plane schema (spec §3.4),
run report format.

Not owned here: vehicle dynamics and wire codec (`../sim/`), operator UI
(`../console/`), repo-level contracts (`../AGENTS.md`).

## Local Contracts

- `#![forbid(unsafe_code)]`; pure decision logic in library crates
  (fleet-core/alloc/mission/safety/modes/simctl/mavlink), async composition
  only in fleet-cli.
- Scenario DSL (spec §9.1) is schema-frozen; `[sim]` command template per
  ADR-0008 (placeholders `{instance}`, `{hil_port}`, `{duration_s}`,
  `{sysid}`, `{sitsim}`).
- NED everywhere in setpoints: up = negative z. Transit layer is `10 + 5k m`
  AGL; policy 7's AltitudeDiverge dz is NED (climb = negative) and applies
  only between airborne vehicles (ADR-0011).
- Exit codes: 0 COMPLETE, 2 ABORTED, 3 error/infrastructure.

## Work Guidance

- Read `docs/SPEC.md` sections before changing FSM/policy/allocation
  behavior; ADRs in `docs/adr/` override older spec text where they conflict.
- Arm/offboard semantics are PX4-v1.16-verified (ADR-0009/0010): DO_SET_MODE
  decomposes into param2/param3; the GCS heartbeat pump is required for
  arming; the 1 Hz pump is not optional.
- LAND_TIMEOUT_MS (120 s) and the boot budget are measured against real
  dynamics (ADR-0011) — do not shrink them to speed up tests without
  re-running F-2.
- `scripts/run_sitsim_vehicle.sh` defaults are repo-relative (`../../sim/
  target/debug/sitsim-cli`); override with FLEET_SITSIM_BIN.

## Verification

- `cargo test --workspace` — FSM/policy/alloc/runner unit tests + router
  tests incl. gateway-shaped WS upgrades.
- Live: `tests/run_f1.sh` (bring-up + estop), `tests/run_f2.sh` (2 vehicles
  with real rustsitsim dynamics, physical flight asserted from replay ground
  truth), `tests/demo_live.toml` + `../scripts/browser_live_test.sh` for the
  console experience.

## Child DOX Index

No child AGENTS.md files yet. Candidates when they become durable boundaries:
`crates/fleet-safety` (policy engine), `crates/fleet-mavlink` (link + ack
ladder), `tests/` (live harnesses).
