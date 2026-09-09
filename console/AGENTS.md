# console/ — RustSim operator console (GCS v1)

## Purpose

Single-page Next.js operator console that mounts **seven permanently-mounted
tabs** (hidden by CSS, never unmounted) so each telemetry engine survives
mode switches. The seven tabs map to GCS v1's feature areas
(`docs/GCS_SPEC.md` §5):

- **Plan** (`PlanView.tsx`) — mission editor + geofence drawing +
  validation + save/upload (GCS_SPEC §5.1, §8.1). Backed by `:8300`.
- **Fly** (`FlyView.tsx`) — live map, attitude HUD, instrument widgets,
  pre-arm checklist, action bar (GCS_SPEC §5.2, §8.2).
- **Sim Console** (`SimConsole.tsx`) — rustsitsim plane on `:8200+i` —
  10 Hz physics telemetry, strip charts, fault injection (unchanged from
  pre-GCS).
- **Fleet C2** (`FleetC2.tsx`) — mavfleet plane on `:8400` — fleet table,
  NED map, task board, event log, e-stop (unchanged from pre-GCS).
- **Operator Map** (`OperatorMap.tsx`) — mavfleet plane — Leaflet geo map
  with direct SITL control, ADR-0017 (unchanged from pre-GCS).
- **Vehicle Setup** (`VehicleSetup.tsx`) — the ADR-0016 configuration
  workflow, extended in M4 with param search, presets, diff-against-defaults.
- **Analyze** (`AnalyzeView.tsx`) — `.replay` + `.ulg` browser with
  timeline scrub, topic plotting, and live-vehicle overlay (GCS_SPEC §5.5,
  §8.5). Backed by `:8300`.

The telemetry engines run a LIVE/SIMULATED dual mode: if the Rust backend
answers through the gateway, everything is live; otherwise a client-side
mock engine keeps the UI operable and the live endpoint is re-probed every
12 s.

## Ownership

Owned here: pages/components/hooks/libs under `src/`. Concretely:

- The 7 mounted views: `src/components/dashboard/{PlanView,FlyView,
  SimConsole,FleetC2,OperatorMap,VehicleSetup,AnalyzeView}.tsx`.
- The GCS v1 panels + dialogs that ship on those views:
  `MissionBindingsPanel.tsx` (M5 fleet-mission binding),
  `StartFleetDialog.tsx` (M5 parallel/sequential start),
  `PatternsDialog.tsx` (M7 survey/corridor/perimeter generator),
  `PlanMap.tsx` (the Plan View's mission map widget),
  `StripChart.tsx` + `StatTile.tsx` + `ConnBadge.tsx` (Fly/Analyze HUD
  widgets), `TaskPanel.tsx` (Fly View action bar), `EventLog.tsx`,
  `FaultConsole.tsx` (Sim Console), `ThemeToggle.tsx`.
- The hooks: `usePlanCatalog` (Plan View ↔ `:8300`), `useFlyView` (Fly View
  instruments + action bar), `useSimConsole` (Sim Console engines),
  `useFleetC2` (Fleet C2 table + WS), `useOperatorMap` (Operator Map
  engines, ADR-0017), `useVehicleSetup` (Vehicle Setup engines, ADR-0016),
  `useAnalyze` (Analyze View replay/ULog streaming).
- The tolerant protocol normalizers (`src/lib/conn.ts`) and the dual-mode
  lifecycle. The GCS v1 client-side helpers: `src/lib/plan-types.ts`
  (MissionFile TS mirror of ADR-0019's Rust struct), `src/lib/patterns.ts`
  (M7 survey/corridor/perimeter generators per GCS_SPEC §5.6).

Not owned here: the backend schemas (sim SPEC §4, fleet spec §3.4 — the
console normalizes tolerantly but does not redefine them); the gateway
itself (see `Caddyfile.example`); the `:8300` catalog server itself
(`fleet/crates/fleet-mission/src/gcs/`, see `../fleet/AGENTS.md`); the
GCS v1 spec (`../docs/GCS_SPEC.md` owns scope, feature areas, G-ladder,
and milestone roadmap).

## Local Contracts

- No database, no auth, no server state: the console is a pure client of
  the three Rust control planes (`:8200+i` sim, `:8300` catalog,
  `:8400` fleet).
- API routing styles (see `src/lib/conn.ts`):
  - `gateway` (default): relative fetches + `?XTransformPort=<port>`, WS at
    `/?XTransformPort=<port>` — used behind the Caddy gateway
    (`Caddyfile.example`) and preview proxies. The `port` argument selects
    the backend: `8200+i` (sim), `8300` (catalog), `8400` (fleet). Plan and
    Analyze views use `?XTransformPort=8300`; Sim Console uses `8200+i`;
    Fleet C2 + Operator Map use `8400`.
  - `direct` (`NEXT_PUBLIC_RSIM_API_STYLE=direct`): absolute
    `http://127.0.0.1:<port>` REST + `ws://127.0.0.1:<port>/` for local runs
    without the gateway.
- All seven views stay mounted (hidden by CSS) so telemetry engines survive
  tab switches. Leaflet (Operator Map, Plan View map widget) therefore
  initializes inside a 0-size container — `GeoMap` / `PlanMap` must
  `invalidateSize()` BEFORE any `fitBounds` (fitting against the stale 0×0
  cached map size produces a degenerate world view where map clicks resolve
  to garbage lat/lon; the O-2 harness carries a fence-size guard against
  exactly this regression).
- Geo conversion is the TS port of the Rust `GeoOrigin` (`src/lib/geo.ts`,
  ECEF + Bowring, same numbers as the manager) — never ad-hoc linear math.
- Operator commands are thin REST POSTs to the ADR-0017 plane
  (`/api/mission{,/start,/clear}`, `/api/vehicles/{i}/{arm,takeoff,land,
  rtl,hold,goto}`); the mock engine implements the same semantics (fence
  validation, op* ids, go-to flight) so SIMULATED mode stays honest. GCS
  v1 adds `:8400` endpoints `/api/vehicles/{i}/mission/upload`,
  `/api/vehicles/{i}/mission/start`, `/api/fleet/mission-bindings`,
  `/api/fleet/start`, `/api/fleet/patterns/{name}/generate` and `:8300`
  endpoints `/api/missions`, `/api/replays`, `/api/ulogs` — all routed
  through the same gateway `?XTransformPort=` pattern.
- Dependencies are trimmed to what `src/` actually imports (radix
  tabs/select/label/progress/scroll-area/alert-dialog/toast/slot, lucide,
  next-themes, **leaflet** + `@types/leaflet`, cn util, Tailwind v4). No
  prisma, no template cruft, no react-leaflet wrapper.

## Work Guidance

- `npm install && npm run build && npm start` (or `npm run dev`). Port 3000.
- For the gateway mode, run Caddy with `Caddyfile.example` and open :81.
- Frame normalization is deliberately tolerant (key aliases, envelope
  unwrapping) — extend the normalizers rather than hard-coding backend
  shapes.
- Mock engines (`src/lib/mock-sim.ts`, `mock-fleet.ts`) exist for offline
  demo/development, not to mask backend regressions: live-vs-mock is always
  visible in the UI.

## Verification

- `npm run lint` and `npm run build` must be clean.
- End-to-end: `../scripts/browser_live_test.sh` — opens the console through
  the gateway, asserts the LIVE badge on both consoles, asserts telemetry is
  moving (two snapshots differ), and captures screenshots.
- Operator Map end-to-end: `../scripts/browser_map_test.sh` (O-2) — real
  map clicks place waypoints, upload + start mission drive the live fleet,
  screenshots captured. Vehicle Setup end-to-end:
  `../scripts/browser_setup_test.sh` (S-2).
- **GCS v1 G-ladder (G-0..G-13)** — 14 single-invocation harnesses under
  `console/tests/run_g*.sh`, all green as of M7 (per `docs/GCS_SPEC.md` §9
  and recorded with evidence in `../docs/VERIFICATION.md`). They extend the
  existing I/F/S/O/R ladder. Mapping to milestones:
  - M1: G-0 (version check), G-1 (validation), G-2 (persistence)
  - M2: G-3 (upload, 3 MAVLink types), G-4 (download, 3 types)
  - M3: G-5 (Fly View 1-vehicle telemetry), G-6 (multi-vehicle select),
    G-7 (pre-arm + arm/disarm)
  - M4: G-8 (Vehicle Setup param extensions)
  - M5: G-9 (fleet mission binding), G-10 (fleet orchestration)
  - M6: G-11 (ULog browse + plot), G-12 (replay scrub + overlay)
  - M7: G-13 (survey/corridor/perimeter patterns)
  Each harness follows the existing single-invocation pattern (start →
  assert → teardown in one shell call; no background processes survive).
  Mock helpers used by the G-ladder live alongside the harnesses:
  `mock_px4_mission.py`, `mock_px4_fly.py`, `mock_px4_fleet.py`,
  `gcs_mission_client.py`, `mk_replay.py`, `test_patterns.ts`.

## Child DOX Index

| Child | Scope |
|---|---|
| `docs/adr/` | GCS-specific ADRs numbered 0019+ to continue the cross-repo sequence (the latest existing ADR outside console/ is fleet's 0018). Each ADR follows the same format as `sim/docs/adr/` and `fleet/docs/adr/`. Accepted: **0019** (mission file format — TOML on disk, JSON on wire), **0020** (persistence — filesystem with atomic writes), **0026** (mission validation rules — strict bounds, any polygon, rally ≤ 5), **0027** (`:8300` server shape — new `fleet-catalog` binary in `fleet-mission`), **0029** (PX4 version policy — hard reject on v1.16.2 mismatch). Proposed (not yet accepted): 0021 (ULog serving), 0022 (map tiling), 0023 (multi-vehicle UI), 0024 (replay streaming), 0025 (param preset format), 0028 (survey pattern generator). |

No child AGENTS.md files yet. Candidates when they become durable
boundaries: `src/components/dashboard/` (the 7 mounted views + their
panels/dialogs), `src/lib/conn.ts` + `src/hooks/` (the telemetry engines),
`tests/` (the G-ladder harnesses).
