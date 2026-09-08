# RustSim GCS v1 — Engineering Specification (SITL-only)

**Status:** Draft v0.1 — 2026-09-09
**Owners:** RustSim core team
**Scope:** `console/` (primary), `fleet/` (extensions), `sim/` (read-only consumer)
**Supersedes:** none
**Related:** `docs/ARCHITECTURE.md`, `docs/VERIFICATION.md`, `console/AGENTS.md`, `fleet/docs/adr/0016-vehicle-setup-control-plane.md`, `fleet/docs/adr/0017-operator-map-control-plane.md`, `fleet/docs/adr/0018-runtime-control-plane.md`

> This spec defines what it takes to turn the existing RustSim console into a
> QGroundControl / Mission Planner-class ground control station **for PX4 SITL
> only**. It is a binding engineering contract: every feature, gate, and
> milestone listed here is intended to be implemented, verified, and shipped in
> v1. Decisions deferred to ADRs are listed in §13; everything else is
> decided by this document.

---

## 1. Purpose

RustSim today is a simulation testbed with an operator console bolted on top.
The console already drives real PX4 SITL through the Operator Map (ADR-0017)
and the Vehicle Setup plane (ADR-0016), and the fleet manager already flies
auctioned missions end-to-end (F-2). What it does not yet have is the
**ground-control-station shape**: a permanent Plan View that lets an operator
design, save, version, upload, and re-fly missions; a Fly View that consolidates
all the live-flight widgets QGC operators expect; and an Analyze View that
turns the `.replay` and `.ulog` artifacts the harnesses already produce into
something an operator can scrub and plot.

This spec closes that gap. The deliverable is a single web application — the
existing Next.js console, extended — that an operator can sit in front of and
do everything they would do in QGC, **provided every vehicle is a PX4 SITL
process and not real hardware**. v1 is unapologetically SITL-only: real FC
flashing, serial links, USB discovery, and on-field operations are explicitly
out of scope (§3.2) and are not a future-flag hidden anywhere in this spec.

The motivation for SITL-only is not a limitation of ambition but a deliberate
scope boundary. Real-hardware GCS work is dominated by driver compatibility,
firmware update flows, regulatory airspace data, and field-replaceable
parameter sets — none of which are the RustSim project's core competency, and
all of which would dilute the simulator's value. By restricting v1 to SITL,
RustSim can ship a GCS that is the **best-in-class SITL operator tool**
rather than a mediocre general GCS, and the path to real hardware (if ever
taken) is a separate v2 spec, not a feature flag.

## 2. Background

### 2.1 What RustSim is today

RustSim is a three-component testbed:

- **`sim/` (RustSim Core)** — A lockstep HIL flight simulator that replaces
  Gazebo/jMAVSim entirely. It contains a hand-rolled MAVLink v2 codec
  (golden-vectored against PX4's own headers), a 6-DOF quadrotor dynamics
  model, BMI088-class sensor models with realistic noise, a 10-fault
  injection engine, deterministic replays, and a REST+WS control plane on
  port 8200+i per vehicle.
- **`fleet/` (RustSim Fleet)** — A multi-vehicle mission manager that
  spawns `sim` + PX4 SITL pairs, allocates tasks by sequential auction
  (Hungarian baseline), flies offboard missions at a 20 Hz setpoint cadence,
  enforces an 8-policy safety ladder, and writes run reports with
  CI-classifiable exit codes.
- **`console/` (RustSim Console)** — A Next.js 16 operator console with four
  permanently-mounted views: Sim Console (10 Hz physics telemetry, strip
  charts, live fault injection), Fleet C2 (fleet table, NED map, task board,
  event log, e-stop), Operator Map (Leaflet geo map: live GPS-marked
  vehicles, trajectories, geofence; click-to-fly Go To, plan/edit/upload/
  start waypoint missions, arm/takeoff/land/RTL/hold action bar), and
  Vehicle Setup (airframe/sensors/power/modes/params — the QGC-style
  configuration workflow).

The full stack speaks PX4 v1.16.2's native HIL TCP wire on 4560+i and
MAVLink UDP on 14540+i. Every protocol claim in the docs is backed by a
live capture against real PX4 SITL — see `docs/VERIFICATION.md` for the
I-1/I-2/F-1/F-2/S-1/S-2/O-1/O-2/R-1 ladder, all PASS as of 2026-09-08.

### 2.2 What QGroundControl and Mission Planner are

**QGroundControl (QGC)** is the reference open-source GCS for PX4 and
ArduPilot, written in C++/Qt. Its UI is organized into five top-level views:
**Plan** (mission editor with waypoint/survey/corridor patterns, geofence,
rally points), **Fly** (live map + HUD + instrument widgets + MAVLink
console), **Vehicle Setup** (airframe, sensors, radio, power, modes,
parameters — a full QGC-style configuration workflow), **Analyze** (log
browse, MAVLink console, MAVLink inspector, geo tag images), and
**Application Settings**. Mission files are JSON (`.plan` format),
cross-compatible with ArduPilot via the same MAVLink mission protocol.

**Mission Planner (MP)** is a Windows-only .NET GCS for ArduPilot. It is
heavier on mission planning features (advanced survey grids, corridor
patterns, photo mission planning with camera triggers) and log analysis
(graphing any logged parameter against any other, with statistical
reductions), but lacks QGC's clean five-view separation and modern UI.

Both target real hardware as the primary use case. SITL is a bolt-on
driven by `--serial-port=udp:` style configuration, not a first-class
citizen. Operators running SITL in either tool deal with simulated-link
quirks, fake-GPS caveats, and a tool surface that includes 90% real-hardware
machinery they will never touch.

### 2.3 The gap RustSim GCS v1 closes

The gap is the inverse of QGC/MP's posture. RustSim already has SITL as a
first-class citizen — the simulator and fleet manager are built around it.
What it lacks is the **GCS surface**: the Plan and Analyze views, and the
Fly View's instrument widgets and pre-arm checklist. Closing this gap means
an operator who wants to work against SITL (for teaching, for CI/CD on
autopilot code, for reproducible bug reports against PX4, for fleet-mission
rehearsal) reaches for RustSim GCS instead of QGC, because RustSim GCS does
SITL better than QGC does SITL.

The bet: in 2026, the population of operators running SITL is large and
growing — autopilot developers, CI pipelines, training programs, fleet
operators pre-rehearsing missions. None of them need a real Pixhawk. All of
them need a tool that takes SITL seriously.

## 3. Goals & Non-Goals

### 3.1 Goals (v1)

- **G-1 — Plan, fly, and analyze missions against N concurrent PX4 SITL
  vehicles** from a single browser tab. N is bounded by sandbox CPU/RAM
  (target: N=4 supported, N=8 best-effort).
- **G-2 — Five feature areas, each with its own view:** Plan View (mission
  editor), Fly View (live ops + instruments), Vehicle Setup (extend
  existing), Fleet C2 (extend existing), Analyze View (ULog + replay).
- **G-3 — Single deployable:** the existing Next.js console behind the Caddy
  :81 gateway. No new packaging, no Electron, no Tauri.
- **G-4 — Live + replay dual mode:** every view that consumes telemetry
  works against either a live SITL fleet or a `.replay` file loaded into a
  "replay vehicle" (the existing dual-mode pattern in `src/lib/conn.ts`).
- **G-5 — Verifiable via single-invocation harnesses** that extend the
  existing I/F/S/O/R ladder with new G gates (§10). No feature ships without
  a harness.

### 3.2 Non-Goals (v1)

- **Real hardware.** No USB, no serial, no Pixhawk, no real GPS, no real
  RC. The console talks only to PX4 SITL processes over TCP/UDP.
- **Firmware management.** No flashing, no bootloader interaction, no
  firmware downloads. SITL is "always flashed."
- **Production deployment hardening.** No auth, no multi-tenant, no audit
  log, no rate limiting. Single-operator single-host is the deployment shape.
- **Mobile-native apps.** Browser-only. PWA install is allowed but the
  install is still a web app, not a native iOS/Android binary.
- **Voice, video, chat comms.** Not in v1.
- **Airspace, weather, NOTAMs.** Not in v1. The Operator Map remains a
  Leaflet geo map with no airspace overlay.
- **Offline map tiles.** OSM tiles are fetched live. An offline tile bundle
  is a v1.1 candidate (ADR-0022).
- **ArduPilot support.** PX4 SITL only. ArduPilot SITL support is an open
  question (§14) but explicitly not committed.

## 4. Architecture

### 4.1 Component map — existing vs v1

| Component | Today | v1 changes |
|---|---|---|
| `sim/` | Lockstep HIL simulator, REST+WS control plane on :8200+i | Unchanged. Read-only consumer of v1 work. |
| `fleet/` | Mission manager, auction allocator, offboard execution, 8-policy safety ladder | Extended: new `fleet-mission` crate handles the MAVLink mission protocol (upload/download/count/ack). New REST endpoints on :8400 for mission CRUD and per-vehicle mission binding. |
| `console/` | Next.js 16 app, 4 mounted views | Extended: 3 new top-level views (Plan, Fly consolidated from existing Operator Map + Sim Console, Analyze). Existing Operator Map becomes the Plan View's map widget; Sim Console becomes part of Fly View's telemetry panel. |
| `docs/` | Architecture, verification, ops, sandbox setup, deployment | New: this GCS_SPEC.md. |
| New: `console/docs/adr/` | — | New ADR home for GCS decisions, numbered 0019+ to continue the cross-repo sequence. |

The architectural principle is **minimum churn**: `sim/` and `fleet/` are
the engine, `console/` is the GCS app. v1 extends the console with new
views and extends the fleet control plane with new endpoints, but does not
restructure either workspace. The existing live-verified ladder (I/F/S/O/R)
must continue to pass after every v1 milestone; nothing in v1 may regress
those gates.

### 4.2 Topology

```
┌─────────────────────────────────────────────────────────────┐
│  Browser (operator)                                         │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  RustSim GCS (Next.js 16, console/)                  │  │
│  │  Views: Plan | Fly | Setup | Fleet C2 | Analyze      │  │
│  │  Routing: gateway mode (?XTransformPort=<port>)     │  │
│  └──────────────────────────────────────────────────────┘  │
└──────────────────────────┬──────────────────────────────────┘
                           │ HTTP + WS to :81
┌──────────────────────────▼──────────────────────────────────┐
│  Caddy gateway :81 (XTransformPort)                        │
│  ┌───────────────┬────────────────┬────────────────────┐   │
│  │  :8200+i      │  :8400          │  :8300 (new)       │   │
│  │  per-vehicle  │  fleet manager  │  mission catalog   │   │
│  │  sim control  │  (existing)     │  + replay server   │   │
│  │  plane        │                 │  (new in v1)        │   │
│  └───────┬───────┴────────┬────────┴─────────┬──────────┘   │
└──────────┼────────────────┼──────────────────┼──────────────┘
           │                │                  │
   ┌───────▼───────┐  ┌─────▼─────┐    ┌──────▼──────┐
   │  RustSim Core │  │  fleet-   │    │  fleet-     │
   │  (sim/)       │  │  cli      │    │  mission    │
   │  :8200+i      │  │  :8400    │    │  :8300      │
   └───────┬───────┘  └─────┬─────┘    └─────────────┘
           │ TCP HIL         │ MAVLink UDP
           │ :4560+i         │ :14540+i
   ┌───────▼───────┐  ┌──────▼──────┐
   │  PX4 SITL     │  │  PX4 SITL   │
   │  v1.16.2      │  │  v1.16.2    │
   │  instance i   │  │  instance i │
   └───────────────┘  └─────────────┘
```

The new `:8300` mission catalog + replay server is the only new control
plane introduced by v1. It is a new `fleet-mission` binary (or an extension
of `fleet-cli` with a `serve` subcommand — ADR-0020) that owns the mission
file store, mission validation, and the replay file server. Keeping it
separate from `:8400` lets the mission catalog stay up while the fleet
manager is being torn down between runs (the existing harnesses do this
frequently).

### 4.3 Port map (existing, extended)

| Port | Owner | Purpose | New in v1? |
|---|---|---|---|
| :81 | Caddy | Single network entry point | No |
| :3000 | console (dev) | Next.js dev server | No |
| :4560+i | sim ↔ PX4 | HIL TCP lockstep, 200 Hz | No |
| :8200+i | sim control plane | REST + WS per vehicle | No |
| :8400 | fleet manager | REST + WS fleet control | No |
| :8300 | **fleet-mission (new)** | REST mission catalog + replay server | **Yes** |
| :14540+i | PX4 ↔ fleet | MAVLink UDP | No |

### 4.4 Data flows

**Plan → Fly flow.** An operator designs a mission in the Plan View (map
clicks, table edits, geofence drawing). The mission is saved as a TOML
file in the mission catalog (`:8300`). To fly it, the operator selects a
target vehicle and clicks Upload; `:8300` translates the mission into
MAVLink `MISSION_ITEM_INT` messages and sends them through `:8400`'s
existing offboard-mission upload path; PX4 SITL acknowledges each item.
The operator switches to Fly View, selects the vehicle, arms via the action
bar, and clicks Start Mission; `:8400` issues `MAV_CMD_MISSION_START`.

**Fly → Analyze flow.** During a flight, the sim control plane on `:8200+i`
writes the `.replay` file (deterministic tick-by-tick record). PX4 SITL
writes a `.ulg` file in its build/instance dir. After the flight, the
operator opens Analyze View, picks the `.replay` (or `.ulg`), and scrubs
the timeline; `:8300` streams the file over WS in chunks. Topics are
plotted in ECharts strip charts; the map view replays the trajectory.

## 5. Feature Areas

Each feature area below has the same structure: user stories, acceptance
criteria, UX flow, API contracts (new endpoints only — existing endpoints
are in `sim/docs/PROTOCOL.md` and `fleet/docs/SPEC.md`), data models, and
verification gates.

### 5.1 Mission Planning (Plan View) — NEW

**What it is.** The QGC Plan View analog: a permanent top-level view where
an operator designs, edits, validates, saves, versions, uploads, and
downloads missions. The map widget reuses the existing Leaflet Operator
Map; the surrounding editor (waypoint table, altitude editor, geofence
panel, rally points, validation status) is new.

**User stories.**

- As an operator, I can click on the map to add a waypoint at the clicked
  lat/lon, drag a waypoint to move it, and right-click to delete, so that I
  can design a mission visually.
- As an operator, I can edit waypoint altitude, hold time, accept radius,
  and pass-radius in a side table, so that I can fine-tune mission
  parameters without leaving the map.
- As an operator, I can draw an inclusion polygon and an exclusion polygon
  for the geofence, set ceiling and floor altitudes, and the editor
  validates that every waypoint is inside the inclusion and outside the
  exclusion, so that I cannot save a mission that would breach the fence.
- As an operator, I can save a mission with a name and have it persist
  across browser sessions, list saved missions, duplicate, rename, and
  delete them, so that I have a mission library.
- As an operator, I can upload a saved mission to a selected vehicle (PX4
  SITL), watch the upload progress per-item with ack status, and download
  the current mission from a vehicle to compare with the saved version.

**Acceptance criteria.**

- AC-5.1.1: A mission with 0 waypoints fails validation with a clear
  message ("mission must have at least one waypoint") and cannot be saved
  or uploaded.
- AC-5.1.2: A mission with a waypoint outside the inclusion geofence fails
  validation; the offending waypoint is highlighted on the map.
- AC-5.1.3: A saved mission persists across browser reloads and across
  console restarts (it lives in `:8300`'s file store, not in browser
  localStorage).
- AC-5.1.4: Upload to a vehicle that is disarmed and in `STANDBY` state
  succeeds; upload to a vehicle in `ACTIVE` or `FAULT` state returns HTTP
  409 with a clear message.
- AC-5.1.5: Download from a vehicle returns the mission currently loaded in
  PX4's mission pool, not the one `:8300` last uploaded (i.e. it queries
  PX4, not the catalog).

**UX flow — plan and upload a mission.**
1. Operator opens Plan View (tab is permanently mounted).
2. Map initializes; existing Leaflet pattern from Operator Map (must call
   `invalidateSize()` before `fitBounds`, see `console/AGENTS.md`).
3. Operator clicks "New Mission"; editor enters edit mode.
4. Operator clicks on the map 4 times → 4 waypoints appear, connected by
   a polyline, with numbered markers.
5. Operator sets each waypoint's altitude in the side table (default 12 m).
6. Operator draws an inclusion geofence polygon around the waypoints.
7. Operator clicks Save → enters name "demo-square" → confirms.
8. Operator selects target vehicle from a dropdown (populated from `:8400`
   `/api/vehicles`).
9. Operator clicks Upload → progress bar shows "3/4 ack'd" then "4/4 ack'd"
   → toast "Mission uploaded to vehicle 0".
10. Operator switches to Fly View (the mounted tab is preserved).

**API contracts (new).**

- `POST /api/missions` — create mission in catalog. Body: mission TOML or
  JSON. Returns: `{id, version}`.
- `GET /api/missions` — list all saved missions. Returns: `[{id, name,
  version, updated_at, waypoint_count}]`.
- `GET /api/missions/{id}` — fetch a mission by id. Returns: full mission
  TOML/JSON.
- `PUT /api/missions/{id}` — update. Body: mission. Returns: `{version}`.
  Version is bumped; old version is retained for diff.
- `DELETE /api/missions/{id}` — soft-delete (catalog retains a tombstone
  for replay referential integrity).
- `POST /api/missions/{id}/validate` — schema + geofence geometry
  validation. Returns: `{valid, errors: [{waypoint_seq, message}]}`.
- `POST /api/vehicles/{i}/mission/upload?mission_id={id}` — upload via
  MAVLink mission protocol. Returns: `{items_sent, items_acked,
  failed_items: []}`.
- `GET /api/vehicles/{i}/mission` — download current mission from PX4.
  Returns: mission JSON.
- `POST /api/vehicles/{i}/mission/start` — `MAV_CMD_MISSION_START`.
- `POST /api/vehicles/{i}/mission/abort` — issue RTL or Hold (configurable).

**Data model.**

```toml
# Mission TOML format — see ADR-0019 for the format decision
[mission]
id = "01J8K2..."           # ULID, sortable
name = "demo-square"
version = 3
created_at = "2026-09-09T10:00:00Z"
updated_at = "2026-09-09T10:05:00Z"
vehicle_type = "quad"      # for validation: max speed, climb rate

[[waypoints]]
seq = 0
frame = 3                  # MAV_FRAME_GLOBAL_RELATIVE_ALT
command = 16               # MAV_CMD_NAV_WAYPOINT
x = 37.4135                # lat
y = -122.1015              # lon
z = 12.0                   # alt_m relative to home
param1 = 0.5               # hold_s
param2 = 2.0               # accept_radius_m
param3 = 0.0               # pass_radius_m
param4 = 0.0               # yaw_deg (NaN = face direction of travel)

# ... more waypoints ...

[geofence]
ceiling_m = 60
floor_m = 0
inclusion = [[37.4130, -122.1020], [37.4140, -122.1020],
             [37.4140, -122.1010], [37.4130, -122.1010]]   # lat, lon polygon
exclusion = []                                              # list of polygons

[[rally]]
lat = 37.4133
lon = -122.1014
alt_m = 0.0
```

**Verification gates.** G-1 (validation), G-2 (persistence CRUD round-trip),
G-3 (upload ack), G-4 (download round-trip). See §10.

### 5.2 Live Flight Ops (Fly View) — EXTEND EXISTING

**What it is.** The QGC Fly View analog: a live map with vehicle markers
and trajectories, an attitude HUD, instrument widgets (battery, signal,
mode, armed state, GPS fix, EKF status), a pre-arm checklist, and the
action bar (arm/takeoff/land/RTL/hold/goto/mission-start). The existing
Sim Console (10 Hz strip charts) becomes a side panel within Fly View;
the existing Operator Map becomes the main map widget. Fly View also
replaces the standalone Operator Map for live ops — Operator Map remains
in the codebase as the Plan View's map widget, but no longer has its own
top-level tab.

**User stories.**

- As an operator, I can see all connected vehicles on the map
  simultaneously, each with a distinct color, a velocity vector, and a
  trajectory trail, so that I have fleet awareness.
- As an operator, I can select one vehicle as "active" (clicking it on the
  map or in the fleet table) — the action bar, instrument widgets, and
  strip charts all reflect the active vehicle, so that I can operate one
  vehicle at a time without losing fleet context.
- As an operator, the action bar shows the current state of the active
  vehicle (disarmed/armed/in-mission/landed) and only enables actions
  valid in that state, so that I cannot issue an invalid command.
- As an operator, the pre-arm checklist runs automatically when I click
  Arm and shows me which checks passed/failed (EKF2, GPS fix, mode, fence)
  so that I know why arming might be blocked.
- As an operator, I can click any point on the map to issue a Go To command
  to the active vehicle, with the destination validated against the
  geofence before sending, so that I cannot command a breach.

**Acceptance criteria.**

- AC-5.2.1: With 2 vehicles connected, both appear on the map within 1 s of
  the first heartbeat, with distinct colors and live position updates at
  10 Hz.
- AC-5.2.2: Selecting vehicle 1 as active updates the action bar,
  instruments, and strip charts within 100 ms; selecting vehicle 2 does
  the same.
- AC-5.2.3: Arm is disabled (button greyed out, tooltip explains why) when
  the active vehicle is in `ACTIVE` or `FAULT` state, or when a pre-arm
  check is failing.
- AC-5.2.4: Go To with a destination outside the inclusion geofence is
  rejected client-side with a toast; no REST call is made.
- AC-5.2.5: The strip chart updates at 10 Hz for 60 s of history, then
  scrolls (no history cap, but rendering is throttled to 10 Hz).

**UX flow — fly a planned mission.** (Continues from §5.1 step 10.)
1. Operator is in Fly View; the mission uploaded to vehicle 0 is visible
   on the map as a dashed polyline.
2. Operator selects vehicle 0 as active (it's already highlighted).
3. Action bar shows: DISARMED. Pre-arm checks all green.
4. Operator clicks Arm → action bar transitions to ARMED in <500 ms.
5. Operator clicks Start Mission → action bar shows IN MISSION, map shows
   the vehicle moving along the polyline, strip chart shows altitude
   climbing to 12 m.
6. Operator can click Hold mid-mission to pause; vehicle holds position.
7. Operator clicks Resume → mission continues.
8. Mission completes → vehicle auto-RTLs (per mission plan) → lands →
   disarms → action bar shows DISARMED with a green "mission complete"
   indicator.

**API contracts (new).**

- `GET /api/vehicles/{i}/prearm-checks` — returns list of checks and
  pass/fail state. Server-side, derived from the active vehicle's health
  flags (EKF2, GPS, mode, fence).
- `POST /api/vehicles/{i}/goto` — body: `{lat, lon, alt_m}`. Validates
  against fence (server-side too, defense in depth).
- (existing) `/api/vehicles/{i}/{arm,takeoff,land,rtl,hold}` — unchanged.

**Verification gates.** G-5 (Fly View 1-vehicle telemetry), G-6
(multi-vehicle Fly View), G-7 (pre-arm + arm/disarm round-trip).

### 5.3 Vehicle Setup — EXTEND EXISTING (S-1/S-2 already verified)

**What it is.** The QGC Vehicle Setup view, already implemented and
verified (S-1: 44 REST checks, S-2: 13 browser checks). v1 extends it
with: parameter search/filter, parameter preset save/load, calibration
status indicators, and a "diff against defaults" view.

**User stories.**

- As an operator, I can search parameters by name substring (case-
  insensitive) and filter by group, so that I can find the parameter I
  need in a list of 1000+.
- As an operator, I can save the current parameter set as a named preset
  ("test-1", "high-wind-config"), load a preset back, and delete presets,
  so that I can switch between configurations quickly.
- As an operator, I can see which sensors are calibrated (gyro, accel,
  mag, level) and click to re-calibrate, so that I know the vehicle is
  ready to fly.
- As an operator, I can see a diff between the current parameter set and
  PX4's defaults (parameters that have been changed from default are
  highlighted), so that I know what's been customized.

**Acceptance criteria.**

- AC-5.3.1: Parameter search filters the list within 100 ms for 1000+
  parameters, with debounced input (250 ms).
- AC-5.3.2: A preset save → load round-trip restores every parameter to
  the saved value; a diff between the saved and restored state is empty.
- AC-5.3.3: Calibration status indicators update within 1 s of the
  underlying PX4 flag changing.
- AC-5.3.4: The diff-against-defaults view highlights only parameters
  whose current value differs from PX4's compiled-in default, with the
  default value shown alongside.

**API contracts (new).**

- `GET /api/vehicles/{i}/params?search={q}&group={g}` — list with
  optional filters. Returns: `[{name, value, type, group, default,
  is_changed}]`.
- `GET /api/vehicles/{i}/param-presets` — list presets. Returns: `[{name,
  created_at, param_count}]`.
- `POST /api/vehicles/{i}/param-presets` — save current as preset.
- `POST /api/vehicles/{i}/param-presets/{name}/load` — load preset.
- `DELETE /api/vehicles/{i}/param-presets/{name}`.

**Verification gate.** G-8 (extends S-1 + S-2 with search, preset
round-trip, diff-against-defaults).

### 5.4 Multi-Vehicle Fleet C2 — EXTEND EXISTING (F-1/F-2 already verified)

**What it is.** The existing Fleet C2 view, already verified (F-1: bring-up
+ e-stop, F-2: 2-vehicle auctioned mission). v1 adds: per-vehicle mission
binding (different missions to different vehicles), fleet-wide mission
orchestration (sequential or parallel), and a swarming-pattern library
(form-on-leader, follow-the-leader, search-grid).

**User stories.**

- As an operator, I can assign different missions to different vehicles
  (vehicle 0 → patrol mission A, vehicle 1 → survey mission B), then
  start both with a single "Start Fleet" button.
- As an operator, I can choose orchestration mode: parallel (all vehicles
  start their missions at once) or sequential (vehicle 1 starts after
  vehicle 0 reaches its first waypoint), so that I can do staggered
  takeoffs for safety or column flights for time-efficiency.
- As an operator, I can load a swarming pattern from the library
  (follow-the-leader), assign a leader vehicle, and the fleet manager
  auto-generates per-vehicle missions that maintain formation.

**Acceptance criteria.**

- AC-5.4.1: Assigning mission A to vehicle 0 and mission B to vehicle 1,
  then clicking Start Fleet, results in both vehicles flying their
  assigned missions; the fleet table shows per-vehicle mission progress.
- AC-5.4.2: Sequential orchestration starts vehicle 1 only after vehicle 0
  reaches its first waypoint, with a configurable timeout (default 30 s)
  that aborts the fleet if the gate is not met.
- AC-5.4.3: Follow-the-leader pattern with 2 vehicles maintains a
  configurable separation (default 10 m horizontal, 2 m vertical) for
  the duration of the leader's mission.

**API contracts (new).**

- `POST /api/fleet/mission-bindings` — body: `[{vehicle_id, mission_id}]`.
  Binds missions to vehicles.
- `POST /api/fleet/start` — body: `{mode: "parallel" | "sequential",
  sequential_gate: "first_waypoint" | "takeoff_complete", timeout_s: 30}`.
- `GET /api/fleet/patterns` — list available swarming patterns.
- `POST /api/fleet/patterns/{name}/generate` — body: pattern params.
  Returns: generated per-vehicle missions.

**Verification gates.** G-9 (per-vehicle mission binding), G-10 (sequential
+ parallel orchestration).

### 5.5 Log / ULog Analysis (Analyze View) — NEW

**What it is.** The QGC Analyze View analog: a view for browsing the
`.replay` files (RustSim's own deterministic record) and the `.ulg` files
(PX4's standard ULog format) produced by SITL runs, plotting any topic
over time, and scrubbing a flight replay with the map and telemetry
re-rendering.

**User stories.**

- As an operator, I can see a list of recent `.replay` and `.ulg` files
  (sorted by mtime), with metadata (duration, vehicle count, scenario
  name), and click one to open it in the replay viewer.
- As an operator, I can scrub the timeline of a replay with a slider and
  the map re-renders the vehicle position at that tick, the strip charts
  show the values at that tick, and the attitude HUD shows the orientation
  at that tick.
- As an operator, I can plot any logged topic (e.g. `vehicle_local_position`,
  `sensor_combined.gyro_rad`) against time in an ECharts strip chart, and
  overlay multiple topics on the same chart with shared x-axis.
- As an operator, I can overlay two flights (live vs replay, or replay A
  vs replay B) on the same map and same chart, so that I can compare
  performance across runs.

**Acceptance criteria.**

- AC-5.5.1: A `.replay` file with 200 Hz × 60 s (12,000 ticks) opens within
  2 s and the timeline scrubber is responsive (<100 ms per scrub step).
- AC-5.5.2: Plotting any topic from a `.ulg` file (≤1 MB) returns within
  1 s; the chart supports pan, zoom, and crosshair.
- AC-5.5.3: Overlaying two flights on the same map renders both trajectories
  with distinct colors and a legend; scrubbing is locked to the longer
  flight's timeline.
- AC-5.5.4: A `.replay` loaded into a "replay vehicle" is indistinguishable
  from a live vehicle in Fly View (the existing dual-mode pattern from
  `console/AGENTS.md` extends naturally).

**UX flow — analyze a flight.**
1. Operator opens Analyze View (new tab, permanently mounted).
2. List shows recent runs: `i2_flight.replay`, `f2_demo.replay`,
   `instance_0.ulg` (auto-discovered from `sim/tests/i2_artifacts/` and
   `fleet/tests/f2_artifacts/`).
3. Operator clicks `f2_demo.replay` → opens in the viewer.
4. Timeline scrubber at the bottom shows the full 3-minute flight.
5. Operator scrubs to tick 6000 (30 s in) → map shows both vehicles mid-
  mission, attitude HUDs show their orientations, strip charts show the
  values at that tick.
6. Operator clicks "Add Plot" → selects `vehicle_local_position.z` →
  ECharts strip chart shows altitude over time for both vehicles.
7. Operator clicks "Overlay Live" → selects vehicle 0 (currently running
  a new mission) → its live trajectory is overlaid on the replay trajectory
  in green vs blue.

**API contracts (new).**

- `GET /api/replays` — list available `.replay` files. Returns: `[{filename,
  duration_s, vehicle_count, scenario_sha256, mtime}]`.
- `GET /api/replays/{file}/meta` — header info (tick_rate_hz, seed,
  scenario_sha256, records, virtual_duration_s).
- `WS /ws/replays/{file}/stream?from_tick={n}&to_tick={m}` — stream a
  range of ticks as JSON records; client controls pacing.
- `GET /api/ulogs` — list available `.ulg` files.
- `GET /api/ulogs/{file}/topics` — list topic names in the ULog.
- `GET /api/ulogs/{file}/topics/{name}/data?from_s={a}&to_s={b}` — fetch
  a time range of a single topic as JSON `{t: [...], values: [...]}`.

**Data model.** `.replay` files are RustSim's existing format
(`sitsim-cli replay-info` parses them; format documented in
`sim/docs/SPEC.md` §8.1). `.ulg` files are PX4's standard ULog binary
format; parsing is done server-side via `pyulog` (already installed per
`SANDBOX_SETUP.md` §5) exposed through `:8300`'s REST endpoints — no
browser-side ULog parser is written in v1 (ADR-0021).

**Verification gates.** G-11 (ULog browse + plot), G-12 (replay scrub +
overlay).

## 6. Data Models (consolidated)

### 6.1 Mission (v1)

See §5.1 data model block. Format: TOML on disk (ADR-0019); JSON via the
REST API. Versioned: every save creates a new version, old versions
retained for diff. Soft-deleted (tombstone retained for replay
referential integrity).

### 6.2 Vehicle runtime state

Inherited from the existing `fleet-cli` vehicle FSM (`fleet/docs/SPEC.md`
§3.2): `INIT → READY → ACTIVE → (LANDING | RETURNING | HOLDING) → LANDED
→ DISARMED`, with `FAULT` as a terminal-from-anywhere state. v1 adds no
new states; the GCS UI maps these to colors and action-bar states.

### 6.3 Telemetry sample

The existing 10 Hz sample from the sim control plane (`sim/docs/SPEC.md`
§4): `{tick, t_s, pos_ned_m: [x,y,z], vel_ned_ms: [x,y,z], q_wxyz:
[w,x,y,z], battery_v, battery_pct, ...}`. v1 consumes this unchanged;
the Analyze View also reads the 200 Hz replay record (same fields, 20×
the density).

### 6.4 Parameter

Inherited from `fleet-cli`'s existing param API (S-1 verified). Each
param: `{name, value, type: "int|float|enum|string", group, default,
min, max, is_changed}`. v1 adds `is_changed` (true if value ≠ default)
to support the diff-against-defaults view.

### 6.5 Mission binding

New: `{vehicle_id, mission_id, binding_state: "assigned|uploaded|
active|complete|aborted"}`. Tracks the lifecycle of a mission's
relationship to a vehicle across the Plan→Fly flow.

## 7. API Contracts (consolidated new endpoints)

All new endpoints live on `:8300` (mission catalog + replay server) or
extend `:8400` (fleet manager). Existing endpoints on `:8200+i` (sim
control plane) are unchanged.

| Method | Path | Owner | Purpose |
|---|---|---|---|
| POST | `/api/missions` | :8300 | Create mission |
| GET | `/api/missions` | :8300 | List missions |
| GET | `/api/missions/{id}` | :8300 | Fetch mission |
| PUT | `/api/missions/{id}` | :8300 | Update mission (bumps version) |
| DELETE | `/api/missions/{id}` | :8300 | Soft-delete mission |
| POST | `/api/missions/{id}/validate` | :8300 | Schema + fence validation |
| POST | `/api/vehicles/{i}/mission/upload?mission_id={id}` | :8400 | MAVLink upload |
| GET | `/api/vehicles/{i}/mission` | :8400 | MAVLink download |
| POST | `/api/vehicles/{i}/mission/start` | :8400 | `MAV_CMD_MISSION_START` |
| POST | `/api/vehicles/{i}/mission/abort` | :8400 | RTL or Hold |
| GET | `/api/vehicles/{i}/prearm-checks` | :8400 | Pre-arm check list |
| POST | `/api/vehicles/{i}/goto` | :8400 | Go To validated against fence |
| GET | `/api/vehicles/{i}/params?search&q&group` | :8400 | Search/filter params |
| GET | `/api/vehicles/{i}/param-presets` | :8300 | List presets |
| POST | `/api/vehicles/{i}/param-presets` | :8300 | Save preset |
| POST | `/api/vehicles/{i}/param-presets/{name}/load` | :8300 | Load preset |
| DELETE | `/api/vehicles/{i}/param-presets/{name}` | :8300 | Delete preset |
| POST | `/api/fleet/mission-bindings` | :8400 | Bind missions to vehicles |
| POST | `/api/fleet/start` | :8400 | Start fleet (parallel/sequential) |
| GET | `/api/fleet/patterns` | :8400 | List swarming patterns |
| POST | `/api/fleet/patterns/{name}/generate` | :8400 | Generate per-vehicle missions |
| GET | `/api/replays` | :8300 | List `.replay` files |
| GET | `/api/replays/{file}/meta` | :8300 | Replay header |
| WS | `/ws/replays/{file}/stream?from_tick&to_tick` | :8300 | Stream replay range |
| GET | `/api/ulogs` | :8300 | List `.ulg` files |
| GET | `/api/ulogs/{file}/topics` | :8300 | List ULog topics |
| GET | `/api/ulogs/{file}/topics/{name}/data?from_s&to_s` | :8300 | Fetch ULog topic data |

All endpoints return JSON; errors use `{error: {code, message,
details?}}` with appropriate HTTP status codes (400 validation, 404 not
found, 409 conflict, 422 unprocessable, 500 internal). All `:8300`
endpoints are accessible through the Caddy gateway via
`?XTransformPort=8300`, consistent with the existing dual-mode routing
pattern in `console/AGENTS.md`.

## 8. UX Flows (consolidated)

The five views are mounted permanently (hidden by CSS, never unmounted)
so that telemetry engines survive tab switches — this is the existing
console pattern (see `console/AGENTS.md` Local Contracts). The tab bar
reflects the five views in order: **Plan → Fly → Setup → Fleet C2 →
Analyze**. The order matches the operator's typical workflow (design →
fly → configure → orchestrate → review).

### 8.1 Plan a mission → Fly it

See §5.1 (plan steps 1-10) and §5.2 (fly steps 1-8). The transition from
Plan View to Fly View preserves the active-vehicle selection and the
uploaded-mission polyline; Fly View reads the mission back from PX4 on
mount to confirm.

### 8.2 Replay a previous flight

See §5.5 UX flow. The replay viewer is a separate sub-component within
Analyze View; it does not interfere with live ops (live vehicles remain
visible in Fly View).

### 8.3 Configure a vehicle, save preset, restore later

1. Operator opens Vehicle Setup, selects vehicle 0.
2. Operator searches `MPC_XY_VEL_MAX` → finds it → sets to 8.0.
3. Operator searches `MC_ROLLRATE_P` → sets to 7.5.
4. Operator clicks "Save as preset" → enters name "aggressive-corners".
5. Later: operator opens Vehicle Setup, clicks "Load preset" →
   "aggressive-corners" → params restored.
6. The diff-against-defaults view now shows two changed params (the two
   just set), highlighted.

### 8.4 Fly a 2-vehicle fleet mission with sequential orchestration

1. Operator assigns mission A to vehicle 0 and mission B to vehicle 1
   in Plan View.
2. Operator opens Fleet C2, clicks "Start Fleet" → selects
   `mode: sequential, gate: first_waypoint, timeout: 30`.
3. Vehicle 0 takes off, flies to its first waypoint.
4. Vehicle 0 reaches first waypoint → fleet manager starts vehicle 1's
   mission → vehicle 1 takes off.
5. Fleet table shows both vehicles IN MISSION, with per-vehicle progress
   bars.
6. Either vehicle finishes → auto-RTL → lands → disarms → its row in
   the fleet table shows COMPLETE.
7. Both vehicles complete → fleet table shows FLEET COMPLETE green banner.

## 9. Verification Gates (G-ladder)

The G-ladder extends the existing I/F/S/O/R ladder recorded in
`docs/VERIFICATION.md`. Each gate is a single-invocation harness under
`console/tests/` (or `scripts/` for cross-cutting ones), runs against
real PX4 SITL, asserts PASS/FAIL with an exit code, and is named with a
`G-` prefix to distinguish from the existing ladder.

| Gate | Name | What it asserts | Harness path | Est. runtime |
|---|---|---|---|---|
| G-1 | Mission validation | Schema + geofence geometry; rejects empty/out-of-fence missions | `console/tests/run_g1_validation.sh` | ~30 s |
| G-2 | Mission persistence | CRUD round-trip (create, list, fetch, update, delete); survives catalog restart | `console/tests/run_g2_persistence.sh` | ~30 s |
| G-3 | Mission upload | MAVLink mission protocol: count → items → ack; all items ack'd by PX4 | `console/tests/run_g3_upload.sh` | ~1 min |
| G-4 | Mission download | Download from PX4 after upload; round-trip equality (seq, command, x, y, z) | `console/tests/run_g4_download.sh` | ~1 min |
| G-5 | Fly View 1-vehicle telemetry | 10 Hz telemetry moves on map + strip + attitude HUD for 60 s | `console/tests/run_g5_flyview.sh` | ~1.5 min |
| G-6 | Multi-vehicle Fly View | 2 vehicles on map simultaneously, select-active switch <100 ms | `console/tests/run_g6_multivehicle.sh` | ~2 min |
| G-7 | Pre-arm + arm/disarm | Pre-arm checks run, block arm when failing, pass when fixed, arm→disarm round-trip | `console/tests/run_g7_arm.sh` | ~2 min |
| G-8 | Vehicle Setup extensions | Param search filter, preset save/load round-trip, diff-against-defaults | `console/tests/run_g8_setup_ext.sh` | ~2 min |
| G-9 | Fleet mission binding | Vehicle 0 mission A + vehicle 1 mission B, both fly correctly | `console/tests/run_g9_binding.sh` | ~3 min |
| G-10 | Fleet orchestration | Sequential mode: vehicle 1 starts after vehicle 0 reaches WP1; parallel mode: both start together | `console/tests/run_g10_orchestration.sh` | ~3 min |
| G-11 | ULog browse + plot | List `.ulg` files, list topics, fetch topic data, plot in chart | `console/tests/run_g11_ulog.sh` | ~1 min |
| G-12 | Replay scrub + overlay | Load `.replay`, scrub timeline, plot topic, overlay live vehicle | `console/tests/run_g12_replay.sh` | ~1.5 min |

Each harness must follow the existing single-invocation pattern: start
→ assert → teardown in one shell call, with no background processes
surviving between calls. The existing flakiness notes (e.g., the F-1
READY poll race, see `docs/SANDBOX_SETUP.md` §9) apply; new harnesses
that depend on FSM transitions must use the f2-style "READY or any
post-READY state" gate, not strict `fsm == "READY"`.

## 10. Milestone Roadmap

The roadmap is sequenced so that each milestone is independently
shippable and independently verifiable. Milestone exit criteria are the
verification gates listed in §9.

| Milestone | Scope | Gates | Est. effort |
|---|---|---|---|
| M1: Plan View MVP | Mission editor, TOML persistence, CRUD endpoints, validation | G-1, G-2 | 2 weeks |
| M2: Mission upload/download | MAVLink mission protocol integration with fleet-cli; upload + download round-trip | G-3, G-4 | 1.5 weeks |
| M3: Fly View extensions | Instrument widgets, multi-vehicle active-select, pre-arm checklist | G-5, G-6, G-7 | 2 weeks |
| M4: Vehicle Setup extensions | Param search, presets, diff-against-defaults | G-8 | 1 week |
| M5: Fleet mission orchestration | Per-vehicle binding, parallel + sequential modes, swarming patterns | G-9, G-10 | 2.5 weeks |
| M6: Analyze View | ULog browser, replay viewer, overlay | G-11, G-12 | 3 weeks |
| **v1 release** | All 12 gates green; existing I/F/S/O/R ladder re-run green; README + docs updated | I/F/S/O/R + G-1..G-12 | **~12 weeks total** (one engineer, sequential) |

M1 and M3 can be parallelized across two engineers. M2 depends on M1.
M4 is independent (can run parallel to any of M2/M3). M5 depends on M2.
M6 is independent (can run parallel to M3/M4/M5). With two engineers,
total wall-clock time drops to ~8 weeks.

## 11. Risks & Mitigations

- **R-1: PX4 v1.16.2 mission protocol dialect drift.** The README and
  ADRs already document PX4's pinned v1.16.2 divergences from
  `common.xml` (the `HIL_ACTUATOR_CONTROLS` size-sorted layout, the
  per-motor normalized thrust, the `DO_SET_MODE` param layout). The
  mission protocol (`MISSION_ITEM_INT`, `MISSION_COUNT`,
  `MISSION_ACK`) may have similar drift that only live capture reveals.
  **Mitigation:** golden-vector capture of a known mission upload, ADR
  per divergence discovered, all captured in `fleet/docs/adr/`.

- **R-2: ULog parsing in the browser.** `pyulog` is Python; running it
  in the browser would require a WASM port or a JS reimplementation.
  **Mitigation:** all ULog parsing stays server-side (`:8300` uses
  `/usr/bin/python3.13 -m pyulog` per `SANDBOX_SETUP.md` §5); the
  browser only consumes JSON. ADR-0021 records this decision. If a
  future v1.1 wants pure-client ULog parsing, that's a separate ADR.

- **R-3: Multi-vehicle UI complexity.** QGC's solution is the
  "active vehicle" pattern: one vehicle is active, the rest are
  visible-but-passive. RustSim v1 adopts this pattern (§5.2).
  **Mitigation:** document the active-vehicle contract in
  `console/AGENTS.md` and add a UI test (part of G-6) that asserts
  switching active vehicles updates all dependent widgets within 100 ms.

- **R-4: Mission persistence corruption.** If the catalog's file store
  is corrupted (disk full, crash mid-write), the operator could lose
  missions. **Mitigation:** writes are atomic (write to temp file, fsync,
  rename); old versions are retained (§5.1); a recovery tool can
  rebuild from the version history. ADR-0020 records the
  file-format-vs-SQLite-vs-sled decision.

- **R-5: Map tile availability.** OSM tiles are fetched live; if the
  operator is offline (or the tile server is rate-limiting), the map
  goes blank. **Mitigation:** v1 accepts this limitation (it's a
  known issue); v1.1 may bundle an offline tile set (ADR-0022). For
  v1, the console shows a clear "tiles unavailable — connect to
  internet" message instead of a blank map.

- **R-6: Replay file size growth.** A 60 s flight at 200 Hz is ~12,000
  records, ~1 MB; a 10-minute fleet run is ~10 MB. WS streaming in
  ranges (§5.5) mitigates the in-browser memory cost. **Mitigation:**
  the WS endpoint streams ranges; the client never holds the whole
  file in memory. ADR-0024 records the streaming protocol choice.

- **R-7: Concurrency on `:8300`.** The mission catalog + replay server
  is a single process; multiple operators (or multiple tabs) could
  race on mission writes. **Mitigation:** v1 is single-operator
  (§3.2 non-goal); concurrent writes are undefined behavior. A
  future v1.1 may add a single-writer lock if needed.

## 12. Open Questions

These are explicitly not decided by this spec and must be resolved by
ADR before the implementing milestone begins:

- **Q-1 (ADR-0019):** Mission file format — TOML (human-readable, matches
  existing scenario files) vs JSON (QGC-compatible) vs MAVLink `.plan`
  (cross-tool). Current lean: TOML with a JSON export.
- **Q-2 (ADR-0020):** Mission + preset persistence — filesystem (atomic
  writes, versioned, git-friendly) vs SQLite (queryable, transactional)
  vs sled (Rust-native, embedded). Current lean: filesystem.
- **Q-3 (ADR-0021):** ULog serving — server-side `pyulog` (current
  plan) vs WASM port of `pyulog` (heavy, deferred to v1.1) vs a Rust
  ULog parser crate (none exists; would be a new sim/ crate). Current
  lean: server-side pyulog.
- **Q-4 (ADR-0022):** Map tiling — live OSM (v1) vs offline bundle
  (v1.1 candidate) vs vector tiles (MapBox-style, v2 candidate).
  Current lean: live OSM for v1.
- **Q-5 (ADR-0023):** Multi-vehicle UI pattern — active-vehicle (QGC
  style, current plan) vs split-view (Mission Planner style) vs
  fleet-aware (everyone is active, all widgets tile). Current lean:
  active-vehicle.
- **Q-6 (ADR-0024):** Replay streaming protocol — WS chunks (current
  plan) vs HTTP range requests (simpler server, harder seeking) vs
  Server-Sent Events (one-way only). Current lean: WS chunks.
- **Q-7 (ADR-0025):** Param preset format — TOML (matches missions) vs
  JSON (matches QGC param files) vs PX4 params file format
  (cross-compatible). Current lean: TOML.
- **Q-8 (ADR-0026):** Mission validation rules — altitude bounds (per-
  vehicle max?), fence containment (strict inclusion?), geofence shape
  (convex only or any polygon?), rally point count limit. Current lean:
  strict per-vehicle-type bounds, any polygon, rally limit = 5.

- **Q-9 (Open, not ADR-bound):** Should v1 support ArduPilot SITL in
  addition to PX4 SITL? The MAVLink mission protocol is shared, but the
  HIL simulator and fleet manager are PX4-specific. Adding ArduPilot
  would require a parallel HIL backend. **Defer to v2 spec; v1 is
  PX4-only.**

- **Q-10 (Open):** Should the console be installable as a PWA for
  offline use? The dual-mode routing and Leaflet tiles both work
  offline once loaded, but missions and replays require the `:8300`
  server. **Defer to v1.1; v1 is browser-only.**

- **Q-11 (Open):** Should there be a "scenario recorder" that turns a
  live flight into a `.replay` file for later analysis? `sitsim-cli`
  already produces `.replay` files for every `scenario-run`; this would
  just expose them through the catalog UI. **Likely yes for v1, but
  scope to M6.**

## 13. ADR Backlog

Each ADR will live under `console/docs/adr/` (new directory) and be
numbered to continue the cross-repo sequence (the latest existing ADR
is fleet's 0018; the GCS backlog starts at 0019). Each ADR must follow
the existing ADR format used in `sim/docs/adr/` and `fleet/docs/adr/`.

| ADR | Title | Owning milestone | Status |
|---|---|---|---|
| 0019 | Mission file format (TOML vs JSON vs .plan) | M1 | Proposed |
| 0020 | Mission + preset persistence (filesystem vs SQLite vs sled) | M1 | Proposed |
| 0021 | ULog serving (server-side pyulog vs WASM vs Rust crate) | M6 | Proposed |
| 0022 | Map tiling strategy (live OSM vs offline bundle vs vector) | M1 (decision), v1.1 (offline) | Proposed |
| 0023 | Multi-vehicle UI pattern (active-vehicle vs split vs fleet-aware) | M3 | Proposed |
| 0024 | Replay streaming protocol (WS chunks vs HTTP range vs SSE) | M6 | Proposed |
| 0025 | Param preset format (TOML vs JSON vs PX4 params) | M4 | Proposed |
| 0026 | Mission validation rules (altitude/fence/rally bounds) | M1 | Proposed |
| 0027 | `:8300` server: new `fleet-mission` binary vs `fleet-cli serve` extension | M1 | Proposed |

## 14. References

### Internal

- `docs/ARCHITECTURE.md` — current port map and data flows
- `docs/VERIFICATION.md` — existing I/F/S/O/R ladder with PASS evidence
- `docs/SANDBOX_SETUP.md` — verified bring-up sequence
- `console/AGENTS.md` — console ownership and dual-mode routing contract
- `fleet/docs/SPEC.md` — fleet manager spec
- `fleet/docs/adr/0016-vehicle-setup-control-plane.md` — Vehicle Setup
- `fleet/docs/adr/0017-operator-map-control-plane.md` — Operator Map
- `fleet/docs/adr/0018-runtime-control-plane.md` — Runtime control plane
- `sim/docs/PROTOCOL.md` — MAVLink wire claims
- `sim/docs/SPEC.md` §8.1 — replay format

### External

- QGroundControl: http://qgroundcontrol.com/ — reference GCS for PX4
- Mission Planner: https://ardupilot.org/planner/ — ArduPilot GCS
- PX4 MAVLink mission protocol:
  https://docs.px4.io/main/en/mission_planning/mission_planning.html
- MAVLink common.xml mission messages:
  https://mavlink.io/en/messages/common.html#MISSION_ITEM_INT

---

**End of spec v0.1.** Implementation begins after ADR-0019, ADR-0020,
and ADR-0026 are accepted (the M1-blocking decisions). All other ADRs
can be resolved in parallel with their owning milestone.
