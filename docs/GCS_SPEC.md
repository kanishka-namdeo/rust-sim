# RustSim GCS v1 — Engineering Specification (SITL-only)

**Status:** Draft v0.2 — 2026-09-09 (grounded with market research + zero-assumption UX flows)
**Owners:** RustSim core team
**Scope:** `console/` (primary), `fleet/` (extensions), `sim/` (read-only consumer)
**Supersedes:** v0.1 (2026-09-09)
**Related:** `docs/ARCHITECTURE.md`, `docs/VERIFICATION.md`, `console/AGENTS.md`, `fleet/docs/adr/0016-vehicle-setup-control-plane.md`, `fleet/docs/adr/0017-operator-map-control-plane.md`, `fleet/docs/adr/0018-runtime-control-plane.md`

> This spec defines what it takes to turn the existing RustSim console into a
> QGroundControl / Mission Planner-class ground control station **for PX4 SITL
> only**. It is a binding engineering contract: every feature, gate, and
> milestone listed here is intended to be implemented, verified, and shipped in
> v1. Decisions deferred to ADRs are listed in §13; everything else is
> decided by this document.
>
> **What changed in v0.2:** Added §1.1 Market Context (concrete market sizes
> + CAGRs from 2026 industry reports), §2.3 Competitive Landscape (QGC v5.1,
> Mission Planner, DroneDeploy, AirHub, Foxglove, PX4 Flight Review), §3.3
> PX4 Version Policy (v1.18 beta risk), §5.6 Survey/Corridor Patterns scope
> decision (re-promoted from v1.1 candidate to v1, based on competitive
> parity), completely rewritten §8 UX Flows as zero-assumption step-by-step
> procedures, two new ADRs (0028, 0029), and four new research-grounded
> risks (R-8 through R-11). All UX flows now specify pre-conditions, exact
> UI element labels, click-by-click state transitions, error paths, and
> post-conditions — no implicit assumptions remain.

---

## 1. Purpose

RustSim today is a simulation testbed with an operator console bolted on top.
The console already drives real PX4 SITL through the Operator Map (ADR-0017)
and the Vehicle Setup plane (ADR-0016), and the fleet manager already flies
auctioned missions end-to-end (F-2). What it does not yet have is the
**ground-control-station shape**: a permanent Plan View that lets an operator
design, save, version, upload, and re-fly missions; a Fly View that consolidates
all the live-flight widgets QGC operators expect; and an Analyze View that
turns the `.replay` and `.ulg` artifacts the harnesses already produce into
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

### 1.1 Market Context (grounded in 2026 industry research)

The decision to build a SITL-focused GCS is supported by four converging
market trends documented in 2026 industry reports. Numbers below are
cited verbatim from the source reports; URLs are in §14 External References.

**Trend 1 — The GCS market itself is large and growing fast.** The global UAV
ground control station market was valued at USD 9.60 billion in 2025 and is
projected to grow from USD 11.78 billion in 2026 to USD 60.10 billion by 2034
(droneintelligence.ai, 2026). A separate report pegs the drone ground control
station market at USD 5.6 billion in 2024 with a 12.8% CAGR (dataintelo.com,
2026). The headline number varies by report — driven by different definitions
of "ground control station" (handheld RC vs. laptop GCS vs. fixed
infrastructure) — but every report agrees the market is double-digit-CAGR
and multi-billion-dollar today. RustSim GCS does not need a meaningful
share of this market to justify its existence; it needs to be the best tool
for the SITL-only segment of it.

**Trend 2 — The drone simulator market is the closest analog and is also
booming.** Fortune Business Insights (Aug 2026) projects the drone
simulator market to grow from USD 1.38 billion in 2026 to USD 4.15 billion
by 2034 at a 14.71% CAGR. Straits Research (2026) puts it at USD 1.53
billion (2026) → USD 4.43 billion (2034) at 14.2% CAGR. The simulator
market is the one RustSim directly competes in; its growth is a direct
demand signal for a GCS that takes SITL seriously.

**Trend 3 — Drone swarm control is the fastest-growing sub-segment.** The
"Drone Swarm Control Ground Station Market" (growthmarketreports.com, 2026)
reached USD 1.60 billion in 2025 and is forecast to hit USD 4.87 billion by
2034 at a 13.1% CAGR. This is the market segment RustSim's Fleet C2 view
(§5.4) directly addresses. Academic work on swarm trajectory optimization
(arxiv.org, March 2026) and industry swarm-platform writeups (a-bots.com,
Sep 2025) both describe architectures that match RustSim's: an offline
planning toolchain + a choreography/orchestration layer + per-vehicle
execution. The fleet manager + auction allocator that RustSim already ships
is exactly the swarm-control pattern these reports describe.

**Trend 4 — UAV flight training and simulation is the largest adjacent
market.** The UAV Flight Training And Simulation Market (snsinsider.com,
Apr 2026) is valued at USD 3.00 billion in 2025 and projected to reach
USD 16.46 billion by 2035. Training programs are a primary consumer of
SITL-class tools: every drone pilot trained on PX4 today trains on SITL
before touching a real airframe. RustSim GCS, with deterministic replays
(§5.5) and scenario injection (ADR-0018), is well-positioned as a
training-grade tool — every flight is reproducible, every failure mode is
injectable, every run produces a reviewable artifact.

**Implication for v1 scope.** The market data does not change v1's SITL-only
scope boundary (§3.2), but it does shift priority weight inside v1. The
Fleet C2 view (§5.4) is not just "an extension of existing F-1/F-2 work" —
it is the GCS feature most directly aligned with the fastest-growing market
segment (Trend 3). The Analyze View (§5.5) is not just "log browsing" — it
is the feature most directly aligned with the training market (Trend 4),
because training requires review. The milestone roadmap (§10) reflects this
reweighting: M5 (Fleet mission orchestration) and M6 (Analyze View) are
both kept inside v1 rather than deferred to v1.1, even though they are the
two most expensive milestones.

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
ArduPilot, written in C++/Qt. The current stable release as of September
2026 is **QGC v5.1.x** (with v5.1.4 specifically fixing a Plan View bug
where polyline/polygon vertex dragging froze the UI — see
github.com/mavlink/qgroundcontrol releases; this exact bug class is one
RustSim GCS must explicitly test against, since the Leaflet-based Operator
Map has the same dragging pattern). QGC's UI is organized into five
top-level views: **Plan** (mission editor with waypoint/survey/corridor
patterns, geofence, rally points), **Fly** (live map + HUD + instrument
widgets + MAVLink console), **Vehicle Setup** (airframe, sensors, radio,
power, modes, parameters — a full QGC-style configuration workflow),
**Analyze** (log browse, MAVLink console, MAVLink inspector, geo tag
images), and **Application Settings**. Mission files are JSON (`.plan`
format), cross-compatible with ArduPilot via the same MAVLink mission
protocol.

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

### 2.3 Competitive Landscape (grounded in 2026 research)

RustSim GCS does not enter an empty market. The decision to ship a
SITL-only GCS is informed by what already exists and where each competitor
is weak. The landscape splits into three tiers.

**Tier 1 — Open-source desktop GCS (direct functional competitors).**
- **QGC v5.1.x** (above) — the reference. Strength: full PX4 v1.16+ dialect
  coverage, real-hardware path, mature UI. Weakness for SITL users: SITL is
  a config flag, not a first-class mode; the 90% real-hardware UI surface is
  dead weight; the Plan View vertex-dragging bug that v5.1.4 just fixed is
  the kind of regression that ships when SITL is not the primary test path.
- **Mission Planner** — ArduPilot-focused; does not natively target PX4.
  Out of scope as a direct competitor; relevant only for UX pattern
  borrowing (its survey grid generator is more mature than QGC's).

**Tier 2 — Commercial cloud flight planning (mission editor competitors).**
- **DroneDeploy** (help.dronedeploy.com, Mar 2026) — cloud mission planning,
  automated flight paths, camera-trigger-aware photo missions. Strength:
  polished mission editor, cloud sync. Weakness: requires a real drone and
  real camera; no SITL path; closed source; no fleet orchestration.
- **AirHub "Waypoint Patterns"** (airhub.app, Jul 2025) — automated
  flight-path generation for area scouting. Strength: pattern library
  (grid, corridor, perimeter). Weakness: same as DroneDeploy — no SITL, no
  fleet, closed source.

**Tier 3 — Web-based log analysis (Analyze View competitors).**
- **PX4 Flight Review** (github.com/PX4/flight_review) — official PX4 web
  app for ULog analysis. Strength: deep PX4 topic knowledge, official
  support. Weakness: upload-then-analyze workflow (no live telemetry
  overlay); Python backend (not Rust); no replay-vs-live comparison.
- **Foxglove** (foxglove.dev) — general-purpose web visualization, supports
  PX4 ULog. Strength: polished plotting, ROS2 integration. Weakness:
  generic, not drone-native; no mission planning or fleet features.

**Where RustSim GCS wins.** The intersection no competitor occupies: a
SITL-first GCS with mission planning (Tier 2 parity), fleet orchestration
(Trend 3 market segment), and integrated log analysis (Tier 3 parity), all
open-source, all in Rust + TypeScript, all speaking PX4's native wire.
Specifically, RustSim GCS v1 will be the only tool where an operator can:
plan a 4-waypoint mission → upload it to 2 SITL vehicles → start them
sequentially → watch live telemetry → e-stop → scrub the replay → diff the
replay against a second run, all in one browser tab. QGC cannot do the
multi-vehicle swarming; DroneDeploy cannot do SITL at all; Flight Review
cannot do live ops; Foxglove cannot plan missions.

## 3. Goals & Non-Goals

### 3.1 Goals (v1)

- **G-1 — Plan, fly, and analyze missions against N concurrent PX4 SITL
  vehicles** from a single browser tab. N is bounded by sandbox CPU/RAM
  (target: N=4 supported, N=8 best-effort).
- **G-2 — Six feature areas, each with its own view:** Plan View (mission
  editor), Fly View (live ops + instruments), Vehicle Setup (extend
  existing), Fleet C2 (extend existing), Analyze View (ULog + replay), and
  Survey Patterns (auto-generated mission generators — re-promoted to v1
  based on competitive parity with QGC/MP/DroneDeploy/AirHub, see §5.6).
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
  question (§12) but explicitly not committed.
- **3D terrain/obstacle view.** "3D flight view to show flight path on
  hilly terrain" was the most-requested QGC feature on the PX4 Discuss
  forum (Jun 2018, still open). RustSim GCS v1 uses 2D Leaflet only. 3D
  is a v2 candidate, not a v1.1 — it requires a Cesium or MapboxGL 3D
  integration that is out of scope for v1's browser performance budget.

### 3.3 PX4 Version Policy (NEW in v0.2 — risk-driven)

RustSim pins PX4-Autopilot at **v1.16.2**. As of September 2026, PX4 v1.18
is entering beta (quad-drone-lab.co.kr weekly briefing, Jul 2026). The
v0.1 spec assumed v1.16.2 was the stable target; v0.2 makes the version
policy explicit because the dialect drift documented in the existing ADRs
(0010, 0011, 0015 — see `sim/docs/PROTOCOL.md`) was captured against
v1.16.2 and may not hold against v1.18.

**v1 ships against PX4 v1.16.2.** No exceptions, no "let's try v1.18 in CI."
The reason is the same as the reason for golden-vectoring the MAVLink
codec: protocol claims are only claims until they are captured live, and
every capture costs a harness run. v1.18 will be evaluated for v1.1 or v2
in a separate spec.

**The version policy is enforced by:**
1. `sim/px4-version` and `fleet/px4-version` files (already exist in the
   repo) recording the pinned SHA + tag.
2. A new harness gate `G-0` (§9) that asserts `make px4_sitl_default`
   was run against the pinned version, by checking the build directory's
   `git describe --tags` output.
3. The `:8300` mission catalog server (§4.2) refuses to upload a mission
   to a vehicle whose reported PX4 version differs from the pinned one —
   returns HTTP 426 Upgrade Required with a clear message.

This means: if an operator clones PX4 at v1.18 and tries to drive it with
RustSim GCS v1, the upload is rejected. The operator must either downgrade
to v1.16.2 or wait for RustSim GCS v1.1. There is no silent fallback.

## 4. Architecture

### 4.1 Component map — existing vs v1

| Component | Today | v1 changes |
|---|---|---|
| `sim/` | Lockstep HIL simulator, REST+WS control plane on :8200+i | Unchanged. Read-only consumer of v1 work. |
| `fleet/` | Mission manager, auction allocator, offboard execution, 8-policy safety ladder | Extended: new `fleet-mission` crate handles the MAVLink mission protocol (upload/download/count/ack) for all three `MAV_MISSION_TYPE` values (mission, fence, rally). New REST endpoints on :8400 for mission CRUD and per-vehicle mission binding. |
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
MAVLink `MISSION_ITEM_INT` messages — three sequential uploads, one per
`MAV_MISSION_TYPE` (mission, fence, rally, in that order, per mavlink.io
Mission Protocol spec Aug 2026) — and sends them through `:8400`'s
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

**What's new in v0.2.** v0.1 covered only `MAV_MISSION_TYPE_MISSION`
(flight plan). Per the mavlink.io Mission Protocol spec (updated Aug 12
2026), the modern protocol carries three mission types via the
`MAV_MISSION_TYPE` enum: **MISSION** (the flight plan), **FENCE** (the
geofence polygon set), and **RALLY** (rally/safe points). v0.2 commits
to all three for QGC parity. The `:8300` catalog stores them as three
separate items per saved mission (so a mission's flight plan can be
re-uploaded without re-uploading its fence), and the upload path runs
three sequential MAVLink transactions per the spec ordering.

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
- As an operator, I can place rally points (safe-landing alternatives) on
  the map, each with an altitude, and the editor validates that each rally
  point is inside the inclusion geofence, so that I cannot designate a
  rally point I cannot legally fly to.
- As an operator, I can save a mission with a name and have it persist
  across browser sessions, list saved missions, duplicate, rename, and
  delete them, so that I have a mission library.
- As an operator, I can upload a saved mission to a selected vehicle (PX4
  SITL), watch the upload progress per-item with ack status for each of
  the three mission types, and download the current mission from a vehicle
  to compare with the saved version.

**Acceptance criteria.**

- AC-5.1.1: A mission with 0 waypoints fails validation with a clear
  message ("mission must have at least one waypoint") and cannot be saved
  or uploaded. Exit code from `/api/missions/{id}/validate` is 422.
- AC-5.1.2: A mission with a waypoint outside the inclusion geofence fails
  validation; the offending waypoint is highlighted on the map in red and
  the side table row is highlighted in red with the message "waypoint 3
  outside inclusion fence by 12.4 m".
- AC-5.1.3: A saved mission persists across browser reloads and across
  console restarts (it lives in `:8300`'s file store, not in browser
  localStorage).
- AC-5.1.4: Upload to a vehicle that is disarmed and in `STANDBY` state
  succeeds; upload to a vehicle in `ACTIVE` or `FAULT` state returns HTTP
  409 with a clear message. Upload to a vehicle whose PX4 version differs
  from the pinned v1.16.2 returns HTTP 426 (§3.3).
- AC-5.1.5: Download from a vehicle returns the mission currently loaded in
  PX4's mission pool, not the one `:8300` last uploaded (i.e. it queries
  PX4, not the catalog). The download returns three separate items
  (flight plan, fence, rally) and the UI displays all three in tabs.
- AC-5.1.6: The upload progress bar shows three sub-bars (Mission, Fence,
  Rally), each with `items_sent / items_acked`. If any sub-upload fails,
  the entire upload is rolled back (PX4's mission pool is reset to its
  pre-upload state via `MISSION_CLEAR_ALL`).
- AC-5.1.7 (NEW in v0.2): Vertex dragging on the map (waypoints, fence
  polygon corners, rally points) does not freeze the UI even with 50+
  vertices. This is the QGC v5.1.4 bug class (see §2.2) and is verified by
  G-1.

**UX flow — plan and upload a mission.** See §8.1 for the zero-assumption
step-by-step.

**API contracts (new).**

- `POST /api/missions` — create mission in catalog. Body: mission TOML or
  JSON. Returns: `{id, version}`.
- `GET /api/missions` — list all saved missions. Returns: `[{id, name,
  version, updated_at, waypoint_count, fence_count, rally_count}]`.
- `GET /api/missions/{id}` — fetch a mission by id. Returns: full mission
  TOML/JSON with three top-level sections: `mission`, `geofence`, `rally`.
- `PUT /api/missions/{id}` — update. Body: mission. Returns: `{version}`.
  Version is bumped; old version is retained for diff.
- `DELETE /api/missions/{id}` — soft-delete (catalog retains a tombstone
  for replay referential integrity).
- `POST /api/missions/{id}/validate` — schema + geofence geometry +
  rally-point containment validation. Returns: `{valid, errors: [{type:
  "waypoint|fence|rally", seq, message}]}`.
- `POST /api/vehicles/{i}/mission/upload?mission_id={id}` — upload via
  MAVLink mission protocol, three sequential transactions (mission, fence,
  rally). Returns: `{mission: {sent, acked, failed}, fence: {...}, rally:
  {...}}`. Rolls back on any failure.
- `GET /api/vehicles/{i}/mission?type={mission|fence|rally}` — download
  one mission type from PX4. Returns: mission JSON for that type.
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
px4_version = "v1.16.2"    # the pinned version this mission was authored against

[[mission.waypoints]]
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
seq = 0
lat = 37.4133
lon = -122.1014
alt_m = 0.0
```

**Verification gates.** G-0 (PX4 version check, new in v0.2), G-1
(validation including the vertex-drag stress test), G-2 (persistence CRUD
round-trip), G-3 (three-type upload ack), G-4 (three-type download
round-trip). See §9.

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

**UX flow — fly a planned mission.** See §8.2 for the zero-assumption
step-by-step.

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

**UX flow — configure, save preset, restore later.** See §8.3 for the
zero-assumption step-by-step.

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

**Market alignment (NEW in v0.2).** The "Drone Swarm Control Ground
Station Market" report (growthmarketreports.com, 2026) sizes this exact
feature category at USD 1.60 billion (2025) → USD 4.87 billion (2034) at
13.1% CAGR. Academic work on swarm trajectory optimization (arxiv.org,
March 2026) and industry swarm-platform writeups (a-bots.com, Sep 2025)
both describe the same architecture RustSim already implements: an
offline planning toolchain + a choreography layer + per-vehicle execution.
This view is the GCS feature most directly aligned with the fastest-growing
market segment (§1.1 Trend 3); it is not a "v1.1 maybe" feature.

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

**UX flow — fly a 2-vehicle fleet mission with sequential orchestration.**
See §8.4 for the zero-assumption step-by-step.

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

**Competitive context (NEW in v0.2).** This view's direct competitors are
PX4 Flight Review (github.com/PX4/flight_review — official, upload-then-
analyze, Python backend) and Foxglove (foxglove.dev — general-purpose,
ROS2-friendly, supports PX4 ULog). RustSim GCS's differentiation is: (a)
live + replay in the same view (Flight Review is upload-only; Foxglove is
live-only or file-only, not both in one tab); (b) Rust backend with
zero-copy WS streaming (Flight Review is Python+HTTP); (c) the replay is
RustSim's deterministic record, not PX4's noisy EKF output, so two runs of
the same scenario produce byte-identical replays that diff cleanly.

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

**UX flow — analyze a flight.** See §8.5 for the zero-assumption
step-by-step.

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

### 5.6 Survey & Corridor Patterns — RE-PROMOTED TO v1 (NEW in v0.2)

**What it is.** Auto-generated mission generators: Survey Grid (draw a
polygon, get parallel back-and-forth legs with camera triggers),
Corridor Pattern (draw a polyline, get a survey of a corridor of
configurable width around it), and Perimeter Pattern (draw a polygon,
get a single boundary-following loop). These are the QGC Plan View
patterns that Mission Planner made famous and that DroneDeploy and
AirHub have since commercialized.

**Why this was re-promoted from v1.1 candidate to v1.** v0.1 listed
"survey patterns (grids/corridors) on the Operator Map's plan editor" as
a Roadmap item (outside v1 scope). Market research (§2.3 Tier 2)
shows these patterns are now table-stakes: QGC v5.1 ships them, Mission
Planner has had them since 2015, DroneDeploy's entire product is built
on them, and AirHub's "Waypoint Patterns" feature (Jul 2025) is the
most recent commercial entry. A GCS that ships v1 without them is not
QGC-parity. The cost is moderate (a `console/lib/patterns.ts` generator
module, no new backend), and the value is large (Tier 2 competitive
parity). Re-promoted.

**User stories.**

- As an operator, I can draw a polygon on the map, select "Survey Grid"
  from the patterns menu, configure leg spacing (default 10 m), altitude
  (default 30 m), and camera trigger interval (default 5 s), and the
  editor generates a back-and-forth waypoint pattern inside the polygon,
  with camera-trigger waypoints at every leg.
- As an operator, I can draw a polyline on the map, select "Corridor
  Pattern", configure corridor width (default 20 m) and leg spacing, and
  the editor generates a survey pattern covering the corridor.
- As an operator, I can draw a polygon, select "Perimeter Pattern", and
  the editor generates a single loop following the polygon boundary at
  a configurable offset (default 5 m inside the polygon).

**Acceptance criteria.**

- AC-5.6.1: Survey Grid on a 100 m × 100 m polygon with 10 m leg spacing
  generates 10 legs (correct count) and 21 waypoints (correct count,
  including turns), all inside the polygon (validation passes).
- AC-5.6.2: Corridor Pattern on a 100 m polyline with 20 m width generates
  legs that cover the corridor with no gaps wider than the leg spacing.
- AC-5.6.3: Perimeter Pattern on a 4-vertex polygon generates 4 waypoints
  (one per vertex offset inward) connected in a loop.
- AC-5.6.4: All generated waypoints inherit the mission's default altitude
  and accept-radius, editable after generation like any manually-placed
  waypoint.

**UX flow — generate a survey grid.** See §8.6 for the zero-assumption
step-by-step.

**API contracts.** None new — patterns are generated client-side in
`console/lib/patterns.ts` and the resulting waypoints are inserted into
the existing mission editor. The mission is then saved/uploaded via the
existing `:8300` endpoints.

**Verification gate.** G-13 (pattern generation correctness + integration
with mission editor).

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
| POST | `/api/missions/{id}/validate` | :8300 | Schema + fence + rally validation |
| POST | `/api/vehicles/{i}/mission/upload?mission_id={id}` | :8400 | MAVLink upload (3 types) |
| GET | `/api/vehicles/{i}/mission?type={mission|fence|rally}` | :8400 | MAVLink download |
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
found, 409 conflict, 422 unprocessable, 426 upgrade required, 500
internal). All `:8300` endpoints are accessible through the Caddy gateway
via `?XTransformPort=8300`, consistent with the existing dual-mode
routing pattern in `console/AGENTS.md`.

## 8. UX Flows (zero-assumption step-by-step)

> **Reading guide.** Each flow below is a numbered procedure. Every step
> specifies: (a) the exact UI element the operator interacts with (button
> label, panel name, tab name), (b) the pre-condition that must be true
> before the step, (c) the action, (d) the resulting state change, and
> (e) the post-condition that must be true after. Error paths are called
> out inline. If a step depends on a previous step's post-condition, the
> dependency is named explicitly. **There are no implicit assumptions:**
> if a step says "click X", X is labeled exactly that way in the UI; if a
> step says "vehicle 0 is in STANDBY", that state is observable in the
> Fly View status bar before the step begins.

The five views are mounted permanently (hidden by CSS, never unmounted)
so that telemetry engines survive tab switches — this is the existing
console pattern (see `console/AGENTS.md` Local Contracts). The tab bar
reflects the five views in order: **Plan → Fly → Setup → Fleet C2 →
Analyze**. The order matches the operator's typical workflow (design →
fly → configure → orchestrate → review).

### 8.1 Plan a mission → upload to a vehicle

**Pre-conditions (must all be true before starting):**
- P1: The console is loaded in the browser; the top tab bar shows all
  five tabs: Plan, Fly, Setup, Fleet C2, Analyze.
- P2: At least one PX4 SITL vehicle is running and connected; the Fly
  View status bar (visible when Fly tab is active) shows "Vehicle 0:
  STANDBY" in green.
- P3: The mission catalog server `:8300` is reachable; the Plan View's
  mission-list panel (left side) shows either "No saved missions" or a
  list of previously-saved missions.

**Steps:**

1. **Click the "Plan" tab** in the top tab bar. Post-condition: the Plan
   View is visible; the map widget has rendered (centered on the last
   known vehicle position, or on the default geofence center if no
   vehicle is connected); the mission-list panel on the left shows saved
   missions or "No saved missions"; the editor panel on the right shows
   "No mission loaded — click New Mission to start" or the last-edited
   mission.
2. **Click the "New Mission" button** in the editor panel header.
   Post-condition: the editor enters "edit mode"; the map cursor becomes
   a crosshair; a new empty mission with `id=null` (not yet saved) is
   shown in the editor; the side table is empty with a single row
   placeholder "Click on the map to add waypoint 1".
3. **Click on the map** at the desired first waypoint location.
   Post-condition: a numbered blue marker "1" appears at the clicked
   lat/lon; the side table row 1 fills in with `seq=0, lat=<clicked>,
   lon=<clicked>, alt=12.0 (default), hold=0.0, accept=2.0`; the
   placeholder updates to "Click on the map to add waypoint 2".
4. **Repeat step 3** three more times at different locations.
   Post-condition: 4 numbered blue markers (1, 2, 3, 4) appear on the
   map, connected by a blue polyline in numbered order; the side table
   has 4 rows.
5. **Click on waypoint 1's altitude cell** in the side table, type `15`,
   press Tab. Post-condition: waypoint 1's `alt` shows `15.0`; the
   marker label updates to "1 (15m)".
6. **Click the "Draw Geofence" toggle** in the editor panel toolbar.
   Post-condition: the map cursor becomes a polygon-drawing cursor; a
   tooltip appears: "Click to add fence vertices; double-click to close".
7. **Click on the map 4 times** to draw a polygon around the 4 waypoints,
   then **double-click** to close. Post-condition: a green dashed polygon
   appears on the map enclosing all 4 waypoints; the editor panel shows
   "Geofence: 4 vertices, ceiling 60 m, floor 0 m" with editable fields.
8. **Click the "Validate" button** in the editor panel toolbar.
   Post-condition: a toast appears: "Validation passed: 4 waypoints, 1
   fence, 0 rally points". The "Save" button becomes enabled (was
   disabled while validation had not been run).
   - **Error path A:** if any waypoint is outside the fence, the toast
     says "Validation failed: waypoint 2 outside inclusion fence by 8.3
     m"; waypoint 2's marker turns red; the side table row 2 highlights
     red with the message; the Save button stays disabled. Operator
     must move the waypoint inside the fence and re-click Validate.
9. **Click the "Save" button** in the editor panel toolbar.
   Post-condition: a modal appears titled "Save Mission" with a
   text input for the mission name (placeholder: "demo-square"),
   pre-filled with the previous name if editing, and OK / Cancel
   buttons.
10. **Type `demo-square` in the name input**, click OK. Post-condition:
    the modal closes; a toast appears: "Mission saved as 'demo-square'
    (v1)"; the mission-list panel on the left updates to show
    "demo-square (v1, 4 wps, 1 fence, 0 rally)".
11. **Click the "Upload to Vehicle" button** in the editor panel
    toolbar. Post-condition: a modal appears titled "Select target
    vehicle" with a dropdown listing all connected vehicles
    (e.g. "Vehicle 0 (STANDBY)", "Vehicle 1 (STANDBY)") and OK / Cancel
    buttons.
12. **Select "Vehicle 0 (STANDBY)" from the dropdown**, click OK.
    Post-condition: the modal closes; the editor panel shows three
    progress bars stacked vertically, labeled "Mission (flight plan)",
    "Fence", "Rally". Each bar starts at 0%.
    - The Mission bar advances: 0% → 25% (1/4 ack'd) → 50% → 75% → 100%.
    - The Fence bar advances: 0% → 25% (1/4 ack'd) → 50% → 75% → 100%.
    - The Rally bar shows 100% immediately (0 rally points, no upload
      needed).
    - **Error path B:** if any item fails to ack, that bar turns red,
      the upload aborts, all three bars roll back to 0%, a toast
      appears: "Upload failed: mission item 2 not ack'd by PX4 (timeout
      5 s). Mission pool reset to pre-upload state." The vehicle's PX4
      mission pool is cleared via `MISSION_CLEAR_ALL`.
    - **Error path C:** if vehicle 0's PX4 version differs from v1.16.2
      (§3.3), the upload is rejected before any bar moves; a toast
      appears: "Upload rejected: vehicle 0 reports PX4 v1.18.0,
      RustSim GCS v1 requires v1.16.2 (HTTP 426)."
13. **Wait for all three bars to reach 100% green.** Post-condition: a
    toast appears: "Mission 'demo-square' uploaded to vehicle 0 (4
    waypoints, 1 fence, 0 rally)". The Upload button greys out and the
    label changes to "Re-upload".
14. **Click the "Fly" tab** in the top tab bar. Post-condition: the Fly
    View becomes visible; vehicle 0 is selected as the active vehicle
    (highlighted on the map); the uploaded mission's polyline is
    visible on the Fly View map as a dashed line; the action bar at the
    bottom shows "DISARMED" with a green "Arm" button enabled.

### 8.2 Fly a planned mission (continues from §8.1 step 14)

**Pre-conditions:**
- P1: The Fly tab is active.
- P2: Vehicle 0 is selected as active (highlighted on the Fly View map).
- P3: The uploaded mission's polyline is visible as a dashed line.
- P4: The action bar shows "DISARMED" with "Arm" enabled.
- P5: The pre-arm checklist panel (right side) shows 5 checks: EKF2,
  GPS fix, Mode, Fence, Battery — each with a status icon (grey =
  not yet run).

**Steps:**

1. **Click the "Arm" button** in the action bar. Post-condition: the
   pre-arm checks run; each check's status icon flips from grey to
   green (pass) or red (fail) within 1 s. The action bar transitions to
   "PRE-ARM CHECKS RUNNING" with a spinner.
   - **Error path A:** if any check fails, the action bar shows
     "ARM BLOCKED" in red; the failing check(s) show a red ✗ with a
     tooltip explaining the failure (e.g. "EKF2: not converged, wait 5
     s"); the Arm button greys out. Operator must resolve the failure
     (e.g. wait for EKF2 to converge) and click "Re-run checks" before
     Arm re-enables.
2. **Wait for all 5 checks to flip green.** Post-condition: the action
   bar transitions from "PRE-ARM CHECKS RUNNING" to "ARMED" in amber;
   the Arm button label changes to "Disarm" (red); a new button
   "Start Mission" appears and is enabled (the vehicle has a mission
   loaded from §8.1).
3. **Click the "Start Mission" button** in the action bar.
   Post-condition: `:8400` issues `MAV_CMD_MISSION_START`; vehicle 0's
   state transitions STANDBY → ACTIVE within 500 ms; the action bar
   shows "IN MISSION (WP 1/4)" in green; the map shows vehicle 0's
   marker moving along the polyline toward waypoint 1; the strip chart
   panel (bottom) shows altitude climbing toward 12 m.
4. **Observe the flight.** Post-condition (after ~30 s for a typical
   4-waypoint mission at 12 m altitude): the action bar shows "IN
   MISSION (WP 2/4)", then "WP 3/4", then "WP 4/4"; the map shows the
   vehicle reaching each waypoint and turning; the strip chart shows
   altitude holding at 12 m between waypoints, climbing/descending at
   each turn.
   - **Operator intervention path:** at any point during the mission,
     the operator can click the "Hold" button in the action bar;
     vehicle 0 transitions ACTIVE → HOLDING; the action bar shows
     "HOLDING" in amber; the "Resume" button appears. Clicking Resume
     transitions back to ACTIVE and the mission continues.
5. **Mission auto-completes.** Post-condition: after the last waypoint,
   vehicle 0 auto-RTLs (per the mission plan — the last implicit
   waypoint is RTL); the action bar shows "RETURNING" in amber; the
   map shows the vehicle returning to its launch point; the strip
   chart shows altitude descending.
6. **Vehicle lands and disarms.** Post-condition: vehicle 0 transitions
   RETURNING → LANDED → DISARMED within 10 s of reaching the launch
   point; the action bar shows "DISARMED — MISSION COMPLETE" in green;
   a green checkmark icon appears next to the mission name in the
   Fly View's mission panel.
7. **Click the "Analyze" tab** in the top tab bar to review the
   flight (see §8.5).

### 8.3 Configure a vehicle, save preset, restore later

**Pre-conditions:**
- P1: At least one PX4 SITL vehicle is connected and in STANDBY state.
- P2: The Setup tab is active (click "Setup" in the top tab bar).

**Steps:**

1. **Click "Vehicle 0"** in the Setup View's vehicle selector (top-left
   dropdown). Post-condition: the Setup View's left panel populates
   with the parameter list for vehicle 0 (1000+ rows); the right
   panel shows calibration status (gyro, accel, mag, level — each
   green = calibrated or red = not calibrated); a search input at the
   top of the parameter list is empty with placeholder "Search params
   by name or group...".
2. **Click the search input**, type `MPC_XY_VEL_MAX`. Post-condition
   (after 250 ms debounce): the parameter list filters to show only
   matching rows; `MPC_XY_VEL_MAX` appears at the top with current
   value `12.0`, type `FLOAT`, group `MPC`, default `12.0`,
   `is_changed=false`.
3. **Double-click the value cell** for `MPC_XY_VEL_MAX`, type `8.0`,
   press Enter. Post-condition: the value cell shows `8.0` in blue
   (indicating unsaved change); `is_changed=true`; a "Save to vehicle"
   button at the bottom of the parameter list becomes enabled (was
   disabled).
4. **Clear the search input** (click the ✗ in the search input).
   Post-condition: the full parameter list is shown again; the row for
   `MPC_XY_VEL_MAX` is highlighted in pale yellow to indicate it has
   an unsaved change.
5. **Search for `MC_ROLLRATE_P`**, double-click the value cell, type
   `7.5`, press Enter. Post-condition: `MC_ROLLRATE_P` shows `7.5` in
   blue; `is_changed=true`.
6. **Click the "Save to vehicle" button** at the bottom of the
   parameter list. Post-condition: a confirmation modal appears:
   "Write 2 changed parameters to vehicle 0?" with OK / Cancel
   buttons. Click OK. Post-condition: the values are written to PX4
   via `PARAM_SET`; the value cells flip from blue to black
   (persisted); a toast appears: "2 parameters written".
7. **Click the "Save as preset" button** in the parameter list toolbar.
   Post-condition: a modal appears: "Save parameter preset" with a
   text input for the preset name (placeholder: "aggressive-corners")
   and OK / Cancel buttons.
8. **Type `aggressive-corners`**, click OK. Post-condition: the modal
   closes; a toast appears: "Preset 'aggressive-corners' saved (2
   params)". The preset appears in the "Presets" panel below the
   parameter list.
9. **Switch vehicle or restart console** (simulating "later").
   Post-condition: the parameter list shows fresh values (the changes
   from step 6 are persisted in PX4 SITL; if SITL was restarted, the
   params reset to defaults).
10. **Click "Load preset"** in the parameter list toolbar.
    Post-condition: a modal appears listing saved presets; select
    "aggressive-corners" and click OK. Post-condition: the 2 saved
    values are written to PX4; the parameter list shows them with
    `is_changed=true` (because they now differ from defaults).
11. **Click the "Diff against defaults" toggle** in the parameter list
    toolbar. Post-condition: the parameter list filters to show only
    rows where `is_changed=true`; the 2 parameters set in steps 3 and
    5 appear with their current value and the default value shown
    side-by-side in red/green (current/default).

### 8.4 Fly a 2-vehicle fleet mission with sequential orchestration

**Pre-conditions:**
- P1: Two PX4 SITL vehicles are running and connected; the Fly View
  status bar shows "Vehicle 0: STANDBY" and "Vehicle 1: STANDBY".
- P2: Two missions are saved in the mission catalog: "patrol-A" (4
  waypoints) and "survey-B" (6 waypoints).
- P3: The Fleet C2 tab is active.

**Steps:**

1. **Click the "Mission Bindings" tab** in the Fleet C2 View's right
   panel. Post-condition: a table appears with columns "Vehicle",
   "Mission", "Binding state"; two rows (Vehicle 0, Vehicle 1) show
   "— / unassigned".
2. **Click "Assign" in the Vehicle 0 row.** Post-condition: a dropdown
   appears listing saved missions; select "patrol-A". Post-condition:
   row 0 shows "patrol-A / assigned".
3. **Click "Assign" in the Vehicle 1 row**, select "survey-B".
   Post-condition: row 1 shows "survey-B / assigned".
4. **Click the "Upload all" button** at the top of the Mission
   Bindings panel. Post-condition: each row shows an upload progress
   indicator (the same 3-bar pattern as §8.1 step 12, one per vehicle);
   both vehicles' missions upload in parallel; on completion, each
   row's binding state flips to "uploaded".
5. **Click the "Start Fleet" button** in the Fleet C2 View's toolbar.
   Post-condition: a modal appears titled "Start fleet" with:
   - Mode dropdown: "Parallel" or "Sequential" (default: Parallel).
   - Sequential gate dropdown (visible only if Sequential selected):
     "First waypoint reached" or "Takeoff complete" (default: First
     waypoint reached).
   - Timeout input (visible only if Sequential selected): default 30 s.
   - OK / Cancel buttons.
6. **Select "Sequential" from the Mode dropdown**, keep gate as "First
   waypoint reached", keep timeout as 30 s, click OK.
   Post-condition: the modal closes; the Fleet C2 table shows Vehicle 0
   transitioning to "ACTIVE" (taking off); Vehicle 1 stays in
   "STANDBY (waiting for gate)"; the map shows vehicle 0 climbing.
7. **Wait ~20 s.** Post-condition: vehicle 0 reaches its first
   waypoint; the Fleet C2 table shows Vehicle 0 "IN MISSION (WP 1/4)";
   the binding state of Vehicle 1 flips to "active" and Vehicle 1
   transitions STANDBY → ACTIVE (taking off); the map shows vehicle 1
   climbing.
   - **Error path A:** if vehicle 0 does not reach its first waypoint
     within 30 s, the fleet aborts; both vehicles transition to
     RETURNING (RTL); the Fleet C2 table shows "FLEET ABORTED
     (sequential gate timeout)"; a toast appears with details.
8. **Observe both vehicles flying their missions.** Post-condition:
   the Fleet C2 table shows per-vehicle progress (WP x/N) for both;
   the map shows both trajectories with distinct colors (vehicle 0 =
   blue, vehicle 1 = orange).
9. **Each vehicle completes its mission independently** and auto-RTLs.
   Post-condition: as each vehicle lands and disarms, its row in the
   Fleet C2 table shows "COMPLETE" in green.
10. **When both vehicles are COMPLETE**, the Fleet C2 View shows a green
    banner: "FLEET COMPLETE — 2/2 vehicles, 10/10 waypoints".
11. **Click the "Analyze" tab** to review (see §8.5).

### 8.5 Analyze a flight (replay scrub + plot + overlay)

**Pre-conditions:**
- P1: At least one flight has been completed (§8.2 step 6 or §8.4
  step 9); at least one `.replay` file and one `.ulg` file exist.
- P2: The Analyze tab is active.

**Steps:**

1. **Click the "Recent flights" panel** in the Analyze View's left
   sidebar. Post-condition: a list of recent `.replay` and `.ulg` files
   appears, sorted by mtime (most recent first); each row shows
   filename, duration, vehicle count, scenario name, mtime.
2. **Click the row for the most recent `.replay`** (e.g.
   `i2_flight.replay`). Post-condition: the file loads (≤2 s); the
   Analyze View's main area shows three panels: (top) a map with the
   replay's full trajectory as a polyline; (middle) a strip chart
   area (empty, with a button "Add plot"); (bottom) a timeline scrubber
   spanning the full duration with a draggable playhead at tick 0.
3. **Drag the playhead** to the middle of the timeline. Post-condition
   (within 100 ms): the map updates to show the vehicle marker at its
   position at that tick; the marker's orientation matches the
   replay's quaternion at that tick; the strip chart area shows a
   vertical "now" line at the scrubbed time.
4. **Click the "Add plot" button** in the strip chart area.
   Post-condition: a modal appears titled "Add plot" with a dropdown
   of available topics from the replay (e.g. `pos_ned_m.z`,
   `vel_ned_m.magnitude`, `battery_v`, `q_wxyz`).
5. **Select `pos_ned_m.z`**, click OK. Post-condition: a new strip
   chart appears in the strip chart area showing altitude over time
   for the full replay; the "now" line is at the scrubbed time; the
   y-axis is labeled "altitude (m)".
6. **Click "Add plot" again**, select `battery_v`. Post-condition: a
   second strip chart appears below the first; the two share an
   x-axis (time); the "now" line spans both.
7. **Click the "Overlay live" button** in the Analyze View's toolbar.
   Post-condition: a modal appears listing currently-connected live
   vehicles (e.g. "Vehicle 0 (ACTIVE)"); select "Vehicle 0" and click
   OK. Post-condition: the map updates to show two trajectories: the
   replay (blue) and the live vehicle (green); a legend appears in
   the map's top-right corner; the live trajectory updates at 10 Hz;
   the strip charts add a second line (green) for the live vehicle's
   current value.
8. **Scrub the timeline** while the live vehicle flies. Post-condition:
   the replay's marker moves with the scrubber; the live vehicle's
   marker moves on its own (real-time); the strip charts show both
   lines; the replay's "now" line is independent of the live vehicle's
   time (they are not synchronized).
9. **Click the "Stop overlay" button** in the Analyze View's toolbar
   to remove the live vehicle. Post-condition: the green trajectory
   and lines disappear; the replay-only view is restored.

### 8.6 Generate a survey grid (NEW in v0.2)

**Pre-conditions:**
- P1: The Plan tab is active.
- P2: A new empty mission is in edit mode (§8.1 steps 1-2).

**Steps:**

1. **Click the "Patterns" dropdown** in the editor panel toolbar.
   Post-condition: a dropdown menu appears with three options:
   "Survey Grid", "Corridor Pattern", "Perimeter Pattern".
2. **Select "Survey Grid".** Post-condition: the map cursor becomes a
   polygon-drawing cursor; a tooltip appears: "Click to add polygon
   vertices; double-click to close and generate".
3. **Click on the map 4 times** to draw a polygon (e.g. a 100 m × 100 m
   square), then **double-click** to close. Post-condition: a modal
   appears titled "Survey Grid parameters" with fields:
   - Leg spacing (m): default 10.
   - Altitude (m): default 30.
   - Camera trigger interval (s): default 5.
   - Leg direction (deg): default 0 (north-south).
   - Generate / Cancel buttons.
4. **Keep defaults, click "Generate".** Post-condition (within 200 ms):
   the modal closes; the map shows 10 parallel legs inside the polygon
   (5 north-south, 5 south-north, alternating); 21 waypoints appear
   (1 takeoff implied + 20 leg waypoints + camera-trigger markers
   between legs); the side table updates with 21 rows; the side table
   shows each waypoint's `command` (16 for NAV_WAYPOINT, 200 for
   CAMERA_TRIGGER as appropriate).
5. **Click the "Validate" button** in the editor panel toolbar.
   Post-condition: a toast appears: "Validation passed: 21 waypoints, 0
   fence, 0 rally" (no fence has been drawn yet; the survey is inside
   the polygon but no formal fence exists).
   - **Error path A:** if any generated waypoint falls outside the
     polygon (a bug in the generator), validation fails with the
     offending waypoint highlighted.
6. **Draw a geofence around the polygon** (§8.1 steps 6-7).
7. **Re-validate**, then **Save** with name "demo-survey", **Upload to
   vehicle 0** (§8.1 steps 8-13).

## 9. Verification Gates (G-ladder)

The G-ladder extends the existing I/F/S/O/R ladder recorded in
`docs/VERIFICATION.md`. Each gate is a single-invocation harness under
`console/tests/` (or `scripts/` for cross-cutting ones), runs against
real PX4 SITL, asserts PASS/FAIL with an exit code, and is named with a
`G-` prefix to distinguish from the existing ladder.

| Gate | Name | What it asserts | Harness path | Est. runtime |
|---|---|---|---|---|
| G-0 (NEW v0.2) | PX4 version check | `make px4_sitl_default` build dir's `git describe --tags` == v1.16.2; `:8300` rejects upload to a vehicle reporting a different version with HTTP 426 | `console/tests/run_g0_version.sh` | ~10 s |
| G-1 | Mission validation | Schema + geofence geometry + rally containment; rejects empty/out-of-fence missions; vertex-drag stress test (50+ vertices, no UI freeze — QGC v5.1.4 bug class) | `console/tests/run_g1_validation.sh` | ~30 s |
| G-2 | Mission persistence | CRUD round-trip (create, list, fetch, update, delete); survives catalog restart | `console/tests/run_g2_persistence.sh` | ~30 s |
| G-3 | Mission upload (3 types) | MAVLink mission protocol: count → items → ack for all three `MAV_MISSION_TYPE` values; all items ack'd by PX4; rollback on failure | `console/tests/run_g3_upload.sh` | ~1 min |
| G-4 | Mission download (3 types) | Download from PX4 after upload; round-trip equality (seq, command, x, y, z) for all three types | `console/tests/run_g4_download.sh` | ~1 min |
| G-5 | Fly View 1-vehicle telemetry | 10 Hz telemetry moves on map + strip + attitude HUD for 60 s | `console/tests/run_g5_flyview.sh` | ~1.5 min |
| G-6 | Multi-vehicle Fly View | 2 vehicles on map simultaneously, select-active switch <100 ms | `console/tests/run_g6_multivehicle.sh` | ~2 min |
| G-7 | Pre-arm + arm/disarm | Pre-arm checks run, block arm when failing, pass when fixed, arm→disarm round-trip | `console/tests/run_g7_arm.sh` | ~2 min |
| G-8 | Vehicle Setup extensions | Param search filter, preset save/load round-trip, diff-against-defaults | `console/tests/run_g8_setup_ext.sh` | ~2 min |
| G-9 | Fleet mission binding | Vehicle 0 mission A + vehicle 1 mission B, both fly correctly | `console/tests/run_g9_binding.sh` | ~3 min |
| G-10 | Fleet orchestration | Sequential mode: vehicle 1 starts after vehicle 0 reaches WP1; parallel mode: both start together | `console/tests/run_g10_orchestration.sh` | ~3 min |
| G-11 | ULog browse + plot | List `.ulg` files, list topics, fetch topic data, plot in chart | `console/tests/run_g11_ulog.sh` | ~1 min |
| G-12 | Replay scrub + overlay | Load `.replay`, scrub timeline, plot topic, overlay live vehicle | `console/tests/run_g12_replay.sh` | ~1.5 min |
| G-13 (NEW v0.2) | Survey/corridor/perimeter patterns | Generate each pattern on a known polygon; assert waypoint count, all-inside-polygon, no gaps > leg spacing | `console/tests/run_g13_patterns.sh` | ~1 min |

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
| M1: Plan View MVP | Mission editor, TOML persistence, CRUD endpoints, validation, G-0 version gate | G-0, G-1, G-2 | 2 weeks |
| M2: Mission upload/download | MAVLink mission protocol integration with fleet-cli; upload + download round-trip (3 types) | G-3, G-4 | 1.5 weeks |
| M3: Fly View extensions | Instrument widgets, multi-vehicle active-select, pre-arm checklist | G-5, G-6, G-7 | 2 weeks |
| M4: Vehicle Setup extensions | Param search, presets, diff-against-defaults | G-8 | 1 week |
| M5: Fleet mission orchestration | Per-vehicle binding, parallel + sequential modes, swarming patterns | G-9, G-10 | 2.5 weeks |
| M6: Analyze View | ULog browser, replay viewer, overlay | G-11, G-12 | 3 weeks |
| M7 (NEW v0.2): Survey Patterns | Survey grid, corridor, perimeter generators in `console/lib/patterns.ts` | G-13 | 1 week |
| **v1 release** | All 14 gates green; existing I/F/S/O/R ladder re-run green; README + docs updated | I/F/S/O/R + G-0..G-13 | **~13 weeks total** (one engineer, sequential) |

M1 and M3 can be parallelized across two engineers. M2 depends on M1.
M4 is independent (can run parallel to any of M2/M3). M5 depends on M2.
M6 is independent (can run parallel to M3/M4/M5). M7 depends on M1
(needs the mission editor). With two engineers, total wall-clock time
drops to ~9 weeks.

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

- **R-8 (NEW v0.2): PX4 v1.18 stable release during v1 development.**
  PX4 v1.18 was entering beta as of July 2026 (quad-drone-lab.co.kr
  weekly briefing). If v1.18 reaches stable during RustSim GCS v1
  development (Q4 2026 or Q1 2027), operators will ask "does it
  support v1.18?" and the answer will be "no, v1.16.2 only" — which
  may feel outdated. **Mitigation:** the explicit PX4 version policy
  (§3.3) and the G-0 gate make this a documented decision, not a
  surprise. A v1.1 spec (out of scope here) will evaluate v1.18 and
  capture its dialect drift in new ADRs. The version-policy page in
  the docs (linked from the README) will state "RustSim GCS v1
  supports PX4 v1.16.2; v1.18 evaluation is tracked in issue #NNN".

- **R-9 (NEW v0.2): QGC Plan View vertex-dragging freeze regression.**
  QGC v5.1.4 (released mid-2026) specifically fixed a bug where
  dragging polyline/polygon vertices in Plan View froze the UI. This
  is the exact same UI pattern RustSim GCS v1's Plan View uses
  (Leaflet markers + drag handlers). Without an explicit stress test,
  RustSim GCS could ship the same bug class. **Mitigation:** G-1
  includes a vertex-drag stress test (50+ vertices, drag each, assert
  no UI freeze and <100 ms render latency per drag). The test is
  documented as a regression guard against the QGC v5.1.4 bug class.

- **R-10 (NEW v0.2): ULog topic schema changes across PX4 versions.**
  PX4's ULog topic names and field layouts are not stable across
  versions; a topic like `vehicle_local_position` may have different
  fields in v1.16.2 vs v1.18. The Analyze View (§5.5) reads topics
  by name. **Mitigation:** the `:8300` `/api/ulogs/{file}/topics`
  endpoint returns the actual topics in the file (not a hardcoded
  list); the UI builds the topic dropdown dynamically. The
  `pyulog`-based parser handles arbitrary field layouts. This is
  defense-in-depth against schema drift; combined with R-8's version
  policy, the risk is contained.

- **R-11 (NEW v0.2): Web-based GCS performance ceiling.** A
  web-based GCS (vs. QGC's native Qt) has inherent overhead: browser
  rendering, JS garbage collection, WS framing. With 4+ vehicles at
  10 Hz telemetry each, the strip charts and map markers must render
  without dropping frames. **Mitigation:** v1's telemetry is 10 Hz
  (not 200 Hz — that's the replay rate, only used in Analyze View
  with WS streaming); the strip chart uses ECharts' `appendData` API
  (not redraw); the map uses Leaflet's `Marker` (not `L.circleMarker`
  which is slower); the Fleet C2 table virtualizes rows beyond 8
  vehicles. A performance gate G-5 (and G-6) measures frame rate and
  asserts no drop below 30 FPS for 60 s.

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
- **Q-9 (ADR-0027):** `:8300` server shape — new `fleet-mission` binary
  vs `fleet-cli serve` extension vs a `console/api/` Next.js API route
  that proxies to a Rust sidecar. Current lean: new `fleet-mission`
  binary.
- **Q-10 (ADR-0028, NEW v0.2):** Survey pattern generator location —
  client-side (`console/lib/patterns.ts`, current plan, no backend
  needed) vs server-side (`:8300` exposes `/api/patterns/{name}/generate`).
  Current lean: client-side; simpler, no new endpoints, patterns are
  deterministic geometry.
- **Q-11 (ADR-0029, NEW v0.2):** PX4 version policy enforcement — hard
  reject (current plan, HTTP 426) vs warning + allow (operator accepts
  risk). Current lean: hard reject; the dialect drift documented in
  existing ADRs means "warning + allow" is silent breakage.

These are explicitly open (not ADR-bound):

- **Q-12 (Open, not ADR-bound):** Should v1 support ArduPilot SITL in
  addition to PX4 SITL? The MAVLink mission protocol is shared, but the
  HIL simulator and fleet manager are PX4-specific. Adding ArduPilot
  would require a parallel HIL backend. **Defer to v2 spec; v1 is
  PX4-only.**

- **Q-13 (Open):** Should there be a "scenario recorder" that turns a
  live flight into a `.replay` file for later analysis? `sitsim-cli`
  already produces `.replay` files for every `scenario-run`; this would
  just expose them through the catalog UI. **Likely yes for v1, but
  scope to M6.**

- **Q-14 (Open):** Should the console be installable as a PWA for
  offline use? The dual-mode routing and Leaflet tiles both work
  offline once loaded, but missions and replays require the `:8300`
  server. **Defer to v1.1; v1 is browser-only.**

- **Q-15 (Open, NEW v0.2):** Should the GCS support 3D terrain/obstacle
  view (Cesium or MapboxGL 3D)? QGC's user community lists this as the
  most-wanted feature. **Defer to v2; v1 is 2D Leaflet only (§3.2).**

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
| 0027 | `:8300` server shape (new binary vs fleet-cli extension vs Next API) | M1 | Proposed |
| 0028 (NEW v0.2) | Survey pattern generator location (client vs server) | M7 | Proposed |
| 0029 (NEW v0.2) | PX4 version policy enforcement (hard reject vs warn) | M1 | Proposed |

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

### External — market research (grounding for §1.1, §2.3, §5.4, §5.6)

- Fortune Business Insights, "Drone Simulator Market Size, Share, Industry
  Growth, 2034" (Aug 10, 2026) —
  https://www.fortunebusinessinsights.com — USD 1.38B (2026) → USD 4.15B
  (2034) at 14.71% CAGR.
- Straits Research, "Drone Simulator Market Size, Share, Growth, Analysis"
  (2026) — https://straitsresearch.com — USD 1.53B (2026) → USD 4.43B
  (2034) at 14.2% CAGR.
- Market Research Future, "Ground Control Station Market Size, Share
  Report By 2035" (Aug 24, 2026) — https://www.marketresearchfuture.com —
  UAV market projected to reach USD 50B by 2026.
- Dataintelo, "Drone Ground Control Station Market Research Report 2033"
  (2026) — https://dataintelo.com — USD 5.6B (2024), 12.8% CAGR.
- Maximizemarketresearch, "Drone Simulator Market — Global Industry
  Analysis" (Apr 7, 2026) — https://www.maximizemarketresearch.com —
  USD 1.21B (2025) → USD 3.18B (2032).
- SNS Insider, "UAV Flight Training And Simulation Market Size, Share &
  Forecast" (Apr 24, 2026) — https://www.snsinsider.com — USD 3.00B
  (2025) → USD 16.46B (2035).
- Growth Market Reports, "Drone Swarm Control Ground Station Market 2034"
  (2026) — https://growthmarketreports.com — USD 1.60B (2025) → USD 4.87B
  (2034) at 13.1% CAGR.
- Drone Intelligence AI, "Ground Control Station Market 2026 Forecast"
  (2026) — https://droneintelligence.ai — USD 9.60B (2025) → USD 60.10B
  (2034).

### External — competitive landscape (grounding for §2.2, §2.3, §5.5, §5.6)

- QGroundControl releases — https://github.com/mavlink/qgroundcontrol
  (v5.1.x current as of Sep 2026; v5.1.4 fixed Plan View vertex-drag
  freeze).
- QGroundControl docs — https://docs.qgroundcontrol.com (Plan Toolbar,
  Plan View architecture reference).
- Mission Planner Flight PLAN — https://ardupilot.org (survey + corridor
  pattern generators).
- DroneDeploy, "Flight Plan Settings Overview" (Mar 13, 2026) —
  https://help.dronedeploy.com (cloud mission planning, camera-trigger
  patterns).
- AirHub, "Waypoint Patterns: Automated Flight Paths for Drone Scouting"
  (Jul 15, 2025) — https://www.airhub.app (pattern library reference).
- DroneFly, "Drone Flight Path Planning Software Explained" (Jul 11,
  2025) — https://www.dronefly.com (mission planning software overview).
- PX4 Flight Review — https://github.com/PX4/flight_review (official
  ULog analysis web app; competitive reference for §5.5 Analyze View).
- Foxglove, "What is PX4 ULog?" — https://foxglove.dev (general-purpose
  web visualization; competitive reference for §5.5).
- Aomway, "Using ULog + Flight Review to Diagnose Problems" (Aug 17,
  2026) — https://www.aomway.com (ULog analysis workflow reference).
- A-Bots, "Swarm of Drones and Drone Show Apps" (Sep 19, 2025) —
  https://a-bots.com (swarm platform architecture; validates RustSim's
  plan + choreograph + execute split).
- Arxiv, "Optimal Allocation and Trajectories for Swarm Drone" (Mar 25,
  2026) — https://arxiv.org (academic swarm trajectory optimization;
  validates RustSim's auction allocator).

### External — protocol references (grounding for §5.1, §3.3)

- MAVLink Mission Protocol — https://mavlink.io (updated Aug 12, 2026;
  documents the 3-type `MAV_MISSION_TYPE` enum: mission/fence/rally).
- MAVLink Common Message Set (common.xml) — https://mavlink.io (updated
  Sep 3, 2026; `MAV_MISSION_TYPE_MISSION` enum reference).
- PX4 v1.16 Simulation docs — https://docs.px4.io (HIL + SITL reference).
- Quad-Drone-Lab, "PX4 weekly briefing" (Jul 20, 2026) —
  https://quad-drone-lab.co.kr (PX4 v1.18 beta entry; grounds R-8).
- Hamish Willee, "Mission Protocol" — https://hamishwillee.gitbooks.io
  (MAVLink 2 mission types reference).

---

**End of spec v0.2.** Implementation begins after ADRs 0019, 0020, 0026,
and 0029 are accepted (the M1-blocking decisions, plus the new
version-policy ADR). All other ADRs can be resolved in parallel with
their owning milestone.
