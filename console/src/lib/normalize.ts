/**
 * Consolidated normalizers for the GCS v2 Operations Canvas (M8, T-A3).
 *
 * Spec: docs/GCS_V2_SPEC.md §9.1 + §13.1 M8 skeleton interface #3 + P5/P8.
 *
 * Replaces the v1 normalizer copies — `conn.ts:normalizeFleetSnapshot`,
 * plus the implicit per-hook copies in `useFleetC2`, `useOperatorMap` — with
 * one module that:
 *
 *  - reads fleet-frame `attitude_q_wxyz` first (P5 fix: the live wire HAS
 *    the quaternion at `vehicles[].attitude_q_wxyz`; v1's `conn.ts:301`
 *    probed `q_wxyz` / `attitude.q_wxyz` and missed it, so the FlyView HUD
 *    fell back to a yaw-only synthesis with roll/pitch = 0);
 *  - tolerates the field-name variants across the live wire (degE7 fixes,
 *    `last_heartbeat_ms`, `current_task`) and the mock engines (`lat` /
 *    `lon` already decimal degrees, no quaternion);
 *  - is the single source for the v2 telemetry store
 *    (`state/telemetry-store.ts`).
 *
 * v1's `conn.ts` is left untouched (the `/` route still uses it). At M14 the
 * v1 tree dies and only this module remains.
 */

import type {
  FleetEvent,
  FleetSnapshot,
  FleetTask,
  FleetVehicle,
  Geofence,
  GeoOriginView,
} from './types.ts'

// ---------------------------------------------------------------------------
// tolerant field helpers (engine-neutral — work on live wire + mocks)
// ---------------------------------------------------------------------------

type Rec = Record<string, unknown>

function asRec(v: unknown): Rec | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Rec) : null
}

function num(...cands: unknown[]): number | null {
  for (const c of cands) {
    if (typeof c === 'number' && Number.isFinite(c)) return c
    if (typeof c === 'string' && c.trim() !== '' && Number.isFinite(Number(c))) return Number(c)
  }
  return null
}

function bool(...cands: unknown[]): boolean | null {
  for (const c of cands) {
    if (typeof c === 'boolean') return c
    if (c === 1 || c === 0) return c === 1
    if (typeof c === 'string') {
      const s = c.toLowerCase()
      if (s === 'true' || s === 'connected' || s === 'closed' || s === 'yes') return true
      if (s === 'false' || s === 'disconnected' || s === 'open' || s === 'no') return false
    }
  }
  return null
}

function str(...cands: unknown[]): string | null {
  for (const c of cands) if (typeof c === 'string' && c.length > 0) return c
  return null
}

function vec3(...cands: unknown[]): [number, number, number] | null {
  for (const c of cands) {
    if (Array.isArray(c) && c.length >= 3) {
      const a = num(c[0], c[1], c[2])
      const b = num(c[1])
      const d = num(c[2])
      if (a != null && b != null && d != null) return [a, b, d]
    }
    const r = asRec(c)
    if (r) {
      const x = num(r.north, r.n, r.x, r[0])
      const y = num(r.east, r.e, r.y, r[1])
      const z = num(r.down, r.d, r.z, r[2])
      if (x != null && y != null && z != null) return [x, y, z]
    }
  }
  return null
}

function numArr(c: unknown, len: number): number[] | null {
  if (Array.isArray(c) && c.length >= len && c.every((x) => num(x) != null)) {
    return c.slice(0, len).map((x) => num(x) as number)
  }
  return null
}

/** Unwrap `{"ok":bool,"data":...}` envelope if present, else return raw. */
function unwrapEnvelope(j: unknown): unknown {
  const r = asRec(j)
  if (r && 'ok' in r && 'data' in r) return (r as { data: unknown }).data ?? {}
  return j
}

/**
 * Yaw from quaternion (ZYX NED aerospace convention).
 * `q = [w, x, y, z]`; `yaw = atan2(2(wz + xy), 1 - 2(y² + z²))` normalized
 * to `[0, 360)` degrees.
 */
function yawFromQuat(q: readonly number[]): number {
  const [w, x, y, z] = q
  const siny = 2 * (w * z + x * y)
  const cosy = 1 - 2 * (y * y + z * z)
  let yaw = Math.atan2(siny, cosy) * (180 / Math.PI)
  if (yaw < 0) yaw += 360
  return yaw
}

// ---------------------------------------------------------------------------
// Fallback constants (dual-mode mock ladder — same shape as v1 conn.ts)
// ---------------------------------------------------------------------------

export const FALLBACK_FENCE: Geofence = {
  points: [
    [-45, -45],
    [45, -45],
    [45, 45],
    [-45, 45],
  ],
  ceiling_m: 60,
  floor_m: 0,
}

// ---------------------------------------------------------------------------
// mavfleet normalizers (SPEC §3.4 / §5)
// ---------------------------------------------------------------------------

/**
 * Normalize a /ws/fleet frame (or GET /api/fleet payload) into the
 * `{snapshot, events}` pair the v2 telemetry store consumes.
 *
 * Tolerant of field-name variants across the live wire
 * (`vehicles[].attitude_q_wxyz`, `lat_deg_e7`, `lon_deg_e7`,
 * `current_task`, `last_heartbeat_ms`) and the mock engine
 * (`vehicles[].lat` / `lon` already decimal degrees, no quaternion).
 *
 * P5 fix: reads `r.attitude_q_wxyz` FIRST (the actual wire field name),
 * and populates `attitude_q_wxyz` on the returned FleetVehicle. The legacy
 * `r.attitude.q_wxyz` / `r.q_wxyz` probes stay as fallbacks for tolerance.
 *
 * @returns `null` only if the input has zero vehicles (the unparseable case).
 */
export function normalizeFleetSnapshot(
  raw: unknown,
  prevVehicles: Record<string, FleetVehicle> | null,
): { snapshot: FleetSnapshot; events: FleetEvent[] } | null {
  const body = asRec(unwrapEnvelope(raw)) ?? {}
  const vehiclesRaw = Array.isArray(body.vehicles)
    ? body.vehicles
    : Array.isArray((asRec(body.fleet) ?? {}).vehicles)
      ? (asRec(body.fleet) as { vehicles: unknown[] }).vehicles
      : []
  if (vehiclesRaw.length === 0) return null

  const vehicles: FleetVehicle[] = vehiclesRaw.map((vr, i) => {
    const r = asRec(vr) ?? {}
    const id = str(r.id, r.name, r.vehicle_id) ?? `V${i + 1}`
    const prev = prevVehicles?.[id] ?? null
    const pos = vec3(r.position_ned_m, r.pos_ned_m, r.position, r.local_pos_ned_m) ?? prev?.position_ned_m ?? [0, 0, 0]

    // P5: read attitude_q_wxyz FIRST (the actual wire field name on the live
    // :8400 fleet WS — wire-verified 2026-09-09). The legacy probes
    // (`r.attitude.q_wxyz`, `r.q_wxyz`) stay as fallbacks for tolerance to
    // other planes / mocks that may use those nested / unprefixed shapes.
    // Carry-forward from `prev?.attitude_q_wxyz` for stability across
    // momentary wire dropouts (matches the v1 `pos`/`yaw` carry-forward pattern).
    const q =
      (numArr(r.attitude_q_wxyz, 4) as [number, number, number, number] | null) ??
      (numArr(asRec(r.attitude ?? r)?.q_wxyz, 4) as [number, number, number, number] | null) ??
      (numArr(r.q_wxyz, 4) as [number, number, number, number] | null) ??
      prev?.attitude_q_wxyz ?? null
    const yaw = num(r.yaw_deg, r.heading_deg) ?? (q ? yawFromQuat(q) : prev?.yaw_deg ?? 0)

    // Heartbeat age: v1 prefers `heartbeat_age_s`; the live wire exposes
    // `last_heartbeat_ms` (process uptime ms, monotonic). When only the ms
    // field is present we can't recompute the age without state, so we
    // defer to the previous vehicle's value (or 0).
    const hbAgeS = num(r.heartbeat_age_s, r.hb_age_s, r.heartbeat_age) ?? null
    const heartbeat_age_s = hbAgeS ?? prev?.heartbeat_age_s ?? 0

    const healthRaw = Array.isArray(r.health) ? r.health : []

    // ADR-0017: GLOBAL_POSITION_INT fix (degE7 on the wire -> degrees).
    // A fix of exactly (0,0) means "not received yet" (the Rust default).
    const latE7 = num(r.lat_deg_e7, r.lat_e7)
    const lonE7 = num(r.lon_deg_e7, r.lon_e7)
    const hasFix = latE7 != null && lonE7 != null && (latE7 !== 0 || lonE7 !== 0)
    const relAltMm = num(r.relative_alt_mm, r.rel_alt_mm)
    const altMm = num(r.alt_mm)

    return {
      id,
      index: num(r.index, r.i) ?? i,
      sysid: num(r.sysid, r.sys_id) ?? i + 1,
      mode: str(r.mode, r.flight_mode, r.nav_mode) ?? 'UNKNOWN',
      fsm: str(r.fsm, r.state, r.fsm_state) ?? 'INIT',
      battery_pct: num(r.battery_pct, r.battery, r.battery_remaining) ?? 0,
      voltage_v: num(r.voltage_v, r.battery_voltage_v, r.voltage),
      position_ned_m: pos,
      velocity_ned_ms: vec3(r.velocity_ned_ms, r.vel_ned_ms, r.velocity) ?? prev?.velocity_ned_ms ?? [0, 0, 0],
      // Live wire: degE7 → decimal degrees. Mock engines: already decimal
      // degrees via `r.lat` / `r.lon`. Has-fix gate is the ADR-0017 rule.
      lat: hasFix ? (latE7 as number) / 1e7 : (num(r.lat, r.latitude) ?? prev?.lat ?? null),
      lon: hasFix ? (lonE7 as number) / 1e7 : (num(r.lon, r.longitude) ?? prev?.lon ?? null),
      alt_msl_m: hasFix && altMm != null ? altMm / 1000 : (num(r.alt_msl_m, r.alt_msl) ?? prev?.alt_msl_m ?? null),
      alt_agl_m: hasFix && relAltMm != null ? relAltMm / 1000 : (num(r.alt_agl_m, r.alt_agl) ?? prev?.alt_agl_m ?? null),
      armed: bool(r.armed, r.is_armed) ?? prev?.armed ?? false,
      yaw_deg: yaw,
      heartbeat_age_s,
      stale: bool(r.stale, r.telemetry_stale) ?? heartbeat_age_s > 1.5,
      health: healthRaw.map((h) => String(h)),
      task_id: str(r.task_id, r.task, r.current_task) ?? null,
      breadcrumb: prev?.breadcrumb ?? [],
      // P5: propagate the quaternion so the Attitude HUD reads
      // attitude_q_wxyz → quatToEulerDeg (lib/format.ts). null = the field
      // is absent on the wire (mock engine / older backend) — caller falls
      // back to the v1 yaw-only synthesis.
      attitude_q_wxyz: q,
    }
  })

  const tasksRaw = Array.isArray(body.tasks) ? body.tasks : []
  const tasks: FleetTask[] = tasksRaw.map((tr) => {
    const r = asRec(tr) ?? {}
    const pos = vec3(r.pos_ned_m, r.position_ned_m, r.position) ?? [0, 0, -10]
    return {
      id: str(r.id, r.task_id) ?? '?',
      pos_ned_m: pos,
      hover_s: num(r.hover_s, r.hover) ?? 0,
      reward: num(r.reward) ?? 0,
      status: (str(r.status, r.state) ?? 'pending') as FleetTask['status'],
      assigned_to: str(r.assigned_to, r.vehicle, r.assignee) ?? null,
      progress: num(r.progress) ?? 0,
    }
  })

  const gfRec = asRec(body.geofence) ?? asRec(asRec(body.env)?.geofence) ?? null
  const gfPts = (gfRec ? (Array.isArray(gfRec.points_ned_m) ? gfRec.points_ned_m : gfRec.points) : null) as unknown
  const points: [number, number][] = Array.isArray(gfPts)
    ? gfPts
        .map((p) => {
          const a = Array.isArray(p) ? p : null
          if (a && a.length >= 2) return [num(a[0]) ?? 0, num(a[1]) ?? 0] as [number, number]
          const r = asRec(p)
          if (r) return [num(r.north, r.n) ?? 0, num(r.east, r.e) ?? 0] as [number, number]
          return null
        })
        .filter((p): p is [number, number] => p != null)
    : []

  // ADR-0017: the scenario geo origin rides the frame; when absent (older
  // backend) the map falls back to the PX4 test field constant.
  const originRec = asRec(body.geo_origin) ?? asRec(asRec(body.env)?.origin) ?? null
  const geo_origin: GeoOriginView | null = originRec
    ? {
        lat_deg: num(originRec.lat_deg, originRec.lat) ?? 47.39777,
        lon_deg: num(originRec.lon_deg, originRec.lon) ?? 8.54558,
        alt_m: num(originRec.alt_m, originRec.alt) ?? 500,
      }
    : null

  const events: FleetEvent[] = (Array.isArray(body.events) ? body.events : []).map(normalizeEvent).filter((e): e is FleetEvent => e != null)

  return {
    snapshot: {
      phase: str(body.phase, body.fleet_phase, body.state) ?? 'RUNNING',
      t_s: num(body.t_s, body.t, body.virtual_time_s) ?? 0,
      vehicles,
      tasks,
      geofence: {
        points: points.length >= 3 ? points : FALLBACK_FENCE.points,
        ceiling_m: num(gfRec?.ceiling_m, gfRec?.ceiling) ?? FALLBACK_FENCE.ceiling_m,
        floor_m: num(gfRec?.floor_m, gfRec?.floor) ?? FALLBACK_FENCE.floor_m,
      },
      geo_origin,
    },
    events,
  }
}

function normalizeEvent(e: unknown): FleetEvent | null {
  const r = asRec(e) ?? {}
  const detail = str(r.detail, r.message, r.desc, r.text)
  if (!detail) return null
  const sevRaw = str(r.severity, r.level)?.toLowerCase()
  const severity: FleetEvent['severity'] =
    sevRaw === 'warn' || sevRaw === 'warning'
      ? 'warn'
      : sevRaw === 'critical' || sevRaw === 'crit' || sevRaw === 'error'
        ? 'critical'
        : 'info'
  return {
    t: num(r.t, r.timestamp, r.t_ms, r.wall_ms) ?? Date.now(),
    t_s: num(r.t_s, r.virtual_time_s, r.t) ?? 0,
    kind: str(r.kind, r.type, r.category) ?? 'info',
    vehicle: str(r.vehicle, r.vehicle_id),
    detail,
    severity,
  }
}
