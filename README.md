<div align="center">

# RustSim GCS

**A QGroundControl / Mission Planner-class ground control station for PX4 SITL — pure Rust + TypeScript, real PX4, operator-driven SITL lifecycle (QGC/MP pattern).**

[![sim tests](https://img.shields.io/badge/sim-99%20tests-brightgreen)](sim)
[![fleet tests](https://img.shields.io/badge/fleet-285%20tests-brightgreen)](fleet)
[![G-ladder](https://img.shields.io/badge/G--ladder-G--0%20%E2%86%92%20G--13%20(11%20surviving)-brightgreen)](console/tests)
[![console](https://img.shields.io/badge/console-7%20overlay%20panels%2C%20lint%2Bbuild%20clean-blue)](console)
[![I-2](https://img.shields.io/badge/live--verified-I--1%20%2F%20I--2%20flight-success)](docs/VERIFICATION.md)
[![F-2](https://img.shields.io/badge/live--verified-F--1%20%2F%20F--2%20fleet-success)](docs/VERIFICATION.md)
[![O-2](https://img.shields.io/badge/live--verified-O--1%20%2F%20O--2%20operator--map-success)](docs/VERIFICATION.md)
[![SITL supervisor](https://img.shields.io/badge/:8500-supervisor%20(ADR--0030)-blueviolet)](console/docs/adr/0030-sitl-supervisor.md)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

</div>

---

RustSim GCS is a full **ground control station for PX4-Autopilot v1.16.2
SITL** — not a simulator with a console bolted on, but a QGC/MP-class
operator surface that happens to *be* the simulator. It grew out of a
lockstep HIL testbed, and the testbed is still here under the hood, but
what you sit in front of is the operator tool: a single-screen Operations
Canvas (full-bleed MapLibre GL map + edge-HUD widgets + right-click
context menus + single-key shortcuts) with 7 overlay panels for
mission authoring, fleet C2, SITL lifecycle, vehicle setup, ULog analyze,
pre-flight checks, and settings — all backed by the catalog (`:8300`),
the fleet-supervisor (`:8500`, the SITL lifecycle manager), the fleet
manager (`:8400`, spawned on demand), and the per-vehicle sim/`PX4`
HIL pairs.

**SITL is operator-driven (QGC/MP pattern, ADR-0030).** The GCS does
NOT auto-spawn SITL on launch — `scripts/stack_up.sh start` brings up
catalog + supervisor + console only; the operator starts SITL on demand
from the GCS UI's "SITL Manager" panel or via `scripts/stack_up.sh
start-fleet`.

The whole stack is built from five components, all in this repository:

| Component | What it is |
|---|---|
| **[sim/](sim)** — *RustSim Core* | A lockstep HIL flight simulator that replaces Gazebo/jMAVSim entirely: hand-rolled MAVLink v2 codec (golden-vectored against PX4's own headers), 6-DOF quadrotor dynamics, BMI088-class sensor models, a 10-fault injection engine, deterministic replays, REST+WS control plane on `:8200+i` per vehicle. Post-2026-09-10: the GCS UI no longer consumes the sim's internal data plane; the sim stays as PX4's HIL physics engine. |
| **[fleet/](fleet)** — *RustSim Fleet* + `fleet-mission` | A multi-vehicle mission manager: spawns sim+PX4 pairs on demand, drives offboard missions at a 20 Hz setpoint cadence, enforces geofence breach detection. The **`fleet-mission` crate** adds the GCS catalog backend (`:8300`) — mission file + preset persistence (ADR-0020), validation (ADR-0026), version check (ADR-0029), ULog browse. The **`fleet-supervisor` binary** (`:8500`) owns the SITL lifecycle (ADR-0030). The 2026-09-10 lean cleanup removed the auction allocator (Hungarian baseline), the 8-policy safety ladder, the scenario DSL/compiler/runner/report, the fault proxy, the runtime hot-swap/append routes, and the `mavfleet check` subcommand. |
| **[console/](console)** — *RustSim Console* | A Next.js 16 GCS — the **Operations Canvas**: a single full-bleed MapLibre GL map with edge-HUD widgets, right-click context menus, single-key shortcuts, and **7 overlay panels** (post-2026-09-10): mission, library, fleet, **sitl** (the SITL Manager — start/stop SITL on demand, ADR-0030), setup, analyze (ULog-only), preflight, settings, cheat. Settings includes the Map view options (6 basemap providers, orientation, 2D/3D projection, 7 layer toggles). All with honest LIVE/SIMULATED dual-mode. |
| **[docs/GCS_SPEC.md](docs/GCS_SPEC.md)** — *GCS v1 spec (historical)* | The engineering contract for the v1 buildout: 7 milestones, 14 verification gates (G-0 → G-13), 6 feature areas, full UX flow specs, market context, competitive landscape, and the risk register. The shipped console is now GCS v2 (Operations Canvas); this spec records the v1 design intent. ~1600 lines. |
| **[console/docs/adr/](console/docs/adr)** — *GCS ADRs* | The decision log for the GCS layer: 6 accepted ADRs in the 0019-0030 range — 0019 (mission file format), 0020 (persistence + atomic writes), 0026 (validation rules), 0027 (`:8300` server shape), 0029 (PX4 version policy), **0030 (SITL supervisor — operator-driven lifecycle)**. Additional drafts (0021-0025, 0028) in progress. |

**No Gazebo. No jmavsim. No external MAVLink crate. No database. No real
hardware.** v1 is unapologetically SITL-only: real FC flashing, serial
links, USB discovery, and on-field operations are explicitly out of scope
(see GCS_SPEC.md §3.2). The whole stack is Rust + TypeScript, speaks PX4's
native HIL TCP wire on `4560+i` and MAVLink UDP on `14540+i`, and every
protocol claim in the docs is backed by a live capture against real PX4.

![Fleet C2, live](docs/images/rustsim-fleetc2-live.png)

*The Fleet C2 console during a live F-2 run: two vehicles, each driven by
its own RustSim Core instance + a real PX4 SITL process, flying a
4-waypoint mission in OFFBOARD mode. (Historical F-2 record; the
2026-09-10 cleanup removed the auction allocator + the `run_f2.sh`
harness, but the underlying fleet bring-up + MAVLink-driven offboard
flight path is still exercised by `run_f1.sh` and the operator-driven
fleet start.)*

> **Screenshots.** The images under [docs/images/](docs/images/) cover the
> original I/F/S/O/R-console surfaces (Fleet C2, Operator Map, Vehicle
> Setup, Fly View, Plan View). Captures of the Operations Canvas
> (single-screen MapLibre GL map + 7 overlay panels) and the SITL
> Manager panel are being added as part of the post-cleanup DOC-Screenshots
> pass.

## Live-verified, not "should work"

Every headline claim of the simulator-and-fleet substrate has a
single-invocation harness and a recorded PASS
([docs/VERIFICATION.md](docs/VERIFICATION.md)):

- **I-1 — boot gate**: PX4 rcS completes on our HIL stream, EKF2 estimator
  output flows, heartbeats + loop-closure + ULog verified.
- **I-2 — physical flight**: arm → OFFBOARD climb → hover → land → disarm,
  proven from the simulator's replay ground truth (not the EKF's opinion).
- **F-1 — fleet bring-up**: N vehicles READY with live health; operator
  e-stop → ABORTED(2) with event log; clean port teardown.
- **Browser-live**: the operator console drives the same fleet through the
  preview gateway; LIVE badges, moving telemetry, screenshots captured.
- **S-1 / S-2 — vehicle setup (ADR-0016)**: REST- and browser-level passes
  of the QGC-style configuration tab — param download, typed writes, gyro
  calibration, mode switch, airframe apply Iris → Boat → Iris with
  controlled restarts, persistence across reboots.
- **O-1 / O-2 — operator map control (ADR-0017)**: REST- and browser-level
  passes of the map view where the user flies SITL themselves — go-to
  flights, fence-validated mission upload, guided arm/takeoff/land/RTL/hold,
  all against real PX4.
- **SITL lifecycle (ADR-0030, 2026-09-10)**: the GCS launches cold
  (`stack_up.sh start` = catalog :8300 + supervisor :8500 + console :3000,
  NO fleet); the operator starts SITL on demand from the GCS UI's SITL
  Manager panel or `stack_up.sh start-fleet`; the supervisor (:8500) owns
  the mavfleet process tree.

> **Note (2026-09-10 cleanup).** The R-1 runtime control plane
> (`PUT /api/fleet`, `POST /api/tasks`, `POST /api/vehicles/{i}/faults`,
> fault proxy, hot scenario load, task-append reallocation) and the
> G-10/G-12/G-19 GCS gates were removed end-to-end. The GCS no longer
> auto-spawns SITL on launch — see ADR-0030 for the operator-driven
> QGC/MP pattern. The F-2 auction harness was removed; the surviving
> fleet-coverage harness is `run_f1.sh` (bring-up + e-stop) plus the
> operator-driven fleet start path.

## GCS v1 verification ladder (G-0 through G-13, 11 surviving)

The G-ladder extends the existing I/F/S/O/R record above. Each gate is a
single-invocation harness under [console/tests/](console/tests), runs
against real PX4 SITL (or, for the pure-TS gates like G-13, against the
compiled pattern module), asserts PASS/FAIL with an exit code, and is
named with a `G-` prefix. Full table: GCS_SPEC.md §9. The 2026-09-10
lean cleanup deleted **G-10** (orchestration), **G-12** (replay scrub),
and **G-19** (analyze sim) — the table below keeps the historical row
labels but marks them DELETED so the G-numbering stays stable.

| Gate | Name | Harness |
|---|---|---|
| **G-0** | PX4 version check — `:8300` rejects upload to a vehicle reporting anything other than v1.16.2 (HTTP 426) | `console/tests/run_g0_version.sh` |
| **G-1** | Mission validation — schema + geofence geometry + rally containment; vertex-drag stress test (50+ vertices, no UI freeze — the QGC v5.1.4 bug class) | `run_g1_validation.sh` |
| **G-2** | Mission persistence — CRUD round-trip (create, list, fetch, update, delete); survives catalog restart | `run_g2_persistence.sh` |
| **G-3** | Mission upload — MAVLink mission protocol for all three `MAV_MISSION_TYPE` values; rollback on failure | `run_g3_upload.sh` |
| **G-4** | Mission download — round-trip equality after upload for all three types | `run_g4_download.sh` |
| **G-5** | Fly View 1-vehicle telemetry — 10 Hz telemetry moves on map + strip + attitude HUD for 60 s | `run_g5_flyview.sh` |
| **G-6** | Multi-vehicle Fly View — 2 vehicles on map simultaneously, select-active switch <100 ms | `run_g6_multivehicle.sh` |
| **G-7** | Pre-arm + arm/disarm — pre-arm checks run, block when failing, pass when fixed, arm→disarm round-trip | `run_g7_arm.sh` |
| **G-8** | Vehicle Setup extensions — param search filter, preset save/load round-trip, diff-against-defaults | `run_g8_setup_ext.sh` |
| **G-9** | Fleet mission binding — vehicle 0 flies mission A, vehicle 1 flies mission B, both correct | `run_g9_binding.sh` |
| ~~G-10~~ | **DELETED 2026-09-10** (orchestration + sequential-auction paths removed in cleanup) | — |
| **G-11** | ULog browse + plot — list `.ulg` files, list topics, fetch topic data, plot in chart (trimmed to ULog-only; replay scrub removed) | `run_g11_ulog.sh` |
| ~~G-12~~ | **DELETED 2026-09-10** (`.replay` scrub tab removed in cleanup) | — |
| **G-13** | Survey/corridor/perimeter patterns — generate each pattern on a known polygon; assert waypoint count, all-inside-polygon, no gaps > leg spacing | `run_g13_patterns.sh` |

## GCS v1 feature areas

Seven feature areas, each mapped to its milestone and verification gates
(full text in GCS_SPEC.md §5):

1. **Plan View** — mission editor with TOML-on-disk / JSON-on-wire
   persistence, schema + geofence + rally containment validation, the
   `:8300` catalog CRUD API, and the PX4 version gate. *Milestone M1,
   gates G-0, G-1, G-2.*
2. **Mission upload / download** — the MAVLink mission protocol
   (`MISSION_COUNT` → `MISSION_ITEM_INT` → `MISSION_ACK`) for all three
   `MAV_MISSION_TYPE` values (mission, fence, rally), with rollback on
   failure and round-trip-equality download. *Milestone M2, gates
   G-3, G-4.*
3. **Fly View** — instrument widgets (attitude HUD, strip charts, map),
   multi-vehicle active-select with <100 ms switch latency, pre-arm
   checklist that actually blocks arming. *Milestone M3, gates
   G-5, G-6, G-7.*
4. **Vehicle Setup extensions** — parameter search filter, preset
   save/load round-trip (ADR-0020), diff-against-defaults view.
   *Milestone M4, gate G-8.*
5. **Fleet mission orchestration** — per-vehicle mission binding,
   parallel and sequential execution modes. (Swarming patterns +
   the F-2 auction allocator were removed in the 2026-09-10 cleanup.)
   *Milestone M5, gate G-9.*
6. **Analyze View** — ULog browse + plot (server-side `pyulog` per
   ADR-0021, browser consumes JSON). The `.replay` scrub timeline was
   removed in the 2026-09-10 cleanup (custom sim format; ULog stays).
   *Milestone M6, gate G-11.*
7. **Survey Patterns** — survey grid, corridor, and perimeter generators
   in [console/src/lib/patterns.ts](console/src/lib/patterns.ts); all
   client-side, all geo-correct (haversine, point-in-polygon, all-inside
   assertions). *Milestone M7, gate G-13.*

## Architecture

```
┌─ Web deployment (browser) ──────────────────────────────────────────────────┐
│                                                                              │
│  browser ── Caddy gateway :81 ──┬─ :8300  RustSim Catalog (mission CRUD, ULog, presets)                       │
│                                 ├─ :8500  RustSim Supervisor (SITL lifecycle: start/stop mavfleet, ADR-0030) │
│                                 ├─ :3000  RustSim Console (Operations Canvas — Next.js standalone server)    │
│                                 └─ :8400  RustSim Fleet manager (spawned on-demand by :8500; MAVLink UDP 14540+i)            │
│              PX4 SITL v1.16.2 ◄── TCP 4560+i ┘ (HIL lockstep, 200 Hz)                                       │
└──────────────────────────────────────────────────────────────────────────────┘

┌─ Desktop deployment (Tauri 2.x) ─────────────────────────────────────────────┐
│                                                                              │
│  RustSim GCS.app / .exe / .AppImage                                          │
│    ├─ Tauri webview (WebKitGTK / WebView2 / WKWebView)                        │
│    │    loads static export from console/out/ (output: 'export')             │
│    │    talks direct to http://127.0.0.1:{8300,8400,8500} (no gateway)       │
│    └─ Tauri Rust backend (src-tauri/)                                        │
│         spawns fleet-catalog (:8300) + fleet-supervisor (:8500) at startup   │
│         as tokio::process::Command children with kill_on_drop(true)          │
│         graceful_shutdown() on window close → POST /api/sitl/stop → kill     │
│                                                                              │
│  PX4 SITL v1.16.2 — installed separately (not bundled); discovered via      │
│  PX4_ROOT env or ../PX4-Autopilot. Supervisor spawns mavfleet + px4 on demand │
│  when operator clicks Start in the SITL Manager panel.                       │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Two deployment paths, one codebase:**
- **Web** (`scripts/stack_up.sh start`): catalog + supervisor + Next.js console behind Caddy `:81`. Browse from any machine on the LAN. SITL stays operator-driven.
- **Desktop** (`cargo tauri build` → `.deb`/`.AppImage`/`.dmg`/`.msi`): a single double-clickable app that spawns its own catalog + supervisor + webview. No Caddy, no Node server, no terminal. SITL lifecycle identical (supervisor on `:8500`).

SITL is operator-driven in both paths (QGC/MP pattern, ADR-0030): the GCS launches cold; the operator starts SITL from the SITL Manager panel or `stack_up.sh start-fleet`.

Full port map and data flows: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
GCS-specific topology and API contracts: [docs/GCS_SPEC.md](docs/GCS_SPEC.md) §4 and §7.
Tauri repurpose spec + 8-milestone plan: [docs/TAURI_APP_SPEC.md](docs/TAURI_APP_SPEC.md).

## Quickstart

Prerequisites: Rust stable, Node 20+, Python 3.12+ (`pymavlink`, `pyulog`),
and PX4-Autopilot v1.16.2 built once:

```bash
git clone --depth 1 --branch v1.16.2 --recurse-submodules --shallow-submodules \
    https://github.com/PX4/PX4-Autopilot.git ../PX4-Autopilot
(cd ../PX4-Autopilot && make px4_sitl_default)

(cd sim     && cargo build --workspace)
(cd fleet   && cargo build --workspace)      # builds fleet-catalog + fleet-supervisor too
(cd console && npm install && npm run build)
```

> Setting this up in the Z.AI sandbox (or a lean container without Rust /
> cmake / a full npm registry)? Follow
> [docs/SANDBOX_SETUP.md](docs/SANDBOX_SETUP.md) — the exact verified
> sequence, including every environment-specific fix.

Start the persistent operator stack (the QGC/MP pattern: GCS launches
cold, the operator starts SITL on demand):

```bash
bash scripts/stack_up.sh start
# brings up: catalog :8300, supervisor :8500, console :3000
# NO fleet — SITL is NOT auto-spawned.
# Browse http://localhost:81 (via the Caddy gateway)
# Start SITL from the GCS UI's SITL Manager panel, or:
bash scripts/stack_up.sh start-fleet   # CLI: spawn the fleet (:8400 + PX4 SITL)
```

Verify the supervisor is up:

```bash
curl http://127.0.0.1:8500/api/sitl/status
# {"ok":true,"data":{"running":false,"vehicle_count":0,...}}
```

### Desktop app (Tauri 2.x) — alternative to the web stack

Build a single double-clickable desktop installer (no Caddy, no Node server,
no terminal needed by the operator):

```bash
cd console && npm install --no-audit --no-fund @tauri-apps/cli@^2 @tauri-apps/api@^2
cd src-tauri && cargo tauri build --bundles deb,appimage
# → src-tauri/target/release/bundle/deb/RustSim GCS_1.0.0_amd64.deb
# → src-tauri/target/release/bundle/appimage/RustSim GCS_1.0.0_amd64.AppImage
```

Install the `.deb` (`sudo apt install ./RustSim\ GCS_1.0.0_amd64.deb`) or
`chmod +x` the AppImage + double-click. The Tauri binary spawns
`fleet-catalog` + `fleet-supervisor` at startup and kills them on window
close (`graceful_shutdown()`). SITL stays operator-driven — click Start
in the SITL Manager panel, same as the web stack.

> Tauri build needs the same system deps as the web stack (`libwebkit2gtk-4.1-dev`,
> `libgtk-3-dev`, `librsvg2-dev`, etc. on Debian/Ubuntu). PX4-Autopilot
> v1.16.2 is NOT bundled — install it separately per the Quickstart above;
> the Tauri binary discovers it via `PX4_ROOT` env or `../PX4-Autopilot`.
> See [docs/TAURI_APP_SPEC.md](docs/TAURI_APP_SPEC.md) for the full spec
> + 8-milestone verification record.

Fly one vehicle, for real (single-invocation integration harness —
spawns + asserts + tears down in one shell call, no persistent stack
needed):

```bash
(cd sim && bash tests/run_i2_flight.sh)      # ~3 min, expects "I-2 PASS"
(cd fleet && bash tests/run_f1.sh)          # ~2 min, fleet bring-up + e-stop
```

Run the G-ladder (any subset, or all 11 surviving gates):

```bash
bash console/tests/run_g0_version.sh         # ~10 s — PX4 version gate
bash console/tests/run_g1_validation.sh      # ~30 s — mission validation
bash console/tests/run_g2_persistence.sh     # ~30 s — mission CRUD
bash console/tests/run_g3_upload.sh          # ~1 min — MAVLink mission upload
bash console/tests/run_g4_download.sh        # ~1 min — MAVLink mission download
bash console/tests/run_g5_flyview.sh          # ~1.5 min — Fly View telemetry
bash console/tests/run_g6_multivehicle.sh    # ~2 min — multi-vehicle Fly View
bash console/tests/run_g7_arm.sh              # ~2 min — pre-arm + arm/disarm
bash console/tests/run_g8_setup_ext.sh       # ~2 min — Vehicle Setup extensions
bash console/tests/run_g9_binding.sh          # ~3 min — fleet mission binding
bash console/tests/run_g11_ulog.sh           # ~1 min — ULog browse + plot
bash console/tests/run_g13_patterns.sh       # ~1 min — survey patterns (pure TS)
# G-10 / G-12 / G-19 were DELETED in the 2026-09-10 cleanup.
```

Manual tour: `docs/OPERATIONS.md` — raw control planes, the operator
driven SITL lifecycle, the demo scenario, environment knobs.

## Protocol highlights (the hard-won ones)

PX4's pinned v1.16.2 dialect diverges from the official MAVLink common.xml in
ways that only live capture reveals — all documented with evidence in
[sim/docs/PROTOCOL.md](sim/docs/PROTOCOL.md) and the ADRs:

- `HIL_ACTUATOR_CONTROLS` is packed with **size-sorted core fields**
  (`flags@8, controls@16, mode@80`), not the official extension layout
  ([ADR-0015](sim/docs/adr/0015-px4-v116-actuator-layout.md)).
- PX4 v1.16 sends **per-motor normalized thrust [0,1]**, not the jMAVSim-era
  [-1,1] ([ADR-0011](sim/docs/adr/0011-hil-actuator-pwm-mapping.md)).
- A **noiseless IMU or mag starves EKF2** and blocks arming — sensor noise is
  a correctness feature ([ADR-0012](sim/docs/adr/0012-noiseless-mag-starves-ekf2.md),
  [ADR-0014](sim/docs/adr/0014-realistic-sensor-noise-determinism.md)).
- `DO_SET_MODE` takes decomposed param2/param3, not the packed mode word
  ([ADR-0010](fleet/docs/adr/0010-do-set-mode-param-layout.md)).
- Grounded, co-located vehicles must not trigger vertical-separation
  overrides — they can't comply, and the hijacked goal stream pins the fleet
  on the ground ([ADR-0011](fleet/docs/adr/0011-grounded-separation-gate.md)).
- `PARAM_VALUE` / `PARAM_SET` wire order is generated-header order
  (value f32 @0 ...), not XML declaration order, and PX4 v1.16's param
  protocol is **typed** — INT32 params travel as their bit pattern inside
  the f32 field and `param_type` carries the v2 dialect wire constants
  (REAL32 = 9, INT32 = 6). QGC-style writes must be typed or PX4 rejects
  them ([ADR-0016](fleet/docs/adr/0016-vehicle-setup-control-plane.md)).
- PX4's param autosave is deferred ~300 ms — an immediate post-write reboot
  races the flash and boots the old airframe; the apply-restart path waits
  it out ([ADR-0016](fleet/docs/adr/0016-vehicle-setup-control-plane.md)).
- PX4 v1.16 battery params carry the `BAT1_` instance prefix
  (`BAT1_N_CELLS`, `BAT1_V_EMPTY`, ...) — the unprefixed `BAT_*` ids of
  earlier versions do not exist.

## Repository layout

```
sim/                          RustSim Core   — 8 crates, docs/ (SPEC, PROTOCOL, ADRs), live harnesses
fleet/                        RustSim Fleet  — 7 crates incl. fleet-mission (GCS catalog :8300),
                                              + fleet-supervisor binary (SITL lifecycle :8500, ADR-0030),
                                              docs/ (SPEC, SCENARIOS, ADRs), live harnesses
console/                      RustSim Console — Next.js 16 app, Operations Canvas + 7 overlay panels,
                                              dual API routing incl. :8300 (catalog) + :8500 (supervisor)
                                              (M-T1: output: 'export' for Tauri static bundling)
  src/components/canvas/      MapLibre GL canvas + overlays (SitlManagerPanel, FleetC2Panel,
                                MissionStrip, LibraryPanel, SetupDrawer, AnalyzeOverlay,
                                PreFlightPanel, SettingsPanel, OnboardingTour)
  src/state/map-settings.ts   Map view options (6 basemaps, orientation, projection, 7 layer toggles)
  src/lib/patterns.ts         Survey grid / corridor / perimeter generators (M7, ADR-0028)
  tests/                      G-ladder harnesses (11 surviving: G-0..G-9, G-11, G-13) + mock PX4
  docs/adr/                   GCS ADRs 0019-0030 (6 accepted: 0019, 0020, 0026, 0027, 0029, 0030)
src-tauri/                    Tauri 2.x desktop shell (M-T2..M-T7) — spawns catalog + supervisor
                              as tokio::process children, graceful_shutdown on window close.
  src/{main,backends,commands,shutdown,log_pipe}.rs   the Rust lifecycle manager
  tauri.conf.json             window config (1280×800), CSP (allows 127.0.0.1:{8300,8400,8500}),
                              bundle targets (deb/rpm/appimage for Linux, dmg for macOS, msi/nsis for Win)
  capabilities/main.json      Tauri 2 capabilities (core + 7 plugins)
docs/                         architecture, GCS v1/v2 spec, verification record, operations
                              runbook, sandbox setup, deployment guide, evidence images,
                              Tauri repurpose spec + M-T1..M-T8 verification records
  GCS_SPEC.md                the v1 engineering spec (7 milestones, 14 gates, 6 feature areas)
  GCS_V2_SPEC.md             the v2 Operations Canvas redesign spec (M8..M15, G-14..G-21)
  TAURI_APP_SPEC.md          the Tauri repurpose spec (8 milestones M-T1..M-T8, appendices C–Q)
  VERIFICATION.md             the I/F/S/O/R + G ladder evidence record
  MT5_VERIFICATION.md         M-T5 SITL lifecycle end-to-end (2 vehicles READY, 10 Hz WS telemetry)
  MT6_VERIFICATION.md         M-T6 graceful shutdown (zero orphans, 7/7 ports released)
  MT7_VERIFICATION.md         M-T7 cross-platform packaging (.deb + AppImage on Linux; CI matrix for macOS/Win)
  MT8_VERIFICATION.md         M-T8 screenshot verification (BLOCKED by WebKitGTK 2.52 wedge in headless container)
  images/                     live screenshots (Fleet C2, Operator Map, Vehicle Setup, Fly/Plan View)
scripts/                      cross-repo live tests (browser), golden-vector generator,
                              stack_up.sh persistent-stack launcher (catalog + supervisor + console
                              on `start`; fleet on `start-fleet`; ADR-0030)
fleet/crates/fleet-mission/    the catalog crate — `src/gcs/` (mission_file, store, validation,
                              version_check, server, ulog, preset), `fleet-catalog` binary
fleet/crates/fleet-cli/src/bin/supervisor.rs   the fleet-supervisor binary (SITL lifecycle :8500)
```

Each component carries its own `AGENTS.md` (the [DOX](https://github.com/agent0ai/dox)
contract hierarchy): the root [AGENTS.md](AGENTS.md) is the rail, and
`sim/`, `fleet/`, `console/`, `docs/`, `scripts/` own their local contracts.

## Development

```bash
(cd sim     && cargo test --workspace)   # 99 tests
(cd fleet   && cargo test --workspace)   # 285 tests (incl. fleet-mission catalog suite)
(cd console && npm run lint && npm run build)
```

G-ladder gates (single-invocation, run any subset — 11 surviving after
the 2026-09-10 cleanup):

```bash
for g in console/tests/run_g*.sh; do bash "$g"; done   # 11 surviving, ~22 min total
```

Integration harnesses are single-invocation by design (start → assert →
teardown in one shell call); do not split them, and do not shrink the
measured time budgets without re-running the corresponding live case.

## Roadmap

### Shipped in v1

- ✅ **Survey patterns** (M7 / G-13) — survey grid, corridor, and perimeter
  generators in `console/src/lib/patterns.ts`.
- ✅ **Offboard mission commands via MAVLink mission protocol** (M2 / G-3,
  G-4) — `MISSION_COUNT` → `MISSION_ITEM_INT` → `MISSION_ACK` for all
  three `MAV_MISSION_TYPE` values, replacing the position-only offboard
  setpoint stream with the FC-side mission protocol as an alternative
  flight path.
- ✅ **Plan View, Fly View, Analyze View, Fleet C2 orchestration,
  Vehicle Setup extensions** — see the *GCS v1 feature areas* above.

### Shipped — Tauri desktop app (M-T1..M-T7, 2026-09-11)

- ✅ **M-T1** — Next.js `output: 'export'` static export (no Node server
  in production; vendored Geist TTF fonts; favicon pinned to `/logo.svg`).
- ✅ **M-T2** — Tauri 2.x skeleton: `src-tauri/` with `tauri.conf.json`
  (1280×800 window, CSP allowing `http://127.0.0.1:{8300,8400,8500}`),
  7 plugins, 6 placeholder icons.
- ✅ **M-T3** — MapLibre worker URL verified for all 4 runtime environments
  (Next dev, web stack `:81`, Tauri Linux/Win, Tauri macOS) + runtime
  fetch probe for clear error reporting.
- ✅ **M-T4** — Rust backend orchestration: `tokio::process::Command` +
  `kill_on_drop(true)` for `fleet-catalog` + `fleet-supervisor`; 6 IPC
  commands; `graceful_shutdown()` on window close.
- ✅ **M-T5** — SITL lifecycle end-to-end: `POST /api/sitl/start` → 2
  vehicles READY → 10 Hz WebSocket telemetry → `POST /api/sitl/stop` →
  all 7 ports released, no orphans.
- ✅ **M-T6** — Graceful shutdown verified: zero orphan processes, 7/7
  ports released, idempotent (handles SITL-not-running edge case).
- ✅ **M-T7** — Cross-platform packaging: `.deb` (4.5 MB) + `.AppImage`
  (104 MB) produced on Linux. macOS `.dmg` + Windows `.msi` deferred
  to CI matrix on real OSes.
- ⚠️ **M-T8** — Screenshot verification: BLOCKED by WebKitGTK 2.52 wedge
  in headless container. The Tauri binary launches + `main()` runs +
  bridge setup works, but the webview init hangs Xvfb before `setup()`
  fires. Real-hardware screenshot capture deferred to CI matrix.

### Planned for v1.1

- **QGC `.plan` import / export** — cross-compatible mission file exchange
  with QGroundControl's JSON format (ADR-0019 chose TOML for the on-disk
  RustSim format; a `.plan` bridge is the natural v1.1 extension).
- **Offline map tiles** — currently the Operator Map and Fly View use
  Leaflet's default online tile layer; an offline-tile bundle is needed
  for air-gapped operations and for reproducible test environments
  (ADR-0022 draft).
- **Real PX4 v1.18 support** — v1.16.2 is the pinned and verified version;
  v1.18 brings dialect drift and is currently rejected at the gate (G-0,
  ADR-0029). v1.1 will re-validate against v1.18 stable.
- **Sequential gate polling** — the G-ladder harnesses currently run
  independently; a CI-mode runner that polls each gate on a smoke trigger
  and aggregates the report is needed for production CI.
- **CI matrix across PX4 versions** to track dialect drift (now blocked on
  v1.18 support above).

## Running it on your own machine

[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) is the guide: prerequisites, the
one-host quickstart (the persistent operator stack via `scripts/stack_up.sh
start` — catalog + supervisor + console; then `start-fleet` or the GCS
UI's SITL Manager panel to spawn SITL), browser access from other machines
(the Caddy gateway on `:81` is the single network entry point; SSH tunnels
for remote access), the port map (`:8300` catalog, `:8500` supervisor,
`:8400` fleet (on-demand), `:3000` console, `:81` gateway, `:8200+i` sim,
`4560+i` HIL TCP, `14540+i` MAVLink UDP), and an always-on service sketch.

## License

Apache-2.0 — see [LICENSE](LICENSE). PX4-Autopilot is BSD-licensed and is a
build-time external dependency, not vendored here.
