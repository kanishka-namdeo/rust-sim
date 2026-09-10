# ADR-0026: Mission validation rules — strict bounds, any polygon, rally ≤ 5

- Status: Accepted
- Date: 2026-09-09
- Owning milestone: M1 (Plan View MVP)
- Supersedes: none
- Related: GCS_SPEC.md §5.1 (acceptance criteria AC-5.1.1, AC-5.1.2),
  §5.6 (survey patterns), §6.1 (data model), §8.1 step 8 (Validate
  button UX), §9 G-1 (validation gate); ADR-0019 (file format — the
  struct this ADR validates)

## Context

GCS_SPEC.md §5.1 AC-5.1.1 and AC-5.1.2 require the Plan View to
validate missions before save and upload: empty missions are rejected;
waypoints outside the inclusion geofence are rejected with the
offending waypoint highlighted. But the spec left the *exact* validation
rules open (GCS_SPEC.md §12 Q-8 / ADR-0026): altitude bounds, fence
containment strictness, polygon shape constraints, rally point count
limits.

The decision matters because validation rules are the operator's
safety net. A mission that passes validation but cannot fly (waypoint
too high for the airframe, fence polygon self-intersecting so
containment is undefined, rally point outside the fence so the
"safe landing" is itself a breach) is worse than no validation at all
— it gives false confidence. The rules must be conservative enough to
catch real errors but not so strict that legitimate missions are
rejected.

This ADR draws on:
- PX4's own arming and mission-acceptance rules (PX4 User Guide v1.16,
  "Mission Planning" section — https://docs.px4.io)
- QGC's Plan View validation (QGC v5.1 source, `PlanMasterController.cc`)
- The existing RustSim fleet-compiler validation
  (`fleet-core/src/compiler.rs`, verified by F-1/F-2), which already
  validates scenario tasks against the geofence polygon and altitude
  box.

## Decision

**Strict per-vehicle-type altitude bounds, any simple polygon for
the fence, rally point count ≤ 5, all waypoints/rally inside the
inclusion fence.** Validation runs server-side on `:8300` (via
`POST /api/missions/{id}/validate`) AND client-side in the Plan View
editor (so the operator gets immediate feedback before save). The
two implementations share a single rules document (this ADR); if they
diverge, the server-side implementation is authoritative and the
client-side is a bug.

### Rule V-1: Mission must have at least one waypoint

A mission with zero waypoints is invalid. The error message is:
`"mission must have at least one waypoint"`. The Plan View editor
disables the Save and Upload buttons when the waypoint count is zero.

### Rule V-2: Waypoint count must not exceed 100

PX4's mission pool has a compile-time limit (default 1000 in v1.16.2's
`param/COM_MISSSION_MAX`, but QGC imposes a UI limit of 1000). RustSim
v1 imposes 100 to keep the editor responsive and the upload protocol
fast (100 × `MISSION_ITEM_INT` ≈ 10 KB, ACK'd in <2 s on a healthy
loop). The error message: `"mission has {N} waypoints; maximum is 100"`.
Operators who need more waypoints should split into multiple missions
or use the survey-pattern generator (GCS_SPEC.md §5.6), which can
produce more waypoints but is bounded by the same limit.

### Rule V-3: All waypoints must be inside the inclusion geofence

Strict containment. A waypoint on the fence boundary (on the polygon
edge) is **inside** (closed polygon semantics). A waypoint outside by
any distance (1 cm or 100 m) is rejected. The error message names the
offending waypoint and the breach distance: `"waypoint {seq} outside
inclusion fence by {distance} m"`. The Plan View highlights the
offending marker in red and the side-table row in red.

If no inclusion fence is defined (the `geofence.inclusion` array is
empty), this rule is **skipped** — the operator can fly without a
fence. This is allowed (the catalog does not require a fence) but
the Fly View's Go To fence validation (GCS_SPEC.md §5.2 AC-5.2.4) is
also skipped, and the fleet's geofence-breach detection (the passive
`GEOFENCE_WARN` health flag in `fleet-safety/geofence.rs` post-2026-09-10
cleanup — the 8-policy safety ladder was removed) is degraded to
altitude-only. The Plan View shows a warning toast on Save: `"no
inclusion fence defined; flight will be unfenced"`.

### Rule V-4: No waypoint may be inside an exclusion polygon

If `geofence.exclusion` is non-empty, every waypoint must be outside
every exclusion polygon. A waypoint on the exclusion boundary is
**outside** (open polygon semantics for exclusion, closed for
inclusion — so a waypoint on the shared edge of an inclusion and
exclusion polygon is inside the inclusion and outside the exclusion,
which is the safe interpretation). Error: `"waypoint {seq} inside
exclusion polygon {index} by {distance} m"`.

### Rule V-5: Altitude bounds are per-vehicle-type

Each vehicle type has a `max_altitude_m` and `min_altitude_m`. A
waypoint's altitude must be within `[min, max]` inclusive. The
defaults (grounded in PX4's `MPC_Z_VEL_MAX_UP/DN` and physical
airframe limits for the default Iris quad):

| vehicle_type | min_altitude_m | max_altitude_m | rationale |
|---|---|---|---|
| `quad` (default Iris) | 0 | 120 | 0 = ground; 120 m = EU/US drone altitude limit |
| `fixed` | 0 | 150 | fixed-wing cruise ceiling in SITL |
| `vtol` | 0 | 150 | VTOL matches fixed-wing in forward flight |
| `rover` | 0 | 0 | rovers do not fly; altitude must be 0 |

Error: `"waypoint {seq} altitude {z} m outside [{min}, {max}] for
vehicle_type {type}"`. An operator can override the bounds by editing
the mission TOML's `[mission] vehicle_type` field — but the catalog
does not expose this in the UI (the UI uses the dropdown). Direct
TOML editing is the escape hatch.

The `vehicle_type` field defaults to `quad` if absent. The Plan View
has a vehicle-type dropdown in the editor panel header; changing it
re-runs validation against the new bounds.

### Rule V-6: Geofence ceiling and floor must bound all waypoint altitudes

`geofence.ceiling_m` must be ≥ the highest waypoint altitude;
`geofence.floor_m` must be ≤ the lowest. Error: `"waypoint {seq}
altitude {z} m above geofence ceiling {ceiling} m"` (or "below floor").
This is a stricter restatement of V-5 when a fence is present — V-5
bounds against the vehicle type, V-6 bounds against the fence. Both
must pass. If the operator sets ceiling = 60 m and a waypoint at 70 m,
V-6 catches it even if V-5 (max 120 for quad) passes.

### Rule V-7: Geofence inclusion polygon must be a simple polygon

A "simple polygon" is one that does not self-intersect. A polygon
where two edges cross (e.g. a bowtie shape) is invalid because
point-in-polygon containment is ambiguous on the crossing. Error:
`"inclusion fence self-intersects at edge {i}–{i+1} and edge {j}–{j+1}"`.

The Plan View's polygon-drawing tool prevents self-intersection by
construction (the drawing algorithm rejects a click that would create
a crossing), but a hand-edited TOML can still produce one. The
server-side validation catches it.

Convexity is **not** required — concave polygons (e.g. an L-shaped
fence around a building corner) are valid and common. The validation
uses the Shamos-Hoey algorithm (O(n log n)) for self-intersection
detection; for n ≤ 50 vertices (the G-1 stress-test limit) this is
sub-millisecond.

### Rule V-8: Geofence inclusion polygon must have ≥ 3 vertices

A polygon with 0, 1, or 2 vertices is not a polygon. Error: `"inclusion
fence must have at least 3 vertices (got {N})"`. A 3-vertex polygon
(a triangle) is the minimum and is valid.

### Rule V-9: Rally point count must not exceed 5

PX4's default `RALLY_POINT_MAX` in v1.16.2 is 5. Exceeding it causes
the rally upload to fail mid-protocol (some points are accepted, the
rest are rejected, the mission pool is left in an inconsistent state).
RustSim v1 enforces 5 as the limit **before** upload. Error:
`"rally point count {N} exceeds maximum 5"`. The Plan View's rally-
point tool disables the "Add rally point" button after the 5th.

### Rule V-10: All rally points must be inside the inclusion geofence

Same containment rule as V-3, applied to rally points. A rally point
is a "safe landing alternative"; a rally point outside the fence is a
safe-landing site the vehicle cannot legally fly to — a contradiction.
Error: `"rally point {seq} outside inclusion fence by {distance} m"`.

If no inclusion fence is defined, this rule is skipped (consistent
with V-3).

### Rule V-11: Rally point altitude must be ≥ floor and ≤ ceiling

Same as V-6, applied to rally points. A rally point's `alt_m` is the
altitude the vehicle will descend to before landing. Error:
`"rally point {seq} altitude {z} m outside [{floor}, {ceiling}]"`.

### Rule V-12: Waypoint `command` must be a supported MAV_CMD

Only the following `MAV_CMD_*` values are accepted in v1 (the set
QGC's Plan View supports for PX4):

| command | name | notes |
|---|---|---|
| 16 | `MAV_CMD_NAV_WAYPOINT` | default; the editor's "add waypoint" produces this |
| 17 | `MAV_CMD_NAV_LOITER_UNLIM` | loiter indefinitely |
| 18 | `MAV_CMD_NAV_LOITER_TURNS` | loiter N turns |
| 19 | `MAV_CMD_NAV_LOITER_TIME` | loiter N seconds |
| 20 | `MAV_CMD_NAV_RETURN_TO_LAUNCH` | RTL |
| 21 | `MAV_CMD_NAV_LAND` | land at this position |
| 22 | `MAV_CMD_NAV_TAKEOFF` | takeoff (rare in mission context; usually a separate action) |
| 31 | `MAV_CMD_NAV_LOITER_TO_ALT` | loiter to altitude |
| 200 | `MAV_CMD_DO_SET_CAM_TRIGG_DIST` | camera trigger (used by survey patterns, §5.6) |

Error: `"waypoint {seq} has unsupported command {cmd}; supported:
16, 17, 18, 19, 20, 21, 22, 31, 200"`. Unsupported commands are
rejected rather than silently passed to PX4, because PX4's behavior
on unknown commands is version-dependent and the version policy
(ADR-0029) requires us to know what we're sending.

### Rule V-13: Waypoint `frame` must be a supported MAV_FRAME

| frame | name | notes |
|---|---|---|
| 0 | `MAV_FRAME_GLOBAL` | absolute alt above MSL |
| 1 | `MAV_FRAME_LOCAL_NED` | NED relative to home (rare in missions) |
| 3 | `MAV_FRAME_GLOBAL_RELATIVE_ALT` | alt relative to home (default; what the editor produces) |
| 10 | `MAV_FRAME_GLOBAL_INT` | like 0 but scaled-int (PX4 prefers this on the wire) |
| 11 | `MAV_FRAME_GLOBAL_RELATIVE_ALT_INT` | like 3 but scaled-int |

Error: `"waypoint {seq} has unsupported frame {frame}; supported: 0,
1, 3, 10, 11"`. The editor produces frame 3 by default; the upload
path converts to frame 11 (scaled-int) on the wire per PX4's
preference.

## Consequences

### Positive

- **Operator safety net.** A mission that passes V-1 through V-13
  is guaranteed to be flyable by a PX4 SITL vehicle of the named
  type, inside the named fence, with rally points the vehicle can
  reach. The operator can click Upload with confidence.
- **Server-side is authoritative.** The client-side validation in
  the Plan View is a UX nicety (immediate feedback); the server-side
  validation in `:8300` is the contract. A hand-edited TOML that
  bypasses the UI is still caught at upload time.
- **Consistency with existing fleet compiler.** V-3 and V-4 (fence
  containment) reuse the `fleet-core/src/compiler.rs` polygon-
  containment code already verified by F-1/F-2. No new geometry code
  for the common case.
- **Self-intersection detection is fast.** Shamos-Hoey at O(n log n)
  for n ≤ 50 is sub-millisecond; the G-1 stress test (50+ vertices)
  will not be slow.

### Negative

- **100-waypoint limit may be too low for some survey missions.**
  A 1 km × 1 km survey at 10 m leg spacing is 100 legs = ~200
  waypoints, exceeding the limit. Mitigation: the survey-pattern
  generator (§5.6) splits large surveys into multiple missions
  automatically (a v1.1 feature; v1 just rejects). The error message
  guides the operator: `"split into multiple missions or reduce
  survey area"`.
- **Per-vehicle-type altitude bounds are hardcoded.** Adding a new
  vehicle type requires a code change (not a config change). The
  bounds live in `fleet-mission/src/validation.rs` as a `match` on
  `vehicle_type`. A v1.1 may externalize them to a TOML config
  file; v1 hardcodes.
- **No support for terrain-following altitudes.** PX4 supports
  `MAV_FRAME_GLOBAL_TERRAIN_ALT` (frame 9) where altitude is above
  the terrain (not above home), but RustSim GCS v1 does not have a
  terrain model (the Leaflet map is 2D, no DEM). Frame 9 is rejected
  by V-13. Terrain-following is a v2 candidate (requires Cesium or a
  DEM tile service).

### Neutral

- **Closed-inclusion, open-exclusion semantics.** A waypoint on the
  shared edge of an inclusion and exclusion polygon is "inside
  inclusion, outside exclusion" — the safe interpretation. This is
  documented in V-3 and V-4; the implementation must use a point-
  in-polygon test that distinguishes boundary from interior
  (the `geo` crate's `Contains` trait does this correctly).

## Alternatives considered

### No validation (rejected)

Trust the operator. PX4 will reject bad missions at upload time
anyway. **Rejected** because PX4's rejection messages are terse
(`"MISSION_ACK: denied"`) and do not name the offending waypoint;
the operator has to debug by binary search. Server-side validation
gives actionable error messages.

### QGC-compatible validation only (rejected)

Adopt QGC's exact validation rules (from `PlanMasterController.cc`).
**Rejected** because QGC's rules are C++-embedded and not documented
as a spec; copying them without understanding would make RustSim
behave identically to QGC but for the wrong reasons. This ADR
re-derives the rules from first principles (PX4's arming rules +
geometry) and cites the sources.

### Strict polygon convexity (rejected)

Require the inclusion fence to be convex. **Rejected** because
concave fences are legitimate (L-shaped around a building, U-shaped
around a runway). Convexity would reject valid operator intent.

### No rally point limit (rejected)

Trust PX4's `RALLY_POINT_MAX` param. **Rejected** because the
default is 5 and exceeding it causes a partial-upload inconsistency
(some points accepted, some rejected) that leaves the mission pool
in a bad state. Pre-validating at 5 prevents the inconsistency.

## Verification

- **Unit:** `fleet-mission/src/validation.rs` has a test per rule
  (V-1 through V-13), each with a passing and failing case. Total
  ~30 test cases.
- **Integration:** G-1 (GCS_SPEC.md §9) exercises:
  - Empty mission → rejected (V-1).
  - 101-waypoint mission → rejected (V-2).
  - Waypoint outside inclusion → rejected with distance (V-3).
  - Waypoint inside exclusion → rejected with distance (V-4).
  - Waypoint at 200 m on a quad → rejected (V-5).
  - Waypoint at 70 m with ceiling 60 → rejected (V-6).
  - Self-intersecting fence → rejected with edge indices (V-7).
  - 2-vertex fence → rejected (V-8).
  - 6 rally points → rejected (V-9).
  - Rally outside fence → rejected (V-10).
  - Rally at -5 m (below floor 0) → rejected (V-11).
  - Waypoint with command 999 → rejected (V-12).
  - Waypoint with frame 99 → rejected (V-13).
  - Valid 4-waypoint mission with fence and 2 rally points → accepted.
  - Vertex-drag stress test (QGC v5.1.4 bug class): 50-vertex fence,
    drag each vertex, assert no UI freeze and <100 ms render latency
    per drag (this is a UI test, runs in the browser harness).
- **Regression:** the existing `mavfleet check tests/demo_live.toml`
  harness continues to pass — the demo scenario has 4 waypoints
  inside its fence, well within all V-rules.

## References

- GCS_SPEC.md §5.1 (mission planning, acceptance criteria), §5.6
  (survey patterns, which must pass the same validation), §6.1
  (data model — the struct these rules validate), §8.1 step 8
  (Validate button UX), §9 G-1 (validation gate), §11 R-9
  (QGC vertex-drag regression risk)
- ADR-0019 (file format — the `MissionFile` struct this ADR's rules
  operate on)
- ADR-0029 (PX4 version policy — the rules assume PX4 v1.16.2's
  `RALLY_POINT_MAX` = 5 and `COM_MISSION_MAX` default 1000)
- PX4 User Guide v1.16, "Mission Planning" — https://docs.px4.io
- QGC v5.1 source, `PlanMasterController.cc` —
  https://github.com/mavlink/qgroundcontrol
- Shamos-Hoey self-intersection algorithm (standard computational
  geometry reference; implemented in the `geo` crate's
  `BooleanOps` trait)
- Existing RustSim fleet compiler: `fleet-core/src/compiler.rs`
  (verified by F-1/F-2; V-3 and V-4 reuse its polygon-containment
  code)
