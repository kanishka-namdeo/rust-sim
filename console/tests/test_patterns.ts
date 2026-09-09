/**
 * G-13 harness test runner (M7) — verifies the survey / corridor / perimeter
 * pattern generators in `console/src/lib/patterns.ts` produce correct
 * waypoints per GCS_SPEC.md §5.6 AC-5.6.1..AC-5.6.4.
 *
 * Run via `console/tests/run_g13_patterns.sh` which compiles this file with
 * `tsc` and executes the resulting JS. Each test prints `✓` on pass or `✗
 * <reason>` on fail; exit code is 0 on all-pass, 1 on any failure.
 *
 * Imports use the `.js` extension so the source compiles cleanly under
 * `--module nodenext --moduleResolution nodenext` and the emitted JS resolves
 * the same path under Node ESM at runtime.
 */

import {
  generateSurveyGrid,
  generateCorridor,
  generatePerimeter,
  pointInPolygon,
  haversineM,
  DEFAULT_SURVEY_GRID_OPTS,
  DEFAULT_CORRIDOR_OPTS,
  DEFAULT_PERIMETER_OPTS,
} from "../src/lib/patterns.js";
import type { PlanWaypoint } from "../src/lib/plan-types.js";

// Minimal Node globals — avoids a hard dependency on @types/node so the test
// compiles cleanly even when tsc is invoked with explicit file args (which
// bypasses the console/tsconfig.json `types` resolution).
declare const console: { log(...args: unknown[]): void };
declare const process: { exit(code: number): never };

// ---------------------------------------------------------------------------
// Constants — match plan-types.ts (PX4_TEST_FIELD, DEFAULT_WP_ALT_M=12, etc.)
// and patterns.ts (DEFAULT_SURVEY_GRID_OPTS.altitudeM=30).
// ---------------------------------------------------------------------------

const PX4_TEST_FIELD = { lat: 47.39777, lon: 8.54558 };

/** MAV_FRAME_GLOBAL_RELATIVE_ALT (3) — the QGC Plan View default. */
const FRAME_GLOBAL_RELATIVE_ALT = 3;
/** MAV_CMD_NAV_WAYPOINT (16) — a fly-to waypoint. */
const MAV_CMD_NAV_WAYPOINT = 16;
/** Camera-trigger command (200) — see patterns.ts header. */
const MAV_CMD_CAMERA_TRIGGER = 200;

/** Default waypoint accept radius (m) — plan-types.ts DEFAULT_WP_ACCEPT_M. */
const DEFAULT_ACCEPT_RADIUS_M = 2;

// ---------------------------------------------------------------------------
// Test geometry — all patterns evaluated around the PX4 test field
// (47.39777, 8.54558) per GCS_SPEC §5.6 and ADR-0017.
// ---------------------------------------------------------------------------

const M_PER_DEG_LAT = 111319.488;
function mPerDegLon(latDeg: number): number {
  return M_PER_DEG_LAT * Math.cos((latDeg * Math.PI) / 180);
}

/** Build a 100 m × 100 m axis-aligned square polygon at the PX4 test field. */
function square100m(): [number, number][] {
  const lat = PX4_TEST_FIELD.lat;
  const lon = PX4_TEST_FIELD.lon;
  const dLat = 50 / M_PER_DEG_LAT; // 50 m north/south of centre
  const dLon = 50 / mPerDegLon(lat); // 50 m east/west of centre
  // CCW around centroid: SW → SE → NE → NW.
  return [
    [lat - dLat, lon - dLon],
    [lat - dLat, lon + dLon],
    [lat + dLat, lon + dLon],
    [lat + dLat, lon - dLon],
  ];
}

/** Build a 100 m east-going polyline at the PX4 test field (2 points). */
function polyline100m(): [number, number][] {
  const lat = PX4_TEST_FIELD.lat;
  const lon = PX4_TEST_FIELD.lon;
  const dLon = 100 / mPerDegLon(lat); // 100 m east
  return [
    [lat, lon],
    [lat, lon + dLon],
  ];
}

// ---------------------------------------------------------------------------
// Tiny test framework — no dependencies, prints ✓/✗ and exits 0/1.
// ---------------------------------------------------------------------------

let failures = 0;
let checks = 0;

function check(cond: boolean, what: string, detail?: string): void {
  checks++;
  if (cond) {
    console.log(`  ✓ ${what}`);
  } else {
    failures++;
    console.log(`  ✗ ${what}${detail ? ` — ${detail}` : ""}`);
  }
}

function approxEq(a: number, b: number, eps = 1e-9): boolean {
  return Math.abs(a - b) <= eps;
}

/**
 * "Inside-or-on-boundary" point-in-polygon test — wrapper around the
 * patterns.ts ray-cast `pointInPolygon` (which is strict-less-than and
 * therefore excludes points exactly on an edge) that additionally accepts
 * points within `tolM` metres of any polygon edge.
 *
 * Survey / corridor generators legitimately place waypoints exactly on the
 * polygon boundary (where each leg enters/exits the polygon), so the AC-5.6.1
 * "all inside polygon" assertion needs to tolerate boundary contact. The
 * default 0.05 m tolerance is well under the 2 m accept-radius the mission
 * editor uses, so it does not weaken the assertion in any practical sense.
 */
function pointInPolygonTolerant(
  lat: number,
  lon: number,
  polygon: [number, number][],
  tolM = 0.05,
): boolean {
  if (pointInPolygon(lat, lon, polygon)) return true;
  // Boundary check: is the point within tolM of any polygon edge?
  const cLat = polygon.reduce((s, p) => s + p[0], 0) / polygon.length;
  const cLon = polygon.reduce((s, p) => s + p[1], 0) / polygon.length;
  const mPerLon = mPerDegLon(cLat);
  const pE = (lon - cLon) * mPerLon;
  const pN = (lat - cLat) * M_PER_DEG_LAT;
  for (let i = 0; i < polygon.length; i++) {
    const aLat = polygon[i][0];
    const aLon = polygon[i][1];
    const bLat = polygon[(i + 1) % polygon.length][0];
    const bLon = polygon[(i + 1) % polygon.length][1];
    const aE = (aLon - cLon) * mPerLon;
    const aN = (aLat - cLat) * M_PER_DEG_LAT;
    const bE = (bLon - cLon) * mPerLon;
    const bN = (bLat - cLat) * M_PER_DEG_LAT;
    if (pointToSegmentM(pE, pN, aE, aN, bE, bN) <= tolM) return true;
  }
  return false;
}

/**
 * Perpendicular distance (m) from a [lat, lon] point to a polyline, using the
 * local equirectangular projection around the polyline's centroid. Used by
 * the corridor test (AC-5.6.2: all WPs within corridorWidthM/2 of polyline).
 */
function perpDistanceToPolylineM(pt: [number, number], polyline: [number, number][]): number {
  if (polyline.length < 2) return Infinity;
  // Centroid of the polyline (average of vertices).
  const cLat = polyline.reduce((s, p) => s + p[0], 0) / polyline.length;
  const cLon = polyline.reduce((s, p) => s + p[1], 0) / polyline.length;
  const mPerLon = mPerDegLon(cLat);
  const pE = (pt[1] - cLon) * mPerLon;
  const pN = (pt[0] - cLat) * M_PER_DEG_LAT;
  let best = Infinity;
  for (let i = 0; i + 1 < polyline.length; i++) {
    const aE = (polyline[i][1] - cLon) * mPerLon;
    const aN = (polyline[i][0] - cLat) * M_PER_DEG_LAT;
    const bE = (polyline[i + 1][1] - cLon) * mPerLon;
    const bN = (polyline[i + 1][0] - cLat) * M_PER_DEG_LAT;
    best = Math.min(best, pointToSegmentM(pE, pN, aE, aN, bE, bN));
  }
  return best;
}

function pointToSegmentM(
  px: number, py: number,
  ax: number, ay: number,
  bx: number, by: number,
): number {
  const abx = bx - ax;
  const aby = by - ay;
  const apx = px - ax;
  const apy = py - ay;
  const ab2 = abx * abx + aby * aby;
  let t = ab2 > 0 ? (apx * abx + apy * aby) / ab2 : 0;
  t = Math.max(0, Math.min(1, t));
  const cx = ax + t * abx;
  const cy = ay + t * aby;
  return Math.hypot(px - cx, py - cy);
}

/**
 * Project a waypoint's position onto the polyline and return its arc-length
 * parameter `s` (m from the polyline start). Used by the corridor test
 * (AC-5.6.2: no gaps wider than legSpacingM along the corridor direction).
 *
 * For a polyline along the east axis (as in our test), `s` is just the
 * east-offset from the start, in metres.
 */
function arcLengthAlongPolylineM(pt: [number, number], polyline: [number, number][]): number {
  if (polyline.length < 2) return 0;
  // For our test polyline (single east-going segment), s = (lon - lon0) * mPerLon.
  // For a general polyline, project onto the closest segment. Here we keep
  // it simple and assume the test polyline runs east — which it does.
  const cLat = polyline[0][0];
  const mPerLon = mPerDegLon(cLat);
  return (pt[1] - polyline[0][1]) * mPerLon;
}

// ---------------------------------------------------------------------------
// Test 1: Survey Grid (AC-5.6.1)
//
//   100 m × 100 m polygon, legSpacingM=10, altitudeM=30 → ≥10 legs, all WPs
//   inside the polygon, all altitudes = 30, ≥1 camera-trigger WP (command 200).
// ---------------------------------------------------------------------------

function test1_surveyGrid(): void {
  console.log("Test 1: survey grid (AC-5.6.1) — 100×100 m, 10 m legs, alt=30");
  const poly = square100m();
  const wps = generateSurveyGrid(poly, DEFAULT_SURVEY_GRID_OPTS);

  check(wps.length > 0, "grid produces waypoints", `got ${wps.length}`);

  // ≥10 legs — a leg is the segment between a camera-trigger (cmd=200) and
  // the following nav waypoint (cmd=16). We count legs by counting camera
  // triggers (one per leg).
  const camTriggers = wps.filter((w) => w.command === MAV_CMD_CAMERA_TRIGGER);
  check(
    camTriggers.length >= 10,
    "≥10 legs generated",
    `got ${camTriggers.length} camera-trigger WPs (= leg count)`,
  );

  // All waypoints inside the polygon (with 0.05 m tolerance for points on
  // the boundary — survey legs legitimately start/end on the polygon edge).
  let allInside = true;
  let firstOutside: PlanWaypoint | null = null;
  for (const w of wps) {
    if (!pointInPolygonTolerant(w.x, w.y, poly)) {
      allInside = false;
      firstOutside = w;
      break;
    }
  }
  check(
    allInside,
    "all waypoints inside polygon (0.05 m boundary tolerance)",
    firstOutside
      ? `WP seq=${firstOutside.seq} at (${firstOutside.x.toFixed(6)}, ${firstOutside.y.toFixed(6)}) is outside`
      : undefined,
  );

  // All altitudes = 30 m (DEFAULT_SURVEY_GRID_OPTS.altitudeM).
  const badAlt = wps.find((w) => !approxEq(w.z, 30));
  check(
    !badAlt,
    "all waypoints have altitudeM=30",
    badAlt ? `WP seq=${badAlt.seq} has z=${badAlt.z}` : undefined,
  );

  // ≥1 camera-trigger waypoint (command=200) — AC-5.6.1 explicitly requires
  // "camera-trigger waypoints at every leg".
  check(
    camTriggers.length >= 1,
    "≥1 camera-trigger waypoint (command=200)",
    `got ${camTriggers.length}`,
  );
}

// ---------------------------------------------------------------------------
// Test 2: Corridor Pattern (AC-5.6.2)
//
//   100 m polyline, corridorWidthM=20, legSpacingM=10 → no gaps > 10 m along
//   the corridor, all WPs within corridorWidthM/2 = 10 m of the polyline.
// ---------------------------------------------------------------------------

function test2_corridor(): void {
  console.log("Test 2: corridor (AC-5.6.2) — 100 m polyline, w=20, legs=10");
  const line = polyline100m();
  const wps = generateCorridor(line, DEFAULT_CORRIDOR_OPTS);

  check(wps.length > 0, "corridor produces waypoints", `got ${wps.length}`);

  // All waypoints within corridorWidthM/2 = 10 m of the polyline.
  const halfW = DEFAULT_CORRIDOR_OPTS.corridorWidthM / 2;
  let maxDist = 0;
  let maxWp: PlanWaypoint | null = null;
  for (const w of wps) {
    const d = perpDistanceToPolylineM([w.x, w.y], line);
    if (d > maxDist) {
      maxDist = d;
      maxWp = w;
    }
  }
  check(
    maxDist <= halfW + 1e-6,
    `all WPs within corridorWidthM/2=${halfW} m of polyline`,
    maxWp ? `WP seq=${maxWp.seq} is ${maxDist.toFixed(3)} m away` : undefined,
  );

  // No gaps wider than legSpacingM = 10 m along the corridor direction.
  // Project each WP onto the polyline (arc-length `s` from start) and check
  // the maximum gap between consecutive `s` values (sorted ascending).
  const sValues = wps
    .map((w) => arcLengthAlongPolylineM([w.x, w.y], line))
    .sort((a, b) => a - b);
  let maxGap = 0;
  let gapDetail = "";
  for (let i = 1; i < sValues.length; i++) {
    const gap = sValues[i] - sValues[i - 1];
    if (gap > maxGap) {
      maxGap = gap;
      gapDetail = `between s=${sValues[i - 1].toFixed(2)} m and s=${sValues[i].toFixed(2)} m`;
    }
  }
  check(
    maxGap <= DEFAULT_CORRIDOR_OPTS.legSpacingM + 1e-6,
    `no gaps wider than legSpacingM=${DEFAULT_CORRIDOR_OPTS.legSpacingM} m along corridor`,
    `${gapDetail}: ${maxGap.toFixed(3)} m`,
  );

  // Sanity: the corridor spans the full 100 m polyline (start and end are
  // both represented).
  const spanM = sValues[sValues.length - 1] - sValues[0];
  check(
    spanM >= 100 - 1e-3,
    "corridor spans the full polyline length (100 m)",
    `span=${spanM.toFixed(3)} m`,
  );
}

// ---------------------------------------------------------------------------
// Test 3: Perimeter Pattern (AC-5.6.3)
//
//   4-vertex polygon, offsetM=5 → exactly 4 waypoints (one per offset vertex),
//   forming a loop (last connects back to first), all inside the original.
// ---------------------------------------------------------------------------

function test3_perimeter(): void {
  console.log("Test 3: perimeter (AC-5.6.3) — 4-vertex polygon, offset=5");
  const poly = square100m();
  const wps = generatePerimeter(poly, DEFAULT_PERIMETER_OPTS);

  check(
    wps.length === 4,
    "exactly 4 waypoints (one per offset vertex)",
    `got ${wps.length}`,
  );
  if (wps.length !== 4) return; // can't check loop/inside meaningfully

  // All waypoints inside the original polygon (offset is inward).
  let allInside = true;
  let firstOutside: PlanWaypoint | null = null;
  for (const w of wps) {
    if (!pointInPolygon(w.x, w.y, poly)) {
      allInside = false;
      firstOutside = w;
      break;
    }
  }
  check(
    allInside,
    "all waypoints inside the original polygon",
    firstOutside
      ? `WP seq=${firstOutside.seq} at (${firstOutside.x.toFixed(6)}, ${firstOutside.y.toFixed(6)}) is outside`
      : undefined,
  );

  // Waypoints form a loop: the path P[0] → P[1] → P[2] → P[3] → P[0] closes.
  // Check the loop is non-degenerate (signed area > 0) and simple (no
  // self-intersections among consecutive edges). For a 4-vertex convex
  // polygon, "no self-intersections" reduces to "every edge has positive
  // turn" — we approximate by checking the signed area.
  const innerLat = wps.map((w) => w.x);
  const innerLon = wps.map((w) => w.y);
  let area2 = 0;
  for (let i = 0; i < 4; i++) {
    const j = (i + 1) % 4;
    area2 += innerLat[i] * innerLon[j] - innerLat[j] * innerLon[i];
  }
  // Convert signed area (deg²) to m² via the local equirectangular projection.
  const cLat = innerLat.reduce((s, v) => s + v, 0) / 4;
  const areaM2 = Math.abs(area2) * M_PER_DEG_LAT * mPerDegLon(cLat) / 2;
  check(
    areaM2 > 1.0,
    "waypoints form a non-degenerate closed loop",
    `signed area=${areaM2.toFixed(2)} m²`,
  );

  // The perimeter should NOT include a duplicate closing waypoint (i.e.,
  // P[0] ≠ P[N-1] in lat/lon — the loop closes implicitly).
  const first = wps[0];
  const last = wps[wps.length - 1];
  const closingGapM = haversineM(first.x, first.y, last.x, last.y);
  check(
    closingGapM > 1.0,
    "no duplicate closing waypoint (loop closes implicitly)",
    `gap P[last]→P[first] = ${closingGapM.toFixed(3)} m`,
  );

  // Each offset vertex should be ≥ offsetM from the original vertex
  // (perpendicular distance from each edge is offsetM=5; for a 90° corner,
  // the corner-to-corner distance is offsetM*√2 ≈ 7.07 m).
  let minCornerOffset = Infinity;
  for (const w of wps) {
    let best = Infinity;
    for (const v of poly) {
      const d = haversineM(w.x, w.y, v[0], v[1]);
      if (d < best) best = d;
    }
    if (best < minCornerOffset) minCornerOffset = best;
  }
  // For the centroid-shift method, the corner-to-corner distance for a
  // 100 m square with offsetM=5 is exactly 5 m (each vertex moves 5 m
  // toward the centroid along the diagonal). Accept ≥ 4 m to tolerate
  // floating-point and projection rounding.
  check(
    minCornerOffset >= 4.0,
    "vertices offset inward by ≈ offsetM=5 m",
    `min corner offset=${minCornerOffset.toFixed(3)} m`,
  );
}

// ---------------------------------------------------------------------------
// Test 4: Integration with mission editor (AC-5.6.4)
//
//   Generate a survey grid → all WPs have frame=3, command ∈ {16, 200},
//   default altitude (30 m), default accept-radius (2 m on NAV_WAYPOINT).
// ---------------------------------------------------------------------------

function test4_integration(): void {
  console.log("Test 4: mission editor integration (AC-5.6.4) — frame/command/alt/accept-radius");
  const poly = square100m();
  const wps = generateSurveyGrid(poly, DEFAULT_SURVEY_GRID_OPTS);

  check(wps.length > 0, "grid produces waypoints", `got ${wps.length}`);

  // frame=3 (GLOBAL_RELATIVE_ALT) on every WP.
  const badFrame = wps.find((w) => w.frame !== FRAME_GLOBAL_RELATIVE_ALT);
  check(
    !badFrame,
    `all waypoints have frame=${FRAME_GLOBAL_RELATIVE_ALT} (GLOBAL_RELATIVE_ALT)`,
    badFrame ? `WP seq=${badFrame.seq} has frame=${badFrame.frame}` : undefined,
  );

  // command ∈ {16 (NAV_WAYPOINT), 200 (camera trigger)}.
  const badCmd = wps.find(
    (w) => w.command !== MAV_CMD_NAV_WAYPOINT && w.command !== MAV_CMD_CAMERA_TRIGGER,
  );
  check(
    !badCmd,
    "all waypoints have command ∈ {16 (NAV_WAYPOINT), 200 (CAMERA_TRIGGER)}",
    badCmd ? `WP seq=${badCmd.seq} has command=${badCmd.command}` : undefined,
  );

  // Default altitude on every WP (DEFAULT_SURVEY_GRID_OPTS.altitudeM = 30).
  const badAlt = wps.find((w) => !approxEq(w.z, DEFAULT_SURVEY_GRID_OPTS.altitudeM));
  check(
    !badAlt,
    `all waypoints inherit default altitude=${DEFAULT_SURVEY_GRID_OPTS.altitudeM} m`,
    badAlt ? `WP seq=${badAlt.seq} has z=${badAlt.z}` : undefined,
  );

  // Default accept-radius on NAV_WAYPOINT WPs (DEFAULT_ACCEPT_RADIUS_M = 2).
  const navs = wps.filter((w) => w.command === MAV_CMD_NAV_WAYPOINT);
  const badAcc = navs.find((w) => !approxEq(w.param2, DEFAULT_ACCEPT_RADIUS_M));
  check(
    !badAcc,
    `NAV_WAYPOINT WPs have default accept-radius=${DEFAULT_ACCEPT_RADIUS_M} m (param2)`,
    badAcc ? `WP seq=${badAcc.seq} has param2=${badAcc.param2}` : undefined,
  );
}

// ---------------------------------------------------------------------------
// Run all tests.
// ---------------------------------------------------------------------------

function main(): void {
  console.log("[G-13] patterns.ts verification — 4 tests");
  console.log("----------------------------------------");
  test1_surveyGrid();
  console.log();
  test2_corridor();
  console.log();
  test3_perimeter();
  console.log();
  test4_integration();
  console.log("----------------------------------------");
  console.log(`[G-13] ${checks - failures}/${checks} checks passed`);
  if (failures > 0) {
    console.log(`[G-13] FAIL: ${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("[G-13] all checks passed");
  process.exit(0);
}

main();
