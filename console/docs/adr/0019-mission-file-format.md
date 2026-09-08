# ADR-0019: Mission file format — TOML on disk, JSON on the wire

- Status: Accepted
- Date: 2026-09-09
- Owning milestone: M1 (Plan View MVP)
- Supersedes: none
- Related: GCS_SPEC.md §5.1, §6.1; will be referenced by ADR-0020 (persistence), ADR-0025 (param preset format), ADR-0026 (validation rules)

## Context

The GCS Plan View (GCS_SPEC.md §5.1) needs a serializable mission format
that the operator's browser edits, the `:8300` mission-catalog server
stores on disk, and the MAVLink mission-protocol upload path translates
into `MISSION_ITEM_INT` messages. The same format must also carry the
geofence (inclusion + exclusion polygons, ceiling, floor) and rally
points, because per the mavlink.io Mission Protocol spec (updated Aug 12
2026) all three are uploaded as separate `MAV_MISSION_TYPE` transactions
but they are authored together as one operator-facing "mission."

Three candidate formats were considered:

1. **TOML** — human-readable, the format already used for RustSim
   scenario files (`fleet/tests/demo_live.toml`, `sim/tests/i2_flight.toml`)
   and for PX4 SITL instance config. Rust's `toml` crate is mature
   (already a workspace dep in `sim/` and `fleet/`). Authoring a mission
   by hand in a text editor is practical.

2. **JSON** — the format QGC uses for `.plan` files
   (QGC Plan File Specification, qgroundcontrol.com docs). Native to
   the browser (`JSON.parse`/`JSON.stringify`); native to the REST API
   (every endpoint returns JSON). Not pleasant to hand-edit.

3. **QGC `.plan`** — a specific JSON schema (with `mission`, `complexItems`,
   `geoFence`, `rallyPoints` top-level keys) designed for cross-tool
   compatibility between QGC and ArduPilot Mission Planner. Documented at
   https://github.com/mavlink/qgroundcontrol/blob/master/src/PlanView/PlanMasterController.cc

The decision matters because the format is sticky: once missions are
saved on disk in one format, migrating to another is a one-way conversion
that loses comments, ordering, and any format-specific features. The
catalog server (`:8300`) and every consumer (Plan View editor, Analyze
View replay overlay, fleet-mission binding, swarming-pattern generator)
will all read and write this format for the lifetime of v1.

## Decision

**TOML on disk, JSON on the wire.** Missions are stored in the
`:8300` catalog as TOML files (`<ulid>.toml`). Every REST endpoint on
`:8300` that returns a mission returns it as JSON (a `serde_json::Value`
produced by `toml::from_str` → `serde_json::to_value`), and every
endpoint that accepts a mission body accepts either TOML or JSON
(content-type sniffed: `application/toml` → TOML, `application/json`
or default → JSON). The two representations are losslessly convertible
because they share one serde struct (`MissionFile`).

### The `MissionFile` struct (the single source of truth)

```rust
// console/docs/adr/0019-sketch.rs — illustrative, not authoritative
// until M1 implementation; the real struct lives in fleet-mission or
// fleet-core, TBD by ADR-0027.

use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct MissionFile {
    pub mission: MissionMeta,
    pub geofence: Geofence,
    pub rally: Vec<RallyPoint>,
    pub waypoints: Vec<Waypoint>,  // the flight plan; MAV_MISSION_TYPE_MISSION
}

#[derive(Serialize, Deserialize)]
pub struct MissionMeta {
    pub id: String,            // ULID, sortable
    pub name: String,
    pub version: u32,
    pub created_at: String,    // RFC 3339
    pub updated_at: String,
    pub vehicle_type: String, // "quad" | "fixed" | "vtol" | "rover"
    pub px4_version: String,  // "v1.16.2" — for ADR-0029 version gate
}

#[derive(Serialize, Deserialize)]
pub struct Waypoint {
    pub seq: u16,
    pub frame: u8,         // MAV_FRAME_*
    pub command: u16,     // MAV_CMD_*
    pub x: f64,           // lat (frame 0/3) or x (frame 1/8)
    pub y: f64,           // lon or y
    pub z: f32,           // alt_m
    pub param1: f32,      // hold_s for MAV_CMD_NAV_WAYPOINT
    pub param2: f32,      // accept_radius_m
    pub param3: f32,      // pass_radius_m
    pub param4: f32,      // yaw_deg (NaN = face direction of travel)
}

#[derive(Serialize, Deserialize)]
pub struct Geofence {
    pub ceiling_m: f32,
    pub floor_m: f32,
    pub inclusion: Vec<[f64; 2]>,      // [lat, lon] polygon; empty = no fence
    pub exclusion: Vec<Vec<[f64; 2]>>, // list of polygons
}

#[derive(Serialize, Deserialize)]
pub struct RallyPoint {
    pub seq: u16,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f32,
}
```

### Why TOML wins on disk

- **Consistency with the existing repo.** Every RustSim config file is
  TOML (scenario files, sim instance config, replay scenario files). A
  GCS operator who already edits `demo_live.toml` by hand edits
  `<ulid>.toml` by hand with the same muscle memory.
- **Human readability for hand-authoring.** The README's "Quickstart"
  shows operators hand-editing scenario TOMLs; the same workflow extends
  to missions. JSON's noise (quotes around every key, no comments,
  trailing-comma strictness) makes hand-authoring error-prone.
- **Comments survive.** TOML supports inline comments (`#`); an operator
  annotating "why is WP3 at 15 m?" keeps the annotation. JSON has no
  comments; a `.plan` file with a `comment` field is a non-standard hack.
- **The `toml` crate is already a workspace dep.** No new dependency.

### Why JSON wins on the wire

- **The browser speaks JSON natively.** `fetch().then(r => r.json())`
  is the default; a TOML response would require shipping a TOML parser
  to the client (`smol-toml` is ~30 KB minified).
- **Every other REST endpoint in RustSim returns JSON.** Console
  normalizers (`src/lib/conn.ts`) expect JSON; mixing TOML and JSON
  responses would force per-endpoint parsing logic.
- **`serde_json` is already a workspace dep.** No new dependency.

### Why not QGC `.plan`

- **Cross-tool compatibility is a non-goal for v1.** GCS_SPEC.md §3.2
  commits to PX4 SITL only; operators using RustSim GCS v1 do not also
  use QGC against the same vehicles (the RustSim simulator replaces
  QGC's Gazebo/jMAVSim, and RustSim's mission protocol replaces QGC's
  Plan View upload). A `.plan` import/export path is a v1.1 candidate,
  not a v1 requirement.
- **`.plan`'s schema is over-engineered for RustSim's needs.** Its
  `complexItems` array carries survey-grid definitions, camera triggers,
  and corridor patterns as nested structures — exactly what
  GCS_SPEC.md §5.6 puts in a separate client-side generator
  (`console/lib/patterns.ts`) that emits plain `Waypoint` objects. A
  `.plan` file would force the catalog server to parse and re-emit
  `complexItems`, doubling the surface area.
- **`.plan`'s `mission` block uses MAVLink frame/command integers** but
  wraps them in a `MissionItem` struct with string-typed `command` fields
  (`"MAV_CMD_NAV_WAYPOINT"` not `16`). This is friendly to QGC's C++
  enum-lookup code but unfriendly to RustSim's serde structs, which
  would need a custom deserializer.

### Cross-format import/export (v1.1 candidate, not v1)

A future v1.1 may add:
- `POST /api/missions/import?qgc_plan=<file>` — parse a `.plan` file,
  extract waypoints + fence + rally, emit a `MissionFile` TOML.
- `GET /api/missions/{id}?format=qgc_plan` — emit a `.plan` file from
  a stored mission.

These are explicitly out of v1 scope. If an operator needs to share a
mission with a QGC user in v1, they re-create it in QGC by hand (the
operator can read the RustSim TOML and transcribe the lat/lon/alt
values). This is acceptable because v1 operators are SITL-only and
the cross-over with QGC users is low.

## Consequences

### Positive

- **One serde struct, two serializations.** The `MissionFile` struct
  is the single source of truth; TOML and JSON are both `#[derive(Serialize, Deserialize)]`
  outputs. A schema change is one Rust edit, not two-format migrations.
- **Hand-editable missions.** Operators can `cat` a mission file, edit
  a waypoint's altitude, and `curl PUT` it back — no QGC required.
  This matches the existing scenario-file workflow.
- **No new dependencies.** `toml` and `serde_json` are already in the
  workspace.
- **Comments preserved on disk.** The catalog's TOML writer
  (`toml::to_string_pretty`) preserves the struct fields; an operator
  who hand-edits a comment into the TOML keeps it through round-trips
  that go TOML → struct → TOML. (Round-trips through JSON lose comments;
  this is documented in `:8300`'s API doc as a known limitation.)

### Negative

- **Two formats to test.** Every catalog round-trip test must cover
  both TOML-on-disk and JSON-on-the-wire. The G-2 persistence harness
  (GCS_SPEC.md §9) exercises both.
- **No direct QGC interoperability.** An operator cannot drop a QGC
  `.plan` file into the catalog. The mitigation (v1.1 import/export)
  is documented above.
- **Comments lost on JSON round-trips.** If an operator hand-edits a
  TOML comment, then the catalog re-serializes via JSON (e.g. through
  a `PUT` with a JSON body), the comment is gone. The catalog's write
  path always writes TOML to disk (not JSON), so this only happens if
  the operator uses the REST API to update — which is the documented
  behavior and is acceptable.

### Neutral

- **ULID-based filenames.** `<ulid>.toml` is sortable by mtime
  (ULIDs are time-ordered) and collision-free. The catalog stores all
  missions in one directory (`/var/lib/rustsim/missions/` or
  `$RSIM_MISSION_DIR`, see ADR-0020); with 1000 missions the directory
  is still small enough for `readdir` to be fast (<10 ms).

## Alternatives considered

### JSON on disk (rejected)

Same struct, but `.json` files on disk. Loses comments, loses
human-readability for hand-authoring, gains nothing (the `serde_json`
crate produces the same JSON either way). The wire format is already
JSON; the disk format is the only place TOML's human-readability
matters.

### MessagePack on disk (rejected)

Binary, compact, fast. But unreadable by humans — defeats the
hand-editing workflow. Considered for the replay file format
(which is already binary, see `sim/docs/SPEC.md` §8.1), but rejected
for missions because missions are operator-authored, not
simulator-produced.

### SQLite with TOML columns (rejected, deferred to ADR-0020)

Store missions as TOML text in a SQLite `missions` table. This is
the ADR-0020 question (persistence layer), not the ADR-0019 question
(file format). ADR-0019 decides the *format*; ADR-0020 decides the
*storage*. If ADR-0020 picks SQLite, the TOML text still goes in a
TEXT column.

## Verification

- **Unit:** the `fleet-mission` (or `fleet-cli`, per ADR-0027) crate
  has a `mission_format` test module that round-trips a known mission
  through TOML → struct → TOML (asserts byte-equality modulo comments)
  and through TOML → struct → JSON → struct → TOML (asserts field
  equality, comments may be lost).
- **Integration:** G-2 (GCS_SPEC.md §9) exercises the full catalog
  CRUD round-trip: create via REST (JSON body) → fetch via REST (JSON
  body) → fetch via REST with `?format=toml` (TOML body) → update via
  REST (TOML body) → fetch on-disk file (TOML) → compare.
- **Regression:** the existing `mavfleet check tests/demo_live.toml`
  harness (which already parses TOML scenarios) continues to pass
  after the `MissionFile` struct is introduced — the struct is
  additive, it does not replace the scenario TOML format.

## References

- GCS_SPEC.md §5.1 (Mission Planning), §6.1 (Mission data model), §7
  (API contracts)
- mavlink.io Mission Protocol spec (updated Aug 12 2026) —
  https://mavlink.io — documents the 3-type `MAV_MISSION_TYPE` enum
- QGC Plan File Specification —
  https://github.com/mavlink/qgroundcontrol/blob/master/src/PlanView/PlanMasterController.cc
- Existing RustSim TOML usage: `fleet/tests/demo_live.toml`,
  `sim/tests/i2_flight.toml`
- ADR-0020 (persistence — where the TOML files live)
- ADR-0025 (param preset format — will follow the same TOML-on-disk,
  JSON-on-the-wire pattern for consistency)
- ADR-0026 (validation rules — operates on the `MissionFile` struct)
- ADR-0027 (`:8300` server shape — where the struct lives)
