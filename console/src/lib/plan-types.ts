/**
 * Mission catalog types — the :8300 fleet-catalog contract per
 * ADR-0019 (mission file format) + ADR-0020 (persistence) + ADR-0026
 * (validation rules). The catalog stores missions as three independent
 * MAV_MISSION_TYPE items per the spec (§5.1):
 *
 *   - waypoints[]     → MAV_MISSION_TYPE_MISSION (the flight plan)
 *   - geofence{}      → MAV_MISSION_TYPE_FENCE (inclusion + exclusion)
 *   - rally[]         → MAV_MISSION_TYPE_RALLY (safe-landing points)
 *
 * All shapes are tolerant: the catalog may evolve field names; the
 * `plan-normalize` helpers below adapt to either shape.
 */

/** One catalog mission item — matches MissionWaypoint in fleet-mission/src/gcs/types.rs. */
export interface PlanWaypoint {
  /** Sequence index (0-based; the home waypoint is seq 0 in PX4's pool). */
  seq: number
  /** MAV_FRAME_* (3 = GLOBAL_RELATIVE_ALT, the QGC Plan View default). */
  frame: number
  /** MAV_CMD_* (16 = NAV_WAYPOINT, 21 = LAND, 22 = TAKEOFF, 31 = LOITER_TIME, 200 = RETURN). */
  command: number
  /** Latitude (degrees). The catalog uses x/y/z rather than lat/lon/alt because the
   *  MAVLink mission item int is `x: int32` (lat E7) and the JSON shape mirrors that. */
  x: number
  /** Longitude (degrees). */
  y: number
  /** Altitude AGL (m, QGC Plan View convention). */
  z: number
  /** param1 — for MAV_CMD_NAV_WAYPOINT, hold time (s). */
  param1: number
  /** param2 — accept radius (m). */
  param2: number
  /** param3 — pass radius (m, 0 = ignore). */
  param3: number
  /** param4 — yaw (deg, NaN = ignore). */
  param4: number
}

/** The mission geofence (matches MissionGeofence). */
export interface PlanGeofence {
  ceiling_m: number
  floor_m: number
  /** Inclusion polygon vertices [lat, lon] pairs (≥ 3 to enclose). */
  inclusion: [number, number][]
  /** Exclusion polygons (a list of polygons, each ≥ 3 vertices). */
  exclusion: [number, number][][]
}

/** One rally point (matches RallyPoint). */
export interface PlanRally {
  seq: number
  frame: number
  x: number // lat
  y: number // lon
  z: number // alt AGL
}

/** Mission metadata (matches MissionMeta). */
export interface PlanMissionMeta {
  /** ULID assigned by the catalog (empty string before first save). */
  id: string
  name: string
  /** Version — bumped on every PUT (1, 2, 3, ...). */
  version: number
  /** RFC3339 timestamp. */
  created_at: string
  /** RFC3339 timestamp. */
  updated_at: string
  vehicle_type: string
  px4_version: string
}

/** The full mission file (MissionFile) — what POST/PUT/GET /api/missions/{id} return. */
export interface PlanMissionFile {
  mission: PlanMissionMeta
  waypoints: PlanWaypoint[]
  geofence: PlanGeofence
  rally: PlanRally[]
}

/** One entry in GET /api/missions (MissionSummary). */
export interface PlanMissionSummary {
  id: string
  name: string
  version: number
  updated_at: string
  waypoint_count: number
  fence_count: number
  rally_count: number
  deleted: boolean
}

/** Validation error returned by POST /api/missions/{id}/validate. */
export interface PlanValidationError {
  /** Stable rule id from ADR-0026 (V-1..V-13). */
  rule: string
  /** Human-readable detail. */
  message: string
  /** Waypoint sequence (1-based) when applicable. */
  seq?: number
  /** Distance in metres when applicable (e.g. "outside fence by 8.3 m"). */
  distance?: number
}

/** Validation result. */
export interface PlanValidationResult {
  valid: boolean
  errors: PlanValidationError[]
}

// ---------------------------------------------------------------------------
// tolerant normalizers — the catalog may send slightly different shapes; map
// to the canonical interfaces above.
// ---------------------------------------------------------------------------

type Rec = Record<string, unknown>

function asRec(v: unknown): Rec | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Rec) : null
}

function num(...cands: unknown[]): number {
  for (const c of cands) {
    if (typeof c === 'number' && Number.isFinite(c)) return c
    if (typeof c === 'string' && c.trim() !== '' && Number.isFinite(Number(c))) return Number(c)
  }
  return 0
}

function str(...cands: unknown[]): string {
  for (const c of cands) if (typeof c === 'string' && c.length > 0) return c
  return ''
}

function pair(c: unknown): [number, number] | null {
  if (Array.isArray(c) && c.length >= 2) {
    const a = num(c[0])
    const b = num(c[1])
    return [a, b]
  }
  return null
}

/** Normalize a raw waypoint (catalog or local edit) into canonical shape. */
export function normalizePlanWaypoint(raw: unknown, fallbackSeq: number): PlanWaypoint {
  const r = asRec(raw) ?? {}
  return {
    seq: num(r.seq, r.sequence, fallbackSeq),
    frame: num(r.frame),
    command: num(r.command, r.cmd, 16),
    x: num(r.x, r.lat, r.latitude, 0),
    y: num(r.y, r.lon, r.lng, r.longitude, 0),
    z: num(r.z, r.alt, r.altitude, 12),
    param1: num(r.param1, r.p1, 0),
    param2: num(r.param2, r.p2, 2),
    param3: num(r.param3, r.p3, 0),
    param4: num(r.param4, r.p4, 0),
  }
}

/** Normalize the geofence — inclusion is a single polygon, exclusion is a list of polygons. */
export function normalizePlanGeofence(raw: unknown): PlanGeofence {
  const r = asRec(raw) ?? {}
  const incRaw = Array.isArray(r.inclusion) ? r.inclusion : []
  const inclusion: [number, number][] = incRaw
    .map((v) => pair(v))
    .filter((p): p is [number, number] => p != null)
  const excRaw = Array.isArray(r.exclusion) ? r.exclusion : []
  // exclusion may be a flat list of pairs (single polygon) or a list of polygons
  const exclusion: [number, number][][] = excRaw
    .map((g) => {
      if (Array.isArray(g) && g.length >= 3 && Array.isArray(g[0])) {
        return (g as unknown[]).map((v) => pair(v)).filter((p): p is [number, number] => p != null)
      }
      const single = pair(g)
      return single ? [single] : []
    })
    .filter((poly) => poly.length >= 3)
  return {
    ceiling_m: num(r.ceiling_m, r.ceiling, 60),
    floor_m: num(r.floor_m, r.floor, 0),
    inclusion,
    exclusion,
  }
}

/** Normalize a full mission file (response from GET / POST / PUT). */
export function normalizePlanMissionFile(raw: unknown): PlanMissionFile {
  const r = asRec(raw) ?? {}
  const m = asRec(r.mission) ?? {}
  const mission: PlanMissionMeta = {
    id: str(m.id),
    name: str(m.name, 'untitled'),
    version: num(m.version, 1),
    created_at: str(m.created_at, m.created, ''),
    updated_at: str(m.updated_at, m.updated, ''),
    vehicle_type: str(m.vehicle_type, m.frame_type, 'quad'),
    px4_version: str(m.px4_version, m.autopilot_version, 'v1.16.2'),
  }
  const wpsRaw = Array.isArray(r.waypoints) ? r.waypoints : []
  const waypoints: PlanWaypoint[] = wpsRaw.map((w, i) => normalizePlanWaypoint(w, i))
  const geofence = normalizePlanGeofence(r.geofence)
  const rallyRaw = Array.isArray(r.rally) ? r.rally : []
  const rally: PlanRally[] = rallyRaw.map((rr, i) => {
    const x = asRec(rr) ?? {}
    return {
      seq: num(x.seq, i),
      frame: num(x.frame, 3),
      x: num(x.x, x.lat, 0),
      y: num(x.y, x.lon, 0),
      z: num(x.z, x.alt, 0),
    }
  })
  return { mission, waypoints, geofence, rally }
}

/** Default mission shape used by "New Mission". */
export function emptyPlanMission(name = 'untitled'): PlanMissionFile {
  const now = new Date().toISOString().replace(/\.\d{3}Z$/, 'Z')
  return {
    mission: {
      id: '',
      name,
      version: 0,
      created_at: now,
      updated_at: now,
      vehicle_type: 'quad',
      px4_version: 'v1.16.2',
    },
    waypoints: [],
    geofence: {
      ceiling_m: 60,
      floor_m: 0,
      inclusion: [],
      exclusion: [],
    },
    rally: [],
  }
}

/** The PX4 test field — the catalog + the map's default center. */
export const PX4_TEST_FIELD = { lat: 47.39777, lon: 8.54558 } as const

/** Default waypoint altitude (m AGL, QGC Plan View convention). */
export const DEFAULT_WP_ALT_M = 12

/** Default accept radius (m, MAV_CMD_NAV_WAYPOINT param2). */
export const DEFAULT_WP_ACCEPT_M = 2

/** Default hold time (s, MAV_CMD_NAV_WAYPOINT param1). */
export const DEFAULT_WP_HOLD_S = 0

// ---------------------------------------------------------------------------
// MAVLink mission protocol wire types (M2 — ADR-0019 §5.1)
// ---------------------------------------------------------------------------
// The catalog proxies GET /api/vehicles/{i}/mission?type=mission through to
// the fleet manager (:8400), which speaks the MAVLink mission protocol and
// returns the items in their wire format: x/y are int32 (lat/lon × 1e7),
// matching the MAVLink `MISSION_ITEM_INT` message (msg id 73).
// ---------------------------------------------------------------------------

/** One MAVLink MISSION_ITEM_INT (msg 73) in wire JSON form — int32 lat/lon E7. */
export interface MissionItemIntWire {
  /** Sequence index in the vehicle's mission pool (0-based; 0 = home). */
  seq: number
  /** MAV_FRAME_* (3 = GLOBAL_RELATIVE_ALT). */
  frame: number
  /** MAV_CMD_* (16 = NAV_WAYPOINT, 22 = TAKEOFF, 21 = LAND, 31 = LOITER_TIME, 20 = RETURN). */
  command: number
  /** Whether this item is current (1) or not (0). */
  current: number
  /** 1 = autocontinue to next item, 0 = loiter. */
  autocontinue: number
  /** Latitude × 1e7 (int32). e.g. 473977700 = 47.3977700°. 0 means "not set". */
  x: number
  /** Longitude × 1e7 (int32). */
  y: number
  /** Altitude (m, frame-relative). */
  z: number
  /** param1 — for MAV_CMD_NAV_WAYPOINT, hold time (s). */
  param1: number
  /** param2 — for MAV_CMD_NAV_WAYPOINT, accept radius (m). */
  param2: number
  /** param3 — pass radius (m). */
  param3: number
  /** param4 — yaw (deg, NaN = ignore). */
  param4: number
}

/** Mission type enum (MAV_MISSION_TYPE_*). */
export type MissionType = 'mission' | 'fence' | 'rally'

/** Numeric mission_type id (MAV_MISSION_TYPE enum: 0=mission, 1=fence, 2=rally). */
export const MISSION_TYPE_ID: Record<MissionType, number> = {
  mission: 0,
  fence: 1,
  rally: 2,
}

/** Result of GET /api/vehicles/{i}/mission?type={mission|fence|rally}. */
export interface MissionDownloadResult {
  /** Whether the HTTP request succeeded and the envelope parsed. */
  ok: boolean
  /** HTTP status code (0 = network error). */
  status: number
  /** Stable error code from the catalog's error envelope (null on success). */
  code: string | null
  /** Human-readable error (null on success). */
  error: string | null
  /** Numeric mission_type the vehicle returned (0/1/2). null on failure. */
  missionType: number | null
  /** Downloaded items (already converted to PlanWaypoint shape, E7 → deg). null on failure. */
  items: PlanWaypoint[] | null
  /** Raw wire items (preserved int32 x/y for parity diagnostics). null on failure. */
  rawItems: MissionItemIntWire[] | null
}

/**
 * Normalize a wire MISSION_ITEM_INT (int32 E7 lat/lon) into the canonical
 * PlanWaypoint shape (decimal degrees). Used by the M2 download path.
 */
export function normalizeMissionItemIntWire(raw: unknown, fallbackSeq: number): MissionItemIntWire {
  const r = (raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}) as Record<string, unknown>
  const numOrZero = (...cands: unknown[]): number => {
    for (const c of cands) {
      if (typeof c === 'number' && Number.isFinite(c)) return c
      if (typeof c === 'string' && c.trim() !== '' && Number.isFinite(Number(c))) return Number(c)
    }
    return 0
  }
  return {
    seq: numOrZero(r.seq, r.sequence, fallbackSeq),
    frame: numOrZero(r.frame),
    command: numOrZero(r.command, r.cmd, 16),
    current: numOrZero(r.current),
    autocontinue: numOrZero(r.autocontinue, r.auto_continue, 1),
    // x/y are int32 E7 on the wire — keep raw for parity, convert to deg later.
    x: numOrZero(r.x),
    y: numOrZero(r.y),
    z: numOrZero(r.z, r.alt, 0),
    param1: numOrZero(r.param1, r.p1, 0),
    param2: numOrZero(r.param2, r.p2, 0),
    param3: numOrZero(r.param3, r.p3, 0),
    param4: numOrZero(r.param4, r.p4, 0),
  }
}

/**
 * Convert a wire MISSION_ITEM_INT (int32 E7 lat/lon) into the canonical
 * PlanWaypoint shape (decimal degrees). Used to populate the comparison
 * table against the saved mission.
 */
export function wireItemToWaypoint(w: MissionItemIntWire, fallbackSeq: number): PlanWaypoint {
  return {
    seq: w.seq,
    frame: w.frame,
    command: w.command,
    // x is int32 lat×1e7; convert to degrees. 0 means "not set" → treat as 0.
    x: w.x / 1e7,
    y: w.y / 1e7,
    z: w.z,
    param1: w.param1,
    param2: w.param2,
    param3: w.param3,
    param4: w.param4,
  }
}

/**
 * Floating-point comparison with tolerance for the download/saved mission
 * parity check. Lat/lon E7 round-trip introduces ~1e-7° jitter (≈ 11 mm at
 * the equator); altitude/param rounding introduces ~1e-6 m jitter.
 */
export function waypointsMatch(
  a: PlanWaypoint,
  b: PlanWaypoint,
  opts: { latLonEps?: number; scalarEps?: number } = {},
): boolean {
  const latLonEps = opts.latLonEps ?? 1e-6
  const scalarEps = opts.scalarEps ?? 1e-3
  if (a.command !== b.command) return false
  if (a.frame !== b.frame) return false
  if (Math.abs(a.x - b.x) > latLonEps) return false
  if (Math.abs(a.y - b.y) > latLonEps) return false
  if (Math.abs(a.z - b.z) > scalarEps) return false
  if (Math.abs(a.param1 - b.param1) > scalarEps) return false
  if (Math.abs(a.param2 - b.param2) > scalarEps) return false
  if (Math.abs(a.param3 - b.param3) > scalarEps) return false
  // param4 = yaw, NaN means "ignore" — match if both NaN or both equal.
  const nanA = Number.isNaN(a.param4)
  const nanB = Number.isNaN(b.param4)
  if (nanA !== nanB) return false
  if (!nanA && Math.abs(a.param4 - b.param4) > scalarEps) return false
  return true
}
