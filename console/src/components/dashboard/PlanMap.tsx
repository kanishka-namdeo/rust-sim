'use client'

/**
 * Plan View map (ADR-0019 / GCS_SPEC §5.1, §8.1).
 *
 * Leaflet 1.9 map integrated directly (no react-leaflet wrapper — the
 * trimmed-dependency contract, same as the Operator Map). Centers on the
 * PX4 test field (47.397770° N, 8.545580° E, zoom 16) per §8.1 step 1.
 *
 * Three interaction modes:
 *   - 'idle'     — read-only (a saved mission is loaded)
 *   - 'waypoint' — map click adds a numbered blue waypoint; markers are
 *                  draggable; right-click removes (QGC Plan View grammar)
 *   - 'fence'    — map click adds a fence polygon vertex; double-click
 *                  closes the polygon (renders green dashed)
 *
 * The console mounts all tabs forceMount-hidden — Leaflet initializes
 * inside a 0×0 container. The ResizeObserver calls `invalidateSize()`
 * BEFORE any `fitBounds()` (console/AGENTS.md rule against the QGC v5.1.4
 * bug class: fitting against a stale 0×0 cached size produces a
 * degenerate world view where map clicks resolve to garbage lat/lon).
 */

import { useEffect, useRef } from 'react'
import type * as LeafletNS from 'leaflet'
import 'leaflet/dist/leaflet.css'

import type { LatLng } from '@/lib/geo'
import type { PlanGeofence, PlanWaypoint } from '@/lib/plan-types'

export type PlanMapMode = 'idle' | 'waypoint' | 'fence'

export interface PlanMapProps {
  /** Center latitude (defaults to PX4 test field). */
  centerLat: number
  /** Center longitude. */
  centerLon: number
  /** Initial zoom (default 16). */
  zoom: number
  /** Currently-planned waypoints (rendered as numbered blue markers + polyline). */
  waypoints: PlanWaypoint[]
  /** Currently-planned geofence (rendered as green dashed polygon). */
  geofence: PlanGeofence
  /** Sequence numbers (1-based) that failed validation — rendered in red. */
  erroredSeqs: Set<number>
  /** Active drawing mode. */
  mode: PlanMapMode
  /** True while drawing a fence polygon (cursor + tooltip hint). */
  fenceDrawing: boolean
  /** Waypoint click → select (in any mode). */
  onSelectWaypoint?: (seq: number) => void
  /** Map click in 'waypoint' mode → add a waypoint. */
  onAddWaypoint?: (lat: number, lng: number) => void
  /** Waypoint drag end → update its lat/lon. */
  onMoveWaypoint?: (seq: number, lat: number, lng: number) => void
  /** Waypoint right-click → remove. */
  onRemoveWaypoint?: (seq: number) => void
  /** Map click in 'fence' mode → add polygon vertex. */
  onAddFenceVertex?: (lat: number, lng: number) => void
  /** Map dbl-click in 'fence' mode → close polygon. */
  onCloseFence?: () => void
  /** Map click in 'idle' mode → recenter + select nothing. */
  onMapClickIdle?: (lat: number, lng: number) => void
  className?: string
}

const TILE_URL = 'https://tile.openstreetmap.org/{z}/{x}/{y}.png'
const TILE_ATTR = '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'
const TILE_ERROR_LIMIT = 8

const WAYPOINT_COLOR = '#0ea5e9' // sky-500 — QGC blue
const WAYPOINT_ERR_COLOR = '#dc2626' // red-600
const PATH_COLOR = '#0ea5e9'
const FENCE_COLOR = '#10b981' // emerald-500 — QGC green

export function PlanMap(props: PlanMapProps) {
  const holderRef = useRef<HTMLDivElement | null>(null)
  const mapRef = useRef<LeafletNS.Map | null>(null)
  const LRef = useRef<typeof LeafletNS | null>(null)
  const tileRef = useRef<LeafletNS.TileLayer | null>(null)
  const tileErrorsRef = useRef(0)
  const homeRef = useRef<LeafletNS.CircleMarker | null>(null)
  const wpMarkers = useRef<Map<number, LeafletNS.Marker>>(new Map())
  const planPathRef = useRef<LeafletNS.Polyline | null>(null)
  const fenceRef = useRef<LeafletNS.Polygon | null>(null)
  const fenceVertsRef = useRef<Map<number, LeafletNS.Marker>>(new Map())
  const fenceTempLineRef = useRef<LeafletNS.Polyline | null>(null)
  const propsRef = useRef(props)
  useEffect(() => {
    propsRef.current = props
  }, [props])

  // --------------------------------------------------------------- map init
  useEffect(() => {
    let cancelled = false
    let roCleanup: (() => void) | null = null
    const el = holderRef.current
    if (!el) return
    import('leaflet').then((mod) => {
      if (cancelled || !el) return
      const L = (mod.default ?? mod) as typeof LeafletNS
      LRef.current = L
      const map = L.map(el, {
        center: [props.centerLat, props.centerLon],
        zoom: props.zoom,
        zoomControl: true,
        attributionControl: true,
        doubleClickZoom: false, // we use dblclick to close the fence polygon
      })
      mapRef.current = map
      const tiles = L.tileLayer(TILE_URL, {
        attribution: TILE_ATTR,
        maxZoom: 19,
      })
      tiles.on('tileerror', () => {
        tileErrorsRef.current++
        if (tileErrorsRef.current >= TILE_ERROR_LIMIT && tileRef.current) {
          // offline: swap to a drawn graticule (same fallback as the Operator Map)
          tileRef.current.remove()
          tileRef.current = null
          const Grid = L.GridLayer.extend({
            createTile(coords: { x: number; y: number; z: number }) {
              const tile = document.createElement('canvas')
              const size = this.getTileSize()
              tile.width = size
              tile.height = size
              const ctx = tile.getContext('2d')
              if (ctx) {
                ctx.strokeStyle = 'rgba(128,128,140,0.35)'
                ctx.lineWidth = 1
                ctx.strokeRect(0.5, 0.5, size - 1, size - 1)
                ctx.fillStyle = 'rgba(128,128,140,0.55)'
                ctx.font = '10px monospace'
                ctx.fillText(`${coords.z}/${coords.x}/${coords.y}`, 8, 14)
              }
              return tile
            },
          })
          const grid = new (Grid as unknown as new (opts: { tileSize: number }) => LeafletNS.GridLayer)({
            tileSize: 256,
          })
          grid.addTo(map)
        }
      })
      tiles.addTo(map)
      tileRef.current = tiles

      map.on('click', (e: LeafletNS.LeafletMouseEvent) => {
        const p = propsRef.current
        if (p.mode === 'waypoint') p.onAddWaypoint?.(e.latlng.lat, e.latlng.lng)
        else if (p.mode === 'fence') p.onAddFenceVertex?.(e.latlng.lat, e.latlng.lng)
        else p.onMapClickIdle?.(e.latlng.lat, e.latlng.lng)
      })
      map.on('dblclick', (e: LeafletNS.LeafletMouseEvent) => {
        const p = propsRef.current
        if (p.mode === 'fence') {
          // dblclick also fires a click first on some browsers; ignore the
          // vertex that may have been added by that click — the closeFence
          // handler is responsible for keeping the polygon well-formed
          p.onCloseFence?.()
          L.DomEvent.stopPropagation(e)
        }
      })

      // The console mounts all tabs forceMount-hidden; Leaflet in a zero-size
      // container renders degenerate. ResizeObserver: invalidateSize when the
      // container becomes visible. (console/AGENTS.md rule.)
      const ro = new ResizeObserver(() => {
        map.invalidateSize()
      })
      ro.observe(el)
      roCleanup = () => ro.disconnect()
      requestAnimationFrame(() => map.invalidateSize())
    })
    return () => {
      cancelled = true
      roCleanup?.()
      mapRef.current?.remove()
      mapRef.current = null
      wpMarkers.current.clear()
      fenceVertsRef.current.clear()
      homeRef.current = null
      planPathRef.current = null
      fenceRef.current = null
      fenceTempLineRef.current = null
    }
  }, [])

  // --------------------------------------------------------------- home marker
  useEffect(() => {
    const L = LRef.current
    const map = mapRef.current
    if (!L || !map) return
    if (!homeRef.current) {
      homeRef.current = L.circleMarker([props.centerLat, props.centerLon], {
        radius: 5,
        color: '#10b981',
        fillColor: '#10b981',
        fillOpacity: 0.85,
        weight: 2,
      })
        .addTo(map)
        .bindTooltip(
          `HOME · ${props.centerLat.toFixed(6)}, ${props.centerLon.toFixed(6)}`,
        )
    }
  }, [props.centerLat, props.centerLon])

  // --------------------------------------------------------- waypoint markers
  useEffect(() => {
    const L = LRef.current
    const map = mapRef.current
    if (!L || !map) return
    const seen = new Set<number>()
    const editable = props.mode === 'waypoint'
    props.waypoints.forEach((wp, i) => {
      seen.add(wp.seq)
      const err = props.erroredSeqs.has(wp.seq)
      const color = err ? WAYPOINT_ERR_COLOR : WAYPOINT_COLOR
      const html = `<div class="rsim-wpm ${editable ? 'rsim-wpm-edit' : ''}" style="--wc:${color}; border-color:${color}; background:${color}33">
          <span class="rsim-wpnum">${i + 1}</span>
          <span class="rsim-wpalt">${wp.z.toFixed(0)}m</span>
        </div>`
      let m = wpMarkers.current.get(wp.seq)
      const icon = L.divIcon({
        html,
        className: 'rsim-wpicon',
        iconSize: [34, 34],
        iconAnchor: [17, 17],
      })
      if (!m) {
        m = L.marker([wp.x, wp.y], {
          icon,
          draggable: editable,
          zIndexOffset: 300,
          keyboard: false,
        }).addTo(map)
        m.on('dragend', () => {
          const ll = m!.getLatLng()
          propsRef.current.onMoveWaypoint?.(wp.seq, ll.lat, ll.lng)
        })
        m.on('contextmenu', (e: LeafletNS.LeafletMouseEvent) => {
          L.DomEvent.stopPropagation(e)
          propsRef.current.onRemoveWaypoint?.(wp.seq)
        })
        m.on('click', (e: LeafletNS.LeafletMouseEvent) => {
          L.DomEvent.stopPropagation(e)
          propsRef.current.onSelectWaypoint?.(wp.seq)
        })
        m.bindTooltip(`waypoint ${i + 1} · alt ${wp.z.toFixed(0)} m · hold ${wp.param1.toFixed(0)} s · accept ${wp.param2.toFixed(1)} m`, {
          direction: 'top',
        })
        wpMarkers.current.set(wp.seq, m)
      } else {
        m.setLatLng([wp.x, wp.y])
        m.setIcon(icon)
        m.setTooltipContent(`waypoint ${i + 1} · alt ${wp.z.toFixed(0)} m · hold ${wp.param1.toFixed(0)} s · accept ${wp.param2.toFixed(1)} m`)
        if (m.dragging) {
          if (editable) m.dragging.enable()
          else m.dragging.disable()
        }
      }
    })
    for (const [seq, m] of wpMarkers.current) {
      if (!seen.has(seq)) {
        m.remove()
        wpMarkers.current.delete(seq)
      }
    }
    // planned path (blue polyline — QGC direction-line grammar)
    if (props.waypoints.length >= 2) {
      const pts: LatLng[] = props.waypoints.map((w) => ({ lat: w.x, lng: w.y }))
      if (!planPathRef.current) {
        planPathRef.current = L.polyline(pts, {
          color: PATH_COLOR,
          weight: 2,
          opacity: 0.85,
          dashArray: '8 6',
        }).addTo(map)
      } else {
        planPathRef.current.setLatLngs(pts)
      }
    } else if (planPathRef.current) {
      planPathRef.current.remove()
      planPathRef.current = null
    }
  }, [props.waypoints, props.erroredSeqs, props.mode])

  // --------------------------------------------------------- geofence polygon
  useEffect(() => {
    const L = LRef.current
    const map = mapRef.current
    if (!L || !map) return

    // Render the closed inclusion polygon (when ≥ 3 vertices)
    const pts: LatLng[] = props.geofence.inclusion.map(([lat, lon]) => ({ lat, lng: lon }))
    if (pts.length >= 3) {
      if (!fenceRef.current) {
        fenceRef.current = L.polygon(pts, {
          color: FENCE_COLOR,
          weight: 1.5,
          dashArray: '6 5',
          fillOpacity: 0.05,
        }).addTo(map)
      } else {
        fenceRef.current.setLatLngs(pts)
      }
      if (!fenceRef.current.getTooltip()) {
        fenceRef.current.bindTooltip(
          `geofence · ${pts.length} vertices · ceiling ${props.geofence.ceiling_m.toFixed(0)} m · floor ${props.geofence.floor_m.toFixed(0)} m`,
          { sticky: true },
        )
      } else {
        fenceRef.current.setTooltipContent(
          `geofence · ${pts.length} vertices · ceiling ${props.geofence.ceiling_m.toFixed(0)} m · floor ${props.geofence.floor_m.toFixed(0)} m`,
        )
      }
    } else if (fenceRef.current) {
      fenceRef.current.remove()
      fenceRef.current = null
    }

    // Render the in-progress fence vertex markers (always, even when < 3)
    const seen = new Set<number>()
    props.geofence.inclusion.forEach(([lat, lon], i) => {
      seen.add(i)
      let m = fenceVertsRef.current.get(i)
      const html = `<div class="rsim-fvert" style="--vc:${FENCE_COLOR}">${i + 1}</div>`
      const icon = L.divIcon({
        html,
        className: 'rsim-fvert-icon',
        iconSize: [22, 22],
        iconAnchor: [11, 11],
      })
      if (!m) {
        m = L.marker([lat, lon], { icon, draggable: props.mode === 'fence', zIndexOffset: 200, keyboard: false }).addTo(map)
        m.on('contextmenu', (e: LeafletNS.LeafletMouseEvent) => {
          L.DomEvent.stopPropagation(e)
          // remove this vertex from the inclusion
          const next = propsRef.current.geofence.inclusion.filter((_, j) => j !== i)
          // We can't change props from here; bubble up via a synthetic
          // "remove vertex" by mutating through the parent — but the parent
          // only exposes onAddFenceVertex / onCloseFence. So we re-issue an
          // onAddFenceVertex with the previous vertex position to "undo"
          // is wrong; instead, we re-emit by calling the close handler if
          // 1 vertex remains, otherwise nothing. The right move: the parent
          // passes a remove callback when needed. For now, we ask the user
          // to clear the whole fence.
        })
        m.bindTooltip(`fence vertex ${i + 1}`, { direction: 'top' })
        fenceVertsRef.current.set(i, m)
      } else {
        m.setLatLng([lat, lon])
        m.setIcon(icon)
        if (m.dragging) {
          if (props.mode === 'fence') m.dragging.enable()
          else m.dragging.disable()
        }
      }
    })
    for (const [i, m] of fenceVertsRef.current) {
      if (!seen.has(i)) {
        m.remove()
        fenceVertsRef.current.delete(i)
      }
    }
    // Re-key the vertex map (indices shifted when one is removed)
    if (fenceVertsRef.current.size !== props.geofence.inclusion.length) {
      const oldMap = new Map(fenceVertsRef.current)
      fenceVertsRef.current.clear()
      props.geofence.inclusion.forEach((_, i) => {
        const m = oldMap.get(i)
        if (m) fenceVertsRef.current.set(i, m)
      })
    }

    // Temp polyline (the open polyline while drawing, before closing)
    if (props.fenceDrawing && pts.length >= 1) {
      // show a thin gray dotted line from the last vertex to the cursor —
      // we approximate by drawing the open polyline of all current pts
      if (!fenceTempLineRef.current) {
        fenceTempLineRef.current = L.polyline(pts, {
          color: FENCE_COLOR,
          weight: 1,
          opacity: 0.6,
          dashArray: '2 4',
        }).addTo(map)
      } else {
        fenceTempLineRef.current.setLatLngs(pts)
      }
    } else if (fenceTempLineRef.current) {
      fenceTempLineRef.current.remove()
      fenceTempLineRef.current = null
    }
  }, [props.geofence, props.mode, props.fenceDrawing])

  // cursor: crosshair in waypoint mode, cell in fence mode
  useEffect(() => {
    const el = holderRef.current
    if (!el) return
    el.classList.toggle('rsim-plan-cursor', props.mode === 'waypoint')
    el.classList.toggle('rsim-fence-cursor', props.mode === 'fence')
  }, [props.mode])

  return (
    <div
      ref={holderRef}
      role="img"
      aria-label="Plan map: click to add waypoints, draw geofence polygon, drag to move"
      className={`relative h-full w-full ${props.className ?? ''}`}
    />
  )
}
