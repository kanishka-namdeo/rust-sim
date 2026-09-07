# scripts/ — shared cross-repo scripts

## Purpose

Scripts that operate across component boundaries (or on repo-level
artifacts): the live browser test that drives fleet + console together, and
the golden-vector generator for the sim's MAVLink codec.

## Ownership

Owned here: `browser_live_test.sh`, `gen_golden93.py`. Component-owned
harnesses stay in their components (`../sim/tests/`, `../fleet/tests/`).

## Local Contracts

- `browser_live_test.sh` is single-invocation (start fleet + console + a
  headless browser, assert, teardown) and assumes: both Rust binaries built,
  the console built (`console/.next/standalone`), PX4 at `PX4_ROOT`, and the
  Caddy gateway reachable on :81. Screenshots land in `docs/images/`.
- `gen_golden93.py` regenerates the PX4-v1.16-layout golden vector consumed
  by `sim/crates/sitsim-mavlink/tests/golden_vectors.rs` — after running it,
  the affected Rust tests must be re-run.

## Work Guidance

- Keep the console path and ports in `browser_live_test.sh` in sync with
  `../console/AGENTS.md` and the port map in `../docs/ARCHITECTURE.md`.
- The script intentionally tests through the gateway origin (:81), not the
  direct :3000 origin, because the gateway path is the production routing.

## Verification

- `browser_live_test.sh` exits 0 only when both consoles report LIVE,
  telemetry is demonstrably moving, and screenshots were written.

## Child DOX Index

None (flat directory).
