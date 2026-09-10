# fleet/ — RustSim Fleet (mavfleet + fleet-catalog + fleet-supervisor)

## Purpose

Multi-vehicle mission manager: spawns and supervises per-vehicle
sim+PX4 pairs (scenario `[sim]` command template →
`scripts/run_sitsim_vehicle.sh`, each pair CPU-pinned to its own core
via `taskset` since 2026-09-09 — PX4's arrival-driven SITL sensor
timing rails the EKF2 accel bias under cross-pair scheduler jitter;
kill switch `RSIM_NO_CPU_AFFINITY`), exports the scenario `[env]`
wind/turbulence to the sim wrapper
(`RSIM_WIND_MS`/`RSIM_TURBULENCE`), binds per-vehicle MAVLink
telemetry/onboard links, aggregates a 10 Hz FleetFrame, serves the
fleet REST+WS control plane on `:8400` (operator-driven flight —
arm/takeoff/land/rtl/hold/goto/mission upload/start, per-vehicle
mission bindings + fleet-start parallel + sequential, e-stop,
passive geofence-breach detection), and hosts the `:8300` mission
catalog server (`fleet-catalog` binary, ADR-0027) for the console's
Plan / Library / Analyze overlays to CRUD missions and stream `.ulg`
artifacts (the `/api/replays*` routes were removed 2026-09-10 with
the Analyze `.replay` tab). The `:8500` fleet-supervisor
(`fleet-supervisor` binary, ADR-0030) owns the `mavfleet` child
process lifecycle so the GCS UI can start/stop SITL on demand
(operator-driven SITL lifecycle, QGC/MP pattern).

## Ownership

Owned here: FSM (fleet-core), passive geofence (fleet-safety — the
8-policy safety ladder was removed 2026-09-10), MAVLink
command/ack semantics (fleet-mavlink), fleet control-plane schema
(spec §3.4 — the auction allocator / mission runner / 8-policy ladder
/ scenario DSL / fault proxy / runtime hot-swap / task-append
routes were all removed 2026-09-10), and the GCS v1 `:8300` catalog
server (`fleet-catalog` binary in the `fleet-mission` crate). The
catalog's library modules live in `crates/fleet-mission/src/gcs/`
(the scenario DSL + compiler + per-vehicle runner + run-report
modules that used to live alongside were removed end-to-end in the
2026-09-10 cleanup — `fleet-mission` is now catalog-only):

- `gcs/mod.rs` — module root.
- `gcs/mission_file.rs` — `MissionFile` serde struct (ADR-0019:
  TOML on disk, JSON on wire; the single source of truth for both
  serializations).
- `gcs/store.rs` — filesystem persistence with atomic writes
  (ADR-0020: temp-file + fsync + rename; version history as sibling
  files; tombstone soft-deletes).
- `gcs/validation.rs` — strict mission validation (ADR-0026: empty
  mission rejected, waypoints must be inside the inclusion geofence,
  any simple polygon permitted, rally ≤ 5 points).
- `gcs/version_check.rs` — PX4 version policy enforcement (ADR-0029:
  hard reject with HTTP 426 if the vehicle's reported PX4 version is
  not exactly `v1.16.2`; `RSIM_ALLOW_UNPINNED_PX4=1` escape hatch for
  core-team capture sessions).
- `gcs/server.rs` — the axum app for `:8300`; thin handlers
  delegating to `store` / `validation` / `version_check` / `ulog`
  (the `replay` module + `/api/replays*` routes were removed
  2026-09-10).
- `gcs/ulog.rs` — `.ulg` filesystem listing + meta; ULog parsing is
  delegated to `pyulog` per ADR-0021 (graceful 503 when unavailable).
- `gcs/preset.rs` — vehicle-param presets (ADR-0025 draft;
  per-vehicle TOML files, in-place overwrite, no version history).

The `fleet-cli` crate ships **three** binaries post-2026-09-10:

- `mavfleet` — the fleet manager (`run` subcommand only; the
  `check` subcommand was removed 2026-09-10 because it validated the
  removed scenario DSL).
- `fleet-supervisor` — the SITL lifecycle manager on `:8500`
  (ADR-0030; new 2026-09-10). Routes: `GET /api/sitl/status`,
  `POST /api/sitl/start`, `POST /api/sitl/stop`,
  `GET /api/sitl/scenarios`, `GET /api/health`. Started by
  `scripts/stack_up.sh start` BEFORE the fleet; the fleet is started
  on-demand by the supervisor when the operator POSTs `/api/sitl/start`
  (or clicks the GCS UI's SITL Manager panel button).
- `fleet-catalog` — actually built from the `fleet-mission` crate
  (ADR-0027), not `fleet-cli`, but listed here for the binary roster.

Not owned here: vehicle dynamics and wire codec (`../sim/`), operator
UI (`../console/`), repo-level contracts (`../AGENTS.md`). The GCS
v2 spec (`../docs/GCS_V2_SPEC.md`) owns the scope and feature areas;
ADRs for the `:8300` catalog live in `../console/docs/adr/` (0019,
0020, 0026, 0027, 0029 accepted; 0021, 0024, 0025 proposed); ADR-0030
(operator-driven SITL lifecycle) lives in
`../console/docs/adr/0030-sitl-supervisor.md`.

## Local Contracts

- `#![forbid(unsafe_code)]`; pure decision logic in library crates
  (fleet-core/safety/modes/simctl/mavlink), async composition only in
  fleet-cli.
- The lean `config.rs` parser (`fleet-cli/src/config.rs`) accepts the
  `[fleet]`, `[env]`, `[sim]`, `[[vehicle_override]]` keys for
  forward-compat (the `[[tasks]]`, `[[event]]`, `[success]` keys are
  silently ignored — they were the scenario DSL's task/event/success
  blocks, removed 2026-09-10 but kept parseable in the persistent
  `tests/*.toml` fixtures to avoid churning the live harness scripts).
- Scenario DSL parse + compile + run + report are GONE; `mavfleet run`
  just spawns N PX4 SITL + sim pairs and serves the operator-driven
  control plane.
- NED everywhere in setpoints: up = negative z. Transit layer is
  `10 + 5k m` AGL (the 8-policy ladder's Policy 7 AltitudeDiverge
  override was removed 2026-09-10; transit layers are now advisory
  only).
- Exit codes: 0 COMPLETE, 2 ABORTED, 3 error/infrastructure.

## Work Guidance

- Read `docs/SPEC.md` (kept as the v0.1 design record — see the
  Historical banner at the top) and `docs/adr/` (0018 is marked
  Superseded/Historical) before changing FSM/geofence behavior;
  ADRs override older spec text where they conflict.
- Arm/offboard semantics are PX4-v1.16-verified (ADR-0009/0010):
  DO_SET_MODE decomposes into param2/param3; the GCS heartbeat pump
  is required for arming; the 1 Hz pump is not optional.
- LAND_TIMEOUT_MS (120 s) and the boot budget are measured against
  real dynamics (ADR-0011) — do not shrink them to speed up tests
  without re-running F-1.
- `scripts/run_sitsim_vehicle.sh` defaults are repo-relative
  (`../../sim/target/debug/sitsim-cli`); override with
  `FLEET_SITSIM_BIN`.

## Verification

- `cargo test --workspace` — 285 tests (post-2026-09-10). Covers
  FSM + geofence (passive — the 8-policy ladder was removed),
  fleet-mavlink codec + golden vectors, fleet-modes decode,
  fleet-simctl spawn, fleet-cli router tests incl. gateway-shaped WS
  upgrades, fleet-supervisor integration tests (new 2026-09-10), and
  the `:8300` catalog's mission-file round-trip, store atomic-write,
  validation, version-check, and preset unit tests. (The deleted
  buckets: auction + Hungarian, mission runner, 8-policy ladder,
  ADR-0018 runtime-control-plane — see the cleanup note in
  `../docs/VERIFICATION.md`.)
- Live: `tests/run_f1.sh` (bring-up + estop; was 7 steps + report
  assertions, trimmed 2026-09-10 to events-only — the run-report.json
  assertions were removed with the run-report writer).
  `tests/demo_live.toml` + `../scripts/browser_live_test.sh` for the
  console experience. (The deleted harnesses: `tests/run_f2.sh` /
  F-2 — removed 2026-09-10 with the auction-flown mission; the
  `R-1` runtime-control-plane harness was removed with the runtime
  control plane.)
- Operator plane: `tests/live_test_operator.sh` (O-1, ADR-0017) +
  `../scripts/browser_map_test.sh` (O-2) — the geo frame, go-to,
  guided commands, fence-validated mission upload (operator-driven;
  the auction-flown path was removed 2026-09-10) against real PX4.
- SITL lifecycle (ADR-0030, 2026-09-10): `scripts/stack_up.sh start`
  brings up catalog + supervisor + console; the SITL Manager panel
  POSTs `/api/sitl/start` on the supervisor (`:8500`), which spawns
  `mavfleet run`; `GET /api/sitl/status` reports running + vehicle
  count + PID; `POST /api/sitl/stop` tears it down.
- GCS v1 G-ladder: the catalog-side endpoints on `:8300` are
  exercised by the `console/tests/run_g*.sh` harnesses (11 surviving
  post-2026-09-10; see `../console/AGENTS.md`); they spin up
  `fleet-catalog` directly or via the Caddy gateway
  `?XTransformPort=8300`.
- Boot-gate polling in both harnesses accepts READY or any post-READY
  FSM state (ACTIVE/RTL/LANDED): the all-READY snapshot window can be
  <200 ms wide on fast machines because vehicles flip READY→ACTIVE
  together once the telemetry gate opens. Do not revert to a strict
  `fsm == "READY"` predicate and do not widen time budgets to
  compensate.

## Child DOX Index

No child AGENTS.md files yet. Candidates when they become durable
boundaries: `crates/fleet-safety` (geofence-only post-2026-09-10),
`crates/fleet-mavlink` (link + ack ladder), `crates/fleet-cli/src/bin/`
(three binaries: `mavfleet`, `fleet-supervisor`, and the
`fleet-catalog` proxy from `fleet-mission`), `tests/` (live
harnesses).
