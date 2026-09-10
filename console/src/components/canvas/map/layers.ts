/**
 * GCS v2 Operations Canvas — MapLibre layer catalog (M8, T-A2).
 *
 * Spec: docs/GCS_V2_SPEC.md §6.4 — the single source of truth for map
 * sources + layers. All dynamic data as **GeoJSON sources updated via
 * `source.setData(fc)`** (the official realtime pattern) — never DOM
 * markers (P2 lesson: imperative marker soup in v1's three Leaflet
 * components). Vehicles as symbol+circle layers so `queryRenderedFeatures`
 * hit-testing works (§7.2 context menus — C lands at M9).
 *
 * M8 scope: L1 (vehicles), L2 (tracks), L13 (graticule). The full L1-L13
 * catalog lands across M8-M13; this module ships the M8 subset plus the
 * catalog's `LayerId` type and the `__rsimMapDebug.layers` counters (§9.2 —
 * part of the binding gate-instrument contract).
 *
 * Layer add order is fixed at init (once); per-frame updates only call
 * `setData`. The MapCanvas component calls `addLayers(map)` once after
 * the basemap `load` event, then `setData` on each frame from the
 * FlushLoop-driven render pass (§9.2).
 */

import type { FleetVehicle, FleetSnapshot } from '@/lib/types'
import { getTrack } from '@/state/telemetry-store'

// ---------------------------------------------------------------------------
// Per-vehicle identity colors (HSL-spaced; v0 cyan, v1 violet per spec §5.1).
// Extended by 60° HSL for fleet sizes up to 6; the spec §6.4 L1 says "extends
// per fleet size, HSL-spaced". For M8 we only need v0/v1.
// ---------------------------------------------------------------------------

export function vehicleColor(i: number): string {
  // Spec §5.1: --rsim-v0 #22D3EE (cyan 188°) / --rsim-v1 #A78BFA (violet 262°).
  // HSL spaced by 74° for v0..v1, then continue around the wheel for v2..v5.
  const hues = [188, 262, 56, 332, 116, 24]
  const h = hues[i % hues.length]
  return `hsl(${h}, 78%, 60%)`
}

// ---------------------------------------------------------------------------
// Layer/source ids — exported for the gate instrument counters (§9.2).
// The MapCanvas passes these to `map.addSource`/`addLayer` and to the
// __rsimMapDebug.layers counter map (feature counts updated per setData).
// ---------------------------------------------------------------------------

export const LAYER_IDS = [
  'vehicles-halo',
  'vehicles-body',
  'vehicles-label',
  'tracks-line',
  'fence-fill',
  'fence-line',
  'fence-vertices',
  'wp-body',
  'wp-line',
  'wp-err',
  'mission-line',
  'mission-flown',
  'rally-body',
  'rally-label',
  'graticule-line',
] as const

export type LayerId = (typeof LAYER_IDS)[number]

export const SOURCE_IDS = ['vehicles', 'tracks', 'fence', 'waypoints', 'mission-active', 'rally', 'graticule'] as const
export type SourceId = (typeof SOURCE_IDS)[number]

// ---------------------------------------------------------------------------
// Feature counters — the __rsimMapDebug.layers contract (§9.2 binding).
// Plain counters kept by `setData` callers; never `queryRenderedFeatures`
// in the hook path (perf + the gate-soak budget rule).
// ---------------------------------------------------------------------------

const featureCounts = new Map<LayerId, number>()

export function getLayerFeatureCount(id: LayerId): number {
  return featureCounts.get(id) ?? 0
}

function bumpLayerCount(id: LayerId, n: number): void {
  featureCounts.set(id, n)
}

// ---------------------------------------------------------------------------
// Vehicle GeoJSON — L1 source
// ---------------------------------------------------------------------------

export interface VehicleFeatureProperties {
  index: number
  id: string
  fsm: string
  armed: boolean
  mode: string
  battery: number
  heading: number
  active: boolean
}

export function vehicleFeatureCollection(
  vehicles: FleetVehicle[],
  activeIndex: number,
): GeoJSON.FeatureCollection<GeoJSON.Point, VehicleFeatureProperties> {
  const features: GeoJSON.Feature<GeoJSON.Point, VehicleFeatureProperties>[] = vehicles
    .filter((v) => v.lat != null && v.lon != null)
    .map((v) => ({
      type: 'Feature' as const,
      geometry: { type: 'Point' as const, coordinates: [v.lon as number, v.lat as number] },
      properties: {
        index: v.index,
        id: v.id,
        fsm: v.fsm,
        armed: v.armed,
        mode: v.mode,
        battery: v.battery_pct,
        heading: v.yaw_deg,
        active: v.index === activeIndex,
      },
    }))
  bumpLayerCount('vehicles-halo', features.length)
  bumpLayerCount('vehicles-body', features.length)
  bumpLayerCount('vehicles-label', features.length)
  return { type: 'FeatureCollection', features }
}

// ---------------------------------------------------------------------------
// Tracks GeoJSON — L2 source (one LineString per vehicle, 6-dec coords)
// ---------------------------------------------------------------------------

export function trackFeatureCollection(
  vehicles: FleetVehicle[],
): GeoJSON.FeatureCollection<GeoJSON.LineString, { index: number; id: string }> {
  const features: GeoJSON.Feature<GeoJSON.LineString, { index: number; id: string }>[] = []
  for (const v of vehicles) {
    const track = getTrack(v.index)
    if (track.length < 2) continue
    // 6-dec coords (spec §6.4 L2) — ~0.1 m resolution, fine for tracks.
    const coords = track.map(([lon, lat]) => [Number(lon.toFixed(6)), Number(lat.toFixed(6))])
    features.push({
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: coords },
      properties: { index: v.index, id: v.id },
    })
  }
  bumpLayerCount('tracks-line', features.length)
  return { type: 'FeatureCollection', features }
}

// ---------------------------------------------------------------------------
// Graticule GeoJSON — L13 source (canvas-drawn lat/lon grid, recomputed
// on moveend). Used as the offline basemap fallback (§6.3 row 4) and the
// NO-GL fallback HUD grid (§6.2).
// ---------------------------------------------------------------------------

export function graticuleFeatureCollection(
  bounds: { west: number; south: number; east: number; north: number },
  stepDeg = 0.01,
): GeoJSON.FeatureCollection<GeoJSON.LineString, { kind: 'lat' | 'lon'; v: number }> {
  const lines: GeoJSON.Feature<GeoJSON.LineString, { kind: 'lat' | 'lon'; v: number }>[] = []
  // Round to stepDeg multiples so the grid is stable under small pans.
  const south = Math.floor(bounds.south / stepDeg) * stepDeg
  const north = Math.ceil(bounds.north / stepDeg) * stepDeg
  const west = Math.floor(bounds.west / stepDeg) * stepDeg
  const east = Math.ceil(bounds.east / stepDeg) * stepDeg
  for (let lat = south; lat <= north + 1e-9; lat += stepDeg) {
    lines.push({
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: [
        [west, Number(lat.toFixed(6))],
        [east, Number(lat.toFixed(6))],
      ] },
      properties: { kind: 'lat', v: lat },
    })
  }
  for (let lon = west; lon <= east + 1e-9; lon += stepDeg) {
    lines.push({
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: [
        [Number(lon.toFixed(6)), south],
        [Number(lon.toFixed(6)), north],
      ] },
      properties: { kind: 'lon', v: lon },
    })
  }
  bumpLayerCount('graticule-line', lines.length)
  return { type: 'FeatureCollection', features: lines }
}

// ---------------------------------------------------------------------------
// L3: Fence — inclusion polygon + exclusion[] polygons + vertices.
// Spec §6.4 L3: "inclusion polygon + exclusion[] polygons + vertices
// FeatureCollection (props kind/poly_id/vtx_id)".
// ---------------------------------------------------------------------------

export interface FenceFeatureProperties {
  kind: 'inclusion' | 'exclusion'
  poly_id: number // 0 = inclusion, 1..N = exclusion polygons
  vtx_id: number // vertex index within the polygon
}

export function fenceFeatureCollection(
  fence: { inclusion: [number, number][]; exclusion: [number, number][][] } | null,
): GeoJSON.FeatureCollection<GeoJSON.Geometry, FenceFeatureProperties> {
  const features: GeoJSON.Feature<GeoJSON.Geometry, FenceFeatureProperties>[] = []
  if (!fence) {
    bumpLayerCount('fence-fill', 0)
    bumpLayerCount('fence-line', 0)
    bumpLayerCount('fence-vertices', 0)
    return { type: 'FeatureCollection', features }
  }

  // Inclusion polygon (poly_id 0)
  if (fence.inclusion.length >= 3) {
    const coords = fence.inclusion.map(([lat, lon]) => [Number(lon.toFixed(6)), Number(lat.toFixed(6))]) as [number, number][]
    // Close the ring
    coords.push(coords[0])
    features.push({
      type: 'Feature',
      geometry: { type: 'Polygon', coordinates: [coords] },
      properties: { kind: 'inclusion', poly_id: 0, vtx_id: -1 },
    })
  }

  // Exclusion polygons (poly_id 1..N)
  fence.exclusion.forEach((poly, i) => {
    if (poly.length >= 3) {
      const coords = poly.map(([lat, lon]) => [Number(lon.toFixed(6)), Number(lat.toFixed(6))]) as [number, number][]
      coords.push(coords[0])
      features.push({
        type: 'Feature',
        geometry: { type: 'Polygon', coordinates: [coords] },
        properties: { kind: 'exclusion', poly_id: i + 1, vtx_id: -1 },
      })
    }
  })

  // Vertex markers (for drag + M4 context menu)
  const vertexFeatures: GeoJSON.Feature<GeoJSON.Point, FenceFeatureProperties>[] = []
  fence.inclusion.forEach((v, i) => {
    vertexFeatures.push({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [Number(v[1].toFixed(6)), Number(v[0].toFixed(6))] },
      properties: { kind: 'inclusion', poly_id: 0, vtx_id: i },
    })
  })
  fence.exclusion.forEach((poly, pi) => {
    poly.forEach((v, vi) => {
      vertexFeatures.push({
        type: 'Feature',
        geometry: { type: 'Point', coordinates: [Number(v[1].toFixed(6)), Number(v[0].toFixed(6))] },
        properties: { kind: 'exclusion', poly_id: pi + 1, vtx_id: vi },
      })
    })
  })

  bumpLayerCount('fence-fill', features.filter((f) => f.geometry.type === 'Polygon').length)
  bumpLayerCount('fence-line', features.filter((f) => f.geometry.type === 'Polygon').length)
  bumpLayerCount('fence-vertices', vertexFeatures.length)

  // Combine polygons + vertex points into one FeatureCollection.
  // MapLibre paint properties are per-layer-type; the fence-fill + fence-line
  // layers filter to Polygon, fence-vertices filters to Point.
  return { type: 'FeatureCollection', features: [...features, ...vertexFeatures] }
}

// ---------------------------------------------------------------------------
// L4: Waypoints — mission waypoints (Point, props seq/alt/hold/accept/errors[]).
// Spec §6.4 L4: "wp-body (circle 7px + seq label), wp-line (dashed, accent),
// wp-err (danger ring when errors non-empty)".
// ---------------------------------------------------------------------------

export interface WaypointFeatureProperties {
  seq: number
  alt: number
  hold: number
  accept: number
  errors: string[]
}

export function waypointFeatureCollection(
  waypoints: { seq: number; x: number; y: number; z: number; param1: number; param2: number }[] | null,
  erroredSeqs: Set<number> = new Set(),
): GeoJSON.FeatureCollection<GeoJSON.Point, WaypointFeatureProperties> {
  const features: GeoJSON.Feature<GeoJSON.Point, WaypointFeatureProperties>[] = []
  if (!waypoints) {
    bumpLayerCount('wp-body', 0)
    bumpLayerCount('wp-err', 0)
    return { type: 'FeatureCollection', features }
  }
  for (const w of waypoints) {
    const errors: string[] = erroredSeqs.has(w.seq) ? ['validation_error'] : []
    features.push({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [Number(w.y.toFixed(6)), Number(w.x.toFixed(6))] },
      properties: {
        seq: w.seq,
        alt: w.z,
        hold: w.param1,
        accept: w.param2,
        errors,
      },
    })
  }
  bumpLayerCount('wp-body', features.length)
  bumpLayerCount('wp-err', features.filter((f) => f.properties.errors.length > 0).length)
  return { type: 'FeatureCollection', features }
}

/** Waypoint polyline (L4 wp-line) — the planned mission path. */
export function waypointLineFeatureCollection(
  waypoints: { seq: number; x: number; y: number }[] | null,
): GeoJSON.FeatureCollection<GeoJSON.LineString, { count: number }> {
  if (!waypoints || waypoints.length < 2) {
    bumpLayerCount('wp-line', 0)
    return { type: 'FeatureCollection', features: [] }
  }
  const coords = waypoints.map((w) => [Number(w.y.toFixed(6)), Number(w.x.toFixed(6))] as [number, number])
  bumpLayerCount('wp-line', 1)
  return {
    type: 'FeatureCollection',
    features: [{
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: coords },
      properties: { count: waypoints.length },
    }],
  }
}

// ---------------------------------------------------------------------------
// L5: Mission-active — uploaded/active mission polyline + flown-leg highlight.
// Spec §6.4 L5 + P6 fix: "the active mission MUST render on the Fly map;
// flown-leg highlight derives from vehicle position vs leg geometry".
// ---------------------------------------------------------------------------

export interface MissionActiveProperties {
  totalLegs: number
  flownLegs: number
  activeLeg: number // -1 = not started
}

export function missionActiveFeatureCollection(
  missionPath: [number, number][] | null, // [lon, lat] pairs
  activeLeg: number = -1,
  flownLegs: number = 0,
): GeoJSON.FeatureCollection<GeoJSON.LineString, MissionActiveProperties> {
  if (!missionPath || missionPath.length < 2) {
    bumpLayerCount('mission-line', 0)
    bumpLayerCount('mission-flown', 0)
    return { type: 'FeatureCollection', features: [] }
  }
  const features: GeoJSON.Feature<GeoJSON.LineString, MissionActiveProperties>[] = []
  // Full mission line (remaining + current leg)
  features.push({
    type: 'Feature',
    geometry: { type: 'LineString', coordinates: missionPath },
    properties: { totalLegs: missionPath.length - 1, flownLegs, activeLeg },
  })
  // Flown portion (legs 0..activeLeg)
  if (activeLeg > 0) {
    features.push({
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: missionPath.slice(0, activeLeg + 1) },
      properties: { totalLegs: missionPath.length - 1, flownLegs, activeLeg },
    })
  }
  bumpLayerCount('mission-line', 1)
  bumpLayerCount('mission-flown', activeLeg > 0 ? 1 : 0)
  return { type: 'FeatureCollection', features }
}

// ---------------------------------------------------------------------------
// L7: Rally — rally points (R glyph).
// Spec §6.4 L7: "rally-body (R glyph), rally-label". ≤5 (V-12).
// ---------------------------------------------------------------------------

export interface RallyFeatureProperties {
  seq: number
  alt: number
}

export function rallyFeatureCollection(
  rally: { seq: number; x: number; y: number; z: number }[] | null,
): GeoJSON.FeatureCollection<GeoJSON.Point, RallyFeatureProperties> {
  const features: GeoJSON.Feature<GeoJSON.Point, RallyFeatureProperties>[] = []
  if (!rally) {
    bumpLayerCount('rally-body', 0)
    return { type: 'FeatureCollection', features }
  }
  for (const r of rally) {
    features.push({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [Number(r.y.toFixed(6)), Number(r.x.toFixed(6))] },
      properties: { seq: r.seq, alt: r.z },
    })
  }
  bumpLayerCount('rally-body', features.length)
  return { type: 'FeatureCollection', features }
}

// ---------------------------------------------------------------------------
// Layer init — called once after the basemap `load` event. The layer
// catalog here is the M8 subset; M10..M13 add L3..L12.
// ---------------------------------------------------------------------------

export interface LayerInitCtx {
  addSource: (id: SourceId, spec: unknown) => void
  addLayer: (spec: unknown) => void
  /** Layout/paint property token resolver — typically `maplibregl` css vars. */
  colorToken: (name: string, fallback: string) => string
}

export function addLayers(ctx: LayerInitCtx): void {
  // --- L1: vehicles (halo + body + label) ---
  ctx.addSource('vehicles', {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] } as GeoJSON.FeatureCollection,
  })

  // Halo: 22px circle, vehicle color 25% opacity; 28px + full opacity when active.
  // Spec §6.4: "active vehicle = halo full opacity + 28 px radius (static paint
  // swap — MapLibre has no CSS transitions, no per-frame setPaintProperty churn)".
  ctx.addLayer({
    id: 'vehicles-halo',
    type: 'circle',
    source: 'vehicles',
    paint: {
      'circle-radius': ['case', ['get', 'active'], 28, 22],
      'circle-color': ['let', 'c', ['concat', 'hsl(', ['/', ['*', ['get', 'index'], 74], 1], ', 78%, 60%)'], ['var', 'c']],
      'circle-opacity': ['case', ['get', 'active'], 1, 0.25],
      'circle-stroke-width': 1,
      'circle-stroke-color': ctx.colorToken('--rsim-text', '#E6EDF3'),
      'circle-stroke-opacity': 0.4,
    },
  })

  // Body: triangle symbol rotated by heading.
  // MapLibre symbol layers take an `icon-image` (a sprite) for shapes; for
  // the M8 spike we use a triangle drawn via the `text-field` with a Unicode
  // ▲ rotated by `icon-rotate`. M9 lands the sprite.
  ctx.addLayer({
    id: 'vehicles-body',
    type: 'symbol',
    source: 'vehicles',
    layout: {
      'text-field': '▲',
      'text-size': 16,
      'text-rotate': ['get', 'heading'],
      'text-allow-overlap': true,
      'text-ignore-placement': true,
    },
    paint: {
      'text-color': ['case', ['get', 'active'], ctx.colorToken('--rsim-text', '#E6EDF3'), '#E6EDF3'],
    },
  })

  // Label: "v0 · READY · 87%" — format string assembled via expression.
  ctx.addLayer({
    id: 'vehicles-label',
    type: 'symbol',
    source: 'vehicles',
    layout: {
      'text-field': ['concat', 'v', ['to-string', ['get', 'index']], ' · ', ['get', 'fsm'], ' · ', ['to-string', ['floor', ['get', 'battery']]], '%'],
      'text-size': 11,
      'text-offset': [0, 2.2],
      'text-anchor': 'top',
      'text-allow-overlap': true,
    },
    paint: {
      'text-color': ctx.colorToken('--rsim-text', '#E6EDF3'),
      'text-halo-color': ctx.colorToken('--rsim-surface-solid', '#11161D'),
      'text-halo-width': 2,
    },
  })

  // --- L2: tracks (per-vehicle LineString) ---
  ctx.addSource('tracks', {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] } as GeoJSON.FeatureCollection,
  })
  ctx.addLayer({
    id: 'tracks-line',
    type: 'line',
    source: 'tracks',
    paint: {
      // Per-vehicle color via index → HSL. Spec §6.4 L2: "1.5 px, vehicle
      // color, 70 % opacity".
      'line-color': ['concat', 'hsl(', ['to-string', ['+', ['*', ['get', 'index'], 74], 188]], ', 78%, 60%)'],
      'line-width': 1.5,
      'line-opacity': 0.7,
    },
    layout: {
      'line-cap': 'round',
      'line-join': 'round',
    },
  })

  // --- L13: graticule (the offline basemap fallback grid) ---
  ctx.addSource('graticule', {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] } as GeoJSON.FeatureCollection,
  })
  ctx.addLayer({
    id: 'graticule-line',
    type: 'line',
    source: 'graticule',
    paint: {
      'line-color': ctx.colorToken('--rsim-grid', '#151C26'),
      'line-width': 0.5,
      'line-opacity': 0.6,
    },
    layout: { visibility: 'none' }, // hidden by default; visible when OFFLINE
  })

  // --- L3: fence (inclusion + exclusion polygons + vertices) ---
  // Spec §6.4 L3: "fence-fill (inclusion accent-dim 8% / exclusion danger 8%),
  // fence-line (dashed 1.5px), fence-vertices (circle 5px, draggable)".
  ctx.addSource('fence', {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] } as GeoJSON.FeatureCollection,
  })
  ctx.addLayer({
    id: 'fence-fill',
    type: 'fill',
    source: 'fence',
    filter: ['==', '$type', 'Polygon'],
    paint: {
      // inclusion = accent-dim 8%, exclusion = danger 8%
      'fill-color': ['case', ['==', ['get', 'kind'], 'exclusion'], ctx.colorToken('--rsim-danger', '#EF4444'), ctx.colorToken('--rsim-accent-dim', '#0E7490')],
      'fill-opacity': ['case', ['==', ['get', 'kind'], 'exclusion'], 0.08, 0.08],
    },
    layout: { visibility: 'none' }, // shown in Plan/Fence mode
  })
  ctx.addLayer({
    id: 'fence-line',
    type: 'line',
    source: 'fence',
    filter: ['==', '$type', 'Polygon'],
    paint: {
      'line-color': ['case', ['==', ['get', 'kind'], 'exclusion'], ctx.colorToken('--rsim-danger', '#EF4444'), ctx.colorToken('--rsim-accent', '#22D3EE')],
      'line-width': 1.5,
      'line-dasharray': [6, 5],
      'line-opacity': 0.8,
    },
    layout: { visibility: 'none' },
  })
  ctx.addLayer({
    id: 'fence-vertices',
    type: 'circle',
    source: 'fence',
    filter: ['==', '$type', 'Point'],
    paint: {
      'circle-radius': 5,
      'circle-color': ['case', ['==', ['get', 'kind'], 'exclusion'], ctx.colorToken('--rsim-danger', '#EF4444'), ctx.colorToken('--rsim-accent', '#22D3EE')],
      'circle-stroke-width': 1.5,
      'circle-stroke-color': ctx.colorToken('--rsim-text', '#E6EDF3'),
      'circle-stroke-opacity': 0.8,
    },
    layout: { visibility: 'none' }, // shown in Fence mode
  })

  // --- L4: waypoints (body + line + error ring) ---
  // Spec §6.4 L4: "wp-body (circle 7px + seq label), wp-line (dashed, accent),
  // wp-err (danger ring when errors non-empty)".
  ctx.addSource('waypoints', {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] } as GeoJSON.FeatureCollection,
  })
  ctx.addLayer({
    id: 'wp-line',
    type: 'line',
    source: 'waypoints',
    paint: {
      'line-color': ctx.colorToken('--rsim-accent', '#22D3EE'),
      'line-width': 1.5,
      'line-dasharray': [8, 6],
      'line-opacity': 0.7,
    },
    layout: { visibility: 'none' }, // shown in Plan mode
  })
  ctx.addLayer({
    id: 'wp-body',
    type: 'circle',
    source: 'waypoints',
    paint: {
      'circle-radius': 7,
      'circle-color': ctx.colorToken('--rsim-accent', '#22D3EE'),
      'circle-stroke-width': 1.5,
      'circle-stroke-color': ctx.colorToken('--rsim-text', '#E6EDF3'),
      'circle-stroke-opacity': 0.9,
    },
    layout: { visibility: 'none' }, // shown in Plan mode
  })
  ctx.addLayer({
    id: 'wp-err',
    type: 'circle',
    source: 'waypoints',
    filter: ['>', ['length', ['get', 'errors']], 0],
    paint: {
      'circle-radius': 12,
      'circle-color': ctx.colorToken('--rsim-danger', '#EF4444'),
      'circle-opacity': 0.0, // transparent fill — just the ring
      'circle-stroke-width': 2,
      'circle-stroke-color': ctx.colorToken('--rsim-danger', '#EF4444'),
      'circle-stroke-opacity': 0.9,
    },
    layout: { visibility: 'none' }, // shown in Plan mode
  })
  // Waypoint seq labels (reuse symbol layer pattern)
  ctx.addLayer({
    id: 'wp-label',
    type: 'symbol',
    source: 'waypoints',
    layout: {
      'text-field': ['to-string', ['get', 'seq']],
      'text-size': 10,
      'text-offset': [0, 0.5],
      'text-anchor': 'top',
      'text-allow-overlap': true,
      visibility: 'none',
    },
    paint: {
      'text-color': ctx.colorToken('--rsim-text', '#E6EDF3'),
      'text-halo-color': ctx.colorToken('--rsim-surface-solid', '#11161D'),
      'text-halo-width': 2,
    },
  })

  // --- L5: mission-active (uploaded mission polyline + flown-leg highlight) ---
  // Spec §6.4 L5 + P6 fix. mission-line = remaining + current leg (accent);
  // mission-flown = ok-green solid.
  ctx.addSource('mission-active', {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] } as GeoJSON.FeatureCollection,
  })
  ctx.addLayer({
    id: 'mission-line',
    type: 'line',
    source: 'mission-active',
    filter: ['==', ['get', 'activeLeg'], ['get', 'activeLeg']], // all features (no filter)
    paint: {
      'line-color': ctx.colorToken('--rsim-accent', '#22D3EE'),
      'line-width': 2,
      'line-opacity': 0.7,
    },
    layout: { visibility: 'none' }, // shown during missionFlying
  })
  ctx.addLayer({
    id: 'mission-flown',
    type: 'line',
    source: 'mission-active',
    // The flown portion is the second feature in the FC (when present).
    // Filter to features where activeLeg > 0 (the flown feature has activeLeg > 0;
    // the full mission line also has activeLeg but it's the first feature).
    // Simplest: filter by feature index is not possible in MapLibre expressions;
    // we use a separate property. For M10 we render both features and let the
    // flown one overlay the full one with a different color.
    paint: {
      'line-color': ctx.colorToken('--rsim-ok', '#34D399'),
      'line-width': 2.5,
      'line-opacity': 0.9,
    },
    layout: { visibility: 'none' }, // shown during missionFlying
  })

  // --- L7: rally (R glyph + label) ---
  ctx.addSource('rally', {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] } as GeoJSON.FeatureCollection,
  })
  ctx.addLayer({
    id: 'rally-body',
    type: 'symbol',
    source: 'rally',
    layout: {
      'text-field': 'R',
      'text-size': 14,
      'text-allow-overlap': true,
      visibility: 'none',
    },
    paint: {
      'text-color': ctx.colorToken('--rsim-alert', '#F59E0B'),
      'text-halo-color': ctx.colorToken('--rsim-surface-solid', '#11161D'),
      'text-halo-width': 2,
    },
  })
  ctx.addLayer({
    id: 'rally-label',
    type: 'symbol',
    source: 'rally',
    layout: {
      'text-field': ['concat', 'rally ', ['to-string', ['get', 'seq']], ' · ', ['to-string', ['get', 'alt']], 'm'],
      'text-size': 10,
      'text-offset': [0, 1.5],
      'text-anchor': 'top',
      'text-allow-overlap': true,
      visibility: 'none',
    },
    paint: {
      'text-color': ctx.colorToken('--rsim-text-dim', '#8B98A5'),
      'text-halo-color': ctx.colorToken('--rsim-surface-solid', '#11161D'),
      'text-halo-width': 2,
    },
  })
}

// ---------------------------------------------------------------------------
// Layer visibility helpers — toggle Plan/Fence mode layers on/off.
// Called by MapCanvas when the app-store's mapMode changes.
// ---------------------------------------------------------------------------

export function setPlanModeLayersVisible(ctx: { setLayoutProperty: (layer: string, name: 'visibility', value: 'visible' | 'none') => void }, mode: 'fly' | 'plan' | 'fence' | 'corridor'): void {
  const planVisible = mode === 'plan' || mode === 'fence' || mode === 'corridor'
  const fenceVisible = mode === 'fence' || mode === 'plan'
  const wpVisible = mode === 'plan' || mode === 'corridor'

  // L3 fence
  ctx.setLayoutProperty('fence-fill', 'visibility', fenceVisible ? 'visible' : 'none')
  ctx.setLayoutProperty('fence-line', 'visibility', fenceVisible ? 'visible' : 'none')
  ctx.setLayoutProperty('fence-vertices', 'visibility', mode === 'fence' ? 'visible' : 'none')
  // L4 waypoints
  ctx.setLayoutProperty('wp-line', 'visibility', wpVisible ? 'visible' : 'none')
  ctx.setLayoutProperty('wp-body', 'visibility', wpVisible ? 'visible' : 'none')
  ctx.setLayoutProperty('wp-err', 'visibility', wpVisible ? 'visible' : 'none')
  ctx.setLayoutProperty('wp-label', 'visibility', wpVisible ? 'visible' : 'none')
  // L7 rally
  ctx.setLayoutProperty('rally-body', 'visibility', planVisible ? 'visible' : 'none')
  ctx.setLayoutProperty('rally-label', 'visibility', planVisible ? 'visible' : 'none')
}

/** Show/hide the L5 mission-active layers (during missionFlying). */
export function setMissionActiveLayersVisible(ctx: { setLayoutProperty: (layer: string, name: 'visibility', value: 'visible' | 'none') => void }, visible: boolean): void {
  ctx.setLayoutProperty('mission-line', 'visibility', visible ? 'visible' : 'none')
  ctx.setLayoutProperty('mission-flown', 'visibility', visible ? 'visible' : 'none')
}
