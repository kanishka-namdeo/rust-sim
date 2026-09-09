# GCS v2 Spec — Implementation Readiness Review (Board Verdict)

**Date:** 2026-09-10 · **Reviewed artifact:** `docs/GCS_V2_SPEC.md` v1.1 (commit `7e00202`, 1151 lines) · **Outcome:** spec amended to **v1.2** (this commit)
**Question put to the board:** *Is the spec ready to be implemented?*
**Board:** PM (main thread) · Senior Engineering — "M8 pilot engineer" simulation (worklog Task 17) · QA lead — gate executability audit (worklog Task 18)

---

## Verdict

> ## GO — with the readiness conditions applied as spec v1.2 (this commit)
>
> The spec v1.1 was **architecture-sound and factually verified but not yet
> zero-questions executable**. The review found no hard blockers in the design;
> it found 6 coordination/instrumentation gaps concentrated in **M8, the kickoff
> milestone**, plus ~20 smaller pilot questions. All 6 gaps and the 15
> highest-leverage questions are fixed in v1.2 (amendment log below). What
> remains is explicitly en-route work with owners, not spec defects.

**Readiness scores (0–100):**

| Dimension | v1.1 | v1.2 | Basis |
|---|---|---|---|
| Architecture & decisions (D1–D8) | 95 | 95 | Unchanged — triple-verified (Tasks 13/14/15), no decision invalidated |
| Factual grounding (endpoints/WS/deps) | 95 | 95 | Source- and registry-verified (Task 13/14); wire truths locked |
| Verification plan executability | 80 | 93 | Task 18: 8/8 gates objective, 0 blocked; instruments + SOURCE column now specified |
| M8 day-1 executability | 58 | ~85 | Task 17 pilot: route mechanism, store API, gate asserts, ownership — now written down |
| Milestone/estimation realism | 75 | 85 | M8 scope contradiction fixed (A+B); estimate corrected 13.5 → 14.5 eng-wks |

---

## Method

- **Pilot simulation (Task 17):** a senior-engineer agent was told "you execute
  M8 starting tomorrow," read the entire spec + every referenced console file,
  live-re-verified npm registry + basemaps + agent-browser, and reported day-1
  executability per task card, 22 author-questions, dependency/sequencing
  analysis, rework risks, and a confidence score.
- **QA gate audit (Task 18):** extracted every gate G-14..G-21 assertion,
  learned the v1 harness idiom from `run_g0/g5/g6` + `browser_map_test.sh`,
  probed actual tooling (agent-browser 0.35.0 capabilities, PIL/numpy
  availability), audited per-gate objectivity/automatability, regression
  strategy, P1–P12 traceability, and real-SITL coverage.
- **PM synthesis (main thread):** full spec read (both versions), worklog
  Tasks 1–16, scope/risk/roadmap judgment.

---

## Findings by role

### PM

- **Strong:** D5 "nothing dropped" is enforced by a §3.2 feature-home table;
  every milestone ships operable on `main` with a one-revert rollback; the
  estimate is stated with its assumptions; R1–R12 each carry a mitigation; the
  zero-backend-change scope keeps the blast radius console-only.
- **Found & fixed:** the plan's own parallelism claim was wrong — §13.1 said
  "M8 = stream A alone" while G-14 asserts B's zone furniture (zones A–G).
  Plan-of-record now reads A+B through M8 (estimate corrected), C from M9.
- **Noted, accepted:** M8 estimate optimism (2 → 2.5 wks) stems from B's
  furniture + gate instrumentation now being explicit — better honest than
  discover-in-flight.

### Senior Engineering (Task 17 — highlights)

- **Confirmed executable as-written:** T-A1 (deps/worker/basemap spine — all
  premises live-verified: `maplibre-gl@6.9.0` dist files, `setWorkerUrl`
  export, `finish-standalone.mjs` copies `public/`, prebuild hooks).
- **Blockers found, all fixed in v1.2:**
  1. `/?canvas=1` mechanism unspecified + `CanvasLoader.tsx`/`page.tsx`
     unowned — and `useSearchParams()` on the statically-prerendered v1 page
     is a known build failure. → **`/canvas` dedicated route, stream A owns,
     rationale documented (§4.1); `/legacy` likewise at M9.**
  2. Cross-stream contracts referenced 40+ times but never written: telemetry
     store API, FlushLoop↔HUD-DOM seam, app-store mock-vs-real split. →
     **§13.1 "M8 skeleton interfaces" (5 numbered contracts, incl. the
     `useHudRef` registry and the real `app-store.ts` at M8).**
  3. M8 scope contradiction (above).
  4. G-14 assertion methods (zones, ≥ 60 %, pixel sampling, offline flip,
     SwiftShader pass-through) — → **§12 measurement conventions +
     `__rsimMapDebug`/`__rsimTelemetry` instruments (§9.2) + T-D1 CDP spike.**
  5. P5/P8 normalizer timing ambiguity → `lib/normalize.ts` reads
     `attitude_q_wxyz` from day 1; HUD wiring stays M9.
- **Residual risks (owned, en-route):** react-map-gl-vs-vanilla decision is a
  1-day M8 spike with a **default of record** (vanilla) if the wrapper fights
  any §6.4/§7.2 pattern (R-7); `public/maplibre/` gitignore + Radix exact pins
  now specified (§9.3).

### QA (Task 18 — highlights)

- **Per-gate:** 8/8 objective, 0 blocked; G-18/G-19 READY as-written; G-14/
  15/16/17/20/21 NEEDS-WORK — every gap was a missing instrument or a wording
  flaw, not a plan defect. All closed in v1.2:
  1. `window.__rsimMapDebug` map-state hook (the WebGL successor to v1's DOM
     idiom — 5 of 8 gates depend on it) — specified at §9.2 + T-A2/T-A3 DoD.
  2. `framesIn`/`framesRendered`/`loadedAtMs` counters so G-20's "0 dropped
     frames" and "load ≤ 3 s" have instruments.
  3. Per-gate SOURCE column (§12): mock vs REAL SITL pinned — G-16/G-17 are
     REAL by need (flights), G-15 split MOCK verbs + REAL P5 leg; honors the
     project's real-SITL validation rule.
  4. G-15 gate-before-horse fixed (guard target = goto alt chip; param-search
     guard re-asserts at G-18).
  5. G-15 P5 "roll/pitch non-zero" flakiness fixed (assert quaternion delta +
     normalize.test.ts, not physics of a parked vehicle).
  6. SwiftShader launch path: agent-browser 0.35.0 has no launch-args flag →
     T-D1 spike must prove `--cdp` attach + PIL pixel-diff BEFORE G-14 is
     authored (R-2/§12).
- **Regression strategy verified sufficient** (a real finding of fact): the
  G-0..G-13 backend harnesses contain **zero browser driving** — the M8/M9
  route changes provably cannot break them; the three v1 browser scripts
  re-pointed at M9 and run unmodified are the regression proof. Two orphaned
  v1 UI budgets (G-6 select ≤ 100 ms, G-1 vertex-drag stress) re-homed into
  G-15/G-16.
- **P1–P12 traceability:** no orphan defects; three weak wordings strengthened
  (P5 gate cell, P6 real-SITL note, P10 build line in G-20).

---

## Amendment log — v1.1 → v1.2 (all applied in this commit)

| # | Change | Where |
|---|---|---|
| 1 | `/canvas` dedicated route + ownership + Next.js build-hazard rationale; `/legacy` at M9 | §4.1, §12 M8/M9, R-6 |
| 2 | M8 = streams A+B; "stream A alone" corrected | §13.1 |
| 3 | M8 skeleton interfaces: telemetry-store API, HUD ref registry, real app-store at M8, command-bus signature | §13.1 (new block) |
| 4 | Gate instruments `__rsimMapDebug` + `__rsimTelemetry` | §9.2 |
| 5 | Browser-gate measurement conventions (test-ids, bounding-box, PIL pixel-sample, CDP-attach) | §12 |
| 6 | Per-gate SOURCE column (REAL vs MOCK) | §12 |
| 7 | G-14 re-specified with concrete assertion methods | §12 |
| 8 | G-15: guard target + P5 quaternion-delta + select-latency budget | §12 |
| 9 | G-16 vertex-drag stress; G-20 P10 + decomposition; G-21 `__rsimMapDebug` observables | §12 |
| 10 | T-A2 gains L1/L2 + hook; T-A3 gains normalize/P5 timing + counters; T-B1 real store + disabled verbs; T-D1 WebGL spike + DOX/gitignore | §13.2 |
| 11 | Tokens additive+scoped (not re-authored at M8); mono = CSS stack only | §3.1, §5.1, §5.2 |
| 12 | Radix exact pins; `public/maplibre/` gitignored | §9.3 |
| 13 | R-2 (CDP mechanism of record), R-6 (verified immunity), R-7 (default of record) | §14 |
| 14 | Effort estimate 13.5 → 14.5 eng-wks, wall-clock ≈ 7 wks | §12 |

## Remaining open items (en-route, owned — not blockers)

| Item | Owner | Due |
|---|---|---|
| R-7 spike outcome (react-map-gl vs vanilla) | Stream A | M8 day 2 |
| T-D1 headless-WebGL spike (CDP + PIL) | Stream D | before G-14 authoring |
| G-20 §3.2 checklist runner decomposition detail | Stream D | M14 |
| QGC 5.1 hold-to-confirm for mode changes (candidate amendment, operator-feedback-gated) | PM | post-M15 |
| Light-mode token set, i18n beyond units, PWA | out of scope (documented) | — |

## Board recommendation

Greenlight **M8 kickoff** on spec v1.2. Sequence: T-D1 WebGL spike + T-A1
deps/worker on day 1 (they de-risk each other), skeleton interfaces land in
week 1, then T-A2/T-A3/T-B1 in parallel, G-14 authored only after the spike
proves the instrument path. The fleet stays parked until the G-14 session
(disk discipline, §13.3), and the first G-14 run is the real-SITL online
validation of the canvas — per project rule, on real vehicles, not mocks.
