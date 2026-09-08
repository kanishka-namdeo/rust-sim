'use client'

/**
 * Operator map data + command hook (mavfleet :8400, ADR-0017).
 *
 * Same dual-mode lifecycle as useFleetC2: probe REST /api/fleet through the
 * gateway → LIVE via the fleet WS frames (10 Hz) with REST fallback, or
 * SIMULATED via the FleetMockEngine's operator plane (fence-validated
 * uploads, go-to flight, guided commands — the same semantics).
 *
 * The command surface is the QGC action bar: arm/disarm, takeoff, land,
 * RTL, hold, go-to (map click), e-stop — plus the Plan-view mission
 * workflow: upload waypoints, start, clear. Every live command is a plain
 * REST POST to the manager's operator control plane.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchGw, gw, normalizeFleetSnapshot, probePlane, unwrapEnvelope, wsUrl } from '@/lib/conn'
import { FleetMockEngine } from '@/lib/mock-fleet'
import { DEFAULT_ORIGIN, nedPolygonToGeo, type GeoOrigin, type LatLng } from '@/lib/geo'
import type {
  ConnState,
  FleetEvent,
  FleetSnapshot,
  FleetVehicle,
  GuidedResult,
  MapWaypoint,
  MissionUploadResult,
} from '@/lib/types'

export const FLEET_PORT = 8400

const EVENT_CAP = 120
const TRAIL_CAP = 120

/** Vehicle colors on the map (by fleet index). */
const VEHICLE_COLORS = ['#38bdf8', '#f472b6', '#a3e635', '#fbbf24', '#c084fc', '#fb7185', '#34d399', '#f97316']

export function useOperatorMap() {
  const [conn, setConn] = useState<ConnState>('connecting')
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryAt, setRetryAt] = useState<number | null>(null)
  const [snapshot, setSnapshot] = useState<FleetSnapshot | null>(null)
  const [events, setEvents] = useState<FleetEvent[]>([])
  const [tilesOffline, setTilesOffline] = useState(false)
  const [frameCount, setFrameCount] = useState(0)
  const [trails, setTrails] = useState<Record<string, LatLng[]>>({})

  const engineRef = useRef<FleetMockEngine | null>(null)
  const connRef = useRef<ConnState>('connecting')
  const vehicleMapRef = useRef<Record<string, FleetVehicle>>({})
  const trailsRef = useRef<Record<string, LatLng[]>>({})

  const setConnBoth = useCallback((c: ConnState) => {
    connRef.current = c
    setConn(c)
  }, [])

  const applySnapshot = useCallback(
    (snap: FleetSnapshot, newEvents: FleetEvent[]) => {
      vehicleMapRef.current = Object.fromEntries(snap.vehicles.map((v) => [v.id, v]))
      setSnapshot(snap)
      if (newEvents.length > 0) {
        setEvents((prev) => [...prev, ...newEvents].slice(-EVENT_CAP))
      }
      setFrameCount((c) => c + 1)
      // geo trails from the live lat/lon fixes
      const nextTrails: Record<string, LatLng[]> = { ...trailsRef.current }
      for (const v of snap.vehicles) {
        if (v.lat == null || v.lon == null) continue
        const arr = nextTrails[v.id] ? [...nextTrails[v.id]] : []
        const last = arr[arr.length - 1]
        if (!last || Math.abs(last.lat - v.lat) > 1e-9 || Math.abs(last.lng - v.lon) > 1e-9) {
          arr.push({ lat: v.lat, lng: v.lon })
          if (arr.length > TRAIL_CAP) arr.shift()
        }
        nextTrails[v.id] = arr
      }
      trailsRef.current = nextTrails
      setTrails(nextTrails)
    },
    [],
  )

  const ingestRaw = useCallback(
    (raw: unknown): boolean => {
      const norm = normalizeFleetSnapshot(raw, vehicleMapRef.current)
      if (!norm) return false
      applySnapshot(norm.snapshot, norm.events)
      return true
    },
    [applySnapshot],
  )

  useEffect(() => {
    let cancelled = false
    let ws: WebSocket | null = null
    let mockTimer: ReturnType<typeof setInterval> | null = null
    let pollTimer: ReturnType<typeof setInterval> | null = null
    let retryTimer: ReturnType<typeof setInterval> | null = null
    let wsFailures = 0
    let pollFailures = 0

    const engine = new FleetMockEngine()
    engineRef.current = engine

    const clearTimers = () => {
      if (mockTimer) {
        clearInterval(mockTimer)
        mockTimer = null
      }
      if (pollTimer) {
        clearInterval(pollTimer)
        pollTimer = null
      }
    }
    const teardownLive = () => {
      if (ws) {
        ws.onclose = null
        ws.onerror = null
        ws.onmessage = null
        try {
          ws.close()
        } catch {
          /* noop */
        }
        ws = null
      }
      clearTimers()
    }

    const startMock = (reason: string) => {
      if (cancelled) return
      teardownLive()
      setConnBoth('simulated')
      setLastError(reason)
      setRetryAt(Date.now() + 12000)
      if (mockTimer) return
      const fresh = new FleetMockEngine()
      engineRef.current = fresh
      vehicleMapRef.current = {}
      trailsRef.current = {}
      setTrails({})
      setEvents([])
      mockTimer = setInterval(() => {
        const tickRes = fresh.tick(0.2)
        applySnapshot(tickRes.snapshot, tickRes.events)
      }, 200)
    }

    const openWs = () => {
      if (cancelled) return
      try {
        ws = new WebSocket(wsUrl(FLEET_PORT))
      } catch {
        startMock('WS construction failed')
        return
      }
      ws.onmessage = (ev) => {
        wsFailures = 0
        try {
          const raw = JSON.parse(typeof ev.data === 'string' ? ev.data : '{}')
          ingestRaw(raw)
        } catch {
          /* skip malformed frame */
        }
      }
      ws.onclose = () => {
        if (cancelled) return
        wsFailures++
        if (wsFailures > 3) {
          startMock('WS stream closed — backend offline')
          return
        }
        setTimeout(() => {
          if (!cancelled && connRef.current === 'live') openWs()
        }, 2500)
      }
      ws.onerror = () => {
        /* onclose follows */
      }
    }

    const pollFleet = () => {
      fetchGw(gw(FLEET_PORT, '/api/fleet'), { method: 'GET' }, 2500)
        .then(async (res) => {
          if (!res.ok) throw new Error(`HTTP ${res.status}`)
          const j = await res.json()
          if (cancelled) return
          pollFailures = 0
          ingestRaw(unwrapEnvelope(j))
        })
        .catch(() => {
          if (cancelled) return
          pollFailures++
          if (pollFailures >= 2 && connRef.current === 'live') {
            startMock('REST /api/fleet polling failed — backend offline')
          }
        })
    }

    const goLive = () => {
      if (cancelled) return
      clearTimers()
      setConnBoth('live')
      setLastError(null)
      setRetryAt(null)
      pollFleet()
      openWs()
      pollTimer = setInterval(pollFleet, 4000)
    }

    probePlane(gw(FLEET_PORT, '/api/fleet')).then((alive) => {
      if (cancelled) return
      if (alive) goLive()
      else startMock('backend :8400 unreachable')
    })

    retryTimer = setInterval(() => {
      if (cancelled || connRef.current === 'live') return
      probePlane(gw(FLEET_PORT, '/api/fleet'), 2000).then((alive) => {
        if (cancelled || !alive || connRef.current === 'live') return
        goLive()
      })
      if (connRef.current === 'simulated') setRetryAt(Date.now() + 12000)
    }, 12000)

    return () => {
      cancelled = true
      teardownLive()
      if (retryTimer) clearInterval(retryTimer)
    }
  }, [applySnapshot, ingestRaw, setConnBoth])

  // --------------------------------------------------------------- commands

  /** POST one operator command; returns the `data` block or throws with the
   *  backend's honest error message. */
  const postCmd = useCallback(
    async (path: string, body: unknown, timeoutMs = 6000): Promise<Record<string, unknown>> => {
      const res = await fetchGw(
        gw(FLEET_PORT, path),
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body ?? {}),
        },
        timeoutMs,
      )
      const j = (await res.json().catch(() => null)) as { ok?: boolean; data?: unknown; error?: string } | null
      if (!res.ok || !j?.ok) {
        throw new Error(j?.error ?? `HTTP ${res.status}`)
      }
      return (j.data ?? {}) as Record<string, unknown>
    },
    [],
  )

  const live = conn === 'live'

  const estop = useCallback(async (): Promise<void> => {
    if (connRef.current === 'live') {
      await postCmd('/api/fleet/estop', {})
    } else {
      engineRef.current?.estop()
    }
  }, [postCmd])

  const arm = useCallback(
    async (index: number, armFlag: boolean): Promise<GuidedResult> => {
      if (connRef.current === 'live') {
        return (await postCmd(`/api/vehicles/${index}/arm`, { arm: armFlag })) as GuidedResult
      }
      const r = engineRef.current?.arm(index, armFlag) ?? { accepted: false, command: 'arm' }
      if (!r.accepted) throw new Error('mock: arm rejected (vehicle flying a mission?)')
      return r as GuidedResult
    },
    [postCmd],
  )

  const takeoff = useCallback(
    async (index: number, altM: number): Promise<GuidedResult> => {
      if (connRef.current === 'live') {
        return (await postCmd(`/api/vehicles/${index}/takeoff`, { alt_m: altM })) as GuidedResult
      }
      const r = engineRef.current?.takeoff(index, altM) ?? { accepted: false, command: 'takeoff' }
      if (!r.accepted) throw new Error('mock: takeoff rejected')
      return r as GuidedResult
    },
    [postCmd],
  )

  const land = useCallback(
    async (index: number): Promise<GuidedResult> => {
      if (connRef.current === 'live') {
        return (await postCmd(`/api/vehicles/${index}/land`, {})) as GuidedResult
      }
      return (engineRef.current?.land(index) ?? { accepted: false, command: 'land' }) as GuidedResult
    },
    [postCmd],
  )

  const rtl = useCallback(
    async (index: number): Promise<GuidedResult> => {
      if (connRef.current === 'live') {
        return (await postCmd(`/api/vehicles/${index}/rtl`, {})) as GuidedResult
      }
      return (engineRef.current?.rtl(index) ?? { accepted: false, command: 'rtl' }) as GuidedResult
    },
    [postCmd],
  )

  const hold = useCallback(
    async (index: number): Promise<GuidedResult> => {
      if (connRef.current === 'live') {
        return (await postCmd(`/api/vehicles/${index}/hold`, {})) as GuidedResult
      }
      return (engineRef.current?.hold(index) ?? { accepted: false, command: 'hold' }) as GuidedResult
    },
    [postCmd],
  )

  const goto = useCallback(
    async (index: number, lat: number, lng: number, altM?: number): Promise<GuidedResult> => {
      const body: Record<string, unknown> = { lat_deg: lat, lon_deg: lng }
      if (altM != null) body.alt_m = altM
      if (connRef.current === 'live') {
        return (await postCmd(`/api/vehicles/${index}/goto`, body)) as GuidedResult
      }
      const alt = altM ?? 10
      return (engineRef.current?.goto(index, lat, lng, alt) ?? {
        accepted: false,
        command: 'goto',
      }) as GuidedResult
    },
    [postCmd],
  )

  const uploadMission = useCallback(
    async (items: MapWaypoint[], replace: boolean): Promise<MissionUploadResult> => {
      if (connRef.current === 'live') {
        const payload = {
          items: items.map((w) => ({
            label: w.key,
            lat_deg: w.lat,
            lon_deg: w.lng,
            alt_m: w.alt_m,
            hover_s: w.hover_s,
          })),
          mode: replace ? 'replace' : 'append',
        }
        return (await postCmd('/api/mission', payload, 8000)) as MissionUploadResult
      }
      return engineRef.current?.uploadMission(items, replace) ?? { accepted: [], rejected: [], pool: 0 }
    },
    [postCmd],
  )

  const startMission = useCallback(
    async (): Promise<{ started: boolean; reason: string | null }> => {
      if (connRef.current === 'live') {
        return (await postCmd('/api/mission/start', {})) as { started: boolean; reason: string | null }
      }
      return engineRef.current?.startMission() ?? { started: false, reason: 'no engine' }
    },
    [postCmd],
  )

  const clearMission = useCallback(
    async (): Promise<{ cleared: number }> => {
      if (connRef.current === 'live') {
        return (await postCmd('/api/mission/clear', {})) as { cleared: number }
      }
      return { cleared: engineRef.current?.clearMission() ?? 0 }
    },
    [postCmd],
  )

  // ------------------------------------------------------------ derived geo

  const origin: GeoOrigin = snapshot?.geo_origin ?? DEFAULT_ORIGIN
  const fenceGeo: LatLng[] = snapshot ? nedPolygonToGeo(origin, snapshot.geofence.points) : []

  return {
    conn,
    port: FLEET_PORT,
    live,
    lastError,
    retryAt,
    snapshot,
    events,
    trails,
    tilesOffline,
    setTilesOffline,
    frameCount,
    origin,
    fenceGeo,
    vehicleColors: VEHICLE_COLORS,
    // commands
    estop,
    arm,
    takeoff,
    land,
    rtl,
    hold,
    goto,
    uploadMission,
    startMission,
    clearMission,
  }
}

export type OperatorMapApi = ReturnType<typeof useOperatorMap>
