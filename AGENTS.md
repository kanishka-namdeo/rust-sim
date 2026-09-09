# DOX Framework — RustSim repository rail

## Purpose

This file is the repository-wide `AGENTS.md` contract for **RustSim** — the
unified testbed of PX4-native lockstep simulation (`sim/`), multi-vehicle
fleet operations (`fleet/`), and the live operator console (`console/`).
It provides portable guidance for agents and maintainers; it is not a
security boundary or deterministic permission system.

Core contract: work products, source materials, instructions, records, assets, and durable docs must stay understandable from the nearest applicable `AGENTS.md` plus every parent `AGENTS.md` above it.

## Repository Scope

- Apply this file to the repository root and all descendants.
- Add a nested `AGENTS.md` only for a durable package, service, library, infrastructure domain, or generated-artifact boundary.
- Do not create speculative child files merely to populate an index.
- RustSim repo rules (below) bind every component; component `AGENTS.md`
  files specialize them locally.

The repo is in **GCS v1** scope: turning `console/` into a QGC/MP-class
ground control station for PX4 SITL only. The buildout is structured as
7 milestones (M1..M7) backed by 14 verification gates (G-0..G-13, which
extend the existing I/F/S/O/R ladder in `docs/VERIFICATION.md`). The
buildout introduced a third Rust control plane — the `:8300` mission
catalog + replay server (binary `fleet-catalog`, owned by
`fleet/crates/fleet-mission/`, see ADR-0027) — and grew the console
from 4 mounted views to 7 tabs (Plan, Fly, Sim Console, Fleet C2,
Operator Map, Vehicle Setup, Analyze). The binding spec is
`docs/GCS_SPEC.md`; GCS-specific ADRs live in `console/docs/adr/`
(numbered 0019+ to continue the cross-repo sequence).

The **GCS v2 "Operations Canvas"** redesign — single-screen full-bleed
MapLibre GL map with edge-HUD widgets, right-click context menus, and
single-key shortcuts (milestones M8..M15, gates G-14..G-21, zero backend
changes) — is specified in `docs/GCS_V2_SPEC.md`. It supersedes
`GCS_SPEC.md`'s presentation-layer contracts. Until M8 lands, the shipped
console remains GCS v1 (7 tabs, Leaflet); v2 spec statements describe
target state, not shipped state.

## RustSim Repository Rules (Local Contracts)

- No credentials in the tree: git auth tokens (session PATs) live only in
  `.git/config`, never in committed files, docs, or harness scripts.
- Wire truth is live-captured: new protocol facts discovered against real
  PX4 go into the owning `PROTOCOL.md`/ADR with capture evidence, not into
  code comments alone.
- Integration work happens in single-invocation harnesses (start → assert →
  teardown in one shell call; background processes do not survive between
  shell calls on some platforms): `sim/tests/run_i1.sh`,
  `sim/tests/run_i2_flight.sh`, `fleet/tests/run_f1.sh`, `fleet/tests/run_f2.sh`,
  `fleet/scripts/live_test_setup.sh`, `fleet/tests/live_test_operator.sh`,
  `scripts/browser_live_test.sh`, `scripts/browser_setup_test.sh`,
  `scripts/browser_map_test.sh`.
- Sandbox / lean-container bring-up follows `docs/SANDBOX_SETUP.md`
  top-to-bottom; every deviation recorded there was hit live. Do not
  improvise around it.
- Claims discipline: docs record only what a harness proved (`docs/
  VERIFICATION.md`); a spec table lists implemented routes and names
  planned-but-unbuilt ones explicitly.

## Work Guidance

- Current test baselines: `sim/` 99 unit tests (97 pass in a no-PX4
  sandbox; the 2 in `tests/fake_px4_boot.rs` require a live PX4 SITL
  process), `fleet/` 285 unit tests (was 169 before GCS v1 added the
  `fleet-mission` GCS modules + tests), `console/` `npm run lint` +
  `npm run build` clean.
- GCS v1 verification ladder: 14 gates G-0..G-13 under
  `console/tests/run_g*.sh` (single-invocation: start → assert →
  teardown, see `docs/GCS_SPEC.md` §9). All 14 are green as of M7.
- Before changing FSM/policy/allocation/wire behavior, read the owning SPEC
  section and ADRs; ADRs override older spec text where they conflict.
- Time budgets in harnesses are measured against real dynamics — do not
  shrink them without re-running the corresponding live case.

## Hierarchy and Precedence

- Guidance accumulates from the repository root toward the target path.
- The root is the DOX rail: project-wide instructions, global preferences, durable workflow rules, and the top-level Child DOX Index.
- Child files own domain-specific instructions and their own Child DOX Index.
- Each parent explains what its direct children cover and what stays owned by the parent.
- The closer a doc is to the work, the more specific and practical it must be. Put broad rules in parent docs and concrete details in child docs.
- Conflict order:
  1. Explicit current user instructions take priority over repository guidance.
  2. Applicable child guidance takes priority over broader guidance for local details.
  3. Repository-wide safety and governance rules remain binding; a child may clarify or specialize local rules but may not weaken them.
  4. Sibling instructions do not apply outside their own subtrees.
  5. Report unresolved conflicts instead of guessing; remove recurring ambiguity by updating the owning `AGENTS.md`.
- These are DOX semantics; individual tools may load a different or partial instruction chain.

## Required Child File Shape

Every governed `AGENTS.md` uses this order:

1. Purpose
2. Ownership
3. Local Contracts
4. Work Guidance
5. Verification
6. Child DOX Index

Nested files state their exact governed path, local ownership, local contracts, existing verification, and governed descendants. They reference inherited guidance instead of copying it.

- Work Guidance must reflect the current standards of the project or user instructions; if there are no specific standards or instructions yet, leave it empty.
- Verification must reflect an existing check; if no verification framework exists yet, leave it empty and update it when one exists.

## Style

- Keep docs concise, current, and operational.
- Document stable contracts, not diary entries.
- Prefer direct bullets with explicit names.
- Do not duplicate rules across many files unless each scope needs a local version.
- Delete stale notes instead of explaining history.
- Trim obvious statements, repeated rules, misplaced detail, and warnings for risks that no longer exist.

## Read Before Editing

1. Read the root `AGENTS.md`.
2. Identify every file or folder expected to change.
3. Walk from the repository root to each target path.
4. Read every `AGENTS.md` found along each route. If a parent Child DOX Index lists a child whose scope contains the path, read that child and continue from there.
5. Use the nearest `AGENTS.md` as the local contract and parent docs for repository-wide rules.
6. If behavior is unclear or instructions conflict, stop and report the ambiguity.

Do not rely on memory. Re-read the applicable DOX chain in the current session before editing.

## Update After Editing

Every meaningful change requires a DOX pass before the task is done.

1. Check whether purpose, scope, ownership, responsibilities, durable structure, contracts, workflows, operating rules, inputs, outputs, permissions, constraints, side effects, artifacts, or user preferences about behavior, communication, process, organization, or quality changed.
2. Update the nearest owning `AGENTS.md` when those responsibilities changed.
3. Update parent docs when parent-level structure, ownership, workflow, or child index changes. Update child docs when parent changes alter local rules.
4. Refresh every affected Child DOX Index.
5. Remove stale or contradictory instructions.
6. Run relevant existing verification.
7. Report any governance file intentionally left unchanged and why.

Small edits that do not change behavior or contracts may leave docs unchanged, but the DOX pass still must happen.

## User Preferences

When the user requests a durable behavior change, record it here or in the relevant child `AGENTS.md`.

- **Proactive web search**: when a task introduces an unfamiliar project,
  error, library, or protocol, run a web search for context *before* guessing
  at fixes. Record the query and the most useful result URL in the worklog.
- **Commit + push on every development milestone**: a milestone is any point
  at which a build, test, lint, or run gate passes (or is intentionally
  blocked and the blocker is recorded). Do not accumulate uncommitted
  changes across milestones — push each one to `origin/<current-branch>` so
  the remote reflects the verified state. Use a Conventional-Commits-style
  subject (`chore:`, `docs:`, `feat:`, `fix:`, `test:`) and reference the
  milestone name in the body. Never commit session PATs, `.env` files, build
  artifacts, or anything under `target/`, `node_modules/`, `.next/`, or
  `*_artifacts/`.

## Verification

- `cargo test --workspace` green in `sim/` (99 tests; 2 of them need a
  live PX4 SITL process — see `sim/tests/fake_px4_boot.rs`) and `fleet/`
  (285 tests); `console`: `npm run lint` + `npm run build` clean.
- Live harness ladder recorded with evidence in `docs/VERIFICATION.md`:
  I-1/I-2 (single vehicle, real PX4), F-1/F-2 (fleet, real dynamics),
  S-1/S-2 (vehicle-setup plane), O-1/O-2 (operator map control),
  R-1 (runtime control plane: fault proxy, task append, hot scenarios
  load), browser-live (console end-to-end through the gateway).
- GCS v1 ladder: G-0..G-13 under `console/tests/run_g*.sh` — 14 gates
  spanning mission version-check, validation, persistence, MAVLink
  upload + download, Fly View telemetry, multi-vehicle select, pre-arm
  + arm, Vehicle Setup param extensions, fleet mission binding +
  orchestration, ULog + replay analyze, and survey/corridor/perimeter
  patterns (GCS_SPEC.md §9). All 14 are green as of M7.

## Compatibility and Security Limits

- Use ordinary Markdown and headings only; do not require frontmatter, custom parsers, or tool-specific commands.
- Verify instruction discovery and precedence for each supported agent and version.
- Never treat `AGENTS.md` as access control, secret protection, deployment approval, or compliance enforcement.
- Put deterministic restrictions in repository permissions, CI, deployment controls, secret management, or equivalent tooling.

## Child DOX Index

| Child | Scope |
|-------|-------|
| `sim/AGENTS.md` | RustSim Core workspace: physics, sensors, MAVLink HIL codec, replay, fault engine, control plane |
| `fleet/AGENTS.md` | RustSim Fleet workspace: mission manager, links, allocator, safety policies, fleet control plane, `fleet-mission` GCS catalog (`:8300`) |
| `console/AGENTS.md` | Operator console: 7 mounted tabs (Plan, Fly, Sim Console, Fleet C2, Operator Map, Vehicle Setup, Analyze), gateway/direct API routing incl. `:8300`, build & run |
| `docs/AGENTS.md` | Cross-repo documentation: architecture, GCS_SPEC.md, verification evidence, operations runbook, sandbox setup sequence |
| `scripts/AGENTS.md` | Shared cross-repo scripts: the three browser live tests, golden-vector generator |
| `console/docs/adr/` | GCS-specific ADRs numbered 0019+ (child of `console/AGENTS.md` — listed there with the accepted/proposed split) |
