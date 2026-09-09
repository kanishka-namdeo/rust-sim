/**
 * GCS v2 Operations Canvas — MapLibre camera rules (M8, T-A2).
 *
 * Spec: docs/GCS_V2_SPEC.md §6.5 — the camera behavior contract:
 *
 * - Default: pitch 0, bearing 0, maxPitch 60, fit to fence + 20% padding on
 *   boot (or home if no fence). `Z` resets (north-up, pitch 0, re-fit fence
 *   — v1.1 re-bind from `R`, which stays RTL). `X` toggles pitch 0 ↔ 55
 *   ("3D glance").
 * - Follow mode (default on): camera easeTo vehicle at ≥ 1 Hz, only when
 *   the operator has not manually panned for > 5s (manual pan suspends
 *   follow; `C` or the follow chip re-engages) — prevents the follow/pan
 *   fight.
 * - NED inset: `V` toggles. (M8 just registers the key; the full NED
 *   re-projection lands at M15.)
 * - Zoom clamps: 3..19; `+`/`-` step 1. Bounding-box vehicle select =
 *   `Shift+drag` (§7.1 — C lands at M9).
 * - `fitBounds` before `invalidateSize` is the O-2 fence-guard bug class
 *   — MapLibre has no 0×0 hidden-container problem, which deletes that
 *   entire bug class. The fence-size regression guard still ships as a
 *   gate assertion (G-20).
 *
 * M8 scope: boot fit-to-fence + follow-mode easeTo. `Z`/`X`/`C`/`V` and
 * manual-pan suspend land with the ShortcutsProvider at M9 (T-C1).
 */

import type { FleetSnapshot, FleetVehicle, Geofence } from '@/lib/types'
import { nedPolygonToGeo, DEFAULT_ORIGIN, type GeoOrigin } from '@/lib/geo'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface CameraState {
  follow: boolean
  /** Wall-clock ms of the last operator-initiated pan/drag; 0 = never. */
  lastManualPanAt: number
  /** The last vehicle the camera eased to (for ≥1 Hz cadence). */
  lastFollowVehicleId: string | null
  lastFollowEaseAt: number
  pitch: number
  bearing: number
  mode: 'geo' | 'ned'
}

export function initialCameraState(): CameraState {
  return {
    follow: true,
    lastManualPanAt: 0,
    lastFollowVehicleId: null,
    lastFollowEaseAt: 0,
    pitch: 0,
    bearing: 0,
    mode: 'geo',
  }
}

export const DEFAULT_PITCH = 0
export const DEFAULT_BEARING = 0
export const MAX_PITCH = 60
export const PITCH_GLANCE_DEG = 55 // §6.5 X "3D glance"
export const MIN_ZOOM = 3
export const MAX_ZOOM = 19
export const FOLLOW_SUSPEND_MS = 5000 // §6.5 manual pan suspends follow >5s
export const FOLLOW_EASE_MIN_MS = 1000 // §6.5 camera easeTo vehicle at ≥1 Hz
export const FIT_PADDING_RATIO = 0.2 // §6.5 fit to fence + 20% padding

// ---------------------------------------------------------------------------
// fitBounds from a fence polygon — the boot default. Falls back to
// DEFAULT_ORIGIN if no fence.
// ---------------------------------------------------------------------------

export interface Bounds {
  west: number
  south: number
  east: number
  north: number
}

export function fenceBounds(snapshot: FleetSnapshot | null): Bounds | null {
  if (!snapshot || !snapshot.geo_origin) return null
  const origin: GeoOrigin = snapshot.geo_origin
  const fence: Geofence | null = snapshot.geofence
  if (!fence || fence.points.length < 3) {
    // No usable fence — fit to home (or DEFAULT_ORIGIN) at zoom 16.
    return {
      west: origin.lon_deg - 0.005,
      south: origin.lat_deg - 0.003,
      east: origin.lon_deg + 0.005,
      north: origin.lat_deg + 0.003,
    }
  }
  const geo = nedPolygonToGeo(origin, fence.points)
  let west = Infinity
  let south = Infinity
  let east = -Infinity
  let north = -Infinity
  for (const p of geo) {
    if (p.lng < west) west = p.lng
    if (p.lng > east) east = p.lng
    if (p.lat < south) south = p.lat
    if (p.lat > north) north = p.lat
  }
  return { west, south, east, north }
}

export function fitBoundsOptions(bounds: Bounds): {
  padding: { top: number; bottom: number; left: number; right: number }
  pitch: number
  bearing: number
} {
  // 20% padding around the bounds (spec §6.5).
  return {
    padding: { top: 80, bottom: 96 + 80, left: 56, right: 312 + 40 },
    pitch: DEFAULT_PITCH,
    bearing: DEFAULT_BEARING,
  }
}

// ---------------------------------------------------------------------------
// Follow mode — camera easeTo the active vehicle at ≥1 Hz, suspended by
// manual pan.
// ---------------------------------------------------------------------------

export function shouldFollowEase(
  state: CameraState,
  vehicle: FleetVehicle | null,
  now: number,
): boolean {
  if (!state.follow || !vehicle || vehicle.lat == null || vehicle.lon == null) return false
  // Suspend if operator panned within FOLLOW_SUSPEND_MS.
  if (state.lastManualPanAt > 0 && now - state.lastManualPanAt < FOLLOW_SUSPEND_MS) return false
  // Cadence: ≥1 Hz.
  if (now - state.lastFollowEaseAt < FOLLOW_EASE_MIN_MS) return false
  return true
}

export function followEaseTarget(
  vehicle: FleetVehicle,
  state: CameraState,
): { center: [number, number]; zoom: number; pitch: number; bearing: number } {
  // Keep current zoom + pitch (follow is pan-only by default; the operator
  // can adjust zoom separately). Bearing stays north-up unless the operator
  // chose a bearing manually (M9 lands the `B` rotate-bear bindings).
  return {
    center: [vehicle.lon as number, vehicle.lat as number],
    zoom: Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, 16)),
    pitch: state.pitch,
    bearing: state.bearing,
  }
}

export function markManualPan(state: CameraState, now: number): CameraState {
  return { ...state, lastManualPanAt: now }
}

export function markFollowEase(state: CameraState, vehicleId: string, now: number): CameraState {
  return { ...state, lastFollowVehicleId: vehicleId, lastFollowEaseAt: now }
}

// ---------------------------------------------------------------------------
// Reset (Z) and pitch glance (X) — the camera-verb state transitions.
// §6.5: Z resets north-up + pitch 0 + re-fit fence.
// §6.5: X toggles pitch 0 ↔ 55.
// ---------------------------------------------------------------------------

export function resetCamera(state: CameraState): CameraState {
  return { ...state, pitch: DEFAULT_PITCH, bearing: DEFAULT_BEARING, lastManualPanAt: 0 }
}

export function togglePitchGlance(state: CameraState): CameraState {
  return {
    ...state,
    pitch: state.pitch === 0 ? PITCH_GLANCE_DEG : 0,
  }
}

// ---------------------------------------------------------------------------
// NED inset (V) — the mode marker only at M8. The full NED re-projection
// of all GeoJSON layers through lib/geo.ts LLA↔NED anchored on the frame's
// geo_origin lands at M15 (T-M15).
// ---------------------------------------------------------------------------

export function toggleNedInset(state: CameraState): CameraState {
  return { ...state, mode: state.mode === 'geo' ? 'ned' : 'geo' }
}

// ---------------------------------------------------------------------------
// DEFAULT_ORIGIN fallback for MapCanvas boot — the PX4 test field (47.39777,
// 8.54558) per lib/geo.ts.
// ---------------------------------------------------------------------------

export function bootCenter(): [number, number] {
  return [DEFAULT_ORIGIN.lon_deg, DEFAULT_ORIGIN.lat_deg]
}

export function bootZoom(): number {
  return 16
}
