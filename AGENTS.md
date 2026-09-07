# RustSim — root AGENTS.md (DOX rail)

## Purpose

RustSim (`rust-sim`) is a unified PX4-native simulation and fleet-operations
testbed written in Rust with a live operator console:

- `sim/` — **RustSim Core** (rustsitsim): a lockstep HIL flight simulator that
  drives real, unmodified PX4 SITL over the native MAVLink TCP wire (no
  Gazebo, no jmavsim): 6-DOF quadrotor dynamics, BMI088-class sensor models,
  a 10-fault injection engine, deterministic replays, and a REST+WS control
  plane.
- `fleet/` — **RustSim Fleet** (mavfleet): a multi-vehicle mission manager
  that spawns sim+PX4 pairs, allocates tasks by sequential auction, flies
  offboard missions, and enforces an 8-policy safety ladder.
- `console/` — **RustSim Console**: a Next.js single page with two live
  consoles (Sim Console on `:8200`, Fleet C2 on `:8400`) with dual
  LIVE/SIMULATED modes.

Everything in this repo is verified live against real PX4-Autopilot v1.16.2
(see `docs/VERIFICATION.md`).

## Ownership

- Repo-level: branding, cross-component contracts (ports, scenarios, gateway
  routing), release notes, CI, this DOX tree.
- Component-level: owned by each child's AGENTS.md (`sim/`, `fleet/`,
  `console/`, `docs/`, `scripts/`).

## Local Contracts

- License: Apache-2.0 (all components).
- Rust: workspace-per-component (`sim/Cargo.toml`, `fleet/Cargo.toml`);
  `#![forbid(unsafe_code)]` in library crates; unit tests colocated.
- Console: TypeScript/Next.js 16 app router, Tailwind v4, shadcn-style local
  components; no database.
- Ports are part of the contract (see `docs/ARCHITECTURE.md`): HIL TCP
  4560+i, sitsim control plane 8200+i, fleet control plane 8400, telemetry
  UDP 14540+i, onboard UDP 14580+i.
- Commit style: conventional commits; every protocol/behavior change needs an
  ADR in the owning component (`sim/docs/adr/`, `fleet/docs/adr/`).
- No secrets in the repo; `.env` files are gitignored.

## Work Guidance

- Read the nearest AGENTS.md before editing a subtree; ADRs before touching
  protocol code.
- Setting the stack up in the Z.AI sandbox (or any lean container): follow
  `docs/SANDBOX_SETUP.md` — the exact verified sequence, including the
  environment-specific fixes (Rust/cmake installs, radix registry pins,
  PX4 shallow-clone tag fetch, author-layout symlink). Do not improvise
  around it; every deviation there was hit live.
- Integration work happens in single-invocation harnesses (background
  processes do not survive between shell calls on some platforms): see
  `sim/tests/run_i1.sh`, `sim/tests/run_i2_flight.sh`, `fleet/tests/run_f1.sh`,
  `fleet/tests/run_f2.sh`, `scripts/browser_live_test.sh`.
- New protocol facts discovered against real PX4 go into the owning
  PROTOCOL.md/ADR with live-capture evidence, not into code comments alone.

## Verification

- `cargo test --workspace` green in both `sim/` (99+ tests) and `fleet/`
  (126+ tests).
- Live harnesses: I-1 (PX4 boot gate), I-2 (physical flight, direct wire),
  F-1 (fleet bring-up + estop), F-2 (fleet flies with real dynamics),
  browser-live (operator console end-to-end through the preview gateway).
- Console: `npm run lint` + `npm run build` clean; `scripts/browser_live_test.sh`
  PASS requires LIVE badges on both consoles and moving telemetry.

## Child DOX Index

| Child | Scope |
|-------|-------|
| `sim/AGENTS.md` | RustSim Core workspace: physics, sensors, MAVLink HIL codec, replay, fault engine, control plane |
| `fleet/AGENTS.md` | RustSim Fleet workspace: mission manager, links, allocator, safety policies, fleet control plane |
| `console/AGENTS.md` | Operator console: pages, hooks, gateway/direct API routing, build & run |
| `docs/AGENTS.md` | Cross-repo documentation: architecture, verification evidence, operations runbook, sandbox setup sequence |
| `scripts/AGENTS.md` | Shared cross-repo scripts: live browser test, golden-vector generator, build helpers |
