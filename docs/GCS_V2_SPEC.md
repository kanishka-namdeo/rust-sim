# RustSim GCS v2 — Operations Canvas Engineering Specification

**Status:** Draft v1.0 — 2026-09-09 (execution-ready; grounded in the v1.0.6x shipped console and 2026-09-09 UX/tech research)
**Owners:** RustSim core team
**Scope:** `console/` (primary — full front-end re-architecture); `fleet/`, `sim/`, `fleet-catalog` (read-only consumers, **zero backend changes**, §10)
**Supersedes:** the UX/presentation layer of `GCS_SPEC.md` v0.2.1 (§8 UX flows and §9 UI-facing gate assertions). All backend contracts of v1 (ADRs 0016–0029, mission persistence, validation V-1..V-13, the `:8300`/`:8400`/`:8200+i` planes, the G-0..G-13 backend-level gates) carry over unchanged.
**Related:** `docs/GCS_SPEC.md`, `docs/ARCHITECTURE.md`, `docs/VERIFICATION.md`, `console/AGENTS.md`, `console/docs/adr/`, `fleet/docs/adr/`
**Research base:** competitor UX survey (QGC 4/5, Mission Planner, Auterion Mission Control, UgCS, DJI FlightHub 2 Virtual Cockpit, FlytBase, DroneDeploy, MDPI Future Internet 13(8):188) + MapLibre GL v6 technical survey, 2026-09-09, sources in Appendix F.

> This spec is the binding engineering contract for **GCS v2 "Operations Canvas"**: the
> re-architecture of the 7-tab console into one modern single-screen operator
> interface — a full-bleed MapLibre GL map canvas with edge-HUD widgets, right-click
> context menus, and single-key keyboard shortcuts. Every feature shipped in GCS v1
> (all 7 tabs) gets a v2 home; nothing is dropped (decision D5). The spec is written
> for execution by agent teams: §13 breaks the work into file-path-precise tasks,
> §12 defines the M8..M15 milestone ladder with new gates G-14..G-21, and every
> interaction contract is specified to the exact key, menu item, and endpoint.

---

## 1. Purpose, Scope & Decision Log

### 1.1 What Operations Canvas is

GCS v1 delivered a QGC/MP-class feature set through seven permanently-mounted tabs
(Plan, Fly, Sim Console, Fleet C2, Operator Map, Vehicle Setup, Analyze). It works —
14 gates green — but its shape is a 2015-class tabbed application. Operators
context-switch between tabs to do one job (plan a mission, fly it, watch the fleet),
exactly the pain the QGC community has carried for seven years ("unify Plan and Fly"
has been a top request since 2018). Mission Planner solves it with floating windows
(dated); Auterion and DJI's Virtual Cockpit solve it the modern way: **one screen, a
dominant live map, every control docked at the edges, commands delivered through
map-context interactions and single keys**.

Operations Canvas is that shape, built on the existing Next.js console and the
existing three Rust planes:

- **One full-bleed map canvas** (MapLibre GL JS v6) covering the entire viewport.
- **Edge furniture**: a top status strip, a left icon rail, a right telemetry column,
  a bottom command bar. Panels collapse to the edges; the map never moves.
- **Right-click context menus** on every meaningful map target (map, vehicle,
  waypoint, fence vertex, mission segment, task marker) — location-scoped verb menus
  in the Google Maps / Mission Planner grammar, but short, grouped, state-filtered.
- **Single-key shortcuts** for every operator verb (T takeoff, L land, R RTL, H hold,
  E e-stop, 1/2 vehicle select, G goto-cursor…), with a `?` cheat-sheet overlay.
- **All of v1, re-homed**: Plan-on-live-map (mission editing is a map *mode*, not a
  page), Fleet C2 as edge widgets + per-vehicle cards, Vehicle Setup and Analyze as
  overlay drawers, Sim Console as the SITL control widget set.

### 1.2 Scope boundaries (carried from v1, unchanged)

SITL-only: every vehicle is a PX4-Autopilot v1.16.2 SITL process (ADR-0029 version
pin). No real hardware, flashing, serial links, or field ops. The console remains a
**pure client** (ADR-0027 corollary, §10): no server-side data state in Next.js, no
new Next API routes beyond the existing identity stub, no database. Dual-mode
operation (LIVE vs SIMULATED mock engines with visible badge and 12 s re-probe)
remains a hard contract — the console must be usable with all planes down.

### 1.3 Relationship to GCS_SPEC.md v1

`GCS_SPEC.md` remains the binding spec for the backend planes and the G-0..G-13
ladder's backend assertions (validation rules, persistence semantics, upload/download
protocol, PX4 version gate). This document supersedes v1's **presentation-layer**
contracts (§8 UX flows, tab structure, widget layout). Where the two conflict on UI
matters, this document wins. Where v1 deferred decisions to ADRs, those ADRs remain
in force; v2 adds no new ADRs (all v2 decisions are made here, decision-log style,
because they are presentation-layer choices reversible at the code level).

### 1.4 Decision log (D1–D8, all fixed for v2)

| # | Decision | Value | Rationale (research-grounded) |
|---|---|---|---|
| D1 | Deliverable/rollout home | Incremental ladder on `main`, milestones M8..M15, console shippable at every gate | Matches repo gate discipline; no long-lived branch drift; every gate re-runs the full existing ladder |
| D2 | Map engine | **MapLibre GL JS `^6.9.0`** (BSD-3-Clause, ESM-only, WebGL2) | 60 fps GPU pan/zoom, vector dark styles, pitch/bearing; the 2026 web-map standard (DroneDeploy/FlytBase class) |
| D3 | React binding | `react-map-gl ^8.1.3` subpath `react-map-gl/maplibre` with explicit `mapLib` prop | visgl-maintained, maplibre-v6-ready, no version lock; fallback: vanilla `useEffect` (§6.2) |
| D4 | Layout family | Edge HUD + rails (fixed furniture: top strip / left rail / right column / bottom bar) | Auterion/DJI-class; zero window-management complexity vs dockable panels; deterministic for agent testing |
| D5 | Feature coverage | Full parity day 1 — every v1 tab function has a v2 home before any v1 tab is removed | Nothing regresses; v1 tabs stay mounted until the owning parity gate passes (§12 M14) |
| D6 | Keyboard scheme | Single-key operator verbs + `?` cheat-sheet overlay; fixed keymap, no customization UI | The #1 unmet request in both QGC and MP communities; DJI's X/V/F/1-2-3 is the proven real-world single-key set |
| D7 | Visual language | Dark tactical: `#0B0F14` canvas, cyan `#22D3EE` primary accent, amber `#F59E0B` alert accent | Operator-room standard (Auterion); long-session legibility; matches existing `defaultTheme="dark"` |
| D8 | Spec depth | Execution-grade: interaction contracts to the key/menu-item/endpoint level, per-task file paths, gates per milestone | Written for agent teams; zero UI-assumption steps inherited from v1 §8 discipline |

---

## 2. North Star & UX Doctrine

### 2.1 The single-screen law

> **The map never moves, never reflows, and never shares the viewport with a
> full-screen panel.** Every piece of chrome docks at an edge, collapses into that
> edge, or floats as a bounded overlay ≤ 40 % of viewport area. At any moment the
> operator sees ≥ 60 % of the map, the active vehicle, and its track.

This one law drives every layout decision below. The failure mode it kills is
documented across the research: FlytBase/DroneDeploy dashboards drift to "table +
tiny map"; DJI's pre-VC "Live Flight Controls" was publicly criticized for exactly
this and was rebuilt as Virtual Cockpit.

### 2.2 Zone map (the Operations Canvas layout contract)

```
+--------------------------------------------------------------------------------------+
| A. STATUS STRIP   [RSIM] [phase] [v0 READY v1 READY] [link] [GPS] [bat] [clock] [E-STOP]|
+---+----------------------------------------------------------------------------------+---+
| B.|                                                                                  | C.|
| L |                                                                                  | T|
| E |                                                                                  | E|
| F |                                                                                  | L|
| T |                                                                                  | E|
|   |                     E. MAP CANVAS (MapLibre GL, full-bleed)                     | M|
| R |                                                                                  |   |
| A |   vehicles - tracks - fence - waypoints - mission lines - task markers           | C|
| I |                                                                                  | O|
| L |   (right-click any target: context menu; left-click: select; drag: edit)         | L|
|   |                                                                                  | U|
| I |                                                                                  | M|
| C |                                                                                  | N|
| N |                                                                                  |   |
+---+---------------------------+------------------------------------------------------+---+
| D. COMMAND BAR  [active vehicle] [mode] [ARM][TAKEOFF][LAND][RTL][HOLD] [MISSION >]  |
|                 [goto target] [map mode: Fly/Plan/Fence] [? shortcuts] [notifications]|
+--------------------------------------------------------------------------------------+
   F. OVERLAY PANELS (bounded, from left rail): Plan strip - Fleet C2 - Setup drawer -
      Analyze overlay - Sim control - Pre-arm checklist         G. NOTIFICATION STACK
                                                                (top-right, queued toasts)
```

| Zone | Name | Size contract | Collapse behavior |
|---|---|---|---|
| A | Status strip | fixed 48 px top, full width, `pointer-events:auto` | never collapses; E-STOP always visible |
| B | Left icon rail | fixed 56 px wide, full height (between A and D) | always visible; panel toggles only |
| C | Right telemetry column | 312 px default (xs: 0 collapsed) | collapses to a 40 px tab showing attitude + battery only |
| D | Command bar | fixed 96 px bottom (2 rows: verbs + context) | never collapses; hides verb buttons that are state-invalid |
| E | Map canvas | everything else; `inset: 0` under A/B/C/D | **never moves** (A/C/D overlay it with translucent backgrounds) |
| F | Overlay panels | ≤ 40 % viewport area, anchored to left rail | slide out; state persists per session |
| G | Notification stack | top-right, below A | auto-dismiss 6 s (errors sticky), max 5 visible |

Edge furniture (A, C, D) renders **over** the map with `rgba(11,15,20,0.88)` +
`backdrop-blur(8px)`-equivalent translucency (solid `rgba` fallback — no
`backdrop-filter` in PDF/print contexts, and it is on the Playwright CSS blacklist
for any future print path). The map's attribution control sits bottom-right inside
zone E, always visible (basemap licenses, §6.4).

### 2.3 The ten UX rules (research-derived, binding)

1. **Plan-on-live-map.** Mission editing is a map *mode* (Waypoint / Fence / Corridor
   tools), never a page. QGC's Plan/Fly split is its oldest community complaint; UgCS
   and the MDPI fleet GCS both edit on the operational map. (Source: QGC docs +
   discuss threads; UgCS manual.)
2. **State-gated commands.** A command button renders enabled **only** when the
   backend state permits it (armed/missio/fsm checks). Invalid verbs are hidden or
   visibly disabled with a one-line reason tooltip — Auterion's "only state-valid
   buttons shown" pattern, guarding MP's ungated-button pitfall.
3. **Right-click = location-scoped verb menu.** Short, grouped, state-filtered
   (§7.2). Never a kitchen-sink flat list (MP pitfall: camera + planner + survey +
   third-party mixed). Max 7 items, 3 groups, destructive items last + red.
4. **Hold-to-confirm replaces dialog chains.** Destructive/irreversible flight verbs
   (ARM, TAKEOFF, START MISSION, START FLEET) use a 400 ms press-and-hold button or a
   confirm chip; never QGC's dialog-in-dialog chains. DISARM/LAND/RTL/HOLD are
   single-tap (they are safety-positive).
5. **E-stop is one deliberate action, visually firewalled.** Single press of `E` or
   click of the top-strip E-STOP fires `POST /api/fleet/estop` immediately (fleet
   estop is safe-positive: false positive costs a relaunch, false negative costs a
   vehicle). It renders amber-on-dark, physically separated from takeoff-class
   verbs, flashes the status strip for 2 s, and cannot share a hold-to-confirm group
   with any other verb. (Sources: AMC separate motor-kill; DJI one-touch stop.)
6. **Single-key operator verbs + cheat sheet.** The loudest unmet request in QGC
   ("keyboard shortcuts" Blue Robotics thread) and MP (2017 GitHub issue, still
   open). `?` opens the overlay; every shortcut is discoverable in ≤ 2 s. Fixed
   keymap, no customization UI in v2 (D6).
7. **Map key-grammar, Google-Earth class.** Arrows pan, `+`/`-` zoom, `R` reset
   north/pitch, `C` re-center on active vehicle, `F` follow toggle. MapLibre's
   built-in KeyboardHandler handles these when the canvas is focused; our global
   layer handles letter verbs with input-focus guards (§7.3.2) — no conflicts by
   construction (non-letter vs letter).
8. **Fleet = per-vehicle cards, not tables.** MDPI Future Internet 13(8):188's
   multi-operator study: bottom per-drone cards (id, next target, coords, battery,
   RTH, mini-attitude) kept shared-display performance high during unexpected
   events. v2's Fleet C2 panel renders cards; the full FSM table survives as an
   expandable detail inside the overlay panel, not the primary surface.
9. **Notifications are a queue, not a slot.** v1's `TOAST_LIMIT=1` + ~17 min remove
   delay showed one sticky toast and dropped the rest — a real ops defect (§11). v2:
   stacked queue, 6 s auto-dismiss, severity colors, error-class sticky until
   acknowledged, click-through to source panel. Battery thresholds ladder (MDPI):
   30 % warn / 20 % caution / 10 % auto-RTL notice surfaced as card badges.
10. **Offline is a first-class state.** Basemap ladder with raster fallback and an
    empty-style offline mode (§6.4); telemetry never depends on tiles; every plane
    shows its own LIVE / SIMULATED / CONNECTING(n s) badge (ConnBadge contract
    preserved). The map must render our GeoJSON overlays with zero network.

### 2.4 Operator personas kept in frame

- **The solo operator** (flies v0 by hand: goto/takeoff/land from the map) — served
  by zone D verbs, right-click goto, single keys.
- **The fleet supervisor** (binds missions, starts fleet orchestration, watches
  auction + safety events) — served by Fleet C2 overlay + event rail + vehicle cards.
- **The test engineer** (injects faults, swaps scenarios, scrubs replays, plots
  ULogs) — served by the Sim control panel + Analyze overlay + context-menu fault
  injection.
One screen serves all three without mode-switching pages: overlays are toggled, the
canvas and its data layers persist underneath (the v1 `forceMount` lesson, §11 P1).

---

## 3. Current-State Baseline (v1 inventory — the migration source of truth)

Executed against the shipped tree 2026-09-09 (HEAD `a307651`). Full component-level
detail lives in the inventory worklog entry; this section is the binding summary.

### 3.1 Stack facts (what v2 builds on)

| Item | Value (v1 shipped) | v2 change |
|---|---|---|
| Framework | Next.js `^16.1.1` (App Router, single page `/`, `output:"standalone"`) | unchanged; still one page |
| React | 19.x | unchanged |
| Styling | Tailwind CSS 4 + shadcn/ui (new-york) + CSS-var tokens in `globals.css` | tokens re-authored (§5), shadcn retained |
| Map | Leaflet 1.9.4 direct-import, 3 imperative map components (~1,100 lines) | replaced by MapLibre GL 6 (§6); Leaflet deleted at M14 |
| State | pure React hooks + refs, hoisted engines in `page.tsx`; no store lib | module stores via `useSyncExternalStore` (§9), zero new deps |
| Tests | 14 `console/tests/run_g*.sh` + 3 `scripts/browser_*.sh`, agent-browser CLI as driver | extended G-14..G-21 + browser gates (§12) |
| Gateway | Caddy `:81`, `?XTransformPort=<port>` → localhost port; `src/lib/conn.ts` `gw()/wsUrl()` | unchanged (§10) |
| Build | `next build` + `finish-standalone.mjs` → `.next/standalone/server.js` | + maplibre worker copy step (§6.2) |

### 3.2 Feature inventory — every v1 feature gets a v2 home (D5)

| v1 tab | v1 features (condensed) | v2 home |
|---|---|---|
| Plan | mission library (list/load/delete/refresh), new mission, waypoint tool (add/drag/remove), geofence draw (vertex/close), corridor draw, patterns (survey-grid/corridor/perimeter modals), waypoint table (alt/hold/accept edit), fence ceiling/floor, validate (ADR-0026), save (POST/PUT), upload-to-vehicle (3 sub-bars), download-from-vehicle + comparison, validation status panels | map **Plan mode** + **Mission strip** overlay (F) + **Library** overlay; patterns via map mode + panel; validation markers as map layers (§6.5) + strip badges |
| Fly | attitude HUD (SVG horizon), instruments (battery/signal/mode/GPS/EKF), pre-arm checklist (ARM gate), map with vehicles + goto confirm bar + follow, action bar ARM/DISARM/TAKEOFF/LAND/RTL/HOLD/START MISSION, vehicle select + list, 3 strip charts | right telemetry column (C) + command bar (D) + context menus (§7.2); pre-arm = pre-flight checklist panel; strip charts = Analyze-lite plots docked in column C |
| Sim Console | sim status (phase/px4_connected/loop_closed/tick p95), stat tiles, e-stop, trajectory NED map, strip charts (alt/yaw/motors), sensor panel, fault console (inject + list + clear) | **SITL control** overlay (F) + fault verbs in vehicle context menu + stat tiles in status strip A; per-vehicle sim plane selector (fixes §11 P9) |
| Fleet C2 | fleet phase + FSM counts, e-stop, patterns dialog (3 swarm patterns + generate + auto-bind), start-fleet dialog (parallel/sequential + gate + timeout), mission bindings panel (assign/upload/state badges), NED fleet map, fleet table, task panel + auction log, event log (filterable, 8-policy safety ladder) | **Fleet C2** overlay (F) with vehicle cards (R8) + bindings sub-panel + patterns sub-panel + start dialog; events = **event rail** (G-adjacent bottom-left feed) + safety ladder detail in overlay; fleet NED view = camera "NED inset" toggle in the same canvas (§6.6) |
| Operator Map | live geo map, vehicle popups, trajectories, fence, Plan/Fly toggle, waypoint add/edit/remove + upload + start + clear, goto, guided action bar, follow | the canvas itself (E): Plan mode + goto flow + command verbs — this tab *is* the v2 shell, minus the tab |
| Vehicle Setup | vehicle selector, sections (summary/airframe/sensors/power/safety/flight-modes/params), airframe apply + reboot, calibrate buttons, param download/search/group/edit/diff/presets | **Setup drawer** overlay (F), per selected vehicle; opened from vehicle context menu + left rail |
| Analyze | replay/ULog lists, trajectory + playhead + scrubber + play, stacked strip charts + add-plot topic modal, overlay-live modal, auto-plot pos_ned_m | **Analyze overlay** (F): full-height right sheet with scrubber docked above command bar; playhead + trail rendered as map layers (§6.5 L11/L12) |

### 3.3 API surface (consumed by console today — all preserved, §10)

- **Fleet `:8400`** — 23 endpoints the console calls: `/api/fleet` (snapshot), `/api/events`, `/api/fleet/estop`, `/api/fleet/mission-bindings` (GET/POST/DELETE), `/api/fleet/start`, `/api/fleet/patterns` + `/{name}/generate`, `/api/vehicles/{i}` + `/setup` `/params` (+POST, `/refresh`) `/airframe` `/calibrate` `/mode` `/prearm-checks` `/arm` `/takeoff` `/land` `/rtl` `/hold` `/goto` `/mission?type=`, `/api/mission` `/start` `/clear`, `/api/airframes`, `/api/modes`. WS `/` (or `/ws/fleet`) at 10 Hz → `FleetFrame` (30-field `VehicleView[]`, tasks, geofence, `events_tail[16]`).
- **Catalog `:8300`** — `/api/missions` CRUD + `/validate` + `/versions/{v}` + `/rollback`, `/api/vehicles/{i}/mission/upload`, `/api/vehicles/{i}/param-presets` (+`/{name}/load`, DELETE), `/api/replays` + `/{file}/meta|topics|data`, `/api/ulogs` + `/{file}/topics/{topic}/data`, `/api/health`. No WS (REST paging only; ADR-0024 stays Proposed).
- **Sim `:8200+i`** — `/api/status`, `/api/faults` (POST/DELETE), `/api/estop`; WS `/` ≈10 Hz `SimFrame`. v1 pins to `:8200` (vehicle 0 only) — v2 adds a per-vehicle sim selector (§11 P9).

Envelope contract everywhere: `{"ok":true,"data":…}` / `{"ok":false,"error":{code,message,details?}}` with 400/404/409/422/426/500/503.

### 3.4 v1 defects v2 must fix (traceability in §11)

P1 tab CSS-hiding forces engine mount gymnastics · P2 1,100 lines of imperative
Leaflet × 3 components, no abstraction · P3 toast ceiling (one sticky) · P4 zero
keyboard handling · P5 FlyView attitude synthesized from yaw only (roll/pitch
always 0 — the normalizer drops `attitude_q_wxyz`) · P6 no mission polyline on Fly
map · P7 PlanMap cannot delete individual fence vertices · P8 normalizer helpers
re-implemented 3× · P9 Sim Console pinned to vehicle 0 · P10 `ignoreBuildErrors:
true` hides type drift · P11 hardcoded `useVehicleSetup(2)` vehicle count · P12
spec↔code drift (GCS_SPEC §8 prescribes 5 tabs, shipped 7).

---

## 4. Information Architecture — the Operations Canvas

### 4.1 Component tree (target)

```
app/page.tsx                      — server shell, renders <CanvasLoader/>
components/canvas/CanvasLoader.tsx      ('use client', dynamic import, ssr:false)
components/canvas/OperationsCanvas.tsx  — zone grid + focus/keyboard providers
components/canvas/MapCanvas.tsx         — MapLibre, all maplibre imports live HERE ONLY
components/canvas/map/layers.ts         — source+layer catalog (§6.5)
components/canvas/map/camera.ts         — camera rules (§6.6)
components/canvas/map/context-menu.tsx  — right-click host (§7.2)
components/canvas/strip/StatusStrip.tsx      — zone A
components/canvas/rail/LeftRail.tsx          — zone B (icon buttons → overlays)
components/canvas/column/TelemetryColumn.tsx — zone C (attitude, instruments, mini-plots)
components/canvas/bar/CommandBar.tsx         — zone D (verbs, mode switch, goto target)
components/canvas/overlays/*            — MissionStrip, LibraryPanel, FleetC2Panel,
                                          SetupDrawer, AnalyzeOverlay, SimControlPanel,
                                          PreFlightPanel, CheatSheetDialog
components/canvas/notify/NotificationStack.tsx  — zone G
state/app-store.ts        — selection, modes, overlay toggles (§9.3)
state/telemetry-store.ts  — WS ingest + rAF flush (§9.2)
state/command-bus.ts      — POST + busy + error envelope → notifications (§9.4)
lib/conn.ts, lib/geo.ts, lib/plan-types.ts, lib/patterns.ts  — carried over (P8 fix consolidates normalizers into lib/normalize.ts)
```

Old `tabs/` components are deleted in M14 only after their functions pass the
parity gates (D5). During M8..M13 the legacy tabs remain mounted (hidden behind a
debug route `/?legacy=1` — not a user surface, just a migration safety net) while
v2 widgets come up; the default route flips to OperationsCanvas at M8.

### 4.2 Focus and z-order model

- **Map is the default focus.** Clicking any widget returns focus to the map on
  close (widget `onClose` → `map.focus()`), so key-grammar (§2.3 R7) is always live.
- Z-order (bottom→top): map canvas E → edge furniture A/C/D → overlay panels F →
  context menu → dialogs (cheat sheet, save, upload) → notification stack G.
- Overlay panels stack left-anchored (from rail B) with a max of **two** open at
  once (newest on top, older auto-collapses to its rail icon) — prevents the
  window-zoo pitfall without hiding data.

### 4.3 Responsive contract (operator screens, not phones)

| Breakpoint | Behavior |
|---|---|
| ≥ 1600 px | full: rail + column + bar + two overlays side by side |
| 1280–1599 px | column C 248 px; overlays max one; telemetry column collapses first |
| 1024–1279 px | column C collapsed to mini-tab (attitude + battery); command bar single-row |
| < 1024 px | rail collapses to icon-only overflow (⋯) — console is a desktop tool; no mobile layout is specced |

### 4.4 Pointer-events layering (the HUD-over-map iron rule)

One overlay root `div.pointer-events-none` spanning the canvas; every interactive
widget inside sets `pointer-events:auto`. Map gestures (pan/zoom/contextmenu) pass
through everywhere else. This is the standard full-bleed-map pattern and the reason
zone E never needs to know zone A/C/D exist.

---

## 5. Design System — Dark Tactical (D7)

### 5.1 Color tokens (CSS custom properties, `globals.css`)

| Token | Value | Use |
|---|---|---|
| `--rsim-bg` | `#0B0F14` | app background, canvas base |
| `--rsim-surface` | `rgba(11,15,20,0.88)` | edge furniture, overlays (translucent) |
| `--rsim-surface-solid` | `#11161D` | dialogs, dropdowns, drawers |
| `--rsim-border` | `#1E2630` | 1px hairlines |
| `--rsim-text` | `#E6EDF3` | primary text |
| `--rsim-text-dim` | `#8B98A5` | secondary text, axis labels |
| `--rsim-accent` | `#22D3EE` | primary accent: selection, active vehicle, LIVE badge, focus rings |
| `--rsim-accent-dim` | `#0E7490` | selected-item backgrounds (10–15 % tint) |
| `--rsim-alert` | `#F59E0B` | amber: warnings, E-STOP, hold-to-confirm progress |
| `--rsim-danger` | `#EF4444` | errors, FAULT phase, destructive menu items |
| `--rsim-ok` | `#34D399` | passed checks, READY, landed |
| `--rsim-v0` / `--rsim-v1` | `#22D3EE` / `#A78BFA` | per-vehicle identity colors (extends per fleet size, HSL-spaced) |
| `--rsim-grid` | `#151C26` | map graticule, chart grids |

Light mode is **out of scope for v2** (D7: dark tactical); the v1
`ThemeToggle` is retired with the tabs. The token layer keeps names
engine-neutral so a light set can be added later without re-speccing.

### 5.2 Typography

| Role | Family | Size/weight |
|---|---|---|
| App/HUD numerics | `"JetBrains Mono", "Sarasa Mono SC", ui-monospace` (tabular figures) | 13 px/500; stat tiles 22 px/600 |
| UI text | system stack (`Inter`-class) | 13 px/400; labels 11 px/600 uppercase tracking 0.08em |
| Map labels | MapLibre symbol layers, same mono for coords | 11–12 px, `#E6EDF3` on `#11161D` halo |

Numerals are always monospace/tabular — telemetry that reflows is telemetry you
stop reading (operator-HUD convention).

### 5.3 Spacing, radius, elevation

- 4 px base grid; widget padding 12 px; widget gap 8 px; rail icon 40×40 px.
- Radius: 6 px controls, 10 px panels, 4 px chips. No 2+ radii mixing per surface.
- Elevation: two levels only — edge furniture (border + 1px inset highlight) and
  floating overlays (8 px soft shadow, 30 % black). Restraint is the design.

### 5.4 State colors (badges — contract reused from v1 ConnBadge)

`LIVE` = accent cyan · `SIMULATED` = violet `#A78BFA` · `CONNECTING (n s)` = dim
gray with countdown · `OFFLINE` = danger. FSM phases: `READY` ok-green, `ACTIVE`
accent cyan, `FAULT/ABORTED` danger, `SETUP_HOLD` amber. Battery badge ladder: 30 %
warn amber / 20 % caution amber pulse / 10 % danger + auto-RTL notice (R9).

---

## 6. Map Canvas Engine (MapLibre GL JS)

### 6.1 Pinned facts (2026-09-09, verified from registry + docs)

- `maplibre-gl@^6.9.0` — **BSD-3-Clause** (not BSD-2; license check in G-14), ESM-only
  (no CJS/UMD entry; `require()`/server import fails with
  `ERR_PACKAGE_PATH_NOT_EXPORTED`), **no default export** (`import * as maplibregl
  from 'maplibre-gl'`), requires WebGL2, targets ES2019. Bundle ≈ 148 KB gz + 10.6 KB
  CSS + worker.
- React binding: `react-map-gl@^8.1.3`, subpath `react-map-gl/maplibre`, MIT,
  visgl-maintained, peers optional — caller passes `mapLib={() => import('maplibre-gl')}`.
  If the abstraction fights the layer catalog (§6.5), fall back to vanilla
  `useEffect` + `useRef` — the layer/marker contract below is written engine-neutral
  on purpose. Either way **all maplibre imports stay inside
  `components/canvas/MapCanvas.tsx`** and its `map/` children.

### 6.2 The Next.js integration (exact, worker-first)

The #1 integration failure (documented in MapLibre's own Next.js notes): under both
Turbopack and webpack, `new URL(..., import.meta.url)` drops the worker's
`maplibre-gl-shared.mjs` sibling — the map mounts but **never loads a tile**.
Therefore:

```jsonc
// package.json scripts (additions)
"predev":  "node scripts/copy-maplibre-worker.mjs",
"prebuild":"node scripts/copy-maplibre-worker.mjs"
```

```js
// scripts/copy-maplibre-worker.mjs — copies BOTH siblings into public/
// (finish-standalone.mjs already ships public/ into .next/standalone)
import { cpSync, mkdirSync } from "node:fs";
mkdirSync("public/maplibre", { recursive: true });
for (const f of ["maplibre-gl-worker.mjs", "maplibre-gl-shared.mjs"]) {
  cpSync(`node_modules/maplibre-gl/dist/${f}`, `public/maplibre/${f}`);
}
```

```tsx
// components/canvas/MapCanvas.tsx ('use client' — the ONLY maplibre import site)
import * as maplibregl from "maplibre-gl";
import { setWorkerUrl } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
setWorkerUrl("/maplibre/maplibre-gl-worker.mjs");   // module top-level, once
```

Server/Client split: `app/page.tsx` (Server Component) renders
`<CanvasLoader/>`; `CanvasLoader.tsx` is `'use client'` and does
`dynamic(() => import('@/components/canvas/OperationsCanvas'), { ssr: false })`
(`ssr:false` is illegal inside Server Components — known Next.js rule).
`next.config.ts` keeps `output:"standalone"`, `reactStrictMode:false` (strict mode's
double-mount races the WS engines; v1 already disabled it — do not re-enable without
re-running G-14..G-21). Set `ignoreBuildErrors:false` at M14 (P10 fix).

### 6.3 Basemap ladder (D2/D7 — dark, keyless-first, offline-safe)

| Priority | Style | URL / source | License/attribution duty |
|---|---|---|---|
| 1 | OpenFreeMap **dark** | `https://tiles.openfreemap.org/styles/dark` (verified 200 from sandbox) | "© OpenFreeMap © OpenMapTiles · Data from OpenStreetMap" — free incl. commercial, no key |
| 2 | CARTO dark-matter | `https://basemaps.cartocdn.com/gl/dark-matter-gl-style/style.json` (keyless CDN, verified) | "© OpenStreetMap contributors © CARTO" — free tier, low volume |
| 3 | OSM raster | style JSON with raster source `tile.openstreetmap.org/{z}/{x}/{y}.png` | OSMF tile policy: attribution visible, low-volume internal use only |
| 4 | Offline | local empty style `{version:8,sources:{},layers:[]}` + our GeoJSON overlays | n/a |

Ladder behavior: style load error → next entry; offline → bottom + `OFFLINE` chip in
the status strip + basemap retry every 60 s. Telemetry/tracks/fence/mission layers
never depend on the basemap. Attribution control bottom-right, collapsed, always
visible (license compliance is a gate: G-14 asserts it).

### 6.4 Layer catalog (single source of truth: `components/canvas/map/layers.ts`)

All dynamic data as **GeoJSON sources updated via `source.setData(fc)`** (the
official realtime pattern) — never DOM markers (P2 lesson: imperative marker soup).
Vehicles as symbol+circle layers so `queryRenderedFeatures` hit-testing (§7.2)
works.

| # | Source (id) | Content | Layers | Visibility | Style notes |
|---|---|---|---|---|---|
| L1 | `vehicles` | per-vehicle Feature (Point) w/ props `index, fsm, armed, mode, battery, heading` | `vehicles-halo` (circle, 22 px, vehicle color 25 % opacity), `vehicles-body` (symbol: triangle rotated by `heading`), `vehicles-label` (symbol text: `v0 · READY · 87 %`) | always | active vehicle halo pulses; `icon-rotate: ["get","heading"]`, `icon-allow-overlap:true` |
| L2 | `tracks` | per-vehicle LineString ring buffer (last 1800 samples, 6-dec coords) | `tracks-line` (1.5 px, vehicle color, 70 % opacity, `line-gradient` fade for active) | always | cap + `maxZoom`≈12 per large-data guide |
| L3 | `fence` | Polygon + vertices FeatureCollection | `fence-fill` (accent-dim 8 %), `fence-line` (dashed 1.5 px), `fence-vertices` (circle 5 px, draggable) | Plan mode + always-on faint | ceiling/floor shown in strip, not on map |
| L4 | `waypoints` | mission waypoints (Point, props `seq, alt, hold, accept, errors[]`) | `wp-body` (circle 7 px + seq label), `wp-line` (dashed, accent), `wp-err` (danger ring when `errors` non-empty) | Plan mode + during flight (dimmed) | ADR-0026 validation errors paint `wp-err` |
| L5 | `mission-active` | uploaded/active mission polyline + remaining-leg highlight | `mission-line`, `mission-flown` (ok-green, solid) | missionFlying | fly-over of P6: the active mission MUST render on the Fly map |
| L6 | `tasks` | fleet task markers (Point, props `label, state, assignee`) | `task-body` (diamond), `task-label` | Fleet C2 overlay open | auction state colors |
| L7 | `rally` | rally points | `rally-body` (R glyph), `rally-label` | Plan mode | ≤5 (V-12) |
| L8 | `home` | home position per vehicle | `home-body` (H glyph, vehicle color) | home_set | |
| L9 | `goto-target` | pending goto target (gimbal marker) | `goto-body`, `goto-line` (from active vehicle) | goto pending | amber; cleared on ack |
| L10 | `measure` | measure line + endpoint | `measure-line`, `measure-label` (distance/bearing) | measure active | tool from context menu |
| L11 | `replay-trail` | replay/ULog trajectory | `replay-line` (violet), `replay-head` (playhead circle + tangent arrow) | Analyze overlay open | §8 Analyze |
| L12 | `live-overlay` | second-vehicle overlay trail when Analyze "overlay live" active | `live-overlay-line` | overlay-live on | distinct color vs replay |
| L13 | `graticule` | canvas-drawn lat/lon grid (GeoJSON lines recomputed on moveend) | `graticule-line` (grid color, 0.5 px) | basemap offline / OFFLINE | replaces Leaflet fallback grid |

Layer add order is fixed at init (once); per-frame updates only call
`setData`. Editing interactions (drag waypoint / fence vertex / insert) mutate the
owning GeoJSON and re-set data — same pattern as v1's Leaflet editors, but one
shared implementation instead of three (P2, P7).

### 6.5 Camera rules (`components/canvas/map/camera.ts`)

- Default: `pitch: 0, bearing: 0`, `maxPitch: 60`, fit to fence + 20 % padding on
  boot (or home if no fence). `R` resets. `X` toggles pitch 0 ↔ 55 ("3D glance").
- **Follow mode** (default on): camera `easeTo` vehicle at ≥ 1 Hz, only when the
  operator has not manually panned for > 5 s (manual pan suspends follow; `C` or the
  follow chip re-engages) — prevents the follow/pan fight.
- **NED inset**: the Fleet C2 "NED view" (v1 NedMap) becomes a camera mode, not a
  separate widget: `shiftNED` projects `position_ned_m` around the geo origin
  (lib/geo.ts port, never ad-hoc) into a tiny local CRS at zoom ~15 north-up — the
  same canvas renders the fleet NED picture. Toggle `V`.
- Zoom clamps: 3..19; `+`/`-` step 1. Bounding-box vehicle select = `Shift+drag`
  (§7.1).
- `fitBounds` before `invalidateSize` is the O-2 fence-guard bug class — MapLibre
  has no 0×0 hidden-container problem (single always-visible canvas), which deletes
  that entire bug class; the fence-size regression guard still ships as a gate
  assertion (G-20).

---

## 7. Interaction Model

### 7.1 Mouse grammar (map canvas)

| Gesture | Target | Effect |
|---|---|---|
| left-click | empty map | clear selection; (goto mode active) place goto target → confirm chip |
| left-click | vehicle | select active vehicle (chip in bar D updates; `queryRenderedFeatures` hit) |
| left-click | waypoint/fence vertex/task | select object (edit fields appear in strip/panel) |
| left-click | mission leg | select leg (insert-here affordance at click point) |
| drag | waypoint/fence vertex | move it (GeoJSON re-set; validation re-runs debounced 300 ms) |
| `Shift+drag` | empty map | box-select vehicles (multi-select for fleet verbs) |
| dblclick | map | zoom +1 (MapLibre default) |
| right-click | any target | context menu (§7.2) |
| wheel | map | zoom (MapLibre default) |
| drag | empty map | pan (suspends follow, §6.5) |

Goto flow (the highest-frequency operator verb): `G` enters goto mode (cursor
crosshair, chip "click target for vN") → map click places L9 target with alt input
chip → `Enter` confirms `POST /api/vehicles/{i}/goto` / `Esc` cancels. Identical
result from right-click → "Go to here".

### 7.2 Right-click context menus (the verb tree)

Implementation: `map.on('contextmenu', e => { e.preventDefault(); const f =
map.queryRenderedFeatures(e.point, {layers:[…]})[0]; openMenu(e.point, f); })` —
one Radix `DropdownMenu`-class component rendered at a virtual anchor
(`position:fixed` at event x/y), closed by `Escape`/blur/`map.once('move')`. Menus
are **state-filtered** (R3): items whose precondition fails render dimmed with the
reason as tooltip, never hidden silently (discoverability beats guessability).

**M1 — on empty map** (no feature hit):
```
Go to here                      →  goto flow (active vehicle)          [vehicle selected]
Go to here… (alt)               →  goto chip with alt input
Add waypoint here               →  Plan mode: insert at seq            [Plan mode]
Add fence vertex here           →  Plan mode: append vertex           [Plan mode]
Add rally point here            →  Plan mode                          [Plan mode, <5 rally]
Measure distance                →  measure tool (L10)
Center map here                 →  camera easeTo
Copy coordinates                →  clipboard "lat, lon" (6-dec)
── SITL ──
Inject fault…                   →  fault picker for active vehicle    [sim plane LIVE]
Load scenario…                  →  scenario picker (hot swap)
```
**M2 — on vehicle** (`v{index}` chip header + state line):
```
Select vehicle                  →  set active                          [not active]
── Flight (active vehicle) ──
Arm / Disarm                    →  hold-to-confirm ARM, tap disarm
Takeoff…                        →  alt chip → hold-to-confirm
Land · RTL · Hold               →  single tap (safety-positive)
Go to here (this vehicle)       →  goto flow targeting THIS vehicle
── Mission ──
Upload mission…                 →  Library picker → catalog upload
Start mission                   →  POST /api/mission/start
── Setup / SITL ──
Vehicle setup…                  →  Setup drawer (this vehicle)
Calibrate sensor…               →  sensor picker → POST /calibrate
Inject fault…                   →  fault picker
── Danger ──
FLEET E-STOP                    →  red, immediate POST /api/fleet/estop
```
**M3 — on waypoint** (`WP {seq}` header):
```
Edit waypoint…                  →  strip inline editor (alt/hold/accept)
Insert waypoint before/after    →  Plan mode: click next point
Delete waypoint                 →  reseq (confirm-free, undoable via strip)
```
**M4 — on fence vertex** (P7 fix — v1 could not):
```
Delete vertex                   →  re-close polygon; V-1..V-13 re-validate
Insert vertex on segment…       →  click on segment midpoint
Edit ceiling/floor…             →  strip inputs
```
**M5 — on mission leg / task marker**:
```
Insert waypoint here            →  split leg at click point
Reassign task…                  →  fleet task reassign (Fleet C2)
View task detail                →  Task panel focus
```
Menu constraints (binding): max 7 visible items before grouping separator; groups
ordered [Flight | Mission | Setup/SITL | Danger]; Danger always last, red, and the
E-stop item renders even when other items are all dimmed.

### 7.3 Keyboard map (single-key operator verbs, fixed)

#### 7.3.1 The map

| Key | Verb | Backend | Guard |
|---|---|---|---|
| `1`–`9` | select vehicle N | — | n < fleet size |
| `[` `]` | cycle vehicle selection | — | wrap |
| `G` | goto mode toggle | → click → `POST /api/vehicles/{i}/goto` | vehicle selected |
| `A` | **ARM** (hold-to-confirm 400 ms) | `POST /api/vehicles/{i}/arm {arm:true}` | prearm checks (fetch first), fsm READY |
| `D` | disarm (tap) | `…/arm {arm:false}` | armed |
| `T` | takeoff (alt chip → hold-to-confirm) | `…/takeoff {alt_m}` | armed, disarmed→auto-arm attempt? NO — arm separately (R: QGC flows) |
| `L` | land (tap) | `…/land` | armed |
| `R` | RTL (tap) | `…/rtl` | armed |
| `H` | hold (tap) | `…/hold` | missionFlying or armed |
| `E` | **FLEET E-STOP** (tap, immediate) | `POST /api/fleet/estop` | always enabled, fires on keyup |
| `M` | toggle mission strip overlay | — | — |
| `P` | toggle Plan mode (waypoint tool) | — | — |
| `F` | toggle follow camera | — | — |
| `C` | re-center on active vehicle | — | — |
| `V` | toggle NED inset view | — | — |
| `B` | toggle Fleet C2 overlay | — | — |
| `N` | cycle basemap (ladder §6.3) | — | — |
| `X` | pitch glance 0↔55° | — | — |
| `+`/`-` `←→↑↓` | MapLibre KeyboardHandler (pan/zoom) | — | canvas focus |
| `Shift+arrows` | rotate/bear (MapLibre) | — | canvas focus |
| `Space` | play/pause replay | — | Analyze overlay |
| `←/→` (Analyze) | scrub ∓/±2 s | — | Analyze overlay + canvas unfocused |
| `?` | cheat-sheet overlay | — | §7.3.3 |
| `Esc` | cancel mode / close top-most overlay / clear selection | — | staged |
| `F8` | notifications panel (v1 muscle memory) | — | retained |

`E` fires on **keyup** with a status-strip flash + confirmation toast; no
hold-to-confirm (R5: e-stop is the one verb where speed beats confirmation — false
positive costs a fleet relaunch, false negative costs a vehicle). Documented
tradeoff, not an accident.

#### 7.3.2 The global key handler (guards — binding)

One `window.addEventListener('keydown', handler, true)` (capture phase) in
`OperationsCanvas`. The handler **skips** when: `e.target.closest('input,
textarea, select, [contenteditable="true"]')`; any Radix menu/dialog is open
(shortcut registry checks open-state); or a text-selection drag is in progress.
`preventDefault` + `stopPropagation` for handled keys. This mirrors the Gmail/GitHub
pattern and avoids MP's "hotkeys eat the param search" class of bug.

#### 7.3.3 Cheat sheet

`?` (Shift+`/`) opens a Radix dialog: two-column kbd table grouped [Selection |
Flight | Camera | Modes | Panels | Replay], each row `<kbd>` chip + verb + the
backend endpoint (tiny, dim — operators learn the wire truth). First-run hint chip
in the status strip: "Press ? for shortcuts" (dismissable, persisted in
localStorage).

### 7.4 Command safety (state gating + hold-to-confirm)

| Verb class | Interaction | Reason |
|---|---|---|
| ARM, TAKEOFF, START MISSION, START FLEET, airframe apply | **hold-to-confirm** (400 ms, amber progress fill) | irreversible-ish, high consequence |
| DISARM, LAND, RTL, HOLD, estop, mission clear | single tap / single key | safety-positive — never gate safety verbs behind confirmation |
| goto | two-step (place → Enter) but no hold | low consequence, reversible |
| delete waypoint/vertex, delete mission, delete preset | tap + 6 s undo toast (command-bus deferred commit) | reversible via undo |
| param write, calibrate, preset load | tap + toast ack (echo-confirmed by PARAM_SET ack) | v1 contract |

State-gating source of truth: the telemetry store's latest `FleetFrame` (fsm,
armed, missionFlying) + prearm-checks when relevant. Buttons dim with reason
tooltips ("vehicle v0 not READY", "mission already active"). The command bar never
renders an enabled verb the backend would reject — R2.

---

## 8. Widget Catalog (per-widget binding contracts)

Each widget: zone, data source + refresh, interactions, v1 origin, acceptance
condition (asserted by the gate named in §12). "rAF" = updated by the 60 fps flush
loop (§9.2), "state" = React state committed ≤ 5 Hz.

### 8.1 Zone A — Status strip (`strip/StatusStrip.tsx`)

| Widget | Content | Source | Notes |
|---|---|---|---|
| brand chip | `RSIM` + fleet phase badge | fleet frame (state) | phase colors §5.4 |
| vehicle chips | `v0 READY · 87 %` per vehicle, active outlined | fleet frame | click = select |
| link chips | per-plane ConnBadge (fleet/sim/catalog) + OFFLINE chip | probe engines | v1 ConnBadge contract |
| clock | sim time + wall clock (mono) | fleet frame | |
| **E-STOP** | amber button, rightmost, 48 px | — | `E` key equivalent; flash 2 s on fire |

### 8.2 Zone B — Left rail (`rail/LeftRail.tsx`)

Icon buttons (40×40) top→bottom with 10 px labels: Plan mode `P` · Mission strip
`M` · Library · Fleet C2 `B` · SITL control · Setup `S` · Analyze `Y` · Pre-flight
· Notifications `F8` · Cheat sheet `?` · Settings (locale, units). Active overlay
icons get the accent-dim background; max two open (§4.2).

### 8.3 Zone C — Telemetry column (`column/TelemetryColumn.tsx`)

| Widget | Content | Source | v1 origin |
|---|---|---|---|
| Attitude HUD | SVG artificial horizon, roll/pitch/yaw + heading tape, from `attitude_q_wxyz` (P5 fix: full quaternion) | fleet frame (rAF) | FlyView HUD |
| Instruments | battery (% + V + ladder badge), link/signal (heartbeat age), mode + fsm, GPS (fix/sats/lat/lon), EKF/health | fleet frame (state) | FlyView instruments |
| Mini-plots | 3 dockable StripChart-class plots (default: alt, battery, ground speed; add/remove via plot picker) — canvas 2D, 60 s window @ 10 Hz | telemetry store (rAF) | FlyView strip charts |
| Pre-flight panel (toggle) | 5 QGC checks + all_passed; ARM gate reads it | `/api/vehicles/{i}/prearm-checks` (1 s while visible) | FlyView checklist |

### 8.4 Zone D — Command bar (`bar/CommandBar.tsx`)

Row 1: active-vehicle chip · mode chip · verbs [ARM(hold) · TAKEOFF(hold) · LAND ·
RTL · HOLD · START MISSION(hold)] · MISSION ▸ (strip) · goto target chip (when
pending). Row 2: map-mode switch [Fly · Plan · Fence] · follow toggle · cheat-sheet
hint. State-gated per §7.4; every verb shows its shortcut key on the button
(`ARM (A)`). Busy state (in-flight POST) = spinner chip 400 ms min display.

### 8.5 Zone F — Overlay panels (left-anchored, ≤ 40 % viewport)

| Panel | Content (from v1) | Data |
|---|---|---|
| MissionStrip `M` | waypoint table (seq/lat,lon/AGL/hold/accept inline edit), validation status + errors list, save/upload actions (3 sub-bar progress: Mission/Fence/Rally), fence ceiling/floor inputs, pattern generators (survey/corridor/perimeter params), undo stack | plan store (local) + catalog REST |
| Library | mission list (load/delete/refresh), new mission, download-from-vehicle + comparison table | `:8300 /api/missions*`, `/api/vehicles/{i}/mission?type=` |
| FleetC2 `B` | **vehicle cards** (id, fsm, mode, battery badge, next target, coords, mini-attitude, RTH button — MDPI pattern) · bindings sub-panel (assign/upload/state badges) · patterns sub-panel (generate + auto-bind) · start-fleet dialog (mode/gate/timeout, per-vehicle results) · task board + auction log · event log w/ filters + 8-policy safety ladder detail · full FSM table (expandable) | fleet REST + WS `events_tail` |
| SimControl | per-vehicle **sim plane selector** `:8200+i` (P9 fix), sim status (phase, px4_connected, loop_closed, tick p95), sensor tiles, fault console (§7.3 catalog inject + active list + clear), scenario picker (hot swap) | sim REST/WS per vehicle |
| Setup `S` | full v1 Vehicle Setup as a drawer: sections, airframe apply + reboot badge, calibration buttons, params (download/search/group/edit/diff/presets) | `:8400` setup/params REST + `:8300` presets |
| Analyze `Y` | replay/ULog lists, scrubber (dock above bar D) + play/pause + step, plot stack + add-topic modal, overlay-live picker | `:8300` replays/ulogs REST |
| PreFlight | §8.3 panel in overlay form for dual-display ops | same |

### 8.6 Zone G — Notification stack (`notify/NotificationStack.tsx`)

P3 fix: real queue (max 5 visible, 6 s auto-dismiss, errors sticky + acknowledge,
click-through opens source panel). Severity colors §5.4; estop/battery-ladder
events render as persistent status chips in strip A until cleared (they are
states, not events).

---

## 9. Data & State Architecture (zero new runtime deps)

### 9.1 The plane multiplexer (`state/telemetry-store.ts`)

One module-level store replacing v1's per-hook sockets:

- **Fleet socket** (`:8400` WS) — 10 Hz `FleetFrame` → `normalizeFleetSnapshot`
  (consolidated from the 3 v1 copies, P8) → ring buffers (tracks 1800, strips 600,
  events 200) + last-frame ref.
- **Sim sockets** (`:8200+i`) — one per vehicle present in the fleet snapshot
  (P9/P11 fix: count discovered from fleet frame, not hardcoded) → `SimFrame`
  normalizers → per-vehicle ring buffers.
- **REST pollers** — low-rate panels (bindings 5 s, events 4 s→replaced by
  `events_tail` + 30 s REST reconcile, setup 1 s while drawer open, prearm 1 s
  while visible). Pollers pause when their panel is closed (v1 always polled —
  CPU win on 2 vCPU).
- **Probe/retry ladder (v1 contract, preserved):** probe → LIVE: WS + REST
  fallback → 3 × 2.5 s retry → SIMULATED mock engine (5–10 Hz) → 12 s re-probe.
  Mock engines port from `lib/mock-sim.ts` / `mock-setup.ts` unchanged.

### 9.2 The render pipeline (10 Hz WS → 60 fps HUD)

```
WS frame → normalizer → store.commit(raw)      // no React state
rAF loop (one, in OperationsCanvas):
  · dirty? → map sources setData (L1..L13)
  · HUD DOM refs: textContent / style.transform (attitude, numerics, bars)
  · React state commit ≤ 5 Hz: armed/mode/fsm/phase/battery-chip changes only
```

Telemetry numbers never touch React state (the 60 fps rule from the MapLibre
realtime pattern + HUD convention). Plots (canvas 2D) draw in the same rAF pass.

### 9.3 App store (`state/app-store.ts`)

`useSyncExternalStore` module store: `{ activeVehicle, multiSelect[], mapMode:
fly|plan|fence, overlays: {library, fleet, sim, setup, analyze, preflight, cheat},
follow, gotoPending, plan (mission draft + undo stack), notifications[] }`.
Commands dispatch through the **command bus** (§9.4) so busy/error handling is
uniform. No zustand/redux — the trimmed-dependency policy (console/AGENTS.md)
stands; total new runtime deps: `maplibre-gl`, `react-map-gl`,
`@radix-ui/react-context-menu`, `@radix-ui/react-dialog` (toast/dialog primitives
already present).

### 9.4 Command bus (`state/command-bus.ts`)

`command(name, vehicle?, payload)` → POST via `gw()` → `{ok}` → success toast /
`{error:{code,message}}` → notification + panel surfacing; busy registry →
button/key gating; deferred-undo wrapper for delete-class verbs (§7.4). Every
flight verb from §7.2/§7.3 routes through here — one place to log, one place to
gate.

---

## 10. Backend Deltas — none (the zero-change guarantee)

v2 is a console-only re-architecture. Concretely and binding:

- `fleet/`, `sim/`, `fleet-catalog` Rust code: **unchanged** — not one route, not
  one WS frame shape, not one envelope.
- No new Next API routes; the console stays a pure client (ADR-0027 corollary).
- Catalog WS streaming (ADR-0024) remains Proposed — Analyze keeps REST paging.
- The gateway contract (`?XTransformPort`, `src/lib/conn.ts` `gw()/wsUrl()`,
  direct-mode env) is reused as-is. (Note for a future hardening ADR, out of scope
  here: the Caddy matcher has no port allowlist.)
- The G-0..G-13 backend gates keep passing unmodified throughout M8..M15 — they run
  against the same binaries. Only their UI-facing browser assertions are superseded
  by G-14+ where the flow moved (§12).

---

## 11. Defects Fixed by v2 (P→fix→gate traceability)

| P | v1 defect | v2 fix (section) | Gate |
|---|---|---|---|
| P1 | tab CSS-hiding + engine mount gymnastics | single canvas, engines = store subscriptions (§9) | G-14 |
| P2 | 1,100 lines imperative Leaflet × 3 | one MapCanvas + layers.ts catalog (§6.4) | G-14 |
| P3 | toast ceiling (1 sticky, drops rest) | notification queue (§8.6) | G-16 |
| P4 | zero keyboard handling | full keymap + guards + cheat sheet (§7.3) | G-15 |
| P5 | attitude from yaw only (roll/pitch = 0) | quaternion through normalizer (§8.3) | G-15 |
| P6 | no mission polyline on Fly map | L5 mission-active layer (§6.4) | G-16 |
| P7 | cannot delete single fence vertex | M4 context menu + shared editor (§7.2) | G-17 |
| P8 | normalizers re-implemented 3× | `lib/normalize.ts` consolidation (§9.1) | G-14 |
| P9 | Sim Console pinned to vehicle 0 | per-vehicle sim plane selector (§8.5) | G-19 |
| P10 | `ignoreBuildErrors:true` | flipped false at M14 + full type pass | G-20 |
| P11 | hardcoded vehicle count 2 | count from fleet snapshot (§9.1) | G-18 |
| P12 | spec↔code drift (5 vs 7 tabs) | this spec matches shipped reality (§3) | — |

---

## 12. Milestone Roadmap M8..M15 & Gates G-14..G-21

Ladder rules (inherited): every milestone ships on `main` with the console
shippable (all previous gates + the v1 backend gates green); every new gate is a
**single-invocation harness** (`console/tests/run_g*.sh` pattern: start → assert →
teardown, one shell call, PID-trap cleanup, work dir preserved on failure);
FSM-dependent assertions use the f2-style "READY or any post-READY state" predicate;
browser gates drive agent-browser with `--enable-unsafe-swiftshader` (headless
WebGL2, §14 R2) and follow the v1 selector conventions (a11y snapshot → `@eN`).

| Milestone | Scope (shippable state) | New gate | Asserts |
|---|---|---|---|
| **M8 — Canvas shell** | MapLibre canvas + zones A–E + tokens + basemap ladder + offline; telemetry store + rAF pipeline; fleet WS LIVE badge; default route = OperationsCanvas, legacy at `/?legacy=1`; sim/catalog probes | **G-14** `run_g14_canvas.sh` | map `load` fires + canvas exists + attribution visible; vehicles + tracks render from real fleet WS (pixel-sample non-background); telemetry numerics change across 12 s (snapshot-diff); all three planes' ConnBadges correct incl. forced-offline → SIMULATED; maplibre license file check (BSD-3); `npm run build` + worker files present in standalone |
| **M9 — Command & keys** | command bus + state gating + hold-to-confirm; command bar verbs; full keymap + guards + cheat sheet; goto flow (G key + right-click); estop (E + strip button); P5 quaternion HUD | **G-15** `run_g15_keys.sh` | keymap fires verbs via REST spies (assert `POST /arm` etc.); guards: typing in param search does not trigger verbs; hold-to-confirm requires 400 ms (agent-browser press timing); `E` → fleet estop REST observed + strip flash; attitude roll/pitch non-zero during flight replay mock |
| **M10 — Plan-on-map** | map modes (Waypoint/Fence/Corridor) + drag/insert/delete via context menus M3/M4/M5; mission strip + library overlays; validation markers (L4 `wp-err`); save/upload/download parity flows | **G-16** `run_g16_plan.sh` | click-to-add → validate (V-rules round-trip via catalog) → save → upload 3-type progress → G-3/G-4 still green; fence vertex delete + insert (P7); mission polyline L5 visible during flight (P6) |
| **M11 — Fleet C2 on canvas** | vehicle cards + bindings + patterns + start dialog + task markers (L6) + event rail + notification queue (P3) | **G-17** `run_g17_fleet.sh` | 2-vehicle fleet: cards show live state; bind v0/v1 missions → start (parallel + sequential) → flights observed on canvas; estop teardown; events rail + queue behaviors (max 5, sticky errors) |
| **M12 — Setup drawer** | full setup surface as drawer, per-vehicle (P11 dynamic count) | **G-18** `run_g18_setup.sh` | v1 S-1/S-2 flows through the drawer (param download/search/write, calibrate, mode, airframe apply + restart, presets) |
| **M13 — Analyze + SITL** | analyze overlay (scrub, plots, overlay-live, replay trail layers L11/L12); SimControl overlay with per-vehicle plane selector (P9), fault inject via context menu + panel, scenario hot-swap | **G-19** `run_g19_analyze_sim.sh` | replay scrub ranges + monotonicity (G-12 parity), ULog plots, overlay-live; fault inject/clear on v1's sim plane; scenario swap |
| **M14 — Legacy removal + parity audit** | delete `tabs/` + Leaflet + `ThemeToggle`; `ignoreBuildErrors:false` (P10); full parity checklist §3.2; perf pass (§14 R3) | **G-20** `run_g20_parity.sh` | every §3.2 row exercised end-to-end on real SITL; bundle < 400 KB gz JS; 60 s telemetry soak: 0 dropped WS frames rendered, no React state churn > 5 Hz; O-2 fence-size guard re-run |
| **M15 — Polish** | pitch glance, NED inset `V`, measure tool, undo stack everywhere, first-run hints, settings (units) | **G-21** `run_g21_polish.sh` | feature-level assertions for each polish item + full ladder re-run (all 21 gates) |

Rollback strategy: every milestone is one revert away from green (no long-lived
branches); M8's `/?legacy=1` escape hatch remains until M14 removes it.

## 13. Agent Execution Plan (for the executing agent teams)

### 13.1 Streams (parallelizable after M8)

| Stream | Owns | Key files (create/modify) |
|---|---|---|
| A — Canvas & data | MapCanvas, layers, camera, telemetry store, normalizers, worker script | `components/canvas/MapCanvas.tsx`, `map/{layers,camera}.ts`, `scripts/copy-maplibre-worker.mjs`, `state/telemetry-store.ts`, `lib/normalize.ts` |
| B — Widgets & overlays | strip/rail/column/bar + all overlay panels + notification stack | `components/canvas/{strip,rail,column,bar,overlays,notify}/*` |
| C — Interactions | context menus, keymap, guards, command bus, goto flow, hold-to-confirm | `components/canvas/map/context-menu.tsx`, `state/{app-store,command-bus}.ts`, `components/canvas/ShortcutsProvider.tsx` |
| D — Gates & docs | harnesses G-14..G-21, browser-gate helper (swiftshader flags), DOX updates | `console/tests/run_g1[4-9]*.sh`, `run_g2[01]*.sh`, `console/tests/lib/browser_common.sh`, `docs/VERIFICATION.md` |

Dependencies: M8 = stream A alone (B/C/D start on the M8 skeleton interfaces:
zone grid, store API, command-bus signature). C depends on A's hit-testing + B's
widgets existing from M9 onward. D writes gates alongside each milestone.

### 13.2 Task cards (each task: files → DoD → verify command)

1. **T-A1 worker + deps**: add deps (`maplibre-gl@^6.9.0`, `react-map-gl@^8.1.3`,
   `@radix-ui/react-context-menu`, `@radix-ui/react-dialog`), copy script, `pre*`
   scripts. DoD: `npm run build` clean, `public/maplibre/*` in standalone. Verify:
   `ls console/.next/standalone/public/maplibre/` + G-14 build assertions.
2. **T-A2 canvas + ladder + offline**: MapCanvas with style ladder, graticule L13,
   attribution. DoD: G-14 map-load + offline assertions.
3. **T-A3 telemetry store + rAF**: fleet/sim sockets, normalizers (P8), ring
   buffers, flush loop. DoD: G-14 telemetry-diff + 0 React churn (DevTools React
   profiler count in a soak).
4. **T-B1 zone furniture**: strip/rail/column/bar skeletons per §8 contracts with
   mock store. DoD: layout gate assertions in G-14 (zones present, map ≥ 60 %).
5. **T-C1 keymap + cheat sheet**: ShortcutsProvider, registry, guards, `?` dialog.
   DoD: G-15 guard assertions.
6. **T-C2 command bus + verbs**: state gating, hold-to-confirm, estop, goto flow,
   busy registry. DoD: G-15 verb REST assertions.
7. **T-B2..B6 overlays**: mission strip → library → fleet C2 → setup → analyze →
   sim control, one task each, porting v1 component logic onto the store/command
   bus. DoD: the matching G-16..G-19.
8. **T-C3 context menus**: M1..M5 trees, state-filtered items, undo for deletes.
   DoD: G-16/G-17 menu assertions.
9. **T-D1..D8 gates**: one task per G-14..G-21 harness following the v1 harness
   conventions (§12 rules). DoD: gate green + `docs/VERIFICATION.md` row appended.
10. **T-X1 cleanup (M14)**: delete `tabs/`, Leaflet, ThemeToggle; type pass with
    `ignoreBuildErrors:false`. DoD: G-20.

### 13.3 Working conventions for the executing agents (binding)

- Read the DOX chain first (root `AGENTS.md` → `console/AGENTS.md` → this spec
  section-relevant parts). This spec is the source of truth for UI behavior; ADRs
  override it only on backend contracts (none change, §10).
- Milestone = commit boundary: Conventional-Commits subject referencing the gate
  (`feat(console): M9 command bar + keymap (G-15)`), push to `origin/main` after
  the gate is green (AGENTS.md user preference).
- Keep code debuggable: store commits log every WS frame in dev (`?debug=ws` query
  flag renders the frame inspector overlay), command bus logs verb → endpoint →
  result. No minified thinking — plain reducers, named functions.
- No new deps beyond §9.3's four without a spec amendment.
- Time budgets in gates are measured against real dynamics — do not shrink
  (SANDBOX_SETUP.md §9 rule).
- Disk discipline for browser gates: launch the fleet fresh (`stack_up.sh
  start-fleet` prunes old runs), stop it after (`stop-fleet`) — a 2-vehicle fleet
  writes ~0.5–0.7 MB/s/vehicle of ULog (live incident 2026-09-09: 3.8 GB / ~1.4 h
  idling filled the 9.9 GB sandbox disk). Gates must `stop-fleet` in teardown.

## 14. Risks & Mitigations

| # | Risk | Mitigation |
|---|---|---|
| R-1 | MapLibre worker misconfig → map mounts, no tiles (Turbopack + webpack both affected) | §6.2 pre-scripts copy BOTH sibling files; G-14 asserts tiles/pixels, not just `load` |
| R-2 | Headless browser gates: Chrome no longer auto-falls-back to SwiftShader → WebGL unavailable | gate launcher passes `--enable-unsafe-swiftshader`; assertions are pixel-presence, never fps; local re-verified at M8 (G-14 includes a headless render check) |
| R-3 | 2-vCPU sandbox perf: WS + map + React churn | §9.2 pipeline (no React state for numerics), pollers pause when panels closed, ring-buffer caps; G-20 soak gate |
| R-4 | Sandbox disk exhaustion from fleet ULogs during long gate sessions | §13.3 discipline: fresh fleet per gate, stop-fleet teardown; stack_up pruning already keeps newest run only |
| R-5 | Tile/basemap licensing drift (CARTO key terms, OSMF policy) | keyless-first ladder (OpenFreeMap primary); attribution gate G-14; N key cycles ladder for operator fallback |
| R-6 | Regression of v1 gate ladder during migration | legacy route `/?legacy=1` keeps old surfaces mountable until M14; every milestone re-runs the full ladder |
| R-7 | react-map-gl abstraction friction with layer catalog | engine-neutral layer contracts (§6.4) + vanilla fallback (§6.1); decision point at M8 with a 1-day spike |
| R-8 | Keyboard conflicts (inputs, Radix menus, MapLibre handler) | §7.3.2 capture-phase guards + open-state registry; G-15 asserts the param-search guard specifically |
| R-9 | Notification/event flood during fleet ops (auction + safety ladder) | queue caps (§8.6), state-chip vs event split, `events_tail` reconcile 30 s |
| R-10 | Scope creep toward a full dockable/floating window system | D4 fixed: edge furniture only; two-overlay cap (§4.2); spec amendments required for more |
| R-11 | Dual-mode (SIMULATED) divergence from LIVE shapes | normalizers are the single source (P8 consolidation) + mock engines reuse the same normalizer paths; G-14 asserts badge + behavior in forced-offline |
| R-12 | E-stop accidental trigger via keyboard | accepted residual risk (R5 doctrine) with keyup-fire + strip flash + immediate undo path = relaunch cycle documented; revisit only with operator feedback |

## 15. Appendices

### A. Keymap summary
§7.3.1 table is the binding list (31 bindings). No conflicts exist by construction:
letters = operator verbs (guarded), non-letters = camera (MapLibre), digits =
selection, `?`/`Esc`/`F8` = chrome.

### B. Context-menu tree summary
§7.2 M1..M5 is the binding tree; every item names its backend endpoint and guard.

### C. Token table
§5.1–§5.4 are the complete token set (13 colors, 3 type roles, spacing/radius/
elevation rules). Implementation: one `globals.css` block + Tailwind CSS-var
mapping; no hex literals outside the token file (lint rule at M14).

### D. Endpoint reference
§3.3 + the v1 inventory (worklog 2026-09-09, Task 11) enumerates all 45+ console
called routes with payloads; v2 calls exactly these, through the command bus.

### E. Layer reference
§6.4 L1..L13 is the complete layer catalog with sources, styling, and visibility
rules.

### F. Research sources (2026-09-09)
QGC fly/plan view docs + Blue Robotics shortcut thread + PX4 discuss · ArduPilot
Mission Planner flight-data docs + GitHub hotkey issue (2017) · Auterion Mission
Control docs (fly view, quick-actions sidebar, flight map) · UgCS 3.5 manual +
flight-planning-tools docs · DJI FlightHub 2 Virtual Cockpit announcement + GS Pro
page · FlytBase docs/releases · DroneDeploy help (Map Settings) · Google Maps/Earth
shortcut + context-menu support pages · Gmail/GitHub `?` cheat-sheet patterns ·
MDPI Future Internet 13(8):188 (2021, multi-operator web GCS study) · ResearchGate
GCS review (2024) · MapLibre GL JS docs (Next.js notes, v5→v6 migration, realtime
pattern, large-data guide), npm registry (maplibre-gl 6.9.0, react-map-gl 8.1.3),
OpenFreeMap, CARTO basemap terms, OSMF tile policy, chromestatus SwiftShader entry.
Full query→URL list preserved in the 2026-09-09 worklog (Tasks 10 and 12).

