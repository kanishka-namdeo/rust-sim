# scripts/ — shared cross-repo scripts

## Purpose

Scripts that operate across component boundaries (or on repo-level
artifacts): the three browser live tests that drive fleet + console
together, the golden-vector generator for the sim's MAVLink codec, and
the persistent operator-stack launcher.

## Ownership

Owned here: `browser_live_test.sh` (browser-live), `browser_setup_test.sh`
(S-2, vehicle setup), `browser_map_test.sh` (O-2, operator map),
`gen_golden93.py`, `stack_up.sh` (start/stop/status for the daemonized
operator stack: catalog :8300 + fleet :8400 + console :3000, fleet on
`tests/operator_session.toml` — double-fork daemons survive agent-shell
process reaping; see SANDBOX_SETUP.md reval 2026-09-09). Component-owned
harnesses stay in their components (`../sim/tests/`, `../fleet/tests/`,
`../fleet/scripts/`).

## Local Contracts

- All three browser tests are single-invocation (start fleet + console + a
  headless browser, assert, teardown) and assume: both Rust binaries built,
  the console built (`console/.next/standalone`), PX4 at `PX4_ROOT`, and the
  Caddy gateway answering on :81 (a 502 means the gateway is up and the
  backends are not — that is the expected precondition). Screenshots land
  in `docs/images/`. The setup and map tests also kill leaked
  px4/sim/next-server processes from earlier runs — they squat the
  per-instance ports or serve a stale bundle and abort the fresh run.
- `gen_golden93.py` regenerates the PX4-v1.16-layout golden vector consumed
  by `sim/crates/sitsim-mavlink/tests/golden_vectors.rs` — after running it,
  the affected Rust tests must be re-run.

## Work Guidance

- Keep the console path and ports in the browser tests in sync with
  `../console/AGENTS.md` and the port map in `../docs/ARCHITECTURE.md`.
- The scripts intentionally test through the gateway origin (:81), not the
  direct :3000 origin, because the gateway path is the production routing.
- `browser_map_test.sh` engages on ANY vehicle winning the auction (a
  boot-time EKF drift can fence-edge vehicle 0 out of the bid set) —
  never hard-code the winner in fleet assertions.

## Verification

- `browser_live_test.sh` exits 0 only when both consoles report LIVE,
  telemetry is demonstrably moving, and screenshots were written.
- `browser_setup_test.sh` exits 0 only when the Vehicle Setup tab passes its
  13 end-to-end checks (S-2); `browser_map_test.sh` only when the Operator
  Map passes its 16 (O-2). Both are recorded in `../docs/VERIFICATION.md`.

## Child DOX Index

None (flat directory).
