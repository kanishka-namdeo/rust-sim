'use client'

/**
 * mavfleet Fleet C2 data hook (:8400).
 *
 * Same dual-mode lifecycle as useSimConsole: probe REST /api/fleet through the
 * gateway → LIVE via /ws/fleet frames (10 Hz) + /api/events tail polling, or
 * SIMULATED via the FleetMockEngine with periodic live retries. The only fleet
 * command surface is E-STOP (POST /api/fleet/estop) per SPEC §13 — everything
 * else is read-only by design.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchGw, gw, normalizeFleetSnapshot, probePlane, unwrapEnvelope, wsUrl } from '@/lib/conn'
import { FleetMockEngine } from '@/lib/mock-fleet'
import type { AuctionEntry, ConnState, FleetEvent, FleetSnapshot } from '@/lib/types'

export const FLEET_PORT = 8400

const EVENT_CAP = 200
const AUCTION_CAP = 40

export function useFleetC2() {
  const [conn, setConn] = useState<ConnState>('connecting')
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryAt, setRetryAt] = useState<number | null>(null)
  const [snapshot, setSnapshot] = useState<FleetSnapshot | null>(null)
  const [events, setEvents] = useState<FleetEvent[]>([])
  const [auctions, setAuctions] = useState<AuctionEntry[]>([])
  const [frameCount, setFrameCount] = useState(0)

  const engineRef = useRef<FleetMockEngine | null>(null)
  const connRef = useRef<ConnState>('connecting')
  const vehicleMapRef = useRef<Record<string, import('@/lib/types').FleetVehicle>>({})

  const setConnBoth = useCallback((c: ConnState) => {
    connRef.current = c
    setConn(c)
  }, [])

  /** Ingest a normalized snapshot + incremental events/auctions. */
  const applySnapshot = useCallback(
    (snap: FleetSnapshot, newEvents: FleetEvent[], newAuctions: AuctionEntry[]) => {
      vehicleMapRef.current = Object.fromEntries(snap.vehicles.map((v) => [v.id, v]))
      setSnapshot(snap)
      if (newEvents.length > 0) {
        setEvents((prev) => [...prev, ...newEvents].slice(-EVENT_CAP))
      }
      if (newAuctions.length > 0) {
        setAuctions((prev) => [...prev, ...newAuctions].slice(-AUCTION_CAP))
      }
      setFrameCount((c) => c + 1)
    },
    [],
  )

  /** Feed a raw live frame (WS or REST) through the normalizer. */
  const ingestRaw = useCallback(
    (raw: unknown): boolean => {
      const norm = normalizeFleetSnapshot(raw, vehicleMapRef.current)
      if (!norm) return false
      const { snapshot: snap, events: evs, auctions: aucs } = norm
      // carry breadcrumbs forward (WS frames may only carry positions)
      for (const v of snap.vehicles) {
        const prev = vehicleMapRef.current[v.id]
        if (prev && v.breadcrumb.length === 0) v.breadcrumb = prev.breadcrumb
        v.breadcrumb.push({ n: v.position_ned_m[0], e: v.position_ned_m[1] })
        if (v.breadcrumb.length > 6) v.breadcrumb.shift()
      }
      applySnapshot(snap, evs, aucs)
      return true
    },
    [applySnapshot],
  )

  useEffect(() => {
    let cancelled = false
    let ws: WebSocket | null = null
    let mockTimer: ReturnType<typeof setInterval> | null = null
    let eventsTimer: ReturnType<typeof setInterval> | null = null
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
      if (eventsTimer) {
        clearInterval(eventsTimer)
        eventsTimer = null
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
      setEvents([])
      setAuctions([])
      mockTimer = setInterval(() => {
        const tickRes = fresh.tick(0.2)
        applySnapshot(tickRes.snapshot, tickRes.events, tickRes.auctions)
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

    const pollEvents = () => {
      fetchGw(gw(FLEET_PORT, '/api/events'), { method: 'GET' }, 2500)
        .then(async (res) => {
          if (!res.ok) return
          const j = await res.json()
          if (cancelled) return
          const data = unwrapEnvelope(j)
          const arr = Array.isArray(data)
            ? data
            : Array.isArray((data as { events?: unknown[] })?.events)
              ? (data as { events: unknown[] }).events
              : []
          const seen = new Set(events.map((e) => e.t + e.detail))
          const fresh = arr
            .map((e) => normalizeEventTolerant(e))
            .filter((e): e is FleetEvent => e != null && !seen.has(e.t + e.detail))
          if (fresh.length > 0) setEvents((prev) => [...prev, ...fresh].slice(-EVENT_CAP))
        })
        .catch(() => {
          /* events polling is best-effort */
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
      eventsTimer = setInterval(pollEvents, 4000)
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

  // ---------------------------------------------------------------- command

  const estop = useCallback(async (): Promise<boolean> => {
    if (connRef.current === 'live') {
      try {
        const res = await fetchGw(gw(FLEET_PORT, '/api/fleet/estop'), { method: 'POST' })
        return res.ok
      } catch {
        return false
      }
    }
    engineRef.current?.estop()
    return true
  }, [])

  return { conn, port: FLEET_PORT, lastError, retryAt, snapshot, events, auctions, frameCount, estop }
}

export type FleetC2Api = ReturnType<typeof useFleetC2>

function normalizeEventTolerant(e: unknown): FleetEvent | null {
  if (!e || typeof e !== 'object') return null
  const r = e as Record<string, unknown>
  const detail = typeof r.detail === 'string' ? r.detail : typeof r.message === 'string' ? r.message : null
  if (!detail) return null
  const sevRaw = typeof r.severity === 'string' ? r.severity.toLowerCase() : 'info'
  const severity =
    sevRaw === 'warn' || sevRaw === 'warning' ? 'warn' : sevRaw === 'critical' || sevRaw === 'error' ? 'critical' : 'info'
  return {
    t: typeof r.t === 'number' ? r.t : typeof r.timestamp === 'number' ? r.timestamp : Date.now(),
    t_s: typeof r.t_s === 'number' ? r.t_s : 0,
    kind: typeof r.kind === 'string' ? r.kind : 'info',
    vehicle: typeof r.vehicle === 'string' ? r.vehicle : null,
    detail,
    severity,
  }
}
