# fleet/ — RustSim Fleet (mavfleet + fleet-catalog)

## Purpose

Multi-vehicle mission manager: spawns and supervises per-vehicle sim+PX4
pairs (scenario `[sim]` command template -> `scripts/run_sitsim_vehicle.sh`,
each pair CPU-pinned to its own core via `taskset` since 2026-09-09 —
PX4's arrival-driven SITL sensor timing rails the EKF2 accel bias under
cross-pair scheduler jitter; kill switch `RSIM_NO_CPU_AFFINITY`), exports
the scenario `[env]` wind/turbulence to the sim wrapper
(`RSIM_WIND_MS`/`RSIM_TURBULENCE`), binds per-vehicle MAVLink
telemetry/onboard links, allocates tasks with a
sequential auction (Hungarian-optimal baseline), flies offboard missions via
a 20 Hz setpoint pump, enforces the 8-policy safety ladder, serves the fleet
REST+WS control plane on `:8400`, writes run reports + event logs with
CI-classifiable exit codes, and (new in GCS v1) hosts the `:8300` mission
catalog + replay server (`fleet-catalog` binary, ADR-0027) so the console's
Plan and Analyze tabs can CRUD missions and stream `.replay` / `.ulg`
artifacts.

## Ownership

Owned here: FSM, health/policy engine, allocator, runner profile math,
fleet-mavlink command/ack semantics, fleet control-plane schema (spec
§3.4), run report format, and the GCS v1 `:8300` catalog server
(`fleet-catalog` binary). The catalog's library modules live in
`crates/fleet-mission/src/gcs/`:

- `gcs/mod.rs` — module root.
- `gcs/mission_file.rs` — `MissionFile` serde struct (ADR-0019:
  TOML on disk, JSON on wire; the single source of truth for both
  serializations).
- `gcs/store.rs` — filesystem persistence with atomic writes
  (ADR-0020: temp-file + fsync + rename; version history as sibling files;
  tombstone soft-deletes).
- `gcs/validation.rs` — strict mission validation (ADR-0026: empty
  mission rejected, waypoints must be inside the inclusion geofence, any
  simple polygon permitted, rally ≤ 5 points).
- `gcs/version_check.rs` — PX4 version policy enforcement (ADR-0029:
  hard reject with HTTP 426 if the vehicle's reported PX4 version is not
  exactly `v1.16.2`; `RSIM_ALLOW_UNPINNED_PX4=1` escape hatch for core-team
capture sessions).
- `gcs/server.rs` — the axum app for `:8300`; thin handlers delegating
to `store` / `validation` / `version_check` / `replay` / `ulog`.
- `gcs/replay.rs` — `.replay` file parser (64-byte header + 96-byte fixed
tick records; vendored here so `fleet-mission` does not pull in the full
sim workspace).
- `gcs/ulog.rs` — `.ulg` filesystem listing + meta; ULog parsing is
delegated to `pyulog` per ADR-0021 (graceful 503 when unavailable).
- `gcs/preset.rs` — vehicle-param presets (ADR-0025 draft; per-vehicle
  TOML files, in-place overwrite, no version history).

Not owned here: vehicle dynamics and wire codec (`../sim/`), operator UI
(`../console/`), repo-level contracts (`../AGENTS.md`). The GCS v1 spec
(`../docs/GCS_SPEC.md`) owns the scope and feature areas; ADRs for the
`:8300` catalog live in `../console/docs/adr/` (0019, 0020, 0026, 0027,
0029 accepted; 0021, 0024, 0025 proposed).

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

- `cargo test --workspace` — 285 tests (was 169 before GCS v1 added the
  `fleet-mission` GCS modules + their unit tests). Covers FSM/policy/alloc/
  runner unit tests, router tests incl. gateway-shaped WS upgrades, and the
  `:8300` catalog's mission-file round-trip, store atomic-write,
  validation, version-check, replay-parsing, and preset unit tests.
- Live: `tests/run_f1.sh` (bring-up + estop), `tests/run_f2.sh` (2 vehicles
  with real rustsitsim dynamics, physical flight asserted from replay ground
  truth), `tests/demo_live.toml` + `../scripts/browser_live_test.sh` for the
  console experience.
- Operator plane: `tests/live_test_operator.sh` (O-1, ADR-0017) + `../
  scripts/browser_map_test.sh` (O-2) — the geo frame, go-to, guided
  commands, fence-validated mission upload and the auction-flown op* tasks
  against real PX4.
- Runtime plane: `tests/live_test_runtime.sh` (R-1, ADR-0018) — the fault
  proxy to the sims' REST planes, runtime NED task append, and the hot
  scenario load (staged PUT, graceful abort, same-port rebind, isolated
  run dirs), all against real PX4.
- GCS v1 G-ladder: the catalog-side endpoints on `:8300` are exercised by
  the `console/tests/run_g*.sh` harnesses (G-0..G-13, see
  `../console/AGENTS.md`); they spin up `fleet-catalog` directly or via
  the Caddy gateway `?XTransformPort=8300`.
- The scenario `fault` events inject for real through the sim fault plane
  (ADR-0018; the old "NOT injected" interim placeholder is gone). Keep
  fault windows clear of the arming phase — a live gps_denial blocks
  PX4's GPS-dependent arming gate for 10+ s (ADR-0018, live-captured).
- Boot-gate polling in both harnesses accepts READY or any post-READY FSM
  state (ACTIVE/RTL/LANDED): the all-READY snapshot window can be <200 ms
  wide on fast machines because vehicles flip READY->ACTIVE together once
  the telemetry gate opens. Do not revert to a strict `fsm == "READY"`
  predicate and do not widen time budgets to compensate.

## Child DOX Index

No child AGENTS.md files yet. Candidates when they become durable boundaries:
`crates/fleet-safety` (policy engine), `crates/fleet-mavlink` (link + ack
ladder), `tests/` (live harnesses).
