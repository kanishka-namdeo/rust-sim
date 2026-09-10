# ADR-0030: SITL lifecycle is operator-driven (QGC/MP pattern)

- Status: Accepted
- Date: 2026-09-10
- Owning milestone: post-M7 lean-cleanup follow-on (the GCS v2 "Operations Canvas" + the SITL supervisor land together)
- Supersedes: none (refines the lifecycle assumption embedded in
  `scripts/stack_up.sh`'s pre-2026-09-10 `start` verb)
- Related: `scripts/stack_up.sh` (the persistent-stack launcher);
  `fleet/crates/fleet-cli/src/bin/supervisor.rs` (the supervisor binary);
  `console/src/components/canvas/overlays/SitlManagerPanel.tsx` (the
  GCS UI panel); `console/src/hooks/useSitlSupervisor.ts` (the hook);
  `docs/ARCHITECTURE.md` (port map now lists `:8500 supervisor`);
  `docs/SANDBOX_SETUP.md` (step 8 + the 2026-09-10 reval note)

## Context

QGroundControl and Mission Planner — the two open-source GCS
reference points RustSim GCS v1/v2 models — do NOT auto-spawn SITL when
the GCS launches. The operator starts SITL on demand:

- **QGC** — the operator runs PX4 SITL in a separate terminal, or
  clicks *Mock Link* / *Add Vehicle* in the application; the GCS
  attaches to whatever link is already alive.
- **Mission Planner** — the *Simulation* tab has explicit *Start SITL*
  / *Stop SITL* buttons; the rest of the GCS does not assume a vehicle
  is connected.

RustSim GCS pre-2026-09-10 inverted this pattern: `scripts/stack_up.sh
start` brought up the catalog (:8300) **and** the fleet manager
(:8400, spawning N PX4 SITL + sim pairs) in one go. That made the
"double-fork daemon survives the agent-shell process reaper" trick
(see `docs/SANDBOX_SETUP.md` reval 2026-09-09) carry the whole fleet's
process tree, with three consequences that conflicted with the QGC/MP
model and the Task-7a/7b lean cleanup:

1. **Wasted resources when nobody is flying.** A long-idle
   `hold_for_setup` fleet keeps 2× PX4 SITL processes + 2× sitsim
   processes + the manager ticking at 200 Hz virtual time, doing
   nothing. On the 2-vCPU Z.AI sandbox that pegged the CPU and made
   the console dev server sluggish.
2. **The cleanup removed the runtime control plane** (Task 7b:
   `PUT /api/fleet`, `POST /api/tasks`, `POST /api/vehicles/{i}/faults`,
   the auction allocator, the runner, the 8-policy ladder, the
   scenario DSL). The runtime hot-swap and runtime task append paths
   that the supervisor previously offered through :8400 are gone; the
   only remaining way to (re)start the fleet is the `mavfleet run`
   CLI or a fresh `stack_up.sh start-fleet`. The supervisor's process
   lifecycle needed a single owner.
3. **The GCS UI had no "start SITL" button.** The Operations Canvas
   (GCS v2) shipped a Settings panel, a Cheat Sheet, etc., but nothing
   to spawn the fleet. An operator who browsed the console saw an
   empty FleetFrame and had to drop to a terminal.

## Decision

The SITL lifecycle is operator-driven, mirroring QGC/MP:

1. **`scripts/stack_up.sh start` no longer auto-starts the fleet.** It
   brings up three planes only:
   - `:8300` catalog — the mission + ULog + preset REST server.
   - `:8500` supervisor — the new SITL lifecycle manager (this ADR).
   - `:3000` console — the GCS UI.
   No `mavfleet` process is spawned, no PX4 SITL pairs are spawned, no
   `:8400` fleet plane comes up. The console opens with an empty
   fleet, exactly like QGC opens with no vehicle attached.

2. **A new `fleet-supervisor` binary owns the mavfleet process
   lifecycle.** It listens on `:8500` and exposes a small REST API:
   - `GET  /api/sitl/status` — `{running, vehicle_count, pid,
     started_at_ms, run_dir, scenario}` (reads `:8400`'s fleet frame
     for `vehicle_count` when running).
   - `POST /api/sitl/start` — body `{scenario?: string}`; spawns
     `mavfleet run --fleet <scenario> --api-port 8400 --run-dir
     <ts>`; HTTP 409 if already running, 404 if the scenario TOML
     does not exist.
   - `POST /api/sitl/stop` — kills the mavfleet child + (via the
     manager's own teardown) its px4/sim grandchildren; idempotent.
   - `GET  /api/sitl/scenarios` — lists the scenario TOMLs in
     `fleet/tests/` plus the default name.
   - `GET  /api/health` — liveness probe.
   The supervisor is the single owner: the GCS UI, the CLI launcher
   (`stack_up.sh start-fleet`), and the stop-on-shutdown path
   (`stack_up.sh stop` calls `POST /api/sitl/stop` before tearing
   down the supervisor itself) all go through it.

3. **The GCS UI gets a "SITL Manager" overlay panel** in the
   Operations Canvas (LeftRail + the overlay stack). It shows
   RUNNING/STOPPED status, the running scenario, the started-at
   timestamp, the live vehicle count, and offers a hold-to-confirm
   *Start SITL* button (with a scenario dropdown populated from
   `GET /api/sitl/scenarios`) and a single-tap safety-positive *Stop
   SITL* button. The panel is the operator-facing counterpart of
   QGC's *Mock Link* button or Mission Planner's *Simulation* tab.
   The console overlay count goes from 6 (post-Task-7a) to 7:
   mission, library, fleet, **sitl**, setup, analyze, preflight,
   settings, cheat. The implementation lives in
   `console/src/components/canvas/overlays/SitlManagerPanel.tsx` +
   `console/src/hooks/useSitlSupervisor.ts` (1 Hz status poll when
   the panel is open).

4. **`scripts/stack_up.sh start-fleet` remains for CLI users.** It
   (re)launches just the fleet plane (:8400 + PX4 SITL pairs) by
   hitting the supervisor's `POST /api/sitl/start` — i.e. the same
   code path the UI uses. The CLI path exists so an operator who
   never opens the browser can still start SITL.

## Consequences

### Positive

- **Matches the QGC/MP lifecycle model.** The GCS launches cold; the
  operator starts SITL when they actually want to fly; closing the
  GCS does not have to mean killing the fleet (and vice versa).
- **Idle resources go to zero.** With the supervisor up but no
  mavfleet running, only the catalog + supervisor + console consume
  CPU/RAM; the per-vehicle PX4 SITL + sim pairs spawn on demand and
  tear down on stop.
- **Single owner for the mavfleet process.** The supervisor's
  AppState holds the `tokio::process::Child`; the UI's "Stop" button
  and `stack_up.sh stop-fleet` and `stack_up.sh stop` all reach the
  same `POST /api/sitl/stop` (or the supervisor's pid file as a
  fallback). No more races between the launcher's `kill_pidfile` and
  the manager's graceful self-teardown.
- **The CLI and the UI share the same code path.** `start-fleet` is
  a `curl POST /api/sitl/start` in a shell script; the UI's hold-to-
  confirm button is the same call from a browser. Drift between the
  two is structurally impossible.
- **The supervisor reports the live vehicle_count.** It does a
  `GET :8400/api/fleet` round-trip on every `/api/sitl/status` call,
  so the UI panel never shows a stale "RUNNING" while the manager
  has actually exited (a real failure mode on the 2-vCPU sandbox
  when PX4 boot is slow).

### Negative

- **The operator must explicitly start SITL.** An operator who
  opens the console and expects to see two vehicles (the pre-2026-09-10
  behavior) sees an empty FleetFrame instead. Mitigation: the SITL
  Manager panel is the first overlay in the LeftRail (when SITL is
  stopped) and the Fleet C2 overlay carries a "SITL stopped — start
  it from the SITL Manager panel" hint.
- **One more plane to bring up.** The persistent stack is now
  catalog + supervisor + console (was catalog + fleet + console).
  `stack_up.sh start` brings all three up; `start-fleet` adds the
  fleet on top. The 2-vCPU sandbox handles the three idle planes
  fine (verified).
- **The supervisor's status poll does a `:8400` round-trip when
  running.** 1 Hz from one browser tab is negligible; the supervisor
  falls back to `vehicle_count = 0` if `:8400` is unreachable (e.g.
  mid-teardown), so a transient `:8400` miss does not crash the UI.

### Neutral

- **The supervisor binary lives in `fleet/crates/fleet-cli/src/bin/`**
  alongside the `mavfleet` binary (both share the workspace; the
  supervisor's `Cargo.toml` declares `[[bin]] name = "fleet-supervisor"`).
  The console and the scripts reach it over `:8500` REST, not in-
  process — same as the catalog and the manager.
- **The `mavfleet run` CLI is unchanged.** It still works as a
  standalone one-shot (the integration harnesses `run_f1.sh` /
  `run_f2.sh` etc. invoke it directly). The supervisor is the
  persistent-stack entry point; the harnesses keep using the CLI.

## Alternatives considered

### Keep auto-spawn on `stack_up.sh start` (rejected)

Pre-2026-09-10 behavior. **Rejected** because it carries the whole
fleet's process tree on a stack the operator may not be flying,
conflicts with the QGC/MP lifecycle model, and makes the supervisor's
process-tree ownership ambiguous (the launcher's `kill_pidfile` and
the manager's graceful teardown race when `stack_up.sh stop` runs).

### Put the SITL Manager in the Fleet C2 overlay (rejected)

Re-use the Fleet C2 panel's bindings sub-panel. **Rejected** because
Fleet C2 is fleet-state UI (vehicle cards, events, e-stop) — putting
fleet *lifecycle* UI there conflates two concerns, and Fleet C2 is
only meaningful when the fleet is running. The SITL Manager is most
useful when the fleet is NOT running (that is the only state from
which "Start SITL" is actionable), so it has to live outside Fleet
C2.

### Auto-spawn SITL on first GCS UI open (rejected)

Spawn the fleet lazily when the operator first opens the console,
instead of eagerly at `stack_up.sh start`. **Rejected** because it
re-introduces the same surprise-vehicle-in-CPU problem at first
browser open, and because the supervisor would have to drive the
spawn from the server side anyway (the browser cannot fork
processes). The current design — explicit operator action through
the SITL Manager panel — is the QGC/MP model and the only one that
matches the operator's mental model.

### Fold the supervisor into the catalog (rejected)

Run the supervisor inside the `fleet-catalog` binary on `:8300`.
**Rejected** because the catalog is the mission + ULog + preset
server; lifecycle is a separate responsibility. Folding them
would force a single binary to own two unrelated REST planes, and
the supervisor needs to spawn `mavfleet` (a workspace sibling
binary) — that's a fleet-cli concern, not a fleet-mission concern.

## Verification

- **Manual (live, 2026-09-10):** `scripts/stack_up.sh start` brings
  up catalog + supervisor + console with no `:8400` fleet plane.
  `curl http://127.0.0.1:8500/api/sitl/status` returns
  `{"ok": true, "data": {"running": false, ...}}`. Opening the GCS
  in a browser shows the SITL Manager panel with "STOPPED". Clicking
  *Start SITL* spawns the fleet; the panel flips to "RUNNING · 2
  vehicles · scenario operator_session.toml" within ~10 s; the Fleet
  C2 overlay shows the two vehicles.
- **`scripts/stack_up.sh status`** prints the four-plane state
  (catalog :8300, supervisor :8500, fleet :8400, console :3000) plus
  the SITL lifecycle line (RUNNING/STOPPED, vehicle count, scenario,
  pid) read from the supervisor.
- **`scripts/stack_up.sh stop`** calls `POST /api/sitl/stop` (clean
  teardown via the supervisor), then tears down the supervisor +
  catalog + console, asserting all four ports go free.
- **Regression:** the integration harnesses (`run_i1.sh`,
  `run_i2_flight.sh`, `run_f1.sh`, `run_f2.sh`, `live_test_setup.sh`)
  keep using the `mavfleet` CLI directly — they are unchanged.

## References

- QGroundControl — *Mock Link* / *Add Vehicle* pattern (the GCS does
  not auto-spawn vehicles; the operator starts SITL in a separate
  terminal or attaches to an existing link).
- Mission Planner — *Simulation* tab with explicit Start/Stop SITL
  buttons.
- `scripts/stack_up.sh` (the persistent-stack launcher — the
  `start`, `start-fleet`, `stop-fleet`, `stop`, `status` verbs).
- `fleet/crates/fleet-cli/src/bin/supervisor.rs` (the supervisor
  binary; ~500 lines, `#![forbid(unsafe_code)]`).
- `console/src/components/canvas/overlays/SitlManagerPanel.tsx`
  (the GCS UI panel; hold-to-confirm Start, single-tap Stop).
- `console/src/hooks/useSitlSupervisor.ts` (the thin REST wrapper +
  1 Hz status poll).
- `docs/ARCHITECTURE.md` (port map now lists `:8500 supervisor`).
- `docs/SANDBOX_SETUP.md` (step 8 + the 2026-09-10 reval record).
- Task 7a/7b cleanup (worklog 2026-09-10) — the runtime control
  plane removal that motivated making the supervisor the single
  lifecycle owner.
