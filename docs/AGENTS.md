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
`DEPLOYMENT.md`, `SANDBOX_SETUP.md`, `GCS_SPEC.md`, `GCS_V2_SPEC.md`,
`images/`. Not owned here: component specs/ADRs (see `../sim/AGENTS.md`,
`../fleet/AGENTS.md`, `../console/AGENTS.md`).

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
- `GCS_SPEC.md` is the binding engineering spec for turning `console/`
  into a QGC/MP-class GCS for PX4 SITL only (v1). It defines the 6
  feature areas (Plan, Fly, Setup, Fleet C2, Analyze, Survey Patterns),
  the new `:8300` mission-catalog + replay server, the G-0..G-13
  verification gates that extend the existing I/F/S/O/R ladder, the
  M1..M7 milestone roadmap, and is grounded in 2026 market research
  (§1.1 Market Context, §2.3 Competitive Landscape). All UX flows in
  §8 are zero-assumption step-by-step procedures: each step specifies
  the exact UI element label, the pre-condition that must hold, the
  action, the resulting state change, the post-condition, and any
  error paths — no implicit assumptions remain. ADRs that block M1
  (0019, 0020, 0026, 0029) must be accepted before implementation
  begins; the remaining ADRs (0021..0025, 0027, 0028) can be resolved
  in parallel with their owning milestone. Update this spec only when
  scope, architecture, the G-ladder, or the market/competitive context
  changes — not for ADR-level decisions (those go in
  `console/docs/adr/`).
- `GCS_V2_SPEC.md` is the binding engineering spec for the **GCS v2
  "Operations Canvas"** redesign (single full-bleed MapLibre GL map +
  edge-HUD widgets + right-click context menus + single-key shortcuts,
  M8..M15 milestones, G-14..G-21 gates). It supersedes `GCS_SPEC.md`'s
  presentation-layer contracts (§8 UX flows, UI-facing gate assertions)
  and guarantees **zero backend changes** (all ADRs 0016–0029 and the
  G-0..G-13 backend gates carry over). Until M8 lands, the shipped
  console remains GCS v1; v2 statements describe target state, not
  shipped state. Update it only via the decision-log/section-amendment
  pattern it defines (§1.4, §13.3).

## Work Guidance

- Keep the port map in `ARCHITECTURE.md` in sync with `fleet-simctl`'s port
  constants and the console's `SIM_PORT` / `FLEET_PORT` hooks.
- **Known debt (post-M7):** `ARCHITECTURE.md`'s ASCII topology + port map
  still show only `:8200+i` and `:8400`. It must be extended to include the
  GCS v1 `:8300` mission-catalog + replay server (binary `fleet-catalog`,
  owned by `fleet/crates/fleet-mission/`, ADR-0027) and the 7 console tabs.
  `GCS_SPEC.md` §4.2/§4.3 is the authoritative reference for the v1 topology
  and port map until `ARCHITECTURE.md` catches up.

## Verification

- Screenshots referenced by READMEs exist in `images/`.
- Statements in `VERIFICATION.md` map 1:1 to harnesses that exist in the
  tree.

## Child DOX Index

None (flat directory).
