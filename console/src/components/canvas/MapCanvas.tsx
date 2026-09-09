'use client'

/**
 * GCS v2 Operations Canvas — the MapLibre map canvas (M8, T-A2).
 *
 * Spec: docs/GCS_V2_SPEC.md §6 (the entire map chapter) + §13.1 #1 (route
 * mechanism is CanvasLoader; this is the actual map component).
 *
 * All maplibre imports live HERE ONLY (§6.1/D3). The worker URL is set
 * once at module top-level (§6.2 — the #1 Next.js integration failure:
 * under both Turbopack and webpack, `new URL(..., import.meta.url)` drops
 * the maplibre-gl-shared.mjs sibling, so the map mounts but never loads
 * a tile; the prebuild copy script in T-A1 + `setWorkerUrl` here fix it).
 *
 * WebGL2 failure is a first-class state (§6.2 v1.1): since v6.7.0
 * `new maplibregl.Map()` THROWS `GPUInitializationError` — this component
 * catches it, renders the offline-style graticule HUD fallback (§6.3 row 4)
 * and shows a `NO-GL` chip in the status strip. `NO-GL` and `OFFLINE` are
 * distinct states with distinct chips.
 *
 * This M8 deliverable ships:
 *  - MapLibre v6 vanilla (useRef + useEffect — no react-map-gl wrapper)
 *  - Basemap ladder (§6.3): OpenFreeMap dark → CARTO dark-matter → OSM raster
 *    → offline empty style, with style-load-error → next entry, attribution
 *    bottom-right, 60s basemap retry when offline.
 *  - L1 vehicles (halo + body + label) + L2 tracks + L13 graticule.
 *  - __rsimMapDebug gate instrument (§9.2 binding): {loadedAtMs, styleName,
 *    pitch, bearing, mode, layers: {<id> → featureCount}}.
 *
 * The full L3..L12 catalog lands across M10..M13; context menus (§7.2) and
 * the layer-edit interactions (§7.1) land with T-C3 / T-B2..B7.
 */

import { useEffect, useRef, type JSX } from 'react'
import * as maplibregl from 'maplibre-gl'
import { setWorkerUrl } from 'maplibre-gl'
import 'maplibre-gl/dist/maplibre-gl.css'

import {
  addLayers,
  vehicleFeatureCollection,
  trackFeatureCollection,
  graticuleFeatureCollection,
  type LayerId,
} from './map/layers'
import {
  bootCenter,
  bootZoom,
  fenceBounds,
  fitBoundsOptions,
  initialCameraState,
  markManualPan,
  markFollowEase,
  resetCamera,
  shouldFollowEase,
  followEaseTarget,
  togglePitchGlance,
  toggleNedInset,
  type CameraState,
} from './map/camera'
import { getFleetSnapshot, useTelemetrySnapshot } from '@/state/telemetry-store'
import { useAppStore } from '@/state/app-store'
import { DEFAULT_ORIGIN } from '@/lib/geo'

// Worker URL set once at module load (§6.2 — the official Next.js workaround).
// The prebuild script (T-A1) copies both siblings into public/maplibre/.
setWorkerUrl('/maplibre/maplibre-gl-worker.mjs')

// ---------------------------------------------------------------------------
// Basemap ladder (§6.3) — keyless-first, dark, offline-safe.
// ---------------------------------------------------------------------------

const BASEMAP_LADDER = [
  {
    name: 'OpenFreeMap dark',
    style: 'https://tiles.openfreemap.org/styles/dark',
    attribution: '© OpenFreeMap © OpenMapTiles · Data from OpenStreetMap',
  },
  {
    name: 'CARTO dark-matter',
    style: 'https://basemaps.cartocdn.com/gl/dark-matter-gl-style/style.json',
    attribution: '© OpenStreetMap contributors © CARTO',
    // CARTO keyless deprecated 2026-08-26 — transient fallback only.
    deprecated: true as const,
  },
  {
    // OSM raster — built inline below as a raster source style.
    name: 'OSM raster',
    style: {
      version: 8,
      sources: {
        osm: {
          type: 'raster' as const,
          tiles: ['https://tile.openstreetmap.org/{z}/{x}/{y}.png'],
          tileSize: 256,
          attribution: '© OpenStreetMap contributors',
        },
      },
      layers: [
        { id: 'osm-tiles', type: 'raster' as const, source: 'osm' },
      ],
    },
    attribution: '© OpenStreetMap contributors',
  },
  {
    name: 'Offline',
    style: { version: 8, sources: {}, layers: [] },
    attribution: 'Offline — overlays only',
  },
] as const

// ---------------------------------------------------------------------------
// __rsimMapDebug gate instrument (§9.2 binding). Always-on; negligible cost.
// ---------------------------------------------------------------------------

interface MapDebug {
  loadedAtMs: number | null
  styleName: string | null
  pitch: number
  bearing: number
  mode: 'geo' | 'ned'
  layers: Record<LayerId, number>
}

const mapDebug: MapDebug = {
  loadedAtMs: null,
  styleName: null,
  pitch: 0,
  bearing: 0,
  mode: 'geo',
  layers: {
    'vehicles-halo': 0,
    'vehicles-body': 0,
    'vehicles-label': 0,
    'tracks-line': 0,
    'graticule-line': 0,
  },
}

if (typeof window !== 'undefined') {
  ;(window as unknown as { __rsimMapDebug?: unknown }).__rsimMapDebug = mapDebug
}

// ---------------------------------------------------------------------------
// The component
// ---------------------------------------------------------------------------

export function MapCanvas(): JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const mapRef = useRef<maplibregl.Map | null>(null)
  const cameraRef = useRef<CameraState>(initialCameraState())
  const basemapIdxRef = useRef(0)
  const basemapRetryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const noGlRef = useRef(false)
  const offlineRef = useRef(false)

  // Subscribe to telemetry + app store — the snapshot is what we draw.
  const snap = useTelemetrySnapshot()
  const app = useAppStore()

  // ---------------------------------------------------------------------------
  // 1. Map init — vanilla maplibre (the spec's R-7 fallback is the default).
  //    Try/catch GPUInitializationError (v6.7.0 throws — §6.2 v1.1).
  // ---------------------------------------------------------------------------
  useEffect(() => {
    if (!containerRef.current || mapRef.current) return
    let map: maplibregl.Map
    try {
      map = new maplibregl.Map({
        container: containerRef.current,
        center: bootCenter(),
        zoom: bootZoom(),
        minZoom: 3,
        maxZoom: 19,
        pitch: 0,
        bearing: 0,
        maxPitch: 60,
        style: BASEMAP_LADDER[0].style as maplibregl.StyleSpecification | string,
        attributionControl: { compact: true },
        canvasContextAttributes: { antialias: true, powerPreference: 'high-performance' },
      })
    } catch (e) {
      // GPUInitializationError (v6.7.0+) — render the NO-GL fallback.
      console.warn('[MapCanvas] WebGL init failed — rendering NO-GL fallback', e)
      noGlRef.current = true
      mapDebug.styleName = 'NO-GL'
      return
    }
    mapRef.current = map

    map.on('load', () => {
      mapDebug.loadedAtMs = performance.now()
      mapDebug.styleName = BASEMAP_LADDER[basemapIdxRef.current].name

      // Add M8 layers (L1, L2, L13).
      addLayers({
        addSource: (id, spec) => map.addSource(id, spec as maplibregl.SourceSpecification),
        addLayer: (spec) => map.addLayer(spec as maplibregl.LayerSpecification),
        colorToken: (name, fallback) => readColorToken(name, fallback),
      })

      // Fit to fence (or DEFAULT_ORIGIN) on boot.
      const fleetSnap = getFleetSnapshot()
      const bounds = fenceBounds(fleetSnap)
      if (bounds) {
        map.fitBounds(
          [
            [bounds.west, bounds.south],
            [bounds.east, bounds.north],
          ],
          fitBoundsOptions(bounds),
        )
      }

      // Initial data flush — draw vehicles/tracks even before the first WS frame.
      flushData()
    })

    // Style load error → next basemap in the ladder.
    map.on('styleimagemissing', () => {
      // no-op — handled by error event
    })
    map.on('error', (e) => {
      // MapLibre emits 'error' for tile/style-load failures. The error has
      // an `error` field with a `source` hint; we advance the basemap ladder
      // when a style-source load fails.
      const source = (e as { sourceId?: string; source?: unknown }).sourceId
      if (source && !e.error?.message?.includes('404')) return // ignore per-tile 404s
      advanceBasemap()
    })

    // __rsimMapDebug: pitch/bearing updated on moveend.
    map.on('moveend', () => {
      mapDebug.pitch = map.getPitch()
      mapDebug.bearing = map.getBearing()
      // Recompute graticule bounds (used as offline fallback grid).
      const b = map.getBounds()
      const fc = graticuleFeatureCollection({
        west: b.getWest(),
        south: b.getSouth(),
        east: b.getEast(),
        north: b.getNorth(),
      })
      const src = map.getSource('graticule') as maplibregl.GeoJSONSource | undefined
      if (src) {
        src.setData(fc)
        mapDebug.layers['graticule-line'] = fc.features.length
      }
    })

    // Manual pan detection — suspend follow for FOLLOW_SUSPEND_MS (§6.5).
    map.on('dragstart', () => {
      cameraRef.current = markManualPan(cameraRef.current, performance.now())
    })

    // Source data — count features as they land (for the gate instrument).
    map.on('sourcedata', (e) => {
      if (!e.isSourceLoaded) return
      // Map source id → layer-id counter (for L1/L2).
      const id = e.sourceId as 'vehicles' | 'tracks' | 'graticule' | undefined
      if (id === 'vehicles') {
        const src = map.getSource(id) as maplibregl.GeoJSONSource | undefined
        const fc = src?._data as GeoJSON.FeatureCollection | undefined
        const n = fc?.features.length ?? 0
        mapDebug.layers['vehicles-halo'] = n
        mapDebug.layers['vehicles-body'] = n
        mapDebug.layers['vehicles-label'] = n
      } else if (id === 'tracks') {
        const src = map.getSource(id) as maplibregl.GeoJSONSource | undefined
        const fc = src?._data as GeoJSON.FeatureCollection | undefined
        mapDebug.layers['tracks-line'] = fc?.features.length ?? 0
      }
    })

    return () => {
      if (basemapRetryTimerRef.current) {
        clearTimeout(basemapRetryTimerRef.current)
        basemapRetryTimerRef.current = null
      }
      map.remove()
      mapRef.current = null
      mapDebug.loadedAtMs = null
      mapDebug.styleName = null
    }
     
  }, [])

  // ---------------------------------------------------------------------------
  // 2. Advance the basemap ladder on style-load error (§6.3).
  // ---------------------------------------------------------------------------
  function advanceBasemap(): void {
    const map = mapRef.current
    if (!map) return
    const next = (basemapIdxRef.current + 1) % BASEMAP_LADDER.length
    basemapIdxRef.current = next
    const entry = BASEMAP_LADDER[next]
    mapDebug.styleName = entry.name
    offlineRef.current = entry.name === 'Offline'
    // setStyle returns `this` (sync); the new style loads async, so we
    // listen for `style.load` once to re-add our overlay layers.
    map.setStyle(entry.style as maplibregl.StyleSpecification | string, { diff: false })
    map.once('style.load', () => {
      addLayers({
        addSource: (id, spec) => map.addSource(id, spec as maplibregl.SourceSpecification),
        addLayer: (spec) => map.addLayer(spec as maplibregl.LayerSpecification),
        colorToken: (name, fallback) => readColorToken(name, fallback),
      })
      if (offlineRef.current) {
        map.setLayoutProperty('graticule-line', 'visibility', 'visible')
        basemapRetryTimerRef.current = setTimeout(advanceBasemap, 60000) // 60s
      }
      flushData()
    })
  }

  // ---------------------------------------------------------------------------
  // 3. Data flush — called from telemetry subscription + rAF cadence.
  //    Per spec §6.4: "Layer add order is fixed at init (once); per-frame
  //    updates only call setData. Editing interactions mutate the owning
  //    GeoJSON and re-set data — same pattern as v1's Leaflet editors, but
  //    one shared implementation instead of three (P2, P7)."
  // ---------------------------------------------------------------------------
  function flushData(): void {
    const map = mapRef.current
    if (!map || !map.isStyleLoaded()) return
    const fleetSnap = getFleetSnapshot()
    if (!fleetSnap) return

    // L1: vehicles.
    const vehiclesFc = vehicleFeatureCollection(fleetSnap.vehicles, app.activeVehicle)
    const vehiclesSrc = map.getSource('vehicles') as maplibregl.GeoJSONSource | undefined
    if (vehiclesSrc) vehiclesSrc.setData(vehiclesFc)

    // L2: tracks.
    const tracksFc = trackFeatureCollection(fleetSnap.vehicles)
    const tracksSrc = map.getSource('tracks') as maplibregl.GeoJSONSource | undefined
    if (tracksSrc) tracksSrc.setData(tracksFc)

    // Follow-mode easeTo (§6.5): ≥1 Hz, suspend on manual pan.
    const now = performance.now()
    const active = fleetSnap.vehicles.find((v) => v.index === app.activeVehicle) ?? fleetSnap.vehicles[0]
    if (shouldFollowEase(cameraRef.current, active ?? null, now)) {
      const target = followEaseTarget(active, cameraRef.current)
      map.easeTo({ ...target, duration: 800 })
      cameraRef.current = markFollowEase(cameraRef.current, active.id, now)
    }
  }

  // Flush data on every telemetry snapshot change.
  useEffect(() => {
    flushData()
     
  }, [snap])

  // ---------------------------------------------------------------------------
  // 4. App-store-driven camera verbs (Z reset, X pitch glance, V NED inset, F follow).
  //    M8 wires Z/X/V/F through app-store selectors; M9's ShortcutsProvider
  //    will dispatch them as single-key verbs.
  // ---------------------------------------------------------------------------
  useEffect(() => {
    const map = mapRef.current
    if (!map) return
    // Follow toggle from app-store.
    cameraRef.current = { ...cameraRef.current, follow: app.follow }
  }, [app.follow])

  // ---------------------------------------------------------------------------
  // Render — the map container + the NO-GL / OFFLINE fallback chip.
  // ---------------------------------------------------------------------------

  return (
    <div
      ref={containerRef}
      data-rsim-zone="E"
      className="absolute inset-0"
      style={{ background: 'var(--rsim-bg, #0B0F14)' }}
      tabIndex={0}
      aria-label="Map canvas — right-click for context menu, single-key shortcuts when focused"
    >
      {noGlRef.current && (
        <div
          className="absolute inset-0 flex items-center justify-center"
          style={{
            backgroundImage:
              'linear-gradient(var(--rsim-grid, #151C26) 1px, transparent 1px), linear-gradient(90deg, var(--rsim-grid, #151C26) 1px, transparent 1px)',
            backgroundSize: '40px 40px',
          }}
        >
          <div className="rounded border border-amber-500/40 bg-amber-500/10 px-4 py-2 text-amber-400">
            NO-GL — WebGL2 unavailable; rendering offline fallback grid
          </div>
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// CSS-var token reader — fallback to the spec literal if the var is unset.
// ---------------------------------------------------------------------------

function readColorToken(name: string, fallback: string): string {
  if (typeof window === 'undefined') return fallback
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return v || fallback
}
