# docs/ — cross-repo documentation

## Purpose

Repo-level documentation that spans components: the architecture and port
contracts, the live verification record, and the operations runbook.
Component-specific docs (SPEC, PROTOCOL, SCENARIOS, ADRs) stay with their
components in `sim/docs/` and `fleet/docs/`.

## Ownership

Owned here: `ARCHITECTURE.md`, `VERIFICATION.md`, `OPERATIONS.md`,
`images/`. Not owned here: component specs/ADRs (see `../sim/AGENTS.md`,
`../fleet/AGENTS.md`).

## Local Contracts

- `VERIFICATION.md` records only live-run results with harness paths — no
  aspirational claims; update it when a harness result changes.
- `images/` holds evidence screenshots referenced from the READMEs.

## Work Guidance

- Keep the port map in `ARCHITECTURE.md` in sync with `fleet-simctl`'s port
  constants and the console's `SIM_PORT` / `FLEET_PORT` hooks.

## Verification

- Screenshots referenced by READMEs exist in `images/`.
- Statements in `VERIFICATION.md` map 1:1 to harnesses that exist in the
  tree.

## Child DOX Index

None (flat directory).
