'use client'

/**
 * GCS v2 Operations Canvas — map settings store.
 *
 * The map view options an operator expects from a QGC/MP-class GCS:
 *   - Basemap provider (street-dark / street-light / satellite / hybrid /
 *     terrain / offline) — QGC has Street/Satellite/Hybrid/Terrain
 *   - Map orientation (north-up / track-up) — QGC has both
 *   - Camera projection (2D / 3D-pitch) — QGC's "3D Pitch" view
 *   - Per-layer visibility toggles (vehicles, tracks, waypoints, geofence,
 *     rally, mission, graticule) — QGC's layer panel
 *
 * Persisted to localStorage `rsim.map.v1` so the operator's choices survive
 * reloads (QGC saves map settings in its ini; we mirror that here).
 *
 * Implementation: a `useSyncExternalStore` module store (matches the
 * app-store + telemetry-store pattern; no external state library).
 */

import { useSyncExternalStore } from 'react'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type BasemapId = 'street-dark' | 'street-light' | 'satellite' | 'hybrid' | 'terrain' | 'offline'

export type MapOrientation = 'north-up' | 'track-up'

export type MapProjection = '2d' | '3d-pitch'

/** Layer-group toggles. Each group toggles one or more MapLibre layer ids. */
export type LayerGroup =
  | 'vehicles'
  | 'tracks'
  | 'waypoints'
  | 'geofence'
  | 'rally'
  | 'mission'
  | 'graticule'

export interface MapSettings {
  basemap: BasemapId
  orientation: MapOrientation
  projection: MapProjection
  layers: Record<LayerGroup, boolean>
}

// ---------------------------------------------------------------------------
// Basemap catalog — free / keyless MapLibre styles (research: QGC has
// Street/Satellite/Hybrid/Terrain; MapLibre options cover all four with
// OpenFreeMap + Esri World Imagery + OpenTopoMap).
// ---------------------------------------------------------------------------

export interface BasemapSpec {
  id: BasemapId
  label: string
  /** MapLibre style URL or inline StyleSpecification. */
  style: string | object
  attribution: string
}

export const BASEMAPS: BasemapSpec[] = [
  {
    id: 'street-dark',
    label: 'Street (dark)',
    style: 'https://tiles.openfreemap.org/styles/dark',
    attribution: '© OpenFreeMap © OpenMapTiles · Data from OpenStreetMap',
  },
  {
    id: 'street-light',
    label: 'Street (light)',
    style: 'https://tiles.openfreemap.org/styles/positron',
    attribution: '© OpenFreeMap © OpenMapTiles · Data from OpenStreetMap',
  },
  {
    id: 'satellite',
    label: 'Satellite',
    style: {
      version: 8,
      sources: {
        esri: {
          type: 'raster' as const,
          tiles: ['https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}'],
          tileSize: 256,
          attribution: '© Esri, Maxar, Earthstar Geographics',
          maxzoom: 19,
        },
      },
      layers: [{ id: 'esri-sat', type: 'raster' as const, source: 'esri' }],
    },
    attribution: '© Esri, Maxar, Earthstar Geographics',
  },
  {
    id: 'hybrid',
    label: 'Hybrid (satellite + labels)',
    style: {
      version: 8,
      sources: {
        esri: {
          type: 'raster' as const,
          tiles: ['https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}'],
          tileSize: 256,
          maxzoom: 19,
        },
        esri_ref: {
          type: 'raster' as const,
          tiles: ['https://server.arcgisonline.com/ArcGIS/rest/services/Reference/World_Boundaries_and_Places/MapServer/tile/{z}/{y}/{x}'],
          tileSize: 256,
          maxzoom: 19,
        },
      },
      layers: [
        { id: 'esri-sat', type: 'raster' as const, source: 'esri' },
        { id: 'esri-ref', type: 'raster' as const, source: 'esri_ref' },
      ],
    },
    attribution: '© Esri, Maxar, Earthstar Geographics',
  },
  {
    id: 'terrain',
    label: 'Terrain (topographic)',
    style: {
      version: 8,
      sources: {
        otm: {
          type: 'raster' as const,
          tiles: ['https://a.tile.opentopomap.org/{z}/{x}/{y}.png', 'https://b.tile.opentopomap.org/{z}/{x}/{y}.png', 'https://c.tile.opentopomap.org/{z}/{x}/{y}.png'],
          tileSize: 256,
          attribution: '© OpenTopoMap (CC-BY-SA) · SRTM',
          maxzoom: 17,
        },
      },
      layers: [{ id: 'otm-tiles', type: 'raster' as const, source: 'otm' }],
    },
    attribution: '© OpenTopoMap (CC-BY-SA) · SRTM',
  },
  {
    id: 'offline',
    label: 'Offline (overlays only)',
    style: { version: 8, sources: {}, layers: [] },
    attribution: 'Offline — overlays only',
  },
]

export function getBasemap(id: BasemapId): BasemapSpec {
  return BASEMAPS.find((b) => b.id === id) ?? BASEMAPS[0]
}

// ---------------------------------------------------------------------------
// Layer group → MapLibre layer-id mapping (kept here so the settings store
// is the single source of truth).
// ---------------------------------------------------------------------------

export const LAYER_GROUP_MAP: Record<LayerGroup, readonly string[]> = {
  vehicles: ['vehicles-halo', 'vehicles-body', 'vehicles-label'],
  tracks: ['tracks-line'],
  waypoints: ['wp-body', 'wp-line', 'wp-err'],
  geofence: ['fence-fill', 'fence-line', 'fence-vertices'],
  rally: ['rally-body', 'rally-label'],
  mission: ['mission-line', 'mission-flown'],
  graticule: ['graticule-line'],
}

// ---------------------------------------------------------------------------
// localStorage persistence
// ---------------------------------------------------------------------------

const STORAGE_KEY = 'rsim.map.v1'

const DEFAULTS: MapSettings = {
  basemap: 'street-dark',
  orientation: 'north-up',
  projection: '2d',
  layers: {
    vehicles: true,
    tracks: true,
    waypoints: true,
    geofence: true,
    rally: true,
    mission: true,
    graticule: false,
  },
}

function loadSettings(): MapSettings {
  if (typeof window === 'undefined') return DEFAULTS
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return DEFAULTS
    const parsed = JSON.parse(raw) as Partial<MapSettings>
    return {
      basemap: parsed.basemap ?? DEFAULTS.basemap,
      orientation: parsed.orientation ?? DEFAULTS.orientation,
      projection: parsed.projection ?? DEFAULTS.projection,
      layers: { ...DEFAULTS.layers, ...(parsed.layers ?? {}) },
    }
  } catch {
    return DEFAULTS
  }
}

function saveSettings(s: MapSettings): void {
  if (typeof window === 'undefined') return
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(s))
  } catch {
    // localStorage unavailable (private mode) — non-fatal
  }
}

// ---------------------------------------------------------------------------
// Module-level store (useSyncExternalStore contract — matches app-store)
// ---------------------------------------------------------------------------

let state: MapSettings = loadSettings()
const listeners = new Set<() => void>()
let version = 0

function bump(): void {
  version++
  for (const fn of listeners) fn()
}

export function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

export function getSnapshot(): MapSettings {
  return state
}

export function getVersion(): number {
  return version
}

// ---------------------------------------------------------------------------
// Mutators
// ---------------------------------------------------------------------------

export function setBasemap(id: BasemapId): void {
  state = { ...state, basemap: id }
  saveSettings(state)
  bump()
}

export function setOrientation(o: MapOrientation): void {
  state = { ...state, orientation: o }
  saveSettings(state)
  bump()
}

export function setProjection(p: MapProjection): void {
  state = { ...state, projection: p }
  saveSettings(state)
  bump()
}

export function setLayerVisibility(group: LayerGroup, visible: boolean): void {
  state = { ...state, layers: { ...state.layers, [group]: visible } }
  saveSettings(state)
  bump()
}

/** Cycle to the next basemap in the catalog (the `N` shortcut). */
export function cycleBasemap(): void {
  const idx = BASEMAPS.findIndex((b) => b.id === state.basemap)
  const next = BASEMAPS[(idx + 1) % BASEMAPS.length]
  setBasemap(next.id)
}

// ---------------------------------------------------------------------------
// React hook
// ---------------------------------------------------------------------------

export function useMapSettings(): MapSettings {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}
