/** Formatting + math helpers shared by both consoles. */

/** Fixed-decimal formatter with NaN/undefined guard. */
export function fmt(v: number | null | undefined, digits = 2, fallback = '—'): string {
  if (v == null || !Number.isFinite(v)) return fallback
  return v.toFixed(digits)
}

/** Signed fixed-decimal, e.g. "+3.2". */
export function fmtSigned(v: number | null | undefined, digits = 2, fallback = '—'): string {
  if (v == null || !Number.isFinite(v)) return fallback
  const s = v.toFixed(digits)
  return v >= 0 ? `+${s}` : s
}

/** Format a vector as "x / y / z" with given digits. */
export function fmtVec(v: readonly number[] | null | undefined, digits = 2): string {
  if (!v || v.length === 0) return '—'
  return v.map((x) => fmt(x, digits)).join(' / ')
}

/** Virtual microseconds → "mm:ss.d" mission clock. */
export function fmtMissionClock(t_us: number | null | undefined): string {
  if (t_us == null || !Number.isFinite(t_us)) return '—'
  const total = Math.max(0, t_us) / 1e6
  const m = Math.floor(total / 60)
  const s = total - m * 60
  return `${String(m).padStart(2, '0')}:${s.toFixed(1).padStart(4, '0')}`
}

/** Wall-clock epoch ms → "HH:MM:SS" (local). */
export function fmtWallClock(t: number): string {
  const d = new Date(t)
  return [d.getHours(), d.getMinutes(), d.getSeconds()]
    .map((x) => String(x).padStart(2, '0'))
    .join(':')
}

/** Battery percentage → semantic tone class. */
export function batteryTone(pct: number | null | undefined): 'ok' | 'warn' | 'crit' | 'unknown' {
  if (pct == null || !Number.isFinite(pct)) return 'unknown'
  if (pct < 20) return 'crit' // mavfleet policy 4: BATTERY_CRIT
  if (pct < 30) return 'warn' // mavfleet policy 5: BATTERY_LOW
  return 'ok'
}

/**
 * Quaternion (w, x, y, z) → aerospace ZYX Euler angles in degrees.
 * NED frame: roll φ about x (north), pitch θ about y (east), yaw ψ about z (down).
 */
export function quatToEulerDeg(
  q: readonly number[] | null | undefined,
): { roll_deg: number; pitch_deg: number; yaw_deg: number } {
  const fallback = { roll_deg: 0, pitch_deg: 0, yaw_deg: 0 }
  if (!q || q.length < 4 || q.every((x) => !Number.isFinite(x))) return fallback
  const [w0, x0, y0, z0] = q
  const norm = Math.hypot(w0, x0, y0, z0)
  if (norm < 1e-9) return fallback
  const w = w0 / norm
  const x = x0 / norm
  const y = y0 / norm
  const z = z0 / norm

  const sinr_cosp = 2 * (w * x + y * z)
  const cosr_cosp = 1 - 2 * (x * x + y * y)
  const roll = Math.atan2(sinr_cosp, cosr_cosp)

  const sinp = 2 * (w * y - z * x)
  const pitch = Math.asin(Math.min(1, Math.max(-1, sinp)))

  const siny_cosp = 2 * (w * z + x * y)
  const cosy_cosp = 1 - 2 * (y * y + z * z)
  const yaw = Math.atan2(siny_cosp, cosy_cosp)

  const rad2deg = 180 / Math.PI
  let yawDeg = yaw * rad2deg
  if (yawDeg < 0) yawDeg += 360 // compass-style 0..360
  return { roll_deg: roll * rad2deg, pitch_deg: pitch * rad2deg, yaw_deg: yawDeg }
}

/** Clamp v into [lo, hi]. */
export function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v))
}

/** Lerp. */
export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t
}

/** Small deterministic-ish PRNG (mulberry32) for the mock engines. */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a |= 0
    a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/** Box–Muller gaussian noise. */
export function gaussian(rng: () => number, sigma = 1): number {
  const u1 = Math.max(1e-9, rng())
  const u2 = rng()
  return sigma * Math.sqrt(-2 * Math.log(u1)) * Math.cos(2 * Math.PI * u2)
}
