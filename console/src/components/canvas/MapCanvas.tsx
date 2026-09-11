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
  setPlanModeLayersVisible,
  setMissionActiveLayersVisible,
  vehicleFeatureCollection,
  trackFeatureCollection,
  graticuleFeatureCollection,
  fenceFeatureCollection,
  waypointFeatureCollection,
  waypointLineFeatureCollection,
  missionActiveFeatureCollection,
  rallyFeatureCollection,
  taskFeatureCollection,
  getLayerFeatureCount,
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
import { useAppStore, getSnapshot as getAppStoreSnapshot, setGotoPending } from '@/state/app-store'
import {
  useMapSettings,
  getSnapshot as getMapSettingsSnapshot,
  getBasemap,
  setBasemap,
  cycleBasemap,
  LAYER_GROUP_MAP,
  type BasemapId,
} from '@/state/map-settings'
import { usePlanStore, getSnapshot as getPlanStoreSnapshot, getErroredSeqs, addWaypoint, addFenceVertex, addExclusionVertex, moveFenceVertex, moveWaypoint } from '@/state/plan-store'
import { DEFAULT_ORIGIN } from '@/lib/geo'

// Worker URL — set once at module load (§6.2).
//
// The MapLibre v6 worker is an ESM module that must be served at a stable
// URL the browser can fetch. Under Turbopack + Next.js 16, the
// `new URL('maplibre-gl/dist/maplibre-gl-worker.mjs', import.meta.url)`
// pattern that works under webpack/rspack fails with "chunk path empty
// but not in a worker" because Turbopack intercepts the import.meta.url
// resolution and the chunk path comes through empty (MapLibre issue #8126).
//
// The fix: the prebuild script (T-A1, scripts/copy-maplibre-worker.mjs)
// copies both worker siblings into public/maplibre/ so Next.js serves them
// at /maplibre/maplibre-gl-worker.mjs. We resolve an ABSOLUTE URL here so
// the worker fetch isn't subject to relative-path interception.
//
// M-T3 (Tauri repurpose) — the worker URL derivation.
//
// `window.location.origin` is correct in ALL four runtime environments:
//
//   1. Next.js dev server (npm run dev on :3000):
//        origin = http://localhost:3000
//        worker URL = http://localhost:3000/maplibre/maplibre-gl-worker.mjs
//        Next.js serves public/ at the dev origin.
//
//   2. Web stack behind Caddy gateway (:81, XTransformPort):
//        origin = http://localhost:81
//        worker URL = http://localhost:81/maplibre/maplibre-gl-worker.mjs
//        Caddy proxies / to :3000 and serves public/ via the Next.js
//        standalone server (or static export server).
//
//   3. Tauri webview on Linux/Windows (useHttpsScheme: false, the default):
//        origin = http://tauri.localhost
//        worker URL = http://tauri.localhost/maplibre/maplibre-gl-worker.mjs
//        Tauri's asset protocol serves frontendDist (../console/out) at
//        this origin, so /maplibre/... resolves to the bundled worker.
//
//   4. Tauri webview on macOS (and Linux):
//        origin = tauri://localhost
//        worker URL = tauri://localhost/maplibre/maplibre-gl-worker.mjs
//        Same as above — the Tauri asset protocol serves at this origin.
//
// Why NOT use `new URL('/maplibre/...', import.meta.url)` (the spec's
// Appendix F.4 suggestion)?
//
//   Turbopack intercepts `new URL(literal_string, import.meta.url)` as an
//   asset import. With an ABSOLUTE path like '/maplibre/...' it fails:
//     "Module not found: Can't resolve '/maplibre/maplibre-gl-worker.mjs'
//      server relative imports are not implemented yet."
//   With a RELATIVE path like 'maplibre/...' it would try to bundle the
//   file as a chunk (duplicate of what copy-maplibre-worker.mjs already
//   did) and drop the maplibre-gl-shared.mjs sibling (the original
//   MapLibre #8126 bug). Both paths are worse than window.location.origin.
//
// `window.location.origin` is the simplest pattern that works everywhere
// without bundler-specific workarounds.
//
// M-T3 also adds a runtime probe: if the worker URL returns non-200 (e.g.
// a misconfigured Tauri build that doesn't bundle public/maplibre/), we
// log a clear error to the console so the operator sees it immediately
// instead of debugging a blank map.
if (typeof window !== 'undefined') {
  const workerUrl = new URL('/maplibre/maplibre-gl-worker.mjs', window.location.origin).href
  setWorkerUrl(workerUrl)

  // M-T3: runtime probe — async, non-blocking. If this fails, the map
  // will mount but never load a tile; the operator sees a clear console
  // error pointing at the misconfigured bundle.
  fetch(workerUrl, { method: 'HEAD', cache: 'no-store' })
    .then((res) => {
      if (!res.ok) {
        console.error(
          `[rsim] MapLibre worker URL "${workerUrl}" returned HTTP ${res.status}. ` +
          `The map will mount but tiles won't load. Check that public/maplibre/ ` +
          `is bundled into the static export (run \`npm run build\` in console/).`,
        )
      }
    })
    .catch(() => {
      // Network error — likely a Tauri webview origin issue. Don't log
      // here; the maplibre-gl library will log its own worker-load error.
    })
}

// ---------------------------------------------------------------------------
// Basemap catalog (§6.3) — keyless-first, offline-safe. The catalog lives in
// state/map-settings.ts so the Settings panel + the cycle-basemap shortcut
// share a single source of truth. The Settings panel exposes Street (dark),
// Street (light), Satellite, Hybrid, Terrain, Offline — the QGC/MP-class
// basemap options.
// ---------------------------------------------------------------------------

// Helper — returns the active basemap spec from the settings store.
function activeBasemap(): ReturnType<typeof getBasemap> {
  return getBasemap(getMapSettingsSnapshot().basemap)
}

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
  // §9.2 binding: layer feature counts kept by `layers.ts` on each `setData`
  // (never queryRenderedFeatures in the hook path). Expose via a getter Proxy
  // so the gate assertion `__rsimMapDebug.layers["vehicles-body"] >= 2` reads
  // the live counter, not a stale snapshot. `JSON.stringify` on the parent
  // `mapDebug` object accesses `.layers` via `get`, so the Proxy returns the
  // live count per-property without needing `ownKeys` (which would require the
  // LAYER_IDS constant from layers.ts and complicates the serialization path).
  layers: new Proxy({} as Record<LayerId, number>, {
    get: (_target, prop: string) => {
      return getLayerFeatureCount(prop as LayerId)
    },
  }),
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
  const basemapRetryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const noGlRef = useRef(false)
  const offlineRef = useRef(false)
  /** Last basemap id we applied to the map — used to detect Settings changes. */
  const appliedBasemapRef = useRef<BasemapId | null>(null)
  /** Applied layer-visibility mask — used to detect Settings changes. */
  const appliedLayersRef = useRef<Record<string, boolean>>({})

  // Subscribe to telemetry + app store + map settings + plan store — the
  // snapshot is what we draw.
  const snap = useTelemetrySnapshot()
  const app = useAppStore()
  const plan = usePlanStore()
  const mapSettings = useMapSettings()

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
        style: activeBasemap().style as maplibregl.StyleSpecification | string,
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

    // Expose the map instance for debugging + gate assertions (the M8 spike:
    // `__rsimMapDebug.layers` counters need a way to verify the source actually
    // has data; the `sourcedata` event is unreliable for the initial setData).
    if (typeof window !== 'undefined') {
      ;(window as unknown as { __rsimMap?: maplibregl.Map }).__rsimMap = map
    }

    map.on('load', () => {
      mapDebug.loadedAtMs = performance.now()
      mapDebug.styleName = activeBasemap().label
      appliedBasemapRef.current = getMapSettingsSnapshot().basemap

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

    // Style load error → fall to next basemap (the offline-safe ladder).
    map.on('styleimagemissing', () => {
      // no-op — handled by error event
    })
    map.on('error', (e) => {
      // MapLibre emits 'error' for tile/style-load failures. The error has
      // an `error` field with a `source` hint; we advance to the next
      // basemap when a style-source load fails (e.g. provider down).
      const source = (e as { sourceId?: string; source?: unknown }).sourceId
      if (source && !e.error?.message?.includes('404')) return // ignore per-tile 404s
      // Pick the next keyless basemap that isn't the one we just tried.
      const current = getMapSettingsSnapshot().basemap
      const order: BasemapId[] = ['street-dark', 'street-light', 'satellite', 'hybrid', 'terrain', 'offline']
      const idx = order.indexOf(current)
      // Try the next 4 basemaps in the ladder; if all fail we land on 'offline'.
      for (let i = 1; i <= order.length; i++) {
        const next = order[(idx + i) % order.length]
        if (next !== current) {
          setBasemap(next)
          break
        }
      }
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
        // graticuleFeatureCollection already calls bumpLayerCount —
        // __rsimMapDebug.layers['graticule-line'] reads live via the Proxy.
      }
    })

    // Manual pan detection — suspend follow for FOLLOW_SUSPEND_MS (§6.5).
    map.on('dragstart', () => {
      cameraRef.current = markManualPan(cameraRef.current, performance.now())
    })

    // M10: Plan-mode click handlers — §7.1 map grammar + §2.3 rule 1 (Plan-on-live-map).
    //   - In Plan/Fly mode (fly): left-click empty map → clear selection (or goto if pending)
    //   - In waypoint mode (plan): left-click → addWaypoint
    //   - In fence mode (fence): left-click → addFenceVertex; dblclick → closeFence
    //   - In corridor mode (corridor): left-click → addCorridorVertex; dblclick → finish
    // The app-store's gotoPending takes precedence (§7.1 goto flow).
    map.on('click', (e) => {
      const appState = getAppStoreSnapshot()
      const planState = getPlanStoreSnapshot()
      const lng = e.lngLat.lng
      const lat = e.lngLat.lat

      // Goto flow takes precedence — §7.1: G enters goto mode, click places target.
      if (appState.gotoPending) {
        setGotoPending(false)
        // Fire goto via the command bus (dynamic import to avoid circular dep).
        void import('@/state/command-bus').then(({ command }) => {
          void command('goto', appState.activeVehicle, { lat, lon: lng })
        })
        return
      }

      // Plan-mode editing
      if (appState.mapMode === 'plan') {
        addWaypoint(lat, lng)
      } else if (appState.mapMode === 'fence') {
        // If there's an in-progress exclusion polygon, append to it.
        const exclusionCount = planState.file.geofence.exclusion.length
        if (exclusionCount > 0 && planState.file.geofence.exclusion[exclusionCount - 1].length < 3) {
          addExclusionVertex(exclusionCount - 1, lat, lng)
        } else {
          addFenceVertex(lat, lng)
        }
      } else if (appState.mapMode === 'corridor') {
        // Corridor mode (M15 wires the full corridor tool; M10 just adds vertices).
        addWaypoint(lat, lng)
      }
    })

    // M10: dblclick closes the fence polygon (§7.1: "in Fence/Plan draw mode
    // dblclick closes the polygon — preventDefault suppresses the zoom").
    map.on('dblclick', (e) => {
      const appState = getAppStoreSnapshot()
      if (appState.mapMode === 'fence' || appState.mapMode === 'plan') {
        e.preventDefault()
        // Closing is conceptual — the L3 polygon layer renders when ≥ 3 vertices.
      }
    })

    // M10 follow-up: context menu (§7.2) — right-click on any map target.
    // §7.2: `map.on('contextmenu', e => { e.preventDefault(); const f =
    // map.queryRenderedFeatures(e.point, {layers:[…]}); openMenu(e.point, f) })`.
    // Close on user camera gestures (dragstart, wheel, canvas mousedown
    // outside menu); programmatic easeTo (follow) never closes a menu.
    map.on('contextmenu', (e) => {
      e.preventDefault()
      const point = e.point
      const lng = e.lngLat.lng
      const lat = e.lngLat.lat
      // Query the rendered layers for a hit — vehicles, waypoints, fence
      // vertices, rally, mission line.
      const layers = ['vehicles-halo', 'vehicles-body', 'wp-body', 'fence-vertices', 'rally-body', 'mission-line']
      const features = map.queryRenderedFeatures(point, { layers })
      const hit = features[0]
      if (!hit) {
        // M1 — empty map
        void import('./map/context-menu').then(({ openMenu }) => openMenu({
          x: point.x, y: point.y, kind: 'empty', lng, lat,
        }))
        return
      }
      const props = hit.properties as Record<string, unknown>
      // Determine the menu kind from the layer id + props.
      if (hit.layer?.id === 'vehicles-halo' || hit.layer?.id === 'vehicles-body') {
        void import('./map/context-menu').then(({ openMenu }) => openMenu({
          x: point.x, y: point.y, kind: 'vehicle',
          props: { vehicleIndex: typeof props.index === 'number' ? props.index : undefined, vehicleId: typeof props.id === 'string' ? props.id : undefined },
          lng, lat,
        }))
      } else if (hit.layer?.id === 'wp-body') {
        void import('./map/context-menu').then(({ openMenu }) => openMenu({
          x: point.x, y: point.y, kind: 'waypoint',
          props: { seq: typeof props.seq === 'number' ? props.seq : undefined },
          lng, lat,
        }))
      } else if (hit.layer?.id === 'fence-vertices') {
        void import('./map/context-menu').then(({ openMenu }) => openMenu({
          x: point.x, y: point.y, kind: 'fence-vertex',
          props: {
            polyId: typeof props.poly_id === 'number' ? props.poly_id : undefined,
            vtxId: typeof props.vtx_id === 'number' ? props.vtx_id : undefined,
          },
          lng, lat,
        }))
      } else if (hit.layer?.id === 'rally-body') {
        void import('./map/context-menu').then(({ openMenu }) => openMenu({
          x: point.x, y: point.y, kind: 'rally',
          props: { seq: typeof props.seq === 'number' ? props.seq : undefined, alt: typeof props.alt === 'number' ? props.alt : undefined },
          lng, lat,
        }))
      } else if (hit.layer?.id === 'mission-line') {
        void import('./map/context-menu').then(({ openMenu }) => openMenu({
          x: point.x, y: point.y, kind: 'mission-leg', lng, lat,
        }))
      } else {
        void import('./map/context-menu').then(({ openMenu }) => openMenu({
          x: point.x, y: point.y, kind: 'empty', lng, lat,
        }))
      }
    })

    // Close the context menu on user camera gestures (§7.2 v1.1 close semantics).
    map.on('dragstart', () => { void import('./map/context-menu').then(({ closeMenu }) => closeMenu()) })
    map.on('wheel', () => { void import('./map/context-menu').then(({ closeMenu }) => closeMenu()) })

    // M10 follow-up: fence vertex + waypoint drag interaction (§7.1 — drag to
    // move; validation re-runs debounced 300ms). MapLibre v6 has no built-in
    // feature drag; we implement with mousedown on the layer + mousemove +
    // mouseup on the map canvas. The drag updates the plan store, which
    // triggers flushData to re-set the source.
    let dragTarget: { kind: 'waypoint' | 'fence-vertex'; seq?: number; polyId?: number; vtxId?: number } | null = null
    map.on('mousedown', 'wp-body', (e) => {
      const props = e.features?.[0]?.properties as Record<string, unknown> | undefined
      if (props && typeof props.seq === 'number') {
        dragTarget = { kind: 'waypoint', seq: props.seq }
        map.getCanvas().style.cursor = 'grabbing'
        e.preventDefault()
      }
    })
    map.on('mousedown', 'fence-vertices', (e) => {
      const props = e.features?.[0]?.properties as Record<string, unknown> | undefined
      if (props && typeof props.poly_id === 'number' && typeof props.vtx_id === 'number') {
        dragTarget = { kind: 'fence-vertex', polyId: props.poly_id, vtxId: props.vtx_id }
        map.getCanvas().style.cursor = 'grabbing'
        e.preventDefault()
      }
    })
    map.on('mousemove', (e) => {
      if (!dragTarget) return
      const lng = e.lngLat.lng
      const lat = e.lngLat.lat
      if (dragTarget.kind === 'waypoint' && dragTarget.seq != null) {
        moveWaypoint(dragTarget.seq, lat, lng)
      } else if (dragTarget.kind === 'fence-vertex' && dragTarget.polyId != null && dragTarget.vtxId != null) {
        moveFenceVertex(dragTarget.polyId, dragTarget.vtxId, lat, lng)
      }
    })
    const endDrag = (): void => {
      if (dragTarget) {
        dragTarget = null
        map.getCanvas().style.cursor = ''
      }
    }
    map.on('mouseup', endDrag)
    map.on('dragend', endDrag)

    // NOTE: the `sourcedata` handler was removed — the layer feature counts
    // are kept by `layers.ts:bumpLayerCount` on each `setData` (the spec
    // §9.2 binding: "feature counts are plain counters kept by layers.ts
    // on each setData, never queryRenderedFeatures in the hook path"). The
    // `__rsimMapDebug.layers` Proxy reads live from `getLayerFeatureCount`.

    // M15: camera verb listeners — dispatched by ShortcutsProvider (Z/X/C/N).
    const onCameraReset = (): void => {
      const fleetSnap = getFleetSnapshot()
      const bounds = fenceBounds(fleetSnap)
      if (bounds) {
        map.fitBounds([[bounds.west, bounds.south], [bounds.east, bounds.north]], fitBoundsOptions(bounds))
      }
      map.easeTo({ pitch: 0, bearing: 0 })
      cameraRef.current = resetCamera(cameraRef.current)
    }
    const onPitchGlance = (): void => {
      // X toggles pitch 0 ↔ 55 ("3D glance"). Mirror into the projection
      // setting so the Settings panel reflects the current state.
      const next = cameraRef.current.pitch === 0 ? 55 : 0
      map.easeTo({ pitch: next })
      cameraRef.current = togglePitchGlance(cameraRef.current)
      // Sync the projection setting (3d-pitch ↔ 2d) — fire-and-forget.
      void import('@/state/map-settings').then(({ getSnapshot, setProjection }) => {
        const want = next === 0 ? '2d' : '3d-pitch'
        if (getSnapshot().projection !== want) setProjection(want)
      })
    }
    const onRecenter = (): void => {
      const fleetSnap = getFleetSnapshot()
      const active = fleetSnap?.vehicles.find((v) => v.index === getAppStoreSnapshot().activeVehicle) ?? fleetSnap?.vehicles[0]
      if (active && active.lat != null && active.lon != null) {
        map.easeTo({ center: [active.lon, active.lat], zoom: 16 })
      }
    }
    const onCycleBasemap = (): void => { cycleBasemap() }
    window.addEventListener('rsim:camera-reset', onCameraReset)
    window.addEventListener('rsim:camera-pitch-glance', onPitchGlance)
    window.addEventListener('rsim:camera-recenter', onRecenter)
    window.addEventListener('rsim:cycle-basemap', onCycleBasemap)

    return () => {
      window.removeEventListener('rsim:camera-reset', onCameraReset)
      window.removeEventListener('rsim:camera-pitch-glance', onPitchGlance)
      window.removeEventListener('rsim:camera-recenter', onRecenter)
      window.removeEventListener('rsim:cycle-basemap', onCycleBasemap)
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
  // 2. Apply map-settings changes (basemap / orientation / projection /
  //    layer visibility). Subscribes to the map-settings store; on change,
  //    re-styles the map + re-applies overlay layers + sets visibility.
  // ---------------------------------------------------------------------------
  useEffect(() => {
    const map = mapRef.current
    if (!map) return
    const settings = getMapSettingsSnapshot()
    const spec = getBasemap(settings.basemap)
    // Basemap change — re-style + re-add overlays.
    if (appliedBasemapRef.current !== settings.basemap) {
      mapDebug.styleName = spec.label
      offlineRef.current = settings.basemap === 'offline'
      appliedBasemapRef.current = settings.basemap
      // setStyle returns `this` (sync); the new style loads async, so we
      // listen for `style.load` once to re-add our overlay layers.
      map.setStyle(spec.style as maplibregl.StyleSpecification | string, { diff: false })
      map.once('style.load', () => {
        addLayers({
          addSource: (id, s) => map.addSource(id, s as maplibregl.SourceSpecification),
          addLayer: (l) => map.addLayer(l as maplibregl.LayerSpecification),
          colorToken: (name, fallback) => readColorToken(name, fallback),
        })
        if (offlineRef.current) {
          // Show the graticule as the offline fallback grid + schedule a
          // 60s retry back to the operator's chosen basemap.
          try { map.setLayoutProperty('graticule-line', 'visibility', 'visible') } catch { /* style not loaded */ }
          if (basemapRetryTimerRef.current) clearTimeout(basemapRetryTimerRef.current)
          basemapRetryTimerRef.current = setTimeout(() => {
            // After 60s offline, retry the operator's originally-selected
            // basemap (the one they picked in Settings).
            const current = getMapSettingsSnapshot().basemap
            if (current === 'offline') {
              setBasemap('street-dark')
            }
          }, 60000)
        }
        // Re-apply layer visibility (the layers were just re-added —
        // default visibility is 'visible'; we need to hide disabled ones).
        applyLayerVisibility(map, settings)
        flushData()
      })
      return
    }
    // Orientation + projection — apply via easeTo (no re-style needed).
    if (settings.projection === '3d-pitch') {
      if (cameraRef.current.pitch === 0) {
        map.easeTo({ pitch: 55 })
        cameraRef.current = { ...cameraRef.current, pitch: 55 }
      }
    } else if (settings.projection === '2d') {
      if (cameraRef.current.pitch !== 0) {
        map.easeTo({ pitch: 0 })
        cameraRef.current = { ...cameraRef.current, pitch: 0 }
      }
    }
    // Track-up — the bearing follow happens in flushData (we read the
    // active vehicle's heading and easeTo that bearing on each follow
    // cadence). North-up — reset bearing to 0 once, then flushData keeps
    // it at 0.
    if (settings.orientation === 'north-up' && cameraRef.current.bearing !== 0) {
      map.easeTo({ bearing: 0 })
      cameraRef.current = { ...cameraRef.current, bearing: 0 }
    }
    // Layer visibility — apply immediately (no re-style needed).
    applyLayerVisibility(map, settings)
  }, [mapSettings])

  // Apply per-layer visibility from the settings store.
  function applyLayerVisibility(map: maplibregl.Map, settings: ReturnType<typeof getMapSettingsSnapshot>): void {
    for (const group of Object.keys(settings.layers) as (keyof typeof settings.layers)[]) {
      const visible = settings.layers[group]
      const layerIds = LAYER_GROUP_MAP[group]
      for (const layerId of layerIds) {
        const before = appliedLayersRef.current[layerId]
        if (before === visible) continue
        try {
          map.setLayoutProperty(layerId, 'visibility', visible ? 'visible' : 'none')
          appliedLayersRef.current[layerId] = visible
        } catch {
          // Layer not in the style yet (e.g. mid-basemap-swap) — skip; will
          // retry on the next settings change or the next flushData.
        }
      }
    }
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

    // L1: vehicles + L2: tracks — always present (even with no fleet, the
    // empty FeatureCollection clears the layers).
    if (fleetSnap) {
      const vehiclesFc = vehicleFeatureCollection(fleetSnap.vehicles, app.activeVehicle)
      const vehiclesSrc = map.getSource('vehicles') as maplibregl.GeoJSONSource | undefined
      if (vehiclesSrc) vehiclesSrc.setData(vehiclesFc)

      const tracksFc = trackFeatureCollection(fleetSnap.vehicles)
      const tracksSrc = map.getSource('tracks') as maplibregl.GeoJSONSource | undefined
      if (tracksSrc) tracksSrc.setData(tracksFc)
    }

    // M10: L3 fence, L4 waypoints, L5 mission-active, L7 rally — flush from the plan store.
    const planState = getPlanStoreSnapshot()
    const erroredSeqs = getErroredSeqs()

    // L3: fence (inclusion + exclusion polygons + vertices)
    const fenceFc = fenceFeatureCollection(planState.file.geofence)
    const fenceSrc = map.getSource('fence') as maplibregl.GeoJSONSource | undefined
    if (fenceSrc) fenceSrc.setData(fenceFc)

    // L4: waypoints (body + line + error ring) — combine Points + LineString in one FC.
    const wpFc = waypointFeatureCollection(planState.file.waypoints, erroredSeqs)
    const wpLineFc = waypointLineFeatureCollection(planState.file.waypoints)
    const combinedWpFc: GeoJSON.FeatureCollection = {
      type: 'FeatureCollection',
      features: [...wpFc.features, ...wpLineFc.features],
    }
    const wpSrc = map.getSource('waypoints') as maplibregl.GeoJSONSource | undefined
    if (wpSrc) wpSrc.setData(combinedWpFc)

    // L5: mission-active — only when a mission is flying (P6 fix).
    const missionActive = fleetSnap?.vehicles.some((v) => v.fsm === 'ACTIVE' && v.armed) ?? false
    const missionSrc = map.getSource('mission-active') as maplibregl.GeoJSONSource | undefined
    if (missionActive && planState.file.waypoints.length >= 2) {
      const missionPath = planState.file.waypoints.map((w) => [Number(w.y.toFixed(6)), Number(w.x.toFixed(6))] as [number, number])
      const missionFc = missionActiveFeatureCollection(missionPath, -1, 0)
      if (missionSrc) missionSrc.setData(missionFc)
      try { setMissionActiveLayersVisible({ setLayoutProperty: (l, n, v) => map.setLayoutProperty(l, n, v) }, true) } catch { /* style not loaded */ }
    } else {
      if (missionSrc) missionSrc.setData({ type: 'FeatureCollection', features: [] })
      try { setMissionActiveLayersVisible({ setLayoutProperty: (l, n, v) => map.setLayoutProperty(l, n, v) }, false) } catch { /* style not loaded */ }
    }

    // L7: rally
    const rallyFc = rallyFeatureCollection(planState.file.rally)
    const rallySrc = map.getSource('rally') as maplibregl.GeoJSONSource | undefined
    if (rallySrc) rallySrc.setData(rallyFc)

    // L6: tasks — fleet task markers (diamond + label). Visible when Fleet C2 overlay is open.
    const tasksFc = taskFeatureCollection(fleetSnap?.tasks ?? null, fleetSnap?.geo_origin ?? null)
    const tasksSrc = map.getSource('tasks') as maplibregl.GeoJSONSource | undefined
    if (tasksSrc) tasksSrc.setData(tasksFc)

    // Plan-mode layer visibility — toggle L3/L4/L6/L7 based on app.mapMode + fleet overlay.
    try {
      setPlanModeLayersVisible({ setLayoutProperty: (l, n, v) => map.setLayoutProperty(l, n, v) }, app.mapMode, app.overlays.fleet)
    } catch {
      // Style not loaded yet — skip; will retry on next flush.
    }

    // Follow-mode easeTo (§6.5): ≥1 Hz, suspend on manual pan.
    if (fleetSnap) {
      const now = performance.now()
      const active = fleetSnap.vehicles.find((v) => v.index === app.activeVehicle) ?? fleetSnap.vehicles[0]
      if (shouldFollowEase(cameraRef.current, active ?? null, now)) {
        const target = followEaseTarget(active, cameraRef.current)
        // Track-up: rotate the bearing to follow the active vehicle's
        // heading (yaw_deg). The map's `bearing` is the rotation from
        // north; we negate the vehicle heading so the vehicle's nose
        // points up. Only applied when the operator chose 'track-up' in
        // the Settings panel.
        const settings = getMapSettingsSnapshot()
        if (settings.orientation === 'track-up' && active && typeof active.yaw_deg === 'number' && !Number.isNaN(active.yaw_deg)) {
          const desiredBearing = (active.yaw_deg + 360) % 360
          target.bearing = desiredBearing
          cameraRef.current = { ...cameraRef.current, bearing: desiredBearing }
        }
        map.easeTo({ ...target, duration: 800 })
        cameraRef.current = markFollowEase(cameraRef.current, active!.id, now)
      }
    }
  }

  // Flush data on every telemetry snapshot + plan store + mapMode change.
  useEffect(() => {
    flushData()

  }, [snap, plan, app.mapMode])

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
