'use client'

/**
 * Geo map (ADR-0017): a Leaflet 1.9 map integrated directly (no
 * react-leaflet wrapper — the trimmed-dependency contract). OSM raster
 * tiles online; when tiles fail (offline deployment) the layer degrades to
 * a drawn graticule so the view stays honest and usable.
 *
 * Renders: live vehicles (heading arrows, click popup), trajectories, the
 * geofence polygon (geo-projected from NED), the home origin, operator
 * waypoints (draggable, numbered) and the planned path — QGC Fly/Plan
 * visual grammar.
 */

import { useEffect, useRef } from 'react'
import type * as LeafletNS from 'leaflet'
import 'leaflet/dist/leaflet.css'

import type { GeoOrigin, LatLng } from '@/lib/geo'
import type { MapWaypoint } from '@/lib/types'

export interface GeoVehicleMarker {
  key: string
  lat: number
  lng: number
  heading_deg: number
  label: string
  color: string
  detail: string
}

export interface GeoMapProps {
  origin: GeoOrigin
  fenceGeo: LatLng[]
  fenceCeilingM: number
  vehicles: GeoVehicleMarker[]
  /** Per-vehicle trajectory in geo coords (oldest first). */
  trails: Record<string, LatLng[]>
  trailColors: Record<string, string>
  waypoints: MapWaypoint[]
  planMode: boolean
  followKey: string | null
  onMapClick: (lat: number, lng: number) => void
  onWaypointDrag: (key: string, lat: number, lng: number) => void
  onWaypointRemove: (key: string) => void
  onTilesOffline: () => void
  className?: string
}

const TILE_URL = 'https://tile.openstreetmap.org/{z}/{x}/{y}.png'
const TILE_ATTR = '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'
const TILE_ERROR_LIMIT = 8

export function GeoMap(props: GeoMapProps) {
  const holderRef = useRef<HTMLDivElement | null>(null)
  const mapRef = useRef<LeafletNS.Map | null>(null)
  const LRef = useRef<typeof LeafletNS | null>(null)
  const tileRef = useRef<LeafletNS.TileLayer | null>(null)
  const tileErrorsRef = useRef(0)
  const fenceRef = useRef<LeafletNS.Polygon | null>(null)
  const homeRef = useRef<LeafletNS.CircleMarker | null>(null)
  const vehicleMarkers = useRef<Map<string, LeafletNS.Marker>>(new Map())
  const trailLines = useRef<Map<string, LeafletNS.Polyline>>(new Map())
  const wpMarkers = useRef<Map<string, LeafletNS.Marker>>(new Map())
  const planPathRef = useRef<LeafletNS.Polyline | null>(null)
  const propsRef = useRef(props)
  const fenceBoundsRef = useRef<LeafletNS.LatLngBoundsExpression | null>(null)
  useEffect(() => {
    propsRef.current = props
  }, [props])
  const didFitRef = useRef(false)

  // ------------------------------------------------------------- map init
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
        center: [props.origin.lat_deg, props.origin.lon_deg],
        zoom: 18,
        zoomControl: true,
        attributionControl: true,
      })
      mapRef.current = map
      const tiles = L.tileLayer(TILE_URL, {
        attribution: TILE_ATTR,
        maxZoom: 19,
      })
      tiles.on('tileerror', () => {
        tileErrorsRef.current++
        if (tileErrorsRef.current >= TILE_ERROR_LIMIT && tileRef.current) {
          // offline: swap to a drawn graticule so the map stays usable and
          // honest about what it is
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
          propsRef.current.onTilesOffline()
        }
      })
      tiles.addTo(map)
      tileRef.current = tiles
      map.on('click', (e: LeafletNS.LeafletMouseEvent) => {
        propsRef.current.onMapClick(e.latlng.lat, e.latlng.lng)
      })
      // The console mounts all tabs hidden (forceMount) — Leaflet in a
      // zero-size container renders degenerate. ResizeObserver: re-measure
      // when the tab becomes visible, and fit the fence once it has a
      // real size.
      const ro = new ResizeObserver(() => {
        map.invalidateSize()
        if (!didFitRef.current && el.clientWidth > 100 && fenceBoundsRef.current) {
          didFitRef.current = true
          map.fitBounds(fenceBoundsRef.current)
        }
      })
      ro.observe(el)
      roCleanup = () => ro.disconnect()
      // invalidate once mounted so the container sizes correctly inside
      // tab-hidden layouts
      requestAnimationFrame(() => map.invalidateSize())
    })
    return () => {
      cancelled = true
      roCleanup?.()
      mapRef.current?.remove()
      mapRef.current = null
      vehicleMarkers.current.clear()
      trailLines.current.clear()
      wpMarkers.current.clear()
      fenceRef.current = null
      homeRef.current = null
      planPathRef.current = null
    }
  }, [])

  // ------------------------------------------------------------- fence/home
  useEffect(() => {
    const L = LRef.current
    const map = mapRef.current
    if (!L || !map) return
    if (props.fenceGeo.length >= 3) {
      if (!fenceRef.current) {
        fenceRef.current = L.polygon(props.fenceGeo, {
          color: '#d97706',
          weight: 1.5,
          dashArray: '6 5',
          fillOpacity: 0.05,
        }).addTo(map)
      } else {
        fenceRef.current.setLatLngs(props.fenceGeo)
      }
      if (!fenceRef.current.getTooltip()) {
        fenceRef.current.bindTooltip(`geofence · ceiling ${props.fenceCeilingM.toFixed(0)} m`, {
          sticky: true,
        })
      }
      if (!didFitRef.current) {
        // remember the bounds; the ResizeObserver fits once the container
        // has a real size (tab visible)
        fenceBoundsRef.current = fenceRef.current.getBounds()
        if ((mapRef.current?.getContainer()?.clientWidth ?? 0) > 100) {
          didFitRef.current = true
          // CRITICAL: sync Leaflet's internal size BEFORE fitting — the
          // console mounts all tabs forceMount-hidden, and fitting against
          // the stale 0×0 cached size produces a degenerate world view
          // (the fence collapses to a few pixels and map clicks resolve
          // to garbage lat/lon).
          map.invalidateSize()
          map.fitBounds(fenceRef.current.getBounds().pad(0.35))
        }
      }
    }
    // home = the scenario origin
    if (!homeRef.current) {
      homeRef.current = L.circleMarker([props.origin.lat_deg, props.origin.lon_deg], {
        radius: 5,
        color: '#10b981',
        fillColor: '#10b981',
        fillOpacity: 0.85,
        weight: 2,
      })
        .addTo(map)
        .bindTooltip(
          `HOME · ${props.origin.lat_deg.toFixed(6)}, ${props.origin.lon_deg.toFixed(6)} · ${props.origin.alt_m.toFixed(0)} m MSL`,
        )
    } else {
      homeRef.current.setLatLng([props.origin.lat_deg, props.origin.lon_deg])
    }
  }, [props.fenceGeo, props.fenceCeilingM, props.origin])

  // ------------------------------------------------------------- vehicles
  useEffect(() => {
    const L = LRef.current
    const map = mapRef.current
    if (!L || !map) return
    const seen = new Set<string>()
    for (const v of props.vehicles) {
      seen.add(v.key)
      const html = `<div class="rsim-vmarker" style="--vc:${v.color}">
          <span class="rsim-vlabel">${v.label}</span>
          <span class="rsim-vring"></span>
          <span class="rsim-varrow" style="transform: rotate(${v.heading_deg}deg)"></span>
        </div>`
      let m = vehicleMarkers.current.get(v.key)
      if (!m) {
        m = L.marker([v.lat, v.lng], {
          icon: L.divIcon({
            html,
            className: 'rsim-vicon',
            iconSize: [46, 46],
            iconAnchor: [23, 23],
          }),
          zIndexOffset: 500,
          keyboard: false,
        }).addTo(map)
        m.bindPopup(v.detail, { closeButton: true })
        vehicleMarkers.current.set(v.key, m)
      } else {
        m.setLatLng([v.lat, v.lng])
        const el = m.getElement()?.querySelector('.rsim-varrow')
        if (el instanceof HTMLElement) el.style.transform = `rotate(${v.heading_deg}deg)`
        const ring = m.getElement()?.querySelector('.rsim-vring')
        if (ring instanceof HTMLElement) ring.style.setProperty('--vc', v.color)
        const lab = m.getElement()?.querySelector('.rsim-vlabel')
        if (lab instanceof HTMLElement) {
          lab.style.setProperty('--vc', v.color)
          lab.textContent = v.label
        }
        m.setPopupContent(v.detail)
      }
    }
    for (const [key, m] of vehicleMarkers.current) {
      if (!seen.has(key)) {
        m.remove()
        vehicleMarkers.current.delete(key)
      }
    }
    // follow the selected vehicle
    if (props.followKey) {
      const m = vehicleMarkers.current.get(props.followKey)
      if (m) map.panTo(m.getLatLng(), { animate: true, duration: 0.4 })
    }
  }, [props.vehicles, props.followKey])

  // ------------------------------------------------------------- trails
  useEffect(() => {
    const L = LRef.current
    const map = mapRef.current
    if (!L || !map) return
    const seen = new Set<string>()
    for (const [key, pts] of Object.entries(props.trails)) {
      if (pts.length < 2) continue
      seen.add(key)
      let line = trailLines.current.get(key)
      if (!line) {
        line = L.polyline(pts, {
          color: props.trailColors[key] ?? '#38bdf8',
          weight: 1.6,
          opacity: 0.75,
        }).addTo(map)
        trailLines.current.set(key, line)
      } else {
        line.setLatLngs(pts)
      }
    }
    for (const [key, line] of trailLines.current) {
      if (!seen.has(key)) {
        line.remove()
        trailLines.current.delete(key)
      }
    }
  }, [props.trails, props.trailColors])

  // ------------------------------------------------------- plan waypoints
  useEffect(() => {
    const L = LRef.current
    const map = mapRef.current
    if (!L || !map) return
    const seen = new Set<string>()
    props.waypoints.forEach((wp, i) => {
      seen.add(wp.key)
      let m = wpMarkers.current.get(wp.key)
      const icon = L.divIcon({
        html: `<div class="rsim-wpm ${props.planMode ? 'rsim-wpm-edit' : ''}">
            <span class="rsim-wpnum">${i + 1}</span>
            <span class="rsim-wpalt">${wp.alt_m.toFixed(0)}m</span>
          </div>`,
        className: 'rsim-wpicon',
        iconSize: [34, 34],
        iconAnchor: [17, 17],
      })
      if (!m) {
        m = L.marker([wp.lat, wp.lng], {
          icon,
          draggable: props.planMode,
          zIndexOffset: 300,
        }).addTo(map)
        m.on('dragend', () => {
          const ll = m!.getLatLng()
          propsRef.current.onWaypointDrag(wp.key, ll.lat, ll.lng)
        })
        m.on('contextmenu', () => propsRef.current.onWaypointRemove(wp.key))
        m.bindTooltip(`${wp.key} · ${wp.alt_m.toFixed(0)} m AGL · hover ${wp.hover_s.toFixed(0)} s`, {
          direction: 'top',
        })
        wpMarkers.current.set(wp.key, m)
      } else {
        m.setLatLng([wp.lat, wp.lng])
        m.setIcon(icon)
        m.setTooltipContent(`${wp.key} · ${wp.alt_m.toFixed(0)} m AGL · hover ${wp.hover_s.toFixed(0)} s`)
        if (m.dragging) {
          if (props.planMode) m.dragging.enable()
          else m.dragging.disable()
        }
      }
    })
    for (const [key, m] of wpMarkers.current) {
      if (!seen.has(key)) {
        m.remove()
        wpMarkers.current.delete(key)
      }
    }
    // the planned path between waypoints (QGC direction-line grammar)
    if (props.waypoints.length >= 2) {
      const pts: LatLng[] = props.waypoints.map((w) => ({ lat: w.lat, lng: w.lng }))
      if (!planPathRef.current) {
        planPathRef.current = L.polyline(pts, {
          color: '#0ea5e9',
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
  }, [props.waypoints, props.planMode])

  // cursor: crosshair while planning
  useEffect(() => {
    const el = holderRef.current
    if (el) el.classList.toggle('rsim-plan-cursor', props.planMode)
  }, [props.planMode])

  return (
    <div
      ref={holderRef}
      role="img"
      aria-label="Operator geo map: live vehicle positions, geofence, planned mission"
      className={`relative h-full w-full ${props.className ?? ''}`}
    />
  )
}
