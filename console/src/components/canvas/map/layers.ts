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
  'graticule-line',
] as const

export type LayerId = (typeof LAYER_IDS)[number]

export const SOURCE_IDS = ['vehicles', 'tracks', 'graticule'] as const
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
}
