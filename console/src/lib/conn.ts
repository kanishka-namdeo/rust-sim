/**
 * Gateway client + protocol normalizers.
 *
 * Two API routing styles:
 *
 * - "gateway" (legacy web stack): every request is a RELATIVE path; the
 *   backend port travels in the `XTransformPort` query parameter and the
 *   Caddy gateway (see Caddyfile.example) forwards it:
 *     REST:  fetch('/api/status?XTransformPort=8200')
 *     WS:    new WebSocket('/?XTransformPort=8200')
 *   Used behind the preview proxy (scripts/stack_up.sh + Caddy :81)
 *   where absolute localhost URLs are unreachable from the browser.
 *
 * - "direct" (Tauri default, also dev-without-gateway): absolute
 *   localhost URLs straight to the Rust control planes. Both backends
 *   upgrade WebSockets at `/` (and their spec paths /ws/telemetry,
 *   /ws/fleet).
 *
 * M-T1 (Tauri repurpose): the Tauri build sets
 * NEXT_PUBLIC_RSIM_API_STYLE=direct at build time via .env.production;
 * the legacy web stack keeps gateway mode (default). See
 * docs/TAURI_APP_SPEC.md §5.1 and Appendix F.3.
 */

import type {
  FleetEvent,
  FleetSnapshot,
  FleetTask,
  FleetVehicle,
} from './types'

/** Routing style: "gateway" (relative + XTransformPort) or "direct" (localhost). */
export const API_STYLE: 'gateway' | 'direct' =
  process.env.NEXT_PUBLIC_RSIM_API_STYLE === 'direct' ? 'direct' : 'gateway'

/** Build a URL for a backend port in the active routing style. */
export function gw(port: number, path: string, query: Record<string, string | number> = {}): string {
  const qs = new URLSearchParams()
  for (const [k, v] of Object.entries(query)) qs.set(k, String(v))
  if (API_STYLE === 'direct') {
    const q = qs.toString()
    return `http://127.0.0.1:${port}${path}${q ? `?${q}` : ''}`
  }
  qs.set('XTransformPort', String(port))
  return `${path}?${qs.toString()}`
}

/** Build the WebSocket URL: gateway query-routing, or direct localhost. */
export function wsUrl(port: number): string {
  if (typeof window === 'undefined') return ''
  if (API_STYLE === 'direct') {
    return `ws://127.0.0.1:${port}/`
  }
  const proto = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${window.location.host}/?XTransformPort=${port}`
}

/** fetch with timeout + no-store; throws on network error / non-2xx. */
export async function fetchGw(
  url: string,
  init: RequestInit = {},
  timeoutMs = 3500,
): Promise<Response> {
  const ctrl = new AbortController()
  const timer = setTimeout(() => ctrl.abort(), timeoutMs)
  try {
    return await fetch(url, { ...init, cache: 'no-store', signal: ctrl.signal })
  } finally {
    clearTimeout(timer)
  }
}

/** Probe a backend plane; resolves true only when it answers with an envelope. */
export async function probePlane(url: string, timeoutMs = 2500): Promise<boolean> {
  try {
    const res = await fetchGw(url, { method: 'GET' }, timeoutMs)
    if (!res.ok) return false
    const j: unknown = await res.json().catch(() => null)
    if (j && typeof j === 'object' && 'ok' in (j as Record<string, unknown>)) {
      return Boolean((j as { ok?: unknown }).ok)
    }
    return true // 2xx JSON without envelope — still alive
  } catch {
    return false
  }
}

// ---------------------------------------------------------------------------
// tolerant scalar helpers
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

/** Unwrap {"ok":bool,"data":...} envelope if present, else return raw. */
export function unwrapEnvelope(j: unknown): unknown {
  const r = asRec(j)
  if (r && 'ok' in r && 'data' in r) return (r as { data: unknown }).data ?? {}
  return j
}

// ---------------------------------------------------------------------------
// mavfleet normalizers (SPEC §3.4 / §5)
// ---------------------------------------------------------------------------

const FALLBACK_FENCE: FleetSnapshot['geofence'] = {
  points: [
    [-45, -45],
    [45, -45],
    [45, 45],
    [-45, 45],
  ],
  ceiling_m: 60,
  floor_m: 0,
}

/** Normalize a /ws/fleet frame or GET /api/fleet payload. */
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
    const q = numArr(asRec(r.attitude ?? r)?.q_wxyz ?? r.q_wxyz, 4)
    const yaw = num(r.yaw_deg, r.heading_deg) ?? (q ? yawFromQuat(q) : prev?.yaw_deg ?? 0)
    const hbAge = num(r.heartbeat_age_s, r.hb_age_s, r.heartbeat_age) ?? 0
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
      lat: hasFix ? (latE7 as number) / 1e7 : prev?.lat ?? null,
      lon: hasFix ? (lonE7 as number) / 1e7 : prev?.lon ?? null,
      alt_msl_m: hasFix && altMm != null ? altMm / 1000 : prev?.alt_msl_m ?? null,
      alt_agl_m: hasFix && relAltMm != null ? relAltMm / 1000 : prev?.alt_agl_m ?? null,
      armed: bool(r.armed, r.is_armed) ?? prev?.armed ?? false,
      yaw_deg: yaw,
      heartbeat_age_s: hbAge,
      stale: bool(r.stale, r.telemetry_stale) ?? hbAge > 1.5,
      health: healthRaw.map((h) => String(h)),
      task_id: str(r.task_id, r.task, r.current_task) ?? null,
      breadcrumb: prev?.breadcrumb ?? [],
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
  const geo_origin = originRec
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
  const severity = sevRaw === 'warn' || sevRaw === 'warning' ? 'warn' : sevRaw === 'critical' || sevRaw === 'crit' || sevRaw === 'error' ? 'critical' : 'info'
  return {
    t: num(r.t, r.timestamp, r.t_ms, r.wall_ms) ?? Date.now(),
    t_s: num(r.t_s, r.virtual_time_s, r.t) ?? 0,
    kind: str(r.kind, r.type, r.category) ?? 'info',
    vehicle: str(r.vehicle, r.vehicle_id),
    detail,
    severity,
  }
}

function yawFromQuat(q: readonly number[]): number {
  const [w, x, y, z] = q
  const siny = 2 * (w * z + x * y)
  const cosy = 1 - 2 * (y * y + z * z)
  let yaw = Math.atan2(siny, cosy) * (180 / Math.PI)
  if (yaw < 0) yaw += 360
  return yaw
}
