# console/ — RustSim operator console (GCS v2 Operations Canvas)

## Purpose

Single-page Next.js operator console — the **Operations Canvas** — that
mounts the post-2026-09-10 set of 7 overlay panels (kept mounted via
single-overlay-mode toggles in `app-store.ts` so each telemetry engine
survives panel switches). The overlays map to GCS v2's feature areas
(`docs/GCS_V2_SPEC.md` §4 + §5; the v1 `docs/GCS_SPEC.md` is kept as a
Historical record):

- **Mission Strip** (`MissionStrip.tsx`) — mission editor + geofence
  drawing + validation + save/upload (GCS_SPEC §5.1, §8.1). Backed by
  the `:8300` catalog.
- **Library** (`LibraryPanel.tsx`) — mission catalog browser +
  recall; backed by `:8300`.
- **Fleet C2** (`FleetC2Panel.tsx`) — mavfleet plane on `:8400` —
  fleet table (Cards / Bindings / Events tabs; the swarming-patterns
  dropdown was removed 2026-09-10), e-stop.
- **SITL Manager** (`SitlManagerPanel.tsx`) — the `:8500` supervisor
  plane — operator-facing entry to start/stop SITL (QGC/MP pattern,
  ADR-0030; new 2026-09-10). Replaces the removed Sim Console / Sim
  Control panel (M13, removed 2026-09-10).
- **Setup Drawer** (`SetupDrawer.tsx`) — the ADR-0016 vehicle-setup
  workflow on `:8400` — airframe catalog, sensor calibration triggers,
  power/safety params, flight modes, the live param editor.
- **Analyze** (`AnalyzeOverlay.tsx`) — ULog browser + topic plotting
  (the `.replay` scrub tab was removed 2026-09-10). Backed by `:8300`.
- **Pre-Flight** (`PreFlightPanel.tsx`) — pre-arm checklist +
  prearm-check REST surface on `:8400`.
- **Settings** (`SettingsPanel.tsx`) — map view options (basemap
  provider, North/Track-up orientation, 2D/3D pitch projection, 7
  layer-visibility toggles; new `state/map-settings.ts` module, added
  2026-09-10) + PX4 version policy (ADR-0029) + about page.
- **Cheat Sheet** — keyboard shortcuts reference dialog (opened via
  the left-rail button or `?`).

The telemetry engines run a LIVE/SIMULATED dual mode: if the Rust
backend answers through the gateway, everything is live; otherwise a
client-side mock engine keeps the UI operable and the live endpoint is
re-probed every 12 s.

## Ownership

Owned here: pages/components/hooks/libs/state under `src/`. Concretely:

- The overlay panels: `src/components/canvas/overlays/{MissionStrip,
  LibraryPanel,FleetC2Panel,SitlManagerPanel,SetupDrawer,
  AnalyzeOverlay,PreFlightPanel,SettingsPanel}.tsx` +
  `OnboardingTour.tsx` + `GotoConfirmChip.tsx`.
- The canvas skeleton: `OperationsCanvas.tsx` (zone A–H grid per
  GCS_V2_SPEC §4), `MapCanvas.tsx` (full-bleed MapLibre), edge
  furniture (`strip/StatusStrip.tsx`, `rail/LeftRail.tsx`,
  `column/TelemetryColumn.tsx`, `bar/CommandBar.tsx`,
  `notify/NotificationStack.tsx`), `FlushLoop.tsx` (single rAF flush),
  `ShortcutsProvider.tsx` (keyboard + cheat-sheet dialog).
- The map view stack: `components/canvas/map/{MapCanvas,camera,layers,
  context-menu}` + `state/map-settings.ts` (basemap + orientation +
  projection + layer-visibility, added 2026-09-10).
- The hooks: `usePlanCatalog` (Plan ↔ `:8300`), `useFleetC2` (Fleet C2
  table + WS), `useSitlSupervisor` (`:8500` SITL lifecycle — new
  2026-09-10; verbs `sitl_start`, `sitl_stop`), `useVehicleSetup`
  (Setup Drawer engines, ADR-0016), `useAnalyze` (Analyze ULog
  streaming). (The removed `useSimConsole`, `useFlyView`,
  `useOperatorMap` hooks were trimmed 2026-09-10 — their consumed
  types (`SimFrame`, `ActiveFault`, `FAULT_CATALOG`, `SwarmPattern`,
  `Replay*`) were removed with them.)
- The tolerant protocol normalizers (`src/lib/conn.ts`) and the
  dual-mode lifecycle. The GCS v1 client-side helpers:
  `src/lib/plan-types.ts` (MissionFile TS mirror of ADR-0019's Rust
  struct).
- The mock engines (`src/lib/mock-fleet.ts`, `mock-setup.ts`) for
  offline demo; `mock-sim.ts` was removed 2026-09-10 (the GCS no
  longer consumes the sim's data plane).

Not owned here: the backend schemas (sim SPEC §4, fleet spec §3.4 —
the console normalizes tolerantly but does not redefine them); the
gateway itself (see `Caddyfile.example`); the `:8300` catalog server
itself (`fleet/crates/fleet-mission/src/gcs/`, see
`../fleet/AGENTS.md`); the `:8500` fleet-supervisor binary
(`fleet/crates/fleet-cli/src/bin/supervisor.rs`, see `../fleet/AGENTS.md`);
the GCS v2 spec (`../docs/GCS_V2_SPEC.md` owns scope, feature areas,
G-ladder, and milestone roadmap); ADR-0030 (operator-driven SITL
lifecycle) lives in `console/docs/adr/0030-sitl-supervisor.md`.

## Local Contracts

- No database, no auth, no server state: the console is a pure client
  of the four Rust control planes:
  - `:8200+i` sim — kept as PX4's HIL physics engine; the GCS UI no
    longer consumes its internal data plane (the per-vehicle sim
    socket ladder was removed 2026-09-10).
  - `:8300` catalog — mission CRUD + ULog browse + param presets.
  - `:8400` fleet — spawned on-demand by the supervisor; telemetry
    WS + arm/land/rtl/hold/goto + mission upload/download + prearm
    checks + QGC-style Vehicle Setup + fleet mission bindings +
    e-stop.
  - `:8500` supervisor — SITL lifecycle (`POST /api/sitl/start`,
    `POST /api/sitl/stop`, `GET /api/sitl/status`,
    `GET /api/sitl/scenarios`). The supervisor owns the `mavfleet`
    child process (ADR-0030).
- API routing styles (see `src/lib/conn.ts`):
  - `gateway` (default): relative fetches + `?XTransformPort=<port>`,
    WS at `/?XTransformPort=<port>` — used behind the Caddy gateway
    (`Caddyfile.example`) and preview proxies. The `port` argument
    selects the backend: `8200+i` (sim, UI no longer consumes),
    `8300` (catalog), `8400` (fleet), `8500` (supervisor). Plan,
    Analyze, and Library use `?XTransformPort=8300`; Fleet C2 +
    Setup Drawer use `8400`; SITL Manager uses `8500`.
  - `direct` (`NEXT_PUBLIC_RSIM_API_STYLE=direct`): absolute
    `http://127.0.0.1:<port>` REST + `ws://127.0.0.1:<port>/` for
    local runs without the gateway.
- Overlay panels use single-overlay-mode (opening a new overlay
  closes the others; see `toggleOverlay` in `app-store.ts`).
  MapLibre (the full-bleed canvas) initializes inside a full-size
  container; camera transitions are eased (state/map-settings.ts
  orientation + projection); `fitBounds` is always called against a
  non-zero cached map size.
- Geo conversion is the TS port of the Rust `GeoOrigin`
  (`src/lib/geo.ts`, ECEF + Bowring, same numbers as the manager) —
  never ad-hoc linear math.
- Operator commands are thin REST POSTs to the ADR-0017 plane on
  `:8400` (`/api/mission{,/start,/clear}`,
  `/api/vehicles/{i}/{arm,takeoff,land,rtl,hold,goto}`,
  `/api/vehicles/{i}/{mission/upload,mission/start,prearm-checks,
  params,params/refresh,airframe,calibrate,mode,setup}`,
  `/api/fleet/{start,mission-bindings,estop}`,
  `/api/airframes`, `/api/modes`) and to the ADR-0030 supervisor on
  `:8500` (`/api/sitl/{start,stop,status,scenarios}`). The mock
  engine implements the same semantics (fence validation, op* ids,
  go-to flight) so SIMULATED mode stays honest. (The removed
  `PUT /api/fleet`, `POST /api/tasks`, `POST /api/vehicles/{i}/faults`,
  `GET /api/fleet/patterns*`, `GET /api/replays*` routes — and the
  fault/swarm/replay command verbs — were trimmed 2026-09-10.)
- Dependencies are trimmed to what `src/` actually imports (radix
  tabs/select/label/progress/scroll-area/alert-dialog/toast/slot,
  lucide, next-themes, **maplibre-gl** + `@types/maplibre-gl`, cn
  util, Tailwind v4). No prisma, no template cruft, no react-leaflet
  wrapper, no Leaflet (MapLibre replaced Leaflet in GCS v2).

## Work Guidance

- `npm install && npm run build && npm start` (or `npm run dev`). Port 3000.
- For the gateway mode, run Caddy with `Caddyfile.example` and open :81.
  The persistent stack (catalog :8300 + supervisor :8500 + console
  :3000) is started by `../scripts/stack_up.sh start`; the fleet is
  then started on-demand via `stack_up.sh start-fleet` (CLI) or the
  GCS UI's SITL Manager panel (POST /api/sitl/start). ADR-0030.
- Frame normalization is deliberately tolerant (key aliases, envelope
  unwrapping) — extend the normalizers rather than hard-coding backend
  shapes.
- Mock engines (`src/lib/mock-fleet.ts`, `mock-setup.ts`) exist for
  offline demo/development, not to mask backend regressions:
  live-vs-mock is always visible in the UI. (The `mock-sim.ts` mock
  was removed with the Sim Console in 2026-09-10.)

## Verification

- `npm run lint` and `npm run build` must be clean.
- End-to-end: `../scripts/browser_live_test.sh` — opens the console
  through the gateway, asserts the LIVE badge, asserts telemetry is
  moving (two snapshots differ), and captures screenshots.
- Operator Map end-to-end: `../scripts/browser_map_test.sh` (O-2) —
  real map clicks place waypoints, upload + start mission drive the
  live fleet, screenshots captured. Vehicle Setup end-to-end:
  `../scripts/browser_setup_test.sh` (S-2).
- **GCS v1 G-ladder** — 11 surviving single-invocation harnesses
  under `console/tests/run_g*.sh` (was 14 before the 2026-09-10
  cleanup), all green as of the post-cleanup reval (per
  `../docs/VERIFICATION.md`). They extend the existing I/F/S/O ladder.
  Mapping to milestones:
  - M1: G-0 (version check), G-1 (validation), G-2 (persistence)
  - M2: G-3 (upload, 3 MAVLink types), G-4 (download, 3 types)
  - M3: G-5 (Fly View 1-vehicle telemetry), G-6 (multi-vehicle
    select), G-7 (pre-arm + arm/disarm)
  - M4: G-8 (Vehicle Setup param extensions)
  - M5: G-9 (fleet mission binding), G-17 (fleet live bring-up)
  - M6: G-11 (ULog browse + plot, trimmed to ULog-only 2026-09-10)
  - M7: G-13 (survey/corridor/perimeter patterns; the swarming-
    patterns dropdown was removed 2026-09-10 but the generator
    math in `src/lib/patterns.ts` is preserved)
  - Polish: G-14 (canvas parity), G-15 (keyboard), G-16 (plan),
    G-20 (parity), G-21 (gate-harness list, trimmed 2026-09-10)
  - **DELETED 2026-09-10**: G-10 (fleet orchestration + sequential
    auction — auction removed), G-12 (.replay scrub — Analyze
    .replay tab removed), G-19 (Sim Console / fault console / SIM
    E-STOP / replays tab — Sim Console overlay + sim socket ladder
    removed).
  Each harness follows the existing single-invocation pattern
  (start → assert → teardown in one shell call; no background
  processes survive). Mock helpers used by the G-ladder live
  alongside the harnesses: `mock_px4_mission.py`,
  `mock_px4_fly.py`, `mock_px4_fleet.py`, `gcs_mission_client.py`,
  `test_patterns.ts`. (`mk_replay.py` was removed with G-12 in
  2026-09-10.)

## Child DOX Index

| Child | Scope |
|---|---|
| `docs/adr/` | GCS-specific ADRs numbered 0019+ to continue the cross-repo sequence (the latest existing ADR outside console/ is fleet's 0018). Each ADR follows the same format as `sim/docs/adr/` and `fleet/docs/adr/`. Accepted: **0019** (mission file format — TOML on disk, JSON on wire), **0020** (persistence — filesystem with atomic writes; the `.replay` symlink section is a 2026-09-10 historical record — the `/api/replays*` routes were removed), **0026** (mission validation rules — strict bounds, any polygon, rally ≤ 5), **0027** (`:8300` server shape — new `fleet-catalog` binary in `fleet-mission`; the references to the removed `scenario.rs` / `compile.rs` / `runner/` / `report.rs` siblings are 2026-09-10 historical record), **0029** (PX4 version policy — hard reject on v1.16.2 mismatch), **0030** (SITL lifecycle is operator-driven — QGC/MP pattern; the `fleet-supervisor` binary on :8500 owns the `mavfleet` child; stack_up.sh start no longer auto-starts the fleet; the GCS UI's SITL Manager overlay panel is the operator-facing entry — added 2026-09-10). Proposed (not yet accepted): 0021 (ULog serving), 0022 (map tiling), 0023 (multi-vehicle UI), 0024 (replay streaming — the `.replay` half is removed in the 2026-09-10 cleanup), 0025 (param preset format), 0028 (survey pattern generator). |

No child AGENTS.md files yet. Candidates when they become durable
boundaries: `src/components/canvas/overlays/` (the overlay panels),
`src/state/` + `src/hooks/` (the telemetry engines + map-settings +
sitl-supervisor), `tests/` (the G-ladder harnesses).
