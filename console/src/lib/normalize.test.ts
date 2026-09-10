/**
 * Logic-first unit tests for `lib/normalize.ts` (M8, T-A3).
 *
 * Spec: docs/GCS_V2_SPEC.md §13.1 M8 skeleton interface #3 + §13.3 working
 * conventions ("Logic-first unit tests (node:test, no browser) land with the
 * modules: lib/normalize.test.ts (field-name regressions à la P5)...").
 *
 * Run:
 *   node --test src/lib/normalize.test.ts
 * or via `npm test` (which restricts to tests matching `^normalize`).
 *
 * Uses Node 24's built-in type-stripping (`--test` runs `.ts` files directly,
 * imports resolve relative `.ts` paths). No tsx, no vitest, no jest — the
 * spec forbids new runtime deps beyond §9.3's four (maplibre-gl,
 * react-map-gl, @radix-ui/react-dropdown-menu, @radix-ui/react-dialog).
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'

import {
  normalizeFleetSnapshot,
  FALLBACK_FENCE,
} from './normalize.ts'

// ---------------------------------------------------------------------------
// P5 regression — the headline test (GCS_V2_SPEC.md §11 P5, G-14 assertion:
// "normalize.test.ts field-name regression"). The live :8400 fleet wire carries
// attitude as `vehicles[].attitude_q_wxyz` (top-level, with underscore). v1's
// conn.ts:301 probed `q_wxyz` / `attitude.q_wxyz` and missed it, so v1's
// FlyView synthesized a fake quaternion from yaw_deg alone (roll/pitch = 0).
// v2's normalizer MUST read `attitude_q_wxyz` first and propagate it onto the
// returned FleetVehicle.
// ---------------------------------------------------------------------------

test('P5: normalizeFleetSnapshot reads attitude_q_wxyz (top-level, with underscore) from the live wire', () => {
  // Real wire frame shape, captured live 2026-09-09 from /api/fleet on a
  // 2-vehicle PX4 SITL fleet (vehicle 0 holding near level, q ≈ [1, ~0, ~0, 0.01]).
  const raw = {
    ok: true,
    data: {
      phase: 'RUNNING',
      t_s: 1.234,
      vehicles: [
        {
          id: 'v0',
          index: 0,
          sysid: 1,
          mode: 'OFFBOARD',
          fsm: 'READY',
          battery_pct: 100,
          voltage_v: 12.6,
          position_ned_m: [0.1, 0.2, -10.3],
          velocity_ned_ms: [0, 0, 0],
          // The actual wire field — what v1 missed.
          attitude_q_wxyz: [0.9999516606330872, -0.0002893558703362942, 0.00046396534889936447, 0.009813201613724232],
          lat_deg_e7: 473977700,
          lon_deg_e7: 85455800,
          alt_mm: 489650,
          relative_alt_mm: -10280,
          armed: false,
          yaw_deg: 1.12,
          last_heartbeat_ms: 50,
          health: ['ekf2', 'gps'],
          current_task: null,
        },
      ],
      tasks: [],
      geofence: { points_ned_m: [[-45, -45], [45, -45], [45, 45], [-45, 45]], ceiling_m: 60, floor_m: 0 },
      geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
    },
  }

  const out = normalizeFleetSnapshot(raw, null)
  assert.ok(out, 'normalizer returned null on a valid frame')
  const v0 = out.snapshot.vehicles[0]
  assert.equal(v0.id, 'v0')
  // The P5 contract: attitude_q_wxyz must be populated and equal to the wire value.
  assert.ok(v0.attitude_q_wxyz, 'attitude_q_wxyz is missing on the normalized vehicle (P5 regression)')
  assert.deepEqual(
    v0.attitude_q_wxyz,
    [0.9999516606330872, -0.0002893558703362942, 0.00046396534889936447, 0.009813201613724232],
    'attitude_q_wxyz must equal the wire value',
  )
})

test('P5 fallback chain: attitude.q_wxyz (nested) and q_wxyz (unprefixed) still work for tolerance', () => {
  // Some planes / mocks may use the nested shape or the unprefixed shape; the
  // normalizer must still extract a quaternion from those (engine-neutral).
  const nestedRaw = {
    ok: true,
    data: {
      phase: 'RUNNING',
      t_s: 0,
      vehicles: [{ id: 'v0', attitude: { q_wxyz: [0.7, 0, 0, 0.7] }, lat_deg_e7: 473977700, lon_deg_e7: 85455800 }],
      tasks: [],
      geofence: { points_ned_m: [[-45, -45], [45, -45], [45, 45], [-45, 45]], ceiling_m: 60, floor_m: 0 },
      geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
    },
  }
  const unprefixedRaw = {
    ok: true,
    data: {
      phase: 'RUNNING',
      t_s: 0,
      vehicles: [{ id: 'v0', q_wxyz: [0.5, 0.5, 0.5, 0.5], lat_deg_e7: 473977700, lon_deg_e7: 85455800 }],
      tasks: [],
      geofence: { points_ned_m: [[-45, -45], [45, -45], [45, 45], [-45, 45]], ceiling_m: 60, floor_m: 0 },
      geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
    },
  }

  const outN = normalizeFleetSnapshot(nestedRaw, null)
  const outU = normalizeFleetSnapshot(unprefixedRaw, null)
  assert.deepEqual(outN?.snapshot.vehicles[0].attitude_q_wxyz, [0.7, 0, 0, 0.7])
  assert.deepEqual(outU?.snapshot.vehicles[0].attitude_q_wxyz, [0.5, 0.5, 0.5, 0.5])
})

test('P5: attitude_q_wxyz absent → null on the normalized vehicle (mock-engine shape)', () => {
  // Mock engines (lib/mock-fleet.ts) emit no quaternion; the normalizer
  // must surface this as `null` so the HUD falls back to the v1 yaw-only
  // synthesis (never silently produce a fake identity quaternion).
  const raw = {
    ok: true,
    data: {
      phase: 'RUNNING',
      t_s: 0,
      vehicles: [{ id: 'v0', yaw_deg: 90, lat: 47.39777, lon: 8.54558, position_ned_m: [0, 0, -10] }],
      tasks: [],
      geofence: { points_ned_m: [[-45, -45], [45, -45], [45, 45], [-45, 45]], ceiling_m: 60, floor_m: 0 },
      geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
    },
  }
  const out = normalizeFleetSnapshot(raw, null)
  assert.equal(out?.snapshot.vehicles[0].attitude_q_wxyz, null)
  // yaw_deg still propagated so the v1 fallback works.
  assert.equal(out?.snapshot.vehicles[0].yaw_deg, 90)
})

// ---------------------------------------------------------------------------
// ADR-0017: GLOBAL_POSITION_INT degE7 → decimal degrees + has-fix gate.
// (0, 0) on the wire means "not received yet" (the Rust default); the
// normalizer must surface lat/lon as `null` in that case, not 0,0.
// ---------------------------------------------------------------------------

test('lat_deg_e7 / lon_deg_e7 → decimal degrees (ADR-0017)', () => {
  const raw = {
    ok: true,
    data: {
      vehicles: [{ id: 'v0', lat_deg_e7: 473977700, lon_deg_e7: 85455800, alt_mm: 489650, relative_alt_mm: -10280 }],
      tasks: [],
      geofence: { points_ned_m: [[-45, -45], [45, -45], [45, 45], [-45, 45]], ceiling_m: 60, floor_m: 0 },
      geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
    },
  }
  const out = normalizeFleetSnapshot(raw, null)
  const v = out?.snapshot.vehicles[0]
  assert.equal(v?.lat, 47.39777, 'lat must be decimal degrees (473977700 / 1e7)')
  assert.equal(v?.lon, 8.54558, 'lon must be decimal degrees (85455800 / 1e7)')
  assert.equal(v?.alt_msl_m, 489.65, 'alt_msl_m = alt_mm / 1000')
  assert.equal(v?.alt_agl_m, -10.28, 'alt_agl_m = relative_alt_mm / 1000')
})

test('lat/lon (0,0) means "no fix yet" — normalized to null, not 0,0 (ADR-0017)', () => {
  const raw = {
    ok: true,
    data: {
      vehicles: [{ id: 'v0', lat_deg_e7: 0, lon_deg_e7: 0 }],
      tasks: [],
      geofence: { points_ned_m: [[-45, -45], [45, -45], [45, 45], [-45, 45]], ceiling_m: 60, floor_m: 0 },
      geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
    },
  }
  const out = normalizeFleetSnapshot(raw, null)
  assert.equal(out?.snapshot.vehicles[0].lat, null)
  assert.equal(out?.snapshot.vehicles[0].lon, null)
})

test('mock-engine shape (already-decimal lat/lon) is also accepted', () => {
  // The mock-fleet engine emits lat/lon as decimal degrees directly.
  const raw = {
    ok: true,
    data: {
      vehicles: [{ id: 'v0', lat: 47.39777, lon: 8.54558, position_ned_m: [0, 0, -10] }],
      tasks: [],
      geofence: { points: [[-45, -45], [45, -45], [45, 45], [-45, 45]], ceiling_m: 60, floor_m: 0 },
      geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
    },
  }
  const out = normalizeFleetSnapshot(raw, null)
  assert.equal(out?.snapshot.vehicles[0].lat, 47.39777)
  assert.equal(out?.snapshot.vehicles[0].lon, 8.54558)
})

// ---------------------------------------------------------------------------
// Tolerance + envelope — the normalizer must accept the {ok,data} envelope
// and the bare-frame variants (no envelope, vehicles at top vs under `fleet`).
// ---------------------------------------------------------------------------

test('envelope unwrap: {ok:true, data:{...}} → data', () => {
  const raw = { ok: true, data: { vehicles: [{ id: 'v0' }], geofence: { points_ned_m: [[-1, -1], [1, -1], [1, 1], [-1, 1]] } } }
  assert.ok(normalizeFleetSnapshot(raw, null))
})

test('bare frame (no envelope) is also accepted', () => {
  const raw = { vehicles: [{ id: 'v0' }], geofence: { points_ned_m: [[-1, -1], [1, -1], [1, 1], [-1, 1]] } }
  assert.ok(normalizeFleetSnapshot(raw, null))
})

test('`fleet.vehicles` nested shape is also accepted (older wire variant)', () => {
  const raw = { fleet: { vehicles: [{ id: 'v0' }], geofence: { points_ned_m: [[-1, -1], [1, -1], [1, 1], [-1, 1]] } } }
  assert.ok(normalizeFleetSnapshot(raw, null))
})

test('empty vehicles array → null (the unparseable case)', () => {
  assert.equal(normalizeFleetSnapshot({ ok: true, data: { vehicles: [] } }, null), null)
  assert.equal(normalizeFleetSnapshot({ vehicles: [] }, null), null)
  assert.equal(normalizeFleetSnapshot({}, null), null)
  assert.equal(normalizeFleetSnapshot(null, null), null)
})

// ---------------------------------------------------------------------------
// Geofence fallback — < 3 vertices → FALLBACK_FENCE (matches v1 conn.ts).
// ---------------------------------------------------------------------------

test('geofence < 3 vertices → FALLBACK_FENCE points', () => {
  const raw = {
    vehicles: [{ id: 'v0', lat_deg_e7: 473977700, lon_deg_e7: 85455800 }],
    geofence: { points_ned_m: [[0, 0]], ceiling_m: 99, floor_m: 5 },
    geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
  }
  const out = normalizeFleetSnapshot(raw, null)
  assert.deepEqual(out?.snapshot.geofence.points, FALLBACK_FENCE.points)
  // ceiling/floor still read from the input (only points fall back).
  assert.equal(out?.snapshot.geofence.ceiling_m, 99)
  assert.equal(out?.snapshot.geofence.floor_m, 5)
})

test('geo_origin absent → null (map falls back to DEFAULT_ORIGIN in lib/geo.ts)', () => {
  const raw = {
    vehicles: [{ id: 'v0', lat_deg_e7: 473977700, lon_deg_e7: 85455800 }],
    geofence: { points_ned_m: [[-1, -1], [1, -1], [1, 1], [-1, 1]] },
  }
  const out = normalizeFleetSnapshot(raw, null)
  assert.equal(out?.snapshot.geo_origin, null)
})

// ---------------------------------------------------------------------------
// prevVehicles carry-forward — breadcrumb, pos, q, yaw survive missing fields.
// ---------------------------------------------------------------------------

test('prevVehicles carry-forward: pos/q/yaw/breadcrumb when wire omits them', () => {
  // prev q is the quaternion for yaw=42° (so the carry-forward yields the
  // same yaw as prev — the data is internally consistent, as a real frame
  // would be). q = [cos(21°), 0, 0, sin(21°)] ≈ [0.9336, 0, 0, 0.3584].
  const prevYaw = 42
  const prevQ: [number, number, number, number] = [
    Math.cos((prevYaw / 2) * Math.PI / 180),
    0,
    0,
    Math.sin((prevYaw / 2) * Math.PI / 180),
  ]
  const prevVehicles = {
    v0: {
      id: 'v0',
      index: 0,
      sysid: 1,
      mode: 'OFFBOARD',
      fsm: 'READY',
      battery_pct: 99,
      voltage_v: 12.5,
      position_ned_m: [1, 2, 3] as [number, number, number],
      velocity_ned_ms: [0, 0, 0] as [number, number, number],
      lat: 47.39777,
      lon: 8.54558,
      alt_msl_m: 489.65,
      alt_agl_m: -10.28,
      armed: false,
      yaw_deg: prevYaw,
      heartbeat_age_s: 0.1,
      stale: false,
      health: ['ekf2'],
      task_id: null,
      breadcrumb: [{ n: 1, e: 2 }, { n: 1.5, e: 2.5 }],
      attitude_q_wxyz: prevQ,
    },
  }
  // Wire drops position_ned_m, q, yaw_deg, breadcrumb — the normalizer must
  // carry them forward from prev.
  const raw = {
    vehicles: [{ id: 'v0', lat_deg_e7: 473977700, lon_deg_e7: 85455800 }],
    geofence: { points_ned_m: [[-1, -1], [1, -1], [1, 1], [-1, 1]] },
    geo_origin: { lat_deg: 47.39777, lon_deg: 8.54558, alt_m: 500 },
  }
  const out = normalizeFleetSnapshot(raw, prevVehicles)
  const v = out?.snapshot.vehicles[0]
  assert.deepEqual(v?.position_ned_m, [1, 2, 3])
  assert.deepEqual(v?.attitude_q_wxyz, prevQ, 'attitude_q_wxyz carried forward from prev (P5 stability)')
  // yaw derived from the carried-forward quaternion ≈ prev yaw (within
  // float32 epsilon — yawFromQuat is exact for this axis-only quaternion).
  assert.ok(Math.abs((v?.yaw_deg ?? 0) - prevYaw) < 1e-6, `yaw_deg derived from carried q ≈ ${prevYaw}, got ${v?.yaw_deg}`)
  assert.deepEqual(v?.breadcrumb, [{ n: 1, e: 2 }, { n: 1.5, e: 2.5 }])
})

