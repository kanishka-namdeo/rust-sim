'use client'

/**
 * mavfleet Fleet C2 data hook (:8400).
 *
 * Same dual-mode lifecycle: probe REST /api/fleet through the gateway →
 * LIVE via /ws/fleet frames (10 Hz) + /api/events tail polling, or SIMULATED
 * via the FleetMockEngine with periodic live retries. The fleet command
 * surface (SPEC §5.4 / §13) covers:
 *   - E-STOP                    — POST /api/fleet/estop (single-button abort)
 *   - mission bindings          — GET / POST / DELETE /api/fleet/mission-bindings
 *   - fleet start (parallel/sequential)
 *
 * All fleet API calls route through `src/lib/conn.ts` gateway mode with
 * `?XTransformPort=8400`. The catalog (:8300) is the source of truth for
 * saved missions (the Assign dropdown fetches `GET /api/missions` from there);
 * bindings themselves are persisted on the fleet plane so they survive
 * catalog restarts.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchGw, gw, normalizeFleetSnapshot, probePlane, unwrapEnvelope, wsUrl } from '@/lib/conn'
import { FleetMockEngine } from '@/lib/mock-fleet'
import type {
  ConnState,
  FleetEvent,
  FleetSnapshot,
  FleetStartMode,
  FleetStartResult,
  MissionBinding,
  MissionBindingState,
  SequentialGate,
} from '@/lib/types'

export const FLEET_PORT = 8400

const EVENT_CAP = 200

export function useFleetC2() {
  const [conn, setConn] = useState<ConnState>('connecting')
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryAt, setRetryAt] = useState<number | null>(null)
  const [snapshot, setSnapshot] = useState<FleetSnapshot | null>(null)
  const [events, setEvents] = useState<FleetEvent[]>([])
  const [bindings, setBindings] = useState<MissionBinding[]>([])
  const [busy, setBusy] = useState(false)
  const [frameCount, setFrameCount] = useState(0)

  const engineRef = useRef<FleetMockEngine | null>(null)
  const connRef = useRef<ConnState>('connecting')
  const vehicleMapRef = useRef<Record<string, import('@/lib/types').FleetVehicle>>({})

  const setConnBoth = useCallback((c: ConnState) => {
    connRef.current = c
    setConn(c)
  }, [])

  /** Ingest a normalized snapshot + incremental events. */
  const applySnapshot = useCallback(
    (snap: FleetSnapshot, newEvents: FleetEvent[]) => {
      vehicleMapRef.current = Object.fromEntries(snap.vehicles.map((v) => [v.id, v]))
      setSnapshot(snap)
      if (newEvents.length > 0) {
        setEvents((prev) => [...prev, ...newEvents].slice(-EVENT_CAP))
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
      const { snapshot: snap, events: evs } = norm
      // carry breadcrumbs forward (WS frames may only carry positions)
      for (const v of snap.vehicles) {
        const prev = vehicleMapRef.current[v.id]
        if (prev && v.breadcrumb.length === 0) v.breadcrumb = prev.breadcrumb
        v.breadcrumb.push({ n: v.position_ned_m[0], e: v.position_ned_m[1] })
        if (v.breadcrumb.length > 6) v.breadcrumb.shift()
      }
      applySnapshot(snap, evs)
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

  // ---- v1 (GCS_SPEC §5.4): mission bindings + fleet start -----

  /** Pull a human-readable error message out of the backend's error envelope.
   * The envelope shape is `{ok:false, error:{code,message}} | {ok:false, error:string}`. */
  const envelopeError = (raw: unknown, fallback: string): string => {
    if (!raw || typeof raw !== 'object') return fallback
    const r = raw as Record<string, unknown>
    const err = r.error
    if (typeof err === 'string') return err || fallback
    if (err && typeof err === 'object') {
      const e = err as Record<string, unknown>
      const msg = typeof e.message === 'string' ? e.message : typeof e.code === 'string' ? e.code : null
      if (msg) return msg
    }
    return fallback
  }

  /** Tolerant binding-shape normalizer (the backend may omit fields). */
  const normBinding = (raw: unknown): MissionBinding | null => {
    if (!raw || typeof raw !== 'object') return null
    const r = raw as Record<string, unknown>
    const vehicle_id = typeof r.vehicle_id === 'number' ? r.vehicle_id : Number(r.vehicle_id)
    if (!Number.isFinite(vehicle_id)) return null
    const mission_id = typeof r.mission_id === 'string' ? r.mission_id : String(r.mission_id ?? '')
    const rawState = typeof r.binding_state === 'string' ? r.binding_state : typeof r.state === 'string' ? r.state : 'unassigned'
    const allowed: MissionBindingState[] = ['unassigned', 'assigned', 'uploaded', 'active', 'complete', 'aborted']
    const binding_state = (allowed.includes(rawState as MissionBindingState) ? rawState : 'unassigned') as MissionBindingState
    return { vehicle_id, mission_id, binding_state }
  }

  /** GET /api/fleet/mission-bindings (:8400) — refresh the bindings table. */
  const listMissionBindings = useCallback(async (): Promise<MissionBinding[]> => {
    try {
      const res = await fetchGw(gw(FLEET_PORT, '/api/fleet/mission-bindings'), { method: 'GET' }, 3000)
      if (!res.ok) return []
      const j = await res.json()
      const data = unwrapEnvelope(j)
      const arr = Array.isArray(data) ? data : Array.isArray((data as { bindings?: unknown[] })?.bindings) ? (data as { bindings: unknown[] }).bindings : []
      const list = arr.map(normBinding).filter((b): b is MissionBinding => b != null)
      setBindings(list)
      return list
    } catch {
      return []
    }
  }, [])

  /**
   * POST /api/fleet/mission-bindings (:8400) — body: `{"bindings":[{vehicle_id,
   * mission_id}, ...]}`. Persists the operator's assignments to the fleet plane
   * so they survive console reconnects. Returns the refreshed bindings list.
   */
  const setMissionBindings = useCallback(
    async (inputs: { vehicle_id: number; mission_id: string }[]): Promise<{ ok: boolean; bindings: MissionBinding[]; error: string | null }> => {
      setBusy(true)
      try {
        const res = await fetchGw(
          gw(FLEET_PORT, '/api/fleet/mission-bindings'),
          {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ bindings: inputs }),
          },
          6000,
        )
        if (!res.ok) {
          let err = `HTTP ${res.status}`
          try {
            const j = await res.json()
            err = envelopeError(unwrapEnvelope(j), err)
          } catch {
            /* keep HTTP status as error */
          }
          return { ok: false, bindings: [], error: err }
        }
        const j = await res.json()
        const data = unwrapEnvelope(j)
        const arr = Array.isArray(data)
          ? data
          : Array.isArray((data as { bindings?: unknown[] })?.bindings)
            ? (data as { bindings: unknown[] }).bindings
            : []
        const list = arr.map(normBinding).filter((b): b is MissionBinding => b != null)
        setBindings(list)
        return { ok: true, bindings: list, error: null }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        return { ok: false, bindings: [], error: msg }
      } finally {
        setBusy(false)
      }
    },
    [],
  )

  /** DELETE /api/fleet/mission-bindings/{vehicleId} (:8400) — clear one row. */
  const clearMissionBinding = useCallback(
    async (vehicleId: number): Promise<boolean> => {
      try {
        const res = await fetchGw(gw(FLEET_PORT, `/api/fleet/mission-bindings/${vehicleId}`), { method: 'DELETE' }, 3000)
        if (!res.ok) return false
        setBindings((prev) => prev.filter((b) => b.vehicle_id !== vehicleId))
        return true
      } catch {
        return false
      }
    },
    [],
  )

  /**
   * POST /api/fleet/start (:8400) — kick off every bound mission.
   *
   * Body shape per SPEC §5.4:
   *   `{mode: "parallel"|"sequential", sequential_gate?: "first_waypoint"|
   *    "takeoff_complete", timeout_s?: 30}`
   *
   * Returns the per-vehicle result list (`[{vehicle_id, mission_id, status}]`)
   * so the modal can stream started/failed/timeout rows as they land.
   */
  const startFleet = useCallback(
    async (mode: FleetStartMode, sequentialGate?: SequentialGate, timeoutS?: number): Promise<{ ok: boolean; results: FleetStartResult[]; error: string | null }> => {
      setBusy(true)
      try {
        const body: Record<string, unknown> = { mode }
        if (mode === 'sequential') {
          body.sequential_gate = sequentialGate ?? 'first_waypoint'
          body.timeout_s = typeof timeoutS === 'number' && Number.isFinite(timeoutS) ? timeoutS : 30
        }
        const res = await fetchGw(
          gw(FLEET_PORT, '/api/fleet/start'),
          {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(body),
          },
          10_000,
        )
        if (!res.ok) {
          let err = `HTTP ${res.status}`
          try {
            const j = await res.json()
            err = envelopeError(unwrapEnvelope(j), err)
          } catch {
            /* keep HTTP status */
          }
          return { ok: false, results: [], error: err }
        }
        const j = await res.json()
        const data = unwrapEnvelope(j)
        const arr = Array.isArray(data)
          ? data
          : Array.isArray((data as { results?: unknown[] })?.results)
            ? (data as { results: unknown[] }).results
            : []
        const results: FleetStartResult[] = arr
          .map((r): FleetStartResult | null => {
            if (!r || typeof r !== 'object') return null
            const x = r as Record<string, unknown>
            const vehicle_id = typeof x.vehicle_id === 'number' ? x.vehicle_id : Number(x.vehicle_id)
            const mission_id = typeof x.mission_id === 'string' ? x.mission_id : String(x.mission_id ?? '')
            const status = x.status === 'started' || x.status === 'failed' || x.status === 'timeout' ? (x.status as FleetStartResult['status']) : 'failed'
            const detail = typeof x.detail === 'string' ? x.detail : typeof x.message === 'string' ? x.message : undefined
            if (!Number.isFinite(vehicle_id)) return null
            return { vehicle_id, mission_id, status, detail }
          })
          .filter((r): r is FleetStartResult => r != null)
        // Promote any binding whose vehicle reported "started" to "active"
        // (the backend drives the rest of the lifecycle via telemetry).
        const startedIds = new Set(results.filter((r) => r.status === 'started').map((r) => r.vehicle_id))
        if (startedIds.size > 0) {
          setBindings((prev) =>
            prev.map((b) => (startedIds.has(b.vehicle_id) && b.binding_state === 'uploaded' ? { ...b, binding_state: 'active' } : b)),
          )
        }
        return { ok: true, results, error: null }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        return { ok: false, results: [], error: msg }
      } finally {
        setBusy(false)
      }
    },
    [],
  )

  /** Refresh bindings on a short interval while live. The bind panel also
   * calls these on mount; the interval keeps the binding_state column in sync
   * as the fleet orchestrator advances it. */
  useEffect(() => {
    if (conn !== 'live') return
    let cancelled = false
    const tick = async () => {
      if (cancelled) return
      await listMissionBindings()
    }
    void tick()
    const t = setInterval(() => {
      void tick()
    }, 5000)
    return () => {
      cancelled = true
      clearInterval(t)
    }
  }, [conn, listMissionBindings])

  /** Local optimistic assign — used by the Assign dropdown so the UI flips to
   * "assigned" instantly, then POSTs to persist. Falls back to nothing if the
   * POST fails (the bindings list refresh will correct the row). */
  const assignLocal = useCallback((vehicleId: number, missionId: string) => {
    setBindings((prev) => {
      const idx = prev.findIndex((b) => b.vehicle_id === vehicleId)
      if (idx === -1) return [...prev, { vehicle_id: vehicleId, mission_id: missionId, binding_state: 'assigned' }]
      const next = [...prev]
      next[idx] = { ...next[idx], mission_id: missionId, binding_state: 'assigned' }
      return next
    })
  }, [])

  /** Local optimistic upload flip — used by "Upload all". */
  const markUploaded = useCallback((vehicleIds: number[]) => {
    const set = new Set(vehicleIds)
    setBindings((prev) => prev.map((b) => (set.has(b.vehicle_id) && b.binding_state === 'assigned' ? { ...b, binding_state: 'uploaded' } : b)))
  }, [])

  return {
    conn,
    port: FLEET_PORT,
    lastError,
    retryAt,
    snapshot,
    events,
    bindings,
    busy,
    frameCount,
    estop,
    listMissionBindings,
    setMissionBindings,
    clearMissionBinding,
    assignLocal,
    markUploaded,
    startFleet,
  }
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
