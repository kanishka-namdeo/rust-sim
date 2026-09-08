# docs/ — cross-repo documentation

## Purpose

Repo-level documentation that spans components: the architecture and port
contracts, the live verification record, the operations runbook, the
deployment guide for running the stack on an external system, and the
verified sandbox bring-up sequence.
Component-specific docs (SPEC, PROTOCOL, SCENARIOS, ADRs) stay with their
components in `sim/docs/` and `fleet/docs/`.

## Ownership

Owned here: `ARCHITECTURE.md`, `VERIFICATION.md`, `OPERATIONS.md`,
`DEPLOYMENT.md`, `SANDBOX_SETUP.md`, `images/`. Not owned here:
component specs/ADRs (see `../sim/AGENTS.md`, `../fleet/AGENTS.md`).

## Local Contracts

- `VERIFICATION.md` records only live-run results with harness paths — no
  aspirational claims; update it when a harness result changes.
- `SANDBOX_SETUP.md` records the exact, live-verified bring-up sequence for
  the Z.AI sandbox environment (toolchain gaps, registry pins, PX4 shallow-
  clone tag fix, author-layout symlinks). Update it whenever the sandbox
  environment changes or a harness's invocation pattern changes — commands
  in it must be copy-pasteable, in order, and actually pass.
- `images/` holds evidence screenshots referenced from the READMEs.
- `DEPLOYMENT.md` describes running the stack on an external system after
  `git clone` (prerequisites, one-host quickstart, LAN/tunnel access
  patterns, the port map, always-on wrapping). It must stay consistent
  with `Caddyfile.example`'s binding (:81 all interfaces, backends
  localhost) and the console's two routing modes.

## Work Guidance

- Keep the port map in `ARCHITECTURE.md` in sync with `fleet-simctl`'s port
  constants and the console's `SIM_PORT` / `FLEET_PORT` hooks.

## Verification

- Screenshots referenced by READMEs exist in `images/`.
- Statements in `VERIFICATION.md` map 1:1 to harnesses that exist in the
  tree.

## Child DOX Index

None (flat directory).
