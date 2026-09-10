# ADR-0017: Operator map control plane — geo map view with direct SITL control

**Status**: Accepted (v0.2). **Note (2026-09-10 cleanup)**: the
operator mission + guided command endpoints (`POST /api/mission{,
/start,/clear}`, `POST /api/vehicles/{i}/{arm,takeoff,land,rtl,hold,
goto}`) are still current and live on `:8400`. The console's Operator
Map is now the full-bleed MapLibre canvas on the Operations Canvas
(post-2026-09-10 the GCS uses overlay panels, not tabs; see
`../console/AGENTS.md`). However, the **auction allocator**, the
**per-vehicle mission runner**, the **20 Hz offboard setpoint pump**,
the **hover observation + safety ladder** referenced below were all
removed end-to-end in the 2026-09-10 cleanup. Operator-uploaded
missions are no longer flown autonomously by the auction — the
operator drives them via `POST /api/fleet/start` (parallel or
sequential) or via `POST /api/vehicles/{i}/goto` per vehicle. The
geo→NED conversion, the fence-validated upload, the `op*` id namespace,
and the `geo_origin` block on FleetFrame are all still current.

**Context**

The console's only spatial view is the Fleet C2 NED canvas (home-relative
metres, read-only task board) and mission tasks come exclusively from the
scenario TOML at startup. The README roadmap calls for "mission editing
(waypoint placement on the map)"; the operator-facing ask is broader: a
**map view where the user controls SITL themselves**, the way QGroundControl
and Mission Planner do.

Survey of the products this feature imitates:

- **QGC Fly View**: vehicle marker + trajectory on a geo map, an action bar
  (arm/disarm, emergency stop, takeoff, land, RTL, pause) and *map position
  actions* — click the map, a popup offers **Go To Location** / Orbit, confirm
  and the vehicle flies there; Start/Continue Mission buttons gate mission
  execution.
- **QGC Plan View**: numbered waypoint markers + a Planned Home on the map;
  click to select, **drag to reposition**, per-waypoint altitude editing, path
  lines with direction arrows, then an **Upload** toolbar button sends the
  mission to the vehicle.
- **Mission Planner Flight Planner**: right-click the map to add waypoints, a
  waypoint table with altitudes, "Write WPs" (upload) / "Read WPs".
- **mavelous** (open-source browser GCS): Leaflet map, double-click sends the
  drone to that location — the direct precedent for browser-side go-to.

**Decision**

Add an *operator control plane* to mavfleet + an *Operator Map* view to the
console, following the ADR-0016 discipline (thin REST plane over live links,
every write is the same MAVLink the supervisor itself sends, LIVE/SIMULATED
dual-mode console):

1. **Geodesy in fleet** (`fleet-core/src/geo.rs`): a std-only WGS84
   `GeoOrigin` (`ned_to_geodetic` via ECEF + Bowring, `geodetic_to_ned`),
   ported from rustsitsim's proven `sitsim-env/src/geodesy.rs` (< 1 mm
   round-trip over 10 km). The fleet converts; the console never invents
   coordinates.
2. **Geo on the wire**: `VehicleView` gains `lat_deg_e7` / `lon_deg_e7` /
   `alt_mm` / `relative_alt_mm` (already decoded from `GLOBAL_POSITION_INT`
   into `VehicleState`, just never published), and `FleetFrame` gains a
   `geo_origin` block `{lat_deg, lon_deg, alt_m}` plus the scenario geofence
   (`points_ned_m`, ceiling, floor) so the console draws the *real* fence on
   both maps (the NED map previously showed only the client fallback).
3. **Single source of origin**: scenario `[env] origin = {lat_deg, lon_deg,
   alt_m}` (default: the PX4 test field 47.397770, 8.545580, 500 m). The
   manager uses it for all geo conversion; `SimCtl` exports it as
   `RSIM_ORIGIN_LAT/LON/ALT` to the spawned sim wrapper, and
   `run_sitsim_vehicle.sh` writes it into the per-vehicle TOML — the sim's
   HIL_GPS and the fleet's conversions can no longer disagree.
4. **Operator mission endpoints** (the manager owns the mission machinery;
   REST queues commands, the supervisor tick drains them — same discipline as
   ADR-0016 restarts):
   - `POST /api/mission` — upload operator waypoints `{items: [{lat_deg,
     lon_deg, alt_m, hover_s}], mode: append|replace}`; each is converted
     geo→NED and validated with the *compiler's own rules* (inside the fence
     polygon, inside the altitude box, reachable) — invalid items are
     rejected with reasons, never discovered mid-flight. Operator tasks get
     `op*` ids and enter the same task board, pool and sequential auction as
     scenario tasks; the mission runners, offboard 20 Hz pump, hover
     observation and safety ladder fly them unchanged.
   - `POST /api/mission/start` — starts the deferred mission (the
     `hold_for_setup` bench flips to RUNNING; an already-running mission is a
     no-op ack).
   - `POST /api/mission/clear` — drops queued (not active) operator tasks.
5. **Guided command endpoints** (QGC Fly-View action bar; direct
   `LinkHandle` writes from the REST handler, exactly like the setup plane,
   gated on "vehicle not flying a mission"):
   - `POST /api/vehicles/{i}/arm` `{arm: bool}` — COMPONENT_ARM_DISARM with
     the same TEMPORARILY_REJECTED retry ladder as `spawn_engage`.
   - `POST /api/vehicles/{i}/takeoff` `{alt_m}` — MAV_CMD_NAV_TAKEOFF (22)
     with param7 altitude: QGC's takeoff semantics (PX4 arms and climbs).
   - `POST /api/vehicles/{i}/land`, `…/rtl` — AUTO.LAND / AUTO.RTL **plus**
     setpoint-stream stop (a go-to-flown vehicle is streaming OFFBOARD
     setpoints; the mode change alone doesn't stop the pump).
   - `POST /api/vehicles/{i}/hold` — pause: AUTO.LOITER + stream stop (QGC
     "Pause"; multicopter loiters).
   - `POST /api/vehicles/{i}/goto` `{lat_deg, lon_deg, alt_m?}` — geo→NED,
     clamp into the fence minus 2 m (runner rule §7.2), then the engage
     sequence: hold-at the current estimate (starts the ≥2 Hz stream PX4
     requires), set the goal, arm ladder + DO_SET_MODE(OFFBOARD). The vehicle
     flies to the clicked point and holds — QGC's Go To Location.
6. **Console Operator Map** (4th tab, `forceMount` like the others):
   Leaflet 1.9 integrated directly in a client component (no react-leaflet
   wrapper — the trimmed-dependency contract stands: 2 new deps, `leaflet` +
   `@types/leaflet`). OSM raster tiles online; if tiles fail (sandboxed /
   offline deployment) the layer degrades to a drawn graticule grid and the
   UI says so — the view stays honest and usable. The view provides: live
   vehicle markers with heading + trajectory + home + geofence, a vehicle
   selector and the guided action bar (with AlertDialog confirms for
   arm/takeoff/land/estop), map click = **Go To Location** popup when flying,
   and a **Plan mode**: click to add numbered waypoints, drag to edit,
   per-waypoint altitude/hover fields, upload (append/replace) and Start
   Mission buttons. LIVE/SIMULATED dual-mode like every console view.
7. **Fence honesty on the NED map**: `normalizeFleetSnapshot` now prefers the
   backend-published geofence over the client fallback.

**Consequences**

- The console can command SITL end-to-end: upload a mission on the map, start
  it, go-to, take off, hold, land, RTL, e-stop — all through MAVLink the
  supervisor itself uses, all visible in the event log and run report.
- Operator missions reuse the auction allocator (multi-vehicle upload works
  and is deconflicted by assignment) — deliberately *not* the FC-side
  MAVLink mission protocol (MISSION_COUNT/MISSION_ITEM_INT); our missions fly
  OFFBOARD under the 20 Hz pump with the safety ladder attached, which is the
  repo's established flight path. That scope choice is honest and documented
  here.
- Guided endpoints are gated on `mission_active[i]` (the supervisor publishes
  per-tick whether vehicle i has a live runner) so a user command can never
  fight an autonomous mission; e-stop (policy 1) remains unconditional.
- Geo telemetry comes from PX4's own `GLOBAL_POSITION_INT` (which comes from
  the sim's HIL_GPS) — the map shows the estimate the vehicle actually flies,
  not simulator ground truth.

**Verification**

- `fleet-core` geodesy round-trip unit tests; `fleet-cli` router tests for
  the new endpoints (gate paths, validation, queueing, frame fields).
- `fleet/tests/live_test_operator.sh` (O-1): REST-only live pass — mission
  upload (with an out-of-fence item rejected), start, flight observed via
  `/api/fleet` lat/lon, go-to + hold + land against real PX4 SITL.
- `scripts/browser_map_test.sh` (O-2): browser pass through the gateway —
  Operator Map LIVE, waypoint placement by map click, upload + start, guided
  command, screenshots to `docs/images/`.
- `docs/VERIFICATION.md` records both rows.
