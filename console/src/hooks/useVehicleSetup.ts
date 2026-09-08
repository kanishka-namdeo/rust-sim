'use client'

/**
 * Vehicle Setup data hook (:8400, ADR-0016 — the QGC/Mission-Planner
 * configuration workflow).
 *
 * Same dual-mode lifecycle as the other console hooks: probe the REST
 * plane → LIVE (poll the setup summary + param store, run the actions
 * against the endpoints) or SIMULATED (MockSetupEngine, with periodic
 * live retries). Catalog and modes are static-shaped in both modes.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchGw, gw, probePlane, unwrapEnvelope } from '@/lib/conn'
import { MOCK_AIRFRAME_GROUPS, MOCK_MODES, MockSetupEngine } from '@/lib/mock-setup'
import type {
  AirframeGroup,
  CalSensor,
  ModeEntry,
  ParamStoreView,
  VehicleSetupSummary,
} from '@/lib/types'

export const FLEET_PORT = 8400

export type SetupActionOutcome = {
  ok: boolean
  /** Human detail for the toast (live: backend message; sim: demo text). */
  detail?: string
}

export function useVehicleSetup(vehicleCount: number) {
  const [conn, setConn] = useState<'connecting' | 'live' | 'simulated'>('connecting')
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryAt, setRetryAt] = useState<number | null>(null)
  const [index, setIndex] = useState(0)
  const [summary, setSummary] = useState<VehicleSetupSummary | null>(null)
  const [paramStore, setParamStore] = useState<ParamStoreView | null>(null)
  const [groups, setGroups] = useState<AirframeGroup[]>(MOCK_AIRFRAME_GROUPS)
  const [modes, setModes] = useState<ModeEntry[]>(MOCK_MODES)
  const [busy, setBusy] = useState(false)

  const engineRef = useRef<MockSetupEngine | null>(null)
  const connRef = useRef<'connecting' | 'live' | 'simulated'>('connecting')
  const idxRef = useRef(0)

  const setIndexBoth = useCallback((i: number) => {
    idxRef.current = i
    setIndex(i)
  }, [])

  const setConnBoth = useCallback((c: 'connecting' | 'live' | 'simulated') => {
    connRef.current = c
    setConn(c)
  }, [])

  // -- live polling --------------------------------------------------------

  const pollOnce = useCallback(async () => {
    const i = idxRef.current
    try {
      const res = await fetchGw(gw(FLEET_PORT, `/api/vehicles/${i}/setup`), { method: 'GET' }, 2500)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const j = await res.json()
      const data = unwrapEnvelope(j)
      setSummary(normalizeSummary(data))
    } catch {
      // summary polling is best-effort; conn state is probed separately
    }
    try {
      const res = await fetchGw(gw(FLEET_PORT, `/api/vehicles/${i}/params`), { method: 'GET' }, 2500)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const j = await res.json()
      setParamStore(normalizeParamStore(unwrapEnvelope(j)))
    } catch {
      /* params polling is best-effort too */
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    let pollTimer: ReturnType<typeof setInterval> | null = null
    let retryTimer: ReturnType<typeof setInterval> | null = null

    const engine = new MockSetupEngine()
    engineRef.current = engine

    const stopPolling = () => {
      if (pollTimer) {
        clearInterval(pollTimer)
        pollTimer = null
      }
    }

    const startMock = (reason: string) => {
      if (cancelled) return
      stopPolling()
      setConnBoth('simulated')
      setLastError(reason)
      setRetryAt(Date.now() + 12000)
      if (!pollTimer) {
        const fresh = new MockSetupEngine()
        engineRef.current = fresh
        pollTimer = setInterval(() => {
          fresh.step()
          const v = fresh.vehicles[idxRef.current] ?? fresh.vehicles[0]
          if (v) {
            setSummary(v.summary())
            setParamStore(v.paramStoreView())
          }
        }, 1000)
      }
    }

    const goLive = async () => {
      if (cancelled) return
      stopPolling()
      setConnBoth('live')
      setLastError(null)
      setRetryAt(null)
      try {
        // catalog + modes are static-shaped: fetch once, keep mock shapes as fallback
        const [afRes, modeRes] = await Promise.all([
          fetchGw(gw(FLEET_PORT, '/api/airframes'), { method: 'GET' }, 2500),
          fetchGw(gw(FLEET_PORT, '/api/modes'), { method: 'GET' }, 2500),
        ])
        if (afRes.ok) {
          const j = unwrapEnvelope(await afRes.json()) as { groups?: AirframeGroup[] }
          if (Array.isArray(j.groups) && j.groups.length > 0) setGroups(j.groups)
        }
        if (modeRes.ok) {
          const j = unwrapEnvelope(await modeRes.json()) as { modes?: ModeEntry[] }
          if (Array.isArray(j.modes) && j.modes.length > 0) setModes(j.modes)
        }
      } catch {
        /* keep mock shapes */
      }
      await pollOnce()
      pollTimer = setInterval(() => void pollOnce(), 1000)
    }

    probePlane(gw(FLEET_PORT, '/api/vehicles/0/setup')).then((alive) => {
      if (cancelled) return
      if (alive) void goLive()
      else startMock('backend :8400 unreachable')
    })

    retryTimer = setInterval(() => {
      if (cancelled || connRef.current === 'live') return
      probePlane(gw(FLEET_PORT, '/api/vehicles/0/setup'), 2000).then((alive) => {
        if (cancelled || !alive || connRef.current === 'live') return
        void goLive()
      })
      if (connRef.current === 'simulated') setRetryAt(Date.now() + 12000)
    }, 12000)

    return () => {
      cancelled = true
      stopPolling()
      if (retryTimer) clearInterval(retryTimer)
    }
  }, [pollOnce, setConnBoth])

  // -- actions (live endpoints / mock engine) ------------------------------

  const act = useCallback(
    async (
      path: string,
      body: Record<string, unknown>,
      mock: () => SetupActionOutcome,
    ): Promise<SetupActionOutcome> => {
      setBusy(true)
      try {
        if (connRef.current === 'live') {
          try {
            const res = await fetchGw(gw(FLEET_PORT, path), {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(body),
            }, 5000)
            const j: unknown = await res.json().catch(() => null)
            const env = (j && typeof j === 'object' && 'ok' in (j as Record<string, unknown>)) ? (j as { ok: unknown; error?: unknown }) : null
            if (!res.ok || (env && env.ok === false)) {
              const msg = env?.error ?? `HTTP ${res.status}`
              return { ok: false, detail: String(msg) }
            }
            const d = (env && 'data' in (env as Record<string, unknown>)) ? unwrapEnvelope(j) : {}
            const data = (d ?? {}) as Record<string, unknown>
            return { ok: true, detail: detailFor(path, data) }
          } catch (e) {
            return { ok: false, detail: `network: ${e instanceof Error ? e.message : 'failed'}` }
          }
        }
        // simulated
        await new Promise((r) => setTimeout(r, 350)) // demo latency
        return mock()
      } finally {
        setBusy(false)
        // refresh the views right after an action in both modes
        if (connRef.current === 'live') void pollOnce()
      }
    },
    [pollOnce],
  )

  const refreshParams = useCallback(
    () =>
      act(`/api/vehicles/${idxRef.current}/params/refresh`, {}, () => {
        engineRef.current?.vehicles[idxRef.current]?.requestParamList()
        return { ok: true, detail: 'parameter download started (simulated)' }
      }),
    [act],
  )

  const writeParam = useCallback(
    (id: string, value: number) =>
      act(`/api/vehicles/${idxRef.current}/params`, { id, value }, () => {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, detail: 'no vehicle' }
        const r = v.writeParam(id, value)
        return { ok: r.ok, detail: r.ok ? `${id} = ${value} (echo confirmed, simulated)` : 'write rejected' }
      }),
    [act],
  )

  const applyAirframe = useCallback(
    (sysAutostart: number) =>
      act(`/api/vehicles/${idxRef.current}/airframe`, { sys_autostart: sysAutostart }, () => {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, detail: 'no vehicle' }
        const r = v.applyAirframe(sysAutostart)
        return {
          ok: r.ok,
          detail: r.ok
            ? `SYS_AUTOSTART=${sysAutostart} written — rebooting with new airframe (simulated)`
            : 'apply rejected (armed?)',
        }
      }),
    [act],
  )

  const calibrate = useCallback(
    (sensor: CalSensor) =>
      act(`/api/vehicles/${idxRef.current}/calibrate`, { sensor }, () => {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, detail: 'no vehicle' }
        const r = v.calibrate(sensor)
        return { ok: r.accepted, detail: r.accepted ? `${r.description} calibration accepted (simulated)` : 'rejected (armed?)' }
      }),
    [act],
  )

  const setMode = useCallback(
    (mode: string) =>
      act(`/api/vehicles/${idxRef.current}/mode`, { mode }, () => {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v || !v.setMode(mode)) return { ok: false, detail: 'mode rejected' }
        return { ok: true, detail: `mode ${mode} accepted (simulated)` }
      }),
    [act],
  )

  return {
    conn,
    port: FLEET_PORT,
    lastError,
    retryAt,
    index,
    setIndex: setIndexBoth,
    vehicleCount,
    summary,
    paramStore,
    groups,
    modes,
    busy,
    refreshParams,
    writeParam,
    applyAirframe,
    calibrate,
    setMode,
  }
}

export type VehicleSetupApi = ReturnType<typeof useVehicleSetup>

// ---------------------------------------------------------------------------
// tolerant normalizers (envelope shapes from ADR-0016's REST plane)
// ---------------------------------------------------------------------------

function num(v: unknown): number | null {
  if (typeof v === 'number' && Number.isFinite(v)) return v
  if (typeof v === 'string' && v.trim() !== '' && Number.isFinite(Number(v))) return Number(v)
  return null
}

function str(v: unknown, fallback = ''): string {
  return typeof v === 'string' && v.length > 0 ? v : fallback
}

function rec(v: unknown): Record<string, number | null> | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const r = v as Record<string, unknown>
  const out: Record<string, number | null> = {}
  for (const [k, val] of Object.entries(r)) out[k] = num(val)
  return out
}

function normalizeSummary(raw: unknown): VehicleSetupSummary | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const r = raw as Record<string, unknown>
  const air = (r.airframe && typeof r.airframe === 'object' ? r.airframe : {}) as Record<string, unknown>
  const cal = (r.calibration && typeof r.calibration === 'object' && !Array.isArray(r.calibration) ? r.calibration : null) as Record<string, unknown> | null
  const params = (r.params && typeof r.params === 'object' && !Array.isArray(r.params) ? r.params : null) as Record<string, unknown> | null
  const autopilot = (r.autopilot && typeof r.autopilot === 'object' ? r.autopilot : {}) as Record<string, unknown>
  return {
    index: num(r.index) ?? 0,
    sysid: num(r.sysid) ?? 1,
    compid: num(r.compid) ?? 1,
    fsm: str(r.fsm, 'INIT'),
    mode: str(r.mode, 'UNKNOWN'),
    mode_word: num(r.mode_word) ?? 0,
    armed: r.armed === true,
    battery_pct: num(r.battery_pct) ?? 0,
    voltage_v: num(r.voltage_v),
    restart_pending: r.restart_pending === true,
    autopilot: {
      type: str(autopilot.type, 'PX4'),
      version: str(autopilot.version, ''),
    },
    airframe: {
      sys_autostart: num(air.sys_autostart),
      name: str(air.name, 'Unknown airframe'),
      frame_type: str(air.frame_type),
      category: str(air.category, 'Other'),
      dynamics_compatible: air.dynamics_compatible === true,
    },
    params: params
      ? {
          total: num(params.total) ?? 0,
          received: num(params.received) ?? 0,
          state: str(params.state, 'Idle'),
        }
      : null,
    calibration: cal
      ? {
          accel: cal.accel === true ? true : cal.accel === false ? false : null,
          gyro: cal.gyro === true ? true : cal.gyro === false ? false : null,
          mag0: cal.mag0 === true ? true : cal.mag0 === false ? false : null,
          mag1: cal.mag1 === true ? true : cal.mag1 === false ? false : null,
          mag2: cal.mag2 === true ? true : cal.mag2 === false ? false : null,
          level_horizon: cal.level_horizon === true ? true : cal.level_horizon === false ? false : null,
        }
      : null,
    power: rec(r.power),
    safety: rec(r.safety),
  }
}

function normalizeParamStore(raw: unknown): ParamStoreView | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const r = raw as Record<string, unknown>
  const params = Array.isArray(r.params)
    ? r.params
        .map((p) => {
          const e = (p && typeof p === 'object' ? p : {}) as Record<string, unknown>
          const id = str(e.id)
          if (!id) return null
          return { id, value: num(e.value) ?? 0, type: num(e.type) ?? 9, index: num(e.index) ?? 0 }
        })
        .filter((p): p is ParamStoreView['params'][number] => p != null)
    : []
  return {
    total: num(r.total) ?? 0,
    received: num(r.received) ?? params.length,
    state: str(r.state, 'Idle'),
    requested_ms: num(r.requested_ms),
    last_value_ms: num(r.last_value_ms),
    params,
  }
}

/** Turn an action response body into a short toast detail. */
function detailFor(path: string, data: Record<string, unknown>): string | undefined {
  if (path.endsWith('/params/refresh')) {
    return data.requested === true ? 'PARAM_REQUEST_LIST sent — watch progress on this page' : 'request not queued'
  }
  if (path.endsWith('/params')) {
    const c = data.confirmed
    return c != null ? `${data.id} = ${c} (PARAM_VALUE echo confirmed)` : 'write UNCONFIRMED (no echo)'
  }
  if (path.endsWith('/airframe')) {
    return data.write_confirmed === true
      ? `SYS_AUTOSTART=${data.sys_autostart} written — restart queued, airframe loads at next boot`
      : 'write unconfirmed — restart NOT queued'
  }
  if (path.endsWith('/calibrate')) {
    return data.accepted === true
      ? `MAV_CMD 241 (${data.description}) ACCEPTED — CAL_* params update after the run`
      : `command not accepted (result=${data.result}) — disarmed + idle required`
  }
  if (path.endsWith('/mode')) {
    return data.accepted === true ? `mode ${data.mode} ACCEPTED` : `mode change not accepted (result=${data.result})`
  }
  return undefined
}
