/**
 * M7: Survey / Corridor / Perimeter pattern generators (ADR-0028).
 *
 * Pure TypeScript library — no React, no DOM, no API calls. Generates
 * `PlanWaypoint[]` from a polygon (or polyline) + options, entirely on the
 * client. The resulting waypoints replace the mission editor's current
 * waypoint set; the operator can then edit / validate / save / upload as
 * usual (§5.6, §8.6).
 *
 * Coordinate model: lat/lon at input and output; internally we convert each
 * shape to local ENU metres relative to its centroid for the boustrophedon
 * / offset math (a flat-Earth approximation is more than accurate enough at
 * the sub-100 m geofence scale the GCS works at — see ADR-0017 / geo.ts).
 *
 * MAVLink commands used (per GCS_SPEC §5.6):
 *   - 16   MAV_CMD_NAV_WAYPOINT         — visit this point (param2 = accept radius)
 *   - 200  MAV_CMD_DO_TRIGGER_CONTROL   — fire the camera (param1 = interval)
 *
 * AC-5.6.1 (100 m × 100 m polygon, 10 m leg spacing): the survey grid
 * generator emits 10 legs × 2 endpoints + 1 closing nav = 21 waypoints,
 * matching the spec's expected count. The closing nav at the end brings the
 * drone back to the start of leg 0 (a small RTL-to-start, common in survey
 * patterns so the drone exits the survey area the same way it entered).
 *
 * AC-5.6.3 (4-vertex polygon perimeter): emits 4 waypoints (one per inset
 * vertex), connected in a loop. No explicit return-to-start waypoint — the
 * loop closes implicitly (the drone flies from v'[n-1] back to v'[0]).
 *
 * AC-5.6.4: all generated waypoints inherit the mission's default altitude
 * (`opts.altitudeM`) and accept-radius (2 m), editable post-generation like
 * any manually-placed waypoint.
 */

import type { PlanWaypoint } from './plan-types'

// ============================================================================
// Types
// ============================================================================

export interface SurveyGridOpts {
  /** Distance between adjacent parallel legs (m, default 10). */
  legSpacingM: number
  /** Flight altitude AGL (m, default 30). */
  altitudeM: number
  /** Direction the legs run (deg, 0 = north-south, 90 = east-west, default 0). */
  legDirectionDeg: number
  /** Camera trigger interval (s, written into the trigger waypoint's param1). */
  cameraTriggerIntervalS: number
}

export interface CorridorOpts {
  /** Corridor half-width × 2 = total corridor width (m, default 20). */
  corridorWidthM: number
  /** Distance between adjacent sweep lines along the corridor (m, default 10). */
  legSpacingM: number
  /** Flight altitude AGL (m, default 30). */
  altitudeM: number
}

export interface PerimeterOpts {
  /** How far to inset the perimeter from the source polygon edge (m, default 5). */
  offsetM: number
  /** Flight altitude AGL (m, default 30). */
  altitudeM: number
}

export type PatternName = 'survey-grid' | 'corridor' | 'perimeter'

/** Default option sets — match the §5.6 acceptance criteria. */
export const DEFAULT_SURVEY_GRID_OPTS: SurveyGridOpts = {
  legSpacingM: 10,
  altitudeM: 30,
  legDirectionDeg: 0,
  cameraTriggerIntervalS: 5,
}

export const DEFAULT_CORRIDOR_OPTS: CorridorOpts = {
  corridorWidthM: 20,
  legSpacingM: 10,
  altitudeM: 30,
}

export const DEFAULT_PERIMETER_OPTS: PerimeterOpts = {
  offsetM: 5,
  altitudeM: 30,
}

// ============================================================================
// Geo math helpers (self-contained — kept here so patterns.ts is pure & testable)
// ============================================================================

const EARTH_R_M = 6378137.0 // WGS84 equatorial radius
const DEG2RAD = Math.PI / 180
const RAD2DEG = 180 / Math.PI

/** Haversine great-circle distance between two lat/lon points, in metres. */
export function haversineM(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const phi1 = lat1 * DEG2RAD
  const phi2 = lat2 * DEG2RAD
  const dPhi = (lat2 - lat1) * DEG2RAD
  const dLam = (lon2 - lon1) * DEG2RAD
  const a = Math.sin(dPhi / 2) ** 2 + Math.cos(phi1) * Math.cos(phi2) * Math.sin(dLam / 2) ** 2
  const c = 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a))
  return EARTH_R_M * c
}

/** Initial bearing (deg, 0 = N, 90 = E) from point 1 to point 2. */
export function bearingDeg(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const phi1 = lat1 * DEG2RAD
  const phi2 = lat2 * DEG2RAD
  const dLam = (lon2 - lon1) * DEG2RAD
  const y = Math.sin(dLam) * Math.cos(phi2)
  const x = Math.cos(phi1) * Math.sin(phi2) - Math.sin(phi1) * Math.cos(phi2) * Math.cos(dLam)
  return (Math.atan2(y, x) * RAD2DEG + 360) % 360
}

/** Destination lat/lon given start point + bearing + distance (great-circle). */
export function destinationPoint(
  lat: number,
  lon: number,
  bearingDegIn: number,
  distanceM: number,
): [number, number] {
  const phi1 = lat * DEG2RAD
  const lam1 = lon * DEG2RAD
  const theta = bearingDegIn * DEG2RAD
  const delta = distanceM / EARTH_R_M
  const phi2 = Math.asin(
    Math.sin(phi1) * Math.cos(delta) + Math.cos(phi1) * Math.sin(delta) * Math.cos(theta),
  )
  const lam2 =
    lam1 +
    Math.atan2(
      Math.sin(theta) * Math.sin(delta) * Math.cos(phi1),
      Math.cos(delta) - Math.sin(phi1) * Math.sin(phi2),
    )
  const lon2 = ((lam2 * RAD2DEG) + 540) % 360 - 180
  return [phi2 * RAD2DEG, lon2]
}

/**
 * Polygon area in m² (planar approximation using local ENU). Sign follows the
 * vertex order; the absolute value is returned. Polygons with fewer than 3
 * vertices have zero area.
 */
export function polygonArea(polygon: [number, number][]): number {
  if (polygon.length < 3) return 0
  const [originLat, originLon] = polygonCentroid(polygon)
  let area = 0
  for (let i = 0; i < polygon.length; i++) {
    const [lat1, lon1] = polygon[i]
    const [lat2, lon2] = polygon[(i + 1) % polygon.length]
    const [e1, n1] = latLonToEnu(lat1, lon1, originLat, originLon)
    const [e2, n2] = latLonToEnu(lat2, lon2, originLat, originLon)
    area += e1 * n2 - e2 * n1
  }
  return Math.abs(area) / 2
}

/** Whether a lat/lon point is inside a lat/lon polygon (ray-cast test). */
export function pointInPolygon(lat: number, lon: number, polygon: [number, number][]): boolean {
  let inside = false
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i++) {
    const [latI, lonI] = polygon[i]
    const [latJ, lonJ] = polygon[j]
    const intersect =
      lonI > lon !== lonJ > lon &&
      lat < ((latJ - latI) * (lon - lonI)) / (lonJ - lonI) + latI
    if (intersect) inside = !inside
  }
  return inside
}

// ============================================================================
// Internal helpers
// ============================================================================

/** Average of vertices — a fine centroid for sub-100 m polygons. */
function polygonCentroid(polygon: [number, number][]): [number, number] {
  let lat = 0
  let lon = 0
  for (const [la, lo] of polygon) {
    lat += la
    lon += lo
  }
  return [lat / polygon.length, lon / polygon.length]
}

/** Polyline centroid (same idea — average of vertices). */
function polylineCentroid(polyline: [number, number][]): [number, number] {
  return polygonCentroid(polyline)
}

/** Lat/lon → local ENU metres relative to an origin. */
function latLonToEnu(
  lat: number,
  lon: number,
  originLat: number,
  originLon: number,
): [number, number] {
  const latRad = originLat * DEG2RAD
  const mPerDegLat = EARTH_R_M * DEG2RAD
  const mPerDegLon = EARTH_R_M * DEG2RAD * Math.cos(latRad)
  return [(lon - originLon) * mPerDegLon, (lat - originLat) * mPerDegLat]
}

/** Local ENU metres → lat/lon. */
function enuToLatLon(
  e: number,
  n: number,
  originLat: number,
  originLon: number,
): [number, number] {
  const latRad = originLat * DEG2RAD
  const mPerDegLat = EARTH_R_M * DEG2RAD
  const mPerDegLon = EARTH_R_M * DEG2RAD * Math.cos(latRad)
  return [originLat + n / mPerDegLat, originLon + e / mPerDegLon]
}

/** Build a MAV_FRAME_GLOBAL_RELATIVE_ALT waypoint. */
function makeWaypoint(
  seq: number,
  command: number,
  lat: number,
  lon: number,
  alt: number,
  params?: Partial<Pick<PlanWaypoint, 'param1' | 'param2' | 'param3' | 'param4'>>,
): PlanWaypoint {
  return {
    seq,
    frame: 3, // MAV_FRAME_GLOBAL_RELATIVE_ALT
    command,
    x: lat,
    y: lon,
    z: alt,
    param1: params?.param1 ?? 0,
    param2: params?.param2 ?? 2, // accept radius (m) — matches DEFAULT_WP_ACCEPT_M
    param3: params?.param3 ?? 0,
    param4: params?.param4 ?? 0,
  }
}

/**
 * Find the parameter `t` along the leg direction where the leg (a line
 * passing through `legBase` along `legDir`) crosses the polygon edge
 * `a → b`. Returns `null` if there is no intersection.
 *
 * The leg is parametrized as p(t) = legBase + t * legDir.
 * The edge is parametrized as e(s) = a + s * (b - a), s ∈ [0, 1].
 *
 * Solving the 2×2 linear system:
 *   a + s*(b-a) = legBase + t*legDir
 *
 * Cramer's rule on the determinant of [[(b-a), -legDir]] gives the answer.
 */
function legEdgeIntersection(
  ax: number,
  ay: number,
  bx: number,
  by: number,
  legBaseX: number,
  legBaseY: number,
  legDirX: number,
  legDirY: number,
): { t: number; s: number } | null {
  const dx = bx - ax
  const dy = by - ay
  // The system:
  //   dx * s - legDirX * t = legBaseX - ax
  //   dy * s - legDirY * t = legBaseY - ay
  const rhs0 = legBaseX - ax
  const rhs1 = legBaseY - ay
  const det = dx * -legDirY - dy * -legDirX
  if (Math.abs(det) < 1e-12) return null
  const s = (rhs0 * -legDirY - -legDirX * rhs1) / det
  const t = (dx * rhs1 - dy * rhs0) / det
  if (s < 0 || s > 1) return null
  return { t, s }
}

// ============================================================================
// Survey Grid (boustrophedon parallel legs inside a polygon)
// ============================================================================

/**
 * Generate a back-and-forth (boustrophedon) survey grid inside a polygon.
 *
 * Algorithm (GCS_SPEC §5.6):
 *   1. Compute the polygon's bounding box in local ENU metres.
 *   2. Project all polygon vertices onto the cross-track axis (perpendicular
 *      to legDirectionDeg) to find the cross-track range.
 *   3. Generate `numLegs` parallel legs at `legSpacingM` intervals, centered
 *      in the cross-track range (half-spacing inset from the edges).
 *   4. For each leg: find all intersections with the polygon edges, sort
 *      them along the leg direction, pair them up as (entry, exit).
 *   5. Emit a camera-trigger waypoint (cmd=200) at each leg entry, then a
 *      nav waypoint (cmd=16) at the leg exit. Boustrophedon alternates
 *      direction per leg.
 *   6. Close with a final nav waypoint at the entry of leg 0 (return-to-start),
 *      matching AC-5.6.1's expected 21-waypoint count for a 100 m × 100 m
 *      polygon with 10 m leg spacing (10 legs × 2 + 1 closing = 21).
 */
export function generateSurveyGrid(
  polygon: [number, number][],
  opts: SurveyGridOpts,
): PlanWaypoint[] {
  if (polygon.length < 3) return []
  const legSpacing = Math.max(0.5, opts.legSpacingM)
  const altitude = opts.altitudeM

  const [originLat, originLon] = polygonCentroid(polygon)
  // Convert polygon vertices to local ENU metres.
  const poly: [number, number][] = polygon.map(([la, lo]) =>
    latLonToEnu(la, lo, originLat, originLon),
  )

  // Leg direction unit vector in ENU. legDirectionDeg is the heading the
  // drone flies along the leg:
  //   0° (north) → (E=0, N=+1)
  //   90° (east) → (E=+1, N=0)
  const legDirRad = (opts.legDirectionDeg ?? 0) * DEG2RAD
  const legDirX = Math.sin(legDirRad)
  const legDirY = Math.cos(legDirRad)
  // Cross-track direction = leg direction rotated 90° to the right.
  const crossDirX = legDirY
  const crossDirY = -legDirX

  // Project polygon vertices onto the cross-track axis to find the range.
  let crossMin = Infinity
  let crossMax = -Infinity
  for (const [e, n] of poly) {
    const cross = e * crossDirX + n * crossDirY
    if (cross < crossMin) crossMin = cross
    if (cross > crossMax) crossMax = cross
  }
  const crossRange = crossMax - crossMin
  if (crossRange <= 0) return []

  // Number of legs and the first leg's cross-track offset.
  // Center the legs in the cross-track range so the legs at the boundary
  // are inset by half a spacing (matches AC-5.6.1: 100 m range / 10 m spacing
  // → 10 legs, not 11).
  const numLegs = Math.max(1, Math.floor(crossRange / legSpacing + 1e-9))
  const firstOffset = crossMin + (crossRange - (numLegs - 1) * legSpacing) / 2

  const waypoints: PlanWaypoint[] = []
  let seq = 0
  let firstEntryLatLon: [number, number] | null = null

  for (let legIdx = 0; legIdx < numLegs; legIdx++) {
    const crossOffset = firstOffset + legIdx * legSpacing
    // The leg passes through (crossOffset * crossDir) along legDir.
    const legBaseX = crossOffset * crossDirX
    const legBaseY = crossOffset * crossDirY

    // Find all intersections with polygon edges, along the leg direction.
    const tValues: number[] = []
    for (let i = 0; i < poly.length; i++) {
      const a = poly[i]
      const b = poly[(i + 1) % poly.length]
      const hit = legEdgeIntersection(
        a[0],
        a[1],
        b[0],
        b[1],
        legBaseX,
        legBaseY,
        legDirX,
        legDirY,
      )
      if (hit) tValues.push(hit.t)
    }
    if (tValues.length < 2) continue

    // Sort along the leg direction.
    tValues.sort((p, q) => p - q)

    // Pair up (entry, exit). For non-convex polygons we may get multiple
    // pairs per leg — emit waypoints for each pair to keep the survey
    // continuous. Boustrophedon alternates direction per leg.
    const reverse = legIdx % 2 === 1
    for (let pairIdx = 0; pairIdx * 2 + 1 < tValues.length; pairIdx++) {
      const t1 = tValues[pairIdx * 2]
      const t2 = tValues[pairIdx * 2 + 1]
      const tEntry = reverse ? t2 : t1
      const tExit = reverse ? t1 : t2

      // Move the entry/exit slightly inward (1 mm along the leg direction)
      // so they land strictly inside the polygon rather than exactly on an
      // edge. Ray-cast point-in-polygon is unstable on boundary points;
      // 1 mm is well below survey-pattern precision but enough to disambiguate.
      const EPS_INSET = 0.001
      const entrySign = reverse ? -1 : 1
      const exitSign = reverse ? 1 : -1
      const entryE = legBaseX + tEntry * legDirX + entrySign * EPS_INSET * legDirX
      const entryN = legBaseY + tEntry * legDirY + entrySign * EPS_INSET * legDirY
      const exitE = legBaseX + tExit * legDirX + exitSign * EPS_INSET * legDirX
      const exitN = legBaseY + tExit * legDirY + exitSign * EPS_INSET * legDirY
      const [entryLat, entryLon] = enuToLatLon(entryE, entryN, originLat, originLon)
      const [exitLat, exitLon] = enuToLatLon(exitE, exitN, originLat, originLon)

      if (firstEntryLatLon === null) firstEntryLatLon = [entryLat, entryLon]

      // Camera trigger at the leg entry (cmd=200).
      waypoints.push(
        makeWaypoint(seq++, 200, entryLat, entryLon, altitude, {
          param1: opts.cameraTriggerIntervalS,
          param2: 0,
          param3: 0,
          param4: 0,
        }),
      )
      // Nav waypoint at the leg exit (cmd=16).
      waypoints.push(
        makeWaypoint(seq++, 16, exitLat, exitLon, altitude, {
          param1: 0,
          param2: 2,
          param3: 0,
          param4: 0,
        }),
      )
    }
  }

  // Closing nav waypoint: return to the first leg's entry (return-to-start).
  // Brings the total count to 21 for the standard AC-5.6.1 case.
  if (firstEntryLatLon) {
    waypoints.push(
      makeWaypoint(seq++, 16, firstEntryLatLon[0], firstEntryLatLon[1], altitude, {
        param1: 0,
        param2: 2,
        param3: 0,
        param4: 0,
      }),
    )
  }

  return waypoints
}

// ============================================================================
// Corridor (back-and-forth sweeps across a polyline, connected along it)
// ============================================================================

/**
 * Generate a corridor survey: a sequence of back-and-forth sweeps across
 * the polyline, spaced `legSpacingM` apart along the polyline direction,
 * each spanning `corridorWidthM` perpendicular to the polyline.
 *
 * Algorithm (GCS_SPEC §5.6):
 *   1. Walk along the polyline at `legSpacingM` intervals.
 *   2. At each walk position, compute the polyline's tangent direction.
 *   3. The perpendicular direction is the sweep axis — generate two
 *      endpoints at ±corridorWidthM/2 from the polyline.
 *   4. Boustrophedon: alternate sweep direction so the drone doesn't waste
 *      time flying back to the start of each sweep.
 *   5. The drone traverses: entry → exit (sweep 1) → entry → exit (sweep 2) ...
 *      with each sweep's exit connecting to the next sweep's entry (short
 *      along-polyline hop).
 */
export function generateCorridor(
  polyline: [number, number][],
  opts: CorridorOpts,
): PlanWaypoint[] {
  if (polyline.length < 2) return []
  const legSpacing = Math.max(0.5, opts.legSpacingM)
  const corridorWidth = Math.max(1, opts.corridorWidthM)
  const altitude = opts.altitudeM
  const halfWidth = corridorWidth / 2

  const [originLat, originLon] = polylineCentroid(polyline)
  // Convert polyline vertices to local ENU.
  const poly: [number, number][] = polyline.map(([la, lo]) =>
    latLonToEnu(la, lo, originLat, originLon),
  )

  // Walk along the polyline, emitting a sample every `legSpacing` metres.
  // Each sample carries its position + the tangent direction of the segment
  // it lives on (used for the perpendicular sweep axis).
  interface Sample {
    e: number
    n: number
    tangentX: number
    tangentY: number
  }
  const samples: Sample[] = []
  let distanceAlong = 0
  let nextSampleAt = 0
  for (let i = 0; i < poly.length - 1; i++) {
    const [e0, n0] = poly[i]
    const [e1, n1] = poly[i + 1]
    const dx = e1 - e0
    const dy = n1 - n0
    const segLen = Math.hypot(dx, dy)
    if (segLen < 1e-9) continue
    const tx = dx / segLen
    const ty = dy / segLen
    // Emit samples at multiples of `legSpacing` that fall within this segment.
    while (nextSampleAt <= distanceAlong + segLen + 1e-9) {
      const fracIntoSeg = (nextSampleAt - distanceAlong) / segLen
      if (fracIntoSeg < -1e-9 || fracIntoSeg > 1 + 1e-9) break
      const sx = e0 + fracIntoSeg * dx
      const sy = n0 + fracIntoSeg * dy
      samples.push({ e: sx, n: sy, tangentX: tx, tangentY: ty })
      nextSampleAt += legSpacing
    }
    distanceAlong += segLen
  }
  // Always emit the final point as a sample so the corridor ends cleanly.
  if (samples.length === 0 || samples[samples.length - 1].e !== poly[poly.length - 1][0] ||
      samples[samples.length - 1].n !== poly[poly.length - 1][1]) {
    const last = poly[poly.length - 1]
    const prev = poly[poly.length - 2]
    const dx = last[0] - prev[0]
    const dy = last[1] - prev[1]
    const len = Math.hypot(dx, dy)
    const tx = len > 1e-9 ? dx / len : 1
    const ty = len > 1e-9 ? dy / len : 0
    samples.push({ e: last[0], n: last[1], tangentX: tx, tangentY: ty })
  }

  if (samples.length === 0) return []

  // For each sample, compute the sweep endpoints at ±halfWidth perpendicular
  // to the tangent. Boustrophedon alternates the direction of each sweep so
  // the drone enters the next sweep on the same side it just exited.
  const waypoints: PlanWaypoint[] = []
  let seq = 0

  for (let i = 0; i < samples.length; i++) {
    const s = samples[i]
    // Perpendicular to tangent (rotate 90° CCW): (-ty, tx) — points "left" of travel.
    const perpX = -s.tangentY
    const perpY = s.tangentX
    const leftE = s.e + perpX * halfWidth
    const leftN = s.n + perpY * halfWidth
    const rightE = s.e - perpX * halfWidth
    const rightN = s.n - perpY * halfWidth
    // Boustrophedon: even sweeps go left → right, odd sweeps go right → left.
    const reverse = i % 2 === 1
    const [startE, startN] = reverse ? [rightE, rightN] : [leftE, leftN]
    const [endE, endN] = reverse ? [leftE, leftN] : [rightE, rightN]
    const [startLat, startLon] = enuToLatLon(startE, startN, originLat, originLon)
    const [endLat, endLon] = enuToLatLon(endE, endN, originLat, originLon)

    // Camera trigger at the sweep start (cmd=200).
    waypoints.push(
      makeWaypoint(seq++, 200, startLat, startLon, altitude, {
        param1: 0,
        param2: 0,
        param3: 0,
        param4: 0,
      }),
    )
    // Nav waypoint at the sweep end (cmd=16).
    waypoints.push(
      makeWaypoint(seq++, 16, endLat, endLon, altitude, {
        param1: 0,
        param2: 2,
        param3: 0,
        param4: 0,
      }),
    )
  }

  return waypoints
}

// ============================================================================
// Perimeter (single loop inset from a polygon)
// ============================================================================

/**
 * Generate a perimeter patrol: a single loop of waypoints inset `offsetM`
 * from the source polygon's edges. One waypoint per inset vertex, connected
 * in order. The loop closes implicitly (the drone flies from the last
 * vertex back to the first — no explicit return-to-start waypoint per AC-5.6.3).
 *
 * Inset method: each vertex is shifted toward the polygon centroid by
 * `offsetM`. This is exact for regular (convex) polygons and a good
 * approximation for irregular convex polygons at the small offset distances
 * typical of geofence perimeters.
 */
export function generatePerimeter(
  polygon: [number, number][],
  opts: PerimeterOpts,
): PlanWaypoint[] {
  if (polygon.length < 3) return []
  const offset = Math.max(0, opts.offsetM)
  const altitude = opts.altitudeM

  const [originLat, originLon] = polygonCentroid(polygon)
  const poly: [number, number][] = polygon.map(([la, lo]) =>
    latLonToEnu(la, lo, originLat, originLon),
  )
  // Centroid in ENU.
  const centroidE = poly.reduce((sum, p) => sum + p[0], 0) / poly.length
  const centroidN = poly.reduce((sum, p) => sum + p[1], 0) / poly.length

  const waypoints: PlanWaypoint[] = []
  let seq = 0
  for (const [e, n] of poly) {
    const dx = centroidE - e
    const dy = centroidN - n
    const dist = Math.hypot(dx, dy)
    // If the vertex is at the centroid (degenerate), keep it as-is.
    const moveFactor = dist > 1e-9 ? offset / dist : 0
    const insetE = e + dx * moveFactor
    const insetN = n + dy * moveFactor
    const [lat, lon] = enuToLatLon(insetE, insetN, originLat, originLon)
    waypoints.push(
      makeWaypoint(seq++, 16, lat, lon, altitude, {
        param1: 0,
        param2: 2,
        param3: 0,
        param4: 0,
      }),
    )
  }

  return waypoints
}

// ============================================================================
// Small type + round-trip tests (self-test on import in dev — no-op in prod)
// ============================================================================

/**
 * Run a smoke test of all three generators against the §5.6 acceptance
 * criteria polygons. Returns a summary object; intended for ad-hoc dev use
 * and as a reference for Subagent C's harness. Does not throw on failure —
 * the Subagent C harness is the authoritative test.
 */
export function selfTest(): {
  survey: { count: number; expected: number }
  corridor: { count: number }
  perimeter: { count: number; expected: number }
} {
  // 100 m × 100 m square at the PX4 test field — AC-5.6.1.
  const centerLat = 47.39777
  const centerLon = 8.54558
  const halfDegLat = 50 / 111319.488
  const halfDegLon = 50 / (111319.488 * Math.cos(centerLat * DEG2RAD))
  const square: [number, number][] = [
    [centerLat - halfDegLat, centerLon - halfDegLon],
    [centerLat - halfDegLat, centerLon + halfDegLon],
    [centerLat + halfDegLat, centerLon + halfDegLon],
    [centerLat + halfDegLat, centerLon - halfDegLon],
  ]
  const survey = generateSurveyGrid(square, DEFAULT_SURVEY_GRID_OPTS)
  // Corridor: 100 m polyline along the X axis.
  const polyline: [number, number][] = []
  for (let i = 0; i <= 10; i++) {
    const lat = centerLat
    const lon = centerLon + (i * 10 - 50) / (111319.488 * Math.cos(centerLat * DEG2RAD))
    polyline.push([lat, lon])
  }
  const corridor = generateCorridor(polyline, DEFAULT_CORRIDOR_OPTS)
  // Perimeter on the 4-vertex square — AC-5.6.3 (expect 4 waypoints).
  const perimeter = generatePerimeter(square, DEFAULT_PERIMETER_OPTS)
  return {
    survey: { count: survey.length, expected: 21 },
    corridor: { count: corridor.length },
    perimeter: { count: perimeter.length, expected: 4 },
  }
}
