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

The repo is in **GCS lean-scope** (post-cleanup 2026-09-10): turning
`console/` into a QGC/MP-class ground control station for PX4 SITL only,
with overkill features removed end-to-end. The GCS v1 buildout was
structured as 7 milestones (M1..M7) backed by 14 verification gates
(G-0..G-13). The GCS v2 "Operations Canvas" redesign (M8..M15, gates
G-14..G-21) is the current shipped state — single-screen full-bleed
MapLibre GL map with edge-HUD widgets, right-click context menus, and
single-key shortcuts.

**2026-09-10 lean cleanup** removed end-to-end:
- **UI**: Sim Console overlay (physics telemetry + fault console + SIM
  E-STOP — overkill for a GCS), Fleet C2 swarming patterns panel,
  Analyze `.replay` tab (custom sim format; ULog stays), fault/swarm
  command verbs (`fault_inject`/`fault_clear`/`estop_sim`/`scenario_hotswap`/`task_append`),
  the `SimFrame`/`ActiveFault`/`FAULT_CATALOG`/`SwarmPattern`/`Replay*` types,
  the per-vehicle sim socket ladder in the telemetry store, the mock-sim
  engine, and the 3 dead v1 dashboard widgets (StatTile/StripChart/ConnBadge).
- **Backend**: `fleet-alloc` crate (auction + Hungarian), `fleet-safety/policy.rs`
  (8-policy ladder — PX4's own failsafes cover the operator case),
  `fleet-mission` scenario DSL + compiler + runner + report, `fleet-cli/simproxy.rs`
  (HTTP/1.1 fault proxy), `fleet-cli/report.rs` (supervisor run-report),
  routes `PUT /api/fleet` (hot-swap), `POST /api/tasks` (runtime task append),
  `POST /api/vehicles/{i}/faults` (fault proxy), `GET/POST /api/fleet/patterns*`
  (swarming), `GET /api/replays*` (custom sim format), `mavfleet check`
  subcommand. The supervisor tick body shrank from autonomy (auction +
  runner + policy + timeline + hot-load) to "spawn N vehicles, bind links,
  10 Hz FleetFrame aggregation, serve REST+WS, e-stop".
- **DB**: no SQL/SQLite/sled — TOML/JSON-on-disk with atomic writes in
  `fleet-mission/src/gcs/store.rs` is the GCS-appropriate pattern; kept.
- **Tests**: deleted G-10 (orchestration), G-12 (replay scrub), G-19
  (analyze sim — fault console + SIM E-STOP + replays tab), R-1 harness
  (`live_test_runtime.sh`), F-2 harness (`run_f2.sh`), F-2's
  `live_test_operator.sh` step (e). Trimmed G-11 to ULog-only. Trimmed
  G-14 to drop the `simSockets` assertion. Trimmed G-20 to expect 6
  overlay panels (was 7). Trimmed G-21's gate-harness list.

The console ships 6 overlay panels (Mission Strip + Library + Fleet C2
cards/bindings/events + Setup Drawer + Analyze ULog-only + Pre-Flight +
Settings + Cheat Sheet). The fleet catalog (`:8300`) serves mission
CRUD + ULog browse + param presets. The fleet manager (`:8400`) serves
telemetry WS + arm/land/rtl/hold/goto + mission upload/download + prearm
checks + QGC-style Vehicle Setup + fleet mission bindings + e-stop. The
sim/ workspace stays as PX4's HIL physics engine (TCP 4560+i), but its
internal data is no longer consumed by the GCS UI.

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
